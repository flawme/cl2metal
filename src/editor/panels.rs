//! The editor's panels: the file tree, the status line, find/replace, the
//! command palette and the completion popup.

use super::Editor;
use super::buffer::ExternalChange;
use super::mode::Mode;
use super::palette::PaletteAction;
use super::pos::Pos;
use super::tree::TreePrompt;
use super::view::{EditorStyle, Metrics};

/// How long a status message stays up.
const MESSAGE_SECS: f64 = 6.0;

impl Editor {
    /// The file tree down the left-hand side.
    pub(crate) fn draw_tree(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        style: &EditorStyle,
        font: &egui::FontId,
        metrics: Metrics,
    ) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, style.panel_wash);
        let row_h = metrics.cell_h + 2.0;
        // Whether the prompt was already up when this frame started: the click
        // that opens it must not also count as a click outside it.
        let prompt_was_open = self.tree.prompt.is_some();
        let header = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), row_h + 4.0));
        let root_name = self
            .tree
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.tree.root.display().to_string());
        // A root that was deleted off the disk says so instead of pretending
        // the cached rows are still there.
        let (root_label, root_color) = if self.tree.root_missing {
            (format!("{root_name} — gone"), style.error)
        } else {
            (root_name, style.dim)
        };
        painter.text(
            egui::pos2(header.min.x + 8.0, header.center().y),
            egui::Align2::LEFT_CENTER,
            root_label,
            font.clone(),
            root_color,
        );
        // Header actions: refresh, new file, new folder, and — with a row
        // selected — rename and delete.
        let mut x = rect.max.x - row_h - 4.0;
        let mut button = |ui: &mut egui::Ui, glyph: &str, tip: &str| -> bool {
            let rect = egui::Rect::from_min_size(
                egui::pos2(x, header.min.y + 2.0),
                egui::vec2(row_h, row_h),
            );
            let clicked = ui
                .interact(
                    rect,
                    ui.id().with(("tree-btn", glyph)),
                    egui::Sense::click(),
                )
                .on_hover_text(tip)
                .clicked();
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                glyph,
                font.clone(),
                if clicked { style.accent } else { style.dim },
            );
            x -= row_h + 2.0;
            clicked
        };
        if button(ui, "⟳", "refresh") {
            self.tree.invalidate();
        }
        if button(ui, "+", "new file") {
            self.tree.prompt = Some(TreePrompt::New {
                dir: self.tree.target_dir(),
                name: String::new(),
                is_dir: false,
            });
        }
        if button(ui, "⊞", "new folder") {
            self.tree.prompt = Some(TreePrompt::New {
                dir: self.tree.target_dir(),
                name: String::new(),
                is_dir: true,
            });
        }
        if let Some(path) = self.tree.selected.clone() {
            if button(ui, "✏", "rename") {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.tree.prompt = Some(TreePrompt::Rename {
                    path: path.clone(),
                    name,
                });
            }
            if button(ui, "✖", "delete — moves to the trash") {
                self.trash_from_tree(&path);
            }
        }

        self.tree.refresh_if_dirty();
        let list = egui::Rect::from_min_max(
            egui::pos2(rect.min.x, header.max.y),
            egui::pos2(rect.max.x, rect.max.y),
        );
        let rows = (list.height() / row_h).floor().max(1.0) as usize;
        let scroll = self
            .tree_scroll
            .min(self.tree.rows().len().saturating_sub(rows) as f32);
        self.tree_scroll = scroll;
        // Ease the drawn offset toward the target, so the wheel glides.
        let dt = self.frame_dt.clamp(0.0, 0.1);
        let k = 1.0 - (-dt * 26.0).exp();
        self.tree_scroll_display += (scroll - self.tree_scroll_display) * k;
        if (self.tree_scroll_display - scroll).abs() < 0.005 {
            self.tree_scroll_display = scroll;
        } else {
            ui.ctx().request_repaint();
        }
        let top = self.tree_scroll_display.floor().max(0.0) as usize;
        let frac = (self.tree_scroll_display - top as f32).clamp(0.0, 1.0);

        let mut clicked: Option<(std::path::PathBuf, bool)> = None;
        // A drag in flight: its source, and the folder row under the pointer.
        let dragging = self.tree.drag.clone();
        let mut started: Option<std::path::PathBuf> = None;
        let mut hover_target: Option<std::path::PathBuf> = None;
        // One row past the viewport is kept: the sub-line shift pulls it in
        // at the bottom edge.
        for (i, row) in self
            .tree
            .rows()
            .iter()
            .enumerate()
            .skip(top)
            .take(rows + 1)
        {
            let y = list.min.y + (i - top) as f32 * row_h - frac * row_h;
            let row_rect = egui::Rect::from_min_size(
                egui::pos2(list.min.x, y),
                egui::vec2(list.width(), row_h),
            );
            let response = ui.interact(
                row_rect,
                ui.id().with(("editor-tree-row", i)),
                egui::Sense::click_and_drag(),
            );
            let is_dragged = dragging.as_deref() == Some(row.path.as_path());
            if response.hovered() || is_dragged {
                painter.rect_filled(row_rect, 0.0, style.current_line);
            }
            // egui reports hover only for the widget that owns the pointer,
            // and while a drag is in flight that is the row being carried, so
            // the landing zone is found geometrically instead.
            let over_row = ui
                .input(|i| i.pointer.latest_pos())
                .is_some_and(|p| row_rect.contains(p));
            // A folder row under the pointer while dragging is the landing
            // zone; the row being carried itself is not.
            let is_target = dragging.is_some() && row.dir && !is_dragged && over_row;
            if is_target {
                hover_target = Some(row.path.clone());
            }
            let is_active = self
                .buffer()
                .path
                .as_deref()
                .is_some_and(|p| p == row.path.as_path());
            if is_active {
                painter.rect_filled(row_rect, 0.0, style.selection);
            }
            let indent = 8.0 + row.depth as f32 * 12.0;
            let marker = if row.dir {
                if row.expanded { "▾" } else { "▸" }
            } else {
                " "
            };
            painter.text(
                egui::pos2(row_rect.min.x + indent, row_rect.center().y),
                egui::Align2::LEFT_CENTER,
                marker,
                font.clone(),
                style.dim,
            );
            painter.text(
                egui::pos2(row_rect.min.x + indent + 14.0, row_rect.center().y),
                egui::Align2::LEFT_CENTER,
                &row.name,
                font.clone(),
                if is_dragged || row.dir {
                    style.accent
                } else {
                    style.fg
                },
            );
            // The landing zone is outlined on top of everything else drawn
            // for the row, so it reads as the place the entry will go.
            if is_target {
                painter.rect_stroke(
                    row_rect,
                    2.0,
                    egui::Stroke::new(1.5, style.accent),
                    egui::StrokeKind::Inside,
                );
            }
            if response.drag_started_by(egui::PointerButton::Primary) {
                started = Some(row.path.clone());
            }
            if response.clicked() {
                clicked = Some((row.path.clone(), row.dir));
            }
        }
        // The header is the root: dropping there moves into the root folder.
        // Geometric, for the same reason as the row targets.
        if dragging.is_some()
            && ui
                .input(|i| i.pointer.latest_pos())
                .is_some_and(|p| header.contains(p))
        {
            hover_target = Some(self.tree.root.clone());
        }
        if let Some(path) = started {
            self.tree.drag = Some(path);
            self.tree.drag_target = None;
            ui.ctx()
                .output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
        }
        if let Some(target) = hover_target {
            self.tree.drag_target = Some(target);
        }
        // The button coming up ends the drag, wherever the pointer is: a
        // drop anywhere but on a folder row just puts it back.
        if dragging.is_some() && !ui.input(|i| i.pointer.primary_down()) {
            let from = self.tree.drag.take().expect("a drag was in flight");
            let target = self.tree.drag_target.take();
            ui.ctx()
                .output_mut(|o| o.cursor_icon = egui::CursorIcon::Default);
            if let Some(to_dir) = target
                && from.parent() != Some(to_dir.as_path())
            {
                self.move_in_tree(&from, &to_dir);
            }
        }
        if let Some((path, dir)) = clicked {
            // A click on a row while the prompt is up dismisses the prompt
            // instead of opening the file.
            if prompt_was_open {
                self.tree.prompt = None;
            } else {
                self.tree.selected = Some(path.clone());
                if dir {
                    self.tree.toggle(&path);
                } else {
                    self.open_path(&path);
                }
            }
        }
        if ui
            .interact(
                list,
                ui.id().with("editor-tree-scroll"),
                egui::Sense::hover(),
            )
            .hovered()
        {
            let delta = ui.input(|i| i.smooth_scroll_delta.y);
            if delta.abs() > 0.5 {
                self.tree_scroll = (self.tree_scroll - delta / row_h).max(0.0);
            }
        }

        // Delete over the tree: the pointer is in the tree, so the key is
        // the tree's to take. Plain Delete moves the selected entry to the
        // trash — recoverable, so nothing stands in its way — and
        // Shift+Delete asks before erasing for good.
        if self.tree.prompt.is_none()
            && ui.input(|i| {
                i.pointer
                    .interact_pos()
                    .is_some_and(|p| rect.contains(p))
                    && i.key_pressed(egui::Key::Delete)
            })
            && let Some(path) = self.tree.selected.clone()
        {
            let shift = ui.input(|i| i.modifiers.shift);
            if shift {
                let dir = path.is_dir();
                self.tree.prompt = Some(TreePrompt::Delete { path, dir });
            } else {
                self.trash_from_tree(&path);
            }
        }
    }

    /// The tree's question — new file, new folder, rename, permanently
    /// delete — over the middle of the editor, like every other modal.
    ///
    /// Drawn after the panes, so it rides on top of them; in the tree's
    /// top corner it was easy to miss, and the panes would have covered a
    /// centered box painted any earlier.
    pub(crate) fn draw_tree_prompt(
        &mut self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        style: &EditorStyle,
        font: &egui::FontId,
        metrics: Metrics,
        prompt_was_open: bool,
    ) {
        let row_h = metrics.cell_h + 2.0;
        let dialog = egui::Rect::from_center_size(
            area.center(),
            egui::vec2(
                (area.width() * 0.42).clamp(320.0, 480.0),
                row_h * 2.0 + 12.0,
            ),
        );
        // A press anywhere outside the box dismisses it. The flag comes from
        // the start of the frame, so the click that opens the question — a
        // header button, Shift+Delete — does not also count against it.
        if prompt_was_open
            && ui.input(|i| i.pointer.any_pressed())
            && !ui
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| dialog.contains(p))
        {
            self.tree.prompt = None;
        }
        if let Some(prompt) = self.tree.prompt.clone() {
            let painter = ui.painter_at(area);
            painter.rect_filled(dialog, 5.0, style.panel_bg);
            painter.rect_stroke(
                dialog,
                5.0,
                egui::Stroke::new(1.0, style.border),
                egui::StrokeKind::Inside,
            );
            let (label, mut text, is_delete) = match &prompt {
                TreePrompt::New { name, is_dir, .. } => (
                    if *is_dir { "new folder" } else { "new file" },
                    name.clone(),
                    false,
                ),
                TreePrompt::Rename { name, .. } => ("rename", name.clone(), false),
                TreePrompt::Delete { path, dir } => (
                    if *dir {
                        "permanently delete folder"
                    } else {
                        "permanently delete file"
                    },
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    true,
                ),
            };
            painter.text(
                egui::pos2(dialog.min.x + 8.0, dialog.min.y + 10.0),
                egui::Align2::LEFT_CENTER,
                label,
                font.clone(),
                style.dim,
            );
            let mut enter = false;
            let mut escape = false;
            if is_delete {
                painter.text(
                    egui::pos2(dialog.min.x + 8.0, dialog.min.y + row_h + 10.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{text} — are you sure? Enter deletes for good, Esc cancels"),
                    font.clone(),
                    style.error,
                );
                enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            } else {
                let field = egui::Rect::from_min_size(
                    egui::pos2(dialog.min.x + 8.0, dialog.min.y + row_h + 4.0),
                    egui::vec2(dialog.width() - 16.0, row_h),
                );
                let response = ui.put(
                    field,
                    egui::TextEdit::singleline(&mut text).font(font.clone()),
                );
                response.request_focus();
                enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                match &mut self.tree.prompt {
                    Some(TreePrompt::New { name, .. }) => *name = text.clone(),
                    Some(TreePrompt::Rename { name, .. }) => *name = text.clone(),
                    _ => {}
                }
            }
            if enter {
                self.tree.prompt = None;
                let result = match &prompt {
                    TreePrompt::New { dir, is_dir, .. } => self.tree.create(dir, &text, *is_dir),
                    TreePrompt::Rename { path, .. } => self.tree.rename(path, &text),
                    TreePrompt::Delete { path, dir } => self
                        .tree
                        .delete(path, *dir)
                        .map(|()| std::path::PathBuf::new()),
                };
                match result {
                    Ok(path) if path.as_os_str().is_empty() => {
                        // The deleted entry may have open buffers: without this
                        // they keep editing, and the next save quietly
                        // resurrects the file.
                        let detached =
                            if let TreePrompt::Delete { path, .. } = &prompt {
                                self.tree_deleted(path)
                            } else {
                                false
                            };
                        if !detached {
                            self.say("deleted for good");
                        }
                    }
                    Ok(path) => {
                        if let TreePrompt::Rename { path: old, .. } = &prompt {
                            self.buffers_follow(old, &path);
                        }
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.say(format!("{name}"));
                    }
                    Err(e) => self.say(format!("{e}")),
                }
            } else if escape {
                self.tree.prompt = None;
            }
        }
    }

    /// The status line under the panes.
    pub(crate) fn draw_status(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        style: &EditorStyle,
        font: &egui::FontId,
        metrics: Metrics,
    ) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, style.panel_wash);
        painter.line_segment(
            [
                egui::pos2(rect.min.x, rect.min.y),
                egui::pos2(rect.max.x, rect.min.y),
            ],
            egui::Stroke::new(1.0, style.border),
        );
        let y = rect.center().y;
        let mut x = rect.min.x + 10.0;
        let mode = self.mode.label();
        let mode_color = match self.mode {
            Mode::Insert => style.accent,
            Mode::Visual(_) => style.warn,
            Mode::Command(_) => style.warn,
            _ => style.fg,
        };
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            mode,
            font.clone(),
            mode_color,
        );
        x += metrics.cell_w * (mode.len() as f32 + 2.0);

        // The buffer name, with a dot when it has unsaved changes. The
        // no-file state says so once here, instead of `[scratch]` twice —
        // once as the title and once as the location.
        let buf = self.buffer();
        let scratch = buf.scratch;
        let title = if scratch {
            "no file selected".to_owned()
        } else {
            buf.title()
        };
        let dirty = if buf.dirty { " ●" } else { "" };
        let remote = if buf.is_remote() { " ⇅" } else { "" };
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            format!("{title}{dirty}{remote}"),
            font.clone(),
            if buf.dirty { style.warn } else { style.fg },
        );
        x += metrics.cell_w * (title.len() + dirty.len() + remote.len() + 2) as f32;

        // Where it lives: local path or host:path.
        if !scratch {
            let location = buf.location();
            painter.text(
                egui::pos2(x, y),
                egui::Align2::LEFT_CENTER,
                &location,
                font.clone(),
                style.dim,
            );
        }

        // The right-hand side: line:col, language, EOL, and any warning.
        let cursor = buf.cursors.first().copied().unwrap_or_default();
        let mut right = format!(
            "{}:{}  {}  {}",
            cursor.pos.line + 1,
            cursor.pos.col + 1,
            self.language(),
            buf.eol.label()
        );
        if buf.cursors.len() > 1 {
            right = format!("{} cursors  {right}", buf.cursors.len());
        }
        if buf.large {
            right = format!("large file  {right}");
        }
        if let Some((_, change)) = self
            .external
            .iter()
            .find(|(i, _)| *i == self.active)
            .map(|(i, c)| (*i, *c))
        {
            right = match change {
                ExternalChange::Changed => format!("changed on disk  {right}"),
                ExternalChange::Gone => format!("deleted on disk  {right}"),
                ExternalChange::None => right,
            };
        }
        painter.text(
            egui::pos2(rect.max.x - 10.0, y),
            egui::Align2::RIGHT_CENTER,
            right,
            font.clone(),
            style.dim,
        );

        // A message replaces the middle of the line while it lasts.
        if let Some((text, at)) = &self.message
            && self.now - at < MESSAGE_SECS
        {
            painter.text(
                egui::pos2(rect.center().x, y),
                egui::Align2::CENTER_CENTER,
                text,
                font.clone(),
                style.accent,
            );
        }
    }

    /// The find panel, the palette and the completion popup.
    pub(crate) fn draw_overlays(
        &mut self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        style: &EditorStyle,
        font: &egui::FontId,
        metrics: Metrics,
    ) {
        if self.find.open {
            self.draw_find(ui, area, style, font, metrics);
        }
        if self.palette.is_some() {
            self.draw_palette(ui, area, style, font, metrics);
        }
        if self.completion.is_some() {
            self.draw_completion(ui, area, style, font, metrics);
        }
        if self.mode.is_command() {
            self.draw_command_line(ui, area, style, font, metrics);
        }
        if self.close_prompt.is_some() {
            self.draw_close_prompt(ui, area, style, font, metrics);
        }
    }

    /// The question asked before closing a buffer with unsaved changes.
    ///
    /// It is the last thing drawn, over everything else, because nothing else
    /// in the editor should be reachable while it is up.
    fn draw_close_prompt(
        &mut self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        style: &EditorStyle,
        font: &egui::FontId,
        metrics: Metrics,
    ) {
        let Some(prompt) = self.close_prompt.clone() else {
            return;
        };
        let title = self
            .buffers
            .get(prompt.buffer)
            .map(|b| b.title())
            .unwrap_or_default();
        let row_h = metrics.cell_h + 6.0;
        let width = 420.0f32.min(area.width() - 24.0);
        let card = egui::Rect::from_center_size(area.center(), egui::vec2(width, row_h * 3.0 + 18.0));
        let painter = ui.painter_at(area);
        // Dim what is behind, so the question is the only thing in focus.
        painter.rect_filled(area, 0.0, egui::Color32::from_black_alpha(96));
        painter.rect_filled(card, 8.0, style.panel_bg);
        painter.rect_stroke(
            card,
            8.0,
            egui::Stroke::new(1.0, style.border),
            egui::StrokeKind::Inside,
        );
        painter.text(
            egui::pos2(card.min.x + 14.0, card.min.y + row_h * 0.5 + 6.0),
            egui::Align2::LEFT_CENTER,
            format!("Save changes to {title}?"),
            font.clone(),
            style.fg,
        );

        // The buttons, laid out left to right.
        let mut labels: Vec<&str> = Vec::new();
        if prompt.can_save {
            labels.push("Save");
        }
        labels.push("Discard");
        labels.push("Cancel");
        let mut x = card.min.x + 14.0;
        let mut buttons: Vec<(&str, egui::Rect)> = Vec::new();
        for label in labels {
            let w = metrics.cell_w * (label.len() as f32 + 4.0) + 16.0;
            let rect = egui::Rect::from_min_size(
                egui::pos2(x, card.min.y + row_h + 8.0),
                egui::vec2(w, row_h),
            );
            buttons.push((label, rect));
            x += w + 8.0;
        }
        let mut answer: Option<&str> = None;
        for (label, rect) in &buttons {
            let clicked = ui
                .interact(*rect, ui.id().with(("close-prompt", *label)), egui::Sense::click())
                .clicked();
            let color = match *label {
                "Save" => style.accent,
                "Discard" => style.error,
                _ => style.fg,
            };
            painter.rect_filled(*rect, 5.0, style.panel_wash);
            painter.rect_stroke(
                *rect,
                5.0,
                egui::Stroke::new(1.0, style.border),
                egui::StrokeKind::Inside,
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                *label,
                font.clone(),
                color,
            );
            if clicked {
                answer = Some(label);
            }
        }
        // The keys, so the question can be answered without the mouse.
        let hint = if prompt.can_save {
            "enter save · d discard · esc cancel"
        } else {
            "d discard · esc cancel"
        };
        painter.text(
            egui::pos2(card.min.x + 14.0, card.max.y - row_h * 0.5 - 4.0),
            egui::Align2::LEFT_CENTER,
            hint,
            font.clone(),
            style.dim,
        );

        match answer {
            Some("Save") => self.answer_close(true),
            Some("Discard") => self.answer_close(false),
            Some(_) => self.close_prompt = None,
            None => {}
        }
    }

    /// The find/replace bar.
    fn draw_find(
        &mut self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        style: &EditorStyle,
        font: &egui::FontId,
        metrics: Metrics,
    ) {
        let height = metrics.cell_h + 16.0;
        let width = (area.width() - 24.0).min(560.0);
        let rect = egui::Rect::from_min_size(
            egui::pos2(area.max.x - width - 12.0, area.min.y + 8.0),
            egui::vec2(
                width,
                if self.find.replace_open {
                    height * 2.0
                } else {
                    height
                },
            ),
        );
        let painter = ui.painter_at(area);
        painter.rect_filled(rect, 6.0, style.panel_bg);
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.0, style.border),
            egui::StrokeKind::Inside,
        );
        let field_h = metrics.cell_h + 8.0;
        let query_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 8.0, rect.min.y + 4.0),
            egui::vec2(rect.width() - 16.0, field_h),
        );
        let response = ui.put(
            query_rect,
            egui::TextEdit::singleline(&mut self.find.query)
                .font(font.clone())
                .hint_text("find"),
        );
        if response.changed() {
            self.refresh_find();
        }
        if self.find.replace_open {
            let replace_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + 8.0, rect.min.y + field_h + 6.0),
                egui::vec2(rect.width() - 16.0, field_h),
            );
            ui.put(
                replace_rect,
                egui::TextEdit::singleline(&mut self.find.replacement)
                    .font(font.clone())
                    .hint_text("replace"),
            );
        }
        let count = if self.find.matches == 0 {
            "no matches".to_owned()
        } else {
            format!("{} of {}", self.find.current + 1, self.find.matches)
        };
        painter.text(
            egui::pos2(rect.max.x - 10.0, rect.max.y - 8.0),
            egui::Align2::RIGHT_BOTTOM,
            count,
            font.clone(),
            style.dim,
        );
    }

    /// The command palette.
    fn draw_palette(
        &mut self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        style: &EditorStyle,
        font: &egui::FontId,
        metrics: Metrics,
    ) {
        let width = (area.width() - 40.0).min(620.0);
        let row_h = metrics.cell_h + 6.0;
        let items = self
            .palette
            .as_ref()
            .map(|p| p.filtered().len())
            .unwrap_or(0);
        let height = (row_h * (items.min(12) as f32 + 1.0) + 16.0).min(area.height() - 40.0);
        let rect = egui::Rect::from_min_size(
            egui::pos2(area.center().x - width * 0.5, area.min.y + 40.0),
            egui::vec2(width, height),
        );
        let painter = ui.painter_at(area);
        painter.rect_filled(rect, 8.0, style.panel_bg);
        painter.rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(1.0, style.border),
            egui::StrokeKind::Inside,
        );
        let field = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 8.0, rect.min.y + 6.0),
            egui::vec2(rect.width() - 16.0, metrics.cell_h + 8.0),
        );
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        let response = ui.put(
            field,
            egui::TextEdit::singleline(&mut palette.query)
                .font(font.clone())
                .hint_text("command"),
        );
        if response.changed() {
            palette.selected = 0;
        }
        let filtered: Vec<(String, String)> = palette
            .filtered()
            .iter()
            .map(|i| (i.label.clone(), i.detail.clone()))
            .collect();
        let selected = palette.selected;
        for (i, (label, detail)) in filtered.iter().take(12).enumerate() {
            let y = field.max.y + 6.0 + i as f32 * row_h;
            let row = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + 6.0, y),
                egui::vec2(rect.width() - 12.0, row_h),
            );
            if i == selected {
                painter.rect_filled(row, 4.0, style.selection);
            }
            painter.text(
                egui::pos2(row.min.x + 8.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                font.clone(),
                style.fg,
            );
            painter.text(
                egui::pos2(row.max.x - 8.0, row.center().y),
                egui::Align2::RIGHT_CENTER,
                detail,
                font.clone(),
                style.dim,
            );
        }
    }

    /// The completion popup.
    fn draw_completion(
        &mut self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        style: &EditorStyle,
        font: &egui::FontId,
        metrics: Metrics,
    ) {
        let Some(completion) = self.completion.as_ref() else {
            return;
        };
        let row_h = metrics.cell_h + 4.0;
        let width = 260.0;
        let height = row_h * completion.items.len().min(8) as f32 + 8.0;
        let rect = egui::Rect::from_min_size(
            egui::pos2(area.min.x + 40.0, area.center().y),
            egui::vec2(width, height),
        );
        let painter = ui.painter_at(area);
        painter.rect_filled(rect, 6.0, style.panel_bg);
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.0, style.border),
            egui::StrokeKind::Inside,
        );
        for (i, item) in completion.items.iter().take(8).enumerate() {
            let y = rect.min.y + 4.0 + i as f32 * row_h;
            if i == completion.selected {
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(rect.min.x + 4.0, y),
                        egui::vec2(rect.width() - 8.0, row_h),
                    ),
                    3.0,
                    style.selection,
                );
            }
            painter.text(
                egui::pos2(rect.min.x + 10.0, y + row_h * 0.5),
                egui::Align2::LEFT_CENTER,
                item,
                font.clone(),
                style.fg,
            );
        }
    }

    /// The `:` or `/` line, drawn over the status line.
    fn draw_command_line(
        &mut self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        style: &EditorStyle,
        font: &egui::FontId,
        metrics: Metrics,
    ) {
        let height = metrics.cell_h + 10.0;
        let rect = egui::Rect::from_min_size(
            egui::pos2(area.min.x, area.max.y - height),
            egui::vec2(area.width(), height),
        );
        let painter = ui.painter_at(area);
        painter.rect_filled(rect, 0.0, style.panel_bg);
        let prefix = match self.mode {
            Mode::Command(super::mode::CommandKind::Ex) => ":",
            Mode::Command(super::mode::CommandKind::SearchForward) => "/",
            Mode::Command(super::mode::CommandKind::SearchBackward) => "?",
            _ => ":",
        };
        painter.text(
            egui::pos2(rect.min.x + 10.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("{prefix}{}", self.command_line),
            font.clone(),
            style.fg,
        );
        painter.text(
            egui::pos2(rect.max.x - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "Enter runs · Esc cancels",
            font.clone(),
            style.dim,
        );
    }

    /// Recompute the find panel's match count.
    pub(crate) fn refresh_find(&mut self) {
        let query = super::Query {
            text: self.find.query.clone(),
            case_sensitive: self.find.case_sensitive,
            whole_word: self.find.whole_word,
            regex: self.find.regex,
        };
        let matches = super::search::find_all(self.buffer(), &query).len();
        self.find.matches = matches;
        if matches == 0 {
            self.find.current = 0;
        } else {
            self.find.current = self.find.current.min(matches - 1);
        }
    }

    /// Move the cursor to the find panel's current match.
    pub(crate) fn find_step(&mut self, forward: bool) {
        let query = super::Query {
            text: self.find.query.clone(),
            case_sensitive: self.find.case_sensitive,
            whole_word: self.find.whole_word,
            regex: self.find.regex,
        };
        if query.is_empty() {
            return;
        }
        let buf = self.buffer_mut();
        let from = buf.cursors.first().map_or(Pos::new(0, 0), |c| c.pos);
        let Some(span) = super::search::find_next(buf, &query, from, !forward) else {
            return;
        };
        buf.cursors = vec![super::pos::Cursor::new(span.start)];
        let line = span.start.line as f32;
        if line < buf.scroll[1] || line > buf.scroll[1] + 20.0 {
            buf.scroll[1] = (line - 6.0).max(0.0);
        }
    }

    /// Replace the current match, or all of them.
    pub(crate) fn find_replace(&mut self, all: bool) {
        let query = super::Query {
            text: self.find.query.clone(),
            case_sensitive: self.find.case_sensitive,
            whole_word: self.find.whole_word,
            regex: self.find.regex,
        };
        if query.is_empty() {
            return;
        }
        let replacement = self.find.replacement.clone();
        if all {
            let count = super::search::replace_all(self.buffer_mut(), &query, &replacement);
            self.say(format!("{count} replacements"));
            self.refresh_find();
            return;
        }
        let buf = self.buffer_mut();
        let from = buf.cursors.first().map_or(Pos::new(0, 0), |c| c.pos);
        let Some(span) = super::search::find_next(buf, &query, from, false) else {
            return;
        };
        buf.replace(span, &replacement, false);
        self.refresh_find();
    }

    /// Run the palette's selected item.
    pub(crate) fn palette_accept(&mut self) {
        let Some(item) = self.palette.as_ref().and_then(|p| p.current()) else {
            self.palette = None;
            return;
        };
        self.palette = None;
        match item.action {
            PaletteAction::Command(cmd) => self.execute(cmd),
            PaletteAction::OpenPath(path) => {
                self.open_path(&path);
            }
            PaletteAction::GotoLine(line) => self.goto_line(line),
            PaletteAction::SwitchBuffer(index) => self.focus_buffer(index),
            PaletteAction::Keybindings => self.wants_keybindings = true,
            PaletteAction::Settings => self.wants_settings = true,
            PaletteAction::Split(dir) => self.split(dir),
        }
    }

    /// Accept the selected completion.
    pub(crate) fn completion_accept(&mut self) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let Some(item) = completion.items.get(completion.selected).cloned() else {
            return;
        };
        let buf = self.buffer_mut();
        let start = completion.start;
        let end = Pos::new(start.line, start.col + completion.prefix.chars().count());
        buf.replace(super::pos::Span::new(start, end), &item, false);
    }

    /// Ask for completions at the cursor: words from the buffer, for now.
    pub(crate) fn request_completion(&mut self) {
        let buf = self.buffer();
        let cursor = buf.cursors.first().copied().unwrap_or_default();
        let line = buf.line(cursor.pos.line);
        let chars: Vec<char> = line.chars().collect();
        let mut start = cursor.pos.col.min(chars.len());
        while start > 0 {
            let c = chars[start - 1];
            if c.is_alphanumeric() || c == '_' {
                start -= 1;
            } else {
                break;
            }
        }
        let prefix: String = chars[start..cursor.pos.col.min(chars.len())]
            .iter()
            .collect();
        if prefix.len() < 2 {
            self.say("nothing to complete");
            return;
        }
        // Every word in the buffer that starts with the prefix.
        let mut words: Vec<String> = Vec::new();
        for line in 0..buf.line_count() {
            for word in buf
                .line(line)
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            {
                if word.len() > prefix.len()
                    && word.starts_with(&prefix)
                    && !words.iter().any(|w| w == word)
                {
                    words.push(word.to_owned());
                }
                if words.len() >= 200 {
                    break;
                }
            }
        }
        if words.is_empty() {
            self.say(format!("no completion for {prefix}"));
            return;
        }
        words.sort();
        self.completion = Some(super::Completion {
            items: words,
            selected: 0,
            prefix,
            start: Pos::new(cursor.pos.line, start),
        });
    }
}
