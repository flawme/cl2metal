//! The MCP stdio server (`cl2metal mcp`) — the process coding-agent
//! hosts launch to use cl2metal's SSH profiles.
//!
//! One JSON-RPC message per line on stdin/stdout, following the
//! 2025-06-18 MCP lifecycle: `initialize` → `initialized` →
//! `tools/list` → `tools/call` (`ping` any time). Each tool call becomes
//! one request to the broker socket the running app owns, so every
//! decision — identity, consent, allowlist, audit — stays in the app.
//! This process holds no secrets and opens no sockets beyond the
//! broker's; it exits when stdin closes.

use std::io::{BufRead, BufReader, Write};

use serde_json::{Value, json};

use super::protocol::{Op, Request, Response};
use super::transport::{self, BlockingStream};

/// The only protocol version this server speaks. A client asking for
/// another version gets this one back, which is the spec's negotiation:
/// echo when it matches, offer ours otherwise.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// JSON-RPC error codes this server answers with.
mod code {
    pub const PARSE: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
}

/// What every connecting agent is told, in the `initialize` result.
const INSTRUCTIONS: &str = "\
cl2metal is this machine's terminal app; it owns the SSH profiles and \
their credentials. You never see secrets: you request sessions, the \
human approves each one in a popup, and every command is audited \
locally.

- Call list_profiles first: it doubles as the availability check. If it \
errors, agent access is off in cl2metal's settings (Settings -> \
Security) or the app is not running — say so and stop.
- connect pops a consent dialog on the user's screen. They may take up \
to two minutes to answer; do not retry in a loop.
- One connect per task: reuse the session for every command, close it \
when done. Each connect is a popup the human has to answer.
- Commands are screened: read-only ones (ls, cat on a plain file, git \
status, ...) run without asking; everything else — anything that \
writes, runs an unknown tool, or touches .env, dotfiles or key \
material — pops an approval with the exact command. The human may \
answer once for the whole session. \"consent denied\" or \"consent timed \
out\" means stop and say so; do not retry.
- exec returns exit_code, stdout and stderr (each capped at 1 MiB); the \
timeout defaults to 15 s.
- upload {session, local_path, remote_path} and download {session, \
remote_path, local_path} move one file over SFTP (64 MiB cap). Both return \
bytes and sha256, and both need the human's approval unless they already \
chose \"allow all\".
- sessions lists the sessions this process has open; audit_tail returns the \
last N audit lines (default 50, max 500).
- If the user enabled read-only mode, only the curated read-only commands \
run; writes are refused outright.
- The user can read everything you ran in their agent audit log.";

/// The built-in agent skill, printed by `cl2metal mcp skill`. Ready to
/// drop into any host's skills directory as-is.
pub const SKILL_MD: &str = r#"---
name: cl2metal-mcp
description: Use cl2metal's SSH sessions through the local MCP broker - check availability with list_profiles, connect with human consent, run screened commands with exec, close when done
---

# cl2metal (SSH via the local terminal app)

cl2metal is a terminal + SSH client. Its MCP server lets you use the SSH
profiles saved in the running cl2metal app: the app owns the
credentials, the human approves every session in a popup, and every
command is written to the user's local audit log.

## Availability

`list_profiles` is the health check. It only works while both hold:

1. the cl2metal app is running, and
2. Settings -> Security -> "Let local coding agents use SSH profiles
   (MCP)" is on.

If the tool errors, report exactly that — agent access is off or the
app is not running — and stop. Do not retry in a loop.

## The tools

| tool | what it does |
|---|---|
| `list_profiles` | the SSH profiles the app allows agents to use |
| `connect {profile}` | opens a session; a consent popup appears on the user's screen and they must click Allow |
| `exec {session, command, timeout_ms?}` | runs one command; read-only ones run freely, everything else needs the user's approval; returns `exit_code`, `stdout`, `stderr` |
| `upload {session, local_path, remote_path}` | copies a local file to the host over SFTP; returns `bytes` and `sha256` |
| `download {session, remote_path, local_path}` | copies a host file to this machine over SFTP; returns `bytes` and `sha256` |
| `sessions` | the sessions this MCP process has open |
| `audit_tail {lines?}` | the last N lines of the local agent audit log |
| `close {session}` | closes the session |

