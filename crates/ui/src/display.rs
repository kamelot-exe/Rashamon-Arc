//! Display output — presents framebuffer to screen.
//!
//! Primary path: DRM/KMS direct display (/dev/dri/card0).
//! Fallback: SDL2 window for desktop environments.

mod drm_display;
mod sdl_display;

use rashamon_renderer::Framebuffer;
use sdl2::VideoSubsystem;
use std::io;

/// The display subsystem.
pub struct Display {
    inner: DisplayInner,
}

enum DisplayInner {
    Drm(drm_display::Display),
    Sdl(sdl_display::Display),
}

impl Display {
    pub fn new(video: &VideoSubsystem, width: u32, height: u32) -> io::Result<Self> {
        // Try DRM/KMS first.
        match drm_display::Display::new(width, height) {
            Ok(drm) => {
                return Ok(Self {
                    inner: DisplayInner::Drm(drm),
                });
            }
            Err(e) => {
                eprintln!("[display] DRM/KMS unavailable ({e}), falling back to SDL2 window");
            }
        }

        // Fallback: SDL2 window.
        let sdl = sdl_display::Display::new(video, width, height)?;
        Ok(Self {
            inner: DisplayInner::Sdl(sdl),
        })
    }

    /// Present the framebuffer to the display.
    pub fn present(&mut self, fb: &Framebuffer) -> io::Result<()> {
        match &mut self.inner {
            DisplayInner::Drm(drm) => drm.present(fb)?,
            DisplayInner::Sdl(sdl) => sdl.present(fb)?,
        }

        Ok(())
    }
}
