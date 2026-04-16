//! Shared layout constants — imported by both rendering and hit-test code.

pub const FB_WIDTH:          u32 = 1920;
pub const FB_HEIGHT:         u32 = 1080;
pub const TAB_BAR_HEIGHT:    u32 = 38;
pub const CHROME_BAR_HEIGHT: u32 = 44;
pub const TOP_BAR_HEIGHT:    u32 = TAB_BAR_HEIGHT + CHROME_BAR_HEIGHT;

pub const TAB_START_X:   u32 = 8;
pub const TAB_SEP:       u32 = 2;
pub const TAB_MAX_W:     u32 = 180;
pub const TAB_MIN_W:     u32 = 80;
pub const TAB_NEW_BTN_W: u32 = 36;

pub const ADDR_BAR_W: u32 = 700;
pub const ADDR_BAR_H: u32 = 30;
pub const ADDR_BAR_R: u32 = 15;

pub const RETRY_BTN_W: u32 = 140;
pub const RETRY_BTN_H: u32 = 38;

pub const QUICK_LINK_W:   u32 = 120;
pub const QUICK_LINK_H:   u32 = 100;
pub const QUICK_LINK_GAP: u32 = 16;

#[inline]
pub fn tab_width(n: usize) -> u32 {
    let avail = FB_WIDTH.saturating_sub(TAB_START_X + TAB_NEW_BTN_W + 12);
    ((avail / n.max(1) as u32).saturating_sub(TAB_SEP))
        .min(TAB_MAX_W)
        .max(TAB_MIN_W)
}

#[inline]
pub fn retry_btn_pos() -> (u32, u32) {
    let cx = FB_WIDTH / 2;
    let cy = TOP_BAR_HEIGHT + (FB_HEIGHT - TOP_BAR_HEIGHT) / 2;
    (cx.saturating_sub(RETRY_BTN_W / 2), cy + 80)
}
