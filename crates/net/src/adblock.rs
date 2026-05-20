//! Minimal built-in ad/tracker blocking engine.
//!
//! This is intentionally not an EasyList/uBlock implementation yet. The v0.2
//! foundation supports fast domain matching, per-domain allowlisting, and simple
//! URL substring rules for patterns that include a path.

use std::collections::HashSet;

/// A single adblock rule.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The raw filter text.
    pub text: String,
    /// Whether this is a third-party rule.
    #[allow(dead_code)]
    pub third_party: bool,
    /// Match type.
    pub kind: RuleKind,
}

#[derive(Debug, Clone)]
pub enum RuleKind {
    /// Host equals the domain, or is a subdomain of it.
    Domain(String),
    /// URL substring match, used for path-sensitive starter rules.
    Substring(String),
}

/// The adblock engine — holds all rules and evaluates requests.
pub struct AdblockEngine {
    rules: Vec<Rule>,
    /// Domains that are always allowed.
    allowlist: HashSet<String>,
    enabled: bool,
    /// Stats.
    blocked_count: u64,
    total_count: u64,
}

impl AdblockEngine {
    pub fn new() -> Self {
        let mut engine = Self {
            rules: Vec::new(),
            allowlist: HashSet::new(),
            enabled: true,
            blocked_count: 0,
            total_count: 0,
        };
        engine.load_default_rules();
        engine
    }

    /// Load built-in default rules (common ad/tracker domains).
    fn load_default_rules(&mut self) {
        let default_rules = [
            "doubleclick.net",
            "googlesyndication.com",
            "google-analytics.com",
            "googletagmanager.com",
            "facebook.net",
            "facebook.com/tr",
            "adsystem.com",
            "adservice.google.com",
            "scorecardresearch.com",
        ];

        for rule in &default_rules {
            self.add_block_rule(rule);
        }
    }

    /// Load additional lightweight rules from text.
    ///
    /// Supported syntax:
    /// - blank lines and `!` comments are ignored
    /// - `@@domain.tld` adds a domain allowlist rule
    /// - lines without `/` are domain rules
    /// - lines with `/` are URL substring rules
    pub fn load_rules_from_text(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('!') {
                continue;
            }
            if line.starts_with("@@") {
                self.allowlist_domain(line.trim_start_matches("@@"));
                continue;
            }
            self.add_block_rule(line);
        }
    }

    /// Check if a request should be blocked.
    pub fn should_block(&mut self, url: &str, origin: &str) -> (bool, Option<String>) {
        self.total_count += 1;
        if !self.enabled {
            return (false, None);
        }

        let url_lc = url.to_ascii_lowercase();
        let origin_lc = origin.to_ascii_lowercase();
        let url_host = extract_host(&url_lc);
        let origin_host = extract_host(&origin_lc);
        if self.allowlist.iter().any(|domain| {
            host_matches_domain(url_host.as_deref(), domain)
                || host_matches_domain(origin_host.as_deref(), domain)
        }) {
            return (false, None);
        }

        for rule in &self.rules {
            let matches = match &rule.kind {
                RuleKind::Domain(domain) => host_matches_domain(url_host.as_deref(), domain),
                RuleKind::Substring(pattern) => url_lc.contains(pattern),
            };
            if matches {
                self.blocked_count += 1;
                return (true, Some(rule.text.clone()));
            }
        }

        (false, None)
    }

    pub fn add_block_rule(&mut self, rule_text: &str) {
        let text = rule_text.trim().to_ascii_lowercase();
        if text.is_empty() {
            return;
        }
        let kind = if text.contains('/') {
            RuleKind::Substring(text.clone())
        } else {
            RuleKind::Domain(normalize_domain(&text))
        };
        self.rules.push(Rule { text, third_party: true, kind });
    }

    pub fn allowlist_domain(&mut self, domain: &str) {
        let domain = normalize_domain(domain);
        if !domain.is_empty() {
            self.allowlist.insert(domain);
        }
    }

    pub fn remove_allowlist_domain(&mut self, domain: &str) {
        self.allowlist.remove(&normalize_domain(domain));
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Backward-compatible helper for the text fallback client.
    pub fn toggle_rule(&mut self, rule_text: &str) {
        let normalized = rule_text.trim().to_ascii_lowercase();
        if let Some(pos) = self.rules.iter().position(|rule| rule.text == normalized) {
            self.rules.remove(pos);
        } else {
            self.add_block_rule(&normalized);
        }
    }

    pub fn blocked_count(&self) -> u64 {
        self.blocked_count
    }

    pub fn total_count(&self) -> u64 {
        self.total_count
    }
}

fn host_matches_domain(host: Option<&str>, domain: &str) -> bool {
    let Some(host) = host else { return false; };
    host == domain || host.ends_with(&format!(".{domain}"))
}

fn normalize_domain(input: &str) -> String {
    let mut host = extract_host(input).unwrap_or_else(|| input.to_ascii_lowercase());
    if let Some((before_path, _)) = host.split_once('/') {
        host = before_path.to_string();
    }
    if let Some((before_port, port)) = host.rsplit_once(':') {
        if port.chars().all(|c| c.is_ascii_digit()) {
            host = before_port.to_string();
        }
    }
    host.trim_start_matches('.').to_string()
}

fn extract_host(input: &str) -> Option<String> {
    let input = input.trim().to_ascii_lowercase();
    if input.is_empty() {
        return None;
    }
    let after_scheme = input
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(input.as_str());
    let after_user = after_scheme.rsplit_once('@').map(|(_, rest)| rest).unwrap_or(after_scheme);
    let host_port = after_user.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host_port
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
        .map(|(host, _)| host)
        .unwrap_or(host_port)
        .trim_matches('.');
    if host.is_empty() { None } else { Some(host.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::AdblockEngine;

    #[test]
    fn blocks_default_domain_and_subdomain() {
        let mut engine = AdblockEngine::new();
        let (blocked, reason) = engine.should_block("https://ad.doubleclick.net/page", "");
        assert!(blocked);
        assert_eq!(reason.as_deref(), Some("doubleclick.net"));
    }

    #[test]
    fn supports_path_substring_rule() {
        let mut engine = AdblockEngine::new();
        let (blocked, reason) = engine.should_block("https://www.facebook.com/tr?id=1", "");
        assert!(blocked);
        assert_eq!(reason.as_deref(), Some("facebook.com/tr"));
    }

    #[test]
    fn allowlist_domain_overrides_block_rule() {
        let mut engine = AdblockEngine::new();
        engine.allowlist_domain("doubleclick.net");
        let (blocked, _) = engine.should_block("https://ad.doubleclick.net/page", "");
        assert!(!blocked);
    }
}
