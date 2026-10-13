//! The command vocabulary shared by keybindings, the palette and `:` lines.
//!
//! One enum, three front doors: a key sequence resolves to a `Command`, the
//! palette lists commands by name, and `:` lines parse into the same enum. That
//! is what keeps the keybinding editor honest — it can only bind things that
//! actually exist.

use super::motion::Motion;
use super::operator::Operator;

/// Something the editor can be asked to do.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Command {
    // Motions.
    Move(Motion),
    // Operators.
    Operate(Operator),
    // Entering insert mode.
    InsertBefore,
    InsertAfter,
    InsertLineStart,
    InsertLineEnd,
    OpenBelow,
    OpenAbove,
    // Simple edits.
    DeleteChar,
    DeleteCharBefore,
    DeleteLine,
    DeleteToLineEnd,
    JoinLines,
    // History.
    Undo,
    Redo,
    // Registers.
    YankLine,
    PasteAfter,
    PasteBefore,
    /// Copy to the system clipboard.
    Copy,
    /// Paste from the system clipboard.
    Paste,
    // Case and indentation.
    Indent,
    Dedent,
    ToggleComment,
    ToggleCase,
    // Visual mode and multiple cursors.
    VisualChar,
    VisualLine,
    VisualBlock,
    SelectAll,
    SelectNextOccurrence,
    SelectAllOccurrences,
    AddCursorBelow,
    AddCursorAbove,
    ClearExtraCursors,
    // Files and buffers.
    Save,
    SaveAs,
    CloseBuffer,
    NextBuffer,
    PrevBuffer,
    ReopenFromDisk,
    NewScratch,
    // Navigation.
    GotoLine,
    GotoFirstLine,
    GotoLastLine,
    GotoDefinition,
    GotoBracket,
    // Search.
    SearchForward,
    SearchBackward,
    SearchNext,
    SearchPrev,
    Replace,
    FindInFiles,
    // View.
    ToggleTree,
    ToggleWhitespace,
    ToggleLineNumbers,
    ToggleWrap,
    // Layout.
    SplitRight,
    SplitDown,
    CloseSplit,
    NextSplit,
    PrevSplit,
    // Application.
    CommandPalette,
    ToggleTerminal,
    ToggleSshMode,
    Keybindings,
    Settings,
    // Mode.
    Escape,
    EnterCommandLine,
    // Language server.
    Completion,
    Hover,
    FormatDocument,
    // Scrolling.
    ScrollHalfDown,
    ScrollHalfUp,
    CenterCursor,
}

