//! EDL (Emergency Download) — Qualcomm 9008 USB device detection and
//! session management (Sahara → Firehose configure → operations).
//!
//! Transport is `QdlBackend::Usb` (WinUSB stub on Windows via
//! `qcom-usb-userspace-drivers` / libusb on Linux). The previous
//! `QdlBackend::Serial` path went through the kernel-mode usbser COM
//! port — upstream `qdl::serial` set no read/write timeout (literal
//! `// TODO: timeouts?` in the source), so any device-side stall larger
//! than the OS-default serial timeout punched through qdl's
//! `firehose_program_storage` `.expect("Error sending data")` and aborted
//! the whole flash mid-partition. `QdlBackend::Usb` sets explicit 10 s
//! endpoint timeouts at the `nusb` layer so the same stall surfaces as a
//! recoverable `io::Error` instead.

use std::io::{Cursor, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;

use ltbox_core::i18n::tr;

use qdl::types::{
    FirehoseConfiguration, FirehoseResetMode, FirehoseStorageType, QdlBackend, QdlChan, QdlDevice,
    QdlReadWrite,
};

const QUALCOMM_VID: u16 = 0x05C6;
const QUALCOMM_EDL_PID: u16 = 0x9008;

/// Stable identifier returned by [`find_edl_device`] / [`wait_for_edl_device`]
/// when the EDL endpoint is visible via libusb. The actual `nusb::Device`
/// is opened later by qdl's USB backend (`qdl::usb::setup_usb_device`);
/// LTBox only needs a presence marker for logging + post-reset stability
/// checks, so we return a small synthetic string instead of plumbing a
/// raw `nusb` handle through the wait loop (which would tie the abstract
/// `wait_for_stable_port_with` helper to a concrete USB type and break
/// the unit tests that drive it with `String` ports).
const EDL_DEVICE_MARKER: &str = "USB:VID_05C6&PID_9008";

/// Build + send a Firehose `<erase>` XML to the device.
///
/// Inlined here instead of calling a `qdl::firehose_erase_storage`
/// wrapper so the dependency surface stays on the upstream-portable
/// `qdl::firehose_write_getack` primitive. Mirrors the v2 Python
/// flow that hand-writes a `FHLoaderErase.xml` and feeds it into
/// the Firehose pass: same XML payload, same end behaviour.
fn send_firehose_erase(
    dev: &mut QdlDevice<dyn QdlReadWrite>,
    num_sectors: usize,
    lun: u8,
    start_sector: &str,
) -> std::result::Result<(), String> {
    let sector_size = dev.fh_cfg.storage_sector_size;
    // Self-closed `<erase>` inside a `<data>` root with the XML
    // declaration matches what xmltree emits in qdl's internal
    // `firehose_xml_setup`. Firehose's parser is lenient about
    // whitespace but strict about attribute names + spelling.
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><data><erase SECTOR_SIZE_IN_BYTES="{sector_size}" num_partition_sectors="{num_sectors}" physical_partition_number="{lun}" start_sector="{start_sector}" /></data>"#
    );
    let mut buf = xml.into_bytes();
    qdl::firehose_write_getack(
        dev,
        &mut buf,
        format!("erase sectors {start_sector}..+{num_sectors}"),
    )
    .map_err(|e| format!("{e}"))
}

