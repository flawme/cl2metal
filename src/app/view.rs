//! Drawing: the startup overlay, the tab strip, the terminal surface, and
//! the modal overlays.

use super::*;

impl App {
    /// Loading progress shown by the startup overlay: the window is up, the
    /// wallpaper is settled, the shell is spawned, the first output has
    /// arrived. The overlay stays until all of it is done, so nothing pops in
    /// after the terminal appears.
    pub(crate) fn load_target(&self) -> f32 {
        let mut p = 0.10;
        if self.background_settled() {
            p += 0.35;
        }
        if !self.tabs.is_empty() {
            p += 0.20;
        }
        if self.ready_at.is_some() {
            p += 0.35;
        }
        p
    }

    /// True once the wallpaper question is answered: loaded, absent, or failed.
    pub(crate) fn background_settled(&self) -> bool {
        self.background.is_some()
            || (self.background_rx.is_none() && self.background_loading.is_none())
    }

    /// How strongly the wallpaper shows during the startup overlay.
    pub(crate) fn background_fade(&self) -> f32 {
        match self.background_at {
            Some(at) => (((self.now - at) / 0.45) as f32).clamp(0.0, 1.0),
            None => 0.0,
        }
    }

    /// How opaque the startup curtain is. With a wallpaper behind it the
    /// loading screen shows the image instead of a flat colour.
    pub(crate) fn curtain_alpha(&self) -> f32 {
        if self.background.is_some() { 0.55 } else { 1.0 }
    }

    /// The startup overlay: the wordmark plus a progress bar that fills with
    /// the real loading progress, like Tabby's preload screen.
    pub(crate) fn draw_entry_animation(&self, ui: &egui::Ui, rect: egui::Rect) {
        if self.entry_t >= 1.0 {
            return;
        }
        let t = self.entry_t;
        let fade = 1.0 - t;
        let painter = ui.painter();
        let center = egui::pos2(rect.center().x, rect.center().y - 10.0 * (1.0 - t));
        let label = "cl2metal";
        let font = egui::FontId::new(18.0, egui::FontFamily::Monospace);
        let color = ui::with_alpha(color32(self.theme.ui.text), 0.82 * fade);
        painter.text(center, egui::Align2::CENTER_CENTER, label, font, color);

        // Progress bar under the wordmark, filling with the real load.
        let track_w = 180.0;
        let track = egui::Rect::from_min_size(
            egui::pos2(center.x - track_w * 0.5, center.y + 16.0),
            egui::vec2(track_w, 3.0),
        );
        painter.rect_filled(
            track,
            1.5,
            ui::with_alpha(color32(self.theme.ui.border), 0.9 * fade),
        );
        let filled = track_w * self.load_progress.clamp(0.0, 1.0);
        if filled > 0.5 {
            let bar = egui::Rect::from_min_size(track.min, egui::vec2(filled, 3.0));
            painter.rect_filled(
                bar,
                1.5,
                ui::with_alpha(color32(self.theme.ui.accent), 0.95 * fade),
            );
        }
    }

    pub(crate) fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();
        let active = self.active;

        // Stamp tabs that have not been drawn yet: the first frame after a tab
        // opens is what starts its entry animation.
        let now = self.now;
        for tab in &mut self.tabs {
            if tab.opened_at.is_none() {
                tab.opened_at = Some(now);
            }
        }

        // Tab state.
        let mut select: Option<usize> = None;
        let mut close: Option<usize> = None;
        let mut rename: Option<usize> = None;
        let mut drag_from: Option<usize> = None;
        let mut drag_pos: Option<egui::Pos2> = None;
        let mut tab_rects: Vec<egui::Rect> = Vec::new();
        let titles: Vec<String> = self
            .tabs
            .iter()
            .map(|t| t.display_title().to_owned())
            .collect();
        let statuses: Vec<(bool, bool, bool, bool, bool)> = self
            .tabs
            .iter()
            .map(|t| {
                (
                    t.kind == TabKind::Ssh,
                    t.ready,
                    t.dead.is_some(),
                    // Shell integration: last command failed.
                    t.core.last_exit().is_some_and(|code| code != 0),
                    // Output arrived while the tab was in the background.
                    t.unread,
                )
            })
            .collect();
        let progress: Vec<(Option<u8>, bool)> = self
            .tabs
            .iter()
            .map(|t| (t.core.progress(), t.core.progress_is_error()))
            .collect();
        // Entry/leave animation state per tab.
        let anim: Vec<(Option<f64>, Option<f64>)> = self
            .tabs
            .iter()
            .map(|t| (t.opened_at, t.closing_at))
            .collect();

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;

