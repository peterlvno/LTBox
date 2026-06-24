//! End-to-end root pipeline: download → dump → patch → resign → flash.
//!
//! Orchestrates [`crate::magisk`], [`crate::ksu`], [`crate::avb`], and
//! `ltbox_device::edl`. Outputs land in `cfg.output_dir` (patched boot +
//! rebuilt vbmeta), then flash pushes them to the active slot.

use std::path::PathBuf;

// fs_err: io::Error Display includes the path, so bare `?` gives readable errors.
use fs_err as fs;

use ltbox_core::github::GitHubClient;
use ltbox_core::i18n::tr;
use ltbox_core::{LtboxError, Result, tr_args};

use crate::{avb, gki, key_map};

pub mod apatch;
pub mod apk;
pub mod ksu;
pub mod magisk;
pub mod skroot;

// Re-exports preserving the pre-split flat public API:
// `ltbox_patch::root_pipeline::stage_root_manager_apk` etc. continue to
// resolve unchanged for external callers (notably the GUI).
pub use apatch::{download_apatch_payload, download_apatch_payload_nightly};
pub use ksu::{
    download_ksu_payload, download_ksu_payload_nightly, normalize_ksu_kernel_version,
    stage_root_manager_apk,
};
pub use magisk::{download_latest_magisk_apk, download_magisk_apk_nightly};

/// Pick the avbtool-rs key_spec for re-signing.
/// Missing pubkey means unsigned; unknown signed pubkeys abort before writes.
fn resolve_signing_key(
    pubkey_sha1: Option<&str>,
    image_name: &str,
    log: &mut Vec<String>,
) -> Result<Option<String>> {
    match key_map::key_spec_for_signed_pubkey(pubkey_sha1) {
        Ok(Some(spec)) => {
            let sha = pubkey_sha1.unwrap_or("").trim();
            ltbox_core::live!(
                log,
                "[AVB] {image_name} {} {sha} → {} {spec}",
                tr("log_avb_pubkey"),
                tr("log_avb_bundled")
            );
            Ok(Some(spec.to_string()))
        }
        Ok(None) => {
            ltbox_core::live!(
                log,
                "[AVB] {image_name} {}",
                tr("log_avb_unsigned_skip_key")
            );
            Ok(None)
        }
        Err(sha) => Err(LtboxError::Avb(tr_args!(
            "err_avb_signing_key_unknown",
            image = image_name,
            key = sha
        ))),
    }
}

/// Provider families carried through the GUI wizard state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootFamily {
    /// Magisk / forks — init_boot ramdisk injection.
    Magisk,
    /// KernelSU-style LKM — init_boot with ksuinit + kernelsu.ko.
    KernelSU,
    /// APatch — boot image via kptools + kpimg.
    APatch,
    /// SKRoot Lite — direct kernel binary patch inside boot.img.
    Skroot,
}

/// Provider inside the family to fetch from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootProvider {
    Magisk,
    MagiskFork,
    KernelSU,
    KernelSUNext,
    SukiSU,
    ReSukiSU,
    APatch,
    FolkPatch,
    Skroot,
}

/// Release channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootVersion {
    Stable,
    Nightly,
}

/// Root pipeline input from the GUI wizard.
#[derive(Clone)]
pub struct RootPipelineConfig {
    pub family: RootFamily,
    pub provider: RootProvider,
    pub version: RootVersion,

    /// APK extraction + boot patching workspace. Cleaned on entry.
    pub work_dir: PathBuf,
    /// Where patched boot + vbmeta land.
    pub output_dir: PathBuf,
    /// EDL loader path (`xbl_s_devprg_ns.melf`).
    pub loader: PathBuf,
    /// Active slot (`_a` / `_b` / empty; empty → flash defaults to `_a`).
    pub slot_suffix: String,
    /// Magisk `PREINITDEVICE`. Empty → Magisk resolves at runtime.
    pub preinit_device: String,
    /// GKI-mode only: user-supplied AnyKernel3 zip.
    pub gki_kernel_zip: Option<PathBuf>,
    /// Device kernel version (`major.minor.patch` from `uname -r`) —
    /// used by KSU to pick the matching `.ko` release asset.
    pub kernel_version: Option<String>,
    /// GKI mode → patch `boot.img` via `gki::patch_boot` instead of the
    /// Magisk/KSU ramdisk path.
    pub gki_mode: bool,
    /// APatch / FolkPatch: `.kpm` modules to embed.
    pub kpm_paths: Vec<PathBuf>,
    /// APatch / FolkPatch: superkey (8..=63 ASCII alphanumeric).
    pub superkey: String,
    /// Magisk Forks: user-picked variant APK (local-APK-only in v2 parity).
    pub magisk_forks_apk: Option<PathBuf>,
    /// Nightly: manual workflow run ID. `None` → auto-detect latest.
    pub nightly_run_id: Option<u64>,
}

