//! Modal popup views (device info, OTA, ARB index, country, region, rescue region, log). Extracted from `main.rs`.

use crate::*;
use iced::widget::{self, Space, button, column, container, row, scrollable, text};
use iced::{Element, Length, Theme};
use theme::with_alpha;

impl App {
    /// Device-info popup: render the Lenovo PTSTPD `data` block as a
    /// 2-column key/value table. Branches on `DeviceInfoState` so the
    /// modal stays open through Loading / Error / Ready transitions
    /// without flashing in/out of existence.
    pub(crate) fn device_info_popup_view(&self) -> Element<'_, Message> {
        let Some((serial, state)) = self.device_info_popup.clone() else {
            return container(text("")).into();
        };
        let title = text(self.t("device_info_popup_title").to_string())
            .size(theme::text_size::WIZARD_STEP_TITLE);
        // Copy-icon button — only enabled once the upstream payload is
        // cached; clicking copies the unmodified `data` JSON to the
        // clipboard and surfaces a toast.
        let copy_payload: Option<String> = self
            .device_info_cache
            .get(&serial)
            .map(|i| i.data_pretty.clone());
        let copy_glyph = text("⧉").size(16);
        let copy_btn = if let Some(payload) = copy_payload {
            button(container(copy_glyph).padding([2, 6]))
                .on_press(Message::CopyToClipboard(payload))
                .padding(0)
                .style(|t: &Theme, status| {
                    let p = pal_of(t);
                    let hovered = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: Some(
                            if hovered {
                                p.surface_container_high
                            } else {
                                p.surface_container
                            }
                            .into(),
                        ),
                        text_color: p.on_surface,
                        border: iced::Border {
                            radius: 6.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
        } else {
            // Same shape, no on_press — keeps the header layout stable
            // during the loading / error states without leaving an
            // active click target.
            button(container(copy_glyph).padding([2, 6]))
                .padding(0)
                .style(|t: &Theme, _s| {
                    let p = pal_of(t);
                    button::Style {
                        background: Some(p.surface_container.into()),
                        text_color: p.on_surface_variant,
                        border: iced::Border {
                            radius: 6.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
        };
        let header = iced::widget::row![title, Space::new().width(Length::Fill), copy_btn]
            .align_y(iced::Alignment::Center);
        let serial_line = text(format!("{}: {serial}", self.t("device_info_popup_serial")))
            .size(12)
            .style(muted_style);

        let body: Element<'_, Message> = match &state {
            DeviceInfoState::Loading => self.popup_loading_view(),
            DeviceInfoState::Error(e) => {
                self.popup_error_view("device_info_popup_error", e, Message::DeviceInfoRetry)
            }
            DeviceInfoState::Ready => {
                let info = match self.device_info_cache.get(&serial) {
                    Some(i) => i,
                    None => {
                        return container(text("")).into();
                    }
                };
                let mut table = column![].spacing(0);
                for (i, (k, v)) in info.fields.iter().enumerate() {
                    let display_v = v.clone().unwrap_or_default();
                    let key_cell = text(k.clone()).size(12).style(muted_style).width(180);
                    let val_cell = text(display_v).size(12).width(Length::Fill);
                    let row_inner = iced::widget::row![key_cell, val_cell]
                        .spacing(12)
                        .padding([4, 10])
                        .align_y(iced::Alignment::Center);
                    let zebra = i % 2 == 1;
                    let tinted = container(row_inner).width(Length::Fill).style(
                        move |t: &Theme| -> container::Style {
                            let p = pal_of(t);
                            container::Style {
                                background: if zebra {
                                    Some(iced::Background::Color(p.surface_container_low))
                                } else {
                                    None
                                },
                                ..Default::default()
                            }
                        },
                    );
                    table = table.push(tinted);
                }
                scrollable(table)
                    .height(Length::Fixed(420.0))
                    .width(Length::Fill)
                    .into()
            }
        };

        let close_btn = button(text(self.t("btn_close").to_string()).size(12))
            .on_press(Message::DeviceInfoClose)
            .padding([6, 18])
            .style(md_filled_btn_style);

        let content = column![
            header,
            serial_line,
            widget::rule::horizontal(1),
            body,
            iced::widget::row![Space::new().width(Length::Fill), close_btn]
                .align_y(iced::Alignment::Center),
        ]
        .spacing(12)
        .padding(20)
        .width(640);

        m3_dialog(content.into())
    }

    /// Lenovo OTA "querynewfirmware" popup. Opens when the user clicks
    /// the dashboard firmware version. Mirrors `device_info_popup_view`
    /// for header / spinner / error / close-button shape, but renders
    /// the OTA payload as a stacked card (From / To / Size / MD5 /
    /// Changelog / Download) instead of a flat key-value table.
    pub(crate) fn ota_popup_view(&self) -> Element<'_, Message> {
        let Some((_serial, _firmware_id, state)) = self.ota_popup.clone() else {
            return container(text("")).into();
        };
        let title =
            text(self.t("ota_popup_title").to_string()).size(theme::text_size::WIZARD_STEP_TITLE);
        let header = iced::widget::row![title, Space::new().width(Length::Fill)]
            .align_y(iced::Alignment::Center);

        let body: Element<'_, Message> = match &state {
            OtaPopupState::Loading => self.popup_loading_view(),
            OtaPopupState::Error(e) => {
                self.popup_error_view("ota_popup_error", e, Message::OtaRetry)
            }
            OtaPopupState::NoUpdate => container(
                text(self.t("ota_popup_unavailable").to_string())
                    .size(14)
                    .style(muted_style)
                    .width(Length::Fill)
                    .center(),
            )
            .width(Length::Fill)
            .height(48)
            .center_x(Length::Fill)
            .center_y(48)
            .into(),
            OtaPopupState::Ready(update) => {
                // Changelog text lives in `self.ota_changelog_editor`,
                // seeded by the `OtaFetched` handler from `desc_cn`
                // (Chinese GUI locale, when populated) or `desc_en`.
                // Rendered here through `text_editor` so drag-select +
                // Ctrl+C work — a plain `text` widget is a static label
                // and won't surface a selection.
                let size_str = ltbox_core::lenovo_ota::format_size(update.size_bytes);

                let from_to_row = column![
                    text(format!("{}: {}", self.t("ota_popup_from"), update.from))
                        .size(12)
                        .style(muted_style),
                    text(format!("{}: {}", self.t("ota_popup_to"), update.to)).size(13),
                ]
                .spacing(4);

                let meta_row = iced::widget::row![
                    info_kv(self.t("ota_popup_size"), &size_str),
                    info_kv(self.t("ota_popup_md5"), &update.md5),
                ]
                .spacing(40);

                let changelog_editor: Element<'_, Message> =
                    iced::widget::text_editor(&self.ota_changelog_editor)
                        .on_action(Message::OtaChangelogAction)
                        .size(12)
                        .into();
                let changelog_block = column![
                    text(self.t("ota_popup_changelog").to_string())
                        .size(11)
                        .style(label_style),
                    container(changelog_editor)
                        .padding([8, 10])
                        .width(Length::Fill)
                        .style(|t: &Theme| {
                            let p = pal_of(t);
                            container::Style {
                                background: Some(p.surface_container_low.into()),
                                border: iced::Border {
                                    color: p.outline_variant,
                                    width: 1.0,
                                    radius: theme::shape::SM.into(),
                                },
                                ..Default::default()
                            }
                        }),
                ]
                .spacing(4);

                scrollable(
                    column![
                        from_to_row,
                        widget::rule::horizontal(1),
                        meta_row,
                        widget::rule::horizontal(1),
                        changelog_block,
                    ]
                    .spacing(12)
                    .width(Length::Fill),
                )
                .height(Length::Fixed(420.0))
                .width(Length::Fill)
                .into()
            }
        };

        // Bottom action row: Download (when Ready + url present) sits
        // left of Close so the scrollable body's right-edge gutter
        // can't overlap the action — both buttons live on the dialog
        // chrome below the scrollable, not inside it.
        let download_url: Option<String> = match &state {
            OtaPopupState::Ready(u) if !u.download_url.is_empty() => Some(u.download_url.clone()),
            _ => None,
        };
        let close_btn = button(text(self.t("btn_close").to_string()).size(12))
            .on_press(Message::OtaClose)
            .padding([6, 18])
            .style(md_filled_btn_style);
        let mut action_row = iced::widget::row![Space::new().width(Length::Fill)]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        if let Some(url) = download_url {
            let download_btn = button(text(self.t("ota_popup_download").to_string()).size(12))
                .on_press(Message::OtaOpenDownload(url))
                .padding([6, 18])
                .style(md_filled_btn_style);
            action_row = action_row.push(download_btn);
        }
        action_row = action_row.push(close_btn);

        let content = column![header, widget::rule::horizontal(1), body, action_row,]
            .spacing(12)
            .padding(20)
            .width(640);

        m3_dialog(content.into())
    }

    /// PatchArb timestamp popup. Reads `adv_wizard.arb_index_buffer`
    /// for the in-flight typing and renders the UTC representation in
    /// real time once the buffer hits exactly 10 digits. OK is enabled
    /// only on a 10-digit buffer that parses to a `u64`.
    pub(crate) fn arb_index_popup_view(&self) -> Element<'_, Message> {
        let buf = self.adv_wizard.arb_index_buffer.clone();
        let valid = buf.len() == 10 && buf.parse::<u64>().is_ok();

        // UTC preview only when the buffer is exactly 10 digits, so
        // shrinking the value (e.g. backspacing while editing) makes
        // the preview disappear instead of jumping to a stale time.
        let utc_preview: Element<'_, Message> = if valid {
            let ts: u64 = buf.parse().unwrap_or(0);
            let formatted = format_unix_timestamp_utc(ts);
            text(formatted)
                .size(13)
                .color(iced::Color::from_rgb(0.4, 0.7, 0.4))
                .into()
        } else {
            // Keep a fixed-height placeholder so the layout doesn't
            // jump when the preview appears / disappears.
            container(text("").size(13)).height(20).into()
        };

        let title = text(self.t("arb_index_popup_title").to_string())
            .size(theme::text_size::WIZARD_STEP_TITLE);
        let subtitle = text(self.t("arb_index_popup_subtitle").to_string())
            .size(12)
            .style(muted_style);

        let input = iced::widget::text_input(
            self.t("arb_index_popup_placeholder"),
            &self.adv_wizard.arb_index_buffer,
        )
        .on_input(|s| Message::Adv(AdvMsg::AdvWizArbIndexInput(s)))
        .on_submit(Message::Adv(AdvMsg::AdvWizArbIndexConfirm))
        .padding([8, 12])
        .size(14)
        .width(Length::Fill);

        let cancel_btn = button(text(self.t("btn_cancel").to_string()).size(13))
            .on_press(Message::Adv(AdvMsg::AdvWizArbIndexCancel))
            .padding([8, 18])
            .style(md_text_btn_style);
        let ok_btn_inner = text(self.t("btn_ok").to_string())
            .size(13)
            .color(iced::Color::WHITE);
        let ok_btn = if valid {
            button(ok_btn_inner)
                .on_press(Message::Adv(AdvMsg::AdvWizArbIndexConfirm))
                .padding([8, 18])
                .style(md_filled_btn_style)
        } else {
            button(ok_btn_inner)
                .padding([8, 18])
                .style(md_filled_btn_style)
        };

        let content = column![
            title,
            subtitle,
            utc_preview,
            input,
            iced::widget::row![Space::new().width(Length::Fill), cancel_btn, ok_btn]
                .spacing(8)
                .align_y(iced::Alignment::Center),
        ]
        .spacing(12)
        .padding(20)
        .width(420);

        m3_dialog(content.into())
    }

    pub(crate) fn country_popup_view(&self) -> Element<'_, Message> {
        let mut list = column![].spacing(2);
        let selected_code = self.country_popup_selected_code();
        // Flash wizard only — hide "Do not change" from the Advanced
        // PatchDevinfo flow because that action requires a concrete target
        // code to write into devinfo/persist.
        if !self.adv_needs_country {
            let skipped = self.wf_config.country_action.is_skipped();
            list = list.push(
                button(text(self.t("popup_country_do_not_change").to_string()).size(13))
                    .on_press(Message::SkipCountryPatch)
                    .padding([6, 14])
                    .width(Length::Fill)
                    .style(move |t: &Theme, status| {
                        let p = pal_of(t);
                        let hover = matches!(status, button::Status::Hovered);
                        button::Style {
                            background: Some(if skipped {
                                p.primary.into()
                            } else if hover {
                                p.surface_container_high.into()
                            } else {
                                iced::Color::TRANSPARENT.into()
                            }),
                            text_color: if skipped { p.on_primary } else { p.on_surface },
                            ..Default::default()
                        }
                    }),
            );
            list = list.push(widget::rule::horizontal(1));
        }
        // TB322FC PRC-only: only CN is selectable. Non-CN rows render
        // as disabled buttons so the constraint stays visible on the
        // popup. The "Do not change" row above remains usable.
        let tb322fc = self.is_tb322fc();
        for entry in COUNTRY_CODES {
            let code = entry.code.to_string();
            let selected = selected_code == Some(entry.code);
            let label = format!("{} — {}", entry.code, entry.name);
            let disabled = tb322fc && !entry.code.eq_ignore_ascii_case("CN");
            let mut btn = button(text(label).size(13))
                .padding([6, 14])
                .width(Length::Fill)
                .style(move |t: &Theme, status| {
                    let p = pal_of(t);
                    let hover = matches!(status, button::Status::Hovered);
                    if disabled {
                        return button::Style {
                            background: Some(iced::Color::TRANSPARENT.into()),
                            text_color: with_alpha(p.on_surface, 0.38),
                            ..Default::default()
                        };
                    }
                    button::Style {
                        background: Some(if selected {
                            p.primary.into()
                        } else if hover {
                            p.surface_container_high.into()
                        } else {
                            iced::Color::TRANSPARENT.into()
                        }),
                        text_color: if selected { p.on_primary } else { p.on_surface },
                        ..Default::default()
                    }
                });
            if !disabled {
                btn = btn.on_press(Message::SelectCountry(code));
            }
            list = list.push(btn);
        }

        let popup_content: Element<'_, Message> = column![
            row![
                text(self.t("popup_select_country").to_string()).size(16),
                Space::new().width(Length::Fill),
                button(
                    text(self.t("btn_cancel").to_string())
                        .size(12)
                        .style(muted_style)
                )
                .on_press(Message::DismissCountryPopup)
                .padding([4, 12])
                .style(neutral_pill_btn_style),
            ]
            .align_y(iced::Alignment::Center),
            widget::rule::horizontal(1),
            scrollable(list).height(300),
        ]
        .spacing(10)
        .padding(20)
        .width(400)
        .into();
        m3_dialog(popup_content)
    }

    /// PRC / ROW radio popup for the Advanced RegionConvert wizard.
    /// Smaller than the country popup (only two choices) so the
    /// content uses M3 radio rows in a fixed-width card.
    pub(crate) fn region_target_popup_view(&self) -> Element<'_, Message> {
        let selected = self.adv_wizard.region_target;
        let mut list = column![].spacing(2);
        for target in [DeviceRegion::Prc, DeviceRegion::Row] {
            let is_selected = selected == Some(target);
            let label = self.t(target.label_key()).to_string();
            let bg_color = if is_selected {
                ACCENT
            } else {
                iced::Color::TRANSPARENT
            };
            let txt_color = if is_selected {
                iced::Color::WHITE
            } else {
                iced::Color::BLACK
            };
            list = list.push(
                button(text(label).size(13).color(txt_color))
                    .on_press(Message::SelectRegionTarget(target))
                    .padding([6, 14])
                    .width(Length::Fill)
                    .style(move |_t: &Theme, status| {
                        let hover = matches!(status, button::Status::Hovered);
                        button::Style {
                            background: Some(if is_selected {
                                bg_color.into()
                            } else if hover {
                                iced::Color::from_rgba(0.357, 0.388, 0.878, 0.08).into()
                            } else {
                                iced::Color::TRANSPARENT.into()
                            }),
                            text_color: txt_color,
                            ..Default::default()
                        }
                    }),
            );
        }

        let popup_content: Element<'_, Message> = column![
            row![
                text(self.t("popup_select_region_target").to_string()).size(16),
                Space::new().width(Length::Fill),
                button(
                    text(self.t("btn_cancel").to_string())
                        .size(12)
                        .style(muted_style)
                )
                .on_press(Message::DismissRegionTargetPopup)
                .padding([4, 12])
                .style(neutral_pill_btn_style),
            ]
            .align_y(iced::Alignment::Center),
            widget::rule::horizontal(1),
            list,
        ]
        .spacing(10)
        .padding(20)
        .width(320)
        .into();
        m3_dialog(popup_content)
    }

    pub(crate) fn rescue_region_popup_view(&self) -> Element<'_, Message> {
        let mk_option = |region: RescueRegion, desc_key: &'static str| {
            let label = self.t(region.label_key()).to_string();
            let desc = self.t(desc_key).to_string();
            let selected = self.sysupdate.rescue_region == Some(region);
            button(
                column![
                    text(label).size(15).style(on_surface_style),
                    text(desc).size(12).style(muted_style),
                ]
                .spacing(4),
            )
            .on_press(Message::Sys(SysMsg::SysRescueRegion(region)))
            .padding([10, 16])
            .width(Length::Fill)
            .style(move |t: &Theme, status| {
                let p = pal_of(t);
                let hover = matches!(status, button::Status::Hovered);
                let bg = if selected {
                    p.primary_container.into()
                } else if hover {
                    with_alpha(p.primary, theme::state::HOVER).into()
                } else {
                    iced::Color::TRANSPARENT.into()
                };
                button::Style {
                    background: Some(bg),
                    text_color: p.on_surface,
                    border: iced::Border {
                        color: if selected {
                            p.primary
                        } else {
                            p.outline_variant
                        },
                        width: 1.0,
                        radius: theme::shape::SM.into(),
                    },
                    ..Default::default()
                }
            })
        };
        let popup_content: Element<'_, Message> = column![
            row![
                text(self.t("rescue_region_popup_title").to_string()).size(16),
                Space::new().width(Length::Fill),
                button(
                    text(self.t("btn_cancel").to_string())
                        .size(12)
                        .style(muted_style)
                )
                .on_press(Message::Sys(SysMsg::SysRescueRegionPopupDismiss))
                .padding([4, 12])
                .style(neutral_pill_btn_style),
            ]
            .align_y(iced::Alignment::Center),
            widget::rule::horizontal(1),
            text(self.t("rescue_region_popup_subtitle").to_string())
                .size(12)
                .style(muted_style),
            mk_option(RescueRegion::Prc, "rescue_region_prc_desc"),
            mk_option(RescueRegion::Row, "rescue_region_row_desc"),
        ]
        .spacing(10)
        .padding(20)
        .width(420)
        .into();
        m3_dialog(popup_content)
    }

    /// Full-viewport log popup. Replaces the wizard body while open;
    /// dismissed via Close.
    pub(crate) fn log_popup_view(&self) -> Element<'_, Message> {
        let editor = iced::widget::text_editor(&self.log_editor)
            .on_action(Message::LogEditorAction)
            .size(11)
            .height(Length::Fill);
        let body = column![
            row![
                text(self.t("log_popup_title").to_string()).size(theme::text_size::TITLE_LARGE),
                Space::new().width(Length::Fill),
                button(text(self.t("btn_save_log").to_string()).size(12))
                    .on_press(Message::SaveLog)
                    .padding([6, 14])
                    .style(|t: &Theme, _s| {
                        let p = pal_of(t);
                        button::Style {
                            background: Some(with_alpha(p.on_surface, 0.1).into()),
                            text_color: p.on_surface,
                            border: iced::Border {
                                radius: 6.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }),
                button(text(self.t("btn_close").to_string()).size(12))
                    .on_press(Message::ToggleLogPopup(false))
                    .padding([6, 16])
                    .style(|_t: &Theme, status| {
                        let a = match status {
                            button::Status::Hovered => 1.0,
                            _ => 0.85,
                        };
                        button::Style {
                            background: Some(iced::Color { a, ..ACCENT }.into()),
                            text_color: iced::Color::WHITE,
                            border: iced::Border {
                                radius: 6.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            widget::rule::horizontal(1),
            container(editor)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(10)
                .style(|t: &Theme| theme::surface_card_style(
                    t,
                    theme::SurfaceLevel::Low,
                    theme::shape::SM,
                    0,
                )),
        ]
        .spacing(12)
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fill);
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
