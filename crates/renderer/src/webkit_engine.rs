//! WebKitGTK content engine — real web rendering via WebKit 2.50+.
//!
//! Architecture (main-thread GTK):
//!   WebKitEngine::create() returns (WebKitEngine, WebKitDriver).
//!   WebKitEngine  — Send, holds mpsc channels, implements ContentEngine.
//!   WebKitDriver  — !Send, holds live GTK objects, must be pump()ed from main thread.
//!
//! Navigation session identity:
//!   Every navigate(url, nav_id) call stamps a nav_id on the engine side.
//!   The driver shares an Rc<Cell<u64>> with all GTK signal closures.
//!   When navigate() is processed, the cell is updated to the new nav_id so
//!   every subsequent reply (FrameReady, TitleChanged, LoadFailed …) carries
//!   the correct session token.
//!   poll_events() drops any reply whose nav_id ≠ expected_nav_id, preventing
//!   late replies from a superseded navigation from leaking into the shell.

use crate::engine_trait::{ContentEngine, EngineEvent, EngineFrame};
use crate::framebuffer::{Framebuffer, Pixel};

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

// ── IPC types ─────────────────────────────────────────────────────────────────

enum Cmd {
    Navigate(String, u64),  // (url, nav_id)
    ScrollTo(i32),          // absolute page-Y in pixels
    Shutdown,
}

enum Reply {
    FrameReady {
        nav_id: u64,
        pixels: Vec<u8>,
        width:  u32,
        height: u32,
        title:  String,
        url:    String,
    },
    TitleChanged(u64, String),
    ContentHeight(u64, u32),
    LoadFailed(u64, String),
}

// ── Cached frame ──────────────────────────────────────────────────────────────

struct CachedFrame {
    pixels: Vec<u8>,
    width:  u32,
    height: u32,
}

// ── Engine (Send) ─────────────────────────────────────────────────────────────

pub struct WebKitEngine {
    cmd_tx:          mpsc::SyncSender<Cmd>,
    reply_rx:        mpsc::Receiver<Reply>,
    cache:           Option<CachedFrame>,
    title:           Option<String>,
    url:             Option<String>,
    events:          Vec<EngineEvent>,
    scroll_y:        i32,
    expected_nav_id: u64,
}

// ── Driver (!Send — main thread only) ────────────────────────────────────────

pub struct WebKitDriver {
    cmd_rx:      mpsc::Receiver<Cmd>,
    reply_tx:    mpsc::SyncSender<Reply>,
    webview:     webkit2gtk::WebView,
    _window:     gtk::OffscreenWindow,
    w:           u32,
    h:           u32,
    /// Shared with GTK signal closures: always holds the nav_id of the most
    /// recently dispatched Navigate command.  Signal callbacks read this when
    /// they fire so every reply carries the correct session token.
    nav_id_cell: Rc<Cell<u64>>,
}

impl WebKitEngine {
    /// Create engine + driver. **Must be called from the main thread.**
    pub fn create(content_w: u32, content_h: u32)
        -> Result<(Self, WebKitDriver), Box<dyn std::error::Error>>
    {
        use gtk::prelude::{ContainerExt, GtkWindowExt, WidgetExt};
        use webkit2gtk::{
            HardwareAccelerationPolicy, LoadEvent, Settings, SettingsExt,
            WebView, WebViewExt,
        };

        gtk::init().map_err(|e| format!("GTK init failed: {e}"))?;

        let (cmd_tx, cmd_rx)     = mpsc::sync_channel::<Cmd>(8);
        let (reply_tx, reply_rx) = mpsc::sync_channel::<Reply>(8);

        let settings = Settings::new();
        settings.set_enable_webgl(false);
        settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never);

        let webview = WebView::new();
        webview.set_settings(&settings);
        webview.set_size_request(content_w as i32, content_h as i32);

        let window = gtk::OffscreenWindow::new();
        window.set_default_size(content_w as i32, content_h as i32);
        window.add(&webview);
        window.show_all();

        // nav_id_cell is shared between the driver and all signal closures.
        let nav_id_cell: Rc<Cell<u64>> = Rc::new(Cell::new(0));

        // ── load-changed: snapshot on finish, height hint on commit ──────────
        {
            let tx        = reply_tx.clone();
            let nav_cell  = Rc::clone(&nav_id_cell);
            let w = content_w;
            let h = content_h;
            webview.connect_load_changed(move |wv, event| {
                let nav_id = nav_cell.get();
                match event {
                    LoadEvent::Finished => {
                        let title = wv.title().map(|s| s.to_string()).unwrap_or_default();
                        let url   = wv.uri().map(|s| s.to_string()).unwrap_or_default();
                        eprintln!("[webkit] load-finished nav={nav_id}: {url:?}");
                        let _ = tx.try_send(Reply::ContentHeight(nav_id, 8000));
                        take_snapshot(wv, w, h, nav_id, title, url, tx.clone());
                    }
                    _ => {}
                }
            });
        }

