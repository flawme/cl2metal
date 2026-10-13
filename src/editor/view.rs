//! Drawing the editor: panes, text, selections and cursors.
//!
//! Text is laid out by egui (so Unicode shaping is egui's problem, not ours)
//! while geometry — cursor and selection rectangles, hit testing — comes from a
//! column map that expands tabs to the next tab stop and counts wide glyphs as
//! two cells. Both sides use the same map, so they cannot drift apart.

use super::Editor;
use super::buffer::Buffer;
use super::mode::Mode;
use super::motion::{self, Motion};
use super::operator;
use super::pos::{Pos, Span, VisualKind};
use super::syntax::Style as SynStyle;

/// The colours the editor draws with, taken from the application theme.
#[derive(Clone, Copy)]
pub(crate) struct EditorStyle {
    /// The surface behind the text. Transparent: the application paints one
    /// shared backdrop for the whole window and the editor must not tint it a
    /// second time.
    pub(crate) bg: egui::Color32,
    pub(crate) fg: egui::Color32,
    pub(crate) dim: egui::Color32,
    pub(crate) accent: egui::Color32,
    pub(crate) selection: egui::Color32,
    pub(crate) current_line: egui::Color32,
    /// The light wash the large side surfaces are drawn with: the file tree,
    /// the gutter, the status line.
    pub(crate) panel_wash: egui::Color32,
    pub(crate) border: egui::Color32,
    pub(crate) match_bg: egui::Color32,
    /// Popups and dialogs, which must stay readable over any wallpaper.
    pub(crate) panel_bg: egui::Color32,
    pub(crate) warn: egui::Color32,
    pub(crate) error: egui::Color32,
}

/// The monospace cell size.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Metrics {
    pub(crate) cell_w: f32,
    pub(crate) cell_h: f32,
}

impl Metrics {
    pub(crate) fn measure(ui: &egui::Ui, font: &egui::FontId) -> Self {
        let (cell_w, cell_h) = ui
            .ctx()
            .fonts_mut(|f| (f.glyph_width(font, 'M'), f.row_height(font)));
        Self {
            cell_w: cell_w.max(1.0),
            cell_h: cell_h.max(1.0),
        }
    }
}

/// The display width of a character in cells.
pub(crate) fn display_width(c: char) -> usize {
    if c == '\t' {
        return 1;
    }
    unicode_width::UnicodeWidthChar::width(c)
        .unwrap_or(0)
        .max(1)
}

/// The cell column of every character index, plus the end column.
///
/// `cols[i]` is where character `i` starts; `cols[len]` is the line's width in
/// cells. Tabs advance to the next tab stop.
pub(crate) fn column_map(line: &str, tab_width: usize) -> Vec<usize> {
    let tab_width = tab_width.max(1);
    let mut cols = Vec::with_capacity(line.chars().count() + 1);
    let mut col = 0usize;
    for c in line.chars() {
        cols.push(col);
        if c == '\t' {
            col += tab_width - (col % tab_width);
        } else {
            col += display_width(c);
        }
    }
    cols.push(col);
    cols
}

/// Expand tabs in a piece of a line, starting at `start_col`.
fn expand_tabs(text: &str, start_col: usize, tab_width: usize) -> String {
    let tab_width = tab_width.max(1);
    let mut out = String::with_capacity(text.len());
    let mut col = start_col;
    for c in text.chars() {
        if c == '\t' {
            let spaces = tab_width - (col % tab_width);
            for _ in 0..spaces {
                out.push(' ');
            }
            col += spaces;
        } else {
            out.push(c);
            col += display_width(c);
        }
    }
    out
}

