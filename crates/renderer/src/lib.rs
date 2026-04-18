//! Rashamon Renderer — browser rendering engine integration.
//!
//! Default: WebKitGTK (feature = "webkit") — real web rendering.
//! Fallback: stub / text renderer when webkit feature is disabled.

mod engine;
mod engine_trait;
pub mod framebuffer;
#[cfg(feature = "servo")]
mod servo_embedder;
#[cfg(not(feature = "servo"))]
mod servo_host;

#[cfg(feature = "webkit")]
mod webkit_engine;

pub use engine::RenderEngine;
pub use engine_trait::{ContentEngine, EngineEvent, EngineFrame};
pub use framebuffer::Framebuffer;
