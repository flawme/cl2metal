//! Configuration and secure credential storage for cl2metal.
//!
//! Config lives in `~/.config/cl2metal/config.toml`.
//! Secrets (SSH passwords, key passphrases) NEVER live in the config file —
//! they are stored in the system keyring (Secret Service / GNOME Keyring / KWallet).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const APP_NAME: &str = "cl2metal";
pub const KEYRING_SERVICE: &str = "cl2metal";
/// Font size used by Ctrl+0 and as the initial default.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

fn default_font() -> String {
    crate::platform::default_font()
}
fn default_font_size() -> f32 {
    DEFAULT_FONT_SIZE
}
fn default_theme() -> String {
    "midnight".to_owned()
}
fn default_scrollback() -> usize {
    10_000
}
fn default_port() -> u16 {
    22
}
fn default_padding() -> [f32; 2] {
    [8.0, 12.0]
}
fn default_cursor_blink() -> bool {
    true
}
fn default_visual_bell() -> bool {
    true
}
fn default_true() -> bool {
    true
}
fn default_keepalive() -> u64 {
    30
}
fn default_opacity() -> f32 {
    1.0
}
fn default_background_image_opacity() -> f32 {
    0.28
}

/// Preferred cursor shape (apps can still override it with DECSCUSR).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CursorShape {
    #[default]
    Block,
    Beam,
    Underline,
}

impl CursorShape {
    pub fn label(self) -> &'static str {
        match self {
            CursorShape::Block => "block",
            CursorShape::Beam => "beam",
            CursorShape::Underline => "underline",
        }
    }
}

/// How the background image is fitted to the terminal area.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundFit {
    /// Fill the area, cropping the overflow (keeps the aspect ratio).
    #[default]
    Cover,
    /// Fit inside the area, letterboxed (keeps the aspect ratio).
    Contain,
    /// Fill the area exactly, distorting the image.
    Stretch,
}

