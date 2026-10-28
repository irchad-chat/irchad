# Phase 2: Voice

**Goal:** click a channel's voice room and talk. On Linux it should beat Discord at push-to-talk on Wayland, device handling and audio quality.

## chadd
- [ ] `POST /api/login` checks credentials via the ergo API and returns a short-lived session token (signed, and holding the casefolded account).
- [ ] `POST /api/voice/token {channel}` returns a LiveKit JWT with identity = account and room = casefolded channel, using a short TTL.
- [ ] Refuse rooms whose channel has `chad/voice=off`, by reading channel metadata via the ergo API. If the API can't read it, the client enforces it and this is noted.

## chad-media
- [ ] Room lifecycle: connect, reconnect, leave, and clean teardown when the IRC connection drops.
- [ ] Capture and playback on the PipeWire native graph:
  - named nodes that follow the default device
  - hotplug
  - per-participant streams mixed locally, with per-user volume
- [ ] APM: AEC (far-end = our mix), NS and AGC. Optional RNNoise toggle.
- [ ] Voice activity detection with an adjustable threshold plus input meter, and push-to-talk:
  - Wayland: GlobalShortcuts portal. Fallback is "PTT works only while focused", stated in the UI.
  - X11: XInput2 raw events.
  - Windows: raw input.
- [ ] Mute and deafen (deafen implies mute). Speaking indicators from LiveKit active speakers.
- [ ] Opus with DTX and RED. Expose a bitrate setting (default 64 kbps).

## chad-core / desktop
- [ ] Set user metadata `chad/voice=<#chan>` on join, and clear it on leave or on any media disconnect.
- [ ] Sidebar shows voice participants under each channel from metadata. Inside the room, show the live LiveKit list.
- [ ] Voice panel showing the connected channel, latency/quality, mute, deafen and disconnect.
- [ ] Audio settings: devices, input test/loopback, VAD threshold, PTT bind and release delay.

## Done when
- A 3-person call on the dev stack is clear, with no echo on laptop speakers.
- PTT works unfocused on KDE Wayland, GNOME Wayland and X11.
- Unplugging a USB headset mid-call falls back to the default device without dropping the call.
- An irssi user sees the voice metadata and nothing else.
