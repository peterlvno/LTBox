//! App shell: root view dispatcher, titlebar, sidebar, content frame, banners, toast, dialogs. Extracted from `main.rs`.

use crate::*;
use iced::widget::{self, Space, button, column, container, row, scrollable, text};
use iced::{Element, Length, Theme};
use iced_aw::widget::Spinner;

impl App {
    pub(crate) fn view(&self) -> Element<'_, Message> {
        self.sync_runtime_theme();
        let mut main = column![];
        main = main.push(self.title_bar());
        main = main.push(widget::rule::horizontal(1).style(shell_rule_style));
        // Sidebar floats in Stack over a fixed rail placeholder so
        // content never reflows during tween.
        let rail_placeholder = container(iced::widget::Space::new())
            .width(Length::Fixed(SIDEBAR_RAIL_WIDTH))
            .height(Length::Fill);
        let row_base = row![rail_placeholder, self.content()].height(Length::Fill);
        let row_area = iced::widget::Stack::with_children(vec![row_base.into(), self.sidebar()])
            .width(Length::Fill)
            .height(Length::Fill);
        main = main.push(row_area);
        main = main.push(self.status_bar());

        // 1-px outline + 1-px inset so children don't overpaint border.
        let framed = container(main)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(1)
            .style(|t: &Theme| container::Style {
                border: iced::Border {
                    color: pal_of(t).outline_variant,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            });

        // Error banner below popups so the scrim dims the banner too.
        let mut layers: Vec<Element<'_, Message>> = vec![framed.into()];

        if let Some(err) = &self.error_msg {
            layers.push(self.error_banner(err));
        }
        if self.country_popup_open {
            layers.push(self.country_popup_view());
        }
        if self.region_target_popup_open {
            layers.push(self.region_target_popup_view());
        }
        if let Some(t) = self.reboot_confirm_target {
            layers.push(self.reboot_confirm_popup(t));
        }
        if self.root.run_id_popup_open {
            layers.push(self.root_run_id_popup());
        }
        if self.root.kernel_version_popup_open {
            layers.push(self.root_kernel_version_popup());
        }
        if self.root.superkey_popup_open {
            layers.push(self.root_superkey_popup());
        }
        if self.should_show_busy_progress_dialog() {
            layers.push(self.busy_progress_dialog());
        }
        if self.device_info_popup.is_some() {
            layers.push(self.device_info_popup_view());
        }
        if self.ota_popup.is_some() {
            layers.push(self.ota_popup_view());
        }
        if self.arb_index_popup_open {
            layers.push(self.arb_index_popup_view());
        }
        if self.toast_msg.is_some() {
            layers.push(self.toast_view());
        }

        // Resize handles last so the 4px/8px hit areas at the window
        // edges and corners sit above every popup/toast — the user can
        // still grab the border while a dialog is open. Events outside
        // each handle's bounding box pass through to the layers below
        // so normal UI clicks aren't intercepted.
        layers.push(self.resize_handles());

        iced::widget::Stack::with_children(layers).into()
    }

    pub(crate) fn title_bar(&self) -> Element<'_, Message> {
        let title_content = container(
            row![
                iced::widget::image(TITLE_BAR_ICON_HANDLE.clone())
                    .width(16)
                    .height(16),
                text("LTBox").size(12).style(muted_style),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .padding([8, 12])
        .width(Length::Fill);

        let drag_area = iced::widget::mouse_area(title_content)
            .on_press(Message::Window(WindowMsg::WindowDrag))
            .on_double_click(Message::Window(WindowMsg::WindowToggleMaximize));

        let btn_w = 46;
        let btn_h = 32;

        let minimize_btn = button(
            container(lucide_icon(icon::win_minimize(), 12.0, |t: &Theme| {
                pal_of(t).on_surface
            }))
            .width(btn_w)
            .height(btn_h)
            .center_x(btn_w)
            .center_y(btn_h),
        )
        .on_press(Message::Window(WindowMsg::WindowMinimize))
        .padding(0)
        .style(|_t: &Theme, status| {
            let hover = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hover {
                    Some(iced::Color::from_rgba(0.5, 0.5, 0.5, 0.15).into())
                } else {
                    None
                },
                ..Default::default()
            }
        });

        let maximize_btn = button(
            container(lucide_icon(icon::win_maximize(), 12.0, |t: &Theme| {
                pal_of(t).on_surface
            }))
            .width(btn_w)
            .height(btn_h)
            .center_x(btn_w)
            .center_y(btn_h),
        )
        .on_press(Message::Window(WindowMsg::WindowToggleMaximize))
        .padding(0)
        .style(|_t: &Theme, status| {
            let hover = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hover {
                    Some(iced::Color::from_rgba(0.5, 0.5, 0.5, 0.15).into())
                } else {
                    None
                },
                ..Default::default()
            }
        });

