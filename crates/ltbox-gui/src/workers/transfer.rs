//! Partition + physical-range flash/dump workers. Each runs off the
//! UI thread: route to EDL, open Sahara/Firehose, scan GPTs or
//! transfer images, then reset. Extracted from `main.rs`.

use crate::{
    ConnectionStatus, DumpPartRow, DumpPartsScanResult, FlashPartRow, FlashPartsScanResult,
    FlashRowState, ensure_edl,
};
use ltbox_core::tr_args;

/// Shared scan phase for the Flash/Dump Partitions wizards: route to EDL,
/// open Sahara, read GPTs on LUN 0..=5. On success returns the scanned
/// partitions plus the still-open session so the caller can map its rows
/// and then bounce the device back to EDL. On failure it logs (and, for a
/// scan error, resets) and returns the error string. `tag` is the log
/// channel prefix; `open_failed_key` / `scan_failed_key` are the i18n keys
/// for the two failure messages (the only per-wizard text difference).
fn scan_lun_partitions(
    conn: ConnectionStatus,
    loader_path: &str,
    tag: &str,
    open_failed_key: &str,
    scan_failed_key: &str,
    log: &mut Vec<String>,
) -> Result<
    (
        Vec<ltbox_device::edl::GptPartitionInfo>,
        ltbox_device::edl::EdlSession,
    ),
    String,
> {
    if ensure_edl(conn, tag, log).is_err() {
        return Err(ltbox_core::i18n::tr("err_edl_transition_failed"));
    }

    std::thread::sleep(std::time::Duration::from_secs(2));
    let loader = std::path::PathBuf::from(loader_path);
    let mut session = match ltbox_device::edl::EdlSession::open(&loader, true, log) {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[{}] {}",
                tag,
                tr_args!(open_failed_key, error = e.to_string())
            );
            return Err(tr_args!(
                "err_edl_session_open_failed",
                error = e.to_string()
            ));
        }
    };

    match session.scan_partitions(0..=5, log) {
        Ok(parts) => Ok((parts, session)),
        Err(e) => {
            ltbox_core::live!(
                log,
                "[{}] {}",
                tag,
                tr_args!(scan_failed_key, error = e.to_string())
            );
            let _ = session.reset_to_edl(log);
            Err(tr_args!("err_parts_scan_failed", error = e.to_string()))
        }
    }
}

/// Flash Partitions scan phase. Mirror of `dump_parts_scan`: shares the
/// transition + open + GPT scan via `scan_lun_partitions`, then maps the
/// partitions into checkable flash rows and bounces back to EDL so the
/// exec pass can reopen without a power-cycle.
pub(crate) fn flash_parts_scan(
    conn: ConnectionStatus,
    loader_path: String,
) -> FlashPartsScanResult {
    let mut log = Vec::new();
    let (parts, mut session) = match scan_lun_partitions(
        conn,
        &loader_path,
        "FlashParts",
        "live_flashparts_edl_open_failed",
        "live_flashparts_scan_failed",
        &mut log,
    ) {
        Ok(v) => v,
        Err(error) => {
            return FlashPartsScanResult {
                logs: log,
                rows: Vec::new(),
                error: Some(error),
            };
        }
    };

    let rows: Vec<FlashPartRow> = parts
        .into_iter()
        .map(|p| FlashPartRow {
            lun: p.lun,
            label: p.name,
            start_sector: p.start_sector,
            num_sectors: p.num_sectors,
            size_bytes: p.size_bytes,
            file_path: None,
            state: FlashRowState::Unchecked,
        })
        .collect();

    if let Err(e) = session.reset_to_edl(&mut log) {
        ltbox_core::live!(
            log,
            "[FlashParts] {}",
            tr_args!("live_flashparts_reset_failed", error = e)
        );
    }

    ltbox_core::live!(
        log,
        "[FlashParts] {}",
        tr_args!(
            "live_dumpparts_scan_complete",
            count = rows.len().to_string()
        )
    );
    FlashPartsScanResult {
        logs: log,
        rows,
        error: None,
    }
}

