//! Minimal persistence layer — bookmarks, history, prefs.
//!
//! Files live in `$XDG_DATA_HOME/rashamon-arc/` (fallback: `~/.local/share/rashamon-arc/`).
//! Format: hand-written JSON, no external crate.
//! All I/O is best-effort — failures are logged to stderr but never crash the browser.

use std::fs;
use std::path::PathBuf;

// ── Data directory ────────────────────────────────────────────────────────────

fn data_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("rashamon-arc")
}

fn ensure_dir(dir: &PathBuf) {
    if let Err(e) = fs::create_dir_all(dir) {
        eprintln!("[persist] cannot create data dir: {e}");
    }
}

/// Atomic write: write to .tmp then rename to final path.
fn atomic_write(path: &PathBuf, content: &str) {
    let tmp = path.with_extension("json.tmp");
    match fs::write(&tmp, content) {
        Ok(()) => {
            if let Err(e) = fs::rename(&tmp, path) {
                eprintln!("[persist] rename failed for {}: {e}", path.display());
                let _ = fs::remove_file(&tmp);
            }
        }
        Err(e) => eprintln!("[persist] write failed for {}: {e}", path.display()),
    }
}

// ── Public data types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StoredBookmark {
    pub title: String,
    pub url:   String,
}

#[derive(Debug, Clone)]
pub struct StoredHistory {
    pub url:   String,
    pub title: String,
}

// ── Load / Save: bookmarks ────────────────────────────────────────────────────

pub fn load_bookmarks() -> Vec<StoredBookmark> {
    let path = data_dir().join("bookmarks.json");
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    parse_array(&text)
        .into_iter()
        .filter_map(|obj| {
            let title = find_field(&obj, "title").unwrap_or_default();
            let url   = find_field(&obj, "url")?;
            if url.is_empty() { return None; }
            Some(StoredBookmark { title, url })
        })
        .collect()
}

pub fn save_bookmarks(bookmarks: &[StoredBookmark]) {
    let dir = data_dir();
    ensure_dir(&dir);
    let path = dir.join("bookmarks.json");
    let mut out = String::from("[\n");
    for (i, bm) in bookmarks.iter().enumerate() {
        out.push_str("  {");
        out.push_str(&kv("title", &bm.title));
        out.push_str(", ");
        out.push_str(&kv("url", &bm.url));
        out.push('}');
        if i + 1 < bookmarks.len() { out.push(','); }
        out.push('\n');
    }
    out.push_str("]\n");
    atomic_write(&path, &out);
}

// ── Load / Save: history ──────────────────────────────────────────────────────

pub fn load_history() -> Vec<StoredHistory> {
    let path = data_dir().join("history.json");
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    parse_array(&text)
        .into_iter()
        .filter_map(|obj| {
            let url   = find_field(&obj, "url")?;
            let title = find_field(&obj, "title").unwrap_or_default();
            if url.is_empty() { return None; }
            Some(StoredHistory { url, title })
        })
        .collect()
}

pub fn save_history(history: &[StoredHistory]) {
    let dir = data_dir();
    ensure_dir(&dir);
    let path = dir.join("history.json");
    let mut out = String::from("[\n");
    for (i, e) in history.iter().enumerate() {
        out.push_str("  {");
        out.push_str(&kv("url", &e.url));
        out.push_str(", ");
        out.push_str(&kv("title", &e.title));
        out.push('}');
        if i + 1 < history.len() { out.push(','); }
        out.push('\n');
    }
    out.push_str("]\n");
    atomic_write(&path, &out);
}

// ── Load / Save: prefs ────────────────────────────────────────────────────────

pub fn load_theme() -> Option<String> {
    let path = data_dir().join("prefs.json");
    let text = fs::read_to_string(&path).ok()?;
    let obj  = parse_object(&text)?;
    find_field(&obj, "theme")
}

pub fn save_theme(theme: &str) {
    let dir = data_dir();
    ensure_dir(&dir);
    let path = dir.join("prefs.json");
    let out  = format!("{{\n  {}\n}}\n", kv("theme", theme));
    atomic_write(&path, &out);
}

// ── Minimal JSON encoder ──────────────────────────────────────────────────────

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => { out.push_str(&format!("\\u{:04x}", c as u32)); }
            c    => out.push(c),
        }
    }
    out.push('"');
    out
}

fn kv(key: &str, val: &str) -> String {
    format!("{}: {}", json_str(key), json_str(val))
}

