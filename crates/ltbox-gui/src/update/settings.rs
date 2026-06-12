//! Settings-view handler: language, theme, default-loader path.
//! Extracted from `main.rs`.
use crate::*;
use iced::Task;

impl App {
    /// Settings view — language pick, theme dropdown, default-loader
    /// path management. Each variant either updates `self.settings`
    /// and persists, or spawns the file picker `Task` for the loader
    /// path.
    pub(crate) fn update_settings(&mut self, msg: SettingsMsg) -> Task<Message> {
        match msg {
            SettingsMsg::SetLanguage(l) => {
                self.settings.language = l;
                self.translations = Translations::load(l);
                install_core_translator(l);
                self.persist_settings();
                Task::none()
            }
            SettingsMsg::SetThemeSeed(seed) => {
                self.theme_seed = seed;
                self.sync_runtime_theme();
                self.persist_settings();
                Task::none()
            }
            SettingsMsg::SetQcomDriverMode(mode) => {
                if self.busy {
                    return Task::none();
                }
                let mode = effective_qcom_driver_mode(mode);
                if self.qcom_driver_mode == mode {
                    return Task::none();
                }
                self.qcom_driver_mode = mode;
                ltbox_device::driver::set_qcom_driver_mode(mode);
                self.driver_status = None;
                self.driver_update = None;
                self.qcom_driver_update_dismissed = false;
                self.persist_settings();
                Task::batch([
                    Task::perform(
                        async {
                            tokio::task::spawn_blocking(
                                ltbox_device::driver::check_required_drivers,
                            )
                            .await
                            .unwrap_or(ltbox_device::driver::DriverStatus::NotWindows)
                        },
                        Message::DriverCheckDone,
                    ),
                    Task::perform(
                        async {
                            tokio::task::spawn_blocking(ltbox_device::driver::check_driver_update)
                                .await
                                .unwrap_or(None)
                        },
                        Message::DriverUpdateCheckDone,
                    ),
                    Task::perform(
                        async {
                            tokio::task::spawn_blocking(ltbox_device::driver::probe_connectivity)
                                .await
                                .unwrap_or(false)
                        },
                        Message::ConnectivityChecked,
                    ),
                ])
            }
            SettingsMsg::SettingsPickDefaultLoader => {
                let spec = loader_file_spec("picker_target_edl_loader");
                pickers::pick_file_for(spec, &self.recent_paths, |__v| {
                    Message::Settings(SettingsMsg::SettingsDefaultLoaderChosen(__v))
                })
            }
            SettingsMsg::SettingsDefaultLoaderChosen(path) => {
                if let Some(p) = path {
                    self.remember_recent(pickers::PickerKind::File, &p);
                    self.default_loader_path = Some(p);
                    self.persist_settings();
                }
                Task::none()
            }
            SettingsMsg::SettingsClearDefaultLoader => {
                self.default_loader_path = None;
                self.persist_settings();
                Task::none()
            }
        }
    }
}
