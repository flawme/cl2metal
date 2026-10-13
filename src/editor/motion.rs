//! Motions: where a cursor lands.
//!
//! Every motion is a pure function of the buffer and the starting position, so
//! they compose with counts and operators without special cases.

use super::buffer::Buffer;
use super::pos::Pos;

/// A cursor movement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Motion {
    Left,
    Right,
    Up,
    Down,
    /// `w` — start of the next word.
    WordNext,
    /// `e` — end of the current or next word.
    WordEnd,
    /// `b` — start of the previous word.
    WordBack,
    /// `W` — start of the next whitespace-delimited word.
    BigWordNext,
    /// `E`
    BigWordEnd,
    /// `B`
    BigWordBack,
    /// `0`
    LineStart,
    /// `$`
    LineEnd,
    /// `^`
    FirstNonBlank,
    /// `gg`
    DocStart,
    /// `G`
    DocEnd,
    /// `:N` or `NG`
    Line(usize),
    /// `f` / `t`: the target character, and whether to stop before it.
    FindChar(char, bool),
    /// `%`
    MatchBracket,
    /// `}`
    ParagraphNext,
    /// `{`
    ParagraphBack,
    /// `H` / `M` / `L`
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
    /// `n` / `N`
    SearchNext,
    SearchPrev,
}

impl Motion {
    /// Whether the motion covers whole lines, which makes an operator linewise.
    pub(crate) fn linewise(self) -> bool {
        matches!(
            self,
            Motion::Up
                | Motion::Down
                | Motion::ParagraphNext
                | Motion::ParagraphBack
                | Motion::DocStart
                | Motion::DocEnd
        )
    }

    /// Whether the motion includes the character it lands on.
    pub(crate) fn inclusive(self) -> bool {
        matches!(
            self,
            Motion::WordEnd | Motion::BigWordEnd | Motion::FindChar(..) | Motion::MatchBracket
        )
    }
}

/// A character that is part of a word (`w`, `e`, `b`).
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The character class a word motion steps over: 0 whitespace, 1 word,
/// 2 punctuation.
fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if is_word_char(c) {
        1
    } else {
        2
    }
}

/// The whitespace-delimited class used by the `W`/`E`/`B` motions.
fn big_class(c: char) -> u8 {
    u8::from(!c.is_whitespace())
}

fn class_at(buf: &Buffer, line: usize, col: usize) -> u8 {
    buf.char_at(Pos::new(line, col)).map_or(0, class)
}

fn big_class_at(buf: &Buffer, line: usize, col: usize) -> u8 {
    buf.char_at(Pos::new(line, col)).map_or(0, big_class)
}

/// Resolve a motion from `from`, repeated `count` times.
pub(crate) fn resolve(buf: &Buffer, from: Pos, motion: Motion, count: usize) -> Pos {
    let count = count.max(1);
    let mut pos = from;
    match motion {
        Motion::Line(n) => return buf.clamp(Pos::new(n.saturating_sub(1), 0)),
        Motion::DocStart => return Pos::new(0, 0),
        Motion::DocEnd => {
            let line = buf.line_count().saturating_sub(1);
            return Pos::new(line, buf.first_non_blank(line));
        }
        Motion::FindChar(target, before) => {
            return find_char(buf, from, target, before, count);
        }
        Motion::MatchBracket => return match_bracket(buf, from).unwrap_or(from),
        Motion::SearchNext | Motion::SearchPrev => return from,
        _ => {}
    }
    for _ in 0..count {
        pos = step(buf, pos, motion);
    }
    buf.clamp(pos)
}

/// One step of a motion. Public so text objects can walk the buffer too.
pub(crate) fn step(buf: &Buffer, pos: Pos, motion: Motion) -> Pos {
    match motion {
        Motion::Left => {
            if pos.col > 0 {
                Pos::new(pos.line, pos.col - 1)
            } else if pos.line > 0 {
                let line = pos.line - 1;
                Pos::new(line, buf.line_len(line))
            } else {
                pos
            }
        }
        Motion::Right => {
            if pos.col < buf.line_len(pos.line) {
                Pos::new(pos.line, pos.col + 1)
            } else if pos.line + 1 < buf.line_count() {
                Pos::new(pos.line + 1, 0)
            } else {
                pos
            }
        }
        Motion::Up => Pos::new(pos.line.saturating_sub(1), pos.col),
        Motion::Down => Pos::new((pos.line + 1).min(buf.line_count() - 1), pos.col),
        Motion::LineStart => Pos::new(pos.line, 0),
        Motion::LineEnd => {
            let len = buf.line_len(pos.line);
            Pos::new(pos.line, len.saturating_sub(1).max(0).min(len))
        }
        Motion::FirstNonBlank => Pos::new(pos.line, buf.first_non_blank(pos.line)),
        Motion::WordNext => word_next(buf, pos, class, class_at),
        Motion::WordEnd => word_end(buf, pos, class, class_at),
        Motion::WordBack => word_back(buf, pos, class, class_at),
        Motion::BigWordNext => word_next(buf, pos, big_class, big_class_at),
        Motion::BigWordEnd => word_end(buf, pos, big_class, big_class_at),
        Motion::BigWordBack => word_back(buf, pos, big_class, big_class_at),
        Motion::ParagraphNext => paragraph(buf, pos, true),
        Motion::ParagraphBack => paragraph(buf, pos, false),
        Motion::ScreenTop | Motion::ScreenMiddle | Motion::ScreenBottom => pos,
        Motion::Line(_) | Motion::DocStart | Motion::DocEnd | Motion::FindChar(..) => pos,
        Motion::MatchBracket => match_bracket(buf, pos).unwrap_or(pos),
        Motion::SearchNext | Motion::SearchPrev => pos,
    }
}

