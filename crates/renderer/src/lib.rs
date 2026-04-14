//! Rashamon Renderer — browser rendering engine integration.
//!
//! Supports:
//! - Servo as primary engine
//! - WPE WebKit as fallback research path
//! - Framebuffer-first rendering
//! - Software rendering (GPU abstraction later)

mod engine;
pub mod framebuffer;
mod servo_host;

pub use engine::RenderEngine;
pub use framebuffer::Framebuffer;
pub use servo_host::ServoHost;
