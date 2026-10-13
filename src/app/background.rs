//! The background image: decoding it off the main thread, uploading it,
//! and the alpha values the chrome and terminal are drawn with.

use super::*;

impl App {
    pub(crate) fn sync_background(&mut self, ctx: &egui::Context) {
        let want = self
            .config
            .background_image
            .clone()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let current = self
            .background
            .as_ref()
            .map(|b| b.path.clone())
            .unwrap_or_default();
        if want == current || self.background_loading.as_deref() == Some(want.as_str()) {
            return;
        }
        if want.is_empty() {
            log::info!("background image cleared");
            self.background = None;
            self.background_loading = None;
            self.background_rx = None;
            self.background_at = None;
            return;
        }
        let path = expand_tilde(&want);
        if !std::path::Path::new(&path).is_file() {
            log::debug!("background image is not a file (yet): {path}");
            return;
        }
        // Decoding and downscaling a large wallpaper takes seconds in a debug
        // build. Doing it here would stall the first frame — and with it the
        // startup animation — behind a blank window, so it runs on a worker
        // thread and the texture is uploaded when it lands.
        let max_side = ctx
            .input(|i| i.max_texture_side)
            .min(MAX_BACKGROUND_PX as usize) as u32;
        let (tx, rx) = flume::bounded(1);
        let thread_path = path.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let result = load_image(&thread_path, max_side);
            log::info!(
                "background image decoded in {:.2}s: {thread_path}",
                started.elapsed().as_secs_f32()
            );
            let _ = tx.send(result);
        });
        log::info!("background image loading: {path}");
        self.background_loading = Some(want);
        self.background_rx = Some(rx);
    }

    /// Pick up a finished background-image load and upload it as a texture.
    pub(crate) fn poll_background(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.background_rx else {
            return;
        };
        let Ok(result) = rx.try_recv() else {
            return;
        };
        self.background_rx = None;
        let path = self.background_loading.take().unwrap_or_default();
        // The user may have changed or cleared the path while it loaded.
        let want = self
            .config
            .background_image
            .clone()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if want != path {
            log::debug!("dropping stale background load: {path}");
            return;
        }
        match result {
            Ok((rgba, w, h)) => {
                let image =
                    egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                let texture =
                    ctx.load_texture("cl2metal-background", image, egui::TextureOptions::LINEAR);
                log::info!("background image ready: {path} ({w}x{h})");
                self.background = Some(ImageGpu {
                    path,
                    size: [w, h],
                    texture,
                });
                self.background_at = Some(self.now);
            }
            Err(e) => log::warn!("background image {path}: {e:#}"),
        }
        self.refresh_ui_alpha(ctx);
    }

    /// Apply a config to the running app.
    ///
    /// Used for live preview while the settings modal is open, for Save, and
    /// for Cancel (with the snapshot taken when the modal opened).
    pub(crate) fn bg_alpha(&self) -> f32 {
        if self.background.is_some() {
            bg_alpha_for(self.config.background_image_opacity)
        } else {
            1.0
        }
    }

    /// Opacity of the chrome (tab bar, panels, modals). It stays readable even
    /// with the image at full strength.
    pub(crate) fn ui_alpha(&self) -> f32 {
        if self.background.is_some() {
            ui_alpha_for(self.config.background_image_opacity)
        } else {
            1.0
        }
    }

    /// Opacity of the large side surfaces (the SSH sidebar, the editor's file
    /// tree and status line, the SFTP browser).
    pub(crate) fn panel_alpha(&self) -> f32 {
        if self.background.is_some() {
            panel_alpha_for(self.config.background_image_opacity)
        } else {
            1.0
        }
    }

    /// Push the current chrome opacity into the theme and egui's visuals.
    pub(crate) fn refresh_ui_alpha(&mut self, ctx: &egui::Context) {
        let alpha = self.ui_alpha();
        if (alpha - self.theme.ui_alpha).abs() > f32::EPSILON {
            self.theme.ui_alpha = alpha;
            ui::apply_theme(ctx, &self.theme);
        }
    }
}

