//! Screen share in the UI: the share dialog, the stage that shows someone's
//! share, and the wgpu paint callback that turns its I420 frames into pixels.

use std::sync::{Arc, mpsc};

use chad_media::{
    AudioApp, ContentHint, Preset, ScreenFrame, ScreenSettings, SourceId, Target, Targets, Voice,
};
use eframe::egui_wgpu::{self, wgpu};
use egui::{RichText, vec2};
use serde::{Deserialize, Serialize};

use crate::app::PaneDrag;
use crate::icons::{self, Tip};
use crate::voice;

const SHADER: &str = r"
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs(@builtin(vertex_index) i: u32) -> Out {
    // One triangle that covers the viewport egui set to our rect.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

@group(0) @binding(0) var ty: texture_2d<f32>;
@group(0) @binding(1) var tu: texture_2d<f32>;
@group(0) @binding(2) var tv: texture_2d<f32>;
@group(0) @binding(3) var s: sampler;

@fragment fn fs(in: Out) -> @location(0) vec4<f32> {
    // BT.601 limited range, what libyuv's ARGB -> I420 produced on the sender.
    let y = 1.164 * (textureSample(ty, s, in.uv).r - 0.0625);
    let u = textureSample(tu, s, in.uv).r - 0.5;
    let v = textureSample(tv, s, in.uv).r - 0.5;
    return vec4<f32>(y + 1.596 * v, y - 0.392 * u - 0.813 * v, y + 2.017 * u, 1.0);
}
";

struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

/// The planes of the frame on screen.
struct Planes {
    size: (u32, u32),
    seq: u64,
    textures: [wgpu::Texture; 3],
    bind: wgpu::BindGroup,
}

/// Sets up the I420 → RGB pipeline once, at startup.
pub fn install(cc: &eframe::CreationContext<'_>) {
    let Some(rs) = cc.wgpu_render_state.as_ref() else {
        return;
    };
    let device = &rs.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("i420"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let tex = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("i420"),
        entries: &[
            tex(0),
            tex(1),
            tex(2),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("i420"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("i420"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: rs.target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("i420"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    rs.renderer.write().callback_resources.insert(Pipeline {
        pipeline,
        layout,
        sampler,
    });
}

struct Video(Arc<ScreenFrame>);

/// (width, height, stride) per plane, if the buffers really hold them (D14:
/// the frame came from a peer's encoder).
fn planes(f: &ScreenFrame) -> Option<[(u32, u32, u32); 3]> {
    let (cw, ch) = (f.width.div_ceil(2), f.height.div_ceil(2));
    let dims = [(f.width, f.height), (cw, ch), (cw, ch)];
    let data = [&f.y, &f.u, &f.v];
    let mut out = [(0, 0, 0); 3];
    for i in 0..3 {
        let (w, h) = dims[i];
        let s = f.strides[i];
        if s < w || (s as usize) * (h as usize) > data[i].len() {
            return None;
        }
        out[i] = (w, h, s);
    }
    Some(out)
}

impl egui_wgpu::CallbackTrait for Video {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        res: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let f = &self.0;
        let Some(dims) = planes(f) else {
            return Vec::new();
        };
        let stale = res
            .get::<Planes>()
            .is_none_or(|p| p.size != (f.width, f.height));
        if stale {
            let Some(pl) = res.get::<Pipeline>() else {
                return Vec::new();
            };
            let textures = dims.map(|(w, h, _)| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("i420 plane"),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            });
            let views = textures
                .each_ref()
                .map(|t| t.create_view(&Default::default()));
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("i420"),
                layout: &pl.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&views[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&pl.sampler),
                    },
                ],
            });
            res.insert(Planes {
                size: (f.width, f.height),
                seq: 0,
                textures,
                bind,
            });
        }
        let Some(p) = res.get_mut::<Planes>() else {
            return Vec::new();
        };
        if p.seq != f.seq {
            p.seq = f.seq;
            for ((tex, data), (w, h, stride)) in p.textures.iter().zip([&f.y, &f.u, &f.v]).zip(dims)
            {
                queue.write_texture(
                    tex.as_image_copy(),
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(stride),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        res: &egui_wgpu::CallbackResources,
    ) {
        let (Some(pl), Some(p)) = (res.get::<Pipeline>(), res.get::<Planes>()) else {
            return;
        };
        pass.set_pipeline(&pl.pipeline);
        pass.set_bind_group(0, &p.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Paints the frame letterboxed into the space left in `ui`.
fn video(ui: &mut egui::Ui, frame: Arc<ScreenFrame>) {
    let avail = ui.available_rect_before_wrap();
    ui.painter().rect_filled(avail, 0.0, egui::Color32::BLACK);
    let k = (avail.width() / frame.width as f32).min(avail.height() / frame.height as f32);
    let rect = egui::Rect::from_center_size(
        avail.center(),
        vec2(frame.width as f32 * k, frame.height as f32 * k),
    );
    ui.painter()
        .add(egui_wgpu::Callback::new_paint_callback(rect, Video(frame)));
    ui.allocate_rect(avail, egui::Sense::hover());
}

pub enum StageAction {
    Close,
    Layout(Layout),
    Fullscreen(bool),
    /// The share's audio, for the settings; already applied.
    Volume(String, f32),
}

/// Where a watched share goes when it isn't full screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    /// In place of the chat.
    #[default]
    Fill,
    /// In a small window over the chat.
    Float,
    /// A pane beside the chat panes.
    Tile,
    /// In its own window.
    PopOut,
}

impl Layout {
    const ALL: [(Self, &str, &str); 4] = [
        (Self::Fill, icons::APP_WINDOW, "Fill the window"),
        (
            Self::Float,
            icons::PICTURE_IN_PICTURE,
            "Float over the chat",
        ),
        (Self::Tile, icons::SQUARES_FOUR, "Tile beside the chat"),
        (Self::PopOut, icons::ARROW_SQUARE_OUT, "Pop out"),
    ];
}

/// Someone's screen share, filling `ui`: a bar with who and how many watch,
/// then the video. `tile`: its cell when tiled, which makes its title a drag
/// handle like a chat pane's.
pub fn stage(
    ui: &mut egui::Ui,
    voice: &Voice,
    layout: Layout,
    fullscreen: bool,
    tile: Option<usize>,
) -> Option<StageAction> {
    let who = voice.watching()?;
    let camera = voice.watching_camera();
    let (watchers, own) = voice
        .call()
        .and_then(|c| c.participants.into_iter().find(|p| p.identity == who))
        .map_or((0, false), |p| (p.watchers, p.local));
    let mut action = None;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let close = if own {
                "Close preview"
            } else {
                "Stop watching"
            };
            if ui.button(icons::X).tip(close).clicked() {
                action = Some(StageAction::Close);
            }
            let (icon, tip) = if fullscreen {
                (icons::CORNERS_IN, "Exit full screen (Esc)")
            } else {
                (icons::CORNERS_OUT, "Full screen")
            };
            if ui.button(icon).tip(tip).clicked() {
                action = Some(StageAction::Fullscreen(!fullscreen));
            }
            // Our own share's sound never plays here; cameras have none.
            if !own && !camera {
                ui.menu_button(icons::SPEAKER_HIGH, |ui| {
                    let mut v = voice.screen_volume(&who);
                    let l = ui.label("Screen share volume");
                    if voice::volume_slider(ui, &mut v).labelled_by(l.id).changed() {
                        voice.set_screen_volume(&who, v);
                        action = Some(StageAction::Volume(who.clone(), v));
                    }
                })
                .response
                .tip("Volume");
            }
            if !fullscreen {
                ui.separator();
                for (l, icon, tip) in Layout::ALL.into_iter().rev() {
                    if ui.selectable_label(layout == l, icon).tip(tip).clicked() && layout != l {
                        action = Some(StageAction::Layout(l));
                    }
                }
            }
            // Whatever's left of the buttons, so a narrow tile truncates the title.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let (icon, what) = if camera {
                    (icons::VIDEO_CAMERA, "camera")
                } else {
                    (icons::SCREENCAST, "screen")
                };
                let title = if own {
                    format!("{icon} Your {what}")
                } else {
                    format!("{icon} {who}'s {what}")
                };
                let title = RichText::new(title).strong();
                match tile {
                    Some(i) => {
                        ui.dnd_drag_source(egui::Id::unique("screen-drag"), PaneDrag(i), |ui| {
                            ui.add(egui::Label::new(title).truncate())
                        })
                        .response
                        .on_hover_text("Drag onto a pane to swap them");
                    }
                    None => {
                        ui.add(egui::Label::new(title).truncate());
                    }
                }
                if !camera {
                    let watching =
                        RichText::new(format!("{} {watchers} watching", icons::EYE)).weak();
                    ui.add(egui::Label::new(watching).truncate());
                }
            });
        });
    });
    if fullscreen && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = Some(StageAction::Fullscreen(false));
    }
    match voice.screen() {
        Some(f) => video(ui, f),
        None => {
            ui.centered_and_justified(|ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Waiting for video…");
                });
            });
        }
    }
    action
}

