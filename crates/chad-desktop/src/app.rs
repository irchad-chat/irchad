//! The window: projects core events into a view model and draws it.
//!
//! Layout, left to right: network rail, channel list (with the voice panel at
//! its foot during a call), then the open buffers tiled as panes, each with
//! its own chat (topic, log, composer) and member list.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chad_core::{
    Auth, Blob, BufferInfo, BufferKind, ChatMessage, Command, ConnState, Core, DeviceInfo,
    Encryption, Event, Member, NetCommand, SendStatus, Typing, VerifyState, VerifyStep, VoiceState,
    invite,
};
use chad_media::{InputMode, Voice};
use egui::{
    Align2, Color32, CornerRadius, FontId, Key, KeyboardShortcut, Modifiers, RichText, Sense,
    TextureHandle, vec2,
};
use tokio::sync::mpsc::UnboundedReceiver;
use url::Url;

use crate::chatlog::{self, Action, ChatLog, Extras};
use crate::icons::{self, Tip};
use crate::media::{self, Media};
use crate::platform::idle::Idle;
use crate::platform::ptt::Ptt;
use crate::platform::{self, Os, Tray};
use crate::screen::{self, DialogOutcome, Layout, ShareDialog, StageAction};
use crate::settings::{self, Notify, Settings};
use crate::voice;
use crate::{complete, emoji, format, gifs, update};

const TOAST_TTL: Duration = Duration::from_secs(8);
/// Only messages this fresh notify; gap-fill after a restart stays quiet.
const NOTIFY_WINDOW_MS: i64 = 60_000;
/// The AWAY reason other clients show.
const AWAY: &str = "Away";

/// A network's decoded icon, or `None` if it couldn't be had.
type IconMsg = (String, Option<egui::ColorImage>);
/// A file from the file dialog: (network, buffer or `None` for an avatar, path).
type Picked = (String, Option<String>, std::path::PathBuf);
/// (network, file, the clip or why not).
type Decoded = (String, String, Result<std::sync::Arc<[i16]>, String>);

pub struct App {
    core: Core,
    events: UnboundedReceiver<Event>,
    os: mpsc::Receiver<Os>,
    tray: Tray,
    icons: (mpsc::Sender<IconMsg>, mpsc::Receiver<IconMsg>),
    view: View,
    /// Origin of the network on screen.
    current: Option<String>,
    /// Buffers tiled in the main window, as (network, key). The focused one,
    /// `panes[pane]`, is `current`'s selected buffer.
    panes: Vec<(String, String)>,
    pane: usize,
    /// Show the next selected buffer in the focused pane, not a new one.
    replace_pane: bool,
    settings: Settings,
    show_settings: bool,
    join: Option<JoinDialog>,
    verify_code: String,
    signin_password: String,
    verify_dismissed: Option<String>,
    confirm_remove: Option<String>,
    /// A guest making an account.
    signup: Option<SignUpDialog>,
    adding: Option<(BufferKind, String)>,
    toasts: Vec<(String, Instant)>,
    last_badge: (bool, u32),
    voice: Voice,
    /// Listening for push-to-talk, while that's the input mode.
    ptt: Option<Ptt>,
    binding_ptt: bool,
    share_dialog: Option<ShareDialog>,
    stage: Stage,
    picker: Option<Picker>,
    switcher: Option<Switcher>,
    profile: Option<ProfileDialog>,
    /// The emoji verification on screen: (network, nick, where it stands).
    verify: Option<(String, String, VerifyState)>,
    files: (mpsc::Sender<Picked>, mpsc::Receiver<Picked>),
    /// Decoded clips by (network, file).
    // shortcut: an admin replacing a file under the same name isn't picked up
    // until restart; key on a hash or mtime from chadd if that matters.
    clips: HashMap<(String, String), std::sync::Arc<[i16]>>,
    /// Clips decoded off the UI thread, to play.
    decoded: (mpsc::Sender<Decoded>, mpsc::Receiver<Decoded>),
    /// Numbers attachments, for their preview's key.
    attached: u64,
    idle: Idle,
    /// Whether the networks were last told we're away.
    away_sent: bool,
    update: mpsc::Receiver<(String, String)>,
    /// A newer release: (version, page).
    new_version: Option<(String, String)>,
}

/// The emoji picker, for the composer or a reaction. The composer's has a
/// GIF tab too, when the network searches GIFs.
struct Picker {
    react: Option<(String, String, String)>,
    at: egui::Pos2,
    query: String,
    /// Shown for a frame already: the click that opened it is no "click elsewhere".
    shown: bool,
    gif_tab: bool,
    gifs: gifs::Gifs,
}

/// Ctrl+K: jump to any channel or DM by name.
#[derive(Default)]
struct Switcher {
    query: String,
    sel: usize,
}

struct ProfileDialog {
    network: String,
    name: String,
}

/// How a watched screen share is shown (what is watched lives in `Voice`,
/// where it goes in `Settings::stage`).
#[derive(Default)]
struct Stage {
    fullscreen: bool,
    /// Its cell in the pane grid when tiled.
    cell: usize,
}

/// The view model from the core, with the window's own state on each network
/// and buffer.
type View = chad_core::view::View<NetUi, BufUi>;
type Network = chad_core::view::Net<NetUi, BufUi>;

#[derive(Default)]
struct NetUi {
    /// Key of the buffer on screen.
    selected: Option<String>,
    /// Put the cursor in the composer on the next frame.
    focus: bool,
    icon: Icon,
    media: Media,
}

#[derive(Default)]
enum Icon {
    #[default]
    None,
    Loading,
    Ready(TextureHandle),
    Failed,
}

#[derive(Default)]
struct BufUi {
    log: ChatLog,
    members_asked: bool,
    older_asked: Option<Instant>,
    mark_sent: bool,
    draft: String,
    /// The Tab completion in progress in the draft.
    tab: Option<complete::Tab>,
    /// (msgid, nick) of the message being replied to.
    reply: Option<(String, String)>,
    /// A message to scroll to once `LoadAround` brings it in.
    jump: Option<String>,
    /// Names of files on their way up.
    uploads: Vec<String>,
    /// Files waiting in the composer, uploaded when the message is sent.
    attached: Vec<Attachment>,
}

struct Attachment {
    name: String,
    bytes: Blob,
    /// Its preview in the network's `Media`.
    key: String,
}

#[derive(Default)]
struct JoinDialog {
    invite: String,
    mode: JoinMode,
    account: String,
    password: String,
    email: String,
    /// The invite sent to the core, while waiting for the outcome.
    busy: Option<String>,
    error: Option<String>,
}

/// A guest signing up on the network they're in.
struct SignUpDialog {
    network: String,
    account: String,
    password: String,
    email: String,
    /// Sent to the core, while waiting for the outcome.
    busy: bool,
    error: Option<String>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum JoinMode {
    #[default]
    Login,
    Register,
    Guest,
}

use chad_core::view::key;

/// The nicks a buffer's composer completes: the channel's members, or the
/// other side of a DM.
/// Narrowest a chat column gets before its pane hides the member list.
const MIN_CHAT_W: f32 = 360.0;

/// A dialog's button row: right-aligned, the primary action accent-filled and
/// rightmost. Returns (primary clicked, cancel clicked).
pub(crate) fn footer(ui: &mut egui::Ui, primary: &str, enabled: bool, busy: bool) -> (bool, bool) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let sel = ui.visuals().selection;
        let go =
            egui::Button::new(RichText::new(primary).color(sel.stroke.color)).fill(sel.bg_fill);
        let go = ui.add_enabled(enabled, go).clicked();
        let cancel = ui.button("Cancel").clicked();
        if busy {
            ui.spinner();
        }
        (go, cancel)
    })
    .inner
}

fn completable(
    members: &Option<Vec<Member>>,
    msgs: &[ChatMessage],
    info: &BufferInfo,
    own: &str,
) -> Vec<String> {
    let recent = msgs.iter().rev().map(|m| m.nick.as_str());
    match members {
        Some(ms) => complete::candidates(ms.iter().map(|m| m.nick.as_str()), recent, own),
        None if info.kind == BufferKind::Dm => {
            complete::candidates([info.target.as_str()], recent, own)
        }
        None => Vec::new(),
    }
}

fn state_text(state: &ConnState) -> Option<(String, bool)> {
    match state {
        ConnState::Connected => None,
        ConnState::Connecting => Some(("Connecting…".into(), false)),
        ConnState::NeedsVerification { .. } => Some(("Waiting for your email code".into(), false)),
        ConnState::Disconnected { error } => Some((format!("{error} Reconnecting…"), false)),
        ConnState::Failed { error } => Some((error.clone(), true)),
    }
}

/// Columns in the pane grid: as square as it gets, wider than tall.
fn pane_cols(n: usize) -> usize {
    (1..).find(|c| c * c >= n).unwrap_or(1)
}

/// Drag-and-drop payload: the grid cell of the pane whose title is being dragged.
pub struct PaneDrag(pub usize);

fn badge(n: u32) -> String {
    if n > 99 { "99+".into() } else { n.to_string() }
}

