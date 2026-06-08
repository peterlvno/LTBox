//! Reboot workers: issue the reboot command over ADB / Fastboot / EDL.
//! Each runs on a blocking thread; extracted from the update_reboot handler.

use crate::{ConnectionStatus, RebootTarget, ensure_edl};
use std::path::PathBuf;

pub(crate) fn reboot_worker(
    conn: ConnectionStatus,
    target: RebootTarget,
    reboot_cmd_sent: String,
) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    match (conn, target) {
        (ConnectionStatus::Adb | ConnectionStatus::AdbRecovery, t) => {
            let mut adb = ltbox_device::adb::AdbManager::new();
            // `AdbManager::reboot` needs the serial
            // from a prior `check_device` call.
            if !adb.check_device().unwrap_or(false) {
                return Err("No ADB device detected — try replugging the cable".into());
            }
            let arg = match t {
                RebootTarget::System => "",
                RebootTarget::Recovery => "recovery",
                RebootTarget::Bootloader => "bootloader",
                RebootTarget::Edl => "edl",
            };
            if let Err(e) = adb.reboot(arg) {
                return Err(format!("ADB reboot failed: {e}"));
            }
        }
        (ConnectionStatus::Fastboot, t) => {
            let mut dev = ltbox_device::fastboot::FastbootDevice::open()
                .map_err(|e| format!("Fastboot open: {e}"))?;
            match t {
                RebootTarget::System => {
                    dev.reboot().map_err(|e| format!("reboot: {e}"))?;
                }
                RebootTarget::Bootloader => {
                    dev.reboot_bootloader()
                        .map_err(|e| format!("reboot-bootloader: {e}"))?;
                }
                RebootTarget::Edl => {
                    drop(dev);
                    ensure_edl(ConnectionStatus::Fastboot, "Reboot", &mut log)
                        .map_err(|()| ltbox_core::i18n::tr("err_edl_transition_failed"))?;
                }
                RebootTarget::Recovery => {
                    return Err(
                        "Fastboot cannot reboot to recovery directly — switch to ADB first".into(),
                    );
                }
            }
        }
        (ConnectionStatus::Edl, _) => {
            // RebootTo routes EDL through
            // RebootEdlWithLoader, never here.
            unreachable!("EDL reboot goes through RebootEdlWithLoader");
        }
        (ConnectionStatus::None, _) => {
            return Err("No device connected".into());
        }
        (ConnectionStatus::AdbUnauthorized, _) => {
            return Err("USB debugging is not authorized on the device".into());
        }
        (ConnectionStatus::AdbServerBlocking, _) => {
            return Err(
                                                        "An external adb server is holding the USB interface. Kill it from the dashboard and retry.".into(),
                                                    );
        }
    }
    ltbox_core::live!(log, "[Reboot] {}", reboot_cmd_sent);
    Ok(log)
}

pub(crate) fn reboot_edl_with_loader_worker(
    loader: PathBuf,
    target: RebootTarget,
    reboot_cmd_sent: String,
) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    // `auto_reset=false` — reset is triggered explicitly below.
    let mut session = ltbox_device::edl::EdlSession::open(&loader, false, &mut log)
        .map_err(|e| format!("EDL session open: {e}"))?;
    match target {
        RebootTarget::System => {
            // Reboot-to-system is the user's intent here; the inner EDL
            // reset log lines duplicate the surrounding `[Reboot]` start +
            // `command sent` lines, so swallow them into a scratch log.
            let mut quiet = Vec::new();
            session.reset_tolerant(&mut quiet);
        }
        RebootTarget::Edl => {
            session
                .reset_to_edl(&mut log)
                .map_err(|e| format!("reset_to_edl: {e}"))?;
        }
        other => {
            return Err(format!("Reboot to {other:?} is not supported from EDL"));
        }
    }
    ltbox_core::live!(log, "[Reboot] {}", reboot_cmd_sent);
    Ok(log)
}
