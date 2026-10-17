//! Local shell sessions backed by a native PTY.

use super::{ClientMsg, MsgTx, SessionMsg, Waker};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// The shell to run when the config does not name one.
pub(crate) fn default_shell() -> String {
    #[cfg(windows)]
    {
        // PowerShell is the better shell when it is there: `pwsh` is the
        // cross-platform one, `powershell.exe` ships with Windows.
        for exe in ["pwsh.exe", "powershell.exe"] {
            if find_on_path(exe) {
                return exe.to_owned();
            }
        }
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned())
    }
}

/// Is `exe` on PATH?
#[cfg(windows)]
fn find_on_path(exe: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(exe).is_file()))
}

/// Spawn a local shell and return `(client_tx, msg_rx, shell_pid)` for the
/// UI side. The pid is the shell's own, so its working directory can be
/// followed.
pub fn spawn(
    shell: &str,
    cols: u16,
    rows: u16,
    exec: Option<&str>,
    wake: Waker,
) -> anyhow::Result<(
    flume::Sender<ClientMsg>,
    flume::Receiver<SessionMsg>,
    Option<u32>,
)> {
    let (client_tx, client_rx) = flume::unbounded::<ClientMsg>();
    let (msg_tx, msg_rx) = flume::unbounded::<SessionMsg>();
    let msg_tx = MsgTx::new(msg_tx, wake);

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    // Resolve the shell.
    let program = if shell.is_empty() {
        default_shell()
    } else {
        shell.to_owned()
    };
    let mut cmd = CommandBuilder::new(&program);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    // Shell integration (OSC 133) without touching the user's dotfiles. It is a
    // bash/zsh mechanism; the Windows shells have no equivalent hook yet, so
    // prompt marks, prompt jumping and the exit-status dot stay unix-only.
    #[cfg(unix)]
    {
        if let Some(rc) = super::integration::bash_rcfile(&program) {
            cmd.arg("--rcfile");
            cmd.arg(rc);
        }
        if let Some(dotdir) = super::integration::zsh_dotdir(&program) {
            cmd.env("ZDOTDIR", dotdir);
        }
    }
    // `--exec`: run one command instead of an interactive shell.
    if let Some(cmdline) = exec {
        #[cfg(unix)]
        {
            cmd.arg("-c");
            cmd.arg(cmdline);
        }
        #[cfg(windows)]
        {
            let is_cmd = program.to_ascii_lowercase().ends_with("cmd.exe");
            cmd.arg(if is_cmd { "/C" } else { "-Command" });
            cmd.arg(cmdline);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    log::info!("spawning shell: {cmd:?}");
    let child = pair.slave.spawn_command(cmd)?;
    // The shell's pid, so the editor's tree can follow its working directory.
    let shell_pid = child.process_id();
    // Drop the slave so the shell sees EOF if we die.
    drop(pair.slave);

    #[cfg(unix)]
    start_tasks(pair.master, child, client_rx, msg_tx)?;
    #[cfg(not(unix))]
    start_threads(pair.master, child, client_rx, msg_tx)?;

    Ok((client_tx, msg_rx, shell_pid))
}

/// Run the session's three loops as tasks on the shared runtime.
///
/// A thread per loop would grow with every pane; tasks on one fixed pool
/// do not. The pty's fd is duplicated non-blocking for the async reads
/// and writes; the master itself stays behind a lock, because it owns the
/// pty and answers resizes.
#[cfg(unix)]
fn start_tasks(
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    client_rx: flume::Receiver<ClientMsg>,
    msg_tx: MsgTx,
) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = master
        .as_raw_fd()
        .ok_or_else(|| anyhow::anyhow!("the pty has no file descriptor"))?;
    let read_fd = dup_nonblocking(fd)?;
    let write_fd = dup_nonblocking(fd)?;
    let master = std::sync::Arc::new(std::sync::Mutex::new(master));
    let killer = child.clone_killer();

    // Reader task: pty → UI.
    {
        let msg_tx = msg_tx.clone();
        super::runtime().spawn(async move {
            use std::io::Read;
            // The registration needs the runtime's reactor, so it happens
            // here rather than on the spawning thread.
            let afd = match tokio::io::unix::AsyncFd::new(std::fs::File::from(read_fd)) {
                Ok(a) => a,
                Err(e) => {
                    log::warn!("pty reader: {e}");
                    return;
                }
            };
            let mut buf = [0u8; 8192];
            loop {
                let mut guard = match afd.readable().await {
                    Ok(g) => g,
                    Err(_) => break,
                };
                match guard.try_io(|inner| {
                    let mut f = inner.get_ref();
                    f.read(&mut buf)
                }) {
                    Ok(Ok(0)) | Ok(Err(_)) => break,
                    Ok(Ok(n)) => {
                        if msg_tx.send(SessionMsg::Data(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(_) => continue,
                }
            }
        });
    }

    // Writer task: UI → pty, plus resize and kill.
    {
        let master = master.clone();
        super::runtime().spawn(async move {
            use std::io::Write;
            let afd = match tokio::io::unix::AsyncFd::new(std::fs::File::from(write_fd)) {
                Ok(a) => a,
                Err(e) => {
                    log::warn!("pty writer: {e}");
                    return;
                }
            };
            let mut killer = killer;
            while let Ok(msg) = client_rx.recv_async().await {
                match msg {
                    ClientMsg::Input(bytes) => {
                        let mut rest = &bytes[..];
                        while !rest.is_empty() {
                            let mut guard = match afd.writable().await {
                                Ok(g) => g,
                                Err(_) => return,
                            };
                            match guard.try_io(|inner| {
                                let mut f = inner.get_ref();
                                f.write(rest)
                            }) {
                                Ok(Ok(n)) => rest = &rest[n..],
                                Ok(Err(_)) => return,
                                Err(_) => continue,
                            }
                        }
                    }
                    ClientMsg::Resize {
                        cols,
                        rows,
                        cell_w,
                        cell_h,
                    } => {
                        if let Ok(m) = master.lock() {
                            let _ = m.resize(PtySize {
                                rows,
                                cols,
                                pixel_width: cell_w,
                                pixel_height: cell_h,
                            });
                        }
                    }
                    ClientMsg::Close => {
                        let _ = killer.kill();
                        return;
                    }
                }
            }
        });
    }

    // Waiter task: report child exit. A timer and `try_wait` instead of a
    // parked thread.
    {
        super::runtime().spawn(async move {
            let mut child = child;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        log::info!("shell exited: {status}");
                        let _ = msg_tx.send(SessionMsg::Exited(status.to_string()));
                        break;
                    }
                    Ok(None) => {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                    Err(e) => {
                        let _ = msg_tx.send(SessionMsg::Exited(format!("wait failed: {e}")));
                        break;
                    }
                }
            }
        });
    }
    Ok(())
}

/// A private, non-blocking duplicate of a pty fd, for the async tasks.
#[cfg(unix)]
fn dup_nonblocking(fd: std::os::fd::RawFd) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if dup < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(dup) };
    let flags = unsafe { libc::fcntl(dup, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(dup, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(owned)
}

/// The Windows path: one thread per loop, as before.
///
/// `AsyncFd` is a unix facility; on Windows the sessions keep their own
/// threads.
#[cfg(not(unix))]
fn start_threads(
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    client_rx: flume::Receiver<ClientMsg>,
    msg_tx: MsgTx,
) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    // Reader thread: pty → UI.
    {
        let msg_tx = msg_tx.clone();
        let mut reader = master.try_clone_reader()?;
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if msg_tx.send(SessionMsg::Data(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    // Writer thread: UI → pty, plus resize and kill.
    let killer = child.clone_killer();
    {
        let mut writer = master.take_writer()?;
        std::thread::spawn(move || {
            let mut killer = killer;
            while let Ok(msg) = client_rx.recv() {
                match msg {
                    ClientMsg::Input(bytes) => {
                        if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                            break;
                        }
                    }
                    ClientMsg::Resize {
                        cols,
                        rows,
                        cell_w,
                        cell_h,
                    } => {
                        let _ = master.resize(PtySize {
                            rows,
                            cols,
                            pixel_width: cell_w,
                            pixel_height: cell_h,
                        });
                    }
                    ClientMsg::Close => {
                        let _ = killer.kill();
                        break;
                    }
                }
            }
        });
    }

    // Waiter thread: report child exit.
    {
        std::thread::spawn(move || {
            let mut child = child;
            match child.wait() {
                Ok(status) => {
                    log::info!("shell exited: {status}");
                    let _ = msg_tx.send(SessionMsg::Exited(status.to_string()));
                }
                Err(e) => {
                    let _ = msg_tx.send(SessionMsg::Exited(format!("wait failed: {e}")));
                }
            }
        });
    }
    Ok(())
}

/// The names of the processes still running under a shell, if any.
///
/// This is what the close confirmation asks about: a pane whose shell has
/// living children — `npm`, an editor, an ssh session — is doing work, and
/// closing the pane would kill that work.
#[cfg(target_os = "linux")]
pub use super::linux::running_children;

#[cfg(windows)]
pub use super::windows::running_children;

#[cfg(not(any(target_os = "linux", windows)))]
pub fn running_children(_pid: u32) -> Vec<String> {
    Vec::new()
}

/// The working directory of a shell process, when it can be read.
///
/// This is what lets the editor's file tree follow `cd`: the shell's own
/// directory is exactly where the user navigated to.
#[cfg(target_os = "linux")]
pub use super::linux::shell_cwd;

#[cfg(windows)]
pub use super::windows::shell_cwd;

#[cfg(not(any(target_os = "linux", windows)))]
pub fn shell_cwd(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn the_shell_directory_can_be_read_from_proc() {
        // The test runner itself is a process, and /proc reports its cwd.
        assert_eq!(shell_cwd(std::process::id()), std::env::current_dir().ok());
    }

    #[test]
    #[cfg(windows)]
    fn the_shell_directory_can_be_read_on_windows() {
        let cwd = shell_cwd(std::process::id());
        assert!(cwd.is_some());
        let clean = |p: std::path::PathBuf| {
            p.to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_ascii_lowercase()
        };
        assert_eq!(cwd.map(clean), std::env::current_dir().ok().map(clean));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_shell_with_a_running_child_says_so() {
        // The test runner stands in for the shell: a child it spawns is
        // exactly what the close confirmation must see.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        std::thread::sleep(std::time::Duration::from_millis(80));
        let names = running_children(std::process::id());
        assert!(
            names.iter().any(|n| n == "sleep"),
            "the child is seen: {names:?}"
        );
        let _ = child.kill();
        let _ = child.wait();
        // A reaped child is gone; nothing else runs under the test runner.
        std::thread::sleep(std::time::Duration::from_millis(80));
        let names = running_children(std::process::id());
        assert!(
            !names.iter().any(|n| n == "sleep"),
            "the reaped child is gone: {names:?}"
        );
    }

    #[test]
    fn a_session_reports_output_and_exit() {
        #[cfg(unix)]
        let (shell, script) = ("", "read line; echo got:$line");
        #[cfg(windows)]
        let (shell, script) = ("cmd.exe", "set /p line= & call echo got:%line%");

        let (client_tx, msg_rx, _pid) =
            spawn(shell, 80, 24, Some(script), std::sync::Arc::new(|| {}))
                .expect("spawn a session");
        // Give the shell a moment to start, then type into it: this covers
        // the reader, the writer, and the exit reporter in one pass.
        std::thread::sleep(std::time::Duration::from_millis(500));
        #[cfg(unix)]
        let input = b"hello\n";
        #[cfg(windows)]
        let input = b"hello\r\n";

        let mut text = String::new();
        let mut exit_reason = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut input_sent = false;

        // On Windows ConPTY, respond to initial cursor position probes before/as we send input
        let start = std::time::Instant::now();
        while std::time::Instant::now() < deadline && exit_reason.is_none() {
            match msg_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(SessionMsg::Data(bytes)) => {
                    let s = String::from_utf8_lossy(&bytes);
                    #[cfg(windows)]
                    if s.contains("\x1b[6n") {
                        let _ = client_tx.send(ClientMsg::Input(b"\x1b[1;1R".to_vec()));
                    }
                    text.push_str(&s);
                }
                Ok(SessionMsg::Exited(r)) => exit_reason = Some(r),
                Ok(_) => {}
                Err(_) => {}
            }
            if !input_sent && start.elapsed() >= std::time::Duration::from_millis(500) {
                let _ = client_tx.send(ClientMsg::Input(input.to_vec()));
                input_sent = true;
            }
        }
        // Drain any trailing Data messages queued before or alongside the exit event.
        while let Ok(msg) = msg_rx.recv_timeout(std::time::Duration::from_millis(300)) {
            if let SessionMsg::Data(bytes) = msg {
                text.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        assert!(
            text.contains("got:hello"),
            "the typed line came back: {text:?}, exit: {exit_reason:?}"
        );
        assert!(exit_reason.is_some(), "the exit was reported");
    }

    #[test]
    fn non_existent_pid_has_no_directory() {
        assert!(shell_cwd(u32::MAX).is_none());
    }

    /// The app erases an intercepted `:command` before running it in-app.
    /// On Windows that is one backspace per character (PSReadLine leaves
    /// Ctrl+U unbound); this proves the sequence clears the shell's pending
    /// line and that `\r` — not `\n` — submits the line that follows.
    #[test]
    #[cfg(windows)]
    fn erasing_a_typed_line_works_in_the_windows_shell() {
        use std::time::{Duration, Instant};

        // The default shell is what a pane gets; cmd.exe passes this too.
        let (client_tx, msg_rx, _pid) =
            spawn("", 120, 30, None, std::sync::Arc::new(|| {})).expect("spawn the default shell");

        let mut text = String::new();
        let drain = |msg_rx: &flume::Receiver<SessionMsg>,
                     client_tx: &flume::Sender<ClientMsg>,
                     text: &mut String,
                     ms: u64| {
            let deadline = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < deadline {
                match msg_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(SessionMsg::Data(bytes)) => {
                        let s = String::from_utf8_lossy(&bytes);
                        if s.contains("\x1b[6n") {
                            let _ = client_tx.send(ClientMsg::Input(b"\x1b[1;1R".to_vec()));
                        }
                        text.push_str(&s);
                    }
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
        };

        drain(&msg_rx, &client_tx, &mut text, 3000); // startup

        // Type a line, then erase it with one backspace per character.
        let typed = "echo MARKER_ONE";
        let _ = client_tx.send(ClientMsg::Input(typed.as_bytes().to_vec()));
        std::thread::sleep(Duration::from_millis(400));
        let _ = client_tx.send(ClientMsg::Input(crate::app::eggs::erase_bytes(typed.len())));
        std::thread::sleep(Duration::from_millis(400));
        // The next line must arrive clean and must submit with \r, not \n.
        let _ = client_tx.send(ClientMsg::Input(b"echo MARKER_TWO\r".to_vec()));
        drain(&msg_rx, &client_tx, &mut text, 2500);

        let _ = client_tx.send(ClientMsg::Close);
        println!("shell transcript >>>{text}<<<");
        assert!(text.contains("MARKER_TWO"), "the second line ran: {text:?}");
        assert!(
            !text.contains("MARKER_ONEecho"),
            "the first line was actually erased: {text:?}"
        );
    }
}
