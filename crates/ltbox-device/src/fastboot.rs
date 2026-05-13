//! Minimal Fastboot over nusb — only the commands LTBox uses
//! (getvar, oem edl, reboot, reboot-bootloader, detect). Protocol:
//! ASCII command → bulk write → read OKAY/FAIL/DATA/INFO.

use nusb::Endpoint;
use nusb::transfer::{Buffer, Bulk, In, Out};
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FastbootError {
    #[error("USB error: {0}")]
    Usb(String),
    #[error("Device not found")]
    DeviceNotFound,
    #[error("Command failed: {0}")]
    CommandFailed(String),
    #[error("Timeout")]
    Timeout,
}

type Result<T> = std::result::Result<T, FastbootError>;

const FASTBOOT_USB_CLASS: u8 = 0xFF;
const FASTBOOT_USB_SUBCLASS: u8 = 0x42;
const FASTBOOT_USB_PROTOCOL: u8 = 0x03;

const WAIT_SLEEP: Duration = Duration::from_secs(2);

#[derive(Debug, Default, Clone)]
pub struct FastbootVars {
    pub model: Option<String>,
    pub product: Option<String>,
    pub serialno: Option<String>,
    pub current_slot: Option<String>,
    pub build_display_id: Option<String>,
    pub ram_gb: Option<String>,
    pub storage_gb: Option<String>,
    pub rollback_indices: std::collections::HashMap<u32, u64>,
}

pub struct FastbootDevice {
    // Kept alive so the endpoints below stay bound to the claim.
    _interface: nusb::Interface,
    ep_in: Endpoint<Bulk, In>,
    ep_out: Endpoint<Bulk, Out>,
}

impl FastbootDevice {
    pub fn open() -> Result<Self> {
        use nusb::MaybeFuture;

        let devices = nusb::list_devices()
            .wait()
            .map_err(|e| FastbootError::Usb(e.to_string()))?
            .collect::<Vec<_>>();

        for dev_info in devices {
            let Ok(device) = dev_info.open().wait() else {
                continue;
            };
            for config in device.configurations() {
                for iface in config.interfaces() {
                    for alt in iface.alt_settings() {
                        if alt.class() != FASTBOOT_USB_CLASS
                            || alt.subclass() != FASTBOOT_USB_SUBCLASS
                            || alt.protocol() != FASTBOOT_USB_PROTOCOL
                        {
                            continue;
                        }
                        let mut in_addr: u8 = 0;
                        let mut out_addr: u8 = 0;
                        for ep in alt.endpoints() {
                            match ep.direction() {
                                nusb::transfer::Direction::In => in_addr = ep.address(),
                                nusb::transfer::Direction::Out => out_addr = ep.address(),
                            }
                        }
                        if in_addr == 0 || out_addr == 0 {
                            continue;
                        }
                        let interface = device
                            .claim_interface(iface.interface_number())
                            .wait()
                            .map_err(|e| FastbootError::Usb(e.to_string()))?;
                        let ep_in = interface
                            .endpoint::<Bulk, In>(in_addr)
                            .map_err(|e| FastbootError::Usb(e.to_string()))?;
                        let ep_out = interface
                            .endpoint::<Bulk, Out>(out_addr)
                            .map_err(|e| FastbootError::Usb(e.to_string()))?;
                        return Ok(Self {
                            _interface: interface,
                            ep_in,
                            ep_out,
                        });
                    }
                }
            }
        }

        Err(FastbootError::DeviceNotFound)
    }

    pub fn check_device() -> bool {
        Self::open().is_ok()
    }

