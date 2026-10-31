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
    Encryption, Event, Member, NetCommand, NetworkInfo, SendStatus, Typing, VerifyState,
    VerifyStep, VoiceState, invite,
};
use chad_media::{InputMode, Voice};
use egui::{
    Align2, Color32, CornerRadius, FontId, Key, KeyboardShortcut, Modifiers, RichText, Sense,
    TextureHandle, vec2,
};
use tokio::sync::mpsc::UnboundedReceiver;
use url::Url;

use crate::chatlog::{self, Action, ChatLog, Extras, Profile};
use crate::icons::{self, Tip};
use crate::media::Media;
use crate::platform::idle::Idle;
use crate::platform::ptt::Ptt;
use crate::platform::{self, Os, Tray};
use crate::screen::{self, DialogOutcome, Layout, ShareDialog, StageAction};
use crate::settings::{self, Notify, Settings};
use crate::voice;
use crate::{complete, emoji, format, update};

const TYPING_TTL: Duration = Duration::from_secs(6);
const TOAST_TTL: Duration = Duration::from_secs(8);
/// Only messages this fresh notify; gap-fill after a restart stays quiet.
const NOTIFY_WINDOW_MS: i64 = 60_000;
/// The AWAY reason other clients show.
const AWAY: &str = "Away";

/// A network's decoded icon, or `None` if it couldn't be had.
type IconMsg = (String, Option<egui::ColorImage>);
/// A file from the file dialog: (network, buffer or `None` for an avatar, path).
type Picked = (String, Option<String>, std::path::PathBuf);

pub struct App {
    core: Core,
    events: UnboundedReceiver<Event>,
    os: mpsc::Receiver<Os>,
    tray: Tray,
    icons: (mpsc::Sender<IconMsg>, mpsc::Receiver<IconMsg>),
    nets: Vec<Network>,
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
    adding: Option<(BufferKind, String)>,
    toasts: Vec<(String, Instant)>,
    last_badge: (bool, u32),
    voice: Voice,
    /// Our call: (network, state). One at a time across networks.
    call: Option<(String, VoiceState)>,
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
    idle: Idle,
    /// Whether the networks were last told we're away.
    away_sent: bool,
    update: mpsc::Receiver<(String, String)>,
    /// A newer release: (version, page).
    new_version: Option<(String, String)>,
}

