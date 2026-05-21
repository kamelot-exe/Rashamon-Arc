//! Display output — presents framebuffer to screen.
//!
//! Primary path: DRM/KMS direct display (/dev/dri/card0).
//! Fallback: SDL2 window for desktop environments.

#[cfg(feature = "linux-desktop")]
mod drm_display;
#[cfg(feature = "linux-desktop")]
mod sdl_display;

use rashamon_renderer::Framebuffer;
#[cfg(feature = "linux-desktop")]
use sdl2::VideoSubsystem;
use std::io;

/// The display subsystem.
pub struct Display {
    inner: DisplayInner,
}

enum DisplayInner {
    #[cfg(feature = "linux-desktop")]
    Drm(drm_display::Display),
    #[cfg(feature = "linux-desktop")]
    Sdl(sdl_display::Display),
    #[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
    KamelotStub,
}

impl Display {
    /// `win_w × win_h` — actual window size on screen.
    /// `fb_w × fb_h`  — logical framebuffer size (all UI maths use this).
    #[cfg(feature = "linux-desktop")]
    pub fn new(
        video: &VideoSubsystem,
        win_w: u32,
        win_h: u32,
        fb_w: u32,
        fb_h: u32,
    ) -> io::Result<Self> {
        // Try DRM/KMS first (uses fb dimensions directly).
        match drm_display::Display::new(fb_w, fb_h) {
            Ok(drm) => {
                return Ok(Self {
                    inner: DisplayInner::Drm(drm),
                })
            }
            Err(e) => {
                if std::env::var_os("RASHAMON_DEBUG").is_some() {
                    eprintln!("[display] DRM/KMS unavailable ({e}), falling back to SDL2 window");
                }
            }
        }

        let sdl = sdl_display::Display::new(video, win_w, win_h, fb_w, fb_h)?;
        Ok(Self {
            inner: DisplayInner::Sdl(sdl),
        })
    }

    #[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
    pub fn new_kamelot_stub() -> io::Result<Self> {
        Ok(Self {
            inner: DisplayInner::KamelotStub,
        })
    }

    /// Present the framebuffer to the display.
    pub fn present(&mut self, fb: &Framebuffer) -> io::Result<()> {
        match &mut self.inner {
            #[cfg(feature = "linux-desktop")]
            DisplayInner::Drm(drm) => drm.present(fb)?,
            #[cfg(feature = "linux-desktop")]
            DisplayInner::Sdl(sdl) => sdl.present(fb)?,
            #[cfg(all(feature = "kamelot", not(feature = "linux-desktop")))]
            DisplayInner::KamelotStub => {
                let _ = fb;
            }
        }

        Ok(())
    }
}
