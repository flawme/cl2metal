//! The agent access broker: the socket server behind `cl2metal mcp`.
//!
//! One user-owned unix socket, one JSON object per line in each
//! direction. Connections are identified with `SO_PEERCRED` and refused
//! unless they belong to this user; every new session requires an Allow
//! click in the consent modal, and with command screening on, every
//! command that is not known read-only needs one too; every decision
//! lands in the audit log.
//! Credentials never cross the socket: the agent holds profile names
//! and opaque session ids, the keyring stays behind this process.
//!
//! Ownership is split cleanly with the UI: the broker never blocks the
//! frame loop (it queues `AgentEvent`s and answers travel back through
//! per-request channels), and the UI never blocks the broker (a modal
//! it never answers is a denial once the timeout passes).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::{Config, SshProfile};

/// How long the consent modal waits before the broker denies on the
/// user's behalf. The UI drops unanswered modals at the same age.
pub const CONSENT_TIMEOUT_SECS: f64 = 120.0;

/// What the broker needs the UI for. The App drains these between
/// frames; answers travel back through the reply channels embedded in
/// each event, so neither side ever blocks the other.
pub enum AgentEvent {
    /// A new session wants the human's Allow first.
    Consent(ConsentRequest),
    /// A headless session hit a question only the human can answer
    /// (host key, password, MFA). `profile` is the name the keyring
    /// remember-box keys on, exactly as a pane's tab is.
    Prompt {
        profile: String,
        request: crate::term::SshRequest,
        reply: std::sync::mpsc::Sender<Result<String, ()>>,
    },
}

/// One pending consent question, drawn by the UI.
pub struct ConsentRequest {
    /// The asking process, e.g. `code (pid 1234)`.
    pub requester: String,
    /// What the agent is asking for; the modal is built from it.
    pub kind: ConsentKind,
    /// The answer; the UI dropping it unanswered reads as denial.
    pub reply: tokio::sync::oneshot::Sender<ConsentAnswer>,
}

/// What a consent question is about.
pub enum ConsentKind {
    /// A new session on a profile.
    Connect {
        profile: String,
        /// `user@host`, as the session would connect.
        user_at_host: String,
    },
    /// One command in a session that already exists. The modal shows
    /// the exact command text.
    Exec {
        command: String,
        /// `user@host`, for "this runs as ...".
        user_at_host: String,
    },
    /// One file transfer on a session that already exists. The modal
    /// shows both paths and the direction.
    File {
        /// "upload" or "download".
        action: String,
        local_path: String,
        remote_path: String,
        /// `user@host`, for "this runs as ...".
        user_at_host: String,
    },
}

/// The human's decision on a consent question.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConsentAnswer {
    /// Yes, this once.
    Allow,
    /// No: the broker answers "consent denied".
    Deny,
    /// Yes — and stop asking for the rest of this session.
    AllowAll,
}

/// Live settings the UI can change while the broker runs. The socket
/// never comes down; these decide what each request is allowed to do,
/// so "off" answers with a clear refusal instead of a confusing
/// connection-refused, and "on" is instant.
pub(crate) struct Shared {
    enabled: AtomicBool,
    confirm_commands: AtomicBool,
    read_only: AtomicBool,
    default_timeout_ms: AtomicU64,
    keepalive: AtomicU64,
    profiles: Mutex<Vec<SshProfile>>,
    allow_profiles: Mutex<Option<Vec<String>>>,
}

impl Shared {
    fn from_config(config: &Config) -> Self {
        Self {
            enabled: AtomicBool::new(config.agent_api.enabled),
            confirm_commands: AtomicBool::new(config.agent_api.confirm_commands),
            read_only: AtomicBool::new(config.agent_api.read_only),
            default_timeout_ms: AtomicU64::new(config.agent_api.default_timeout_ms),
            keepalive: AtomicU64::new(config.ssh_keepalive),
            profiles: Mutex::new(config.profiles.clone()),
            allow_profiles: Mutex::new(config.agent_api.allow_profiles.clone()),
        }
    }

    /// Copy the live settings in (startup, and whenever the settings
    /// modal saves).
    fn refresh(&self, config: &Config) {
        self.enabled
            .store(config.agent_api.enabled, Ordering::Relaxed);
        self.confirm_commands
            .store(config.agent_api.confirm_commands, Ordering::Relaxed);
        self.read_only
            .store(config.agent_api.read_only, Ordering::Relaxed);
        self.default_timeout_ms
            .store(config.agent_api.default_timeout_ms, Ordering::Relaxed);
        self.keepalive
            .store(config.ssh_keepalive, Ordering::Relaxed);
        *self.profiles.lock().unwrap() = config.profiles.clone();
        *self.allow_profiles.lock().unwrap() = config.agent_api.allow_profiles.clone();
    }

