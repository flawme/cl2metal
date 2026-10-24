//! The SFTP browser: a remote directory listing with transfers.
//!
//! The protocol work lives in `term::sftp` on its own thread; this is the view
//! and the small state machine that turns clicks into requests.

use super::*;
use crate::term::sftp::{self, Entry, SftpEvent, SftpRequest};

/// What the panel wants the application to do.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum SftpAction {
    /// Open a remote file in the editor.
    Open(String),
    /// Nothing: the panel handled it.
    None,
}

/// A question the panel is asking.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum SftpPrompt {
    Mkdir(String),
    Rename(String, String),
    Delete(String, bool),
}

/// The SFTP browser.
pub(crate) struct SftpPanel {
    pub(crate) host: String,
    pub(crate) cwd: String,
    pub(crate) entries: Vec<Entry>,
    pub(crate) selected: usize,
    pub(crate) status: Option<String>,
    pub(crate) progress: Option<(String, u64, Option<u64>)>,
    pub(crate) connected: bool,
    pub(crate) open: bool,
    pub(crate) prompt: Option<SftpPrompt>,
    /// The file waiting for a download to finish, so it can be opened.
    pub(crate) open_after_read: Option<String>,
    /// A finished read waiting to be handed to the application.
    pub(crate) pending_open: Option<(String, Vec<u8>)>,
    /// A download in flight: the remote name and where it should land.
    pub(crate) pending_download: Option<(String, std::path::PathBuf)>,
    tx: flume::Sender<SftpRequest>,
    rx: flume::Receiver<SftpEvent>,
}

impl SftpPanel {
    /// Connect to a host and list its home directory.
    pub(crate) fn connect(params: SshParams, wake: Waker) -> Self {
        let (tx, rx) = flume::unbounded();
        let (req_tx, req_rx) = flume::unbounded();
        let host = format!("{}@{}:{}", params.user, params.host, params.port);
        sftp::spawn(params, wake, tx, req_rx);
        Self {
            host,
            cwd: "/".to_owned(),
            entries: Vec::new(),
            selected: 0,
            status: Some("connecting…".to_owned()),
            progress: None,
            connected: false,
            open: true,
            prompt: None,
            open_after_read: None,
            pending_open: None,
            pending_download: None,
            tx: req_tx,
            rx,
        }
    }

    /// Send a request to the worker.
    pub(crate) fn request(&self, request: SftpRequest) {
        let _ = self.tx.send(request);
    }

