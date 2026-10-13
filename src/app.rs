//! Application state: tabs, input routing, session plumbing, and layout.

use crate::config::{AuthMethod, Config, CursorShape, SshProfile, secrets};
use crate::render::{
    self, AtlasGpu, CursorOpts, RendererCore, TerminalCallback,
    glyphs::{self, GlyphAtlas},
};
use crate::term::{
    ClientMsg, Listener, MarkKind, PromptMark, SessionMsg, TermCore, Waker,
    input::{self, MouseBtn, Outcome},
    local, ssh,
    ssh::SshParams,
};
use crate::theme::{self, Theme};
use crate::ui::{self, BAR_H, Overlay, PaletteAction, PromptKind, SshPrompt, color32};
use alacritty_terminal::{
    event::WindowSize,
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{TermMode, search::RegexSearch},
};
use eframe::egui;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TabKind {
    Local,
    Ssh,
}

/// Which surface the window is showing.
///
/// Both stay alive across a switch: the terminal keeps its sessions and the
/// editor keeps its buffers and unsaved work.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum View {
    Terminal,
    Editor,
}

/// One session inside a tab: a terminal, its PTY/SSH plumbing, and its state.
///
/// A tab holds at least one pane; splitting adds more, each with its own
/// session, scrollback, and title.
pub struct Pane {
    pub kind: TabKind,
    pub profile: Option<String>,
    /// Full SSH profile, so ad-hoc sessions can reconnect too.
    pub ssh_profile: Option<SshProfile>,
    /// The local shell's process id, so the editor's tree can follow its
    /// `cd`. SSH panes have none: the directory lives on the remote host.
    pub shell_pid: Option<u32>,
    pub core: TermCore,
    pub client_tx: flume::Sender<ClientMsg>,
    pub msg_rx: flume::Receiver<SessionMsg>,
    pub term_rx: flume::Receiver<SessionMsg>,
    pub dead: Option<String>,
    /// SSH: true once output arrived (connection established).
    pub ready: bool,
    /// Cursor blinking requested by the application (DECSCUSR / mode 12).
    pub cursor_blink: bool,
    /// Session transcript, when logging is enabled.
    pub log: Option<std::fs::File>,
    /// When to retry a dropped SSH session.
    pub reconnect_at: Option<f64>,
    /// Automatic reconnect attempts since the last successful output.
    pub reconnect_attempts: u8,
    /// Title reported by the application (OSC 0/2).
    pub osc_title: Option<String>,
    /// Prompt marks already inspected, for the command-finished notification.
    pub marks_seen: usize,
    /// When the running command started (OSC 133 `C` mark), for the
    /// "finished after Ns" notification.
    pub command_started_at: Option<f64>,
}

/// How a tab's area is divided between its panes.
pub enum PaneNode {
    Leaf(usize),
    Split {
        dir: SplitDir,
        /// Fraction of the area given to `a` (0.15..=0.85).
        ratio: f32,
        a: Box<PaneNode>,
        b: Box<PaneNode>,
    },
}

/// Split orientation: side by side, or stacked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitDir {
    /// Side by side, with a vertical divider.
    Right,
    /// Stacked, with a horizontal divider.
    Down,
}

/// How to even out a tab's panes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EvenMode {
    /// All side by side, equal width.
    Columns,
    /// All stacked, equal height.
    Rows,
    /// A grid, as square as possible.
    Grid,
}

/// A chain of splits where every child ends up the same size: the first takes
/// `1/n` of the area and the rest share what is left.
fn chain_nodes(nodes: Vec<PaneNode>, dir: SplitDir) -> PaneNode {
    let mut iter = nodes.into_iter().rev();
    let mut node = iter.next().expect("a chain needs at least one node");
    for (i, prev) in iter.enumerate() {
        // `i + 1` nodes are already inside `node`, so this subtree holds `i + 2`.
        let total = (i + 2) as f32;
        node = PaneNode::Split {
            dir,
            ratio: 1.0 / total,
            a: Box::new(prev),
            b: Box::new(node),
        };
    }
    node
}

impl PaneNode {
    /// Pane indices in draw order.
    pub fn leaves(&self) -> Vec<usize> {
        match self {
            PaneNode::Leaf(i) => vec![*i],
            PaneNode::Split { a, b, .. } => {
                let mut out = a.leaves();
                out.extend(b.leaves());
                out
            }
        }
    }

    /// Replace the leaf `target` with a split of itself and `new`.
    pub fn split_leaf(&mut self, target: usize, new: usize, dir: SplitDir) -> bool {
        match self {
            PaneNode::Leaf(i) if *i == target => {
                let old = PaneNode::Leaf(target);
                *self = PaneNode::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(old),
                    b: Box::new(PaneNode::Leaf(new)),
                };
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { a, b, .. } => {
                a.split_leaf(target, new, dir) || b.split_leaf(target, new, dir)
            }
        }
    }

    /// Drop the leaf `target`, collapsing its parent split into the sibling.
    pub fn remove_leaf(&mut self, target: usize) -> bool {
        match self {
            PaneNode::Leaf(_) => false,
            PaneNode::Split { a, b, .. } => {
                let a_hit = matches!(**a, PaneNode::Leaf(i) if i == target);
                let b_hit = matches!(**b, PaneNode::Leaf(i) if i == target);
                if a_hit {
                    *self = std::mem::replace(b, PaneNode::Leaf(0));
                    return true;
                }
                if b_hit {
                    *self = std::mem::replace(a, PaneNode::Leaf(0));
                    return true;
                }
                a.remove_leaf(target) || b.remove_leaf(target)
            }
        }
    }

