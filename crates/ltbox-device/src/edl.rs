//! EDL (Emergency Download) — Qualcomm 9008 serial port detection and
//! session management (Sahara → Firehose configure → operations).

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

/// Scan for the Qualcomm 9008 serial port.
pub fn find_edl_port() -> Result<String> {
    let ports = serialport::available_ports().map_err(|e| EdlError::Serial(e.to_string()))?;
    for port in &ports {
        if let serialport::SerialPortType::UsbPort(usb) = &port.port_type
            && usb.vid == QUALCOMM_VID
            && usb.pid == QUALCOMM_EDL_PID
        {
            return Ok(port.port_name.clone());
        }
    }
    Err(EdlError::PortNotFound)
}

pub fn check_device() -> bool {
    find_edl_port().is_ok()
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
        find_edl_port,
        std::thread::sleep,
        move || Instant::now() >= deadline,
        EDL_SESSION_OPEN_TIMEOUT,
    )
}

/// Wait for an EDL device; returns port name.
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
    /// `auto_reset` is **intentionally ignored** — `reset_on_drop` stays
    /// `false` because qdl's drop-time reset path recurses into
    /// `firehose_write` → `firehose_reset` → … and overflows the stack
    /// when the channel vanishes mid-flash. Call [`EdlSession::reset`]
    /// explicitly on the happy path.
    pub fn open(loader_path: &Path, auto_reset: bool, log: &mut Vec<String>) -> Result<Self> {
        let _ = auto_reset;
        log.push(format!("[EDL] {}", tr("log_edl_scanning")));
        let port = wait_for_stable_port()?;
        log.push(format!("[EDL] {} {port}", tr("log_edl_found_on")));

        log.push(format!(
            "[EDL] {} {}",
            tr("log_edl_loading_programmer"),
            loader_path.display()
        ));
        let mbn = std::fs::read(loader_path)
            .map_err(|e| EdlError::Session(format!("Failed to read loader: {e}")))?;
        log.push(format!(
            "[EDL] {} {} bytes",
            tr("log_edl_programmer_size"),
            mbn.len()
        ));

        log.push(format!("[EDL] {}", tr("log_edl_serial_transport")));
        let rw = qdl::setup_target_device(QdlBackend::Serial, None, Some(port.clone()))
            .map_err(|e| EdlError::Session(format!("Transport setup failed: {e}")))?;

        let mut dev = QdlDevice {
            rw,
            fh_cfg: FirehoseConfiguration {
                storage_type: FirehoseStorageType::Ufs,
                storage_sector_size: 4096,
                bypass_storage: false,
                backend: QdlBackend::Serial,
                skip_firehose_log: true,
                verbose_firehose: false,
                ..Default::default()
            },
            reset_on_drop: false,
        };

        log.push(format!("[EDL] {}", tr("log_edl_sahara_uploading")));
        qdl::sahara::sahara_run(
            &mut dev,
            qdl::sahara::SaharaMode::WaitingForImage,
            None,
            &mut [mbn],
            vec![],
            false,
        )
        .map_err(|e| EdlError::Session(format!("Sahara failed: {e}")))?;
        log.push(format!("[EDL] {}", tr("log_edl_sahara_uploaded")));

        // See `open` doc: reset_on_drop stays false to dodge qdl's recursive reset.
        dev.reset_on_drop = false;

        log.push(format!("[EDL] {}", tr("log_edl_firehose_configuring")));
        qdl::firehose_read(&mut dev, qdl::parsers::firehose_parser_ack_nak)
            .map_err(|e| EdlError::Session(format!("Firehose read failed: {e}")))?;
        qdl::firehose_configure(&mut dev, false)
            .map_err(|e| EdlError::Session(format!("Firehose configure failed: {e}")))?;
        qdl::firehose_read(&mut dev, qdl::parsers::firehose_parser_configure_response)
            .map_err(|e| EdlError::Session(format!("Firehose config response failed: {e}")))?;
        log.push(format!("[EDL] {}", tr("log_edl_firehose_configured")));

        Ok(Self { dev })
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
        let gpt_len = header.first_usable_lba as usize;
        let mut buf = Cursor::new(Vec::<u8>::new());
        qdl::firehose_read_storage(&mut self.dev, &mut buf, gpt_len, slot, lun, 0)
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
            log.push(format!("[EDL] {} LUN {lun}", tr("log_edl_reading_gpt")));
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
                    log.push(format!(
                        "[EDL] {}",
                        tr("log_edl_lun_gpt_read_failed")
                            .replace("{lun}", &lun.to_string())
                            .replace("{error}", &e.to_string())
                    ));
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
        let gpt_len = header.first_usable_lba as usize;
        buf.rewind()?;
        qdl::firehose_read_storage(&mut self.dev, &mut buf, gpt_len, slot, lun, 0)
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
        log.push(format!(
            "[EDL] {} '{part_name}' on LUN {lun}...",
            tr("log_edl_lookup_partition")
        ));
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
        log.push(format!(
            "[EDL] {} {part_name}: LBA {start}-{end} ({sectors} sectors)",
            tr("log_edl_found_partition")
        ));

        let mut out_file = std::fs::File::create(output)?;
        log.push(format!(
            "[EDL] $ {} {part_name} → {}",
            tr("log_edl_dump_cmd"),
            output.display()
        ));
        qdl::firehose_read_storage(&mut self.dev, &mut out_file, sectors, slot, lun, start_u32)
            .map_err(|e| EdlError::Session(format!("Partition read failed: {e}")))?;
        log.push(format!("[EDL] {} {part_name}", tr("log_edl_dumped")));
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
        log.push(format!(
            "[EDL] $ {} {part_name} → {} (LUN {lun}, start {start_sector}, {num_sectors} sectors)",
            tr("log_edl_dump_cmd"),
            output.display()
        ));
        qdl::firehose_read_storage(
            &mut self.dev,
            &mut out_file,
            num_sectors,
            0,
            lun,
            start_sector,
        )
        .map_err(|e| EdlError::Session(format!("Partition read failed: {e}")))?;
        log.push(format!("[EDL] {} {part_name}", tr("log_edl_dumped")));
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
        log: &mut Vec<String>,
    ) -> Result<()> {
        let mut file = std::fs::File::open(image)?;
        let file_len = file.metadata()?.len();
        let sector_size = self.dev.fh_config().storage_sector_size as u64;
        let num_sectors = file_len.div_ceil(sector_size) as usize;
        log.push(format!(
            "[EDL] $ {} {part_name} ← {} ({file_len} bytes, {num_sectors} sectors, LUN {lun})",
            tr("log_edl_flash_cmd"),
            image.display()
        ));
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
        log.push(format!("[EDL] {} {part_name}", tr("log_edl_flashed")));
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
        let total = header.backup_lba + 1;
        log.push(format!(
            "[EDL] {}",
            tr("log_edl_lun_total_sectors")
                .replace("{lun}", &lun.to_string())
                .replace("{total}", &total.to_string())
        ));
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
        log.push(format!(
            "[EDL] $ {}",
            tr("log_edl_dump_lun_cmd")
                .replace("{lun}", &lun.to_string())
                .replace("{path}", &output.display().to_string())
                .replace("{total}", &total.to_string())
        ));
        qdl::firehose_read_storage(&mut self.dev, &mut out_file, total as usize, 0, lun, 0)
            .map_err(|e| EdlError::Session(format!("Physical LUN read failed: {e}")))?;
        log.push(format!(
            "[EDL] {}",
            tr("log_edl_dumped_lun").replace("{lun}", &lun.to_string())
        ));
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
        log.push(format!(
            "[EDL] $ {}",
            tr("log_edl_flash_lun_cmd")
                .replace("{lun}", &lun.to_string())
                .replace("{path}", &image.display().to_string())
                .replace("{bytes}", &file_len.to_string())
                .replace("{sectors}", &num_sectors.to_string())
        ));
        qdl::firehose_program_storage(&mut self.dev, &mut file, "", num_sectors, 0, lun, "0")
            .map_err(|e| EdlError::Session(format!("Physical LUN write failed: {e}")))?;
        log.push(format!(
            "[EDL] {}",
            tr("log_edl_flashed_lun").replace("{lun}", &lun.to_string())
        ));
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
        log.push(format!(
            "[EDL] {}",
            tr("log_edl_erase_part_cmd")
                .replace("{part}", part_name)
                .replace("{lun}", &lun.to_string())
                .replace("{start}", start_sector)
                .replace("{sectors}", &num_sectors.to_string())
        ));
        qdl::firehose_erase_storage(&mut self.dev, num_sectors, lun, start_sector)
            .map_err(|e| EdlError::Session(format!("Erase {part_name} failed: {e}")))?;
        log.push(format!(
            "[EDL] {}",
            tr("log_edl_erased_part").replace("{part}", part_name)
        ));
        Ok(())
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
        log.push(format!(
            "[EDL] {} '{part_name}' on LUN {lun}...",
            tr("log_edl_lookup_partition")
        ));
        let (start, _end) = self.find_partition(part_name, slot, lun)?;

        let mut file = std::fs::File::open(image)?;
        let file_len = file.metadata()?.len();
        let sector_size = self.dev.fh_config().storage_sector_size as u64;
        let num_sectors = file_len.div_ceil(sector_size) as usize;
        log.push(format!(
            "[EDL] $ {} {part_name} ← {} ({file_len} bytes, {num_sectors} sectors)",
            tr("log_edl_flash_cmd"),
            image.display()
        ));

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
        log.push(format!("[EDL] {} {part_name}", tr("log_edl_flashed")));
        Ok(())
    }

    pub fn reset(&mut self, log: &mut Vec<String>) -> Result<()> {
        log.push(format!("[EDL] $ {}", tr("log_edl_reset_cmd")));
        qdl::firehose_reset(&mut self.dev, &FirehoseResetMode::Reset, 2)
            .map_err(|e| EdlError::Session(format!("Reset failed: {e}")))?;
        log.push(format!("[EDL] {}", tr("log_edl_reset_initiated")));
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
            log.push(format!("[EDL] Reset command returned after handoff: {e}"));
        }
    }

    /// Bounce back to Sahara (does NOT boot system). Required after a
    /// dump-only session so the next `open()` gets a fresh Hello —
    /// otherwise Sahara times out. Mirrors v2 qdl-rs default behavior.
    pub fn reset_to_edl(&mut self, log: &mut Vec<String>) -> Result<()> {
        log.push(format!("[EDL] $ {}", tr("log_edl_reset_to_edl_cmd")));
        qdl::firehose_reset(&mut self.dev, &FirehoseResetMode::ResetToEdl, 0)
            .map_err(|e| EdlError::Session(format!("reset_to_edl failed: {e}")))?;
        log.push(format!("[EDL] {}", tr("log_edl_reset_to_edl_sent")));
        Ok(())
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
            log.push(format!("[Flash] {}", tr("log_flash_wipe_enabled")));
            self.pre_erase_wipe_labels(program_xmls, log)?;
        } else {
            log.push(format!("[Flash] {}", tr("log_flash_wipe_disabled")));
        }

        for xml_path in program_xmls {
            log.push(format!(
                "[EDL] $ {} {}",
                tr("log_edl_flash_cmd"),
                xml_path.display()
            ));
            self.flash_one_rawprogram(xml_path, wipe, log)?;
        }
        for xml_path in patch_xmls {
            log.push(format!(
                "[EDL] $ {}",
                tr("log_edl_patch_xml_cmd").replace("{path}", &xml_path.display().to_string())
            ));
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
            log.push(entry.log_line());
            qdl::firehose_erase_storage(
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
                            log.push(format!(
                                "[EDL] {}",
                                tr("log_edl_skip_keep_data").replace("{label}", label)
                            ));
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

        let image_path = xml_dir.join(&filename);
        if !image_path.exists() {
            log.push(format!(
                "[EDL] {}",
                tr("log_edl_skip_image_missing")
                    .replace("{label}", &label)
                    .replace("{path}", &image_path.display().to_string())
            ));
            return Ok(());
        }

        let mut file = std::fs::File::open(&image_path)?;
        if file_sector_offset > 0 {
            file.seek(SeekFrom::Start(sector_size * file_sector_offset))?;
        }

        log.push(format!(
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
        ));

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

        log.push(format!(
            "[EDL] {}",
            tr("log_edl_erase_lun_cmd")
                .replace("{lun}", &lun.to_string())
                .replace("{start}", start_sector)
                .replace("{sectors}", &num_sectors.to_string())
        ));
        qdl::firehose_erase_storage(&mut self.dev, num_sectors, lun, start_sector)
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

            log.push(format!(
                "[EDL] {}",
                tr("log_edl_patch_lun_cmd")
                    .replace("{lun}", &lun.to_string())
                    .replace("{start}", start_sector)
                    .replace("{offset}", &byte_off.to_string())
                    .replace("{bytes}", &size.to_string())
                    .replace("{value}", value)
            ));
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

/// Collect `rawprogram*.xml` and `patch*.xml` from `dir`. Drops v2 filter
/// targets (WIPE/BLANK variants, `rawprogram0.xml` GPT programmer).
/// Returns `(raw_xmls, patch_xmls)` sorted.
pub fn collect_firmware_xmls(dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut raw_xmls = Vec::new();
    let mut patch_xmls = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return (raw_xmls, patch_xmls),
    };

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
            raw_xmls.push(path);
        } else if lower.starts_with("patch") {
            patch_xmls.push(path);
        }
    }

    raw_xmls.sort();
    patch_xmls.sort();
    (raw_xmls, patch_xmls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::{SystemTime, UNIX_EPOCH};

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
}