    /// The settings gate. Ping and close pass without it; everything
    /// else is agent access and needs the switch on.
    fn gate(&self) -> Result<(), &'static str> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Err("agent access is turned off in cl2metal settings");
        }
        Ok(())
    }

    /// Whether exec requests are screened: read-only commands run,
    /// everything else needs the human.
    fn confirm_commands(&self) -> bool {
        self.confirm_commands.load(Ordering::Relaxed)
    }

    /// Whether only the curated read-only command set may run. Stricter
    /// than consent: the human cannot approve a write in this mode.
    fn read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    fn find_profile(&self, name: &str) -> Option<SshProfile> {
        self.profiles
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.name == name)
            .cloned()
    }

    /// Whether an agent may open this profile at all.
    fn allowed(&self, name: &str) -> bool {
        match &*self.allow_profiles.lock().unwrap() {
            None => true,
            Some(list) => list.iter().any(|allowed| allowed == name),
        }
    }

    /// What agents are told exists: only profiles they may use.
    fn advertised(&self) -> Vec<super::protocol::ProfileInfo> {
        self.profiles
            .lock()
            .unwrap()
            .iter()
            .filter(|p| self.allowed(&p.name))
            .map(|p| super::protocol::ProfileInfo {
                name: p.name.clone(),
                host: p.host.clone(),
                user: p.user.clone(),
                port: p.port,
            })
            .collect()
    }

    fn default_timeout(&self) -> u64 {
        self.default_timeout_ms.load(Ordering::Relaxed)
    }

    fn keepalive_secs(&self) -> u64 {
        self.keepalive.load(Ordering::Relaxed)
    }
}

/// The app's handle to the running broker.
pub struct Broker {
    events: flume::Receiver<AgentEvent>,
    shared: Arc<Shared>,
}

impl Broker {
    /// Every event waiting for the UI, oldest first. Bounded so a burst
    /// cannot eat the frame; the broker wakes the UI per event anyway.
    pub fn drain(&self) -> Vec<AgentEvent> {
        let mut drained = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            drained.push(event);
            if drained.len() == 64 {
                break;
            }
        }
        drained
    }

    /// Copy a freshly saved config in: the on/off switch, the profile
    /// list, the allowlist, the timeouts. The settings modal calls this
    /// on save.
    pub fn refresh(&self, config: &Config) {
        self.shared.refresh(config);
    }
}

// ---------------------------------------------------------------------------
// Unix socket server
// ---------------------------------------------------------------------------

mod broker {
    use std::collections::HashMap;
    use std::io;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{
        AgentEvent, CONSENT_TIMEOUT_SECS, ConsentAnswer, ConsentKind, ConsentRequest, Shared,
    };
    use crate::agent::audit::Audit;
    use crate::agent::guard;
    use crate::agent::protocol::{self, Op, Request, Response};
    use crate::agent::session::{self, AgentSession, SessionEvent};
    use crate::agent::transport;
    use crate::config::{AuthMethod, Config, SshProfile, secrets};
    use crate::term::Waker;
    use crate::term::ssh::SshParams;
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

    /// How long a connect may take end to end. Auth prompts are
    /// human-paced; this bound only catches a session that never comes
    /// back at all.
    const CONNECT_TIMEOUT_SECS: u64 = 600;
    /// Grace beyond an exec's own deadline, so a briefly busy session
    /// thread is not falsely reported as timed out.
    const EXEC_SLACK_MS: u64 = 2_000;
    /// Session-id salt: keeps ids unique across a fast open/close cycle.
    static SALT: AtomicU64 = AtomicU64::new(0);

    /// One audit line, never a failure: a poisoned lock just skips
    /// lines, matching the audit's own degrade-not-die contract.
    fn audit_line(audit: &Mutex<Audit>, what: &str) {
        if let Ok(mut log) = audit.lock() {
            log.record(what);
        }
    }