/// `w` / `W`: skip the run under the cursor, then any whitespace.
fn word_next(
    buf: &Buffer,
    pos: Pos,
    class_of: fn(char) -> u8,
    class_at: fn(&Buffer, usize, usize) -> u8,
) -> Pos {
    let (mut line, mut col) = (pos.line, pos.col);
    let start = class_at(buf, line, col);
    if start != 0 {
        while col < buf.line_len(line) && class_at(buf, line, col) == start {
            col += 1;
        }
    }
    loop {
        if col >= buf.line_len(line) {
            if line + 1 >= buf.line_count() {
                return Pos::new(line, buf.line_len(line));
            }
            line += 1;
            col = 0;
            continue;
        }
        if class_at(buf, line, col) == 0 {
            col += 1;
            continue;
        }
        break;
    }
    let _ = class_of;
    Pos::new(line, col)
}

/// `e` / `E`: forward at least one character, then to the end of that run.
fn word_end(
    buf: &Buffer,
    pos: Pos,
    _class_of: fn(char) -> u8,
    class_at: fn(&Buffer, usize, usize) -> u8,
) -> Pos {
    let (mut line, mut col) = (pos.line, pos.col);
    loop {
        if col + 1 >= buf.line_len(line) {
            if line + 1 >= buf.line_count() {
                return Pos::new(line, buf.line_len(line).saturating_sub(1));
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
        let c = class_at(buf, line, col);
        if c != 0 {
            while col + 1 < buf.line_len(line) && class_at(buf, line, col + 1) == c {
                col += 1;
            }
            return Pos::new(line, col);
        }
    }
}

/// `b` / `B`: back at least one character, then to the start of that run.
fn word_back(
    buf: &Buffer,
    pos: Pos,
    _class_of: fn(char) -> u8,
    class_at: fn(&Buffer, usize, usize) -> u8,
) -> Pos {
    let (mut line, mut col) = (pos.line, pos.col);
    loop {
        if col == 0 {
            if line == 0 {
                return Pos::new(0, 0);
            }
            line -= 1;
            col = buf.line_len(line);
            if col == 0 {
                continue;
            }
        } else {
            col -= 1;
        }
        let c = class_at(buf, line, col);
        if c != 0 {
            while col > 0 && class_at(buf, line, col - 1) == c {
                col -= 1;
            }
            return Pos::new(line, col);
        }
    }
}

/// `}` / `{`: the next or previous blank line.
fn paragraph(buf: &Buffer, pos: Pos, forward: bool) -> Pos {
    let mut line = pos.line;
    if forward {
        line += 1;
        while line < buf.line_count() && !buf.line(line).trim().is_empty() {
            line += 1;
        }
        Pos::new(line.min(buf.line_count().saturating_sub(1)), 0)
    } else {
        while line > 0 && !buf.line(line).trim().is_empty() {
            line -= 1;
        }
        while line > 0 && buf.line(line).trim().is_empty() {
            line -= 1;
        }
        Pos::new(line, 0)
    }
}

/// `f` / `t`: the next occurrence of `target` on this line.
fn find_char(buf: &Buffer, from: Pos, target: char, before: bool, count: usize) -> Pos {
    let line = buf.line(from.line);
    let chars: Vec<char> = line.chars().collect();
    let mut col = from.col;
    for _ in 0..count.max(1) {
        let mut found = None;
        for (i, c) in chars.iter().enumerate().skip(col + 1) {
            if *c == target {
                found = Some(i);
                break;
            }
        }
        match found {
            Some(i) => col = if before { i.saturating_sub(1) } else { i },
            None => return from,
        }
    }
    Pos::new(from.line, col)
}

/// The bracket pairs `%` understands.
fn bracket_pair(c: char) -> Option<(char, char, bool)> {
    match c {
        '(' => Some(('(', ')', true)),
        ')' => Some(('(', ')', false)),
        '[' => Some(('[', ']', true)),
        ']' => Some(('[', ']', false)),
        '{' => Some(('{', '}', true)),
        '}' => Some(('{', '}', false)),
        '<' => Some(('<', '>', true)),
        '>' => Some(('<', '>', false)),
        _ => None,
    }
}

/// `%`: the matching bracket, scanning with nesting.
pub(crate) fn match_bracket(buf: &Buffer, pos: Pos) -> Option<Pos> {
    let here = buf.char_at(pos)?;
    let (open, close, forward) = bracket_pair(here)?;
    let mut depth = 0i32;
    let mut p = pos;
    loop {
        let c = buf.char_at(p)?;
        if c == open {
            depth += if forward { 1 } else { -1 };
        } else if c == close {
            depth += if forward { -1 } else { 1 };
        }
        if depth == 0 && p != pos {
            return Some(p);
        }
        p = if forward {
            let next = step(buf, p, Motion::Right);
            if next == p {
                return None;
            }
            next
        } else {
            let next = step(buf, p, Motion::Left);
            if next == p {
                return None;
            }
            next
        };
    }
}

/// The bracket pair enclosing `pos`, for highlighting: `(open, close)`.
pub(crate) fn enclosing_brackets(buf: &Buffer, pos: Pos) -> Option<(Pos, Pos)> {
    let here = buf.char_at(pos)?;
    if bracket_pair(here).is_some() {
        return match_bracket(buf, pos).map(|m| if m < pos { (m, pos) } else { (pos, m) });
    }
    // Otherwise look at the character just before the cursor, which is where a
    // closing bracket sits when the cursor is right after it.
    if pos.col > 0 {
        let before = Pos::new(pos.line, pos.col - 1);
        if let Some(c) = buf.char_at(before)
            && bracket_pair(c).is_some()
        {
            return match_bracket(buf, before)
                .map(|m| if m < before { (m, before) } else { (before, m) });
        }
    }
    None
}
