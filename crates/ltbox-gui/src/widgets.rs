//! Wizard navigation bars and small view widgets/helpers (nav buttons,
//! color blend/easing, device portrait, layout consts). Extracted from main.rs.

use crate::*;
use iced::widget::{Space, button, column, container, row, text};
use iced::{Element, Length, Theme};

/// True for the localized "Start" / "Dump" labels — the primary-action button
/// shown only on a wizard's confirm/start screen (intermediate steps use
/// "Next" / "Scan"). Drives the red Cancel button in the footer helpers.
pub(crate) fn is_start_label(label: &str) -> bool {
    label == ltbox_core::i18n::tr("btn_start").as_str()
        || label == ltbox_core::i18n::tr("btn_dump").as_str()
}

pub(crate) fn wizard_nav<'a>(
    can_back: bool,
    next_label: &str,
    can_next: bool,
    back_label: &str,
) -> Element<'a, Message> {
    let mut r = row![].spacing(8).padding([12, 24]);

    if can_back {
        r = r.push(
            button(text(back_label.to_string()).size(13))
                .on_press(Message::Root(RootMsg::RootBack))
                .padding([10, 20])
                .style(md_text_btn_style),
        );
    }

    r = r.push(Space::new().width(Length::Fill));

    // Red "Cancel" on the confirm/start step → StartOver (see
    // `wizard_nav_generic` for the M3 placement rationale).
    if is_start_label(next_label) {
        r = r.push(
            button(text(ltbox_core::i18n::tr("btn_cancel")).size(13))
                .on_press(Message::StartOver)
                .padding([10, 20])
                .style(md_error_text_btn_style),
        );
    }

    let next_btn = button(text(next_label.to_string()).size(13))
        .padding([10, 24])
        .style(md_filled_btn_style);

    r = r.push(if can_next {
        next_btn.on_press(Message::Root(RootMsg::RootNext))
    } else {
        next_btn
    });

    // Top-edge divider only — bottom + sides come from the window
    // outline / sidebar rule, so a 4-side border here would render
    // the top + left as a double line.
    column![
        widget::rule::horizontal(1).style(shell_rule_style),
        container(r)
            .width(Length::Fill)
            .style(|t: &Theme| panel_bg(t)),
    ]
    .into()
}

/// Linear mix of two colors by `t` ∈ [0, 1].
pub(crate) fn blend(base: iced::Color, overlay: iced::Color, t: f32) -> iced::Color {
    let t = t.clamp(0.0, 1.0);
    iced::Color {
        r: base.r * (1.0 - t) + overlay.r * t,
        g: base.g * (1.0 - t) + overlay.g * t,
        b: base.b * (1.0 - t) + overlay.b * t,
        a: base.a,
    }
}

// =========================================================================
// Reusable widgets
// =========================================================================

/// Section header. Renders the label when `expanded` is `true`,
/// otherwise an invisible spacer at the same fixed height — keeps
/// the nav column from re-flowing vertically as the sidebar tween
/// crosses its midpoint.
pub(crate) const SEC_HDR_HEIGHT: f32 = 36.0;

/// Cubic ease-out curve `f(t) = 1 - (1 - t)^3`, mapped to `[0, 1]`.
/// Used by the sidebar tween so labels fade in faster early and
/// settle smoothly near the spring's resting point.
pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Pinned nav button height — matches the expanded label form so
/// the sidebar tween's mid-frame swap between icon-only and
/// label content doesn't push every row vertically.
pub(crate) const NAV_BTN_HEIGHT: f32 = 38.0;

/// Collapsed sidebar rail width (icon-only). The main row reserves
/// exactly this much space so the content area never reflows when the
/// sidebar tweens — the expanded form floats over content via a
/// `Stack` overlay.
pub(crate) const SIDEBAR_RAIL_WIDTH: f32 = 64.0;
pub(crate) const SIDEBAR_EXPANDED_WIDTH: f32 = 210.0;

