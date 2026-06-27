//! Root worker: build patched boot/init_boot artifacts (Magisk / KernelSU
//! / APatch / GKI), flash them over EDL, and stage the manager APK.
//! Extracted from the update_root handler.

use crate::{
    ConnectionStatus, Family, LiveLabels, Provider, RootMode, VerChoice, efisp_asset_suffix,
    efisp_is_empty, fingerprint_token_match, install_root_manager_apk, open_edl_session,
    phase_marker, stage_manager_apk_for_manual_install, transition_to_edl,
    wait_and_install_root_manager_apk,
};
use ltbox_core::{i18n::tr, live, tr_args};

// The 13 params are the closure's captured locals, threaded through verbatim
// from the update_root handler; bundling them into a struct would only move the
// noise. Extraction is mechanical, so keep the 1:1 capture->param mapping.
#[allow(clippy::too_many_arguments)]
pub(crate) fn root_worker(
    family: Option<Family>,
    mode: Option<RootMode>,
    provider: Option<Provider>,
    version: Option<VerChoice>,
    file_path: Option<String>,
    gui_kernel_version: Option<String>,
    conn: ConnectionStatus,
    fw_folder: Option<String>,
    kpm_paths: Vec<std::path::PathBuf>,
    superkey: String,
    nightly_run_id: Option<u64>,
    preinit_device: String,
    ll: LiveLabels,
) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    let skip_adb = conn.skip_adb();

    // GKI route: AnyKernel3 zip is the full input —
    // no provider / version / GitHub fetch.
    let is_gki_route = mode == Some(RootMode::Gki);
    let family = family.ok_or_else(|| tr("err_root_family_missing"))?;
    let is_skroot_route = family == Family::Skroot;
    let (provider, version) = if is_gki_route {
        // `Magisk` stand-in — picks magiskboot as
        // the backend for unpack/repack.
        (Provider::Magisk, VerChoice::Stable)
    } else if is_skroot_route {
        // SKRoot has no provider/version picker; the pipeline always uses
        // the latest Lite release manager APK and direct boot.img patching.
        (Provider::Magisk, VerChoice::Stable)
    } else {
        let prov = provider.ok_or_else(|| tr("err_root_provider_missing"))?;
        let ver = version.ok_or_else(|| tr("err_root_version_missing"))?;
        (prov, ver)
    };

    use ltbox_patch::root_pipeline::{
        RootFamily, RootPipelineConfig, RootProvider, RootVersion, build_patched_artifacts,
        ensure_nightly_run_id, stage_root_manager_apk, stage_root_payload,
    };

    let pipe_family = match family {
        Family::Magisk => RootFamily::Magisk,
        Family::KernelSU => RootFamily::KernelSU,
        Family::APatch => RootFamily::APatch,
        Family::Skroot => RootFamily::Skroot,
    };
    let pipe_provider = if is_skroot_route {
        RootProvider::Skroot
    } else {
        match provider {
            Provider::Magisk => RootProvider::Magisk,
            Provider::MagiskForks => RootProvider::MagiskFork,
            Provider::KernelSU => RootProvider::KernelSU,
            Provider::KernelSUNext => RootProvider::KernelSUNext,
            Provider::SukiSU => RootProvider::SukiSU,
            Provider::ReSukiSU => RootProvider::ReSukiSU,
            Provider::APatch => RootProvider::APatch,
            Provider::FolkPatch => RootProvider::FolkPatch,
        }
    };
    let pipe_version = match version {
        VerChoice::Stable => RootVersion::Stable,
        VerChoice::Nightly => RootVersion::Nightly,
    };
    let file_path_buf: Option<std::path::PathBuf> =
        file_path.as_ref().map(std::path::PathBuf::from);

    let loader_path = fw_folder.ok_or_else(|| tr("err_root_loader_not_selected"))?;
    let loader = std::path::PathBuf::from(&loader_path);
    if !loader.is_file() {
        return Err(tr_args!("err_root_loader_missing", path = loader.display()));
    }
    // Accept single-blob loaders (`.melf` / `.mbn` /
    // `.elf`), the `.xml` multi-image manifest, or its
    // encrypted `.x` form (TB323FU; decrypted in
    // `EdlSession::open`). Filename is free-form.
    let loader_ok = loader
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            let l = e.to_ascii_lowercase();
            l == "melf" || l == "mbn" || l == "elf" || l == "xml"
        })
        || ltbox_core::sahara_xml::is_encrypted_manifest_filename(&loader);
    if !loader_ok {
        return Err(tr_args!(
            "err_root_loader_invalid_ext",
            path = loader.display()
        ));
    }
    // Signing key: pipeline resolves via KEY_MAP
    // + `public_key_sha1`; PEM is `include_str!`'d
    // in avbtool-rs. No on-disk key consulted here.
    ltbox_core::live!(
        log,
        "[Root] {}",
        tr_args!("log_root_loader", path = loader.display().to_string())
    );

    let base = ltbox_core::app_paths::work_dir_for("root");
    let work_dir = base.join("work");
    let output_dir = base.join("out");
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir)
        .map_err(|e| tr_args!("err_root_work_dir_failed", error = e))?;
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| tr_args!("err_root_output_dir_failed", error = e))?;

    // Phase 1/7 — ADB connect + slot/kver detect.
    // Front-loaded so the user sees something happen
    // before the long manager-APK / payload download.
    live!(log, "[Root] {}", phase_marker(1, 7, &ll.op_root_phase[0]));
    // Slot detection MUST succeed — root flashes
    // boot_<slot> + vbmeta_<slot> + init_boot_<slot>,
    // and silently defaulting to `_a` previously
    // landed flashes on the wrong slot when the
    // device was actually running on `_b`. Poll
    // both ADB + Fastboot up to 30 s; on failure,
    // the helper returns a diagnostic that names
    // which transport last failed and what to do
    // (re-plug into normal/recovery, reboot to
    // bootloader, fix unauthorized ADB, …).
    let slot_suffix =
        ltbox_device::controller::poll_active_slot(std::time::Duration::from_secs(30), &mut log)
            .map_err(|e| e.to_string())?;
    // Kernel version probe (KSU LKM) needs ADB
    // shell; runs only when ADB is currently
    // usable so the slot-resolved-via-Fastboot
    // path doesn't waste 30 s waiting for a
    // shell that won't come.
    let mut kernel_version: Option<String> = gui_kernel_version.clone();
    let mut adb_ready_at_start = false;
    if !skip_adb && let Some(mut adb) = ltbox_device::adb::AdbManager::new_if_connected() {
        adb_ready_at_start = true;
        if mode == Some(RootMode::Lkm) {
            if let Ok(Some(kv)) = adb.get_kernel_version() {
                let normalized = ltbox_patch::root_pipeline::normalize_ksu_kernel_version(&kv);
                live!(
                    log,
                    "[ADB] {}",
                    tr_args!(
                        "live_adb_kernel_version",
                        version = normalized.as_deref().unwrap_or(&kv)
                    )
                );
                if let Some(kv) = normalized {
                    kernel_version = Some(kv);
                }
            } else {
                live!(log, "[ADB] {}", ll.adb_no_kver);
            }
        }
    }
    if mode == Some(RootMode::Lkm) && kernel_version.is_none() {
        return Err(tr("err_ksu_lkm_kernel_version_required"));
    }

    let mut manager_cfg = RootPipelineConfig {
        family: pipe_family,
        provider: pipe_provider,
        version: pipe_version,
        work_dir: work_dir.clone(),
        output_dir: output_dir.clone(),
        loader: loader.clone(),
        slot_suffix: slot_suffix.clone(),
        preinit_device: preinit_device.clone(),
        kernel_version: kernel_version.clone(),
        gki_kernel_zip: if is_gki_route {
            file_path_buf.clone()
        } else {
            None
        },
        gki_mode: is_gki_route,
        kpm_paths: kpm_paths.clone(),
        superkey: superkey.clone(),
        magisk_forks_apk: if matches!(pipe_provider, RootProvider::MagiskFork) {
            file_path_buf.clone()
        } else {
            None
        },
        nightly_run_id,
    };
    // Phase 2/7: download all root payloads before EDL.
    live!(log, "[Root] {}", phase_marker(2, 7, &ll.op_root_phase[1]));
    // Pin the nightly workflow run ID once so
    // every fetch in this Phase 2 pulls from
    // the SAME upstream build. Without this,
    // a new workflow landing between the
    // ~minute-long manager APK download and
    // the .ko/ksuinit fetch would split the
    // installed manager APK across two
    // different builds.
    ensure_nightly_run_id(&mut manager_cfg, &mut log)
        .map_err(|e| tr_args!("err_root_nightly_run_failed", error = e))?;
    let mut manager_apk = stage_root_manager_apk(&manager_cfg, &mut log)
        .map_err(|e| tr_args!("err_root_manager_apk_failed", error = e))?;
    stage_root_payload(&manager_cfg, &mut log)
        .map_err(|e| tr_args!("err_root_payload_failed", error = e))?;
    // Manager install is non-fatal; keep the path to surface in the
    // post-run manual-install reminder (the on-device /sdcard copy when
    // the push fallback worked, otherwise the local staged file).
    // `keep_staging` forces the work dir to survive cleanup only in the
    // latter case, where the local file is the user's last resort.
    let mut manager_install_failed_path: Option<std::path::PathBuf> = None;
    let mut keep_staging = false;
    let manager_installed_pre_edl = if adb_ready_at_start {
        if let Some(path) = manager_apk.as_ref() {
            match install_root_manager_apk(path, &mut log) {
                Ok(()) => true,
                Err(e) => {
                    live!(
                        log,
                        "[Root] {}",
                        tr_args!("log_root_manager_apk_install_failed_manual", error = e)
                    );
                    let (reminder, keep) = stage_manager_apk_for_manual_install(path, &mut log);
                    manager_install_failed_path = Some(reminder);
                    keep_staging |= keep;
                    false
                }
            }
        } else {
            false
        }
    } else {
        false
    };

    // Device phase errors still attempt an EDL -> system reset.
    let device_phase_result: std::result::Result<(), String> =
        (|| -> std::result::Result<(), String> {
            // Phase 3/7 — Reboot to EDL (was Phase 1/6).
            live!(log, "[Root] {}", phase_marker(3, 7, &ll.op_root_phase[2]));
            transition_to_edl(conn, &ll, &mut log)?;

            // GKI/APatch use boot_<slot>; Magisk/KSU use
            // init_boot_<slot>. Geometry resolves from GPT.
            let is_gki_mode = is_gki_route;
            let base_name =
                ltbox_patch::root_pipeline::boot_partition_base(pipe_family, is_gki_mode);
            // `slot_suffix` was poll-resolved at Phase 1
            // and propagated through `RootPipelineConfig`;
            // it is guaranteed to be `_a` or `_b` here.
            let boot_primary = format!("{base_name}{slot_suffix}");
            let vbmeta_primary = format!("vbmeta{slot_suffix}");
            // Lenovo devices on Qualcomm UFS place
            // boot / init_boot / vbmeta on LUN 4 (userdata
            // LUN), same index used by the reference
            // `qdl-rs --phys-part-idx 4` recipe.
            const ROOT_PARTITIONS_LUN: u8 = 4;
            live!(
                log,
                "[Root] {} {} / {} (LUN {ROOT_PARTITIONS_LUN})",
                ll.root_resolved_prefix,
                boot_primary,
                vbmeta_primary,
            );

            // Phase 4/7 — Read stock images (was Phase 2/6).
            live!(log, "[Root] {}", phase_marker(4, 7, &ll.op_root_phase[3]));
            // Hoisted so Phase 6 can echo the path.
            // Routed through `app_paths::backup_dir_for`
            // so AppImage / distro Linux installs don't
            // try to write next to the executable.
            let backup_dir = ltbox_core::app_paths::backup_dir_for(&format!("backup_{base_name}"));
            // Set inside the dump block from the dumped boot/init_boot
            // fingerprint; carried to Phase 5 to skip AVB + vbmeta.
            let is_tb323fu;
            // TB323FU only: when the dumped efisp is empty (stock,
            // GBL-unprovisioned) we download the region GBL here and
            // flash it alongside the patched boot at Phase 6.
            let mut root_efisp_efi: Option<std::path::PathBuf> = None;
            {
                let mut session = open_edl_session(&loader, false, &mut log)?;
                // Patch pipeline hardcodes `init_boot.img` /
                // `vbmeta.img` regardless of device label.
                let boot_out = if base_name == "boot" {
                    "boot.img"
                } else {
                    "init_boot.img"
                };
                let dumped_boot = work_dir.join(boot_out);
                let dumped_vbmeta = work_dir.join("vbmeta.img");
                // `dump_partition` scans the LUN's GPT for the
                // named partition — matches the shell-level
                // `qdl-rs --phys-part-idx 4 dump-part <name>`.
                session
                    .dump_partition(
                        &boot_primary,
                        &dumped_boot,
                        0,
                        ROOT_PARTITIONS_LUN,
                        &mut log,
                    )
                    .map_err(|e| {
                        tr_args!(
                            "err_root_dump_partition_failed",
                            partition = boot_primary,
                            error = e
                        )
                    })?;

                // TB323FU root needs provisioned efisp; once present,
                // skip AVB footer and vbmeta work. Keep the fingerprint
                // so an empty efisp can fetch the matching region GBL.
                let boot_fp = ltbox_patch::avb::extract_image_avb_info(&dumped_boot)
                    .ok()
                    .and_then(|info| ltbox_patch::avb::build_fingerprint(&info));
                is_tb323fu = boot_fp
                    .as_deref()
                    .map(|fp| fingerprint_token_match(fp, "TB323FU"))
                    .unwrap_or(false);
                if is_tb323fu {
                    live!(
                        log,
                        "[Root] {}",
                        ltbox_core::i18n::tr("log_root_efisp_check")
                    );
                    let dumped_efisp = work_dir.join("efisp.img");
                    session
                        .dump_partition("efisp", &dumped_efisp, 0, ROOT_PARTITIONS_LUN, &mut log)
                        .map_err(|e| {
                            tr_args!(
                                "err_root_dump_partition_failed",
                                partition = "efisp",
                                error = e
                            )
                        })?;
                    let efisp_empty = std::fs::read(&dumped_efisp)
                        .map(|d| efisp_is_empty(&d))
                        .unwrap_or(true);
                    if efisp_empty {
                        // Empty efisp = stock, GBL-unprovisioned, so the
                        // firmware was never rollback-patched — fetch the
                        // non-`_arb` region GBL and flash it with the patched
                        // boot at Phase 6. efisp flashing no longer wipes data,
                        // so provisioning it here is safe in any data mode. The
                        // region comes from the device vendor_boot's
                        // `product_region` DTB marker — the AVB fingerprint
                        // carries no `_PRC`/`_ROW` token.
                        let vb_part = format!("vendor_boot{slot_suffix}");
                        let dumped_vb = work_dir.join("vendor_boot.img");
                        let is_prc = match ltbox_core::partition_lun::lun_for_partition(&vb_part) {
                            Some(lun)
                                if session
                                    .dump_partition(&vb_part, &dumped_vb, 0, lun, &mut log)
                                    .is_ok() =>
                            {
                                ltbox_patch::region::detect_product_region(&dumped_vb)
                                    == Some(ltbox_patch::region::RegionTarget::Prc)
                            }
                            _ => false,
                        };
                        let suffix = efisp_asset_suffix(is_prc, false);
                        live!(
                            log,
                            "[Root] {}",
                            tr_args!("live_flash_efisp_fetch", variant = suffix)
                        );
                        let gh = ltbox_core::github::GitHubClient::from_url(
                            "github.com/miner7222/gbl_root_baldur",
                        )
                        .map_err(|e| tr_args!("err_root_efisp_github_failed", error = e))?;
                        let (asset_name, asset_url) = gh
                            .latest_release_asset_where(|n| {
                                n.to_ascii_lowercase().ends_with(suffix)
                            })
                            .map_err(|e| {
                                tr_args!("err_root_efisp_asset_missing", suffix = suffix, error = e)
                            })?;
                        let efi_dir = ltbox_core::app_paths::work_dir_for("root_efisp");
                        let _ = std::fs::remove_dir_all(&efi_dir);
                        std::fs::create_dir_all(&efi_dir)
                            .map_err(|e| tr_args!("err_root_efisp_work_dir_failed", error = e))?;
                        let efi_path = efi_dir.join(&asset_name);
                        if let Err(e) = ltbox_core::downloader::download_to_file(
                            &asset_url, &efi_path, &mut log,
                        ) {
                            return Err(tr_args!(
                                "err_root_efisp_download_failed",
                                asset = asset_name,
                                error = e
                            ));
                        }
                        live!(
                            log,
                            "[Root] {}",
                            tr_args!("live_flash_efisp_fetched", name = asset_name)
                        );
                        root_efisp_efi = Some(efi_path);
                    } else {
                        live!(log, "[Root] {}", ltbox_core::i18n::tr("log_root_efisp_ok"));
                    }
                }

                // vbmeta stays untouched on TB323FU (GBL handles
                // verification) — skip its dump + backup.
                if !is_tb323fu {
                    session
                        .dump_partition(
                            &vbmeta_primary,
                            &dumped_vbmeta,
                            0,
                            ROOT_PARTITIONS_LUN,
                            &mut log,
                        )
                        .map_err(|e| {
                            tr_args!(
                                "err_root_dump_partition_failed",
                                partition = vbmeta_primary,
                                error = e
                            )
                        })?;
                }
                // Stock-image safety net for Unroot, captured
                // before the irreversible patch + flash. A copy
                // failure must abort the run.
                std::fs::create_dir_all(&backup_dir).map_err(|e| {
                    tr_args!(
                        "err_root_backup_dir_failed",
                        path = backup_dir.display(),
                        error = e
                    )
                })?;
                std::fs::copy(&dumped_boot, backup_dir.join(boot_out)).map_err(|e| {
                    tr_args!("err_root_backup_copy_failed", image = boot_out, error = e)
                })?;
                if !is_tb323fu {
                    std::fs::copy(&dumped_vbmeta, backup_dir.join("vbmeta.img")).map_err(|e| {
                        tr_args!(
                            "err_root_backup_copy_failed",
                            image = "vbmeta.img",
                            error = e
                        )
                    })?;
                }
                if is_tb323fu {
                    live!(
                        log,
                        "[Root] {} {} → {}",
                        ll.root_backup_copy_prefix,
                        boot_out,
                        backup_dir.display()
                    );
                } else {
                    live!(
                        log,
                        "[Root] {} {} + vbmeta.img → {}",
                        ll.root_backup_copy_prefix,
                        boot_out,
                        backup_dir.display()
                    );
                }
                // Bounce to Sahara — otherwise the second
                // session's sahara_run times out because
                // the device is still in Firehose.
                session
                    .reset_to_edl(&mut log)
                    .map_err(|e| tr_args!("err_root_reset_to_edl_failed", error = e))?;
                // Terminate any dangling pbr `\r`-only
                // line so the next message gets a fresh row.
                println!();
                live!(log, "[EDL] {}", ll.closing_dump);
                // Drop session — serial port closes so
                // the post-patch open gets a fresh handle.
            }

            // Phase 5/7 — Offline patch + AVB resign +
            // vbmeta rebuild. Network downloads moved
            // up to Phase 2; this step never touches
            // the network so progress now matches the
            // "patching" label.
            live!(log, "[Root] {}", phase_marker(5, 7, &ll.op_root_phase[4]));

            // The patch phase reuses the same config the
            // download phase built — none of the input
            // locals mutate between Phase 2 and Phase 5
            // (only `nightly_run_id` was hoisted out of
            // `manager_cfg` for logging). Clone the cfg
            // instead of re-cloning every field, which
            // keeps the two phases in lockstep automatically
            // if a future field gets added to the struct.
            let cfg = manager_cfg.clone();
            let artifacts = build_patched_artifacts(&cfg, is_tb323fu, &mut log)
                .map_err(|e| tr_args!("err_root_patch_failed", error = e))?;
            if manager_apk.is_none() {
                manager_apk = artifacts.manager_apk.clone();
            }
            // Phase 6/7 — Write patched images (was Phase
            // 5/6). Old standalone Phase 4 marker dropped
            // since there was no real work between it and
            // flash open — collapsed into this one phase.
            live!(log, "[Root] {}", phase_marker(6, 7, &ll.op_root_phase[5]));
            let mut session = open_edl_session(&loader, true, &mut log)?;
            // Mirror of the equivalent one-shot `qdl-rs
            // --phys-part-idx 4 write <name> <img>` — GPT
            // resolves the start sector, so no rawprogram
            // sector attrs to thread through.
            // Provision efisp with the region GBL fetched above (only set
            // when the dumped efisp was empty) BEFORE flashing the patched
            // boot. Ordering matters for brick-safety: if the GBL flash
            // fails, boot is still the stock image, so the error-path
            // `reset_tolerant` below boots stock — not a patched boot left
            // without its GBL root of trust.
            if let Some(efi) = &root_efisp_efi {
                let efisp_lun = ltbox_core::partition_lun::lun_for_partition("efisp").unwrap_or(4);
                live!(
                    log,
                    "[Root] {}",
                    ltbox_core::i18n::tr("live_flash_efisp_flash")
                );
                session
                    .flash_partition("efisp", efi, 0, efisp_lun, &mut log)
                    .map_err(|e| tr_args!("err_root_efisp_provision_failed", error = e))?;
                live!(
                    log,
                    "[Root] {}",
                    ltbox_core::i18n::tr("live_flash_efisp_flashed")
                );
            }
            session
                .flash_partition(
                    &boot_primary,
                    &artifacts.patched_boot,
                    0,
                    ROOT_PARTITIONS_LUN,
                    &mut log,
                )
                .map_err(|e| {
                    tr_args!(
                        "err_root_flash_partition_failed",
                        partition = boot_primary,
                        error = e
                    )
                })?;
            if let Some(vbpath) = &artifacts.patched_vbmeta {
                session
                    .flash_partition(&vbmeta_primary, vbpath, 0, ROOT_PARTITIONS_LUN, &mut log)
                    .map_err(|e| {
                        tr_args!(
                            "err_root_flash_partition_failed",
                            partition = vbmeta_primary,
                            error = e
                        )
                    })?;
            }
            println!();
            // Phase 7/7 — Reboot to system (was Phase 6/6).
            live!(log, "[Root] {}", phase_marker(7, 7, &ll.op_root_phase[6]));
            // Surface the backup folder before the reset
            // so the user doesn't have to scroll.
            if backup_dir.exists() {
                live!(
                    log,
                    "[Root] {} {}",
                    ll.backup_saved_prefix,
                    backup_dir.display()
                );
            }
            session.reset_tolerant(&mut log);
            // Skip post-reboot retry if the pre-EDL install
            // already failed for a deterministic reason
            // (e.g. `INSTALL_FAILED_VERSION_DOWNGRADE`) — the
            // 60 s wait + reinstall would just hit the same
            // error after the user's burned a minute waiting.
            // The end-of-run reminder still fires from the
            // pre-EDL `manager_install_failed_path` stamp.
            if !manager_installed_pre_edl
                && manager_install_failed_path.is_none()
                && let Some(path) = manager_apk.as_ref()
                && let Err(e) = wait_and_install_root_manager_apk(
                    path,
                    std::time::Duration::from_secs(60),
                    &mut log,
                )
            {
                // Same non-fatal handling as the pre-EDL path —
                // log the warning, record the donor path for the
                // post-run reminder, keep going so the user
                // doesn't lose the success summary just because
                // the manager package couldn't auto-install.
                live!(
                    log,
                    "[Root] {}",
                    tr_args!("log_root_manager_apk_install_failed_manual", error = e)
                );
                let (reminder, keep) = stage_manager_apk_for_manual_install(path, &mut log);
                manager_install_failed_path = Some(reminder);
                keep_staging |= keep;
            }
            if let Some(path) = manager_install_failed_path.as_ref() {
                live!(
                    log,
                    "[Root] {}",
                    tr_args!(
                        "log_root_manager_apk_manual_reminder",
                        path = path.display().to_string()
                    )
                );
            }
            live!(log, "[Root] {}", ll.root_completed);
            Ok(())
        })();
    match device_phase_result {
        Ok(()) => {
            // Keep staging files on error for debugging, and also when the
            // manager APK could not be auto-installed *and* the on-device
            // push fallback failed — the local staged APK is then the only
            // copy the user can reach, so the reminder points at it and the
            // work dir must survive.
            if !keep_staging {
                let _ = std::fs::remove_dir_all(&base);
            }
            Ok(log)
        }
        Err(e) => {
            // Best-effort: open a fresh session on the same
            // loader and ask the device to boot. `reset_tolerant`
            // already swallows the post-handoff error some
            // devices return, so this never masks the real
            // error — failures here are only logged.
            let mut reset_log: Vec<String> = Vec::new();
            reset_log.push(format!(
                "[EDL] {}",
                tr_args!("log_edl_attempt_reset_after_error", error = e.to_string())
            ));
            if let Ok(mut s) = ltbox_device::edl::EdlSession::open(&loader, false, &mut reset_log) {
                s.reset_tolerant(&mut reset_log);
            } else {
                reset_log.push(format!(
                    "[EDL] {}",
                    ltbox_core::i18n::tr("log_edl_reset_reopen_skipped")
                ));
            }
            for line in reset_log {
                println!("{line}");
            }
            Err(e)
        }
    }
}
