//! Reusable view components (dialogs, cards, step bar, icon tiles, lucide helpers). Extracted from `main.rs`.

use crate::*;
use iced::widget::{self, Space, button, column, container, row, text};
use iced::{Element, Length, Theme};
use theme::with_alpha;

/// Centered M3 dialog card on a scrim. Inner owns padding/width. MODAL: the
/// whole layer is wrapped in `opaque`, so it captures every pointer event and
/// nothing behind it reacts. Use for confirm dialogs (reboot, country,
/// region-target, rescue, root prompts) that must block the panel behind them.
pub(crate) fn m3_dialog(inner: Element<'_, Message>) -> Element<'_, Message> {
    // `opaque` makes the whole dialog layer capture every pointer event, so
    // hover/click can't fall through the parent `Stack` to the panel behind it
    // (the scrim alone only paints — it doesn't block). The card's own buttons
    // still receive their clicks.
    iced::widget::opaque(m3_dialog_layers(inner))
}

/// Like [`m3_dialog`] but MODELESS: no `opaque` wrapper, so pointer events fall
/// through the scrim to the panel behind. Use for the busy progress dialog — a
/// long-running flash must not trap the user; the sidebar (and current view)
/// stay clickable so they can navigate back to the op's progress screen.
pub(crate) fn m3_dialog_modeless(inner: Element<'_, Message>) -> Element<'_, Message> {
    m3_dialog_layers(inner)
}

/// Shared scrim + centered card layers behind [`m3_dialog`] /
/// [`m3_dialog_modeless`]. The scrim only paints its dim background (in iced a
/// plain `container` does not capture pointer events); modality is decided by
/// the caller wrapping this in `opaque` or not.
fn m3_dialog_layers(inner: Element<'_, Message>) -> Element<'_, Message> {
    let card = container(inner).style(|t: &Theme| {
        let p = pal_of(t);
        container::Style {
            background: Some(p.surface_container.into()),
            border: iced::Border {
                color: p.outline_variant,
                width: 1.0,
                radius: 28.0.into(),
            },
            shadow: iced::Shadow {
                color: with_alpha(p.shadow, 0.3),
                offset: iced::Vector::new(0.0, 8.0),
                blur_radius: 24.0,
            },
            ..Default::default()
        }
    });
    let scrim = container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        // M3 modal scrim: the `scrim` role (black) at 32%, not a hardcoded 45%.
        .style(|t: &Theme| container::Style {
            background: Some(with_alpha(pal_of(t).scrim, 0.32).into()),
            ..Default::default()
        });
    let centered = container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill);
    iced::widget::stack![scrim, centered].into()
}

/// Wrap `inner` (typically a wizard's nav row) in a hover tooltip that explains
/// why its Start button is disabled — e.g. the device isn't on ADB, or the
/// picked EDL loader doesn't fit the connected model. `hint` is the already
/// localized reason text.
pub(crate) fn disabled_reason_tooltip<'a>(
    inner: Element<'a, Message>,
    hint: String,
) -> Element<'a, Message> {
    iced::widget::tooltip(
        inner,
        container(text(hint).size(12))
            .padding([6, 10])
            .style(|t: &Theme| {
                let p = pal_of(t);
                container::Style {
                    background: Some(p.surface_container_high.into()),
                    text_color: Some(p.on_surface),
                    border: iced::Border {
                        color: p.outline_variant,
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                }
            }),
        iced::widget::tooltip::Position::Top,
    )
    .into()
}

pub(crate) fn wizard_step_bar<'a>(steps: &[&str], current: usize) -> Element<'a, Message> {
    let mut r = row![]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .padding([14, 24]);

    for (i, &label) in steps.iter().enumerate() {
        if i > 0 {
            let completed = i <= current;
            r = r.push(container(text("")).width(Length::Fill).height(2).style(
                move |t: &Theme| {
                    let p = pal_of(t);
                    let color = if completed {
                        p.success
                    } else {
                        p.outline_variant
                    };
                    container::Style {
                        background: Some(color.into()),
                        ..Default::default()
                    }
                },
            ));
        }

        let done = i < current;
        let active = i == current;
        let dot_text = if done {
            "\u{2713}".to_string()
        } else {
            (i + 1).to_string()
        };

        let dot = container(text(dot_text).size(12).center().style(move |t: &Theme| {
            let p = pal_of(t);
            let fg = if done || active {
                iced::Color::WHITE
            } else {
                p.on_surface_variant
            };
            iced::widget::text::Style { color: Some(fg) }
        }))
        .width(28)
        .height(28)
        .center_x(28)
        .center_y(28)
        .style(move |t: &Theme| {
            let p = pal_of(t);
            let bg = if done {
                p.success
            } else if active {
                p.primary
            } else {
                p.surface_container_high
            };
            let border_color = if done || active {
                bg
            } else {
                p.outline_variant
            };
            container::Style {
                background: Some(bg.into()),
                border: iced::Border {
                    color: border_color,
                    width: 1.0,
                    radius: 14.0.into(),
                },
                ..Default::default()
            }
        });

        let lbl_widget = text(label.to_string()).size(12).style(move |t: &Theme| {
            let p = pal_of(t);
            let color = if done {
                p.success
            } else if active {
                p.primary
            } else {
                p.on_surface_variant
            };
            iced::widget::text::Style { color: Some(color) }
        });
        r = r.push(
            row![dot, lbl_widget]
                .spacing(6)
                .align_y(iced::Alignment::Center),
        );
    }

    // Bottom-edge divider only — top + sides come from the
    // surrounding window outline / sidebar rule, so a 4-side
    // border here would render the bottom + left as a double line.
    column![
        container(r)
            .width(Length::Fill)
            .style(|t: &Theme| panel_bg(t)),
        widget::rule::horizontal(1).style(shell_rule_style),
    ]
    .into()
}

pub(crate) fn sec_hdr<'a>(label: &str, label_alpha: f32) -> Element<'a, Message> {
    if label_alpha <= 0.0 {
        return container(text(""))
            .height(Length::Fixed(SEC_HDR_HEIGHT))
            .into();
    }
    let owned = label.to_string();
    let alpha = label_alpha;
    container(
        text(owned)
            .size(theme::text_size::LABEL_SMALL)
            // Same no-wrap rationale as nav_btn — section header text
            // ("Tools" / "도구") must not flow into two lines mid-tween.
            .wrapping(iced::widget::text::Wrapping::None)
            .style(move |t: &Theme| iced::widget::text::Style {
                color: Some(with_alpha(pal_of(t).on_surface_variant, alpha)),
            }),
    )
    .padding([10, 22])
    .height(Length::Fixed(SEC_HDR_HEIGHT))
    .into()
}

pub(crate) fn card<'a>(
    title: &str,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    container(
        column![
            text(title.to_string())
                .size(13)
                .style(label_style)
                .line_height(1.0),
            content.into(),
        ]
        .spacing(6)
        .padding(iced::Padding {
            top: 10.0,
            right: 18.0,
            bottom: 14.0,
            left: 18.0,
        })
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .style(|t: &Theme| {
        theme::surface_card_style(t, theme::SurfaceLevel::Default, theme::shape::MD, 1)
    })
    .into()
}

pub(crate) fn info_kv<'a>(label: &str, value: &str) -> Element<'a, Message> {
    column![
        text(label.to_string()).size(11).style(label_style),
        text(value.to_string()).size(14),
    ]
    .spacing(3)
    .into()
}

pub(crate) fn info_kv_center<'a>(label: &str, value: &str) -> Element<'a, Message> {
    column![
        text(label.to_string())
            .size(11)
            .style(label_style)
            .width(Length::Fill)
            .center(),
        // `WordOrGlyph` so a long, space-less file path wraps at glyph
        // boundaries within the panel instead of overflowing + clipping.
        text(value.to_string())
            .size(14)
            .width(Length::Fill)
            .center()
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
    ]
    .spacing(3)
    .width(Length::Fill)
    .align_x(iced::Alignment::Center)
    .into()
}

/// Like [`info_kv_center`] but the value is a click-to-edit "hidden
/// dropdown": pixel-identical to the static row until pressed, so casual
/// users never notice it. When `changed` (the picked option diverges from
/// the confirm baseline) the row takes an accent background + border and a
/// hover caution spelling out that this is a power-user override.
pub(crate) fn info_kv_center_editable<'a>(
    label: &str,
    value: &str,
    changed: bool,
    caution: &str,
    on_open: Message,
) -> Element<'a, Message> {
    let inner = column![
        text(label.to_string())
            .size(11)
            .style(label_style)
            .width(Length::Fill)
            .center(),
        text(value.to_string())
            .size(14)
            .width(Length::Fill)
            .center()
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
    ]
    .spacing(3)
    .width(Length::Fill)
    .align_x(iced::Alignment::Center);

    let btn = button(inner)
        .on_press(on_open)
        .padding([6, 10])
        .width(Length::Fill)
        .style(move |t: &Theme, status| {
            let p = pal_of(t);
            // Unchanged: no fill even on hover, so the row reads as plain
            // text. Changed: accent tint that deepens slightly on hover/press.
            let bg = if changed {
                with_alpha(p.primary, 0.16 + theme::state_alpha(status))
            } else {
                iced::Color::TRANSPARENT
            };
            button::Style {
                background: Some(bg.into()),
                text_color: p.on_surface,
                border: iced::Border {
                    color: if changed {
                        p.primary
                    } else {
                        iced::Color::TRANSPARENT
                    },
                    width: if changed { 1.0 } else { 0.0 },
                    radius: theme::shape::SM.into(),
                },
                ..Default::default()
            }
        });

    if !changed {
        return btn.into();
    }

    iced::widget::tooltip(
        btn,
        container(
            text(caution.to_string())
                .size(12)
                .style(warning_style)
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
        )
        .padding([8, 12])
        .max_width(280)
        .style(|t: &Theme| {
            let p = pal_of(t);
            container::Style {
                background: Some(p.surface_container_high.into()),
                border: iced::Border {
                    color: p.outline_variant,
                    width: 1.0,
                    radius: theme::shape::SM.into(),
                },
                ..Default::default()
            }
        }),
        iced::widget::tooltip::Position::Top,
    )
    .gap(6)
    .into()
}

