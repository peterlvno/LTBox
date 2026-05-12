//! Material 3 Expressive color system — indigo-seed tonal palettes.
//!
//! Roles per m3.material.io/styles/color/roles. All hand-picked colors
//! go through [`Palette`] so light/dark + re-theming live in one place.

// Design-system scaffolding: palette roles, state-layer alphas, shape
// and type-scale tokens are kept complete even when a given binding is
// not yet referenced, so future UI work can pull from a stable surface.
#![allow(dead_code)]

use iced::{Color, color};

/// Semantic color slots per Material 3.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub primary: Color,
    pub on_primary: Color,
    pub primary_container: Color,
    pub on_primary_container: Color,

    pub secondary: Color,
    pub on_secondary: Color,
    pub secondary_container: Color,
    pub on_secondary_container: Color,

    pub tertiary: Color,
    pub on_tertiary: Color,
    pub tertiary_container: Color,
    pub on_tertiary_container: Color,

    pub error: Color,
    pub on_error: Color,
    pub error_container: Color,
    pub on_error_container: Color,

    /// Success — M3 doesn't ship this; tonal family of tertiary green.
    pub success: Color,
    pub warning: Color,

    pub background: Color,
    pub on_background: Color,

    pub surface: Color,
    pub surface_dim: Color,
    pub surface_bright: Color,
    pub surface_container_lowest: Color,
    pub surface_container_low: Color,
    pub surface_container: Color,
    pub surface_container_high: Color,
    pub surface_container_highest: Color,
    pub on_surface: Color,
    pub on_surface_variant: Color,

    pub outline: Color,
    pub outline_variant: Color,

    pub scrim: Color,
    pub shadow: Color,
}

/// Light palette — indigo primary, neutral surfaces.
pub const LIGHT: Palette = Palette {
    primary: color!(0x465AAA),
    on_primary: color!(0xFFFFFF),
    primary_container: color!(0xDDE1FF),
    on_primary_container: color!(0x001A43),

    secondary: color!(0x5B5D72),
    on_secondary: color!(0xFFFFFF),
    secondary_container: color!(0xE0E1F9),
    on_secondary_container: color!(0x181A2C),

    tertiary: color!(0x76546F),
    on_tertiary: color!(0xFFFFFF),
    tertiary_container: color!(0xFFD7F5),
    on_tertiary_container: color!(0x2C1229),

    error: color!(0xBA1A1A),
    on_error: color!(0xFFFFFF),
    error_container: color!(0xFFDAD6),
    on_error_container: color!(0x410002),

    success: color!(0x216C2A),
    warning: color!(0xE6A000),

    background: color!(0xFBF8FD),
    on_background: color!(0x1B1B21),

    surface: color!(0xFBF8FD),
    surface_dim: color!(0xDBD9E0),
    surface_bright: color!(0xFBF8FD),
    surface_container_lowest: color!(0xFFFFFF),
    surface_container_low: color!(0xF5F2F7),
    surface_container: color!(0xEFECF1),
    surface_container_high: color!(0xE9E7EB),
    surface_container_highest: color!(0xE3E1E6),
    on_surface: color!(0x1B1B21),
    on_surface_variant: color!(0x47464F),

    outline: color!(0x77767F),
    outline_variant: color!(0xC7C5D0),

    scrim: color!(0x000000),
    shadow: color!(0x000000),
};