impl BackgroundFit {
    pub fn label(self) -> &'static str {
        match self {
            BackgroundFit::Cover => "cover",
            BackgroundFit::Contain => "contain",
            BackgroundFit::Stretch => "stretch",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    #[default]
    Password,
    Key,
    /// Use the running ssh-agent (SSH_AUTH_SOCK).
    Agent,
}

/// A saved SSH connection profile. Credentials are not stored here.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SshProfile {
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    #[serde(default)]
    pub auth: AuthMethod,
    /// Path to a private key file (used when `auth == Key`).
    #[serde(default)]
    pub key_path: Option<String>,
    /// Jump host: `[user@]host[:port]` (ProxyJump).
    #[serde(default)]
    pub proxy_jump: Option<String>,
    /// Local port forwards: `[bind:]port:host:hostport` (ssh -L).
    #[serde(default)]
    pub forwards: Vec<String>,
    /// Remote port forwards: `[bind:]port:host:hostport` (ssh -R).
    #[serde(default)]
    pub remote_forwards: Vec<String>,
    /// Dynamic SOCKS5 forwards: `[bind:]port` (ssh -D).
    #[serde(default)]
    pub socks_forwards: Vec<String>,
    /// Forward the ssh-agent to the remote host (ssh -A).
    #[serde(default)]
    pub agent_forwarding: bool,
    /// The group this host belongs to, for the SSH workspace sidebar.
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Font family for terminal text.
    pub font: String,
    /// Font size in points.
    pub font_size: f32,
    /// Theme id: "midnight" | "graphite" | "paper".
    pub theme: String,
    /// Shell to launch ("" = $SHELL, fallback /bin/bash).
    pub shell: String,
    /// Scrollback lines.
    pub scrollback: usize,
    /// Padding around the terminal grid, in points: [x, y].
    pub padding: [f32; 2],
    /// Preferred cursor shape: "block" | "beam" | "underline".
    #[serde(default)]
    pub cursor_shape: CursorShape,
    /// Blink the cursor when idle.
    #[serde(default = "default_cursor_blink")]
    pub cursor_blink: bool,
    /// Flash the terminal when the session rings the bell.
    #[serde(default = "default_visual_bell")]
    pub visual_bell: bool,
    /// Copy a mouse selection to the clipboard as soon as it is made.
    #[serde(default = "default_true")]
    pub copy_on_select: bool,
    /// Send a desktop notification when a command finishes while the window
    /// is unfocused (needs shell integration for the timing).
    #[serde(default = "default_true")]
    pub desktop_notifications: bool,
    /// SSH keepalive interval in seconds (0 disables it).
    #[serde(default = "default_keepalive")]
    pub ssh_keepalive: u64,
    /// Allow programs to read the clipboard with OSC 52 (off by default).
    #[serde(default)]
    pub allow_osc52_read: bool,
    /// Write every session's output to a log file.
    #[serde(default)]
    pub log_sessions: bool,
    /// Re-open SSH sessions automatically when the connection drops.
    #[serde(default)]
    pub auto_reconnect: bool,
    /// Window opacity (1.0 = opaque).
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// Background image drawn behind the terminal text (path; empty = none).
    #[serde(default)]
    pub background_image: Option<String>,
    /// How strongly the background image shows through (0 = hidden, 1 = full).
    #[serde(default = "default_background_image_opacity")]
    pub background_image_opacity: f32,
    /// How the background image is fitted to the terminal area.
    #[serde(default)]
    pub background_fit: BackgroundFit,
    /// Colour overrides applied on top of the selected theme, e.g.
    /// `accent = "#ff8800"`, `bg = "#000000"`.
    #[serde(default)]
    pub colors: std::collections::BTreeMap<String, String>,
    /// Last window size in points, restored on the next launch.
    #[serde(default)]
    pub window_size: Option<[f32; 2]>,
    /// Last window position in points, restored on the next launch.
    #[serde(default)]
    pub window_pos: Option<[f32; 2]>,
    /// Store SSH passwords in the system keyring (never on disk in plaintext).
    pub remember_secrets: bool,
    /// Saved SSH profiles.
    pub profiles: Vec<SshProfile>,
    /// Editor settings.
    #[serde(default)]
    pub editor: EditorConfig,
    /// Editor keybindings: command name -> key sequence (`"save" = "ctrl+s"`).
    #[serde(default)]
    pub editor_keys: std::collections::BTreeMap<String, String>,
    /// Whether the SSH workspace sidebar is shown.
    #[serde(default)]
    pub ssh_mode: bool,
    /// Profile names marked as favourites, in the order they were starred.
    #[serde(default)]
    pub favorites: Vec<String>,
    /// Recently connected hosts, most recent first.
    #[serde(default)]
    pub recent_hosts: Vec<String>,
    /// The editor's file tree root. Empty means follow the focused
    /// terminal's directory, so `cd` in the terminal moves the tree.
    #[serde(default)]
    pub editor_root: Option<String>,
    /// Which surface to open on: "terminal" or "editor".
    #[serde(default = "default_view")]
    pub view: String,
}

/// The surface a fresh install opens on.
fn default_view() -> String {
    "terminal".to_owned()
}

/// Editor settings, persisted under `[editor]` in config.toml.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    pub tab_width: usize,
    pub expand_tabs: bool,
    pub line_numbers: bool,
    pub relative_numbers: bool,
    pub show_whitespace: bool,
    pub wrap: bool,
    pub highlight_current_line: bool,
    /// `block`, `bar` or `underline`.
    pub cursor_shape: String,
    pub scroll_off: usize,
    pub auto_indent: bool,
    pub highlight_word: bool,
    /// Whether the file tree is open.
    pub tree_open: bool,
    /// The file tree's column width in points; its grip drags it.
    pub tree_width: f32,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            tab_width: 4,
            expand_tabs: true,
            line_numbers: true,
            relative_numbers: false,
            show_whitespace: false,
            wrap: false,
            highlight_current_line: true,
            cursor_shape: "block".to_owned(),
            scroll_off: 3,
            auto_indent: true,
            highlight_word: true,
            tree_open: true,
            tree_width: 220.0,
        }
    }
}

