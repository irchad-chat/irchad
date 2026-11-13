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

## D24: Production runs natively under systemd, not compose (2026-10-29)
The first production box (`chat.gentoo.party`, Debian 13, 1 vCPU / 2 GB) runs ergo, LiveKit, chadd and Caddy as plain binaries with hardened systemd units. Files are in `deploy/native/`.
Every service is a single binary, so containers bought us little, and Docker's bridge NAT and per-port `docker-proxy` sit in LiveKit's UDP media path.
Persistent history stays in ergo's SQLite (no MariaDB), to save RAM on a small box.
`deploy/compose.yaml` stays the dev stack. `deploy/native/ergo-prod.py` derives the production `ircd.yaml` from `deploy/ergo.yaml`, so the two can't drift silently.

## D25: Link previews and remote images are fetched by chadd (2026-10-30)
Fetching a posted link from the client would tell whoever posted it the IP of everyone who read it. That's a cheap way to find out where people are.
chadd fetches instead: `POST /api/preview {url}` returns the page's OpenGraph title, description, site and image, and `POST /api/media {url}` returns an image's bytes (sniffed as PNG, JPEG, GIF or WebP, ≤ 8 MB).
Both need a session token and are throttled per account (120 a minute), so chadd isn't an open proxy. Links come from anyone on the network, so chadd only connects to public unicast addresses: the resolver drops private, loopback, link-local, CGNAT and reserved ranges, IP-literal URLs are checked the same way, and so is every redirect hop.
The client fetches its own network's uploads directly (same origin, nothing new learns its IP), and everything else goes through chadd: inline images, preview images and avatars. Guests have no session, so they only see the network's own uploads.
There's still a "load images and previews automatically" setting (on by default). Turned off, each one waits for a click.
This costs the operator bandwidth, and it makes the server's IP the one sites see. Both are acceptable for a small self-hosted network.

## D26: Uploads live on chadd's disk and double as `draft/FILEHOST` (2026-10-30)
`POST /api/upload` takes the raw body and the name from `Content-Disposition`, and answers `201 Created` with `Location` and `{url}`. Files are stored at `<uploads.dir>/<random id>/<sanitized name>` and served by chadd at `/uploads/…`.
The type comes from the bytes, never the client. Only PNG, JPEG, GIF, WebP, MP4 and WebM get a media extension and are served inline. Anything else is `application/octet-stream` with `Content-Disposition: attachment`, and every response has `nosniff` and `Content-Security-Policy: default-src 'none'; sandbox`, so an upload can't run script on the network's origin.
Auth is a Bearer session token or HTTP Basic with the IRC account. That makes it the soju-style `draft/FILEHOST` that plain clients such as goguma and gamja understand. ergo advertises it through `server.additional-isupport` (compose env in dev, `ergo-prod.py` in production), so plain IRC users can upload too.
Discovery's `chadd.uploads.max_bytes` (default 25 MB) says uploads are on. Leave `[uploads]` out of chadd.toml to turn them off.
Avatars are uploads too: the client uploads the image, then sets metadata `avatar` to its URL. Display names are metadata `display-name`. Both are the de-facto keys other clients read.
No quotas and no expiry yet. Operators prune by hand (`docs/hosting.md`).

## D27: Distribution: a Flatpak bundle, a tarball and a per-user NSIS installer (2026-10-30)
Tagged releases (`.gitea/workflows/release.yml`) ship a Flatpak bundle built on `org.freedesktop.Platform` 26.08, a tarball with an `install.sh`, the bare binaries, a Windows NSIS installer, and the chadd image.
The Flatpak build needs the network, for cargo and for webrtc-sys's prebuilt libwebrtc, so it isn't Flathub-ready. Vendoring both is the job for a later Flathub submission.
The NSIS installer installs per user, under `%LOCALAPPDATA%\Programs` with HKCU keys (no admin prompt), and registers `irchad://` before the first run. It's NSIS rather than WiX because it builds anywhere with no toolset beyond `makensis`.
There's no AppImage. The bare binary already writes its own `.desktop` entry, icon and scheme handler on first run, so the tarball's `install.sh` just puts it on `PATH`.
The client checks `git.gentoo.party`'s latest release at startup (a setting, on by default) and shows a notice with a link. It never downloads or installs anything itself.

## D28: Auto-away comes from the desktop's idle time (2026-10-30)
Away is IRC `AWAY`, sent to every network when the user picks Away or has been idle for the configured minutes (10 by default, 0 for never). `Net` re-sends it after reconnects.
Idle time is the whole desktop's, not just our window's. On Wayland it comes from `ext-idle-notify-v1`: a notification at 60 s, so "idle since" is a minute before it fires, and any threshold of a minute or more works without re-arming. On X11 it comes from the XScreenSaver extension, and on Windows from `GetLastInputInfo`.
On a Wayland compositor without the protocol (GNOME before 48), we don't fall back to XWayland, which only sees X clients' input. Input to our own window is the signal there.

## D29: How encrypted DMs go over IRC (2026-10-30)
This refines the Phase 4 design (D8 still holds) where testing against ergo showed it needed to.
- **Device records** are user metadata `chad/e2e/d/<device id>`: `<curve25519> <ed25519> <fallback key> <signature>`. The signature covers the folded nick and the device ID, so the server can't move a record to another person or device. Clients find them with `METADATA <nick> LIST`.
  There's no shared device list: two devices of one account updating a list at once lose one of the writes, and the live test hit exactly that. `chad/e2e/devices` is set whenever a device publishes or unpublishes, and its only job is to tell subscribers (`METADATA * SUB`) to fetch again. A change that lands while a fetch is underway triggers one more fetch.
- **Prekeys:** fallback keys only, rotated weekly; the previous one keeps working through one rotation. A replayed pre-key message can't start a second session while the first is kept (four per peer device).
- **Transport:** one PRIVMSG per device, ours included, with `+chad/e2e=<type>:<base64>`, `+chad/e2e-from` and `+chad/e2e-to`, and the fallback body `🔒 Encrypted message, open IrChad to read`.
  The first copy goes alone. Its echo's msgid goes on the others as `+chad/e2e-id`, and every device files the message under that msgid. That way replies, reactions, `REDACT` and `CHATHISTORY` references keep working with one real server msgid.
  Plaintext is capped at 2000 bytes per message so the ciphertext fits ergo's client-tag budget, and longer text goes out as several messages.
- **Not encrypted:** replies' targets, reactions, typing, redactions, timing and sizes. They're tags and commands the server sees, as it sees who talks to whom.
- **Storage:** the Olm account and sessions are pickled with a key from the OS secret store (entry `<origin>#e2e`). Without a keyring, encrypted DMs stay off and DMs go in the clear with the marker.
  Decrypted text is stored once (an Olm message decrypts once) in the ordinary cache. shortcut: it isn't encrypted at rest (the phase file said encrypted SQLite), and it's pruned at 2000 messages per buffer like any scrollback. SQLCipher, or a column encrypted with the pickle key, is the upgrade if at-rest matters.
- **Trust:** trust on first use per (nick, device). A device whose keys change is pinned again but flagged, and the DM warns until the user verifies or accepts.
  SAS runs over `TAGMSG +chad/e2e-verify`: request, accept with a SHA-256 commitment to the responder's key, both keys, then a MAC of each side's signing key once the people say the emoji match. Only another person's devices can be verified for now, not our own.
- **Logging out** (leaving the network) deletes the record before quitting, and the cache and pickle key with it. A device that's removed while offline stays listed until someone deletes it by hand.

## D30: GIF search goes through chadd, to KLIPY by default (2026-10-31)
The composer's picker has a GIF tab. `POST /api/gifs {query, pos}` (session token, 60 a minute per account) asks a Tenor v2-shaped API and returns a checked page of `{id, title, preview, url, width, height}`. An empty query gets trending (`featured`).
Going through chadd keeps searches and IPs away from the provider, as in D25, and keeps the API key on the server. `[gifs]` in chadd.toml turns it on, and discovery's `chadd.gifs.provider` tells clients, which show "Powered by <provider>".
KLIPY is the default (`api.klipy.com/v2/`) because its API is a drop-in for Tenor's, which Google has been winding down for third parties. `api` can point at any Tenor v2-shaped API.
The answer is hostile like any other (D14): it's capped at 256 KB and 40 items, and each item is kept only with an https URL on a hostname whose path ends in an image extension. The posted rendition is `mediumgif`, else `gif` when it's at most `/api/media`'s 8 MB. Thumbnails and the GIF itself load through `/api/media`, so that throttle went from 120 to 300 a minute to fit a page of 20 thumbnails.
Sending a GIF sends its URL as a plain message. Plain IRC clients see a link, and nothing new goes on the wire.