    /// Renumber leaves through `map` (old index -> new index).
    pub fn remap(&mut self, map: &[Option<usize>]) {
        match self {
            PaneNode::Leaf(i) => {
                if let Some(new) = map.get(*i).copied().flatten() {
                    *i = new;
                }
            }
            PaneNode::Split { a, b, .. } => {
                a.remap(map);
                b.remap(map);
            }
        }
    }

    /// Rebuild the tree so every pane gets an equal share of the area.
    ///
    /// Splitting halves the focused pane, which is what every terminal does;
    /// this is the explicit "even out" (tmux's `even-horizontal`,
    /// `even-vertical`, and `tiled` layouts).
    pub fn even_out(&mut self, mode: EvenMode) {
        let leaves = self.leaves();
        if leaves.len() <= 1 {
            return;
        }
        *self = match mode {
            EvenMode::Columns => chain_nodes(
                leaves.into_iter().map(PaneNode::Leaf).collect(),
                SplitDir::Right,
            ),
            EvenMode::Rows => chain_nodes(
                leaves.into_iter().map(PaneNode::Leaf).collect(),
                SplitDir::Down,
            ),
            EvenMode::Grid => {
                // As square as possible: rows of `cols` panes, equal heights.
                let cols = (leaves.len() as f32).sqrt().ceil().max(1.0) as usize;
                let rows: Vec<PaneNode> = leaves
                    .chunks(cols)
                    .map(|row| {
                        chain_nodes(
                            row.iter().copied().map(PaneNode::Leaf).collect(),
                            SplitDir::Right,
                        )
                    })
                    .collect();
                chain_nodes(rows, SplitDir::Down)
            }
        };
    }
}

pub struct Tab {
    /// Sessions in this tab, at least one.
    pub panes: Vec<Pane>,
    /// How the panes divide the terminal area.
    pub tree: PaneNode,
    /// The focused pane.
    pub active_pane: usize,
    pub base_title: String,
    /// User-set title (double-click a tab to rename).
    pub title_override: Option<String>,
    /// When the header started its entry animation (stamped on first draw, so
    /// the first frame after the tab opens always animates).
    pub opened_at: Option<f64>,
    /// When the tab was closed: the header shrinks out for `TAB_ANIM_SECS`
    /// before it is dropped, like Tabby's `:leave` transition.
    pub closing_at: Option<f64>,
    /// Output arrived while this tab was in the background.
    pub unread: bool,
}

impl Tab {
    /// The focused pane.
    pub fn pane(&self) -> &Pane {
        &self.panes[self.active_pane.min(self.panes.len() - 1)]
    }

    /// The focused pane, mutably.
    pub fn pane_mut(&mut self) -> &mut Pane {
        let i = self.active_pane.min(self.panes.len() - 1);
        &mut self.panes[i]
    }

    pub fn display_title(&self) -> &str {
        if let Some(title) = &self.title_override {
            return title;
        }
        match &self.pane().osc_title {
            Some(t) if !t.is_empty() => t,
            _ => &self.base_title,
        }
    }
}

/// Field access on a tab goes to its focused pane, so the session plumbing
/// reads the same whether a tab has one pane or several.
impl std::ops::Deref for Tab {
    type Target = Pane;
    fn deref(&self) -> &Pane {
        self.pane()
    }
}

impl std::ops::DerefMut for Tab {
    fn deref_mut(&mut self) -> &mut Pane {
        self.pane_mut()
    }
}

pub struct LayoutInfo {
    pub rect: egui::Rect,
    pub origin_px: [f32; 2],
    pub cell_px: [f32; 2],
    pub ppp: f32,
    pub cols: usize,
    pub rows: usize,
}

impl Clone for LayoutInfo {
    fn clone(&self) -> Self {
        Self {
            rect: self.rect,
            origin_px: self.origin_px,
            cell_px: self.cell_px,
            ppp: self.ppp,
            cols: self.cols,
            rows: self.rows,
        }
    }
}

/// Monotonic source of unique atlas versions (forces full GPU re-upload).
static NEXT_ATLAS_VERSION: AtomicU64 = AtomicU64::new(1);

/// Startup animation length, in seconds.
const ENTRY_SECS: f64 = 0.62;

/// Minimum time the startup overlay stays up, so it never just flashes.
const MIN_ENTRY_SECS: f64 = 0.7;

/// Stop waiting for the first terminal output after this long.
const MAX_ENTRY_WAIT: f64 = 8.0;

/// Rough time the whole startup takes, used to keep the progress bar moving
/// while the wallpaper decodes.
const LOAD_ESTIMATE_SECS: f32 = 2.6;

/// How long the finished loading screen is held before it fades out, so the
/// wallpaper and the full bar are actually seen.
const READY_HOLD_SECS: f64 = 0.4;

/// Tab enter/leave animation length, matching Tabby's 250ms tab transition.
const TAB_ANIM_SECS: f64 = 0.25;

/// Only notify about commands that ran at least this long.
const NOTIFY_MIN_SECS: f64 = 5.0;

/// Shortest gap between two UI wake-ups from session output, in milliseconds.
///
/// A streaming program can produce thousands of chunks a second; without a cap
/// the UI thread would render as fast as it can and starve input.
const WAKE_MIN_MS: u64 = 8;

/// Milliseconds since the process started.
fn now_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

/// The callback session threads use to ask for a frame.
///
/// Throttled to `WAKE_MIN_MS`, but the tail of a burst is never dropped: the
/// throttled path schedules a repaint instead of skipping it.
fn make_waker(ctx: &egui::Context) -> Waker {
    let ctx = ctx.clone();
    let last = std::sync::Arc::new(AtomicU64::new(0));
    std::sync::Arc::new(move || {
        let now = now_ms();
        let prev = last.load(Ordering::Relaxed);
        if now.saturating_sub(prev) >= WAKE_MIN_MS {
            last.store(now, Ordering::Relaxed);
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(WAKE_MIN_MS));
        }
    })
}

