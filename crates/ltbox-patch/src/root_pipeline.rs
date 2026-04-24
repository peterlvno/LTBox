//! End-to-end root pipeline: download → dump → patch → resign → flash.
//!
//! Orchestrates [`crate::magisk`], [`crate::ksu`], [`crate::avb`], and
//! `ltbox_device::edl`. Outputs land in `cfg.output_dir` (patched boot +
//! rebuilt vbmeta), then flash pushes them to the active slot.

use std::path::{Path, PathBuf};

// fs_err: io::Error Display includes the path, so bare `?` gives readable errors.
use fs_err as fs;

use ltbox_core::downloader::download_to_file;
use ltbox_core::github::GitHubClient;
use ltbox_core::i18n::tr;
use ltbox_core::{LtboxError, Result};

use crate::{avb, gki, key_map, ksu, magisk};

/// Echo to `println!` so the GUI's stdout tap streams it to the live log.
/// `$log` kept for call-site compatibility but ignored — library messages
/// push into it separately.
macro_rules! live {
    ($log:expr, $($arg:tt)*) => {{
        let _ = &$log;
        println!($($arg)*);
    }};
}

/// Pick the avbtool-rs key_spec for re-signing.
/// `None` → unsigned (NONE algorithm); `Some(sha)` → `KEY_MAP` lookup,
/// hard error on miss (signing key rolled — add to the map).
fn resolve_signing_key(
    pubkey_sha1: Option<&str>,
    image_name: &str,
    log: &mut Vec<String>,
) -> Result<Option<String>> {
    let Some(sha) = pubkey_sha1 else {
        log.push(format!(
            "[AVB] {image_name} {}",
            tr("log_avb_unsigned_skip_key")
        ));
        return Ok(None);
    };
    if let Some(spec) = key_map::key_spec_for_pubkey(Some(sha)) {
        log.push(format!(
            "[AVB] {image_name} {} {sha} → {} {spec}",
            tr("log_avb_pubkey"),
            tr("log_avb_bundled")
        ));
        return Ok(Some(spec.to_string()));
    }
    Err(LtboxError::Avb(format!(
        "No signing key available for {image_name}: stock pubkey_sha1 = {sha} is not in the bundled KEY_MAP. If the device's signing key has rolled, add it to `ltbox_patch::key_map::KEY_MAP`."
    )))
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
}

/// Release channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootVersion {
    Stable,
    Nightly,
}

/// Root pipeline input — GUI wizard state converted into a flat struct.
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
    })
}

/// Resolve `(repo, run_id)` for a nightly fetch. Manual IDs are validated
/// against the provider's workflow so bad IDs fail fast, not at nightly.link.
fn resolve_nightly_run(
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
            log.push(format!(
                "[Nightly] {repo}: validating manual run id {id} against workflow {workflow_file} (branch {branch})"
            ));
            if !client.workflow_run_matches(id, workflow_file, Some(branch))? {
                return Err(LtboxError::Patch(format!(
                    "Manual run id {id} does not match workflow {workflow_file} on branch {branch} of {repo}"
                )));
            }
            id
        }
        None => {
            log.push(format!(
                "[Nightly] {repo}: auto-detecting latest successful run for {workflow_file} (branch {branch})"
            ));
            client
                .latest_successful_run(workflow_file, Some(branch))?
                .ok_or_else(|| {
                    LtboxError::Patch(format!(
                        "No successful {workflow_file} run found on {repo}:{branch}"
                    ))
                })?
        }
    };
    log.push(format!("[Nightly] {repo}: using run id {run_id}"));
    Ok((repo, run_id))
}

/// Build the `nightly.link` public-mirror URL. Response is always ZIP-wrapped.
fn nightly_artifact_url(repo: &str, run_id: u64, artifact_name: &str) -> String {
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
    if gki_mode || matches!(family, RootFamily::APatch) {
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
        RootProvider::KernelSUNext => "rifsxd/KernelSU-Next",
        RootProvider::SukiSU => "SukiSU-Ultra/SukiSU-Ultra",
        RootProvider::ReSukiSU => "ReSukiSU/ReSukiSU",
        RootProvider::APatch => "bmax121/APatch",
        RootProvider::FolkPatch => "LyraVoid/FolkPatch",
    })
}

fn ksu_manager_asset_preferences(provider: RootProvider) -> &'static [&'static str] {
    match provider {
        RootProvider::KernelSU => &["manager.zip", "Manager.zip"],
        RootProvider::KernelSUNext => &["manager-spoofed.zip", "manager.zip"],
        RootProvider::SukiSU => &["Spoofed-Manager.zip", "Manager.zip", "manager.zip"],
        RootProvider::ReSukiSU => &[
            "Spoofed-Manager-release.zip",
            "Manager-release.zip",
            "manager.zip",
        ],
        _ => &[],
    }
}

fn select_manager_asset(
    assets: &[(String, String)],
    preferred_names: &[&str],
) -> Option<(String, String)> {
    preferred_names
        .iter()
        .find_map(|preferred| {
            assets
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(preferred))
                .cloned()
        })
        .or_else(|| {
            assets
                .iter()
                .find(|(name, _)| {
                    let lower = name.to_lowercase();
                    lower.ends_with(".apk") && lower.contains("manager") && !lower.contains("debug")
                })
                .cloned()
        })
        .or_else(|| {
            assets
                .iter()
                .find(|(name, _)| {
                    let lower = name.to_lowercase();
                    lower.ends_with(".zip") && lower.contains("manager")
                })
                .cloned()
        })
}

