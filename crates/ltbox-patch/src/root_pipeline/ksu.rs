//! KernelSU-family (KernelSU / KSU-Next / SukiSU / ReSukiSU) manager APK,
//! kernel `.ko`, and `ksuinit` payload acquisition.
//!
//! Also hosts the manager-APK orchestration entry point
//! [`stage_root_manager_apk`], which dispatches across all root families
//! and lives here because the KSU branches are the bulk of its logic.

use std::path::{Path, PathBuf};

use fs_err as fs;

use ltbox_core::downloader::download_to_file;
use ltbox_core::github::GitHubClient;
use ltbox_core::i18n::tr;
use ltbox_core::{LtboxError, Result, tr_args};

use super::apatch::{download_apatch_payload, download_apatch_payload_nightly};
use super::apk::{
    copy_apk_to, extract_first_apk_from_zip, ksu_manager_nightly_preferences,
    ksu_manager_stable_preferences, select_manager_asset, stage_manager_from_downloaded_asset,
};
use super::magisk::{
    download_latest_magisk_apk, download_magisk_apk_nightly, fetch_nightly_apk_outer_zip,
};
use super::{
    RootFamily, RootPipelineConfig, RootProvider, RootVersion, nightly_artifact_url, provider_repo,
    resolve_nightly_run,
};

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
    let (name, url) = select_manager_asset(&assets, ksu_manager_stable_preferences(provider))
        .ok_or_else(|| LtboxError::Download(format!("No manager APK artifact on latest {repo}")))?;
    ltbox_core::live!(
        log,
        "[KSU] {repo} {}",
        tr_args!("log_release_latest_asset", tag = tag, name = name)
    );
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
    let (artifact_name, _) =
        select_manager_asset(&pairs, ksu_manager_nightly_preferences(provider)).ok_or_else(
            || {
                LtboxError::Patch(format!(
                    "{repo} run {run_id}: no manager APK artifact (got {artifact_names:?})"
                ))
            },
        )?;
    ltbox_core::live!(
        log,
        "[KSU] {repo} {}",
        tr_args!("log_nightly_artifact", artifact = artifact_name)
    );
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
            ltbox_core::live!(log, "[GKI] {}", tr("log_gki_no_manager_apk"));
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
                ltbox_core::live!(
                    log,
                    "[Magisk] {}",
                    tr_args!("log_magisk_staged_fork_apk", path = manager_apk.display())
                );
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
            ltbox_core::live!(
                log,
                "[APatch] {}",
                tr_args!("log_staged_manager_apk", path = manager_apk.display())
            );
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
    // Accept legacy `_kernelsu.ko` and current `-{kver}-lkm` naming.
    // Trailing `-`/EOS sentinel prevents `6.1` matching `6.10/6.11/6.12`.
    let want = kver.to_lowercase();
    let lkm_marker = format!("-{want}-lkm");
    artifact_names
        .iter()
        .find(|n| {
            let lower = n.to_lowercase();
            // Legacy: "*-{kver}_kernelsu.ko"
            if lower.contains("_kernelsu.ko") && ksu_ko_kver_matches(&lower, &want) {
                return true;
            }
            // Current: "android<api>-{kver}-lkm" (zip wrapper, real
            // .ko inside).
            lower.contains(&lkm_marker)
        })
        .cloned()
}

