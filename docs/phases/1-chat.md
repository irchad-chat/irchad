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

- [ ] Layout:
  - network rail on the far left with the icon from discovery
  - channel list, chat log and member list
  - composer with typing indicator, replies and reactions
- [ ] Message rendering:
  - mIRC formatting codes and links
  - mentions, highlighted, with a mention badge
  - grouping consecutive messages
  - day separators and a "new messages" line
- [ ] "Join a network" dialog plus `irchad://` scheme handling, with single-instance forwarding over D-Bus.
- [ ] Desktop notifications for mentions and DMs. Tray icon via SNI with an unread state.
- [ ] Settings for theme (dark default), font size and notification rules. Persisted per user.
- [ ] Error UX in plain words: no raw numerics. "That username is taken", not `433`.

### Carried over from Phase 0

- [ ] System font fallback via fontconfig (Noto Sans CJK, Noto Emoji). egui's bundled fonts render CJK and emoji as tofu (see `notes/egui-chatlog.md`).
- [ ] IME check in the composer with fcitx5 or ibus, on Wayland (text-input-v3) and X11 (XIM).
- [ ] Discovery origins that glibc can't resolve (`chat.localhost`). Use `IRCHAD_HOST=localhost` for native dev, or make sure clients report the failure clearly.

## Windows

- [ ] Scheme handler registration, named-pipe single instance, toast notifications, Credential Manager.

## Done when

- With two clients plus irssi on the dev stack: invite → register → chat → restart → history and unread state are intact.
- Always-on: quit IrChad, then have irssi post in a channel and DM the user. irssi still sees the user in the channel (away). On relaunch both messages appear with correct unread counts, and no duplicate JOINs are sent.
- The client works in two networks at once.
- irssi users see nothing weird: no tag spam in bodies, and no CTCP.
- Verified on GNOME Wayland, KDE Wayland, one X11 session, and Windows.
