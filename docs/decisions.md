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
chadd's `downloads` now defaults to `releases/download/latest/`, which Gitea resolves to the newest non-prerelease release. The Android APK is only built for tagged releases, and the desktop update check (D27) compares against the latest release, so new chads start on the build that check expects. Nightly stays available on the releases page for testers.
