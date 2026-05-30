//! Browser core driver.
//!
//! This module owns the browser state plus renderer handle and exposes a small
//! action layer that can be driven by Linux SDL today or a future Kamelot event
//! loop later. Drawing stays in `main.rs`; platform code should feed normalized
//! actions here instead of mutating browser state directly over time.

use crate::input::{BrowserKey, MouseButton, PlatformEvent};
use crate::hit_test::UiHitTarget;
use crate::layout::{FB_HEIGHT, FB_WIDTH, OVERLAY_VISIBLE, TOP_BAR_HEIGHT};
use crate::omnibox::{self, InternalRoute, MatchEntry, OmniboxResult};
use crate::persist;
use crate::theme::ColorPalette;
use crate::ui_state::{
    self, BrowserState, HoveredRegion, OverlayKind, PageState, SplitPane, TabId,
};
use rashamon_renderer::{
    origin_from_url, CursorKind, EngineEvent, PermissionDecision, PermissionKind, RenderEngine,
};

const LOAD_TIMEOUT_FRAMES: u64 = 360;
const SCROLL_LINE_PIXELS: i32 = 40;
const SCROLL_WHEEL_PIXELS: i32 = 80;

#[derive(Default)]
pub(crate) struct SaveDirty {
    pub bookmarks: bool,
    pub history: bool,
    pub prefs: bool,
}

impl SaveDirty {
    pub(crate) fn any(&self) -> bool {
        self.bookmarks || self.history || self.prefs
    }
}

#[allow(dead_code)]
pub(crate) enum BrowserAction {
    Navigate(String),
    NavigateUrl(String),
    Back,
    Forward,
    Reload,
    NewTab { private: bool },
    CloseActiveTab,
    CloseTab(TabId),
    SwitchTab(TabId),
    ToggleSplitView,
    Scroll(i32),
    FocusAddressBar,
    AddressBarChar(char),
    AddressBarBackspace,
    AddressBarSubmit,
    AddressBarCancel,
    OpenOverlay(OverlayKind),
    CloseOverlay,
    OverlayActivate,
    OverlayMove(i32),
    OverlayScroll(i32),
    OpenSiteInfo,
    CloseSiteInfo,
    ToggleBookmark,
    FindOpen,
    FindClose,
    FindChar(char),
    FindBackspace,
    FindNext,
    FindPrevious,
    PermissionAllow,
    PermissionDeny,
    PermissionToggleRemember,
    SitePermissionSet(PermissionKind, PermissionDecision),
    SiteAdblockToggle,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    CycleTheme,
    Quit,
}

pub(crate) struct BrowserCoreDriver {
    pub(crate) state: BrowserState,
    pub(crate) engine: RenderEngine,
    pub(crate) save_dirty: SaveDirty,
    pub(crate) cursor: CursorKind,
}

impl BrowserCoreDriver {
    pub(crate) fn new(content_w: u32, content_h: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let engine = RenderEngine::new(content_w, content_h)?;
        let mut driver = Self {
            state: BrowserState::new(),
            engine,
            save_dirty: SaveDirty::default(),
            cursor: CursorKind::Default,
        };
        driver.load_user_data();
        driver.register_initial_tab();
        Ok(driver)
    }

    pub(crate) fn register_initial_tab(&mut self) {
        let id = self.state.active_tab_id;
        let is_private = self.state.active_tab().map_or(false, |tab| tab.is_private);
        self.engine.create_tab(id.raw(), is_private);
        self.engine.set_active_tab(id.raw());
    }