/// The line ranges a selection covers, as `(line, start_col, end_col)`.
pub(crate) fn selection_runs(
    buf: &Buffer,
    span: Span,
    kind: VisualKind,
    anchor_col: usize,
    cursor_col: usize,
) -> Vec<(usize, usize, usize)> {
    match kind {
        VisualKind::Line => {
            let start = span.start.line;
            let end = span.end.line;
            (start..=end)
                .map(|line| (line, 0, buf.line_len(line).max(1)))
                .collect()
        }
        VisualKind::Block => {
            let (start, end) = (span.start.line, span.end.line);
            let (a, b) = if anchor_col <= cursor_col {
                (anchor_col, cursor_col)
            } else {
                (cursor_col, anchor_col)
            };
            (start..=end)
                .map(|line| {
                    let len = buf.line_len(line);
                    (line, a.min(len), (b + 1).min(len).max(a.min(len)))
                })
                .collect()
        }
        VisualKind::Char => {
            if span.start.line == span.end.line {
                vec![(span.start.line, span.start.col, span.end.col)]
            } else {
                let mut runs = Vec::new();
                runs.push((
                    span.start.line,
                    span.start.col,
                    buf.line_len(span.start.line),
                ));
                for line in span.start.line + 1..span.end.line {
                    runs.push((line, 0, buf.line_len(line)));
                }
                runs.push((span.end.line, 0, span.end.col));
                runs
            }
        }
    }
}

/// A pane's geometry: where the text starts and how much fits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PaneGeometry {
    pub(crate) rect: egui::Rect,
    pub(crate) text: egui::Rect,
    pub(crate) metrics: Metrics,
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) gutter: f32,
}

impl Editor {
    /// Draw the whole editor into `area`.
    pub(crate) fn draw(
        &mut self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        style: &EditorStyle,
        font: egui::FontId,
    ) {
        ui.painter().rect_filled(area, 0.0, style.bg);
        let metrics = Metrics::measure(ui, &font);
        // Whether the tree was already asking something when the frame began:
        // the click that opens its question must not also count as a click
        // outside it. Passed on to the prompt, which draws late.
        let prompt_was_open = self.tree.prompt.is_some();

        // The file tree takes a column on the left when it is open. The grip
        // on its edge drags the column wider or narrower, the way a code
        // editor's sidebar does.
        const MIN_TREE_WIDTH: f32 = 140.0;
        let mut pane_area = area;
        if self.tree.open {
            let width = self
                .tree
                .width
                .clamp(MIN_TREE_WIDTH, area.width() * 0.6);
            self.tree.width = width;
            let tree_rect = egui::Rect::from_min_size(area.min, egui::vec2(width, area.height()));
            self.draw_tree(ui, tree_rect, style, &font, metrics);
            pane_area =
                egui::Rect::from_min_max(egui::pos2(tree_rect.max.x + 1.0, area.min.y), area.max);
            // The grip: a drag strip straddling the tree's edge. Saving is
            // asked for when the drag ends, so a drag in motion does not
            // write the config on every frame.
            let grip = egui::Rect::from_min_max(
                egui::pos2(tree_rect.max.x - 3.0, area.min.y),
                egui::pos2(tree_rect.max.x + 3.0, area.max.y),
            );
            let grip_id = ui.id().with("editor-tree-width");
            let response = ui
                .interact(grip, grip_id, egui::Sense::drag())
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            if response.dragged()
                && let Some(p) = response.interact_pointer_pos()
            {
                self.tree.width = (p.x - area.min.x).clamp(MIN_TREE_WIDTH, area.width() * 0.6);
            }
            if response.drag_stopped() {
                self.wants_width_save = true;
            }
            if response.hovered() || response.dragged() {
                ui.painter()
                    .rect_filled(grip, 0.0, style.accent.gamma_multiply(0.5));
            }
            ui.painter().line_segment(
                [
                    egui::pos2(tree_rect.max.x, area.min.y),
                    egui::pos2(tree_rect.max.x, area.max.y),
                ],
                egui::Stroke::new(1.0, style.border),
            );
        }

        // The status line sits at the bottom of the editor area.
        let status_h = metrics.cell_h + 8.0;
        let status_rect = egui::Rect::from_min_max(
            egui::pos2(pane_area.min.x, pane_area.max.y - status_h),
            pane_area.max,
        );
        let panes_rect = egui::Rect::from_min_max(
            pane_area.min,
            egui::pos2(pane_area.max.x, status_rect.min.y),
        );

        let focused = self.layout.focused();
        let rects = self.layout.rects(panes_rect);
        for (index, rect) in &rects {
            let is_focused = *index == focused;
            self.draw_pane(ui, *index, *rect, style, &font, metrics, is_focused);
        }
        // Dividers, draggable.
        for (path, bar) in self.layout.dividers(panes_rect) {
            let response = ui.interact(
                bar,
                ui.id().with(("editor-divider", path.clone())),
                egui::Sense::drag(),
            );
            if response.hovered() || response.dragged() {
                ui.painter()
                    .rect_filled(bar, 0.0, style.accent.gamma_multiply(0.5));
            }
            if response.dragged() {
                let parent = &path[..path.len().saturating_sub(1)];
                let area = divider_area(&self.layout, panes_rect, parent);
                let dir = divider_dir(&self.layout, parent);
                if let Some(dir) = dir {
                    let ratio = match dir {
                        super::SplitDir::Right => response
                            .interact_pointer_pos()
                            .map_or(0.5, |p| (p.x - area.min.x) / area.width().max(1.0)),
                        super::SplitDir::Down => response
                            .interact_pointer_pos()
                            .map_or(0.5, |p| (p.y - area.min.y) / area.height().max(1.0)),
                    };
                    self.layout.set_ratio(parent, ratio);
                }
            }
        }

        self.draw_status(ui, status_rect, style, &font, metrics);
        // The tree's question, after the panes so it rides over them.
        self.draw_tree_prompt(ui, area, style, &font, metrics, prompt_was_open);
        self.draw_overlays(ui, area, style, &font, metrics);
        // Last, so it sits over the empty pane a long read leaves behind.
        self.draw_loading(ui, area, style, &font);
    }

