//! Dashboard view (device status, action tiles). Extracted from `main.rs`.

use crate::*;
use iced::widget::{Space, button, column, container, row, text};
use iced::{Element, Length, Theme};

impl App {
    pub(crate) fn view_dashboard(&self) -> Element<'_, Message> {
        let model = if self.device_model.is_empty() {
            "—"
        } else {
            &self.device_model
        };
        let slot = if self.device_slot.is_empty() {
            "—"
        } else {
            &self.device_slot
        };
        let firmware = if self.device_firmware.is_empty() {
            "—"
        } else {
            &self.device_firmware
        };
        // i18n key (`arb_*`) or numeric from fastboot vars; translation
        // layer passes numerics through.
        let arb_raw = self.device_arb.clone();
        let arb_display = if arb_raw.is_empty() {
            "—".to_string()
        } else if arb_raw.starts_with("arb_") {
            self.t(&arb_raw).to_string()
        } else {
            arb_raw
        };
        let arb = arb_display.as_str();
        let ram = if self.device_ram.is_empty() {
            "—"
        } else {
            &self.device_ram
        };
        let storage = if self.device_storage.is_empty() {
            "—"
        } else {
            &self.device_storage
        };
        let op_text = if self.busy {
            let base = self.t("dash_operation_in_progress").to_string();
            let label = format!("{base} - {}", self.busy_operation_label());
            text(label).size(13).style(accent_style)
        } else {
            text(self.t("dash_no_operation").to_string())
                .size(13)
                .style(muted_style)
        };
        let _log_preview_len = self.log_lines.len();

        // Title + divider dropped — sidebar already labels the active view,
        // so the duplicate header was eating vertical space without telling
        // the user anything new. `height(Fill)` so the log card (the last
        // child) can claim the remaining vertical space — keeps the top +
        // bottom dashboard margins symmetric.
        let mut content = column![]
            .spacing(14)
            .width(Length::Fill)
            .height(Length::Fill);

