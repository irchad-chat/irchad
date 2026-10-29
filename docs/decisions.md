# Decisions

Append-only. To change a decision, add a new entry that supersedes the old one by number.

## D1: Fresh Rust workspace, one repo for client and server (2026-10-27)
The previous Wails/Go/Vue app had to shuttle native video into a web view, which was its biggest pain point.
Rust lets the client and chadd share types (discovery), and libwebrtc is available through LiveKit's Rust SDK.

## D2: egui for the desktop UI (2026-10-27)
The user knows egui and wants to hack on it. Video-to-texture is trivial, and it ships as a single binary.
Known costs: we need custom virtualization for variable-height chat rows, plus extra care around cross-message text selection and IME. These get a spike in Phase 0.
`chad-core` is UI-agnostic, so the UI could be replaced without touching anything else.

## D3: Android is native Kotlin/Compose, not egui (2026-10-27)
egui's soft keyboard and IME support on Android is weak, and LiveKit's Android SDK is mature.
Android shares `chad-core` (IRC + state + E2EE) over UniFFI.

## D4: chadd is a small Rust service (2026-10-27)
Its scope is discovery, invite landing page, session login (via ergo HTTP API), LiveKit join tokens, and later uploads.
Anything that can be IRC-native stays IRC-native.

## D5: Invite URL = network origin (2026-10-27)
`https://<origin>/invite[/<code>]` or `irchad://join?origin=...`. Discovery is `/.well-known/irchad.json`.
Codes have no authority in v1.

## D6: Voice presence via user metadata, not a chadd service (2026-10-27)
It stays visible to plain IRC clients, ergo clears it on disconnect, and it adds no server component.
Stale state is possible if the media connection dies while IRC stays up, so the client must clear the key on any room disconnect.

## D7: Linux is state of the art, with X11 still first-class (2026-10-27)
Use PipeWire, portals, D-Bus, SNI and Secret Service everywhere. X11 fallbacks exist only where a portal can't serve X11 (capture on some desktops, PTT) and sit behind the same traits.

## D8: E2EE DMs via Olm (vodozemac), not OTR, not MLS (2026-10-27)
OTR is legacy: it requires both sides online, has no multi-device support, and uses old crypto.
MLS (openmls) is the state of the art for *groups*, but it needs an ordered delivery service for commits. That's a poor fit for IRC and overkill for 1:1.
Olm gives an async double ratchet with per-device sessions, SAS verification is in the same crate, and it's audited.
Known gap: no post-quantum handshake (PQXDH). Revisit MLS if E2EE group channels are ever wanted.

## D9: Require ergo ≥ 2.19 (2026-10-27)
We need metadata (2.17) and the channel-metadata privacy fix (2.19). webpush (2.15) is needed for Android later.

## D10: Self-hosted Gitea with Gitea Actions, not GitHub (2026-10-27)
The repo lives at `git.gentoo.party` under `sam`. CI and release workflows are Gitea Actions in `.gitea/workflows/`, and releases and the chadd container image are published to that Gitea instance.
Tooling is `tea`, not `gh`. Windows CI needs a Windows `act_runner` registered on the instance.

## D11: Always-on is the default (2026-10-27)
ergo runs with `accounts.multiclient.always-on: opt-out`, so every account is always-on unless the user turns it off via NickServ from a normal IRC client. Use `opt-out` rather than `mandatory` so plain-IRC users keep the choice.
Chads expect Discord semantics: you stay in your channels, and messages wait for you. The client relies on chathistory for replay, not `autoreplay-missed`, and identifies each install with a SASL device ID (`account@<device>`).

## D12: Shared wire types live in `chad-proto` (2026-10-27)
The discovery document (and later the chadd API request/response types) lives in a small `chad-proto` crate that both `chad-core` and `chadd` depend on.
It's not in `chad-core`, because chadd would then transitively pull in `chad-media` (libwebrtc) and the rest of the client.

## D13: Dev stack runs as a rootless podman/docker compose project named `irchad-ng` (2026-10-27)
The old `../irchad` compose project is called `irchad`, and reusing that name would recreate its containers.
Rootless containers can't bind :443, so Caddy publishes on `IRCHAD_HTTPS_PORT` (default 8443), and the dev origin is `https://chat.localhost:8443`.

## D14: The server is untrusted input (2026-10-28)
ergo is the only supported IRCd, but that's a protocol choice, not a trust boundary. The person running a network can patch ergo, proxy it, or point an invite at a hostile server.
Treat everything from the server (IRC lines, discovery documents, chadd responses, LiveKit signalling) as attacker-controlled. A bad server must not be able to crash the client, run code, read local files, or exhaust memory or CPU.
Bound every buffer the server can grow (open batches, multiline bodies, history pages, member lists), and fail closed with an error rather than a panic.
This doesn't cover what a server can always do (drop, delay or forge plaintext messages). E2EE DMs (D8) are the answer there.

