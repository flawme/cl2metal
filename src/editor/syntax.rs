//! Syntax highlighting for the editor, built on `syntect` with its pure-Rust
//! `fancy-regex` backend.
//!
//! Loading the bundled syntax and theme sets is expensive, so they are loaded
//! once into a [`Highlighter`] and reused for every document. A document is
//! highlighted by a single [`HighlightLines`] pass that walks the lines in
//! order, so multi-line constructs (block comments, raw strings, heredocs)
//! keep their state across line boundaries.

// The editor is not wired into the app binary yet, so from the bin target's
// point of view this whole module is dead code. Drop this once it is.
#![allow(dead_code)]

use std::ops::Range;
use std::path::Path;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, FontStyle, Style as SyntectStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// A resolved style for a span of text.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Style {
    pub(crate) fg: [u8; 3],
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
}

/// Highlighted spans of one line: byte ranges into that line.
pub(crate) type LineSpans = Vec<(Range<usize>, Style)>;

/// Bundled dark theme used for the editor.
const THEME_NAME: &str = "base16-ocean.dark";

/// Foreground used if the theme does not define one (base16 ocean's light
/// grey).
const FALLBACK_FG: [u8; 3] = [0xc0, 0xc5, 0xce];

/// Background used if the theme does not define one (base16 ocean's dark
/// blue-grey).
const FALLBACK_BG: [u8; 3] = [0x2b, 0x30, 0x3b];

/// A reusable highlighter: the syntax and theme sets are loaded once.
pub(crate) struct Highlighter {
    syntax_set: SyntaxSet,
    theme: Theme,
    default_fg: [u8; 3],
    background: [u8; 3],
}