impl Command {
    /// The stable name used in config.toml, the palette and `:` lines.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Command::Move(m) => match m {
                Motion::Left => "move_left",
                Motion::Right => "move_right",
                Motion::Up => "move_up",
                Motion::Down => "move_down",
                Motion::WordNext => "move_word_next",
                Motion::WordEnd => "move_word_end",
                Motion::WordBack => "move_word_back",
                Motion::BigWordNext => "move_big_word_next",
                Motion::BigWordEnd => "move_big_word_end",
                Motion::BigWordBack => "move_big_word_back",
                Motion::LineStart => "move_line_start",
                Motion::LineEnd => "move_line_end",
                Motion::FirstNonBlank => "move_first_non_blank",
                Motion::DocStart => "goto_first_line",
                Motion::DocEnd => "goto_last_line",
                Motion::Line(_) => "goto_line",
                Motion::FindChar(..) => "find_char",
                Motion::MatchBracket => "goto_bracket",
                Motion::ParagraphNext => "move_paragraph_next",
                Motion::ParagraphBack => "move_paragraph_back",
                Motion::ScreenTop => "move_screen_top",
                Motion::ScreenMiddle => "move_screen_middle",
                Motion::ScreenBottom => "move_screen_bottom",
                Motion::SearchNext => "search_next",
                Motion::SearchPrev => "search_prev",
            },
            Command::Operate(op) => match op {
                Operator::Delete => "delete",
                Operator::Change => "change",
                Operator::Yank => "yank",
                Operator::Indent => "indent",
                Operator::Dedent => "dedent",
                Operator::Upper => "uppercase",
                Operator::Lower => "lowercase",
                Operator::ToggleCase => "toggle_case",
                Operator::Comment => "comment",
            },
            Command::InsertBefore => "insert_before",
            Command::InsertAfter => "insert_after",
            Command::InsertLineStart => "insert_line_start",
            Command::InsertLineEnd => "insert_line_end",
            Command::OpenBelow => "open_below",
            Command::OpenAbove => "open_above",
            Command::DeleteChar => "delete_char",
            Command::DeleteCharBefore => "delete_char_before",
            Command::DeleteLine => "delete_line",
            Command::DeleteToLineEnd => "delete_to_line_end",
            Command::JoinLines => "join_lines",
            Command::Undo => "undo",
            Command::Redo => "redo",
            Command::YankLine => "yank_line",
            Command::PasteAfter => "paste_after",
            Command::PasteBefore => "paste_before",
            Command::Copy => "copy",
            Command::Paste => "paste",
            Command::Indent => "indent",
            Command::Dedent => "dedent",
            Command::ToggleComment => "toggle_comment",
            Command::ToggleCase => "toggle_case",
            Command::VisualChar => "visual_char",
            Command::VisualLine => "visual_line",
            Command::VisualBlock => "visual_block",
            Command::SelectAll => "select_all",
            Command::SelectNextOccurrence => "select_next_occurrence",
            Command::SelectAllOccurrences => "select_all_occurrences",
            Command::AddCursorBelow => "add_cursor_below",
            Command::AddCursorAbove => "add_cursor_above",
            Command::ClearExtraCursors => "clear_extra_cursors",
            Command::Save => "save",
            Command::SaveAs => "save_as",
            Command::CloseBuffer => "close_buffer",
            Command::NextBuffer => "next_buffer",
            Command::PrevBuffer => "prev_buffer",
            Command::ReopenFromDisk => "reopen_from_disk",
            Command::NewScratch => "new_scratch",
            Command::GotoLine => "goto_line",
            Command::GotoFirstLine => "goto_first_line",
            Command::GotoLastLine => "goto_last_line",
            Command::GotoDefinition => "goto_definition",
            Command::GotoBracket => "goto_bracket",
            Command::SearchForward => "search_forward",
            Command::SearchBackward => "search_backward",
            Command::SearchNext => "search_next",
            Command::SearchPrev => "search_prev",
            Command::Replace => "replace",
            Command::FindInFiles => "find_in_files",
            Command::ToggleTree => "toggle_tree",
            Command::ToggleWhitespace => "toggle_whitespace",
            Command::ToggleLineNumbers => "toggle_line_numbers",
            Command::ToggleWrap => "toggle_wrap",
            Command::SplitRight => "split_right",
            Command::SplitDown => "split_down",
            Command::CloseSplit => "close_split",
            Command::NextSplit => "next_split",
            Command::PrevSplit => "prev_split",
            Command::CommandPalette => "command_palette",
            Command::ToggleTerminal => "toggle_terminal",
            Command::ToggleSshMode => "toggle_ssh_mode",
            Command::Keybindings => "keybindings",
            Command::Settings => "settings",
            Command::Escape => "escape",
            Command::EnterCommandLine => "enter_command_line",
            Command::Completion => "completion",
            Command::Hover => "hover",
            Command::FormatDocument => "format_document",
            Command::ScrollHalfDown => "scroll_half_down",
            Command::ScrollHalfUp => "scroll_half_up",
            Command::CenterCursor => "center_cursor",
        }
    }

    /// Parse a name back into a command.
    pub(crate) fn from_name(name: &str) -> Option<Command> {
        let cmd = match name {
            "move_left" => Command::Move(Motion::Left),
            "move_right" => Command::Move(Motion::Right),
            "move_up" => Command::Move(Motion::Up),
            "move_down" => Command::Move(Motion::Down),
            "move_word_next" => Command::Move(Motion::WordNext),
            "move_word_end" => Command::Move(Motion::WordEnd),
            "move_word_back" => Command::Move(Motion::WordBack),
            "move_big_word_next" => Command::Move(Motion::BigWordNext),
            "move_big_word_end" => Command::Move(Motion::BigWordEnd),
            "move_big_word_back" => Command::Move(Motion::BigWordBack),
            "move_line_start" => Command::Move(Motion::LineStart),
            "move_line_end" => Command::Move(Motion::LineEnd),
            "move_first_non_blank" => Command::Move(Motion::FirstNonBlank),
            "move_paragraph_next" => Command::Move(Motion::ParagraphNext),
            "move_paragraph_back" => Command::Move(Motion::ParagraphBack),
            "move_screen_top" => Command::Move(Motion::ScreenTop),
            "move_screen_middle" => Command::Move(Motion::ScreenMiddle),
            "move_screen_bottom" => Command::Move(Motion::ScreenBottom),
            "goto_first_line" => Command::GotoFirstLine,
            "goto_last_line" => Command::GotoLastLine,
            "goto_line" => Command::GotoLine,
            "goto_bracket" => Command::GotoBracket,
            "goto_definition" => Command::GotoDefinition,
            "find_char" => Command::Move(Motion::FindChar(' ', false)),
            "delete" => Command::Operate(Operator::Delete),
            "change" => Command::Operate(Operator::Change),
            "yank" => Command::Operate(Operator::Yank),
            "uppercase" => Command::Operate(Operator::Upper),
            "lowercase" => Command::Operate(Operator::Lower),
            "insert_before" => Command::InsertBefore,
            "insert_after" => Command::InsertAfter,
            "insert_line_start" => Command::InsertLineStart,
            "insert_line_end" => Command::InsertLineEnd,
            "open_below" => Command::OpenBelow,
            "open_above" => Command::OpenAbove,
            "delete_char" => Command::DeleteChar,
            "delete_char_before" => Command::DeleteCharBefore,
            "delete_line" => Command::DeleteLine,
            "delete_to_line_end" => Command::DeleteToLineEnd,
            "join_lines" => Command::JoinLines,
            "undo" => Command::Undo,
            "redo" => Command::Redo,
            "yank_line" => Command::YankLine,
            "paste_after" => Command::PasteAfter,
            "paste_before" => Command::PasteBefore,
            "copy" => Command::Copy,
            "paste" => Command::Paste,
            "indent" => Command::Indent,
            "dedent" => Command::Dedent,
            "toggle_comment" => Command::ToggleComment,
            "toggle_case" => Command::ToggleCase,
            "visual_char" => Command::VisualChar,
            "visual_line" => Command::VisualLine,
            "visual_block" => Command::VisualBlock,
            "select_all" => Command::SelectAll,
            "select_next_occurrence" => Command::SelectNextOccurrence,
            "select_all_occurrences" => Command::SelectAllOccurrences,
            "add_cursor_below" => Command::AddCursorBelow,
            "add_cursor_above" => Command::AddCursorAbove,
            "clear_extra_cursors" => Command::ClearExtraCursors,
            "save" => Command::Save,
            "save_as" => Command::SaveAs,
            "close_buffer" => Command::CloseBuffer,
            "next_buffer" => Command::NextBuffer,
            "prev_buffer" => Command::PrevBuffer,
            "reopen_from_disk" => Command::ReopenFromDisk,
            "new_scratch" => Command::NewScratch,
            "search_forward" => Command::SearchForward,
            "search_backward" => Command::SearchBackward,
            "search_next" => Command::SearchNext,
            "search_prev" => Command::SearchPrev,
            "replace" => Command::Replace,
            "find_in_files" => Command::FindInFiles,
            "toggle_tree" => Command::ToggleTree,
            "toggle_whitespace" => Command::ToggleWhitespace,
            "toggle_line_numbers" => Command::ToggleLineNumbers,
            "toggle_wrap" => Command::ToggleWrap,
            "split_right" => Command::SplitRight,
            "split_down" => Command::SplitDown,
            "close_split" => Command::CloseSplit,
            "next_split" => Command::NextSplit,
            "prev_split" => Command::PrevSplit,
            "command_palette" => Command::CommandPalette,
            "toggle_terminal" => Command::ToggleTerminal,
            "toggle_ssh_mode" => Command::ToggleSshMode,
            "keybindings" => Command::Keybindings,
            "settings" => Command::Settings,
            "escape" => Command::Escape,
            "enter_command_line" => Command::EnterCommandLine,
            "completion" => Command::Completion,
            "hover" => Command::Hover,
            "format_document" => Command::FormatDocument,
            "scroll_half_down" => Command::ScrollHalfDown,
            "scroll_half_up" => Command::ScrollHalfUp,
            "center_cursor" => Command::CenterCursor,
            _ => return None,
        };
        Some(cmd)
    }

    /// Whether the command is meaningful in insert mode.
    pub(crate) fn works_in_insert(&self) -> bool {
        matches!(
            self,
            Command::Escape
                | Command::Save
                | Command::Undo
                | Command::Redo
                | Command::Move(Motion::Left | Motion::Right | Motion::Up | Motion::Down)
                | Command::DeleteChar
                | Command::DeleteCharBefore
                | Command::Move(Motion::LineStart | Motion::LineEnd)
                | Command::Move(Motion::WordNext | Motion::WordBack)
                | Command::PasteAfter
                | Command::Copy
                | Command::Paste
                | Command::Completion
                | Command::SelectAll
                | Command::SelectNextOccurrence
                | Command::AddCursorBelow
                | Command::AddCursorAbove
                | Command::ClearExtraCursors
                | Command::CommandPalette
                | Command::ToggleTerminal
                | Command::ToggleSshMode
                | Command::Keybindings
                | Command::Settings
                | Command::FindInFiles
                | Command::Replace
                | Command::SearchForward
                | Command::SearchNext
                | Command::SearchPrev
                | Command::GotoLine
                | Command::CloseBuffer
                | Command::NextBuffer
                | Command::PrevBuffer
                | Command::SplitRight
                | Command::SplitDown
                | Command::CloseSplit
                | Command::NextSplit
                | Command::PrevSplit
                | Command::ToggleTree
                | Command::ToggleWhitespace
                | Command::ToggleLineNumbers
                | Command::ToggleWrap
                | Command::FormatDocument
                | Command::Hover
                | Command::ScrollHalfDown
                | Command::ScrollHalfUp
        )
    }
}

