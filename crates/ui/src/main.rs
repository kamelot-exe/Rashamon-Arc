//! Rashamon Arc — main browser UI process.
mod display;
mod draw;
mod font;
mod input;
mod theme;
mod ui_state;

use crate::font::FontManager;
use rashamon_net::HttpClient;
use rashamon_renderer::{Framebuffer, RenderEngine};
use ui_state::BrowserState;

const FB_WIDTH: u32  = 1920;
const FB_HEIGHT: u32 = 1080;

// ── Layout constants ──────────────────────────────────────────────────────────
const TAB_BAR_HEIGHT: u32  = 38;
const CHROME_BAR_HEIGHT: u32 = 44;
const TOP_BAR_HEIGHT: u32  = TAB_BAR_HEIGHT + CHROME_BAR_HEIGHT; // 82

const TAB_START_X: u32   = 8;
const TAB_SEP: u32        = 2;
const TAB_MAX_W: u32      = 180;
const TAB_MIN_W: u32      = 80;
const TAB_NEW_BTN_W: u32  = 36;

const ADDR_BAR_W: u32 = 700;
const ADDR_BAR_H: u32 = 30;
const ADDR_BAR_R: u32 = 15;

// Loading behaviour: minimum frames before resolving, max before error.
const LOAD_MIN_FRAMES: u64 = 60;   // 1 s  — ensures loading state is visible
const LOAD_TIMEOUT_FRAMES: u64 = 360; // 6 s  — then show error

// Error-page retry button geometry (shared between draw + click)
const RETRY_BTN_W: u32 = 140;
const RETRY_BTN_H: u32 = 38;

fn compute_tab_width(num_tabs: usize) -> u32 {
    let available = FB_WIDTH.saturating_sub(TAB_START_X + TAB_NEW_BTN_W + 12);
    let n = num_tabs.max(1) as u32;
    let w = (available / n).saturating_sub(TAB_SEP);
    w.min(TAB_MAX_W).max(TAB_MIN_W)
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("=== Rashamon Arc ===");

    let sdl_context    = sdl2::init()?;
    let video_subsystem = sdl_context.video()?;
    let _ = sdl_context.mouse().show_cursor(true);
    let event_pump     = sdl_context.event_pump()?;

    let font_data    = include_bytes!("../assets/DejaVuSansMono.ttf");
    let font_manager = FontManager::new(font_data)?;

    let mut fb      = Framebuffer::new(FB_WIDTH, FB_HEIGHT);
    let mut engine  = RenderEngine::new()?;
    let _http       = HttpClient::new();
    let mut state   = BrowserState::new();
    let mut display = display::Display::new(&video_subsystem, FB_WIDTH, FB_HEIGHT)?;
    let mut input_handler = input::InputHandler::new(event_pump)?;

    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let url = resolve_url(&args[1]);
        do_navigate(&mut state, &mut engine, &url).ok();
    }

    let mut running = true;
    while running {
        state.frame_count += 1;
        state.tick_nav_btn();

        if let Some(event) = input_handler.poll_event()? {
            match event {
                input::Event::Quit => running = false,
                input::Event::KeyPress(key) => {
                    handle_keypress(&mut state, &mut engine, &mut running, key, &input_handler)?
                }
                input::Event::MouseMove { x, y } => {
                    state.set_mouse_pos(x.max(0) as u32, y.max(0) as u32);
                }
                input::Event::MouseDown { x, y, button } => {
                    if button == 1 {
                        handle_mouse_down(&mut state, &mut engine, x as u32, y as u32);
                    }
                }
            }
        }

        // ── Loading state management ─────────────────────────────────────────
        update_loading_state(&mut state, &mut engine);
        state.check_if_bookmarked();

        // ── Render ───────────────────────────────────────────────────────────
        fb.clear(state.theme.bg);
        engine.render(&mut fb)?;
        render_ui(&mut fb, &state, &font_manager);
        display.present(&fb)?;

        std::thread::sleep(std::time::Duration::from_millis(16));
    }

    Ok(())
}

// ── Loading management ────────────────────────────────────────────────────────

