//! RenderEngine — dispatcher over whichever ContentEngine backend is active.
//!
//! Selection order (highest priority first):
//!   1. ServoHost     (feature = "servo")   — Servo engine
//!   2. WebKitEngine  (feature = "webkit")  — WebKitGTK 2.50+, default real engine
//!   3. ServoHost stub (always available)   — text-renderer fallback
//!
//! GTK note: when WebKit is active, RenderEngine owns a WebKitDriver.
//! Call `pump_gtk()` once per frame from the main thread so GTK events are processed.

use crate::engine_trait::{ContentEngine, EngineEvent, EngineFrame};
use crate::framebuffer::Framebuffer;

#[cfg(not(feature = "servo"))]
use crate::servo_host::ServoHost;

#[cfg(feature = "webkit")]
use crate::webkit_engine::{WebKitEngine, WebKitDriver};

#[cfg(feature = "servo")]
use crate::servo_embedder::ServoHost;

/// Top-level rendering handle owned by the browser shell.
/// Must remain on the main thread when WebKit is active.
pub struct RenderEngine {
    inner:       Box<dyn ContentEngine>,
    real_engine: bool,
    #[cfg(feature = "webkit")]
    driver:      Option<WebKitDriver>,
}

impl RenderEngine {
    pub fn new(content_w: u32, content_h: u32) -> Result<Self, Box<dyn std::error::Error>> {
        // Servo takes priority when built.
        #[cfg(feature = "servo")]
        match ServoHost::new(content_w, content_h) {
            Ok(sh) => {
                eprintln!("[renderer] Using Servo engine");
                return Ok(Self {
                    inner: Box::new(sh),
                    real_engine: true,
                    #[cfg(feature = "webkit")]
                    driver: None,
                });
            }
            Err(e) => eprintln!("[renderer] Servo init failed ({e}), falling back"),
        }

        // WebKitGTK — real rendering. GTK is initialised here (main thread).
        #[cfg(feature = "webkit")]
        match WebKitEngine::create(content_w, content_h) {
            Ok((wk, driver)) => {
                eprintln!("[renderer] Using WebKitGTK engine");
                return Ok(Self {
                    inner:       Box::new(wk),
                    real_engine: true,
                    driver:      Some(driver),
                });
            }
            Err(e) => eprintln!("[renderer] WebKit init failed ({e}), falling back to stub"),
        }

        // Stub fallback: text renderer handles content.
        eprintln!("[renderer] Using stub engine (text renderer active)");
        Ok(Self {
            inner:       Box::new(ServoHost::new()?),
            real_engine: false,
            #[cfg(feature = "webkit")]
            driver:      None,
        })
    }

    /// Pump pending GTK/GLib events and dispatch WebKit commands.
    /// Call once per frame **from the main thread** when WebKit is active.
    pub fn pump_gtk(&mut self) {
        #[cfg(feature = "webkit")]
        if let Some(ref mut d) = self.driver {
            d.pump();
        }
    }

    pub fn navigate(&mut self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.inner.navigate(url)
    }

    pub fn go_back(&mut self)    -> Result<(), Box<dyn std::error::Error>> { self.inner.go_back() }
    pub fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> { self.inner.go_forward() }
    pub fn reload(&mut self)     -> Result<(), Box<dyn std::error::Error>> { self.inner.reload() }

    pub fn scroll(&mut self, delta_y: i32) { self.inner.scroll(delta_y); }

    pub fn render_into(
        &mut self,
        fb:  &mut Framebuffer,
        x:   u32, y: u32, w: u32, h: u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>> {
        self.inner.render_into(fb, x, y, w, h)
    }

    pub fn poll_events(&mut self) -> Vec<EngineEvent> {
        self.inner.poll_events()
    }

    pub fn title(&self)       -> Option<String> { self.inner.title() }
    pub fn current_url(&self) -> Option<String> { self.inner.current_url() }

    /// True when a real rendering engine (Servo or WebKit) is active.
    pub fn is_real_engine(&self) -> bool { self.real_engine }
}