/// Ease-out cubic, for the startup animation.
fn ease_out_cubic(x: f32) -> f32 {
    let inv = 1.0 - x.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

/// Ease-in-out cubic, for the tab leave animation (Tabby uses `ease-in-out`).
fn ease_in_out_cubic(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        4.0 * x * x * x
    } else {
        1.0 - (-2.0 * x + 2.0).powi(3) / 2.0
    }
}

/// Tab width during the enter animation.
///
/// Tabby animates `flex-basis`/`width` from 1px to the tab's full size, so the
/// header is revealed by its own growing box.
fn tab_enter_width(full: f32, progress: f32) -> f32 {
    1.0 + (full - 1.0) * progress.clamp(0.0, 1.0)
}

/// Tab width during the leave animation: the full size shrinking to nothing.
fn tab_leave_width(full: f32, progress: f32) -> f32 {
    full * (1.0 - progress.clamp(0.0, 1.0))
}

fn next_atlas_version() -> u64 {
    NEXT_ATLAS_VERSION.fetch_add(1, Ordering::Relaxed)
}

pub struct App {
    config: Config,
    theme: Theme,
    font_db: fontdb::Database,
    atlas: GlyphAtlas,
    atlas_gpu: std::sync::Arc<AtlasGpu>,
    renderer: std::sync::Arc<RendererCore>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// GPU name and backend, shown by the `:info` panel.
    gpu: String,
    /// How session threads ask for a frame when output arrives.
    waker: Waker,

    tabs: Vec<Tab>,
    active: usize,
    layout: Option<LayoutInfo>,
    /// Per-pane layouts of the active tab, rebuilt every frame.
    pane_layouts: Vec<(usize, LayoutInfo)>,
    /// The divider currently being dragged, as a tree path.
    divider_drag: Option<Vec<bool>>,

    overlay: Overlay,
    prompts: VecDeque<SshPrompt>,
    prompt_text: String,

    selecting: bool,
    mouse_held: Option<MouseBtn>,
    /// Last click time/position and run length, for double/triple-click selection.
    last_click: Option<(f64, egui::Pos2)>,
    click_count: u8,
    last_focused: bool,
    last_title: String,
    /// When the editor's tree last followed the terminal's directory.
    last_root_follow: f64,
    toast: Option<(String, f64)>,
    now: f64,
    /// Wall-clock time of the first frame; drives the startup animation.
    entry_start: Option<f64>,
    /// Eased 0..=1 progress of the startup animation (1.0 = overlay gone).
    entry_t: f32,
    /// When the startup overlay started fading out (None = still loading).
    entry_fade_at: Option<f64>,
    /// Smoothed 0..=1 loading progress shown by the startup overlay.
    load_progress: f32,
    /// Time of the previous frame, for time-based smoothing.
    last_now: f64,
    /// When the first terminal output arrived: the app is ready to be shown.
    ready_at: Option<f64>,
    /// Cursor visible this frame (blink phase).
    cursor_on: bool,
    /// Last time the user typed, so the cursor stays lit while typing.
    last_input: f64,
    /// Time of the most recent BEL, for the visual bell.
    bell_at: Option<f64>,
    /// System clipboard, created on first use.
    clipboard: Option<arboard::Clipboard>,
    /// Scrollback search bar state (`None` = closed).
    search: Option<SearchState>,
    /// Recently used palette entries, most recent first.
    recent_actions: Vec<String>,
    /// `--exec` command, consumed by the first local tab.
    exec: Option<String>,
    /// Window rect from the last frame, saved on exit.
    last_window_rect: Option<egui::Rect>,
    /// Background image, loaded onto the GPU (None = plain theme background).
    background: Option<ImageGpu>,
    /// Path currently being decoded on the loader thread.
    background_loading: Option<String>,
    /// Result channel of the loader thread.
    background_rx: Option<flume::Receiver<BackgroundPixels>>,
    /// When the wallpaper became ready, so it can fade in.
    background_at: Option<f64>,
    /// When the loading bar first reached 100%, so the overlay can hold for a
    /// beat and let the finished state be seen.
    all_ready_at: Option<f64>,
    /// Easter egg: party mode hue-cycles the accent until this time.
    party_until: Option<f64>,
    /// Accent and cursor colours to restore when party mode ends.
    party_base: Option<([u8; 3], [u8; 3])>,
    /// Easter egg: the Matrix rain runs until this time.
    matrix_until: Option<f64>,
    /// Rain columns: `[x, y, speed]`.
    matrix_drops: Vec<[f32; 3]>,
    /// Rolling window of recent keys, for the Konami code.
    konami: VecDeque<egui::Key>,
    /// System info panel (`:info`), when open.
    sysinfo: Option<sysinfo::SysInfo>,
    /// When the panel opened, so it can fade in.
    sysinfo_at: Option<f64>,
    /// When the panel's numbers were last re-read.
    sysinfo_refresh_at: f64,
    /// This process's CPU time and the wall clock when it was sampled.
    cpu_sample: Option<(u64, f64)>,
    /// CPU use since the last sample, as a percentage of one core.
    cpu_percent: Option<f32>,
    /// Recently closed tabs, newest last, for `Ctrl+Shift+R`.
    closed: Vec<ClosedTab>,
    /// Seconds since the previous frame, for time-based animation.
    frame_dt: f32,
    /// A running `:bench` measurement (`None` = not benching).
    bench: Option<eggs::Bench>,
    /// Which surface is in front.
    view: View,
    /// The SSH workspace sidebar.
    ssh_mode: bool,
    /// The sidebar's search box.
    host_query: String,
    /// The sidebar's scroll offset.
    ssh_scroll: f32,
    /// The built-in editor.
    editor: crate::editor::Editor,
    /// The SFTP browser, when a host's files are open.
    sftp: Option<sftp_panel::SftpPanel>,
}