    /// Start the broker: bind the first usable endpoint and serve until the
    /// process ends. Returns `None` when nothing could be bound — the app
    /// runs on without agent access; that is never a startup failure.
    pub fn start(config: &Config, wake: Waker) -> Option<super::Broker> {
        let shared = Arc::new(Shared::from_config(config));
        let audit = Arc::new(Mutex::new(Audit::open()));
        let (events_tx, events_rx) = flume::unbounded::<AgentEvent>();
        let mut bound = None;
        for endpoint in transport::endpoints() {
            match transport::bind(&endpoint) {
                Ok(listener) => {
                    bound = Some((listener, endpoint));
                    break;
                }
                Err(e) => log::warn!("agent socket: cannot bind {}: {e}", endpoint.display()),
            }
        }
        let Some((listener, endpoint)) = bound else {
            log::warn!("agent socket: no bindable endpoint; agent access is off for this run");
            return None;
        };
        log::info!("agent socket listening at {}", endpoint.display());
        audit_line(&audit, &format!("listening at {}", endpoint.display()));
        crate::term::runtime().spawn(accept_loop(
            listener,
            shared.clone(),
            audit,
            events_tx,
            wake,
        ));
        Some(super::Broker {
            events: events_rx,
            shared,
        })
    }

    async fn accept_loop(
        listener: transport::Listener,
        shared: Arc<Shared>,
        audit: Arc<Mutex<Audit>>,
        events: flume::Sender<AgentEvent>,
        wake: Waker,
    ) {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(pair) => pair,
                Err(e) => {
                    log::warn!("agent socket: accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
            };
            // Identity before anything else: same user only. On Windows the
            // named pipe's default DACL is the same-user guarantee, so
            // `peer.uid` is None and the check is skipped.
            if let Some(uid) = peer.uid {
                if uid != transport::our_uid() {
                    audit_line(
                        &audit,
                        &format!("rejected uid {uid} (pid {}): not this user", peer.pid),
                    );
                    continue;
                }
            }
            let requester = transport::requester_name(peer.pid);
            audit_line(&audit, &format!("{requester} connected"));
            let conn = Conn {
                requester,
                shared: shared.clone(),
                audit: audit.clone(),
                events: events.clone(),
                wake: wake.clone(),
                sessions: HashMap::new(),
            };
            tokio::spawn(handle_conn(stream, conn));
        }
    }

    /// One live session on a connection, with what the command
    /// approval modal needs to say about it.
    struct LiveSession {
        session: AgentSession,
        /// `user@host`, shown on every approval this session asks for.
        user_at_host: String,
        /// The human clicked "allow all future commands": the rest of
        /// the session runs without further approvals.
        auto_allowed: bool,
    }

    /// One connection's state. Sessions live and die with it: an agent
    /// cannot outlive its socket, and one connection can never see ids
    /// minted on another.
    struct Conn {
        requester: String,
        shared: Arc<Shared>,
        audit: Arc<Mutex<Audit>>,
        events: flume::Sender<AgentEvent>,
        wake: Waker,
        sessions: HashMap<String, LiveSession>,
    }

    impl Conn {
        fn note(&self, what: &str) {
            audit_line(&self.audit, what);
        }

        /// Audit the refusal, then hand it back as the response.
        fn denied(&self, id: u64, op: &str, reason: &str) -> Response {
            self.note(&format!("denied {op} for {}: {reason}", self.requester));
            Response::error(id, reason)
        }

        /// Ask the human through the consent modal. `Err` means the
        /// window closed without an answer; a dropped reply channel —
        /// the app closing, a modal expiring — reads as a denial.
        async fn ask(&self, kind: ConsentKind) -> Result<ConsentAnswer, ()> {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let _ = self.events.send(AgentEvent::Consent(ConsentRequest {
                requester: self.requester.clone(),
                kind,
                reply: reply_tx,
            }));
            (self.wake)();
            match tokio::time::timeout(Duration::from_secs_f64(CONSENT_TIMEOUT_SECS), reply_rx)
                .await
            {
                Ok(Ok(answer)) => Ok(answer),
                Ok(Err(_)) => Ok(ConsentAnswer::Deny),
                Err(_) => Err(()),
            }
        }

