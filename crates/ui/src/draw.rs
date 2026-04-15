//! UI drawing primitives.

use rashamon_renderer::framebuffer::{Framebuffer, Pixel};

// A simple placeholder for text rendering.
pub fn draw_text(fb: &mut Framebuffer, x: u32, y: u32, text: &str, color: Pixel, max_w: u32) {
    let mut current_x = x;
    for _ in text.chars() {
        if current_x + 6 > x + max_w {
            break;
        }
        // This is a very basic stub. A real implementation would use a font renderer.
        fb.fill_rect(current_x, y, 5, 10, color);
        current_x += 7; // Advance for next character
    }
}

// Draws a rectangle with rounded corners (simulated).
pub fn draw_rounded_rect(fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32, r: u32, color: Pixel) {
    if r == 0 || w < 2 * r || h < 2 * r {
        fb.fill_rect(x, y, w, h, color);
        return;
    }
    // Center
    fb.fill_rect(x + r, y, w - 2 * r, h, color);
    fb.fill_rect(x, y + r, w, h - 2 * r, color);
}

// --- Icon Drawing ---

pub fn draw_icon_close(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, color: Pixel) {
    let s = size / 2;
    for i in 0..=s {
        fb.set_pixel(cx - i, cy - i, color);
        fb.set_pixel(cx + i, cy - i, color);
        fb.set_pixel(cx - i, cy + i, color);
        fb.set_pixel(cx + i, cy + i, color);
    }
}

pub fn draw_icon_add(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, color: Pixel) {
    let s = size / 2;
    fb.fill_rect(cx - s, cy, size + 1, 2, color);
    fb.fill_rect(cx, cy - s, 2, size + 1, color);
}
