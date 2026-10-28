//! Phase 0 spike: a virtualized, variable-height chat log in egui.
//!
//!     cargo run --release -p chad-desktop --example chatlog_spike
//!
//! `SPIKE_BENCH=1` runs a scripted scroll from bottom to top (which triggers
//! history backfill), prints frame timings and exits.
//! Force X11 with `WAYLAND_DISPLAY= cargo run ...`.
//! Findings: docs/notes/egui-chatlog.md.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, FontId, Frame, Label, Layout, Rect, RichText, ScrollArea, UiBuilder, vec2,
};

const LOADED: usize = 45_000;
const BACKLOG: usize = 5_000;
const BACKFILL_BATCH: usize = 500;
const BACKFILL_LATENCY: Duration = Duration::from_millis(250);

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("chatlog spike")
            .with_inner_size([900.0, 700.0]),
        ..Default::default()
    };
    let bench = std::env::var_os("SPIKE_BENCH").is_some();
    eframe::run_native(
        "chatlog spike",
        options,
        Box::new(move |_cc| Ok(Box::new(Spike::new(bench)))),
    )
}

// ---------------------------------------------------------------------------
// Fake data

enum Body {
    Text(String),
    Code(String),
    Image { w: f32, h: f32, color: Color32 },
}

struct Msg {
    id: u64,
    author: &'static str,
    /// First message of a run from the same author: shows the header.
    header: bool,
    body: Body,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const AUTHORS: &[&str] = &["alice", "bob", "carol", "dave", "mallory"];
const WORDS: &[&str] = &[
    "the",
    "voice",
    "channel",
    "is",
    "lagging",
    "again",
    "anyone",
    "tried",
    "pipewire",
    "wayland",
    "screen",
    "share",
    "works",
    "now",
    "ergo",
    "metadata",
    "lol",
    "ship",
    "it",
    "rust",
    "egui",
    "borrow",
    "checker",
    "says",
    "no",
    "😀",
    "日本語",
    "テスト",
    "naïve",
    "café",
    "https://example.com/a/long/url",
];

fn gen_msgs(rng: &mut Rng, first_id: u64, n: usize) -> Vec<Msg> {
    let mut out = Vec::with_capacity(n);
    let mut last_author = "";
    for i in 0..n {
        let author = if rng.below(3) == 0 {
            AUTHORS[rng.below(AUTHORS.len() as u64) as usize]
        } else if last_author.is_empty() {
            AUTHORS[0]
        } else {
            last_author
        };
        let body = match rng.below(100) {
            0..=79 => {
                // Mostly short, sometimes a long wrapped paragraph.
                let words = if rng.below(8) == 0 {
                    40 + rng.below(120)
                } else {
                    1 + rng.below(18)
                };
                let mut s = String::new();
                for _ in 0..words {
                    if !s.is_empty() {
                        s.push(' ');
                    }
                    s.push_str(WORDS[rng.below(WORDS.len() as u64) as usize]);
                }
                Body::Text(s)
            }
            80..=91 => {
                let lines = 2 + rng.below(14);
                let mut s = String::new();
                for l in 0..lines {
                    s.push_str(&format!(
                        "{}let x{l} = foo({}); // {}\n",
                        "    ".repeat(rng.below(3) as usize),
                        rng.below(1000),
                        "comment ".repeat(rng.below(4) as usize)
                    ));
                }
                Body::Code(s.trim_end().to_owned())
            }
            _ => Body::Image {
                w: 160.0 + rng.below(240) as f32,
                h: 90.0 + rng.below(210) as f32,
                color: Color32::from_rgb(rng.below(200) as u8 + 30, 90, rng.below(200) as u8 + 30),
            },
        };
        out.push(Msg {
            id: first_id + i as u64,
            author,
            header: author != last_author,
            body,
        });
        last_author = author;
    }
    out
}

// ---------------------------------------------------------------------------
// Virtualized log

/// Heights are estimated until a row has been laid out once at the current
/// width. `prefix[i]` is the y of row i; `prefix[n]` is the content height.
struct ChatLog {
    msgs: Vec<Msg>,
    heights: Vec<f32>,
    measured: Vec<bool>,
    prefix: Vec<f32>,
    prefix_dirty: bool,
    width: f32,
    /// Applied on the next pass (usually a discarded re-run of this frame).
    pending_offset: Option<f32>,
    unread_from: Option<u64>,
    backlog: Vec<Msg>,
    backfill_started: Option<Instant>,
    rows_laid_out: usize,
    /// Scroll offset of the last pass.
    last_top: f32,
    /// Viewport height of the last pass.
    view_height: f32,
    /// (top row id, px of it hidden above the viewport) just before a prepend;
    /// checked on the next pass to measure keep-position drift.
    prepend_anchor: Option<(u64, f32)>,
    max_prepend_drift: f32,
}

const ROW_GAP: f32 = 2.0;

impl ChatLog {
    fn estimate(msg: &Msg, width: f32) -> f32 {
        let header = if msg.header { 22.0 } else { 0.0 };
        let body = match &msg.body {
            Body::Text(s) => {
                let chars_per_line = (width / 7.5).max(10.0);
                (s.chars().count() as f32 / chars_per_line).ceil().max(1.0) * 17.0
            }
            Body::Code(s) => s.lines().count() as f32 * 15.0 + 12.0,
            Body::Image { h, .. } => *h,
        };
        header + body + ROW_GAP
    }

