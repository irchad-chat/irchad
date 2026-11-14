# Phase 3: Screen share

**Goal:** stream a game at 1080p60 with its audio from Wayland or X11, using hardware encode. This is Discord's worst area on Linux and our headline feature.

## Capture (behind a `ScreenSource` trait)
- [x] Wayland (`chad-media/src/platform/portal.rs`):
  - ScreenCast portal via `ashpd`, plus a restore token so we don't re-prompt every time. The token persists in the desktop settings, and "Choose again" drops it.
  - PipeWire stream, negotiating DMA-BUF first with an SHM fallback. DMA-BUF is linear only, read through mmap; see [perf notes](../notes/screenshare-perf.md).
  - cursor as metadata, not baked into the frame. We draw it onto our copy of the frame and restore what it covered, so cursor-only updates are cheap.
  - Also handles the `VideoCrop` meta (mutter's window shares).
- [x] X11: use the portal when its backend supports X11 (offered in our picker if `AvailableSourceTypes` answers).
  Otherwise use XComposite for windows, XShm for the root window and XDamage to skip unchanged frames, behind our own picker UI.
  These are libwebrtc's own capturers, not `x11rb` (D21). Listing was checked against XWayland. A real X11 session is untested.
- [x] Windows: Windows.Graphics.Capture (monitor and window) through our own picker. libwebrtc's WGC capturer (D21). It builds in CI only and is untested on Windows.

## App audio
- [x] Linux: list PipeWire application nodes (`Stream/Output/Audio`, never our own pid) and capture the selected app's output. It's linked straight to the app node with `node.dont-fallback`/`dont-reconnect`, so it can't fall back to the mic. Never capture our own playback, so there's no voice feedback loop.
- [x] Windows: WASAPI process loopback (Win10 2004+), `platform/loopback.rs`. The app list comes from the default output's audio sessions. It's written against the windows 0.62 sources, isn't compiled locally (no Windows target here), and is untested.

## Encode & publish
- [x] Single-layer track with no simulcast. Content hint "detail" for text and "motion" for games, both selectable. They map to the degradation preference (keep resolution / keep frame rate).
- [x] Codec order (D23):
  - H.264 via VAAPI on AMD/Intel
  - NVENC on Nvidia (AV1)
  - software VP9 fallback
  We log which encoder actually got picked: the `encoder_implementation` from outbound-rtp stats, 5 s in.
- [x] Presets 720p30, 1080p60 and source, with a bitrate cap setting.
- [x] Zero-copy goal: a DMA-BUF → VAAPI path if the SDK allows it. It doesn't, so the copy cost is documented in [`docs/notes/screenshare-perf.md`](../notes/screenshare-perf.md).

## View
- [x] Decoded frames go to a wgpu texture with an I420 → RGB shader (`chad-desktop/src/screen.rs`, egui paint callback, BT.601 limited range).
- [x] Stage view, fullscreen, pop-out window, and an "N watching" count (D22). Subscribe only while the viewer is visible, to save bandwidth: `auto_subscribe` is off, and opening another buffer stops watching unless the share is floated, tiled or popped out.

## Discovered
- [x] chadd's voice token grants `screen_share`, `screen_share_audio` and `canUpdateOwnMetadata` (D22).
- [x] Linking libwebrtc's desktop capturer pulls in its GIO portal code. `chad-media/build.rs` links gio/gobject/glib (D21).
- [x] `crates/chad-media/examples/screen_check.rs`: an end-to-end share → watch check with a hidden test-pattern source (`Target::Pattern`). It passes at 1080p60 (OpenH264) and 720p30 (VAAPI).
- [ ] VAAPI H.264 on the Raphael iGPU delivers 46–51 fps at 1080p60 through libwebrtc's wrapper, against 55–60 for OpenH264 ([perf](../notes/screenshare-perf.md)). Measure on a discrete AMD/Intel card before changing the codec order.
- [x] `LIBVA_DRIVER_NAME=nvidia` (hybrid Nvidia boxes) breaks VAAPI on the AMD/Intel node, and we silently get OpenH264. The encoder is now picked by libwebrtc's probes, NVENC first, and the desktop client unsets that variable at startup (D92).
- [x] NVENC is compiled only when CUDA headers exist at build time. The release build needs them, or Nvidia users get software AV1. The CI image now carries `cuda.h` (`deploy/ci.Dockerfile`, D92). **Rebuild the image** (`ci-image.yml`) before the next release.
- [x] Pre-Ada Nvidia cards have NVENC without AV1, which made AV1 fall back to libaom. Such cards now get H.264 over NVENC (`nvenc_av1_supported()` in the vendored webrtc-sys, D92). Not yet tested on a pre-Ada card.
- [ ] AV1 shares decode in software on viewers (dav1d, about 45 % of a core at 1080p60), because webrtc-sys's NVDEC only does H.264/H.265 and there's no VAAPI decoder. H.264 over NVENC would decode in hardware. Settings → Screen Sharing → "Best compatibility (H.264)" lets a sharer choose that, and with no NVENC the default is now H.264. Whether Automatic should prefer H.264 even with AV1 NVENC is still open: weigh it against AV1's quality per bit ([perf](../notes/screenshare-perf.md)).
- [x] Windows: NVENC builds there through a vendored webrtc-sys (D89), and a DXGI probe sends Nvidia to NVENC. `encoder_check` (no server needed) shows `NVIDIA AV1 Encoder` and, with `CODEC=h264`, `NVIDIA H264 Encoder` (both `hardware: true`) at 1080p on an RTX 4090.
- [x] Upstream bug, Linux too: webrtc-sys's NVENC H.264 took its level from the SDP's `42e01f` (3.1, max 720p), so 1080p failed `nvEncInitializeEncoder` with `NV_ENC_ERR_INVALID_PARAM` and libwebrtc fell back to OpenH264. The vendored copy uses `NV_ENC_LEVEL_AUTOSELECT`.
- [x] Public-stream viewers on LibreWolf got no video from an Nvidia sharer. Its subscriber SDP offers only VP8, VP9 and H.264, so it rejected the AV1 m-section (port 0). We keep AV1, and the watch page tells a viewer whose browser takes no AV1 video to use a Chromium-based one (D90).
- [ ] Check that the watch page's AV1 notice shows in LibreWolf and stays hidden in Chromium.
- [x] Self-preview: your own LIVE badge, or "Preview" in the voice panel, opens your share on the stage as it is sent (after scaling, before encoding). The encoder copies frames only while the preview is open. It doesn't count toward "N watching" and closes when the share ends.
- [x] Choose where a watched share goes from its bar: fill the chat area (the default), float in a small movable, resizable window over the chat, tile as a pane in the pane grid (drag its title to swap it with a chat pane), or pop out. The choice is remembered (`Settings::stage`). Only "fill" stops watching when you open another buffer.
- [ ] Live `shots` scenario for float and tile. It needs a share to watch, so it was checked headless with a fake watch only.
- [x] Sound cues (`engine::Cue`, synthesised like join/leave). Mute and deafen share one cue: two low notes, falling when muting and rising when unmuting. Anyone starting or stopping a share gets a three-note rise or fall. Another viewer joining your share, or the share you watch, gets two high taps. Remote changes come from diffing room snapshots, and the first snapshot after joining is skipped. Someone who leaves mid-share gets the leave cue, not the share-ended cue.
- [ ] Cues replace each other rather than mix. If two land at once, only the later one is heard.
- [ ] Not heard live yet: only tested headless.
- [x] A capture that ends between starting and being stored is remembered (`Shared::ended`), so the share is dropped rather than left without video.
- [ ] Phase 2's "Done when" is still unverified; this phase was started on request anyway.

## Done when
Not verified yet. All three need people and hardware.

- A 1080p60 game share with audio from KDE Wayland, GNOME Wayland and X11 is viewed smoothly by two peers.
- Encoder CPU use on an AMD/Intel machine is recorded in the perf note. Done for the Raphael iGPU (VAAPI and OpenH264). A discrete card is still owed.
- Windows share works, with app audio.