const EDL_STABILITY_INTERVAL: Duration = Duration::from_secs(1);
const EDL_DISCONNECT_OBSERVE: Duration = Duration::from_secs(5);
const EDL_SESSION_OPEN_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Error, Debug)]
pub enum EdlError {
    #[error("EDL port not found")]
    PortNotFound,
    #[error("Timed out after {0:?} waiting for a stable EDL port")]
    PortTimeout(Duration),
    #[error("Serial error: {0}")]
    Serial(String),
    #[error("EDL session error: {0}")]
    Session(String),
    #[error("Partition not found: {0}")]
    PartitionNotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, EdlError>;

/// Scan for the Qualcomm 9008 EDL endpoint via libusb. Returns
/// [`EDL_DEVICE_MARKER`] when at least one matching device is enumerated,
/// otherwise [`EdlError::PortNotFound`].
///
/// Implemented with `nusb::list_devices()` to mirror the discovery path
/// `qdl::usb::setup_usb_device` will take when actually opening the
/// transport — keeping both probes on the same enumeration source avoids
/// "visible to probe, invisible to open" mismatch (the classic failure
/// mode of the previous code, which probed `serialport::available_ports`
/// and then opened a totally different transport on top of it).
pub fn find_edl_device() -> Result<String> {
    use nusb::MaybeFuture;
    let devices = nusb::list_devices()
        .wait()
        .map_err(|e| EdlError::Serial(format!("USB device enumeration failed: {e}")))?;
    for d in devices {
        if d.vendor_id() == QUALCOMM_VID && d.product_id() == QUALCOMM_EDL_PID {
            return Ok(EDL_DEVICE_MARKER.to_string());
        }
    }
    Err(EdlError::PortNotFound)
}

pub fn check_device() -> bool {
    find_edl_device().is_ok()
}

/// Wait for a stable EDL port after a reset. Two phases:
/// 1. If a port is visible, observe for `EDL_DISCONNECT_OBSERVE`; treat a
///    name change or disconnect as stale and fall through.
/// 2. Otherwise, return when the same port is seen twice in a row
///    (`EDL_STABILITY_INTERVAL` apart).
///
/// Returns [`EdlError::PortTimeout`] once `deadline_exceeded()` trips.
fn wait_for_stable_port_with<F, S, D>(
    mut find_port: F,
    mut sleep: S,
    mut deadline_exceeded: D,
    timeout: Duration,
) -> Result<String>
where
    F: FnMut() -> Result<String>,
    S: FnMut(Duration),
    D: FnMut() -> bool,
{
    // Phase 1: watch for disconnect OR name change; the OS can keep the
    // old COM handle alive briefly while the new one enumerates.
    if let Ok(initial) = find_port() {
        let mut observed = Duration::ZERO;
        let mut saw_disconnect = false;
        while observed < EDL_DISCONNECT_OBSERVE && !deadline_exceeded() {
            sleep(EDL_STABILITY_INTERVAL);
            observed += EDL_STABILITY_INTERVAL;
            match find_port() {
                Ok(current) if current == initial => {}
                // Name changed → old handle stale. Phase 2 tolerates the move.
                Ok(_) => {
                    saw_disconnect = true;
                    break;
                }
                Err(EdlError::PortNotFound) => {
                    saw_disconnect = true;
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        if !saw_disconnect {
            return Ok(initial);
        }
    }

    // Phase 2: wait for post-reset port to appear + stabilize.
    let mut last: Option<String> = None;
    while !deadline_exceeded() {
        sleep(EDL_STABILITY_INTERVAL);
        match find_port() {
            Ok(port) => {
                if last.as_deref() == Some(port.as_str()) {
                    return Ok(port);
                }
                last = Some(port);
            }
            Err(EdlError::PortNotFound) => last = None,
            Err(e) => return Err(e),
        }
    }

    Err(EdlError::PortTimeout(timeout))
}

fn wait_for_stable_port() -> Result<String> {
    let deadline = Instant::now() + EDL_SESSION_OPEN_TIMEOUT;
    wait_for_stable_port_with(
        find_edl_device,
        std::thread::sleep,
        move || Instant::now() >= deadline,
        EDL_SESSION_OPEN_TIMEOUT,
    )
}

/// Wait for an EDL device; returns the stable-device marker string.
///
/// Name kept (`wait_for_device`) to avoid an extra rename churn through
/// `controller.rs`; the underlying transport just moved from a COM port
/// name to a libusb VID/PID marker.
pub fn wait_for_device() -> Result<String> {
    wait_for_stable_port()
}

/// One partition entry surfaced by [`EdlSession::scan_partitions`].
#[derive(Debug, Clone)]
pub struct GptPartitionInfo {
    pub lun: u8,
    pub name: String,
    pub start_sector: u64,
    pub num_sectors: u64,
    pub size_bytes: u64,
}

/// EDL session: owns `QdlDevice`, exposes partition ops.
pub struct EdlSession {
    dev: QdlDevice<dyn QdlReadWrite>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WipeErasePlanEntry {
    label: String,
    lun: u8,
    start_sector: String,
    num_sectors: usize,
}

impl WipeErasePlanEntry {
    fn log_line(&self) -> String {
        self.log_line_with_template(&tr("log_edl_pre_erase_cmd"))
    }

    fn log_line_with_template(&self, template: &str) -> String {
        format!(
            "[EDL] {}",
            template
                .replace("{label}", &self.label)
                .replace("{lun}", &self.lun.to_string())
                .replace("{start}", &self.start_sector)
                .replace("{sectors}", &self.num_sectors.to_string())
        )
    }
}

impl EdlSession {
    /// Open: find port → Sahara upload → Firehose configure.
    ///
    /// `auto_reset` ignored — `reset_on_drop` stays `false` to avoid qdl's
    /// recursive drop-time reset stack overflow. Call [`EdlSession::reset`]
    /// explicitly on the happy path.
    pub fn open(loader_path: &Path, auto_reset: bool, log: &mut Vec<String>) -> Result<Self> {
        let _ = auto_reset;
        ltbox_core::live!(log, "[EDL] {}", tr("log_edl_scanning"));
        let port = wait_for_stable_port()?;
        // `port` is now a libusb marker string ("USB:VID_05C6&PID_9008"),
        // not a COM port name — the log line wording is generic enough
        // ("found on …") that the swap doesn't require an i18n update.
        ltbox_core::live!(log, "[EDL] {} {port}", tr("log_edl_found_on"));

        // An encrypted manifest (`qsahara_device_programmer.x`) is decrypted
        // to the plaintext `.xml` beside it before use, so its per-id images
        // (referenced relative to that directory) still resolve. Some Lenovo
        // firmware packs ship the manifest only in this encrypted form. This
        // is the single point loaders are consumed, so decrypting here covers
        // every caller (Flash / Unroot / Rescue / DetectArb / Dump).
        let decrypted_holder;
        let loader_path: &Path =
            if ltbox_core::sahara_xml::is_encrypted_manifest_filename(loader_path) {
                let out = loader_path.with_file_name(ltbox_core::sahara_xml::MANIFEST_FILENAME);
                ltbox_core::crypto::decrypt_file(loader_path, &out).map_err(|e| {
                    EdlError::Session(format!(
                        "Decrypt loader manifest {}: {e}",
                        loader_path.display()
                    ))
                })?;
                decrypted_holder = out;
                decrypted_holder.as_path()
            } else {
                loader_path
            };

        // Manifest (TB323FU/kaanapali): slot array indexed by Sahara image-id.
        // Single loader (.melf/.mbn/.elf): one-element slice at slot 0.
        let mut slots: Vec<Option<Vec<u8>>> =
            if ltbox_core::sahara_xml::is_manifest_filename(loader_path) {
                ltbox_core::live!(
                    log,
                    "[EDL] {} {}",
                    tr("log_edl_loading_programmer"),
                    loader_path.display()
                );
                let (slots, paths) = ltbox_core::sahara_xml::load_image_slots(loader_path)
                    .map_err(|e| EdlError::Session(format!("Sahara manifest: {e}")))?;
                let total: usize = slots
                    .iter()
                    .filter_map(|s| s.as_ref().map(|b| b.len()))
                    .sum();
                ltbox_core::live!(
                    log,
                    "[EDL] {} ({} images, {} bytes total)",
                    tr("log_edl_programmer_size"),
                    paths.len(),
                    total
                );
                slots
            } else {
                ltbox_core::live!(
                    log,
                    "[EDL] {} {}",
                    tr("log_edl_loading_programmer"),
                    loader_path.display()
                );
                let mbn = std::fs::read(loader_path)
                    .map_err(|e| EdlError::Session(format!("Failed to read loader: {e}")))?;
                ltbox_core::live!(
                    log,
                    "[EDL] {} {} bytes",
                    tr("log_edl_programmer_size"),
                    mbn.len()
                );
                vec![Some(mbn)]
            };

        ltbox_core::live!(log, "[EDL] {}", tr("log_edl_transport_setup"));
        // `QdlBackend::Usb` — see module-level doc for the rationale on
        // migrating off the Serial COM-port path. `setup_target_device`
        // discovers the device via libusb (VID 05C6 + PID 9008 / 900E)
        // and claims the bulk-in / bulk-out endpoints with explicit
        // 10 s read + write timeouts; no port name or serial number is
        // required when only one EDL device is plugged in.
        let _ = &port; // port marker retained for logging only
        let rw = qdl::setup_target_device(QdlBackend::Usb, None, None)
            .map_err(|e| EdlError::Session(format!("Transport setup failed: {e}")))?;

        let mut dev = QdlDevice {
            rw,
            fh_cfg: FirehoseConfiguration {
                storage_type: FirehoseStorageType::Ufs,
                storage_sector_size: 4096,
                bypass_storage: false,
                backend: QdlBackend::Usb,
                skip_firehose_log: true,
                verbose_firehose: false,
                ..Default::default()
            },
            reset_on_drop: false,
        };

        ltbox_core::live!(log, "[EDL] {}", tr("log_edl_sahara_uploading"));
        // qdl `sahara_run` indexes `img_arr` by image-id when len>1, else slot 0.
        // First attempt: trust PBL to emit Sahara HELLO. `sahara_run`'s loop
        // blocks on the first `channel.read` waiting for the HELLO packet,
        // then replies with HELLO_RESP (qdl `sahara.rs:489-509`).
        let first_attempt = qdl::sahara::sahara_run(
            &mut dev,
            qdl::sahara::SaharaMode::WaitingForImage,
            None,
            &mut slots,
            vec![],
            false,
        );
        match first_attempt {
            Ok(_) => {}
            Err(e) => {
                // Skip-HELLO fallback: mirrors qdl CLI's
                // `--skip-hello-wait` (cli/src/main.rs:246-248): send an
                // unsolicited HELLO_RESP to nudge the PBL state machine
                // forward, then retry `sahara_run`. Only triggered on a
                // timeout-shaped error so happy-path devices keep using the
                // standard handshake (no behavior change unless the first
                // attempt already failed).
                //
                // Primary check: structured downcast to
                // `io::ErrorKind::TimedOut` (qdl-rs propagates the
                // serialport read timeout as an `io::Error` wrapped in
                // `anyhow::Error`). String fallback is defense-in-depth in
                // case a future qdl-rs revision changes the wrapping.
                let timed_out = e
                    .downcast_ref::<std::io::Error>()
                    .map(|io| io.kind() == std::io::ErrorKind::TimedOut)
                    .unwrap_or(false)
                    || {
                        let emsg = e.to_string();
                        emsg.contains("timed out")
                            || emsg.contains("TimedOut")
                            || emsg.contains("timeout")
                    };
                if !timed_out {
                    return Err(EdlError::Session(format!("Sahara failed: {e}")));
                }
                ltbox_core::live!(log, "[EDL] {}", tr("log_edl_sahara_skip_hello_retry"));
                qdl::sahara::sahara_send_hello_rsp(
                    &mut dev,
                    qdl::sahara::SaharaMode::WaitingForImage,
                )
                .map_err(|e| {
                    EdlError::Session(format!("Sahara skip-hello HELLO_RESP send failed: {e}"))
                })?;
                qdl::sahara::sahara_run(
                    &mut dev,
                    qdl::sahara::SaharaMode::WaitingForImage,
                    None,
                    &mut slots,
                    vec![],
                    false,
                )
                .map_err(|e| {
                    EdlError::Session(format!("Sahara failed after skip-hello retry: {e}"))
                })?;
            }
        }
        ltbox_core::live!(log, "[EDL] {}", tr("log_edl_sahara_uploaded"));

        // See `open` doc: reset_on_drop stays false to dodge qdl's recursive reset.
        dev.reset_on_drop = false;

        ltbox_core::live!(log, "[EDL] {}", tr("log_edl_firehose_configuring"));
        qdl::firehose_read(&mut dev, qdl::parsers::firehose_parser_ack_nak)
            .map_err(|e| EdlError::Session(format!("Firehose read failed: {e}")))?;
        qdl::firehose_configure(&mut dev, false)
            .map_err(|e| EdlError::Session(format!("Firehose configure failed: {e}")))?;
        qdl::firehose_read(&mut dev, qdl::parsers::firehose_parser_configure_response)
            .map_err(|e| EdlError::Session(format!("Firehose config response failed: {e}")))?;
        ltbox_core::live!(log, "[EDL] {}", tr("log_edl_firehose_configured"));

        Ok(Self { dev })
    }

    /// Max plausible GPT metadata span (protective MBR + header + entry
    /// array) in sectors. A malformed or hostile GPT can report a huge
    /// `first_usable_lba`; passing it straight to `firehose_read_storage`
    /// as a sector count would allocate/read gigabytes. Real GPTs fit in a
    /// few dozen sectors, so 2048 is a generous ceiling.
    const MAX_GPT_METADATA_SECTORS: u64 = 2048;

    /// Extra sectors appended to a GPT metadata read so the partition-entry
    /// array is never the last sector delivered. The USB Firehose read can
    /// return the final sector of a transfer short/garbled; a fully-populated
    /// GPT (TB323FU LUN 4 carries 128 entries filling LBA 2..=5, i.e. up to
    /// `first_usable_lba`) reads its entry array right up to the buffer's last
    /// byte, so the flaky tail corrupts it and `gptman` fails to parse. Reading
    /// a couple of slack sectors keeps the entry array in the reliable middle;
    /// `gptman` ignores the trailing padding.
    const GPT_READ_TAIL_MARGIN: usize = 2;

    /// Clamp a GPT header's `first_usable_lba` to a sane Firehose read
    /// length, rejecting implausible values instead of over-reading.
    fn gpt_read_sectors(first_usable_lba: u64) -> Result<usize> {
        if first_usable_lba == 0 || first_usable_lba > Self::MAX_GPT_METADATA_SECTORS {
            return Err(EdlError::Session(format!(
                "GPT first_usable_lba {first_usable_lba} outside plausible range 1..={}",
                Self::MAX_GPT_METADATA_SECTORS
            )));
        }
        Ok(first_usable_lba as usize)
    }

    /// Read the GPT of a single LUN. Returns the parsed `gptman::GPT`.
    /// Used by `scan_partitions` and `find_partition`.
    fn read_gpt_for_lun(&mut self, slot: u8, lun: u8) -> Result<gptman::GPT> {
        let mut buf = Cursor::new(Vec::<u8>::new());
        qdl::firehose_read_storage(&mut self.dev, &mut buf, 1, slot, lun, 1)
            .map_err(|e| EdlError::Session(format!("GPT probe failed: {e}")))?;
        buf.rewind()?;
        let header = gptman::GPTHeader::read_from(&mut buf)
            .map_err(|e| EdlError::Session(format!("GPT header parse failed: {e}")))?;
        let gpt_len = Self::gpt_read_sectors(header.first_usable_lba)?;
        let read_len =
            (gpt_len + Self::GPT_READ_TAIL_MARGIN).min(Self::MAX_GPT_METADATA_SECTORS as usize);
        let mut buf = Cursor::new(Vec::<u8>::new());
        qdl::firehose_read_storage(&mut self.dev, &mut buf, read_len, slot, lun, 0)
            .map_err(|e| EdlError::Session(format!("GPT read failed: {e}")))?;
        buf.set_position(self.dev.fh_config().storage_sector_size as u64);
        gptman::GPT::read_from(&mut buf, self.dev.fh_config().storage_sector_size as u64)
            .map_err(|e| EdlError::Session(format!("GPT parse failed: {e}")))
    }

    /// Read the GPT of every LUN in `lun_range` and flatten the named
    /// partitions. GPT placeholder slots (empty name or zero size) are
    /// dropped. Per-LUN failures are logged but do not abort the scan —
    /// devices often expose only a subset of the 0..=5 LUN range.
    pub fn scan_partitions(
        &mut self,
        lun_range: std::ops::RangeInclusive<u8>,
        log: &mut Vec<String>,
    ) -> Result<Vec<GptPartitionInfo>> {
        let sector_size = self.dev.fh_config().storage_sector_size as u64;
        let mut out = Vec::new();
        for lun in lun_range {
            ltbox_core::live!(log, "[EDL] {} LUN {lun}", tr("log_edl_reading_gpt"));
            match self.read_gpt_for_lun(0, lun) {
                Ok(gpt) => {
                    for (_idx, part) in gpt.iter() {
                        let name = part.partition_name.as_str().to_string();
                        let Ok(num_sectors) = part.size() else {
                            continue;
                        };
                        if num_sectors == 0 || name.is_empty() {
                            continue;
                        }
                        out.push(GptPartitionInfo {
                            lun,
                            name,
                            start_sector: part.starting_lba,
                            num_sectors,
                            size_bytes: num_sectors * sector_size,
                        });
                    }
                }
                Err(e) => {
                    ltbox_core::live!(
                        log,
                        "[EDL] {}",
                        tr("log_edl_lun_gpt_read_failed")
                            .replace("{lun}", &lun.to_string())
                            .replace("{error}", &e.to_string())
                    );
                }
            }
        }
        Ok(out)
    }

    /// GPT lookup by partition name.
    fn find_partition(&mut self, part_name: &str, slot: u8, lun: u8) -> Result<(u64, u64)> {
        let mut buf = Cursor::new(Vec::<u8>::new());
        qdl::firehose_read_storage(&mut self.dev, &mut buf, 1, slot, lun, 1)
            .map_err(|e| EdlError::Session(format!("GPT probe failed: {e}")))?;
        buf.rewind()?;
        let header = gptman::GPTHeader::read_from(&mut buf)
            .map_err(|e| EdlError::Session(format!("GPT header parse failed: {e}")))?;
        let gpt_len = Self::gpt_read_sectors(header.first_usable_lba)?;
        // Over-read past the GPT metadata — see `GPT_READ_TAIL_MARGIN`.
        let read_len =
            (gpt_len + Self::GPT_READ_TAIL_MARGIN).min(Self::MAX_GPT_METADATA_SECTORS as usize);
        buf.rewind()?;
        qdl::firehose_read_storage(&mut self.dev, &mut buf, read_len, slot, lun, 0)
            .map_err(|e| EdlError::Session(format!("GPT read failed: {e}")))?;
        buf.set_position(self.dev.fh_config().storage_sector_size as u64);
        let gpt = gptman::GPT::read_from(&mut buf, self.dev.fh_config().storage_sector_size as u64)
            .map_err(|e| EdlError::Session(format!("GPT parse failed: {e}")))?;

        let part = gpt
            .iter()
            .find(|(_, p)| p.partition_name.as_str() == part_name)
            .ok_or_else(|| EdlError::PartitionNotFound(part_name.to_string()))?
            .1;
        Ok((part.starting_lba, part.ending_lba))
    }

    /// Dump a partition (GPT-by-name) to a file.
    pub fn dump_partition(
        &mut self,
        part_name: &str,
        output: &Path,
        slot: u8,
        lun: u8,
        log: &mut Vec<String>,
    ) -> Result<()> {
        ltbox_core::live!(
            log,
            "[EDL] {} '{part_name}' on LUN {lun}...",
            tr("log_edl_lookup_partition")
        );
        let (start, end) = self.find_partition(part_name, slot, lun)?;
        // Reject degenerate GPT entries up-front: end<start wraps under
        // u64, end==start is a zero-sector partition (valid but nothing to
        // dump), and a sector count past usize::MAX cannot be allocated.
        // Firehose's `start_sector` protocol field is u32 so also refuse
        // >u32::MAX starting LBAs rather than silently truncating.
        let span = end
            .checked_sub(start)
            .and_then(|d| d.checked_add(1))
            .ok_or_else(|| {
                EdlError::Session(format!(
                    "Partition {part_name} GPT range invalid: start={start} end={end}"
                ))
            })?;
        let sectors = usize::try_from(span).map_err(|_| {
            EdlError::Session(format!("Partition {part_name} span {span} exceeds usize"))
        })?;
        let start_u32 = u32::try_from(start).map_err(|_| {
            EdlError::Session(format!(
                "Partition {part_name} start LBA {start} exceeds Firehose u32 limit"
            ))
        })?;
        ltbox_core::live!(
            log,
            "[EDL] {} {part_name}: LBA {start}-{end} ({sectors} sectors)",
            tr("log_edl_found_partition")
        );

        let mut out_file = std::fs::File::create(output)?;
        ltbox_core::live!(
            log,
            "[EDL] {} {part_name} → {}",
            tr("log_edl_dump_cmd"),
            output.display()
        );
        qdl::firehose_read_storage(&mut self.dev, &mut out_file, sectors, slot, lun, start_u32)
            .map_err(|e| EdlError::Session(format!("Partition read failed: {e}")))?;
        ltbox_core::live!(log, "[EDL] {} {part_name}", tr("log_edl_dumped"));
        Ok(())
    }

    /// Dump with pre-resolved (LUN, start, length); skips GPT lookup.
    /// Needed because some Lenovo devices put boot/init_boot on non-zero
    /// LUNs that the LUN-0 GPT can't describe. Mirrors v2
    /// `EdlPartitionService.dump_partition`.
    pub fn dump_partition_at(
        &mut self,
        part_name: &str,
        output: &Path,
        lun: u8,
        start_sector: u32,
        num_sectors: usize,
        log: &mut Vec<String>,
    ) -> Result<()> {
        let mut out_file = std::fs::File::create(output)?;
        ltbox_core::live!(
            log,
            "[EDL] {} {part_name} → {} (LUN {lun}, start {start_sector}, {num_sectors} sectors)",
            tr("log_edl_dump_cmd"),
            output.display()
        );
        qdl::firehose_read_storage(
            &mut self.dev,
            &mut out_file,
            num_sectors,
            0,
            lun,
            start_sector,
        )
        .map_err(|e| EdlError::Session(format!("Partition read failed: {e}")))?;
        ltbox_core::live!(log, "[EDL] {} {part_name}", tr("log_edl_dumped"));
        Ok(())
    }

    /// Flash with pre-resolved (LUN, start). Counterpart to
    /// [`dump_partition_at`]; skips the GPT lookup used by [`flash_partition`].
    pub fn flash_partition_at(
        &mut self,
        part_name: &str,
        image: &Path,
        lun: u8,
        start_sector: &str,
        partition_sectors: u64,
        log: &mut Vec<String>,
    ) -> Result<()> {
        let mut file = std::fs::File::open(image)?;
        let file_len = file.metadata()?.len();
        let sector_size = self.dev.fh_config().storage_sector_size as u64;
        let image_sectors = file_len.div_ceil(sector_size);
        // Refuse to program past the partition — an oversized image would
        // spill into the next partition and brick the device. The by-name
        // `flash_partition` resolves the span from the GPT; this explicit-
        // start variant relies on the caller's prior partition scan, passed
        // in as `partition_sectors`.
        if image_sectors > partition_sectors {
            return Err(EdlError::Session(format!(
                "Flash {part_name}: image is {image_sectors} sectors but the partition spans only {partition_sectors}"
            )));
        }
        let num_sectors = image_sectors as usize;
        ltbox_core::live!(
            log,
            "[EDL] {} {part_name} ← {} ({file_len} bytes, {num_sectors} sectors, LUN {lun})",
            tr("log_edl_flash_cmd"),
            image.display()
        );
        qdl::firehose_program_storage(
            &mut self.dev,
            &mut file,
            part_name,
            num_sectors,
            0,
            lun,
            start_sector,
        )
        .map_err(|e| EdlError::Session(format!("Partition write failed: {e}")))?;
        ltbox_core::live!(log, "[EDL] {} {part_name}", tr("log_edl_flashed"));
        Ok(())
    }

    /// Total sector count of a physical LUN. Probes the primary GPT
    /// header (sector 1) and returns `backup_lba + 1`, since
    /// `backup_lba` points at the backup header (last sector of the
    /// disk). Used by the Physical Storage Dump wizard to size a
    /// whole-LUN read without an explicit disk-size query.
    pub fn physical_lun_sector_count(&mut self, lun: u8, log: &mut Vec<String>) -> Result<u64> {
        let mut buf = Cursor::new(Vec::<u8>::new());
        qdl::firehose_read_storage(&mut self.dev, &mut buf, 1, 0, lun, 1)
            .map_err(|e| EdlError::Session(format!("GPT probe failed: {e}")))?;
        buf.rewind()?;
        let header = gptman::GPTHeader::read_from(&mut buf)
            .map_err(|e| EdlError::Session(format!("GPT header parse failed: {e}")))?;
        let total = header.backup_lba.checked_add(1).ok_or_else(|| {
            EdlError::Session(format!(
                "GPT backup_lba {} overflows when computing LUN sector count",
                header.backup_lba
            ))
        })?;
        ltbox_core::live!(
            log,
            "[EDL] {}",
            tr("log_edl_lun_total_sectors")
                .replace("{lun}", &lun.to_string())
                .replace("{total}", &total.to_string())
        );
        Ok(total)
    }

    /// Whole-LUN dump to a file. Reads every sector from LUN `lun`
    /// (count derived from the GPT header's `alternate_lba`) straight
    /// into `output`. Mirrors qdlrs `Dump` but without GPT decoding —
    /// the caller gets a raw physical image.
    pub fn dump_physical_storage(
        &mut self,
        lun: u8,
        output: &Path,
        log: &mut Vec<String>,
    ) -> Result<()> {
        let total = self.physical_lun_sector_count(lun, log)?;
        let mut out_file = std::fs::File::create(output)?;
        ltbox_core::live!(
            log,
            "[EDL] {}",
            tr("log_edl_dump_lun_cmd").replace(
                "{lun}",
                &lun.to_string()
                    .replace("{path}", &output.display().to_string())
                    .replace("{total}", &total.to_string())
            )
        );
        qdl::firehose_read_storage(&mut self.dev, &mut out_file, total as usize, 0, lun, 0)
            .map_err(|e| EdlError::Session(format!("Physical LUN read failed: {e}")))?;
        ltbox_core::live!(
            log,
            "[EDL] {}",
            tr("log_edl_dumped_lun").replace("{lun}", &lun.to_string())
        );
        Ok(())
    }

    /// Whole-LUN raw flash. Mirrors qdlrs `OverwriteStorage`: empty
    /// partition name + start sector "0", file sector count derived
    /// from the image length. No GPT lookup, no bounds check — the
    /// image is written verbatim to sector 0 of `lun`.
    pub fn flash_physical_storage(
        &mut self,
        lun: u8,
        image: &Path,
        log: &mut Vec<String>,
    ) -> Result<()> {
        let mut file = std::fs::File::open(image)?;
        let file_len = file.metadata()?.len();
        let sector_size = self.dev.fh_config().storage_sector_size as u64;
        let num_sectors = file_len.div_ceil(sector_size) as usize;
        ltbox_core::live!(
            log,
            "[EDL] {}",
            tr("log_edl_flash_lun_cmd").replace(
                "{lun}",
                &lun.to_string()
                    .replace("{path}", &image.display().to_string())
                    .replace("{bytes}", &file_len.to_string())
                    .replace("{sectors}", &num_sectors.to_string())
            )
        );
        qdl::firehose_program_storage(&mut self.dev, &mut file, "", num_sectors, 0, lun, "0")
            .map_err(|e| EdlError::Session(format!("Physical LUN write failed: {e}")))?;
        ltbox_core::live!(
            log,
            "[EDL] {}",
            tr("log_edl_flashed_lun").replace("{lun}", &lun.to_string())
        );
        Ok(())
    }

    /// Erase a sector range. Used by the Flash Partitions wizard when
    /// the user flags a row as "erase". `start_sector` is passed through
    /// Firehose as a string so negative offsets (e.g. "-1") stay valid.
    pub fn erase_partition_at(
        &mut self,
        part_name: &str,
        lun: u8,
        start_sector: &str,
        num_sectors: usize,
        log: &mut Vec<String>,
    ) -> Result<()> {
        ltbox_core::live!(
            log,
            "[EDL] {}",
            tr("log_edl_erase_part_cmd")
                .replace("{part}", part_name)
                .replace(
                    "{lun}",
                    &lun.to_string()
                        .replace("{start}", start_sector)
                        .replace("{sectors}", &num_sectors.to_string())
                )
        );
        send_firehose_erase(&mut self.dev, num_sectors, lun, start_sector)
            .map_err(|e| EdlError::Session(format!("Erase {part_name} failed: {e}")))?;
        ltbox_core::live!(
            log,
            "[EDL] {}",
            tr("log_edl_erased_part").replace("{part}", part_name)
        );
        Ok(())
    }

    /// Sectors spanned by a GPT partition (`end` is the inclusive last LBA).
    /// Errors on an inverted range so brick-critical erase/flash refuse bad
    /// geometry rather than silently touching a wrong, tiny span.
    fn partition_span_sectors(part_name: &str, start: u64, end: u64) -> Result<usize> {
        end.checked_sub(start)
            .map(|delta| delta as usize + 1)
            .ok_or_else(|| {
                EdlError::Session(format!(
                    "{part_name}: invalid GPT range (start {start} > end {end})"
                ))
            })
    }

    /// Erase a whole partition resolved by name from the device GPT. Looks up
    /// the partition's start/end LBA (like [`Self::flash_partition`]) and erases
    /// every sector it spans.
    pub fn erase_partition_by_name(
        &mut self,
        part_name: &str,
        slot: u8,
        lun: u8,
        log: &mut Vec<String>,
    ) -> Result<()> {
        ltbox_core::live!(
            log,
            "[EDL] {} '{part_name}' on LUN {lun}...",
            tr("log_edl_lookup_partition")
        );
        let (start, end) = self.find_partition(part_name, slot, lun)?;
        let num_sectors = Self::partition_span_sectors(part_name, start, end)?;
        self.erase_partition_at(part_name, lun, &start.to_string(), num_sectors, log)
    }

    /// Flash image to a partition (GPT-by-name).
    pub fn flash_partition(
        &mut self,
        part_name: &str,
        image: &Path,
        slot: u8,
        lun: u8,
        log: &mut Vec<String>,
    ) -> Result<()> {
        ltbox_core::live!(
            log,
            "[EDL] {} '{part_name}' on LUN {lun}...",
            tr("log_edl_lookup_partition")
        );
        let (start, end) = self.find_partition(part_name, slot, lun)?;
        let span = Self::partition_span_sectors(part_name, start, end)?;

        let mut file = std::fs::File::open(image)?;
        let file_len = file.metadata()?.len();
        let sector_size = self.dev.fh_config().storage_sector_size as u64;
        let num_sectors = file_len.div_ceil(sector_size) as usize;
        // Refuse to program past the partition — an oversized image would
        // spill into the next partition and brick the device.
        if num_sectors > span {
            return Err(EdlError::Session(format!(
                "Flash {part_name}: image is {num_sectors} sectors but the partition spans only {span}"
            )));
        }
        ltbox_core::live!(
            log,
            "[EDL] {} {part_name} ← {} ({file_len} bytes, {num_sectors} sectors)",
            tr("log_edl_flash_cmd"),
            image.display()
        );

        qdl::firehose_program_storage(
            &mut self.dev,
            &mut file,
            part_name,
            num_sectors,
            slot,
            lun,
            &start.to_string(),
        )
        .map_err(|e| EdlError::Session(format!("Partition write failed: {e}")))?;
        ltbox_core::live!(log, "[EDL] {} {part_name}", tr("log_edl_flashed"));
        Ok(())
    }

    pub fn reset(&mut self, log: &mut Vec<String>) -> Result<()> {
        ltbox_core::live!(log, "[EDL] {}", tr("log_edl_reset_cmd"));
        qdl::firehose_reset(&mut self.dev, &FirehoseResetMode::Reset, 2)
            .map_err(|e| EdlError::Session(format!("Reset failed: {e}")))?;
        ltbox_core::live!(log, "[EDL] {}", tr("log_edl_reset_initiated"));
        Ok(())
    }

    /// Best-effort system reset for end-of-flow cleanup.
    ///
    /// v2 called qdl reset with `check=False`; some devices successfully
    /// reset the USB endpoint while qdl still reports an error. Use this
    /// after all destructive writes have already completed and the only
    /// remaining action is booting the device back to system.
    pub fn reset_tolerant(&mut self, log: &mut Vec<String>) {
        if let Err(e) = self.reset(log) {
            ltbox_core::live!(
                log,
                "[EDL] {}",
                tr("log_edl_reboot_handoff_error").replace("{error}", &e.to_string())
            );
        }
    }

    /// Bounce back to Sahara (does NOT boot system). Required after a
    /// dump-only session so the next `open()` gets a fresh Hello —
    /// otherwise Sahara times out. Mirrors v2 qdl-rs default behavior.
    pub fn reset_to_edl(&mut self, log: &mut Vec<String>) -> Result<()> {
        ltbox_core::live!(log, "[EDL] {}", tr("log_edl_reset_to_edl_cmd"));
        qdl::firehose_reset(&mut self.dev, &FirehoseResetMode::ResetToEdl, 0)
            .map_err(|e| EdlError::Session(format!("reset_to_edl failed: {e}")))?;
        ltbox_core::live!(log, "[EDL] {}", tr("log_edl_reset_to_edl_sent"));
        Ok(())
    }

    /// Mark `xbl_a`'s LUN as the boot drive (Firehose
    /// `<setbootablestoragedrive value="1"/>`, equivalent to fh_loader's
    /// `setactivepartition=1`). LUN 1 is hardcoded — every supported
    /// Lenovo Qualcomm tablet (TB320FC / TB321FU / TB322FC / TB323FU /
    /// TB520FU / TB710FU) places `xbl_a` on LUN 1.
    ///
    /// Lenovo firmware rawprograms only target `_a`, so a full firmware
    /// flash always lands on `_a`. Call this after the flash so the SoC
    /// boots from the freshly-written `_a` on the next reset; without
    /// it a device that was previously running on `_b` would continue
    /// booting `_b`'s pre-flash firmware.
    pub fn set_active_slot_a(&mut self, log: &mut Vec<String>) -> Result<()> {
        const XBL_A_LUN: u8 = 1;
        ltbox_core::live!(
            log,
            "[Flash] {}",
            tr("live_flash_set_bootable").replace("{lun}", &XBL_A_LUN.to_string())
        );
        qdl::firehose_set_bootable(&mut self.dev, XBL_A_LUN)
            .map_err(|e| EdlError::Session(format!("setbootablestoragedrive failed: {e}")))
    }

    /// Flash every `<program>` in the rawprogram XMLs, then apply patch
    /// XMLs. XML coordinates drive the flash, so no slot-suffix guessing
    /// (EDL can't read slot suffix from ADB). Images resolve against the
    /// XML's own directory. Empty filename / `num_sectors=0` entries
    /// skipped (GPT placeholders). Missing images logged and skipped.
    /// Mirrors v2 `flash_rawprogram` in `bin/ltbox/device/edl.py`.
    pub fn flash_rawprogram(
        &mut self,
        program_xmls: &[PathBuf],
        patch_xmls: &[PathBuf],
        log: &mut Vec<String>,
    ) -> Result<()> {
        // Back-compat: default to keep-data.
        self.flash_rawprogram_with_wipe(program_xmls, patch_xmls, false, log)
    }

    /// Erased on wipe=true (matches v2 `_ERASE_LABELS`).
    const WIPE_ERASE_BASES: &'static [&'static str] = &["userdata", "metadata", "frp"];

    /// Skipped on wipe=false. Narrower than `WIPE_ERASE_BASES`: frp is
    /// not user state. Matches v2 `_patch_xml_for_wipe` (wipe=0).
    const KEEP_DATA_SKIP_BASES: &'static [&'static str] = &["userdata", "metadata"];

    /// Match `label` against bases, with or without `_a`/`_b` suffix.
    fn label_matches_base(label: &str, bases: &[&str]) -> bool {
        let l = label.to_ascii_lowercase();
        bases
            .iter()
            .any(|b| l == *b || l.starts_with(&format!("{b}_")))
    }

    fn wipe_labels(label: &str) -> bool {
        Self::label_matches_base(label, Self::WIPE_ERASE_BASES)
    }

    fn keep_data_skip_labels(label: &str) -> bool {
        Self::label_matches_base(label, Self::KEEP_DATA_SKIP_BASES)
    }

    /// Flash with explicit user-data mode.
    ///
    /// `wipe=true` (v2 `pre_erase=True`): erase userdata/metadata/frp
    /// (+ slot variants) before flashing, then flash rawprograms, then
    /// apply patches.
    ///
    /// `wipe=false` (v2 `_patch_xml_for_wipe(wipe=0)`): skip
    /// userdata/metadata entries during the flash pass.
    pub fn flash_rawprogram_with_wipe(
        &mut self,
        program_xmls: &[PathBuf],
        patch_xmls: &[PathBuf],
        wipe: bool,
        log: &mut Vec<String>,
    ) -> Result<()> {
        if wipe {
            ltbox_core::live!(log, "[Flash] {}", tr("log_flash_wipe_enabled"));
            self.pre_erase_wipe_labels(program_xmls, log)?;
        } else {
            ltbox_core::live!(log, "[Flash] {}", tr("log_flash_wipe_disabled"));
        }

        for xml_path in program_xmls {
            ltbox_core::live!(
                log,
                "[EDL] {} {}",
                tr("log_edl_flash_cmd"),
                xml_path.display()
            );
            self.flash_one_rawprogram(xml_path, wipe, log)?;
        }
        for xml_path in patch_xmls {
            // Show file name only — the full disk path was noisy and added
            // nothing the user couldn't already see in the firmware folder.
            let display_name = xml_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| xml_path.display().to_string());
            ltbox_core::live!(
                log,
                "[EDL] {}",
                tr("log_edl_patch_xml_cmd").replace("{path}", &display_name)
            );
            self.apply_patch_xml(xml_path, log)?;
        }
        Ok(())
    }

    /// Flash every `<program>` / `<erase>` node exactly as the rawprogram
    /// XMLs list them, then apply patch XMLs — no keep-data skipping and no
    /// pre-erase pass.
    ///
    /// This mirrors a stock Lenovo flash script as closely as possible: the
    /// data-wipe outcome is decided entirely by which rawprogram the catalog
    /// selected (e.g. a persist-preserving `save_persist` variant vs a
    /// `write_persist` one), not by any LTBox-side keep/wipe policy. Callers
    /// that want the userdata/metadata keep-skip or the userdata/metadata/frp
    /// pre-erase must use [`Self::flash_rawprogram_with_wipe`] instead.
    pub fn flash_rawprogram_verbatim(
        &mut self,
        program_xmls: &[PathBuf],
        patch_xmls: &[PathBuf],
        log: &mut Vec<String>,
    ) -> Result<()> {
        for xml_path in program_xmls {
            ltbox_core::live!(
                log,
                "[EDL] {} {}",
                tr("log_edl_flash_cmd"),
                xml_path.display()
            );
            // `wipe = true` here only means "do not skip userdata/metadata":
            // `flash_one_rawprogram` writes every node verbatim and the
            // separate pre-erase pass is intentionally not run.
            self.flash_one_rawprogram(xml_path, true, log)?;
        }
        for xml_path in patch_xmls {
            let display_name = xml_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| xml_path.display().to_string());
            ltbox_core::live!(
                log,
                "[EDL] {}",
                tr("log_edl_patch_xml_cmd").replace("{path}", &display_name)
            );
            self.apply_patch_xml(xml_path, log)?;
        }
        Ok(())
    }

    /// Erase every `<program>` whose label is in `WIPE_ERASE_BASES`
    /// using XML-reported coordinates. Skips partitions absent from the XMLs.
    fn pre_erase_wipe_labels(
        &mut self,
        program_xmls: &[PathBuf],
        log: &mut Vec<String>,
    ) -> Result<()> {
        for entry in Self::collect_wipe_erase_plan(program_xmls)? {
            ltbox_core::live!(log, "{}", entry.log_line());
            send_firehose_erase(
                &mut self.dev,
                entry.num_sectors,
                entry.lun,
                &entry.start_sector,
            )
            .map_err(|e| EdlError::Session(format!("Erase {} failed: {e}", entry.label)))?;
        }
        Ok(())
    }

    fn collect_wipe_erase_plan(program_xmls: &[PathBuf]) -> Result<Vec<WipeErasePlanEntry>> {
        let mut plan = Vec::new();
        for xml_path in program_xmls {
            let xml_content = std::fs::read_to_string(xml_path)?;
            let doc = roxmltree::Document::parse(&xml_content).map_err(|e| {
                EdlError::Session(format!("XML parse error in {}: {e}", xml_path.display()))
            })?;
            for node in doc.descendants() {
                if !node.tag_name().name().eq_ignore_ascii_case("program") {
                    continue;
                }
                let label = node.attribute("label").unwrap_or("").trim();
                if !Self::wipe_labels(label) {
                    continue;
                }
                let ctx = format!("{} <program label={label}>", xml_path.display());
                let num_sectors: usize =
                    parse_xml_attr(&node, "num_partition_sectors", 0usize, &ctx)?;
                if num_sectors == 0 {
                    continue;
                }
                let lun: u8 = parse_xml_attr(&node, "physical_partition_number", 0u8, &ctx)?;
                let start_sector = node.attribute("start_sector").unwrap_or("0");
                plan.push(WipeErasePlanEntry {
                    label: label.to_string(),
                    lun,
                    start_sector: start_sector.to_string(),
                    num_sectors,
                });
            }
        }
        Ok(plan)
    }

    fn flash_one_rawprogram(
        &mut self,
        xml_path: &Path,
        wipe: bool,
        log: &mut Vec<String>,
    ) -> Result<()> {
        let xml_content = std::fs::read_to_string(xml_path)?;
        let doc = roxmltree::Document::parse(&xml_content).map_err(|e| {
            EdlError::Session(format!("XML parse error in {}: {e}", xml_path.display()))
        })?;
        let xml_dir = xml_path.parent().unwrap_or(Path::new("."));

        for node in doc.descendants() {
            match node.tag_name().name().to_lowercase().as_str() {
                "program" => {
                    // Keep-data: skip userdata/metadata entries.
                    if !wipe {
                        let label = node.attribute("label").unwrap_or("").trim();
                        if Self::keep_data_skip_labels(label) {
                            ltbox_core::live!(
                                log,
                                "[EDL] {}",
                                tr("log_edl_skip_keep_data").replace("{label}", label)
                            );
                            continue;
                        }
                    }
                    self.flash_program_node(&node, xml_dir, log)?;
                }
                "erase" => self.erase_program_node(&node, log)?,
                _ => continue,
            }
        }
        Ok(())
    }

    fn flash_program_node(
        &mut self,
        node: &roxmltree::Node<'_, '_>,
        xml_dir: &Path,
        log: &mut Vec<String>,
    ) -> Result<()> {
        let label = node.attribute("label").unwrap_or("").trim().to_string();
        let filename = node.attribute("filename").unwrap_or("").trim().to_string();
        let ctx = format!("<program label={label}>");
        let num_sectors: usize = parse_xml_attr(node, "num_partition_sectors", 0usize, &ctx)?;

        // Skip GPT placeholders / empty entries (qdl CLI `parse_program_cmd`).
        if filename.is_empty() || num_sectors == 0 {
            return Ok(());
        }

        let lun: u8 = parse_xml_attr(node, "physical_partition_number", 0u8, &ctx)?;
        let slot: u8 = parse_xml_attr(node, "slot", 0u8, &ctx)?;
        let start_sector = node.attribute("start_sector").unwrap_or("0");
        let file_sector_offset: u64 = parse_xml_attr(node, "file_sector_offset", 0u64, &ctx)?;
        let sector_size: u64 = self.dev.fh_config().storage_sector_size as u64;

        let image_path = ltbox_core::safe_path::safe_join(xml_dir, &filename)
            .map_err(|e| EdlError::Session(e.to_string()))?;
        if !image_path.exists() {
            ltbox_core::live!(
                log,
                "[EDL] {}",
                tr("log_edl_skip_image_missing")
                    .replace("{label}", &label)
                    .replace("{path}", &image_path.display().to_string())
            );
            return Ok(());
        }

        let mut file = std::fs::File::open(&image_path)?;
        if file_sector_offset > 0 {
            // `file_sector_offset * sector_size` can overflow u64 if the
            // rawprogram XML carries a hostile or corrupted value (untrusted
            // input — the same XML that names the partition). On overflow the
            // wrap-around lands at a tiny offset and we'd flash bytes from the
            // wrong region of `image_path` to the device. Reject overflow and
            // require the resulting byte offset to fit inside the image file
            // so we never seek past EOF and feed Firehose stale read data.
            let byte_offset = sector_size.checked_mul(file_sector_offset).ok_or_else(|| {
                EdlError::Session(format!(
                    "{ctx}: file_sector_offset {file_sector_offset} \
                         × sector_size {sector_size} overflows u64"
                ))
            })?;
            let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
            if byte_offset >= file_len {
                return Err(EdlError::Session(format!(
                    "{ctx}: file_sector_offset {file_sector_offset} \
                     (byte offset {byte_offset}) >= image length {file_len} \
                     for {}",
                    image_path.display(),
                )));
            }
            file.seek(SeekFrom::Start(byte_offset))?;
        }

        ltbox_core::live!(
            log,
            "[EDL] {}",
            tr("log_edl_flash_program_cmd")
                .replace("{label}", &label)
                .replace(
                    "{image}",
                    image_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(""),
                )
                .replace("{lun}", &lun.to_string())
                .replace("{start}", start_sector)
                .replace("{sectors}", &num_sectors.to_string())
        );

        qdl::firehose_program_storage(
            &mut self.dev,
            &mut file,
            &label,
            num_sectors,
            slot,
            lun,
            start_sector,
        )
        .map_err(|e| EdlError::Session(format!("Program {label} failed: {e}")))?;
        Ok(())
    }

    fn erase_program_node(
        &mut self,
        node: &roxmltree::Node<'_, '_>,
        log: &mut Vec<String>,
    ) -> Result<()> {
        let ctx = "<erase>";
        let num_sectors: usize = parse_xml_attr(node, "num_partition_sectors", 0usize, ctx)?;
        if num_sectors == 0 {
            return Ok(());
        }
        let lun: u8 = parse_xml_attr(node, "physical_partition_number", 0u8, ctx)?;
        let start_sector = node.attribute("start_sector").unwrap_or("0");

        ltbox_core::live!(
            log,
            "[EDL] {}",
            tr("log_edl_erase_lun_cmd").replace(
                "{lun}",
                &lun.to_string()
                    .replace("{start}", start_sector)
                    .replace("{sectors}", &num_sectors.to_string())
            )
        );
        send_firehose_erase(&mut self.dev, num_sectors, lun, start_sector)
            .map_err(|e| EdlError::Session(format!("Erase failed: {e}")))?;
        Ok(())
    }

    fn apply_patch_xml(&mut self, xml_path: &Path, log: &mut Vec<String>) -> Result<()> {
        let xml_content = std::fs::read_to_string(xml_path)?;
        let doc = roxmltree::Document::parse(&xml_content).map_err(|e| {
            EdlError::Session(format!("XML parse error in {}: {e}", xml_path.display()))
        })?;

        for node in doc.descendants() {
            if !node.tag_name().name().eq_ignore_ascii_case("patch") {
                continue;
            }
            // Non-DISK patches target files, not storage.
            let filename = node.attribute("filename").unwrap_or("");
            if filename != "DISK" {
                continue;
            }
            let ctx = "<patch>";
            let byte_off: u64 = parse_xml_attr(&node, "byte_offset", 0u64, ctx)?;
            let lun: u8 = parse_xml_attr(&node, "physical_partition_number", 0u8, ctx)?;
            let slot: u8 = parse_xml_attr(&node, "slot", 0u8, ctx)?;
            let size: u64 = parse_xml_attr(&node, "size_in_bytes", 0u64, ctx)?;
            let start_sector = node.attribute("start_sector").unwrap_or("0");
            let value = node.attribute("value").unwrap_or("");

            ltbox_core::live!(
                log,
                "[EDL] {}",
                tr("log_edl_patch_lun_cmd")
                    .replace("{lun}", &lun.to_string())
                    .replace("{start}", start_sector)
                    .replace("{offset}", &byte_off.to_string())
                    .replace("{bytes}", &size.to_string())
                    .replace("{value}", value)
            );
            qdl::firehose_patch(
                &mut self.dev,
                byte_off,
                slot,
                lun,
                size,
                start_sector,
                value,
            )
            .map_err(|e| EdlError::Session(format!("Patch failed: {e}")))?;
        }
        Ok(())
    }
}

/// Parse an XML attribute, distinguishing three cases:
///   - attribute absent → returns `default`
///   - attribute present and parseable → returns the parsed value
///   - attribute present but malformed → returns
///     `EdlError::Session` with context
///
/// Silently defaulting a malformed value (e.g. `num_partition_sectors="bogus"`
/// → 0) lets a corrupt rawprogram XML steer a flash at sector 0 or skip a
/// real partition entirely. Values that are legitimately optional (e.g.
/// `slot`, `file_sector_offset`) still default cleanly when absent.
fn parse_xml_attr<T>(
    node: &roxmltree::Node<'_, '_>,
    attr: &str,
    default: T,
    context: &str,
) -> Result<T>
where
    T: std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    match node.attribute(attr) {
        None => Ok(default),
        Some(raw) => raw
            .trim()
            .parse::<T>()
            .map_err(|e| EdlError::Session(format!("{context}: invalid {attr}='{raw}': {e}"))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawprogramFamily {
    Other,
    Persist,
    Devinfo,
}

fn rawprogram_family(name_lower: &str) -> RawprogramFamily {
    match name_lower {
        "rawprogram_unsparse0.xml"
        | "rawprogram_unsparse0-half.xml"
        | "rawprogram_write_persist_unsparse0.xml"
        | "rawprogram_save_persist_unsparse0.xml"
        | "rawprogram_save_persist_ota_unsparse0.xml" => RawprogramFamily::Persist,
        "rawprogram4.xml" | "rawprogram4_write_devinfo.xml" | "rawprogram_unsparse4.xml" => {
            RawprogramFamily::Devinfo
        }
        _ => RawprogramFamily::Other,
    }
}

fn filename_rank(path: &Path, preferred: &[&str]) -> usize {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    preferred
        .iter()
        .position(|candidate| name == *candidate)
        .unwrap_or(preferred.len())
}

fn select_one_by_name_priority(mut paths: Vec<PathBuf>, preferred: &[&str]) -> Option<PathBuf> {
    paths.sort_by(|a, b| {
        filename_rank(a, preferred)
            .cmp(&filename_rank(b, preferred))
            .then_with(|| a.file_name().cmp(&b.file_name()))
    });
    paths.into_iter().next()
}

fn select_persist_xml(paths: Vec<PathBuf>, allow_dp_filenames: bool) -> Option<PathBuf> {
    if allow_dp_filenames {
        select_one_by_name_priority(
            paths,
            &[
                "rawprogram_write_persist_unsparse0.xml",
                "rawprogram_unsparse0.xml",
                "rawprogram_save_persist_ota_unsparse0.xml",
                "rawprogram_save_persist_unsparse0.xml",
                "rawprogram_unsparse0-half.xml",
            ],
        )
    } else {
        select_one_by_name_priority(
            paths,
            &[
                "rawprogram_save_persist_ota_unsparse0.xml",
                "rawprogram_save_persist_unsparse0.xml",
                "rawprogram_unsparse0-half.xml",
                "rawprogram_unsparse0.xml",
                "rawprogram_write_persist_unsparse0.xml",
            ],
        )
    }
}

fn select_devinfo_xml(paths: Vec<PathBuf>, allow_dp_filenames: bool) -> Option<PathBuf> {
    if allow_dp_filenames {
        select_one_by_name_priority(
            paths,
            &[
                "rawprogram4_write_devinfo.xml",
                "rawprogram4.xml",
                "rawprogram_unsparse4.xml",
            ],
        )
    } else {
        select_one_by_name_priority(
            paths,
            &[
                "rawprogram4.xml",
                "rawprogram_unsparse4.xml",
                "rawprogram4_write_devinfo.xml",
            ],
        )
    }
}

fn validate_dp_filename_usage(raw_xmls: &[PathBuf], allow_dp_filenames: bool) -> Result<()> {
    for xml_path in raw_xmls {
        let xml_content = std::fs::read_to_string(xml_path)?;
        let doc = roxmltree::Document::parse(&xml_content).map_err(|e| {
            EdlError::Session(format!("XML parse error in {}: {e}", xml_path.display()))
        })?;
        let xml_dir = xml_path.parent().unwrap_or(Path::new("."));
        for node in doc.descendants() {
            if !node.tag_name().name().eq_ignore_ascii_case("program") {
                continue;
            }
            let filename = node.attribute("filename").unwrap_or("").trim();
            let lower = filename.to_ascii_lowercase();
            if lower != "persist.img" && lower != "devinfo.img" {
                continue;
            }
            if !allow_dp_filenames {
                return Err(EdlError::Session(format!(
                    "{} references {filename}, but devinfo/persist image flashing is disabled",
                    xml_path.display()
                )));
            }
            let image_path = xml_dir.join(filename);
            if !image_path.exists() {
                return Err(EdlError::Session(format!(
                    "{} references {filename}, but {} is missing",
                    xml_path.display(),
                    image_path.display()
                )));
            }
        }
    }
    Ok(())
}

/// Collect `rawprogram*.xml` and `patch*.xml` from `dir` for firmware
/// flashing. Drops v2 filter targets (WIPE/BLANK variants,
/// `rawprogram0.xml` GPT programmer) and treats devinfo/persist XML
/// variants as mutually exclusive families.
///
/// `allow_dp_filenames=false` is the normal v3 firmware path: country-code
/// images are dumped, patched, and flashed explicitly after rawprogram
/// flashing, so a selected rawprogram must not reference `persist.img` or
/// `devinfo.img`.
pub fn collect_firmware_xmls_for_flash(
    dir: &Path,
    allow_dp_filenames: bool,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut raw_xmls = Vec::new();
    let mut patch_xmls = Vec::new();
    let mut persist_xmls = Vec::new();
    let mut devinfo_xmls = Vec::new();
    let entries = std::fs::read_dir(dir)?;

    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let lower = name.to_lowercase();
        if !lower.ends_with(".xml") {
            continue;
        }
        if lower.starts_with("rawprogram") {
            // v2 skips WIPE / zero-GPT variants.
            if name.contains("WIPE_PARTITIONS") || name.contains("BLANK_GPT") {
                continue;
            }
            if name == "rawprogram0.xml" {
                continue;
            }
            match rawprogram_family(&lower) {
                RawprogramFamily::Other => raw_xmls.push(path),
                RawprogramFamily::Persist => persist_xmls.push(path),
                RawprogramFamily::Devinfo => devinfo_xmls.push(path),
            }
        } else if lower.starts_with("patch") {
            patch_xmls.push(path);
        }
    }

    if let Some(path) = select_persist_xml(persist_xmls, allow_dp_filenames) {
        raw_xmls.push(path);
    }
    if let Some(path) = select_devinfo_xml(devinfo_xmls, allow_dp_filenames) {
        raw_xmls.push(path);
    }

    raw_xmls.sort();
    patch_xmls.sort();
    validate_dp_filename_usage(&raw_xmls, allow_dp_filenames)?;
    Ok((raw_xmls, patch_xmls))
}

/// Back-compatible XML collection for read-only catalog lookups. Firmware
/// flashing should call [`collect_firmware_xmls_for_flash`] so unsafe
/// devinfo/persist references surface as errors.
pub fn collect_firmware_xmls(dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    collect_firmware_xmls_for_flash(dir, false).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn partition_span_sectors_counts_inclusive_and_rejects_inverted() {
        // end is the inclusive last LBA, so span = end - start + 1.
        assert_eq!(
            EdlSession::partition_span_sectors("p", 100, 199).unwrap(),
            100
        );
        assert_eq!(EdlSession::partition_span_sectors("p", 5, 5).unwrap(), 1);
        // Inverted range must error, never produce a tiny bogus span.
        assert!(EdlSession::partition_span_sectors("p", 200, 100).is_err());
    }

    #[test]
    fn gpt_read_sectors_bounds_first_usable_lba() {
        // Plausible GPT metadata spans pass through unchanged.
        assert_eq!(EdlSession::gpt_read_sectors(6).unwrap(), 6);
        assert_eq!(EdlSession::gpt_read_sectors(34).unwrap(), 34);
        assert_eq!(
            EdlSession::gpt_read_sectors(EdlSession::MAX_GPT_METADATA_SECTORS).unwrap(),
            EdlSession::MAX_GPT_METADATA_SECTORS as usize
        );
        // Zero and implausibly large values (a malformed or hostile GPT) are
        // rejected before they can drive an unbounded Firehose read.
        assert!(EdlSession::gpt_read_sectors(0).is_err());
        assert!(EdlSession::gpt_read_sectors(EdlSession::MAX_GPT_METADATA_SECTORS + 1).is_err());
        assert!(EdlSession::gpt_read_sectors(u64::MAX).is_err());
    }

    struct TempXml(PathBuf);

    impl TempXml {
        fn new(contents: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "ltbox-edl-wipe-plan-{}-{nonce}.xml",
                std::process::id()
            ));
            std::fs::write(&path, contents).expect("write temp rawprogram");
            Self(path)
        }

        fn path(&self) -> PathBuf {
            self.0.clone()
        }
    }

