//! WebKitGTK content engine — per-tab WebView architecture.
//!
//! ## Architecture
//!
//! `WebKitEngine` (Send) — channel endpoint owned by the renderer layer.
//! `WebKitDriver` (!Send) — holds live GTK objects, pumped from main thread.
//!
//! One `WebView` is created per browser tab and lives inside an `OffscreenWindow`.
//! The driver routes every command to the correct WebView via `tab_id`.
//! Signal closures capture their owning `tab_id` at creation time, so events
//! are naturally routed to the correct tab without any shell-side guard.
//!
//! ## Tab lifecycle
//!
//!   engine.create_tab(tab_id, is_private)   → Cmd::CreateTab → new WebView
//!   engine.close_tab(tab_id)                → Cmd::CloseTab  → drop WebView
//!   engine.set_active_tab(tab_id)           → Cmd::SwitchTab → snapshot
//!   engine.navigate(url, nav_id)            → Cmd::Navigate  → load_uri on active tab
//!
//! ## Snapshots
//!
//! Rendering is snapshot-based: after load-finished (or after scroll, or on
//! tab switch) the driver calls `wv.snapshot()`, converts the Cairo surface to
//! a packed Vec<u8> (ARGB32 little-endian = [B,G,R,A] per pixel), and sends it
//! as `Reply::FrameReady`.  `render_into` blits the latest cached frame.

use crate::engine_trait::{ContentEngine, CursorKind, EngineEvent, EngineFrame, EnginePerfStats};
use crate::framebuffer::{Framebuffer, Pixel};
use crate::permissions::{
    origin_from_url, DecisionSource, PermissionDecision, PermissionKind, PermissionStore,
};
use crate::platform;
use glib::Cast;
use rashamon_net::AdblockEngine;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::{Duration, Instant};

macro_rules! trace {
    ($($arg:tt)*) => {
        if std::env::var_os("RASHAMON_DEBUG").is_some() {
            eprintln!($($arg)*);
        }
    };
}

fn perf_enabled() -> bool {
    std::env::var_os("RASHAMON_PERF").is_some()
        || std::env::args().skip(1).any(|arg| arg == "--perf")
}

static PERF_T0: OnceLock<Instant> = OnceLock::new();
static PERF_FIRST_TAB: AtomicU64 = AtomicU64::new(0);
static PERF_FIRST_FRAME_DONE: AtomicBool = AtomicBool::new(false);
static WEBEXT_AVAILABLE: AtomicBool = AtomicBool::new(false);
static WEBEXT_CONFIGURED: AtomicBool = AtomicBool::new(false);
static WEBEXT_DISABLED: AtomicBool = AtomicBool::new(false);
static WEBEXT_READY: AtomicBool = AtomicBool::new(false);
static WEBEXT_BLOCKED_EVENTS: AtomicU64 = AtomicU64::new(0);
static WEBEXT_RULES_OK: AtomicU64 = AtomicU64::new(0);
static WEBEXT_RULES_ERROR: AtomicU64 = AtomicU64::new(0);
static WEBEXT_PROBE_BLOCKED: AtomicU64 = AtomicU64::new(0);
static WEBEXT_PROBE_CLEAR: AtomicU64 = AtomicU64::new(0);
static ADBLOCK_POLICY_BLOCKS: AtomicU64 = AtomicU64::new(0);
static ADBLOCK_SUBRESOURCE_REWRITES: AtomicU64 = AtomicU64::new(0);

fn perf_mark_global(stage: &'static str) {
    if !perf_enabled() {
        return;
    }
    let t0 = PERF_T0.get_or_init(Instant::now);
    eprintln!("[perf] webkit_stage={} t_ms={}", stage, t0.elapsed().as_millis());
}

fn perf_mark_tab(stage: &'static str, tab_id: u64) {
    if !perf_enabled() {
        return;
    }
    let first = PERF_FIRST_TAB
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            if v == 0 { Some(tab_id) } else { None }
        })
        .unwrap_or_else(|v| v);
    let tracked = if first == 0 { tab_id } else { first };
    if tab_id != tracked {
        return;
    }
    if PERF_FIRST_FRAME_DONE.load(Ordering::Relaxed) && stage != "frame-ready-emitted" {
        return;
    }
    let t0 = PERF_T0.get_or_init(Instant::now);
    eprintln!(
        "[perf] webkit_stage={} t_ms={} tab={}",
        stage,
        t0.elapsed().as_millis(),
        tab_id
    );
    if stage == "frame-ready-emitted" {
        PERF_FIRST_FRAME_DONE.store(true, Ordering::Relaxed);
    }
}

pub fn webext_is_configured_for_test() -> bool {
    WEBEXT_CONFIGURED.load(Ordering::Relaxed)
}

pub fn webext_is_available_for_test() -> bool {
    WEBEXT_AVAILABLE.load(Ordering::Relaxed)
}

pub fn webext_is_disabled_for_test() -> bool {
    WEBEXT_DISABLED.load(Ordering::Relaxed)
}

pub fn webext_ready_for_test() -> bool {
    WEBEXT_READY.load(Ordering::Relaxed)
}

pub fn webext_blocked_events_for_test() -> u64 {
    WEBEXT_BLOCKED_EVENTS.load(Ordering::Relaxed)
}

pub fn webext_rules_ok_for_test() -> u64 {
    WEBEXT_RULES_OK.load(Ordering::Relaxed)
}

pub fn webext_rules_error_for_test() -> u64 {
    WEBEXT_RULES_ERROR.load(Ordering::Relaxed)
}

pub fn webext_probe_blocked_for_test() -> u64 {
    WEBEXT_PROBE_BLOCKED.load(Ordering::Relaxed)
}

pub fn webext_probe_clear_for_test() -> u64 {
    WEBEXT_PROBE_CLEAR.load(Ordering::Relaxed)
}

pub fn adblock_policy_blocks_for_test() -> u64 {
    ADBLOCK_POLICY_BLOCKS.load(Ordering::Relaxed)
}

pub fn adblock_subresource_rewrites_for_test() -> u64 {
    ADBLOCK_SUBRESOURCE_REWRITES.load(Ordering::Relaxed)
}

// ── IPC ───────────────────────────────────────────────────────────────────────

enum Cmd {
    /// Create a new WebView for this tab.  Private tabs get an ephemeral context.
    CreateTab  { tab_id: u64, is_private: bool },
    /// Destroy the WebView and release GTK resources.
    CloseTab   { tab_id: u64 },
    /// Activate tab and request a fresh snapshot (no reload).
    SwitchTab  { tab_id: u64 },
    /// Resize a tab WebView and request a fresh snapshot.
    SetTabViewport { tab_id: u64, width: u32, height: u32 },
    /// Load a URL in the specified tab's WebView.
    Navigate   { tab_id: u64, url: String, nav_id: u64 },
    /// Scroll by delta pixels and re-snapshot the specified tab.
    ScrollBy   { tab_id: u64, delta: i32 },
    /// Dispatch a click in content-area coordinates.
    Click      { tab_id: u64, x: u32, y: u32 },
    RightClick { tab_id: u64, x: u32, y: u32 },
    MouseMove  { tab_id: u64, x: u32, y: u32 },
    TextInput  { tab_id: u64, text: String },
    KeyPress   { tab_id: u64, key: String },
    /// Native WebKit back/forward — no URL re-load.
    GoBack     { tab_id: u64 },
    GoForward  { tab_id: u64 },
    Zoom       { tab_id: u64, step: i32 },
    AdblockAllowDomain { domain: String },
    AdblockRemoveAllowDomain { domain: String },
    FindText { tab_id: u64, query: String },
    FindNext { tab_id: u64 },
    FindPrevious { tab_id: u64 },
    FindClear { tab_id: u64 },
    DownloadUrl { tab_id: u64, url: String },
    ResolvePermission { id: u64, allow: bool, remember: bool },
    QuerySitePermissions { origin: String, private: bool },
    SetSitePermission { origin: String, kind: PermissionKind, decision: PermissionDecision, private: bool },
    SetSiteAdblock { origin: String, allowlisted: bool, private: bool },
    ForceSuspendInactive,
    Shutdown,
}

enum Reply {
    FrameReady {
        tab_id: u64,
        nav_id: u64,
        gen:    u64,
        reason: &'static str,
        pixels: Vec<u8>,
        width:  u32,
        height: u32,
        title:  String,
        url:    String,
    },
    TitleChanged  { tab_id: u64, nav_id: u64, title:  String },
    UrlChanged    { tab_id: u64, nav_id: u64, url:    String },
    ContentHeight { tab_id: u64, nav_id: u64, h:      u32    },
    LoadFailed    { tab_id: u64, nav_id: u64, reason: String },
    /// WebKit reports whether the tab's history stack has back/forward entries.
    NavState      { tab_id: u64, can_back: bool, can_forward: bool },
    FindMatchCount { tab_id: u64, count: u32 },
    DownloadStarted { id: u64, filename: String, path: String },
    DownloadProgress { id: u64, received: u64, progress: f64 },
    DownloadFinished { id: u64, path: String },
    DownloadFailed { id: u64, reason: String },
    PermissionPrompt { id: u64, tab_id: u64, nav_id: u64, origin: String, kind: PermissionKind },
    PermissionResolved { tab_id: u64, id: u64 },
    SitePermissions {
        origin: String,
        decisions: Vec<(PermissionKind, PermissionDecision)>,
        adblock_enabled: bool,
        adblock_allowlisted: bool,
        blocked_count: u64,
    },
    CursorChanged { tab_id: u64, cursor: CursorKind },
    TabSuspended { tab_id: u64 },
    TabWaking { tab_id: u64 },
}

// ── Per-tab engine state ──────────────────────────────────────────────────────

struct CachedFrame { pixels: Vec<u8>, width: u32, height: u32 }

#[derive(Default)]
struct PerTabState {
    cache:            Option<CachedFrame>,
    title:            Option<String>,
    url:              Option<String>,
    expected_nav_id:  u64,
    latest_frame_gen: u64,
    can_back:         bool,
    can_forward:      bool,
}

// ── WebKitEngine (Send) ───────────────────────────────────────────────────────

pub struct WebKitEngine {
    cmd_tx:         mpsc::SyncSender<Cmd>,
    reply_rx:       mpsc::Receiver<Reply>,
    active_tab_id:  u64,
    tab_states:     HashMap<u64, PerTabState>,
    pending_events: Vec<(u64, EngineEvent)>,
    suspended_tab_ids: HashSet<u64>,
}

// ── WebKitDriver (!Send — main thread only) ───────────────────────────────────

struct TabEntry {
    webview:     webkit2gtk::WebView,
    window:      gtk::OffscreenWindow,
    is_private:  bool,
    nav_id_cell: Rc<Cell<u64>>,
    frame_gen:   Rc<Cell<u64>>,
    schedule_gen: Rc<Cell<u64>>,
    alive:       Rc<Cell<bool>>,
    last_active: Instant,
    last_url:    String,
    viewport_w:  u32,
    viewport_h:  u32,
    viewport_w_cell: Rc<Cell<u32>>,
    viewport_h_cell: Rc<Cell<u32>>,
}

struct SuspendedTab {
    is_private: bool,
    url:        String,
    nav_id:     u64,
    viewport_w: u32,
    viewport_h: u32,
}

struct PendingPermission {
    id: u64,
    tab_id: u64,
    nav_id: u64,
    origin: String,
    kind: PermissionKind,
    private: bool,
    request: webkit2gtk::PermissionRequest,
}

#[derive(Default, Clone, Copy)]
struct OriginBlockCounts {
    in_process_policy: u64,
    in_process_subresource: u64,
    webextension_hard: u64,
}

impl OriginBlockCounts {
    fn total(self) -> u64 {
        self.in_process_policy
            .saturating_add(self.in_process_subresource)
            .saturating_add(self.webextension_hard)
    }
}

#[derive(Clone, Copy)]
enum BlockSource {
    InProcessPolicy,
    InProcessSubresource,
    WebExtensionHard,
}

pub struct WebKitDriver {
    cmd_rx:   mpsc::Receiver<Cmd>,
    reply_tx: mpsc::SyncSender<Reply>,
    tabs:     HashMap<u64, TabEntry>,
    suspended_tabs: HashMap<u64, SuspendedTab>,
    active_tab_id: u64,
    adblock:  Rc<RefCell<AdblockEngine>>,
    adblock_allowlist_path: PathBuf,
    permissions: Rc<RefCell<PermissionStore>>,
    pending_permission: Rc<RefCell<Option<PendingPermission>>>,
    permission_seq: Rc<Cell<u64>>,
    normal_context: webkit2gtk::WebContext,
    download_seq: Rc<Cell<u64>>,
    active_downloads: Rc<Cell<u32>>,
    blocked_by_origin: Rc<RefCell<HashMap<String, OriginBlockCounts>>>,
    w:        u32,
    h:        u32,
    suspend_after: Duration,
    suspend_disabled: bool,
}

