# Spike: LiveKit Rust SDK

**Verdict: GO.** There's one Linux gotcha with a known workaround (PipeWire symbol clash, below).
Code: `crates/chad-media/examples/livekit_spike.rs`. It uses `livekit` 0.9.7 (libwebrtc 0.3.54, webrtc-sys 0.3.51) against LiveKit server 1.13.11 on the dev stack.

```sh
# Trust Caddy's internal CA (rustls-native-certs honours SSL_CERT_FILE):
podman exec irchad-ng-caddy-1 cat /data/caddy/pki/authorities/local/root.crt > caddy-root.crt
cat /etc/ssl/certs/ca-certificates.crt caddy-root.crt > ca-bundle.crt
SSL_CERT_FILE=$PWD/ca-bundle.crt cargo run --release -p chad-media --example livekit_spike
```

## Results (Linux, Hyprland, PipeWire 1.6.9)

```
RESULT APM echo cancellation (own I/O): ERLE 51.7 dB over last 2 s (ok)
libwebrtc pipewire stubs initialized: true
pipewire stream: Paused -> Streaming
spike-sub connected to room "spike"
spike-pub published 440 Hz tone
spike-sub subscribed to "tone" from spike-pub
received 9.96s of audio (478080 samples); after warm-up: ~440.0 Hz, rms 6957
RESULT tone round-trip: ok
RESULT pipewire playback: ok (491520 frames pulled by the graph)
```

- **Join and publish:** two participants join room `spike` via Caddy (`wss://localhost:8443/livekit`, with the prefix stripped).
  Tokens are minted locally with `livekit-api` (`access-token` feature only). The `NativeAudioSource` (48 kHz mono, 10 ms frames, 100 ms queue) publishes a 440 Hz sine.
- **Receive:** `NativeAudioStream` at 48 kHz mono gets 9.96 s of audio in a 10 s run. After warm-up it's still 440 Hz, and its RMS matches the input (0.3 × 32767/√2 ≈ 6950), so Opus and the network leave the signal intact.
- **PipeWire playback:** a native `pipewire` 0.10 output stream (`media.role=Communication`, S16LE mono 48 kHz, `RT_PROCESS`) reaches `Streaming`, and the graph pulls about 491k frames.
- **APM with our own I/O: reachable and working.** `livekit::webrtc::native::apm::AudioProcessingModule::new(aec, agc, hpf, ns)` exposes `process_reverse_stream` (far end: what we play), `process_stream` (near end: the mic) and `set_stream_delay_ms`, on i16 in 10 ms chunks.
  Fed a synthetic pure echo (far end delayed 20 ms at −6 dB), it removes **51.7 dB** after convergence. That's exactly the AEC-with-our-own-playback-mix setup the architecture wants.
  Note the `AudioSourceOptions { echo_cancellation, … }` on `NativeAudioSource` doesn't do this for us, because libwebrtc's own ADM isn't involved.
- **Windows build:** CI builds the example on the `windows-latest` runner, since `cargo test` builds examples (green in run 48).
  It needs `+crt-static`, because libwebrtc's prebuilt uses the static CRT (`/MT`). That's set in `.cargo/config.toml`.
  It's a build only: nothing runs it against a server on Windows yet.

## ⚠ PipeWire symbol clash (Linux)

The prebuilt `libwebrtc.a` contains webrtc's lazily-loaded PipeWire **stubs** for screen capture: weak `pw_init`, `pw_stream_new`, … that call through `pw_*_ptr` function pointers.
Because they're statically linked, those weak definitions win over `libpipewire-0.3.so`. So **pipewire-rs ends up calling the stubs**, and the first call (`pw_init`) jumps to a null pointer and segfaults.

**Workaround (in the spike):** call `bool webrtc::InitializePipeWire()` (mangled `_ZN6webrtc18InitializePipeWireEv`) once before `pw::init()`.
It's in `libwebrtc.a`, and it dlopens `libpipewire-0.3.so.0` and fills every stub pointer. After that the stubs forward to the real library, and functions webrtc doesn't stub resolve to libpipewire directly. Both paths reach the same loaded library.

Risks and alternatives, to settle in Phase 2:
- The mangled name is private ABI, so it could change with a webrtc-sys bump. Mitigation: the link fails loudly (undefined symbol) rather than silently, and `chad-media` gets a startup test that initializes PipeWire.
- Alternatively build libwebrtc with `rtc_link_pipewire=true` (no stubs) via `LK_CUSTOM_WEBRTC`. That's correct but means owning a libwebrtc build.
- Alternatively dlopen PipeWire ourselves rather than linking it, so pipewire-rs isn't used. That's more code.
- Rust-side wrappers: put the call in `chad-media`'s PipeWire backend constructor, `#[cfg(target_os = "linux")]`.

## Build and CI notes

- `livekit` features: `default-features = false, features = ["native", "rustls-tls-native-roots"]`. Without `native` you get "no network transport registered" at connect.
- webrtc-sys downloads a prebuilt libwebrtc (**444 MB extracted**) into `target/…/build/scratch-*/out/livekit_webrtc`. `Swatinem/rust-cache` caches `target/`, and the Gitea cache server works ("Cache saved successfully" on both runners in the first CI run).
- **webrtc-sys needs clang ≥ 21** (libwebrtc ships a hermetic libc++), and its build script checks the version. CI installs clang 21 from apt.llvm.org and sets `CC`/`CXX`, because Ubuntu 24.04's clang is 18.
- Don't set a global `RUSTFLAGS` in CI. It overrides the per-target rustflags in `.cargo/config.toml`.
- The only extra dynamic dependency on Linux is `libpipewire-0.3.so.0` (from pipewire-rs). The CI Linux job installs `libpipewire-0.3-dev`, `libclang-dev` and `clang` for bindgen.
- Media used UDP 50000–50100, published by compose with `--node-ip 127.0.0.1`. That's same-machine only, so set `LIVEKIT_NODE_IP` to test from another device.

## For Phase 2

- An SPSC ring (e.g. `rtrb`) replaces the spike's `Mutex<VecDeque>` between the network and the PipeWire RT thread.
- Mix all remote tracks ourselves (or use `livekit::webrtc::native::audio_mixer`). Feed the mix to PipeWire **and** to `apm.process_reverse_stream`. The mic goes through `process_stream` (plus optional `nnnoiseless`) and then into `NativeAudioSource`.
- Measure the real output latency from PipeWire (`pw_stream_get_time_n`) for `set_stream_delay_ms`.