/// Per-provider `(workflow_file, default_branch)` for nightly runs.
/// Returns `None` for providers without a nightly channel (e.g. MagiskFork).
fn provider_workflow(provider: RootProvider) -> Option<(&'static str, &'static str)> {
    Some(match provider {
        RootProvider::Magisk => ("ci.yml", "master"),
        RootProvider::MagiskFork => return None,
        RootProvider::KernelSU => ("build-manager.yml", "main"),
        RootProvider::KernelSUNext => ("build-manager-ci.yml", "dev"),
        RootProvider::SukiSU => ("build-manager.yml", "main"),
        RootProvider::ReSukiSU => ("build-manager.yml", "main"),
        RootProvider::APatch => ("build.yml", "main"),
        RootProvider::FolkPatch => ("build.yml", "main"),
        RootProvider::Skroot => return None,
    })
}

/// Resolve `(repo, run_id)` for a nightly fetch. Manual IDs are validated
/// against the provider's workflow so bad IDs fail fast, not at nightly.link.
pub(super) fn resolve_nightly_run(
    provider: RootProvider,
    manual_run_id: Option<u64>,
    log: &mut Vec<String>,
) -> Result<(&'static str, u64)> {
    let repo = provider_repo(provider).ok_or_else(|| {
        LtboxError::Patch(format!(
            "resolve_nightly_run: unsupported provider {provider:?}"
        ))
    })?;
    let (workflow_file, branch) = provider_workflow(provider).ok_or_else(|| {
        LtboxError::Patch(format!(
            "resolve_nightly_run: no workflow metadata for {provider:?}"
        ))
    })?;
    let client = GitHubClient::new(repo)?;

    let run_id = match manual_run_id {
        Some(id) => {
            ltbox_core::live!(
                log,
                "[Nightly] {repo}: {}",
                tr_args!(
                    "log_nightly_validating_manual",
                    id = id,
                    workflow = workflow_file,
                    branch = branch,
                )
            );
            if !client.workflow_run_matches(id, workflow_file, Some(branch))? {
                return Err(LtboxError::Patch(format!(
                    "Manual run id {id} does not match workflow {workflow_file} on branch {branch} of {repo}"
                )));
            }
            id
        }
        None => {
            ltbox_core::live!(
                log,
                "[Nightly] {repo}: {}",
                tr_args!(
                    "log_nightly_auto_detect",
                    workflow = workflow_file,
                    branch = branch,
                )
            );
            client
                .latest_successful_run(workflow_file, Some(branch))?
                .ok_or_else(|| {
                    LtboxError::Patch(format!(
                        "No successful {workflow_file} run found on {repo}:{branch}"
                    ))
                })?
        }
    };
    ltbox_core::live!(
        log,
        "[Nightly] {repo}: {}",
        tr_args!("log_nightly_using_run_id", id = run_id)
    );
    Ok((repo, run_id))
}

/// Resolve and cache one nightly run ID so all artifacts match.
pub fn ensure_nightly_run_id(cfg: &mut RootPipelineConfig, log: &mut Vec<String>) -> Result<()> {
    if !matches!(cfg.version, RootVersion::Nightly) {
        return Ok(());
    }
    if cfg.nightly_run_id.is_some() {
        return Ok(());
    }
    if matches!(cfg.provider, RootProvider::MagiskFork) {
        return Ok(());
    }
    let (_repo, run_id) = resolve_nightly_run(cfg.provider, None, log)?;
    cfg.nightly_run_id = Some(run_id);
    Ok(())
}

/// Build the `nightly.link` public-mirror URL. Response is always ZIP-wrapped.
pub(super) fn nightly_artifact_url(repo: &str, run_id: u64, artifact_name: &str) -> String {
    let suffix = if artifact_name.ends_with(".zip") {
        ""
    } else {
        ".zip"
    };
    format!("https://nightly.link/{repo}/actions/runs/{run_id}/{artifact_name}{suffix}")
}

/// Which base partition this pipeline targets.
/// `"boot"` for GKI + APatch/FolkPatch (kernel-blob patching),
/// `"init_boot"` for Magisk / KSU (ramdisk injection).
pub fn boot_partition_base(family: RootFamily, gki_mode: bool) -> &'static str {
    if gki_mode || matches!(family, RootFamily::APatch | RootFamily::Skroot) {
        "boot"
    } else {
        "init_boot"
    }
}