// ── Construction ──────────────────────────────────────────────────────────────

impl WebKitEngine {
    /// Initialise GTK and create the channel pair.  **Must be called from the
    /// main thread** (GTK requirement).
    pub fn create(content_w: u32, content_h: u32)
        -> Result<(Self, WebKitDriver), Box<dyn std::error::Error>>
    {
        WEBEXT_AVAILABLE.store(false, Ordering::Relaxed);
        WEBEXT_CONFIGURED.store(false, Ordering::Relaxed);
        WEBEXT_DISABLED.store(false, Ordering::Relaxed);
        WEBEXT_READY.store(false, Ordering::Relaxed);
        WEBEXT_BLOCKED_EVENTS.store(0, Ordering::Relaxed);
        WEBEXT_RULES_OK.store(0, Ordering::Relaxed);
        WEBEXT_RULES_ERROR.store(0, Ordering::Relaxed);
        WEBEXT_PROBE_BLOCKED.store(0, Ordering::Relaxed);
        WEBEXT_PROBE_CLEAR.store(0, Ordering::Relaxed);
        ADBLOCK_POLICY_BLOCKS.store(0, Ordering::Relaxed);
        ADBLOCK_SUBRESOURCE_REWRITES.store(0, Ordering::Relaxed);
        perf_mark_global("gtk-init-start");
        if std::env::var_os("GDK_BACKEND").is_none() {
            std::env::set_var("GDK_BACKEND", "x11,wayland");
        }
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
        gtk::init().map_err(|e| format!("GTK init failed: {e}"))?;
        perf_mark_global("gtk-init-done");

        let (cmd_tx, cmd_rx)     = mpsc::sync_channel::<Cmd>(32);
        let (reply_tx, reply_rx) = mpsc::sync_channel::<Reply>(32);
        let download_seq = Rc::new(Cell::new(0));
        let active_downloads = Rc::new(Cell::new(0));
        let pending_permission = Rc::new(RefCell::new(None));
        let permission_seq = Rc::new(Cell::new(0));
        let blocked_by_origin = Rc::new(RefCell::new(HashMap::new()));
        let adblock_allowlist_path = rashamon_data_dir().join("adblock_allowlist.json");
        let mut adblock = AdblockEngine::new();
        adblock.load_allowlist_from_path(&adblock_allowlist_path);
        let suspend_after = std::env::var("RASHAMON_SUSPEND_AFTER_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(120));
        let suspend_disabled = std::env::var_os("RASHAMON_DISABLE_TAB_SUSPEND").is_some();
        perf_mark_global("webcontext-create-start");
        let normal_context = create_normal_web_context();
        perf_mark_global("webcontext-create-done");
        connect_downloads_for_context(
            &normal_context,
            reply_tx.clone(),
            Rc::clone(&download_seq),
            Rc::clone(&active_downloads),
        );

        trace!("[webkit] Engine created ({}×{})", content_w, content_h);

        let engine = WebKitEngine {
            cmd_tx,
            reply_rx,
            active_tab_id:  0,
            tab_states:     HashMap::new(),
            pending_events: Vec::new(),
            suspended_tab_ids: HashSet::new(),
        };

        let driver = WebKitDriver {
            cmd_rx,
            reply_tx,
            tabs: HashMap::new(),
            suspended_tabs: HashMap::new(),
            active_tab_id: 0,
            adblock: Rc::new(RefCell::new(adblock)),
            adblock_allowlist_path,
            permissions: Rc::new(RefCell::new(PermissionStore::load_default())),
            pending_permission,
            permission_seq,
            normal_context,
            download_seq,
            active_downloads,
            blocked_by_origin,
            w:    content_w,
            h:    content_h,
            suspend_after,
            suspend_disabled,
        };

        Ok((engine, driver))
    }
}

impl Drop for WebKitEngine {
    fn drop(&mut self) { let _ = self.cmd_tx.try_send(Cmd::Shutdown); }
}

// ── ContentEngine impl ────────────────────────────────────────────────────────

impl ContentEngine for WebKitEngine {
    fn create_tab(&mut self, tab_id: u64, is_private: bool) {
        trace!("[webkit] create_tab {tab_id} private={is_private}");
        self.tab_states.entry(tab_id).or_insert_with(PerTabState::default);
        let _ = self.cmd_tx.try_send(Cmd::CreateTab { tab_id, is_private });
    }

    fn close_tab(&mut self, tab_id: u64) {
        trace!("[webkit] close_tab {tab_id}");
        self.tab_states.remove(&tab_id);
        self.suspended_tab_ids.remove(&tab_id);
        let _ = self.cmd_tx.try_send(Cmd::CloseTab { tab_id });
    }

    fn set_active_tab(&mut self, tab_id: u64) {
        trace!("[webkit] set_active_tab {tab_id}");
        self.active_tab_id = tab_id;
        // Request a fresh snapshot — the WebView already has its page loaded.
        let _ = self.cmd_tx.try_send(Cmd::SwitchTab { tab_id });
    }

    fn set_tab_viewport(&mut self, tab_id: u64, width: u32, height: u32) {
        trace!("[webkit] set_tab_viewport {tab_id} {width}x{height}");
        if let Some(state) = self.tab_states.get_mut(&tab_id) {
            state.cache = None;
        }
        let _ = self.cmd_tx.try_send(Cmd::SetTabViewport {
            tab_id,
            width,
            height,
        });
    }

    fn navigate(&mut self, url: &str, nav_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let tab_id = self.active_tab_id;
        trace!("[webkit] navigate tab={tab_id} nav={nav_id} -> {url}");
        let state = self.tab_states.entry(tab_id).or_insert_with(PerTabState::default);
        state.expected_nav_id = nav_id;
        state.cache           = None;
        state.url             = Some(url.to_string());
        self.pending_events.push((tab_id, EngineEvent::LoadStarted));
        self.cmd_tx.send(Cmd::Navigate { tab_id, url: url.to_string(), nav_id })
            .map_err(|e| format!("webkit cmd channel closed: {e}"))?;
        Ok(())
    }

    fn current_nav_id(&self) -> u64 {
        self.tab_states.get(&self.active_tab_id)
            .map(|s| s.expected_nav_id)
            .unwrap_or(0)
    }

    fn go_back(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::GoBack { tab_id });
        Ok(())
    }

    fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::GoForward { tab_id });
        Ok(())
    }

    fn can_go_back(&self) -> bool {
        self.tab_states.get(&self.active_tab_id)
            .map(|s| s.can_back)
            .unwrap_or(false)
    }

    fn can_go_forward(&self) -> bool {
        self.tab_states.get(&self.active_tab_id)
            .map(|s| s.can_forward)
            .unwrap_or(false)
    }

    fn reload(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(url) = self.tab_states.get(&self.active_tab_id)
            .and_then(|s| s.url.clone())
        {
            let nav_id = self.current_nav_id();
            self.navigate(&url, nav_id)?;
        }
        Ok(())
    }

    fn zoom_in(&mut self) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::Zoom { tab_id, step: 1 });
    }

    fn zoom_out(&mut self) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::Zoom { tab_id, step: -1 });
    }

    fn zoom_reset(&mut self) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::Zoom { tab_id, step: 0 });
    }

    fn click(&mut self, x: u32, y: u32) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::Click { tab_id, x, y });
    }

    fn right_click(&mut self, x: u32, y: u32) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::RightClick { tab_id, x, y });
    }

    fn mouse_move(&mut self, x: u32, y: u32) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::MouseMove { tab_id, x, y });
    }

    fn text_input(&mut self, text: &str) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::TextInput {
            tab_id,
            text: text.to_string(),
        });
    }

    fn key_press(&mut self, key: &str) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::KeyPress {
            tab_id,
            key: key.to_string(),
        });
    }

    fn adblock_allow_domain(&mut self, domain: &str) {
        let _ = self.cmd_tx.try_send(Cmd::AdblockAllowDomain {
            domain: domain.to_string(),
        });
    }

    fn adblock_remove_allow_domain(&mut self, domain: &str) {
        let _ = self.cmd_tx.try_send(Cmd::AdblockRemoveAllowDomain {
            domain: domain.to_string(),
        });
    }

    fn find_text(&mut self, query: &str) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::FindText {
            tab_id,
            query: query.to_string(),
        });
    }

    fn find_next(&mut self) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::FindNext { tab_id });
    }

    fn find_previous(&mut self) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::FindPrevious { tab_id });
    }

    fn find_clear(&mut self) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::FindClear { tab_id });
    }

    fn download_url(&mut self, url: &str) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::DownloadUrl {
            tab_id,
            url: url.to_string(),
        });
    }

    fn resolve_permission(&mut self, id: u64, allow: bool, remember: bool) {
        let _ = self.cmd_tx.try_send(Cmd::ResolvePermission { id, allow, remember });
    }

    fn query_site_permissions(&mut self, origin: &str, private: bool) {
        let _ = self.cmd_tx.try_send(Cmd::QuerySitePermissions {
            origin: origin.to_string(),
            private,
        });
    }

    fn set_site_permission(
        &mut self,
        origin: &str,
        kind: PermissionKind,
        decision: PermissionDecision,
        private: bool,
    ) {
        let _ = self.cmd_tx.try_send(Cmd::SetSitePermission {
            origin: origin.to_string(),
            kind,
            decision,
            private,
        });
    }

    fn set_site_adblock_allowlisted(&mut self, origin: &str, allowlisted: bool, private: bool) {
        let _ = self.cmd_tx.try_send(Cmd::SetSiteAdblock {
            origin: origin.to_string(),
            allowlisted,
            private,
        });
    }

    fn force_suspend_inactive_tabs(&mut self) {
        let _ = self.cmd_tx.try_send(Cmd::ForceSuspendInactive);
    }

    fn scroll(&mut self, delta_y: i32) {
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::ScrollBy { tab_id, delta: delta_y });
    }

    fn render_into(
        &mut self,
        fb:  &mut Framebuffer,
        x:   u32, y: u32, w: u32, h: u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>> {
        self.render_tab_into(self.active_tab_id, fb, x, y, w, h)
    }

    fn render_tab_into(
        &mut self,
        tab_id: u64,
        fb:     &mut Framebuffer,
        x:      u32,
        y:      u32,
        w:      u32,
        h:      u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>> {
        let Some(state) = self.tab_states.get(&tab_id) else {
            return Ok(EngineFrame::NotReady);
        };
        let Some(cache) = &state.cache else { return Ok(EngineFrame::NotReady); };
        if cache.width != w || cache.height != h {
            return Ok(EngineFrame::NotReady);
        }

        let src_w = cache.width;
        let src_h = cache.height;
        let rows  = h.min(src_h);
        let cols  = w.min(src_w);

        // Cairo ARGB32 little-endian: memory = [B, G, R, A] per pixel.
        for row in 0..rows {
            for col in 0..cols {
                let s = ((row * src_w) + col) as usize * 4;
                if s + 2 < cache.pixels.len() {
                    let b = cache.pixels[s];
                    let g = cache.pixels[s + 1];
                    let r = cache.pixels[s + 2];
                    fb.set_pixel(x + col, y + row, Pixel { r, g, b });
                }
            }
        }
        Ok(EngineFrame::Ready)
    }

    fn poll_events(&mut self) -> Vec<(u64, EngineEvent)> {
        loop {
            match self.reply_rx.try_recv() {
                Ok(Reply::FrameReady { tab_id, nav_id, gen, reason, pixels, width, height, title, url }) => {
                    let Some(state) = self.tab_states.get_mut(&tab_id) else {
                        trace!("[webkit] drop frame for closed tab={tab_id} gen={gen} reason={reason}");
                        continue;
                    };
                    // Allow nav_id == 0 for switch-triggered snapshots (no active nav).
                    if nav_id != 0 && state.expected_nav_id != 0
                        && nav_id != state.expected_nav_id
                    {
                        trace!("[webkit] drop stale FrameReady tab={tab_id} nav={nav_id} (expected {})",
                            state.expected_nav_id);
                        continue;
                    }
                    if gen < state.latest_frame_gen {
                        trace!("[webkit] drop old FrameReady tab={tab_id} gen={gen} latest={} reason={reason}",
                            state.latest_frame_gen);
                        continue;
                    }
                    state.latest_frame_gen = gen;
                    trace!("[webkit] FrameReady tab={tab_id} gen={gen} reason={reason} {}x{} ({} bytes)",
                        width, height, pixels.len());
                    state.cache = Some(CachedFrame { pixels, width, height });
                    state.title = Some(title.clone());
                    state.url   = Some(url.clone());
                    self.pending_events.push((tab_id, EngineEvent::TitleChanged(title)));
                    self.pending_events.push((tab_id, EngineEvent::UrlChanged(url)));
                    self.pending_events.push((tab_id, EngineEvent::LoadComplete));
                    self.pending_events.push((tab_id, EngineEvent::FrameReady { reason }));
                }
                Ok(Reply::TitleChanged { tab_id, nav_id, title }) => {
                    let state = self.tab_states.entry(tab_id).or_insert_with(PerTabState::default);
                    if nav_id != 0 && state.expected_nav_id != 0
                        && nav_id != state.expected_nav_id
                    { continue; }
                    state.title = Some(title.clone());
                    self.pending_events.push((tab_id, EngineEvent::TitleChanged(title)));
                }
                Ok(Reply::UrlChanged { tab_id, nav_id, url }) => {
                    let state = self.tab_states.entry(tab_id).or_insert_with(PerTabState::default);
                    if nav_id != 0 && state.expected_nav_id != 0
                        && nav_id != state.expected_nav_id
                    { continue; }
                    state.url = Some(url.clone());
                    self.pending_events.push((tab_id, EngineEvent::UrlChanged(url)));
                }
                Ok(Reply::ContentHeight { tab_id, nav_id, h }) => {
                    let state = self.tab_states.entry(tab_id).or_insert_with(PerTabState::default);
                    if nav_id != 0 && state.expected_nav_id != 0
                        && nav_id != state.expected_nav_id
                    { continue; }
                    self.pending_events.push((tab_id, EngineEvent::ContentHeightChanged(h)));
                }
                Ok(Reply::LoadFailed { tab_id, nav_id, reason }) => {
                    let state = self.tab_states.entry(tab_id).or_insert_with(PerTabState::default);
                    if nav_id != 0 && state.expected_nav_id != 0
                        && nav_id != state.expected_nav_id
                    {
                        trace!("[webkit] drop stale LoadFailed tab={tab_id} nav={nav_id}");
                        continue;
                    }
                    trace!("[webkit] LoadFailed tab={tab_id}: {reason}");
                    self.pending_events.push((tab_id, EngineEvent::LoadFailed(reason)));
                }
                Ok(Reply::NavState { tab_id, can_back, can_forward }) => {
                    let state = self.tab_states.entry(tab_id).or_insert_with(PerTabState::default);
                    state.can_back    = can_back;
                    state.can_forward = can_forward;
                    self.pending_events.push((tab_id,
                        EngineEvent::NavStateChanged { can_back, can_forward }));
                }
                Ok(Reply::FindMatchCount { tab_id, count }) => {
                    self.pending_events.push((tab_id, EngineEvent::FindMatchCount(count)));
                }
                Ok(Reply::DownloadStarted { id, filename, path }) => {
                    self.pending_events.push((0, EngineEvent::DownloadStarted { id, filename, path }));
                }
                Ok(Reply::DownloadProgress { id, received, progress }) => {
                    self.pending_events.push((0, EngineEvent::DownloadProgress { id, received, progress }));
                }
                Ok(Reply::DownloadFinished { id, path }) => {
                    self.pending_events.push((0, EngineEvent::DownloadFinished { id, path }));
                }
                Ok(Reply::DownloadFailed { id, reason }) => {
                    self.pending_events.push((0, EngineEvent::DownloadFailed { id, reason }));
                }
                Ok(Reply::PermissionPrompt { id, tab_id, nav_id, origin, kind }) => {
                    self.pending_events.push((tab_id, EngineEvent::PermissionPrompt {
                        id,
                        origin,
                        kind,
                        nav_id,
                    }));
                }
                Ok(Reply::PermissionResolved { tab_id, id }) => {
                    self.pending_events.push((tab_id, EngineEvent::PermissionResolved { id }));
                }
                Ok(Reply::SitePermissions {
                    origin,
                    decisions,
                    adblock_enabled,
                    adblock_allowlisted,
                    blocked_count,
                }) => {
                    self.pending_events.push((0, EngineEvent::SitePermissions {
                        origin,
                        decisions,
                        adblock_enabled,
                        adblock_allowlisted,
                        blocked_count,
                    }));
                }
                Ok(Reply::CursorChanged { tab_id, cursor }) => {
                    self.pending_events.push((tab_id, EngineEvent::CursorChanged(cursor)));
                }
                Ok(Reply::TabSuspended { tab_id }) => {
                    self.suspended_tab_ids.insert(tab_id);
                    trace!("[webkit] tab suspended event tab={tab_id}");
                }
                Ok(Reply::TabWaking { tab_id }) => {
                    self.suspended_tab_ids.remove(&tab_id);
                    trace!("[webkit] tab waking event tab={tab_id}");
                    self.pending_events.push((tab_id, EngineEvent::LoadStarted));
                }
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
            }
        }
        std::mem::take(&mut self.pending_events)
    }

    fn title(&self) -> Option<String> {
        self.tab_states.get(&self.active_tab_id)?.title.clone()
    }
    fn current_url(&self) -> Option<String> {
        self.tab_states.get(&self.active_tab_id)?.url.clone()
    }

    fn perf_stats(&self) -> EnginePerfStats {
        let suspended_tabs = self.suspended_tab_ids.len();
        let live_webviews = self.tab_states.len().saturating_sub(suspended_tabs);
        EnginePerfStats {
            live_webviews,
            suspended_tabs,
        }
    }
}

// ── WebKitDriver ──────────────────────────────────────────────────────────────

impl WebKitDriver {
    /// Pump GTK events and dispatch queued commands.
    /// **Must be called from the main thread every frame.**
    pub fn pump(&mut self) {
        // Process all pending GLib/GTK events without blocking.
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }

        // Dispatch commands.
        loop {
            match self.cmd_rx.try_recv() {
                Ok(Cmd::CreateTab { tab_id, is_private }) => {
                    if self.tabs.contains_key(&tab_id) || self.suspended_tabs.contains_key(&tab_id) { continue; }
                    let entry = make_tab_entry(
                        tab_id, is_private, self.w, self.h, self.reply_tx.clone(),
                        Rc::clone(&self.adblock),
                        Rc::clone(&self.blocked_by_origin),
                        Rc::clone(&self.permissions),
                        Rc::clone(&self.pending_permission),
                        Rc::clone(&self.permission_seq),
                        &self.normal_context,
                        Rc::clone(&self.download_seq),
                        Rc::clone(&self.active_downloads),
                    );
                    self.tabs.insert(tab_id, entry);
                    trace!("[webkit-driver] created WebView for tab {tab_id}");
                }

                Ok(Cmd::CloseTab { tab_id }) => {
                    deny_pending_permission_for_tab(
                        &self.pending_permission,
                        tab_id,
                        &self.reply_tx,
                        "tab-close",
                    );
                    if let Some(entry) = self.tabs.remove(&tab_id) {
                        use webkit2gtk::WebViewExt;
                        entry.alive.set(false);
                        next_cell_generation(&entry.schedule_gen);
                        next_cell_generation(&entry.frame_gen);
                        entry.webview.stop_loading();
                    }
                    self.suspended_tabs.remove(&tab_id);
                    trace!("[webkit-driver] dropped WebView for tab {tab_id}");
                }

                Ok(Cmd::SetTabViewport { tab_id, width, height }) => {
                    if !self.tabs.contains_key(&tab_id) {
                        if let Some(suspended) = self.suspended_tabs.get_mut(&tab_id) {
                            suspended.viewport_w = width;
                            suspended.viewport_h = height;
                        }
                        self.resume_tab(tab_id);
                    }
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        resize_tab_entry(entry, width, height);
                        if entry.last_url.is_empty() {
                            trace!("[webkit-driver] SetTabViewport {tab_id} skip snapshot reason=no-url");
                            continue;
                        }
                        let token = next_cell_generation(&entry.schedule_gen);
                        schedule_snapshot(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                            Rc::clone(&entry.alive), token, "viewport-resize",
                            Duration::from_millis(80),
                        );
                        schedule_snapshot(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                            Rc::clone(&entry.alive), token, "viewport-resize-settle",
                            Duration::from_millis(180),
                        );
                    }
                }

                Ok(Cmd::SwitchTab { tab_id }) => {
                    self.active_tab_id = tab_id;
                    if !self.tabs.contains_key(&tab_id) {
                        self.resume_tab(tab_id);
                    }
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        entry.last_active = Instant::now();
                        let nav_id = entry.nav_id_cell.get();
                        trace!("[webkit-driver] SwitchTab {tab_id} -> snapshot nav={nav_id}");
                        send_view_state(&entry.webview, tab_id, 0, &self.reply_tx);
                        if entry.last_url.is_empty() {
                            trace!("[webkit-driver] SwitchTab {tab_id} skip snapshot reason=no-url");
                            continue;
                        }
                        let token = next_cell_generation(&entry.schedule_gen);
                        request_snapshot_now(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.alive),
                            "tab-switch",
                        );
                        schedule_snapshot(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                            Rc::clone(&entry.alive), token, "tab-switch-settle",
                            Duration::from_millis(90),
                        );
                    }
                }

                Ok(Cmd::Navigate { tab_id, url, nav_id }) => {
                    if !self.tabs.contains_key(&tab_id) {
                        self.resume_tab(tab_id);
                    }
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        use webkit2gtk::WebViewExt;
                        trace!("[webkit-driver] Navigate tab={tab_id} nav={nav_id}: {url}");
                        perf_mark_tab("first-navigate-call", tab_id);
                        deny_pending_permission_for_tab(
                            &self.pending_permission,
                            tab_id,
                            &self.reply_tx,
                            "navigation",
                        );
                        // Update shared cell BEFORE load_uri so synchronous signals
                        // fire with the correct nav_id.
                        entry.nav_id_cell.set(nav_id);
                        entry.last_active = Instant::now();
                        entry.last_url = url.clone();
                        next_cell_generation(&entry.schedule_gen);
                        next_cell_generation(&entry.frame_gen);
                        if let Some(reason) = adblock_block_reason(
                            &self.adblock, &url, &wv_url(&entry.webview), entry.is_private,
                        ) {
                            log_adblock_block_with_kind(&url, &reason, "navigation-preload");
                            increment_origin_block_count(
                                &self.blocked_by_origin,
                                &wv_url(&entry.webview),
                                BlockSource::InProcessPolicy,
                            );
                            let _ = self.reply_tx.try_send(Reply::LoadFailed {
                                tab_id,
                                nav_id,
                                reason: format!("Blocked by adblock ({reason})"),
                            });
                            continue;
                        }
                        entry.webview.load_uri(&url);
                    } else {
                        trace!("[webkit-driver] Navigate for unknown tab {tab_id}");
                    }
                }

                Ok(Cmd::ScrollBy { tab_id, delta }) => {
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        entry.last_active = Instant::now();
                        use webkit2gtk::WebViewExt;
                        let script = format!("window.scrollBy(0, {delta})");
                        #[allow(deprecated)]
                        entry.webview.run_javascript(
                            &script, None::<&gio::Cancellable>, |_| {},
                        );
                        let token = next_cell_generation(&entry.schedule_gen);
                        schedule_snapshot(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                            Rc::clone(&entry.alive), token, "scroll-fast",
                            Duration::from_millis(16),
                        );
                        schedule_snapshot(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                            Rc::clone(&entry.alive), token, "scroll-settle",
                            Duration::from_millis(72),
                        );
                    }
                }

                Ok(Cmd::Click { tab_id, x, y }) => {
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        entry.last_active = Instant::now();
                        use webkit2gtk::WebViewExt;
                        let script = format!(
                            r#"(function() {{
                                var x = {x};
                                var y = {y};
                                var el = document.elementFromPoint(x, y);
                                if (!el) return false;
                                var down = {{ view: window, bubbles: true, cancelable: true,
                                    clientX: x, clientY: y, button: 0, buttons: 1 }};
                                var up = {{ view: window, bubbles: true, cancelable: true,
                                    clientX: x, clientY: y, button: 0, buttons: 0 }};
                                el.dispatchEvent(new MouseEvent('mousemove', down));
                                el.dispatchEvent(new MouseEvent('mousedown', down));
                                if (typeof el.focus === 'function') {{
                                    try {{ el.focus({{ preventScroll: true }}); }} catch (_) {{ el.focus(); }}
                                }}
                                el.dispatchEvent(new MouseEvent('mouseup', up));
                                el.dispatchEvent(new MouseEvent('click', up));
                                return true;
                            }})()"#
                        );
                        #[allow(deprecated)]
                        entry.webview.run_javascript(
                            &script, None::<&gio::Cancellable>, |_| {},
                        );
                        let token = next_cell_generation(&entry.schedule_gen);
                        schedule_view_state_sync(
                            &entry.webview, tab_id, self.reply_tx.clone(),
                            Rc::clone(&entry.nav_id_cell), Rc::clone(&entry.alive),
                            "click-state", Duration::from_millis(80),
                        );
                        schedule_snapshot(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                            Rc::clone(&entry.alive), token, "click-fast",
                            Duration::from_millis(80),
                        );
                        schedule_snapshot(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                            Rc::clone(&entry.alive), token, "click-settle",
                            Duration::from_millis(220),
                        );
                    }
                }

                Ok(Cmd::RightClick { tab_id, x, y }) => {
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        entry.last_active = Instant::now();
                        use webkit2gtk::WebViewExt;
                        let script = format!(
                            r#"(function() {{
                                var x = {x};
                                var y = {y};
                                var el = document.elementFromPoint(x, y);
                                if (!el) return false;
                                var ev = {{ view: window, bubbles: true, cancelable: true,
                                    clientX: x, clientY: y, button: 2, buttons: 2 }};
                                el.dispatchEvent(new MouseEvent('mousedown', ev));
                                el.dispatchEvent(new MouseEvent('contextmenu', ev));
                                el.dispatchEvent(new MouseEvent('mouseup',
                                    {{ view: window, bubbles: true, cancelable: true,
                                       clientX: x, clientY: y, button: 2, buttons: 0 }}));
                                return true;
                            }})()"#
                        );
                        #[allow(deprecated)]
                        entry.webview.run_javascript(
                            &script, None::<&gio::Cancellable>, |_| {},
                        );
                    }
                }

                Ok(Cmd::MouseMove { tab_id, x, y }) => {
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        entry.last_active = Instant::now();
                        use webkit2gtk::WebViewExt;
                        let script = format!(
                            r#"(function() {{
                                var x = {x};
                                var y = {y};
                                var el = document.elementFromPoint(x, y);
                                if (!el) return 'default';
                                el.dispatchEvent(new MouseEvent('mousemove',
                                    {{ view: window, bubbles: true, cancelable: true,
                                       clientX: x, clientY: y, button: 0, buttons: 0 }}));
                                var clickable = el.closest &&
                                    el.closest('a,button,[role="button"],input[type=button],input[type=submit],summary,label');
                                var editable = el.closest &&
                                    el.closest('input:not([type=button]):not([type=submit]):not([type=checkbox]):not([type=radio]),textarea,[contenteditable=true]');
                                var cursor = window.getComputedStyle(el).cursor || '';
                                if (editable || cursor === 'text') return 'text';
                                if (clickable || cursor === 'pointer') return 'pointer';
                                if (cursor === 'wait' || cursor === 'progress') return 'wait';
                                return 'default';
                            }})()"#
                        );
                        let tx = self.reply_tx.clone();
                        #[allow(deprecated)]
                        entry.webview.run_javascript(
                            &script, None::<&gio::Cancellable>, move |result| {
                                let cursor = result
                                    .ok()
                                    .and_then(|js| js.js_value())
                                    .map(|value| cursor_from_js(&value.to_string()))
                                    .unwrap_or(CursorKind::Default);
                                let _ = tx.try_send(Reply::CursorChanged { tab_id, cursor });
                            },
                        );
                    }
                }

                Ok(Cmd::TextInput { tab_id, text }) => {
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        entry.last_active = Instant::now();
                        use webkit2gtk::WebViewExt;
                        let text = js_string_literal(&text);
                        let script = format!(
                            r#"(function(text) {{
                                var el = document.activeElement;
                                if (!el) return false;
                                var tag = (el.tagName || '').toLowerCase();
                                var editable = el.isContentEditable || tag === 'textarea' ||
                                    (tag === 'input' && !/^(button|submit|checkbox|radio|file|reset|image)$/i.test(el.type || ''));
                                if (!editable) return false;
                                el.dispatchEvent(new InputEvent('beforeinput',
                                    {{ bubbles: true, cancelable: true, inputType: 'insertText', data: text }}));
                                if (tag === 'textarea' || tag === 'input') {{
                                    var start = el.selectionStart == null ? el.value.length : el.selectionStart;
                                    var end = el.selectionEnd == null ? el.value.length : el.selectionEnd;
                                    el.value = el.value.slice(0, start) + text + el.value.slice(end);
                                    el.selectionStart = el.selectionEnd = start + text.length;
                                }} else {{
                                    document.execCommand('insertText', false, text);
                                }}
                                el.dispatchEvent(new InputEvent('input',
                                    {{ bubbles: true, cancelable: false, inputType: 'insertText', data: text }}));
                                return true;
                            }})({text})"#
                        );
                        #[allow(deprecated)]
                        entry.webview.run_javascript(
                            &script, None::<&gio::Cancellable>, |_| {},
                        );
                        let token = next_cell_generation(&entry.schedule_gen);
                        schedule_snapshot(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                            Rc::clone(&entry.alive), token, "text-input",
                            Duration::from_millis(40),
                        );
                    }
                }

                Ok(Cmd::KeyPress { tab_id, key }) => {
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        entry.last_active = Instant::now();
                        use webkit2gtk::WebViewExt;
                        let key = js_string_literal(&key);
                        let script = format!(
                            r#"(function(key) {{
                                var el = document.activeElement || document.body;
                                var init = {{ key: key, bubbles: true, cancelable: true }};
                                var cancelled = !el.dispatchEvent(new KeyboardEvent('keydown', init));
                                if (!cancelled) {{
                                    var tag = (el.tagName || '').toLowerCase();
                                    var editable = el && (el.isContentEditable || tag === 'textarea' ||
                                        (tag === 'input' && !/^(button|submit|checkbox|radio|file|reset|image)$/i.test(el.type || '')));
                                    if (editable && (tag === 'textarea' || tag === 'input')) {{
                                        var start = el.selectionStart == null ? el.value.length : el.selectionStart;
                                        var end = el.selectionEnd == null ? el.value.length : el.selectionEnd;
                                        if (key === 'Backspace' && (start > 0 || end > start)) {{
                                            var delStart = end > start ? start : start - 1;
                                            el.value = el.value.slice(0, delStart) + el.value.slice(end);
                                            el.selectionStart = el.selectionEnd = delStart;
                                            el.dispatchEvent(new InputEvent('input',
                                                {{ bubbles: true, inputType: 'deleteContentBackward', data: null }}));
                                        }} else if (key === 'Enter' && tag === 'textarea') {{
                                            el.value = el.value.slice(0, start) + '\n' + el.value.slice(end);
                                            el.selectionStart = el.selectionEnd = start + 1;
                                            el.dispatchEvent(new InputEvent('input',
                                                {{ bubbles: true, inputType: 'insertLineBreak', data: null }}));
                                        }}
                                    }}
                                }}
                                el.dispatchEvent(new KeyboardEvent('keyup', init));
                                return true;
                            }})({key})"#
                        );
                        #[allow(deprecated)]
                        entry.webview.run_javascript(
                            &script, None::<&gio::Cancellable>, |_| {},
                        );
                        let token = next_cell_generation(&entry.schedule_gen);
                        schedule_snapshot(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                            Rc::clone(&entry.alive), token, "key-input",
                            Duration::from_millis(40),
                        );
                    }
                }

                Ok(Cmd::GoBack { tab_id }) => {
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        entry.last_active = Instant::now();
                        use webkit2gtk::WebViewExt;
                        if entry.webview.can_go_back() {
                            // nav_id 0 means "native navigation, no shell nav_id"
                            entry.nav_id_cell.set(0);
                            entry.webview.go_back();
                            schedule_view_state_sync(
                                &entry.webview, tab_id, self.reply_tx.clone(),
                                Rc::clone(&entry.nav_id_cell), Rc::clone(&entry.alive),
                                "back-state-fast", Duration::from_millis(40),
                            );
                            schedule_view_state_sync(
                                &entry.webview, tab_id, self.reply_tx.clone(),
                                Rc::clone(&entry.nav_id_cell), Rc::clone(&entry.alive),
                                "back-state-settle", Duration::from_millis(140),
                            );
                            // load-changed will fire and snapshot when done.
                            // Safety-net: snapshot after a delay in case it was
                            // a same-page (fragment) navigation.
                            let token = next_cell_generation(&entry.schedule_gen);
                            schedule_snapshot(
                                &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                                self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                                Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                                Rc::clone(&entry.alive), token, "back-forward-settle",
                                Duration::from_millis(220),
                            );
                        }
                    }
                }

                Ok(Cmd::GoForward { tab_id }) => {
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        entry.last_active = Instant::now();
                        use webkit2gtk::WebViewExt;
                        if entry.webview.can_go_forward() {
                            entry.nav_id_cell.set(0);
                            entry.webview.go_forward();
                            schedule_view_state_sync(
                                &entry.webview, tab_id, self.reply_tx.clone(),
                                Rc::clone(&entry.nav_id_cell), Rc::clone(&entry.alive),
                                "forward-state-fast", Duration::from_millis(40),
                            );
                            schedule_view_state_sync(
                                &entry.webview, tab_id, self.reply_tx.clone(),
                                Rc::clone(&entry.nav_id_cell), Rc::clone(&entry.alive),
                                "forward-state-settle", Duration::from_millis(140),
                            );
                            let token = next_cell_generation(&entry.schedule_gen);
                            schedule_snapshot(
                                &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                                self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                                Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                                Rc::clone(&entry.alive), token, "back-forward-settle",
                                Duration::from_millis(220),
                            );
                        }
                    }
                }

                Ok(Cmd::Zoom { tab_id, step }) => {
                    if let Some(entry) = self.tabs.get_mut(&tab_id) {
                        entry.last_active = Instant::now();
                        use webkit2gtk::WebViewExt;
                        let next = if step == 0 {
                            1.0
                        } else {
                            (entry.webview.zoom_level() + step as f64 * 0.1).clamp(0.5, 2.0)
                        };
                        entry.webview.set_zoom_level(next);
                        let token = next_cell_generation(&entry.schedule_gen);
                        schedule_snapshot(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                            Rc::clone(&entry.alive), token, "zoom-fast",
                            Duration::from_millis(24),
                        );
                        schedule_snapshot(
                            &entry.webview, entry.viewport_w, entry.viewport_h, tab_id,
                            self.reply_tx.clone(), Rc::clone(&entry.nav_id_cell),
                            Rc::clone(&entry.frame_gen), Rc::clone(&entry.schedule_gen),
                            Rc::clone(&entry.alive), token, "zoom-settle",
                            Duration::from_millis(90),
                        );
                    }
                }

                Ok(Cmd::AdblockAllowDomain { domain }) => {
                    self.adblock.borrow_mut().allowlist_domain(&domain);
                    self.adblock.borrow().save_allowlist_to_path(&self.adblock_allowlist_path);
                    self.sync_webext_rules_to_live_tabs();
                    trace!("[adblock] allowlisted {domain}");
                }

                Ok(Cmd::AdblockRemoveAllowDomain { domain }) => {
                    self.adblock.borrow_mut().remove_allowlist_domain(&domain);
                    self.adblock.borrow().save_allowlist_to_path(&self.adblock_allowlist_path);
                    self.sync_webext_rules_to_live_tabs();
                    trace!("[adblock] removed allowlist {domain}");
                }

                Ok(Cmd::FindText { tab_id, query }) => {
                    if let Some(entry) = self.tabs.get(&tab_id) {
                        webkit_find_text(entry, &query);
                    }
                }

                Ok(Cmd::FindNext { tab_id }) => {
                    if let Some(entry) = self.tabs.get(&tab_id) {
                        webkit_find_next(entry);
                    }
                }

                Ok(Cmd::FindPrevious { tab_id }) => {
                    if let Some(entry) = self.tabs.get(&tab_id) {
                        webkit_find_previous(entry);
                    }
                }

                Ok(Cmd::FindClear { tab_id }) => {
                    if let Some(entry) = self.tabs.get(&tab_id) {
                        webkit_find_clear(entry);
                    }
                }

                Ok(Cmd::DownloadUrl { tab_id, url }) => {
                    if let Some(entry) = self.tabs.get(&tab_id) {
                        use webkit2gtk::WebViewExt;
                        trace!("[webkit-download] explicit download tab={tab_id}: {url}");
                        if entry.webview.download_uri(&url).is_none() {
                            let _ = self.reply_tx.try_send(Reply::DownloadFailed {
                                id: 0,
                                reason: format!("Could not start download: {url}"),
                            });
                        }
                    }
                }

                Ok(Cmd::ResolvePermission { id, allow, remember }) => {
                    resolve_pending_permission(
                        &self.pending_permission,
                        &self.permissions,
                        &self.tabs,
                        &self.reply_tx,
                        id,
                        allow,
                        remember,
                    );
                }

                Ok(Cmd::QuerySitePermissions { origin, private }) => {
                    send_site_info(
                        &self.reply_tx,
                        &self.permissions,
                        &self.adblock,
                        &self.blocked_by_origin,
                        &origin,
                        private,
                    );
                }

                Ok(Cmd::SetSitePermission { origin, kind, decision, private }) => {
                    self.permissions.borrow_mut().set(&origin, kind, decision, private);
                    trace!(
                        "[permissions] site-panel set origin={} kind={} decision={} private={}",
                        origin,
                        kind.as_str(),
                        decision.as_str(),
                        private,
                    );
                    send_site_info(
                        &self.reply_tx,
                        &self.permissions,
                        &self.adblock,
                        &self.blocked_by_origin,
                        &origin,
                        private,
                    );
                }

                Ok(Cmd::SetSiteAdblock { origin, allowlisted, private }) => {
                    {
                        let mut adblock = self.adblock.borrow_mut();
                        if allowlisted {
                            adblock.allowlist_domain_for_context(&origin, private);
                        } else {
                            adblock.remove_allowlist_domain_for_context(&origin, private);
                        }
                    }
                    if !private {
                        self.adblock
                            .borrow()
                            .save_allowlist_to_path(&self.adblock_allowlist_path);
                    }
                    trace!(
                        "[adblock] site-panel origin={} allowlisted={} private={}",
                        origin,
                        allowlisted,
                        private,
                    );
                    self.sync_webext_rules_to_live_tabs();
                    send_site_info(
                        &self.reply_tx,
                        &self.permissions,
                        &self.adblock,
                        &self.blocked_by_origin,
                        &origin,
                        private,
                    );
                }

                Ok(Cmd::ForceSuspendInactive) => {
                    self.suspend_inactive_tabs(true);
                }

                Ok(Cmd::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }

        self.suspend_inactive_tabs(false);
    }

    fn sync_webext_rules_to_live_tabs(&self) {
        for (tab_id, entry) in &self.tabs {
            send_webext_rules(
                *tab_id,
                &entry.webview,
                &self.adblock,
                entry.is_private,
                true,
                std::env::var_os("RASHAMON_WEBEXT_SMOKE_INVALID_RULES").is_some(),
            );
        }
    }

    fn suspend_inactive_tabs(&mut self, force: bool) {
        if self.suspend_disabled && !force {
            trace!(
                "[webkit-driver] skip suspend reason=disabled live={} suspended={} downloads={}",
                self.tabs.len(),
                self.suspended_tabs.len(),
                self.active_downloads.get(),
            );
            return;
        }
        if self.active_downloads.get() > 0 {
            trace!(
                "[webkit-driver] skip suspend reason=active-downloads live={} suspended={} downloads={}",
                self.tabs.len(),
                self.suspended_tabs.len(),
                self.active_downloads.get(),
            );
            return;
        }
        let now = Instant::now();
        let active = self.active_tab_id;
        let ids = self.tabs
            .iter()
            .filter_map(|(tab_id, entry)| {
                if *tab_id == active {
                    trace!("[webkit-driver] skip suspend tab={tab_id} reason=active");
                    return None;
                }
                if entry.last_url.is_empty() {
                    trace!("[webkit-driver] skip suspend tab={tab_id} reason=no-url");
                    return None;
                }
                if force || now.saturating_duration_since(entry.last_active) >= self.suspend_after {
                    Some(*tab_id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for tab_id in ids {
            self.suspend_tab(tab_id);
        }
    }

    fn suspend_tab(&mut self, tab_id: u64) {
        deny_pending_permission_for_tab(
            &self.pending_permission,
            tab_id,
            &self.reply_tx,
            "tab-suspend",
        );
        let Some(entry) = self.tabs.remove(&tab_id) else { return };
        use webkit2gtk::WebViewExt;
        let url = if entry.last_url.is_empty() {
            wv_url(&entry.webview)
        } else {
            entry.last_url.clone()
        };
        entry.alive.set(false);
        next_cell_generation(&entry.schedule_gen);
        next_cell_generation(&entry.frame_gen);
        entry.webview.stop_loading();
        if !url.is_empty() {
            self.suspended_tabs.insert(tab_id, SuspendedTab {
                is_private: entry.is_private,
                url: url.clone(),
                nav_id: entry.nav_id_cell.get(),
                viewport_w: entry.viewport_w,
                viewport_h: entry.viewport_h,
            });
            trace!(
                "[webkit-driver] suspended tab={} url={} live={} suspended={}",
                tab_id,
                url,
                self.tabs.len(),
                self.suspended_tabs.len(),
            );
            let _ = self.reply_tx.try_send(Reply::TabSuspended { tab_id });
        }
    }

    fn resume_tab(&mut self, tab_id: u64) {
        let Some(suspended) = self.suspended_tabs.remove(&tab_id) else { return };
        let mut entry = make_tab_entry(
            tab_id,
            suspended.is_private,
            suspended.viewport_w,
            suspended.viewport_h,
            self.reply_tx.clone(),
            Rc::clone(&self.adblock),
            Rc::clone(&self.blocked_by_origin),
            Rc::clone(&self.permissions),
            Rc::clone(&self.pending_permission),
            Rc::clone(&self.permission_seq),
            &self.normal_context,
            Rc::clone(&self.download_seq),
            Rc::clone(&self.active_downloads),
        );
        use webkit2gtk::WebViewExt;
        entry.nav_id_cell.set(suspended.nav_id);
        entry.last_url = suspended.url.clone();
        trace!(
            "[webkit-driver] resumed tab={} url={} live={} suspended={}",
            tab_id,
            suspended.url,
            self.tabs.len() + 1,
            self.suspended_tabs.len(),
        );
        entry.webview.load_uri(&suspended.url);
        self.tabs.insert(tab_id, entry);
        let _ = self.reply_tx.try_send(Reply::TabWaking { tab_id });
    }
}

// ── WebView factory ───────────────────────────────────────────────────────────

fn make_tab_entry(
    tab_id:    u64,
    is_private: bool,
    w:         u32,
    h:         u32,
    reply_tx:  mpsc::SyncSender<Reply>,
    adblock:   Rc<RefCell<AdblockEngine>>,
    blocked_by_origin: Rc<RefCell<HashMap<String, OriginBlockCounts>>>,
    permissions: Rc<RefCell<PermissionStore>>,
    pending_permission: Rc<RefCell<Option<PendingPermission>>>,
    permission_seq: Rc<Cell<u64>>,
    normal_context: &webkit2gtk::WebContext,
    download_seq: Rc<Cell<u64>>,
    active_downloads: Rc<Cell<u32>>,
) -> TabEntry {
    use gtk::prelude::{ContainerExt, GtkWindowExt, WidgetExt};
    use webkit2gtk::{
        FindControllerExt, HardwareAccelerationPolicy, LoadEvent, NavigationPolicyDecision,
        NavigationPolicyDecisionExt, PolicyDecisionExt, PolicyDecisionType,
        ResponsePolicyDecision, ResponsePolicyDecisionExt, Settings, SettingsExt,
        URIRequestExt, WebView, WebViewExt,
    };

    let settings = Settings::new();
    settings.set_enable_webgl(false);
    settings.set_javascript_can_access_clipboard(false);
    settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never);

    perf_mark_tab("webview-create-start", tab_id);
    let webview = if is_private {
        use webkit2gtk::WebContext;
        let ctx = WebContext::new_ephemeral();
        connect_downloads_for_context(
            &ctx,
            reply_tx.clone(),
            Rc::clone(&download_seq),
            Rc::clone(&active_downloads),
        );
        let wv  = WebView::with_context(&ctx);
        wv.set_settings(&settings);
        wv
    } else {
        let wv = WebView::with_context(normal_context);
        wv.set_settings(&settings);
        wv
    };
    perf_mark_tab("webview-create-done", tab_id);
    webview.set_size_request(w as i32, h as i32);

    let window = gtk::OffscreenWindow::new();
    window.set_default_size(w as i32, h as i32);
    window.add(&webview);
    window.show_all();

    connect_webext_view_messages(tab_id, &webview, Rc::clone(&blocked_by_origin));
    send_webext_ping_and_rules(tab_id, &webview, &adblock, is_private);

    let nav_id_cell: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    let frame_gen: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    let schedule_gen: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    let alive: Rc<Cell<bool>> = Rc::new(Cell::new(true));
    let viewport_w_cell: Rc<Cell<u32>> = Rc::new(Cell::new(w));
    let viewport_h_cell: Rc<Cell<u32>> = Rc::new(Cell::new(h));

    {
        let permissions = Rc::clone(&permissions);
        let pending_permission = Rc::clone(&pending_permission);
        let permission_seq = Rc::clone(&permission_seq);
        let reply_tx = reply_tx.clone();
        let nav_id_cell = Rc::clone(&nav_id_cell);
        webview.connect_permission_request(move |wv, request| {
            handle_permission_request(
                wv,
                request,
                tab_id,
                nav_id_cell.get(),
                Rc::clone(&permissions),
                Rc::clone(&pending_permission),
                Rc::clone(&permission_seq),
                reply_tx.clone(),
                is_private,
            )
        });
    }

    if let Some(find) = webview.find_controller() {
        let tx = reply_tx.clone();
        find.connect_counted_matches(move |_fc, count| {
            trace!("[webkit-find] counted tab={tab_id} count={count}");
            let _ = tx.try_send(Reply::FindMatchCount { tab_id, count });
        });
        let tx = reply_tx.clone();
        find.connect_failed_to_find_text(move |_fc| {
            trace!("[webkit-find] failed tab={tab_id}");
            let _ = tx.try_send(Reply::FindMatchCount { tab_id, count: 0 });
        });
        let tx = reply_tx.clone();
        find.connect_found_text(move |_fc, count| {
            trace!("[webkit-find] found tab={tab_id} count={count}");
            let _ = tx.try_send(Reply::FindMatchCount { tab_id, count });
        });
    }

    // decide-policy is the WebKitGTK hook available in-process for cancelling
    // navigations before WebKit commits them. Subresource blocking will need a
    // WebKit web extension later; keep v0.2 foundation deliberately small.
    {
        let tx = reply_tx.clone();
        let nc = Rc::clone(&nav_id_cell);
        let ab = Rc::clone(&adblock);
        let blocked_by_origin = Rc::clone(&blocked_by_origin);
        webview.connect_decide_policy(move |wv, decision, decision_type| {
            let uri = match decision_type {
                PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction => {
                    decision
                        .downcast_ref::<NavigationPolicyDecision>()
                        .and_then(|d| d.navigation_action())
                        .and_then(|a| a.request())
                        .and_then(|r| r.uri())
                        .map(|s| s.to_string())
                        .map(|u| (u, "policy-navigation", true))
                }
                PolicyDecisionType::Response => decision
                    .downcast_ref::<ResponsePolicyDecision>()
                    .and_then(|d| d.request())
                    .and_then(|r| r.uri())
                    .map(|s| s.to_string())
                    .map(|u| (u, "policy-response", false)),
                _ => None,
            };
            let Some((uri, kind, emit_load_failed)) = uri else {
                return false;
            };
            if let Some(reason) = adblock_block_reason(&ab, &uri, &wv_url(wv), is_private) {
                log_adblock_block_with_kind(&uri, &reason, kind);
                increment_origin_block_count(
                    &blocked_by_origin,
                    &wv_url(wv),
                    BlockSource::InProcessPolicy,
                );
                decision.ignore();
                if emit_load_failed {
                    let _ = tx.try_send(Reply::LoadFailed {
                        tab_id,
                        nav_id: nc.get(),
                        reason: format!("Blocked by adblock ({reason})"),
                    });
                }
                return true;
            }
            false
        });
    }

    // resource-load-started is the best in-process hook for subresource request
    // interception in this WebKitGTK path. We cannot cancel directly here, so we
    // rewrite blocked requests to a local data URI as a safe no-network fallback.
    {
        let ab = Rc::clone(&adblock);
        let blocked_by_origin = Rc::clone(&blocked_by_origin);
        webview.connect_resource_load_started(move |wv, _resource, request| {
            let Some(uri) = request.uri().map(|u| u.to_string()) else {
                return;
            };
            let page_url = wv_url(wv);
            if uri == page_url {
                return;
            }
            if let Some(reason) = adblock_block_reason(&ab, &uri, &page_url, is_private) {
                log_adblock_block_with_kind(&uri, &reason, "subresource");
                request.set_uri("data:text/plain;charset=utf-8,blocked%20by%20Rashamon%20Arc%20adblock");
                increment_origin_block_count(
                    &blocked_by_origin,
                    &page_url,
                    BlockSource::InProcessSubresource,
                );
            }
        });
    }

    // load-changed: snapshot on Finished, height hint on Committed.
    {
        let tx  = reply_tx.clone();
        let nc  = Rc::clone(&nav_id_cell);
        let fg  = Rc::clone(&frame_gen);
        let sg  = Rc::clone(&schedule_gen);
        let alive = Rc::clone(&alive);
        let vw = Rc::clone(&viewport_w_cell);
        let vh = Rc::clone(&viewport_h_cell);
        webview.connect_load_changed(move |wv, event| {
            if event == LoadEvent::Started {
                perf_mark_tab("load-start", tab_id);
            }
            if event == LoadEvent::Committed {
                perf_mark_tab("load-committed", tab_id);
                let nav_id = nc.get();
                let _ = tx.try_send(Reply::UrlChanged {
                    tab_id, nav_id, url: wv_url(wv),
                });
            }
            if event == LoadEvent::Finished {
                perf_mark_tab("load-finished", tab_id);
                use webkit2gtk::WebViewExt as _;
                let nav_id    = nc.get();
                let can_back  = wv.can_go_back();
                let can_fwd   = wv.can_go_forward();
                let token     = next_cell_generation(&sg);
                let _ = tx.try_send(Reply::ContentHeight { tab_id, nav_id, h: 200_000 });
                let _ = tx.try_send(Reply::NavState {
                    tab_id, can_back, can_forward: can_fwd,
                });
                request_snapshot_now(
                    wv, vw.get(), vh.get(), tab_id, tx.clone(), Rc::clone(&nc),
                    Rc::clone(&fg), Rc::clone(&alive), "load-finished",
                );
                schedule_snapshot(
                    wv, vw.get(), vh.get(), tab_id, tx.clone(), Rc::clone(&nc),
                    Rc::clone(&fg), Rc::clone(&sg), Rc::clone(&alive),
                    token, "spa-settle", Duration::from_millis(180),
                );
                schedule_snapshot(
                    wv, vw.get(), vh.get(), tab_id, tx.clone(), Rc::clone(&nc),
                    Rc::clone(&fg), Rc::clone(&sg), Rc::clone(&alive),
                    token, "spa-late", Duration::from_millis(650),
                );
            }
        });
    }

    // load-failed: suppress WebKit-internal cancellation noise.
    {
        let tx  = reply_tx.clone();
        let nc  = Rc::clone(&nav_id_cell);
        webview.connect_load_failed(move |_wv, _ev, _uri, err| {
            let nav_id = nc.get();
            let msg    = err.to_string();
            let is_cancel = msg.contains("ancelled")
                || msg.contains("policy change")
                || msg.contains("nterrupted");
            if !is_cancel {
                let _ = tx.try_send(Reply::LoadFailed {
                    tab_id, nav_id, reason: msg,
                });
            }
            false
        });
    }

    // title-notify: intermediate title updates.
    {
        let tx  = reply_tx.clone();
        let nc  = Rc::clone(&nav_id_cell);
        webview.connect_title_notify(move |wv| {
            if let Some(t) = wv.title() {
                let nav_id = nc.get();
                let _ = tx.try_send(Reply::TitleChanged {
                    tab_id, nav_id, title: t.to_string(),
                });
            }
        });
    }

    TabEntry {
        webview,
        window,
        is_private,
        nav_id_cell,
        frame_gen,
        schedule_gen,
        alive,
        last_active: Instant::now(),
        last_url: String::new(),
        viewport_w: w,
        viewport_h: h,
        viewport_w_cell,
        viewport_h_cell,
    }
}

fn resize_tab_entry(entry: &mut TabEntry, width: u32, height: u32) {
    let width = width.max(1);
    let height = height.max(1);
    if entry.viewport_w == width && entry.viewport_h == height {
        return;
    }
    use gtk::prelude::{GtkWindowExt, WidgetExt};
    entry.viewport_w = width;
    entry.viewport_h = height;
    entry.viewport_w_cell.set(width);
    entry.viewport_h_cell.set(height);
    next_cell_generation(&entry.schedule_gen);
    next_cell_generation(&entry.frame_gen);
    entry.webview.set_size_request(width as i32, height as i32);
    entry.window.set_default_size(width as i32, height as i32);
    entry.window.resize(width as i32, height as i32);
    trace!("[webkit-driver] resized tab viewport {}x{}", width, height);
}

// ── Permission handling ──────────────────────────────────────────────────────

fn handle_permission_request(
    wv: &webkit2gtk::WebView,
    request: &webkit2gtk::PermissionRequest,
    tab_id: u64,
    nav_id: u64,
    store: Rc<RefCell<PermissionStore>>,
    pending: Rc<RefCell<Option<PendingPermission>>>,
    seq: Rc<Cell<u64>>,
    tx: mpsc::SyncSender<Reply>,
    private: bool,
) -> bool {
    use webkit2gtk::{
        GeolocationPermissionRequest, NotificationPermissionRequest, PermissionRequestExt,
        UserMediaPermissionRequest, UserMediaPermissionRequestExt,
    };

    let Some(origin) = origin_from_url(&wv_url(wv)) else {
        request.deny();
        trace!("[permissions] request origin=<unknown> kind=unknown source=default decision=deny final=deny");
        return true;
    };

    if request.downcast_ref::<NotificationPermissionRequest>().is_some() {
        return decide_single_permission(
            request,
            &store,
            &pending,
            &seq,
            &tx,
            tab_id,
            nav_id,
            private,
            &origin,
            PermissionKind::Notifications,
        );
    }
    if request.downcast_ref::<GeolocationPermissionRequest>().is_some() {
        return decide_single_permission(
            request,
            &store,
            &pending,
            &seq,
            &tx,
            tab_id,
            nav_id,
            private,
            &origin,
            PermissionKind::Geolocation,
        );
    }
    if let Some(media) = request.downcast_ref::<UserMediaPermissionRequest>() {
        let mut kinds = Vec::new();
        if media.is_for_video_device() {
            kinds.push(PermissionKind::Camera);
        }
        if media.is_for_audio_device() {
            kinds.push(PermissionKind::Microphone);
        }
        if kinds.is_empty() {
            request.deny();
            trace!("[permissions] request origin={origin} kind=user-media source=default decision=deny final=deny");
            return true;
        }

        let mut all_allowed = true;
        for kind in kinds {
            let (decision, source) = store.borrow().get(&origin, kind, private);
            if decision == PermissionDecision::Ask {
                return queue_permission_prompt(
                    request, pending, seq, tx, tab_id, nav_id, private, origin, kind,
                );
            }
            let allowed = decision == PermissionDecision::Allow;
            all_allowed &= allowed;
            trace_permission_decision(
                &origin,
                kind,
                decision,
                source,
                if allowed { "allow" } else { "deny" },
            );
        }
        if all_allowed {
            request.allow();
        } else {
            request.deny();
        }
        return true;
    }

    request.deny();
    trace!("[permissions] request origin={origin} kind=unsupported source=default decision=deny final=deny");
    true
}

fn decide_single_permission(
    request: &webkit2gtk::PermissionRequest,
    store: &Rc<RefCell<PermissionStore>>,
    pending: &Rc<RefCell<Option<PendingPermission>>>,
    seq: &Rc<Cell<u64>>,
    tx: &mpsc::SyncSender<Reply>,
    tab_id: u64,
    nav_id: u64,
    private: bool,
    origin: &str,
    kind: PermissionKind,
) -> bool {
    use webkit2gtk::PermissionRequestExt;

    let (decision, source) = store.borrow().get(origin, kind, private);
    match decision {
        PermissionDecision::Allow => {
            request.allow();
            trace_permission_decision(origin, kind, decision, source, "allow");
        }
        PermissionDecision::Deny | PermissionDecision::Ask => {
            if decision == PermissionDecision::Ask {
                return queue_permission_prompt(
                    request, Rc::clone(pending), Rc::clone(seq), tx.clone(),
                    tab_id, nav_id, private, origin.to_string(), kind,
                );
            } else {
                request.deny();
                trace_permission_decision(origin, kind, decision, source, "deny");
            }
        }
    }
    true
}

fn queue_permission_prompt(
    request: &webkit2gtk::PermissionRequest,
    pending: Rc<RefCell<Option<PendingPermission>>>,
    seq: Rc<Cell<u64>>,
    tx: mpsc::SyncSender<Reply>,
    tab_id: u64,
    nav_id: u64,
    private: bool,
    origin: String,
    kind: PermissionKind,
) -> bool {
    use webkit2gtk::PermissionRequestExt;

    if let Some(old) = pending.borrow_mut().take() {
        trace!("[permissions] stale prompt denied id={} reason=replaced", old.id);
        old.request.deny();
        let _ = tx.try_send(Reply::PermissionResolved {
            tab_id: old.tab_id,
            id: old.id,
        });
    }

    let id = seq.get().wrapping_add(1).max(1);
    seq.set(id);
    *pending.borrow_mut() = Some(PendingPermission {
        id,
        tab_id,
        nav_id,
        origin: origin.clone(),
        kind,
        private,
        request: request.clone(),
    });
    trace!(
        "[permissions] prompt id={} tab={} nav={} origin={} kind={}",
        id,
        tab_id,
        nav_id,
        origin,
        kind.as_str(),
    );
    let _ = tx.try_send(Reply::PermissionPrompt { id, tab_id, nav_id, origin, kind });
    true
}

fn resolve_pending_permission(
    pending: &Rc<RefCell<Option<PendingPermission>>>,
    store: &Rc<RefCell<PermissionStore>>,
    tabs: &HashMap<u64, TabEntry>,
    tx: &mpsc::SyncSender<Reply>,
    id: u64,
    allow: bool,
    remember: bool,
) {
    use webkit2gtk::PermissionRequestExt;

    let Some(p) = pending.borrow_mut().take() else {
        trace!("[permissions] resolve ignored id={id} reason=no-pending");
        return;
    };
    if p.id != id {
        trace!("[permissions] resolve ignored id={id} pending={} reason=mismatch", p.id);
        *pending.borrow_mut() = Some(p);
        return;
    }

    let nav_current = tabs
        .get(&p.tab_id)
        .map(|tab| tab.nav_id_cell.get() == p.nav_id)
        .unwrap_or(false);
    if !nav_current {
        trace!("[permissions] stale prompt denied id={} reason=nav-changed", p.id);
        p.request.deny();
    } else if allow {
        if remember || p.private {
            store.borrow_mut().set(&p.origin, p.kind, PermissionDecision::Allow, p.private);
        }
        trace!("[permissions] prompt allowed id={} remember={} private={}", p.id, remember, p.private);
        p.request.allow();
    } else {
        if remember || p.private {
            store.borrow_mut().set(&p.origin, p.kind, PermissionDecision::Deny, p.private);
        }
        trace!("[permissions] prompt denied id={} remember={} private={}", p.id, remember, p.private);
        p.request.deny();
    }
    let _ = tx.try_send(Reply::PermissionResolved { tab_id: p.tab_id, id: p.id });
}

fn deny_pending_permission_for_tab(
    pending: &Rc<RefCell<Option<PendingPermission>>>,
    tab_id: u64,
    tx: &mpsc::SyncSender<Reply>,
    reason: &'static str,
) {
    use webkit2gtk::PermissionRequestExt;

    let should_deny = pending.borrow().as_ref().map_or(false, |p| p.tab_id == tab_id);
    if !should_deny {
        return;
    }
    if let Some(p) = pending.borrow_mut().take() {
        trace!("[permissions] stale prompt denied id={} reason={reason}", p.id);
        p.request.deny();
        let _ = tx.try_send(Reply::PermissionResolved { tab_id: p.tab_id, id: p.id });
    }
}

fn trace_permission_decision(
    origin: &str,
    kind: PermissionKind,
    decision: PermissionDecision,
    source: DecisionSource,
    final_action: &str,
) {
    trace!(
        "[permissions] request origin={} kind={} source={} decision={} final={}",
        origin,
        kind.as_str(),
        decision_source_label(source),
        decision.as_str(),
        final_action,
    );
}

fn decision_source_label(source: DecisionSource) -> &'static str {
    match source {
        DecisionSource::Persisted => "persisted",
        DecisionSource::Session => "session",
        DecisionSource::Default => "default",
    }
}

fn permission_kinds() -> [PermissionKind; 5] {
    [
        PermissionKind::Notifications,
        PermissionKind::Geolocation,
        PermissionKind::Camera,
        PermissionKind::Microphone,
        PermissionKind::Clipboard,
    ]
}

fn send_site_info(
    tx: &mpsc::SyncSender<Reply>,
    permissions: &Rc<RefCell<PermissionStore>>,
    adblock: &Rc<RefCell<AdblockEngine>>,
    blocked_by_origin: &Rc<RefCell<HashMap<String, OriginBlockCounts>>>,
    origin: &str,
    private: bool,
) {
    let decisions = permission_kinds()
        .iter()
        .map(|kind| {
            let (decision, _) = permissions.borrow().get(origin, *kind, private);
            (*kind, decision)
        })
        .collect();
    let adblock_ref = adblock.borrow();
    let adblock_enabled = adblock_ref.is_enabled();
    let adblock_allowlisted = adblock_ref.is_allowlisted_domain(origin, private);
    let blocked_count = blocked_by_origin
        .borrow()
        .get(&origin_key(origin))
        .copied()
        .map(OriginBlockCounts::total)
        .unwrap_or(0);
    let _ = tx.try_send(Reply::SitePermissions {
        origin: origin.to_string(),
        decisions,
        adblock_enabled,
        adblock_allowlisted,
        blocked_count,
    });
}

fn rashamon_data_dir() -> PathBuf {
    platform::default_data_dir()
}

fn create_normal_web_context() -> webkit2gtk::WebContext {
    use webkit2gtk::{WebContext, WebsiteDataManager};

    let profile_name = std::env::var("RASHAMON_PROFILE_SUFFIX")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "normal".to_string());
    let profile_root = rashamon_data_dir().join("webkit-profile").join(profile_name);
    let data_dir = profile_root.join("data");
    let cache_dir = profile_root.join("cache");
    let _ = std::fs::create_dir_all(&data_dir);
    let _ = std::fs::create_dir_all(&cache_dir);

    let manager = WebsiteDataManager::builder()
        .base_data_directory(data_dir.to_string_lossy().as_ref())
        .base_cache_directory(cache_dir.to_string_lossy().as_ref())
        .build();
    let context = WebContext::with_website_data_manager(&manager);
    configure_web_extension(&context);
    trace!(
        "[webkit] profile normal data_dir={} cache_dir={}",
        data_dir.display(),
        cache_dir.display()
    );
    context
}

