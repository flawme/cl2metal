//! SSH sessions backed by `russh` (pure Rust, memory-safe).
//!
//! Security model:
//! - Host keys are verified against `~/.ssh/known_hosts`; unknown hosts require
//!   explicit trust-on-first-use confirmation; changed keys are rejected (MITM).
//! - Credentials are provided by the caller (from the OS keyring) and are never
//!   written to disk by cl2metal.

use super::{ClientMsg, MsgTx, SessionMsg, SshRequest, Waker};
use crate::config::AuthMethod;
use russh::keys::agent::client::AgentClient;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, known_hosts, load_secret_key};
use std::sync::Arc;

#[derive(Clone)]
pub struct SshParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Which authentication method to use.
    pub auth: AuthMethod,
    /// Password auth (from keyring or prompted by the UI).
    pub password: Option<String>,
    /// Public key auth: path to the private key.
    pub key_path: Option<String>,
    /// Passphrase for an encrypted private key.
    pub passphrase: Option<String>,
    /// Jump host: `[user@]host[:port]`.
    pub proxy_jump: Option<String>,
    /// Local port forwards: `[bind:]port:host:hostport`.
    pub forwards: Vec<String>,
    /// Remote port forwards: `[bind:]port:host:hostport` (ssh -R).
    pub remote_forwards: Vec<String>,
    /// Dynamic SOCKS5 forwards: `[bind:]port` (ssh -D).
    pub socks_forwards: Vec<String>,
    /// Forward the ssh-agent to the remote host (ssh -A).
    pub agent_forwarding: bool,
    /// Keepalive interval in seconds (0 disables it).
    pub keepalive: u64,
    pub cols: u16,
    pub rows: u16,
}

/// A parsed local-forward rule.
#[derive(Clone, Debug)]
struct Forward {
    bind: String,
    port: u16,
    host: String,
    host_port: u16,
}

/// Parse `[bind:]port:host:hostport`.
fn parse_forward(spec: &str) -> Option<Forward> {
    let parts: Vec<&str> = spec.split(':').collect();
    let (bind, port, host, host_port) = match parts.as_slice() {
        [port, host, host_port] => (
            "127.0.0.1".to_owned(),
            port.parse().ok()?,
            (*host).to_owned(),
            host_port.parse().ok()?,
        ),
        [bind, port, host, host_port] => (
            (*bind).to_owned(),
            port.parse().ok()?,
            (*host).to_owned(),
            host_port.parse().ok()?,
        ),
        _ => return None,
    };
    Some(Forward {
        bind,
        port,
        host,
        host_port,
    })
}

/// A parsed dynamic-forward rule (`ssh -D`): a local SOCKS5 listener.
#[derive(Clone, Debug)]
struct SocksForward {
    bind: String,
    port: u16,
}

/// Parse `[bind:]port` for a SOCKS5 listener.
fn parse_socks_forward(spec: &str) -> Option<SocksForward> {
    let parts: Vec<&str> = spec.split(':').collect();
    match parts.as_slice() {
        [port] => Some(SocksForward {
            bind: "127.0.0.1".to_owned(),
            port: port.parse().ok()?,
        }),
        [bind, port] => Some(SocksForward {
            bind: (*bind).to_owned(),
            port: port.parse().ok()?,
        }),
        _ => None,
    }
}

/// Parse `[user@]host[:port]`, defaulting to `default_user` and port 22.
fn parse_jump(spec: &str, default_user: &str) -> (String, String, u16) {
    let (user, rest) = match spec.split_once('@') {
        Some((user, rest)) => (user.to_owned(), rest),
        None => (default_user.to_owned(), spec),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => (host.to_owned(), port.parse().unwrap_or(22)),
        None => (rest.to_owned(), 22),
    };
    (user, host, port)
}

/// Spawn an SSH session. Returns `(client_tx, msg_rx)` for the UI side.
pub fn spawn(
    params: SshParams,
    wake: Waker,
) -> anyhow::Result<(flume::Sender<ClientMsg>, flume::Receiver<SessionMsg>)> {
    let (client_tx, client_rx) = flume::unbounded::<ClientMsg>();
    let (msg_tx, msg_rx) = flume::unbounded::<SessionMsg>();
    let msg_tx = MsgTx::new(msg_tx, wake);

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = msg_tx.send(SessionMsg::Exited(format!("tokio runtime: {e}")));
                return;
            }
        };
        rt.block_on(run(params, client_rx, msg_tx));
    });

    Ok((client_tx, msg_rx))
}

