//! Display output — presents framebuffer to screen.
//!
//! For the MVP, this is a file-based output (writes PPM frames).
//! In production, uses DRM/KMS ioctl for direct display.

use rashamon_renderer::Framebuffer;
use std::fs::File;
use std::io::Write;

/// The display subsystem.
pub struct Display {
    width: u32,
    height: u32,
    frame_count: u64,
}

impl Display {
    pub fn new(width: u32, height: u32) -> Result<Self, Box<dyn std::error::Error>> {
        eprintln!("[display] {}x{} (PPM output stub)", width, height);
        Ok(Self {
            width,
            height,
            frame_count: 0,
        })
    }

    /// Present the framebuffer to the display.
    /// In production: DRM/KMS page flip.
    /// For now: write every 60th frame as PPM for verification.
    pub fn present(&mut self, fb: &Framebuffer) -> Result<(), Box<dyn std::error::Error>> {
        self.frame_count += 1;

        // Write a frame every 60 frames for visual verification.
        if self.frame_count % 60 == 1 {
            let path = format!("frame_{:04}.ppm", self.frame_count);
            write_ppm(&path, fb)?;
            eprintln!("[display] Wrote {}", path);
        }

        Ok(())
    }
}

/// Write a framebuffer to a PPM file (portable pixmap format).
fn write_ppm(path: &str, fb: &Framebuffer) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    writeln!(file, "P6")?;
    writeln!(file, "{} {}", fb.width, fb.height)?;
    writeln!(file, "255")?;

    for y in 0..fb.height {
        let row_offset = (y * fb.stride) as usize;
        for x in 0..fb.width {
            let offset = row_offset + (x * 3) as usize;
            file.write_all(&[
                fb.data[offset + 2], // R
                fb.data[offset + 1], // G
                fb.data[offset],     // B
            ])?;
        }
    }

    Ok(())
}
