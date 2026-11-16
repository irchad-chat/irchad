//! The window: projects core events into a view model and draws it.
//!
//! Layout, left to right: network rail, channel list (with the voice panel at
//! its foot during a call), then the open buffers tiled as panes, each with
//! its own chat (topic, log, composer) and member list.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chad_core::{
    Auth, Blob, BufferKind, ChatMessage, Command, ConnState, Core, Crypt, DeviceInfo, Encryption,
    Event, Member, MessageKind, NetCommand, Typing, VerifyState, VerifyStep, VoiceState, invite,
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
use crate::platform::ptt::{Action as KeyAction, Ptt, Sink};
use crate::platform::{self, Os, Tray};
use crate::screen::{self, DialogOutcome, Layout, ShareDialog, StageAction};
use crate::settings::{self, Notify, Settings};
use crate::voice;
use crate::{emoji, gifs, update};
use chad_core::complete;
use chad_core::format;

mod chat;
mod dialogs;
mod events;
mod overlay;
mod sidebar;
mod tiles;
mod verify;

const TOAST_TTL: Duration = Duration::from_secs(8);
/// The AWAY reason other clients show.
const AWAY: &str = "Away";

/// A network's decoded icon, or `None` if it couldn't be had.
type IconMsg = (String, Option<egui::ColorImage>);
/// A file from the file dialog: (network, buffer or `None` for an avatar, path).
/// A file read for upload: (network, target, file name, its bytes).
type Picked = (String, Option<String>, String, Result<Vec<u8>, String>);
/// (network, file, the clip or why not).
type Decoded = (String, String, Result<std::sync::Arc<[i16]>, String>);

pub struct App {
    core: Core,
    events: UnboundedReceiver<Event>,
    os: mpsc::Receiver<Os>,
    tray: Tray,
    /// Quit from the tray: the close goes through instead of hiding.
    quitting: bool,
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
    settings_section: settings::Section,
    join: Option<JoinDialog>,
    /// The first-launch card on screen.
    welcome_card: usize,
    /// The cards were read or skipped this run. They're back next launch,
    /// until a first network is signed in to (`Settings::welcomed`).
    welcome_done: bool,
    /// The app icon, for the first-launch cards.
    logo: Option<egui::TextureHandle>,
    verify_code: String,
    signin_password: String,
    verify_dismissed: Option<String>,
    confirm_remove: Option<String>,
    /// The "Delete account" box in the leave dialog.
    delete_account: bool,
    /// A guest making an account.
    signup: Option<SignUpDialog>,
    set_password: Option<PasswordDialog>,
    adding: Option<(BufferKind, String)>,
    /// "Browse channels…": the network's LIST.
    browse: Option<ChannelBrowser>,
    toasts: Vec<(String, Instant)>,
    /// Invites to answer, with the toasts: (network, from, channel).
    invites: Vec<(String, String, String)>,
    /// Toasts from background tasks.
    toast_rx: (mpsc::Sender<String>, mpsc::Receiver<String>),
    /// Where the logs (and caches) live.
    data_dir: std::path::PathBuf,
    last_badge: (bool, u32),
    last_title: String,
    voice: Voice,
    /// Listening for push-to-talk while that's the input mode, the whisper
    /// key, and the overlay's combo.
    ptt: Option<Ptt>,
    /// The key being bound in the voice or overlay settings.
    binding_ptt: Option<KeyAction>,
    /// The overlay is toggled on by its combo (D74).
    overlay_on: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Recent messages for the overlay.
    overlay_lines: crate::overlay::Lines,
    /// Watching for a focused game, while the overlay is enabled.
    focus: Option<platform::overlay::Focus>,
    /// When "Preview" was clicked in its settings.
    overlay_preview: Option<Instant>,
    /// eframe's GPU, which the Wayland overlay draws with too.
    #[cfg(all(target_os = "linux", feature = "wayland"))]
    wgpu: Option<eframe::egui_wgpu::RenderState>,
    /// eframe's `wl_display`, which the overlay's layer surface shares.
    #[cfg(all(target_os = "linux", feature = "wayland"))]
    wl_display: Option<eframe::egui_wgpu::wgpu::rwh::RawDisplayHandle>,
    /// The overlay's layer surface on Wayland, while it's up (D75).
    #[cfg(all(target_os = "linux", feature = "wayland"))]
    overlay_layer: Option<platform::overlay_wayland::Layer>,
    share_dialog: Option<ShareDialog>,
    /// Why turning the camera on failed, once it has.
    camera_error: Option<std::sync::mpsc::Receiver<String>>,
    /// Why watching a public share (D87) failed.
    stream_status: Option<std::sync::mpsc::Receiver<String>>,
    /// Our share's public link, to copy again.
    stream_link: Option<Url>,
    /// The stream room we asked a token for, so we ask once per watch.
    stream_asked: Option<String>,
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
    /// Rich Presence listener, while `share_activity` is on.
    rpc: Option<platform::rpc::Rpc>,
    /// Running programs and media players, while sharing either.
    poll: Option<platform::presence::Poll>,
    /// Discord's game list, from a chadd that serves it; the poller reads it too.
    games: tokio::sync::watch::Sender<Option<std::sync::Arc<crate::presence::Games>>>,
    /// When we last asked for it; it's asked again later if that failed.
    games_asked: Option<Instant>,
    /// Where the last list chadd sent is kept, so games are spotted from launch.
    games_cache: std::path::PathBuf,
    /// Reading `games_cache`, at startup; chadd isn't asked until it's done.
    games_load: Option<std::thread::JoinHandle<Option<chad_core::GameList>>>,
    /// The ETag of the list we have, sent back so an unchanged one isn't resent.
    games_etag: Option<String>,
    /// Whether chadd has answered this run (a new list or "still current").
    games_fresh: bool,
    /// Why the last ask for it failed, for the overlay's settings.
    games_error: Option<String>,
    /// The `chad/activity` we last sent everywhere (`None`: we leave it alone),
    /// and when.
    activity_sent: (Option<String>, Option<Instant>),
    /// The `chad/music` sent with it.
    music_sent: Option<chad_core::Music>,
    update: mpsc::Receiver<update::Status>,
    /// A newer build, for the banner.
    new_version: Option<update::Status>,
}

