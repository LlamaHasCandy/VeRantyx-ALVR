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

The bridge uses ALVR's versioned `alvr_packets`, `alvr_session`, and `alvr_sockets` crates rather than hand-rolling the wire format. It performs the control handshake, negotiates an HEVC session, starts ALVR's UDP stream, sends decoder configuration, reassembles VeRantyx's local `VRAN` packets, and forwards completed frames as ALVR video packets. ALVR client IDR requests are forwarded back to the encoder. Quest tracking and controller input are translated into a small localhost `QTRK` packet, which the native Swift process writes into VeRantyx's existing shared `SharedHands` structure for the SteamVR driver.

## Build

From the repository root on an Apple Silicon Mac:

```bash
bash scripts/build_alvr_bridge.sh
```

Cargo fetches ALVR's open-source crates at tag `v20.14.1`. Build output is copied to `build/verantyx-alvr-bridge`. To run the small protocol-layout/reassembly tests before launching SteamVR, run `cargo test --manifest-path ALVRBridge/Cargo.toml`.

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

The ALVR branch targets ALVR 20.14.1's Quest protocol and default ports. A protocol mismatch is reported instead of silently trying to decode incompatible packets. The launcher defaults to the project's Wine 11 prefix. Set `CROSSOVER_BOTTLE` to use the experimental CrossOver runner; CrossOver's graphics backend and live headset path have not been validated.

## Explicit limitations of this first pass

- The bridge compiled successfully and its two Rust unit tests passed on the user's Mac. End-to-end headset streaming, stereo layout, and tracking still need live validation.
- The existing encoder produces a side-by-side HEVC frame. This bridge advertises 960×1080 per eye by default; the Quest's decoder/display behavior still needs a live test.
- The QTRK return path for Quest head/controller poses and buttons has been added, but its transform conventions and runtime behavior have **not been compiled or validated live**. Treat tracking/controller support as experimental until tested in-game.
- Game audio and haptics are not yet bridged to ALVR streams. The current custom audio/frustum/status datagrams are ignored by the ALVR relay. The bridge also currently falls back to VeRantyx's stored/default eye frustum rather than consuming ALVR's live `ViewsConfig`.
- ALVR v20.14.1 is pinned because the existing Quest client must match the bridge protocol. This does not claim compatibility with ALVR nightly/21.x clients.

This is an initial integration, not a finished replacement for ALVR. Next steps are compile fixes (if any), on-Mac packet testing, validating stereo layout and poses in SteamVR, then adding audio and haptics streams.

## CrossOver bottles (experimental)

The ALVR bridge and HEVC encoder remain native macOS processes. The existing
OpenVR capture proxy and SteamVR driver are installed into the Windows bottle;
no ALVR DLL is injected into SteamVR. CrossOver mode is experimental and has
not been validated with a live headset.

1. In CrossOver, create a 64-bit Windows bottle and install Windows Steam into
   it. Install SteamVR and a compatible SteamVR title into the same bottle.
   Do not run `scripts/setup.sh`; that script creates the project's Wine 11
   prefix instead.
2. Find the bottle name in CrossOver. The default private bottle location is
   `~/Library/Application Support/CrossOver/Bottles/<bottle-name>`.
3. From the repository root, launch with the bottle name and your Quest IP:

```bash
CROSSOVER_BOTTLE="Your Bottle Name" \
CROSSOVER_WINE_BIN="/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine" \
ALVR_CLIENT_IP=192.168.1.123 \
VERANTYX_TRANSPORT=alvr \
bash scripts/launch_vision_pro_vr.sh
```

If CrossOver is installed outside `/Applications`, set
`CROSSOVER_WINE_BIN` to its `Contents/SharedSupport/CrossOver/bin/wine`.
Set `WINEPREFIX` explicitly if the bottle is stored in a custom location.
The launcher uses CodeWeavers' `wine --bottle <name> --cx-app <executable>`
command interface through `VRDriver/wine_launcher.sh`. ALVR mode skips the
Vision Pro pose wait automatically because Quest tracking arrives through ALVR.

