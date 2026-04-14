//! Rashamon Sandbox — capability-based process isolation.
//!
//! Provides primitives for:
//! - Per-tab process sandboxing
//! - Filesystem access restriction
//! - Clipboard access gating
//! - Permission prompts

mod capabilities;
mod seccomp;

pub use capabilities::{Capability, CapabilitySet};
pub use seccomp::install_seccomp_profile;
