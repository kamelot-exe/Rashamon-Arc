//! Minimal built-in ad/tracker blocking engine.
//!
//! This is intentionally not an EasyList/uBlock implementation yet. The v0.2
//! foundation supports fast domain matching, per-domain allowlisting, and simple
//! URL substring rules for patterns that include a path.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub const ADBLOCK_RULE_PAYLOAD_VERSION: u32 = 1;

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

/// Compact export model used to sync the current effective rules to the
/// WebExtension path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdblockRulePayload {
    pub version: u32,
    pub enabled: bool,
    pub blocked_domains: Vec<String>,
    pub blocked_substrings: Vec<String>,
    pub allowlist_domains: Vec<String>,
}

impl AdblockRulePayload {
    pub fn to_sync_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("version={}\n", self.version));
        out.push_str(if self.enabled { "enabled=1\n" } else { "enabled=0\n" });
        for domain in &self.blocked_domains {
            push_payload_line(&mut out, "block-domain", domain);
        }
        for pattern in &self.blocked_substrings {
            push_payload_line(&mut out, "block-substring", pattern);
        }
        for domain in &self.allowlist_domains {
            push_payload_line(&mut out, "allow-domain", domain);
        }
        out
    }
}

/// The adblock engine — holds all rules and evaluates requests.
pub struct AdblockEngine {
    rules: Vec<Rule>,
    /// Domains that are always allowed.
    allowlist: HashSet<String>,
    /// Session-only allowlist used by private tabs.
    session_allowlist: HashSet<String>,
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
            session_allowlist: HashSet::new(),
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
        self.should_block_for_context(url, origin, false)
    }

    pub fn should_block_for_context(
        &mut self,
        url: &str,
        origin: &str,
        private: bool,
    ) -> (bool, Option<String>) {
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
        if private && self.session_allowlist.iter().any(|domain| {
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
        self.allowlist_domain_for_context(domain, false);
    }

    pub fn allowlist_domain_for_context(&mut self, domain: &str, private: bool) {
        let domain = normalize_domain(domain);
        if !domain.is_empty() {
            if private {
                self.session_allowlist.insert(domain);
            } else {
                self.allowlist.insert(domain);
            }
        }
    }

    pub fn remove_allowlist_domain(&mut self, domain: &str) {
        self.remove_allowlist_domain_for_context(domain, false);
    }

    pub fn remove_allowlist_domain_for_context(&mut self, domain: &str, private: bool) {
        let domain = normalize_domain(domain);
        if private {
            self.session_allowlist.remove(&domain);
        } else {
            self.allowlist.remove(&domain);
        }
    }

    pub fn is_allowlisted_domain(&self, domain: &str, private: bool) -> bool {
        let domain = normalize_domain(domain);
        self.allowlist.contains(&domain) || (private && self.session_allowlist.contains(&domain))
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn allowlist_entries(&self) -> Vec<String> {
        let mut entries = self.allowlist.iter().cloned().collect::<Vec<_>>();
        entries.sort();
        entries
    }

    pub fn export_rule_payload_for_context(&self, private: bool) -> AdblockRulePayload {
        let mut blocked_domains = Vec::new();
        let mut blocked_substrings = Vec::new();
        for rule in &self.rules {
            match &rule.kind {
                RuleKind::Domain(domain) => blocked_domains.push(domain.clone()),
                RuleKind::Substring(pattern) => blocked_substrings.push(pattern.clone()),
            }
        }

        let mut allowlist_domains = self.allowlist_entries();
        if private {
            allowlist_domains.extend(self.session_allowlist.iter().cloned());
            allowlist_domains.sort();
            allowlist_domains.dedup();
        }

        AdblockRulePayload {
            version: ADBLOCK_RULE_PAYLOAD_VERSION,
            enabled: self.enabled,
            blocked_domains,
            blocked_substrings,
            allowlist_domains,
        }
    }

    pub fn export_rule_sync_text_for_context(&self, private: bool) -> String {
        self.export_rule_payload_for_context(private).to_sync_text()
    }

    pub fn load_allowlist_from_path(&mut self, path: &Path) {
        let Ok(text) = fs::read_to_string(path) else { return };
        self.allowlist.clear();
        for domain in parse_string_array(&text) {
            self.allowlist_domain(&domain);
        }
    }

    pub fn save_allowlist_to_path(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            if fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let mut out = String::from("[\n");
        let entries = self.allowlist_entries();
        for (i, domain) in entries.iter().enumerate() {
            out.push_str("  ");
            out.push_str(&json_str(domain));
            if i + 1 < entries.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("]\n");
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, out).is_ok() && fs::rename(&tmp, path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
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

fn parse_string_array(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_str = false;
    let mut esc = false;
    let mut cur = String::new();
    for ch in text.chars() {
        if in_str {
            if esc {
                cur.push(match ch {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_ascii_lowercase());
                }
                cur.clear();
                in_str = false;
            } else {
                cur.push(ch);
            }
        } else if ch == '"' {
            in_str = true;
        }
    }
    out
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn push_payload_line(out: &mut String, key: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() || value.contains('\n') || value.contains('\r') {
        return;
    }
    out.push_str(key);
    out.push('=');
    out.push_str(value);
    out.push('\n');
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

    #[test]
    fn persisted_allowlist_roundtrips_and_bad_file_is_empty() {
        let path = std::env::temp_dir().join(format!(
            "rashamon-adblock-smoke-{}.json",
            std::process::id(),
        ));
        let _ = std::fs::remove_file(&path);
        let mut engine = AdblockEngine::new();
        engine.allowlist_domain("doubleclick.net");
        engine.save_allowlist_to_path(&path);

        let mut loaded = AdblockEngine::new();
        loaded.load_allowlist_from_path(&path);
        let (blocked, _) = loaded.should_block("https://ad.doubleclick.net/page", "");
        assert!(!blocked);

        std::fs::write(&path, "{not-json").unwrap();
        let mut bad = AdblockEngine::new();
        bad.load_allowlist_from_path(&path);
        let (blocked, _) = bad.should_block("https://ad.doubleclick.net/page", "");
        let _ = std::fs::remove_file(path);
        assert!(blocked);
    }

    #[test]
    fn private_allowlist_is_session_only() {
        let mut engine = AdblockEngine::new();
        engine.allowlist_domain_for_context("doubleclick.net", true);
        let (private_blocked, _) =
            engine.should_block_for_context("https://ad.doubleclick.net/page", "", true);
        let (normal_blocked, _) =
            engine.should_block_for_context("https://ad.doubleclick.net/page", "", false);
        assert!(!private_blocked);
        assert!(normal_blocked);
    }

    #[test]
    fn exports_structured_effective_rules() {
        let mut engine = AdblockEngine::new();
        engine.allowlist_domain("https://example.com:443/path");
        engine.allowlist_domain_for_context("session.example", true);
        engine.set_enabled(false);

        let normal = engine.export_rule_payload_for_context(false);
        assert_eq!(normal.version, 1);
        assert!(!normal.enabled);
        assert!(normal.blocked_domains.iter().any(|d| d == "doubleclick.net"));
        assert!(normal.blocked_substrings.iter().any(|p| p == "facebook.com/tr"));
        assert_eq!(normal.allowlist_domains, vec!["example.com".to_string()]);

        let private = engine.export_rule_payload_for_context(true);
        assert_eq!(
            private.allowlist_domains,
            vec!["example.com".to_string(), "session.example".to_string()]
        );

        let text = private.to_sync_text();
        assert!(text.starts_with("version=1\nenabled=0\n"));
        assert!(text.contains("block-domain=doubleclick.net\n"));
        assert!(text.contains("block-substring=facebook.com/tr\n"));
        assert!(text.contains("allow-domain=session.example\n"));
    }
}
