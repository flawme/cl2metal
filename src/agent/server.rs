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
    default_timeout_ms: AtomicU64,
    keepalive: AtomicU64,
    profiles: Mutex<Vec<SshProfile>>,
    allow_profiles: Mutex<Option<Vec<String>>>,
}

impl Shared {
    #[cfg(unix)]
    fn from_config(config: &Config) -> Self {
        Self {
            enabled: AtomicBool::new(config.agent_api.enabled),
            confirm_commands: AtomicBool::new(config.agent_api.confirm_commands),
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
        self.default_timeout_ms
            .store(config.agent_api.default_timeout_ms, Ordering::Relaxed);
        self.keepalive
            .store(config.ssh_keepalive, Ordering::Relaxed);
        *self.profiles.lock().unwrap() = config.profiles.clone();
        *self.allow_profiles.lock().unwrap() = config.agent_api.allow_profiles.clone();
    }

    /// The settings gate. Ping and close pass without it; everything
    /// else is agent access and needs the switch on.
    #[cfg(unix)]
    fn gate(&self) -> Result<(), &'static str> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Err("agent access is turned off in cl2metal settings");
        }
        Ok(())
    }

    /// Whether exec requests are screened: read-only commands run,
    /// everything else needs the human.
    #[cfg(unix)]
    fn confirm_commands(&self) -> bool {
        self.confirm_commands.load(Ordering::Relaxed)
    }

    #[cfg(unix)]
    fn find_profile(&self, name: &str) -> Option<SshProfile> {
        self.profiles
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.name == name)
            .cloned()
    }

    /// Whether an agent may open this profile at all.
    #[cfg(unix)]
    fn allowed(&self, name: &str) -> bool {
        match &*self.allow_profiles.lock().unwrap() {
            None => true,
            Some(list) => list.iter().any(|allowed| allowed == name),
        }
    }

    /// What agents are told exists: only profiles they may use.
    #[cfg(unix)]
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

    #[cfg(unix)]
    fn default_timeout(&self) -> u64 {
        self.default_timeout_ms.load(Ordering::Relaxed)
    }

    #[cfg(unix)]
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

#[cfg(unix)]
mod unix {
    use std::collections::HashMap;
    use std::io;
    use std::path::PathBuf;
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

