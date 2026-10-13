//! Registers: where yanks and deletes go.

use std::collections::HashMap;

/// Text held by a register.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct Register {
    pub(crate) lines: Vec<String>,
    /// True when the text was taken linewise (`dd`, `yy`), so paste puts it on
    /// lines of its own.
    pub(crate) linewise: bool,
}

impl Register {
    pub(crate) fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub(crate) fn from_text(text: &str, linewise: bool) -> Self {
        Self {
            lines: text.split('\n').map(str::to_owned).collect(),
            linewise,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lines.len() <= 1 && self.lines.first().is_none_or(String::is_empty)
    }
}

/// The register file: the unnamed register plus the named ones.
#[derive(Default)]
pub(crate) struct Registers {
    unnamed: Register,
    named: HashMap<char, Register>,
}

impl Registers {
    /// Store text. `name` is the register the user asked for, if any; the
    /// unnamed register always receives it too, as Vim does.
    pub(crate) fn set(&mut self, name: Option<char>, reg: Register) {
        if let Some(c) = name
            && c != '_'
        {
            self.named.insert(c, reg.clone());
        }
        self.unnamed = reg;
    }

    /// Read a register. An unset named register falls back to the unnamed one,
    /// which is friendlier than pasting nothing.
    pub(crate) fn get(&self, name: Option<char>) -> Option<&Register> {
        match name {
            Some('_') => None,
            Some(c) => self.named.get(&c).or(Some(&self.unnamed)),
            None => Some(&self.unnamed),
        }
    }

    pub(crate) fn unnamed(&self) -> &Register {
        &self.unnamed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_register_also_fills_the_unnamed_one() {
        let mut regs = Registers::default();
        regs.set(Some('a'), Register::from_text("hello", false));
        assert_eq!(regs.get(Some('a')).unwrap().text(), "hello");
        assert_eq!(regs.unnamed().text(), "hello");
    }

    #[test]
    fn an_unset_named_register_falls_back_to_unnamed() {
        let mut regs = Registers::default();
        regs.set(None, Register::from_text("last yank", false));
        assert_eq!(regs.get(Some('z')).unwrap().text(), "last yank");
    }

    #[test]
    fn the_black_hole_register_swallows_text() {
        let mut regs = Registers::default();
        regs.set(Some('_'), Register::from_text("gone", false));
        assert!(regs.get(Some('_')).is_none());
        // The unnamed register still took it, as Vim does.
        assert_eq!(regs.unnamed().text(), "gone");
    }

    #[test]
    fn linewise_text_keeps_its_lines() {
        let reg = Register::from_text("one\ntwo\n", true);
        assert!(reg.linewise);
        assert_eq!(reg.lines, vec!["one", "two", ""]);
        assert_eq!(reg.text(), "one\ntwo\n");
    }
}