        // Unauthorized ADB wins over the platform warning — empty
        // `ro.boot.hardware` otherwise reads as "unsupported platform".
        if self.connection == ConnectionStatus::AdbServerBlocking {
            let msg: Element<'_, Message> = text(self.t("dash_adb_server_blocking").to_string())
                .size(12)
                .style(warning_container_text_style)
                .width(Length::Fill)
                .into();
            let kill_btn: Element<'_, Message> = button(
                text(self.t("btn_kill_adb_server").to_string())
                    .size(12)
                    .style(warning_container_text_style),
            )
            .on_press(Message::KillAdbServer)
            .padding([6, 12])
            .style(|t: &Theme, status| {
                let p = pal_of(t);
                button::Style {
                    background: Some(
                        with_alpha(p.on_warning_container, theme::state_alpha(status).max(0.10))
                            .into(),
                    ),
                    text_color: p.on_warning_container,
                    border: iced::Border {
                        radius: theme::shape::XS.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .into();
            content = content.push(
                container(
                    row![msg, kill_btn]
                        .spacing(12)
                        .width(Length::Fill)
                        .align_y(iced::Alignment::Center),
                )
                .padding([6, 16])
                .width(Length::Fill)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    container::Style {
                        background: Some(p.warning_container.into()),
                        border: iced::Border {
                            color: p.warning_container,
                            radius: theme::shape::SM.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }),
            );
        } else if self.connection == ConnectionStatus::AdbUnauthorized {
            content = content.push(
                container(
                    text(self.t("dash_adb_unauthorized").to_string())
                        .size(12)
                        .style(warning_container_text_style),
                )
                .padding([10, 16])
                .width(Length::Fill)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    container::Style {
                        background: Some(p.warning_container.into()),
                        border: iced::Border {
                            color: p.warning_container,
                            radius: theme::shape::SM.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }),
            );
        } else if self.platform_supported == Some(false) {
            content = content.push(
                container(
                    text(self.t("dash_unsupported_platform").to_string())
                        .size(12)
                        .style(warning_container_text_style),
                )
                .padding([10, 16])
                .width(Length::Fill)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    container::Style {
                        background: Some(p.warning_container.into()),
                        border: iced::Border {
                            color: p.warning_container,
                            radius: theme::shape::SM.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                }),
            );
        }

        if matches!(
            self.driver_status,
            Some(
                ltbox_device::driver::DriverStatus::Missing(_)
                    | ltbox_device::driver::DriverStatus::UdevRulesMissing
                    | ltbox_device::driver::DriverStatus::UdevRulesStale
                    | ltbox_device::driver::DriverStatus::UdevRulesNoPermission
                    | ltbox_device::driver::DriverStatus::KernelDriverMissing
                    | ltbox_device::driver::DriverStatus::KernelDriverUnsupported
            )
        ) {
            content = content.push(self.driver_warning_banner());
        } else if self.driver_update.is_some() {
            // Driver present but outdated — optional update prompt. Mutually
            // exclusive with the missing banner above (a missing driver has
            // no version to compare).
            content = content.push(self.driver_update_banner());
        }

        // Dual-USB-C port advisory — independent of the driver banners, so a
        // matching model still gets the port hint even when drivers are fine.
        if let Some(model) = self.dual_usb_advisory_model() {
            content = content.push(self.dual_usb_advisory_banner(model));
        }

        let mut device_col = column![].spacing(0).width(Length::Fill);
        device_col = device_col.push(
            text(self.t("dash_device").to_string())
                .size(13)
                .style(label_style)
                .line_height(1.0),
        );
        device_col = device_col.push(Space::new().height(4));
        if !self.device_market_name.is_empty() {
            device_col = device_col.push(
                text(self.device_market_name.clone())
                    .size(16)
                    .line_height(1.0),
            );
        }
        device_col = device_col.push(Space::new().height(12));
        device_col = device_col.push(
            row![
                info_kv(self.t("device_model"), model),
                info_kv(self.t("device_ram"), ram),
                info_kv(self.t("device_storage"), storage),
                info_kv(self.t("device_slot"), slot),
            ]
            .spacing(40),
        );
        device_col = device_col.push(Space::new().height(6));
        // Firmware kv is clickable when a firmware id is populated —
        // tap to fetch the matching Lenovo OTA update payload. Wrap in
        // a `button` with `dash_clickable_btn_style` so the cell stays
        // flush with the card at rest but tints on hover, making the
        // click affordance visible. The previous `mouse_area` only set
        // the cursor — users on a stable pointer (touchpad tap) had no
        // visual cue.
        // Firmware kv uses vertical-only padding so the clickable
        // hover bg still has breathing room top/bottom around the
        // label, but the label's left edge stays at the cell's x=0
        // — keeping it column-aligned with the row above (모델 / RAM
        // / 저장소 / 슬롯). Horizontal hover padding would push the
        // label right and break that alignment.
        let firmware_kv: Element<'_, Message> = if self.device_firmware.is_empty() {
            info_kv(self.t("device_firmware"), firmware)
        } else {
            button(info_kv(self.t("device_firmware"), firmware))
                .on_press(Message::OtaOpen)
                .padding([4, 0])
                .style(dash_clickable_btn_style)
                .into()
        };
        // Row align_y(Center) handles the vertical mismatch: the
        // firmware button is `4 + label + 4` tall while the bare ARB
        // kv is just `label` tall — centering both within the row's
        // max height puts the two labels at the same y without
        // introducing any horizontal offset on the ARB cell.
        // When the rollback value is a real committed index (numeric, from
        // fastboot `stored_rollback_index`), hover shows it as a UTC datetime;
        // the yes/no model fallback gets no tooltip.
        let arb_kv: Element<'_, Message> = if let Ok(idx) = self.device_arb.parse::<u64>() {
            iced::widget::tooltip(
                info_kv(self.t("device_arb"), arb),
                container(text(crate::format_unix_timestamp_utc(idx)).size(11))
                    .padding([6, 10])
                    .style(|t: &Theme| theme::tooltip_style(t, theme::shape::SM)),
                iced::widget::tooltip::Position::Top,
            )
            .into()
        } else {
            info_kv(self.t("device_arb"), arb)
        };
        device_col = device_col.push(
            row![arb_kv, firmware_kv,]
                .spacing(40)
                .align_y(iced::Alignment::Center),
        );

        // Pin the inner row to 160 px regardless of whether the device is
        // populated. Without this the empty-state card collapses to the
        // text column's natural height, then jumps taller once a device
        // connects — same card, two different sizes. The portrait branch
        // already used `height(160)`; the empty branch now matches so the
        // dashboard layout doesn't reflow on connect.
        let device_card_inner: Element<'_, Message> = if self.device_model.is_empty() {
            container(device_col).width(Length::Fill).height(160).into()
        } else {
            let portrait: Element<'_, Message> = match device_portrait(&self.device_model) {
                DevicePortrait::Png(h) => iced::widget::image(h)
                    .height(Length::Fill)
                    .content_fit(iced::ContentFit::ScaleDown)
                    .into(),
                DevicePortrait::Svg(h) => iced::widget::svg(h)
                    .height(Length::Fill)
                    .content_fit(iced::ContentFit::ScaleDown)
                    .into(),
            };
            // Click on the portrait fires the Lenovo PTSTPD lookup popup.
            // Skip when no serial was captured (e.g. EDL connection) so
            // the click is a clear no-op rather than triggering an empty
            // upstream query.
            let portrait_box = container(portrait)
                .width(220)
                .height(Length::Fill)
                .center_x(220)
                .center_y(Length::Fill);
            let portrait_clickable: Element<'_, Message> = if self.device_serial.is_empty() {
                portrait_box.into()
            } else {
                // Same hover-tint pattern as the firmware kv so both
                // dashboard click targets look identically interactive.
                button(portrait_box)
                    .on_press(Message::DeviceInfoOpen)
                    .padding(0)
                    .style(dash_clickable_btn_style)
                    .into()
            };
            row![device_col, portrait_clickable,]
                .spacing(16)
                .align_y(iced::Alignment::Center)
                .height(160)
                .into()
        };
        content = content.push(
            container(
                container(device_card_inner)
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
                // Elevation 1 to match its sibling dashboard cards (current-op,
                // log); it was the only one flat at 0.
                theme::surface_card_style(t, theme::SurfaceLevel::Default, theme::shape::MD, 1)
            }),
        );
        content = content.push(card(self.t("dash_current_operation"), op_text));
        // Read-only text_editor so drag-select + Ctrl+C work. `Length::Fill`
        // height so the editor expands to fill whatever space the parent log
        // card claims — combined with the log card's own `height(Fill)` and
        // the dashboard column's `height(Fill)`, this is what makes the log
        // grow to balance the dashboard's top and bottom padding.
        let dash_log_editor: Element<'_, Message> = iced::widget::text_editor(&self.log_editor)
            .on_action(Message::LogEditorAction)
            .size(11)
            .height(Length::Fill)
            .into();
        // Bottom-right "Save Log" — same neutral pill the wizard exec
        // step uses, sized so the label doesn't get clipped at any
        // language. Right-aligned via a Fill spacer.
        let dash_save_btn = button(
            text(self.t("btn_save_log").to_string())
                .size(11)
                .style(muted_style)
                .center(),
        )
        .on_press(Message::SaveLog)
        .padding([4, 14])
        .style(neutral_pill_btn_style);
        let dash_log_card = container(
            column![
                text(self.t("dash_log").to_string())
                    .size(13)
                    .style(label_style)
                    .line_height(1.0),
                dash_log_editor,
                row![Space::new().width(Length::Fill), dash_save_btn]
                    .align_y(iced::Alignment::Center),
            ]
            .spacing(8)
            .padding(iced::Padding {
                top: 10.0,
                right: 18.0,
                bottom: 14.0,
                left: 18.0,
            })
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|t: &Theme| {
            theme::surface_card_style(t, theme::SurfaceLevel::Default, theme::shape::MD, 1)
        });
        content = content.push(dash_log_card);
        content.into()
    }
}