    /// The indicator shown while a file is being read on a worker thread.
    ///
    /// A long file takes long enough that the window would otherwise look
    /// frozen: the read runs off the UI thread, and this is what says so.
    fn draw_loading(
        &self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        style: &EditorStyle,
        font: &egui::FontId,
    ) {
        let Some(loading) = &self.loading else {
            return;
        };
        // The spinner is the only thing moving, so keep frames coming.
        ui.ctx().request_repaint();
        let elapsed = loading.elapsed(self.now);
        let painter = ui.painter_at(area);
        let center = area.center();
        let radius = 13.0;
        let segments = 12;
        // A comet: the head is bright and the tail fades, turning once a second.
        let turns = (elapsed * 1.1) as f32;
        for i in 0..segments {
            let t = i as f32 / segments as f32;
            let alpha = 0.10 + 0.90 * (t - turns).rem_euclid(1.0).powi(3);
            let angle = t * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            let at = center + egui::vec2(angle.cos(), angle.sin()) * radius;
            painter.circle_filled(at, 2.0, crate::ui::with_alpha(style.accent, alpha));
        }
        // The name, and the elapsed time once the wait is long enough to
        // wonder about.
        let label = if elapsed >= 0.3 {
            format!("opening {} — {elapsed:.1}s", loading.name())
        } else {
            format!("opening {}", loading.name())
        };
        painter.text(
            egui::pos2(center.x, center.y + radius + 16.0),
            egui::Align2::CENTER_CENTER,
            label,
            font.clone(),
            style.dim,
        );
    }