        async fn handle(&mut self, id: u64, op: Op) -> Response {
            match op {
                Op::Ping => Response {
                    id,
                    ok: true,
                    ..Response::default()
                },
                Op::ListProfiles => {
                    if let Err(reason) = self.shared.gate() {
                        return self.denied(id, "list_profiles", reason);
                    }
                    Response {
                        id,
                        ok: true,
                        profiles: Some(self.shared.advertised()),
                        ..Response::default()
                    }
                }
                Op::Connect { profile } => self.connect(id, profile).await,
                Op::Exec {
                    session,
                    command,
                    timeout_ms,
                } => self.exec(id, session, command, timeout_ms).await,
                Op::Upload {
                    session,
                    local_path,
                    remote_path,
                } => self.transfer(id, session, local_path, remote_path, true).await,
                Op::Download {
                    session,
                    remote_path,
                    local_path,
                } => self.transfer(id, session, local_path, remote_path, false).await,
                Op::Sessions => {
                    if let Err(reason) = self.shared.gate() {
                        return self.denied(id, "sessions", reason);
                    }
                    let mut sessions: Vec<protocol::SessionInfo> = self
                        .sessions
                        .iter()
                        .map(|(id, live)| protocol::SessionInfo {
                            id: id.clone(),
                            user_at_host: live.user_at_host.clone(),
                            auto_allowed: live.auto_allowed,
                        })
                        .collect();
                    sessions.sort_by(|a, b| a.id.cmp(&b.id));
                    Response {
                        id,
                        ok: true,
                        sessions: Some(sessions),
                        ..Response::default()
                    }
                }
                Op::AuditTail { lines } => {
                    if let Err(reason) = self.shared.gate() {
                        return self.denied(id, "audit_tail", reason);
                    }
                    let count = lines
                        .unwrap_or(50)
                        .clamp(1, protocol::MAX_AUDIT_LINES);
                    Response {
                        id,
                        ok: true,
                        audit: Some(crate::agent::audit::tail(count)),
                        ..Response::default()
                    }
                }
                Op::Close { session } => self.close(id, session),
            }
        }

