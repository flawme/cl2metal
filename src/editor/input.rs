//! Key and mouse input: chords in, commands out.
//!
//! The keymap decides what a key means in normal and visual modes; insert mode
//! sends text straight to the buffer and only consults the keymap for the keys
//! that are commands there (Esc, Ctrl+S, arrows). That split is what keeps
//! modal shortcuts from eating ordinary typing.

use super::Editor;
use super::command::Command;
use super::keymap::{ChordKey, KeyChord, Resolution};
use super::mode::{CommandKind, Mode};
use super::operator::TextObject;
use super::pos::{Pos, VisualKind};

/// The chord for an egui key, if the keymap can name it.
pub(crate) fn chord_key(key: egui::Key) -> Option<ChordKey> {
    use egui::Key as K;
    Some(match key {
        K::A => ChordKey::Char('a'),
        K::B => ChordKey::Char('b'),
        K::C => ChordKey::Char('c'),
        K::D => ChordKey::Char('d'),
        K::E => ChordKey::Char('e'),
        K::F => ChordKey::Char('f'),
        K::G => ChordKey::Char('g'),
        K::H => ChordKey::Char('h'),
        K::I => ChordKey::Char('i'),
        K::J => ChordKey::Char('j'),
        K::K => ChordKey::Char('k'),
        K::L => ChordKey::Char('l'),
        K::M => ChordKey::Char('m'),
        K::N => ChordKey::Char('n'),
        K::O => ChordKey::Char('o'),
        K::P => ChordKey::Char('p'),
        K::Q => ChordKey::Char('q'),
        K::R => ChordKey::Char('r'),
        K::S => ChordKey::Char('s'),
        K::T => ChordKey::Char('t'),
        K::U => ChordKey::Char('u'),
        K::V => ChordKey::Char('v'),
        K::W => ChordKey::Char('w'),
        K::X => ChordKey::Char('x'),
        K::Y => ChordKey::Char('y'),
        K::Z => ChordKey::Char('z'),
        K::Num0 => ChordKey::Char('0'),
        K::Num1 => ChordKey::Char('1'),
        K::Num2 => ChordKey::Char('2'),
        K::Num3 => ChordKey::Char('3'),
        K::Num4 => ChordKey::Char('4'),
        K::Num5 => ChordKey::Char('5'),
        K::Num6 => ChordKey::Char('6'),
        K::Num7 => ChordKey::Char('7'),
        K::Num8 => ChordKey::Char('8'),
        K::Num9 => ChordKey::Char('9'),
        K::Colon => ChordKey::Char(':'),
        K::Semicolon => ChordKey::Char(';'),
        K::Comma => ChordKey::Char(','),
        K::Period => ChordKey::Char('.'),
        K::Slash => ChordKey::Char('/'),
        K::Backslash | K::IntlBackslash => ChordKey::Char('\\'),
        K::Pipe => ChordKey::Char('|'),
        K::Questionmark => ChordKey::Char('?'),
        K::Exclamationmark => ChordKey::Char('!'),
        K::OpenBracket => ChordKey::Char('['),
        K::CloseBracket => ChordKey::Char(']'),
        K::OpenCurlyBracket => ChordKey::Char('{'),
        K::CloseCurlyBracket => ChordKey::Char('}'),
        K::Backtick => ChordKey::Char('`'),
        K::Minus => ChordKey::Char('-'),
        K::Plus => ChordKey::Char('+'),
        K::Equals => ChordKey::Char('='),
        K::Quote => ChordKey::Char('\''),
        K::Enter => ChordKey::Enter,
        K::Escape => ChordKey::Esc,
        K::Tab => ChordKey::Tab,
        K::Space => ChordKey::Space,
        K::Backspace => ChordKey::Backspace,
        K::Delete => ChordKey::Delete,
        K::ArrowUp => ChordKey::Up,
        K::ArrowDown => ChordKey::Down,
        K::ArrowLeft => ChordKey::Left,
        K::ArrowRight => ChordKey::Right,
        K::Home => ChordKey::Home,
        K::End => ChordKey::End,
        K::PageUp => ChordKey::PageUp,
        K::PageDown => ChordKey::PageDown,
        K::F1 => ChordKey::F(1),
        K::F2 => ChordKey::F(2),
        K::F3 => ChordKey::F(3),
        K::F4 => ChordKey::F(4),
        K::F5 => ChordKey::F(5),
        K::F6 => ChordKey::F(6),
        K::F7 => ChordKey::F(7),
        K::F8 => ChordKey::F(8),
        K::F9 => ChordKey::F(9),
        K::F10 => ChordKey::F(10),
        K::F11 => ChordKey::F(11),
        K::F12 => ChordKey::F(12),
        _ => return None,
    })
}