/// Ask the UI a question and block until it answers (session thread only).
fn ask(msg_tx: &MsgTx, request: SshRequest) -> Result<String, ()> {
    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    if msg_tx
        .send(SessionMsg::SshRequest(request, reply_tx))
        .is_err()
    {
        return Err(());
    }
    reply_rx.recv().unwrap_or(Err(()))
}

/// Where remote-forward connections are delivered, keyed by the address and
/// port the server bound.
type RemoteTargets = Arc<std::sync::Mutex<std::collections::HashMap<(String, u32), (String, u16)>>>;

/// The russh client handler: host-key checks and forwarded channels.
pub(crate) struct Client {
    host: String,
    port: u16,
    msg_tx: MsgTx,
    /// Remote forwards (`ssh -R`): where to deliver connections the server
    /// sends back, keyed by the address and port the server bound.
    remote_targets: RemoteTargets,
}

impl russh::client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = server_public_key.public_key();
        match known_hosts::check_known_hosts(&self.host, self.port, &key) {
            Ok(true) => Ok(true),
            Ok(false) => {
                // Trust on first use — but only with explicit user consent.
                let fingerprint = format!("{}", key.fingerprint(HashAlg::Sha256));
                let key_type = format!("{:?}", key.algorithm());
                let trusted = ask(
                    &self.msg_tx,
                    SshRequest::HostKey {
                        host: self.host.clone(),
                        port: self.port,
                        key_type,
                        fingerprint,
                    },
                );
                if trusted.is_ok() {
                    known_hosts::learn_known_hosts(&self.host, self.port, &key)?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Err(e) => {
                // Key changed: possible man-in-the-middle. Refuse.
                log::error!("host key changed for {}: {e}", self.host);
                Err(e.into())
            }
        }
    }

    /// A connection the server accepted on a remote forward (`ssh -R`).
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let target = self.remote_targets.lock().ok().and_then(|m| {
            m.get(&(connected_address.to_owned(), connected_port))
                .cloned()
        });
        let Some((host, port)) = target else {
            log::warn!("no local target for remote forward {connected_address}:{connected_port}");
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            match tokio::net::TcpStream::connect((host.as_str(), port)).await {
                Ok(mut socket) => {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                }
                Err(e) => log::warn!("remote forward {host}:{port}: {e}"),
            }
        });
        Ok(())
    }
}

async fn run(params: SshParams, client_rx: flume::Receiver<ClientMsg>, msg_tx: MsgTx) {
    let exit = match session(&params, &client_rx, &msg_tx).await {
        Ok(msg) => msg,
        Err(e) => format!("ssh error: {e}"),
    };
    let _ = msg_tx.send(SessionMsg::Exited(exit));
}

/// Public-key auth from a key file, prompting for a passphrase when needed.
///
/// Like OpenSSH, a passphrase that does not open the key is asked for again
/// instead of ending the session.
async fn auth_key(
    handle: &mut russh::client::Handle<Client>,
    params: &SshParams,
    msg_tx: &MsgTx,
) -> anyhow::Result<bool> {
    let Some(key_path) = &params.key_path else {
        anyhow::bail!("no private key configured for {}", params.user);
    };
    let mut passphrase = params.passphrase.clone();
    let mut asked = 0;
    let key = loop {
        match load_secret_key(key_path, passphrase.as_deref()) {
            Ok(key) => break key,
            Err(e) => {
                // A wrong passphrase and a damaged key produce the same
                // decryption errors, so keep asking — unless the key file
                // itself is unreadable, which no passphrase can fix.
                if asked >= AUTH_ATTEMPTS || matches!(e, russh::keys::Error::IO(_)) {
                    return Err(anyhow::anyhow!("failed to load key {key_path}: {e}"));
                }
                let prompt = if asked == 0 {
                    format!("Passphrase for key {key_path}: ")
                } else {
                    format!("Passphrase for key {key_path} (did not work, try again): ")
                };
                asked += 1;
                passphrase = Some(
                    ask(msg_tx, SshRequest::Passphrase { prompt })
                        .map_err(|_| anyhow::anyhow!("key passphrase entry cancelled"))?,
                );
            }
        }
    };
    let hash = handle.best_supported_rsa_hash().await?.flatten();
    let result = handle
        .authenticate_publickey(
            params.user.clone(),
            PrivateKeyWithHashAlg::new(Arc::new(key), hash),
        )
        .await?;
    Ok(result.success())
}