        async fn connect(&mut self, id: u64, profile_name: String) -> Response {
            if let Err(reason) = self.shared.gate() {
                return self.denied(id, "connect", reason);
            }
            let Some(profile) = self.shared.find_profile(&profile_name) else {
                return self.denied(id, "connect", &format!("no profile named {profile_name}"));
            };
            if !self.shared.allowed(&profile_name) {
                return self.denied(id, "connect", "that profile is not allowed for agents");
            }
            let user_at_host = format!("{}@{}", profile.user, profile.host);

            // The consent round-trip: ask the UI, wait for the human.
            match self
                .ask(ConsentKind::Connect {
                    profile: profile_name.clone(),
                    user_at_host: user_at_host.clone(),
                })
                .await
            {
                Err(()) => {
                    self.note(&format!(
                        "consent timeout for {} ({user_at_host})",
                        self.requester
                    ));
                    return Response::error(id, "consent timed out");
                }
                // The connect modal offers no Allow-all; treating one
                // as a plain Allow is the honest reading.
                Ok(ConsentAnswer::Allow | ConsentAnswer::AllowAll) => {}
                Ok(ConsentAnswer::Deny) => {
                    self.note(&format!(
                        "consent denied for {} ({user_at_host})",
                        self.requester
                    ));
                    return Response::error(id, "consent denied");
                }
            }

            let session_id = protocol::new_session_id(SALT.fetch_add(1, Ordering::Relaxed));
            let (ev_tx, ev_rx) = flume::unbounded::<SessionEvent>();
            let params = broker_params(&profile, self.shared.keepalive_secs());
            let (session, outcome_rx) =
                session::spawn(session_id.clone(), params, ev_tx, self.wake.clone());
            self.forward_prompts(profile_name, session_id.clone(), ev_rx);

            match tokio::time::timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS), outcome_rx).await
            {
                Ok(Ok(Ok(()))) => {
                    self.note(&format!(
                        "session {session_id} opened by {} as {user_at_host}",
                        self.requester
                    ));
                    self.sessions.insert(
                        session_id.clone(),
                        LiveSession {
                            session,
                            user_at_host: user_at_host.clone(),
                            auto_allowed: false,
                        },
                    );
                    Response {
                        id,
                        ok: true,
                        session: Some(session_id),
                        host: Some(profile.host),
                        user: Some(profile.user),
                        ..Response::default()
                    }
                }
                Ok(Ok(Err(reason))) => {
                    self.note(&format!("session for {} failed: {reason}", self.requester));
                    Response::error(id, reason)
                }
                // The one-shot died without an answer: the session
                // thread is gone; call it a failure.
                Ok(Err(_)) => Response::error(id, "session ended during connect"),
                Err(_) => {
                    // Dropping the handle ends the exec loop, which
                    // disconnects the socket underneath.
                    self.note(&format!(
                        "connect for {} timed out ({user_at_host})",
                        self.requester
                    ));
                    Response::error(id, "connect timed out")
                }
            }
        }

        /// Relay the session's human questions to the UI until the
        /// session ends. The session's own pump thread wakes on every
        /// event; the relay adds a wake after the hand-off because the
        /// UI's drain can beat it to the frame.
        fn forward_prompts(
            &self,
            profile: String,
            id: String,
            events_rx: flume::Receiver<SessionEvent>,
        ) {
            let ui = self.events.clone();
            let wake = self.wake.clone();
            let audit = self.audit.clone();
            tokio::spawn(async move {
                while let Ok(event) = events_rx.recv_async().await {
                    match event {
                        SessionEvent::Prompt(request, reply) => {
                            let _ = ui.send(AgentEvent::Prompt {
                                profile: profile.clone(),
                                request,
                                reply,
                            });
                            (wake)();
                        }
                        SessionEvent::Failed(reason) => {
                            audit_line(&audit, &format!("session {id} failed: {reason}"));
                        }
                        SessionEvent::Exited(reason) => {
                            audit_line(&audit, &format!("session {id} ended: {reason}"));
                            break;
                        }
                        SessionEvent::Connected => {}
                    }
                }
            });
        }

        async fn exec(
            &mut self,
            id: u64,
            session: String,
            command: String,
            timeout_ms: Option<u64>,
        ) -> Response {
            if let Err(reason) = self.shared.gate() {
                return self.denied(id, "exec", reason);
            }
            if !protocol::valid_session_id(&session) {
                return Response::error(id, "bad session id");
            }
            if let Err(reason) = protocol::validate_command(&command) {
                return self.denied(id, "exec", reason);
            }
            // Read-only mode is stricter than consent: the human cannot
            // approve a write, because the setting says writes are off.
            if self.shared.read_only()
                && guard::classify(&command) != guard::Verdict::AutoAllowed
            {
                return self.denied(id, "exec", "read-only mode: command needs write access");
            }
            let timeout_ms = protocol::clamp_timeout(timeout_ms, self.shared.default_timeout());
            let Some(live) = self.sessions.get(&session) else {
                return Response::error(id, "no such session");
            };
            let user_at_host = live.user_at_host.clone();
            let auto_allowed = live.auto_allowed;

            // Command screening: with the setting on, only known
            // read-only commands run without the human; one "allow all
            // future" click covers the rest of the session.
            if self.shared.confirm_commands()
                && !auto_allowed
                && guard::classify(&command) == guard::Verdict::NeedsConsent
            {
                match self
                    .ask(ConsentKind::Exec {
                        command: command.clone(),
                        user_at_host: user_at_host.clone(),
                    })
                    .await
                {
                    Err(()) => {
                        self.note(&format!(
                            "exec {session} by {}: consent timed out",
                            self.requester
                        ));
                        return Response::error(id, "consent timed out");
                    }
                    Ok(ConsentAnswer::Deny) => {
                        return self.denied(id, "exec", "consent denied");
                    }
                    Ok(ConsentAnswer::Allow) => {}
                    Ok(ConsentAnswer::AllowAll) => {
                        self.note(&format!(
                            "session {session}: {} approved all future commands",
                            self.requester
                        ));
                        if let Some(live) = self.sessions.get_mut(&session) {
                            live.auto_allowed = true;
                        }
                    }
                }
            }

            let Some(live) = self.sessions.get(&session) else {
                return Response::error(id, "session closed");
            };
            // The session thread enforces the deadline itself; the outer
            // wait only exists so a wedged thread cannot hang the
            // socket.
            let Some(reply) = live
                .session
                .exec(command.clone(), Duration::from_millis(timeout_ms))
            else {
                return Response::error(id, "session closed");
            };
            let wait = Duration::from_millis(timeout_ms + EXEC_SLACK_MS);
            match tokio::time::timeout(wait, reply).await {
                Ok(Ok(Ok(outcome))) => {
                    self.note(&format!(
                        "exec {session} by {}: exit {} ({} ms): {}",
                        self.requester, outcome.exit_code, timeout_ms, command
                    ));
                    Response {
                        id,
                        ok: true,
                        exit_code: Some(outcome.exit_code),
                        stdout: Some(outcome.stdout),
                        stderr: Some(outcome.stderr),
                        ..Response::default()
                    }
                }
                Ok(Ok(Err(reason))) => {
                    self.note(&format!(
                        "exec {session} by {} failed: {reason}",
                        self.requester
                    ));
                    Response::error(id, reason)
                }
                Ok(Err(_)) => Response::error(id, "session closed"),
                Err(_) => {
                    self.note(&format!("exec {session} by {} timed out", self.requester));
                    Response::error(id, "timed out")
                }
            }
        }

        /// Upload or download one file over the session's SFTP subsystem.
        /// File transfers are always consent-worthy: they read or write a
        /// file, and the modal shows both paths.
        async fn transfer(
            &mut self,
            id: u64,
            session: String,
            local_path: String,
            remote_path: String,
            upload: bool,
        ) -> Response {
            let op = if upload { "upload" } else { "download" };
            if let Err(reason) = self.shared.gate() {
                return self.denied(id, op, reason);
            }
            if !protocol::valid_session_id(&session) {
                return Response::error(id, "bad session id");
            }
            if let Err(reason) = protocol::validate_path(&local_path) {
                return self.denied(id, op, reason);
            }
            if let Err(reason) = protocol::validate_path(&remote_path) {
                return self.denied(id, op, reason);
            }
            if upload && self.shared.read_only() {
                return self.denied(id, op, "read-only mode: uploads are disabled");
            }
            let Some(live) = self.sessions.get(&session) else {
                return Response::error(id, "no such session");
            };
            let user_at_host = live.user_at_host.clone();
            let auto_allowed = live.auto_allowed;

            if !auto_allowed {
                match self
                    .ask(ConsentKind::File {
                        action: op.to_owned(),
                        local_path: local_path.clone(),
                        remote_path: remote_path.clone(),
                        user_at_host: user_at_host.clone(),
                    })
                    .await
                {
                    Err(()) => {
                        self.note(&format!(
                            "{op} {session} by {}: consent timed out",
                            self.requester
                        ));
                        return Response::error(id, "consent timed out");
                    }
                    Ok(ConsentAnswer::Deny) => {
                        return self.denied(id, op, "consent denied");
                    }
                    Ok(ConsentAnswer::Allow) => {}
                    Ok(ConsentAnswer::AllowAll) => {
                        self.note(&format!(
                            "session {session}: {} approved all future commands",
                            self.requester
                        ));
                        if let Some(live) = self.sessions.get_mut(&session) {
                            live.auto_allowed = true;
                        }
                    }
                }
            }

            let Some(live) = self.sessions.get(&session) else {
                return Response::error(id, "session closed");
            };
            let reply = if upload {
                live.session.upload(local_path.clone(), remote_path.clone())
            } else {
                live.session.download(remote_path.clone(), local_path.clone())
            };
            let Some(reply) = reply else {
                return Response::error(id, "session closed");
            };
            // A transfer can take longer than an exec; give it the protocol's
            // maximum timeout plus the same slack.
            let wait = Duration::from_millis(protocol::MAX_TIMEOUT_MS + EXEC_SLACK_MS);
            match tokio::time::timeout(wait, reply).await {
                Ok(Ok(Ok(outcome))) => {
                    self.note(&format!(
                        "{op} {session} by {}: {} bytes sha256 {} ({} -> {})",
                        self.requester, outcome.bytes, outcome.sha256, local_path, remote_path
                    ));
                    Response {
                        id,
                        ok: true,
                        bytes: Some(outcome.bytes),
                        sha256: Some(outcome.sha256),
                        ..Response::default()
                    }
                }
                Ok(Ok(Err(reason))) => {
                    self.note(&format!(
                        "{op} {session} by {} failed: {reason}",
                        self.requester
                    ));
                    Response::error(id, reason)
                }
                Ok(Err(_)) => Response::error(id, "session closed"),
                Err(_) => {
                    self.note(&format!("{op} {session} by {} timed out", self.requester));
                    Response::error(id, "timed out")
                }
            }
        }

        fn close(&mut self, id: u64, session: String) -> Response {
            if !protocol::valid_session_id(&session) {
                return Response::error(id, "bad session id");
            }
            // Closing is cleanup, not access: always allowed.
            match self.sessions.remove(&session) {
                Some(_) => {
                    self.note(&format!("session {session} closed by {}", self.requester));
                    Response {
                        id,
                        ok: true,
                        ..Response::default()
                    }
                }
                None => Response::error(id, "no such session"),
            }
        }
    }

    /// Serve one connection: one request per line, answered in order.
    async fn handle_conn(stream: transport::Stream, mut conn: Conn) {
        let (read_half, mut write_half) = tokio::io::split(stream);
        let mut reader = tokio::io::BufReader::new(read_half);
        let mut line = Vec::new();
        loop {
            match read_line_capped(&mut reader, &mut line).await {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => {
                    conn.note(&format!(
                        "{} dropped: line over the size cap",
                        conn.requester
                    ));
                    break;
                }
            }
            let response = match std::str::from_utf8(&line) {
                Ok(text) => match serde_json::from_str::<Request>(text) {
                    Ok(request) => {
                        let Request { id, op } = request;
                        conn.handle(id, op).await
                    }
                    Err(e) => Response::error(0, format!("bad request: {e}")),
                },
                Err(_) => Response::error(0, "not utf-8"),
            };
            let mut text = serde_json::to_string(&response)
                .unwrap_or_else(|_| r#"{"id":0,"ok":false,"error":"encode failed"}"#.to_owned());
            text.push('\n');
            if write_half.write_all(text.as_bytes()).await.is_err() {
                break;
            }
        }
        conn.note(&format!(
            "{} disconnected; {} session(s) closed with it",
            conn.requester,
            conn.sessions.len()
        ));
        // `conn` drops here, taking the session handles with it: their
        // exec loops end and the connections underneath close.
    }

    /// Read one newline-terminated line into `buf`. An untrusted client
    /// must not be able to make us buffer without bound, so a line past
    /// [`protocol::MAX_LINE_BYTES`] errors the connection instead of
    /// completing. A final line without its newline still arrives; a
    /// clean EOF reads as zero.
    pub(super) async fn read_line_capped<R>(
        reader: &mut tokio::io::BufReader<R>,
        buf: &mut Vec<u8>,
    ) -> io::Result<usize>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        buf.clear();
        loop {
            let (newline, used) = {
                let available = reader.fill_buf().await?;
                match available.iter().position(|&b| b == b'\n') {
                    Some(pos) => {
                        buf.extend_from_slice(&available[..=pos]);
                        (true, pos + 1)
                    }
                    None => {
                        buf.extend_from_slice(available);
                        (false, available.len())
                    }
                }
            };
            reader.consume(used);
            if buf.len() > protocol::MAX_LINE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "line over the size cap",
                ));
            }
            if newline {
                return Ok(buf.len());
            }
            if used == 0 {
                // EOF: `buf` holds a trailing line without its newline.
                return Ok(buf.len());
            }
        }
    }

    /// Connect parameters, built exactly the way a pane builds them:
    /// secrets load from the keyring, key paths expand. None of it ever
    /// crosses the socket.
    pub(super) fn broker_params(profile: &SshProfile, keepalive: u64) -> SshParams {
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
            .map(crate::app::expand_tilde)
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
            keepalive,
            // Unused on the connect-only path; honest placeholders.
            cols: 80,
            rows: 24,
        }
    }
}

