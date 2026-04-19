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

use crate::engine_trait::{ContentEngine, EngineEvent, EngineFrame};
use crate::framebuffer::{Framebuffer, Pixel};

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

// ── IPC ───────────────────────────────────────────────────────────────────────

enum Cmd {
    /// Create a new WebView for this tab.  Private tabs get an ephemeral context.
    CreateTab  { tab_id: u64, is_private: bool },
    /// Destroy the WebView and release GTK resources.
    CloseTab   { tab_id: u64 },
    /// Activate tab and request a fresh snapshot (no reload).
    SwitchTab  { tab_id: u64 },
    /// Load a URL in the specified tab's WebView.
    Navigate   { tab_id: u64, url: String, nav_id: u64 },
    /// Scroll and re-snapshot the specified tab.
    ScrollTo   { tab_id: u64, y: i32 },
    Shutdown,
}

enum Reply {
    FrameReady {
        tab_id: u64,
        nav_id: u64,
        pixels: Vec<u8>,
        width:  u32,
        height: u32,
        title:  String,
        url:    String,
    },
    TitleChanged  { tab_id: u64, nav_id: u64, title:  String },
    ContentHeight { tab_id: u64, nav_id: u64, h:      u32    },
    LoadFailed    { tab_id: u64, nav_id: u64, reason: String },
}

// ── Per-tab engine state ──────────────────────────────────────────────────────

struct CachedFrame { pixels: Vec<u8>, width: u32, height: u32 }

#[derive(Default)]
struct PerTabState {
    cache:            Option<CachedFrame>,
    title:            Option<String>,
    url:              Option<String>,
    expected_nav_id:  u64,
}

// ── WebKitEngine (Send) ───────────────────────────────────────────────────────

pub struct WebKitEngine {
    cmd_tx:         mpsc::SyncSender<Cmd>,
    reply_rx:       mpsc::Receiver<Reply>,
    active_tab_id:  u64,
    tab_states:     HashMap<u64, PerTabState>,
    pending_events: Vec<(u64, EngineEvent)>,
    scroll_y:       i32,
}

// ── WebKitDriver (!Send — main thread only) ───────────────────────────────────

struct TabEntry {
    webview:     webkit2gtk::WebView,
    _window:     gtk::OffscreenWindow,
    nav_id_cell: Rc<Cell<u64>>,
}

pub struct WebKitDriver {
    cmd_rx:   mpsc::Receiver<Cmd>,
    reply_tx: mpsc::SyncSender<Reply>,
    tabs:     HashMap<u64, TabEntry>,
    w:        u32,
    h:        u32,
}

// ── Construction ──────────────────────────────────────────────────────────────