pub(crate) fn nav_btn<'a>(
    view: View,
    label: &str,
    active: bool,
    enabled: bool,
    label_alpha: f32,
) -> Element<'a, Message> {
    // M3 active indicator pill: a 32x28 `secondary_container` chip
    // wraps the icon when the item is selected. Replaces the older
    // "tint the whole button" treatment so the active marker stays
    // anchored to the icon and the label stays readable in its
    // standard `on_surface` color.
    let icon = lucide_icon(view.nav_icon(), 18.0, move |t: &Theme| {
        let p = pal_of(t);
        if !enabled {
            with_alpha(p.on_surface, 0.38)
        } else if active {
            p.on_secondary_container
        } else {
            p.on_surface_variant
        }
    });
    let icon_pill: Element<'a, Message> = container(icon)
        .width(Length::Fixed(32.0))
        .height(Length::Fixed(28.0))
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center)
        .style(move |t: &Theme| {
            if active && enabled {
                iced::widget::container::Style {
                    background: Some(pal_of(t).secondary_container.into()),
                    border: iced::Border {
                        radius: theme::shape::FULL.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            } else {
                iced::widget::container::Style::default()
            }
        })
        .into();

    // Single base layout in both modes: icon left-anchored + optional
    // label. Keeping the icon's horizontal position constant across
    // modes means it does not jump from "centered in 64 px shell"
    // to "left-padded next to label" the moment the label mounts.
    // Outer padding shrinks from 22 → 15 (= 22 - (32-18)/2) so the
    // pill's geometric center sits at the same x as the bare icon
    // did before, avoiding a horizontal shift the moment a row
    // becomes active.
    let mut inner = iced::widget::row![icon_pill]
        .spacing(8)
        .align_y(iced::Alignment::Center);
    if label_alpha > 0.0 {
        // Resolve the base text color (hover / disabled apply via the
        // button style below; here we just fade the label in along
        // the spring), then re-apply alpha so the glyph fades in step
        // with the sidebar width tween. M3 nav rail uses `on_surface`
        // for both active and inactive labels in the expanded form —
        // the pill carries the emphasis, the label stays uniform.
        let alpha = label_alpha;
        let base_label_color = move |t: &Theme| -> iced::Color {
            let p = pal_of(t);
            if !enabled {
                with_alpha(p.on_surface, 0.38)
            } else {
                p.on_surface
            }
        };
        inner = inner.push(
            text(label.to_string())
                .size(13)
                .height(Length::Fill)
                .align_y(iced::alignment::Vertical::Center)
                // Forbid wrapping: during the sidebar spring there is
                // a brief window where the panel is wide enough to
                // mount the label but too narrow for long glyphs to
                // fit on one line. Wrapping into 2 rows mid-tween then
                // collapsing back to 1 row reads as a jank flicker.
                // No-wrap lets the text overflow under the panel's
                // clip rect instead — invisible until width settles.
                .wrapping(iced::widget::text::Wrapping::None)
                .style(move |t: &Theme| iced::widget::text::Style {
                    color: Some(with_alpha(base_label_color(t), alpha)),
                }),
        );
    }
    let content: Element<'a, Message> = container(inner)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(iced::Alignment::Center)
        .into();

    // Outer padding 15px on each side: the 32-wide pill centered in
    // a 62-wide content box places the pill (and the 18px icon inside)
    // at the same on-screen x as the 18px icon at padding 22 used to.
    // Vertical padding stays 0; height is fixed by NAV_BTN_HEIGHT.
    let btn = button(content)
        .padding([0, 15])
        .width(Length::Fill)
        .height(Length::Fixed(NAV_BTN_HEIGHT))
        .style(move |t: &Theme, status| {
            let p = pal_of(t);
            if !enabled {
                return button::Style {
                    background: None,
                    text_color: with_alpha(p.on_surface, 0.38),
                    ..Default::default()
                };
            }
            // State layers per M3: hover 8%, pressed 12%. The spec
            // does NOT add a persistent row tint to active items —
            // the indicator pill (secondary_container chip around the
            // icon) is the sole "selected" marker, and state layers
            // only stack on top during interaction. Idle active items
            // therefore get no row background.
            let bg = theme::state_layer_bg(status, p.on_surface).map(|c| c.into());
            button::Style {
                background: bg,
                text_color: if active {
                    p.on_surface
                } else {
                    p.on_surface_variant
                },
                ..Default::default()
            }
        });
    if enabled {
        btn.on_press(Message::Navigate(view)).into()
    } else {
        btn.into()
    }
}

// Device portrait handles — built once, cloned each render.
// Unknown models fall through to `GENERIC_TABLET_SVG_HANDLE`.
static TB320FC_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb320fc.png").as_slice(),
        )
    });
static TB321FU_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb321fu.png").as_slice(),
        )
    });
static TB322FC_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb322fc.png").as_slice(),
        )
    });
static TB323FU_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb323fu.png").as_slice(),
        )
    });
static TB520FU_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb520fu.png").as_slice(),
        )
    });
static TB710FU_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../assets/devices/tb710fu.png").as_slice(),
        )
    });
static GENERIC_TABLET_SVG_HANDLE: std::sync::LazyLock<iced::widget::svg::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::svg::Handle::from_memory(
            include_bytes!("../assets/devices/generic_tablet.svg").as_slice(),
        )
    });