pub(crate) fn adv_grid_btn<'a>(item: AdvAction, label: &str) -> Element<'a, Message> {
    // Inner container: border-only via `sel_card_style`. Earlier
    // version used `theme::surface_card_style` which paints an opaque
    // bg — that bg sat on top of the button's hover fill, swallowing
    // the highlight and making the grid feel dead on hover.
    let content = container(
        text(label.to_string())
            .size(12)
            .center()
            .width(Length::Fill)
            .style(|t: &Theme| iced::widget::text::Style {
                color: Some(pal_of(t).on_surface),
            }),
    )
    .padding([18, 12])
    .width(Length::Fill)
    .center_x(Length::Fill)
    .style(|t: &Theme| sel_card_style(t, false));

    button(content)
        .on_press(Message::Adv(AdvMsg::AdvConfirm(item)))
        .padding(0)
        .width(Length::Fill)
        .style(|t: &Theme, status| sel_card_btn_style(t, status, false))
        .into()
}

pub(crate) fn svg_icon(bytes: &'static [u8], size: f32) -> Element<'static, Message> {
    iced::widget::svg(iced::widget::svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        .into()
}

/// Disabled-state SVG icon — recolours the bitmap to `on_surface` at
/// 0.38 alpha. Brand colour is intentionally lost so the disabled card
/// reads as inert. Pair with [`icon_option_card_sub_disabled`].
pub(crate) fn svg_icon_disabled(bytes: &'static [u8], size: f32) -> Element<'static, Message> {
    iced::widget::svg(iced::widget::svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        .style(|t: &Theme, _| iced::widget::svg::Style {
            color: Some(with_alpha(pal_of(t).on_surface, 0.38)),
        })
        .into()
}

static SKROOT_ICON_HANDLE: std::sync::LazyLock<iced::widget::image::Handle> =
    std::sync::LazyLock::new(|| {
        iced::widget::image::Handle::from_bytes(
            include_bytes!("../../assets/icons/skroot.png").as_slice(),
        )
    });

pub(crate) fn skroot_icon(size: f32) -> Element<'static, Message> {
    widget::image(SKROOT_ICON_HANDLE.clone())
        .width(size)
        .height(size)
        .content_fit(iced::ContentFit::ScaleDown)
        .into()
}

/// Primary-coloured Lucide icon sized to `size`. Matches the colour
/// role the old per-asset SVG glyphs used for wizard tiles, status
/// markers, and confirm-step eyebrows.
pub(crate) fn lucide_primary(
    icon: iced::widget::Text<'static, Theme, iced::Renderer>,
    size: f32,
) -> Element<'static, Message> {
    icon.size(size)
        .style(|t: &Theme| iced::widget::text::Style {
            color: Some(pal_of(t).primary),
        })
        .into()
}

/// Disabled-state Lucide icon — `on_surface` at 0.38 alpha (M3 disabled
/// content tone). Pair with [`icon_option_card_sub_disabled`] so the
/// whole card reads as "not pickable on this device".
pub(crate) fn lucide_disabled(
    icon: iced::widget::Text<'static, Theme, iced::Renderer>,
    size: f32,
) -> Element<'static, Message> {
    icon.size(size)
        .style(|t: &Theme| iced::widget::text::Style {
            color: Some(with_alpha(pal_of(t).on_surface, 0.38)),
        })
        .into()
}

/// Lucide icon coloured by an arbitrary theme-driven closure. Used
/// where colour depends on widget state (nav active / disabled,
/// op success / failure, title-bar hover).
pub(crate) fn lucide_icon(
    icon: iced::widget::Text<'static, Theme, iced::Renderer>,
    size: f32,
    color: impl Fn(&Theme) -> iced::Color + 'static,
) -> Element<'static, Message> {
    icon.size(size)
        .style(move |t: &Theme| iced::widget::text::Style {
            color: Some(color(t)),
        })
        .into()
}

