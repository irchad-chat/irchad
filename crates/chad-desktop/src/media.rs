//! Images, link previews and avatars in the chat, per network. Bytes come
//! from the core (own uploads directly, anything else through chadd, D25);
//! decoding happens off the UI thread and is bounded (D14).

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::mpsc;
use std::time::Duration;

use chad_core::{LinkPreview, NetCommand};
use egui::{Color32, CornerRadius, RichText, Sense, TextureHandle, Vec2, vec2};
use image::{AnimationDecoder, ImageDecoder};
use url::Url;

use crate::chatlog::nick_color;
use crate::icons;

/// Largest image shown inline, in points.
const MAX_SHOWN: Vec2 = vec2(400.0, 300.0);
/// Decoded frames are scaled to fit this, in pixels.
const MAX_PIXELS: u32 = 800;
const MAX_FRAMES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
    /// One of this network's uploads that isn't media.
    File,
    Page,
}

/// What a link in a message is, by its path.
pub fn kind(url: &str, network: &str) -> Option<Kind> {
    let u = Url::parse(url).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let ext = u
        .path()
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let own = Url::parse(network).is_ok_and(|n| n.origin() == u.origin())
        && u.path().starts_with("/uploads/");
    Some(match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => Kind::Image,
        "mp4" | "webm" | "mov" => Kind::Video,
        _ if own => Kind::File,
        _ => Kind::Page,
    })
}

struct Frames {
    frames: Vec<(TextureHandle, f32)>,
    /// Sum of the frame delays, in seconds.
    period: f32,
    size: Vec2,
}

enum Img {
    Loading,
    Ready(Frames),
    Failed,
}

enum Prev {
    Loading,
    Ready(LinkPreview),
    Failed,
}

type Decoded = (String, Option<Vec<(egui::ColorImage, f32)>>);

pub struct Media {
    images: HashMap<String, Img>,
    previews: HashMap<String, Prev>,
    /// For the core, sent by the app after each frame.
    pub wanted: Vec<NetCommand>,
    decoded: (mpsc::Sender<Decoded>, mpsc::Receiver<Decoded>),
}

impl Default for Media {
    fn default() -> Self {
        Self {
            images: HashMap::new(),
            previews: HashMap::new(),
            wanted: Vec::new(),
            decoded: mpsc::channel(),
        }
    }
}

impl Media {
    /// The core's answer to `NetCommand::Media`.
    pub fn bytes(
        &mut self,
        url: String,
        bytes: Result<chad_core::Blob, String>,
        ctx: &egui::Context,
    ) {
        let Ok(bytes) = bytes else {
            self.images.insert(url, Img::Failed);
            return;
        };
        let (tx, ctx) = (self.decoded.0.clone(), ctx.clone());
        tokio::task::spawn_blocking(move || {
            let _ = tx.send((url, decode(&bytes.0, MAX_PIXELS)));
            ctx.request_repaint();
        });
    }

    /// Forgets failed images and previews, so they're asked for again: on
    /// (re)connecting, since the scrollback shown before that asked offline.
    pub fn retry_failed(&mut self) {
        self.images.retain(|_, i| !matches!(i, Img::Failed));
        self.previews.retain(|_, p| !matches!(p, Prev::Failed));
    }

    pub fn preview(&mut self, url: String, preview: Option<LinkPreview>) {
        let p = match preview {
            Some(p) if p.title.is_some() || p.description.is_some() => Prev::Ready(p),
            _ => Prev::Failed,
        };
        self.previews.insert(url, p);
    }

