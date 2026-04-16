//! Font rendering with subpixel anti-aliasing via alpha blending.

use rashamon_renderer::framebuffer::{Framebuffer, Pixel};
use rusttype::{Font, Scale, point};

pub struct FontManager<'a> {
    font: Font<'a>,
}

impl<'a> FontManager<'a> {
    pub fn new(font_data: &'a [u8]) -> std::io::Result<Self> {
        let font = Font::try_from_bytes(font_data)
            .ok_or_else(|| std::io::Error::new(
                std::io::ErrorKind::InvalidData, "failed to load font"))?;
        Ok(Self { font })
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
                if cov < 0.02 { return; }                // invisible — skip

                let px = bb.min.x + gx as i32;
                let py = bb.min.y + gy as i32;
                if px < 0 || py < 0 { return; }
                let px = px as u32;
                let py = py as u32;

                if cov >= 0.98 {
                    // Full coverage: no need to read the background.
                    fb.set_pixel(px, py, color);
                } else {
                    // Sub-pixel blend: result = fg * cov + bg * (1 - cov)
                    let bg = fb.get_pixel(px, py);
                    let a  = (cov * 256.0) as u32;
                    let ia = 256 - a;
                    let blended = Pixel {
                        r: ((color.r as u32 * a + bg.r as u32 * ia) >> 8) as u8,
                        g: ((color.g as u32 * a + bg.g as u32 * ia) >> 8) as u8,
                        b: ((color.b as u32 * a + bg.b as u32 * ia) >> 8) as u8,
                    };
                    fb.set_pixel(px, py, blended);
                }
            });
        }
    }

    pub fn text_width(&self, text: &str, size: f32) -> u32 {
        let scale  = Scale::uniform(size);
        let glyphs: Vec<_> = self.font.layout(text, scale, point(0.0, 0.0)).collect();
        let Some(last) = glyphs.last() else { return 0 };
        let end = last.position().x + last.unpositioned().h_metrics().advance_width;
        end.ceil() as u32
    }
}
