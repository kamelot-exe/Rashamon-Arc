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
}

impl TabState {
    pub fn new(url: String) -> Self {
        let title = if url.is_empty() {
            "New Tab".to_string()
        } else {
            url.clone()
        };
        Self {
            id: NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed),
            url,
            title,
            is_loading: false,
            is_pinned: false,
            is_bookmarked: false,
            security: SecurityLevel::Unknown,
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
}

impl BrowserState {
    pub fn new() -> Self {
        let default_palette = ColorPalette::KamelotDark;
        let tabs = vec![TabState::new("".to_string())];
        let active_tab_index = 0;

        Self {
            tabs,
            active_tab_index,
            mouse_x: 0,
            mouse_y: 0,
            frame_count: 0,
            palette: default_palette,
            theme: get_theme(default_palette),
            address_bar_focused: false,
            address_bar_content: "".to_string(),
            bookmarks: vec![
                QuickLink { title: "GitHub".to_string(), url: "https://github.com".to_string() },
                QuickLink { title: "Rust Lang".to_string(), url: "https://www.rust-lang.org".to_string() },
                QuickLink { title: "Servo".to_string(), url: "https://servo.org".to_string() },
                QuickLink { title: "Hacker News".to_string(), url: "https://news.ycombinator.com".to_string() },
            ],
        }
    }

    pub fn active_tab(&self) -> Option<&TabState> {
        self.tabs.get(self.active_tab_index)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut TabState> {
        self.tabs.get_mut(self.active_tab_index)
    }

    pub fn new_tab(&mut self, url: String) {
        let new_tab = TabState::new(url);
        self.tabs.push(new_tab);
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
                if let Some(tab) = self.active_tab_mut() {
                    tab.is_bookmarked = false;
                }
                return;
            }
            let is_bookmarked = self.bookmarks.iter().any(|b| b.url == url);
            if let Some(tab) = self.active_tab_mut() {
                tab.is_bookmarked = is_bookmarked;
            }
        }
    }
}
