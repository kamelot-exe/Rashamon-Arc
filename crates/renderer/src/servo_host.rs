//! Servo host — manages the Servo rendering engine.
//!
//! This is a minimal stub that demonstrates the integration pattern.
//! Full Servo integration requires the `servo` crate which depends on
//! many system libraries (OpenGL, EGL, libxml2, etc.).

use crate::framebuffer::Framebuffer;

pub struct ServoHost {
    title: Option<String>,
    initialized: bool,
    history: Vec<String>,
    history_index: usize,
}

impl ServoHost {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        eprintln!("[servo] ServoHost initialized (stub — full Servo integration pending)");
        Ok(Self {
            title: None,
            initialized: false,
            history: Vec::new(),
            history_index: 0,
        })
    }

    pub fn navigate(&mut self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("[servo] navigate -> {url}");
        if !self.history.is_empty() && self.history_index < self.history.len() - 1 {
            self.history.truncate(self.history_index + 1);
        }
        if !url.is_empty() {
            self.history.push(url.to_string());
            self.history_index = self.history.len().saturating_sub(1);
        }
        self.title = Some(self.derive_title(url));
        self.initialized = true;
        Ok(())
    }

    pub fn render(&mut self, fb: &mut Framebuffer) -> Result<(), Box<dyn std::error::Error>> {
        if !self.initialized { return Ok(()); }
        self.render_stub(fb);
        Ok(())
    }

    fn render_stub(&self, fb: &mut Framebuffer) {
        use crate::framebuffer::Pixel;

        let url = match self.url() {
            Some(u) if !u.is_empty() => u,
            _ => return,
        };

        // The UI chrome covers 0..TOP_BAR_HEIGHT; content starts below.
        const TOP: u32 = 82;
        let content_h = fb.height.saturating_sub(TOP);
        let margin_x: u32 = 80;
        let content_w = fb.width.saturating_sub(margin_x * 2);

        // Page background — white like a real browser page
        let page_bg = Pixel { r: 255, g: 255, b: 255 };
        fb.fill_rect(0, TOP, fb.width, content_h, page_bg);

        // Thin page shadow / separator at top of content
        fb.fill_rect(0, TOP, fb.width, 1, Pixel { r: 210, g: 210, b: 215 });

        // ── Simulated page structure ──────────────────────────────────────────

        // Hero / banner area
        let hero_y = TOP + 32;
        let hero_h: u32 = 8;
        let title_color = Pixel { r: 32, g: 32, b: 34 };
        let text_color  = Pixel { r: 160, g: 160, b: 165 };
        let img_color   = Pixel { r: 228, g: 232, b: 240 };

        // Page title — wide dark bar
        fb.fill_rect(margin_x, hero_y, content_w * 7 / 10, hero_h, title_color);
        fb.fill_rect(margin_x, hero_y + 14, content_w * 5 / 10, 5, title_color);

        // Subtitle line
        fb.fill_rect(margin_x, hero_y + 30, content_w * 4 / 10, 4, text_color);

        // ── Body columns ──────────────────────────────────────────────────────

        let body_y = hero_y + 55;
        let col_w = content_w * 6 / 10;   // left text column
        let img_x = margin_x + col_w + 24;
        let img_w = content_w.saturating_sub(col_w + 24);
        let img_h: u32 = 140;

        // Simulated image / card on the right
        fb.fill_rect(img_x, body_y, img_w, img_h, img_color);
        // Image inner detail lines
        let img_mid_y = body_y + img_h / 2;
        fb.fill_rect(img_x + 16, img_mid_y - 6, img_w.saturating_sub(32), 4, Pixel { r: 200, g: 205, b: 215 });
        fb.fill_rect(img_x + 16, img_mid_y + 4,  img_w * 2 / 3, 4, Pixel { r: 200, g: 205, b: 215 });

        // Text paragraph lines in left column
        let line_gap: u32 = 16;
        let line_heights = [4u32, 4, 4, 4, 3, 4, 3, 4];
        let line_widths_frac = [10u32, 10, 10, 8, 10, 10, 6, 10];
        for (i, (&lh, &wf)) in line_heights.iter().zip(line_widths_frac.iter()).enumerate() {
            let ly = body_y + i as u32 * line_gap;
            fb.fill_rect(margin_x, ly, col_w * wf / 10, lh, text_color);
        }

        // Second paragraph below image
        let para2_y = body_y + img_h + 28;
        let para2_lines = [10u32, 10, 8, 10, 7];
        for (i, &wf) in para2_lines.iter().enumerate() {
            let ly = para2_y + i as u32 * line_gap;
            fb.fill_rect(margin_x, ly, content_w * wf / 10, 4, text_color);
        }

        // Footer strip
        let footer_y = fb.height.saturating_sub(40);
        fb.fill_rect(0, footer_y, fb.width, 1, Pixel { r: 220, g: 220, b: 225 });
        fb.fill_rect(0, footer_y + 1, fb.width, fb.height - footer_y - 1,
            Pixel { r: 248, g: 248, b: 250 });

        // URL indicator (dimmed, shows what page is loaded)
        let _ = url; // acknowledged — real Servo would render the actual page
    }

    fn derive_title(&self, url: &str) -> String {
        if url.is_empty() { return "New Tab".to_string(); }
        url.trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_start_matches("www.")
            .split('/')
            .next()
            .unwrap_or(url)
            .to_string()
    }

    pub fn go_back(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.history_index > 0 {
            self.history_index -= 1;
            if let Some(url) = self.history.get(self.history_index).cloned() {
                self.title = Some(self.derive_title(&url));
            }
        }
        Ok(())
    }

    pub fn go_forward(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.history_index + 1 < self.history.len() {
            self.history_index += 1;
            if let Some(url) = self.history.get(self.history_index).cloned() {
                self.title = Some(self.derive_title(&url));
            }
        }
        Ok(())
    }

    pub fn reload(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    pub fn title(&self) -> Option<String> { self.title.clone() }

    pub fn url(&self) -> Option<String> {
        self.history.get(self.history_index).cloned()
    }
}
