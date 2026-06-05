//! Firmware flash worker: validate the firmware folder, route to EDL,
//! apply region / rollback / country / wipe modifications, and flash every
//! image (incl. the TB323FU ARB-overlay path). Extracted from the
//! update_flash handler.

use crate::{
    ConnectionStatus, CountryPatchProgress, LiveLabels, WorkflowConfig, build_tb323fu_arb_overlays,
    efisp_asset_suffix, find_edl_loader, fingerprint_token_match, phase_marker, transition_to_edl,
};
use ltbox_core::{live, tr_args};

fn reboot_fastboot_to_system_after_pre_edl_abort(log: &mut Vec<String>) {
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

pub(crate) fn flash_worker(
    cfg: WorkflowConfig,
    conn: ConnectionStatus,
    device_model: String,
    fw_folder: String,
    mut rb_mode: ltbox_patch::rollback::RollbackMode,
    ll: LiveLabels,
) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    let edl_start = matches!(conn, ConnectionStatus::Edl);
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
    let probe_fastboot = || -> (Option<u64>, bool) {
        match ltbox_device::fastboot::FastbootDevice::open() {
            Ok(mut dev) => match dev.get_all_vars() {
                Ok(v) => (
                    ltbox_patch::rollback::compute_device_rollback_index(&v.rollback_indices),
                    true,
                ),
                Err(_) => (None, false),
            },
            Err(_) => (None, false),
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
    let (device_rollback_index, fastboot_reachable) = probe;

    // 3. Scan firmware folder
    let vendor_boot = fw_dir.join("vendor_boot.img");
    let vbmeta = fw_dir.join("vbmeta.img");
    let boot = fw_dir.join("boot.img");
    let has_vendor_boot = vendor_boot.exists();
    let has_vbmeta = vbmeta.exists();
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

    // Cross-check vendor_boot against the probed model
    // before EDL, and retain its fingerprint for SKU gates.
    let mut vendor_boot_fingerprint: Option<String> = None;
    if has_vendor_boot {
        match ltbox_patch::avb::extract_image_avb_info(&vendor_boot) {
            Ok(info) => {
                // Pull the fingerprint prop up-front so the SKU
                // gate below works on EDL-start too — there
                // `device_model` is empty and the validate path
                // would skip without populating it.
                let fp_prop = info
                    .props
                    .iter()
                    .find(|(k, _)| k == "com.android.build.vendor_boot.fingerprint")
                    .and_then(|(_, v)| std::str::from_utf8(v).ok())
                    .map(|s| s.trim_end_matches('\0').to_string());

                if edl_start {
                    vendor_boot_fingerprint = fp_prop;
                } else {
                    use ltbox_patch::region::{ModelValidation, validate_device_model};
                    match validate_device_model(&info, &device_model) {
                        ModelValidation::Match { fingerprint } => {
                            ltbox_core::live!(
                                log,
                                "[Flash] {}",
                                ltbox_core::i18n::tr("live_rescue_model_check_ok")
                            );
                            vendor_boot_fingerprint = Some(fingerprint);
                        }
                        ModelValidation::Missing => {
                            ltbox_core::live!(
                                log,
                                "[Flash] {}",
                                ltbox_core::i18n::tr("live_rescue_no_fingerprint_skip")
                            );
                            vendor_boot_fingerprint = fp_prop;
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
                            reboot_fastboot_to_system_after_pre_edl_abort(&mut log);
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
    let tb323fu_skip_region = vendor_boot_fingerprint
        .as_deref()
        .map(|fp| fingerprint_token_match(fp, "TB323FU"))
        .unwrap_or(false)
        || fingerprint_token_match(&device_model, "TB323FU");

    // GBL/ARB work follows the TARGET firmware identity
    // (vendor_boot fp), never the connected device.
    let target_is_tb323fu = vendor_boot_fingerprint
        .as_deref()
        .map(|fp| fingerprint_token_match(fp, "TB323FU"))
        .unwrap_or(false);

    // EDL-start can't reach Fastboot vars (the generic ARB
    // index source) or the device model, so non-TB323FU
    // silently downgrades ARB On/Auto → Off. TB323FU is
    // exempt: it reads its index by dumping partitions over
    // this very EDL session, so it keeps On/Auto.
    if edl_start && !target_is_tb323fu && rb_mode != ltbox_patch::rollback::RollbackMode::Off {
        rb_mode = ltbox_patch::rollback::RollbackMode::Off;
        live!(
            log,
            "[Flash] {}",
            ltbox_core::i18n::tr("live_flash_edl_start_skips")
        );
    }

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
    // index. Bail before EDL. Runs AFTER the TB323FU On→Auto
    // demotion above, so an EDL-start TB323FU (whose index
    // is read over EDL, not fastboot) is already Auto here
    // and is not aborted.
    if matches!(rb_mode, ltbox_patch::rollback::RollbackMode::On) && !fastboot_reachable {
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
                reboot_fastboot_to_system_after_pre_edl_abort(&mut log);
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
                    reboot_fastboot_to_system_after_pre_edl_abort(&mut log);
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
    if has_boot {
        // Pre-result "Analyzing …" line dropped — analysis is
        // synchronous and the result line ("boot.img rollback
        // index: …") fires immediately after.
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

    // Full-firmware flash: rawprogram + patch XMLs
    // drive every program node (no slot guessing).
    let (raw_xmls, patch_xmls) = ltbox_device::edl::collect_firmware_xmls_for_flash(fw_dir, false)
        .map_err(|e| tr_args!("err_flash_xml_selection_failed", error = e.to_string()))?;
    if raw_xmls.is_empty() {
        return Err(tr_args!("err_flash_no_rawprogram_xml", path = fw_folder));
    }
    // Stage ARB copies; flash them after rawprogram.
    let mut arb_patched: Vec<(String, u8, std::path::PathBuf)> = Vec::new();
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
        let (overlays, need) =
            build_tb323fu_arb_overlays(&mut session, fw_dir, &arb_work_dir, &mut log)?;
        tb323fu_arb_need = need;
        arb_patched = overlays;
    } else if rb_mode != ltbox_patch::rollback::RollbackMode::Off {
        let arb_work_dir = ltbox_core::app_paths::work_dir_for("flash_arb");
        let _ = std::fs::remove_dir_all(&arb_work_dir);
        std::fs::create_dir_all(&arb_work_dir).map_err(|e| format!("arb work dir: {e}"))?;

        // (base, on-disk filename, slot label)
        let label_pairs: &[(&str, &str, &str)] = &[
            ("boot", "boot.img", "boot_a"),
            ("vbmeta_system", "vbmeta_system.img", "vbmeta_system_a"),
        ];
        for (log_name, filename, slot_label) in label_pairs {
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
                &source,
                device_rollback_index,
                rb_mode,
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
                    needs = analysis.needs_patch.to_string(),
                    mode = format!("{:?}", rb_mode)
                )
            );
            if !analysis.needs_patch {
                continue;
            }
            let Some(target) = device_rollback_index else {
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
        let suffix = efisp_asset_suffix(vendor_boot_fingerprint.as_deref(), tb323fu_arb_need);
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
    session
        .flash_rawprogram_with_wipe(&raw_xmls, &patch_xmls, cfg.wipe, &mut log)
        .map_err(|e| tr_args!("err_flash_firmware_failed", error = e.to_string()))?;

    // Overlay ARB-patched boot/vbmeta_system by GPT name.
    for (label, lun, patched) in &arb_patched {
        live!(
            log,
            "[ARB] {}",
            tr_args!("live_arb_flash_patched", label = label)
        );
        if let Err(e) = session.flash_partition(label, patched, 0, *lun, &mut log) {
            return Err(tr_args!(
                "err_flash_arb_partition_failed",
                label = label,
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
            vendor_boot_fingerprint
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
        let tb323fu_persist_backup_only = vendor_boot_fingerprint
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
