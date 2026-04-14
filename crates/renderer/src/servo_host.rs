//! Servo host — manages the Servo rendering engine.
//!
//! This is a minimal stub that demonstrates the integration pattern.
//! Full Servo integration requires the `servo` crate which depends on
//! many system libraries (OpenGL, EGL, libxml2, etc.).
//!
//! For the MVP software-rendering path, we use Servo's offscreen API
//! once the full dependency is available.

use crate::framebuffer::Framebuffer;

/// Host process for the Servo engine.
pub struct ServoHost {
    current_url: Option<String>,
    title: Option<String>,
    /// Whether Servo is fully initialized.
    initialized: bool,
}

impl ServoHost {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // In production: initialize Servo here.
        // servo::init().map_err(|e| e.into())?;
        eprintln!("[servo] ServoHost initialized (stub — full Servo integration pending)");
        Ok(Self {
            current_url: None,
            title: None,
            initialized: false,
        })
    }

    pub fn navigate(&mut self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[servo] navigate -> {url}");
        // In production: servo::load_url(url)?;
        self.current_url = Some(url.to_string());
        self.title = Some(url.to_string());
        self.initialized = true;
        Ok(())
    }

    pub fn render(&mut self, fb: &mut Framebuffer) -> Result<(), Box<dyn std::error::Error>> {
        if !self.initialized {
            return Ok(());
        }
        // In production: Servo renders into an offscreen buffer.
        // We grab the buffer and blit it into our framebuffer.
        // servo::render_frame()?;
        // let servo_fb = servo::framebuffer()?;
        // fb.blit_dirty_rect(servo_fb, 0, 0, 0, 0, servo_fb.width, servo_fb.height);

        // Stub: render a simple "page" pattern to demonstrate the pipeline
        self.render_stub(fb);
        Ok(())
    }

    /// Render a stub pattern so we can see something on screen.
    fn render_stub(&self, fb: &mut Framebuffer) {
        use crate::framebuffer::Pixel;
        // White background
        fb.clear(Pixel::WHITE);

        // Draw a simple "browser" frame to show the pipeline works
        let w = fb.width;
        let h = fb.height;

        // Top bar (dark gray)
        fb.fill_rect(0, 0, w, 40, Pixel { r: 48, g: 48, b: 48 });

        // URL bar area (lighter gray)
        fb.fill_rect(10, 8, w - 20, 24, Pixel { r: 72, g: 72, b: 72 });

        // If we have a URL, render it as a simple text indicator
        if let Some(ref url) = self.current_url {
            // Render "Rashamon Arc" + URL as colored blocks (font rendering comes later)
            // For now, a simple visual indicator: green bar proportional to URL length
            let bar_w = (url.len() as u32 * 5).min(w - 30);
            fb.fill_rect(15, 12, bar_w, 16, Pixel { r: 80, g: 180, b: 80 });
        }

        // Content area: render a checkerboard pattern to show active rendering
        let tile = 20;
        for y in (50..h).step_by(tile as usize) {
            for x in (0..w).step_by(tile as usize) {
                let cx = x / tile;
                let cy = (y - 50) / tile;
                if (cx + cy) % 2 == 0 {
                    fb.fill_rect(x, y, tile, tile, Pixel { r: 240, g: 240, b: 240 });
                }
            }
        }

        // Bottom status bar
        fb.fill_rect(0, h - 24, w, 24, Pixel { r: 48, g: 48, b: 48 });
    }

    pub fn go_back(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[servo] go_back");
        Ok(())
    }

    pub fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[servo] go_forward");
        Ok(())
    }

    pub fn reload(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[servo] reload");
        if let Some(ref url) = self.current_url.clone() {
            self.navigate(url)?;
        }
        Ok(())
    }

    pub fn title(&self) -> Option<String> {
        self.title.clone()
    }

    pub fn url(&self) -> Option<String> {
        self.current_url.clone()
    }
}
