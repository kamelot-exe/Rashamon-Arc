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

    /// Perform a real GET request and return the response body as text.
    ///
    /// Uses `curl` as a subprocess (the crate has no async runtime or TLS
    /// dependency today; curl is available on KamelotOS).
    ///
    /// Returns `Err(reason)` on:
    ///   - adblock block
    ///   - curl not found / network failure
    ///   - HTTP 4xx/5xx
    pub fn fetch_text(&mut self, url: &str) -> Result<String, String> {
        use std::process::Command;

        // Adblock gate
        let (blocked, reason) = self.adblock.should_block(url, "");
        if blocked {
            return Err(format!("Blocked: {}", reason.unwrap_or_default()));
        }

        // Sentinel written by curl's --write-out so we can split status code
        // from the body without relying on headers.
        // \x04 (ASCII EOT) never appears in HTML, making the split unambiguous.
        const SENTINEL: &str = "\x04RASHAMON_STATUS:";

        let out = Command::new("curl")
            .args([
                "--silent",
                "--location",               // follow redirects
                "--max-time",    "12",       // 12 s wall-clock timeout
                "--max-filesize","4194304",  // 4 MB cap
                "--compressed",             // accept gzip/br if server sends it
            ])
            .args(["--user-agent",
                "Mozilla/5.0 (X11; Linux x86_64) Rashamon/0.1"])
            .args(["--write-out",
                &format!("{SENTINEL}%{{http_code}}")])
            .arg(url)
            .output()
            .map_err(|_| "curl is not available on this system".to_string())?;

        // curl exits non-zero on network errors; stdout is empty then.
        if out.stdout.is_empty() {
            return Err(match out.status.code() {
                Some(6)  => "Could not resolve host (DNS failure)".into(),
                Some(7)  => "Failed to connect to server".into(),
                Some(28) => "Request timed out".into(),
                Some(35) => "SSL/TLS handshake failed".into(),
                Some(n)  => format!("curl error {n}"),
                None     => "Connection failed".into(),
            });
        }

        let raw = String::from_utf8_lossy(&out.stdout);

        if let Some(sep) = raw.rfind(SENTINEL) {
            let status: u16 = raw[sep + SENTINEL.len()..].trim().parse().unwrap_or(0);
            let body = raw[..sep].to_string();

            if status == 0 && body.trim().is_empty() {
                return Err("Connection failed (no response)".into());
            }
            if status >= 400 {
                return Err(match status {
                    404 => "Page not found (HTTP 404)".into(),
                    403 => "Access denied (HTTP 403)".into(),
                    500..=599 => format!("Server error (HTTP {status})"),
                    _ => format!("HTTP error {status}"),
                });
            }

            eprintln!("[http] {} OK — {} bytes", url, body.len());
            Ok(body)
        } else {
            // Sentinel missing: curl may have been interrupted after partial output.
            if out.status.success() {
                Ok(raw.into_owned())
            } else {
                Err("Connection failed".into())
            }
        }
    }

    pub fn adblock_stats(&self) -> (u64, u64) {
        (self.adblock.blocked_count(), self.adblock.total_count())
    }

    pub fn adblock_toggle(&mut self, rule: &str) {
        self.adblock.toggle_rule(rule);
    }
}
