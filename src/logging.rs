//! File + stderr logging, so an unexpected close or a panic leaves a trace.
//!
//! A GUI launch (file manager, desktop entry) throws stderr away, so every
//! message is also appended to `~/.local/state/cl2metal/cl2metal.log`.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Where the log lives (printed at startup).
pub fn log_path() -> PathBuf {
    let dir = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(crate::config::APP_NAME);
    let _ = std::fs::create_dir_all(&dir);
    dir.join("cl2metal.log")
}

struct Logger {
    file: Mutex<std::fs::File>,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // Our own crate logs everything; dependencies stay quiet.
        if metadata.target().starts_with("cl2metal") {
            metadata.level() <= log::Level::Debug
        } else {
            metadata.level() <= log::Level::Warn
        }
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line = format!(
            "[{}] {:<5} {}: {}\n",
            hms(secs),
            record.level(),
            record.target(),
            record.args()
        );
        let _ = std::io::stderr().write_all(line.as_bytes());
        if let Ok(mut file) = self.file.lock() {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    fn flush(&self) {}
}

/// `HH:MM:SS` (UTC) for a unix timestamp.
fn hms(secs: u64) -> String {
    let day = secs % 86_400;
    format!("{:02}:{:02}:{:02}", day / 3600, (day % 3600) / 60, day % 60)
}

/// Install the logger.
///
/// Returns the log path so the caller can report it.
pub fn init() -> PathBuf {
    let path = log_path();
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => {
            let logger = Logger {
                file: Mutex::new(file),
            };
            if log::set_boxed_logger(Box::new(logger)).is_ok() {
                log::set_max_level(log::LevelFilter::Debug);
            }
        }
        Err(e) => {
            eprintln!("cl2metal: cannot open {}: {e}", path.display());
        }
    }

    path
}
