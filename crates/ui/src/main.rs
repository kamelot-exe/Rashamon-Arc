//! Rashamon Arc — main browser UI process.
mod display;
mod draw;
mod font;
mod input;
mod theme;
mod ui_state;

use crate::font::FontManager;
use rashamon_net::HttpClient;
use rashamon_renderer::{framebuffer::Pixel, Framebuffer, RenderEngine};
use ui_state::{BrowserState, PageState, TabId, derive_title};

// ── Layout ────────────────────────────────────────────────────────────────────

const FB_WIDTH: u32       = 1920;
const FB_HEIGHT: u32      = 1080;
const TAB_BAR_HEIGHT: u32 = 38;
const CHROME_BAR_HEIGHT: u32 = 44;
const TOP_BAR_HEIGHT: u32 = TAB_BAR_HEIGHT + CHROME_BAR_HEIGHT; // 82

const TAB_START_X: u32  = 8;
const TAB_SEP: u32       = 2;
const TAB_MAX_W: u32     = 180;
const TAB_MIN_W: u32     = 80;
const TAB_NEW_BTN_W: u32 = 36;

const ADDR_BAR_W: u32 = 700;
const ADDR_BAR_H: u32 = 30;
const ADDR_BAR_R: u32 = 15;

// Loading timing (at 60 fps)
const LOAD_MIN_FRAMES: u64     = 60;  // 1 s minimum for visible loading state
const LOAD_TIMEOUT_FRAMES: u64 = 360; // 6 s → error

// Retry button (shared by draw + click hit-test)
const RETRY_BTN_W: u32 = 140;
const RETRY_BTN_H: u32 = 38;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn compute_tab_width(n: usize) -> u32 {
    let avail = FB_WIDTH.saturating_sub(TAB_START_X + TAB_NEW_BTN_W + 12);
    ((avail / n.max(1) as u32).saturating_sub(TAB_SEP))
        .min(TAB_MAX_W)
        .max(TAB_MIN_W)
}

fn retry_btn_pos() -> (u32, u32) {
    let cx = FB_WIDTH / 2;
    let cy = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
    (cx.saturating_sub(RETRY_BTN_W / 2), cy + 80)
}