## D31: chadd uses KLIPY's native API, not its Tenor shim (2026-10-31)
Supersedes D30's choice of API, but not its proxy design. In production the Tenor-compatible `api.klipy.com/v2/{featured,search}` answered 404, so chadd now calls `GET <api>/<app key>/gifs/{search,trending}` (default `api` is `https://api.klipy.com/api/v1/`) with `page`, `per_page`, `content_filter` and `format_filter=gif`.
The answer is `{result, data: {data: [..], has_next}}`. chadd posts `file.md.gif` (else `hd` when it's at most 8 MB), previews `sm`, then `xs`, then `md`, skips items whose `type` isn't `gif` (ads), and hands out the next page number as the opaque `pos`. The answer cap went up to 1 MB because items carry inline blurred previews.
`[gifs]` no longer takes `provider`. KLIPY is the only provider, and the picker's "Search KLIPY" placeholder is required by KLIPY's attribution rules.
Open question: KLIPY's integration requirements ask that requests and media loads come from the end-user client unless KLIPY approves otherwise in writing. Proxying through chadd (D25, D30) needs that approval.
`customer_id` isn't sent, since it's optional and would identify users to KLIPY.

## D32: Linux CI jobs run in a prebuilt image (2026-10-31)
Every Linux build job used to spend minutes on `apt-get` and `llvm.sh 21` before cargo started. `deploy/ci.Dockerfile` bakes that in (PipeWire headers, lavapipe, clang 21 with `CC`/`CXX`/`LIBCLANG_PATH` set, node for JS actions, rustup with stable), and `ci.yaml`, `nightly.yml` and `release.yml` run their Linux jobs in `192.0.2.7:3000/sam/irchad-ci:latest` as the job `container:`.
`ci-image.yml` rebuilds it weekly, by hand, and on changes to the Dockerfile. The jobs still run `dtolnay/rust-toolchain@stable`, so a stale image costs one toolchain download, not a stale compiler.
The image goes through Gitea's registry over the LAN, not `git.gentoo.party`. Cloudflare in front of that host rejects any request over 100 MB, and `docker push` sends each layer in one request, so the clang and gcc layers can't get through.
The LAN registry is plain HTTP, so the Linux runner host's dockerd lists `192.0.2.7:3000` in `insecure-registries`. A new Linux runner needs that too, and has to be on the same LAN.
Only clang and libclang are installed, not everything `llvm.sh` installs (lldb, lld, clangd, Python), since nothing uses the rest.
It's Ubuntu 24.04, the same as the runner's `ubuntu-latest`, so release binaries keep the same glibc floor. Changing the base changes which distros can run the Linux binary.
Windows jobs, and the jobs that only call the Gitea API or Docker, stay on the runner's defaults.

## D33: CI deploys chadd only when it changed; Nightly runs nightly (2026-10-31)
`deploy-chadd` used to rebuild chadd and restart it on prod on every push to `main`, though most pushes don't touch it. It now reads the deployed commit from `/usr/local/bin/chadd.rev` on prod and deploys only when chadd's inputs differ from it (`crates/chadd`, `crates/chad-proto`, `Cargo.toml`, `Cargo.lock`, `.cargo/`, `deploy/chadd.Dockerfile`). It diffs against prod rather than the push's `before`, so a deploy whose run was cancelled by a newer push still happens on the next one.
Nightly ran on every push too, which queued two Windows builds per push on the single Windows runner. It now runs at 04:00 UTC and skips when the `nightly` tag already points at `main`'s HEAD. Running it by hand always rebuilds. Nightly users and chadd's landing page get builds up to a day behind `main`.
CI skips pushes that touch only `docs/**` or Markdown.

## D34: Windows jobs keep a target dir on the runner instead of rust-cache (2026-11-01)
On the host-mode Windows runner, rust-cache never restored: each job builds in a fresh `C:\gitea-runner\work\<random>\hostexecutor`, and the cache version hashes the absolute paths it saves. Every Windows job logged `No cache found`, compiled all ~490 crates (13–15 minutes for CI) and uploaded 1.4 GB that nothing read.
The Windows jobs now set `CARGO_TARGET_DIR` to a fixed folder on the runner, `C:\gitea-runner\cargo-target\ci` for CI and `...\dist` for Nightly and Release, so dependencies stay built between jobs. Each job first wipes its folder once it passes 40 GB, since nothing else prunes it.
Cargo locks a target dir, so jobs sharing one wait for each other rather than corrupt it. The Linux jobs keep rust-cache: they run in containers with a stable `/workspace/...` path, and their cache restores.

## D35: A guest signs up by reconnecting, then rejoins their channels once (2026-11-01)
A guest network can turn into an account network (`NetCommand::Register`). The client QUITs and reconnects through the normal sign-up path: pre-registration `REGISTER *`, email `VERIFY` if needed, then a fresh SASL login with the device ID (D11). It doesn't use an in-session `REGISTER`, because ergo ties that to the current nick and the login would lack the `@device`.
Until that login succeeds the network stays a guest on disk (meta `guest`). A failure at any step, including a restart, leaves it a guest, and a failure while connected reconnects it as one (`Event::SignUpFailed`).
On success the network gets meta `account`, its password is saved, E2EE starts, and meta `rejoin` is set for one sync. The new account is always-on but in no channels yet, so that first sync JOINs the cached channels instead of dropping them. After that the server's list is the truth again.

## D36: ergo's auto-away hides a crashed user's stale `chad/voice` (2026-11-01)
Narrows D16's stale window. When an always-on user's last device disconnects, ergo's auto-away sends `AWAY :User is currently disconnected`.
Clients that see that reason drop the user's voice presence locally. The metadata itself stays until their next launch clears it.
Still stale: viewers who connect after the AWAY was sent (WHO's `G` flag carries no reason), and users who were already away by hand, assuming ergo keeps a manual away message rather than overwriting it (unverified).

## D37: The soundboard is a second audio track, its clips a directory on chadd (2026-11-01)
Admins drop mp3/ogg/wav/flac files into chadd's `[sounds] dir`; there's no upload UI (no admin UI yet).
Signed-in clients list them with `POST /api/sounds` and fetch one with `POST /api/sound`, like the GIF picker. Clients decode at most 10 s of a clip, off the UI thread, and cache it per run.
Playing a clip publishes nothing new over IRC. The player's client sends it on its own LiveKit track, named `soundboard`, with source `Unknown`. LiveKit takes only one track per known source, and the mic already has `Microphone`, so the voice token now grants `unknown` too.
Listeners mix that track as its own source at one "Soundboard volume" for everyone's clips, apart from voices. So only people in the call hear a clip, and the player hears it the same way.
The DSP thread feeds the clip at the mic's 10 ms pace. A clip is dropped while muted or outside a call, and the player hears it through the AEC reference so their mic doesn't send it twice.
The alternative was a data message telling every client to play the clip locally. It was rejected because it needs `canPublishData`, all clients agreeing on the list, and a download per listener.

## D38: The UI view model lives in chad-core, not in each UI (2026-11-01)
Until now chad-desktop projected core `Event`s into its own state (buffers, members, unreads, voice) in `app.rs`.
Android would have to write that logic a second time in Kotlin, and the two copies would drift. So the projection moves into chad-core, and both UIs read the state from there.
Desktop gets refactored onto it first, as the opening item of Phase 6, so the API is proven before UniFFI exposes it.

## D39: Android pushes over UnifiedPush only; without a distributor the user picks (2026-11-01)
A backgrounded app drops its IRC socket shortly after backgrounding. ergo's `draft/webpush` wakes it through UnifiedPush, and it catches up via chathistory when opened. Always-on (D11) keeps the user in their channels meanwhile.
There's no FCM, embedded or otherwise. That keeps the APK free of Google libs and means nobody has to own a Firebase project.
Onboarding suggests installing a distributor (ntfy). Without one, the user chooses between a persistent socket in a foreground service (notifications work, a permanent notification shows, some battery cost) and offline (no background notifications).
Periodic WorkManager sync was rejected, because delayed notifications feel broken for chat.

## D40: Android ships as a signed APK on Gitea releases (2026-11-01)
Releases are laid out so Obtainium can track them: stable asset names and one APK per ABI or a universal one. There's no Play Store and no F-Droid for now.
The application ID is `party.gentoo.irchad`. It doesn't need to resolve, only to be unique and never change after the first release. minSdk is 29 (Android 10).
The APK is built by Gitea Actions in the Linux CI image (D32), extended with the Android SDK, NDK and cargo-ndk.

## D41: Android looks native: Material You dynamic color (2026-11-01)
Android follows the wallpaper palette instead of matching desktop's Catppuccin. It's a Compose app (D3), so it should feel like one.

## D42: Android finds TLS roots through `SSL_CERT_DIR` (2026-11-01)
rustls-native-certs knows no Android trust store, so with no setup every TLS connection fails. chad-ffi points `SSL_CERT_DIR` at the system stores (`/apex/com.android.conscrypt/cacerts`, then `/system/etc/security/cacerts`) and the app's own `files/certs`, before any thread starts.
That covers both the WebSocket (chad-irc) and reqwest without touching their TLS setup. User-installed CAs aren't seen. `files/certs` is how a dev CA gets in for now.
rustls-platform-verifier, which uses Android's own verifier over JNI and needs an extra Gradle component, is the upgrade if user CAs or revocation start to matter.

## D43: Android plays calls behind a foreign `Voice` trait; the core still runs them (2026-11-01)
chad-ffi implements chad-core's `Media` with a UniFFI callback trait, `Voice { join(id, url, token, call), hang_up(id) }`, that Kotlin implements with the LiveKit Android SDK. The core keeps owning voice exactly as on desktop: one call across networks, the chadd token, `chad/voice` metadata and `VoiceState`. Kotlin reports back through the `Call` object (`connected`, `ended`).
Each join gets an id because the core's drop of an old call can reach Kotlin after the next join; a stale `hang_up` is ignored.
Kotlin owns what's in the room (participants, speaking, mute, deafen, which share is watched), the same split as desktop's `chad-media`. It follows desktop's rules: auto-subscribe off, mics and the soundboard track always, a screen share and its audio only while watched, and the `watching` attribute for the sharer's count (D22).
LiveKit's signalling goes through OkHttp, which uses the Java trust store and not D42's `SSL_CERT_DIR`. When `files/certs` holds anything, the app hands LiveKit an OkHttp client that trusts the system CAs plus those, so the dev CA works for both stacks. Otherwise OkHttp keeps its defaults.
A call runs in a `microphone` foreground service, whose notification has mute, deafen and leave. Audio focus and headset or Bluetooth routing are the SDK's AudioSwitch defaults.

## D44: The landing page links the latest release, not nightly (2026-11-01)
chadd's `downloads` now defaults to `releases/download/latest/`, which Gitea resolves to the newest non-prerelease release. The Android APK is only built for tagged releases, and the desktop update check (D27) compares against the latest release, so new chads start on the build that check expects. The footer links the visitor's nightly build on desktop; there is no Android nightly. `downloads` is the releases root and the page appends `latest/` or `nightly/`.

## D45: The project website is a static Astro site in `site/` (2026-11-01)
`site/` is a one-page Astro site with no client JS: what IrChad is, downloads, and how to host a network. It's separate from chadd's per-network landing page, which stays server-rendered, but it uses the same palette and the same `releases/download/{latest,nightly}/<asset>` links (D44). If an asset is renamed, update both `crates/chadd/src/routes.rs` and `site/src/pages/index.astro`.

## D46: Builds are mirrored to garage at downloads.gentoo.party/irchad/ (2026-11-01)
Supersedes the download URLs in D44 and D45. Gitea releases stay the source of truth, and the desktop update check (D27) still reads the Gitea API. After each release or nightly, CI runs `deploy/s3-mirror.sh`, which copies the release's assets into the `downloads.gentoo.party` garage bucket under `irchad/{latest,v<version>,nightly}/<asset>`. It uses plain `curl --aws-sigv4`, so nothing new is installed, and a garage key is set as the `S3_ACCESS_KEY`/`S3_SECRET_KEY` secrets. chadd's `downloads`, the README and `site/` all link there. Objects carry `Cache-Control: max-age=300` because `latest/` and `nightly/` are overwritten in place behind Cloudflare.

## D47: Push notifications show what ergo pushes; encrypted DMs show their fallback (2026-11-01)
Refines D39. Each network gets its own UnifiedPush instance (named by its origin), because the VAPID key comes from that network's ISUPPORT `VAPID`. The core emits `PushAvailable` on every connection, and the app answers with the distributor's endpoint. The core then sends `WEBPUSH REGISTER` once per connection, which renews ergo's 14-day expiry. The UnifiedPush connector (3.x) decrypts the pushes (RFC 8291), and `chad-ffi`'s `read_push` turns the IRC line into a notification. A `MARKREAD` push clears that conversation's notification.
ergo pushes only DMs and channel messages that contain the **account name** (not the nick), and keeps just the `time`, `msgid` and `account` tags. `+chad/e2e` doesn't make it through, so an encrypted DM can't be decrypted from the push. It shows its readable fallback ("🔒 Encrypted message, open IrChad to read"), and opening the app decrypts it as usual. We rejected waking the core on each push to fetch and decrypt history, because it's slow and racy for a notification.
ergo also refuses push endpoints on private or loopback IPs, so the dev stack needs a public push server (ntfy.sh) on the phone.
"Stay connected" is a `specialUse` foreground service that keeps the process and the core alive. The core's fresh DMs and mentions (the desktop's rule, now `chad_core::view::notifiable`) notify only in that mode. Nothing is shown while an activity is on screen.

## D48: On Android, soundboard clips are mixed into the mic track (2026-11-01)
Refines D37 for Android. The LiveKit Android SDK has one capture path (WebRTC's audio device module), so every local audio track it makes carries the mic. A separate `soundboard` track would need a native audio source, which the SDK doesn't expose. So Android mixes a clip into the mic's captured buffers with the SDK's `MixerAudioBufferCallback`.
Listeners hear Android's clips at that person's voice volume, not at their "Soundboard volume". Android still subscribes to everyone else's `soundboard` track as D37 says. Muting drops the clip, as on desktop. The player hears it through a local `USAGE_VOICE_COMMUNICATION` stream, so the echo canceller keeps the clip from going out twice.
Clips are decoded by `MediaCodec` to at most 10 s of mono PCM, then resampled to the mic's rate. If the SDK gains custom audio sources, Android should switch to D37's own track.

## D49: The core pauses connections while the Android app is in the background (2026-11-01)
Implements D39's "drops its IRC socket shortly after backgrounding". `Command::Background { on }` reaches every network. A connected network finishes sending, closes its socket and waits. It doesn't pause while it's in voice, so a call (and its voice metadata) keeps going until it ends. A network that is joining or has failed keeps its state. On `on: false` it reconnects at once (with the backoff reset), and the usual reconnect catch-up (chathistory `AFTER` the last msgid) fills the gap.
The app sends it a minute after its last activity stops, and when the process starts with nothing on screen (a push, a boot). It sends nothing in "Stay connected" mode. Desktop never sends it.

## D50: Composer markdown is sent as IRC formatting codes (2026-11-02)
The composer turns `**bold**`, `*italic*` and `` `code` `` into `\x02`, `\x1d` and `\x11` (`chad_core::format::markdown`), so plain IRC clients see the same styles instead of markdown punctuation. A ```` ``` ```` fenced block becomes one `\x11…\x11` line per line (a blank line is a monospace space), because every line of a multiline message is its own PRIVMSG to a client without `draft/multiline`. The fence's first line is a language and is dropped.
On receive there is no tag to mark a block. A line that is entirely monospace is drawn as a code block, so a message line that is nothing but `` `x` `` shows as a block too. Formatting is parsed per line, as plain clients see it.

## D51: Message edits are a new PRIVMSG tagged `+chad/edit` (2026-11-02)
IRCv3 has no ratified edit spec and ergo implements none, so an edit is a normal PRIVMSG (or multiline batch) to the same target with `+chad/edit=<original msgid>`. Its text is `* <new text>`, the usual IRC correction, so plain clients see a readable follow-up line. In DMs it's encrypted like any other message.
IrChad never shows the edit as a message. It replaces the stored original's text and marks it "(edited)", but only when (D14): the original is in the same buffer and not redacted or a notice, the author matches (same account, or same nick when neither has one), and an encrypted original is only changed by an encrypted edit, so the server can't forge one. Each row keeps the time of the edit that set its text, and an older edit never overwrites a newer one, whatever order history replays them in.
An edit of a message this device hasn't stored is dropped. The desktop composer prefills the edit with the text stripped of formatting.

## D52: Your own stale E2EE devices can be removed from the profile dialog (2026-11-02)
Partly supersedes D29's "stays listed until someone deletes it by hand". Each of your other devices in the profile dialog has a Remove button (`NetCommand::RemoveDevice`). It clears `chad/e2e/d/<id>`, rewrites `chad/e2e/devices` and fetches our devices again. There's no confirmation or revocation. If the device is still running, it sees the change and publishes itself again, so a mistaken removal heals itself and removal only sticks for dead installs.

## D53: One download per desktop platform (2026-11-02)

Releases and nightlies ship `irchad-linux-x86_64.tar.gz` (binary, `install.sh`, desktop files) and `irchad-windows-x86_64-setup.exe` only. The bare binaries were the same bytes uncompressed (Linux 59 MiB vs 25 MiB gzipped). The tarball name carries no version so `latest/` and `nightly/` links keep working. Nightlies from before this look for the old asset name and won't see newer nightlies; their users update by hand once.

## D54: chadd caches previews and proxied images on disk (2026-11-02)

A link posted to a busy channel made every reader's client ask chadd for its preview and image, so chadd fetched it N times.
Now `/api/preview` and `/api/media` go through a cache (`[cache]` in chadd.toml). Concurrent requests for the same URL share one fetch, which runs in its own task so it finishes even if every requester hangs up; errors are shared with whoever was waiting but not kept.
Successes go to `<cache.dir>` for a day. The URLs come from anyone (D14), so the total is capped (`max_bytes`, default 1 GiB, least recently used evicted down to 90%), each item is capped (`max_item`, default 8 MiB; bigger is served, not kept), every file is charged at least 4 KiB so tiny items can't bloat the index, URLs over 8 KiB are refused, and each file stores its own URL so a hash collision is a miss rather than someone else's bytes. Non-images aren't kept. Without `[cache]`, fetches are still shared while in flight.
Flooding unique URLs can only push others out of the cache, within the existing per-account throttle; it can't grow the disk.

## D55: Production moves to irchad.chat (2026-11-03)
The production origin is `https://irchad.chat` (chadd `origin`, ergo server name, FILEHOST, VAPID subscriber). Caddy keeps serving `chat.gentoo.party` with the same routes. Clients re-fetch discovery from their saved origin on every connect, so they pick up the new endpoints without being re-added, and upload links in old history keep resolving. The app IDs (`party.gentoo.IrChad`, the Android package) and `downloads.gentoo.party` don't change.

## D56: Official channels are a `chad/channels` list on the home channel (2026-11-03)
chadd's config names a `home` channel, published in discovery. On first login the client joins it, not everything in `LIST`. Its chanops keep the network's official channels in channel metadata `chad/channels` (ergo only lets chanops set it, and pushes changes live to members). The client trusts that key from the home channel only, so ops of other channels can't move chads around.
Each value is diffed against the last one acted on (store meta `official`), not against what we're in. So a chad who parts an official channel isn't pulled back, and a removed channel is parted only by chads still in it. An unset key changes nothing, so one slip can't empty everyone's sidebar. Plain IRC users never see it. Known gap: a channel added while one device is offline is joined again when that device reconnects, even if the chad already left it on another device.

## D57: The bare origin serves the project website, not the invite page (2026-11-03)
`https://irchad.chat/` (and `www.`) is the Astro site from `site/`, a static `site/dist` that Caddy serves from `/var/www/irchad` as the fallback for every path not routed to chadd, ergo or LiveKit. chadd no longer answers `/`; the invite page lives only at `/invite[/<code>]`. Self-hosted networks without the site get a 404 at the bare origin, so their invite link must include `/invite`. Pasting a bare origin into the client is unaffected, since that goes through discovery.

## D58: On iOS the core checks certificates with the system verifier (2026-11-04)
rustls-native-certs reads macOS's keychain but has nothing for iOS, whose trust store apps can't read, so every TLS connection would fail. On iOS, chad-irc's `tls_config` uses rustls-platform-verifier (SecTrust, with ring as the provider), and reqwest is handed the same config (`use_preconfigured_tls`). That covers user-installed profiles too, which is how a dev CA gets in, and LiveKit's URLSession sees the same trust. Other platforms keep rustls-native-certs. Android stays on D42; the platform verifier is still its upgrade path.

## D59: iOS is a SwiftUI app over chad-ffi, built on Linux with xtool (2026-11-04)
The same shape as Android (D38, D43): `chad-ffi` is compiled to a static library and linked into a SwiftPM app that xtool builds, signs and installs from Linux, with no Mac in the loop. The UI is native SwiftUI, a `NavigationSplitView` that is a sidebar on iPad and a stack on iPhone, and it follows the system's look, as Android does with Material You (D41). Minimum iOS 18.
Calls are played by the LiveKit Swift SDK behind chad-ffi's `Voice`. The soundboard uses the SDK's `SoundPlayer`, which mixes a clip into the mic for the others and plays it locally, so like Android (D48) listeners hear it at this person's voice volume; clips are cut to 10 s with AVAudioFile first.
Credentials and E2EE store keys go in the keychain with `AfterFirstUnlockThisDeviceOnly`, and the data directory is excluded from backups, because a restored copy of the Olm account on a second device would break its sessions.
Backgrounding follows D49 with iOS's limits: connections pause 20 s after the app leaves the screen, inside a background task, and a call keeps the app running through the `audio` background mode.

## D60: iOS has no notifications while suspended, until there's an APNs relay (2026-11-04)
iOS suspends a backgrounded app, so the live connection can't notify (Android's "Stay connected" has no iOS equivalent), and ergo's `draft/webpush` can't reach an iOS app directly: Apple delivers only through APNs, signed with the app publisher's key.
The way forward is a relay that receives ergo's Web Push at `https://<relay>/<device token>` and forwards the still-encrypted payload to APNs. A Notification Service Extension then decrypts it (RFC 8291 with CryptoKit, keys in a shared keychain group) and shows `read_push`'s text. The relay holds the APNs key, so it's run by whoever publishes the App Store build, not by each network's chadd, and it only ever sees ciphertext and device tokens.
Until then, iOS notifies only while the core runs: in a call, or in the moments after leaving the app.

## D61: Android processes the mic itself in the SDK's capture callback (2026-11-05)
Refines D48. Desktop's mic settings (RNNoise, input volume, voice activity with a sensitivity, push to talk, "Hear yourself") have no LiveKit Android SDK option, so Android sets its own `AudioBufferCallback` on the mic track (`Mic` in `Voice.kt`) in place of `MixerAudioBufferCallback`. It edits the captured buffer in place: RNNoise (chad-ffi's `Denoiser`, the same nnnoiseless desktop uses, 48 kHz mono only), gain, the gate, the loopback, and then the soundboard clip mixed in.
Unlike desktop, this runs before WebRTC's echo canceller and noise suppression, because the SDK's callback sits between capture and the APM. Push to talk is a hold-to-talk button in the voice panel. The speaker route is the SDK's AudioSwitch. Per-person, screen-share and soundboard volumes are `RemoteAudioTrack.setVolume`.

