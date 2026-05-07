//! ADB client via `adb_client` crate (ADB server at localhost:5037).

use adb_client::ADBDeviceExt;
use adb_client::RebootType;
use adb_client::server::ADBServer;
use adb_client::server_device::ADBServerDevice;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AdbError {
    #[error("ADB error: {0}")]
    Client(String),
    #[error("Device not found")]
    DeviceNotFound,
    #[error("Command failed: {0}")]
    CommandFailed(String),
    #[error("Timeout waiting for device")]
    Timeout,
}

type Result<T> = std::result::Result<T, AdbError>;

/// Upper bound on `wait_for_device` before surfacing `Timeout`. Matches v2's
/// `DeviceController` ~120s expectation for post-reboot re-detection.
const WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

pub struct AdbManager {
    server_addr: SocketAddrV4,
    serial: Option<String>,
    pub skip_adb: bool,
    pub connected_once: bool,
}

impl AdbManager {
    pub fn new() -> Self {
        Self {
            server_addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 5037),
            serial: None,
            skip_adb: false,
            connected_once: false,
        }
    }

    /// Convenience for the `let mut adb = AdbManager::new(); if
    /// adb.check_device().unwrap_or(false) { ... }` pattern repeated
    /// across the GUI, root pipeline, and rescue flows. Returns
    /// `Some(adb)` when a fully-authorized device is reachable, `None`
    /// otherwise (including when the underlying probe errored — every
    /// existing caller swallowed the error via `unwrap_or(false)`).
    pub fn new_if_connected() -> Option<Self> {
        let mut adb = Self::new();
        if adb.check_device().unwrap_or(false) {
            Some(adb)
        } else {
            None
        }
    }

    fn server(&self) -> ADBServer {
        ADBServer::new(self.server_addr)
    }

    /// Last-known serial captured by `check_device` /
    /// `check_device_state` / `wait_for_device`. `None` until the first
    /// successful probe; never cleared by `AdbManager`.
    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    fn device(&self) -> Result<ADBServerDevice> {
        let serial = self.serial.clone().ok_or(AdbError::DeviceNotFound)?;
        Ok(ADBServerDevice::new(serial, Some(self.server_addr)))
    }

    /// Probe for a *fully-authorized* ADB device; updates stored serial.
    ///
    /// Only devices in state `Device` are treated as connected. v2
    /// `device/adb.py::wait_for_device` used `state == "device"` for the
    /// same reason: an `unauthorized` / `offline` / `authorizing` device
    /// shows up in `adb devices` but every `shell` call will fail, so
    /// treating it as connected sends destructive operations into a
    /// guaranteed mid-flow failure. Callers that *want* to see non-Device
    /// states should use [`check_device_state`](Self::check_device_state).
    pub fn check_device(&mut self) -> Result<bool> {
        let mut server = self.server();
        match server.devices() {
            Ok(devices) => {
                if let Some(dev) = devices
                    .iter()
                    .find(|d| matches!(d.state, adb_client::server::DeviceState::Device))
                {
                    self.serial = Some(dev.identifier.clone());
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Err(_) => Ok(false),
        }
    }

    /// Like `check_device` but returns the raw ADB state token
    /// (`"device"`, `"unauthorized"`, …) so callers can pattern-match
    /// without importing `adb_client::DeviceState`.
    pub fn check_device_state(&mut self) -> Result<Option<&'static str>> {
        let mut server = self.server();
        let Ok(devices) = server.devices() else {
            return Ok(None);
        };
        let Some(dev) = devices.into_iter().next() else {
            return Ok(None);
        };
        self.serial = Some(dev.identifier);
        Ok(Some(match dev.state {
            adb_client::server::DeviceState::Device => "device",
            adb_client::server::DeviceState::Unauthorized => "unauthorized",
            adb_client::server::DeviceState::Authorizing => "authorizing",
            adb_client::server::DeviceState::Offline => "offline",
            adb_client::server::DeviceState::Recovery => "recovery",
            adb_client::server::DeviceState::Bootloader => "bootloader",
            adb_client::server::DeviceState::Sideload => "sideload",
            adb_client::server::DeviceState::Rescue => "rescue",
            adb_client::server::DeviceState::Connecting => "connecting",
            adb_client::server::DeviceState::NoPerm => "noperm",
            adb_client::server::DeviceState::Detached => "detached",
            adb_client::server::DeviceState::Host => "host",
            adb_client::server::DeviceState::NoDevice => "no device",
        }))
    }

    /// Wait up to [`WAIT_TIMEOUT`] for a fully-authorized ADB device.
    ///
    /// Returns `AdbError::Timeout` on expiry instead of spinning forever —
    /// important for GUI flows where the user might have toggled `skip_adb`
    /// or the device got stuck in `unauthorized` state that will never
    /// promote without user action.
    pub fn wait_for_device(&mut self) -> Result<()> {
        if self.skip_adb {
            return Err(AdbError::DeviceNotFound);
        }
        let deadline = std::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            if self.check_device()? {
                self.connected_once = true;
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(AdbError::Timeout);
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    /// Run shell command; returns trimmed stdout.
    pub fn shell(&self, cmd: &str) -> Result<String> {
        let mut dev = self.device()?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        dev.shell_command(
            &cmd.to_string(),
            Some(&mut stdout as &mut dyn std::io::Write),
            Some(&mut stderr as &mut dyn std::io::Write),
        )
        .map_err(|e| AdbError::CommandFailed(e.to_string()))?;
        Ok(String::from_utf8_lossy(&stdout).trim().to_string())
    }

    pub fn get_model(&self) -> Result<Option<String>> {
        match self.shell("getprop ro.product.model") {
            Ok(m) if !m.is_empty() => Ok(Some(m)),
            _ => Ok(None),
        }
    }

    /// Active slot suffix — only `"_a"` or `"_b"`, else `None`.
    ///
    /// v2 parity: `device/adb.py::get_slot_suffix` whitelisted `["_a", "_b"]`
    /// and returned `None` otherwise. Some bootloaders return an empty or
    /// garbage prop on non-A/B devices; feeding that into downstream
    /// `vendor_boot{suffix}` partition names produces lookups for e.g.
    /// `vendor_bootxyz` that fail with a misleading error.
    pub fn get_slot_suffix(&self) -> Result<Option<String>> {
        match self.shell("getprop ro.boot.slot_suffix") {
            Ok(s) if s == "_a" || s == "_b" => Ok(Some(s)),
            _ => Ok(None),
        }
    }

    pub fn get_kernel_version(&self) -> Result<Option<String>> {
        match self.shell("cat /proc/version") {
            Ok(v) => {
                if let Some(start) = v.find("Linux version ") {
                    let rest = &v[start + 14..];
                    let ver: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '.')
                        .collect();
                    if !ver.is_empty() {
                        return Ok(Some(ver));
                    }
                }
                Ok(None)
            }
            Err(_) => Ok(None),
        }
    }

    pub fn reboot(&mut self, target: &str) -> Result<()> {
        let mut dev = self.device()?;
        let reboot_type = match target {
            "bootloader" => RebootType::Bootloader,
            "recovery" => RebootType::Recovery,
            "sideload" => RebootType::Sideload,
            _ => RebootType::System,
        };
        // RebootType has no EDL variant; fall back to shell.
        if target == "edl" {
            self.shell("reboot edl")?;
            return Ok(());
        }
        dev.reboot(reboot_type)
            .map_err(|e| AdbError::CommandFailed(e.to_string()))
    }

    pub fn install(&self, apk_path: &str) -> Result<()> {
        let mut dev = self.device()?;
        let path = Path::new(apk_path);
        dev.install(path, None)
            .map_err(|e| AdbError::CommandFailed(e.to_string()))
    }
}

impl Default for AdbManager {
    fn default() -> Self {
        Self::new()
    }
}
