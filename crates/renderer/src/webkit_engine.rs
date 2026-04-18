//! WebKitGTK content engine — real web rendering via WebKit 2.50+.
//!
//! Architecture:
//!   Main thread  ←  mpsc channels  →  GTK thread
//!   (SDL2 loop)                       (glib MainLoop + WebView)
//!
//! navigate() → sends Cmd::Navigate to GTK thread.
//! GTK thread loads the page, waits for load-finished, takes a snapshot.
//! Snapshot pixels (Cairo ARGB32 = BGRA in memory) are sent back via channel.
//! poll_events() drains the channel → emits EngineEvents.
//! render_into() blits the cached pixel buffer into the framebuffer region.

use crate::engine_trait::{ContentEngine, EngineEvent, EngineFrame};
use crate::framebuffer::{Framebuffer, Pixel};

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

// ── IPC types ─────────────────────────────────────────────────────────────────

enum Cmd {
    Navigate(String),
    Shutdown,
}

enum Reply {
    FrameReady {
        /// Raw BGRA pixels: Cairo ARGB32, little-endian → [B,G,R,A] in memory.
        pixels: Vec<u8>,
        width:  u32,
        height: u32,
        title:  String,
        url:    String,
    },
    TitleChanged(String),
    UrlChanged(String),
    LoadFailed(String),
}

// ── Cached frame ──────────────────────────────────────────────────────────────

struct CachedFrame {
    pixels: Vec<u8>,
    width:  u32,
    height: u32,
}

// ── Engine ────────────────────────────────────────────────────────────────────

pub struct WebKitEngine {
    cmd_tx:   mpsc::SyncSender<Cmd>,
    reply_rx: mpsc::Receiver<Reply>,
    cache:    Option<CachedFrame>,
    title:    Option<String>,
    url:      Option<String>,
    events:   Vec<EngineEvent>,
}

impl WebKitEngine {
    pub fn new(content_w: u32, content_h: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let (cmd_tx, cmd_rx)     = mpsc::sync_channel::<Cmd>(8);
        let (reply_tx, reply_rx) = mpsc::sync_channel::<Reply>(8);

        thread::Builder::new()
            .name("webkit-gtk".into())
            .spawn(move || webkit_thread(cmd_rx, reply_tx, content_w, content_h))?;

        eprintln!("[webkit] Engine spawned ({}×{})", content_w, content_h);
        Ok(Self {
            cmd_tx,
            reply_rx,
            cache:  None,
            title:  None,
            url:    None,
            events: Vec::new(),
        })
    }
}

impl Drop for WebKitEngine {
    fn drop(&mut self) { let _ = self.cmd_tx.try_send(Cmd::Shutdown); }
}