fn update_loading_state(state: &mut BrowserState, engine: &mut RenderEngine) {
    let (is_loading, load_start, url_clone) = match state.active_tab() {
        Some(t) => (t.is_loading, t.load_start_frame, t.url.clone()),
        None => return,
    };

    if !is_loading { return; }

    let elapsed = state.frame_count.saturating_sub(load_start);

    // Timeout → error
    if elapsed >= LOAD_TIMEOUT_FRAMES {
        if let Some(tab) = state.active_tab_mut() {
            tab.is_loading = false;
            tab.error = Some("Page unavailable".to_string());
        }
        return;
    }

    // Resolve once minimum time has passed and engine has a title
    if elapsed >= LOAD_MIN_FRAMES {
        if let Some(engine_title) = engine.title() {
            let engine_url = engine.url().unwrap_or_default();
            if let Some(tab) = state.active_tab_mut() {
                tab.is_loading = false;
                tab.error = None;
                tab.title = engine_title;
                // Keep tab.url consistent with what engine resolved to
                if !engine_url.is_empty() && engine_url == url_clone {
                    // already set
                }
            }
            state.sync_address_bar();
        }
    }
}

// ── Navigation helper ─────────────────────────────────────────────────────────

fn do_navigate(
    state: &mut BrowserState,
    engine: &mut RenderEngine,
    url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if url.is_empty() { return Ok(()); }
    let interim_title = hostname_from_url(url);
    let frame = state.frame_count;
    if let Some(tab) = state.active_tab_mut() {
        tab.url = url.to_string();
        tab.title = interim_title;
        tab.is_loading = true;
        tab.load_start_frame = frame;
        tab.error = None;
        tab.push_history(url);
    }
    engine.navigate(url)?;
    state.sync_address_bar();
    Ok(())
}