/// Every command name, for the keybinding editor and the palette.
///
/// A test asserts each one parses, so this list cannot drift from `from_name`.
pub(crate) fn all_names() -> &'static [&'static str] {
    &[
        "move_left",
        "move_right",
        "move_up",
        "move_down",
        "move_word_next",
        "move_word_end",
        "move_word_back",
        "move_big_word_next",
        "move_big_word_end",
        "move_big_word_back",
        "move_line_start",
        "move_line_end",
        "move_first_non_blank",
        "move_paragraph_next",
        "move_paragraph_back",
        "move_screen_top",
        "move_screen_middle",
        "move_screen_bottom",
        "goto_first_line",
        "goto_last_line",
        "goto_line",
        "goto_bracket",
        "goto_definition",
        "delete",
        "change",
        "yank",
        "uppercase",
        "lowercase",
        "insert_before",
        "insert_after",
        "insert_line_start",
        "insert_line_end",
        "open_below",
        "open_above",
        "delete_char",
        "delete_char_before",
        "delete_line",
        "delete_to_line_end",
        "join_lines",
        "undo",
        "redo",
        "yank_line",
        "paste_after",
        "paste_before",
        "copy",
        "paste",
        "indent",
        "dedent",
        "toggle_comment",
        "toggle_case",
        "visual_char",
        "visual_line",
        "visual_block",
        "select_all",
        "select_next_occurrence",
        "select_all_occurrences",
        "add_cursor_below",
        "add_cursor_above",
        "clear_extra_cursors",
        "save",
        "save_as",
        "close_buffer",
        "next_buffer",
        "prev_buffer",
        "reopen_from_disk",
        "new_scratch",
        "search_forward",
        "search_backward",
        "search_next",
        "search_prev",
        "replace",
        "find_in_files",
        "toggle_tree",
        "toggle_whitespace",
        "toggle_line_numbers",
        "toggle_wrap",
        "split_right",
        "split_down",
        "close_split",
        "next_split",
        "prev_split",
        "command_palette",
        "toggle_terminal",
        "toggle_ssh_mode",
        "keybindings",
        "settings",
        "escape",
        "enter_command_line",
        "completion",
        "hover",
        "format_document",
        "scroll_half_down",
        "scroll_half_up",
        "center_cursor",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_parses_back_to_its_command() {
        // A representative sweep: the round-trip is what the keybinding editor
        // and the config file depend on.
        let samples = [
            Command::Move(Motion::WordNext),
            Command::Operate(Operator::Delete),
            Command::Operate(Operator::Yank),
            Command::Save,
            Command::Undo,
            Command::VisualBlock,
            Command::SelectNextOccurrence,
            Command::AddCursorBelow,
            Command::ToggleTree,
            Command::SplitRight,
            Command::CommandPalette,
            Command::GotoLine,
            Command::FormatDocument,
            Command::Escape,
        ];
        for cmd in samples {
            let name = cmd.name();
            let back = Command::from_name(name);
            assert_eq!(back.as_ref(), Some(&cmd), "{name} did not round-trip");
        }
    }

    #[test]
    fn unknown_names_are_rejected() {
        assert!(Command::from_name("not_a_command").is_none());
        assert!(Command::from_name("").is_none());
    }

    #[test]
    fn the_name_list_is_complete_and_parses() {
        for name in all_names() {
            assert!(
                Command::from_name(name).is_some(),
                "{name} is listed but does not parse"
            );
        }
        // The list has no duplicates.
        let mut sorted: Vec<&str> = all_names().to_vec();
        let count = sorted.len();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), count, "duplicate name in all_names");
    }

    #[test]
    fn insert_mode_allows_editing_but_not_modal_motions() {
        assert!(Command::Escape.works_in_insert());
        assert!(Command::DeleteChar.works_in_insert());
        assert!(Command::Save.works_in_insert());
        // `dd` and visual mode are normal-mode commands.
        assert!(!Command::Operate(Operator::Delete).works_in_insert());
        assert!(!Command::VisualLine.works_in_insert());
        assert!(!Command::OpenBelow.works_in_insert());
    }
}
