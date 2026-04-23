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
        if edl::check_device() {
            self.mode = DeviceMode::Edl;
            return Ok(());
        }

        // Try every transition that doesn't need ADB first, so the skip_adb
        // user with the device in Fastboot still gets a chance via OEM EDL.
        if FastbootDevice::check_device() {
            info!("Device in Fastboot, attempting EDL transition...");
            if let Ok(mut dev) = FastbootDevice::open() {
                if dev.oem_edl().is_ok() {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    if edl::check_device() {
                        self.mode = DeviceMode::Edl;
                        return Ok(());
                    }
                }
                // OEM EDL didn't land the device in EDL. Without ADB there
                // is no second-chance transition available — bail instead
                // of blocking on `edl::wait_for_device()` that cannot
                // complete.
                if self.skip_adb {
                    return Err(ControllerError::NoDevice);
                }
                info!("OEM EDL failed, falling back to ADB...");
                let _ = dev.continue_boot();
                self.adb.wait_for_device()?;
                self.adb.reboot("edl")?;
            } else if self.skip_adb {
                return Err(ControllerError::NoDevice);
            }
        } else if self.skip_adb {
            // No EDL, no Fastboot, skip_adb: nothing we can do to drive
            // the transition. Refuse instead of blocking for the whole
            // edl::wait_for_device timeout.
            return Err(ControllerError::NoDevice);
        } else {
            info!("Rebooting to EDL via ADB...");
            self.adb.wait_for_device()?;
            self.adb.reboot("edl")?;
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

impl Default for DeviceController {
    fn default() -> Self {
        Self::new()
    }
}
