//! Render engine abstraction — wraps Servo or WPE WebKit.

use crate::framebuffer::Framebuffer;
use crate::servo_host::ServoHost;

/// The rendering engine.
pub enum RenderEngine {
    Servo(ServoHost),
    // WpeWebKit — placeholder for fallback research path
}

impl RenderEngine {
    /// Create a new render engine. Attempts Servo first.
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let servo = ServoHost::new()?;
        Ok(Self::Servo(servo))
    }

    /// Navigate to a URL.
    pub fn navigate(&mut self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Servo(host) => host.navigate(url),
        }
    }

    /// Render the current page into the framebuffer.
    pub fn render(&mut self, fb: &mut Framebuffer) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Servo(host) => host.render(fb),
        }
    }

    /// Go back in history.
    pub fn go_back(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Servo(host) => host.go_back(),
        }
    }

    /// Go forward in history.
    pub fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Servo(host) => host.go_forward(),
        }
    }

    /// Reload the current page.
    pub fn reload(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Servo(host) => host.reload(),
        }
    }

    /// Get the current page title.
    pub fn title(&self) -> Option<String> {
        match self {
            Self::Servo(host) => host.title(),
        }
    }

    /// Get the current URL.
    pub fn url(&self) -> Option<String> {
        match self {
            Self::Servo(host) => host.url(),
        }
    }
}
