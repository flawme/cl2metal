//! Operators and text objects.
//!
//! An operator acts on a span; a text object produces a span. Keeping the two
//! apart is what lets `d`, `c`, `y`, `>` and `~` share one implementation and
//! work with motions, text objects and visual selections alike.

use super::buffer::Buffer;
use super::motion::{Motion, is_word_char, step};
use super::pos::{Pos, Span};

/// A command that acts on a span of text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Dedent,
    Upper,
    Lower,
    ToggleCase,
    Comment,
}

impl Operator {
    /// Whether the operator leaves the editor in insert mode.
    pub(crate) fn enters_insert(self) -> bool {
        matches!(self, Operator::Change)
    }

    /// Whether the operator takes text into a register.
    pub(crate) fn yanks(self) -> bool {
        matches!(self, Operator::Delete | Operator::Change | Operator::Yank)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Operator::Delete => "delete",
            Operator::Change => "change",
            Operator::Yank => "yank",
            Operator::Indent => "indent",
            Operator::Dedent => "dedent",
            Operator::Upper => "uppercase",
            Operator::Lower => "lowercase",
            Operator::ToggleCase => "toggle case",
            Operator::Comment => "comment",
        }
    }
}

/// A text object: the span `i`/`a` selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TextObject {
    InnerWord,
    AWord,
    InnerBigWord,
    ABigWord,
    InnerQuote(char),
    AQuote(char),
    InnerParen,
    AParen,
    InnerBrace,
    ABrace,
    InnerBracket,
    ABracket,
    InnerParagraph,
    AParagraph,
}

/// The span a text object covers at `pos`.
pub(crate) fn text_object(buf: &Buffer, pos: Pos, obj: TextObject) -> Option<Span> {
    match obj {
        TextObject::InnerWord => word_object(buf, pos, false, false),
        TextObject::AWord => word_object(buf, pos, true, false),
        TextObject::InnerBigWord => word_object(buf, pos, false, true),
        TextObject::ABigWord => word_object(buf, pos, true, true),
        TextObject::InnerQuote(q) => quote_object(buf, pos, q, true),
        TextObject::AQuote(q) => quote_object(buf, pos, q, false),
        TextObject::InnerParen => bracket_object(buf, pos, '(', ')', true),
        TextObject::AParen => bracket_object(buf, pos, '(', ')', false),
        TextObject::InnerBrace => bracket_object(buf, pos, '{', '}', true),
        TextObject::ABrace => bracket_object(buf, pos, '{', '}', false),
        TextObject::InnerBracket => bracket_object(buf, pos, '[', ']', true),
        TextObject::ABracket => bracket_object(buf, pos, '[', ']', false),
        TextObject::InnerParagraph => paragraph_object(buf, pos, true),
        TextObject::AParagraph => paragraph_object(buf, pos, false),
    }
}

/// The run of same-class characters around `pos`, optionally with the
/// whitespace that follows it (`aw`).
fn word_object(buf: &Buffer, pos: Pos, around: bool, big: bool) -> Option<Span> {
    let line = buf.line(pos.line);
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return Some(Span::new(pos, pos));
    }
    let col = pos.col.min(chars.len() - 1);
    let class = |c: char| {
        if big {
            u8::from(!c.is_whitespace())
        } else if c.is_whitespace() {
            0
        } else if is_word_char(c) {
            1
        } else {
            2
        }
    };
    let here = class(chars[col]);
    let mut start = col;
    while start > 0 && class(chars[start - 1]) == here {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && class(chars[end + 1]) == here {
        end += 1;
    }
    end += 1;
    if around {
        // Trailing whitespace belongs to `aw`; at the end of a line it is the
        // leading whitespace instead, as in Vim.
        let mut after = end;
        while after < chars.len() && chars[after].is_whitespace() {
            after += 1;
        }
        if after > end {
            end = after;
        } else {
            while start > 0 && chars[start - 1].is_whitespace() {
                start -= 1;
            }
        }
    }
    Some(Span::new(
        Pos::new(pos.line, start),
        Pos::new(pos.line, end),
    ))
}

