//! Easter eggs and internal commands: hidden effects, the Konami code, and the
//! Matrix rain.
//!
//! Commands typed at the prompt are handled by the app itself when they start
//! with a colon (`:info`, `:matrix`, `:party`, `:42`, `:help`): the shell's
//! copy of the line is erased (Ctrl+U on unix, backspaces on Windows) and the
//! command never reaches the shell. Only local panes run them: an SSH pane
//! refuses the line with a notice, the way `:bench` is — the command drives
//! this app, not the remote host. The long `cl2metal --x` forms still work as
//! aliases. The eggs are also reachable from the Konami code and from hidden
//! palette entries.

use super::*;

/// A command handled by the app itself, typed as `:name` at the prompt.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum InternalCmd {
    /// `:info` — the system info panel.
    Info,
    /// `:matrix` — digital rain.
    Matrix,
    /// `:party` — hue-cycling accent.
    Party,
    /// `:42` — the answer.
    Answer,
    /// `:help` — list the internal commands.
    Help,
    /// `:theme <id>` — switch theme.
    Theme(String),
    /// `:font <size>` — set the font size.
    Font(f32),
    /// `:bg <path>` / `:bg off` — set or clear the wallpaper.
    Background(Option<String>),
    /// `:clear` — clear the scrollback.
    Clear,
    /// `:reload` — re-read config.toml.
    Reload,
    /// `:ssh <user@host[:port]>` — connect.
    Ssh(String),
    /// `:even [rows|grid]` — even out the panes.
    Even(EvenMode),
    /// `:bench [secs]` — run the spinning-torus benchmark in this pane.
    Bench(f32),
    /// A command that needs an argument: show how to use it.
    Usage(&'static str),
}

/// The internal commands, for `:help` and the completion popup.
pub(crate) const INTERNAL_COMMANDS: &[(&str, &str)] = &[
    ("info", "system info panel"),
    ("matrix", "digital rain"),
    ("party", "party mode"),
    ("42", "the answer"),
    ("theme", "switch theme"),
    ("font", "font size"),
    ("bg", "wallpaper image"),
    ("clear", "clear the scrollback"),
    ("reload", "re-read config.toml"),
    ("ssh", "connect to a host"),
    ("even", "even out the panes"),
    ("bench", "spinning torus benchmark"),
    ("help", "list these commands"),
];

