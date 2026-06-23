//! Material 3 Expressive color system — indigo-seed tonal palettes.
//!
//! Roles per m3.material.io/styles/color/roles. All hand-picked colors
//! go through [`Palette`] so light/dark + re-theming live in one place.

// Design-system scaffolding: palette roles, state-layer alphas, shape
// and type-scale tokens are kept complete even when a given binding is
// not yet referenced, so future UI work can pull from a stable surface.
#![allow(dead_code)]

use iced::{Color, color};
use std::sync::RwLock;

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
    pub warning_container: Color,
    pub on_warning_container: Color,

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
    warning: color!(0x735B00),
    warning_container: color!(0xFFF0C2),
    on_warning_container: color!(0x241A00),

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
    warning_container: color!(0x5A4300),
    on_warning_container: color!(0xFFDFA3),

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

/// Teal seed palette, generated to the same role structure as the indigo base.
pub const TEAL_LIGHT: Palette = Palette {
    primary: color!(0x006A6A),
    on_primary: color!(0xFFFFFF),
    primary_container: color!(0x9CF1EF),
    on_primary_container: color!(0x002020),

    secondary: color!(0x4A6363),
    on_secondary: color!(0xFFFFFF),
    secondary_container: color!(0xCCE8E7),
    on_secondary_container: color!(0x051F1F),

    tertiary: color!(0x4B607C),
    on_tertiary: color!(0xFFFFFF),
    tertiary_container: color!(0xD3E4FF),
    on_tertiary_container: color!(0x041C35),

    error: LIGHT.error,
    on_error: LIGHT.on_error,
    error_container: LIGHT.error_container,
    on_error_container: LIGHT.on_error_container,

    success: LIGHT.success,
    warning: LIGHT.warning,
    warning_container: LIGHT.warning_container,
    on_warning_container: LIGHT.on_warning_container,

    background: color!(0xF7FAF9),
    on_background: LIGHT.on_background,

    surface: color!(0xF7FAF9),
    surface_dim: color!(0xD7DBDA),
    surface_bright: color!(0xF7FAF9),
    surface_container_lowest: color!(0xFFFFFF),
    surface_container_low: color!(0xF0F4F3),
    surface_container: color!(0xEAEEED),
    surface_container_high: color!(0xE4E8E7),
    surface_container_highest: color!(0xDEE2E1),
    on_surface: LIGHT.on_surface,
    on_surface_variant: color!(0x3F4948),

    outline: color!(0x6F7978),
    outline_variant: color!(0xBFC9C8),

    scrim: color!(0x000000),
    shadow: color!(0x000000),
};

pub const TEAL_DARK: Palette = Palette {
    primary: color!(0x80D5D3),
    on_primary: color!(0x003737),
    primary_container: color!(0x004F4F),
    on_primary_container: color!(0x9CF1EF),

    secondary: color!(0xB0CCCB),
    on_secondary: color!(0x1B3534),
    secondary_container: color!(0x324B4A),
    on_secondary_container: color!(0xCCE8E7),

    tertiary: color!(0xB3C8E9),
    on_tertiary: color!(0x1C314C),
    tertiary_container: color!(0x334865),
    on_tertiary_container: color!(0xD3E4FF),

    error: DARK.error,
    on_error: DARK.on_error,
    error_container: DARK.error_container,
    on_error_container: DARK.on_error_container,

    success: DARK.success,
    warning: DARK.warning,
    warning_container: DARK.warning_container,
    on_warning_container: DARK.on_warning_container,

    background: color!(0x111414),
    on_background: DARK.on_background,

    surface: color!(0x111414),
    surface_dim: color!(0x111414),
    surface_bright: color!(0x363A39),
    surface_container_lowest: color!(0x0C0F0F),
    surface_container_low: color!(0x191C1C),
    surface_container: color!(0x1D2020),
    surface_container_high: color!(0x272B2A),
    surface_container_highest: color!(0x323535),
    on_surface: DARK.on_surface,
    on_surface_variant: color!(0xBFC9C8),

    outline: color!(0x899392),
    outline_variant: color!(0x3F4948),

    scrim: color!(0x000000),
    shadow: color!(0x000000),
};

