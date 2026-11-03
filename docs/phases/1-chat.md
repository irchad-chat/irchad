# Phase 1: Connect & chat

**Goal:** a Discord user pastes an invite URL, creates an account, and chats, without learning any IRC concept. The client supports multiple networks from day one.

## chad-irc

- [x] Message parse/serialize with tags (escaping), source and params. Write property tests and a fuzz target.
      Fuzz target is `crates/chad-irc/fuzz` (standalone). On stable: `RUSTC_BOOTSTRAP=1 cargo fuzz run message -s none` (no unsafe, so no ASan needed). 32.7M runs / 3 min clean on 2026-10-28. Deliberately not in CI: run it as a pre-release check (see Phase 5).
- [x] CAP 302 negotiation, including `cap-notify` for runtime changes. Request every capability in `ARCHITECTURE.md` that ergo advertises.
      `chad_irc::cap::Negotiator` (sans-IO); the requested set is `cap::WANTED`. Requests `draft/metadata-2` only: `-3` is an unmerged spec PR (ircv3/ircv3-specifications#613) that ergo already advertises. Revisit when it merges.
- [x] chad-core: surface cap `NEW`/`DEL` to the UI layer (e.g. `away-notify` dropped → fall back to polling), once core exists.
      `Event::CapsChanged { added, removed }` (only caps in `WANTED`). No polling fallback is built: ergo never drops caps at runtime. Add it if one ever does.
- [x] SASL SCRAM-SHA-256 with PLAIN fallback, and `REGISTER` (draft/account-registration) including the verify step.
      `chad_irc::sasl::Sasl` (verifies the server signature; falls back to PLAIN only if SCRAM is refused before the proof) and `chad_irc::register`.
      Live check: `cargo test -p chad-irc --test ergo_live -- --ignored` (each run leaves an `lt<timestamp>` account behind).
      ergo only registers the current nick (`ACCOUNT_NAME_MUST_BE_NICK`), so send `NICK <username>` then `REGISTER * …`.
      `VERIFY` is unit-tested only: the dev stack has email verification off.
- [x] chad-core: after `REGISTER` succeeds the connection is logged in as `bob`, not `bob@<device>`. Reconnect with SASL so the device ID is in place (D11).
      `conn::session` QUITs on `REGISTER`/`VERIFY` success and reconnects at once with SASL `account@<device>`.
- [x] Batch reassembly (nested).
      `chad_irc::batch::Assembler` (sans-IO). It returns plain lines or whole top-level batches, and a nested batch sits in its parent where it closed.
      Lines tagged with an unknown batch pass through as plain messages. Call `clear()` on disconnect.
      It caps open batches (64) and held bytes (32 MiB) per D14, and on `Overflow` the caller should disconnect.
- [x] Labeled-response correlation (request → future), ISUPPORT, and casemapping.
      `chad_irc::label::Labels<T>` holds a caller token per label (a `oneshot::Sender` makes it a future); feed it `Assembler` output.
      `chad_irc::isupport::{Isupport, Casemapping}`. `rfc8265` is Unicode-lowercase only (no PRECIS), which is fine while ergo runs `ascii`.
- [x] WebSocket transport (`text.ircv3.net`), keepalive, and reconnect with backoff.
      Cap the incoming frame/line length (D14). `Message::parse` has no limit of its own.
      `chad_irc::ws::{Connection, Backoff}`: frames capped at 16 KiB, answers server PING, PINGs after 90 s idle and times out 30 s later. The reconnect loop itself belongs to chad-core.
      rustls with OS roots; for the dev stack set `SSL_CERT_FILE` to a bundle with Caddy's CA (`notes/livekit-sdk.md`). Live: `ergo_live` `websocket_isupport_and_labels`.
- [x] Review `cap`, `sasl` and `register` against D14 (hostile server): unbounded `CAP LS` accumulation, oversized SCRAM iteration counts, and panics.
      Fixed: `CAP` offers capped at 256, SASL incoming payload capped at 8 KiB. Iterations were already capped (2^20); no server-reachable panics found. `register` holds no state.

## chad-core

Bumped workspace `rust-version` to 1.92, because oo7 0.6 and rusqlite 0.41 require it.

- [x] Network model: origin, discovery doc, credentials (secret store), and connection state.
      API: `chad_core::start(Config) -> (Core, events)`, `Command`/`NetCommand` in, `Event` out (`lib.rs`). Networks are keyed by origin.
      The cache is `<data_dir>/networks/<host_port>.sqlite`, and its `meta` table holds the origin, account and last discovery doc (refetched on every connect, the cache used if that fails).
      The password goes in `secrets::SecretStore`: `SystemSecrets` is oo7 on Linux, and Windows is a stub until the Windows item below. `MemorySecrets` is for tests.
      Discovery is bounded (64 KiB body, 15 s, https only, `wss` websocket required).
      Device ID: 16 hex chars in `<data_dir>/device-id`, shared by all networks.
- [x] Join flow: parse an invite (https, `irchad://`, bare host) → discovery → register or login → first-login auto-join from `LIST`.
      `invite::parse`. The network is kept only after its first login succeeds; any earlier failure gives `Event::JoinFailed` and deletes its cache.
      Auto-join runs once per install (`meta.autojoined`), and only when the server put us in no channels. `?channel=` is joined after sync.
      Email verification surfaces as `ConnState::NeedsVerification` and is answered with `NetCommand::Verify`. Like `chad-irc`'s, it's unit-level only (dev stack has verification off).
      Live: `cargo test -p chad-core --test live -- --ignored` (register → LIST auto-join → send → restart from cache → alice sees it via chathistory). It needs `IRCHAD_HOST=localhost` on the stack and `SSL_CERT_FILE`.
- [x] Buffers: channels and DMs. History backfill via `CHATHISTORY` (latest, before, around), with dedupe by `msgid`.
      Pages of 100, labeled. `NetCommand::LoadOlder`/`LoadAround`, and `Event::HistoryStart` when `BEFORE` runs dry. Dedupe is the `msgid UNIQUE` column.
      An incoming DM creates its buffer. Messages for channels we're not in are dropped.
- [x] Members: names, away status, account. Presence via `away-notify`, `account-notify`, `extended-join` and MONITOR for DM partners.
      Lazy `NetCommand::Members` → `WHO #c %tcnfa` (WHOX gives nick, away, account and prefixes in one go). `Event::Members` is the full list after each change. DM presence is `BufferInfo::{online, away}`.
      Known gap: `MODE +o` etc. doesn't update prefixes until the next `Members`.
- [x] Read markers (`MARKREAD`) feed unread and mention counts and sync across devices.
      Counts are computed from the cache (`Store::counts`): others' messages after the marker. A mention is your nick as a whole word, or any DM. Markers only move forward.
- [x] Send path: `echo-message`, pending/failed state, `draft/multiline` for pasted blocks, `+typing`, `+draft/reply`, `+draft/react`, and `REDACT` for your own messages.
      Sends are labeled, and the echo replaces the pending message (matched by `local_id`). An error reply, or the connection dropping, marks it failed.
      Lines over 350 bytes are split with `draft/multiline-concat`, and the server's `max-bytes`/`max-lines` are checked before sending. `+typing=active` goes out at most every 3 s per buffer. CTCP other than ACTION is dropped, never answered.
- [x] SQLite cache per network, so the last scrollback is shown instantly at startup.
      `store.rs` (rusqlite, bundled, WAL). It keeps 2000 messages per buffer, pruned on open, and up to 200 reactions per message. Startup shows the newest 200 per buffer before connecting.
      Writes happen inline on the network task. That's fine at chat rates; revisit if history pages ever stall input.
- [x] Sign in again after `ConnState::Failed` (keyring locked or gone, password changed): `NetCommand::SignIn { password }` retries and saves the password once a login works.
      Found while testing the desktop: a locked keyring left the network stuck with no way out but leaving it. The keyring is now unlocked first (`oo7` `unlock()`, the desktop's own prompt).
- [x] Guest join, no account (D15): `Auth::Guest { nick }`, and "Guest" in the join dialog.
      No SASL, nothing in the secret store, no `PERSISTENCE` (so cached channels are re-`JOIN`ed). A taken nick tries `nick1`..`nick9`. After the first join a guest never sits in `Failed`; it keeps retrying. Live: `live` `guest_join_collision_restart`.
- [x] Always-on (on by default server-side, D11). The client acts like a bouncer client, not a fresh IRC client:
      After the end of MOTD the client sends `PERSISTENCE GET`, then a labeled `PING`. Its reply marks the end of ergo's reattach burst, so the channel list is complete.
      With persistence on, cached channels the server didn't rejoin are dropped. With it off, they're re-`JOIN`ed.
      Then it gap-fills: `AFTER` from each buffer's newest msgid, up to 10 pages, or `LATEST` when the buffer is empty. It also sends `TARGETS` from the newest cached time, and `MONITOR` for DM partners. Quitting the app closes the socket without sending `QUIT`.
  - SASL login with a stable per-install device ID (`account@<device>`). Phase 4 reuses the same ID for E2EE.
  - the channel list comes from the server on attach, so don't re-`JOIN` on reconnect. Leaving a channel is an explicit `PART` that persists.
  - gap fill on reconnect: `CHATHISTORY AFTER` from each buffer's last cached `msgid`, plus `CHATHISTORY TARGETS` to find DMs that arrived while offline
  - never set `autoreplay-missed`, because chathistory covers it and it breaks with more than one device
  - if a user turns always-on off by hand (via NickServ from another client), keep working, but rejoin channels on connect.

## chad-desktop

- [x] Layout:
  - network rail on the far left with the icon from discovery
  - channel list, chat log and member list
  - composer with typing indicator, replies and reactions
    `app.rs` (view model + panels), `chatlog.rs` (the spike's virtualized log, now over `ChatMessage`). Icons are fetched by `chad_core::fetch_icon` (1 MiB cap) and decoded with `image` limits (2048², 32 MiB), D14. The dev stack has no icon, so only the letter fallback has been seen.
    Hover a message for quick reactions (👍 ❤ 😂 😮), reply and delete (own). Clicking a reaction chip adds yours. There's no full emoji picker and no un-react (core has no command for it).
- [x] Message rendering:
  - mIRC formatting codes and links
  - mentions, highlighted, with a mention badge
  - grouping consecutive messages
  - day separators and a "new messages" line
    `format.rs`: bold/italic/underline/strike/mono/reverse/`\x03` 0–15/`\x04` hex; other control chars are dropped. Links are `http(s)://` only. The extended 16–98 palette renders as default (`shortcut:`).
    Grouping breaks after 5 min, on a day change, the unread line, or a reply. The "new messages" line is fixed when the buffer is opened and stays until you switch away. Times are local (`jiff`).
- [x] "Join a network" dialog plus `irchad://` scheme handling, with single-instance forwarding over D-Bus.
      Linux: `org.freedesktop.Application` on `party.gentoo.IrChad` (a second `irchad <url>` calls `Open` and exits; verified). The `.desktop` file is written to `$XDG_DATA_HOME/applications` with `xdg-mime default`, unless a packaged one exists or we're in a Flatpak.
      An invite for a network you're already in just switches to it (and joins `?channel=`).
- [x] Desktop notifications for mentions and DMs. Tray icon via SNI with an unread state.
      `notify-rust` (freedesktop on Linux, toasts on Windows). Only for messages under 60 s old that you aren't looking at, so gap-fill after a restart is quiet. The tray (`ksni`) shows a white dot for unread and a red one plus `NeedsAttention` for mentions. The window title gets `(n)` too.
      Not checked by eye yet: the notification popups and the tray icon on GNOME (needs the AppIndicator extension) and KDE.
- [x] Settings for theme (dark default), font size and notification rules. Persisted per user.
      eframe `persistence` (`~/.local/share/irchad/app.ron`). Notify: every message, mentions and DMs (default), or nothing.
- [x] Error UX in plain words: no raw numerics. "That username is taken", not `433`.
      Core already maps errors to words. The desktop shows them in the join dialog, in toasts, inline under failed sends, and under the network name for connection state, with a password box when signing in again is the fix.

### Carried over from Phase 0

- [x] System font fallback via fontconfig (Noto Sans CJK, Noto Emoji). egui's bundled fonts render CJK and emoji as tofu (see `notes/egui-chatlog.md`).
      eframe 0.37's `system_fonts` feature does this on demand (fontique, which links fontconfig, so CI needs `libfontconfig-dev`). CJK and emoji are confirmed rendering on X11; the emoji are monochrome.
- [x] IME check in the composer with fcitx5 or ibus, on Wayland (text-input-v3) and X11 (XIM).
      Wayland: works with fcitx5 Pinyin on Hyprland (text-input-v3), checked 2026-10-28; Unicode input (Ctrl+Alt+Shift+U) works too. X11 (XIM via XWayland, `XMODIFIERS=@im=fcitx`) works too. The composer is a plain `TextEdit::multiline` (Enter sends, Shift+Enter is a newline).
- [x] Discovery origins that glibc can't resolve (`chat.localhost`). Use `IRCHAD_HOST=localhost` for native dev, or make sure clients report the failure clearly.
      Resolved in the client: `*.localhost` goes to 127.0.0.1 (RFC 6761) for discovery, icons and the WebSocket (`chad_irc::ws::is_localhost`).

## Windows

- [x] Scheme handler registration, named-pipe single instance, toast notifications, Credential Manager.
      Builds and runs in a Windows VM against the Linux dev stack (2026-10-28): connect, chat, DMs, typing. `irchad://` links and single-instance forwarding are deferred to Phase 5 (user's call).
      Dev setup: `IRCHAD_HOST=irchad.test` in `deploy/.env`, hosts entries on both sides, Caddy's root in the Windows user Root store (`certutil -user -addstore Root`), and the host firewall opened for 8443 from `virbr0`. `HKCU\Software\Classes\irchad`, pipe `\\.\pipe\party.gentoo.IrChad-<user>`, `notify-rust` toasts, and `CredWriteW`/`CredReadW` (`irchad:<origin>`). Tray on Windows: `Shell_NotifyIconW` on a hidden window of its own thread, with close-to-tray and an HKCU `Run` autostart, both in Settings → Windows.

## Done when

Met 2026-10-28, except where noted. The user deferred GNOME/KDE, two networks at once, and `irchad://` links to Phase 5, when most features exist.
Verified: Linux and Windows clients plus irssi; DMs and typing between them; reconnect and restart with history (including as a guest); always-on; irssi sees nothing odd; IME on Wayland (Hyprland) and X11.

- With two clients plus irssi on the dev stack: invite → register → chat → restart → history and unread state are intact.
- Always-on: quit IrChad, then have irssi post in a channel and DM the user. irssi still sees the user in the channel (away). On relaunch both messages appear with correct unread counts, and no duplicate JOINs are sent.
- The client works in two networks at once.
- irssi users see nothing weird: no tag spam in bodies, and no CTCP.
- Verified on GNOME Wayland, KDE Wayland, one X11 session, and Windows.
