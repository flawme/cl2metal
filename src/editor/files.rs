//! Buffers, files and the commands that manage them.

use super::buffer::{Buffer, ExternalChange, RemoteFile};
use super::command::Command;
use super::mode::{CommandKind, Mode};
use super::pos::{Cursor, Pos};
use super::search::{self, Query};
use super::{Editor, Palette, SplitDir};
use std::path::{Path, PathBuf};

/// How often the editor re-checks open files for outside changes. Half a
/// second reads as real time to the person deleting the file, and a stat per
/// buffer twice a second costs nothing.
const EXTERNAL_CHECK_SECS: f64 = 0.5;

impl Editor {
    /// The commands that need the editor shell rather than the buffer.
    pub(crate) fn execute_outer(&mut self, cmd: Command) {
        match cmd {
            Command::Save => self.save_active(),
            Command::SaveAs => {
                self.mode = Mode::Command(CommandKind::Ex);
                self.command_line = "w ".to_owned();
            }
            Command::CloseBuffer => self.close_buffer(self.active),
            Command::NextBuffer => self.switch_buffer(1),
            Command::PrevBuffer => self.switch_buffer(-1),
            Command::ReopenFromDisk => self.reopen_active(),
            Command::NewScratch => self.new_scratch(),
            Command::GotoLine => {
                self.mode = Mode::Command(CommandKind::Ex);
                self.command_line.clear();
            }
            Command::GotoFirstLine => self.goto_line(1),
            Command::GotoLastLine => self.goto_line(usize::MAX),
            Command::GotoDefinition
            | Command::Hover
            | Command::Completion
            | Command::FormatDocument => {
                self.say("no language server is attached to this buffer");
            }
            Command::SearchForward => {
                self.mode = Mode::Command(CommandKind::SearchForward);
                self.command_line.clear();
            }
            Command::SearchBackward => {
                self.mode = Mode::Command(CommandKind::SearchBackward);
                self.command_line.clear();
            }
            Command::Replace => {
                self.find.open = true;
                self.find.replace_open = true;
                self.find.focus = 0;
            }
            Command::FindInFiles => {
                self.find.open = true;
                self.find.replace_open = false;
                self.find.focus = 0;
            }
            Command::ToggleTree => self.tree.open = !self.tree.open,
            Command::ToggleWhitespace => {
                self.settings.show_whitespace = !self.settings.show_whitespace
            }
            Command::ToggleLineNumbers => self.settings.line_numbers = !self.settings.line_numbers,
            Command::ToggleWrap => self.settings.wrap = !self.settings.wrap,
            Command::SplitRight => self.split(SplitDir::Right),
            Command::SplitDown => self.split(SplitDir::Down),
            Command::CloseSplit => self.close_split(),
            Command::NextSplit => self.layout.focus_next(true),
            Command::PrevSplit => self.layout.focus_next(false),
            Command::CommandPalette => {
                let palette = Palette::new(self);
                self.palette = Some(palette);
            }
            Command::ToggleTerminal => self.wants_terminal = true,
            Command::ToggleSshMode => self.wants_ssh_mode = true,
            Command::Keybindings => self.wants_keybindings = true,
            Command::Settings => self.wants_settings = true,
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Buffers
    // -----------------------------------------------------------------------

    /// Open a local file, reusing an open buffer when there is one.
    /// Open a file, reading it on a worker thread.
    ///
    /// The read is the one part of opening that cannot be made cheap — a 90 MB
    /// file takes about 0.7 s in a debug build — so it runs off the UI thread
    /// and the editor shows a loading indicator until the buffer lands. Returns
    /// true when the file is open already or is on its way.
    pub(crate) fn open_path(&mut self, path: &Path) -> bool {
        let path = expand(path);
        if let Some(index) = self
            .buffers
            .iter()
            .position(|b| b.path.as_deref() == Some(path.as_path()))
        {
            self.focus_buffer(index);
            return true;
        }
        // A second click on the same row must not start a second read.
        if self.loading.as_ref().is_some_and(|l| l.path == path) {
            return true;
        }
        let (tx, rx) = flume::bounded(1);
        let thread_path = path.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let result = Buffer::open(&thread_path);
            log::info!(
                "read {} in {:.2}s",
                thread_path.display(),
                started.elapsed().as_secs_f32()
            );
            let _ = tx.send(result);
        });
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.say(format!("opening {name}…"));
        self.loading = Some(super::Loading {
            path,
            started: self.now,
            rx,
        });
        true
    }