impl EditorConfig {
    /// The editor's runtime settings.
    pub fn to_settings(&self) -> crate::editor::Settings {
        crate::editor::Settings {
            tab_width: self.tab_width.clamp(1, 16),
            expand_tabs: self.expand_tabs,
            line_numbers: self.line_numbers,
            relative_numbers: self.relative_numbers,
            show_whitespace: self.show_whitespace,
            wrap: self.wrap,
            highlight_current_line: self.highlight_current_line,
            cursor_shape: self.cursor_shape.clone(),
            scroll_off: self.scroll_off.min(20),
            auto_indent: self.auto_indent,
            highlight_word: self.highlight_word,
        }
    }

    /// Read the settings back out of a running editor, for saving.
    ///
    /// The tree's width is passed in rather than read from the editor: the
    /// caller hands back the config's own value, so the every-frame
    /// comparison this feeds stays blind to the width. A drag in motion
    /// must not rewrite the config on every frame, and the width reaches
    /// the config only when its drag ends (`wants_width_save`).
    pub fn from_settings(
        settings: &crate::editor::Settings,
        tree_open: bool,
        tree_width: f32,
    ) -> Self {
        Self {
            tab_width: settings.tab_width,
            expand_tabs: settings.expand_tabs,
            line_numbers: settings.line_numbers,
            relative_numbers: settings.relative_numbers,
            show_whitespace: settings.show_whitespace,
            wrap: settings.wrap,
            highlight_current_line: settings.highlight_current_line,
            cursor_shape: settings.cursor_shape.clone(),
            scroll_off: settings.scroll_off,
            auto_indent: settings.auto_indent,
            highlight_word: settings.highlight_word,
            tree_open,
            tree_width,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font: default_font(),
            font_size: default_font_size(),
            theme: default_theme(),
            shell: String::new(),
            scrollback: default_scrollback(),
            padding: default_padding(),
            cursor_shape: CursorShape::default(),
            cursor_blink: default_cursor_blink(),
            visual_bell: default_visual_bell(),
            copy_on_select: true,
            desktop_notifications: true,
            ssh_keepalive: default_keepalive(),
            allow_osc52_read: false,
            log_sessions: false,
            auto_reconnect: false,
            opacity: default_opacity(),
            background_image: None,
            background_image_opacity: default_background_image_opacity(),
            background_fit: BackgroundFit::default(),
            colors: std::collections::BTreeMap::new(),
            window_size: None,
            window_pos: None,
            remember_secrets: true,
            profiles: Vec::new(),
            editor: EditorConfig::default(),
            editor_keys: std::collections::BTreeMap::new(),
            ssh_mode: false,
            favorites: Vec::new(),
            recent_hosts: Vec::new(),
            editor_root: None,
            view: default_view(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(APP_NAME)
            .join("config.toml")
    }

    /// Load config, tolerating a missing or malformed file (falls back to defaults).
    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("ignoring invalid config {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        // The config contains no secrets, but keep it private anyway.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

/// Parse `~/.ssh/config` into connection profiles.
///
/// Wildcard `Host` patterns are skipped (they are not concrete hosts), and
/// profiles without an `IdentityFile` default to ssh-agent authentication.
pub fn import_ssh_config() -> Vec<SshProfile> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let path = home.join(".ssh").join("config");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    parse_ssh_config(&text)
}

/// Parse OpenSSH client config text into profiles.
pub fn parse_ssh_config(text: &str) -> Vec<SshProfile> {
    let default_user = std::env::var("USER").unwrap_or_default();
    let mut profiles: Vec<SshProfile> = Vec::new();
    let mut current: Option<SshProfile> = None;

    let flush = |current: &mut Option<SshProfile>, profiles: &mut Vec<SshProfile>| {
        if let Some(mut p) = current.take() {
            if p.host.is_empty() {
                p.host = p.name.clone();
            }
            if p.key_path.is_none() {
                p.auth = AuthMethod::Agent;
            }
            profiles.push(p);
        }
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(|c: char| c == '=' || c.is_whitespace()) else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let value = value.trim().trim_matches('"').to_owned();
        if key == "host" {
            flush(&mut current, &mut profiles);
            if value.contains(['*', '?', '!']) {
                continue; // pattern, not a concrete host
            }
            current = Some(SshProfile {
                name: value,
                host: String::new(),
                port: default_port(),
                user: default_user.clone(),
                auth: AuthMethod::Key,
                key_path: None,
                proxy_jump: None,
                forwards: Vec::new(),
                remote_forwards: Vec::new(),
                socks_forwards: Vec::new(),
                agent_forwarding: false,
                group: None,
            });
            continue;
        }
        let Some(p) = current.as_mut() else {
            continue;
        };
        match key.as_str() {
            "hostname" => p.host = value,
            "user" => p.user = value,
            "port" => {
                if let Ok(port) = value.parse() {
                    p.port = port;
                }
            }
            "identityfile" => {
                p.key_path = Some(value);
                p.auth = AuthMethod::Key;
            }
            "proxyjump" => p.proxy_jump = Some(value),
            "localforward" => p.forwards.push(value),
            "remoteforward" => p.remote_forwards.push(value),
            "dynamicforward" => p.socks_forwards.push(value),
            "forwardagent" => {
                p.agent_forwarding = matches!(value.to_lowercase().as_str(), "yes" | "true")
            }
            _ => {}
        }
    }
    flush(&mut current, &mut profiles);
    profiles
}

/// Directory for session transcripts.
pub fn log_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_NAME)
        .join("logs")
}

/// Read/write access to `~/.ssh/known_hosts` for the settings UI.
pub mod known_hosts {
    use std::path::PathBuf;

