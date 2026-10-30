//! Headless agent sessions: authenticated SSH connections that own no
//! pane, no grid and no tab, serving exec requests from the broker.
//!
//! One OS thread per session, one current-thread runtime (the pattern the
//! SFTP worker uses), so the connection's backing task stays alive for
//! the connection's lifetime. Prompts raised during connect — host key
//! trust, passwords, MFA — are forwarded to the UI from a dedicated pump
//! thread, NOT from the runtime: `ask()` blocks the runtime thread while
//! it waits for the human, so anything that must happen during that wait
//! lives outside the runtime.

use russh::ChannelMsg;
use std::time::Duration;

use crate::term::ssh::{Client, SshParams, connect_only};
use crate::term::{SessionMsg, SshRequest, Waker};

/// One exec request, queued to a session thread.
pub(crate) struct ExecJob {
    pub command: String,
    pub timeout: Duration,
    pub reply: tokio::sync::oneshot::Sender<Result<ExecOutcome, String>>,
}

/// What one exec returns.
#[derive(Debug, PartialEq)]
pub(crate) struct ExecOutcome {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Which way a file transfer goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FileDirection {
    /// Local file -> remote host.
    Upload,
    /// Remote host -> local file.
    Download,
}

/// One file transfer, queued to a session thread.
pub(crate) struct FileJob {
    pub direction: FileDirection,
    pub local_path: String,
    pub remote_path: String,
    pub reply: tokio::sync::oneshot::Sender<Result<FileOutcome, String>>,
}

/// What one file transfer returns.
#[derive(Debug, PartialEq)]
pub(crate) struct FileOutcome {
    pub bytes: u64,
    pub sha256: String,
}

/// One queued job: the session thread serves execs and transfers in order.
enum Job {
    Exec(ExecJob),
    File(FileJob),
}

/// Events a headless session reports to its owner.
pub(crate) enum SessionEvent {
    /// The connection needs a human answer (host key, password, MFA) —
    /// the same shape the panes use, so the UI pipeline is shared.
    Prompt(SshRequest, std::sync::mpsc::Sender<Result<String, ()>>),
    /// Connected and authenticated.
    Connected,
    /// The connect failed.
    Failed(String),
    /// The session ended (the broker drops the handle).
    Exited(String),
}

/// The broker-side handle to one headless session. The broker keys its
/// map by session id and derives its own display names; the handle is
/// only the job queue.
pub(crate) struct AgentSession {
    job_tx: flume::Sender<Job>,
}

impl AgentSession {
    /// Queue one exec; `None` when the session thread is already gone.
    pub fn exec(
        &self,
        command: String,
        timeout: Duration,
    ) -> Option<tokio::sync::oneshot::Receiver<Result<ExecOutcome, String>>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.job_tx
            .send(Job::Exec(ExecJob {
                command,
                timeout,
                reply: tx,
            }))
            .ok()
            .map(|()| rx)
    }

    /// Queue one upload; `None` when the session thread is already gone.
    pub fn upload(
        &self,
        local_path: String,
        remote_path: String,
    ) -> Option<tokio::sync::oneshot::Receiver<Result<FileOutcome, String>>> {
        self.enqueue_file(FileDirection::Upload, local_path, remote_path)
    }

    /// Queue one download; `None` when the session thread is already gone.
    pub fn download(
        &self,
        remote_path: String,
        local_path: String,
    ) -> Option<tokio::sync::oneshot::Receiver<Result<FileOutcome, String>>> {
        self.enqueue_file(FileDirection::Download, local_path, remote_path)
    }

    fn enqueue_file(
        &self,
        direction: FileDirection,
        local_path: String,
        remote_path: String,
    ) -> Option<tokio::sync::oneshot::Receiver<Result<FileOutcome, String>>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.job_tx
            .send(Job::File(FileJob {
                direction,
                local_path,
                remote_path,
                reply: tx,
            }))
            .ok()
            .map(|()| rx)
    }
}

/// Append `data` to `sink`, capping at [`protocol::MAX_OUTPUT_BYTES`] with
/// a truncation marker so a runaway command cannot exhaust memory.
fn cap_into(sink: &mut Vec<u8>, data: &[u8]) {
    let cap = super::protocol::MAX_OUTPUT_BYTES;
    if sink.len() >= cap {
        return;
    }
    if sink.len() + data.len() <= cap {
        sink.extend_from_slice(data);
        return;
    }
    let room = cap - sink.len();
    sink.extend_from_slice(&data[..room]);
    sink.extend_from_slice(b"\n...[truncated]");
}