    fn rebuild_prefix(&mut self) {
        if !self.prefix_dirty {
            return;
        }
        self.prefix.clear();
        self.prefix.reserve(self.heights.len() + 1);
        let mut y = 0.0;
        self.prefix.push(0.0);
        for h in &self.heights {
            y += h;
            self.prefix.push(y);
        }
        self.prefix_dirty = false;
    }

    fn set_width(&mut self, width: f32) {
        if (width - self.width).abs() < 0.5 {
            return;
        }
        self.width = width;
        for (i, m) in self.msgs.iter().enumerate() {
            self.heights[i] = Self::estimate(m, width);
            self.measured[i] = false;
        }
        self.prefix_dirty = true;
    }

    fn prepend(&mut self, older: Vec<Msg>) -> f32 {
        let hs: Vec<f32> = older
            .iter()
            .map(|m| Self::estimate(m, self.width))
            .collect();
        let added: f32 = hs.iter().sum();
        let n = older.len();
        self.msgs.splice(0..0, older);
        self.heights.splice(0..0, hs);
        self.measured.splice(0..0, std::iter::repeat_n(false, n));
        self.prefix_dirty = true;
        added
    }

    fn push(&mut self, msg: Msg) {
        self.heights.push(Self::estimate(&msg, self.width));
        self.measured.push(false);
        self.msgs.push(msg);
        self.prefix_dirty = true;
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.msgs.binary_search_by_key(&id, |m| m.id).ok()
    }

    /// A real offset, not f32::MAX: egui uses an out-of-range offset as-is for
    /// one pass, which paints the whole log off-screen.
    fn scroll_to_bottom(&mut self) {
        self.rebuild_prefix();
        let total = *self.prefix.last().unwrap();
        self.pending_offset = Some((total - self.view_height).max(0.0));
    }

    fn scroll_to_index(&mut self, i: usize, margin: f32) {
        self.rebuild_prefix();
        self.pending_offset = Some((self.prefix[i] - margin).max(0.0));
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.set_width(ui.available_width() - 16.0);
        self.rebuild_prefix();

        let mut area = ScrollArea::vertical()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .animated(false);
        if let Some(off) = self.pending_offset.take() {
            area = area.vertical_scroll_offset(off);
        }

        let mut compensate = 0.0;
        let mut near_top = false;
        let out = area.show_viewport(ui, |ui, viewport| {
            let total = *self.prefix.last().unwrap();
            ui.set_height(total);
            let origin = ui.min_rect().min;
            near_top = viewport.min.y < 400.0;

            let first = self
                .prefix
                .partition_point(|&y| y <= viewport.min.y)
                .saturating_sub(1)
                // An offset past the end would otherwise land on prefix[n],
                // one past the last row.
                .min(self.msgs.len() - 1);
            let mut y = self.prefix[first];
            let mut i = first;
            let mut changed = false;
            while i < self.msgs.len() && y < viewport.max.y {
                let rect =
                    Rect::from_min_size(origin + vec2(0.0, y), vec2(self.width, f32::INFINITY));
                let unread = self.unread_from == Some(self.msgs[i].id);
                let resp = ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(rect)
                        .layout(Layout::top_down(Align::Min))
                        .id_salt(self.msgs[i].id),
                    |ui| row_ui(ui, &self.msgs[i], unread),
                );
                self.rows_laid_out += 1;
                let h = resp.response.rect.height() + ROW_GAP;
                if !self.measured[i] || (h - self.heights[i]).abs() > 0.5 {
                    let d = h - self.heights[i];
                    // Keep whatever is at the top of the viewport still: if this
                    // row starts above it, everything visible moves by d.
                    if self.prefix[i] < viewport.min.y && d.abs() > 0.5 {
                        compensate += d;
                    }
                    self.heights[i] = h;
                    self.measured[i] = true;
                    changed = true;
                }
                y += h;
                i += 1;
            }
            if changed {
                self.prefix_dirty = true;
            }
            (
                viewport,
                (self.msgs[first].id, viewport.min.y - self.prefix[first]),
            )
        });