/// Exec phase. Reopens the EDL session, walks the active rows, flashing
/// or erasing each, then reboots to system.
pub(crate) fn flash_parts_execute(
    loader_path: String,
    rows: Vec<FlashPartRow>,
) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let loader = std::path::PathBuf::from(&loader_path);
    let mut session = match ltbox_device::edl::EdlSession::open(&loader, true, &mut log) {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[FlashParts] {}",
                tr_args!("live_flashparts_edl_open_failed", error = e.to_string())
            );
            return Err(tr_args!(
                "err_edl_session_open_failed",
                error = e.to_string()
            ));
        }
    };

    for row in &rows {
        match row.state {
            FlashRowState::Flash => {
                let Some(path) = row.file_path.as_ref() else {
                    ltbox_core::live!(
                        log,
                        "[FlashParts] {}",
                        tr_args!("live_flashparts_skipping", label = row.label)
                    );
                    continue;
                };
                let img = std::path::Path::new(path);
                if !img.exists() {
                    ltbox_core::live!(
                        log,
                        "[FlashParts] {}",
                        tr_args!(
                            "live_flashparts_skipping_missing",
                            label = row.label,
                            path = path
                        )
                    );
                    continue;
                }
                let file_name = img
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone());
                ltbox_core::live!(
                    log,
                    "[FlashParts] {}",
                    tr_args!(
                        "live_flashparts_flashing",
                        label = row.label,
                        file = file_name,
                        lun = row.lun.to_string()
                    )
                );
                if let Err(e) = session.flash_partition_at(
                    &row.label,
                    img,
                    row.lun,
                    &row.start_sector.to_string(),
                    row.num_sectors,
                    &mut log,
                ) {
                    ltbox_core::live!(
                        log,
                        "[FlashParts] {}",
                        tr_args!(
                            "live_flashparts_part_failed",
                            label = row.label,
                            error = e.to_string()
                        )
                    );
                    // Abort the remaining writes — a failed write can mean a
                    // dropped link, and the device is left in EDL for retry.
                    return Err(tr_args!(
                        "err_flash_parts_part_failed",
                        label = row.label,
                        error = e.to_string()
                    ));
                }
            }
            FlashRowState::Erase => {
                ltbox_core::live!(
                    log,
                    "[FlashParts] {}",
                    tr_args!(
                        "live_flashparts_erasing",
                        label = row.label,
                        lun = row.lun.to_string(),
                        sectors = row.num_sectors.to_string()
                    )
                );
                if let Err(e) = session.erase_partition_at(
                    &row.label,
                    row.lun,
                    &row.start_sector.to_string(),
                    row.num_sectors as usize,
                    &mut log,
                ) {
                    ltbox_core::live!(
                        log,
                        "[FlashParts] {}",
                        tr_args!(
                            "live_flashparts_erase_failed",
                            label = row.label,
                            error = e.to_string()
                        )
                    );
                    return Err(tr_args!(
                        "err_flash_parts_erase_failed",
                        label = row.label,
                        error = e.to_string()
                    ));
                }
            }
            FlashRowState::Unchecked => {}
        }
    }

    ltbox_core::live!(
        log,
        "[FlashParts] {}",
        ltbox_core::i18n::tr("live_flashparts_resetting")
    );
    session.reset_tolerant(&mut log);
    ltbox_core::live!(
        log,
        "[FlashParts] {}",
        ltbox_core::i18n::tr("live_flashparts_done")
    );
    Ok(log)
}

/// Scan GPTs on LUNs 0..=5 using the picked loader. Leaves the device
/// in EDL (bounces through `reset_to_edl`) so the dump pass can re-open
/// Sahara without a power-cycle.
pub(crate) fn dump_parts_scan(conn: ConnectionStatus, loader_path: String) -> DumpPartsScanResult {
    let mut log = Vec::new();
    let (parts, mut session) = match scan_lun_partitions(
        conn,
        &loader_path,
        "DumpParts",
        "live_dumpparts_edl_open_failed",
        "live_dumpparts_scan_failed",
        &mut log,
    ) {
        Ok(v) => v,
        Err(error) => {
            return DumpPartsScanResult {
                logs: log,
                rows: Vec::new(),
                error: Some(error),
            };
        }
    };

    let rows: Vec<DumpPartRow> = parts
        .into_iter()
        .map(|p| DumpPartRow {
            lun: p.lun,
            label: p.name,
            start_sector: p.start_sector,
            num_sectors: p.num_sectors,
            size_bytes: p.size_bytes,
            selected: false,
        })
        .collect();

    // Bounce back to Sahara so the next `open()` on the dump pass gets
    // a fresh Hello. Without this Sahara times out.
    if let Err(e) = session.reset_to_edl(&mut log) {
        ltbox_core::live!(
            log,
            "[DumpParts] {}",
            tr_args!("live_dumpparts_reset_failed", error = e)
        );
    }

    ltbox_core::live!(
        log,
        "[DumpParts] {}",
        tr_args!(
            "live_dumpparts_scan_complete",
            count = rows.len().to_string()
        )
    );
    DumpPartsScanResult {
        logs: log,
        rows,
        error: None,
    }
}

