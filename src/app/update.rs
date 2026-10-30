//! The update check: one request per launch, a prompt when a newer release
//! exists, and a hand-off to the user's own terminal to run the installer.

use crate::term::Waker;

/// The latest release, as reported by the GitHub API.
const LATEST_URL: &str = "https://api.github.com/repos/flawme/cl2metal/releases/latest";

/// Start the check on a worker thread.
///
/// The result is `Some(version)` when a release newer than this build exists,
/// `None` when the app is current or the network is unavailable. Offline is
/// not an error: the request fails and no prompt appears.
pub(crate) fn check(wake: Waker) -> flume::Receiver<Option<String>> {
    let (tx, rx) = flume::bounded(1);
    std::thread::spawn(move || {
        let result = latest_version().filter(|latest| is_newer(latest, env!("CARGO_PKG_VERSION")));
        match &result {
            Some(version) => log::info!(
                "update available: {version} (running {})",
                env!("CARGO_PKG_VERSION")
            ),
            None => log::debug!("no update available"),
        }
        let _ = tx.send(result);
        wake();
    });
    rx
}

/// Fetch the latest release tag with `curl`, falling back to `wget`.
fn latest_version() -> Option<String> {
    let body = fetch(LATEST_URL)?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    json.get("tag_name")?.as_str().map(str::to_owned)
}

fn fetch(url: &str) -> Option<String> {
    let curl = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "10", url])
        .output();
    if let Ok(out) = curl
        && out.status.success()
    {
        return Some(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let wget = std::process::Command::new("wget")
        .args(["-qO-", "--timeout=10", url])
        .output();
    if let Ok(out) = wget
        && out.status.success()
    {
        return Some(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    None
}

/// Parse `v0.3.0` or `cl2-0.1.2` into numbers.
pub(crate) fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let tag = tag.trim();
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    let tag = tag.strip_prefix("cl2-").unwrap_or(tag);
    let mut parts = tag.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// Is `latest` newer than `current`?
pub(crate) fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// The command that installs the latest release.
pub(crate) fn update_command() -> &'static str {
    #[cfg(windows)]
    {
        "powershell -NoProfile -ExecutionPolicy Bypass -Command \"irm https://raw.githubusercontent.com/flawme/cl2metal/master/install.ps1 | iex\""
    }
    #[cfg(not(windows))]
    {
        "curl -fsSL https://raw.githubusercontent.com/flawme/cl2metal/master/install.sh | sh"
    }
}

/// The shell line the terminal runs: the installer, then a pause so the
/// result stays readable after the terminal would otherwise close.
pub(crate) fn terminal_script() -> String {
    #[cfg(windows)]
    {
        update_command().to_owned()
    }
    #[cfg(not(windows))]
    {
        format!(
            "{}; echo; printf 'cl2metal: press Enter to close\\n'; read -r _",
            update_command()
        )
    }
}

/// Open the user's terminal on the update command.
pub(crate) fn run_in_terminal() -> Result<(), String> {
    crate::platform::open_terminal(&terminal_script())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_tags_parse_with_or_without_a_prefix() {
        assert_eq!(parse_version("v0.3.0"), Some((0, 3, 0)));
        assert_eq!(parse_version("cl2-0.1.2"), Some((0, 1, 2)));
        assert_eq!(parse_version("0.10.4"), Some((0, 10, 4)));
        assert_eq!(parse_version("nightly"), None);
        assert_eq!(parse_version("v1.2"), None);
    }

    #[test]
    fn newer_means_numerically_greater() {
        assert!(is_newer("v0.3.1", "0.3.0"));
        assert!(is_newer("v0.10.0", "0.9.9"), "10 > 9, not string order");
        assert!(!is_newer("v0.3.0", "0.3.0"));
        assert!(!is_newer("v0.2.9", "0.3.0"));
        assert!(!is_newer("garbage", "0.3.0"));
    }

    #[test]
    fn the_update_command_names_the_installer() {
        let cmd = update_command();
        assert!(cmd.contains("install."), "command: {cmd}");
        assert!(cmd.contains("flawme/cl2metal"), "command: {cmd}");
    }
}