fn configure_web_extension(context: &webkit2gtk::WebContext) {
    use webkit2gtk::{UserMessageExt, WebContextExt};

    if std::env::var_os("RASHAMON_DISABLE_WEBEXT").is_some() {
        trace!("[adblock-webext] disabled via RASHAMON_DISABLE_WEBEXT=1");
        WEBEXT_DISABLED.store(true, Ordering::Relaxed);
        WEBEXT_AVAILABLE.store(false, Ordering::Relaxed);
        WEBEXT_CONFIGURED.store(false, Ordering::Relaxed);
        return;
    }
    WEBEXT_DISABLED.store(false, Ordering::Relaxed);

    let default_dir = option_env!("RASHAMON_WEBEXT_DIR")
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let ext_dir = std::env::var("RASHAMON_WEBEXT_DIR").ok().or(default_dir);
    let Some(ext_dir) = ext_dir else {
        trace!("[adblock-webext] no extension dir available, fallback to in-process");
        WEBEXT_AVAILABLE.store(false, Ordering::Relaxed);
        WEBEXT_CONFIGURED.store(false, Ordering::Relaxed);
        return;
    };

    if !std::path::Path::new(&ext_dir).exists() {
        trace!(
            "[adblock-webext] extension dir missing ({}), fallback to in-process",
            ext_dir
        );
        WEBEXT_AVAILABLE.store(false, Ordering::Relaxed);
        WEBEXT_CONFIGURED.store(false, Ordering::Relaxed);
        return;
    }
    WEBEXT_AVAILABLE.store(true, Ordering::Relaxed);

    context.set_web_extensions_directory(&ext_dir);
    context.set_web_extensions_initialization_user_data(&glib::Variant::from(""));
    context.connect_initialize_web_extensions(|_| {
        trace!("[adblock-webext] initialize_web_extensions signal");
    });
    context.connect_user_message_received(|_, message| {
        if std::env::var_os("RASHAMON_DEBUG").is_some() {
            trace!("[adblock-webext] user-message: {:?}", message.name());
        }
        false
    });
    WEBEXT_CONFIGURED.store(true, Ordering::Relaxed);
    trace!("[adblock-webext] configured dir={ext_dir}");
}

