//! Flash wizard view + steps (region, target, data, folder, confirm, exec). Extracted from `main.rs`.

use crate::*;
use iced::widget::{self, button, column, container, row, scrollable, text};
use iced::{Element, Length, Theme};

impl App {
    pub(crate) fn view_flash_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.flash.is_in_exec() {
            return self.log_popup_view();
        }
        let step_labels: Vec<&str> = FLASH_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.flash.step);
        let body = match self.flash.step {
            0 => self.flash_region_step(),
            1 => self.flash_target_step(),
            2 => self.flash_data_step(),
            3 => self.flash_folder_step(),
            4 => self.flash_confirm_step(),
            _ => self.flash_exec_step(),
        };
        let nav = if self.flash.step < 5 {
            let is_start = self.flash.step == 4;
            let label_owned = if is_start {
                self.t("btn_start").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            let can = self.flash.can_next()
                && !(self.busy && is_start)
                && (!is_start || self.device_reachable());
            wizard_nav_generic(
                self.flash.step > 0,
                &label_owned,
                can,
                self.t("btn_back"),
                Message::Flash(FlashMsg::FlashBack),
                Message::Flash(FlashMsg::FlashNext),
            )
        } else {
            container(text("")).into()
        };
        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn flash_region_step(&self) -> Element<'_, Message> {
        let prc_icon = lucide_primary(icon::region_prc(), 57.6);
        // TB322FC is a PRC-only SKU. Render ROW as a disabled card with
        // a grayed icon so the constraint is visible — silent skip
        // would confuse users who expect both options.
        let tb322fc = self.is_tb322fc();
        let row_card: Element<'_, Message> = if tb322fc {
            icon_option_card_sub_disabled(
                lucide_disabled(icon::region_row(), 57.6),
                self.t("region_row"),
                self.t("flash_unsupported_tb322fc"),
            )
        } else {
            icon_option_card_sub(
                lucide_primary(icon::region_row(), 57.6),
                self.t("region_row"),
                self.t("region_row_name"),
                self.flash.device_region == Some(DeviceRegion::Row),
                Message::Flash(FlashMsg::FlashRegion(DeviceRegion::Row)),
            )
        };
        let col = column![
            text(self.t("flash_region_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_region_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            row![
                icon_option_card_sub(
                    prc_icon,
                    self.t("region_prc"),
                    self.t("region_prc_name"),
                    self.flash.device_region == Some(DeviceRegion::Prc),
                    Message::Flash(FlashMsg::FlashRegion(DeviceRegion::Prc))
                ),
                row_card,
            ]
            .spacing(12),
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    pub(crate) fn flash_target_step(&self) -> Element<'_, Message> {
        let device = lucide_primary(icon::tile_device(), 57.6);
        // TB322FC ships only in PRC, so cross-region (OtherRegion) is
        // never a valid target. Disable the card with a grayed icon to
        // keep the constraint visible on the picker.
        let tb322fc = self.is_tb322fc();
        let other_card: Element<'_, Message> = if tb322fc {
            icon_option_card_sub_disabled(
                lucide_disabled(icon::tile_globe(), 57.6),
                self.t(FlashTarget::OtherRegion.label_key()),
                self.t("flash_unsupported_tb322fc"),
            )
        } else {
            icon_option_card_sub(
                lucide_primary(icon::tile_globe(), 57.6),
                self.t(FlashTarget::OtherRegion.label_key()),
                self.t("flashtarget_other_desc"),
                self.flash.target == Some(FlashTarget::OtherRegion),
                Message::Flash(FlashMsg::FlashTarget(FlashTarget::OtherRegion)),
            )
        };
        let col = column![
            text(self.t("flash_target_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_target_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            row![
                other_card,
                icon_option_card_sub(
                    device,
                    self.t(FlashTarget::SameRegion.label_key()),
                    self.t("flashtarget_same_desc"),
                    self.flash.target == Some(FlashTarget::SameRegion),
                    Message::Flash(FlashMsg::FlashTarget(FlashTarget::SameRegion))
                ),
            ]
            .spacing(12),
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    pub(crate) fn flash_data_step(&self) -> Element<'_, Message> {
        let shield = lucide_primary(icon::tile_shield(), 57.6);
        let wipe = lucide_primary(icon::tile_wipe(), 57.6);
        let col = column![
            text(self.t("flash_data_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_data_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            row![
                icon_option_card_sub(
                    shield,
                    self.t(DataMode::Keep.label_key()),
                    self.t("datamode_keep_desc"),
                    self.flash.data_mode == Some(DataMode::Keep),
                    Message::Flash(FlashMsg::FlashDataMode(DataMode::Keep))
                ),
                icon_option_card_sub(
                    wipe,
                    self.t(DataMode::Wipe.label_key()),
                    self.t("datamode_wipe_desc"),
                    self.flash.data_mode == Some(DataMode::Wipe),
                    Message::Flash(FlashMsg::FlashDataMode(DataMode::Wipe))
                ),
            ]
            .spacing(12),
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    pub(crate) fn flash_folder_step(&self) -> Element<'_, Message> {
        let selected = self.flash.firmware_folder.is_some();
        let status = if let Some(p) = &self.flash.firmware_folder {
            p.clone()
        } else {
            self.t("flash_folder_placeholder").to_string()
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_browse_folder").to_string())
                        .size(14)
                        .center(),
                    text(self.t("flash_folder_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .on_press(Message::Flash(FlashMsg::FlashSelectFolder))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let chips = self.recent_chips(
            self.recent_paths
                .recent(PickerTarget::FlashFolder.kind().storage_key()),
            |p| Message::RecentFolderPicked(PickerTarget::FlashFolder, p),
            "picker_recents",
            false,
        );
        let col = column![
            text(self.t("flash_folder_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status)
                .size(12)
                .width(Length::Fill)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(if selected { p.success } else { p.outline }),
                    }
                })
                .center()
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            chips,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    pub(crate) fn flash_confirm_step(&self) -> Element<'_, Message> {
        let dash = "—".to_string();
        let region = self
            .flash
            .device_region
            .map(|r| self.t(r.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());
        let target = self
            .flash
            .target
            .map(|t| self.t(t.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());
        let data = self
            .flash
            .data_mode
            .map(|d| self.t(d.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());
        // Confirm rows use short value labels (Modify / Auto / Ignore)
        // instead of the verbose "… rollback index" strings shown in
        // logs — the review summary is tighter to read that way.
        let modify_region = self
            .t(if self.wf_config.modify_region {
                "flash_confirm_rb_on"
            } else {
                "flash_confirm_rb_off"
            })
            .to_string();
        let rollback = self
            .t(match self.wf_config.modify_rollback {
                RollbackSetting::On => "flash_confirm_rb_on",
                RollbackSetting::Auto => "flash_confirm_rb_auto",
                RollbackSetting::Off => "flash_confirm_rb_off",
            })
            .to_string();
        let mut col = column![
            text(self.t("flash_confirm_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_confirm_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            info_kv_center(self.t("flash_confirm_region"), &region),
            info_kv_center(self.t("flash_confirm_target"), &target),
            info_kv_center(self.t("flash_confirm_data"), &data),
            info_kv_center(self.t("flash_confirm_region_edit"), &modify_region),
            info_kv_center(self.t("device_arb"), &rollback),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        if let Some(cc) = self.wf_config.country_action.target() {
            let entry = COUNTRY_CODES.iter().find(|e| e.code == cc);
            let label = entry
                .map(|e| format!("{} — {}", e.code, e.name))
                .unwrap_or_else(|| cc.to_string());
            col = col.push(info_kv_center(self.t("flash_confirm_country"), &label));
        } else if self.wf_config.wipe && self.wf_config.country_action.is_skipped() {
            col = col.push(info_kv_center(
                self.t("flash_confirm_country"),
                self.t("flash_confirm_country_skip"),
            ));
        }
        let folder_owned = self
            .flash
            .firmware_folder
            .clone()
            .unwrap_or_else(|| dash.clone());
        col = col.push(info_kv_center(
            self.t("flash_confirm_folder"),
            &folder_owned,
        ));

        // Destructive-op callout — parity with v2 `_confirm_full_flash_overwrite`.
        // The wizard's Next button is the trigger, so surface the hazard
        // inline instead of trusting the summary alone. Uses the palette's
        // `warning` colour (amber) so it doesn't read as an error/failure.
        let warning_key = if self.wf_config.wipe {
            "flash_confirm_warning_wipe"
        } else {
            "flash_confirm_warning"
        };
        col = col.push(widget::rule::horizontal(1));
        col = col.push(
            text(self.t(warning_key).to_string())
                .size(13)
                .style(warning_style)
                .center(),
        );

        // Wrap in scrollable so the summary can grow past the viewport
        // (e.g. ARB ON + country patch + region modify all push extra
        // info_kv rows). Nav row stays outside this fn — `view_flash_wizard`
        // composes `[step_bar, body, nav]`, so Back / Start stay sticky at
        // the bottom even when content scrolls.
        container(scrollable(col).height(Length::Fill).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn flash_exec_step(&self) -> Element<'_, Message> {
        self.exec_step_view()
    }
}