/// Spawn a headless session. Returns the handle plus a one-shot that
/// resolves once connect + auth finish: `Ok(())` when the session is
/// live, `Err(reason)` when it is not. Human time is unlimited — the
/// caller decides how long to wait, not the session.
pub(crate) fn spawn(
    id: String,
    params: SshParams,
    events: flume::Sender<SessionEvent>,
    wake: Waker,
) -> (
    AgentSession,
    tokio::sync::oneshot::Receiver<Result<(), String>>,
) {
    let (job_tx, job_rx) = flume::unbounded::<Job>();
    let (msg_tx, msg_rx) = flume::unbounded::<SessionMsg>();
    // The UI wakes on every event, like every other backend message.
    let msg_tx = crate::term::MsgTx::new(msg_tx, wake.clone());

    let (outcome_tx, outcome_rx) = tokio::sync::oneshot::channel();
    let name = format!("agent-{id}");

    // The prompt pump. It must NOT live on the session runtime: `ask()`
    // blocks that thread waiting for a human reply, so the forwarder has
    // to be a plain thread. It exits when the session thread drops
    // `msg_tx`.
    let pump_events = events.clone();
    std::thread::Builder::new()
        .name(format!("{name}-pump"))
        .spawn(move || {
            while let Ok(msg) = msg_rx.recv() {
                match msg {
                    SessionMsg::SshRequest(request, reply) => {
                        let _ = pump_events.send(SessionEvent::Prompt(request, reply));
                    }
                    SessionMsg::Exited(reason) => {
                        let _ = pump_events.send(SessionEvent::Exited(reason));
                    }
                    // A headless session has no grid: data, titles and
                    // the rest are noise here.
                    _ => {}
                }
                (wake)();
            }
        })
        .expect("spawn the agent prompt pump");

    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            // The one-shot is single-use and needed on both failure paths,
            // so it travels as an Option and is taken exactly once.
            let mut outcome_tx = Some(outcome_tx);
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    report(&events, outcome_tx.take(), format!("tokio runtime: {e}"));
                    return;
                }
            };
            rt.block_on(async move {
                let Some(outcome_tx) = outcome_tx else { return };
                let conn = match connect_only(&params, &msg_tx).await {
                    Ok(conn) => {
                        let _ = events.send(SessionEvent::Connected);
                        let _ = outcome_tx.send(Ok(()));
                        conn
                    }
                    Err(e) => {
                        report(&events, Some(outcome_tx), e.to_string());
                        return;
                    }
                };
                // Serve jobs until the broker drops the handle.
                while let Ok(job) = job_rx.recv_async().await {
                    match job {
                        Job::Exec(job) => {
                            let outcome = exec_one(&conn.handle, &job).await;
                            let _ = job.reply.send(outcome);
                        }
                        Job::File(job) => {
                            let outcome = file_one(&conn.handle, &job).await;
                            let _ = job.reply.send(outcome);
                        }
                    }
                }
                let _ = conn
                    .handle
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "agent session closed",
                        "en",
                    )
                    .await;
            });
        })
        .expect("spawn the agent session thread");

    (AgentSession { job_tx }, outcome_rx)
}

/// Report a connect failure on both channels (event for the UI toast and
/// audit, one-shot for the waiting requester).
fn report(
    events: &flume::Sender<SessionEvent>,
    outcome: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    reason: String,
) {
    let _ = events.send(SessionEvent::Failed(reason.clone()));
    if let Some(outcome) = outcome {
        let _ = outcome.send(Err(reason));
    }
}

