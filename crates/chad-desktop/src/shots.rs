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
        let harness = Harness::builder()
            .with_size([1100.0, 720.0])
            // shortcut: some fields lack accessible names; re-enable once labelled.
            .with_accessibility_check(true)
            .wgpu()
            .build_eframe(move |cc| {
                // eframe adds this for the real window; kittest doesn't.
                cc.egui_ctx.add_font_provider(Arc::new(
                    egui_system_font_provider::SystemFontProvider::new(),
                ));
                let (_tx, os) = mpsc::channel();
                let voice = chad_media::Voice::new(Arc::new(|| {}));
                let (core, events) = chad_core::start(chad_core::Config {
                    data_dir,
                    secrets: Arc::new(chad_core::secrets::MemorySecrets::default()),
                    notify: Arc::new(|| {}),
                    media: Some(Arc::new(voice::Glue(voice.clone()))),
                    version: "IrChad test".into(),
                });
                App::new(cc, core, events, os, platform::Tray::none(), open, voice)
            });
        drop(_guard);
        Self {
            harness,
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

#[test]
fn join_dialog() {
    let mut s = Shots::new(Some("https://chat.localhost:8443/?channel=%23general"));
    s.harness.run();
    s.harness.get_by_label("Guest").click();
    s.shot("join_dialog_guest");
}

#[test]
fn settings() {
    let mut s = Shots::new(None);
    s.harness.run();
    s.click("Settings");
    s.shot("settings");
}

impl Shots {
    /// Clicks the widget named `label` and lets the UI react.
    pub fn click(&mut self, label: &str) {
        self.harness.get_by_label(label).click();
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
    }));
    let (g, _grx) = core(
        2,
        Auth::Guest {
            nick: guest.clone(),
        },
    );
    g.send(net(NetCommand::Send {
        target: alice.clone(),
        text: "hello from a plain IRC client".into(),
        reply_to: None,
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
    s.harness.get_by_label_contains(&bob).click();
    s.settle(1);
    s.shot("e2ee_dm_verified");
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