/// Dark palette — LIGHT shifted along the M3 tonal scale.
pub const DARK: Palette = Palette {
    primary: color!(0xB5C4FF),
    on_primary: color!(0x152F64),
    primary_container: color!(0x2C4379),
    on_primary_container: color!(0xDDE1FF),

    secondary: color!(0xC4C5DD),
    on_secondary: color!(0x2D2F42),
    secondary_container: color!(0x434559),
    on_secondary_container: color!(0xE0E1F9),

    tertiary: color!(0xE5BAD8),
    on_tertiary: color!(0x44263F),
    tertiary_container: color!(0x5C3D56),
    on_tertiary_container: color!(0xFFD7F5),

    error: color!(0xFFB4AB),
    on_error: color!(0x690005),
    error_container: color!(0x93000A),
    on_error_container: color!(0xFFDAD6),

    success: color!(0x8ADA95),
    warning: color!(0xF5BE4B),

    background: color!(0x131318),
    on_background: color!(0xE4E1E9),

    surface: color!(0x131318),
    surface_dim: color!(0x131318),
    surface_bright: color!(0x3A393F),
    surface_container_lowest: color!(0x0E0E13),
    surface_container_low: color!(0x1B1B21),
    surface_container: color!(0x201F26),
    surface_container_high: color!(0x2A2930),
    surface_container_highest: color!(0x35343B),
    on_surface: color!(0xE4E1E9),
    on_surface_variant: color!(0xC7C5D0),

    outline: color!(0x918F99),
    outline_variant: color!(0x47464F),

    scrim: color!(0x000000),
    shadow: color!(0x000000),
};

/// Active palette for the current dark-mode flag.
pub const fn palette(dark_mode: bool) -> &'static Palette {
    if dark_mode { &DARK } else { &LIGHT }
}

/// Overlay a color with alpha — used for M3 state layers.
pub const fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

/// M3 state-layer alphas.
pub mod state {
    pub const HOVER: f32 = 0.08;
    pub const FOCUS: f32 = 0.10;
    pub const PRESSED: f32 = 0.12;
    pub const DRAGGED: f32 = 0.16;
}

/// M3 motion tokens — easing curves (cubic-bezier control points) and
/// duration tokens (milliseconds). Spring-driven animations (sidebar
/// rail) stay on their physical model; these are reference values for
/// linear-interpolated tweens (popup fade, toast slide, page transition).
pub mod motion {
    /// `cubic-bezier(x1, y1, x2, y2)` — outer control points.
    pub type Easing = (f32, f32, f32, f32);

    // Emphasized — primary easing for incoming/outgoing content,
    // navigation, and other large layout changes.
    pub const EMPHASIZED: Easing = (0.2, 0.0, 0.0, 1.0);
    pub const EMPHASIZED_DECELERATE: Easing = (0.05, 0.7, 0.1, 1.0);
    pub const EMPHASIZED_ACCELERATE: Easing = (0.3, 0.0, 0.8, 0.15);
    // Standard — secondary easing for small UI elements and state
    // changes that should feel routine rather than emphasized.
    pub const STANDARD: Easing = (0.2, 0.0, 0.0, 1.0);
    pub const STANDARD_DECELERATE: Easing = (0.0, 0.0, 0.0, 1.0);
    pub const STANDARD_ACCELERATE: Easing = (0.3, 0.0, 1.0, 1.0);
    pub const LINEAR: Easing = (0.0, 0.0, 1.0, 1.0);

    // Duration tokens (ms). M3 groups durations into short / medium /
    // long / extra long; pick by the magnitude of the layout change.
    pub const SHORT_1: u32 = 50;
    pub const SHORT_2: u32 = 100;
    pub const SHORT_3: u32 = 150;
    pub const SHORT_4: u32 = 200;
    pub const MEDIUM_1: u32 = 250;
    pub const MEDIUM_2: u32 = 300;
    pub const MEDIUM_3: u32 = 350;
    pub const MEDIUM_4: u32 = 400;
    pub const LONG_1: u32 = 450;
    pub const LONG_2: u32 = 500;
    pub const LONG_3: u32 = 550;
    pub const LONG_4: u32 = 600;
    pub const EXTRA_LONG_1: u32 = 700;
    pub const EXTRA_LONG_2: u32 = 800;
    pub const EXTRA_LONG_3: u32 = 900;
    pub const EXTRA_LONG_4: u32 = 1000;

    /// Parametric evaluation of a cubic Bézier at `t in [0, 1]`. This
    /// returns `y(t)` for parameter `t`, not `y(x)` (CSS easing form).
    /// For animation tweens both forms read close enough since the
    /// curves are monotonic; if exact CSS parity matters, invert via
    /// Newton's method on `x` first.
    pub fn eval(curve: Easing, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        let (_, p1y, _, p2y) = curve;
        let u = 1.0 - t;
        3.0 * u * u * t * p1y + 3.0 * u * t * t * p2y + t * t * t
    }
}

