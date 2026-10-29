# Architecture

## System

```
                 https://chat.example.com  (Caddy, :443, auto TLS)
                 ├── /.well-known/irchad.json  → chadd   discovery
                 ├── / and /invite/*           → chadd   landing page
                 ├── /api/*                    → chadd   auth, LiveKit tokens, (later) uploads
                 ├── /irc                      → ergo    IRC over WebSocket
                 └── /livekit                  → livekit signalling (media: UDP direct / TURN)

client ──wss──▶ ergo        text, presence, history, metadata, E2EE DM transport
client ──https─▶ chadd      discovery, session token, LiveKit join token
client ──webrtc─▶ livekit   voice / screen share
chadd  ──http─▶ ergo API    credential check
```

A network is one origin. Everything about it comes from the discovery document, so the client stores nothing but the origin and the user's credentials.

## Invites and discovery

The whole point of chadd is that a single URL gets a user onto someone else's network.

- **The invite URL** is `https://chat.example.com/invite[/<code>]`.
  Opening it (or the bare origin) in a browser gives a chadd-served landing page with a "Click to Join" button (`irchad://join?origin=https://chat.example.com[&code=..]`) and a download button for the visitor's OS, picked from the User-Agent (chadd's `downloads`, defaulting to the nightly release).
- **Pasting** either form into the client's "Join a network" box works the same.
  A bare `chat.example.com` also works, defaulting to https.
- **Scheme handler:** on Linux it's `x-scheme-handler/irchad` in the `.desktop` file, and on Windows it's a registry key.
  A second launch forwards the URL to the running instance (D-Bus on Linux, a named pipe on Windows).
- **Discovery:** `GET <origin>/.well-known/irchad.json`:

```json
{
  "version": 1,
  "name": "Chad Network",
  "icon": "https://chat.example.com/icon.png",
  "irc":     { "websocket": "wss://chat.example.com/irc" },
  "chadd":   { "api": "https://chat.example.com/api" },
  "livekit": { "url": "wss://chat.example.com/livekit" },
  "registration": "open"
}
```

- v1 invite codes carry **no authority**. They may only hint at a landing channel (`?channel=#general`).
  Code-gated registration is parked in Phase 7, so don't build it early.
- The discovery types live in the small shared `chad-proto` crate (D12), so client and server can't drift.

## Accounts and auth

- Users never see NickServ. Registration uses `draft/account-registration` (`REGISTER`), and login uses SASL (SCRAM-SHA-256 preferred, PLAIN fallback).
  Account name = nick, enforced by ergo config.
- ergo **always-on** clients plus multiclient give bouncer behavior for free: channels stay joined, and history is kept while offline.
  Always-on is the default (`accounts.multiclient.always-on: opt-out`, D11). The client logs in as `account@<device>`, treats the server's channel list as authoritative, and fills gaps with `CHATHISTORY AFTER` and `TARGETS`.
- Credentials are stored per network in the OS secret store.
- **Guests** (D15) join with just a nickname: no SASL, no always-on, nothing stored. The client rejoins their channels on reconnect, and they miss what's said while away. No chadd session, so no voice, uploads or E2EE DMs.
- **chadd session (D18):** the client POSTs the account and password to `chadd /api/login`. chadd checks them with ergo's `/v1/check_auth` and returns a 1 h signed session token.
  `POST /api/voice/token {channel}` with `Authorization: Bearer <session>` returns a 10 min LiveKit JWT. Its identity is the account, its room is the ASCII-lowercased channel, and it only allows publishing the microphone.
  Request and response types live in `chad_proto::chadd`.

## Client crates

```
chad-desktop (egui) ──commands──▶ chad-core ──▶ chad-irc  ──▶ ergo
   │    ▲                            │ │  └────▶ chad-e2ee
   │    └───────events / snapshots───┘ └─ Media trait (D17)
   │                                          ▲ implemented by
   └──▶ chad-media ──▶ livekit ───────────────┘ chad-desktop's voice::Glue
```

- `chad-core` owns all state: networks, buffers, members, unread markers, voice state.
  It runs on a tokio runtime and exposes `Command` in and `Event` out channels.
  egui keeps a projected view model, and core calls `ctx.request_repaint()` (via a callback) on change.
  Inside, there's one task per network: `conn` does the I/O, registration and reconnects, and `net::Net` is the sans-IO state machine, unit-tested from IRC lines.
  The only things kept locally are the per-network SQLite cache (whose `meta` holds the origin, account and cached discovery) and the password, behind `secrets::SecretStore`.
- `chad-irc` is pure protocol with no I/O policy. It handles tag escaping, batch reassembly, labeled-response correlation, CAP 302 negotiation, ISUPPORT and casemapping.
  Everything is sans-IO except `chad_irc::ws`, the WebSocket transport (one connection, keepalive, frame cap). Reconnect policy lives in `chad-core`.
  The capabilities we request are listed once, in `chad_irc::cap::WANTED`.
  It's fuzzed and unit-tested against real ergo transcripts.
  It treats the server as hostile input (D14), so every buffer the server can grow is bounded.