#[derive(Clone, Copy, PartialEq)]
enum PickerTab {
    Emoji,
    Stickers,
    Gifs,
}

/// The emoji picker, for the composer or a reaction. The composer's has a
/// sticker tab too when the network has stickers, and a GIF tab when it
/// searches GIFs.
struct Picker {
    react: Option<(String, String, String)>,
    at: egui::Pos2,
    query: String,
    /// Shown for a frame already: the click that opened it is no "click elsewhere".
    shown: bool,
    tab: PickerTab,
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
    /// Read receipts in DMs (D68), as the account had them when opened.
    receipts: bool,
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
    /// This pane's member list is toggled off.
    hide_members: bool,
    /// The member panel's width, for the member list (by address) and
    /// profile count it was measured against.
    members_width: std::cell::Cell<Option<(usize, usize, f32)>>,
    older_asked: Option<Instant>,
    mark_sent: bool,
    draft: String,
    /// The Tab completion in progress in the draft.
    tab: Option<complete::Tab>,
    /// Nicks completed into the draft, for `+chad/mentions` (D66).
    picked: Vec<String>,
    /// (msgid, nick) of the message being replied to.
    reply: Option<(String, String)>,
    /// msgid of our message the draft replaces.
    editing: Option<String>,
    /// A message to scroll to once `LoadAround` brings it in.
    jump: Option<String>,
    /// Names of files on their way up.
    uploads: Vec<String>,
    /// Files waiting in the composer, uploaded when the message is sent.
    attached: Vec<Attachment>,
}

impl BufUi {
    /// A nick was completed into the draft.
    fn pick(&mut self, nick: &str) {
        if !nick.is_empty()
            && self.picked.len() < chad_core::mention::MAX * 4
            && !self.picked.iter().any(|p| p == nick)
        {
            self.picked.push(nick.to_owned());
        }
    }

