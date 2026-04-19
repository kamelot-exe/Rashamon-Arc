//! Real Servo embedding — compiled only when `--features servo` is active.
//!
//! ## Build requirements (one-time, ~45 min first build)
//!
//! System packages (Debian/Ubuntu):
//!   sudo apt install libssl-dev pkg-config python3 libfontconfig1-dev libfreetype6-dev
//!   # Ensure llvm-objdump is on PATH (SpiderMonkey build requirement):
//!   sudo ln -s /usr/bin/llvm-objdump-20 /usr/local/bin/llvm-objdump
//!   # Build artifacts must NOT go to a path with spaces or non-ASCII:
//!   export CARGO_TARGET_DIR=/tmp/ra_target
//!
//! Build command:
//!   CARGO_TARGET_DIR=/tmp/ra_target cargo build --release \
//!       --package rashamon-ui --features rashamon-renderer/servo
//!
//! ## Architecture
//!
//! Servo post-2024 embedding API:
//!   ServoBuilder::default().build()          → Servo
//!   SoftwareRenderingContext::new(size)       → offscreen software renderer
//!   WebView::new(&servo, rc).delegate().build() → WebView
//!   servo.spin_event_loop()                   → drive internals
//!   webview.paint()                           → trigger composite
//!   rendering_context.read_to_image(rect)     → Option<RgbaImage> (RGBA, top-left)
//!
//! Threading: Servo uses Rc and is !Send — it lives on its own OS thread.
//! The main thread communicates via mpsc channels.

#![cfg(feature = "servo")]

use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::engine_trait::{ContentEngine, EngineEvent, EngineFrame};
use crate::framebuffer::{Framebuffer, Pixel};

// ── IPC between main thread and Servo thread ──────────────────────────────────

enum ServoCmd {
    Navigate(String),
    GoBack,
    GoForward,
    Reload,
    Tick,       // request a composite + pixel readback
    Shutdown,
}

struct ServoFrame {
    pixels: Vec<u8>,   // RGBA, top-left origin
    width:  u32,
    height: u32,
    title:  String,
    url:    String,
}

enum ServoReply {
    FrameReady(ServoFrame),
    TitleChanged(String),
    UrlChanged(String),
    LoadStarted,
    LoadComplete,
    LoadFailed(String),
}

// ── ServoHost — ContentEngine backed by the Servo thread ─────────────────────

pub struct ServoHost {
    cmd_tx:   mpsc::SyncSender<ServoCmd>,
    reply_rx: mpsc::Receiver<ServoReply>,
    cache:    Option<ServoFrame>,
    title:    Option<String>,
    url:      Option<String>,
    events:   Vec<EngineEvent>,
}