/// The emoji picker, for the composer or a reaction.
struct Picker {
    react: Option<(String, String, String)>,
    at: egui::Pos2,
    query: String,
    /// Shown for a frame already: the click that opened it is no "click elsewhere".
    shown: bool,
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

struct Network {
    info: NetworkInfo,
    buffers: Vec<Buffer>,
    /// Key of the buffer on screen.
    selected: Option<String>,
    /// Put the cursor in the composer on the next frame.
    focus: bool,
    icon: Icon,
    /// By `key(nick)`.
    profiles: HashMap<String, Profile>,
    /// Encrypted-DM devices, by `key(nick)`.
    devices: HashMap<String, Vec<DeviceInfo>>,
    media: Media,
}

enum Icon {
    None,
    Loading,
    Ready(TextureHandle),
    Failed,
}

struct Buffer {
    key: String,
    info: BufferInfo,
    log: ChatLog,
    members: Option<Vec<Member>>,
    members_asked: bool,
    typing: Vec<(String, Instant)>,
    history_start: bool,
    older_asked: Option<Instant>,
    mark_sent: bool,
    draft: String,
    /// The Tab completion in progress in the draft.
    tab: Option<complete::Tab>,
    /// (msgid, nick) of the message being replied to.
    reply: Option<(String, String)>,
    /// A message to scroll to once `LoadAround` brings it in.
    jump: Option<String>,
    /// Who's in the channel's voice room (metadata).
    voice: Vec<String>,
    /// Names of files on their way up.
    uploads: Vec<String>,
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

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum JoinMode {
    #[default]
    Login,
    Register,
    Guest,
}

/// Buffer names compare under ergo's `ascii` casemapping.
fn key(target: &str) -> String {
    target.to_ascii_lowercase()
}

/// The nicks a buffer's composer completes: the channel's members, or the
/// other side of a DM.
fn completable(
    members: &Option<Vec<Member>>,
    log: &ChatLog,
    info: &BufferInfo,
    own: &str,
) -> Vec<String> {
    let recent = log.msgs.iter().rev().map(|m| m.nick.as_str());
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

impl Network {
    fn buffer(&self, k: &str) -> Option<usize> {
        self.buffers.iter().position(|b| b.key == k)
    }

    fn unread(&self) -> bool {
        self.buffers.iter().any(|b| b.info.unread > 0)
    }

    fn mentions(&self) -> u32 {
        self.buffers.iter().map(|b| b.info.mentions).sum()
    }

    /// Shows a buffer, with the "new messages" line above the first unread one.
    fn select(&mut self, k: &str) {
        self.selected = Some(k.to_owned());
        self.focus = true;
        let Some(i) = self.buffer(k) else { return };
        let b = &mut self.buffers[i];
        let mut left = b.info.unread;
        b.log.unread_from = None;
        if left > 0 {
            for (idx, m) in b.log.msgs.iter().enumerate().rev() {
                if !m.own {
                    left -= 1;
                    if left == 0 {
                        b.log.unread_from = m.msgid.clone();
                        b.log.scroll_to_index(idx, 60.0);
                        break;
                    }
                }
            }
        }
        if b.log.unread_from.is_none() {
            b.log.scroll_to_bottom();
        }
        b.mark_sent = false;
    }
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
        let settings_check_updates = settings.check_updates;
        let mut app = Self {
            core,
            events,
            os,
            tray,
            icons: mpsc::channel(),
            nets: Vec::new(),
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
            adding: None,
            toasts: Vec::new(),
            last_badge: (false, 0),
            voice,
            call: None,
            ptt: None,
            binding_ptt: false,
            share_dialog: None,
            stage: Stage::default(),
            picker: None,
            switcher: None,
            profile: None,
            verify: None,
            files: mpsc::channel(),
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
        self.nets.iter().position(|n| n.info.network == origin)
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

    /// Sends a file to chadd; the URL lands in the buffer's draft.
    fn upload(&mut self, network: &str, target: Option<String>, name: String, bytes: Vec<u8>) {
        let Some(max) = self
            .net_idx(network)
            .and_then(|i| self.nets[i].info.uploads)
        else {
            return self.toast("This network doesn't take uploads.".into());
        };
        if bytes.len() as u64 > max {
            return self.toast(format!(
                "{name} is too big. The limit here is {} MB.",
                max >> 20
            ));
        }
        if let Some(b) = target.as_deref().and_then(|t| self.buffer_mut(network, t)) {
            b.uploads.push(name.clone());
        }
        let bytes = Blob(bytes.into());
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
        let n = &self.nets[self.current.as_deref().and_then(|c| self.net_idx(c))?];
        let b = &n.buffers[n.selected.as_deref().and_then(|k| n.buffer(k))?];
        Some((n.info.network.clone(), b.info.target.clone()))
    }

    /// Alt+Up/Down: the previous or next buffer, as listed.
    fn step_buffer(&mut self, step: isize) {
        let Some(ni) = self.current.as_deref().and_then(|c| self.net_idx(c)) else {
            return;
        };
        let n = &mut self.nets[ni];
        let len = n.buffers.len() as isize;
        if len == 0 {
            return;
        }
        let at = n
            .selected
            .as_deref()
            .and_then(|k| n.buffer(k))
            .map_or(-1, |i| i as isize);
        let next = (at + step).rem_euclid(len) as usize;
        let k = n.buffers[next].key.clone();
        n.select(&k);
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
                let n = &self.nets[i];
                let k = n.selected.clone().filter(|k| n.buffer(k).is_some())?;
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
            .is_some_and(|p| !alive(&self.nets, p));
        self.panes.retain(|p| alive(&self.nets, p));
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
            self.nets[ni].selected = Some(k);
            self.nets[ni].focus = true;
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
                let Some(bi) = self.nets[ni].buffer(k) else {
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
                if self.nets[ni].buffers[bi].info.kind == BufferKind::Channel {
                    egui::Panel::right("members")
                        .default_size(200.0)
                        .size_range(140.0..=360.0)
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
                        self.nets[i].select(&key(&ch));
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
        match ev {
            Event::Network(info) => {
                let origin = info.network.clone();
                let connected = info.state == ConnState::Connected;
                match self.net_idx(&origin) {
                    Some(i) => {
                        let n = &mut self.nets[i];
                        if connected && n.info.state != ConnState::Connected {
                            // Member lists are dropped with the connection.
                            for b in &mut n.buffers {
                                b.members = None;
                                b.members_asked = false;
                            }
                        }
                        if n.info.icon != info.icon {
                            n.icon = Icon::None;
                        }
                        n.info = info;
                    }
                    None => {
                        if self.away_sent {
                            let message = Some(AWAY.to_owned());
                            self.send(&origin, NetCommand::SetAway { message });
                        }
                        self.nets.push(Network {
                            info,
                            buffers: Vec::new(),
                            selected: None,
                            focus: false,
                            icon: Icon::None,
                            profiles: HashMap::new(),
                            devices: HashMap::new(),
                            media: Media::default(),
                        });
                    }
                }
                let i = self.net_idx(&origin).unwrap_or_default();
                let waiting = matches!(
                    self.nets[i].info.state,
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
                        self.nets[i].selected = Some(key(&ch));
                    }
                }
                if self.current.is_none() {
                    self.current = Some(origin);
                }
            }
            Event::NetworkRemoved { network } => {
                self.nets.retain(|n| n.info.network != network);
                if self.current.as_deref() == Some(&network) {
                    self.current = self.nets.first().map(|n| n.info.network.clone());
                }
            }
            Event::JoinFailed { invite, error } => match &mut self.join {
                Some(j) if j.busy.as_deref() == Some(&invite) => {
                    j.busy = None;
                    j.error = Some(error);
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
                let n = &mut self.nets[ni];
                let k = key(&buffer.target);
                match n.buffer(&k) {
                    Some(bi) => {
                        let b = &mut n.buffers[bi];
                        if b.info.unread != buffer.unread {
                            b.mark_sent = false;
                        }
                        b.info = buffer;
                    }
                    None => {
                        n.buffers.push(Buffer {
                            key: k.clone(),
                            info: buffer,
                            log: ChatLog::default(),
                            members: None,
                            members_asked: false,
                            typing: Vec::new(),
                            history_start: false,
                            older_asked: None,
                            mark_sent: false,
                            draft: String::new(),
                            tab: None,
                            reply: None,
                            jump: None,
                            voice: Vec::new(),
                            uploads: Vec::new(),
                        });
                        n.buffers.sort_by(|a, b| {
                            (a.info.kind == BufferKind::Dm, &a.key)
                                .cmp(&(b.info.kind == BufferKind::Dm, &b.key))
                        });
                        if n.selected.is_none() || n.selected.as_deref() == Some(&k) {
                            n.select(&k);
                        }
                    }
                }
            }
            Event::BufferRemoved { network, target } => {
                let Some(ni) = self.net_idx(&network) else {
                    return;
                };
                let n = &mut self.nets[ni];
                let k = key(&target);
                n.buffers.retain(|b| b.key != k);
                if n.selected.as_deref() == Some(&k) {
                    n.selected = None;
                    if let Some(first) = n.buffers.first().map(|b| b.key.clone()) {
                        n.select(&first);
                    }
                }
            }
            Event::BufferRenamed { network, from, to } => {
                let Some(ni) = self.net_idx(&network) else {
                    return;
                };
                let n = &mut self.nets[ni];
                let (old, new) = (key(&from), key(&to));
                let Some(bi) = n.buffer(&old) else { return };
                n.buffers[bi].key = new.clone();
                n.buffers.sort_by(|a, b| {
                    (a.info.kind == BufferKind::Dm, &a.key)
                        .cmp(&(b.info.kind == BufferKind::Dm, &b.key))
                });
                if n.selected.as_deref() == Some(&old) {
                    n.selected = Some(new.clone());
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
                let n = &mut self.nets[ni];
                let Some(bi) = n.buffer(&k) else { return };
                let b = &mut n.buffers[bi];
                b.older_asked = None;
                let new = b.log.merge(&messages);
                for &i in &new {
                    let m = &messages[i];
                    b.typing.retain(|(nick, _)| nick != &m.nick);
                    if !viewing && should_notify(self.settings.notify, m) {
                        let who = chatlog::shown(&n.profiles, &m.nick);
                        let summary = match b.info.kind {
                            BufferKind::Dm => who.to_owned(),
                            BufferKind::Channel => format!("{who} in {}", b.info.target),
                        };
                        platform::notify(summary, chatlog::snippet(&format::strip(&m.text), 200));
                    }
                }
                if let Some(id) = &b.jump
                    && let Some(i) = b.log.index_of(id)
                {
                    b.log.scroll_to_index(i, 60.0);
                    b.jump = None;
                }
            }
            Event::HistoryStart { network, target } => {
                if let Some(b) = self.buffer_mut(&network, &target) {
                    b.history_start = true;
                }
            }
            Event::Members {
                network,
                channel,
                members,
            } => {
                if let Some(b) = self.buffer_mut(&network, &channel) {
                    b.members = Some(members);
                }
            }
            Event::Typing {
                network,
                target,
                nick,
                state,
            } => {
                if let Some(b) = self.buffer_mut(&network, &target) {
                    b.typing.retain(|(n, _)| n != &nick);
                    if state == Typing::Active {
                        b.typing.push((nick, Instant::now()));
                    }
                }
            }
            Event::Voice { network, state } => {
                if state != VoiceState::Idle {
                    self.call = Some((network, state));
                } else if self.call.as_ref().is_some_and(|(n, _)| *n == network) {
                    self.call = None;
                }
            }
            Event::VoiceMembers {
                network,
                channel,
                nicks,
            } => {
                if let Some(b) = self.buffer_mut(&network, &channel) {
                    b.voice = nicks;
                }
            }
            Event::Error { network, error } => {
                let name = self
                    .net_idx(&network)
                    .map_or(network, |i| self.nets[i].info.name.clone());
                self.toast(format!("{name}: {error}"));
            }
            Event::Uploaded {
                network,
                target,
                result,
            } => {
                match (&target, result) {
                    (Some(t), Ok(url)) => {
                        if let Some(b) = self.buffer_mut(&network, t) {
                            if !b.draft.is_empty() && !b.draft.ends_with(char::is_whitespace) {
                                b.draft.push(' ');
                            }
                            b.draft.push_str(url.as_str());
                        }
                    }
                    (None, Ok(url)) => self.send(
                        &network,
                        NetCommand::SetAvatar {
                            url: url.to_string(),
                        },
                    ),
                    (_, Err(e)) => self.toast(format!("Upload failed: {e}")),
                }
                if let Some(b) = target.and_then(|t| self.buffer_mut(&network, &t))
                    && !b.uploads.is_empty()
                {
                    b.uploads.remove(0);
                }
            }
            Event::Profile {
                network,
                nick,
                display_name,
                avatar,
            } => {
                if let Some(i) = self.net_idx(&network) {
                    let p = Profile {
                        display_name,
                        avatar,
                    };
                    self.nets[i].profiles.insert(key(&nick), p);
                }
            }
            Event::Devices {
                network,
                nick,
                devices,
            } => {
                if let Some(i) = self.net_idx(&network) {
                    self.nets[i].devices.insert(key(&nick), devices);
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
                    self.nets[i].media.preview(url, preview);
                }
            }
            Event::Media {
                network,
                url,
                result,
            } => {
                if let Some(i) = self.net_idx(&network) {
                    self.nets[i].media.bytes(url, result, ctx);
                }
            }
        }
    }

    fn buffer_mut(&mut self, network: &str, target: &str) -> Option<&mut Buffer> {
        let ni = self.net_idx(network)?;
        let n = &mut self.nets[ni];
        let bi = n.buffer(&key(target))?;
        Some(&mut n.buffers[bi])
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
        for n in &mut self.nets {
            n.media.poll(ctx);
            for cmd in n.media.wanted.drain(..) {
                self.core.send(Command::Net {
                    network: n.info.network.clone(),
                    cmd,
                });
            }
        }
        while let Ok((network, target, path)) = self.files.1.try_recv() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            let max = self
                .net_idx(&network)
                .and_then(|i| self.nets[i].info.uploads)
                .unwrap_or(0);
            match read_file(&path, max) {
                Ok(bytes) => self.upload(&network, target, name, bytes),
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
            for n in &self.nets {
                let message = away.then(|| AWAY.to_owned());
                self.send(&n.info.network, NetCommand::SetAway { message });
            }
        }
        if !idle.is_zero() && !away {
            ctx.request_repaint_after(Duration::from_secs(30));
        }
        while let Ok((origin, img)) = self.icons.1.try_recv() {
            if let Some(i) = self.net_idx(&origin) {
                self.nets[i].icon = match img {
                    Some(img) => Icon::Ready(ctx.load_texture(
                        format!("icon:{origin}"),
                        img,
                        Default::default(),
                    )),
                    None => Icon::Failed,
                };
            }
        }
        for n in &mut self.nets {
            if let (Icon::None, Some(url)) = (&n.icon, &n.info.icon) {
                n.icon = Icon::Loading;
                fetch_icon(
                    n.info.network.clone(),
                    url.clone(),
                    self.icons.0.clone(),
                    ctx,
                );
            }
        }
        let now = (
            self.nets.iter().any(Network::unread),
            self.nets.iter().map(Network::mentions).sum(),
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
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            let mut pick = None;
            for n in &self.nets {
                let selected = self.current.as_deref() == Some(&n.info.network);
                let (rect, resp) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::click());
                let p = ui.painter();
                let round = if selected || resp.hovered() { 14 } else { 24 };
                match &n.icon {
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
                    let pill = egui::Rect::from_center_size(
                        egui::pos2(ui.max_rect().left() + 2.0, rect.center().y),
                        vec2(4.0, h),
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
            if !self.nets.is_empty() {
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
            ui.add_space(8.0);
            if ui
                .add_sized(
                    [48.0, 48.0],
                    egui::Button::new(RichText::new(icons::GEAR).size(20.0)),
                )
                .tip("Settings")
                .clicked()
            {
                self.show_settings = !self.show_settings;
            }
        });
    }

    fn channels(&mut self, ui: &mut egui::Ui, ni: usize) {
        let network = self.nets[ni].info.network.clone();
        if let Some((net, state)) = self.call.clone() {
            let name = self
                .net_idx(&net)
                .map_or(net.clone(), |i| self.nets[i].info.name.clone());
            let action = egui::Panel::bottom("voice-panel")
                .show(ui, |ui| {
                    voice::panel(ui, &self.voice, &state, &name, self.ptt.as_ref())
                })
                .inner;
            match action {
                Some(voice::PanelAction::Leave) => self.send(&net, NetCommand::LeaveVoice),
                Some(voice::PanelAction::Share) => {
                    self.share_dialog = Some(ShareDialog::new(&self.voice, ui.ctx()));
                }
                Some(voice::PanelAction::StopShare) => self.voice.stop_share(),
                None => {}
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(icons::DOTS_THREE, |ui| {
                    if ui.button("Leave network…").clicked() {
                        self.confirm_remove = Some(network.clone());
                    }
                })
                .response
                .tip("Network options");
                // Whatever's left of the menu, so long names truncate instead of overlapping it.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let name = RichText::new(&self.nets[ni].info.name).heading();
                    ui.add(egui::Label::new(name).truncate());
                });
            });
        });
        if let Some((text, bad)) = state_text(&self.nets[ni].info.state) {
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
        let own = self.nets[ni].info.nick.clone();
        let shown = chatlog::shown(&self.nets[ni].profiles, &own).to_owned();
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
                let name = self.nets[ni]
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
                            if ui.small_button("+").tip(hint).clicked() {
                                self.adding = Some((kind, String::new()));
                            }
                        });
                    });
                    let n = &self.nets[ni];
                    for b in n.buffers.iter().filter(|b| b.info.kind == kind) {
                        let selected = n.selected.as_deref() == Some(&b.key);
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
                            &self.call,
                            Some((net, VoiceState::Connecting { channel } | VoiceState::Connected { channel }))
                                if *net == network && key(channel) == b.key
                        );
                        if b.info.voice && !in_call {
                            let right = if b.info.mentions > 0 { 34.0 } else { 14.0 };
                            let at = egui::Rect::from_center_size(
                                resp.rect.right_center() - vec2(right, 0.0),
                                vec2(20.0, 20.0),
                            );
                            let join = egui::Button::new(RichText::new(icons::SPEAKER_HIGH).small()).frame(false);
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
                            && matches!(&self.call, Some((_, VoiceState::Connected { .. })));
                        if (live || !b.voice.is_empty())
                            && let Some(who) = voice::members(
                                ui,
                                &self.voice,
                                live,
                                &b.voice,
                                &mut self.settings.volumes,
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
            self.nets[ni].select(&k);
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

    fn members(&mut self, ui: &mut egui::Ui, ni: usize, bi: usize) {
        let dark = ui.visuals().dark_mode;
        let network = self.nets[ni].info.network.clone();
        let own = self.nets[ni].info.nick.clone();
        let profiles = &self.nets[ni].profiles;
        let Some(members) = &self.nets[ni].buffers[bi].members else {
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
            self.nets[ni].select(&key(&nick));
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
        let network = self.nets[ni].info.network.clone();
        let own = self.nets[ni].info.nick.clone();
        let connected = self.nets[ni].info.state == ConnState::Connected;
        let focused = ui.input(|i| i.focused);
        let core = self.core.clone();
        let send = |cmd| {
            core.send(Command::Net {
                network: network.clone(),
                cmd,
            })
        };
        let uploads = self.nets[ni].info.uploads.is_some();
        let auto_media = self.settings.load_media;
        let n = &mut self.nets[ni];
        let (b, profiles, media, devices) =
            (&mut n.buffers[bi], &n.profiles, &mut n.media, &n.devices);
        let focus = active && std::mem::take(&mut n.focus);
        let target = b.info.target.clone();
        let mut close = false;
        // Set inside the panels, acted on once `b` is free again.
        let mut attach = false;
        let mut emoji_at = None;
        let mut pick_reaction = None;
        let mut pasted = Vec::new();

        if connected && b.info.kind == BufferKind::Channel && !b.members_asked {
            b.members_asked = true;
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
            b.typing.retain(|(_, t)| t.elapsed() < TYPING_TTL);
            let typing = match b.typing.as_slice() {
                [] => String::new(),
                [(a, _)] => format!("{a} is typing…"),
                [(a, _), (c, _)] => format!("{a} and {c} are typing…"),
                _ => "Several people are typing…".into(),
            };
            if !b.typing.is_empty() {
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            }
            ui.label(RichText::new(typing).small().weak());
            for name in &b.uploads {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(format!("Uploading {name}…")).small());
                });
            }
            let id = egui::Id::unique(("composer", &network, &b.key));
            // The `@` list was open last frame, so Tab, Enter and Escape are its.
            let open_id = id.with("completing");
            let completing = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
            if let Some((_, nick)) = &b.reply {
                let mut cancel = false;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Replying to {nick}")).small());
                    cancel = ui.small_button(icons::X).tip("Cancel reply").clicked();
                });
                if cancel || (!completing && ui.input(|i| i.key_pressed(Key::Escape))) {
                    b.reply = None;
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
            let out = ui
                .horizontal(|ui| {
                    if uploads
                        && ui
                            .button(RichText::new(icons::PAPERCLIP).size(18.0))
                            .tip("Upload a file")
                            .clicked()
                    {
                        attach = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let e = ui
                            .button(RichText::new(icons::SMILEY).size(18.0))
                            .tip("Emoji");
                        if e.clicked() {
                            emoji_at = Some(e.rect.left_top());
                        }
                        let edit = egui::TextEdit::multiline(&mut b.draft)
                            .hint_text(format!("Message {target}"))
                            .desired_rows(1)
                            .desired_width(f32::INFINITY)
                            .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                            .lock_focus(true);
                        let (members, log, info) = (&b.members, &b.log, &b.info);
                        egui::CompletionPopup::new(id).show(ui, edit, |q| {
                            if !q.word.starts_with('@') {
                                return Vec::new();
                            }
                            let nicks = completable(members, log, info, &own);
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
                let end = b.draft.chars().count();
                let mut cursor = state
                    .cursor
                    .char_range()
                    .map_or(end, |r| r.primary.index.into());
                let mut moved = false;
                if let Some(back) = tab {
                    let nicks = completable(&b.members, &b.log, &b.info, &own);
                    if let Some(c) = complete::tab(&mut b.draft, cursor, &mut b.tab, &nicks, back) {
                        (cursor, moved, changed) = (c, true, true);
                    }
                }
                if changed && emoji::replace_shortcodes(&mut b.draft, &mut cursor) {
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
                let state = if b.draft.trim().is_empty() {
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
                let text = b.draft.trim_end().to_owned();
                if !text.trim().is_empty() {
                    let reply_to = b.reply.take().map(|(id, _)| id);
                    send(NetCommand::Send {
                        target: target.clone(),
                        text,
                        reply_to,
                    });
                    b.draft.clear();
                    b.log.scroll_to_bottom();
                }
            }
            ui.add_space(4.0);
        });

        let dm = egui::CentralPanel::default().show(ui, |ui| {
            if b.log.msgs.is_empty() {
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
            let out = b
                .log
                .ui(ui, &own, b.info.kind == BufferKind::Channel, &mut extras);
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
                            b.log.scroll_to_bottom();
                        }
                    });
            }
            if out.near_top
                && connected
                && !b.history_start
                && !b.log.msgs.is_empty()
                && b.older_asked
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
            {
                b.older_asked = Some(Instant::now());
                send(NetCommand::LoadOlder {
                    target: target.clone(),
                });
            }
            if out.at_bottom && focused && b.info.unread > 0 && !b.mark_sent {
                b.mark_sent = true;
                send(NetCommand::MarkRead {
                    target: target.clone(),
                });
            }
            let mut dm = None;
            for a in out.actions {
                match a {
                    Action::Reply(i) => {
                        if let Some(id) = b.log.msgs[i].msgid.clone() {
                            b.reply = Some((id, b.log.msgs[i].nick.clone()));
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
                    Action::Jump { msgid } => match b.log.index_of(&msgid) {
                        Some(i) => b.log.scroll_to_index(i, 60.0),
                        None => {
                            b.jump = Some(msgid.clone());
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
            self.nets[ni].select(&key(&nick));
        }
        if let Some((msgid, at)) = pick_reaction {
            self.picker = Some(Picker {
                react: Some((network.clone(), target.clone(), msgid)),
                at,
                query: String::new(),
                shown: false,
            });
        }
        if let Some(at) = emoji_at {
            self.picker = Some(Picker {
                react: None,
                at,
                query: String::new(),
                shown: false,
            });
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
                        self.upload(&network, Some(target.clone()), "pasted.png".into(), bytes)
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
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let label = match j.mode {
                        JoinMode::Login => "Log in",
                        JoinMode::Register => "Sign up",
                        JoinMode::Guest => "Join",
                    };
                    submit = ui
                        .add_enabled(ready && !busy, egui::Button::new(label))
                        .clicked()
                        || (ready && !busy && ui.input(|i| i.key_pressed(Key::Enter)));
                    if busy {
                        ui.spinner();
                    }
                });
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

        if let Some(ni) = self.current.as_deref().and_then(|c| self.net_idx(c))
            && let ConnState::NeedsVerification { message } = &self.nets[ni].info.state
            && self.verify_dismissed.as_deref() != Some(&self.nets[ni].info.network)
        {
            let network = self.nets[ni].info.network.clone();
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
                .map_or(network.clone(), |i| self.nets[i].info.name.clone());
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::unique("remove")).show(ctx, |ui| {
                ui.heading(format!("Leave {name}?"));
                ui.label("IrChad forgets this network and your saved password for it.");
                ui.label(
                    "This device's encryption keys go too, so it can't read encrypted DMs again.",
                );
                ui.label("Your account stays, so you can join again later.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    let leave = egui::Button::new(RichText::new("Leave").color(Color32::WHITE))
                        .fill(Color32::from_rgb(218, 55, 60));
                    if ui.add(leave).clicked() {
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
        let Some(p) = &mut self.picker else { return };
        let mut picked = None;
        let area = egui::Area::new(egui::Id::unique("emoji-picker"))
            .fixed_pos(p.at)
            .pivot(Align2::RIGHT_BOTTOM)
            .constrain(true)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    picked = emoji::picker(ui, &mut p.query);
                });
            });
        let close = (p.shown && area.response.clicked_elsewhere())
            || ctx.input(|i| i.key_pressed(Key::Escape));
        p.shown = true;
        if let Some(e) = picked {
            match p.react.clone() {
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
                        && let Some(b) = self.buffer_mut(&network, &target)
                    {
                        b.draft.push_str(&e);
                    }
                }
            }
            self.picker = None;
        } else if close {
            self.picker = None;
        }
    }

    fn switcher_ui(&mut self, ctx: &egui::Context) {
        let Some(sw) = &mut self.switcher else { return };
        let q = sw.query.trim().to_lowercase();
        // (network, key, label), names starting with the query first.
        let mut hits: Vec<(bool, String, String, String)> = self
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
                self.nets[i].select(&k);
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
        let n = &mut self.nets[ni];
        let own = n.info.nick.clone();
        let avatar = n.profiles.get(&key(&own)).and_then(|p| p.avatar.clone());
        let (mut save, mut close, mut choose, mut remove) = (false, false, false, false);
        let modal = egui::Modal::new(egui::Id::unique("profile")).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.heading(format!("Your profile on {}", n.info.name));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
                n.media.avatar(ui, rect, &own, avatar.as_ref(), true);
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
            ui.horizontal(|ui| {
                close = ui.button("Cancel").clicked();
                save = ui.button("Save").clicked();
            });
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
