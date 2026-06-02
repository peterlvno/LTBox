//! Settings view (language, theme, default loader). Extracted from `main.rs`.

use crate::*;
use iced::widget::{self, Space, button, column, container, row, text};
use iced::{Element, Length, Theme};
use theme::{is_dark, mix_color, with_alpha};

impl App {
    pub(crate) fn view_settings(&self) -> Element<'_, Message> {
        let s = &self.settings;

        // Single untitled "Preferences" card holds language + theme.
        let lang_row = row![
            text(self.t("settings_language").to_string())
                .size(13)
                .width(Length::Fill),
            widget::pick_list(
                LANGUAGES.iter().map(|l| l.label()).collect::<Vec<_>>(),
                Some(s.language.label()),
                |selected| {
                    let l = LANGUAGES
                        .iter()
                        .find(|l| l.label() == selected)
                        .copied()
                        .unwrap_or(Language::En);
                    Message::Settings(SettingsMsg::SetLanguage(l))
                },
            )
            // Match the row label's 13 px size so the trigger button
            // doesn't tower over the "Language" label next to it. The
            // menu items inherit `text_size` for visual consistency
            // with the trigger.
            .text_size(13)
            .width(160),
        ]
        .align_y(iced::Alignment::Center);

        let t_system = self.t(ThemeChoice::System.label_key()).to_string();
        let t_light = self.t(ThemeChoice::Light.label_key()).to_string();
        let t_dark = self.t(ThemeChoice::Dark.label_key()).to_string();
        let current_theme_label = match self.theme_choice {
            ThemeChoice::System => t_system.clone(),
            ThemeChoice::Light => t_light.clone(),
            ThemeChoice::Dark => t_dark.clone(),
        };
        let theme_options: Vec<String> = vec![t_system.clone(), t_light.clone(), t_dark.clone()];
        let theme_row = row![
            text(self.t("settings_theme").to_string())
                .size(13)
                .width(Length::Fill),
            widget::pick_list(theme_options, Some(current_theme_label), move |selected| {
                let choice = if selected == t_system {
                    ThemeChoice::System
                } else if selected == t_dark {
                    ThemeChoice::Dark
                } else {
                    ThemeChoice::Light
                };
                Message::SetTheme(choice)
            },)
            // Match the row label's 13 px size. Same rationale as the
            // language pick list.
            .text_size(13)
            .width(160),
        ]
        .align_y(iced::Alignment::Center);

        // Default EDL loader used to auto-fill loader pickers.
        let default_loader_help = self.t("settings_default_loader_help").to_string();
        let help_icon = widget::tooltip(
            container(text("?").size(11).style(label_style))
                .padding([2, 6])
                .style(|t: &Theme| {
                    let p = pal_of(t);
                    container::Style {
                        background: Some(with_alpha(p.on_surface_variant, 0.10).into()),
                        border: iced::Border {
                            radius: theme::shape::FULL.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }),
            container(text(default_loader_help).size(11))
                .padding([6, 10])
                .max_width(280)
                .style(|t: &Theme| theme::tooltip_style(t, theme::shape::SM)),
            widget::tooltip::Position::Right,
        );

        // Icon-only actions keep tooltips for accessibility.
        let browse_btn = button(
            container(lucide_icon(icon::settings_browse(), 18.0, |t: &Theme| {
                pal_of(t).on_secondary_container
            }))
            .width(36)
            .height(36)
            .center_x(36)
            .center_y(36),
        )
        .on_press(Message::Settings(SettingsMsg::SettingsPickDefaultLoader))
        .padding(0)
        .style(|t: &Theme, status| {
            let p = pal_of(t);
            let base = p.secondary_container;
            // iced's `button::Style::background` only accepts a single
            // color/gradient, so the M3 state-layer (semi-transparent
            // on_X over the tonal base) is pre-composited into one
            // opaque tint via `mix_color`.
            let bg = match status {
                button::Status::Hovered => {
                    mix_color(base, p.on_secondary_container, theme::state::HOVER)
                }
                button::Status::Pressed => {
                    mix_color(base, p.on_secondary_container, theme::state::PRESSED)
                }
                _ => base,
            };
            let bg = Some(bg.into());
            button::Style {
                background: bg,
                border: iced::Border {
                    radius: theme::shape::FULL.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        });
        let browse_tip = widget::tooltip(
            browse_btn,
            container(text(self.t("settings_default_loader_browse").to_string()).size(11))
                .padding([6, 10])
                .style(|t: &Theme| theme::tooltip_style(t, theme::shape::XS)),
            widget::tooltip::Position::Top,
        );

        let mut default_loader_actions = row![browse_tip,]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        if self.default_loader_path.is_some() {
            let clear_btn = button(
                container(lucide_icon(icon::settings_clear(), 18.0, |t: &Theme| {
                    pal_of(t).on_error_container
                }))
                .width(36)
                .height(36)
                .center_x(36)
                .center_y(36),
            )
            .on_press(Message::Settings(SettingsMsg::SettingsClearDefaultLoader))
            .padding(0)
            .style(|t: &Theme, status| {
                let p = pal_of(t);
                let base = p.error_container;
                let bg = match status {
                    button::Status::Hovered => {
                        Some(mix_color(base, p.on_error_container, theme::state::HOVER).into())
                    }
                    button::Status::Pressed => {
                        Some(mix_color(base, p.on_error_container, theme::state::PRESSED).into())
                    }
                    _ => Some(base.into()),
                };
                button::Style {
                    background: bg,
                    border: iced::Border {
                        radius: theme::shape::FULL.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });
            let clear_tip = widget::tooltip(
                clear_btn,
                container(text(self.t("settings_default_loader_clear").to_string()).size(11))
                    .padding([6, 10])
                    .style(|t: &Theme| theme::tooltip_style(t, theme::shape::XS)),
                widget::tooltip::Position::Top,
            );
            default_loader_actions = default_loader_actions.push(clear_tip);
        }

        let default_loader_top = row![
            text(self.t("settings_default_loader").to_string())
                .size(13)
                .line_height(1.0),
            help_icon,
            Space::new().width(Length::Fill),
            default_loader_actions,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let default_loader_path_str = self
            .default_loader_path
            .clone()
            .unwrap_or_else(|| self.t("settings_default_loader_unset").to_string());
        let default_loader_row = column![
            default_loader_top,
            text(default_loader_path_str).size(11).style(muted_style),
        ]
        .spacing(6);

        let prefs_card = container(
            column![lang_row, theme_row, default_loader_row,]
                .spacing(14)
                .padding(iced::Padding {
                    top: 14.0,
                    right: 18.0,
                    bottom: 14.0,
                    left: 18.0,
                })
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .style(|t: &Theme| {
            let p = pal_of(t);
            container::Style {
                background: Some(p.surface_container.into()),
                border: iced::Border {
                    color: p.outline_variant,
                    width: 1.0,
                    radius: theme::shape::MD.into(),
                },
                shadow: theme::elevation(1, is_dark(t)),
                ..Default::default()
            }
        });

        column![
            text(self.t("settings_title").to_string()).size(theme::text_size::TITLE_LARGE),
            widget::rule::horizontal(1),
            prefs_card,
        ]
        .spacing(14)
        .width(Length::Fill)
        .into()
    }
}