/// The symbol a shifted key produces on a US layout.
///
/// egui reports some shifted keys as their own variant (`:` for shift+`;`) and
/// others as the unshifted key plus the modifier (`$` is shift+`4`). Folding
/// both into one character is what lets the config file say `$` and mean it.
fn shifted_symbol(c: char) -> Option<char> {
    Some(match c {
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '.' => '>',
        ',' => '<',
        '-' => '_',
        '=' => '+',
        '\'' => '"',
        '`' => '~',
        '/' => '?',
        ';' => ':',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        _ => return None,
    })
}

/// The chord for a key press, with its modifiers.
pub(crate) fn chord(key: egui::Key, mods: egui::Modifiers) -> Option<KeyChord> {
    let mut key = chord_key(key)?;
    let mut shift = mods.shift;
    if let ChordKey::Char(c) = key {
        if shift && let Some(symbol) = shifted_symbol(c) {
            key = ChordKey::Char(symbol);
            shift = false;
        } else if !c.is_ascii_lowercase() {
            // The character already carries the shift, so the modifier would
            // make `:` and `shift+:` two different bindings.
            shift = false;
        }
    }
    Some(KeyChord {
        key,
        ctrl: mods.ctrl || mods.command,
        shift,
        alt: mods.alt,
    })
}

/// The canonical string for a key press, for the keybinding recorder.
pub(crate) fn chord_string(key: egui::Key, mods: egui::Modifiers) -> Option<String> {
    let chord = chord(key, mods)?;
    Some(super::keymap::format_sequence(&[chord]))
}

/// The text object a selector (`i` or `a`) and an object key name.
fn text_object_for(selector: char, key: char) -> Option<TextObject> {
    let inner = selector == 'i';
    Some(match key {
        'w' => {
            if inner {
                TextObject::InnerWord
            } else {
                TextObject::AWord
            }
        }
        'W' => {
            if inner {
                TextObject::InnerBigWord
            } else {
                TextObject::ABigWord
            }
        }
        '"' => {
            if inner {
                TextObject::InnerQuote('"')
            } else {
                TextObject::AQuote('"')
            }
        }
        '\'' => {
            if inner {
                TextObject::InnerQuote('\'')
            } else {
                TextObject::AQuote('\'')
            }
        }
        '`' => {
            if inner {
                TextObject::InnerQuote('`')
            } else {
                TextObject::AQuote('`')
            }
        }
        '(' | ')' | 'b' => {
            if inner {
                TextObject::InnerParen
            } else {
                TextObject::AParen
            }
        }
        '{' | '}' | 'B' => {
            if inner {
                TextObject::InnerBrace
            } else {
                TextObject::ABrace
            }
        }
        '[' | ']' => {
            if inner {
                TextObject::InnerBracket
            } else {
                TextObject::ABracket
            }
        }
        'p' => {
            if inner {
                TextObject::InnerParagraph
            } else {
                TextObject::AParagraph
            }
        }
        _ => return None,
    })
}

