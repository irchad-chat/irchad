# Screen share: copy costs and encoders

## Zero copy: not possible with this SDK

The plan's goal was DMA-BUF → VAAPI without touching the CPU. livekit 0.9.7 / webrtc-sys 0.3.51 can't do it:

- `NativeBuffer::from_dmabuf` exists, but only the Jetson (V4L2) encoder reads it. The VAAPI encoder (`webrtc-sys/src/vaapi`) takes I420 and uploads it itself, and every other encoder calls `ToI420()`.
- So frames have to be in CPU memory as I420 by the time they reach `NativeVideoSource` anyway.

What we do instead, per frame:

1. **Wayland:** the portal stream gives us linear DMA-BUF (mmap, bracketed with `DMA_BUF_IOCTL_SYNC`) or shared memory. We copy the (cropped) picture into our own buffer, because we draw the cursor from metadata onto it and the producer's buffer may be read-only.
   **X11 / Windows:** libwebrtc's capturer already hands us a CPU frame.
2. libyuv `ARGBToI420` (or `ABGRToI420` for RGBx), then `I420Scale` down to the preset if the source is bigger.
3. The encoder: VAAPI uploads the I420 surface; software encoders read it directly.

Measured on a Ryzen 7 7800X3D (`cargo test --release -p chad-media convert_cost -- --ignored --nocapture`):

| Step | 1080p | 4K (scaled to 1080p) |
|---|---|---|
| Frame copy | 0.17 ms | 0.82 ms |
| BGRA → I420 (+ scale) | 0.29 ms | 1.32 ms |

At 60 fps that's about 3 % of one core at 1080p and 13 % at 4K, so the copies aren't what limits us.

**Untested risk:** a linear DMA-BUF that lives in VRAM (a discrete AMD card) can be very slow to read through a CPU mapping, because it's uncached. If a tester sees low capture fps on such a card, drop the DMA-BUF format offer (`format_pod(fps, true)` in `platform/portal.rs`) so the compositor does its own GPU → SHM copy. The proper fix is a GPU import (EGL/Vulkan) with GPU colour conversion, which would also be the first step towards zero copy if the SDK's VAAPI encoder ever takes DMA-BUF.

The DMA-BUF path maps the buffer for every frame (a `shortcut:` in `copy_frame`). Cache the mapping per buffer if that shows up in a profile.

## Encoders

`crates/chad-media/examples/screen_check.rs` shares a moving 1080p test pattern through `Voice::share`, and a second `Voice` in the same process watches it. It reports the frames received, the watcher count and CPU per thread, and covers both ends' encode and decode.

```sh
SSL_CERT_FILE=$PWD/ca-bundle.crt cargo run --release -p chad-media --example screen_check
# PRESET=720p30|1080p60|source, CHECK_SECS=10
```

Results on 2026-10-29 (Ryzen 7 7800X3D with the Raphael iGPU as `renderD128`, plus an RTX 4090; Mesa radeonsi; dev LiveKit on loopback):

| Encoder (as logged) | Preset | Received | Encoder thread | Whole process |
|---|---|---|---|---|
| VAAPI H264 Encoder (radeonsi, hardware) | 1080p60 | 46–51 fps at 1920×1080 | 3 % | 56–60 % |
| VAAPI H264 Encoder (radeonsi, hardware) | 720p30 | 29 fps at 1280×720 | 1 % | 29 % |
| OpenH264 (software fallback) | 1080p60 | 55–60 fps at 1920×1080 | 39 % | 84 % |

"Whole process" includes generating the pattern (about 20 %) and the viewer's decode (about 15 %), so it overstates what a real sharer pays.

- **VAAPI saves a lot of CPU, but doesn't reach 60 fps at 1080p** on this iGPU through libwebrtc's VAAPI wrapper, which encodes synchronously. Check it on a real AMD/Intel discrete card before deciding anything. If it holds there too, prefer OpenH264 or VP9 at 1080p60 when the cores are there.
- With `LIBVA_DRIVER_NAME=nvidia` set (common on hybrid Nvidia boxes, and set on the test machine), VAAPI can't open the AMD node. libwebrtc then silently falls back to OpenH264, and the log line says so.
- **NVENC wasn't measured.** It's compiled only when CUDA headers are present at build time (`/usr/local/cuda/include/cuda.h`), and they weren't. The release build should have them, or Nvidia users get software AV1 (libaom).

### 2026-11-14: hybrid AMD + Nvidia, after D92

Same machine (RTX 4090 driver 620.70, Mesa 26.1.8), built with `CUDA_HOME` pointing at the cuda_cudart redist only. With the old vendor pick, a real 4K share from KDE Wayland logged `OpenH264 (hardware: false), 1920x1080 at 37 fps`, because `LIBVA_DRIVER_NAME=nvidia` broke VAAPI.

`encoder_check` (no server; it now prints encode time per frame):

| Encoder | 1080p fps | Encode ms/frame | Process CPU |
|---|---|---|---|
| NVIDIA AV1 Encoder | 63 | 2.3 | 14 % |
| NVIDIA H264 Encoder | 62 | 2.6 | 9 % |
| VAAPI H264 (radeonsi) | 45–62 | 4.0 | 11 % |
| OpenH264 | 32–45 | 10–13 | 24 % |

`screen_check` at 1080p60 (sharer and viewer in one process, `IRCHAD_SCREEN_ENCODER` to force one):

| Encoder | Received | Encoder thread | Viewer decode |
|---|---|---|---|
| NVIDIA AV1 (auto) | 59.9 fps | ~1 % | dav1d 45 % |
| VAAPI H264 | 52.7 fps | 3 % | NVDEC 3 % |
| libaom AV1 (software) | 60.7 fps | 97 % + 17 % | dav1d 41 % |

VAAPI still stops short of 60 fps through libwebrtc's synchronous wrapper, even though each encode takes only 4 ms. NVENC holds 60. AV1 shifts the cost to viewers, who decode it in software.

Still owed (phase "Done when"): a real game share from KDE Wayland, GNOME Wayland and X11 with two viewers, and the encoder CPU on a discrete AMD or Intel card.
