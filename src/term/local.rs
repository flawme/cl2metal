//! Local shell sessions backed by a native PTY.

use super::{ClientMsg, MsgTx, SessionMsg, Waker};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};

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

    // Reader thread: pty → UI.
    {
        let msg_tx = msg_tx.clone();
        let mut reader = pair.master.try_clone_reader()?;
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
        let mut writer = pair.master.take_writer()?;
        let master = pair.master;
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

    Ok((client_tx, msg_rx, shell_pid))
}

/// The working directory of a shell process, when it can be read.
///
/// This is what lets the editor's file tree follow `cd`: the shell's own
/// directory is exactly where the user navigated to.
#[cfg(target_os = "linux")]
pub fn shell_cwd(pid: u32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// Without `/proc` there is nothing to read; the tree keeps its root.
#[cfg(not(target_os = "linux"))]
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
        assert_eq!(
            shell_cwd(std::process::id()),
            std::env::current_dir().ok()
        );
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn without_proc_there_is_no_directory() {
        assert!(shell_cwd(1).is_none());
    }
}