/// Decode an image file into RGBA8 pixels, downscaled to fit `max_side`.
///
/// egui refuses textures larger than the GPU's maximum texture side, so an
/// oversized wallpaper is scaled down instead of panicking. The result is
/// cached next to the session logs: decoding a large JPEG takes seconds in a
/// debug build, and the cache turns every later launch into a file read.
pub(crate) fn load_image(path: &str, max_side: u32) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    use anyhow::Context;
    let max_side = max_side.max(64);
    let cache = cache_path(path, max_side);
    if let Some(cache) = &cache
        && let Ok((rgba, w, h)) = read_cache(cache)
    {
        log::info!("background image from cache: {path} ({w}x{h})");
        return Ok((rgba, w, h));
    }

    let t0 = std::time::Instant::now();
    let img = image::open(path).with_context(|| format!("opening {path}"))?;
    let mut rgba = img.to_rgba8();
    let decoded = t0.elapsed().as_secs_f32();
    let (w, h) = (rgba.width(), rgba.height());
    let longest = w.max(h);
    if longest > max_side {
        let scale = max_side as f32 / longest as f32;
        let nw = ((w as f32 * scale).round() as u32).max(1);
        let nh = ((h as f32 * scale).round() as u32).max(1);
        // `thumbnail` is a fast box filter: a debug build takes seconds on the
        // triangle filter, and this runs while the terminal is already up.
        rgba = image::imageops::thumbnail(&rgba, nw, nh);
        log::info!(
            "background image downscaled {w}x{h} -> {nw}x{nh} (decode {decoded:.2}s, resize {:.2}s)",
            t0.elapsed().as_secs_f32() - decoded
        );
    } else {
        log::info!("background image decoded in {decoded:.2}s ({w}x{h})");
    }
    let (w, h) = (rgba.width(), rgba.height());
    let pixels = rgba.into_raw();
    if let Some(cache) = &cache {
        write_cache(cache, w, h, &pixels);
    }
    Ok((pixels, w, h))
}

/// Cache file for a wallpaper, keyed by its path, size, mtime and target size.
fn cache_path(path: &str, max_side: u32) -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    max_side.hash(&mut hasher);
    if let Ok(meta) = std::fs::metadata(path) {
        meta.len().hash(&mut hasher);
        if let Ok(modified) = meta.modified()
            && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            since.as_secs().hash(&mut hasher);
        }
    }
    let dir = crate::config::log_dir();
    let dir = dir.parent().unwrap_or(&dir);
    Some(dir.join(format!("background-{:016x}.rgba", hasher.finish())))
}

/// Read a cached decode: `CL2B` + width + height + RGBA8 pixels.
pub(crate) fn read_cache(path: &std::path::Path) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let data = std::fs::read(path)?;
    anyhow::ensure!(data.len() > 12 && &data[..4] == b"CL2B", "bad cache header");
    let w = u32::from_le_bytes(data[4..8].try_into()?);
    let h = u32::from_le_bytes(data[8..12].try_into()?);
    let pixels = data[12..].to_vec();
    anyhow::ensure!(
        pixels.len() == w as usize * h as usize * 4,
        "cache size mismatch"
    );
    Ok((pixels, w, h))
}

/// Write a decode to the cache, dropping the cache of a previous wallpaper.
pub(crate) fn write_cache(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    let mut out = Vec::with_capacity(12 + rgba.len());
    out.extend_from_slice(b"CL2B");
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(rgba);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let old = entry.path();
                if old != path && is_cache_file(&old) {
                    let _ = std::fs::remove_file(old);
                }
            }
        }
    }
    if let Err(e) = std::fs::write(path, out) {
        log::debug!("background cache write failed: {e}");
    }
}

/// True for `background-*.rgba` cache files.
fn is_cache_file(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("background-") && n.ends_with(".rgba"))
}

/// Opacity of the terminal cell backgrounds for a given image strength.
/// Wallpaper is an ambient layer, not a text background, so the terminal keeps
/// a substantial surface even at 100% image strength.
pub(crate) fn bg_alpha_for(image_opacity: f32) -> f32 {
    (1.0 - image_opacity * 0.60).clamp(0.40, 1.0)
}

/// Opacity of the chrome for a given image strength. It never drops below a
/// floor, so the tab bar and modals stay readable over any wallpaper.
pub(crate) fn ui_alpha_for(image_opacity: f32) -> f32 {
    (1.0 - image_opacity * 0.18).clamp(0.78, 1.0)
}

/// Opacity of the large side surfaces: the SSH sidebar, the editor's file tree
/// and status line, the SFTP browser.
///
/// A wash, not a wall. The application paints one shared backdrop for the whole
/// window and it already carries the surface tint, so a panel only has to
/// separate itself from the text area — its border and its own text colours do
/// the rest. Anything heavier hides the wallpaper behind a 240 px column, which
/// is what made the image look like it vanished when SSH mode or the editor
/// came up. The wash thins as the image strengthens, so a bolder wallpaper
/// keeps more of itself.
pub(crate) fn panel_alpha_for(image_opacity: f32) -> f32 {
    (0.30 * (1.0 - image_opacity)).clamp(0.10, 0.30)
}

/// True when the path looks like an image the decoder can handle.
pub(crate) fn is_image_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "bmp")
    )
}