    /// Turns decoded images into textures; call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((url, frames)) = self.decoded.1.try_recv() {
            let img = match frames {
                Some(frames) if !frames.is_empty() => {
                    let size = frames[0].0.size;
                    let mut period = 0.0;
                    let frames = frames
                        .into_iter()
                        .enumerate()
                        .map(|(i, (img, delay))| {
                            period += delay;
                            let name = format!("media:{url}:{i}");
                            (ctx.load_texture(name, img, Default::default()), delay)
                        })
                        .collect();
                    Img::Ready(Frames {
                        frames,
                        period,
                        size: vec2(size[0] as f32, size[1] as f32),
                    })
                }
                _ => Img::Failed,
            };
            self.images.insert(url, img);
        }
    }

    fn want_image(&mut self, url: &str) {
        if !self.images.contains_key(url) {
            self.images.insert(url.to_owned(), Img::Loading);
            self.wanted.push(NetCommand::Media {
                url: url.to_owned(),
            });
        }
    }

    /// An inline image, or a button to load it when `auto` is off.
    pub fn image(&mut self, ui: &mut egui::Ui, url: &str, auto: bool) {
        if auto {
            self.want_image(url);
        }
        match self.images.get(url) {
            None => {
                if ui.button(format!("{} Load image", icons::EYE)).clicked() {
                    self.want_image(url);
                }
            }
            Some(Img::Loading) => {
                ui.add_sized([120.0, 80.0], egui::Spinner::new());
            }
            Some(Img::Failed) => {
                ui.hyperlink_to(RichText::new("Image couldn't be shown").weak().small(), url);
            }
            Some(Img::Ready(f)) => {
                let scale = (MAX_SHOWN.x / f.size.x)
                    .min(MAX_SHOWN.y / f.size.y)
                    .min(1.0);
                let tex = frame_at(f, ui);
                let r = ui
                    .add(
                        egui::Image::new((tex.id(), f.size * scale))
                            .corner_radius(CornerRadius::same(6))
                            .sense(Sense::click()),
                    )
                    .on_hover_text(url);
                r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, url));
                if r.clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            }
        }
    }

    /// A clickable tile of `size` points for a picker. Fetched once it's in
    /// view, whatever the auto-load setting: the user asked for these, and
    /// they come through chadd like any other image.
    pub fn thumb(
        &mut self,
        ui: &mut egui::Ui,
        url: &str,
        size: Vec2,
        label: &str,
    ) -> egui::Response {
        let (rect, r) = ui.allocate_exact_size(size, Sense::click());
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        if !ui.is_rect_visible(rect) {
            return r;
        }
        self.want_image(url);
        let radius = CornerRadius::same(4);
        match self.images.get(url) {
            Some(Img::Ready(f)) => {
                egui::Image::new((frame_at(f, ui).id(), size))
                    .corner_radius(radius)
                    .paint_at(ui, rect);
            }
            _ => {
                ui.painter()
                    .rect_filled(rect, radius, ui.visuals().faint_bg_color);
            }
        }
        if r.hovered() || r.has_focus() {
            let stroke = egui::Stroke::new(2.0, ui.visuals().selection.stroke.color);
            ui.painter()
                .rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
        }
        r
    }

    /// A card for a video or file link: there's no player, so it opens outside.
    pub fn card(ui: &mut egui::Ui, url: &str, kind: Kind) {
        let name = Url::parse(url)
            .ok()
            .and_then(|u| u.path_segments()?.next_back().map(str::to_owned))
            .unwrap_or_default();
        let icon = if kind == Kind::Video {
            icons::PLAY
        } else {
            icons::FILE
        };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).size(24.0));
                ui.hyperlink_to(name, url);
            });
        });
    }

    /// A link's title, description and image, or a button when `auto` is off.
    pub fn link_preview(&mut self, ui: &mut egui::Ui, url: &str, auto: bool) {
        let ask = |m: &mut Self| {
            m.previews.insert(url.to_owned(), Prev::Loading);
            m.wanted.push(NetCommand::Preview {
                url: url.to_owned(),
            });
        };
        if auto && !self.previews.contains_key(url) {
            ask(self);
        }
        let p = match self.previews.get(url) {
            None => {
                if ui.small_button("Show link preview").clicked() {
                    ask(self);
                }
                return;
            }
            Some(Prev::Ready(p)) => p.clone(),
            Some(Prev::Loading | Prev::Failed) => return,
        };
        let accent = Color32::from_rgb(88, 101, 242);
        egui::Frame::group(ui.style())
            .stroke(egui::Stroke::new(
                1.0,
                ui.visuals().widgets.noninteractive.bg_stroke.color,
            ))
            .show(ui, |ui| {
                ui.set_max_width(MAX_SHOWN.x);
                if let Some(site) = &p.site {
                    ui.label(RichText::new(site).small().weak());
                }
                if let Some(t) = &p.title {
                    ui.hyperlink_to(RichText::new(t).strong().color(accent), url);
                }
                if let Some(d) = &p.description {
                    ui.label(RichText::new(crate::chatlog::snippet(d, 200)).small());
                }
                if let Some(img) = &p.image {
                    self.image(ui, img.as_str(), auto);
                }
            });
    }

    /// A round avatar at `rect`: the image, or the nick's initial on its colour.
    pub fn avatar(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        nick: &str,
        url: Option<&Url>,
        auto: bool,
    ) {
        if let Some(u) = url
            && auto
        {
            self.want_image(u.as_str());
            if let Some(Img::Ready(f)) = self.images.get(u.as_str()) {
                egui::Image::new((frame_at(f, ui).id(), rect.size()))
                    .corner_radius(CornerRadius::same(255))
                    .paint_at(ui, rect);
                return;
            }
        }
        let p = ui.painter();
        p.circle_filled(
            rect.center(),
            rect.width() / 2.0,
            nick_color(nick, true).gamma_multiply(0.6),
        );
        let initial: String = nick.chars().take(1).collect::<String>().to_uppercase();
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(rect.height() * 0.5),
            Color32::WHITE,
        );
    }
}

