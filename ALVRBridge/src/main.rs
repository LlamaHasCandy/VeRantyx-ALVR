use anyhow::{bail, Context, Result};
use alvr_common::{
    glam::UVec2, protocol_id_u64, ButtonInfo, DeviceMotion, BUTTON_INFO, HAND_LEFT_ID,
    HAND_RIGHT_ID, HEAD_ID,
};
use alvr_packets::{
    decode_video_streaming_capabilities, encode_stream_config, ClientConnectionResult,
    ButtonEntry, ButtonValue, ClientControlPacket, DecoderInitializationConfig,
    NegotiatedStreamingConfig, ServerControlPacket, Tracking, VideoPacketHeader, TRACKING, VIDEO,
};
use alvr_session::{CodecType, SessionConfig, SocketProtocol};
use alvr_sockets::{PeerType, ProtoControlSocket, StreamSocketBuilder};
use clap::Parser;
use std::{
    collections::HashMap,
    io::ErrorKind,
    net::{IpAddr, SocketAddr, UdpSocket},
    thread,
    time::{Duration, Instant},
};

const VRAN_MAGIC: u32 = 0x5652_414E;
const VRAN_HEADER_BYTES: usize = 96;
const CONTROL_PORT: u16 = 9943;
const DEFAULT_STREAM_PORT: u16 = 9942;
const DEFAULT_INGEST_PORT: u16 = 9998;
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_ACTIVE_FRAMES: usize = 8;

#[derive(Parser, Debug)]
#[command(name = "verantyx-alvr-bridge", about = "ALVR 20.14.1 transport for VeRantyx")]
struct Args {
    /// IP address of the Quest with the ALVR client open and waiting for a streamer.
    #[arg(long, env = "ALVR_CLIENT_IP")]
    client_ip: IpAddr,

    /// ALVR stream port.
    #[arg(long, default_value_t = DEFAULT_STREAM_PORT)]
    stream_port: u16,

    /// Local UDP port that receives VeRantyx's existing VRAN-framed HEVC output.
    #[arg(long, default_value_t = DEFAULT_INGEST_PORT, env = "VERANTYX_ALVR_INGEST_PORT")]
    ingest_port: u16,

    /// Per-eye resolution negotiated with the ALVR client.
    #[arg(long, default_value_t = 960)]
    eye_width: u32,

    #[arg(long, default_value_t = 1080)]
    eye_height: u32,

    #[arg(long, default_value_t = 72.0)]
    fps: f32,
}

struct FrameAssembly {
    chunks: Vec<Option<Vec<u8>>>,
    received_chunks: usize,
    total_bytes: usize,
    timestamp: f64,
    last_update: Instant,
}

struct EncodedFrame {
    sequence: u32,
    timestamp: f64,
    data: Vec<u8>,
}

#[derive(Default)]
struct FrameAssembler {
    frames: HashMap<u32, FrameAssembly>,
}

