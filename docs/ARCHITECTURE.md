# Architecture

## System

```
                 https://chat.example.com  (Caddy, :443, auto TLS)
                 ├── /.well-known/irchad.json  → chadd   discovery
                 ├── /invite/*                 → chadd   landing page
                 ├── everything else           → static  project website, if installed (D57)
                 ├── /api/*                    → chadd   auth, LiveKit tokens, uploads, link previews
                 ├── /uploads/*                → chadd   uploaded files (D26)
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
  Opening it in a browser gives a chadd-served landing page with a "Click to Join" button (`irchad://join?origin=https://chat.example.com[&code=..]`) and a download button for the visitor's OS, picked from the User-Agent (chadd's `downloads`: the latest release, with a nightly link on desktop).
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
  "chadd":   { "api": "https://chat.example.com/api", "uploads": { "max_bytes": 26214400 } },
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
  A guest can sign up later from the network menu without leaving (D35).
- **chadd session (D18):** the client POSTs the account and password to `chadd /api/login`. chadd checks them with ergo's `/v1/check_auth` and returns a 1 h signed session token.
  `POST /api/voice/token {channel}` with `Authorization: Bearer <session>` returns a 10 min LiveKit JWT. Its identity is the account, its room is the ASCII-lowercased channel, and it allows publishing the microphone, a screen share with its audio, and the soundboard track (source `unknown`, D37).
  `POST /api/upload` (raw body, Bearer or HTTP Basic, also advertised as ISUPPORT `draft/FILEHOST`) stores a file (D26).
  `POST /api/preview` and `POST /api/media` fetch a link's preview or an image on the client's behalf, from public addresses only (D25).
  `POST /api/gifs {query, pos}` searches the GIF provider set up in chadd.toml (KLIPY by default), and discovery's `chadd.gifs` says it's on (D30).
  `POST /api/sounds` lists the soundboard (the files in chadd.toml's `[sounds] dir`), and `POST /api/sound {file}` returns one clip (D37).
  Request and response types live in `chad_proto::chadd`. Calls share one session per network, and the client logs in again on a 401.

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
  `chad_core::view::View<N, B>` projects the events into networks, buffers, messages, members, typing and voice (D38). Each UI hangs its own per-network and per-buffer state on `N` and `B` (desktop: drafts, scroll layout, textures). `apply` returns the message edits so the UI can follow them. Core calls `ctx.request_repaint()` (via a callback) on change. On Android, `chad-ffi` holds a `View<(), ()>` and exposes snapshots of it over UniFFI. Android's calls are played by the LiveKit Android SDK behind chad-ffi's `Voice` callback trait, which stands in for `chad-media` as the core's `Media` (D43). mIRC formatting and link kinds are parsed once, in `chad_core::format`, for both UIs. While backgrounded, Android pauses the connections with `Command::Background` (D49). iOS is built the same way: a SwiftUI app over chad-ffi, with calls played by the LiveKit Swift SDK (D59).
  Inside, there's one task per network: `conn` does the I/O, registration and reconnects, and `net::Net` is the sans-IO state machine, unit-tested from IRC lines.
  The only things kept locally are the per-network SQLite cache (whose `meta` holds the origin, account and cached discovery) and the password, behind `secrets::SecretStore`.
  With E2EE (D29), the cache also holds this device's Olm account and sessions, encrypted with a key kept in the secret store under `<origin>#e2e`, plus the pinned devices of everyone it has talked to.
- `chad-e2ee` is sans-IO crypto over `vodozemac`: the Olm account and sessions, signed device records, and the SAS verification state machine. `chad-core`'s `net::e2e` does the IRC side.
- `chad-irc` is pure protocol with no I/O policy. It handles tag escaping, batch reassembly, labeled-response correlation, CAP 302 negotiation, ISUPPORT and casemapping.
  Everything is sans-IO except `chad_irc::ws`, the WebSocket transport (one connection, keepalive, frame cap). Reconnect policy lives in `chad-core`.
  The capabilities we request are listed once, in `chad_irc::cap::WANTED`.
  It's fuzzed and unit-tested against real ergo transcripts.
  It treats the server as hostile input (D14), so every buffer the server can grow is bounded.
- `chad-media` (`Voice`) owns the LiveKit `Room`, the PipeWire/WASAPI audio graph, the APM (AEC/NS/AGC), capture backends and decoded video frames.
  Video goes to the UI as GPU textures, using I420 → RGB in a wgpu shader rather than a CPU conversion.
- Persistence is a SQLite cache (`rusqlite`) per network for scrollback and read state. History of record lives on ergo (`draft/chathistory`).
  Decrypted E2EE plaintext is the exception and lives only locally, in the same cache (D29). It isn't encrypted at rest beyond whatever the disk is, and it's pruned with the rest of the scrollback.

## IRC conventions

| Concept | IRC mapping |
|---|---|
| "Server" in the sidebar rail | Network (one origin) |
| Text channel | IRC channel. Auto-join everything in `LIST` on first login, and after that always-on keeps it. |
| Voice room | Every channel has one, named `<casefolded channel>`. Ops disable it by setting channel metadata `chad/voice=off`, which the client enforces (D19). |
| Who's in voice | User metadata `chad/voice=<#channel>`, set by the client and cleared on leave, media loss and app exit. ergo keeps it for disconnected always-on users, so the client also clears a stale value on reconnect (D16). Clients `METADATA * SUB chad/voice` before `CAP END` and `SYNC` each channel they join. LiveKit is the authority once you're inside a room. |
| Official channels | chadd's `home` (discovery) is the channel every chad is put in on first login. Its chanops set channel metadata `chad/channels` to a space-separated list. The client subscribes, GETs it after each sync, and diffs it against the last list it acted on: added channels are joined, dropped ones parted. A channel a chad left by hand stays left, and an unset key changes nothing (D56). Deleting a channel for everyone, plain clients too, is `CS PURGE`. |
| Display name / avatar | Metadata `display-name`, `avatar` (de-facto keys), subscribed with `chad/voice` before `CAP END`. For people who join later, the client sends `METADATA <nick> GET`. The avatar is an upload's URL (D26). |
| Activity | User metadata `chad/activity`: plain text such as `Playing Celeste: Chapter 3`, at most 128 characters. Fetched and subscribed like the display name. Desktop sets it when the user opts in: a game's Discord Rich Presence, else a running game matched against chadd's copy of Discord's list, else the track of a media player the user allowed: MPRIS on Linux, GlobalSystemMediaTransportControls on Windows (D63, D64, D65). |
| Away | `AWAY`, from the status menu or desktop idle time (D28), re-sent after reconnects |
| Images and link previews | Plain URLs in the message. The client shows images and video/file cards inline, and previews the first other link through chadd (D25). |
| GIFs | The picker's GIF tab searches through chadd (D30) and sends the GIF's URL as a plain message, so it shows inline like any other image. |
| Typing | `+typing` |
| Replies / reactions | `+draft/reply`, `+draft/react` |
| Mentions | The nick in the text, plus `+chad/mentions=<nick> <account>,...` for nicks picked from completion. IrChad shows them with the account's current name, and highlights on its own account, nick or display name (D66). |
| Edit | Not in IRCv3, so IrChad's own: a new PRIVMSG tagged `+chad/edit=<msgid>`, text `* <new text>` (D51) |
| Delete | `draft/message-redaction` (`REDACT`) |
| Unread | `draft/read-marker` (`MARKREAD`), synced across devices |
| Encrypted DM | One PRIVMSG per recipient device with `+chad/e2e=<type>:<ciphertext>`, `+chad/e2e-from`/`-to=<device>` and a readable fallback body. Copies after the first carry `+chad/e2e-id=<first copy's msgid>` (D29). |
| E2EE devices | User metadata `chad/e2e/d/<device>` (signed keys), found with `METADATA <nick> LIST`. `chad/e2e/devices` is set whenever a device comes or goes, as a subscribed change signal. |
| Verification | `TAGMSG <nick>` with `+chad/e2e-verify` (SAS steps) |
| Mobile push | `draft/webpush` (ergo ≥ 2.15, experimental) |

Relevant ergo versions: webpush landed in 2.15, metadata in 2.17, and 2.19 fixed a channel-metadata visibility leak. **Require ergo ≥ 2.19.**

## Linux platform matrix

Runtime detection goes through `XDG_SESSION_TYPE` and portal availability (query the portal's `version`/`AvailableSourceTypes`).

| Concern | Wayland | X11 |
|---|---|---|
| Window / GPU | eframe + wgpu (Vulkan) | same |
| Screen capture | ScreenCast portal (`ashpd`, restore token) → our own PipeWire stream: linear DMA-BUF first, SHM fallback, cursor as metadata drawn by us | ScreenCast portal if the desktop serves it on X11 (e.g. KDE), offered in our picker. Otherwise libwebrtc's own X11 capturers (XShm + XDamage for screens, XComposite for windows) behind our picker (D21). |
| Screen-share audio | PipeWire: capture an app's node, exclude our own playback | same (PipeWire is display-server independent) |
| Push-to-talk | GlobalShortcuts portal (KDE, GNOME ≥ 48, Hyprland). Fallback: PTT works only while focused, and the UI says so. | XInput2 raw key events via `x11rb` (no grab, sees press and release) |
| Mic / speakers | PipeWire native: node names, default tracking, hotplug | same |
| Notifications | `org.freedesktop.Notifications` (portal inside Flatpak) | same |
| Tray | StatusNotifierItem (`ksni`), no XEmbed | same |
| Secrets | Secret Service via `oo7` | same |
| Single instance / URL forwarding | D-Bus name `party.gentoo.IrChad` serving `org.freedesktop.Application` (`Open`/`Activate`) (`zbus`) | same |
| `irchad://` scheme | user `.desktop` file with `x-scheme-handler/irchad`, written at startup unless a packaged one exists | same |
| Fallback fonts (CJK, emoji) | eframe `system_fonts` (fontique → fontconfig), on demand | same |
| Idle → away | `ext-idle-notify-v1` (none: our own window's input) | XScreenSaver extension |
| File drag-and-drop | Not in winit 0.30. Paste (`Ctrl+V` of files or images) and the FileChooser portal (the paperclip) instead. | XDND via winit |

Windows equivalents: WGC capture (libwebrtc's, D21), WASAPI (process loopback for app audio), `GetAsyncKeyState` polling for PTT (raw input later), toast notifications, Credential Manager, and a named pipe for single instance.

## Media

- One LiveKit room per voice channel. The participant identity is the ergo account name, and the token comes from chadd.
- Voice is Opus with DTX and RED, with a bitrate setting (default 64 kbps). AEC, NS and AGC come from libwebrtc's APM, fed with our own playback mix as the far-end reference.
  Optional RNNoise (`nnnoiseless`) runs before the APM.
- Audio path: the backend (PipeWire streams `irchad-mic` / `irchad-voice` with role Communication on Linux, cpal elsewhere) moves 10 ms frames through lock-free rings.
  A DSP thread runs RNNoise → APM → VAD/PTT gate → `NativeAudioSource`. Each remote track decodes into its own ring, and the playback callback mixes them with per-user volume.
  Device choice is a PipeWire `target.object`. Without one, WirePlumber follows the default and handles unplug.
- Screen share uses a single layer with no simulcast. Codec preference, by the PCI vendor of the first render node (D23):
  - H.264 via VAAPI on AMD/Intel.
  - AV1 via NVENC on Nvidia.
  - Software VP9 otherwise (and on Windows for now).
  The SDK falls back to software if the asked-for encoder isn't there, and 5 s in we log the encoder libwebrtc actually picked. Presets: 720p30, 1080p60, source, plus a bitrate cap.
  The content hint maps to libwebrtc's degradation preference: "Text" keeps resolution, "Motion" keeps the frame rate.
- Capture backends implement `ScreenSource` and hand BGRA/RGBA frames to one sink, which converts to I420 (libyuv), scales to the preset and feeds a `NativeVideoSource`.
  App audio (PipeWire app stream node, or WASAPI process loopback) goes out as a second, stereo `ScreenshareAudio` track.
- Viewers subscribe to tracks themselves (`auto_subscribe` off): microphones always, a screen share and its audio only while it's on screen.
  A viewer sets its LiveKit participant attribute `watching=<identity>`, and the sharer counts those for "N watching" (D22).
  Decoded frames go to the UI as I420 planes, and a wgpu paint callback converts them to RGB in a shader.
