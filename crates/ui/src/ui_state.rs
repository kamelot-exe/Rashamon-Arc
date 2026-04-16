//! Core browser state model for Rashamon Arc.
//!
//! Designed as an explicit, minimal state machine.
//! All browser behaviour flows through BrowserState methods — no implicit logic.

use crate::theme::{get_theme, ColorPalette, Theme};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

// ── TabId ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(usize);

impl TabId {
    fn next() -> Self { Self(NEXT_ID.fetch_add(1, Ordering::Relaxed)) }
}

// ── PageState ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum PageState {
    NewTab,
    Loading,
    Loaded,
    Error(String),
}

impl PageState {
    pub fn is_loading(&self) -> bool { matches!(self, Self::Loading) }
    pub fn is_new_tab(&self) -> bool { matches!(self, Self::NewTab) }
    pub fn is_error(&self) -> bool { matches!(self, Self::Error(_)) }

    pub fn error_msg(&self) -> Option<&str> {
        if let Self::Error(m) = self { Some(m) } else { None }
    }
}

// ── NavigationEntry ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NavigationEntry {
    pub url: String,
    pub display_url: String,
    pub title: String,
}

impl NavigationEntry {
    fn from_url(url: &str) -> Self {
        Self {
            url: url.to_string(),
            display_url: url.to_string(),
            title: derive_title(url),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn derive_title(url: &str) -> String {
    if url.is_empty() { return "New Tab".to_string(); }
    url.trim_start_matches("https://")
       .trim_start_matches("http://")
       .trim_start_matches("www.")
       .split('/')
       .next()
       .unwrap_or(url)
       .to_string()
}

// ── TabState ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TabState {
    pub id: TabId,
    pub title: String,
    pub url: String,
    pub display_url: String,
    pub page_state: PageState,
    pub is_pinned: bool,
    pub is_bookmarked: bool,
    pub history: Vec<NavigationEntry>,
    pub history_index: usize,
    pub last_committed_url: String,
    pub load_start_frame: u64,
}

impl TabState {
    pub fn new_tab() -> Self {
        Self {
            id: TabId::next(),
            title: "New Tab".to_string(),
            url: String::new(),
            display_url: String::new(),
            page_state: PageState::NewTab,
            is_pinned: false,
            is_bookmarked: false,
            history: Vec::new(),
            history_index: 0,
            last_committed_url: String::new(),
            load_start_frame: 0,
        }
    }

    pub fn can_go_back(&self) -> bool { self.history_index > 0 }

    pub fn can_go_forward(&self) -> bool {
        self.history_index + 1 < self.history.len()
    }

    /// Commit a successful navigation into the history stack.
    fn commit(&mut self, url: &str, title: &str) {
        // Truncate forward entries when navigating to a new URL.
        if self.history_index + 1 < self.history.len() {
            self.history.truncate(self.history_index + 1);
        }
        let entry = NavigationEntry {
            url: url.to_string(),
            display_url: url.to_string(),
            title: title.to_string(),
        };
        self.history.push(entry);
        self.history_index = self.history.len() - 1;
        self.last_committed_url = url.to_string();
    }

    /// Best title to display in the tab strip.
    pub fn tab_title(&self) -> &str {
        match &self.page_state {
            PageState::NewTab  => "New Tab",
            PageState::Loading => {
                if self.title.is_empty() { "Loading…" } else { &self.title }
            }
            PageState::Loaded  => {
                if self.title.is_empty() { &self.url } else { &self.title }
            }
            PageState::Error(_) => {
                if self.title.is_empty() { "Error" } else { &self.title }
            }
        }
    }
}

// ── QuickLink ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct QuickLink {
    pub title: String,
    pub url: String,
}

// ── BrowserState ──────────────────────────────────────────────────────────────

pub struct BrowserState {
    pub tabs: Vec<TabState>,
    pub active_tab_id: TabId,

    pub mouse_x: u32,
    pub mouse_y: u32,
    pub frame_count: u64,

    pub theme: Theme,
    pub palette: ColorPalette,

    /// Single source of truth for the address bar text field.
    pub address_bar_focused: bool,
    pub address_bar_input: String,

    pub bookmarks: Vec<QuickLink>,

