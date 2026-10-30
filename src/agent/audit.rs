//! Append-only audit trail for the agent access API.
//!
//! Every agent request that reaches a decision — consent asked, session
//! opened, command run, session closed, request denied — is one line in
//! `agent-audit.log` next to the app's other state. The log never
//! contains secrets (none exist on this path) and never blocks a
//! request: a failing audit file degrades to no logging.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;

use crate::config::APP_NAME;

/// Where the audit log lives, next to the app's other state files.
pub fn audit_path() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(APP_NAME)
        .join("agent-audit.log")
}

/// `YYYY-MM-DD HH:MM:SSZ` for a unix timestamp, UTC like the app log.
fn stamp(secs: u64) -> String {
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to (year, month, day) — Howard Hinnant's
/// `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u64, u64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The audit log handle. The broker owns exactly one.
pub struct Audit {
    file: Option<std::fs::File>,
}

impl Audit {
    /// Open for append, creating the directory when needed. Failure is
    /// logged and swallowed: the feature must work without auditing.
    pub fn open() -> Self {
        let path = audit_path();
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            log::warn!("agent audit: cannot create {}: {e}", parent.display());
            return Self { file: None };
        }
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => Self { file: Some(file) },
            Err(e) => {
                log::warn!("agent audit: cannot open {}: {e}", path.display());
                Self { file: None }
            }
        }
    }

    /// Append one line. The caller supplies the who and the what; this
    /// stamps the when.
    pub fn record(&mut self, what: &str) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(file, "[{}] {what}", stamp(secs));
        let _ = file.flush();
    }
}

/// The last `lines` lines of the audit log, oldest first. A missing or
/// unreadable file reads as empty; the audit is best-effort by design.
pub fn tail(lines: usize) -> Vec<String> {
    tail_from(&audit_path(), lines)
}

fn tail_from(path: &std::path::Path, lines: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].iter().map(|line| (*line).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_full_utc_dates() {
        assert_eq!(stamp(0), "1970-01-01 00:00:00Z");
        // 2026-10-24 16:50:05 UTC.
        assert_eq!(stamp(1_792_860_605), "2026-10-24 16:50:05Z");
    }

    #[test]
    fn leap_days_lands_on_their_exact_date() {
        // 2024-02-29: day 19782 since the epoch.
        let (year, month, day) = civil_from_days(19_782);
        assert_eq!((year, month, day), (2024, 2, 29));
    }

    #[test]
    fn tail_returns_the_last_lines() {
        let dir = std::env::temp_dir().join(format!("cl2metal-audit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audit.log");
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        assert_eq!(tail_from(&path, 2), vec!["b".to_owned(), "c".to_owned()]);
        assert_eq!(tail_from(&path, 99).len(), 3);
        assert!(tail_from(&dir.join("missing.log"), 5).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
