# Phase 1: Connect & chat

**Goal:** a Discord user pastes an invite URL, creates an account, and chats, without learning any IRC concept. The client supports multiple networks from day one.

## chad-irc
- [ ] Message parse/serialize with tags (escaping), source and params. Write property tests and a fuzz target.
- [ ] CAP 302 negotiation, including `cap-notify` for runtime changes. Request every capability in `ARCHITECTURE.md` that ergo advertises.
- [ ] SASL SCRAM-SHA-256 with PLAIN fallback, and `REGISTER` (draft/account-registration) including the verify step.
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

## Windows
- [ ] Scheme handler registration, named-pipe single instance, toast notifications, Credential Manager.

## Done when
- With two clients plus irssi on the dev stack: invite → register → chat → restart → history and unread state are intact.
- Always-on: quit IrChad, then have irssi post in a channel and DM the user. irssi still sees the user in the channel (away). On relaunch both messages appear with correct unread counts, and no duplicate JOINs are sent.
- The client works in two networks at once.
- irssi users see nothing weird: no tag spam in bodies, and no CTCP.
- Verified on GNOME Wayland, KDE Wayland, one X11 session, and Windows.
