//! ContentEngine — stable interface every rendering backend must implement.

use crate::framebuffer::Framebuffer;
use crate::permissions::{PermissionDecision, PermissionKind};

#[derive(Debug, Clone, Copy, Default)]
pub struct EnginePerfStats {
    pub live_webviews: usize,
    pub suspended_tabs: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorKind {
    Default,
    Pointer,
    Text,
    Wait,
}

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
    /// WebKit reports whether native back/forward history is available.
    NavStateChanged { can_back: bool, can_forward: bool },
    /// WebKit find-in-page reported a match count for the active query.
    FindMatchCount(u32),
    DownloadStarted { id: u64, filename: String, path: String },
    DownloadProgress { id: u64, received: u64, progress: f64 },
    DownloadFinished { id: u64, path: String },
    DownloadFailed { id: u64, reason: String },
    PermissionPrompt {
        id: u64,
        origin: String,
        kind: PermissionKind,
        nav_id: u64,
    },
    PermissionResolved { id: u64 },
    SitePermissions {
        origin: String,
        decisions: Vec<(PermissionKind, PermissionDecision)>,
        adblock_enabled: bool,
        adblock_allowlisted: bool,
        blocked_count: u64,
    },
    CursorChanged(CursorKind),
    FrameReady { reason: &'static str },
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
    fn zoom_in(&mut self) {}
    fn zoom_out(&mut self) {}
    fn zoom_reset(&mut self) {}
    fn adblock_allow_domain(&mut self, _domain: &str) {}
    fn adblock_remove_allow_domain(&mut self, _domain: &str) {}
    fn find_text(&mut self, _query: &str) {}
    fn find_next(&mut self) {}
    fn find_previous(&mut self) {}
    fn find_clear(&mut self) {}
    fn download_url(&mut self, _url: &str) {}
    fn resolve_permission(&mut self, _id: u64, _allow: bool, _remember: bool) {}
    fn query_site_permissions(&mut self, _origin: &str, _private: bool) {}
    fn set_site_permission(
        &mut self,
        _origin: &str,
        _kind: PermissionKind,
        _decision: PermissionDecision,
        _private: bool,
    ) {}
    fn set_site_adblock_allowlisted(&mut self, _origin: &str, _allowlisted: bool, _private: bool) {}
    fn force_suspend_inactive_tabs(&mut self) {}

    /// Whether the active tab's WebView has native back/forward history.
    /// Returns false on stub engines — shell history is used instead.
    fn can_go_back(&self)    -> bool { false }
    fn can_go_forward(&self) -> bool { false }

    /// Scroll the active tab's viewport by `delta_y` pixels (positive = down).
    fn scroll(&mut self, delta_y: i32);

    /// Send a content-area click to the active tab.
    fn click(&mut self, _x: u32, _y: u32) {}
    fn right_click(&mut self, _x: u32, _y: u32) {}
    fn mouse_move(&mut self, _x: u32, _y: u32) {}
    fn text_input(&mut self, _text: &str) {}
    fn key_press(&mut self, _key: &str) {}

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
    fn perf_stats(&self) -> EnginePerfStats {
        EnginePerfStats::default()
    }

    fn title(&self)       -> Option<String>;
    fn current_url(&self) -> Option<String>;

    /// The `nav_id` of the most recent `navigate()` for the active tab, or 0.
    fn current_nav_id(&self) -> u64 { 0 }
}
