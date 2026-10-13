//! Hosting the editor inside the application.
//!
//! The editor owns its buffers, modes and layout; this module is the seam: it
//! builds the editor's colours from the application theme, routes keys to it
//! while it is in front, and services the requests it makes back (switch to the
//! terminal, open the settings, write a remote buffer).

use super::*;
use crate::editor::{Editor, EditorStyle, Settings};

impl App {
    /// The editor's colours, taken from the application theme.
    pub(crate) fn editor_style(&self) -> EditorStyle {
        let ui = &self.theme.ui;
        // The application paints one shared backdrop for the whole client area
        // (wallpaper plus a single tint) and the terminal deliberately does not
        // repaint it. The editor must not either: filling its area with the
        // surface colour again would tint the wallpaper a second time and wash
        // it out, which is what made the background look like it disappeared
        // when the editor came up.
        EditorStyle {
            bg: egui::Color32::TRANSPARENT,
            fg: color32(ui.text),
            dim: color32(ui.text_dim),
            accent: color32(ui.accent),
            selection: ui::with_alpha(color32(ui.accent), self.ui_alpha() * 0.28),
            current_line: ui::with_alpha(color32(self.theme.bg), self.ui_alpha() * 0.55),
            panel_wash: ui::with_alpha(color32(ui.panel_bg), self.panel_alpha()),
            border: color32(ui.border),
            match_bg: ui::with_alpha(color32(ui.accent), self.ui_alpha() * 0.16),
            // Popups and dialogs are transient and must stay readable over any
            // wallpaper, so they keep the chrome's stronger floor.
            panel_bg: ui::with_alpha(color32(ui.panel_bg), self.ui_alpha()),
            warn: color32(ui.text),
            error: color32(ui.danger),
        }
    }

    /// The editor's font: the terminal's size, egui's monospace family.
    pub(crate) fn editor_font(&self) -> egui::FontId {
        egui::FontId::new(self.config.font_size.max(8.0), egui::FontFamily::Monospace)
    }

    /// Build the editor from the config.
    pub(crate) fn make_editor(&self) -> Editor {
        let settings: Settings = self.config.editor.to_settings();
        let keymap = crate::editor::Keymap::from_config(&self.config.editor_keys);
        let root = self
            .config
            .editor_root
            .as_deref()
            .map(|s| crate::editor::expand(std::path::Path::new(s)))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        let mut editor = Editor::new(settings, keymap, root);
        editor.tree.open = self.config.editor.tree_open;
        editor
    }

