# Phase 7: Later / parking lot

These are not scheduled. Move an item into a phase only when the user asks.

- Webcam (desktop): PipeWire Camera portal on Linux, Media Foundation on Windows.
- Admin UI: kick, ban, channel management. Until then, admins use a normal IRC client.
  When this lands, chadd must also enforce bans and channel membership for voice tokens.
- Invite codes that gate registration (chadd mints codes, and registration goes through the ergo API).
- DM voice/video calls (a LiveKit room per DM pair, with ringing via TAGMSG).
- Message editing: move from `+chad/edit` (D51) to the IRCv3 spec once one is ratified and ergo implements it.
- Custom emoji per network (a chadd-hosted set advertised in discovery).
- One-time prekeys for E2EE, post-quantum handshake, and MLS for encrypted channels.
- macOS: accepted if it happens to work, never targeted.