fn hostname_from_url(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

// ── URL resolution ────────────────────────────────────────────────────────────

fn resolve_url(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() { return String::new(); }
    let has_scheme = raw.starts_with("http://") || raw.starts_with("https://");
    let looks_like_url = !raw.contains(' ') && raw.contains('.');
    if has_scheme {
        raw.to_string()
    } else if looks_like_url {
        format!("https://{raw}")
    } else {
        format!("https://duckduckgo.com/?q={}", raw.replace(' ', "+"))
    }
}

// ── Keyboard input ────────────────────────────────────────────────────────────

fn handle_keypress(
    state: &mut BrowserState,
    engine: &mut RenderEngine,
    running: &mut bool,
    key: input::Key,
    input: &input::InputHandler,
) -> Result<(), Box<dyn std::error::Error>> {
    match key {
        input::Key::Escape => {
            if state.address_bar_focused {
                state.address_bar_focused = false;
                state.sync_address_bar();
            } else {
                *running = false;
            }
        }
        input::Key::Char('p') if input.is_ctrl_pressed() => state.cycle_theme(),
        input::Key::Char('t') if input.is_ctrl_pressed() => {
            state.new_tab("".to_string());
        }
        input::Key::Char('w') if input.is_ctrl_pressed() => {
            let idx = state.active_tab_index;
            state.close_tab(idx);
            // Navigate engine to the now-active tab
            if let Some(tab) = state.active_tab() {
                if !tab.url.is_empty() {
                    engine.navigate(&tab.url).ok();
                }
            }
        }
        input::Key::Char('r') if input.is_ctrl_pressed() => {
            let url = state.active_tab().map(|t| t.url.clone()).unwrap_or_default();
            if !url.is_empty() {
                state.press_nav_btn(3);
                do_navigate(state, engine, &url)?;
            }
        }
        input::Key::Enter => {
            if state.address_bar_focused {
                let raw = state.address_bar_content.trim().to_string();
                if raw.is_empty() {
                    state.address_bar_focused = false;
                } else {
                    let url = resolve_url(&raw);
                    do_navigate(state, engine, &url)?;
                    state.address_bar_focused = false;
                }
            }
        }
        input::Key::Backspace => {
            if state.address_bar_focused {
                state.address_bar_content.pop();
            }
        }
        input::Key::Char(c) => {
            if state.address_bar_focused {
                state.address_bar_content.push(c);
            }
        }
        _ => {}
    }
    Ok(())
}

// ── Mouse input ───────────────────────────────────────────────────────────────

fn handle_mouse_down(state: &mut BrowserState, engine: &mut RenderEngine, x: u32, y: u32) {
    let tw = compute_tab_width(state.tabs.len());

    if y < TAB_BAR_HEIGHT {
        handle_tab_bar_click(state, engine, x, y, tw);
    } else if y < TOP_BAR_HEIGHT {
        handle_chrome_bar_click(state, engine, x, y);
    } else {
        handle_content_click(state, engine, x, y);
    }
}

fn handle_tab_bar_click(
    state: &mut BrowserState,
    engine: &mut RenderEngine,
    x: u32,
    y: u32,
    tw: u32,
) {
    let _ = y;
    let mut tab_x = TAB_START_X;
    for i in 0..state.tabs.len() {
        let close_cx = tab_x + tw.saturating_sub(18);
        if x >= close_cx && x < tab_x + tw {
            state.close_tab(i);
            if let Some(tab) = state.active_tab() {
                let url = tab.url.clone();
                if !url.is_empty() { engine.navigate(&url).ok(); }
            }
            return;
        }
        if x >= tab_x && x < tab_x + tw {
            if i != state.active_tab_index {
                state.set_active_tab(i);
                if let Some(tab) = state.active_tab() {
                    let url = tab.url.clone();
                    if !url.is_empty() { engine.navigate(&url).ok(); }
                }
            }
            return;
        }
        tab_x += tw + TAB_SEP;
    }
    // New tab (+) button
    if x >= tab_x && x < tab_x + TAB_NEW_BTN_W {
        state.new_tab("".to_string());
    }
}

fn handle_chrome_bar_click(state: &mut BrowserState, engine: &mut RenderEngine, x: u32, y: u32) {
    let btn_r: u32 = 16;

    // Back
    if x >= 12 && x < 12 + btn_r * 2 {
        let back_url = state.active_tab_mut().and_then(|t| t.navigate_back());
        if let Some(url) = back_url {
            state.press_nav_btn(1);
            let frame = state.frame_count;
            if let Some(tab) = state.active_tab_mut() {
                tab.url = url.clone();
                tab.title = hostname_from_url(&url);
                tab.is_loading = true;
                tab.load_start_frame = frame;
                tab.error = None;
            }
            engine.navigate(&url).ok();
            state.sync_address_bar();
        }
        return;
    }

    // Forward
    if x >= 54 && x < 54 + btn_r * 2 {
        let fwd_url = state.active_tab_mut().and_then(|t| t.navigate_forward());
        if let Some(url) = fwd_url {
            state.press_nav_btn(2);
            let frame = state.frame_count;
            if let Some(tab) = state.active_tab_mut() {
                tab.url = url.clone();
                tab.title = hostname_from_url(&url);
                tab.is_loading = true;
                tab.load_start_frame = frame;
                tab.error = None;
            }
            engine.navigate(&url).ok();
            state.sync_address_bar();
        }
        return;
    }

    // Reload
    if x >= 96 && x < 96 + btn_r * 2 {
        let url = state.active_tab().map(|t| t.url.clone()).unwrap_or_default();
        if !url.is_empty() {
            state.press_nav_btn(3);
            let frame = state.frame_count;
            if let Some(tab) = state.active_tab_mut() {
                tab.is_loading = true;
                tab.load_start_frame = frame;
                tab.error = None;
            }
            engine.navigate(&url).ok();
        }
        return;
    }

    // Address bar
    let bar_x = (FB_WIDTH - ADDR_BAR_W) / 2;
    let bar_y = TAB_BAR_HEIGHT + (CHROME_BAR_HEIGHT - ADDR_BAR_H) / 2;

    // Bookmark icon (rightmost ~26px of bar)
    let bm_x = bar_x + ADDR_BAR_W - 26;
    if x >= bm_x && x < bar_x + ADDR_BAR_W && y >= bar_y && y < bar_y + ADDR_BAR_H {
        state.toggle_bookmark_for_active_tab();
        return;
    }

    if x >= bar_x && x < bar_x + ADDR_BAR_W && y >= bar_y && y < bar_y + ADDR_BAR_H {
        state.address_bar_focused = true;
        return;
    }

    state.address_bar_focused = false;
    state.sync_address_bar();
}

fn handle_content_click(state: &mut BrowserState, engine: &mut RenderEngine, x: u32, y: u32) {
    let (is_new_tab, is_error) = match state.active_tab() {
        Some(t) => (t.url.is_empty(), t.error.is_some()),
        None => return,
    };

    if is_error {
        // Retry button
        let (btn_x, btn_y) = retry_btn_pos();
        if x >= btn_x && x < btn_x + RETRY_BTN_W && y >= btn_y && y < btn_y + RETRY_BTN_H {
            let url = state.active_tab().map(|t| t.url.clone()).unwrap_or_default();
            if !url.is_empty() {
                do_navigate(state, engine, &url).ok();
            }
            return;
        }
    }

    if is_new_tab {
        // Search box → focus address bar
        let cx = FB_WIDTH / 2;
        let content_h = FB_HEIGHT - TOP_BAR_HEIGHT;
        let cy = TOP_BAR_HEIGHT + content_h / 2;
        let search_w: u32 = 600;
        let search_h: u32 = 48;
        let search_x = cx.saturating_sub(search_w / 2);
        let search_y = cy.saturating_sub(90);
        if x >= search_x && x < search_x + search_w && y >= search_y && y < search_y + search_h {
            state.address_bar_focused = true;
            return;
        }

        // Quick links
        let num_links = state.bookmarks.len().min(6) as u32;
        if num_links > 0 {
            let card_w: u32 = 120;
            let card_h: u32 = 100;
            let card_gap: u32 = 16;
            let row_w = num_links * card_w + (num_links - 1) * card_gap;
            let mut link_x = cx.saturating_sub(row_w / 2);
            let link_y = cy + 46;
            let urls: Vec<String> = state.bookmarks.iter().take(6).map(|b| b.url.clone()).collect();
            for url in urls {
                if x >= link_x && x < link_x + card_w && y >= link_y && y < link_y + card_h {
                    do_navigate(state, engine, &url).ok();
                    return;
                }
                link_x += card_w + card_gap;
            }
        }
    }

    state.address_bar_focused = false;
    state.sync_address_bar();
}

// ── Top-level render ──────────────────────────────────────────────────────────

fn render_ui(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;

    let is_new_tab = state.active_tab().map_or(false, |t| t.url.is_empty());
    let is_loading = state.active_tab().map_or(false, |t| t.is_loading && !t.url.is_empty());
    let is_error   = state.active_tab().map_or(false, |t| t.error.is_some());

    if is_new_tab {
        draw_new_tab_page(fb, state, font);
    } else if is_error {
        draw_error_page(fb, state, font);
    } else if is_loading {
        draw_loading_overlay(fb, state, font);
    }
    // Loaded state: engine already rendered into fb; just draw chrome on top.

    // ── Tab bar row ───────────────────────────────────────────────────────────
    fb.fill_rect(0, 0, fb.width, TAB_BAR_HEIGHT, theme.tab_bar_bg);
    draw_tab_row(fb, state, font);

    // Separator between tab row and chrome row
    fb.fill_rect(0, TAB_BAR_HEIGHT - 1, fb.width, 1, theme.border);
    // Erase separator under active tab (connected look)
    let tw = compute_tab_width(state.tabs.len());
    let active_x = TAB_START_X + state.active_tab_index as u32 * (tw + TAB_SEP);
    fb.fill_rect(active_x, TAB_BAR_HEIGHT - 1, tw, 2, theme.surface);

    // ── Chrome row ────────────────────────────────────────────────────────────
    fb.fill_rect(0, TAB_BAR_HEIGHT, fb.width, CHROME_BAR_HEIGHT, theme.surface);
    draw_chrome_row(fb, state, font);

    // Bottom border of chrome
    fb.fill_rect(0, TOP_BAR_HEIGHT, fb.width, 1, theme.border);
}

// ── Tab row ───────────────────────────────────────────────────────────────────

fn draw_tab_row(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let tw = compute_tab_width(state.tabs.len());
    const TAB_TOP: u32 = 4;
    const TAB_H: u32   = TAB_BAR_HEIGHT - TAB_TOP;

    let mut tab_x = TAB_START_X;
    for (i, tab) in state.tabs.iter().enumerate() {
        let is_active  = i == state.active_tab_index;
        let is_hovered = state.mouse_y < TAB_BAR_HEIGHT
            && state.mouse_x >= tab_x
            && state.mouse_x < tab_x + tw;

        let bg = if is_active { theme.tab_active_bg }
                 else if is_hovered { theme.tab_hover_bg }
                 else { theme.tab_bg };
        let fg = if is_active { theme.tab_active_fg } else { theme.tab_fg };

        draw::draw_rounded_rect_top(fb, tab_x, TAB_TOP, tw, TAB_H, 6, bg);

        // Active tab: extend down to merge with chrome row
        if is_active {
            fb.fill_rect(tab_x, TAB_BAR_HEIGHT - 2, tw, 3, theme.surface);
        }

        // Left accent line on active tab
        if is_active {
            fb.fill_rect(tab_x, TAB_TOP + 4, 2, TAB_H - 8, theme.accent);
        }

        // Tab title
        let title: &str = if tab.is_loading {
            tab.display_title()
        } else {
            tab.display_title()
        };
        let title_x = tab_x + 14;
        let title_y = TAB_TOP + (TAB_H / 2).saturating_sub(7);
        let close_reserve = if is_active || is_hovered { 24 } else { 8 };
        let max_w = tw.saturating_sub(title_x - tab_x + close_reserve);
        draw::draw_text(fb, font, title_x, title_y, title, 13.0, fg, max_w);

        // Close button
        if is_active || is_hovered {
            let close_cx = tab_x + tw.saturating_sub(16);
            let close_cy = TAB_TOP + TAB_H / 2;
            let close_hovered = state.mouse_x >= close_cx.saturating_sub(8)
                && state.mouse_x < close_cx + 8
                && state.mouse_y >= TAB_TOP
                && state.mouse_y < TAB_BAR_HEIGHT;
            if close_hovered {
                draw::draw_circle_filled(fb, close_cx, close_cy, 8, theme.tab_close_hover);
            }
            draw::draw_icon_close(fb, close_cx, close_cy, 7, fg);
        }

        // Loading progress bar at bottom of tab
        if tab.is_loading {
            let anim = (state.frame_count * 4 % tw as u64) as u32;
            fb.fill_rect(tab_x, TAB_BAR_HEIGHT - 3, anim, 2, theme.accent);
        }

        // Error indicator dot
        if tab.error.is_some() {
            let dot_cx = tab_x + tw.saturating_sub(28);
            fb.fill_rect(dot_cx, TAB_TOP + TAB_H / 2 - 3, 6, 6, theme.security_err);
        }

        // Pinned dot
        if tab.is_pinned {
            fb.fill_rect(tab_x + 5, TAB_TOP + 6, 4, 4, theme.accent);
        }

        tab_x += tw + TAB_SEP;
    }

    // New tab (+) button
    let add_cx = tab_x + TAB_NEW_BTN_W / 2;
    let add_cy = TAB_TOP + TAB_H / 2;
    let add_hovered = state.mouse_y < TAB_BAR_HEIGHT
        && state.mouse_x >= tab_x
        && state.mouse_x < tab_x + TAB_NEW_BTN_W;
    if add_hovered {
        draw::draw_circle_filled(fb, add_cx, add_cy, 13, theme.tab_hover_bg);
    }
    draw::draw_icon_add(fb, add_cx, add_cy, 10, theme.icon_fg);
}

// ── Chrome row ────────────────────────────────────────────────────────────────

fn draw_chrome_row(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let chrome_cy = TAB_BAR_HEIGHT + CHROME_BAR_HEIGHT / 2;

    let can_back    = state.active_tab().map_or(false, |t| t.can_go_back());
    let can_forward = state.active_tab().map_or(false, |t| t.can_go_forward());

    draw_nav_button(fb, state, 28, chrome_cy, NavBtn::Back,    can_back);
    draw_nav_button(fb, state, 70, chrome_cy, NavBtn::Forward, can_forward);
    draw_nav_button(fb, state, 112, chrome_cy, NavBtn::Reload, true);

    draw_address_bar(fb, state, font);

    draw::draw_icon_menu(fb, FB_WIDTH - 28, chrome_cy, state.theme.icon_fg);
}

enum NavBtn { Back, Forward, Reload }

fn draw_nav_button(
    fb: &mut Framebuffer,
    state: &BrowserState,
    cx: u32,
    cy: u32,
    btn: NavBtn,
    enabled: bool,
) {
    let theme   = state.theme;
    let btn_r: u32 = 16;
    let btn_id: u8 = match btn { NavBtn::Back => 1, NavBtn::Forward => 2, NavBtn::Reload => 3 };

    let is_hovered = state.mouse_y >= TAB_BAR_HEIGHT
        && state.mouse_y < TOP_BAR_HEIGHT
        && state.mouse_x >= cx.saturating_sub(btn_r)
        && state.mouse_x < cx + btn_r;

    let is_pressed = state.nav_btn_pressed == btn_id;

    if is_pressed {
        draw::draw_circle_filled(fb, cx, cy, btn_r, theme.accent);
    } else if is_hovered && enabled {
        draw::draw_circle_filled(fb, cx, cy, btn_r, theme.control_hover_bg);
    }

    let color = if !enabled {
        // Dimmed — can't navigate
        theme.fg_secondary
    } else if is_pressed {
        theme.accent_fg
    } else {
        theme.icon_fg
    };

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

    let bar_bg = if state.address_bar_focused {
        theme.address_bar_bg_focused
    } else {
        theme.address_bar_bg
    };
    let border_color = if state.address_bar_focused {
        theme.address_bar_border_focused
    } else {
        theme.address_bar_border
    };

    draw::draw_rounded_rect(fb,
        bar_x.saturating_sub(1), bar_y.saturating_sub(1),
        ADDR_BAR_W + 2, ADDR_BAR_H + 2, ADDR_BAR_R + 1, border_color);
    draw::draw_rounded_rect(fb, bar_x, bar_y, ADDR_BAR_W, ADDR_BAR_H, ADDR_BAR_R, bar_bg);

    let icon_x = bar_x + 14;
    let icon_y = bar_y + ADDR_BAR_H / 2;

    if let Some(tab) = state.active_tab() {
        if tab.is_loading && !tab.url.is_empty() {
            draw::draw_icon_spinner(fb, icon_x, icon_y, 5, state.frame_count, theme.icon_fg);
        } else if tab.error.is_some() {
            // Error indicator — red dot
            draw::draw_circle_filled(fb, icon_x, icon_y, 5, theme.security_err);
        } else if tab.url.starts_with("https://") {
            draw::draw_icon_lock(fb, icon_x, icon_y, theme.security_ok);
        } else if !tab.url.is_empty() {
            draw::draw_icon_globe(fb, icon_x, icon_y, theme.icon_fg);
        }
    }

    let text_x = bar_x + 34;
    let text_y = bar_y + (ADDR_BAR_H.saturating_sub(14)) / 2;
    let text_max_w = ADDR_BAR_W.saturating_sub(34 + 30);

    if state.address_bar_content.is_empty() && !state.address_bar_focused {
        draw::draw_text(fb, font, text_x, text_y, "Search or enter URL",
            14.0, theme.placeholder, text_max_w);
    } else {
        draw::draw_text(fb, font, text_x, text_y, &state.address_bar_content,
            14.0, theme.address_bar_fg, text_max_w);
        if state.address_bar_focused && (state.frame_count / 28) % 2 == 0 {
            let cw = font.text_width(&state.address_bar_content, 14.0);
            let cursor_x = (text_x + cw + 1).min(bar_x + ADDR_BAR_W - 34);
            fb.fill_rect(cursor_x, text_y, 2, 15, theme.accent);
        }
    }

    if let Some(tab) = state.active_tab() {
        let bm_x = bar_x + ADDR_BAR_W - 18;
        let star_color = if tab.is_bookmarked { theme.accent } else { theme.icon_fg };
        draw::draw_icon_star(fb, bm_x, icon_y, 11, star_color, tab.is_bookmarked);
    }
}

// ── New tab page ──────────────────────────────────────────────────────────────

fn draw_new_tab_page(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx = FB_WIDTH / 2;
    let content_h = FB_HEIGHT - TOP_BAR_HEIGHT;
    let cy = TOP_BAR_HEIGHT + content_h / 2;

    // Slightly lighter content background vs pure bg
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, content_h, theme.bg);

    // ── Branding ──────────────────────────────────────────────────────────────
    let title_text = "rashamon arc";
    let title_w = font.text_width(title_text, 32.0);
    draw::draw_text(fb, font, cx.saturating_sub(title_w / 2), cy.saturating_sub(200),
        title_text, 32.0, theme.fg, 600);

    let tagline = "your private arc of the web";
    let tagline_w = font.text_width(tagline, 15.0);
    draw::draw_text(fb, font, cx.saturating_sub(tagline_w / 2), cy.saturating_sub(156),
        tagline, 15.0, theme.fg_secondary, 600);

    // ── Search box ────────────────────────────────────────────────────────────
    let search_w: u32 = 600;
    let search_h: u32 = 48;
    let search_r: u32 = 24;
    let search_x = cx.saturating_sub(search_w / 2);
    let search_y = cy.saturating_sub(90);

    let search_focused = state.address_bar_focused;
    let border = if search_focused { theme.address_bar_border_focused } else { theme.address_bar_border };
    draw::draw_rounded_rect(fb, search_x.saturating_sub(1), search_y.saturating_sub(1),
        search_w + 2, search_h + 2, search_r + 1, border);
    draw::draw_rounded_rect(fb, search_x, search_y, search_w, search_h, search_r,
        theme.address_bar_bg);

    if state.address_bar_content.is_empty() {
        let hint = "Search or enter URL";
        let hw = font.text_width(hint, 15.0);
        draw::draw_text(fb, font, search_x + (search_w - hw) / 2,
            search_y + (search_h.saturating_sub(14)) / 2,
            hint, 15.0, theme.placeholder, search_w - 40);
    } else {
        draw::draw_text(fb, font, search_x + 24,
            search_y + (search_h.saturating_sub(14)) / 2,
            &state.address_bar_content, 15.0, theme.address_bar_fg, search_w - 48);
        if search_focused && (state.frame_count / 28) % 2 == 0 {
            let cw = font.text_width(&state.address_bar_content, 15.0);
            let cursor_x = (search_x + 24 + cw + 1).min(search_x + search_w - 24);
            fb.fill_rect(cursor_x, search_y + (search_h.saturating_sub(16)) / 2, 2, 16, theme.accent);
        }
    }

    // Keyboard hints
    let hints = "Ctrl+T  new tab   \u{2022}   Ctrl+W  close   \u{2022}   Ctrl+P  theme   \u{2022}   Ctrl+R  reload";
    let hw = font.text_width(hints, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(hw / 2), search_y + search_h + 14,
        hints, 11.0, theme.fg_secondary, 900);

    // ── Quick links ───────────────────────────────────────────────────────────
    draw_quick_links(fb, state, font, cx, cy);
}

