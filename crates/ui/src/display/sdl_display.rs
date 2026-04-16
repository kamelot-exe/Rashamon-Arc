//! SDL2 display — fallback for interactive window when DRM is unavailable.

use rashamon_renderer::Framebuffer;
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{Canvas, Texture};
use sdl2::video::Window;
use sdl2::VideoSubsystem;
use std::io;
use std::mem;

pub struct Display {
    canvas:  Canvas<Window>,
    texture: Texture<'static>,
    fb_w:    u32,
    fb_h:    u32,
}

impl Display {
    pub fn new(
        video: &VideoSubsystem,
        win_w: u32, win_h: u32,
        fb_w:  u32, fb_h:  u32,
    ) -> io::Result<Self> {
        eprintln!("[display] window {}×{}, fb {}×{} (SDL2)", win_w, win_h, fb_w, fb_h);

        let window = video
            .window("Rashamon Arc", win_w, win_h)
            .position_centered()
            .build()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let mut canvas = window
            .into_canvas()
            .accelerated()          // use GPU for the final blit
            .build()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let tc = canvas.texture_creator();

        // BGR24 matches the framebuffer's in-memory layout exactly (b,g,r per
        // pixel), so present() can copy rows with a single memcpy — no swap.
        let texture = tc
            .create_texture_streaming(PixelFormatEnum::BGR24, fb_w, fb_h)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // Extend lifetime past tc (standard SDL2 Rust workaround — tc is a
        // zero-cost wrapper; the real resource is owned by the canvas/renderer).
        let texture = unsafe { mem::transmute::<_, Texture<'static>>(texture) };

        canvas.clear();
        canvas.present();

        Ok(Self { canvas, texture, fb_w, fb_h })
    }

    /// Copy framebuffer to the SDL texture and blit to window.
    /// The framebuffer stores pixels as BGR, and our texture is BGR24 —
    /// so each row is a straight memcpy with no per-pixel conversion.
    pub fn present(&mut self, fb: &Framebuffer) -> io::Result<()> {
        let fb_w    = self.fb_w as usize;
        let fb_h    = self.fb_h as usize;
        let stride  = fb.stride as usize;
        let row_len = fb_w * 3; // bytes we want per row (no padding)

        self.texture
            .with_lock(None, |buf: &mut [u8], pitch: usize| {
                for y in 0..fb_h {
                    let src = y * stride;
                    let dst = y * pitch;
                    buf[dst..dst + row_len]
                        .copy_from_slice(&fb.data[src..src + row_len]);
                }
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        self.canvas.clear();
        self.canvas
            .copy(&self.texture, None, None)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        self.canvas.present();
        Ok(())
    }
}