    /// Puts our message `m` in the composer to edit it.
    fn start_edit(&mut self, m: &ChatMessage) {
        if let Some(id) = &m.msgid {
            self.reply = None;
            self.editing = Some(id.clone());
            // shortcut: the draft gets the text without its formatting;
            // turn IRC codes back into markdown if people miss it.
            self.draft = format::strip(&m.text);
            self.picked = m.mentions.iter().map(|x| x.nick.clone()).collect();
        }
    }
}

struct Attachment {
    name: String,
    bytes: Blob,
    /// Its preview in the network's `Media`.
    key: String,
}

/// Filled in for "Join a network", so newcomers can just sign up.
const DEFAULT_SERVER: &str = "irchad.chat";

/// The join dialog's checkbox for a classic IRC network (D72).
pub(crate) const CLASSIC_LABEL: &str =
    "I know what IRC is and I want to connect to a non-IrChad server";

struct JoinDialog {
    /// The server URL, or a classic network's host.
    invite: String,
    /// What the server offers, once it answered: the sign-in step.
    server: Option<chad_core::ServerInfo>,
    /// The invite `server` answered for, which the join sends.
    url: String,
    /// Any IRC network, by host and port (D72).
    classic: bool,
    /// Classic only; empty means the default for TLS or not.
    port: String,
    /// Classic only: no TLS.
    plaintext: bool,
    mode: JoinMode,
    account: String,
    password: String,
    email: String,
    /// The invite sent to the core, while waiting for the outcome.
    busy: Option<String>,
    error: Option<String>,
    /// Signing in with Discord (D96), once started.
    discord: Option<DiscordStep>,
}

enum DiscordStep {
    /// In the browser, at this link once the core has made it.
    Browser(Option<String>),
    /// A first sign-in: the account name to use.
    Name { name: String, error: Option<String> },
}

/// A password for other IRC clients, on a Discord account (D96).
struct PasswordDialog {
    network: String,
    password: String,
    busy: bool,
    error: Option<String>,
}

impl Default for JoinDialog {
    fn default() -> Self {
        Self {
            invite: DEFAULT_SERVER.into(),
            server: None,
            url: String::new(),
            classic: false,
            port: String::new(),
            plaintext: false,
            mode: JoinMode::default(),
            account: String::new(),
            password: String::new(),
            email: String::new(),
            busy: None,
            error: None,
            discord: None,
        }
    }
}

/// "Browse channels…": every channel on a network, from LIST.
struct ChannelBrowser {
    network: String,
    filter: String,
    /// `None` until the LIST is in.
    channels: Option<Vec<chad_core::ListedChannel>>,
}

