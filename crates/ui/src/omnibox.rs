//! Omnibox — input classification, URL normalisation, and search routing.

// ── Search provider ───────────────────────────────────────────────────────────

pub struct SearchProvider {
    pub name:      &'static str,
    /// Query URL template — `{}` is replaced with the percent-encoded query.
    pub query_url: &'static str,
}

impl SearchProvider {
    pub fn build_url(&self, query: &str) -> String {
        let encoded = percent_encode(query);
        self.query_url.replace("{}", &encoded)
    }
}

pub const DEFAULT_PROVIDER: SearchProvider = SearchProvider {
    name:      "DuckDuckGo",
    query_url: "https://duckduckgo.com/?q={}",
};

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => { out.push('%'); out.push(hex_hi(b)); out.push(hex_lo(b)); }
        }
    }
    out
}

#[inline] fn hex_hi(b: u8) -> char { char::from_digit((b >> 4) as u32, 16).unwrap().to_ascii_uppercase() }
#[inline] fn hex_lo(b: u8) -> char { char::from_digit((b & 0xf) as u32, 16).unwrap().to_ascii_uppercase() }

// ── Input classification ──────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum InputKind {
    /// Ready-to-navigate URL (already normalised — has a scheme).
    Url(String),
    /// Plaintext search query.
    Search(String),
    /// Internal browser route.
    Internal(InternalRoute),
}

#[derive(Debug, PartialEq)]
pub enum InternalRoute {
    Blank,
    History,
    Bookmarks,
}

/// Classify and normalise raw omnibox input.
pub fn classify_input(raw: &str) -> InputKind {
    let s = raw.trim();
    if s.is_empty() { return InputKind::Search(String::new()); }

    // ── Internal routes ───────────────────────────────────────────────────────
    match s.to_ascii_lowercase().as_str() {
        "about:blank"     => return InputKind::Internal(InternalRoute::Blank),
        "about:history"   => return InputKind::Internal(InternalRoute::History),
        "about:bookmarks" => return InputKind::Internal(InternalRoute::Bookmarks),
        _ => {}
    }

    // ── Explicit scheme → URL ─────────────────────────────────────────────────
    if s.contains("://") {
        return InputKind::Url(s.to_string());
    }
    // chrome: / about: / data: without "://" still treated as URL
    if s.starts_with("chrome:") || s.starts_with("about:") || s.starts_with("data:") {
        return InputKind::Url(s.to_string());
    }

    // ── localhost ─────────────────────────────────────────────────────────────
    let lower = s.to_ascii_lowercase();
    if lower == "localhost" || lower.starts_with("localhost:") || lower.starts_with("localhost/") {
        return InputKind::Url(format!("http://{s}"));
    }

    // ── IPv4 ──────────────────────────────────────────────────────────────────
    if is_ipv4(s) {
        return InputKind::Url(format!("http://{s}"));
    }

    // ── Domain heuristic: no spaces + contains dot ────────────────────────────
    if !s.contains(' ') && s.contains('.') && !s.starts_with('.') && !s.ends_with('.') {
        return InputKind::Url(format!("https://{s}"));
    }

    InputKind::Search(s.to_string())
}

fn is_ipv4(s: &str) -> bool {
    // Strip optional port and path
    let host = s.split('/').next().unwrap_or(s);
    let host = host.split(':').next().unwrap_or(host);
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 { return false; }
    parts.iter().all(|p| p.parse::<u8>().is_ok())
}

// ── History / bookmark matching ───────────────────────────────────────────────

/// A minimal borrowed view of a visit record for matching.
pub struct MatchEntry<'a> {
    pub url:   &'a str,
    pub title: &'a str,
}

/// Try to find a direct match in bookmarks (checked first) then history.
/// Returns the URL of the first strong match, or `None`.
///
/// "Strong match" = query is a case-insensitive substring of title OR url.
pub fn match_history_bookmarks<'a>(
    query:     &str,
    bookmarks: impl Iterator<Item = MatchEntry<'a>>,
    history:   impl Iterator<Item = MatchEntry<'a>>,
) -> Option<String> {
    let q = query.to_ascii_lowercase();
    if q.is_empty() { return None; }

    let test = |e: MatchEntry<'_>| -> Option<String> {
        if e.url.to_ascii_lowercase().contains(&q)
            || e.title.to_ascii_lowercase().contains(&q)
        {
            Some(e.url.to_string())
        } else {
            None
        }
    };

    for entry in bookmarks { if let Some(u) = test(entry) { return Some(u); } }
    for entry in history   { if let Some(u) = test(entry) { return Some(u); } }
    None
}

// ── Top-level resolver ────────────────────────────────────────────────────────

/// Full omnibox resolution:
/// 1. classify input
/// 2. for Search queries: try bookmark/history match first
/// 3. fall back to search provider
///
/// Returns `OmniboxResult` telling the caller what to do.
pub enum OmniboxResult {
    Navigate(String),
    OpenOverlay(InternalRoute),
    Nothing,
}

pub fn resolve<'a>(
    raw:       &str,
    bookmarks: impl Iterator<Item = MatchEntry<'a>>,
    history:   impl Iterator<Item = MatchEntry<'a>>,
    provider:  &SearchProvider,
) -> OmniboxResult {
    match classify_input(raw) {
        InputKind::Url(url)          => OmniboxResult::Navigate(url),
        InputKind::Internal(route)   => OmniboxResult::OpenOverlay(route),
        InputKind::Search(q) if q.is_empty() => OmniboxResult::Nothing,
        InputKind::Search(q) => {
            let url = match_history_bookmarks(&q, bookmarks, history)
                .unwrap_or_else(|| provider.build_url(&q));
            OmniboxResult::Navigate(url)
        }
    }
}
