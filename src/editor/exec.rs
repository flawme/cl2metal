//! Command execution: what each command does to the buffer.
//!
//! Motions, operators and text objects meet here. Every edit goes through
//! `Buffer::apply_parts`, so a multi-cursor edit is one undo step and the
//! registers see exactly the text that left the buffer.

use super::Editor;
use super::buffer::Buffer;
use super::command::Command;
use super::mode::{CommandKind, Mode};
use super::motion::{self, Motion};
use super::operator::{self, Operator, TextObject};
use super::pos::{Cursor, Pos, Span, VisualKind};
use super::register::Register;

/// Where insert mode puts the cursor when it starts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum InsertAt {
    /// `i`
    Cursor,
    /// `a`
    After,
    /// `I`
    LineStart,
    /// `A`
    LineEnd,
    /// `o` / `O`
    NewLineBelow,
    NewLineAbove,
}

impl Editor {
    /// Run a command, honouring the count, register and operator that were
    /// typed before it.
    pub(crate) fn execute(&mut self, cmd: Command) {
        let count = self.pending.count();
        let register = self.pending.register;
        let pending_op = self.pending.operator;
        self.pending.clear();
        self.buffer_mut().break_undo_run();

        match cmd {
            Command::Move(m) => match pending_op {
                Some(op) => self.operate_motion(op, m, count, register),
                None => self.move_cursor(m, count, self.mode.is_visual()),
            },
            Command::Operate(op) => {
                if self.mode.is_visual() {
                    // An operator in visual mode acts on the selection.
                    let kind = match self.mode {
                        Mode::Visual(kind) => kind,
                        _ => VisualKind::Char,
                    };
                    let spans = self.visual_spans();
                    self.mode = Mode::Normal;
                    self.apply_operator(op, spans, kind == VisualKind::Line, register);
                } else if let Some(prev) = pending_op {
                    // `dd`, `yy`, `cc`: the operator repeats on whole lines.
                    self.operate_lines(prev, count, register);
                } else {
                    self.pending.operator = Some(op);
                    self.pending.count = Some(count);
                    self.pending.register = register;
                }
            }
            Command::InsertBefore => self.enter_insert(InsertAt::Cursor),
            Command::InsertAfter => self.enter_insert(InsertAt::After),
            Command::InsertLineStart => self.enter_insert(InsertAt::LineStart),
            Command::InsertLineEnd => self.enter_insert(InsertAt::LineEnd),
            Command::OpenBelow => self.enter_insert(InsertAt::NewLineBelow),
            Command::OpenAbove => self.enter_insert(InsertAt::NewLineAbove),
            Command::DeleteChar => self.delete_chars(count, false),
            Command::DeleteCharBefore => self.delete_chars(count, true),
            Command::DeleteLine => self.operate_lines(Operator::Delete, count, register),
            Command::DeleteToLineEnd => self.delete_to_line_end(),
            Command::JoinLines => self.join_lines(count),
            Command::Undo => {
                self.buffer_mut().undo();
            }
            Command::Redo => {
                self.buffer_mut().redo();
            }
            Command::YankLine => self.operate_lines(Operator::Yank, count, register),
            Command::PasteAfter => self.paste(count, true, register),
            Command::PasteBefore => self.paste(count, false, register),
            Command::Copy => self.copy_to_clipboard(),
            Command::Paste => self.paste_from_clipboard(),
            Command::Indent => self.transform_lines(Operator::Indent, count),
            Command::Dedent => self.transform_lines(Operator::Dedent, count),
            Command::ToggleComment => self.transform_lines(Operator::Comment, count),
            Command::ToggleCase => self.toggle_case_at_cursor(count),
            Command::VisualChar => self.enter_visual(VisualKind::Char),
            Command::VisualLine => self.enter_visual(VisualKind::Line),
            Command::VisualBlock => self.enter_visual(VisualKind::Block),
            Command::SelectAll => self.select_all(),
            Command::SelectNextOccurrence => self.select_next_occurrence(),
            Command::SelectAllOccurrences => self.select_all_occurrences(),
            Command::AddCursorBelow => self.add_cursor(1),
            Command::AddCursorAbove => self.add_cursor(-1),
            Command::ClearExtraCursors => self.clear_extra_cursors(),
            Command::GotoBracket => self.goto_bracket(),
            Command::SearchNext => self.search_step(false),
            Command::SearchPrev => self.search_step(true),
            Command::Escape => self.escape(),
            Command::EnterCommandLine => {
                self.mode = Mode::Command(CommandKind::Ex);
                self.command_line.clear();
            }
            Command::ScrollHalfDown => self.scroll_half(1.0),
            Command::ScrollHalfUp => self.scroll_half(-1.0),
            Command::CenterCursor => self.center_cursor(),
            // Everything else is handled by the editor shell (files, panels,
            // layout) or by the application around it.
            other => self.execute_outer(other),
        }
    }