    /// One trusted host key.
    #[derive(Clone, Debug)]
    pub struct Entry {
        /// 0-based line number in the file, used for removal.
        pub line: usize,
        /// Host pattern(s) as written (may be hashed).
        pub hosts: String,
        pub key_type: String,
        /// True when the host is stored hashed (`|1|…`).
        pub hashed: bool,
    }

    pub fn path() -> Option<PathBuf> {
        dirs::home_dir().map(|home| home.join(".ssh").join("known_hosts"))
    }

    /// List entries, skipping comments, blank lines and markers.
    pub fn list() -> Vec<Entry> {
        let Some(path) = path() else {
            return Vec::new();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        parse(&text)
    }

    /// Parse `known_hosts` text into entries.
    pub fn parse(text: &str) -> Vec<Entry> {
        text.lines()
            .enumerate()
            .filter_map(|(line, raw)| {
                let raw = raw.trim();
                if raw.is_empty() || raw.starts_with('#') || raw.starts_with('@') {
                    return None;
                }
                let mut parts = raw.split_whitespace();
                let hosts = parts.next()?.to_owned();
                let key_type = parts.next().unwrap_or_default().to_owned();
                Some(Entry {
                    line,
                    hashed: hosts.starts_with("|1|"),
                    hosts,
                    key_type,
                })
            })
            .collect()
    }

    /// Remove one entry (by line number) from `known_hosts`.
    pub fn forget(line: usize) -> std::io::Result<()> {
        let Some(path) = path() else {
            return Ok(());
        };
        let text = std::fs::read_to_string(&path)?;
        let mut out = String::new();
        for (i, raw) in text.lines().enumerate() {
            if i == line {
                continue;
            }
            out.push_str(raw);
            out.push('\n');
        }
        std::fs::write(&path, out)
    }
}

/// Secure credential storage backed by the OS keyring.
pub mod secrets {
    use super::KEYRING_SERVICE;
    use keyring::Error;