/// Shows a buffer, with the "new messages" line above the first unread one.
fn select(n: &mut Network, k: &str) {
    n.ui.selected = Some(k.to_owned());
    n.ui.focus = true;
    let Some(i) = n.buffer(k) else { return };
    let b = &mut n.buffers[i];
    let mut left = b.info.unread;
    b.ui.log.unread_from = None;
    if left > 0 {
        for (idx, m) in b.msgs.iter().enumerate().rev() {
            if !m.own {
                left -= 1;
                if left == 0 {
                    b.ui.log.unread_from = m.msgid.clone();
                    b.ui.log.scroll_to_index(idx, 60.0);
                    break;
                }
            }
        }
    }
    if b.ui.log.unread_from.is_none() {
        b.ui.log.scroll_to_bottom();
    }
    b.ui.mark_sent = false;
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        core: Core,
        events: UnboundedReceiver<Event>,
        os: mpsc::Receiver<Os>,
        tray: Tray,
        open: Option<String>,
        voice: Voice,
    ) -> Self {
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, settings::KEY))
            .unwrap_or_default();
        icons::install(&cc.egui_ctx);
        screen::install(cc);
        settings.apply(&cc.egui_ctx);
        voice.set_settings(settings.audio.clone());
        for (id, v) in &settings.volumes {
            voice.set_volume(id, *v);
        }
        for (id, v) in &settings.screen_volumes {
            voice.set_screen_volume(id, *v);
        }
        let settings_check_updates = settings.check_updates;
        let mut app = Self {
            core,
            events,
            os,
            tray,
            icons: mpsc::channel(),
            view: View::default(),
            current: None,
            panes: Vec::new(),
            pane: 0,
            replace_pane: false,
            settings,
            show_settings: false,
            join: None,
            verify_code: String::new(),
            signin_password: String::new(),
            verify_dismissed: None,
            confirm_remove: None,
            signup: None,
            adding: None,
            toasts: Vec::new(),
            last_badge: (false, 0),
            voice,
            ptt: None,
            binding_ptt: false,
            share_dialog: None,
            stage: Stage::default(),
            picker: None,
            switcher: None,
            profile: None,
            verify: None,
            files: mpsc::channel(),
            clips: HashMap::new(),
            decoded: mpsc::channel(),
            attached: 0,
            idle: Idle::start({
                let ctx = cc.egui_ctx.clone();
                move || ctx.request_repaint()
            }),
            away_sent: false,
            new_version: None,
            update: {
                let (tx, rx) = mpsc::channel();
                if settings_check_updates {
                    let ctx = cc.egui_ctx.clone();
                    tokio::spawn(async move {
                        if let Some(r) = update::check().await {
                            let _ = tx.send(r);
                            ctx.request_repaint();
                        }
                    });
                }
                rx
            },
        };
        app.restart_ptt(&cc.egui_ctx);
        if let Some(url) = open {
            app.open_invite(&url);
        }
        app
    }

    /// (Re)starts the global push-to-talk listener for the current bind.
    fn restart_ptt(&mut self, ctx: &egui::Context) {
        self.ptt = None;
        if self.settings.audio.mode == InputMode::PushToTalk {
            let ctx = ctx.clone();
            let wake = move || ctx.request_repaint();
            self.ptt = Some(Ptt::start(self.voice.clone(), self.settings.ptt, wake));
        }
    }

    fn net_idx(&self, origin: &str) -> Option<usize> {
        self.view.net(origin)
    }

    fn send(&self, network: &str, cmd: NetCommand) {
        self.core.send(Command::Net {
            network: network.to_owned(),
            cmd,
        });
    }

    /// Queued for `poll`, which reads and uploads it.
    fn upload_file(&self, network: &str, target: Option<String>, path: std::path::PathBuf) {
        let _ = self.files.0.send((network.to_owned(), target, path));
    }

    /// Attaches a file to the buffer's composer, to go up when the message
    /// is sent; an avatar (no target) goes to chadd right away.
    fn upload(
        &mut self,
        ctx: &egui::Context,
        network: &str,
        target: Option<String>,
        name: String,
        bytes: Vec<u8>,
    ) {
        let Some(max) = self
            .net_idx(network)
            .and_then(|i| self.view.nets[i].info.uploads)
        else {
            return self.toast("This network doesn't take uploads.".into());
        };
        if bytes.len() as u64 > max {
            return self.toast(format!(
                "{name} is too big. The limit here is {} MB.",
                max >> 20
            ));
        }
        let bytes = Blob(bytes.into());
        if let Some(t) = &target {
            self.attached += 1;
            let preview = format!("attach:{}", self.attached);
            let Some(ni) = self.net_idx(network) else {
                return;
            };
            let n = &mut self.view.nets[ni];
            let Some(bi) = n.buffer(&key(t)) else {
                return;
            };
            if media::media_kind(&name) == Some(media::Kind::Image) {
                n.ui.media.local(preview.clone(), bytes.clone(), ctx);
            }
            n.buffers[bi].ui.attached.push(Attachment {
                name,
                bytes,
                key: preview,
            });
            return;
        }
        self.send(
            network,
            NetCommand::Upload {
                target,
                name,
                bytes,
            },
        );
    }

    /// The network and buffer on screen.
    fn on_screen(&self) -> Option<(String, String)> {
        let n = &self.view.nets[self.current.as_deref().and_then(|c| self.net_idx(c))?];
        let b = &n.buffers[n.ui.selected.as_deref().and_then(|k| n.buffer(k))?];
        Some((n.info.network.clone(), b.info.target.clone()))
    }

    /// Alt+Up/Down: the previous or next buffer, as listed.
    fn step_buffer(&mut self, step: isize) {
        let Some(ni) = self.current.as_deref().and_then(|c| self.net_idx(c)) else {
            return;
        };
        let n = &mut self.view.nets[ni];
        let len = n.buffers.len() as isize;
        if len == 0 {
            return;
        }
        let at =
            n.ui.selected
                .as_deref()
                .and_then(|k| n.buffer(k))
                .map_or(-1, |i| i as isize);
        let next = (at + step).rem_euclid(len) as usize;
        let k = n.buffers[next].key.clone();
        select(n, &k);
        self.replace_pane = true;
    }

    /// Keeps `panes` in step with the selected buffer: a newly selected one
    /// opens a pane (or takes over the focused one), and gone ones close.
    fn tile(&mut self, ctx: &egui::Context) {
        let before = (self.panes.len(), self.pane);
        let alive = |nets: &[Network], (net, k): &(String, String)| {
            nets.iter()
                .any(|n| n.info.network == *net && n.buffer(k).is_some())
        };
        let cur = self
            .current
            .as_deref()
            .and_then(|c| self.net_idx(c))
            .and_then(|i| {
                let n = &self.view.nets[i];
                let k = n.ui.selected.clone().filter(|k| n.buffer(k).is_some())?;
                Some((n.info.network.clone(), k))
            });
        let replace = std::mem::take(&mut self.replace_pane);
        if !self.settings.panes {
            self.panes = cur.into_iter().collect();
            self.pane = 0;
            return;
        }
        let focus_gone = self
            .panes
            .get(self.pane)
            .is_some_and(|p| !alive(&self.view.nets, p));
        self.panes.retain(|p| alive(&self.view.nets, p));
        self.pane = self.pane.min(self.panes.len().saturating_sub(1));
        if focus_gone && !self.panes.is_empty() {
            // Parted or closed: focus a neighbour rather than open a new pane.
            self.focus_pane(self.pane);
        } else if let Some(cur) = cur {
            if let Some(i) = self.panes.iter().position(|p| *p == cur) {
                self.pane = i;
            } else if replace && !self.panes.is_empty() {
                self.panes[self.pane] = cur;
            } else {
                self.panes.push(cur);
                self.pane = self.panes.len() - 1;
            }
        }
        if (self.panes.len(), self.pane) != before {
            ctx.request_repaint();
        }
    }

    fn focus_pane(&mut self, i: usize) {
        let Some((net, k)) = self.panes.get(i).cloned() else {
            return;
        };
        self.pane = i;
        if let Some(ni) = self.net_idx(&net) {
            self.view.nets[ni].ui.selected = Some(k);
            self.view.nets[ni].ui.focus = true;
            self.current = Some(net);
        }
    }

    fn close_pane(&mut self, i: usize) {
        self.panes.remove(i);
        if i == self.pane {
            self.focus_pane(i.min(self.panes.len() - 1));
        } else if i < self.pane {
            self.pane -= 1;
        }
    }

    /// The share's cell in the pane grid, when it's tiled there. The chat
    /// panes fill the other cells in order.
    fn screen_cell(&self) -> Option<usize> {
        (self.settings.stage == Layout::Tile && self.voice.watching().is_some())
            .then(|| self.stage.cell.min(self.panes.len()))
    }

    /// A pane's title dropped on another pane: they trade places, and focus
    /// stays with its buffer. The tiled share counts as a pane here.
    fn swap_cells(&mut self, a: usize, b: usize) {
        let focused = self.panes.get(self.pane).cloned();
        let screen = self.screen_cell();
        let mut cells: Vec<_> = std::mem::take(&mut self.panes)
            .into_iter()
            .map(Some)
            .collect();
        if let Some(s) = screen {
            cells.insert(s, None);
        }
        if a < cells.len() && b < cells.len() {
            cells.swap(a, b);
        }
        if let Some(s) = cells.iter().position(Option::is_none) {
            self.stage.cell = s;
        }
        self.panes = cells.into_iter().flatten().collect();
        self.pane = focused
            .and_then(|f| self.panes.iter().position(|p| *p == f))
            .unwrap_or(0);
    }

    /// Ctrl+arrows: the pane next to the focused one in that direction,
    /// passing over the tiled share.
    fn step_pane(&mut self, k: Key) {
        let screen = self.screen_cell();
        let n = self.panes.len() + usize::from(screen.is_some());
        let cols = pane_cols(n);
        let rows = n.div_ceil(cols);
        let mut c = match screen {
            Some(s) if self.pane >= s => self.pane + 1,
            _ => self.pane,
        };
        loop {
            c = match k {
                Key::ArrowLeft if c % cols > 0 => c - 1,
                Key::ArrowRight if c % cols + 1 < cols && c + 1 < n => c + 1,
                Key::ArrowUp if c >= cols => c - cols,
                // The last row can be short.
                Key::ArrowDown if c / cols + 1 < rows => (c + cols).min(n - 1),
                _ => return,
            };
            if Some(c) != screen {
                break;
            }
        }
        self.focus_pane(match screen {
            Some(s) if c > s => c - 1,
            _ => c,
        });
    }

    /// The open panes in a grid, rows filled left to right; a short last row
    /// stretches. The focused pane is outlined. Dragging a pane's title onto
    /// another swaps them. A tiled share takes a cell like a chat pane.
    fn tiles(&mut self, ui: &mut egui::Ui) {
        let screen = self.screen_cell();
        let chats = self.panes.len();
        let n = chats + usize::from(screen.is_some());
        let cols = pane_cols(n);
        let area = ui.available_rect_before_wrap();
        let h = area.height() / n.div_ceil(cols) as f32;
        let (mut close, mut focus, mut swap) = (None, None, None);
        let dragged = egui::DragAndDrop::payload::<PaneDrag>(ui.ctx()).map(|p| p.0);
        let panes = self.panes.clone();
        for i in 0..n {
            let (row, col) = (i / cols, i % cols);
            let w = area.width() / cols.min(n - row * cols) as f32;
            let rect = egui::Rect::from_min_size(
                area.min + vec2(col as f32 * w, row as f32 * h),
                vec2(w, h),
            );
            let p = match screen {
                Some(s) if i == s => None,
                Some(s) if i > s => Some(i - 1),
                _ => Some(i),
            };
            if let Some(p) = p {
                let (net, k) = &panes[p];
                let Some(ni) = self.net_idx(net) else {
                    continue;
                };
                let Some(bi) = self.view.nets[ni].buffer(k) else {
                    continue;
                };
                let mut pane = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink(2.0))
                        .id_salt(("pane", net, k)),
                );
                pane.set_clip_rect(rect);
                let active = p == self.pane;
                if !active
                    && ui.rect_contains_pointer(rect)
                    && ui.input(|i| i.pointer.any_pressed())
                {
                    focus = Some(p);
                }
                // A narrow pane drops the member list rather than crushing the chat.
                let fit = (self.view.nets[ni].buffers[bi].info.kind == BufferKind::Channel)
                    .then(|| self.members_width(&pane, ni, bi))
                    .filter(|fit| rect.width() - fit >= MIN_CHAT_W);
                if let Some(fit) = fit {
                    egui::Panel::right(egui::Id::unique(("members", net, k)))
                        .default_size(200.0)
                        .size_range(fit..=360.0)
                        .show(&mut pane, |ui| self.members(ui, ni, bi));
                }
                if self.chat(&mut pane, ni, bi, active, (n > 1).then_some(i), chats > 1) {
                    close = Some(p);
                }
                if active && chats > 1 {
                    let stroke = egui::Stroke::new(2.0, ui.visuals().selection.bg_fill);
                    ui.painter().rect_stroke(
                        rect.shrink(1.0),
                        4.0,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            } else {
                let mut pane = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink(2.0))
                        .id_salt("screen-tile"),
                );
                pane.set_clip_rect(rect);
                let ctx = ui.ctx().clone();
                self.stage_ui(&mut pane, &ctx, (n > 1).then_some(i));
            }
            if let Some(from) = dragged.filter(|&d| d != i && d < n)
                && ui.rect_contains_pointer(rect)
            {
                let fill = ui.visuals().selection.bg_fill.gamma_multiply(0.25);
                ui.painter().rect_filled(rect.shrink(2.0), 4.0, fill);
                if ui.input(|i| i.pointer.any_released()) {
                    swap = Some((from, i));
                }
            }
        }
        if let Some((a, b)) = swap {
            egui::DragAndDrop::clear_payload(ui.ctx());
            self.swap_cells(a, b);
        } else if let Some(i) = close {
            self.close_pane(i);
        } else if let Some(i) = focus {
            self.focus_pane(i);
        }
    }

    fn toast(&mut self, text: String) {
        // The same message again just restarts its timer instead of stacking.
        self.toasts.retain(|(t, _)| *t != text);
        self.toasts.push((text, Instant::now()));
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    /// A pasted or forwarded invite: show the network if we're in it already,
    /// otherwise open "Join a network" with it filled in.
    fn open_invite(&mut self, url: &str) {
        match invite::parse(url) {
            Ok(inv) => {
                if let Some(i) = self.net_idx(&inv.origin) {
                    self.current = Some(inv.origin.clone());
                    if let Some(ch) = inv.channel {
                        self.send(
                            &inv.origin,
                            NetCommand::JoinChannel {
                                channel: ch.clone(),
                            },
                        );
                        select(&mut self.view.nets[i], &key(&ch));
                    }
                    return;
                }
                self.join = Some(JoinDialog {
                    invite: url.trim().to_owned(),
                    ..Default::default()
                });
            }
            Err(e) => self.toast(e),
        }
    }

    fn handle(&mut self, ev: Event, ctx: &egui::Context) {
        // A removed buffer's key, and whether an icon changed, before the view forgets.
        let removed = match &ev {
            Event::BufferRemoved { network, target } => Some((network.clone(), key(target))),
            _ => None,
        };
        let icon_changed = match &ev {
            Event::Network(info) => self
                .net_idx(&info.network)
                .is_some_and(|i| self.view.nets[i].info.icon != info.icon),
            _ => false,
        };
        let unread_was = match &ev {
            Event::Buffer { network, buffer } => self
                .view
                .buf(network, &buffer.target)
                .map(|b| b.info.unread),
            _ => None,
        };
        let applied = self.view.apply(&ev);
        match ev {
            Event::Network(info) => {
                let origin = info.network.clone();
                let i = self.net_idx(&origin).unwrap_or_default();
                if applied.reconnected {
                    let n = &mut self.view.nets[i];
                    for b in &mut n.buffers {
                        b.ui.members_asked = false;
                    }
                    n.ui.media.retry_failed();
                }
                if applied.new && self.away_sent {
                    let message = Some(AWAY.to_owned());
                    self.send(&origin, NetCommand::SetAway { message });
                }
                if icon_changed {
                    self.view.nets[i].ui.icon = Icon::None;
                }
                // Signed up, or waiting on the email's code: the dialog's done.
                if let Some(s) = &self.signup
                    && s.busy
                    && s.network == origin
                    && (!info.guest || matches!(info.state, ConnState::NeedsVerification { .. }))
                {
                    self.signup = None;
                }
                let waiting = matches!(
                    info.state,
                    ConnState::Connected | ConnState::NeedsVerification { .. }
                );
                if waiting
                    && let Some(j) = &self.join
                    && let Some(inv) = j.busy.as_deref().and_then(|b| invite::parse(b).ok())
                    && inv.origin == origin
                {
                    self.join = None;
                    self.current = Some(origin.clone());
                    // Open the invite's channel, not whichever autojoin lands first.
                    if let Some(ch) = inv.channel {
                        self.view.nets[i].ui.selected = Some(key(&ch));
                    }
                }
                if self.current.is_none() {
                    self.current = Some(origin);
                }
            }
            Event::NetworkRemoved { network } => {
                if self.current.as_deref() == Some(&network) {
                    self.current = self.view.nets.first().map(|n| n.info.network.clone());
                }
            }
            Event::JoinFailed { invite, error } => match &mut self.join {
                Some(j) if j.busy.as_deref() == Some(&invite) => {
                    j.busy = None;
                    j.error = Some(error);
                }
                _ => self.toast(error),
            },
            Event::SignUpFailed { network, error } => match &mut self.signup {
                Some(s) if s.network == network => {
                    s.busy = false;
                    s.error = Some(error);
                }
                _ => self.toast(error),
            },
            Event::CapsChanged {
                network,
                added,
                removed,
            } => {
                tracing::info!("{network}: caps +{added:?} -{removed:?}");
            }
            Event::Buffer { network, buffer } => {
                let Some(ni) = self.net_idx(&network) else {
                    return;
                };
                let n = &mut self.view.nets[ni];
                let k = key(&buffer.target);
                if applied.new {
                    if n.ui.selected.is_none() || n.ui.selected.as_deref() == Some(&k) {
                        select(n, &k);
                    }
                } else if let Some(bi) = n.buffer(&k)
                    && unread_was != Some(buffer.unread)
                {
                    // Unread changed since we said it was read: say so again.
                    n.buffers[bi].ui.mark_sent = false;
                }
            }
            Event::BufferRemoved { .. } => {
                let Some((network, k)) = removed else { return };
                let Some(ni) = self.net_idx(&network) else {
                    return;
                };
                let n = &mut self.view.nets[ni];
                if n.ui.selected.as_deref() == Some(&k) {
                    n.ui.selected = None;
                    if let Some(first) = n.buffers.first().map(|b| b.key.clone()) {
                        select(n, &first);
                    }
                }
            }
            Event::BufferRenamed { network, from, to } => {
                let Some(ni) = self.net_idx(&network) else {
                    return;
                };
                let n = &mut self.view.nets[ni];
                let (old, new) = (key(&from), key(&to));
                if n.buffer(&new).is_none() {
                    return;
                }
                if n.ui.selected.as_deref() == Some(&old) {
                    n.ui.selected = Some(new.clone());
                }
                for p in &mut self.panes {
                    if p.0 == network && p.1 == old {
                        p.1 = new.clone();
                    }
                }
            }
            Event::Messages {
                network,
                target,
                messages,
            } => {
                // On screen counts as seen, even when focus is elsewhere.
                let seen = ctx.input(|i| {
                    let v = i.viewport();
                    i.focused || (v.minimized != Some(true) && v.occluded != Some(true))
                });
                let k = key(&target);
                let viewing = seen && self.panes.iter().any(|p| p.0 == network && p.1 == k);
                let Some(ni) = self.net_idx(&network) else {
                    return;
                };
                let n = &mut self.view.nets[ni];
                let Some(bi) = n.buffer(&k) else { return };
                let b = &mut n.buffers[bi];
                b.ui.older_asked = None;
                b.ui.log.apply(&applied.edits, &messages);
                for &i in &applied.fresh {
                    let m = &messages[i];
                    if !viewing && should_notify(self.settings.notify, m) {
                        let who = chatlog::shown(&n.profiles, &m.nick);
                        let summary = match b.info.kind {
                            BufferKind::Dm => who.to_owned(),
                            BufferKind::Channel => format!("{who} in {}", b.info.target),
                        };
                        platform::notify(summary, chatlog::snippet(&format::strip(&m.text), 200));
                    }
                }
                if let Some(id) = &b.ui.jump
                    && let Some(i) = b.index_of(id)
                {
                    b.ui.log.scroll_to_index(i, 60.0);
                    b.ui.jump = None;
                }
            }
            Event::Error { network, error } => {
                let name = self
                    .net_idx(&network)
                    .map_or(network, |i| self.view.nets[i].info.name.clone());
                self.toast(format!("{name}: {error}"));
            }
            Event::Uploaded {
                network,
                target,
                result,
            } => {
                match (&target, result) {
                    (Some(t), Ok(url)) => self.send(
                        &network,
                        NetCommand::Send {
                            target: t.clone(),
                            text: url.to_string(),
                            reply_to: None,
                        },
                    ),
                    (None, Ok(url)) => self.send(
                        &network,
                        NetCommand::SetAvatar {
                            url: url.to_string(),
                        },
                    ),
                    (_, Err(e)) => self.toast(format!("Upload failed: {e}")),
                }
                if let Some(b) = target.and_then(|t| self.view.buf_mut(&network, &t))
                    && !b.ui.uploads.is_empty()
                {
                    b.ui.uploads.remove(0);
                }
            }
            Event::Verification {
                network,
                nick,
                state,
            } => {
                // A request from someone else waits behind the one on screen.
                let busy = self.verify.as_ref().is_some_and(|(n, k, s)| {
                    (*n != network || key(k) != key(&nick))
                        && !matches!(s, VerifyState::Done | VerifyState::Failed(_))
                });
                if !busy {
                    self.verify = Some((network, nick, state));
                } else if state == VerifyState::Incoming {
                    self.send(
                        &network,
                        NetCommand::Verification {
                            nick,
                            step: VerifyStep::Cancel,
                        },
                    );
                }
            }
            Event::Preview {
                network,
                url,
                preview,
            } => {
                if let Some(i) = self.net_idx(&network) {
                    self.view.nets[i].ui.media.preview(url, preview);
                }
            }
            Event::Media {
                network,
                url,
                result,
            } => {
                if let Some(i) = self.net_idx(&network) {
                    self.view.nets[i].ui.media.bytes(url, result, ctx);
                }
            }
            Event::Sounds { result: Err(e), .. } => tracing::info!("soundboard: {e}"),
            Event::Sound {
                network,
                file,
                result,
            } => match result {
                Ok(bytes) => {
                    let (tx, ctx) = (self.decoded.0.clone(), ctx.clone());
                    tokio::task::spawn_blocking(move || {
                        // A hostile file that panics the decoder only loses the sound (D14).
                        let clip = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            chad_media::load_sound(bytes.0, &file)
                        }))
                        .unwrap_or_else(|_| Err("It's damaged.".into()));
                        let _ = tx.send((network, file, clip));
                        ctx.request_repaint();
                    });
                }
                Err(e) => self.toast(e),
            },
            Event::Gifs {
                network,
                query,
                pos,
                result,
            } => {
                let here = self.on_screen().is_some_and(|(n, _)| n == network);
                if let Some(p) = &mut self.picker
                    && here
                {
                    p.gifs.page(&query, pos.as_deref(), result);
                }
            }
            // Held in the view, nothing more to do.
            Event::HistoryStart { .. }
            | Event::Members { .. }
            | Event::Typing { .. }
            | Event::Voice { .. }
            | Event::VoiceMembers { .. }
            | Event::Profile { .. }
            | Event::Devices { .. }
            | Event::Sounds { .. } => {}
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.events.try_recv() {
            self.handle(ev, ctx);
        }
        while let Ok(msg) = self.os.try_recv() {
            match msg {
                Os::Open(url) => self.open_invite(&url),
                Os::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Os::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        // No global shortcut: push-to-talk follows the key while we're focused.
        if let Some(p) = self.ptt.as_ref().filter(|p| p.focused_only()) {
            let bind = p.bind;
            self.voice
                .set_ptt(ctx.input(|i| i.focused && bind.down(i)) && !self.binding_ptt);
        }
        for n in &mut self.view.nets {
            n.ui.media.poll(ctx);
            n.ui.media.viewer(ctx);
            for cmd in n.ui.media.wanted.drain(..) {
                self.core.send(Command::Net {
                    network: n.info.network.clone(),
                    cmd,
                });
            }
        }
        while let Ok((network, file, clip)) = self.decoded.1.try_recv() {
            match clip {
                Ok(clip) => {
                    self.clips.insert((network, file), clip.clone());
                    if let Err(e) = self.voice.play_sound(clip) {
                        self.toast(e.into());
                    }
                }
                Err(e) => self.toast(format!("Couldn't play {file}: {e}")),
            }
        }
        while let Ok((network, target, path)) = self.files.1.try_recv() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            let max = self
                .net_idx(&network)
                .and_then(|i| self.view.nets[i].info.uploads)
                .unwrap_or(0);
            match read_file(&path, max) {
                Ok(bytes) => self.upload(ctx, &network, target, name, bytes),
                Err(e) => self.toast(format!("{name}: {e}")),
            }
        }
        if let Ok((version, url)) = self.update.try_recv() {
            self.new_version = Some((version, url));
        }
        let input = ctx.input(|i| {
            i.raw.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::PointerMoved(_)
                        | egui::Event::MouseWheel { .. }
                )
            })
        });
        let idle = self.idle.idle_for(input);
        let after = Duration::from_secs(60 * u64::from(self.settings.away_after));
        let away = self.settings.away || (self.settings.away_after > 0 && idle >= after);
        if away != self.away_sent {
            self.away_sent = away;
            for n in &self.view.nets {
                let message = away.then(|| AWAY.to_owned());
                self.send(&n.info.network, NetCommand::SetAway { message });
            }
        }
        if !idle.is_zero() && !away {
            ctx.request_repaint_after(Duration::from_secs(30));
        }
        while let Ok((origin, img)) = self.icons.1.try_recv() {
            if let Some(i) = self.net_idx(&origin) {
                self.view.nets[i].ui.icon = match img {
                    Some(img) => Icon::Ready(ctx.load_texture(
                        format!("icon:{origin}"),
                        img,
                        Default::default(),
                    )),
                    None => Icon::Failed,
                };
            }
        }
        for n in &mut self.view.nets {
            if let (Icon::None, Some(url)) = (&n.ui.icon, &n.info.icon) {
                n.ui.icon = Icon::Loading;
                fetch_icon(
                    n.info.network.clone(),
                    url.clone(),
                    self.icons.0.clone(),
                    ctx,
                );
            }
        }
        let now = (
            self.view.nets.iter().any(Network::unread),
            self.view.nets.iter().map(Network::mentions).sum(),
        );
        if now != self.last_badge {
            self.last_badge = now;
            self.tray.set(now.0, now.1);
            let title = match now.1 {
                0 => "IrChad".into(),
                n => format!("({}) IrChad", badge(n)),
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    fn rail(&mut self, ui: &mut egui::Ui) {
        // Settings pinned to the bottom, networks from the top.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if ui
                .add_sized(
                    [48.0, 48.0],
                    egui::Button::new(RichText::new(icons::GEAR).size(22.0)),
                )
                .tip("Settings")
                .clicked()
            {
                self.show_settings = !self.show_settings;
            }
            ui.separator();
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                self.networks(ui);
            });
        });
    }

    fn networks(&mut self, ui: &mut egui::Ui) {
        let mut pick = None;
        for n in &self.view.nets {
            let selected = self.current.as_deref() == Some(&n.info.network);
            let (rect, resp) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::click());
            let p = ui.painter();
            let round = if selected || resp.hovered() { 14 } else { 24 };
            match &n.ui.icon {
                Icon::Ready(tex) => {
                    egui::Image::new(tex)
                        .corner_radius(CornerRadius::same(round))
                        .paint_at(ui, rect);
                }
                _ => {
                    let fill = if selected {
                        Color32::from_rgb(88, 101, 242)
                    } else {
                        ui.visuals().widgets.inactive.bg_fill
                    };
                    p.rect_filled(rect, round, fill);
                    let initial: String = n.info.name.chars().take(1).collect();
                    p.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        initial.to_uppercase(),
                        FontId::proportional(20.0),
                        ui.visuals().strong_text_color(),
                    );
                }
            }
            if selected || n.unread() {
                let h = if selected { 36.0 } else { 8.0 };
                // In the gutter left of the icon, clear of it.
                let pill = egui::Rect::from_min_max(
                    egui::pos2(rect.left() - 9.0, rect.center().y - h / 2.0),
                    egui::pos2(rect.left() - 5.0, rect.center().y + h / 2.0),
                );
                p.rect_filled(pill, 2.0, ui.visuals().strong_text_color());
            }
            let m = n.mentions();
            if m > 0 {
                mention_badge(ui, rect.right_bottom() - vec2(6.0, 6.0), m);
            }
            if !matches!(n.info.state, ConnState::Connected) {
                p.circle_filled(rect.right_top() + vec2(-4.0, 4.0), 5.0, Color32::GRAY);
            }
            if resp.tip(&n.info.name).clicked() {
                pick = Some(n.info.network.clone());
            }
            ui.add_space(8.0);
        }
        if pick.is_some() {
            self.current = pick;
        }
        if !self.view.nets.is_empty() {
            ui.separator();
        }
        let add = ui
            .add_sized(
                [48.0, 48.0],
                egui::Button::new(RichText::new("+").size(24.0)),
            )
            .tip("Join a network");
        if add.clicked() {
            self.join = Some(JoinDialog::default());
        }
    }

    fn channels(&mut self, ui: &mut egui::Ui, ni: usize) {
        let network = self.view.nets[ni].info.network.clone();
        if let Some((net, state)) = self.view.call.clone() {
            let name = self
                .net_idx(&net)
                .map_or(net.clone(), |i| self.view.nets[i].info.name.clone());
            let action = egui::Panel::bottom("voice-panel")
                .show(ui, |ui| {
                    voice::panel(
                        ui,
                        &self.voice,
                        &state,
                        &name,
                        self.ptt.as_ref(),
                        self.net_idx(&net)
                            .map(|i| self.view.nets[i].sounds.as_slice()),
                        &mut self.settings.audio,
                    )
                })
                .inner;
            match action {
                Some(voice::PanelAction::Leave) => self.send(&net, NetCommand::LeaveVoice),
                Some(voice::PanelAction::Share) => {
                    self.share_dialog = Some(ShareDialog::new(&self.voice, ui.ctx()));
                }
                Some(voice::PanelAction::StopShare) => self.voice.stop_share(),
                Some(voice::PanelAction::Soundboard) => self.send(&net, NetCommand::Sounds),
                Some(voice::PanelAction::Sound(file)) => {
                    match self.clips.get(&(net.clone(), file.clone())) {
                        Some(clip) => {
                            if let Err(e) = self.voice.play_sound(clip.clone()) {
                                self.toast(e.into());
                            }
                        }
                        None => self.send(&net, NetCommand::Sound { file }),
                    }
                }
                Some(voice::PanelAction::Preview) => {
                    let me = self.voice.call().and_then(|c| {
                        c.participants
                            .into_iter()
                            .find(|p| p.local)
                            .map(|p| p.identity)
                    });
                    self.voice.watch(me);
                }
                None => {}
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(icons::DOTS_THREE, |ui| {
                    if self.view.nets[ni].info.guest && ui.button("Create an account…").clicked()
                    {
                        self.signup = Some(SignUpDialog {
                            network: network.clone(),
                            account: self.view.nets[ni].info.nick.clone(),
                            password: String::new(),
                            email: String::new(),
                            busy: false,
                            error: None,
                        });
                    }
                    if ui.button("Leave network…").clicked() {
                        self.confirm_remove = Some(network.clone());
                    }
                })
                .response
                .tip("Network options");
                // Whatever's left of the menu, so long names truncate instead of overlapping it.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let name = RichText::new(&self.view.nets[ni].info.name).heading();
                    ui.add(egui::Label::new(name).truncate());
                });
            });
        });
        if let Some((text, bad)) = state_text(&self.view.nets[ni].info.state) {
            let color = if bad {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().warn_fg_color
            };
            ui.label(RichText::new(text).small().color(color));
            if bad {
                let mut go = false;
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.signin_password)
                            .password(true)
                            .hint_text("Password")
                            .desired_width(120.0),
                    );
                    go = ui.button("Sign in").clicked()
                        || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                });
                if go && !self.signin_password.is_empty() {
                    let password = std::mem::take(&mut self.signin_password);
                    self.send(&network, NetCommand::SignIn { password });
                }
            }
        }
        let own = self.view.nets[ni].info.nick.clone();
        let shown = chatlog::shown(&self.view.nets[ni].profiles, &own).to_owned();
        let (dot, color) = if self.away_sent {
            (icons::MOON, voice::AWAY)
        } else {
            ("●", voice::ONLINE)
        };
        let status = egui::Atoms::new((
            RichText::new(dot).color(color).small(),
            RichText::new(if shown == own {
                own.clone()
            } else {
                format!("{shown} ({own})")
            })
            .small(),
        ));
        ui.menu_button(status, |ui| {
            ui.radio_value(&mut self.settings.away, false, "Online");
            ui.radio_value(&mut self.settings.away, true, "Away");
            ui.separator();
            if ui
                .button(format!("{} Edit profile…", icons::USER_CIRCLE))
                .clicked()
            {
                let name = self.view.nets[ni]
                    .profiles
                    .get(&key(&own))
                    .and_then(|p| p.display_name.clone())
                    .unwrap_or_default();
                self.profile = Some(ProfileDialog {
                    network: network.clone(),
                    name,
                });
                self.send(&network, NetCommand::Devices { nick: own.clone() });
                ui.close();
            }
        })
        .response
        .tip("Your status and profile");
        ui.separator();

        let mut pick = None;
        let mut part = None;
        let mut close = None;
        let mut join_voice = None;
        let mut watch = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (kind, title) in [
                    (BufferKind::Channel, "CHANNELS"),
                    (BufferKind::Dm, "DIRECT MESSAGES"),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(title).small().weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let hint = match kind {
                                BufferKind::Channel => "Join a channel",
                                BufferKind::Dm => "Message someone",
                            };
                            let add = egui::Button::new(RichText::new("+").size(18.0))
                                .min_size(vec2(24.0, 22.0));
                            if ui.add(add).tip(hint).clicked() {
                                self.adding = Some((kind, String::new()));
                            }
                        });
                    });
                    let n = &self.view.nets[ni];
                    for b in n.buffers.iter().filter(|b| b.info.kind == kind) {
                        let selected = n.ui.selected.as_deref() == Some(&b.key);
                        let mut text = RichText::new(&b.info.target);
                        if b.info.unread > 0 || selected {
                            text = text.strong().color(ui.visuals().strong_text_color());
                        } else {
                            text = text.color(ui.visuals().weak_text_color());
                        }
                        let mut atoms = egui::Atoms::new(text);
                        let mut tip = None;
                        if kind == BufferKind::Dm {
                            let (dot, color, t) = match (b.info.online, b.info.away) {
                                (Some(true), false) => ("●", voice::ONLINE, "Online"),
                                (Some(true), true) => ("◐", voice::AWAY, "Away"),
                                _ => ("○", ui.visuals().weak_text_color(), "Offline"),
                            };
                            atoms.push_left(RichText::new(dot).color(color));
                            tip = Some(t);
                        }
                        let mut resp = ui
                            .with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                                ui.add(
                                    egui::Button::selectable(selected, atoms)
                                        .min_size(vec2(0.0, 24.0)),
                                )
                            })
                            .inner;
                        if let Some(tip) = tip {
                            resp = resp.on_hover_text(tip);
                        }
                        if b.info.mentions > 0 {
                            mention_badge(
                                ui,
                                resp.rect.right_center() - vec2(14.0, 0.0),
                                b.info.mentions,
                            );
                        }
                        let in_call = matches!(
                            &self.view.call,
                            Some((net, VoiceState::Connecting { channel } | VoiceState::Connected { channel }))
                                if *net == network && key(channel) == b.key
                        );
                        if b.info.voice && !in_call {
                            let right = if b.info.mentions > 0 { 38.0 } else { 16.0 };
                            let at = egui::Rect::from_center_size(
                                resp.rect.right_center() - vec2(right, 0.0),
                                vec2(26.0, 22.0),
                            );
                            let join =
                                egui::Button::new(RichText::new(icons::SPEAKER_HIGH).size(16.0));
                            if ui.put(at, join).tip("Join voice").clicked() {
                                join_voice = Some(b.info.target.clone());
                            }
                        }
                        resp.context_menu(|ui| match kind {
                            BufferKind::Channel => {
                                if b.info.voice && !in_call && ui.button("Join voice").clicked() {
                                    join_voice = Some(b.info.target.clone());
                                }
                                if ui.button("Leave channel").clicked() {
                                    part = Some(b.info.target.clone());
                                }
                            }
                            BufferKind::Dm => {
                                if ui.button("Close conversation").clicked() {
                                    close = Some(b.info.target.clone());
                                }
                            }
                        });
                        if resp.clicked() {
                            pick = Some(b.key.clone());
                        }
                        let live = in_call
                            && matches!(&self.view.call, Some((_, VoiceState::Connected { .. })));
                        if (live || !b.voice.is_empty())
                            && let Some(who) = voice::members(
                                ui,
                                &self.voice,
                                live,
                                &b.voice,
                                &mut self.settings.volumes,
                                &mut self.settings.screen_volumes,
                            )
                        {
                            watch = Some(who);
                        }
                    }
                    if let Some((k, text)) = &mut self.adding
                        && *k == kind
                    {
                        let hint = match kind {
                            BufferKind::Channel => "#channel",
                            BufferKind::Dm => "username",
                        };
                        let r = ui.add(
                            egui::TextEdit::singleline(text)
                                .hint_text(hint)
                                .id(adding_id()),
                        );
                        // Focus it while open: the "+" click itself would steal focus
                        // on the first frame, leaving a field that never closes.
                        if !r.has_focus() && !r.lost_focus() {
                            r.request_focus();
                        }
                        // Escape or clicking away cancels; Enter submits.
                        if r.lost_focus() {
                            let enter = ui.input(|i| i.key_pressed(Key::Enter));
                            let name = text.trim().to_owned();
                            let kind = *k;
                            self.adding = None;
                            if enter && !name.is_empty() {
                                let cmd = match kind {
                                    BufferKind::Channel => {
                                        let ch = if name.starts_with('#') {
                                            name
                                        } else {
                                            format!("#{name}")
                                        };
                                        pick = Some(key(&ch));
                                        NetCommand::JoinChannel { channel: ch }
                                    }
                                    BufferKind::Dm => {
                                        pick = Some(key(&name));
                                        NetCommand::OpenDm { nick: name }
                                    }
                                };
                                self.send(&network, cmd);
                            }
                        }
                    }
                    ui.add_space(8.0);
                }
            });
        if let Some(k) = pick {
            select(&mut self.view.nets[ni], &k);
            // Back to the chat; a share that isn't in its place keeps playing.
            if self.settings.stage == Layout::Fill {
                self.voice.watch(None);
            }
        }
        if let Some(who) = watch {
            self.voice.watch(Some(who));
        }
        if let Some(channel) = part {
            self.send(&network, NetCommand::Part { channel });
        }
        if let Some(nick) = close {
            self.send(&network, NetCommand::CloseDm { nick });
        }
        if let Some(channel) = join_voice {
            self.send(&network, NetCommand::JoinVoice { channel });
        }
    }

    /// The member list's narrowest width: its widest row, plus the panel's
    /// margins, the scroll bar and a little air. Capped, since display names
    /// can be long; past that they truncate.
    fn members_width(&self, ui: &egui::Ui, ni: usize, bi: usize) -> f32 {
        let profiles = &self.view.nets[ni].profiles;
        let Some(members) = &self.view.nets[ni].buffers[bi].members else {
            return 100.0;
        };
        let body = egui::TextStyle::Body.resolve(ui.style());
        let small = egui::TextStyle::Small.resolve(ui.style());
        let width = |text: String, font: &FontId| {
            ui.painter()
                .layout_no_wrap(text, font.clone(), Color32::WHITE)
                .size()
                .x
        };
        let away = members.iter().filter(|m| m.away).count();
        let widest = members
            .iter()
            .map(|m| {
                let name = chatlog::shown(profiles, &m.nick);
                width(format!("{}{name}", m.prefix), &body)
            })
            .chain([
                width(format!("ONLINE — {}", members.len() - away), &small),
                width(format!("AWAY — {away}"), &small),
            ])
            .fold(0.0, f32::max);
        let s = &ui.style().spacing.scroll;
        let bar = s.bar_width + s.bar_inner_margin + s.bar_outer_margin;
        (widest + 16.0 + bar + 12.0).clamp(100.0, 360.0)
    }

    fn members(&mut self, ui: &mut egui::Ui, ni: usize, bi: usize) {
        let dark = ui.visuals().dark_mode;
        let network = self.view.nets[ni].info.network.clone();
        let own = self.view.nets[ni].info.nick.clone();
        let profiles = &self.view.nets[ni].profiles;
        let Some(members) = &self.view.nets[ni].buffers[bi].members else {
            ui.add_space(8.0);
            ui.spinner();
            return;
        };
        let mut dm = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (away, title) in [(false, "ONLINE"), (true, "AWAY")] {
                    let list: Vec<&Member> = members.iter().filter(|m| m.away == away).collect();
                    if list.is_empty() {
                        continue;
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("{title} — {}", list.len()))
                            .small()
                            .weak(),
                    );
                    for m in list {
                        let name = chatlog::shown(profiles, &m.nick);
                        let mut t = RichText::new(format!("{}{name}", m.prefix))
                            .color(chatlog::nick_color(&m.nick, dark));
                        if away {
                            t = t.weak();
                        }
                        let r = ui
                            .add(egui::Label::new(t).sense(Sense::click()).truncate())
                            .on_hover_text(match &m.account {
                                Some(a) => format!("{} · account {a}", m.nick),
                                None => format!("{} · not logged in", m.nick),
                            });
                        if r.clicked() && m.nick != own {
                            dm = Some(m.nick.clone());
                        }
                    }
                }
            });
        if let Some(nick) = dm {
            self.send(&network, NetCommand::OpenDm { nick: nick.clone() });
            select(&mut self.view.nets[ni], &key(&nick));
        }
    }

    /// One buffer's pane. `active`: the focused pane, which takes the
    /// composer focus. `tile`: its cell when it's one of several, which makes
    /// its title a drag handle. `closable`: it isn't the only chat pane.
    /// Returns whether its close button was clicked.
    fn chat(
        &mut self,
        ui: &mut egui::Ui,
        ni: usize,
        bi: usize,
        active: bool,
        tile: Option<usize>,
        closable: bool,
    ) -> bool {
        let network = self.view.nets[ni].info.network.clone();
        let own = self.view.nets[ni].info.nick.clone();
        let connected = self.view.nets[ni].info.state == ConnState::Connected;
        let focused = ui.input(|i| i.focused);
        let core = self.core.clone();
        let send = |cmd| {
            core.send(Command::Net {
                network: network.clone(),
                cmd,
            })
        };
        let uploads = self.view.nets[ni].info.uploads.is_some();
        let gifs = self.view.nets[ni].info.gifs.is_some();
        let auto_media = self.settings.load_media;
        // Keys that aren't typing: the one being bound, and push-to-talk's.
        let ptt_key = self.ptt.as_ref().map(|p| p.bind);
        let binding_ptt = self.binding_ptt;
        let n = &mut self.view.nets[ni];
        let (b, profiles, media, devices) =
            (&mut n.buffers[bi], &n.profiles, &mut n.ui.media, &n.devices);
        let focus = active && std::mem::take(&mut n.ui.focus);
        let target = b.info.target.clone();
        let mut close = false;
        // Set inside the panels, acted on once `b` is free again.
        let mut attach = false;
        // Where the picker opens, and whether on the GIF tab (searching
        // `gif_query`, from `/gif`).
        let mut emoji_at = None;
        let (mut gif_query, mut no_gifs) = (String::new(), false);
        let mut pick_reaction = None;
        let mut pasted = Vec::new();

        if connected && b.info.kind == BufferKind::Channel && !b.ui.members_asked {
            b.ui.members_asked = true;
            send(NetCommand::Members {
                channel: target.clone(),
            });
        }

        egui::Panel::top("topic").show(ui, |ui| {
            if closable {
                // A child, so it doesn't push the topic row down.
                let at = egui::Rect::from_min_size(
                    ui.max_rect().right_top() - vec2(24.0, 0.0),
                    vec2(24.0, 24.0),
                );
                close = ui
                    .new_child(egui::UiBuilder::new().max_rect(at))
                    .button(icons::X)
                    .tip("Close pane")
                    .clicked();
            }
            ui.horizontal(|ui| {
                if closable {
                    ui.set_max_width(ui.available_width() - 28.0);
                }
                match tile {
                    Some(i) => {
                        let id = egui::Id::unique(("pane-drag", &network, &b.key));
                        ui.dnd_drag_source(id, PaneDrag(i), |ui| ui.heading(&target))
                            .response
                            .on_hover_text("Drag onto another pane to swap them");
                    }
                    None => {
                        ui.heading(&target);
                    }
                }
                if b.info.kind == BufferKind::Dm {
                    let status = match (b.info.online, b.info.away) {
                        (Some(true), false) => "online",
                        (Some(true), true) => "away",
                        (Some(false), _) => "offline",
                        (None, _) => "",
                    };
                    ui.label(RichText::new(status).weak());
                    let lock = match b.info.encryption {
                        Encryption::Unknown => None,
                        Encryption::Off => Some((icons::LOCK_OPEN, "Not encrypted", OFF_TIP)),
                        Encryption::On => Some((icons::LOCK, "Encrypted", ON_TIP)),
                        Encryption::Verified => {
                            Some((icons::SHIELD_CHECK, "Verified", VERIFIED_TIP))
                        }
                        Encryption::Changed => Some((icons::WARNING, "Keys changed", CHANGED_TIP)),
                    };
                    if let Some((icon, text, tip)) = lock {
                        ui.separator();
                        ui.label(RichText::new(format!("{icon} {text}")).weak())
                            .on_hover_ui(|ui| {
                                ui.label(tip);
                                for d in devices.get(&b.key).into_iter().flatten() {
                                    device_row(ui, d);
                                }
                            });
                    }
                    if connected
                        && matches!(b.info.encryption, Encryption::On | Encryption::Changed)
                        && ui.small_button("Verify…").clicked()
                    {
                        send(NetCommand::Verification {
                            nick: target.clone(),
                            step: VerifyStep::Start,
                        });
                    }
                }
                let topic = format::strip(&b.info.topic);
                if !topic.is_empty() {
                    ui.separator();
                    ui.add(egui::Label::new(RichText::new(&topic).weak()).truncate())
                        .on_hover_text(&topic);
                }
            });
            if b.info.encryption == Encryption::Changed {
                ui.horizontal_wrapped(|ui| {
                    let warn = format!(
                        "{} {target}'s encryption keys changed. That's normal after they \
                         reinstall, but it can also mean someone is listening in. Verify \
                         to be sure.",
                        icons::WARNING
                    );
                    ui.label(RichText::new(warn).color(ui.visuals().warn_fg_color));
                    if ui.button("Accept the new keys").clicked() {
                        send(NetCommand::AcceptKeys {
                            nick: target.clone(),
                        });
                    }
                });
            }
        });

        egui::Panel::bottom("composer").show(ui, |ui| {
            ui.add_space(4.0);
            let typing: Vec<_> = b.typing().collect();
            let typing_text = match typing.as_slice() {
                [] => String::new(),
                [a] => format!("{a} is typing…"),
                [a, c] => format!("{a} and {c} are typing…"),
                _ => "Several people are typing…".into(),
            };
            if !typing.is_empty() {
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            }
            ui.label(RichText::new(typing_text).small().weak());
            for name in &b.ui.uploads {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(format!("Uploading {name}…")).small());
                });
            }
            let mut unattach = None;
            ui.horizontal_wrapped(|ui| {
                for (i, a) in b.ui.attached.iter().enumerate() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.vertical(|ui| {
                            match media::media_kind(&a.name) {
                                Some(media::Kind::Image) => {
                                    media.local_preview(ui, &a.key, vec2(160.0, 120.0))
                                }
                                Some(media::Kind::Video) => {
                                    ui.label(RichText::new(icons::PLAY).size(32.0));
                                }
                                _ => {
                                    ui.label(RichText::new(icons::FILE).size(32.0));
                                }
                            }
                            ui.horizontal(|ui| {
                                ui.add(egui::Label::new(RichText::new(&a.name).small()).truncate());
                                if ui
                                    .small_button(icons::X)
                                    .tip(&format!("Remove {}", a.name))
                                    .clicked()
                                {
                                    unattach = Some(i);
                                }
                            });
                        });
                    });
                }
            });
            if let Some(i) = unattach {
                media.forget(&b.ui.attached.remove(i).key);
            }
            let id = egui::Id::unique(("composer", &network, &b.key));
            // The `@` list was open last frame, so Tab, Enter and Escape are its.
            let open_id = id.with("completing");
            let completing = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
            // Typing or pasting while nothing has focus goes to the composer,
            // this frame, so the first character isn't lost.
            if active
                && focused
                && !binding_ptt
                && ui.memory(|m| m.focused().is_none())
                && ui.input(|i| {
                    !ptt_key.is_some_and(|k| k.down(i))
                        && i.events.iter().any(|e| match e {
                            egui::Event::Text(t) => !t.is_empty(),
                            egui::Event::Paste(_) | egui::Event::PasteImage(_) => true,
                            _ => false,
                        })
                })
            {
                ui.memory_mut(|m| m.request_focus(id));
            }
            if let Some((_, nick)) = &b.ui.reply {
                let mut cancel = false;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Replying to {nick}")).small());
                    cancel = ui.small_button(icons::X).tip("Cancel reply").clicked();
                });
                if cancel || (!completing && ui.input(|i| i.key_pressed(Key::Escape))) {
                    b.ui.reply = None;
                }
            }
            // Tab completes nicks rather than moving focus (Shift+Tab goes back).
            let mut tab = None;
            if ui.memory(|m| m.has_focus(id)) {
                ui.input_mut(|i| {
                    // Plain Tab is left to the open list, which accepts with it.
                    if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                        tab = (!completing).then_some(true);
                    } else if !completing && i.consume_key(Modifiers::NONE, Key::Tab) {
                        tab = Some(false);
                    }
                });
            }
            // Pasted images and files become uploads instead of text.
            if ui.memory(|m| m.has_focus(id)) {
                ui.input_mut(|i| {
                    i.events.retain(|e| match e {
                        egui::Event::PasteImage(img) => {
                            pasted.push(Pasted::Image(img.clone()));
                            false
                        }
                        egui::Event::Paste(text) => match pasted_files(text) {
                            Some(paths) => {
                                pasted.extend(paths.into_iter().map(Pasted::File));
                                false
                            }
                            None => true,
                        },
                        _ => true,
                    })
                });
            }
            // Square buttons as tall as the one-line field beside them.
            let side = vec2(1.0, 1.0) * (ui.text_style_height(&egui::TextStyle::Body) + 24.0);
            let out = ui
                .horizontal(|ui| {
                    if uploads
                        && ui
                            .add(
                                egui::Button::new(RichText::new(icons::PAPERCLIP).size(18.0))
                                    .min_size(side),
                            )
                            .tip("Upload a file")
                            .clicked()
                    {
                        attach = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let e = ui
                            .add(
                                egui::Button::new(RichText::new(icons::SMILEY).size(18.0))
                                    .min_size(side),
                            )
                            .tip("Emoji");
                        if e.clicked() {
                            emoji_at = Some((e.rect.left_top(), false));
                        }
                        if gifs {
                            let g = ui
                                .add(
                                    egui::Button::new(RichText::new("GIF").strong().size(13.0))
                                        .min_size(side),
                                )
                                .tip("GIFs");
                            if g.clicked() {
                                emoji_at = Some((g.rect.left_top(), true));
                            }
                        }
                        let edit = egui::TextEdit::multiline(&mut b.ui.draft)
                            .hint_text(format!("Message {target}"))
                            .desired_rows(1)
                            .margin(egui::Margin::symmetric(14, 12))
                            .desired_width(f32::INFINITY)
                            .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                            .lock_focus(true);
                        let (members, msgs, info) = (&b.members, &b.msgs, &b.info);
                        egui::CompletionPopup::new(id).show(ui, edit, |q| {
                            if !q.word.starts_with('@') {
                                return Vec::new();
                            }
                            let nicks = completable(members, msgs, info, &own);
                            let list = complete::suggest(&nicks, q.word).unwrap_or_default();
                            list.into_iter()
                                .map(|n| {
                                    egui::Suggestion::new(complete::insert(n, q.is_at_start()))
                                        .content(n.to_owned())
                                })
                                .collect()
                        })
                    })
                    .inner
                })
                .inner;
            ui.data_mut(|d| d.insert_temp(open_id, out.is_open));
            let resp = out.text_edit.response.response;
            if focus {
                resp.request_focus();
            }
            // Screen readers get the hint as the field's name.
            ui.ctx()
                .accesskit_node_builder(resp.id, |n| n.set_label(format!("Message {target}")));
            let mut changed = resp.changed() || out.accepted.is_some();
            if (changed || tab.is_some())
                && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id)
            {
                let end = b.ui.draft.chars().count();
                let mut cursor = state
                    .cursor
                    .char_range()
                    .map_or(end, |r| r.primary.index.into());
                let mut moved = false;
                if let Some(back) = tab {
                    let nicks = completable(&b.members, &b.msgs, &b.info, &own);
                    if let Some(c) =
                        complete::tab(&mut b.ui.draft, cursor, &mut b.ui.tab, &nicks, back)
                    {
                        (cursor, moved, changed) = (c, true, true);
                    }
                }
                if changed && emoji::replace_shortcodes(&mut b.ui.draft, &mut cursor) {
                    moved = true;
                }
                if moved {
                    let c = egui::text::CCursor::new(cursor);
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(c)));
                    state.store(ui.ctx(), id);
                }
            }
            if changed {
                let state = if b.ui.draft.trim().is_empty() {
                    Typing::Done
                } else {
                    Typing::Active
                };
                send(NetCommand::Typing {
                    target: target.clone(),
                    state,
                });
            }
            let enter = ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
            if resp.has_focus() && enter {
                let text = b.ui.draft.trim_end().to_owned();
                if let Some(q) = gifs::command(&text) {
                    // `/gif [search]` opens the GIF tab instead of sending.
                    if gifs {
                        emoji_at = Some((resp.rect.right_top(), true));
                        gif_query = q.to_owned();
                        b.ui.draft.clear();
                    } else {
                        no_gifs = true;
                    }
                } else if !text.trim().is_empty() || !b.ui.attached.is_empty() {
                    if !text.trim().is_empty() {
                        let reply_to = b.ui.reply.take().map(|(id, _)| id);
                        send(NetCommand::Send {
                            target: target.clone(),
                            text,
                            reply_to,
                        });
                    }
                    // Each attachment follows as its own message once it's up.
                    for a in b.ui.attached.drain(..) {
                        media.forget(&a.key);
                        b.ui.uploads.push(a.name.clone());
                        send(NetCommand::Upload {
                            target: Some(target.clone()),
                            name: a.name,
                            bytes: a.bytes,
                        });
                    }
                    b.ui.draft.clear();
                    b.ui.log.scroll_to_bottom();
                }
            }
            ui.add_space(12.0);
        });

        let dm = egui::CentralPanel::default().show(ui, |ui| {
            if b.msgs.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new(format!("No messages in {target} yet. Say hi!")).weak());
                });
            }
            let mut extras = Extras {
                profiles,
                media,
                network: &network,
                auto_media,
                dm: b.info.kind == BufferKind::Dm,
            };
            let channel = b.info.kind == BufferKind::Channel;
            let out = b.ui.log.ui(ui, &b.msgs, &own, channel, &mut extras);
            if !out.at_bottom {
                let pos = ui.max_rect().right_bottom() - vec2(16.0, 16.0);
                egui::Area::new(egui::Id::unique(("jump-latest", &network, &b.key)))
                    .fixed_pos(pos)
                    .pivot(Align2::RIGHT_BOTTOM)
                    .show(ui.ctx(), |ui| {
                        if ui
                            .button(format!("Jump to latest {}", icons::ARROW_DOWN))
                            .clicked()
                        {
                            b.ui.log.scroll_to_bottom();
                        }
                    });
            }
            if out.near_top
                && connected
                && !b.history_start
                && !b.msgs.is_empty()
                && b.ui
                    .older_asked
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
            {
                b.ui.older_asked = Some(Instant::now());
                send(NetCommand::LoadOlder {
                    target: target.clone(),
                });
            }
            if out.at_bottom && focused && b.info.unread > 0 && !b.ui.mark_sent {
                b.ui.mark_sent = true;
                send(NetCommand::MarkRead {
                    target: target.clone(),
                });
            }
            let mut dm = None;
            for a in out.actions {
                match a {
                    Action::Reply(i) => {
                        if let Some(id) = b.msgs[i].msgid.clone() {
                            b.ui.reply = Some((id, b.msgs[i].nick.clone()));
                        }
                    }
                    Action::React { msgid, emoji } => {
                        send(NetCommand::React {
                            target: target.clone(),
                            msgid,
                            emoji,
                        });
                    }
                    Action::Unreact { msgid, emoji } => {
                        send(NetCommand::Unreact {
                            target: target.clone(),
                            msgid,
                            emoji,
                        });
                    }
                    Action::Delete { msgid } => {
                        send(NetCommand::Redact {
                            target: target.clone(),
                            msgid,
                        });
                    }
                    Action::Jump { msgid } => match b.index_of(&msgid) {
                        Some(i) => b.ui.log.scroll_to_index(i, 60.0),
                        None => {
                            b.ui.jump = Some(msgid.clone());
                            send(NetCommand::LoadAround {
                                target: target.clone(),
                                msgid,
                            });
                        }
                    },
                    Action::OpenDm(nick) => dm = Some(nick),
                    Action::PickReaction { msgid, at } => pick_reaction = Some((msgid, at)),
                }
            }
            dm
        });
        if let Some(nick) = dm.inner {
            send(NetCommand::OpenDm { nick: nick.clone() });
            select(&mut self.view.nets[ni], &key(&nick));
        }
        if let Some((msgid, at)) = pick_reaction {
            self.picker = Some(Picker {
                react: Some((network.clone(), target.clone(), msgid)),
                at,
                query: String::new(),
                shown: false,
                gif_tab: false,
                gifs: Default::default(),
            });
        }
        if let Some((at, gif_tab)) = emoji_at {
            self.picker = Some(Picker {
                react: None,
                at,
                query: String::new(),
                shown: false,
                gif_tab,
                gifs: gifs::Gifs::searching(gif_query),
            });
        }
        if no_gifs {
            self.toast("This network doesn't have GIFs.".into());
        }
        if attach {
            let (tx, ctx) = (self.files.0.clone(), ui.ctx().clone());
            let (network, target) = (network.clone(), target.clone());
            tokio::spawn(async move {
                if let Some(f) = rfd::AsyncFileDialog::new().pick_file().await {
                    let _ = tx.send((network, Some(target), f.path().to_owned()));
                    ctx.request_repaint();
                }
            });
        }
        for p in pasted {
            match p {
                Pasted::File(path) => self.upload_file(&network, Some(target.clone()), path),
                Pasted::Image(img) => match png(&img) {
                    Some(bytes) => {
                        let ctx = ui.ctx().clone();
                        self.upload(
                            &ctx,
                            &network,
                            Some(target.clone()),
                            "pasted.png".into(),
                            bytes,
                        )
                    }
                    None => self.toast("Couldn't paste that image.".into()),
                },
            }
        }
        close
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            ui.heading("Welcome to IrChad");
            ui.label("Paste an invite link to get started.");
            ui.add_space(12.0);
            if ui
                .button(RichText::new("Join a network").size(18.0))
                .clicked()
            {
                self.join = Some(JoinDialog::default());
            }
        });
    }

    /// The watched screen share; `ctx` is the window it's in. `tile`: its
    /// cell, when it's one of several in the pane grid.
    fn stage_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, tile: Option<usize>) {
        let (layout, fullscreen) = (self.settings.stage, self.stage.fullscreen);
        match screen::stage(ui, &self.voice, layout, fullscreen, tile) {
            Some(StageAction::Close) => {
                if self.stage.fullscreen {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                }
                self.voice.watch(None);
            }
            Some(StageAction::Layout(l)) => self.settings.stage = l,
            Some(StageAction::Fullscreen(on)) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
                self.stage.fullscreen = on;
            }
            Some(StageAction::Volume(who, v)) => {
                self.settings.screen_volumes.insert(who, v);
            }
            None => {}
        }
    }

    /// The share in its own window. Closing the window stops watching.
    fn popout(&mut self, ctx: &egui::Context) {
        let id = egui::ViewportId::from_hash_of("screen-share");
        let title = format!("{}'s screen", self.voice.watching().unwrap_or_default());
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_app_id(platform::APP_ID)
            .with_inner_size([1280.0, 760.0]);
        ctx.show_viewport_immediate(id, builder, |ui, _| {
            let vctx = ui.ctx().clone();
            egui::CentralPanel::default().show(ui, |ui| self.stage_ui(ui, &vctx, None));
            if vctx.input(|i| i.viewport().close_requested()) {
                self.voice.watch(None);
            }
        });
    }

    /// The share in a small window over the chat; drag it anywhere, resize
    /// it from the edges.
    fn float(&mut self, ctx: &egui::Context) {
        let size = vec2(480.0, 300.0);
        egui::Window::new("Screen share")
            .id(egui::Id::unique("screen-float"))
            .title_bar(false)
            .resizable(true)
            .default_size(size)
            .min_size(vec2(240.0, 150.0))
            .pivot(Align2::RIGHT_BOTTOM)
            .default_pos(ctx.content_rect().right_bottom() - vec2(16.0, 16.0))
            .constrain(true)
            .show(ctx, |ui| self.stage_ui(ui, ctx, None));
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(d) = &mut self.share_dialog {
            let s = &mut self.settings;
            match d.show(ctx, &self.voice, &mut s.screen, &mut s.restore_token) {
                DialogOutcome::Open => {}
                DialogOutcome::Closed => self.share_dialog = None,
                DialogOutcome::Shared(token) => {
                    if token.is_some() {
                        s.restore_token = token;
                    }
                    self.share_dialog = None;
                }
            }
        }
        if let Some(j) = &mut self.join {
            let mut submit = false;
            let mut cancel = false;
            let busy = j.busy.is_some();
            let modal = egui::Modal::new(egui::Id::unique("join")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading("Join a network");
                ui.add_space(8.0);
                let l = ui.label("Invite link");
                ui.add_enabled(
                    !busy,
                    egui::TextEdit::singleline(&mut j.invite)
                        .hint_text("https://chat.example.com/invite")
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(l.id);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        ui.selectable_value(&mut j.mode, JoinMode::Login, "I have an account");
                        ui.selectable_value(&mut j.mode, JoinMode::Register, "Create an account");
                        ui.selectable_value(&mut j.mode, JoinMode::Guest, "Guest");
                    });
                });
                ui.add_space(4.0);
                egui::Grid::new("join-fields")
                    .num_columns(2)
                    .show(ui, |ui| {
                        let guest = j.mode == JoinMode::Guest;
                        let l = ui.label(if guest { "Nickname" } else { "Username" });
                        let field =
                            |text| egui::TextEdit::singleline(text).desired_width(f32::INFINITY);
                        ui.add_enabled(!busy, field(&mut j.account))
                            .labelled_by(l.id);
                        ui.end_row();
                        if !guest {
                            let l = ui.label("Password");
                            ui.add_enabled(!busy, field(&mut j.password).password(true))
                                .labelled_by(l.id);
                            ui.end_row();
                        }
                        if j.mode == JoinMode::Register {
                            let l = ui.label("Email");
                            ui.add_enabled(!busy, field(&mut j.email).hint_text("optional"))
                                .labelled_by(l.id);
                            ui.end_row();
                        }
                    });
                if j.mode == JoinMode::Guest {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Guests miss what's said while they're away, \
                             and anyone can take the nickname.",
                        )
                        .weak(),
                    );
                }
                if let Some(e) = &j.error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
                }
                ui.add_space(8.0);
                let ready = !j.invite.trim().is_empty()
                    && !j.account.trim().is_empty()
                    && (j.mode == JoinMode::Guest || !j.password.is_empty());
                let label = match j.mode {
                    JoinMode::Login => "Log in",
                    JoinMode::Register => "Sign up",
                    JoinMode::Guest => "Join",
                };
                let (go, no) = footer(ui, label, ready && !busy, busy);
                submit = go || (ready && !busy && ui.input(|i| i.key_pressed(Key::Enter)));
                cancel = no;
            });
            if submit {
                match invite::parse(&j.invite) {
                    Err(e) => j.error = Some(e),
                    Ok(_) => {
                        let (account, password) = (j.account.trim().to_owned(), j.password.clone());
                        let auth = match j.mode {
                            JoinMode::Register => {
                                let email =
                                    Some(j.email.trim().to_owned()).filter(|e| !e.is_empty());
                                Auth::Register {
                                    account,
                                    password,
                                    email,
                                }
                            }
                            JoinMode::Login => Auth::Login { account, password },
                            JoinMode::Guest => Auth::Guest { nick: account },
                        };
                        j.error = None;
                        j.busy = Some(j.invite.trim().to_owned());
                        self.core.send(Command::Join {
                            invite: j.invite.trim().to_owned(),
                            auth,
                        });
                    }
                }
            }
            // A join in flight can't be cancelled; it ends on its own.
            if (cancel || modal.should_close()) && !busy {
                self.join = None;
            }
        }

        if let Some(s) = &mut self.signup {
            let mut submit = false;
            let mut cancel = false;
            let name = self
                .view
                .nets
                .iter()
                .find(|n| n.info.network == s.network)
                .map_or(s.network.clone(), |n| n.info.name.clone());
            let modal = egui::Modal::new(egui::Id::unique("signup")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading(format!("Create an account on {name}"));
                ui.label(
                    RichText::new(
                        "Keep your name, catch up on what's said while you're away, \
                         and use voice and encrypted DMs.",
                    )
                    .weak(),
                );
                ui.add_space(8.0);
                egui::Grid::new("signup-fields")
                    .num_columns(2)
                    .show(ui, |ui| {
                        let field =
                            |text| egui::TextEdit::singleline(text).desired_width(f32::INFINITY);
                        let l = ui.label("Username");
                        ui.add_enabled(!s.busy, field(&mut s.account))
                            .labelled_by(l.id);
                        ui.end_row();
                        let l = ui.label("Password");
                        ui.add_enabled(!s.busy, field(&mut s.password).password(true))
                            .labelled_by(l.id);
                        ui.end_row();
                        let l = ui.label("Email");
                        ui.add_enabled(!s.busy, field(&mut s.email).hint_text("optional"))
                            .labelled_by(l.id);
                        ui.end_row();
                    });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("You'll reconnect for a moment; your channels come with you.")
                        .weak(),
                );
                if let Some(e) = &s.error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
                }
                ui.add_space(8.0);
                let ready = !s.account.trim().is_empty() && !s.password.is_empty() && !s.busy;
                let (go, no) = footer(ui, "Sign up", ready, s.busy);
                submit = go || (ready && ui.input(|i| i.key_pressed(Key::Enter)));
                cancel = no;
            });
            if submit {
                s.busy = true;
                s.error = None;
                self.verify_dismissed = None;
                self.core.send(Command::Net {
                    network: s.network.clone(),
                    cmd: NetCommand::Register {
                        account: s.account.trim().to_owned(),
                        password: s.password.clone(),
                        email: Some(s.email.trim().to_owned()).filter(|e| !e.is_empty()),
                    },
                });
            }
            // A sign-up in flight can't be cancelled; it ends on its own.
            if (cancel || modal.should_close()) && !s.busy {
                self.signup = None;
            }
        }

        if let Some(ni) = self.current.as_deref().and_then(|c| self.net_idx(c))
            && let ConnState::NeedsVerification { message } = &self.view.nets[ni].info.state
            && self.verify_dismissed.as_deref() != Some(&self.view.nets[ni].info.network)
        {
            let network = self.view.nets[ni].info.network.clone();
            let mut done = false;
            let modal = egui::Modal::new(egui::Id::unique("verify")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.heading("Check your email");
                ui.label(message);
                ui.add(egui::TextEdit::singleline(&mut self.verify_code).hint_text("Code"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Not now").clicked() {
                        done = true;
                    }
                    if ui.button("Verify").clicked() && !self.verify_code.trim().is_empty() {
                        let code = std::mem::take(&mut self.verify_code).trim().to_owned();
                        self.core.send(Command::Net {
                            network: network.clone(),
                            cmd: NetCommand::Verify { code },
                        });
                    }
                });
            });
            if done || modal.should_close() {
                self.verify_dismissed = Some(network);
            }
        }

        if let Some(network) = self.confirm_remove.clone() {
            let name = self
                .net_idx(&network)
                .map_or(network.clone(), |i| self.view.nets[i].info.name.clone());
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::unique("remove")).show(ctx, |ui| {
                ui.heading(format!("Leave {name}?"));
                ui.label("IrChad forgets this network and your saved password for it.");
                ui.label(
                    "This device's encryption keys go too, so it can't read encrypted DMs again.",
                );
                ui.label("Your account stays, so you can join again later.");
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let leave = egui::Button::new(RichText::new("Leave").color(Color32::WHITE))
                        .fill(Color32::from_rgb(218, 55, 60));
                    let leave = ui.add(leave).clicked();
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if leave {
                        self.core.send(Command::Remove {
                            network: network.clone(),
                        });
                        close = true;
                    }
                });
            });
            if close || modal.should_close() {
                self.confirm_remove = None;
            }
        }

        let mut open = self.show_settings;
        egui::Window::new("Settings")
            .open(&mut open)
            .pivot(Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                if self.settings.ui(ui) {
                    self.settings.apply(ctx);
                }
                let ptt = (self.settings.audio.mode, self.settings.ptt);
                if voice::settings(
                    ui,
                    &mut self.settings,
                    &self.voice,
                    self.ptt.as_ref(),
                    &mut self.binding_ptt,
                ) {
                    self.voice.set_settings(self.settings.audio.clone());
                    if (self.settings.audio.mode, self.settings.ptt) != ptt {
                        self.restart_ptt(ctx);
                    }
                }
                ui.separator();
                ui.weak(format!("IrChad {}", crate::update::version()));
            });
        if !open {
            self.binding_ptt = false;
            if self.voice.testing() {
                self.voice.set_testing(false);
            }
        }
        self.show_settings = open;

        self.picker_ui(ctx);
        self.switcher_ui(ctx);
        self.profile_ui(ctx);
        self.verify_ui(ctx);
        if let Some((version, url)) = self.new_version.clone() {
            let mut close = false;
            egui::Area::new(egui::Id::unique("update"))
                .anchor(Align2::RIGHT_TOP, vec2(-16.0, 16.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("IrChad {version} is out."));
                            ui.hyperlink_to("Get it", url);
                            close = ui.small_button(icons::X).tip("Dismiss").clicked();
                        });
                    });
                });
            if close {
                self.new_version = None;
            }
        }

        self.toasts.retain(|(_, t)| t.elapsed() < TOAST_TTL);
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(Duration::from_secs(1));
            egui::Area::new(egui::Id::unique("toasts"))
                .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -72.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let mut dismiss = HashSet::new();
                    for (i, (text, _)) in self.toasts.iter().enumerate() {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_max_width(320.0);
                            ui.horizontal(|ui| {
                                ui.label(text);
                                if ui.small_button(icons::X).tip("Dismiss").clicked() {
                                    dismiss.insert(i);
                                }
                            });
                        });
                    }
                    let mut i = 0;
                    self.toasts.retain(|_| {
                        i += 1;
                        !dismiss.contains(&(i - 1))
                    });
                });
        }
    }
}