    /// Visual press state for nav buttons (1=back, 2=fwd, 3=reload).
    pub nav_btn_pressed: u8,
    pub nav_btn_pressed_frame: u64,
}

impl BrowserState {
    pub fn new() -> Self {
        let palette = ColorPalette::KamelotDark;
        let first_tab = TabState::new_tab();
        let first_id = first_tab.id;
        Self {
            tabs: vec![first_tab],
            active_tab_id: first_id,
            mouse_x: 0,
            mouse_y: 0,
            frame_count: 0,
            palette,
            theme: get_theme(palette),
            address_bar_focused: false,
            address_bar_input: String::new(),
            bookmarks: vec![
                QuickLink { title: "GitHub".to_string(),      url: "https://github.com".to_string() },
                QuickLink { title: "Hacker News".to_string(), url: "https://news.ycombinator.com".to_string() },
                QuickLink { title: "Rust Lang".to_string(),   url: "https://www.rust-lang.org".to_string() },
                QuickLink { title: "MDN".to_string(),         url: "https://developer.mozilla.org".to_string() },
                QuickLink { title: "Servo".to_string(),       url: "https://servo.org".to_string() },
                QuickLink { title: "Crates.io".to_string(),   url: "https://crates.io".to_string() },
            ],
            nav_btn_pressed: 0,
            nav_btn_pressed_frame: 0,
        }
    }

    // ── Tab accessors ─────────────────────────────────────────────────────────