/// The "Share your screen" dialog.
pub struct ShareDialog {
    targets: Option<Targets>,
    targets_rx: mpsc::Receiver<Targets>,
    apps: Vec<AudioApp>,
    pick: Option<Pick>,
    app: Option<String>,
    started: Option<mpsc::Receiver<Result<Option<String>, String>>>,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    Portal,
    Source(SourceId),
}

pub enum DialogOutcome {
    Open,
    Closed,
    /// Sharing; save the portal's new restore token.
    Shared(Option<String>),
}

impl ShareDialog {
    pub fn new(voice: &Voice, ctx: &egui::Context) -> Self {
        let (tx, targets_rx) = mpsc::channel();
        let (v, ctx) = (voice.clone(), ctx.clone());
        tokio::spawn(async move {
            let _ = tx.send(v.share_targets().await);
            ctx.request_repaint();
        });
        Self {
            targets: None,
            targets_rx,
            apps: voice.audio_apps(),
            pick: None,
            app: None,
            started: None,
            error: None,
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        voice: &Voice,
        settings: &mut ScreenSettings,
        restore_token: &mut Option<String>,
    ) -> DialogOutcome {
        if let Ok(t) = self.targets_rx.try_recv() {
            self.pick = t
                .portal
                .then_some(Pick::Portal)
                .or_else(|| t.sources.first().map(|s| Pick::Source(s.id)));
            self.targets = Some(t);
        }
        if let Some(rx) = &self.started
            && let Ok(r) = rx.try_recv()
        {
            self.started = None;
            match r {
                Ok(token) => return DialogOutcome::Shared(token),
                Err(e) => self.error = Some(e),
            }
        }
        let busy = self.started.is_some();
        let mut outcome = DialogOutcome::Open;
        egui::Modal::new(egui::Id::unique("share-screen")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading("Share your screen");
            ui.add_space(8.0);
            let Some(targets) = &self.targets else {
                ui.spinner();
                return;
            };
            ui.add_enabled_ui(!busy, |ui| {
                if targets.portal {
                    ui.radio_value(
                        &mut self.pick,
                        Some(Pick::Portal),
                        "Pick with the system dialog",
                    );
                    if self.pick == Some(Pick::Portal) && restore_token.is_some() {
                        ui.horizontal(|ui| {
                            ui.add_space(24.0);
                            ui.label(RichText::new("Uses your last choice.").small().weak());
                            if ui.small_button("Choose again").clicked() {
                                *restore_token = None;
                            }
                        });
                    }
                }
                if !targets.sources.is_empty() {
                    egui::ScrollArea::vertical()
                        .max_height(200.0)
                        .show(ui, |ui| {
                            for s in &targets.sources {
                                let icon = if s.id.window {
                                    icons::ARROW_SQUARE_OUT
                                } else {
                                    icons::MONITOR
                                };
                                ui.radio_value(
                                    &mut self.pick,
                                    Some(Pick::Source(s.id)),
                                    format!("{icon} {}", s.title),
                                );
                            }
                        });
                }
                if !targets.portal && targets.sources.is_empty() {
                    ui.label("Nothing here can be shared.");
                }
                ui.add_space(8.0);
                egui::Grid::new("share-options")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Sound");
                        let shown = self
                            .app
                            .as_ref()
                            .and_then(|id| self.apps.iter().find(|a| &a.id == id))
                            .map_or("No sound", |a| a.name.as_str());
                        egui::ComboBox::from_id_salt("share-app")
                            .selected_text(shown)
                            .width(260.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.app, None, "No sound");
                                for a in &self.apps {
                                    ui.selectable_value(&mut self.app, Some(a.id.clone()), &a.name);
                                }
                            })
                            .response
                            .on_hover_text(
                                "An app's sound goes out with the video. Yours never does.",
                            );
                        ui.end_row();

                        ui.label("Quality");
                        ui.horizontal(|ui| {
                            for p in [Preset::P720p30, Preset::P1080p60, Preset::Source] {
                                ui.selectable_value(&mut settings.preset, p, p.label());
                            }
                        });
                        ui.end_row();

                        ui.label("Optimize for");
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut settings.hint, ContentHint::Motion, "Motion")
                                .on_hover_text("Games and video: keeps it smooth.");
                            ui.selectable_value(&mut settings.hint, ContentHint::Detail, "Text")
                                .on_hover_text("Code and documents: keeps it sharp.");
                        });
                        ui.end_row();