const FAVICON_COLORS: [rashamon_renderer::framebuffer::Pixel; 8] = [
    rashamon_renderer::framebuffer::Pixel { r: 79,  g: 140, b: 255 },
    rashamon_renderer::framebuffer::Pixel { r: 52,  g: 168, b: 83  },
    rashamon_renderer::framebuffer::Pixel { r: 255, g: 152, b: 0   },
    rashamon_renderer::framebuffer::Pixel { r: 233, g: 30,  b: 99  },
    rashamon_renderer::framebuffer::Pixel { r: 156, g: 39,  b: 176 },
    rashamon_renderer::framebuffer::Pixel { r: 0,   g: 188, b: 212 },
    rashamon_renderer::framebuffer::Pixel { r: 121, g: 85,  b: 72  },
    rashamon_renderer::framebuffer::Pixel { r: 96,  g: 125, b: 139 },
];

fn draw_quick_links(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager, cx: u32, cy: u32) {
    let theme = state.theme;
    let card_w: u32 = 120;
    let card_h: u32 = 100;
    let card_gap: u32 = 16;
    let num = state.bookmarks.len().min(6) as u32;
    if num == 0 { return; }

    let row_w = num * card_w + (num.saturating_sub(1)) * card_gap;
    let mut card_x = cx.saturating_sub(row_w / 2);
    let card_y = cy + 46;

    let label = "Quick access";
    let lw = font.text_width(label, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(lw / 2), card_y.saturating_sub(20),
        label, 11.0, theme.fg_secondary, 200);

    for (i, bookmark) in state.bookmarks.iter().take(6).enumerate() {
        let fav_color = FAVICON_COLORS[i % FAVICON_COLORS.len()];
        let is_hovered = state.mouse_y >= card_y
            && state.mouse_y < card_y + card_h
            && state.mouse_x >= card_x
            && state.mouse_x < card_x + card_w
            && state.mouse_y >= TOP_BAR_HEIGHT;

        let card_bg = if is_hovered { theme.new_tab_card_hover_bg } else { theme.new_tab_card_bg };
        draw::draw_rounded_rect(fb, card_x, card_y, card_w, card_h, 10, card_bg);

        if is_hovered {
            // Subtle border on hover
            draw::draw_rounded_rect_outline(fb, card_x as i32, card_y as i32,
                card_w as i32, card_h as i32, 10, theme.accent);
        }

        let fav_cx = card_x + card_w / 2;
        let fav_cy = card_y + 32;
        draw::draw_circle_filled(fb, fav_cx, fav_cy, 20, fav_color);

        let first: String = bookmark.title.chars().next()
            .unwrap_or('?').to_uppercase().collect();
        let letter_w = font.text_width(&first, 16.0);
        draw::draw_text(fb, font, fav_cx.saturating_sub(letter_w / 2), fav_cy.saturating_sub(8),
            &first, 16.0, rashamon_renderer::framebuffer::Pixel::WHITE, 24);

        let title_y = card_y + card_h - 28;
        let tw_max = card_w.saturating_sub(12);
        let title_text_w = font.text_width(&bookmark.title, 12.0).min(tw_max);
        let title_x = card_x + (card_w - title_text_w) / 2;
        draw::draw_text(fb, font, title_x, title_y, &bookmark.title, 12.0, theme.fg, tw_max);

        card_x += card_w + card_gap;
    }
}

