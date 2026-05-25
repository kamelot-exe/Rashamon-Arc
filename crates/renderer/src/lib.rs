//! Rashamon Renderer — browser rendering engine integration.
//!
//! Default: WebKitGTK (feature = "webkit") — real web rendering.
//! Fallback: stub / text renderer when webkit feature is disabled.

mod engine;
mod engine_trait;
pub mod framebuffer;
mod permissions;
mod platform;
#[cfg(feature = "servo")]
mod servo_embedder;
#[cfg(not(feature = "servo"))]
mod servo_host;

#[cfg(feature = "webkit")]
mod webkit_engine;

pub use engine::RenderEngine;
pub use engine_trait::{ContentEngine, CursorKind, EngineEvent, EngineFrame, EnginePerfStats};
pub use framebuffer::Framebuffer;
pub use permissions::{
    origin_from_url, DecisionSource, PermissionDecision, PermissionKind, PermissionStore,
};
#[cfg(feature = "webkit")]
pub use webkit_engine::download_destination_for_test;
#[cfg(feature = "webkit")]
pub use webkit_engine::{
    webext_blocked_events_for_test, webext_is_available_for_test, webext_is_configured_for_test,
    webext_is_disabled_for_test, webext_ready_for_test,
};
#[cfg(not(feature = "webkit"))]
pub fn download_destination_for_test(filename: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(filename)
}
#[cfg(not(feature = "webkit"))]
pub fn webext_is_configured_for_test() -> bool { false }
#[cfg(not(feature = "webkit"))]
pub fn webext_is_available_for_test() -> bool { false }
#[cfg(not(feature = "webkit"))]
pub fn webext_is_disabled_for_test() -> bool { false }
#[cfg(not(feature = "webkit"))]
pub fn webext_ready_for_test() -> bool { false }
#[cfg(not(feature = "webkit"))]
pub fn webext_blocked_events_for_test() -> u64 { 0 }