/// Rose seed palette for users who want a warmer accent family.
pub const ROSE_LIGHT: Palette = Palette {
    primary: color!(0x984061),
    on_primary: color!(0xFFFFFF),
    primary_container: color!(0xFFD9E3),
    on_primary_container: color!(0x3E001D),

    secondary: color!(0x74565F),
    on_secondary: color!(0xFFFFFF),
    secondary_container: color!(0xFFD9E3),
    on_secondary_container: color!(0x2B151D),

    tertiary: color!(0x7D5635),
    on_tertiary: color!(0xFFFFFF),
    tertiary_container: color!(0xFFDCC2),
    on_tertiary_container: color!(0x301400),

    error: LIGHT.error,
    on_error: LIGHT.on_error,
    error_container: LIGHT.error_container,
    on_error_container: LIGHT.on_error_container,

    success: LIGHT.success,
    warning: LIGHT.warning,
    warning_container: LIGHT.warning_container,
    on_warning_container: LIGHT.on_warning_container,

    background: color!(0xFFFBFF),
    on_background: LIGHT.on_background,

    surface: color!(0xFFFBFF),
    surface_dim: color!(0xE5D7DC),
    surface_bright: color!(0xFFFBFF),
    surface_container_lowest: color!(0xFFFFFF),
    surface_container_low: color!(0xFCF0F4),
    surface_container: color!(0xF6EAEE),
    surface_container_high: color!(0xF0E4E8),
    surface_container_highest: color!(0xEADFE3),
    on_surface: LIGHT.on_surface,
    on_surface_variant: color!(0x514349),

    outline: color!(0x82737A),
    outline_variant: color!(0xD4C2C8),

    scrim: color!(0x000000),
    shadow: color!(0x000000),
};