impl ContentEngine for WebKitEngine {
    fn navigate(&mut self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[webkit] navigate → {url}");
        self.cache  = None;
        self.url    = Some(url.to_string());
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

    fn scroll(&mut self, _delta_y: i32) {
        // Snapshot approach: scroll is handled by BrowserState's node-scroll code.
        // Future: send Cmd::Scroll → retake snapshot at new offset.
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

// ── GTK / WebKit thread ───────────────────────────────────────────────────────

fn webkit_thread(
    cmd_rx:   mpsc::Receiver<Cmd>,
    reply_tx: mpsc::SyncSender<Reply>,
    w: u32,
    h: u32,
) {
    use gtk::prelude::{ContainerExt, GtkWindowExt, WidgetExt};
    use webkit2gtk::{
        HardwareAccelerationPolicy, LoadEvent, Settings, SettingsExt,
        SnapshotOptions, SnapshotRegion, WebView, WebViewExt,
    };
    // cairo is the rendering crate — needed for ImageSurface pixel readback.
    use cairo;

    if let Err(e) = gtk::init() {
        eprintln!("[webkit] GTK init failed: {e}");
        return;
    }
    eprintln!("[webkit] GTK thread running, WebView {}×{}", w, h);

    let main_loop = glib::MainLoop::new(None, false);

    // Disable hardware accel so offscreen rendering always works
    let settings = Settings::new();
    settings.set_enable_webgl(false);
    settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never);

    let webview = WebView::new();
    webview.set_settings(&settings);

    // OffscreenWindow renders to a pixmap — no display window appears
    let window = gtk::OffscreenWindow::new();
    window.set_default_size(w as i32, h as i32);
    window.add(&webview);
    window.show_all();

    // ── load-changed: snapshot on finish ─────────────────────────────────────
    {
        let tx = reply_tx.clone();
        webview.connect_load_changed(move |wv, event| {
            if event != LoadEvent::Finished { return; }

            let title = wv.title().map(|s| s.to_string()).unwrap_or_default();
            let url   = wv.uri().map(|s| s.to_string()).unwrap_or_default();
            eprintln!("[webkit] load-finished: {url:?}  title={title:?}");

            let tx = tx.clone();
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
                            // snapshot() returns a generic cairo::Surface.
                            // Copy it into an ImageSurface we own so we can read pixels.
                            let mut img = match cairo::ImageSurface::create(
                                cairo::Format::ARgb32, w as i32, h as i32,
                            ) {
                                Ok(s) => s,
                                Err(e) => {
                                    eprintln!("[webkit] ImageSurface::create error: {e:?}");
                                    let _ = tx.try_send(Reply::LoadFailed(
                                        format!("cairo create: {e:?}"),
                                    ));
                                    return;
                                }
                            };

                            // Paint the snapshot onto our ImageSurface.
                            {
                                let ctx = match cairo::Context::new(&img) {
                                    Ok(c) => c,
                                    Err(e) => {
                                        let _ = tx.try_send(Reply::LoadFailed(
                                            format!("cairo ctx: {e:?}"),
                                        ));
                                        return;
                                    }
                                };
                                let _ = ctx.set_source_surface(&src_surface, 0.0, 0.0);
                                let _ = ctx.paint();
                            } // ctx dropped → flush

                            let sw     = img.width()  as u32;
                            let sh     = img.height() as u32;
                            let stride = img.stride() as u32;
                            eprintln!("[webkit] ImageSurface: {sw}×{sh} stride={stride}");

                            // Extract BGRA bytes (Cairo ARGB32 little-endian = [B,G,R,A]).
                            let pixels: Vec<u8> = match img.data() {
                                Err(e) => {
                                    eprintln!("[webkit] cairo data borrow: {e:?}");
                                    let _ = tx.try_send(Reply::LoadFailed(
                                        format!("cairo borrow: {e:?}"),
                                    ));
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
                            let _ = tx.try_send(Reply::FrameReady {
                                pixels, width: sw, height: sh, title, url,
                            });
                        }
                    }
                },
            );
        });
    }

    // ── load-failed ───────────────────────────────────────────────────────────
    {
        let tx = reply_tx.clone();
        webview.connect_load_failed(move |_wv, _event, uri, error| {
            eprintln!("[webkit] load-failed: {uri} — {error}");
            let _ = tx.try_send(Reply::LoadFailed(error.to_string()));
            false
        });
    }

    // ── title changed ─────────────────────────────────────────────────────────
    {
        let tx = reply_tx.clone();
        webview.connect_title_notify(move |wv| {
            if let Some(t) = wv.title() {
                let _ = tx.try_send(Reply::TitleChanged(t.to_string()));
            }
        });
    }

    // ── Command polling (glib timeout, 8 ms) ──────────────────────────────────
    {
        let wv = webview.clone();
        let ml = main_loop.clone();
        glib::timeout_add_local(Duration::from_millis(8), move || {
            match cmd_rx.try_recv() {
                Ok(Cmd::Navigate(url)) => {
                    eprintln!("[webkit] load_uri: {url}");
                    wv.load_uri(&url);
                }
                Ok(Cmd::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => {
                    ml.quit();
                    return glib::ControlFlow::Break;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            glib::ControlFlow::Continue
        });
    }

    eprintln!("[webkit] entering glib main loop");
    main_loop.run();
    eprintln!("[webkit] glib main loop exited");
}
