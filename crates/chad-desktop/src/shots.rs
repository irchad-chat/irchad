//! Headless screenshots of the real `App`, rendered with wgpu by egui_kittest.
//!
//! `cargo test -p chad-desktop shots -- --nocapture` writes PNGs to
//! `target/shots/`. Drive the UI through accesskit (`get_by_label`, `click`,
//! `type_text`) between `run()` and `shot()`.

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use crate::{app::App, platform, voice};

/// Keeps the runtime and data dir alive as long as the harness.
pub struct Shots {
    pub harness: Harness<'static, App>,
    /// The app's audio engine, to see what it's doing.
    pub voice: chad_media::Voice,
    _rt: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
}

impl Shots {
    /// A fresh app (no networks, default settings) at the main window's size.
    /// `open` is an invite URL, as if passed on the command line.
    pub fn new(open: Option<&str>) -> Self {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let _guard = rt.enter();
        let dir = tempfile::tempdir().expect("temp dir");
        let data_dir = dir.path().to_owned();
        let open = open.map(str::to_owned);
        let voice = chad_media::Voice::new(Arc::new(|| {}));
        let app_voice = voice.clone();
        let harness = Harness::builder()
            .with_size([1100.0, 720.0])
            // Scroll-to-bottom animations take ~20 frames to settle.
            .with_max_steps(64)
            // shortcut: some fields lack accessible names; re-enable once labelled.
            .with_accessibility_check(true)
            .wgpu()
            .build_eframe(move |cc| {
                // eframe adds this for the real window; kittest doesn't.
                cc.egui_ctx.add_font_provider(Arc::new(
                    egui_system_font_provider::SystemFontProvider::new(),
                ));
                let (_tx, os) = mpsc::channel();
                let voice = app_voice;
                let (core, events) = chad_core::start(chad_core::Config {
                    data_dir: data_dir.clone(),
                    secrets: Arc::new(chad_core::secrets::MemorySecrets::default()),
                    notify: Arc::new(|| {}),
                    media: Some(Arc::new(voice::Glue(voice.clone()))),
                    version: "IrChad test".into(),
                });
                let tray = platform::Tray::none();
                App::new(cc, core, events, os, tray, open, voice, &data_dir)
            });
        drop(_guard);
        Self {
            harness,
            voice,
            _rt: rt,
            _dir: dir,
        }
    }

    /// Settles the UI, then saves `target/shots/<name>.png` and returns its path.
    pub fn shot(&mut self, name: &str) -> PathBuf {
        let _guard = self._rt.enter();
        self.harness.run();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shots dir");
        let path = dir.join(format!("{name}.png"));
        let img = self.harness.render().expect("render");
        img.save(&path).expect("save png");
        println!("shot: {}", path.display());
        path
    }
}

#[test]
fn empty() {
    let mut s = Shots::new(None);
    s.shot("empty");
}

/// The sign-in step for a server offering everything.
fn sign_in(s: &mut Shots) {
    s.harness.state_mut().shot_server(chad_core::ServerInfo {
        origin: "https://chat.localhost:8443".into(),
        name: "Chad Dev Network".into(),
        sign_up: true,
        guests: true,
        discord: true,
        classic: false,
    });
    s.harness.run();
}

/// First launch: the cards, then the join dialog.
#[test]
fn welcome() {
    let mut s = Shots::new(None);
    s.shot("welcome_1");
    s.harness.get_by_label("Next").click();
    s.shot("welcome_2");
    s.harness.get_by_label("Skip").click();
    s.harness.run();
    s.harness.get_by_label("Server address");
    assert!(s.harness.query_by_label("Next").is_none());
    // Skipped, not signed in: they're back next launch.
    assert!(!s.harness.state().settings_welcomed());
}

/// A link opening the app waits behind the cards.
#[test]
fn welcome_before_invite() {
    let mut s = Shots::new(Some("https://chat.localhost:8443/"));
    s.harness.run();
    assert!(s.harness.query_by_label("Server address").is_none());
    s.harness.get_by_label("Next");
}