## D62: Blocking is a local ignore list in the core, by account or else `*!*@host` (2026-11-05)
"Block" is a traditional IRC ignore, kept per network in the store (`blocks` table) and never sent to the server, so the blocked person isn't told. The mask is `$a:<account>` when we know their account (from the message or member the UI picked, else a WHOX), and `*!*@<host>` for a guest.
`Net` drops everything from a blocked source as it arrives (messages, multiline, history pages, typing, reactions, CTCP, verification), so there's no placeholder, no unread count, no DM buffer and no notification. Messages cached before the block are filtered in `emit_messages`, and on a change the view (D38) drops them from every buffer; a host block can only match cached messages and reactions by the nick they had, since neither stores a host. Unblocking doesn't refetch what was dropped.
Android also skips pushes from blocked people (`Client::push_blocked`), which needs the block list loaded first.

## D63: Activity comes from games' Discord Rich Presence, opt-in, as plain-text metadata (2026-11-06)
Desktop (setting "Share what I'm playing", off by default) listens where Discord's SDKs look for Discord: the highest free `discord-ipc-0..9` socket in `$XDG_RUNTIME_DIR` (else `$TMPDIR`, `/tmp`), or the named pipe of the same name on Windows. SDKs try from 0 up, so Discord, which takes the lowest, wins whichever starts first; one that answers is never taken over. Only our own uid may connect. It answers the handshake with a fake `READY` and keeps `SET_ACTIVITY`; frames are capped at 64 KiB, connections at 16, and the handshake must come within 10 s. Turning the setting off hangs up on every game. Flatpak and Snap Discord paths aren't served.
What it hears goes out as user metadata `chad/activity`, plain text ("Playing Celeste: Chapter 3") so a plain client's `METADATA GET` reads it too, at most every 15 s, and cleared at once. Only a client with sharing on touches the key (`SetActivity { text: None }` otherwise), so a phone or a second desktop on the same account never clears it. Sharing on clears it at once and after each reconnect while idle, which also removes a value a crash left (always-on keeps metadata).
Rich Presence names the game only by Discord application id. chadd, with `discord_games = true`, downloads Discord's undocumented `applications/detectable` list at startup and daily, keeps `{id, name, executables}` (Windows and Linux non-launcher executables as lowercase path suffixes), and serves it at `POST <api>/games` to signed-in users (5 a minute each), cut to fit the clients' 8 MiB cap; discovery's `chadd.games` says it's there. A client that gets no list asks again 5 minutes later. The whole list goes to the client, so neither chadd nor Discord learns who plays what. Without it, the line uses the activity's `details`.
The same setting also spots games that don't speak Rich Presence: every 20 s desktop lists our own processes (`/proc` executables plus a `.exe` `argv[0]`, which is how Wine and Proton games show up; ToolHelp and full image paths on Windows) and matches them against the list's executables by path suffix, ignoring Wine's own `C:\windows\` programs, Windows processes whose full path we can't read, and Discord's `>` entries (they also need the program's arguments to match). If the list ever outgrows the clients' 8 MiB cap, chadd drops the executables before any game. A second setting, "Share what I'm listening to" (Linux), reads the first MPRIS player that's playing, browsers included. The line shared is the best of: Rich Presence, else "Playing <game>" from a running executable, else "Listening to <title> by <artist>". Desktop shows our own line under our name in the sidebar. Windows has no music source yet (it'd need WinRT's GlobalSystemMediaTransportControls).

