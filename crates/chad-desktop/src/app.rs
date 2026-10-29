//! The window: projects core events into a view model and draws it.
//!
//! Layout, left to right: network rail, channel list (with the voice panel at
//! its foot during a call), chat (topic, log, composer), member list.

use std::collections::HashSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chad_core::{
    Auth, BufferInfo, BufferKind, ChatMessage, Command, ConnState, Core, Event, Member, NetCommand,
    NetworkInfo, SendStatus, Typing, VoiceState, invite,
};
use chad_media::{InputMode, Voice};
use egui::{
    Align2, Color32, CornerRadius, FontId, Key, KeyboardShortcut, Modifiers, RichText, Sense,
    TextureHandle, vec2,
};
use tokio::sync::mpsc::UnboundedReceiver;
use url::Url;

use crate::chatlog::{self, Action, ChatLog};
use crate::platform::ptt::Ptt;
use crate::platform::{self, Os, Tray};
use crate::settings::{self, Notify, Settings};
use crate::voice;
use crate::{format, icons};

const TYPING_TTL: Duration = Duration::from_secs(6);
const TOAST_TTL: Duration = Duration::from_secs(8);
/// Only messages this fresh notify; gap-fill after a restart stays quiet.
const NOTIFY_WINDOW_MS: i64 = 60_000;

/// A network's decoded icon, or `None` if it couldn't be had.
type IconMsg = (String, Option<egui::ColorImage>);

pub struct App {
    core: Core,
    events: UnboundedReceiver<Event>,
    os: mpsc::Receiver<Os>,
    tray: Tray,
    icons: (mpsc::Sender<IconMsg>, mpsc::Receiver<IconMsg>),
    nets: Vec<Network>,
    /// Origin of the network on screen.
    current: Option<String>,
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
}

