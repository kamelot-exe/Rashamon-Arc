//! Semantic hit-testing for Rashamon Arc chrome/content.
//!
//! This keeps pixel/layout knowledge separate from browser behavior. Platform
//! loops feed coordinates here, then send the resulting target to the core
//! driver.

use crate::layout::*;
use crate::ui_state::{BrowserState, OverlayKind, PageState, TabId};
use rashamon_renderer::{PermissionDecision, PermissionKind};

pub(crate) type Rect = (u32, u32, u32, u32);

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) enum UiHitTarget {
    None,
    BackButton,
    ForwardButton,
    ReloadButton,
    AddressBar,
    BookmarkButton,
    SiteInfoButton,
    NewTabButton,
    Tab(TabId),
    CloseTab(TabId),
    NewTabSearch,
    QuickLink(String),
    OverlayActivate,
    PermissionAllow,
    PermissionDeny,
    PermissionRememberToggle,
    PermissionPrompt,
    SiteInfoPanel,
    SitePermission {
        permission: PermissionKind,
        decision: PermissionDecision,
    },
    SiteAdblockToggle,
    FindClose,
    ErrorRetry,
    Content,
}

pub(crate) fn in_rect(x: u32, y: u32, rect: Rect) -> bool {
    let (rx, ry, rw, rh) = rect;
    x >= rx && x < rx + rw && y >= ry && y < ry + rh
}

pub(crate) fn hit_test_ui(state: &BrowserState, x: u32, y: u32) -> UiHitTarget {
    if let Some(target) = hit_permission_prompt(state, x, y) {
        return target;
    }
    if let Some(target) = hit_site_info_panel(state, x, y) {
        return target;
    }
    if y < TAB_BAR_HEIGHT {
        return hit_tab_bar(state, x);
    }
    if y < TOP_BAR_HEIGHT {
        return hit_chrome_bar(state, x, y);
    }
    if state.overlay != OverlayKind::None {
        return UiHitTarget::OverlayActivate;
    }
    hit_content(state, x, y)
}

fn hit_permission_prompt(state: &BrowserState, x: u32, y: u32) -> Option<UiHitTarget> {
    let prompt = state.permission_prompt.as_ref()?;
    let prompt_current = state.tabs.iter().any(|tab| {
        tab.id.raw() == prompt.tab_id && tab.id == state.active_tab_id && tab.nav_id == prompt.nav_id
    });
    if !prompt_current {
        return None;
    }
    let (remember, deny, allow) = permission_prompt_hit_rects();
    if in_rect(x, y, remember) {
        Some(UiHitTarget::PermissionRememberToggle)
    } else if in_rect(x, y, deny) {
        Some(UiHitTarget::PermissionDeny)
    } else if in_rect(x, y, allow) {
        Some(UiHitTarget::PermissionAllow)
    } else if in_rect(x, y, permission_prompt_rect()) {
        Some(UiHitTarget::PermissionPrompt)
    } else {
        None
    }
}

fn hit_site_info_panel(state: &BrowserState, x: u32, y: u32) -> Option<UiHitTarget> {
    state.site_info.as_ref()?;
    if !in_rect(x, y, site_info_rect()) {
        return None;
    }
    if state.site_info.as_ref().and_then(|panel| panel.origin.as_ref()).is_none() {
        return Some(UiHitTarget::SiteInfoPanel);
    }
    if in_rect(x, y, site_info_adblock_rect()) {
        return Some(UiHitTarget::SiteAdblockToggle);
    }
    for (idx, kind) in permission_kinds_ui().iter().enumerate() {
        for (decision, rect) in site_info_permission_rects(idx) {
            if in_rect(x, y, rect) {
                return Some(UiHitTarget::SitePermission {
                    permission: *kind,
                    decision,
                });
            }
        }
    }
    Some(UiHitTarget::SiteInfoPanel)
}

fn hit_tab_bar(state: &BrowserState, x: u32) -> UiHitTarget {
    let tw = state.tab_width;
    for i in 0..state.tabs.len() {
        let lx = TAB_START_X + i as u32 * (tw + TAB_SEP);
        let rx = lx + tw;
        if x >= lx && x < rx {
            let id = state.tabs[i].id;
            return if x >= lx + tw.saturating_sub(18) {
                UiHitTarget::CloseTab(id)
            } else {
                UiHitTarget::Tab(id)
            };
        }
    }
    let next_x = TAB_START_X + state.tabs.len() as u32 * (tw + TAB_SEP);
    if x >= next_x && x < next_x + TAB_NEW_BTN_W {
        UiHitTarget::NewTabButton
    } else {
        UiHitTarget::None
    }
}

