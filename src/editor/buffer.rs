//! The text buffer: lines, cursors, edits and undo.
//!
//! Text is stored as one `String` per line with no line terminator, which keeps
//! every edit a line-range splice and makes undo a matter of putting the old
//! lines back. Line endings are normalised to LF in memory and restored on
//! save, so a CRLF file round-trips unchanged.

use super::pos::{Cursor, Pos, Span};
use super::undo::{Edit, Part, UndoStack};
use std::path::{Path, PathBuf};

/// How a file's lines end.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Eol {
    #[default]
    Lf,
    Crlf,
}

impl Eol {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::Crlf => "\r\n",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
        }
    }
}

/// Where a remote buffer lives, so a save can go back over SFTP.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct RemoteFile {
    /// The host label shown in the UI, e.g. `user@host:22`.
    pub(crate) host: String,
    /// The absolute path on the remote side.
    pub(crate) path: String,
}

/// What the file looked like when it was loaded or last saved.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct DiskState {
    pub(crate) len: u64,
    pub(crate) modified: Option<std::time::SystemTime>,
}

/// Whether the file on disk changed under us.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ExternalChange {
    None,
    /// Someone else wrote it.
    Changed,
    /// It was deleted or moved.
    Gone,
}

/// Read a file's state, for external-change detection.
pub(crate) fn disk_state(path: &Path) -> Option<DiskState> {
    let meta = std::fs::metadata(path).ok()?;
    Some(DiskState {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// One open document.
pub(crate) struct Buffer {
    pub(crate) path: Option<PathBuf>,
    /// Set when the document lives on an SSH host.
    pub(crate) remote: Option<RemoteFile>,
    pub(crate) lines: Vec<String>,
    pub(crate) eol: Eol,
    /// Whether the file ends with a line terminator.
    pub(crate) final_newline: bool,
    pub(crate) dirty: bool,
    /// Bumped on every edit, so caches (syntax spans) can tell they are stale.
    pub(crate) version: u64,
    /// The file's state at load/save time.
    pub(crate) disk: Option<DiskState>,
    pub(crate) undo: UndoStack,
    /// Cursors, most recently added last; the primary cursor is index 0.
    pub(crate) cursors: Vec<Cursor>,
    /// Scroll offset in cells, kept per buffer so switching tabs keeps the view.
    pub(crate) scroll: [f32; 2],
    /// The offset actually drawn, eased toward `scroll` so a touchpad's small
    /// deltas land as motion instead of jumps.
    pub(crate) scroll_display: f32,
    /// Where the primary cursor was when the view was last scrolled to keep it
    /// visible. Scrolling the wheel must not be undone by that follow, so it
    /// only runs when the cursor has actually moved.
    pub(crate) scroll_anchor: Option<Pos>,
    /// Set for files too large to highlight.
    pub(crate) large: bool,
    /// Set when the file could not be decoded as UTF-8.
    pub(crate) lossy: bool,
    /// Cached syntax spans, one entry per line, valid for `spans_version`.
    pub(crate) spans: Vec<Vec<(std::ops::Range<usize>, super::syntax::Style)>>,
    /// The buffer version the spans were computed for.
    pub(crate) spans_version: u64,
    /// The buffer that stands in when no file is open. Typing into it is
    /// refused with a hint to open a file, so a keystroke cannot fill a
    /// buffer that has nowhere to be saved.
    pub(crate) scratch: bool,
}

/// Files larger than this are opened without syntax highlighting.
///
/// The pure-Rust regex backend highlights at roughly 0.5 MB/s, so this is the
/// point past which the work is not worth doing at all; below it the highlight
/// runs on a worker thread and never blocks a frame.
pub(crate) const LARGE_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Files larger than this are refused outright.
///
/// Every edit is a line splice, so its cost scales with the line it touches: a
/// 200 MB file holding one long line measured 352 ms per Enter, and the 511 MB
/// binary that lives in `target/debug` froze the window for four seconds just
/// to read. A file this big is not text anyone edits, and refusing it before
/// the read is what keeps a stray click in the file tree from stalling the UI.
pub(crate) const MAX_EDIT_BYTES: u64 = 64 * 1024 * 1024;

impl Buffer {
    /// An empty scratch buffer.
    pub(crate) fn empty() -> Self {
        Self {
            path: None,
            remote: None,
            lines: vec![String::new()],
            eol: Eol::Lf,
            final_newline: false,
            dirty: false,
            version: 0,
            disk: None,
            undo: UndoStack::default(),
            cursors: vec![Cursor::new(Pos::new(0, 0))],
            scroll: [0.0, 0.0],
            scroll_display: 0.0,
            scroll_anchor: None,
            large: false,
            lossy: false,
            spans: Vec::new(),
            spans_version: u64::MAX,
            scratch: false,
        }
    }

    /// A buffer from text, normalising line endings to LF.
    pub(crate) fn from_text(text: &str, path: Option<PathBuf>) -> Self {
        let crlf = text.contains("\r\n");
        let normalized = if crlf {
            text.replace("\r\n", "\n")
        } else {
            text.to_owned()
        };
        let final_newline = normalized.ends_with('\n');
        let body = if final_newline {
            &normalized[..normalized.len() - 1]
        } else {
            normalized.as_str()
        };
        let lines: Vec<String> = body.split('\n').map(str::to_owned).collect();
        let disk = path.as_deref().and_then(disk_state);
        Self {
            path,
            remote: None,
            lines,
            eol: if crlf { Eol::Crlf } else { Eol::Lf },
            final_newline,
            dirty: false,
            version: 0,
            disk,
            undo: UndoStack::default(),
            cursors: vec![Cursor::new(Pos::new(0, 0))],
            scroll: [0.0, 0.0],
            scroll_display: 0.0,
            scroll_anchor: None,
            large: false,
            lossy: false,
            spans: Vec::new(),
            spans_version: u64::MAX,
            scratch: false,
        }
    }

    /// The buffer shown when no file is open.
    ///
    /// Typing into it is refused with a hint to open a file; a buffer the
    /// user asked for (the new buffer command, save as) is an ordinary one.
    pub(crate) fn scratch() -> Self {
        Self {
            scratch: true,
            ..Self::empty()
        }
    }

    /// Read a local file.
    pub(crate) fn open(path: &Path) -> std::io::Result<Self> {
        // The size is checked before the read, so a stray click on a huge file
        // costs a stat instead of a multi-second read.
        let size = std::fs::metadata(path)?.len();
        if size > MAX_EDIT_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "too large to edit ({:.0} MB); the limit is {} MB",
                    size as f64 / (1024.0 * 1024.0),
                    MAX_EDIT_BYTES / (1024 * 1024)
                ),
            ));
        }
        let bytes = std::fs::read(path)?;
        let large = bytes.len() as u64 > LARGE_FILE_BYTES;
        let (text, lossy) = match String::from_utf8(bytes) {
            Ok(text) => (text, false),
            Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
        };
        let mut buf = Self::from_text(&text, Some(path.to_path_buf()));
        buf.large = large;
        buf.lossy = lossy;
        Ok(buf)
    }

    /// The whole document as text, with its line endings restored.
    pub(crate) fn text(&self) -> String {
        let mut out = String::with_capacity(self.lines.iter().map(String::len).sum::<usize>() + 64);
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push_str(self.eol.as_str());
            }
            out.push_str(line);
        }
        if self.final_newline {
            out.push_str(self.eol.as_str());
        }
        out
    }

    pub(crate) fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub(crate) fn line(&self, i: usize) -> &str {
        self.lines.get(i).map_or("", String::as_str)
    }

    /// The line's length in characters.
    pub(crate) fn line_len(&self, i: usize) -> usize {
        self.line(i).chars().count()
    }

    /// The byte offset of a character column.
    pub(crate) fn byte_col(&self, line: usize, col: usize) -> usize {
        let text = self.line(line);
        text.char_indices()
            .nth(col)
            .map_or(text.len(), |(byte, _)| byte)
    }

    /// The character column of a byte offset.
    pub(crate) fn col_of_byte(&self, line: usize, byte: usize) -> usize {
        self.line(line)[..byte.min(self.line(line).len())]
            .chars()
            .count()
    }

    pub(crate) fn char_at(&self, pos: Pos) -> Option<char> {
        self.line(pos.line).chars().nth(pos.col)
    }

    /// The character before `pos`, crossing into the previous line.
    pub(crate) fn char_before(&self, pos: Pos) -> Option<char> {
        if pos.col > 0 {
            self.char_at(Pos::new(pos.line, pos.col - 1))
        } else if pos.line > 0 {
            self.char_at(Pos::new(
                pos.line - 1,
                self.line_len(pos.line - 1).saturating_sub(1),
            ))
        } else {
            None
        }
    }

    /// Clamp a position into the document, allowing the one-past-the-end
    /// column that insert mode uses.
    pub(crate) fn clamp(&self, pos: Pos) -> Pos {
        let line = pos.line.min(self.line_count().saturating_sub(1));
        Pos::new(line, pos.col.min(self.line_len(line)))
    }

    /// Clamp for normal mode, where the cursor sits *on* a character.
    pub(crate) fn clamp_normal(&self, pos: Pos) -> Pos {
        let pos = self.clamp(pos);
        let len = self.line_len(pos.line);
        Pos::new(pos.line, pos.col.min(len.saturating_sub(1)))
    }

    /// The first non-blank column of a line.
    pub(crate) fn first_non_blank(&self, line: usize) -> usize {
        self.line(line)
            .chars()
            .position(|c| !c.is_whitespace())
            .unwrap_or(0)
    }

    /// The leading whitespace width of a line, counting a tab as `tab_width`.
    pub(crate) fn indent_width(&self, line: usize, tab_width: usize) -> usize {
        let mut width = 0;
        for c in self.line(line).chars() {
            match c {
                ' ' => width += 1,
                '\t' => width += tab_width - (width % tab_width),
                _ => break,
            }
        }
        width
    }

    /// The text of a span, as lines.
    pub(crate) fn lines_in(&self, span: Span) -> Vec<String> {
        if span.start.line == span.end.line {
            let text = self.line(span.start.line);
            let a = self.byte_col(span.start.line, span.start.col);
            let b = self.byte_col(span.start.line, span.end.col);
            return vec![text[a..b].to_owned()];
        }
        let mut out = Vec::with_capacity(span.end.line - span.start.line + 1);
        let first = self.line(span.start.line);
        out.push(first[self.byte_col(span.start.line, span.start.col)..].to_owned());
        for line in span.start.line + 1..span.end.line {
            out.push(self.line(line).to_owned());
        }
        let last = self.line(span.end.line);
        out.push(last[..self.byte_col(span.end.line, span.end.col)].to_owned());
        out
    }

    /// The text of a span, joined with newlines.
    pub(crate) fn text_in(&self, span: Span) -> String {
        self.lines_in(span).join("\n")
    }

    // -----------------------------------------------------------------------
    // Edits
    // -----------------------------------------------------------------------

    /// The line-range replacement that inserting `text` at `pos` would make,
    /// plus the position just after the inserted text.
    ///
    /// Pure: a caller can collect parts for several cursors and apply them as
    /// one undo step.
    pub(crate) fn insert_part(&self, pos: Pos, text: &str) -> (Part, Pos) {
        let pos = self.clamp(pos);
        let parts: Vec<&str> = text.split('\n').collect();
        let line = self.line(pos.line).to_owned();
        let byte = self.byte_col(pos.line, pos.col);
        if parts.len() == 1 {
            let mut new_line = String::with_capacity(line.len() + text.len());
            new_line.push_str(&line[..byte]);
            new_line.push_str(text);
            new_line.push_str(&line[byte..]);
            let end = Pos::new(pos.line, pos.col + text.chars().count());
            (
                Part {
                    at: pos.line,
                    before: vec![line],
                    after: vec![new_line],
                },
                end,
            )
        } else {
            let mut new_lines = Vec::with_capacity(parts.len() + 1);
            let mut first = String::from(&line[..byte]);
            first.push_str(parts[0]);
            new_lines.push(first);
            for part in &parts[1..parts.len() - 1] {
                new_lines.push((*part).to_owned());
            }
            let mut last = String::from(parts[parts.len() - 1]);
            last.push_str(&line[byte..]);
            new_lines.push(last);
            let end = Pos::new(
                pos.line + parts.len() - 1,
                parts[parts.len() - 1].chars().count(),
            );
            (
                Part {
                    at: pos.line,
                    before: vec![line],
                    after: new_lines,
                },
                end,
            )
        }
    }

    /// Apply replacements as one undo step, leaving the cursors at
    /// `cursors_after`. Parts must be ordered by ascending `at`.
    pub(crate) fn apply_parts(
        &mut self,
        parts: Vec<Part>,
        cursors_after: Vec<Cursor>,
        merge: bool,
    ) {
        if parts.is_empty() {
            return;
        }
        let cursors_before = self.cursors.clone();
        // Apply from the last part to the first, so earlier line indices stay
        // valid while later ones are spliced.
        for part in parts.iter().rev() {
            let at = part.at.min(self.lines.len());
            let count = part.before.len().min(self.lines.len() - at);
            self.lines
                .splice(at..at + count, part.after.iter().cloned());
        }
        self.version += 1;
        self.cursors = cursors_after.clone();
        self.undo.push(
            Edit {
                parts,
                cursors_before,
                cursors_after,
            },
            merge,
        );
        // Undoing back to the saved position clears the marker again.
        self.dirty = self.undo.is_dirty();
    }

    /// The cursor state after an edit that moves only the primary cursor.
    fn cursors_moved(&self, pos: Pos) -> Vec<Cursor> {
        let mut cursors = self.cursors.clone();
        if let Some(first) = cursors.first_mut() {
            first.set(pos);
        }
        cursors
    }

    /// Insert text at `pos` with a single cursor; returns the position after it.
    pub(crate) fn insert(&mut self, pos: Pos, text: &str, merge: bool) -> Pos {
        let (part, end) = self.insert_part(pos, text);
        let cursors = self.cursors_moved(end);
        self.apply_parts(vec![part], cursors, merge);
        end
    }

    /// The replacement that deleting `span` would make, plus the deleted text.
    pub(crate) fn delete_part(&self, span: Span) -> (Part, String) {
        let span = Span::new(self.clamp(span.start), self.clamp(span.end));
        let removed = self.text_in(span);
        if span.start.line == span.end.line {
            let line = self.line(span.start.line).to_owned();
            let a = self.byte_col(span.start.line, span.start.col);
            let b = self.byte_col(span.start.line, span.end.col);
            let mut new_line = String::with_capacity(line.len());
            new_line.push_str(&line[..a]);
            new_line.push_str(&line[b..]);
            (
                Part {
                    at: span.start.line,
                    before: vec![line],
                    after: vec![new_line],
                },
                removed,
            )
        } else {
            let first = self.line(span.start.line).to_owned();
            let last = self.line(span.end.line).to_owned();
            let a = self.byte_col(span.start.line, span.start.col);
            let b = self.byte_col(span.end.line, span.end.col);
            let mut joined = String::with_capacity(first.len() + last.len());
            joined.push_str(&first[..a]);
            joined.push_str(&last[b..]);
            let before = self.lines[span.start.line..=span.end.line].to_vec();
            (
                Part {
                    at: span.start.line,
                    before,
                    after: vec![joined],
                },
                removed,
            )
        }
    }

    /// The replacement that substituting `text` for `span` would make.
    pub(crate) fn replace_part(&self, span: Span, text: &str) -> (Part, Pos) {
        let span = Span::new(self.clamp(span.start), self.clamp(span.end));
        let start = span.start;
        let line = self.line(start.line).to_owned();
        let a = self.byte_col(start.line, start.col);
        let b = self.byte_col(span.end.line, span.end.col);
        let tail = if span.start.line == span.end.line {
            line[b..].to_owned()
        } else {
            self.line(span.end.line)[b..].to_owned()
        };
        let parts: Vec<&str> = text.split('\n').collect();
        let mut new_lines = Vec::with_capacity(parts.len());
        let mut first = String::from(&line[..a]);
        first.push_str(parts[0]);
        new_lines.push(first);
        if parts.len() > 1 {
            for part in &parts[1..parts.len() - 1] {
                new_lines.push((*part).to_owned());
            }
            let mut last = String::from(parts[parts.len() - 1]);
            last.push_str(&tail);
            new_lines.push(last);
        } else {
            new_lines[0].push_str(&tail);
        }
        let end = if parts.len() == 1 {
            Pos::new(start.line, start.col + text.chars().count())
        } else {
            Pos::new(
                start.line + parts.len() - 1,
                parts[parts.len() - 1].chars().count(),
            )
        };
        let before = self.lines[span.start.line..=span.end.line].to_vec();
        (
            Part {
                at: span.start.line,
                before,
                after: new_lines,
            },
            end,
        )
    }

    /// The replacement for swapping whole lines. `at` may be one past the last
    /// line, which appends.
    pub(crate) fn lines_part(&self, at: usize, count: usize, new: Vec<String>) -> Part {
        let at = at.min(self.lines.len());
        let count = count.min(self.lines.len().saturating_sub(at));
        Part {
            at,
            before: self.lines[at..at + count].to_vec(),
            after: new,
        }
    }

    /// Delete a span with a single cursor; returns the deleted text.
    pub(crate) fn delete(&mut self, span: Span, merge: bool) -> String {
        let (part, removed) = self.delete_part(span);
        let cursors = self.cursors_moved(span.start);
        self.apply_parts(vec![part], cursors, merge);
        removed
    }

    /// Replace a span with text; returns the position after it.
    pub(crate) fn replace(&mut self, span: Span, text: &str, merge: bool) -> Pos {
        let (part, end) = self.replace_part(span, text);
        let cursors = self.cursors_moved(end);
        self.apply_parts(vec![part], cursors, merge);
        end
    }

    /// Replace whole lines `at..at+count` with `new`.
    pub(crate) fn splice_lines(&mut self, at: usize, count: usize, new: Vec<String>, merge: bool) {
        let part = self.lines_part(at, count, new);
        let cursors = self.cursors.clone();
        self.apply_parts(vec![part], cursors, merge);
    }

    /// Undo one step; true when something was undone.
    pub(crate) fn undo(&mut self) -> bool {
        let Some(edit) = self.undo.undo() else {
            return false;
        };
        for part in edit.parts.iter().rev() {
            let at = part.at.min(self.lines.len());
            let count = part.after.len().min(self.lines.len() - at);
            self.lines
                .splice(at..at + count, part.before.iter().cloned());
        }
        self.cursors = edit.cursors_before.clone();
        self.version += 1;
        self.dirty = self.undo.is_dirty();
        true
    }

    /// Redo one step; true when something was redone.
    pub(crate) fn redo(&mut self) -> bool {
        let Some(edit) = self.undo.redo() else {
            return false;
        };
        for part in edit.parts.iter() {
            let at = part.at.min(self.lines.len());
            let count = part.before.len().min(self.lines.len() - at);
            self.lines
                .splice(at..at + count, part.after.iter().cloned());
        }
        self.cursors = edit.cursors_after.clone();
        self.version += 1;
        self.dirty = self.undo.is_dirty();
        true
    }

    /// End the current undo run, so the next edit is its own step.
    pub(crate) fn break_undo_run(&mut self) {
        self.undo.break_run();
    }

    /// Start collecting edits into one undo step.
    pub(crate) fn begin_group(&mut self) {
        self.undo.begin_group();
    }

    /// Close the group opened by `begin_group`.
    pub(crate) fn end_group(&mut self) {
        self.undo.end_group();
        // The grouped edit only reaches the stack now, so the marker is
        // recomputed here rather than in `apply_parts`.
        self.dirty = self.undo.is_dirty();
    }

    // -----------------------------------------------------------------------
    // Files
    // -----------------------------------------------------------------------

    /// Remember the current content as the written one, clearing the marker.
    pub(crate) fn mark_saved(&mut self) {
        self.undo.mark_saved();
        self.dirty = false;
    }

    /// Write the buffer to its path.
    pub(crate) fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "this buffer has no file name",
            ));
        };
        std::fs::write(&path, self.text())?;
        self.mark_saved();
        self.disk = disk_state(&path);
        Ok(())
    }

    /// Re-read the file from disk, discarding in-memory changes.
    pub(crate) fn reload(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "this buffer has no file name",
            ));
        };
        let fresh = Self::open(&path)?;
        self.lines = fresh.lines;
        self.eol = fresh.eol;
        self.final_newline = fresh.final_newline;
        self.disk = fresh.disk;
        self.large = fresh.large;
        self.lossy = fresh.lossy;
        self.dirty = false;
        self.version += 1;
        self.undo = UndoStack::default();
        self.cursors = vec![Cursor::new(Pos::new(0, 0))];
        Ok(())
    }

    /// Whether the file changed on disk since it was loaded or saved.
    pub(crate) fn check_external(&self) -> ExternalChange {
        if self.remote.is_some() {
            return ExternalChange::None;
        }
        let Some(path) = &self.path else {
            return ExternalChange::None;
        };
        match disk_state(path) {
            Some(now) => match self.disk {
                Some(was) if was == now => ExternalChange::None,
                Some(_) => ExternalChange::Changed,
                None => ExternalChange::None,
            },
            None => {
                if self.disk.is_some() {
                    ExternalChange::Gone
                } else {
                    ExternalChange::None
                }
            }
        }
    }

    /// The name shown on the tab: the file name, or `[scratch]`.
    pub(crate) fn title(&self) -> String {
        match (&self.remote, &self.path) {
            (Some(remote), _) => remote
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&remote.path)
                .to_owned(),
            (None, Some(path)) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            (None, None) => "[scratch]".to_owned(),
        }
    }

    /// The full label, for the status line: local path or `host:path`.
    pub(crate) fn location(&self) -> String {
        match (&self.remote, &self.path) {
            (Some(remote), _) => format!("{}:{}", remote.host, remote.path),
            (None, Some(path)) => path.display().to_string(),
            (None, None) => "[scratch]".to_owned(),
        }
    }

    /// Whether this buffer is remote.
    pub(crate) fn is_remote(&self) -> bool {
        self.remote.is_some()
    }
}