/// Resolve the GitHub repo slug for a given provider.
pub fn provider_repo(provider: RootProvider) -> Option<&'static str> {
    Some(match provider {
        RootProvider::Magisk => "topjohnwu/Magisk",
        RootProvider::MagiskFork => return None,
        RootProvider::KernelSU => "tiann/KernelSU",
        // Upstream moved to the KernelSU-Next org; the old `rifsxd/KernelSU-Next`
        // redirects but its release assets aren't mirrored, so pin the new slug.
        RootProvider::KernelSUNext => "KernelSU-Next/KernelSU-Next",
        RootProvider::SukiSU => "SukiSU-Ultra/SukiSU-Ultra",
        RootProvider::ReSukiSU => "ReSukiSU/ReSukiSU",
        RootProvider::APatch => "bmax121/APatch",
        RootProvider::FolkPatch => "LyraVoid/FolkPatch",
        RootProvider::Skroot => "abcz316/SKRoot-linuxKernelRoot",
    })
}

/// Pre-fetch root payloads before EDL; `build_patched_artifacts` runs offline.
pub fn stage_root_payload(cfg: &RootPipelineConfig, log: &mut Vec<String>) -> Result<()> {
    fs::create_dir_all(&cfg.work_dir)?;
    if cfg.gki_mode {
        return Ok(());
    }
    match cfg.family {
        RootFamily::Magisk => {
            // Skip if already extracted from a prior call.
            if cfg.work_dir.join("magiskinit").exists() {
                return Ok(());
            }
            let apk_path = cfg.work_dir.join("magisk.apk");
            let manager_apk = cfg.work_dir.join("manager.apk");
            // Reuse stage_root_manager_apk's bytes when available
            // — saves a duplicate ~10 MB fetch in the common path.
            if !apk_path.exists() {
                if matches!(cfg.provider, RootProvider::MagiskFork) {
                    let src = cfg.magisk_forks_apk.as_ref().ok_or_else(|| {
                        LtboxError::Patch("Magisk forks require a local APK — none supplied".into())
                    })?;
                    if !src.exists() {
                        return Err(LtboxError::Patch(format!(
                            "Magisk forks APK does not exist: {}",
                            src.display()
                        )));
                    }
                    fs::copy(src, &apk_path)
                        .map_err(|e| LtboxError::Patch(format!("stage forks APK: {e}")))?;
                } else if manager_apk.exists() {
                    fs::copy(&manager_apk, &apk_path).map_err(|e| {
                        LtboxError::Patch(format!("magisk.apk copy from manager.apk: {e}"))
                    })?;
                } else {
                    match cfg.version {
                        RootVersion::Stable => {
                            download_latest_magisk_apk(cfg.provider, &apk_path, log)?;
                        }
                        RootVersion::Nightly => {
                            download_magisk_apk_nightly(
                                cfg.provider,
                                cfg.nightly_run_id,
                                &cfg.work_dir,
                                &apk_path,
                                log,
                            )?;
                        }
                    }
                }
            }
            ltbox_core::live!(log, "[Magisk] {}", tr("log_magisk_extracting_payload"));
            crate::magisk::extract_apk_payload(&apk_path, &cfg.work_dir)?;
        }
        RootFamily::KernelSU => {
            // Skip if both files already on disk from a prior call.
            let ko = cfg.work_dir.join("kernelsu.ko");
            let init = cfg.work_dir.join("init");
            if ko.exists() && init.exists() {
                return Ok(());
            }
            match cfg.version {
                RootVersion::Stable => {
                    ltbox_core::live!(log, "[KSU] {}", tr("log_ksu_fetching_stable"));
                    download_ksu_payload(
                        cfg.provider,
                        cfg.kernel_version.as_deref(),
                        &cfg.work_dir,
                        log,
                    )?;
                }
                RootVersion::Nightly => {
                    ltbox_core::live!(
                        log,
                        "[KSU] {}",
                        tr_args!(
                            "log_ksu_fetching_nightly",
                            run_id = format!("{:?}", cfg.nightly_run_id),
                        )
                    );
                    download_ksu_payload_nightly(
                        cfg.provider,
                        cfg.kernel_version.as_deref(),
                        cfg.nightly_run_id,
                        &cfg.work_dir,
                        log,
                    )?;
                }
            }
        }
        RootFamily::APatch => {
            // stage_root_manager_apk for APatch already downloads the
            // APK and extracts kpimg via download_apatch_payload — no
            // additional payload fetch needed here.
        }
        RootFamily::Skroot => {
            // SKRoot Lite patches the dumped kernel directly. The manager
            // APK is fetched by stage_root_manager_apk; no extra payload.
        }
    }
    Ok(())
}

