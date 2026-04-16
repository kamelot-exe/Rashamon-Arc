//! Font rendering with subpixel anti-aliasing via alpha blending.

use rashamon_renderer::framebuffer::{Framebuffer, Pixel};
use rusttype::{Font, Scale, point};
use std::cell::RefCell;

// ── Width cache ───────────────────────────────────────────────────────────────

struct WidthEntry {
    size_bits: u32,
    text:      Box<str>,
    width:     u32,
}

/// Small Vec-based cache for `text_width`.
///
/// UI renders a small fixed vocabulary of strings (tab titles, address bar
/// text, static labels). Linear scan over ≤128 entries is faster than a
/// HashMap for this working set and avoids key allocation on cache hits.
struct WidthCache(Vec<WidthEntry>);

impl WidthCache {
    fn new() -> Self { Self(Vec::with_capacity(64)) }

    fn get(&self, text: &str, size_bits: u32) -> Option<u32> {
        for e in &self.0 {
            if e.size_bits == size_bits && &*e.text == text {
                return Some(e.width);
            }
        }
        None
    }

    fn insert(&mut self, text: &str, size_bits: u32, width: u32) {
        // Hard cap: evict everything when full. The working set is small and
        // stable (UI labels + current URL), so eviction is rare in practice.
        if self.0.len() >= 128 { self.0.clear(); }
        self.0.push(WidthEntry { size_bits, text: text.into(), width });
    }
}

// ── FontManager ───────────────────────────────────────────────────────────────

pub struct FontManager<'a> {
    font:        Font<'a>,
    width_cache: RefCell<WidthCache>,
}

impl<'a> FontManager<'a> {
    pub fn new(font_data: &'a [u8]) -> std::io::Result<Self> {
        let font = Font::try_from_bytes(font_data)
            .ok_or_else(|| std::io::Error::new(
                std::io::ErrorKind::InvalidData, "failed to load font"))?;
        Ok(Self { font, width_cache: RefCell::new(WidthCache::new()) })
    }

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
        let scale     = Scale::uniform(size);
        let v_metrics = self.font.v_metrics(scale);
        let origin    = point(x as f32, y as f32 + v_metrics.ascent);
        let limit_x   = (x + max_w) as f32;

        for glyph in self.font.layout(text, scale, origin) {
            if glyph.position().x > limit_x { break; }
            let Some(bb) = glyph.pixel_bounding_box() else { continue };

            glyph.draw(|gx, gy, cov| {
                if cov < 0.02 { return; }

                let px = bb.min.x + gx as i32;
                let py = bb.min.y + gy as i32;
                if px < 0 || py < 0 { return; }
                let px = px as u32;
                let py = py as u32;

                if cov >= 0.98 {
                    fb.set_pixel(px, py, color);
                } else {
                    let bg = fb.get_pixel(px, py);
                    let a  = ((cov * 255.0 + 0.5) as u32).min(255);
                    let ia = 255u32 - a;
                    let blended = Pixel {
                        r: ((color.r as u32 * a + bg.r as u32 * ia) / 255) as u8,
                        g: ((color.g as u32 * a + bg.g as u32 * ia) / 255) as u8,
                        b: ((color.b as u32 * a + bg.b as u32 * ia) / 255) as u8,
                    };
                    fb.set_pixel(px, py, blended);
                }
            });
        }
    }

    /// Return the pixel advance width of `text` at `size`, cached.
    ///
    /// Cache hit: one linear scan over ≤128 short entries — no allocation.
    /// Cache miss: one `Box<str>` allocation to store the entry.
    pub fn text_width(&self, text: &str, size: f32) -> u32 {
        let size_bits = size.to_bits();

        // ── fast path: cache hit (no allocation) ─────────────────────────────
        {
            let cache = self.width_cache.borrow();
            if let Some(w) = cache.get(text, size_bits) {
                return w;
            }
        }

        // ── slow path: measure and cache ──────────────────────────────────────
        let w = self.measure(text, size);
        self.width_cache.borrow_mut().insert(text, size_bits, w);
        w
    }

    /// Invalidate all cached widths — call after font changes (unused in MVP).
    #[allow(dead_code)]
    pub fn invalidate_cache(&self) {
        self.width_cache.borrow_mut().0.clear();
    }

    fn measure(&self, text: &str, size: f32) -> u32 {
        let scale = Scale::uniform(size);
        if let Some(last) = self.font.layout(text, scale, point(0.0, 0.0)).last() {
            let end = last.position().x + last.unpositioned().h_metrics().advance_width;
            end.ceil() as u32
        } else {
            0
        }
    }
}