## D64: Music is shared only from media players the user ticks (2026-11-06)
Supersedes D63's "the first MPRIS player that's playing, browsers included". With "Share what I'm listening to" on, settings list every MPRIS player seen (up to 32), by bus name without its instance suffix (`org.mpris.MediaPlayer2.firefox.instance_1_23` is `firefox`), and only ticked ones are read; the list starts empty. A browser reports any tab's title, so sharing from it by default could leak what someone is watching. Players ticked before stay listed as "not running".

## D65: Windows shares music from GlobalSystemMediaTransportControls (2026-11-06)
Supersedes D63's "Windows has no music source yet". "Share what I'm listening to" now works on Windows too: each 20 s poll asks WinRT's `GlobalSystemMediaTransportControlsSessionManager` (the sessions the taskbar's media flyout shows) for its sessions, lists them by app id (AUMID, e.g. `Spotify.exe` or a Store app's `Family_hash!App`; empty or over 256 UTF-16 units skipped), and reads title and artist from the first ticked one that's playing, with D64's whitelist and per-player timeout unchanged. It polls rather than subscribing to the change events, so a chatty player can't make us set `chad/activity` more often than MPRIS can. The poll runs on tokio's threads, which join no COM apartment, so `Media::new` calls `CoIncrementMTAUsage` to keep the process's implicit MTA alive. Only apps that register with SMTC show up: browsers, Spotify and the Media Player app do, some older Win32 players don't.

