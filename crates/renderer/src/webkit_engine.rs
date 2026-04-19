//! WebKitGTK content engine — real web rendering via WebKit 2.50+.
//!
//! Architecture (main-thread GTK):
//!   WebKitEngine::create() returns (WebKitEngine, WebKitDriver).
//!   WebKitEngine  — Send, holds mpsc channels, implements ContentEngine.
//!   WebKitDriver  — !Send, holds live GTK objects, must be pump()ed from main thread.
//!
//!   navigate() → enqueues Cmd::Navigate.
//!   WebKitDriver::pump() drains cmd channel, calls load_uri on WebView.
//!   GTK load-finished signal → take_snapshot → sends Reply::FrameReady.
//!   WebKitEngine::poll_events() drains reply_rx → EngineEvents.
//!   render_into() blits cached BGRA pixels into framebuffer.

use crate::engine_trait::{ContentEngine, EngineEvent, EngineFrame};
use crate::framebuffer::{Framebuffer, Pixel};

use std::sync::mpsc;
use std::time::Duration;

// ── IPC types ─────────────────────────────────────────────────────────────────

enum Cmd {
    Navigate(String),
    ScrollTo(i32),
    Shutdown,
}

enum Reply {
    FrameReady {
        pixels: Vec<u8>,
        width:  u32,
        height: u32,
        title:  String,
        url:    String,
    },
    TitleChanged(String),
    UrlChanged(String),
    ContentHeight(u32),
    LoadFailed(String),
}

// ── Cached frame ──────────────────────────────────────────────────────────────

struct CachedFrame {
    pixels: Vec<u8>,
    width:  u32,
    height: u32,
}

// ── Engine (Send) ─────────────────────────────────────────────────────────────

pub struct WebKitEngine {
    cmd_tx:   mpsc::SyncSender<Cmd>,
    reply_rx: mpsc::Receiver<Reply>,
    cache:    Option<CachedFrame>,
    title:    Option<String>,
    url:      Option<String>,
    events:   Vec<EngineEvent>,
    scroll_y: i32,
}

// ── Driver (!Send — main thread only) ────────────────────────────────────────

pub struct WebKitDriver {
    cmd_rx:   mpsc::Receiver<Cmd>,
    reply_tx: mpsc::SyncSender<Reply>,
    webview:  webkit2gtk::WebView,
    _window:  gtk::OffscreenWindow,
    w: u32,
    h: u32,
}

impl WebKitEngine {
    /// Create engine + driver. **Must be called from the main thread.**
    /// The caller is responsible for calling `WebKitDriver::pump()` every frame.
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

        // ── Signals ───────────────────────────────────────────────────────────
        {
            let tx = reply_tx.clone();
            let w = content_w;
            let h = content_h;
            webview.connect_load_changed(move |wv, event| {
                if event != LoadEvent::Finished { return; }
                let title = wv.title().map(|s| s.to_string()).unwrap_or_default();
                let url   = wv.uri().map(|s| s.to_string()).unwrap_or_default();
                eprintln!("[webkit] load-finished: {url:?}  title={title:?}");
                let _ = tx.try_send(Reply::ContentHeight(8000));
                take_snapshot(wv, w, h, title, url, tx.clone());
            });
        }
        {
            let tx = reply_tx.clone();
            webview.connect_load_failed(move |_wv, _event, uri, error| {
                eprintln!("[webkit] load-failed: {uri} — {error}");
                let _ = tx.try_send(Reply::LoadFailed(error.to_string()));
                false
            });
        }
        {
            let tx = reply_tx.clone();
            webview.connect_title_notify(move |wv| {
                if let Some(t) = wv.title() {
                    let _ = tx.try_send(Reply::TitleChanged(t.to_string()));
                }
            });
        }

        eprintln!("[webkit] Engine created ({}×{})", content_w, content_h);

        let engine = WebKitEngine {
            cmd_tx,
            reply_rx,
            cache:    None,
            title:    None,
            url:      None,
            events:   Vec::new(),
            scroll_y: 0,
        };

        let driver = WebKitDriver {
            cmd_rx,
            reply_tx,
            webview,
            _window: window,
            w: content_w,
            h: content_h,
        };

