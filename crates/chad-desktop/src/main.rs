// Release builds on Windows: no console window (and no stderr, so no logs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod catppuccin;
mod chatlog;
mod emoji;
mod gifs;
mod icons;
mod media;
mod overlay;
mod platform;
mod presence;
mod screen;
mod settings;
#[cfg(all(test, target_os = "linux"))]
mod shots;
mod update;
mod voice;

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result {
    // `irchad <url>`: the scheme handler (or a user) opening an invite.
    let open = std::env::args().nth(1).filter(|a| !a.starts_with('-'));

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = rt.enter();

    let wake = platform::Wake::default();
    let (os_tx, os_rx) = mpsc::channel();
    let os_tx = platform::OsTx::new(os_tx, wake.clone());
    // Dev: `IRCHAD_PROFILE=bob` runs another instance beside the normal one (to
    // test calls) with its own data and settings, and passwords kept in memory.
    let profile = std::env::var("IRCHAD_PROFILE")
        .ok()
        .filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()));
    let mut data_dir =
        eframe::storage_dir("IrChad").unwrap_or_else(|| PathBuf::from("irchad-data"));
    // Held until exit: owning the bus name (or pipe) is what makes us the instance.
    let _instance = if let Some(p) = &profile {
        data_dir = data_dir.join("profiles").join(p);
        init_logs(&data_dir);
        None
    } else {
        // Before the logs: a second launch mustn't rotate the running one's file.
        let Some(instance) = rt.block_on(platform::single_instance(open.clone(), os_tx.clone()))
        else {
            return Ok(());
        };
        init_logs(&data_dir);
        platform::register_scheme();
        Some(instance)
    };
    let tray = rt.block_on(platform::Tray::spawn(os_tx));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("IrChad")
            .with_app_id(platform::APP_ID)
            .with_icon(platform::window_icon())
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 400.0])
            // egui-wgpu gives any window a see-through backbuffer only if the
            // main one asked: the game overlay's needs it (D74). The main
            // window's panels cover it, so it stays opaque.
            .with_transparent(true),
        persistence_path: profile.is_some().then(|| data_dir.join("app.ron")),
        #[cfg(windows)]
        wgpu_options: see_through_dx12(),
        ..Default::default()
    };
    let result = eframe::run_native(
        "IrChad",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let _ = wake.set(ctx.clone());
            let repaint = ctx.clone();
            let voice = chad_media::Voice::new(Arc::new(move || repaint.request_repaint()));
            let (core, events) = chad_core::start(chad_core::Config {
                data_dir: data_dir.clone(),
                secrets: if profile.is_some() {
                    Arc::new(chad_core::secrets::MemorySecrets::default())
                } else {
                    Arc::new(chad_core::secrets::SystemSecrets)
                },
                notify: Arc::new(move || ctx.request_repaint()),
                media: Some(Arc::new(voice::Glue(voice.clone()))),
                version: format!("IrChad {}", update::version()),
            });
            Ok(Box::new(app::App::new(
                cc, core, events, os_rx, tray, open, voice, &data_dir,
            )))
        }),
    );
    // "Restart to update": let go of the single-instance name (or pipe) and
    // everything on the runtime first, or the new one hands over to us.
    if update::installed() {
        // Holds the D-Bus name on Linux; the pipe on Windows is on the runtime.
        #[allow(clippy::drop_non_drop)]
        drop(_instance);
        drop(_guard);
        rt.shutdown_timeout(std::time::Duration::from_secs(3));
        update::relaunch();
    }
    result
}

/// DX12 presenting through DirectComposition, the only way its windows can
/// be see-through (the game overlay's, D74), and DX12 only: Vulkan on Windows
/// has no see-through windows. `WGPU_BACKEND` and
/// `WGPU_DX12_PRESENTATION_SYSTEM` still override both.
#[cfg(windows)]
fn see_through_dx12() -> eframe::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuSetup, WgpuSetupCreateNew};
    use eframe::wgpu;
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    let desc = &mut setup.instance_descriptor;
    desc.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::DX12);
    desc.backend_options.dx12.presentation_system =
        wgpu::Dx12SwapchainKind::from_env().unwrap_or(wgpu::Dx12SwapchainKind::DxgiFromVisual);
    eframe::WgpuConfiguration {
        wgpu_setup: WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

/// Logs to stderr and to `irchad.log` in the data dir, where a Windows release
/// (no console) can still be troubleshot. The run before is kept as `irchad.old.log`.
fn init_logs(dir: &std::path::Path) {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let path = dir.join("irchad.log");
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::rename(&path, dir.join("irchad.old.log"));
    // shortcut: no size cap within a run (info is a few lines per reconnect); rotate by size if long uptimes grow it.
    let file = std::fs::File::create(&path).ok().map(std::sync::Mutex::new);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt = tracing_subscriber::fmt().with_env_filter(filter);
    match file {
        Some(f) => fmt
            .with_writer(std::io::stderr.and(f))
            .with_ansi(false)
            .init(),
        None => fmt.init(),
    }
}

/// Zips `irchad.log` and `irchad.old.log` from `dir` into `out`.
pub fn export_logs(dir: &std::path::Path, out: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    let file = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    for name in ["irchad.log", "irchad.old.log"] {
        // Read whole: the live log keeps growing while we copy.
        let Ok(bytes) = std::fs::read(dir.join(name)) else {
            continue;
        };
        zip.start_file(name, opts).map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}