    pub fn active_tab(&self) -> Option<&TabState> {
        self.tabs.iter().find(|t| t.id == self.active_tab_id)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut TabState> {
        self.tabs.iter_mut().find(|t| t.id == self.active_tab_id)
    }

    /// Index of the active tab in the tab Vec (for positional rendering).
    pub fn active_tab_pos(&self) -> usize {
        self.tabs.iter().position(|t| t.id == self.active_tab_id).unwrap_or(0)
    }

    // ── Tab lifecycle ─────────────────────────────────────────────────────────

    pub fn open_new_tab(&mut self) {
        let tab = TabState::new_tab();
        let id = tab.id;
        self.tabs.push(tab);
        self.activate_tab(id);
    }

    pub fn close_tab(&mut self, id: TabId) {
        if self.tabs.len() == 1 {
            // Replace the last tab with a fresh New Tab instead of emptying the vec.
            let fresh = TabState::new_tab();
            let fresh_id = fresh.id;
            self.tabs[0] = fresh;
            self.active_tab_id = fresh_id;
            self.sync_address_bar();
            return;
        }
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        let closing_active = id == self.active_tab_id;
        self.tabs.remove(idx);
        if closing_active {
            let new_idx = idx.min(self.tabs.len() - 1);
            let new_id = self.tabs[new_idx].id;
            self.activate_tab(new_id);
        }
    }

    pub fn activate_tab(&mut self, id: TabId) {
        if self.tabs.iter().any(|t| t.id == id) {
            self.active_tab_id = id;
            self.address_bar_focused = false;
            self.sync_address_bar();
        }
    }

    // ── Navigation ────────────────────────────────────────────────────────────

    /// Start navigating the active tab to `url`. Returns url for caller to hand
    /// to the render engine.
    pub fn begin_navigate(&mut self, url: &str) -> Option<String> {
        if url.is_empty() { return None; }
        let frame = self.frame_count;
        let url = url.to_string();
        if let Some(tab) = self.active_tab_mut() {
            tab.url = url.clone();
            tab.display_url = url.clone();
            tab.title = derive_title(&url);
            tab.page_state = PageState::Loading;
            tab.load_start_frame = frame;
        }
        self.address_bar_input = url.clone();
        self.address_bar_focused = false;
        Some(url)
    }

    /// Called by the main loop once loading has resolved successfully.
    pub fn resolve_loading(&mut self, engine_title: String) {
        let url = match self.active_tab() {
            Some(t) if t.page_state.is_loading() => t.url.clone(),
            _ => return,
        };
        let title = if engine_title.is_empty() { derive_title(&url) } else { engine_title };
        if let Some(tab) = self.active_tab_mut() {
            tab.title = title.clone();
            tab.page_state = PageState::Loaded;
            tab.commit(&url, &title);
        }
    }

    /// Called when loading times out or the engine signals an error.
    pub fn fail_loading(&mut self, message: &str) {
        if let Some(tab) = self.active_tab_mut() {
            if tab.page_state.is_loading() {
                tab.page_state = PageState::Error(message.to_string());
            }
        }
    }

    /// Navigate back within the active tab's history.
    /// Returns the URL to load, or None if at the beginning.
    pub fn go_back(&mut self) -> Option<String> {
        let frame = self.frame_count;
        let tab = self.active_tab_mut()?;
        if !tab.can_go_back() { return None; }
        tab.history_index -= 1;
        let entry = tab.history[tab.history_index].clone();
        tab.url = entry.url.clone();
        tab.display_url = entry.display_url.clone();
        tab.title = entry.title.clone();
        tab.page_state = PageState::Loading;
        tab.load_start_frame = frame;
        self.address_bar_input = entry.url.clone();
        Some(entry.url)
    }

    /// Navigate forward within the active tab's history.
    pub fn go_forward(&mut self) -> Option<String> {
        let frame = self.frame_count;
        let tab = self.active_tab_mut()?;
        if !tab.can_go_forward() { return None; }
        tab.history_index += 1;
        let entry = tab.history[tab.history_index].clone();
        tab.url = entry.url.clone();
        tab.display_url = entry.display_url.clone();
        tab.title = entry.title.clone();
        tab.page_state = PageState::Loading;
        tab.load_start_frame = frame;
        self.address_bar_input = entry.url.clone();
        Some(entry.url)
    }

    /// Reload the active tab. Returns the URL to reload, or None.
    pub fn reload(&mut self) -> Option<String> {
        let frame = self.frame_count;
        let url = self.active_tab()
            .filter(|t| !t.url.is_empty())
            .map(|t| t.url.clone())?;
        if let Some(tab) = self.active_tab_mut() {
            tab.page_state = PageState::Loading;
            tab.load_start_frame = frame;
        }
        Some(url)
    }

    // ── Address bar ───────────────────────────────────────────────────────────

    pub fn sync_address_bar(&mut self) {
        let url = self.active_tab().map(|t| t.url.clone()).unwrap_or_default();
        self.address_bar_input = url;
    }

    pub fn focus_address_bar(&mut self) {
        self.address_bar_focused = true;
    }

    pub fn cancel_address_bar_edit(&mut self) {
        self.address_bar_focused = false;
        self.sync_address_bar();
    }

    // ── Theme ─────────────────────────────────────────────────────────────────

    pub fn cycle_theme(&mut self) {
        let next = self.palette.cycle();
        self.palette = next;
        self.theme = get_theme(next);
    }

    // ── Mouse / input helpers ─────────────────────────────────────────────────

    pub fn set_mouse_pos(&mut self, x: u32, y: u32) {
        self.mouse_x = x;
        self.mouse_y = y;
    }

    pub fn press_nav_btn(&mut self, id: u8) {
        self.nav_btn_pressed = id;
        self.nav_btn_pressed_frame = self.frame_count;
    }

    pub fn tick_nav_btn(&mut self) {
        if self.nav_btn_pressed != 0 && self.frame_count > self.nav_btn_pressed_frame + 12 {
            self.nav_btn_pressed = 0;
        }
    }

    // ── Bookmarks ─────────────────────────────────────────────────────────────

    pub fn toggle_bookmark(&mut self) {
        let (url, title, was) = match self.active_tab() {
            Some(t) if !t.url.is_empty() => (t.url.clone(), t.title.clone(), t.is_bookmarked),
            _ => return,
        };
        if was {
            self.bookmarks.retain(|b| b.url != url);
        } else {
            self.bookmarks.push(QuickLink { title, url });
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.is_bookmarked = !was;
        }
    }

    pub fn refresh_bookmark_flag(&mut self) {
        let url = self.active_tab().map(|t| t.url.clone()).unwrap_or_default();
        let is_bm = !url.is_empty() && self.bookmarks.iter().any(|b| b.url == url);
        if let Some(tab) = self.active_tab_mut() {
            tab.is_bookmarked = is_bm;
        }
    }
}
