//! Firmware flash worker: validate the firmware folder, route to EDL,
//! apply region / rollback / country / wipe modifications, and flash every
//! image (incl. the TB323FU ARB-overlay path). Extracted from the
//! update_flash handler.

use crate::{
    ConnectionStatus, CountryPatchProgress, LiveLabels, WorkflowConfig, active_slot_suffix,
    build_tb323fu_arb_overlays, efisp_asset_suffix, find_edl_loader, fingerprint_token_match,
    is_rollback_protected_model, phase_marker, read_device_rollback_index_via_edl,
    transition_to_edl,
};
use ltbox_core::{live, tr_args};

fn should_reboot_fastboot_to_system_after_pre_edl_abort(started_in_fastboot: bool) -> bool {
    !started_in_fastboot
}

fn reboot_fastboot_to_system_after_pre_edl_abort(log: &mut Vec<String>, started_in_fastboot: bool) {
    if !should_reboot_fastboot_to_system_after_pre_edl_abort(started_in_fastboot) {
        return;
    }
    if !ltbox_device::fastboot::FastbootDevice::check_device() {
        return;
    }
    ltbox_core::live!(
        log,
        "[Fastboot] {}",
        ltbox_core::i18n::tr("live_fastboot_rebooting_system")
    );
    match ltbox_device::fastboot::FastbootDevice::open() {
        Ok(mut dev) => {
            if let Err(e) = dev.reboot() {
                ltbox_core::live!(
                    log,
                    "[Fastboot] {}",
                    tr_args!("live_fastboot_reboot_failed", error = e.to_string())
                );
            }
        }
        Err(e) => {
            ltbox_core::live!(
                log,
                "[Fastboot] {}",
                tr_args!("live_fastboot_open_failed", error = e.to_string())
            );
        }
    }
}

/// Supported Lenovo SKUs, matched as fingerprint tokens to recover the device
/// model from an EDL-dumped vendor_boot when fastboot/ADB never ran.
const SUPPORTED_MODELS: [&str; 6] = [
    "TB320FC", "TB321FU", "TB322FC", "TB323FU", "TB520FU", "TB710FU",
];

/// EDL-start device identity + rollback floor, read over the open EDL session.
struct EdlStartProbe {
    /// Device model token recovered from the vbmeta_system fingerprint, or empty
    /// when no supported SKU token matched.
    model_token: String,
    /// Component-wise device rollback floors `(boot, vbmeta_system)` — the MAX
    /// of each location across both slots — or `None` for the no-ARB TB322FC.
    rollback_floors: Option<(u64, u64)>,
}

/// Read the device model + committed rollback index on an EDL-start flash by
/// dumping BOTH slots over the open session. The active slot is unknown in
/// EDL-start and the inactive slot's images may not carry parseable AVB info,
/// so each partition is dumped from `_a` and `_b` and the valid side is used
/// (the higher-index slot when both parse). Returns `Err` (the caller resets
/// back into EDL and aborts) when neither slot yields a valid vbmeta_system,
/// when the recovered model does not match the target firmware, or — for a
/// rollback-protected model — when neither slot yields a valid boot +
/// vbmeta_system pair.
fn read_edl_start_device(
    session: &mut ltbox_device::edl::EdlSession,
    firmware_fp: Option<&str>,
    log: &mut Vec<String>,
) -> std::result::Result<EdlStartProbe, String> {
    let work_dir = ltbox_core::app_paths::work_dir_for("flash_edl_probe");
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir).map_err(|e| format!("edl probe work dir: {e}"))?;

    // 1. Model + vbmeta_system rollback index — dump vbmeta_system from BOTH
    //    slots. It carries the device build fingerprint (system.fingerprint) AND
    //    the per-location rollback index, so one dump identifies the model and
    //    yields the vbmeta_system floor; vendor_boot / vbmeta need no separate
    //    dump. A device left mid-cross-flash can have one slot on a different
    //    SKU, so keep every fingerprint that parses.
    let mut device_fps: Vec<String> = Vec::new();
    let mut vbs_idx: [Option<u64>; 2] = [None, None];
    for (i, slot) in ["_a", "_b"].into_iter().enumerate() {
        let part = format!("vbmeta_system{slot}");
        let Some(lun) = ltbox_core::partition_lun::lun_for_partition(&part) else {
            continue;
        };
        let out = work_dir.join(format!("dev_{part}.img"));
        if let Some(info) = session
            .dump_partition(&part, &out, 0, lun, log)
            .ok()
            .and_then(|_| ltbox_patch::avb::extract_image_avb_info(&out).ok())
        {
            if let Some(fp) = ltbox_patch::avb::build_fingerprint(&info) {
                device_fps.push(fp);
            }
            vbs_idx[i] = Some(info.rollback_index);
        }
        let _ = std::fs::remove_file(&out);
    }
    if device_fps.is_empty() {
        return Err(ltbox_core::i18n::tr("err_flash_edl_avb_invalid"));
    }

    // Recover the model token, preferring a slot whose SKU the target firmware
    // also names — a device left mid-cross-flash can carry a stale non-matching
    // slot alongside the matching one. On EDL-start there is no other model
    // source, so an unrecognized device, a firmware image with no vendor_boot
    // fingerprint, or a genuine mismatch all abort (the caller resets back into
    // EDL) rather than flash blind — TB323FU especially, since its ARB/region
    // gates key off the model and would silently stay off on an unidentified
    // device.
    let mut model_token = String::new();
    'find: for fp in &device_fps {
        for m in SUPPORTED_MODELS {
            if fingerprint_token_match(fp, m) {
                let matches_fw = firmware_fp
                    .map(|fw| fingerprint_token_match(fw, m))
                    .unwrap_or(false);
                if matches_fw {
                    model_token = m.to_string();
                    break 'find;
                }
                if model_token.is_empty() {
                    model_token = m.to_string();
                }
            }
        }
    }
    let firmware_matches = firmware_fp
        .map(|fw| fingerprint_token_match(fw, &model_token))
        .unwrap_or(false);
    if model_token.is_empty() || !firmware_matches {
        ltbox_core::live!(
            log,
            "[Flash] {}",
            tr_args!(
                "live_rescue_model_mismatch_abort",
                device = if model_token.is_empty() {
                    device_fps.join(", ")
                } else {
                    model_token.clone()
                },
                fingerprint = firmware_fp.unwrap_or("").to_string()
            )
        );
        return Err(ltbox_core::i18n::tr("err_flash_model_mismatch_pre_edl"));
    }
    ltbox_core::live!(
        log,
        "[Flash] {}",
        ltbox_core::i18n::tr("live_rescue_model_check_ok")
    );

    // 2. TB322FC has no rollback protection — skip the index read.
    if model_token.eq_ignore_ascii_case("TB322FC") {
        return Ok(EdlStartProbe {
            model_token,
            rollback_floors: None,
        });
    }

    // 3. Rollback floor — vbmeta_system was read above; add the boot floor by
    //    dumping boot from both slots, then take the per-location MAX across
    //    slots. AVB rollback indices are per-location and the two slots can hold
    //    different images, so a single slot can underestimate one location (and
    //    TB323FU re-signs each location to its own floor). Each location must
    //    parse on at least one slot.
    ltbox_core::live!(log, "[ARB] {}", ltbox_core::i18n::tr("live_arb_edl_dump"));
    let mut boot_idx: [Option<u64>; 2] = [None, None];
    for (i, slot) in ["_a", "_b"].into_iter().enumerate() {
        let boot = format!("boot{slot}");
        let Some(boot_lun) = ltbox_core::partition_lun::lun_for_partition(&boot) else {
            continue;
        };
        let boot_img = work_dir.join(format!("dev_{boot}.img"));
        boot_idx[i] = session
            .dump_partition(&boot, &boot_img, 0, boot_lun, log)
            .ok()
            .and_then(|_| ltbox_patch::avb::extract_image_avb_info(&boot_img).ok())
            .map(|info| info.rollback_index);
        let _ = std::fs::remove_file(&boot_img);
    }
    let Some(floors) = rollback_floors(boot_idx, vbs_idx) else {
        return Err(ltbox_core::i18n::tr("err_flash_edl_avb_invalid"));
    };
    Ok(EdlStartProbe {
        model_token,
        rollback_floors: Some(floors),
    })
}

/// Component-wise device rollback floors from per-slot boot / vbmeta_system
/// indices (`None` = that slot's image did not parse). Each location's floor is
/// the MAX across both slots, because AVB rollback indices are per-location and
/// the two slots can hold different images. Returns `None` (the caller aborts)
/// when a location parsed on neither slot.
fn rollback_floors(boot: [Option<u64>; 2], vbs: [Option<u64>; 2]) -> Option<(u64, u64)> {
    let boot_floor = boot.into_iter().flatten().max()?;
    let vbs_floor = vbs.into_iter().flatten().max()?;
    Some((boot_floor, vbs_floor))
}

/// Dump a partition over EDL and parse its AVB info, cleaning up the temp file.
/// `None` on any dump/parse failure.
fn dump_avb_info(
    session: &mut ltbox_device::edl::EdlSession,
    part: &str,
    lun: u8,
    work_dir: &std::path::Path,
    log: &mut Vec<String>,
) -> Option<ltbox_patch::avb::AvbImageInfo> {
    let out = work_dir.join(format!("dev_{part}.img"));
    let info = session
        .dump_partition(part, &out, 0, lun, log)
        .ok()
        .and_then(|_| ltbox_patch::avb::extract_image_avb_info(&out).ok());
    let _ = std::fs::remove_file(&out);
    info
}

