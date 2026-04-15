//! Rashamon UI — the main browser UI process.
mod display;
mod input;
mod ui_state;

use rashamon_renderer::{Framebuffer, RenderEngine};
use rashamon_net::HttpClient;
use ui_state::BrowserState;

const FB_WIDTH: u32 = 1920;
const FB_HEIGHT: u32 = 1080;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("=== Rashamon Arc v0.1.0 ===");
    eprintln!("Design: Minimal Premium | Engine: Servo | Security: Adblock Active");

    let mut fb = Framebuffer::new(FB_WIDTH, FB_HEIGHT);
    let mut engine = RenderEngine::new()?;
    let _http = HttpClient::new();
    let mut state = BrowserState::new();
    let mut display = display::Display::new(FB_WIDTH, FB_HEIGHT)?;
    let mut input_handler = input::InputHandler::new()?;

    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let url = &args[1];
        state.set_url(url.clone());
        engine.navigate(url)?;
    }

    let mut running = true;
    while running {
        if let Some(event) = input_handler.poll_event()? {
            match event {
                input::Event::Quit => running = false,
                input::Event::KeyPress(key) => {
                    match key {
                        input::Key::Escape => {
                            if state.show_palette {
                                state.show_palette = false;
                            } else {
                                running = false;
                            }
                        }
                        // Вызов Command Palette: Ctrl + P
                        input::Key::Char('p') if input_handler.is_ctrl_pressed() => {
                            state.show_palette = !state.show_palette;
                        }
                        input::Key::Enter => {
                            if let Some(url) = state.url() {
                                // Core: Secure by default (Basic Adblock/Tracker filter)
                                let is_blocked = url.contains("ads.") || url.contains("tracker");
                                
                                if is_blocked {
                                    eprintln!("[security] Blocked navigation to: {}", url);
                                } else {
                                    eprintln!("[ui] Navigate -> {}", url);
                                    engine.navigate(&url)?;
                                    state.show_palette = false;
                                }
                            }
                        }
                        input::Key::Backspace => state.url_pop_char(),
                        input::Key::Char(c) => state.url_push_char(c),
                        _ => {}
                    }
                }
                input::Event::MouseMove { x, y } => state.set_mouse_pos(x, y),
                input::Event::MouseDown { x, y, button } => {
                    // Обработка клика по Top Bar (например, кнопка назад)
                    if button == 1 && y < 44 {
                        if x < 60 { engine.go_back()?; }
                    }
                }
            }
        }

        // Рендерим контент Servo
        engine.render(&mut fb)?;

        // Накладываем Premium UI Overlay
        render_ui_overlay(&mut fb, &state);

        display.present(&fb)?;
        std::thread::sleep(std::time::Duration::from_millis(16)); // ~60fps
    }

    Ok(())
}

fn render_ui_overlay(fb: &mut Framebuffer, state: &BrowserState) {
    use rashamon_renderer::framebuffer::Pixel;

    // Палитра Rashamon Arc
    let bg_dark = Pixel { r: 15, g: 15, b: 15 };     // Основной фон
    let accent = Pixel { r: 40, g: 40, b: 40 };      // Поля ввода
    let text_main = Pixel { r: 200, g: 200, b: 200 }; // Текст
    let highlight = Pixel { r: 100, g: 140, b: 255 }; // Приватный режим/Акцент

    // 1. Single Top Bar (44px - как в премиальных нативных приложениях)
    fb.fill_rect(0, 0, fb.width, 44, bg_dark);

    // Address Bar (Компактный, центрированный)
    let url_w = (fb.width as f32 * 0.5) as u32;
    let url_x = (fb.width - url_w) / 2;
    fb.fill_rect(url_x, 8, url_w, 28, accent);

    if let Some(url) = state.url() {
        let text_bar_w = (url.len() as u32 * 7).min(url_w - 20);
        fb.fill_rect(url_x + 10, 14, text_bar_w, 16, text_main);
    }

    // Индикатор Private Mode (если активен)
    if state.is_private {
        fb.fill_rect(url_x - 35, 12, 20, 20, highlight);
    }

    // 2. Command Palette (Оверлей)
    if state.show_palette {
        let p_w = 600u32;
        let p_h = 40u32;
        let p_x = (fb.width - p_w) / 2;
        let p_y = 120u32;

        // Рисуем только строку ввода (Minimalism)
        fb.fill_rect(p_x - 2, p_y - 2, p_w + 4, p_h + 4, highlight); // Border
        fb.fill_rect(p_x, p_y, p_w, p_h, bg_dark);
    }
}