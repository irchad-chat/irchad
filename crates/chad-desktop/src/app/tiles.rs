//! Split panes: the tile grid and pane focus.

use super::*;

impl App {
    /// Keeps `panes` in step with the selected buffer: a newly selected one
    /// opens a pane (or takes over the focused one), and gone ones close.
    pub(super) fn tile(&mut self, ctx: &egui::Context) {
        let before = (self.panes.len(), self.pane);
        let alive = |nets: &[Network], (net, k): &(String, String)| {
            nets.iter()
                .any(|n| n.info.network == *net && n.buffer(k).is_some())
        };
        let cur = self
            .current
            .as_deref()
            .and_then(|c| self.net_idx(c))
            .and_then(|i| {
                let n = &self.view.nets[i];
                let k = n.ui.selected.clone().filter(|k| n.buffer(k).is_some())?;
                Some((n.info.network.clone(), k))
            });
        let replace = std::mem::take(&mut self.replace_pane);
        if !self.settings.panes {
            self.panes = cur.into_iter().collect();
            self.pane = 0;
            return;
        }
        let focus_gone = self
            .panes
            .get(self.pane)
            .is_some_and(|p| !alive(&self.view.nets, p));
        self.panes.retain(|p| alive(&self.view.nets, p));
        self.pane = self.pane.min(self.panes.len().saturating_sub(1));
        if focus_gone && !self.panes.is_empty() {
            // Parted or closed: focus a neighbour rather than open a new pane.
            self.focus_pane(self.pane);
        } else if let Some(cur) = cur {
            if let Some(i) = self.panes.iter().position(|p| *p == cur) {
                self.pane = i;
            } else if replace && !self.panes.is_empty() {
                self.panes[self.pane] = cur;
            } else {
                self.panes.push(cur);
                self.pane = self.panes.len() - 1;
            }
        }
        if (self.panes.len(), self.pane) != before {
            ctx.request_repaint();
        }
    }

    pub(super) fn focus_pane(&mut self, i: usize) {
        let Some((net, k)) = self.panes.get(i).cloned() else {
            return;
        };
        self.pane = i;
        if let Some(ni) = self.net_idx(&net) {
            self.view.nets[ni].ui.selected = Some(k);
            self.view.nets[ni].ui.focus = true;
            self.current = Some(net);
        }
    }

    pub(super) fn close_pane(&mut self, i: usize) {
        self.panes.remove(i);
        if i == self.pane {
            self.focus_pane(i.min(self.panes.len() - 1));
        } else if i < self.pane {
            self.pane -= 1;
        }
    }

    /// The share's cell in the pane grid, when it's tiled there. The chat
    /// panes fill the other cells in order.
    pub(super) fn screen_cell(&self) -> Option<usize> {
        (self.settings.stage == Layout::Tile && self.voice.watching().is_some())
            .then(|| self.stage.cell.min(self.panes.len()))
    }

    /// A pane's title dropped on another pane: they trade places, and focus
    /// stays with its buffer. The tiled share counts as a pane here.
    pub(super) fn swap_cells(&mut self, a: usize, b: usize) {
        let focused = self.panes.get(self.pane).cloned();
        let screen = self.screen_cell();
        let mut cells: Vec<_> = std::mem::take(&mut self.panes)
            .into_iter()
            .map(Some)
            .collect();
        if let Some(s) = screen {
            cells.insert(s, None);
        }
        if a < cells.len() && b < cells.len() {
            cells.swap(a, b);
        }
        if let Some(s) = cells.iter().position(Option::is_none) {
            self.stage.cell = s;
        }
        self.panes = cells.into_iter().flatten().collect();
        self.pane = focused
            .and_then(|f| self.panes.iter().position(|p| *p == f))
            .unwrap_or(0);
    }

    /// Ctrl+arrows: the pane next to the focused one in that direction,
    /// passing over the tiled share.
    pub(super) fn step_pane(&mut self, k: Key) {
        let screen = self.screen_cell();
        let n = self.panes.len() + usize::from(screen.is_some());
        let cols = pane_cols(n);
        let rows = n.div_ceil(cols);
        let mut c = match screen {
            Some(s) if self.pane >= s => self.pane + 1,
            _ => self.pane,
        };
        loop {
            c = match k {
                Key::ArrowLeft if c % cols > 0 => c - 1,
                Key::ArrowRight if c % cols + 1 < cols && c + 1 < n => c + 1,
                Key::ArrowUp if c >= cols => c - cols,
                // The last row can be short.
                Key::ArrowDown if c / cols + 1 < rows => (c + cols).min(n - 1),
                _ => return,
            };
            if Some(c) != screen {
                break;
            }
        }
        self.focus_pane(match screen {
            Some(s) if c > s => c - 1,
            _ => c,
        });
    }