#[test]
fn join_dialog() {
    let mut s = Shots::new(None);
    s.harness.state_mut().shot_join();
    s.shot("join_dialog_server");
    sign_in(&mut s);
    s.harness.get_by_label("Sign in with Discord");
    s.shot("join_dialog_login");
    s.harness.get_by_label("Sign up").click();
    s.shot("join_dialog_sign_up");
    s.harness.get_by_label("Continue as guest").click();
    s.shot("join_dialog_guest");
    s.harness.get_by_label("Change server").click();
    s.harness.run();
    s.harness.get_by_label("Server address");
}

/// A server that only takes logins shows nothing else.
#[test]
fn join_dialog_login_only() {
    let mut s = Shots::new(None);
    s.harness.state_mut().shot_server(chad_core::ServerInfo {
        origin: "https://chat.example.com".into(),
        name: "Closed Network".into(),
        sign_up: false,
        guests: false,
        discord: false,
        classic: false,
    });
    s.harness.run();
    for gone in ["Sign up", "Continue as guest", "Sign in with Discord"] {
        assert!(s.harness.query_by_label(gone).is_none(), "{gone}");
    }
    s.shot("join_dialog_login_only");
}

#[test]
fn join_dialog_discord() {
    let mut s = Shots::new(None);
    s.harness
        .state_mut()
        .shot_discord_name("https://chat.localhost:8443/", "sam_h", None);
    s.shot("discord_name");
    s.harness.state_mut().shot_discord_name(
        "https://chat.localhost:8443/",
        "nickserv",
        Some("That name is taken. Try another."),
    );
    s.shot("discord_name_taken");
}

#[test]
fn join_dialog_classic() {
    let mut s = Shots::new(None);
    s.harness.state_mut().shot_join();
    s.harness.run();
    s.harness.get_by_label(crate::app::CLASSIC_LABEL).click();
    s.harness.run();
    s.harness.get_by_label("Encrypted (TLS)").click();
    s.shot("join_dialog_classic");
}

#[test]
fn settings() {
    let mut s = Shots::new(None);
    s.harness.run();
    s.click("Settings");
    s.shot("settings");
    s.click("Mocha");
    s.shot("settings_mocha");
    s.click("Latte");
    s.shot("settings_latte");
    for page in [
        "Chat",
        "Voice",
        "Screen Sharing",
        "Game Overlay",
        "General",
        "Blocked Users",
    ] {
        s.click(page);
        s.shot(&format!(
            "settings_{}",
            page.to_lowercase().replace(' ', "_")
        ));
    }
}

/// The overlay alone in each corner, over a busy "game" so the see-through
/// backgrounds show.
#[test]
fn overlay() {
    use std::time::{Duration, Instant};

    use crate::overlay::{self, Corner, Line, View, VoiceRow};

    let row = |name: &str, speaking, muted| VoiceRow {
        nick: name.to_lowercase(),
        name: name.into(),
        speaking,
        muted,
        avatar: None,
    };
    let voice = [
        row("Alice", true, false),
        row("bob", false, true),
        row("Carol", false, false),
    ];
    let now = Instant::now();
    let line = |secs: u64, nick: &str, text: &str| Line {
        at: now - Duration::from_secs(secs),
        channel: "#games".into(),
        nick: nick.into(),
        name: nick.into(),
        text: text.into(),
        action: false,
    };
    let lines = [
        line(8, "alice", "anyone up for another round?"),
        line(5, "bob", "give me 5 minutes, finishing this match"),
        line(
            2,
            "carol",
            "the new map is wild, there's a whole secret area behind the waterfall",
        ),
        line(1, "dave", "gg"),
    ];
    for corner in Corner::ALL {
        let view = View {
            corner,
            opacity: 0.55,
            fade: Some(Duration::from_secs(9)),
            voice: &voice,
            lines: lines.iter().collect(),
        };
        // The App installs the icon font; here it lands a frame later.
        let mut fonts = false;
        let mut harness = Harness::builder()
            .with_size(overlay::SIZE)
            .wgpu()
            .build_ui(|ui| {
                if !std::mem::replace(&mut fonts, true) {
                    crate::icons::install(ui.ctx());
                    ui.ctx().request_repaint();
                    return;
                }
                // Stripes stand in for a game.
                let r = ui.max_rect();
                for i in 0..24 {
                    let x = r.left() + i as f32 * 20.0;
                    let c = if i % 2 == 0 {
                        egui::Color32::from_rgb(230, 200, 90)
                    } else {
                        egui::Color32::from_rgb(60, 120, 200)
                    };
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(x, r.top()),
                            egui::vec2(20.0, r.height()),
                        ),
                        0.0,
                        c,
                    );
                }
                overlay::ui(ui, &view, now, &mut |_, _, _| false);
            });
        harness.run();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shots dir");
        let name = format!("overlay_{}", corner.name().to_lowercase().replace(' ', "_"));
        let path = dir.join(format!("{name}.png"));
        harness
            .render()
            .expect("render")
            .save(&path)
            .expect("save png");
        println!("shot: {}", path.display());
    }
}