/// The frame to show now, scheduling a repaint for the next one.
fn frame_at<'a>(f: &'a Frames, ui: &egui::Ui) -> &'a TextureHandle {
    if f.frames.len() == 1 || f.period <= 0.0 {
        return &f.frames[0].0;
    }
    let t = (ui.input(|i| i.time) as f32) % f.period;
    let mut acc = 0.0;
    for (tex, delay) in &f.frames {
        acc += delay;
        if t < acc {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f32((acc - t).max(0.01)));
            return tex;
        }
    }
    &f.frames[0].0
}

fn limits() -> image::Limits {
    let mut l = image::Limits::default();
    l.max_image_width = Some(8192);
    l.max_image_height = Some(8192);
    l.max_alloc = Some(128 << 20);
    l
}

/// Frames (just one unless it's an animated GIF) scaled to fit `max` pixels,
/// each with its delay in seconds. `None` for anything we can't or won't show.
pub fn decode(bytes: &[u8], max: u32) -> Option<Vec<(egui::ColorImage, f32)>> {
    let fit = |img: image::DynamicImage| {
        let img = if img.width() > max || img.height() > max {
            img.thumbnail(max, max)
        } else {
            img
        }
        .to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw())
    };
    let mut r = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    if r.format() == Some(image::ImageFormat::Gif) {
        let mut d = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?;
        d.set_limits(limits()).ok()?;
        let mut frames = Vec::new();
        // Each frame is a full canvas; this bounds the work and memory per GIF.
        let mut pixels: u64 = 0;
        for f in d.into_frames().take(MAX_FRAMES) {
            let f = f.ok()?;
            let (n, d) = f.delay().numer_denom_ms();
            let delay = (n as f32 / d.max(1) as f32 / 1000.0).max(0.02);
            let buf = f.into_buffer();
            pixels += u64::from(buf.width()) * u64::from(buf.height());
            if pixels > 200_000_000 {
                break;
            }
            frames.push((fit(buf.into()), delay));
        }
        return (!frames.is_empty()).then_some(frames);
    }
    r.limits(limits());
    Some(vec![(fit(r.decode().ok()?), 0.0)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        let net = "https://chat.example.com";
        assert_eq!(kind("https://x.org/a.PNG?x=1", net), Some(Kind::Image));
        assert_eq!(kind("https://x.org/v.webm", net), Some(Kind::Video));
        assert_eq!(kind("https://x.org/page", net), Some(Kind::Page));
        assert_eq!(
            kind("https://chat.example.com/uploads/ab/notes.txt", net),
            Some(Kind::File)
        );
        assert_eq!(kind("ftp://x.org/a.png", net), None);
    }

    #[test]
    fn decodes_bounded() {
        let mut png = Vec::new();
        image::RgbaImage::new(2000, 10)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let f = decode(&png, 800).unwrap();
        assert_eq!(f[0].0.size, [800, 4]);
        assert!(decode(b"not an image", 800).is_none());
    }
}
