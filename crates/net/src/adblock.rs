//! Network-level ad/tracker blocking engine.
//!
//! Uses rule-based matching (EasyList-compatible format).
//! Rules are loaded at startup and evaluated per-request.

use std::collections::HashSet;

/// A single adblock rule.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The raw filter text.
    pub text: String,
    /// Whether this is a third-party rule.
    pub third_party: bool,
    /// Match type.
    pub kind: RuleKind,
}

#[derive(Debug, Clone)]
pub enum RuleKind {
    /// URL substring match.
    Contains(String),
    /// Exact domain match.
    Domain(String),
    /// Regex match.
    Regex(String),
}

/// The adblock engine — holds all rules and evaluates requests.
pub struct AdblockEngine {
    rules: Vec<Rule>,
    /// Domains that are always allowed (whitelist).
    whitelist: HashSet<String>,
    /// Stats.
    blocked_count: u64,
    total_count: u64,
}

impl AdblockEngine {
    pub fn new() -> Self {
        let mut engine = Self {
            rules: Vec::new(),
            whitelist: HashSet::new(),
            blocked_count: 0,
            total_count: 0,
        };
        engine.load_default_rules();
        engine
    }

    /// Load built-in default rules (common ad/tracker domains).
    fn load_default_rules(&mut self) {
        // Built-in minimal rule set — common ad/tracker domains.
        let default_domains = [
            "doubleclick.net",
            "googleadservices.com",
            "googlesyndication.com",
            "adservice.google.com",
            "pagead2.googlesyndication.com",
            "adsystem.com",
            "amazon-adsystem.com",
            "facebook.com/tr/",
            "facebook.net",
            "connect.facebook.net",
            "analytics.google.com",
            "google-analytics.com",
            "statcounter.com",
            "scorecardresearch.com",
            "quantserve.com",
            "ads-twitter.com",
            "syndication.twitter.com",
            "ads.linkedin.com",
            "bat.bing.com",
            "ads.yahoo.com",
            "analytics.yahoo.com",
            "pixel.redditmedia.com",
            "events.redditmedia.com",
        ];

        for domain in &default_domains {
            self.rules.push(Rule {
                text: domain.to_string(),
                third_party: true,
                kind: RuleKind::Contains(domain.to_string()),
            });
        }

        eprintln!("[adblock] Loaded {} default rules", default_domains.len());
    }

    /// Load additional rules from a text file (EasyList format).
    pub fn load_rules_from_text(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
                continue;
            }
            if line.starts_with("@@") {
                // Whitelist rule.
                let domain = line.trim_start_matches("@@");
                self.whitelist.insert(domain.to_string());
                continue;
            }
            self.rules.push(Rule {
                text: line.to_string(),
                third_party: line.contains("$third-party"),
                kind: RuleKind::Contains(line.to_string()),
            });
        }
    }

    /// Check if a request should be blocked.
    pub fn should_block(&mut self, url: &str, origin: &str) -> (bool, Option<String>) {
        self.total_count += 1;

        // Check whitelist first.
        if self.whitelist.iter().any(|d| url.contains(d) || origin.contains(d)) {
            return (false, None);
        }

        for rule in &self.rules {
            let matches = match &rule.kind {
                RuleKind::Contains(pattern) => url.contains(pattern),
                RuleKind::Domain(domain) => url.contains(domain),
                RuleKind::Regex(pat) => {
                    regex::Regex::new(pat).map(|r| r.is_match(url)).unwrap_or(false)
                }
            };
            if matches {
                self.blocked_count += 1;
                return (true, Some(rule.text.clone()));
            }
        }

        (false, None)
    }

    /// Toggle a specific rule.
    pub fn toggle_rule(&mut self, rule_text: &str) {
        if let Some(pos) = self.rules.iter().position(|r| r.text == rule_text) {
            self.rules.remove(pos);
        } else {
            self.rules.push(Rule {
                text: rule_text.to_string(),
                third_party: true,
                kind: RuleKind::Contains(rule_text.to_string()),
            });
        }
    }

    pub fn blocked_count(&self) -> u64 {
        self.blocked_count
    }

    pub fn total_count(&self) -> u64 {
        self.total_count
    }
}
