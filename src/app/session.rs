//! Tab lifecycle and the session message pump: spawning shells and SSH
//! sessions, reconnecting, closing, and draining their output.

use super::*;

impl App {
    pub(crate) fn open_log(&self, title: &str) -> Option<std::fs::File> {
        if !self.config.log_sessions {
            return None;
        }
        let dir = crate::config::log_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::warn!("session log dir: {e}");
            return None;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let safe: String = title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = dir.join(format!("{stamp}-{safe}.log"));
        match std::fs::File::create(&path) {
            Ok(file) => {
                log::info!("session log: {}", path.display());
                Some(file)
            }
            Err(e) => {
                log::warn!("session log {}: {e}", path.display());
                None
            }
        }
    }

    /// Build a local pane (shell + terminal core) without adding it anywhere.
    pub(crate) fn new_local_pane(
        &self,
        cols: usize,
        rows: usize,
        exec: Option<&str>,
        cwd: Option<std::path::PathBuf>,
    ) -> Result<Pane, String> {
        let (client_tx, msg_rx, shell_pid) = local::spawn(
            &self.config.shell,
            cols as u16,
            rows as u16,
            exec,
            cwd.as_deref(),
            self.waker.clone(),
        )
        .map_err(|e| format!("Failed to spawn shell: {e:#}"))?;
        let (term_tx, term_rx) = flume::unbounded();
        let mut core = TermCore::new(
            cols,
            rows,
            self.config.scrollback,
            cursor_style(&self.config),
            Listener::new(term_tx),
        );
        // The OSC 11 reply should match what the text actually sits on.
        core.set_background(self.theme.bg);
        let log = self.open_log(&shell_label(&self.config.shell));
        Ok(Pane {
            kind: TabKind::Local,
            profile: None,
            ssh_profile: None,
            shell_pid,
            core,
            client_tx,
            msg_rx,
            term_rx,
            dead: None,
            ready: true,
            cursor_blink: self.config.cursor_blink,
            log,
            reconnect_at: None,
            reconnect_attempts: 0,
            osc_title: None,
            marks_seen: 0,
            command_started_at: None,
        })
    }

    /// Build an SSH pane for a profile, without adding it anywhere.
    pub(crate) fn new_ssh_pane(
        &self,
        profile: &SshProfile,
        cols: usize,
        rows: usize,
    ) -> Result<Pane, String> {
        let params = self.ssh_params(profile, cols, rows);
        let (client_tx, msg_rx) =
            ssh::spawn(params, self.waker.clone()).map_err(|e| format!("SSH failed: {e:#}"))?;
        let (term_tx, term_rx) = flume::unbounded();
        let mut core = TermCore::new(
            cols,
            rows,
            self.config.scrollback,
            cursor_style(&self.config),
            Listener::new(term_tx),
        );
        // The OSC 11 reply should match what the text actually sits on.
        core.set_background(self.theme.bg);
        let log = self.open_log(&format!("{}@{}", profile.user, profile.host));
        Ok(Pane {
            kind: TabKind::Ssh,
            profile: Some(profile.name.clone()),
            ssh_profile: Some(profile.clone()),
            shell_pid: None,
            core,
            client_tx,
            msg_rx,
            term_rx,
            dead: None,
            ready: false,
            cursor_blink: self.config.cursor_blink,
            log,
            reconnect_at: None,
            reconnect_attempts: 0,
            osc_title: None,
            marks_seen: 0,
            command_started_at: None,
        })
    }

    /// A tab holding a single pane.
    fn single_pane_tab(pane: Pane, base_title: String) -> Tab {
        Tab {
            panes: vec![pane],
            tree: PaneNode::Leaf(0),
            active_pane: 0,
            base_title,
            title_override: None,
            opened_at: None,
            closing_at: None,
            unread: false,
        }
    }