/// A tab that was closed, kept so `Ctrl+Shift+R` can bring the session back.
pub(crate) struct ClosedTab {
    pub(crate) kind: TabKind,
    pub(crate) profile: Option<String>,
    pub(crate) ssh_profile: Option<SshProfile>,
    pub(crate) title_override: Option<String>,
    pub(crate) base_title: String,
}

/// How many closed tabs are remembered for reopening.
const CLOSED_CAP: usize = 10;

/// RGBA8 pixels plus their size, as decoded by the background loader thread.
type BackgroundPixels = anyhow::Result<(Vec<u8>, u32, u32)>;

/// A decoded background image living on the GPU.
struct ImageGpu {
    /// Path it was loaded from, so a config change can be detected.
    path: String,
    size: [u32; 2],
    texture: egui::TextureHandle,
}

/// State of the scrollback search bar.
pub struct SearchState {
    pub query: String,
    /// Query the collected matches were compiled for.
    compiled_for: String,
    regex: Option<RegexSearch>,
    error: Option<String>,
    matches: Vec<std::ops::RangeInclusive<Point>>,
    current: usize,
}

/// Upper bound on collected matches, to keep a broad query cheap.
const SEARCH_CAP: usize = 2000;

/// Longest edge a background image is kept at, to bound GPU memory.
const MAX_BACKGROUND_PX: u32 = 2560;

/// Command-line options.
#[derive(Default, Clone)]
pub struct Cli {
    /// Connect to this saved profile on startup.
    pub profile: Option<String>,
    /// Ad-hoc `user@host[:port]` to connect to on startup.
    pub host: Option<String>,
    /// Command to run in the first local tab instead of an interactive shell.
    pub exec: Option<String>,
    /// Open this file in the editor on startup.
    pub edit: Option<String>,
}

impl Cli {
    /// Parse `cl2metal [--profile NAME] [--exec CMD] [--edit FILE] [user@host]`.
    pub fn parse() -> Self {
        let mut cli = Cli::default();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--profile" | "-p" => cli.profile = args.next(),
                "--exec" | "-e" => cli.exec = args.next(),
                "--edit" => cli.edit = args.next(),
                "--help" | "-h" => {
                    println!(
                        "cl2metal — terminal + SSH client\n\n\
                         USAGE:\n  cl2metal [OPTIONS] [user@host[:port]]\n\n\
                         OPTIONS:\n  -p, --profile NAME  connect to a saved profile\n  \
                         -e, --exec CMD      run CMD in the first tab\n  \
                         --edit FILE         open FILE in the editor\n  \
                         -h, --help          show this help"
                    );
                    std::process::exit(0);
                }
                other if !other.starts_with('-') => cli.host = Some(other.to_owned()),
                _ => {}
            }
        }
        cli
    }
}