            // Reserve room for the right-hand actions, then hand the rest to the
            // scrollable tab strip so many tabs never clip.
            let icons_w = 30.0 * 4.0 + 6.0 * 3.0 + 10.0;
            let strip_w = (ui.available_width() - icons_w).max(60.0);
            ui.allocate_ui_with_layout(
                egui::vec2(strip_w, 30.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt("tab_strip")
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            // Tabs, left to right.
                            for i in 0..titles.len() {
                                let title = &titles[i];
                                let (is_ssh, ready, dead, failed, unread) = statuses[i];
                                let (progress, progress_err) = progress[i];
                                let (opened_at, closing_at) = anim[i];
                                let closing = closing_at.is_some();
                                let is_active = i == active && !closing;
                                let label = truncate_label(title, 24);
                                let galley = ui.painter().layout_no_wrap(
                                    label.clone(),
                                    egui::FontId::new(12.5, egui::FontFamily::Proportional),
                                    color32(theme.ui.tab_inactive_fg),
                                );
                                // Tabby's tab transition: the header grows from
                                // 1px to its full width on open, and shrinks to
                                // nothing on close. The content is clipped by the
                                // box, so it is revealed as the tab grows.
                                let full_w = (galley.size().x + 56.0).max(112.0);
                                let w = match closing_at {
                                    Some(at) => tab_leave_width(
                                        full_w,
                                        ease_in_out_cubic(((self.now - at) / TAB_ANIM_SECS) as f32),
                                    ),
                                    None => tab_enter_width(
                                        full_w,
                                        ease_out_cubic(
                                            ((self.now - opened_at.unwrap_or(self.now))
                                                / TAB_ANIM_SECS)
                                                as f32,
                                        ),
                                    ),
                                };
                                let h = 30.0;
                                let sense = if closing {
                                    egui::Sense::hover()
                                } else {
                                    egui::Sense::click_and_drag()
                                };
                                let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), sense);
                                tab_rects.push(rect);

                                let hovered = resp.hovered();
                                let hover_t = ui.ctx().animate_bool_with_time(
                                    resp.id.with("hover"),
                                    hovered && !is_active && !closing,
                                    0.12,
                                );
                                let title_color = if is_active {
                                    color32(theme.ui.text)
                                } else {
                                    ui::mix(
                                        color32(theme.ui.tab_inactive_fg),
                                        color32(theme.ui.text),
                                        hover_t,
                                    )
                                };
                                let painter = ui.painter_at(rect);
                                if is_active {
                                    // The active tab deliberately has no fill or stroke. It
                                    // sits directly on the shared wallpaper; its accent line,
                                    // title, status, and close control carry the active state.
                                    let line = egui::Rect::from_min_max(
                                        egui::pos2(rect.min.x + 12.0, rect.max.y - 2.0),
                                        egui::pos2(rect.min.x + 30.0, rect.max.y),
                                    );
                                    painter.rect_filled(line, 1.0, color32(theme.ui.accent));
                                } else if hover_t > 0.0 {
                                    painter.rect_filled(
                                        rect,
                                        8.0,
                                        ui::with_alpha(color32(theme.ui.tab_hover_bg), hover_t),
                                    );
                                }

                                // Status dot.
                                let dot_rgb = if dead || failed {
                                    theme.ui.danger
                                } else if is_ssh && !ready {
                                    theme.ui.accent
                                } else if is_ssh {
                                    theme.ui.success
                                } else {
                                    theme.ui.text_dim
                                };
                                let dot_center = egui::pos2(rect.min.x + 14.0, rect.center().y);
                                let dot_alpha = if is_ssh && !ready && !dead { 140 } else { 255 };
                                let dot = egui::Color32::from_rgba_unmultiplied(
                                    dot_rgb[0], dot_rgb[1], dot_rgb[2], dot_alpha,
                                );
                                painter.circle_filled(dot_center, 3.5, dot);

                                // Unread output: a small badge in the corner,
                                // so a background tab that produced something
                                // is visible at a glance.
                                if unread && !closing {
                                    painter.circle_filled(
                                        egui::pos2(rect.max.x - 12.0, rect.min.y + 7.0),
                                        3.0,
                                        color32(theme.ui.accent),
                                    );
                                }

                                // Title.
                                painter.galley(
                                    egui::pos2(
                                        rect.min.x + 25.0,
                                        rect.center().y - galley.size().y * 0.5,
                                    ),
                                    galley.clone(),
                                    title_color,
                                );

                                // OSC 9;4 progress: a thin bar along the bottom.
                                if let Some(percent) = progress {
                                    let bar = egui::Rect::from_min_max(
                                        egui::pos2(rect.min.x + 6.0, rect.max.y - 2.5),
                                        egui::pos2(
                                            rect.min.x
                                                + 6.0
                                                + (rect.width() - 12.0) * (percent as f32 / 100.0),
                                            rect.max.y - 1.0,
                                        ),
                                    );
                                    let color = if progress_err {
                                        theme.ui.danger
                                    } else {
                                        theme.ui.accent
                                    };
                                    painter.rect_filled(bar, 1.0, color32(color));
                                }

                                // Close button (on hover or when active).
                                let close_rect = egui::Rect::from_min_size(
                                    egui::pos2(rect.max.x - 22.0, rect.center().y - 8.0),
                                    egui::vec2(16.0, 16.0),
                                );
                                let close_hovered = hovered
                                    && !closing
                                    && resp
                                        .hover_pos()
                                        .is_some_and(|p| close_rect.expand(3.0).contains(p));
                                if (hovered || is_active) && !closing {
                                    if close_hovered {
                                        painter.circle_filled(
                                            close_rect.center(),
                                            8.0,
                                            ui::with_alpha(color32(theme.ui.danger), 0.18),
                                        );
                                    }
                                    let close_color = if close_hovered {
                                        color32(theme.ui.danger)
                                    } else if hovered {
                                        color32(theme.ui.text)
                                    } else {
                                        color32(theme.ui.text_dim)
                                    };
                                    painter.line_segment(
                                        [
                                            close_rect.min + egui::vec2(4.5, 4.5),
                                            close_rect.max - egui::vec2(4.5, 4.5),
                                        ],
                                        egui::Stroke::new(1.5, close_color),
                                    );
                                    painter.line_segment(
                                        [
                                            egui::pos2(
                                                close_rect.min.x + 4.5,
                                                close_rect.max.y - 4.5,
                                            ),
                                            egui::pos2(
                                                close_rect.max.x - 4.5,
                                                close_rect.min.y + 4.5,
                                            ),
                                        ],
                                        egui::Stroke::new(1.5, close_color),
                                    );
                                }

                                if resp.clicked()
                                    && let Some(p) = resp.interact_pointer_pos()
                                {
                                    if close_rect.contains(p) {
                                        close = Some(i);
                                    } else {
                                        select = Some(i);
                                    }
                                }
                                // Middle-click closes the tab, like a browser.
                                // It goes through the same close path, so a
                                // running process still asks first.
                                if resp.clicked_by(egui::PointerButton::Middle) {
                                    close = Some(i);
                                }
                                if resp.double_clicked() {
                                    rename = Some(i);
                                }
                                if resp.drag_stopped() {
                                    drag_from = Some(i);
                                    drag_pos = resp.interact_pointer_pos();
                                }
                                resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                            }
                        });
                },
            );

            // Right-aligned actions.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if ui::icon_button(ui, &theme, ui::Icon::Settings, "Settings (Ctrl+Shift+,)")
                    .clicked()
                {
                    let draft = self.config.clone();
                    let font = self.config.font.clone();
                    self.overlay = Overlay::Settings {
                        original: draft.clone(),
                        original_font: font.clone(),
                        draft,
                        font,
                        error: None,
                        known_hosts: crate::config::known_hosts::list(),
                    };
                }
                if ui::icon_button(ui, &theme, ui::Icon::Add, "New terminal (Ctrl+Shift+T)")
                    .clicked()
                {
                    self.spawn_local();
                }
                // The editor and the SSH workspace, with the active one lit.
                let editor_active = self.view == View::Editor;
                if ui::icon_button_tinted(
                    ui,
                    &theme,
                    ui::Icon::Editor,
                    "Editor (Ctrl+Shift+E)",
                    editor_active,
                )
                .clicked()
                {
                    self.set_view(if editor_active {
                        View::Terminal
                    } else {
                        View::Editor
                    });
                }
                if ui::icon_button_tinted(
                    ui,
                    &theme,
                    ui::Icon::Ssh,
                    "SSH workspace (Ctrl+Shift+S)",
                    self.ssh_mode,
                )
                .clicked()
                {
                    self.toggle_ssh_mode();
                }
            });
        });

        if let Some(i) = select {
            self.active = i;
        }
        if let Some(i) = close {
            let ctx = ui.ctx().clone();
            self.close_tab(i, &ctx, "tab close button");
        }
        if let Some(i) = rename {
            let text = self
                .tabs
                .get(i)
                .map(|t| t.display_title().to_owned())
                .unwrap_or_default();
            self.overlay = Overlay::Rename { tab: i, text };
        }
        // Drag-to-reorder: drop the tab where the pointer was released.
        if let Some(from) = drag_from
            && let Some(pos) = drag_pos
        {
            let target = tab_rects
                .iter()
                .enumerate()
                .filter(|(_, r)| r.center().x <= pos.x)
                .map(|(i, _)| i)
                .next_back()
                .unwrap_or(0);
            if target != from && from < self.tabs.len() {
                let tab = self.tabs.remove(from);
                let target = target.min(self.tabs.len());
                self.tabs.insert(target, tab);
                // Keep the same session selected after the move.
                if self.active == from {
                    self.active = target;
                } else if from < self.active && target >= self.active {
                    self.active -= 1;
                } else if from > self.active && target <= self.active {
                    self.active += 1;
                }
            }
        }
    }

    /// The status toast, drawn on the foreground layer.
    ///
    /// It is deliberately not part of any panel: a message has to be visible
    /// whichever surface is in front and whatever is drawn over it.
    pub(crate) fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((msg, at)) = self.toast.clone() else {
            return;
        };
        let age = self.now - at;
        if age > 8.0 {
            self.toast = None;
            return;
        }
        // Fade in quickly, fade out over the last half second.
        let fade = ((age / 0.18).min(1.0) * ((8.0 - age) / 0.5).min(1.0)).clamp(0.0, 1.0) as f32;
        let full = ctx.content_rect();
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("cl2metal-toast"));
        let painter = ctx.layer_painter(layer);
        let toast_color = ui::with_alpha(color32(self.theme.ui.danger), fade);
        let galley = painter.layout_no_wrap(
            msg,
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
            toast_color,
        );
        let pad = egui::vec2(10.0, 6.0);
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                full.max.x - galley.size().x - 12.0 - pad.x,
                full.min.y + 8.0,
            ),
            galley.size() + pad * 2.0,
        );
        painter.rect_filled(
            rect,
            8.0,
            ui::with_alpha(
                color32(self.theme.ui.panel_bg),
                0.94 * fade * self.theme.ui_alpha,
            ),
        );
        painter.rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(
                1.0,
                ui::with_alpha(color32(self.theme.ui.danger), 0.45 * fade),
            ),
            egui::StrokeKind::Inside,
        );
        painter.galley(rect.min + pad, galley, toast_color);
        // Keep the fade animating while it is on screen.
        ctx.request_repaint();
    }

    /// The countdown an image hovering over the window runs before it
    /// becomes the background.
    pub(crate) fn draw_bg_drop(&mut self, ctx: &egui::Context) {
        let Some(at) = self.bg_drop_at else {
            return;
        };
        let left = (Self::BG_DROP_SECS - (self.now - at)).max(0.0);
        let full = ctx.content_rect();
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("cl2metal-bg-drop"));
        let painter = ctx.layer_painter(layer);
        let title = painter.layout_no_wrap(
            format!("Background image in {left:.1}s"),
            egui::FontId::new(12.5, egui::FontFamily::Proportional),
            color32(self.theme.ui.text),
        );
        let hint = painter.layout_no_wrap(
            "drop it on the terminal to attach instead".to_owned(),
            egui::FontId::new(11.0, egui::FontFamily::Proportional),
            color32(self.theme.ui.text_dim),
        );
        let pad = egui::vec2(14.0, 10.0);
        let title_h = title.size().y;
        let w = title.size().x.max(hint.size().x) + pad.x * 2.0;
        let h = title_h + hint.size().y + 12.0 + pad.y * 2.0;
        let rect = egui::Rect::from_min_size(
            egui::pos2(full.center().x - w * 0.5, full.min.y + 44.0),
            egui::vec2(w, h),
        );
        painter.rect_filled(
            rect,
            10.0,
            ui::with_alpha(color32(self.theme.ui.panel_bg), 0.96 * self.theme.ui_alpha),
        );
        painter.rect_stroke(
            rect,
            10.0,
            egui::Stroke::new(1.0, color32(self.theme.ui.border)),
            egui::StrokeKind::Inside,
        );
        painter.galley(rect.min + pad, title, color32(self.theme.ui.text));
        painter.galley(
            rect.min + pad + egui::vec2(0.0, title_h + 4.0),
            hint,
            color32(self.theme.ui.text_dim),
        );
        // The progress bar.
        let track = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + pad.x, rect.max.y - pad.y - 3.0),
            egui::vec2(rect.width() - pad.x * 2.0, 3.0),
        );
        let done = (1.0 - left / Self::BG_DROP_SECS).clamp(0.0, 1.0) as f32;
        painter.rect_filled(
            track,
            1.5,
            ui::with_alpha(color32(self.theme.ui.border), 0.6),
        );
        painter.rect_filled(
            egui::Rect::from_min_size(track.min, egui::vec2(track.width() * done, track.height())),
            1.5,
            color32(self.theme.ui.accent),
        );
        ctx.request_repaint();
    }

    pub(crate) fn terminal_area(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let full = ui.available_rect_before_wrap();
        let mut resp = ui.allocate_rect(full, egui::Sense::click_and_drag());
        // The terminal owns the keyboard. egui moves focus on a bare Tab or
        // arrow key unless the focused widget's event filter swallows them, and
        // the widgets it lands on are the tab-bar buttons: the next Enter then
        // activates whichever one it reached (Settings, or a new tab).
        if matches!(self.overlay, Overlay::None)
            && self.search.is_none()
            && self.prompts.is_empty()
            && self.consents.is_empty()
            && self.sysinfo.is_none()
        {
            if !resp.has_focus() {
                resp.request_focus();
            }
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    resp.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
        }
        if resp.contains_pointer() {
            resp = resp.on_hover_cursor(egui::CursorIcon::Text);
        }

        // OSC 8 hyperlinks: pointing hand + tooltip; Ctrl+click opens.
        if let Some(pos) = ctx.input(|i| i.pointer.hover_pos())
            && let Some(uri) = self.hyperlink_at(pos)
        {
            resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            resp.show_tooltip_text(format!("{uri}\nCtrl+click to open"));
        }

        if self.tabs.is_empty() {
            return;
        }

        // Raster size / metrics.
        let ppp = ctx.pixels_per_point();
        let size_px = Self::raster_size(&self.config, ppp);
        self.atlas.set_size(size_px);
        let metrics = self.atlas.metrics();

        let cell_px = [metrics.cell_w, metrics.cell_h];
        let cell_pt = [cell_px[0] / ppp, cell_px[1] / ppp];
        let pad = self.config.padding;

        // Lay the tab's panes out over the terminal area.
        let rects = pane_rects(&self.tabs[self.active].tree, full);
        let mut layouts: Vec<(usize, LayoutInfo)> = Vec::with_capacity(rects.len());
        for (pane_i, rect) in &rects {
            let inner = rect.shrink2(egui::vec2(pad[1], pad[0]));
            let cols = ((inner.width() / cell_pt[0]).floor() as usize).max(2);
            let rows = ((inner.height() / cell_pt[1]).floor() as usize).max(1);
            let used = egui::vec2(cols as f32 * cell_pt[0], rows as f32 * cell_pt[1]);
            // Horizontally centred, but anchored to the top: the first row sits
            // right under the tab strip instead of floating in the leftover
            // space.
            let origin_pt = egui::pos2(inner.min.x + (inner.width() - used.x) * 0.5, inner.min.y);
            layouts.push((
                *pane_i,
                LayoutInfo {
                    rect: *rect,
                    origin_px: [origin_pt.x * ppp, origin_pt.y * ppp],
                    cell_px,
                    ppp,
                    cols,
                    rows,
                },
            ));
        }

        // Resize the panes whose grid changed, and remember the focused one.
        let active_pane = self.tabs[self.active].active_pane;
        let mut active_layout = None;
        {
            let tab = &mut self.tabs[self.active];
            for (pane_i, l) in &layouts {
                let pane = &mut tab.panes[*pane_i];
                if pane.core.cols() != l.cols || pane.core.rows() != l.rows {
                    log::debug!(
                        "pane {pane_i} layout: cols={} rows={} rect={:?}",
                        l.cols,
                        l.rows,
                        l.rect
                    );
                    pane.core.resize(l.cols, l.rows);
                    let _ = pane.client_tx.send(ClientMsg::Resize {
                        cols: l.cols as u16,
                        rows: l.rows as u16,
                        cell_w: cell_px[0] as u16,
                        cell_h: cell_px[1] as u16,
                    });
                }
                if *pane_i == active_pane {
                    active_layout = Some(l.clone());
                }
            }
        }
        // Background tabs are not laid out: give their panes the whole area.
        let full_inner = full.shrink2(egui::vec2(pad[1], pad[0]));
        let full_cols = ((full_inner.width() / cell_pt[0]).floor() as usize).max(2);
        let full_rows = ((full_inner.height() / cell_pt[1]).floor() as usize).max(1);
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if i == self.active {
                continue;
            }
            for pane in &mut tab.panes {
                if pane.core.cols() != full_cols || pane.core.rows() != full_rows {
                    pane.core.resize(full_cols, full_rows);
                    let _ = pane.client_tx.send(ClientMsg::Resize {
                        cols: full_cols as u16,
                        rows: full_rows as u16,
                        cell_w: cell_px[0] as u16,
                        cell_h: cell_px[1] as u16,
                    });
                }
            }
        }
        self.pane_layouts = layouts.clone();
        if let Some(l) = active_layout {
            self.layout = Some(l);
        }

        // The focused pane's geometry drives the IME and the completion popup.
        let (origin_pt, cell_pt) = match self.layout.as_ref() {
            Some(l) => (
                egui::pos2(l.origin_px[0] / l.ppp, l.origin_px[1] / l.ppp),
                [l.cell_px[0] / l.ppp, l.cell_px[1] / l.ppp],
            ),
            None => (full.min, [8.0, 16.0]),
        };

        // IME: let the platform compose text at the terminal cursor.
        let composing = matches!(self.overlay, Overlay::None)
            && self.search.is_none()
            && self.prompts.is_empty()
            && self.consents.is_empty();
        if composing && self.last_focused {
            let cursor_rect = {
                let tab = &self.tabs[self.active];
                let content = tab.pane().core.term.renderable_content();
                let point = content.cursor.point;
                let row = point.line.0 + content.display_offset as i32;
                let x = origin_pt.x + point.column.0 as f32 * cell_pt[0];
                let y = origin_pt.y + row as f32 * cell_pt[1];
                egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell_pt[0], cell_pt[1]))
            };
            ctx.output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    purpose: egui::IMEPurpose::Terminal,
                    rect: full,
                    cursor_rect,
                    should_interrupt_composition: false,
                });
            });
        }

        // `:` command completion popup, next to the cursor line.
        self.draw_completions(ui, origin_pt, cell_pt, full);

        // Build & paint every pane of the active tab.
        let atlas_view = self.sync_atlas();
        let search_cells = self.search_cells();
        let bg_alpha = self.bg_alpha();
        for (pane_i, l) in &layouts {
            let is_active = *pane_i == active_pane;
            let cursor_opts = CursorOpts {
                visible: is_active && self.cursor_on,
                hollow: !is_active || !self.last_focused,
            };
            let frame = {
                let pane = &self.tabs[self.active].panes[*pane_i];
                let (_, scroll_px) = pane.core.scroll.base_and_shift(l.cell_px[1]);
                render::build_frame(
                    &pane.core,
                    &mut self.atlas,
                    &self.theme,
                    l.origin_px,
                    l.cell_px,
                    cursor_opts,
                    if is_active {
                        search_cells.as_ref()
                    } else {
                        None
                    },
                    // The UI has already painted the one shared backdrop for
                    // the whole client area. Default terminal cells must not
                    // repaint it as a separate, darker rectangle below the tab
                    // strip.
                    true,
                    scroll_px,
                )
            };
            let callback = TerminalCallback::new(
                self.renderer.clone(),
                atlas_view.clone(),
                *pane_i as u64,
                render::viewport_px(l.rect, l.ppp),
                frame,
                bg_alpha,
            );
            ui.painter()
                .add(egui_wgpu::Callback::new_paint_callback(l.rect, callback));
        }

        // Dividers between panes: drag to resize.
        for d in pane_dividers(&self.tabs[self.active].tree, full) {
            let resp = ui.interact(
                d.rect.expand(2.0),
                ui.id().with(("pane_divider", &d.path)),
                egui::Sense::click_and_drag(),
            );
            let hot = resp.hovered() || resp.dragged();
            if hot {
                ui.ctx().set_cursor_icon(match d.dir {
                    SplitDir::Right => egui::CursorIcon::ResizeHorizontal,
                    SplitDir::Down => egui::CursorIcon::ResizeVertical,
                });
            }
            if resp.drag_started() {
                self.divider_drag = Some(d.path.clone());
            }
            if resp.dragged()
                && self.divider_drag.as_deref() == Some(d.path.as_slice())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let ratio = match d.dir {
                    SplitDir::Right => {
                        (pos.x - d.area.min.x) / (d.area.width() - PANE_DIVIDER).max(1.0)
                    }
                    SplitDir::Down => {
                        (pos.y - d.area.min.y) / (d.area.height() - PANE_DIVIDER).max(1.0)
                    }
                };
                set_ratio(&mut self.tabs[self.active].tree, &d.path, ratio);
            }
            if resp.drag_stopped() {
                self.divider_drag = None;
            }
            let painter = ui.painter();
            let line = if d.dir == SplitDir::Right {
                egui::Rect::from_min_max(
                    egui::pos2(d.rect.center().x - 0.5, d.rect.min.y + 4.0),
                    egui::pos2(d.rect.center().x + 0.5, d.rect.max.y - 4.0),
                )
            } else {
                egui::Rect::from_min_max(
                    egui::pos2(d.rect.min.x + 4.0, d.rect.center().y - 0.5),
                    egui::pos2(d.rect.max.x - 4.0, d.rect.center().y + 0.5),
                )
            };
            painter.rect_filled(
                line,
                0.0,
                ui::with_alpha(color32(self.theme.ui.border), if hot { 0.9 } else { 0.45 }),
            );
        }

        // Startup animation: the terminal is revealed top-down from its own
        // background, so the first frame is a clean terminal-colored surface.
        if self.entry_t < 1.0 {
            let t = self.entry_t;
            let curtain = egui::Rect::from_min_max(
                egui::pos2(full.min.x, full.min.y + full.height() * t),
                full.max,
            );
            let veil = (1.0 - t) * (1.0 - t) * self.curtain_alpha();
            ui.painter()
                .rect_filled(curtain, 0.0, ui::with_alpha(color32(self.theme.bg), veil));
        }

        // Visual bell: a brief flash over the terminal.
        if let Some(at) = self.bell_at {
            let age = self.now - at;
            if age > 0.15 {
                self.bell_at = None;
            } else if self.config.visual_bell {
                let a = ((1.0 - age / 0.15) * 0.10) as f32;
                ui.painter().rect_filled(
                    full,
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, (a * 255.0) as u8),
                );
            }
        }

        // Scrollbar (only once there is history to scroll). It belongs to the
        // focused pane, so it hugs that pane's right edge.
        let pane_rect = self.layout.as_ref().map(|l| l.rect).unwrap_or(full);
        let history = self.tabs[self.active].core.term.grid().history_size();
        if history > 0 {
            let offset = self.tabs[self.active].core.term.grid().display_offset();
            let rows = self.tabs[self.active].core.rows();
            let bar_w = self.scrollbar_w;
            let track = egui::Rect::from_min_max(
                egui::pos2(pane_rect.max.x - 4.0 - bar_w, pane_rect.min.y + 4.0),
                egui::pos2(pane_rect.max.x - 4.0, pane_rect.max.y - 4.0),
            );
            let total = (rows + history) as f32;
            let thumb_h = (track.height() * rows as f32 / total).max(24.0);
            let frac = 1.0 - offset as f32 / history as f32;
            let thumb_y = track.min.y + (track.height() - thumb_h) * frac.clamp(0.0, 1.0);
            let thumb = egui::Rect::from_min_size(
                egui::pos2(track.min.x, thumb_y),
                egui::vec2(track.width(), thumb_h),
            );
            let hit = egui::Rect::from_min_max(
                egui::pos2(track.min.x - 5.0, track.min.y),
                egui::pos2(track.max.x + 5.0, track.max.y),
            );
            let bar = ui.interact(
                hit,
                ui.id().with("scrollbar"),
                egui::Sense::click_and_drag(),
            );
            let active = bar.hovered() || bar.dragged();
            // The bar breathes wider under the pointer, VS Code style.
            let want = if active {
                SCROLLBAR_WIDE
            } else {
                SCROLLBAR_THIN
            };
            let was = self.scrollbar_w;
            self.scrollbar_w = eased_width(self.scrollbar_w, want, self.frame_dt);
            if self.scrollbar_w != was {
                ui.ctx().request_repaint();
            }
            let painter = ui.painter();
            let radius = (bar_w * 0.5).min(4.0);
            painter.rect_filled(
                track,
                radius,
                ui::with_alpha(
                    color32(self.theme.ui.border),
                    if active { 0.9 } else { 0.5 },
                ),
            );
            painter.rect_filled(
                thumb,
                radius,
                ui::with_alpha(
                    color32(self.theme.ui.text_dim),
                    if active { 1.0 } else { 0.7 },
                ),
            );
            let drag_pos = if bar.dragged() || bar.clicked() {
                bar.interact_pointer_pos()
            } else {
                None
            };
            if let Some(pos) = drag_pos {
                let span = (track.height() - thumb_h).max(1.0);
                let rel = ((pos.y - track.min.y) / span).clamp(0.0, 1.0);
                let target = ((1.0 - rel) * history as f32).round() as i32;
                let delta = target - offset as i32;
                if delta != 0 {
                    self.scroll_terminal(Scroll::Delta(delta));
                }
            }
        }

        // Dead tab banner.
        if let Some(reason) = self.tabs[self.active].dead.clone() {
            let text = format!("[{reason}]   Enter close   R reconnect");
            let dim_color = color32(self.theme.ui.text_dim);
            let galley = ui.painter().layout_no_wrap(
                text,
                egui::FontId::new(13.0, egui::FontFamily::Proportional),
                dim_color,
            );
            let pad = egui::vec2(14.0, 8.0);
            let rect = egui::Rect::from_center_size(full.center(), galley.size() + pad * 2.0);
            let painter = ui.painter();
            painter.rect_filled(
                rect,
                8.0,
                ui::with_alpha(color32(self.theme.ui.panel_bg), 0.94 * self.theme.ui_alpha),
            );
            painter.rect_stroke(
                rect,
                8.0,
                egui::Stroke::new(1.0, color32(self.theme.ui.border)),
                egui::StrokeKind::Inside,
            );
            painter.galley(rect.min + pad, galley, dim_color);
        }

        // Right-click menu.
        let mut menu: Option<MenuAction> = None;
        resp.context_menu(|ui| {
            if ui.button("Copy").clicked() {
                menu = Some(MenuAction::Copy);
                ui.close();
            }
            if ui.button("Paste").clicked() {
                menu = Some(MenuAction::Paste);
                ui.close();
            }
            if ui.button("Select all").clicked() {
                menu = Some(MenuAction::SelectAll);
                ui.close();
            }
            ui.separator();
            if ui.button("Search…").clicked() {
                menu = Some(MenuAction::Search);
                ui.close();
            }
            if ui.button("Clear scrollback").clicked() {
                menu = Some(MenuAction::ClearScrollback);
                ui.close();
            }
            ui.separator();
            if ui.button("Rename tab").clicked() {
                menu = Some(MenuAction::RenameTab);
                ui.close();
            }
            if ui.button("New tab").clicked() {
                menu = Some(MenuAction::NewTab);
                ui.close();
            }
            if ui.button("Close tab").clicked() {
                menu = Some(MenuAction::CloseTab);
                ui.close();
            }
        });
        if let Some(action) = menu {
            self.run_menu_action(action, &ctx);
        }
    }

    /// Apply a terminal context-menu action.
    pub(crate) fn run_menu_action(&mut self, action: MenuAction, ctx: &egui::Context) {
        match action {
            MenuAction::Copy => {
                if let Some(text) = self.selection_text()
                    && !text.is_empty()
                {
                    ctx.copy_text(text);
                }
            }
            MenuAction::Paste => {
                if let Some(text) = self.read_clipboard() {
                    let bracketed = self
                        .tabs
                        .get(self.active)
                        .is_some_and(|t| t.core.term.mode().contains(TermMode::BRACKETED_PASTE));
                    let bytes = input::encode_paste(&text, bracketed);
                    self.last_input = self.now;
                    self.scroll_terminal(Scroll::Bottom);
                    self.send_input(bytes);
                }
            }
            MenuAction::SelectAll => self.select_all(),
            MenuAction::Search => self.toggle_search(),
            MenuAction::ClearScrollback => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.core.term.grid_mut().clear_history();
                }
            }
            MenuAction::RenameTab => {
                let text = self
                    .tabs
                    .get(self.active)
                    .map(|t| t.display_title().to_owned())
                    .unwrap_or_default();
                self.overlay = Overlay::Rename {
                    tab: self.active,
                    text,
                };
            }
            MenuAction::NewTab => self.spawn_local(),
            MenuAction::CloseTab => {
                let active = self.active;
                self.close_tab(active, ctx, "context menu");
            }
        }
    }

    pub(crate) fn draw_overlays(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme.clone();

        // Agent consent gates every brokered session and every screened
        // command, and it draws first: nothing underneath it is answered
        // while a question is pending.
        while let Some(front) = self.consents.front() {
            if self.now - front.0 <= CONSENT_TIMEOUT_SECS {
                break;
            }
            // The broker gave up on its side; answer denial so the state
            // is deterministic even if the clocks drifted.
            if let Some((_, dead)) = self.consents.pop_front() {
                let _ = dead.reply.send(ConsentAnswer::Deny);
            }
        }
        if let Some(front) = self.consents.front_mut() {
            let remaining = (CONSENT_TIMEOUT_SECS - (self.now - front.0)).max(0.0);
            let mut decision: Option<ConsentAnswer> = None;
            ui::show_consent(ui, &theme, &front.1, remaining, &mut decision);
            if let Some(answer) = decision
                && let Some((_, answered)) = self.consents.pop_front()
            {
                let _ = answered.reply.send(answer);
            }
            return;
        }

        // SSH prompts (queued).
        if !self.prompts.is_empty() {
            let mut submit = false;
            let mut cancel = false;
            let mut text = std::mem::take(&mut self.prompt_text);
            {
                let prompt = self.prompts.front_mut().unwrap();
                ui::show_prompt(
                    ui,
                    &theme,
                    &mut prompt.kind,
                    &mut text,
                    &mut submit,
                    &mut cancel,
                );
            }
            self.prompt_text = text;
            if submit || cancel {
                self.finish_prompt(submit);
            }
            return;
        }

        let mut close = false;
        let mut saved: Option<(Config, String)> = None;
        let mut preview: Option<(Config, String)> = None;
        let mut restore: Option<(Config, String)> = None;
        let mut action: Option<PaletteAction> = None;
        let mut connect: Option<String> = None;
        let mut view_image: Option<String> = None;
        let mut close_confirm: Option<CloseScope> = None;
        let mut forget_host: Option<usize> = None;
        let mut rename_submit: Option<(usize, String)> = None;
        let mut paste: Option<String> = None;
        let mut profile_save: Option<(crate::config::SshProfile, Option<String>)> = None;
        let mut profile_delete: Option<String> = None;
        let mut keys_save: Option<std::collections::BTreeMap<String, String>> = None;
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Palette { query, selected } => {
                if let Some(a) = ui::show_palette(
                    ui,
                    &theme,
                    &self.config,
                    query,
                    selected,
                    &self.recent_actions,
                ) {
                    action = Some(a);
                    close = true;
                }
            }
            Overlay::Settings {
                draft,
                font,
                error,
                known_hosts,
                original,
                original_font,
            } => {
                let fonts = glyphs::monospace_families(&self.font_db);
                let (c, s, f) =
                    ui::show_settings(ui, &theme, draft, font, error, &fonts, known_hosts);
                // The draft is a faithful copy of the config, font included,
                // so a plain comparison detects any change.
                draft.font = font.clone();
                if *draft != self.config {
                    preview = Some((draft.clone(), font.clone()));
                }
                if c {
                    close = true;
                }
                saved = s;
                forget_host = f;
                if close && saved.is_none() {
                    // Cancel (or Esc): put the config back the way it was.
                    restore = Some((original.clone(), original_font.clone()));
                }
            }
            Overlay::Connect { text, error } => {
                let (c, submit) = ui::show_connect(ui, &theme, text, error);
                if c {
                    close = true;
                }
                connect = submit;
            }
            Overlay::ViewImage { text, error } => {
                let (c, submit) = ui::show_view_image(ui, &theme, text, error);
                if c {
                    close = true;
                }
                view_image = submit;
            }
            Overlay::Image { path, texture } => {
                if ui::show_image(ui, &theme, path, texture) {
                    close = true;
                }
            }
            Overlay::CloseConfirm { scope, jobs } => {
                let (cancel, go) = ui::show_close_confirm(ui, &theme, jobs, scope);
                if cancel {
                    self.force_close = false;
                    close = true;
                }
                if go {
                    close_confirm = Some(*scope);
                    close = true;
                }
            }
            Overlay::Rename { tab, text } => {
                let (c, submit) = ui::show_rename(ui, &theme, text);
                if c {
                    close = true;
                }
                rename_submit = submit.map(|title| (*tab, title));
            }
            Overlay::PasteConfirm { text } => {
                let (c, ok) = ui::show_paste_confirm(ui, &theme, text);
                if c {
                    close = true;
                }
                if ok {
                    paste = Some(text.clone());
                }
            }
            Overlay::Profile {
                profile,
                original,
                error,
            } => {
                let (c, save, remove) =
                    ui::show_profile(ui, &theme, profile, original.as_deref(), error);
                if c {
                    close = true;
                }
                if save {
                    profile_save = Some((profile.clone(), original.clone()));
                }
                if remove {
                    profile_delete = original.clone();
                }
            }
            Overlay::Keybindings {
                bindings,
                recording,
                error,
            } => {
                let (c, save, reset) = ui::show_keybindings(ui, &theme, bindings, recording, error);
                if c {
                    close = true;
                }
                if reset {
                    *bindings = crate::editor::keymap::defaults()
                        .into_iter()
                        .map(|(c, s)| (c.to_owned(), s.to_owned()))
                        .collect();
                    *error = None;
                }
                if save {
                    keys_save = Some(bindings.clone());
                }
            }
        }
        if let Some((tab, title)) = rename_submit {
            if let Some(t) = self.tabs.get_mut(tab) {
                t.title_override = if title.trim().is_empty() {
                    None
                } else {
                    Some(title)
                };
            }
            close = true;
        }
        if let Some((profile, original)) = profile_save {
            match self.save_profile(profile, original.as_deref()) {
                Ok(()) => close = true,
                Err(e) => {
                    if let Overlay::Profile { error, .. } = &mut self.overlay {
                        *error = Some(e);
                    }
                }
            }
        }
        if let Some(name) = profile_delete {
            self.delete_profile(&name);
            close = true;
        }
        if let Some(bindings) = keys_save {
            self.config.editor_keys = bindings;
            self.editor.keymap = crate::editor::Keymap::from_config(&self.config.editor_keys);
            self.persist_config();
            self.toast = Some(("keybindings saved".to_owned(), self.now));
            log::info!("keybindings saved");
            close = true;
        }
        if let Some(spec) = connect {
            match self.connect_adhoc(&spec) {
                Ok(()) => close = true,
                Err(e) => {
                    if let Overlay::Connect { error, .. } = &mut self.overlay {
                        *error = Some(e);
                    }
                }
            }
        }
        if let Some(line) = view_image {
            match self.open_image(&line, ui.ctx()) {
                // Success replaced the dialog with the picture; make sure the
                // close below cannot take the new overlay back down.
                Ok(()) => close = false,
                Err(e) => {
                    if let Overlay::ViewImage { error, .. } = &mut self.overlay {
                        *error = Some(e);
                    }
                }
            }
        }
        if let Some(scope) = close_confirm {
            match scope {
                CloseScope::Pane { tab, pane } => {
                    if tab < self.tabs.len() {
                        self.force_close = true;
                        self.tabs[tab].active_pane = pane;
                        self.active = tab;
                        self.close_pane(ui.ctx());
                        self.force_close = false;
                    }
                }
                CloseScope::Tab(i) => {
                    if i < self.tabs.len() {
                        self.force_close = true;
                        self.close_tab(i, ui.ctx(), "confirmed");
                        self.force_close = false;
                    }
                }
                CloseScope::App => {
                    // The flag stays set: the close request arrives next
                    // frame and must go through this time.
                    self.force_close = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if let Some(line) = forget_host {
            if let Err(e) = crate::config::known_hosts::forget(line) {
                self.toast = Some((format!("known_hosts: {e}"), self.now));
            }
            if let Overlay::Settings { known_hosts, .. } = &mut self.overlay {
                *known_hosts = crate::config::known_hosts::list();
            }
        }
        if let Some(a) = action {
            self.run_action(a, ui.ctx());
        }
        if let Some(text) = paste {
            let mode = self
                .tabs
                .get(self.active)
                .map(|t| *t.core.term.mode())
                .unwrap_or(TermMode::empty());
            self.paste_now(&text, mode);
            close = true;
        }
        if let Some((draft, font)) = saved {
            let ctx = ui.ctx().clone();
            // Removed hosts and changed auth methods invalidate keyring
            // entries; it happens on save so Cancel leaves them alone.
            if let Overlay::Settings { original, .. } = &self.overlay {
                self.drop_stale_secrets(&original.profiles, &draft.profiles);
            }
            self.apply_config(draft, font, &ctx);
        } else if let Some((config, font)) = restore {
            let ctx = ui.ctx().clone();
            self.apply_config(config, font, &ctx);
        } else if let Some((draft, font)) = preview {
            // Live preview: the draft is already applied to the running app.
            let ctx = ui.ctx().clone();
            self.apply_config(draft, font, &ctx);
        }
        if close {
            self.overlay = Overlay::None;
        }
    }

    /// Open a picture into the image viewer.
    ///
    /// The path is taken as the user typed it, a leading `~` included, and a
    /// failure keeps the dialog up with the reason in it rather than closing
    /// on a blank screen.
    fn open_image(&mut self, line: &str, ctx: &egui::Context) -> Result<(), String> {
        let line = line.trim();
        if line.is_empty() {
            return Err("name a picture to look at".to_owned());
        }
        let path = crate::editor::expand(std::path::Path::new(line));
        let image = decode_image(&path)?;
        let texture = ctx.load_texture(
            path.display().to_string(),
            image,
            egui::TextureOptions::LINEAR,
        );
        log::info!("image opened: {}", path.display());
        self.overlay = Overlay::Image { path, texture };
        Ok(())
    }
}

/// Decode a picture file into pixels for a texture.
///
/// Free-standing so it can be tested without an App or a live egui context.
/// The same size limit as the editor's text files applies, for the same
/// reason: a stray `View` on something enormous must be refused by a stat
/// instead of stalling the interface on a decode.
fn decode_image(path: &std::path::Path) -> Result<egui::ColorImage, String> {
    const LIMIT: u64 = crate::editor::MAX_EDIT_BYTES;
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.len() > LIMIT {
        return Err(format!(
            "{}: {:.0} MB is too much to look at; the limit is {} MB",
            path.display(),
            meta.len() as f64 / (1024.0 * 1024.0),
            LIMIT / (1024 * 1024)
        ));
    }
    let img = image::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(size, &img))
}

