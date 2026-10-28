# Phase 1: Connect & chat

**Goal:** a Discord user pastes an invite URL, creates an account, and chats, without learning any IRC concept. The client supports multiple networks from day one.

## chad-irc

- [x] Message parse/serialize with tags (escaping), source and params. Write property tests and a fuzz target.
      Fuzz target is `crates/chad-irc/fuzz` (standalone). On stable: `RUSTC_BOOTSTRAP=1 cargo fuzz run message -s none` (no unsafe, so no ASan needed). 32.7M runs / 3 min clean on 2026-10-28. Deliberately not in CI: run it as a pre-release check (see Phase 5).
- [x] CAP 302 negotiation, including `cap-notify` for runtime changes. Request every capability in `ARCHITECTURE.md` that ergo advertises.
      `chad_irc::cap::Negotiator` (sans-IO); the requested set is `cap::WANTED`. Requests `draft/metadata-2` only: `-3` is an unmerged spec PR (ircv3/ircv3-specifications#613) that ergo already advertises. Revisit when it merges.
- [ ] chad-core: surface cap `NEW`/`DEL` to the UI layer (e.g. `away-notify` dropped → fall back to polling), once core exists.
- [x] SASL SCRAM-SHA-256 with PLAIN fallback, and `REGISTER` (draft/account-registration) including the verify step.
      `chad_irc::sasl::Sasl` (verifies the server signature; falls back to PLAIN only if SCRAM is refused before the proof) and `chad_irc::register`.
      Live check: `cargo test -p chad-irc --test ergo_live -- --ignored` (each run leaves an `lt<timestamp>` account behind).
      ergo only registers the current nick (`ACCOUNT_NAME_MUST_BE_NICK`), so send `NICK <username>` then `REGISTER * …`.
      `VERIFY` is unit-tested only: the dev stack has email verification off.
- [ ] chad-core: after `REGISTER` succeeds the connection is logged in as `bob`, not `bob@<device>`. Reconnect with SASL so the device ID is in place (D11).
- [ ] Batch reassembly (nested), labeled-response correlation (request → future), ISUPPORT, and casemapping.
- [ ] WebSocket transport (`text.ircv3.net`), keepalive, and reconnect with backoff.

## chad-core

- [ ] Network model: origin, discovery doc, credentials (secret store), and connection state.
- [ ] Join flow: parse an invite (https, `irchad://`, bare host) → discovery → register or login → first-login auto-join from `LIST`.
- [ ] Buffers: channels and DMs. History backfill via `CHATHISTORY` (latest, before, around), with dedupe by `msgid`.
- [ ] Members: names, away status, account. Presence via `away-notify`, `account-notify`, `extended-join` and MONITOR for DM partners.
- [ ] Read markers (`MARKREAD`) feed unread and mention counts and sync across devices.
- [ ] Send path: `echo-message`, pending/failed state, `draft/multiline` for pasted blocks, `+typing`, `+draft/reply`, `+draft/react`, and `REDACT` for your own messages.
- [ ] SQLite cache per network, so the last scrollback is shown instantly at startup.
- [ ] Always-on (on by default server-side, D11). The client acts like a bouncer client, not a fresh IRC client:
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