/// Offline pipeline outcome — everything before the EDL flash step.
pub struct PatchedArtifacts {
    pub patched_boot: PathBuf,
    /// `None` when the original vbmeta can stay (no chain).
    pub patched_vbmeta: Option<PathBuf>,
    pub manager_apk: Option<PathBuf>,
    /// Target partition name (`init_boot_a`, `boot_a`, …).
    pub boot_partition: String,
    pub vbmeta_partition: Option<String>,
}

/// Build patched artifacts: fetch payload, patch, resign, rebuild vbmeta,
/// move finals into `output_dir`. Caller must have already dumped stock
/// images into `cfg.work_dir` (GUI reuses the EDL session for flash).
pub fn build_patched_artifacts(
    cfg: &RootPipelineConfig,
    skip_avb: bool,
    log: &mut Vec<String>,
) -> Result<PatchedArtifacts> {
    fs::create_dir_all(&cfg.work_dir)?;
    fs::create_dir_all(&cfg.output_dir)?;

    // GKI → boot.img; LKM → init_boot.img. GUI dump step picks the right one.
    let base_part = boot_partition_base(cfg.family, cfg.gki_mode);
    let stock_filename = if base_part == "boot" {
        "boot.img"
    } else {
        "init_boot.img"
    };
    let stock_boot_src = cfg.work_dir.join(stock_filename);
    let vbmeta_src = cfg.work_dir.join("vbmeta.img");
    if !stock_boot_src.exists() {
        return Err(LtboxError::Patch(format!(
            "work_dir is missing the stock {stock_filename} dump"
        )));
    }
    // TB323FU GBL root flashes the repacked boot as-is — AVB / vbmeta are not
    // touched — so the stock vbmeta dump isn't required.
    if !skip_avb && !vbmeta_src.exists() {
        return Err(LtboxError::Patch(
            "work_dir is missing the stock vbmeta.img dump".into(),
        ));
    }
    // Defensive: GUI Phase 2 prefetches the manager APK + payload
    // before EDL, but headless callers (and the stable test
    // surface) shouldn't have to remember the order. Both helpers
    // are idempotent against already-staged files.
    let staged_manager_apk = cfg.work_dir.join("manager.apk");
    if !cfg.gki_mode && !staged_manager_apk.exists() {
        stage_root_manager_apk(cfg, log)?;
    }
    if !cfg.gki_mode {
        stage_root_payload(cfg, log)?;
    }

    let patched_boot = if cfg.gki_mode {
        // GKI: swap kernel blob from user's AnyKernel3 zip — no GitHub fetch.
        let kernel_zip = cfg.gki_kernel_zip.as_ref().ok_or_else(|| {
            LtboxError::Patch("GKI mode requires a custom kernel zip — none supplied".into())
        })?;
        ltbox_core::live!(
            log,
            "[GKI] {}",
            tr_args!("log_gki_kernel_zip", path = kernel_zip.display())
        );
        gki::patch_boot(&cfg.work_dir, kernel_zip, log)?
    } else {
        match cfg.family {
            RootFamily::Magisk => {
                ltbox_core::live!(log, "[Magisk] {}", tr("log_magisk_patching_init_boot"));
                crate::magisk::patch_init_boot(&cfg.work_dir, &cfg.preinit_device, log)?
            }
            RootFamily::KernelSU => {
                ltbox_core::live!(log, "[KSU] {}", tr("log_ksu_patching_init_boot"));
                crate::ksu::patch_init_boot(&cfg.work_dir, log)?
            }
            RootFamily::APatch => {
                ltbox_core::live!(
                    log,
                    "[APatch] {}",
                    tr_args!(
                        "log_apatch_patching_boot",
                        kpm_count = cfg.kpm_paths.len(),
                        superkey_len = cfg.superkey.len(),
                    )
                );
                crate::apatch::patch_boot(&cfg.work_dir, &cfg.kpm_paths, &cfg.superkey, log)?
            }
            RootFamily::Skroot => skroot::patch_boot(&cfg.work_dir, log)?,
        }
    };

    let final_boot = cfg.output_dir.join(stock_filename);
    if final_boot.exists() {
        fs::remove_file(&final_boot).ok();
    }
    fs::rename(&patched_boot, &final_boot)?;
    ltbox_core::live!(
        log,
        "[Root] {} {} {} {}",
        tr("log_root_patched"),
        stock_filename,
        tr("log_root_ready_at"),
        final_boot.display()
    );

    // Slot suffix must be poll-resolved by the caller. Defaulting to
    // `_a` here was a silent footgun: when the device was actually
    // running on `_b`, the patched artifact landed on the wrong slot
    // and the user got "root succeeded" with the active slot still
    // unmodified. The GUI threads `controller::poll_active_slot`
    // through `RootPipelineConfig.slot_suffix`; reject an empty
    // value rather than picking a guess.
    if cfg.slot_suffix.is_empty() {
        return Err(LtboxError::Patch(
            "slot_suffix is empty; caller must resolve the active slot via \
             controller::poll_active_slot before invoking the root pipeline"
                .to_string(),
        ));
    }
    let suffix = cfg.slot_suffix.clone();

    let (patched_vbmeta, vbmeta_partition) = if skip_avb {
        // TB323FU GBL root: boot verification is handled by the GBL EFI on
        // `efisp`, so the stock AVB chain is bypassed entirely. Flash the
        // repacked image as-is — no re-footer, no vbmeta rebuild, no vbmeta
        // flash (the caller skips the vbmeta dump too).
        ltbox_core::live!(log, "[AVB] {}", tr("log_root_skip_avb_tb323fu"));
        (None, None)
    } else {
        // Re-apply AVB footer. Algorithm + rollback index copied from stock to
        // preserve device's rollback state. Signing key via `KEY_MAP` on stock pubkey.
        let stock_info = avb::extract_image_avb_info(&stock_boot_src)?;
        let boot_key =
            resolve_signing_key(stock_info.public_key_sha1.as_deref(), stock_filename, log)?;
        // Erase any stale AVB footer before re-applying ours. A missing footer
        // is the normal case for a freshly built image, so this is best-effort —
        // but surface a real failure (I/O, corruption) in the log instead of
        // swallowing it silently, since `add_hash_footer` then runs on this image.
        if let Err(e) = avb::erase_footer(&final_boot) {
            ltbox_core::live!(
                log,
                "[AVB] {}",
                tr_args!("log_avb_erase_footer_skipped", error = e.to_string())
            );
        }
        avb::add_hash_footer(
            &final_boot,
            &stock_info,
            boot_key.as_deref(),
            Some(stock_info.rollback_index),
        )?;
        ltbox_core::live!(
            log,
            "[AVB] {} {} ({} rollback={}, key={})",
            tr("log_avb_refootered"),
            stock_filename,
            stock_info.algorithm,
            stock_info.rollback_index,
            boot_key.as_deref().unwrap_or("(unsigned)"),
        );

        // Rebuild vbmeta with fresh hash descriptor. vbmeta pubkey may differ
        // from boot pubkey, so verify it separately against KEY_MAP.
        let stock_vbmeta_info = avb::extract_image_avb_info(&vbmeta_src)?;
        let vbmeta_key = resolve_signing_key(
            stock_vbmeta_info.public_key_sha1.as_deref(),
            "vbmeta.img",
            log,
        )?;
        let final_vbmeta = cfg.output_dir.join("vbmeta.img");
        match vbmeta_key.as_deref() {
            Some(key) => {
                avb::rebuild_vbmeta_with_chained_images(
                    &final_vbmeta,
                    &vbmeta_src,
                    &[&final_boot],
                    key,
                    None,
                )?;
                ltbox_core::live!(
                    log,
                    "[AVB] {} {} at {} (key={key})",
                    tr("log_avb_rebuilt_vbmeta"),
                    stock_filename,
                    final_vbmeta.display(),
                );
            }
            None => {
                // Unsigned vbmeta: copy stock through. Stale chain hash is fine
                // since NONE-algorithm bootloaders skip verification.
                fs::copy(&vbmeta_src, &final_vbmeta)?;
                ltbox_core::live!(
                    log,
                    "[AVB] {} {}",
                    tr("log_avb_vbmeta_unsigned_copied"),
                    final_vbmeta.display(),
                );
            }
        }
        (Some(final_vbmeta), Some(format!("vbmeta{suffix}")))
    };

    Ok(PatchedArtifacts {
        patched_boot: final_boot,
        patched_vbmeta,
        manager_apk: staged_manager_apk.exists().then_some(staged_manager_apk),
        boot_partition: format!("{base_part}{suffix}"),
        vbmeta_partition,
    })
}