    // -----------------------------------------------------------------------
    // Motions
    // -----------------------------------------------------------------------

    /// Move every cursor by a motion.
    pub(crate) fn move_cursor(&mut self, motion: Motion, count: usize, extend: bool) {
        let insert = self.mode.is_insert();
        let vertical = matches!(motion, Motion::Up | Motion::Down);
        let buf = self.buffer_mut();
        let mut cursors = buf.cursors.clone();
        for cursor in cursors.iter_mut() {
            let from = cursor.pos;
            let want = cursor.want_col.unwrap_or(from.col);
            let mut target = motion::resolve(buf, from, motion, count);
            if vertical {
                target.col = want;
            }
            let target = if insert {
                buf.clamp(target)
            } else {
                buf.clamp_normal(target)
            };
            if extend {
                cursor.extend(target);
            } else {
                cursor.set(target);
            }
            cursor.want_col = vertical.then_some(want);
        }
        buf.cursors = cursors;
    }

    /// `%`: jump to the matching bracket.
    fn goto_bracket(&mut self) {
        let buf = self.buffer_mut();
        let mut cursors = buf.cursors.clone();
        for cursor in cursors.iter_mut() {
            if let Some(target) = motion::match_bracket(buf, cursor.pos) {
                cursor.set(target);
            }
        }
        buf.cursors = cursors;
    }

    // -----------------------------------------------------------------------
    // Operators
    // -----------------------------------------------------------------------

    /// An operator with a motion: `dw`, `d$`, `2d3w`.
    fn operate_motion(
        &mut self,
        op: Operator,
        motion: Motion,
        count: usize,
        register: Option<char>,
    ) {
        let linewise = motion.linewise();
        let inclusive = motion.inclusive();
        let buf = self.buffer();
        let spans: Vec<Span> = buf
            .cursors
            .iter()
            .map(|c| {
                let target = motion::resolve(buf, c.pos, motion, count);
                let end = if inclusive {
                    motion::step(buf, target, Motion::Right)
                } else {
                    target
                };
                let span = Span::new(c.pos, end);
                if linewise {
                    operator::linewise_span(buf, span)
                } else {
                    span
                }
            })
            .collect();
        self.apply_operator(op, spans, linewise, register);
    }

    /// An operator repeated on whole lines: `dd`, `yy`, `3cc`.
    fn operate_lines(&mut self, op: Operator, count: usize, register: Option<char>) {
        let buf = self.buffer();
        let spans: Vec<Span> = buf
            .cursors
            .iter()
            .map(|c| {
                let last = (c.pos.line + count - 1).min(buf.line_count().saturating_sub(1));
                Span::new(Pos::new(c.pos.line, 0), Pos::new(last, buf.line_len(last)))
            })
            .collect();
        self.apply_operator(op, spans, true, register);
    }

    /// An operator with a text object: `diw`, `ci"`, `ya(`.
    pub(crate) fn operate_object(
        &mut self,
        op: Operator,
        object: TextObject,
        register: Option<char>,
    ) {
        let buf = self.buffer();
        let spans: Vec<Span> = buf
            .cursors
            .iter()
            .filter_map(|c| operator::text_object(buf, c.pos, object))
            .collect();
        if spans.is_empty() {
            return;
        }
        self.apply_operator(op, spans, false, register);
    }

