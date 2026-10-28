# Spike: egui chat log

**Verdict: GO.** Performance, keep-position and cross-message selection are confirmed by hand on Hyprland (2026-10-28). The IME check moved to Phase 1, because no IME was installed to test with.
Code: `crates/chad-desktop/examples/chatlog_spike.rs` (egui/eframe 0.37, wgpu).

```sh
cargo run --release -p chad-desktop --example chatlog_spike                 # interactive
SPIKE_BENCH=1 cargo run --release -p chad-desktop --example chatlog_spike   # scripted bench
WAYLAND_DISPLAY= ...                                                         # force X11 (XWayland)
```

## Design that worked

- **Our own virtualization** on top of `ScrollArea::show_viewport`. We don't use `show_rows`, because it needs uniform row heights.
  Each row has a height, estimated until it's first laid out at the current width, and a prefix-sum array gives y offsets.
  Each pass binary-searches the first visible row and lays out rows until the viewport is filled. Only visible rows ever get laid out.
  A width change re-estimates every row and marks them unmeasured.
- **Anchor to the top of the viewport.** When a row that starts above the viewport changes measured height, we shift the scroll offset by the delta,
  so the content you're looking at doesn't move. We apply the shift with `ctx.request_discard()`.
  egui re-runs the pass immediately and never shows the wrong frame, so there's no visible jump.
  This is skipped while stuck to the bottom, where `stick_to_bottom` already does the right thing.
- **Prepend (history backfill):** splice the older rows in with estimated heights, add their total height to the scroll offset, and `request_discard()`.
  The rows themselves get measured lazily if the user scrolls up into them.
- **Jump to unread:** `scroll_to_index` sets the offset to `prefix[i]`. Rows above the target aren't measured on the way, so the estimate-based position is stable.
  The unread divider is drawn in-row.
- Rows are placed at a running y within the frame, starting at `prefix[first]`, so rows never overlap even when estimates are off.

## Results (Hyprland, RTX 4090, 120 Hz, release build)

The scripted bench starts at the bottom of 45,000 messages and scrolls up 60 px per frame. That's fast wheel scrolling.
Near the top it backfills 5,000 older messages in 10 batches of 500 with a 250 ms simulated round trip, until it reaches message #0.
The message mix is about 80% text (1 in 8 a long wrapped paragraph, with CJK, emoji and URLs), 12% code blocks and 8% inline images, plus author headers.

| | Wayland | X11 (XWayland) |
|---|---|---|
| frames | 6,823 | 6,922 |
| ui pass CPU p50 / p99 / max | 0.095 / 0.27 / 7.1 ms | 0.086 / 0.26 / 6.4 ms |
| frame dt p50 / p99 | 8.28 / 9.77 ms | 8.25 / 8.77 ms |
| keep-position drift across all 10 prepends | **0 px** | — |

The ui pass stays far under budget, and frame time sits on the 120 Hz vsync.
The max of roughly 7 ms is the first frames: font atlas warm-up and initial measurement.
50,000 messages cost nothing per frame, because only around 15 rows are laid out.
Drift is measured as the top row's id and how far it's scrolled past, compared just before and after each prepend.

## Needs a human (not automatable here)

These have to be checked on both Wayland and X11. The spike has everything needed to try them.

- [x] **Text selection across messages.** Works (confirmed by hand). egui's `LabelSelectionState` supports drag-selecting across multiple `Label`s in one pass, and every row body here is a `Label`.
  Expected limitation: when a selection's anchor row scrolls out of view it isn't laid out any more, so the selection may drop.
  If that matters, Phase 1 needs either app-level selection state (a msgid range) or keeping the anchor row alive while dragging.
- [ ] **IME in the composer** (deferred to Phase 1; needs fcitx5 or ibus installed) (`TextEdit::multiline`), with fcitx5 or ibus on Wayland (text-input-v3) and X11 (XIM).
  Check the preedit position and that commit works.
- [ ] Wheel and touchpad scrolling feel, since the bench drives the offset directly.

## Bugs found by hand (fixed)

- Jump-to-bottom (and sending) used `vertical_scroll_offset(f32::MAX)`. egui applies an out-of-range offset as-is for one pass, so the log is painted entirely off-screen.
  Nothing repaints afterwards, so it stays blank until the next input, and in one path it also indexed one past the last row and panicked.
  Fix: scroll to the real bottom (`total - viewport height`), and clamp the first-row lookup.

## Notes for Phase 1

- **Fonts:** egui's bundled fonts have no CJK or emoji, so they render as tofu (□). Load system fallbacks through fontconfig, e.g. Noto Sans CJK and Noto Emoji.
  egui can't render color emoji (no COLR/CBDT support), so use the monochrome `Noto Emoji` for now, and look at color emoji in Phase 5.

- egui 0.37's API is `App::ui(&mut self, ui: &mut Ui, …)` with `Panel::top/bottom/left` (no separate `TopBottomPanel`/`SidePanel`), and panels take a `&mut Ui`.
- Real inline images will come from `egui::Image` with a known size from metadata, such as image dimensions in a tag or from a HEAD request.
  Unknown sizes should get a placeholder size, so the image doesn't reflow everything below it.
- Persist the height cache per buffer and width bucket if switching channels feels slow. At 50k rows it isn't needed.
