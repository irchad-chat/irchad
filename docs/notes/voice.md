# Phase 2 voice: findings and how to check it

## End-to-end check without a second person

`crates/chad-media/examples/voice_check.rs` joins a room through `chad_media::Voice`, using your real mic, speakers and the APM.
A raw LiveKit participant publishes a quiet 440 Hz tone and counts how much of your mic it receives.

```sh
SSL_CERT_FILE=$PWD/ca-bundle.crt LIVEKIT_URL=wss://chat.localhost:8443/livekit \
  cargo run -p chad-media --example voice_check
```

Result on Linux (PipeWire 1.6, Hyprland): participants ok, 6.0 s of mic heard over 6 s, RTT reported. You should hear the tone in your output.
If `chat.localhost` doesn't resolve through glibc on your box, add another name for 127.0.0.1 to `/etc/hosts`.

The core side has a live test, `chad-core/tests/live.rs::voice_token_and_metadata` (ignored by default).
It covers login, the voice token, the `chad/voice` metadata seen by a second client, and its clearing.

## ergo metadata behaviour (2.19.1)

- Updates arrive as numerics `761 <me> <target> <key> <vis> :<value>` and `766 … :key not set`, not as `METADATA` commands.
- `METADATA * SUB chad/voice` before `CAP END` makes the reattach burst include everyone's value. Without it, nothing is pushed.
- Metadata for a user who joins after us is not pushed. `METADATA #chan SYNC` returns the channel and its members, and we send it on our own join.
- An always-on user's metadata survives their last disconnect (D16).
- Only channel ops can set channel keys, so ops set `chad/voice=off` with `/metadata #chan set chad/voice off` from any client.
- `client-throttle` defaults to 10 sets per 2 min.

## Media details

- `NativeAudioSource` with queue size 0 makes `capture_frame` complete synchronously, so the DSP thread calls it with `now_or_never()` and never blocks on tokio.
- RTT comes from the nominated ICE candidate pair's `current_round_trip_time`. Filtering on `> 0` hides loopback (it's 0.000 s there).
- `AudioProcessingModule` isn't `Send`, so it's created and used only on the DSP thread.
- `webrtc::InitializePipeWire()` must run before pipewire-rs touches anything (see `livekit-sdk.md`).

## Manual checks still owed (phase Done when)

- 3-person call with laptop speakers: listen for echo. If there is echo, tune `STREAM_DELAY_MS` in `engine.rs` first.
- PTT unfocused: KDE Wayland and GNOME ≥ 48 (portal dialog on first bind), X11 (XI2 raw events, no grab).
- Pull a USB headset mid-call with "System default" selected. WirePlumber should move `irchad-mic`/`irchad-voice` and the call stays up.
- irssi in the channel: it sees `METADATA` numerics only if it subscribed, and no other traffic.
