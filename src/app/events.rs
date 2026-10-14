//! Keyboard and pointer routing: shortcuts, terminal input encoding,
//! selection, scrolling, and the context menu.

use super::background::is_image_path;
use super::eggs::EasterEgg;
use super::*;

/// Quote a path for the shell, so a dropped file with spaces lands as one
/// word at the prompt.
pub(crate) fn shell_quote(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    let plain = text
        .chars()
        .all(|c| c.is_alphanumeric() || "._-/@:+=".contains(c));
    if plain {
        text.into_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

impl App {
    /// How long an image must hover over the window before it becomes the
    /// background. The delay is what lets one drag serve two purposes.
    pub(crate) const BG_DROP_SECS: f64 = 3.0;

    /// An image dragged over the window: a countdown runs before it becomes
    /// the background.
    ///
    /// The delay is what lets the same drag serve two purposes: hold the
    /// image over the window for three seconds and it becomes the wallpaper;
    /// drop it sooner — on the terminal, into an agent's prompt — and it goes
    /// there instead. Moving it off the window cancels.
    pub(crate) fn tick_background_drop(&mut self, ctx: &egui::Context) {
        if !matches!(self.overlay, Overlay::None) {
            self.bg_drop_at = None;
            return;
        }
        let hovered = ctx.input(|i| i.raw.hovered_files.clone());
        let image = hovered
            .iter()
            .filter_map(|f| f.path.clone())
            .find(|p| is_image_path(p));
        let Some(path) = image else {
            // Moved away, or nothing hovering: cancel.
            self.bg_drop_at = None;
            return;
        };
        if self.bg_drop_at.is_none() {
            // A fresh hover: a previous background application is history.
            self.bg_drop_done = false;
        }
        let at = *self.bg_drop_at.get_or_insert(self.now);
        if self.now - at >= Self::BG_DROP_SECS {
            self.bg_drop_at = None;
            self.bg_drop_done = true;
            self.set_background_image(&path, ctx);
        } else {
            // Keep the countdown animating.
            ctx.request_repaint();
        }
    }

    /// Make an image the background, and say so.
    fn set_background_image(&mut self, path: &std::path::Path, ctx: &egui::Context) {
        let path = path.to_string_lossy().into_owned();
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        log::info!("background image set: {path}");
        self.config.background_image = Some(path);
        self.sync_background(ctx);
        self.refresh_ui_alpha(ctx);
        if let Err(e) = self.config.save() {
            log::warn!("saving config after a background change: {e:#}");
        }
        self.toast = Some((format!("Background image: {name}"), self.now));
    }

    /// Files dropped on the window.
    ///
    /// On the terminal a drop types the file's path at the cursor — that is
    /// how an agent's prompt receives an attachment. In the editor,
    /// non-images open; an image is the wallpaper's, and the hover countdown
    /// above is how it gets there.
    pub(crate) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        // Any drop ends the background countdown: the file went somewhere.
        self.bg_drop_at = None;
        // The release that completed a background countdown is the tail of
        // that same drag: it must not also type the path.
        let consumed = self.bg_drop_done;
        self.bg_drop_done = false;
        if consumed {
            return;
        }
        // The settings modal owns the config while it is open.
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        let paths: Vec<std::path::PathBuf> = dropped
            .iter()
            .map(|f| f.path().to_path_buf())
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        if paths.is_empty() {
            return;
        }
        match self.view {
            View::Terminal => {
                let mut text = String::new();
                for path in &paths {
                    text.push_str(&shell_quote(path));
                    text.push(' ');
                }
                log::info!("dropped {} file(s) at the prompt", paths.len());
                self.send_input(text.into_bytes());
            }
            View::Editor => {
                for path in paths.iter().filter(|p| !is_image_path(p)) {
                    self.editor.open_path(path);
                }
            }
        }
    }

    pub(crate) fn handle_input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let mut mods = ctx.input(|i| i.modifiers);

        // 1. SSH prompts take the highest priority.
        if !self.prompts.is_empty() {
            let cancel = events.iter().any(|ev| {
                matches!(
                    ev,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            });
            if cancel {
                self.finish_prompt(false);
            }
            return;
        }

        // 2. Command palette.
        if matches!(self.overlay, Overlay::Palette { .. }) {
            let (query, mut selected) = match &self.overlay {
                Overlay::Palette { query, selected } => (query.clone(), *selected),
                _ => unreachable!(),
            };
            let mut action: Option<PaletteAction> = None;
            let mut close = false;
            let count = ui::palette_filtered(&self.config, &query, &self.recent_actions).len();
            for ev in &events {
                if let egui::Event::Key {
                    key, pressed: true, ..
                } = ev
                {
                    match key {
                        egui::Key::Escape => {
                            close = true;
                            break;
                        }
                        egui::Key::ArrowDown => {
                            selected = (selected + 1).min(count.saturating_sub(1));
                        }
                        egui::Key::ArrowUp => {
                            selected = selected.saturating_sub(1);
                        }
                        egui::Key::Enter => {
                            action =
                                ui::palette_filtered(&self.config, &query, &self.recent_actions)
                                    .get(selected)
                                    .map(|(_, a)| a.clone());
                            break;
                        }
                        _ => {}
                    }
                }
            }
            if let Some(a) = action {
                self.overlay = Overlay::None;
                self.run_action(a, ctx);
                return;
            }
            if close {
                self.overlay = Overlay::None;
                return;
            }
            if let Overlay::Palette {
                query: q,
                selected: s,
            } = &mut self.overlay
            {
                *q = query;
                *s = selected;
            }
            return;
        }

        // 3. Settings / connect / rename / profile / keybindings / images /
        //    close confirmations: Esc closes.
        if matches!(
            self.overlay,
            Overlay::Settings { .. }
                | Overlay::Connect { .. }
                | Overlay::Rename { .. }
                | Overlay::Profile { .. }
                | Overlay::Keybindings { .. }
                | Overlay::ViewImage { .. }
                | Overlay::Image { .. }
                | Overlay::CloseConfirm { .. }
        ) {
            let esc = events.iter().any(|ev| {
                matches!(
                    ev,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            });
            if esc {
                self.revert_settings(ctx);
                // A cancelled close confirmation must not leave the next
                // close forced through.
                self.force_close = false;
                self.overlay = Overlay::None;
            }
            return;
        }

        // 3b. Recording a keybinding: the next key press becomes the binding.
        if let Overlay::Keybindings {
            recording: Some(command),
            ..
        } = &self.overlay
        {
            let command = command.clone();
            let mut recorded: Option<String> = None;
            let mut cancelled = false;
            for ev in &events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = ev
                {
                    if *key == egui::Key::Escape {
                        cancelled = true;
                        break;
                    }
                    if let Some(sequence) = crate::editor::chord_string(*key, *modifiers) {
                        recorded = Some(sequence);
                        break;
                    }
                }
            }
            if let Overlay::Keybindings {
                bindings,
                recording,
                error,
            } = &mut self.overlay
            {
                if cancelled {
                    *recording = None;
                } else if let Some(sequence) = recorded {
                    bindings.insert(command.clone(), sequence);
                    *recording = None;
                    // Report a conflict as soon as it is created.
                    let pairs: Vec<(String, String)> = bindings
                        .iter()
                        .map(|(c, s)| (c.clone(), s.clone()))
                        .collect();
                    *error = crate::editor::keymap::conflicts(&pairs)
                        .first()
                        .map(|(seq, cmds)| format!("{seq} is bound to {}", cmds.join(" and ")));
                }
            }
            return;
        }

        // 3b. A buffer with unsaved changes is asking what to do with them.
        // Nothing else in the editor may see the keys while it is up.
        if self.view == View::Editor && self.editor.close_prompt.is_some() {
            for ev in &events {
                if let egui::Event::Key {
                    key, pressed: true, ..
                } = ev
                {
                    match key {
                        egui::Key::Escape => self.editor.close_prompt = None,
                        egui::Key::Enter => self.editor.answer_close(true),
                        egui::Key::D => self.editor.answer_close(false),
                        _ => {}
                    }
                }
            }
            return;
        }

        // 3c. The editor owns the keyboard while it is in front — unless the
        // file tree is asking a question, in which case its text box takes the
        // keys and the buffer must not see them.
        if self.view == View::Editor
            && matches!(self.overlay, Overlay::None)
            && self.editor.tree.prompt.is_none()
        {
            // egui sends a Text event after the Key event for the same press.
            // A key the editor used as a command — `i`, `:`, a motion — must
            // not also insert its own character, so the text that follows a key
            // handled in a mode that does not take text is dropped. In a mode
            // that does take text the key event is not the text, so the text
            // event is the one that inserts.
            let mut swallow_text = false;
            for ev in &events {
                match ev {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        let takes_text = self.editor.mode.takes_text();
                        // The editor gets first refusal; a key it does not use
                        // still reaches the application's own shortcuts.
                        if !self.editor_key(*key, *modifiers, None) {
                            self.app_shortcut(key, *modifiers, ctx);
                        }
                        swallow_text = !takes_text;
                    }
                    egui::Event::Text(text) => {
                        if swallow_text {
                            swallow_text = false;
                        } else {
                            self.editor.text_input(text);
                        }
                    }
                    _ => {}
                }
            }
            return;
        }

        // 3a. Paste guard: Enter pastes, Esc cancels.
        if let Overlay::PasteConfirm { text } = &self.overlay {
            let text = text.clone();
            let mut confirmed = false;
            let mut cancelled = false;
            for ev in &events {
                if let egui::Event::Key {
                    key, pressed: true, ..
                } = ev
                {
                    match key {
                        egui::Key::Enter => confirmed = true,
                        egui::Key::Escape => cancelled = true,
                        _ => {}
                    }
                }
            }
            if confirmed {
                self.overlay = Overlay::None;
                let mode = self
                    .tabs
                    .get(self.active)
                    .map(|t| *t.core.term.mode())
                    .unwrap_or(TermMode::empty());
                self.paste_now(&text, mode);
            } else if cancelled {
                log::info!("paste guard: cancelled");
                self.overlay = Overlay::None;
            }
            return;
        }

        // 3a. System info panel (`:info`): Esc closes it.
        if self.sysinfo.is_some() {
            let esc = events.iter().any(|ev| {
                matches!(
                    ev,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            });
            if esc {
                self.close_sysinfo();
                return;
            }
        }

        // 3b. Search bar: Enter/Shift+Enter step, Esc closes, keys stay in the field.
        if self.search.is_some() {
            let mut close = false;
            let mut step: Option<bool> = None;
            for ev in &events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = ev
                {
                    match key {
                        egui::Key::Escape => close = true,
                        egui::Key::Enter => step = Some(!modifiers.shift),
                        _ => {}
                    }
                }
            }
            if close {
                self.search = None;
            } else if let Some(forward) = step {
                self.search_step(forward);
            } else {
                self.refresh_search();
            }
            return;
        }

        if self.tabs.is_empty() {
            return;
        }

        // 4. Dead tab: Enter closes it, R reconnects.
        if self.tabs[self.active].dead.is_some() {
            let mut enter = false;
            let mut reconnect = false;
            for ev in &events {
                if let egui::Event::Key {
                    key, pressed: true, ..
                } = ev
                {
                    match key {
                        egui::Key::Enter => enter = true,
                        egui::Key::R => reconnect = true,
                        _ => {}
                    }
                }
            }
            let active = self.active;
            if enter {
                self.close_tab(active, ctx, "dead tab + Enter");
            } else if reconnect {
                self.reconnect_tab(active);
            }
            return;
        }

        let mode = *self.tabs[self.active].core.term.mode();
        let mouse_mode = mode.contains(TermMode::MOUSE_MODE);

        for ev in &events {
            match ev {
                egui::Event::ModifiersChanged(m) => {
                    mods = *m;
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    // Easter egg: the Konami code starts party mode.
                    self.track_konami(*key, ctx);
                    if self.app_shortcut(key, *modifiers, ctx) {
                        continue;
                    }
                    if self.scroll_shortcut(key, *modifiers) {
                        ctx.request_repaint();
                        continue;
                    }
                    self.dispatch(input::handle_event(ev, mods, mode), mode, ctx);
                }
                egui::Event::PointerMoved(pos) => {
                    self.on_pointer_moved(*pos, mode, mouse_mode, ctx)
                }
                egui::Event::PointerButton { .. } => {
                    self.on_pointer_button(ev, mode, mouse_mode, ctx)
                }
                egui::Event::MouseWheel { .. } => self.on_wheel(ev, mode, mouse_mode, ctx),
                _ => self.dispatch(input::handle_event(ev, mods, mode), mode, ctx),
            }
        }
    }

    pub(crate) fn dispatch(&mut self, outcome: Outcome, mode: TermMode, ctx: &egui::Context) {
        match outcome {
            Outcome::Bytes(b) => {
                // Easter egg: a magic command typed at the prompt runs an
                // effect instead of reaching the shell.
                if b.len() == 1 && b[0] == b'\r' && self.try_typed_egg(ctx) {
                    return;
                }
                // Tab completes a partially typed internal command.
                if b.len() == 1 && b[0] == b'\t' && self.complete_internal() {
                    return;
                }
                self.last_input = self.now;
                // Typing snaps the viewport back to the live output.
                self.scroll_terminal(Scroll::Bottom);
                self.send_input(b);
            }
            Outcome::Copy => {
                if let Some(text) = self.selection_text()
                    && !text.is_empty()
                {
                    ctx.copy_text(text);
                }
            }
            Outcome::Paste(text) => {
                self.paste_text(&text, mode);
            }
            Outcome::None => {}
        }
    }

    /// Paste text into the active session.
    ///
    /// A multi-line paste that ends in a newline would run its last line the
    /// moment it lands, so it asks first.
    pub(crate) fn paste_text(&mut self, text: &str, mode: TermMode) {
        if paste_needs_confirm(text) {
            log::info!(
                "paste guard: {} lines ending in a newline",
                text.lines().count()
            );
            self.overlay = Overlay::PasteConfirm {
                text: text.to_owned(),
            };
            return;
        }
        self.last_input = self.now;
        self.scroll_terminal(Scroll::Bottom);
        let bytes = input::encode_paste(text, mode.contains(TermMode::BRACKETED_PASTE));
        self.send_input(bytes);
    }

    /// Paste without the guard: the user already confirmed it.
    pub(crate) fn paste_now(&mut self, text: &str, mode: TermMode) {
        self.last_input = self.now;
        self.scroll_terminal(Scroll::Bottom);
        let bytes = input::encode_paste(text, mode.contains(TermMode::BRACKETED_PASTE));
        self.send_input(bytes);
    }

    /// Scroll the active tab's viewport.
    pub(crate) fn scroll_terminal(&mut self, scroll: Scroll) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let rows = tab.core.rows() as f32;
            match scroll {
                Scroll::Delta(n) => tab.core.scroll.wheel(n as f32),
                Scroll::PageUp => tab.core.scroll.wheel(rows),
                Scroll::PageDown => tab.core.scroll.wheel(-rows),
                // A huge target: the terminal clamps at the history's top
                // and the state settles there.
                Scroll::Top => tab.core.scroll.jump(f32::MAX / 4.0),
                Scroll::Bottom => tab.core.scroll.snap(0.0),
            }
        }
    }

    /// Scroll one pane's viewport, without moving the focus. `lines` is
    /// fractional: the easing glides through them.
    pub(crate) fn scroll_pane(&mut self, pane_i: usize, lines: f32) {
        if let Some(tab) = self.tabs.get_mut(self.active)
            && let Some(pane) = tab.panes.get_mut(pane_i)
        {
            pane.core.scroll.wheel(lines);
        }
    }

    // -----------------------------------------------------------------------
    // Scrollback search
    // -----------------------------------------------------------------------

    /// Open (or close) the search bar.
    pub(crate) fn scroll_shortcut(&mut self, key: &egui::Key, mods: egui::Modifiers) -> bool {
        use egui::Key;
        if !mods.shift || mods.ctrl || mods.alt {
            return false;
        }
        let scroll = match key {
            Key::PageUp => Scroll::PageUp,
            Key::PageDown => Scroll::PageDown,
            _ => return false,
        };
        self.scroll_terminal(scroll);
        true
    }

    /// Ctrl+Shift+A: select the whole scrollback and viewport.
    pub(crate) fn select_all(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let history = tab.core.term.grid().history_size();
        let rows = tab.core.rows();
        let cols = tab.core.cols();
        let start = Point::new(Line(-(history as i32)), Column(0));
        let end = Point::new(Line(rows as i32 - 1), Column(cols.saturating_sub(1)));
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        tab.core.term.selection = Some(selection);
    }

    /// Ctrl+Shift+Up/Down: jump to the previous/next prompt mark.
    pub(crate) fn jump_prompt(&mut self, backward: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let top = -(tab.core.term.grid().display_offset() as i32);
        let target = if backward {
            tab.core
                .marks()
                .iter()
                .rev()
                .find(|m| m.kind == MarkKind::Prompt && m.line < top)
                .map(|m| m.line)
        } else {
            tab.core
                .marks()
                .iter()
                .find(|m| m.kind == MarkKind::Prompt && m.line > top)
                .map(|m| m.line)
        };
        if let Some(line) = target {
            tab.core.scroll_to(Point::new(Line(line), Column(0)));
        }
    }

    /// Ctrl+= / Ctrl+- / Ctrl+0 / Ctrl+scroll: terminal font size
    /// (0 = reset to default).
    pub(crate) fn zoom_font(&mut self, delta: f32, ctx: &egui::Context) {
        let new = zoomed_font_size(self.config.font_size, delta);
        if (new - self.config.font_size).abs() < f32::EPSILON {
            return;
        }
        log::info!("font zoom: {:.1} -> {:.1} pt", self.config.font_size, new);
        self.config.font_size = new;
        // Keep an open settings draft in step, so the live-preview comparison
        // does not immediately undo the zoom.
        if let Overlay::Settings { draft, .. } = &mut self.overlay {
            draft.font_size = new;
        }
        let size = Self::raster_size(&self.config, ctx.pixels_per_point());
        let mut atlas = GlyphAtlas::new(&self.font_db, &self.config.font, size);
        atlas.version = next_atlas_version();
        self.atlas = atlas;
    }

    /// Handle app-level shortcuts. Returns true when consumed.
    pub(crate) fn app_shortcut(
        &mut self,
        key: &egui::Key,
        mods: egui::Modifiers,
        ctx: &egui::Context,
    ) -> bool {
        use egui::Key;
        if mods.ctrl || mods.alt {
            log::debug!("shortcut key: {key:?} mods={mods:?}");
        }
        // Fullscreen: F11.
        if matches!(key, Key::F11) && !mods.ctrl && !mods.alt {
            let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            return true;
        }
        if !mods.ctrl || mods.alt {
            return false;
        }
        // Tab cycling: Ctrl+Tab / Ctrl+Shift+Tab.
        if matches!(key, Key::Tab) {
            self.cycle_tab();
            return true;
        }
        if !mods.shift {
            // Terminal font zoom.
            match key {
                Key::Equals | Key::Plus => {
                    self.zoom_font(1.0, ctx);
                    return true;
                }
                Key::Minus => {
                    self.zoom_font(-1.0, ctx);
                    return true;
                }
                Key::Num0 => {
                    self.zoom_font(0.0, ctx);
                    return true;
                }
                _ => return false,
            }
        }
        match key {
            Key::T => {
                self.spawn_local();
                true
            }
            // Ctrl+Shift+E is the editor's split-down key while the editor is
            // in front, so the view switch lives on Ctrl+Shift+O there.
            Key::O => {
                let next = match self.view {
                    View::Terminal => View::Editor,
                    View::Editor => View::Terminal,
                };
                self.set_view(next);
                true
            }
            Key::S => {
                self.toggle_ssh_mode();
                true
            }
            Key::R => {
                self.reopen_closed_tab();
                true
            }
            Key::D => {
                self.split_pane(SplitDir::Right);
                true
            }
            Key::E => {
                self.split_pane(SplitDir::Down);
                true
            }
            Key::W => {
                // iTerm2 semantics: close the focused pane, and the tab when
                // it is the last one.
                self.close_pane(ctx);
                true
            }
            Key::X => {
                // Alias. Some desktops grab Ctrl+Shift+X before the app sees
                // it, so Ctrl+Shift+W is the documented binding.
                self.close_pane(ctx);
                true
            }
            Key::P | Key::K => {
                self.overlay = Overlay::Palette {
                    query: String::new(),
                    selected: 0,
                };
                true
            }
            Key::N => {
                self.overlay = Overlay::Palette {
                    query: "SSH".to_owned(),
                    selected: 0,
                };
                true
            }
            Key::Comma => {
                let draft = self.config.clone();
                let font = self.config.font.clone();
                self.overlay = Overlay::Settings {
                    original: draft.clone(),
                    original_font: font.clone(),
                    draft,
                    font,
                    error: None,
                    known_hosts: crate::config::known_hosts::list(),
                };
                true
            }
            Key::A => {
                self.select_all();
                true
            }
            Key::F => {
                self.toggle_search();
                true
            }
            Key::ArrowUp => {
                self.jump_prompt(true);
                true
            }
            Key::ArrowDown => {
                self.jump_prompt(false);
                true
            }
            Key::PageUp => {
                self.scroll_terminal(Scroll::PageUp);
                true
            }
            Key::PageDown => {
                self.scroll_terminal(Scroll::PageDown);
                true
            }
            Key::Home => {
                self.scroll_terminal(Scroll::Top);
                true
            }
            Key::End => {
                self.scroll_terminal(Scroll::Bottom);
                true
            }
            _ => false,
        }
    }

    /// Palette label for an action, used for the recents list.
    pub(crate) fn action_label(&self, action: &PaletteAction) -> Option<String> {
        ui::palette_items(&self.config)
            .into_iter()
            .find(|(_, a)| std::mem::discriminant(a) == std::mem::discriminant(action))
            .map(|(label, _)| label)
    }

    pub(crate) fn cycle_tab(&mut self) {
        if self.tabs.len() > 1 {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }

    pub(crate) fn run_action(&mut self, action: PaletteAction, ctx: &egui::Context) {
        // Remember what was used, so the palette can surface it first.
        if let Some(label) = self.action_label(&action) {
            self.recent_actions.retain(|l| l != &label);
            self.recent_actions.insert(0, label);
            self.recent_actions.truncate(5);
        }
        match action {
            PaletteAction::NewTerminal => self.spawn_local(),
            PaletteAction::Connect(i) => {
                if let Some(p) = self.config.profiles.get(i).cloned() {
                    self.spawn_ssh(p);
                }
            }
            PaletteAction::ConnectAdhoc => {
                self.overlay = Overlay::Connect {
                    text: String::new(),
                    error: None,
                };
            }
            PaletteAction::CloseTab => {
                let active = self.active;
                self.close_tab(active, ctx, "palette");
            }
            PaletteAction::Settings => {
                let draft = self.config.clone();
                let font = self.config.font.clone();
                self.overlay = Overlay::Settings {
                    original: draft.clone(),
                    original_font: font.clone(),
                    draft,
                    font,
                    error: None,
                    known_hosts: crate::config::known_hosts::list(),
                };
            }
            PaletteAction::Quit => {
                log::info!("palette Quit -> closing the window");
                for tab in &self.tabs {
                    let _ = tab.client_tx.send(ClientMsg::Close);
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            PaletteAction::Party => self.start_easter_egg(EasterEgg::Party, ctx),
            PaletteAction::Matrix => self.start_easter_egg(EasterEgg::Matrix, ctx),
            PaletteAction::Answer => self.start_easter_egg(EasterEgg::Answer, ctx),
            PaletteAction::SystemInfo => self.show_sysinfo(),
            PaletteAction::OpenEditor => self.set_view(View::Editor),
            PaletteAction::OpenTerminal => self.set_view(View::Terminal),
            PaletteAction::ToggleSshMode => self.toggle_ssh_mode(),
            PaletteAction::Sftp => self.open_sftp(),
            PaletteAction::Keybindings => self.open_keybindings(),
            PaletteAction::ViewImage => {
                self.overlay = Overlay::ViewImage {
                    text: String::new(),
                    error: None,
                };
            }
            PaletteAction::ReopenTab => self.reopen_closed_tab(),
            PaletteAction::SplitRight => self.split_pane(SplitDir::Right),
            PaletteAction::SplitDown => self.split_pane(SplitDir::Down),
            PaletteAction::ClosePane => self.close_pane(ctx),
            PaletteAction::EvenColumns => self.even_panes(EvenMode::Columns),
            PaletteAction::EvenRows => self.even_panes(EvenMode::Rows),
            PaletteAction::EvenGrid => self.even_panes(EvenMode::Grid),
        }
    }

    // -----------------------------------------------------------------------
    // Mouse
    // -----------------------------------------------------------------------

    /// Convert a pointer position to viewport cell coordinates.
    pub(crate) fn cell_at(&self, pos: egui::Pos2) -> Option<(u16, u16)> {
        let l = self.pane_layout_at(pos)?;
        let px = pos.x * l.ppp;
        let py = pos.y * l.ppp;
        let col = ((px - l.origin_px[0]) / l.cell_px[0]).floor();
        let row = ((py - l.origin_px[1]) / l.cell_px[1]).floor();
        if col < 0.0 || row < 0.0 || col >= l.cols as f32 || row >= l.rows as f32 {
            return None;
        }
        Some((col as u16, row as u16))
    }

    /// The layout of the pane under a point, if any.
    pub(crate) fn pane_layout_at(&self, pos: egui::Pos2) -> Option<&LayoutInfo> {
        self.pane_layouts
            .iter()
            .find(|(_, l)| l.rect.contains(pos))
            .map(|(_, l)| l)
    }

    pub(crate) fn viewport_point(&self, col: u16, row: u16) -> Point {
        let tab = &self.tabs[self.active];
        // The layout a pointer position was measured against can be one frame
        // older than the grid (a split or a resize shrinks it), so clamp to
        // what the terminal actually has before indexing it.
        let rows = tab.core.rows().max(1);
        let cols = tab.core.cols().max(1);
        let row = (row as usize).min(rows - 1);
        let col = (col as usize).min(cols - 1);
        let offset = tab.core.term.grid().display_offset();
        alacritty_terminal::term::viewport_to_point(offset, Point::new(row, Column(col)))
    }

    /// True when the pointer is over the scrollbar strip at the right edge of
    /// the focused pane.
    pub(crate) fn over_scrollbar(&self, pos: egui::Pos2) -> bool {
        let Some(tab) = self.tabs.get(self.active) else {
            return false;
        };
        let Some((pane_i, l)) = self.pane_layouts.iter().find(|(_, l)| l.rect.contains(pos)) else {
            return false;
        };
        if *pane_i != tab.active_pane {
            return false;
        }
        let history = tab.pane().core.term.grid().history_size();
        history > 0 && pos.x >= l.rect.max.x - 9.0
    }

    /// The OSC 8 hyperlink under a screen position, if any.
    ///
    /// The cell is read from the pane under the pointer, and the row is
    /// clamped to that pane's grid: the layout can be one frame older than the
    /// terminal after a split or a resize.
    pub(crate) fn hyperlink_at(&self, pos: egui::Pos2) -> Option<String> {
        let (pane_i, l) = self
            .pane_layouts
            .iter()
            .find(|(_, l)| l.rect.contains(pos))
            .map(|(i, l)| (*i, l))?;
        let tab = self.tabs.get(self.active)?;
        let pane = tab.panes.get(pane_i)?;
        let px = pos.x * l.ppp;
        let py = pos.y * l.ppp;
        let col = ((px - l.origin_px[0]) / l.cell_px[0]).floor();
        let row = ((py - l.origin_px[1]) / l.cell_px[1]).floor();
        if col < 0.0 || row < 0.0 || col >= l.cols as f32 || row >= l.rows as f32 {
            return None;
        }
        let rows = pane.core.rows().max(1);
        let cols = pane.core.cols().max(1);
        let row = (row as usize).min(rows - 1);
        let col = (col as usize).min(cols - 1);
        let offset = pane.core.term.grid().display_offset();
        let point =
            alacritty_terminal::term::viewport_to_point(offset, Point::new(row, Column(col)));
        let cell = &pane.core.term.grid()[point];
        cell.hyperlink().map(|link| link.uri().to_owned())
    }

    /// Cell-relative x position (0..1) for selection side detection.
    pub(crate) fn sub_cell_x(&self, pos: egui::Pos2) -> f32 {
        let Some(l) = self.pane_layout_at(pos).or(self.layout.as_ref()) else {
            return 0.0;
        };
        (pos.x * l.ppp - l.origin_px[0]) % l.cell_px[0]
    }

    pub(crate) fn on_pointer_button(
        &mut self,
        ev: &egui::Event,
        mode: TermMode,
        mouse_mode: bool,
        ctx: &egui::Context,
    ) {
        let egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers,
        } = ev
        else {
            return;
        };
        let Some(btn) = MouseBtn::from_egui(*button) else {
            return;
        };

        if mouse_mode {
            if let Some((col, row)) = self.cell_at(*pos) {
                let bytes = input::encode_mouse(btn, *pressed, col, row, *modifiers, mode);
                self.send_input(bytes);
                self.mouse_held = if *pressed { Some(btn) } else { None };
            } else if !pressed {
                self.mouse_held = None;
            }
            return;
        }

        match (button, pressed) {
            (egui::PointerButton::Primary, true) => {
                // Clicking a pane focuses it first, so the hyperlink, the
                // selection, and the scrollbar all act on that pane.
                self.focus_pane_at(*pos);
                // Ctrl+click opens an OSC 8 hyperlink.
                if modifiers.ctrl
                    && let Some(uri) = self.hyperlink_at(*pos)
                {
                    ctx.open_url(egui::OpenUrl {
                        url: uri,
                        new_tab: true,
                    });
                    return;
                }
                if self.over_scrollbar(*pos) {
                    return; // the scrollbar owns this strip
                }
                if let Some((col, row)) = self.cell_at(*pos) {
                    let point = self.viewport_point(col, row);
                    let cw = self.layout.as_ref().unwrap().cell_px[0];
                    let side = if self.sub_cell_x(*pos) < cw * 0.5 {
                        Side::Left
                    } else {
                        Side::Right
                    };
                    // Double-click selects the word, triple-click the line.
                    let count = match self.last_click {
                        Some((t, p)) if self.now - t < 0.4 && p.distance(*pos) < 4.0 => {
                            (self.click_count + 1).min(3)
                        }
                        _ => 1,
                    };
                    self.last_click = Some((self.now, *pos));
                    self.click_count = count;
                    let kind = match count {
                        2 => SelectionType::Semantic,
                        3 => SelectionType::Lines,
                        _ => SelectionType::Simple,
                    };
                    let tab = &mut self.tabs[self.active];
                    tab.core.term.selection = Some(Selection::new(kind, point, side));
                    self.selecting = true;
                }
            }
            (egui::PointerButton::Primary, false) => {
                if self.selecting {
                    // X11 copy-on-select: publish to the primary selection.
                    if let Some(text) = self.selection_text() {
                        self.set_primary(&text);
                        // Optionally take the clipboard too, like iTerm2's
                        // "Copy on select".
                        if self.config.copy_on_select {
                            ctx.copy_text(text);
                        }
                    }
                }
                self.selecting = false;
            }
            (egui::PointerButton::Middle, true) => {
                // Middle-click pastes the primary selection (classic X11).
                if let Some(text) = self.read_primary() {
                    self.paste_text(&text, mode);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn on_pointer_moved(
        &mut self,
        pos: egui::Pos2,
        mode: TermMode,
        mouse_mode: bool,
        ctx: &egui::Context,
    ) {
        if mouse_mode {
            if let Some(held) = self.mouse_held
                && let Some((col, row)) = self.cell_at(pos)
            {
                let mods = ctx.input(|i| i.modifiers);
                let bytes = input::encode_mouse_motion(held, col, row, mods, mode);
                self.send_input(bytes);
            }
            return;
        }
        if !self.selecting {
            return;
        }
        if self.over_scrollbar(pos) {
            return;
        }
        if let Some((col, row)) = self.cell_at(pos) {
            let point = self.viewport_point(col, row);
            let cw = self.layout.as_ref().unwrap().cell_px[0];
            let side = if self.sub_cell_x(pos) < cw * 0.5 {
                Side::Left
            } else {
                Side::Right
            };
            let tab = &mut self.tabs[self.active];
            if let Some(sel) = tab.core.term.selection.as_mut() {
                sel.update(point, side);
            }
        }
    }

    pub(crate) fn on_wheel(
        &mut self,
        ev: &egui::Event,
        mode: TermMode,
        mouse_mode: bool,
        ctx: &egui::Context,
    ) {
        let egui::Event::MouseWheel {
            unit,
            delta,
            modifiers,
            ..
        } = ev
        else {
            return;
        };
        if modifiers.ctrl {
            // Ctrl+scroll: terminal font zoom, like most terminals.
            let dir = if delta.y > 0.0 { 1.0 } else { -1.0 };
            self.zoom_font(dir, ctx);
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let Some((col, row)) = self.cell_at(pos) else {
            return;
        };
        let dir_up = delta.y > 0.0;

        if mouse_mode && !modifiers.alt {
            let btn = if dir_up {
                MouseBtn::WheelUp
            } else {
                MouseBtn::WheelDown
            };
            let bytes = input::encode_mouse(btn, true, col, row, *modifiers, mode);
            self.send_input(bytes);
            return;
        }

        if modifiers.alt {
            return;
        }
        // Scroll the pane under the pointer, without moving the focus.
        let cell_h = self
            .pane_layout_at(pos)
            .map(|l| l.cell_px[1])
            .unwrap_or(16.0);
        let lines = match unit {
            egui::MouseWheelUnit::Point => delta.y / cell_h,
            _ => delta.y,
        };
        // Fractional on purpose: the easing glides through the lines, so a
        // burst of events does not jump.
        let lines = lines.abs() * if dir_up { 1.0 } else { -1.0 };
        if lines != 0.0 {
            let pane_i = self
                .pane_layouts
                .iter()
                .find(|(_, l)| l.rect.contains(pos))
                .map(|(i, _)| *i)
                .unwrap_or(self.tabs[self.active].active_pane);
            self.scroll_pane(pane_i, lines);
            ctx.request_repaint();
        }
    }

    // -----------------------------------------------------------------------
    // Prompts
    // -----------------------------------------------------------------------

    pub(crate) fn finish_prompt(&mut self, submit: bool) {
        let Some(prompt) = self.prompts.pop_front() else {
            return;
        };
        if submit {
            let text = self.prompt_text.clone();
            let remember = self.should_remember(&prompt);
            let profile_name = self.tabs.get(prompt.tab).and_then(|t| t.profile.clone());
            if let (Some(profile), true) = (profile_name, remember) {
                match &prompt.kind {
                    PromptKind::Password { .. } => {
                        let _ = secrets::store_password(&profile, &text);
                    }
                    PromptKind::Passphrase { .. } => {
                        let _ = secrets::store_passphrase(&profile, &text);
                    }
                    PromptKind::HostKey { .. } => {}
                    PromptKind::Keyboard { .. } => {}
                }
            }
            let _ = prompt.reply.send(Ok(text));
        } else {
            let _ = prompt.reply.send(Err(()));
        }
        self.prompt_text.clear();
    }

    pub(crate) fn should_remember(&self, prompt: &SshPrompt) -> bool {
        if !self.config.remember_secrets {
            return false;
        }
        match &prompt.kind {
            PromptKind::Password { remember, .. } => *remember,
            PromptKind::Passphrase { remember, .. } => *remember,
            PromptKind::HostKey { .. } => false,
            PromptKind::Keyboard { .. } => false,
        }
    }
}

/// A paste that would run something on its own: several lines, ending in a
/// newline. A single command pasted with its trailing newline is normal and
/// goes straight through.
pub(crate) fn paste_needs_confirm(text: &str) -> bool {
    text.ends_with('\n') && text.trim_end_matches(['\n', '\r']).contains('\n')
}