## D15: Guests can join without an account (2026-10-28)
A chad can join with only a nickname, the way any IRC client can, when the server allows it (ergo's `require-sasl: false`, the default).
Guests get text chat only: no always-on (it needs an account), no chadd session (so no voice or uploads), and no E2EE DMs (keys are published per account).
The client keeps their channel list locally and re-`JOIN`s it on reconnect. Nothing goes in the secret store.

## D16: ergo keeps always-on users' metadata after disconnect (2026-10-28)
Partly supersedes D6. ergo does *not* clear user metadata while an always-on client has no devices connected, so `chad/voice` can outlive the call.
The client clears its own key on leave, on any media disconnect, and before exiting the app. On reconnect, the `metadata *ALL` burst may still carry our old value, and the client clears it.
Another of our devices that is still in a call re-asserts its own value.
Remaining stale window: the app is killed without a clean exit, and that lasts until the next launch.

## D17: chad-core drives media through a `Media` trait (2026-10-28)
`chad-core` asks for a call through `trait Media { fn join(url, token) -> events }` and never depends on `chad-media` (libwebrtc).
The desktop implements it with `chad_media::Voice`, and Android will implement it over the LiveKit Android SDK.
Core owns *when* to be in a call: the token, metadata, and leaving on IRC loss. The media crate owns *how*: the room, audio and DSP.

## D18: chadd session tokens are JWTs with a per-process secret (2026-10-28)
`/api/login` checks the password with ergo's `/v1/check_auth` and returns an HS256 JWT (issuer `chadd-session`, 1 h TTL), built with livekit-api's token code. That means no new JWT crate and no session table.
The signing secret is random per chadd process, so a restart invalidates sessions. The client re-logs in once on 401 using its stored password.
Logins are throttled to 10/min per IP and per account.

## D19: `chad/voice=off` is enforced by the client (2026-10-28)
ergo's HTTP API can't read channel metadata, so chadd can't refuse a voice token for an `off` channel.
The client reads the channel key via `METADATA SYNC` and refuses to join (or leaves) such a room.
A modified client can still join, which is acceptable for a soft "this channel has no voice" switch. If it ever needs to be hard, chadd gets an IRC connection or ergo gets an API.

## D20: UI icons are Phosphor, reactions stay system emoji (2026-10-29)
`chad-desktop` draws UI chrome (buttons, voice controls, reply markers) with Phosphor Icons (regular) from `src/icons.rs`. Emoji reactions keep the system/egui emoji fonts.
The font is vendored in `crates/chad-desktop/assets/` (MIT) instead of depending on `egui-phosphor`, because 0.14 pins egui 0.36 and we're on 0.37. It's installed as a lowest-priority proportional fallback; its glyphs are in the Private Use Area, so it never shadows text.
If `egui-phosphor` catches up to our egui, switch to the crate and delete the vendored copy.

## D21: Wayland capture is ours, X11 and Windows capture are libwebrtc's (2026-10-29)
On Wayland we drive the ScreenCast portal with `ashpd` and read the PipeWire node on our own connection. That gets us a persistent restore token (no picker every time), control over DMA-BUF/SHM negotiation, and the cursor as metadata.
libwebrtc's portal capturer would prompt on every share and needs a GLib main loop.
On X11 and Windows we use libwebrtc's `DesktopCapturer`, which already ships in the prebuilt we link: XShm + XDamage for screens and XComposite for windows on X11, and Windows.Graphics.Capture on Windows.
That replaces the planned `x11rb` and hand-written WGC backends with no new code or dependencies. Both sit behind the same `ScreenSource` trait and our own picker.
Linking it pulls in libwebrtc's GIO-based portal code, so `chad-media`'s build script links `gio-2.0`, `gobject-2.0` and `glib-2.0` on Linux.

## D22: "N watching" comes from a LiveKit participant attribute (2026-10-29)
LiveKit doesn't tell a publisher who subscribed. Each viewer sets its own attribute `watching=<identity>` (empty when not watching), and every client counts them.
chadd's voice token therefore grants `canUpdateOwnMetadata`. That also lets a participant change its LiveKit display name, which we never show (we show the identity, which is the account).
It's LiveKit-only state, so IRC clients never see it.

## D23: The video encoder is chosen by GPU vendor (2026-10-29)
The first `/sys/class/drm/renderD*` node whose PCI vendor we know decides: AMD/Intel → H.264 with the VAAPI backend, Nvidia → AV1 with NVENC, anything else → VP9 in software.
The SDK falls back on its own if that encoder isn't available (for example `LIBVA_DRIVER_NAME` pointing at a decode-only driver, or a build without CUDA headers, which compiles NVENC out). We log the encoder libwebrtc really picked from the outbound-rtp stats.
Off Linux there's no probe yet, so Windows encodes VP9 in software.
