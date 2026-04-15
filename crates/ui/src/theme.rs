//! Theming system for Rashamon Arc.

use rashamon_renderer::framebuffer::Pixel;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorPalette {
    KamelotDark,
    GraphiteMinimal,
    LightClean,
}

impl ColorPalette {
    pub fn cycle(&self) -> Self {
        match self {
            Self::KamelotDark => Self::GraphiteMinimal,
            Self::GraphiteMinimal => Self::LightClean,
            Self::LightClean => Self::KamelotDark,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    // Core
    pub bg: Pixel,
    pub fg: Pixel,
    pub border: Pixel,

    // UI components
    pub tab_bg: Pixel,
    pub tab_fg: Pixel,
    pub tab_active_bg: Pixel,
    pub tab_active_fg: Pixel,
    pub tab_hover_bg: Pixel,
    pub address_bar_bg: Pixel,
    pub address_bar_fg: Pixel,

    // Accents
    pub accent: Pixel,
    pub accent_fg: Pixel,
    pub security_ok: Pixel,
    pub security_err: Pixel,
}

pub const KAMELOT_DARK: Theme = Theme {
    bg: Pixel { r: 11, g: 11, b: 11 },         // #0B0B0B
    fg: Pixel { r: 220, g: 220, b: 220 },   // #DCDCDC
    border: Pixel { r: 43, g: 43, b: 43 },     // #2B2B2B
    tab_bg: Pixel { r: 28, g: 28, b: 28 },     // #1C1C1C
    tab_fg: Pixel { r: 150, g: 150, b: 150 }, // #969696
    tab_active_bg: Pixel { r: 11, g: 11, b: 11 }, // Same as main bg
    tab_active_fg: Pixel { r: 220, g: 220, b: 220 },
    tab_hover_bg: Pixel { r: 40, g: 40, b: 40 },
    address_bar_bg: Pixel { r: 22, g: 22, b: 22 },
    address_bar_fg: Pixel { r: 200, g: 200, b: 200 },
    accent: Pixel { r: 205, g: 92, b: 0 },      // #CD5C00 (Burnt Orange)
    accent_fg: Pixel { r: 255, g: 255, b: 255 },
    security_ok: Pixel { r: 34, g: 139, b: 34 }, // ForestGreen
    security_err: Pixel { r: 220, g: 20, b: 60 }, // Crimson
};

pub const GRAPHITE_MINIMAL: Theme = Theme {
    bg: Pixel { r: 20, g: 20, b: 20 },         // #141414
    fg: Pixel { r: 180, g: 180, b: 180 },   // #B4B4B4
    border: Pixel { r: 40, g: 40, b: 40 },     // #282828
    tab_bg: Pixel { r: 30, g: 30, b: 30 },     // #1E1E1E
    tab_fg: Pixel { r: 120, g: 120, b: 120 }, // #787878
    tab_active_bg: Pixel { r: 20, g: 20, b: 20 },
    tab_active_fg: Pixel { r: 180, g: 180, b: 180 },
    tab_hover_bg: Pixel { r: 45, g: 45, b: 45 },
    address_bar_bg: Pixel { r: 25, g: 25, b: 25 },
    address_bar_fg: Pixel { r: 170, g: 170, b: 170 },
    accent: Pixel { r: 80, g: 80, b: 80 },      // #505050
    accent_fg: Pixel { r: 220, g: 220, b: 220 },
    security_ok: Pixel { r: 80, g: 120, b: 80 },
    security_err: Pixel { r: 140, g: 80, b: 80 },
};

pub const LIGHT_CLEAN: Theme = Theme {
    bg: Pixel { r: 245, g: 245, b: 245 },   // #F5F5F5
    fg: Pixel { r: 20, g: 20, b: 20 },         // #141414
    border: Pixel { r: 210, g: 210, b: 210 }, // #D2D2D2
    tab_bg: Pixel { r: 230, g: 230, b: 230 }, // #E6E6E6
    tab_fg: Pixel { r: 100, g: 100, b: 100 }, // #646464
    tab_active_bg: Pixel { r: 245, g: 245, b: 245 },
    tab_active_fg: Pixel { r: 20, g: 20, b: 20 },
    tab_hover_bg: Pixel { r: 215, g: 215, b: 215 },
    address_bar_bg: Pixel { r: 255, g: 255, b: 255 },
    address_bar_fg: Pixel { r: 30, g: 30, b: 30 },
    accent: Pixel { r: 0, g: 122, b: 255 },     // #007AFF (Standard Blue)
    accent_fg: Pixel { r: 255, g: 255, b: 255 },
    security_ok: Pixel { r: 46, g: 204, b: 113 },
    security_err: Pixel { r: 231, g: 76, b: 60 },
};

pub fn get_theme(palette: ColorPalette) -> Theme {
    match palette {
        ColorPalette::KamelotDark => KAMELOT_DARK,
        ColorPalette::GraphiteMinimal => GRAPHITE_MINIMAL,
        ColorPalette::LightClean => LIGHT_CLEAN,
    }
}
