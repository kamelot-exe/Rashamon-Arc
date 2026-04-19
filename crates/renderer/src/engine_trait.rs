//! ContentEngine — stable interface every rendering backend must implement.

use crate::framebuffer::Framebuffer;

/// Events the engine pushes up to the browser shell.
/// Drained once per frame via `ContentEngine::poll_events`.
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
    /// Navigate to an absolute URL.
    ///
    /// `nav_id` is a monotonically-increasing session token minted by
    /// `BrowserState` for every navigation (begin_navigate / go_back /
    /// go_forward / reload).  The engine tags every asynchronous reply with
    /// this token; replies whose token differs from the most recently accepted
    /// one are silently discarded before they become `EngineEvent`s.
    fn navigate(&mut self, url: &str, nav_id: u64) -> Result<(), Box<dyn std::error::Error>>;

    /// The `nav_id` passed to the most recent `navigate()` call, or 0.
    /// The shell uses this to guard event application against stale sessions.
    fn current_nav_id(&self) -> u64 { 0 }

    fn go_back(&mut self)    -> Result<(), Box<dyn std::error::Error>>;
    fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>>;
    fn reload(&mut self)     -> Result<(), Box<dyn std::error::Error>>;

    /// Scroll the viewport by `delta_y` pixels (positive = scroll down).
    fn scroll(&mut self, delta_y: i32);

    /// Composite the current page into `fb` at the given content rectangle.
    fn render_into(
        &mut self,
        fb:  &mut Framebuffer,
        x:   u32,
        y:   u32,
        w:   u32,
        h:   u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>>;

    /// Drain queued events produced since the last call. Call once per frame.
    fn poll_events(&mut self) -> Vec<EngineEvent>;

    fn title(&self)       -> Option<String>;
    fn current_url(&self) -> Option<String>;
}
