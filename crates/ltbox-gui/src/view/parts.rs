//! Partition + physical-storage dump/flash wizard views + steps. Extracted from `main.rs`.

use crate::*;
use iced::widget::{self, button, column, container, row, scrollable, text};
use iced::{Element, Length, Theme};
use theme::with_alpha;

impl App {
    pub(crate) fn view_flash_parts_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.flash_parts.step >= 3 {
            return self.log_popup_view();
        }

        let step_labels: Vec<&str> = FLASH_PARTS_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.flash_parts.step);

        let body: Element<'_, Message> = match self.flash_parts.step {
            0 => self.flash_parts_loader_step(),
            1 => self.flash_parts_select_step(),
            2 => self.flash_parts_confirm_step(),
            _ => self.exec_step_view(),
        };

        let nav = if self.flash_parts.step < 3 {
            let label = match self.flash_parts.step {
                0 => self.t("btn_scan").to_string(),
                1 => self.t("btn_next").to_string(),
                2 => self.t("btn_start").to_string(),
                _ => self.t("btn_next").to_string(),
            };
            let is_start = self.flash_parts.step == 2 || self.flash_parts.step == 0;
            let can = self.flash_parts.can_next()
                && !(self.busy && is_start)
                && (!is_start || self.device_reachable());
            wizard_nav_generic(
                true,
                &label,
                can,
                self.t("btn_back"),
                if self.flash_parts.step == 0 {
                    Message::FlashParts(FlashPartsMsg::FlashPartsClose)
                } else {
                    Message::FlashParts(FlashPartsMsg::FlashPartsBack)
                },
                Message::FlashParts(FlashPartsMsg::FlashPartsNext),
            )
        } else {
            container(text("")).into()
        };

        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn flash_parts_loader_step(&self) -> Element<'_, Message> {
        let selected = self.flash_parts.loader_path.is_some();
        let status = match (&self.flash_parts.loader_path, &self.flash_parts.scan_error) {
            (_, Some(e)) => format!("⚠ {e}"),
            (Some(p), None) => p.clone(),
            _ => self.t("dump_parts_loader_placeholder").to_string(),
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
        .on_press(Message::FlashParts(FlashPartsMsg::FlashPartsSelectLoader))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let status_color = if self.flash_parts.scan_error.is_some() {
            iced::Color::from_rgb(0.9, 0.2, 0.2)
        } else if selected {
            GREEN
        } else {
            LABEL
        };
        let chips = self.recent_file_chips(
            &["melf", "xml", "x"],
            |p| Message::FlashParts(FlashPartsMsg::FlashPartsLoaderChosen(Some(p))),
            "picker_recents",
        );
        let col = column![
            text(self.t("dump_parts_loader_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status)
                .size(12)
                .width(Length::Fill)
                .color(status_color)
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

    pub(crate) fn flash_parts_select_step(&self) -> Element<'_, Message> {
        let active = self.flash_parts.sort_col;
        let desc = self.flash_parts.sort_desc;
        let mk_msg = |c: PartsSortColumn| Message::FlashParts(FlashPartsMsg::FlashPartsSortBy(c));
        let header = row![
            text(" ").size(11).width(32), // checkbox col
            parts_sort_header(
                self.t("flash_parts_col_lun").to_string(),
                active == PartsSortColumn::Lun,
                desc,
                Length::Fixed(50.0),
                mk_msg(PartsSortColumn::Lun),
            ),
            parts_sort_header(
                self.t("flash_parts_col_label").to_string(),
                active == PartsSortColumn::Label,
                desc,
                Length::FillPortion(3),
                mk_msg(PartsSortColumn::Label),
            ),
            parts_sort_header(
                self.t("flash_parts_col_start").to_string(),
                active == PartsSortColumn::Start,
                desc,
                Length::FillPortion(2),
                mk_msg(PartsSortColumn::Start),
            ),
            parts_sort_header(
                self.t("dump_parts_col_size").to_string(),
                active == PartsSortColumn::Size,
                desc,
                Length::FillPortion(2),
                mk_msg(PartsSortColumn::Size),
            ),
            parts_sort_header(
                self.t("flash_parts_col_file").to_string(),
                active == PartsSortColumn::File,
                desc,
                Length::FillPortion(3),
                mk_msg(PartsSortColumn::File),
            ),
        ]
        .spacing(8)
        .padding([6, 10])
        .align_y(iced::Alignment::Center);

        let mut list = column![header, widget::rule::horizontal(1)].spacing(0);
        for (idx, r) in self.flash_parts.rows.iter().enumerate() {
            // Fixed-width tri-state marker: skip, flash, or erase.
            let marker: Element<'_, Message> = match r.state {
                FlashRowState::Unchecked => iced::widget::checkbox(false)
                    .on_toggle(move |_| {
                        Message::FlashParts(FlashPartsMsg::FlashPartsToggleRow(idx))
                    })
                    .into(),
                FlashRowState::Flash => iced::widget::checkbox(true)
                    .on_toggle(move |_| {
                        Message::FlashParts(FlashPartsMsg::FlashPartsToggleRow(idx))
                    })
                    .into(),
                FlashRowState::Erase => text("⛔")
                    .size(18)
                    .color(iced::Color::from_rgb(0.9, 0.2, 0.2))
                    .into(),
            };
            let marker_btn = button(
                container(marker)
                    .width(32)
                    .height(20)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .padding(0)
            .on_press(Message::FlashParts(FlashPartsMsg::FlashPartsToggleRow(idx)))
            .style(|_t: &Theme, _s| button::Style {
                background: None,
                ..Default::default()
            });

            // Filename column: short display only.
            let file_disp = r
                .file_path
                .as_ref()
                .map(|p| {
                    std::path::Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone())
                })
                .unwrap_or_default();

            let data_row = iced::widget::row![
                container(marker_btn).width(32),
                text(r.lun.to_string()).size(12).width(50),
                text(r.label.clone()).size(12).width(Length::FillPortion(3)),
                text(r.start_sector.to_string())
                    .size(12)
                    .width(Length::FillPortion(2)),
                text(format_bytes_auto(r.size_bytes))
                    .size(12)
                    .width(Length::FillPortion(2)),
                text(file_disp).size(12).width(Length::FillPortion(3)),
            ]
            .spacing(8)
            .padding([4, 10])
            .align_y(iced::Alignment::Center);

            // Tint the whole row by its tri-state so flash/erase pop
            // visually; light/dark both pull from the M3 container roles.
            let row_state = r.state;
            let tinted = container(data_row).width(Length::Fill).style(
                move |t: &Theme| -> container::Style {
                    let p = pal_of(t);
                    let bg = match row_state {
                        FlashRowState::Flash => Some(p.primary_container),
                        FlashRowState::Erase => Some(p.error_container),
                        FlashRowState::Unchecked => None,
                    };
                    container::Style {
                        background: bg.map(iced::Background::Color),
                        ..Default::default()
                    }
                },
            );

            // Whole row is a double-click target for the file picker.
            let clickable = iced::widget::mouse_area(tinted).on_double_click(Message::FlashParts(
                FlashPartsMsg::FlashPartsPickRowFile(idx),
            ));
            list = list.push(clickable);
        }

        let scrolled = scrollable(list).height(Length::Fill).width(Length::Fill);

        let col = column![
            text(self.t("flash_parts_select_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_parts_select_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            scrolled,
        ]
        .spacing(10)
        .padding(20)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn flash_parts_confirm_step(&self) -> Element<'_, Message> {
        let rows = self.flash_parts.active_rows();
        let erase_rows: Vec<&FlashPartRow> = rows
            .iter()
            .filter(|r| r.state == FlashRowState::Erase)
            .collect();
        let flash_rows: Vec<&FlashPartRow> = rows
            .iter()
            .filter(|r| r.state == FlashRowState::Flash)
            .collect();

        let mut col = column![
            text(self.t("flash_parts_confirm_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_parts_confirm_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);

        // ERASE block first, red and loud.
        if !erase_rows.is_empty() {
            let red = iced::Color::from_rgb(0.9, 0.2, 0.2);
            let mut erase_col = column![
                text(self.t("flash_parts_confirm_erase_warn").to_string())
                    .size(14)
                    .color(red)
            ]
            .spacing(4);
            for r in &erase_rows {
                erase_col = erase_col.push(
                    text(format!(
                        "⛔ {} (LUN {}, {})",
                        r.label,
                        r.lun,
                        format_bytes_auto(r.size_bytes)
                    ))
                    .size(13)
                    .color(red),
                );
            }
            let erase_card =
                container(erase_col)
                    .padding(14)
                    .style(move |t: &Theme| container::Style {
                        background: Some(iced::Background::Color(with_alpha(red, 0.12))),
                        border: iced::Border {
                            color: red,
                            width: 1.0,
                            radius: 8.0.into(),
                        },
                        text_color: Some(pal_of(t).on_surface),
                        ..Default::default()
                    });
            col = col.push(erase_card);
        }

        // FLASH block.
        if !flash_rows.is_empty() {
            let mut flash_col = column![
                text(self.t("flash_parts_confirm_flash_hdr").to_string())
                    .size(14)
                    .style(on_surface_style)
            ]
            .spacing(4);
            for r in &flash_rows {
                let fname = r
                    .file_path
                    .as_ref()
                    .map(|p| {
                        std::path::Path::new(p)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| p.clone())
                    })
                    .unwrap_or_default();
                flash_col = flash_col.push(
                    text(format!("• {} (LUN {}) ← {}", r.label, r.lun, fname))
                        .size(12)
                        .style(muted_style),
                );
            }
            col = col.push(container(flash_col).padding(14).width(Length::Fill));
        }

        container(scrollable(col).height(Length::Fill).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn view_dump_parts_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.dump_parts.step >= 2 {
            return self.log_popup_view();
        }

        let step_labels: Vec<&str> = DUMP_PARTS_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.dump_parts.step);

        let body: Element<'_, Message> = match self.dump_parts.step {
            0 => self.dump_parts_loader_step(),
            1 => self.dump_parts_select_step(),
            _ => self.exec_step_view(),
        };

        let nav = if self.dump_parts.step < 2 {
            let is_dump_step = self.dump_parts.step == 1;
            let label = if is_dump_step {
                self.t("btn_dump").to_string()
            } else {
                self.t("btn_scan").to_string()
            };
            // DumpParts touches EDL on both Scan (step 0) and Dump
            // (step 1) — both spawn workers that talk to the device.
            // Gate both buttons on reachability.
            let can = self.dump_parts.can_next() && !self.busy && self.device_reachable();
            wizard_nav_generic(
                true,
                &label,
                can,
                self.t("btn_back"),
                if self.dump_parts.step == 0 {
                    Message::DumpParts(DumpPartsMsg::DumpPartsClose)
                } else {
                    Message::DumpParts(DumpPartsMsg::DumpPartsBack)
                },
                Message::DumpParts(DumpPartsMsg::DumpPartsNext),
            )
        } else {
            container(text("")).into()
        };

        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn dump_parts_loader_step(&self) -> Element<'_, Message> {
        let selected = self.dump_parts.loader_path.is_some();
        let status = match (&self.dump_parts.loader_path, &self.dump_parts.scan_error) {
            (_, Some(e)) => format!("⚠ {e}"),
            (Some(p), None) => p.clone(),
            _ => self.t("dump_parts_loader_placeholder").to_string(),
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
        .on_press(Message::DumpParts(DumpPartsMsg::DumpPartsSelectLoader))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let status_color = if self.dump_parts.scan_error.is_some() {
            iced::Color::from_rgb(0.9, 0.2, 0.2)
        } else if selected {
            GREEN
        } else {
            LABEL
        };
        let chips = self.recent_file_chips(
            &["melf", "xml", "x"],
            |p| Message::DumpParts(DumpPartsMsg::DumpPartsLoaderChosen(Some(p))),
            "picker_recents",
        );
        let col = column![
            text(self.t("dump_parts_loader_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status)
                .size(12)
                .width(Length::Fill)
                .color(status_color)
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

    pub(crate) fn dump_parts_select_step(&self) -> Element<'_, Message> {
        let active = self.dump_parts.sort_col;
        let desc = self.dump_parts.sort_desc;
        let mk_msg = |c: PartsSortColumn| Message::DumpParts(DumpPartsMsg::DumpPartsSortBy(c));
        // Header select-all: checked iff every row is selected (and there
        // is at least one row). Click flips toward whichever direction
        // would change state for the majority — full-select if any are
        // unchecked, else clear.
        let all_checked =
            !self.dump_parts.rows.is_empty() && self.dump_parts.rows.iter().all(|r| r.selected);
        let header_cb = iced::widget::checkbox(all_checked)
            .on_toggle(|_| Message::DumpParts(DumpPartsMsg::DumpPartsToggleAll));
        let header = row![
            container(header_cb).width(32),
            parts_sort_header(
                self.t("flash_parts_col_lun").to_string(),
                active == PartsSortColumn::Lun,
                desc,
                Length::Fixed(50.0),
                mk_msg(PartsSortColumn::Lun),
            ),
            parts_sort_header(
                self.t("flash_parts_col_label").to_string(),
                active == PartsSortColumn::Label,
                desc,
                Length::FillPortion(3),
                mk_msg(PartsSortColumn::Label),
            ),
            parts_sort_header(
                self.t("flash_parts_col_start").to_string(),
                active == PartsSortColumn::Start,
                desc,
                Length::FillPortion(2),
                mk_msg(PartsSortColumn::Start),
            ),
            parts_sort_header(
                self.t("dump_parts_col_size").to_string(),
                active == PartsSortColumn::Size,
                desc,
                Length::FillPortion(2),
                mk_msg(PartsSortColumn::Size),
            ),
        ]
        .spacing(8)
        .padding([6, 10])
        .align_y(iced::Alignment::Center);

        let mut list = column![header, widget::rule::horizontal(1)].spacing(0);
        for (idx, row) in self.dump_parts.rows.iter().enumerate() {
            let cb = iced::widget::checkbox(row.selected)
                .on_toggle(move |_| Message::DumpParts(DumpPartsMsg::DumpPartsToggleRow(idx)));
            let data_row = iced::widget::row![
                container(cb).width(32),
                text(row.lun.to_string()).size(12).width(50),
                text(row.label.clone())
                    .size(12)
                    .width(Length::FillPortion(3)),
                text(row.start_sector.to_string())
                    .size(12)
                    .width(Length::FillPortion(2)),
                text(format_bytes_auto(row.size_bytes))
                    .size(12)
                    .width(Length::FillPortion(2)),
            ]
            .spacing(8)
            .padding([4, 10])
            .align_y(iced::Alignment::Center);
            // Tint selected rows so the dump set is visible at a glance.
            let selected = row.selected;
            let tinted = container(data_row).width(Length::Fill).style(
                move |t: &Theme| -> container::Style {
                    let p = pal_of(t);
                    container::Style {
                        background: if selected {
                            Some(iced::Background::Color(p.primary_container))
                        } else {
                            None
                        },
                        ..Default::default()
                    }
                },
            );
            list = list.push(tinted);
        }

        let scrolled = scrollable(list).height(Length::Fill).width(Length::Fill);

        let col = column![
            text(self.t("dump_parts_select_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("dump_parts_select_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            scrolled,
        ]
        .spacing(10)
        .padding(20)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn view_dump_phys_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.dump_phys.step >= 2 {
            return self.log_popup_view();
        }

        let step_labels: Vec<&str> = DUMP_PHYS_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.dump_phys.step);

        let body: Element<'_, Message> = match self.dump_phys.step {
            0 => self.dump_phys_loader_step(),
            1 => self.dump_phys_select_step(),
            _ => self.exec_step_view(),
        };

        let nav = if self.dump_phys.step < 2 {
            let is_dump_step = self.dump_phys.step == 1;
            let label = if is_dump_step {
                self.t("btn_dump").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            // DumpPhys talks to EDL — gate both Scan + Dump on a
            // reachable device.
            let can = self.dump_phys.can_next() && !self.busy && self.device_reachable();
            wizard_nav_generic(
                true,
                &label,
                can,
                self.t("btn_back"),
                if self.dump_phys.step == 0 {
                    Message::DumpPhys(DumpPhysMsg::DumpPhysClose)
                } else {
                    Message::DumpPhys(DumpPhysMsg::DumpPhysBack)
                },
                Message::DumpPhys(DumpPhysMsg::DumpPhysNext),
            )
        } else {
            container(text("")).into()
        };

        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn dump_phys_loader_step(&self) -> Element<'_, Message> {
        let selected = self.dump_phys.loader_path.is_some();
        let status = match (&self.dump_phys.loader_path, &self.dump_phys.loader_error) {
            (_, Some(e)) => format!("⚠ {e}"),
            (Some(p), None) => p.clone(),
            _ => self.t("dump_parts_loader_placeholder").to_string(),
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
        .on_press(Message::DumpPhys(DumpPhysMsg::DumpPhysSelectLoader))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let status_color = if self.dump_phys.loader_error.is_some() {
            iced::Color::from_rgb(0.9, 0.2, 0.2)
        } else if selected {
            GREEN
        } else {
            LABEL
        };
        let chips = self.recent_file_chips(
            &["melf", "xml", "x"],
            |p| Message::DumpPhys(DumpPhysMsg::DumpPhysLoaderChosen(Some(p))),
            "picker_recents",
        );
        let col = column![
            text(self.t("dump_parts_loader_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status)
                .size(12)
                .width(Length::Fill)
                .color(status_color)
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

    pub(crate) fn dump_phys_select_step(&self) -> Element<'_, Message> {
        let header = row![
            text(" ").size(11).width(32),
            text(self.t("phys_col_storage").to_string())
                .size(11)
                .width(Length::Fill)
                .style(muted_style),
        ]
        .spacing(8)
        .padding([6, 10])
        .align_y(iced::Alignment::Center);

        let mut list = column![header, widget::rule::horizontal(1)].spacing(0);
        for idx in 0..PHYS_LUN_COUNT {
            let checked = self.dump_phys.selected[idx];
            let cb = iced::widget::checkbox(checked)
                .on_toggle(move |_| Message::DumpPhys(DumpPhysMsg::DumpPhysToggleRow(idx)));
            let data_row = iced::widget::row![
                container(cb).width(32),
                text(format!("LUN {idx}")).size(12).width(Length::Fill),
            ]
            .spacing(8)
            .padding([4, 10])
            .align_y(iced::Alignment::Center);
            list = list.push(data_row);
        }

        let scrolled = scrollable(list).height(Length::Fill).width(Length::Fill);

        let col = column![
            text(self.t("phys_select_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("phys_select_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            scrolled,
        ]
        .spacing(10)
        .padding(20)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn view_flash_phys_wizard(&self) -> Element<'_, Message> {
        if self.log_popup_open && self.flash_phys.step >= 3 {
            return self.log_popup_view();
        }

        let step_labels: Vec<&str> = FLASH_PHYS_STEPS.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.flash_phys.step);

        let body: Element<'_, Message> = match self.flash_phys.step {
            0 => self.flash_phys_loader_step(),
            1 => self.flash_phys_select_step(),
            2 => self.flash_phys_confirm_step(),
            _ => self.exec_step_view(),
        };

        let nav = if self.flash_phys.step < 3 {
            let label = match self.flash_phys.step {
                0 => self.t("btn_next").to_string(),
                1 => self.t("btn_next").to_string(),
                2 => self.t("btn_start").to_string(),
                _ => self.t("btn_next").to_string(),
            };
            let is_start = self.flash_phys.step == 2;
            let can = self.flash_phys.can_next()
                && !(self.busy && is_start)
                && (!is_start || self.device_reachable());
            wizard_nav_generic(
                true,
                &label,
                can,
                self.t("btn_back"),
                if self.flash_phys.step == 0 {
                    Message::FlashPhys(FlashPhysMsg::FlashPhysClose)
                } else {
                    Message::FlashPhys(FlashPhysMsg::FlashPhysBack)
                },
                Message::FlashPhys(FlashPhysMsg::FlashPhysNext),
            )
        } else {
            container(text("")).into()
        };

        column![step_bar, body, nav]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn flash_phys_loader_step(&self) -> Element<'_, Message> {
        let selected = self.flash_phys.loader_path.is_some();
        let status = match (&self.flash_phys.loader_path, &self.flash_phys.loader_error) {
            (_, Some(e)) => format!("⚠ {e}"),
            (Some(p), None) => p.clone(),
            _ => self.t("dump_parts_loader_placeholder").to_string(),
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
        .on_press(Message::FlashPhys(FlashPhysMsg::FlashPhysSelectLoader))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let status_color = if self.flash_phys.loader_error.is_some() {
            iced::Color::from_rgb(0.9, 0.2, 0.2)
        } else if selected {
            GREEN
        } else {
            LABEL
        };
        let chips = self.recent_file_chips(
            &["melf", "xml", "x"],
            |p| Message::FlashPhys(FlashPhysMsg::FlashPhysLoaderChosen(Some(p))),
            "picker_recents",
        );
        let col = column![
            text(self.t("dump_parts_loader_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            btn,
            text(status)
                .size(12)
                .width(Length::Fill)
                .color(status_color)
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

    pub(crate) fn flash_phys_select_step(&self) -> Element<'_, Message> {
        let header = row![
            text(" ").size(11).width(32),
            text(self.t("phys_col_storage").to_string())
                .size(11)
                .width(Length::FillPortion(2))
                .style(muted_style),
            text(self.t("flash_parts_col_file").to_string())
                .size(11)
                .width(Length::FillPortion(3))
                .style(muted_style),
        ]
        .spacing(8)
        .padding([6, 10])
        .align_y(iced::Alignment::Center);

        let mut list = column![header, widget::rule::horizontal(1)].spacing(0);
        for idx in 0..PHYS_LUN_COUNT {
            let checked = self.flash_phys.selected[idx];
            let cb = iced::widget::checkbox(checked)
                .on_toggle(move |_| Message::FlashPhys(FlashPhysMsg::FlashPhysToggleRow(idx)));

            let file_disp = self.flash_phys.file_paths[idx]
                .as_ref()
                .map(|p| {
                    std::path::Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone())
                })
                .unwrap_or_default();

            let data_row = iced::widget::row![
                container(cb).width(32),
                text(format!("LUN {idx}"))
                    .size(12)
                    .width(Length::FillPortion(2)),
                text(file_disp).size(12).width(Length::FillPortion(3)),
            ]
            .spacing(8)
            .padding([4, 10])
            .align_y(iced::Alignment::Center);

            let clickable = iced::widget::mouse_area(data_row)
                .on_double_click(Message::FlashPhys(FlashPhysMsg::FlashPhysPickRowFile(idx)));
            list = list.push(clickable);
        }

        let scrolled = scrollable(list).height(Length::Fill).width(Length::Fill);

        let col = column![
            text(self.t("phys_select_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_phys_select_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
            scrolled,
        ]
        .spacing(10)
        .padding(20)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn flash_phys_confirm_step(&self) -> Element<'_, Message> {
        let pairs = self.flash_phys.active_pairs();

        let mut col = column![
            text(self.t("flash_parts_confirm_title").to_string())
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center(),
            text(self.t("flash_phys_confirm_subtitle").to_string())
                .size(13)
                .style(muted_style)
                .center(),
            widget::rule::horizontal(1),
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);

        if !pairs.is_empty() {
            let mut list = column![
                text(self.t("flash_parts_confirm_flash_hdr").to_string())
                    .size(14)
                    .style(on_surface_style)
            ]
            .spacing(4);
            for (lun, path) in &pairs {
                let fname = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                list = list.push(
                    text(format!("• LUN {lun} ← {fname}"))
                        .size(12)
                        .style(muted_style),
                );
            }
            col = col.push(container(list).padding(14).width(Length::Fill));
        }

        container(scrollable(col).height(Length::Fill).width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