        // ── load-failed ───────────────────────────────────────────────────────
        {
            let tx       = reply_tx.clone();
            let nav_cell = Rc::clone(&nav_id_cell);
            webview.connect_load_failed(move |_wv, _event, uri, error| {
                let nav_id = nav_cell.get();
                let msg    = error.to_string();
                eprintln!("[webkit] load-failed nav={nav_id}: {uri} — {msg}");
                // WebKit fires this for internal load cancellations (caused by
                // calling load_uri() while a previous load is in-flight).
                // Suppress these so they don't incorrectly mark the new
                // navigation as failed.
                let is_cancel = msg.contains("ancelled")
                    || msg.contains("policy change")
                    || msg.contains("interrupted")
                    || msg.contains("nterrupted");
                if !is_cancel {
                    let _ = tx.try_send(Reply::LoadFailed(nav_id, msg));
                }
                false
            });
        }

        // ── title changed ─────────────────────────────────────────────────────
        {
            let tx       = reply_tx.clone();
            let nav_cell = Rc::clone(&nav_id_cell);
            webview.connect_title_notify(move |wv| {
                if let Some(t) = wv.title() {
                    let nav_id = nav_cell.get();
                    let _ = tx.try_send(Reply::TitleChanged(nav_id, t.to_string()));
                }
            });
        }

        eprintln!("[webkit] Engine created ({}×{})", content_w, content_h);

        let engine = WebKitEngine {
            cmd_tx,
            reply_rx,
            cache:           None,
            title:           None,
            url:             None,
            events:          Vec::new(),
            scroll_y:        0,
            expected_nav_id: 0,
        };

        let driver = WebKitDriver {
            cmd_rx,
            reply_tx,
            webview,
            _window:     window,
            w:           content_w,
            h:           content_h,
            nav_id_cell,
        };

        Ok((engine, driver))
    }
}

impl Drop for WebKitEngine {
    fn drop(&mut self) { let _ = self.cmd_tx.try_send(Cmd::Shutdown); }
}

impl ContentEngine for WebKitEngine {
    fn navigate(&mut self, url: &str, nav_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[webkit] navigate nav={nav_id} → {url}");
        self.expected_nav_id = nav_id;
        self.cache    = None;
        self.url      = Some(url.to_string());
        self.scroll_y = 0;
        self.events.push(EngineEvent::LoadStarted);
        self.cmd_tx.send(Cmd::Navigate(url.to_string(), nav_id))
            .map_err(|e| format!("webkit channel closed: {e}"))?;
        Ok(())
    }

    fn current_nav_id(&self) -> u64 { self.expected_nav_id }

    fn go_back(&mut self)    -> Result<(), Box<dyn std::error::Error>> { Ok(()) }
    fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> { Ok(()) }

    fn reload(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(url) = self.url.clone() {
            self.navigate(&url, self.expected_nav_id)?;
        }
        Ok(())
    }

    fn scroll(&mut self, delta_y: i32) {
        self.scroll_y = (self.scroll_y + delta_y).max(0);
        let _ = self.cmd_tx.try_send(Cmd::ScrollTo(self.scroll_y));
    }

