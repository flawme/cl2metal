//! The OS-specific bits that are not tied to one feature: the desktop
//! notifier, the application-menu install, and the default font.
//!
//! One file per OS, wired here, so a new platform is a new file plus the two
//! lines below. The `:info` panel keeps its own per-OS readers next to it in
//! `app/sysinfo/`.

#[cfg(unix)]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use linux::*;
#[cfg(windows)]
pub(crate) use windows::*;
