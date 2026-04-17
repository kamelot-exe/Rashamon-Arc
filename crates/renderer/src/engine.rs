//! RenderEngine — dispatcher over whichever ContentEngine backend is active.

use crate::engine_trait::{ContentEngine, EngineEvent, EngineFrame};
use crate::framebuffer::Framebuffer;
use crate::servo_host::ServoHost;

/// Top-level handle the browser shell holds.
///
/// Backed by `ServoHost` (real Servo when `feature = "servo"` is enabled,
/// stub otherwise).  Adding a second engine variant is a one-line change here.
pub struct RenderEngine {
    inner: Box<dyn ContentEngine>,
}

impl RenderEngine {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let host = ServoHost::new()?;
        Ok(Self { inner: Box::new(host) })
    }

    pub fn navigate(&mut self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.inner.navigate(url)
    }

    pub fn go_back(&mut self)    -> Result<(), Box<dyn std::error::Error>> { self.inner.go_back() }
    pub fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> { self.inner.go_forward() }
    pub fn reload(&mut self)     -> Result<(), Box<dyn std::error::Error>> { self.inner.reload() }

    pub fn scroll(&mut self, delta_y: i32) { self.inner.scroll(delta_y); }

    /// Composite engine content into the framebuffer content region.
    /// Returns `EngineFrame::Ready` when real pixels were written.
    pub fn render_into(
        &mut self,
        fb:  &mut Framebuffer,
        x:   u32,
        y:   u32,
        w:   u32,
        h:   u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>> {
        self.inner.render_into(fb, x, y, w, h)
    }

    /// Drain engine events.  Call once per frame to sync title/url/load state.
    pub fn poll_events(&mut self) -> Vec<EngineEvent> {
        self.inner.poll_events()
    }

    pub fn title(&self)       -> Option<String> { self.inner.title() }
    pub fn current_url(&self) -> Option<String> { self.inner.current_url() }
}