/// Parse raw address-bar text into a full URL or DuckDuckGo search.
fn resolve_url(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() { return String::new(); }
    if raw.contains("://") { return raw.to_string(); }
    if !raw.contains(' ') && raw.contains('.') {
        return format!("https://{raw}");
    }
    format!("https://duckduckgo.com/?q={}", raw.replace(' ', "+"))
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("=== Rashamon Arc ===");

    let sdl           = sdl2::init()?;
    let video         = sdl.video()?;
    let _             = sdl.mouse().show_cursor(true);
    let event_pump    = sdl.event_pump()?;

    let font_data     = include_bytes!("../assets/DejaVuSansMono.ttf");
    let font          = FontManager::new(font_data)?;
    let mut fb        = Framebuffer::new(FB_WIDTH, FB_HEIGHT);
    let mut engine    = RenderEngine::new()?;
    let _http         = HttpClient::new();
    let mut state     = BrowserState::new();
    let mut display   = display::Display::new(&video, FB_WIDTH, FB_HEIGHT)?;
    let mut input     = input::InputHandler::new(event_pump)?;

    // Optional command-line URL
    if let Some(arg_url) = std::env::args().nth(1) {
        let url = resolve_url(&arg_url);
        if let Some(url) = state.begin_navigate(&url) {
            engine.navigate(&url).ok();
        }
    }

    let mut running = true;
    while running {
        state.frame_count += 1;
        state.tick_nav_btn();

        // ── Events ────────────────────────────────────────────────────────────
        if let Some(ev) = input.poll_event()? {
            match ev {
                input::Event::Quit => running = false,
                input::Event::KeyPress(k) =>
                    on_key(&mut state, &mut engine, &mut running, k, &input)?,
                input::Event::MouseMove { x, y } =>
                    state.set_mouse_pos(x.max(0) as u32, y.max(0) as u32),
                input::Event::MouseDown { x, y, button } if button == 1 =>
                    on_click(&mut state, &mut engine, x as u32, y as u32),
                _ => {}
            }
        }

        // ── Loading state machine ─────────────────────────────────────────────
        tick_loading(&mut state, &mut engine);
        state.refresh_bookmark_flag();

        // ── Render ────────────────────────────────────────────────────────────
        fb.clear(state.theme.bg);
        engine.render(&mut fb)?;
        render_ui(&mut fb, &state, &font);
        display.present(&fb)?;

        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    Ok(())
}

// ── Loading state machine ─────────────────────────────────────────────────────

fn tick_loading(state: &mut BrowserState, engine: &mut RenderEngine) {
    let Some(tab) = state.active_tab() else { return };
    if !tab.page_state.is_loading() { return; }

    let elapsed = state.frame_count.saturating_sub(tab.load_start_frame);

    if elapsed >= LOAD_TIMEOUT_FRAMES {
        state.fail_loading("Page unavailable");
        return;
    }
    if elapsed >= LOAD_MIN_FRAMES {
        if let Some(title) = engine.title() {
            state.resolve_loading(title);
        }
    }
}

// ── Keyboard ──────────────────────────────────────────────────────────────────

fn on_key(
    state: &mut BrowserState,
    engine: &mut RenderEngine,
    running: &mut bool,
    key: input::Key,
    input: &input::InputHandler,
) -> Result<(), Box<dyn std::error::Error>> {
    match key {
        input::Key::Escape => {
            if state.address_bar_focused {
                state.cancel_address_bar_edit();
            } else {
                *running = false;
            }
        }

        input::Key::Char('p') if input.is_ctrl_pressed() => state.cycle_theme(),

        input::Key::Char('t') if input.is_ctrl_pressed() => {
            state.open_new_tab();
        }

        input::Key::Char('w') if input.is_ctrl_pressed() => {
            let id = state.active_tab_id;
            state.close_tab(id);
            // Re-navigate engine to whichever tab is now active
            if let Some(url) = state.active_tab().map(|t| t.url.clone()).filter(|u| !u.is_empty()) {
                engine.navigate(&url).ok();
            }
        }

        input::Key::Char('r') if input.is_ctrl_pressed() => {
            state.press_nav_btn(3);
            if let Some(url) = state.reload() {
                engine.navigate(&url).ok();
            }
        }

        input::Key::Enter if state.address_bar_focused => {
            let raw = state.address_bar_input.trim().to_string();
            if !raw.is_empty() {
                let url = resolve_url(&raw);
                if let Some(url) = state.begin_navigate(&url) {
                    engine.navigate(&url).ok();
                }
            } else {
                state.cancel_address_bar_edit();
            }
        }

        input::Key::Backspace if state.address_bar_focused => {
            state.address_bar_input.pop();
        }

        input::Key::Char(c) if state.address_bar_focused => {
            state.address_bar_input.push(c);
        }

        _ => {}
    }
    Ok(())
}

// ── Mouse ─────────────────────────────────────────────────────────────────────

fn on_click(state: &mut BrowserState, engine: &mut RenderEngine, x: u32, y: u32) {
    if y < TAB_BAR_HEIGHT {
        click_tab_bar(state, engine, x);
    } else if y < TOP_BAR_HEIGHT {
        click_chrome_bar(state, engine, x, y);
    } else {
        click_content(state, engine, x, y);
    }
}

fn click_tab_bar(state: &mut BrowserState, engine: &mut RenderEngine, x: u32) {
    let tw = compute_tab_width(state.tabs.len());

    // Collect (tab_id, left_x, right_x, close_x) to avoid borrowing issues
    let slots: Vec<(TabId, u32, u32, u32)> = state.tabs.iter().enumerate().map(|(i, t)| {
        let lx = TAB_START_X + i as u32 * (tw + TAB_SEP);
        let close_x = lx + tw.saturating_sub(18);
        (t.id, lx, lx + tw, close_x)
    }).collect();

    for (id, lx, rx, close_x) in &slots {
        if x >= *close_x && x < *rx {
            // Close button
            state.close_tab(*id);
            if let Some(url) = state.active_tab().map(|t| t.url.clone()).filter(|u| !u.is_empty()) {
                engine.navigate(&url).ok();
            }
            return;
        }
        if x >= *lx && x < *rx {
            // Tab body — activate
            if *id != state.active_tab_id {
                state.activate_tab(*id);
                if let Some(url) = state.active_tab().map(|t| t.url.clone()).filter(|u| !u.is_empty()) {
                    engine.navigate(&url).ok();
                }
            }
            return;
        }
    }

    // New tab (+) button
    let next_x = TAB_START_X + state.tabs.len() as u32 * (tw + TAB_SEP);
    if x >= next_x && x < next_x + TAB_NEW_BTN_W {
        state.open_new_tab();
    }
}

fn click_chrome_bar(state: &mut BrowserState, engine: &mut RenderEngine, x: u32, y: u32) {
    let btn_r: u32 = 16;

    // Back
    if x >= 12 && x < 12 + btn_r * 2 {
        state.press_nav_btn(1);
        if let Some(url) = state.go_back() {
            engine.navigate(&url).ok();
        }
        return;
    }
    // Forward
    if x >= 54 && x < 54 + btn_r * 2 {
        state.press_nav_btn(2);
        if let Some(url) = state.go_forward() {
            engine.navigate(&url).ok();
        }
        return;
    }
    // Reload
    if x >= 96 && x < 96 + btn_r * 2 {
        state.press_nav_btn(3);
        if let Some(url) = state.reload() {
            engine.navigate(&url).ok();
        }
        return;
    }

    // Address bar
    let bar_x = (FB_WIDTH - ADDR_BAR_W) / 2;
    let bar_y = TAB_BAR_HEIGHT + (CHROME_BAR_HEIGHT - ADDR_BAR_H) / 2;

    // Bookmark star (rightmost 26px of bar)
    if x >= bar_x + ADDR_BAR_W - 26 && x < bar_x + ADDR_BAR_W
        && y >= bar_y && y < bar_y + ADDR_BAR_H
    {
        state.toggle_bookmark();
        return;
    }

    if x >= bar_x && x < bar_x + ADDR_BAR_W && y >= bar_y && y < bar_y + ADDR_BAR_H {
        state.focus_address_bar();
        return;
    }

    // Clicking elsewhere in chrome → cancel editing
    state.cancel_address_bar_edit();
}

fn click_content(state: &mut BrowserState, engine: &mut RenderEngine, x: u32, y: u32) {
    let page = state.active_tab().map(|t| t.page_state.clone());

    match page {
        Some(PageState::Error(_)) => {
            let (bx, by) = retry_btn_pos();
            if x >= bx && x < bx + RETRY_BTN_W && y >= by && y < by + RETRY_BTN_H {
                if let Some(url) = state.reload() {
                    engine.navigate(&url).ok();
                }
                return;
            }
        }

        Some(PageState::NewTab) => {
            let cx = FB_WIDTH / 2;
            let cy = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;

            // Search box → focus address bar
            let sw: u32 = 600;
            let sh: u32 = 48;
            let sx = cx.saturating_sub(sw / 2);
            let sy = cy.saturating_sub(90);
            if x >= sx && x < sx + sw && y >= sy && y < sy + sh {
                state.focus_address_bar();
                return;
            }

            // Quick link cards
            let num = state.bookmarks.len().min(6) as u32;
            if num > 0 {
                let cw: u32 = 120;
                let ch: u32 = 100;
                let gap: u32 = 16;
                let row_w = num * cw + (num - 1) * gap;
                let mut lx = cx.saturating_sub(row_w / 2);
                let ly = cy + 46;
                let urls: Vec<String> = state.bookmarks.iter().take(6)
                    .map(|b| b.url.clone()).collect();
                for url in urls {
                    if x >= lx && x < lx + cw && y >= ly && y < ly + ch {
                        if let Some(url) = state.begin_navigate(&url) {
                            engine.navigate(&url).ok();
                        }
                        return;
                    }
                    lx += cw + gap;
                }
            }
        }

        _ => {}
    }

    state.cancel_address_bar_edit();
}

// ── Top-level render ──────────────────────────────────────────────────────────

fn render_ui(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;

    // Content-area overlays (under the chrome)
    match state.active_tab().map(|t| &t.page_state) {
        Some(PageState::NewTab)    => draw_new_tab(fb, state, font),
        Some(PageState::Loading)   => draw_loading(fb, state, font),
        Some(PageState::Error(_))  => draw_error(fb, state, font),
        Some(PageState::Loaded) | None => {} // engine rendered the page
    }

    // Tab bar
    fb.fill_rect(0, 0, fb.width, TAB_BAR_HEIGHT, theme.tab_bar_bg);
    draw_tab_row(fb, state, font);

    // Separator between tab row and chrome row
    fb.fill_rect(0, TAB_BAR_HEIGHT - 1, fb.width, 1, theme.border);
    // Erase separator under the active tab for the "connected" look
    let tw = compute_tab_width(state.tabs.len());
    let active_pos = state.active_tab_pos();
    let active_x = TAB_START_X + active_pos as u32 * (tw + TAB_SEP);
    fb.fill_rect(active_x, TAB_BAR_HEIGHT - 1, tw, 2, theme.surface);

    // Chrome row
    fb.fill_rect(0, TAB_BAR_HEIGHT, fb.width, CHROME_BAR_HEIGHT, theme.surface);
    draw_chrome_row(fb, state, font);
    fb.fill_rect(0, TOP_BAR_HEIGHT, fb.width, 1, theme.border);
}

// ── Tab row ───────────────────────────────────────────────────────────────────

fn draw_tab_row(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let tw = compute_tab_width(state.tabs.len());
    const TOP: u32 = 4;
    const H: u32   = TAB_BAR_HEIGHT - TOP;

    for (i, tab) in state.tabs.iter().enumerate() {
        let tx = TAB_START_X + i as u32 * (tw + TAB_SEP);
        let is_active  = tab.id == state.active_tab_id;
        let is_hovered = state.mouse_y < TAB_BAR_HEIGHT
            && state.mouse_x >= tx && state.mouse_x < tx + tw;

        let bg = if is_active { theme.tab_active_bg }
                 else if is_hovered { theme.tab_hover_bg }
                 else { theme.tab_bg };
        let fg = if is_active { theme.tab_active_fg } else { theme.tab_fg };

        draw::draw_rounded_rect_top(fb, tx, TOP, tw, H, 6, bg);

        if is_active {
            // Extend down to merge visually with chrome row
            fb.fill_rect(tx, TAB_BAR_HEIGHT - 2, tw, 3, theme.surface);
            // Left accent bar
            fb.fill_rect(tx, TOP + 4, 2, H - 8, theme.accent);
        }

        // Title
        let title = tab.tab_title();
        let title_x = tx + 14;
        let title_y = TOP + (H / 2).saturating_sub(7);
        let close_reserve = if is_active || is_hovered { 24 } else { 8 };
        let max_title_w = tw.saturating_sub(title_x - tx + close_reserve);
        draw::draw_text(fb, font, title_x, title_y, title, 13.0, fg, max_title_w);

        // Close button
        if is_active || is_hovered {
            let cx = tx + tw.saturating_sub(16);
            let cy = TOP + H / 2;
            let close_hot = state.mouse_x >= cx.saturating_sub(8)
                && state.mouse_x < cx + 8
                && state.mouse_y >= TOP && state.mouse_y < TAB_BAR_HEIGHT;
            if close_hot {
                draw::draw_circle_filled(fb, cx, cy, 8, theme.tab_close_hover);
            }
            draw::draw_icon_close(fb, cx, cy, 7, fg);
        }

        // Loading progress bar at bottom edge of tab
        if tab.page_state.is_loading() {
            let anim = (state.frame_count * 4 % tw as u64) as u32;
            fb.fill_rect(tx, TAB_BAR_HEIGHT - 3, anim, 2, theme.accent);
        }

        // Error dot
        if tab.page_state.is_error() {
            let dot_x = tx + tw.saturating_sub(28);
            fb.fill_rect(dot_x, TOP + H / 2 - 3, 6, 6, theme.security_err);
        }

        if tab.is_pinned {
            fb.fill_rect(tx + 5, TOP + 6, 4, 4, theme.accent);
        }
    }

    // New tab (+) button
    let add_x = TAB_START_X + state.tabs.len() as u32 * (tw + TAB_SEP);
    let add_cx = add_x + TAB_NEW_BTN_W / 2;
    let add_cy = TOP + H / 2;
    let add_hot = state.mouse_y < TAB_BAR_HEIGHT
        && state.mouse_x >= add_x && state.mouse_x < add_x + TAB_NEW_BTN_W;
    if add_hot {
        draw::draw_circle_filled(fb, add_cx, add_cy, 13, theme.tab_hover_bg);
    }
    draw::draw_icon_add(fb, add_cx, add_cy, 10, theme.icon_fg);
}

// ── Chrome row ────────────────────────────────────────────────────────────────

fn draw_chrome_row(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let cy = TAB_BAR_HEIGHT + CHROME_BAR_HEIGHT / 2;
    let can_back    = state.active_tab().map_or(false, |t| t.can_go_back());
    let can_forward = state.active_tab().map_or(false, |t| t.can_go_forward());

    draw_nav_btn(fb, state, 28,  cy, NavBtn::Back,    can_back);
    draw_nav_btn(fb, state, 70,  cy, NavBtn::Forward, can_forward);
    draw_nav_btn(fb, state, 112, cy, NavBtn::Reload,  true);

    draw_address_bar(fb, state, font);
    draw::draw_icon_menu(fb, FB_WIDTH - 28, cy, state.theme.icon_fg);
}

enum NavBtn { Back, Forward, Reload }

fn draw_nav_btn(fb: &mut Framebuffer, state: &BrowserState, cx: u32, cy: u32, btn: NavBtn, enabled: bool) {
    let theme  = state.theme;
    let r: u32 = 16;
    let btn_id = match btn { NavBtn::Back => 1u8, NavBtn::Forward => 2, NavBtn::Reload => 3 };

    let hovered = state.mouse_y >= TAB_BAR_HEIGHT && state.mouse_y < TOP_BAR_HEIGHT
        && state.mouse_x >= cx.saturating_sub(r) && state.mouse_x < cx + r;
    let pressed = state.nav_btn_pressed == btn_id;

    if pressed {
        draw::draw_circle_filled(fb, cx, cy, r, theme.accent);
    } else if hovered && enabled {
        draw::draw_circle_filled(fb, cx, cy, r, theme.control_hover_bg);
    }

    let color = if pressed { theme.accent_fg }
                else if !enabled { theme.fg_secondary }
                else { theme.icon_fg };

    match btn {
        NavBtn::Back    => draw::draw_icon_back(fb, cx, cy, 10, color),
        NavBtn::Forward => draw::draw_icon_forward(fb, cx, cy, 10, color),
        NavBtn::Reload  => draw::draw_icon_reload(fb, cx, cy, 7, color),
    }
}

// ── Address bar ───────────────────────────────────────────────────────────────

fn draw_address_bar(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let bar_x = (FB_WIDTH - ADDR_BAR_W) / 2;
    let bar_y = TAB_BAR_HEIGHT + (CHROME_BAR_HEIGHT - ADDR_BAR_H) / 2;

    // Background + border
    let bg     = if state.address_bar_focused { theme.address_bar_bg_focused } else { theme.address_bar_bg };
    let border = if state.address_bar_focused { theme.address_bar_border_focused } else { theme.address_bar_border };
    draw::draw_rounded_rect(fb, bar_x.saturating_sub(1), bar_y.saturating_sub(1),
        ADDR_BAR_W + 2, ADDR_BAR_H + 2, ADDR_BAR_R + 1, border);
    draw::draw_rounded_rect(fb, bar_x, bar_y, ADDR_BAR_W, ADDR_BAR_H, ADDR_BAR_R, bg);

    // Left icon (lock / globe / spinner / error)
    let icon_x = bar_x + 14;
    let icon_y = bar_y + ADDR_BAR_H / 2;
    if let Some(tab) = state.active_tab() {
        match &tab.page_state {
            PageState::Loading => draw::draw_icon_spinner(fb, icon_x, icon_y, 5, state.frame_count, theme.icon_fg),
            PageState::Error(_) => draw::draw_circle_filled(fb, icon_x, icon_y, 5, theme.security_err),
            _ if tab.url.starts_with("https://") => draw::draw_icon_lock(fb, icon_x, icon_y, theme.security_ok),
            _ if !tab.url.is_empty() => draw::draw_icon_globe(fb, icon_x, icon_y, theme.icon_fg),
            _ => {}
        }
    }

    // Text
    let tx = bar_x + 34;
    let ty = bar_y + (ADDR_BAR_H.saturating_sub(14)) / 2;
    let max_w = ADDR_BAR_W.saturating_sub(34 + 30);

    if state.address_bar_input.is_empty() && !state.address_bar_focused {
        draw::draw_text(fb, font, tx, ty, "Search or enter URL", 14.0, theme.placeholder, max_w);
    } else {
        draw::draw_text(fb, font, tx, ty, &state.address_bar_input, 14.0, theme.address_bar_fg, max_w);
        if state.address_bar_focused && (state.frame_count / 28) % 2 == 0 {
            let cw = font.text_width(&state.address_bar_input, 14.0);
            let cx = (tx + cw + 1).min(bar_x + ADDR_BAR_W - 34);
            fb.fill_rect(cx, ty, 2, 15, theme.accent);
        }
    }

    // Bookmark star
    if let Some(tab) = state.active_tab() {
        let star_x = bar_x + ADDR_BAR_W - 18;
        let star_col = if tab.is_bookmarked { theme.accent } else { theme.icon_fg };
        draw::draw_icon_star(fb, star_x, icon_y, 11, star_col, tab.is_bookmarked);
    }
}

// ── New Tab page ──────────────────────────────────────────────────────────────

fn draw_new_tab(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx = FB_WIDTH / 2;
    let content_h = FB_HEIGHT - TOP_BAR_HEIGHT;
    let cy = TOP_BAR_HEIGHT + content_h / 2;

    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, content_h, theme.bg);

    // Branding
    let brand = "rashamon arc";
    let bw = font.text_width(brand, 32.0);
    draw::draw_text(fb, font, cx.saturating_sub(bw / 2), cy.saturating_sub(200), brand, 32.0, theme.fg, 600);

    let tagline = "your private arc of the web";
    let tw = font.text_width(tagline, 15.0);
    draw::draw_text(fb, font, cx.saturating_sub(tw / 2), cy.saturating_sub(156),
        tagline, 15.0, theme.fg_secondary, 600);

    // Search box
    let sw: u32 = 600;
    let sh: u32 = 48;
    let sr: u32 = 24;
    let sx = cx.saturating_sub(sw / 2);
    let sy = cy.saturating_sub(90);

    let border = if state.address_bar_focused { theme.address_bar_border_focused } else { theme.address_bar_border };
    draw::draw_rounded_rect(fb, sx.saturating_sub(1), sy.saturating_sub(1), sw + 2, sh + 2, sr + 1, border);
    draw::draw_rounded_rect(fb, sx, sy, sw, sh, sr, theme.address_bar_bg);

    if state.address_bar_input.is_empty() {
        let hint = "Search or enter URL";
        let hw = font.text_width(hint, 15.0);
        draw::draw_text(fb, font, sx + (sw - hw) / 2,
            sy + (sh.saturating_sub(14)) / 2, hint, 15.0, theme.placeholder, sw - 40);
    } else {
        draw::draw_text(fb, font, sx + 24, sy + (sh.saturating_sub(14)) / 2,
            &state.address_bar_input, 15.0, theme.address_bar_fg, sw - 48);
        if state.address_bar_focused && (state.frame_count / 28) % 2 == 0 {
            let cw = font.text_width(&state.address_bar_input, 15.0);
            let cur_x = (sx + 24 + cw + 1).min(sx + sw - 24);
            fb.fill_rect(cur_x, sy + (sh.saturating_sub(16)) / 2, 2, 16, theme.accent);
        }
    }

    let hints = "Ctrl+T  new tab   \u{2022}   Ctrl+W  close   \u{2022}   Ctrl+P  theme   \u{2022}   Ctrl+R  reload";
    let hw = font.text_width(hints, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(hw / 2), sy + sh + 14,
        hints, 11.0, theme.fg_secondary, 900);

    // Quick links
    draw_quick_links(fb, state, font, cx, cy);
}

