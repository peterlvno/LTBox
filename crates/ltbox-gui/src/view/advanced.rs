//! Advanced menu + generic adv wizard views + steps + image-info exec. Extracted from `main.rs`.

use crate::*;
use iced::widget::{self, Space, button, column, container, row, scrollable, text};
use iced::{Element, Length, Theme};

impl App {
    pub(crate) fn view_advanced(&self) -> Element<'_, Message> {
        // Dedicated wizards preempt the grid.
        if self.advanced_wizard_open.is_flash_parts() {
            return self.view_flash_parts_wizard();
        }
        if self.advanced_wizard_open.is_dump_parts() {
            return self.view_dump_parts_wizard();
        }
        if self.advanced_wizard_open.is_dump_phys() {
            return self.view_dump_phys_wizard();
        }
        if self.advanced_wizard_open.is_flash_phys() {
            return self.view_flash_phys_wizard();
        }
        if self.advanced_wizard_open.is_simple_flash() {
            return self.view_simple_flash_wizard();
        }
        if self.adv_wizard.action.is_some() {
            return self.view_adv_wizard();
        }

        let mut content = column![
            text(self.t("nav_advanced").to_string()).size(theme::text_size::TITLE_LARGE),
            widget::rule::horizontal(1),
        ]
        .spacing(14)
        .width(Length::Fill);

        for section in ADV_SECTIONS {
            content = content.push(
                text(self.t(section.title_key).to_string())
                    .size(11)
                    .style(label_style),
            );
            let mut rows = column![].spacing(8);
            for chunk in section.items.chunks(3) {
                let mut r = row![].spacing(8);
                for &item in chunk {
                    r = r.push(adv_grid_btn(item, self.t(item.label_key())));
                }
                for _ in chunk.len()..3 {
                    r = r.push(Space::new().width(Length::Fill));
                }
                rows = rows.push(r);
            }
            content = content.push(rows);
        }