// ---------------------------------------------------------------------------
// Pane layout
// ---------------------------------------------------------------------------

/// Thickness of the divider between two panes, in points.
const PANE_DIVIDER: f32 = 6.0;

/// A draggable divider between two panes.
pub(crate) struct Divider {
    /// The divider strip itself.
    pub(crate) rect: egui::Rect,
    /// The split's whole area, for turning a pointer position into a ratio.
    pub(crate) area: egui::Rect,
    pub(crate) dir: SplitDir,
    /// Which split node this is: `true` takes `b` at that level.
    pub(crate) path: Vec<bool>,
}

/// Rects for every pane of a tab, in draw order.
pub(crate) fn pane_rects(node: &PaneNode, area: egui::Rect) -> Vec<(usize, egui::Rect)> {
    match node {
        PaneNode::Leaf(i) => vec![(*i, area)],
        PaneNode::Split { dir, ratio, a, b } => {
            let r = ratio.clamp(0.15, 0.85);
            let (ra, rb) = match dir {
                SplitDir::Right => {
                    let x = area.min.x + (area.width() - PANE_DIVIDER) * r;
                    (
                        egui::Rect::from_min_max(area.min, egui::pos2(x, area.max.y)),
                        egui::Rect::from_min_max(
                            egui::pos2(x + PANE_DIVIDER, area.min.y),
                            area.max,
                        ),
                    )
                }
                SplitDir::Down => {
                    let y = area.min.y + (area.height() - PANE_DIVIDER) * r;
                    (
                        egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, y)),
                        egui::Rect::from_min_max(
                            egui::pos2(area.min.x, y + PANE_DIVIDER),
                            area.max,
                        ),
                    )
                }
            };
            let mut out = pane_rects(a, ra);
            out.extend(pane_rects(b, rb));
            out
        }
    }
}

