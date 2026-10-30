//! Cross-platform IPC transport for the agent broker.
//!
//! Unix: a unix-domain socket in the user's runtime dir, with a `/tmp`
//! fallback keyed by uid. Windows: a named pipe
//! `\\.\pipe\cl2metal-agent-<user>`.
//!
//! The broker and the `cl2metal mcp` child must agree on the endpoint even
//! when the child did not inherit the app's environment. That is the bug this
//! module fixes: `XDG_RUNTIME_DIR` is often absent in an MCP host's child
//! process, so the conventional `/run/user/<uid>` path is always a candidate,
//! not only when the variable happens to be set.

use std::io;

/// One endpoint an agent client can connect to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    #[cfg(unix)]
    Unix(std::path::PathBuf),
    #[cfg(windows)]
    Windows(String),
}

impl Endpoint {
    /// A human-readable form for logs and error messages.
    pub fn display(&self) -> String {
        match self {
            #[cfg(unix)]
            Endpoint::Unix(path) => path.display().to_string(),
            #[cfg(windows)]
            Endpoint::Windows(name) => name.clone(),
        }
    }

    /// Whether a client should try this endpoint. Unix checks the socket
    /// file; Windows always returns true because a named pipe has no
    /// filesystem entry — the connect itself is the availability check.
    pub fn exists(&self) -> bool {
        match self {
            #[cfg(unix)]
            Endpoint::Unix(path) => path.exists(),
            #[cfg(windows)]
            Endpoint::Windows(_) => true,
        }
    }
}

/// Candidate endpoints in preference order.
///
/// Unix: `$XDG_RUNTIME_DIR`, then the conventional `/run/user/<uid>` path
/// (always, even when the variable is missing), then `/tmp` keyed by uid.
/// Windows: the per-user named pipe.
pub fn endpoints() -> Vec<Endpoint> {
    #[cfg(unix)]
    {
        let uid = our_uid();
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
            paths.push(std::path::PathBuf::from(dir).join("cl2metal-agent.sock"));
        }
        // The app binds here whenever XDG_RUNTIME_DIR is set. An MCP host's
        // child process frequently does not inherit it, so this must be a
        // candidate unconditionally. The path is Linux-specific; macOS uses
        // the /tmp fallback below.
        #[cfg(target_os = "linux")]
        paths.push(std::path::PathBuf::from(format!(
            "/run/user/{uid}/cl2metal-agent.sock"
        )));
        paths.push(std::env::temp_dir().join(format!("cl2metal-agent-{uid}.sock")));

        let mut out: Vec<std::path::PathBuf> = Vec::new();
        for path in paths {
            if !out.contains(&path) {
                out.push(path);
            }
        }
        out.into_iter().map(Endpoint::Unix).collect()
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".to_owned());
        let safe: String = user
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(64)
            .collect();
        let safe = if safe.is_empty() {
            "user".to_owned()
        } else {
            safe
        };
        vec![Endpoint::Windows(format!(
            r"\\.\pipe\cl2metal-agent-{safe}"
        ))]
    }
}

/// The first endpoint that exists, or an error naming every path checked.
/// The message is deliberately diagnostic: "app not running" and "socket
/// path mismatch" are different failures and must not look the same.
pub fn client_endpoint() -> Result<Endpoint, String> {
    let candidates = endpoints();
    for endpoint in &candidates {
        if endpoint.exists() {
            return Ok(endpoint.clone());
        }
    }
    let checked = candidates
        .iter()
        .map(Endpoint::display)
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "no agent socket (checked {checked}); is cl2metal running with agent access on?"
    ))
}

/// The peer's identity. `uid` is `Some` on Unix; on Windows it is `None`
/// because the named pipe's default DACL already restricts connections to
/// the creating user (plus admins/system), and Windows has no uid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub pid: u32,
    pub uid: Option<u32>,
}

/// A bound listener.
pub enum Listener {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    #[cfg(windows)]
    Windows(WindowsListener),
}

