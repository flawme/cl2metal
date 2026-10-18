//! `:mcp-on` — registers this app's MCP server into the coding-agent
//! hosts found on this machine.
//!
//! Pure file edits, nothing clever: TOML hosts (Sarvam, Codex) get a
//! `[mcp_servers.cl2metal]` table appended — their existing bytes,
//! comments included, are kept verbatim — and JSON hosts (Claude
//! Code, Antigravity, Cursor, Windsurf) get the `mcpServers.cl2metal`
//! entry merged into the parsed object, so every other key survives.
//! A host that is already registered is left untouched, a host whose
//! config directory does not exist is reported as not installed, and
//! a config that cannot be parsed is never written.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

/// What happened with one host.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Status {
    /// Written: the registration landed in the file.
    Registered,
    /// The file already names this server; untouched.
    Already,
    /// No sign of that host on this machine.
    NotInstalled,
    /// The config exists but could not be read or parsed; untouched.
    Failed,
}

/// One host's outcome, for the toast and the log.
pub struct HostReport {
    pub host: &'static str,
    pub status: Status,
    /// The file touched, or why nothing was.
    pub detail: String,
}

impl HostReport {
    fn registered(host: &'static str, detail: impl Into<String>) -> Self {
        Self {
            host,
            status: Status::Registered,
            detail: detail.into(),
        }
    }

    fn already(host: &'static str, detail: impl Into<String>) -> Self {
        Self {
            host,
            status: Status::Already,
            detail: detail.into(),
        }
    }

    fn not_installed(host: &'static str, detail: impl Into<String>) -> Self {
        Self {
            host,
            status: Status::NotInstalled,
            detail: detail.into(),
        }
    }

    fn failed(host: &'static str, detail: impl Into<String>) -> Self {
        Self {
            host,
            status: Status::Failed,
            detail: detail.into(),
        }
    }
}

/// One line for the toast.
pub fn summary(reports: &[HostReport]) -> String {
    let count = |status: Status| reports.iter().filter(|r| r.status == status).count();
    let mut line = format!(
        "mcp-on: {} registered, {} already present, {} not found",
        count(Status::Registered),
        count(Status::Already),
        count(Status::NotInstalled)
    );
    let failed = count(Status::Failed);
    if failed > 0 {
        line.push_str(&format!(", {failed} failed (details in the log)"));
    }
    line
}

/// Register into every coding-agent host this machine shows evidence
/// of, and report what happened to each.
pub fn register_all() -> Vec<HostReport> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "cl2metal".to_owned());
    register_into(&home, &exe, on_path("claude"))
}

/// The hosts, in report order. `claude_installed` stands for "the
/// claude CLI is on PATH" — Claude Code keeps its user-scope servers
/// in `~/.claude.json`, which a fresh install has not created yet.
fn register_into(home: &Path, exe: &str, claude_installed: bool) -> Vec<HostReport> {
    let mut reports = Vec::new();
    reports.push(toml_host(
        "Sarvam Code",
        &home.join(".sarvam").join("config.toml"),
        exe,
    ));
    reports.push(toml_host(
        "Codex",
        &home.join(".codex").join("config.toml"),
        exe,
    ));
    let claude = home.join(".claude.json");
    if claude.exists() || claude_installed {
        reports.push(json_merge("Claude Code", &claude, exe));
    } else {
        reports.push(HostReport::not_installed(
            "Claude Code",
            "no ~/.claude.json and no claude on PATH",
        ));
    }
    let json_hosts = [
        (
            "Antigravity",
            home.join(".gemini").join("config").join("mcp_config.json"),
        ),
        ("Cursor", home.join(".cursor").join("mcp.json")),
        (
            "Windsurf",
            home.join(".codeium")
                .join("windsurf")
                .join("mcp_config.json"),
        ),
    ];
    for (host, path) in json_hosts {
        if !path.exists() && !path.parent().is_some_and(|p| p.exists()) {
            reports.push(HostReport::not_installed(host, "no config directory"));
        } else {
            reports.push(json_merge(host, &path, exe));
        }
    }
    reports
}

/// The registration snippet, with escaping the toml crate guarantees.
#[derive(Serialize)]
struct McpServer {
    command: String,
    args: Vec<&'static str>,
}

