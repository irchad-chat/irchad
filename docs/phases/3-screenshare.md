# Phase 3: Screen share

**Goal:** stream a game at 1080p60 with its audio from Wayland or X11, using hardware encode. This is Discord's worst area on Linux and our headline feature.

## Capture (behind a `ScreenSource` trait)
- [ ] Wayland:
  - ScreenCast portal via `ashpd`, plus a restore token so we don't re-prompt every time
  - PipeWire stream, negotiating DMA-BUF first with an SHM fallback
  - cursor as metadata, not baked into the frame
- [ ] X11: use the portal when its backend supports X11.
  Otherwise use `x11rb` capture (XComposite for windows, XShm for the root window, XDamage to skip unchanged frames) behind our own picker UI.
- [ ] Windows: Windows.Graphics.Capture (monitor and window) through our own picker.

## App audio
- [ ] Linux: list PipeWire application nodes and capture the selected app's output. Never capture our own playback, so there's no voice feedback loop.
- [ ] Windows: WASAPI process loopback (Win10 2004+).

## Encode & publish
- [ ] Single-layer track with no simulcast. Content hint "detail" for text and "motion" for games, both selectable.
- [ ] Codec order:
  - H.264 via VAAPI on AMD/Intel
  - NVENC (AV1 or H.265) on Nvidia
  - software VP9 fallback
  Log which encoder actually got picked.
- [ ] Presets 720p30, 1080p60 and source, with a bitrate cap setting.
- [ ] Zero-copy goal: a DMA-BUF → VAAPI path if the SDK allows it. Otherwise document the copy cost in `docs/notes/screenshare-perf.md`.

## View
- [ ] Decoded frames go to a wgpu texture with an I420 → RGB shader.
- [ ] Stage view, fullscreen, pop-out window, and an "N watching" count. Subscribe only while the viewer is visible, to save bandwidth.

## Done when
- A 1080p60 game share with audio from KDE Wayland, GNOME Wayland and X11 is viewed smoothly by two peers.
- Encoder CPU use on an AMD/Intel machine is recorded in the perf note.
- Windows share works, with app audio.