        let close_btn = button(
            container(lucide_icon(icon::win_close(), 12.0, |t: &Theme| {
                pal_of(t).on_surface
            }))
            .width(btn_w)
            .height(btn_h)
            .center_x(btn_w)
            .center_y(btn_h),
        )
        .on_press(Message::Window(WindowMsg::WindowClose))
        .padding(0)
        .style(|_t: &Theme, status| {
            let hover = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hover {
                    Some(iced::Color::from_rgb(0.9, 0.2, 0.2).into())
                } else {
                    None
                },
                ..Default::default()
            }
        });

        container(
            row![drag_area, minimize_btn, maximize_btn, close_btn,]
                .align_y(iced::Alignment::Center)
                .height(btn_h),
        )
        .width(Length::Fill)
        .style(|t: &Theme| container::Style {
            background: Some(pal_of(t).surface_container_low.into()),
            ..Default::default()
        })
        .into()
    }

    /// Invisible edge/corner handles for the borderless window.
    pub(crate) fn resize_handles(&self) -> Element<'_, Message> {
        const EDGE: f32 = 4.0;
        const CORNER: f32 = 8.0;

        // Build one positioned, transparent handle.
        // `dir`: which window edge / corner this handle resizes.
        // `w` / `h`: handle hit-area size.
        // `x` / `y`: alignment of the handle inside the Fill outer.
        // `interaction`: cursor to show on hover.
        let handle = |dir: iced::window::Direction,
                      w: Length,
                      h: Length,
                      x: iced::alignment::Horizontal,
                      y: iced::alignment::Vertical,
                      interaction: iced::mouse::Interaction|
         -> Element<'_, Message> {
            let hit = container(iced::widget::Space::new()).width(w).height(h);
            let area = iced::widget::mouse_area(hit)
                .on_press(Message::Window(WindowMsg::WindowResize(dir)))
                .interaction(interaction);
            container(area)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(x)
                .align_y(y)
                .into()
        };

        use iced::alignment::{Horizontal, Vertical};
        use iced::mouse::Interaction;
        use iced::window::Direction;
        let edges: Vec<Element<'_, Message>> = vec![
            // Edges first (lower z) so corners can overlap them.
            handle(
                Direction::North,
                Length::Fill,
                Length::Fixed(EDGE),
                Horizontal::Center,
                Vertical::Top,
                Interaction::ResizingVertically,
            ),
            handle(
                Direction::South,
                Length::Fill,
                Length::Fixed(EDGE),
                Horizontal::Center,
                Vertical::Bottom,
                Interaction::ResizingVertically,
            ),
            handle(
                Direction::West,
                Length::Fixed(EDGE),
                Length::Fill,
                Horizontal::Left,
                Vertical::Center,
                Interaction::ResizingHorizontally,
            ),
            handle(
                Direction::East,
                Length::Fixed(EDGE),
                Length::Fill,
                Horizontal::Right,
                Vertical::Center,
                Interaction::ResizingHorizontally,
            ),
            // Corners on top so the diagonal cursor + diagonal resize
            // win at the actual corner pixels.
            handle(
                Direction::NorthWest,
                Length::Fixed(CORNER),
                Length::Fixed(CORNER),
                Horizontal::Left,
                Vertical::Top,
                Interaction::ResizingDiagonallyDown,
            ),
            handle(
                Direction::NorthEast,
                Length::Fixed(CORNER),
                Length::Fixed(CORNER),
                Horizontal::Right,
                Vertical::Top,
                Interaction::ResizingDiagonallyUp,
            ),
            handle(
                Direction::SouthWest,
                Length::Fixed(CORNER),
                Length::Fixed(CORNER),
                Horizontal::Left,
                Vertical::Bottom,
                Interaction::ResizingDiagonallyUp,
            ),
            handle(
                Direction::SouthEast,
                Length::Fixed(CORNER),
                Length::Fixed(CORNER),
                Horizontal::Right,
                Vertical::Bottom,
                Interaction::ResizingDiagonallyDown,
            ),
        ];
        iced::widget::Stack::with_children(edges)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn sidebar(&self) -> Element<'_, Message> {
        // Label opacity tween — mounts at 40% width so there's room
        // for glyphs to land, fades in via ease-out-cubic to 100% at
        // the spring's settle point. Width and opacity ride the same
        // spring so visual coherence holds across the whole animation.
        let label_t = ((self.sidebar_anim - 0.4) / 0.5).clamp(0.0, 1.0);
        let label_alpha = ease_out_cubic(label_t);
        let mut col = column![].spacing(1).padding([16, 0]);
        for &v in NAV_MAIN {
            col = col.push(nav_btn(
                v,
                self.t(v.sidebar_label_key()),
                self.current_view == v,
                self.is_nav_enabled(v),
                label_alpha,
            ));
        }
        col = col.push(sec_hdr(self.t("nav_section_tools"), label_alpha));
        for &v in NAV_TOOLS {
            col = col.push(nav_btn(
                v,
                self.t(v.sidebar_label_key()),
                self.current_view == v,
                self.is_nav_enabled(v),
                label_alpha,
            ));
        }

        // Nav column fills; update pill anchored below.
        let body: Element<'_, Message> = if let Some(release) = self.update_available.as_ref() {
            column![
                container(col).width(Length::Fill).height(Length::Fill),
                self.update_available_pill(release),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            container(col)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };

        let width =
            SIDEBAR_RAIL_WIDTH + (SIDEBAR_EXPANDED_WIDTH - SIDEBAR_RAIL_WIDTH) * self.sidebar_anim;
        let panel = container(body)
            .width(width)
            .height(Length::Fill)
            .style(panel_bg);
        let shell =
            row![panel, widget::rule::vertical(1).style(shell_rule_style)].height(Length::Fill);
        // Idle interaction prevents click-through to wizard cards
        // under the Stack (Stack levitates the cursor for lower
        // layers when top reports a non-None interaction).
        iced::widget::mouse_area(shell)
            .on_enter(Message::SidebarHoverEnter)
            .on_exit(Message::SidebarHoverExit)
            .on_press(Message::Noop)
            .interaction(iced::mouse::Interaction::Idle)
            .into()
    }

    pub(crate) fn content(&self) -> Element<'_, Message> {
        if self.current_view == View::Root {
            return self.view_root_wizard();
        }
        if self.current_view == View::Flash {
            return self.view_flash_wizard();
        }
        if self.current_view == View::SystemUpdate {
            return self.view_sysupdate_wizard();
        }
        if self.current_view == View::Unroot {
            return self.view_unroot_wizard();
        }
        // Advanced wizards (generic + FlashPartitions) skip the grid's
        // scrollable+padding wrapper so the step bar isn't pinched and
        // the 280 px browse card doesn't stretch.
        if self.current_view == View::Advanced
            && (self.advanced_wizard_open.is_open() || self.adv_wizard.action.is_some())
        {
            return self.view_advanced();
        }

        // Reboot cards need Fill height; scrollable would force Shrink
        // and collapse them.
        if self.current_view == View::Reboot {
            return container(
                container(self.view_reboot())
                    .padding(24)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        }
        let inner = match self.current_view {
            View::Dashboard => self.view_dashboard(),
            View::Advanced => self.view_advanced(),
            View::Settings => self.view_settings(),
            _ => self.view_placeholder(),
        };
        // Dashboard wants the log card to fill the leftover vertical space
        // so the inner top + bottom margins stay symmetric. A `scrollable`
        // gives its child unbounded height, which collapses every
        // `Length::Fill` inside the dashboard tree to zero — so the
        // dashboard skips the scrollable wrapper and lets its own
        // `column.height(Fill)` claim the bounded viewport directly.
        // Other views (Advanced, Settings, …) keep the scrollable wrapper
        // because their content can legitimately grow past the viewport.
        let body: Element<'_, Message> = if matches!(self.current_view, View::Dashboard) {
            container(inner)
                .padding(24)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            scrollable(container(inner).padding(24).width(Length::Fill))
                .style(m3_scrollable_style)
                .into()
        };
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub(crate) fn error_banner(&self, msg: &str) -> Element<'_, Message> {
        // Floating overlay via `view()`'s stack so the layout below
        // doesn't shift.
        let card = container(
            row![
                text(format!("  {msg}"))
                    .size(12)
                    .style(error_container_text_style),
                Space::new().width(Length::Fill),
                button(text(" × ").size(14).style(error_container_text_style))
                    .on_press(Message::DismissError)
                    .padding([2, 10])
                    .style(|t: &Theme, status| {
                        let p = pal_of(t);
                        let a = theme::state_alpha(status);
                        button::Style {
                            background: if a > 0.0 {
                                Some(with_alpha(p.on_error_container, a).into())
                            } else {
                                None
                            },
                            text_color: p.on_error_container,
                            border: iced::Border {
                                radius: theme::shape::XS.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }),
            ]
            .padding([8, 16])
            .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .style(move |t: &Theme| {
            let p = pal_of(t);
            container::Style {
                background: Some(p.error_container.into()),
                border: iced::Border {
                    color: p.error_container,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                shadow: theme::elevation(2, theme::is_dark(t)),
                ..Default::default()
            }
        });
        // Pin to y=0 via a Fill-height spacer below.
        column![card, Space::new().width(Length::Fill).height(Length::Fill)]
            .width(Length::Fill)
            .into()
    }

    pub(crate) fn status_bar(&self) -> Element<'_, Message> {
        let p = self.pal();
        let status_color = self.connection.color(&p);
        let status_label = self.t(self.connection.label_key());
        let model_text = if self.device_model.is_empty() {
            ""
        } else {
            &self.device_model
        };
        let mut status_row = row![
            text(format!("●  {status_label}"))
                .size(12)
                .color(status_color),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);
        if !model_text.is_empty() {
            status_row =
                status_row.push(text(format!("— {model_text}")).size(12).style(muted_style));
        }
        status_row = status_row.push(Space::new().width(Length::Fill));
        if self.busy {
            status_row = status_row.push(
                text(self.t("status_working").to_string())
                    .size(12)
                    .style(accent_style),
            );
        }
        status_row = status_row.push(
            // Debug builds show "debug" instead of the version so a dev
            // build is never mistaken for a released one in screenshots/bug
            // reports.
            text(if cfg!(debug_assertions) {
                "debug"
            } else {
                concat!("v", env!("CARGO_PKG_VERSION"))
            })
            .size(12)
            .style(muted_style),
        );
        // Top divider via an explicit `horizontal_rule` (1 px) so
        // the meeting point with the sidebar's right divider lands
        // as a single line per direction (M3 bottom-app-bar
        // guidance: one divider per shared edge).
        column![
            widget::rule::horizontal(1).style(shell_rule_style),
            container(status_row.padding([8, 20]))
                .width(Length::Fill)
                .style(|t: &Theme| panel_bg(t)),
        ]
        .into()
    }

    /// Warning-container banner shell shared by the
    /// missing-driver install prompt and the optional update prompt. The
    /// Qualcomm USB driver is not strictly mandatory for every LTBox
    /// feature, so both prompts use a warning tone rather than a hard error.
    fn driver_banner_container<'a>(
        &self,
        content: impl Into<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        container(content)
            .padding([12, 16])
            .width(Length::Fill)
            .style(move |t: &Theme| {
                let p = pal_of(t);
                container::Style {
                    background: Some(p.warning_container.into()),
                    border: iced::Border {
                        color: p.warning_container,
                        width: 1.0,
                        radius: theme::shape::SM.into(),
                    },
                    ..Default::default()
                }
            })
            .into()
    }

    /// Wrap a (disabled) driver button in a hover tooltip explaining the
    /// download needs an internet connection — shown while offline so the
    /// greyed-out button isn't a dead end with no explanation.
    fn needs_internet_tooltip<'a>(
        &self,
        btn: impl Into<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        widget::tooltip(
            btn,
            container(text(self.t("driver_needs_internet_tip").to_string()).size(11))
                .padding([6, 10])
                .max_width(240)
                .style(|t: &Theme| theme::tooltip_style(t, theme::shape::SM)),
            widget::tooltip::Position::Top,
        )
        .into()
    }

    pub(crate) fn driver_warning_banner(&self) -> Element<'_, Message> {
        use ltbox_device::driver::DriverStatus;
        let installing = self.installing_drivers;
        // Per-state copy. Windows drivers and Linux kernel-driver packages
        // download from GitHub (network required); Linux udev-rules install is
        // a local pkexec call. Unsupported states show copy only, no action.
        let (title_key, desc_key, install_key, needs_network, can_install) =
            match self.driver_status {
                Some(DriverStatus::UdevRulesMissing) => (
                    "driver_udev_missing_title",
                    "driver_udev_missing_desc",
                    "driver_udev_install_btn",
                    false,
                    true,
                ),
                Some(DriverStatus::UdevRulesStale) => (
                    "driver_udev_stale_title",
                    "driver_udev_stale_desc",
                    "driver_udev_install_btn",
                    false,
                    true,
                ),
                Some(DriverStatus::UdevRulesNoPermission) => (
                    "driver_udev_noperm_title",
                    "driver_udev_noperm_desc",
                    "driver_udev_install_btn",
                    false,
                    true,
                ),
                Some(DriverStatus::KernelDriverMissing) => (
                    "driver_kernel_missing_title",
                    "driver_kernel_missing_desc",
                    "driver_install_btn",
                    true,
                    true,
                ),
                Some(DriverStatus::KernelDriverUnsupported) => (
                    "driver_kernel_unsupported_title",
                    "driver_kernel_unsupported_desc",
                    "driver_install_btn",
                    false,
                    false,
                ),
                _ => (
                    "driver_missing_title",
                    "driver_missing_desc",
                    "driver_install_btn",
                    true,
                    true,
                ),
            };
        let offline = needs_network && self.online == Some(false);
        let btn_label = if installing {
            self.t("driver_installing_btn").to_string()
        } else {
            self.t(install_key).to_string()
        };
        // `Length::Shrink` width on the inner text + `wrapping::None` so
        // a long localized label (e.g. Korean "다운로드 & 설치") never
        // collapses into a per-grapheme vertical column when the parent
        // row decides the button's natural width is wider than the slot
        // it has — let the button overflow its slot instead of shredding
        // the label.
        let btn_label_text = text(btn_label)
            .size(theme::text_size::LABEL_LARGE)
            .wrapping(iced::widget::text::Wrapping::None);
        let action: Element<'_, Message> = if can_install {
            let mut btn = button(btn_label_text)
                .padding([8, 18])
                .style(md_filled_btn_style);
            // Offline → the fetch can only fail, so disable + explain on hover.
            if !installing && !offline {
                btn = btn.on_press(Message::InstallDrivers);
            }
            if offline {
                self.needs_internet_tooltip(btn)
            } else {
                btn.into()
            }
        } else {
            Space::new().width(0).into()
        };

        // `body` fills the remainder via `Length::Fill` so the button
        // sits flush right with its natural width — the previous
        // `Space::new().width(Fill)` between two `Shrink` siblings made
        // the row's total width depend on each text's natural width,
        // which under a long desc string overflowed the banner and left
        // the button only a sliver — collapsing its label into a
        // vertical glyph stack.
        let body = column![
            text(self.t(title_key).to_string())
                .size(theme::text_size::TITLE_MEDIUM)
                .style(warning_container_text_style),
            text(self.t(desc_key).to_string())
                .size(theme::text_size::BODY_SMALL)
                .style(warning_container_text_style),
        ]
        .spacing(4)
        .width(Length::Fill);

        let content = row![body, action]
            .spacing(12)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center);

        self.driver_banner_container(content)
    }

    /// Optional "driver update available" banner — shown when the installed
    /// Qualcomm driver is older than the latest release and the user has
    /// not dismissed it. [Update] reuses the install flow; [Don't show
    /// again] persists the dismissal and drops the banner.
    pub(crate) fn driver_update_banner(&self) -> Element<'_, Message> {
        let installing = self.installing_drivers;
        let offline = self.online == Some(false);
        let (current, latest) = self
            .driver_update
            .as_ref()
            .map(|u| (u.current.clone(), u.latest.clone()))
            .unwrap_or_default();

        let update_label = if installing {
            self.t("driver_installing_btn").to_string()
        } else {
            self.t("driver_update_btn").to_string()
        };
        let mut update_btn = button(
            text(update_label)
                .size(theme::text_size::LABEL_LARGE)
                .wrapping(iced::widget::text::Wrapping::None),
        )
        .padding([8, 18])
        .style(md_filled_btn_style);
        if !installing && !offline {
            update_btn = update_btn.on_press(Message::InstallDrivers);
        }
        let update_action: Element<'_, Message> = if offline {
            self.needs_internet_tooltip(update_btn)
        } else {
            update_btn.into()
        };

        let mut dismiss_btn = button(
            text(self.t("driver_dont_show_again").to_string())
                .size(theme::text_size::LABEL_LARGE)
                .wrapping(iced::widget::text::Wrapping::None),
        )
        .padding([8, 18])
        .style(banner_text_btn_style);
        // Dismiss needs no network — only gate it on an in-flight install.
        if !installing {
            dismiss_btn = dismiss_btn.on_press(Message::DismissDriverUpdate);
        }

        let body = column![
            text(self.t("driver_update_title").to_string())
                .size(theme::text_size::TITLE_MEDIUM)
                .style(warning_container_text_style),
            text(tr_args!(
                "driver_update_desc",
                current = current,
                latest = latest
            ))
            .size(theme::text_size::BODY_SMALL)
            .style(warning_container_text_style),
        ]
        .spacing(4)
        .width(Length::Fill);

        let content = row![body, update_action, dismiss_btn]
            .spacing(8)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center);

        self.driver_banner_container(content)
    }

    /// Dual-USB-C port advisory for TB320FC / TB321FU / TB322FC / TB323FU —
    /// only the long-edge port carries USB data, so warn the user to use it.
    /// Amber, with "Don't show again" (persist per model) + "Close" (this
    /// session). `model` is threaded into both button messages so the
    /// dismissal/close targets the model currently shown.
    pub(crate) fn dual_usb_advisory_banner(&self, model: &str) -> Element<'_, Message> {
        let model = model.to_string();
        let dont_show = button(
            text(self.t("driver_dont_show_again").to_string())
                .size(theme::text_size::LABEL_LARGE)
                .wrapping(iced::widget::text::Wrapping::None),
        )
        .padding([8, 18])
        .style(banner_text_btn_style)
        .on_press(Message::DismissDualUsbAdvisory(model.clone()));
        let close = button(
            text(self.t("btn_close").to_string())
                .size(theme::text_size::LABEL_LARGE)
                .wrapping(iced::widget::text::Wrapping::None),
        )
        .padding([8, 18])
        .style(md_filled_btn_style)
        .on_press(Message::CloseDualUsbAdvisory(model));

        let body = column![
            text(self.t("dual_usb_advisory_title").to_string())
                .size(theme::text_size::TITLE_MEDIUM)
                .style(warning_container_text_style),
            text(self.t("dual_usb_advisory_desc").to_string())
                .size(theme::text_size::BODY_SMALL)
                .style(warning_container_text_style),
        ]
        .spacing(4)
        .width(Length::Fill);

        let content = row![body, dont_show, close]
            .spacing(8)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center);

        self.driver_banner_container(content)
    }

    /// Bottom-of-screen transient toast. Renders a low-attention pill
    /// over a transparent passthrough container so the rest of the
    /// view keeps responding to clicks while the toast is on screen.
    pub(crate) fn toast_view(&self) -> Element<'_, Message> {
        let Some(msg) = self.toast_msg.clone() else {
            return container(text("")).into();
        };
        // Background = `on_surface` (near-black in light, near-white
        // in dark); text needs the inverse to stay readable in both
        // modes — `surface` is exactly that role pair.
        let pill = container(
            text(msg)
                .size(12)
                .style(|t: &Theme| iced::widget::text::Style {
                    color: Some(pal_of(t).surface),
                }),
        )
        .padding([8, 16])
        .style(|t: &Theme| -> container::Style {
            let p = pal_of(t);
            container::Style {
                background: Some(p.on_surface.into()),
                border: iced::Border {
                    radius: 18.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        });
        container(pill)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(iced::Padding {
                top: 0.0,
                right: 0.0,
                // Sits ~100 px above the viewport floor — clears the
                // device-info popup's close-button row without
                // crowding the table content.
                bottom: 100.0,
                left: 0.0,
            })
            .center_x(Length::Fill)
            .align_y(iced::Alignment::End)
            .into()
    }

    pub(crate) fn busy_progress_dialog(&self) -> Element<'_, Message> {
        let op_name = self.busy_operation_label();
        let body = self
            .busy_body_override()
            .unwrap_or_else(|| tr_args!("progress_dialog_body", operation = op_name));

        let spinner: Element<'_, Message> = Spinner::new()
            .width(Length::Fixed(42.0))
            .height(Length::Fixed(42.0))
            .circle_radius(3.0)
            .into();
        let spinner_box = container(spinner)
            .width(56)
            .height(56)
            .center_x(56)
            .center_y(56)
            .style(|t: &Theme| {
                let p = pal_of(t);
                container::Style {
                    text_color: Some(p.primary),
                    ..Default::default()
                }
            });

        let title_col = column![
            text(self.t("progress_dialog_title").to_string())
                .size(theme::text_size::TITLE_MEDIUM)
                .style(on_surface_style),
            text(body).size(13).style(muted_style),
        ]
        .spacing(6)
        .width(Length::Fill);

        let content = column![
            row![spinner_box, title_col]
                .spacing(18)
                .align_y(iced::Alignment::Center),
        ]
        .spacing(16)
        .padding(24)
        .width(420);

        // Modeless: a flash can run for minutes, so the busy dialog must NOT
        // trap the user — the sidebar stays clickable to navigate back to the
        // running op's progress screen. (Confirm dialogs use the modal
        // `m3_dialog`.)
        m3_dialog_modeless(content.into())
    }

    /// Shared loading-state body for any `_popup_view` that fetches
    /// upstream data. 48 px tall slim box with a centred spinner —
    /// every popup uses the same shape, so consolidate here instead
    /// of duplicating the container chain in each call site.
    pub(crate) fn popup_loading_view(&self) -> Element<'_, Message> {
        container(Spinner::new())
            .width(Length::Fill)
            .height(48)
            .center_x(Length::Fill)
            .center_y(48)
            .into()
    }

    pub(crate) fn view_placeholder(&self) -> Element<'_, Message> {
        column![
            text(self.t(self.current_view.label_key()).to_string())
                .size(theme::text_size::TITLE_LARGE),
            widget::rule::horizontal(1),
            container(text("").size(14).style(muted_style))
                .padding(48)
                .width(Length::Fill)
                .center_x(Length::Fill),
        ]
        .spacing(14)
        .width(Length::Fill)
        .into()
    }
}