## How to behave

- Connect once per task and reuse the session for all commands; close
  it at the end. Every `connect` is a popup the human has to answer.
- `connect` answering "consent denied" or "consent timed out" means the
  user said no or never saw the popup. Stop and say so.
- Commands are screened before they run. Read-only commands (listing,
  reading plain files, `git status`) execute without asking; anything
  that writes, runs an unknown tool, or touches `.env`, dotfiles or
  key material pops an approval with the exact command. If `exec`
  answers "consent denied" or "consent timed out", stop and say so.
- The approval modal offers "Allow all future commands": after that
  click, the rest of the session runs without prompts.
- `exec` output is capped at 1 MiB per stream; `timeout_ms` defaults to
  15000 and clamps to 100..120000.
- Sessions close on their own if this process exits; `close` is still
  the polite end.

## Wiring this server into a host (for the human)

The one-command way: open cl2metal and type `:mcp-on` — it registers
this server into every coding-agent host it finds on the machine
(Sarvam Code, Codex, Claude Code, Antigravity, Cursor, Windsurf) and
reports what it did.

The manual shapes:

Sarvam Code — `~/.sarvam/config.toml` or `<repo>/.sarvam/config.toml`:

    [mcp_servers.cl2metal]
    command = "cl2metal"
    args = ["mcp"]

Codex — `~/.codex/config.toml`:

    [mcp_servers.cl2metal]
    command = "cl2metal"
    args = ["mcp"]

Claude Code:

    claude mcp add cl2metal -- cl2metal mcp

Antigravity — IDE: agent side panel `…` menu → MCP Servers → Manage MCP
Servers → View raw config. CLI: type `/mcp` in the prompt panel for the
manager overlay, or edit the files directly — global
`~/.gemini/config/mcp_config.json`, workspace `.agents/mcp_config.json`.
The shape is the same everywhere:

    {
      "mcpServers": {
        "cl2metal": {
          "command": "cl2metal",
          "args": ["mcp"]
        }
      }
    }

`cl2metal mcp skill` prints this document.
"#;

/// Run the stdio server until stdin closes.
pub fn run() -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut broker = BrokerClient::new();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| format!("stdin: {e}"))?;
        if let Some(response) = handle_line(&mut broker, &line) {
            let mut text = serde_json::to_string(&response).map_err(|e| format!("encode: {e}"))?;
            text.push('\n');
            out.write_all(text.as_bytes())
                .map_err(|e| format!("stdout: {e}"))?;
            out.flush().map_err(|e| format!("stdout: {e}"))?;
        }
    }
    Ok(())
}

/// One incoming line becomes at most one response line. Notifications
/// (no `id`) and blank lines get no answer at all.
fn handle_line(broker: &mut BrokerClient, line: &str) -> Option<Value> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(e) => {
            return Some(error_response(
                Value::Null,
                code::PARSE,
                &format!("parse error: {e}"),
            ));
        }
    };
    let Some(fields) = message.as_object() else {
        return Some(error_response(
            Value::Null,
            code::INVALID_REQUEST,
            "the message must be an object",
        ));
    };
    // A message without an id is a notification: never answer it.
    let id = fields.get("id").cloned()?;
    let Some(method) = fields.get("method").and_then(Value::as_str) else {
        return Some(error_response(id, code::INVALID_REQUEST, "missing method"));
    };
    let params = fields.get("params").cloned().unwrap_or_else(|| json!({}));
    Some(match method {
        "initialize" => result_response(id, initialize_result(&params)),
        "ping" => result_response(id, json!({})),
        "tools/list" => result_response(id, json!({ "tools": tools() })),
        "tools/call" => match call_tool(broker, &params) {
            ToolCall::Result(result) => result_response(id, result),
            ToolCall::Protocol(message) => error_response(id, code::INVALID_PARAMS, &message),
        },
        other => error_response(
            id,
            code::METHOD_NOT_FOUND,
            &format!("unknown method: {other}"),
        ),
    })
}

