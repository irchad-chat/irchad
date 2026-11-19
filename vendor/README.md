# Vendored crates

## webrtc-sys 0.3.51

crates.io's webrtc-sys, used through `[patch.crates-io]` in the root
`Cargo.toml`, with NVENC built on Windows too (D89). Upstream compiles its
`src/nvidia` encoder on Linux only.

Our changes (each is marked `irchad:` in the source):

- `build.rs`: on Windows, when `CUDA_HOME` or `CUDA_PATH` has `include/cuda.h`,
  compile the NVENC encoder (H.264, H.265, AV1) and the NVDEC decoder
  (H.264, H.265) and define `USE_NVIDIA_VIDEO_CODEC` and
  `USE_NVIDIA_VIDEO_DECODER`. Without it, a cargo warning and no NVENC/NVDEC,
  as on Linux. `scripts/cuda-headers.ps1` fetches the header. On Linux, a
  missing `cuda.h` is no longer a cargo warning, only a line in the build
  script output (`cargo build -vv`), and changing `CUDA_HOME` reruns the
  build script.
- `src/nvidia/cuda_loader_win.cpp` (new): defines the `cu*` driver calls the
  encoder makes and forwards them to `nvcuda.dll`, loaded on first use. It's
  the Windows twin of the Linux implib trampolines in
  `src/lazy_load_deps_for/nvidia`. Nothing links `nvcuda.lib`, so `irchad.exe`
  still starts without an Nvidia driver. The decoder's calls (`cuMemAlloc`,
  the stream ones) forward the same way.
- `src/nvidia/cuvid_loader_win.cpp` (new): the same for the NVDEC entry
  points, forwarded to `nvcuvid.dll` (which the driver installs), so nothing
  links `nvcuvid.lib` either. On x64 the header maps `cuvidMapVideoFrame` to
  `cuvidMapVideoFrame64`, which is the export defined.
- `src/nvidia/nvidia_encoder_factory.cpp`: the NVENC probe loads
  `nvEncodeAPI64.dll` with `LoadLibrary` instead of `dlopen`.
- `src/nvidia/nvidia_decoder_factory.cpp`: the NVDEC probe loads `nvcuvid.dll`
  with `LoadLibrary` instead of `dlopen`, so upstream's decoder (already in
  the tree, Linux-only until now) builds and runs on Windows.
- `src/video_decoder_factory.cpp`: the NVDEC factory is gated on
  `USE_NVIDIA_VIDEO_DECODER`; the AMF decoder factory is registered after it
  under `USE_AMF_VIDEO_CODEC`, so a hybrid box decodes on its Nvidia half;
  `QueryCodecSupport` now answers `is_power_efficient` for a hardware
  factory's formats, which is where the receive stats'
  `powerEfficientDecoder` comes from; and `nvdec_supported()` /
  `amf_decoder_supported()` / `amf_decoder_av1_supported()` are new FFI
  calls (with `src/webrtc.rs`, `include/livekit/webrtc.h`).
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
  H.264 and AV1, and an AMF decoder for the same two codecs.
  `AMF/` holds AMD's SDK headers (v1.5.3, MIT, `core/`, the `components/`
  headers the encoders need and `components/VideoDecoderUVD.h` for the
  decoder, unmodified). `amf_runtime.cpp` loads `amfrt64.dll` from the driver
  and makes a context on the AMD adapter, `amf_encoder.cpp` is the encoder
  with `amf_encoder_factory.cpp` its factory and probe; `amf_decoder.cpp` is
  the decoder (Annex B H.264 or low-overhead AV1 OBUs in through host
  buffers, NV12 out through a host copy and one libyuv pass) with
  `amf_decoder_factory.cpp` its factory and probe. `build.rs` always compiles
  them on Windows and defines `USE_AMF_VIDEO_CODEC`.
- `src/video_encoder_factory.cpp`, `src/webrtc.rs`, `include/livekit/webrtc.h`:
  the AMF factory is registered as the `Hardware` backend after NVENC and
  VAAPI, and `amf_supported()` / `amf_av1_supported()` are new FFI calls.
- `src/nvidia/NvCodec/include/Utils/Logger.h`: `winsock2.h` instead of
  `winsock.h`, which clashes with webrtc's.
- `src/nvidia/{h264,h265,av1}_encoder_impl.cpp`, `src/amf/amf_encoder.cpp`,
  `src/vaapi/h264_encoder_impl.cpp`: `SetRates` keeps the frame rate from
  `InitEncode` instead of libwebrtc's measured input rate. Their CBR budgets
  each frame as bitrate / frame rate, so after a capture stall (a low
  measured rate) the frames that follow overshot the bitrate 2–3× and sat
  in the pacer for up to 0.7 s. `DIP=1 encoder_check` reproduces it
  (`docs/notes/stream-stutter.md`).
- `src/amf/amf_decoder.cpp`: `SubmitInput` returning
  `AMF_DECODER_NO_FREE_SURFACES` is handled like `AMF_INPUT_FULL` (drain,
  then retry), and `AMF_RESOLUTION_UPDATED` counts as accepted. Both used to
  fail the decode, so WebRTC asked for a key frame and the picture froze
  until it came.
- `src/webrtc.rs`, `include/livekit/webrtc.h`, `src/webrtc.cpp`: a new
  `new_log_sink_at(fnc, min)` FFI call, a log sink for `min` and above only.
  The `libwebrtc` crate forwards all of libwebrtc's log at `debug`, which the
  app's `info` filter drops, decoder errors included. chad-media adds a
  second sink for warnings and errors (`monitor::libwebrtc_warnings`).

To see the full diff, compare with the registry copy:
`diff -r ~/.cargo/registry/src/index.crates.io-*/webrtc-sys-0.3.51 vendor/webrtc-sys`.

Bumping `livekit` means re-vendoring the new webrtc-sys and reapplying these
changes, or dropping the patch once upstream builds NVENC on Windows.
Check with `cargo run -p chad-media --example encoder_check`. On an Nvidia box
it should print `NVIDIA AV1 Encoder (hardware: true)`. With `CODEC=h264` it
should print `NVIDIA H264 Encoder (hardware: true)` at 1920x1080. On a Radeon
under Windows, `BACKEND=amf` should print `AMD AMF AV1 Encoder` (or, with
`CODEC=h264` or on a card that pads AV1, `AMD AMF H264 Encoder`).
The decode side: `cargo run -p chad-media --example decoder_check`. On an
Nvidia box (Windows needs the CUDA headers at build time) it should print
`NVIDIA H264 Decoder (hardware: true)` for H.264; on a Radeon,
`AMD AMF H264 Decoder` or, for AV1 on a card that decodes it,
`AMD AMF AV1 Decoder`. In the app, the stream stats overlay names the decoder
a watched share actually runs.
- `src/nvidia/av1_encoder_impl.{h,cpp}`, `src/nvidia/nvidia_encoder_factory.cpp`:
  NVENC AV1 offers `L1T2` besides `L1T1` (D107). In `L1T2`, picture type
  decision moves to the client (`enablePTD = 0`) and every other frame is
  encoded with `refPicFlag = 0`, so nothing references it; libwebrtc's
  `ScalabilityStructureL1T2` describes that in the dependency descriptor.
  `temporalId` stays 0, so the bitstream has no layer markers and any AV1
  decoder takes it. The layer is decided before each frame is encoded
  (it used to be after), and a frame the controller handed out that never
  reaches the callback forces a key frame, as the descriptor's frame diffs
  would no longer match.