pub(crate) fn icon_option_card_sub(
    icon: Element<'static, Message>,
    label: &str,
    sub: &str,
    selected: bool,
    msg: Message,
) -> Element<'static, Message> {
    option_card(icon, label, sub, selected, Some(msg), false)
}

/// Disabled twin of [`icon_option_card_sub`]. Same icon / label / sub
/// layout, but rendered without an `on_press` so the button widget
/// reports `button::Status::Disabled` and the text reads as muted.
/// Used by the Root wizard to grey out family / mode cards that the
/// connected device's model doesn't support (e.g. Magisk on TB320FC).
pub(crate) fn icon_option_card_sub_disabled(
    icon: Element<'static, Message>,
    label: &str,
    sub: &str,
) -> Element<'static, Message> {
    option_card(icon, label, sub, false, None, false)
}

/// Square (1:1) variant of [`icon_option_card_sub`] for single-row wizard
/// steps. The fixed `WIZARD_CARD_SQUARE` side makes each option a square and
/// lets its row shrink-wrap + centre instead of stretching the cards full
/// width. The icon → title → description stack is unchanged.
pub(crate) fn icon_option_card_sub_square(
    icon: Element<'static, Message>,
    label: &str,
    sub: &str,
    selected: bool,
    msg: Message,
) -> Element<'static, Message> {
    option_card(icon, label, sub, selected, Some(msg), true)
}