fn connect_webext_view_messages(
    tab_id: u64,
    webview: &webkit2gtk::WebView,
    blocked_by_origin: Rc<RefCell<HashMap<String, OriginBlockCounts>>>,
) {
    use webkit2gtk::{UserMessageExt, WebViewExt};
    webview.connect_user_message_received(move |wv, message| {
        let Some(name) = message.name() else {
            return false;
        };
        let name = name.to_string();
        if name == "rashamon-webext-ready" || name == "rashamon-webext-pong" {
            WEBEXT_READY.store(true, Ordering::Relaxed);
            trace!("[webext] ready tab={tab_id}");
            return true;
        }
        if name == "rashamon-webext-blocked" {
            if let Some(params) = message.parameters() {
                if let Some((url, page_url, kind)) = params.get::<(String, String, String)>() {
                    trace!("[adblock-webext] blocked {url} kind={kind} source=webextension-hard");
                    increment_origin_block_count(
                        &blocked_by_origin,
                        &page_url,
                        BlockSource::WebExtensionHard,
                    );
                    return true;
                }
            }
            increment_origin_block_count(
                &blocked_by_origin,
                &wv_url(wv),
                BlockSource::WebExtensionHard,
            );
            trace!("[adblock-webext] blocked event source=webextension-hard params=none");
            return true;
        }
        false
    });
}

