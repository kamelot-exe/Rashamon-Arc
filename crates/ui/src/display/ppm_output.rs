//! PPM file output — fallback display for testing without DRM access.

use rashamon_renderer::Framebuffer;
use std::fs::File;
use std::io::{self, Write};

pub struct PpmOutput {
    width: u32,
    height: u32,
}

impl PpmOutput {
    pub fn new(width: u32, height: u32) -> io::Result<Self> {
        eprintln!("[display] {}x{} PPM output (DRM fallback)", width, height);
        Ok(Self { width, height })
    }

    /// Write a PPM frame every N frames.
    pub fn present(&mut self, frame_count: u64, fb: &Framebuffer) -> io::Result<()> {
        // Write every 60th frame for verification.
        if frame_count % 60 != 1 {
            return Ok(());
        }

        let path = format!("frame_{:04}.ppm", frame_count);
        let mut file = File::create(&path)?;
        writeln!(file, "P6")?;
        writeln!(file, "{} {}", fb.width, fb.height)?;
        writeln!(file, "255")?;

        for y in 0..fb.height {
            let row_offset = (y * fb.stride) as usize;
            for x in 0..fb.width {
                let offset = row_offset + (x * 3) as usize;
                // PPM expects RGB, our FB stores BGR.
                file.write_all(&[
                    fb.data[offset + 2], // R
                    fb.data[offset + 1], // G
                    fb.data[offset],     // B
                ])?;
            }
        }

        eprintln!("[display] Wrote {}", path);
        Ok(())
    }
}