pub const ROSE_DARK: Palette = Palette {
    primary: color!(0xFFB1C8),
    on_primary: color!(0x5E1134),
    primary_container: color!(0x7B2949),
    on_primary_container: color!(0xFFD9E3),

    secondary: color!(0xE3BDC8),
    on_secondary: color!(0x422A33),
    secondary_container: color!(0x5A3F49),
    on_secondary_container: color!(0xFFD9E3),

    tertiary: color!(0xF2BD91),
    on_tertiary: color!(0x49290D),
    tertiary_container: color!(0x633F20),
    on_tertiary_container: color!(0xFFDCC2),

    error: DARK.error,
    on_error: DARK.on_error,
    error_container: DARK.error_container,
    on_error_container: DARK.on_error_container,

    success: DARK.success,
    warning: DARK.warning,
    warning_container: DARK.warning_container,
    on_warning_container: DARK.on_warning_container,

    background: color!(0x171216),
    on_background: DARK.on_background,

    surface: color!(0x171216),
    surface_dim: color!(0x171216),
    surface_bright: color!(0x3F373B),
    surface_container_lowest: color!(0x120D10),
    surface_container_low: color!(0x211A1E),
    surface_container: color!(0x261E23),
    surface_container_high: color!(0x30282D),
    surface_container_highest: color!(0x3B3337),
    on_surface: DARK.on_surface,
    on_surface_variant: color!(0xD4C2C8),

    outline: color!(0x9C8D93),
    outline_variant: color!(0x514349),

    scrim: color!(0x000000),
    shadow: color!(0x000000),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeSeed {
    #[default]
    Indigo,
    Teal,
    Rose,
}

impl ThemeSeed {
    pub const ALL: [Self; 3] = [Self::Indigo, Self::Teal, Self::Rose];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Indigo => "theme_seed_indigo",
            Self::Teal => "theme_seed_teal",
            Self::Rose => "theme_seed_rose",
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::Indigo => "indigo",
            Self::Teal => "teal",
            Self::Rose => "rose",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "indigo" => Some(Self::Indigo),
            "teal" => Some(Self::Teal),
            "rose" => Some(Self::Rose),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RuntimeTheme {
    seed: ThemeSeed,
    dark_mode: bool,
}

static RUNTIME_THEME: RwLock<RuntimeTheme> = RwLock::new(RuntimeTheme {
    seed: ThemeSeed::Indigo,
    dark_mode: false,
});

pub fn set_runtime_theme(seed: ThemeSeed, dark_mode: bool) {
    if let Ok(mut runtime) = RUNTIME_THEME.write() {
        *runtime = RuntimeTheme { seed, dark_mode };
    }
}

fn runtime_theme() -> RuntimeTheme {
    RUNTIME_THEME.read().map_or(
        RuntimeTheme {
            seed: ThemeSeed::Indigo,
            dark_mode: false,
        },
        |runtime| *runtime,
    )
}

/// Current runtime dark-mode flag (kept in sync by `sync_runtime_theme`). Lets
/// view code pick light/dark assets without an `&iced::Theme` in hand.
pub fn runtime_dark() -> bool {
    runtime_theme().dark_mode
}

/// Active palette for the current dark-mode flag.
pub const fn palette(dark_mode: bool) -> &'static Palette {
    if dark_mode { &DARK } else { &LIGHT }
}

pub fn palette_for(seed: ThemeSeed, dark_mode: bool) -> Palette {
    match (seed, dark_mode) {
        (ThemeSeed::Indigo, false) => LIGHT,
        (ThemeSeed::Indigo, true) => DARK,
        (ThemeSeed::Teal, false) => TEAL_LIGHT,
        (ThemeSeed::Teal, true) => TEAL_DARK,
        (ThemeSeed::Rose, false) => ROSE_LIGHT,
        (ThemeSeed::Rose, true) => ROSE_DARK,
    }
}

pub fn active_palette() -> Palette {
    let runtime = runtime_theme();
    palette_for(runtime.seed, runtime.dark_mode)
}

pub fn active_palette_for(t: &iced::Theme) -> Palette {
    let runtime = runtime_theme();
    palette_for(runtime.seed, is_dark(t))
}

pub fn iced_palette(seed: ThemeSeed, dark_mode: bool) -> iced::theme::Palette {
    let p = palette_for(seed, dark_mode);
    iced::theme::Palette {
        background: p.background,
        text: p.on_surface,
        primary: p.primary,
        success: p.success,
        warning: p.warning,
        danger: p.error,
    }
}

/// Probe `iced::Theme` for the active mode. We don't store a flag on
/// the theme directly, so the heuristic looks at `palette().background`
/// — light backgrounds have a high red channel (M3 surface tones land
/// at `0xFB+` on light), dark ones at `0x13+`. Centralised so both
/// `theme::tooltip_style` (and the rest of this module) and the GUI
/// call sites agree on a single source of truth.
pub fn is_dark(t: &iced::Theme) -> bool {
    t.palette().background.r < 0.5
}

/// Overlay a color with alpha — used for M3 state layers.
pub const fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

/// Blend `overlay` over `base` with the given alpha. Used to flatten
/// an M3 state-layer (translucent on_X color) into a single opaque
/// background tint, since `iced::widget::button::Style::background`
/// only accepts one color/gradient at a time and can't stack a
/// semi-transparent layer over the tonal fill.
pub fn mix_color(base: Color, overlay: Color, alpha: f32) -> Color {
    let inv = 1.0 - alpha;
    Color {
        r: base.r * inv + overlay.r * alpha,
        g: base.g * inv + overlay.g * alpha,
        b: base.b * inv + overlay.b * alpha,
        a: 1.0,
    }
}

/// M3 state-layer alphas.
pub mod state {
    pub const HOVER: f32 = 0.08;
    pub const FOCUS: f32 = 0.10;
    pub const PRESSED: f32 = 0.12;
    pub const DRAGGED: f32 = 0.16;
}

/// M3 state-layer alpha for an `iced::widget::button::Status` — `0.0`
/// when idle, `HOVER` on hover, `PRESSED` while pressed. Centralises
/// the inline `match status { Hovered => HOVER, Pressed => PRESSED,
/// _ => 0.0 }` pattern that was scattered across the GUI's button
/// style closures.
pub fn state_alpha(status: iced::widget::button::Status) -> f32 {
    use iced::widget::button::Status;
    match status {
        Status::Hovered => state::HOVER,
        Status::Pressed => state::PRESSED,
        _ => 0.0,
    }
}

/// Combine [`state_alpha`] with [`with_alpha`] to produce the M3
/// state-layer tint for a button's background overlay. Returns
/// `None` when the button is idle so callers can use
/// `Option<Background>` directly.
///
/// `layer_color` is the M3 "on-X" color of the surface the button
/// sits on (usually `palette.on_surface`).
pub fn state_layer_bg(status: iced::widget::button::Status, layer_color: Color) -> Option<Color> {
    let alpha = state_alpha(status);
    if alpha == 0.0 {
        None
    } else {
        Some(with_alpha(layer_color, alpha))
    }
}

/// Standard M3 tooltip container style — `surface_container_high`
/// background, `outline_variant` 1 px border, level-2 elevation.
/// `radius` lets the caller pick `shape::XS` / `shape::SM` to match
/// the surrounding component scale.
pub fn tooltip_style(t: &iced::Theme, radius: f32) -> iced::widget::container::Style {
    let dark = is_dark(t);
    let p = active_palette_for(t);
    iced::widget::container::Style {
        background: Some(p.surface_container_high.into()),
        text_color: Some(p.on_surface),
        border: iced::Border {
            color: p.outline_variant,
            width: 1.0,
            radius: radius.into(),
        },
        shadow: elevation(2, dark),
        ..Default::default()
    }
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
    let dark = is_dark(t);
    let p = active_palette_for(t);
    container::Style {
        background: Some(level.bg(&p).into()),
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
    // Dark M3 conveys elevation mainly through tonal surface containers, so
    // shadows stay subtle — a gentle ramp by level (0.20..0.36) rather than a
    // flat, heavy 0.6 black. Light theme keeps one soft key shadow.
    let shadow_color = if dark_mode {
        let alpha = 0.20 + 0.04 * f32::from(level.min(5).saturating_sub(1));
        Color::from_rgba(0.0, 0.0, 0.0, alpha)
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
