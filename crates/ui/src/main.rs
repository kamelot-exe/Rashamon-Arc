//! Rashamon Arc — main browser UI process.
mod display;
mod draw;
mod font;
mod input;
mod layout;
mod omnibox;
mod page;
mod persist;
mod theme;
mod ui_state;

use crate::font::FontManager;
use crate::layout::*;
use crate::page::{PageNode, parse_html};
use rashamon_net::HttpClient;
use rashamon_renderer::{Framebuffer, RenderEngine};
use rashamon_renderer::framebuffer::Pixel;
use ui_state::{BrowserState, DirtyFlags, OverlayKind, PageState, TabId, derive_title};

use std::sync::mpsc;

// Loading timing (at 60 fps)
const LOAD_MIN_FRAMES:     u64 = 60;   // 1 s minimum visible loading state
const LOAD_TIMEOUT_FRAMES: u64 = 360;  // 6 s → show error

// Page layout constants (shared between render and measure)
const MARGIN:  u32 = 120;
const MAX_W:   u32 = 880;
const PAD_TOP: u32 = 28;

// Scroll speeds
const SCROLL_LINE:  i32 = 40;
const SCROLL_WHEEL: i32 = 80;

// Private tab accent colour (purple stripe)
const PRIVATE_ACCENT: Pixel = Pixel { r: 130, g: 70, b: 200 };

// ── Helpers ───────────────────────────────────────────────────────────────────

#[inline]
fn scale(v: i32, factor: f32, max: u32) -> u32 {
    ((v.max(0) as f32 * factor) as u32).min(max - 1)
}

/// Run the omnibox pipeline and trigger navigation or an overlay as needed.
fn omnibox_navigate(
    raw:    &str,
    state:  &mut BrowserState,
    engine: &mut RenderEngine,
) {
    use omnibox::{resolve, MatchEntry, OmniboxResult, InternalRoute, DEFAULT_PROVIDER};

    let bm_iter = state.bookmarks.iter()
        .map(|b| MatchEntry { url: &b.url, title: &b.title });
    let hist_iter = state.global_history.iter().rev()
        .map(|e| MatchEntry { url: &e.url, title: &e.title });

    match resolve(raw, bm_iter, hist_iter, &DEFAULT_PROVIDER) {
        OmniboxResult::Navigate(url) => {
            if let Some(url) = state.begin_navigate(&url) {
                engine.navigate(&url).ok();
            }
        }
        OmniboxResult::OpenOverlay(InternalRoute::History)   => {
            state.toggle_overlay(OverlayKind::History);
        }
        OmniboxResult::OpenOverlay(InternalRoute::Bookmarks) => {
            state.toggle_overlay(OverlayKind::Bookmarks);
        }
        OmniboxResult::OpenOverlay(InternalRoute::Blank) => {
            state.open_new_tab();
        }
        OmniboxResult::Nothing => {
            state.cancel_address_bar_edit();
        }
    }
}

// ── Persistence helpers ───────────────────────────────────────────────────────

#[derive(Default)]
struct SaveDirty {
    bookmarks: bool,
    history:   bool,
    prefs:     bool,
}

impl SaveDirty {
    fn any(&self) -> bool { self.bookmarks || self.history || self.prefs }
}

/// Load persisted data into browser state on startup.
fn load_user_data(state: &mut BrowserState) {
    use crate::theme::ColorPalette;

    // Theme preference — applied first so the initial render uses the right theme.
    if let Some(theme_str) = persist::load_theme() {
        if let Some(palette) = ColorPalette::from_str(&theme_str) {
            state.apply_palette(palette);
        }
    }

    // Bookmarks — replace the built-in defaults if user has saved bookmarks.
    let stored_bm = persist::load_bookmarks();
    if !stored_bm.is_empty() {
        state.bookmarks = stored_bm.into_iter()
            .map(|b| ui_state::QuickLink::new(b.title, b.url))
            .collect();
    }

    // History — loaded oldest-first, same as storage order.
    let stored_hist = persist::load_history();
    for e in stored_hist {
        state.global_history.push(ui_state::GlobalHistoryEntry {
            url:   e.url,
            title: e.title,
            when:  0, // wall-clock unknown; ordering preserved by position
        });
    }
}

/// Flush any dirty saves in background threads (fire-and-forget).
fn flush_saves(state: &BrowserState, dirty: &mut SaveDirty) {
    use crate::theme::ColorPalette;

    if dirty.bookmarks {
        let bm: Vec<persist::StoredBookmark> = state.bookmarks.iter()
            .map(|b| persist::StoredBookmark { title: b.title.clone(), url: b.url.clone() })
            .collect();
        std::thread::spawn(move || persist::save_bookmarks(&bm));
        dirty.bookmarks = false;
    }

    if dirty.history {
        let hist: Vec<persist::StoredHistory> = state.global_history.iter()
            .map(|e| persist::StoredHistory { url: e.url.clone(), title: e.title.clone() })
            .collect();
        std::thread::spawn(move || persist::save_history(&hist));
        dirty.history = false;
    }

    if dirty.prefs {
        let name = state.palette.as_str().to_string();
        std::thread::spawn(move || persist::save_theme(&name));
        dirty.prefs = false;
    }
}

// ── Fetch / parse pipeline ────────────────────────────────────────────────────

enum FetchOutcome {
    Success { title: Option<String>, nodes: Vec<PageNode> },
    Failure(String),
}

struct PendingFetch {
    tab_id:   TabId,
    receiver: mpsc::Receiver<FetchOutcome>,
}

fn do_fetch(url: String) -> FetchOutcome {
    let mut client = HttpClient::new();
    match client.fetch_text(&url) {
        Err(reason) => FetchOutcome::Failure(reason),
        Ok(html)    => {
            let parsed = parse_html(&html);
            FetchOutcome::Success { title: parsed.title, nodes: parsed.nodes }
        }
    }
}

fn spawn_fetch(tab_id: TabId, url: String) -> PendingFetch {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(do_fetch(url)); });
    PendingFetch { tab_id, receiver: rx }
}

// ── Content height measurement ────────────────────────────────────────────────