/// The `initialize` result. We speak one protocol version, so the
/// negotiation is simply offering it.
fn initialize_result(params: &Value) -> Value {
    // Read for the handshake log; the answer is the same either way.
    let _ = params.get("protocolVersion");
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "cl2metal", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

/// The tools, one per broker op. Annotations follow the MCP 2025-06-18
/// tool metadata: read-only tools are marked, and the ones that write or
/// reach the network carry the matching hints.
fn tools() -> Value {
    json!([
        {
            "name": "list_profiles",
            "description": "List the SSH profiles cl2metal lets agents use. Doubles as the availability check: an error means the app is not running or agent access is off in its settings.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "connect",
            "description": "Open an SSH session to a saved profile. A consent popup appears in the cl2metal window; the human must click Allow. Returns the session id to pass to exec, upload and download.",
            "inputSchema": {
                "type": "object",
                "properties": { "profile": { "type": "string" } },
                "required": ["profile"]
            },
            "annotations": { "openWorldHint": true }
        },
        {
            "name": "exec",
            "description": "Run one command in an open session and capture stdout, stderr and the exit code. Each output stream is capped at 1 MiB.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string" },
                    "command": { "type": "string" },
                    "timeout_ms": { "type": "integer", "description": "Optional; clamped to 100..120000, default 15000." }
                },
                "required": ["session", "command"]
            },
            "annotations": { "destructiveHint": true, "openWorldHint": true }
        },
        {
            "name": "upload",
            "description": "Copy a local file to the remote host over SFTP. Returns the byte count and sha256 of the transferred bytes. The human approves the transfer; the file is capped at 64 MiB.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string" },
                    "local_path": { "type": "string", "description": "Path on this machine." },
                    "remote_path": { "type": "string", "description": "Path on the SSH host." }
                },
                "required": ["session", "local_path", "remote_path"]
            },
            "annotations": { "destructiveHint": true, "idempotentHint": true }
        },
        {
            "name": "download",
            "description": "Copy a remote file to this machine over SFTP. Returns the byte count and sha256 of the transferred bytes. The human approves the transfer; the file is capped at 64 MiB.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string" },
                    "remote_path": { "type": "string", "description": "Path on the SSH host." },
                    "local_path": { "type": "string", "description": "Path on this machine." }
                },
                "required": ["session", "remote_path", "local_path"]
            },
            "annotations": { "destructiveHint": true, "idempotentHint": true }
        },
        {
            "name": "sessions",
            "description": "List the sessions this MCP process has open: id, user@host, and whether commands are auto-approved.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "audit_tail",
            "description": "Return the last N lines of the local agent audit log (default 50, max 500).",
            "inputSchema": {
                "type": "object",
                "properties": { "lines": { "type": "integer", "description": "1..500, default 50." } }
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "close",
            "description": "Close an open session. Sessions also close on their own when this process exits.",
            "inputSchema": {
                "type": "object",
                "properties": { "session": { "type": "string" } },
                "required": ["session"]
            },
            "annotations": { "destructiveHint": true, "idempotentHint": true }
        }
    ])
}

/// What one tools/call resolved to.
enum ToolCall {
    /// A tools/call result for the host.
    Result(Value),
    /// A protocol refusal (unknown tool, missing argument); the message
    /// is complete and ready for the wire.
    Protocol(String),
}