/// Actions offered by the terminal's right-click menu.
#[derive(Clone, Copy)]
pub(crate) enum MenuAction {
    Copy,
    Paste,
    SelectAll,
    Search,
    ClearScrollback,
    RenameTab,
    NewTab,
    CloseTab,
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

// Submodules: the application layer is split by responsibility.
mod background;
mod clipboard;
mod editor_view;
pub(crate) mod eggs;
mod events;
mod search;
mod session;
mod sftp_panel;
mod ssh_mode;
mod sysinfo;
mod view;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ssh_tests;
impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, cli: Cli) -> Self {
        let config = Config::load();
        let mut theme = theme::builtin(&config.theme);
        theme::apply_overrides(&mut theme, &config.colors);
        ui::apply_theme(&cc.egui_ctx, &theme);
        // Ctrl+= / Ctrl+- / Ctrl+0 and Ctrl+scroll zoom the terminal font, not
        // the egui chrome, so egui's own keyboard zoom stays off.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);

        // Ship the shell-integration hooks (OSC 133) for bash/zsh.
        #[cfg(unix)]
        if let Err(e) = crate::term::integration::ensure_scripts() {
            log::warn!("shell integration scripts unavailable: {e}");
        }

        let font_db = glyphs::load_font_db();
        let ppp = cc.egui_ctx.pixels_per_point();
        let size_px = Self::raster_size(&config, ppp);
        let atlas = GlyphAtlas::new(&font_db, &config.font, size_px);
        // A shell can print its first prompt before the first egui frame. Give
        // it the restored viewport's actual grid up front so the first frame
        // does not resize a live terminal and move that prompt down.
        let initial_layout = Self::initial_layout(&config, atlas.metrics(), ppp);

        let state = cc
            .wgpu_render_state
            .as_ref()
            .expect("cl2metal requires the wgpu backend");
        let renderer = RendererCore::new(state);
        // Version 0 forces the first sync to upload the whole atlas.
        let atlas_gpu = std::sync::Arc::new(AtlasGpu::new(&state.device, 2048, 2048, 0));
        let device = state.device.clone();
        let queue = state.queue.clone();
        let adapter = state.adapter.get_info();
        let gpu = format!("{} ({:?})", adapter.name, adapter.backend);
        log::info!("gpu: {gpu}");
        let waker = make_waker(&cc.egui_ctx);

        // The editor is built before the struct literal because `config` moves
        // into it.
        let editor = {
            let settings = config.editor.to_settings();
            let keymap = crate::editor::Keymap::from_config(&config.editor_keys);
            let root = config
                .editor_root
                .as_deref()
                .map(|s| crate::editor::expand(std::path::Path::new(s)))
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("/"));
            let mut editor = crate::editor::Editor::new(settings, keymap, root);
            editor.tree.open = config.editor.tree_open;
            editor.tree.width = config.editor.tree_width;
            editor
        };
        let ssh_mode = config.ssh_mode;
        let start_view = if config.view == "editor" {
            View::Editor
        } else {
            View::Terminal
        };

        let mut app = Self {
            config,
            theme,
            font_db,
            atlas,
            atlas_gpu,
            renderer,
            device,
            queue,
            gpu,
            waker,
            tabs: Vec::new(),
            active: 0,
            layout: Some(initial_layout),
            pane_layouts: Vec::new(),
            divider_drag: None,
            overlay: Overlay::None,
            prompts: VecDeque::new(),
            prompt_text: String::new(),
            selecting: false,
            mouse_held: None,
            last_click: None,
            click_count: 0,
            last_focused: true,
            last_title: String::new(),
            last_root_follow: 0.0,
            toast: None,
            now: 0.0,
            entry_start: None,
            entry_t: 0.0,
            entry_fade_at: None,
            load_progress: 0.0,
            last_now: 0.0,
            ready_at: None,
            cursor_on: true,
            last_input: 0.0,
            bell_at: None,
            clipboard: None,
            search: None,
            recent_actions: Vec::new(),
            exec: cli.exec.clone(),
            last_window_rect: None,
            background: None,
            background_loading: None,
            background_rx: None,
            background_at: None,
            all_ready_at: None,
            party_until: None,
            party_base: None,
            matrix_until: None,
            matrix_drops: Vec::new(),
            konami: VecDeque::new(),
            sysinfo: None,
            sysinfo_at: None,
            sysinfo_refresh_at: 0.0,
            cpu_sample: None,
            cpu_percent: None,
            closed: Vec::new(),
            frame_dt: 0.0,
            bench: None,
            view: start_view,
            ssh_mode,
            host_query: String::new(),
            ssh_scroll: 0.0,
            editor,
            sftp: None,
        };
        log::info!("app constructed; spawning the first tab");
        app.sync_background(&cc.egui_ctx);
        app.refresh_ui_alpha(&cc.egui_ctx);
        app.spawn_local();
        // Startup connection requested on the command line.
        if let Some(name) = cli.profile {
            match app.config.profiles.iter().find(|p| p.name == name).cloned() {
                Some(profile) => app.spawn_ssh(profile),
                None => {
                    app.toast = Some((format!("no profile named {name}"), app.now));
                }
            }
        } else if let Some(host) = cli.host
            && let Err(e) = app.connect_adhoc(&host)
        {
            app.toast = Some((e, app.now));
        }
        // A file requested on the command line opens in the editor.
        if let Some(path) = cli.edit {
            let path = crate::editor::expand(std::path::Path::new(&path));
            app.editor.open_path(&path);
            app.view = View::Editor;
            log::info!("opened {} in the editor", path.display());
        }
        app
    }

    /// Save the config, logging (never failing the command) on error.
    pub(crate) fn persist_config(&self) {
        if let Err(e) = self.config.save() {
            log::warn!("saving config: {e:#}");
        }
    }

    fn raster_size(config: &Config, ppp: f32) -> u16 {
        (config.font_size * ppp).round().clamp(6.0, 160.0) as u16
    }

    fn grid_size(&self) -> (usize, usize) {
        match &self.layout {
            Some(l) => (l.cols, l.rows),
            None => (100, 30),
        }
    }

    /// Estimate the first terminal grid from the configured viewport. This is
    /// replaced by the exact egui content rect on the first frame, but avoids
    /// an initial 100×30 PTY followed by a resize after the shell has already
    /// produced a prompt.
    fn initial_layout(config: &Config, metrics: glyphs::CellMetrics, ppp: f32) -> LayoutInfo {
        let [width, height] = config.window_size.unwrap_or([1100.0, 720.0]);
        let rect = egui::Rect::from_min_size(
            egui::pos2(0.0, BAR_H),
            egui::vec2(width.max(480.0), (height - BAR_H).max(1.0)),
        );
        let cell_pt = [metrics.cell_w / ppp, metrics.cell_h / ppp];
        let inner = rect.shrink2(egui::vec2(config.padding[1], config.padding[0]));
        let cols = ((inner.width() / cell_pt[0]).floor() as usize).max(2);
        let rows = ((inner.height() / cell_pt[1]).floor() as usize).max(1);
        let used = egui::vec2(cols as f32 * cell_pt[0], rows as f32 * cell_pt[1]);
        let origin = egui::pos2(
            inner.min.x + (inner.width() - used.x) * 0.5,
            inner.min.y + (inner.height() - used.y) * 0.5,
        );
        LayoutInfo {
            rect,
            origin_px: [origin.x * ppp, origin.y * ppp],
            cell_px: [metrics.cell_w, metrics.cell_h],
            ppp,
            cols,
            rows,
        }
    }

    // -----------------------------------------------------------------------
    // Session spawning
    // -----------------------------------------------------------------------

    /// Open a transcript file for a new tab, when session logging is on.
    fn sync_atlas(&mut self) -> std::sync::Arc<wgpu::TextureView> {
        let (pixels, w, h, version, dirty) = self.atlas.upload_data();
        let gpu_version = self.atlas_gpu.inner.lock().unwrap().version;
        if gpu_version != version {
            log::debug!(
                "atlas upload: gpu v{gpu_version} -> cpu v{version}, {w}x{h}, {} bytes",
                pixels.len()
            );
        }
        self.atlas_gpu
            .sync(&self.device, &self.queue, pixels, w, h, version, dirty)
    }
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let logic_started = std::time::Instant::now();
        self.now = ctx.input(|i| i.time);
        let dt = (self.now - self.last_now).clamp(0.0, 0.25);
        self.last_now = self.now;
        self.frame_dt = dt as f32;
        // Session output and input events request their own repaints; the
        // timers below only cover the animations that need a wake-up.
        self.drain(ctx);
        // Tabs whose leave animation has run out are dropped here.
        self.sweep_closing_tabs(ctx);
        // A finished wallpaper decode lands here (it never blocks a frame).
        self.poll_background(ctx);
        // SFTP events, and any remote file they finished reading.
        self.poll_sftp();
        // Easter eggs that are running (party mode, the rain).
        self.update_eggs(ctx);

        // Startup overlay: it covers the real loading time (window, shell,
        // first output) and fades out once the terminal is actually usable.
        let start = *self.entry_start.get_or_insert(self.now);
        let elapsed = self.now - start;
        // Real milestones, plus a slow creep so the bar never looks stuck
        // while the wallpaper decodes.
        let creep = (elapsed as f32 / LOAD_ESTIMATE_SECS).min(0.92);
        let target = self.load_target().max(creep);
        self.load_progress += ((target - self.load_progress).min(dt as f32 * 1.2)).max(0.0);
        let ready = self.ready_at.is_some() || elapsed > MAX_ENTRY_WAIT;
        if self.load_progress >= 0.999 && self.all_ready_at.is_none() {
            self.all_ready_at = Some(self.now);
        }
        let held = self
            .all_ready_at
            .is_some_and(|at| self.now - at >= READY_HOLD_SECS);
        if self.entry_fade_at.is_none() && ready && held && elapsed >= MIN_ENTRY_SECS {
            self.entry_fade_at = Some(self.now);
            log::info!("startup overlay fading out after {elapsed:.2}s");
        }
        self.entry_t = match self.entry_fade_at {
            Some(at) => ease_out_cubic(((self.now - at) / ENTRY_SECS) as f32),
            None => 0.0,
        };
        if self.entry_t < 1.0 {
            ctx.request_repaint();
        }

        // Tab enter/leave animations need frames while they run.
        if self.tabs.iter().any(|t| {
            t.opened_at.is_none_or(|at| self.now - at < TAB_ANIM_SECS)
                || t.closing_at.is_some_and(|at| self.now - at < TAB_ANIM_SECS)
        }) {
            ctx.request_repaint();
        }

        // Cursor blink: lit for the first half of each second after the last
        // keystroke, so it stays solid while typing.
        let blink =
            self.config.cursor_blink && self.tabs.get(self.active).is_none_or(|t| t.cursor_blink);
        let phase = (self.now - self.last_input) % 1.0;
        self.cursor_on = !blink || phase < 0.5;

        // Wake up for the next blink edge, the bell flash, or the toast fade.
        let mut wake: Option<f64> = None;
        if blink {
            wake = Some(if phase < 0.5 {
                0.5 - phase
            } else {
                1.0 - phase
            });
        }
        if self.bell_at.is_some() {
            wake = Some(wake.map_or(0.05, |w: f64| w.min(0.05)));
        }
        if self.toast.is_some() {
            wake = Some(wake.map_or(0.1, |w: f64| w.min(0.1)));
        }
        if let Some(dt) = wake {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(dt.max(0.016)));
        }

        // Automatic reconnects that have come due (never a closing tab).
        let due: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.closing_at.is_none() && t.reconnect_at.is_some_and(|at| self.now >= at)
            })
            .map(|(i, _)| i)
            .collect();
        for i in due {
            self.tabs[i].reconnect_at = None;
            self.reconnect_tab(i);
        }

        // Focus tracking for terminals that request it.
        let focused = ctx.input(|i| i.focused);
        if focused != self.last_focused {
            log::debug!("window focus: {focused}");
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            log::info!("window close requested by the compositor/user");
        }
        if focused != self.last_focused {
            self.last_focused = focused;
            if let Some(tab) = self.tabs.get(self.active)
                && tab.core.term.mode().contains(TermMode::FOCUS_IN_OUT)
            {
                let seq: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
                let _ = tab.client_tx.send(ClientMsg::Input(seq.to_vec()));
            }
        }

        // Unread flags and command-finished notifications.
        self.update_tab_activity(focused);

        // The `:info` panel reports the terminal's own CPU and memory, so keep
        // a fresh sample ready, and refresh the panel while it is open.
        if self.now - self.cpu_sample.map_or(0.0, |(_, at)| at) >= 0.5 {
            self.sample_process_cpu();
        }
        if self.sysinfo.is_some() && self.now - self.sysinfo_refresh_at >= 0.5 {
            self.sysinfo_refresh_at = self.now;
            self.refresh_sysinfo();
            ctx.request_repaint();
        }

        // Dropping an image onto the window sets it as the background.
        self.handle_dropped_files(ctx);

        self.handle_input(ctx);

        // Files dropped on the window from the desktop: the editor opens them.
        // Images are the wallpaper's — dropping one anywhere sets the
        // background, which `handle_dropped_files` takes care of.
        if self.view == View::Editor {
            let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    // A drop without a path (text, or an in-app drag) has an
                    // empty one.
                    .filter(|f| !f.path().as_os_str().is_empty())
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| !crate::app::background::is_image_path(p))
                    .collect()
            });
            for path in dropped {
                self.editor.open_path(&path);
            }
        }

        // The editor's requests to the application (view switches, remote
        // saves, settings) are serviced every frame, whichever view is in front.
        self.service_editor(ctx);

        // `:bench` measures the frame cadence and the cost of this pass.
        self.measure_bench(ctx, logic_started.elapsed());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let started = std::time::Instant::now();
        let ctx = ui.ctx().clone();
        let t = self.entry_t;

        // --- one continuous background layer (behind everything) ---
        //
        // Keep the wallpaper's placement exactly once across the client area.
        // The tint is also painted exactly once, so the tab strip and terminal
        // cannot form differently blended rectangular regions at their seam.
        if let Some(bg) = &self.background {
            let rect = ctx.content_rect();
            let quad = render::image_quad(
                [rect.min.x, rect.min.y, rect.width(), rect.height()],
                bg.size,
                self.config.background_fit,
            );
            let image_rect = egui::Rect::from_min_size(
                egui::pos2(quad.pos[0], quad.pos[1]),
                egui::vec2(quad.size[0], quad.size[1]),
            );
            let uv = egui::Rect::from_min_max(
                egui::pos2(quad.uv0[0], quad.uv0[1]),
                egui::pos2(quad.uv1[0], quad.uv1[1]),
            );
            ui.painter().image(
                bg.texture.id(),
                image_rect,
                uv,
                egui::Color32::WHITE.gamma_multiply(self.background_fade()),
            );
        }
        // Tint over the wallpaper: lighter while the startup overlay is up, so
        // the wordmark stays readable on a busy image.
        let tint = self.bg_alpha() * (t + (1.0 - t) * 0.55);
        ui.painter().rect_filled(
            ctx.content_rect(),
            0.0,
            ui::with_alpha(color32(self.theme.bg), tint),
        );

        // --- tab bar ---
        // This panel only owns layout and interaction. It intentionally has no
        // fill, border, or separator: the continuous backdrop above shows
        // through both it and the terminal below.
        // Tight vertical margin: the strip should hug the terminal below it.
        let bar_frame = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 3));
        egui::Panel::top("tab_bar")
            .exact_size(BAR_H)
            .frame(bar_frame)
            .show(ui, |ui| {
                ui.multiply_opacity(t);
                self.tab_bar(ui);
            });

        // --- main surface: the SSH sidebar, then the terminal or the editor ---
        if self.ssh_mode {
            egui::Panel::left("ssh_sidebar")
                .exact_size(240.0)
                .resizable(false)
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let rect = ui.max_rect();
                    self.ssh_sidebar(ui, rect);
                });
        }
        egui::CentralPanel::no_frame().show(ui, |ui| match self.view {
            View::Terminal => self.terminal_area(ui),
            View::Editor => {
                let rect = ui.max_rect();
                self.editor_area(ui, rect);
            }
        });

        // --- SFTP browser, when a host's files are open ---
        if let Some(panel) = &self.sftp
            && panel.open
        {
            let height = 220.0f32.min(ctx.content_rect().height() * 0.4);
            egui::Panel::bottom("sftp_panel")
                .exact_size(height)
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    let rect = ui.max_rect();
                    let alpha = self.panel_alpha();
                    if let Some(panel) = &mut self.sftp {
                        panel.draw(ui, rect, &self.theme, alpha);
                    }
                });
        }

        // --- easter egg: digital rain over the terminal ---
        if self.matrix_until.is_some() {
            let rect = self
                .layout
                .as_ref()
                .map(|l| l.rect)
                .unwrap_or_else(|| ctx.content_rect());
            let dt = self.frame_dt;
            self.draw_matrix(ui, rect, dt);
        }

        // --- system info panel (`:info`) ---
        if self.sysinfo.is_some() {
            let fade = self.sysinfo_fade();
            if fade < 1.0 {
                ctx.request_repaint();
            }
            let rect = ctx.content_rect();
            if let Some(info) = &self.sysinfo {
                info.draw(ui, rect, &self.theme, fade);
            }
        }

        // --- entry animation ---
        self.draw_entry_animation(ui, ctx.content_rect());

        // --- status toast: the last thing drawn, so nothing can hide it ---
        self.draw_toast(&ctx);

        // --- scrollback search bar ---
        if let Some(mut state) = self.search.take() {
            let anchor = self
                .layout
                .as_ref()
                .map(|l| l.rect)
                .unwrap_or_else(|| ctx.content_rect());
            let status = if let Some(err) = &state.error {
                err.clone()
            } else if state.query.is_empty() {
                String::new()
            } else if state.matches.is_empty() {
                "no matches".to_owned()
            } else {
                format!("{}/{}", state.current + 1, state.matches.len())
            };
            let is_error = state.error.is_some();
            let (close, next, prev) =
                ui::show_search(ui, &self.theme, anchor, &mut state.query, &status, is_error);
            if !close {
                self.search = Some(state);
                if next {
                    self.search_step(true);
                }
                if prev {
                    self.search_step(false);
                }
            }
        }

        // --- overlays ---
        self.draw_overlays(ui);

        // --- file drop hint ---
        if !ctx.input(|i| i.raw.hovered_files.is_empty()) {
            let rect = ctx.content_rect();
            let painter = ui.painter();
            painter.rect_filled(
                rect,
                0.0,
                ui::with_alpha(color32(self.theme.ui.accent), 0.12),
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop an image to set it as the background",
                egui::FontId::new(15.0, egui::FontFamily::Proportional),
                color32(self.theme.ui.text),
            );
        }

        // Remember the geometry so it can be restored next launch.
        let rect = ctx.content_rect();
        if self.last_window_rect != Some(rect) {
            log::debug!("window rect: {rect:?}");
        }
        self.last_window_rect = Some(rect);

        // --- window title ---
        let title = self
            .tabs
            .get(self.active)
            .map(|t| format!("{} — cl2metal", t.display_title()))
            .unwrap_or_else(|| "cl2metal".to_owned());
        if title != self.last_title {
            self.last_title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }

        // Frame cost, for diagnosing jank (the UI pass, not the GPU).
        let ms = started.elapsed().as_secs_f32() * 1000.0;
        if ms > 8.0 {
            log::debug!("slow frame: {ms:.1}ms");
        }
        // `:bench` folds this pass into its report.
        self.record_ui(ms);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Honour the configured window opacity.
        let bg = self.theme.ui.panel_bg;
        [
            bg[0] as f32 / 255.0,
            bg[1] as f32 / 255.0,
            bg[2] as f32 / 255.0,
            self.config.opacity.clamp(0.2, 1.0),
        ]
    }

    fn on_exit(&mut self) {
        log::info!(
            "on_exit: saving window geometry {:?}",
            self.last_window_rect
        );
        // An open settings modal only holds a live preview, never a saved
        // change: closing the window must not commit it.
        if let Overlay::Settings {
            original,
            original_font,
            ..
        } = &self.overlay
        {
            self.config = original.clone();
            self.config.font = original_font.clone();
        }
        // Persist the window geometry for the next launch.
        if let Some(rect) = self.last_window_rect {
            self.config.window_size = Some([rect.width(), rect.height()]);
            self.config.window_pos = Some([rect.min.x, rect.min.y]);
            if let Err(e) = self.config.save() {
                log::warn!("saving window geometry: {e}");
            }
        }
    }
}
impl App {
    fn revert_settings(&mut self, ctx: &egui::Context) {
        let restore = match &self.overlay {
            Overlay::Settings {
                original,
                original_font,
                ..
            } => Some((original.clone(), original_font.clone())),
            _ => None,
        };
        if let Some((config, font)) = restore {
            self.apply_config(config, font, ctx);
        }
    }