        let (viewport, anchor) = out.inner;
        if let Some((id, into)) = self.prepend_anchor.take() {
            let drift = if id == anchor.0 {
                (into - anchor.1).abs()
            } else {
                f32::INFINITY
            };
            self.max_prepend_drift = self.max_prepend_drift.max(drift);
        }
        self.last_top = viewport.min.y;
        self.view_height = viewport.height();
        let at_bottom = viewport.max.y >= out.content_size.y - 2.0;
        if compensate != 0.0 && !at_bottom {
            self.pending_offset = Some(viewport.min.y + compensate);
            ui.ctx()
                .request_discard("chat log row heights changed above viewport");
        }

        // History backfill: simulated network round-trip, then prepend while
        // keeping the current view fixed.
        if near_top && !self.backlog.is_empty() {
            match self.backfill_started {
                None => self.backfill_started = Some(Instant::now()),
                Some(t) if t.elapsed() >= BACKFILL_LATENCY => {
                    let take = BACKFILL_BATCH.min(self.backlog.len());
                    let older = self.backlog.split_off(self.backlog.len() - take);
                    self.prepend_anchor = Some(anchor);
                    let added = self.prepend(older);
                    self.pending_offset =
                        Some(self.pending_offset.unwrap_or(viewport.min.y) + added);
                    self.backfill_started = None;
                    ui.ctx().request_discard("history prepended");
                }
                Some(_) => ui.ctx().request_repaint_after(BACKFILL_LATENCY),
            }
        }
    }
}

