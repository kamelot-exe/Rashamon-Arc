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
            Self::KamelotDark     => Self::GraphiteMinimal,
            Self::GraphiteMinimal => Self::LightClean,
            Self::LightClean      => Self::KamelotDark,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::KamelotDark     => "kamelot-dark",
            Self::GraphiteMinimal => "graphite-minimal",
            Self::LightClean      => "light-clean",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "kamelot-dark"     => Some(Self::KamelotDark),
            "graphite-minimal" => Some(Self::GraphiteMinimal),
            "light-clean"      => Some(Self::LightClean),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    // Core
    pub bg: Pixel,               // Content area background
    pub fg: Pixel,               // Primary text
    pub fg_secondary: Pixel,     // Secondary / dimmed text
    pub border: Pixel,           // Subtle borders
    pub surface: Pixel,          // Chrome bar background (address bar row)
    pub placeholder: Pixel,      // Placeholder text

    // Tab system
    pub tab_bar_bg: Pixel,       // Tab strip row background (slightly darker)
    pub tab_bg: Pixel,           // Inactive tab background
    pub tab_fg: Pixel,           // Inactive tab text
    pub tab_active_bg: Pixel,    // Active tab bg (matches surface for connected look)
    pub tab_active_fg: Pixel,    // Active tab text
    pub tab_hover_bg: Pixel,     // Hovered tab background
    pub tab_close_hover: Pixel,  // Close button hover background

    // Address bar
    pub address_bar_bg: Pixel,
    pub address_bar_bg_focused: Pixel,
    pub address_bar_fg: Pixel,
    pub address_bar_border: Pixel,
    pub address_bar_border_focused: Pixel,

    // Controls
    pub icon_fg: Pixel,
    pub control_hover_bg: Pixel,

    // New tab page
    pub new_tab_card_bg: Pixel,
    pub new_tab_card_hover_bg: Pixel,

    // Accents
    pub accent: Pixel,
    pub accent_fg: Pixel,
    pub security_ok: Pixel,
    pub security_err: Pixel,
}

pub const KAMELOT_DARK: Theme = Theme {
    bg:           Pixel { r: 13,  g: 15,  b: 19  }, // deep slate, not pure black
    fg:           Pixel { r: 238, g: 241, b: 245 },
    fg_secondary: Pixel { r: 132, g: 140, b: 150 },
    border:       Pixel { r: 42,  g: 48,  b: 58  },
    surface:      Pixel { r: 22,  g: 25,  b: 31  },
    placeholder:  Pixel { r: 104, g: 112, b: 124 },

    tab_bar_bg:        Pixel { r: 16,  g: 18,  b: 23  },
    tab_bg:            Pixel { r: 24,  g: 27,  b: 34  },
    tab_fg:            Pixel { r: 146, g: 154, b: 164 },
    tab_active_bg:     Pixel { r: 22,  g: 25,  b: 31  },
    tab_active_fg:     Pixel { r: 238, g: 238, b: 238 }, // full bright
    tab_hover_bg:      Pixel { r: 34,  g: 39,  b: 48  },
    tab_close_hover:   Pixel { r: 58,  g: 65,  b: 78  },

    address_bar_bg:            Pixel { r: 30,  g: 34,  b: 42  },
    address_bar_bg_focused:    Pixel { r: 35,  g: 40,  b: 49  },
    address_bar_fg:            Pixel { r: 238, g: 238, b: 238 },
    address_bar_border:        Pixel { r: 50,  g: 57,  b: 69  },
    address_bar_border_focused: Pixel { r: 223, g: 132, b: 83  },

    icon_fg:          Pixel { r: 168, g: 176, b: 188 },
    control_hover_bg: Pixel { r: 39,  g: 45,  b: 55  },

    new_tab_card_bg:       Pixel { r: 24,  g: 28,  b: 35  },
    new_tab_card_hover_bg: Pixel { r: 36,  g: 42,  b: 52  },

    accent:       Pixel { r: 200, g: 107, b: 60  }, // #C86B3C warm orange
    accent_fg:    Pixel { r: 255, g: 255, b: 255 },
    security_ok:  Pixel { r: 52,  g: 168, b: 83  }, // green
    security_err: Pixel { r: 220, g: 53,  b: 69  }, // red
};