/// Download latest Magisk APK into `dst_path`; returns the tag name.
pub fn download_latest_magisk_apk(
    provider: RootProvider,
    dst_path: &Path,
    log: &mut Vec<String>,
) -> Result<String> {
    let repo = provider_repo(provider).ok_or_else(|| {
        LtboxError::Patch("Magisk forks need a local APK — not yet wired in v3".into())
    })?;
    let client = GitHubClient::new(repo)?;
    let (tag, assets) = client.latest_release_assets()?;
    let (name, url) = assets
        .into_iter()
        .find(|(n, _)| {
            let lower = n.to_lowercase();
            lower.ends_with(".apk") && !lower.contains("debug")
        })
        .ok_or_else(|| LtboxError::Download(format!("No release APK on latest {repo}")))?;
    log.push(format!("[Magisk] Latest release: {tag} — asset {name}"));
    download_to_file(&url, dst_path, log)?;
    Ok(tag)
}

/// Download outer nightly ZIP → extract → move inner `.apk` onto `dst_apk`.
/// `rename` falls back to `copy` for cross-volume moves under WSL.
#[allow(clippy::too_many_arguments)]
fn fetch_nightly_apk_outer_zip(
    log_tag: &str,
    repo: &str,
    run_id: u64,
    artifact_name: &str,
    staging_name: &str,
    work_dir: &Path,
    dst_apk: &Path,
    log: &mut Vec<String>,
) -> Result<()> {
    let outer_zip_path = work_dir.join(format!("{staging_name}.zip"));
    let url = nightly_artifact_url(repo, run_id, artifact_name);
    download_to_file(&url, &outer_zip_path, log)?;

    let staging = work_dir.join(staging_name);
    if staging.exists() {
        fs::remove_dir_all(&staging).ok();
    }
    fs::create_dir_all(&staging)?;
    {
        let f = fs::File::open(&outer_zip_path)?;
        let mut archive = zip::ZipArchive::new(f)
            .map_err(|e| LtboxError::Patch(format!("{repo}: nightly artifact not a zip: {e}")))?;
        archive
            .extract(&staging)
            .map_err(|e| LtboxError::Patch(format!("{repo}: extract nightly zip: {e}")))?;
    }

    let apk_src = fs::read_dir(&staging)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("apk"))
        })
        .ok_or_else(|| {
            LtboxError::Patch(format!(
                "{repo} nightly artifact {artifact_name}: no .apk found after extract"
            ))
        })?;

    if dst_apk.exists() {
        fs::remove_file(dst_apk).ok();
    }
    fs::rename(&apk_src, dst_apk).or_else(|_| fs::copy(&apk_src, dst_apk).map(|_| ()))?;
    log.push(format!(
        "[{log_tag}] staged nightly APK: {}",
        dst_apk.display()
    ));
    Ok(())
}

/// Fetch a nightly Magisk APK via `nightly.link`. Prefers `app-release` /
/// `apk-ng-release` artifacts over debug. `manual_run_id = None` →
/// latest successful `ci.yml` run on `master`.
pub fn download_magisk_apk_nightly(
    provider: RootProvider,
    manual_run_id: Option<u64>,
    work_dir: &Path,
    dst_path: &Path,
    log: &mut Vec<String>,
) -> Result<u64> {
    let (repo, run_id) = resolve_nightly_run(provider, manual_run_id, log)?;
    let client = GitHubClient::new(repo)?;
    let artifact_names = client.workflow_artifacts(run_id)?;
    if artifact_names.is_empty() {
        return Err(LtboxError::Patch(format!(
            "{repo} run {run_id} has no artifacts"
        )));
    }
    // Prefer release variants over debug artifacts.
    let preferred: &[&str] = &["app-release", "apk-ng-release"];
    let artifact_name = preferred
        .iter()
        .find_map(|p| {
            artifact_names
                .iter()
                .find(|n| n.to_lowercase().starts_with(p))
                .cloned()
        })
        .or_else(|| {
            artifact_names
                .iter()
                .find(|n| !n.to_lowercase().contains("debug"))
                .cloned()
        })
        .ok_or_else(|| {
            LtboxError::Patch(format!(
                "{repo} run {run_id}: no release APK artifact (got {artifact_names:?})"
            ))
        })?;
    log.push(format!("[Magisk] {repo} nightly artifact: {artifact_name}"));
    fetch_nightly_apk_outer_zip(
        "Magisk",
        repo,
        run_id,
        &artifact_name,
        "magisk_nightly",
        work_dir,
        dst_path,
        log,
    )?;
    Ok(run_id)
}

/// Pull `assets/kpimg` out of a staged APatch/FolkPatch APK into `work_dir/kpimg`.
fn extract_kpimg_from_apk(
    repo: &str,
    apk_path: &Path,
    work_dir: &Path,
    log: &mut Vec<String>,
) -> Result<()> {
    let kpimg_dst = work_dir.join("kpimg");
    let f = fs::File::open(apk_path)?;
    let mut archive = zip::ZipArchive::new(f)
        .map_err(|e| LtboxError::Patch(format!("{repo}: APK not a zip: {e}")))?;
    let mut entry = archive
        .by_name("assets/kpimg")
        .map_err(|e| LtboxError::Patch(format!("{repo}: APK missing assets/kpimg: {e}")))?;
    let size = entry.size();
    let mut out = fs::File::create(&kpimg_dst)?;
    std::io::copy(&mut entry, &mut out)?;
    log.push(format!(
        "[APatch] extracted assets/kpimg → {} ({} bytes)",
        kpimg_dst.display(),
        size
    ));
    Ok(())
}

