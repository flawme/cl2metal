//! Keyboard/mouse encoding: egui events → xterm-compatible byte sequences.

use alacritty_terminal::term::TermMode;
use egui::{Event, Key, Modifiers, PointerButton};

/// Result of processing one input event for the terminal.
pub enum Outcome {
    /// Bytes to write to the session.
    Bytes(Vec<u8>),
    /// Copy the current selection to the clipboard.
    Copy,
    /// Paste the given text (already read from the clipboard).
    Paste(String),
    /// Nothing for the terminal.
    None,
}

/// Handle a single egui event when the terminal is focused.
///
/// `mods` is the current modifier state (kept up to date by the caller via
/// `ModifiersChanged` events) and is used for events that don't carry modifiers.
pub fn handle_event(event: &Event, mods: Modifiers, mode: TermMode) -> Outcome {
    match event {
        Event::Text(text) => {
            if mods.ctrl || mods.command {
                return Outcome::None; // handled via Key events
            }
            let mut bytes = text.as_bytes().to_vec();
            if mods.alt {
                bytes.insert(0, 0x1b);
            }
            Outcome::Bytes(bytes)
        }
        Event::Paste(text) => Outcome::Paste(text.clone()),
        // IME composition result (CJK, dead keys, compose key).
        Event::Ime(egui::ImeEvent::Commit(text)) => Outcome::Bytes(text.clone().into_bytes()),
        Event::Ime(_) => Outcome::None,
        // egui-winit maps Ctrl+C / Ctrl+X / Ctrl+V to these on all platforms.
        Event::Copy => {
            if mods.shift {
                Outcome::Copy
            } else {
                Outcome::Bytes(vec![0x03])
            }
        }
        Event::Cut => Outcome::Bytes(vec![0x18]),
        Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => match encode_key(key, *modifiers, mode) {
            Some(v) if !v.is_empty() => Outcome::Bytes(v),
            _ => Outcome::None,
        },
        _ => Outcome::None,
    }
}