/// The text between a pair of quotes on this line.
///
/// Escaped quotes are not special-cased: a `\"` inside a string ends the object
/// early, which is the same trade-off several small editors make.
fn quote_object(buf: &Buffer, pos: Pos, q: char, inner: bool) -> Option<Span> {
    let chars: Vec<char> = buf.line(pos.line).chars().collect();
    let quotes: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == q)
        .map(|(i, _)| i)
        .collect();
    if quotes.len() < 2 {
        return None;
    }
    let pair = quotes
        .windows(2)
        .find(|w| pos.col >= w[0] && pos.col <= w[1])
        .or_else(|| quotes.windows(2).find(|w| w[0] > pos.col))?;
    let (open, close) = (pair[0], pair[1]);
    Some(if inner {
        Span::new(Pos::new(pos.line, open + 1), Pos::new(pos.line, close))
    } else {
        Span::new(Pos::new(pos.line, open), Pos::new(pos.line, close + 1))
    })
}

/// The span between the enclosing bracket pair, found with nesting.
fn bracket_object(buf: &Buffer, pos: Pos, open: char, close: char, inner: bool) -> Option<Span> {
    // On the opening bracket itself, that is the start.
    let start = if buf.char_at(pos) == Some(open) {
        pos
    } else {
        // Otherwise walk back to the bracket we are inside: the first opening
        // bracket that is not already matched by a closing one on the way.
        let mut depth = 0i32;
        let mut p = pos;
        loop {
            let c = buf.char_at(p)?;
            if c == close {
                depth += 1;
            } else if c == open {
                depth -= 1;
                if depth <= 0 {
                    break p;
                }
            }
            let prev = step(buf, p, Motion::Left);
            if prev == p {
                return None;
            }
            p = prev;
        }
    };
    // Then forward to its match.
    let mut depth = 0i32;
    let mut p = start;
    let end = loop {
        let c = buf.char_at(p)?;
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                break p;
            }
        }
        let next = step(buf, p, Motion::Right);
        if next == p {
            return None;
        }
        p = next;
    };
    Some(if inner {
        Span::new(step(buf, start, Motion::Right), end)
    } else {
        Span::new(start, step(buf, end, Motion::Right))
    })
}

/// The block of non-blank lines around `pos`; `around` includes the blank line
/// that follows it.
fn paragraph_object(buf: &Buffer, pos: Pos, inner: bool) -> Option<Span> {
    let mut start = pos.line;
    while start > 0 && !buf.line(start - 1).trim().is_empty() {
        start -= 1;
    }
    let mut end = pos.line;
    while end + 1 < buf.line_count() && !buf.line(end + 1).trim().is_empty() {
        end += 1;
    }
    let end_col = buf.line_len(end);
    if inner {
        Some(Span::new(Pos::new(start, 0), Pos::new(end, end_col)))
    } else {
        let after = (end + 1).min(buf.line_count().saturating_sub(1));
        Some(Span::new(
            Pos::new(start, 0),
            Pos::new(after, buf.line_len(after)),
        ))
    }
}

/// Expand a span to whole lines, for linewise operators and `V`.
pub(crate) fn linewise_span(buf: &Buffer, span: Span) -> Span {
    let start = Pos::new(span.start.line, 0);
    let end_line = span.end.line.min(buf.line_count().saturating_sub(1));
    // A span that ends at column 0 does not include that line.
    let end_line = if span.end.col == 0 && end_line > span.start.line {
        end_line - 1
    } else {
        end_line
    };
    let end = Pos::new(end_line, buf.line_len(end_line));
    Span::new(start, end)
}

/// The span a block selection covers on one line, given the anchor and cursor
/// columns. Blocks are inclusive of the cursor column, as in Vim.
pub(crate) fn block_span_on_line(
    buf: &Buffer,
    line: usize,
    anchor_col: usize,
    cursor_col: usize,
) -> Span {
    let (a, b) = if anchor_col <= cursor_col {
        (anchor_col, cursor_col)
    } else {
        (cursor_col, anchor_col)
    };
    let len = buf.line_len(line);
    let start = a.min(len);
    let end = (b + 1).min(len);
    Span::new(Pos::new(line, start), Pos::new(line, end))
}
