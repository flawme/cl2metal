//! Configurable keybindings: chords, Vim-style multi-key sequences, conflict
//! detection and a `config.toml` round-trip.
//!
//! A binding is a sequence of one or more [`KeyChord`]s. A single chord covers
//! the usual `ctrl+s`-style shortcut; a multi-chord sequence covers Vim-style
//! normal-mode commands such as `g g`. Config entries are
//! `command = "sequence"` pairs, e.g. `save = "ctrl+s"` or
//! `goto_definition = "g d"`.

// The editor is not wired into the app binary yet, so from the bin target's
// point of view this whole module is dead code. Drop this once it is.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

/// One key press: a key plus the modifiers that matter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct KeyChord {
    pub(crate) key: ChordKey,
    pub(crate) ctrl: bool,
    pub(crate) shift: bool,
    pub(crate) alt: bool,
}

/// A key, named stably so a config file survives a round-trip.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum ChordKey {
    Char(char), // lowercase letters and digits
    Enter,
    Esc,
    Tab,
    Space,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

/// A command bound to a key sequence.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Binding {
    pub(crate) keys: Vec<KeyChord>,
    pub(crate) command: String,
}

/// What the pending chords mean.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Resolution {
    /// The pending chords are exactly a binding.
    Match(String),
    /// The pending chords are a strict prefix of at least one binding.
    Prefix,
    /// Nothing is bound to this.
    None,
}

/// The resolved keymap.
#[derive(Clone, Debug)]
pub(crate) struct Keymap {
    /// Bindings in a stable order: the defaults, then any additions.
    bindings: Vec<Binding>,
    /// Config command names that are not known commands.
    unknown: Vec<String>,
    /// Config command names whose sequence could not be parsed.
    invalid: Vec<String>,
}

impl Keymap {
    /// Defaults, overridden by `command = "sequence"` entries from the config.
    ///
    /// Unknown command names and unparseable sequences are ignored; they are
    /// reported by [`Keymap::unknown_commands`] and [`Keymap::invalid_entries`].
    pub(crate) fn from_config(config: &BTreeMap<String, String>) -> Self {
        let known: BTreeSet<&str> = defaults().into_iter().map(|(command, _)| command).collect();
        let mut keymap = Self::defaults_only();
        for (command, sequence) in config {
            if !known.contains(command.as_str()) {
                keymap.unknown.push(command.clone());
                continue;
            }
            match parse_sequences(sequence) {
                Ok(list) => keymap.replace_all(command, list),
                Err(_) => keymap.invalid.push(command.clone()),
            }
        }
        keymap
    }

    /// The current bindings, for saving back to config.toml.
    pub(crate) fn to_config(&self) -> BTreeMap<String, String> {
        // A command may hold several sequences; they are written as a list so
        // none of them is lost on the way back in.
        let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for binding in &self.bindings {
            grouped
                .entry(binding.command.clone())
                .or_default()
                .push(format_sequence(&binding.keys));
        }
        grouped
            .into_iter()
            .map(|(command, sequences)| (command, sequences.join(LIST_SEP)))
            .collect()
    }

    /// Command names in the config that are not known commands.
    pub(crate) fn unknown_commands(&self) -> Vec<String> {
        self.unknown.clone()
    }

    /// Config entries whose sequence could not be parsed: the command names.
    pub(crate) fn invalid_entries(&self) -> Vec<String> {
        self.invalid.clone()
    }

    /// Resolve the chords typed so far. An exact match wins over a prefix.
    pub(crate) fn resolve(&self, pending: &[KeyChord]) -> Resolution {
        let mut prefix = false;
        for binding in &self.bindings {
            if binding.keys == pending {
                return Resolution::Match(binding.command.clone());
            }
            if binding.keys.len() > pending.len() && binding.keys.starts_with(pending) {
                prefix = true;
            }
        }
        if prefix {
            Resolution::Prefix
        } else {
            Resolution::None
        }
    }

    /// The current bindings, in a stable order: the defaults, then additions.
    pub(crate) fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// Replace (or add) the binding for a command.
    ///
    /// The sequence must parse and the command name must not be empty. A
    /// successful call also clears the command from the config reports.
    pub(crate) fn set(&mut self, command: &str, sequence: &str) -> Result<(), String> {
        let command = command.trim();
        if command.is_empty() {
            return Err("command name is empty".to_string());
        }
        let keys = parse_sequence(sequence)?;
        self.replace_all(command, vec![keys]);
        self.unknown.retain(|name| name != command);
        self.invalid.retain(|name| name != command);
        Ok(())
    }

