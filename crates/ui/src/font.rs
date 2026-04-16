//! Font rendering manager using rusttype.

use rashamon_renderer::framebuffer::{Framebuffer, Pixel};
use rusttype::{Font, Scale, point};

pub struct FontManager<'a> {
    font: Font<'a>,
}

impl<'a> FontManager<'a> {
    pub fn new(font_data: &'a [u8]) -> std::io::Result<Self> {
        let font = Font::try_from_bytes(font_data)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "failed to load font"))?;
        Ok(Self { font })
    }

    pub fn draw_text(
        &self,
        fb: &mut Framebuffer,
        x: u32,
        y: u32,
        text: &str,
        size: f32,
        color: Pixel,
        max_w: u32,
    ) {
        let scale = Scale::uniform(size);
        let v_metrics = self.font.v_metrics(scale);
        let origin = point(x as f32, y as f32 + v_metrics.ascent);
        let glyphs: Vec<_> = self.font.layout(text, scale, origin).collect();

        let limit_x = (x + max_w) as f32;
        for glyph in &glyphs {
            // Stop if we've exceeded max_w
            if glyph.position().x > limit_x {
                break;
            }
            if let Some(bb) = glyph.pixel_bounding_box() {
                glyph.draw(|gx, gy, v| {
                    if v > 0.1 {
                        let px = bb.min.x + gx as i32;
                        let py = bb.min.y + gy as i32;
                        if px >= 0 && py >= 0 {
                            fb.set_pixel(px as u32, py as u32, color);
                        }
                    }
                });
            }
        }
    }

    pub fn text_width(&self, text: &str, size: f32) -> u32 {
        let scale = Scale::uniform(size);
        let glyphs: Vec<_> = self.font.layout(text, scale, point(0.0, 0.0)).collect();
        if glyphs.is_empty() {
            return 0;
        }
        let last = glyphs.last().unwrap();
        let end_x = last.position().x + last.unpositioned().h_metrics().advance_width;
        end_x.ceil() as u32
    }
}