pub use broker::start;

#[cfg(all(test, unix))]
mod tests {
    use super::broker::*;
    use super::*;
    use crate::agent::protocol;
    use crate::agent::transport;
    use crate::config::{AgentApiConfig, AuthMethod};

    fn profile(name: &str) -> SshProfile {
        SshProfile {
            name: name.into(),
            host: format!("{name}.example"),
            port: 22,
            user: "deploy".into(),
            auth: AuthMethod::Agent,
            key_path: None,
            proxy_jump: None,
            forwards: Vec::new(),
            remote_forwards: Vec::new(),
            socks_forwards: Vec::new(),
            agent_forwarding: false,
            group: None,
        }
    }

    #[test]
    fn agents_see_only_allowed_profiles() {
        let config = Config {
            profiles: vec![profile("alpha"), profile("beta")],
            agent_api: AgentApiConfig {
                allow_profiles: Some(vec!["alpha".into()]),
                ..AgentApiConfig::default()
            },
            ..Config::default()
        };
        let shared = Shared::from_config(&config);

        assert!(shared.allowed("alpha"));
        assert!(!shared.allowed("beta"));
        let names: Vec<String> = shared.advertised().into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["alpha".to_owned()]);
    }

    #[test]
    fn the_settings_gate_refuses_when_off() {
        let mut config = Config::default();
        config.agent_api.enabled = false;
        let shared = Shared::from_config(&config);

        assert_eq!(
            shared.gate(),
            Err("agent access is turned off in cl2metal settings")
        );
        // The switch only gates access; profiles stay resolvable for
        // the moment it flips back on.
        assert!(shared.find_profile("anything").is_none());
    }

    #[test]
    fn refresh_flips_the_live_settings() {
        let config = Config {
            profiles: vec![profile("alpha")],
            ..Config::default()
        };
        let shared = Shared::from_config(&config);
        assert!(shared.gate().is_ok());
        assert!(
            shared.confirm_commands(),
            "screening defaults to on with the config"
        );

        let saved = Config {
            agent_api: AgentApiConfig {
                enabled: false,
                confirm_commands: false,
                ..AgentApiConfig::default()
            },
            profiles: Vec::new(),
            ..Config::default()
        };
        shared.refresh(&saved);

        assert!(shared.gate().is_err());
        assert!(!shared.confirm_commands());
        assert!(shared.find_profile("alpha").is_none());
    }

    #[test]
    fn connect_params_mirror_the_pane_path() {
        let mut p = profile("gamma");
        p.auth = AuthMethod::Key;
        p.key_path = Some("~/keys/id_ed25519".into());
        p.proxy_jump = Some("bastion:2222".into());
        let params = broker_params(&p, 33);

        assert_eq!(params.user, "deploy");
        assert_eq!(params.host, "gamma.example");
        assert_eq!(params.keepalive, 33);
        assert_eq!(params.proxy_jump.as_deref(), Some("bastion:2222"));
        let key = params.key_path.expect("key path survives");
        assert!(key.starts_with('/'), "tilde expanded: {key}");
        assert!(!key.contains('~'));
    }

    #[test]
    fn line_reader_caps_runaway_input() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            use tokio::io::AsyncWriteExt as _;
            // A well-formed line arrives whole; EOF after it reads zero.
            let (mut client, server) = tokio::io::duplex(1024);
            client.write_all(b"{\"id\":1}").await.unwrap();
            drop(client);
            let mut reader = tokio::io::BufReader::new(server);
            let mut buf = Vec::new();
            let n = read_line_capped(&mut reader, &mut buf).await.unwrap();
            assert_eq!(buf, br#"{"id":1}"#);
            assert_eq!(n, buf.len());
            assert_eq!(read_line_capped(&mut reader, &mut buf).await.unwrap(), 0);
        });

        rt.block_on(async {
            use tokio::io::AsyncWriteExt as _;
            // A trailing line without its newline still arrives.
            let (mut client, server) = tokio::io::duplex(1024);
            client.write_all(b"no newline").await.unwrap();
            drop(client);
            let mut reader = tokio::io::BufReader::new(server);
            let mut buf = Vec::new();
            let n = read_line_capped(&mut reader, &mut buf).await.unwrap();
            assert_eq!(buf, b"no newline");
            assert_eq!(n, buf.len());
        });

        rt.block_on(async {
            use tokio::io::AsyncWriteExt as _;
            // A line past the cap kills the connection, not the memory.
            // Exactly cap+1 bytes, all of which get consumed before the
            // error, so the writer finishes too.
            let (mut client, server) = tokio::io::duplex(1024);
            let writer = tokio::spawn(async move {
                client
                    .write_all(&vec![b'a'; protocol::MAX_LINE_BYTES + 1])
                    .await
                    .unwrap();
            });
            let mut reader = tokio::io::BufReader::new(server);
            let mut buf = Vec::new();
            assert!(read_line_capped(&mut reader, &mut buf).await.is_err());
            writer.await.unwrap();
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn requester_names_come_from_proc_and_degrade_to_the_pid() {
        let own = transport::requester_name(std::process::id());
        assert!(own.contains(&format!("pid {}", std::process::id())));

        // A pid nothing can own reads as the bare number.
        assert_eq!(transport::requester_name(2_147_483_647), "pid 2147483647");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_peer_credentials_identify_the_client_process() {
        let dir = std::env::temp_dir().join(format!("cl2metal-peer-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sock");
        let _ = std::fs::remove_file(&path);
        // Bind from this plain test thread the way the GUI thread does, then
        // let the shared runtime drive both ends: tokio refuses to start a
        // runtime inside another one, and bind_socket already blocks on the
        // shared runtime.
        let listener = transport::bind(&transport::Endpoint::Unix(path.clone())).expect("bind socket");
        crate::term::runtime().block_on(async {
            let client = tokio::net::UnixStream::connect(&path)
                .await
                .expect("connect client");
            let (_server, peer) = listener.accept().await.expect("accept client");
            assert_eq!(peer.pid, std::process::id());
            assert_eq!(peer.uid, Some(unsafe { libc::getuid() }));
            drop(client);
        });
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The regression pin for the launch crash: binding must not need a
    /// running reactor, because `start()` is called from the GUI thread.
    /// This test runs on a plain test thread — the exact context that
    /// used to panic.
    #[test]
    fn binding_needs_no_running_reactor() {
        let dir = std::env::temp_dir().join(format!("cl2metal-bind-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sock");
        let listener = transport::bind(&transport::Endpoint::Unix(path.clone())).expect("bind off the runtime");
        assert!(path.exists(), "the bound socket file exists");
        drop(listener);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