    /// Draw the editor surface.
    pub(crate) fn editor_area(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let style = self.editor_style();
        let font = self.editor_font();
        self.editor.now = self.now;
        self.editor.frame_dt = self.frame_dt;
        // The editor owns the keyboard while it is in front: egui's own focus
        // navigation must not swallow Tab or the arrow keys.
        let resp = ui.interact(
            rect,
            ui.id().with("editor-surface"),
            egui::Sense::click_and_drag(),
        );
        if matches!(self.overlay, Overlay::None)
            && self.editor.tree.prompt.is_none()
            && !resp.has_focus()
        {
            resp.request_focus();
        }
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                resp.id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });
        self.editor.draw(ui, rect, &style, font);
    }

    /// Route a key press to the editor. Returns true when it was consumed.
    pub(crate) fn editor_key(
        &mut self,
        key: egui::Key,
        mods: egui::Modifiers,
        text: Option<&str>,
    ) -> bool {
        self.editor.now = self.now;
        self.editor.key(key, mods, text)
    }

    /// Service the requests the editor makes back to the application.
    pub(crate) fn service_editor(&mut self, ctx: &egui::Context) {
        if self.editor.wants_terminal {
            self.editor.wants_terminal = false;
            self.set_view(View::Terminal);
        }
        if self.editor.wants_ssh_mode {
            self.editor.wants_ssh_mode = false;
            self.toggle_ssh_mode();
        }
        if self.editor.wants_settings {
            self.editor.wants_settings = false;
            self.open_settings();
        }
        if self.editor.wants_keybindings {
            self.editor.wants_keybindings = false;
            self.open_keybindings();
        }
        // A remote buffer was saved: hand the bytes to the SFTP session.
        if let Some(save) = self.editor.pending_remote_save.take() {
            self.save_remote_buffer(save);
        }
        // A file read that finished on its worker thread. Polled whichever view
        // is in front, so a read started in the editor still lands after the
        // user switches to the terminal.
        if self.editor.poll_loading() {
            ctx.request_repaint();
        }
        // Files that changed under an open buffer.
        self.editor.check_external_changes(self.now);
        // The file tree follows the focused terminal's directory, so a `cd`
        // there moves the tree; a root pinned with `editor_root` wins.
        self.follow_terminal_root();
        // The editor's own settings are the config's, so a change made with
        // `:set` is written back.
        let settings = crate::config::EditorConfig::from_settings(
            &self.editor.settings,
            self.editor.tree.open,
            self.config.editor.tree_width,
        );
        if settings != self.config.editor {
            self.config.editor = settings;
            self.persist_config();
        }
        // A finished tree-width drag: remember the new width. Only here, so
        // the config is not rewritten for every frame the drag moves.
        if self.editor.wants_width_save {
            self.editor.wants_width_save = false;
            if (self.config.editor.tree_width - self.editor.tree.width).abs() > 0.5 {
                self.config.editor.tree_width = self.editor.tree.width;
                self.persist_config();
            }
        }
        let _ = ctx;
    }

    /// Keep the editor's file tree rooted at the focused terminal's directory.
    ///
    /// `cd` in the shell is how you say what you are working on, so the tree
    /// follows it — unless the config pins a root with `editor_root`. Reading
    /// `/proc` is cheap but not free, so it is polled rather than read every
    /// frame.
    fn follow_terminal_root(&mut self) {
        const EVERY_SECS: f64 = 0.25;
        if self.config.editor_root.is_some() || self.now < self.last_root_follow + EVERY_SECS {
            return;
        }
        self.last_root_follow = self.now;
        let Some(pane) = self.tabs.get(self.active) else {
            return;
        };
        // SSH panes have no local pid; their directory is on the remote host.
        let Some(pid) = pane.shell_pid else {
            return;
        };
        if pane.dead.is_some() {
            return;
        }
        let Some(cwd) = crate::term::local::shell_cwd(pid) else {
            return;
        };
        // A shell sitting in a deleted directory reports a path that ends in
        // " (deleted)" and no longer exists; following it would root the tree
        // at a ghost.
        if cwd != self.editor.tree.root && cwd.is_absolute() && cwd.exists() {
            log::info!("editor tree follows the terminal: {}", cwd.display());
            self.editor.tree.set_root(cwd);
        }
    }

    /// Write a remote buffer back over SFTP.
    ///
    /// The bytes and the destination were captured when the save was asked for,
    /// so the buffer may already be closed by the time this runs.
    fn save_remote_buffer(&mut self, save: crate::editor::PendingRemoteSave) {
        let Some(panel) = &self.sftp else {
            self.toast = Some((
                format!(
                    "no SFTP session for {} — open the file browser",
                    save.host
                ),
                self.now,
            ));
            return;
        };
        if !panel.connected {
            self.toast = Some((
                format!("SFTP session for {} is not connected", save.host),
                self.now,
            ));
            return;
        }
        panel.request(crate::term::sftp::SftpRequest::Write {
            path: save.path.clone(),
            data: save.text.into_bytes(),
        });
        if let Some(buffer) = self.editor.buffers.get_mut(save.buffer) {
            buffer.mark_saved();
        }
        self.toast = Some((format!("wrote {}", save.path), self.now));
        log::info!("remote save: {}", save.path);
    }

    /// Open a remote file in the editor.
    pub(crate) fn open_remote_in_editor(&mut self, host: String, path: String, data: Vec<u8>) {
        let text = String::from_utf8_lossy(&data).into_owned();
        self.editor.open_remote(host, path, text);
        self.set_view(View::Editor);
    }

    /// Switch the front surface, keeping both alive.
    pub(crate) fn set_view(&mut self, view: View) {
        if self.view == view {
            return;
        }
        self.view = view;
        log::info!("view: {view:?}");
        // Remember it for the next launch.
        self.config.view = match view {
            View::Terminal => "terminal".to_owned(),
            View::Editor => "editor".to_owned(),
        };
        self.persist_config();
        // The editor keeps its buffers and unsaved work across the switch.
        if view == View::Editor && self.editor.buffers.iter().all(|b| b.path.is_none()) {
            // A first visit with nothing open: show the tree, not a blank page.
            self.editor.tree.invalidate();
        }
    }

    /// Open the SFTP browser for the host of the focused pane.
    ///
    /// The browser needs its own SSH connection, so it is opened on demand and
    /// only for a pane that is actually an SSH session.
    pub(crate) fn open_sftp(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let Some(profile) = tab.ssh_profile.clone() else {
            self.toast = Some((
                "SFTP needs an SSH session — open one first".to_owned(),
                self.now,
            ));
            return;
        };
        // The size is irrelevant for a subsystem channel; the session never
        // opens a shell.
        let params = self.ssh_params(&profile, 80, 24);
        let wake = self.waker.clone();
        self.sftp = Some(sftp_panel::SftpPanel::connect(params, wake));
        self.toast = Some((format!("SFTP: {}", profile.name), self.now));
        log::info!("sftp panel opened for {}", profile.name);
    }

    /// Poll the SFTP worker and hand finished reads to the editor.
    pub(crate) fn poll_sftp(&mut self) {
        let Some(panel) = &mut self.sftp else {
            return;
        };
        panel.poll();
        if let Some((path, data)) = panel.pending_open.take() {
            let host = panel.host.clone();
            self.open_remote_in_editor(host, path, data);
        }
    }

    /// Open the settings modal.
    pub(crate) fn open_settings(&mut self) {
        let draft = self.config.clone();
        let font = self.config.font.clone();
        self.overlay = Overlay::Settings {
            draft,
            font,
            error: None,
            known_hosts: crate::config::known_hosts::list(),
            original: self.config.clone(),
            original_font: self.config.font.clone(),
        };
    }

    /// Open the keybinding editor.
    pub(crate) fn open_keybindings(&mut self) {
        let mut bindings: std::collections::BTreeMap<String, String> =
            self.editor.keymap.to_config().into_iter().collect();
        // Show every command the editor knows, even unbound ones, so a binding
        // can be added from scratch.
        for &name in crate::editor::command::all_names() {
            bindings.entry(name.to_owned()).or_default();
        }
        self.overlay = Overlay::Keybindings {
            bindings,
            recording: None,
            error: None,
        };
    }
}
