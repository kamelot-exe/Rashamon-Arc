//! Minimal HTML parser → page content model.
//!
//! Not a spec-compliant parser. Goal: extract readable structure from real
//! HTML pages — titles, headings, paragraphs, lists — well enough to display
//! a useful first-pass rendering.
//!
//! Pipeline:
//!   parse_html(html)
//!     → extract_title (title tag + og:title fallback)
//!     → remove_tag_blocks (strip script/style/svg noise)
//!     → skip_head (start from <body>)
//!     → walk_content (emit PageNodes from structural tags)

// ── Public types ──────────────────────────────────────────────────────────────

/// A structured content node from an HTML page.
#[derive(Debug, Clone)]
pub enum PageNode {
    Heading { level: u8, text: String },
    Paragraph(String),
    ListItem(String),
    Pre(String),
    HRule,
}

/// Parsed representation of a fetched HTML page.
#[derive(Debug, Clone, Default)]
pub struct ParsedPage {
    pub title: Option<String>,
    pub nodes: Vec<PageNode>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn parse_html(html: &str) -> ParsedPage {
    let title = extract_title(html).or_else(|| extract_og_title(html));
    let nodes = extract_nodes(html);
    ParsedPage { title, nodes }
}

// ── Title extraction ──────────────────────────────────────────────────────────

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let ts = lower.find("<title")?;
    let open_end = lower[ts..].find('>')? + ts + 1;
    let close = lower[open_end..].find("</title")?;
    let raw = &html[open_end..open_end + close];
    let text = decode_entities(raw.trim());
    if text.is_empty() { None } else { Some(text) }
}

fn extract_og_title(html: &str) -> Option<String> {
    // <meta property="og:title" content="...">
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(rel) = lower[pos..].find("<meta") {
        let abs = pos + rel;
        let tag_end = lower[abs..].find('>').map(|e| abs + e).unwrap_or(html.len());
        let tag_lower = &lower[abs..=tag_end.min(html.len()-1)];
        let tag_raw   = &html[abs..=tag_end.min(html.len()-1)];
        if tag_lower.contains("og:title") || tag_lower.contains("twitter:title") {
            if let Some(v) = attr_value(tag_raw, "content") {
                if !v.is_empty() { return Some(v); }
            }
        }
        pos = tag_end + 1;
        if pos >= html.len() { break; }
    }
    None
}

/// Extract a named attribute value from a raw (non-lowercased) tag fragment.
fn attr_value(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let key   = format!("{attr}=\"");
    let start = lower.find(&key)? + key.len();
    let end   = tag[start..].find('"')? + start;
    Some(decode_entities(&tag[start..end]))
}

// ── Block stripping ───────────────────────────────────────────────────────────

