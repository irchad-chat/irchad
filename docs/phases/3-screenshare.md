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
- [x] Stage view, fullscreen, pop-out window, and an "N watching" count (D22). Subscribe only while the viewer is visible, to save bandwidth: `auto_subscribe` is off, and opening another buffer stops watching unless popped out.

## Discovered
- [x] chadd's voice token grants `screen_share`, `screen_share_audio` and `canUpdateOwnMetadata` (D22).
- [x] Linking libwebrtc's desktop capturer pulls in its GIO portal code. `chad-media/build.rs` links gio/gobject/glib (D21).
- [x] `crates/chad-media/examples/screen_check.rs`: an end-to-end share → watch check with a hidden test-pattern source (`Target::Pattern`). It passes at 1080p60 (OpenH264) and 720p30 (VAAPI).
- [ ] VAAPI H.264 on the Raphael iGPU delivers 46–51 fps at 1080p60 through libwebrtc's wrapper, against 55–60 for OpenH264 ([perf](../notes/screenshare-perf.md)). Measure on a discrete AMD/Intel card before changing the codec order.
- [ ] `LIBVA_DRIVER_NAME=nvidia` (hybrid Nvidia boxes) breaks VAAPI on the AMD/Intel node, and we silently get OpenH264. Consider choosing the Nvidia path when that variable is set.
- [ ] NVENC is compiled only when CUDA headers exist at build time. The release build needs them, or Nvidia users get software AV1.
- [ ] Windows: no GPU probe yet (VP9 in software). Probe the DXGI adapter vendor once NVENC builds there.
- [ ] No self-preview of your own share.
- [x] A capture that ends between starting and being stored is remembered (`Shared::ended`), so the share is dropped rather than left without video.
- [ ] Phase 2's "Done when" is still unverified; this phase was started on request anyway.

## Done when
Not verified yet. All three need people and hardware.

- A 1080p60 game share with audio from KDE Wayland, GNOME Wayland and X11 is viewed smoothly by two peers.
- Encoder CPU use on an AMD/Intel machine is recorded in the perf note. Done for the Raphael iGPU (VAAPI and OpenH264). A discrete card is still owed.
- Windows share works, with app audio.
