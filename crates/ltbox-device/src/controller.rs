//! Unified ADB / Fastboot / EDL state-transition controller.

use crate::adb::AdbManager;
use crate::edl;
use crate::fastboot::FastbootDevice;
use thiserror::Error;
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceMode {
    Unknown,
    Adb,
    Fastboot,
    Edl,
}

#[derive(Error, Debug)]
pub enum ControllerError {
    #[error("ADB error: {0}")]
    Adb(#[from] crate::adb::AdbError),
    #[error("Fastboot error: {0}")]
    Fastboot(#[from] crate::fastboot::FastbootError),
    #[error("EDL error: {0}")]
    Edl(#[from] crate::edl::EdlError),
    #[error("No device found in any mode")]
    NoDevice,
    #[error("Operation requires {0} mode")]
    WrongMode(String),
}

type Result<T> = std::result::Result<T, ControllerError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdlTransitionRoute {
    AlreadyEdl,
    AdbReboot,
    FastbootContinueThenAdb,
    ManualWait,
}

fn plan_edl_transition(in_edl: bool, in_fastboot: bool, skip_adb: bool) -> EdlTransitionRoute {
    if in_edl {
        EdlTransitionRoute::AlreadyEdl
    } else if in_fastboot && !skip_adb {
        EdlTransitionRoute::FastbootContinueThenAdb
    } else if skip_adb {
        EdlTransitionRoute::ManualWait
    } else {
        EdlTransitionRoute::AdbReboot
    }
}

pub struct DeviceController {
    pub adb: AdbManager,
    pub skip_adb: bool,
    mode: DeviceMode,
}

impl DeviceController {
    pub fn new() -> Self {
        Self {
            adb: AdbManager::new(),
            skip_adb: false,
            mode: DeviceMode::Unknown,
        }
    }

    /// Detect mode by probing each protocol.
    pub fn detect_mode(&mut self) -> DeviceMode {
        if edl::check_device() {
            self.mode = DeviceMode::Edl;
        } else if FastbootDevice::check_device() {
            self.mode = DeviceMode::Fastboot;
        } else if !self.skip_adb {
            if let Ok(true) = self.adb.check_device() {
                self.mode = DeviceMode::Adb;
            } else {
                self.mode = DeviceMode::Unknown;
            }
        } else {
            self.mode = DeviceMode::Unknown;
        }
        self.mode
    }

    pub fn current_mode(&self) -> DeviceMode {
        self.mode
    }

    pub fn ensure_fastboot(&mut self) -> Result<()> {
        if FastbootDevice::check_device() {
            self.mode = DeviceMode::Fastboot;
            return Ok(());
        }
        // skip_adb means we can't issue an ADB reboot — so waiting on a
        // Fastboot device that nothing is going to produce would hang the
        // GUI for the whole fastboot wait timeout. Surface immediately so
        // the caller can prompt the user for a manual transition.
        if self.skip_adb {
            return Err(ControllerError::NoDevice);
        }
        info!("Rebooting to bootloader via ADB...");
        self.adb.wait_for_device()?;
        self.adb.reboot("bootloader")?;
        info!("Waiting for Fastboot...");
        let _ = FastbootDevice::wait_for_device()?;
        self.mode = DeviceMode::Fastboot;
        Ok(())
    }

    pub fn ensure_edl(&mut self) -> Result<()> {
        match plan_edl_transition(
            edl::check_device(),
            FastbootDevice::check_device(),
            self.skip_adb,
        ) {
            EdlTransitionRoute::AlreadyEdl => {
                self.mode = DeviceMode::Edl;
                return Ok(());
            }
            EdlTransitionRoute::FastbootContinueThenAdb => {
                info!("Device in Fastboot, resuming boot for ADB EDL transition...");
                let mut dev = FastbootDevice::open()?;
                let _ = dev.continue_boot();
                info!("Waiting for ADB...");
                self.adb.wait_for_device()?;
                info!("Rebooting to EDL via ADB...");
                self.adb.reboot("edl")?;
            }
            EdlTransitionRoute::AdbReboot => {
                info!("Rebooting to EDL via ADB...");
                self.adb.wait_for_device()?;
                self.adb.reboot("edl")?;
            }
            EdlTransitionRoute::ManualWait => {
                let _ = edl::wait_for_device()?;
                self.mode = DeviceMode::Edl;
                return Ok(());
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = edl::wait_for_device()?;
        self.mode = DeviceMode::Edl;
        Ok(())
    }

    /// Active slot suffix; ensures Fastboot first.
    pub fn detect_active_slot(&mut self) -> Result<Option<String>> {
        self.ensure_fastboot()?;
        let mut dev = FastbootDevice::open()?;
        Ok(dev.get_slot_suffix()?)
    }
}

/// Poll ADB then Fastboot for the active slot suffix until one
/// returns `_a` or `_b`, or the deadline expires.
///
/// Slot is required for every flash / dump / root path: writing to
/// the wrong slot's `boot_*` / `vbmeta_*` / `init_boot_*` partition
/// either fails AVB on the next boot (if the device flips slots
/// post-flash) or quietly leaves the device on the unmodified slot
/// (if it doesn't). Defaulting to `_a` when probing fails was a
/// silent footgun — flashes landed on `_a` while the device was
/// running on `_b`, so the user saw "flash succeeded" but nothing
/// changed. Force a hard error instead so the caller has to fix the
/// transport state before any destructive op runs.
///
/// Polls both transports because the device's state mid-flow
/// determines which one answers: ADB works in normal / recovery,
/// Fastboot works in bootloader. EDL has no slot getvar — caller
/// must probe BEFORE entering EDL.
///
/// `log` receives one human-readable line per poll attempt
/// (suppressed via the standard `live!` macro contract — drop the
/// `Vec` in headless callers).
pub fn poll_active_slot(
    timeout: std::time::Duration,
    log: &mut Vec<String>,
) -> std::result::Result<String, String> {
    let deadline = std::time::Instant::now() + timeout;
    let mut adb_attempted = false;
    let mut fastboot_attempted = false;
    let mut last_adb_err = String::new();
    let mut last_fastboot_err = String::new();

    while std::time::Instant::now() < deadline {
        // ADB attempt — only if device is currently in a state that
        // accepts shell (Device or Recovery).
        let mut adb = AdbManager::new();
        match adb.check_device_state() {
            Ok(Some(state @ ("device" | "recovery"))) => {
                adb_attempted = true;
                match adb.get_slot_suffix() {
                    Ok(Some(s)) if s == "_a" || s == "_b" => {
                        ltbox_core::live!(log, "[Slot] resolved via ADB ({state}): {s}");
                        return Ok(s);
                    }
                    Ok(Some(other)) => {
                        last_adb_err = format!("ADB returned unexpected slot value `{other}`");
                    }
                    Ok(None) => {
                        last_adb_err =
                            "ADB returned empty `ro.boot.slot_suffix` (device may not be A/B)"
                                .to_string();
                    }
                    Err(e) => {
                        last_adb_err = format!("ADB shell failed: {e}");
                    }
                }
            }
            Ok(Some(state)) => {
                last_adb_err = format!("ADB state `{state}` does not accept shell");
            }
            Ok(None) => {
                last_adb_err = "no ADB device visible".to_string();
            }
            Err(e) => {
                last_adb_err = format!("ADB probe failed: {e}");
            }
        }

        // Fastboot attempt — open() fails fast if the device isn't
        // in bootloader, so no separate state probe.
        match FastbootDevice::open() {
            Ok(mut fb) => {
                fastboot_attempted = true;
                match fb.get_slot_suffix() {
                    Ok(Some(s)) if s == "_a" || s == "_b" => {
                        ltbox_core::live!(log, "[Slot] resolved via Fastboot: {s}");
                        return Ok(s);
                    }
                    Ok(Some(other)) => {
                        last_fastboot_err =
                            format!("Fastboot returned unexpected `current-slot` value `{other}`");
                    }
                    Ok(None) => {
                        last_fastboot_err =
                            "Fastboot `current-slot` getvar returned empty (device may not be A/B)"
                                .to_string();
                    }
                    Err(e) => {
                        last_fastboot_err = format!("Fastboot getvar failed: {e}");
                    }
                }
            }
            Err(e) => {
                last_fastboot_err = format!("Fastboot open failed: {e}");
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    // Build a diagnostic that surfaces what was tried + the last
    // failure mode per transport so the user knows whether to plug
    // ADB cable, reboot to bootloader, or fix permissions.
    let mut detail = String::new();
    if adb_attempted {
        detail.push_str(&format!("ADB: {last_adb_err}. "));
    } else {
        detail.push_str("ADB: never reached a shell-capable state. ");
    }
    if fastboot_attempted {
        detail.push_str(&format!("Fastboot: {last_fastboot_err}."));
    } else {
        detail.push_str("Fastboot: device never enumerated as a Fastboot endpoint.");
    }
    Err(format!(
        "Could not detect active slot via ADB or Fastboot within {timeout:?}. {detail} \
         Connect the device in normal / recovery mode (ADB) or bootloader mode (Fastboot) and retry. \
         Defaulting to slot `_a` was previously silent and led to flashes landing on the wrong slot."
    ))
}

impl Default for DeviceController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edl_route_from_fastboot_prefers_adb_when_available() {
        assert_eq!(
            plan_edl_transition(false, true, false),
            EdlTransitionRoute::FastbootContinueThenAdb
        );
    }

    #[test]
    fn edl_route_from_fastboot_waits_manual_when_adb_skipped() {
        assert_eq!(
            plan_edl_transition(false, true, true),
            EdlTransitionRoute::ManualWait
        );
    }
}