    /// Draw one pane: gutter, text, selections, cursors.
    fn draw_pane(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        rect: egui::Rect,
        style: &EditorStyle,
        font: &egui::FontId,
        metrics: Metrics,
        focused: bool,
    ) {
        // The no-file pane: no gutter, no cursor — a centered invitation
        // instead, so an empty editor does not dress up as a file with one
        // blank line. Only when the pane is genuinely empty; text that ended
        // up here some other way still gets drawn.
        {
            let buf = self.buffer_at(index);
            if buf.scratch && buf.text().is_empty() {
                let painter = ui.painter_at(rect);
                let center = rect.center();
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    "no file selected",
                    font.clone(),
                    style.dim,
                );
                let hint = egui::FontId {
                    size: font.size * 0.8,
                    ..font.clone()
                };
                painter.text(
                    egui::pos2(center.x, center.y + font.size * 2.0),
                    egui::Align2::CENTER_CENTER,
                    "open one from the tree · drop a file on the window",
                    hint,
                    crate::ui::with_alpha(style.dim, 0.75),
                );
                return;
            }
        }
        let gutter = if self.settings.line_numbers {
            metrics.cell_w * 5.0 + 12.0
        } else {
            8.0
        };
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + gutter, rect.min.y + 4.0),
            egui::pos2(rect.max.x - 10.0, rect.max.y - 4.0),
        );
        let rows = ((text_rect.height() / metrics.cell_h).floor() as usize).max(1);
        let cols = ((text_rect.width() / metrics.cell_w).floor() as usize).max(1);
        let geometry = PaneGeometry {
            rect,
            text: text_rect,
            metrics,
            rows,
            cols,
            gutter,
        };
        // Follow the cursor only when it actually moved. Running this every
        // frame fights the wheel: scrolling down would be undone the moment
        // the cursor left the scrolloff margin, so the view snapped back.
        if focused {
            self.follow_cursor(index, geometry);
        }
        self.ensure_spans(index);
        if self.syntax_rx.is_some() {
            // A highlight job is running: come back for its result.
            ui.ctx().request_repaint();
        }

        let tab_width = self.settings.tab_width;
        // Ease the drawn offset toward the target, so a touchpad's many small
        // deltas become motion instead of jumps.
        let dt = self.frame_dt.clamp(0.0, 0.1);
        {
            let buf = &mut self.buffers[index];
            let k = 1.0 - (-dt * 26.0).exp();
            buf.scroll_display += (buf.scroll[1] - buf.scroll_display) * k;
            if (buf.scroll_display - buf.scroll[1]).abs() > 0.005 {
                ui.ctx().request_repaint();
            }
        }
        let buf = &self.buffers[index];
        let top_f = buf.scroll_display.max(0.0);
        let top = top_f.floor() as usize;
        let sub = (top_f - top as f32) * metrics.cell_h;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, style.bg);
        if focused {
            painter.rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(1.0, style.border),
                egui::StrokeKind::Inside,
            );
        }

        // Gutter background and line numbers.
        if self.settings.line_numbers {
            let gutter_rect = egui::Rect::from_min_max(
                rect.min,
                egui::pos2(rect.min.x + gutter - 6.0, rect.max.y),
            );
            painter.rect_filled(gutter_rect, 0.0, style.panel_wash);
        }

        let cursor_line = buf.cursors.first().map_or(0, |c| c.pos.line);
        // The selections, with visual mode's inclusive end already applied.
        let visual: Vec<(Span, VisualKind, usize)> = {
            let kind = match self.mode {
                Mode::Visual(kind) => kind,
                _ => VisualKind::Char,
            };
            buf.cursors
                .iter()
                .filter_map(|c| {
                    let span = c.selection()?;
                    let span = match kind {
                        VisualKind::Char => {
                            Span::new(span.start, motion::step(buf, span.end, Motion::Right))
                        }
                        VisualKind::Line => operator::linewise_span(buf, span),
                        VisualKind::Block => span,
                    };
                    Some((span, kind, c.anchor.map_or(c.pos.col, |a| a.col)))
                })
                .collect()
        };
        let word = if self.settings.highlight_word {
            word_at(
                buf,
                cursor_line,
                buf.cursors.first().map_or(0, |c| c.pos.col),
            )
        } else {
            None
        };
        let bracket = super::motion::enclosing_brackets(
            buf,
            buf.cursors.first().map_or(Pos::new(0, 0), |c| c.pos),
        );

        for row in 0..rows {
            let line = top + row;
            if line >= buf.line_count() {
                break;
            }
            let y = text_rect.min.y + row as f32 * metrics.cell_h - sub;
            let line_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x, y),
                egui::vec2(rect.width(), metrics.cell_h),
            );
            let cols_map = column_map(buf.line(line), tab_width);

            // Current line.
            if focused && self.settings.highlight_current_line && line == cursor_line {
                painter.rect_filled(line_rect, 0.0, style.current_line);
            }

            // Selections.
            for (span, kind, anchor_col) in &visual {
                for (run_line, start, end) in
                    selection_runs(buf, *span, *kind, *anchor_col, span.end.col)
                {
                    if run_line != line {
                        continue;
                    }
                    let x0 = text_rect.min.x
                        + cols_map[start.min(cols_map.len() - 1)] as f32 * metrics.cell_w;
                    let x1 = text_rect.min.x
                        + cols_map[end.min(cols_map.len() - 1)] as f32 * metrics.cell_w;
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(x0, y),
                            egui::pos2(x1.max(x0 + 2.0), y + metrics.cell_h),
                        ),
                        0.0,
                        style.selection,
                    );
                }
            }

            // Every occurrence of the word under the cursor.
            if focused
                && let Some(word) = &word
                && word.len() > 1
            {
                let text = buf.line(line);
                let mut from = 0;
                while let Some(at) = text[from..].find(word.as_str()) {
                    let start = from + at;
                    let end = start + word.len();
                    let c0 = buf.col_of_byte(line, start);
                    let c1 = buf.col_of_byte(line, end);
                    let x0 = text_rect.min.x
                        + cols_map[c0.min(cols_map.len() - 1)] as f32 * metrics.cell_w;
                    let x1 = text_rect.min.x
                        + cols_map[c1.min(cols_map.len() - 1)] as f32 * metrics.cell_w;
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(x0, y),
                            egui::pos2(x1, y + metrics.cell_h),
                        ),
                        0.0,
                        style.match_bg,
                    );
                    from = end.max(start + 1);
                    if from >= text.len() {
                        break;
                    }
                }
            }

            // The matching bracket pair.
            if focused
                && let Some((open, close)) = bracket
                && (open.line == line || close.line == line)
            {
                for pos in [open, close] {
                    if pos.line != line {
                        continue;
                    }
                    let x = text_rect.min.x
                        + cols_map[pos.col.min(cols_map.len() - 1)] as f32 * metrics.cell_w;
                    painter.rect_stroke(
                        egui::Rect::from_min_size(
                            egui::pos2(x, y),
                            egui::vec2(metrics.cell_w, metrics.cell_h),
                        ),
                        0.0,
                        egui::Stroke::new(1.0, style.accent),
                        egui::StrokeKind::Inside,
                    );
                }
            }

            // The text itself.
            let job = self.line_job(index, line, font, style, tab_width);
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            painter.galley(egui::pos2(text_rect.min.x, y), galley, style.fg);

            // Whitespace markers.
            if self.settings.show_whitespace {
                for (i, c) in buf.line(line).chars().enumerate() {
                    if c != ' ' && c != '\t' {
                        continue;
                    }
                    let x = text_rect.min.x + cols_map[i] as f32 * metrics.cell_w;
                    let glyph = if c == '\t' { "→" } else { "·" };
                    painter.text(
                        egui::pos2(x, y + metrics.cell_h * 0.5),
                        egui::Align2::CENTER_CENTER,
                        glyph,
                        font.clone(),
                        style.dim.gamma_multiply(0.7),
                    );
                }
            }

            // Line numbers.
            if self.settings.line_numbers {
                let number = if self.settings.relative_numbers && line != cursor_line {
                    (line as i64 - cursor_line as i64)
                        .unsigned_abs()
                        .to_string()
                } else {
                    (line + 1).to_string()
                };
                let color = if line == cursor_line {
                    style.fg
                } else {
                    style.dim
                };
                painter.text(
                    egui::pos2(rect.min.x + gutter - 12.0, y + metrics.cell_h * 0.5),
                    egui::Align2::RIGHT_CENTER,
                    number,
                    font.clone(),
                    color,
                );
            }

            // Cursors.
            for cursor in &buf.cursors {
                if cursor.pos.line != line {
                    continue;
                }
                let col = cursor.pos.col.min(cols_map.len() - 1);
                let x = text_rect.min.x + cols_map[col] as f32 * metrics.cell_w;
                let width = if cursor.pos.col < buf.line_len(line) {
                    let w = cols_map[(col + 1).min(cols_map.len() - 1)] - cols_map[col];
                    w.max(1) as f32 * metrics.cell_w
                } else {
                    metrics.cell_w * 0.5
                };
                let cursor_rect =
                    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, metrics.cell_h));
                let active = focused && self.mode.is_insert();
                let color = if active { style.accent } else { style.fg };
                match self.settings.cursor_shape.as_str() {
                    "bar" => {
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                cursor_rect.min,
                                egui::vec2(2.0, metrics.cell_h),
                            ),
                            0.0,
                            color,
                        );
                    }
                    "underline" => {
                        painter.rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(cursor_rect.min.x, cursor_rect.max.y - 2.0),
                                cursor_rect.max,
                            ),
                            0.0,
                            color,
                        );
                    }
                    _ => {
                        painter.rect_filled(cursor_rect, 0.0, color.gamma_multiply(0.55));
                    }
                }
            }
        }

        // Scrollbar: drawn from the target offset, and draggable.
        let total = buf.line_count().max(1) as f32;
        let visible = rows as f32;
        let max_scroll = (total - visible).max(0.0);
        if max_scroll > 0.0 {
            let track = egui::Rect::from_min_max(
                egui::pos2(rect.max.x - 6.0, rect.min.y + 2.0),
                egui::pos2(rect.max.x - 2.0, rect.max.y - 2.0),
            );
            painter.rect_filled(track, 2.0, style.border.gamma_multiply(0.4));
            let frac = visible / total;
            let thumb_h = (track.height() * frac).max(12.0);
            let travel = (track.height() - thumb_h).max(1.0);
            let offset = thumb_offset(buf.scroll[1], max_scroll);
            let y = track.min.y + travel * offset;
            // A strip wider than the 4 px drawn, so the thumb is easy to grab.
            let grab = egui::Rect::from_min_max(
                egui::pos2(rect.max.x - 12.0, rect.min.y),
                egui::pos2(rect.max.x, rect.max.y),
            );
            let bar = ui.interact(
                grab,
                ui.id().with(("editor-bar", index)),
                egui::Sense::click_and_drag(),
            );
            if bar.dragged() || bar.clicked() {
                if let Some(point) = bar.interact_pointer_pos() {
                    let target = scroll_at_pointer(
                        point.y,
                        track.min.y,
                        travel,
                        thumb_h,
                        max_scroll,
                    );
                    let buf = &mut self.buffers[index];
                    buf.scroll[1] = target;
                    // The thumb is the position: no easing lag while dragging.
                    buf.scroll_display = buf.scroll[1];
                }
            }
            let hot = bar.hovered() || bar.dragged();
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(track.min.x, y),
                    egui::vec2(track.width(), thumb_h),
                ),
                2.0,
                if hot { style.accent } else { style.dim },
            );
        }

        // Mouse: click to place the cursor, drag to select. The right-hand
        // strip belongs to the scrollbar, so it is left out of this hit area.
        let text_hit = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.max.x - 12.0, rect.max.y),
        );
        let response = ui.interact(
            text_hit,
            ui.id().with(("editor-pane", index)),
            egui::Sense::click_and_drag(),
        );
        if response.clicked() || response.dragged() {
            if let Some(point) = response.interact_pointer_pos() {
                let pos = self.point_to_pos(index, point, geometry);
                let extend = response.dragged() || ui.input(|i| i.modifiers.shift);
                let kind = if ui.input(|i| i.modifiers.alt) {
                    VisualKind::Block
                } else {
                    VisualKind::Char
                };
                self.click_at(pos, extend, kind);
            }
        }
        if response.double_clicked()
            && let Some(point) = response.interact_pointer_pos()
        {
            let pos = self.point_to_pos(index, point, geometry);
            self.select_word_at(pos);
        }
        if response.triple_clicked()
            && let Some(point) = response.interact_pointer_pos()
        {
            let pos = self.point_to_pos(index, point, geometry);
            self.select_line_at(pos);
        }
        // The wheel works anywhere over the pane, scrollbar strip included.
        if ui.rect_contains_pointer(rect) {
            // Every delta counts: a touchpad sends many small ones, and a
            // threshold is what makes two-finger scrolling feel like it sticks.
            let delta = ui.input(|i| i.smooth_scroll_delta.y);
            if delta != 0.0 {
                let buf = &mut self.buffers[index];
                let max = (buf.line_count() as f32 - rows as f32).max(0.0);
                buf.scroll[1] = (buf.scroll[1] - delta / metrics.cell_h).clamp(0.0, max);
                ui.ctx().request_repaint();
            }
        }
    }

    /// The layout job for one line, with syntax colours.
    fn line_job(
        &self,
        index: usize,
        line: usize,
        font: &egui::FontId,
        style: &EditorStyle,
        tab_width: usize,
    ) -> egui::text::LayoutJob {
        let buf = &self.buffers[index];
        let text = buf.line(line);
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = f32::INFINITY;
        let spans = buf.spans.get(line).filter(|s| !s.is_empty());
        let mut col = 0usize;
        match spans {
            Some(spans) => {
                for (range, span_style) in spans {
                    let piece = text.get(range.clone()).unwrap_or("");
                    let expanded = expand_tabs(piece, col, tab_width);
                    col += expanded.chars().count();
                    job.append(&expanded, 0.0, text_format(font, span_style, style));
                }
            }
            None => {
                let expanded = expand_tabs(text, 0, tab_width);
                job.append(
                    &expanded,
                    0.0,
                    egui::TextFormat {
                        font_id: font.clone(),
                        color: style.fg,
                        ..Default::default()
                    },
                );
            }
        }
        job
    }

    /// Keep the cursor inside the visible rows.
    /// Keep the cursor on screen, but only when it actually moved.
    ///
    /// Running this every frame fights the wheel: scrolling down would be
    /// undone the moment the cursor left the scrolloff margin, so the view
    /// snapped back to the cursor instead of scrolling.
    pub(crate) fn follow_cursor(&mut self, index: usize, geometry: PaneGeometry) {
        let cursor_now = self.buffers[index].cursors.first().map(|c| c.pos);
        if self.buffers[index].scroll_anchor == cursor_now {
            return;
        }
        self.buffers[index].scroll_anchor = cursor_now;
        self.scroll_into_view(index, geometry);
    }

    fn scroll_into_view(&mut self, index: usize, geometry: PaneGeometry) {
        let off = self.settings.scroll_off;
        let buf = &mut self.buffers[index];
        let line = buf.cursors.first().map_or(0, |c| c.pos.line) as f32;
        let top = buf.scroll[1];
        let rows = geometry.rows as f32;
        if line < top + off as f32 {
            buf.scroll[1] = (line - off as f32).max(0.0);
        } else if line > top + rows - 1.0 - off as f32 {
            buf.scroll[1] = (line - rows + 1.0 + off as f32).max(0.0);
        }
    }

    /// Re-highlight a buffer when it changed.
    ///
    /// The work runs on a worker thread: the pure-Rust regex backend highlights
    /// at roughly 0.5 MB/s, which would be a visible stall on the UI thread for
    /// anything but a small file.
    pub(crate) fn ensure_spans(&mut self, index: usize) {
        // A finished job lands here.
        if let Some((job, version, rx)) = self.syntax_rx.take() {
            match rx.try_recv() {
                Ok(spans) => {
                    if let Some(buf) = self.buffers.get_mut(job)
                        && buf.version == version
                    {
                        buf.spans = spans;
                        buf.spans_version = version;
                    }
                }
                Err(flume::TryRecvError::Empty) => {
                    // Still running: keep it and ask for another frame.
                    self.syntax_rx = Some((job, version, rx));
                }
                Err(flume::TryRecvError::Disconnected) => {}
            }
        }
        let buf = &self.buffers[index];
        if buf.spans_version == buf.version || buf.large || self.syntax_rx.is_some() {
            return;
        }
        let (version, path, text) = (buf.version, buf.path.clone(), buf.text());
        let highlighter = self.highlighter.clone();
        let (tx, rx) = flume::bounded(1);
        std::thread::spawn(move || {
            let spans = highlighter.highlight(path.as_deref(), &text);
            let _ = tx.send(spans);
        });
        self.syntax_rx = Some((index, version, rx));
    }

    /// The buffer position under a point.
    pub(crate) fn point_to_pos(
        &self,
        index: usize,
        point: egui::Pos2,
        geometry: PaneGeometry,
    ) -> Pos {
        let buf = &self.buffers[index];
        let top = buf.scroll_display.max(0.0).floor() as usize;
        let row = ((point.y - geometry.text.min.y) / geometry.metrics.cell_h).floor();
        let row = row.max(0.0) as usize;
        let line = (top + row).min(buf.line_count().saturating_sub(1));
        let x = (point.x - geometry.text.min.x).max(0.0);
        let cell = x / geometry.metrics.cell_w;
        let cols = column_map(buf.line(line), self.settings.tab_width);
        let mut col = 0;
        for (i, start) in cols.iter().enumerate().take(cols.len().saturating_sub(1)) {
            if (*start as f32) <= cell {
                col = i;
            } else {
                break;
            }
        }
        Pos::new(line, col)
    }
}

