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
    /// `win_w × win_h` — actual window size on screen.
    /// `fb_w × fb_h`  — logical framebuffer size (all UI maths use this).
    pub fn new(
        video: &VideoSubsystem,
        win_w: u32,
        win_h: u32,
        fb_w: u32,
        fb_h: u32,
    ) -> io::Result<Self> {
        // Try DRM/KMS first (uses fb dimensions directly).
        match drm_display::Display::new(fb_w, fb_h) {
            Ok(drm) => return Ok(Self { inner: DisplayInner::Drm(drm) }),
            Err(e)  => eprintln!("[display] DRM/KMS unavailable ({e}), falling back to SDL2 window"),
        }

        let sdl = sdl_display::Display::new(video, win_w, win_h, fb_w, fb_h)?;
        Ok(Self { inner: DisplayInner::Sdl(sdl) })
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