/// Remove all content between paired `<tag>…</tag>` blocks (case-insensitive).
fn remove_tag_blocks(html: &str, tag: &str) -> String {
    let open  = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut result = html.to_string();

    loop {
        let lower = result.to_ascii_lowercase();
        // Find an opening tag that is a true tag boundary (not e.g. <scripting>)
        let start = {
            let mut found = None;
            let mut scan  = 0;
            while let Some(rel) = lower[scan..].find(&open) {
                let abs  = scan + rel;
                let next = lower.as_bytes().get(abs + open.len()).copied();
                if matches!(next, Some(b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/')) {
                    found = Some(abs);
                    break;
                }
                scan = abs + open.len();
                if scan >= lower.len() { break; }
            }
            found
        };
        let start = match start { None => break, Some(s) => s };

        // Find matching close tag
        let after = &lower[start..];
        let end   = after.find(&close).map(|p| start + p + close.len());
        match end {
            Some(e) => { result.replace_range(start..e, " "); }
            None    => { result.truncate(start); break; }
        }
    }
    result
}

// ── Body extraction ───────────────────────────────────────────────────────────

fn extract_nodes(html: &str) -> Vec<PageNode> {
    // Remove noise tags entirely
    let mut clean = html.to_string();
    for tag in &["script", "style", "noscript", "svg", "template", "iframe"] {
        clean = remove_tag_blocks(&clean, tag);
    }
    // Start from <body> when present
    let body = {
        let lower = clean.to_ascii_lowercase();
        if let Some(bp) = lower.find("<body") {
            if let Some(be) = lower[bp..].find('>') {
                clean[bp + be + 1..].to_string()
            } else { clean }
        } else { clean }
    };

    let mut nodes = walk_content(&body);

    // Drop trivially-short text nodes (navigation labels, vote arrows, etc.)
    nodes.retain(|n| match n {
        PageNode::Paragraph(t) | PageNode::ListItem(t) => {
            let t = t.trim();
            t.len() >= 20 || (t.len() >= 4 && t.chars().filter(|c| c.is_alphabetic()).count() >= 4)
        }
        PageNode::Heading { text, .. } => !text.trim().is_empty(),
        PageNode::Pre(t) => !t.trim().is_empty(),
        PageNode::HRule  => true,
    });

    // Remove consecutive HRules
    nodes.dedup_by(|a, b| matches!((a, b), (PageNode::HRule, PageNode::HRule)));

    // Cap total nodes to keep rendering fast
    nodes.truncate(120);
    nodes
}

// ── Content walker ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Block { None, H1, H2, H3, P, Li, Pre }

fn walk_content(html: &str) -> Vec<PageNode> {
    let mut nodes    = Vec::new();
    let mut block    = Block::None;
    let mut cur      = String::new();
    let mut in_pre   = false;
    let bytes = html.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            // Text node
            let start = i;
            while i < bytes.len() && bytes[i] != b'<' { i += 1; }
            let raw      = &html[start..i];
            let decoded  = decode_entities(raw);
            let collapsed = collapse_ws(&decoded);
            if !collapsed.is_empty() {
                if in_pre {
                    cur.push_str(raw); // preserve pre whitespace
                } else {
                    if !cur.is_empty() && !cur.ends_with(' ') { cur.push(' '); }
                    cur.push_str(&collapsed);
                }
            }
            continue;
        }

        // Tag
        let tag_end  = find_tag_end(html, i);
        let inner    = &html[i + 1..tag_end];
        let (name, is_close) = tag_info(inner);

        match (name.as_str(), is_close) {
            // ── Headings ──
            ("h1", false) => { flush(&mut cur, block, &mut nodes); block = Block::H1; }
            ("h1", true)  => { flush(&mut cur, block, &mut nodes); block = Block::None; }
            ("h2", false) => { flush(&mut cur, block, &mut nodes); block = Block::H2; }
            ("h2", true)  => { flush(&mut cur, block, &mut nodes); block = Block::None; }
            (h, false) if matches!(h, "h3"|"h4"|"h5"|"h6") => {
                flush(&mut cur, block, &mut nodes); block = Block::H3;
            }
            (h, true) if matches!(h, "h3"|"h4"|"h5"|"h6") => {
                flush(&mut cur, block, &mut nodes); block = Block::None;
            }

            // ── Paragraphs ──
            ("p", false) => { flush(&mut cur, block, &mut nodes); block = Block::P; }
            ("p", true)  => { flush(&mut cur, block, &mut nodes); block = Block::None; }

            // ── List items ──
            ("li", false) => { flush(&mut cur, block, &mut nodes); block = Block::Li; }
            ("li", true)  => { flush(&mut cur, block, &mut nodes); block = Block::None; }

            // ── Pre / code blocks ──
            ("pre", false) => { flush(&mut cur, block, &mut nodes); in_pre = true; block = Block::Pre; }
            ("pre", true)  => {
                if !cur.trim().is_empty() { nodes.push(PageNode::Pre(cur.trim().to_string())); }
                cur.clear(); in_pre = false; block = Block::None;
            }

            // ── Horizontal rule ──
            ("hr", _) => {
                flush(&mut cur, block, &mut nodes);
                block = Block::None;
                nodes.push(PageNode::HRule);
            }

            // ── Line break ──
            ("br", _) => {
                if !cur.trim().is_empty() { cur.push('\n'); }
            }

            // ── Block containers — flush on close ──
            (tag, true) if matches!(tag,
                "div"|"article"|"section"|"main"|"header"|"footer"|
                "aside"|"nav"|"ul"|"ol"|"table"|"tbody"|"thead"|"tr"|
                "td"|"th"|"blockquote"|"figure"|"figcaption"|"details"|"summary"
            ) => { flush(&mut cur, block, &mut nodes); block = Block::None; }

            // ── Opening block containers — flush existing text ──
            (tag, false) if matches!(tag, "article"|"section"|"main"|"blockquote") => {
                flush(&mut cur, block, &mut nodes); block = Block::P;
            }

            // ── Everything else (inline tags, unknown) — ignore structure ──
            _ => {}
        }

        i = tag_end + 1;
    }

    flush(&mut cur, block, &mut nodes);
    nodes
}

