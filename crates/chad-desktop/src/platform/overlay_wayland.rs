//! The game overlay on Wayland (D75): a `wlr-layer-shell` surface on the
//! focused output, since an `xdg_toplevel` can't stay on top or be placed. It
//! has its own Wayland connection, egui context and egui-wgpu renderer, on
//! eframe's wgpu device. Not on GNOME, which has no layer-shell.

use std::collections::HashMap;
use std::sync::OnceLock;

use eframe::egui_wgpu::{self, RendererOptions, ScreenDescriptor, wgpu};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_compositor, wl_region, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{self, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

use crate::overlay::{Corner, MARGIN};

type Error = Box<dyn std::error::Error>;

/// Whether the compositor offers layer-shell; asked once.
pub fn available() -> bool {
    static HAS: OnceLock<bool> = OnceLock::new();
    *HAS.get_or_init(|| {
        let has = Connection::connect_to_env()
            .ok()
            .and_then(|c| registry_queue_init::<State>(&c).ok())
            .is_some_and(|(g, _)| {
                g.contents().with_list(|l| {
                    l.iter()
                        .any(|g| g.interface == ZwlrLayerShellV1::interface().name)
                })
            });
        tracing::debug!(has, "overlay: wlr-layer-shell");
        has
    })
}

/// The edges a corner's surface is anchored to.
fn anchor(c: Corner) -> Anchor {
    let x = if c.right() {
        Anchor::Right
    } else {
        Anchor::Left
    };
    let y = if c.bottom() {
        Anchor::Bottom
    } else {
        Anchor::Top
    };
    x | y
}

#[derive(Default)]
struct State {
    configured: bool,
    closed: bool,
}

/// The overlay's surface; dropping it hides it.
pub struct Layer {
    pub corner: Corner,
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    surface: wl_surface::WlSurface,
    layer: ZwlrLayerSurfaceV1,
    /// Dropped before the Wayland surface it draws to.
    wgpu: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    /// Integer buffer scale.
    scale: u32,
    /// The main renderer's textures (avatars) registered with ours, by
    /// their main id.
    imported: HashMap<egui::TextureId, egui::TextureId>,
}

impl Layer {
    /// A `size` (points) surface `MARGIN` in from `corner` of the focused
    /// output, drawn at `ppp` rounded up. `main` lends its style and fonts.
    ///
    /// `display` is eframe's: wgpu only makes surfaces on the `wl_display`
    /// its instance was made with, so this shares winit's connection, on a
    /// queue of its own.
    pub fn new(
        rs: &egui_wgpu::RenderState,
        display: wgpu::rwh::RawDisplayHandle,
        main: &egui::Context,
        corner: Corner,
        size: egui::Vec2,
        ppp: f32,
    ) -> Result<Self, Error> {
        let wgpu::rwh::RawDisplayHandle::Wayland(d) = display else {
            return Err("eframe isn't on Wayland".into());
        };
        // SAFETY: winit's display lives as long as the app, which outlives
        // every `Layer`; a foreign backend never closes it.
        let conn = Connection::from_backend(unsafe {
            wayland_client::backend::Backend::from_foreign_display(d.display.as_ptr().cast())
        });
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let (surface, layer) = make(&globals, &qh, corner, size)?;
        let scale = (ppp.ceil() as u32).clamp(1, 4);
        if surface.version() >= 3 {
            surface.set_buffer_scale(scale as i32);
        }
        surface.commit();
        let mut state = State::default();
        queue.roundtrip(&mut state)?;

        let display = conn.backend().display_ptr().cast();
        let window = surface.id().as_ptr().cast();
        let (Some(display), Some(window)) = (
            std::ptr::NonNull::new(display),
            std::ptr::NonNull::new(window),
        ) else {
            return Err("no libwayland pointers".into());
        };
        use wgpu::rwh::{
            RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
        };
        // SAFETY: the display and surface outlive the wgpu surface: `Drop`
        // drops it first, and `conn` keeps the display.
        let target = unsafe {
            rs.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                        display,
                    ))),
                    raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
                })?
        };
        let caps = target.get_capabilities(&rs.adapter);
        let format = if caps.formats.contains(&rs.target_format) {
            rs.target_format
        } else {
            *caps.formats.first().ok_or("no surface formats")?
        };
        let alpha_mode = [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
        ]
        .into_iter()
        .find(|m| caps.alpha_modes.contains(m))
        .unwrap_or(caps.alpha_modes[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: (size.x as u32) * scale,
            height: (size.y as u32) * scale,
            // Never wait on the compositor: this draws on the UI thread.
            present_mode: wgpu::PresentMode::AutoNoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        target.configure(&rs.device, &config);

        let ctx = egui::Context::default();
        crate::icons::install(&ctx);
        // What eframe gives its own context: CJK, emoji and others, on demand.
        ctx.add_font_provider(std::sync::Arc::new(
            egui_system_font_provider::SystemFontProvider::new(),
        ));
        ctx.set_global_style(main.global_style());
        let renderer = egui_wgpu::Renderer::new(&rs.device, format, RendererOptions::default());
        Ok(Self {
            corner,
            conn,
            queue,
            state,
            surface,
            layer,
            wgpu: Some(target),
            config,
            ctx,
            renderer,
            scale,
            imported: HashMap::new(),
        })
    }

    /// Whether the compositor took it away; make a new one then.
    pub fn closed(&self) -> bool {
        self.state.closed
    }

    /// The main renderer's textures `main` as ours, to draw with: each is
    /// the same GPU texture, registered once. Ones the main renderer hasn't
    /// uploaded yet are left out, and ones not asked for again are forgotten.
    pub fn import(
        &mut self,
        rs: &egui_wgpu::RenderState,
        main: &[egui::TextureId],
    ) -> &HashMap<egui::TextureId, egui::TextureId> {
        let Self {
            imported, renderer, ..
        } = self;
        imported.retain(|id, ours| {
            let keep = main.contains(id);
            if !keep {
                // Only our bind group: a registered texture isn't ours to destroy.
                renderer.free_texture(ours);
            }
            keep
        });
        let theirs = rs.renderer.read();
        for id in main {
            if imported.contains_key(id) {
                continue;
            }
            let Some(tex) = theirs.texture(id).and_then(|t| t.texture.as_ref()) else {
                continue;
            };
            let view = tex.create_view(&Default::default());
            let ours =
                renderer.register_native_texture(&rs.device, &view, wgpu::FilterMode::Linear);
            imported.insert(*id, ours);
        }
        imported
    }

    /// Handles our Wayland events, then draws `add` once the surface is
    /// configured.
    pub fn paint(&mut self, rs: &egui_wgpu::RenderState, add: impl FnMut(&mut egui::Ui)) {
        if let Err(e) = self.pump() {
            tracing::info!("overlay: Wayland: {e}");
            self.state.closed = true;
            return;
        }
        let Some(target) = self.wgpu.as_ref().filter(|_| self.state.configured) else {
            return;
        };
        let ppp = self.scale as f32;
        let size = egui::vec2(self.config.width as f32, self.config.height as f32) / ppp;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let mut input = input;
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(ppp);
        let mut out = self.ctx.run_ui(input, add);
        // Applied before a frame is acquired, which can fail: epaint panics on
        // an unapplied delta, and the font atlas would be lost. Freed textures
        // aren't in this frame's jobs, and wgpu defers destroying in-use ones.
        for (id, deltas) in out.textures_delta.set.drain() {
            for delta in &deltas {
                self.renderer
                    .update_texture(&rs.device, &rs.queue, id, delta);
            }
        }
        for id in out.textures_delta.free.drain() {
            self.renderer.free_texture(&id);
        }
        let jobs = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let screen = ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: out.pixels_per_point,
        };
        let frame = match target.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated => {
                target.configure(&rs.device, &self.config);
                return;
            }
            other => {
                tracing::debug!("overlay: no frame: {other:?}");
                return;
            }
        };
        let mut enc = rs.device.create_command_encoder(&Default::default());
        let user = self
            .renderer
            .update_buffers(&rs.device, &rs.queue, &mut enc, &jobs, &screen);
        let view = frame.texture.create_view(&Default::default());
        {
            let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        rs.queue.submit(user.into_iter().chain([enc.finish()]));
        rs.queue.present(frame);
    }

    /// Handles what winit's loop has read for us: it owns the socket, and
    /// libwayland puts our objects' events on our queue.
    fn pump(&mut self) -> Result<(), Error> {
        self.queue.dispatch_pending(&mut self.state)?;
        self.conn.flush()?;
        Ok(())
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        self.wgpu = None;
        self.layer.destroy();
        self.surface.destroy();
        let _ = self.conn.flush();
    }
}