// ── Loading overlay ───────────────────────────────────────────────────────────

fn draw_loading_overlay(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx = FB_WIDTH / 2;
    let cy = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;

    // Dim the content area slightly
    let content_h = FB_HEIGHT - TOP_BAR_HEIGHT;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, content_h, theme.bg);

    // Spinner
    draw::draw_icon_spinner(fb, cx, cy.saturating_sub(20), 14, state.frame_count, theme.fg_secondary);

    // Loading text
    let dots_count = ((state.frame_count / 18) % 4) as usize;
    let dots = &"..."[..dots_count];
    let msg = format!("Loading{dots}");
    let mw = font.text_width(&msg, 14.0);
    draw::draw_text(fb, font, cx.saturating_sub(mw / 2), cy + 8,
        &msg, 14.0, theme.fg_secondary, 200);

    // URL hint
    if let Some(tab) = state.active_tab() {
        let host = tab.hostname();
        let hw = font.text_width(&host, 12.0);
        draw::draw_text(fb, font, cx.saturating_sub(hw / 2), cy + 30,
            &host, 12.0, theme.placeholder, 600);
    }

    // Progress bar at top of content area
    let elapsed = state.frame_count.saturating_sub(
        state.active_tab().map_or(0, |t| t.load_start_frame));
    let progress = ((elapsed as f32 / LOAD_MIN_FRAMES as f32) * FB_WIDTH as f32) as u32;
    let progress = progress.min(FB_WIDTH - 4);
    fb.fill_rect(0, TOP_BAR_HEIGHT + 1, progress, 2, theme.accent);
}

