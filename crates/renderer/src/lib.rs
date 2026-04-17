//! Rashamon Renderer — browser rendering engine integration.
//!
//! Default build: stub mode (text renderer fallback active in the shell).
//! `--features servo`: real Servo embedding via WebRender + GL readback.

mod engine;
mod engine_trait;
pub mod framebuffer;
mod servo_embedder;
mod servo_host;

pub use engine::RenderEngine;
pub use engine_trait::{ContentEngine, EngineEvent, EngineFrame};
pub use framebuffer::Framebuffer;