pub fn download_ksu_payload(
    provider: RootProvider,
    kernel_version: Option<&str>,
    staging_dir: &Path,
    log: &mut Vec<String>,
) -> Result<()> {
    let repo = provider_repo(provider)
        .ok_or_else(|| LtboxError::Patch(format!("Unknown KSU provider: {provider:?}")))?;
    let client = GitHubClient::new(repo)?;
    let (tag, assets) = client.latest_release_assets()?;
    ltbox_core::live!(
        log,
        "[KSU] {}",
        tr_args!("log_ksu_latest_release", tag = tag)
    );

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
    ltbox_core::live!(
        log,
        "[KSU] {}",
        tr_args!("log_ksu_downloading_lkm", name = ko_name)
    );
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
    ltbox_core::live!(
        log,
        "[KSU] {}",
        tr_args!("log_ksu_downloading_ksuinit", name = ksuinit_artifact)
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
    // magiskboot expects `init`, not `ksuinit`.
    let init_path = staging_dir.join("init");
    let copied = crate::zip_util::copy_capped(
        &mut entry,
        &init_path,
        crate::zip_util::MAX_ENTRY_BYTES,
        &member_name,
    )?;
    drop(entry);
    let _ = fs::remove_file(&tmp_zip);
    ltbox_core::live!(
        log,
        "[KSU] {}",
        tr_args!("log_ksu_staged_init_lkm", bytes = copied)
    );
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
    ltbox_core::live!(
        log,
        "[KSU] {}",
        tr_args!("log_ksu_nightly_lkm_artifact", artifact = ko_artifact)
    );
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
        let ko_path = staging_dir.join("kernelsu.ko");
        crate::zip_util::copy_capped(
            &mut entry,
            &ko_path,
            crate::zip_util::MAX_ENTRY_BYTES,
            &member_name,
        )?;
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
    ltbox_core::live!(
        log,
        "[KSU] {}",
        tr_args!("log_ksu_nightly_ksuinit_artifact", artifact = init_artifact)
    );
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
        let init_path = staging_dir.join("init");
        crate::zip_util::copy_capped(
            &mut entry,
            &init_path,
            crate::zip_util::MAX_ENTRY_BYTES,
            &member_name,
        )?;
    }
    let _ = fs::remove_file(&init_zip_path);
    ltbox_core::live!(log, "[KSU] {}", tr("log_ksu_staged_nightly_init"));
    Ok(run_id)
}