/// Device active-slot identity (read from vbmeta_system), for the AVB key-class
/// policy.
struct DeviceVbmeta {
    /// Active slot suffix (`_a` / `_b`).
    slot: &'static str,
    /// Root-of-trust class of the active-slot vbmeta_system.
    class: ltbox_patch::key_map::KeyClass,
    /// avbtool key spec for the active-slot vbmeta_system pubkey (`Some` iff
    /// `class == Testkey`) — used to reject re-signing a device on a testkey the
    /// re-sign path doesn't support.
    key_spec: Option<&'static str>,
    /// Device-committed per-location rollback floors — the MAX of each location
    /// across BOTH slots (AVB indices are per-location; the slots can differ).
    boot_floor: u64,
    vbs_floor: u64,
}

/// Read the device's active-slot root-of-trust + committed rollback floors from
/// vbmeta_system — the unified identity source: it carries the signing key (root
/// of trust), the device build fingerprint, and the per-location rollback index,
/// so vbmeta / vendor_boot need no separate dump here. The active slot is
/// `known_active` (fastboot `current-slot`) when set; otherwise (EDL-start) it
/// is the valid vbmeta_system side, or — when both parse — the slot with the
/// higher vbmeta_system rollback index (ties favour `_a`). The floors are
/// component-wise maxima of `boot` + `vbmeta_system` across BOTH slots (the
/// device's true committed floor — a single slot can understate one location).
/// Errors (the caller resets to the device's start mode and aborts) when no
/// slot's vbmeta_system / floors parse.
fn read_device_vbmeta(
    session: &mut ltbox_device::edl::EdlSession,
    known_active: Option<&str>,
    work_dir: &std::path::Path,
    log: &mut Vec<String>,
) -> std::result::Result<DeviceVbmeta, String> {
    let vbs_lun = ltbox_core::partition_lun::lun_for_partition("vbmeta_system")
        .ok_or_else(|| "no LUN for vbmeta_system".to_string())?;
    let boot_lun = ltbox_core::partition_lun::lun_for_partition("boot")
        .ok_or_else(|| "no LUN for boot".to_string())?;

    // Per slot: full vbmeta_system AVB info (validity + pubkey + rollback index)
    // and the boot rollback index.
    let mut vbs_info: [Option<ltbox_patch::avb::AvbImageInfo>; 2] = [None, None];
    let mut boot_idx: [Option<u64>; 2] = [None, None];
    for (i, slot) in ["_a", "_b"].into_iter().enumerate() {
        vbs_info[i] = dump_avb_info(
            session,
            &format!("vbmeta_system{slot}"),
            vbs_lun,
            work_dir,
            log,
        );
        boot_idx[i] = dump_avb_info(session, &format!("boot{slot}"), boot_lun, work_dir, log)
            .map(|info| info.rollback_index);
    }

    // Active slot: fastboot's when known; otherwise the valid vbmeta_system side,
    // or (both valid) the higher vbmeta_system rollback index, ties favour `_a`.
    let active = match known_active {
        Some(s) => usize::from(active_slot_suffix(Some(s)) == "_b"),
        None => match (vbs_info[0].as_ref(), vbs_info[1].as_ref()) {
            (Some(_), None) => 0,
            (None, Some(_)) => 1,
            (Some(a), Some(b)) => usize::from(b.rollback_index > a.rollback_index),
            (None, None) => {
                return Err("no device vbmeta_system parsed on either slot".to_string());
            }
        },
    };
    let slot = if active == 1 { "_b" } else { "_a" };

    // Class + testkey spec from the active-slot vbmeta_system pubkey.
    let active_info = vbs_info[active]
        .as_ref()
        .ok_or_else(|| format!("device vbmeta_system{slot} AVB unreadable"))?;
    let pubkey = active_info.public_key_sha1.as_deref();
    let class = ltbox_patch::key_map::classify_pubkey(pubkey);
    let key_spec = ltbox_patch::key_map::key_spec_for_pubkey(pubkey);

    // Component-wise device rollback floor (max per location across both slots).
    let vbs_idx = [
        vbs_info[0].as_ref().map(|i| i.rollback_index),
        vbs_info[1].as_ref().map(|i| i.rollback_index),
    ];
    let (boot_floor, vbs_floor) = rollback_floors(boot_idx, vbs_idx)
        .ok_or_else(|| "device boot/vbmeta_system AVB unreadable on both slots".to_string())?;

    Ok(DeviceVbmeta {
        slot,
        class,
        key_spec,
        boot_floor,
        vbs_floor,
    })
}

/// Back up the device's active-slot `abl` (the bootloader) for later restore.
/// `abl` is not in the static LUN map, so its LUN is found by GPT-scanning the
/// device. Returns `(lun, backup_path)` — the LUN holds both `abl_a`/`abl_b`,
/// and the restore later targets `abl_a` (firmware always lands on `_a`).
fn backup_device_abl(
    session: &mut ltbox_device::edl::EdlSession,
    slot: &str,
    work_dir: &std::path::Path,
    log: &mut Vec<String>,
) -> std::result::Result<(u8, std::path::PathBuf), String> {
    let parts = session
        .scan_partitions(0..=5, log)
        .map_err(|e| format!("scan partitions for abl: {e}"))?;
    let abl_part = format!("abl{slot}");
    let lun = parts
        .iter()
        .find(|p| p.name == "abl_a")
        .or_else(|| parts.iter().find(|p| p.name == abl_part))
        .map(|p| p.lun)
        .ok_or_else(|| "abl partition not found on device".to_string())?;
    let out = work_dir.join("dev_abl_backup.img");
    session
        .dump_partition(&abl_part, &out, 0, lun, log)
        .map_err(|e| format!("dump device {abl_part}: {e}"))?;
    Ok((lun, out))
}

/// Best-effort restore of the backed-up device `abl` onto `abl_a` for the flash
/// error paths: once the firmware's own (fixed-key) abl may have been written,
/// the original testkey abl must go back even on failure, or the device is left
/// with a fixed-key bootloader on a testkey-resigned chain. Failures here are
/// logged and swallowed — the caller is already returning an error and leaves
/// the device in EDL for retry.
fn restore_abl_best_effort(
    session: &mut ltbox_device::edl::EdlSession,
    abl_restore: &Option<(u8, std::path::PathBuf)>,
    log: &mut Vec<String>,
) {
    if let Some((lun, abl_img)) = abl_restore {
        ltbox_core::live!(
            log,
            "[ARB] {}",
            ltbox_core::i18n::tr("live_flash_abl_restore")
        );
        let _ = session.flash_partition("abl_a", abl_img, 0, *lun, log);
    }
}

