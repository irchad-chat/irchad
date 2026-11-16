# Vendored crates

## webrtc-sys 0.3.51

crates.io's webrtc-sys, used through `[patch.crates-io]` in the root
`Cargo.toml`, with NVENC built on Windows too (D89). Upstream compiles its
`src/nvidia` encoder on Linux only.

Our changes (each is marked `irchad:` in the source):

- `build.rs`: on Windows, when `CUDA_HOME` or `CUDA_PATH` has `include/cuda.h`,
  compile the NVENC encoder (H.264, H.265, AV1) and define
  `USE_NVIDIA_VIDEO_CODEC`. Without it, a cargo warning and no NVENC, as on
  Linux. `scripts/cuda-headers.ps1` fetches the header. NVDEC stays Linux-only
  behind a new `USE_NVIDIA_VIDEO_DECODER`, since it would need the Video Codec
  SDK's `nvcuvid.lib`. On Linux, a missing `cuda.h` is no longer a cargo
  warning, only a line in the build script output (`cargo build -vv`), and
  changing `CUDA_HOME` reruns the build script.
- `src/nvidia/cuda_loader_win.cpp` (new): defines the `cu*` driver calls the
  encoder makes and forwards them to `nvcuda.dll`, loaded on first use. It's
  the Windows twin of the Linux implib trampolines in
  `src/lazy_load_deps_for/nvidia`. Nothing links `nvcuda.lib`, so `irchad.exe`
  still starts without an Nvidia driver.
- `src/nvidia/nvidia_encoder_factory.cpp`: the NVENC probe loads
  `nvEncodeAPI64.dll` with `LoadLibrary` instead of `dlopen`.
- `src/video_decoder_factory.cpp`: the NVDEC factory is gated on
  `USE_NVIDIA_VIDEO_DECODER`.
- `src/nvidia/h264_encoder_impl.cpp`: the level is always
  `NV_ENC_LEVEL_AUTOSELECT`. Upstream copies it from the SDP (3.1 in
  `42e01f`), which fails at 1080p and falls back to OpenH264. This change
  isn't Windows-specific.
- `src/webrtc.rs`, `include/livekit/webrtc.h`, `src/video_encoder_factory.cpp`,
  `src/nvidia/nvidia_encoder_factory.h` (`IsAv1Supported` made public):
  a new `nvenc_av1_supported()` FFI call. The merged codec list can't tell
  NVENC's AV1 from libaom's, and pre-Ada cards have NVENC without AV1, so
  screen share asks for H.264 over NVENC there (D92).
- `src/amf/` (new) and `build.rs`: an AMD AMF encoder for Windows (D104),
  H.264 and AV1. `AMF/` holds AMD's SDK headers (v1.5.3, MIT, `core/` and the
  six `components/` headers the encoders need, unmodified). `amf_runtime.cpp`
  loads `amfrt64.dll` from the driver and makes a context on the AMD adapter,
  `amf_encoder.cpp` is the encoder, and `amf_encoder_factory.cpp` the factory
  and its probe. `build.rs` always compiles them on Windows and defines
  `USE_AMF_VIDEO_CODEC`.
- `src/video_encoder_factory.cpp`, `src/webrtc.rs`, `include/livekit/webrtc.h`:
  the AMF factory is registered as the `Hardware` backend after NVENC and
  VAAPI, and `amf_supported()` / `amf_av1_supported()` are new FFI calls.
- `src/nvidia/NvCodec/include/Utils/Logger.h`: `winsock2.h` instead of
  `winsock.h`, which clashes with webrtc's.

To see the full diff, compare with the registry copy:
`diff -r ~/.cargo/registry/src/index.crates.io-*/webrtc-sys-0.3.51 vendor/webrtc-sys`.

Bumping `livekit` means re-vendoring the new webrtc-sys and reapplying these
changes, or dropping the patch once upstream builds NVENC on Windows.
Check with `cargo run -p chad-media --example encoder_check`. On an Nvidia box
it should print `NVIDIA AV1 Encoder (hardware: true)`. With `CODEC=h264` it
should print `NVIDIA H264 Encoder (hardware: true)` at 1920x1080. On a Radeon
under Windows, `BACKEND=amf` should print `AMD AMF AV1 Encoder` (or, with
`CODEC=h264` or on a card that pads AV1, `AMD AMF H264 Encoder`).