    pub fn wait_for_device() -> Result<Self> {
        loop {
            match Self::open() {
                Ok(dev) => return Ok(dev),
                Err(FastbootError::DeviceNotFound) => {
                    std::thread::sleep(WAIT_SLEEP);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Submit `buf` on the OUT endpoint and block for completion.
    fn bulk_write(&mut self, buf: Vec<u8>) -> Result<()> {
        self.ep_out.submit(Buffer::from(buf));
        let completion = pollster::block_on(self.ep_out.next_complete());
        completion
            .status
            .map_err(|e| FastbootError::Usb(e.to_string()))?;
        Ok(())
    }

    /// Submit a 4 KiB read on the IN endpoint and block for completion.
    /// Returns the initialized prefix of the filled buffer.
    fn bulk_read(&mut self) -> Result<Vec<u8>> {
        self.ep_in.submit(Buffer::new(4096));
        let completion = pollster::block_on(self.ep_in.next_complete());
        completion
            .status
            .map_err(|e| FastbootError::Usb(e.to_string()))?;
        let len = completion.actual_len;
        let mut out = completion.buffer.into_vec();
        out.truncate(len);
        Ok(out)
    }

    /// Send command and read until OKAY/FAIL.
    fn command(&mut self, cmd: &str) -> Result<String> {
        self.bulk_write(cmd.as_bytes().to_vec())?;

        loop {
            let data = self.bulk_read()?;

            if data.len() < 4 {
                return Err(FastbootError::CommandFailed("Short response".into()));
            }

            let status = std::str::from_utf8(&data[..4]).unwrap_or("");
            let payload = std::str::from_utf8(&data[4..]).unwrap_or("").trim();

            match status {
                "OKAY" => return Ok(payload.to_string()),
                "FAIL" => return Err(FastbootError::CommandFailed(payload.to_string())),
                "INFO" => continue,
                "DATA" => return Ok(payload.to_string()),
                _ => return Err(FastbootError::CommandFailed(format!("Unknown: {status}"))),
            }
        }
    }

    /// Send command, collect all INFO lines.
    fn command_all(&mut self, cmd: &str) -> Result<Vec<String>> {
        self.bulk_write(cmd.as_bytes().to_vec())?;

        let mut lines = Vec::new();
        loop {
            let data = self.bulk_read()?;
            if data.len() < 4 {
                break;
            }
            let status = std::str::from_utf8(&data[..4]).unwrap_or("");
            let payload = std::str::from_utf8(&data[4..])
                .unwrap_or("")
                .trim()
                .to_string();
            match status {
                "INFO" => lines.push(payload),
                "OKAY" => break,
                "FAIL" => return Err(FastbootError::CommandFailed(payload)),
                _ => break,
            }
        }
        Ok(lines)
    }

    pub fn getvar(&mut self, variable: &str) -> Result<String> {
        self.command(&format!("getvar:{variable}"))
    }

    pub fn get_model(&mut self) -> Result<Option<String>> {
        match self.getvar("product") {
            Ok(v) if !v.is_empty() => Ok(Some(v)),
            _ => Ok(None),
        }
    }

    /// Active slot suffix (`_a` / `_b`).
    pub fn get_slot_suffix(&mut self) -> Result<Option<String>> {
        match self.getvar("current-slot") {
            Ok(slot) if !slot.is_empty() => {
                let suffix = if slot.starts_with('_') {
                    slot
                } else {
                    format!("_{slot}")
                };
                Ok(Some(suffix))
            }
            _ => Ok(None),
        }
    }

    /// Parse vars from `getvar:all` INFO lines.
    pub fn get_all_vars(&mut self) -> Result<FastbootVars> {
        let mut vars = FastbootVars {
            current_slot: self.get_slot_suffix()?,
            ..FastbootVars::default()
        };
        if let Ok(sn) = self.getvar("serialno")
            && !sn.is_empty()
        {
            vars.serialno = Some(sn);
        }
        if let Ok(lines) = self.command_all("getvar:all") {
            for line in &lines {
                // hwboardid layout varies per SKU:
                //   `TB322FC_SM8750P_16+512`  (model + SoC + spec)
                //   `SM8750P_16+512`          (SoC + spec, no model token)
                // RAM/storage always sit in the trailing `<n>+<n>` block,
                // so we parse them off the tail regardless of layout.
                // Model identification moved to the dedicated
                // `modelname:` line below — the leading hwboardid token
                // is the SoC name on stripped SKUs and not a reliable
                // model source.
                if let Some(val) = line.strip_prefix("hwboardid:") {
                    let val = val.trim();
                    if let Some((_prefix, tail)) = val.rsplit_once('_') {
                        if let Some((ram, storage)) = tail.split_once('+') {
                            vars.ram_gb = Some(format!("{ram} GB"));
                            vars.storage_gb = Some(format!("{storage} GB"));
                        }
                    } else if let Some((ram, storage)) = val.split_once('+') {
                        // Single-token form like `16+512` (defensive
                        // fallback — no SKU observed shipping this).
                        vars.ram_gb = Some(format!("{ram} GB"));
                        vars.storage_gb = Some(format!("{storage} GB"));
                    }
                }
                // `modelname:TB322FC` — the bootloader-published model
                // identifier. Stable across SKUs that strip the model
                // token from `hwboardid`.
                if let Some(val) = line.strip_prefix("modelname:") {
                    let val = val.trim();
                    if !val.is_empty() {
                        vars.model = Some(val.to_string());
                    }
                }
                if let Some((slot, val)) = parse_stored_rollback_line(line) {
                    vars.rollback_indices.insert(slot, val);
                }
                if let Some(val) = line.strip_prefix("build-display-id:") {
                    let v = val.trim();
                    if !v.is_empty() {
                        vars.build_display_id = Some(v.to_string());
                    }
                } else if let Some(val) = line.strip_prefix("build.display.id:") {
                    let v = val.trim();
                    if !v.is_empty() {
                        vars.build_display_id = Some(v.to_string());
                    }
                }
            }
        }
        // product = market_name in GUI
        if let Ok(p) = self.get_model() {
            vars.product = p;
        }
        Ok(vars)
    }

    pub fn oem_edl(&mut self) -> Result<()> {
        self.command("oem edl").map(|_| ())
    }

    pub fn reboot(&mut self) -> Result<()> {
        self.command("reboot").map(|_| ())
    }

    pub fn reboot_bootloader(&mut self) -> Result<()> {
        self.command("reboot-bootloader").map(|_| ())
    }
}

/// Parse `stored_rollback_index:N = HEX`. Value is always base-16.
/// Tolerates `(N)`/`N)` slot wrapping and optional `0x` prefix.
pub(crate) fn parse_stored_rollback_line(line: &str) -> Option<(u32, u64)> {
    let rest = line.strip_prefix("stored_rollback_index:")?;
    let (slot_str, val_str) = rest.split_once('=')?;
    let slot_str = slot_str
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    let val_str = val_str.trim();
    let slot: u32 = slot_str.parse().ok()?;
    let hex = val_str.strip_prefix("0x").unwrap_or(val_str);
    let val = u64::from_str_radix(hex, 16).ok()?;
    Some((slot, val))
}

#[cfg(test)]
mod tests {
    use super::parse_stored_rollback_line;

    #[test]
    fn bare_hex_parses_as_base16() {
        // Regression: bare hex previously fell through to 0 via unwrap_or(0).
        let out = parse_stored_rollback_line("stored_rollback_index:0 = 41B7A200");
        assert_eq!(out, Some((0, 0x41B7A200)));
    }

    #[test]
    fn prefixed_hex_still_parses() {
        let out = parse_stored_rollback_line("stored_rollback_index:1 = 0xDEADBEEF");
        assert_eq!(out, Some((1, 0xDEADBEEF)));
    }

    #[test]
    fn small_decimal_digits_parse_as_hex() {
        // Contract: always base-16 (v2 `int(_, 16)`). "100" → 0x100.
        let out = parse_stored_rollback_line("stored_rollback_index:0 = 100");
        assert_eq!(out, Some((0, 0x100)));
    }

    #[test]
    fn malformed_line_returns_none() {
        assert!(parse_stored_rollback_line("unrelated:0 = ff").is_none());
        assert!(parse_stored_rollback_line("stored_rollback_index:not_a_slot = ff").is_none());
        assert!(parse_stored_rollback_line("stored_rollback_index:0 = not_hex_ghi").is_none());
        assert!(parse_stored_rollback_line("stored_rollback_index:0").is_none());
    }

    #[test]
    fn trailing_paren_on_slot_is_stripped() {
        let out = parse_stored_rollback_line("stored_rollback_index:0) = 41B7A200");
        assert_eq!(out, Some((0, 0x41B7A200)));
    }

    #[test]
    fn both_parens_on_slot_are_stripped() {
        // Regression: `(0)` used to fail to parse and silently skip ARB.
        let out = parse_stored_rollback_line("stored_rollback_index:(0) = 41B7A200");
        assert_eq!(out, Some((0, 0x41B7A200)));
    }
}
