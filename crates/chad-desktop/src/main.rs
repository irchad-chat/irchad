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
        None
    } else {
        let Some(instance) = rt.block_on(platform::single_instance(open.clone(), os_tx.clone()))
        else {
            tracing::info!("handed over to the running IrChad");
            return Ok(());
        };
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
    eframe::run_native(
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
    )
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
