//! EDL-entry helpers: route the device into EDL/9008 from ADB,
//! Fastboot, or a manual trigger. Extracted from `main.rs`.

use crate::{ConnectionStatus, EdlEntryAction, edl_entry_action};
use ltbox_core::tr_args;

/// Transition the device to EDL from whatever state it is in. Returns
/// `Ok(())` if the device is already in EDL or was sent there.
/// Shared by `dump_parts_scan`. Mirrors the inline block in
/// `flash_parts_execute`.
pub(crate) fn wait_for_edl_ready(tag: &str, log: &mut Vec<String>) -> Result<(), ()> {
    ltbox_core::live!(
        log,
        "[{tag}] {}",
        ltbox_core::i18n::tr("live_wait_edl_port")
    );
    match ltbox_device::edl::wait_for_device() {
        Ok(_) => {
            ltbox_core::live!(log, "[{tag}] {}", ltbox_core::i18n::tr("live_edl_ready"));
            Ok(())
        }
        Err(e) => {
            // Localize the common wait failures so the detail isn't raw
            // English; rarer variants keep their Display text.
            let detail = match &e {
                ltbox_device::edl::EdlError::PortTimeout(d) => {
                    tr_args!("live_edl_wait_timeout", seconds = d.as_secs().to_string())
                }
                ltbox_device::edl::EdlError::PortNotFound => {
                    ltbox_core::i18n::tr("live_edl_port_not_found")
                }
                other => other.to_string(),
            };
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                tr_args!("live_edl_not_found", error = detail)
            );
            // Only the reboot / manual-wait paths reach here — an
            // already-in-EDL start never calls this — so the device was sent
            // toward EDL but its port never appeared. No write has happened
            // yet; reassure the user before the caller aborts.
            ltbox_core::live!(
                log,
                "{}",
                ltbox_core::i18n::tr("live_edl_reboot_no_port_notice")
            );
            Err(())
        }
    }
}

pub(crate) fn wait_for_manual_edl(tag: &str, log: &mut Vec<String>) -> Result<(), ()> {
    ltbox_core::live!(
        log,
        "[{tag}] {}",
        ltbox_core::i18n::tr("live_manual_reboot_edl_wait")
    );
    wait_for_edl_ready(tag, log)
}

pub(crate) fn reboot_adb_to_edl(
    tag: &str,
    log: &mut Vec<String>,
    mgr: &mut ltbox_device::adb::AdbManager,
) -> Result<(), ()> {
    // Command echo (`adb reboot edl`) suppressed — the user only sees the
    // outcome (waiting / reached EDL / failure).
    //
    // `mgr` is caller-provided so the upstream Fastboot→ADB→EDL path can
    // reuse the `AdbManager` it already opened in `wait_for_device`.
    // Constructing a fresh `AdbManager` here would create a second
    // libusb claimer for the same Android endpoint while the caller's
    // cached `ADBUSBDevice` is still alive — `LIBUSB_ERROR_BUSY` on the
    // claim, retried + bucketed into `check_device_state` →
    // `Some("unauthorized")`, and the worker would bail to
    // `wait_for_manual_edl` even with the device fully authorized.
    // `check_device` now accepts only `Device` state, so use
    // `check_device_state` here — recovery-state ADB can also seed the
    // serial before issuing `reboot edl`.
    let state = match mgr.check_device_state() {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                tr_args!("live_adb_state_probe_failed", error = e.to_string())
            );
            return wait_for_manual_edl(tag, log);
        }
    };
    match state {
        Some("device") | Some("recovery") => {}
        Some(other) => {
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                tr_args!("live_adb_state_cannot_reboot_edl", state = other)
            );
            return wait_for_manual_edl(tag, log);
        }
        None => {
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                ltbox_core::i18n::tr("live_no_adb_device_found")
            );
            return wait_for_manual_edl(tag, log);
        }
    }
    match mgr.reboot("edl") {
        Ok(_) => wait_for_edl_ready(tag, log),
        Err(e) => {
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                tr_args!("live_adb_reboot_edl_failed", error = e.to_string())
            );
            wait_for_manual_edl(tag, log)
        }
    }
}

pub(crate) fn fastboot_reboot_then_adb_edl(tag: &str, log: &mut Vec<String>) -> Result<(), ()> {
    // Device boots into the OS so ADB comes up for the subsequent
    // `adb reboot edl`. `oem edl` are intentionally not used (oem edl
    // misbehaves on some devices).
    match ltbox_device::fastboot::FastbootDevice::open() {
        Ok(mut dev) => {
            if let Err(e) = dev.reboot() {
                ltbox_core::live!(
                    log,
                    "[{tag}] {}",
                    tr_args!("live_fastboot_reboot_failed", error = e.to_string())
                );
                return wait_for_manual_edl(tag, log);
            }
        }
        Err(e) => {
            ltbox_core::live!(
                log,
                "[{tag}] {}",
                tr_args!("live_fastboot_open_failed", error = e.to_string())
            );
            return wait_for_manual_edl(tag, log);
        }
    }

    ltbox_core::live!(
        log,
        "[{tag}] {}",
        ltbox_core::i18n::tr("live_adb_wait_after_fastboot")
    );
    let mut mgr = ltbox_device::adb::AdbManager::new();
    if let Err(e) = mgr.wait_for_device() {
        ltbox_core::live!(
            log,
            "[{tag}] {}",
            tr_args!("live_adb_wait_after_fastboot_failed", error = e.to_string())
        );
        return wait_for_manual_edl(tag, log);
    }
    // Hand the same `mgr` (which already holds the cached
    // `ADBUSBDevice` from `wait_for_device`) to `reboot_adb_to_edl`
    // instead of letting it open a second one — see the doc comment
    // on `reboot_adb_to_edl` for the USB-claim race rationale.
    reboot_adb_to_edl(tag, log, &mut mgr)
}

pub(crate) fn ensure_edl(
    conn: ConnectionStatus,
    tag: &str,
    log: &mut Vec<String>,
) -> Result<(), ()> {
    match edl_entry_action(conn) {
        EdlEntryAction::AlreadyEdl => {
            ltbox_core::live!(log, "[{tag}] {}", ltbox_core::i18n::tr("live_edl_already"));
            Ok(())
        }
        EdlEntryAction::AdbReboot => {
            let mut mgr = ltbox_device::adb::AdbManager::new();
            reboot_adb_to_edl(tag, log, &mut mgr)
        }
        EdlEntryAction::FastbootRebootThenAdb => fastboot_reboot_then_adb_edl(tag, log),
        EdlEntryAction::ManualWait => wait_for_manual_edl(tag, log),
    }
}
