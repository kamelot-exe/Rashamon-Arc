//! Rashamon Net — network process with built-in ad blocking.
//!
//! Responsibilities:
//! - All HTTP/HTTPS requests go through this process
//! - Ad/tracker blocking at the network level
//! - Cookie and cache management
//! - Per-origin storage isolation

mod adblock;
mod http_client;

pub use adblock::AdblockEngine;
pub use http_client::HttpClient;
