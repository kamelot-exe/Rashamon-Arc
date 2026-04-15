pub struct BrowserState {
    pub url_buffer: String,
    pub title: Option<String>,
    pub tab_count: usize,
    pub mouse_x: u32,
    pub mouse_y: u32,
    pub show_palette: bool,
}

impl BrowserState {
    pub fn new() -> Self {
        Self {
            url_buffer: String::new(),
            title: None,
            tab_count: 1,
            mouse_x: 0,
            mouse_y: 0,
            show_palette: false,
        }
    }

    pub fn url(&self) -> Option<String> {
        if self.url_buffer.is_empty() { None } else { Some(self.url_buffer.clone()) }
    }

    pub fn set_url(&mut self, url: String) { self.url_buffer = url; }
    pub fn url_push_char(&mut self, c: char) { self.url_buffer.push(c); }
    pub fn url_pop_char(&mut self) { self.url_buffer.pop(); }
    pub fn set_mouse_pos(&mut self, x: u32, y: u32) { self.mouse_x = x; self.mouse_y = y; }
}
