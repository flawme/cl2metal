//! Editing modes and the pending-command state.

use super::operator::Operator;
use super::pos::VisualKind;

/// The editor's modal state.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Mode {
    Normal,
    Insert,
    /// Overwrite as you type.
    Replace,
    Visual(VisualKind),
    /// Typing a `:` or `/` line.
    Command(CommandKind),
}

/// Which command line is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CommandKind {
    /// `:` — ex commands.
    Ex,
    /// `/` — search forward.
    SearchForward,
    /// `?` — search backward.
    SearchBackward,
}

impl Mode {
    pub(crate) fn is_visual(&self) -> bool {
        matches!(self, Mode::Visual(_))
    }

    pub(crate) fn is_insert(&self) -> bool {
        matches!(self, Mode::Insert | Mode::Replace)
    }

    pub(crate) fn is_command(&self) -> bool {
        matches!(self, Mode::Command(_))
    }

    /// Whether a key press in this mode is text rather than a command.
    ///
    /// egui sends a Text event after the Key event for the same press, so the
    /// application needs to know whether the key it just handled was a command:
    /// a command must not also insert its own character.
    pub(crate) fn takes_text(&self) -> bool {
        matches!(self, Mode::Insert | Mode::Replace | Mode::Command(_))
    }

    /// The short label shown in the status line.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Replace => "REPLACE",
            Mode::Visual(VisualKind::Char) => "VISUAL",
            Mode::Visual(VisualKind::Line) => "V-LINE",
            Mode::Visual(VisualKind::Block) => "V-BLOCK",
            Mode::Command(CommandKind::Ex) => "COMMAND",
            Mode::Command(CommandKind::SearchForward) => "SEARCH",
            Mode::Command(CommandKind::SearchBackward) => "SEARCH",
        }
    }
}

/// A count, operator, register or prefix waiting for the rest of a command.
///
/// `2d3w` is a count, an operator, another count and a motion; `diw` is an
/// operator, an object selector and an object. Both live here.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct Pending {
    pub(crate) count: Option<usize>,
    pub(crate) operator: Option<Operator>,
    pub(crate) register: Option<char>,
    /// `g`, `f`, `t`, `"`, `z` … waiting for its second key.
    pub(crate) prefix: Option<char>,
    /// `i` or `a` waiting for the text object (`diw`).
    pub(crate) object: Option<char>,
}

impl Pending {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.count.is_none()
            && self.operator.is_none()
            && self.register.is_none()
            && self.prefix.is_none()
            && self.object.is_none()
    }

    /// The count, defaulting to 1.
    pub(crate) fn count(&self) -> usize {
        self.count.unwrap_or(1)
    }

    /// Fold a digit into the count (`2`, then `0` makes 20).
    pub(crate) fn push_digit(&mut self, d: usize) {
        self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(d));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_accumulate_digits() {
        let mut pending = Pending::default();
        assert_eq!(pending.count(), 1);
        pending.push_digit(2);
        pending.push_digit(0);
        assert_eq!(pending.count(), 20);
    }

    #[test]
    fn mode_labels_cover_every_mode() {
        assert_eq!(Mode::Normal.label(), "NORMAL");
        assert_eq!(Mode::Visual(VisualKind::Block).label(), "V-BLOCK");
        assert!(Mode::Insert.is_insert());
        assert!(Mode::Visual(VisualKind::Char).is_visual());
        assert!(!Mode::Normal.is_insert());
    }

    #[test]
    fn clearing_resets_every_field() {
        let mut pending = Pending {
            count: Some(3),
            operator: Some(Operator::Delete),
            register: Some('a'),
            prefix: Some('g'),
            object: Some('i'),
        };
        assert!(!pending.is_empty());
        pending.clear();
        assert!(pending.is_empty());
        assert_eq!(pending.count(), 1);
    }
}
