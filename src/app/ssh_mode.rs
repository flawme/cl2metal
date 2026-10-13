//! SSH Mode: the workspace sidebar.
//!
//! A host list with search, groups, favourites, recents and live connection
//! status, plus the actions that manage profiles. The sidebar is a view over
//! the same `config.profiles` the rest of the application uses, so a host
//! edited here is the host the terminal connects to.

use super::*;

/// One row in the sidebar.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct HostRow {
    pub(crate) name: String,
    pub(crate) host: String,
    pub(crate) user: String,
    pub(crate) port: u16,
    pub(crate) group: Option<String>,
    pub(crate) favorite: bool,
    pub(crate) connected: bool,
    pub(crate) recent: bool,
}

/// What a click on a row should do.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum HostAction {
    Connect(String),
    Edit(String),
    Duplicate(String),
    Delete(String),
    ToggleFavorite(String),
    NewHost,
}

impl App {
    /// Turn SSH Mode on or off.
    pub(crate) fn toggle_ssh_mode(&mut self) {
        self.ssh_mode = !self.ssh_mode;
        self.config.ssh_mode = self.ssh_mode;
        self.persist_config();
        let state = if self.ssh_mode { "on" } else { "off" };
        self.toast = Some((format!("ssh mode {state}"), self.now));
        log::info!("ssh mode {state}");
    }