/// The `:command` word being typed at the end of the line, if it is still the
/// command itself (no arguments yet).
pub(crate) fn typed_command(line: &str) -> Option<&str> {
    let line = line.trim_end();
    let start = line.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
    let rest = line[start..].strip_prefix(':')?;
    if rest.contains(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

/// The byte offset the last `:word` on the line starts at.
///
/// That is where the internal command's own text begins, so the offset also
/// tells how much of the line the shell's editor must erase.
pub(crate) fn internal_command_at(line: &str) -> Option<usize> {
    let line = line.trim_end();
    line.char_indices()
        .filter(|(i, c)| *c == ':' && (*i == 0 || line[..*i].ends_with(char::is_whitespace)))
        .map(|(i, _)| i)
        .next_back()
}

/// How many characters an alias typed at the prompt covers.
///
/// The whole token counts, so a `./cl2metal --party` invocation is erased in
/// one go.
pub(crate) fn alias_erase_len(line: &str, suffix: &str) -> usize {
    let start = line.len() - suffix.len();
    let word_start = line[..start]
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    line[word_start..].chars().count()
}

/// The bytes that erase a typed command from the shell editor's line.
///
/// The unix line editors (readline) kill the line with Ctrl+U. The Windows
/// shells have no equivalent — PSReadLine leaves Ctrl+U unbound and the
/// console's own editor does not know it — so there the line is erased with
/// one backspace per character, which every editor understands.
pub(crate) fn erase_bytes(chars: usize) -> Vec<u8> {
    #[cfg(windows)]
    {
        vec![0x08; chars.min(4096)]
    }
    #[cfg(not(windows))]
    {
        let _ = chars;
        vec![0x15]
    }
}

/// Parse a line typed at the prompt as an internal command.
///
/// The line carries the shell prompt, so the command is looked up as the last
/// `:word` on it, and it must start a word: `:info` matches, `foo:info` does
/// not.
pub(crate) fn parse_internal(line: &str) -> Option<InternalCmd> {
    let line = line.trim_end();
    let start = internal_command_at(line)?;
    let rest = &line[start + 1..];
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((name, args)) => (name, args.trim()),
        None => (rest, ""),
    };
    Some(match name {
        "info" => InternalCmd::Info,
        "matrix" => InternalCmd::Matrix,
        "party" => InternalCmd::Party,
        "42" => InternalCmd::Answer,
        "help" | "?" => InternalCmd::Help,
        "clear" => InternalCmd::Clear,
        "reload" => InternalCmd::Reload,
        "theme" if args.is_empty() => InternalCmd::Usage(":theme <midnight|graphite|paper>"),
        "theme" => InternalCmd::Theme(args.to_owned()),
        "font" => match args.parse::<f32>() {
            Ok(size) => InternalCmd::Font(size),
            Err(_) => InternalCmd::Usage(":font <size in points>"),
        },
        "bg" if args.is_empty() => InternalCmd::Usage(":bg <path> | :bg off"),
        "bg" if args == "off" || args == "none" => InternalCmd::Background(None),
        "bg" => InternalCmd::Background(Some(args.to_owned())),
        "ssh" if args.is_empty() => InternalCmd::Usage(":ssh user@host[:port]"),
        "ssh" => InternalCmd::Ssh(args.to_owned()),
        "even" => InternalCmd::Even(match args {
            "" | "columns" | "cols" => EvenMode::Columns,
            "rows" => EvenMode::Rows,
            "grid" | "tile" | "tiled" => EvenMode::Grid,
            _ => return Some(InternalCmd::Usage(":even [columns|rows|grid]")),
        }),
        "bench" => match args.parse::<f32>() {
            Ok(secs) => InternalCmd::Bench(secs.clamp(1.0, 120.0)),
            Err(_) if args.is_empty() => InternalCmd::Bench(BENCH_SECS),
            Err(_) => return Some(InternalCmd::Usage(":bench [seconds]")),
        },
        _ => return None,
    })
}

/// A hidden effect.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EasterEgg {
    /// Hue-cycle the accent for a few seconds.
    Party,
    /// Digital rain over the terminal.
    Matrix,
    /// The answer.
    Answer,
}

impl EasterEgg {
    /// The palette label for this egg.
    pub(crate) fn label(self) -> &'static str {
        match self {
            EasterEgg::Party => "Party mode",
            EasterEgg::Matrix => "Matrix rain",
            EasterEgg::Answer => "The Answer",
        }
    }
}

impl From<EasterEgg> for PaletteAction {
    fn from(egg: EasterEgg) -> Self {
        match egg {
            EasterEgg::Party => PaletteAction::Party,
            EasterEgg::Matrix => PaletteAction::Matrix,
            EasterEgg::Answer => PaletteAction::Answer,
        }
    }
}

/// The eggs a palette query reveals: they stay hidden until named.
pub(crate) fn hidden_eggs(query: &str) -> Vec<EasterEgg> {
    let mut eggs = Vec::new();
    if query.contains("party") {
        eggs.push(EasterEgg::Party);
    }
    if query.contains("matrix") {
        eggs.push(EasterEgg::Matrix);
    }
    if query == "42" {
        eggs.push(EasterEgg::Answer);
    }
    eggs
}

/// Konami code: up up down down left right left right B A.
const KONAMI: [egui::Key; 10] = [
    egui::Key::ArrowUp,
    egui::Key::ArrowUp,
    egui::Key::ArrowDown,
    egui::Key::ArrowDown,
    egui::Key::ArrowLeft,
    egui::Key::ArrowRight,
    egui::Key::ArrowLeft,
    egui::Key::ArrowRight,
    egui::Key::B,
    egui::Key::A,
];

/// How long party mode lasts.
const PARTY_SECS: f64 = 8.0;
/// How long the rain falls.
const MATRIX_SECS: f64 = 12.0;
/// Glyphs in a rain column's trail.
const MATRIX_TRAIL: usize = 9;
/// How long `:bench` runs when no duration is given.
const BENCH_SECS: f32 = 10.0;