/// Post-dump stability window before the next EDL op. Large partition
/// reads (e.g. boot_a ~96 MB) leave the USB endpoint in a lingering state;
/// a subsequent reset/open can race a still-draining read and surface as
/// "stale COM port" or Sahara timeout. Mirrors v2 `post_sleep=15` in
/// `bin/ltbox/actions/edl.py::dump_partitions`.
const EDL_POST_DUMP_STABILIZE: std::time::Duration = std::time::Duration::from_secs(15);

/// Partition bases whose dump failure must be surfaced as a critical
/// error, not a per-row log line. These carry region/board state that a
/// subsequent rescue flow cannot reconstruct from scratch. Mirrors v2
/// `critical_targets` set in `bin/ltbox/actions/edl.py::dump_partitions`.
const CRITICAL_DUMP_BASES: &[&str] = &["devinfo", "persist"];

/// Match a partition label (possibly slot-suffixed) against the critical
/// base set. `devinfo`, `devinfo_a`, `DEVINFO_B` all match.
pub(crate) fn is_critical_dump_label(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    CRITICAL_DUMP_BASES
        .iter()
        .any(|base| l == *base || l.starts_with(&format!("{base}_")))
}

#[derive(Debug, Default)]
pub(crate) struct CountryPatchProgress {
    /// Labels that must be patched for the run to count as complete. Set
    /// per-run because the country-code partition differs by model
    /// (`devinfo` on most SKUs, `oemowninfo` on TB320FC / TB323FU).
    expected: Vec<String>,
    flashed_or_confirmed: Vec<String>,
    failures: Vec<String>,
}

impl CountryPatchProgress {
    pub(crate) fn new(expected: &[&str]) -> Self {
        Self {
            expected: expected.iter().map(|s| s.to_string()).collect(),
            ..Self::default()
        }
    }

    pub(crate) fn mark_flashed(&mut self, label: &str) {
        if !self.flashed_or_confirmed.iter().any(|seen| seen == label) {
            self.flashed_or_confirmed.push(label.to_string());
        }
    }

    pub(crate) fn mark_failed(&mut self, label: &str, reason: impl Into<String>) {
        self.failures.push(format!("{label}: {}", reason.into()));
    }

    pub(crate) fn finish(&self) -> std::result::Result<(), String> {
        let missing = self
            .expected
            .iter()
            .filter(|label| !self.flashed_or_confirmed.iter().any(|seen| seen == *label))
            .cloned()
            .collect::<Vec<_>>();

        if self.failures.is_empty() && missing.is_empty() {
            return Ok(());
        }

        let mut parts = Vec::new();
        if !self.failures.is_empty() {
            parts.push(self.failures.join("; "));
        }
        if !missing.is_empty() {
            parts.push(format!("missing {}", missing.join(", ")));
        }
        Err(format!(
            "country-code patch incomplete ({})",
            parts.join("; ")
        ))
    }
}

/// Forward buffered worker logs to the stdout tap queue immediately.
///
/// Long-running advanced actions often collect lines in a local `Vec<String>`
/// and only hand that vec back on completion, which makes the exec card look
/// stalled. Emitting lines here lets the UI drain them every 500 ms via
/// `DrainStdoutTap`.
pub(crate) fn flush_worker_logs(log: &mut Vec<String>) {
    for line in log.drain(..) {
        println!("{line}");
    }
}

