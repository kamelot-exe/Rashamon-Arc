//! HTTP client — handles all network requests.
//!
//! For the MVP, uses curl via a subprocess.
//! In production, replace with hyper + rustls for full control.

use crate::adblock::AdblockEngine;
use rashamon_ipc::{NetworkRequest, NetworkResponse};

/// The HTTP client with integrated adblocking.
pub struct HttpClient {
    adblock: AdblockEngine,
}

impl HttpClient {
    pub fn new() -> Self {
        Self {
            adblock: AdblockEngine::new(),
        }
    }

    /// Execute a network request, potentially blocking it.
    pub fn execute(&mut self, req: NetworkRequest) -> NetworkResponse {
        // Check adblock rules.
        let (blocked, block_reason) = self.adblock.should_block(&req.url, &req.origin);
        if blocked {
            eprintln!("[http] BLOCKED: {} ({})", req.url, block_reason.as_deref().unwrap_or(""));
            return NetworkResponse {
                status: 0,
                headers: vec![],
                body: vec![],
                blocked: true,
                block_reason,
            };
        }

        // In production: use hyper + rustls here.
        // For now, return a stub response.
        eprintln!("[http] {} {}", req.method, req.url);

        NetworkResponse {
            status: 200,
            headers: vec![
                ("content-type".to_string(), "text/html; charset=utf-8".to_string()),
            ],
            body: b"<html><body><h1>Rashamon Arc</h1><p>Network layer active. Adblock: OK</p></body></html>".to_vec(),
            blocked: false,
            block_reason: None,
        }
    }

    pub fn adblock_stats(&self) -> (u64, u64) {
        (self.adblock.blocked_count(), self.adblock.total_count())
    }

    pub fn adblock_toggle(&mut self, rule: &str) {
        self.adblock.toggle_rule(rule);
    }
}