    /// Apply an operator to one span per cursor.
    pub(crate) fn apply_operator(
        &mut self,
        op: Operator,
        spans: Vec<Span>,
        linewise: bool,
        register: Option<char>,
    ) {
        if spans.is_empty() {
            return;
        }
        if matches!(
            op,
            Operator::Indent
                | Operator::Dedent
                | Operator::Upper
                | Operator::Lower
                | Operator::Comment
        ) {
            self.transform_spans(op, spans);
            return;
        }
        let buf = self.buffer_mut();
        let mut ordered = spans;
        ordered.sort_by_key(|s| s.start);
        // Build the replacements from the last span to the first, so earlier
        // spans keep their positions.
        let mut parts = Vec::new();
        let mut texts = Vec::new();
        for span in ordered.iter().rev() {
            if linewise {
                // Whole lines leave the buffer entirely, newline and all.
                let at = span.start.line;
                let count = (span.end.line - span.start.line + 1).min(buf.line_count() - at);
                let removed = buf.lines[at..at + count].join("\n");
                let after = if count >= buf.line_count() {
                    // Never leave the buffer with no lines at all.
                    vec![String::new()]
                } else {
                    Vec::new()
                };
                parts.push(buf.lines_part(at, count, after));
                texts.push(removed);
            } else {
                let (part, removed) = buf.delete_part(*span);
                parts.push(part);
                texts.push(removed);
            }
        }
        parts.reverse();
        texts.reverse();
        let text = texts.join("\n");
        let cursors: Vec<Cursor> = ordered
            .iter()
            .map(|s| Cursor::new(Pos::new(s.start.line.min(buf.line_count() - 1), 0)))
            .collect();
        if op == Operator::Yank {
            // A yank changes no text: only the register.
            self.registers
                .set(register, Register::from_text(&text, linewise));
            return;
        }
        buf.apply_parts(parts, cursors, false);
        // The buffer may have shrunk under the cursors.
        let clamped: Vec<Cursor> = buf
            .cursors
            .iter()
            .map(|c| {
                let mut c = *c;
                c.pos = buf.clamp_normal(c.pos);
                c.anchor = c.anchor.map(|a| buf.clamp_normal(a));
                c
            })
            .collect();
        buf.cursors = clamped;
        if op.yanks() {
            self.registers
                .set(register, Register::from_text(&text, linewise));
        }
        if op.enters_insert() {
            self.mode = Mode::Insert;
        }
    }

    /// In-place transforms: case, indentation, comments.
    fn transform_spans(&mut self, op: Operator, spans: Vec<Span>) {
        let tab_width = self.settings.tab_width;
        let comment = self.comment_prefix().to_owned();
        let buf = self.buffer_mut();
        let mut ordered = spans;
        ordered.sort_by_key(|s| s.start);
        let mut parts = Vec::new();
        for span in ordered.iter().rev() {
            let text = buf.text_in(*span);
            let new = match op {
                Operator::Upper => text.to_uppercase(),
                Operator::Lower => text.to_lowercase(),
                Operator::ToggleCase => toggle_case(&text),
                Operator::Indent => shift_lines(&text, true, tab_width),
                Operator::Dedent => shift_lines(&text, false, tab_width),
                Operator::Comment => toggle_comment(&text, &comment),
                _ => text.clone(),
            };
            if new != text {
                let (part, _) = buf.replace_part(*span, &new);
                parts.push(part);
            }
        }
        parts.reverse();
        if !parts.is_empty() {
            let cursors = buf.cursors.clone();
            buf.apply_parts(parts, cursors, false);
        }
    }

    /// Indent, dedent or comment the lines the cursors are on.
    fn transform_lines(&mut self, op: Operator, count: usize) {
        let buf = self.buffer();
        let spans: Vec<Span> = buf
            .cursors
            .iter()
            .map(|c| {
                let last = (c.pos.line + count - 1).min(buf.line_count().saturating_sub(1));
                Span::new(Pos::new(c.pos.line, 0), Pos::new(last, buf.line_len(last)))
            })
            .collect();
        self.transform_spans(op, spans);
    }

