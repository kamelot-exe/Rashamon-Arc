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

use crate::engine_trait::{ContentEngine, EngineEvent, EngineFrame, EnginePerfStats};
use crate::framebuffer::Framebuffer;
use crate::permissions::{PermissionDecision, PermissionKind};

#[cfg(not(feature = "servo"))]
use crate::servo_host::ServoHost;

#[cfg(feature = "webkit")]
use crate::webkit_engine::{WebKitEngine, WebKitDriver};

#[cfg(feature = "servo")]
use crate::servo_embedder::ServoHost;

fn debug_enabled() -> bool {
    std::env::var_os("RASHAMON_DEBUG").is_some()
}

/// Top-level rendering handle owned by the browser shell.
/// Must remain on the main thread when WebKit is active.
pub struct RenderEngine {
    inner:       Box<dyn ContentEngine>,
    real_engine: bool,
    #[cfg(feature = "webkit")]
    driver:      Option<WebKitDriver>,
}

impl RenderEngine {
    #[allow(unused_variables)]
    pub fn new(content_w: u32, content_h: u32) -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(feature = "servo")]
        match ServoHost::new(content_w, content_h) {
            Ok(sh) => {
                eprintln!("Renderer: Servo");
                return Ok(Self {
                    inner:       Box::new(sh),
                    real_engine: true,
                    #[cfg(feature = "webkit")]
                    driver:      None,
                });
            }
            Err(e) => {
                if debug_enabled() {
                    eprintln!("[renderer] Servo init failed ({e}), falling back");
                }
            }
        }

        #[cfg(feature = "webkit")]
        match WebKitEngine::create(content_w, content_h) {
            Ok((wk, driver)) => {
                eprintln!("Renderer: WebKitGTK");
                return Ok(Self {
                    inner:       Box::new(wk),
                    real_engine: true,
                    driver:      Some(driver),
                });
            }
            Err(e) => {
                if debug_enabled() {
                    eprintln!("[renderer] WebKit init failed ({e}), falling back to stub");
                } else {
                    eprintln!("Renderer: text fallback");
                }
            }
        }

        if debug_enabled() {
            eprintln!("[renderer] Using stub engine (text renderer active)");
        }
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
    pub fn zoom_in(&mut self)    { self.inner.zoom_in(); }
    pub fn zoom_out(&mut self)   { self.inner.zoom_out(); }
    pub fn zoom_reset(&mut self) { self.inner.zoom_reset(); }
    pub fn adblock_allow_domain(&mut self, domain: &str) {
        self.inner.adblock_allow_domain(domain);
    }
    pub fn adblock_remove_allow_domain(&mut self, domain: &str) {
        self.inner.adblock_remove_allow_domain(domain);
    }
    pub fn find_text(&mut self, query: &str) { self.inner.find_text(query); }
    pub fn find_next(&mut self) { self.inner.find_next(); }
    pub fn find_previous(&mut self) { self.inner.find_previous(); }
    pub fn find_clear(&mut self) { self.inner.find_clear(); }
    pub fn download_url(&mut self, url: &str) { self.inner.download_url(url); }
    pub fn resolve_permission(&mut self, id: u64, allow: bool, remember: bool) {
        self.inner.resolve_permission(id, allow, remember);
    }
    pub fn query_site_permissions(&mut self, origin: &str, private: bool) {
        self.inner.query_site_permissions(origin, private);
    }
    pub fn set_site_permission(
        &mut self,
        origin: &str,
        kind: PermissionKind,
        decision: PermissionDecision,
        private: bool,
    ) {
        self.inner.set_site_permission(origin, kind, decision, private);
    }
    pub fn set_site_adblock_allowlisted(
        &mut self,
        origin: &str,
        allowlisted: bool,
        private: bool,
    ) {
        self.inner
            .set_site_adblock_allowlisted(origin, allowlisted, private);
    }
    pub fn force_suspend_inactive_tabs(&mut self) {
        self.inner.force_suspend_inactive_tabs();
    }

    pub fn can_go_back(&self)    -> bool { self.inner.can_go_back() }
    pub fn can_go_forward(&self) -> bool { self.inner.can_go_forward() }

    pub fn scroll(&mut self, delta_y: i32) { self.inner.scroll(delta_y); }
    pub fn click(&mut self, x: u32, y: u32) { self.inner.click(x, y); }
    pub fn right_click(&mut self, x: u32, y: u32) { self.inner.right_click(x, y); }
    pub fn mouse_move(&mut self, x: u32, y: u32) { self.inner.mouse_move(x, y); }
    pub fn text_input(&mut self, text: &str) { self.inner.text_input(text); }
    pub fn key_press(&mut self, key: &str) { self.inner.key_press(key); }

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
    pub fn perf_stats(&self) -> EnginePerfStats {
        self.inner.perf_stats()
    }

    pub fn current_nav_id(&self) -> u64 { self.inner.current_nav_id() }
    pub fn title(&self)           -> Option<String> { self.inner.title() }
    pub fn current_url(&self)     -> Option<String> { self.inner.current_url() }
    pub fn is_real_engine(&self)  -> bool { self.real_engine }
}
