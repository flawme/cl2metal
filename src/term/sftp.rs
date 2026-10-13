//! SFTP client over an SSH channel.
//!
//! The worker owns its own SSH connection: the terminal session thread owns its
//! channel loop, so a second connection is the clean way to get an independent
//! subsystem channel. Requests arrive over a flume channel and are served in
//! order; results are reported back as [`SftpEvent`]s.
//!
//! Contract:
//! - `Connected` is sent once, after the subsystem handshake, carrying the
//!   session's canonical home directory.
//! - `Mkdir`, `Remove` and `Rename` are confirmed by re-listing the affected
//!   directory (`Listed`), so the UI can refresh without a round trip.
//! - `Cancel` aborts the transfer in flight at the next 64 KiB chunk boundary.
//! - `Error` carries a short reason only: file contents and credentials are
//!   never echoed back.

// The UI side that drives this worker is wired up separately, so until it
// lands every item here reads as unused.
#![allow(dead_code)]

use super::ssh::{SshConnection, SshParams, connect_only};
use super::{MsgTx, SessionMsg, Waker};
use russh_sftp::client::SftpSession;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Streamed transfers move this many bytes per chunk.
const CHUNK: usize = 64 * 1024;

/// What the UI asks the worker to do.
#[derive(Clone, Debug)]
pub(crate) enum SftpRequest {
    /// List an absolute remote directory.
    List(String),
    /// Stat an absolute remote path.
    Stat(String),
    /// Read an absolute remote file.
    Read(String),
    /// Write a file, creating or truncating it.
    Write { path: String, data: Vec<u8> },
    /// Create a directory.
    Mkdir(String),
    /// Remove a file, or an empty directory when `dir` is set.
    Remove { path: String, dir: bool },
    /// Rename or move a path.
    Rename { from: String, to: String },
    /// Abort the transfer in flight.
    Cancel,
}

/// One remote directory entry.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) dir: bool,
    pub(crate) size: u64,
    pub(crate) mode: u32,
    /// Modification time in unix seconds, when the server reports one.
    pub(crate) modified: Option<u64>,
}

/// What the worker reports back.
#[derive(Clone, Debug)]
pub(crate) enum SftpEvent {
    /// The subsystem is up; `home` is the session's canonical home directory.
    Connected { home: String },
    /// A directory listing, sorted directories-first.
    Listed { path: String, entries: Vec<Entry> },
    /// Metadata for a single path.
    Stat { path: String, entry: Entry },
    /// The full contents of a file.
    Read { path: String, data: Vec<u8> },
    /// A file was written.
    Written { path: String, bytes: usize },
    /// Transfer progress, emitted after every chunk.
    Progress {
        path: String,
        done: u64,
        total: Option<u64>,
    },
    /// Something failed; `context` names the operation.
    Error { context: String, message: String },
    /// The worker is done: the request channel closed or the connection died.
    Disconnected,
}

/// Start the worker. It owns its SSH connection and exits when `rx` closes.
pub(crate) fn spawn(
    params: SshParams,
    wake: Waker,
    tx: flume::Sender<SftpEvent>,
    rx: flume::Receiver<SftpRequest>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                fail(&tx, "sftp runtime", &e);
                let _ = tx.send(SftpEvent::Disconnected);
                return;
            }
        };
        rt.block_on(worker(params, wake, tx, rx));
    })
}

