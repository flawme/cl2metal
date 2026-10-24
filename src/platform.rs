//! The OS-specific bits that are not tied to one feature: the desktop
//! notifier, the application-menu install, and the default font.
//!
//! One file per OS, wired here, so a new platform is a new file plus the two
//! lines below. The `:info` panel keeps its own per-OS readers next to it in
//! `app/sysinfo/`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub(crate) use linux::*;
#[cfg(target_os = "macos")]
pub(crate) use macos::*;
#[cfg(windows)]
pub(crate) use windows::*;
