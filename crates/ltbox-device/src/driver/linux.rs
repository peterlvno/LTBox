//! Linux / macOS device-driver / udev-rule status + installer (stub).
//!
//! Returns `DriverStatus::NotWindows` from every probe so GUI
//! behaviour matches the pre-rename `windows_driver::*` no-op path.
//! Real Linux probing — udev rule existence, `/sys/bus/usb/devices`
//! walk for `05c6:9008`, serial-node permission test — is owed by
//! the L2/L3 hardware testing pass once a Lenovo target is in
//! reach. See `PLAN_Linux_Support.md` for the planned variant
//! expansion (`UdevRuleMissing`, `DevicePresentNoSerialNode`, …).
//!
//! `download_and_install` returns `DriverError::NotWindows` because
//! the Linux flow will not download a driver blob — the user installs
//! a shipped `misc/udev/51-ltbox-qcom.rules` file via
//! `pkexec ltbox --install-udev` (planned). Returning Err here keeps
//! the GUI's install button safe until that pkexec wiring lands.

use super::{DriverError, DriverStatus, Result};

pub fn check_required_drivers() -> DriverStatus {
    DriverStatus::NotWindows
}

pub fn download_and_install(_log: &mut Vec<String>) -> Result<()> {
    Err(DriverError::NotWindows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_check_returns_not_windows() {
        assert_eq!(check_required_drivers(), DriverStatus::NotWindows);
    }

    #[test]
    fn linux_install_errors_with_not_windows() {
        let mut log = Vec::new();
        let err = download_and_install(&mut log).unwrap_err();
        assert!(matches!(err, DriverError::NotWindows));
    }
}