/// Where the scrollbar thumb sits in its track, as a fraction of the travel.
pub(crate) fn thumb_offset(scroll: f32, max_scroll: f32) -> f32 {
    if max_scroll <= 0.0 {
        0.0
    } else {
        (scroll / max_scroll).clamp(0.0, 1.0)
    }
}

/// The scroll offset a pointer at `pointer_y` asks for, with the thumb centred
/// on the pointer.
pub(crate) fn scroll_at_pointer(
    pointer_y: f32,
    track_min_y: f32,
    travel: f32,
    thumb_h: f32,
    max_scroll: f32,
) -> f32 {
    let t = ((pointer_y - track_min_y - thumb_h * 0.5) / travel.max(1.0)).clamp(0.0, 1.0);
    t * max_scroll
}

/// The text format for a syntax span.
fn text_format(font: &egui::FontId, span: &SynStyle, _style: &EditorStyle) -> egui::TextFormat {
    let mut color = egui::Color32::from_rgb(span.fg[0], span.fg[1], span.fg[2]);
    if span.bold {
        // egui has no bold monospace family here, so bold spans are brightened.
        color = color.gamma_multiply(1.25);
    }
    egui::TextFormat {
        font_id: font.clone(),
        color,
        italics: span.italic,
        underline: if span.underline {
            egui::Stroke::new(1.0, color)
        } else {
            egui::Stroke::NONE
        },
        ..Default::default()
    }
}