impl App {
    fn picker_ui(&mut self, ctx: &egui::Context) {
        let here = self.on_screen();
        // GIFs only from the composer, on a network that searches them.
        let gif_net = here
            .as_ref()
            .and_then(|(n, _)| self.net_idx(n))
            .and_then(|i| Some((i, self.view.nets[i].info.gifs.clone()?)));
        let Some(p) = &mut self.picker else { return };
        let gif_net = gif_net.filter(|_| p.react.is_none());
        let nets = &mut self.view.nets;
        let (mut picked, mut gif) = (None, None);
        let area = egui::Area::new(egui::Id::unique("emoji-picker"))
            .fixed_pos(p.at)
            .pivot(Align2::RIGHT_BOTTOM)
            .constrain(true)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(emoji::PICKER_WIDTH);
                    match &gif_net {
                        Some((ni, provider)) => {
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut p.gif_tab, false, "Emoji");
                                ui.selectable_value(&mut p.gif_tab, true, "GIFs");
                            });
                            if p.gif_tab {
                                gif = p.gifs.ui(ui, &mut nets[*ni].ui.media, provider);
                            } else {
                                picked = emoji::picker(ui, &mut p.query);
                            }
                        }
                        None => picked = emoji::picker(ui, &mut p.query),
                    }
                });
            });
        let close = (p.shown && area.response.clicked_elsewhere())
            || ctx.input(|i| i.key_pressed(Key::Escape));
        p.shown = true;
        let react = p.react.clone();
        match (gif, here) {
            (Some(gifs::Action::Fetch { query, pos }), Some((network, _))) => {
                self.send(&network, NetCommand::Gifs { query, pos });
            }
            // Posted as a plain link: every client can open it, and IrChad
            // shows it inline.
            (Some(gifs::Action::Pick(g)), Some((network, target))) => {
                if let Some(b) = self.view.buf_mut(&network, &target) {
                    let reply_to = b.ui.reply.take().map(|(id, _)| id);
                    b.ui.log.scroll_to_bottom();
                    self.send(
                        &network,
                        NetCommand::Send {
                            target,
                            text: g.url.into(),
                            reply_to,
                        },
                    );
                }
                self.picker = None;
                self.refocus_composer();
                return;
            }
            _ => {}
        }
        if let Some(e) = picked {
            match react {
                Some((network, target, msgid)) => {
                    self.send(
                        &network,
                        NetCommand::React {
                            target,
                            msgid,
                            emoji: e,
                        },
                    );
                }
                None => {
                    if let Some((network, target)) = self.on_screen()
                        && let Some(b) = self.view.buf_mut(&network, &target)
                    {
                        // At the caret, which then sits after the emoji.
                        let id = egui::Id::unique(("composer", &network, &b.key));
                        let state = egui::TextEdit::load_state(ctx, id);
                        let end = b.ui.draft.chars().count();
                        let at = state
                            .as_ref()
                            .and_then(|s| s.cursor.char_range())
                            .map_or(end, |r| usize::from(r.primary.index).min(end));
                        let byte =
                            b.ui.draft
                                .char_indices()
                                .nth(at)
                                .map_or(b.ui.draft.len(), |(i, _)| i);
                        b.ui.draft.insert_str(byte, &e);
                        if let Some(mut s) = state {
                            let c = egui::text::CCursor::new(at + e.chars().count());
                            s.cursor
                                .set_char_range(Some(egui::text::CCursorRange::one(c)));
                            s.store(ctx, id);
                        }
                    }
                }
            }
            self.picker = None;
            self.refocus_composer();
        } else if close {
            self.picker = None;
        }
    }

    /// Back to typing in the buffer on screen, e.g. after the picker.
    fn refocus_composer(&mut self) {
        if let Some(ni) = self.current.as_deref().and_then(|c| self.net_idx(c)) {
            self.view.nets[ni].ui.focus = true;
        }
    }

    fn switcher_ui(&mut self, ctx: &egui::Context) {
        let Some(sw) = &mut self.switcher else { return };
        let q = sw.query.trim().to_lowercase();
        // (network, key, label), names starting with the query first.
        let mut hits: Vec<(bool, String, String, String)> = self
            .view
            .nets
            .iter()
            .flat_map(|n| {
                n.buffers.iter().filter_map(|b| {
                    let name = b.info.target.to_lowercase();
                    let pos = name.trim_start_matches('#').find(&q)?;
                    let label = format!("{}  ·  {}", b.info.target, n.info.name);
                    Some((pos != 0, n.info.network.clone(), b.key.clone(), label))
                })
            })
            .collect();
        hits.sort_by(|a, b| (a.0, &a.3).cmp(&(b.0, &b.3)));
        hits.truncate(10);
        let mut go = None;
        let (up, down, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.key_pressed(Key::Enter),
            )
        });
        if !hits.is_empty() {
            if up {
                sw.sel = sw.sel.saturating_sub(1);
            }
            if down {
                sw.sel = (sw.sel + 1).min(hits.len() - 1);
            }
            sw.sel = sw.sel.min(hits.len() - 1);
        }
        let modal = egui::Modal::new(egui::Id::unique("switcher")).show(ctx, |ui| {
            ui.set_width(360.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut sw.query)
                    .hint_text("Where to? Channel or person")
                    .desired_width(f32::INFINITY),
            );
            ui.ctx()
                .accesskit_node_builder(r.id, |n| n.set_label("Where to? Channel or person"));
            if !r.has_focus() && !r.lost_focus() {
                r.request_focus();
            }
            if r.changed() {
                sw.sel = 0;
            }
            for (i, (_, net, k, label)) in hits.iter().enumerate() {
                let b = ui.add(egui::Button::selectable(i == sw.sel, label.as_str()));
                if b.clicked() || (enter && i == sw.sel) {
                    go = Some((net.clone(), k.clone()));
                }
            }
            if hits.is_empty() {
                ui.label(RichText::new("Nothing by that name.").weak());
            }
        });
        if let Some((net, k)) = go {
            if let Some(i) = self.net_idx(&net) {
                self.current = Some(net);
                select(&mut self.view.nets[i], &k);
            }
            self.switcher = None;
        } else if modal.should_close() {
            self.switcher = None;
        }
    }

    fn profile_ui(&mut self, ctx: &egui::Context) {
        let Some(network) = self.profile.as_ref().map(|d| d.network.clone()) else {
            return;
        };
        let Some(ni) = self.net_idx(&network) else {
            self.profile = None;
            return;
        };
        let Some(d) = &mut self.profile else { return };
        let n = &mut self.view.nets[ni];
        let own = n.info.nick.clone();
        let avatar = n.profiles.get(&key(&own)).and_then(|p| p.avatar.clone());
        let (mut save, mut close, mut choose, mut remove) = (false, false, false, false);
        let modal = egui::Modal::new(egui::Id::unique("profile")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.heading(format!("Your profile on {}", n.info.name));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
                n.ui.media.avatar(ui, rect, &own, avatar.as_ref(), true);
                ui.vertical(|ui| {
                    if n.info.uploads.is_some() {
                        choose = ui.button("Choose a picture…").clicked();
                    }
                    if avatar.is_some() {
                        remove = ui.button("Remove picture").clicked();
                    }
                });
            });
            ui.add_space(8.0);
            let l = ui.label("Display name");
            ui.add(
                egui::TextEdit::singleline(&mut d.name)
                    .hint_text(&own)
                    .char_limit(64)
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(l.id);
            ui.label(
                RichText::new(format!("People still see that you're {own}."))
                    .small()
                    .weak(),
            );
            if let Some(devices) = n.devices.get(&key(&own)).filter(|d| !d.is_empty()) {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("{} Devices with encrypted DMs", icons::LOCK)).strong(),
                );
                for d in devices {
                    device_row(ui, d);
                }
            }
            ui.add_space(8.0);
            (save, close) = footer(ui, "Save", true, false);
        });
        if save {
            let name = d.name.trim().to_owned();
            self.send(&network, NetCommand::SetDisplayName { name });
        }
        if remove {
            self.send(&network, NetCommand::SetAvatar { url: String::new() });
        }
        if choose {
            let (tx, ctx) = (self.files.0.clone(), ctx.clone());
            let network = network.clone();
            tokio::spawn(async move {
                let pick = rfd::AsyncFileDialog::new()
                    .add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"])
                    .pick_file()
                    .await;
                if let Some(f) = pick {
                    let _ = tx.send((network, None, f.path().to_owned()));
                    ctx.request_repaint();
                }
            });
        }
        if save || close || modal.should_close() {
            self.profile = None;
        }
    }
}

