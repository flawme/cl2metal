//! Terminal session core: wraps `alacritty_terminal`'s `Term` and defines the
//! message protocol between the UI thread and session backend threads.

pub mod input;
/// bash/zsh OSC 133 hooks: a unix mechanism.
#[cfg(unix)]
pub mod integration;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod local;
pub mod sftp;
pub mod ssh;
#[cfg(windows)]
pub mod windows;

/// A callback the session threads use to ask the UI for a repaint.
///
/// The session layer knows nothing about egui; the app hands it a closure.
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// The runtime the unix terminal sessions run on.
///
/// One shared, fixed-size pool instead of a thread per pane: splitting a
/// pane adds tasks, not threads, so the thread count stops growing with
/// the number of panes. Windows keeps a thread per loop (there is no
/// `AsyncFd` there), so it never builds this pool.
#[cfg(unix)]
pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("cl2metal-pty")
            .enable_all()
            .build()
            .expect("build the terminal runtime")
    })
}

/// A `SessionMsg` sender that also wakes the UI.
///
/// Without this, output only appears on the next animation frame or cursor
/// blink edge: a streaming program (an agent, a TUI) would land in half-second
/// jumps instead of smoothly.
#[derive(Clone)]
pub struct MsgTx {
    tx: flume::Sender<SessionMsg>,
    wake: Waker,
}

impl MsgTx {
    pub fn new(tx: flume::Sender<SessionMsg>, wake: Waker) -> Self {
        Self { tx, wake }
    }

    /// Send a message and wake the UI. An error means the UI is gone.
    pub fn send(&self, msg: SessionMsg) -> Result<(), flume::SendError<SessionMsg>> {
        let result = self.tx.send(msg);
        (self.wake)();
        result
    }
}

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::Dimensions,
    index::{Column, Direction, Line, Point, Side},
    term::{Config as TermConfig, Term, search::RegexSearch},
    vte,
};
use std::sync::Arc;

/// Messages sent from the UI thread to a session backend.
#[derive(Debug)]
pub enum ClientMsg {
    /// Bytes typed by the user (already encoded escape sequences).
    Input(Vec<u8>),
    /// Viewport resized.
    Resize {
        cols: u16,
        rows: u16,
        cell_w: u16,
        cell_h: u16,
    },
    /// Close the session (kill process / disconnect).
    Close,
}

/// Interactive request from a backend that needs a human answer.
#[derive(Debug, Clone)]
pub enum SshRequest {
    /// Trust-on-first-use host key verification.
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
    },
    /// Password prompt.
    Password { prompt: String },
    /// Keyboard-interactive prompt (MFA / enterprise servers).
    KeyboardInteractive { prompt: String },
    /// Private key passphrase prompt.
    Passphrase { prompt: String },
}

/// Messages sent from a session backend to the UI thread.
pub enum SessionMsg {
    /// Terminal output bytes.
    Data(Vec<u8>),
    /// Window title set by the application.
    Title(String),
    /// BEL.
    Bell,
    /// The application changed the cursor blinking state (DECSCUSR / mode 12).
    CursorBlink,
    /// OSC 52 clipboard store request.
    ClipboardStore(String),
    /// OSC 52 clipboard read request: reply with `fmt(clipboard_text)`.
    ClipboardLoad(Arc<dyn Fn(&str) -> String + Send + Sync>),
    /// A string the terminal wants written back to the PTY (query replies etc.).
    PtyWrite(String),
    /// Color query (OSC 10/11/12…): reply with `fmt(color)`.
    ColorRequest(usize, Arc<dyn Fn(vte::ansi::Rgb) -> String + Send + Sync>),
    /// Text area size query: reply with `fmt(size)`.
    SizeRequest(Arc<dyn Fn(WindowSize) -> String + Send + Sync>),
    /// Backend needs an answer before it can continue (blocking the session thread).
    SshRequest(SshRequest, std::sync::mpsc::Sender<Result<String, ()>>),
    /// Session ended with a message.
    Exited(String),
    /// A one-off notice for the UI (toast).
    Notice(String),
}

/// Bridges alacritty terminal events into the message channel.
pub struct Listener {
    tx: flume::Sender<SessionMsg>,
}

