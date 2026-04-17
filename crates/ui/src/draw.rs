//! UI drawing primitives for Rashamon Arc.

use crate::font::FontManager;
use rashamon_renderer::framebuffer::{Framebuffer, Pixel};
use std::sync::OnceLock;

// ── Precomputed icon geometry ─────────────────────────────────────────────────
//
// Static icons whose geometry never changes are computed once on first use and
// reused every frame.  This eliminates 160+ sin/cos calls per loading frame
// and ~130 calls per chrome redraw.

/// Reload arc at size=7: 48 point-pairs (inner+outer ring) + tip offset.
struct ReloadArc {
    /// (dx_inner, dy_inner, dx_outer, dy_outer) for each of the 48 arc steps.
    points: Vec<(i8, i8, i8, i8)>,
    tip_dx: i8,
    tip_dy: i8,
}

static RELOAD_ARC_7: OnceLock<ReloadArc> = OnceLock::new();

fn reload_arc_7() -> &'static ReloadArc {
    RELOAD_ARC_7.get_or_init(|| {
        let r = 7.0_f32;
        let steps = 48;
        let arc_frac = 0.78_f32;
        let mut points = Vec::with_capacity(steps);
        for i in 0..steps {
            let t     = i as f32 / steps as f32;
            let angle = t * 2.0 * std::f32::consts::PI * arc_frac
                      + std::f32::consts::FRAC_PI_2;
            let (s, c) = angle.sin_cos();
            points.push((
                (c * r) as i8, (s * r) as i8,
                (c * (r + 1.0)) as i8, (s * (r + 1.0)) as i8,
            ));
        }
        let end_angle = 2.0 * std::f32::consts::PI * arc_frac
                      + std::f32::consts::FRAC_PI_2;
        let (es, ec) = end_angle.sin_cos();
        ReloadArc { points, tip_dx: (ec * r) as i8, tip_dy: (es * r) as i8 }
    })
}

/// Globe icon at r=6: outer ring + inner ellipse, as (dx, dy) pairs.
struct GlobePixels {
    outer: Vec<(i8, i8)>,
    inner: Vec<(i8, i8)>,
}

static GLOBE_PIXELS: OnceLock<GlobePixels> = OnceLock::new();

fn globe_pixels() -> &'static GlobePixels {
    GLOBE_PIXELS.get_or_init(|| {
        let r = 6_f32;
        let steps = 40;
        let mut outer = Vec::with_capacity(steps);
        let mut inner = Vec::with_capacity(steps);
        for i in 0..steps {
            let angle = (i as f32 / steps as f32) * 2.0 * std::f32::consts::PI;
            let (s, c) = angle.sin_cos();
            outer.push(((c * r) as i8, (s * r) as i8));
            inner.push(((c * r * 0.55) as i8, (s * r * 0.35) as i8));
        }
        GlobePixels { outer, inner }
    })
}

/// Spinner base circle: unit-radius (cos, sin) for each of 32 steps.
/// At draw time rotate by frame-derived angle with 1 sin_cos() call instead
/// of 32, then scale by radius.
static SPINNER_UNIT: OnceLock<[(f32, f32); 32]> = OnceLock::new();

fn spinner_unit() -> &'static [(f32, f32); 32] {
    SPINNER_UNIT.get_or_init(|| {
        let mut pts = [(0.0_f32, 0.0_f32); 32];
        for i in 0..32 {
            let angle = (i as f32 / 32.0) * 2.0 * std::f32::consts::PI;
            let (s, c) = angle.sin_cos();
            pts[i] = (c, s);
        }
        pts
    })
}

// Safe set_pixel using signed coordinates — clips silently.
#[inline(always)]
fn sp(fb: &mut Framebuffer, x: i32, y: i32, color: Pixel) {
    if x >= 0 && y >= 0 {
        fb.set_pixel(x as u32, y as u32, color);
    }
}

// Draw a 2-pixel-thick line segment via integer coords.
#[inline(always)]
fn sp2(fb: &mut Framebuffer, x: i32, y: i32, dx: i32, dy: i32, color: Pixel) {
    sp(fb, x, y, color);
    sp(fb, x + dx, y + dy, color);
}

// ── Text ─────────────────────────────────────────────────────────────────────

pub fn draw_text(
    fb: &mut Framebuffer,
    font: &FontManager,
    x: u32,
    y: u32,
    text: &str,
    size: f32,
    color: Pixel,
    max_w: u32,
) {
    font.draw_text(fb, x, y, text, size, color, max_w);
}

// ── Filled shapes ─────────────────────────────────────────────────────────────