/// Disabled twin of [`icon_option_card_sub_square`].
pub(crate) fn icon_option_card_sub_square_disabled(
    icon: Element<'static, Message>,
    label: &str,
    sub: &str,
) -> Element<'static, Message> {
    option_card(icon, label, sub, false, None, true)
}

/// Shared body for the vertical icon → title → description option card.
/// `msg = None` renders the disabled affordance; `square` swaps the
/// full-width × fixed-height box for a fixed 1:1 square.
fn option_card(
    icon: Element<'static, Message>,
    label: &str,
    sub: &str,
    selected: bool,
    msg: Option<Message>,
    square: bool,
) -> Element<'static, Message> {
    let enabled = msg.is_some();
    let label_style_fn = if enabled {
        on_surface_style
    } else {
        muted_style
    };
    // Sub text centres vertically inside the fixed box — top-aligning
    // left long gaps between short descs and the label above.
    let sub_text: Element<'static, Message> = if sub.is_empty() {
        text(" ").size(11).width(Length::Fill).center().into()
    } else {
        text(sub.to_string())
            .size(11)
            .style(muted_style)
            .width(Length::Fill)
            .center()
            .into()
    };
    // Square cards are narrower (fixed side), so longer localized
    // descriptions wrap to more lines. The 200px-tall square has vertical
    // slack below the icon + label, so give it a taller sub-row to absorb
    // ~4 lines instead of clipping; the standard card keeps its 2-line row.
    let sub_h = if square {
        WIZARD_CARD_SQUARE_SUB_HEIGHT
    } else {
        SUB_ROW_HEIGHT
    };
    let sub_row = container(sub_text)
        .width(Length::Fill)
        .height(Length::Fixed(sub_h))
        .align_y(iced::alignment::Vertical::Center);
    // Explicit icon→label vs label→desc gaps — a single `spacing` read
    // unbalanced because the centred sub-row adds ~9 px padding.
    let content = column![
        icon_tile(icon),
        Space::new().height(14),
        text(label.to_string())
            .size(13)
            .style(label_style_fn)
            .width(Length::Fill)
            .center(),
        Space::new().height(4),
        sub_row,
    ]
    .spacing(0)
    .align_x(iced::Alignment::Center);

    // Square → fixed side both ways so the row shrink-wraps and centres;
    // otherwise full width × the standard card height.
    let card_w: Length = if square {
        Length::Fixed(WIZARD_CARD_SQUARE)
    } else {
        Length::Fill
    };
    let card_h: f32 = if square {
        WIZARD_CARD_SQUARE
    } else {
        WIZARD_CARD_HEIGHT
    };

    let inner = container(content)
        .padding([20, 16])
        .width(card_w)
        .height(card_h)
        .center_x(card_w)
        .center_y(card_h)
        .style(move |t: &Theme| sel_card_style(t, selected && enabled));

    let btn = button(inner).padding(0).width(card_w);
    match msg {
        Some(m) => btn
            .on_press(m)
            .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected))
            .into(),
        None => btn
            // No `on_press` — iced reports Status::Disabled. Stronger M3
            // disabled affordance: dimmer surface + a thin outline_variant
            // border so the inert card reads distinctly against active ones.
            .style(|t: &Theme, _status| {
                let p = pal_of(t);
                button::Style {
                    background: Some(with_alpha(p.surface_container_low, 0.5).into()),
                    text_color: with_alpha(p.on_surface, 0.38),
                    border: iced::Border {
                        color: with_alpha(p.outline_variant, 0.6),
                        width: 1.0,
                        radius: theme::shape::MD.into(),
                    },
                    ..Default::default()
                }
            })
            .into(),
    }
}