const OFF_TIP: &str = "They don't use IrChad here, so your messages aren't end-to-end encrypted.";
const ON_TIP: &str = "End-to-end encrypted: only your devices and theirs can read it. Verify to make sure nobody is in the middle.";
const VERIFIED_TIP: &str = "End-to-end encrypted, and you compared emoji with them.";
const CHANGED_TIP: &str = "One of their devices has a new key since you last talked.";

/// One encrypted-DM device: its name, whether it's verified, and its fingerprint.
fn device_row(ui: &mut egui::Ui, d: &DeviceInfo) {
    ui.horizontal(|ui| {
        let name = if d.this { "This device" } else { d.id.as_str() };
        ui.label(name);
        if d.verified {
            ui.label(icons::SHIELD_CHECK).on_hover_text("Verified");
        }
    });
    ui.label(RichText::new(&d.fingerprint).monospace().small().weak());
}

impl App {
    /// Comparing emoji with someone (SAS), one person at a time.
    fn verify_ui(&mut self, ctx: &egui::Context) {
        let Some((network, nick, state)) = self.verify.clone() else {
            return;
        };
        let (mut step, mut close) = (None, false);
        let modal = egui::Modal::new(egui::Id::unique("verify")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(format!("Verify {nick}"));
            ui.add_space(8.0);
            match &state {
                VerifyState::Incoming => {
                    ui.label(format!(
                        "{nick} wants to make sure your messages to each other are private, \
                         by comparing emoji."
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Not now").clicked() {
                            step = Some(VerifyStep::Cancel);
                        }
                        if ui.button("Start").clicked() {
                            step = Some(VerifyStep::Accept);
                        }
                    });
                }
                VerifyState::Waiting => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Waiting for {nick}…"));
                    });
                    if ui.button("Cancel").clicked() {
                        step = Some(VerifyStep::Cancel);
                    }
                }
                VerifyState::Compare(emoji) => {
                    ui.label(format!(
                        "Check that {nick} sees the same emoji, in the same order. Compare \
                         them in person or on a call, not by message."
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        for (e, name) in emoji {
                            ui.allocate_ui(vec2(48.0, 64.0), |ui| {
                                ui.vertical_centered(|ui| {
                                    ui.label(RichText::new(*e).size(28.0));
                                    ui.label(RichText::new(*name).small());
                                });
                            });
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("They don't match").clicked() {
                            step = Some(VerifyStep::Cancel);
                        }
                        if ui.button("They match").clicked() {
                            step = Some(VerifyStep::Match);
                        }
                    });
                }
                VerifyState::Done => {
                    ui.label(format!(
                        "{} {nick}'s device is verified. Nobody is listening in.",
                        icons::SHIELD_CHECK
                    ));
                    close = ui.button("Done").clicked();
                }
                VerifyState::Failed(why) => {
                    ui.label(why);
                    close = ui.button("Close").clicked();
                }
            }
        });
        let finished = matches!(state, VerifyState::Done | VerifyState::Failed(_));
        if !finished && step.is_none() && modal.should_close() {
            step = Some(VerifyStep::Cancel);
        }
        if let Some(step) = step {
            let nick = nick.clone();
            self.send(&network, NetCommand::Verification { nick, step });
        }
        if close || (finished && modal.should_close()) {
            self.verify = None;
        }
    }
}

