//! The cl2metal editor: a modal, Vim-inspired text and code editor.
//!
//! Layout: `buffer` owns text, cursors and undo; `mode`, `motion` and
//! `operator` implement modal editing; `exec` runs commands; `input` routes
//! keys through `keymap`; `view` draws it with egui; `files` owns buffers,
//! saving and the file tree; `layout` splits the area; `syntax` highlights;
//! `search` finds and replaces; `lang` knows per-language conventions.

mod buffer;
pub(crate) mod command;
mod exec;
mod files;
mod input;
pub(crate) mod keymap;
mod lang;
mod layout;
mod mode;
mod motion;
mod operator;
mod palette;
mod panels;
mod pos;
mod register;
mod search;
mod syntax;
mod tree;
mod undo;
mod view;

#[cfg(test)]
mod tests;

pub(crate) use buffer::Buffer;
pub(crate) use buffer::MAX_EDIT_BYTES;
pub(crate) use files::expand;
pub(crate) use input::chord_string;
pub(crate) use keymap::Keymap;
pub(crate) use layout::SplitDir;
pub(crate) use palette::Palette;
pub(crate) use view::EditorStyle;

use buffer::ExternalChange;
use keymap::KeyChord;
use layout::Layout;
use mode::{Mode, Pending};
use pos::Pos;
use register::Registers;
use search::Query;
use syntax::Highlighter;
use tree::FileTree;

/// Editor behaviour that the config file can change.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Settings {
    /// Columns a tab occupies, and the width of one indent step.
    pub(crate) tab_width: usize,
    /// Insert spaces instead of a tab character.
    pub(crate) expand_tabs: bool,
    pub(crate) line_numbers: bool,
    /// Show the distance from the cursor instead of absolute numbers.
    pub(crate) relative_numbers: bool,
    pub(crate) show_whitespace: bool,
    pub(crate) wrap: bool,
    pub(crate) highlight_current_line: bool,
    /// `block`, `bar` or `underline`.
    pub(crate) cursor_shape: String,
    /// Keep this many lines visible above and below the cursor.
    pub(crate) scroll_off: usize,
    /// Copy the previous line's indentation on a new line.
    pub(crate) auto_indent: bool,
    /// Highlight every occurrence of the word under the cursor.
    pub(crate) highlight_word: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            expand_tabs: true,
            line_numbers: true,
            relative_numbers: false,
            show_whitespace: false,
            wrap: false,
            highlight_current_line: true,
            cursor_shape: "block".to_owned(),
            scroll_off: 3,
            auto_indent: true,
            highlight_word: true,
        }
    }
}

/// The find/replace panel's state.
#[derive(Clone, Default, Debug)]
pub(crate) struct FindPanel {
    pub(crate) open: bool,
    pub(crate) replace_open: bool,
    pub(crate) query: String,
    pub(crate) replacement: String,
    pub(crate) case_sensitive: bool,
    pub(crate) whole_word: bool,
    pub(crate) regex: bool,
    /// Matches in the active buffer, and which one the cursor is on.
    pub(crate) matches: usize,
    pub(crate) current: usize,
    /// Which field has focus: 0 query, 1 replacement.
    pub(crate) focus: usize,
}

/// The last search, so `n` and `N` can repeat it.
#[derive(Clone, Default, Debug)]
pub(crate) struct SearchState {
    pub(crate) query: Query,
    pub(crate) backward: bool,
}

/// A completion popup (LSP items, or words from the buffer).
#[derive(Clone, Default, Debug)]
pub(crate) struct Completion {
    pub(crate) items: Vec<String>,
    pub(crate) selected: usize,
    /// The text being completed, replaced when an item is accepted.
    pub(crate) prefix: String,
    pub(crate) start: Pos,
}

/// A question the editor asks before closing a buffer with unsaved changes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClosePrompt {
    /// The buffer the question is about.
    pub(crate) buffer: usize,
    /// Whether it has somewhere to save to. A scratch buffer does not, so its
    /// only answers are discard and cancel.
    pub(crate) can_save: bool,
}

/// A remote buffer waiting for the app to write it over SFTP.
///
/// The bytes and the destination are captured when the save is asked for, so
/// the write still happens if the buffer is closed before the app services it.
#[derive(Clone, Debug)]
pub(crate) struct PendingRemoteSave {
    /// The buffer it came from, so a completed write can clear its marker.
    pub(crate) buffer: usize,
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) text: String,
}

/// Highlighted spans for a whole document, one entry per line.
pub(crate) type DocumentSpans = Vec<Vec<(std::ops::Range<usize>, syntax::Style)>>;

/// A file being read on a worker thread.
///
/// Reading a long file is the one editor operation that cannot be made cheap:
/// a 90 MB file takes about 0.7 s in a debug build. It runs off the UI thread
/// so the window keeps drawing, and the editor shows a loading indicator until
/// the buffer lands.
pub(crate) struct Loading {
    pub(crate) path: std::path::PathBuf,
    /// When the read started, for the elapsed time in the indicator.
    pub(crate) started: f64,
    rx: flume::Receiver<std::io::Result<Buffer>>,
}

impl Loading {
    /// The name shown in the indicator.
    pub(crate) fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// How long the read has been running, in seconds.
    pub(crate) fn elapsed(&self, now: f64) -> f64 {
        (now - self.started).max(0.0)
    }
}

