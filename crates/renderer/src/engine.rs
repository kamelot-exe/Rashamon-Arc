//! RenderEngine — dispatcher over whichever ContentEngine backend is active.
//!
//! Selection order (highest priority first):
//!   1. ServoHost     (feature = "servo")   — Servo engine, requires SpiderMonkey build
//!   2. WebKitEngine  (feature = "webkit")  — WebKitGTK 2.50+, default real engine
//!   3. ServoHost stub (always available)   — text-renderer fallback

use crate::engine_trait::{ContentEngine, EngineEvent, EngineFrame};
use crate::framebuffer::Framebuffer;

#[cfg(not(feature = "servo"))]
use crate::servo_host::ServoHost;

#[cfg(feature = "webkit")]
use crate::webkit_engine::WebKitEngine;

#[cfg(feature = "servo")]
use crate::servo_embedder::ServoHost;

/// Top-level rendering handle owned by the browser shell.
pub struct RenderEngine {
    inner: Box<dyn ContentEngine>,
}

impl RenderEngine {
    /// Construct the best available engine for the given content area size.
    ///
    /// `content_w` × `content_h`: pixel dimensions of the content region
    /// (everything below the chrome bar).
    pub fn new(content_w: u32, content_h: u32) -> Result<Self, Box<dyn std::error::Error>> {
        // Servo takes priority when built — it supersedes WebKit entirely.
        #[cfg(feature = "servo")]
        match ServoHost::new(content_w, content_h) {
            Ok(sh) => {
                eprintln!("[renderer] Using Servo engine");
                return Ok(Self { inner: Box::new(sh) });
            }
            Err(e) => {
                eprintln!("[renderer] Servo init failed ({e}), falling back");
            }
        }

        // WebKitGTK — default real rendering path.
        #[cfg(feature = "webkit")]
        match WebKitEngine::new(content_w, content_h) {
            Ok(wk) => {
                eprintln!("[renderer] Using WebKitGTK engine");
                return Ok(Self { inner: Box::new(wk) });
            }
            Err(e) => {
                eprintln!("[renderer] WebKit init failed ({e}), falling back to stub");
            }
        }

        // Stub fallback: text renderer handles content, engine is no-op.
        eprintln!("[renderer] Using stub engine (text renderer active)");
        Ok(Self { inner: Box::new(ServoHost::new()?) })
    }

    pub fn navigate(&mut self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.inner.navigate(url)
    }

    pub fn go_back(&mut self)    -> Result<(), Box<dyn std::error::Error>> { self.inner.go_back() }
    pub fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> { self.inner.go_forward() }
    pub fn reload(&mut self)     -> Result<(), Box<dyn std::error::Error>> { self.inner.reload() }

    pub fn scroll(&mut self, delta_y: i32) { self.inner.scroll(delta_y); }

    /// Composite engine content into the framebuffer content region.
    pub fn render_into(
        &mut self,
        fb:  &mut Framebuffer,
        x:   u32, y: u32, w: u32, h: u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>> {
        self.inner.render_into(fb, x, y, w, h)
    }

    /// Drain engine events. Call once per frame to sync title/url/load state.
    pub fn poll_events(&mut self) -> Vec<EngineEvent> {
        self.inner.poll_events()
    }

    pub fn title(&self)       -> Option<String> { self.inner.title() }
    pub fn current_url(&self) -> Option<String> { self.inner.current_url() }

    /// True when a real rendering engine (Servo or WebKit) is active.
    /// When true the browser shell should skip its text-fetch fallback.
    pub fn is_real_engine(&self) -> bool {
        #[cfg(feature = "servo")]  { return true; }
        #[cfg(feature = "webkit")] { return true; }
        #[allow(unreachable_code)] false
    }
}