impl Listener {
    pub fn new(tx: flume::Sender<SessionMsg>) -> Self {
        Self { tx }
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let msg = match event {
            Event::Title(title) => SessionMsg::Title(title),
            Event::ResetTitle => SessionMsg::Title(String::new()),
            Event::PtyWrite(s) => SessionMsg::PtyWrite(s),
            Event::ColorRequest(index, fmt) => SessionMsg::ColorRequest(index, fmt),
            Event::TextAreaSizeRequest(fmt) => SessionMsg::SizeRequest(fmt),
            Event::ClipboardStore(_, text) => SessionMsg::ClipboardStore(text),
            Event::ClipboardLoad(_, fmt) => SessionMsg::ClipboardLoad(fmt),
            Event::Bell => SessionMsg::Bell,
            Event::CursorBlinkingChange => SessionMsg::CursorBlink,
            // Repaints are driven by Data arrival; child exit by the PTY waiter thread.
            _ => return,
        };
        let _ = self.tx.send(msg);
    }
}

/// Grid dimensions handed to `Term`.
#[derive(Clone, Copy)]
pub struct Dims {
    pub cols: usize,
    pub rows: usize,
    pub history: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.rows + self.history
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// A semantic prompt mark reported by the shell (OSC 133).
#[derive(Clone, Copy, Debug)]
pub struct PromptMark {
    /// Grid line the mark was seen on (negative = scrollback).
    pub line: i32,
    pub kind: MarkKind,
    /// Exit status, on `Done` marks only.
    pub exit: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    /// `A` — a prompt is being drawn.
    Prompt,
    /// `B` — the prompt ended, user input starts.
    Command,
    /// `C` — the command started producing output.
    Output,
    /// `D` — the command finished.
    Done,
}

/// Sub-line scroll state for one pane.
///
/// The wheel accumulates fractional lines and the drawn offset eases toward
/// the target, so a burst of events glides instead of jumping. The integer
/// part goes to the terminal's display offset; the fraction becomes a pixel
/// shift at render time.
#[derive(Default)]
pub struct ScrollState {
    /// Where the scroll is headed, in lines (0 = live, up = older).
    target: f32,
    /// Where it is drawn this frame, eased toward the target.
    display: f32,
}

impl ScrollState {
    /// Add wheel lines (fractional; positive = older).
    pub fn wheel(&mut self, lines: f32) {
        self.target += lines;
    }

    /// Head for an offset (keyboard paging): eased, not snapped.
    pub fn jump(&mut self, lines: f32) {
        self.target = lines;
    }

    /// Snap straight to an offset (typing returns to the live output).
    pub fn snap(&mut self, lines: f32) {
        self.target = lines;
        self.display = lines;
    }

    /// Ease toward the target. Returns true while still moving.
    pub fn ease(&mut self, dt: f32) -> bool {
        let k = 1.0 - (-dt * 26.0).exp();
        self.display += (self.target - self.display) * k;
        if (self.target - self.display).abs() < 0.005 {
            self.display = self.target;
            false
        } else {
            true
        }
    }

    /// Pull the state back to what the terminal actually reached, when a
    /// request ran past the history bounds. Without this the easing would
    /// chase an unreachable target forever.
    pub fn settle_to(&mut self, actual: f32) {
        if self.target > actual + 0.5 {
            self.target = actual;
        }
        if self.display > actual + 0.5 {
            self.display = actual;
        }
    }

    /// The integer offset to show, and the pixel shift that goes with it:
    /// the frame is drawn at `base` and shifted up by the returned pixels,
    /// which is what makes the motion sub-line.
    pub fn base_and_shift(&self, cell_h: f32) -> (i32, f32) {
        let base = self.display.ceil();
        (base as i32, (base - self.display) * cell_h)
    }
}

/// The parse/render state of one tab.
pub struct TermCore {
    pub term: Term<Listener>,
    processor: vte::ansi::Processor,
    pub dims: Dims,
    /// OSC 133 marks, oldest first.
    marks: Vec<PromptMark>,
    /// Partial OSC 133 sequence carried over between reads.
    pending: Vec<u8>,
    /// OSC 9;4 progress (0..=100), when the shell reports one.
    progress: Option<u8>,
    /// Raw OSC 9;4 state, so error states can be styled differently.
    progress_state: String,
    /// Answers to terminal capability queries, waiting to be written back to
    /// the session — the local pty or the SSH channel.
    responses: Vec<Vec<u8>>,
    /// The theme's background colour, for the OSC 11 reply.
    bg_rgb: [u8; 3],
    /// Sub-line scroll state: the wheel's fractional target and the eased
    /// drawn offset.
    pub scroll: ScrollState,
}

/// Marks kept per tab before the oldest are dropped.
const MARK_CAP: usize = 1024;

impl TermCore {
    pub fn new(
        cols: usize,
        rows: usize,
        history: usize,
        cursor: vte::ansi::CursorStyle,
        listener: Listener,
    ) -> Self {
        let dims = Dims {
            cols: cols.max(2),
            rows: rows.max(1),
            history,
        };
        let config = TermConfig {
            scrolling_history: history,
            default_cursor_style: cursor,
            // Modern apps (neovim, helix) negotiate the kitty protocol.
            kitty_keyboard: true,
            ..TermConfig::default()
        };
        let term = Term::new(config, &dims, listener);
        Self {
            term,
            processor: vte::ansi::Processor::new(),
            dims,
            marks: Vec::new(),
            pending: Vec::new(),
            progress: None,
            progress_state: String::new(),
            responses: Vec::new(),
            bg_rgb: [30, 30, 30],
            scroll: ScrollState::default(),
        }
    }

