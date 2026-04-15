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
    pub fn new(video: &VideoSubsystem, width: u32, height: u32) -> io::Result<Self> {
        eprintln!("[display] {}x{} SDL2 output", width, height);
        let window = video
            .window("Rashamon Arc", width, height)
            .position_centered()
            .build()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let mut canvas = window
            .into_canvas()
            .build()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        let texture_creator = canvas.texture_creator();
        let texture = texture_creator
            .create_texture_streaming(PixelFormatEnum::RGB24, width, height)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // Unsafely extend the texture's lifetime to 'static. This is a common
        // workaround for this issue in older versions of the sdl2 crate.
        let texture = unsafe { mem::transmute::<_, Texture<'static>>(texture) };

        canvas.clear();
        canvas.present();

        Ok(Self {
            canvas,
            texture,
            width,
            height,
        })
    }

    pub fn present(&mut self, fb: &Framebuffer) -> io::Result<()> {
        self.texture
            .with_lock(None, |buffer: &mut [u8], pitch: usize| {
                let fb_w = fb.width as usize;
                let fb_h = fb.height as usize;

                for y in 0..fb_h {
                    let fb_row_start = y * fb.stride as usize;
                    let texture_row_start = y * pitch;

                    let fb_row = &fb.data[fb_row_start..fb_row_start + fb_w * 3];
                    let texture_row = &mut buffer[texture_row_start..texture_row_start + fb_w * 3];

                    for (i, chunk) in fb_row.chunks_exact(3).enumerate() {
                        // Framebuffer is BGR, SDL texture is RGB
                        let b = chunk[0];
                        let g = chunk[1];
                        let r = chunk[2];
                        texture_row[i * 3] = r;
                        texture_row[i * 3 + 1] = g;
                        texture_row[i * 3 + 2] = b;
                    }
                }
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        self.canvas.clear();
        self.canvas.copy(&self.texture, None, None).unwrap();
        self.canvas.present();

        Ok(())
    }
}