    /// The hosts shown in the sidebar, filtered by the search box.
    pub(crate) fn host_rows(&self) -> Vec<HostRow> {
        let query = self.host_query.trim().to_lowercase();
        let mut rows: Vec<HostRow> = self
            .config
            .profiles
            .iter()
            .filter(|profile| {
                query.is_empty()
                    || profile.name.to_lowercase().contains(&query)
                    || profile.host.to_lowercase().contains(&query)
                    || profile.user.to_lowercase().contains(&query)
                    || profile
                        .group
                        .as_deref()
                        .is_some_and(|g| g.to_lowercase().contains(&query))
            })
            .map(|profile| HostRow {
                name: profile.name.clone(),
                host: profile.host.clone(),
                user: profile.user.clone(),
                port: profile.port,
                group: profile.group.clone(),
                favorite: self.config.favorites.iter().any(|f| f == &profile.name),
                connected: self.is_connected(&profile.name),
                recent: self.config.recent_hosts.iter().any(|r| r == &profile.name),
            })
            .collect();
        // Favourites first, then recents, then by name.
        rows.sort_by(|a, b| {
            b.favorite
                .cmp(&a.favorite)
                .then_with(|| b.recent.cmp(&a.recent))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        rows
    }

    /// Whether any open tab is connected with this profile.
    pub(crate) fn is_connected(&self, profile: &str) -> bool {
        self.tabs.iter().any(|tab| {
            tab.panes.iter().any(|pane| {
                pane.ssh_profile.as_ref().is_some_and(|p| p.name == profile) && pane.dead.is_none()
            })
        })
    }

    /// Remember a host as recently used.
    pub(crate) fn note_recent(&mut self, profile: &str) {
        self.config.recent_hosts.retain(|r| r != profile);
        self.config.recent_hosts.insert(0, profile.to_owned());
        self.config.recent_hosts.truncate(10);
        self.persist_config();
    }

    /// Star or unstar a host.
    pub(crate) fn toggle_favorite(&mut self, profile: &str) {
        if let Some(at) = self.config.favorites.iter().position(|f| f == profile) {
            self.config.favorites.remove(at);
        } else {
            self.config.favorites.push(profile.to_owned());
        }
        self.persist_config();
    }

    /// Copy a profile under a new name.
    pub(crate) fn duplicate_profile(&mut self, name: &str) -> Option<String> {
        let profile = self
            .config
            .profiles
            .iter()
            .find(|p| p.name == name)?
            .clone();
        let mut copy = profile;
        let mut n = 2;
        loop {
            let candidate = format!("{name} ({n})");
            if !self.config.profiles.iter().any(|p| p.name == candidate) {
                copy.name = candidate.clone();
                break;
            }
            n += 1;
        }
        self.config.profiles.push(copy);
        self.persist_config();
        self.toast = Some((format!("duplicated {name}"), self.now));
        log::info!("ssh profile duplicated: {name}");
        Some(name.to_owned())
    }

    /// Delete a profile, with its keyring secret.
    pub(crate) fn delete_profile(&mut self, name: &str) {
        let before = self.config.profiles.len();
        self.config.profiles.retain(|p| p.name != name);
        if self.config.profiles.len() == before {
            return;
        }
        self.config.favorites.retain(|f| f != name);
        self.config.recent_hosts.retain(|r| r != name);
        // The stored password goes with it.
        crate::config::secrets::delete_password(name);
        self.persist_config();
        self.toast = Some((format!("deleted {name}"), self.now));
        log::info!("ssh profile deleted: {name}");
    }

    /// Connect to a saved profile.
    pub(crate) fn connect_profile(&mut self, name: &str) {
        let Some(profile) = self
            .config
            .profiles
            .iter()
            .find(|p| p.name == name)
            .cloned()
        else {
            self.toast = Some((format!("no profile named {name}"), self.now));
            return;
        };
        self.note_recent(name);
        self.spawn_ssh(profile);
    }

    /// The sidebar itself.
    pub(crate) fn ssh_sidebar(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let theme = self.theme.clone();
        let panel_alpha = self.panel_alpha();
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            0.0,
            ui::with_alpha(color32(theme.ui.panel_bg), panel_alpha),
        );
        painter.line_segment(
            [
                egui::pos2(rect.max.x, rect.min.y),
                egui::pos2(rect.max.x, rect.max.y),
            ],
            egui::Stroke::new(1.0, color32(theme.ui.border)),
        );

        let row_h = 30.0;
        let mut y = rect.min.y + 8.0;
        let left = rect.min.x + 10.0;
        let width = rect.width() - 20.0;

        // Header.
        painter.text(
            egui::pos2(left, y + 6.0),
            egui::Align2::LEFT_CENTER,
            "SSH",
            egui::FontId::new(13.0, egui::FontFamily::Monospace),
            color32(theme.ui.accent),
        );
        let new_rect =
            egui::Rect::from_min_size(egui::pos2(rect.max.x - 74.0, y), egui::vec2(64.0, 22.0));
        if ui
            .interact(new_rect, ui.id().with("ssh-new-host"), egui::Sense::click())
            .clicked()
        {
            self.open_new_host();
        }
        painter.rect_filled(new_rect, 4.0, color32(theme.bg));
        painter.text(
            new_rect.center(),
            egui::Align2::CENTER_CENTER,
            "+ host",
            egui::FontId::new(11.0, egui::FontFamily::Monospace),
            color32(theme.ui.text),
        );
        y += 30.0;

        // Search.
        let search_rect = egui::Rect::from_min_size(egui::pos2(left, y), egui::vec2(width, 24.0));
        ui.put(
            search_rect,
            egui::TextEdit::singleline(&mut self.host_query)
                .font(egui::FontId::new(12.0, egui::FontFamily::Monospace))
                .hint_text("search hosts"),
        );
        y += 32.0;

        // Rows.
        let rows = self.host_rows();
        let list = egui::Rect::from_min_max(
            egui::pos2(rect.min.x, y),
            egui::pos2(rect.max.x, rect.max.y - 30.0),
        );
        let visible = (list.height() / row_h).floor().max(1.0) as usize;
        let max_scroll = rows.len().saturating_sub(visible) as f32;
        self.ssh_scroll = self.ssh_scroll.clamp(0.0, max_scroll);
        let top = self.ssh_scroll.floor() as usize;

        let mut action: Option<HostAction> = None;
        let mut last_group: Option<Option<String>> = None;
        for (i, row) in rows.iter().enumerate().skip(top).take(visible) {
            let row_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x + 4.0, list.min.y + (i - top) as f32 * row_h),
                egui::vec2(rect.width() - 8.0, row_h - 2.0),
            );
            // A group heading when the group changes.
            if last_group.as_ref() != Some(&row.group) {
                last_group = Some(row.group.clone());
                painter.text(
                    egui::pos2(left, row_rect.min.y - 2.0),
                    egui::Align2::LEFT_BOTTOM,
                    row.group.clone().unwrap_or_else(|| "hosts".to_owned()),
                    egui::FontId::new(10.0, egui::FontFamily::Monospace),
                    color32(theme.ui.text_dim),
                );
            }
            let response =
                ui.interact(row_rect, ui.id().with(("ssh-row", i)), egui::Sense::click());
            if response.hovered() {
                painter.rect_filled(row_rect, 4.0, color32(theme.bg));
            }
            if response.clicked() {
                action = Some(HostAction::Connect(row.name.clone()));
            }
            if response.secondary_clicked() {
                action = Some(HostAction::Edit(row.name.clone()));
            }
            // A status dot: green when a session is open, dim otherwise.
            let dot = egui::pos2(row_rect.min.x + 8.0, row_rect.center().y);
            let color = if row.connected {
                color32(theme.ui.accent)
            } else {
                color32(theme.ui.text_dim).gamma_multiply(0.5)
            };
            painter.circle_filled(dot, 3.0, color);
            // A star for favourites.
            if row.favorite {
                painter.text(
                    egui::pos2(row_rect.max.x - 12.0, row_rect.center().y),
                    egui::Align2::CENTER_CENTER,
                    "★",
                    egui::FontId::new(11.0, egui::FontFamily::Monospace),
                    color32(theme.ui.accent),
                );
            }
            painter.text(
                egui::pos2(row_rect.min.x + 18.0, row_rect.center().y - 6.0),
                egui::Align2::LEFT_CENTER,
                &row.name,
                egui::FontId::new(12.0, egui::FontFamily::Monospace),
                color32(theme.ui.text),
            );
            painter.text(
                egui::pos2(row_rect.min.x + 18.0, row_rect.center().y + 7.0),
                egui::Align2::LEFT_CENTER,
                format!("{}@{}:{}", row.user, row.host, row.port),
                egui::FontId::new(10.0, egui::FontFamily::Monospace),
                color32(theme.ui.text_dim),
            );
        }
        if rows.is_empty() {
            painter.text(
                egui::pos2(left, list.min.y + 10.0),
                egui::Align2::LEFT_TOP,
                if self.config.profiles.is_empty() {
                    "no hosts yet — add one with + host"
                } else {
                    "no host matches the search"
                },
                egui::FontId::new(11.0, egui::FontFamily::Monospace),
                color32(theme.ui.text_dim),
            );
        }
        if ui
            .interact(list, ui.id().with("ssh-scroll"), egui::Sense::hover())
            .hovered()
        {
            let delta = ui.input(|i| i.smooth_scroll_delta.y);
            if delta.abs() > 0.5 {
                self.ssh_scroll = (self.ssh_scroll - delta / row_h).clamp(0.0, max_scroll);
            }
        }

