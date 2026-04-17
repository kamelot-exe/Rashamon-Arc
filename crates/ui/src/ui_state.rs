//! Core browser state model for Rashamon Arc.

use crate::layout::{self, *};
use crate::theme::{get_theme, ColorPalette, Theme};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

// ── TabId ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(usize);

impl TabId {
    fn next() -> Self { Self(NEXT_ID.fetch_add(1, Ordering::Relaxed)) }
}

// ── PageState ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum PageState {
    NewTab,
    Loading,
    Loaded,
    Error(String),
}

impl PageState {
    pub fn is_loading(&self) -> bool { matches!(self, Self::Loading) }
    pub fn is_new_tab(&self) -> bool { matches!(self, Self::NewTab) }
    pub fn is_error(&self)   -> bool { matches!(self, Self::Error(_)) }

    pub fn error_msg(&self) -> Option<&str> {
        if let Self::Error(m) = self { Some(m) } else { None }
    }
}

// ── NavResult ─────────────────────────────────────────────────────────────────

/// Outcome classified at navigation-submit time, before any loading begins.
///
/// `WillLoad`     — URL is structurally valid; tab enters Loading then Loaded.
/// `WillFail(why)` — URL is unsupported/malformed; tab goes straight to Error.
///
/// Storing the result on the tab means `commit_navigation` reads a predetermined
/// verdict rather than racing a timer to guess whether the page is reachable.
#[derive(Debug, Clone, PartialEq)]
pub enum NavResult {
    WillLoad,
    WillFail(String),
}

/// Classify a fully-resolved URL at submit time.
/// Called by `begin_navigate` and `reload` before any state change.
fn classify_url(url: &str) -> NavResult {
    if url.is_empty() {
        return NavResult::WillFail("No address entered".into());
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return NavResult::WillLoad;
    }
    if url.starts_with("file://") {
        return NavResult::WillFail("Local file access is not yet supported".into());
    }
    if url.starts_with("chrome://") || url.starts_with("about:") || url.starts_with("data:") {
        return NavResult::WillFail("Browser-internal addresses are not supported".into());
    }
    NavResult::WillFail("Unsupported address format".into())
}

// ── DirtyFlags ────────────────────────────────────────────────────────────────

/// Per-region repaint flags.
/// Only dirty regions are cleared and redrawn each frame — avoiding full-screen
/// repaints for common micro-events like cursor blink, hover, and typing.
#[derive(Debug, Default, Clone, Copy)]
pub struct DirtyFlags {
    pub tabs:    bool,   // y = 0 .. TAB_BAR_HEIGHT
    pub chrome:  bool,   // y = TAB_BAR_HEIGHT .. TOP_BAR_HEIGHT
    pub content: bool,   // y = TOP_BAR_HEIGHT .. FB_HEIGHT
}

impl DirtyFlags {
    #[inline] pub fn any(self)    -> bool { self.tabs || self.chrome || self.content }
    #[inline] pub fn all(&mut self) { self.tabs = true; self.chrome = true; self.content = true; }
    #[inline] pub fn clear(&mut self) { *self = Self::default(); }
}

// ── HoveredRegion ─────────────────────────────────────────────────────────────

/// Interactive UI region the cursor is currently over.
/// Each variant maps to one DirtyFlags field so hover changes only repaint the
/// affected strip, not the whole window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoveredRegion {
    None,
    Tab(usize),
    TabClose(usize),
    NewTabBtn,
    NavBack,
    NavForward,
    NavReload,
    AddressBar,
    BookmarkStar,
    QuickLink(usize),
    RetryBtn,
    ContentArea, // non-interactive content — no visual hover change
}

// ── NavigationEntry ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NavigationEntry {
    pub url:         String,
    pub display_url: String,
    pub title:       String,
    /// `None` → page committed successfully.
    /// `Some(reason)` → navigation failed; restoring this entry shows an Error page.
    pub error_msg:   Option<String>,
}

