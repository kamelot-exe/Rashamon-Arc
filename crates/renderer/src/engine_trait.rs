//! ContentEngine — stable interface every rendering backend must implement.

use crate::framebuffer::Framebuffer;

/// Events the engine pushes up to the browser shell.
/// Drained once per frame via `ContentEngine::poll_events`.
/// Each event is tagged with the `tab_id` of the WebView that produced it.
/// A `tab_id` of 0 means "active tab" (used by single-view stubs).
#[derive(Debug, Clone)]
pub enum EngineEvent {
    TitleChanged(String),
    /// Actual URL after redirects.
    UrlChanged(String),
    LoadStarted,
    LoadComplete,
    LoadFailed(String),
    /// Full scrollable height of the loaded page in pixels.
    ContentHeightChanged(u32),
}

/// Whether the engine wrote real pixels into the framebuffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineFrame {
    /// Engine composited pixels — caller should skip its own content renderer.
    Ready,
    /// Engine not yet ready or has no page — caller should use its fallback.
    NotReady,
}

/// Stable contract every content engine must satisfy.
pub trait ContentEngine: Send {
    // ── Tab lifecycle (default no-ops for single-view stubs) ──────────────────

    /// Create a new WebView for `tab_id`.  Private tabs get an ephemeral context.
    fn create_tab(&mut self, _tab_id: u64, _is_private: bool) {}

    /// Destroy the WebView for `tab_id` and release its resources.
    fn close_tab(&mut self, _tab_id: u64) {}

    /// Mark `tab_id` as the active tab and request a fresh snapshot.
    /// For per-tab engines this does NOT trigger a page reload.
    fn set_active_tab(&mut self, _tab_id: u64) {}

    // ── Navigation (operate on the currently active tab) ──────────────────────

    /// Navigate the active tab's WebView to `url`.
    ///
    /// `nav_id` is a monotonically-increasing session token minted by
    /// `BrowserState`; the engine tags every async reply with it so that
    /// stale replies are discarded before becoming `EngineEvent`s.
    fn navigate(&mut self, url: &str, nav_id: u64) -> Result<(), Box<dyn std::error::Error>>;

    fn go_back(&mut self)    -> Result<(), Box<dyn std::error::Error>>;
    fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>>;
    fn reload(&mut self)     -> Result<(), Box<dyn std::error::Error>>;

    /// Scroll the active tab's viewport by `delta_y` pixels (positive = down).
    fn scroll(&mut self, delta_y: i32);

    /// Composite the active tab's current page into `fb` at the content rect.
    fn render_into(
        &mut self,
        fb:  &mut Framebuffer,
        x:   u32,
        y:   u32,
        w:   u32,
        h:   u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>>;

    /// Drain queued `(tab_id, event)` pairs produced since the last call.
    /// `tab_id == 0` means "the active tab" — stubs always emit 0.
    fn poll_events(&mut self) -> Vec<(u64, EngineEvent)>;

    fn title(&self)       -> Option<String>;
    fn current_url(&self) -> Option<String>;

    /// The `nav_id` of the most recent `navigate()` for the active tab, or 0.
    fn current_nav_id(&self) -> u64 { 0 }
}