impl Editor {
    /// Feed a key press. Returns true when the editor consumed it.
    pub(crate) fn key(
        &mut self,
        key: egui::Key,
        mods: egui::Modifiers,
        text: Option<&str>,
    ) -> bool {
        if let Mode::Command(kind) = self.mode {
            return self.command_line_key(key, text, kind);
        }
        if self.mode.is_insert() {
            return self.insert_key(key, mods, text);
        }
        self.modal_key(key, mods)
    }

    /// Normal and visual modes: the keymap decides, with counts and text
    /// objects handled first.
    fn modal_key(&mut self, key: egui::Key, mods: egui::Modifiers) -> bool {
        let plain = !mods.ctrl && !mods.alt && !mods.command;
        // Digits build a count, except a bare `0` which is a motion.
        if plain
            && let Some(ChordKey::Char(c)) = chord_key(key)
            && c.is_ascii_digit()
            && !(c == '0' && self.pending.count.is_none())
        {
            self.pending.push_digit(c as usize - '0' as usize);
            return true;
        }
        // An operator is waiting: `i`/`a` select a text object.
        if self.pending.operator.is_some()
            && plain
            && let Some(ChordKey::Char(c)) = chord_key(key)
        {
            if self.pending.object.is_none() && (c == 'i' || c == 'a') {
                self.pending.object = Some(c);
                return true;
            }
            if let Some(selector) = self.pending.object
                && let Some(object) = text_object_for(selector, c)
            {
                let op = self
                    .pending
                    .operator
                    .unwrap_or(super::operator::Operator::Delete);
                let register = self.pending.register;
                self.pending.clear();
                self.operate_object(op, object, register);
                return true;
            }
        }
        let Some(chord) = chord(key, mods) else {
            return false;
        };
        self.pending_keys.push(chord);
        match self.keymap.resolve(&self.pending_keys) {
            Resolution::Match(name) => {
                self.pending_keys.clear();
                if let Some(cmd) = Command::from_name(&name) {
                    self.execute(cmd);
                }
                true
            }
            Resolution::Prefix => true,
            Resolution::None => {
                self.pending_keys.clear();
                self.pending.clear();
                false
            }
        }
    }