## D66: `@`-mentions carry the account in `+chad/mentions`, and show the current name (2026-11-06)
Like Discord, a mention should still name the person after they change their display name or nick. The text keeps the nick, so plain clients see `bob: hi` and highlight on it. Completion (the `@` list and Tab) matches display names too but always inserts the nick. Nicks completed into the draft that are still in the text, belong to a channel member with an account, and are at most 16, go out as the client tag `+chad/mentions=<nick> <account>,<nick> <account>` (on the PRIVMSG, or the multiline BATCH start). Receivers keep a pair only if its nick is a whole word of the text (ASCII case-insensitive), so a forged tag can't ping invisibly. They draw each such word as a pill with the current name of whoever holds that account in the channel now, else of the written nick. A message highlights you if the tag names your account (from `900` RPL_LOGGEDIN), or the text has your nick or display name as a whole word. The pairs are cached with the message, and an edit replaces them. Channels only: everything in a DM highlights anyway, and the tag would sit outside the encryption. Pairs are names, not character offsets, because offsets break with formatting codes, multiline joins and the edit's `* ` prefix. Mobile reads the tag for highlights but doesn't send it or draw pills yet.

## D67: Mobile sends and draws mentions too (2026-11-06)
Supersedes D66's "Mobile reads the tag for highlights but doesn't send it or draw pills yet". `Client::nicks` returns `{nick, shown}`, so the `@` chips show display names and insert the nick. `send` and `edit` take the picked nicks, and `Message.mentions` seeds them when editing. `Span.mention` holds the name to show, with the core already having split each mention into its own span, so Android and iOS only style it. iOS gets the `@` list Android had. SwiftUI's `TextField` doesn't expose the cursor, so on iOS the list works on the draft's last word.

## D68: Opt-in read receipts in DMs, as `+chad/read` while both accounts have `chad/receipts=1` (2026-11-06)
Discord has never had read receipts, and we already know our read position from `draft/read-marker`. They're off by default and only for 1:1 DMs. In a channel a receipt would go to every member, which is noisy and leaks a lot.
The setting is user metadata `chad/receipts=1` (unset is off), so every device of an account agrees and a phone can't disagree with the desktop. Desktop changes it in the profile dialog, and mobile through `Client::set_receipts`. The core keeps the last value it saw in store meta, so offline startup knows it. It subscribes to the key with the others before `CAP END`, and while ours is on it `GET`s it for each DM peer after sync, when a DM opens, and when we turn ours on.
When we mark a DM read, and both our flag and theirs are on, the core sends `TAGMSG <nick> +chad/read=<msgid>` after the `MARKREAD`. The msgid is the newest message we hold there, whoever wrote it. It's a msgid, not a timestamp, so the receiver can check it names a real message in that DM (encrypted DMs have one real msgid per message, D29). A TAGMSG is invisible to plain clients.
Receiving one, with D14 in mind: it's ignored unless our own flag is on. It has to come from the DM peer, to us (not to a channel, nor our own echo or another device's), and name a msgid of at most 128 bytes that we hold in that DM. It only ever moves forward. Its time is the server's `time` tag, clamped to now and to no earlier than the message. Blocked people's are dropped with everything else of theirs. It's stored per buffer (`buffers.seen`, `seen_at`) and shown as `BufferInfo::seen` only while our flag is on. When their flag turns off, their last receipt is deleted.
"Seen" goes under our newest message they've read, unless they've written since, which says as much (`Buf::seen`).
Receipts aren't encrypted, even in encrypted DMs. The server already learns when we read from `MARKREAD`, so they tell it nothing new. A modified client can advertise the flag and never send receipts. That can't be prevented, so it's accepted.
`deploy/ergo.yaml` adds `+chad/read` to `history.tagmsg-storage.whitelist`, so `CHATHISTORY` replays receipts that came while we were offline. A server without that line delivers them live only, and nothing breaks. Android and iOS have the same toggle in their profile screens, and show "Seen <time>" under the message (`Network.receipts`, `Buffer.seen`/`seen_at`, `Client::set_receipts`); phones have no hover, so the time is in the text.