        content.into()
    }

    /// Advanced wizard. PatchDevinfo: source/country/confirm/exec.
    /// Others: source/confirm/exec.
    pub(crate) fn view_adv_wizard(&self) -> Element<'_, Message> {
        let is_exec = self.adv_wizard.step == self.adv_wizard.exec_step();
        if self.log_popup_open && is_exec && !self.adv_wizard.is_image_info() {
            return self.log_popup_view();
        }

        let step_labels: Vec<&str> = self.adv_wizard.steps().iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.adv_wizard.step);

        let needs_country = self.adv_wizard.needs_country();
        let needs_region_target = self.adv_wizard.needs_region_target();
        let is_confirm = self.adv_wizard.is_confirm_step();

        let detect_arb_step0 = matches!(self.adv_wizard.action, Some(AdvAction::DetectArb))
            && self.adv_wizard.step == 0;
        let body: Element<'_, Message> = if is_exec && self.adv_wizard.is_image_info() {
            self.adv_image_info_exec_step()
        } else if is_exec {
            self.exec_step_view()
        } else if detect_arb_step0 {
            self.adv_wiz_detect_arb_step()
        } else if is_confirm {
            self.adv_wiz_confirm_step()
        } else if needs_country && self.adv_wizard.step == 1 {
            self.adv_wiz_country_step()
        } else if needs_region_target && self.adv_wizard.step == 1 {
            self.adv_wiz_region_target_step()
        } else if matches!(self.adv_wizard.action, Some(AdvAction::PatchArb))
            && self.adv_wizard.step == 1
        {
            self.adv_wiz_arb_inspect_step()
        } else {
            self.adv_wiz_source_step()
        };

        let nav: Element<'_, Message> = if is_exec {
            container(text("")).into()
        } else {
            let label = if is_confirm || detect_arb_step0 {
                self.t("btn_start").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            // DetectArb gates Start on either a picked loader (TB320FC
            // path) or no requirement at all (other models — Start is
            // always enabled). Other wizards keep the standard
            // `can_next` check.
            let is_start = is_confirm || detect_arb_step0;
            // Most advanced operations (RegionConvert / PatchDevinfo /
            // PatchArb / RebuildVbmeta) are folder-only and don't need
            // a connected device. The ones that DO touch the device
            // (DetectArb, ConvertXml stays offline so excluded) get
            // the reachability gate at the Start step.
            let needs_device = matches!(self.adv_wizard.action, Some(AdvAction::DetectArb));
            let can = if detect_arb_step0 {
                if self.device_model.eq_ignore_ascii_case("TB320FC") {
                    self.adv_wizard.file_path.is_some()
                } else {
                    true
                }
            } else {
                self.adv_wizard.can_next()
            } && !self.busy
                && (!is_start || !needs_device || self.device_reachable());
            wizard_nav_generic(
                true,
                &label,
                can,
                self.t("btn_back"),
                Message::Adv(AdvMsg::AdvWizBack),
                Message::Adv(AdvMsg::AdvWizNext),
            )
        };

        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Step 0 — Browse tile. Matches Flash/Root folder steps.
    pub(crate) fn adv_wiz_source_step(&self) -> Element<'_, Message> {
        let action = match self.adv_wizard.action {
            Some(a) => a,
            None => return container(text("")).into(),
        };
        let selected = if self.adv_wizard.is_image_info() {
            !self.adv_wizard.file_paths.is_empty()
        } else {
            self.adv_wizard.file_path.is_some()
        };
        let status = if self.adv_wizard.is_image_info() && selected {
            tr_args!(
                "adv_image_info_selected_count",
                count = self.adv_wizard.file_paths.len().to_string()
            )
        } else {
            self.adv_wizard
                .file_path
                .clone()
                .unwrap_or_else(|| self.t("adv_source_placeholder").to_string())
        };
        let browse_key = if self.adv_wizard.is_image_info() {
            "btn_browse_files"
        } else if self.adv_wizard.is_folder_op() {
            "btn_browse_folder"
        } else {
            "btn_browse_file"
        };
        let btn = button(
            container(
                column![
                    text(self.t(browse_key).to_string()).size(14).center(),
                    text(self.t(action.source_desc_key()).to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fixed(280.0))
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(Length::Fixed(280.0))
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .width(Length::Shrink)
        .on_press(Message::Adv(AdvMsg::AdvWizBrowse))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        // Shrink-wrap the 280 px card so the hit area stays tight.
        let btn_row = row![
            Space::new().width(Length::Fill),
            btn,
            Space::new().width(Length::Fill),
        ];
        let status_style = move |t: &Theme| {
            let p = pal_of(t);
            iced::widget::text::Style {
                color: Some(if selected { p.success } else { p.outline }),
            }
        };
        let chips: Element<'_, Message> = if self.adv_wizard.is_image_info() {
            self.recent_file_chips(
                &["img"],
                |p| Message::Adv(AdvMsg::AdvWizBrowseManyDone(Some(vec![p]))),
                "picker_recents",
            )
        } else {
            let kind = self.adv_wizard.picker_kind();
            if kind.is_folder() {
                self.recent_chips(
                    self.recent_paths.recent(kind.storage_key()),
                    |p| Message::Adv(AdvMsg::AdvWizBrowseDone(Some(p))),
                    "picker_recents",
                    false,
                )
            } else {
                let (_, exts) = self.adv_wizard.accepted_exts();
                self.recent_file_chips(
                    exts,
                    |p| Message::Adv(AdvMsg::AdvWizBrowseDone(Some(p))),
                    "picker_recents",
                )
            }
        };
        let col = column![
            text(self.t(action.label_key()).to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn_row,
            text(status)
                .size(12)
                .width(Length::Fill)
                .style(status_style)
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

    /// Step 1 (PatchDevinfo only) — country picker tile; opens the
    /// shared country popup.
    pub(crate) fn adv_wiz_country_step(&self) -> Element<'_, Message> {
        let selected = self.adv_wizard.country.is_some();
        let status = self
            .adv_wizard
            .country
            .clone()
            .unwrap_or_else(|| self.t("adv_country_placeholder").to_string());
        let btn = button(
            container(
                column![
                    text(self.t("btn_pick_country").to_string())
                        .size(14)
                        .center(),
                    text(self.t("adv_country_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fixed(280.0))
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(Length::Fixed(280.0))
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .width(Length::Shrink)
        .on_press(Message::Adv(AdvMsg::AdvWizOpenCountry))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let btn_row = row![
            Space::new().width(Length::Fill),
            btn,
            Space::new().width(Length::Fill),
        ];
        let status_style = move |t: &Theme| {
            let p = pal_of(t);
            iced::widget::text::Style {
                color: Some(if selected { p.success } else { p.outline }),
            }
        };
        let col = column![
            text(self.t("adv_country_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("adv_country_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            btn_row,
            text(status)
                .size(12)
                .width(Length::Fill)
                .style(status_style)
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

    /// Step 1 for `RegionConvert`: card that opens the target picker
    /// popup. Mirrors `adv_wiz_country_step` shape so the wizard
    /// rendering stays consistent with the other "needs option"
    /// flow (PatchDevinfo).
    pub(crate) fn adv_wiz_region_target_step(&self) -> Element<'_, Message> {
        let selected = self.adv_wizard.region_target.is_some();
        let status = match self.adv_wizard.region_target {
            Some(target) => self.t(target.label_key()).to_string(),
            None => self.t("adv_region_target_placeholder").to_string(),
        };
        let btn = button(
            container(
                column![
                    text(self.t("btn_pick_region_target").to_string())
                        .size(14)
                        .center(),
                    text(self.t("adv_region_target_desc").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                ]
                .spacing(6)
                .width(Length::Fixed(280.0))
                .align_x(iced::Alignment::Center),
            )
            .padding([20, 24])
            .width(Length::Fixed(280.0))
            .style(move |t: &Theme| sel_card_style(t, selected)),
        )
        .width(Length::Shrink)
        .on_press(Message::Adv(AdvMsg::AdvWizOpenRegionTarget))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let btn_row = row![
            Space::new().width(Length::Fill),
            btn,
            Space::new().width(Length::Fill),
        ];
        let status_style = move |t: &Theme| {
            let p = pal_of(t);
            iced::widget::text::Style {
                color: Some(if selected { p.success } else { p.outline }),
            }
        };
        let col = column![
            text(self.t("adv_region_target_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("adv_region_target_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            btn_row,
            text(status)
                .size(12)
                .width(Length::Fill)
                .style(status_style)
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

    /// PatchArb inspect step — render boot.img + vbmeta_system.img
    /// rollback indices (decimal + UTC) read from the picked folder so
    /// the user can sanity-check the source before opening the
    /// timestamp popup. Next on this step opens the popup.
    pub(crate) fn adv_wiz_arb_inspect_step(&self) -> Element<'_, Message> {
        let (boot_idx, vbmeta_idx) = self.adv_wizard.arb_inspect.unwrap_or((0, 0));
        let mk_row = |label_key: &'static str, idx: u64| -> Element<'_, Message> {
            let utc = format_unix_timestamp_utc(idx);
            iced::widget::row![
                text(self.t(label_key).to_string())
                    .size(13)
                    .style(muted_style)
                    .width(220),
                text(idx.to_string()).size(13).width(140),
                text(utc).size(12).style(muted_style),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center)
            .into()
        };
        let col = column![
            text(self.t("adv_arb_inspect_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("adv_arb_inspect_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            Space::new().height(8),
            mk_row("adv_arb_inspect_boot", boot_idx),
            mk_row("adv_arb_inspect_vbmeta", vbmeta_idx),
        ]
        .spacing(8)
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

    /// DetectArb step 0. TB320FC needs an EDL loader (the deeper
    /// path falls back to dumping `boot_a` + `vbmeta_system_a` when
    /// stored_rollback_index is missing, so a Firehose loader is
    /// required); other models just see a Start prompt because the
    /// detection runs entirely over fastboot vars.
    pub(crate) fn adv_wiz_detect_arb_step(&self) -> Element<'_, Message> {
        let needs_loader = self.device_model.eq_ignore_ascii_case("TB320FC");
        let title = text(self.t("adv_detect_arb").to_string())
            .size(theme::text_size::WIZARD_STEP_TITLE)
            .center();
        let subtitle_key = if needs_loader {
            "adv_src_detect_arb_loader"
        } else {
            "adv_src_detect_arb_start"
        };
        let subtitle = text(self.t(subtitle_key).to_string())
            .size(13)
            .style(muted_style)
            .center();
        let mut col = column![title, subtitle]
            .spacing(14)
            .padding(28)
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);
        if needs_loader {
            let selected = self.adv_wizard.file_path.is_some();
            let status = self
                .adv_wizard
                .file_path
                .clone()
                .unwrap_or_else(|| self.t("adv_source_placeholder").to_string());
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
                    .width(Length::Fixed(280.0))
                    .align_x(iced::Alignment::Center),
                )
                .padding([20, 24])
                .width(Length::Fixed(280.0))
                .style(move |t: &Theme| sel_card_style(t, selected)),
            )
            .width(Length::Shrink)
            .on_press(Message::Adv(AdvMsg::AdvWizBrowse))
            .padding(0)
            .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
            col = col.push(
                row![
                    Space::new().width(Length::Fill),
                    btn,
                    Space::new().width(Length::Fill),
                ]
                .align_y(iced::Alignment::Center),
            );
            let status_style = move |t: &Theme| {
                let p = pal_of(t);
                iced::widget::text::Style {
                    color: Some(if selected { p.success } else { p.outline }),
                }
            };
            col = col.push(
                text(status)
                    .size(12)
                    .width(Length::Fill)
                    .style(status_style)
                    .center()
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            );
        }
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    /// Confirm step — Next becomes Start.
    pub(crate) fn adv_wiz_confirm_step(&self) -> Element<'_, Message> {
        let action = match self.adv_wizard.action {
            Some(a) => a,
            None => return container(text("")).into(),
        };
        let dash = "—".to_string();
        let path = self
            .adv_wizard
            .file_path
            .clone()
            .unwrap_or_else(|| dash.clone());
        let mut col = column![
            text(self.t(action.label_key()).to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t(action.desc_key()).to_string())
                .size(13)
                .style(muted_style)
                .center(),
            Space::new().height(12),
            info_kv_center(self.t("adv_confirm_source"), &path),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        if self.adv_wizard.needs_country() {
            let code = self.adv_wizard.country.clone().unwrap_or(dash.clone());
            col = col.push(info_kv_center(self.t("adv_confirm_country"), &code));
        }
        if self.adv_wizard.needs_region_target() {
            let label = self
                .adv_wizard
                .region_target
                .map(|r| self.t(r.label_key()).to_string())
                .unwrap_or(dash);
            col = col.push(info_kv_center(self.t("adv_confirm_region_target"), &label));
        }
        if matches!(self.adv_wizard.action, Some(AdvAction::PatchArb))
            && let Some(idx) = self.adv_wizard.arb_index_committed
        {
            let utc = format_unix_timestamp_utc(idx);
            col = col.push(info_kv_center(
                self.t("adv_confirm_arb_index"),
                &format!("{idx}  ({utc})"),
            ));
            if let Some((boot_idx, vbmeta_idx)) = self.adv_wizard.arb_inspect {
                col = col.push(info_kv_center(
                    self.t("adv_arb_inspect_boot"),
                    &format!("{boot_idx} → {idx}"),
                ));
                col = col.push(info_kv_center(
                    self.t("adv_arb_inspect_vbmeta"),
                    &format!("{vbmeta_idx} → {idx}"),
                ));
            }
        }
        container(
            scrollable(col)
                .style(m3_scrollable_style)
                .height(Length::Fill)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    pub(crate) fn adv_image_info_exec_step(&self) -> Element<'_, Message> {
        let action_label = self
            .adv_wizard
            .action
            .map(|a| self.t(a.label_key()).to_string())
            .unwrap_or_else(|| self.t("adv_image_info").to_string());
        let status = if self.busy {
            self.t("exec_executing_title").to_string()
        } else if self.error_msg.is_some() {
            self.t("exec_failed_title").to_string()
        } else {
            self.t("exec_done_title").to_string()
        };
        let is_error = self.error_msg.is_some();
        let is_busy = self.busy;
        let status_color = move |t: &Theme| {
            let p = pal_of(t);
            let color = if is_error {
                p.error
            } else if is_busy {
                p.primary
            } else {
                p.success
            };
            iced::widget::text::Style { color: Some(color) }
        };

        let editor = iced::widget::text_editor(&self.image_info_log_editor)
            .on_action(Message::ImageInfoLogEditorAction)
            .size(11)
            .height(Length::Fill);

        let pill_style = neutral_pill_btn_style;
        let mut buttons = row![
            button(
                text(self.t("btn_save_log").to_string())
                    .size(11)
                    .style(muted_style)
                    .center(),
            )
            .on_press(Message::SaveLog)
            .padding([4, 12])
            .style(pill_style)
        ]
        .spacing(8);

        if !self.busy {
            buttons = buttons.push(
                button(
                    text(self.t("btn_start_over").to_string())
                        .size(11)
                        .style(muted_style)
                        .center(),
                )
                .on_press(Message::StartOver)
                .padding([4, 12])
                .style(pill_style),
            );
        }

        let header = row![
            column![
                text(action_label).size(theme::text_size::TITLE_LARGE),
                text(status).size(12).style(status_color),
            ]
            .spacing(4),
            Space::new().width(Length::Fill),
            buttons,
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);

        let body = column![
            header,
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

    /// Simple Firmware Flash wizard: intro (description) → confirm → exec.
    /// The firmware-folder picker opens on Next from the intro step.
    pub(crate) fn view_simple_flash_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.simple_flash.step >= 2 {
            return self.log_popup_view();
        }
        let step_labels: Vec<&str> = SIMPLE_FLASH_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.simple_flash.step);
        let body: Element<'_, Message> = match self.simple_flash.step {
            0 => self.simple_flash_intro_step(),
            1 => self.simple_flash_confirm_step(),
            _ => self.exec_step_view(),
        };
        let nav = if self.simple_flash.step < 2 {
            let is_start = self.simple_flash.step == 1;
            let label = if is_start {
                self.t("btn_start").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            let can = self.simple_flash.can_next()
                && !(self.busy && is_start)
                && (!is_start || self.device_reachable());
            wizard_nav_generic(
                true,
                &label,
                can,
                self.t("btn_back"),
                if self.simple_flash.step == 0 {
                    // Back on the intro step returns to the Advanced grid.
                    Message::SimpleFlash(SimpleFlashMsg::SimpleFlashClose)
                } else {
                    Message::SimpleFlash(SimpleFlashMsg::SimpleFlashBack)
                },
                Message::SimpleFlash(SimpleFlashMsg::SimpleFlashNext),
            )
        } else {
            container(text("")).into()
        };
        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Intro step — shows the action description; Next opens the folder picker.
    fn simple_flash_intro_step(&self) -> Element<'_, Message> {
        let selected = self.simple_flash.firmware_folder.is_some();
        let status = self
            .simple_flash
            .firmware_folder
            .clone()
            .unwrap_or_else(|| self.t("flash_folder_placeholder").to_string());
        let status_style = move |t: &Theme| {
            let p = pal_of(t);
            iced::widget::text::Style {
                color: Some(if selected { p.success } else { p.outline }),
            }
        };
        let col = column![
            text(self.t("adv_simple_flash").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("adv_simple_flash_desc").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            Space::new().height(8),
            text(self.t("simple_flash_pick_hint").to_string())
                .size(12)
                .style(muted_style)
                .center(),
            text(status)
                .size(12)
                .width(Length::Fill)
                .style(status_style)
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

    /// Confirm step — mirrors the firmware-flash confirm, but with fixed
    /// values: region edit / rollback bypass are OFF, and device region,
    /// flash target, and data-wipe outcome are all "unknown" because Simple
    /// Flash performs no detection or modification (the wipe outcome is
    /// decided solely by the firmware's own rawprogram).
    fn simple_flash_confirm_step(&self) -> Element<'_, Message> {
        let unknown = self.t("common_unknown").to_string();
        let off = self.t("flash_confirm_rb_off").to_string();
        let folder = self
            .simple_flash
            .firmware_folder
            .clone()
            .unwrap_or_else(|| "—".to_string());
        let rows = vec![
            text(self.t("simple_flash_confirm_warning").to_string())
                .size(13)
                .style(warning_style)
                .center()
                .into(),
            widget::rule::horizontal(1).into(),
            info_kv_center(self.t("flash_confirm_region"), &unknown),
            info_kv_center(self.t("flash_confirm_target"), &unknown),
            info_kv_center(self.t("flash_confirm_data"), &unknown),
            info_kv_center(self.t("flash_confirm_region_edit"), &off),
            info_kv_center(self.t("flash_confirm_rollback"), &off),
            info_kv_center(self.t("flash_confirm_folder"), &folder),
        ];
        self.confirm_view(
            "flash_confirm_title",
            self.t("flash_confirm_subtitle").to_string(),
            rows,
        )
    }
}
