//! Font loading and glyph rasterization into a shared atlas (R8 coverage).

use ab_glyph::{Font, FontArc, FontVec, GlyphId, PxScale, ScaleFont};
use fontdb::{Database, Family, Query, Stretch, Style, Weight};
use std::collections::HashMap;

pub const STYLE_REGULAR: u8 = 0;
pub const STYLE_BOLD: u8 = 1;
pub const STYLE_ITALIC: u8 = 2;
pub const STYLE_BOLD_ITALIC: u8 = 3;

#[derive(Clone, Debug)]
pub struct GlyphEntry {
    /// Bearing offset from the cell's top, in pixels.
    pub top: f32,
    pub w: f32,
    pub h: f32,
    /// UV rectangle in the atlas (min_x, min_y, max_x, max_y).
    pub uv: [f32; 4],
}

const ATLAS_SIZE: u32 = 2048;
const MARGIN: u32 = 1;

pub struct GlyphAtlas {
    fonts: [Option<FontArc>; 4],
    /// Rasterization scale in physical pixels.
    pub size_px: u16,
    cache: HashMap<(u8, char, u16), GlyphEntry>,
    /// Synthetic cell-sized entries (the rounded-corner arcs), by cell size.
    synth_cache: HashMap<(char, u16, u16), GlyphEntry>,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    pack_x: u32,
    pack_y: u32,
    row_h: u32,
    /// Bumped whenever the texture must be recreated at a new size.
    pub version: u64,
    /// Rects that need re-upload (relative to the current texture).
    pub dirty: Vec<[u16; 4]>,
    /// Union bounds of dirty rects (in pixels) for cheap full uploads.
    metrics: CellMetrics,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CellMetrics {
    pub cell_w: f32,
    pub cell_h: f32,
}

fn load_face(db: &Database, family: &str, weight: Weight, style: Style) -> Option<FontArc> {
    let mut best: Option<(&fontdb::FaceInfo, i32)> = None;
    for face in db.faces() {
        if !face
            .families
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(family))
        {
            continue;
        }
        let weight_dist = (face.weight.0 as i32 - weight.0 as i32).abs();
        let style_dist = if face.style == style { 0 } else { 10_000 };
        let score = weight_dist + style_dist;
        if best.map(|(_, s)| score < s).unwrap_or(true) {
            best = Some((face, score));
        }
    }
    let (face, _) = best?;
    let font: FontVec = db.with_face_data(face.id, |data, index| {
        FontVec::try_from_vec_and_index(data.to_vec(), index).ok()
    })??;
    Some(FontArc::new(font))
}

/// Find some monospaced font when the configured family doesn't exist.
fn fallback_family(db: &Database) -> String {
    const CANDIDATES: [&str; 6] = [
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Noto Sans Mono",
        "Ubuntu Mono",
        "Menlo",
        "Consolas",
    ];
    for cand in CANDIDATES {
        if db
            .faces()
            .any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(cand)))
        {
            return cand.to_owned();
        }
    }
    db.faces()
        .find(|f| f.monospaced)
        .and_then(|f| f.families.first().map(|(n, _)| n.clone()))
        .unwrap_or_else(|| "sans-serif".to_owned())
}

impl GlyphAtlas {
    pub fn new(db: &Database, family: &str, size_px: u16) -> Self {
        let family = if db.faces().any(|f| {
            f.families
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case(family))
        }) {
            family.to_owned()
        } else {
            log::warn!("font '{family}' not found, using fallback");
            fallback_family(db)
        };
        log::info!("using font family: {family}");

        let regular = load_face(db, &family, Weight::NORMAL, Style::Normal);
        let bold = load_face(db, &family, Weight::BOLD, Style::Normal).or_else(|| regular.clone());
        let italic =
            load_face(db, &family, Weight::NORMAL, Style::Italic).or_else(|| regular.clone());
        let bold_italic =
            load_face(db, &family, Weight::BOLD, Style::Italic).or_else(|| bold.clone());