/// Every divider in a tab's tree, with the path needed to resize it.
pub(crate) fn pane_dividers(node: &PaneNode, area: egui::Rect) -> Vec<Divider> {
    let mut out = Vec::new();
    collect_dividers(node, area, &mut Vec::new(), &mut out);
    out
}

fn collect_dividers(
    node: &PaneNode,
    area: egui::Rect,
    path: &mut Vec<bool>,
    out: &mut Vec<Divider>,
) {
    let PaneNode::Split { dir, ratio, a, b } = node else {
        return;
    };
    let r = ratio.clamp(0.15, 0.85);
    let (ra, rb, strip) = match dir {
        SplitDir::Right => {
            let x = area.min.x + (area.width() - PANE_DIVIDER) * r;
            (
                egui::Rect::from_min_max(area.min, egui::pos2(x, area.max.y)),
                egui::Rect::from_min_max(egui::pos2(x + PANE_DIVIDER, area.min.y), area.max),
                egui::Rect::from_min_max(
                    egui::pos2(x, area.min.y),
                    egui::pos2(x + PANE_DIVIDER, area.max.y),
                ),
            )
        }
        SplitDir::Down => {
            let y = area.min.y + (area.height() - PANE_DIVIDER) * r;
            (
                egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, y)),
                egui::Rect::from_min_max(egui::pos2(area.min.x, y + PANE_DIVIDER), area.max),
                egui::Rect::from_min_max(
                    egui::pos2(area.min.x, y),
                    egui::pos2(area.max.x, y + PANE_DIVIDER),
                ),
            )
        }
    };
    out.push(Divider {
        rect: strip,
        area,
        dir: *dir,
        path: path.clone(),
    });
    path.push(false);
    collect_dividers(a, ra, path, out);
    path.pop();
    path.push(true);
    collect_dividers(b, rb, path, out);
    path.pop();
}