// ── Minimal JSON decoder ──────────────────────────────────────────────────────

type JsonObj = Vec<(String, String)>;

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self { Self { src: src.as_bytes(), pos: 0 } }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len()
            && matches!(self.src[self.pos], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> { self.src.get(self.pos).copied() }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) { self.pos += 1; true } else { false }
    }

    fn parse_string(&mut self) -> Option<String> {
        if self.peek() != Some(b'"') { return None; }
        self.pos += 1;
        let mut out = String::new();
        loop {
            let b = *self.src.get(self.pos)?;
            if b == b'"' { self.pos += 1; return Some(out); }
            if b == b'\\' {
                self.pos += 1;
                match *self.src.get(self.pos)? {
                    b'"'  => { out.push('"');  self.pos += 1; }
                    b'\\' => { out.push('\\'); self.pos += 1; }
                    b'/'  => { out.push('/');  self.pos += 1; }
                    b'n'  => { out.push('\n'); self.pos += 1; }
                    b'r'  => { out.push('\r'); self.pos += 1; }
                    b't'  => { out.push('\t'); self.pos += 1; }
                    b'u'  => {
                        self.pos += 1;
                        if self.pos + 4 > self.src.len() { return None; }
                        let hex = std::str::from_utf8(&self.src[self.pos..self.pos + 4]).ok()?;
                        let code = u32::from_str_radix(hex, 16).ok()?;
                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        self.pos += 4;
                    }
                    other => { out.push(other as char); self.pos += 1; }
                }
            } else {
                // Grab a full UTF-8 sequence.
                let start = self.pos;
                self.pos += 1;
                while self.pos < self.src.len() && (self.src[self.pos] & 0xC0) == 0x80 {
                    self.pos += 1;
                }
                if let Ok(s) = std::str::from_utf8(&self.src[start..self.pos]) {
                    out.push_str(s);
                }
            }
        }
    }

    fn skip_value(&mut self) {
        self.skip_ws();
        match self.peek() {
            Some(b'"') => { self.parse_string(); }
            Some(b'{') => { self.parse_obj(); }
            Some(b'[') => { self.parse_arr_raw(); }
            _ => {
                while self.pos < self.src.len()
                    && !matches!(self.src[self.pos], b',' | b'}' | b']')
                {
                    self.pos += 1;
                }
            }
        }
    }

    fn parse_obj(&mut self) -> Option<JsonObj> {
        self.skip_ws();
        if !self.eat(b'{') { return None; }
        let mut pairs = Vec::new();
        loop {
            self.skip_ws();
            if self.eat(b'}') { break; }
            if !pairs.is_empty() {
                if !self.eat(b',') { break; }
                self.skip_ws();
                if self.eat(b'}') { break; }
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if !self.eat(b':') { break; }
            self.skip_ws();
            // Only record string values; skip non-string values gracefully.
            if self.peek() == Some(b'"') {
                if let Some(val) = self.parse_string() {
                    pairs.push((key, val));
                    continue;
                }
            }
            self.skip_value();
        }
        Some(pairs)
    }

    fn parse_arr_raw(&mut self) {
        if !self.eat(b'[') { return; }
        let mut depth = 1usize;
        while self.pos < self.src.len() && depth > 0 {
            match self.src[self.pos] {
                b'[' => { depth += 1; self.pos += 1; }
                b']' => { depth -= 1; self.pos += 1; }
                b'"' => { self.parse_string(); }
                _    => { self.pos += 1; }
            }
        }
    }

    fn parse_array(&mut self) -> Vec<JsonObj> {
        self.skip_ws();
        let mut items = Vec::new();
        if !self.eat(b'[') { return items; }
        loop {
            self.skip_ws();
            if self.eat(b']') { break; }
            if !items.is_empty() {
                if !self.eat(b',') { break; }
                self.skip_ws();
                if self.eat(b']') { break; }
            }
            if self.peek() == Some(b'{') {
                if let Some(obj) = self.parse_obj() {
                    items.push(obj);
                    continue;
                }
            }
            // Skip non-object element
            self.skip_value();
        }
        items
    }
}

fn parse_array(src: &str) -> Vec<JsonObj> {
    Parser::new(src).parse_array()
}

fn parse_object(src: &str) -> Option<JsonObj> {
    Parser::new(src).parse_obj()
}

fn find_field(obj: &JsonObj, key: &str) -> Option<String> {
    obj.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}