    /// Check whether the system keyring is usable.
    pub fn available() -> bool {
        match keyring::Entry::store_status() {
            Ok(()) => true,
            Err(e) => {
                log::warn!("system keyring unavailable: {e}");
                false
            }
        }
    }

    pub fn store_password(profile: &str, password: &str) -> keyring::Result<()> {
        keyring::Entry::new(KEYRING_SERVICE, &format!("password:{profile}"))?.set_password(password)
    }

    pub fn load_password(profile: &str) -> Option<String> {
        match keyring::Entry::new(KEYRING_SERVICE, &format!("password:{profile}"))
            .ok()?
            .get_password()
        {
            Ok(pw) => Some(pw),
            Err(Error::NoEntry) => None,
            Err(e) => {
                log::warn!("keyring read failed for {profile}: {e}");
                None
            }
        }
    }

    pub fn delete_password(profile: &str) {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &format!("password:{profile}")) {
            let _ = entry.delete_credential();
        }
    }

    pub fn store_passphrase(profile: &str, passphrase: &str) -> keyring::Result<()> {
        keyring::Entry::new(KEYRING_SERVICE, &format!("passphrase:{profile}"))?
            .set_password(passphrase)
    }

    pub fn load_passphrase(profile: &str) -> Option<String> {
        match keyring::Entry::new(KEYRING_SERVICE, &format!("passphrase:{profile}"))
            .ok()?
            .get_password()
        {
            Ok(pw) => Some(pw),
            Err(Error::NoEntry) => None,
            Err(e) => {
                log::warn!("keyring read failed for {profile}: {e}");
                None
            }
        }
    }

    pub fn delete_passphrase(profile: &str) {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &format!("passphrase:{profile}")) {
            let _ = entry.delete_credential();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_config_parses_hosts_and_options() {
        let text = "\
# a comment
Host prod
    HostName prod.example.com
    User deploy
    Port 2222
    IdentityFile ~/.ssh/id_ed25519
    ProxyJump bastion
    LocalForward 8080:localhost:5432
    RemoteForward 9000:localhost:3000
    DynamicForward 1080
    ForwardAgent yes

Host staging
    HostName staging.example.com

Host *.internal
    User ignored
";
        let profiles = parse_ssh_config(text);
        assert_eq!(profiles.len(), 2, "wildcard hosts must be skipped");

        let prod = &profiles[0];
        assert_eq!(prod.name, "prod");
        assert_eq!(prod.host, "prod.example.com");
        assert_eq!(prod.user, "deploy");
        assert_eq!(prod.port, 2222);
        assert_eq!(prod.auth, AuthMethod::Key);
        assert_eq!(prod.key_path.as_deref(), Some("~/.ssh/id_ed25519"));
        assert_eq!(prod.proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(prod.forwards, vec!["8080:localhost:5432".to_owned()]);
        assert_eq!(prod.remote_forwards, vec!["9000:localhost:3000".to_owned()]);
        assert_eq!(prod.socks_forwards, vec!["1080".to_owned()]);
        assert!(prod.agent_forwarding);

        // No IdentityFile -> ssh-agent, and the alias doubles as the host.
        let staging = &profiles[1];
        assert_eq!(staging.host, "staging.example.com");
        assert_eq!(staging.auth, AuthMethod::Agent);
        assert_eq!(staging.port, 22);
    }

    #[test]
    fn ssh_config_alias_without_hostname_uses_the_alias() {
        let profiles = parse_ssh_config("Host box\n  User root\n");
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].host, "box");
        assert_eq!(profiles[0].user, "root");
    }

    #[test]
    fn known_hosts_parse_skips_noise_and_flags_hashed_entries() {
        let text = "\
# comment
@cert-authority *.example.com ssh-rsa AAAA
plain.example.com ssh-ed25519 AAAAC3Nza
|1|abc=|def= ssh-rsa AAAAB3Nza

";
        let entries = known_hosts::parse(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].hosts, "plain.example.com");
        assert_eq!(entries[0].key_type, "ssh-ed25519");
        assert!(!entries[0].hashed);
        assert_eq!(entries[0].line, 2);
        assert!(entries[1].hashed);
        assert_eq!(entries[1].line, 3);
    }

