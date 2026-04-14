//! Browser UI state management.

/// The current browser state.
#[derive(Debug)]
pub struct BrowserState {
    pub url: String,
    pub title: Option<String>,
    pub tab_count: usize,
    mouse_x: i32,
    mouse_y: i32,
}

impl BrowserState {
    pub fn new() -> Self {
        Self {
            url: String::new(),
            title: Some("Rashamon Arc".to_string()),
            tab_count: 1,
            mouse_x: 0,
            mouse_y: 0,
        }
    }

    pub fn set_url(&mut self, url: String) {
        self.url = url;
    }

    pub fn url(&self) -> Option<&str> {
        if self.url.is_empty() {
            None
        } else {
            Some(&self.url)
        }
    }

    pub fn url_push_char(&mut self, c: char) {
        self.url.push(c);
    }

    pub fn url_pop_char(&mut self) {
        self.url.pop();
    }

    pub fn set_mouse_pos(&mut self, x: i32, y: i32) {
        self.mouse_x = x;
        self.mouse_y = y;
    }

    pub fn mouse_x(&self) -> i32 {
        self.mouse_x
    }

    pub fn mouse_y(&self) -> i32 {
        self.mouse_y
    }
}