/// Filled rectangle with rounded corners (all four corners).
pub fn draw_rounded_rect(fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32, r: u32, color: Pixel) {
    if w == 0 || h == 0 { return; }
    let r = r.min(w / 2).min(h / 2);
    if r == 0 {
        fb.fill_rect(x, y, w, h, color);
        return;
    }

    // Horizontal slabs
    fb.fill_rect(x + r, y, w - 2 * r, h, color);
    // Left/right side slabs (excluding corner areas)
    fb.fill_rect(x, y + r, r, h - 2 * r, color);
    fb.fill_rect(x + w - r, y + r, r, h - 2 * r, color);

    // Four rounded corners using midpoint algorithm
    fill_quarter_circle(fb, (x + r) as i32, (y + r) as i32, r as i32, color, 0);       // TL
    fill_quarter_circle(fb, (x + w - r - 1) as i32, (y + r) as i32, r as i32, color, 1); // TR
    fill_quarter_circle(fb, (x + r) as i32, (y + h - r - 1) as i32, r as i32, color, 2); // BL
    fill_quarter_circle(fb, (x + w - r - 1) as i32, (y + h - r - 1) as i32, r as i32, color, 3); // BR
}

/// Filled rect with only the top corners rounded (for tabs).
pub fn draw_rounded_rect_top(fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32, r: u32, color: Pixel) {
    if w == 0 || h == 0 { return; }
    let r = r.min(w / 2).min(h / 2);
    if r == 0 {
        fb.fill_rect(x, y, w, h, color);
        return;
    }
    // Bottom full-width slab
    fb.fill_rect(x, y + r, w, h - r, color);
    // Top center slab
    fb.fill_rect(x + r, y, w - 2 * r, r, color);
    // Top corners
    fill_quarter_circle(fb, (x + r) as i32, (y + r) as i32, r as i32, color, 0);          // TL
    fill_quarter_circle(fb, (x + w - r - 1) as i32, (y + r) as i32, r as i32, color, 1);  // TR
}

/// Draw a filled circle.
pub fn draw_circle_filled(fb: &mut Framebuffer, cx: u32, cy: u32, r: u32, color: Pixel) {
    let cx = cx as i32;
    let cy = cy as i32;
    let r = r as i32;
    for dy in -r..=r {
        let dx = ((r * r - dy * dy) as f32).sqrt() as i32;
        for x in (cx - dx)..=(cx + dx) {
            sp(fb, x, cy + dy, color);
        }
    }
}

// Quarter-circle fill helper: quadrant 0=TL, 1=TR, 2=BL, 3=BR.
fn fill_quarter_circle(fb: &mut Framebuffer, cx: i32, cy: i32, r: i32, color: Pixel, quad: u8) {
    for dy in 0..=r {
        let dx = ((r * r - dy * dy) as f32).sqrt() as i32;
        match quad {
            0 => { for px in (cx - dx)..=cx { sp(fb, px, cy - dy, color); } }
            1 => { for px in cx..=(cx + dx) { sp(fb, px, cy - dy, color); } }
            2 => { for px in (cx - dx)..=cx { sp(fb, px, cy + dy, color); } }
            _ => { for px in cx..=(cx + dx) { sp(fb, px, cy + dy, color); } }
        }
    }
}

// ── Outline shapes ────────────────────────────────────────────────────────────

/// Outline rounded rectangle (border only).
pub fn draw_rounded_rect_outline(fb: &mut Framebuffer, x: i32, y: i32, w: i32, h: i32, r: i32, color: Pixel) {
    if w <= 0 || h <= 0 { return; }
    let r = r.min(w / 2).min(h / 2);
    // Straight edges
    for px in (x + r)..(x + w - r) {
        sp(fb, px, y, color);
        sp(fb, px, y + h - 1, color);
    }
    for py in (y + r)..(y + h - r) {
        sp(fb, x, py, color);
        sp(fb, x + w - 1, py, color);
    }
    // Corner arcs
    outline_quarter_circle(fb, x + r, y + r, r, color, 0);
    outline_quarter_circle(fb, x + w - r - 1, y + r, r, color, 1);
    outline_quarter_circle(fb, x + r, y + h - r - 1, r, color, 2);
    outline_quarter_circle(fb, x + w - r - 1, y + h - r - 1, r, color, 3);
}

fn outline_quarter_circle(fb: &mut Framebuffer, cx: i32, cy: i32, r: i32, color: Pixel, quad: u8) {
    if r <= 0 { sp(fb, cx, cy, color); return; }
    // Bresenham midpoint circle — O(r) integer ops, zero trig.
    let mut x = r;
    let mut y = 0;
    let mut d = 1 - r;
    while x >= y {
        let (ax, ay)  = quad_arc_pt(cx, cy, x, y, quad);
        let (bx, by)  = quad_arc_pt(cx, cy, y, x, quad);
        sp(fb, ax, ay, color);
        sp(fb, bx, by, color);
        y += 1;
        d = if d <= 0 { d + 2 * y + 1 } else { x -= 1; d + 2 * (y - x) + 1 };
    }
}