/// The editor: buffers, the mode, and everything the UI needs to draw it.
pub(crate) struct Editor {
    pub(crate) buffers: Vec<Buffer>,
    pub(crate) active: usize,
    pub(crate) mode: Mode,
    pub(crate) pending: Pending,
    /// Chords typed so far, waiting for the rest of a sequence.
    pub(crate) pending_keys: Vec<KeyChord>,
    pub(crate) registers: Registers,
    pub(crate) keymap: Keymap,
    pub(crate) settings: Settings,
    pub(crate) layout: Layout,
    pub(crate) tree: FileTree,
    pub(crate) highlighter: std::sync::Arc<Highlighter>,
    /// A highlight job in flight: the buffer, its version, and the result.
    pub(crate) syntax_rx: Option<(usize, u64, flume::Receiver<DocumentSpans>)>,
    /// A file being read on a worker thread.
    pub(crate) loading: Option<Loading>,
    /// The `:` or `/` line being typed.
    pub(crate) command_line: String,
    /// The status message and when it was set.
    pub(crate) message: Option<(String, f64)>,
    pub(crate) search: SearchState,
    pub(crate) find: FindPanel,
    pub(crate) palette: Option<Palette>,
    pub(crate) completion: Option<Completion>,
    /// Buffers whose file changed on disk since they were loaded.
    pub(crate) external: Vec<(usize, ExternalChange)>,
    /// A remote buffer waiting for the app to write it over SFTP.
    pub(crate) pending_remote_save: Option<PendingRemoteSave>,
    /// A buffer with unsaved changes that is asking what to do with them.
    pub(crate) close_prompt: Option<ClosePrompt>,
    /// When the files were last checked for outside changes.
    pub(crate) last_external_check: f64,
    /// When the last edit happened, for the "modified" indicator and autosave.
    pub(crate) last_edit_at: f64,
    /// The file tree's scroll offset.
    pub(crate) tree_scroll: f32,
    /// Frame time, handed in by the app.
    pub(crate) now: f64,
    /// Seconds since the previous frame, for easing.
    pub(crate) frame_dt: f32,
    /// Set when the editor wants the terminal view instead.
    pub(crate) wants_terminal: bool,
    /// Set when the editor wants the SSH sidebar toggled.
    pub(crate) wants_ssh_mode: bool,
    /// Set when the editor wants the app's settings or keybinding editor.
    pub(crate) wants_settings: bool,
    pub(crate) wants_keybindings: bool,
    /// Set when the tree's column width changed and the app should remember
    /// it. Raised when the drag ends, so a drag does not write the config
    /// on every frame it moves.
    pub(crate) wants_width_save: bool,
}

impl Editor {
    /// A new editor with one empty scratch buffer.
    pub(crate) fn new(settings: Settings, keymap: Keymap, root: std::path::PathBuf) -> Self {
        Self {
            buffers: vec![Buffer::scratch()],
            active: 0,
            mode: Mode::Normal,
            pending: Pending::default(),
            pending_keys: Vec::new(),
            registers: Registers::default(),
            keymap,
            settings,
            layout: Layout::new(),
            tree: FileTree::new(root),
            highlighter: std::sync::Arc::new(Highlighter::new()),
            syntax_rx: None,
            loading: None,
            command_line: String::new(),
            message: None,
            search: SearchState::default(),
            find: FindPanel::default(),
            palette: None,
            completion: None,
            external: Vec::new(),
            pending_remote_save: None,
            close_prompt: None,
            last_external_check: 0.0,
            last_edit_at: 0.0,
            tree_scroll: 0.0,
            now: 0.0,
            frame_dt: 0.0,
            wants_terminal: false,
            wants_ssh_mode: false,
            wants_settings: false,
            wants_keybindings: false,
            wants_width_save: false,
        }
    }

    /// The active buffer.
    pub(crate) fn buffer(&self) -> &Buffer {
        &self.buffers[self.active.min(self.buffers.len() - 1)]
    }

    /// The active buffer, mutably.
    pub(crate) fn buffer_mut(&mut self) -> &mut Buffer {
        let at = self.active.min(self.buffers.len() - 1);
        &mut self.buffers[at]
    }

    /// The buffer a layout leaf points at.
    pub(crate) fn buffer_at(&self, index: usize) -> &Buffer {
        &self.buffers[index.min(self.buffers.len() - 1)]
    }

    /// Show a message in the status line.
    pub(crate) fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), self.now));
    }

    /// Whether any buffer has unsaved changes.
    pub(crate) fn has_unsaved(&self) -> bool {
        self.buffers.iter().any(|b| b.dirty)
    }

    /// The names of buffers with unsaved changes.
    pub(crate) fn unsaved_names(&self) -> Vec<String> {
        self.buffers
            .iter()
            .filter(|b| b.dirty)
            .map(Buffer::title)
            .collect()
    }

    /// The comment prefix for the active buffer's language.
    pub(crate) fn comment_prefix(&self) -> &'static str {
        lang::comment_prefix(self.buffer().path.as_deref())
    }

    /// The language name shown in the status line.
    pub(crate) fn language(&self) -> &'static str {
        lang::language_name(self.buffer().path.as_deref())
    }

    /// Re-read the settings that come from the config file.
    pub(crate) fn apply_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }
}