    fn render_into(
        &mut self,
        fb:  &mut Framebuffer,
        x:   u32, y: u32, w: u32, h: u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>> {
        let Some(cache) = &self.cache else { return Ok(EngineFrame::NotReady); };

        let src_w = cache.width;
        let src_h = cache.height;
        let rows  = h.min(src_h);
        let cols  = w.min(src_w);

        // Cairo ARGB32 on little-endian: memory layout = [B, G, R, A] per pixel.
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

    fn poll_events(&mut self) -> Vec<EngineEvent> {
        loop {
            match self.reply_rx.try_recv() {
                Ok(Reply::FrameReady { nav_id, pixels, width, height, title, url }) => {
                    if nav_id != self.expected_nav_id {
                        eprintln!("[webkit] drop stale FrameReady nav={nav_id} (expected {})", self.expected_nav_id);
                        continue;
                    }
                    eprintln!("[webkit] Frame ready: {}×{} ({} bytes)", width, height, pixels.len());
                    self.cache = Some(CachedFrame { pixels, width, height });
                    self.title = Some(title.clone());
                    self.url   = Some(url.clone());
                    self.events.push(EngineEvent::TitleChanged(title));
                    self.events.push(EngineEvent::UrlChanged(url));
                    self.events.push(EngineEvent::LoadComplete);
                    self.events.push(EngineEvent::ContentHeightChanged(height));
                }
                Ok(Reply::TitleChanged(nav_id, t)) => {
                    if nav_id != self.expected_nav_id { continue; }
                    self.title = Some(t.clone());
                    self.events.push(EngineEvent::TitleChanged(t));
                }
                Ok(Reply::ContentHeight(nav_id, h)) => {
                    if nav_id != self.expected_nav_id { continue; }
                    self.events.push(EngineEvent::ContentHeightChanged(h));
                }
                Ok(Reply::LoadFailed(nav_id, r)) => {
                    if nav_id != self.expected_nav_id {
                        eprintln!("[webkit] drop stale LoadFailed nav={nav_id}");
                        continue;
                    }
                    eprintln!("[webkit] Load failed: {r}");
                    self.events.push(EngineEvent::LoadFailed(r));
                }
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
            }
        }
        std::mem::take(&mut self.events)
    }

    fn title(&self)       -> Option<String> { self.title.clone() }
    fn current_url(&self) -> Option<String> { self.url.clone() }
}

// ── Driver ────────────────────────────────────────────────────────────────────

impl WebKitDriver {
    /// Process pending GLib events and dispatch queued commands to WebView.
    /// **Must be called from the main thread every frame.**
    pub fn pump(&mut self) {
        use webkit2gtk::WebViewExt;

        // Drain all pending GLib/GTK events without blocking.
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }

        // Dispatch commands queued by WebKitEngine.
        loop {
            match self.cmd_rx.try_recv() {
                Ok(Cmd::Navigate(url, nav_id)) => {
                    eprintln!("[webkit-driver] load_uri nav={nav_id}: {url}");
                    // Update the shared cell BEFORE calling load_uri so that
                    // any synchronously-fired GTK signals see the new nav_id.
                    self.nav_id_cell.set(nav_id);
                    self.webview.load_uri(&url);
                }
                Ok(Cmd::ScrollTo(y)) => {
                    eprintln!("[webkit-driver] scroll_to: {y}");
                    let script = format!("window.scrollTo(0, {y})");
                    self.webview.run_javascript(&script, None::<&gio::Cancellable>, |_| {});
                    let wv         = self.webview.clone();
                    let tx         = self.reply_tx.clone();
                    let w          = self.w;
                    let h          = self.h;
                    let nav_cell   = Rc::clone(&self.nav_id_cell);
                    glib::timeout_add_local(Duration::from_millis(150), move || {
                        let nav_id = nav_cell.get();
                        let title  = wv.title().map(|s| s.to_string()).unwrap_or_default();
                        let url    = wv.uri().map(|s| s.to_string()).unwrap_or_default();
                        take_snapshot(&wv, w, h, nav_id, title, url, tx.clone());
                        glib::ControlFlow::Break
                    });
                }
                Ok(Cmd::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
    }
}

// ── Snapshot helper ───────────────────────────────────────────────────────────

#[cfg(feature = "webkit")]
fn take_snapshot(
    wv:     &webkit2gtk::WebView,
    w:      u32,
    h:      u32,
    nav_id: u64,
    title:  String,
    url:    String,
    tx:     mpsc::SyncSender<Reply>,
) {
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};
    use cairo;

    wv.snapshot(
        SnapshotRegion::Visible,
        SnapshotOptions::empty(),
        None::<&gio::Cancellable>,
        move |result| {
            match result {
                Err(e) => {
                    eprintln!("[webkit] snapshot error: {e}");
                    let _ = tx.try_send(Reply::LoadFailed(nav_id, e.to_string()));
                }
                Ok(src_surface) => {
                    let mut img = match cairo::ImageSurface::create(
                        cairo::Format::ARgb32, w as i32, h as i32,
                    ) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = tx.try_send(Reply::LoadFailed(nav_id, format!("cairo create: {e:?}")));
                            return;
                        }
                    };
                    {
                        let ctx = match cairo::Context::new(&img) {
                            Ok(c) => c,
                            Err(e) => {
                                let _ = tx.try_send(Reply::LoadFailed(nav_id, format!("cairo ctx: {e:?}")));
                                return;
                            }
                        };
                        let _ = ctx.set_source_surface(&src_surface, 0.0, 0.0);
                        let _ = ctx.paint();
                    }

                    let sw     = img.width()  as u32;
                    let sh     = img.height() as u32;
                    let stride = img.stride() as u32;
                    eprintln!("[webkit] ImageSurface: {sw}×{sh} stride={stride}");

                    let pixels: Vec<u8> = match img.data() {
                        Err(e) => {
                            let _ = tx.try_send(Reply::LoadFailed(nav_id, format!("cairo borrow: {e:?}")));
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

                    eprintln!("[webkit] sending frame nav={nav_id}: {} bytes", pixels.len());
                    let _ = tx.try_send(Reply::FrameReady {
                        nav_id, pixels, width: sw, height: sh, title, url,
                    });
                }
            }
        },
    );
}