    /// `~`: flip the case of the next `count` characters.
    fn toggle_case_at_cursor(&mut self, count: usize) {
        let buf = self.buffer();
        let spans: Vec<Span> = buf
            .cursors
            .iter()
            .map(|c| {
                let end = motion::resolve(buf, c.pos, Motion::Right, count);
                Span::new(c.pos, end)
            })
            .collect();
        self.transform_spans(Operator::ToggleCase, spans);
    }

    // -----------------------------------------------------------------------
    // Insert mode
    // -----------------------------------------------------------------------

    /// Enter insert mode at a position relative to the cursor.
    pub(crate) fn enter_insert(&mut self, at: InsertAt) {
        let buf = self.buffer_mut();
        let mut cursors = buf.cursors.clone();
        let mut parts = Vec::new();
        for cursor in cursors.iter_mut() {
            let line = cursor.pos.line;
            let len = buf.line_len(line);
            match at {
                InsertAt::Cursor => cursor.set(cursor.pos),
                InsertAt::After => {
                    let col = (cursor.pos.col + 1).min(len);
                    cursor.set(Pos::new(line, col));
                }
                InsertAt::LineStart => cursor.set(Pos::new(line, buf.first_non_blank(line))),
                InsertAt::LineEnd => cursor.set(Pos::new(line, len)),
                InsertAt::NewLineBelow | InsertAt::NewLineAbove => {
                    let at_line = if at == InsertAt::NewLineBelow {
                        line + 1
                    } else {
                        line
                    };
                    let part = buf.lines_part(at_line, 0, vec![String::new()]);
                    parts.push(part);
                    cursor.set(Pos::new(at_line, 0));
                }
            }
        }
        if !parts.is_empty() {
            buf.apply_parts(parts, cursors.clone(), false);
        }
        buf.cursors = cursors;
        self.mode = Mode::Insert;
    }

    /// Type text at every cursor, replacing any selection.
    ///
    /// The edits are applied one cursor at a time, from the last to the first,
    /// so each one sees the previous one's result and two cursors on the same
    /// line cannot overwrite each other. They are grouped into a single undo
    /// step.
    pub(crate) fn insert_text(&mut self, text: &str) {
        // The buffer standing in for "no file open" takes no text: every path
        // that writes (typing, Enter, tab, paste) lands here, and a keystroke
        // into a buffer with nowhere to be saved is work being thrown away.
        if self.buffer().scratch {
            self.say("open a file to write");
            return;
        }
        let replace = self.mode == Mode::Replace;
        let was_visual = self.mode.is_visual();
        let buf = self.buffer_mut();
        let mut cursors = buf.cursors.clone();
        let mut order: Vec<usize> = (0..cursors.len()).collect();
        order.sort_by_key(|i| std::cmp::Reverse(cursors[*i].pos));
        buf.begin_group();
        for i in order {
            let cursor = cursors[i];
            let (part, end) = if let Some(span) = cursor.selection() {
                // Typing over a selection replaces it, which is what makes
                // "select all occurrences, then type" work.
                buf.replace_part(span, text)
            } else if replace && !text.contains('\n') {
                // Overwrite: take the characters under the cursor with it.
                let end = motion::resolve(buf, cursor.pos, Motion::Right, text.chars().count());
                buf.replace_part(Span::new(cursor.pos, end), text)
            } else {
                buf.insert_part(cursor.pos, text)
            };
            cursors[i].set(end);
            buf.apply_parts(vec![part], cursors.clone(), false);
        }
        buf.end_group();
        if was_visual {
            self.mode = Mode::Insert;
        }
    }

    /// Backspace at every cursor.
    pub(crate) fn backspace(&mut self) {
        let buf = self.buffer_mut();
        let mut cursors = buf.cursors.clone();
        let mut order: Vec<usize> = (0..cursors.len()).collect();
        order.sort_by_key(|i| std::cmp::Reverse(cursors[*i].pos));
        buf.begin_group();
        for i in order {
            let pos = cursors[i].pos;
            let start = if pos.col > 0 {
                Pos::new(pos.line, pos.col - 1)
            } else if pos.line > 0 {
                Pos::new(pos.line - 1, buf.line_len(pos.line - 1))
            } else {
                continue;
            };
            let (part, _) = buf.delete_part(Span::new(start, pos));
            cursors[i].set(start);
            buf.apply_parts(vec![part], cursors.clone(), true);
        }
        buf.end_group();
    }