/// Fetch APatch/FolkPatch Stable APK → stash at `work_dir/apatch.apk`,
/// extract `assets/kpimg` → `work_dir/kpimg`.
pub fn download_apatch_payload(
    provider: RootProvider,
    work_dir: &Path,
    log: &mut Vec<String>,
) -> Result<String> {
    let repo = provider_repo(provider).ok_or_else(|| {
        LtboxError::Patch(format!(
            "download_apatch_payload: unsupported provider {provider:?}"
        ))
    })?;
    let client = GitHubClient::new(repo)?;
    let (tag, assets) = client.latest_release_assets()?;
    let (name, url) = assets
        .into_iter()
        .find(|(n, _)| n.to_lowercase().ends_with(".apk"))
        .ok_or_else(|| LtboxError::Download(format!("No release APK on latest {repo}")))?;
    log.push(format!("[APatch] {repo} latest: {tag} — asset {name}"));

    let apk_path = work_dir.join("apatch.apk");
    download_to_file(&url, &apk_path, log)?;
    extract_kpimg_from_apk(repo, &apk_path, work_dir, log)?;
    Ok(tag)
}

/// Fetch APatch/FolkPatch Nightly APK via `nightly.link` → extract kpimg.
/// `manual_run_id = None` → latest successful run on provider's workflow.
pub fn download_apatch_payload_nightly(
    provider: RootProvider,
    manual_run_id: Option<u64>,
    work_dir: &Path,
    log: &mut Vec<String>,
) -> Result<u64> {
    let (repo, run_id) = resolve_nightly_run(provider, manual_run_id, log)?;
    let client = GitHubClient::new(repo)?;
    let artifact_names = client.workflow_artifacts(run_id)?;
    if artifact_names.is_empty() {
        return Err(LtboxError::Patch(format!(
            "{repo} run {run_id} has no artifacts"
        )));
    }
    // Case-insensitive prefix match after stripping .zip/.apk.
    let prefix = match provider {
        RootProvider::APatch => "apatch",
        RootProvider::FolkPatch => "folkpatch",
        _ => "",
    };
    let artifact_name = artifact_names
        .iter()
        .find(|n| {
            let lower = n.to_lowercase();
            let stripped = lower
                .strip_suffix(".zip")
                .unwrap_or(&lower)
                .strip_suffix(".apk")
                .unwrap_or_else(|| lower.strip_suffix(".zip").unwrap_or(&lower));
            stripped.starts_with(prefix)
        })
        .cloned()
        .or_else(|| artifact_names.into_iter().next())
        .ok_or_else(|| {
            LtboxError::Patch(format!(
                "{repo} run {run_id}: no matching artifact for prefix {prefix:?}"
            ))
        })?;
    log.push(format!("[APatch] {repo} nightly artifact: {artifact_name}"));
    // Canonical apk path so Stable / Nightly share downstream steps.
    let apk_path = work_dir.join("apatch.apk");
    fetch_nightly_apk_outer_zip(
        "APatch",
        repo,
        run_id,
        &artifact_name,
        "apatch_nightly",
        work_dir,
        &apk_path,
        log,
    )?;
    extract_kpimg_from_apk(repo, &apk_path, work_dir, log)?;
    Ok(run_id)
}

fn copy_apk_to(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        fs::remove_file(dst).ok();
    }
    fs::copy(src, dst)?;
    Ok(())
}

fn extract_first_apk_from_zip(
    archive_path: &Path,
    output_path: &Path,
    log_tag: &str,
    log: &mut Vec<String>,
) -> Result<bool> {
    let f = fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| {
        LtboxError::Patch(format!(
            "{}: APK container not a zip: {e}",
            archive_path.display()
        ))
    })?;
    let member_name = archive
        .file_names()
        .find(|n| n.to_lowercase().ends_with(".apk") && !n.ends_with('/'))
        .map(|s| s.to_string());
    let Some(member_name) = member_name else {
        return Ok(false);
    };
    let mut entry = archive.by_name(&member_name).map_err(|e| {
        LtboxError::Patch(format!(
            "{}: read {member_name}: {e}",
            archive_path.display()
        ))
    })?;
    if output_path.exists() {
        fs::remove_file(output_path).ok();
    }
    let mut out = fs::File::create(output_path)?;
    std::io::copy(&mut entry, &mut out)?;
    log.push(format!(
        "[{log_tag}] extracted manager APK {member_name} -> {}",
        output_path.display()
    ));
    Ok(true)
}

fn stage_manager_from_downloaded_asset(
    asset_path: &Path,
    manager_apk: &Path,
    log_tag: &str,
    log: &mut Vec<String>,
) -> Result<()> {
    if asset_path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("apk"))
    {
        copy_apk_to(asset_path, manager_apk)?;
        log.push(format!(
            "[{log_tag}] staged manager APK: {}",
            manager_apk.display()
        ));
        return Ok(());
    }
    if extract_first_apk_from_zip(asset_path, manager_apk, log_tag, log)? {
        return Ok(());
    }
    Err(LtboxError::Patch(format!(
        "{log_tag}: manager artifact {} did not contain an APK",
        asset_path.display()
    )))
}