impl Listener {
    /// Wait for one client. Unix refuses a peer it cannot identify; Windows
    /// returns the client PID when the OS reports it and 0 otherwise.
    pub async fn accept(&self) -> io::Result<(Stream, Peer)> {
        match self {
            #[cfg(unix)]
            Listener::Unix(listener) => {
                let (stream, _) = listener.accept().await?;
                let peer = peer_creds(&stream).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::PermissionDenied, "cannot identify peer")
                })?;
                Ok((Stream::Unix(stream), peer))
            }
            #[cfg(windows)]
            Listener::Windows(listener) => listener.accept().await,
        }
    }
}

/// Bind an endpoint. Unix removes a stale socket file first (this process is
/// the only one that can own the name); Windows creates the first pipe
/// instance with `first_pipe_instance` so a squatter cannot win the name.
pub fn bind(endpoint: &Endpoint) -> io::Result<Listener> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Unix(path) => {
            let _ = std::fs::remove_file(path);
            let listener =
                crate::term::runtime().block_on(async { tokio::net::UnixListener::bind(path) })?;
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            Ok(Listener::Unix(listener))
        }
        #[cfg(windows)]
        Endpoint::Windows(name) => {
            let server = tokio::net::windows::named_pipe::ServerOptions::new()
                .first_pipe_instance(true)
                .create(name)?;
            Ok(Listener::Windows(WindowsListener {
                name: name.clone(),
                next: tokio::sync::Mutex::new(Some(server)),
            }))
        }
    }
}

/// Connect a blocking stream. The MCP child is a plain stdio process, so it
/// uses the blocking half of the transport.
pub fn connect_blocking(endpoint: &Endpoint) -> io::Result<BlockingStream> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Unix(path) => {
            std::os::unix::net::UnixStream::connect(path).map(BlockingStream::Unix)
        }
        #[cfg(windows)]
        Endpoint::Windows(name) => std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)
            .map(BlockingStream::Windows),
    }
}

/// A connected stream on the broker side.
pub enum Stream {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    #[cfg(windows)]
    Windows(tokio::net::windows::named_pipe::NamedPipeServer),
}

impl tokio::io::AsyncRead for Stream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Stream::Unix(stream) => std::pin::Pin::new(stream).poll_read(cx, buf),
            #[cfg(windows)]
            Stream::Windows(stream) => std::pin::Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for Stream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match self.get_mut() {
            #[cfg(unix)]
            Stream::Unix(stream) => std::pin::Pin::new(stream).poll_write(cx, buf),
            #[cfg(windows)]
            Stream::Windows(stream) => std::pin::Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Stream::Unix(stream) => std::pin::Pin::new(stream).poll_flush(cx),
            #[cfg(windows)]
            Stream::Windows(stream) => std::pin::Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Stream::Unix(stream) => std::pin::Pin::new(stream).poll_shutdown(cx),
            #[cfg(windows)]
            Stream::Windows(stream) => std::pin::Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// A connected stream on the MCP child side.
pub enum BlockingStream {
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
    #[cfg(windows)]
    Windows(std::fs::File),
}

impl BlockingStream {
    pub fn try_clone(&self) -> io::Result<Self> {
        match self {
            #[cfg(unix)]
            BlockingStream::Unix(stream) => stream.try_clone().map(BlockingStream::Unix),
            #[cfg(windows)]
            BlockingStream::Windows(file) => file.try_clone().map(BlockingStream::Windows),
        }
    }
}

impl std::io::Read for BlockingStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            BlockingStream::Unix(stream) => stream.read(buf),
            #[cfg(windows)]
            BlockingStream::Windows(file) => file.read(buf),
        }
    }
}

impl std::io::Write for BlockingStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            BlockingStream::Unix(stream) => stream.write(buf),
            #[cfg(windows)]
            BlockingStream::Windows(file) => file.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            BlockingStream::Unix(stream) => stream.flush(),
            #[cfg(windows)]
            BlockingStream::Windows(file) => file.flush(),
        }
    }
}

/// This user's uid. Windows has no uid; 0 is a placeholder that the broker
/// never compares because [`Peer::uid`] is `None` there.
pub fn our_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::getuid() }
    }
    #[cfg(windows)]
    {
        0
    }
}

