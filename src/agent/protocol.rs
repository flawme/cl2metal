//! Wire protocol for the agent access socket.
//!
//! One JSON object per line in both directions. Every request carries a
//! client-chosen `id` that the response echoes. Requests are internally
//! tagged by `op`. Serde cannot combine internally tagged enums with
//! `deny_unknown_fields`, so unknown extra fields are ignored here; every
//! value that reaches a session is checked by the helpers below instead.
//!
//! The protocol is deliberately value-only: profile names, opaque session
//! ids and command text. Credentials never cross it.

use serde::{Deserialize, Serialize};

/// Hard cap on one protocol line.
pub const MAX_LINE_BYTES: usize = 64 * 1024;
/// Hard cap on one command's length.
pub const MAX_COMMAND_BYTES: usize = 8 * 1024;
/// Cap on the stdout/stderr text returned by one exec.
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
pub const MIN_TIMEOUT_MS: u64 = 100;
pub const MAX_TIMEOUT_MS: u64 = 120_000;
pub const DEFAULT_TIMEOUT_MS: u64 = 15_000;

/// One agent request, as it appears on the socket.
#[derive(Deserialize, Serialize, Debug, PartialEq)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

/// The operation tagged by `op` in a [`Request`].
#[derive(Deserialize, Serialize, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Ping,
    ListProfiles,
    /// Open a session to a saved profile. Fires the consent modal.
    Connect {
        profile: String,
    },
    /// Run one command in an open session and capture its output.
    Exec {
        session: String,
        command: String,
        timeout_ms: Option<u64>,
    },
    Close {
        session: String,
    },
}

/// A profile as advertised to agents. No auth material, no key paths.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct ProfileInfo {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: u16,
}

/// The response to one [`Request`]. Only the fields the op fills are
/// serialized, so the line stays small and readable.
#[derive(Serialize, Deserialize, Debug, PartialEq, Default)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profiles: Option<Vec<ProfileInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

impl Response {
    /// An error response carrying nothing but the message.
    pub fn error(id: u64, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            error: Some(message.into()),
            ..Self::default()
        }
    }
}

/// Reject empty, oversized or NUL-containing commands before they reach a
/// session. Anything else is the remote shell's business — the human
/// consented to the session, not to individual commands.
pub fn validate_command(command: &str) -> Result<(), &'static str> {
    if command.is_empty() {
        return Err("empty command");
    }
    if command.len() > MAX_COMMAND_BYTES {
        return Err("command too long");
    }
    if command.contains('\0') {
        return Err("command contains NUL");
    }
    Ok(())
}

/// Clamp a caller-supplied timeout into the sane range.
pub fn clamp_timeout(requested: Option<u64>, default_ms: u64) -> u64 {
    let base = requested.unwrap_or(default_ms);
    base.clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS)
}

/// A fresh opaque session id: `s_` + 12 hex chars.
pub fn new_session_id(salt: u64) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    hasher.write_u64(salt ^ now);
    hasher.write_u64(std::process::id() as u64);
    // A u64 hash prints as up to 16 hex digits; mask to 48 bits so the
    // id is always exactly the 12 characters valid_session_id expects.
    format!("s_{:012x}", hasher.finish() & 0xffff_ffff_ffff)
}

/// Session ids are `s_` followed by exactly 12 hex characters.
pub fn valid_session_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("s_") else {
        return false;
    };
    rest.len() == 12 && rest.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse_for_every_op() {
        let ping: Request = serde_json::from_str(r#"{"id":1,"op":"ping"}"#).unwrap();
        assert_eq!(ping.op, Op::Ping);

        let list: Request = serde_json::from_str(r#"{"id":2,"op":"list_profiles"}"#).unwrap();
        assert_eq!(list.op, Op::ListProfiles);

        let connect: Request =
            serde_json::from_str(r#"{"id":3,"op":"connect","profile":"vps"}"#).unwrap();
        assert_eq!(
            connect.op,
            Op::Connect {
                profile: "vps".to_owned()
            }
        );

        let exec: Request = serde_json::from_str(
            r#"{"id":4,"op":"exec","session":"s_0123456789ab","command":"uptime","timeout_ms":5000}"#,
        )
        .unwrap();
        assert_eq!(
            exec.op,
            Op::Exec {
                session: "s_0123456789ab".to_owned(),
                command: "uptime".to_owned(),
                timeout_ms: Some(5000),
            }
        );

        let close: Request =
            serde_json::from_str(r#"{"id":5,"op":"close","session":"s_0123456789ab"}"#).unwrap();
        assert_eq!(
            close.op,
            Op::Close {
                session: "s_0123456789ab".to_owned()
            }
        );
    }

    #[test]
    fn unknown_ops_are_rejected() {
        assert!(serde_json::from_str::<Request>(r#"{"id":1,"op":"reboot"}"#).is_err());
        // Missing tag entirely.
        assert!(serde_json::from_str::<Request>(r#"{"id":1}"#).is_err());
    }

    #[test]
    fn command_validation_rejects_the_dangerous_and_useless() {
        assert!(validate_command("uptime").is_ok());
        assert!(
            validate_command("echo hi\nwc -l").is_ok(),
            "newlines are fine"
        );
        assert_eq!(validate_command(""), Err("empty command"));
        assert!(validate_command(&"x".repeat(MAX_COMMAND_BYTES + 1)).is_err());
        assert_eq!(validate_command("ls\0rm"), Err("command contains NUL"));
    }

    #[test]
    fn timeouts_clamp_into_the_safe_window() {
        assert_eq!(clamp_timeout(None, DEFAULT_TIMEOUT_MS), DEFAULT_TIMEOUT_MS);
        assert_eq!(clamp_timeout(Some(0), DEFAULT_TIMEOUT_MS), MIN_TIMEOUT_MS);
        assert_eq!(
            clamp_timeout(Some(1_000_000), DEFAULT_TIMEOUT_MS),
            MAX_TIMEOUT_MS
        );
        // A silly configured default is clamped too.
        assert_eq!(clamp_timeout(None, 1), MIN_TIMEOUT_MS);
    }

    #[test]
    fn session_ids_have_a_stable_shape_and_are_unique() {
        let a = new_session_id(1);
        let b = new_session_id(2);
        assert!(valid_session_id(&a), "{a} should be valid");
        assert!(valid_session_id(&b));
        assert_ne!(a, b, "consecutive ids must differ");
    }

    #[test]
    fn session_id_validation_rejects_malformed_ids() {
        assert!(!valid_session_id("0123456789ab"));
        assert!(!valid_session_id("s_"));
        assert!(!valid_session_id("s_0123456789abc"));
        assert!(!valid_session_id("s_0123456789aZ"));
        assert!(!valid_session_id(""));
    }

    #[test]
    fn responses_skip_unfilled_fields() {
        let text = serde_json::to_string(&Response::error(7, "denied")).unwrap();
        assert_eq!(text, r#"{"id":7,"ok":false,"error":"denied"}"#);

        let ok = Response {
            id: 9,
            ok: true,
            session: Some("s_0123456789ab".to_owned()),
            host: Some("vps.example".to_owned()),
            user: Some("deploy".to_owned()),
            ..Response::default()
        };
        let text = serde_json::to_string(&ok).unwrap();
        assert_eq!(
            text,
            r#"{"id":9,"ok":true,"session":"s_0123456789ab","host":"vps.example","user":"deploy"}"#
        );
    }
}
