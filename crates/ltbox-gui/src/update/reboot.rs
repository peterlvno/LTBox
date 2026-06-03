//! Reboot-view handler (target pick, confirm, EDL-loader dispatch). Extracted from `main.rs`.

use crate::*;
use iced::Task;

impl App {
    #[allow(unreachable_code)]
    pub(crate) fn update_reboot(&mut self, msg: RebootMsg) -> Task<Message> {
        match msg {
            RebootMsg::RebootRequest(target) => {
                if self.busy {
                    return Task::none();
                }
                if !target.available_from(self.connection) {
                    self.error_msg = Some(format!(
                        "{:?} not reachable from {:?}",
                        target, self.connection
                    ));
                    return Task::none();
                }
                self.reboot_confirm_target = Some(target);
                Task::none()
            }
            RebootMsg::RebootDismiss => {
                self.reboot_confirm_target = None;
                Task::none()
            }
            RebootMsg::RebootConfirm => {
                if let Some(t) = self.reboot_confirm_target.take() {
                    return self.update(Message::Reboot(RebootMsg::RebootTo(t)));
                }
                Task::none()
            }
            RebootMsg::RebootTo(target) => {
                if self.busy {
                    return Task::none();
                }
                let conn = self.connection;
                if !target.available_from(conn) {
                    self.error_msg = Some(format!("{:?} not reachable from {:?}", target, conn));
                    return Task::none();
                }
                // EDL needs a Firehose loader before Power(reset).
                if matches!(conn, ConnectionStatus::Edl) {
                    return self.pick_loader_with_default(move |path| {
                        Message::Reboot(RebootMsg::RebootEdlWithLoader(target, path))
                    });
                }
                self.begin_op(View::Reboot);
                self.error_msg = None;
                self.log_push(format!(
                    "[Reboot] {}",
                    tr_args!(
                        "log_reboot_target_from",
                        target = self.t(target.label_key()),
                        source = format!("{conn:?}")
                    ),
                ));
                let reboot_cmd_sent = self.t("log_reboot_command_sent").to_string();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            reboot_worker(conn, target, reboot_cmd_sent)
                        })
                        .await
                        .unwrap_or_else(|_| Err("Task failed".to_string()))
                    },
                    |r| match r {
                        Ok(lines) => Message::Reboot(RebootMsg::RebootDone(lines)),
                        Err(e) => Message::OperationError(e),
                    },
                );
                Task::none()
            }
            RebootMsg::RebootEdlWithLoader(target, path) => {
                let Some(loader_input) = path else {
                    self.log_push(format!(
                        "[Reboot] {}",
                        self.t("log_reboot_cancelled_no_loader")
                    ));
                    return Task::none();
                };
                // Accept direct loader files. Legacy folder paths from
                // older recents remain supported via resolve_loader_input.
                let loader = match self.resolve_loader_input(&loader_input) {
                    Ok(p) => std::path::PathBuf::from(p),
                    Err(msg) => {
                        self.error_msg = Some(msg);
                        return Task::none();
                    }
                };
                if !loader.exists() {
                    self.error_msg = Some(format!("Loader not found: {}", loader.display()));
                    return Task::none();
                }
                self.begin_op(View::Reboot);
                self.error_msg = None;
                self.log_push(format!(
                    "[Reboot] {}",
                    tr_args!(
                        "log_reboot_target_from_edl",
                        target = self.t(target.label_key()),
                        loader = loader.display().to_string()
                    ),
                ));
                let reboot_cmd_sent = self.t("log_reboot_command_sent").to_string();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            reboot_edl_with_loader_worker(loader, target, reboot_cmd_sent)
                        })
                        .await
                        .unwrap_or_else(|_| Err("Task failed".to_string()))
                    },
                    |r| match r {
                        Ok(lines) => Message::Reboot(RebootMsg::RebootDone(lines)),
                        Err(e) => Message::OperationError(e),
                    },
                );
                Task::none()
            }
            RebootMsg::RebootDone(lines) => {
                self.end_op();
                self.flush_exec_done_log(lines);
                Task::none()
            }
        }
    }
}
