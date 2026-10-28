# Plan

**Current phase: 1 (Connect & chat)**

Update the line above when a phase's "Done when" holds.

| # | Phase | File | Status |
|---|---|---|---|
| 0 | Foundations: workspace, dev stack, CI, risk spikes | [phases/0-foundations.md](phases/0-foundations.md) | done (2026-10-28) |
| 1 | Connect & chat: invite URLs, multi-network, text MVP | [phases/1-chat.md](phases/1-chat.md) | in progress |
| 2 | Voice | [phases/2-voice.md](phases/2-voice.md) | not started |
| 3 | Screen share | [phases/3-screenshare.md](phases/3-screenshare.md) | not started |
| 4 | Encrypted DMs | [phases/4-e2ee-dms.md](phases/4-e2ee-dms.md) | not started |
| 5 | Polish & distribution | [phases/5-polish.md](phases/5-polish.md) | not started |
| 6 | Android | [phases/6-android.md](phases/6-android.md) | not started |
| 7 | Later / parking lot | [phases/7-later.md](phases/7-later.md) | — |

## Ordering rules

- Linux (Wayland **and** X11) is verified at the end of every phase. Windows gets at least a build plus a smoke test.
- Phases 2 and 3 depend on 1. Phase 4 depends on 1 and is independent of 2 and 3, so it can be pulled earlier if the user wants.
- Phase 6 needs `chad-core` to be stable, which means after Phase 4 at the earliest.
- "Better than Discord on Linux" is a requirement, not a slogan. Each media phase has explicit Linux items where Discord fails:
  Wayland screen share with audio, PTT on Wayland, PipeWire-native devices, and hardware encode.