fn call_tool(broker: &mut BrokerClient, params: &Value) -> ToolCall {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return ToolCall::Protocol("missing tool name".into());
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let op = match name {
        "list_profiles" => Op::ListProfiles,
        "connect" => {
            let Some(profile) = arguments.get("profile").and_then(Value::as_str) else {
                return ToolCall::Protocol("connect needs a profile".into());
            };
            Op::Connect {
                profile: profile.to_owned(),
            }
        }
        "exec" => {
            let Some(session) = arguments.get("session").and_then(Value::as_str) else {
                return ToolCall::Protocol("exec needs a session".into());
            };
            let Some(command) = arguments.get("command").and_then(Value::as_str) else {
                return ToolCall::Protocol("exec needs a command".into());
            };
            Op::Exec {
                session: session.to_owned(),
                command: command.to_owned(),
                timeout_ms: arguments.get("timeout_ms").and_then(Value::as_u64),
            }
        }
        "upload" => {
            let Some(session) = arguments.get("session").and_then(Value::as_str) else {
                return ToolCall::Protocol("upload needs a session".into());
            };
            let Some(local_path) = arguments.get("local_path").and_then(Value::as_str) else {
                return ToolCall::Protocol("upload needs a local_path".into());
            };
            let Some(remote_path) = arguments.get("remote_path").and_then(Value::as_str) else {
                return ToolCall::Protocol("upload needs a remote_path".into());
            };
            Op::Upload {
                session: session.to_owned(),
                local_path: local_path.to_owned(),
                remote_path: remote_path.to_owned(),
            }
        }
        "download" => {
            let Some(session) = arguments.get("session").and_then(Value::as_str) else {
                return ToolCall::Protocol("download needs a session".into());
            };
            let Some(remote_path) = arguments.get("remote_path").and_then(Value::as_str) else {
                return ToolCall::Protocol("download needs a remote_path".into());
            };
            let Some(local_path) = arguments.get("local_path").and_then(Value::as_str) else {
                return ToolCall::Protocol("download needs a local_path".into());
            };
            Op::Download {
                session: session.to_owned(),
                remote_path: remote_path.to_owned(),
                local_path: local_path.to_owned(),
            }
        }
        "sessions" => Op::Sessions,
        "audit_tail" => Op::AuditTail {
            lines: arguments
                .get("lines")
                .and_then(Value::as_u64)
                .map(|n| n as usize),
        },
        "close" => {
            let Some(session) = arguments.get("session").and_then(Value::as_str) else {
                return ToolCall::Protocol("close needs a session".into());
            };
            Op::Close {
                session: session.to_owned(),
            }
        }
        unknown => return ToolCall::Protocol(format!("unknown tool: {unknown}")),
    };
    match broker.request(op) {
        // The broker's own line is the tool text: compact, complete,
        // and exactly what the app decided.
        Ok(response) => ToolCall::Result(tool_result(
            serde_json::to_string(&response)
                .unwrap_or_else(|_| r#"{"ok":false,"error":"encode failed"}"#.to_owned()),
            !response.ok,
        )),
        // A broker refusal is a tool failure the agent can read and
        // report: app not running, access off, consent denied.
        Err(error) => ToolCall::Result(tool_result(error, true)),
    }
}

/// One tools/call result: the text, plus the error flag when it failed.
fn tool_result(text: String, is_error: bool) -> Value {
    let mut result = json!({ "content": [{ "type": "text", "text": text }] });
    if is_error {
        result["isError"] = json!(true);
    }
    result
}

fn result_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The broker connection. Opened on first use and kept for the
/// process's life: broker sessions are per-connection, so reconnecting
/// would strand every open session.
struct BrokerClient {
    /// The endpoint to use; `None` resolves the app's candidates lazily.
    socket: Option<transport::Endpoint>,
    reader: Option<BufReader<BlockingStream>>,
    writer: Option<BlockingStream>,
    next_id: u64,
}

impl BrokerClient {
    fn new() -> Self {
        Self {
            socket: None,
            reader: None,
            writer: None,
            next_id: 1,
        }
    }

    /// A client pinned to one endpoint, so failure is deterministic
    /// wherever a test runs. Tests only: the live client always
    /// resolves the app's own candidates.
    #[cfg(test)]
    fn pinned(socket: transport::Endpoint) -> Self {
        Self {
            socket: Some(socket),
            reader: None,
            writer: None,
            next_id: 1,
        }
    }

    /// One request, one response. Errors are strings ready to become
    /// tool text.
    fn request(&mut self, op: Op) -> Result<Response, String> {
        if self.reader.is_none() {
            self.connect()
                .map_err(|e| format!("cl2metal is not running ({e})"))?;
        }
        let request = Request {
            id: self.next_id,
            op,
        };
        self.next_id += 1;
        let mut line = serde_json::to_string(&request).map_err(|e| format!("encode: {e}"))?;
        line.push('\n');
        let writer = self.writer.as_mut().expect("connect left a writer");
        writer
            .write_all(line.as_bytes())
            .map_err(|e| format!("broker write failed: {e}"))?;
        writer
            .flush()
            .map_err(|e| format!("broker write failed: {e}"))?;
        let reader = self.reader.as_mut().expect("connect left a reader");
        let mut response = String::new();
        let read = reader
            .read_line(&mut response)
            .map_err(|e| format!("broker read failed: {e}"))?;
        if read == 0 {
            return Err("cl2metal closed the connection".into());
        }
        serde_json::from_str(&response).map_err(|e| format!("unreadable broker reply: {e}"))
    }

    fn connect(&mut self) -> std::io::Result<()> {
        let endpoint = match self.socket.clone() {
            Some(endpoint) => endpoint,
            None => transport::client_endpoint()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e))?,
        };
        let stream = transport::connect_blocking(&endpoint)?;
        self.writer = Some(stream.try_clone()?);
        self.reader = Some(BufReader::new(stream));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned to an endpoint that cannot exist, so every call fails the same
    /// way on every machine.
    fn dead_broker() -> BrokerClient {
        #[cfg(unix)]
        let endpoint =
            transport::Endpoint::Unix(std::path::PathBuf::from("/nonexistent/cl2metal-mcp-test"));
        #[cfg(windows)]
        let endpoint =
            transport::Endpoint::Windows(r"\\.\pipe\cl2metal-mcp-test-does-not-exist".to_owned());
        BrokerClient::pinned(endpoint)
    }

    #[test]
    fn initialize_handshake_matches_the_2025_06_18_shape() {
        let mut broker = dead_broker();
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;
        let response = handle_line(&mut broker, line).expect("answered");
        assert_eq!(response["id"], json!(1));
        assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(response["result"]["serverInfo"]["name"], "cl2metal");
        assert!(response["result"]["capabilities"]["tools"].is_object());
        let instructions = response["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains("list_profiles"));
        assert!(instructions.contains("audit"));
        assert!(
            instructions.contains("screened"),
            "teaches the command gate"
        );
    }

    #[test]
    fn initialized_notification_gets_no_answer() {
        let mut broker = dead_broker();
        assert!(
            handle_line(
                &mut broker,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
            )
            .is_none()
        );
    }

    #[test]
    fn tools_list_names_every_broker_op() {
        let mut broker = dead_broker();
        let response = handle_line(
            &mut broker,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        )
        .expect("answered");
        let names: Vec<&str> = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "list_profiles",
                "connect",
                "exec",
                "upload",
                "download",
                "sessions",
                "audit_tail",
                "close"
            ]
        );
        for tool in response["result"]["tools"].as_array().unwrap() {
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["description"].as_str().is_some_and(|d| !d.is_empty()));
        }
        // The read-only tools carry the MCP annotation; the writing ones
        // carry the destructive hint.
        let by_name = |name: &str| {
            response["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap()
                .clone()
        };
        assert_eq!(
            by_name("list_profiles")["annotations"]["readOnlyHint"],
            true
        );
        assert_eq!(by_name("sessions")["annotations"]["readOnlyHint"], true);
        assert_eq!(by_name("audit_tail")["annotations"]["readOnlyHint"], true);
        assert_eq!(by_name("upload")["annotations"]["destructiveHint"], true);
        assert_eq!(by_name("download")["annotations"]["destructiveHint"], true);
    }

    #[test]
    fn a_dead_broker_is_a_tool_error_not_a_protocol_error() {
        let mut broker = dead_broker();
        let line = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_profiles","arguments":{}}}"#;
        let response = handle_line(&mut broker, line).expect("answered");
        assert_eq!(response["id"], json!(3));
        assert_eq!(response["result"]["isError"], json!(true));
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("not running"), "text said: {text}");
        // A refusal is a result, not a JSON-RPC error.
        assert!(response.get("error").is_none());
    }

    #[test]
    fn unknown_tools_and_methods_answer_with_their_codes() {
        let mut broker = dead_broker();
        let response = handle_line(
            &mut broker,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"reboot","arguments":{}}}"#,
        )
        .expect("answered");
        assert_eq!(response["error"]["code"], json!(-32602));
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("unknown tool: reboot")
        );

        let response = handle_line(
            &mut broker,
            r#"{"jsonrpc":"2.0","id":5,"method":"resources/list"}"#,
        )
        .expect("answered");
        assert_eq!(response["error"]["code"], json!(-32601));
    }

    #[test]
    fn missing_arguments_are_named_in_the_error() {
        let mut broker = dead_broker();
        let line = r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"connect","arguments":{}}}"#;
        let response = handle_line(&mut broker, line).expect("answered");
        assert_eq!(response["error"]["code"], json!(-32602));
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("profile")
        );
    }

    #[test]
    fn transfer_tools_name_the_missing_argument() {
        let mut broker = dead_broker();
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"upload","arguments":{"session":"s_0123456789ab"}}}"#;
        let response = handle_line(&mut broker, line).expect("answered");
        assert_eq!(response["error"]["code"], json!(-32602));
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("local_path")
        );

        let line = r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"download","arguments":{"session":"s_0123456789ab","remote_path":"/root/a"}}}"#;
        let response = handle_line(&mut broker, line).expect("answered");
        assert_eq!(response["error"]["code"], json!(-32602));
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("local_path")
        );
    }

    #[test]
    fn garbage_lines_answer_as_parse_errors() {
        let mut broker = dead_broker();
        let response = handle_line(&mut broker, "not json").expect("answered");
        assert_eq!(response["error"]["code"], json!(-32700));
        assert_eq!(response["id"], Value::Null);

        // Blank input is skipped silently.
        assert!(handle_line(&mut broker, "   ").is_none());
    }

    #[test]
    fn ping_answers_an_empty_result() {
        let mut broker = dead_broker();
        let response = handle_line(&mut broker, r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#)
            .expect("answered");
        assert_eq!(response["result"], json!({}));
        assert!(response.get("error").is_none());
    }

    #[test]
    fn the_skill_prints_as_a_real_skill_file() {
        assert!(SKILL_MD.starts_with("---\n"), "frontmatter comes first");
        assert!(SKILL_MD.contains("name: cl2metal-mcp"));
        // The hosts a human might wire up.
        assert!(SKILL_MD.contains("[mcp_servers.cl2metal]"));
        assert!(SKILL_MD.contains("claude mcp add cl2metal"));
        assert!(SKILL_MD.contains("mcpServers"), "the Antigravity shape");
        assert!(
            SKILL_MD.contains(".agents/mcp_config.json"),
            "the Antigravity CLI paths"
        );
        // The one-command registration sweep.
        assert!(SKILL_MD.contains(":mcp-on"), "the registration command");
        // The skill must teach the availability check the user asked for.
        assert!(SKILL_MD.contains("list_profiles"));
        // The transfer and inspection tools are part of the contract.
        assert!(SKILL_MD.contains("upload"));
        assert!(SKILL_MD.contains("download"));
        assert!(SKILL_MD.contains("sessions"));
        assert!(SKILL_MD.contains("audit_tail"));
        // And the command screening the user asked for.
        assert!(
            SKILL_MD.contains("Allow all future commands"),
            "the approval modal's blanket option"
        );
        assert!(
            SKILL_MD.contains(".env") && SKILL_MD.contains("dotfiles"),
            "the always-ask rule for secrets"
        );
    }
}