    #[test]
    fn config_round_trips_through_toml() {
        let mut config = Config {
            font_size: 18.0,
            cursor_shape: CursorShape::Beam,
            cursor_blink: false,
            visual_bell: false,
            ssh_keepalive: 15,
            allow_osc52_read: true,
            ..Config::default()
        };
        config.profiles.push(SshProfile {
            name: "prod".to_owned(),
            host: "prod.example.com".to_owned(),
            port: 2222,
            user: "deploy".to_owned(),
            auth: AuthMethod::Agent,
            key_path: None,
            proxy_jump: Some("bastion".to_owned()),
            forwards: vec!["8080:localhost:5432".to_owned()],

            remote_forwards: Vec::new(),
            socks_forwards: Vec::new(),
            agent_forwarding: false,
            group: None,
        });

        let text = toml::to_string_pretty(&config).expect("serialize");
        let back: Config = toml::from_str(&text).expect("deserialize");
        assert_eq!(back.font_size, 18.0);
        assert_eq!(back.cursor_shape, CursorShape::Beam);
        assert!(!back.cursor_blink);
        assert!(!back.visual_bell);
        assert_eq!(back.ssh_keepalive, 15);
        assert!(back.allow_osc52_read);
        assert_eq!(back.profiles.len(), 1);
        assert_eq!(back.profiles[0].auth, AuthMethod::Agent);
        assert_eq!(back.profiles[0].proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(back.profiles[0].forwards.len(), 1);
        // The new switches survive the round trip too.
        assert!(back.copy_on_select);
        assert!(back.desktop_notifications);
    }

    #[test]
    fn config_defaults_apply_for_missing_fields() {
        // Older config files predate the cursor/keepalive options.
        let back: Config = toml::from_str("font = \"mono\"\n").expect("deserialize");
        assert_eq!(back.font, "mono");
        assert_eq!(back.cursor_shape, CursorShape::Block);
        assert!(back.cursor_blink);
        assert!(back.visual_bell);
        assert_eq!(back.ssh_keepalive, 30);
        assert!(!back.allow_osc52_read);
        // Copy-on-select and notifications default to on.
        assert!(back.copy_on_select);
        assert!(back.desktop_notifications);
    }

    #[test]
    fn cursor_shape_labels_are_stable() {
        assert_eq!(CursorShape::Block.label(), "block");
        assert_eq!(CursorShape::Beam.label(), "beam");
        assert_eq!(CursorShape::Underline.label(), "underline");
    }

    #[test]
    fn background_image_settings_round_trip() {
        let mut config = Config::default();
        assert_eq!(config.background_image, None);
        assert_eq!(config.background_fit, BackgroundFit::Cover);
        assert!(
            config.background_image_opacity > 0.0 && config.background_image_opacity < 1.0,
            "the default must show some of the image without drowning the text"
        );

        config.background_image = Some("~/Pictures/wall.png".to_owned());
        config.background_image_opacity = 0.6;
        config.background_fit = BackgroundFit::Contain;
        let text = toml::to_string(&config).expect("serialise");
        let back: Config = toml::from_str(&text).expect("parse");
        assert_eq!(
            back.background_image.as_deref(),
            Some("~/Pictures/wall.png")
        );
        assert_eq!(back.background_image_opacity, 0.6);
        assert_eq!(back.background_fit, BackgroundFit::Contain);
    }

    #[test]
    fn background_fit_labels_are_stable() {
        assert_eq!(BackgroundFit::Cover.label(), "cover");
        assert_eq!(BackgroundFit::Contain.label(), "contain");
        assert_eq!(BackgroundFit::Stretch.label(), "stretch");
    }
}
