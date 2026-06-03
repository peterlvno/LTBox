//! Unroot-wizard handler. Extracted from `main.rs`.

use crate::*;
use iced::Task;

impl App {
    #[allow(unreachable_code)]
    pub(crate) fn update_unroot(&mut self, msg: UnrootMsg) -> Task<Message> {
        match msg {
            UnrootMsg::SetUnrootType(t) => {
                self.unroot.unroot_type = Some(t);
                Task::none()
            }
            UnrootMsg::UnrootSelectFolder => {
                self.picker_target = PickerTarget::UnrootFolder;
                return pick_folder_task(
                    pickers::PickerKind::QfilFirmwareFolder,
                    &self.recent_paths,
                    Message::FolderSelected,
                );
                Task::none()
            }
            UnrootMsg::UnrootSelectLoader => {
                return self.pick_loader_with_default(|__v| {
                    Message::Unroot(UnrootMsg::UnrootLoaderChosen(__v))
                });
                Task::none()
            }
            UnrootMsg::UnrootLoaderChosen(path) => {
                if let Some(p) = path {
                    self.remember_recent(pickers::PickerKind::File, &p);
                    self.unroot.loader_path = Some(p);
                }
                Task::none()
            }
            UnrootMsg::UnrootNext => {
                if self.unroot.step == 3 {
                    self.unroot.next();
                    return self.update(Message::Unroot(UnrootMsg::UnrootExecStart));
                }
                self.unroot.next();
                // If we just advanced onto the loader step and a
                // Settings-level default loader is configured + still
                // on disk, pre-fill it + skip straight to the folder
                // step — matches the Root wizard's loader-skip pattern
                // (see `RootNext` step-5 fill + advance).
                if self.unroot.step == 1
                    && self.unroot.loader_path.is_none()
                    && let Some(path) = self.resolved_default_loader()
                {
                    self.unroot.loader_path = Some(path);
                    self.unroot.next();
                }
                Task::none()
            }
            UnrootMsg::UnrootBack => {
                self.unroot.back();
                Task::none()
            }
            UnrootMsg::UnrootExecStart => {
                let Some(unroot_type) = self.unroot.unroot_type else {
                    return Task::none();
                };
                let Some(folder) = self.unroot.folder_path.clone() else {
                    return Task::none();
                };
                let conn = self.connection;
                // Loader is decoupled from the backup folder — `folder`
                // holds boot.img + vbmeta.img, the loader can live
                // anywhere (Settings default, or whatever the user
                // pointed the loader picker at). `validate_loader_path`
                // surfaces a missing-file error before the device-side
                // work starts, matching the other wizards' behaviour.
                let loader_override =
                    match self.validate_loader_path(&self.unroot.loader_path.clone()) {
                        Ok(p) => Some(p),
                        Err(()) => return Task::none(),
                    };
                self.begin_op(View::Unroot);
                self.op_steps = self.derive_unroot_op_steps();
                self.error_msg = None;
                self.log_push(format!(
                    "[Unroot] {}",
                    tr_args!("log_op_starting", what = self.t(unroot_type.label_key()))
                ));
                let ll = self.live_labels();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            ltbox_core::runtime::run_heavy(move || {
                                unroot_worker(folder, unroot_type, loader_override, conn, ll)
                            })
                            .and_then(|r| r)
                        })
                        .await
                        .unwrap_or(Err("Task failed".to_string()))
                    },
                    |result| match result {
                        Ok(lines) => Message::Unroot(UnrootMsg::UnrootExecDone(lines)),
                        Err(e) => Message::OperationError(e),
                    },
                );
                Task::none()
            }
            UnrootMsg::UnrootExecDone(lines) => {
                self.flush_exec_done_log(lines);
                self.end_op();
                Task::none()
            }
        }
    }
}
