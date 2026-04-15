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
    pub bg: Pixel,              // Content area background
    pub fg: Pixel,              // Primary text
    pub border: Pixel,          // Subtle borders
    pub surface: Pixel,         // Top bar background
    pub placeholder: Pixel,     // Placeholder text

    // UI components
    pub tab_bg: Pixel,
    pub tab_fg: Pixel,
    pub tab_active_bg: Pixel,
    pub tab_active_fg: Pixel,
    pub tab_hover_bg: Pixel,
    pub address_bar_bg: Pixel,
    pub address_bar_fg: Pixel,
    pub icon_fg: Pixel,

    // Accents
    pub accent: Pixel,
    pub accent_fg: Pixel,
    pub security_ok: Pixel,
    pub security_err: Pixel,
}

pub const KAMELOT_DARK: Theme = Theme {
    bg: Pixel { r: 11, g: 11, b: 11 },          // #0B0B0B
    fg: Pixel { r: 241, g: 241, b: 241 },    // #F1F1F1
    border: Pixel { r: 43, g: 43, b: 43 },      // #2B2B2B
    surface: Pixel { r: 17, g: 17, b: 17 },     // #111111
    placeholder: Pixel { r: 100, g: 100, b: 100 },

    tab_bg: Pixel { r: 30, g: 30, b: 30 },      // #1E1E1E
    tab_fg: Pixel { r: 160, g: 160, b: 160 },
    tab_active_bg: Pixel { r: 11, g: 11, b: 11 }, // Same as content bg
    tab_active_fg: Pixel { r: 241, g: 241, b: 241 },
    tab_hover_bg: Pixel { r: 45, g: 45, b: 45 },
    address_bar_bg: Pixel { r: 30, g: 30, b: 30 }, // #1E1E1E
    address_bar_fg: Pixel { r: 241, g: 241, b: 241 },
    icon_fg: Pixel { r: 180, g: 180, b: 180 },

    accent: Pixel { r: 200, g: 107, b: 60 },    // #C86B3C
    accent_fg: Pixel { r: 255, g: 255, b: 255 },
    security_ok: Pixel { r: 34, g: 139, b: 34 },
    security_err: Pixel { r: 220, g: 20, b: 60 },
};

pub const GRAPHITE_MINIMAL: Theme = Theme {
    bg: Pixel { r: 20, g: 20, b: 20 },         // #141414
    fg: Pixel { r: 180, g: 180, b: 180 },   // #B4B4B4
    border: Pixel { r: 40, g: 40, b: 40 },     // #282828
    surface: Pixel { r: 25, g: 25, b: 25 },     // #191919
    placeholder: Pixel { r: 90, g: 90, b: 90 }, // #5A5A5A

    tab_bg: Pixel { r: 30, g: 30, b: 30 },     // #1E1E1E
    tab_fg: Pixel { r: 120, g: 120, b: 120 }, // #787878
    tab_active_bg: Pixel { r: 20, g: 20, b: 20 },
    tab_active_fg: Pixel { r: 180, g: 180, b: 180 },
    tab_hover_bg: Pixel { r: 45, g: 45, b: 45 },
    address_bar_bg: Pixel { r: 25, g: 25, b: 25 },
    address_bar_fg: Pixel { r: 170, g: 170, b: 170 },
    icon_fg: Pixel { r: 140, g: 140, b: 140 }, // #8C8C8C

    accent: Pixel { r: 80, g: 80, b: 80 },      // #505050
    accent_fg: Pixel { r: 220, g: 220, b: 220 },
    security_ok: Pixel { r: 80, g: 120, b: 80 },
    security_err: Pixel { r: 140, g: 80, b: 80 },
};

pub const LIGHT_CLEAN: Theme = Theme {
    bg: Pixel { r: 245, g: 245, b: 245 },   // #F5F5F5
    fg: Pixel { r: 20, g: 20, b: 20 },         // #141414
    border: Pixel { r: 210, g: 210, b: 210 }, // #D2D2D2
    surface: Pixel { r: 255, g: 255, b: 255 },  // #FFFFFF
    placeholder: Pixel { r: 150, g: 150, b: 150 }, // #969696

    tab_bg: Pixel { r: 230, g: 230, b: 230 }, // #E6E6E6
    tab_fg: Pixel { r: 100, g: 100, b: 100 }, // #646464
    tab_active_bg: Pixel { r: 245, g: 245, b: 245 },
    tab_active_fg: Pixel { r: 20, g: 20, b: 20 },
    tab_hover_bg: Pixel { r: 215, g: 215, b: 215 },
    address_bar_bg: Pixel { r: 255, g: 255, b: 255 },
    address_bar_fg: Pixel { r: 30, g: 30, b: 30 },
    icon_fg: Pixel { r: 120, g: 120, b: 120 }, // #787878

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
