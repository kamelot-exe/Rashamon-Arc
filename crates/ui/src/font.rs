//! Font rendering — cached glyph rasterizer + layout cache.
//!
//! Two-level cache eliminates repeated font work:
//!
//!   LayoutCache  — Vec keyed by (text, size): per-glyph bb offsets + total
//!                  advance width.  One linear scan over ≤128 short strings
//!                  per draw call.  Cache hit → zero font.layout() calls.
//!
//!   RasterCache  — HashMap<u64,(glyph_id,size)>: pixel coverage bitmap.
//!                  O(1) lookup.  Cache hit → zero glyph.draw() calls.
//!
//! Hot path (warm cache, e.g. unchanged tab title or address bar):
//!   draw_text = 1 Vec scan (layout) + N HashMap lookups (rasters) + blits.
//!   Zero font work.
//!
//! Cold path (first frame / new string):
//!   draw_text = font.layout() + glyph.draw() per new glyph + two inserts.
//!   All subsequent frames are free.

use rashamon_renderer::framebuffer::{Framebuffer, Pixel};
use rusttype::{Font, Scale, point};
use std::cell::RefCell;
use std::collections::HashMap;

// ── Layout cache ──────────────────────────────────────────────────────────────

/// One visible glyph in a pre-computed string layout.
///
/// Layout origin is `(0.0, ascent)`, so:
///   actual_pixel_x = draw_x + bb_x
///   actual_pixel_y = draw_y + bb_y
/// No per-call ascent adjustment needed.
#[derive(Clone)]
struct CachedGlyph {
    glyph_id: u16,
    bb_x:     i32,
    bb_y:     i32,
}

struct LayoutEntry {
    size_bits:   u32,
    text:        Box<str>,
    glyphs:      Vec<CachedGlyph>,
    total_width: u32,
}

/// Vec-based layout cache — accessed once per draw call (not per glyph).
/// Linear scan over ≤128 short strings is fast and allocation-free on hit.
struct LayoutCache(Vec<LayoutEntry>);

impl LayoutCache {
    fn new() -> Self { Self(Vec::with_capacity(64)) }

    fn get(&self, text: &str, size_bits: u32) -> Option<&LayoutEntry> {
        self.0.iter().find(|e| e.size_bits == size_bits && &*e.text == text)
    }

    fn insert(&mut self, entry: LayoutEntry) {
        if self.0.len() >= 128 { self.0.clear(); }
        self.0.push(entry);
    }
}

// ── Glyph raster cache ────────────────────────────────────────────────────────

/// Pre-rasterized coverage bitmap for one glyph at one pixel size.
/// Position-independent: coverage values are relative to the glyph's own bb.
struct GlyphRaster {
    width:    u8,
    height:   u8,
    coverage: Vec<u8>,  // row-major, width × height
}

/// Pack (size_bits, glyph_id) into a single u64 key — no allocation, no collision.
#[inline(always)]
fn raster_key(glyph_id: u16, size_bits: u32) -> u64 {
    ((size_bits as u64) << 16) | (glyph_id as u64)
}

/// HashMap-based raster cache — O(1) per-glyph lookup in the draw hot path.
///
/// Previously a Vec with O(n) linear scan; switching to HashMap reduces the
/// cost of a 50-char string from 50 × 512 comparisons to 50 hash lookups.
struct RasterCache {
    map: HashMap<u64, GlyphRaster>,
}

impl RasterCache {
    fn new() -> Self { Self { map: HashMap::with_capacity(256) } }

    #[inline]
    fn get(&self, glyph_id: u16, size_bits: u32) -> Option<&GlyphRaster> {
        self.map.get(&raster_key(glyph_id, size_bits))
    }

    #[inline]
    fn contains(&self, glyph_id: u16, size_bits: u32) -> bool {
        self.map.contains_key(&raster_key(glyph_id, size_bits))
    }

    fn insert(&mut self, glyph_id: u16, size_bits: u32, raster: GlyphRaster) {
        // UI uses ~95 printable ASCII chars × 4–5 sizes ≈ 400 entries steady-state.
        // Evict-all at 512 means one cold rebuild; cache refills within one frame.
        if self.map.len() >= 512 { self.map.clear(); }
        self.map.insert(raster_key(glyph_id, size_bits), raster);
    }
}

// ── FontManager ───────────────────────────────────────────────────────────────

pub struct FontManager<'a> {
    font:         Font<'a>,
    layout_cache: RefCell<LayoutCache>,
    raster_cache: RefCell<RasterCache>,
}

impl<'a> FontManager<'a> {
    pub fn new(font_data: &'a [u8]) -> std::io::Result<Self> {
        let font = Font::try_from_bytes(font_data)
            .ok_or_else(|| std::io::Error::new(
                std::io::ErrorKind::InvalidData, "failed to load font"))?;
        Ok(Self {
            font,
            layout_cache: RefCell::new(LayoutCache::new()),
            raster_cache: RefCell::new(RasterCache::new()),
        })
    }