/// Connect, open the subsystem, then serve requests until `rx` closes.
async fn worker(
    params: SshParams,
    wake: Waker,
    tx: flume::Sender<SftpEvent>,
    rx: flume::Receiver<SftpRequest>,
) {
    // `connect_only` asks the UI to confirm trust-on-first-use host keys
    // through a `MsgTx`. This worker has no UI channel of its own, so the
    // receiver is dropped: an unknown host fails fast with a clear error
    // instead of blocking forever. Hosts already in `known_hosts` (for
    // example because a terminal session to them is open) connect normally.
    let (msg_tx, msg_rx) = flume::unbounded::<SessionMsg>();
    drop(msg_rx);
    let msg_tx = MsgTx::new(msg_tx, wake);

    let conn = match connect_only(&params, &msg_tx).await {
        Ok(conn) => conn,
        Err(e) => {
            fail(&tx, "connect", &e);
            let _ = tx.send(SftpEvent::Disconnected);
            return;
        }
    };

    let session = match open_subsystem(&conn).await {
        Ok(session) => session,
        Err(e) => {
            fail(&tx, "sftp subsystem", &e);
            let _ = tx.send(SftpEvent::Disconnected);
            return;
        }
    };

    // The canonical home directory. `canonicalize(".")` is the SFTP realpath
    // call; if the server refuses it the home is unknown, so fall back to "/",
    // the one path that always exists.
    let home = session
        .canonicalize(".")
        .await
        .unwrap_or_else(|_| "/".to_owned());
    let _ = tx.send(SftpEvent::Connected { home });

    let cancel = AtomicBool::new(false);
    let mut pending: VecDeque<SftpRequest> = VecDeque::new();
    loop {
        let req = match pending.pop_front() {
            Some(req) => req,
            None => match rx.recv_async().await {
                Ok(req) => req,
                // The UI dropped its end: shut down.
                Err(_) => break,
            },
        };
        if matches!(req, SftpRequest::Cancel) {
            cancel.store(true, Ordering::SeqCst);
            continue;
        }
        // A fresh request starts a fresh transfer.
        cancel.store(false, Ordering::SeqCst);
        if matches!(
            handle(&session, req, &tx, &rx, &mut pending, &cancel).await,
            Transfer::Shutdown
        ) {
            break;
        }
    }

    let _ = session.close().await;
    let _ = conn
        .handle
        .disconnect(russh::Disconnect::ByApplication, "", "English")
        .await;
    let _ = tx.send(SftpEvent::Disconnected);
}

/// Open a session channel, request the `sftp` subsystem and wrap the channel
/// stream in a high-level session.
async fn open_subsystem(conn: &SshConnection) -> anyhow::Result<SftpSession> {
    let channel = conn.handle.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    Ok(SftpSession::new(channel.into_stream()).await?)
}

/// Serve one request. Returns [`Transfer::Shutdown`] when the request channel
/// closed mid-transfer and the worker should exit.
async fn handle(
    session: &SftpSession,
    req: SftpRequest,
    tx: &flume::Sender<SftpEvent>,
    rx: &flume::Receiver<SftpRequest>,
    pending: &mut VecDeque<SftpRequest>,
    cancel: &AtomicBool,
) -> Transfer {
    match req {
        SftpRequest::List(path) => {
            match list(session, &path).await {
                Ok(entries) => {
                    let _ = tx.send(SftpEvent::Listed { path, entries });
                }
                Err(e) => fail(tx, &format!("list {path}"), &e),
            }
            Transfer::Done
        }
        SftpRequest::Stat(path) => {
            match stat(session, &path).await {
                Ok(entry) => {
                    let _ = tx.send(SftpEvent::Stat { path, entry });
                }
                Err(e) => fail(tx, &format!("stat {path}"), &e),
            }
            Transfer::Done
        }
        SftpRequest::Read(path) => read_stream(session, &path, tx, rx, pending, cancel).await,
        SftpRequest::Write { path, data } => {
            write_stream(session, &path, &data, tx, rx, pending, cancel).await
        }
        SftpRequest::Mkdir(path) => {
            match session.create_dir(path.as_str()).await {
                Ok(()) => refresh(session, &parent_remote(&path), tx).await,
                Err(e) => fail(tx, &format!("mkdir {path}"), &e),
            }
            Transfer::Done
        }
        SftpRequest::Remove { path, dir } => {
            let result = if dir {
                session.remove_dir(path.as_str()).await
            } else {
                session.remove_file(path.as_str()).await
            };
            match result {
                Ok(()) => refresh(session, &parent_remote(&path), tx).await,
                Err(e) => fail(tx, &format!("remove {path}"), &e),
            }
            Transfer::Done
        }
        SftpRequest::Rename { from, to } => {
            match session.rename(from.as_str(), to.as_str()).await {
                Ok(()) => refresh(session, &parent_remote(&to), tx).await,
                Err(e) => fail(tx, &format!("rename {from} to {to}"), &e),
            }
            Transfer::Done
        }
        // Handled by the caller, which sets the flag and never gets here.
        SftpRequest::Cancel => Transfer::Done,
    }
}