/// M3 shape scale (corner radius in px). Expressive uses rounder
/// corners than baseline M3.
pub mod shape {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const FULL: f32 = 9999.0;
}

/// M3 type scale (font size in px).
pub mod text_size {
    pub const DISPLAY_LARGE: f32 = 57.0;
    pub const DISPLAY_MEDIUM: f32 = 45.0;
    pub const DISPLAY_SMALL: f32 = 36.0;
    pub const HEADLINE_LARGE: f32 = 32.0;
    pub const HEADLINE_MEDIUM: f32 = 28.0;
    pub const HEADLINE_SMALL: f32 = 24.0;
    pub const TITLE_LARGE: f32 = 22.0;
    pub const TITLE_MEDIUM: f32 = 16.0;
    pub const TITLE_SMALL: f32 = 14.0;
    pub const BODY_LARGE: f32 = 16.0;
    pub const BODY_MEDIUM: f32 = 14.0;
    pub const BODY_SMALL: f32 = 12.0;
    pub const LABEL_LARGE: f32 = 14.0;
    pub const LABEL_MEDIUM: f32 = 12.0;
    pub const LABEL_SMALL: f32 = 11.0;
    /// Tighter than HEADLINE_SMALL. Not a formal M3 token.
    pub const WIZARD_STEP_TITLE: f32 = 20.0;
}

/// Which palette surface container the card fills with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceLevel {
    /// `surface_container_low` — sidebar, subtle secondary panels.
    Low,
    /// `surface_container` — default card surface.
    Default,
    /// `surface_container_high` — raised dialogs / popovers.
    High,
    /// `surface_container_highest` — topmost modal sheets.
    Highest,
    /// `surface_container_lowest` — disabled rescue card / log panels.
    Lowest,
}

impl SurfaceLevel {
    fn bg(self, p: &Palette) -> iced::Color {
        match self {
            Self::Lowest => p.surface_container_lowest,
            Self::Low => p.surface_container_low,
            Self::Default => p.surface_container,
            Self::High => p.surface_container_high,
            Self::Highest => p.surface_container_highest,
        }
    }
}

/// Shared M3 card/panel container style. `radius` + `elevation_level`
/// are theme-reactive when relevant.
pub fn surface_card_style(
    t: &iced::Theme,
    level: SurfaceLevel,
    radius: f32,
    elevation_level: u8,
) -> iced::widget::container::Style {
    use iced::widget::container;
    let dark = t.palette().background.r < 0.5;
    let p = if dark { &DARK } else { &LIGHT };
    container::Style {
        background: Some(level.bg(p).into()),
        border: iced::Border {
            color: p.outline_variant,
            width: 1.0,
            radius: radius.into(),
        },
        shadow: elevation(elevation_level, dark),
        ..Default::default()
    }
}

/// M3 elevation → `iced::Shadow`. `0` = none, `5` = modal-dialog.
pub fn elevation(level: u8, dark_mode: bool) -> iced::Shadow {
    use iced::{Color, Shadow, Vector};
    let shadow_color = if dark_mode {
        Color::from_rgba(0.0, 0.0, 0.0, 0.6)
    } else {
        Color::from_rgba(0.0, 0.0, 0.0, 0.15)
    };
    match level {
        0 => Shadow {
            color: Color::TRANSPARENT,
            offset: Vector::ZERO,
            blur_radius: 0.0,
        },
        1 => Shadow {
            color: shadow_color,
            offset: Vector::new(0.0, 1.0),
            blur_radius: 3.0,
        },
        2 => Shadow {
            color: shadow_color,
            offset: Vector::new(0.0, 2.0),
            blur_radius: 6.0,
        },
        3 => Shadow {
            color: shadow_color,
            offset: Vector::new(0.0, 4.0),
            blur_radius: 8.0,
        },
        4 => Shadow {
            color: shadow_color,
            offset: Vector::new(0.0, 6.0),
            blur_radius: 10.0,
        },
        _ => Shadow {
            color: shadow_color,
            offset: Vector::new(0.0, 8.0),
            blur_radius: 12.0,
        },
    }
}
