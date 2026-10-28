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