        let fonts = [regular, bold, italic, bold_italic];
        let mut atlas = Self {
            fonts,
            size_px,
            cache: HashMap::new(),
            synth_cache: HashMap::new(),
            pixels: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            width: ATLAS_SIZE,
            height: ATLAS_SIZE,
            pack_x: MARGIN,
            pack_y: MARGIN,
            row_h: 0,
            version: 1,
            dirty: Vec::new(),
            metrics: CellMetrics::default(),
        };
        atlas.metrics = atlas.compute_metrics();
        atlas
    }

    /// Primary font reference (for cell metrics).
    fn primary(&self) -> Option<&FontArc> {
        self.fonts[STYLE_REGULAR as usize].as_ref()
    }

    fn compute_metrics(&self) -> CellMetrics {
        let Some(font) = self.primary() else {
            return CellMetrics {
                cell_w: 8.0,
                cell_h: 16.0,
            };
        };
        let scale = PxScale::from(self.size_px as f32);
        let scaled = font.as_scaled(scale);
        let cell_w = scaled.h_advance(font.glyph_id('M')).round().max(1.0);
        let ascent = scaled.ascent();
        let descent = scaled.descent(); // negative
        let cell_h = (ascent - descent).round().max(1.0);
        CellMetrics { cell_w, cell_h }
    }

    pub fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    /// Set the rasterization size (physical pixels); invalidates the cache.
    pub fn set_size(&mut self, size_px: u16) {
        if self.size_px == size_px {
            return;
        }
        self.size_px = size_px;
        self.cache.clear();
        self.reset_pack();
        self.metrics = self.compute_metrics();
        self.version += 1;
        self.dirty
            .push([0, 0, self.width as u16, self.height as u16]);
    }

    fn reset_pack(&mut self) {
        self.pixels.fill(0);
        self.pack_x = MARGIN;
        self.pack_y = MARGIN;
        self.row_h = 0;
    }

    fn slot(&self, bold: bool, italic: bool) -> u8 {
        match (bold, italic) {
            (false, false) => STYLE_REGULAR,
            (true, false) => STYLE_BOLD,
            (false, true) => STYLE_ITALIC,
            (true, true) => STYLE_BOLD_ITALIC,
        }
    }

    /// Get (or rasterize) a glyph. Returns `None` when the glyph is blank/missing.
    pub fn glyph(&mut self, c: char, bold: bool, italic: bool) -> Option<GlyphEntry> {
        if c == ' ' || c == '\t' {
            return None;
        }
        let slot = self.slot(bold, italic);
        let px = self.size_px;
        let key = (slot, c, px);
        if let Some(entry) = self.cache.get(&key) {
            return Some(entry.clone());
        }

        let font = self.fonts[slot as usize].as_ref()?;
        let gid = font.glyph_id(c);
        if gid.0 == 0 && c != '\u{fffd}' {
            // Missing glyph: try the regular style as a last resort before giving up.
            if slot != STYLE_REGULAR {
                let regular = self.fonts[STYLE_REGULAR as usize].as_ref()?;
                let gid = regular.glyph_id(c);
                if gid.0 != 0 {
                    return self.rasterize(STYLE_REGULAR, gid, px, key);
                }
            }
            return None;
        }
        self.rasterize(slot, gid, px, key)
    }

    fn rasterize(
        &mut self,
        slot: u8,
        gid: GlyphId,
        px: u16,
        key: (u8, char, u16),
    ) -> Option<GlyphEntry> {
        let font = self.fonts[slot as usize].as_ref()?;
        let scale = PxScale::from(px as f32);
        let scaled = font.as_scaled(scale);
        let ascent = scaled.ascent();

        let glyph = gid.with_scale_and_position(scale, ab_glyph::point(0.0, ascent));
        let outlined = font.outline_glyph(glyph)?;
        let bounds = outlined.px_bounds();
        let w = bounds.width().ceil() as i64;
        let h = bounds.height().ceil() as i64;
        if w <= 0 || h <= 0 {
            // Blank glyph (e.g. zero-width): cache as empty.
            let entry = GlyphEntry {
                top: 0.0,
                w: 0.0,
                h: 0.0,
                uv: [0.0; 4],
            };
            self.cache.insert(key, entry.clone());
            return Some(entry);
        }

        let (uw, uh) = (w as u32, h as u32);
        let pos = self.pack(uw, uh)?;

        // Blit coverage.
        let (x0, y0) = (pos.0 as i64, pos.1 as i64);
        outlined.draw(|dx, dy, coverage| {
            let x = x0 + dx as i64;
            let y = y0 + dy as i64;
            if x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height {
                let idx = (y as u32 * self.width + x as u32) as usize;
                let v = (coverage * 255.0) as u8;
                if v > self.pixels[idx] {
                    self.pixels[idx] = v;
                }
            }
        });

        self.dirty
            .push([pos.0 as u16, pos.1 as u16, uw as u16, uh as u16]);

        let entry = GlyphEntry {
            top: bounds.min.y,
            w: uw as f32,
            h: uh as f32,
            uv: [
                pos.0 as f32 / self.width as f32,
                pos.1 as f32 / self.height as f32,
                (pos.0 + uw) as f32 / self.width as f32,
                (pos.1 + uh) as f32 / self.height as f32,
            ],
        };
        self.cache.insert(key, entry.clone());
        Some(entry)
    }

    /// Rasterize a synthetic, cell-sized glyph — the anti-aliased arc of a
    /// rounded box-drawing corner — into the atlas. Unlike font glyphs these
    /// blit cell-aligned (`top` is 0 and the entry spans the whole cell), so
    /// the arc meets the corner's straight arms exactly. Cached per size.
    pub fn synthetic(&mut self, c: char, cw: f32, ch: f32) -> Option<GlyphEntry> {
        let key = (c, cw as u16, ch as u16);
        if let Some(e) = self.synth_cache.get(&key) {
            return Some(e.clone());
        }
        let (uw, uh) = (cw.ceil() as u32, ch.ceil() as u32);
        if uw == 0 || uh == 0 {
            return None;
        }
        let pos = self.pack(uw, uh)?;
        let (x0, y0) = (pos.0, pos.1);
        super::boxes::paint_arc(c, cw, ch, &mut |dx: u32, dy: u32, v: u8| {
            let idx = ((y0 + dy) * self.width + x0 + dx) as usize;
            if v > self.pixels[idx] {
                self.pixels[idx] = v;
            }
        });
        self.dirty.push([pos.0 as u16, pos.1 as u16, uw as u16, uh as u16]);
        let entry = GlyphEntry {
            top: 0.0,
            w: uw as f32,
            h: uh as f32,
            uv: [
                pos.0 as f32 / self.width as f32,
                pos.1 as f32 / self.height as f32,
                (pos.0 + uw) as f32 / self.width as f32,
                (pos.1 + uh) as f32 / self.height as f32,
            ],
        };
        self.synth_cache.insert(key, entry.clone());
        Some(entry)
    }

    /// Shelf-pack a `w x h` box; grows the atlas when full.
    fn pack(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w + 2 * MARGIN > self.width || h + 2 * MARGIN > self.height {
            return None; // Glyph larger than the whole atlas.
        }
        if self.pack_x + w + MARGIN > self.width {
            self.pack_y += self.row_h + MARGIN;
            self.pack_x = MARGIN;
            self.row_h = 0;
        }
        if self.pack_y + h + MARGIN > self.height {
            self.grow();
            return self.pack(w, h);
        }
        let pos = (self.pack_x, self.pack_y);
        self.pack_x += w + MARGIN;
        self.row_h = self.row_h.max(h);
        Some(pos)
    }

    fn grow(&mut self) {
        // Double the height; re-rasterization is driven by the cleared cache.
        let new_height = self.height * 2;
        if new_height > 8192 {
            log::error!("glyph atlas exhausted");
            return;
        }
        log::info!("growing glyph atlas to {}x{}", self.width, new_height);
        self.height = new_height;
        self.pixels = vec![0; (self.width * self.height) as usize];
        self.cache.clear();
        self.synth_cache.clear();
        self.pack_x = MARGIN;
        self.pack_y = MARGIN;
        self.row_h = 0;
        self.version += 1;
        self.dirty
            .push([0, 0, self.width as u16, self.height as u16]);
    }

    /// Take the CPU-side pixels + version for uploading.
    pub fn upload_data(&mut self) -> (&Vec<u8>, u32, u32, u64, Vec<[u16; 4]>) {
        let dirty = std::mem::take(&mut self.dirty);
        (&self.pixels, self.width, self.height, self.version, dirty)
    }
}