impl FrameAssembler {
    fn ingest(&mut self, datagram: &[u8]) -> Option<EncodedFrame> {
        if datagram.len() < VRAN_HEADER_BYTES || read_u32(datagram, 0)? != VRAN_MAGIC {
            return None;
        }

        let sequence = read_u32(datagram, 4)?;
        let chunk_index = read_u32(datagram, 8)? as usize;
        let total_chunks = read_u32(datagram, 12)? as usize;
        let payload_size = read_u32(datagram, 20)? as usize;
        let timestamp = f64::from_le_bytes(datagram.get(24..32)?.try_into().ok()?);

        if total_chunks == 0
            || total_chunks > 16_384
            || chunk_index >= total_chunks
            || payload_size != datagram.len() - VRAN_HEADER_BYTES
            || payload_size == 0
            || total_chunks.saturating_mul(payload_size) > MAX_FRAME_BYTES * 2
            || !timestamp.is_finite()
        {
            return None;
        }

        let now = Instant::now();
        self.frames
            .retain(|_, frame| now.duration_since(frame.last_update) < Duration::from_secs(2));

        if !self.frames.contains_key(&sequence) && self.frames.len() >= MAX_ACTIVE_FRAMES {
            if let Some(oldest) = self
                .frames
                .iter()
                .min_by_key(|(_, frame)| frame.last_update)
                .map(|(sequence, _)| *sequence)
            {
                self.frames.remove(&oldest);
            }
        }

        let frame = self.frames.entry(sequence).or_insert_with(|| FrameAssembly {
            chunks: vec![None; total_chunks],
            received_chunks: 0,
            total_bytes: 0,
            timestamp,
            last_update: now,
        });

        if frame.chunks.len() != total_chunks || frame.chunks[chunk_index].is_some() {
            return None;
        }

        let payload = datagram[VRAN_HEADER_BYTES..].to_vec();
        if frame.total_bytes.saturating_add(payload.len()) > MAX_FRAME_BYTES {
            self.frames.remove(&sequence);
            return None;
        }

        frame.total_bytes += payload.len();
        frame.chunks[chunk_index] = Some(payload);
        frame.received_chunks += 1;
        frame.last_update = now;

        if frame.received_chunks != frame.chunks.len() {
            return None;
        }

        let frame = self.frames.remove(&sequence)?;
        let mut data = Vec::with_capacity(frame.total_bytes);
        for chunk in frame.chunks {
            data.extend_from_slice(chunk.as_deref()?);
        }

        Some(EncodedFrame {
            sequence,
            timestamp: frame.timestamp,
            data,
        })
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}

fn annex_b_ranges(frame: &[u8]) -> Vec<(usize, usize, usize)> {
    let mut starts = Vec::<(usize, usize)>::new();
    let mut i = 0;
    while i + 3 < frame.len() {
        if frame[i..].starts_with(&[0, 0, 0, 1]) {
            starts.push((i, 4));
            i += 4;
        } else if frame[i..].starts_with(&[0, 0, 1]) {
            starts.push((i, 3));
            i += 3;
        } else {
            i += 1;
        }
    }

    starts
        .iter()
        .enumerate()
        .filter_map(|(index, (start, code_len))| {
            let nal_start = *start + *code_len;
            let end = starts.get(index + 1).map_or(frame.len(), |(next, _)| *next);
            (nal_start < end).then_some((*start, nal_start, end))
        })
        .collect()
}

fn is_idr_frame(frame: &[u8]) -> bool {
    annex_b_ranges(frame).iter().any(|(_, start, end)| {
        start < end && matches!((frame[*start] >> 1) & 0x3f, 16..=21)
    })
}

fn hevc_parameter_sets(frame: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    for (start, nal_start, end) in annex_b_ranges(frame) {
        if nal_start >= end {
            continue;
        }
        if matches!((frame[nal_start] >> 1) & 0x3f, 32..=34) {
            output.extend_from_slice(&frame[start..end]);
        }
    }
    output
}

fn send_keyframe_request(socket: &UdpSocket, encoder_peer: Option<SocketAddr>) {
    if let Some(peer) = encoder_peer {
        let _ = socket.send_to(b"REQUEST_KEYFRAME", peer);
    }
}

#[derive(Clone)]
struct InputState {
    left_trigger: f32,
    right_trigger: f32,
    left_grip: f32,
    right_grip: f32,
    left_stick_x: f32,
    left_stick_y: f32,
    right_stick_x: f32,
    right_stick_y: f32,
    left_buttons: u32,
    right_buttons: u32,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            left_trigger: 0.0,
            right_trigger: 0.0,
            left_grip: 0.0,
            right_grip: 0.0,
            left_stick_x: 0.0,
            left_stick_y: 0.0,
            right_stick_x: 0.0,
            right_stick_y: 0.0,
            left_buttons: 0,
            right_buttons: 0,
        }
    }
}