pub const GRAPHITE_MINIMAL: Theme = Theme {
    bg:           Pixel { r: 16,  g: 16,  b: 16  }, // #101010
    fg:           Pixel { r: 190, g: 190, b: 190 }, // #BEBEBE
    fg_secondary: Pixel { r: 90,  g: 90,  b: 90  }, // #5A5A5A
    border:       Pixel { r: 34,  g: 34,  b: 34  }, // #222222
    surface:      Pixel { r: 22,  g: 22,  b: 22  }, // #161616
    placeholder:  Pixel { r: 75,  g: 75,  b: 75  }, // #4B4B4B

    tab_bar_bg:        Pixel { r: 14,  g: 14,  b: 14  }, // #0E0E0E
    tab_bg:            Pixel { r: 24,  g: 24,  b: 24  }, // #181818
    tab_fg:            Pixel { r: 100, g: 100, b: 100 },
    tab_active_bg:     Pixel { r: 22,  g: 22,  b: 22  }, // matches surface
    tab_active_fg:     Pixel { r: 190, g: 190, b: 190 },
    tab_hover_bg:      Pixel { r: 35,  g: 35,  b: 35  },
    tab_close_hover:   Pixel { r: 50,  g: 50,  b: 50  },

    address_bar_bg:            Pixel { r: 28,  g: 28,  b: 28  },
    address_bar_bg_focused:    Pixel { r: 24,  g: 24,  b: 24  },
    address_bar_fg:            Pixel { r: 175, g: 175, b: 175 },
    address_bar_border:        Pixel { r: 40,  g: 40,  b: 40  },
    address_bar_border_focused: Pixel { r: 120, g: 120, b: 120 }, // white-ish

    icon_fg:          Pixel { r: 120, g: 120, b: 120 },
    control_hover_bg: Pixel { r: 35,  g: 35,  b: 35  },

    new_tab_card_bg:       Pixel { r: 24,  g: 24,  b: 24  },
    new_tab_card_hover_bg: Pixel { r: 36,  g: 36,  b: 36  },

    accent:       Pixel { r: 110, g: 110, b: 110 }, // pure grey accent
    accent_fg:    Pixel { r: 220, g: 220, b: 220 },
    security_ok:  Pixel { r: 80,  g: 140, b: 80  },
    security_err: Pixel { r: 150, g: 70,  b: 70  },
};

pub const LIGHT_CLEAN: Theme = Theme {
    bg:           Pixel { r: 248, g: 248, b: 248 }, // #F8F8F8
    fg:           Pixel { r: 16,  g: 16,  b: 16  }, // #101010
    fg_secondary: Pixel { r: 140, g: 140, b: 140 }, // #8C8C8C
    border:       Pixel { r: 216, g: 216, b: 216 }, // #D8D8D8
    surface:      Pixel { r: 255, g: 255, b: 255 }, // #FFFFFF
    placeholder:  Pixel { r: 160, g: 160, b: 160 }, // #A0A0A0

    tab_bar_bg:        Pixel { r: 234, g: 234, b: 236 }, // #EAEAEC subtle grey
    tab_bg:            Pixel { r: 218, g: 218, b: 220 }, // inactive tabs
    tab_fg:            Pixel { r: 100, g: 100, b: 100 },
    tab_active_bg:     Pixel { r: 255, g: 255, b: 255 }, // white = active
    tab_active_fg:     Pixel { r: 16,  g: 16,  b: 16  },
    tab_hover_bg:      Pixel { r: 228, g: 228, b: 230 },
    tab_close_hover:   Pixel { r: 200, g: 200, b: 202 },

    address_bar_bg:            Pixel { r: 240, g: 240, b: 242 }, // slightly off-white
    address_bar_bg_focused:    Pixel { r: 255, g: 255, b: 255 },
    address_bar_fg:            Pixel { r: 16,  g: 16,  b: 16  },
    address_bar_border:        Pixel { r: 210, g: 210, b: 212 },
    address_bar_border_focused: Pixel { r: 0,   g: 122, b: 255 }, // iOS blue

    icon_fg:          Pixel { r: 120, g: 120, b: 122 },
    control_hover_bg: Pixel { r: 220, g: 220, b: 222 },

    new_tab_card_bg:       Pixel { r: 242, g: 242, b: 244 },
    new_tab_card_hover_bg: Pixel { r: 228, g: 228, b: 230 },

    accent:       Pixel { r: 0,   g: 122, b: 255 }, // #007AFF
    accent_fg:    Pixel { r: 255, g: 255, b: 255 },
    security_ok:  Pixel { r: 46,  g: 204, b: 113 },
    security_err: Pixel { r: 231, g: 76,  b: 60  },
};

pub fn get_theme(palette: ColorPalette) -> Theme {
    match palette {
        ColorPalette::KamelotDark => KAMELOT_DARK,
        ColorPalette::GraphiteMinimal => GRAPHITE_MINIMAL,
        ColorPalette::LightClean => LIGHT_CLEAN,
    }
}