    /// Drop a command's binding.
    pub(crate) fn unbind(&mut self, command: &str) {
        self.bindings.retain(|binding| binding.command != command);
    }

    /// Back to the defaults.
    pub(crate) fn reset(&mut self) {
        *self = Self::defaults_only();
    }

    /// The defaults, with no config applied.
    fn defaults_only() -> Self {
        let bindings = defaults()
            .into_iter()
            .map(|(command, sequence)| Binding {
                keys: parse_sequence(sequence).expect("default sequences are valid"),
                command: command.to_string(),
            })
            .collect();
        Self {
            bindings,
            unknown: Vec::new(),
            invalid: Vec::new(),
        }
    }

    /// Replace every binding for `command` with `list`, or append them if the
    /// command is new. A command can hold more than one sequence — `undo` has
    /// both `u` and `ctrl+z` — so an override replaces the whole set.
    fn replace_all(&mut self, command: &str, list: Vec<Vec<KeyChord>>) {
        self.bindings.retain(|binding| binding.command != command);
        for keys in list {
            self.bindings.push(Binding {
                keys,
                command: command.to_string(),
            });
        }
    }
}

/// Parse "ctrl+shift+p" (one chord) or "g g" (a Vim-style sequence).
pub(crate) fn parse_sequence(s: &str) -> Result<Vec<KeyChord>, String> {
    let chords = s
        .split_whitespace()
        .map(parse_chord)
        .collect::<Result<Vec<_>, _>>()?;
    if chords.is_empty() {
        return Err("empty key sequence".to_string());
    }
    Ok(chords)
}

/// The separator between the sequences of one command in the config.
///
/// A comma on its own cannot be the separator: `ctrl+,` is a valid binding. A
/// comma *followed by a space* can, because `format_sequence` never emits one
/// (chords are joined by a single space and a chord never ends in a comma).
pub(crate) const LIST_SEP: &str = ", ";

/// Parse one sequence, or several separated by [`LIST_SEP`].
///
/// The list form is what lets a command keep a Vim binding and a conventional
/// one at once: `undo = "u, ctrl+z"`.
pub(crate) fn parse_sequences(s: &str) -> Result<Vec<Vec<KeyChord>>, String> {
    let mut out = Vec::new();
    for part in s.split(LIST_SEP) {
        if part.trim().is_empty() {
            return Err("empty key sequence".to_string());
        }
        out.push(parse_sequence(part)?);
    }
    if out.is_empty() {
        return Err("empty key sequence".to_string());
    }
    Ok(out)
}