fn toml_table(exe: &str) -> String {
    let body = toml::to_string(&McpServer {
        command: exe.to_owned(),
        args: vec!["mcp"],
    })
    .unwrap_or_else(|_| "command = \"cl2metal\"\nargs = [\"mcp\"]\n".to_owned());
    format!("[mcp_servers.cl2metal]\n{body}")
}

/// Append the table to a host's TOML config, or create the file when
/// the config directory exists but the file does not. Existing bytes
/// are never rewritten.
fn toml_host(host: &'static str, path: &Path, exe: &str) -> HostReport {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return HostReport::failed(host, format!("{}: {e}", path.display()));
        }
    };
    if let Some(text) = &existing
        && (text.contains("[mcp_servers.cl2metal]") || text.contains("mcp_servers.cl2metal ="))
    {
        return HostReport::already(host, path.display().to_string());
    }
    // No file and no sign of the tool: leave the name unclaimed.
    if existing.is_none() && !path.parent().is_some_and(|p| p.exists()) {
        return HostReport::not_installed(host, "no config directory");
    }
    let mut text = existing.unwrap_or_default();
    if text.is_empty() {
        text.push_str("# registered by cl2metal :mcp-on\n");
    } else if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&toml_table(exe));
    match std::fs::write(path, text) {
        Ok(()) => HostReport::registered(host, path.display().to_string()),
        Err(e) => HostReport::failed(host, format!("{}: {e}", path.display())),
    }
}

