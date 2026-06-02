//! Unroot wizard view + steps. Extracted from `main.rs`.

use crate::*;
use iced::widget::{self, button, column, container, row, scrollable, text};
use iced::{Element, Length, Theme};

impl App {
    pub(crate) fn view_unroot_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.unroot.is_in_exec() {
            return self.log_popup_view();
        }
        let step_labels: Vec<&str> = UNROOT_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.unroot.step);
        let body = match self.unroot.step {
            0 => self.unroot_type_step(),
            1 => self.unroot_loader_step(),
            2 => self.unroot_folder_step(),
            3 => self.unroot_confirm_step(),
            _ => self.unroot_exec_step(),
        };
        let nav = if self.unroot.step < 4 {
            let is_start = self.unroot.step == 3;
            let label_owned = if is_start {
                self.t("btn_start").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            let can = self.unroot.can_next()
                && !(self.busy && is_start)
                && (!is_start || self.device_reachable());
            wizard_nav_generic(
                self.unroot.step > 0,
                &label_owned,
                can,
                self.t("btn_back"),
                Message::Unroot(UnrootMsg::UnrootBack),
                Message::Unroot(UnrootMsg::UnrootNext),
            )
        } else {
            container(text("")).into()
        };
        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn unroot_type_step(&self) -> Element<'_, Message> {
        // Unroot reuses the Lucide puzzle/layers glyphs that the root
        // wizard uses for the LKM/GKI pick — context (title + label)
        // disambiguates.
        let lkm_icon = lucide_primary(icon::root_lkm(), 57.6);
        let gki_icon = lucide_primary(icon::root_gki(), 57.6);
        let col = column![
            text(self.t("unroot_method_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("unroot_method_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            row![
                icon_option_card_sub(
                    lkm_icon,
                    self.t(UnrootType::MagiskLkm.label_key()),
                    self.t(UnrootType::MagiskLkm.desc_key()),
                    self.unroot.unroot_type == Some(UnrootType::MagiskLkm),
                    Message::Unroot(UnrootMsg::SetUnrootType(UnrootType::MagiskLkm))
                ),
                icon_option_card_sub(
                    gki_icon,
                    self.t(UnrootType::APatchGki.label_key()),
                    self.t(UnrootType::APatchGki.desc_key()),
                    self.unroot.unroot_type == Some(UnrootType::APatchGki),
                    Message::Unroot(UnrootMsg::SetUnrootType(UnrootType::APatchGki))
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

    pub(crate) fn unroot_loader_step(&self) -> Element<'_, Message> {
        let selected = self.unroot.loader_path.is_some();
        let status = if let Some(p) = &self.unroot.loader_path {
            p.clone()
        } else {
            self.t("dump_parts_loader_placeholder").to_string()
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_browse_loader").to_string())
                        .size(14)
                        .center(),
                    text(self.loader_picker_desc())
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
        .on_press(Message::Unroot(UnrootMsg::UnrootSelectLoader))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let col = column![
            text(self.t("unroot_loader_title").to_string())
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

    pub(crate) fn unroot_folder_step(&self) -> Element<'_, Message> {
        let selected = self.unroot.folder_path.is_some();
        let desc_owned = self
            .unroot
            .unroot_type
            .map(|t| self.t(t.folder_desc_key()).to_string())
            .unwrap_or_else(|| self.t("unroot_folder_placeholder").to_string());
        let status = if let Some(p) = &self.unroot.folder_path {
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
                    text(desc_owned).size(11).style(muted_style).center(),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(280)
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .on_press(Message::Unroot(UnrootMsg::UnrootSelectFolder))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let chips = self.recent_chips(
            self.recent_paths
                .recent(PickerTarget::UnrootFolder.kind().storage_key()),
            |p| Message::RecentFolderPicked(PickerTarget::UnrootFolder, p),
            "picker_recents",
            false,
        );
        let col = column![
            text(self.t("unroot_folder_title").to_string())
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

    pub(crate) fn unroot_confirm_step(&self) -> Element<'_, Message> {
        let dash = "—".to_string();
        let method = self
            .unroot
            .unroot_type
            .map(|t| self.t(t.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());
        let loader = self
            .unroot
            .loader_path
            .clone()
            .unwrap_or_else(|| dash.clone());
        let folder = self
            .unroot
            .folder_path
            .clone()
            .unwrap_or_else(|| dash.clone());
        let col = column![
            text(self.t("unroot_confirm_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("unroot_confirm_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            info_kv_center(self.t("unroot_step_method"), &method),
            info_kv_center(self.t("unroot_loader_title"), &loader),
            info_kv_center(self.t("unroot_folder_title"), &folder),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(scrollable(col).height(Length::Fill).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn unroot_exec_step(&self) -> Element<'_, Message> {
        self.exec_step_view()
    }
}