    pub(crate) fn dispatch(&mut self, action: BrowserAction) -> bool {
        match action {
            BrowserAction::Navigate(raw) => self.omnibox_navigate(&raw),
            BrowserAction::NavigateUrl(url) => self.navigate_url(&url),
            BrowserAction::Back => self.go_back(),
            BrowserAction::Forward => self.go_forward(),
            BrowserAction::Reload => self.reload(),
            BrowserAction::NewTab { private } => self.open_tab(private),
            BrowserAction::CloseActiveTab => self.close_active_tab(),
            BrowserAction::CloseTab(tab_id) => self.close_tab(tab_id),
            BrowserAction::SwitchTab(tab_id) => self.switch_tab(tab_id),
            BrowserAction::ToggleSplitView => self.toggle_split_view(),
            BrowserAction::Scroll(delta) => {
                self.state.scroll_by(delta);
                self.engine.scroll(delta);
            }
            BrowserAction::FocusAddressBar => self.state.focus_address_bar(),
            BrowserAction::AddressBarChar(c) => self.state.type_char(c),
            BrowserAction::AddressBarBackspace => self.state.type_backspace(),
            BrowserAction::AddressBarSubmit => {
                let raw = self.state.address_bar_input.trim().to_string();
                self.state.cancel_address_bar_edit();
                if !raw.is_empty() {
                    self.omnibox_navigate(&raw);
                }
            }
            BrowserAction::AddressBarCancel => self.state.cancel_address_bar_edit(),
            BrowserAction::OpenOverlay(kind) => self.state.toggle_overlay(kind),
            BrowserAction::CloseOverlay => self.state.close_overlay(),
            BrowserAction::OverlayActivate => self.activate_overlay(),
            BrowserAction::OverlayMove(delta) => self.state.overlay_move_selection(delta),
            BrowserAction::OverlayScroll(delta) => self.state.overlay_scroll_by(delta),
            BrowserAction::OpenSiteInfo => self.open_site_info_panel(),
            BrowserAction::CloseSiteInfo => self.state.close_site_info(),
            BrowserAction::ToggleBookmark => {
                self.state.toggle_bookmark();
                self.save_dirty.bookmarks = true;
            }
            BrowserAction::FindOpen => self.find_open(),
            BrowserAction::FindClose => self.find_close(),
            BrowserAction::FindChar(c) => self.find_char(c),
            BrowserAction::FindBackspace => self.find_backspace(),
            BrowserAction::FindNext => self.engine.find_next(),
            BrowserAction::FindPrevious => self.engine.find_previous(),
            BrowserAction::PermissionAllow => self.resolve_permission(true),
            BrowserAction::PermissionDeny => self.resolve_permission(false),
            BrowserAction::PermissionToggleRemember => self.toggle_permission_remember(),
            BrowserAction::SitePermissionSet(kind, decision) => {
                self.set_site_permission(kind, decision);
            }
            BrowserAction::SiteAdblockToggle => self.toggle_site_adblock(),
            BrowserAction::ZoomIn => self.engine.zoom_in(),
            BrowserAction::ZoomOut => self.engine.zoom_out(),
            BrowserAction::ZoomReset => self.engine.zoom_reset(),
            BrowserAction::CycleTheme => {
                self.state.cycle_theme();
                self.save_dirty.prefs = true;
            }
            BrowserAction::Quit => return true,
        }
        false
    }

    pub(crate) fn dispatch_hit(&mut self, target: UiHitTarget) -> bool {
        let close_site_info_first = self.state.site_info.is_some()
            && !matches!(
                target,
                UiHitTarget::SiteInfoPanel
                    | UiHitTarget::SitePermission { .. }
                    | UiHitTarget::SiteAdblockToggle
                    | UiHitTarget::PermissionAllow
                    | UiHitTarget::PermissionDeny
                    | UiHitTarget::PermissionRememberToggle
                    | UiHitTarget::PermissionPrompt
            );
        if close_site_info_first {
            self.state.close_site_info();
        }

        match target {
            UiHitTarget::None | UiHitTarget::SiteInfoPanel | UiHitTarget::PermissionPrompt => false,
            UiHitTarget::BackButton => self.dispatch(BrowserAction::Back),
            UiHitTarget::ForwardButton => self.dispatch(BrowserAction::Forward),
            UiHitTarget::ReloadButton | UiHitTarget::ErrorRetry => self.dispatch(BrowserAction::Reload),
            UiHitTarget::AddressBar | UiHitTarget::NewTabSearch => {
                self.dispatch(BrowserAction::FocusAddressBar)
            }
            UiHitTarget::BookmarkButton => self.dispatch(BrowserAction::ToggleBookmark),
            UiHitTarget::SiteInfoButton => self.dispatch(BrowserAction::OpenSiteInfo),
            UiHitTarget::NewTabButton => self.dispatch(BrowserAction::NewTab { private: false }),
            UiHitTarget::Tab(tab_id) => self.dispatch(BrowserAction::SwitchTab(tab_id)),
            UiHitTarget::CloseTab(tab_id) => self.dispatch(BrowserAction::CloseTab(tab_id)),
            UiHitTarget::QuickLink(url) => self.dispatch(BrowserAction::NavigateUrl(url)),
            UiHitTarget::OverlayActivate => self.dispatch(BrowserAction::OverlayActivate),
            UiHitTarget::PermissionAllow => self.dispatch(BrowserAction::PermissionAllow),
            UiHitTarget::PermissionDeny => self.dispatch(BrowserAction::PermissionDeny),
            UiHitTarget::PermissionRememberToggle => {
                self.dispatch(BrowserAction::PermissionToggleRemember)
            }
            UiHitTarget::SitePermission {
                permission,
                decision,
            } => self.dispatch(BrowserAction::SitePermissionSet(permission, decision)),
            UiHitTarget::SiteAdblockToggle => self.dispatch(BrowserAction::SiteAdblockToggle),
            UiHitTarget::FindClose => self.dispatch(BrowserAction::FindClose),
            UiHitTarget::Content => self.dispatch(BrowserAction::AddressBarCancel),
        }
    }