struct Network {
    info: NetworkInfo,
    buffers: Vec<Buffer>,
    /// Key of the buffer on screen.
    selected: Option<String>,
    icon: Icon,
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
    /// (msgid, nick) of the message being replied to.
    reply: Option<(String, String)>,
    /// A message to scroll to once `LoadAround` brings it in.
    jump: Option<String>,
    /// Who's in the channel's voice room (metadata).
    voice: Vec<String>,
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

fn state_text(state: &ConnState) -> Option<(String, bool)> {
    match state {
        ConnState::Connected => None,
        ConnState::Connecting => Some(("Connecting…".into(), false)),
        ConnState::NeedsVerification { .. } => Some(("Waiting for your email code".into(), false)),
        ConnState::Disconnected { error } => Some((format!("{error} Reconnecting…"), false)),
        ConnState::Failed { error } => Some((error.clone(), true)),
    }
}

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
        settings.apply(&cc.egui_ctx);
        voice.set_settings(settings.audio.clone());
        for (id, v) in &settings.volumes {
            voice.set_volume(id, *v);
        }
        let mut app = Self {
            core,
            events,
            os,
            tray,
            icons: mpsc::channel(),
            nets: Vec::new(),
            current: None,
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
                    None => self.nets.push(Network {
                        info,
                        buffers: Vec::new(),
                        selected: None,
                        icon: Icon::None,
                    }),
                }
                let i = self.net_idx(&origin).unwrap_or_default();
                let waiting = matches!(
                    self.nets[i].info.state,
                    ConnState::Connected | ConnState::NeedsVerification { .. }
                );
                if waiting
                    && let Some(j) = &self.join
                    && j.busy
                        .as_deref()
                        .and_then(|b| invite::parse(b).ok())
                        .is_some_and(|inv| inv.origin == origin)
                {
                    self.join = None;
                    self.current = Some(origin.clone());
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
                            reply: None,
                            jump: None,
                            voice: Vec::new(),
                        });
                        n.buffers.sort_by(|a, b| {
                            (a.info.kind == BufferKind::Dm, &a.key)
                                .cmp(&(b.info.kind == BufferKind::Dm, &b.key))
                        });
                        if n.selected.is_none() {
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
                let on_screen = self.current.as_deref() == Some(&network);
                let Some(ni) = self.net_idx(&network) else {
                    return;
                };
                let n = &mut self.nets[ni];
                let k = key(&target);
                let Some(bi) = n.buffer(&k) else { return };
                let viewing = seen && on_screen && n.selected.as_deref() == Some(&k);
                let b = &mut n.buffers[bi];
                b.older_asked = None;
                let new = b.log.merge(&messages);
                for &i in &new {
                    let m = &messages[i];
                    b.typing.retain(|(nick, _)| nick != &m.nick);
                    if !viewing && should_notify(self.settings.notify, m) {
                        let summary = match b.info.kind {
                            BufferKind::Dm => m.nick.clone(),
                            BufferKind::Channel => format!("{} in {}", m.nick, b.info.target),
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
                if resp.on_hover_text(&n.info.name).clicked() {
                    pick = Some(n.info.network.clone());
                }
                ui.add_space(8.0);
            }
            if pick.is_some() {
                self.current = pick;
            }
            ui.separator();
            let add = ui
                .add_sized(
                    [48.0, 48.0],
                    egui::Button::new(RichText::new("+").size(24.0)),
                )
                .on_hover_text("Join a network");
            if add.clicked() {
                self.join = Some(JoinDialog::default());
            }
            ui.add_space(8.0);
            if ui
                .add_sized(
                    [48.0, 48.0],
                    egui::Button::new(RichText::new(icons::GEAR).size(20.0)),
                )
                .on_hover_text("Settings")
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
            if let Some(voice::PanelAction::Leave) = action {
                self.send(&net, NetCommand::LeaveVoice);
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.heading(&self.nets[ni].info.name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(icons::DOTS_THREE, |ui| {
                    if ui.button("Leave network…").clicked() {
                        self.confirm_remove = Some(network.clone());
                    }
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
        ui.label(
            RichText::new(format!("Signed in as {}", self.nets[ni].info.nick))
                .small()
                .weak(),
        );
        ui.separator();

        let mut pick = None;
        let mut part = None;
        let mut close = None;
        let mut join_voice = None;
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
                            if ui.small_button("+").on_hover_text(hint).clicked() {
                                self.adding = Some((kind, String::new()));
                                ui.memory_mut(|m| m.request_focus(adding_id()));
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
                        let mut tip = None;
                        if kind == BufferKind::Dm {
                            let dot;
                            (dot, tip) = match (b.info.online, b.info.away) {
                                (Some(true), false) => ("●", Some("Online")),
                                (Some(true), true) => ("◐", Some("Away")),
                                _ => ("○", Some("Offline")),
                            };
                            text = RichText::new(format!("{dot} {}", b.info.target)).color(
                                if b.info.unread > 0 || selected {
                                    ui.visuals().strong_text_color()
                                } else {
                                    ui.visuals().weak_text_color()
                                },
                            );
                        }
                        let mut resp = ui
                            .with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                                ui.add(
                                    egui::Button::selectable(selected, text)
                                        .frame_when_inactive(false)
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
                            if ui.put(at, join).on_hover_text("Join voice").clicked() {
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
                        if live || !b.voice.is_empty() {
                            voice::members(
                                ui,
                                &self.voice,
                                live,
                                &b.voice,
                                &mut self.settings.volumes,
                            );
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
                        let mut t = RichText::new(format!("{}{}", m.prefix, m.nick))
                            .color(chatlog::nick_color(&m.nick, dark));
                        if away {
                            t = t.weak();
                        }
                        let r = ui
                            .add(egui::Label::new(t).sense(Sense::click()).truncate())
                            .on_hover_text(match &m.account {
                                Some(a) => format!("Account: {a}"),
                                None => "Not logged in".into(),
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

    fn chat(&mut self, ui: &mut egui::Ui, ni: usize, bi: usize) {
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
        let b = &mut self.nets[ni].buffers[bi];
        let target = b.info.target.clone();

        if connected && b.info.kind == BufferKind::Channel && !b.members_asked {
            b.members_asked = true;
            send(NetCommand::Members {
                channel: target.clone(),
            });
        }

        egui::Panel::top("topic").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(&target);
                if b.info.kind == BufferKind::Dm {
                    let status = match (b.info.online, b.info.away) {
                        (Some(true), false) => "online",
                        (Some(true), true) => "away",
                        (Some(false), _) => "offline",
                        (None, _) => "",
                    };
                    ui.label(RichText::new(status).weak());
                }
                let topic = format::strip(&b.info.topic);
                if !topic.is_empty() {
                    ui.separator();
                    ui.add(egui::Label::new(RichText::new(&topic).weak()).truncate())
                        .on_hover_text(&topic);
                }
            });
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
            if let Some((_, nick)) = &b.reply {
                let mut cancel = false;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Replying to {nick}")).small());
                    cancel = ui
                        .small_button(icons::X)
                        .on_hover_text("Cancel reply")
                        .clicked();
                });
                if cancel || ui.input(|i| i.key_pressed(Key::Escape)) {
                    b.reply = None;
                }
            }
            let resp = ui.add(
                egui::TextEdit::multiline(&mut b.draft)
                    .hint_text(format!("Message {target}"))
                    .desired_rows(1)
                    .desired_width(f32::INFINITY)
                    .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                    .id_salt(("composer", &network, &b.key)),
            );
            if resp.changed() {
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
            let out = b.log.ui(ui, &own, b.info.kind == BufferKind::Channel);
            if !out.at_bottom {
                let pos = ui.max_rect().right_bottom() - vec2(16.0, 16.0);
                egui::Area::new(egui::Id::unique("jump-latest"))
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
                }
            }
            dm
        });
        if let Some(nick) = dm.inner {
            send(NetCommand::OpenDm { nick: nick.clone() });
            self.nets[ni].select(&key(&nick));
        }
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

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(j) = &mut self.join {
            let mut submit = false;
            let mut cancel = false;
            let busy = j.busy.is_some();
            let modal = egui::Modal::new(egui::Id::unique("join")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading("Join a network");
                ui.add_space(8.0);
                ui.label("Invite link");
                ui.add_enabled(
                    !busy,
                    egui::TextEdit::singleline(&mut j.invite)
                        .hint_text("https://chat.example.com/invite")
                        .desired_width(f32::INFINITY),
                );
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
                        ui.label(if guest { "Nickname" } else { "Username" });
                        ui.add_enabled(!busy, egui::TextEdit::singleline(&mut j.account));
                        ui.end_row();
                        if !guest {
                            ui.label("Password");
                            ui.add_enabled(
                                !busy,
                                egui::TextEdit::singleline(&mut j.password).password(true),
                            );
                            ui.end_row();
                        }
                        if j.mode == JoinMode::Register {
                            ui.label("Email");
                            ui.add_enabled(
                                !busy,
                                egui::TextEdit::singleline(&mut j.email).hint_text("optional"),
                            );
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
                ui.label("Your account stays, so you can join again later.");
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
                                if ui.small_button(icons::X).clicked() {
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);

        egui::Panel::left("rail")
            .exact_size(68.0)
            .resizable(false)
            .show(ui, |ui| self.rail(ui));

        let ni = self.current.as_deref().and_then(|c| self.net_idx(c));
        let bi = ni.and_then(|ni| {
            let n = &self.nets[ni];
            n.selected.as_deref().and_then(|k| n.buffer(k))
        });
        if let Some(ni) = ni {
            egui::Panel::left("channels")
                .default_size(220.0)
                .size_range(160.0..=400.0)
                .show(ui, |ui| self.channels(ui, ni));
        }
        if let (Some(ni), Some(bi)) = (ni, bi)
            && self.nets[ni].buffers[bi].info.kind == BufferKind::Channel
        {
            egui::Panel::right("members")
                .default_size(200.0)
                .size_range(140.0..=360.0)
                .show(ui, |ui| self.members(ui, ni, bi));
        }
        egui::CentralPanel::default().show(ui, |ui| match (ni, bi) {
            (None, _) => self.welcome(ui),
            (Some(_), None) => {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("No channels yet. Join one with + on the left.").weak());
                });
            }
            (Some(ni), Some(bi)) => self.chat(ui, ni, bi),
        });

        self.dialogs(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, settings::KEY, &self.settings);
    }
}