fn send_webext_ping_and_rules(
    tab_id: u64,
    webview: &webkit2gtk::WebView,
    adblock: &Rc<RefCell<AdblockEngine>>,
    private: bool,
) {
    use webkit2gtk::{UserMessage, UserMessageExt, WebViewExt};

    if !WEBEXT_CONFIGURED.load(Ordering::Relaxed) {
        trace!("[adblock-webext] rules/ping skipped tab={tab_id} reason=not-configured");
        return;
    }

    send_webext_rules(tab_id, webview, adblock, private, false, false);

    let ping = UserMessage::new(
        "rashamon-webext-ping",
        Some(&glib::Variant::from("ping")),
    );
    let webview_for_ready = webview.clone();
    let adblock_for_ready = Rc::clone(adblock);
    webview.send_message_to_page(&ping, None::<&gio::Cancellable>, move |result| {
        match result {
            Ok(reply) => {
                let ready = reply
                    .name()
                    .as_ref()
                    .map(|n| n.as_str() == "rashamon-webext-pong")
                    .unwrap_or(false);
                if ready {
                    WEBEXT_READY.store(true, Ordering::Relaxed);
                    trace!("[webext] ready tab={tab_id}; syncing rules after handshake");
                    send_webext_rules(
                        tab_id,
                        &webview_for_ready,
                        &adblock_for_ready,
                        private,
                        true,
                        std::env::var_os("RASHAMON_WEBEXT_SMOKE_INVALID_RULES").is_some(),
                    );
                } else {
                    trace!("[webext] ping reply unexpected tab={tab_id}: {:?}", reply.name());
                }
            }
            Err(err) => trace!("[webext] ping failed tab={tab_id}: {err}; fallback=in-process"),
        }
    });
}

