//! Qualcomm 9008 EDL USB driver detection + auto-install on Windows.
//!
//! Needs `qcadb.inf` (ADB composite) and `qcwdfser.inf` (WDF serial) from
//! `qualcomm/qcom-usb-kernel-drivers` releases. Presence probed via
//! `pnputil /enum-drivers`, then the DriverStore FileRepository as fallback.
//! Install: download → extract → `pnputil /add-driver` per `.inf`.
//!
//! Cross-platform `DriverStatus` / `DriverError` / `Result` types
//! live in `driver/mod.rs`; this file is only compiled on Windows
//! (gated by `#[cfg(windows)]` in `driver/mod.rs`) so every
//! `cfg!(windows)` runtime check from the pre-rename module folds
//! into compile-time guarantees here.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{DriverError, DriverStatus, Result};

/// `Command::new` + `CREATE_NO_WINDOW` so `pnputil` does not flash a console.
fn silent_command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

const REQUIRED_INFS: &[&str] = &["qcadb.inf", "qcwdfser.inf"];

#[derive(Debug, serde::Deserialize)]
struct GithubRelease {
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

const RELEASES_API: &str =
    "https://api.github.com/repos/qualcomm/qcom-usb-kernel-drivers/releases/latest";
const ASSET_PREFIX: &str = "qud-win-";
const ASSET_SUFFIX: &str = "_arm64_amd64.zip";
const USER_AGENT: &str = concat!("ltbox/", env!("CARGO_PKG_VERSION"));

/// Probe whether the Qualcomm USB drivers are installed.
pub fn check_required_drivers() -> DriverStatus {
    let missing: Vec<&'static str> = REQUIRED_INFS
        .iter()
        .copied()
        .filter(|inf| !is_driver_present(inf))
        .collect();

    if missing.is_empty() {
        DriverStatus::Present
    } else {
        DriverStatus::Missing(missing)
    }
}

fn is_driver_present(inf_name: &str) -> bool {
    driver_present_via_pnputil(inf_name) || driver_present_via_driver_store(inf_name)
}

fn driver_present_via_pnputil(inf_name: &str) -> bool {
    let output = match silent_command("pnputil").arg("/enum-drivers").output() {
        Ok(o) => o,
        Err(_) => return false,
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let target = inf_name.to_ascii_lowercase();
    stdout.lines().any(|line| {
        if let Some((_, v)) = line.split_once(':') {
            v.trim().to_ascii_lowercase() == target
        } else {
            false
        }
    })
}

fn driver_present_via_driver_store(inf_name: &str) -> bool {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let repo = Path::new(&system_root)
        .join("System32")
        .join("DriverStore")
        .join("FileRepository");
    let Ok(entries) = std::fs::read_dir(&repo) else {
        return false;
    };
    let prefix = inf_name.to_ascii_lowercase();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with(&prefix) {
            return true;
        }
    }
    false
}

/// Download the latest `qcom-usb-kernel-drivers` release and `pnputil`-install
/// each `.inf` under a `Windows10/` folder. `log` is pushed per milestone.
pub fn download_and_install(log: &mut Vec<String>) -> Result<()> {
    log.push("[Driver] Fetching latest release metadata...".to_string());
    // Shorter timeout than core::build_agent — zip is <10 MB; bail fast.
    let agent = ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .new_agent();

    let release: GithubRelease = agent
        .get(RELEASES_API)
        .call()?
        .body_mut()
        .read_json()
        .map_err(|e| DriverError::Parse(e.to_string()))?;

    let (asset_name, asset_url) = release
        .assets
        .into_iter()
        .find(|a| a.name.starts_with(ASSET_PREFIX) && a.name.ends_with(ASSET_SUFFIX))
        .map(|a| (a.name, a.browser_download_url))
        .ok_or(DriverError::NoAsset)?;

    log.push(format!("[Driver] Asset: {asset_name}"));

    let tmp_dir = std::env::temp_dir().join(format!("ltbox_qcom_drv_{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)?;
    let zip_path = tmp_dir.join(&asset_name);

    log.push("[Driver] Downloading...".to_string());
    {
        let mut resp = agent.get(&asset_url).call()?;
        let mut reader = resp.body_mut().as_reader();
        let mut file = std::fs::File::create(&zip_path)?;
        std::io::copy(&mut reader, &mut file)?;
    }

    log.push("[Driver] Extracting archive...".to_string());
    let extract_dir = tmp_dir.join("extracted");
    std::fs::create_dir_all(&extract_dir)?;
    extract_zip(&zip_path, &extract_dir)?;

    let mut inf_files: Vec<PathBuf> = Vec::new();
    walk_collect_infs(&extract_dir, &mut inf_files);
    if inf_files.is_empty() {
        cleanup(&tmp_dir);
        return Err(DriverError::NoInf);
    }

    for inf in &inf_files {
        let name = inf
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        log.push(format!("[Driver] Installing {name}..."));
        let out = silent_command("pnputil")
            .arg("/add-driver")
            .arg(inf)
            .arg("/install")
            .output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                log.push(format!("[Driver] pnputil {name} failed: {stderr}"));
            }
            Err(e) => log.push(format!("[Driver] pnputil {name} spawn failed: {e}")),
        }
    }

    log.push("[Driver] Installation finished. Reboot is recommended.".to_string());
    cleanup(&tmp_dir);
    Ok(())
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out_path = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out_file = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out_file)?;
        }
    }
    Ok(())
}

fn walk_collect_infs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_collect_infs(&path, out);
        } else if let Some(ext) = path.extension()
            && ext.eq_ignore_ascii_case("inf")
            && path.components().any(|c| {
                c.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("Windows10")
            })
        {
            out.push(path);
        }
    }
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Missing(...)` must never carry an empty vec — empty list
    /// would make the GUI banner say "missing nothing" which is
    /// confusing.
    #[test]
    fn missing_list_is_empty_when_all_present() {
        if let DriverStatus::Missing(list) = check_required_drivers() {
            assert!(!list.is_empty());
        }
    }
}