    struct TempFirmwareDir(PathBuf);

    impl TempFirmwareDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("ltbox-edl-fw-{}-{nonce}", std::process::id()));
            std::fs::create_dir_all(&path).expect("create temp firmware dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, name: &str, contents: &str) {
            std::fs::write(self.0.join(name), contents).expect("write temp firmware file");
        }

        fn write_bytes(&self, name: &str, contents: &[u8]) {
            std::fs::write(self.0.join(name), contents).expect("write temp firmware image");
        }
    }

    impl Drop for TempFirmwareDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn xml_names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
            .collect()
    }

    impl Drop for TempXml {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn run_with_deadline_never(ports: Vec<Result<String>>) -> Result<String> {
        let mut queue: VecDeque<Result<String>> = ports.into();
        wait_for_stable_port_with(
            || queue.pop_front().unwrap_or(Err(EdlError::PortNotFound)),
            |_| {},
            || false,
            EDL_SESSION_OPEN_TIMEOUT,
        )
    }

    #[test]
    fn stale_port_disconnects_then_new_port_stabilizes() {
        // Phase 1: Ok(COM6) → disconnect. Phase 2: COM7 twice → return.
        let port = run_with_deadline_never(vec![
            Ok("COM6".to_string()),
            Err(EdlError::PortNotFound),
            Ok("COM7".to_string()),
            Ok("COM7".to_string()),
        ])
        .expect("stable port");
        assert_eq!(port, "COM7");
    }

    #[test]
    fn visible_port_without_disconnect_is_trusted() {
        // Port visible for full 5-poll observation window.
        let port = run_with_deadline_never(vec![
            Ok("COM6".to_string()),
            Ok("COM6".to_string()),
            Ok("COM6".to_string()),
            Ok("COM6".to_string()),
            Ok("COM6".to_string()),
            Ok("COM6".to_string()),
        ])
        .expect("stable port");
        assert_eq!(port, "COM6");
    }

    #[test]
    fn phase1_name_change_forces_phase2() {
        // Regression: Phase 1 that only checks Ok/Err presence would latch
        // the dead COM6 handle across a reset-to-EDL renumeration.
        let port = run_with_deadline_never(vec![
            Ok("COM6".to_string()),
            Ok("COM7".to_string()), // phase 1 name change → fall through
            Ok("COM7".to_string()),
            Ok("COM7".to_string()),
        ])
        .expect("stable port");
        assert_eq!(port, "COM7");
    }

    #[test]
    fn timeout_returns_port_timeout_error() {
        let mut polls_remaining: u32 = 3;
        let err = wait_for_stable_port_with(
            || Err(EdlError::PortNotFound),
            |_| {},
            move || {
                if polls_remaining == 0 {
                    true
                } else {
                    polls_remaining -= 1;
                    false
                }
            },
            EDL_SESSION_OPEN_TIMEOUT,
        )
        .expect_err("should time out");
        assert!(matches!(err, EdlError::PortTimeout(_)));
    }

    #[test]
    fn wipe_erase_plan_keeps_xml_order_and_log_geometry() {
        let rawprogram = TempXml::new(
            r#"
            <data>
              <program label="metadata" physical_partition_number="0" start_sector="8192" num_partition_sectors="2048" />
              <program label="super" physical_partition_number="0" start_sector="16384" num_partition_sectors="4096" />
              <program label="frp" physical_partition_number="1" start_sector="24576" num_partition_sectors="128" />
              <program label="userdata_a" physical_partition_number="2" start_sector="32768" num_partition_sectors="0" />
              <program label="userdata_b" physical_partition_number="3" start_sector="65536" num_partition_sectors="8192" />
            </data>
            "#,
        );

        let plan = EdlSession::collect_wipe_erase_plan(&[rawprogram.path()])
            .expect("collect wipe erase plan");

        assert_eq!(
            plan.iter()
                .map(|entry| entry.label.as_str())
                .collect::<Vec<_>>(),
            ["metadata", "frp", "userdata_b"]
        );
        let template = "erase {label} (LUN {lun}, start {start}, {sectors} sectors)";
        assert_eq!(
            plan[0].log_line_with_template(template),
            "[EDL] erase metadata (LUN 0, start 8192, 2048 sectors)"
        );
        assert_eq!(
            plan[1].log_line_with_template(template),
            "[EDL] erase frp (LUN 1, start 24576, 128 sectors)"
        );
        assert_eq!(
            plan[2].log_line_with_template(template),
            "[EDL] erase userdata_b (LUN 3, start 65536, 8192 sectors)"
        );
    }

    #[test]
    fn collect_firmware_xmls_chooses_safe_dp_variants_once() {
        let fw = TempFirmwareDir::new();
        fw.write("rawprogram1.xml", "<data/>");
        fw.write(
            "rawprogram_unsparse0.xml",
            r#"<data><program label="persist" filename="persist.img" num_partition_sectors="1"/></data>"#,
        );
        fw.write(
            "rawprogram_unsparse0-half.xml",
            r#"<data><program label="persist" filename="" num_partition_sectors="1"/></data>"#,
        );
        fw.write(
            "rawprogram4.xml",
            r#"<data><program label="devinfo" filename="" num_partition_sectors="1"/></data>"#,
        );
        fw.write(
            "rawprogram4_write_devinfo.xml",
            r#"<data><program label="devinfo" filename="devinfo.img" num_partition_sectors="1"/></data>"#,
        );
        fw.write("patch0.xml", "<data/>");

        let (raw, patch) =
            collect_firmware_xmls_for_flash(fw.path(), false).expect("collect safe XMLs");
        let names = xml_names(&raw);

        assert!(names.contains(&"rawprogram1.xml".to_string()));
        assert!(names.contains(&"rawprogram_unsparse0-half.xml".to_string()));
        assert!(names.contains(&"rawprogram4.xml".to_string()));
        assert!(!names.contains(&"rawprogram_unsparse0.xml".to_string()));
        assert!(!names.contains(&"rawprogram4_write_devinfo.xml".to_string()));
        assert_eq!(
            names.len(),
            names.iter().collect::<std::collections::HashSet<_>>().len()
        );
        assert_eq!(xml_names(&patch), vec!["patch0.xml".to_string()]);
    }

    #[test]
    fn collect_firmware_xmls_prefers_persistless_lun0_for_simple_flash() {
        // Simple Flash reuses `collect_firmware_xmls_for_flash(dir, false)`:
        // when a firmware ships both a persist-less LUN0 rawprogram
        // (save_persist, empty persist filename) and a persist-writing one
        // (write_persist), the persist-less variant must win and be the *only*
        // LUN0 rawprogram kept.
        let fw = TempFirmwareDir::new();
        fw.write(
            "rawprogram_save_persist_unsparse0.xml",
            r#"<data><program label="persist" filename="" num_partition_sectors="1"/></data>"#,
        );
        fw.write(
            "rawprogram_write_persist_unsparse0.xml",
            r#"<data><program label="persist" filename="persist.img" num_partition_sectors="1"/></data>"#,
        );

        let (raw, _) =
            collect_firmware_xmls_for_flash(fw.path(), false).expect("collect persist-less LUN0");
        let names = xml_names(&raw);

        assert!(names.contains(&"rawprogram_save_persist_unsparse0.xml".to_string()));
        assert!(!names.contains(&"rawprogram_write_persist_unsparse0.xml".to_string()));
        // Exactly one LUN0 rawprogram is kept.
        let lun0 = names
            .iter()
            .filter(|n| n.contains("persist_unsparse0"))
            .count();
        assert_eq!(lun0, 1);
    }

    #[test]
    fn collect_firmware_xmls_allows_dp_xmls_when_images_exist() {
        let fw = TempFirmwareDir::new();
        fw.write("rawprogram1.xml", "<data/>");
        fw.write(
            "rawprogram_save_persist_unsparse0.xml",
            r#"<data><program label="persist" filename="" num_partition_sectors="1"/></data>"#,
        );
        fw.write(
            "rawprogram_write_persist_unsparse0.xml",
            r#"<data><program label="persist" filename="persist.img" num_partition_sectors="1"/></data>"#,
        );
        fw.write(
            "rawprogram4.xml",
            r#"<data><program label="devinfo" filename="" num_partition_sectors="1"/></data>"#,
        );
        fw.write(
            "rawprogram4_write_devinfo.xml",
            r#"<data><program label="devinfo" filename="devinfo.img" num_partition_sectors="1"/></data>"#,
        );
        fw.write_bytes("persist.img", b"persist");
        fw.write_bytes("devinfo.img", b"devinfo");

        let (raw, _) = collect_firmware_xmls_for_flash(fw.path(), true).expect("collect DP XMLs");
        let names = xml_names(&raw);

        assert!(names.contains(&"rawprogram_write_persist_unsparse0.xml".to_string()));
        assert!(names.contains(&"rawprogram4_write_devinfo.xml".to_string()));
        assert!(!names.contains(&"rawprogram_save_persist_unsparse0.xml".to_string()));
        assert!(!names.contains(&"rawprogram4.xml".to_string()));
        assert_eq!(
            names.len(),
            names.iter().collect::<std::collections::HashSet<_>>().len()
        );
    }

    #[test]
    fn collect_firmware_xmls_rejects_disabled_dp_references() {
        let fw = TempFirmwareDir::new();
        fw.write(
            "rawprogram_unsparse0.xml",
            r#"<data><program label="persist" filename="persist.img" num_partition_sectors="1"/></data>"#,
        );

        let err = collect_firmware_xmls_for_flash(fw.path(), false)
            .expect_err("persist.img should be rejected");
        assert!(err.to_string().contains("persist.img"));
        assert!(err.to_string().contains("disabled"));
    }

    #[test]
    fn collect_firmware_xmls_rejects_allowed_dp_when_image_missing() {
        let fw = TempFirmwareDir::new();
        fw.write(
            "rawprogram_write_persist_unsparse0.xml",
            r#"<data><program label="persist" filename="persist.img" num_partition_sectors="1"/></data>"#,
        );

        let err = collect_firmware_xmls_for_flash(fw.path(), true)
            .expect_err("persist.img image should be required");
        assert!(err.to_string().contains("persist.img"));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn real_firmware_xml_matrix_when_available() {
        let Some(dir) = std::env::var_os("LTBOX_REAL_FIRMWARE_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        if !dir.join("rawprogram_unsparse0-half.xml").exists() {
            return;
        }

        let (raw, patch) =
            collect_firmware_xmls_for_flash(&dir, false).expect("collect real safe XMLs");
        let names = xml_names(&raw);

        assert!(names.contains(&"rawprogram_unsparse0-half.xml".to_string()));
        assert!(!names.contains(&"rawprogram_unsparse0.xml".to_string()));
        assert!(names.contains(&"rawprogram4.xml".to_string()));
        assert_eq!(
            names.len(),
            names.iter().collect::<std::collections::HashSet<_>>().len()
        );
        assert!(!patch.is_empty());

        let wipe_plan = EdlSession::collect_wipe_erase_plan(&raw).expect("collect wipe plan");
        assert_eq!(
            wipe_plan
                .iter()
                .filter(|entry| entry.label == "metadata")
                .count(),
            7
        );
        assert_eq!(
            wipe_plan
                .iter()
                .filter(|entry| entry.label == "userdata")
                .count(),
            10
        );
        assert!(wipe_plan.iter().any(|entry| entry.label == "frp"));
    }
}