fn send_webext_rules(
    tab_id: u64,
    webview: &webkit2gtk::WebView,
    adblock: &Rc<RefCell<AdblockEngine>>,
    private: bool,
    smoke_probe: bool,
    invalid_probe: bool,
) {
    use webkit2gtk::{UserMessage, UserMessageExt, WebViewExt};

    if !WEBEXT_CONFIGURED.load(Ordering::Relaxed) {
        trace!("[adblock-webext] rules sync skipped tab={tab_id} reason=not-configured fallback=in-process");
        return;
    }

    let payload = adblock.borrow().export_rule_sync_text_for_context(private);
    trace!(
        "[adblock-webext] rules sync start tab={tab_id} private={private} bytes={}",
        payload.len()
    );
    let rules = UserMessage::new(
        "rashamon-webext-set-rules",
        Some(&glib::Variant::from((payload,))),
    );
    let webview_for_probe = webview.clone();
    webview.send_message_to_page(&rules, None::<&gio::Cancellable>, move |result| {
        match result {
            Ok(reply) => {
                let name = reply.name().map(|n| n.to_string()).unwrap_or_default();
                if name == "rashamon-webext-rules-ok" {
                    WEBEXT_RULES_OK.fetch_add(1, Ordering::Relaxed);
                    trace!("[adblock-webext] rules sync ok tab={tab_id}");
                    if invalid_probe {
                        send_webext_invalid_rules_probe(tab_id, &webview_for_probe, smoke_probe);
                    } else if smoke_probe {
                        send_webext_smoke_probe(tab_id, &webview_for_probe);
                    }
                } else {
                    WEBEXT_RULES_ERROR.fetch_add(1, Ordering::Relaxed);
                    trace!("[adblock-webext] rules sync unexpected tab={tab_id}: {name}; fallback=in-process");
                }
            }
            Err(err) => {
                WEBEXT_RULES_ERROR.fetch_add(1, Ordering::Relaxed);
                trace!("[adblock-webext] rules sync failed tab={tab_id}: {err}; fallback=in-process");
            }
        }
    });
}

fn send_webext_smoke_probe(tab_id: u64, webview: &webkit2gtk::WebView) {
    use webkit2gtk::{UserMessage, UserMessageExt, WebViewExt};
    if std::env::var_os("RASHAMON_WEBEXT_SMOKE_PROBE").is_none() {
        return;
    }
    let probe = UserMessage::new(
        "rashamon-webext-probe-url",
        Some(&glib::Variant::from(("https://doubleclick.net/pagead/probe",))),
    );
    webview.send_message_to_page(&probe, None::<&gio::Cancellable>, move |result| {
        match result {
            Ok(reply) => {
                let name = reply.name().map(|n| n.to_string()).unwrap_or_default();
                if name == "rashamon-webext-probe-blocked" {
                    WEBEXT_PROBE_BLOCKED.fetch_add(1, Ordering::Relaxed);
                } else if name == "rashamon-webext-probe-clear" {
                    WEBEXT_PROBE_CLEAR.fetch_add(1, Ordering::Relaxed);
                }
                trace!("[webext] probe reply tab={tab_id}: {name}");
            }
            Err(err) => trace!("[webext] probe failed tab={tab_id}: {err}; fallback=in-process"),
        }
    });
}

fn send_webext_invalid_rules_probe(tab_id: u64, webview: &webkit2gtk::WebView, probe_after_reject: bool) {
    use webkit2gtk::{UserMessage, UserMessageExt, WebViewExt};
    let invalid = UserMessage::new(
        "rashamon-webext-set-rules",
        Some(&glib::Variant::from(("version=999\nenabled=1\n",))),
    );
    let webview_for_probe = webview.clone();
    webview.send_message_to_page(&invalid, None::<&gio::Cancellable>, move |result| {
        match result {
            Ok(reply) => {
                let name = reply.name().map(|n| n.to_string()).unwrap_or_default();
                if name == "rashamon-webext-rules-error" {
                    WEBEXT_RULES_ERROR.fetch_add(1, Ordering::Relaxed);
                    trace!("[adblock-webext] invalid rules rejected tab={tab_id}");
                    if probe_after_reject {
                        send_webext_smoke_probe(tab_id, &webview_for_probe);
                    }
                } else {
                    trace!(
                        "[adblock-webext] invalid rules reply unexpected tab={tab_id}: {name}"
                    );
                }
            }
            Err(err) => trace!("[adblock-webext] invalid rules probe failed tab={tab_id}: {err}"),
        }
    });
}

// ── Snapshot helper ───────────────────────────────────────────────────────────

fn next_cell_generation(cell: &Rc<Cell<u64>>) -> u64 {
    let next = cell.get().wrapping_add(1).max(1);
    cell.set(next);
    next
}

fn send_view_state(
    wv:     &webkit2gtk::WebView,
    tab_id: u64,
    nav_id: u64,
    tx:     &mpsc::SyncSender<Reply>,
) {
    use webkit2gtk::WebViewExt;
    let _ = tx.try_send(Reply::TitleChanged {
        tab_id,
        nav_id,
        title: wv_title(wv),
    });
    let _ = tx.try_send(Reply::UrlChanged {
        tab_id,
        nav_id,
        url: wv_url(wv),
    });
    let _ = tx.try_send(Reply::NavState {
        tab_id,
        can_back: wv.can_go_back(),
        can_forward: wv.can_go_forward(),
    });
}

fn schedule_view_state_sync(
    wv:     &webkit2gtk::WebView,
    tab_id: u64,
    tx:     mpsc::SyncSender<Reply>,
    nav_id: Rc<Cell<u64>>,
    alive:  Rc<Cell<bool>>,
    reason: &'static str,
    delay:  Duration,
) {
    let wv = wv.clone();
    glib::timeout_add_local(delay, move || {
        if alive.get() {
            let nav_id = nav_id.get();
            trace!("[webkit] state sync tab={tab_id} nav={nav_id} reason={reason}");
            send_view_state(&wv, tab_id, nav_id, &tx);
        } else {
            trace!("[webkit] skip state sync closed tab={tab_id} reason={reason}");
        }
        glib::ControlFlow::Break
    });
}

fn request_snapshot_now(
    wv:        &webkit2gtk::WebView,
    w:         u32,
    h:         u32,
    tab_id:    u64,
    tx:        mpsc::SyncSender<Reply>,
    nav_id:    Rc<Cell<u64>>,
    frame_gen: Rc<Cell<u64>>,
    alive:     Rc<Cell<bool>>,
    reason:    &'static str,
) {
    if !alive.get() {
        trace!("[webkit] skip snapshot closed tab={tab_id} reason={reason}");
        return;
    }
    perf_mark_tab("snapshot-requested", tab_id);
    let gen = next_cell_generation(&frame_gen);
    let nav_id_value = nav_id.get();
    trace!("[webkit] snapshot request tab={tab_id} nav={nav_id_value} gen={gen} reason={reason}");
    take_snapshot(
        wv, w, h, tab_id, nav_id_value, gen, reason,
        wv_title(wv), wv_url(wv), tx, frame_gen, alive,
    );
}