impl InputState {
    fn apply(&mut self, entries: &[ButtonEntry]) {
        for entry in entries {
            let Some(ButtonInfo { path, device_id, .. }) = BUTTON_INFO.get(&entry.path_id) else {
                continue;
            };
            let is_left = *device_id == *HAND_LEFT_ID;
            let is_right = *device_id == *HAND_RIGHT_ID;
            if !is_left && !is_right {
                continue;
            }
            match (path.rsplit("/input/").next().unwrap_or(""), entry.value) {
                ("trigger/value", ButtonValue::Scalar(value)) => {
                    if is_left { self.left_trigger = value.clamp(0.0, 1.0); }
                    else { self.right_trigger = value.clamp(0.0, 1.0); }
                }
                ("squeeze/value", ButtonValue::Scalar(value)) => {
                    if is_left { self.left_grip = value.clamp(0.0, 1.0); }
                    else { self.right_grip = value.clamp(0.0, 1.0); }
                }
                ("thumbstick/x", ButtonValue::Scalar(value)) => {
                    if is_left { self.left_stick_x = value.clamp(-1.0, 1.0); }
                    else { self.right_stick_x = value.clamp(-1.0, 1.0); }
                }
                ("thumbstick/y", ButtonValue::Scalar(value)) => {
                    if is_left { self.left_stick_y = value.clamp(-1.0, 1.0); }
                    else { self.right_stick_y = value.clamp(-1.0, 1.0); }
                }
                (path, ButtonValue::Binary(value)) => {
                    let bit = if is_left {
                        match path {
                            "x/click" => Some(1u32 << 8),
                            "y/click" => Some(1u32 << 9),
                            "thumbstick/click" => Some(1u32 << 10),
                            "thumbstick/touch" => Some(1u32 << 11),
                            "menu/click" | "system/click" => Some(1u32 << 12),
                            _ => None,
                        }
                    } else {
                        match path {
                            "a/click" => Some(1u32),
                            "b/click" => Some(1u32 << 1),
                            "thumbstick/click" => Some(1u32 << 2),
                            "thumbstick/touch" => Some(1u32 << 3),
                            "menu/click" | "system/click" => Some(1u32 << 4),
                            _ => None,
                        }
                    };
                    if let Some(bit) = bit {
                        let buttons = if is_left { &mut self.left_buttons } else { &mut self.right_buttons };
                        if value { *buttons |= bit; } else { *buttons &= !bit; }
                    }
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone)]
struct PoseState {
    timestamp: f64,
    head: [f32; 16],
    left: [f32; 16],
    right: [f32; 16],
    left_linear: [f32; 3],
    right_linear: [f32; 3],
    left_angular: [f32; 3],
    right_angular: [f32; 3],
    head_tracked: bool,
    left_tracked: bool,
    right_tracked: bool,
    velocity_valid: bool,
}

impl Default for PoseState {
    fn default() -> Self {
        Self {
            timestamp: 0.0,
            head: identity_matrix(),
            left: identity_matrix(),
            right: identity_matrix(),
            left_linear: [0.0; 3],
            right_linear: [0.0; 3],
            left_angular: [0.0; 3],
            right_angular: [0.0; 3],
            head_tracked: false,
            left_tracked: false,
            right_tracked: false,
            velocity_valid: false,
        }
    }
}

fn identity_matrix() -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    ]
}

fn motion_for(tracking: &Tracking, device_id: u64) -> Option<DeviceMotion> {
    tracking.device_motions.iter().find_map(|(id, motion)| {
        (*id == device_id).then_some(*motion)
    })
}

fn transform_for(motion: DeviceMotion) -> [f32; 16] {
    let p = motion.pose.position;
    let q = motion.pose.orientation;
    let (x, y, z, w) = (q.x, q.y, q.z, q.w);
    [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        0.0,
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        0.0,
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
        0.0,
        p.x, p.y, p.z, 1.0,
    ]
}

impl PoseState {
    fn update(&mut self, tracking: &Tracking) {
        self.timestamp = tracking.target_timestamp.as_secs_f64();
        let head = motion_for(tracking, *HEAD_ID);
        let left = motion_for(tracking, *HAND_LEFT_ID);
        let right = motion_for(tracking, *HAND_RIGHT_ID);

        self.head_tracked = head.is_some();
        self.left_tracked = left.is_some();
        self.right_tracked = right.is_some();

        if let Some(motion) = head {
            self.head = transform_for(motion);
        }
        if let Some(motion) = left {
            self.left = transform_for(motion);
            self.left_linear = [motion.linear_velocity.x, motion.linear_velocity.y, motion.linear_velocity.z];
            self.left_angular = [motion.angular_velocity.x, motion.angular_velocity.y, motion.angular_velocity.z];
        } else {
            self.left_linear = [0.0; 3];
            self.left_angular = [0.0; 3];
        }
        if let Some(motion) = right {
            self.right = transform_for(motion);
            self.right_linear = [motion.linear_velocity.x, motion.linear_velocity.y, motion.linear_velocity.z];
            self.right_angular = [motion.angular_velocity.x, motion.angular_velocity.y, motion.angular_velocity.z];
        } else {
            self.right_linear = [0.0; 3];
            self.right_angular = [0.0; 3];
        }
        self.velocity_valid = left.is_some() || right.is_some();
    }