#[inline(always)]
fn quad_arc_pt(cx: i32, cy: i32, dx: i32, dy: i32, quad: u8) -> (i32, i32) {
    match quad {
        0 => (cx - dx, cy - dy),
        1 => (cx + dx, cy - dy),
        2 => (cx - dx, cy + dy),
        _ => (cx + dx, cy + dy),
    }
}

// ── Icons ─────────────────────────────────────────────────────────────────────

/// Close icon — proper X, 2px thick.
pub fn draw_icon_close(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, color: Pixel) {
    let s = (size / 2) as i32;
    let cx = cx as i32;
    let cy = cy as i32;
    for i in -s..=s {
        sp2(fb, cx + i, cy + i, 1, 0, color);  // \ diagonal, 2px thick
        sp2(fb, cx - i, cy + i, 1, 0, color);  // / diagonal, 2px thick
    }
}

/// Back (left chevron) icon — 2px thick.
pub fn draw_icon_back(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, color: Pixel) {
    let s = (size / 2) as i32;
    let cx = cx as i32;
    let cy = cy as i32;
    for i in 0..=s {
        // upper arm: (cx, cy-s) → (cx-s, cy)
        sp2(fb, cx - i, cy - (s - i), 0, 1, color);
        // lower arm: (cx-s, cy) → (cx, cy+s)
        sp2(fb, cx - i, cy + (s - i), 0, 1, color);
    }
}

/// Forward (right chevron) icon — 2px thick.
pub fn draw_icon_forward(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, color: Pixel) {
    let s = (size / 2) as i32;
    let cx = cx as i32;
    let cy = cy as i32;
    for i in 0..=s {
        sp2(fb, cx + i, cy - (s - i), 0, 1, color);
        sp2(fb, cx + i, cy + (s - i), 0, 1, color);
    }
}

/// Reload / refresh icon — circular arrow.
/// Size=7 (the only size used in the UI) uses precomputed geometry: zero trig per call.
pub fn draw_icon_reload(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, color: Pixel) {
    let cx = cx as i32;
    let cy = cy as i32;

    if size == 7 {
        let arc = reload_arc_7();
        for &(di, dj, di2, dj2) in &arc.points {
            sp(fb, cx + di  as i32, cy + dj  as i32, color);
            sp(fb, cx + di2 as i32, cy + dj2 as i32, color);
        }
        let tip_x = cx + arc.tip_dx as i32;
        let tip_y = cy + arc.tip_dy as i32;
        sp(fb, tip_x - 3, tip_y, color);
        sp(fb, tip_x - 2, tip_y, color);
        sp(fb, tip_x - 2, tip_y + 1, color);
        sp(fb, tip_x - 1, tip_y + 2, color);
        sp(fb, tip_x - 1, tip_y + 3, color);
        return;
    }

    // Fallback for sizes other than 7 (not currently used).
    let r = size as f32;
    let arc_frac = 0.78_f32;
    for i in 0_u32..48 {
        let t     = i as f32 / 48.0;
        let angle = t * 2.0 * std::f32::consts::PI * arc_frac + std::f32::consts::FRAC_PI_2;
        let (s, c) = angle.sin_cos();
        sp(fb, cx + (c * r) as i32,         cy + (s * r) as i32,         color);
        sp(fb, cx + (c * (r + 1.0)) as i32, cy + (s * (r + 1.0)) as i32, color);
    }
    let end_angle = 2.0 * std::f32::consts::PI * arc_frac + std::f32::consts::FRAC_PI_2;
    let (es, ec) = end_angle.sin_cos();
    let tip_x = cx + (ec * r) as i32;
    let tip_y = cy + (es * r) as i32;
    sp(fb, tip_x - 3, tip_y, color);
    sp(fb, tip_x - 2, tip_y, color);
    sp(fb, tip_x - 2, tip_y + 1, color);
    sp(fb, tip_x - 1, tip_y + 2, color);
    sp(fb, tip_x - 1, tip_y + 3, color);
}

/// Add / new-tab icon — clean plus.
pub fn draw_icon_add(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, color: Pixel) {
    let s = (size / 2) as i32;
    let cx = cx as i32;
    let cy = cy as i32;
    // Horizontal bar
    for x in (cx - s)..=(cx + s) {
        sp(fb, x, cy, color);
        sp(fb, x, cy + 1, color);
    }
    // Vertical bar
    for y in (cy - s)..=(cy + s) {
        sp(fb, cx, y, color);
        sp(fb, cx + 1, y, color);
    }
}