    /// Pick up a finished file read.
    ///
    /// Called every frame, whichever view is in front, so a read started in the
    /// editor still lands after the user switches to the terminal.
    pub(crate) fn poll_loading(&mut self) -> bool {
        let Some(loading) = self.loading.as_ref() else {
            return false;
        };
        let result = loading.rx.try_recv();
        if matches!(result, Err(flume::TryRecvError::Empty)) {
            return false;
        }
        let loading = self.loading.take().expect("checked just above");
        match result {
            Ok(Ok(buffer)) => {
                let title = buffer.title();
                self.buffers.push(buffer);
                let index = self.buffers.len() - 1;
                self.focus_buffer(index);
                self.tree.reveal(&loading.path);
                self.say(format!("opened {title}"));
            }
            Ok(Err(e)) => {
                self.say(format!("cannot open {}: {e}", loading.path.display()));
            }
            Err(_) => {
                self.say(format!(
                    "cannot open {}: the reader stopped",
                    loading.path.display()
                ));
            }
        }
        true
    }

    /// Open a file that lives on an SSH host.
    pub(crate) fn open_remote(&mut self, host: String, path: String, text: String) {
        if let Some(index) = self.buffers.iter().position(|b| {
            b.remote
                .as_ref()
                .is_some_and(|r| r.host == host && r.path == path)
        }) {
            self.focus_buffer(index);
            return;
        }
        let mut buffer = Buffer::from_text(&text, None);
        buffer.remote = Some(RemoteFile {
            host: host.clone(),
            path: path.clone(),
        });
        self.buffers.push(buffer);
        let index = self.buffers.len() - 1;
        self.focus_buffer(index);
        self.say(format!("opened {host}:{path}"));
    }

    /// Point the active pane at a buffer.
    pub(crate) fn focus_buffer(&mut self, index: usize) {
        if index >= self.buffers.len() {
            return;
        }
        self.active = index;
        self.layout.set_focused(index);
    }

    /// Save the active buffer. Remote buffers are handed to the app, which owns
    /// the SFTP session.
    pub(crate) fn save_active(&mut self) {
        let index = self.active;
        if self.buffers[index].is_remote() {
            let buffer = &self.buffers[index];
            let Some(remote) = buffer.remote.clone() else {
                return;
            };
            self.pending_remote_save = Some(super::PendingRemoteSave {
                buffer: index,
                host: remote.host,
                path: remote.path,
                text: buffer.text(),
            });
            return;
        }
        // A file that was deleted on disk comes back with this save: the
        // editor recreates it rather than losing the work, but it says so,
        // because a deleted file quietly reappearing is its own surprise.
        let was_deleted = self.buffers[index]
            .path
            .as_deref()
            .is_some_and(|p| !p.exists());
        match self.buffers[index].save() {
            Ok(()) => {
                let title = self.buffers[index].title();
                if was_deleted {
                    self.say(format!("recreated {title} — it had been deleted on disk"));
                } else {
                    self.say(format!("wrote {title}"));
                }
            }
            Err(e) => {
                let name = self.buffers[index].title();
                self.say(format!("cannot write {name}: {e}"));
            }
        }
    }

    /// Save the active buffer under a new name.
    pub(crate) fn save_as(&mut self, path: &str) {
        let path = expand(Path::new(path));
        let index = self.active;
        self.buffers[index].path = Some(path.clone());
        self.buffers[index].remote = None;
        // It has somewhere to live now, so typing into it is ordinary again.
        self.buffers[index].scratch = false;
        match self.buffers[index].save() {
            Ok(()) => {
                self.tree.invalidate();
                self.say(format!("wrote {}", path.display()));
            }
            Err(e) => self.say(format!("cannot write {}: {e}", path.display())),
        }
    }

    /// Move a file or folder inside the tree, keeping the buffers that point
    /// at it pointing at the new location.
    pub(crate) fn move_in_tree(&mut self, from: &Path, to_dir: &Path) {
        match self.tree.mv(from, to_dir) {
            Ok(to) => {
                self.buffers_follow(from, &to);
                let name = from
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.say(format!("moved {name}"));
            }
            Err(e) => self.say(format!("cannot move {}: {e}", from.display())),
        }
    }

    /// Rewrite the paths of the buffers that lived at `from`, or inside it,
    /// so they follow it to `to`.
    ///
    /// A rename and a move are the same operation as far as an open buffer is
    /// concerned; without this, saving after either writes the old path back
    /// onto the disk.
    pub(crate) fn buffers_follow(&mut self, from: &Path, to: &Path) {
        for buffer in &mut self.buffers {
            let Some(path) = buffer.path.clone() else {
                continue;
            };
            if path == from {
                buffer.path = Some(to.to_path_buf());
            } else if let Ok(rest) = path.strip_prefix(from) {
                buffer.path = Some(to.join(rest));
            }
        }
    }

