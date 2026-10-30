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