const FAVICON_COLORS: [Pixel; 8] = [
    Pixel { r: 79,  g: 140, b: 255 },
    Pixel { r: 52,  g: 168, b: 83  },
    Pixel { r: 255, g: 152, b: 0   },
    Pixel { r: 233, g: 30,  b: 99  },
    Pixel { r: 156, g: 39,  b: 176 },
    Pixel { r: 0,   g: 188, b: 212 },
    Pixel { r: 121, g: 85,  b: 72  },
    Pixel { r: 96,  g: 125, b: 139 },
];

fn draw_quick_links(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager, cx: u32, cy: u32) {
    let theme = state.theme;
    let cw: u32 = 120;
    let ch: u32 = 100;
    let gap: u32 = 16;
    let num = state.bookmarks.len().min(6) as u32;
    if num == 0 { return; }

    let row_w = num * cw + (num - 1) * gap;
    let mut card_x = cx.saturating_sub(row_w / 2);
    let card_y = cy + 46;

    let lbl = "Quick access";
    let lw = font.text_width(lbl, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(lw / 2), card_y.saturating_sub(20),
        lbl, 11.0, theme.fg_secondary, 200);

    for (i, bm) in state.bookmarks.iter().take(6).enumerate() {
        let fav_col = FAVICON_COLORS[i % FAVICON_COLORS.len()];
        let hovered = state.mouse_y >= card_y && state.mouse_y < card_y + ch
            && state.mouse_x >= card_x && state.mouse_x < card_x + cw
            && state.mouse_y >= TOP_BAR_HEIGHT;

        let card_bg = if hovered { theme.new_tab_card_hover_bg } else { theme.new_tab_card_bg };
        draw::draw_rounded_rect(fb, card_x, card_y, cw, ch, 10, card_bg);
        if hovered {
            draw::draw_rounded_rect_outline(fb, card_x as i32, card_y as i32, cw as i32, ch as i32, 10, theme.accent);
        }

        let fav_cx = card_x + cw / 2;
        let fav_cy = card_y + 32;
        draw::draw_circle_filled(fb, fav_cx, fav_cy, 20, fav_col);

        let first: String = bm.title.chars().next().unwrap_or('?').to_uppercase().collect();
        let lw = font.text_width(&first, 16.0);
        draw::draw_text(fb, font, fav_cx.saturating_sub(lw / 2), fav_cy.saturating_sub(8),
            &first, 16.0, Pixel::WHITE, 24);

        let title_y = card_y + ch - 28;
        let max_tw = cw.saturating_sub(12);
        let title_w = font.text_width(&bm.title, 12.0).min(max_tw);
        let title_x = card_x + (cw - title_w) / 2;
        draw::draw_text(fb, font, title_x, title_y, &bm.title, 12.0, theme.fg, max_tw);

        card_x += cw + gap;
    }
}

