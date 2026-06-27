//! System-update wizard view + steps + the shared exec-step view. Extracted from `main.rs`.

use crate::*;
use iced::widget::{Space, button, column, container, row, text};
use iced::{Element, Length, Theme};
use iced_aw::widget::Spinner;
use ltbox_core::tr_args;
impl App {
    pub(crate) fn view_sysupdate_wizard(&self) -> Element<'_, Message> {
        // Exec-step log popup overlay — without this the "Show log" button
        // on the exec card was a no-op for System Update (Flash/Root/Unroot
        // all had it wired; SysUpdate had been missed).
        if self.log_popup_open && self.sysupdate.is_in_exec() {
            return self.log_popup_view();
        }
        let steps = self.sysupdate.steps();
        let step_labels: Vec<&str> = steps.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.sysupdate.step);
        let is_rescue = self.sysupdate.is_rescue();
        let body = if is_rescue {
            match self.sysupdate.step {
                0 => self.sysupdate_action_step(),
                1 => self.sysupdate_rescue_folder_step(),
                2 => self.sysupdate_confirm_step(),
                _ => self.sysupdate_exec_step(),
            }
        } else {
            match self.sysupdate.step {
                0 => self.sysupdate_action_step(),
                1 => self.sysupdate_confirm_step(),
                _ => self.sysupdate_exec_step(),
            }
        };
        let last_nav_step = steps.len() - 2; // Exec step has no nav row.
        let nav = if self.sysupdate.step <= last_nav_step {
            let is_start = self.sysupdate.step == last_nav_step;
            let label_owned = if is_start {
                self.t("btn_start").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            let can = self.sysupdate.can_next()
                && !(self.busy && is_start)
                && (!is_start || self.device_reachable());
            wizard_nav_generic(
                self.sysupdate.step > 0,
                &label_owned,
                can,
                self.t("btn_back"),
                Message::Sys(SysMsg::SysBack),
                Message::Sys(SysMsg::SysNext),
            )
        } else {
            container(text("")).into()
        };
        let mut layout = column![].width(Length::Fill).height(Length::Fill);
        if let Some(header) = self.sysupdate_action_bar() {
            layout = layout.push(header);
        }
        let core: Element<'_, Message> = layout
            .push(step_bar)
            .push(body)
            .push(nav)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        if self.sysupdate.rescue_region_popup_open {
            iced::widget::Stack::with_children(vec![core, self.rescue_region_popup_view()]).into()
        } else {
            core
        }
    }

    fn sysupdate_action_bar(&self) -> Option<Element<'_, Message>> {
        let rescue = self.sysupdate.is_rescue();
        let (title, subtitle) = match (rescue, self.sysupdate.step) {
            (_, 0) => (
                self.t("sysupdate_action_title").to_string(),
                self.t("sysupdate_action_subtitle").to_string(),
            ),
            (true, 1) => (
                self.t("rescue_folder_title").to_string(),
                self.loader_picker_desc(),
            ),
            (true, 2) | (false, 1) => {
                let desc = self
                    .sysupdate
                    .action
                    .map(|a| self.t(a.desc_key()).to_string())
                    .unwrap_or_default();
                (self.t("sysupdate_confirm_title").to_string(), desc)
            }
            _ => return None,
        };
        Some(wizard_action_bar(title, Some(subtitle)))
    }

    pub(crate) fn sysupdate_action_step(&self) -> Element<'_, Message> {
        let side = self.wizard_square_side();
        let off_icon = lucide_primary(icon::tile_update_off(), 57.6);
        let on_icon = lucide_primary(icon::tile_update_on(), 57.6);
        // TB323FU's vendor_boot/vbmeta sit on a different UFS LUN than the
        // Boot Recovery worker targets, so the flow can't run on it — disable
        // the card (alongside the non-Qualcomm platform gate).
        let rescue_disabled = self.platform_supported == Some(false) || self.is_tb323fu();
        // Gray the icon when disabled, matching the other wizards' disabled
        // option cards (region ROW / OtherRegion).
        let rescue_icon = if rescue_disabled {
            lucide_disabled(icon::tile_rescue(), 57.6)
        } else {
            lucide_primary(icon::tile_rescue(), 57.6)
        };
        let mut cards = row![
            icon_option_card_sub_square_sized(
                off_icon,
                self.t(SysUpdateAction::Disable.label_key()),
                self.t(SysUpdateAction::Disable.desc_key()),
                self.sysupdate.action == Some(SysUpdateAction::Disable),
                Message::Sys(SysMsg::SysAction(SysUpdateAction::Disable)),
                side,
            ),
            icon_option_card_sub_square_sized(
                on_icon,
                self.t(SysUpdateAction::Enable.label_key()),
                self.t(SysUpdateAction::Enable.desc_key()),
                self.sysupdate.action == Some(SysUpdateAction::Enable),
                Message::Sys(SysMsg::SysAction(SysUpdateAction::Enable)),
                side,
            ),
        ]
        .spacing(12);
        if rescue_disabled {
            // Disabled rescue card — no on_press, grayed out; still mirrors
            // the sub-row layout of the other tiles with the Qualcomm-required
            // hint so the label sits at the same height.
            let rescue_req = if self.is_tb323fu() {
                tr_args!("model_unsupported", model = "TB323FU")
            } else {
                self.t("sysupdate_rescue_req").to_string()
            };
            let content = column![
                icon_tile(rescue_icon),
                text(self.t("sysupdate_rescue").to_string())
                    .size(13)
                    .width(Length::Fill)
                    .center()
                    .style(label_style),
                text(rescue_req)
                    .size(11)
                    .width(Length::Fill)
                    .center()
                    .style(label_style),
            ]
            .spacing(8)
            .align_x(iced::Alignment::Center);
            cards = cards.push(
                button(
                    container(content)
                        .padding([20, 16])
                        .width(Length::Fixed(side))
                        .height(side)
                        .center_x(side)
                        .center_y(side)
                        .style(|t: &Theme| {
                            theme::surface_card_style(
                                t,
                                theme::SurfaceLevel::Lowest,
                                theme::shape::MD,
                                0,
                            )
                        }),
                )
                .padding(0)
                .width(Length::Fixed(side))
                .style(|t: &Theme, _s| button::Style {
                    background: None,
                    text_color: pal_of(t).on_surface,
                    ..Default::default()
                }),
            );
        } else {
            cards = cards.push(icon_option_card_sub_square_sized(
                rescue_icon,
                self.t(SysUpdateAction::Rescue.label_key()),
                self.t(SysUpdateAction::Rescue.desc_key()),
                self.sysupdate.action == Some(SysUpdateAction::Rescue),
                Message::Sys(SysMsg::SysAction(SysUpdateAction::Rescue)),
                side,
            ));
        }
        let col = column![cards,]
            .spacing(14)
            .padding(28)
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);
        centered_step(col, self.square_step_max_width(3))
    }

    pub(crate) fn sysupdate_confirm_step(&self) -> Element<'_, Message> {
        let dash = "—".to_string();
        let action = self
            .sysupdate
            .action
            .map(|a| self.t(a.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());
        let mut rows = vec![info_kv_center(self.t("sysupdate_step_action"), &action)];
        // Rescue: echo the chosen firmware folder + region so the user
        // confirms exactly what's about to flash.
        if self.sysupdate.is_rescue() {
            let folder = self
                .sysupdate
                .rescue_folder
                .clone()
                .unwrap_or_else(|| dash.clone());
            let region = self
                .sysupdate
                .rescue_region
                .map(|r| self.t(r.label_key()).to_string())
                .unwrap_or_else(|| dash.clone());
            rows.push(info_kv_center(self.t("rescue_folder_label"), &folder));
            rows.push(info_kv_center(self.t("rescue_region_label"), &region));
        }
        self.confirm_rows_view(rows)
    }

    pub(crate) fn sysupdate_rescue_folder_step(&self) -> Element<'_, Message> {
        // Boot Recovery now consumes only the EDL loader file —
        // dump+flash use GPT-by-name on a fixed LUN, no rawprogram*.xml
        // is read. Step layout still matches the flash / root / unroot
        // pickers (title + 280-wide card button + status path + recent
        // chips), just with file-picker semantics.
        let selected = self.sysupdate.rescue_folder.is_some();
        let status = if let Some(p) = &self.sysupdate.rescue_folder {
            p.clone()
        } else {
            self.t("flash_folder_placeholder").to_string()
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
        .on_press(Message::Sys(SysMsg::SysRescueSelectFolder))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        // Loader recents share the File bucket with other loader
        // pickers (root, advanced) — filter to the same ext set the
        // dialog itself accepts.
        let chips = self.recent_file_chips(
            LOADER_PICKER_EXTS,
            |p| Message::Sys(SysMsg::SysRescueFolderChosen(Some(p))),
            "picker_recents",
        );
        let col = column![
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

    pub(crate) fn sysupdate_exec_step(&self) -> Element<'_, Message> {
        self.exec_step_view()
    }

    /// Reusable exec-step view with collapsible log panel.
    pub(crate) fn exec_step_view(&self) -> Element<'_, Message> {
        let (title, detail) = if self.busy {
            (
                self.t("exec_executing_title").to_string(),
                self.t("exec_executing_subtitle").to_string(),
            )
        } else if self.error_msg.is_some() {
            (
                self.t("exec_failed_title").to_string(),
                self.t("exec_failed_subtitle").to_string(),
            )
        } else {
            (
                self.t("exec_done_title").to_string(),
                self.t("exec_done_subtitle").to_string(),
            )
        };
        let is_error = self.error_msg.is_some();
        let is_busy = self.busy;

        // Shared progress/result card for wizard exec steps.
        let step_icon: Element<'_, Message> = if is_error {
            lucide_icon(icon::op_failed(), 72.0, |t: &Theme| pal_of(t).error)
        } else if is_busy {
            container(
                Spinner::new()
                    .width(Length::Fixed(56.0))
                    .height(Length::Fixed(56.0))
                    .circle_radius(3.5),
            )
            .width(72)
            .height(72)
            .center_x(72)
            .center_y(72)
            .style(|t: &Theme| {
                let p = pal_of(t);
                container::Style {
                    text_color: Some(p.primary),
                    ..Default::default()
                }
            })
            .into()
        } else {
            lucide_icon(icon::op_done(), 72.0, |t: &Theme| pal_of(t).success)
        };

        let (eyebrow_text, label_text) = if self.op_steps.is_empty() {
            (String::new(), detail.clone())
        } else {
            let idx = self.current_op_step.min(self.op_steps.len() - 1);
            let total = self.op_steps.len();
            let step = &self.op_steps[idx];
            let eyebrow_key = if is_error {
                "exec_step_eyebrow_failed"
            } else if is_busy {
                "exec_step_eyebrow_running"
            } else {
                "exec_step_eyebrow_done"
            };
            let eyebrow = tr_args!(
                eyebrow_key,
                n = (idx + 1).to_string(),
                total = total.to_string()
            );
            (eyebrow, step.label.clone())
        };

        let eyebrow_node: Element<'_, Message> = if eyebrow_text.is_empty() {
            Space::new().height(0).into()
        } else {
            text(eyebrow_text)
                .size(11)
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    let color = if is_error {
                        p.error
                    } else if is_busy {
                        p.primary
                    } else {
                        p.success
                    };
                    iced::widget::text::Style { color: Some(color) }
                })
                .into()
        };

        let card_body = column![
            eyebrow_node,
            text(label_text).size(16).style(on_surface_style),
        ]
        .spacing(4)
        .width(Length::Fill);
        let card_row = row![step_icon, card_body]
            .spacing(20)
            .align_y(iced::Alignment::Center);
        let step_card = container(card_row)
            .padding([24, 28])
            .max_width(560)
            .width(Length::Fill)
            .style(move |t: &Theme| {
                let p = pal_of(t);
                let accent = if is_error {
                    p.error
                } else if is_busy {
                    p.primary
                } else {
                    p.success
                };
                container::Style {
                    background: Some(p.surface_container.into()),
                    border: iced::Border {
                        color: accent,
                        width: 1.5,
                        radius: theme::shape::MD.into(),
                    },
                    shadow: theme::elevation(2, theme::is_dark(t)),
                    ..Default::default()
                }
            });

        let mut actions = row![
            wizard_surface_fab(
                icon::fab_show_log(),
                self.t("btn_show_log").to_string(),
                Some(Message::ToggleLogPopup(true)),
            ),
            wizard_surface_fab(
                icon::fab_save_log(),
                self.t("btn_save_log").to_string(),
                Some(Message::SaveLog),
            ),
        ]
        .spacing(WIZARD_FAB_SPACING)
        .align_y(iced::Alignment::Center)
        .height(Length::Fill);

        // "Open Folder" FAB for Advanced ops that produce output —
        // guarded on non-busy to avoid racing the file-manager launch.
        if !self.busy
            && self.current_view == View::Advanced
            && self.adv_wizard.output_dir.is_some()
            && self
                .adv_wizard
                .action
                .map(|a| a.produces_output())
                .unwrap_or(false)
        {
            actions = actions.push(wizard_surface_fab(
                icon::fab_open_folder(),
                self.t("btn_open_folder").to_string(),
                Some(Message::Adv(AdvMsg::AdvWizOpenOutputFolder)),
            ));
        }

        if !self.busy {
            actions = actions.push(wizard_error_fab(
                icon::fab_start_over(),
                self.t("btn_start_over").to_string(),
                Some(Message::StartOver),
            ));
        }

        let col = column![
            text(title)
                .size(theme::text_size::WIZARD_STEP_TITLE)
                .center()
                .style(move |t: &Theme| {
                    let p = pal_of(t);
                    let color = if is_error {
                        p.error
                    } else if is_busy {
                        p.primary
                    } else {
                        p.success
                    };
                    iced::widget::text::Style { color: Some(color) }
                }),
            text(detail).size(13).style(muted_style).center(),
            Space::new().height(8),
            step_card,
        ]
        .spacing(10)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);

        let body = container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill);

        column![
            body,
            wizard_fab_footer(row![].height(Length::Fill), actions),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}
