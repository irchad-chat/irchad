//! The picker's GIF tab: search through chadd (D30), thumbnails through the
//! network's [`Media`], and a click posts the GIF's URL as a plain message,
//! so every IRC client sees a link and IrChad shows it inline.

use chad_core::{Gif, GifPage};
use egui::{RichText, vec2};

use crate::media::Media;

/// Wait this long after typing before searching, in seconds.
const DEBOUNCE: f64 = 0.35;

#[derive(Default)]
pub struct Gifs {
    pub query: String,
    /// When `query` last changed, until it's searched for.
    edited: Option<f64>,
    /// The search `gifs` answers ("" for trending), once there is one.
    shown: Option<String>,
    /// The request on its way: (query, page token).
    pending: Option<(String, Option<String>)>,
    gifs: Vec<Gif>,
    next: Option<String>,
    error: Option<String>,
}

pub enum Action {
    /// Send `NetCommand::Gifs`.
    Fetch {
        query: String,
        pos: Option<String>,
    },
    Pick(Gif),
}

/// The search in a `/gif [search]` command, if `text` is one.
pub fn command(text: &str) -> Option<&str> {
    let text = text.trim();
    let (cmd, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    cmd.eq_ignore_ascii_case("/gif").then(|| rest.trim())
}

impl Gifs {
    /// Opens on `query`'s results (trending when empty).
    pub fn searching(query: String) -> Self {
        Self {
            query,
            ..Self::default()
        }
    }

    /// The core's answer. Stale ones (the search moved on) are dropped.
    pub fn page(&mut self, query: &str, pos: Option<&str>, result: Result<GifPage, String>) {
        let current = self
            .pending
            .as_ref()
            .is_some_and(|(q, p)| q == query && p.as_deref() == pos);
        if !current {
            return;
        }
        self.pending = None;
        match result {
            Ok(page) => {
                if pos.is_none() {
                    self.gifs.clear();
                    self.shown = Some(query.to_owned());
                }
                // Providers may repeat items across pages.
                for g in page.gifs {
                    if !self.gifs.iter().any(|h| h.id == g.id) {
                        self.gifs.push(g);
                    }
                }
                self.next = page.next;
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e);
                self.next = None;
            }
        }
    }

    fn fetch(&mut self, query: String, pos: Option<String>) -> Action {
        self.pending = Some((query.clone(), pos.clone()));
        Action::Fetch { query, pos }
    }

    /// The search box and the grid; `provider` is for the attribution.
    pub fn ui(&mut self, ui: &mut egui::Ui, media: &mut Media, provider: &str) -> Option<Action> {
        let mut action = None;
        let hint = format!("Search {provider}");
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text(&hint)
                .desired_width(f32::INFINITY),
        );
        ui.ctx()
            .accesskit_node_builder(search.id, |n| n.set_label("Search GIFs"));
        if !search.has_focus() && !search.lost_focus() {
            search.request_focus();
        }
        let now = ui.input(|i| i.time);
        if search.changed() {
            self.edited = Some(now);
        }
        let query = self.query.trim().to_owned();
        match self.edited {
            Some(t) if now - t < DEBOUNCE => {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(
                        DEBOUNCE - (now - t),
                    ));
            }
            Some(_) => {
                self.edited = None;
                let asked = self
                    .pending
                    .as_ref()
                    .map(|(q, _)| q)
                    .or(self.shown.as_ref());
                if asked != Some(&query) {
                    action = Some(self.fetch(query, None));
                }
            }
            // Trending, when the tab first opens.
            None if self.shown.is_none() && self.pending.is_none() && self.error.is_none() => {
                action = Some(self.fetch(query, None));
            }
            None => {}
        }
        if search.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && let Some(g) = self.gifs.first()
        {
            action = Some(Action::Pick(g.clone()));
        }

        let width = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        let col_w = (width - gap) / 2.0;
        // Masonry: each GIF goes under the shorter column.
        let mut cols: [(f32, Vec<(usize, f32)>); 2] = Default::default();
        for (i, g) in self.gifs.iter().enumerate() {
            let h = if g.width > 0 && g.height > 0 {
                (col_w * g.height as f32 / g.width as f32).clamp(40.0, 300.0)
            } else {
                col_w
            };
            let c = usize::from(cols[1].0 < cols[0].0);
            cols[c].0 += h + ui.spacing().item_spacing.y;
            cols[c].1.push((i, h));
        }
        let mut more = false;
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal_top(|ui| {
                    for (_, items) in &cols {
                        ui.vertical(|ui| {
                            ui.set_width(col_w);
                            for &(i, h) in items {
                                let g = &self.gifs[i];
                                let label = if g.title.is_empty() { "GIF" } else { &g.title };
                                let r = media.thumb(ui, g.preview.as_str(), vec2(col_w, h), label);
                                let r = if g.title.is_empty() {
                                    r
                                } else {
                                    r.on_hover_text(&g.title)
                                };
                                if r.clicked() {
                                    action = Some(Action::Pick(g.clone()));
                                }
                            }
                        });
                    }
                });
                if self.pending.is_some() {
                    ui.vertical_centered(egui::Ui::spinner);
                } else if let Some(e) = &self.error {
                    ui.label(RichText::new(e).weak());
                    if ui.small_button("Try again").clicked() {
                        self.error = None;
                        self.shown = None;
                    }
                } else if self.shown.is_some() && self.gifs.is_empty() {
                    ui.label(RichText::new("No GIFs found.").weak());
                } else if self.next.is_some() {
                    // Scrolled to the end: the next page.
                    let (rect, _) = ui.allocate_exact_size(vec2(width, 1.0), egui::Sense::hover());
                    more = ui.is_rect_visible(rect);
                }
            });
        if more
            && action.is_none()
            && let Some(shown) = self.shown.clone()
        {
            let pos = self.next.clone();
            action = Some(self.fetch(shown, pos));
        }
        ui.label(
            RichText::new(format!("Powered by {provider}"))
                .small()
                .weak(),
        );
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(id: &str) -> Gif {
        let u = url::Url::parse(&format!("https://media.example/{id}.gif")).unwrap();
        Gif {
            id: id.into(),
            title: String::new(),
            preview: u.clone(),
            url: u,
            width: 0,
            height: 0,
        }
    }

    fn page(ids: &[&str], next: Option<&str>) -> Result<GifPage, String> {
        Ok(GifPage {
            gifs: ids.iter().map(|i| gif(i)).collect(),
            next: next.map(str::to_owned),
        })
    }

    #[test]
    fn gif_command() {
        assert_eq!(command("/gif"), Some(""));
        assert_eq!(command("  /GIF  happy cat "), Some("happy cat"));
        assert_eq!(command("/gif\tdance"), Some("dance"));
        assert_eq!(command("/giffy"), None);
        assert_eq!(command("gif me"), None);
        assert_eq!(command("look /gif"), None);
    }

    #[test]
    fn pages_append_and_stale_answers_are_dropped() {
        let mut g = Gifs::default();
        g.fetch(String::new(), None);
        g.page("", None, page(&["a", "b"], Some("p2")));
        assert_eq!(g.shown.as_deref(), Some(""));
        g.fetch(String::new(), Some("p2".into()));
        // Repeats are skipped.
        g.page("", Some("p2"), page(&["b", "c"], None));
        let ids: Vec<_> = g.gifs.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(g.next, None);

        g.fetch("cat".into(), None);
        g.fetch("cats".into(), None);
        // The answer for "cat" is too late.
        g.page("cat", None, page(&["x"], None));
        assert_eq!(g.gifs.len(), 3);
        g.page("cats", None, page(&["y"], None));
        assert_eq!(g.shown.as_deref(), Some("cats"));
        assert_eq!(g.gifs.len(), 1);
    }
}