#[cfg(test)]
mod tests {
    use super::{
        RootProvider, download_ksu_manager_apk_nightly, download_ksu_manager_apk_stable,
        download_ksu_payload, download_ksu_payload_nightly, ksu_ko_kver_matches,
        normalize_ksu_kernel_version, select_ksu_nightly_ko_artifact, select_ksu_release_ko_asset,
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
    fn ksu_nightly_artifact_selection_picks_new_lkm_naming() {
        // Real artifact list emitted by 2026 KernelSU / KSU-Next /
        // SukiSU / ReSukiSU nightlies — bare `<branch>-<kver>-lkm`
        // wrapper instead of the old `*_kernelsu.ko` filename.
        let artifacts = vec![
            "manager".to_string(),
            "ksud-aarch64-linux-android".to_string(),
            "android16-6.12-lkm".to_string(),
            "android15-6.6-lkm".to_string(),
            "android14-5.15-lkm".to_string(),
            "android14-6.1-lkm".to_string(),
            "android13-5.10-lkm".to_string(),
            "ksuinit".to_string(),
        ];

        assert_eq!(
            select_ksu_nightly_ko_artifact(&artifacts, "6.6"),
            Some("android15-6.6-lkm".to_string())
        );
        assert_eq!(
            select_ksu_nightly_ko_artifact(&artifacts, "5.15"),
            Some("android14-5.15-lkm".to_string())
        );
        // 6.1 must not steal 6.10 / 6.11 / 6.12 — kver match anchors
        // both sides via the surrounding `-` markers.
        assert_eq!(
            select_ksu_nightly_ko_artifact(&artifacts, "6.1"),
            Some("android14-6.1-lkm".to_string())
        );
        // No 4.x in this artifact set.
        assert_eq!(select_ksu_nightly_ko_artifact(&artifacts, "4.14"), None);
    }

    /// Network-dependent end-to-end probe of every LKM provider's
    /// manager-APK fetch path (Stable + Nightly auto). Each iteration
    /// uses an isolated tempdir so failures don't poison subsequent
    /// runs. Marked `#[ignore]` so CI / `cargo test` skip it; run
    /// locally with:
    ///
    ///     cargo test -p ltbox-patch --lib -- --ignored --nocapture lkm_manager_download_smoke
    ///
    /// Pass criteria per provider/channel:
    /// 1. Function returns `Ok(_)`.
    /// 2. `manager.apk` exists at the expected path.
    /// 3. The file is non-empty (full APK download / extraction).
    #[test]
    #[ignore = "hits GitHub releases + nightly.link; run manually"]
    fn lkm_manager_download_smoke() {
        let providers: &[(RootProvider, &str)] = &[
            (RootProvider::KernelSU, "tiann/KernelSU"),
            (RootProvider::KernelSUNext, "KernelSU-Next/KernelSU-Next"),
            (RootProvider::SukiSU, "SukiSU-Ultra/SukiSU-Ultra"),
            (RootProvider::ReSukiSU, "ReSukiSU/ReSukiSU"),
        ];

        let mut report: Vec<(String, String)> = Vec::new();

        for (provider, repo) in providers.iter().copied() {
            // ----- Stable -----
            let stable_label = format!("{repo} stable");
            // ReSukiSU has no Stable releases — expect Err.
            if matches!(provider, RootProvider::ReSukiSU) {
                report.push((
                    stable_label.clone(),
                    "skipped (no Stable channel)".to_string(),
                ));
            } else {
                let tmp = tempfile::tempdir().expect("tempdir");
                let manager_apk = tmp.path().join("manager.apk");
                let mut log = Vec::new();
                let result =
                    download_ksu_manager_apk_stable(provider, tmp.path(), &manager_apk, &mut log);
                let outcome = match result {
                    Ok(tag) => match (
                        manager_apk.exists(),
                        std::fs::metadata(&manager_apk)
                            .map(|m| m.len())
                            .unwrap_or(0),
                    ) {
                        (true, n) if n > 0 => format!("OK tag={tag} size={n}"),
                        (true, _) => "FAIL: manager.apk empty".to_string(),
                        (false, _) => "FAIL: manager.apk missing".to_string(),
                    },
                    Err(e) => format!("FAIL: {e}"),
                };
                eprintln!("[{stable_label}] {outcome}");
                report.push((stable_label, outcome));
            }

            // ----- Nightly auto-detect -----
            let nightly_label = format!("{repo} nightly");
            let tmp = tempfile::tempdir().expect("tempdir");
            let manager_apk = tmp.path().join("manager.apk");
            let mut log = Vec::new();
            let result = download_ksu_manager_apk_nightly(
                provider,
                None,
                tmp.path(),
                &manager_apk,
                &mut log,
            );
            let outcome = match result {
                Ok(run_id) => match (
                    manager_apk.exists(),
                    std::fs::metadata(&manager_apk)
                        .map(|m| m.len())
                        .unwrap_or(0),
                ) {
                    (true, n) if n > 0 => format!("OK run={run_id} size={n}"),
                    (true, _) => "FAIL: manager.apk empty".to_string(),
                    (false, _) => "FAIL: manager.apk missing".to_string(),
                },
                Err(e) => format!("FAIL: {e}"),
            };
            eprintln!("[{nightly_label}] {outcome}");
            report.push((nightly_label, outcome));
        }

        eprintln!("\n=== LKM manager-APK download report ===");
        for (label, outcome) in &report {
            eprintln!("  {label}: {outcome}");
        }
        eprintln!();

        let failures: Vec<&(String, String)> = report
            .iter()
            .filter(|(_, o)| o.starts_with("FAIL"))
            .collect();
        assert!(
            failures.is_empty(),
            "{} provider/channel combinations failed: {:#?}",
            failures.len(),
            failures
        );
    }

    /// Network-dependent probe for the full `download_ksu_payload`
    /// path — `.ko` (kernel module) + `ksuinit` artifact extraction —
    /// against kernel `6.6` for every KSU-family provider that ships
    /// release artifacts.
    ///
    ///     cargo test -p ltbox-patch --lib -- --ignored --nocapture lkm_payload_download_smoke
    #[test]
    #[ignore = "hits GitHub releases + nightly.link; run manually"]
    fn lkm_payload_download_smoke() {
        const KVER: &str = "6.6";
        let providers: &[(RootProvider, &str)] = &[
            (RootProvider::KernelSU, "tiann/KernelSU"),
            (RootProvider::KernelSUNext, "KernelSU-Next/KernelSU-Next"),
            (RootProvider::SukiSU, "SukiSU-Ultra/SukiSU-Ultra"),
        ];

        let mut report: Vec<(String, String)> = Vec::new();

        for (provider, repo) in providers.iter().copied() {
            let label = format!("{repo} payload k{KVER}");
            let tmp = tempfile::tempdir().expect("tempdir");
            let mut log = Vec::new();
            let result = download_ksu_payload(provider, Some(KVER), tmp.path(), &mut log);
            let outcome = match result {
                Ok(()) => {
                    let ko = tmp.path().join("kernelsu.ko");
                    let init = tmp.path().join("init");
                    let ko_n = std::fs::metadata(&ko).map(|m| m.len()).unwrap_or(0);
                    let init_n = std::fs::metadata(&init).map(|m| m.len()).unwrap_or(0);
                    if ko.exists() && ko_n > 0 && init.exists() && init_n > 0 {
                        format!("OK ko={ko_n} init={init_n}")
                    } else {
                        format!(
                            "FAIL: ko_exists={} ko_size={} init_exists={} init_size={}",
                            ko.exists(),
                            ko_n,
                            init.exists(),
                            init_n
                        )
                    }
                }
                Err(e) => format!("FAIL: {e}"),
            };
            eprintln!("[{label}] {outcome}");
            report.push((label, outcome));
        }

        eprintln!("\n=== LKM payload download report ===");
        for (label, outcome) in &report {
            eprintln!("  {label}: {outcome}");
        }
        eprintln!();

        let failures: Vec<&(String, String)> = report
            .iter()
            .filter(|(_, o)| o.starts_with("FAIL"))
            .collect();
        assert!(
            failures.is_empty(),
            "{} provider payloads failed: {:#?}",
            failures.len(),
            failures
        );
    }

    /// Nightly counterpart to `lkm_payload_download_smoke` — exercises
    /// `download_ksu_payload_nightly` so the per-kernel `.ko` artifact
    /// selection + ksuinit extraction get checked against every
    /// provider's actual nightly run, including ReSukiSU which has no
    /// Stable channel and is the only path that's actually used in
    /// production for that fork.
    ///
    ///     cargo test -p ltbox-patch --lib -- --ignored --nocapture lkm_payload_nightly_download_smoke
    #[test]
    #[ignore = "hits GitHub releases + nightly.link; run manually"]
    fn lkm_payload_nightly_download_smoke() {
        const KVER: &str = "6.6";
        let providers: &[(RootProvider, &str)] = &[
            (RootProvider::KernelSU, "tiann/KernelSU"),
            (RootProvider::KernelSUNext, "KernelSU-Next/KernelSU-Next"),
            (RootProvider::SukiSU, "SukiSU-Ultra/SukiSU-Ultra"),
            (RootProvider::ReSukiSU, "ReSukiSU/ReSukiSU"),
        ];

        let mut report: Vec<(String, String)> = Vec::new();

        for (provider, repo) in providers.iter().copied() {
            let label = format!("{repo} nightly payload k{KVER}");
            let tmp = tempfile::tempdir().expect("tempdir");
            let mut log = Vec::new();
            let result =
                download_ksu_payload_nightly(provider, Some(KVER), None, tmp.path(), &mut log);
            let outcome = match result {
                Ok(run_id) => {
                    let ko = tmp.path().join("kernelsu.ko");
                    let init = tmp.path().join("init");
                    let ko_n = std::fs::metadata(&ko).map(|m| m.len()).unwrap_or(0);
                    let init_n = std::fs::metadata(&init).map(|m| m.len()).unwrap_or(0);
                    if ko.exists() && ko_n > 0 && init.exists() && init_n > 0 {
                        format!("OK run={run_id} ko={ko_n} init={init_n}")
                    } else {
                        format!(
                            "FAIL: ko_exists={} ko_size={} init_exists={} init_size={}",
                            ko.exists(),
                            ko_n,
                            init.exists(),
                            init_n
                        )
                    }
                }
                Err(e) => format!("FAIL: {e}"),
            };
            eprintln!("[{label}] {outcome}");
            report.push((label, outcome));
        }

        eprintln!("\n=== LKM nightly payload download report ===");
        for (label, outcome) in &report {
            eprintln!("  {label}: {outcome}");
        }
        eprintln!();

        let failures: Vec<&(String, String)> = report
            .iter()
            .filter(|(_, o)| o.starts_with("FAIL"))
            .collect();
        assert!(
            failures.is_empty(),
            "{} nightly payloads failed: {:#?}",
            failures.len(),
            failures
        );
    }
}