    /// Drain the worker's events.
    pub(crate) fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                SftpEvent::Connected { home } => {
                    self.connected = true;
                    self.cwd = home.clone();
                    self.status = Some(format!("connected to {}", self.host));
                    self.request(SftpRequest::List(home));
                }
                SftpEvent::Listed { path, mut entries } => {
                    sftp::sort_entries(&mut entries);
                    self.entries = entries;
                    self.cwd = path;
                    self.selected = 0;
                    self.status = None;
                }
                SftpEvent::Read { path, data } => {
                    self.progress = None;
                    if let Some((name, dir)) = self.pending_download.take() {
                        let target = dir.join(&name);
                        match std::fs::write(&target, &data) {
                            Ok(()) => {
                                self.status = Some(format!("saved {}", target.display()));
                            }
                            Err(e) => {
                                self.status =
                                    Some(format!("cannot write {}: {e}", target.display()));
                            }
                        }
                    } else if self.open_after_read.take().as_deref() == Some(path.as_str()) {
                        // Handed to the application, which owns the editor.
                        self.status = Some(format!("opened {path}"));
                        self.pending_open = Some((path, data));
                    } else {
                        self.status = Some(format!("read {} bytes from {path}", data.len()));
                    }
                }
                SftpEvent::Written { path, bytes } => {
                    self.progress = None;
                    self.status = Some(format!("wrote {bytes} bytes to {path}"));
                    self.request(SftpRequest::List(self.cwd.clone()));
                }
                SftpEvent::Progress { path, done, total } => {
                    self.progress = Some((path, done, total));
                }
                SftpEvent::Error { context, message } => {
                    self.progress = None;
                    self.status = Some(format!("{context}: {message}"));
                }
                SftpEvent::Disconnected => {
                    self.connected = false;
                    self.status = Some("disconnected".to_owned());
                }
                SftpEvent::Stat { .. } => {}
            }
        }
    }

    /// The entry the selection is on.
    pub(crate) fn current(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// Move the selection.
    pub(crate) fn move_selection(&mut self, delta: i32) {
        if self.entries.is_empty() {
            return;
        }
        let next = self.selected as i32 + delta;
        self.selected = next.clamp(0, self.entries.len() as i32 - 1) as usize;
    }

    /// Enter the selected directory, or open the selected file.
    pub(crate) fn activate(&mut self) -> SftpAction {
        let Some(entry) = self.current().cloned() else {
            return SftpAction::None;
        };
        if entry.dir {
            self.request(SftpRequest::List(entry.path));
            SftpAction::None
        } else {
            self.open_after_read = Some(entry.path.clone());
            self.status = Some(format!("reading {}…", entry.name));
            self.request(SftpRequest::Read(entry.path));
            SftpAction::None
        }
    }

    /// Go up one directory.
    pub(crate) fn go_up(&mut self) {
        let parent = sftp::parent_remote(&self.cwd);
        if parent != self.cwd {
            self.request(SftpRequest::List(parent));
        }
    }

    /// Download the selected file to `dir`.
    pub(crate) fn download(&mut self, dir: &std::path::Path) {
        let Some(entry) = self.current().cloned() else {
            return;
        };
        if entry.dir {
            self.status = Some("only files can be downloaded".to_owned());
            return;
        }
        self.open_after_read = None;
        self.status = Some(format!("downloading {}…", entry.name));
        self.request(SftpRequest::Read(entry.path));
        self.pending_download = Some((entry.name, dir.to_path_buf()));
    }

    /// Upload a local file into the current directory.
    pub(crate) fn upload(&mut self, path: &std::path::Path) {
        let Ok(data) = std::fs::read(path) else {
            self.status = Some(format!("cannot read {}", path.display()));
            return;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "upload".to_owned());
        let target = sftp::join_remote(&self.cwd, &name);
        self.status = Some(format!("uploading {name}…"));
        self.request(SftpRequest::Write { path: target, data });
    }

    /// Cancel the transfer in flight.
    pub(crate) fn cancel(&mut self) {
        self.request(SftpRequest::Cancel);
        self.progress = None;
        self.status = Some("cancelled".to_owned());
    }

    /// The panel itself. `panel_alpha` is the strength the large side surfaces
    /// are drawn at, so the wallpaper reads through this one the same way it
    /// does through the SSH sidebar and the editor's file tree.
    pub(crate) fn draw(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        theme: &Theme,
        panel_alpha: f32,
    ) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            0.0,
            ui::with_alpha(color32(theme.ui.panel_bg), panel_alpha),
        );
        painter.line_segment(
            [
                egui::pos2(rect.min.x, rect.min.y),
                egui::pos2(rect.max.x, rect.min.y),
            ],
            egui::Stroke::new(1.0, color32(theme.ui.border)),
        );
        let font = egui::FontId::new(11.5, egui::FontFamily::Monospace);
        let row_h = 20.0;
        let mut y = rect.min.y + 6.0;
        let left = rect.min.x + 10.0;

        // Header: host, path, and the buttons.
        painter.text(
            egui::pos2(left, y + 6.0),
            egui::Align2::LEFT_CENTER,
            format!("SFTP  {}", self.host),
            font.clone(),
            color32(theme.ui.accent),
        );
        let mut x = rect.max.x - 8.0;
        for (label, width) in [("×", 22.0), ("⟳", 22.0), ("↑", 22.0), ("+", 22.0)] {
            let button =
                egui::Rect::from_min_size(egui::pos2(x - width, y), egui::vec2(width, 20.0));
            if ui
                .interact(
                    button,
                    ui.id().with(("sftp-btn", label)),
                    egui::Sense::click(),
                )
                .clicked()
            {
                match label {
                    "×" => self.open = false,
                    "⟳" => self.request(SftpRequest::List(self.cwd.clone())),
                    "↑" => self.go_up(),
                    _ => self.prompt = Some(SftpPrompt::Mkdir(String::new())),
                }
            }
            painter.text(
                button.center(),
                egui::Align2::CENTER_CENTER,
                label,
                font.clone(),
                color32(theme.ui.text_dim),
            );
            x -= width + 2.0;
        }
        y += 22.0;
        painter.text(
            egui::pos2(left, y + 6.0),
            egui::Align2::LEFT_CENTER,
            &self.cwd,
            font.clone(),
            color32(theme.ui.text_dim),
        );
        y += 22.0;

        // The listing.
        let list = egui::Rect::from_min_max(
            egui::pos2(rect.min.x, y),
            egui::pos2(rect.max.x, rect.max.y - 26.0),
        );
        let visible = (list.height() / row_h).floor().max(1.0) as usize;
        let top = self.selected.saturating_sub(visible.saturating_sub(1));
        let mut activated = false;
        for (i, entry) in self.entries.iter().enumerate().skip(top).take(visible) {
            let row = egui::Rect::from_min_size(
                egui::pos2(list.min.x + 4.0, list.min.y + (i - top) as f32 * row_h),
                egui::vec2(list.width() - 8.0, row_h),
            );
            let response = ui.interact(row, ui.id().with(("sftp-row", i)), egui::Sense::click());
            if i == self.selected {
                painter.rect_filled(row, 3.0, color32(theme.bg));
            } else if response.hovered() {
                painter.rect_filled(row, 3.0, color32(theme.bg).gamma_multiply(0.6));
            }
            if response.clicked() {
                self.selected = i;
            }
            if response.double_clicked() {
                self.selected = i;
                activated = true;
            }
            painter.text(
                egui::pos2(row.min.x + 6.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                sftp::format_mode(entry.mode),
                font.clone(),
                color32(theme.ui.text_dim),
            );
            painter.text(
                egui::pos2(row.min.x + 92.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                if entry.dir {
                    format!("{}/", entry.name)
                } else {
                    entry.name.clone()
                },
                font.clone(),
                if entry.dir {
                    color32(theme.ui.accent)
                } else {
                    color32(theme.ui.text)
                },
            );
            if !entry.dir {
                painter.text(
                    egui::pos2(row.max.x - 6.0, row.center().y),
                    egui::Align2::RIGHT_CENTER,
                    human_size(entry.size),
                    font.clone(),
                    color32(theme.ui.text_dim),
                );
            }
        }

        if activated {
            self.activate();
        }

        // Status and progress.
        let footer = egui::Rect::from_min_size(
            egui::pos2(left, rect.max.y - 20.0),
            egui::vec2(rect.width() - 20.0, 16.0),
        );
        if let Some((path, done, total)) = &self.progress {
            let name = path.rsplit('/').next().unwrap_or(path);
            let text = match total {
                Some(total) if *total > 0 => format!(
                    "{name}  {} / {}  ({}%)",
                    human_size(*done),
                    human_size(*total),
                    done * 100 / total.max(&1)
                ),
                _ => format!("{name}  {}", human_size(*done)),
            };
            painter.text(
                footer.min,
                egui::Align2::LEFT_CENTER,
                text,
                font.clone(),
                color32(theme.ui.accent),
            );
        } else if let Some(status) = &self.status {
            painter.text(
                footer.min,
                egui::Align2::LEFT_CENTER,
                status,
                font.clone(),
                color32(theme.ui.text_dim),
            );
        }
    }
}

/// A short human-readable size.
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
