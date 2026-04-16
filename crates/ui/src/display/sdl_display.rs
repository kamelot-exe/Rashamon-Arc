//! SDL2 display — fallback for interactive window when DRM is unavailable.

use rashamon_renderer::Framebuffer;
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{Canvas, Texture};
use sdl2::video::Window;
use sdl2::VideoSubsystem;
use std::io;
use std::mem;

pub struct Display {
    canvas: Canvas<Window>,
    texture: Texture<'static>,
    width: u32,
    height: u32,
}

impl Display {
    /// Create a window of `win_w × win_h` pixels that displays a framebuffer
    /// of `fb_w × fb_h` pixels (stretched to fill the window if sizes differ).
    pub fn new(
        video: &VideoSubsystem,
        win_w: u32,
        win_h: u32,
        fb_w: u32,
        fb_h: u32,
    ) -> io::Result<Self> {
        eprintln!("[display] window {}x{}, fb {}x{} (SDL2)", win_w, win_h, fb_w, fb_h);

        let window = video
            .window("Rashamon Arc", win_w, win_h)
            .position_centered()
            .build()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let mut canvas = window
            .into_canvas()
            .build()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let texture_creator = canvas.texture_creator();
        // The texture matches the *framebuffer* dimensions, not the window.
        // SDL2 will scale it to fill the window on copy.
        let texture = texture_creator
            .create_texture_streaming(PixelFormatEnum::RGB24, fb_w, fb_h)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // Extend texture lifetime past texture_creator (standard SDL2 Rust workaround).
        let texture = unsafe { mem::transmute::<_, Texture<'static>>(texture) };

        canvas.clear();
        canvas.present();

        Ok(Self { canvas, texture, width: fb_w, height: fb_h })
    }

    pub fn present(&mut self, fb: &Framebuffer) -> io::Result<()> {
        self.texture
            .with_lock(None, |buffer: &mut [u8], pitch: usize| {
                for y in 0..self.height as usize {
                    let src = y * fb.stride as usize;
                    let dst = y * pitch;
                    let row_src = &fb.data[src..src + self.width as usize * 3];
                    let row_dst = &mut buffer[dst..dst + self.width as usize * 3];
                    for (i, chunk) in row_src.chunks_exact(3).enumerate() {
                        // Framebuffer is BGR, SDL texture expects RGB
                        row_dst[i * 3]     = chunk[2]; // R
                        row_dst[i * 3 + 1] = chunk[1]; // G
                        row_dst[i * 3 + 2] = chunk[0]; // B
                    }
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