impl Shots {
    /// Clicks the first widget named `label` and lets the UI react. First,
    /// because a pane's title repeats its sidebar entry, which comes before it.
    pub fn click(&mut self, label: &str) {
        let node = self.harness.get_all_by_label(label).next();
        node.unwrap_or_else(|| panic!("no {label:?}")).click();
        let _guard = self._rt.enter();
        // Not `run()`: spinners and voice meters never settle.
        self.harness.run_steps(4);
    }

    pub fn type_into(&mut self, label: &str, text: &str) {
        self.harness.get_by_label(label).focus();
        self.harness.run();
        self.harness.get_by_label(label).type_text(text);
        self.harness.run();
    }

    /// Steps the UI for `secs` seconds, letting the network catch up.
    pub fn settle(&mut self, secs: u64) {
        let _guard = self._rt.enter();
        for _ in 0..secs * 20 {
            self.harness.step();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Steps the UI until `label` shows up, for at most 20s.
    pub fn wait_for(&mut self, label: &str) {
        let _guard = self._rt.enter();
        for _ in 0..400 {
            self.harness.step();
            if self.harness.query_by_label(label).is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(_guard);
        self.shot("timeout");
        panic!("timed out waiting for {label:?}");
    }
}

/// Logs in as alice and walks the main screens. Needs the dev stack (`deploy/`)
/// and Caddy's root in `SSL_CERT_FILE`; run with `-- --ignored`.
/// `IRCHAD_SHOTS_INVITE` overrides the invite, for a non-default `IRCHAD_HOST`.
#[test]
#[ignore]
fn live() {
    let invite = std::env::var("IRCHAD_SHOTS_INVITE")
        .unwrap_or("https://chat.localhost:8443/?channel=%23general".into());
    let mut s = Shots::new(Some(&invite));
    s.harness.run();
    s.type_into("Username", "alice");
    s.type_into("Password", "alicepass");
    s.click("Log in");
    s.wait_for("Your status and profile");
    s.settle(3);
    s.shot("live_general");
    s.click("Settings");
    s.shot("live_settings");
    s.click("Close window");
    s.click("Message someone");
    s.harness.run();
    s.harness.get_by_label("Settings").type_text("bo");
    s.shot("live_add_dm");
    s.harness.key_press(egui::Key::Escape);
    s.click("#dev");
    s.settle(1);
    s.shot("live_dev");
    s.harness
        .get_all_by_label("Join voice")
        .next()
        .unwrap()
        .click();
    s.settle(4);
    s.shot("live_voice");
    s.click("Disconnect");
    s.click("#general");
    s.settle(1);
    // Not `type_into`: the log is still scrolling, so `run()` wouldn't settle.
    s.harness.get_by_label("Message #general").focus();
    s.settle(1);
    s.harness.get_by_label("Message #general").type_text("hi");
    s.harness.key_press(egui::Key::Enter);
    s.settle(2);
    s.harness.get_all_by_label("hi").last().unwrap().hover();
    s.harness.run();
    s.shot("live_hover");
    s.click("Reply");
    s.shot("live_reply");
    s.harness.key_press(egui::Key::Escape);
    // `@` lists nicks to mention; Tab takes the highlighted one.
    s.harness.get_by_label("Message #general").focus();
    s.harness.run_steps(2);
    s.harness.get_by_label("Message #general").type_text("@b");
    s.harness.run_steps(4);
    s.shot("live_mention");
    s.harness.key_press(egui::Key::Tab);
    s.shot("live_mention_done");
    s.click("bob");
    s.shot("live_card");
    s.click("Send DM");
    s.settle(2);
    s.shot("live_dm");
    s.click("Network options");
    s.shot("live_menu");
    s.click("Leave network…");
    s.shot("live_leave");
    s.click("Cancel");
    s.click("Settings");
    s.click("Light");
    s.click("Close window");
    s.click("#dev");
    s.settle(1);
    s.shot("live_light");
}

/// Starting a meeting (D86): the menu item, then the meeting's row and call.
/// Same setup as `live`.
#[test]
#[ignore]
fn live_meeting() {
    let invite = std::env::var("IRCHAD_SHOTS_INVITE")
        .unwrap_or("https://chat.localhost:8443/?channel=%23general".into());
    let mut s = Shots::new(Some(&invite));
    s.harness.run();
    s.type_into("Username", "alice");
    s.type_into("Password", "alicepass");
    s.click("Log in");
    s.wait_for("Your status and profile");
    s.settle(2);
    s.click("Network options");
    s.shot("live_meeting_menu");
    s.click("New meeting");
    s.settle(4);
    s.shot("live_meeting");
    s.click("Disconnect");
    s.settle(1);
}

/// A custom emoji from `deploy/emoji` in a message and in the picker (D73).
/// Same setup as `live`.
#[test]
#[ignore]
fn live_emoji() {
    let invite = std::env::var("IRCHAD_SHOTS_INVITE")
        .unwrap_or("https://chat.localhost:8443/?channel=%23general".into());
    let mut s = Shots::new(Some(&invite));
    s.harness.run();
    s.type_into("Username", "alice");
    s.type_into("Password", "alicepass");
    s.click("Log in");
    s.wait_for("Your status and profile");
    s.settle(2);
    s.type_into("Message #general", "custom :chad: emoji");
    s.harness.key_press(egui::Key::Enter);
    s.settle(4);
    s.shot("emoji_message");
    s.click("Emoji");
    s.settle(1);
    s.shot("emoji_picker");
    s.harness.key_press(egui::Key::Escape);
    s.settle(1);
    // React with it: searching for its name and Enter picks the custom one.
    s.harness
        .get_all_by_label("custom ")
        .last()
        .unwrap()
        .hover();
    s.harness.run_steps(4);
    s.click("Add a reaction");
    s.settle(1);
    s.harness.get_by_label("Find an emoji").type_text("chad");
    s.settle(1);
    s.harness.key_press(egui::Key::Enter);
    s.settle(3);
    s.shot("emoji_reaction");
}

/// The soundboard menu in a call, listing `deploy/sounds`. Same setup as `live`.
#[test]
#[ignore]
fn live_soundboard() {
    let invite = std::env::var("IRCHAD_SHOTS_INVITE")
        .unwrap_or("https://chat.localhost:8443/?channel=%23dev".into());
    let mut s = Shots::new(Some(&invite));
    s.harness.run();
    s.type_into("Username", "alice");
    s.type_into("Password", "alicepass");
    s.click("Log in");
    s.wait_for("Your status and profile");
    s.settle(3);
    s.harness
        .get_all_by_label("Join voice")
        .next()
        .unwrap()
        .click();
    s.settle(4);
    s.click("Soundboard");
    s.settle(2);
    s.shot("live_soundboard");
    s.click("ding");
    s.settle(1);
    s.harness.key_press(egui::Key::Escape);
    s.click("Disconnect");
}

/// The whisper list (D71): bob joins #dev's call as a raw LiveKit participant,
/// alice right-clicks him and whispers to him. Same setup as `live`, plus
/// `LIVEKIT_URL` (default `wss://localhost:8443/livekit`).
#[test]
#[ignore]
fn live_whisper() {
    use livekit_api::access_token::{AccessToken, VideoGrants};
    let invite = std::env::var("IRCHAD_SHOTS_INVITE")
        .unwrap_or("https://chat.localhost:8443/?channel=%23dev".into());
    let url = std::env::var("LIVEKIT_URL").unwrap_or("wss://localhost:8443/livekit".into());
    let mut s = Shots::new(Some(&invite));
    s.harness.run();
    s.type_into("Username", "alice");
    s.type_into("Password", "alicepass");
    s.click("Log in");
    s.wait_for("Your status and profile");
    s.settle(3);
    s.harness
        .get_all_by_label("Join voice")
        .next()
        .unwrap()
        .click();
    s.settle(4);
    // Whichever channel's voice that was.
    let room = s.voice.call().expect("in a call").room;
    // Dev credentials from deploy/livekit.yaml.
    let token = AccessToken::with_api_key("devkey", "chad-dev-livekit-secret-0123456789abcdef")
        .with_identity("bob")
        .with_grants(VideoGrants {
            room_join: true,
            room,
            ..Default::default()
        })
        .to_jwt()
        .unwrap();
    let (bob, _events) = s
        ._rt
        .block_on(livekit::Room::connect(&url, &token, Default::default()))
        .expect("bob joins");
    s.settle(3);
    // The call's row (bob shows muted: he sends no mic), above the nick list's.
    s.harness
        .get_all_by_label_contains("bob")
        .next()
        .unwrap()
        .click_secondary();
    s.settle(1);
    s.shot("live_whisper_menu");
    s.click("Whisper to bob");
    s.harness.key_press(egui::Key::Escape);
    s.settle(2);
    s.shot("live_whisper_listed");
    s.click("Disconnect");
    s._rt.block_on(bob.close()).ok();
}

/// Split panes: #general and #dev side by side, Ctrl+Left back to #general,
/// #general dragged onto #dev to swap them, then the right one closed. Same setup as `live`.
#[test]
#[ignore]
fn live_panes() {
    let invite = std::env::var("IRCHAD_SHOTS_INVITE")
        .unwrap_or("https://chat.localhost:8443/?channel=%23general".into());
    let mut s = Shots::new(Some(&invite));
    s.harness.run();
    s.type_into("Username", "alice");
    s.type_into("Password", "alicepass");
    s.click("Log in");
    s.wait_for("Your status and profile");
    s.settle(2);
    s.click("#dev");
    s.settle(1);
    s.shot("panes_two");
    // Each pane toggles its own member list; this hides the first's.
    s.click("Hide member list");
    s.settle(1);
    s.shot("panes_no_members");
    s.click("Show member list");
    s.settle(1);
    s.harness
        .key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::ArrowLeft);
    s.settle(1);
    s.shot("panes_focus_left");
    // The pane titles are the rightmost nodes with the buffer's name; the
    // channel list has the others.
    let title = |s: &Shots, name: &str| {
        s.harness
            .get_all_by_label(name)
            .map(|n| n.rect().center())
            .max_by(|a, b| a.x.total_cmp(&b.x))
            .unwrap()
    };
    let (from, to) = (title(&s, "#general"), title(&s, "#dev"));
    s.harness.hover_at(from);
    s.harness.drag_at(from);
    s.harness.run_steps(2);
    for t in [0.1, 0.5, 1.0] {
        s.harness.hover_at(from.lerp(to, t));
        s.harness.run_steps(2);
    }
    s.shot("panes_dragging");
    s.harness.drop_at(to);
    s.settle(1);
    s.shot("panes_swapped");
    s.harness
        .get_all_by_label("Close pane")
        .last()
        .unwrap()
        .click();
    s.settle(1);
    s.shot("panes_closed");
}

/// Phase 5: an inline image and a link preview, the profile dialog, the
/// quick switcher and the emoji picker. Same setup as `live`; set
/// `IRCHAD_SHOTS_IMAGE` to an uploaded image's URL.
#[test]
#[ignore]
fn live_polish() {
    let invite = std::env::var("IRCHAD_SHOTS_INVITE")
        .unwrap_or("https://chat.localhost:8443/?channel=%23general".into());
    let image = std::env::var("IRCHAD_SHOTS_IMAGE").unwrap_or_default();
    let mut s = Shots::new(Some(&invite));
    s.harness.run();
    s.type_into("Username", "alice");
    s.type_into("Password", "alicepass");
    s.click("Log in");
    s.wait_for("Your status and profile");
    s.settle(2);
    s.type_into(
        "Message #general",
        &format!("look {image} and https://www.rust-lang.org/"),
    );
    s.harness.key_press(egui::Key::Enter);
    s.settle(5);
    s.shot("polish_media");
    s.click("Emoji");
    s.shot("polish_emoji");
    s.harness.key_press(egui::Key::Escape);
    s.click("Your status and profile");
    s.click("\u{E4C4} Edit profile…");
    s.settle(1);
    s.shot("polish_profile");
    s.type_into("Display name", "Alice A.");
    s.click("Save");
    s.settle(1);
    s.type_into("Message #general", "now with a name");
    s.harness.key_press(egui::Key::Enter);
    s.settle(2);
    s.shot("polish_named");
    // Up in the empty composer edits that last message.
    s.harness.key_press(egui::Key::ArrowUp);
    s.harness.run_steps(4);
    s.wait_for("Editing your message");
    s.harness
        .get_by_label("Message #general")
        .type_text(", edited");
    s.harness.run();
    s.harness.key_press(egui::Key::Enter);
    s.wait_for("now with a name, edited");
    s.wait_for("(edited)");
    s.shot("polish_edited");
    s.harness
        .key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    s.harness.run();
    s.harness
        .get_by_label("Where to? Channel or person")
        .type_text("de");
    s.harness.run();
    s.shot("polish_switcher");
}

/// Phase 4: an encrypted DM, a plain one, and emoji verification, with a bare
/// core playing bob. Same setup as `live`; fresh accounts every run.
#[test]
#[ignore]
fn live_e2ee() {
    use chad_core::{Auth, Command, ConnState, Event, NetCommand, VerifyState, VerifyStep};
    let invite = std::env::var("IRCHAD_SHOTS_INVITE")
        .unwrap_or("https://chat.localhost:8443/?channel=%23general".into());
    let origin = chad_core::invite::parse(&invite).unwrap().origin;
    let stamp = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
    let (alice, bob, guest) = (
        format!("sa{stamp}"),
        format!("sb{stamp}"),
        format!("sg{stamp}"),
    );
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dirs = [(); 3].map(|()| tempfile::tempdir().unwrap());
    let core = |i: usize, auth: Auth| {
        let _guard = rt.enter();
        let (core, mut rx) = chad_core::start(chad_core::Config {
            data_dir: dirs[i].path().to_owned(),
            secrets: Arc::new(chad_core::secrets::MemorySecrets::default()),
            notify: Arc::new(|| {}),
            media: None,
            version: "IrChad test".into(),
        });
        core.send(Command::Join {
            invite: origin.clone(),
            auth,
        });
        rt.block_on(next(&mut rx, |e| match e {
            Event::Network(n) => (n.state == ConnState::Connected).then_some(()),
            _ => None,
        }));
        (core, rx)
    };
    let register = |account: &str| Auth::Register {
        account: account.into(),
        password: "shotspass".into(),
        email: None,
    };
    drop(core(0, register(&alice)));
    let (b, mut brx) = core(1, register(&bob));
    let net = |cmd| Command::Net {
        network: origin.clone(),
        cmd,
    };

    let mut s = Shots::new(Some(&invite));
    s.harness.run();
    s.type_into("Username", &alice);
    s.type_into("Password", "shotspass");
    s.click("Log in");
    s.wait_for("Your status and profile");
    s.settle(2);
    b.send(net(NetCommand::Send {
        target: alice.clone(),
        text: "hi! only our devices can read this".into(),
        reply_to: None,
        mentions: Vec::new(),
    }));
    let (g, _grx) = core(
        2,
        Auth::Guest {
            nick: guest.clone(),
        },
    );
    g.send(net(NetCommand::Send {
        target: alice.clone(),
        // As the composer sends it: markdown becomes IRC codes.
        text: chad_core::format::markdown(
            "hello from a **plain** *IRC* client, `inline` and:\n```rust\nfn main() {\n\n    println!(\"*hi*\");\n}\n```\nafter",
        ),
        reply_to: None,
        mentions: Vec::new(),
    }));
    // bob is in #general too: his name there opens the DM.
    s.wait_for(&bob);
    s.click(&bob);
    s.wait_for("hi! only our devices can read this");
    s.shot("e2ee_dm");
    s.harness.get_by_label_contains(&guest).click();
    s.settle(1);
    s.shot("e2ee_plain");

    b.send(net(NetCommand::Verification {
        nick: alice.clone(),
        step: VerifyStep::Start,
    }));
    s.wait_for("Start");
    s.shot("e2ee_verify_request");
    s.click("Start");
    s.wait_for("They match");
    s.shot("e2ee_verify_emoji");
    rt.block_on(next(&mut brx, |e| {
        matches!(
            e,
            Event::Verification {
                state: VerifyState::Compare(_),
                ..
            }
        )
        .then_some(())
    }));
    b.send(net(NetCommand::Verification {
        nick: alice.clone(),
        step: VerifyStep::Match,
    }));
    s.click("They match");
    s.wait_for("Done");
    s.shot("e2ee_verified");
    s.click("Done");
    // The sidebar's entry; the DM pane's title has the name too.
    s.harness
        .get_all_by_label_contains(&bob)
        .next()
        .unwrap()
        .click();
    s.settle(1);
    s.shot("e2ee_dm_verified");
}

/// Read receipts (D68): alice turns them on in her profile, bob (a bare core)
/// has them on and reads her DM. Same setup as `live`; fresh accounts every run.
#[test]
#[ignore]
fn live_receipts() {
    use chad_core::{Auth, Command, ConnState, Event, NetCommand};
    let invite = std::env::var("IRCHAD_SHOTS_INVITE")
        .unwrap_or("https://chat.localhost:8443/?channel=%23general".into());
    let origin = chad_core::invite::parse(&invite).unwrap().origin;
    let stamp = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
    let (alice, bob) = (format!("ra{stamp}"), format!("rb{stamp}"));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dirs = [(); 2].map(|()| tempfile::tempdir().unwrap());
    let core = |i: usize, account: &str| {
        let _guard = rt.enter();
        let (core, mut rx) = chad_core::start(chad_core::Config {
            data_dir: dirs[i].path().to_owned(),
            secrets: Arc::new(chad_core::secrets::MemorySecrets::default()),
            notify: Arc::new(|| {}),
            media: None,
            version: "IrChad test".into(),
        });
        core.send(Command::Join {
            invite: origin.clone(),
            auth: Auth::Register {
                account: account.into(),
                password: "shotspass".into(),
                email: None,
            },
        });
        rt.block_on(next(&mut rx, |e| match e {
            Event::Network(n) => (n.state == ConnState::Connected).then_some(()),
            _ => None,
        }));
        (core, rx)
    };
    drop(core(0, &alice));
    let (b, mut brx) = core(1, &bob);
    let net = |cmd| Command::Net {
        network: origin.clone(),
        cmd,
    };
    b.send(net(NetCommand::SetReceipts { on: true }));

    let mut s = Shots::new(Some(&invite));
    s.harness.run();
    s.type_into("Username", &alice);
    s.type_into("Password", "shotspass");
    s.click("Log in");
    s.wait_for("Your status and profile");
    s.settle(2);
    s.click("Your status and profile");
    s.harness.get_by_label_contains("Edit profile").click();
    s.harness.run_steps(4);
    s.click("Read receipts in DMs");
    s.shot("receipts_profile");
    s.click("Save");
    s.settle(1);

    b.send(net(NetCommand::Send {
        target: alice.clone(),
        text: "did you get this?".into(),
        reply_to: None,
        mentions: Vec::new(),
    }));
    s.wait_for("did you get this?");
    s.harness
        .get_all_by_label_contains(&bob)
        .next()
        .unwrap()
        .click();
    s.settle(1);
    let composer = format!("Message {bob}");
    s.harness.get_by_label(&composer).focus();
    s.settle(1);
    s.harness.get_by_label(&composer).type_text("yes!");
    s.harness.key_press(egui::Key::Enter);
    rt.block_on(next(&mut brx, |e| match e {
        Event::Messages { messages, .. } => messages.iter().any(|m| m.text == "yes!").then_some(()),
        _ => None,
    }));
    b.send(net(NetCommand::MarkRead {
        target: alice.clone(),
    }));
    s.wait_for(&format!("{} Seen", crate::icons::EYE));
    s.shot("receipts_seen");
}

/// The first event `f` picks out, within 30s.
async fn next<T>(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<chad_core::Event>,
    mut f: impl FnMut(&chad_core::Event) -> Option<T>,
) -> T {
    let wait = async {
        loop {
            if let Some(t) = f(&rx.recv().await.expect("core stopped")) {
                return t;
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), wait)
        .await
        .expect("timed out")
}