        Ok((engine, driver))
    }
}

impl Drop for WebKitEngine {
    fn drop(&mut self) { let _ = self.cmd_tx.try_send(Cmd::Shutdown); }
}

impl ContentEngine for WebKitEngine {
    fn navigate(&mut self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[webkit] navigate → {url}");
        self.cache    = None;
        self.url      = Some(url.to_string());
        self.scroll_y = 0;
        self.events.push(EngineEvent::LoadStarted);
        self.cmd_tx.send(Cmd::Navigate(url.to_string()))
            .map_err(|e| format!("webkit channel closed: {e}"))?;
        Ok(())
    }

    fn go_back(&mut self)    -> Result<(), Box<dyn std::error::Error>> { Ok(()) }
    fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> { Ok(()) }

    fn reload(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(url) = self.url.clone() { self.navigate(&url)?; }
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

        // Cairo ARGB32 on little-endian: memory order per pixel = [B, G, R, A]
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
                Ok(Reply::FrameReady { pixels, width, height, title, url }) => {
                    eprintln!("[webkit] Frame ready: {}×{} ({} bytes)", width, height, pixels.len());
                    self.cache = Some(CachedFrame { pixels, width, height });
                    self.title = Some(title.clone());
                    self.url   = Some(url.clone());
                    self.events.push(EngineEvent::TitleChanged(title));
                    self.events.push(EngineEvent::UrlChanged(url));
                    self.events.push(EngineEvent::LoadComplete);
                    self.events.push(EngineEvent::ContentHeightChanged(height));
                }
                Ok(Reply::TitleChanged(t)) => {
                    self.title = Some(t.clone());
                    self.events.push(EngineEvent::TitleChanged(t));
                }
                Ok(Reply::UrlChanged(u)) => {
                    self.url = Some(u.clone());
                    self.events.push(EngineEvent::UrlChanged(u));
                }
                Ok(Reply::ContentHeight(h)) => {
                    self.events.push(EngineEvent::ContentHeightChanged(h));
                }
                Ok(Reply::LoadFailed(r)) => {
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
    /// Pump pending GTK events and dispatch queued commands to WebView.
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
                Ok(Cmd::Navigate(url)) => {
                    eprintln!("[webkit] load_uri: {url}");
                    self.webview.load_uri(&url);
                }
                Ok(Cmd::ScrollTo(y)) => {
                    eprintln!("[webkit] scroll_to: {y}");
                    let script = format!("window.scrollTo(0, {y})");
                    self.webview.run_javascript(&script, None::<&gio::Cancellable>, |_| {});
                    let wv  = self.webview.clone();
                    let tx  = self.reply_tx.clone();
                    let w   = self.w;
                    let h   = self.h;
                    glib::timeout_add_local(Duration::from_millis(150), move || {
                        let title = wv.title().map(|s| s.to_string()).unwrap_or_default();
                        let url   = wv.uri().map(|s| s.to_string()).unwrap_or_default();
                        take_snapshot(&wv, w, h, title, url, tx.clone());
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
    wv:    &webkit2gtk::WebView,
    w:     u32,
    h:     u32,
    title: String,
    url:   String,
    tx:    mpsc::SyncSender<Reply>,
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
                    let _ = tx.try_send(Reply::LoadFailed(e.to_string()));
                }
                Ok(src_surface) => {
                    let mut img = match cairo::ImageSurface::create(
                        cairo::Format::ARgb32, w as i32, h as i32,
                    ) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = tx.try_send(Reply::LoadFailed(format!("cairo create: {e:?}")));
                            return;
                        }
                    };
                    {
                        let ctx = match cairo::Context::new(&img) {
                            Ok(c) => c,
                            Err(e) => {
                                let _ = tx.try_send(Reply::LoadFailed(format!("cairo ctx: {e:?}")));
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
                            let _ = tx.try_send(Reply::LoadFailed(format!("cairo borrow: {e:?}")));
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

                    eprintln!("[webkit] sending frame: {} bytes", pixels.len());
                    let _ = tx.try_send(Reply::FrameReady { pixels, width: sw, height: sh, title, url });
                }
            }
        },
    );
}