/// Build a font database with all system fonts.
pub fn load_font_db() -> Database {
    let mut db = Database::new();
    db.load_system_fonts();
    db
}

/// Query helper used by the settings UI to list monospaced families.
pub fn monospace_families(db: &Database) -> Vec<String> {
    let mut names: Vec<String> = db
        .faces()
        .filter(|f| f.monospaced)
        .flat_map(|f| f.families.iter().map(|(n, _)| n.clone()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Used by the settings UI: does fontdb know this query?
#[allow(dead_code)]
pub fn query_exists(db: &Database, family: &str) -> bool {
    db.query(&Query {
        families: &[Family::Name(family)],
        weight: Weight::NORMAL,
        stretch: Stretch::Normal,
        style: Style::Normal,
    })
    .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty font database still yields a working atlas: the synthetic
    /// path rasterizes geometry, not font outlines.
    #[test]
    fn synthetic_arcs_blit_cell_aligned_and_cache() {
        let db = Database::new();
        let mut atlas = GlyphAtlas::new(&db, "none", 18);

        let e = atlas.synthetic('\u{256D}', 9.0, 18.0).expect("the arc rasterizes");
        assert_eq!(e.top, 0.0, "blits at the cell's top edge");
        assert_eq!((e.w, e.h), (9.0, 18.0), "spans the whole cell");
        assert!(!atlas.dirty.is_empty(), "the atlas got pixels to upload");

        // The second call is served from the cache: same entry, no new work.
        let dirty_after_first = atlas.dirty.len();
        let e2 = atlas.synthetic('\u{256D}', 9.0, 18.0).expect("cached");
        assert_eq!(e2.uv, e.uv);
        assert_eq!(atlas.dirty.len(), dirty_after_first);
    }
}
