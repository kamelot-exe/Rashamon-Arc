//! SDL2 display — fallback for interactive window when DRM is unavailable.

use rashamon_renderer::Framebuffer;
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{Canvas, Texture};
use sdl2::video::Window;
use sdl2::VideoSubsystem;
use std::io;
use std::mem;
use std::time::{Duration, Instant};

pub struct Display {
    canvas:  Canvas<Window>,
    texture: Texture<'static>,
    fb_w:    u32,
    fb_h:    u32,
    perf: PerfStats,
}

#[derive(Default)]
struct PerfStats {
    enabled: bool,
    last_dump: Option<Instant>,
    frames: u64,
    copy_us: u128,
    present_us: u128,
}

impl Display {
    pub fn new(
        video: &VideoSubsystem,
        win_w: u32, win_h: u32,
        fb_w:  u32, fb_h:  u32,
    ) -> io::Result<Self> {
        eprintln!("Display: SDL2 window {}x{}", win_w, win_h);

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

        // ARGB8888 matches the framebuffer's BGRA memory layout on little-endian
        // hosts (B,G,R,A bytes in memory), so present() can memcpy rows directly.
        let texture = tc
            .create_texture_streaming(PixelFormatEnum::ARGB8888, fb_w, fb_h)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // Extend lifetime past tc (standard SDL2 Rust workaround — tc is a
        // zero-cost wrapper; the real resource is owned by the canvas/renderer).
        let texture = unsafe { mem::transmute::<_, Texture<'static>>(texture) };

        canvas.clear();
        canvas.present();

        Ok(Self {
            canvas,
            texture,
            fb_w,
            fb_h,
            perf: PerfStats {
                enabled: std::env::var_os("RASHAMON_PERF").is_some()
                    || std::env::args().skip(1).any(|arg| arg == "--perf"),
                last_dump: Some(Instant::now()),
                ..PerfStats::default()
            },
        })
    }

    /// Copy framebuffer to the SDL texture and blit to window.
    /// The framebuffer stores pixels as BGRA, and our texture is ARGB8888 —
    /// so each row is a straight memcpy with no per-pixel conversion.
    pub fn present(&mut self, fb: &Framebuffer) -> io::Result<()> {
        let fb_w    = self.fb_w as usize;
        let fb_h    = self.fb_h as usize;
        let stride  = fb.stride as usize;
        let row_len = fb_w * 4; // bytes we want per row (no padding)

        let copy_t0 = Instant::now();
        self.texture
            .with_lock(None, |buf: &mut [u8], pitch: usize| {
                if pitch == row_len && stride == row_len {
                    // Fast path: contiguous framebuffer copy, no per-row loop.
                    buf[..row_len * fb_h].copy_from_slice(&fb.data[..row_len * fb_h]);
                } else {
                    for y in 0..fb_h {
                        let src = y * stride;
                        let dst = y * pitch;
                        buf[dst..dst + row_len]
                            .copy_from_slice(&fb.data[src..src + row_len]);
                    }
                }
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let copy_elapsed = copy_t0.elapsed();

        let present_t0 = Instant::now();
        self.canvas
            .copy(&self.texture, None, None)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        self.canvas.present();
        let present_elapsed = present_t0.elapsed();

        if self.perf.enabled {
            self.perf.frames = self.perf.frames.saturating_add(1);
            self.perf.copy_us += copy_elapsed.as_micros();
            self.perf.present_us += present_elapsed.as_micros();
            if self
                .perf
                .last_dump
                .map_or(false, |t0| t0.elapsed() >= Duration::from_secs(1))
            {
                let frames = self.perf.frames.max(1) as f64;
                eprintln!(
                    "[perf] fb_to_texture_ms={:.3} texture_present_ms={:.3}",
                    self.perf.copy_us as f64 / frames / 1000.0,
                    self.perf.present_us as f64 / frames / 1000.0
                );
                self.perf.frames = 0;
                self.perf.copy_us = 0;
                self.perf.present_us = 0;
                self.perf.last_dump = Some(Instant::now());
            }
        }
        Ok(())
    }
}