    /// Tell the core the theme's background, so the OSC 11 reply matches
    /// what a probing program sees behind the text.
    pub fn set_background(&mut self, rgb: [u8; 3]) {
        self.bg_rgb = rgb;
    }

    /// Take the answers to capability queries, for writing back to the
    /// session. Drained by the app's pump.
    pub fn take_responses(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.responses)
    }

    /// Apply live config changes: scrollback depth and the default cursor style.
    ///
    /// Apps can still override the cursor with DECSCUSR; this only moves the
    /// default the terminal falls back to.
    pub fn set_options(&mut self, history: usize, cursor: vte::ansi::CursorStyle) {
        self.dims.history = history;
        let config = TermConfig {
            scrolling_history: history,
            default_cursor_style: cursor,
            kitty_keyboard: true,
            ..TermConfig::default()
        };
        self.term.set_options(config);
    }

    /// Feed terminal output, intercepting OSC 133 shell-integration marks and
    /// OSC 9;4 progress reports.
    ///
    /// Those sequences are terminal metadata, not content: they are stripped
    /// from the stream and recorded instead of being drawn.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        let mut start = 0;
        let mut i = 0;
        while i < data.len() {
            // Capability queries (DA1, DA2, XTVERSION, DECRQM): answer them
            // and keep them off the screen.
            if data[i] == 0x1b {
                if let Some((len, response)) = query_at(&data[i..]) {
                    if start < i {
                        self.processor.advance(&mut self.term, &data[start..i]);
                    }
                    self.responses.push(response);
                    i += len;
                    start = i;
                    continue;
                }
                if is_partial_query(&data[i..]) {
                    // Split across reads: hold the tail back.
                    if start < i {
                        self.processor.advance(&mut self.term, &data[start..i]);
                    }
                    self.pending = data[i..].to_vec();
                    return;
                }
            }
            let is_osc = data[i] == 0x1b && data.get(i + 1) == Some(&b']');
            if !is_osc {
                i += 1;
                continue;
            }
            let payload_start = i + 2;
            let mut j = payload_start;
            let mut end = None;
            while j < data.len() {
                if data[j] == 0x07 {
                    end = Some((j, j + 1));
                    break;
                }
                if data[j] == 0x1b && data.get(j + 1) == Some(&b'\\') {
                    end = Some((j, j + 2));
                    break;
                }
                j += 1;
            }
            match end {
                Some((payload_end, seq_end)) => {
                    let payload = &data[payload_start..payload_end];
                    let handled = if let Some(rest) = payload.strip_prefix(b"133;") {
                        self.record_mark(rest);
                        true
                    } else if let Some(rest) = payload.strip_prefix(b"9;4;") {
                        self.record_progress(rest);
                        true
                    } else if payload == b"11;?" {
                        // "What is your background colour?" — answer with the
                        // theme's, so a probing CLI can pick light or dark.
                        let [r, g, b] = self.bg_rgb;
                        self.responses.push(
                            format!("\x1b]11;rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x07")
                                .into_bytes(),
                        );
                        true
                    } else {
                        false
                    };
                    if handled {
                        if start < i {
                            self.processor.advance(&mut self.term, &data[start..i]);
                        }
                        i = seq_end;
                        start = seq_end;
                    } else {
                        // Someone else's OSC: leave it for the parser.
                        i = seq_end;
                    }
                }
                None => {
                    // Sequence split across reads: hold the tail back, unless it
                    // is clearly not ours (or has grown unreasonably long).
                    let payload = &data[payload_start..];
                    let ours = b"133;".starts_with(payload)
                        || b"9;4;".starts_with(payload)
                        || b"11;?".starts_with(payload)
                        || payload.starts_with(b"133;")
                        || payload.starts_with(b"9;4;")
                        || payload.starts_with(b"11;?");
                    if !ours || payload.len() > 4096 {
                        i += 1;
                        continue;
                    }
                    if start < i {
                        self.processor.advance(&mut self.term, &data[start..i]);
                    }
                    self.pending = data[i..].to_vec();
                    return;
                }
            }
        }
        if start < data.len() {
            self.processor.advance(&mut self.term, &data[start..]);
        }
    }

    /// OSC 9;4 progress report: `state;progress`.
    fn record_progress(&mut self, payload: &[u8]) {
        let text = String::from_utf8_lossy(payload);
        let mut parts = text.split(';');
        let state = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim().parse::<u8>().ok();
        self.progress = match state {
            // 0 = clear, 1 = normal, 2 = error, 3 = indeterminate, 4 = paused.
            "0" => None,
            "1" | "4" => value.map(|v| v.min(100)),
            "2" => Some(100),
            "3" => Some(0),
            _ => self.progress,
        };
        self.progress_state = state.to_owned();
    }

    /// Current OSC 9;4 progress, if the shell reported any.
    pub fn progress(&self) -> Option<u8> {
        self.progress
    }

    /// True when the reported progress is an error state.
    pub fn progress_is_error(&self) -> bool {
        self.progress_state == "2"
    }

    fn record_mark(&mut self, payload: &[u8]) {
        let text = String::from_utf8_lossy(payload);
        let mut parts = text.split(';');
        let kind = match parts.next() {
            Some("A") => MarkKind::Prompt,
            Some("B") => MarkKind::Command,
            Some("C") => MarkKind::Output,
            Some("D") => MarkKind::Done,
            _ => return,
        };
        let exit = if kind == MarkKind::Done {
            parts.next().and_then(|s| s.trim().parse::<i32>().ok())
        } else {
            None
        };
        let line = self.term.grid().cursor.point.line.0;
        self.marks.push(PromptMark { line, kind, exit });
        if self.marks.len() > MARK_CAP {
            self.marks.drain(..MARK_CAP / 4);
        }
    }

    /// Semantic prompt marks, oldest first.
    pub fn marks(&self) -> &[PromptMark] {
        &self.marks
    }

    /// Exit status of the most recent finished command, cleared once a new one runs.
    pub fn last_exit(&self) -> Option<i32> {
        let mut exit = None;
        for mark in &self.marks {
            match mark.kind {
                MarkKind::Done => exit = mark.exit,
                MarkKind::Command | MarkKind::Output => exit = None,
                MarkKind::Prompt => {}
            }
        }
        exit
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        if cols == self.dims.cols && rows == self.dims.rows {
            return;
        }
        self.dims.cols = cols;
        self.dims.rows = rows;
        let dims = self.dims;
        self.term.resize(dims);
    }

    pub fn cols(&self) -> usize {
        self.dims.cols
    }

    pub fn rows(&self) -> usize {
        self.dims.rows
    }

    /// Collect up to `cap` matches of `regex` across scrollback and viewport.
    ///
    /// Matches come back in reading order (top of history first).
    pub fn search_collect(
        &self,
        regex: &mut RegexSearch,
        cap: usize,
    ) -> Vec<std::ops::RangeInclusive<Point>> {
        let mut out: Vec<std::ops::RangeInclusive<Point>> = Vec::new();
        let mut origin = Point::new(Line(-(self.term.grid().history_size() as i32)), Column(0));
        while out.len() < cap {
            let Some(found) =
                self.term
                    .search_next(regex, origin, Direction::Right, Side::Left, None)
            else {
                break;
            };
            let start = *found.start();
            // `search_next` wraps around at the end of the grid; stop instead of
            // collecting the same matches again.
            if let Some(previous) = out.last() {
                let previous_start = *previous.start();
                let wrapped = start.line < previous_start.line
                    || (start.line == previous_start.line
                        && start.column.0 <= previous_start.column.0);
                if wrapped {
                    break;
                }
            }
            let end = *found.end();
            out.push(found);
            // Advance one cell past the match so the next search makes progress.
            let next = if end.column.0 + 1 < self.dims.cols {
                Point::new(end.line, Column(end.column.0 + 1))
            } else {
                Point::new(end.line + 1, Column(0))
            };
            let stuck = next.line < origin.line
                || (next.line == origin.line && next.column.0 <= origin.column.0);
            if stuck {
                break;
            }
            origin = next;
        }
        out
    }

    /// Scroll the viewport so `point` is visible.
    pub fn scroll_to(&mut self, point: Point) {
        self.term.scroll_to_point(point);
    }
}

