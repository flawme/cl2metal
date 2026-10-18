//! cl2metal — a sleek, minimal, secure terminal + SSH client.

mod agent;
mod app;
mod bench;
mod config;
mod editor;
mod logging;
mod platform;
mod render;
mod term;
mod theme;
mod ui;

use app::{App, Cli};

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // `--version` / `-V`: report and exit before the log file, the window or
    // the CLI parser get involved.
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("cl2metal {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // `--bench-frames [SECS] [COLS] [ROWS]`: the `:bench` child. It prints the
    // spinning torus to stdout — the parent's PTY — and exits. Handled before
    // the log file so a bench run never touches it.
    if let Some(i) = args.iter().position(|a| a == "--bench-frames") {
        let arg = |n: usize, default: f32| -> f32 {
            args.get(i + n)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(default)
        };
        let secs = arg(1, 10.0).clamp(1.0, 600.0);
        let cols = arg(2, 80.0) as usize;
        let rows = arg(3, 24.0) as usize;
        if let Err(e) = bench::run_stdout(secs, cols, rows) {
            eprintln!("cl2metal: --bench-frames failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let log_path = logging::init();
    install_panic_hook();
    log::info!("=== cl2metal starting ===");
    log::info!("log file: {}", log_path.display());
    log::info!(
        "pid={} args={:?} cwd={:?}",
        std::process::id(),
        std::env::args().collect::<Vec<_>>(),
        std::env::current_dir()
    );
    log::info!(
        "env: SHELL={:?} TERM={:?} DISPLAY={:?} WAYLAND_DISPLAY={:?} XDG_SESSION_TYPE={:?}",
        std::env::var("SHELL").ok(),
        std::env::var("TERM").ok(),
        std::env::var("DISPLAY").ok(),
        std::env::var("WAYLAND_DISPLAY").ok(),
        std::env::var("XDG_SESSION_TYPE").ok()
    );

    // `--install-desktop`: write the menu entry and icon, then exit.
    if args.iter().any(|a| a == "--install-desktop") {
        if let Err(e) = platform::install_desktop() {
            eprintln!("cl2metal: --install-desktop failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    // `cl2metal mcp`: the MCP stdio server coding-agent hosts launch;
    // `cl2metal mcp skill` prints the built-in agent skill instead.
    // Parsed here, before Cli::parse(), because a bare positional would
    // otherwise land in cli.host.
    if args.first().is_some_and(|a| a == "mcp") {
        #[cfg(unix)]
        {
            if args.get(1).map(String::as_str) == Some("skill") {
                print!("{}", agent::mcp::SKILL_MD);
            } else if let Err(e) = agent::mcp::run() {
                eprintln!("cl2metal: mcp: {e}");
                std::process::exit(1);
            }
        }
        #[cfg(not(unix))]
        eprintln!("cl2metal: mcp needs a unix socket; not available here");
        return Ok(());
    }

    let cli = Cli::parse();
    log::info!(
        "cli: profile={:?} host={:?} exec={:?}",
        cli.profile,
        cli.host,
        cli.exec
    );
    let config = config::Config::load();
    log::info!(
        "config: shell={:?} theme={:?} scrollback={} log_sessions={} auto_reconnect={} window={:?} pos={:?}",
        config.shell,
        config.theme,
        config.scrollback,
        config.log_sessions,
        config.auto_reconnect,
        config.window_size,
        config.window_pos
    );

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("cl2metal")
        .with_inner_size(config.window_size.unwrap_or([1100.0, 720.0]))
        .with_min_inner_size([480.0, 300.0]);
    if config.opacity < 1.0 {
        viewport = viewport.with_transparent(true);
    }
    // Ignore absurd saved positions (a monitor that is no longer attached).
    if let Some(pos) = config.window_pos
        && pos[0].abs() < 2000.0
        && pos[1].abs() < 2000.0
    {
        viewport = viewport.with_position(pos);
    }

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    let result = eframe::run_native(
        "cl2metal",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, cli)))),
    );
    match &result {
        Ok(()) => log::info!("=== cl2metal exited normally ==="),
        Err(e) => log::error!("=== cl2metal exited with error: {e} ==="),
    }
    result
}

// ---------------------------------------------------------------------------
// Diagnostics and desktop integration
// ---------------------------------------------------------------------------

/// Log panics through the logger (so they land in the log file) and on stderr.
/// Termination is left to the default behaviour.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "<unknown>".to_owned());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "Box<dyn Any>".to_owned());
        let report = format!(
            "panic at {location}: {message}\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        log::error!("{report}");
        eprintln!("{report}");
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Captures what the panic hook emits through `log::error!`.
    static CAPTURED: Mutex<String> = Mutex::new(String::new());

    struct Capture;

    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            if let Ok(mut captured) = CAPTURED.lock() {
                captured.push_str(&record.args().to_string());
                captured.push('\n');
            }
        }

        fn flush(&self) {}
    }

    #[test]
    fn panic_hook_reports_location_message_and_backtrace() {
        // The hook logs through the global logger; capture it instead of
        // writing to the real log file.
        let _ = log::set_boxed_logger(Box::new(Capture));
        log::set_max_level(log::LevelFilter::Debug);
        install_panic_hook();

        let _ = std::panic::catch_unwind(|| panic!("boom"));

        let captured = CAPTURED.lock().map(|c| c.clone()).unwrap_or_default();
        assert!(
            captured.contains("panic at src/main.rs:")
                || captured.contains("panic at src\\main.rs:"),
            "{captured}"
        );
        assert!(captured.contains("boom"), "{captured}");
        // `Backtrace`'s Display starts with the first frame, not a header.
        assert!(captured.contains("\n   0: "), "{captured}");
    }
}