fn download_ksu_manager_apk_stable(
    provider: RootProvider,
    work_dir: &Path,
    manager_apk: &Path,
    log: &mut Vec<String>,
) -> Result<String> {
    let repo = provider_repo(provider).ok_or_else(|| {
        LtboxError::Patch(format!(
            "download_ksu_manager_apk: unsupported provider {provider:?}"
        ))
    })?;
    let client = GitHubClient::new(repo)?;
    let (tag, assets) = client.latest_release_assets()?;
    let (name, url) = select_manager_asset(&assets, ksu_manager_asset_preferences(provider))
        .ok_or_else(|| LtboxError::Download(format!("No manager APK artifact on latest {repo}")))?;
    log.push(format!("[KSU] {repo} manager: {tag} -> {name}"));
    let asset_path = work_dir.join(&name);
    download_to_file(&url, &asset_path, log)?;
    stage_manager_from_downloaded_asset(&asset_path, manager_apk, "KSU", log)?;
    Ok(tag)
}

fn download_ksu_manager_apk_nightly(
    provider: RootProvider,
    manual_run_id: Option<u64>,
    work_dir: &Path,
    manager_apk: &Path,
    log: &mut Vec<String>,
) -> Result<u64> {
    let (repo, run_id) = resolve_nightly_run(provider, manual_run_id, log)?;
    let client = GitHubClient::new(repo)?;
    let artifact_names = client.workflow_artifacts(run_id)?;
    let pairs: Vec<(String, String)> = artifact_names
        .iter()
        .map(|name| (name.clone(), String::new()))
        .collect();
    let (artifact_name, _) = select_manager_asset(&pairs, ksu_manager_asset_preferences(provider))
        .ok_or_else(|| {
            LtboxError::Patch(format!(
                "{repo} run {run_id}: no manager APK artifact (got {artifact_names:?})"
            ))
        })?;
    log.push(format!(
        "[KSU] {repo} nightly manager artifact: {artifact_name}"
    ));
    fetch_nightly_apk_outer_zip(
        "KSU",
        repo,
        run_id,
        &artifact_name,
        "ksu_manager_nightly",
        work_dir,
        manager_apk,
        log,
    )?;
    Ok(run_id)
}

/// Stage the manager APK used for post-root control into `work_dir/manager.apk`.
pub fn stage_root_manager_apk(
    cfg: &RootPipelineConfig,
    log: &mut Vec<String>,
) -> Result<Option<PathBuf>> {
    fs::create_dir_all(&cfg.work_dir)?;
    let manager_apk = cfg.work_dir.join("manager.apk");
    if manager_apk.exists() {
        fs::remove_file(&manager_apk).ok();
    }

    if cfg.gki_mode {
        let Some(kernel_zip) = cfg.gki_kernel_zip.as_ref() else {
            return Ok(None);
        };
        return if extract_first_apk_from_zip(kernel_zip, &manager_apk, "GKI", log)? {
            Ok(Some(manager_apk))
        } else {
            log.push("[GKI] No manager APK found in kernel zip; skipping auto-install".into());
            Ok(None)
        };
    }

    match cfg.family {
        RootFamily::Magisk => match (cfg.provider, cfg.version) {
            (RootProvider::MagiskFork, _) => {
                let src = cfg.magisk_forks_apk.as_ref().ok_or_else(|| {
                    LtboxError::Patch("Magisk forks require a local APK — none supplied".into())
                })?;
                copy_apk_to(src, &manager_apk)?;
                log.push(format!(
                    "[Magisk] staged fork manager APK: {}",
                    manager_apk.display()
                ));
            }
            (_, RootVersion::Stable) => {
                download_latest_magisk_apk(cfg.provider, &manager_apk, log)?;
            }
            (_, RootVersion::Nightly) => {
                download_magisk_apk_nightly(
                    cfg.provider,
                    cfg.nightly_run_id,
                    &cfg.work_dir,
                    &manager_apk,
                    log,
                )?;
            }
        },
        RootFamily::KernelSU => match cfg.version {
            RootVersion::Stable => {
                download_ksu_manager_apk_stable(cfg.provider, &cfg.work_dir, &manager_apk, log)?;
            }
            RootVersion::Nightly => {
                download_ksu_manager_apk_nightly(
                    cfg.provider,
                    cfg.nightly_run_id,
                    &cfg.work_dir,
                    &manager_apk,
                    log,
                )?;
            }
        },
        RootFamily::APatch => {
            let apk_path = cfg.work_dir.join("apatch.apk");
            match cfg.version {
                RootVersion::Stable => {
                    download_apatch_payload(cfg.provider, &cfg.work_dir, log)?;
                }
                RootVersion::Nightly => {
                    download_apatch_payload_nightly(
                        cfg.provider,
                        cfg.nightly_run_id,
                        &cfg.work_dir,
                        log,
                    )?;
                }
            }
            copy_apk_to(&apk_path, &manager_apk)?;
            log.push(format!(
                "[APatch] staged manager APK: {}",
                manager_apk.display()
            ));
        }
    }

    Ok(Some(manager_apk))
}

// KSU payload: `.ko` is a release asset (per-kernel), `ksuinit` is a
// workflow artifact fetched via `nightly.link` (GitHub API needs auth).

