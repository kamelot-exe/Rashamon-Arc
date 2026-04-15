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

pub fn draw_icon_back(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, color: Pixel) {
    let s = size / 2;
    for i in 0..=s {
        fb.set_pixel(cx - s + i, cy - i, color);
        fb.set_pixel(cx - s + i, cy + i, color);
    }
    fb.fill_rect(cx - s, cy, s, 2, color);
}

pub fn draw_icon_forward(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, color: Pixel) {
    let s = size / 2;
    for i in 0..=s {
        fb.set_pixel(cx + s - i, cy - i, color);
        fb.set_pixel(cx + s - i, cy + i, color);
    }
    fb.fill_rect(cx, cy, s, 2, color);
}

pub fn draw_icon_reload(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, color: Pixel) {
    let s = size;
    // crude circle
    for i in 0..=s {
        let angle = (i as f32 / s as f32) * 2.0 * std::f32::consts::PI * 0.8;
        let px = cx as i32 + (angle.cos() * s as f32) as i32;
        let py = cy as i32 + (angle.sin() * s as f32) as i32;
        fb.set_pixel(px as u32, py as u32, color);
    }
    // arrow head
    fb.set_pixel(cx + s, cy - 2, color);
    fb.set_pixel(cx + s, cy - 1, color);
    fb.set_pixel(cx + s - 1, cy - 2, color);
}

pub fn draw_icon_lock(fb: &mut Framebuffer, cx: u32, cy: u32, color: Pixel) {
    // U-shape
    fb.fill_rect(cx - 4, cy - 6, 2, 4, color);
    fb.fill_rect(cx + 3, cy - 6, 2, 4, color);
    fb.fill_rect(cx - 4, cy - 3, 9, 2, color);
    // Body
    fb.fill_rect(cx - 6, cy - 1, 13, 8, color);
}

pub fn draw_icon_spinner(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, frame: u64, color: Pixel) {
    let angle = (frame % 360) as f32 * (std::f32::consts::PI / 180.0);
    let px = cx as i32 + (angle.cos() * size as f32) as i32;
    let py = cy as i32 + (angle.sin() * size as f32) as i32;
    fb.fill_rect(px as u32 - 1, py as u32 - 1, 3, 3, color);
}