/// ssh-agent auth: offer every identity the agent holds, in order.
async fn auth_agent(
    handle: &mut russh::client::Handle<Client>,
    user: &str,
) -> anyhow::Result<bool> {
    // The agent transport is per-OS: a Unix socket named by SSH_AUTH_SOCK on
    // unix, the OpenSSH agent's named pipe on Windows.
    #[cfg(unix)]
    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|e| anyhow::anyhow!("ssh-agent unavailable ({e}); is SSH_AUTH_SOCK set?"))?;
    #[cfg(windows)]
    let mut agent = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
        .await
        .map_err(|e| {
            anyhow::anyhow!("ssh-agent unavailable ({e}); is the OpenSSH agent service running?")
        })?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| anyhow::anyhow!("ssh-agent request failed: {e}"))?;
    if identities.is_empty() {
        anyhow::bail!("ssh-agent has no identities — run ssh-add first");
    }
    let hash = handle.best_supported_rsa_hash().await?.flatten();
    for identity in identities {
        let public = identity.public_key().into_owned();
        match handle
            .authenticate_publickey_with(user.to_owned(), public, hash, &mut agent)
            .await
        {
            Ok(result) if result.success() => return Ok(true),
            Ok(_) => continue,
            Err(e) => {
                log::warn!("ssh-agent identity rejected: {e}");
                continue;
            }
        }
    }
    Ok(false)
}

/// How many auth attempts before the caller falls back (OpenSSH tries three
/// times too). Applies to passwords and to key passphrases.
const AUTH_ATTEMPTS: u32 = 3;

/// Password auth: the keyring password first, then interactive retries.
///
/// A stored password can go stale — it changed on the server — so a rejection
/// asks for a fresh one instead of ending the session, and asks again for the
/// attempt it has left.
async fn auth_password(
    handle: &mut russh::client::Handle<Client>,
    params: &SshParams,
    msg_tx: &MsgTx,
) -> anyhow::Result<bool> {
    let mut stored = params.password.clone();
    for attempt in 1..=AUTH_ATTEMPTS {
        let password = match stored.take() {
            Some(stored) => stored,
            None => {
                let prompt = if attempt == 1 {
                    format!("Password for {}@{}: ", params.user, params.host)
                } else {
                    let left = AUTH_ATTEMPTS + 1 - attempt;
                    let tries = if left == 1 { "try" } else { "tries" };
                    format!(
                        "Password for {}@{} (authentication failed, {left} {tries} left): ",
                        params.user, params.host
                    )
                };
                ask(msg_tx, SshRequest::Password { prompt })
                    .map_err(|_| anyhow::anyhow!("password entry cancelled"))?
            }
        };
        if handle
            .authenticate_password(params.user.clone(), password)
            .await?
            .success()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Keyboard-interactive auth (MFA, one-time codes, enterprise servers).
async fn auth_keyboard_interactive(
    handle: &mut russh::client::Handle<Client>,
    user: &str,
    msg_tx: &MsgTx,
) -> anyhow::Result<bool> {
    use russh::client::KeyboardInteractiveAuthResponse as Response;
    let mut response = handle
        .authenticate_keyboard_interactive_start(user.to_owned(), None::<String>)
        .await?;
    loop {
        match response {
            Response::Success => return Ok(true),
            Response::Failure { .. } => return Ok(false),
            Response::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let mut answers = Vec::with_capacity(prompts.len());
                for prompt in prompts {
                    let label = if prompt.prompt.trim().is_empty() {
                        if instructions.trim().is_empty() {
                            format!("{name}: ")
                        } else {
                            instructions.clone()
                        }
                    } else {
                        prompt.prompt.clone()
                    };
                    let answer = ask(msg_tx, SshRequest::KeyboardInteractive { prompt: label })
                        .map_err(|_| anyhow::anyhow!("authentication cancelled"))?;
                    answers.push(answer);
                }
                response = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await?;
            }
        }
    }
}

/// Connect to the target through a jump host, returning both handles.
///
/// The bastion handle must stay alive for the tunnel to work, so it is handed
/// back to the caller to hold for the session's lifetime.
async fn connect_via_jump(
    config: Arc<russh::client::Config>,
    jump: &str,
    params: &SshParams,
    msg_tx: &MsgTx,
    remote_targets: RemoteTargets,
) -> anyhow::Result<(russh::client::Handle<Client>, russh::client::Handle<Client>)> {
    let (user, host, port) = parse_jump(jump, &params.user);
    let handler = Client {
        host: host.clone(),
        port,
        msg_tx: msg_tx.clone(),
        remote_targets: remote_targets.clone(),
    };
    let mut jump_handle = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        russh::client::connect(config.clone(), (host.as_str(), port), handler),
    )
    .await
    .map_err(|_| anyhow::anyhow!("jump host {host}:{port} timed out"))??;

    // Authenticate on the bastion with the target's credentials.
    let mut jump_params = params.clone();
    jump_params.host = host.clone();
    jump_params.port = port;
    jump_params.user = user.clone();
    let ok = match jump_params.auth {
        AuthMethod::Agent => auth_agent(&mut jump_handle, &jump_params.user).await?,
        AuthMethod::Key => auth_key(&mut jump_handle, &jump_params, msg_tx).await?,
        AuthMethod::Password => auth_password(&mut jump_handle, &jump_params, msg_tx).await?,
    };
    if !ok {
        anyhow::bail!("jump host authentication failed for {user}@{host}");
    }

    // Tunnel to the target over the bastion.
    let channel = jump_handle
        .channel_open_direct_tcpip(params.host.clone(), params.port as u32, "127.0.0.1", 0)
        .await?;
    let handler = Client {
        host: params.host.clone(),
        port: params.port,
        msg_tx: msg_tx.clone(),
        remote_targets,
    };
    let handle = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        russh::client::connect_stream(config, channel.into_stream(), handler),
    )
    .await
    .map_err(|_| anyhow::anyhow!("connection through jump host timed out"))??;
    Ok((jump_handle, handle))
}