impl ChannelBrowser {
    fn new(network: String) -> Self {
        Self {
            network,
            filter: String::new(),
            channels: None,
        }
    }
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

/// Narrowest the channel and member lists resize to; past that, names truncate.
const LIST_MIN_W: f32 = 120.0;

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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        core: Core,
        events: UnboundedReceiver<Event>,
        os: mpsc::Receiver<Os>,
        tray: Tray,
        open: Option<String>,
        voice: Voice,
        data_dir: &std::path::Path,
    ) -> Self {
        let games_cache = data_dir.join("games.json");
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
        voice.set_whisper_targets(settings.whisper_to.clone());
        let settings_check_updates = settings.check_updates;
        let auto_update = settings.auto_update && update::blocker().is_none();
        let mut app = Self {
            core,
            events,
            os,
            tray,
            quitting: false,
            icons: mpsc::channel(),
            view: View::default(),
            current: None,
            panes: Vec::new(),
            pane: 0,
            replace_pane: false,
            settings,
            show_settings: false,
            settings_section: settings::Section::default(),
            join: None,
            welcome_card: 0,
            welcome_done: false,
            logo: None,
            verify_code: String::new(),
            signin_password: String::new(),
            verify_dismissed: None,
            confirm_remove: None,
            delete_account: false,
            signup: None,
            set_password: None,
            adding: None,
            browse: None,
            toasts: Vec::new(),
            invites: Vec::new(),
            toast_rx: mpsc::channel(),
            data_dir: data_dir.to_owned(),
            last_badge: (false, 0),
            last_title: "IrChad".into(),
            voice,
            ptt: None,
            binding_ptt: None,
            overlay_on: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            overlay_lines: crate::overlay::Lines::default(),
            focus: None,
            overlay_preview: None,
            #[cfg(all(target_os = "linux", feature = "wayland"))]
            wgpu: cc.wgpu_render_state.clone(),
            #[cfg(all(target_os = "linux", feature = "wayland"))]
            wl_display: {
                use eframe::egui_wgpu::wgpu::rwh::HasDisplayHandle as _;
                cc.display_handle().ok().map(|h| h.as_raw())
            },
            #[cfg(all(target_os = "linux", feature = "wayland"))]
            overlay_layer: None,
            share_dialog: None,
            camera_error: None,
            stream_status: None,
            stream_link: None,
            stream_asked: None,
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
            rpc: None,
            poll: None,
            games: tokio::sync::watch::Sender::new(None),
            games_asked: None,
            games_load: Some({
                let path = games_cache.clone();
                std::thread::spawn(move || serde_json::from_slice(&std::fs::read(path).ok()?).ok())
            }),
            games_cache,
            games_etag: None,
            games_fresh: false,
            games_error: None,
            activity_sent: (None, None),
            music_sent: None,
            new_version: None,
            update: {
                let (tx, rx) = mpsc::channel();
                if settings_check_updates {
                    tokio::spawn(update::run(
                        auto_update,
                        data_dir.join("update"),
                        tx,
                        cc.egui_ctx.clone(),
                    ));
                }
                rx
            },
        };
        app.restart_ptt(&cc.egui_ctx);
        #[cfg(windows)]
        if app.settings.autostart {
            platform::autostart(true);
        }
        if let Some(url) = open {
            app.open_invite(&url);
        }
        app
    }

    /// (Re)starts the global push-to-talk, whisper and overlay key listener
    /// for the current binds.
    fn restart_ptt(&mut self, ctx: &egui::Context) {
        self.ptt = None;
        let talk = self.settings.audio.mode == InputMode::PushToTalk;
        let overlay = &self.settings.overlay;
        let binds: Vec<_> = talk
            .then_some((KeyAction::Talk, self.settings.ptt.into()))
            .into_iter()
            .chain(
                self.settings
                    .whisper
                    .map(|b| (KeyAction::Whisper, b.into())),
            )
            .chain(
                overlay
                    .enabled
                    .then_some((KeyAction::Overlay, overlay.toggle)),
            )
            .collect();
        if !binds.is_empty() {
            let ctx = ctx.clone();
            let wake = move || ctx.request_repaint();
            let sink = Sink::new(self.voice.clone(), self.overlay_on.clone(), wake);
            self.ptt = Some(Ptt::start(sink, binds));
        }
    }

    fn net_idx(&self, origin: &str) -> Option<usize> {
        self.view.net(origin)
    }

    /// The network's name, or its origin before discovery names it.
    fn net_name(&self, origin: &str) -> String {
        self.net_idx(origin).map_or_else(
            || origin.to_owned(),
            |i| self.view.nets[i].info.name.clone(),
        )
    }

    /// Watching a call member's public share means joining its stream's
    /// room (D87): ask once for a token, which `Event::StreamToken` uses.
    fn watch_streams(&mut self) {
        let wanted = self.voice.stream_wanted();
        if wanted.is_some()
            && wanted != self.stream_asked
            && let Some((net, _)) = self.view.call.clone()
            && let Some(room) = wanted.clone()
        {
            self.send(&net, NetCommand::StreamToken { room });
        }
        self.stream_asked = wanted;
        if !self.voice.streaming() {
            self.stream_link = None;
        }
    }

    fn send(&self, network: &str, cmd: NetCommand) {
        self.core.send(Command::Net {
            network: network.to_owned(),
            cmd,
        });
    }

    /// Read off the UI thread, then uploaded by `poll`.
    fn upload_file(
        &self,
        ctx: &egui::Context,
        network: &str,
        target: Option<String>,
        path: std::path::PathBuf,
    ) {
        let max = self.upload_limit(network);
        let (tx, ctx) = (self.files.0.clone(), ctx.clone());
        read_picked(tx, ctx, network.to_owned(), target, path, max);
    }

    fn upload_limit(&self, network: &str) -> u64 {
        self.net_idx(network)
            .and_then(|i| self.view.nets[i].info.uploads)
            .unwrap_or(0)
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

    fn toast(&mut self, text: String) {
        // The same message again just restarts its timer instead of stacking.
        self.toasts.retain(|(t, _)| *t != text);
        self.toasts.push((text, Instant::now()));
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    /// A pasted or forwarded invite: show the network if we're in it already,
    /// otherwise open "Join a network" on its sign-in step.
    fn open_invite(&mut self, url: &str) {
        match invite::parse(url) {
            Ok(inv) => {
                if let Some(i) = self.net_idx(&inv.origin) {
                    self.current = Some(inv.origin.clone());
                    if let Some(ch) = inv.channel {
                        let channel = ch.clone();
                        let cmd = if chad_core::meeting::is_meeting(&ch) {
                            NetCommand::JoinMeeting {
                                channel,
                                new: false,
                            }
                        } else {
                            NetCommand::JoinChannel { channel }
                        };
                        self.send(&inv.origin, cmd);
                        select(&mut self.view.nets[i], &key(&ch));
                    }
                    return;
                }
                // The link names the server, so go straight to asking it.
                let invite = url.trim().to_owned();
                self.core.send(Command::Discover {
                    invite: invite.clone(),
                });
                self.join = Some(JoinDialog {
                    busy: Some(invite.clone()),
                    invite,
                    ..Default::default()
                });
            }
            Err(e) => self.toast(e),
        }
    }

    /// No network signed in to yet, ever, and the first-launch cards not
    /// read this run: they come before anything else, the join dialog included.
    pub(crate) fn welcoming(&self) -> bool {
        !self.settings.welcomed && !self.welcome_done && self.view.nets.is_empty()
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        if self.welcoming() {
            return self.welcome_cards(ui);
        }
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            ui.heading("Welcome to IrChad");
            ui.label("Join a network with the link or address someone sent you.");
            ui.add_space(12.0);
            if ui
                .button(RichText::new("Join a network").size(18.0))
                .clicked()
            {
                self.join = Some(JoinDialog::default());
            }
        });
    }

    /// First launch: what a network is, one card at a time, then the join
    /// dialog (already on its sign-in step, if a link opened the app).
    fn welcome_cards(&mut self, ui: &mut egui::Ui) {
        let cards = &chad_core::welcome::CARDS;
        let last = cards.len() - 1;
        let at = self.welcome_card.min(last);
        let card = &cards[at];
        let mut done = false;
        let logo = self.logo.get_or_insert_with(|| {
            let icon = platform::window_icon();
            let size = [icon.width as usize, icon.height as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, &icon.rgba);
            ui.ctx()
                .load_texture("logo", image, egui::TextureOptions::LINEAR)
        });
        let logo = egui::Image::new((logo.id(), egui::Vec2::splat(96.0)));
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 4.0);
            ui.set_max_width(420.0);
            ui.add(logo);
            ui.add_space(12.0);
            ui.label(RichText::new(card.title).heading().size(26.0));
            ui.add_space(8.0);
            ui.label(RichText::new(card.body).size(16.0));
            ui.add_space(16.0);
            let dots: String = (0..=last)
                .map(|i| if i == at { "●" } else { "○" })
                .collect::<Vec<_>>()
                .join(" ");
            let r = ui.label(RichText::new(dots).weak());
            r.ctx.accesskit_node_builder(r.id, |n| {
                n.set_label(format!("Step {} of {}", at + 1, last + 1));
            });
            ui.add_space(16.0);
            let sel = ui.visuals().selection;
            let next = if at == last { "Get started" } else { "Next" };
            let next = egui::Button::new(RichText::new(next).size(16.0).color(sel.stroke.color))
                .fill(sel.bg_fill)
                .min_size(egui::vec2(160.0, 32.0));
            if ui.add(next).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                if at == last {
                    done = true;
                } else {
                    self.welcome_card = at + 1;
                }
            }
            ui.add_space(4.0);
            if at > 0 && ui.link("Back").clicked() {
                self.welcome_card = at - 1;
            }
            if at < last {
                let skip = RichText::new("Skip").weak().small();
                done |= ui.add(egui::Button::new(skip).frame(false)).clicked();
            }
        });
        if done {
            self.welcome_done = true;
            if self.join.is_none() {
                self.join = Some(JoinDialog::default());
            }
        }
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
        let what = if self.voice.watching_camera() {
            "camera"
        } else {
            "screen"
        };
        let who = self.voice.watching().unwrap_or_default();
        let shown = self
            .voice
            .call()
            .and_then(|c| c.participants.into_iter().find(|p| p.identity == who)?.name);
        let title = format!("{}'s {what}", shown.unwrap_or(who));
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_app_id(platform::APP_ID)
            .with_icon(platform::window_icon())
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
}