/// Reduce kernel version to `major.minor` for KSU asset matching
/// (e.g. `6.6.118` → `6.6`). Already-short strings pass through.
pub fn normalize_ksu_kernel_version(kver: &str) -> Option<String> {
    let trimmed = kver.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut parts = trimmed.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    if major.is_empty() || !major.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let minor_digits: String = minor.chars().take_while(|c| c.is_ascii_digit()).collect();
    if minor_digits.is_empty() {
        return None;
    }
    Some(format!("{major}.{minor_digits}"))
}

/// True iff `lower_filename` embeds `kver` between `-{kver}_` delimiters.
/// Prevents unanchored `"6.1"` from matching 6.10 / 6.11 / etc.
fn ksu_ko_kver_matches(lower_filename: &str, kver: &str) -> bool {
    let needle = format!("-{kver}_");
    lower_filename.contains(&needle)
}

fn select_ksu_release_ko_asset(
    assets: &[(String, String)],
    kver: &str,
) -> Option<(String, String)> {
    let want = kver.to_lowercase();
    assets
        .iter()
        .find(|(n, _)| {
            let lower = n.to_lowercase();
            lower.ends_with("_kernelsu.ko") && ksu_ko_kver_matches(&lower, &want)
        })
        .cloned()
}

fn select_ksu_nightly_ko_artifact(artifact_names: &[String], kver: &str) -> Option<String> {
    let want = kver.to_lowercase();
    artifact_names
        .iter()
        .find(|n| {
            let lower = n.to_lowercase();
            lower.contains("_kernelsu.ko") && ksu_ko_kver_matches(&lower, &want)
        })
        .cloned()
}

