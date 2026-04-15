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
    title: Option<String>,
    /// Whether Servo is fully initialized.
    initialized: bool,
    history: Vec<String>,
    history_index: usize,
}

impl ServoHost {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // In production: initialize Servo here.
        // servo::init().map_err(|e| e.into())?;
        eprintln!("[servo] ServoHost initialized (stub — full Servo integration pending)");
        Ok(Self {
            title: None,
            initialized: false,
            history: Vec::new(),
            history_index: 0,
        })
    }

    pub fn navigate(&mut self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[servo] navigate -> {url}");
        // In production: servo::load_url(url)?;

        // If we are not at the end of the history list, any new navigation
        // should clear the "forward" history.
        if !self.history.is_empty() && self.history_index < self.history.len() - 1 {
            self.history.truncate(self.history_index + 1);
        }

        // Don't add empty URLs to history (new tabs)
        if !url.is_empty() {
            self.history.push(url.to_string());
            self.history_index = self.history.len().saturating_sub(1);
        }

        self.title = Some(self.get_title_from_url(url));
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
        // This is a stub for the actual web content rendering.
        // It should only draw into the content area, below the browser chrome.
        // The UI process is responsible for clearing the framebuffer and drawing the chrome.

        // A simple visual indicator for the "web page".
        // A real renderer would draw the parsed and laid-out web page here.
        if let Some(url) = self.url() {
            if !url.is_empty() {
                // White background for the page
                fb.clear(Pixel::WHITE);
                // Draw some "text" as a placeholder for the content
                let mut x = 50;
                let y = 100; // Start below the UI chrome
                for _ in url.chars() {
                    fb.fill_rect(x, y, 10, 20, Pixel { r: 0, g: 0, b: 0 });
                    x += 12;
                    if x > fb.width - 50 { break; }
                }
            }
        }
        // If the URL is empty, we don't draw anything, letting the UI process
        // draw the "New Tab" page over a cleared background.
    }

    fn get_title_from_url(&self, url: &str) -> String {
        if url.is_empty() {
            return "New Tab".to_string();
        }
        url.to_string()
            .replace("https://", "")
            .replace("http://", "")
            .replace("www.", "")
    }

    pub fn go_back(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[servo] go_back");
        if self.history_index > 0 {
            self.history_index -= 1;
            if let Some(url) = self.history.get(self.history_index) {
                self.title = Some(self.get_title_from_url(url));
            }
        }
        Ok(())
    }

    pub fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[servo] go_forward");
        if self.history_index < self.history.len() - 1 {
            self.history_index += 1;
            if let Some(url) = self.history.get(self.history_index) {
                self.title = Some(self.get_title_from_url(url));
            }
        }
        Ok(())
    }

    pub fn reload(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[servo] reload");
        if let Some(url) = self.url() {
             eprintln!("[servo] re-navigating to -> {url}");
             self.title = Some(self.get_title_from_url(&url));
        }
        Ok(())
    }

    pub fn title(&self) -> Option<String> {
        self.title.clone()
    }

    pub fn url(&self) -> Option<String> {
        self.history.get(self.history_index).cloned()
    }
}
