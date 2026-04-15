//! Display output — presents framebuffer to screen.
//!
//! Primary path: DRM/KMS direct display (/dev/dri/card0).
//! Fallback: PPM file output for testing without DRM access.

mod drm_display;
mod ppm_output;

use rashamon_renderer::Framebuffer;
use std::io;

/// The display subsystem.
pub struct Display {
    inner: DisplayInner,
    frame_count: u64,
}

enum DisplayInner {
    Drm(drm_display::Display),
    Ppm(ppm_output::PpmOutput),
}

impl Display {
    pub fn new(width: u32, height: u32) -> io::Result<Self> {
        // Try DRM/KMS first.
        match drm_display::Display::new(width, height) {
            Ok(drm) => {
                return Ok(Self {
                    inner: DisplayInner::Drm(drm),
                    frame_count: 0,
                });
            }
            Err(e) => {
                eprintln!("[display] DRM/KMS unavailable ({e}), falling back to PPM output");
            }
        }

        // Fallback: PPM output.
        let ppm = ppm_output::PpmOutput::new(width, height)?;
        Ok(Self {
            inner: DisplayInner::Ppm(ppm),
            frame_count: 0,
        })
    }

    /// Present the framebuffer to the display.
    pub fn present(&mut self, fb: &Framebuffer) -> io::Result<()> {
        self.frame_count += 1;

        match &mut self.inner {
            DisplayInner::Drm(drm) => drm.present(fb)?,
            DisplayInner::Ppm(ppm) => ppm.present(self.frame_count, fb)?,
        }

        Ok(())
    }
}