/// Wrap a wizard icon. Icons already carry their own rounded-rect bg,
/// so no outer border.
pub(crate) fn icon_tile(icon: Element<'static, Message>) -> Element<'static, Message> {
    container(icon).padding(0).into()
}

impl RebootTarget {
    pub(crate) fn icon(self) -> Element<'static, Message> {
        let glyph = match self {
            Self::System => icon::reboot_system(),
            Self::Recovery => icon::reboot_recovery(),
            Self::Bootloader => icon::reboot_bootloader(),
            Self::Edl => icon::reboot_edl(),
        };
        lucide_primary(glyph, 32.0)
    }
}

impl Family {
    pub(crate) fn icon(self) -> Element<'static, Message> {
        // Kept as bundled SVG assets — these are per-brand logos, not
        // monochrome glyphs, so Lucide's icon set doesn't cover them.
        let bytes: &'static [u8] = match self {
            Self::Magisk => include_bytes!("../../assets/icons/magisk.svg"),
            Self::KernelSU => include_bytes!("../../assets/icons/kernelsu.svg"),
            Self::APatch => include_bytes!("../../assets/icons/apatch.svg"),
            Self::Skroot => return skroot_icon(72.0),
        };
        svg_icon(bytes, 72.0)
    }
}

impl Provider {
    pub(crate) fn icon(self) -> Element<'static, Message> {
        self.icon_sized(72.0)
    }

    /// Provider brand logo at an explicit size. The 2-provider square cards
    /// pass a smaller value so the 72px logo doesn't overflow the fixed
    /// square; the full-width grid cards keep the default 72px.
    pub(crate) fn icon_sized(self, size: f32) -> Element<'static, Message> {
        // Provider brand logos — kept as bespoke SVG, not Lucide.
        let bytes: &'static [u8] = match self {
            Self::Magisk => include_bytes!("../../assets/icons/magisk.svg"),
            Self::MagiskForks => include_bytes!("../../assets/icons/magisk_forks.svg"),
            Self::KernelSU => include_bytes!("../../assets/icons/kernelsu.svg"),
            Self::KernelSUNext => include_bytes!("../../assets/icons/kernelsu_next.svg"),
            Self::SukiSU => include_bytes!("../../assets/icons/sukisu.svg"),
            Self::ReSukiSU => include_bytes!("../../assets/icons/resukisu.svg"),
            Self::APatch => include_bytes!("../../assets/icons/apatch.svg"),
            Self::FolkPatch => include_bytes!("../../assets/icons/folkpatch.svg"),
        };
        svg_icon(bytes, size)
    }
}

