//! Real Servo embedding skeleton.
//!
//! Compiled only when `--features servo` is set.  This file documents the exact
//! integration path and will be filled in as Servo's crate builds on the target.
//!
//! ## Build instructions
//!
//! 1. Add system dependencies (Debian/Ubuntu names):
//!    libgl1-mesa-dev libegl1-mesa-dev libfontconfig1-dev libfreetype6-dev
//!    libssl-dev pkg-config
//!
//! 2. Add to `crates/renderer/Cargo.toml`:
//!    [dependencies]
//!    servo = { git = "https://github.com/servo/servo", rev = "<pin>" }
//!
//!    [features]
//!    servo = ["dep:servo"]
//!
//! 3. Build:
//!    cargo build --release --features rashamon-renderer/servo
//!
//! ## Architecture
//!
//! ```text
//! ServoEmbedder
//!   ├─ ServoGlContext   — SDL2 GL sub-surface for the content area
//!   ├─ servo::Servo<W>  — Servo instance (W = our WindowMethods impl)
//!   ├─ WebViewId        — active top-level browsing context
//!   └─ EventQueue       — pending EngineEvents to deliver to the shell
//! ```
//!
//! ## GL compositing strategy
//!
//! Servo renders via WebRender into an OpenGL FBO.  We own that FBO, read
//! pixels back with `glReadPixels` into `Framebuffer::data`, and let the
//! existing SDL2 blit pipeline present the result alongside the chrome.
//!
//! Once the chrome is also ported to GL quads (later phase), the readback
//! step goes away and Servo renders directly to the display surface.

#![cfg(feature = "servo")]

// ── Servo crate imports (uncomment when the crate is available) ───────────────

// use servo::{Servo, TopLevelBrowsingContextId, LoadStatus};
// use servo::embedder_traits::{EmbedderMsg, EventLoopWaker};
// use servo::webrender_api::units::DeviceIntRect;
// use servo::script_traits::MouseButton;
// use servo::url::ServoUrl;

use crate::engine_trait::{ContentEngine, EngineEvent, EngineFrame};
use crate::framebuffer::Framebuffer;

// ── WindowMethods stub — fill in with SDL2-GL surface details ─────────────────

// struct RashamonWindow {
//     gl:        Rc<dyn gleam::gl::Gl>,     // SDL2 GL context
//     size:      DeviceIntRect,              // content area viewport
//     waker:     Arc<dyn EventLoopWaker>,
// }
//
// impl servo::WindowMethods for RashamonWindow {
//     fn gl(&self) -> Rc<dyn gleam::gl::Gl> { self.gl.clone() }
//
//     fn get_coordinates(&self) -> EmbedderCoordinates {
//         EmbedderCoordinates {
//             viewport:    self.size,
//             framebuffer: self.size.size,
//             window:      (self.size.origin, self.size.size),
//             screen:      self.size.size,
//             screen_avail: self.size.size,
//             hidpi_factor: Scale::new(1.0),
//         }
//     }
//
//     fn set_animation_state(&self, _: AnimationState) {}
//     fn set_fullscreen_state(&self, _: bool) {}
// }

// ── ServoEmbedder ─────────────────────────────────────────────────────────────

pub struct ServoEmbedder {
    // servo:    Servo<RashamonWindow>,
    // view_id:  TopLevelBrowsingContextId,
    // fbo_pixels: Vec<u8>,    // glReadPixels destination
    // events:   Vec<EngineEvent>,
    _marker: std::marker::PhantomData<()>,
}

impl ServoEmbedder {
    pub fn new(_content_w: u32, _content_h: u32) -> Result<Self, Box<dyn std::error::Error>> {
        // 1. Create SDL2 GL context for content area:
        //    let window = SDL2GlWindow::new(content_w, content_h)?;
        //
        // 2. Initialise Servo:
        //    let opts = servo::config::opts::default_opts();
        //    let servo = Servo::new(Arc::new(window), opts, None)?;
        //
        // 3. Open a top-level browsing context:
        //    let view_id = TopLevelBrowsingContextId::new();
        //    servo.create_top_level_browsing_context(
        //        ServoUrl::parse("about:blank")?,
        //        view_id,
        //        None,
        //    );
        //
        // 4. Allocate pixel readback buffer:
        //    let fbo_pixels = vec![0u8; (content_w * content_h * 4) as usize];

        Err("ServoEmbedder not yet built — enable feature 'servo' and add the crate".into())
    }

    /// Drive Servo's internal event loop for one frame.
    ///
    /// Call before `composite_into` each frame.
    pub fn tick(&mut self) {
        // servo.handle_events(vec![]);
        //
        // for msg in servo.get_events() {
        //     match msg {
        //         EmbedderMsg::LoadComplete(_)   => self.events.push(EngineEvent::LoadComplete),
        //         EmbedderMsg::LoadStart(_)       => self.events.push(EngineEvent::LoadStarted),
        //         EmbedderMsg::ChangePageTitle(_, Some(t)) =>
        //             self.events.push(EngineEvent::TitleChanged(t)),
        //         EmbedderMsg::LoadUrl(_, url)   =>
        //             self.events.push(EngineEvent::UrlChanged(url.to_string())),
        //         _ => {}
        //     }
        // }
    }

    /// Read composited pixels from the Servo FBO and blit into `fb`.
    pub fn composite_into(
        &mut self,
        _fb: &mut Framebuffer,
        _x: u32, _y: u32, _w: u32, _h: u32,
    ) -> Result<EngineFrame, Box<dyn std::error::Error>> {
        // servo.recomposite();
        //
        // gl.read_pixels_into_buffer(
        //     0, 0, w as i32, h as i32,
        //     gleam::gl::RGBA, gleam::gl::UNSIGNED_BYTE,
        //     &mut self.fbo_pixels,
        // );
        //
        // // Flip Y (GL origin is bottom-left, our fb is top-left) and blit:
        // for row in 0.._h {
        //     let src_row = _h - 1 - row;
        //     let src = &self.fbo_pixels[(src_row * _w * 4) as usize..][.._w as usize * 4];
        //     for col in 0.._w {
        //         let r = src[col as usize * 4];
        //         let g = src[col as usize * 4 + 1];
        //         let b = src[col as usize * 4 + 2];
        //         _fb.set_pixel(_x + col, _y + row, Pixel { r, g, b });
        //     }
        // }
        //
        // return Ok(EngineFrame::Ready);

        Ok(EngineFrame::NotReady)
    }

    pub fn navigate(&mut self, _url: &str) {
        // servo.load_url(self.view_id, ServoUrl::parse(_url).unwrap());
    }

    pub fn go_back(&mut self) {
        // servo.go_back_in_history(self.view_id, 1);
    }

    pub fn go_forward(&mut self) {
        // servo.go_forward_in_history(self.view_id, 1);
    }

    pub fn reload(&mut self) {
        // servo.reload_current_page(self.view_id);
    }

    pub fn scroll(&mut self, _delta_y: i32) {
        // servo.notify_input_event(
        //     self.view_id,
        //     InputEvent::MouseWheel { delta: WheelDelta::Pixels(Vector2D::new(0.0, _delta_y as f32)) },
        // );
    }

    pub fn drain_events(&mut self) -> Vec<EngineEvent> {
        // std::mem::take(&mut self.events)
        vec![]
    }
}
