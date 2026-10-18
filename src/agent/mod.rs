//! Agent access API: brokers SSH sessions to local coding agents over a
//! user-owned unix socket, with human consent for every session and an
//! append-only audit log.
//!
//! The invariant this module owns: credentials never leave the process.
//! Agents get profile names and opaque session ids; the keyring stays
//! behind the consent modal.

pub mod audit;
pub mod guard;
#[cfg(unix)]
pub mod mcp;
pub mod protocol;
#[cfg(unix)]
pub mod register;
pub mod server;
pub mod session;