impl RootMode {
    pub(crate) fn icon(self) -> Element<'static, Message> {
        // Lucide chip/layers glyphs in place of the old bespoke SVGs.
        let glyph = match self {
            Self::Lkm => icon::root_lkm(),
            Self::Gki => icon::root_gki(),
        };
        lucide_primary(glyph, 57.6)
    }
}

impl VerChoice {
    pub(crate) fn icon(self) -> Element<'static, Message> {
        let glyph = match self {
            Self::Stable => icon::ver_stable(),
            Self::Nightly => icon::ver_nightly(),
        };
        lucide_primary(glyph, 57.6)
    }
}

impl NightlySource {
    pub(crate) fn icon(self) -> Element<'static, Message> {
        let glyph = match self {
            Self::AutoDetect => icon::nightly_auto(),
            Self::ManualInput => icon::nightly_manual(),
        };
        lucide_primary(glyph, 57.6)
    }
}

impl App {
    /// Shared wizard confirm-screen frame: centered title, muted subtitle,
    /// a divider, then the caller's summary `rows` — all inside a fill
    /// scrollable so long summaries (ARB + country + region modify, rescue
    /// folder/region, the flash warning, …) can grow past the viewport.
    /// The wizard's sticky nav row is composed separately by the parent
    /// `view_*_wizard`. Only the title key, subtitle, and rows vary.
    pub(crate) fn confirm_view<'a>(
        &'a self,
        title_key: &str,
        subtitle: String,
        rows: Vec<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        let mut col = column![
            text(self.t(title_key).to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(subtitle).size(13).style(muted_style).center(),
            widget::rule::horizontal(1),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        for r in rows {
            col = col.push(r);
        }
        container(
            iced::widget::scrollable(col)
                .style(m3_scrollable_style)
                .height(Length::Fill)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}
