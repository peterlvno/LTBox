//! Firmware-flash wizard handler. Extracted from `main.rs`.

use crate::*;
use iced::Task;

impl App {
    #[allow(unreachable_code)]
    pub(crate) fn update_flash(&mut self, msg: FlashMsg) -> Task<Message> {
        match msg {
            FlashMsg::FlashRegion(r) => {
                // TB322FC is a PRC-only SKU. The region card UI grays
                // out ROW, but a stale message from a pre-poll click
                // could still land here. Drop it so the wizard never
                // accepts a region the hardware doesn't ship with.
                if self.is_tb322fc() && r == DeviceRegion::Row {
                    return Task::none();
                }
                self.flash.device_region = Some(r);
                Task::none()
            }
            FlashMsg::FlashTarget(t) => {
                // TB322FC: cross-region (OtherRegion) flashes are blocked
                // because the only valid region is PRC. Drop the message
                // even if a stale dispatch slips past the disabled card.
                if self.is_tb322fc() && t == FlashTarget::OtherRegion {
                    return Task::none();
                }
                self.flash.target = Some(t);
                Task::none()
            }
            FlashMsg::FlashDataMode(m) => {
                self.flash.data_mode = Some(m);
                Task::none()
            }
            FlashMsg::FlashNext => {
                // Data step → build WorkflowConfig; wipe opens country popup.
                if self.flash.step == 2 {
                    self.wf_config = WorkflowConfig {
                        modify_region: self.flash.target == Some(FlashTarget::OtherRegion),
                        device_region: self.flash.device_region,
                        modify_rollback: if self.flash.target == Some(FlashTarget::OtherRegion) {
                            RollbackSetting::On
                        } else {
                            RollbackSetting::Auto
                        },
                        wipe: self.flash.data_mode == Some(DataMode::Wipe),
                        country_action: CountryAction::Unset,
                    };
                    if self.wf_config.wipe {
                        self.flash.next();
                        self.country_popup_open = true;
                        return Task::none();
                    }
                }
                if self.flash.step == 4 {
                    self.flash.next();
                    return self.update(Message::Flash(FlashMsg::FlashExecStart));
                }
                self.flash.next();
                Task::none()
            }
            FlashMsg::FlashBack => {
                if self.flash.step == 4 {
                    // Re-arm country patching so the popup's "Do not change"
                    // selection doesn't survive a Back→Next round trip.
                    self.wf_config.country_action = CountryAction::Unset;
                }
                self.flash.back();
                Task::none()
            }
            FlashMsg::FlashSelectFolder => {
                self.picker_target = PickerTarget::FlashFolder;
                return pick_folder_task(
                    pickers::PickerKind::QfilFirmwareFolder,
                    &self.recent_paths,
                    Message::FolderSelected,
                );
                Task::none()
            }
            FlashMsg::FlashSelectLoader => {
                // Always open the picker (don't auto-reuse the Settings default
                // via `pick_loader_with_default`) so the Change button can pick
                // a different loader — the default was already applied when the
                // loader-less folder was selected.
                return pickers::pick_file_for(
                    loader_file_spec("picker_target_edl_loader"),
                    &self.recent_paths,
                    |v| Message::Flash(FlashMsg::FlashLoaderChosen(v)),
                );
                Task::none()
            }
            FlashMsg::FlashLoaderChosen(path) => {
                if let Some(p) = path {
                    // Model-aware resolve: upgrades a `.melf` to a sibling Sahara
                    // manifest on TB323FU (and rejects a standalone `.melf`
                    // there), validates the extension, and records the recent.
                    match self.resolve_loader_input(&p) {
                        Ok(loader) => {
                            self.flash.loader_override = Some(loader);
                            self.flash.loader_error = None;
                        }
                        Err(msg) => self.flash.loader_error = Some(msg),
                    }
                }
                Task::none()
            }
            FlashMsg::FlashExecStart => {
                self.begin_op(View::Flash);
                self.op_steps = self.derive_flash_op_steps();
                self.error_msg = None;
                let cfg = self.wf_config.clone();
                let conn = self.connection;
                let device_model = self.device_model.clone();
                let fw_folder = self.flash.firmware_folder.clone().unwrap_or_default();
                let loader_override = self.flash.loader_override.clone();
                let rollback_label = self.t(cfg.modify_rollback.label_key()).to_string();
                // Split the old single "Starting: modify_region=… rollback=…
                // wipe=…" line into three labelled, translated lines — the
                // raw variable dump read like debug output.
                let region_yn = self
                    .t(if cfg.modify_region {
                        "common_yes"
                    } else {
                        "common_no"
                    })
                    .to_string();
                let wipe_yn = self
                    .t(if cfg.wipe { "common_yes" } else { "common_no" })
                    .to_string();
                self.log_push(format!(
                    "[Flash] {}",
                    tr_args!("live_flash_region_convert", value = region_yn)
                ));
                self.log_push(format!(
                    "[Flash] {}",
                    tr_args!("live_flash_rollback_bypass", value = rollback_label)
                ));
                self.log_push(format!(
                    "[Flash] {}",
                    tr_args!("live_flash_data_wipe", value = wipe_yn)
                ));
                let rb_mode = cfg.modify_rollback.to_mode();
                // NOTE: the EDL-start ARB downgrade (On/Auto → Off when the
                // device can't be Fastboot/ADB-probed) is applied inside the
                // worker, AFTER the firmware's vendor_boot fingerprint is
                // known — so a TB323FU target (which reads its rollback index
                // by dumping partitions over EDL) is exempt and stays on Auto.
                let ll = self.live_labels();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            ltbox_core::runtime::run_heavy(move || {
                                flash_worker(
                                    cfg,
                                    conn,
                                    device_model,
                                    fw_folder,
                                    loader_override,
                                    rb_mode,
                                    ll,
                                )
                            })
                            .and_then(|r| r)
                        })
                        .await
                        .unwrap_or(Err("Task failed".to_string()))
                    },
                    |result| match result {
                        Ok(lines) => Message::Flash(FlashMsg::FlashExecDone(lines)),
                        Err(e) => Message::OperationError(e),
                    },
                );
                Task::none()
            }
            FlashMsg::FlashExecDone(lines) => {
                // Extend *before* end_op so the END separator sits
                // below the backend's detail lines, not above them.
                self.flush_exec_done_log(lines);
                self.end_op();
                self.wf_config = WorkflowConfig::default();
                Task::none()
            }
        }
    }
}
