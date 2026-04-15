//! Rashamon UI — the main browser UI process.
mod display;
mod input;
mod theme;
mod ui_state;

use rashamon_net::HttpClient;
use rashamon_renderer::{framebuffer::Pixel, Framebuffer, RenderEngine};
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
        input::Key::Char('t') if input.is_ctrl_pressed() => state.cycle_theme(),
        input::Key::Char('w') if input.is_ctrl_pressed() => state.close_tab(state.active_tab_index),
        input::Key::Char('r') if input.is_ctrl_pressed() => engine.reload()?,
        input::Key::Enter => {
            if state.address_bar_focused {
                if let Some(tab) = state.active_tab_mut() {
                    let url = state.address_bar_content.clone();
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
    const TOP_BAR_HEIGHT: u32 = 70;
    const TAB_BAR_HEIGHT: u32 = 32;
    const TAB_WIDTH: u32 = 220;
    const TAB_SEP: u32 = 2;

    if y < TOP_BAR_HEIGHT {
        if y < TAB_BAR_HEIGHT {
            // Tab clicks
            let mut tab_x = 20;
            for i in 0..state.tabs.len() {
                if (tab_x..tab_x + TAB_WIDTH).contains(&x) {
                    state.set_active_tab(i);
                    return;
                }
                tab_x += TAB_WIDTH + TAB_SEP;
            }
        } else {
            // Control and Address Bar clicks
            if (20..60).contains(&x) { engine.go_back().ok(); return; }
            if (70..110).contains(&x) { engine.go_forward().ok(); return; }
            if (120..160).contains(&x) { engine.reload().ok(); return; }

            let bar_x = 200;
            let bar_w = FB_WIDTH - 300;
            if (bar_x..bar_x + bar_w).contains(&x) {
                state.address_bar_focused = true;
            } else {
                state.address_bar_focused = false;
                state.sync_address_bar();
            }
        }
    } else {
        state.address_bar_focused = false;
        state.sync_address_bar();
    }
}

fn render_ui(fb: &mut Framebuffer, state: &BrowserState) {
    let theme = state.theme;
    const TOP_BAR_HEIGHT: u32 = 70;
    const TAB_BAR_HEIGHT: u32 = 32;
    const TAB_WIDTH: u32 = 220;
    const TAB_SEP: u32 = 2;

    fb.fill_rect(0, 0, fb.width, TOP_BAR_HEIGHT, theme.bg);
    fb.fill_rect(0, TAB_BAR_HEIGHT, fb.width, 1, theme.border);
    fb.fill_rect(0, TOP_BAR_HEIGHT - 1, fb.width, 1, theme.border);

    let mut tab_x = 20;
    for (i, tab) in state.tabs.iter().enumerate() {
        let is_active = i == state.active_tab_index;
        let is_hovered = state.mouse_y < TAB_BAR_HEIGHT && (tab_x..tab_x + TAB_WIDTH).contains(&state.mouse_x);

        let bg = if is_active { theme.bg } else if is_hovered { theme.tab_hover_bg } else { theme.tab_bg };
        let fg = if is_active { theme.tab_active_fg } else { theme.tab_fg };

        fb.fill_rect(tab_x, 0, TAB_WIDTH, TAB_BAR_HEIGHT, bg);
        if is_active {
            fb.fill_rect(tab_x, TAB_BAR_HEIGHT, TAB_WIDTH, 1, theme.bg); // Hide separator for active tab
        }
        
        // Placeholder for tab title
        fb.fill_rect(tab_x + 10, 10, (tab.title.len() * 7).min(180) as u32, 12, fg);
        tab_x += TAB_WIDTH + TAB_SEP;
    }

    let btn_y = 38;
    fb.fill_rect(25, btn_y, 30, 20, theme.fg); // Back
    fb.fill_rect(75, btn_y, 30, 20, theme.fg); // Forward
    fb.fill_rect(125, btn_y, 30, 20, theme.fg); // Reload

    let bar_x = 200;
    let bar_w = fb.width - 300;
    let bar_y = 34;
    let bar_h = 30;
    fb.fill_rect(bar_x, bar_y, bar_w, bar_h, theme.address_bar_bg);
    if state.address_bar_focused {
        fb.fill_rect(bar_x - 1, bar_y - 1, bar_w + 2, bar_h + 2, theme.accent);
        fb.fill_rect(bar_x, bar_y, bar_w, bar_h, theme.address_bar_bg);
    }
    
    let text_to_render = &state.address_bar_content;
    fb.fill_rect(bar_x + 10, bar_y + 8, (text_to_render.len() * 7).min(bar_w - 20) as u32, 14, theme.address_bar_fg);
}