fn hit_chrome_bar(state: &BrowserState, x: u32, y: u32) -> UiHitTarget {
    if state.find_open {
        let fx = FB_WIDTH.saturating_sub(380 + 48);
        let fy = TAB_BAR_HEIGHT + (CHROME_BAR_HEIGHT - 28) / 2;
        if x >= fx + 356 && x < fx + 380 && y >= fy && y < fy + 28 {
            return UiHitTarget::FindClose;
        }
    }

    let btn_r: u32 = 16;
    if x >= 12 && x < 12 + btn_r * 2 {
        return UiHitTarget::BackButton;
    }
    if x >= 54 && x < 54 + btn_r * 2 {
        return UiHitTarget::ForwardButton;
    }
    if x >= 96 && x < 96 + btn_r * 2 {
        return UiHitTarget::ReloadButton;
    }

    let bar_x = (FB_WIDTH - ADDR_BAR_W) / 2;
    let bar_y = TAB_BAR_HEIGHT + (CHROME_BAR_HEIGHT - ADDR_BAR_H) / 2;
    if x >= bar_x && x < bar_x + 30 && y >= bar_y && y < bar_y + ADDR_BAR_H {
        return UiHitTarget::SiteInfoButton;
    }
    if x >= bar_x + ADDR_BAR_W - 26 && x < bar_x + ADDR_BAR_W
        && y >= bar_y && y < bar_y + ADDR_BAR_H
    {
        return UiHitTarget::BookmarkButton;
    }
    if x >= bar_x && x < bar_x + ADDR_BAR_W && y >= bar_y && y < bar_y + ADDR_BAR_H {
        UiHitTarget::AddressBar
    } else {
        UiHitTarget::None
    }
}

fn hit_content(state: &BrowserState, x: u32, y: u32) -> UiHitTarget {
    match state.active_tab().map(|tab| &tab.page_state) {
        Some(PageState::Error(_)) => {
            let (bx, by) = retry_btn_pos();
            if x >= bx && x < bx + RETRY_BTN_W && y >= by && y < by + RETRY_BTN_H {
                return UiHitTarget::ErrorRetry;
            }
        }
        Some(PageState::NewTab) => {
            let cx = FB_WIDTH / 2;
            let cy = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
            let sw: u32 = 600;
            let sh: u32 = 48;
            let sx = cx.saturating_sub(sw / 2);
            let sy = cy.saturating_sub(90);
            if x >= sx && x < sx + sw && y >= sy && y < sy + sh {
                return UiHitTarget::NewTabSearch;
            }

            let num = state.bookmarks.len().min(6) as u32;
            if num > 0 {
                let row_w = num * QUICK_LINK_W + (num - 1) * QUICK_LINK_GAP;
                let mut lx = cx.saturating_sub(row_w / 2);
                let ly = cy + 46;
                for bookmark in state.bookmarks.iter().take(6) {
                    if x >= lx && x < lx + QUICK_LINK_W && y >= ly && y < ly + QUICK_LINK_H {
                        return UiHitTarget::QuickLink(bookmark.url.clone());
                    }
                    lx += QUICK_LINK_W + QUICK_LINK_GAP;
                }
            }
        }
        _ => {}
    }
    UiHitTarget::Content
}

pub(crate) fn permission_prompt_rect() -> Rect {
    (24, TOP_BAR_HEIGHT + 20, 560, 88)
}

pub(crate) fn permission_prompt_hit_rects() -> (Rect, Rect, Rect) {
    let (x, y, w, _) = permission_prompt_rect();
    (
        (x + 18, y + 56, 150, 22),
        (x + w - 180, y + 52, 72, 26),
        (x + w - 96, y + 52, 72, 26),
    )
}

pub(crate) fn site_info_rect() -> Rect {
    let bar_x = (FB_WIDTH - ADDR_BAR_W) / 2;
    (bar_x, TOP_BAR_HEIGHT + 8, 560, 292)
}

pub(crate) fn permission_kinds_ui() -> [PermissionKind; 5] {
    [
        PermissionKind::Notifications,
        PermissionKind::Geolocation,
        PermissionKind::Camera,
        PermissionKind::Microphone,
        PermissionKind::Clipboard,
    ]
}

pub(crate) fn site_info_permission_rects(row: usize) -> [(PermissionDecision, Rect); 3] {
    let (x, y, _, _) = site_info_rect();
    let row_y = y + 108 + row as u32 * 30;
    [
        (PermissionDecision::Ask, (x + 300, row_y, 52, 22)),
        (PermissionDecision::Allow, (x + 360, row_y, 62, 22)),
        (PermissionDecision::Deny, (x + 430, row_y, 56, 22)),
    ]
}

pub(crate) fn site_info_adblock_rect() -> Rect {
    let (x, y, w, h) = site_info_rect();
    (x + w - 190, y + h - 34, 166, 24)
}