/// The canonical string for a sequence; the inverse of `parse_sequence`.
pub(crate) fn format_sequence(chords: &[KeyChord]) -> String {
    chords
        .iter()
        .map(format_chord)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The default `(command, sequence)` pairs.
///
/// The single-key entries are Vim's normal-mode bindings, so the editor is
/// usable before anything is configured; the Ctrl entries are the ones a
/// desktop application is expected to have. The set must stay conflict-free
/// (there is a test); a name the editor's `Command::from_name` does not know
/// simply does nothing when pressed.
pub(crate) fn defaults() -> Vec<(&'static str, &'static str)> {
    vec![
        // Motions.
        ("move_left", "h"),
        ("move_down", "j"),
        ("move_up", "k"),
        ("move_right", "l"),
        ("move_word_next", "w"),
        ("move_word_end", "e"),
        ("move_word_back", "b"),
        ("move_big_word_next", "shift+w"),
        ("move_big_word_end", "shift+e"),
        ("move_big_word_back", "shift+b"),
        ("move_line_start", "0"),
        ("move_line_end", "$"),
        ("move_first_non_blank", "^"),
        ("move_paragraph_next", "}"),
        ("move_paragraph_back", "{"),
        ("goto_first_line", "g g"),
        ("goto_last_line", "shift+g"),
        ("goto_bracket", "%"),
        ("goto_definition", "g d"),
        // Insert mode.
        ("insert_before", "i"),
        ("insert_after", "a"),
        ("insert_line_start", "shift+i"),
        ("insert_line_end", "shift+a"),
        ("open_below", "o"),
        ("open_above", "shift+o"),
        // Operators and edits.
        ("delete", "d"),
        ("change", "c"),
        ("yank", "y"),
        ("delete_line", "d d"),
        ("yank_line", "y y"),
        ("delete_char", "x"),
        ("delete_char_before", "shift+x"),
        ("delete_to_line_end", "shift+d"),
        ("join_lines", "shift+j"),
        ("paste_after", "p"),
        ("paste_before", "shift+p"),
        // Vim's own undo/redo keys, plus the Ctrl pair every desktop editor
        // has: this is a modal editor, but it must not feel foreign.
        ("undo", "u"),
        ("undo", "ctrl+z"),
        ("redo", "ctrl+r"),
        ("redo", "ctrl+y"),
        ("indent", ">"),
        ("dedent", "<"),
        ("toggle_comment", "g c"),
        ("toggle_case", "~"),
        // Visual mode and multiple cursors.
        ("visual_char", "v"),
        ("visual_line", "shift+v"),
        ("visual_block", "ctrl+v"),
        ("select_all", "ctrl+shift+a"),
        ("select_next_occurrence", "ctrl+d"),
        ("select_all_occurrences", "ctrl+shift+l"),
        ("add_cursor_below", "ctrl+alt+down"),
        ("add_cursor_above", "ctrl+alt+up"),
        // Search.
        ("search_forward", "/"),
        ("search_backward", "?"),
        ("search_next", "n"),
        ("search_prev", "shift+n"),
        ("replace", "ctrl+h"),
        ("find_in_files", "ctrl+shift+f"),
        ("goto_line", "ctrl+g"),
        // Files and buffers.
        ("save", "ctrl+s"),
        ("close_buffer", "ctrl+w"),
        ("next_buffer", "ctrl+tab"),
        ("prev_buffer", "ctrl+shift+tab"),
        ("command_palette", "ctrl+shift+p"),
        ("toggle_tree", "ctrl+b"),
        ("split_right", "ctrl+shift+d"),
        ("split_down", "ctrl+shift+e"),
        ("close_split", "ctrl+shift+w"),
        ("next_split", "ctrl+shift+]"),
        ("prev_split", "ctrl+shift+["),
        ("copy", "ctrl+shift+c"),
        ("paste", "ctrl+shift+v"),
        ("enter_command_line", ":"),
        ("escape", "esc"),
        ("completion", "ctrl+space"),
        ("hover", "shift+k"),
        ("format_document", "ctrl+shift+i"),
        ("toggle_whitespace", "g w"),
        ("toggle_line_numbers", "g n"),
        ("toggle_wrap", "g r"),
        ("toggle_terminal", "ctrl+shift+t"),
        ("toggle_ssh_mode", "ctrl+shift+s"),
        ("keybindings", "ctrl+shift+k"),
        ("settings", "ctrl+shift+,"),
    ]
}

/// Sequences bound to more than one command: `(sequence, commands)`.
///
/// Sequences are compared in canonical form, so `shift+ctrl+p` and
/// `ctrl+shift+p` are the same sequence. Unparseable sequences are skipped.
pub(crate) fn conflicts(bindings: &[(String, String)]) -> Vec<(String, Vec<String>)> {
    let mut by_sequence: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (command, sequence) in bindings {
        let Ok(chords) = parse_sequence(sequence) else {
            continue;
        };
        by_sequence
            .entry(format_sequence(&chords))
            .or_default()
            .push(command.clone());
    }
    by_sequence
        .into_iter()
        .filter_map(|(sequence, mut commands)| {
            commands.sort();
            commands.dedup();
            (commands.len() > 1).then_some((sequence, commands))
        })
        .collect()
}

/// A modifier, in any of its accepted spellings.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Modifier {
    Ctrl,
    Shift,
    Alt,
}

impl Modifier {
    /// `ctrl|control|c`, `shift|s`, `alt|meta|m`, case-insensitively.
    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "c" => Some(Self::Ctrl),
            "shift" | "s" => Some(Self::Shift),
            "alt" | "meta" | "m" => Some(Self::Alt),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Ctrl => "ctrl",
            Self::Shift => "shift",
            Self::Alt => "alt",
        }
    }
}