impl NavigationEntry {
    fn new(url: &str, title: &str, error_msg: Option<String>) -> Self {
        Self {
            url:         url.to_string(),
            display_url: url.to_string(),
            title:       title.to_string(),
            error_msg,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Borrows a short display title from a URL — zero allocation.
pub fn derive_title(url: &str) -> &str {
    if url.is_empty() { return "New Tab"; }
    url.trim_start_matches("https://")
       .trim_start_matches("http://")
       .trim_start_matches("www.")
       .split('/')
       .next()
       .unwrap_or(url)
}

// ── TabState ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TabState {
    pub id:                 TabId,
    pub title:              String,
    pub url:                String,
    pub display_url:        String,
    pub page_state:         PageState,
    pub is_pinned:          bool,
    pub is_bookmarked:      bool,
    pub history:            Vec<NavigationEntry>,
    pub history_index:      usize,
    pub last_committed_url: String,
    pub load_start_frame:   u64,
    /// Pre-classified navigation outcome — set at submit time, read at commit time.
    pub nav_result:         NavResult,
}

impl TabState {
    pub fn new_tab() -> Self {
        Self {
            id:                 TabId::next(),
            title:              "New Tab".to_string(),
            url:                String::new(),
            display_url:        String::new(),
            page_state:         PageState::NewTab,
            is_pinned:          false,
            is_bookmarked:      false,
            history:            Vec::new(),
            history_index:      0,
            last_committed_url: String::new(),
            load_start_frame:   0,
            nav_result:         NavResult::WillLoad,
        }
    }

    pub fn can_go_back(&self)    -> bool { self.history_index > 0 }
    pub fn can_go_forward(&self) -> bool { self.history_index + 1 < self.history.len() }

    fn commit(&mut self, url: &str, title: &str, error_msg: Option<String>) {
        if self.history_index + 1 < self.history.len() {
            self.history.truncate(self.history_index + 1);
        }
        // Reload: the URL matches the current history entry — update in-place
        // rather than pushing a duplicate entry that would break back/forward.
        if error_msg.is_none() {
            if let Some(cur) = self.history.get_mut(self.history_index) {
                if cur.url == url {
                    cur.title     = title.to_string();
                    cur.error_msg = None;
                    self.last_committed_url = url.to_string();
                    return;
                }
            }
        }
        self.history.push(NavigationEntry::new(url, title, error_msg));
        self.history_index      = self.history.len() - 1;
        self.last_committed_url = url.to_string();
    }

    pub fn tab_title(&self) -> &str {
        match &self.page_state {
            PageState::NewTab   => "New Tab",
            PageState::Loading  => if self.title.is_empty() { "Loading…" } else { &self.title },
            PageState::Loaded   => if self.title.is_empty() { &self.url }   else { &self.title },
            PageState::Error(_) => if self.title.is_empty() { "Error" }     else { &self.title },
        }
    }
}

// ── QuickLink ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct QuickLink {
    pub title:       String,
    pub url:         String,
    /// Pre-uppercased first char — avoids per-frame String allocation.
    pub first_upper: char,
}

impl QuickLink {
    pub fn new(title: impl Into<String>, url: impl Into<String>) -> Self {
        let title = title.into();
        let first_upper = title.chars()
            .next()
            .and_then(|c| c.to_uppercase().next())
            .unwrap_or('?');
        Self { title, url: url.into(), first_upper }
    }
}

// ── BrowserState ──────────────────────────────────────────────────────────────

pub struct BrowserState {
    pub tabs:          Vec<TabState>,
    pub active_tab_id: TabId,

    pub mouse_x:     u32,
    pub mouse_y:     u32,
    pub frame_count: u64,

    pub theme:   Theme,
    pub palette: ColorPalette,

    pub address_bar_focused: bool,
    pub address_bar_input:   String,

    pub bookmarks: Vec<QuickLink>,

    pub nav_btn_pressed:       u8,
    pub nav_btn_pressed_frame: u64,

    // ── Layout cache ──────────────────────────────────────────────────────────
    /// Cached tab strip width — recomputed only when tab count changes.
    pub tab_width:  u32,
    /// Active tab's index in `tabs` — avoids O(n) scan per frame.
    pub active_pos: usize,