/// Accept local connections and tunnel each one to the remote side.
async fn run_forward(
    handle: Arc<russh::client::Handle<Client>>,
    forward: Forward,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind((forward.bind.as_str(), forward.port))
        .await
        .map_err(|e| anyhow::anyhow!("bind {}:{}: {e}", forward.bind, forward.port))?;
    loop {
        let (mut socket, peer) = listener.accept().await?;
        let handle = handle.clone();
        let host = forward.host.clone();
        let port = forward.host_port as u32;
        tokio::spawn(async move {
            match handle
                .channel_open_direct_tcpip(host, port, peer.ip().to_string(), peer.port() as u32)
                .await
            {
                Ok(channel) => {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                }
                Err(e) => log::warn!("forward open failed: {e}"),
            }
        });
    }
}

/// Accept SOCKS5 clients and tunnel each CONNECT through the SSH session.
async fn run_socks_forward(
    handle: Arc<russh::client::Handle<Client>>,
    forward: SocksForward,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind((forward.bind.as_str(), forward.port))
        .await
        .map_err(|e| anyhow::anyhow!("bind {}:{}: {e}", forward.bind, forward.port))?;
    loop {
        let (socket, peer) = listener.accept().await?;
        let handle = handle.clone();
        tokio::spawn(async move {
            if let Err(e) = socks_serve(handle, socket, peer).await {
                log::warn!("socks {peer}: {e}");
            }
        });
    }
}

/// Serve one SOCKS5 client: greeting, CONNECT, then a bidirectional copy.
async fn socks_serve(
    handle: Arc<russh::client::Handle<Client>>,
    mut socket: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
) -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // Greeting: version, method count, methods. No authentication offered.
    let mut head = [0u8; 2];
    socket.read_exact(&mut head).await?;
    if head[0] != 5 {
        anyhow::bail!("not SOCKS5 (version {})", head[0]);
    }
    let mut methods = vec![0u8; head[1] as usize];
    socket.read_exact(&mut methods).await?;
    socket.write_all(&[5, 0]).await?;

    // Request: version, command, reserved, address type.
    let mut req = [0u8; 4];
    socket.read_exact(&mut req).await?;
    if req[0] != 5 || req[1] != 1 {
        // Only CONNECT is supported (no BIND, no UDP ASSOCIATE).
        socket.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        anyhow::bail!("unsupported SOCKS command {}", req[1]);
    }
    let host = match req[3] {
        1 => {
            let mut addr = [0u8; 4];
            socket.read_exact(&mut addr).await?;
            std::net::Ipv4Addr::from(addr).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            socket.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            socket.read_exact(&mut name).await?;
            String::from_utf8(name)?
        }
        4 => {
            let mut addr = [0u8; 16];
            socket.read_exact(&mut addr).await?;
            std::net::Ipv6Addr::from(addr).to_string()
        }
        other => {
            socket.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            anyhow::bail!("unsupported address type {other}");
        }
    };
    let mut port = [0u8; 2];
    socket.read_exact(&mut port).await?;
    let port = u16::from_be_bytes(port);

    match handle
        .channel_open_direct_tcpip(
            host.clone(),
            port as u32,
            peer.ip().to_string(),
            peer.port() as u32,
        )
        .await
    {
        Ok(channel) => {
            // Success: bind address 0.0.0.0, port 0.
            socket.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
            Ok(())
        }
        Err(e) => {
            // General failure.
            socket.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            anyhow::bail!("connect {host}:{port}: {e}")
        }
    }
}

