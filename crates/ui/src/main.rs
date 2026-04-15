//! Rashamon UI — the main browser UI process.
mod display;
mod draw;
mod input;
mod theme;
mod ui_state;

use rashamon_net::HttpClient;
use rashamon_renderer::{Framebuffer, RenderEngine};
use ui_state::BrowserState;

const FB_WIDTH: u32 = 1920;
const FB_HEIGHT: u32 = 1080;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("=== Rashamon Arc v0.1.0 ===");
    eprintln!("Design: Minimal Premium | Engine: Servo | Security: Adblock Active");

    let sdl_context = sdl2::init()?;
    let video_subsystem = sdl_context.video()?;
    let _ = sdl_context.mouse().show_cursor(true);
    let event_pump = sdl_context.event_pump()?;

    let mut fb = Framebuffer::new(FB_WIDTH, FB_HEIGHT);
    let mut engine = RenderEngine::new()?;
    let _http = HttpClient::new();
    let mut state = BrowserState::new();
    let mut display = display::Display::new(&video_subsystem, FB_WIDTH, FB_HEIGHT)?;
    let mut input_handler = input::InputHandler::new(event_pump)?;

    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let url = &args[1];
        state.tabs[0].url = url.clone();
        if let Some(tab) = state.active_tab_mut() {
            engine.navigate(&tab.url)?;
            tab.is_loading = true;
        }
    }
    state.sync_address_bar();

    let mut running = true;
    while running {
        state.frame_count += 1;
        if let Some(event) = input_handler.poll_event()? {
            match event {
                input::Event::Quit => running = false,
                input::Event::KeyPress(key) => {
                    handle_keypress(&mut state, &mut engine, &mut running, key, &input_handler)?
                }
                input::Event::MouseMove { x, y } => {
                    state.set_mouse_pos(x.max(0) as u32, y.max(0) as u32)
                }
                input::Event::MouseDown { x, y, button } => {
                    if button == 1 {
                        handle_mouse_down(&mut state, &mut engine, x as u32, y as u32);
                    }
                }
            }
        }

        if let Some(tab) = state.active_tab_mut() {
            if let Some(title) = engine.title() {
                if tab.title != title {
                    tab.title = title;
                }
            }
        }

        engine.render(&mut fb)?;
        render_ui(&mut fb, &state);
        display.present(&fb)?;

        std::thread::sleep(std::time::Duration::from_millis(16));
    }

    Ok(())
}

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
        input::Key::Char('t') if input.is_ctrl_pressed() => state.new_tab("".to_string()),
        input::Key::Char('w') if input.is_ctrl_pressed() => state.close_tab(state.active_tab_index),
        input::Key::Char('r') if input.is_ctrl_pressed() => engine.reload()?,
        input::Key::Enter => {
            if state.address_bar_focused {
                let url = state.address_bar_content.clone();
                if let Some(tab) = state.active_tab_mut() {
                    let final_url = if !url.starts_with("http://") && !url.starts_with("https://") {
                        format!("https://{}", url)
                    } else {
                        url
                    };
                    tab.url = final_url;
                    engine.navigate(&tab.url)?;
                    tab.is_loading = true;
                }
                state.address_bar_focused = false;
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

fn handle_mouse_down(state: &mut BrowserState, engine: &mut RenderEngine, x: u32, y: u32) {
    const TOP_BAR_HEIGHT: u32 = 48;
    const TAB_WIDTH: u32 = 220;
    const TAB_SEP: u32 = 1;
    const CONTROLS_START_X: u32 = 10;
    const TAB_START_X: u32 = 140;

    if y < TOP_BAR_HEIGHT {
        // Browser Controls
        if (CONTROLS_START_X..CONTROLS_START_X + 30).contains(&x) { engine.go_back().ok(); return; }
        if (CONTROLS_START_X + 35..CONTROLS_START_X + 65).contains(&x) { engine.go_forward().ok(); return; }
        if (CONTROLS_START_X + 70..CONTROLS_START_X + 100).contains(&x) { engine.reload().ok(); return; }

        // Tab clicks
        let mut tab_x = TAB_START_X;
        for i in 0..state.tabs.len() {
            let close_x = tab_x + TAB_WIDTH - 22;
            if (close_x..close_x + 18).contains(&x) {
                state.close_tab(i);
                if let Some(tab) = state.active_tab() { engine.navigate(&tab.url).ok(); }
                return;
            }
            if (tab_x..tab_x + TAB_WIDTH).contains(&x) {
                if i != state.active_tab_index {
                    state.set_active_tab(i);
                    if let Some(tab) = state.active_tab() { engine.navigate(&tab.url).ok(); }
                }
                return;
            }
            tab_x += TAB_WIDTH + TAB_SEP;
        }

        // New tab button
        if (tab_x..tab_x + 40).contains(&x) { state.new_tab("".to_string()); return; }

        // Address bar click
        let bar_x = (FB_WIDTH - 700) / 2;
        if (bar_x..bar_x + 700).contains(&x) {
            state.address_bar_focused = true;
        } else {
            state.address_bar_focused = false;
            state.sync_address_bar();
        }
    } else {
        state.address_bar_focused = false;
        state.sync_address_bar();
    }
}

fn render_ui(fb: &mut Framebuffer, state: &BrowserState) {
    let theme = state.theme;
    const TOP_BAR_HEIGHT: u32 = 48;

    // Draw content area background to separate it from the engine's render
    if let Some(tab) = state.active_tab() {
        if tab.url.is_empty() {
            fb.fill_rect(0, TOP_BAR_HEIGHT, fb.width, fb.height - TOP_BAR_HEIGHT, theme.bg);
            draw_new_tab_page(fb, state);
        }
    }

    // Draw top bar background and border
    fb.fill_rect(0, 0, fb.width, TOP_BAR_HEIGHT, theme.surface);
    fb.fill_rect(0, TOP_BAR_HEIGHT - 1, fb.width, 1, theme.border);

    draw_browser_chrome(fb, state);
}

fn draw_browser_chrome(fb: &mut Framebuffer, state: &BrowserState) {
    const CONTROLS_START_X: u32 = 20;
    const TOP_BAR_Y_CENTER: u32 = 24;

    let theme = state.theme;
    let icon_color = theme.icon_fg;

    draw::draw_icon_back(fb, CONTROLS_START_X, TOP_BAR_Y_CENTER, 10, icon_color);
    draw::draw_icon_forward(fb, CONTROLS_START_X + 45, TOP_BAR_Y_CENTER, 10, icon_color);
    draw::draw_icon_reload(fb, CONTROLS_START_X + 90, TOP_BAR_Y_CENTER, 7, icon_color);

    draw_tabs(fb, state);
    draw_address_bar(fb, state);
}

fn draw_tabs(fb: &mut Framebuffer, state: &BrowserState) {
    let theme = state.theme;
    const TOP_BAR_HEIGHT: u32 = 48;
    const TAB_WIDTH: u32 = 220;
    const TAB_SEP: u32 = 1;
    const TAB_START_X: u32 = 140;

    let mut tab_x = TAB_START_X;
    for (i, tab) in state.tabs.iter().enumerate() {
        let is_active = i == state.active_tab_index;
        let is_hovered = state.mouse_y < TOP_BAR_HEIGHT && (tab_x..tab_x + TAB_WIDTH).contains(&state.mouse_x);

        let bg = if is_active { theme.tab_active_bg } else if is_hovered { theme.tab_hover_bg } else { theme.tab_bg };
        let fg = if is_active { theme.tab_active_fg } else { theme.tab_fg };

        draw::draw_rounded_rect(fb, tab_x, 0, TAB_WIDTH, TOP_BAR_HEIGHT - 1, 4, bg);
        if is_active {
            fb.fill_rect(tab_x, TOP_BAR_HEIGHT - 2, TAB_WIDTH, 2, theme.bg);
        }
        
        let title = if tab.title.is_empty() { "New Tab" } else { &tab.title };
        draw::draw_text(fb, tab_x + 15, 18, title, fg, TAB_WIDTH - 45);

        if is_hovered || is_active {
            let close_x = tab_x + TAB_WIDTH - 20;
            let close_y = TOP_BAR_HEIGHT / 2;
            draw::draw_icon_close(fb, close_x, close_y, 8, fg);
        }

        if tab.is_loading {
            let progress = (state.frame_count * 4 % (TAB_WIDTH as u64)) as u32;
            fb.fill_rect(tab_x, TOP_BAR_HEIGHT - 3, progress, 2, theme.accent);
        }

        tab_x += TAB_WIDTH + TAB_SEP;
    }

    draw::draw_icon_add(fb, tab_x + 20, TOP_BAR_HEIGHT / 2, 16, theme.icon_fg);
}

fn draw_address_bar(fb: &mut Framebuffer, state: &BrowserState) {
    let theme = state.theme;
    let bar_w = 700;
    let bar_h = 32;
    let bar_x = (fb.width - bar_w) / 2;
    let bar_y = (48 - bar_h) / 2;

    draw::draw_rounded_rect(fb, bar_x, bar_y, bar_w, bar_h, 4, theme.address_bar_bg);
    if state.address_bar_focused {
        fb.fill_rect(bar_x - 1, bar_y - 1, bar_w + 2, bar_h + 2, theme.accent);
        draw::draw_rounded_rect(fb, bar_x, bar_y, bar_w, bar_h, 4, theme.address_bar_bg);
    }

    let icon_x = bar_x + 15;
    let icon_y = bar_y + bar_h / 2;
    let text_x = bar_x + 35;
    let text_y = bar_y + 10;

    if let Some(tab) = state.active_tab() {
        if tab.is_loading {
            draw::draw_icon_spinner(fb, icon_x, icon_y, 5, state.frame_count * 5, theme.icon_fg);
        } else {
            draw::draw_icon_lock(fb, icon_x, icon_y, theme.icon_fg);
        }
    }

    if state.address_bar_content.is_empty() && !state.address_bar_focused {
        draw::draw_text(fb, text_x, text_y, "Search or enter URL", theme.placeholder, bar_w - 50);
    } else {
        draw::draw_text(fb, text_x, text_y, &state.address_bar_content, theme.address_bar_fg, bar_w - 50);
        if state.address_bar_focused && (state.frame_count / 30) % 2 == 0 {
            let cursor_x = text_x + (state.address_bar_content.len() * 7) as u32;
            fb.fill_rect(cursor_x + 1, text_y - 2, 2, 14, theme.accent);
        }
    }
}

fn draw_new_tab_page(fb: &mut Framebuffer, state: &BrowserState) {
    let theme = state.theme;
    let center_x = fb.width / 2;
    let center_y = fb.height / 2;

    draw::draw_text(fb, center_x - 50, center_y - 200, "Rashamon Arc", theme.fg, 200);

    let input_w = 600;
    let input_h = 44;
    let input_y = center_y - 100;
    draw::draw_rounded_rect(fb, center_x - input_w / 2, input_y, input_w, input_h, 6, theme.surface);
    fb.fill_rect(center_x - input_w/2 -1, input_y -1, input_w+2, input_h+2, theme.border);
    draw::draw_rounded_rect(fb, center_x - input_w / 2, input_y, input_w, input_h, 6, theme.surface);
    draw::draw_text(fb, center_x - input_w / 2 + 20, input_y + 16, "Search or enter URL", theme.placeholder, input_w - 40);

    let mut link_x = center_x - (state.quick_links.len() as u32 * 160 / 2);
    let link_y = center_y - 20;
    for link in &state.quick_links {
        draw::draw_rounded_rect(fb, link_x, link_y, 140, 80, 6, theme.surface);
        draw::draw_text(fb, link_x + 15, link_y + 35, &link.title, theme.fg, 110);
        link_x += 150;
    }
}