/// Parse one chord: `modifier+modifier+key`.
fn parse_chord(token: &str) -> Result<KeyChord, String> {
    let mut parts: Vec<&str> = token.split('+').collect();
    // The last part is the key and the ones before it are modifiers. That is
    // what keeps `ctrl+c` the chord ctrl + the letter c rather than a
    // duplicate modifier.
    let key_part = parts.pop().unwrap_or_default();
    let mut ctrl = false;
    let mut shift = false;
    let mut alt = false;
    for part in parts {
        if part.is_empty() {
            return Err(format!("empty modifier in '{token}'"));
        }
        let Some(modifier) = Modifier::parse(part) else {
            return Err(format!("unknown modifier '{part}' in '{token}'"));
        };
        let slot = match modifier {
            Modifier::Ctrl => &mut ctrl,
            Modifier::Shift => &mut shift,
            Modifier::Alt => &mut alt,
        };
        if *slot {
            return Err(format!(
                "duplicate modifier '{}' in '{token}'",
                modifier.name()
            ));
        }
        *slot = true;
    }
    let key = match parse_key(key_part) {
        Ok(key) => key,
        Err(err) => {
            // A trailing modifier name that is already set is a duplicate
            // modifier, not an unknown key: `ctrl+ctrl` reports the duplicate.
            if let Some(modifier) = Modifier::parse(key_part) {
                let already = match modifier {
                    Modifier::Ctrl => ctrl,
                    Modifier::Shift => shift,
                    Modifier::Alt => alt,
                };
                if already {
                    return Err(format!(
                        "duplicate modifier '{}' in '{token}'",
                        modifier.name()
                    ));
                }
            }
            return Err(err);
        }
    };
    Ok(KeyChord {
        key,
        ctrl,
        shift,
        alt,
    })
}

/// Parse one key: a named key, a function key or a single letter/digit.
fn parse_key(s: &str) -> Result<ChordKey, String> {
    if s.is_empty() {
        return Err("missing key".to_string());
    }
    let lower = s.to_ascii_lowercase();
    let named = match lower.as_str() {
        "enter" => Some(ChordKey::Enter),
        "esc" | "escape" => Some(ChordKey::Esc),
        "tab" => Some(ChordKey::Tab),
        "space" => Some(ChordKey::Space),
        "backspace" => Some(ChordKey::Backspace),
        "delete" => Some(ChordKey::Delete),
        "up" => Some(ChordKey::Up),
        "down" => Some(ChordKey::Down),
        "left" => Some(ChordKey::Left),
        "right" => Some(ChordKey::Right),
        "home" => Some(ChordKey::Home),
        "end" => Some(ChordKey::End),
        "pageup" => Some(ChordKey::PageUp),
        "pagedown" => Some(ChordKey::PageDown),
        _ => None,
    };
    if let Some(key) = named {
        return Ok(key);
    }
    if let Some(n) = lower
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
    {
        return match n {
            1..=12 => Ok(ChordKey::F(n)),
            _ => Err(format!("unknown key '{s}': function keys are f1..f12")),
        };
    }
    let mut chars = s.chars();
    let c = chars.next().expect("checked non-empty");
    if chars.next().is_some() {
        return Err(format!("unknown key '{s}'"));
    }
    if c.is_ascii_alphabetic() {
        Ok(ChordKey::Char(c.to_ascii_lowercase()))
    } else if c.is_ascii_graphic() {
        // Digits and punctuation are keys in their own right: `$`, `:`, `%`.
        Ok(ChordKey::Char(c))
    } else {
        Err(format!("unknown key '{s}'"))
    }
}

/// The canonical string for one chord: `ctrl+shift+alt+key`.
fn format_chord(chord: &KeyChord) -> String {
    let mut out = String::new();
    if chord.ctrl {
        out.push_str("ctrl+");
    }
    if chord.shift {
        out.push_str("shift+");
    }
    if chord.alt {
        out.push_str("alt+");
    }
    out.push_str(&format_key(chord.key));
    out
}