/// Dump selected partitions to `output_folder` as `<label>.img`. Reopens
/// the EDL session (previous scan left device waiting at Sahara), runs
/// the reads back-to-back, then reboots to system.
pub(crate) fn dump_parts_execute(
    loader_path: String,
    output_folder: String,
    rows: Vec<DumpPartRow>,
) -> Vec<String> {
    let mut log = Vec::new();
    let out_dir = std::path::PathBuf::from(&output_folder);
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        ltbox_core::live!(
            log,
            "[DumpParts] {}",
            tr_args!("live_dumpparts_create_output_failed", error = e.to_string())
        );
        return log;
    }

    std::thread::sleep(std::time::Duration::from_secs(2));
    let loader = std::path::PathBuf::from(&loader_path);
    let mut session = match ltbox_device::edl::EdlSession::open(&loader, true, &mut log) {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[DumpParts] {}",
                tr_args!("live_dumpparts_edl_open_failed", error = e.to_string())
            );
            return log;
        }
    };

    let mut critical_failures: Vec<String> = Vec::new();
    for row in &rows {
        let out_path =
            match ltbox_core::safe_path::safe_join(&out_dir, &format!("{}.img", row.label)) {
                Ok(p) => p,
                Err(e) => {
                    // A device-reported GPT label is untrusted; refuse one that
                    // would escape the chosen output directory rather than
                    // writing through the traversal.
                    ltbox_core::live!(
                        log,
                        "[DumpParts] {}",
                        tr_args!(
                            "live_dumpparts_part_failed",
                            label = row.label,
                            error = e.to_string()
                        )
                    );
                    if is_critical_dump_label(&row.label) {
                        critical_failures.push(row.label.clone());
                    }
                    continue;
                }
            };
        ltbox_core::live!(
            log,
            "[DumpParts] {}",
            tr_args!(
                "live_dumpparts_dumping",
                label = row.label,
                path = out_path.display().to_string(),
                lun = row.lun.to_string(),
                bytes = row.size_bytes.to_string()
            )
        );
        // GPT sector values are u64; Firehose takes a u32 start + usize
        // count. Reject out-of-range rather than silently truncating
        // `as u32` — a start LBA past u32::MAX would wrap and dump the
        // wrong region of the LUN.
        let dump_outcome = match (
            u32::try_from(row.start_sector),
            usize::try_from(row.num_sectors),
        ) {
            (Ok(start), Ok(count)) => session
                .dump_partition_at(&row.label, &out_path, row.lun, start, count, &mut log)
                .map_err(|e| e.to_string()),
            _ => Err(format!(
                "partition geometry out of range (start_sector={}, num_sectors={})",
                row.start_sector, row.num_sectors
            )),
        };
        if let Err(e) = dump_outcome {
            ltbox_core::live!(
                log,
                "[DumpParts] {}",
                tr_args!("live_dumpparts_part_failed", label = row.label, error = e)
            );
            if is_critical_dump_label(&row.label) {
                critical_failures.push(row.label.clone());
            }
        }
    }

    ltbox_core::live!(
        log,
        "[DumpParts] {}",
        tr_args!(
            "live_dumpparts_stabilizing",
            seconds = EDL_POST_DUMP_STABILIZE.as_secs().to_string()
        )
    );
    std::thread::sleep(EDL_POST_DUMP_STABILIZE);
    ltbox_core::live!(
        log,
        "[DumpParts] {}",
        ltbox_core::i18n::tr("live_dumpparts_resetting")
    );
    session.reset_tolerant(&mut log);
    // Surface critical-partition failures prominently — region/board state
    // (devinfo/persist) can't be reconstructed from a partial dump and a
    // silent "Done." would hide the hazard.
    if !critical_failures.is_empty() {
        ltbox_core::live!(
            log,
            "[DumpParts] {}",
            tr_args!(
                "live_dumpparts_critical_failure",
                labels = critical_failures.join(", ")
            )
        );
    }
    ltbox_core::live!(
        log,
        "[DumpParts] {}",
        ltbox_core::i18n::tr("live_dumpparts_done")
    );
    log
}