/// A running `:bench` measurement.
///
/// The frames are produced by a child process (`cl2metal --bench-frames`), so a
/// run exercises the whole path a real program takes — child, PTY, reader
/// thread, VT parser, grid, frame build, GPU — while the app measures its own
/// frame intervals and the cost of its `logic()` pass.
pub(crate) struct Bench {
    /// When the measurement started.
    pub(crate) started: f64,
    /// When it ends: the child's run time plus a little slack.
    pub(crate) until: f64,
    /// Time of the previous frame.
    pub(crate) last: f64,
    /// Frames rendered so far.
    pub(crate) frames: u64,
    /// Worst frame interval seen, in ms.
    pub(crate) worst_ms: f32,
    /// Sum of the frame intervals, in ms.
    pub(crate) sum_ms: f64,
    /// Worst `logic()` pass seen, in ms.
    pub(crate) worst_logic_ms: f32,
    /// Sum of the `logic()` passes, in ms.
    pub(crate) sum_logic_ms: f64,
    /// Worst `ui()` pass seen, in ms.
    pub(crate) worst_ui_ms: f32,
    /// Sum of the `ui()` passes, in ms.
    pub(crate) sum_ui_ms: f64,
}

/// The command line that runs the bench child in a pane.
///
/// The child draws one row less than the pane has, so its cursor never scrolls
/// the grid while the animation runs.
pub(crate) fn bench_command_line(exe: &str, secs: f32, cols: usize, rows: usize) -> String {
    let exe = if exe.contains(' ') {
        format!("\"{exe}\"")
    } else {
        exe.to_owned()
    };
    let rows = rows.saturating_sub(1).max(1);
    format!("{exe} --bench-frames {secs:.0} {cols} {rows}")
}

/// Why a typed internal command cannot run in a pane, if it cannot.
///
/// The internal namespace drives this app on this machine, so a remote pane
/// gets the same refusal as `:bench`: the shell on the far end would run
/// something else entirely.
pub(crate) fn internal_refusal(kind: TabKind) -> Option<&'static str> {
    (kind != TabKind::Local).then_some("run this in a local pane")
}

/// Why `:bench` cannot run in a pane, if it cannot.
///
/// The child is this binary, so it only exists on this machine: an SSH pane
/// would run the local path on the remote host.
pub(crate) fn bench_refusal(kind: TabKind, dead: bool) -> Option<&'static str> {
    if dead {
        return Some("bench: no live session in this pane");
    }
    if kind != TabKind::Local {
        return Some("bench: run this in a local pane (the child is this binary)");
    }
    None
}

/// The one-line `:bench` report.
pub(crate) fn bench_summary(bench: &Bench, elapsed: f64) -> String {
    let frames = bench.frames.max(1);
    let fps = bench.frames as f64 / elapsed.max(0.001);
    let mean = bench.sum_ms / frames as f64;
    let mean_logic = bench.sum_logic_ms / frames as f64;
    let mean_ui = bench.sum_ui_ms / frames as f64;
    format!(
        "bench: {} frames in {elapsed:.1}s ({fps:.1} fps), frame {mean:.1}/{:.1} ms, logic {mean_logic:.1}/{:.1} ms, ui {mean_ui:.1}/{:.1} ms",
        bench.frames, bench.worst_ms, bench.worst_logic_ms, bench.worst_ui_ms
    )
}

impl App {
    /// Start a hidden effect.
    pub(crate) fn start_easter_egg(&mut self, egg: EasterEgg, ctx: &egui::Context) {
        match egg {
            EasterEgg::Party => {
                self.party_base = Some((self.theme.ui.accent, self.theme.cursor));
                self.party_until = Some(self.now + PARTY_SECS);
                self.toast = Some(("party mode".to_owned(), self.now));
                log::info!("easter egg: party mode");
            }
            EasterEgg::Matrix => {
                self.matrix_until = Some(self.now + MATRIX_SECS);
                self.matrix_drops.clear();
                log::info!("easter egg: matrix rain");
            }
            EasterEgg::Answer => {
                self.toast = Some((
                    "42 — the answer to life, the universe, and everything".to_owned(),
                    self.now,
                ));
                log::info!("easter egg: the answer");
            }
        }
        ctx.request_repaint();
    }

    /// Feed a key into the Konami-code detector.
    pub(crate) fn track_konami(&mut self, key: egui::Key, ctx: &egui::Context) {
        self.konami.push_back(key);
        while self.konami.len() > KONAMI.len() {
            self.konami.pop_front();
        }
        if self.konami.len() == KONAMI.len() && self.konami.iter().eq(KONAMI.iter()) {
            self.konami.clear();
            self.start_easter_egg(EasterEgg::Party, ctx);
        }
    }

