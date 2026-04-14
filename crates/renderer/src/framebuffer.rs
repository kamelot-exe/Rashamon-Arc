//! Framebuffer abstraction for direct display output.

/// RGB888 pixel format — 3 bytes per pixel, little-endian: B, G, R.
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

/// A framebuffer for software rendering.
/// Owns a contiguous memory buffer: rows * cols * 3 bytes.
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub stride: u32, // bytes per row (width * 3, aligned)
    pub data: Vec<u8>,
}

impl Framebuffer {
    /// Create a new framebuffer with the given dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        let stride = (width * 3 + 3) & !3; // align to 4 bytes
        let size = (stride * height) as usize;
        Self {
            width,
            height,
            stride,
            data: vec![0u8; size],
        }
    }

    /// Set a pixel at (x, y).
    pub fn set_pixel(&mut self, x: u32, y: u32, pixel: Pixel) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = (y * self.stride + x * 3) as usize;
        // BGR little-endian
        self.data[offset] = pixel.b;
        self.data[offset + 1] = pixel.g;
        self.data[offset + 2] = pixel.r;
    }

    /// Get a pixel at (x, y).
    pub fn get_pixel(&self, x: u32, y: u32) -> Pixel {
        if x >= self.width || y >= self.height {
            return Pixel::BLACK;
        }
        let offset = (y * self.stride + x * 3) as usize;
        Pixel {
            b: self.data[offset],
            g: self.data[offset + 1],
            r: self.data[offset + 2],
        }
    }

    /// Fill a rectangle with a solid color.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: Pixel) {
        let x0 = x.min(self.width);
        let y0 = y.min(self.height);
        let x1 = (x + w).min(self.width);
        let y1 = (y + h).min(self.height);

        for row in y0..y1 {
            let row_offset = (row * self.stride + x0 * 3) as usize;
            for col in x0..x1 {
                let offset = row_offset + ((col - x0) * 3) as usize;
                self.data[offset] = color.b;
                self.data[offset + 1] = color.g;
                self.data[offset + 2] = color.r;
            }
        }
    }

    /// Blit a dirty region from source to this framebuffer.
    /// Only copies the specified rectangle for efficiency.
    pub fn blit_dirty_rect(&mut self, src: &Framebuffer, dx: u32, dy: u32, sx: u32, sy: u32, w: u32, h: u32) {
        let w = w.min(src.width - sx).min(self.width - dx);
        let h = h.min(src.height - sy).min(self.height - dy);

        for row in 0..h {
            let src_offset = ((sy + row) * src.stride + sx * 3) as usize;
            let dst_offset = ((dy + row) * self.stride + dx * 3) as usize;
            let bytes = (w * 3) as usize;
            self.data[dst_offset..dst_offset + bytes]
                .copy_from_slice(&src.data[src_offset..src_offset + bytes]);
        }
    }

    /// Clear the entire framebuffer to a color.
    pub fn clear(&mut self, color: Pixel) {
        for y in 0..self.height {
            let offset = (y * self.stride) as usize;
            let row_len = (self.width * 3) as usize;
            for x in (0..row_len).step_by(3) {
                self.data[offset + x] = color.b;
                self.data[offset + x + 1] = color.g;
                self.data[offset + x + 2] = color.r;
            }
        }
    }

    /// Get a raw pointer to the buffer data (for DRM/KMS display).
    pub fn as_ptr(&self) -> *const u8 {
        self.data.as_ptr()
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.data.as_mut_ptr()
    }
}