/// The canonical name of one key.
fn format_key(key: ChordKey) -> String {
    match key {
        ChordKey::Char(c) => c.to_ascii_lowercase().to_string(),
        ChordKey::Enter => "enter".to_string(),
        ChordKey::Esc => "esc".to_string(),
        ChordKey::Tab => "tab".to_string(),
        ChordKey::Space => "space".to_string(),
        ChordKey::Backspace => "backspace".to_string(),
        ChordKey::Delete => "delete".to_string(),
        ChordKey::Up => "up".to_string(),
        ChordKey::Down => "down".to_string(),
        ChordKey::Left => "left".to_string(),
        ChordKey::Right => "right".to_string(),
        ChordKey::Home => "home".to_string(),
        ChordKey::End => "end".to_string(),
        ChordKey::PageUp => "pageup".to_string(),
        ChordKey::PageDown => "pagedown".to_string(),
        ChordKey::F(n) => format!("f{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One chord, parsed.
    fn chord(s: &str) -> KeyChord {
        let chords = parse_sequence(s).unwrap_or_else(|e| panic!("{s:?} should parse: {e}"));
        assert_eq!(chords.len(), 1, "expected one chord in {s:?}");
        chords[0]
    }

    /// A config map from `(command, sequence)` pairs.
    fn config(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(c, s)| (c.to_string(), s.to_string()))
            .collect()
    }

    /// A `(command, sequence)` list for `conflicts`.
    fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(c, s)| (c.to_string(), s.to_string()))
            .collect()
    }

    /// Every default command name.
    fn default_names() -> Vec<String> {
        defaults()
            .into_iter()
            .map(|(command, _)| command.to_string())
            .collect()
    }

    #[test]
    fn parse_and_format_round_trip() {
        let canonical = [
            "ctrl+s",
            "ctrl+shift+p",
            "ctrl+shift+alt+a",
            "alt+f4",
            "shift+tab",
            "g g",
            "g d",
            "d d",
            "y y",
            "c i w",
            "ctrl+space",
            "f1",
            "f12",
            "shift+f5",
            "up",
            "pagedown",
            "enter",
            "esc",
            "backspace",
            "delete",
            "home",
            "end",
            "left",
            "right",
            "0",
            "9",
            "ctrl+0",
            "space",
            "tab",
        ];
        for sequence in canonical {
            let chords = parse_sequence(sequence).unwrap_or_else(|e| panic!("{sequence:?}: {e}"));
            assert_eq!(
                format_sequence(&chords),
                sequence,
                "canonical form of {sequence:?}"
            );
            assert_eq!(parse_sequence(&format_sequence(&chords)).unwrap(), chords);
        }

        // Every default sequence is already canonical and round-trips.
        for (command, sequence) in defaults() {
            let chords = parse_sequence(sequence).unwrap_or_else(|e| panic!("{command}: {e}"));
            assert_eq!(format_sequence(&chords), sequence, "{command}");
        }

        // Modifier spellings are case-insensitive and normalise to the
        // canonical order ctrl+shift+alt+key.
        let canonicalised = [
            ("Shift+Ctrl+P", "ctrl+shift+p"),
            ("CONTROL+S", "ctrl+s"),
            ("meta+x", "alt+x"),
            ("M+X", "alt+x"),
            ("c+shift+alt+z", "ctrl+shift+alt+z"),
            ("G G", "g g"),
            ("  g   g  ", "g g"),
            ("PageUp", "pageup"),
            ("F12", "f12"),
        ];
        for (input, expected) in canonicalised {
            assert_eq!(
                format_sequence(&parse_sequence(input).unwrap()),
                expected,
                "{input:?}"
            );
        }
        assert_eq!(format_sequence(&[]), "");
    }

    #[test]
    fn rejects_unknown_keys_empty_input_and_duplicate_modifiers() {
        for bad in ["", "   ", "\t", "\n"] {
            assert!(parse_sequence(bad).is_err(), "{bad:?} should be rejected");
        }
        for bad in [
            "ctrl+nope",
            "f13",
            "f0",
            "ctrl+",
            "+p",
            "ctrl++p",
            "ctrl",
            "shift",
            "g g nope",
            "ctrl+shift+",
            "ctrl+é",
            "ctrl+ab",
        ] {
            let err = parse_sequence(bad).expect_err(&format!("{bad:?} should be rejected"));
            assert!(!err.is_empty(), "error for {bad:?} must be clear");
        }
        for bad in [
            "ctrl+ctrl+p",
            "ctrl+control+p",
            "c+ctrl+p",
            "shift+shift+p",
            "s+shift+p",
            "alt+alt+p",
            "m+meta+p",
            "ctrl+ctrl",
        ] {
            let err = parse_sequence(bad).expect_err(&format!("{bad:?} should be rejected"));
            assert!(
                err.contains("duplicate"),
                "error for {bad:?} should name the duplicate: {err}"
            );
        }

        // A trailing modifier alias is still a key: `ctrl+c` is ctrl + the
        // letter c, and a bare `c` is just the letter.
        assert_eq!(
            chord("ctrl+c"),
            KeyChord {
                key: ChordKey::Char('c'),
                ctrl: true,
                shift: false,
                alt: false
            }
        );
        assert_eq!(
            chord("c"),
            KeyChord {
                key: ChordKey::Char('c'),
                ctrl: false,
                shift: false,
                alt: false
            }
        );
        assert_eq!(
            chord("ctrl+shift+space"),
            KeyChord {
                key: ChordKey::Space,
                ctrl: true,
                shift: true,
                alt: false
            }
        );
    }

    #[test]
    fn defaults_are_conflict_free() {
        let defaults = defaults();
        let pairs: Vec<(String, String)> = defaults
            .iter()
            .map(|(c, s)| (c.to_string(), s.to_string()))
            .collect();
        assert!(conflicts(&pairs).is_empty(), "defaults must not conflict");

        let required = [
            ("save", "ctrl+s"),
            ("search_forward", "/"),
            ("replace", "ctrl+h"),
            ("goto_line", "ctrl+g"),
            ("command_palette", "ctrl+shift+p"),
            ("next_buffer", "ctrl+tab"),
            ("prev_buffer", "ctrl+shift+tab"),
            ("close_buffer", "ctrl+w"),
            ("split_right", "ctrl+shift+d"),
            ("split_down", "ctrl+shift+e"),
            ("toggle_tree", "ctrl+b"),
            ("undo", "u"),
            ("redo", "ctrl+r"),
            ("copy", "ctrl+shift+c"),
            ("paste", "ctrl+shift+v"),
            ("select_all", "ctrl+shift+a"),
            ("goto_first_line", "g g"),
            ("goto_definition", "g d"),
            ("delete_line", "d d"),
            ("yank_line", "y y"),
            ("toggle_comment", "g c"),
            // The Vim single-key bindings, without which the editor is unusable.
            ("move_left", "h"),
            ("move_down", "j"),
            ("move_up", "k"),
            ("move_right", "l"),
            ("move_word_next", "w"),
            ("insert_before", "i"),
            ("insert_after", "a"),
            ("open_below", "o"),
            ("delete", "d"),
            ("change", "c"),
            ("yank", "y"),
            ("visual_char", "v"),
            ("visual_line", "shift+v"),
            ("visual_block", "ctrl+v"),
            ("search_next", "n"),
            ("enter_command_line", ":"),
            ("escape", "esc"),
            ("paste_after", "p"),
            ("delete_char", "x"),
        ];
        for (command, sequence) in required {
            assert!(
                defaults.contains(&(command, sequence)),
                "missing default {command} = {sequence:?}"
            );
        }

        // A command may hold more than one sequence — `undo` has both `u` and
        // `ctrl+z` — but never the same pair twice.
        let mut pairs: Vec<(&str, &str)> = defaults.clone();
        let count = pairs.len();
        pairs.sort_unstable();
        pairs.dedup();
        assert_eq!(
            pairs.len(),
            count,
            "duplicate (command, sequence) in defaults"
        );

        // The keymap built from no config agrees.
        let keymap = Keymap::from_config(&BTreeMap::new());
        let bound: Vec<(String, String)> = keymap
            .bindings()
            .iter()
            .map(|binding| (binding.command.clone(), format_sequence(&binding.keys)))
            .collect();
        assert_eq!(bound.len(), defaults.len());
        assert!(conflicts(&bound).is_empty());
        assert!(keymap.unknown_commands().is_empty());
        assert!(keymap.invalid_entries().is_empty());
    }

    #[test]
    fn resolve_matches_exact_prefix_and_none() {
        let mut keymap = Keymap::from_config(&BTreeMap::new());

        // Empty pending is a prefix, so the first key of a sequence is not
        // swallowed.
        assert_eq!(keymap.resolve(&[]), Resolution::Prefix);
        assert_eq!(
            keymap.resolve(&[chord("ctrl+s")]),
            Resolution::Match("save".to_string())
        );
        assert_eq!(keymap.resolve(&[chord("g")]), Resolution::Prefix);
        assert_eq!(
            keymap.resolve(&[chord("g"), chord("g")]),
            Resolution::Match("goto_first_line".to_string())
        );
        assert_eq!(
            keymap.resolve(&[chord("g"), chord("d")]),
            Resolution::Match("goto_definition".to_string())
        );
        // `c` is an operator on its own, and an exact match wins over the
        // `c i w` sequence that also starts with it.
        assert_eq!(
            keymap.resolve(&[chord("c")]),
            Resolution::Match("change".to_string())
        );
        // `c` is an operator on its own; the text object that follows it is
        // handled by the input layer, not by the keymap.
        assert_eq!(
            keymap.resolve(&[chord("c")]),
            Resolution::Match("change".to_string())
        );
        assert_eq!(keymap.resolve(&[chord("c"), chord("i")]), Resolution::None);
        assert_eq!(keymap.resolve(&[chord("g"), chord("x")]), Resolution::None);
        assert_eq!(
            keymap.resolve(&[chord("g"), chord("g"), chord("g")]),
            Resolution::None
        );
        assert_eq!(keymap.resolve(&[chord("ctrl+q")]), Resolution::None);
        assert_eq!(
            keymap.resolve(&[chord("ctrl+shift+alt+z")]),
            Resolution::None
        );

        // An exact match wins over a prefix.
        keymap.set("custom", "g").unwrap();
        assert_eq!(
            keymap.resolve(&[chord("g")]),
            Resolution::Match("custom".to_string())
        );
        assert_eq!(
            keymap.resolve(&[chord("g"), chord("g")]),
            Resolution::Match("goto_first_line".to_string())
        );

        // With no bindings at all, nothing is a prefix.
        let mut empty = Keymap::from_config(&BTreeMap::new());
        for command in default_names() {
            empty.unbind(&command);
        }
        assert_eq!(empty.resolve(&[]), Resolution::None);
        assert_eq!(empty.resolve(&[chord("ctrl+s")]), Resolution::None);
    }

    #[test]
    fn config_overrides_defaults_and_reports_unknown_commands() {
        let keymap = Keymap::from_config(&config(&[
            ("save", "ctrl+alt+s"),
            ("goto_definition", "g f"),
            ("not_a_command", "ctrl+q"),
            ("search_forward", "ctrl+nope"),
        ]));

        // Overrides replace the default binding.
        assert_eq!(
            keymap.resolve(&[chord("ctrl+alt+s")]),
            Resolution::Match("save".to_string())
        );
        assert_eq!(keymap.resolve(&[chord("ctrl+s")]), Resolution::None);
        assert_eq!(
            keymap.resolve(&[chord("g"), chord("f")]),
            Resolution::Match("goto_definition".to_string())
        );
        assert_eq!(keymap.resolve(&[chord("g"), chord("d")]), Resolution::None);

        // Unknown commands and bad sequences are ignored but reported.
        assert_eq!(keymap.unknown_commands(), vec!["not_a_command".to_string()]);
        assert_eq!(keymap.invalid_entries(), vec!["search_forward".to_string()]);
        assert_eq!(keymap.resolve(&[chord("ctrl+q")]), Resolution::None);
        // The rejected override left the default binding in place.
        assert_eq!(
            keymap.resolve(&[chord("/")]),
            Resolution::Match("search_forward".to_string())
        );

        // to_config reflects the overrides and round-trips.
        let saved = keymap.to_config();
        assert_eq!(saved.get("save").map(String::as_str), Some("ctrl+alt+s"));
        assert_eq!(
            saved.get("goto_definition").map(String::as_str),
            Some("g f")
        );
        assert_eq!(saved.get("search_forward").map(String::as_str), Some("/"));
        let reloaded = Keymap::from_config(&saved);
        assert!(reloaded.unknown_commands().is_empty());
        assert!(reloaded.invalid_entries().is_empty());
        assert_eq!(reloaded.to_config(), saved);
        assert_eq!(
            reloaded.resolve(&[chord("ctrl+alt+s")]),
            Resolution::Match("save".to_string())
        );
    }

    #[test]
    fn conflicts_finds_duplicates() {
        let found = conflicts(&pairs(&[
            ("save", "ctrl+s"),
            ("search_forward", "ctrl+s"),
            ("undo", "ctrl+z"),
            ("redo", "shift+ctrl+z"),
            ("redo_alt", "ctrl+shift+z"),
            ("goto_first_line", "g g"),
            ("goto_top", "g g"),
            ("broken", "nope"),
        ]));
        assert_eq!(
            found,
            vec![
                (
                    "ctrl+s".to_string(),
                    vec!["save".to_string(), "search_forward".to_string()]
                ),
                (
                    "ctrl+shift+z".to_string(),
                    vec!["redo".to_string(), "redo_alt".to_string()]
                ),
                (
                    "g g".to_string(),
                    vec!["goto_first_line".to_string(), "goto_top".to_string()]
                ),
            ]
        );

        // The same command listed twice is not a conflict.
        assert!(conflicts(&pairs(&[("save", "ctrl+s"), ("save", "ctrl+s")])).is_empty());
        // Unparseable sequences are skipped.
        assert!(conflicts(&pairs(&[("a", "nope"), ("b", "also nope")])).is_empty());
        assert!(conflicts(&[]).is_empty());
    }

    #[test]
    fn set_unbind_and_reset_work() {
        let mut keymap = Keymap::from_config(&BTreeMap::new());

        // set replaces an existing binding.
        keymap.set("save", "ctrl+alt+s").unwrap();
        assert_eq!(
            keymap.resolve(&[chord("ctrl+alt+s")]),
            Resolution::Match("save".to_string())
        );
        assert_eq!(keymap.resolve(&[chord("ctrl+s")]), Resolution::None);

        // set adds a new command.
        keymap.set("custom", "ctrl+alt+c").unwrap();
        assert_eq!(
            keymap.resolve(&[chord("ctrl+alt+c")]),
            Resolution::Match("custom".to_string())
        );
        assert_eq!(
            keymap.to_config().get("custom").map(String::as_str),
            Some("ctrl+alt+c")
        );

        // set rejects a bad sequence or an empty command and changes nothing.
        assert!(keymap.set("save", "ctrl+nope").is_err());
        assert!(keymap.set("save", "").is_err());
        assert!(keymap.set("", "ctrl+s").is_err());
        assert_eq!(
            keymap.resolve(&[chord("ctrl+alt+s")]),
            Resolution::Match("save".to_string())
        );

        // unbind drops the binding; an unbound command is a no-op.
        keymap.unbind("save");
        assert_eq!(keymap.resolve(&[chord("ctrl+alt+s")]), Resolution::None);
        assert!(!keymap.to_config().contains_key("save"));
        keymap.unbind("never_bound");

        // reset returns to the defaults.
        keymap.reset();
        assert_eq!(
            keymap.resolve(&[chord("ctrl+s")]),
            Resolution::Match("save".to_string())
        );
        assert!(!keymap.to_config().contains_key("custom"));
        // `to_config` groups a command's sequences into one list entry.
        let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (command, sequence) in defaults() {
            grouped
                .entry(command.to_string())
                .or_default()
                .push(sequence.to_string());
        }
        let expected: BTreeMap<String, String> = grouped
            .into_iter()
            .map(|(command, sequences)| (command, sequences.join(LIST_SEP)))
            .collect();
        assert_eq!(keymap.to_config(), expected);
        assert!(keymap.unknown_commands().is_empty());
        assert!(keymap.invalid_entries().is_empty());

        // A fixed config entry stops being reported.
        let mut reported =
            Keymap::from_config(&config(&[("nope", "ctrl+q"), ("search_forward", "bad")]));
        assert_eq!(reported.unknown_commands(), vec!["nope".to_string()]);
        assert_eq!(
            reported.invalid_entries(),
            vec!["search_forward".to_string()]
        );
        reported.set("search_forward", "ctrl+f").unwrap();
        assert!(reported.invalid_entries().is_empty());
    }

    #[test]
    fn undo_and_redo_answer_to_both_the_vim_and_the_ctrl_keys() {
        let keymap = Keymap::from_config(&BTreeMap::new());
        for (sequence, command) in [
            ("u", "undo"),
            ("ctrl+z", "undo"),
            ("ctrl+r", "redo"),
            ("ctrl+y", "redo"),
            ("ctrl+s", "save"),
        ] {
            assert_eq!(
                keymap.resolve(&[chord(sequence)]),
                Resolution::Match(command.to_string()),
                "{sequence} should run {command}"
            );
        }
    }

    #[test]
    fn a_command_with_two_sequences_survives_the_config_round_trip() {
        let keymap = Keymap::from_config(&BTreeMap::new());
        let saved = keymap.to_config();
        assert_eq!(saved.get("undo").map(String::as_str), Some("u, ctrl+z"));
        assert_eq!(
            saved.get("redo").map(String::as_str),
            Some("ctrl+r, ctrl+y")
        );

        let reloaded = Keymap::from_config(&saved);
        assert_eq!(
            reloaded.resolve(&[chord("u")]),
            Resolution::Match("undo".to_string())
        );
        assert_eq!(
            reloaded.resolve(&[chord("ctrl+z")]),
            Resolution::Match("undo".to_string())
        );
        assert_eq!(reloaded.to_config(), saved);
    }

    #[test]
    fn a_comma_key_is_not_mistaken_for_the_list_separator() {
        // `ctrl+,` is a real binding (the settings default), so the list
        // separator has to be a comma *followed by a space*.
        let keymap = Keymap::from_config(&BTreeMap::new());
        let saved = keymap.to_config();
        assert_eq!(
            saved.get("settings").map(String::as_str),
            Some("ctrl+shift+,")
        );
        let reloaded = Keymap::from_config(&saved);
        assert_eq!(
            reloaded.resolve(&[chord("ctrl+shift+,")]),
            Resolution::Match("settings".to_string())
        );

        // A list whose first entry ends in a comma key still splits correctly.
        let listed = Keymap::from_config(&config(&[("undo", "ctrl+,, ctrl+z")]));
        assert_eq!(
            listed.resolve(&[chord("ctrl+,")]),
            Resolution::Match("undo".to_string())
        );
        assert_eq!(
            listed.resolve(&[chord("ctrl+z")]),
            Resolution::Match("undo".to_string())
        );
        assert_eq!(
            listed.resolve(&[chord("u")]),
            Resolution::None,
            "the override replaced the whole set"
        );
    }
}