/// Re-list a directory after a mutation so the UI can refresh.
async fn refresh(session: &SftpSession, dir: &str, tx: &flume::Sender<SftpEvent>) {
    match list(session, dir).await {
        Ok(entries) => {
            let _ = tx.send(SftpEvent::Listed {
                path: dir.to_owned(),
                entries,
            });
        }
        Err(e) => fail(tx, &format!("refresh {dir}"), &e),
    }
}

/// Read a directory and turn it into sorted [`Entry`] values.
async fn list(session: &SftpSession, path: &str) -> Result<Vec<Entry>, String> {
    let dir = session.read_dir(path).await.map_err(|e| e.to_string())?;
    let mut entries: Vec<Entry> = dir
        .map(|entry| {
            let metadata = entry.metadata();
            let name = entry.file_name();
            let path = join_remote(path, &name);
            Entry {
                name,
                path,
                dir: metadata.file_type().is_dir(),
                size: metadata.size.unwrap_or(0),
                mode: metadata.permissions.unwrap_or(0),
                modified: metadata.mtime.map(u64::from),
            }
        })
        .collect();
    sort_entries(&mut entries);
    Ok(entries)
}

/// Stat one path.
async fn stat(session: &SftpSession, path: &str) -> Result<Entry, String> {
    let metadata = session.metadata(path).await.map_err(|e| e.to_string())?;
    Ok(Entry {
        name: base_name(path),
        path: path.to_owned(),
        dir: metadata.file_type().is_dir(),
        size: metadata.size.unwrap_or(0),
        mode: metadata.permissions.unwrap_or(0),
        modified: metadata.mtime.map(u64::from),
    })
}

/// Stream a file down in [`CHUNK`]-sized pieces, reporting progress as it goes.
async fn read_stream(
    session: &SftpSession,
    path: &str,
    tx: &flume::Sender<SftpEvent>,
    rx: &flume::Receiver<SftpRequest>,
    pending: &mut VecDeque<SftpRequest>,
    cancel: &AtomicBool,
) -> Transfer {
    let context = format!("read {path}");
    let mut file = match session.open(path).await {
        Ok(file) => file,
        Err(e) => {
            fail(tx, &context, &e);
            return Transfer::Done;
        }
    };
    // The size is only known when the server reports it; the UI shows an
    // indeterminate bar otherwise.
    let total = file.metadata().await.ok().and_then(|m| m.size);
    // Cap the pre-allocation: a huge file should not reserve its whole size up
    // front.
    let mut data = Vec::with_capacity(total.unwrap_or(0).min(1 << 20) as usize);
    let mut buf = vec![0u8; CHUNK];
    let mut done: u64 = 0;
    loop {
        match drain(rx, pending, cancel) {
            Drained::Continue => {}
            Drained::Cancelled => {
                fail(tx, &context, "cancelled");
                return Transfer::Done;
            }
            Drained::Shutdown => return Transfer::Shutdown,
        }
        let n = match file.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                fail(tx, &context, &e);
                return Transfer::Done;
            }
        };
        data.extend_from_slice(&buf[..n]);
        done += n as u64;
        let _ = tx.send(SftpEvent::Progress {
            path: path.to_owned(),
            done,
            total,
        });
    }
    if let Err(e) = file.close().await {
        fail(tx, &context, &e);
        return Transfer::Done;
    }
    let _ = tx.send(SftpEvent::Read {
        path: path.to_owned(),
        data,
    });
    Transfer::Done
}

