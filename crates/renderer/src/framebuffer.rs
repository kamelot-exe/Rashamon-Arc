//! Framebuffer abstraction for direct display output.

/// RGB888 pixel — stored internally as BGR (blue at lowest byte address).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pixel {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Pixel {
    pub const BLACK: Pixel = Pixel { r: 0, g: 0, b: 0 };
    pub const WHITE: Pixel = Pixel { r: 255, g: 255, b: 255 };
}

pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub stride: u32,   // bytes per row (width * 3, aligned to 4 bytes)
    pub data: Vec<u8>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let stride = (width * 3 + 3) & !3;
        let size   = (stride * height) as usize;
        Self { width, height, stride, data: vec![0u8; size] }
    }

    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, pixel: Pixel) {
        if x >= self.width || y >= self.height { return; }
        let off = (y * self.stride + x * 3) as usize;
        self.data[off]     = pixel.b;
        self.data[off + 1] = pixel.g;
        self.data[off + 2] = pixel.r;
    }

    #[inline]
    pub fn get_pixel(&self, x: u32, y: u32) -> Pixel {
        if x >= self.width || y >= self.height { return Pixel::BLACK; }
        let off = (y * self.stride + x * 3) as usize;
        Pixel { b: self.data[off], g: self.data[off + 1], r: self.data[off + 2] }
    }

    /// Fill a solid-colour rectangle. Uses copy_within so each row is
    /// written once then memcpy'd — far faster than per-pixel loops.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: Pixel) {
        let x0 = x.min(self.width);
        let y0 = y.min(self.height);
        let x1 = x.saturating_add(w).min(self.width);
        let y1 = y.saturating_add(h).min(self.height);
        if x0 >= x1 || y0 >= y1 { return; }

        let cols      = (x1 - x0) as usize;
        let row_bytes = cols * 3;
        let stride    = self.stride as usize;

        // Write first row pixel by pixel.
        let row0 = (y0 * self.stride + x0 * 3) as usize;
        for c in 0..cols {
            let off = row0 + c * 3;
            self.data[off]     = color.b;
            self.data[off + 1] = color.g;
            self.data[off + 2] = color.r;
        }

        // Clone first row into every subsequent row with a single memcpy.
        for row in (y0 + 1)..y1 {
            let dst = (row as usize) * stride + x0 as usize * 3;
            self.data.copy_within(row0..row0 + row_bytes, dst);
        }
    }

    /// Clear the entire framebuffer to one colour.
    pub fn clear(&mut self, color: Pixel) {
        let width  = self.width  as usize;
        let stride = self.stride as usize;

        // Fill row 0.
        for x in 0..width {
            let off = x * 3;
            self.data[off]     = color.b;
            self.data[off + 1] = color.g;
            self.data[off + 2] = color.r;
        }
        let row_bytes = width * 3;

        // Broadcast row 0 to all remaining rows.
        for row in 1..self.height as usize {
            let dst = row * stride;
            self.data.copy_within(0..row_bytes, dst);
        }
    }

    pub fn blit_dirty_rect(
        &mut self, src: &Framebuffer,
        dx: u32, dy: u32, sx: u32, sy: u32, w: u32, h: u32,
    ) {
        let w = w.min(src.width.saturating_sub(sx)).min(self.width.saturating_sub(dx));
        let h = h.min(src.height.saturating_sub(sy)).min(self.height.saturating_sub(dy));
        for row in 0..h {
            let s = ((sy + row) * src.stride  + sx * 3) as usize;
            let d = ((dy + row) * self.stride + dx * 3) as usize;
            let n = (w * 3) as usize;
            self.data[d..d + n].copy_from_slice(&src.data[s..s + n]);
        }
    }

    pub fn as_ptr(&self)     -> *const u8 { self.data.as_ptr() }
    pub fn as_mut_ptr(&mut self) -> *mut u8 { self.data.as_mut_ptr() }
}