/// The layer surface, set up but not committed: on the overlay layer (above
/// fullscreen), on the compositor's pick of output (the focused one, the
/// game's), never taking the keyboard, and with no input region, so clicks go
/// through.
///
/// shortcut: anchored to the output's corner, not the game window's, so a
/// windowed game gets the screen's corner; map `Target::rect` via xdg-output
/// when someone asks.
fn make(
    globals: &GlobalList,
    qh: &QueueHandle<State>,
    corner: Corner,
    size: egui::Vec2,
) -> Result<(wl_surface::WlSurface, ZwlrLayerSurfaceV1), Error> {
    let compositor: wl_compositor::WlCompositor = globals.bind(qh, 1..=4, ())?;
    let shell: ZwlrLayerShellV1 = globals.bind(qh, 1..=4, ())?;
    let surface = compositor.create_surface(qh, ());
    let layer = shell.get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Overlay,
        "irchad-overlay".into(),
        qh,
        (),
    );
    layer.set_size(size.x as u32, size.y as u32);
    layer.set_anchor(anchor(corner));
    let m = MARGIN as i32;
    layer.set_margin(m, m, m, m);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    let empty = compositor.create_region(qh, ());
    surface.set_input_region(Some(&empty));
    empty.destroy();
    Ok((surface, layer))
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // The size is ours: we asked for one, anchored to a corner.
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                layer.ack_configure(serial);
                state.configured = true;
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(State: wl_compositor::WlCompositor);
wayland_client::delegate_noop!(State: wl_region::WlRegion);
wayland_client::delegate_noop!(State: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(State: ZwlrLayerShellV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_anchor_to_their_edges() {
        assert_eq!(anchor(Corner::TopLeft), Anchor::Top | Anchor::Left);
        assert_eq!(anchor(Corner::TopRight), Anchor::Top | Anchor::Right);
        assert_eq!(anchor(Corner::BottomLeft), Anchor::Bottom | Anchor::Left);
        assert_eq!(anchor(Corner::BottomRight), Anchor::Bottom | Anchor::Right);
    }
}
