//! Positions, spans and cursors: the editor's coordinate types.
//!
//! Columns count *characters*, not bytes, so a cursor sits on the same glyph in
//! UTF-8 text as it does in ASCII. Display width (for wide CJK glyphs and tabs)
//! is applied at draw time, not here.

/// A position in a buffer: a line index and a character index within that line.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Hash)]
pub(crate) struct Pos {
    pub(crate) line: usize,
    pub(crate) col: usize,
}

impl Pos {
    pub(crate) fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// A half-open range of positions, always ordered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Span {
    pub(crate) start: Pos,
    pub(crate) end: Pos,
}

impl Span {
    /// Order the two ends, so a backwards selection is still a valid span.
    pub(crate) fn new(a: Pos, b: Pos) -> Self {
        if a <= b {
            Self { start: a, end: b }
        } else {
            Self { start: b, end: a }
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub(crate) fn contains(&self, pos: Pos) -> bool {
        self.start <= pos && pos < self.end
    }
}

/// How a visual selection extends from its anchor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VisualKind {
    /// Characterwise: from the anchor to the cursor.
    Char,
    /// Linewise: whole lines.
    Line,
    /// Blockwise: a rectangle of columns.
    Block,
}

/// One cursor: where it is, where its selection is anchored, and the column it
/// wants when moving vertically through short lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Cursor {
    pub(crate) pos: Pos,
    pub(crate) anchor: Option<Pos>,
    /// The column a vertical motion aims for; kept across short lines so
    /// moving down through a ragged block does not drift left.
    pub(crate) want_col: Option<usize>,
}

impl Cursor {
    pub(crate) fn new(pos: Pos) -> Self {
        Self {
            pos,
            anchor: None,
            want_col: None,
        }
    }

    /// The selection, ordered, when there is one.
    pub(crate) fn selection(&self) -> Option<Span> {
        self.anchor
            .filter(|a| *a != self.pos)
            .map(|a| Span::new(a, self.pos))
    }

    pub(crate) fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Move without extending the selection.
    pub(crate) fn set(&mut self, pos: Pos) {
        self.pos = pos;
        self.anchor = None;
        self.want_col = None;
    }

    /// Move and extend the selection from the current anchor.
    pub(crate) fn extend(&mut self, pos: Pos) {
        if self.anchor.is_none() {
            self.anchor = Some(self.pos);
        }
        self.pos = pos;
    }
}
