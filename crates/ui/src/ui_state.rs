use crate::theme::{get_theme, ColorPalette, Theme};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TAB_ID: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone)]
pub struct TabState {
    pub id: usize,
    pub url: String,
    pub title: String,
    pub is_loading: bool,
    pub is_pinned: bool,
    pub is_bookmarked: bool,
    pub security: SecurityLevel,
    pub history: Vec<String>,
    pub history_index: usize,
    pub error: Option<String>,
    pub load_start_frame: u64,
}

impl TabState {
    pub fn new(url: String) -> Self {
        let title = if url.is_empty() {
            "New Tab".to_string()
        } else {
            url.clone()
        };
        let (history, history_index) = if url.is_empty() {
            (Vec::new(), 0)
        } else {
            (vec![url.clone()], 0)
        };
        Self {
            id: NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed),
            url,
            title,
            is_loading: false,
            is_pinned: false,
            is_bookmarked: false,
            security: SecurityLevel::Unknown,
            history,
            history_index,
            error: None,
            load_start_frame: 0,
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.history_index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_index + 1 < self.history.len()
    }

    pub fn push_history(&mut self, url: &str) {
        if url.is_empty() { return; }
        if self.history_index + 1 < self.history.len() {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(url.to_string());
        self.history_index = self.history.len() - 1;
    }

    pub fn navigate_back(&mut self) -> Option<String> {
        if self.can_go_back() {
            self.history_index -= 1;
            Some(self.history[self.history_index].clone())
        } else {
            None
        }
    }

    pub fn navigate_forward(&mut self) -> Option<String> {
        if self.can_go_forward() {
            self.history_index += 1;
            Some(self.history[self.history_index].clone())
        } else {
            None
        }
    }

    pub fn hostname(&self) -> String {
        let s = self.url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_start_matches("www.");
        s.split('/').next().unwrap_or(&self.url).to_string()
    }

    pub fn display_title(&self) -> &str {
        if self.url.is_empty() {
            "New Tab"
        } else if !self.title.is_empty() && self.title != self.url {
            &self.title
        } else {
            &self.url
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SecurityLevel {
    Unknown,
    Secure,
    Insecure,
}

#[derive(Clone)]
pub struct QuickLink {
    pub title: String,
    pub url: String,
}

pub struct BrowserState {
    pub tabs: Vec<TabState>,
    pub active_tab_index: usize,
    pub mouse_x: u32,
    pub mouse_y: u32,
    pub frame_count: u64,
    pub theme: Theme,
    pub palette: ColorPalette,
    pub address_bar_focused: bool,
    pub address_bar_content: String,
    pub bookmarks: Vec<QuickLink>,
    /// Which nav button is visually pressed (1=back, 2=fwd, 3=reload), cleared after 12 frames.
    pub nav_btn_pressed: u8,
    pub nav_btn_pressed_frame: u64,
}

impl BrowserState {
    pub fn new() -> Self {
        let default_palette = ColorPalette::KamelotDark;
        let tabs = vec![TabState::new("".to_string())];
        Self {
            tabs,
            active_tab_index: 0,
            mouse_x: 0,
            mouse_y: 0,
            frame_count: 0,
            palette: default_palette,
            theme: get_theme(default_palette),
            address_bar_focused: false,
            address_bar_content: String::new(),
            bookmarks: vec![
                QuickLink { title: "GitHub".to_string(),       url: "https://github.com".to_string() },
                QuickLink { title: "Hacker News".to_string(),  url: "https://news.ycombinator.com".to_string() },
                QuickLink { title: "Rust Lang".to_string(),    url: "https://www.rust-lang.org".to_string() },
                QuickLink { title: "MDN".to_string(),          url: "https://developer.mozilla.org".to_string() },
                QuickLink { title: "Servo".to_string(),        url: "https://servo.org".to_string() },
                QuickLink { title: "Crates.io".to_string(),    url: "https://crates.io".to_string() },
            ],
            nav_btn_pressed: 0,
            nav_btn_pressed_frame: 0,
        }
    }

    pub fn active_tab(&self) -> Option<&TabState> {
        self.tabs.get(self.active_tab_index)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut TabState> {
        self.tabs.get_mut(self.active_tab_index)
    }

    pub fn new_tab(&mut self, url: String) {
        self.tabs.push(TabState::new(url));
        self.active_tab_index = self.tabs.len() - 1;
        self.sync_address_bar();
    }

    pub fn close_tab(&mut self, index: usize) {
        if self.tabs.len() > 1 && index < self.tabs.len() {
            self.tabs.remove(index);
            if self.active_tab_index >= index && self.active_tab_index > 0 {
                self.active_tab_index -= 1;
            } else if self.active_tab_index >= self.tabs.len() {
                self.active_tab_index = self.tabs.len() - 1;
            }
            self.sync_address_bar();
        }
    }

    pub fn set_active_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab_index = index;
            self.sync_address_bar();
        }
    }

    pub fn sync_address_bar(&mut self) {
        if let Some(tab) = self.active_tab() {
            self.address_bar_content = tab.url.clone();
        }
    }

    pub fn set_theme(&mut self, palette: ColorPalette) {
        self.palette = palette;
        self.theme = get_theme(palette);
    }

    pub fn cycle_theme(&mut self) {
        self.set_theme(self.palette.cycle());
    }

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

    pub fn toggle_bookmark_for_active_tab(&mut self) {
        let (url, title, is_bookmarked) = match self.active_tab() {
            Some(tab) if !tab.url.is_empty() => (tab.url.clone(), tab.title.clone(), tab.is_bookmarked),
            _ => return,
        };
        if is_bookmarked {
            self.bookmarks.retain(|b| b.url != url);
        } else {
            self.bookmarks.push(QuickLink { title, url });
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.is_bookmarked = !is_bookmarked;
        }
    }

    pub fn check_if_bookmarked(&mut self) {
        let url = self.active_tab().map(|t| t.url.clone());
        if let Some(url) = url {
            if url.is_empty() {
                if let Some(tab) = self.active_tab_mut() { tab.is_bookmarked = false; }
                return;
            }
            let is_bookmarked = self.bookmarks.iter().any(|b| b.url == url);
            if let Some(tab) = self.active_tab_mut() { tab.is_bookmarked = is_bookmarked; }
        }
    }
}