        // Footer: the count and the mode switch.
        let footer =
            egui::Rect::from_min_size(egui::pos2(left, rect.max.y - 24.0), egui::vec2(width, 20.0));
        painter.text(
            footer.min,
            egui::Align2::LEFT_CENTER,
            format!("{} hosts", self.config.profiles.len()),
            egui::FontId::new(10.0, egui::FontFamily::Monospace),
            color32(theme.ui.text_dim),
        );

        if let Some(action) = action {
            self.run_host_action(action);
        }
    }

    /// Validate and store a profile from the editor.
    pub(crate) fn save_profile(&mut self, profile: SshProfile) -> Result<(), String> {
        let name = profile.name.trim().to_owned();
        if name.is_empty() {
            return Err("the host needs a name".to_owned());
        }
        if profile.host.trim().is_empty() {
            return Err("the host needs an address".to_owned());
        }
        if profile.user.trim().is_empty() {
            return Err("the host needs a user".to_owned());
        }
        if profile.port == 0 {
            return Err("the port must be between 1 and 65535".to_owned());
        }
        let mut profile = profile;
        profile.name = name.clone();
        profile.host = profile.host.trim().to_owned();
        profile.user = profile.user.trim().to_owned();
        match self.config.profiles.iter().position(|p| p.name == name) {
            Some(at) => self.config.profiles[at] = profile,
            None => self.config.profiles.push(profile),
        }
        self.persist_config();
        self.toast = Some((format!("saved {name}"), self.now));
        log::info!("ssh profile saved: {name}");
        Ok(())
    }

    /// Act on a sidebar click.
    pub(crate) fn run_host_action(&mut self, action: HostAction) {
        match action {
            HostAction::Connect(name) => self.connect_profile(&name),
            HostAction::Edit(name) => self.open_profile_editor(&name),
            HostAction::Duplicate(name) => {
                self.duplicate_profile(&name);
            }
            HostAction::Delete(name) => self.delete_profile(&name),
            HostAction::ToggleFavorite(name) => self.toggle_favorite(&name),
            HostAction::NewHost => self.open_new_host(),
        }
    }

    /// Open the profile editor on a new host.
    pub(crate) fn open_new_host(&mut self) {
        let profile = SshProfile {
            name: "new host".to_owned(),
            host: String::new(),
            port: 22,
            user: std::env::var("USER").unwrap_or_else(|_| "root".to_owned()),
            auth: AuthMethod::Agent,
            key_path: None,
            proxy_jump: None,
            forwards: Vec::new(),
            remote_forwards: Vec::new(),
            socks_forwards: Vec::new(),
            agent_forwarding: false,
            group: None,
        };
        self.overlay = Overlay::Profile {
            profile,
            is_new: true,
            error: None,
        };
    }

    /// Open the profile editor on an existing host.
    pub(crate) fn open_profile_editor(&mut self, name: &str) {
        let Some(profile) = self
            .config
            .profiles
            .iter()
            .find(|p| p.name == name)
            .cloned()
        else {
            return;
        };
        self.overlay = Overlay::Profile {
            profile,
            is_new: false,
            error: None,
        };
    }
}