fn adding_id() -> egui::Id {
    egui::Id::unique("add-buffer")
}

fn should_notify(rule: Notify, m: &ChatMessage) -> bool {
    let fresh = chad_core::time::now_ms() - m.time < NOTIFY_WINDOW_MS;
    let wanted = match rule {
        Notify::All => true,
        Notify::MentionsAndDms => m.mention,
        Notify::Never => false,
    };
    wanted && fresh && !m.own && m.status == SendStatus::Sent && !m.redacted
}

fn mention_badge(ui: &egui::Ui, center: egui::Pos2, n: u32) {
    let text = badge(n);
    let w = 16.0f32.max(8.0 + 7.0 * text.len() as f32);
    let rect = egui::Rect::from_center_size(center, vec2(w, 16.0));
    let p = ui.painter();
    p.rect_filled(rect, 8.0, Color32::from_rgb(237, 66, 69));
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(11.0),
        Color32::WHITE,
    );
}

/// Fetches and decodes a network icon off the UI thread. The server picks the
/// bytes (D14), so decoding is size-limited.
fn fetch_icon(origin: String, url: Url, tx: mpsc::Sender<IconMsg>, ctx: &egui::Context) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let img = match chad_core::fetch_icon(&url).await {
            Ok(bytes) => tokio::task::spawn_blocking(move || decode_icon(&bytes))
                .await
                .ok()
                .flatten(),
            Err(e) => {
                tracing::info!("icon {url}: {e}");
                None
            }
        };
        let _ = tx.send((origin, img));
        ctx.request_repaint();
    });
}

