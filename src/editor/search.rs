//! Search and replace inside a buffer.

use super::buffer::Buffer;
use super::pos::{Pos, Span};

/// A search query.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Query {
    pub(crate) text: String,
    pub(crate) case_sensitive: bool,
    pub(crate) whole_word: bool,
    pub(crate) regex: bool,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            text: String::new(),
            case_sensitive: true,
            whole_word: false,
            regex: false,
        }
    }
}

impl Query {
    pub(crate) fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            ..Self::default()
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// A compiled regex, when the query asks for one.
    fn compiled(&self) -> Option<regex::Regex> {
        if !self.regex {
            return None;
        }
        let mut pattern = self.text.clone();
        if self.whole_word {
            pattern = format!(r"\b(?:{pattern})\b");
        }
        let pattern = if self.case_sensitive {
            pattern
        } else {
            format!("(?i){pattern}")
        };
        regex::Regex::new(&pattern).ok()
    }
}

/// How many matches a search will collect before giving up.
pub(crate) const MATCH_CAP: usize = 5000;

/// Every match in the buffer, in reading order.
pub(crate) fn find_all(buf: &Buffer, query: &Query) -> Vec<Span> {
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let re = query.compiled();
    for line in 0..buf.line_count() {
        let text = buf.line(line);
        for (start, end) in line_matches(text, query, re.as_ref()) {
            out.push(Span::new(
                Pos::new(line, buf.col_of_byte(line, start)),
                Pos::new(line, buf.col_of_byte(line, end)),
            ));
            if out.len() >= MATCH_CAP {
                return out;
            }
        }
    }
    out
}

/// Byte ranges of the matches within one line.
fn line_matches(text: &str, query: &Query, re: Option<&regex::Regex>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if let Some(re) = re {
        for m in re.find_iter(text) {
            if m.start() == m.end() {
                continue;
            }
            out.push((m.start(), m.end()));
        }
        return out;
    }
    let hay = if query.case_sensitive {
        text.to_owned()
    } else {
        text.to_lowercase()
    };
    let needle = if query.case_sensitive {
        query.text.clone()
    } else {
        query.text.to_lowercase()
    };
    if needle.is_empty() {
        return out;
    }
    let mut from = 0;
    while let Some(rel) = hay[from..].find(&needle) {
        let start = from + rel;
        let end = start + needle.len();
        // Lowercasing can change byte lengths; only accept matches whose
        // offsets are still valid in the original text.
        if end <= text.len() && text.is_char_boundary(start) && text.is_char_boundary(end) {
            let whole_ok = !query.whole_word || {
                let before = text[..start].chars().next_back();
                let after = text[end..].chars().next();
                !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
            };
            if whole_ok {
                out.push((start, end));
            }
        }
        from = start + needle.len().max(1);
        if from >= hay.len() {
            break;
        }
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The next match at or after `from`, wrapping to the start.
pub(crate) fn find_next(buf: &Buffer, query: &Query, from: Pos, backward: bool) -> Option<Span> {
    let all = find_all(buf, query);
    if all.is_empty() {
        return None;
    }
    if backward {
        all.iter()
            .rev()
            .find(|m| m.start < from)
            .or_else(|| all.last())
            .copied()
    } else {
        all.iter()
            .find(|m| m.start > from)
            .or_else(|| all.first())
            .copied()
    }
}

/// Replace one span; returns the position after the replacement.
pub(crate) fn replace_one(buf: &mut Buffer, span: Span, with: &str) -> Pos {
    buf.replace(span, with, false)
}

/// Replace every match; returns how many were replaced.
pub(crate) fn replace_all(buf: &mut Buffer, query: &Query, with: &str) -> usize {
    let matches = find_all(buf, query);
    if matches.is_empty() {
        return 0;
    }
    // Replace from the end so earlier spans keep their offsets.
    let mut count = 0;
    for span in matches.into_iter().rev() {
        buf.replace(span, with, false);
        count += 1;
    }
    count
}