    /// The text typed on the cursor's line, up to the cursor.
    pub(crate) fn input_line(&self) -> Option<String> {
        let tab = self.tabs.get(self.active)?;
        let grid = tab.core.term.grid();
        let cursor = grid.cursor.point;
        let row = &grid[cursor.line];
        let mut text = String::new();
        for col in 0..=cursor.column.0 {
            text.push(row[Column(col)].c);
        }
        Some(text.trim_end().to_owned())
    }

    /// Run a hidden command typed at the prompt.
    ///
    /// Returns true when the line was one: it is cleared and never reaches the
    /// shell. Outside a local pane the line is only refused: the command
    /// drives this app, not the machine on the far end of the pane.
    pub(crate) fn try_typed_egg(&mut self, ctx: &egui::Context) -> bool {
        let Some(line) = self.input_line() else {
            return false;
        };
        let refusal = internal_refusal(self.tabs[self.active].kind);
        // The short internal namespace, handled by the app itself.
        if let Some(cmd) = parse_internal(&line) {
            // Erase the shell's copy of the line, so it never runs it.
            if let Some(start) = internal_command_at(&line) {
                self.erase_pending_line(line[start..].chars().count());
                if let Some(reason) = refusal {
                    let name = line[start + 1..].split_whitespace().next().unwrap_or("");
                    self.toast = Some((format!("{name}: {reason}"), self.now));
                    return true;
                }
            }
            self.run_internal(cmd, ctx);
            return true;
        }
        // Long-form aliases, kept for muscle memory.
        let alias = [
            ("cl2metal --party", EasterEgg::Party),
            ("cl2metal --matrix", EasterEgg::Matrix),
            ("cl2metal --42", EasterEgg::Answer),
        ]
        .into_iter()
        .find(|(suffix, _)| line.ends_with(suffix));
        let Some((suffix, egg)) = alias else {
            return false;
        };
        // Erase the whole token the alias was typed as (`./cl2metal --x`
        // counts as one).
        self.erase_pending_line(alias_erase_len(&line, suffix));
        if let Some(reason) = refusal {
            self.toast = Some((format!("{suffix}: {reason}"), self.now));
            return true;
        }
        self.start_easter_egg(egg, ctx);
        true
    }

    /// Erase the shell editor's pending copy of a typed line.
    fn erase_pending_line(&mut self, chars: usize) {
        self.send_input(erase_bytes(chars));
    }

