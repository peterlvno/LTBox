//! Cross-platform device-driver / udev-rule status + installer.
//!
//! Used by the GUI's startup driver-check banner + the Settings
//! "Install Drivers" action. Public surface stays the same regardless
//! of host OS so the GUI never has to branch by `cfg!(windows)`; the
//! platform impl picks how to interpret `Present` / `Missing` /
//! `NotWindows` for the local OS.
//!
//! ## Platform impl mapping
//!
//! | Host    | Module                  | Strategy |
//! |---------|-------------------------|----------|
//! | Windows | `driver::windows`       | `pnputil /enum-drivers` + DriverStore probe; install via `pnputil /add-driver` after fetching the latest `qcom-usb-kernel-drivers` release. |
//! | Linux   | `driver::linux` (stub)  | Returns `NotWindows` for now. Hardware-validated probe (udev rule presence + `/sys/bus/usb/devices` walk for `05c6:9008` + serial-node permission test) lands once a Lenovo Qualcomm target is in reach to test against. |
//! | macOS   | `driver::linux` (reused) | Same `NotWindows` stub; LTBox has no macOS driver story today. |
//!
//! The shared `DriverStatus` / `DriverError` / `Result` types live
//! here so a future Linux variant expansion (`UdevRuleMissing`,
//! `DevicePresentNoSerialNode`, …) is one enum-arm change instead of
//! a per-platform shim. Windows + Linux currently produce the same
//! three variants — the rename is structural; behaviour is unchanged
//! from the pre-rename `windows_driver` module.

/// Shape returned by [`check_required_drivers`].
///
/// Variants today match the pre-rename Windows-only module's surface
/// so the GUI's banner / install-prompt logic compiles without
/// changes. Linux's hardware-validated states (`UdevRuleMissing`,
/// `DevicePresentNoSerialNode`, …) will be added as enum arms once
/// the hardware testing pass proves the probe code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverStatus {
    /// Host has no driver-action concept (Linux + macOS today; the
    /// Linux stub will refine this once udev probing lands).
    NotWindows,
    /// Every required driver / rule is in place.
    Present,
    /// Windows: list of `.inf` filenames not yet installed.
    Missing(Vec<&'static str>),
}

#[derive(thiserror::Error, Debug)]
pub enum DriverError {
    #[error("Not running on Windows — driver install is only supported on Windows")]
    NotWindows,
    // ureq has no thiserror-friendly root error, so collapse transport + status.
    #[error("Network error: {0}")]
    Http(String),
    #[error("GitHub release parse error: {0}")]
    Parse(String),
    #[error("No matching driver asset found in the latest release")]
    NoAsset,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Zip extraction error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("No .inf files found under a Windows10 subdirectory in the archive")]
    NoInf,
    /// `pnputil /add-driver /install` rejected every `.inf` — usually
    /// because LTBox is not running elevated. Carries the count so the
    /// GUI can surface "0/N succeeded" without knowing how many INFs
    /// the archive shipped.
    #[error(
        "pnputil failed for all {count} driver(s). LTBox needs to run as Administrator to install drivers."
    )]
    PnputilAllFailed { count: usize },
}

impl From<ureq::Error> for DriverError {
    fn from(e: ureq::Error) -> Self {
        DriverError::Http(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, DriverError>;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{check_required_drivers, download_and_install};

#[cfg(not(windows))]
mod linux;
#[cfg(not(windows))]
pub use self::linux::{check_required_drivers, download_and_install};