fn measure_content_height(nodes: &[PageNode], font: &FontManager) -> u32 {
    let mut h: u32 = PAD_TOP;
    for node in nodes {
        match node {
            PageNode::Heading { level, text } => {
                let (size, before, after): (f32, u32, u32) = match level {
                    1 => (28.0, 18, 10), 2 => (22.0, 14, 8), _ => (17.0, 10, 6),
                };
                h += before + wrap_text(text, font, size, MAX_W).len() as u32 * (size as u32 + 4) + after;
            }
            PageNode::Paragraph(text) => {
                if text.trim().is_empty() { continue; }
                h += 4 + wrap_text(text, font, 14.0, MAX_W).len() as u32 * 22 + 10;
            }
            PageNode::ListItem(text) => {
                let b = format!("  \u{2022}  {text}");
                h += wrap_text(&b, font, 13.0, MAX_W).len() as u32 * 20 + 3;
            }
            PageNode::Pre(text) => { h += 8 + text.lines().count() as u32 * 18 + 30; }
            PageNode::HRule     => { h += 24; }
        }
    }
    h + 60
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("=== Rashamon Arc ===");

    let sdl   = sdl2::init()?;
    let video = sdl.video()?;
    let _     = sdl.mouse().show_cursor(true);
    video.text_input().start();

    let (win_w, win_h) = video.current_display_mode(0)
        .map(|m| (m.w as u32, m.h as u32))
        .unwrap_or((FB_WIDTH, FB_HEIGHT));
    let win_w   = win_w.min(FB_WIDTH);
    let win_h   = win_h.min(FB_HEIGHT);
    let scale_x = FB_WIDTH  as f32 / win_w as f32;
    let scale_y = FB_HEIGHT as f32 / win_h as f32;
    eprintln!("[main] window {}x{}, scale {:.2}x{:.2}", win_w, win_h, scale_x, scale_y);

    let event_pump = sdl.event_pump()?;
    let font_data  = include_bytes!("../assets/DejaVuSansMono.ttf");
    let font       = FontManager::new(font_data)?;
    let mut fb      = Framebuffer::new(FB_WIDTH, FB_HEIGHT);
    let mut engine  = RenderEngine::new()?;
    let _http       = HttpClient::new();
    let mut state   = BrowserState::new();
    load_user_data(&mut state);
    let mut display = display::Display::new(&video, win_w, win_h, FB_WIDTH, FB_HEIGHT)?;
    let mut input   = input::InputHandler::new(event_pump)?;

    let mut pending_fetch:    Option<PendingFetch>          = None;
    let mut buffered_outcome: Option<(TabId, FetchOutcome)> = None;
    let mut save_dirty = SaveDirty::default();

    if let Some(arg_url) = std::env::args().nth(1) {
        use omnibox::{classify_input, InputKind};
        let nav_url = match classify_input(&arg_url) {
            InputKind::Url(u)    => Some(u),
            InputKind::Search(q) => Some(omnibox::DEFAULT_PROVIDER.build_url(&q)),
            _                    => None,
        };
        if let Some(url) = nav_url {
            if let Some(url) = state.begin_navigate(&url) {
                engine.navigate(&url).ok();
                pending_fetch = Some(spawn_fetch(state.active_tab_id, url));
            }
        }
    }

    let mut running          = true;
    let mut last_blink_phase = 0u64;

    while running {
        state.frame_count += 1;
        state.tick_nav_btn();

        // ── Events ────────────────────────────────────────────────────────────
        while let Some(ev) = input.poll_event()? {
            match ev {
                input::Event::Quit => { running = false; break; }

                input::Event::KeyPress(k) =>
                    on_key(&mut state, &mut engine, &mut running, k, &input, &mut save_dirty)?,

                input::Event::MouseMove { x, y } => {
                    let fx = scale(x, scale_x, FB_WIDTH);
                    let fy = scale(y, scale_y, FB_HEIGHT);
                    state.set_mouse_pos(fx, fy);
                }

                input::Event::MouseDown { x, y, button } if button == 1 => {
                    let fx = scale(x, scale_x, FB_WIDTH);
                    let fy = scale(y, scale_y, FB_HEIGHT);
                    on_click(&mut state, &mut engine, fx, fy, &mut save_dirty);
                }

                input::Event::MouseWheel { delta } => {
                    if state.overlay != OverlayKind::None {
                        state.overlay_scroll_by(-delta);
                    } else {
                        state.scroll_by(-delta * SCROLL_WHEEL);
                    }
                }

                _ => {}
            }
        }

        // ── Spawn fetch ───────────────────────────────────────────────────────
        if let Some(tab) = state.active_tab() {
            if tab.page_state.is_loading() {
                let already = pending_fetch.as_ref().map_or(false, |pf| pf.tab_id == tab.id)
                    || buffered_outcome.as_ref().map_or(false, |(id, _)| *id == tab.id);
                if !already {
                    let tab_id = tab.id;
                    let url    = tab.url.clone();
                    pending_fetch = Some(spawn_fetch(tab_id, url));
                }
            }
        }

        // ── Poll fetch ────────────────────────────────────────────────────────
        if let Some(ref pf) = pending_fetch {
            match pf.receiver.try_recv() {
                Ok(outcome) => {
                    let tab_id = pf.tab_id;
                    pending_fetch = None;
                    buffered_outcome = Some((tab_id, outcome));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let tab_id = pf.tab_id;
                    pending_fetch = None;
                    if state.active_tab().map_or(false, |t| t.id == tab_id && t.page_state.is_loading()) {
                        state.fail_loading("Connection lost");
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        // ── Apply buffered result (after min visual loading time) ─────────────
        if let Some((tab_id, _)) = &buffered_outcome {
            let tab_id = *tab_id;
            let ready = state.active_tab()
                .filter(|t| t.id == tab_id && t.page_state.is_loading())
                .map_or(false, |t| {
                    state.frame_count.saturating_sub(t.load_start_frame) >= LOAD_MIN_FRAMES
                });
            if ready {
                let (_, outcome) = buffered_outcome.take().unwrap();
                match outcome {
                    FetchOutcome::Success { title, nodes } => {
                        let h = measure_content_height(&nodes, &font);
                        state.resolve_loading(title.unwrap_or_default(), nodes);
                        state.set_content_height(h);
                        save_dirty.history = true;
                    }
                    FetchOutcome::Failure(reason) => { state.fail_loading(&reason); }
                }
            }
        }

        // ── Loading timeout ───────────────────────────────────────────────────
        tick_loading(&mut state, &mut engine);

        // ── Continuous-animation dirty ────────────────────────────────────────
        if state.active_tab().map_or(false, |t| t.page_state.is_loading()) {
            state.dirty.tabs    = true;
            state.dirty.chrome  = true;
            state.dirty.content = true;
        }
        if state.address_bar_focused {
            let blink = state.frame_count / 28;
            if blink != last_blink_phase {
                last_blink_phase = blink;
                state.dirty_address_bar();
            }
        }

        // ── Lazy content height (after back/forward) ──────────────────────────
        if state.dirty.content && state.overlay == OverlayKind::None {
            if let Some(tab) = state.active_tab() {
                if matches!(tab.page_state, PageState::Loaded) && tab.content_height == 0 {
                    let nodes: Vec<PageNode> = tab.current_nodes().to_vec();
                    if !nodes.is_empty() {
                        let h = measure_content_height(&nodes, &font);
                        state.set_content_height(h);
                    }
                }
            }
        }

        // ── Render ────────────────────────────────────────────────────────────
        if state.dirty.any() {
            let dirty = state.dirty;
            state.dirty.clear();
            render_ui(&mut fb, &state, &font, dirty);
            display.present(&fb)?;
        }

        // ── Persist (fire-and-forget after render) ────────────────────────────
        if save_dirty.any() {
            flush_saves(&state, &mut save_dirty);
        }

        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    Ok(())
}

// ── Loading state machine ─────────────────────────────────────────────────────

fn tick_loading(state: &mut BrowserState, _engine: &mut RenderEngine) {
    let Some(tab) = state.active_tab() else { return };
    if !tab.page_state.is_loading() { return; }
    if state.frame_count.saturating_sub(tab.load_start_frame) >= LOAD_TIMEOUT_FRAMES {
        state.fail_loading("Request timed out");
    }
}

// ── Keyboard ──────────────────────────────────────────────────────────────────

fn on_key(
    state:      &mut BrowserState,
    engine:     &mut RenderEngine,
    running:    &mut bool,
    key:        input::Key,
    input:      &input::InputHandler,
    save_dirty: &mut SaveDirty,
) -> Result<(), Box<dyn std::error::Error>> {

    // ── Overlay-active keys (intercept before normal flow) ────────────────────
    if state.overlay != OverlayKind::None {
        match key {
            input::Key::Escape => { state.close_overlay(); return Ok(()); }
            input::Key::Enter  => {
                if let Some(url) = state.activate_overlay_item() {
                    if let Some(url) = state.begin_navigate(&url) {
                        engine.navigate(&url).ok();
                    }
                }
                return Ok(());
            }
            input::Key::Up   => { state.overlay_scroll_by(-1); return Ok(()); }
            input::Key::Down => { state.overlay_scroll_by( 1); return Ok(()); }
            input::Key::PageUp   => { state.overlay_scroll_by(-(OVERLAY_VISIBLE as i32)); return Ok(()); }
            input::Key::PageDown => { state.overlay_scroll_by( OVERLAY_VISIBLE as i32);  return Ok(()); }
            // Let Ctrl+shortcuts fall through so user can still open new tab etc.
            input::Key::Char(_) if input.is_ctrl_pressed() => {}
            _ => return Ok(()), // discard non-special keys while overlay is open
        }
    }

    match key {
        input::Key::Escape => {
            if state.address_bar_focused {
                state.cancel_address_bar_edit();
            } else {
                *running = false;
            }
        }

        input::Key::Char('p') if input.is_ctrl_pressed() => {
            state.cycle_theme();
            save_dirty.prefs = true;
        }

        input::Key::Char('t') if input.is_ctrl_pressed() => state.open_new_tab(),

        input::Key::Char('n') if input.is_ctrl_pressed() && input.is_shift_pressed() => {
            state.open_private_tab();
        }

        input::Key::Char('i') if input.is_ctrl_pressed() => state.open_private_tab(),

        input::Key::Char('w') if input.is_ctrl_pressed() => {
            let id = state.active_tab_id;
            state.close_tab(id);
            if let Some(url) = state.active_tab().map(|t| t.url.clone()).filter(|u| !u.is_empty()) {
                engine.navigate(&url).ok();
            }
        }

        input::Key::Char('r') if input.is_ctrl_pressed() => {
            state.press_nav_btn(3);
            if let Some(url) = state.reload() { engine.navigate(&url).ok(); }
        }

        input::Key::Char('h') if input.is_ctrl_pressed() => {
            state.toggle_overlay(OverlayKind::History);
        }

        input::Key::Char('b') if input.is_ctrl_pressed() => {
            state.toggle_overlay(OverlayKind::Bookmarks);
        }

        input::Key::Char('l') if input.is_ctrl_pressed() => {
            state.focus_address_bar();
        }

        input::Key::Enter if state.address_bar_focused => {
            let raw = state.address_bar_input.trim().to_string();
            state.cancel_address_bar_edit();
            if !raw.is_empty() {
                omnibox_navigate(&raw, state, engine);
            }
        }

        input::Key::Backspace if state.address_bar_focused => state.type_backspace(),
        input::Key::Char(c)   if state.address_bar_focused => state.type_char(c),

        // Scroll keys — only when not editing the address bar.
        input::Key::Up   if !state.address_bar_focused => state.scroll_by(-SCROLL_LINE),
        input::Key::Down if !state.address_bar_focused => state.scroll_by( SCROLL_LINE),
        input::Key::PageUp   if !state.address_bar_focused => {
            state.scroll_by(-((FB_HEIGHT - TOP_BAR_HEIGHT) as i32));
        }
        input::Key::PageDown if !state.address_bar_focused => {
            state.scroll_by((FB_HEIGHT - TOP_BAR_HEIGHT) as i32);
        }

        _ => {}
    }
    Ok(())
}

// ── Mouse ─────────────────────────────────────────────────────────────────────

fn on_click(state: &mut BrowserState, engine: &mut RenderEngine, x: u32, y: u32,
            save_dirty: &mut SaveDirty)
{
    if y < TAB_BAR_HEIGHT {
        click_tab_bar(state, engine, x);
    } else if y < TOP_BAR_HEIGHT {
        click_chrome_bar(state, engine, x, y, save_dirty);
    } else if state.overlay != OverlayKind::None {
        click_overlay(state, engine);
    } else {
        click_content(state, engine, x, y);
    }
}

fn click_tab_bar(state: &mut BrowserState, engine: &mut RenderEngine, x: u32) {
    let tw = state.tab_width;
    for i in 0..state.tabs.len() {
        let lx = TAB_START_X + i as u32 * (tw + TAB_SEP);
        let rx = lx + tw;
        if x >= lx && x < rx {
            let id = state.tabs[i].id;
            if x >= lx + tw.saturating_sub(18) {
                state.close_tab(id);
                if let Some(url) = state.active_tab().map(|t| t.url.clone()).filter(|u| !u.is_empty()) {
                    engine.navigate(&url).ok();
                }
            } else if id != state.active_tab_id {
                state.activate_tab(id);
                if let Some(url) = state.active_tab().map(|t| t.url.clone()).filter(|u| !u.is_empty()) {
                    engine.navigate(&url).ok();
                }
            }
            return;
        }
    }
    let next_x = TAB_START_X + state.tabs.len() as u32 * (tw + TAB_SEP);
    if x >= next_x && x < next_x + TAB_NEW_BTN_W {
        state.open_new_tab();
    }
}

fn click_chrome_bar(state: &mut BrowserState, engine: &mut RenderEngine, x: u32, y: u32,
                    save_dirty: &mut SaveDirty)
{
    let btn_r: u32 = 16;
    if x >= 12 && x < 12 + btn_r * 2 {
        state.press_nav_btn(1);
        if let Some(url) = state.go_back() { engine.navigate(&url).ok(); }
        return;
    }
    if x >= 54 && x < 54 + btn_r * 2 {
        state.press_nav_btn(2);
        if let Some(url) = state.go_forward() { engine.navigate(&url).ok(); }
        return;
    }
    if x >= 96 && x < 96 + btn_r * 2 {
        state.press_nav_btn(3);
        if let Some(url) = state.reload() { engine.navigate(&url).ok(); }
        return;
    }
    let bar_x = (FB_WIDTH - ADDR_BAR_W) / 2;
    let bar_y = TAB_BAR_HEIGHT + (CHROME_BAR_HEIGHT - ADDR_BAR_H) / 2;
    if x >= bar_x + ADDR_BAR_W - 26 && x < bar_x + ADDR_BAR_W
        && y >= bar_y && y < bar_y + ADDR_BAR_H
    {
        state.toggle_bookmark();
        save_dirty.bookmarks = true;
        return;
    }
    if x >= bar_x && x < bar_x + ADDR_BAR_W && y >= bar_y && y < bar_y + ADDR_BAR_H {
        state.focus_address_bar();
        return;
    }
    state.cancel_address_bar_edit();
}

fn click_overlay(state: &mut BrowserState, engine: &mut RenderEngine) {
    if let Some(url) = state.activate_overlay_item() {
        if let Some(url) = state.begin_navigate(&url) {
            engine.navigate(&url).ok();
        }
    }
}

fn click_content(state: &mut BrowserState, engine: &mut RenderEngine, x: u32, y: u32) {
    match state.active_tab().map(|t| t.page_state.clone()) {
        Some(PageState::Error(_)) => {
            let (bx, by) = layout::retry_btn_pos();
            if x >= bx && x < bx + RETRY_BTN_W && y >= by && y < by + RETRY_BTN_H {
                if let Some(url) = state.reload() { engine.navigate(&url).ok(); }
                return;
            }
        }
        Some(PageState::NewTab) => {
            let cx = FB_WIDTH / 2;
            let cy = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
            let sw: u32 = 600; let sh: u32 = 48;
            let sx = cx.saturating_sub(sw / 2);
            let sy = cy.saturating_sub(90);
            if x >= sx && x < sx + sw && y >= sy && y < sy + sh {
                state.focus_address_bar();
                return;
            }
            let num = state.bookmarks.len().min(6) as u32;
            if num > 0 {
                let row_w = num * QUICK_LINK_W + (num - 1) * QUICK_LINK_GAP;
                let mut lx = cx.saturating_sub(row_w / 2);
                let ly = cy + 46;
                let urls: Vec<String> = state.bookmarks.iter().take(6).map(|b| b.url.clone()).collect();
                for url in urls {
                    if x >= lx && x < lx + QUICK_LINK_W && y >= ly && y < ly + QUICK_LINK_H {
                        if let Some(url) = state.begin_navigate(&url) { engine.navigate(&url).ok(); }
                        return;
                    }
                    lx += QUICK_LINK_W + QUICK_LINK_GAP;
                }
            }
        }
        _ => {}
    }
    state.cancel_address_bar_edit();
}

// ── Top-level render ──────────────────────────────────────────────────────────

fn render_ui(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager, dirty: DirtyFlags) {
    let theme      = state.theme;
    let tw         = state.tab_width;
    let active_pos = state.active_pos;

    if dirty.content {
        if state.overlay != OverlayKind::None {
            draw_overlay(fb, state, font);
        } else {
            match state.active_tab().map(|t| &t.page_state) {
                Some(PageState::NewTab)   => {
                    if state.active_tab().map_or(false, |t| t.is_private) {
                        draw_private_new_tab(fb, state, font);
                    } else {
                        draw_new_tab(fb, state, font);
                    }
                }
                Some(PageState::Loading)  => draw_loading(fb, state, font),
                Some(PageState::Error(_)) => draw_error(fb, state, font),
                Some(PageState::Loaded)   => {
                    let (nodes, scroll_y) = state.active_tab()
                        .map(|t| (t.current_nodes(), t.scroll_y))
                        .unwrap_or((&[], 0));
                    draw_loaded(fb, state, font, nodes, scroll_y);
                }
                None => {}
            }
        }
    }

    if dirty.tabs {
        fb.fill_rect(0, 0, fb.width, TAB_BAR_HEIGHT, theme.tab_bar_bg);
        draw_tab_row(fb, state, font);
        fb.fill_rect(0, TAB_BAR_HEIGHT - 1, fb.width, 1, theme.border);
        let active_x = TAB_START_X + active_pos as u32 * (tw + TAB_SEP);
        fb.fill_rect(active_x, TAB_BAR_HEIGHT - 1, tw, 2, theme.surface);
    }

    if dirty.chrome {
        fb.fill_rect(0, TAB_BAR_HEIGHT, fb.width, CHROME_BAR_HEIGHT, theme.surface);
        draw_chrome_row(fb, state, font);
        fb.fill_rect(0, TOP_BAR_HEIGHT, fb.width, 1, theme.border);
    }
}

// ── Tab row ───────────────────────────────────────────────────────────────────

fn draw_tab_row(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let tw    = state.tab_width;
    const TOP: u32 = 4;
    const H:   u32 = TAB_BAR_HEIGHT - TOP;

    for (i, tab) in state.tabs.iter().enumerate() {
        let tx         = TAB_START_X + i as u32 * (tw + TAB_SEP);
        let is_active  = tab.id == state.active_tab_id;
        let is_hovered = state.mouse_y < TAB_BAR_HEIGHT
            && state.mouse_x >= tx && state.mouse_x < tx + tw;

        let bg = if is_active       { theme.tab_active_bg }
                 else if is_hovered { theme.tab_hover_bg  }
                 else               { theme.tab_bg        };
        let fg = if is_active { theme.tab_active_fg } else { theme.tab_fg };

        draw::draw_rounded_rect_top(fb, tx, TOP, tw, H, 6, bg);

        if is_active {
            fb.fill_rect(tx, TAB_BAR_HEIGHT - 2, tw, 3, theme.surface);
            let stripe = if tab.is_private { PRIVATE_ACCENT } else { theme.accent };
            fb.fill_rect(tx, TOP + 4, 2, H - 8, stripe);
        }

        // Private tab: small badge dot before title
        let title_x = if tab.is_private && (is_active || is_hovered) {
            draw::draw_circle_filled(fb, tx + 10, TOP + H / 2, 4, PRIVATE_ACCENT);
            tx + 18
        } else {
            tx + 14
        };

        let close_reserve = if is_active || is_hovered { 24 } else { 8 };
        let max_title_w   = tw.saturating_sub(title_x - tx + close_reserve);
        let title_y       = TOP + (H / 2).saturating_sub(7);
        draw::draw_text(fb, font, title_x, title_y, tab.tab_title(), 13.0, fg, max_title_w);

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

        if tab.page_state.is_loading() {
            let anim = (state.frame_count * 4 % tw as u64) as u32;
            fb.fill_rect(tx, TAB_BAR_HEIGHT - 3, anim, 2, theme.accent);
        }
        if tab.page_state.is_error() {
            let dot_x = tx + tw.saturating_sub(28);
            fb.fill_rect(dot_x, TOP + H / 2 - 3, 6, 6, theme.security_err);
        }
        if tab.is_pinned {
            fb.fill_rect(tx + 5, TOP + 6, 4, 4, theme.accent);
        }
    }

    let add_x  = TAB_START_X + state.tabs.len() as u32 * (tw + TAB_SEP);
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
    let cy          = TAB_BAR_HEIGHT + CHROME_BAR_HEIGHT / 2;
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
    let color = if pressed       { theme.accent_fg   }
                else if !enabled { theme.fg_secondary }
                else             { theme.icon_fg      };
    match btn {
        NavBtn::Back    => draw::draw_icon_back(fb, cx, cy, 10, color),
        NavBtn::Forward => draw::draw_icon_forward(fb, cx, cy, 10, color),
        NavBtn::Reload  => draw::draw_icon_reload(fb, cx, cy, 7, color),
    }
}

// ── Address bar ───────────────────────────────────────────────────────────────

fn draw_address_bar(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme  = state.theme;
    let bar_x  = (FB_WIDTH - ADDR_BAR_W) / 2;
    let bar_y  = TAB_BAR_HEIGHT + (CHROME_BAR_HEIGHT - ADDR_BAR_H) / 2;
    let is_prv = state.active_tab().map_or(false, |t| t.is_private);

    let bg     = if state.address_bar_focused { theme.address_bar_bg_focused } else { theme.address_bar_bg };
    let border = if state.address_bar_focused { theme.address_bar_border_focused }
                 else if is_prv { PRIVATE_ACCENT }
                 else { theme.address_bar_border };
    draw::draw_rounded_rect(fb, bar_x.saturating_sub(1), bar_y.saturating_sub(1),
        ADDR_BAR_W + 2, ADDR_BAR_H + 2, ADDR_BAR_R + 1, border);
    draw::draw_rounded_rect(fb, bar_x, bar_y, ADDR_BAR_W, ADDR_BAR_H, ADDR_BAR_R, bg);

    let icon_x = bar_x + 14;
    let icon_y = bar_y + ADDR_BAR_H / 2;
    if let Some(tab) = state.active_tab() {
        match &tab.page_state {
            PageState::Loading  => draw::draw_icon_spinner(fb, icon_x, icon_y, 5, state.frame_count, theme.icon_fg),
            PageState::Error(_) => draw::draw_circle_filled(fb, icon_x, icon_y, 5, theme.security_err),
            _ if tab.url.starts_with("https://") => draw::draw_icon_lock(fb, icon_x, icon_y,
                if is_prv { PRIVATE_ACCENT } else { theme.security_ok }),
            _ if !tab.url.is_empty() => draw::draw_icon_globe(fb, icon_x, icon_y, theme.icon_fg),
            _ => {}
        }
    }

    let tx    = bar_x + 34;
    let ty    = bar_y + (ADDR_BAR_H.saturating_sub(14)) / 2;
    let max_w = ADDR_BAR_W.saturating_sub(34 + 30);

    if state.address_bar_input.is_empty() && !state.address_bar_focused {
        let placeholder = if is_prv { "Private search or URL" } else { "Search or enter URL" };
        draw::draw_text(fb, font, tx, ty, placeholder, 14.0, theme.placeholder, max_w);
    } else {
        draw::draw_text(fb, font, tx, ty, &state.address_bar_input, 14.0, theme.address_bar_fg, max_w);
        if state.address_bar_focused && (state.frame_count / 28) % 2 == 0 {
            let cw = font.text_width(&state.address_bar_input, 14.0);
            let cx = (tx + cw + 1).min(bar_x + ADDR_BAR_W - 34);
            fb.fill_rect(cx, ty, 2, 15, theme.accent);
        }
    }

    if let Some(tab) = state.active_tab() {
        let star_x   = bar_x + ADDR_BAR_W - 18;
        let star_col = if tab.is_bookmarked { theme.accent } else { theme.icon_fg };
        draw::draw_icon_star(fb, star_x, icon_y, 11, star_col, tab.is_bookmarked);
    }
}

// ── Overlay panel (history / bookmarks) ──────────────────────────────────────

fn draw_overlay(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);

    // ── Header ────────────────────────────────────────────────────────────────
    let header_y = TOP_BAR_HEIGHT + 24;
    let (title_str, items_len): (&str, usize) = match state.overlay {
        OverlayKind::History   => ("History",   state.global_history.len()),
        OverlayKind::Bookmarks => ("Bookmarks", state.bookmarks.len()),
        OverlayKind::None      => unreachable!(),
    };

    draw::draw_text(fb, font, OVERLAY_INDENT, header_y, title_str, 22.0, theme.fg, 400);

    if items_len > 0 {
        let count_str = format!("{items_len} item{}", if items_len == 1 { "" } else { "s" });
        let cw = font.text_width(&count_str, 13.0);
        draw::draw_text(fb, font, OVERLAY_INDENT + 200, header_y + 5, &count_str, 13.0, theme.fg_secondary, 200);
        let _ = cw;
    }

    let hint = "Esc close  \u{2022}  Enter open  \u{2022}  Up/Dn scroll";
    let hw   = font.text_width(hint, 11.0);
    draw::draw_text(fb, font,
        FB_WIDTH.saturating_sub(OVERLAY_INDENT + hw), header_y + 6,
        hint, 11.0, theme.fg_secondary, 600);

    // Separator
    fb.fill_rect(OVERLAY_INDENT, header_y + 34, FB_WIDTH - OVERLAY_INDENT * 2, 1, theme.border);

    // ── Empty state ───────────────────────────────────────────────────────────
    if items_len == 0 {
        let msg = match state.overlay {
            OverlayKind::History   => "No history yet — browse some pages",
            OverlayKind::Bookmarks => "No bookmarks yet — click the star to save a page",
            OverlayKind::None      => unreachable!(),
        };
        let mw = font.text_width(msg, 15.0);
        let cy = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
        draw::draw_text(fb, font, FB_WIDTH / 2 - mw / 2, cy, msg, 15.0, theme.fg_secondary, 800);
        return;
    }

    // ── Items ─────────────────────────────────────────────────────────────────
    let content_w = FB_WIDTH.saturating_sub(OVERLAY_INDENT * 2);

    for local_i in 0..OVERLAY_VISIBLE {
        let abs_i = state.overlay_scroll + local_i;
        let item_opt: Option<(&str, &str)> = match state.overlay {
            OverlayKind::History   => state.global_history.iter().rev().nth(abs_i)
                .map(|e| (e.title.as_str(), e.url.as_str())),
            OverlayKind::Bookmarks => state.bookmarks.get(abs_i)
                .map(|b| (b.title.as_str(), b.url.as_str())),
            OverlayKind::None => unreachable!(),
        };
        let (item_title, item_url) = match item_opt {
            Some(v) => v,
            None    => break,
        };

        let iy      = OVERLAY_LIST_TOP + local_i as u32 * OVERLAY_ITEM_H;
        let is_hot  = state.overlay_hover == Some(abs_i);

        if is_hot {
            fb.fill_rect(
                OVERLAY_INDENT.saturating_sub(12), iy,
                content_w + 24, OVERLAY_ITEM_H.saturating_sub(2),
                theme.surface,
            );
            // Left accent bar on hovered item
            fb.fill_rect(OVERLAY_INDENT.saturating_sub(12), iy, 3, OVERLAY_ITEM_H - 2, theme.accent);
        }

        let title_col = if is_hot { theme.fg } else { theme.fg };
        draw::draw_text(fb, font, OVERLAY_INDENT, iy + 10,
            item_title, 14.0, title_col, content_w.saturating_sub(200));
        draw::draw_text(fb, font, OVERLAY_INDENT, iy + 32,
            item_url, 11.0, if is_hot { theme.accent } else { theme.fg_secondary },
            content_w);

        fb.fill_rect(OVERLAY_INDENT, iy + OVERLAY_ITEM_H - 1, content_w, 1, theme.border);
    }

    // Scroll indicator
    if items_len > OVERLAY_VISIBLE {
        let visible_h = OVERLAY_VISIBLE as u32 * OVERLAY_ITEM_H;
        let track_h   = visible_h;
        let thumb_h   = ((track_h as u64 * OVERLAY_VISIBLE as u64) / items_len as u64)
            .max(24).min(track_h as u64) as u32;
        let max_off   = items_len.saturating_sub(OVERLAY_VISIBLE);
        let thumb_y   = OVERLAY_LIST_TOP
            + if max_off > 0 {
                (state.overlay_scroll as u64 * (track_h - thumb_h) as u64 / max_off as u64) as u32
            } else { 0 };
        let sx = FB_WIDTH - OVERLAY_INDENT + 16;
        fb.fill_rect(sx, OVERLAY_LIST_TOP, 4, track_h, theme.surface);
        fb.fill_rect(sx, thumb_y, 4, thumb_h, theme.fg_secondary);
    }
}

// ── New Tab page ──────────────────────────────────────────────────────────────

fn draw_new_tab(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme     = state.theme;
    let cx        = FB_WIDTH / 2;
    let content_h = FB_HEIGHT - TOP_BAR_HEIGHT;
    let cy        = TOP_BAR_HEIGHT + content_h / 2;

    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, content_h, theme.bg);

    let brand = "rashamon arc";
    let bw    = font.text_width(brand, 32.0);
    draw::draw_text(fb, font, cx.saturating_sub(bw / 2), cy.saturating_sub(200), brand, 32.0, theme.fg, 600);

    let tagline = "your private arc of the web";
    let tgw     = font.text_width(tagline, 15.0);
    draw::draw_text(fb, font, cx.saturating_sub(tgw / 2), cy.saturating_sub(156),
        tagline, 15.0, theme.fg_secondary, 600);

    let sw: u32 = 600; let sh: u32 = 48; let sr: u32 = 24;
    let sx = cx.saturating_sub(sw / 2);
    let sy = cy.saturating_sub(90);
    let border = if state.address_bar_focused { theme.address_bar_border_focused } else { theme.address_bar_border };
    draw::draw_rounded_rect(fb, sx.saturating_sub(1), sy.saturating_sub(1), sw + 2, sh + 2, sr + 1, border);
    draw::draw_rounded_rect(fb, sx, sy, sw, sh, sr, theme.address_bar_bg);

    if state.address_bar_input.is_empty() {
        let hint = "Search or enter URL";
        let hw   = font.text_width(hint, 15.0);
        draw::draw_text(fb, font, sx + (sw - hw) / 2, sy + (sh.saturating_sub(14)) / 2,
            hint, 15.0, theme.placeholder, sw - 40);
    } else {
        draw::draw_text(fb, font, sx + 24, sy + (sh.saturating_sub(14)) / 2,
            &state.address_bar_input, 15.0, theme.address_bar_fg, sw - 48);
        if state.address_bar_focused && (state.frame_count / 28) % 2 == 0 {
            let cw    = font.text_width(&state.address_bar_input, 15.0);
            let cur_x = (sx + 24 + cw + 1).min(sx + sw - 24);
            fb.fill_rect(cur_x, sy + (sh.saturating_sub(16)) / 2, 2, 16, theme.accent);
        }
    }

    let hints = "Ctrl+T  new  \u{2022}  Ctrl+I  private  \u{2022}  Ctrl+H  history  \u{2022}  Ctrl+B  bookmarks  \u{2022}  Ctrl+P  theme";
    let hw    = font.text_width(hints, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(hw / 2), sy + sh + 14,
        hints, 11.0, theme.fg_secondary, 1000);

    draw_quick_links(fb, state, font, cx, cy);
}

fn draw_private_new_tab(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme     = state.theme;
    let cx        = FB_WIDTH / 2;
    let content_h = FB_HEIGHT - TOP_BAR_HEIGHT;
    let cy        = TOP_BAR_HEIGHT + content_h / 2;

    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, content_h, theme.bg);

    // Private mode header with coloured accent
    draw::draw_circle_filled(fb, cx, cy.saturating_sub(180), 28, PRIVATE_ACCENT);
    // Draw a simple "P" inside the badge
    let pw = font.text_width("P", 20.0);
    draw::draw_text(fb, font, cx.saturating_sub(pw / 2), cy.saturating_sub(192),
        "P", 20.0, Pixel::WHITE, 30);

    let brand = "private browsing";
    let bw    = font.text_width(brand, 28.0);
    draw::draw_text(fb, font, cx.saturating_sub(bw / 2), cy.saturating_sub(136),
        brand, 28.0, PRIVATE_ACCENT, 600);

    let note = "Pages you visit here won't appear in history.";
    let nw   = font.text_width(note, 13.0);
    draw::draw_text(fb, font, cx.saturating_sub(nw / 2), cy.saturating_sub(96),
        note, 13.0, theme.fg_secondary, 700);

    let sw: u32 = 600; let sh: u32 = 48; let sr: u32 = 24;
    let sx = cx.saturating_sub(sw / 2);
    let sy = cy.saturating_sub(48);
    draw::draw_rounded_rect(fb, sx.saturating_sub(1), sy.saturating_sub(1),
        sw + 2, sh + 2, sr + 1, PRIVATE_ACCENT);
    draw::draw_rounded_rect(fb, sx, sy, sw, sh, sr, theme.address_bar_bg);

    if state.address_bar_input.is_empty() {
        let hint = "Private search or enter URL";
        let hw   = font.text_width(hint, 15.0);
        draw::draw_text(fb, font, sx + (sw - hw) / 2, sy + (sh.saturating_sub(14)) / 2,
            hint, 15.0, theme.placeholder, sw - 40);
    } else {
        draw::draw_text(fb, font, sx + 24, sy + (sh.saturating_sub(14)) / 2,
            &state.address_bar_input, 15.0, theme.address_bar_fg, sw - 48);
        if state.address_bar_focused && (state.frame_count / 28) % 2 == 0 {
            let cw    = font.text_width(&state.address_bar_input, 15.0);
            let cur_x = (sx + 24 + cw + 1).min(sx + sw - 24);
            fb.fill_rect(cur_x, sy + (sh.saturating_sub(16)) / 2, 2, 16, PRIVATE_ACCENT);
        }
    }

    let hints = "Ctrl+T  new tab  \u{2022}  Ctrl+W  close  \u{2022}  Ctrl+H  history  \u{2022}  Esc  exit";
    let hw    = font.text_width(hints, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(hw / 2), sy + sh + 14,
        hints, 11.0, theme.fg_secondary, 900);
}

const FAVICON_COLORS: [Pixel; 8] = [
    Pixel { r: 79,  g: 140, b: 255 }, Pixel { r: 52,  g: 168, b: 83  },
    Pixel { r: 255, g: 152, b: 0   }, Pixel { r: 233, g: 30,  b: 99  },
    Pixel { r: 156, g: 39,  b: 176 }, Pixel { r: 0,   g: 188, b: 212 },
    Pixel { r: 121, g: 85,  b: 72  }, Pixel { r: 96,  g: 125, b: 139 },
];

fn draw_quick_links(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager, cx: u32, cy: u32) {
    let theme = state.theme;
    let num   = state.bookmarks.len().min(6) as u32;
    if num == 0 { return; }

    let row_w      = num * QUICK_LINK_W + (num - 1) * QUICK_LINK_GAP;
    let mut card_x = cx.saturating_sub(row_w / 2);
    let card_y     = cy + 46;

    let lbl = "Quick access";
    let lw  = font.text_width(lbl, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(lw / 2), card_y.saturating_sub(20),
        lbl, 11.0, theme.fg_secondary, 200);

    for (i, bm) in state.bookmarks.iter().take(6).enumerate() {
        let fav_col = FAVICON_COLORS[i % FAVICON_COLORS.len()];
        let hovered = state.mouse_y >= card_y && state.mouse_y < card_y + QUICK_LINK_H
            && state.mouse_x >= card_x && state.mouse_x < card_x + QUICK_LINK_W
            && state.mouse_y >= TOP_BAR_HEIGHT;

        let card_bg = if hovered { theme.new_tab_card_hover_bg } else { theme.new_tab_card_bg };
        draw::draw_rounded_rect(fb, card_x, card_y, QUICK_LINK_W, QUICK_LINK_H, 10, card_bg);
        if hovered {
            draw::draw_rounded_rect_outline(fb, card_x as i32, card_y as i32,
                QUICK_LINK_W as i32, QUICK_LINK_H as i32, 10, theme.accent);
        }
        let fav_cx = card_x + QUICK_LINK_W / 2;
        let fav_cy = card_y + 32;
        draw::draw_circle_filled(fb, fav_cx, fav_cy, 20, fav_col);
        let mut ch_buf = [0u8; 4];
        let ch_str     = bm.first_upper.encode_utf8(&mut ch_buf);
        let lw         = font.text_width(ch_str, 16.0);
        draw::draw_text(fb, font, fav_cx.saturating_sub(lw / 2), fav_cy.saturating_sub(8),
            ch_str, 16.0, Pixel::WHITE, 24);
        let title_y = card_y + QUICK_LINK_H - 28;
        let max_tw  = QUICK_LINK_W.saturating_sub(12);
        let title_w = font.text_width(&bm.title, 12.0).min(max_tw);
        let title_x = card_x + (QUICK_LINK_W - title_w) / 2;
        draw::draw_text(fb, font, title_x, title_y, &bm.title, 12.0, theme.fg, max_tw);
        card_x += QUICK_LINK_W + QUICK_LINK_GAP;
    }
}

// ── Loading overlay ───────────────────────────────────────────────────────────

fn draw_loading(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx    = FB_WIDTH / 2;
    let cy    = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);
    draw::draw_icon_spinner(fb, cx, cy.saturating_sub(20), 14, state.frame_count, theme.fg_secondary);
    const LOADING_MSGS: [&str; 4] = ["Loading...", "Loading.", "Loading..", "Loading..."];
    let msg = LOADING_MSGS[((state.frame_count / 18) % 4) as usize];
    let mw  = font.text_width(msg, 14.0);
    draw::draw_text(fb, font, cx.saturating_sub(mw / 2), cy + 8, msg, 14.0, theme.fg_secondary, 200);
    if let Some(tab) = state.active_tab() {
        if !tab.url.is_empty() {
            let host = derive_title(&tab.url);
            let hw   = font.text_width(host, 12.0);
            draw::draw_text(fb, font, cx.saturating_sub(hw / 2), cy + 30, host, 12.0, theme.placeholder, 600);
        }
    }
    let elapsed  = state.frame_count.saturating_sub(state.active_tab().map_or(0, |t| t.load_start_frame));
    let progress = ((elapsed as f32 / LOAD_MIN_FRAMES as f32) * FB_WIDTH as f32) as u32;
    fb.fill_rect(0, TOP_BAR_HEIGHT + 1, progress.min(FB_WIDTH - 4), 2, theme.accent);
}

// ── Loaded page ───────────────────────────────────────────────────────────────

fn draw_loaded(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager,
               nodes: &[PageNode], scroll_y: u32)
{
    if !nodes.is_empty() {
        draw_page_content(fb, state, font, nodes, scroll_y);
    } else {
        draw_loaded_card(fb, state, font);
    }
}

fn draw_page_content(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager,
                     nodes: &[PageNode], scroll_y: u32)
{
    let theme   = state.theme;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, 1, theme.border);

    let vp_top = TOP_BAR_HEIGHT as i32;
    let vp_bot = FB_HEIGHT as i32;
    let mut y: i32 = vp_top + PAD_TOP as i32 - scroll_y as i32;

    'outer: for node in nodes {
        if y > vp_bot + 200 { break; }
        match node {
            PageNode::Heading { level, text } => {
                let (size, color, before, after): (f32, _, i32, i32) = match level {
                    1 => (28.0, theme.fg, 18, 10),
                    2 => (22.0, theme.fg, 14, 8),
                    _ => (17.0, theme.fg, 10, 6),
                };
                y += before;
                for line in wrap_text(text, font, size, MAX_W) {
                    let lh = size as i32 + 4;
                    if y + lh > vp_top && y < vp_bot {
                        draw::draw_text(fb, font, MARGIN, y as u32, &line, size, color, MAX_W);
                    }
                    y += lh;
                    if y > vp_bot + 200 { break 'outer; }
                }
                y += after;
            }
            PageNode::Paragraph(text) => {
                if text.trim().is_empty() { continue; }
                y += 4;
                for line in wrap_text(text, font, 14.0, MAX_W) {
                    if y + 22 > vp_top && y < vp_bot {
                        draw::draw_text(fb, font, MARGIN, y as u32, &line, 14.0, theme.fg_secondary, MAX_W);
                    }
                    y += 22;
                    if y > vp_bot + 200 { break 'outer; }
                }
                y += 10;
            }
            PageNode::ListItem(text) => {
                let b = format!("  \u{2022}  {text}");
                for line in wrap_text(&b, font, 13.0, MAX_W) {
                    if y + 20 > vp_top && y < vp_bot {
                        draw::draw_text(fb, font, MARGIN, y as u32, &line, 13.0, theme.fg_secondary, MAX_W);
                    }
                    y += 20;
                    if y > vp_bot + 200 { break 'outer; }
                }
                y += 3;
            }
            PageNode::Pre(text) => {
                y += 8;
                let lines: Vec<&str> = text.lines().collect();
                let block_h = lines.len() as i32 * 18 + 16;
                if y < vp_bot && y + block_h > vp_top {
                    let fill_y = y.max(vp_top) as u32;
                    let fill_h = ((y + block_h).min(vp_bot) - fill_y as i32).max(0) as u32;
                    fb.fill_rect(MARGIN.saturating_sub(8), fill_y, MAX_W + 16, fill_h, theme.surface);
                }
                for line in &lines {
                    if y + 18 > vp_top && y < vp_bot {
                        draw::draw_text(fb, font, MARGIN, y as u32, line, 12.0, theme.fg, MAX_W + 8);
                    }
                    y += 18;
                    if y > vp_bot + 200 { break 'outer; }
                }
                y += 14;
            }
            PageNode::HRule => {
                y += 8;
                if y >= vp_top && y < vp_bot {
                    fb.fill_rect(MARGIN, y as u32, MAX_W, 1, theme.border);
                }
                y += 16;
            }
        }
    }

    // Scroll thumb
    let tab = match state.active_tab() { Some(t) => t, None => return };
    if tab.content_height > (FB_HEIGHT - TOP_BAR_HEIGHT) {
        let track_h = (FB_HEIGHT - TOP_BAR_HEIGHT) as u32;
        let thumb_h = ((track_h as u64 * track_h as u64)
                       / tab.content_height as u64).max(24).min(track_h as u64) as u32;
        let max_off = tab.content_height.saturating_sub(track_h);
        let thumb_y = if max_off > 0 {
            TOP_BAR_HEIGHT + (scroll_y as u64 * (track_h - thumb_h) as u64 / max_off as u64) as u32
        } else { TOP_BAR_HEIGHT };
        fb.fill_rect(FB_WIDTH - 4, TOP_BAR_HEIGHT, 4, track_h, theme.surface);
        fb.fill_rect(FB_WIDTH - 4, thumb_y, 4, thumb_h, theme.fg_secondary);
    }
}

fn wrap_text(text: &str, font: &FontManager, size: f32, max_w: u32) -> Vec<String> {
    let space_w = font.text_width(" ", size);
    let mut lines   = Vec::new();
    let mut line    = String::new();
    let mut line_w  = 0u32;
    for word in text.split_whitespace() {
        let ww  = font.text_width(word, size);
        let gap = if line.is_empty() { 0 } else { space_w };
        if !line.is_empty() && line_w + gap + ww > max_w {
            lines.push(std::mem::take(&mut line));
            line_w = 0;
        }
        if !line.is_empty() { line.push(' '); }
        line.push_str(word);
        line_w += gap + ww;
    }
    if !line.is_empty() { lines.push(line); }
    lines
}

fn draw_loaded_card(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx    = FB_WIDTH / 2;
    let cy    = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);
    let Some(tab) = state.active_tab() else { return };
    const CARD_W: u32 = 680; const CARD_H: u32 = 220;
    let card_x = cx.saturating_sub(CARD_W / 2);
    let card_y = cy.saturating_sub(CARD_H / 2);
    draw::draw_rounded_rect(fb, card_x, card_y, CARD_W, CARD_H, 14, theme.surface);
    draw::draw_rounded_rect_outline(fb, card_x as i32, card_y as i32, CARD_W as i32, CARD_H as i32, 14, theme.border);
    let title_w = font.text_width(tab.tab_title(), 22.0).min(CARD_W - 48);
    draw::draw_text(fb, font, cx.saturating_sub(title_w / 2), card_y + 40, tab.tab_title(), 22.0, theme.fg, CARD_W - 48);
    let host   = derive_title(&tab.url);
    let host_w = font.text_width(host, 14.0).min(CARD_W - 48);
    draw::draw_text(fb, font, cx.saturating_sub(host_w / 2), card_y + 74, host, 14.0, theme.accent, CARD_W - 48);
    if !tab.url.is_empty() {
        let uw = font.text_width(&tab.url, 11.0).min(CARD_W - 48);
        draw::draw_text(fb, font, cx.saturating_sub(uw / 2), card_y + 104, &tab.url, 11.0, theme.fg_secondary, CARD_W - 48);
    }
    let hint = "Ctrl+R to reload  \u{2022}  address bar to navigate";
    let hw   = font.text_width(hint, 11.0);
    draw::draw_text(fb, font, cx.saturating_sub(hw / 2), card_y + CARD_H - 28, hint, 11.0, theme.fg_secondary, CARD_W - 48);
}

// ── Error page ────────────────────────────────────────────────────────────────

fn draw_error(fb: &mut Framebuffer, state: &BrowserState, font: &FontManager) {
    let theme = state.theme;
    let cx    = FB_WIDTH / 2;
    let cy    = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
    fb.fill_rect(0, TOP_BAR_HEIGHT, FB_WIDTH, FB_HEIGHT - TOP_BAR_HEIGHT, theme.bg);
    let icon_cy = cy.saturating_sub(72);
    draw::draw_circle_filled(fb, cx, icon_cy, 30, theme.security_err);
    draw::draw_icon_close(fb, cx, icon_cy, 16, Pixel::WHITE);
    let title = "Page couldn't be loaded";
    let tw    = font.text_width(title, 20.0);
    draw::draw_text(fb, font, cx.saturating_sub(tw / 2), cy.saturating_sub(18), title, 20.0, theme.fg, 700);
    let msg = state.active_tab().and_then(|t| t.page_state.error_msg()).unwrap_or("The page is unavailable");
    let mw  = font.text_width(msg, 14.0);
    draw::draw_text(fb, font, cx.saturating_sub(mw / 2), cy + 14, msg, 14.0, theme.fg_secondary, 700);
    if let Some(tab) = state.active_tab() {
        if !tab.url.is_empty() {
            let uw = font.text_width(&tab.url, 12.0).min(800);
            draw::draw_text(fb, font, cx.saturating_sub(uw / 2), cy + 40, &tab.url, 12.0, theme.placeholder, 800);
        }
    }
    let (bx, by) = layout::retry_btn_pos();
    let hovered  = state.mouse_x >= bx && state.mouse_x < bx + RETRY_BTN_W
        && state.mouse_y >= by && state.mouse_y < by + RETRY_BTN_H;
    let btn_bg  = if hovered { theme.accent  } else { theme.surface };
    let btn_brd = if hovered { theme.accent  } else { theme.border  };
    draw::draw_rounded_rect(fb, bx.saturating_sub(1), by.saturating_sub(1), RETRY_BTN_W + 2, RETRY_BTN_H + 2, 9, btn_brd);
    draw::draw_rounded_rect(fb, bx, by, RETRY_BTN_W, RETRY_BTN_H, 8, btn_bg);
    let lbl    = "Try again";
    let lw     = font.text_width(lbl, 14.0);
    let lbl_fg = if hovered { theme.accent_fg } else { theme.fg };
    draw::draw_text(fb, font, bx + (RETRY_BTN_W.saturating_sub(lw)) / 2,
        by + (RETRY_BTN_H.saturating_sub(14)) / 2, lbl, 14.0, lbl_fg, RETRY_BTN_W - 8);
}
