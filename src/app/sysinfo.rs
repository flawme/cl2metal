//! The `:info` panel: a neofetch-style summary of the machine and the session.
//!
//! The panel itself is platform-agnostic; the facts come from one folder per
//! OS (`linux/` reads `/proc`, `macos/` uses system utilities and `windows/`
//! calls Win32 counters), so adding a platform is a new file plus the wiring.

use super::*;

// Keep OS readers out of the panel code.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
use windows as platform;

/// Only the tests reach for the parser directly; the panel uses `cpu_line`.
#[cfg(all(target_os = "linux", test))]
pub(crate) use platform::cpu_model;
pub(crate) use platform::{
    cpu_line, hostname, memory_line, os_line, process_cpu_ticks, process_rss_kib, process_threads,
    total_memory_kib, uptime_line,
};
#[cfg(target_os = "linux")]
pub(crate) use platform::{kernel_line, load_line};
#[cfg(target_os = "macos")]
pub(crate) use platform::{kernel_line, load_line};

/// The wordmark, ANSI-shadow style.
const LOGO: &str = "\
 ██████╗██╗     ██████╗
██╔════╝██║     ╚════██╗
██║     ██║      █████╔╝
██║     ██║     ██╔═══╝
╚██████╗███████╗███████╗
 ╚═════╝╚══════╝╚══════╝";

/// A collected system summary.
pub(crate) struct SysInfo {
    /// `user@host`, the panel title.
    pub(crate) title: String,
    /// Label/value rows, in display order.
    pub(crate) rows: Vec<(String, String)>,
}

impl SysInfo {
    /// Read the machine and session state.
    pub(crate) fn collect(app: &App) -> Self {
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "user".to_owned());
        let mut rows = Vec::new();
        rows.push(("OS".to_owned(), os_line()));
        // Kernel and load average are unix notions; the rows simply do not
        // exist where they would have nothing to say.
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        rows.push(("Kernel".to_owned(), kernel_line()));
        rows.push(("CPU".to_owned(), cpu_line()));
        rows.push(("GPU".to_owned(), app.gpu.clone()));
        rows.push(("Memory".to_owned(), memory_line()));
        rows.push(("App RAM".to_owned(), process_memory_line()));
        rows.push(("App CPU".to_owned(), process_cpu_line(app)));
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        rows.push(("Load".to_owned(), load_line()));
        rows.push(("Uptime".to_owned(), uptime_line()));
        rows.push(("Shell".to_owned(), shell_line(app)));
        rows.push((
            "Terminal".to_owned(),
            format!(
                "cl2metal {} · {} {:.0}pt",
                env!("CARGO_PKG_VERSION"),
                app.config.font,
                app.config.font_size
            ),
        ));
        let (cols, rows_n) = app.grid_size();
        rows.push((
            "Session".to_owned(),
            format!(
                "{} · {}x{} · {} tab{}",
                app.config.theme,
                cols,
                rows_n,
                app.tabs.len(),
                if app.tabs.len() == 1 { "" } else { "s" }
            ),
        ));
        rows.push((
            "Config".to_owned(),
            crate::config::Config::path().display().to_string(),
        ));
        Self {
            title: format!("{user}@{}", hostname()),
            rows,
        }
    }

    /// Draw the panel centred in `rect`.
    pub(crate) fn draw(&self, ui: &egui::Ui, rect: egui::Rect, theme: &Theme, fade: f32) {
        let line_h = 19.0;
        let logo_lines = LOGO.lines().count();
        let body_lines = self.rows.len() + 3;
        let height = (logo_lines.max(body_lines) as f32 + 2.0) * line_h;
        let width = 700.0_f32.min(rect.width() - 40.0);
        let panel = egui::Rect::from_center_size(rect.center(), egui::vec2(width, height));
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            panel,
            12.0,
            ui::with_alpha(color32(theme.ui.panel_bg), 0.94 * fade),
        );
        painter.rect_stroke(
            panel,
            12.0,
            egui::Stroke::new(1.0, ui::with_alpha(color32(theme.ui.border), fade)),
            egui::StrokeKind::Inside,
        );

        let font = egui::FontId::new(13.0, egui::FontFamily::Monospace);
        let accent = ui::with_alpha(color32(theme.ui.accent), fade);
        let text = ui::with_alpha(color32(theme.ui.text), fade);
        let dim = ui::with_alpha(color32(theme.ui.text_dim), fade);

        // Logo on the left, in the accent colour.
        let logo_x = panel.min.x + 20.0;
        for (i, line) in LOGO.lines().enumerate() {
            painter.text(
                egui::pos2(logo_x, panel.min.y + 20.0 + i as f32 * line_h),
                egui::Align2::LEFT_TOP,
                line,
                font.clone(),
                accent,
            );
        }

        // Title, rule, then the label/value rows.
        let x = panel.min.x + 230.0;
        let mut y = panel.min.y + 20.0;
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_TOP,
            &self.title,
            font.clone(),
            accent,
        );
        y += line_h;
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_TOP,
            "─".repeat(self.title.chars().count()),
            font.clone(),
            dim,
        );
        y += line_h * 1.4;
        // Values are elided to the panel width, so a long CPU name cannot
        // spill past the border.
        let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
        let max_chars = (((panel.width() - 314.0 - 16.0) / char_w).floor() as usize).max(8);
        for (label, value) in &self.rows {
            painter.text(
                egui::pos2(x, y),
                egui::Align2::LEFT_TOP,
                label,
                font.clone(),
                accent,
            );
            painter.text(
                egui::pos2(x + 84.0, y),
                egui::Align2::LEFT_TOP,
                truncate_label(value, max_chars),
                font.clone(),
                text,
            );
            y += line_h;
        }
        y += line_h * 0.4;
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_TOP,
            "Esc to close",
            font.clone(),
            dim,
        );
    }
}