/// The capability queries a program can ask, and the answer to send back.
///
/// Modern CLIs (charmbracelet/colorprofile and friends) probe the terminal
/// before choosing a colour profile; a terminal that stays silent makes
/// them fall back to no colour, or wait out a timeout first. These are the
/// probes those libraries send.
fn query_at(data: &[u8]) -> Option<(usize, Vec<u8>)> {
    const QUERIES: &[(&[u8], &[u8])] = &[
        // DA1: VT220 with ANSI colour.
        (b"\x1b[c", b"\x1b[?62;4;6;22c"),
        // DA2: xterm-ish version.
        (b"\x1b[>c", b"\x1b[>1;4000;0c"),
        // XTVERSION.
        (b"\x1b[>q", b"\x1bP>|cl2metal 0.1\x1b\\"),
        // DECRQM: synchronized output and grapheme clusters, both reset.
        (b"\x1b[?2026$p", b"\x1b[?2026;2$y"),
        (b"\x1b[?2027$p", b"\x1b[?2027;2$y"),
    ];
    QUERIES
        .iter()
        .find(|(q, _)| data.starts_with(q))
        .map(|(q, a)| (q.len(), a.to_vec()))
}

/// Whether the tail could still grow into one of the queries.
fn is_partial_query(data: &[u8]) -> bool {
    const QUERIES: &[&[u8]] = &[
        b"\x1b[c",
        b"\x1b[>c",
        b"\x1b[>q",
        b"\x1b[?2026$p",
        b"\x1b[?2027$p",
    ];
    data.len() < 9 && QUERIES.iter().any(|q| q.starts_with(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core() -> TermCore {
        let (tx, _rx) = flume::unbounded();
        TermCore::new(
            80,
            24,
            100,
            vte::ansi::CursorStyle {
                shape: vte::ansi::CursorShape::Block,
                blinking: true,
            },
            Listener::new(tx),
        )
    }

    /// Text currently on screen, row by row.
    fn screen_text(core: &TermCore) -> String {
        let mut out = String::new();
        for indexed in core.term.renderable_content().display_iter {
            out.push(indexed.cell.c);
        }
        out
    }

    #[test]
    fn osc133_marks_are_recorded_and_stripped_from_the_stream() {
        let mut core = core();
        core.feed(b"hi\x1b]133;A\x07there");
        let text = screen_text(&core);
        assert!(text.starts_with("hithere"), "got {text:?}");
        assert_eq!(core.marks().len(), 1);
        assert_eq!(core.marks()[0].kind, MarkKind::Prompt);
        assert_eq!(core.marks()[0].exit, None);
    }

    #[test]
    fn osc133_done_marks_carry_the_exit_status() {
        let mut core = core();
        core.feed(b"\x1b]133;D;130\x07");
        assert_eq!(core.marks().len(), 1);
        assert_eq!(core.marks()[0].kind, MarkKind::Done);
        assert_eq!(core.marks()[0].exit, Some(130));
        assert_eq!(core.last_exit(), Some(130));
    }

    #[test]
    fn capability_queries_are_answered_and_not_printed() {
        let mut core = core();
        core.set_background([0x1e, 0x1e, 0x1e]);
        core.feed(b"\x1b[c\x1b[>q\x1b[?2026$p\x1b[?2027$p\x1b]11;?\x07");
        let responses = core.take_responses();
        assert_eq!(responses.len(), 5, "one answer per query");
        assert_eq!(responses[0], b"\x1b[?62;4;6;22c", "DA1");
        assert!(responses[1].starts_with(b"\x1bP>|"), "XTVERSION reply");
        assert_eq!(responses[2], b"\x1b[?2026;2$y", "sync output: reset");
        assert_eq!(responses[3], b"\x1b[?2027;2$y", "grapheme clusters: reset");
        assert_eq!(
            responses[4], b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07",
            "the theme's background"
        );
        assert!(
            screen_text(&core).trim().is_empty(),
            "nothing of the queries was drawn"
        );
    }

    #[test]
    fn the_scroll_eases_and_shifts_sub_line() {
        let mut s = ScrollState::default();
        s.wheel(2.5);
        // Eases toward the target rather than jumping.
        assert!(s.ease(0.016));
        let (base, shift) = s.base_and_shift(10.0);
        assert!((1..=3).contains(&base), "base {base}");
        assert!((0.0..10.0).contains(&shift), "shift {shift}");
        // Converges.
        for _ in 0..200 {
            s.ease(0.016);
        }
        let (base, shift) = s.base_and_shift(10.0);
        assert_eq!(base, 3, "ceil of 2.5");
        assert!((shift - 5.0).abs() < 0.01, "half a cell: {shift}");
    }

    #[test]
    fn a_scroll_past_the_history_settles() {
        let mut s = ScrollState::default();
        s.jump(1000.0);
        for _ in 0..50 {
            s.ease(0.016);
        }
        // The terminal only reached 40: the state settles there instead of
        // chasing the target forever.
        s.settle_to(40.0);
        assert!(!s.ease(0.016), "settled");
        let (base, _) = s.base_and_shift(10.0);
        assert_eq!(base, 40);
    }

    #[test]
    fn a_query_split_across_reads_is_still_answered() {
        let mut core = core();
        core.feed(b"\x1b[?20");
        assert!(
            core.take_responses().is_empty(),
            "held back, not answered yet"
        );
        core.feed(b"26$p");
        let responses = core.take_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[?2026;2$y");
    }

    #[test]
    fn osc133_sequence_split_across_reads_is_reassembled() {
        let mut core = core();
        core.feed(b"a\x1b]133;D;");
        core.feed(b"7\x07b");
        let text = screen_text(&core);
        assert!(text.starts_with("ab"), "got {text:?}");
        assert_eq!(core.marks().len(), 1);
        assert_eq!(core.last_exit(), Some(7));
    }

    #[test]
    fn osc133_accepts_the_st_terminator() {
        let mut core = core();
        core.feed(b"x\x1b]133;A\x1b\\y");
        let text = screen_text(&core);
        assert!(text.starts_with("xy"), "got {text:?}");
        assert_eq!(core.marks().len(), 1);
    }

    #[test]
    fn last_exit_clears_when_a_new_command_starts() {
        let mut core = core();
        core.feed(b"\x1b]133;D;1\x07");
        assert_eq!(core.last_exit(), Some(1));
        core.feed(b"\x1b]133;C\x07");
        assert_eq!(core.last_exit(), None);
        core.feed(b"\x1b]133;D;0\x07");
        assert_eq!(core.last_exit(), Some(0));
    }

    #[test]
    fn unknown_osc133_payloads_are_ignored() {
        let mut core = core();
        core.feed(b"\x1b]133;Z\x07ok");
        assert!(core.marks().is_empty());
        assert!(screen_text(&core).starts_with("ok"));
    }

    #[test]
    fn other_escape_sequences_still_reach_the_parser() {
        let mut core = core();
        // SGR colour + a title OSC must not be disturbed by the interception.
        core.feed(b"\x1b[31mred\x1b]0;title\x07");
        assert!(screen_text(&core).starts_with("red"));
    }

    #[test]
    fn search_collect_finds_matches_in_reading_order() {
        let mut core = core();
        core.feed(b"alpha beta\r\ngamma beta\r\ndelta");
        let mut regex = RegexSearch::new("beta").expect("regex");
        let matches = core.search_collect(&mut regex, 100);
        assert_eq!(matches.len(), 2);
        let first = *matches[0].start();
        let second = *matches[1].start();
        assert!(first.line < second.line, "matches must be ordered");
    }

    #[test]
    fn search_collect_respects_the_cap() {
        let mut core = core();
        for _ in 0..20 {
            core.feed(b"x x x\r\n");
        }
        let mut regex = RegexSearch::new("x").expect("regex");
        let matches = core.search_collect(&mut regex, 5);
        assert_eq!(matches.len(), 5);
    }

    #[test]
    fn set_options_applies_scrollback_and_cursor_live() {
        let mut core = core();
        // Fill more history than the new limit will keep.
        for i in 0..40 {
            core.feed(format!("line {i}\r\n").as_bytes());
        }
        assert!(core.term.grid().history_size() > 5);

        core.set_options(
            5,
            vte::ansi::CursorStyle {
                shape: vte::ansi::CursorShape::Beam,
                blinking: false,
            },
        );

        // The scrollback shrinks immediately, not just for new output.
        assert_eq!(core.term.grid().history_size(), 5);
        assert_eq!(core.dims.history, 5);
        let style = core.term.cursor_style();
        assert_eq!(style.shape, vte::ansi::CursorShape::Beam);
        assert!(!style.blinking);
    }
}
