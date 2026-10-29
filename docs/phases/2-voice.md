# Phase 2: Voice

**Goal:** click a channel's voice room and talk. On Linux it should beat Discord at push-to-talk on Wayland, device handling and audio quality.

## chadd
- [x] `POST /api/login` checks credentials via the ergo API and returns a short-lived session token (signed, and holding the casefolded account).
- [x] `POST /api/voice/token {channel}` returns a LiveKit JWT with identity = account and room = casefolded channel, using a short TTL.
- [x] Refuse rooms whose channel has `chad/voice=off`, by reading channel metadata via the ergo API. If the API can't read it, the client enforces it and this is noted.
  ergo's API has no metadata read, so the client enforces it (D19). chadd doesn't check channel membership either.

## chad-media
- [x] Room lifecycle: connect, reconnect, leave, and clean teardown when the IRC connection drops.
- [x] Capture and playback on the PipeWire native graph:
  - named nodes that follow the default device
  - hotplug
  - per-participant streams mixed locally, with per-user volume
- [x] APM: AEC (far-end = our mix), NS and AGC. Optional RNNoise toggle.
- [x] Voice activity detection with an adjustable threshold plus input meter, and push-to-talk:
  - Wayland: GlobalShortcuts portal. Fallback is "PTT works only while focused", stated in the UI.
  - X11: XInput2 raw events.
  - Windows: raw input. Done as `GetAsyncKeyState` polling every 5 ms (a `shortcut:`), untested on Windows.
- [x] Mute and deafen (deafen implies mute). Speaking indicators from LiveKit active speakers.
- [x] Opus with DTX and RED. Expose a bitrate setting (default 64 kbps).

## chad-core / desktop
- [x] Set user metadata `chad/voice=<#chan>` on join, and clear it on leave or on any media disconnect.
- [x] Sidebar shows voice participants under each channel from metadata. Inside the room, show the live LiveKit list.
- [x] Voice panel showing the connected channel, latency/quality, mute, deafen and disconnect.
- [x] Audio settings: devices, input test/loopback, VAD threshold, PTT bind and release delay.

## Discovered
- [x] ergo keeps an always-on user's metadata after their last device disconnects (contradicts D6). The client clears a stale `chad/voice` from the reattach burst and on app exit (D16).
- [x] `METADATA * SUB chad/voice` before `CAP END`, plus `METADATA <chan> SYNC` on our own join, because ergo doesn't push metadata for users on JOIN.
- [x] chad-core drives media through a `Media` trait that the desktop implements (D17).
- [x] chadd session tokens are HS256 JWTs signed with a random per-process secret, so a chadd restart logs everyone out of chadd (not IRC) (D18).
- [x] `crates/chad-media/examples/voice_check.rs`: an end-to-end check that runs the real engine against the dev LiveKit ([notes](../notes/voice.md)).
- [ ] Metadata for users who join after us isn't fetched until our next `SYNC` or reconnect, so the sidebar can miss them. Fix: `SYNC` on their JOIN (rate-limited), or wait for ergo to push.
- [ ] AEC uses a fixed 40 ms stream delay. Measure the real PipeWire latency if echo shows up in the 3-person test.
- [ ] ergo's metadata throttle (10 sets per 2 min) is enough for normal join/leave, but rapid hopping hits it. Raise it in `deploy/ergo.yaml` if testers do.
- [ ] Both dev networks share one LiveKit, so `#dev` on each lands in the same room. Prefix the room with the network if that matters beyond dev.

## Done when
Not verified yet. All four need a human with hardware.

- A 3-person call on the dev stack is clear, with no echo on laptop speakers.
- PTT works unfocused on KDE Wayland, GNOME Wayland and X11.
- Unplugging a USB headset mid-call falls back to the default device without dropping the call.
- An irssi user sees the voice metadata and nothing else.