/// Whole-LUN dump. Walks each selected LUN and writes it as
/// `lun_N.img` into `output_folder`. Unlike `dump_parts_execute` there
/// is no prior scan phase — the LUN set comes straight from the user's
/// checkboxes.
pub(crate) fn dump_physical_execute(
    conn: ConnectionStatus,
    loader_path: String,
    output_folder: String,
    luns: Vec<u8>,
) -> Vec<String> {
    let mut log = Vec::new();
    if ensure_edl(conn, "DumpPhys", &mut log).is_err() {
        flush_worker_logs(&mut log);
        return Vec::new();
    }
    flush_worker_logs(&mut log);
    let out_dir = std::path::PathBuf::from(&output_folder);
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        ltbox_core::live!(
            log,
            "[DumpPhys] {}",
            tr_args!("live_dump_phys_create_output_failed", error = e.to_string())
        );
        flush_worker_logs(&mut log);
        return Vec::new();
    }

    std::thread::sleep(std::time::Duration::from_secs(2));
    let loader = std::path::PathBuf::from(&loader_path);
    let mut session = match ltbox_device::edl::EdlSession::open(&loader, true, &mut log) {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[DumpPhys] {}",
                tr_args!("live_dump_phys_edl_open_failed", error = e.to_string())
            );
            flush_worker_logs(&mut log);
            return Vec::new();
        }
    };
    flush_worker_logs(&mut log);

    for lun in &luns {
        let out_path = out_dir.join(format!("lun_{lun}.img"));
        ltbox_core::live!(
            log,
            "[DumpPhys] {}",
            tr_args!(
                "live_dump_phys_dumping_lun",
                lun = lun.to_string(),
                path = out_path.display().to_string()
            )
        );
        flush_worker_logs(&mut log);
        if let Err(e) = session.dump_physical_storage(*lun, &out_path, &mut log) {
            ltbox_core::live!(
                log,
                "[DumpPhys] {}",
                tr_args!(
                    "live_dump_phys_lun_failed",
                    lun = lun.to_string(),
                    error = e.to_string()
                )
            );
        }
        flush_worker_logs(&mut log);
    }

    ltbox_core::live!(
        log,
        "[DumpPhys] {}",
        tr_args!(
            "live_dump_phys_stabilizing_usb",
            seconds = EDL_POST_DUMP_STABILIZE.as_secs().to_string()
        )
    );
    flush_worker_logs(&mut log);
    std::thread::sleep(EDL_POST_DUMP_STABILIZE);
    ltbox_core::live!(
        log,
        "[DumpPhys] {}",
        ltbox_core::i18n::tr("live_dump_phys_resetting_system")
    );
    session.reset_tolerant(&mut log);
    ltbox_core::live!(
        log,
        "[DumpPhys] {}",
        ltbox_core::i18n::tr("live_dump_phys_done")
    );
    flush_worker_logs(&mut log);
    Vec::new()
}

/// Whole-LUN raw flash. Each `(lun, path)` pair is written verbatim
/// from sector 0. Mirrors qdlrs `OverwriteStorage`.
pub(crate) fn flash_physical_execute(
    conn: ConnectionStatus,
    loader_path: String,
    pairs: Vec<(u8, String)>,
) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    if ensure_edl(conn, "FlashPhys", &mut log).is_err() {
        return Err(ltbox_core::i18n::tr("err_edl_transition_failed"));
    }

    std::thread::sleep(std::time::Duration::from_secs(2));
    let loader = std::path::PathBuf::from(&loader_path);
    let mut session = match ltbox_device::edl::EdlSession::open(&loader, true, &mut log) {
        Ok(s) => s,
        Err(e) => {
            ltbox_core::live!(
                log,
                "[FlashPhys] {}",
                tr_args!("live_flashphys_edl_open_failed", error = e.to_string())
            );
            return Err(tr_args!(
                "err_edl_session_open_failed",
                error = e.to_string()
            ));
        }
    };

    for (lun, path) in &pairs {
        let img = std::path::Path::new(path);
        if !img.exists() {
            ltbox_core::live!(
                log,
                "[FlashPhys] {}",
                tr_args!(
                    "live_flashphys_skipping_missing",
                    lun = lun.to_string(),
                    path = path
                )
            );
            continue;
        }
        let file_name = img
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        ltbox_core::live!(
            log,
            "[FlashPhys] {}",
            tr_args!(
                "live_flashphys_flashing",
                lun = lun.to_string(),
                file = file_name
            )
        );
        if let Err(e) = session.flash_physical_storage(*lun, img, &mut log) {
            ltbox_core::live!(
                log,
                "[FlashPhys] {}",
                tr_args!(
                    "live_flashphys_lun_failed",
                    lun = lun.to_string(),
                    error = e.to_string()
                )
            );
            // Abort remaining LUN writes; device stays in EDL for retry.
            return Err(tr_args!(
                "err_flash_phys_write_failed",
                lun = lun.to_string(),
                error = e.to_string()
            ));
        }
    }

    ltbox_core::live!(
        log,
        "[FlashPhys] {}",
        ltbox_core::i18n::tr("live_flashphys_resetting")
    );
    session.reset_tolerant(&mut log);
    ltbox_core::live!(
        log,
        "[FlashPhys] {}",
        ltbox_core::i18n::tr("live_flashphys_done")
    );
    Ok(log)
}