// ── Loading overlay ───────────────────────────────────────────────────────────

fn draw_loading(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx = FB_WIDTH / 2;
    let cy = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;

    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);

    draw::draw_icon_spinner(fb, cx, cy.saturating_sub(20), 14, state.frame_count, theme.fg_secondary);

    let dots = match (state.frame_count / 18) % 4 { 1 => ".", 2 => "..", _ => "..." };
    let msg = format!("Loading{dots}");
    let mw = font.text_width(&msg, 14.0);
    draw::draw_text(fb, font, cx.saturating_sub(mw / 2), cy + 8, &msg, 14.0, theme.fg_secondary, 200);

    // Hostname hint
    if let Some(host) = state.active_tab().map(|t| derive_title(&t.url)) {
        let hw = font.text_width(&host, 12.0);
        draw::draw_text(fb, font, cx.saturating_sub(hw / 2), cy + 30,
            &host, 12.0, theme.placeholder, 600);
    }

    // Progress bar at top of content area
    let elapsed = state.frame_count.saturating_sub(
        state.active_tab().map_or(0, |t| t.load_start_frame));
    let progress = ((elapsed as f32 / LOAD_MIN_FRAMES as f32) * FB_WIDTH as f32) as u32;
    fb.fill_rect(0, TOP_BAR_HEIGHT + 1, progress.min(FB_WIDTH - 4), 2, theme.accent);
}