impl App {
    /// Sample this process's CPU time, keeping `cpu_percent` fresh.
    pub(crate) fn sample_process_cpu(&mut self) {
        let Some(ticks) = process_cpu_ticks() else {
            return;
        };
        if let Some(prev) = self.cpu_sample {
            if let Some(pct) = cpu_percent(prev, (ticks, self.now)) {
                self.cpu_percent = Some(pct);
                self.cpu_sample = Some((ticks, self.now));
            }
        } else {
            self.cpu_sample = Some((ticks, self.now));
        }
    }

    /// Re-read the machine and session facts for the open `:info` panel.
    pub(crate) fn refresh_sysinfo(&mut self) {
        let info = SysInfo::collect(self);
        self.sysinfo = Some(info);
    }

    /// Open (or refresh) the `:info` panel.
    pub(crate) fn show_sysinfo(&mut self) {
        self.sample_process_cpu();
        let info = SysInfo::collect(self);
        log::info!("system info panel: {}", info.title);
        self.sysinfo = Some(info);
        self.sysinfo_at = Some(self.now);
        self.sysinfo_refresh_at = self.now;
    }

    /// Close the `:info` panel.
    pub(crate) fn close_sysinfo(&mut self) {
        self.sysinfo = None;
        self.sysinfo_at = None;
    }

    /// 0..=1 fade-in of the panel.
    pub(crate) fn sysinfo_fade(&self) -> f32 {
        match self.sysinfo_at {
            Some(at) => (((self.now - at) / 0.18).clamp(0.0, 1.0)) as f32,
            None => 1.0,
        }
    }
}

/// Clock ticks per second: `sysconf(_SC_CLK_TCK)` on unix, and the unit the
/// Windows implementation converts `FILETIME` into. 100 on both.
const USER_HZ: f64 = 100.0;

/// CPU percentage from two samples of `(ticks, seconds)`.
///
/// 100% means one core fully busy, like `top`.
pub(crate) fn cpu_percent(prev: (u64, f64), now: (u64, f64)) -> Option<f32> {
    let dt = now.1 - prev.1;
    if dt <= 0.05 {
        return None;
    }
    let secs = now.0.saturating_sub(prev.0) as f64 / USER_HZ;
    Some((secs / dt * 100.0) as f32)
}

/// `142 MiB · 0.9% of 15.6 GiB`: what the terminal itself is holding.
pub(crate) fn process_memory_line() -> String {
    let Some(rss) = process_rss_kib() else {
        return "unknown".to_owned();
    };
    match total_memory_kib().filter(|total| *total > 0) {
        Some(total) => format!(
            "{} · {:.1}% of {}",
            mib(rss),
            rss as f64 / total as f64 * 100.0,
            gib(total)
        ),
        None => mib(rss),
    }
}

/// `3.2% of one core · 24 threads`: what the terminal itself is burning.
pub(crate) fn process_cpu_line(app: &App) -> String {
    let threads = process_threads()
        .map(|t| format!(" · {t} threads"))
        .unwrap_or_default();
    match app.cpu_percent {
        Some(pct) => format!("{pct:.1}% of one core{threads}"),
        None => format!("measuring…{threads}"),
    }
}

/// The shell the next tab would use.
fn shell_line(app: &App) -> String {
    let shell = if app.config.shell.is_empty() {
        crate::term::local::default_shell()
    } else {
        app.config.shell.clone()
    };
    std::path::Path::new(&shell)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or(shell)
}

/// Bytes as GiB with one decimal.
pub(crate) fn gib(kib: u64) -> String {
    format!("{:.1} GiB", kib as f64 / 1024.0 / 1024.0)
}

/// Bytes as MiB, rounded.
pub(crate) fn mib(kib: u64) -> String {
    format!("{} MiB", (kib as f64 / 1024.0).round() as u64)
}
