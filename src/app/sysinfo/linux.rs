//! Linux system facts, read from `/proc`.
//!
//! Every reader degrades to "unknown" when a file is missing, so this code is
//! harmless on a unix without `/proc` — it just reports less.

use super::gib;

/// Read a file and trim it, or None.
pub(crate) fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// The machine's hostname.
pub(crate) fn hostname() -> String {
    read_trim("/proc/sys/kernel/hostname").unwrap_or_else(|| "localhost".to_owned())
}

/// `7.0.0-30-generic`, from /proc/sys/kernel/osrelease.
pub(crate) fn kernel_line() -> String {
    read_trim("/proc/sys/kernel/osrelease").unwrap_or_else(|| "unknown".to_owned())
}

/// `Linux Mint 22.1 (X11)`, from os-release plus the session type.
pub(crate) fn os_line() -> String {
    let name = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(str::to_owned))
        })
        .map(|v| v.trim_matches('"').to_owned())
        .unwrap_or_else(|| std::env::consts::OS.to_owned());
    match std::env::var("XDG_SESSION_TYPE") {
        Ok(session) if !session.is_empty() => format!("{name} ({session})"),
        _ => name,
    }
}

/// The model name out of /proc/cpuinfo text.
///
/// The line is `model name\t: AMD Ryzen ...`, so the tab has to go with the
/// colon and the space.
pub(crate) fn cpu_model(text: &str) -> String {
    text.lines()
        .find_map(|l| l.strip_prefix("model name"))
        .map(|v| v.trim_start_matches([':', ' ', '\t']).trim().to_owned())
        .unwrap_or_else(|| "unknown CPU".to_owned())
}

/// `AMD Ryzen 7 7445HS (12 threads)`, from /proc/cpuinfo.
pub(crate) fn cpu_line() -> String {
    let text = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let model = cpu_model(&text);
    let threads = text
        .lines()
        .filter(|l| l.starts_with("processor"))
        .count()
        .max(1);
    format!("{model} ({threads} threads)")
}

/// Total physical memory in KiB, from /proc/meminfo.
pub(crate) fn total_memory_kib() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    text.lines()
        .find(|l| l.starts_with("MemTotal:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
}

/// `1.7 GiB / 15.6 GiB`, from /proc/meminfo.
pub(crate) fn memory_line() -> String {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let field = |name: &str| -> Option<u64> {
        text.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    match (field("MemTotal:"), field("MemAvailable:")) {
        (Some(total), Some(available)) => {
            format!("{} / {}", gib(total - available), gib(total))
        }
        _ => "unknown".to_owned(),
    }
}

/// `0.52 1.20 1.57`, from /proc/loadavg.
pub(crate) fn load_line() -> String {
    read_trim("/proc/loadavg")
        .map(|l| l.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// `3h 12m`, from /proc/uptime.
pub(crate) fn uptime_line() -> String {
    let Some(seconds) = read_trim("/proc/uptime")
        .and_then(|l| l.split_whitespace().next().map(str::to_owned))
        .and_then(|v| v.parse::<f64>().ok())
    else {
        return "unknown".to_owned();
    };
    let total = seconds as u64;
    let (h, m) = (total / 3600, (total % 3600) / 60);
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// The process's resident set size in KiB, from /proc/self/status.
pub(crate) fn process_rss_kib() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    text.lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
}

/// Threads in this process, from /proc/self/status.
pub(crate) fn process_threads() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    text.lines()
        .find(|l| l.starts_with("Threads:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
}

/// CPU time this process has used, in clock ticks (utime + stime).
pub(crate) fn process_cpu_ticks() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/stat").ok()?;
    // The command name can contain spaces and brackets, so read from the last
    // `)` and count fields from there: state is field 3, utime 14, stime 15.
    let rest = text.rsplit_once(')')?.1;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let utime = fields.get(11)?.parse::<u64>().ok()?;
    let stime = fields.get(12)?.parse::<u64>().ok()?;
    Some(utime + stime)
}