/// Asset for the Dashboard portrait slot.
pub(crate) enum DevicePortrait {
    Png(iced::widget::image::Handle),
    Svg(iced::widget::svg::Handle),
}

pub(crate) fn device_portrait(model: &str) -> DevicePortrait {
    match model.to_uppercase().as_str() {
        "TB320FC" => DevicePortrait::Png(TB320FC_HANDLE.clone()),
        "TB321FU" => DevicePortrait::Png(TB321FU_HANDLE.clone()),
        "TB322FC" => DevicePortrait::Png(TB322FC_HANDLE.clone()),
        "TB323FU" => DevicePortrait::Png(TB323FU_HANDLE.clone()),
        "TB520FU" => DevicePortrait::Png(TB520FU_HANDLE.clone()),
        "TB710FU" => DevicePortrait::Png(TB710FU_HANDLE.clone()),
        _ => DevicePortrait::Svg(GENERIC_TABLET_SVG_HANDLE.clone()),
    }
}

pub(crate) const WIZARD_CARD_HEIGHT: f32 = 180.0;

/// Side length for the square (1:1) option cards used by single-row wizard
/// steps. Sized so a 3-up row still fits within the minimum window width.
pub(crate) const WIZARD_CARD_SQUARE: f32 = 200.0;

/// Fixed sub-row height (~2 lines at size 11) so cards line up across
/// translations.
pub(crate) const SUB_ROW_HEIGHT: f32 = 32.0;

/// Taller sub-row for the narrower square cards (~4 lines at size 11) so
/// longer localized descriptions wrap without clipping. Fits within the
/// square card's vertical slack below the icon + label.
pub(crate) const WIZARD_CARD_SQUARE_SUB_HEIGHT: f32 = 60.0;

pub(crate) const FLASH_PARTS_MARKER_CELL_WIDTH: f32 = 32.0;
pub(crate) const FLASH_PARTS_MARKER_CELL_HEIGHT: f32 = 20.0;
pub(crate) const FLASH_PARTS_MARKER_SIZE: f32 = 16.0;
pub(crate) const FLASH_PARTS_ERASE_DASH_WIDTH: f32 = 9.0;
pub(crate) const FLASH_PARTS_ERASE_DASH_HEIGHT: f32 = 2.0;

pub(crate) fn wizard_nav_generic<'a>(
    can_back: bool,
    next_label: &str,
    can_next: bool,
    back_label: &str,
    back_msg: Message,
    next_msg: Message,
) -> Element<'a, Message> {
    wizard_nav_generic_with_disabled_next_tooltip(
        can_back, next_label, can_next, None, back_label, back_msg, next_msg,
    )
}

pub(crate) fn wizard_nav_generic_with_disabled_next_tooltip<'a>(
    can_back: bool,
    next_label: &str,
    can_next: bool,
    disabled_next_hint: Option<String>,
    back_label: &str,
    back_msg: Message,
    next_msg: Message,
) -> Element<'a, Message> {
    let mut r = row![].spacing(8).padding([12, 24]);
    if can_back {
        r = r.push(
            button(text(back_label.to_string()).size(13))
                .on_press(back_msg)
                .padding([10, 20])
                .style(md_text_btn_style),
        );
    }
    r = r.push(Space::new().width(Length::Fill));
    // Red "Cancel" on the confirm/start step → StartOver, returning the menu
    // to its beginning. M3: the cancel/start decision pair sits at the
    // trailing edge, navigation (Back) at the leading edge; one filled button
    // (Start), destructive action in the error color.
    if is_start_label(next_label) {
        r = r.push(
            button(text(ltbox_core::i18n::tr("btn_cancel")).size(13))
                .on_press(Message::StartOver)
                .padding([10, 20])
                .style(md_error_text_btn_style),
        );
    }
    let next_btn = button(text(next_label.to_string()).size(13))
        .padding([10, 24])
        .style(md_filled_btn_style);
    let next: Element<'_, Message> = if can_next {
        next_btn.on_press(next_msg).into()
    } else if let Some(hint) = disabled_next_hint {
        disabled_reason_tooltip(next_btn.into(), hint)
    } else {
        next_btn.into()
    };
    r = r.push(next);
    // Top-edge divider only — bottom + sides come from the window
    // outline / sidebar rule.
    column![
        widget::rule::horizontal(1).style(shell_rule_style),
        container(r)
            .width(Length::Fill)
            .style(|t: &Theme| panel_bg(t)),
    ]
    .into()
}
