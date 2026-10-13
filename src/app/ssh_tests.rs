//! Tests for the SSH workspace and the SFTP panel's pure helpers.

use super::sftp_panel::human_size;
use super::ssh_mode::HostAction;
use super::*;

/// An app-less profile, for the pure helpers.
fn profile(name: &str, host: &str, group: Option<&str>) -> SshProfile {
    SshProfile {
        name: name.to_owned(),
        host: host.to_owned(),
        port: 22,
        user: "root".to_owned(),
        auth: AuthMethod::Agent,
        key_path: None,
        proxy_jump: None,
        forwards: Vec::new(),
        remote_forwards: Vec::new(),
        socks_forwards: Vec::new(),
        agent_forwarding: false,
        group: group.map(str::to_owned),
    }
}

#[test]
fn human_size_reads_in_binary_units() {
    assert_eq!(human_size(0), "0 B");
    assert_eq!(human_size(999), "999 B");
    assert_eq!(human_size(1024), "1.0 KiB");
    assert_eq!(human_size(1536), "1.5 KiB");
    assert_eq!(human_size(1024 * 1024), "1.0 MiB");
    assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GiB");
}

#[test]
fn sftp_paths_join_and_walk_up() {
    use crate::term::sftp::{join_remote, parent_remote};
    assert_eq!(join_remote("/", "etc"), "/etc");
    assert_eq!(join_remote("/etc", "hosts"), "/etc/hosts");
    assert_eq!(join_remote("/etc/", "hosts"), "/etc/hosts");
    assert_eq!(parent_remote("/"), "/");
    assert_eq!(parent_remote("/etc"), "/");
    assert_eq!(parent_remote("/etc/hosts"), "/etc");
}

#[test]
fn sftp_modes_render_like_ls() {
    use crate::term::sftp::format_mode;
    assert_eq!(format_mode(0o040755), "drwxr-xr-x");
    assert_eq!(format_mode(0o100644), "-rw-r--r--");
    assert_eq!(format_mode(0o100600), "-rw-------");
    // setuid and sticky.
    assert!(format_mode(0o104755).starts_with("-rws"));
    assert!(format_mode(0o041777).starts_with("drwxrwxrwt"));
}

#[test]
fn host_actions_name_the_profile_they_act_on() {
    // The sidebar's actions carry the profile name, so a click cannot act on
    // the wrong host.
    let action = HostAction::Connect("prod".to_owned());
    assert_eq!(action, HostAction::Connect("prod".to_owned()));
    assert_ne!(action, HostAction::Connect("staging".to_owned()));
}

#[test]
fn editor_settings_round_trip_through_the_config() {
    let config = crate::config::EditorConfig {
        tab_width: 2,
        expand_tabs: false,
        line_numbers: false,
        relative_numbers: true,
        show_whitespace: true,
        wrap: true,
        highlight_current_line: false,
        cursor_shape: "bar".to_owned(),
        scroll_off: 8,
        auto_indent: false,
        highlight_word: false,
        tree_open: false,
        tree_width: 300.0,
    };
    let settings = config.to_settings();
    assert_eq!(settings.tab_width, 2);
    assert!(!settings.expand_tabs);
    assert_eq!(settings.cursor_shape, "bar");
    let back = crate::config::EditorConfig::from_settings(&settings, false, 300.0);
    assert_eq!(back, config, "the settings survive a round trip");
}

#[test]
fn an_old_config_without_a_tree_width_still_loads() {
    // A config written before the tree column became resizable has no
    // `tree_width` line; it must load with the width the tree always had.
    let parsed: crate::config::EditorConfig = toml::from_str("tree_open = false").unwrap();
    assert_eq!(parsed.tree_width, 220.0, "the width it always was");
    assert!(!parsed.tree_open);
}

#[test]
fn editor_settings_clamp_absurd_values() {
    let config = crate::config::EditorConfig {
        tab_width: 0,
        scroll_off: 999,
        ..crate::config::EditorConfig::default()
    };
    let settings = config.to_settings();
    assert_eq!(settings.tab_width, 1, "a tab is at least one column");
    assert_eq!(settings.scroll_off, 20, "scrolloff is capped");
}

#[test]
fn a_profile_keeps_its_group_and_forwards_through_toml() {
    let mut config = Config::default();
    let mut p = profile("bastion", "bastion.example.com", Some("work"));
    p.forwards = vec!["8080:localhost:80".to_owned()];
    p.socks_forwards = vec!["1080".to_owned()];
    p.agent_forwarding = true;
    config.profiles.push(p);
    let text = toml::to_string(&config).expect("serialises");
    let back: Config = toml::from_str(&text).expect("parses");
    let saved = &back.profiles[0];
    assert_eq!(saved.group.as_deref(), Some("work"));
    assert_eq!(saved.forwards, vec!["8080:localhost:80"]);
    assert_eq!(saved.socks_forwards, vec!["1080"]);
    assert!(saved.agent_forwarding);
}

#[test]
fn the_workspace_state_survives_a_config_round_trip() {
    let mut config = Config::default();
    config.ssh_mode = true;
    config.favorites = vec!["prod".to_owned()];
    config.recent_hosts = vec!["prod".to_owned(), "staging".to_owned()];
    config.view = "editor".to_owned();
    config
        .editor_keys
        .insert("save".to_owned(), "ctrl+alt+s".to_owned());
    let text = toml::to_string(&config).expect("serialises");
    let back: Config = toml::from_str(&text).expect("parses");
    assert!(back.ssh_mode);
    assert_eq!(back.favorites, vec!["prod"]);
    assert_eq!(back.recent_hosts.len(), 2);
    assert_eq!(back.view, "editor");
    assert_eq!(
        back.editor_keys.get("save").map(String::as_str),
        Some("ctrl+alt+s")
    );
}

#[test]
fn a_config_without_the_new_sections_still_loads() {
    // An older config.toml must keep working: every new field has a default.
    let text = r#"
font = "DejaVu Sans Mono"
font_size = 14.0
theme = "midnight"
shell = ""
scrollback = 10000
padding = [8.0, 6.0]
remember_secrets = true
profiles = []
"#;
    let config: Config = toml::from_str(text).expect("an old config parses");
    assert!(!config.ssh_mode);
    assert_eq!(config.view, "terminal");
    assert_eq!(config.editor.tab_width, 4);
    assert!(config.editor_keys.is_empty());
    assert!(config.favorites.is_empty());
}