// ── Error page ────────────────────────────────────────────────────────────────

fn retry_btn_pos() -> (u32, u32) {
    let cx = FB_WIDTH / 2;
    let cy = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
    let btn_x = cx.saturating_sub(RETRY_BTN_W / 2);
    let btn_y = cy + 80;
    (btn_x, btn_y)
}

fn draw_error_page(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx = FB_WIDTH / 2;
    let content_h = FB_HEIGHT - TOP_BAR_HEIGHT;
    let cy = TOP_BAR_HEIGHT + content_h / 2;

    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, content_h, theme.bg);

    // Error circle icon
    let icon_cy = cy.saturating_sub(72);
    draw::draw_circle_filled(fb, cx, icon_cy, 30, theme.security_err);
    draw::draw_icon_close(fb, cx, icon_cy, 16,
        rashamon_renderer::framebuffer::Pixel::WHITE);

    // Title
    let err_title = "Page couldn't be loaded";
    let tw = font.text_width(err_title, 20.0);
    draw::draw_text(fb, font, cx.saturating_sub(tw / 2), cy.saturating_sub(18),
        err_title, 20.0, theme.fg, 700);

    // Error message
    let msg = state.active_tab()
        .and_then(|t| t.error.as_deref())
        .unwrap_or("The page is unavailable");
    let mw = font.text_width(msg, 14.0);
    draw::draw_text(fb, font, cx.saturating_sub(mw / 2), cy + 14,
        msg, 14.0, theme.fg_secondary, 700);

    // URL
    if let Some(url) = state.active_tab().map(|t| &t.url).filter(|u| !u.is_empty()) {
        let uw = font.text_width(url, 12.0).min(800);
        draw::draw_text(fb, font, cx.saturating_sub(uw / 2), cy + 40,
            url, 12.0, theme.placeholder, 800);
    }

    // Retry button
    let (btn_x, btn_y) = retry_btn_pos();
    let btn_hovered = state.mouse_x >= btn_x && state.mouse_x < btn_x + RETRY_BTN_W
        && state.mouse_y >= btn_y && state.mouse_y < btn_y + RETRY_BTN_H
        && state.mouse_y >= TOP_BAR_HEIGHT;

    let btn_bg = if btn_hovered { theme.accent } else { theme.surface };
    let btn_border = if btn_hovered { theme.accent } else { theme.border };
    draw::draw_rounded_rect(fb, btn_x.saturating_sub(1), btn_y.saturating_sub(1),
        RETRY_BTN_W + 2, RETRY_BTN_H + 2, 9, btn_border);
    draw::draw_rounded_rect(fb, btn_x, btn_y, RETRY_BTN_W, RETRY_BTN_H, 8, btn_bg);

    let lbl = "Try again";
    let lw = font.text_width(lbl, 14.0);
    let lbl_fg = if btn_hovered { theme.accent_fg } else { theme.fg };
    draw::draw_text(fb, font,
        btn_x + (RETRY_BTN_W.saturating_sub(lw)) / 2,
        btn_y + (RETRY_BTN_H.saturating_sub(14)) / 2,
        lbl, 14.0, lbl_fg, RETRY_BTN_W - 8);
}