- `chad-media` (`Voice`) owns the LiveKit `Room`, the PipeWire/WASAPI audio graph, the APM (AEC/NS/AGC), capture backends and decoded video frames.
  Video goes to the UI as GPU textures, using I420 → RGB in a wgpu shader rather than a CPU conversion.
- Persistence is a SQLite cache (`rusqlite`) per network for scrollback and read state. History of record lives on ergo (`draft/chathistory`).
  Decrypted E2EE plaintext is the exception and lives only locally (see Phase 4).

## IRC conventions

| Concept | IRC mapping |
|---|---|
| "Server" in the sidebar rail | Network (one origin) |
| Text channel | IRC channel. Auto-join everything in `LIST` on first login, and after that always-on keeps it. |
| Voice room | Every channel has one, named `<casefolded channel>`. Ops disable it by setting channel metadata `chad/voice=off`, which the client enforces (D19). |
| Who's in voice | User metadata `chad/voice=<#channel>`, set by the client and cleared on leave, media loss and app exit. ergo keeps it for disconnected always-on users, so the client also clears a stale value on reconnect (D16). Clients `METADATA * SUB chad/voice` before `CAP END` and `SYNC` each channel they join. LiveKit is the authority once you're inside a room. |
| Display name / avatar | Metadata `display-name`, `avatar` (de-facto keys) |
| Typing | `+typing` |
| Replies / reactions | `+draft/reply`, `+draft/react` |
| Edit | Not in IRCv3. Parked. |
| Delete | `draft/message-redaction` (`REDACT`) |
| Unread | `draft/read-marker` (`MARKREAD`), synced across devices |
| Encrypted DM | PRIVMSG with `+chad/e2e` ciphertext tag + human-readable fallback body (Phase 4) |
| Mobile push | `draft/webpush` (ergo ≥ 2.15, experimental) |

Relevant ergo versions: webpush landed in 2.15, metadata in 2.17, and 2.19 fixed a channel-metadata visibility leak. **Require ergo ≥ 2.19.**

## Linux platform matrix

Runtime detection goes through `XDG_SESSION_TYPE` and portal availability (query the portal's `version`/`AvailableSourceTypes`).

| Concern | Wayland | X11 |
|---|---|---|
| Window / GPU | eframe + wgpu (Vulkan) | same |
| Screen capture | ScreenCast portal → PipeWire, DMA-BUF | ScreenCast portal if the backend supports X11 (e.g. KDE). Otherwise `x11rb` XComposite/XShm + XDamage. |
| Screen-share audio | PipeWire: capture an app's node, exclude our own playback | same (PipeWire is display-server independent) |
| Push-to-talk | GlobalShortcuts portal (KDE, GNOME ≥ 48, Hyprland). Fallback: PTT works only while focused, and the UI says so. | XInput2 raw key events via `x11rb` (no grab, sees press and release) |
| Mic / speakers | PipeWire native: node names, default tracking, hotplug | same |
| Notifications | `org.freedesktop.Notifications` (portal inside Flatpak) | same |
| Tray | StatusNotifierItem (`ksni`), no XEmbed | same |
| Secrets | Secret Service via `oo7` | same |
| Single instance / URL forwarding | D-Bus name `party.gentoo.IrChad` serving `org.freedesktop.Application` (`Open`/`Activate`) (`zbus`) | same |
| `irchad://` scheme | user `.desktop` file with `x-scheme-handler/irchad`, written at startup unless a packaged one exists | same |
| Fallback fonts (CJK, emoji) | eframe `system_fonts` (fontique → fontconfig), on demand | same |
| Idle → away | `ext-idle-notify-v1` | XScreenSaver extension |

Windows equivalents: WGC capture, WASAPI (process loopback for app audio), `GetAsyncKeyState` polling for PTT (raw input later), toast notifications, Credential Manager, and a named pipe for single instance.

## Media

- One LiveKit room per voice channel. The participant identity is the ergo account name, and the token comes from chadd.
- Voice is Opus with DTX and RED, with a bitrate setting (default 64 kbps). AEC, NS and AGC come from libwebrtc's APM, fed with our own playback mix as the far-end reference.
  Optional RNNoise (`nnnoiseless`) runs before the APM.
- Audio path: the backend (PipeWire streams `irchad-mic` / `irchad-voice` with role Communication on Linux, cpal elsewhere) moves 10 ms frames through lock-free rings.
  A DSP thread runs RNNoise → APM → VAD/PTT gate → `NativeAudioSource`. Each remote track decodes into its own ring, and the playback callback mixes them with per-user volume.
  Device choice is a PipeWire `target.object`. Without one, WirePlumber follows the default and handles unplug.
- Screen share uses a single layer with no simulcast. Codec preference:
  - H.264 via VAAPI on AMD/Intel.
  - AV1 or H.265 via NVENC on Nvidia.
  - Software VP9/AV1 fallback.
- The SDK picks the hardware encoder automatically, and `LIVEKIT_PREFERRED_HW_ENCODER=nvenc|vaapi` overrides that. Presets: 720p30, 1080p60, source.