fn decode_icon(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut r = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 << 20);
    r.limits(limits);
    let img = r.decode().ok()?.thumbnail(96, 96).to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

enum Pasted {
    Image(std::sync::Arc<egui::ColorImage>),
    File(std::path::PathBuf),
}

/// Files copied in a file manager paste as `file://` URIs, one per line.
fn pasted_files(text: &str) -> Option<Vec<std::path::PathBuf>> {
    let paths: Vec<_> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| Url::parse(l.trim()).ok()?.to_file_path().ok())
        .collect::<Option<_>>()?;
    (!paths.is_empty() && paths.iter().all(|p| p.is_file())).then_some(paths)
}

/// A clipboard image as PNG.
fn png(img: &egui::ColorImage) -> Option<Vec<u8>> {
    let [w, h] = img.size;
    let raw: Vec<u8> = img
        .pixels
        .iter()
        .flat_map(|c| c.to_srgba_unmultiplied())
        .collect();
    let buf = image::RgbaImage::from_raw(u32::try_from(w).ok()?, u32::try_from(h).ok()?, raw)?;
    let mut out = Vec::new();
    buf.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// The file, if it isn't over `max` bytes (checked before reading it).
fn read_file(path: &std::path::Path, max: u64) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > max {
        return Err(format!("too big; the limit here is {} MB.", max >> 20));
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);

        let watching = self.voice.watching().is_some();
        let layout = self.settings.stage;
        if !watching && self.stage.fullscreen {
            if layout != Layout::PopOut {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            self.stage.fullscreen = false;
        }
        if watching && layout == Layout::PopOut {
            self.popout(&ctx);
        } else if watching && self.stage.fullscreen {
            egui::CentralPanel::default().show(ui, |ui| self.stage_ui(ui, &ctx, None));
            self.dialogs(&ctx);
            return;
        }

        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::K)))
        {
            self.switcher = Some(Switcher::default());
        }
        let step = ctx.input_mut(|i| {
            let mut alt = |k| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::ALT, k));
            if alt(Key::ArrowUp) {
                -1
            } else if alt(Key::ArrowDown) {
                1
            } else {
                0
            }
        });
        if step != 0 {
            self.step_buffer(step);
        }
        self.tile(&ctx);
        if self.panes.len() > 1 {
            for k in [
                Key::ArrowLeft,
                Key::ArrowRight,
                Key::ArrowUp,
                Key::ArrowDown,
            ] {
                if ctx.input_mut(|i| {
                    i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, k))
                }) {
                    self.step_pane(k);
                }
            }
        }
        // shortcut: winit 0.30 has no drag-and-drop on Wayland, so this only
        // fires on X11 and Windows; paste and the paperclip work everywhere.
        let (dropped, hovering) = ctx.input(|i| {
            let paths: Vec<_> = i
                .raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_owned())
                .collect();
            (paths, !i.raw.hovered_files.is_empty())
        });
        if let Some((network, target)) = self.on_screen() {
            for path in dropped {
                self.upload_file(&network, Some(target.clone()), path);
            }
            if hovering {
                egui::Area::new(egui::Id::unique("drop-hint"))
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .order(egui::Order::Foreground)
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style())
                            .inner_margin(24.0)
                            .show(ui, |ui| {
                                ui.heading(format!(
                                    "{} Drop to upload to {target}",
                                    icons::PAPERCLIP
                                ));
                            });
                    });
            }
        }

        egui::Panel::left("rail")
            .exact_size(68.0)
            .resizable(false)
            .show(ui, |ui| self.rail(ui));

        let ni = self.current.as_deref().and_then(|c| self.net_idx(c));
        if let Some(ni) = ni {
            egui::Panel::left("channels")
                .default_size(240.0)
                .size_range(160.0..=400.0)
                .show(ui, |ui| self.channels(ui, ni));
        }
        // Read again: the popped-out window may have just moved the share back.
        let layout = self.settings.stage;
        egui::CentralPanel::default().show(ui, |ui| match ni {
            _ if watching && layout == Layout::Fill => self.stage_ui(ui, &ctx, None),
            None => self.welcome(ui),
            Some(_) if self.panes.is_empty() && self.screen_cell().is_none() => {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("No channels yet. Join one with + on the left.").weak());
                });
            }
            Some(_) => self.tiles(ui),
        });
        if watching && layout == Layout::Float {
            self.float(&ctx);
        }

        self.dialogs(&ctx);
        // Picks made while drawing show next frame.
        self.tile(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, settings::KEY, &self.settings);
    }
}