## D69: chadd deletes accounts with their messages and uploads; clients catch up from a tombstone log (2026-11-07)
Store rules (Play's account deletion) want the data gone, not just the account. ergo's `NS UNREGISTER` keeps every message in history, and uploads had no owner (D26). Deletion is server-side, so any client gets it: `POST /api/account/delete {account, password}` re-checks the password (`check_auth`, login's throttles), writes a job file and answers `202`.
One worker runs jobs in turn over a short-lived plaintext loopback connection, opered up as `chadd`. That oper class has only `accreg`, `history` and `nofakelag`. A permanent bot connection was rejected: it would mean reconnect logic and an always-opered session for one rare job.
Steps: `NS UNREGISTER` (two-step code), then `HISTSERV EXPORT`, which needs `history.retention.enable-account-indexing`. ergo writes the JSON lines to `server.output-path`, which chadd shares (`[delete] export_dir`). Each exported msgid is appended to `redacted.log` before it's acted on. Channel messages get `REDACT` (oper, so every member sees it live), and anything REDACT refuses gets `HISTSERV DELETE`. ergo 2.19.1's EXPORT leaves DMs out (it joins only channel history), and only a DM's sender may REDACT it, so `HISTSERV FORGET` then deletes them from history without msgids. Sends are paced at 20 a second, network-wide since there's one worker, so the fan-out can't overflow members' sendqs. Then the export file (plain text of the messages) is deleted, then every upload folder whose `user.chad.owner` xattr names the account. chadd sets that xattr at upload; `deploy/tools/upload-owners.py` backfills older ones from chadd's log. A failed job keeps its file and is retried every 10 minutes and at startup.
`POST /api/redacted {since}` (no auth: msgids are random) serves `redacted.log` by position, up to 1000 a page. No `since` returns only the cursor, so a new device or a dropped cursor costs nothing. IrChad checks after every connect, up to 10 pages, and redacts what it holds (store meta `redacted_cursor`). That reaches clients that were offline. A cursor ahead of chadd's log resyncs, and bad msgids are dropped (D14). Discovery's `chadd.delete` says it's on; without it, clients only send `NS UNREGISTER`.
Accepted gaps: DMs leave server history but not the other person's devices, since there's no msgid to tombstone. History from before indexing was switched on isn't found. Plain IRC clients and bouncers keep what they cached. Deleting from a plain client (`NS UNREGISTER`) does no cleanup. Backups must keep xattrs (`rsync -X`, `tar --xattrs`), or uploads lose their owners. Extends D26.

## D70: Webcam on Linux and Windows reuses the screen share pipeline (2026-11-08)
A camera is a second "share" slot: a platform capture feeds the screen share's encoder at 720p 30 fps, published as LiveKit's `Camera` source with the SDK's defaults (simulcast, VP8). Watching one is watching a share: one video on the stage at a time, subscribed only while it's on screen, and no "N watching" (D22 counts screen shares only).
Linux goes through the Camera portal (`ashpd`), then reads the node on its own PipeWire stream with `target.object` set to the camera picked in settings. The list comes from the registry watch we already run for audio (`Video/Source` with `media.role=Camera`), so the settings never prompt.
We take YUY2, NV12 and RGB only. MJPG isn't decoded, so a camera whose 720p30 mode is compressed-only falls back to a smaller uncompressed mode (we ask for at least 15 fps).
Windows uses Media Foundation's source reader with advanced video processing, which decodes MJPG and hands back NV12. We pick the native mode nearest 720p at 25 fps or more.
A grid of everyone's cameras was rejected for now: it needs per-track frame slots and textures, and it costs bandwidth for each camera on screen. chadd's voice token grants `camera`.

## D71: Whisper is a second mic track that the SFU only lets the targets have (2026-11-09)
Hold the whisper key and only the people on your whisper list hear you, like TeamSpeak (`notes/discord-gaps.md` §1). Desktop sends, and desktop, Android and iOS receive. It's pure LiveKit, so plain IRC clients never notice it.
Each desktop call publishes a third audio track, named `whisper`, with source `Unknown`. chadd's token already grants `unknown` for the soundboard (D37). It carries silence (DTX) until the key is down, and then the mic goes there instead of to the normal mic track.
The SFU enforces it, not the receivers (D14). We call `set_track_subscription_permissions`:
- **No one on the list is in the room:** everyone may have everything. The key does nothing in this state, so the track stays silent.
- **Otherwise:** listed people get `allow_all`, and everyone else gets every sid of ours except the whisper track's.

Every client subscribes to `whisper` tracks, and the SFU only lets the targets' subscriptions through. The permissions are recomputed whenever someone comes or goes, our tracks change, or the list changes.
Targets are granted while they're listed, not only while the key is held, so the first word isn't lost to subscription setup. The DSP thread only whispers while `whisper_ready` says the last permission update was sent and named a target. It clears that flag before every update.
`examples/whisper_check.rs` checked this against LiveKit v1.13 using hostile clients that subscribe to everything:
- A non-target never got the track, whether it joined before or after us.
- A newcomer still got our mic once we updated, because the SFU retries a subscription it denied earlier.
- Someone added mid-call got the whisper.

The list is account names in the desktop settings, at most 50. You edit it from a voice member's right-click menu, and there's one list and one key. Several named lists are left for later.
The key is a second bind on the push-to-talk listener: a second shortcut in the same GlobalShortcuts portal session, a second XInput2 detail, a second polled key on Windows. It works in voice-activity mode too, and it takes the mic away from everyone else while held. It uses the push-to-talk release delay.
**Receiving on desktop:**
- A whisper starts on the first loud frame after a second of quiet. Desktop plays `Cue::Whisper`, a tap leaping an octave (about 100 ms), and puts that much silence into the whisper's ring first, so the cue plays before the voice, not over it.
- The silence is taken back out of the quiet frames that follow, so the delay doesn't build up.
- A whisper source may queue that much more than the usual latency cap.
- A whisper plays at that person's voice volume.
- The member list says "whispering to you". The whisperer's own row says "whispering to N", and listed people are marked "whisper".

Accepted gaps:
- Mobile has no whisper cue, since the native SDKs play remote audio themselves and we can't hold it back. Mobile only marks it as speaking.
- The SFU's active-speaker updates go to everyone, so a non-target on mobile, which shows those, sees the whisperer as speaking without hearing anything. Desktop computes speaking from audio it decodes, so it doesn't show this.
- Someone dropped from the list mid-whisper may hear about one round trip more, until the SFU applies the update.
- Newcomers can't subscribe to our mic or shares until our next permission update lands, which is one round trip after they join. That's fail-closed.

## D72: Classic IRC networks: the URL scheme decides, no chadd (2026-11-09)

IrChad can join any IRC network, not just IrChad servers. A network id of `ircs://host:port` or `irc://host:port` makes it *classic*. `https://` is an IrChad server, as before. There's no flag to store, and the two can't disagree.
Classic networks:
- **No chadd and no discovery.** `conn` builds a local `Discovery` with no features. Its required chadd and LiveKit URLs hold the IRC origin, and `chadd::Client::call` refuses non-HTTP URLs, so nothing ever calls them. Voice is refused in core and hidden in the UI.
- **TCP/TLS lines.** `Connection` gained a line variant with the same 16 KiB cap as WebSocket frames (D14). Oversized lines are dropped up to their `\n`. Non-UTF-8 input decodes as latin-1, because old networks still send it. Plaintext `irc://` is allowed, since some EFnet servers have no TLS, but the dialog warns.
- **Auth.** SASL uses the bare account, because `account@device` is ergo's (D11). Without SASL, the client sends NickServ `IDENTIFY <account> <password>` after `001`. Without a password there's no login. Sign-up stays IrChad-only.
- **No labels.** `send_labeled` falls back when `labeled-response` isn't enabled:
  - Sync runs at once.
  - A send completes on its unlabeled own echo (FIFO, same target), or at once without `echo-message`, and is then stored, since there's no history to refill it.
  - LIST collects `322`s until `323`.
  - WHO becomes `NAMES`.
  - Other pendings (history, devices, block lookups) are cap-gated already and get dropped.
  
  `353`/`366` are parsed everywhere, which is harmless on ergo with `no-implicit-names`.
- **Persistence comes from the cap**, not from `!guest`: without `draft/persistence`, `synced` rejoins the cached channels instead of dropping them. Classic never auto-joins via LIST.
- **Server buffer.** `BufferKind::Server`, keyed `*` (never a valid nick or channel), holds server notices, the MOTD and unhandled numerics. Typing there sends a raw line (`NetCommand::Raw`). It's classic-only, so IrChad's chads don't get a status window.
- **Messages without msgids** are cached on classic networks. The cache is the only history there.
- **"Browse channels…"** (`NetCommand::ListChannels` → `Event::ChannelList`) works on both kinds. It keeps at most 20k `322`s and shows the biggest 5k.

Not done: presets, mobile UI for adding a classic network, a nick separate from the account, CertFP/SASL EXTERNAL, and per-network encodings.

## D73: Custom emoji are plain `:name:` text, their images a directory on chadd (2026-11-09)
A network has its own emoji. In a message one is just `:name:` in the body, and as a reaction it's `+draft/react=:name:`. Nothing else goes over IRC: no TAGMSG, no client tag, no id. Plain IRC clients see `:name:`, which is the readable fallback, and history, edits, replies, encrypted DMs and notifications need nothing new because the text is all there is.
IrChad draws the image wherever a `:name:` is in the network's set (`chad_core::emoji::split` for text outside links and code, `lookup` for reactions), and shows the text until the image has loaded.
The set is the files in chadd's `[emoji] dir`, like the soundboard (D37): `<name>.png`, `.jpg`, `.gif` or `.webp`, the name 2 to 32 of `a-z`, `0-9` and `_`, at most 1 MiB each and 500 in all. There's no upload UI (no admin UI yet).
`POST /api/emoji` lists them and `/emoji/<file>` serves them, both without a session, so guests see them too; they're public chat content. chadd keeps the listing for 10 s because anyone can ask. Discovery's `chadd.emoji` says the network has them.
Clients fetch the list on every connect and when an emoji picker opens. The list is server input (D14): the client checks each name and file again, builds the image URL itself on the network's origin. A name that is also a standard shortcode (`:parrot:`) is the network's emoji there: the composer leaves it as typed instead of turning it into the Unicode one. Images come through `NetCommand::Media`'s same-origin path, whatever the media settings say.
Rejected: a TAGMSG beside each message (two messages to pair up in history, and a flicker or a loss when one is missing) and a tag naming an image URL (anyone could bind any name to any image, and every reader's client would fetch it).
The cost: a new or removed file reaches a client at its next connect or picker, and removing a file turns old messages back into `:name:`. Emoji are per network, never per user, and classic networks (D72) have none.
On iOS an animated emoji inside text is a still frame, because SwiftUI's `Text` takes an image but not a view; on Android an animated one moves in one place at a time.
Not done: completing `:name` while typing (the picker's search finds them), and bigger emoji in a message that is nothing else.

## D74: The game overlay is a see-through window over the game, never injected (2026-11-09)
Desktop can show who's in our call (speaking ring, muted icon) and the last few messages from picked channels over a game, in a corner the user picks. It's display only: clicks go through to the game, and nothing in it takes input. A global combo (default Ctrl+Shift+O) shows and hides it, on the same listener and portal session as push-to-talk and whisper (`platform::ptt`, now `Combo` = modifiers + key, and `Sink` for where presses go).
It's our own always-on-top, click-through, see-through window (an eframe viewport), the way Discord's overlay has worked since 2024. Nothing goes into the game process: no DLL, no Present/`vkQueuePresent` hook, no Vulkan layer. Anti-cheat has nothing to see, and it can't crash a game. The cost: it can't draw over exclusive fullscreen, only windowed and borderless (most games on Windows 10/11 present that way anyway).
It shows while a known game's window has focus: every 500 ms `platform::overlay::Focus` takes the focused window's process (Windows `GetForegroundWindow`; X11 `_NET_ACTIVE_WINDOW` → `_NET_WM_PID`) and matches its path against Discord's game list (D63, fetched whenever the overlay is on too) and executable names the user adds in settings. It sits in a corner of that window. Which game is focused never leaves the machine; sharing the activity is still only D63's setting.
Messages are fresh, notifiable channel messages (`chad_core::view::notifiable`), made one line and cut to 200 characters, 50 kept, each shown for `fade_secs`.
Windows: egui-wgpu only gives windows a see-through backbuffer when the main window asks, so the main window is created transparent (its panels cover it). DX12 only does alpha through DirectComposition, so desktop runs DX12 with `DxgiFromVisual` (`WGPU_BACKEND` and `WGPU_DX12_PRESENTATION_SYSTEM` still override it). That leaves winit's white GDI redirection surface showing through, since egui doesn't expose `WS_EX_NOREDIRECTIONBITMAP`. The window is layered already (click-through), so `platform::overlay::finish_window` color-keys white out and adds `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, no border and square corners. The overlay never draws pure white.
Wayland: `xdg_toplevel` can't stay on top, so it needs `wlr-layer-shell` (KDE, Hyprland, Sway, COSMIC; not GNOME), drawn by its own egui context and egui-wgpu renderer on a layer surface. Not written yet: settings say the overlay doesn't work on Wayland. Gamescope's own overlay atom is a later option.
Rejected: injecting into the game (anti-cheat bans, per-API hooks, crashes), and a viewport on Wayland (it wouldn't stay above the game).

## D75: On Wayland the overlay is a layer surface on the focused output (2026-11-10)

Supersedes D74's Wayland paragraph. `platform::overlay_wayland::Layer` is a `zwlr_layer_surface_v1` on the overlay layer (above fullscreen), on winit's `wl_display` (wgpu makes surfaces only on the display its instance was made with) with its own event queue, drawn by its own `egui::Context` and `egui_wgpu::Renderer` on eframe's wgpu instance, device and queue. It has an empty input region (clicks go through), takes no keyboard, uses an exclusive zone of -1, and presents with `AutoNoVsync` so the UI thread never waits on it. winit's loop reads the socket; the layer dispatches its own queue each time it's drawn. The output is the compositor's pick, which is the focused one and so the game's. It's anchored to the chosen corner of that output, not the game's window, so a windowed game gets the screen's corner.
Focus: KWin and wlroots set XWayland's `_NET_ACTIVE_WINDOW` (D74's X11 path) to None while a Wayland window has focus. Hyprland leaves it on the last X window, and `_NET_WM_STATE_FOCUSED` too (checked on Hyprland 2026-11-10), so on Hyprland `activewindow` is asked over its IPC socket instead. That also spots native-Wayland games there. Elsewhere only XWayland games (Proton, Wine, most SDL2) are spotted.
`support()` needs layer-shell, which GNOME doesn't have, and XWayland (or Hyprland).
Avatars: the main renderer's avatar textures are registered with the layer's renderer (`Layer::import`, `register_native_texture` on the same GPU texture), so nothing is decoded or uploaded twice. Its context gets eframe's system font provider too (CJK, emoji). Not yet: fractional scale (it draws at the scale rounded up), native-Wayland focus on Sway (i3 IPC) and KWin (a script), and gamescope's overlay atom.
Rejected: a winit viewport (an `xdg_toplevel` can't stay on top or be placed), and rendering the overlay through the main egui context (eframe owns its viewports' surfaces).


## D76: Desktop self-update with `self_update`, archives signed with zipsign (2026-11-10)

Supersedes D27's last sentence and D53's asset list. The client will download and install updates itself through the `self_update` crate's Gitea backend, then offer a restart. On Linux that's only when the running binary is writable by the user (the tarball's `~/.local/bin`). A binary the user can't write is taken to be from a package manager, and the setting is disabled there, which leaves D27's notice and link. Windows updates the per-user install in place from a new asset, `irchad-windows-x86_64.zip` (`irchad.exe` and `VERSION`). The installer stays the download for new users. An exe swapped in place leaves the uninstall entry's `DisplayVersion` at the installed version.
Signing comes first, because an updater runs what the server hands it (D14). Release and nightly CI sign `irchad-linux-x86_64.tar.gz` and the Windows zip with zipsign 0.2.2 (ed25519), which is what `self_update`'s `signatures` feature verifies (`packaging/sign.sh`, `packaging/windows/update-zip.ps1`). The signature sits inside the archive (a trailing empty gzip member, or a zip prefix), so plain `tar` and `unzip` still work. The signing context is the asset name, as `self_update` expects, so the updater must save the download under that name. The public key is `packaging/update.pub` (raw 32 bytes, base64 `9xCIKu5o20qk3+J5qjsOD9L8N6eiVOsYb9tLH+RZN/I=`), compiled into the client. The private key is the `UPDATE_SIGNING_KEY` secret (the 64-byte zipsign key file, base64), and its only other copy is offline. A signature alone doesn't stop an older signed release being served as new, so both archives carry a `VERSION` line (`0.1.25` or `nightly <sha>`) inside the signed bytes. The updater must check it against what it expected before installing.
Rotating the key means a release that trusts both keys (`verifying_keys` takes several) before CI switches. A leaked key with no such release forces users to update by hand once.
Rejected: Velopack (AppImage only on Linux, and it would replace NSIS), and a separate signed manifest (`self_update` already verifies archives, and nothing else would read one).

## D77: The updater keeps its own release lookup and uses `self_update` for the rest (2026-11-10)

Refines D76. `self_update`'s Gitea backend compares semver tags, so it can't follow the rolling `nightly` release (compared by commit), and its `update()` installs straight after extracting, with no point to check `VERSION`. `update.rs` keeps the existing lookup (latest release or `nightly`, the `html_url` prefix check), takes the asset's `browser_download_url` only under `https://git.gentoo.party/sam/irchad-ng/releases/download/`, and downloads with the app's reqwest 0.12 (capped at 256 MiB) into `<data dir>/update/<asset name>`. From `self_update` it uses `verify_signature`, against `packaging/update.pub` compiled in, and `Extract`. Then it checks the archive's `VERSION` against the release it expected, and the swap is `self-replace`. `self_update` won't build without an HTTP client, so its `ureq` client is compiled in but unused. That's `rustls` with ring, so no aws-lc.
The check runs at startup and then every 6 hours until something newer turns up. With "Download it, then offer to restart" on, the build is staged and the banner offers "Restart to update". Clicking it swaps the program, closes the window, and `main` lets go of the single-instance name and pipe and shuts the runtime down before starting the new one: `exec` on Linux, spawn on Windows. Otherwise the new process would hand over to the dying one and exit. A failed download or install falls back to D27's link.
`update::blocker` makes the setting greyed out, with the reason on hover, for dev builds (an `IRCHAD_COMMIT` without `IRCHAD_NIGHTLY`), Flatpak, platforms with no update archive, and a program IrChad can't replace. That means its folder won't take a new file, or on Unix a new file there wouldn't belong to the program's owner, or that owner is root.

## D78: One IRC client, many drivers: `Login` and `Net` are sans-IO, and each app brings only I/O (2026-11-11)

Registration moved out of `conn.rs` into `chad_core::login`: `Login` takes server lines and gives back lines to send and a `Step` (logged in, needs the emailed code, signed up, done, lost, fatal), and `login::live` feeds `Net` after that. Keepalive moved out of `ws::Connection` into `chad_irc::keepalive` (`Keepalive`, `Backoff`). Both are plain state machines, so the native driver (`conn.rs`, tokio) and the browser's (`chad-web`, wasm) speak exactly the same IRC, and neither app parses IRC itself. What stays per driver is I/O and policy around it: sockets, timers, discovery fetches, the secret store, chadd, reconnect timing.
The web app is one network, its page's origin in production (dev sets `VITE_IRCHAD_NETWORK`). It keeps no cache yet (`Store::memory`, history from chathistory), saves no password, and has no E2EE, chadd or voice. Its device ID for ergo's per-device login (D11) is in `localStorage`. Like chad-ffi it holds a `View` and hands the page snapshots after an `onChange`.
`chad-core` uses `web-time`'s `Instant` and `SystemTime` (std's on native), because std's panic in the browser. The network's `/.well-known/irchad.json` is served with `Access-Control-Allow-Origin: *`: it's public already, and it lets a web client served from elsewhere read it.

## D79: The web app calls chadd through chad-core's client, and `/api` allows any origin (2026-11-12)

Refines D78. `chad_core::chadd` and discovery's `fetch`/`get`/`body` now build on wasm32: there the HTTP client is reqwest over the browser's `fetch` (its own TLS, redirects and timeouts), and the body cap is checked after the browser has buffered it. So the web app logs in to chadd, uploads, previews links, proxies images (D25), searches GIFs, lists custom emoji and catches up on D69 tombstones with the same code and checks as the other apps. chad-web exposes each call the page makes as a Promise, not an `Event`, because the page asks for one result and awaits it. The page loads the network's own URLs (uploads, emoji) directly, and everything else through `media`.
Caddy answers `/api/*` with `Access-Control-Allow-Origin: *` and the preflight headers chadd's calls need. chadd authenticates with bearer tokens only, never cookies, so a cross-origin page gains nothing it couldn't do from a server. It lets the dev page (Vite's origin) and web clients served from elsewhere use chadd.
The web app is still one network with no cache: history comes from chathistory on every load, by choice. It now does replies, edits, deletes, reactions, typing, scrollback, member lists, @ completion, `:shortcodes:`, slash commands (`/join`, `/msg`, `/part`, `/away`, `/back`), sign-up with the emailed code, and a guest signing up in place.

## D80: Web voice runs in the page with `livekit-client`; chad-core keeps the rules (2026-11-12)

Refines D78. As on desktop, chad-core decides whether we may join (guests, `chad/voice=off`, classic networks) and publishes `chad/voice`. chad-web's `joinVoice` sends `JoinVoice`, takes `voice_request` and fetches the token through chadd, the same as `conn.rs` does. It hands the page `{url, token}`, and the page reports back with `voiceConnected` and `leaveVoice(error)`. The LiveKit room lives in the page (`web/src/stores/voice.ts`), because libwebrtc doesn't build for wasm and the browser has WebRTC built in.
The page follows desktop's room rules (D43): auto-subscribe off, microphones and the soundboard and whisper tracks always, and a screen share and its audio or a camera only while it's watched. One video plays on the stage at a time. The `watching` attribute is set for screen shares only (D22, D70). It caps participants and identities as chad-media does (D14).
The web app joins, talks, mutes, deafens and watches. It doesn't publish a screen or camera yet, and it has no soundboard, whisper, push-to-talk or per-person volume.
If the IRC connection drops, chad-core ends the call and the page leaves the room, as desktop does. Closing the tab clears `chad/voice` on a best-effort basis. Otherwise chad-core clears the stale value on the next connect.

## D81: The web app is served at `<network>/chat/` (2026-11-12)

Production serves `web/dist` from `/var/www/irchad-chat` at `/chat/` on the network's own origin, so the page finds its network from `location.origin` and needs no config. `vite build` sets the base to `/chat/`, and `vite dev` keeps `/`. Caddy strips the prefix and sends any unknown path to `index.html`. The content-hashed `assets/` are cached as immutable, `index.html` is `no-cache`, and responses are compressed (the wasm goes from 3.7 MB to 1.4 MB with zstd).
The site's hero links it as "Join in your browser", next to the app download, and the web app itself keeps pointing people at the app (the first-visit notice and the sidebar link, both to `/invite`).

## D82: Reports are a hidden DM to the home channel's `chad/report` account (2026-11-13)

Play's UGC policy wants report next to block. A report is a PRIVMSG to the nick that the home channel's `chad/report` metadata names, so moderators need no new tooling and plain IRC clients see a readable `[Report] …` line. It is set by ops like `chad/channels` (D56), not parsed out of the MOTD, which every client shows as text.
On the reporter's side it is tagged `+chad/report` and never shown: its echo is answered by the label, its copies and history are dropped, and TARGETS skips that account. The shortcut: a moderator's reply sent while the reporter is offline opens no DM until they write again live.
Only Android and iOS have the button for now.

## D83: Music carries progress and a link in `chad/music`, and follows player signals (2026-11-13)

Refines D63–D65. Next to a "Listening to"/"Watching" `chad/activity`, desktop sets user metadata `chad/music` to `<start> <length> [url]`: the unix second the track would have started to be where it is now, its length in seconds (0 when unknown, then no progress is drawn), and its link. Receivers draw progress from their own clock, so a playing track costs no traffic. A seek moves `start`, but a drift of up to 2 s between polls doesn't count as one. The key goes before the line and is cleared with it, and a game's line never carries one.
Receivers treat the value as hostile. `chad_core::Music::parse` takes exactly two or three fields, caps the length at 24 h and keeps a link only if it is https, has no port or userinfo, and points at a known music or video host (Spotify, YouTube, SoundCloud, Apple Music, Tidal, Deezer, Bandcamp, Twitch). The sender applies the same rule. Links come from MPRIS `xesam:url`; Windows SMTC has none.
Music no longer waits for the 20 s poll: on Linux, MPRIS signals (any signal on `/org/mpris/MediaPlayer2`, and `NameOwnerChanged` for `org.mpris.MediaPlayer2.*`) wake it, and on Windows the session manager's `SessionsChanged` and each session's `MediaPropertiesChanged` and `PlaybackInfoChanged` do. One poll runs per 500 ms burst. Games still poll every 20 s, and the 15 s publish limit stays.
"Watching" replaces "Listening to" when Windows says the session is video, or when the player is a browser and the link isn't a music site. The shortcut: Spotify's web player in Firefox, which has no `xesam:url`, says "Watching". The opt-in "Say which player" setting (off by default) appends "on <player>", from MPRIS `Identity` or Windows `AppInfo`, else the player id tidied. Clients choose the icon by the line's prefix: music, play, or game.

## D84: Android embeds an FCM distributor as the push fallback (2026-11-13)

Refines D39, which said push without FCM. Users shouldn't have to install ntfy, so both flavors add UnifiedPush's `embedded-fcm-distributor`. It needs no Firebase project, no `google-services.json` and no gateway. The app already passes ergo's VAPID key to `register`, so the library hands out `https://fcm.googleapis.com/fcm/send/<token>`. ergo then posts its RFC 8291 push there, signed with its own VAPID key, and Google's servers see only ciphertext. The connector still decrypts.
The connector lists our own package as a distributor only when Play services is present, and it prefers an installed external distributor (ntfy) over the embedded one. So `hasDistributor()` now counts our own package, and onboarding picks push on its own on a phone with Play services. The settings list one Push row per push app ("Through ntfy", "Through Google Play services"), and picking one saves it with `saveDistributor`; the new endpoints replace the old ones at ergo. De-Googled phones see the same choices as before. The cost is the `full` build's "no Google" stance and an F-Droid NonFreeNet flag, should it ever go there.
The shortcut: the library talks to Play services through the old `c2dm` intents rather than the Firebase SDK, so a Play services update could break it. If that happens, the fix is the Firebase SDK plus a Web Push → FCM v1 gateway (UnifiedPush `common-proxies`).

## D85: Android also ships on the Play Store, as a separate `play` flavor (2026-11-13)
Supersedes D40's "There's no Play Store". The `store` flavor dimension builds `full`, the signed APK on Gitea releases that D40 describes (still `party.gentoo.irchad`), and `play`, a bundle for the Play Store. `play` is a separate app, `chat.irchad.IrChad`, signed with its own upload key, and Google holds its app signing key. So it never updates the `full` APK, and both can be installed side by side.
`play` drops "Stay connected" (D47), because Play reviews `specialUse` foreground services strictly (`app/src/play/AndroidManifest.xml`), so its background notifications come only through push (D84). It's built locally with `task play`, not in CI. See `android/README.md`.