    /// Delete forward at every cursor.
    pub(crate) fn delete_forward(&mut self) {
        let buf = self.buffer_mut();
        let cursors = buf.cursors.clone();
        let mut order: Vec<usize> = (0..cursors.len()).collect();
        order.sort_by_key(|i| std::cmp::Reverse(cursors[*i].pos));
        buf.begin_group();
        for i in order {
            let pos = cursors[i].pos;
            let end = motion::step(buf, pos, Motion::Right);
            if end == pos {
                continue;
            }
            let (part, _) = buf.delete_part(Span::new(pos, end));
            buf.apply_parts(vec![part], cursors.clone(), true);
        }
        buf.end_group();
    }

    /// `x` / `X`: delete characters around the cursor.
    fn delete_chars(&mut self, count: usize, before: bool) {
        let buf = self.buffer();
        let spans: Vec<Span> = buf
            .cursors
            .iter()
            .map(|c| {
                if before {
                    let start = motion::resolve(buf, c.pos, Motion::Left, count);
                    Span::new(start, c.pos)
                } else {
                    let end = motion::resolve(buf, c.pos, Motion::Right, count);
                    Span::new(c.pos, end)
                }
            })
            .collect();
        self.apply_operator(Operator::Delete, spans, false, None);
    }

    /// `D`: delete to the end of the line.
    fn delete_to_line_end(&mut self) {
        let buf = self.buffer();
        let spans: Vec<Span> = buf
            .cursors
            .iter()
            .map(|c| {
                let len = buf.line_len(c.pos.line);
                Span::new(c.pos, Pos::new(c.pos.line, len))
            })
            .collect();
        self.apply_operator(Operator::Delete, spans, false, None);
    }

    /// `J`: join the next line onto this one.
    fn join_lines(&mut self, count: usize) {
        let buf = self.buffer_mut();
        let mut cursors = buf.cursors.clone();
        let mut parts = Vec::new();
        for cursor in cursors.iter_mut() {
            for _ in 0..count.max(1) {
                let line = cursor.pos.line;
                if line + 1 >= buf.line_count() {
                    break;
                }
                let joined = format!(
                    "{} {}",
                    buf.line(line).trim_end(),
                    buf.line(line + 1).trim_start()
                );
                let part = buf.lines_part(line, 2, vec![joined]);
                parts.push(part);
                cursor.set(Pos::new(line, buf.line_len(line)));
            }
        }
        parts.sort_by_key(|p| p.at);
        if !parts.is_empty() {
            buf.apply_parts(parts, cursors, false);
        }
    }

    /// `p` / `P`: paste from a register.
    fn paste(&mut self, count: usize, after: bool, register: Option<char>) {
        let Some(reg) = self.registers.get(register).cloned() else {
            return;
        };
        if reg.is_empty() {
            return;
        }
        let text = reg.lines.join("\n");
        let linewise = reg.linewise;
        let buf = self.buffer_mut();
        let mut cursors = buf.cursors.clone();
        let mut parts = Vec::new();
        for cursor in cursors.iter_mut() {
            if linewise {
                let at = if after {
                    cursor.pos.line + 1
                } else {
                    cursor.pos.line
                };
                let mut lines: Vec<String> = reg.lines.clone();
                // A trailing empty line from a linewise yank is not content.
                if lines.last().is_some_and(String::is_empty) {
                    lines.pop();
                }
                let part = buf.lines_part(at, 0, lines);
                parts.push(part);
                cursor.set(Pos::new(at, 0));
            } else {
                let pos = if after {
                    motion::step(buf, cursor.pos, Motion::Right)
                } else {
                    cursor.pos
                };
                let (part, end) = buf.insert_part(pos, &text);
                parts.push(part);
                cursor.set(end);
            }
        }
        parts.sort_by_key(|p| p.at);
        buf.apply_parts(parts, cursors, false);
        let _ = count;
    }

    // -----------------------------------------------------------------------
    // Visual mode and multiple cursors
    // -----------------------------------------------------------------------