    /// The open panes in a grid, rows filled left to right; a short last row
    /// stretches. The focused pane is outlined. Dragging a pane's title onto
    /// another swaps them. A tiled share takes a cell like a chat pane.
    pub(super) fn tiles(&mut self, ui: &mut egui::Ui) {
        let screen = self.screen_cell();
        let chats = self.panes.len();
        let n = chats + usize::from(screen.is_some());
        let cols = pane_cols(n);
        let area = ui.available_rect_before_wrap();
        let h = area.height() / n.div_ceil(cols) as f32;
        let (mut close, mut focus, mut swap) = (None, None, None);
        let dragged = egui::DragAndDrop::payload::<PaneDrag>(ui.ctx()).map(|p| p.0);
        let panes = self.panes.clone();
        // A gutter between tiles, for the active outline; a lone tile meets the sidebar.
        let inset = if n > 1 { 2.0 } else { 0.0 };
        for i in 0..n {
            let (row, col) = (i / cols, i % cols);
            let w = area.width() / cols.min(n - row * cols) as f32;
            let rect = egui::Rect::from_min_size(
                area.min + vec2(col as f32 * w, row as f32 * h),
                vec2(w, h),
            );
            let p = match screen {
                Some(s) if i == s => None,
                Some(s) if i > s => Some(i - 1),
                _ => Some(i),
            };
            if let Some(p) = p {
                let (net, k) = &panes[p];
                let Some(ni) = self.net_idx(net) else {
                    continue;
                };
                let Some(bi) = self.view.nets[ni].buffer(k) else {
                    continue;
                };
                let mut pane = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink(inset))
                        .id_salt(("pane", net, k)),
                );
                pane.set_clip_rect(rect);
                let active = p == self.pane;
                if !active
                    && ui.rect_contains_pointer(rect)
                    && ui.input(|i| i.pointer.any_pressed())
                {
                    focus = Some(p);
                }
                // Shown or hidden only by the header's toggle; names truncate when it's narrow.
                let b = &self.view.nets[ni].buffers[bi];
                if !b.ui.hide_members && b.info.kind == BufferKind::Channel {
                    egui::Panel::right(egui::Id::unique(("members", net, k)))
                        .resizable(true)
                        .default_size(248.0)
                        .size_range(LIST_MIN_W..=(rect.width() / 2.0).max(LIST_MIN_W))
                        .show(&mut pane, |ui| self.members(ui, ni, bi));
                }
                if self.chat(&mut pane, ni, bi, active, (n > 1).then_some(i), chats > 1) {
                    close = Some(p);
                }
                if active && chats > 1 {
                    let stroke = egui::Stroke::new(2.0, ui.visuals().selection.bg_fill);
                    ui.painter().rect_stroke(
                        rect.shrink(1.0),
                        4.0,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            } else {
                let mut pane = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink(inset))
                        .id_salt("screen-tile"),
                );
                pane.set_clip_rect(rect);
                let ctx = ui.ctx().clone();
                self.stage_ui(&mut pane, &ctx, (n > 1).then_some(i));
            }
            if let Some(from) = dragged.filter(|&d| d != i && d < n)
                && ui.rect_contains_pointer(rect)
            {
                let fill = ui.visuals().selection.bg_fill.gamma_multiply(0.25);
                ui.painter().rect_filled(rect.shrink(2.0), 4.0, fill);
                if ui.input(|i| i.pointer.any_released()) {
                    swap = Some((from, i));
                }
            }
        }
        if let Some((a, b)) = swap {
            egui::DragAndDrop::clear_payload(ui.ctx());
            self.swap_cells(a, b);
        } else if let Some(i) = close {
            self.close_pane(i);
        } else if let Some(i) = focus {
            self.focus_pane(i);
        }
    }
}