/// Run one command on the connection and collect its result.
///
/// `-1` is the "no exit status arrived" exit code: a server that closes
/// the channel without one (crash, signal) reads as failure rather than
/// silently successful.
async fn exec_one(
    handle: &russh::client::Handle<Client>,
    job: &ExecJob,
) -> Result<ExecOutcome, String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("connection lost: {e}"))?;
    channel
        .exec(true, job.command.as_bytes())
        .await
        .map_err(|e| format!("exec rejected: {e}"))?;

    let deadline = tokio::time::Instant::now() + job.timeout;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code: i32 = -1;
    loop {
        let sleep = tokio::time::sleep_until(deadline);
        tokio::select! {
            _ = sleep => {
                let _ = channel.close().await;
                return Err("timed out".into());
            }
            msg = channel.wait() => match msg {
                Some(ChannelMsg::Data { data }) => cap_into(&mut stdout, &data),
                Some(ChannelMsg::ExtendedData { data, .. }) => cap_into(&mut stderr, &data),
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    exit_code = exit_status as i32;
                }
                Some(ChannelMsg::ExitSignal { .. }) => exit_code = -1,
                Some(ChannelMsg::Close) | None => break,
                // Eof, window adjustments and friends: nothing to do.
                Some(_) => {}
            },
        }
    }
    Ok(ExecOutcome {
        exit_code,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Run one file transfer over an SFTP subsystem channel on the existing
/// authenticated connection. The whole file is buffered (capped at
/// [`protocol::MAX_TRANSFER_BYTES`]) so the sha256 covers exactly the bytes
/// that crossed the wire.
async fn file_one(
    handle: &russh::client::Handle<Client>,
    job: &FileJob,
) -> Result<FileOutcome, String> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("connection lost: {e}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("sftp subsystem rejected: {e}"))?;
    let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("sftp handshake: {e}"))?;

    match job.direction {
        FileDirection::Upload => {
            let data = tokio::fs::read(&job.local_path)
                .await
                .map_err(|e| format!("read {}: {e}", job.local_path))?;
            if data.len() as u64 > super::protocol::MAX_TRANSFER_BYTES {
                return Err(format!(
                    "file too large ({} bytes; cap {} bytes)",
                    data.len(),
                    super::protocol::MAX_TRANSFER_BYTES
                ));
            }
            let sha256 = sha256_hex(&data);
            sftp.write(&job.remote_path, &data)
                .await
                .map_err(|e| format!("write {}: {e}", job.remote_path))?;
            Ok(FileOutcome {
                bytes: data.len() as u64,
                sha256,
            })
        }
        FileDirection::Download => {
            let data = sftp
                .read(&job.remote_path)
                .await
                .map_err(|e| format!("read {}: {e}", job.remote_path))?;
            if data.len() as u64 > super::protocol::MAX_TRANSFER_BYTES {
                return Err(format!(
                    "file too large ({} bytes; cap {} bytes)",
                    data.len(),
                    super::protocol::MAX_TRANSFER_BYTES
                ));
            }
            let sha256 = sha256_hex(&data);
            tokio::fs::write(&job.local_path, &data)
                .await
                .map_err(|e| format!("write {}: {e}", job.local_path))?;
            Ok(FileOutcome {
                bytes: data.len() as u64,
                sha256,
            })
        }
    }
}

/// Lower-case hex sha256 of the transferred bytes.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_caps_at_the_limit_with_a_marker() {
        let cap = super::super::protocol::MAX_OUTPUT_BYTES;
        let mut sink = Vec::new();
        cap_into(&mut sink, &vec![b'x'; cap - 10]);
        assert_eq!(sink.len(), cap - 10);

        // The next chunk lands truncated, never past the cap.
        cap_into(&mut sink, &vec![b'y'; 4096]);
        assert_eq!(sink.len(), cap + b"\n...[truncated]".len());
        assert!(sink.ends_with(b"\n...[truncated]"));

        // Further chunks are dropped entirely.
        cap_into(&mut sink, b"more");
        assert_eq!(sink.len(), cap + b"\n...[truncated]".len());
    }

    #[test]
    fn small_outputs_pass_through_untouched() {
        let mut sink = Vec::new();
        cap_into(&mut sink, b"uptime output");
        assert_eq!(sink, b"uptime output");
    }

    #[test]
    fn session_handles_are_queueable() {
        // The exec queue is a plain flume pair; a session whose thread is
        // gone must reject new jobs instead of leaking them.
        let (tx, rx) = flume::unbounded::<ExecJob>();
        drop(rx);
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        assert!(
            tx.send(ExecJob {
                command: "true".into(),
                timeout: Duration::from_secs(1),
                reply: reply_tx,
            })
            .is_err()
        );
        // The reply channel died with the job; awaiting it errors.
        drop(reply_rx);
    }
}