    /// Insert mode: text goes in, commands still work.
    ///
    /// Only *modified* keys are looked up in the keymap here. A plain letter is
    /// text while typing, even though it is a motion in normal mode — that is
    /// the whole point of modal editing.
    fn insert_key(&mut self, key: egui::Key, mods: egui::Modifiers, text: Option<&str>) -> bool {
        use egui::Key as K;
        let modified = mods.ctrl || mods.alt || mods.command;
        if modified && let Some(chord) = chord(key, mods) {
            self.pending_keys.push(chord);
            match self.keymap.resolve(&self.pending_keys) {
                Resolution::Match(name) => {
                    self.pending_keys.clear();
                    if let Some(cmd) = Command::from_name(&name)
                        && cmd.works_in_insert()
                    {
                        self.execute(cmd);
                        return true;
                    }
                }
                // A multi-key sequence has no meaning while typing.
                Resolution::Prefix | Resolution::None => self.pending_keys.clear(),
            }
        }
        match key {
            K::Escape => {
                self.execute(Command::Escape);
                true
            }
            K::Enter => {
                self.newline();
                true
            }
            K::Backspace => {
                self.backspace();
                true
            }
            K::Delete => {
                self.delete_forward();
                true
            }
            K::Tab => {
                self.insert_tab();
                true
            }
            K::ArrowLeft => {
                self.execute(Command::Move(super::motion::Motion::Left));
                true
            }
            K::ArrowRight => {
                self.execute(Command::Move(super::motion::Motion::Right));
                true
            }
            K::ArrowUp => {
                self.execute(Command::Move(super::motion::Motion::Up));
                true
            }
            K::ArrowDown => {
                self.execute(Command::Move(super::motion::Motion::Down));
                true
            }
            K::Home => {
                self.execute(Command::Move(super::motion::Motion::LineStart));
                true
            }
            K::End => {
                self.execute(Command::Move(super::motion::Motion::LineEnd));
                true
            }
            _ => {
                if let Some(text) = text
                    && !text.is_empty()
                    && !modified
                {
                    self.insert_text(text);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// The `:` or `/` line.
    fn command_line_key(&mut self, key: egui::Key, text: Option<&str>, kind: CommandKind) -> bool {
        use egui::Key as K;
        match key {
            K::Escape => {
                self.mode = Mode::Normal;
                self.command_line.clear();
                true
            }
            K::Enter => {
                let line = std::mem::take(&mut self.command_line);
                self.mode = Mode::Normal;
                match kind {
                    CommandKind::Ex => self.run_ex_command(&line),
                    CommandKind::SearchForward => self.run_search_line(&line, false),
                    CommandKind::SearchBackward => self.run_search_line(&line, true),
                }
                true
            }
            K::Backspace => {
                self.command_line.pop();
                true
            }
            _ => {
                if let Some(text) = text
                    && !text.is_empty()
                {
                    self.command_line.push_str(text);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// A text event: the characters a key press produced.
    ///
    /// egui sends both a key event and a text event for a typed character; the
    /// key event carries the command meaning and this carries the text, so a
    /// plain letter is only ever inserted once.
    pub(crate) fn text_input(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        match self.mode {
            Mode::Command(_) => {
                self.command_line.push_str(text);
                true
            }
            Mode::Insert | Mode::Replace => {
                self.insert_text(text);
                true
            }
            _ => false,
        }
    }

    /// Enter a newline, carrying the current indentation.
    pub(crate) fn newline(&mut self) {
        let indent = if self.settings.auto_indent {
            let buf = self.buffer();
            let line = buf.cursors.first().map_or(0, |c| c.pos.line);
            let width = buf.indent_width(line, self.settings.tab_width);
            " ".repeat(width)
        } else {
            String::new()
        };
        let text = format!("\n{indent}");
        self.insert_text(&text);
    }

    /// A tab: spaces to the next stop, or a real tab.
    pub(crate) fn insert_tab(&mut self) {
        if self.settings.expand_tabs {
            let buf = self.buffer();
            let col = buf.cursors.first().map_or(0, |c| c.pos.col);
            let width = self.settings.tab_width.max(1);
            let spaces = width - (col % width);
            self.insert_text(&" ".repeat(spaces));
        } else {
            self.insert_text("\t");
        }
    }

    /// Place the cursor from a click, extending the selection when dragging.
    pub(crate) fn click_at(&mut self, pos: Pos, extend: bool, kind: VisualKind) {
        let buf = self.buffer_mut();
        let target = buf.clamp(pos);
        if extend {
            if let Some(cursor) = buf.cursors.first_mut() {
                cursor.extend(target);
            }
            if !self.mode.is_visual() {
                self.mode = Mode::Visual(kind);
            }
        } else {
            buf.cursors = vec![super::pos::Cursor::new(target)];
            if self.mode.is_visual() {
                self.mode = Mode::Normal;
            }
        }
    }

    /// Select the word at a position (double click).
    pub(crate) fn select_word_at(&mut self, pos: Pos) {
        let buf = self.buffer_mut();
        let target = buf.clamp(pos);
        if let Some(span) = super::operator::text_object(buf, target, TextObject::InnerWord) {
            buf.cursors = vec![super::pos::Cursor {
                pos: span.end,
                anchor: Some(span.start),
                want_col: None,
            }];
            self.mode = Mode::Visual(VisualKind::Char);
        }
    }

    /// Select the line at a position (triple click).
    pub(crate) fn select_line_at(&mut self, pos: Pos) {
        let buf = self.buffer_mut();
        let line = buf.clamp(pos).line;
        let len = buf.line_len(line);
        buf.cursors = vec![super::pos::Cursor {
            pos: Pos::new(line, len),
            anchor: Some(Pos::new(line, 0)),
            want_col: None,
        }];
        self.mode = Mode::Visual(VisualKind::Line);
    }
}