impl Highlighter {
    /// Load the bundled syntaxes and the dark theme.
    pub(crate) fn new() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let theme = theme_set
            .themes
            .get(THEME_NAME)
            .or_else(|| theme_set.themes.values().next())
            .cloned()
            .unwrap_or_default();
        let default_fg = theme
            .settings
            .foreground
            .map(color_to_rgb)
            .unwrap_or(FALLBACK_FG);
        let background = theme
            .settings
            .background
            .map(color_to_rgb)
            .unwrap_or(FALLBACK_BG);
        Self {
            syntax_set,
            theme,
            default_fg,
            background,
        }
    }

    /// Highlight a whole document. `path` selects the syntax by extension or
    /// file name; unknown files fall back to plain text.
    pub(crate) fn highlight(&self, path: Option<&Path>, text: &str) -> Vec<LineSpans> {
        let syntax = self.syntax_for(path, text);
        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        let lines = split_lines(text);
        let mut out = Vec::with_capacity(lines.len());
        // Reused across lines so a large document does not allocate once per
        // line. The parser is fed newline-terminated lines because the bundled
        // syntaxes are the newline variants.
        let mut feed = String::new();
        for line in lines {
            feed.clear();
            feed.push_str(line);
            feed.push('\n');
            let mut spans: LineSpans = Vec::new();
            match highlighter.highlight_line(&feed, &self.syntax_set) {
                Ok(pieces) => {
                    let mut offset = 0;
                    for (style, piece) in pieces {
                        let start = offset;
                        offset += piece.len();
                        spans.push((start..offset, map_style(style)));
                    }
                }
                Err(_) => {
                    // The parser gave up on this line (for example a regex
                    // backtrack limit): paint it plainly and carry on with the
                    // next one.
                    spans.push((0..feed.len(), self.default_style()));
                }
            }
            out.push(self.normalize(spans, line.len()));
        }
        out
    }

    /// The theme's default foreground, for text with no span.
    pub(crate) fn default_fg(&self) -> [u8; 3] {
        self.default_fg
    }

    /// The theme's background, so the editor can blend with it.
    pub(crate) fn background(&self) -> [u8; 3] {
        self.background
    }

    /// Pick the syntax for `path`, falling back to a shebang match on the
    /// buffer's first line and finally to plain text. The buffer is inspected
    /// rather than the file on disk because it may be unsaved or modified.
    fn syntax_for(&self, path: Option<&Path>, text: &str) -> &SyntaxReference {
        if let Some(path) = path {
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let extension = path.extension().and_then(|x| x.to_str()).unwrap_or("");
            let by_name = if file_name.is_empty() {
                None
            } else {
                self.syntax_set.find_syntax_by_extension(file_name)
            };
            let found = by_name.or_else(|| {
                if extension.is_empty() {
                    None
                } else {
                    self.syntax_set.find_syntax_by_extension(extension)
                }
            });
            if let Some(syntax) = found {
                return syntax;
            }
        }
        let first_line = text.split('\n').next().unwrap_or("");
        self.syntax_set
            .find_syntax_by_first_line(first_line)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
    }

    /// Clamp, sort and gap-fill `spans` so that they tile `0..len` exactly:
    /// ascending, non-overlapping, in bounds, with no gaps.
    fn normalize(&self, spans: LineSpans, len: usize) -> LineSpans {
        let mut out: LineSpans = Vec::with_capacity(spans.len() + 1);
        let mut cursor = 0;
        for (range, style) in spans {
            let start = range.start.min(len);
            let end = range.end.min(len);
            if start >= end {
                // Empty or out of bounds: nothing to draw.
                continue;
            }
            if start > cursor {
                out.push((cursor..start, self.default_style()));
            }
            let start = start.max(cursor);
            if end > start {
                out.push((start..end, style));
                cursor = end;
            }
        }
        if cursor < len {
            out.push((cursor..len, self.default_style()));
        }
        if out.is_empty() {
            // Every line carries at least one span, so the renderer can walk
            // the spans without special-casing empty lines.
            out.push((0..len, self.default_style()));
        }
        out
    }

    /// The style for text the theme has no rule for.
    fn default_style(&self) -> Style {
        Style {
            fg: self.default_fg,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

/// Split `text` into lines the way the editor stores them: on `'\n'`, with one
/// trailing `'\r'` removed, and without a phantom final line when the text ends
/// in a newline. `"a\nb\n"` and `"a\nb"` both yield two lines, `""` yields one
/// empty line.
fn split_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').map(strip_cr).collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Remove one trailing `'\r'`, if present.
fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// Convert a syntect style into the editor's compact style.
fn map_style(style: SyntectStyle) -> Style {
    Style {
        fg: color_to_rgb(style.foreground),
        bold: style.font_style.contains(FontStyle::BOLD),
        italic: style.font_style.contains(FontStyle::ITALIC),
        underline: style.font_style.contains(FontStyle::UNDERLINE),
    }
}

/// Drop the alpha channel: the editor draws opaque text.
fn color_to_rgb(color: Color) -> [u8; 3] {
    [color.r, color.g, color.b]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Assert the per-line span invariants: ascending, non-overlapping, in
    /// bounds, and tiling the whole line with no gaps.
    fn assert_well_formed(lines: &[LineSpans], text: &str) {
        let expected = split_lines(text);
        assert_eq!(lines.len(), expected.len(), "one LineSpans per line");
        for (spans, line) in lines.iter().zip(expected) {
            let mut cursor = 0;
            for (range, _) in spans {
                assert!(range.start <= range.end, "inverted range {range:?}");
                assert!(
                    range.start < range.end || line.is_empty(),
                    "empty range {range:?} on non-empty line {line:?}"
                );
                assert_eq!(
                    range.start, cursor,
                    "gap or overlap at {range:?} in {line:?}"
                );
                assert!(
                    range.end <= line.len(),
                    "range {range:?} out of bounds for {line:?}"
                );
                cursor = range.end;
            }
            assert_eq!(cursor, line.len(), "spans do not cover {line:?}");
        }
    }

    #[test]
    fn rust_source_highlights_keywords_and_strings() {
        let h = Highlighter::new();
        let text = "fn main() {\n    let s = \"hi\";\n}\n";
        let lines = h.highlight(Some(Path::new("main.rs")), text);
        assert_eq!(lines.len(), 3);

        let mut colours = BTreeSet::new();
        for spans in &lines {
            for (_, style) in spans {
                colours.insert(style.fg);
            }
        }
        assert!(
            colours.len() >= 2,
            "expected at least two foreground colours, got {colours:?}"
        );

        let string_line = split_lines(text)[1];
        let start = string_line.find("\"hi\"").expect("literal is present");
        // syntect scopes the quotes separately from the contents, so the span
        // carrying the string colour is the one over the contents.
        let contents = start + 1;
        let span = lines[1]
            .iter()
            .find(|(range, _)| range.start <= contents && range.end >= contents + 2)
            .expect("a span covers the string contents");
        assert_ne!(
            span.1.fg,
            h.default_fg(),
            "the string literal must not use the default colour"
        );
    }

    #[test]
    fn spans_cover_every_line_and_stay_in_bounds() {
        let h = Highlighter::new();
        let cases: [(&str, &str); 6] = [
            ("main.rs", "fn main() {\n    let s = \"hi\";\n}\n"),
            ("main.py", "def f(x):\n    return x + 1\n"),
            ("data.json", "{\n  \"a\": [1, 2],\n  \"b\": null\n}\n"),
            ("Cargo.toml", "[package]\nname = \"cl2metal\"\n"),
            ("notes.md", "# Title\n\nSome *text*.\n"),
            ("mystery.zzz", "???\nplain words\n"),
        ];
        for (name, text) in cases {
            let lines = h.highlight(Some(Path::new(name)), text);
            assert_well_formed(&lines, text);
        }
        // No path at all: still well formed.
        let text = "fn main() {}\n";
        assert_well_formed(&h.highlight(None, text), text);
    }

    #[test]
    fn crlf_lines_are_stripped() {
        let h = Highlighter::new();
        let text = "fn main() {\r\n    let x = 1;\r\n}\r\n";
        let lines = h.highlight(Some(Path::new("main.rs")), text);
        let stripped = split_lines(text);
        assert_eq!(stripped, vec!["fn main() {", "    let x = 1;", "}"]);
        assert_eq!(lines.len(), stripped.len());
        for (spans, line) in lines.iter().zip(&stripped) {
            assert!(!spans.is_empty());
            let mut cursor = 0;
            for (range, _) in spans {
                assert_eq!(range.start, cursor);
                assert!(
                    range.end <= line.len(),
                    "range {range:?} reaches past the stripped length {}",
                    line.len()
                );
                cursor = range.end;
            }
            assert_eq!(cursor, line.len());
        }
    }

    #[test]
    fn empty_text_yields_one_empty_line() {
        let h = Highlighter::new();
        let lines = h.highlight(None, "");
        assert_eq!(lines.len(), 1);
        for (range, style) in &lines[0] {
            assert_eq!(
                range.end, 0,
                "range {range:?} out of bounds for an empty line"
            );
            assert_eq!(style.fg, h.default_fg());
        }
    }

    #[test]
    fn line_count_matches_the_editor() {
        let h = Highlighter::new();
        // A trailing newline terminates the last line rather than starting a
        // phantom empty one.
        assert_eq!(h.highlight(None, "a\nb\n").len(), 2);
        assert_eq!(h.highlight(None, "a\nb").len(), 2);
        assert_eq!(h.highlight(None, "a\r\nb\r\n").len(), 2);
        assert_eq!(h.highlight(None, "").len(), 1);
        assert_eq!(h.highlight(None, "\n").len(), 1);
        assert_eq!(h.highlight(None, "a\n\nb").len(), 3);
    }

    #[test]
    fn multiline_constructs_keep_their_state() {
        let h = Highlighter::new();

        // A block comment swallows the code-looking line inside it.
        let text = "/* a comment\nfn main() {}\n*/\nfn real() {}\n";
        let lines = h.highlight(Some(Path::new("a.rs")), text);
        assert_well_formed(&lines, text);
        let keyword_fg = lines[3]
            .iter()
            .find(|(range, _)| range.start == 0 && range.end >= 2)
            .expect("a span covers `fn`")
            .1
            .fg;
        assert_ne!(
            keyword_fg,
            h.default_fg(),
            "`fn` should be a keyword colour"
        );
        assert!(
            lines[1].iter().all(|(_, style)| style.fg != keyword_fg),
            "a line inside a block comment must not be highlighted as code"
        );

        // A raw string carries on across the line break.
        let text = "let s = r#\"raw\nstill raw\"#;\nlet t = 1;\n";
        let lines = h.highlight(Some(Path::new("b.rs")), text);
        assert_well_formed(&lines, text);
        let first_len = split_lines(text)[0].len();
        let string_fg = lines[0]
            .iter()
            .find(|(range, _)| range.end == first_len)
            .expect("a span ends the first line")
            .1
            .fg;
        assert_ne!(
            string_fg,
            h.default_fg(),
            "the raw string should be coloured"
        );
        assert!(
            lines[1]
                .iter()
                .any(|(range, style)| range.start == 0 && style.fg == string_fg),
            "the raw string must continue onto the next line"
        );
    }

    #[test]
    fn unknown_extension_falls_back_to_plain_text() {
        let h = Highlighter::new();
        let text = "not really code\njust words\n";
        let lines = h.highlight(Some(Path::new("mystery.zzz")), text);
        assert_well_formed(&lines, text);
        for spans in &lines {
            for (_, style) in spans {
                assert_eq!(
                    style.fg,
                    h.default_fg(),
                    "plain text uses the default colour"
                );
            }
        }
    }

    /// Timing is only meaningful in an optimised build: with debug assertions
    /// on, the regex engine runs roughly ten times slower. The pure-Rust
    /// `fancy-regex` backend manages about 0.5 MB/s on Rust source, so a 1 MB
    /// document takes on the order of two seconds; the bound below is
    /// deliberately loose and exists to catch pathological regressions (such as
    /// loading or cloning the syntax set per call), not to enforce a
    /// sub-second budget that this backend cannot meet.
    #[cfg(not(debug_assertions))]
    #[test]
    fn one_megabyte_highlights_without_pathological_slowdown() {
        use std::time::{Duration, Instant};

        let h = Highlighter::new();
        let unit = "fn main() {\n    let s = \"hi\";\n    // comment\n}\n";
        let mut text = String::with_capacity(1 << 20);
        while text.len() < 1 << 20 {
            text.push_str(unit);
        }
        let start = Instant::now();
        let lines = h.highlight(Some(Path::new("big.rs")), &text);
        let elapsed = start.elapsed();
        println!(
            "highlighted {} bytes into {} lines in {elapsed:?}",
            text.len(),
            lines.len()
        );
        assert_eq!(lines.len(), split_lines(&text).len());
        assert!(
            elapsed < Duration::from_secs(10),
            "1 MB took {elapsed:?}, which suggests the syntax set is being \
             reloaded or cloned per call"
        );
    }
}