    // ── Dirty-region rendering ────────────────────────────────────────────────
    /// Region-level repaint flags — set by state mutations, cleared after render.
    pub dirty:          DirtyFlags,
    /// Bumped in `cycle_theme` so the render cache knows to recompute layout.
    pub theme_version:  u64,
    /// Which interactive region the cursor is over — dirty only on region change.
    pub hovered_region: HoveredRegion,
}

impl BrowserState {
    pub fn new() -> Self {
        let palette   = ColorPalette::KamelotDark;
        let first_tab = TabState::new_tab();
        let first_id  = first_tab.id;
        let mut s = Self {
            tabs:          vec![first_tab],
            active_tab_id: first_id,
            mouse_x:       0,
            mouse_y:       0,
            frame_count:   0,
            palette,
            theme:         get_theme(palette),
            address_bar_focused: false,
            address_bar_input:   String::new(),
            bookmarks: vec![
                QuickLink::new("GitHub",      "https://github.com"),
                QuickLink::new("Hacker News", "https://news.ycombinator.com"),
                QuickLink::new("Rust Lang",   "https://www.rust-lang.org"),
                QuickLink::new("MDN",         "https://developer.mozilla.org"),
                QuickLink::new("Servo",       "https://servo.org"),
                QuickLink::new("Crates.io",   "https://crates.io"),
            ],
            nav_btn_pressed:       0,
            nav_btn_pressed_frame: 0,
            tab_width:      0,
            active_pos:     0,
            dirty:          DirtyFlags { tabs: true, chrome: true, content: true },
            theme_version:  0,
            hovered_region: HoveredRegion::None,
        };
        s.update_layout();
        s
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn update_layout(&mut self) {
        self.tab_width  = layout::tab_width(self.tabs.len());
        self.active_pos = self.tabs.iter()
            .position(|t| t.id == self.active_tab_id)
            .unwrap_or(0);
    }

    fn update_bookmark_flag(&mut self) {
        let url   = self.active_tab().map(|t| t.url.clone()).unwrap_or_default();
        let is_bm = !url.is_empty() && self.bookmarks.iter().any(|b| b.url == url);
        if let Some(tab) = self.active_tab_mut() {
            tab.is_bookmarked = is_bm;
        }
    }

    /// Dirty only the hover-affected region for a given interactive element.
    fn dirty_for_hover(region: HoveredRegion, d: &mut DirtyFlags) {
        match region {
            HoveredRegion::Tab(_) | HoveredRegion::TabClose(_) | HoveredRegion::NewTabBtn
                => d.tabs = true,
            HoveredRegion::NavBack | HoveredRegion::NavForward | HoveredRegion::NavReload
            | HoveredRegion::AddressBar | HoveredRegion::BookmarkStar
                => d.chrome = true,
            HoveredRegion::QuickLink(_) | HoveredRegion::RetryBtn
                => d.content = true,
            // Moving within content area (non-interactive) → no visual change.
            HoveredRegion::None | HoveredRegion::ContentArea => {}
        }
    }

    /// Dirty chrome and, if the active tab is a new-tab page, content as well.
    /// Used for address-bar events (typing, focus, blink) because the new-tab
    /// search box mirrors the address bar input.
    pub fn dirty_address_bar(&mut self) {
        self.dirty.chrome = true;
        if self.is_on_new_tab() {
            self.dirty.content = true;
        }
    }

    fn compute_hover_region(&self, x: u32, y: u32) -> HoveredRegion {
        let tw = self.tab_width;

        if y < TAB_BAR_HEIGHT {
            for (i, _) in self.tabs.iter().enumerate() {
                let lx = TAB_START_X + i as u32 * (tw + TAB_SEP);
                let rx = lx + tw;
                if x >= lx && x < rx {
                    return if x >= lx + tw.saturating_sub(18) {
                        HoveredRegion::TabClose(i)
                    } else {
                        HoveredRegion::Tab(i)
                    };
                }
            }
            let add_x = TAB_START_X + self.tabs.len() as u32 * (tw + TAB_SEP);
            if x >= add_x && x < add_x + TAB_NEW_BTN_W {
                return HoveredRegion::NewTabBtn;
            }
            return HoveredRegion::None;
        }

        if y < TOP_BAR_HEIGHT {
            let r: u32 = 16;
            if x >= 12 && x < 12 + r * 2 { return HoveredRegion::NavBack;    }
            if x >= 54 && x < 54 + r * 2 { return HoveredRegion::NavForward; }
            if x >= 96 && x < 96 + r * 2 { return HoveredRegion::NavReload;  }

            let bar_x = (FB_WIDTH - ADDR_BAR_W) / 2;
            let bar_y = TAB_BAR_HEIGHT + (CHROME_BAR_HEIGHT - ADDR_BAR_H) / 2;
            if x >= bar_x && x < bar_x + ADDR_BAR_W && y >= bar_y && y < bar_y + ADDR_BAR_H {
                return if x >= bar_x + ADDR_BAR_W - 26 {
                    HoveredRegion::BookmarkStar
                } else {
                    HoveredRegion::AddressBar
                };
            }
            return HoveredRegion::None;
        }

        // Content area
        match self.active_tab().map(|t| &t.page_state) {
            Some(PageState::Error(_)) => {
                let (bx, by) = retry_btn_pos();
                if x >= bx && x < bx + RETRY_BTN_W && y >= by && y < by + RETRY_BTN_H {
                    return HoveredRegion::RetryBtn;
                }
            }
            Some(PageState::NewTab) => {
                let num = self.bookmarks.len().min(6) as u32;
                if num > 0 {
                    let cx    = FB_WIDTH / 2;
                    let cy    = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
                    let row_w = num * QUICK_LINK_W + (num - 1) * QUICK_LINK_GAP;
                    let mut lx = cx.saturating_sub(row_w / 2);
                    let ly    = cy + 46;
                    for i in 0..num as usize {
                        if x >= lx && x < lx + QUICK_LINK_W && y >= ly && y < ly + QUICK_LINK_H {
                            return HoveredRegion::QuickLink(i);
                        }
                        lx += QUICK_LINK_W + QUICK_LINK_GAP;
                    }
                }
            }
            _ => {}
        }

        HoveredRegion::ContentArea
    }

    // ── Public accessors ──────────────────────────────────────────────────────

    pub fn active_tab(&self) -> Option<&TabState> {
        // Fast path: cached index is almost always correct.
        if let Some(t) = self.tabs.get(self.active_pos) {
            if t.id == self.active_tab_id { return Some(t); }
        }
        // Fallback: O(n) scan (only hits if active_pos is stale).
        self.tabs.iter().find(|t| t.id == self.active_tab_id)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut TabState> {
        // Fast path: cached index is almost always correct.
        let id = self.active_tab_id;
        if let Some(t) = self.tabs.get(self.active_pos) {
            if t.id == id {
                return self.tabs.get_mut(self.active_pos);
            }
        }
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn is_on_new_tab(&self) -> bool {
        self.active_tab().map_or(false, |t| t.page_state.is_new_tab())
    }

    // ── Mouse / hover ─────────────────────────────────────────────────────────

    /// Store raw cursor position and dirty only the region whose hover state changed.
    pub fn set_mouse_pos(&mut self, x: u32, y: u32) {
        self.mouse_x = x;
        self.mouse_y = y;
        let region = self.compute_hover_region(x, y);
        if region != self.hovered_region {
            Self::dirty_for_hover(region,              &mut self.dirty);
            Self::dirty_for_hover(self.hovered_region, &mut self.dirty);
            self.hovered_region = region;
        }
    }

    // ── Address bar input ─────────────────────────────────────────────────────

    pub fn type_char(&mut self, c: char) {
        self.address_bar_input.push(c);
        self.dirty_address_bar();
    }

    pub fn type_backspace(&mut self) {
        if self.address_bar_input.pop().is_some() {
            self.dirty_address_bar();
        }
    }

    // ── Tab lifecycle ─────────────────────────────────────────────────────────

    pub fn open_new_tab(&mut self) {
        let tab = TabState::new_tab();
        let id  = tab.id;
        self.tabs.push(tab);
        self.activate_tab(id);
    }

    pub fn close_tab(&mut self, id: TabId) {
        if self.tabs.len() == 1 {
            let fresh    = TabState::new_tab();
            let fresh_id = fresh.id;
            self.tabs[0]       = fresh;
            self.active_tab_id = fresh_id;
            self.sync_address_bar();
            self.update_layout();
            self.dirty.all();
            return;
        }
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        let closing_active = id == self.active_tab_id;
        self.tabs.remove(idx);
        if closing_active {
            let new_idx = idx.min(self.tabs.len() - 1);
            let new_id  = self.tabs[new_idx].id;
            self.activate_tab(new_id);
        }
        self.update_layout();
        self.dirty.all();
    }

    pub fn activate_tab(&mut self, id: TabId) {
        if self.tabs.iter().any(|t| t.id == id) {
            self.active_tab_id   = id;
            self.address_bar_focused = false;
            self.sync_address_bar();
            self.update_layout();
            self.update_bookmark_flag();
            self.dirty.all();
        }
    }

    // ── Navigation ────────────────────────────────────────────────────────────

    pub fn begin_navigate(&mut self, url: &str) -> Option<String> {
        if url.is_empty() { return None; }
        let result = classify_url(url);
        let frame  = self.frame_count;
        let url    = url.to_string();

        // Extract error reason before moving `result` into the tab.
        let fail_reason = match &result {
            NavResult::WillFail(s) => Some(s.clone()),
            NavResult::WillLoad    => None,
        };

        if let Some(reason) = fail_reason {
            // Invalid / unsupported URL — skip Loading, go straight to Error.
            if let Some(tab) = self.active_tab_mut() {
                tab.url         = url.clone();
                tab.display_url = url.clone();
                tab.title       = derive_title(&url).to_string();
                tab.page_state  = PageState::Error(reason);
                tab.nav_result  = result;
            }
            self.address_bar_input   = url;
            self.address_bar_focused = false;
            self.update_bookmark_flag();
            self.dirty.all();
            return None;
        }

        // Structurally valid URL — enter Loading.
        if let Some(tab) = self.active_tab_mut() {
            tab.url              = url.clone();
            tab.display_url      = url.clone();
            tab.title            = derive_title(&url).to_string();
            tab.page_state       = PageState::Loading;
            tab.load_start_frame = frame;
            tab.nav_result       = result;
        }
        self.address_bar_input   = url.clone();
        self.address_bar_focused = false;
        self.update_bookmark_flag();
        self.dirty.all();
        Some(url)
    }

    /// Commit the in-progress navigation for the active tab.
    /// Called from `tick_loading` after the minimum UX delay has elapsed.
    /// Reads the pre-classified `NavResult` — no timer-driven guessing.
    pub fn commit_navigation(&mut self) {
        let (url, result) = match self.active_tab() {
            Some(t) if t.page_state.is_loading() => (t.url.clone(), t.nav_result.clone()),
            _ => return,
        };
        match result {
            NavResult::WillLoad => {
                let title = self.active_tab()
                    .map(|t| if t.title.is_empty() {
                        derive_title(&url).to_string()
                    } else {
                        t.title.clone()
                    })
                    .unwrap_or_else(|| derive_title(&url).to_string());
                if let Some(tab) = self.active_tab_mut() {
                    tab.title      = title.clone();
                    tab.page_state = PageState::Loaded;
                    tab.commit(&url, &title, None);
                }
            }
            NavResult::WillFail(reason) => {
                // Defensive: WillFail shouldn't reach Loading normally, but
                // reload() on a bad URL can produce this.
                if let Some(tab) = self.active_tab_mut() {
                    tab.page_state = PageState::Error(reason);
                    // Failed navigations are not committed to history.
                }
            }
        }
        self.dirty.all();
    }

    /// Resolve loading with a title from a real engine (future use).
    /// For the current transitional model, `commit_navigation` is the primary path.
    pub fn resolve_loading(&mut self, engine_title: String) {
        let url = match self.active_tab() {
            Some(t) if t.page_state.is_loading() => t.url.clone(),
            _ => return,
        };
        let title = if engine_title.is_empty() {
            derive_title(&url).to_string()
        } else {
            engine_title
        };
        if let Some(tab) = self.active_tab_mut() {
            tab.title      = title.clone();
            tab.page_state = PageState::Loaded;
            tab.commit(&url, &title, None);
        }
        self.dirty.all();
    }

    pub fn fail_loading(&mut self, message: &str) {
        if let Some(tab) = self.active_tab_mut() {
            if tab.page_state.is_loading() {
                tab.page_state = PageState::Error(message.to_string());
                self.dirty.all();
            }
        }
    }

    pub fn go_back(&mut self) -> Option<String> {
        let tab = self.active_tab_mut()?;
        if !tab.can_go_back() { return None; }
        tab.history_index -= 1;
        let entry = tab.history[tab.history_index].clone();
        tab.url         = entry.url.clone();
        tab.display_url = entry.display_url.clone();
        tab.title       = entry.title.clone();
        // Restore committed page state directly — no re-navigation, no timer.
        tab.page_state  = match &entry.error_msg {
            Some(err) => PageState::Error(err.clone()),
            None      => PageState::Loaded,
        };
        self.address_bar_input = entry.url.clone();
        self.update_bookmark_flag();
        self.dirty.all();
        None  // State restored from history; caller need not trigger engine.
    }

    pub fn go_forward(&mut self) -> Option<String> {
        let tab = self.active_tab_mut()?;
        if !tab.can_go_forward() { return None; }
        tab.history_index += 1;
        let entry = tab.history[tab.history_index].clone();
        tab.url         = entry.url.clone();
        tab.display_url = entry.display_url.clone();
        tab.title       = entry.title.clone();
        tab.page_state  = match &entry.error_msg {
            Some(err) => PageState::Error(err.clone()),
            None      => PageState::Loaded,
        };
        self.address_bar_input = entry.url.clone();
        self.update_bookmark_flag();
        self.dirty.all();
        None
    }

    pub fn reload(&mut self) -> Option<String> {
        let frame  = self.frame_count;
        let url    = self.active_tab()
            .filter(|t| !t.url.is_empty())
            .map(|t| t.url.clone())?;
        let result = classify_url(&url);
        if let Some(tab) = self.active_tab_mut() {
            tab.page_state       = PageState::Loading;
            tab.load_start_frame = frame;
            tab.nav_result       = result;
        }
        self.dirty.all();
        Some(url)
    }

    // ── Address bar ───────────────────────────────────────────────────────────

    pub fn sync_address_bar(&mut self) {
        let url = self.active_tab().map(|t| t.url.clone()).unwrap_or_default();
        self.address_bar_input = url;
        // sync_address_bar is called internally from activate/close; callers set
        // the appropriate dirty flags themselves.
    }

    pub fn focus_address_bar(&mut self) {
        self.address_bar_focused = true;
        self.dirty_address_bar();
    }

    pub fn cancel_address_bar_edit(&mut self) {
        self.address_bar_focused = false;
        self.sync_address_bar();
        self.dirty_address_bar();
    }

    // ── Theme ─────────────────────────────────────────────────────────────────

    pub fn cycle_theme(&mut self) {
        let next     = self.palette.cycle();
        self.palette = next;
        self.theme   = get_theme(next);
        self.theme_version += 1;
        self.dirty.all();
    }

    // ── Nav button press indicator ────────────────────────────────────────────

    pub fn press_nav_btn(&mut self, id: u8) {
        self.nav_btn_pressed       = id;
        self.nav_btn_pressed_frame = self.frame_count;
        self.dirty.chrome = true;
    }

    pub fn tick_nav_btn(&mut self) {
        if self.nav_btn_pressed != 0 && self.frame_count > self.nav_btn_pressed_frame + 12 {
            self.nav_btn_pressed = 0;
            self.dirty.chrome    = true;
        }
    }

    // ── Bookmarks ─────────────────────────────────────────────────────────────

    pub fn toggle_bookmark(&mut self) {
        let (url, title, was) = match self.active_tab() {
            Some(t) if !t.url.is_empty() => (t.url.clone(), t.title.clone(), t.is_bookmarked),
            _ => return,
        };
        if was {
            self.bookmarks.retain(|b| b.url != url);
        } else {
            self.bookmarks.push(QuickLink::new(title, url));
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.is_bookmarked = !was;
        }
        self.dirty.chrome = true;
    }
}