impl ServoHost {
    pub fn new(content_w: u32, content_h: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let (cmd_tx, cmd_rx)     = mpsc::sync_channel::<ServoCmd>(8);
        let (reply_tx, reply_rx) = mpsc::sync_channel::<ServoReply>(8);

        thread::Builder::new()
            .name("servo".into())
            .spawn(move || servo_thread(cmd_rx, reply_tx, content_w, content_h))?;

        eprintln!("[servo] ServoHost spawned ({}×{})", content_w, content_h);
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

impl Drop for ServoHost {
    fn drop(&mut self) { let _ = self.cmd_tx.try_send(ServoCmd::Shutdown); }
}

impl ContentEngine for ServoHost {
    fn navigate(&mut self, url: &str, _nav_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[servo] navigate → {url}");
        self.cache = None;
        self.url   = Some(url.to_string());
        self.events.push(EngineEvent::LoadStarted);
        self.cmd_tx.send(ServoCmd::Navigate(url.to_string()))
            .map_err(|e| format!("servo channel closed: {e}"))?;
        Ok(())
    }

    fn go_back(&mut self)    -> Result<(), Box<dyn std::error::Error>> {
        let _ = self.cmd_tx.try_send(ServoCmd::GoBack); Ok(())
    }
    fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let _ = self.cmd_tx.try_send(ServoCmd::GoForward); Ok(())
    }
    fn reload(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let _ = self.cmd_tx.try_send(ServoCmd::Reload); Ok(())
    }
    fn scroll(&mut self, _delta_y: i32) { /* TODO: Servo input API */ }

    fn render_into(
        &mut self,
        fb:  &mut Framebuffer,
        x:   u32, y: u32, w: u32, h: u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>> {
        // Ask Servo to composite and readback this frame.
        let _ = self.cmd_tx.try_send(ServoCmd::Tick);

        let Some(frame) = &self.cache else {
            return Ok(EngineFrame::NotReady);
        };

        eprintln!("[servo] Servo frame ready — blitting {}×{} framebuffer size {}",
            frame.width, frame.height, frame.pixels.len());

        let src_w = frame.width;
        let src_h = frame.height;
        let rows  = h.min(src_h);
        let cols  = w.min(src_w);

        // SoftwareRenderingContext → read_to_image() → RGBA, top-left origin.
        for row in 0..rows {
            for col in 0..cols {
                let s = ((row * src_w) + col) as usize * 4;
                if s + 2 < frame.pixels.len() {
                    let r = frame.pixels[s];
                    let g = frame.pixels[s + 1];
                    let b = frame.pixels[s + 2];
                    fb.set_pixel(x + col, y + row, Pixel { r, g, b });
                }
            }
        }
        Ok(EngineFrame::Ready)
    }

    fn poll_events(&mut self) -> Vec<EngineEvent> {
        loop {
            match self.reply_rx.try_recv() {
                Ok(ServoReply::FrameReady(frame)) => {
                    eprintln!("[servo] Frame ready: {}×{} ({} bytes)",
                        frame.width, frame.height, frame.pixels.len());
                    self.title  = Some(frame.title.clone());
                    self.url    = Some(frame.url.clone());
                    self.events.push(EngineEvent::TitleChanged(frame.title.clone()));
                    self.events.push(EngineEvent::UrlChanged(frame.url.clone()));
                    self.events.push(EngineEvent::LoadComplete);
                    self.events.push(EngineEvent::ContentHeightChanged(frame.height));
                    self.cache  = Some(frame);
                }
                Ok(ServoReply::TitleChanged(t)) => {
                    self.title = Some(t.clone());
                    self.events.push(EngineEvent::TitleChanged(t));
                }
                Ok(ServoReply::UrlChanged(u)) => {
                    self.url = Some(u.clone());
                    self.events.push(EngineEvent::UrlChanged(u));
                }
                Ok(ServoReply::LoadStarted)   => { self.events.push(EngineEvent::LoadStarted); }
                Ok(ServoReply::LoadComplete)  => { self.events.push(EngineEvent::LoadComplete); }
                Ok(ServoReply::LoadFailed(r)) => {
                    eprintln!("[servo] Load failed: {r}");
                    self.events.push(EngineEvent::LoadFailed(r));
                }
                Err(_) => break,
            }
        }
        std::mem::take(&mut self.events)
    }

    fn title(&self)       -> Option<String> { self.title.clone() }
    fn current_url(&self) -> Option<String> { self.url.clone() }
}

// ── Servo thread ──────────────────────────────────────────────────────────────
//
// Servo is !Send (uses Rc).  It lives entirely on this thread.

fn servo_thread(
    cmd_rx:   mpsc::Receiver<ServoCmd>,
    reply_tx: mpsc::SyncSender<ServoReply>,
    w: u32,
    h: u32,
) {
    use servo::{
        DeviceIntRect,
        DeviceIntSize,
        RenderingContext,
        ServoBuilder,
        SoftwareRenderingContext,
        ServoUrl,
        UrlRequest,
        WebView,
        WebViewDelegate,
        LoadStatus,
    };
    use dpi::PhysicalSize;

    eprintln!("[servo] thread starting {}×{}", w, h);

    // ── Software rendering context (no display/GPU required) ─────────────────
    let rc = match SoftwareRenderingContext::new(PhysicalSize::new(w, h)) {
        Ok(r)  => Rc::new(r),
        Err(e) => {
            eprintln!("[servo] SoftwareRenderingContext failed: {e:?}");
            let _ = reply_tx.try_send(ServoReply::LoadFailed(format!("{e:?}")));
            return;
        }
    };

    // ── Build Servo ───────────────────────────────────────────────────────────
    let servo = ServoBuilder::default().build();

    // ── Delegate: receives load/title/url events ──────────────────────────────
    struct Delegate { tx: mpsc::SyncSender<ServoReply> }
    impl WebViewDelegate for Delegate {
        fn notify_load_status_changed(&self, _wv: WebView, status: LoadStatus) {
            let reply = match status {
                LoadStatus::Started  => ServoReply::LoadStarted,
                LoadStatus::Complete => ServoReply::LoadComplete,
                LoadStatus::Failed   => ServoReply::LoadFailed("load failed".into()),
            };
            let _ = self.tx.try_send(reply);
        }
        fn notify_page_title_changed(&self, _wv: WebView, title: Option<String>) {
            if let Some(t) = title { let _ = self.tx.try_send(ServoReply::TitleChanged(t)); }
        }
        fn notify_url_changed(&self, _wv: WebView, url: url::Url) {
            let _ = self.tx.try_send(ServoReply::UrlChanged(url.to_string()));
        }
    }

    // ── Create WebView with software rendering context ────────────────────────
    let webview: WebView = WebView::new(&servo, rc.clone())
        .delegate(Rc::new(Delegate { tx: reply_tx.clone() }))
        .build();

    webview.show();
    webview.focus();
    eprintln!("[servo] WebView ready, entering command loop");

    let tick_sleep = Duration::from_millis(4);
    let mut frame_needed = false;

    loop {
        // Drain commands from main thread.
        loop {
            match cmd_rx.try_recv() {
                Ok(ServoCmd::Navigate(url)) => {
                    eprintln!("[servo] load_request: {url}");
                    match ServoUrl::parse(&url) {
                        Ok(su) => {
                            let req = UrlRequest::new(su.into_url());
                            webview.load_request(req);
                            let _ = reply_tx.try_send(ServoReply::LoadStarted);
                        }
                        Err(e) => {
                            let _ = reply_tx.try_send(ServoReply::LoadFailed(e.to_string()));
                        }
                    }
                }
                Ok(ServoCmd::GoBack)    => { webview.go_back(1); }
                Ok(ServoCmd::GoForward) => { webview.go_forward(1); }
                Ok(ServoCmd::Reload)    => { webview.reload(); }
                Ok(ServoCmd::Tick)      => { frame_needed = true; }
                Ok(ServoCmd::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => {
                    eprintln!("[servo] shutting down");
                    servo.start_shutting_down();
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }

        // Drive Servo's internal event loop (single tick).
        servo.spin_event_loop();

        // On demand: paint and read pixels.
        if frame_needed {
            frame_needed = false;
            webview.paint();

            let rect = DeviceIntRect::from_size(DeviceIntSize::new(w as i32, h as i32));
            if let Some(img) = rc.read_to_image(rect) {
                let url   = webview.url().map(|u| u.to_string()).unwrap_or_default();
                let frame = ServoFrame {
                    pixels: img.into_raw(),   // Vec<u8>, RGBA
                    width:  w,
                    height: h,
                    title:  String::new(),    // title arrives via delegate
                    url,
                };
                eprintln!("[servo] glReadPixels OK → {} bytes", frame.pixels.len());
                let _ = reply_tx.try_send(ServoReply::FrameReady(frame));
            }
        }

        thread::sleep(tick_sleep);
    }
}