pub(crate) fn flash_worker(
    cfg: WorkflowConfig,
    conn: ConnectionStatus,
    mut device_model: String,
    fw_folder: String,
    mut rb_mode: ltbox_patch::rollback::RollbackMode,
    ll: LiveLabels,
) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    let edl_start = matches!(conn, ConnectionStatus::Edl);
    let started_in_fastboot = matches!(conn, ConnectionStatus::Fastboot);
    let fw_dir = std::path::Path::new(&fw_folder);

    // 1. Validate firmware folder
    live!(log, "[Flash] {}", phase_marker(1, 4, &ll.op_flash_phase[0]));
    if !fw_dir.exists() {
        return Err(tr_args!(
            "err_flash_firmware_folder_missing",
            path = fw_folder
        ));
    }
    live!(
        log,
        "[Flash] {}",
        tr_args!("live_flash_firmware_folder", path = fw_folder)
    );

    // 2. Device detection
    //
    // Run the ADB device probe BEFORE the
    // Fastboot bridge below. The previous
    // ordering kicked off `adb reboot bootloader`
    // first and only then asked `AdbManager::
    // check_device`, by which point the device
    // had already detached from ADB — so the
    // detection block always logged "no ADB
    // device info" even when an ADB bridge
    // was sitting right there a second earlier.
    // Now device info gets collected on the live
    // ADB transport, and the bridge takes over
    // afterwards.
    let skip_adb = conn.skip_adb();
    if skip_adb {
        ltbox_core::live!(
            log,
            "[Flash] {}",
            ltbox_core::i18n::tr("live_flash_skip_adb")
        );
    } else {
        ltbox_core::live!(
            log,
            "[ADB] {}",
            ltbox_core::i18n::tr("live_adb_checking_device")
        );
        if ltbox_device::adb::AdbManager::new_if_connected().is_some() {
            ltbox_core::live!(
                log,
                "[ADB] {}",
                ltbox_core::i18n::tr("live_adb_device_connected")
            );
            // The active slot is resolved later via
            // `controller::poll_active_slot` — that
            // helper polls both ADB + Fastboot and
            // hard-errors on probe failure, so the
            // earlier `get_slot_suffix` round-trip
            // here was redundant (its result was
            // assigned to `_slot` and discarded).
        } else {
            ltbox_core::live!(
                log,
                "[ADB] {}",
                ltbox_core::i18n::tr("live_adb_no_device_info")
            );
        }
    }

    // Snapshot rollback index before EDL —
    // `stored_rollback_index` vanishes past
    // Fastboot. Probe Fastboot vars first, and
    // when the device is sitting in ADB bridge
    // it through `adb reboot bootloader` before
    // retrying — otherwise the user sees the
    // ARB=ON abort on every PRC↔ROW flash that
    // started from the ADB-connected state, even
    // though Fastboot is reachable in principle.
    let probe_fastboot = || -> (Option<u64>, bool, Option<String>, String) {
        match ltbox_device::fastboot::FastbootDevice::open() {
            Ok(mut dev) => match dev.get_all_vars() {
                Ok(v) => (
                    ltbox_patch::rollback::compute_device_rollback_index(&v.rollback_indices),
                    true,
                    v.current_slot.clone(),
                    v.raw_getvar_all.clone(),
                ),
                Err(_) => (None, false, None, String::new()),
            },
            Err(_) => (None, false, None, String::new()),
        }
    };
    let mut probe = probe_fastboot();
    let adb_connected = matches!(conn, ConnectionStatus::Adb | ConnectionStatus::AdbRecovery);
    if !probe.1 && adb_connected {
        ltbox_core::live!(
            log,
            "[Flash] {}",
            ltbox_core::i18n::tr("live_flash_adb_to_bootloader")
        );
        if let Some(mut adb) = ltbox_device::adb::AdbManager::new_if_connected() {
            match adb.reboot("bootloader") {
                Ok(()) => {
                    // Poll for Fastboot up to
                    // 60s — ADB→bootloader
                    // typically lands inside 8 s
                    // but cold boots can drag.
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
                    while std::time::Instant::now() < deadline {
                        if ltbox_device::fastboot::FastbootDevice::check_device() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                    probe = probe_fastboot();
                }
                Err(e) => {
                    ltbox_core::live!(
                        log,
                        "[ADB] {}",
                        tr_args!("live_adb_reboot_failed", error = e.to_string())
                    );
                }
            }
        }
    }
    let (device_rollback_index, fastboot_reachable, active_slot, getvar_raw) = probe;

    // 3. Scan firmware folder
    let vendor_boot = fw_dir.join("vendor_boot.img");
    let vbmeta = fw_dir.join("vbmeta.img");
    let vbmeta_system = fw_dir.join("vbmeta_system.img");
    let boot = fw_dir.join("boot.img");
    let has_vendor_boot = vendor_boot.exists();
    let has_vbmeta = vbmeta.exists();
    let has_vbmeta_system = vbmeta_system.exists();
    let has_boot = boot.exists();
    let found = ltbox_core::i18n::tr("live_status_found");
    let not_found = ltbox_core::i18n::tr("live_status_not_found");
    ltbox_core::live!(
        log,
        "[Flash] {}",
        tr_args!(
            "live_flash_vendor_boot_status",
            status = if has_vendor_boot { &found } else { &not_found },
        )
    );
    ltbox_core::live!(
        log,
        "[Flash] {}",
        tr_args!(
            "live_flash_vbmeta_status",
            status = if has_vbmeta { &found } else { &not_found },
        )
    );
    ltbox_core::live!(
        log,
        "[Flash] {}",
        tr_args!(
            "live_flash_boot_status",
            status = if has_boot { &found } else { &not_found },
        )
    );

    // Cross-check the firmware against the probed model before EDL via
    // vbmeta_system's build fingerprint (the unified identity source), and retain
    // it for SKU gates.
    let mut firmware_fingerprint: Option<String> = None;
    if has_vbmeta_system {
        match ltbox_patch::avb::extract_image_avb_info(&vbmeta_system) {
            Ok(info) => {
                // Pull the fingerprint up-front so the SKU gate below works on
                // EDL-start too — there `device_model` is empty and the validate
                // path would skip without populating it.
                let fp_prop = ltbox_patch::avb::build_fingerprint(&info);

                if edl_start {
                    firmware_fingerprint = fp_prop;
                } else {
                    use ltbox_patch::region::{ModelValidation, validate_device_model};
                    match validate_device_model(&info, &device_model) {
                        ModelValidation::Match { fingerprint } => {
                            ltbox_core::live!(
                                log,
                                "[Flash] {}",
                                ltbox_core::i18n::tr("live_rescue_model_check_ok")
                            );
                            firmware_fingerprint = Some(fingerprint);
                        }
                        ModelValidation::Missing => {
                            ltbox_core::live!(
                                log,
                                "[Flash] {}",
                                ltbox_core::i18n::tr("live_rescue_no_fingerprint_skip")
                            );
                            firmware_fingerprint = fp_prop;
                        }
                        ModelValidation::Mismatch {
                            fingerprint,
                            device_model,
                        } => {
                            ltbox_core::live!(
                                log,
                                "[Flash] {}",
                                tr_args!(
                                    "live_rescue_model_mismatch_abort",
                                    device = device_model,
                                    fingerprint = fingerprint
                                )
                            );
                            let err = ltbox_core::i18n::tr("err_flash_model_mismatch_pre_edl");
                            reboot_fastboot_to_system_after_pre_edl_abort(
                                &mut log,
                                started_in_fastboot,
                            );
                            return Err(err);
                        }
                    }
                }
            }
            Err(e) => {
                ltbox_core::live!(
                    log,
                    "[Flash] {}",
                    tr_args!("live_rescue_avb_inspect_skip", error = e.to_string())
                );
            }
        }
    }

    // TB323FU keeps region boot-chain conversion off (it
    // provisions a GBL on efisp instead) but DOES take ARB
    // overlays. Region detect uses fp first, then model.
    let tb323fu_skip_region = firmware_fingerprint
        .as_deref()
        .map(|fp| fingerprint_token_match(fp, "TB323FU"))
        .unwrap_or(false)
        || fingerprint_token_match(&device_model, "TB323FU");

    // GBL/ARB work follows the TARGET firmware identity
    // (vendor_boot fp), never the connected device.
    let target_is_tb323fu = firmware_fingerprint
        .as_deref()
        .map(|fp| fingerprint_token_match(fp, "TB323FU"))
        .unwrap_or(false);

    // EDL-start no longer forces rollback-bypass (or region) off. The device
    // model and committed rollback index are read by dumping vendor_boot +
    // boot + vbmeta_system from BOTH slots over EDL once the session is open
    // (see the `edl_start` block after `EdlSession::open` below), so the
    // user's selected rollback-bypass + region modes are preserved exactly as
    // on an ADB/bootloader start.

    // TB323FU must never run a blind ON: ON bumps even
    // matching indices and would force the testkey/_arb
    // chain when no downgrade is in play. Demote to AUTO so
    // the EDL-dumped device index decides per partition.
    if target_is_tb323fu && rb_mode == ltbox_patch::rollback::RollbackMode::On {
        rb_mode = ltbox_patch::rollback::RollbackMode::Auto;
        ltbox_core::live!(
            log,
            "[ARB] {}",
            ltbox_core::i18n::tr("live_flash_tb323fu_force_auto")
        );
    }
    if tb323fu_skip_region {
        ltbox_core::live!(
            log,
            "[Flash] {}",
            ltbox_core::i18n::tr("live_flash_tb323fu_region_efisp")
        );
    }

    // Rollback=ON + no fastboot vars → can't target a safe
    // index. Bail before EDL — UNLESS the device started in EDL, where the
    // index is read by dumping partitions over the open session (the
    // `edl_start` block after `EdlSession::open`). A bootloader/ADB start with
    // unreachable fastboot still has no index source, so it still aborts.
    if matches!(rb_mode, ltbox_patch::rollback::RollbackMode::On)
        && !fastboot_reachable
        && !edl_start
    {
        live!(
            log,
            "[ARB] {}",
            ltbox_core::i18n::tr("live_arb_on_fastboot_unreachable")
        );
        // Best-effort reboot — any failure stays
        // in the log; wizard still gets the Err.
        if let Some(mut adb) = ltbox_device::adb::AdbManager::new_if_connected() {
            if let Err(e) = adb.shell("reboot") {
                ltbox_core::live!(
                    log,
                    "[ADB] {}",
                    tr_args!("live_adb_reboot_failed", error = e.to_string())
                );
            } else {
                ltbox_core::live!(
                    log,
                    "[ADB] {}",
                    ltbox_core::i18n::tr("live_adb_reboot_sent")
                );
            }
        } else {
            ltbox_core::live!(
                log,
                "[ADB] {}",
                ltbox_core::i18n::tr("live_adb_no_reboot_route")
            );
        }
        return Err(ltbox_core::i18n::tr("err_rollback_on_fastboot_unreachable"));
    }

    // efisp GBL download is deferred until after the EDL
    // ARB dump decides `_arb` (testkey-root) vs stock — see
    // the post-rawprogram-staging block below.
    let mut efisp_efi: Option<std::path::PathBuf> = None;
    let mut tb323fu_arb_need = false;

    // Count .x and .xml files
    // Count flashable `.x` (rawprogram) files. The
    // encrypted Sahara manifest
    // (`qsahara_device_programmer.x`) is a loader, not a
    // flash image, so it is excluded here and left for
    // `EdlSession::open` to decrypt at load time.
    let x_count = std::fs::read_dir(fw_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension()
                        .map(|ext| ext.eq_ignore_ascii_case("x"))
                        .unwrap_or(false)
                        && !ltbox_core::sahara_xml::is_encrypted_manifest_filename(p)
                })
                .count()
        })
        .unwrap_or(0);
    let xml_count = std::fs::read_dir(fw_dir)
        .map(|rd| {
            rd.filter(|e| {
                e.as_ref()
                    .ok()
                    .map(|e| {
                        let p = e.path();
                        p.extension().map(|ext| ext == "xml").unwrap_or(false)
                            && p.file_name()
                                .map(|n| n.to_string_lossy().starts_with("rawprogram"))
                                .unwrap_or(false)
                    })
                    .unwrap_or(false)
            })
            .count()
        })
        .unwrap_or(0);
    ltbox_core::live!(
        log,
        "[Flash] {}",
        tr_args!(
            "live_flash_files_count",
            x_count = x_count.to_string(),
            xml_count = xml_count.to_string()
        )
    );

    // AVB root-of-trust pre-check (before region conversion, which only re-signs
    // via testkeys in KEY_MAP). Classify the firmware via vbmeta_system's pubkey
    // (the unified key source): an `Unknown` key aborts; a fixed-key ("key2")
    // firmware aborts on cross-region for now (same-region key2 is handled after
    // EDL opens; cross-region key2 re-sign is a separate change). TB323FU has its
    // own region path (efisp GBL) and is exempt here.
    let fw_key_class = match ltbox_patch::avb::extract_image_avb_info(&vbmeta_system) {
        Ok(info) => ltbox_patch::key_map::classify_pubkey(info.public_key_sha1.as_deref()),
        Err(_) => ltbox_patch::key_map::KeyClass::Unknown,
    };
    if fw_key_class == ltbox_patch::key_map::KeyClass::Unknown {
        ltbox_core::live!(
            log,
            "[AVB] {}",
            ltbox_core::i18n::tr("live_flash_vbmeta_key_unknown")
        );
        reboot_fastboot_to_system_after_pre_edl_abort(&mut log, started_in_fastboot);
        return Err(ltbox_core::i18n::tr("err_flash_vbmeta_key_unknown"));
    }
    if fw_key_class == ltbox_patch::key_map::KeyClass::Fixed
        && cfg.modify_region
        && !target_is_tb323fu
    {
        reboot_fastboot_to_system_after_pre_edl_abort(&mut log, started_in_fastboot);
        return Err(ltbox_core::i18n::tr(
            "err_flash_key2_cross_region_unsupported",
        ));
    }

    // 4. Region conversion
    let mut region_pair: Option<ltbox_patch::region::RegionBootChainOutput> = None;
    if cfg.modify_region && !tb323fu_skip_region {
        if has_vendor_boot && has_vbmeta {
            ltbox_core::live!(log, "[Region] {}", ltbox_core::i18n::tr("live_region_on"));
            ltbox_core::live!(
                log,
                "[Region] {}",
                ltbox_core::i18n::tr("live_region_ready")
            );
            let Some(device_region) = cfg.device_region else {
                let err = ltbox_core::i18n::tr("err_region_missing_device_region");
                reboot_fastboot_to_system_after_pre_edl_abort(&mut log, started_in_fastboot);
                return Err(err);
            };
            let target = device_region.to_region_target();
            let output_dir = ltbox_core::app_paths::auto_output_dir_for("region_convert");
            ltbox_core::live!(
                log,
                "[Region] {}",
                tr_args!(
                    "live_region_building_pair",
                    region = format!("{:?}", device_region)
                )
            );
            match ltbox_patch::region::build_region_converted_boot_chain(
                fw_dir,
                &output_dir,
                target,
                &ltbox_patch::region::RegionPatternSet::default(),
            ) {
                Ok(ltbox_patch::region::RegionBootChainBuild::Built(output)) => {
                    ltbox_core::live!(
                        log,
                        "[Region] {}",
                        tr_args!(
                            "live_region_source_target",
                            source = format!("{:?}", output.source_region),
                            target = format!("{:?}", output.target)
                        )
                    );
                    ltbox_core::live!(
                        log,
                        "[Region] {}",
                        tr_args!(
                            "live_region_patched",
                            count = output.replacement_count.to_string(),
                            path = output.vendor_boot.display().to_string()
                        )
                    );
                    ltbox_core::live!(
                        log,
                        "[Region] {}",
                        tr_args!(
                            "live_region_pair_rebuilt",
                            path = output.vbmeta.display().to_string()
                        )
                    );
                    region_pair = Some(output);
                }
                Ok(ltbox_patch::region::RegionBootChainBuild::Skipped {
                    source_region,
                    target,
                }) => {
                    ltbox_core::live!(
                        log,
                        "[Region] {}",
                        tr_args!(
                            "live_region_source_target",
                            source = format!("{:?}", source_region),
                            target = format!("{:?}", target)
                        )
                    );
                    ltbox_core::live!(
                        log,
                        "[Region] {}",
                        ltbox_core::i18n::tr("live_region_source_matches_target")
                    );
                }
                Err(e) => {
                    let err = tr_args!("err_region_conversion_failed", error = e.to_string());
                    reboot_fastboot_to_system_after_pre_edl_abort(&mut log, started_in_fastboot);
                    return Err(err);
                }
            }
        } else {
            ltbox_core::live!(
                log,
                "[Region] {}",
                ltbox_core::i18n::tr("live_region_missing_skip")
            );
        }
    }

    // 5. ARB detection. The effective rollback mode is already surfaced by
    // the `[Flash] Bypass rollback protection: …` summary line, so it is not
    // repeated here; this block reports the measured indices + the final
    // bypass decision.
    let device_idx_str = device_rollback_index
        .map(|v| v.to_string())
        .unwrap_or_else(|| ltbox_core::i18n::tr("live_arb_device_index_none"));
    ltbox_core::live!(
        log,
        "[ARB] {}",
        tr_args!("live_arb_device_index", index = device_idx_str)
    );
    if has_boot && !edl_start {
        // Pre-result "Analyzing …" line dropped — analysis is
        // synchronous and the result line ("boot.img rollback
        // index: …") fires immediately after. Skipped on EDL-start: the
        // device index is unknown until the post-open both-slot dump, so a
        // pre-EDL summary here would print a misleading "bypass: no".
        match ltbox_patch::rollback::analyze_rollback_with_mode(
            &boot,
            device_rollback_index,
            rb_mode,
        ) {
            Ok(info) => {
                ltbox_core::live!(
                    log,
                    "[ARB] {}",
                    tr_args!(
                        "live_arb_boot_index_result",
                        index = info.image_index.to_string()
                    )
                );
                ltbox_core::live!(
                    log,
                    "[ARB] {}",
                    tr_args!(
                        "live_arb_rollback_bypass",
                        value = ltbox_core::i18n::tr(if info.needs_patch {
                            "common_yes"
                        } else {
                            "common_no"
                        })
                    )
                );
            }
            Err(e) => ltbox_core::live!(
                log,
                "[ARB] {}",
                tr_args!("live_arb_boot_analysis_failed", error = e.to_string())
            ),
        }
    }
    // ARB analysis above is diagnostic only — flash plan unchanged.

    // 6. XML
    //
    // Decrypt every `.x` in place — output sits next
    // to its source as `<stem>.xml` so the catalog
    // scan below picks it up and the EDL flash can
    // still resolve image paths via `xml_dir.join`.
    if x_count > 0 {
        let x_entries: Vec<std::path::PathBuf> = std::fs::read_dir(fw_dir)
            .map_err(|e| {
                tr_args!(
                    "err_read_dir_failed",
                    path = fw_dir.display().to_string(),
                    error = e.to_string()
                )
            })?
            .filter_map(|r| r.ok().map(|e| e.path()))
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|s| s.to_str())
                        .map(|s| s.eq_ignore_ascii_case("x"))
                        .unwrap_or(false)
                    && !ltbox_core::sahara_xml::is_encrypted_manifest_filename(p)
            })
            .collect();
        let mut decrypted = 0usize;
        for src in &x_entries {
            let stem = src.file_stem().unwrap_or_default();
            let output = fw_dir.join(stem).with_extension("xml");
            ltbox_core::crypto::decrypt_file(src, &output).map_err(|e| {
                tr_args!(
                    "err_decrypt_file_failed",
                    path = src.display().to_string(),
                    error = e.to_string()
                )
            })?;
            decrypted += 1;
        }
        ltbox_core::live!(
            log,
            "[XML] {}",
            tr_args!("live_xml_decrypt_done", count = decrypted.to_string())
        );
    }
    if !cfg.wipe && xml_count > 0 {
        ltbox_core::live!(
            log,
            "[XML] {}",
            ltbox_core::i18n::tr("live_xml_keep_excludes")
        );
    }

    // 7. Country code
    if cfg.wipe {
        ltbox_core::live!(
            log,
            "[Flash] {}",
            ltbox_core::i18n::tr("live_flash_data_mode_wipe")
        );
        if let Some(cc) = cfg.country_action.target() {
            ltbox_core::live!(
                log,
                "[Flash] {}",
                tr_args!("live_flash_country_devinfo", code = cc)
            );
        } else if cfg.country_action.is_skipped() {
            ltbox_core::live!(
                log,
                "[Flash] {}",
                ltbox_core::i18n::tr("live_flash_country_skip")
            );
        }
    }

    // 8. EDL flash
    let loader = find_edl_loader(fw_dir).or_else(|| fw_dir.parent().and_then(find_edl_loader));
    let loader = match loader {
        Some(l) => l,
        None => {
            ltbox_core::live!(
                log,
                "[EDL] {}",
                ltbox_core::i18n::tr("live_edl_loader_missing")
            );
            return Ok(log);
        }
    };

    live!(log, "[Flash] {}", phase_marker(2, 4, &ll.op_flash_phase[1]));
    transition_to_edl(conn, &ll, &mut log)?;

    let mut session = ltbox_device::edl::EdlSession::open(&loader, true, &mut log)
        .map_err(|e| tr_args!("err_edl_session_open_failed", error = e.to_string()))?;

    // EDL-start: fastboot/ADB never ran, so the device model + committed
    // rollback index are unknown. Read them off the device by dumping BOTH
    // slots over the open session — the active slot is unknown in EDL-start,
    // and the inactive slot's images may not carry parseable AVB info.
    // vendor_boot identifies the model (compared against the target firmware
    // fingerprint); boot + vbmeta_system give the rollback floor. If neither
    // slot yields a valid AVB image, reset back into EDL and abort rather than
    // flash blind. TB322FC has no rollback protection → bypass forced Off.
    // On EDL-start, the per-location rollback floors (component-wise max across
    // both slots) feed both the generic ARB overlay loop and TB323FU's overlay
    // builder below — each location keeps its own floor.
    let mut edl_floors: Option<(u64, u64)> = None;
    if edl_start {
        match read_edl_start_device(&mut session, firmware_fingerprint.as_deref(), &mut log) {
            Ok(probe) => {
                device_model = probe.model_token;
                match probe.rollback_floors {
                    None => {
                        // TB322FC is PRC-only. The pre-EDL UI gates that block
                        // cross-region / non-CN country never fired (the model
                        // was unknown until now), so enforce the constraint
                        // here, before any region or country write.
                        let non_cn_country = cfg
                            .country_action
                            .target()
                            .map(|c| !c.eq_ignore_ascii_case("CN"))
                            .unwrap_or(false);
                        if cfg.modify_region || non_cn_country {
                            let _ = session.reset_to_edl(&mut log);
                            return Err(ltbox_core::i18n::tr("err_flash_tb322fc_prc_only"));
                        }
                        rb_mode = ltbox_patch::rollback::RollbackMode::Off;
                        ltbox_core::live!(
                            log,
                            "[ARB] {}",
                            ltbox_core::i18n::tr("live_arb_device_index_none")
                        );
                    }
                    Some(floors) => {
                        // Component-wise floors flow per location into both the
                        // generic overlay loop and the TB323FU builder; never
                        // collapse to a single max (that would inflate the
                        // lower location and block future stock firmware).
                        edl_floors = Some(floors);
                    }
                }
            }
            Err(e) => {
                let _ = session.reset_to_edl(&mut log);
                return Err(e);
            }
        }
    }

    // Full-firmware flash: rawprogram + patch XMLs
    // drive every program node (no slot guessing).
    let (raw_xmls, patch_xmls) = ltbox_device::edl::collect_firmware_xmls_for_flash(fw_dir, false)
        .map_err(|e| tr_args!("err_flash_xml_selection_failed", error = e.to_string()))?;
    if raw_xmls.is_empty() {
        return Err(tr_args!("err_flash_no_rawprogram_xml", path = fw_folder));
    }
    // Stage ARB copies; flash them after rawprogram.
    let mut arb_patched: Vec<(String, u8, std::path::PathBuf)> = Vec::new();
    // abl (bootloader) backup to overlay-restore onto abl_a after the flash —
    // set only for the testkey-device + fixed-firmware re-sign case below.
    let mut abl_restore: Option<(u8, std::path::PathBuf)> = None;

    // AVB root-of-trust gate (device side). The firmware vbmeta was already
    // classified before region conversion (`fw_key_class`): `Unknown` and a
    // cross-region `Fixed` firmware aborted there; `Testkey` firmware and a
    // `Fixed` firmware on TB323FU take their existing paths. Here a same-region
    // `Fixed` ("key2") firmware on any other model classifies the device's
    // active-slot vbmeta and either proceeds as-is (fixed device, no downgrade),
    // re-signs to the testkey + preserves the device bootloader (testkey
    // device), or aborts.
    if fw_key_class == ltbox_patch::key_map::KeyClass::Fixed && !target_is_tb323fu {
        let kc_dir = ltbox_core::app_paths::work_dir_for("flash_keyclass");
        let _ = std::fs::remove_dir_all(&kc_dir);
        std::fs::create_dir_all(&kc_dir).map_err(|e| format!("keyclass work dir: {e}"))?;
        let dev = match read_device_vbmeta(&mut session, active_slot.as_deref(), &kc_dir, &mut log)
        {
            Ok(d) => d,
            Err(e) => {
                if edl_start {
                    let _ = session.reset_to_edl(&mut log);
                } else {
                    let _ = session.reset(&mut log);
                }
                return Err(e);
            }
        };
        match dev.class {
            ltbox_patch::key_map::KeyClass::Unknown => {
                if edl_start {
                    let _ = session.reset_to_edl(&mut log);
                } else {
                    let _ = session.reset(&mut log);
                }
                return Err(ltbox_core::i18n::tr("err_flash_device_key_unknown"));
            }
            ltbox_patch::key_map::KeyClass::Fixed => {
                // Fixed device + fixed firmware: the bootloader enforces region
                // + rollback, so only a same-region, non-downgrade install can
                // proceed (no re-sign, flash as-is).
                let fw_boot_idx = ltbox_patch::avb::extract_image_avb_info(&boot)
                    .map(|i| i.rollback_index)
                    .unwrap_or(0);
                let fw_vbs_idx =
                    ltbox_patch::avb::extract_image_avb_info(&fw_dir.join("vbmeta_system.img"))
                        .map(|i| i.rollback_index)
                        .unwrap_or(0);
                let downgrade = fw_boot_idx < dev.boot_floor || fw_vbs_idx < dev.vbs_floor;
                if cfg.modify_region || downgrade {
                    if edl_start {
                        let _ = session.reset_to_edl(&mut log);
                    } else {
                        let _ = session.reset(&mut log);
                    }
                    return Err(ltbox_core::i18n::tr("err_flash_key2_device_constraint"));
                }
                ltbox_core::live!(
                    log,
                    "[AVB] {}",
                    ltbox_core::i18n::tr("live_flash_key2_proceed")
                );
                rb_mode = ltbox_patch::rollback::RollbackMode::Off;
            }
            ltbox_patch::key_map::KeyClass::Testkey => {
                // The re-sign path supports only the 4096 testkey; a device on a
                // different testkey (e.g. 2048) would reject a 4096-re-signed
                // chain, so abort rather than risk a brick.
                if dev.key_spec != Some("testkey_rsa4096") {
                    if edl_start {
                        let _ = session.reset_to_edl(&mut log);
                    } else {
                        let _ = session.reset(&mut log);
                    }
                    return Err(ltbox_core::i18n::tr("err_flash_device_testkey_unsupported"));
                }
                // Testkey device + fixed firmware: re-sign the install to the
                // testkey root the device trusts, and preserve the device's own
                // abl — the firmware's abl would re-root the chain to the fixed
                // key and reject the re-signed images.
                ltbox_core::live!(
                    log,
                    "[AVB] {}",
                    ltbox_core::i18n::tr("live_flash_key2_resign")
                );
                let arb_work_dir = ltbox_core::app_paths::work_dir_for("flash_arb");
                let _ = std::fs::remove_dir_all(&arb_work_dir);
                std::fs::create_dir_all(&arb_work_dir).map_err(|e| format!("arb work dir: {e}"))?;
                let (overlays, _need) = build_tb323fu_arb_overlays(
                    &mut session,
                    fw_dir,
                    &arb_work_dir,
                    Some(dev.slot),
                    Some((dev.boot_floor, dev.vbs_floor)),
                    true,
                    &mut log,
                )?;
                arb_patched = overlays;
                match backup_device_abl(&mut session, dev.slot, &arb_work_dir, &mut log) {
                    Ok(backup) => abl_restore = Some(backup),
                    Err(e) => {
                        if edl_start {
                            let _ = session.reset_to_edl(&mut log);
                        } else {
                            let _ = session.reset(&mut log);
                        }
                        return Err(e);
                    }
                }
                rb_mode = ltbox_patch::rollback::RollbackMode::Off;
            }
        }
    }
    if rb_mode != ltbox_patch::rollback::RollbackMode::Off && target_is_tb323fu {
        // TB323FU stages the testkey chain whenever the
        // install is a downgrade, independent of region /
        // wipe: the matching `_arb` GBL is flashed to efisp
        // below in the exact same `need` cases, so the chain
        // and its root of trust stay paired. fastboot never
        // exposes the index, so dump it over EDL, testkey
        // re-sign the four AVB partitions and stage overlays
        // (or flash stock when not a downgrade).
        let arb_work_dir = ltbox_core::app_paths::work_dir_for("flash_arb");
        let _ = std::fs::remove_dir_all(&arb_work_dir);
        std::fs::create_dir_all(&arb_work_dir).map_err(|e| format!("arb work dir: {e}"))?;
        let (overlays, need) = build_tb323fu_arb_overlays(
            &mut session,
            fw_dir,
            &arb_work_dir,
            active_slot.as_deref(),
            edl_floors,
            false,
            &mut log,
        )?;
        tb323fu_arb_need = need;
        arb_patched = overlays;
    } else if rb_mode != ltbox_patch::rollback::RollbackMode::Off {
        let arb_work_dir = ltbox_core::app_paths::work_dir_for("flash_arb");
        let _ = std::fs::remove_dir_all(&arb_work_dir);
        std::fs::create_dir_all(&arb_work_dir).map_err(|e| format!("arb work dir: {e}"))?;

        // Per-location device rollback floors. On EDL-start we already read
        // component-wise maxima from BOTH slots; apply each location's own floor
        // to its partition so a higher boot floor never inflates the
        // vbmeta_system location — which would block later stock firmware — and
        // vice versa. Otherwise fall back to the single aggregate index
        // (fastboot, or the active-slot EDL read) applied to every location, as
        // before. TB322FC keeps `None` → the per-partition `else` below skips
        // patching, which is correct since it has no rollback floor.
        let (boot_floor, vbs_floor) = match edl_floors {
            Some((b, v)) => (Some(b), Some(v)),
            None => {
                let idx = match device_rollback_index {
                    Some(i) => Some(i),
                    None if is_rollback_protected_model(&device_model) => {
                        ltbox_core::live!(
                            log,
                            "[ARB] {}",
                            ltbox_core::i18n::tr("live_arb_edl_dump")
                        );
                        Some(read_device_rollback_index_via_edl(
                            &mut session,
                            active_slot.as_deref(),
                            &arb_work_dir,
                            &mut log,
                        )?)
                    }
                    None => None,
                };
                (idx, idx)
            }
        };

        // (base, on-disk filename, slot label, device floor for this location)
        let label_pairs: [(&str, &str, &str, Option<u64>); 2] = [
            ("boot", "boot.img", "boot_a", boot_floor),
            (
                "vbmeta_system",
                "vbmeta_system.img",
                "vbmeta_system_a",
                vbs_floor,
            ),
        ];
        for (log_name, filename, slot_label, loc_floor) in label_pairs {
            let Some(lun) = ltbox_core::partition_lun::lun_for_partition(log_name) else {
                ltbox_core::live!(
                    log,
                    "[ARB] {}",
                    tr_args!("live_arb_skip_no_lun", name = log_name)
                );
                continue;
            };
            let source = fw_dir.join(filename);
            if !source.exists() {
                ltbox_core::live!(
                    log,
                    "[ARB] {}",
                    tr_args!(
                        "live_arb_skip_image_missing",
                        name = log_name,
                        file = source.display().to_string()
                    )
                );
                continue;
            }

            // `Off` is already bypassed; On or Auto here.
            let analysis = match ltbox_patch::rollback::analyze_rollback_with_mode(
                &source, loc_floor, rb_mode,
            ) {
                Ok(a) => a,
                Err(e) => {
                    ltbox_core::live!(
                        log,
                        "[ARB] {}",
                        tr_args!(
                            "live_arb_analyze_failed",
                            name = log_name,
                            error = e.to_string()
                        )
                    );
                    continue;
                }
            };
            ltbox_core::live!(
                log,
                "[ARB] {}",
                tr_args!(
                    "live_arb_image_status",
                    name = log_name,
                    image = analysis.image_index.to_string(),
                    needs = analysis.needs_patch.to_string()
                )
            );
            if !analysis.needs_patch {
                continue;
            }
            let Some(target) = loc_floor else {
                ltbox_core::live!(
                    log,
                    "[ARB] {}",
                    tr_args!("live_arb_skip_unknown_device", name = log_name)
                );
                continue;
            };

            // Non-TB323FU rollback bypass only supports stock keys in KEY_MAP.
            // TB323FU is handled above by the GBL/efisp path.
            let key_from_map = match ltbox_patch::key_map::key_spec_for_signed_pubkey(
                analysis.image_info.public_key_sha1.as_deref(),
            ) {
                Ok(spec) => spec,
                Err(sha) => {
                    let err = tr_args!("err_avb_signing_key_unknown", image = log_name, key = sha);
                    ltbox_core::live!(log, "[ARB] {err}");
                    session.reset_tolerant(&mut log);
                    return Err(err);
                }
            };

            let patched = arb_work_dir.join(format!("{log_name}.arb.img"));
            let is_vbmeta = log_name.starts_with("vbmeta");
            let patch_result = if is_vbmeta {
                // vbmeta always resigns (no add_hash_footer).
                match key_from_map {
                    Some(spec) => {
                        std::fs::copy(&source, &patched)
                            .map_err(|e| format!("copy vbmeta: {e}"))?;
                        ltbox_patch::avb::resign_image(
                            &patched,
                            spec,
                            &analysis.image_info.algorithm,
                            Some(target),
                        )
                        .map_err(|e| format!("resign {log_name}: {e}"))
                    }
                    None => {
                        ltbox_core::live!(
                            log,
                            "[ARB] {}",
                            tr_args!("live_arb_unsigned_skip_resign", name = log_name)
                        );
                        continue;
                    }
                }
            } else if analysis.image_info.algorithm == "NONE" {
                std::fs::copy(&source, &patched).map_err(|e| format!("copy chained: {e}"))?;
                ltbox_patch::avb::add_hash_footer(
                    &patched,
                    &analysis.image_info,
                    key_from_map,
                    Some(target),
                )
                .map_err(|e| format!("patch {log_name}: {e}"))
            } else if let Some(spec) = key_from_map {
                std::fs::copy(&source, &patched).map_err(|e| format!("copy chained: {e}"))?;
                ltbox_patch::avb::resign_image(
                    &patched,
                    spec,
                    &analysis.image_info.algorithm,
                    Some(target),
                )
                .map_err(|e| format!("resign {log_name}: {e}"))
            } else {
                ltbox_core::live!(
                    log,
                    "[ARB] {}",
                    tr_args!("live_arb_unsigned_skip_resign", name = log_name)
                );
                continue;
            };
            if let Err(e) = patch_result {
                ltbox_core::live!(
                    log,
                    "[ARB] {}",
                    tr_args!(
                        "live_arb_patch_failed",
                        name = log_name,
                        error = e.to_string()
                    )
                );
                continue;
            }

            live!(
                log,
                "[ARB] {}",
                tr_args!(
                    "live_arb_prepared_patch",
                    name = log_name,
                    path = patched.display().to_string(),
                    target = target.to_string()
                )
            );
            arb_patched.push((slot_label.to_string(), lun, patched));
        }
    }

    // Download the efisp GBL now that the ARB dump has
    // decided stock vs `_arb` (testkey-root). The `_arb`
    // GBL is fetched whenever a downgrade re-signed the
    // chain; the normal GBL is fetched for a cross-region
    // ("Other region") provisioning install. Neither is
    // gated on data wipe — flashing efisp no longer forces
    // a data reset, so it provisions in data-keep mode too.
    // Both flash below.
    if target_is_tb323fu && (tb323fu_arb_need || cfg.modify_region) {
        // TB323FU's AVB fingerprint carries no region token; read the region
        // from the firmware vendor_boot's `product_region` DTB marker instead.
        let is_prc = ltbox_patch::region::detect_product_region(&vendor_boot)
            == Some(ltbox_patch::region::RegionTarget::Prc);
        let suffix = efisp_asset_suffix(is_prc, tb323fu_arb_need);
        ltbox_core::live!(
            log,
            "[Flash] {}",
            tr_args!("live_flash_efisp_fetch", variant = suffix)
        );
        let gh = ltbox_core::github::GitHubClient::from_url("github.com/miner7222/gbl_root_canoe")
            .map_err(|e| format!("efisp EFI: GitHub client: {e}"))?;
        let (asset_name, asset_url) = gh
            .latest_release_asset_where(|n| n.to_ascii_lowercase().ends_with(suffix))
            .map_err(|e| {
                format!("efisp EFI: no '{suffix}' asset on latest gbl_root_canoe release: {e}")
            })?;
        let efi_dir = ltbox_core::app_paths::work_dir_for("flash_efisp");
        let _ = std::fs::remove_dir_all(&efi_dir);
        std::fs::create_dir_all(&efi_dir).map_err(|e| format!("efisp EFI work dir: {e}"))?;
        let efi_path = efi_dir.join(&asset_name);
        ltbox_core::downloader::download_to_file(&asset_url, &efi_path, &mut log)
            .map_err(|e| format!("efisp EFI: download '{asset_name}' failed: {e}"))?;
        ltbox_core::live!(
            log,
            "[Flash] {}",
            tr_args!("live_flash_efisp_fetched", name = asset_name)
        );
        efisp_efi = Some(efi_path);
    }

    live!(
        log,
        "[Flash] {} ({})",
        phase_marker(3, 4, &ll.op_flash_phase[2]),
        tr_args!(
            "live_flash_phase3_xml_counts",
            raw = raw_xmls.len().to_string(),
            patch = patch_xmls.len().to_string()
        )
    );
    // ABL preservation is brick-critical once the firmware's own (fixed-key) abl
    // can land: if rawprogram or an ARB overlay fails after that point, the
    // original testkey abl must still go back, or the device is left with a
    // fixed-key bootloader on a testkey-resigned chain. Restore best-effort on
    // those error paths (device stays in EDL for retry); the success-path
    // restore below stays fatal.
    if let Err(e) = session.flash_rawprogram_with_wipe(&raw_xmls, &patch_xmls, cfg.wipe, &mut log) {
        let err = tr_args!("err_flash_firmware_failed", error = e.to_string());
        restore_abl_best_effort(&mut session, &abl_restore, &mut log);
        return Err(err);
    }

    // Overlay ARB-patched boot/vbmeta_system by GPT name.
    for (label, lun, patched) in &arb_patched {
        live!(
            log,
            "[ARB] {}",
            tr_args!("live_arb_flash_patched", label = label)
        );
        if let Err(e) = session.flash_partition(label, patched, 0, *lun, &mut log) {
            let err = tr_args!(
                "err_flash_arb_partition_failed",
                label = label,
                error = e.to_string()
            );
            restore_abl_best_effort(&mut session, &abl_restore, &mut log);
            return Err(err);
        }
    }

    // Restore the device's original bootloader on abl_a (testkey-fixed firmware
    // re-signed for a testkey device). The firmware's own abl would re-root the
    // chain to the fixed key and reject the re-signed images; a failed restore
    // leaves the device in EDL rather than resetting into that mismatch.
    if let Some((lun, abl_img)) = &abl_restore {
        live!(
            log,
            "[ARB] {}",
            ltbox_core::i18n::tr("live_flash_abl_restore")
        );
        if let Err(e) = session.flash_partition("abl_a", abl_img, 0, *lun, &mut log) {
            return Err(tr_args!(
                "err_flash_abl_restore_failed",
                error = e.to_string()
            ));
        }
    }

    // efisp GBL, flashed immediately after the ARB overlays
    // so the testkey chain and its `_arb` root of trust are
    // provisioned together, before the best-effort region /
    // country work that can abort in between. A fetched EFI
    // (Some) is flashed: the `_arb` variant whenever a
    // downgrade re-signed the chain — fatal on failure since
    // that chain can't boot without it — or the normal
    // variant on a region-provisioning wipe (best-effort).
    // With no EFI fetched, a same-region wipe strips efisp;
    // every other mode leaves it untouched.
    if target_is_tb323fu {
        let efisp_lun = ltbox_core::partition_lun::lun_for_partition("efisp").unwrap_or(4);
        match &efisp_efi {
            Some(efi) => {
                ltbox_core::live!(
                    log,
                    "[Flash] {}",
                    ltbox_core::i18n::tr("live_flash_efisp_flash")
                );
                if let Err(e) = session.flash_partition("efisp", efi, 0, efisp_lun, &mut log) {
                    ltbox_core::live!(
                        log,
                        "[Flash] {}",
                        tr_args!("live_flash_efisp_flash_failed", error = e.to_string())
                    );
                    // A staged testkey ARB chain only boots
                    // with this `_arb` GBL. Abort loudly
                    // (device stays in EDL for retry) rather
                    // than resetting into a rollback brick.
                    if tb323fu_arb_need {
                        return Err(tr_args!(
                            "err_flash_efisp_arb_failed",
                            error = e.to_string()
                        ));
                    }
                } else {
                    ltbox_core::live!(
                        log,
                        "[Flash] {}",
                        ltbox_core::i18n::tr("live_flash_efisp_flashed")
                    );
                }
            }
            None => {
                // A downgrade always fetches the `_arb` GBL,
                // so reaching here with `need` set is an
                // internal inconsistency — fail safe.
                if tb323fu_arb_need {
                    return Err(ltbox_core::i18n::tr("err_flash_efisp_arb_missing"));
                }
                // Same-region wipe with no downgrade strips
                // the GBL; other modes leave efisp as-is.
                if cfg.wipe && !cfg.modify_region {
                    ltbox_core::live!(
                        log,
                        "[Flash] {}",
                        ltbox_core::i18n::tr("live_flash_efisp_erase")
                    );
                    if let Err(e) = session.erase_partition_by_name("efisp", 0, efisp_lun, &mut log)
                    {
                        ltbox_core::live!(
                            log,
                            "[Flash] {}",
                            tr_args!("live_flash_efisp_erase_failed", error = e.to_string())
                        );
                    } else {
                        ltbox_core::live!(
                            log,
                            "[Flash] {}",
                            ltbox_core::i18n::tr("live_flash_efisp_erased")
                        );
                    }
                }
            }
        }
    }

    // Overwrite rawprogram's stock vendor_boot/vbmeta
    // with the final region-converted AVB-valid pair.
    // This must happen after rawprogram (and after any
    // ARB overlays) so stock XML entries cannot put the
    // unconverted ROW pair back on top.
    if let Some(output) = &region_pair {
        let overlays: [(&str, &std::path::Path); 2] = [
            ("vendor_boot_a", output.vendor_boot.as_path()),
            ("vbmeta_a", output.vbmeta.as_path()),
        ];
        for (label, image) in overlays {
            let Some(lun) = ltbox_core::partition_lun::lun_for_partition(label) else {
                return Err(tr_args!("err_region_flash_no_lun", label = label));
            };
            live!(
                log,
                "[Region] {}",
                tr_args!(
                    "live_region_flashing_final",
                    label = label,
                    path = image.display().to_string()
                )
            );
            if let Err(e) = session.flash_partition(label, image, 0, lun, &mut log) {
                return Err(tr_args!(
                    "err_region_flash_failed",
                    label = label,
                    error = e.to_string()
                ));
            }
        }
    }

    // Country-code patch is best-effort after firmware flash.
    // TB320FC/TB323FU use oemowninfo; others use devinfo.
    if cfg.wipe
        && let Some(target_code) = cfg.country_action.target()
    {
        live!(
            log,
            "[Flash] {}",
            tr_args!("live_flash_country_patch_target", target = target_code)
        );
        let work_dir = ltbox_core::app_paths::work_dir_for("flash_country");
        let _ = std::fs::remove_dir_all(&work_dir);
        if let Err(e) = std::fs::create_dir_all(&work_dir) {
            return Err(tr_args!(
                "err_country_work_dir_failed",
                error = e.to_string()
            ));
        }
        // Keep original region partitions for manual restore.
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let critical_backup =
            ltbox_core::app_paths::backup_dir_for(&format!("backup_critical_{ts}"));
        std::fs::create_dir_all(&critical_backup)
            .map_err(|e| tr_args!("err_country_backup_dir_failed", error = e.to_string()))?;
        // Stash the bootloader's `getvar all` (incl. serialno) next to the
        // backed-up partitions — revives + supersedes v2's `sn.txt`. Empty on
        // an EDL-start flash (no fastboot probe); best-effort, never fatal.
        if !getvar_raw.is_empty() {
            let _ = std::fs::write(critical_backup.join("getvar.txt"), &getvar_raw);
        }
        // devinfo + persist resolve through the hardcoded
        // LUN map; start/num come from the device GPT via
        // `dump_partition_by_name`. Avoids re-decrypting
        // `rawprogram*.x` mid-flow when the catalog scratch
        // dir has been cleaned.
        use ltbox_patch::region::{
            EU_COUNTRY_CODES as EU_CODES, KNOWN_COUNTRY_CODES as KNOWN_CODES,
        };
        // TB320FC / TB323FU keep the country code in
        // `oemowninfo` (LUN 0), not `devinfo` (LUN 4).
        // Detect via the vendor_boot fingerprint (works
        // on EDL-start) or the probe-reported model.
        let oemowninfo_sku = ["TB320FC", "TB323FU"].iter().any(|m| {
            firmware_fingerprint
                .as_deref()
                .map(|fp| fingerprint_token_match(fp, m))
                .unwrap_or(false)
                || fingerprint_token_match(&device_model, m)
        });
        let country_label = if oemowninfo_sku {
            "oemowninfo"
        } else {
            "devinfo"
        };
        // TB323FU keeps /persist as dump + backup only —
        // no country-code patch, no fingerprint edit, no
        // re-flash. Only oemowninfo is modified + flashed.
        let tb323fu_persist_backup_only = firmware_fingerprint
            .as_deref()
            .map(|fp| fingerprint_token_match(fp, "TB323FU"))
            .unwrap_or(false)
            || fingerprint_token_match(&device_model, "TB323FU");
        let mut country_progress = CountryPatchProgress::new(&[country_label, "persist"]);
        for label in [country_label, "persist"] {
            let Some(lun) = ltbox_core::partition_lun::lun_for_partition(label) else {
                let reason = "no hardcoded LUN for label";
                ltbox_core::live!(
                    log,
                    "[Country] {}",
                    tr_args!(
                        "live_country_partition_status",
                        label = label,
                        reason = reason
                    )
                );
                country_progress.mark_failed(label, reason);
                continue;
            };
            let dump_path = work_dir.join(format!("{label}.img"));
            live!(
                log,
                "[Country] {}",
                tr_args!(
                    "live_country_dump_partition",
                    label = label,
                    lun = lun.to_string(),
                    start = "?",
                    sectors = "?"
                )
            );
            if let Err(e) = session.dump_partition(label, &dump_path, 0, lun, &mut log) {
                let reason = format!("dump failed: {e}");
                ltbox_core::live!(
                    log,
                    "[Country] {}",
                    tr_args!(
                        "live_country_partition_status",
                        label = label,
                        reason = reason
                    )
                );
                country_progress.mark_failed(label, reason);
                continue;
            }
            // Backup before any patch touches it.
            if let Err(e) = std::fs::copy(&dump_path, critical_backup.join(format!("{label}.img")))
            {
                let reason = format!("backup failed: {e}");
                ltbox_core::live!(
                    log,
                    "[Country] {}",
                    tr_args!(
                        "live_country_partition_status",
                        label = label,
                        reason = reason
                    )
                );
                country_progress.mark_failed(label, reason);
                continue;
            }
            // TB323FU: /persist work stops at dump + backup.
            if label == "persist" && tb323fu_persist_backup_only {
                live!(
                    log,
                    "[Country] {}",
                    ltbox_core::i18n::tr("live_country_persist_backup_only")
                );
                country_progress.mark_flashed(label);
                continue;
            }
            let detected = match ltbox_patch::region::detect_country_code(&dump_path, KNOWN_CODES) {
                Ok(c) => c,
                Err(e) => {
                    let reason = format!("detect failed: {e}");
                    ltbox_core::live!(
                        log,
                        "[Country] {}",
                        tr_args!(
                            "live_country_partition_status",
                            label = label,
                            reason = reason
                        )
                    );
                    country_progress.mark_failed(label, reason);
                    None
                }
            };
            let patched_path = work_dir.join(format!("{label}.patched.img"));
            // Patch the country code when the partition carries
            // one. `persist` has no real country code (its only
            // matches live in captured logs), so it is a no-op
            // pass-through here — dump + backup already happened.
            let mut changed = false;
            match detected {
                Some(ref old_code) => {
                    live!(
                        log,
                        "[Country] {}",
                        tr_args!(
                            "live_country_patch_transition",
                            label = label,
                            from = old_code,
                            to = target_code
                        )
                    );
                    match ltbox_patch::region::patch_country_code(
                        &dump_path,
                        &patched_path,
                        old_code,
                        target_code,
                        EU_CODES,
                    ) {
                        Ok(c) => changed |= c,
                        Err(e) => {
                            let reason = format!("patch failed: {e}");
                            ltbox_core::live!(
                                log,
                                "[Country] {}",
                                tr_args!(
                                    "live_country_partition_status",
                                    label = label,
                                    reason = reason
                                )
                            );
                            country_progress.mark_failed(label, reason);
                            continue;
                        }
                    }
                }
                None => {
                    if label != "persist" {
                        let reason = "no known code detected";
                        ltbox_core::live!(
                            log,
                            "[Country] {}",
                            tr_args!(
                                "live_country_partition_status",
                                label = label,
                                reason = reason
                            )
                        );
                        country_progress.mark_failed(label, reason);
                        continue;
                    }
                    // persist has no country code — nothing to
                    // patch; `changed` stays false and it is
                    // marked handled in the pass-through below.
                }
            }

            // Flash once if the country code changed.
            if changed {
                if let Err(e) = session.flash_partition(label, &patched_path, 0, lun, &mut log) {
                    ltbox_core::live!(
                        log,
                        "[Country] {}",
                        tr_args!(
                            "live_country_flash_failed",
                            label = label,
                            error = e.to_string()
                        )
                    );
                    country_progress.mark_failed(label, format!("flash failed: {e}"));
                } else {
                    live!(
                        log,
                        "[Country] {}",
                        tr_args!("live_country_patched_flashed", label = label)
                    );
                    country_progress.mark_flashed(label);
                }
            } else if detected.as_deref() == Some(target_code) {
                ltbox_core::live!(
                    log,
                    "[Country] {}",
                    tr_args!(
                        "live_country_partition_already",
                        label = label,
                        target = target_code
                    )
                );
                country_progress.mark_flashed(label);
            } else if label == "persist" {
                // persist carries no country code — nothing to
                // patch; treat as handled so the run still
                // resets to system.
                country_progress.mark_flashed(label);
            } else {
                let reason = "no replacements";
                ltbox_core::live!(
                    log,
                    "[Country] {}",
                    tr_args!(
                        "live_country_partition_status",
                        label = label,
                        reason = reason
                    )
                );
                country_progress.mark_failed(label, reason);
            }
        }
        if let Err(e) = country_progress.finish() {
            // Non-fatal: the firmware itself is already
            // flashed, so warn and STILL reset to system
            // rather than aborting and leaving the device
            // stuck in EDL. The country code just stays
            // whatever was already on the partition.
            ltbox_core::live!(
                log,
                "[Country] {}",
                tr_args!("live_country_warning", error = e)
            );
        }
        // Surface the backup location once
        // per run. Empty dir = every label
        // was skipped.
        if std::fs::read_dir(&critical_backup)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false)
        {
            live!(
                log,
                "[Country] {} {}",
                ll.backup_saved_prefix,
                critical_backup.display()
            );
        }
    }

    // (efisp GBL is flashed earlier — right after the ARB
    // overlays — so the testkey chain and its `_arb` root of
    // trust are provisioned before the best-effort
    // region/country work that can abort in between.)

    // Mark `_a` active before reset. Lenovo
    // firmware rawprograms only target `_a`, so
    // a full flash always lands on `_a`. Without
    // this the SoC may continue booting from a
    // previously-active `_b` on the next reset
    // and the freshly-written `_a` firmware
    // would never run.
    if let Err(e) = session.set_active_slot_a(&mut log) {
        return Err(tr_args!(
            "err_flash_set_bootable_lun_failed",
            error = e.to_string()
        ));
    }

    live!(log, "[Flash] {}", phase_marker(4, 4, &ll.op_flash_phase[3]));
    session.reset_tolerant(&mut log);
    live!(log, "[Flash] {}", ll.flash_completed);
    Ok(log)
}