/// Set the split ratio at `path`.
pub(crate) fn set_ratio(node: &mut PaneNode, path: &[bool], ratio: f32) {
    let PaneNode::Split { ratio: r, a, b, .. } = node else {
        return;
    };
    match path.split_first() {
        None => *r = ratio.clamp(0.15, 0.85),
        Some((true, rest)) => set_ratio(b, rest, ratio),
        Some((false, rest)) => set_ratio(a, rest, ratio),
    }
}

/// The scrollbar's width at rest, and under the pointer.
pub(crate) const SCROLLBAR_THIN: f32 = 3.0;
pub(crate) const SCROLLBAR_WIDE: f32 = 8.0;

/// One eased step of the scrollbar's hover width, snapped when close.
fn eased_width(cur: f32, want: f32, dt: f32) -> f32 {
    let k = 1.0 - (-dt * 26.0).exp();
    let next = cur + (want - cur) * k;
    if (want - next).abs() < 0.05 {
        want
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::decode_image;

    #[test]
    fn the_scrollbar_width_eases_both_ways() {
        // Expands toward the wide target without overshooting.
        let mut w = super::SCROLLBAR_THIN;
        for _ in 0..60 {
            w = super::eased_width(w, super::SCROLLBAR_WIDE, 0.016);
        }
        assert_eq!(w, super::SCROLLBAR_WIDE, "converged on hover");
        // Shrinks back the same way.
        for _ in 0..60 {
            w = super::eased_width(w, super::SCROLLBAR_THIN, 0.016);
        }
        assert_eq!(w, super::SCROLLBAR_THIN, "back to thin at rest");
    }

    #[test]
    fn decoding_a_png_reports_its_size() {
        let dir = std::env::temp_dir().join(format!("cl2metal-image-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dot.png");
        image::RgbaImage::from_pixel(7, 5, image::Rgba([200, 30, 90, 255]))
            .save(&path)
            .unwrap();
        let img = decode_image(&path).unwrap();
        assert_eq!(img.size, [7, 5], "the picture's own dimensions");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decoding_garbage_or_nothing_is_an_error_not_a_crash() {
        let dir = std::env::temp_dir().join(format!("cl2metal-image-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not-a-picture.png");
        std::fs::write(&path, "definitely not a png").unwrap();
        let err = decode_image(&path).unwrap_err();
        assert!(err.contains("not-a-picture.png"), "names the file: {err}");
        // And a file that is not there says so too.
        let missing = decode_image(&dir.join("gone.png")).unwrap_err();
        assert!(missing.contains("gone.png"), "names the file: {missing}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
