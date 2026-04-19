//! ServoHost — content engine implementation.
//!
//! Two compilation paths:
//!
//! 1. `cargo build` (default)   — stub mode: tracks URL/title locally, renders a
//!    wireframe placeholder, always returns `EngineFrame::NotReady` so the browser
//!    shell falls back to its text renderer.
//!
//! 2. `cargo build --features servo` — real Servo embedding via the `servo` crate.
//!    See `servo_embedder.rs` for the full integration skeleton.

use crate::engine_trait::{ContentEngine, EngineEvent, EngineFrame};
use crate::framebuffer::{Framebuffer, Pixel};

// ── Stub implementation (always compiled) ─────────────────────────────────────

pub struct ServoHost {
    title:         Option<String>,
    url:           Option<String>,
    history:       Vec<String>,
    history_index: usize,
    events:        Vec<EngineEvent>,
}

impl ServoHost {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        eprintln!("[renderer] ServoHost: stub mode — compile with --features servo for real rendering");
        Ok(Self {
            title:         None,
            url:           None,
            history:       Vec::new(),
            history_index: 0,
            events:        Vec::new(),
        })
    }

    fn push_url(&mut self, url: &str) {
        if !self.history.is_empty() && self.history_index < self.history.len() - 1 {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(url.to_string());
        self.history_index = self.history.len() - 1;
        self.url   = Some(url.to_string());
        self.title = Some(derive_title(url));
    }
}

fn derive_title(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

impl ContentEngine for ServoHost {
    fn navigate(&mut self, url: &str, _nav_id: u64) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[renderer] navigate -> {url}");
        self.push_url(url);
        self.events.push(EngineEvent::LoadStarted);
        self.events.push(EngineEvent::TitleChanged(self.title.clone().unwrap_or_default()));
        self.events.push(EngineEvent::UrlChanged(url.to_string()));
        Ok(())
    }

    fn go_back(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.history_index > 0 {
            self.history_index -= 1;
            if let Some(u) = self.history.get(self.history_index).cloned() {
                self.url   = Some(u.clone());
                self.title = Some(derive_title(&u));
                self.events.push(EngineEvent::UrlChanged(u));
            }
        }
        Ok(())
    }

    fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.history_index + 1 < self.history.len() {
            self.history_index += 1;
            if let Some(u) = self.history.get(self.history_index).cloned() {
                self.url   = Some(u.clone());
                self.title = Some(derive_title(&u));
                self.events.push(EngineEvent::UrlChanged(u));
            }
        }
        Ok(())
    }

    fn reload(&mut self) -> Result<(), Box<dyn std::error::Error>> { Ok(()) }

    fn scroll(&mut self, _delta_y: i32) {
        // Stub: scroll is managed by BrowserState in text-renderer mode.
    }

    /// Stub render — draws a wireframe placeholder and returns `NotReady` so the
    /// browser shell continues using its text renderer.
    ///
    /// When real Servo is integrated this method calls into `servo_embedder` and
    /// returns `EngineFrame::Ready` once the compositor has written pixels.
    fn render_into(
        &mut self,
        _fb: &mut Framebuffer,
        _x:  u32,
        _y:  u32,
        _w:  u32,
        _h:  u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>> {
        // Real Servo path (disabled until --features servo):
        //
        //   #[cfg(feature = "servo")]
        //   return self.embedder.composite_into(_fb, _x, _y, _w, _h);
        //
        // Until then, signal to the shell that it should use its fallback renderer.
        Ok(EngineFrame::NotReady)
    }

    fn poll_events(&mut self) -> Vec<EngineEvent> {
        std::mem::take(&mut self.events)
    }

    fn title(&self)       -> Option<String> { self.title.clone() }
    fn current_url(&self) -> Option<String> { self.url.clone() }
}

// ── Pixel helper (used by real embedder when it draws diagnostic overlays) ───

#[allow(dead_code)]
fn fill_content_bg(fb: &mut Framebuffer, y: u32, h: u32) {
    fb.fill_rect(0, y, fb.width, h, Pixel::WHITE);
}
