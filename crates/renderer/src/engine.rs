//! RenderEngine — dispatcher over whichever ContentEngine backend is active.
//!
//! Selection order (highest priority first):
//!   1. ServoHost     (feature = "servo")   — Servo engine
//!   2. WebKitEngine  (feature = "webkit")  — WebKitGTK 2.50+ (per-tab WebViews)
//!   3. ServoHost stub                      — text-renderer fallback
//!
//! Tab lifecycle:
//!   Call create_tab(tab_id, is_private) when a new browser tab is created.
//!   Call close_tab(tab_id) before removing the tab from BrowserState.
//!   Call set_active_tab(tab_id) when the active tab changes (no reload issued).
//!   navigate(url, nav_id) always operates on the currently active tab.

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
        #[cfg(feature = "servo")]
        match ServoHost::new(content_w, content_h) {
            Ok(sh) => {
                eprintln!("[renderer] Using Servo engine");
                return Ok(Self {
                    inner:       Box::new(sh),
                    real_engine: true,
                    #[cfg(feature = "webkit")]
                    driver:      None,
                });
            }
            Err(e) => eprintln!("[renderer] Servo init failed ({e}), falling back"),
        }

        #[cfg(feature = "webkit")]
        match WebKitEngine::create(content_w, content_h) {
            Ok((wk, driver)) => {
                eprintln!("[renderer] Using WebKitGTK engine (per-tab WebViews)");
                return Ok(Self {
                    inner:       Box::new(wk),
                    real_engine: true,
                    driver:      Some(driver),
                });
            }
            Err(e) => eprintln!("[renderer] WebKit init failed ({e}), falling back to stub"),
        }

        eprintln!("[renderer] Using stub engine (text renderer active)");
        Ok(Self {
            inner:       Box::new(ServoHost::new()?),
            real_engine: false,
            #[cfg(feature = "webkit")]
            driver:      None,
        })
    }

    // ── GTK pump (no-op on non-WebKit) ────────────────────────────────────────

    pub fn pump_gtk(&mut self) {
        #[cfg(feature = "webkit")]
        if let Some(ref mut d) = self.driver {
            d.pump();
        }
    }

    // ── Tab lifecycle ─────────────────────────────────────────────────────────

    pub fn create_tab(&mut self, tab_id: u64, is_private: bool) {
        self.inner.create_tab(tab_id, is_private);
    }

    pub fn close_tab(&mut self, tab_id: u64) {
        self.inner.close_tab(tab_id);
    }

    /// Activate `tab_id` as the visible tab.  Does NOT reload — the existing
    /// WebView snapshot is blitted immediately; a fresh snapshot is requested
    /// in the background.
    pub fn set_active_tab(&mut self, tab_id: u64) {
        self.inner.set_active_tab(tab_id);
    }

    // ── Navigation ────────────────────────────────────────────────────────────

    pub fn navigate(&mut self, url: &str, nav_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        self.inner.navigate(url, nav_id)
    }

    pub fn go_back(&mut self)    -> Result<(), Box<dyn std::error::Error>> { self.inner.go_back() }
    pub fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> { self.inner.go_forward() }
    pub fn reload(&mut self)     -> Result<(), Box<dyn std::error::Error>> { self.inner.reload() }

    pub fn scroll(&mut self, delta_y: i32) { self.inner.scroll(delta_y); }

    // ── Frame ─────────────────────────────────────────────────────────────────

    pub fn render_into(
        &mut self,
        fb: &mut Framebuffer,
        x: u32, y: u32, w: u32, h: u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>> {
        self.inner.render_into(fb, x, y, w, h)
    }

    /// Drain `(tab_id, event)` pairs produced since the last call.
    /// `tab_id == 0` means "the active tab" (stub path).
    pub fn poll_events(&mut self) -> Vec<(u64, EngineEvent)> {
        self.inner.poll_events()
    }

    pub fn current_nav_id(&self) -> u64 { self.inner.current_nav_id() }
    pub fn title(&self)           -> Option<String> { self.inner.title() }
    pub fn current_url(&self)     -> Option<String> { self.inner.current_url() }
    pub fn is_real_engine(&self)  -> bool { self.real_engine }
}