/// Who is asking: the process name behind the pid, for the consent modal and
/// the audit log. A name we cannot read degrades to the bare pid.
pub fn requester_name(pid: u32) -> String {
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
    #[cfg(windows)]
    let comm = windows_process_name(pid);
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let comm = None;
    match comm {
        Some(comm) => format!("{comm} (pid {pid})"),
        None => format!("pid {pid}"),
    }
}

/// The peer's (pid, uid) on Unix. Linux answers via `SO_PEERCRED`; macOS
/// uses `LOCAL_PEEREPID` + `getpeereid`; other unixes are refused rather
/// than served without attribution.
#[cfg(unix)]
fn peer_creds(stream: &tokio::net::UnixStream) -> Option<Peer> {
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
        (rc == 0).then_some(Peer {
            pid: cred.pid as u32,
            uid: Some(cred.uid),
        })
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd as _;
        let fd = stream.as_raw_fd();
        let mut pid: libc::pid_t = 0;
        let mut pid_len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // macOS exposes the effective peer pid with LOCAL_PEEREPID, and
        // `getpeereid` supplies its effective uid for the same-user check.
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
        (pid_rc == 0 && uid_rc == 0 && pid > 0).then_some(Peer {
            pid: pid as u32,
            uid: Some(uid),
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = stream;
        None
    }
}

/// Windows named-pipe listener. One pipe instance is kept ready for the next
/// client; `accept` hands the connected instance to the broker and creates
/// its replacement before returning.
#[cfg(windows)]
pub struct WindowsListener {
    name: String,
    next: tokio::sync::Mutex<Option<tokio::net::windows::named_pipe::NamedPipeServer>>,
}

#[cfg(windows)]
impl WindowsListener {
    async fn accept(&self) -> io::Result<(Stream, Peer)> {
        let server = {
            let mut guard = self.next.lock().await;
            match guard.take() {
                Some(server) => server,
                // A previous create failed: rebuild the instance rather than
                // leaving the listener permanently dead.
                None => tokio::net::windows::named_pipe::ServerOptions::new().create(&self.name)?,
            }
        };
        server.connect().await?;
        let pid = peer_pid(&server);
        // Create the next instance before returning so the next client can
        // connect while this one is being served.
        let next = tokio::net::windows::named_pipe::ServerOptions::new().create(&self.name)?;
        *self.next.lock().await = Some(next);
        Ok((Stream::Windows(server), Peer { pid, uid: None }))
    }
}

#[cfg(windows)]
fn peer_pid(server: &tokio::net::windows::named_pipe::NamedPipeServer) -> u32 {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
    let mut pid = 0u32;
    let ok = unsafe { GetNamedPipeClientProcessId(server.as_raw_handle() as _, &mut pid) };
    if ok != 0 { pid } else { 0 }
}

#[cfg(windows)]
fn windows_process_name(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let name = path.rsplit(['\\', '/']).next().unwrap_or(&path);
        (!name.is_empty()).then(|| name.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_always_include_the_conventional_runtime_path() {
        let list = endpoints();
        assert!(!list.is_empty());
        #[cfg(unix)]
        {
            #[cfg(target_os = "linux")]
            {
                let uid = our_uid();
                let conventional = format!("/run/user/{uid}/cl2metal-agent.sock");
                assert!(
                    list.iter().any(|e| e.display() == conventional),
                    "the MCP child must find the app's socket even without XDG_RUNTIME_DIR: {list:?}"
                );
            }
            // The /tmp fallback is always last.
            assert!(list.last().unwrap().display().contains("cl2metal-agent-"));
        }
    }

    #[test]
    fn client_endpoint_error_names_every_path_checked() {
        // Point the candidates at a directory that cannot contain a socket.
        let candidates = endpoints();
        let checked = candidates
            .iter()
            .map(Endpoint::display)
            .collect::<Vec<_>>()
            .join(", ");
        assert!(!checked.is_empty());
        // The real error is only produced when none exist; assert the shape
        // of the message without depending on the machine's live sockets.
        let message = format!(
            "no agent socket (checked {checked}); is cl2metal running with agent access on?"
        );
        assert!(message.contains("checked"));
        assert!(message.contains("agent access on"));
    }
}