/// Stream a file up in [`CHUNK`]-sized pieces, reporting progress as it goes.
///
/// A cancelled write leaves the partial file in place: removing it could
/// destroy a file the user cares about.
async fn write_stream(
    session: &SftpSession,
    path: &str,
    data: &[u8],
    tx: &flume::Sender<SftpEvent>,
    rx: &flume::Receiver<SftpRequest>,
    pending: &mut VecDeque<SftpRequest>,
    cancel: &AtomicBool,
) -> Transfer {
    let context = format!("write {path}");
    let mut file = match session.create(path).await {
        Ok(file) => file,
        Err(e) => {
            fail(tx, &context, &e);
            return Transfer::Done;
        }
    };
    let total = Some(data.len() as u64);
    let mut done: u64 = 0;
    for chunk in data.chunks(CHUNK) {
        match drain(rx, pending, cancel) {
            Drained::Continue => {}
            Drained::Cancelled => {
                fail(tx, &context, "cancelled");
                return Transfer::Done;
            }
            Drained::Shutdown => return Transfer::Shutdown,
        }
        if let Err(e) = file.write_all(chunk).await {
            fail(tx, &context, &e);
            return Transfer::Done;
        }
        done += chunk.len() as u64;
        let _ = tx.send(SftpEvent::Progress {
            path: path.to_owned(),
            done,
            total,
        });
    }
    if let Err(e) = file.close().await {
        fail(tx, &context, &e);
        return Transfer::Done;
    }
    let _ = tx.send(SftpEvent::Written {
        path: path.to_owned(),
        bytes: data.len(),
    });
    Transfer::Done
}

/// How a streamed transfer ended.
enum Transfer {
    /// Finished, or failed with an error already reported.
    Done,
    /// The request channel closed: the worker should exit.
    Shutdown,
}

/// What draining the request channel between chunks found.
enum Drained {
    /// Nothing to do; keep transferring.
    Continue,
    /// A `Cancel` arrived: stop at this chunk boundary.
    Cancelled,
    /// The request channel closed: the worker is shutting down.
    Shutdown,
}

/// Take any requests that arrived while a transfer was in flight. `Cancel`
/// raises the flag; anything else is queued for after the transfer, which
/// keeps the worker's request order intact.
fn drain(
    rx: &flume::Receiver<SftpRequest>,
    pending: &mut VecDeque<SftpRequest>,
    cancel: &AtomicBool,
) -> Drained {
    loop {
        match rx.try_recv() {
            Ok(SftpRequest::Cancel) => cancel.store(true, Ordering::SeqCst),
            Ok(other) => pending.push_back(other),
            Err(flume::TryRecvError::Empty) => break,
            Err(flume::TryRecvError::Disconnected) => return Drained::Shutdown,
        }
    }
    if cancel.load(Ordering::SeqCst) {
        Drained::Cancelled
    } else {
        Drained::Continue
    }
}

/// Report a failure. `message` is a short reason: never file contents.
fn fail(tx: &flume::Sender<SftpEvent>, context: &str, message: impl std::fmt::Display) {
    let _ = tx.send(SftpEvent::Error {
        context: context.to_owned(),
        message: message.to_string(),
    });
}

/// Join a remote directory and a name into an absolute POSIX path.
pub(crate) fn join_remote(dir: &str, name: &str) -> String {
    let dir = dir.trim_end_matches('/');
    if dir.is_empty() {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The parent of an absolute POSIX path (`"/"` stays `"/"`).
pub(crate) fn parent_remote(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/".to_owned(),
        Some((parent, _)) => parent.to_owned(),
    }
}

/// The last component of an absolute POSIX path (`"/"` for the root).
fn base_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((_, name)) => name.to_owned(),
        None if trimmed.is_empty() => "/".to_owned(),
        None => trimmed.to_owned(),
    }
}