/// Encode a pressed key (including repeats) into terminal bytes.
pub fn encode_key(key: &Key, mods: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    let ctrl = mods.ctrl;
    let alt = mods.alt;
    let shift = mods.shift;
    // xterm modifier parameter: 1 + shift·1 + alt·2 + ctrl·4.
    let m = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let app_cursor = mode.contains(TermMode::APP_CURSOR);

    // Helper: CSI with optional modifier suffix.
    let csi_mod = |final_byte: u8, base: &[u8]| -> Vec<u8> {
        if m > 1 {
            let mut v = b"\x1b[1;".to_vec();
            v.push(b'0' + m);
            v.push(final_byte);
            v
        } else {
            base.to_vec()
        }
    };
    let tilde_mod = |num: u16| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[{num};{m}~").into_bytes()
        } else {
            format!("\x1b[{num}~").into_bytes()
        }
    };

    // Alt-only printable keys are handled by the Text path (ESC + char).
    let alt_printable = alt && !ctrl;

    // Kitty keyboard protocol: keys that are ambiguous in the legacy encoding
    // (modified Enter/Tab/Backspace/Escape) go out as CSI-u.
    if mode.contains(TermMode::KITTY_KEYBOARD_PROTOCOL) && m > 1 {
        let code = match key {
            Key::Enter => Some(13),
            Key::Tab => Some(9),
            Key::Backspace => Some(127),
            Key::Escape => Some(27),
            _ => None,
        };
        if let Some(code) = code {
            return Some(format!("\x1b[{code};{m}u").into_bytes());
        }
    }

    let seq: Option<Vec<u8>> = match key {
        Key::ArrowUp => Some(if app_cursor && m == 1 {
            b"\x1bOA".to_vec()
        } else {
            csi_mod(b'A', b"\x1b[A")
        }),
        Key::ArrowDown => Some(if app_cursor && m == 1 {
            b"\x1bOB".to_vec()
        } else {
            csi_mod(b'B', b"\x1b[B")
        }),
        Key::ArrowRight => Some(if app_cursor && m == 1 {
            b"\x1bOC".to_vec()
        } else {
            csi_mod(b'C', b"\x1b[C")
        }),
        Key::ArrowLeft => Some(if app_cursor && m == 1 {
            b"\x1bOD".to_vec()
        } else {
            csi_mod(b'D', b"\x1b[D")
        }),
        Key::Home => Some(if app_cursor && m == 1 {
            b"\x1bOH".to_vec()
        } else {
            csi_mod(b'H', b"\x1b[H")
        }),
        Key::End => Some(if app_cursor && m == 1 {
            b"\x1bOF".to_vec()
        } else {
            csi_mod(b'F', b"\x1b[F")
        }),
        Key::PageUp => Some(tilde_mod(5)),
        Key::PageDown => Some(tilde_mod(6)),
        Key::Insert => Some(tilde_mod(2)),
        Key::Delete => Some(tilde_mod(3)),
        Key::Tab if shift && !ctrl && !alt => Some(b"\x1b[Z".to_vec()),
        Key::Tab if ctrl || alt => None, // Ctrl+Tab cycles tabs (app-level)
        Key::Tab => Some(b"\t".to_vec()),
        Key::Enter => Some(if alt {
            b"\x1b\r".to_vec()
        } else {
            b"\r".to_vec()
        }),
        Key::Escape => Some(b"\x1b".to_vec()),
        Key::Backspace => Some(if ctrl {
            vec![0x08]
        } else if alt {
            b"\x1b\x7f".to_vec()
        } else {
            vec![0x7f]
        }),
        Key::Space if ctrl && alt => Some(vec![0x1b, 0x00]),
        Key::Space if ctrl => Some(vec![0x00]),
        Key::Space => None, // plain space arrives as Text

        // Function keys (xterm defaults).
        Key::F1 => Some(if m > 1 {
            csi_mod(b'P', b"")
        } else {
            b"\x1bOP".to_vec()
        }),
        Key::F2 => Some(if m > 1 {
            csi_mod(b'Q', b"")
        } else {
            b"\x1bOQ".to_vec()
        }),
        Key::F3 => Some(if m > 1 {
            csi_mod(b'R', b"")
        } else {
            b"\x1bOR".to_vec()
        }),
        Key::F4 => Some(if m > 1 {
            csi_mod(b'S', b"")
        } else {
            b"\x1bOS".to_vec()
        }),
        Key::F5 => Some(tilde_mod(15)),
        Key::F6 => Some(tilde_mod(17)),
        Key::F7 => Some(tilde_mod(18)),
        Key::F8 => Some(tilde_mod(19)),
        Key::F9 => Some(tilde_mod(20)),
        Key::F10 => Some(tilde_mod(21)),
        Key::F11 => Some(tilde_mod(23)),
        Key::F12 => Some(tilde_mod(24)),
        Key::F13 => Some(tilde_mod(25)),
        Key::F14 => Some(tilde_mod(26)),
        Key::F15 => Some(tilde_mod(28)),
        Key::F16 => Some(tilde_mod(29)),
        Key::F17 => Some(tilde_mod(31)),
        Key::F18 => Some(tilde_mod(32)),
        Key::F19 => Some(tilde_mod(33)),
        Key::F20 => Some(tilde_mod(34)),

        // Let egui handle UI zoom.
        Key::Minus | Key::Equals | Key::Num0 | Key::Plus if ctrl && !alt => None,

        // Ctrl-modified printable keys (Text events are suppressed with ctrl).
        Key::A
        | Key::B
        | Key::C
        | Key::D
        | Key::E
        | Key::F
        | Key::G
        | Key::H
        | Key::I
        | Key::J
        | Key::K
        | Key::L
        | Key::M
        | Key::N
        | Key::O
        | Key::P
        | Key::Q
        | Key::R
        | Key::S
        | Key::T
        | Key::U
        | Key::V
        | Key::W
        | Key::X
        | Key::Y
        | Key::Z
            if ctrl =>
        {
            let byte = b'a' + (*key as u8 - Key::A as u8);
            let mut v = vec![byte - b'a' + 1];
            if alt {
                v.insert(0, 0x1b);
            }
            Some(v)
        }
        Key::Num2 | Key::Num0 if ctrl => Some(vec![0x00]),
        Key::Num3 if ctrl => Some(vec![0x1b]),
        Key::Num4 if ctrl => Some(vec![0x1c]),
        Key::Num5 if ctrl => Some(vec![0x1d]),
        Key::Num6 | Key::Equals if ctrl => Some(vec![0x1e]),
        Key::Num7 if ctrl => Some(vec![0x1f]),
        Key::Num8 if ctrl => Some(vec![0x7f]),
        Key::Slash | Key::Questionmark if ctrl => Some(vec![0x1f]),
        Key::OpenBracket if ctrl => Some(vec![0x1b]),
        Key::Backslash if ctrl => Some(vec![0x1c]),
        Key::CloseBracket if ctrl => Some(vec![0x1d]),

        // Alt + digit / punctuation that produces no Text with ctrl… (plain alt → Text path).
        Key::Minus
        | Key::Comma
        | Key::Period
        | Key::Slash
        | Key::Semicolon
        | Key::Quote
        | Key::Backtick
        | Key::OpenBracket
        | Key::CloseBracket
        | Key::Colon
        | Key::Pipe
        | Key::Questionmark
        | Key::Exclamationmark
        | Key::OpenCurlyBracket
        | Key::CloseCurlyBracket
        | Key::Plus
        | Key::Equals
            if alt_printable =>
        {
            None
        }

        _ => None,
    };

    seq
}

