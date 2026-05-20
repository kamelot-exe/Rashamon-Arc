//! Minimal site-scoped permission model for Rashamon Arc.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionKind {
    Notifications,
    Clipboard,
    Geolocation,
    Camera,
    Microphone,
}

impl PermissionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Notifications => "notifications",
            Self::Clipboard => "clipboard",
            Self::Geolocation => "geolocation",
            Self::Camera => "camera",
            Self::Microphone => "microphone",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "notifications" => Some(Self::Notifications),
            "clipboard" => Some(Self::Clipboard),
            "geolocation" => Some(Self::Geolocation),
            "camera" => Some(Self::Camera),
            "microphone" => Some(Self::Microphone),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny,
    Ask,
}

impl PermissionDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Ask => "ask",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "ask" => Some(Self::Ask),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionSource {
    Persisted,
    Session,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionEntry {
    pub origin: String,
    pub kind: PermissionKind,
    pub decision: PermissionDecision,
}

#[derive(Debug, Clone)]
pub struct PermissionStore {
    persisted: HashMap<(String, PermissionKind), PermissionDecision>,
    session: HashMap<(String, PermissionKind), PermissionDecision>,
    path: PathBuf,
}

impl PermissionStore {
    pub fn load_default() -> Self {
        Self::load_from_path(data_dir().join("permissions.json"))
    }

    pub fn load_from_path(path: PathBuf) -> Self {
        let mut store = Self {
            persisted: HashMap::new(),
            session: HashMap::new(),
            path,
        };
        if let Ok(text) = fs::read_to_string(&store.path) {
            for entry in parse_permissions_json(&text) {
                store
                    .persisted
                    .insert((entry.origin, entry.kind), entry.decision);
            }
        }
        store
    }

    pub fn get(
        &self,
        origin: &str,
        kind: PermissionKind,
        private: bool,
    ) -> (PermissionDecision, DecisionSource) {
        let key = (origin.to_string(), kind);
        if private {
            if let Some(decision) = self.session.get(&key) {
                return (*decision, DecisionSource::Session);
            }
            return (PermissionDecision::Ask, DecisionSource::Default);
        }
        if let Some(decision) = self.persisted.get(&key) {
            return (*decision, DecisionSource::Persisted);
        }
        (PermissionDecision::Ask, DecisionSource::Default)
    }

    pub fn set(
        &mut self,
        origin: &str,
        kind: PermissionKind,
        decision: PermissionDecision,
        private: bool,
    ) {
        let key = (origin.to_string(), kind);
        if private {
            self.session.insert(key, decision);
        } else {
            self.persisted.insert(key, decision);
            self.save();
        }
    }

    pub fn entries(&self) -> Vec<PermissionEntry> {
        let mut out = self
            .persisted
            .iter()
            .map(|((origin, kind), decision)| PermissionEntry {
                origin: origin.clone(),
                kind: *kind,
                decision: *decision,
            })
            .collect::<Vec<_>>();
        out.sort_by(|a, b| {
            a.origin
                .cmp(&b.origin)
                .then_with(|| a.kind.as_str().cmp(b.kind.as_str()))
        });
        out
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                trace_permissions(&format!("cannot create data dir: {e}"));
                return;
            }
        }
        let mut out = String::from("[\n");
        let entries = self.entries();
        for (i, entry) in entries.iter().enumerate() {
            out.push_str("  {");
            out.push_str(&format!(
                "\"origin\": {}, \"permission\": {}, \"decision\": {}",
                json_str(&entry.origin),
                json_str(entry.kind.as_str()),
                json_str(entry.decision.as_str()),
            ));
            out.push('}');
            if i + 1 < entries.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("]\n");
        let tmp = self.path.with_extension("json.tmp");
        match fs::write(&tmp, out) {
            Ok(()) => {
                if fs::rename(&tmp, &self.path).is_err() {
                    let _ = fs::remove_file(&tmp);
                }
            }
            Err(e) => trace_permissions(&format!("write failed: {e}")),
        }
    }
}

pub fn origin_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let (scheme, rest) = trimmed.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let after_user = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
    let host_port = after_user
        .split(['/', '?', '#'])
        .next()?
        .to_ascii_lowercase();
    let host_port = host_port.trim_matches('.');
    if host_port.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host_port}"))
}

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

fn parse_permissions_json(text: &str) -> Vec<PermissionEntry> {
    parse_object_slices(text)
        .into_iter()
        .filter_map(|obj| {
            let origin = find_json_string(obj, "origin")?;
            let kind = PermissionKind::from_str(&find_json_string(obj, "permission")?)?;
            let decision = PermissionDecision::from_str(&find_json_string(obj, "decision")?)?;
            Some(PermissionEntry {
                origin,
                kind,
                decision,
            })
        })
        .collect()
}

fn parse_object_slices(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut in_str = false;
    let mut esc = false;
    for (idx, ch) in text.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => {
                if depth == 0 {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(s) = start.take() {
                        out.push(&text[s..=idx]);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn find_json_string(obj: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let pos = obj.find(&needle)?;
    let after_key = &obj[pos + needle.len()..];
    let colon = after_key.find(':')?;
    parse_json_string(after_key[colon + 1..].trim_start())
}

fn parse_json_string(input: &str) -> Option<String> {
    let mut chars = input.chars();
    if chars.next()? != '"' {
        return None;
    }
    let mut out = String::new();
    let mut esc = false;
    for ch in chars {
        if esc {
            out.push(match ch {
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
            return Some(out);
        } else {
            out.push(ch);
        }
    }
    None
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

fn trace_permissions(message: &str) {
    if std::env::var_os("RASHAMON_DEBUG").is_some() {
        eprintln!("[permissions] {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_preserves_scheme_and_host() {
        assert_eq!(
            origin_from_url("https://Example.com/path"),
            Some("https://example.com".into())
        );
        assert_eq!(
            origin_from_url("http://example.com:8080/a"),
            Some("http://example.com:8080".into())
        );
        assert_ne!(
            origin_from_url("http://example.com"),
            origin_from_url("https://example.com")
        );
    }

    #[test]
    fn malformed_file_loads_empty() {
        let path =
            std::env::temp_dir().join(format!("rashamon-perms-bad-{}.json", std::process::id()));
        fs::write(&path, "{not-json").unwrap();
        let store = PermissionStore::load_from_path(path.clone());
        let _ = fs::remove_file(path);
        assert!(store.entries().is_empty());
    }
}
