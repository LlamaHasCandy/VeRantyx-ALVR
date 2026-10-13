# VeRantyx ALVR bridge (first integration pass)

This folder adds an ALVR **20.14.1** transport adapter for the existing VeRantyx macOS HEVC encoder. It does not replace the SteamVR driver or capture proxy.

## Current architecture

```
SteamVR game
   -> VeRantyx OpenVR capture proxy
   -> MacEncoder / VideoToolbox HEVC
   -> local VRAN UDP datagrams (127.0.0.1:9998)
   -> this Rust bridge
   -> ALVR 20.14.1 control protocol + multiplexed video stream
   -> Quest ALVR client
```

The bridge uses ALVR's versioned `alvr_packets`, `alvr_session`, and `alvr_sockets` crates rather than hand-rolling the wire format. It performs the control handshake, negotiates an HEVC session, starts ALVR's UDP stream, sends decoder configuration, reassembles VeRantyx's local `VRAN` packets, and forwards completed frames as ALVR video packets. ALVR client IDR requests are forwarded back to the encoder.

## Build

From the repository root on an Apple Silicon Mac:

```bash
bash scripts/build_alvr_bridge.sh
```

Cargo fetches ALVR's open-source crates at tag `v20.14.1`. Build output is copied to `build/verantyx-alvr-bridge`.

## Run with the existing launcher

Install the matching ALVR **20.14.x** Quest client, open it, and leave it waiting for a streamer. Find the Quest's LAN IP in the headset/router, then run:

```bash
ALVR_CLIENT_IP=192.168.1.123 VERANTYX_TRANSPORT=alvr bash scripts/launch_vision_pro_vr.sh
```

Replace `192.168.1.123` with the Quest's actual IP. Optional configuration:

- `ALVR_STREAM_PORT=9942`
- `ALVR_EYE_WIDTH=960`
- `ALVR_EYE_HEIGHT=1080`
- `ALVR_FPS=72`
- `VERANTYX_ALVR_INGEST_PORT=9998`

The ALVR branch targets ALVR 20.14.1's Quest protocol and default ports. A protocol mismatch is reported instead of silently trying to decode incompatible packets.

## Explicit limitations of this first pass

- **Not end-to-end tested yet.** This change was written through the GitHub API; it has not been compiled on the user's Mac.
- The existing encoder produces a side-by-side HEVC frame. This bridge advertises 960×1080 per eye by default; the Quest's decoder/display behavior still needs a live test.
- Quest tracking packets are received for verification, but they are not yet copied into VeRantyx's shared-memory `SharedHands` structure. Head motion/controller input will therefore **not yet drive the game correctly**.
- Game audio and haptics are not yet bridged to ALVR streams. The current custom audio/frustum/status datagrams are ignored by the ALVR relay.
- ALVR v20.14.1 is pinned because the existing Quest client must match the bridge protocol. This does not claim compatibility with ALVR nightly/21.x clients.

The intent is to establish the real ALVR control/video transport first, then wire Quest tracking and controller state into the SteamVR driver and add ALVR audio/haptics.
