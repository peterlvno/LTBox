//! Root wizard view + steps + superkey/run-id/kernel-version popups. Extracted from `main.rs`.

use crate::*;
use iced::widget::{Space, button, column, container, row, text};
use iced::{Element, Length, Theme};
use ltbox_core::tr_args;
use theme::with_alpha;

impl App {
    pub(crate) fn view_root_wizard(&self) -> Element<'_, Message> {
        // Superkey / Run-ID / Kernel-version popups all render as
        // top-level M3 dialog overlays via `view()`'s layer stack —
        // do NOT early-return for any of them here, otherwise the
        // KPM step underneath would unmount and Cancel couldn't
        // restore the curated list.
        if self.log_popup_open && self.root.is_in_exec() {
            return self.log_popup_view();
        }
        let steps = self.root.active_steps();
        let step_labels: Vec<&str> = steps.iter().map(|k| self.t(k)).collect();
        let step_bar = wizard_step_bar(&step_labels, self.root.display_step());
        let body = match self.root.step {
            0 => self.root_family_step(),
            1 => {
                if self.root.is_skroot() {
                    self.root_skroot_flavor_step()
                } else {
                    self.root_mode_step()
                }
            }
            2 => {
                if self.root.is_gki() {
                    self.root_file_step(self.t("root_kernel_title"), self.t("root_kernel_subtitle"))
                } else {
                    self.root_provider_step()
                }
            }
            3 => {
                if self.root.is_forks() {
                    self.root_file_step(self.t("root_apk_title"), self.t("root_apk_subtitle"))
                } else {
                    self.root_version_step()
                }
            }
            4 => self.root_nightly_source_step(),
            5 => self.root_folder_step(),
            6 => self.root_confirm_step(),
            8 => self.root_kpm_step(),
            _ => self.root_flash_step(),
        };
        // Step 7 is in-progress — no nav. Step 8 (APatch KPM) needs
        // the normal Back/Next bar, so exclude only 7 explicitly.
        let nav = if self.root.step != 7 {
            let is_start = self.root.step == 6;
            let label_owned = if is_start {
                self.t("btn_start").to_string()
            } else {
                self.t("btn_next").to_string()
            };
            let can = self.root.can_next()
                && !(self.busy && is_start)
                && (!is_start || self.device_reachable());
            wizard_nav(self.root.step > 0, &label_owned, can, self.t("btn_back"))
        } else {
            container(text("")).into()
        };
        let mut layout = column![].width(Length::Fill).height(Length::Fill);
        if let Some(header) = self.root_action_bar() {
            layout = layout.push(header);
        }
        layout
            .push(step_bar)
            .push(body)
            .push(nav)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn root_action_bar(&self) -> Option<Element<'_, Message>> {
        let header = match self.root.step {
            0 => (
                self.t("root_type_title").to_string(),
                self.t("root_type_subtitle").to_string(),
            ),
            1 if self.root.is_skroot() => (
                self.t("root_skroot_flavor_title").to_string(),
                self.t("root_skroot_flavor_subtitle").to_string(),
            ),
            1 => {
                let family = self
                    .root
                    .family
                    .map(|f| self.t(f.label_key()))
                    .unwrap_or("?");
                (
                    tr_args!("root_mode_title_tmpl", family = family),
                    self.t("root_mode_subtitle").to_string(),
                )
            }
            2 if self.root.is_gki() => (
                self.t("root_kernel_title").to_string(),
                self.t("root_kernel_subtitle").to_string(),
            ),
            2 => {
                let family = self.root.family.unwrap_or(Family::KernelSU);
                (
                    tr_args!(
                        "root_provider_title_tmpl",
                        family = self.t(family.label_key())
                    ),
                    self.t("root_provider_subtitle").to_string(),
                )
            }
            3 if self.root.is_forks() => (
                self.t("root_apk_title").to_string(),
                self.t("root_apk_subtitle").to_string(),
            ),
            3 => (
                self.t("root_version_title").to_string(),
                self.t("root_version_subtitle").to_string(),
            ),
            4 => (
                self.t("root_source_title").to_string(),
                self.t("root_source_subtitle").to_string(),
            ),
            5 => (
                self.t("root_folder_title").to_string(),
                self.t("root_folder_subtitle").to_string(),
            ),
            6 => (
                self.t("root_confirm_title").to_string(),
                self.t("root_confirm_subtitle").to_string(),
            ),
            8 => (
                self.t("root_kpm_title").to_string(),
                self.t("root_kpm_subtitle").to_string(),
            ),
            _ => return None,
        };
        Some(wizard_action_bar(header.0, Some(header.1)))
    }

    pub(crate) fn root_kpm_step(&self) -> Element<'_, Message> {
        // No recents here — the KPM list already competes for vertical space.
        let kpm_selected = !self.root.kpm_paths.is_empty();
        let pick_btn = button(
            container(
                column![
                    text(self.t("btn_browse_kpm").to_string()).size(14).center(),
                    text(self.t("root_kpm_desc").to_string())
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
            .style(move |t: &Theme| sel_card_style(t, kpm_selected)),
        )
        .on_press(Message::Root(RootMsg::RootSelectKpm))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, kpm_selected));

        let mut list = column![].spacing(4).width(Length::Fill);
        for path in &self.root.kpm_paths {
            let name = std::path::Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            let p_copy = path.clone();
            let remove = button(text("−").size(14))
                .padding([2, 10])
                .on_press(Message::Root(RootMsg::RootKpmRemove(p_copy)))
                .style(|t: &Theme, _s| {
                    let p = pal_of(t);
                    button::Style {
                        background: Some(with_alpha(p.on_surface, 0.10).into()),
                        text_color: p.on_surface,
                        border: iced::Border {
                            radius: 4.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                });
            list = list.push(
                row![remove, text(name).size(12).style(on_surface_style),]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
            );
        }

        let col = column![pick_btn, list,]
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

    pub(crate) fn root_superkey_popup(&self) -> Element<'_, Message> {
        // M3 text-input dialog — same shape as root_run_id_popup /
        // root_kernel_version_popup so the three APatch-flow popups
        // feel consistent (380 wide, outlined Cancel + filled OK,
        // shared `m3_dialog` scrim + 28-radius card).
        let input = iced::widget::text_input(
            self.t("apatch_superkey_placeholder"),
            &self.root.superkey_buffer,
        )
        .on_input(|__v| Message::Root(RootMsg::RootSuperkeyInput(__v)))
        .on_submit(Message::Root(RootMsg::RootSuperkeyConfirm))
        .secure(true)
        .padding([10, 12])
        .width(Length::Fill)
        .style(m3_text_input_style);

        let err: Element<'_, Message> = match &self.error_msg {
            Some(e) => text(e.clone())
                .size(12)
                .style(|t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(p.error),
                    }
                })
                .into(),
            None => Space::new().height(0).into(),
        };

        // Two-stage flow: first-entry vs verification re-entry. The
        // title + subtitle swap so the user knows the first Confirm
        // didn't commit the key yet, plus the password-manager / form
        // autofill heuristics in the OS see "different" prompts.
        let on_verify_stage = self.root.superkey_first_entry.is_some();
        let title_key = if on_verify_stage {
            "apatch_superkey_verify_title"
        } else {
            "apatch_superkey_title"
        };
        let subtitle_key = if on_verify_stage {
            "apatch_superkey_verify_subtitle"
        } else {
            "apatch_superkey_subtitle"
        };

        let content = column![
            text(self.t(title_key).to_string()).size(20),
            text(self.t(subtitle_key).to_string())
                .size(13)
                .style(muted_style),
            input,
            err,
            row![
                Space::new().width(Length::Fill),
                button(text(self.t("btn_cancel").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootSuperkeyCancel))
                    .padding([8, 18])
                    .style(md_text_btn_style),
                button(text(self.t("btn_ok").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootSuperkeyConfirm))
                    .padding([8, 18])
                    .style(md_filled_btn_style),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(14)
        .padding(24)
        .width(380);

        m3_dialog(content.into())
    }

    pub(crate) fn root_run_id_popup(&self) -> Element<'_, Message> {
        // M3 text-input dialog — 380 wide, outlined Cancel + filled OK.
        let input = iced::widget::text_input(
            self.t("nightly_manual_placeholder"),
            &self.root.run_id_buffer,
        )
        .on_input(|__v| Message::Root(RootMsg::RootRunIdInput(__v)))
        .on_submit(Message::Root(RootMsg::RootRunIdConfirm))
        .padding([10, 12])
        .width(Length::Fill)
        .style(m3_text_input_style);

        let err: Element<'_, Message> = match &self.error_msg {
            Some(e) => text(e.clone())
                .size(12)
                .style(|t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(p.error),
                    }
                })
                .into(),
            None => Space::new().height(0).into(),
        };

        let content = column![
            text(self.t("nightly_manual_title").to_string()).size(20),
            text(self.t("nightly_manual_subtitle").to_string())
                .size(13)
                .style(muted_style),
            input,
            err,
            row![
                Space::new().width(Length::Fill),
                button(text(self.t("btn_cancel").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootRunIdCancel))
                    .padding([8, 18])
                    .style(md_text_btn_style),
                button(text(self.t("btn_ok").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootRunIdConfirm))
                    .padding([8, 18])
                    .style(md_filled_btn_style),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(14)
        .padding(24)
        .width(380);

        m3_dialog(content.into())
    }

    pub(crate) fn root_kernel_version_popup(&self) -> Element<'_, Message> {
        let input = iced::widget::text_input(
            self.t("root_kernel_version_placeholder"),
            &self.root.kernel_version_buffer,
        )
        .on_input(|__v| Message::Root(RootMsg::RootKernelVersionInput(__v)))
        .on_submit(Message::Root(RootMsg::RootKernelVersionConfirm))
        .padding([10, 12])
        .width(Length::Fill)
        .style(m3_text_input_style);

        let err: Element<'_, Message> = match &self.error_msg {
            Some(e) => text(e.clone())
                .size(12)
                .style(|t: &Theme| {
                    let p = pal_of(t);
                    iced::widget::text::Style {
                        color: Some(p.error),
                    }
                })
                .into(),
            None => Space::new().height(0).into(),
        };

        let content = column![
            text(self.t("root_kernel_version_manual_title").to_string()).size(20),
            text(self.t("root_kernel_version_manual_subtitle").to_string())
                .size(13)
                .style(muted_style),
            input,
            err,
            row![
                Space::new().width(Length::Fill),
                button(text(self.t("btn_cancel").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootKernelVersionCancel))
                    .padding([8, 18])
                    .style(md_text_btn_style),
                button(text(self.t("btn_ok").to_string()).size(13))
                    .on_press(Message::Root(RootMsg::RootKernelVersionConfirm))
                    .padding([8, 18])
                    .style(md_filled_btn_style),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(14)
        .padding(24)
        .width(380);

        m3_dialog(content.into())
    }

    pub(crate) fn root_family_step(&self) -> Element<'_, Message> {
        // TODO(root): TB320FC has no init_boot for the current Magisk /
        // KernelSU LKM ramdisk-injection path. Replace it with a
        // vendor_boot patch once real-device verification is available.
        // Keep unsupported cards visible but disabled for now; KernelSU
        // remains pickable through GKI, and APatch stays available.
        let tb320fc = self.is_tb320fc();
        let unsupported_tb320fc = tr_args!("model_unsupported", model = "TB320FC");
        let side = ROOT_WIZARD_2X2_CARD_SIDE;
        let icon_size = ROOT_WIZARD_2X2_ICON_SIZE;
        // Compact 1:1 cards keep the 2×2 grid fully visible at the minimum
        // window height without increasing the shell's min-size contract.
        let mk = |f: Family| -> Element<'_, Message> {
            let disabled = tb320fc && f == Family::Magisk;
            if disabled {
                icon_option_card_sub_square_compact_disabled_sized(
                    f.icon_disabled_sized(icon_size),
                    self.t(f.label_key()),
                    &unsupported_tb320fc,
                    side,
                )
            } else {
                icon_option_card_sub_square_compact_sized(
                    f.icon_sized(icon_size),
                    self.t(f.label_key()),
                    self.t(f.desc_key()),
                    self.root.family == Some(f),
                    Message::Root(RootMsg::RootFamily(f)),
                    side,
                )
            }
        };

        // 2×2 grid — four families, two-up per row.
        let families = [
            Family::Magisk,
            Family::KernelSU,
            Family::APatch,
            Family::Skroot,
        ];
        let mut cards = column![]
            .spacing(ROOT_WIZARD_2X2_GRID_GAP)
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);
        for chunk in families.chunks(2) {
            let mut r = row![].spacing(ROOT_WIZARD_2X2_GRID_GAP);
            for &f in chunk {
                r = r.push(mk(f));
            }
            cards = cards.push(r);
        }

        let col = column![cards,]
            .spacing(14)
            .padding(ROOT_WIZARD_2X2_GRID_PADDING)
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);
        centered_step(col, ROOT_WIZARD_2X2_GRID_MAX_WIDTH)
    }

    pub(crate) fn root_provider_step(&self) -> Element<'_, Message> {
        let family = self.root.family.unwrap_or(Family::KernelSU);
        let providers = family.providers();
        let side = self.wizard_square_side();
        let grid_side = ROOT_WIZARD_2X2_CARD_SIDE;
        let grid_icon_size = ROOT_WIZARD_2X2_ICON_SIZE;

        // KernelSU's four providers form a 2×2 grid (vertical, full-width
        // cards, per the grid rule); Magisk / APatch have two and render as
        // a single-row layout, so their options use the 1:1 square cards.
        let is_grid = providers.len() > 2;
        let card = |p: Provider, selected: bool| -> Element<'_, Message> {
            let sub = p.desc_key().map(|k| self.t(k)).unwrap_or("");
            if is_grid {
                icon_option_card_sub_square_compact_sized(
                    p.icon_sized(grid_icon_size),
                    self.t(p.label_key()),
                    sub,
                    selected,
                    Message::Root(RootMsg::RootProvider(p)),
                    grid_side,
                )
            } else {
                // Smaller brand logo (52 vs 72) so the 72px SVG doesn't
                // overflow the fixed 200px square once the label/desc wraps.
                icon_option_card_sub_square_sized(
                    p.icon_sized(52.0),
                    self.t(p.label_key()),
                    sub,
                    selected,
                    Message::Root(RootMsg::RootProvider(p)),
                    side,
                )
            }
        };

        // align_x centred so both the compact 2×2 grid and the shrink-wrapped
        // two-provider square rows stay centred.
        let mut grid = column![]
            .spacing(if is_grid {
                ROOT_WIZARD_2X2_GRID_GAP
            } else {
                10.0
            })
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);
        for chunk in providers.chunks(2) {
            let mut r = row![].spacing(if is_grid {
                ROOT_WIZARD_2X2_GRID_GAP
            } else {
                10.0
            });
            for &p in chunk {
                r = r.push(card(p, self.root.provider == Some(p)));
            }
            if !is_grid && chunk.len() == 1 {
                r = r.push(Space::new().width(Length::Fill));
            }
            grid = grid.push(r);
        }

        let col = column![grid,]
            .spacing(14)
            .padding(if is_grid {
                ROOT_WIZARD_2X2_GRID_PADDING
            } else {
                28.0
            })
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);
        let max_width = if is_grid {
            ROOT_WIZARD_2X2_GRID_MAX_WIDTH
        } else {
            self.square_step_max_width(providers.len())
        };
        centered_step(col, max_width)
    }

    pub(crate) fn root_file_step(&self, _title: &str, subtitle: &str) -> Element<'_, Message> {
        let selected = self.root.file_path.is_some();
        let status_text = if let Some(p) = &self.root.file_path {
            p.clone()
        } else {
            self.t("flash_folder_placeholder").to_string()
        };

        let btn_label = if self.root.is_gki() {
            self.t("btn_browse_kernel_image")
        } else {
            self.t("btn_browse_apk")
        };

        let btn = button(
            container(
                column![
                    text(btn_label.to_string()).size(14).center(),
                    text(subtitle.to_string())
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
        .on_press(Message::Root(RootMsg::RootSelectFile))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));

        // Root OTA file picker flips between AnyKernel3 zip + raw
        // boot.img (GKI route) and provider APK (Magisk fork / APatch
        // manual) — mirror the dialog filter so recents don't surface
        // the wrong family.
        let accepted: &[&str] = if self.root.is_gki() {
            &["zip", "img"]
        } else {
            &["apk"]
        };
        let chips = self.recent_file_chips(
            accepted,
            |p| Message::RecentFilePicked(PickerTarget::RootFile, p),
            "picker_recents",
        );
        let col = column![
            btn,
            text(status_text)
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

    pub(crate) fn root_folder_step(&self) -> Element<'_, Message> {
        // Root pipeline now needs only the EDL loader (`.melf`) — the
        // full firmware folder was dropped when dump/flash stopped
        // depending on `rawprogram*.xml` and started resolving partition
        // names against the device's on-storage GPT. File-pick only.
        let selected = self.root.folder_path.is_some();
        let status = if let Some(p) = &self.root.folder_path {
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
        .on_press(Message::Root(RootMsg::RootSelectFolder))
        .padding(0)
        .style(move |t: &Theme, status| sel_card_btn_style(t, status, selected));
        let chips = self.recent_file_chips(
            LOADER_PICKER_EXTS,
            |p| Message::RecentFilePicked(PickerTarget::RootLoader, p),
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

    pub(crate) fn root_mode_step(&self) -> Element<'_, Message> {
        let side = self.wizard_square_side();
        // TODO(root): TB320FC has no init_boot for the current KernelSU
        // LKM path; replace it with a vendor_boot patch once real-device
        // verification is available. Keep the card disabled for now, but
        // visible so users can see why LKM is unavailable.
        let tb320fc = self.is_tb320fc();
        let tb323fu = self.is_tb323fu();
        let unsupported_tb320fc = tr_args!("model_unsupported", model = "TB320FC");
        let unsupported_tb323fu = tr_args!("model_unsupported", model = "TB323FU");
        let lkm_card: Element<'_, Message> = if tb320fc {
            icon_option_card_sub_square_disabled_sized(
                RootMode::Lkm.icon_disabled(),
                self.t(RootMode::Lkm.label_key()),
                &unsupported_tb320fc,
                side,
            )
        } else {
            icon_option_card_sub_square_sized(
                RootMode::Lkm.icon(),
                self.t(RootMode::Lkm.label_key()),
                self.t(RootMode::Lkm.desc_key()),
                self.root.mode == Some(RootMode::Lkm),
                Message::Root(RootMsg::RootMode(RootMode::Lkm)),
                side,
            )
        };
        // TODO(root): LTBox currently only swaps the boot.img Image for
        // GKI, which corrupts boot on TB323FU. Keep GKI disabled until
        // vbmeta handling is added.
        let gki_card: Element<'_, Message> = if tb323fu {
            icon_option_card_sub_square_disabled_sized(
                RootMode::Gki.icon_disabled(),
                self.t(RootMode::Gki.label_key()),
                &unsupported_tb323fu,
                side,
            )
        } else {
            icon_option_card_sub_square_sized(
                RootMode::Gki.icon(),
                self.t(RootMode::Gki.label_key()),
                self.t(RootMode::Gki.desc_key()),
                self.root.mode == Some(RootMode::Gki),
                Message::Root(RootMsg::RootMode(RootMode::Gki)),
                side,
            )
        };
        let col = column![row![lkm_card, gki_card,].spacing(12),]
            .spacing(14)
            .padding(28)
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);
        centered_step(col, self.square_step_max_width(2))
    }

    pub(crate) fn root_skroot_flavor_step(&self) -> Element<'_, Message> {
        let side = self.wizard_square_side();
        let lite = icon_option_card_sub_square_sized(
            SkrootFlavor::Lite.icon(),
            self.t(SkrootFlavor::Lite.label_key()),
            self.t(SkrootFlavor::Lite.desc_key()),
            self.root.skroot_flavor == Some(SkrootFlavor::Lite),
            Message::Root(RootMsg::RootSkrootFlavor(SkrootFlavor::Lite)),
            side,
        );
        let pro = icon_option_card_sub_square_disabled_sized(
            SkrootFlavor::Pro.icon_disabled(),
            self.t(SkrootFlavor::Pro.label_key()),
            self.t(SkrootFlavor::Pro.desc_key()),
            side,
        );

        let col = column![row![lite, pro].spacing(12),]
            .spacing(14)
            .padding(28)
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);
        centered_step(col, self.square_step_max_width(2))
    }

    pub(crate) fn root_version_step(&self) -> Element<'_, Message> {
        let side = self.wizard_square_side();
        let mk = |choice: VerChoice| -> Element<'_, Message> {
            icon_option_card_sub_square_sized(
                choice.icon(),
                self.t(choice.label_key()),
                self.t(choice.desc_key()),
                self.root.version == Some(choice),
                Message::Root(RootMsg::RootVersion(choice)),
                side,
            )
        };

        // ReSukiSU ships nightlies only — hide the Stable card so users
        // can't pick a channel that has no release assets. Other providers
        // keep both.
        let version_row = if self.root.provider == Some(Provider::ReSukiSU) {
            row![mk(VerChoice::Nightly)].spacing(12)
        } else {
            row![mk(VerChoice::Stable), mk(VerChoice::Nightly)].spacing(12)
        };

        let col = column![version_row,]
            .spacing(14)
            .padding(28)
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);
        let columns = if self.root.provider == Some(Provider::ReSukiSU) {
            1
        } else {
            2
        };
        centered_step(col, self.square_step_max_width(columns))
    }

    pub(crate) fn root_nightly_source_step(&self) -> Element<'_, Message> {
        let side = self.wizard_square_side();
        let mk = |src: NightlySource| -> Element<'_, Message> {
            icon_option_card_sub_square_sized(
                src.icon(),
                self.t(src.label_key()),
                self.t(src.desc_key()),
                self.root.nightly_source == Some(src),
                Message::Root(RootMsg::RootNightlySource(src)),
                side,
            )
        };

        // Committed ManualInput shows a chip beneath the cards; click re-opens.
        let chip: Element<'_, Message> =
            match (self.root.nightly_source, self.root.run_id.as_deref()) {
                (Some(NightlySource::ManualInput), Some(id)) if !id.is_empty() => {
                    let label = tr_args!("nightly_manual_committed", id = id);
                    button(text(label).size(13).style(on_surface_style))
                        .padding([8, 14])
                        .on_press(Message::Root(RootMsg::RootNightlySource(
                            NightlySource::ManualInput,
                        )))
                        .style(|t: &Theme, status| {
                            let p = pal_of(t);
                            let bg_a = match status {
                                button::Status::Hovered => 0.18,
                                _ => 0.10,
                            };
                            button::Style {
                                background: Some(with_alpha(p.on_surface, bg_a).into()),
                                text_color: p.on_surface,
                                border: iced::Border {
                                    radius: 6.0.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        })
                        .into()
                }
                _ => Space::new().height(0).into(),
            };

        let col = column![
            row![
                mk(NightlySource::AutoDetect),
                mk(NightlySource::ManualInput)
            ]
            .spacing(12),
            chip,
        ]
        .spacing(14)
        .padding(28)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center);
        centered_step(col, self.square_step_max_width(2))
    }

    pub(crate) fn root_confirm_step(&self) -> Element<'_, Message> {
        let dash = "—".to_string();
        let fam = self
            .root
            .family
            .map(|f| self.t(f.label_key()).to_string())
            .unwrap_or_else(|| dash.clone());

        let mut rows = vec![info_kv_center(self.t("root_step_type"), &fam)];

        if self.root.is_skroot() {
            let flavor = self
                .root
                .skroot_flavor
                .map(|f| self.t(f.label_key()).to_string())
                .unwrap_or_else(|| dash.clone());
            rows.push(info_kv_center(self.t("root_step_skroot_flavor"), &flavor));
        } else {
            let mode = self
                .root
                .mode
                .map(|m| self.t(m.label_key()).to_string())
                .unwrap_or_else(|| dash.clone());
            rows.push(info_kv_center(self.t("root_step_mode"), &mode));
        }

        if self.root.is_gki() {
            let path = self.root.file_path.clone().unwrap_or_else(|| dash.clone());
            rows.push(info_kv_center(self.t("root_step_kernel"), &path));
        } else if self.root.is_forks() {
            let path = self.root.file_path.clone().unwrap_or_else(|| dash.clone());
            rows.push(info_kv_center(
                self.t("root_step_provider"),
                self.t("provider_magisk_forks"),
            ));
            rows.push(info_kv_center(self.t("root_step_apk"), &path));
        } else if !self.root.is_skroot() {
            let prov = self
                .root
                .provider
                .map(|p| self.t(p.label_key()).to_string())
                .unwrap_or_else(|| dash.clone());
            let ver = self
                .root
                .version
                .map(|v| self.t(v.label_key()).to_string())
                .unwrap_or_else(|| dash.clone());
            rows.push(info_kv_center(self.t("root_step_provider"), &prov));
            rows.push(info_kv_center(self.t("root_step_version"), &ver));
            if self.root.is_nightly() {
                let src = self
                    .root
                    .nightly_source
                    .map(|s| self.t(s.label_key()).to_string())
                    .unwrap_or_else(|| dash.clone());
                rows.push(info_kv_center(self.t("root_step_source"), &src));
                if self.root.nightly_source == Some(NightlySource::ManualInput) {
                    let id = self.root.run_id.clone().unwrap_or_else(|| dash.clone());
                    rows.push(info_kv_center(self.t("nightly_run_id_label"), &id));
                }
            }
        }

        if self.root.is_apatch() {
            // Count only — don't echo paths (noisy) or the superkey (secret).
            let kpm_summary = if self.root.kpm_paths.is_empty() {
                self.t("root_kpm_none").to_string()
            } else {
                tr_args!(
                    "root_kpm_count_tmpl",
                    n = self.root.kpm_paths.len().to_string()
                )
            };
            rows.push(info_kv_center(self.t("root_step_kpm"), &kpm_summary));
        }

        let folder = self
            .root
            .folder_path
            .clone()
            .unwrap_or_else(|| dash.clone());
        rows.push(info_kv_center(self.t("root_step_folder"), &folder));

        self.confirm_rows_view(rows)
    }

    pub(crate) fn root_flash_step(&self) -> Element<'_, Message> {
        self.exec_step_view()
    }
}