    fn enter_visual(&mut self, kind: VisualKind) {
        let buf = self.buffer_mut();
        let mut cursors = buf.cursors.clone();
        for cursor in cursors.iter_mut() {
            if cursor.anchor.is_none() {
                cursor.anchor = Some(cursor.pos);
            }
        }
        buf.cursors = cursors;
        self.mode = Mode::Visual(kind);
    }

    fn select_all(&mut self) {
        let buf = self.buffer_mut();
        let last = buf.line_count().saturating_sub(1);
        let end = Pos::new(last, buf.line_len(last));
        buf.cursors = vec![Cursor {
            pos: end,
            anchor: Some(Pos::new(0, 0)),
            want_col: None,
        }];
        self.mode = Mode::Visual(VisualKind::Char);
    }

    /// The spans the visual selection covers, one per cursor.
    ///
    /// Visual mode includes the character under the cursor, so the span runs
    /// one past it — `vll` on `abcdef` selects `abc`, as Vim does.
    pub(crate) fn visual_spans(&self) -> Vec<Span> {
        let buf = self.buffer();
        let kind = match self.mode {
            Mode::Visual(kind) => kind,
            _ => VisualKind::Char,
        };
        buf.cursors
            .iter()
            .filter_map(|c| {
                let span = c.selection()?;
                Some(match kind {
                    VisualKind::Char => {
                        Span::new(span.start, motion::step(buf, span.end, Motion::Right))
                    }
                    VisualKind::Line => operator::linewise_span(buf, span),
                    VisualKind::Block => span,
                })
            })
            .collect()
    }

    /// `Ctrl+D`: add the next occurrence of the selection as a cursor.
    pub(crate) fn select_next_occurrence(&mut self) {
        let buf = self.buffer_mut();
        let Some(primary) = buf.cursors.first().copied() else {
            return;
        };
        let Some(span) = primary.selection() else {
            // Nothing selected: take the word under the cursor.
            let Some(word) = operator::text_object(buf, primary.pos, TextObject::InnerWord) else {
                return;
            };
            if word.is_empty() {
                return;
            }
            buf.cursors[0].anchor = Some(word.start);
            buf.cursors[0].pos = word.end;
            return;
        };
        let needle = buf.text_in(span);
        if needle.is_empty() {
            return;
        }
        let query = super::search::Query::new(&needle);
        let matches = super::search::find_all(buf, &query);
        let taken: Vec<Span> = buf.cursors.iter().filter_map(Cursor::selection).collect();
        let next = matches
            .into_iter()
            .find(|m| m.start > span.start && !taken.contains(m));
        if let Some(m) = next {
            buf.cursors.push(Cursor {
                pos: m.end,
                anchor: Some(m.start),
                want_col: None,
            });
        }
    }

    /// `Ctrl+Shift+L`: a cursor on every occurrence of the selection.
    pub(crate) fn select_all_occurrences(&mut self) {
        let buf = self.buffer_mut();
        let Some(primary) = buf.cursors.first().copied() else {
            return;
        };
        let span = match primary.selection() {
            Some(span) => span,
            None => match operator::text_object(buf, primary.pos, TextObject::InnerWord) {
                Some(word) => word,
                None => return,
            },
        };
        let needle = buf.text_in(span);
        if needle.is_empty() {
            return;
        }
        let query = super::search::Query::new(&needle);
        let matches = super::search::find_all(buf, &query);
        if matches.is_empty() {
            return;
        }
        buf.cursors = matches
            .into_iter()
            .map(|m| Cursor {
                pos: m.end,
                anchor: Some(m.start),
                want_col: None,
            })
            .collect();
    }

    /// Add a cursor on the line below (`1`) or above (`-1`).
    fn add_cursor(&mut self, direction: i32) {
        let buf = self.buffer_mut();
        let Some(last) = buf.cursors.last().copied() else {
            return;
        };
        let line = if direction > 0 {
            last.pos.line + 1
        } else {
            match last.pos.line.checked_sub(1) {
                Some(line) => line,
                None => return,
            }
        };
        if line >= buf.line_count() {
            return;
        }
        let col = last.pos.col.min(buf.line_len(line));
        let mut cursor = Cursor::new(Pos::new(line, col));
        cursor.anchor = last
            .anchor
            .map(|a| Pos::new(line, a.col.min(buf.line_len(line))));
        buf.cursors.push(cursor);
    }

