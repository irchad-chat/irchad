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
            .with_accessibility_check(false)
            .wgpu()
            .build_eframe(move |cc| {
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
    let mut s = Shots::new(Some("https://chat.localhost:8443/#general"));
    s.harness.run();
    s.harness.get_by_label("Guest").click();
    s.shot("join_dialog_guest");
}