fn row_ui(ui: &mut egui::Ui, msg: &Msg, unread: bool) {
    if unread {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("NEW")
                    .color(Color32::from_rgb(240, 70, 70))
                    .small()
                    .strong(),
            );
            ui.separator();
        });
    }
    if msg.header {
        ui.horizontal(|ui| {
            ui.label(RichText::new(msg.author).strong());
            ui.label(RichText::new(format!("#{}", msg.id)).weak().small());
        });
    }
    match &msg.body {
        Body::Text(s) => {
            ui.add(Label::new(s.as_str()).wrap());
        }
        Body::Code(s) => {
            Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .inner_margin(6.0)
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.add(
                        Label::new(RichText::new(s.as_str()).font(FontId::monospace(12.0))).wrap(),
                    );
                });
        }
        Body::Image { w, h, color } => {
            let (r, _) = ui.allocate_exact_size(vec2(*w, *h), egui::Sense::click());
            ui.painter().rect_filled(r, 4.0, *color);
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                format!("{w}×{h}"),
                FontId::proportional(12.0),
                Color32::WHITE,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// App

struct Bench {
    frame: usize,
    ui_times: Vec<Duration>,
    dts: Vec<f32>,
    start: Instant,
}

struct Spike {
    log: ChatLog,
    composer: String,
    rng: Rng,
    next_id: u64,
    bench: Option<Bench>,
    last_ui: Duration,
}

impl Spike {
    fn new(bench: bool) -> Self {
        let mut rng = Rng(0x5eed_1234_abcd);
        let backlog = gen_msgs(&mut rng, 0, BACKLOG);
        let msgs = gen_msgs(&mut rng, BACKLOG as u64, LOADED);
        let next_id = (BACKLOG + LOADED) as u64;
        let n = msgs.len();
        let log = ChatLog {
            heights: vec![0.0; n],
            measured: vec![false; n],
            prefix: Vec::new(),
            prefix_dirty: true,
            width: 0.0,
            pending_offset: None,
            unread_from: Some(next_id - 3_000),
            backlog,
            backfill_started: None,
            rows_laid_out: 0,
            last_top: 0.0,
            view_height: 0.0,
            prepend_anchor: None,
            max_prepend_drift: 0.0,
            msgs,
        };
        Self {
            log,
            composer: String::new(),
            rng,
            next_id,
            bench: bench.then(|| Bench {
                frame: 0,
                ui_times: Vec::new(),
                dts: Vec::new(),
                start: Instant::now(),
            }),
            last_ui: Duration::ZERO,
        }
    }

    fn send(&mut self) {
        let text = self.composer.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let mut msg = gen_msgs(&mut self.rng, self.next_id, 1).pop().unwrap();
        msg.author = "you";
        msg.header = true;
        msg.body = Body::Text(text);
        self.next_id += 1;
        self.log.push(msg);
        self.log.scroll_to_bottom();
        self.composer.clear();
    }

    fn bench_step(&mut self, ctx: &egui::Context) {
        let Some(b) = &mut self.bench else { return };
        b.frame += 1;
        b.ui_times.push(self.last_ui);
        b.dts.push(ctx.input(|i| i.unstable_dt));
        // Scroll up 60 px/frame (fast wheel scrolling), starting from the bottom.
        if self.log.pending_offset.is_none() {
            self.log.pending_offset = Some((self.log.last_top - 60.0).max(0.0));
        }
        let done_scrolling = self.log.backlog.is_empty() && self.log.last_top <= 0.0;
        if done_scrolling || b.start.elapsed() > Duration::from_secs(120) {
            report(b, &self.log);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }
}

fn pct(v: &mut [Duration], p: f64) -> Duration {
    v.sort();
    v[((v.len() as f64 - 1.0) * p) as usize]
}

fn report(b: &Bench, log: &ChatLog) {
    let mut ui = b.ui_times.clone();
    let mut dts: Vec<Duration> = b.dts.iter().map(|s| Duration::from_secs_f32(*s)).collect();
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let backend = if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|s| !s.is_empty()) {
        "wayland"
    } else {
        "x11"
    };
    println!(
        "backend={backend} session={session} frames={} wall={:.1}s",
        b.frame,
        b.start.elapsed().as_secs_f32()
    );
    println!(
        "rows total={} backlog_left={} rows_laid_out={}",
        log.msgs.len(),
        log.backlog.len(),
        log.rows_laid_out
    );
    println!(
        "max keep-position drift across prepends: {} px",
        log.max_prepend_drift
    );
    println!(
        "ui pass time: p50={:?} p99={:?} max={:?}",
        pct(&mut ui, 0.5),
        pct(&mut ui, 0.99),
        pct(&mut ui, 1.0)
    );
    println!(
        "frame dt:     p50={:?} p99={:?} max={:?}",
        pct(&mut dts, 0.5),
        pct(&mut dts, 0.99),
        pct(&mut dts, 1.0)
    );
}

impl eframe::App for Spike {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t0 = Instant::now();
        let ctx = ui.ctx().clone();

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Jump to unread").clicked()
                    && let Some(i) = self.log.unread_from.and_then(|id| self.log.index_of(id))
                {
                    self.log.scroll_to_index(i, 40.0);
                }
                if ui.button("Jump to bottom").clicked() {
                    self.log.scroll_to_bottom();
                }
                ui.separator();
                ui.label(format!(
                    "{} msgs, {} in backlog, ui {:.2} ms",
                    self.log.msgs.len(),
                    self.log.backlog.len(),
                    self.last_ui.as_secs_f64() * 1e3
                ));
                if self.log.backfill_started.is_some() {
                    ui.spinner();
                }
            });
        });

        egui::Panel::bottom("composer").show(ui, |ui| {
            let resp = ui.add(
                egui::TextEdit::multiline(&mut self.composer)
                    .hint_text("Message #spike (Enter to send, Shift+Enter for newline; try IME)")
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            if resp.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift)
            {
                // TextEdit has already inserted the newline.
                if self.composer.ends_with('\n') {
                    self.composer.pop();
                }
                self.send();
            }
        });

        egui::CentralPanel::default().show(ui, |ui| self.log.ui(ui));

        self.last_ui = t0.elapsed();
        self.bench_step(&ctx);
    }
}