const OFF_TIP: &str = "They don't use IrChad here, so your messages aren't end-to-end encrypted.";
const ON_TIP: &str = "End-to-end encrypted: only your devices and theirs can read it. Verify to make sure nobody is in the middle.";
const VERIFIED_TIP: &str = "End-to-end encrypted, and you compared emoji with them.";
const CHANGED_TIP: &str = "One of their devices has a new key since you last talked.";

fn adding_id() -> egui::Id {
    egui::Id::unique("add-buffer")
}

fn should_notify(rule: Notify, m: &ChatMessage) -> bool {
    let wanted = match rule {
        Notify::All => true,
        Notify::MentionsAndDms => m.mention,
        Notify::Never => false,
    };
    wanted && chad_core::view::notifiable(m)
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
        .flat_map(egui::Color32::to_srgba_unmultiplied)
        .collect();
    let buf = image::RgbaImage::from_raw(u32::try_from(w).ok()?, u32::try_from(h).ok()?, raw)?;
    let mut out = Vec::new();
    buf.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// The file, if it isn't over `max` bytes (checked before reading it).
/// Reads a picked, pasted or dropped file on a blocking thread and hands it to `poll`.
fn read_picked(
    tx: mpsc::Sender<Picked>,
    ctx: egui::Context,
    network: String,
    target: Option<String>,
    path: std::path::PathBuf,
    max: u64,
) {
    tokio::task::spawn_blocking(move || {
        let name = path
            .file_name()
            .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
        let bytes = read_file(&path, max);
        let _ = tx.send((network, target, name, bytes));
        ctx.request_repaint();
    });
}

fn read_file(path: &std::path::Path, max: u64) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > max {
        return Err(format!("too big; the limit here is {} MB.", max >> 20));
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

impl eframe::App for App {
    /// Also runs while the window is hidden in the tray, so messages,
    /// notifications and "Show" still get through.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        // The overlay is drawn in `ui`, which doesn't run while the window is
        // minimized or behind the game, unless asked. The ask holds for the
        // next frame, so there has to be one: while hidden, that's 10 a second.
        if self.overlay_logic(ctx) {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            if ctx.input(|i| i.viewport().visible()) != Some(true) {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        #[cfg(windows)]
        if ctx.input(|i| i.viewport().close_requested())
            && self.settings.tray_on_close
            && self.tray.present()
            && !self.quitting
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.overlay_ui(&ctx);
        for n in &mut self.view.nets {
            n.ui.media.viewer(&ctx);
        }

        if let Some(e) = self.camera_error.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.toast(e);
        }
        if let Some(m) = self
            .stream_status
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.toast(m);
        }
        self.watch_streams();
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
                self.upload_file(&ctx, &network, Some(target.clone()), path);
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

        self.update_banner(ui);
        egui::Panel::left("rail")
            .exact_size(68.0)
            .resizable(false)
            .show(ui, |ui| self.rail(ui));

        let ni = self.current.as_deref().and_then(|c| self.net_idx(c));
        if let Some(ni) = ni {
            egui::Panel::left("channels")
                .default_size(240.0)
                .size_range(LIST_MIN_W..=400.0)
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

    /// See-through where nothing's drawn: the overlay's window. The main
    /// window is opaque and its panels cover it.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, settings::KEY, &self.settings);
    }
}