    fn clear_extra_cursors(&mut self) {
        let buf = self.buffer_mut();
        buf.cursors.truncate(1);
    }

    /// Leave insert or visual mode.
    pub(crate) fn escape(&mut self) {
        let buf = self.buffer_mut();
        let mut cursors = buf.cursors.clone();
        for cursor in cursors.iter_mut() {
            cursor.clear_selection();
            let pos = cursor.pos;
            cursor.set(buf.clamp_normal(pos));
        }
        buf.cursors = cursors;
        self.pending.clear();
        self.pending_keys.clear();
        self.command_line.clear();
        self.mode = Mode::Normal;
    }

    // -----------------------------------------------------------------------
    // System clipboard
    // -----------------------------------------------------------------------

    /// Copy the selection, or the current line, to the system clipboard.
    fn copy_to_clipboard(&mut self) {
        let text = {
            let buf = self.buffer();
            let spans = self.visual_spans();
            if spans.is_empty() {
                let line = buf.cursors.first().map_or(0, |c| c.pos.line);
                format!("{}\n", buf.line(line))
            } else {
                spans
                    .iter()
                    .map(|span| buf.text_in(*span))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone())) {
            Ok(()) => self.say(format!("copied {} characters", text.chars().count())),
            Err(e) => self.say(format!("clipboard unavailable: {e}")),
        }
    }

    /// Paste the system clipboard at the cursor.
    fn paste_from_clipboard(&mut self) {
        match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(text) => self.insert_text(&text),
            Err(e) => self.say(format!("clipboard unavailable: {e}")),
        }
    }

    // -----------------------------------------------------------------------
    // Scrolling
    // -----------------------------------------------------------------------

    fn scroll_half(&mut self, direction: f32) {
        let buf = self.buffer_mut();
        buf.scroll[1] = (buf.scroll[1] + direction * 12.0).max(0.0);
    }

    fn center_cursor(&mut self) {
        let buf = self.buffer_mut();
        let line = buf.cursors.first().map_or(0, |c| c.pos.line) as f32;
        buf.scroll[1] = (line - 12.0).max(0.0);
    }
}

/// Flip the case of every character.
fn toggle_case(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_uppercase() {
                c.to_lowercase().next().unwrap_or(c)
            } else if c.is_lowercase() {
                c.to_uppercase().next().unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// Add or remove one indent level on every line of `text`.
fn shift_lines(text: &str, indent: bool, tab_width: usize) -> String {
    let unit = " ".repeat(tab_width);
    text.split('\n')
        .map(|line| {
            if indent {
                if line.trim().is_empty() {
                    line.to_owned()
                } else {
                    format!("{unit}{line}")
                }
            } else {
                let mut rest = line;
                let mut removed = 0;
                while removed < tab_width {
                    if let Some(stripped) = rest.strip_prefix(' ') {
                        rest = stripped;
                        removed += 1;
                    } else if let Some(stripped) = rest.strip_prefix('\t') {
                        rest = stripped;
                        break;
                    } else {
                        break;
                    }
                }
                rest.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Comment or uncomment every line of `text`.
fn toggle_comment(text: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return text.to_owned();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let commented = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .all(|l| l.trim_start().starts_with(prefix));
    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                return (*line).to_owned();
            }
            if commented {
                match line.find(prefix) {
                    Some(at) => {
                        let mut out = String::with_capacity(line.len());
                        out.push_str(&line[..at]);
                        out.push_str(&line[at + prefix.len()..]);
                        out
                    }
                    None => (*line).to_owned(),
                }
            } else {
                let indent = line.len() - line.trim_start().len();
                format!("{}{}{}", &line[..indent], prefix, &line[indent..])
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The buffer a command should act on, for tests and the shell.
pub(crate) fn active_buffer(editor: &Editor) -> &Buffer {
    editor.buffer()
}