    /// Load (or reload) the configured background image onto the GPU.
    ///
    /// Called at startup and whenever the config changes. A path that does not
    /// resolve to a file yet (mid-typing in the settings modal) leaves the
    /// current image in place instead of flickering it away.
    fn apply_config(&mut self, mut config: Config, font: String, ctx: &egui::Context) {
        let font_changed = font != self.config.font || config.font_size != self.config.font_size;
        let cursor_changed = config.cursor_shape != self.config.cursor_shape
            || config.cursor_blink != self.config.cursor_blink;
        let scrollback_changed = config.scrollback != self.config.scrollback;
        let opacity_changed = config.opacity != self.config.opacity;
        let image_changed = config.background_image != self.config.background_image;
        config.font = font;
        self.config = config;

        // Load or drop the background image first: the chrome opacity below
        // depends on whether an image ended up loaded.
        if image_changed {
            self.sync_background(ctx);
        }

        let mut new_theme = theme::builtin(&self.config.theme);
        theme::apply_overrides(&mut new_theme, &self.config.colors);
        new_theme.ui_alpha = self.ui_alpha();
        if new_theme.id != self.theme.id
            || new_theme.ui.accent != self.theme.ui.accent
            || (new_theme.ui_alpha - self.theme.ui_alpha).abs() > f32::EPSILON
        {
            self.theme = new_theme.clone();
            ui::apply_theme(ctx, &new_theme);
        }

        if font_changed {
            let size = Self::raster_size(&self.config, ctx.pixels_per_point());
            let mut atlas = GlyphAtlas::new(&self.font_db, &self.config.font, size);
            atlas.version = next_atlas_version();
            self.atlas = atlas;
        }

        // Scrollback depth and the default cursor style live in the terminal
        // core, so push them into every open tab.
        if cursor_changed || scrollback_changed {
            let style = cursor_style(&self.config);
            for tab in &mut self.tabs {
                tab.core.set_options(self.config.scrollback, style);
            }
        }

        // Transparency is a window property; the background alpha itself is
        // read from the config on every frame.
        if opacity_changed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Transparent(
                self.config.opacity < 1.0,
            ));
        }

        log::info!(
            "config applied: theme={} font={:?} {:.1}pt cursor={:?} blink={} scrollback={} opacity={:.2} image={:?}",
            self.config.theme,
            self.config.font,
            self.config.font_size,
            self.config.cursor_shape,
            self.config.cursor_blink,
            self.config.scrollback,
            self.config.opacity,
            self.config.background_image,
        );
    }
}

/// Next font size for a zoom step: `delta == 0` resets to the default, and
/// every other step is clamped to a legible range.
fn zoomed_font_size(current: f32, delta: f32) -> f32 {
    if delta == 0.0 {
        crate::config::DEFAULT_FONT_SIZE
    } else {
        (current + delta).clamp(6.0, 48.0)
    }
}

/// Map the configured cursor preference onto the terminal's cursor style.
fn cursor_style(config: &Config) -> alacritty_terminal::vte::ansi::CursorStyle {
    use alacritty_terminal::vte::ansi::{CursorShape as VteShape, CursorStyle};
    let shape = match config.cursor_shape {
        CursorShape::Block => VteShape::Block,
        CursorShape::Beam => VteShape::Beam,
        CursorShape::Underline => VteShape::Underline,
    };
    CursorStyle {
        shape,
        blinking: config.cursor_blink,
    }
}

fn shell_label(shell: &str) -> String {
    let program = if shell.is_empty() {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned())
    } else {
        shell.to_owned()
    };
    std::path::Path::new(&program)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| program)
}

pub(crate) fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    path.to_owned()
}

fn truncate_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}