    /// Where to try binding, in order: the user's runtime dir (0700 by
    /// construction), then /tmp keyed by uid. `SO_PEERCRED` enforces the
    /// same-user rule either way; the 0600 mode keeps other accounts
    /// from even knocking.
    pub(super) fn socket_paths(runtime_dir: Option<&std::ffi::OsStr>, uid: u32) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Some(dir) = runtime_dir.filter(|d| !d.is_empty()) {
            paths.push(PathBuf::from(dir).join("cl2metal-agent.sock"));
        }
        paths.push(std::env::temp_dir().join(format!("cl2metal-agent-{uid}.sock")));
        paths
    }

    fn socket_path_candidates() -> Vec<PathBuf> {
        socket_paths(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), our_uid())
    }

    /// The socket a client (the `cl2metal mcp` child) should use: the
    /// first bound candidate, so a /tmp fallback is found too.
    pub(crate) fn client_socket_path() -> Option<PathBuf> {
        socket_path_candidates()
            .into_iter()
            .find(|path| path.exists())
    }

    /// Bind one socket path. A tokio listener registers with the
    /// current reactor, and callers reach this from the GUI thread where
    /// no reactor runs — so the shared runtime is borrowed for the bind,
    /// never for serving.
    pub(super) fn bind_socket(path: &std::path::Path) -> std::io::Result<tokio::net::UnixListener> {
        crate::term::runtime().block_on(async { tokio::net::UnixListener::bind(path) })
    }

    fn our_uid() -> u32 {
        unsafe { libc::getuid() }
    }

    /// The peer's (pid, uid), or `None` when the platform cannot tell
    /// us. Linux answers via `SO_PEERCRED`; other unixes get refused
    /// rather than serve traffic we cannot attribute.
    fn peer_creds(stream: &tokio::net::UnixStream) -> Option<(u32, u32)> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd as _;
            let mut cred = libc::ucred {
                pid: 0,
                uid: 0,
                gid: 0,
            };
            let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
            let rc = unsafe {
                libc::getsockopt(
                    stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PEERCRED,
                    &mut cred as *mut libc::ucred as *mut libc::c_void,
                    &mut len,
                )
            };
            (rc == 0).then_some((cred.pid as u32, cred.uid))
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::fd::AsRawFd as _;
            let fd = stream.as_raw_fd();
            let mut pid: libc::pid_t = 0;
            let mut pid_len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
            // macOS exposes the effective peer pid with LOCAL_PEEREPID, and `getpeereid`
            // supplies its effective uid for the same-user check.
            let pid_rc = unsafe {
                libc::getsockopt(
                    fd,
                    libc::SOL_LOCAL,
                    libc::LOCAL_PEEREPID,
                    &mut pid as *mut libc::pid_t as *mut libc::c_void,
                    &mut pid_len,
                )
            };
            let (mut uid, mut gid) = (0, 0);
            let uid_rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
            (pid_rc == 0 && uid_rc == 0 && pid > 0).then_some((pid as u32, uid))
        }
        #[cfg(not(target_os = "linux"))]
        #[cfg(not(target_os = "macos"))]
        {
            let _ = stream;
            None
        }
    }

    /// Who is asking: the process name behind the pid, for the consent
    /// modal and the audit log. A name we cannot read degrades to the
    /// bare pid.
    pub(super) fn requester_name(pid: u32) -> String {
        #[cfg(target_os = "linux")]
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        #[cfg(target_os = "macos")]
        let comm = std::process::Command::new("ps")
            .args(["-o", "comm=", "-p", &pid.to_string()])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|name| !name.is_empty());
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let comm = None;
        match comm {
            Some(comm) => format!("{comm} (pid {pid})"),
            None => format!("pid {pid}"),
        }
    }

    /// Start the broker: bind the socket and serve until the process
    /// ends. Returns `None` when nothing could be bound — the app runs
    /// on without agent access; that is never a startup failure.
    pub fn start(config: &Config, wake: Waker) -> Option<super::Broker> {
        let shared = Arc::new(Shared::from_config(config));
        let audit = Arc::new(Mutex::new(Audit::open()));
        let (events_tx, events_rx) = flume::unbounded::<AgentEvent>();
        let mut bound = None;
        for path in socket_path_candidates() {
            // A socket file left by a crashed run is stale by definition:
            // this process is the only one that can own the name.
            let _ = std::fs::remove_file(&path);
            match bind_socket(&path) {
                Ok(listener) => {
                    bound = Some((listener, path));
                    break;
                }
                Err(e) => log::warn!("agent socket: cannot bind {}: {e}", path.display()),
            }
        }
        let Some((listener, path)) = bound else {
            log::warn!("agent socket: no bindable path; agent access is off for this run");
            return None;
        };
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        log::info!("agent socket listening at {}", path.display());
        audit_line(&audit, &format!("listening at {}", path.display()));
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
        listener: tokio::net::UnixListener,
        shared: Arc<Shared>,
        audit: Arc<Mutex<Audit>>,
        events: flume::Sender<AgentEvent>,
        wake: Waker,
    ) {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(e) => {
                    log::warn!("agent socket: accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
            };
            // Identity before anything else: same user only.
            let Some((pid, uid)) = peer_creds(&stream) else {
                audit_line(&audit, "dropped a connection we could not identify");
                continue;
            };
            if uid != our_uid() {
                audit_line(
                    &audit,
                    &format!("rejected uid {uid} (pid {pid}): not this user"),
                );
                continue;
            }
            let requester = requester_name(pid);
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
    async fn handle_conn(stream: tokio::net::UnixStream, mut conn: Conn) {
        let (read_half, mut write_half) = stream.into_split();
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

#[cfg(unix)]
pub(crate) use unix::client_socket_path;
#[cfg(unix)]
pub use unix::start;

#[cfg(not(unix))]
/// Agent access needs a unix socket. On other platforms the broker
/// simply does not exist, and the app runs without the feature.
pub fn start(_config: &Config, _wake: crate::term::Waker) -> Option<Broker> {
    log::info!("agent access needs a unix socket; not available on this platform");
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::unix::*;
    use super::*;
    use crate::agent::protocol;
    use crate::config::{AgentApiConfig, AuthMethod};
    use std::path::PathBuf;

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
    fn socket_paths_try_the_runtime_dir_then_tmp() {
        let paths = socket_paths(Some(std::ffi::OsStr::new("/run/user/1000")), 1000);
        assert_eq!(
            paths[0],
            PathBuf::from("/run/user/1000/cl2metal-agent.sock")
        );
        assert!(
            paths[1]
                .to_string_lossy()
                .contains("cl2metal-agent-1000.sock")
        );
        assert_eq!(paths.len(), 2);

        let fallback = socket_paths(None, 1000);
        assert_eq!(fallback.len(), 1);

        // An empty runtime dir is no runtime dir.
        assert_eq!(socket_paths(Some(std::ffi::OsStr::new("")), 7).len(), 1);
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
        let own = requester_name(std::process::id());
        assert!(own.contains(&format!("pid {}", std::process::id())));

        // A pid nothing can own reads as the bare number.
        assert_eq!(requester_name(2_147_483_647), "pid 2147483647");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_peer_credentials_identify_the_client_process() {
        let dir = std::env::temp_dir().join(format!("cl2metal-peer-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sock");
        let _ = std::fs::remove_file(&path);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            let listener = bind_socket(&path).expect("bind socket");
            let client = tokio::net::UnixStream::connect(&path)
                .await
                .expect("connect client");
            let (server, _) = listener.accept().await.expect("accept client");
            let (pid, uid) = peer_creds(&server).expect("read peer credentials");
            assert_eq!(pid, std::process::id());
            assert_eq!(uid, unsafe { libc::getuid() });
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
        let listener = bind_socket(&path).expect("bind off the runtime");
        assert!(path.exists(), "the bound socket file exists");
        drop(listener);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