/// An authenticated connection that is not running a shell.
///
/// The SFTP worker needs its own connection: the terminal's session thread owns
/// its channel loop, so a second connection is the clean way to get an
/// independent subsystem channel. The jump host's handle is held here because
/// the tunnel dies with it.
pub(crate) struct SshConnection {
    pub(crate) handle: russh::client::Handle<Client>,
    _jump: Option<russh::client::Handle<Client>>,
}

/// Connect and authenticate without opening a shell.
pub(crate) async fn connect_only(
    params: &SshParams,
    msg_tx: &MsgTx,
) -> anyhow::Result<SshConnection> {
    let config = Arc::new(russh::client::Config {
        nodelay: true,
        keepalive_interval: (params.keepalive > 0)
            .then(|| std::time::Duration::from_secs(params.keepalive)),
        keepalive_max: 3,
        inactivity_timeout: Some(std::time::Duration::from_secs(600)),
        ..russh::client::Config::default()
    });
    let remote_targets: RemoteTargets =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let (jump, mut handle) = match params.proxy_jump.as_deref().map(str::trim) {
        Some(jump) if !jump.is_empty() => {
            let (guard, handle) =
                connect_via_jump(config.clone(), jump, params, msg_tx, remote_targets.clone())
                    .await?;
            (Some(guard), handle)
        }
        _ => {
            let handler = Client {
                host: params.host.clone(),
                port: params.port,
                msg_tx: msg_tx.clone(),
                remote_targets,
            };
            let handle = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                russh::client::connect(config, (params.host.as_str(), params.port), handler),
            )
            .await
            .map_err(|_| anyhow::anyhow!("connection timed out"))??;
            (None, handle)
        }
    };
    let auth_ok = match params.auth {
        AuthMethod::Agent => auth_agent(&mut handle, &params.user).await?,
        AuthMethod::Key => auth_key(&mut handle, params, msg_tx).await?,
        AuthMethod::Password => {
            let ok = auth_password(&mut handle, params, msg_tx).await?;
            if ok {
                true
            } else {
                auth_keyboard_interactive(&mut handle, &params.user, msg_tx).await?
            }
        }
    };
    if !auth_ok {
        anyhow::bail!("authentication failed for {}", params.user);
    }
    Ok(SshConnection {
        handle,
        _jump: jump,
    })
}