    /// Draw `text` at `(x, y)`, clipped to `max_w` pixels.
    ///
    /// Warm cache: 1 Vec scan + N O(1) HashMap lookups + pixel blits.  No font work.
    /// Cold cache: font.layout() + glyph.draw() once; all future calls are warm.
    pub fn draw_text(
        &self,
        fb:    &mut Framebuffer,
        x:     u32,
        y:     u32,
        text:  &str,
        size:  f32,
        color: Pixel,
        max_w: u32,
    ) {
        if text.is_empty() { return; }
        let size_bits = size.to_bits();

        // Populate caches if needed — no-op on warm hit.
        self.ensure_cached(text, size, size_bits);

        // Read-only draw pass.  Both caches borrowed immutably simultaneously.
        let lc     = self.layout_cache.borrow();
        let Some(entry) = lc.get(text, size_bits) else { return };
        let rc     = self.raster_cache.borrow();
        let max_x  = max_w as i32;
        let draw_x = x as i32;
        let draw_y = y as i32;

        for cg in &entry.glyphs {
            if cg.bb_x >= max_x { break; }
            let Some(raster) = rc.get(cg.glyph_id, size_bits) else { continue };
            blit_glyph(fb, draw_x + cg.bb_x, draw_y + cg.bb_y, raster, color);
        }
    }

    /// Pixel advance width of `text` at `size`. Uses the layout cache.
    pub fn text_width(&self, text: &str, size: f32) -> u32 {
        if text.is_empty() { return 0; }
        let size_bits = size.to_bits();
        self.ensure_cached(text, size, size_bits);
        self.layout_cache.borrow()
            .get(text, size_bits)
            .map_or(0, |e| e.total_width)
    }

    /// Flush both caches (call after theme change — font metrics unchanged but
    /// color-independent so this is only needed if the font itself changes).
    #[allow(dead_code)]
    pub fn invalidate_cache(&self) {
        self.layout_cache.borrow_mut().0.clear();
        self.raster_cache.borrow_mut().map.clear();
    }

    // ── Cache population ──────────────────────────────────────────────────────

    /// Build layout + rasters for `(text, size)` if not already cached.
    fn ensure_cached(&self, text: &str, size: f32, size_bits: u32) {
        // Fast path: layout present means rasters were also built then.
        if self.layout_cache.borrow().get(text, size_bits).is_some() { return; }

        let scale     = Scale::uniform(size);
        let v_metrics = self.font.v_metrics(scale);
        // Origin at (0, ascent): cached bb_x/bb_y become direct draw offsets.
        let origin = point(0.0_f32, v_metrics.ascent);

        let mut cached_glyphs = Vec::new();
        let mut total_width   = 0u32;

        for glyph in self.font.layout(text, scale, origin) {
            // Advance width tracked for all glyphs, including invisible (space).
            let adv = (glyph.position().x
                + glyph.unpositioned().h_metrics().advance_width)
                .ceil() as u32;
            if adv > total_width { total_width = adv; }

            let Some(bb) = glyph.pixel_bounding_box() else { continue };

            let glyph_id = glyph.id().0;
            let w = (bb.max.x - bb.min.x) as usize;
            let h = (bb.max.y - bb.min.y) as usize;

            cached_glyphs.push(CachedGlyph {
                glyph_id,
                bb_x: bb.min.x,
                bb_y: bb.min.y,
            });

            // Rasterize only if this glyph×size isn't already in raster cache.
            // No RefCell borrow held here, so borrow_mut() below is safe.
            let need_raster = w > 0 && h > 0
                && !self.raster_cache.borrow().contains(glyph_id, size_bits);

            if need_raster {
                let mut coverage = vec![0u8; w * h];
                glyph.draw(|gx, gy, cov| {
                    let idx = gy as usize * w + gx as usize;
                    if idx < coverage.len() {
                        coverage[idx] = (cov * 255.0 + 0.5) as u8;
                    }
                });
                self.raster_cache.borrow_mut().insert(
                    glyph_id,
                    size_bits,
                    GlyphRaster { width: w as u8, height: h as u8, coverage },
                );
            }
        }

        self.layout_cache.borrow_mut().insert(LayoutEntry {
            size_bits,
            text: text.into(),
            glyphs: cached_glyphs,
            total_width,
        });
    }
}

// ── Glyph blit ────────────────────────────────────────────────────────────────

/// Composite a cached glyph coverage bitmap into `fb` at absolute `(x, y)`.
#[inline]
fn blit_glyph(fb: &mut Framebuffer, x: i32, y: i32, raster: &GlyphRaster, color: Pixel) {
    let w  = raster.width  as i32;
    let h  = raster.height as i32;
    let fw = fb.width  as i32;
    let fh = fb.height as i32;

    for dy in 0..h {
        let py = y + dy;
        if py < 0  { continue; }
        if py >= fh { break; }
        let py = py as u32;

        for dx in 0..w {
            let px = x + dx;
            if px < 0  { continue; }
            if px >= fw { break; }
            let px = px as u32;

            let cov = raster.coverage[(dy * w + dx) as usize];
            if cov < 5 { continue; }

            if cov >= 250 {
                fb.set_pixel(px, py, color);
            } else {
                let a  = cov as u32;
                let ia = 255u32 - a;
                let bg = fb.get_pixel(px, py);
                fb.set_pixel(px, py, Pixel {
                    r: ((color.r as u32 * a + bg.r as u32 * ia) / 255) as u8,
                    g: ((color.g as u32 * a + bg.g as u32 * ia) / 255) as u8,
                    b: ((color.b as u32 * a + bg.b as u32 * ia) / 255) as u8,
                });
            }
        }
    }
}
