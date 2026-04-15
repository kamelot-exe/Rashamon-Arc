//! Font rendering manager using rusttype.

use ab_glyph::{FontRef, PxScale, point};
use rashamon_renderer::framebuffer::{Framebuffer, Pixel};
use rusttype::Font;
use std::io;

pub struct FontManager<'a> {
    font: Font<'a>,
}

impl<'a> FontManager<'a> {
    pub fn new(font_data: &'a [u8]) -> io::Result<Self> {
        let font = Font::try_from_bytes(font_data)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "failed to load font"))?;
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
        let scale = PxScale::from(size);
        let scaled_font = self.font.as_scaled(scale);

        let mut current_x = x as f32;
        let v_metrics = self.font.v_metrics(scale);
        let y_pos = y as f32 + v_metrics.ascent;

        for c in text.chars() {
            if c.is_control() {
                continue;
            }
            let glyph = self.font.glyph(c);
            let h_metrics = glyph.h_metrics();
            let scaled_glyph = scaled_font.scaled_glyph(c);

            if let Some(outline) = scaled_glyph.outline() {
                let bounds = outline.px_bounds();
                if current_x + bounds.width() > (x + max_w) as f32 {
                    break;
                }

                outline.draw(|px, py, v| {
                    if v > 0.1 { // Only draw pixels with significant coverage
                        let final_x = (current_x + bounds.min.x + px as f32) as u32;
                        let final_y = (y_pos + bounds.min.y + py as f32) as u32;
                        fb.set_pixel(final_x, final_y, color);
                    }
                });
            }
            current_x += h_metrics.advance_width;
        }
    }

    pub fn text_width(&self, text: &str, size: f32) -> u32 {
        let scale = PxScale::from(size);
        let mut width = 0.0;
        for c in text.chars() {
            let glyph = self.font.glyph(c);
            let h_metrics = glyph.h_metrics();
            width += h_metrics.advance_width;
        }
        (width * (size / self.font.height_unscaled())) as u32
    }
}