/// Simple firmware flash: flash a firmware folder exactly like a stock
/// Lenovo flash script, skipping every LTBox-side check and modification.
///
/// Unlike [`flash_worker`] this performs **no** fingerprint/model check, **no**
/// signing-key check, and **no** region / rollback / country / wipe handling.
/// It only: decrypts the firmware's own `rawprogram*.x` pack to `.xml`,
/// transitions to EDL per the current connection mode, and flashes the
/// selected rawprogram + patch XMLs verbatim. The XML selection is the *same*
/// [`collect_firmware_xmls_for_flash`](ltbox_device::edl::collect_firmware_xmls_for_flash)
/// the full flash uses, so the persist-less LUN0 rawprogram stays prioritized
/// and only it is included. Whether user data is wiped is therefore decided
/// solely by the firmware package, not by LTBox.
pub(crate) fn simple_flash_worker(
    conn: ConnectionStatus,
    fw_folder: String,
    ll: LiveLabels,
) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    let fw_dir = std::path::Path::new(&fw_folder);

    // 1. Validate firmware folder.
    if !fw_dir.exists() {
        return Err(tr_args!(
            "err_flash_firmware_folder_missing",
            path = fw_folder
        ));
    }
    live!(
        log,
        "[SimpleFlash] {}",
        tr_args!("live_flash_firmware_folder", path = fw_folder)
    );

    // 2. Decrypt the firmware's own `rawprogram*.x` pack to `.xml` so the
    //    catalog scan below can read it. The encrypted Sahara manifest
    //    (`qsahara_device_programmer.x`) is a loader, not a flash image, so it
    //    is left for `EdlSession::open` to decrypt at load time. This unpacks
    //    the firmware as shipped — it is not a content modification.
    let x_entries: Vec<std::path::PathBuf> = std::fs::read_dir(fw_dir)
        .map_err(|e| {
            tr_args!(
                "err_read_dir_failed",
                path = fw_dir.display().to_string(),
                error = e.to_string()
            )
        })?
        .filter_map(|r| r.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.eq_ignore_ascii_case("x"))
                    .unwrap_or(false)
                && !ltbox_core::sahara_xml::is_encrypted_manifest_filename(p)
        })
        .collect();
    if !x_entries.is_empty() {
        let mut decrypted = 0usize;
        for src in &x_entries {
            let stem = src.file_stem().unwrap_or_default();
            let output = fw_dir.join(stem).with_extension("xml");
            ltbox_core::crypto::decrypt_file(src, &output).map_err(|e| {
                tr_args!(
                    "err_decrypt_file_failed",
                    path = src.display().to_string(),
                    error = e.to_string()
                )
            })?;
            decrypted += 1;
        }
        live!(
            log,
            "[XML] {}",
            tr_args!("live_xml_decrypt_done", count = decrypted.to_string())
        );
    }

    // 3. Locate the EDL loader inside the firmware folder (or its parent).
    //    A missing loader is a hard error — nothing can be flashed, so the run
    //    must fail rather than report success.
    let loader = find_edl_loader(fw_dir)
        .or_else(|| fw_dir.parent().and_then(find_edl_loader))
        .ok_or_else(|| ltbox_core::i18n::tr("live_edl_loader_missing"))?;

    // 4. XML selection — identical to the full firmware flash so the
    //    persist-less rawprogram0 stays first and only it is included.
    let (raw_xmls, patch_xmls) = ltbox_device::edl::collect_firmware_xmls_for_flash(fw_dir, false)
        .map_err(|e| tr_args!("err_flash_xml_selection_failed", error = e.to_string()))?;
    if raw_xmls.is_empty() {
        return Err(tr_args!("err_flash_no_rawprogram_xml", path = fw_folder));
    }

    // 5. Transition to EDL using the shared live-probe path (re-probes the
    //    current transport rather than trusting the captured snapshot), then
    //    open the session — same entry path normal firmware flashing uses.
    transition_to_edl(conn, &ll, &mut log)?;
    let mut session = ltbox_device::edl::EdlSession::open(&loader, true, &mut log)
        .map_err(|e| tr_args!("err_edl_session_open_failed", error = e.to_string()))?;

    // 6. Flash verbatim — no FP check, no signing-key check, no region / ARB /
    //    country edits, no keep-data skip.
    live!(
        log,
        "[SimpleFlash] {}",
        tr_args!(
            "live_flash_phase3_xml_counts",
            raw = raw_xmls.len().to_string(),
            patch = patch_xmls.len().to_string()
        )
    );
    session
        .flash_rawprogram_verbatim(&raw_xmls, &patch_xmls, &mut log)
        .map_err(|e| tr_args!("err_flash_firmware_failed", error = e.to_string()))?;

    // 7. Mark `_a` active before reset (Lenovo rawprograms only target `_a`),
    //    same as the stock script / full flash so the device boots the
    //    freshly-written slot on the next reset.
    if let Err(e) = session.set_active_slot_a(&mut log) {
        return Err(tr_args!(
            "err_flash_set_bootable_lun_failed",
            error = e.to_string()
        ));
    }
    session.reset_tolerant(&mut log);
    live!(
        log,
        "[SimpleFlash] {}",
        ltbox_core::i18n::tr("live_flash_completed")
    );
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::should_reboot_fastboot_to_system_after_pre_edl_abort;

    #[test]
    fn pre_edl_abort_reboots_when_flash_flow_entered_fastboot() {
        assert!(should_reboot_fastboot_to_system_after_pre_edl_abort(false));
    }

    #[test]
    fn pre_edl_abort_keeps_fastboot_when_flash_started_in_fastboot() {
        assert!(!should_reboot_fastboot_to_system_after_pre_edl_abort(true));
    }

    #[test]
    fn edl_start_rollback_floors_are_component_wise() {
        use super::rollback_floors;
        // Per-location max across slots: _a=(boot 10, vbs 1), _b=(boot 9, vbs 8)
        // -> (10, 8); a single-slot pick would have missed vbs 8.
        assert_eq!(
            rollback_floors([Some(10), Some(9)], [Some(1), Some(8)]),
            Some((10, 8))
        );
        // A location that parsed on only one slot uses that slot's value.
        assert_eq!(
            rollback_floors([Some(7), None], [None, Some(3)]),
            Some((7, 3))
        );
        // A location that parsed on neither slot -> abort.
        assert_eq!(rollback_floors([None, None], [Some(5), Some(6)]), None);
        assert_eq!(rollback_floors([Some(5), Some(6)], [None, None]), None);
        assert_eq!(rollback_floors([None, None], [None, None]), None);
    }
}