/// Wrap text for paste, honoring bracketed paste mode.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\n").replace('\n', "\r");
    if bracketed {
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(text.as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        text.into_bytes()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum MouseBtn {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

impl MouseBtn {
    fn code(self) -> u8 {
        match self {
            MouseBtn::Left => 0,
            MouseBtn::Middle => 1,
            MouseBtn::Right => 2,
            MouseBtn::WheelUp => 64,
            MouseBtn::WheelDown => 65,
        }
    }

    pub fn from_egui(btn: PointerButton) -> Option<Self> {
        match btn {
            PointerButton::Primary => Some(MouseBtn::Left),
            PointerButton::Middle => Some(MouseBtn::Middle),
            PointerButton::Secondary => Some(MouseBtn::Right),
            _ => None,
        }
    }
}

/// Encode a mouse event for an application that requested mouse reporting.
///
/// `col`/`row` are 0-based cell coordinates.
pub fn encode_mouse(
    btn: MouseBtn,
    pressed: bool,
    col: u16,
    row: u16,
    mods: Modifiers,
    mode: TermMode,
) -> Vec<u8> {
    let mut b = btn.code();
    if mods.shift {
        b |= 4;
    }
    if mods.alt {
        b |= 8;
    }
    if mods.ctrl {
        b |= 16;
    }
    let x = col as u32 + 1;
    let y = row as u32 + 1;
    if mode.contains(TermMode::SGR_MOUSE) {
        format!("\x1b[<{b};{x};{y}{}", if pressed { 'M' } else { 'm' }).into_bytes()
    } else {
        // Legacy X10 encoding.
        let mut v = b"\x1b[M".to_vec();
        v.push(32 + b);
        v.push(32 + 1 + x.min(223) as u8 - 1);
        v.push(32 + 1 + y.min(223) as u8 - 1);
        v
    }
}

/// Mouse motion (drag) with button already held: `btn` is the held button.
pub fn encode_mouse_motion(
    btn: MouseBtn,
    col: u16,
    row: u16,
    mods: Modifiers,
    mode: TermMode,
) -> Vec<u8> {
    let mut b = btn.code() | 32; // motion bit
    if mods.shift {
        b |= 4;
    }
    if mods.alt {
        b |= 8;
    }
    if mods.ctrl {
        b |= 16;
    }
    let x = col as u32 + 1;
    let y = row as u32 + 1;
    if mode.contains(TermMode::SGR_MOUSE) {
        format!("\x1b[<{b};{x};{y}M").into_bytes()
    } else {
        let mut v = b"\x1b[M".to_vec();
        v.push(32 + b);
        v.push(32 + x.min(223) as u8);
        v.push(32 + y.min(223) as u8);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, shift: bool, alt: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            alt,
            ..Default::default()
        }
    }

    #[test]
    fn plain_keys_use_legacy_encoding() {
        let none = Modifiers::default();
        assert_eq!(
            encode_key(&Key::Enter, none, TermMode::empty()),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            encode_key(&Key::Tab, none, TermMode::empty()),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            encode_key(&Key::Escape, none, TermMode::empty()),
            Some(b"\x1b".to_vec())
        );
        assert_eq!(
            encode_key(&Key::Backspace, none, TermMode::empty()),
            Some(vec![0x7f])
        );
        assert_eq!(
            encode_key(&Key::ArrowUp, none, TermMode::empty()),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(&Key::ArrowUp, none, TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn ctrl_letters_send_control_bytes() {
        let ctrl = mods(true, false, false);
        assert_eq!(
            encode_key(&Key::C, ctrl, TermMode::empty()),
            Some(vec![0x03])
        );
        assert_eq!(
            encode_key(&Key::D, ctrl, TermMode::empty()),
            Some(vec![0x04])
        );
        assert_eq!(
            encode_key(&Key::A, ctrl, TermMode::empty()),
            Some(vec![0x01])
        );
    }

    #[test]
    fn modified_keys_carry_the_xterm_modifier_parameter() {
        let ctrl_shift = mods(true, true, false);
        assert_eq!(
            encode_key(&Key::ArrowRight, ctrl_shift, TermMode::empty()),
            Some(b"\x1b[1;6C".to_vec())
        );
        assert_eq!(
            encode_key(&Key::PageUp, mods(false, true, false), TermMode::empty()),
            Some(b"\x1b[5;2~".to_vec())
        );
    }

    #[test]
    fn kitty_protocol_encodes_ambiguous_modified_keys() {
        let kitty = TermMode::KITTY_KEYBOARD_PROTOCOL;
        let shift = mods(false, true, false);
        assert_eq!(
            encode_key(&Key::Enter, shift, kitty),
            Some(b"\x1b[13;2u".to_vec())
        );
        assert_eq!(
            encode_key(&Key::Enter, mods(true, false, false), kitty),
            Some(b"\x1b[13;5u".to_vec())
        );
        assert_eq!(
            encode_key(&Key::Tab, shift, kitty),
            Some(b"\x1b[9;2u".to_vec())
        );
        // Unmodified keys keep the legacy form.
        assert_eq!(
            encode_key(&Key::Enter, Modifiers::default(), kitty),
            Some(b"\r".to_vec())
        );
        // Without the protocol, the legacy form is used too.
        assert_eq!(
            encode_key(&Key::Enter, shift, TermMode::empty()),
            Some(b"\r".to_vec())
        );
    }

    #[test]
    fn paste_honours_bracketed_mode_and_normalises_newlines() {
        assert_eq!(encode_paste("a\nb", false), b"a\rb".to_vec());
        assert_eq!(
            encode_paste("a\nb", true),
            b"\x1b[200~a\rb\x1b[201~".to_vec()
        );
        assert_eq!(encode_paste("x\r\ny", false), b"x\ry".to_vec());
    }

    #[test]
    fn mouse_encoding_sgr_and_legacy() {
        let none = Modifiers::default();
        let sgr = encode_mouse(MouseBtn::Left, true, 0, 0, none, TermMode::SGR_MOUSE);
        assert_eq!(sgr, b"\x1b[<0;1;1M".to_vec());
        let release = encode_mouse(MouseBtn::Left, false, 4, 9, none, TermMode::SGR_MOUSE);
        assert_eq!(release, b"\x1b[<0;5;10m".to_vec());
        let legacy = encode_mouse(MouseBtn::Left, true, 0, 0, none, TermMode::empty());
        assert_eq!(legacy, vec![0x1b, b'[', b'M', 32, 33, 33]);
        let wheel = encode_mouse(MouseBtn::WheelUp, true, 1, 1, none, TermMode::SGR_MOUSE);
        assert_eq!(wheel, b"\x1b[<64;2;2M".to_vec());
    }
}
