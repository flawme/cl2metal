//! Clipboard access: the system clipboard and the X11 primary selection.

use super::*;

impl App {
    pub(crate) fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => self.clipboard = Some(c),
                Err(e) => {
                    log::warn!("clipboard unavailable: {e}");
                    return None;
                }
            }
        }
        self.clipboard.as_mut()
    }

    /// Read the system clipboard.
    pub(crate) fn read_clipboard(&mut self) -> Option<String> {
        let cb = self.clipboard()?;
        cb.get_text().ok().filter(|t| !t.is_empty())
    }

    /// Read the X11 primary selection, falling back to the clipboard.
    pub(crate) fn read_primary(&mut self) -> Option<String> {
        let cb = self.clipboard()?;
        #[cfg(target_os = "linux")]
        {
            use arboard::{GetExtLinux, LinuxClipboardKind};
            if let Ok(text) = cb.get().clipboard(LinuxClipboardKind::Primary).text()
                && !text.is_empty()
            {
                return Some(text);
            }
        }
        cb.get_text().ok().filter(|t| !t.is_empty())
    }

    /// Publish a selection to the X11 primary selection (copy-on-select).
    pub(crate) fn set_primary(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let Some(cb) = self.clipboard() else {
            return;
        };
        #[cfg(target_os = "linux")]
        {
            use arboard::{LinuxClipboardKind, SetExtLinux};
            let _ = cb
                .set()
                .clipboard(LinuxClipboardKind::Primary)
                .text(text.to_owned());
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = cb.set_text(text.to_owned());
        }
    }
}