pub fn download_ksu_payload(
    provider: RootProvider,
    kernel_version: Option<&str>,
    staging_dir: &Path,
    log: &mut Vec<String>,
) -> Result<()> {
    use std::io::{Read, Write};

    let repo = provider_repo(provider)
        .ok_or_else(|| LtboxError::Patch(format!("Unknown KSU provider: {provider:?}")))?;
    let client = GitHubClient::new(repo)?;
    let (tag, assets) = client.latest_release_assets()?;
    live!(log, "[KSU] Latest release: {tag}");

    // -------- 1. Per-kernel `.ko` from release assets --------
    // KSU tags assets by kernel branch (`android15-6.6_kernelsu.ko`);
    // strip patch suffix from device kver before matching.
    let kver = kernel_version
        .and_then(normalize_ksu_kernel_version)
        .ok_or_else(|| {
            LtboxError::Download(
                "KernelSU LKM requires a kernel version such as `6.1`; no safe module fallback is allowed."
                    .into(),
            )
        })?;
    let (ko_name, ko_url) = select_ksu_release_ko_asset(&assets, &kver).ok_or_else(|| {
        LtboxError::Download(format!(
            "No `_kernelsu.ko` release asset on latest {repo} matching kernel `{kver}`."
        ))
    })?;
    live!(log, "[KSU] Downloading LKM: {ko_name}");
    fs::create_dir_all(staging_dir)?;
    let ko_path = staging_dir.join("kernelsu.ko");
    download_to_file(&ko_url, &ko_path, log)?;

    // -------- 2. `ksuinit` binary via nightly.link --------
    let run_id = client.workflow_run_for_tag(&tag).map_err(|e| {
        LtboxError::Download(format!(
            "No workflow run found for tag {tag} on {repo}: {e}"
        ))
    })?;
    let artifacts = client.workflow_artifacts(run_id).map_err(|e| {
        LtboxError::Download(format!("Cannot list artifacts for run {run_id}: {e}"))
    })?;
    let ksuinit_artifact = artifacts
        .iter()
        .find(|n| n.to_lowercase().starts_with("ksuinit"))
        .cloned()
        .ok_or_else(|| {
            LtboxError::Download(format!(
                "No `ksuinit*` workflow artifact on run {run_id} of {repo}"
            ))
        })?;
    let nightly_url = format!(
        "https://nightly.link/{repo}/actions/runs/{run_id}/{ksuinit_artifact}.zip",
        repo = repo,
        run_id = run_id,
        ksuinit_artifact = ksuinit_artifact,
    );
    live!(
        log,
        "[KSU] Downloading ksuinit artifact: {ksuinit_artifact}"
    );
    let tmp_zip = staging_dir.join(format!("{ksuinit_artifact}.zip"));
    download_to_file(&nightly_url, &tmp_zip, log)?;

    let file = fs::File::open(&tmp_zip)
        .map_err(|e| LtboxError::Patch(format!("open ksuinit zip: {e}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| LtboxError::Patch(format!("ksuinit zip read: {e}")))?;
    let member_name: Option<String> = archive
        .file_names()
        .find(|n| n.ends_with("ksuinit") && !n.ends_with('/'))
        .map(|s| s.to_string());
    let member_name = member_name.ok_or_else(|| {
        LtboxError::Patch(format!(
            "`ksuinit` entry missing from {ksuinit_artifact}.zip"
        ))
    })?;
    let mut entry = archive
        .by_name(&member_name)
        .map_err(|e| LtboxError::Patch(format!("ksuinit zip entry: {e}")))?;
    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut buf).map_err(LtboxError::Io)?;
    drop(entry);

    // magiskboot expects `init`, not `ksuinit`.
    let mut out = fs::File::create(staging_dir.join("init"))?;
    out.write_all(&buf)?;
    let _ = fs::remove_file(&tmp_zip);
    live!(log, "[KSU] Staged init ({} bytes) + kernelsu.ko", buf.len());
    Ok(())
}

/// Download `.ko` + `init` from a KSU nightly run into `staging_dir`.
/// LKM selection requires an exact kernel major.minor match.
/// `manual_run_id = None` → latest successful run on provider's workflow.
pub fn download_ksu_payload_nightly(
    provider: RootProvider,
    kernel_version: Option<&str>,
    manual_run_id: Option<u64>,
    staging_dir: &Path,
    log: &mut Vec<String>,
) -> Result<u64> {
    use std::io::{Read, Write};

    let (repo, run_id) = resolve_nightly_run(provider, manual_run_id, log)?;
    let client = GitHubClient::new(repo)?;
    let artifact_names = client.workflow_artifacts(run_id)?;
    if artifact_names.is_empty() {
        return Err(LtboxError::Patch(format!(
            "{repo} run {run_id} has no artifacts"
        )));
    }

    fs::create_dir_all(staging_dir)?;
    let kver = kernel_version
        .and_then(normalize_ksu_kernel_version)
        .ok_or_else(|| {
            LtboxError::Patch(
                "KernelSU Nightly LKM requires a kernel version such as `6.1`; no safe module fallback is allowed."
                    .into(),
            )
        })?;

    // -------- 1. Kernel `.ko` --------
    let ko_artifact = select_ksu_nightly_ko_artifact(&artifact_names, &kver).ok_or_else(|| {
        LtboxError::Patch(format!(
            "{repo} run {run_id}: no *_kernelsu.ko artifact matching kernel {kver} (artifacts={artifact_names:?})"
        ))
    })?;
    live!(log, "[KSU] nightly LKM artifact: {ko_artifact}");
    let ko_zip_path = staging_dir.join("ksu_nightly_lkm.zip");
    let ko_url = nightly_artifact_url(repo, run_id, &ko_artifact);
    download_to_file(&ko_url, &ko_zip_path, log)?;
    {
        let f = fs::File::open(&ko_zip_path)?;
        let mut archive = zip::ZipArchive::new(f)
            .map_err(|e| LtboxError::Patch(format!("{repo}: LKM artifact not a zip: {e}")))?;
        // First `.ko` entry → staging_dir/kernelsu.ko.
        let member_name: String = archive
            .file_names()
            .find(|n| n.to_lowercase().ends_with(".ko"))
            .map(|s| s.to_string())
            .ok_or_else(|| {
                LtboxError::Patch(format!("{repo} {ko_artifact}: no .ko entry in zip"))
            })?;
        let mut entry = archive
            .by_name(&member_name)
            .map_err(|e| LtboxError::Patch(format!("{repo} {ko_artifact}: {e}")))?;
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        drop(entry);
        fs::write(staging_dir.join("kernelsu.ko"), &buf)?;
    }
    let _ = fs::remove_file(&ko_zip_path);

    // -------- 2. ksuinit → `init` --------
    let init_artifact = artifact_names
        .iter()
        .find(|n| n.to_lowercase().starts_with("ksuinit"))
        .cloned()
        .ok_or_else(|| {
            LtboxError::Patch(format!(
                "{repo} run {run_id}: no ksuinit artifact (got {artifact_names:?})"
            ))
        })?;
    live!(log, "[KSU] nightly ksuinit artifact: {init_artifact}");
    let init_zip_path = staging_dir.join("ksu_nightly_init.zip");
    let init_url = nightly_artifact_url(repo, run_id, &init_artifact);
    download_to_file(&init_url, &init_zip_path, log)?;
    {
        let f = fs::File::open(&init_zip_path)?;
        let mut archive = zip::ZipArchive::new(f)
            .map_err(|e| LtboxError::Patch(format!("{repo}: ksuinit artifact not a zip: {e}")))?;
        let member_name: String = archive
            .file_names()
            .find(|n| n.ends_with("ksuinit") && !n.ends_with('/'))
            .map(|s| s.to_string())
            .ok_or_else(|| {
                LtboxError::Patch(format!("{repo} {init_artifact}: no ksuinit entry in zip"))
            })?;
        let mut entry = archive
            .by_name(&member_name)
            .map_err(|e| LtboxError::Patch(format!("{repo} {init_artifact}: {e}")))?;
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        drop(entry);
        let mut out = fs::File::create(staging_dir.join("init"))?;
        out.write_all(&buf)?;
    }
    let _ = fs::remove_file(&init_zip_path);
    live!(log, "[KSU] staged nightly init + kernelsu.ko");
    Ok(run_id)
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
    log: &mut Vec<String>,
) -> Result<PatchedArtifacts> {
    // Guard provider/version combos not wired yet.
    {}

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
    if !vbmeta_src.exists() {
        return Err(LtboxError::Patch(
            "work_dir is missing the stock vbmeta.img dump".into(),
        ));
    }
    let staged_manager_apk = cfg.work_dir.join("manager.apk");
    if !cfg.gki_mode && !staged_manager_apk.exists() {
        stage_root_manager_apk(cfg, log)?;
    }

    let patched_boot = if cfg.gki_mode {
        // GKI: swap kernel blob from user's AnyKernel3 zip — no GitHub fetch.
        let kernel_zip = cfg.gki_kernel_zip.as_ref().ok_or_else(|| {
            LtboxError::Patch("GKI mode requires a custom kernel zip — none supplied".into())
        })?;
        live!(log, "[GKI] Kernel zip: {}", kernel_zip.display());
        gki::patch_boot(&cfg.work_dir, kernel_zip, log)?
    } else {
        match cfg.family {
            RootFamily::Magisk => {
                let apk_path = cfg.work_dir.join("magisk.apk");
                match (cfg.provider, cfg.version) {
                    (RootProvider::MagiskFork, _) => {
                        let src = cfg.magisk_forks_apk.as_ref().ok_or_else(|| {
                            LtboxError::Patch(
                                "Magisk forks require a local APK — none supplied".into(),
                            )
                        })?;
                        if !src.exists() {
                            return Err(LtboxError::Patch(format!(
                                "Magisk forks APK does not exist: {}",
                                src.display()
                            )));
                        }
                        live!(
                            log,
                            "[Magisk] Staging user-supplied forks APK: {}",
                            src.display()
                        );
                        fs::copy(src, &apk_path).map_err(|e| {
                            LtboxError::Patch(format!("Failed to stage forks APK: {e}"))
                        })?;
                    }
                    (_, RootVersion::Stable) => {
                        live!(log, "[Magisk] Fetching latest APK from topjohnwu/Magisk…");
                        download_latest_magisk_apk(cfg.provider, &apk_path, log)?;
                    }
                    (_, RootVersion::Nightly) => {
                        live!(
                            log,
                            "[Magisk] Fetching Nightly APK (run_id={:?}) from topjohnwu/Magisk…",
                            cfg.nightly_run_id
                        );
                        download_magisk_apk_nightly(
                            cfg.provider,
                            cfg.nightly_run_id,
                            &cfg.work_dir,
                            &apk_path,
                            log,
                        )?;
                    }
                }
                live!(
                    log,
                    "[Magisk] Extracting payload from APK (magisk, magiskinit, init-ld, stub.apk)"
                );
                magisk::extract_apk_payload(&apk_path, &cfg.work_dir)?;
                live!(log, "[Magisk] Patching init_boot.img ramdisk…");
                magisk::patch_init_boot(&cfg.work_dir, &cfg.preinit_device, log)?
            }
            RootFamily::KernelSU => {
                match cfg.version {
                    RootVersion::Stable => {
                        live!(log, "[KSU] Fetching latest Stable LKM zip from GitHub…");
                        download_ksu_payload(
                            cfg.provider,
                            cfg.kernel_version.as_deref(),
                            &cfg.work_dir,
                            log,
                        )?;
                    }
                    RootVersion::Nightly => {
                        live!(
                            log,
                            "[KSU] Fetching Nightly payload (run_id={:?})…",
                            cfg.nightly_run_id
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
                live!(
                    log,
                    "[KSU] Patching init_boot.img — swapping init + staging kernelsu.ko…"
                );
                ksu::patch_init_boot(&cfg.work_dir, log)?
            }
            RootFamily::APatch => {
                match cfg.version {
                    RootVersion::Stable => {
                        live!(log, "[APatch] Fetching Stable APK + extracting kpimg…");
                        download_apatch_payload(cfg.provider, &cfg.work_dir, log)?;
                    }
                    RootVersion::Nightly => {
                        live!(
                            log,
                            "[APatch] Fetching Nightly artifact (run_id={:?}) + extracting kpimg…",
                            cfg.nightly_run_id
                        );
                        download_apatch_payload_nightly(
                            cfg.provider,
                            cfg.nightly_run_id,
                            &cfg.work_dir,
                            log,
                        )?;
                    }
                }
                live!(
                    log,
                    "[APatch] Patching boot.img via kptools-rs (kpm_count={}, superkey_len={})",
                    cfg.kpm_paths.len(),
                    cfg.superkey.len()
                );
                crate::apatch::patch_boot(&cfg.work_dir, &cfg.kpm_paths, &cfg.superkey, log)?
            }
        }
    };

    let final_boot = cfg.output_dir.join(stock_filename);
    if final_boot.exists() {
        fs::remove_file(&final_boot).ok();
    }
    fs::rename(&patched_boot, &final_boot)?;
    log.push(format!(
        "[Root] {} {} {} {}",
        tr("log_root_patched"),
        stock_filename,
        tr("log_root_ready_at"),
        final_boot.display()
    ));

    // Re-apply AVB footer. Algorithm + rollback index copied from stock to
    // preserve device's rollback state. Signing key via `KEY_MAP` on stock pubkey.
    let stock_info = avb::extract_image_avb_info(&stock_boot_src)?;
    let boot_key = resolve_signing_key(stock_info.public_key_sha1.as_deref(), stock_filename, log)?;
    avb::erase_footer(&final_boot).ok();
    avb::add_hash_footer(
        &final_boot,
        &stock_info,
        boot_key.as_deref(),
        Some(stock_info.rollback_index),
    )?;
    log.push(format!(
        "[AVB] {} {} ({} rollback={}, key={})",
        tr("log_avb_refootered"),
        stock_filename,
        stock_info.algorithm,
        stock_info.rollback_index,
        boot_key.as_deref().unwrap_or("(unsigned)"),
    ));

    // Rebuild vbmeta with fresh hash descriptor. vbmeta pubkey may differ
    // from boot pubkey — second `KEY_MAP` lookup against the stock vbmeta.
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
            log.push(format!(
                "[AVB] {} {} at {} (key={key})",
                tr("log_avb_rebuilt_vbmeta"),
                stock_filename,
                final_vbmeta.display(),
            ));
        }
        None => {
            // Unsigned vbmeta: copy stock through. Stale chain hash is fine
            // since NONE-algorithm bootloaders skip verification.
            fs::copy(&vbmeta_src, &final_vbmeta)?;
            log.push(format!(
                "[AVB] {} {}",
                tr("log_avb_vbmeta_unsigned_copied"),
                final_vbmeta.display(),
            ));
        }
    }

    // Empty slot suffix → default to `_a`.
    let suffix = if cfg.slot_suffix.is_empty() {
        "_a".to_string()
    } else {
        cfg.slot_suffix.clone()
    };

    Ok(PatchedArtifacts {
        patched_boot: final_boot,
        patched_vbmeta: Some(final_vbmeta),
        manager_apk: staged_manager_apk.exists().then_some(staged_manager_apk),
        boot_partition: format!("{base_part}{suffix}"),
        vbmeta_partition: Some(format!("vbmeta{suffix}")),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ksu_ko_kver_matches, normalize_ksu_kernel_version, select_ksu_nightly_ko_artifact,
        select_ksu_release_ko_asset, select_manager_asset,
    };

    #[test]
    fn exact_major_minor_matches() {
        assert!(ksu_ko_kver_matches("android15-6.1_kernelsu.ko", "6.1"));
        assert!(ksu_ko_kver_matches("android14-5.15_kernelsu.ko", "5.15"));
    }

    #[test]
    fn longer_minor_does_not_match_shorter_prefix() {
        // Regression: unanchored `contains("6.1")` used to match 6.10/6.11/etc.
        assert!(!ksu_ko_kver_matches("android15-6.10_kernelsu.ko", "6.1"));
        assert!(!ksu_ko_kver_matches("android15-6.11_kernelsu.ko", "6.1"));
        assert!(!ksu_ko_kver_matches("android15-6.12_kernelsu.ko", "6.1"));
        assert!(!ksu_ko_kver_matches("android15-6.13_kernelsu.ko", "6.1"));
    }

    #[test]
    fn different_major_does_not_match() {
        assert!(!ksu_ko_kver_matches("android15-5.15_kernelsu.ko", "6.1"));
        assert!(!ksu_ko_kver_matches("android14-6.1_kernelsu.ko", "5.15"));
    }

    #[test]
    fn missing_leading_dash_does_not_match() {
        // `-{kver}_` boundary is required; bare `6.1_kernelsu.ko` is not a stock layout.
        assert!(!ksu_ko_kver_matches("6.1_kernelsu.ko", "6.1"));
    }

    #[test]
    fn ksu_kernel_version_normalizes_to_major_minor() {
        assert_eq!(normalize_ksu_kernel_version("6.1"), Some("6.1".to_string()));
        assert_eq!(
            normalize_ksu_kernel_version("6.1.75"),
            Some("6.1".to_string())
        );
        assert_eq!(
            normalize_ksu_kernel_version("  5.15.149-android14  "),
            Some("5.15".to_string())
        );
    }

    #[test]
    fn ksu_kernel_version_rejects_missing_or_malformed_input() {
        assert_eq!(normalize_ksu_kernel_version(""), None);
        assert_eq!(normalize_ksu_kernel_version("6"), None);
        assert_eq!(normalize_ksu_kernel_version("six.one"), None);
    }

    #[test]
    fn ksu_release_asset_selection_requires_matching_kernel() {
        let assets = vec![
            (
                "android14-5.15_kernelsu.ko".to_string(),
                "https://example.invalid/5.15.ko".to_string(),
            ),
            (
                "android15-6.6_kernelsu.ko".to_string(),
                "https://example.invalid/6.6.ko".to_string(),
            ),
        ];

        let picked = select_ksu_release_ko_asset(&assets, "6.6").expect("6.6 asset");
        assert_eq!(picked.0, "android15-6.6_kernelsu.ko");
        assert!(select_ksu_release_ko_asset(&assets, "6.1").is_none());
    }

    #[test]
    fn ksu_nightly_artifact_selection_does_not_fallback_to_any_module() {
        let artifacts = vec![
            "android14-5.15_kernelsu.ko".to_string(),
            "ksuinit-arm64.zip".to_string(),
        ];

        assert_eq!(
            select_ksu_nightly_ko_artifact(&artifacts, "5.15"),
            Some("android14-5.15_kernelsu.ko".to_string())
        );
        assert_eq!(select_ksu_nightly_ko_artifact(&artifacts, "6.1"), None);
    }

    #[test]
    fn ksu_manager_asset_selection_prefers_provider_names() {
        let assets = vec![
            (
                "random-debug.apk".to_string(),
                "https://example.invalid/debug.apk".to_string(),
            ),
            (
                "manager-spoofed.zip".to_string(),
                "https://example.invalid/manager-spoofed.zip".to_string(),
            ),
            (
                "manager.zip".to_string(),
                "https://example.invalid/manager.zip".to_string(),
            ),
        ];

        let picked = select_manager_asset(&assets, &["manager-spoofed.zip", "manager.zip"])
            .expect("manager asset");
        assert_eq!(picked.0, "manager-spoofed.zip");
    }
}
