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
//! | Windows | `driver::windows`       | `pnputil /enum-drivers` + DriverStore probe for `qcserlib.inf` (the WinUSB stub for PID 9008); install by downloading the signed per-arch `qcom_usb_userspace_drivers_<arch>.exe` from the latest `qcom-usb-userspace-drivers` release and launching it via UAC (`Start-Process -Verb RunAs`). The installer self-elevates, so LTBox itself does not need to run as Administrator. |
//! | Linux   | `driver::linux` (stub)  | Returns `NotWindows` for now. Hardware-validated probe (udev rule presence + `/sys/bus/usb/devices` walk for `05c6:9008` + serial-node permission test) lands once a Qualcomm target is in reach to test against. |
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

/// Result of comparing the locally-installed Qualcomm driver version
/// against the latest signed release on GitHub. Only produced when a
/// driver is already present AND a strictly-newer release exists — the
/// GUI uses its presence to decide whether to show the optional
/// "update available" banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverUpdate {
    /// Dotted version parsed from the installed `qcserlib.inf` `DriverVer`.
    pub current: String,
    /// Dotted version parsed from the latest Windows release tag.
    pub latest: String,
}

#[derive(thiserror::Error, Debug)]
pub enum DriverError {
    #[error("Not running on Windows — driver install is only supported on Windows")]
    NotWindows,
    // ureq has no thiserror-friendly root error, so collapse transport + status.
    #[error("Network error: {0}")]
    Http(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// GitHub release JSON could not be parsed.
    #[error("GitHub release parse error: {0}")]
    Parse(String),
    /// The latest `qcom-usb-userspace-drivers` release shipped no signed
    /// installer `.exe` for the host architecture.
    #[error("No matching signed installer found in the latest release")]
    NoAsset,
    /// The user dismissed the Windows UAC elevation prompt, so the signed
    /// installer never ran. Distinct from `InstallerFailed` so the GUI can
    /// say "approve the prompt and try again" instead of a generic error.
    /// The installer `.exe` self-elevates, so LTBox itself does NOT need to
    /// run as Administrator — the UAC prompt is the only elevation step.
    #[error("Driver install was cancelled at the Windows elevation prompt.")]
    InstallCancelled,
    /// The signed installer `.exe` exited with a non-zero status.
    #[error("Driver installer exited with code {exit_code}.")]
    InstallerFailed { exit_code: i32 },
}

impl From<ureq::Error> for DriverError {
    fn from(e: ureq::Error) -> Self {
        DriverError::Http(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, DriverError>;

/// Best-effort reachability probe for the GitHub host LTBox downloads the
/// Windows Qualcomm-driver installer from. Used to pre-disable the install /
/// update buttons (with an "internet required" tooltip) instead of letting the
/// user click into a download that can only fail. A short timeout keeps a dead
/// network from stalling startup; any transport / non-2xx result reads as
/// offline.
#[cfg(windows)]
pub fn probe_connectivity() -> bool {
    let agent = ureq::Agent::config_builder()
        .user_agent(concat!("ltbox/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(std::time::Duration::from_secs(8)))
        .build()
        .new_agent();
    agent.get("https://api.github.com/").call().is_ok()
}

/// The driver install / update buttons this gates only exist on Windows
/// (Linux + macOS need no Qualcomm driver), so off-Windows this reports
/// "reachable" immediately — no startup network round-trip to GitHub.
#[cfg(not(windows))]
pub fn probe_connectivity() -> bool {
    true
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{check_driver_update, check_required_drivers, download_and_install};

#[cfg(not(windows))]
mod linux;
#[cfg(not(windows))]
pub use self::linux::{check_driver_update, check_required_drivers, download_and_install};