fn flush(text: &mut String, block: Block, nodes: &mut Vec<PageNode>) {
    let t = std::mem::take(text);
    // Collapse multiple internal newlines
    let t: String = t.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if t.is_empty() { return; }
    nodes.push(match block {
        Block::H1  => PageNode::Heading { level: 1, text: t },
        Block::H2  => PageNode::Heading { level: 2, text: t },
        Block::H3  => PageNode::Heading { level: 3, text: t },
        Block::Li  => PageNode::ListItem(t),
        Block::Pre => PageNode::Pre(t),
        Block::P | Block::None => PageNode::Paragraph(t),
    });
}

// ── Tag utilities ─────────────────────────────────────────────────────────────

/// Find the closing `>` of a tag starting at `start` (which points to `<`).
/// Handles quoted attribute values that may contain `>`.
fn find_tag_end(html: &str, start: usize) -> usize {
    let bytes = html.as_bytes();
    let mut i = start + 1;
    let mut quote = b'\0';
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' if quote == b'\0' => { quote = bytes[i]; }
            b'"'  if quote == b'"'  => { quote = b'\0'; }
            b'\'' if quote == b'\'' => { quote = b'\0'; }
            b'>'  if quote == b'\0' => { return i; }
            _ => {}
        }
        i += 1;
    }
    bytes.len().saturating_sub(1)
}

/// Return (lowercase_tag_name, is_close).
fn tag_info(inner: &str) -> (String, bool) {
    let s = inner.trim();
    let is_close = s.starts_with('/');
    let rest = if is_close { s[1..].trim_start() } else { s };
    let name = rest
        .split(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    (name, is_close)
}

// ── Text utilities ────────────────────────────────────────────────────────────

/// Collapse all whitespace runs to a single space.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true; // start as true so leading spaces are dropped
    for c in s.chars() {
        if c.is_whitespace() {
            if !space { out.push(' '); space = true; }
        } else {
            out.push(c); space = false;
        }
    }
    out
}

/// Decode common HTML entities.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') { return s.to_string(); }

    let mut out   = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '&' { out.push(c); continue; }

        // Collect entity name (up to 16 chars or ';')
        let mut ent   = String::new();
        let mut ended = false;
        for _ in 0..16 {
            match chars.peek() {
                Some(';')                         => { chars.next(); ended = true; break; }
                Some(&nc) if nc.is_alphanumeric() || nc == '#' => {
                    ent.push(nc); chars.next();
                }
                _ => break,
            }
        }
        if ended {
            out.push_str(&entity_char(&ent));
        } else {
            out.push('&');
            out.push_str(&ent);
        }
    }
    out
}

fn entity_char(e: &str) -> String {
    match e {
        "amp"   => "&",  "lt"    => "<",  "gt"    => ">",
        "quot"  => "\"", "apos"  => "'",  "nbsp"  => " ",
        "ndash" => "–",  "mdash" => "—",  "hellip"=> "…",
        "laquo" => "«",  "raquo" => "»",
        "copy"  => "©",  "reg"   => "®",  "trade" => "™",
        "bull"  => "•",  "middot"=> "·",
        "lsquo" => "'",  "rsquo" => "'",
        "ldquo" => "\u{201C}", "rdquo" => "\u{201D}",
        e if e.starts_with('#') => {
            let num = &e[1..];
            let code = if let Some(h) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                u32::from_str_radix(h, 16).ok()
            } else {
                num.parse::<u32>().ok()
            };
            return code.and_then(char::from_u32).map(|c| c.to_string()).unwrap_or_default();
        }
        _ => return format!("&{e};"),
    }.to_string()
}