/// Lock icon — shackle + body.
pub fn draw_icon_lock(fb: &mut Framebuffer, cx: u32, cy: u32, color: Pixel) {
    let cx = cx as i32;
    let cy = cy as i32;
    // Shackle (U-shape, 2px thick)
    for px in (cx - 3)..=(cx + 3) {
        sp(fb, px, cy - 6, color);
        sp(fb, px, cy - 5, color);
    }
    sp(fb, cx - 3, cy - 6, color);
    sp(fb, cx - 4, cy - 6, color);
    sp(fb, cx - 4, cy - 5, color);
    sp(fb, cx - 4, cy - 4, color);
    sp(fb, cx - 4, cy - 3, color);
    sp(fb, cx + 3, cy - 6, color);
    sp(fb, cx + 4, cy - 6, color);
    sp(fb, cx + 4, cy - 5, color);
    sp(fb, cx + 4, cy - 4, color);
    sp(fb, cx + 4, cy - 3, color);
    // Body
    for row in (cy - 2)..=(cy + 5) {
        for col in (cx - 6)..=(cx + 6) {
            sp(fb, col, row, color);
        }
    }
    // Keyhole (dark, leave void in body) — draw bg-colored oval in body
    // We skip this since we don't know the bg color here; just keep solid
}

/// Globe icon for non-secure or unknown URLs.
/// Uses precomputed ring geometry — zero trig per call.
pub fn draw_icon_globe(fb: &mut Framebuffer, cx: u32, cy: u32, color: Pixel) {
    let cx = cx as i32;
    let cy = cy as i32;
    let r  = 6_i32;

    let gp = globe_pixels();
    for &(dx, dy) in &gp.outer {
        sp(fb, cx + dx as i32,     cy + dy as i32, color);
        sp(fb, cx + dx as i32 + 1, cy + dy as i32, color);
    }
    for x in (cx - r)..=(cx + r) { sp(fb, x,  cy, color); }
    for y in (cy - r)..=(cy + r) { sp(fb, cx, y,  color); }
    for &(dx, dy) in &gp.inner {
        sp(fb, cx + dx as i32, cy + dy as i32, color);
    }
}

/// Animated spinner — orbiting dot trail.
///
/// Base circle positions are precomputed once.  Per-frame cost: 1 sin_cos()
/// for the rotation offset + 32 rotations (float mults/adds) instead of the
/// original 64 independent sin/cos calls.
pub fn draw_icon_spinner(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, frame: u64, color: Pixel) {
    let r  = size as f32;
    let cx = cx as i32;
    let cy = cy as i32;

    // One trig call for the animation phase; rotate precomputed base points.
    let (sin_off, cos_off) = (frame as f32 * 0.12_f32).sin_cos();
    let base = spinner_unit();
    let dim  = Pixel { r: color.r / 2, g: color.g / 2, b: color.b / 2 };

    for i in 0..32_usize {
        let (bx, by) = base[i];
        // Rotate base point by frame offset angle.
        let rx = (cos_off * bx - sin_off * by) * r;
        let ry = (sin_off * bx + cos_off * by) * r;
        let px = cx + rx as i32;
        let py = cy + ry as i32;

        if i < 8 {
            sp(fb, px,     py, color);
            sp(fb, px + 1, py, color);
        } else if i < 16 {
            sp(fb, px, py, dim);
        }
    }
}

/// Star / bookmark icon.
pub fn draw_icon_star(fb: &mut Framebuffer, cx: u32, cy: u32, size: u32, color: Pixel, filled: bool) {
    let s = (size / 2) as i32;
    let cx = cx as i32;
    let cy = cy as i32;
    if filled {
        // Simple filled diamond-ish star
        for dy in -s..=s {
            let dx = s - dy.abs();
            for x in (cx - dx)..=(cx + dx) {
                sp(fb, x, cy + dy, color);
            }
        }
    } else {
        // Outline diamond
        for i in 0..=s {
            sp(fb, cx - i, cy - (s - i), color);
            sp(fb, cx + i, cy - (s - i), color);
            sp(fb, cx - i, cy + (s - i), color);
            sp(fb, cx + i, cy + (s - i), color);
        }
    }
}

/// Menu/dots icon (three horizontal dots).
pub fn draw_icon_menu(fb: &mut Framebuffer, cx: u32, cy: u32, color: Pixel) {
    let cx = cx as i32;
    let cy = cy as i32;
    for dot_x in [-5_i32, 0, 5] {
        draw_circle_filled(fb, (cx + dot_x) as u32, cy as u32, 2, color);
    }
}