    fn click_content(&mut self, x: u32, y: u32) {
        let (content_x, content_y) = self.route_content_point(x, y, true);
        self.state.cancel_address_bar_edit();
        self.engine.click(content_x, content_y);
    }

    fn route_content_point(&mut self, x: u32, y: u32, activate: bool) -> (u32, u32) {
        let content_y = y.saturating_sub(TOP_BAR_HEIGHT);
        if self.state.split_view.is_none() {
            return (x, content_y);
        }
        let pane = self.state.split_pane_for_x(x);
        let pane_x = match pane {
            SplitPane::Left => 0,
            SplitPane::Right => FB_WIDTH / 2,
        };
        if activate {
            if self.state.split_active_pane() != Some(pane) {
                if let Some(tab_id) = self.state.activate_split_pane(pane) {
                    self.engine.set_active_tab(tab_id.raw());
                }
            }
        }
        (x.saturating_sub(pane_x), content_y)
    }

    fn content_key_name(key: BrowserKey) -> Option<&'static str> {
        match key {
            BrowserKey::Enter => Some("Enter"),
            BrowserKey::Backspace => Some("Backspace"),
            BrowserKey::Left => Some("ArrowLeft"),
            BrowserKey::Right => Some("ArrowRight"),
            BrowserKey::Up => Some("ArrowUp"),
            BrowserKey::Down => Some("ArrowDown"),
            BrowserKey::PageUp => Some("PageUp"),
            BrowserKey::PageDown => Some("PageDown"),
            _ => None,
        }
    }

    fn update_shell_cursor(&mut self, target: &UiHitTarget) {
        if self.state.active_tab().map_or(false, |tab| tab.page_state.is_loading()) {
            self.cursor = CursorKind::Wait;
            return;
        }
        self.cursor = match target {
            UiHitTarget::BackButton
            | UiHitTarget::ForwardButton
            | UiHitTarget::ReloadButton
            | UiHitTarget::BookmarkButton
            | UiHitTarget::SiteInfoButton
            | UiHitTarget::NewTabButton
            | UiHitTarget::Tab(_)
            | UiHitTarget::CloseTab(_)
            | UiHitTarget::QuickLink(_)
            | UiHitTarget::OverlayActivate
            | UiHitTarget::PermissionAllow
            | UiHitTarget::PermissionDeny
            | UiHitTarget::PermissionRememberToggle
            | UiHitTarget::SitePermission { .. }
            | UiHitTarget::SiteAdblockToggle
            | UiHitTarget::FindClose
            | UiHitTarget::ErrorRetry => CursorKind::Pointer,
            UiHitTarget::AddressBar | UiHitTarget::NewTabSearch => CursorKind::Text,
            UiHitTarget::Content
            | UiHitTarget::None
            | UiHitTarget::PermissionPrompt
            | UiHitTarget::SiteInfoPanel => CursorKind::Default,
        };
    }

    pub(crate) fn handle_platform_event(
        &mut self,
        event: PlatformEvent,
        page_scroll_px: i32,
    ) -> bool {
        match event {
            PlatformEvent::Quit => self.dispatch(BrowserAction::Quit),
            PlatformEvent::Tick | PlatformEvent::MouseUp { .. } => false,
            PlatformEvent::WindowResized { .. } => {
                self.sync_split_viewports();
                false
            }
            PlatformEvent::KeyDown { key, modifiers } => {
                let should_send_to_content = !modifiers.ctrl
                    && !self.state.address_bar_focused
                    && !self.state.find_open
                    && self.state.overlay == OverlayKind::None;
                let quit = self.dispatch_key(key, modifiers.ctrl, modifiers.shift, page_scroll_px);
                if quit {
                    return true;
                }
                if should_send_to_content {
                    if let Some(key_name) = Self::content_key_name(key) {
                        self.engine.key_press(key_name);
                    }
                }
                false
            }
            PlatformEvent::TextInput(text) => {
                if self.state.find_open || self.state.address_bar_focused {
                    for ch in text.chars() {
                        if self.state.find_open {
                            self.dispatch(BrowserAction::FindChar(ch));
                        } else if self.state.address_bar_focused {
                            self.dispatch(BrowserAction::AddressBarChar(ch));
                        }
                    }
                } else if self.state.overlay == OverlayKind::None {
                    self.engine.text_input(&text);
                }
                false
            }
            PlatformEvent::MouseMove { x, y } => {
                let x = x.max(0) as u32;
                let y = y.max(0) as u32;
                self.state.set_mouse_pos(x, y);
                let target = crate::hit_test::hit_test_ui(&self.state, x, y);
                self.update_shell_cursor(&target);
                if matches!(target, UiHitTarget::Content) {
                    let (content_x, content_y) = self.route_content_point(x, y, false);
                    self.engine.mouse_move(content_x, content_y);
                }
                false
            }
            PlatformEvent::MouseDown { x, y, button } => {
                let x = x.max(0) as u32;
                let y = y.max(0) as u32;
                let target = crate::hit_test::hit_test_ui(&self.state, x, y);
                match button {
                    MouseButton::Left => {
                        if matches!(target, UiHitTarget::Content) {
                            self.click_content(x, y);
                            false
                        } else {
                            self.dispatch_hit(target)
                        }
                    }
                    MouseButton::Right if matches!(target, UiHitTarget::Content) => {
                        let (content_x, content_y) = self.route_content_point(x, y, true);
                        self.engine.right_click(content_x, content_y);
                        false
                    }
                    _ => false,
                }
            }
            PlatformEvent::Scroll { delta } => {
                if self.state.overlay != OverlayKind::None {
                    self.dispatch(BrowserAction::OverlayScroll(-delta))
                } else {
                    self.dispatch(BrowserAction::Scroll(-delta * SCROLL_WHEEL_PIXELS))
                }
            }
        }
    }

    pub(crate) fn dispatch_key(
        &mut self,
        key: BrowserKey,
        ctrl: bool,
        shift: bool,
        page_scroll_px: i32,
    ) -> bool {
        if self.state.permission_prompt.is_some() && matches!(key, BrowserKey::Escape) {
            return self.dispatch(BrowserAction::PermissionDeny);
        }

        if self.state.find_open {
            match key {
                BrowserKey::Escape => return self.dispatch(BrowserAction::FindClose),
                BrowserKey::Enter if shift => return self.dispatch(BrowserAction::FindPrevious),
                BrowserKey::Enter => return self.dispatch(BrowserAction::FindNext),
                BrowserKey::Backspace => return self.dispatch(BrowserAction::FindBackspace),
                BrowserKey::Char('f') if ctrl => {
                    self.state.find_input.clear();
                    self.state.find_match_count = None;
                    self.state.dirty_find_bar();
                    self.engine.find_clear();
                    return false;
                }
                BrowserKey::Char(c) if !ctrl => return self.dispatch(BrowserAction::FindChar(c)),
                _ => return false,
            }
        }

        if self.state.overlay != OverlayKind::None {
            match key {
                BrowserKey::Escape => return self.dispatch(BrowserAction::CloseOverlay),
                BrowserKey::Enter => return self.dispatch(BrowserAction::OverlayActivate),
                BrowserKey::Up => return self.dispatch(BrowserAction::OverlayMove(-1)),
                BrowserKey::Down => return self.dispatch(BrowserAction::OverlayMove(1)),
                BrowserKey::PageUp => {
                    return self.dispatch(BrowserAction::OverlayScroll(-(OVERLAY_VISIBLE as i32)));
                }
                BrowserKey::PageDown => {
                    return self.dispatch(BrowserAction::OverlayScroll(OVERLAY_VISIBLE as i32));
                }
                BrowserKey::Char(_) if ctrl => {}
                _ => return false,
            }
        }

        match key {
            BrowserKey::Escape => {
                if self.state.site_info.is_some() {
                    self.dispatch(BrowserAction::CloseSiteInfo)
                } else if self.state.address_bar_focused {
                    self.dispatch(BrowserAction::AddressBarCancel)
                } else {
                    self.dispatch(BrowserAction::Quit)
                }
            }
            BrowserKey::Char('p') if ctrl => self.dispatch(BrowserAction::CycleTheme),
            BrowserKey::Char('t') if ctrl => self.dispatch(BrowserAction::NewTab { private: false }),
            BrowserKey::Char('n') if ctrl && shift => self.dispatch(BrowserAction::NewTab { private: true }),
            BrowserKey::Char('i') if ctrl => self.dispatch(BrowserAction::NewTab { private: true }),
            BrowserKey::Char('w') if ctrl => self.dispatch(BrowserAction::CloseActiveTab),
            BrowserKey::Char('r') if ctrl => self.dispatch(BrowserAction::Reload),
            BrowserKey::Char('s') if ctrl && shift => self.dispatch(BrowserAction::ToggleSplitView),
            BrowserKey::Char('h') if ctrl => self.dispatch(BrowserAction::OpenOverlay(OverlayKind::History)),
            BrowserKey::Char('b') if ctrl => self.dispatch(BrowserAction::OpenOverlay(OverlayKind::Bookmarks)),
            BrowserKey::Char('f') if ctrl => self.dispatch(BrowserAction::FindOpen),
            BrowserKey::Char('l') if ctrl => self.dispatch(BrowserAction::FocusAddressBar),
            BrowserKey::ZoomIn if ctrl => self.dispatch(BrowserAction::ZoomIn),
            BrowserKey::ZoomOut if ctrl => self.dispatch(BrowserAction::ZoomOut),
            BrowserKey::ZoomReset if ctrl => self.dispatch(BrowserAction::ZoomReset),
            BrowserKey::Enter if self.state.address_bar_focused => {
                self.dispatch(BrowserAction::AddressBarSubmit)
            }
            BrowserKey::Backspace if self.state.address_bar_focused => {
                self.dispatch(BrowserAction::AddressBarBackspace)
            }
            BrowserKey::Char(c) if self.state.address_bar_focused => {
                self.dispatch(BrowserAction::AddressBarChar(c))
            }
            BrowserKey::Up if !self.state.address_bar_focused => {
                self.dispatch(BrowserAction::Scroll(-SCROLL_LINE_PIXELS))
            }
            BrowserKey::Down if !self.state.address_bar_focused => {
                self.dispatch(BrowserAction::Scroll(SCROLL_LINE_PIXELS))
            }
            BrowserKey::PageUp if !self.state.address_bar_focused => {
                self.dispatch(BrowserAction::Scroll(-page_scroll_px))
            }
            BrowserKey::PageDown if !self.state.address_bar_focused => {
                self.dispatch(BrowserAction::Scroll(page_scroll_px))
            }
            _ => false,
        }
    }

    pub(crate) fn navigate_url(&mut self, url: &str) {
        if let Some(url) = self.state.begin_navigate(url) {
            let nav_id = self.state.active_tab().map_or(0, |tab| tab.nav_id);
            self.engine.navigate(&url, nav_id).ok();
        }
    }

    pub(crate) fn omnibox_navigate(&mut self, raw: &str) {
        let bm_iter = self.state.bookmarks.iter().map(|b| MatchEntry {
            url: &b.url,
            title: &b.title,
        });
        let hist_iter = self.state.global_history.iter().rev().map(|e| MatchEntry {
            url: &e.url,
            title: &e.title,
        });

        match omnibox::resolve(raw, bm_iter, hist_iter, &omnibox::DEFAULT_PROVIDER) {
            OmniboxResult::Navigate(url) => self.navigate_url(&url),
            OmniboxResult::OpenOverlay(InternalRoute::History) => {
                self.state.toggle_overlay(OverlayKind::History);
            }
            OmniboxResult::OpenOverlay(InternalRoute::Bookmarks) => {
                self.state.toggle_overlay(OverlayKind::Bookmarks);
            }
            OmniboxResult::OpenOverlay(InternalRoute::Blank) => self.open_tab(false),
            OmniboxResult::Nothing => self.state.cancel_address_bar_edit(),
        }
    }

    pub(crate) fn open_tab(&mut self, private: bool) {
        let old_split = self.state.split_view;
        if private {
            self.state.open_private_tab();
        } else {
            self.state.open_new_tab();
        }
        let id = self.state.active_tab_id;
        self.engine.create_tab(id.raw(), private);
        self.engine.set_active_tab(id.raw());
        self.restore_displaced_split_tabs(old_split);
        self.sync_split_viewports();
    }

    fn close_active_tab(&mut self) {
        let tab_id = self.state.active_tab_id;
        self.close_tab(tab_id);
    }

    fn close_tab(&mut self, tab_id: TabId) {
        let was_last = self.state.tabs.len() == 1;
        self.engine.close_tab(tab_id.raw());
        self.state.close_tab(tab_id);
        if was_last {
            let new_id = self.state.active_tab_id;
            self.engine.create_tab(new_id.raw(), false);
        }
        self.sync_split_viewports();
        self.sync_active_engine_tab();
    }

    fn switch_tab(&mut self, tab_id: TabId) {
        if tab_id == self.state.active_tab_id {
            return;
        }
        let old_split = self.state.split_view;
        self.state.activate_tab(tab_id);
        self.restore_displaced_split_tabs(old_split);
        self.sync_split_viewports();
        self.sync_active_engine_tab();
    }

    fn sync_active_engine_tab(&mut self) {
        if self.engine.is_real_engine() {
            self.engine.set_active_tab(self.state.active_tab_id.raw());
        } else if let Some(url) = self
            .state
            .active_tab()
            .map(|tab| tab.url.clone())
            .filter(|url| !url.is_empty())
        {
            self.engine.navigate(&url, 0).ok();
        }
    }

    fn toggle_split_view(&mut self) {
        if self.state.split_view.is_some() {
            let old_split = self.state.split_view;
            self.state.exit_split_view();
            if let Some(split) = old_split {
                self.set_full_tab_viewport(split.left);
                self.set_full_tab_viewport(split.right);
            }
            self.sync_active_engine_tab();
            return;
        }

        let left = self.state.active_tab_id;
        let right = if self.state.tabs.len() == 1 {
            let private = self.state.active_tab().map_or(false, |tab| tab.is_private);
            if private {
                self.state.open_private_tab();
            } else {
                self.state.open_new_tab();
            }
            let id = self.state.active_tab_id;
            self.engine.create_tab(id.raw(), private);
            id
        } else {
            let pos = self
                .state
                .tabs
                .iter()
                .position(|tab| tab.id == left)
                .unwrap_or(0);
            let next = (pos + 1) % self.state.tabs.len();
            self.state.tabs[next].id
        };

        self.state.enter_split_view(left, right, SplitPane::Left);
        self.sync_split_viewports();
        self.sync_active_engine_tab();
    }

    fn sync_split_viewports(&mut self) {
        let Some(split) = self.state.split_view else {
            self.set_full_tab_viewport(self.state.active_tab_id);
            return;
        };
        let content_h = FB_HEIGHT.saturating_sub(TOP_BAR_HEIGHT);
        let left_w = FB_WIDTH / 2;
        let right_w = FB_WIDTH.saturating_sub(left_w);
        self.engine.set_tab_viewport(split.left.raw(), left_w, content_h);
        self.engine.set_tab_viewport(split.right.raw(), right_w, content_h);
    }

    fn set_full_tab_viewport(&mut self, tab_id: TabId) {
        self.engine.set_tab_viewport(
            tab_id.raw(),
            FB_WIDTH,
            FB_HEIGHT.saturating_sub(TOP_BAR_HEIGHT),
        );
    }

    fn restore_displaced_split_tabs(&mut self, old_split: Option<ui_state::SplitViewState>) {
        let Some(old_split) = old_split else { return };
        let new_split = self.state.split_view;
        for old_id in [old_split.left, old_split.right] {
            let still_split = new_split.map_or(false, |split| {
                split.left == old_id || split.right == old_id
            });
            if !still_split && self.state.tabs.iter().any(|tab| tab.id == old_id) {
                self.set_full_tab_viewport(old_id);
            }
        }
    }

    fn go_back(&mut self) {
        self.state.press_nav_btn(1);
        if self.engine.is_real_engine() && self.engine.can_go_back() {
            self.engine.go_back().ok();
        } else if let Some(url) = self.state.go_back() {
            self.navigate_current_nav(&url);
        }
    }

    fn go_forward(&mut self) {
        self.state.press_nav_btn(2);
        if self.engine.is_real_engine() && self.engine.can_go_forward() {
            self.engine.go_forward().ok();
        } else if let Some(url) = self.state.go_forward() {
            self.navigate_current_nav(&url);
        }
    }

    fn reload(&mut self) {
        self.state.press_nav_btn(3);
        if let Some(url) = self.state.reload() {
            self.navigate_current_nav(&url);
        }
    }

    fn navigate_current_nav(&mut self, url: &str) {
        let nav_id = self.state.active_tab().map_or(0, |tab| tab.nav_id);
        self.engine.navigate(url, nav_id).ok();
    }

    fn activate_overlay(&mut self) {
        if let Some(url) = self.state.activate_overlay_item() {
            self.navigate_url(&url);
        }
    }

    fn find_open(&mut self) {
        self.state.close_overlay();
        self.state.address_bar_focused = false;
        self.state.find_open = true;
        self.state.find_input.clear();
        self.state.find_match_count = None;
        self.state.dirty_find_bar();
        self.engine.find_clear();
    }

    fn find_close(&mut self) {
        self.state.find_open = false;
        self.state.find_match_count = None;
        self.state.dirty_find_bar();
        self.engine.find_clear();
    }

    fn find_char(&mut self, ch: char) {
        self.state.find_input.push(ch);
        self.state.find_match_count = None;
        self.state.dirty_find_bar();
        self.engine.find_text(&self.state.find_input);
    }

    fn find_backspace(&mut self) {
        if self.state.find_input.pop().is_some() {
            self.state.find_match_count = None;
            self.state.dirty_find_bar();
            self.engine.find_text(&self.state.find_input);
        }
    }

    fn resolve_permission(&mut self, allow: bool) {
        let Some(prompt) = self.state.permission_prompt.clone() else {
            return;
        };
        self.engine
            .resolve_permission(prompt.id, allow, prompt.remember);
        self.state.clear_permission_prompt(prompt.id);
    }

    fn toggle_permission_remember(&mut self) {
        if let Some(prompt) = self.state.permission_prompt.as_mut() {
            prompt.remember = !prompt.remember;
            self.state.dirty.content = true;
        }
    }

    fn active_origin(&self) -> Option<String> {
        self.state
            .active_tab()
            .and_then(|tab| origin_from_url(&tab.url))
    }

    fn open_site_info_panel(&mut self) {
        let origin = self.active_origin();
        let private = self.state.active_tab().map_or(false, |tab| tab.is_private);
        self.state.close_overlay();
        self.state.open_site_info(origin.clone());
        self.state.address_bar_focused = false;
        if let Some(origin) = origin {
            self.engine.query_site_permissions(&origin, private);
        }
    }

    fn set_site_permission(&mut self, kind: PermissionKind, decision: PermissionDecision) {
        let Some(origin) = self.state.site_info.as_ref().and_then(|panel| panel.origin.clone())
        else {
            return;
        };
        let private = self.state.active_tab().map_or(false, |tab| tab.is_private);
        self.engine
            .set_site_permission(&origin, kind, decision, private);
        if let Some(panel) = self.state.site_info.as_mut() {
            if let Some((_, current)) = panel.permissions.iter_mut().find(|(k, _)| *k == kind) {
                *current = decision;
            }
        }
        self.state.dirty.content = true;
    }

    fn toggle_site_adblock(&mut self) {
        let Some(origin) = self.state.site_info.as_ref().and_then(|panel| panel.origin.clone())
        else {
            return;
        };
        let private = self.state.active_tab().map_or(false, |tab| tab.is_private);
        let next_allowlisted = !self
            .state
            .site_info
            .as_ref()
            .map_or(false, |panel| panel.adblock_allowlisted);
        self.engine
            .set_site_adblock_allowlisted(&origin, next_allowlisted, private);
        if let Some(panel) = self.state.site_info.as_mut() {
            panel.adblock_allowlisted = next_allowlisted;
        }
        self.state.dirty.content = true;
    }

    pub(crate) fn pump_engine(&mut self) {
        self.engine.pump_gtk();
    }

    pub(crate) fn tick_loading(&mut self) {
        let Some(tab) = self.state.active_tab() else {
            return;
        };
        if !tab.page_state.is_loading() {
            return;
        }
        let timeout = if self.engine.is_real_engine() {
            LOAD_TIMEOUT_FRAMES * 5
        } else {
            LOAD_TIMEOUT_FRAMES
        };
        if self.state.frame_count.saturating_sub(tab.load_start_frame) >= timeout {
            self.state.fail_loading("Request timed out");
        }
    }

    pub(crate) fn poll_engine_events(&mut self) -> Vec<(u64, EngineEvent)> {
        let events = self.engine.poll_events();
        for (tab_id, ev) in events.iter().cloned() {
            self.apply_engine_event(tab_id, ev);
        }
        events
    }

    fn apply_engine_event(&mut self, tab_id: u64, ev: EngineEvent) {
        let target_raw = if tab_id == 0 {
            self.state.active_tab_id.raw()
        } else {
            tab_id
        };
        let is_active = target_raw == self.state.active_tab_id.raw();

        match ev {
            EngineEvent::TitleChanged(title) => {
                if let Some(tab) = self
                    .state
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id.raw() == target_raw)
                {
                    tab.title = title;
                }
                if is_active {
                    self.state.dirty.chrome = true;
                } else {
                    self.state.dirty.tabs = true;
                }
            }
            EngineEvent::UrlChanged(url) => {
                if let Some(tab) = self
                    .state
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id.raw() == target_raw)
                {
                    tab.url = url.clone();
                }
                if is_active {
                    if !self.state.address_bar_focused {
                        self.state.address_bar_input = url;
                    }
                    self.state.dirty.chrome = true;
                }
            }
            EngineEvent::LoadComplete => {
                self.state.resolve_engine_loading_for(target_raw);
                if is_active {
                    if !self.state.address_bar_focused {
                        self.state.sync_address_bar();
                    }
                    self.state.dirty.content = true;
                    self.save_dirty.history = true;
                }
            }
            EngineEvent::LoadFailed(reason) => {
                self.state.fail_loading_for(target_raw, &reason);
            }
            EngineEvent::ContentHeightChanged(height) => {
                self.state.set_content_height_for(target_raw, height);
            }
            EngineEvent::NavStateChanged {
                can_back,
                can_forward,
            } => {
                if let Some(tab) = self
                    .state
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id.raw() == target_raw)
                {
                    tab.webkit_can_back = can_back;
                    tab.webkit_can_forward = can_forward;
                }
                if is_active {
                    self.state.dirty.chrome = true;
                }
            }
            EngineEvent::FindMatchCount(count) => {
                if is_active {
                    self.state.find_match_count = Some(count);
                    self.state.dirty_find_bar();
                }
            }
            EngineEvent::DownloadStarted { id, filename, path } => {
                self.state.upsert_download_started(id, filename, path);
            }
            EngineEvent::DownloadProgress {
                id,
                received,
                progress,
            } => {
                self.state.update_download_progress(id, received, progress);
            }
            EngineEvent::DownloadFinished { id, path } => {
                self.state.finish_download(id, path);
            }
            EngineEvent::DownloadFailed { id, reason } => {
                self.state.fail_download(id, reason);
            }
            EngineEvent::PermissionPrompt {
                id,
                origin,
                kind,
                nav_id,
            } => {
                self.state
                    .show_permission_prompt(id, target_raw, nav_id, origin, kind);
            }
            EngineEvent::PermissionResolved { id } => {
                self.state.clear_permission_prompt(id);
            }
            EngineEvent::SitePermissions {
                origin,
                decisions,
                adblock_enabled,
                adblock_allowlisted,
                blocked_count,
            } => {
                self.state.set_site_permissions(
                    origin,
                    decisions,
                    adblock_enabled,
                    adblock_allowlisted,
                    blocked_count,
                );
            }
            EngineEvent::CursorChanged(cursor) => {
                if matches!(self.state.hovered_region, HoveredRegion::ContentArea) {
                    self.cursor = cursor;
                }
            }
            EngineEvent::FrameReady { .. } => {}
            EngineEvent::LoadStarted => {
                let frame = self.state.frame_count;
                if let Some(tab) = self
                    .state
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id.raw() == target_raw)
                {
                    tab.page_state = PageState::Loading;
                    tab.load_start_frame = frame;
                }
                if is_active {
                    self.state.dirty.content = true;
                }
            }
        }
    }

    pub(crate) fn flush_saves(&mut self) {
        if self.save_dirty.bookmarks {
            let bookmarks: Vec<persist::StoredBookmark> = self
                .state
                .bookmarks
                .iter()
                .map(|b| persist::StoredBookmark {
                    title: b.title.clone(),
                    url: b.url.clone(),
                })
                .collect();
            std::thread::spawn(move || persist::save_bookmarks(&bookmarks));
            self.save_dirty.bookmarks = false;
        }

        if self.save_dirty.history {
            let history: Vec<persist::StoredHistory> = self
                .state
                .global_history
                .iter()
                .map(|entry| persist::StoredHistory {
                    url: entry.url.clone(),
                    title: entry.title.clone(),
                })
                .collect();
            std::thread::spawn(move || persist::save_history(&history));
            self.save_dirty.history = false;
        }

        if self.save_dirty.prefs {
            let name = self.state.palette.as_str().to_string();
            std::thread::spawn(move || persist::save_theme(&name));
            self.save_dirty.prefs = false;
        }
    }

    fn load_user_data(&mut self) {
        if let Some(theme_str) = persist::load_theme() {
            if let Some(palette) = ColorPalette::from_str(&theme_str) {
                self.state.apply_palette(palette);
            }
        }

        let stored_bookmarks = persist::load_bookmarks();
        if !stored_bookmarks.is_empty() {
            self.state.bookmarks = stored_bookmarks
                .into_iter()
                .map(|b| ui_state::QuickLink::new(b.title, b.url))
                .collect();
        }

        for entry in persist::load_history() {
            self.state
                .global_history
                .push(ui_state::GlobalHistoryEntry {
                    url: entry.url,
                    title: entry.title,
                    when: 0,
                });
        }
    }
}