    fn encode_packet(&self, inputs: &InputState) -> Vec<u8> {
        // Fixed little-endian "QTRK" packet consumed by HardwareEncoder.swift.
        // 4-byte magic + 8-byte timestamp + 3 matrices + velocities + inputs + tracking flags.
        let mut bytes = Vec::with_capacity(284);
        bytes.extend_from_slice(b"QTRK");
        bytes.extend_from_slice(&self.timestamp.to_le_bytes());
        for matrix in [&self.head, &self.left, &self.right] {
            for value in matrix {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        for velocity in [&self.left_linear, &self.right_linear, &self.left_angular, &self.right_angular] {
            for value in velocity {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&[
            (inputs.left_trigger * 255.0).round() as u8,
            (inputs.right_trigger * 255.0).round() as u8,
            (inputs.left_grip * 255.0).round() as u8,
            (inputs.right_grip * 255.0).round() as u8,
        ]);
        bytes.extend_from_slice(&inputs.right_buttons.to_le_bytes());
        bytes.extend_from_slice(&inputs.left_buttons.to_le_bytes());
        for value in [
            inputs.right_stick_x, inputs.right_stick_y,
            inputs.left_stick_x, inputs.left_stick_y,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[
            u8::from(self.head_tracked),
            u8::from(self.left_tracked),
            u8::from(self.right_tracked),
            u8::from(self.velocity_valid),
        ]);
        bytes
    }
}

fn publish_pose_update(socket: &UdpSocket, pose: &PoseState, inputs: &InputState) {
    let packet = pose.encode_packet(inputs);
    let _ = socket.send_to(&packet, ("127.0.0.1", 11_003));
}

fn configure_alvr_session(args: &Args) -> Result<SessionConfig> {
    let mut session = SessionConfig::default();

    // Keep ALVR's official session shape/defaults but select HEVC and match the
    // stream port used by the Quest client.
    let mut json = serde_json::to_value(&session)?;
    json["session_settings"]["video"]["preferred_codec"] = serde_json::json!("Hevc");
    json["session_settings"]["video"]["preferred_fps"] = serde_json::json!(args.fps);
    json["session_settings"]["connection"]["stream_protocol"] = serde_json::json!("Udp");
    json["session_settings"]["connection"]["stream_port"] = serde_json::json!(args.stream_port);
    session.merge_from_json(&json)?;

    session.openvr_config.eye_resolution_width = args.eye_width;
    session.openvr_config.eye_resolution_height = args.eye_height;
    session.openvr_config.target_eye_resolution_width = args.eye_width;
    session.openvr_config.target_eye_resolution_height = args.eye_height;
    session.openvr_config.refresh_rate = args.fps.round() as u32;
    Ok(session)
}

fn connect_to_quest(client_ip: IpAddr) -> Result<(ProtoControlSocket, IpAddr)> {
    loop {
        match ProtoControlSocket::connect_to(
            Duration::from_secs(2),
            PeerType::AnyClient(vec![client_ip]),
        ) {
            Ok(pair) => return Ok(pair),
            Err(error) => {
                eprintln!("ALVR: waiting for Quest client at {client_ip}:{CONTROL_PORT} ({error:#})");
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

fn run(args: Args) -> Result<()> {
    let ingest_socket = UdpSocket::bind(("127.0.0.1", args.ingest_port))
        .with_context(|| format!("binding encoder ingress on 127.0.0.1:{}", args.ingest_port))?;
    ingest_socket.set_nonblocking(true)?;

    println!("VeRantyx ALVR bridge 0.1 — protocol 20.14.1");
    println!("Listening for HardwareEncoder on 127.0.0.1:{}", args.ingest_port);
    println!("Open the ALVR Quest client and leave it on the waiting-for-streamer screen.");

    let (mut proto_socket, quest_ip) = connect_to_quest(args.client_ip)?;
    let result: ClientConnectionResult = proto_socket
        .recv(Duration::from_secs(3))
        .map_err(|error| anyhow::anyhow!("waiting for ALVR client capabilities: {error}"))?;

    let (client_protocol, client_name, capabilities) = match result {
        ClientConnectionResult::ConnectionAccepted {
            client_protocol_id,
            display_name,
            streaming_capabilities,
            ..
        } => (client_protocol_id, display_name, streaming_capabilities),
        ClientConnectionResult::ClientStandby => bail!("Quest ALVR client entered standby"),
    };

    if client_protocol != protocol_id_u64() {
        bail!(
            "ALVR protocol mismatch: Quest protocol ID {client_protocol}, bridge protocol ID {}. Install a matching ALVR 20.14.x Quest client.",
            protocol_id_u64()
        );
    }

    if let Some(capabilities) = capabilities {
        let capabilities = decode_video_streaming_capabilities(&capabilities)?;
        println!(
            "Quest {client_name}: default view {}x{}, refresh rates {:?}, 10-bit support={}",
            capabilities.default_view_resolution.x,
            capabilities.default_view_resolution.y,
            capabilities.supported_refresh_rates,
            capabilities.encoder_10_bits
        );
    } else {
        println!("Quest {client_name} did not report optional streaming capabilities.");
    }

    let session = configure_alvr_session(&args)?;
    let settings = session.to_settings();
    let negotiated = NegotiatedStreamingConfig {
        view_resolution: UVec2::new(args.eye_width, args.eye_height),
        refresh_rate_hint: args.fps,
        game_audio_sample_rate: 0,
        enable_foveated_encoding: false,
        use_multimodal_protocol: false,
        use_full_range: false,
        encoding_gamma: 1.0,
        enable_hdr: false,
        wired: false,
    };
    let stream_config = encode_stream_config(&session, &negotiated)?;

    proto_socket.send(&stream_config)?;
    let (mut control_sender, mut control_receiver) =
        proto_socket.split(Duration::from_millis(5))?;
    control_sender.send(&ServerControlPacket::StartStream)?;

    match control_receiver.recv(Duration::from_secs(3)).map_err(|error| anyhow::anyhow!("waiting for Quest StreamReady: {error}"))? {
        ClientControlPacket::StreamReady => println!("ALVR: Quest stream socket is ready."),
        _ => bail!("Quest sent an unexpected packet instead of StreamReady"),
    }

    let mut stream_socket = StreamSocketBuilder::connect_to_client(
        Duration::from_millis(1),
        quest_ip,
        settings.connection.stream_port,
        SocketProtocol::Udp,
        None,
        settings.connection.server_send_buffer_bytes,
        settings.connection.server_recv_buffer_bytes,
        settings.connection.packet_size.max(1400) as usize,
    ).map_err(|error| anyhow::anyhow!("connecting ALVR stream socket: {error}"))?;
    let mut video_sender = stream_socket.request_stream::<VideoPacketHeader>(VIDEO);
    let mut tracking_receiver = stream_socket.subscribe_to_stream::<Tracking>(TRACKING, 4);
    let stream_epoch = Instant::now();

    println!(
        "ALVR stream ready: Quest={quest_ip}, stream port={}, eye={}x{} @ {} Hz",
        settings.connection.stream_port, args.eye_width, args.eye_height, args.fps
    );
    println!("Quest tracking and controller updates are forwarded to HardwareEncoder over localhost UDP; live validation is still required.");

    let mut assembler = FrameAssembler::default();
    let mut datagram = [0u8; 65_535];
    let mut encoder_peer: Option<SocketAddr> = None;
    let mut decoder_config_sent = false;
    let mut last_keepalive = Instant::now();
    let mut latest_pose = PoseState::default();
    let mut latest_inputs = InputState::default();
    let mut last_stats = Instant::now();
    let mut frames_at_last_stats = 0u64;
    let mut sent_frames = 0u64;
    let mut tracking_packets = 0u64;

    loop {
        // Drain local packets first so a large IDR burst does not sit behind
        // the ALVR control/tracking polling.
        loop {
            match ingest_socket.recv_from(&mut datagram) {
                Ok((size, peer)) => {
                    encoder_peer = Some(peer);
                    let Some(frame) = assembler.ingest(&datagram[..size]) else {
                        continue;
                    };
                    let idr = is_idr_frame(&frame.data);

                    if !decoder_config_sent {
                        if !idr {
                            send_keyframe_request(&ingest_socket, encoder_peer);
                            continue;
                        }
                        let config = hevc_parameter_sets(&frame.data);
                        if config.is_empty() {
                            eprintln!("ALVR: IDR omitted HEVC VPS/SPS/PPS; requesting a new IDR.");
                            send_keyframe_request(&ingest_socket, encoder_peer);
                            continue;
                        }
                        control_sender.send(&ServerControlPacket::DecoderConfig(
                            DecoderInitializationConfig {
                                codec: CodecType::Hevc,
                                config_buffer: config,
                            },
                        ))?;
                        decoder_config_sent = true;
                        println!("ALVR: sent HEVC decoder configuration.");
                    }

                    let header = VideoPacketHeader {
                        timestamp: stream_epoch.elapsed(),
                        is_idr: idr,
                    };
                    let mut buffer = video_sender.get_buffer(&header)?;
                    buffer.set_len(frame.data.len());
                    buffer
                        .get_range_mut(0, frame.data.len())
                        .copy_from_slice(&frame.data);
                    video_sender.send(buffer)?;
                    sent_frames += 1;

                    if sent_frames <= 3 {
                        println!(
                            "ALVR: forwarded frame {} ({} bytes, IDR={}, source sequence={})",
                            sent_frames,
                            frame.data.len(),
                            idr,
                            frame.sequence
                        );
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            }
        }

        // Poll Quest-to-host data. The tracking stream is parsed to verify the
        // return path; shared-memory pose injection is a separate follow-up.
        let _ = stream_socket.recv();
        while let Ok(tracking) = tracking_receiver.recv(Duration::from_millis(1)) {
            if let Ok((tracking, _payload)) = tracking.get() {
                latest_pose.update(&tracking);
                publish_pose_update(&ingest_socket, &latest_pose, &latest_inputs);
            }
            tracking_packets += 1;
        }

        if let Ok(packet) = control_receiver.recv(Duration::from_millis(1)) {
            match packet {
                ClientControlPacket::RequestIdr => send_keyframe_request(&ingest_socket, encoder_peer),
                ClientControlPacket::Buttons(entries) => {
                    latest_inputs.apply(&entries);
                    publish_pose_update(&ingest_socket, &latest_pose, &latest_inputs);
                }
                ClientControlPacket::KeepAlive
                | ClientControlPacket::Battery(_)
                | ClientControlPacket::ViewsConfig(_)
                | ClientControlPacket::PlayspaceSync(_)
                | ClientControlPacket::StreamReady
                | ClientControlPacket::VideoErrorReport
                | ClientControlPacket::ActiveInteractionProfile { .. }
                | ClientControlPacket::Reserved(_)
                | ClientControlPacket::ReservedBuffer(_) => {}
                ClientControlPacket::Log { level, message } => {
                    println!("ALVR client log ({level:?}): {}", message.replace('\n', " "));
                }
            }
        }

        if last_keepalive.elapsed() >= Duration::from_millis(500) {
            control_sender.send(&ServerControlPacket::KeepAlive)?;
            last_keepalive = Instant::now();
        }

        if last_stats.elapsed() >= Duration::from_secs(5) {
            let fps = (sent_frames - frames_at_last_stats) as f64
                / last_stats.elapsed().as_secs_f64();
            println!(
                "ALVR: {:.1} frames/s forwarded; {} tracking packets received",
                fps, tracking_packets
            );
            frames_at_last_stats = sent_frames;
            last_stats = Instant::now();
        }

        thread::sleep(Duration::from_micros(500));
    }
}

fn main() -> Result<()> {
    run(Args::parse())
}


#[cfg(test)]
mod tests {
    use super::*;

    fn vran_fragment(sequence: u32, index: u32, count: u32, payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![0u8; VRAN_HEADER_BYTES];
        packet[0..4].copy_from_slice(&VRAN_MAGIC.to_le_bytes());
        packet[4..8].copy_from_slice(&sequence.to_le_bytes());
        packet[8..12].copy_from_slice(&index.to_le_bytes());
        packet[12..16].copy_from_slice(&count.to_le_bytes());
        packet[16..20].copy_from_slice(&((index as usize * 1100) as u32).to_le_bytes());
        packet[20..24].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        packet[24..32].copy_from_slice(&1.25f64.to_le_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    #[test]
    fn qtrk_packet_layout_matches_swift_receiver_size() {
        let packet = PoseState::default().encode_packet(&InputState::default());
        assert_eq!(packet.len(), 284);
        assert_eq!(&packet[0..4], &b"QTRK"[..]);
    }

    #[test]
    fn reassembles_vran_fragments_even_when_out_of_order() {
        let mut assembler = FrameAssembler::default();
        assert!(assembler.ingest(&vran_fragment(7, 1, 2, b"lo")).is_none());
        let frame = assembler
            .ingest(&vran_fragment(7, 0, 2, b"hel"))
            .expect("second fragment should complete the frame");
        assert_eq!(frame.sequence, 7);
        assert_eq!(frame.data.as_slice(), &b"hello"[..]);
        assert!((frame.timestamp - 1.25).abs() < f64::EPSILON);
    }
}