/// The word under a position, for occurrence highlighting.
fn word_at(buf: &Buffer, line: usize, col: usize) -> Option<String> {
    let text = buf.line(line);
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let col = col.min(chars.len() - 1);
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if !is_word(chars[col]) {
        return None;
    }
    let mut start = col;
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && is_word(chars[end + 1]) {
        end += 1;
    }
    Some(chars[start..=end].iter().collect())
}

/// The area a split covers, for divider dragging.
fn divider_area(layout: &super::Layout, area: egui::Rect, path: &[bool]) -> egui::Rect {
    let mut rect = area;
    let mut node = &layout.root;
    for step in path {
        let super::layout::LayoutNode::Split {
            dir,
            ratio,
            first,
            second,
        } = node
        else {
            break;
        };
        let (a, b) = split_rect(rect, *dir, *ratio);
        if *step {
            rect = b;
            node = second;
        } else {
            rect = a;
            node = first;
        }
    }
    rect
}

/// The direction of the split at a path.
fn divider_dir(layout: &super::Layout, path: &[bool]) -> Option<super::SplitDir> {
    let mut node = &layout.root;
    for step in path {
        let super::layout::LayoutNode::Split { first, second, .. } = node else {
            return None;
        };
        node = if *step { second } else { first };
    }
    match node {
        super::layout::LayoutNode::Split { dir, .. } => Some(*dir),
        super::layout::LayoutNode::Leaf(_) => None,
    }
}

/// Split a rect, mirroring the layout module's geometry.
fn split_rect(area: egui::Rect, dir: super::SplitDir, ratio: f32) -> (egui::Rect, egui::Rect) {
    let half = super::layout::SPLIT_GAP * 0.5;
    match dir {
        super::SplitDir::Right => {
            let cut = area.min.x + area.width() * ratio;
            (
                egui::Rect::from_min_max(area.min, egui::pos2(cut - half, area.max.y)),
                egui::Rect::from_min_max(egui::pos2(cut + half, area.min.y), area.max),
            )
        }
        super::SplitDir::Down => {
            let cut = area.min.y + area.height() * ratio;
            (
                egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, cut - half)),
                egui::Rect::from_min_max(egui::pos2(area.min.x, cut + half), area.max),
            )
        }
    }
}