    /// A file or folder was deleted through the tree: the buffers that lived
    /// there must not keep saving into a path that is no longer on the disk.
    ///
    /// A clean buffer is simply closed; a dirty one keeps its text but loses
    /// its path, so saving it asks for a name instead of quietly resurrecting
    /// the file the user just deleted.
    /// Returns whether any dirty buffer was detached — the caller's message
    /// yields to that one, because "your text is safe" outranks "it moved".
    pub(crate) fn tree_deleted(&mut self, path: &Path) -> bool {
        let mut close: Vec<usize> = Vec::new();
        let mut detached = false;
        for (index, buffer) in self.buffers.iter_mut().enumerate() {
            let Some(p) = buffer.path.clone() else {
                continue;
            };
            if p != path && !p.starts_with(path) {
                continue;
            }
            if buffer.dirty {
                buffer.path = None;
                buffer.remote = None;
                detached = true;
            } else {
                close.push(index);
            }
        }
        for index in close.into_iter().rev() {
            self.close_buffer_now(index);
        }
        if detached {
            self.say("kept the unsaved text; save it under a new name");
        }
        detached
    }

    /// Move a tree entry to the system trash.
    ///
    /// A plain delete is the recoverable kind: the entry leaves its folder
    /// for the desktop's trash, so a mistake is undone from there instead
    /// of from a backup. When the trash cannot be reached the entry stays
    /// on disk and the status line says why — never a silent fall-back to
    /// erasing.
    pub(crate) fn trash_from_tree(&mut self, path: &Path) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Err(e) = trash::delete(path) {
            self.say(format!("could not trash {name}: {e}"));
            return;
        }
        let detached = self.tree_deleted(path);
        if self.tree.selected.as_deref() == Some(path) {
            self.tree.selected = None;
        }
        self.tree.refresh();
        if !detached {
            self.say(format!("{name} — moved to the trash"));
        }
    }

    /// Close a buffer. One with unsaved changes asks first.
    pub(crate) fn close_buffer(&mut self, index: usize) {
        if index >= self.buffers.len() {
            return;
        }
        if self.buffers[index].dirty {
            let can_save = self.buffers[index].path.is_some() || self.buffers[index].is_remote();
            self.close_prompt = Some(super::ClosePrompt {
                buffer: index,
                can_save,
            });
            return;
        }
        self.close_buffer_now(index);
    }

    /// Close a buffer without asking. The prompt's Discard, and `:q!`.
    pub(crate) fn close_buffer_now(&mut self, index: usize) {
        if index >= self.buffers.len() {
            return;
        }
        if self.buffers.len() == 1 {
            // Never leave the editor with no buffer at all.
            self.buffers[0] = Buffer::scratch();
            self.active = 0;
            self.layout = super::Layout::new();
            return;
        }
        self.buffers.remove(index);
        self.layout.remap(index);
        self.active = self.active.min(self.buffers.len() - 1);
        self.layout.set_focused(self.active);
    }

    /// Answer the close question: write the buffer first, or discard it.
    ///
    /// A buffer with nowhere to save to has only discard and cancel, so asking
    /// to save one leaves the question up rather than throwing the work away.
    pub(crate) fn answer_close(&mut self, save: bool) {
        let Some(prompt) = self.close_prompt.take() else {
            return;
        };
        let index = prompt.buffer;
        if index >= self.buffers.len() {
            return;
        }
        if save && !prompt.can_save {
            self.close_prompt = Some(prompt);
            return;
        }
        if save {
            if self.buffers[index].is_remote() {
                // The app does the write, and it may not get to it until after
                // this buffer is gone, so the bytes and the destination travel
                // with the request.
                let buffer = &self.buffers[index];
                let Some(remote) = buffer.remote.clone() else {
                    return;
                };
                self.pending_remote_save = Some(super::PendingRemoteSave {
                    buffer: index,
                    host: remote.host,
                    path: remote.path,
                    text: buffer.text(),
                });
            } else if let Err(e) = self.buffers[index].save() {
                let title = self.buffers[index].title();
                self.say(format!("cannot write {title}: {e}"));
                return;
            }
        }
        self.close_buffer_now(index);
    }

    /// Close the active buffer even if it has unsaved changes.
    pub(crate) fn force_close_active(&mut self) {
        let index = self.active;
        self.close_prompt = None;
        self.close_buffer_now(index);
    }

    /// Switch to the next or previous buffer.
    pub(crate) fn switch_buffer(&mut self, delta: i32) {
        if self.buffers.len() < 2 {
            return;
        }
        let len = self.buffers.len() as i32;
        let next = (self.active as i32 + delta).rem_euclid(len);
        self.focus_buffer(next as usize);
    }

    /// Jump to a line (1-based).
    pub(crate) fn goto_line(&mut self, line: usize) {
        let buf = self.buffer_mut();
        let last = buf.line_count().saturating_sub(1);
        let target = line.saturating_sub(1).min(last);
        let col = buf.first_non_blank(target);
        buf.cursors = vec![Cursor::new(Pos::new(target, col))];
        buf.scroll[1] = (target as f32 - 6.0).max(0.0);
    }

    /// Discard changes and re-read the file.
    pub(crate) fn reopen_active(&mut self) {
        let index = self.active;
        match self.buffers[index].reload() {
            Ok(()) => self.say("reloaded from disk"),
            Err(e) => self.say(format!("cannot reload: {e}")),
        }
    }

    /// A fresh empty buffer.
    pub(crate) fn new_scratch(&mut self) {
        self.buffers.push(Buffer::empty());
        let index = self.buffers.len() - 1;
        self.focus_buffer(index);
    }

    /// Split the focused pane.
    pub(crate) fn split(&mut self, dir: SplitDir) {
        self.layout.split(dir, self.active);
    }

    /// Close the focused pane, or the buffer when it is the last one.
    pub(crate) fn close_split(&mut self) {
        match self.layout.close() {
            Some(_closed) => {
                self.active = self.layout.focused();
            }
            None => self.close_buffer(self.active),
        }
    }

    /// Poll open files for changes made outside the editor.
    ///
    /// A file that was deleted on disk stops being edited: a clean buffer
    /// closes right away, so the editor falls back to choosing a file, and a
    /// dirty one keeps its text but loses its path, so saving asks for a name
    /// instead of quietly resurrecting what the user deleted.
    pub(crate) fn check_external_changes(&mut self, now: f64) {
        if now - self.last_external_check < EXTERNAL_CHECK_SECS {
            return;
        }
        self.last_external_check = now;
        self.external.clear();
        for (index, buffer) in self.buffers.iter().enumerate() {
            let change = buffer.check_external();
            if change != ExternalChange::None {
                self.external.push((index, change));
            }
        }
        let gone: Vec<usize> = self
            .external
            .iter()
            .filter(|(_, c)| *c == ExternalChange::Gone)
            .map(|(i, _)| *i)
            .collect();
        for index in gone.into_iter().rev() {
            let title = self.buffers[index].title();
            if self.buffers[index].dirty {
                self.buffers[index].path = None;
                self.buffers[index].remote = None;
                self.say(format!(
                    "{title} was deleted on disk; the text is kept — save it under a new name"
                ));
            } else {
                self.close_buffer_now(index);
                self.say(format!("{title} was deleted on disk"));
            }
        }
        // Closed buffers shift the indices; drop the stale entries so the
        // status line does not report a change for the wrong buffer.
        self.external
            .retain(|(i, c)| *c == ExternalChange::Changed && *i < self.buffers.len());
    }

    /// Whether a buffer's file changed underneath it.
    pub(crate) fn external_change(&self, index: usize) -> ExternalChange {
        self.external
            .iter()
            .find(|(i, _)| *i == index)
            .map_or(ExternalChange::None, |(_, c)| *c)
    }

    // -----------------------------------------------------------------------
    // Command lines
    // -----------------------------------------------------------------------

    /// Run a `:` line.
    pub(crate) fn run_ex_command(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if let Some(args) = line.strip_prefix("%s/") {
            self.substitute(args, true);
            return;
        }
        if let Some(args) = line.strip_prefix("s/") {
            self.substitute(args, false);
            return;
        }
        let (cmd, args) = match line.split_once(char::is_whitespace) {
            Some((cmd, args)) => (cmd, args.trim()),
            None => (line, ""),
        };
        match cmd {
            "w" | "write" => {
                if args.is_empty() {
                    self.save_active();
                } else {
                    self.save_as(args);
                }
            }
            "q" | "quit" => self.close_buffer(self.active),
            "q!" => self.force_close_active(),
            "wq" | "x" => {
                self.save_active();
                // A save that failed — a scratch buffer with no name, or a
                // remote write still in flight — leaves the buffer dirty, and
                // closing it then would throw the work away.
                if !self.buffer().dirty {
                    self.close_buffer_now(self.active);
                }
            }
            "e" | "edit" => {
                if !args.is_empty() {
                    self.open_path(Path::new(args));
                }
            }
            "sp" | "split" => self.split(SplitDir::Down),
            "vsp" | "vsplit" => self.split(SplitDir::Right),
            "noh" | "nohl" | "nohlsearch" => {
                self.search.query = Query::default();
                self.say("search highlight off");
            }
            "tree" => self.tree.open = !self.tree.open,
            "set" => self.set_option(args),
            _ => {
                if let Ok(n) = cmd.parse::<usize>() {
                    self.goto_line(n);
                    return;
                }
                self.say(format!("unknown command: :{cmd}"));
            }
        }
    }

    /// `:s/old/new/g` and `:%s/old/new/g`.
    fn substitute(&mut self, args: &str, whole_file: bool) {
        let parts: Vec<&str> = args.split('/').collect();
        if parts.len() < 2 {
            self.say("usage: :s/old/new/[g]");
            return;
        }
        let (old, new) = (parts[0], parts[1]);
        let global = parts.get(2).is_some_and(|f| f.contains('g'));
        if old.is_empty() {
            self.say("nothing to replace");
            return;
        }
        let query = Query::new(old);
        if whole_file {
            let count = search::replace_all(self.buffer_mut(), &query, new);
            self.say(format!("{count} replacements"));
            return;
        }
        let line = self.buffer().cursors.first().map_or(0, |c| c.pos.line);
        let text = self.buffer().line(line).to_owned();
        let replaced = if global {
            text.replace(old, new)
        } else {
            text.replacen(old, new, 1)
        };
        if replaced == text {
            self.say("no match on this line");
            return;
        }
        let part = self.buffer().lines_part(line, 1, vec![replaced]);
        let cursors = self.buffer().cursors.clone();
        self.buffer_mut().apply_parts(vec![part], cursors, false);
        self.say("1 replacement");
    }

    /// `:set` options.
    fn set_option(&mut self, args: &str) {
        for arg in args.split_whitespace() {
            let (name, value) = match arg.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (arg, None),
            };
            let on = !name.starts_with("no");
            let base = name.strip_prefix("no").unwrap_or(name);
            match (base, value) {
                ("tabstop" | "ts", Some(v)) => match v.parse::<usize>() {
                    Ok(n) => self.settings.tab_width = n.clamp(1, 16),
                    Err(_) => self.say(format!("not a number: {v}")),
                },
                ("expandtab" | "et", _) => self.settings.expand_tabs = on,
                ("number" | "nu", _) => self.settings.line_numbers = on,
                ("relativenumber" | "rnu", _) => self.settings.relative_numbers = on,
                ("wrap", _) => self.settings.wrap = on,
                ("list" | "whitespace", _) => self.settings.show_whitespace = on,
                ("cursorline" | "cul", _) => self.settings.highlight_current_line = on,
                ("scrolloff" | "so", Some(v)) => match v.parse::<usize>() {
                    Ok(n) => self.settings.scroll_off = n.min(20),
                    Err(_) => self.say(format!("not a number: {v}")),
                },
                ("autoindent" | "ai", _) => self.settings.auto_indent = on,
                _ => self.say(format!("unknown option: {arg}")),
            }
        }
    }

    /// Run a `/` or `?` line.
    pub(crate) fn run_search_line(&mut self, line: &str, backward: bool) {
        if line.is_empty() {
            // A bare `/` repeats the last search.
            if self.search.query.is_empty() {
                return;
            }
        } else {
            self.search.query = Query::new(line);
        }
        self.search.backward = backward;
        self.search_step(backward);
    }

    /// `n` / `N`: jump to the next or previous match.
    pub(crate) fn search_step(&mut self, backward: bool) {
        if self.search.query.is_empty() {
            self.say("no previous search");
            return;
        }
        let backward = backward != self.search.backward;
        let query = self.search.query.clone();
        let buf = self.buffer_mut();
        let from = buf.cursors.first().map_or(Pos::new(0, 0), |c| c.pos);
        let Some(span) = search::find_next(buf, &query, from, backward) else {
            self.say(format!("no match for {}", query.text));
            return;
        };
        buf.cursors = vec![Cursor::new(span.start)];
        let line = span.start.line as f32;
        if line < buf.scroll[1] || line > buf.scroll[1] + 20.0 {
            buf.scroll[1] = (line - 6.0).max(0.0);
        }
    }
}

/// Expand a leading `~` to the home directory.
pub(crate) fn expand(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    if text == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    path.to_path_buf()
}
