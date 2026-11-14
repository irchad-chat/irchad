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
  warning, only a line in the build script output (`cargo build -vv`).
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
- `src/nvidia/NvCodec/include/Utils/Logger.h`: `winsock2.h` instead of
  `winsock.h`, which clashes with webrtc's.

To see the full diff, compare with the registry copy:
`diff -r ~/.cargo/registry/src/index.crates.io-*/webrtc-sys-0.3.51 vendor/webrtc-sys`.

Bumping `livekit` means re-vendoring the new webrtc-sys and reapplying these
changes, or dropping the patch once upstream builds NVENC on Windows.
Check with `cargo run -p chad-media --example encoder_check`. On an Nvidia box
it should print `NVIDIA AV1 Encoder (hardware: true)`. With `CODEC=h264` it
should print `NVIDIA H264 Encoder (hardware: true)` at 1920x1080.