/// An ls-style permission string, e.g. `"drwxr-xr-x"`.
///
/// The first character is `'d'` for a directory (bit `0o040000`), `'-'`
/// otherwise; then the user/group/other `rwx` triplets, with setuid, setgid
/// and sticky rendered as `s`/`S`, `s`/`S` and `t`/`T`.
pub(crate) fn format_mode(mode: u32) -> String {
    let mut out = String::with_capacity(10);
    out.push(if mode & 0o040000 != 0 { 'd' } else { '-' });
    // (read, write, execute, special bit, special letter) per triplet.
    let triplets = [
        (0o400, 0o200, 0o100, 0o4000, 's'),
        (0o040, 0o020, 0o010, 0o2000, 's'),
        (0o004, 0o002, 0o001, 0o1000, 't'),
    ];
    for (read, write, exec, special, letter) in triplets {
        out.push(if mode & read != 0 { 'r' } else { '-' });
        out.push(if mode & write != 0 { 'w' } else { '-' });
        let executable = mode & exec != 0;
        let special_set = mode & special != 0;
        out.push(match (executable, special_set) {
            (true, true) => letter,
            (true, false) => 'x',
            (false, true) => letter.to_ascii_uppercase(),
            (false, false) => '-',
        });
    }
    out
}

/// Directories first, then files, each case-insensitively by name.
pub(crate) fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_and_parent_handle_root_and_trailing_slashes() {
        assert_eq!(join_remote("/", "etc"), "/etc");
        assert_eq!(join_remote("/etc", "hosts"), "/etc/hosts");
        assert_eq!(join_remote("/etc/", "hosts"), "/etc/hosts");
        assert_eq!(join_remote("", "x"), "/x");

        assert_eq!(parent_remote("/"), "/");
        assert_eq!(parent_remote("/etc"), "/");
        assert_eq!(parent_remote("/etc/hosts"), "/etc");
        assert_eq!(parent_remote("/etc/"), "/");
    }

    #[test]
    fn format_mode_renders_directories_and_permissions() {
        assert_eq!(format_mode(0o040755), "drwxr-xr-x");
        assert_eq!(format_mode(0o100644), "-rw-r--r--");
        assert_eq!(format_mode(0o100600), "-rw-------");
        // setuid with and without the execute bit.
        assert_eq!(format_mode(0o104755), "-rwsr-xr-x");
        assert_eq!(format_mode(0o104644), "-rwSr--r--");
        // setgid with and without the execute bit.
        assert_eq!(format_mode(0o102755), "-rwxr-sr-x");
        assert_eq!(format_mode(0o102744), "-rwxr-Sr--");
        // sticky with and without the execute bit.
        assert_eq!(format_mode(0o041777), "drwxrwxrwt");
        assert_eq!(format_mode(0o041776), "drwxrwxrwT");
    }

    #[test]
    fn entries_sort_directories_first_then_names() {
        let entry = |name: &str, dir: bool| Entry {
            name: name.to_owned(),
            path: join_remote("/tmp", name),
            dir,
            size: 0,
            mode: if dir { 0o040755 } else { 0o100644 },
            modified: None,
        };
        let mut entries = vec![
            entry("zeta.txt", false),
            entry("Beta", true),
            entry("alpha.txt", false),
            entry("alpha", true),
        ];
        sort_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Beta", "alpha.txt", "zeta.txt"]);
    }

    #[test]
    fn requests_and_events_round_trip_through_the_channels() {
        let (req_tx, req_rx) = flume::unbounded::<SftpRequest>();
        let (ev_tx, ev_rx) = flume::unbounded::<SftpEvent>();

        req_tx.send(SftpRequest::List("/etc".to_owned())).unwrap();
        match req_rx.recv().unwrap() {
            SftpRequest::List(path) => assert_eq!(path, "/etc"),
            other => panic!("expected a List request, got {other:?}"),
        }

        let entry = Entry {
            name: "hosts".to_owned(),
            path: "/etc/hosts".to_owned(),
            dir: false,
            size: 42,
            mode: 0o100644,
            modified: Some(1_700_000_000),
        };
        ev_tx
            .send(SftpEvent::Listed {
                path: "/etc".to_owned(),
                entries: vec![entry.clone()],
            })
            .unwrap();
        match ev_rx.recv().unwrap() {
            SftpEvent::Listed { path, entries } => {
                assert_eq!(path, "/etc");
                assert_eq!(entries, vec![entry]);
            }
            other => panic!("expected a Listed event, got {other:?}"),
        }
    }
}