impl WebKitEngine {
    /// Initialise GTK and create the channel pair.  **Must be called from the
    /// main thread** (GTK requirement).
    pub fn create(content_w: u32, content_h: u32)
        -> Result<(Self, WebKitDriver), Box<dyn std::error::Error>>
    {
        gtk::init().map_err(|e| format!("GTK init failed: {e}"))?;

        let (cmd_tx, cmd_rx)     = mpsc::sync_channel::<Cmd>(32);
        let (reply_tx, reply_rx) = mpsc::sync_channel::<Reply>(32);

        eprintln!("[webkit] Engine created ({}×{})", content_w, content_h);

        let engine = WebKitEngine {
            cmd_tx,
            reply_rx,
            active_tab_id:  0,
            tab_states:     HashMap::new(),
            pending_events: Vec::new(),
            scroll_y:       0,
        };

        let driver = WebKitDriver {
            cmd_rx,
            reply_tx,
            tabs: HashMap::new(),
            w:    content_w,
            h:    content_h,
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
        eprintln!("[webkit] create_tab {tab_id} private={is_private}");
        self.tab_states.entry(tab_id).or_insert_with(PerTabState::default);
        let _ = self.cmd_tx.try_send(Cmd::CreateTab { tab_id, is_private });
    }

    fn close_tab(&mut self, tab_id: u64) {
        eprintln!("[webkit] close_tab {tab_id}");
        self.tab_states.remove(&tab_id);
        let _ = self.cmd_tx.try_send(Cmd::CloseTab { tab_id });
    }

    fn set_active_tab(&mut self, tab_id: u64) {
        eprintln!("[webkit] set_active_tab {tab_id}");
        self.active_tab_id = tab_id;
        self.scroll_y = 0;
        // Request a fresh snapshot — the WebView already has its page loaded.
        let _ = self.cmd_tx.try_send(Cmd::SwitchTab { tab_id });
    }

    fn navigate(&mut self, url: &str, nav_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let tab_id = self.active_tab_id;
        eprintln!("[webkit] navigate tab={tab_id} nav={nav_id} → {url}");
        let state = self.tab_states.entry(tab_id).or_insert_with(PerTabState::default);
        state.expected_nav_id = nav_id;
        state.cache           = None;
        state.url             = Some(url.to_string());
        self.scroll_y = 0;
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

    // Back/forward/reload: the shell already resolved the target URL via
    // ui_state and calls navigate() directly.  These are no-ops.
    fn go_back(&mut self)    -> Result<(), Box<dyn std::error::Error>> { Ok(()) }
    fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> { Ok(()) }
    fn reload(&mut self)     -> Result<(), Box<dyn std::error::Error>> {
        if let Some(url) = self.tab_states.get(&self.active_tab_id)
            .and_then(|s| s.url.clone())
        {
            let nav_id = self.current_nav_id();
            self.navigate(&url, nav_id)?;
        }
        Ok(())
    }

    fn scroll(&mut self, delta_y: i32) {
        self.scroll_y = (self.scroll_y + delta_y).max(0);
        let tab_id = self.active_tab_id;
        let _ = self.cmd_tx.try_send(Cmd::ScrollTo { tab_id, y: self.scroll_y });
    }

    fn render_into(
        &mut self,
        fb:  &mut Framebuffer,
        x:   u32, y: u32, w: u32, h: u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>> {
        let tab_id = self.active_tab_id;
        let Some(state) = self.tab_states.get(&tab_id) else {
            return Ok(EngineFrame::NotReady);
        };
        let Some(cache) = &state.cache else { return Ok(EngineFrame::NotReady); };

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
                Ok(Reply::FrameReady { tab_id, nav_id, pixels, width, height, title, url }) => {
                    let state = self.tab_states.entry(tab_id).or_insert_with(PerTabState::default);
                    // Allow nav_id == 0 for switch-triggered snapshots (no active nav).
                    if nav_id != 0 && state.expected_nav_id != 0
                        && nav_id != state.expected_nav_id
                    {
                        eprintln!("[webkit] drop stale FrameReady tab={tab_id} nav={nav_id} (expected {})",
                            state.expected_nav_id);
                        continue;
                    }
                    eprintln!("[webkit] FrameReady tab={tab_id} {}×{} ({} bytes)",
                        width, height, pixels.len());
                    state.cache = Some(CachedFrame { pixels, width, height });
                    state.title = Some(title.clone());
                    state.url   = Some(url.clone());
                    self.pending_events.push((tab_id, EngineEvent::TitleChanged(title)));
                    self.pending_events.push((tab_id, EngineEvent::UrlChanged(url)));
                    self.pending_events.push((tab_id, EngineEvent::LoadComplete));
                    self.pending_events.push((tab_id, EngineEvent::ContentHeightChanged(height)));
                }
                Ok(Reply::TitleChanged { tab_id, nav_id, title }) => {
                    let state = self.tab_states.entry(tab_id).or_insert_with(PerTabState::default);
                    if nav_id != 0 && state.expected_nav_id != 0
                        && nav_id != state.expected_nav_id
                    { continue; }
                    state.title = Some(title.clone());
                    self.pending_events.push((tab_id, EngineEvent::TitleChanged(title)));
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
                        eprintln!("[webkit] drop stale LoadFailed tab={tab_id} nav={nav_id}");
                        continue;
                    }
                    eprintln!("[webkit] LoadFailed tab={tab_id}: {reason}");
                    self.pending_events.push((tab_id, EngineEvent::LoadFailed(reason)));
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
                    if self.tabs.contains_key(&tab_id) { continue; }
                    let entry = make_tab_entry(
                        tab_id, is_private, self.w, self.h, self.reply_tx.clone(),
                    );
                    self.tabs.insert(tab_id, entry);
                    eprintln!("[webkit-driver] created WebView for tab {tab_id}");
                }

                Ok(Cmd::CloseTab { tab_id }) => {
                    self.tabs.remove(&tab_id);
                    eprintln!("[webkit-driver] dropped WebView for tab {tab_id}");
                }

                Ok(Cmd::SwitchTab { tab_id }) => {
                    if let Some(entry) = self.tabs.get(&tab_id) {
                        let nav_id = entry.nav_id_cell.get();
                        let title  = wv_title(&entry.webview);
                        let url    = wv_url(&entry.webview);
                        eprintln!("[webkit-driver] SwitchTab {tab_id} → snapshot nav={nav_id}");
                        take_snapshot(
                            &entry.webview, self.w, self.h,
                            tab_id, nav_id, title, url,
                            self.reply_tx.clone(),
                        );
                    }
                }

                Ok(Cmd::Navigate { tab_id, url, nav_id }) => {
                    if let Some(entry) = self.tabs.get(&tab_id) {
                        use webkit2gtk::WebViewExt;
                        eprintln!("[webkit-driver] Navigate tab={tab_id} nav={nav_id}: {url}");
                        // Update shared cell BEFORE load_uri so synchronous signals
                        // fire with the correct nav_id.
                        entry.nav_id_cell.set(nav_id);
                        entry.webview.load_uri(&url);
                    } else {
                        eprintln!("[webkit-driver] Navigate for unknown tab {tab_id}");
                    }
                }

                Ok(Cmd::ScrollTo { tab_id, y }) => {
                    if let Some(entry) = self.tabs.get(&tab_id) {
                        use webkit2gtk::WebViewExt;
                        let script = format!("window.scrollTo(0, {y})");
                        entry.webview.run_javascript(
                            &script, None::<&gio::Cancellable>, |_| {},
                        );
                        // Re-snapshot after a short delay so the scroll settles.
                        let wv       = entry.webview.clone();
                        let tx       = self.reply_tx.clone();
                        let nc       = Rc::clone(&entry.nav_id_cell);
                        let (w, h)   = (self.w, self.h);
                        glib::timeout_add_local(Duration::from_millis(150), move || {
                            let nav_id = nc.get();
                            take_snapshot(
                                &wv, w, h, tab_id, nav_id,
                                wv_title(&wv), wv_url(&wv), tx.clone(),
                            );
                            glib::ControlFlow::Break
                        });
                    }
                }

                Ok(Cmd::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
    }
}

// ── WebView factory ───────────────────────────────────────────────────────────

fn make_tab_entry(
    tab_id:    u64,
    is_private: bool,
    w:         u32,
    h:         u32,
    reply_tx:  mpsc::SyncSender<Reply>,
) -> TabEntry {
    use gtk::prelude::{ContainerExt, GtkWindowExt, WidgetExt};
    use webkit2gtk::{
        HardwareAccelerationPolicy, LoadEvent, Settings, SettingsExt,
        WebView, WebViewExt,
    };

    let settings = Settings::new();
    settings.set_enable_webgl(false);
    settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never);

    let webview = if is_private {
        use webkit2gtk::WebContext;
        let ctx = WebContext::new_ephemeral();
        let wv  = WebView::with_context(&ctx);
        wv.set_settings(&settings);
        wv
    } else {
        let wv = WebView::new();
        wv.set_settings(&settings);
        wv
    };
    webview.set_size_request(w as i32, h as i32);

    let window = gtk::OffscreenWindow::new();
    window.set_default_size(w as i32, h as i32);
    window.add(&webview);
    window.show_all();

    let nav_id_cell: Rc<Cell<u64>> = Rc::new(Cell::new(0));

    // load-changed: snapshot on Finished, height hint on Committed.
    {
        let tx  = reply_tx.clone();
        let nc  = Rc::clone(&nav_id_cell);
        webview.connect_load_changed(move |wv, event| {
            if event == LoadEvent::Finished {
                let nav_id = nc.get();
                let _ = tx.try_send(Reply::ContentHeight { tab_id, nav_id, h: 8000 });
                take_snapshot(
                    wv, w, h, tab_id, nav_id,
                    wv_title(wv), wv_url(wv), tx.clone(),
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

    TabEntry { webview, _window: window, nav_id_cell }
}

// ── Snapshot helper ───────────────────────────────────────────────────────────

fn take_snapshot(
    wv:     &webkit2gtk::WebView,
    w:      u32,
    h:      u32,
    tab_id: u64,
    nav_id: u64,
    title:  String,
    url:    String,
    tx:     mpsc::SyncSender<Reply>,
) {
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};

    wv.snapshot(
        SnapshotRegion::Visible,
        SnapshotOptions::empty(),
        None::<&gio::Cancellable>,
        move |result| match result {
            Err(e) => {
                eprintln!("[webkit] snapshot error tab={tab_id}: {e}");
                let _ = tx.try_send(Reply::LoadFailed {
                    tab_id, nav_id, reason: e.to_string(),
                });
            }
            Ok(src_surface) => {
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

                eprintln!("[webkit] FrameReady tab={tab_id} nav={nav_id}: {} bytes",
                    pixels.len());
                let _ = tx.try_send(Reply::FrameReady {
                    tab_id, nav_id, pixels,
                    width: sw, height: sh, title, url,
                });
            }
        },
    );
}

// ── Small helpers ─────────────────────────────────────────────────────────────

fn wv_title(wv: &webkit2gtk::WebView) -> String {
    use webkit2gtk::WebViewExt;
    wv.title().map(|s| s.to_string()).unwrap_or_default()
}

fn wv_url(wv: &webkit2gtk::WebView) -> String {
    use webkit2gtk::WebViewExt;
    wv.uri().map(|s| s.to_string()).unwrap_or_default()
}