fn schedule_snapshot(
    wv:           &webkit2gtk::WebView,
    w:            u32,
    h:            u32,
    tab_id:       u64,
    tx:           mpsc::SyncSender<Reply>,
    nav_id:       Rc<Cell<u64>>,
    frame_gen:    Rc<Cell<u64>>,
    schedule_gen: Rc<Cell<u64>>,
    alive:        Rc<Cell<bool>>,
    token:        u64,
    reason:       &'static str,
    delay:        Duration,
) {
    let base_frame_gen = frame_gen.get();
    let wv = wv.clone();
    glib::timeout_add_local(delay, move || {
        if !alive.get() {
            trace!("[webkit] skip scheduled snapshot closed tab={tab_id} token={token} reason={reason}");
        } else if (reason.contains("settle") || reason == "spa-late")
            && frame_gen.get() != base_frame_gen
        {
            trace!(
                "[webkit] skip scheduled snapshot tab={tab_id} token={token} reason={reason} base_gen={} latest_gen={}",
                base_frame_gen,
                frame_gen.get()
            );
        } else if schedule_gen.get() == token {
            request_snapshot_now(
                &wv, w, h, tab_id, tx.clone(), Rc::clone(&nav_id),
                Rc::clone(&frame_gen), Rc::clone(&alive), reason,
            );
        } else {
            trace!("[webkit] coalesced snapshot tab={tab_id} token={token} latest={} reason={reason}",
                schedule_gen.get());
        }
        glib::ControlFlow::Break
    });
}

fn take_snapshot(
    wv:     &webkit2gtk::WebView,
    w:      u32,
    h:      u32,
    tab_id: u64,
    nav_id: u64,
    gen:    u64,
    reason: &'static str,
    title:  String,
    url:    String,
    tx:     mpsc::SyncSender<Reply>,
    frame_gen: Rc<Cell<u64>>,
    alive: Rc<Cell<bool>>,
) {
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};
    let started = Instant::now();

    wv.snapshot(
        SnapshotRegion::Visible,
        SnapshotOptions::empty(),
        None::<&gio::Cancellable>,
        move |result| match result {
            Err(e) => {
                trace!("[webkit] snapshot error tab={tab_id} gen={gen} reason={reason}: {e}");
            }
            Ok(src_surface) => {
                if !alive.get() {
                    trace!("[webkit] drop snapshot closed tab={tab_id} gen={gen} reason={reason}");
                    return;
                }
                if frame_gen.get() != gen {
                    trace!("[webkit] drop stale snapshot tab={tab_id} gen={gen} latest={} reason={reason}",
                        frame_gen.get());
                    return;
                }
                let mut img = match cairo::ImageSurface::create(
                    cairo::Format::ARgb32, w as i32, h as i32,
                ) {
                    Ok(s)  => s,
                    Err(e) => {
                        let _ = tx.try_send(Reply::LoadFailed {
                            tab_id, nav_id, reason: format!("cairo create: {e:?}"),
                        });
                        return;
                    }
                };
                {
                    let ctx = match cairo::Context::new(&img) {
                        Ok(c)  => c,
                        Err(e) => {
                            let _ = tx.try_send(Reply::LoadFailed {
                                tab_id, nav_id, reason: format!("cairo ctx: {e:?}"),
                            });
                            return;
                        }
                    };
                    let _ = ctx.set_source_surface(&src_surface, 0.0, 0.0);
                    let _ = ctx.paint();
                }

                let sw     = img.width()  as u32;
                let sh     = img.height() as u32;
                let stride = img.stride() as u32;

                let pixels = match img.data() {
                    Err(e) => {
                        let _ = tx.try_send(Reply::LoadFailed {
                            tab_id, nav_id, reason: format!("cairo borrow: {e:?}"),
                        });
                        return;
                    }
                    Ok(data) => {
                        let mut p = Vec::with_capacity((sw * sh * 4) as usize);
                        for row in 0..sh {
                            for col in 0..sw {
                                let s = (row * stride + col * 4) as usize;
                                if s + 3 < data.len() {
                                    p.push(data[s]);
                                    p.push(data[s + 1]);
                                    p.push(data[s + 2]);
                                    p.push(data[s + 3]);
                                } else {
                                    p.extend_from_slice(&[0, 0, 0, 255]);
                                }
                            }
                        }
                        p
                    }
                };

                trace!("[webkit] FrameReady tab={tab_id} nav={nav_id} gen={gen} reason={reason} in {}ms: {} bytes",
                    started.elapsed().as_millis(), pixels.len());
                perf_mark_tab("snapshot-completed", tab_id);
                if std::env::var_os("RASHAMON_PERF").is_some() {
                    eprintln!(
                        "[perf] snapshot_capture_ms={} reason={} bytes={}",
                        started.elapsed().as_millis(),
                        reason,
                        pixels.len()
                    );
                }
                perf_mark_tab("frame-ready-emitted", tab_id);
                let _ = tx.try_send(Reply::FrameReady {
                    tab_id, nav_id, gen, reason, pixels,
                    width: sw, height: sh, title, url,
                });
            }
        },
    );
}

// ── Small helpers ─────────────────────────────────────────────────────────────

fn adblock_block_reason(
    adblock: &Rc<RefCell<AdblockEngine>>,
    url: &str,
    origin: &str,
    private: bool,
) -> Option<String> {
    let (blocked, reason) = adblock
        .borrow_mut()
        .should_block_for_context(url, origin, private);
    blocked.then(|| reason.unwrap_or_else(|| "matched rule".to_string()))
}

fn log_adblock_block_with_kind(url: &str, reason: &str, kind: &str) {
    trace!("[adblock] blocked {url} reason={reason} kind={kind} source=in-process");
    if std::env::var_os("RASHAMON_PERF").is_some() {
        eprintln!("[perf] adblock_blocked kind={kind}");
    }
}

fn origin_key(origin_or_url: &str) -> String {
    origin_from_url(origin_or_url).unwrap_or_else(|| origin_or_url.to_string())
}

fn increment_origin_block_count(
    blocked_by_origin: &Rc<RefCell<HashMap<String, OriginBlockCounts>>>,
    origin_or_url: &str,
    source: BlockSource,
) {
    match source {
        BlockSource::InProcessPolicy => {
            ADBLOCK_POLICY_BLOCKS.fetch_add(1, Ordering::Relaxed);
        }
        BlockSource::InProcessSubresource => {
            ADBLOCK_SUBRESOURCE_REWRITES.fetch_add(1, Ordering::Relaxed);
        }
        BlockSource::WebExtensionHard => {
            WEBEXT_BLOCKED_EVENTS.fetch_add(1, Ordering::Relaxed);
        }
    }
    if origin_or_url.is_empty() {
        return;
    }
    let key = origin_key(origin_or_url);
    let mut counts = blocked_by_origin.borrow_mut();
    let entry = counts.entry(key).or_default();
    match source {
        BlockSource::InProcessPolicy => {
            entry.in_process_policy = entry.in_process_policy.saturating_add(1);
        }
        BlockSource::InProcessSubresource => {
            entry.in_process_subresource = entry.in_process_subresource.saturating_add(1);
        }
        BlockSource::WebExtensionHard => {
            entry.webextension_hard = entry.webextension_hard.saturating_add(1);
        }
    }
}

fn connect_downloads_for_context(
    context: &webkit2gtk::WebContext,
    tx: mpsc::SyncSender<Reply>,
    download_seq: Rc<Cell<u64>>,
    active_downloads: Rc<Cell<u32>>,
) {
    use webkit2gtk::{DownloadExt, URIRequestExt, WebContextExt};
    context.connect_download_started(move |_ctx, download| {
        let id = next_cell_generation(&download_seq);
        active_downloads.set(active_downloads.get().saturating_add(1));
        let tx_decide = tx.clone();
        download.connect_decide_destination(move |dl, suggested| {
            let dest = download_destination_for_suggested_filename(suggested);
            let filename = dest.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("download")
                .to_string();
            let uri = glib::filename_to_uri(&dest, None).unwrap_or_else(|_| {
                format!("file://{}", dest.to_string_lossy()).into()
            });
            trace!("[webkit-download] start id={id} file={}", dest.display());
            dl.set_allow_overwrite(false);
            dl.set_destination(&uri);
            let _ = tx_decide.try_send(Reply::DownloadStarted {
                id,
                filename,
                path: dest.to_string_lossy().to_string(),
            });
            true
        });

        let tx_progress = tx.clone();
        download.connect_received_data(move |dl, _len| {
            let progress = dl.estimated_progress().clamp(0.0, 1.0);
            let received = dl.received_data_length();
            let _ = tx_progress.try_send(Reply::DownloadProgress { id, received, progress });
        });

        let tx_finished = tx.clone();
        let active_finished = Rc::clone(&active_downloads);
        download.connect_finished(move |dl| {
            active_finished.set(active_finished.get().saturating_sub(1));
            let path = dl.destination()
                .and_then(|uri| glib::filename_from_uri(&uri).ok().map(|(p, _)| p))
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "download complete".to_string());
            trace!("[webkit-download] finished id={id} path={path}");
            let _ = tx_finished.try_send(Reply::DownloadFinished { id, path });
        });

        let tx_failed = tx.clone();
        let active_failed = Rc::clone(&active_downloads);
        download.connect_failed(move |_dl, err| {
            active_failed.set(active_failed.get().saturating_sub(1));
            trace!("[webkit-download] failed id={id}: {err}");
            let _ = tx_failed.try_send(Reply::DownloadFailed {
                id,
                reason: err.to_string(),
            });
        });

        let url = download.request()
            .and_then(|r| r.uri())
            .map(|s| s.to_string())
            .unwrap_or_default();
        trace!("[webkit-download] signal id={id} url={url}");
    });
}

pub fn download_destination_for_test(filename: &str) -> PathBuf {
    download_destination_for_suggested_filename(filename)
}

fn download_destination_for_suggested_filename(suggested: &str) -> PathBuf {
    let dir = default_download_dir();
    let _ = std::fs::create_dir_all(&dir);
    unique_download_path(&dir, &safe_download_filename(suggested))
}

fn default_download_dir() -> PathBuf {
    platform::default_downloads_dir()
}

fn safe_download_filename(suggested: &str) -> String {
    let raw = suggested.rsplit('/').next().unwrap_or(suggested).trim();
    let mut out = String::with_capacity(raw.len().max(8));
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('_');
        }
    }
    let out = out.trim_matches('.').to_string();
    if out.is_empty() { "download".to_string() } else { out }
}

fn unique_download_path(dir: &Path, filename: &str) -> PathBuf {
    let candidate = dir.join(filename);
    if !candidate.exists() {
        return candidate;
    }

    let path = Path::new(filename);
    let stem = path.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    for idx in 1..10_000 {
        let name = if ext.is_empty() {
            format!("{stem} ({idx})")
        } else {
            format!("{stem} ({idx}).{ext}")
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-{}", next_fallback_suffix()))
}

fn next_fallback_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn find_options() -> u32 {
    use webkit2gtk::FindOptions;
    (FindOptions::CASE_INSENSITIVE | FindOptions::WRAP_AROUND).bits()
}

fn webkit_find_text(entry: &TabEntry, query: &str) {
    use webkit2gtk::{FindControllerExt, WebViewExt};
    let Some(find) = entry.webview.find_controller() else { return; };
    if query.trim().is_empty() {
        trace!("[webkit-find] clear empty query");
        find.search_finish();
        return;
    }
    trace!("[webkit-find] search {:?}", query);
    find.search(query, find_options(), 1_000);
    find.count_matches(query, find_options(), 1_000);
}

fn webkit_find_next(entry: &TabEntry) {
    use webkit2gtk::{FindControllerExt, WebViewExt};
    if let Some(find) = entry.webview.find_controller() {
        trace!("[webkit-find] next");
        find.search_next();
    }
}

fn webkit_find_previous(entry: &TabEntry) {
    use webkit2gtk::{FindControllerExt, WebViewExt};
    if let Some(find) = entry.webview.find_controller() {
        trace!("[webkit-find] previous");
        find.search_previous();
    }
}

fn webkit_find_clear(entry: &TabEntry) {
    use webkit2gtk::{FindControllerExt, WebViewExt};
    if let Some(find) = entry.webview.find_controller() {
        trace!("[webkit-find] clear");
        find.search_finish();
    }
}

fn wv_title(wv: &webkit2gtk::WebView) -> String {
    use webkit2gtk::WebViewExt;
    wv.title().map(|s| s.to_string()).unwrap_or_default()
}

fn wv_url(wv: &webkit2gtk::WebView) -> String {
    use webkit2gtk::WebViewExt;
    wv.uri().map(|s| s.to_string()).unwrap_or_default()
}

fn js_string_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn cursor_from_js(value: &str) -> CursorKind {
    match value.trim_matches('"').trim_matches('\'') {
        "pointer" => CursorKind::Pointer,
        "text" => CursorKind::Text,
        "wait" => CursorKind::Wait,
        _ => CursorKind::Default,
    }
}
