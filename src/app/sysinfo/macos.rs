//! macOS system facts, collected from `sysctl`, `vm_stat`, and `ps`.

use super::gib;
use std::process::Command;

fn command_text(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output.status.success().then_some(())?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn sysctl(name: &str) -> Option<String> {
    command_text("sysctl", &["-n", name])
}

pub(crate) fn hostname() -> String {
    command_text("hostname", &[]).unwrap_or_else(|| "localhost".to_owned())
}

pub(crate) fn os_line() -> String {
    let version =
        command_text("sw_vers", &["-productVersion"]).unwrap_or_else(|| "unknown".to_owned());
    format!("macOS {version} ({})", std::env::consts::ARCH)
}

pub(crate) fn kernel_line() -> String {
    sysctl("kern.osrelease").unwrap_or_else(|| "unknown".to_owned())
}

pub(crate) fn cpu_model(_text: &str) -> String {
    sysctl("machdep.cpu.brand_string")
        .or_else(|| sysctl("hw.model"))
        .unwrap_or_else(|| "unknown CPU".to_owned())
}

pub(crate) fn cpu_line() -> String {
    let model = cpu_model("");
    let count = sysctl("hw.logicalcpu").unwrap_or_else(|| "1".to_owned());
    format!("{model} ({count} threads)")
}

pub(crate) fn total_memory_kib() -> Option<u64> {
    sysctl("hw.memsize")?
        .parse::<u64>()
        .ok()
        .map(|bytes| bytes / 1024)
}

fn vm_pages() -> Option<(u64, u64)> {
    let total = total_memory_kib()?;
    let page_size = sysctl("hw.pagesize")?.parse::<u64>().ok()?;
    let output = command_text("vm_stat", &[])?;
    let pages = |label: &str| -> u64 {
        output
            .lines()
            .find(|line| line.starts_with(label))
            .and_then(|line| line.split_whitespace().last())
            .and_then(|value| value.trim_end_matches('.').parse::<u64>().ok())
            .unwrap_or(0)
    };
    let available_pages = ["Pages free:", "Pages inactive:", "Pages speculative:"]
        .iter()
        .map(|name| pages(name))
        .sum::<u64>();
    Some((total, available_pages.saturating_mul(page_size) / 1024))
}

pub(crate) fn memory_line() -> String {
    match vm_pages() {
        Some((total, available)) => {
            format!("{} / {}", gib(total.saturating_sub(available)), gib(total))
        }
        None => "unknown".to_owned(),
    }
}

pub(crate) fn load_line() -> String {
    sysctl("vm.loadavg")
        .map(|raw| {
            raw.trim_matches(['{', '}', ' '])
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

pub(crate) fn uptime_line() -> String {
    let Some(boot) = sysctl("kern.boottime") else {
        return "unknown".to_owned();
    };
    let Some(seconds) = boot
        .split("sec =")
        .nth(1)
        .and_then(|value| value.split([',', '}']).next())
        .and_then(|value| value.trim().parse::<u64>().ok())
    else {
        return "unknown".to_owned();
    };
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
        .unwrap_or_default()
        .as_secs();
    let (hours, minutes) = (elapsed / 3600, elapsed % 3600 / 60);
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

pub(crate) fn process_rss_kib() -> Option<u64> {
    command_text("ps", &["-o", "rss=", "-p", &std::process::id().to_string()])?
        .parse::<u64>()
        .ok()
}

pub(crate) fn process_threads() -> Option<u64> {
    let output = Command::new("ps")
        .args(["-M", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .count() as u64,
    )
}

pub(crate) fn process_cpu_ticks() -> Option<u64> {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `usage` is writable for the duration of the call and RUSAGE_SELF
    // requests counters for this process.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    if result != 0 {
        return None;
    }
    let ticks = |time: libc::timeval| -> u64 {
        let micros = time.tv_sec as u64 * 1_000_000 + time.tv_usec as u64;
        micros / 10_000
    };
    Some(ticks(usage.ru_utime) + ticks(usage.ru_stime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_counters_are_available_on_macos() {
        assert!(total_memory_kib().is_some_and(|memory| memory > 0));
        assert!(process_rss_kib().is_some_and(|memory| memory > 0));
        assert!(process_threads().is_some_and(|threads| threads > 0));
        assert!(process_cpu_ticks().is_some());
    }

    #[test]
    fn load_average_has_three_values() {
        let load = load_line();
        assert_eq!(load.split_whitespace().count(), 3, "{load}");
    }
}