/// Merge the `mcpServers.cl2metal` entry into a host's JSON config,
/// creating the file when its directory exists. Every other key of
/// the parsed object survives the write.
fn json_merge(host: &'static str, path: &Path, exe: &str) -> HostReport {
    let root: Value = match std::fs::read_to_string(path) {
        // A zero-byte config is a fresh install's; treat it as empty.
        Ok(text) if text.trim().is_empty() => serde_json::json!({}),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => value,
            Err(e) => {
                return HostReport::failed(
                    host,
                    format!("{} is not valid JSON (untouched): {e}", path.display()),
                );
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return HostReport::failed(host, format!("{}: {e}", path.display())),
    };
    let mut root = root;
    let Some(obj) = root.as_object_mut() else {
        return HostReport::failed(
            host,
            format!("{} root is not a JSON object (untouched)", path.display()),
        );
    };
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let Some(servers) = servers.as_object_mut() else {
        return HostReport::failed(
            host,
            format!("{} mcpServers is not an object (untouched)", path.display()),
        );
    };
    if servers.contains_key("cl2metal") {
        return HostReport::already(host, path.display().to_string());
    }
    servers.insert(
        "cl2metal".to_owned(),
        serde_json::json!({ "command": exe, "args": ["mcp"] }),
    );
    // Only now, with every check passed, does anything hit the disk.
    let mut text = serde_json::to_string_pretty(&root).unwrap_or_default();
    text.push('\n');
    match std::fs::write(path, text) {
        Ok(()) => HostReport::registered(host, path.display().to_string()),
        Err(e) => HostReport::failed(host, format!("{}: {e}", path.display())),
    }
}

/// Whether a command exists somewhere on PATH.
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cl2metal-register-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn toml_hosts_append_without_disturbing_existing_bytes() {
        let dir = scratch("toml-append");
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "# my own comment, kept byte for byte\nfont = \"mono\"\n\n[editor]\ntabs = 2\n",
        )
        .unwrap();

        let report = toml_host("Sarvam Code", &path, "/opt/cl2metal");
        assert_eq!(report.status, Status::Registered);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# my own comment, kept byte for byte\n"));
        assert!(text.contains("font = \"mono\""));
        assert!(text.contains("[editor]\ntabs = 2"));
        assert!(
            text.ends_with(
                "[mcp_servers.cl2metal]\ncommand = \"/opt/cl2metal\"\nargs = [\"mcp\"]\n"
            )
        );

        // What landed parses as real TOML with our table in it.
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            parsed["mcp_servers"]["cl2metal"]["command"].as_str(),
            Some("/opt/cl2metal")
        );
        assert_eq!(parsed["font"].as_str(), Some("mono"));

        // A second sweep finds its own work done and touches nothing.
        let before = std::fs::read_to_string(&path).unwrap();
        let report = toml_host("Sarvam Code", &path, "/opt/cl2metal");
        assert_eq!(report.status, Status::Already);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn toml_host_creates_a_config_when_only_the_directory_exists() {
        let dir = scratch("toml-create");
        let tool = dir.join(".sarvam");
        std::fs::create_dir_all(&tool).unwrap();
        let path = tool.join("config.toml");

        let report = toml_host("Sarvam Code", &path, "/usr/bin/cl2metal");
        assert_eq!(report.status, Status::Registered);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# registered by cl2metal :mcp-on"));
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            parsed["mcp_servers"]["cl2metal"]["args"][0].as_str(),
            Some("mcp")
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hosts_without_a_trace_are_left_alone() {
        let dir = scratch("missing");
        let path = dir.join(".codex").join("config.toml");
        let report = toml_host("Codex", &path, "/x/cl2metal");
        assert_eq!(report.status, Status::NotInstalled);
        assert!(!path.exists(), "nothing is created for a missing host");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn json_hosts_merge_without_losing_keys() {
        let dir = scratch("json-merge");
        let path = dir.join("mcp_config.json");
        std::fs::write(
            &path,
            "{\"other\": {\"keep\": true}, \"mcpServers\": {\"existing\": {\"command\": \"x\"}}}",
        )
        .unwrap();

        let report = json_merge("Antigravity", &path, "/opt/cl2metal");
        assert_eq!(report.status, Status::Registered);

        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            root["other"]["keep"],
            serde_json::json!(true),
            "other keys survive"
        );
        assert_eq!(
            root["mcpServers"]["existing"]["command"],
            serde_json::json!("x")
        );
        assert_eq!(
            root["mcpServers"]["cl2metal"]["command"],
            serde_json::json!("/opt/cl2metal")
        );
        assert_eq!(
            root["mcpServers"]["cl2metal"]["args"],
            serde_json::json!(["mcp"])
        );

        // Idempotent: the second sweep reports Already and writes nothing.
        let before = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            json_merge("Antigravity", &path, "/opt/cl2metal").status,
            Status::Already
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn json_hosts_refuse_to_touch_broken_configs() {
        let dir = scratch("json-broken");
        let path = dir.join("mcp.json");
        let broken = "{ this is not json";
        std::fs::write(&path, broken).unwrap();

        let report = json_merge("Cursor", &path, "/opt/cl2metal");
        assert_eq!(report.status, Status::Failed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken, "untouched");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn register_into_reports_each_host_it_finds() {
        let home = scratch("sweep");
        // Sarvam: installed, with a config to append to.
        std::fs::create_dir_all(home.join(".sarvam")).unwrap();
        std::fs::write(
            home.join(".sarvam").join("config.toml"),
            "font = \"mono\"\n",
        )
        .unwrap();
        // Cursor: installed (directory present), no config yet.
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        // Codex, Antigravity, Windsurf, Claude: no trace at all.

        let reports = register_into(&home, "/opt/cl2metal", false);
        assert_eq!(reports.len(), 6);
        let by_host = |name: &str| reports.iter().find(|r| r.host == name).unwrap();

        assert_eq!(by_host("Sarvam Code").status, Status::Registered);
        assert_eq!(by_host("Codex").status, Status::NotInstalled);
        assert_eq!(by_host("Claude Code").status, Status::NotInstalled);
        assert_eq!(by_host("Antigravity").status, Status::NotInstalled);
        assert_eq!(by_host("Cursor").status, Status::Registered);
        assert_eq!(by_host("Windsurf").status, Status::NotInstalled);

        // The files that were written say what we registered.
        let sarvam = std::fs::read_to_string(home.join(".sarvam").join("config.toml")).unwrap();
        assert!(sarvam.contains("[mcp_servers.cl2metal]"));
        let cursor: Value = serde_json::from_str(
            &std::fs::read_to_string(home.join(".cursor").join("mcp.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            cursor["mcpServers"]["cl2metal"]["command"],
            serde_json::json!("/opt/cl2metal")
        );

        // Claude installed but unconfigured: the file is created for it.
        let reports = register_into(&home, "/opt/cl2metal", true);
        assert_eq!(
            reports
                .iter()
                .find(|r| r.host == "Claude Code")
                .unwrap()
                .status,
            Status::Registered
        );

        let line = summary(&reports);
        assert!(
            line.starts_with("mcp-on: "),
            "the toast names itself: {line}"
        );
        assert!(line.contains("registered"), "counts registered: {line}");

        let _ = std::fs::remove_dir_all(home);
    }
}