// ── Error page ────────────────────────────────────────────────────────────────

fn draw_error(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx = FB_WIDTH / 2;
    let cy = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;

    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);

    // Error circle
    let icon_cy = cy.saturating_sub(72);
    draw::draw_circle_filled(fb, cx, icon_cy, 30, theme.security_err);
    draw::draw_icon_close(fb, cx, icon_cy, 16, Pixel::WHITE);

    // Title
    let title = "Page couldn't be loaded";
    let tw = font.text_width(title, 20.0);
    draw::draw_text(fb, font, cx.saturating_sub(tw / 2), cy.saturating_sub(18),
        title, 20.0, theme.fg, 700);

    // Error message
    let msg = state.active_tab()
        .and_then(|t| t.page_state.error_msg())
        .unwrap_or("The page is unavailable");
    let mw = font.text_width(msg, 14.0);
    draw::draw_text(fb, font, cx.saturating_sub(mw / 2), cy + 14, msg, 14.0, theme.fg_secondary, 700);

    // URL
    if let Some(url) = state.active_tab().map(|t| t.url.as_str()).filter(|u| !u.is_empty()) {
        let uw = font.text_width(url, 12.0).min(800);
        draw::draw_text(fb, font, cx.saturating_sub(uw / 2), cy + 40, url, 12.0, theme.placeholder, 800);
    }

    // Retry button
    let (bx, by) = retry_btn_pos();
    let hovered = state.mouse_x >= bx && state.mouse_x < bx + RETRY_BTN_W
        && state.mouse_y >= by && state.mouse_y < by + RETRY_BTN_H;
    let btn_bg  = if hovered { theme.accent } else { theme.surface };
    let btn_brd = if hovered { theme.accent } else { theme.border };
    draw::draw_rounded_rect(fb, bx.saturating_sub(1), by.saturating_sub(1),
        RETRY_BTN_W + 2, RETRY_BTN_H + 2, 9, btn_brd);
    draw::draw_rounded_rect(fb, bx, by, RETRY_BTN_W, RETRY_BTN_H, 8, btn_bg);

    let lbl = "Try again";
    let lw = font.text_width(lbl, 14.0);
    let lbl_fg = if hovered { theme.accent_fg } else { theme.fg };
    draw::draw_text(fb, font,
        bx + (RETRY_BTN_W.saturating_sub(lw)) / 2,
        by + (RETRY_BTN_H.saturating_sub(14)) / 2,
        lbl, 14.0, lbl_fg, RETRY_BTN_W - 8);
}