async fn session(
    params: &SshParams,
    client_rx: &flume::Receiver<ClientMsg>,
    msg_tx: &MsgTx,
) -> anyhow::Result<String> {
    let config = russh::client::Config {
        // Interactive shells want low latency: disable Nagle's algorithm.
        nodelay: true,
        keepalive_interval: (params.keepalive > 0)
            .then(|| std::time::Duration::from_secs(params.keepalive)),
        keepalive_max: 3,
        inactivity_timeout: Some(std::time::Duration::from_secs(600)),
        ..russh::client::Config::default()
    };
    let config = Arc::new(config);

    // Remote forwards are delivered through the handler, so the map has to
    // exist before the connection is made.
    let remote_targets: RemoteTargets =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    // Connect directly, or through a jump host when one is configured.
    let (_jump_guard, mut handle) = match params.proxy_jump.as_deref().map(str::trim) {
        Some(jump) if !jump.is_empty() => {
            let (guard, handle) =
                connect_via_jump(config.clone(), jump, params, msg_tx, remote_targets.clone())
                    .await?;
            (Some(guard), handle)
        }
        _ => {
            let handler = Client {
                host: params.host.clone(),
                port: params.port,
                msg_tx: msg_tx.clone(),
                remote_targets: remote_targets.clone(),
            };
            let handle = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                russh::client::connect(config, (params.host.as_str(), params.port), handler),
            )
            .await
            .map_err(|_| anyhow::anyhow!("connection timed out"))??;
            (None, handle)
        }
    };

    // --- authentication -------------------------------------------------
    let auth_ok = match params.auth {
        AuthMethod::Agent => auth_agent(&mut handle, &params.user).await?,
        AuthMethod::Key => auth_key(&mut handle, params, msg_tx).await?,
        AuthMethod::Password => {
            let ok = auth_password(&mut handle, params, msg_tx).await?;
            if ok {
                true
            } else {
                // Many servers with MFA only offer keyboard-interactive.
                auth_keyboard_interactive(&mut handle, &params.user, msg_tx).await?
            }
        }
    };
    if !auth_ok {
        anyhow::bail!("authentication failed for {}", params.user);
    }

    // Authentication is done; share the handle so port forwards can open
    // channels on the same session.
    let handle = Arc::new(handle);

    // --- local port forwards (-L) --------------------------------------
    for spec in &params.forwards {
        match parse_forward(spec) {
            Some(forward) => {
                let label = format!(
                    "{}:{} -> {}:{}",
                    forward.bind, forward.port, forward.host, forward.host_port
                );
                let handle = handle.clone();
                let notice = msg_tx.clone();
                tokio::spawn(async move {
                    match run_forward(handle, forward).await {
                        Ok(()) => {}
                        Err(e) => {
                            let _ = notice
                                .send(SessionMsg::Notice(format!("forward {label} failed: {e}")));
                        }
                    }
                });
            }
            None => {
                let _ = msg_tx.send(SessionMsg::Notice(format!(
                    "ignoring malformed forward rule: {spec}"
                )));
            }
        }
    }

    // --- remote port forwards (-R) --------------------------------------
    for spec in &params.remote_forwards {
        match parse_forward(spec) {
            Some(forward) => {
                let label = format!(
                    "{}:{} -> {}:{}",
                    forward.bind, forward.port, forward.host, forward.host_port
                );
                // The server binds the port; connections come back through the
                // handler, which needs to know where to deliver them.
                if let Ok(mut map) = remote_targets.lock() {
                    map.insert(
                        (forward.bind.clone(), forward.port as u32),
                        (forward.host.clone(), forward.host_port),
                    );
                }
                match handle
                    .tcpip_forward(forward.bind.clone(), forward.port as u32)
                    .await
                {
                    Ok(bound) => {
                        let _ = msg_tx.send(SessionMsg::Notice(format!(
                            "remote forward {label} listening on port {bound}"
                        )));
                    }
                    Err(e) => {
                        let _ = msg_tx.send(SessionMsg::Notice(format!(
                            "remote forward {label} failed: {e}"
                        )));
                    }
                }
            }
            None => {
                let _ = msg_tx.send(SessionMsg::Notice(format!(
                    "ignoring malformed remote forward rule: {spec}"
                )));
            }
        }
    }

    // --- dynamic SOCKS5 forwards (-D) -----------------------------------
    for spec in &params.socks_forwards {
        match parse_socks_forward(spec) {
            Some(forward) => {
                let label = format!("{}:{}", forward.bind, forward.port);
                let handle = handle.clone();
                let notice = msg_tx.clone();
                tokio::spawn(async move {
                    match run_socks_forward(handle, forward).await {
                        Ok(()) => {}
                        Err(e) => {
                            let _ = notice.send(SessionMsg::Notice(format!(
                                "socks forward {label} failed: {e}"
                            )));
                        }
                    }
                });
            }
            None => {
                let _ = msg_tx.send(SessionMsg::Notice(format!(
                    "ignoring malformed socks forward rule: {spec}"
                )));
            }
        }
    }

    // --- interactive shell --------------------------------------------
    let mut channel = handle.channel_open_session().await?;
    if params.agent_forwarding {
        // ssh -A: let the remote host use the local agent.
        if let Err(e) = channel.agent_forward(true).await {
            let _ = msg_tx.send(SessionMsg::Notice(format!("agent forwarding failed: {e}")));
        }
    }
    channel
        .request_pty(
            true,
            "xterm-256color",
            params.cols as u32,
            params.rows as u32,
            0,
            0,
            &[],
        )
        .await?;
    channel.request_shell(true).await?;

    let mut exit_msg: Option<String> = None;

    loop {
        tokio::select! {
            msg = client_rx.recv_async() => {
                match msg {
                    Ok(ClientMsg::Input(bytes)) => {
                        if let Err(e) = channel.data_bytes(bytes).await {
                            exit_msg = Some(format!("send failed: {e}"));
                            break;
                        }
                    }
                    Ok(ClientMsg::Resize { cols, rows, cell_w, cell_h }) => {
                        let _ = channel
                            .window_change(cols as u32, rows as u32, cell_w as u32, cell_h as u32)
                            .await;
                    }
                    Ok(ClientMsg::Close) | Err(_) => {
                        let _ = channel.eof().await;
                        let _ = channel.close().await;
                        let _ = handle
                            .disconnect(
                                russh::Disconnect::ByApplication,
                                "",
                                "English",
                            )
                            .await;
                        return Ok("disconnected".into());
                    }
                }
            }
            msg = channel.wait() => {
                match msg {
                    Some(russh::ChannelMsg::Data { data }) => {
                        if msg_tx.send(SessionMsg::Data(data.to_vec())).is_err() {
                            break;
                        }
                    }
                    Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                        if msg_tx.send(SessionMsg::Data(data.to_vec())).is_err() {
                            break;
                        }
                    }
                    Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                        exit_msg = Some(format!("exited with status {exit_status}"));
                    }
                    Some(russh::ChannelMsg::ExitSignal { signal_name, error_message, .. }) => {
                        exit_msg = Some(if error_message.is_empty() {
                            format!("terminated by {signal_name:?}")
                        } else {
                            format!("terminated by {signal_name:?}: {error_message}")
                        });
                    }
                    Some(russh::ChannelMsg::Eof) => {
                        // Remote closed its side; keep draining until Close.
                    }
                    Some(russh::ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
        }
    }

    let _ = channel.eof().await;
    let _ = channel.close().await;
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "", "English")
        .await;
    Ok(exit_msg.unwrap_or_else(|| "connection closed".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_rules_parse_with_and_without_bind_address() {
        let f = parse_forward("8080:localhost:5432").expect("parse");
        assert_eq!(f.bind, "127.0.0.1");
        assert_eq!(f.port, 8080);
        assert_eq!(f.host, "localhost");
        assert_eq!(f.host_port, 5432);

        let f = parse_forward("0.0.0.0:9000:db.internal:5432").expect("parse");
        assert_eq!(f.bind, "0.0.0.0");
        assert_eq!(f.port, 9000);
        assert_eq!(f.host, "db.internal");
        assert_eq!(f.host_port, 5432);
    }

    #[test]
    fn malformed_forward_rules_are_rejected() {
        assert!(parse_forward("").is_none());
        assert!(parse_forward("8080").is_none());
        assert!(parse_forward("8080:localhost").is_none());
        assert!(parse_forward("notaport:localhost:5432").is_none());
        assert!(parse_forward("8080:localhost:notaport").is_none());
    }

    #[test]
    fn socks_forward_rules_parse_with_and_without_bind_address() {
        let f = parse_socks_forward("1080").expect("parse");
        assert_eq!(f.bind, "127.0.0.1");
        assert_eq!(f.port, 1080);

        let f = parse_socks_forward("0.0.0.0:9050").expect("parse");
        assert_eq!(f.bind, "0.0.0.0");
        assert_eq!(f.port, 9050);
    }

    #[test]
    fn malformed_socks_forward_rules_are_rejected() {
        assert!(parse_socks_forward("").is_none());
        assert!(parse_socks_forward("notaport").is_none());
        assert!(parse_socks_forward("1080:extra:bits").is_none());
    }

    #[test]
    fn jump_specs_default_user_and_port() {
        let (user, host, port) = parse_jump("bastion", "deploy");
        assert_eq!(user, "deploy");
        assert_eq!(host, "bastion");
        assert_eq!(port, 22);

        let (user, host, port) = parse_jump("root@bastion.example.com:2222", "deploy");
        assert_eq!(user, "root");
        assert_eq!(host, "bastion.example.com");
        assert_eq!(port, 2222);
    }
}