                        ui.label("Bitrate cap");
                        ui.add(
                            egui::Slider::new(&mut settings.max_kbps, 1000..=20_000)
                                .suffix(" kbps")
                                .logarithmic(true),
                        );
                        ui.end_row();
                    });
            });
            if let Some(e) = &self.error {
                ui.add_space(4.0);
                ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
            }
            ui.add_space(8.0);
            {
                let target = match self.pick {
                    Some(Pick::Portal) => Some(Target::Portal {
                        restore_token: restore_token.clone(),
                    }),
                    Some(Pick::Source(id)) => Some(Target::Source(id)),
                    None => None,
                };
                let (go, cancel) = crate::app::footer(ui, "Share", !busy && target.is_some(), busy);
                if cancel {
                    outcome = DialogOutcome::Closed;
                }
                if go && let Some(target) = target {
                    let (tx, rx) = mpsc::channel();
                    let (v, s, app, ctx) = (
                        voice.clone(),
                        settings.clone(),
                        self.app.clone(),
                        ctx.clone(),
                    );
                    tokio::spawn(async move {
                        let _ = tx.send(v.share(target, app, s).await);
                        ctx.request_repaint();
                    });
                    self.started = Some(rx);
                    self.error = None;
                }
            }
        });
        outcome
    }
}