    /// Run an internal command.
    pub(crate) fn run_internal(&mut self, cmd: InternalCmd, ctx: &egui::Context) {
        match cmd {
            InternalCmd::Info => self.show_sysinfo(),
            InternalCmd::Matrix => self.start_easter_egg(EasterEgg::Matrix, ctx),
            InternalCmd::Party => self.start_easter_egg(EasterEgg::Party, ctx),
            InternalCmd::Answer => self.start_easter_egg(EasterEgg::Answer, ctx),
            InternalCmd::Help => {
                let list: Vec<String> = INTERNAL_COMMANDS
                    .iter()
                    .map(|(name, _)| format!(":{name}"))
                    .collect();
                self.toast = Some((format!("internal: {}", list.join(" ")), self.now));
                log::info!("internal commands listed");
            }
            InternalCmd::Usage(usage) => {
                self.toast = Some((format!("usage: {usage}"), self.now));
            }
            InternalCmd::Theme(name) => {
                let theme = theme::builtin(&name);
                if theme.id != name {
                    self.toast = Some((format!("no theme named {name}"), self.now));
                    return;
                }
                self.config.theme = name.clone();
                let font = self.config.font.clone();
                let config = self.config.clone();
                self.apply_config(config, font, ctx);
                self.persist_config();
                self.toast = Some((format!("theme: {name}"), self.now));
                log::info!("internal: theme -> {name}");
            }
            InternalCmd::Font(size) => {
                let new = size.clamp(6.0, 48.0);
                self.config.font_size = new;
                // Keep an open settings draft in step with the live value.
                if let Overlay::Settings { draft, .. } = &mut self.overlay {
                    draft.font_size = new;
                }
                let size_px = Self::raster_size(&self.config, ctx.pixels_per_point());
                let mut atlas = GlyphAtlas::new(&self.font_db, &self.config.font, size_px);
                atlas.version = next_atlas_version();
                self.atlas = atlas;
                self.persist_config();
                self.toast = Some((format!("font size: {new:.0}pt"), self.now));
                log::info!("internal: font -> {new:.1}pt");
            }
            InternalCmd::Background(path) => match path {
                Some(path) => {
                    let path = expand_tilde(&path);
                    if !std::path::Path::new(&path).is_file() {
                        self.toast = Some((format!("no such image: {path}"), self.now));
                        return;
                    }
                    self.config.background_image = Some(path.clone());
                    self.sync_background(ctx);
                    self.refresh_ui_alpha(ctx);
                    self.persist_config();
                    self.toast = Some((format!("background: {path}"), self.now));
                    log::info!("internal: background -> {path}");
                }
                None => {
                    self.config.background_image = None;
                    self.sync_background(ctx);
                    self.refresh_ui_alpha(ctx);
                    self.persist_config();
                    self.toast = Some(("background cleared".to_owned(), self.now));
                    log::info!("internal: background cleared");
                }
            },
            InternalCmd::Clear => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.core.term.grid_mut().clear_history();
                }
                log::info!("internal: scrollback cleared");
            }
            InternalCmd::Reload => {
                let config = Config::load();
                let font = config.font.clone();
                self.apply_config(config, font, ctx);
                self.toast = Some(("config reloaded".to_owned(), self.now));
                log::info!("internal: config reloaded");
            }
            InternalCmd::Ssh(spec) => match self.connect_adhoc(&spec) {
                Ok(()) => log::info!("internal: ssh {spec}"),
                Err(e) => self.toast = Some((e, self.now)),
            },
            InternalCmd::Even(mode) => self.even_panes(mode),
            InternalCmd::Bench(secs) => self.start_bench(secs, ctx),
        }
    }

    /// `:bench [secs]` — run the spinning-torus benchmark in the focused pane.
    ///
    /// The child is this same binary, so the frames travel the real path: PTY →
    /// reader thread → VT parser → grid → frame build → GPU.
    pub(crate) fn start_bench(&mut self, secs: f32, ctx: &egui::Context) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if let Some(reason) = bench_refusal(tab.kind, tab.dead.is_some()) {
            self.toast = Some((reason.to_owned(), self.now));
            return;
        }
        let (cols, rows) = (tab.core.cols(), tab.core.rows());
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "cl2metal".to_owned());
        let line = bench_command_line(&exe, secs, cols, rows);
        self.bench = Some(Bench {
            started: self.now,
            until: self.now + secs as f64 + 0.5,
            last: self.now,
            frames: 0,
            worst_ms: 0.0,
            sum_ms: 0.0,
            worst_logic_ms: 0.0,
            sum_logic_ms: 0.0,
            worst_ui_ms: 0.0,
            sum_ui_ms: 0.0,
        });
        // `\r` is the Enter a pty expects on every platform: readline maps it
        // back to newline, and the Windows console — where plain `\n` would
        // merely insert a line break into the editor's buffer — only submits
        // on carriage return.
        self.send_input(format!("{line}\r").into_bytes());
        self.toast = Some((format!("bench: {secs:.0}s in {cols}x{rows}"), self.now));
        log::info!("bench: running `{line}` for {secs:.0}s in {cols}x{rows}");
        ctx.request_repaint();
    }

    /// Advance the `:bench` measurement; called once per frame.
    pub(crate) fn measure_bench(&mut self, ctx: &egui::Context, logic: std::time::Duration) {
        let Some(bench) = &mut self.bench else {
            return;
        };
        let ms = ((self.now - bench.last) * 1000.0) as f32;
        let logic_ms = logic.as_secs_f32() * 1000.0;
        bench.last = self.now;
        bench.frames += 1;
        bench.sum_ms += ms as f64;
        bench.worst_ms = bench.worst_ms.max(ms);
        bench.sum_logic_ms += logic_ms as f64;
        bench.worst_logic_ms = bench.worst_logic_ms.max(logic_ms);
        if self.now >= bench.until {
            self.finish_bench();
        } else {
            // Keep the frames coming: the bench measures the app's own cadence
            // while the child floods the PTY.
            ctx.request_repaint();
        }
    }

    /// Record the cost of this frame's `ui()` pass while a bench runs.
    pub(crate) fn record_ui(&mut self, ms: f32) {
        if let Some(bench) = &mut self.bench {
            bench.sum_ui_ms += ms as f64;
            bench.worst_ui_ms = bench.worst_ui_ms.max(ms);
        }
    }

    /// Report the `:bench` result and stop measuring.
    fn finish_bench(&mut self) {
        let Some(bench) = self.bench.take() else {
            return;
        };
        let elapsed = (self.now - bench.started).max(0.001);
        let summary = bench_summary(&bench, elapsed);
        log::info!("{summary}");
        self.toast = Some((summary, self.now));
    }

    /// Commands matching the `:word` being typed, for the completion popup.
    pub(crate) fn internal_matches(&self) -> Vec<(&'static str, &'static str)> {
        let Some(line) = self.input_line() else {
            return Vec::new();
        };
        if internal_refusal(self.tabs[self.active].kind).is_some() {
            return Vec::new();
        }
        let Some(prefix) = typed_command(&line) else {
            return Vec::new();
        };
        // A bare `:` would list everything; wait for a letter.
        if prefix.is_empty() {
            return Vec::new();
        }
        INTERNAL_COMMANDS
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .copied()
            .collect()
    }

    /// Tab-complete a partially typed `:command`. Returns true when the key
    /// was consumed.
    pub(crate) fn complete_internal(&mut self) -> bool {
        let Some(line) = self.input_line() else {
            return false;
        };
        // Completion is part of the internal namespace: over SSH the Tab
        // belongs to the remote shell.
        if internal_refusal(self.tabs[self.active].kind).is_some() {
            return false;
        }
        let Some(prefix) = typed_command(&line) else {
            return false;
        };
        let matches: Vec<&str> = INTERNAL_COMMANDS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| name.starts_with(prefix))
            .collect();
        // Unknown or ambiguous: the shell can have the Tab.
        if matches.len() != 1 {
            return false;
        }
        let name = matches[0];
        let mut bytes = name.as_bytes()[prefix.len()..].to_vec();
        // Commands that take an argument get a trailing space.
        if matches!(name, "theme" | "font" | "bg" | "ssh") {
            bytes.push(b' ');
        }
        self.send_input(bytes);
        true
    }

    /// The `:` completion popup, drawn next to the cursor line.
    pub(crate) fn draw_completions(
        &self,
        ui: &egui::Ui,
        origin_pt: egui::Pos2,
        cell_pt: [f32; 2],
        full: egui::Rect,
    ) {
        if !matches!(self.overlay, Overlay::None)
            || self.search.is_some()
            || !self.prompts.is_empty()
            || !self.last_focused
        {
            return;
        }
        let matches = self.internal_matches();
        if matches.is_empty() {
            return;
        }
        let tab = &self.tabs[self.active];
        let content = tab.core.term.renderable_content();
        let point = content.cursor.point;
        let row = point.line.0 + content.display_offset as i32;
        let x = origin_pt.x + point.column.0 as f32 * cell_pt[0];
        let y = origin_pt.y + row as f32 * cell_pt[1];

        let line_h = 18.0;
        let width = 250.0;
        let height = matches.len() as f32 * line_h + 10.0;
        // Above the cursor line, unless there is no room there.
        let mut top = y - height - 4.0;
        if top < full.min.y + 4.0 {
            top = y + cell_pt[1] + 4.0;
        }
        let left = x.min(full.max.x - width - 8.0).max(full.min.x + 8.0);
        let panel = egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(width, height));
        let painter = ui.painter_at(full);
        painter.rect_filled(
            panel,
            8.0,
            ui::with_alpha(color32(self.theme.ui.panel_bg), 0.96),
        );
        painter.rect_stroke(
            panel,
            8.0,
            egui::Stroke::new(1.0, color32(self.theme.ui.border)),
            egui::StrokeKind::Inside,
        );
        let font = egui::FontId::new(12.0, egui::FontFamily::Monospace);
        for (i, (name, desc)) in matches.iter().enumerate() {
            let row_y = panel.min.y + 5.0 + i as f32 * line_h;
            painter.text(
                egui::pos2(panel.min.x + 10.0, row_y),
                egui::Align2::LEFT_TOP,
                format!(":{name}"),
                font.clone(),
                color32(self.theme.ui.accent),
            );
            painter.text(
                egui::pos2(panel.min.x + 92.0, row_y),
                egui::Align2::LEFT_TOP,
                *desc,
                font.clone(),
                color32(self.theme.ui.text_dim),
            );
        }
    }

    /// Advance the running effects; called once per frame.
    pub(crate) fn update_eggs(&mut self, ctx: &egui::Context) {
        if let Some(until) = self.party_until {
            if self.now >= until {
                self.party_until = None;
                if let Some((accent, cursor)) = self.party_base.take() {
                    self.theme.ui.accent = accent;
                    self.theme.cursor = cursor;
                }
            } else {
                let (accent, cursor) = self
                    .party_base
                    .unwrap_or((self.theme.ui.accent, self.theme.cursor));
                let spin = ((self.now * 240.0) % 360.0) as f32;
                self.theme.ui.accent = hue_rotate(accent, spin);
                self.theme.cursor = hue_rotate(cursor, spin);
                ctx.request_repaint();
            }
        }
        if self.matrix_until.is_some_and(|until| self.now >= until) {
            self.matrix_until = None;
            self.matrix_drops.clear();
        }
    }

    /// Digital rain over the terminal area.
    pub(crate) fn draw_matrix(&mut self, ui: &egui::Ui, rect: egui::Rect, dt: f32) {
        let Some(until) = self.matrix_until else {
            return;
        };
        let cell = 18.0;
        let cols = (rect.width() / cell).floor().max(1.0) as usize;
        if self.matrix_drops.len() != cols {
            self.matrix_drops = (0..cols)
                .map(|i| {
                    let seed = i as f32;
                    [
                        i as f32 * cell,
                        -(seed * 53.0) % rect.height().max(1.0),
                        70.0 + (seed * 29.0) % 150.0,
                    ]
                })
                .collect();
        }
        // Fade the whole thing out over its last second.
        let fade = (((until - self.now) / 1.2).clamp(0.0, 1.0)) as f32;
        let painter = ui.painter_at(rect);
        let font = egui::FontId::new(15.0, egui::FontFamily::Monospace);
        for drop in &mut self.matrix_drops {
            drop[1] += drop[2] * dt;
            if drop[1] > rect.height() + cell * MATRIX_TRAIL as f32 {
                drop[1] = -cell * MATRIX_TRAIL as f32;
            }
            let x = rect.min.x + drop[0];
            for step in 0..MATRIX_TRAIL {
                let y = rect.min.y + drop[1] - step as f32 * cell;
                if y < rect.min.y - cell || y > rect.max.y {
                    continue;
                }
                let alpha = (1.0 - step as f32 / MATRIX_TRAIL as f32).powi(2) * 0.85 * fade;
                if alpha <= 0.01 {
                    continue;
                }
                let ch = matrix_char(drop[0] as usize, step, self.now);
                painter.text(
                    egui::pos2(x, y),
                    egui::Align2::LEFT_TOP,
                    ch,
                    font.clone(),
                    egui::Color32::from_rgba_unmultiplied(110, 255, 140, (alpha * 255.0) as u8),
                );
            }
        }
        ui.ctx().request_repaint();
    }
}

/// Rotate a colour's hue by `degrees`, keeping saturation and value.
pub(crate) fn hue_rotate(rgb: [u8; 3], degrees: f32) -> [u8; 3] {
    let (r, g, b) = (
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let chroma = max - min;
    let mut hue = if chroma == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / chroma) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / chroma + 2.0)
    } else {
        60.0 * ((r - g) / chroma + 4.0)
    };
    if hue < 0.0 {
        hue += 360.0;
    }
    hue = (hue + degrees).rem_euclid(360.0);
    let sat = if max == 0.0 { 0.0 } else { chroma / max };
    let val = max;

    let c = val * sat;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = val - c;
    let (r2, g2, b2) = match (hue / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r2 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g2 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b2 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

/// A deterministic glyph for the rain: same cell and frame, same character.
pub(crate) fn matrix_char(col: usize, step: usize, now: f64) -> char {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789@#$%&*+=<>/|";
    let seed = (col as u64).wrapping_mul(2_654_435_761)
        ^ (step as u64).wrapping_mul(40_503)
        ^ ((now * 6.0) as u64);
    CHARS[(seed % CHARS.len() as u64) as usize] as char
}
