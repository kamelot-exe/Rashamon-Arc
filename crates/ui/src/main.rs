//! Rashamon UI — the main browser UI process.
//!
//! Responsibilities:
//! - Display framebuffer to screen (DRM/KMS framebuffer)
//! - Handle input events (keyboard, mouse)
//! - URL bar, navigation buttons, tabs
//! - Communicate with renderer and network processes via IPC

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
    eprintln!("Engine: Servo (stub) | Renderer: Software FB | Adblock: built-in");

    // Initialize subsystems.
    let mut fb = Framebuffer::new(FB_WIDTH, FB_HEIGHT);
    let mut engine = RenderEngine::new()?;
    let _http = HttpClient::new();
    let mut state = BrowserState::new();

    // Display output (stub — direct DRM/KMS display comes later).
    let mut display = display::Display::new(FB_WIDTH, FB_HEIGHT)?;

    // Input handler.
    let mut input_handler = input::InputHandler::new()?;

    // Load initial URL if provided.
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let url = &args[1];
        state.set_url(url.clone());
        engine.navigate(url)?;
    }

    // Main event loop.
    eprintln!("[ui] Entering main loop ({}x{})", FB_WIDTH, FB_HEIGHT);
    let mut running = true;
    while running {
        // Poll input events.
        if let Some(event) = input_handler.poll_event()? {
            match event {
                input::Event::Quit => {
                    running = false;
                }
                input::Event::KeyPress(key) => {
                    match key {
                        input::Key::Escape => running = false,
                        input::Key::Enter => {
                            if let Some(url) = state.url() {
                                eprintln!("[ui] Navigate -> {}", url);
                                engine.navigate(&url)?;
                            }
                        }
                        input::Key::Backspace => {
                            state.url_pop_char();
                        }
                        input::Key::Char(c) => {
                            state.url_push_char(c);
                        }
                        input::Key::Left => {}
                        input::Key::Right => {}
                        _ => {}
                    }
                }
                input::Event::MouseMove { x, y } => {
                    state.set_mouse_pos(x, y);
                }
                input::Event::MouseDown { x, y, button } => {
                    // Back button area: top-left corner
                    if button == 1 && x < 60 && y < 60 {
                        eprintln!("[ui] back clicked");
                        engine.go_back()?;
                    }
                }
            }
        }

        // Render the page.
        engine.render(&mut fb)?;

        // Render UI overlay (URL bar, tabs, etc).
        render_ui_overlay(&mut fb, &state);

        // Display the framebuffer.
        display.present(&fb)?;

        // Throttle to ~60fps (16ms per frame).
        std::thread::sleep(std::time::Duration::from_millis(16));
    }

    eprintln!("[ui] Shutdown.");
    Ok(())
}

/// Render the browser UI overlay on top of the page content.
fn render_ui_overlay(fb: &mut Framebuffer, state: &BrowserState) {
    use rashamon_renderer::framebuffer::Pixel;

    // Bottom bar: URL bar.
    let bar_h = 32u32;
    let bar_y = fb.height - bar_h;
    fb.fill_rect(0, bar_y, fb.width, bar_h, Pixel { r: 32, g: 32, b: 32 });

    // URL bar background.
    let url_x = 70u32;
    let url_w = fb.width - 80;
    fb.fill_rect(url_x, bar_y + 4, url_w, bar_h - 8, Pixel { r: 56, g: 56, b: 56 });

    // URL text (rendered as a simple colored bar for now).
    if let Some(url) = state.url() {
        let text_len = url.len().min(200);
        let bar_w = (text_len as u32 * 6).min(url_w - 20);
        fb.fill_rect(url_x + 10, bar_y + 8, bar_w, bar_h - 16, Pixel { r: 180, g: 180, b: 180 });
    }

    // Back button indicator.
    fb.fill_rect(4, bar_y + 4, 56, bar_h - 8, Pixel { r: 80, g: 120, b: 80 });

    // Tab bar at the top.
    let tab_h = 28u32;
    fb.fill_rect(0, 0, fb.width, tab_h, Pixel { r: 28, g: 28, b: 28 });

    // Active tab.
    fb.fill_rect(4, 2, 200, tab_h - 4, Pixel { r: 56, g: 56, b: 56 });

    // Tab title (as a bar).
    if let Some(ref title) = state.title {
        let title_len = title.len().min(30);
        fb.fill_rect(8, 6, title_len as u32 * 6, tab_h - 12, Pixel { r: 160, g: 160, b: 160 });
    }

    // FPS / memory indicator (debug).
    let info = format!("{}x{} | tabs: {}", fb.width, fb.height, state.tab_count);
    let info_len = info.len() as u32;
    fb.fill_rect(fb.width - info_len * 6 - 10, 4, info_len * 6, 20, Pixel { r: 100, g: 100, b: 100 });
}