    /// The focused local shell's directory, when it can be read.
    ///
    /// A new tab or split starts here, so `cd` in one pane is how you say
    /// where the next one should open. SSH panes have no local directory,
    /// and a dead shell's pid is stale, so both fall back to the launch
    /// directory.
    pub(crate) fn active_local_cwd(&self) -> Option<std::path::PathBuf> {
        let pane = self.tabs.get(self.active)?;
        let pid = pane.shell_pid?;
        if pane.dead.is_some() {
            return None;
        }
        let cwd = crate::term::local::shell_cwd(pid)?;
        (cwd.is_absolute() && cwd.exists()).then_some(cwd)
    }

    pub(crate) fn spawn_local(&mut self) {
        let (cols, rows) = self.grid_size();
        let exec = self.exec.take();
        // A new tab opens where the focused shell is, not at the launch
        // directory. The first tab has no focused shell yet, so it falls
        // back to the launch directory.
        let cwd = self.active_local_cwd();
        log::info!(
            "spawn_local: shell={:?} exec={:?} cwd={:?} grid={cols}x{rows}",
            self.config.shell,
            exec,
            cwd
        );
        match self.new_local_pane(cols, rows, exec.as_deref(), cwd) {
            Ok(pane) => {
                let title = shell_label(&self.config.shell);
                self.tabs.push(Self::single_pane_tab(pane, title));
                self.active = self.tabs.len() - 1;
            }
            Err(e) => {
                self.toast = Some((e, self.now));
            }
        }
    }

    /// Build SSH parameters for a profile, loading secrets from the keyring.
    pub(crate) fn ssh_params(&self, profile: &SshProfile, cols: usize, rows: usize) -> SshParams {
        let password = match profile.auth {
            AuthMethod::Password => secrets::load_password(&profile.name),
            _ => None,
        };
        let passphrase = match profile.auth {
            AuthMethod::Key => secrets::load_passphrase(&profile.name),
            _ => None,
        };
        let key_path = profile
            .key_path
            .as_deref()
            .map(expand_tilde)
            .filter(|_| profile.auth == AuthMethod::Key);
        SshParams {
            host: profile.host.clone(),
            port: profile.port,
            user: profile.user.clone(),
            auth: profile.auth,
            password,
            key_path,
            passphrase,
            proxy_jump: profile.proxy_jump.clone(),
            forwards: profile.forwards.clone(),
            remote_forwards: profile.remote_forwards.clone(),
            socks_forwards: profile.socks_forwards.clone(),
            agent_forwarding: profile.agent_forwarding,
            keepalive: self.config.ssh_keepalive,
            cols: cols as u16,
            rows: rows as u16,
        }
    }

    pub(crate) fn spawn_ssh(&mut self, profile: SshProfile) {
        let (cols, rows) = self.grid_size();
        let base_title = format!("{}@{}", profile.user, profile.host);
        match self.new_ssh_pane(&profile, cols, rows) {
            Ok(pane) => {
                self.tabs.push(Self::single_pane_tab(pane, base_title));
                self.active = self.tabs.len() - 1;
            }
            Err(e) => {
                self.toast = Some((e, self.now));
            }
        }
    }

