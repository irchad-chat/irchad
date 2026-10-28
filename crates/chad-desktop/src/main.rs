mod app;
mod chatlog;
mod format;
mod platform;
mod settings;

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // `irchad <url>`: the scheme handler (or a user) opening an invite.
    let open = std::env::args().nth(1).filter(|a| !a.starts_with('-'));

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = rt.enter();

    let wake = platform::Wake::default();
    let (os_tx, os_rx) = mpsc::channel();
    let os_tx = platform::OsTx::new(os_tx, wake.clone());
    // Held until exit: owning the bus name (or pipe) is what makes us the instance.
    let Some(_instance) = rt.block_on(platform::single_instance(open.clone(), os_tx.clone()))
    else {
        tracing::info!("handed over to the running IrChad");
        return Ok(());
    };
    platform::register_scheme();
    let tray = rt.block_on(platform::Tray::spawn(os_tx));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("IrChad")
            .with_app_id(platform::APP_ID)
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "IrChad",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let _ = wake.set(ctx.clone());
            let data_dir =
                eframe::storage_dir("IrChad").unwrap_or_else(|| PathBuf::from("irchad-data"));
            let (core, events) = chad_core::start(chad_core::Config {
                data_dir,
                secrets: Arc::new(chad_core::secrets::SystemSecrets),
                notify: Arc::new(move || ctx.request_repaint()),
            });
            Ok(Box::new(app::App::new(cc, core, events, os_rx, tray, open)))
        }),
    )
}