    /// Connect to an ad-hoc `user@host[:port]` spec without saving a profile.
    pub(crate) fn connect_adhoc(&mut self, spec: &str) -> Result<(), String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("enter user@host".to_owned());
        }
        let (user, host_port) = match spec.split_once('@') {
            Some((user, rest)) => (user.to_owned(), rest),
            None => (
                std::env::var("USER")
                    .or_else(|_| std::env::var("USERNAME"))
                    .unwrap_or_else(|_| "root".to_owned()),
                spec,
            ),
        };
        let (host, port) = match host_port.rsplit_once(':') {
            Some((host, port)) => (
                host.to_owned(),
                port.parse::<u16>()
                    .map_err(|_| format!("invalid port: {port}"))?,
            ),
            None => (host_port.to_owned(), 22),
        };
        if host.is_empty() {
            return Err("enter a host".to_owned());
        }
        if user.is_empty() {
            return Err("enter a user".to_owned());
        }
        // Prefer the agent when one is running, otherwise prompt for a password.
        let auth = if std::env::var_os("SSH_AUTH_SOCK").is_some() {
            AuthMethod::Agent
        } else {
            AuthMethod::Password
        };
        self.spawn_ssh(SshProfile {
            name: format!("{user}@{host}"),
            host,
            port,
            user,
            auth,
            key_path: None,
            proxy_jump: None,
            forwards: Vec::new(),
            remote_forwards: Vec::new(),
            socks_forwards: Vec::new(),
            agent_forwarding: false,
            group: None,
        });
        Ok(())
    }

    /// Re-open a dead pane's session in place.
    pub(crate) fn reconnect_pane(&mut self, tab_i: usize, pane_i: usize) {
        let Some(tab) = self.tabs.get(tab_i) else {
            return;
        };
        let Some(pane) = tab.panes.get(pane_i) else {
            return;
        };
        let kind = pane.kind;
        let ssh_profile = pane.ssh_profile.clone();
        let profile_name = pane.profile.clone();
        let (cols, rows) = self.grid_size();

        // Build the new session first, so a failure leaves the pane intact.
        // The dead shell's pid is stale, so the replacement starts at the
        // launch directory.
        let built = match kind {
            TabKind::Local => self.new_local_pane(cols, rows, None, None),
            TabKind::Ssh => {
                // Prefer the pane's own profile (works for ad-hoc sessions),
                // falling back to a config lookup by name.
                let profile = ssh_profile.or_else(|| {
                    let name = profile_name.as_deref()?;
                    self.config
                        .profiles
                        .iter()
                        .find(|p| p.name == name)
                        .cloned()
                });
                match profile {
                    Some(profile) => self.new_ssh_pane(&profile, cols, rows),
                    None => {
                        self.toast = Some(("profile no longer exists".to_owned(), self.now));
                        return;
                    }
                }
            }
        };

        match built {
            Ok(new) => {
                let pane = &mut self.tabs[tab_i].panes[pane_i];
                let kind = new.kind;
                *pane = new;
                pane.ready = kind == TabKind::Local;
            }
            Err(e) => {
                self.toast = Some((format!("Reconnect failed: {e}"), self.now));
            }
        }
    }

    /// Re-open the focused pane of a tab (the automatic-reconnect path).
    pub(crate) fn reconnect_tab(&mut self, i: usize) {
        let pane = self.tabs.get(i).map(|t| t.active_pane).unwrap_or(0);
        self.reconnect_pane(i, pane);
    }

    /// Split the focused pane, giving the new one a fresh session.
    pub(crate) fn split_pane(&mut self, dir: SplitDir) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let target = tab.active_pane;
        let kind = tab.panes[target].kind;
        let profile = tab.panes[target].ssh_profile.clone();
        let (cols, rows) = self.grid_size();
        // The split starts where the pane being split is.
        let cwd = self.active_local_cwd();
        let built = match kind {
            TabKind::Local => self.new_local_pane(cols, rows, None, cwd),
            TabKind::Ssh => match profile {
                Some(profile) => self.new_ssh_pane(&profile, cols, rows),
                None => self.new_local_pane(cols, rows, None, cwd),
            },
        };
        let pane = match built {
            Ok(pane) => pane,
            Err(e) => {
                self.toast = Some((e, self.now));
                return;
            }
        };
        let tab = &mut self.tabs[self.active];
        let new_index = tab.panes.len();
        tab.panes.push(pane);
        if !tab.tree.split_leaf(target, new_index, dir) {
            // The tree did not know the pane: keep it visible anyway.
            tab.panes.pop();
            return;
        }
        tab.active_pane = new_index;
        log::info!(
            "split pane: tab={} pane={new_index} dir={dir:?}",
            self.active
        );
    }

    /// Even out the focused tab's panes (tmux's `even-*` layouts).
    pub(crate) fn even_panes(&mut self, mode: EvenMode) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if tab.panes.len() <= 1 {
            self.toast = Some(("only one pane".to_owned(), self.now));
            return;
        }
        let count = tab.panes.len();
        tab.tree.even_out(mode);
        log::info!("evened out {count} panes ({mode:?})");
    }

    /// Close the focused pane. The last pane closes the tab instead.
    /// The processes still running in a pane, for the close confirmation.
    ///
    /// A local pane asks its shell's living children — the VS Code
    /// `confirmOnKill` model, where an idle shell never asks and `npm` or
    /// an editor always does. An SSH pane counts as running while its
    /// session is alive: the work is on the far side, where `/proc` cannot
    /// see it, and closing would drop the connection.
    pub(crate) fn pane_jobs(&self, pane: &Pane) -> Vec<String> {
        if pane.dead.is_some() {
            return Vec::new();
        }
        match pane.kind {
            TabKind::Ssh => vec!["ssh".to_owned()],
            TabKind::Local => pane
                .shell_pid
                .map_or_else(Vec::new, crate::term::local::running_children),
        }
    }

    /// Every process running anywhere in the window, for the window close.
    pub(crate) fn all_jobs(&self) -> Vec<String> {
        let mut jobs: Vec<String> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .flat_map(|pane| self.pane_jobs(pane))
            .collect();
        jobs.sort();
        jobs.dedup();
        jobs
    }

    pub(crate) fn close_pane(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if tab.panes.len() <= 1 {
            let active = self.active;
            self.close_tab(active, ctx, "close pane (last one)");
            return;
        }
        let target = tab.active_pane;
        // A pane still doing work asks first.
        if !self.force_close {
            let jobs = self.pane_jobs(&self.tabs[self.active].panes[target]);
            if !jobs.is_empty() {
                self.overlay = Overlay::CloseConfirm {
                    scope: CloseScope::Pane {
                        tab: self.active,
                        pane: target,
                    },
                    jobs,
                };
                return;
            }
        }
        let tab = &mut self.tabs[self.active];
        let _ = tab.panes[target].client_tx.send(ClientMsg::Close);
        tab.panes.remove(target);
        tab.tree.remove_leaf(target);
        // Renumber the surviving panes in the tree.
        let map: Vec<Option<usize>> = (0..tab.panes.len() + 1)
            .map(|old| match old.cmp(&target) {
                std::cmp::Ordering::Less => Some(old),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(old - 1),
            })
            .collect();
        tab.tree.remap(&map);
        tab.active_pane = target.min(tab.panes.len() - 1);
        debug_assert_eq!(
            tab.tree.leaves().len(),
            tab.panes.len(),
            "the pane tree must match the pane list"
        );
        log::info!(
            "closed pane: tab={} pane={target} remaining={}",
            self.active,
            tab.panes.len()
        );
    }

    /// Focus the pane under a point, if any. Returns the pane index.
    pub(crate) fn focus_pane_at(&mut self, pos: egui::Pos2) -> Option<usize> {
        let hit = self
            .pane_layouts
            .iter()
            .find(|(_, l)| l.rect.contains(pos))
            .map(|(i, _)| *i)?;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.active_pane = hit;
        }
        Some(hit)
    }

    /// Close a tab: the session is killed at once, the header shrinks out for
    /// `TAB_ANIM_SECS` (Tabby's `:leave`), then the tab is dropped.
    pub(crate) fn close_tab(&mut self, i: usize, ctx: &egui::Context, reason: &str) {
        if i >= self.tabs.len() {
            return;
        }
        // Any pane in the tab still doing work asks first.
        if !self.force_close {
            let mut jobs: Vec<String> = self.tabs[i]
                .panes
                .iter()
                .flat_map(|pane| self.pane_jobs(pane))
                .collect();
            jobs.sort();
            jobs.dedup();
            if !jobs.is_empty() {
                self.overlay = Overlay::CloseConfirm {
                    scope: CloseScope::Tab(i),
                    jobs,
                };
                return;
            }
        }
        log::info!(
            "close_tab({i}) reason={reason} tabs_before={} active={}",
            self.tabs.len(),
            self.active
        );
        for pane in &mut self.tabs[i].panes {
            let _ = pane.client_tx.send(ClientMsg::Close);
            pane.reconnect_at = None;
        }
        self.tabs[i].closing_at = Some(self.now);

        // Remember the session so `Ctrl+Shift+R` can bring it back.
        let tab = &self.tabs[i];
        self.closed.push(ClosedTab {
            kind: tab.kind,
            profile: tab.profile.clone(),
            ssh_profile: tab.ssh_profile.clone(),
            title_override: tab.title_override.clone(),
            base_title: tab.base_title.clone(),
        });
        if self.closed.len() > CLOSED_CAP {
            self.closed.remove(0);
        }

        // The selection has to leave the closing tab immediately.
        if self.active == i {
            match self
                .tabs
                .iter()
                .enumerate()
                .find(|(j, t)| *j != i && t.closing_at.is_none())
                .map(|(j, _)| j)
            {
                Some(j) => self.active = j,
                None => {
                    log::info!("last tab closed -> closing the window");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    /// Re-open the most recently closed tab (`Ctrl+Shift+R`).
    ///
    /// The session is started fresh; scrollback is not restored.
    pub(crate) fn reopen_closed_tab(&mut self) {
        let Some(closed) = self.closed.pop() else {
            self.toast = Some(("no closed tab to reopen".to_owned(), self.now));
            return;
        };
        match closed.kind {
            TabKind::Local => self.spawn_local(),
            TabKind::Ssh => {
                let profile = closed.ssh_profile.clone().or_else(|| {
                    let name = closed.profile.as_deref()?;
                    self.config
                        .profiles
                        .iter()
                        .find(|p| p.name == name)
                        .cloned()
                });
                match profile {
                    Some(profile) => self.spawn_ssh(profile),
                    None => {
                        self.toast = Some(("profile no longer exists".to_owned(), self.now));
                        return;
                    }
                }
            }
        }
        // Restore the title the tab had, including a user rename.
        if let Some(tab) = self.tabs.last_mut() {
            tab.title_override = closed.title_override.clone();
            if tab.title_override.is_none() {
                tab.base_title = closed.base_title.clone();
            }
        }
        log::info!(
            "reopened closed tab: {} ({:?})",
            closed.base_title,
            closed.kind
        );
    }

    // -----------------------------------------------------------------------
    // Activity: unread flags and desktop notifications
    // -----------------------------------------------------------------------

    /// Clear the active tab's unread flag, and turn finished commands into
    /// desktop notifications while the window is unfocused.
    ///
    /// Timing comes from shell integration: a `C` mark starts the clock and a
    /// `D` mark stops it, so a command that ran for less than
    /// `NOTIFY_MIN_SECS` never interrupts.
    pub(crate) fn update_tab_activity(&mut self, focused: bool) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.unread = false;
        }
        let now = self.now;
        let enabled = self.config.desktop_notifications;
        let mut notices: Vec<(String, String)> = Vec::new();
        for tab in &mut self.tabs {
            let title = tab.display_title().to_owned();
            for pane in &mut tab.panes {
                let marks: Vec<PromptMark> = pane.core.marks().to_vec();
                if marks.len() < pane.marks_seen {
                    // The mark list was trimmed; start over rather than re-fire.
                    pane.marks_seen = 0;
                }
                for mark in &marks[pane.marks_seen.min(marks.len())..] {
                    match mark.kind {
                        MarkKind::Command => pane.command_started_at = Some(now),
                        MarkKind::Done => {
                            let Some(start) = pane.command_started_at.take() else {
                                continue;
                            };
                            let secs = now - start;
                            if enabled && secs >= NOTIFY_MIN_SECS {
                                let exit = match mark.exit {
                                    Some(0) | None => String::new(),
                                    Some(code) => format!(" (exit {code})"),
                                };
                                notices.push((
                                    title.clone(),
                                    format!("command finished after {secs:.0}s{exit}"),
                                ));
                            }
                        }
                        _ => {}
                    }
                }
                pane.marks_seen = marks.len();
            }
        }
        if focused {
            return;
        }
        for (title, body) in notices {
            self.notify_desktop(&title, &body);
        }
    }

    /// Fire a desktop notification. Best effort: never blocks the frame, and a
    /// missing notifier is only a log line.
    pub(crate) fn notify_desktop(&self, title: &str, body: &str) {
        log::info!("desktop notification: {title} — {body}");
        match crate::platform::notification_command(title, body).spawn() {
            // Reap the child off-thread: dropping it would leave a zombie.
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => log::warn!("desktop notification unavailable: {e}"),
        }
    }

    /// Drop tabs whose leave animation has finished.
    pub(crate) fn sweep_closing_tabs(&mut self, ctx: &egui::Context) {
        if !self.tabs.iter().any(|t| t.closing_at.is_some()) {
            return;
        }
        let now = self.now;
        let done = |t: &Tab| t.closing_at.is_some_and(|at| now - at >= TAB_ANIM_SECS);
        if !self.tabs.iter().any(done) {
            // Still animating: keep the frames coming.
            ctx.request_repaint();
            return;
        }
        // Recompute the selection over the tabs that survive the sweep.
        let new_active = if self.tabs.get(self.active).is_some_and(|t| !done(t)) {
            self.tabs[..self.active].iter().filter(|t| !done(t)).count()
        } else {
            self.tabs.iter().position(|t| !done(t)).unwrap_or(0)
        };
        self.tabs.retain(|t| !done(t));
        self.active = new_active.min(self.tabs.len().saturating_sub(1));
        ctx.request_repaint();
    }

    // -----------------------------------------------------------------------
    // Message pump
    // -----------------------------------------------------------------------

    pub(crate) fn drain(&mut self, ctx: &egui::Context) {
        let mut repaint = false;
        for tab_i in 0..self.tabs.len() {
            let pane_count = self.tabs[tab_i].panes.len();
            for pane_i in 0..pane_count {
                let mut msgs = Vec::new();
                {
                    let pane = &self.tabs[tab_i].panes[pane_i];
                    while let Ok(m) = pane.msg_rx.try_recv() {
                        msgs.push(m);
                    }
                    while let Ok(m) = pane.term_rx.try_recv() {
                        msgs.push(m);
                    }
                }
                for msg in msgs {
                    if self.handle_msg(tab_i, pane_i, msg, ctx) {
                        repaint = true;
                    }
                }
            }
        }
        if repaint {
            ctx.request_repaint();
        }
    }

    /// Returns true when a repaint is needed.
    pub(crate) fn handle_msg(
        &mut self,
        tab_i: usize,
        pane_i: usize,
        msg: SessionMsg,
        ctx: &egui::Context,
    ) -> bool {
        match msg {
            SessionMsg::Data(bytes) => {
                if self.ready_at.is_none() {
                    self.ready_at = Some(self.now);
                    if let Some(start) = self.entry_start {
                        log::info!(
                            "first terminal output {:.2}s after the first frame",
                            self.now - start
                        );
                    }
                }
                let pane = &mut self.tabs[tab_i].panes[pane_i];
                pane.core.feed(&bytes);
                // Capability queries the program asked: answer them on the
                // same channel the input goes out on — the local pty or the
                // SSH session.
                for response in pane.core.take_responses() {
                    let _ = pane.client_tx.send(ClientMsg::Input(response));
                }
                pane.ready = true;
                pane.reconnect_attempts = 0;
                if let Some(log) = pane.log.as_mut() {
                    use std::io::Write;
                    let _ = log.write_all(&bytes);
                }
                // Output in the background: flag the tab as unread.
                if tab_i != self.active {
                    self.tabs[tab_i].unread = true;
                }
                true
            }
            SessionMsg::Title(t) => {
                self.tabs[tab_i].panes[pane_i].osc_title =
                    if t.is_empty() { None } else { Some(t) };
                false
            }
            SessionMsg::Bell => {
                self.bell_at = Some(self.now);
                self.config.visual_bell
            }
            SessionMsg::CursorBlink => {
                let pane = &mut self.tabs[tab_i].panes[pane_i];
                pane.cursor_blink = pane.core.term.cursor_style().blinking;
                true
            }
            SessionMsg::ClipboardStore(text) => {
                ctx.copy_text(text);
                false
            }
            SessionMsg::ClipboardLoad(fmt) => {
                // OSC 52 reads are opt-in: a program must not be able to read
                // the clipboard without the user allowing it.
                let text = if self.config.allow_osc52_read {
                    self.read_clipboard().unwrap_or_default()
                } else {
                    String::new()
                };
                let reply = fmt(&text);
                let _ = self.tabs[tab_i].panes[pane_i]
                    .client_tx
                    .send(ClientMsg::Input(reply.into_bytes()));
                false
            }
            SessionMsg::PtyWrite(s) => {
                let _ = self.tabs[tab_i].panes[pane_i]
                    .client_tx
                    .send(ClientMsg::Input(s.into_bytes()));
                false
            }
            SessionMsg::ColorRequest(index, fmt) => {
                let c = match index {
                    256 => self.theme.fg,
                    257 => self.theme.bg,
                    258 => self.theme.cursor,
                    i if i < 16 => self.theme.ansi[i],
                    i if i < 256 => self.theme.indexed(i as u8),
                    _ => self.theme.fg,
                };
                let reply = fmt(theme::to_vte(c));
                let _ = self.tabs[tab_i].panes[pane_i]
                    .client_tx
                    .send(ClientMsg::Input(reply.into_bytes()));
                false
            }
            SessionMsg::SizeRequest(fmt) => {
                let (cw, ch) = match &self.layout {
                    Some(l) => (l.cell_px[0], l.cell_px[1]),
                    None => (8.0, 16.0),
                };
                let size = WindowSize {
                    num_lines: self.tabs[tab_i].panes[pane_i].core.rows() as u16,
                    num_cols: self.tabs[tab_i].panes[pane_i].core.cols() as u16,
                    cell_width: cw as u16,
                    cell_height: ch as u16,
                };
                let reply = fmt(size);
                let _ = self.tabs[tab_i].panes[pane_i]
                    .client_tx
                    .send(ClientMsg::Input(reply.into_bytes()));
                false
            }
            SessionMsg::SshRequest(req, reply) => {
                let profile = self.tabs[tab_i].panes[pane_i].profile.clone();
                self.prompt_text.clear();
                self.prompts.push_back(ui::prompt_from_request(
                    PromptSource::Tab(tab_i),
                    req,
                    reply,
                    profile,
                ));
                ctx.request_repaint();
                false
            }
            SessionMsg::Exited(reason) => {
                log::info!("tab {tab_i} pane {pane_i} session exited: {reason}");
                let pane = &mut self.tabs[tab_i].panes[pane_i];
                pane.dead = Some(reason);
                // Retry dropped SSH sessions automatically, with a short backoff.
                if self.config.auto_reconnect
                    && pane.kind == TabKind::Ssh
                    && pane.reconnect_attempts < 5
                {
                    pane.reconnect_attempts += 1;
                    pane.reconnect_at = Some(self.now + 3.0 * pane.reconnect_attempts as f64);
                }
                true
            }
            SessionMsg::Notice(text) => {
                self.toast = Some((text, self.now));
                true
            }
        }
    }

    // -----------------------------------------------------------------------
    // Input
    // -----------------------------------------------------------------------

    pub(crate) fn send_input(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if let Some(tab) = self.tabs.get(self.active)
            && tab.dead.is_none()
        {
            let _ = tab.client_tx.send(ClientMsg::Input(bytes));
        }
    }

    pub(crate) fn selection_text(&self) -> Option<String> {
        self.tabs
            .get(self.active)
            .and_then(|t| t.core.term.selection_to_string())
    }
}
