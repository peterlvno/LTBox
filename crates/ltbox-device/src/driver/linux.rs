//! Linux device-access provisioning: detect and install the LTBox udev rules
//! that grant the desktop user libusb / serial access to the Qualcomm EDL
//! (`05c6:9008`), Lenovo (`17ef`), and Google ADB (`18d1`) USB IDs.
//! See `misc/udev/51-ltbox-qcom.rules`.
//!
//! Userspace mode has no driver *download*: the rules ship embedded in the
//! binary and are written by the privileged `ltbox --install-udev` entry
//! point. Kernel mode uses Qualcomm's `qcom-usb-kernel-drivers` Debian package
//! (`qud`) when `dpkg` is available.
//!
//! Deferred until a Lenovo Qualcomm target is available on Linux: the
//! `/sys/bus/usb/devices` walk for `05c6:9008` + serial-node permission test
//! (a `DevicePresentNoPermission`-style state). Rules presence / staleness is
//! pure filesystem state and is implemented + tested here today.

use std::io::{Read, Write};

use ltbox_core::{live, tr_args};

use super::{
    DriverError, DriverStatus, DriverUpdate, Result, classify_udev_rules, qcom_driver_mode,
};

/// Where `ltbox --install-udev` writes the rules; kept in sync with the GUI's
/// `UDEV_RULES_PATH`.
const UDEV_RULES_PATH: &str = "/etc/udev/rules.d/51-ltbox-qcom.rules";
const KERNEL_RELEASES_API: &str =
    "https://api.github.com/repos/qualcomm/qcom-usb-kernel-drivers/releases?per_page=10";
const LINUX_TAG_NEEDLE: &str = "lnx";
const KERNEL_DEB_PACKAGE: &str = "qud";
const MAX_KERNEL_DRIVER_ZIP_BYTES: u64 = 256 * 1024 * 1024;
const MAX_KERNEL_DEB_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, serde::Deserialize)]
struct GithubRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    published_at: String,
}

#[derive(Debug, serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

pub fn check_required_drivers() -> DriverStatus {
    if qcom_driver_mode().is_kernel() {
        return check_kernel_driver();
    }
    check_udev_rules()
}

fn check_udev_rules() -> DriverStatus {
    match std::fs::read_to_string(UDEV_RULES_PATH) {
        Ok(content) => classify_udev_rules(Some(&content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => classify_udev_rules(None),
        // File exists but is unreadable (permission or otherwise) — surface a
        // repairable state rather than silently claiming the rules are fine.
        Err(_) => DriverStatus::UdevRulesNoPermission,
    }
}

fn check_kernel_driver() -> DriverStatus {
    if which_program("dpkg-query").is_none() {
        return DriverStatus::KernelDriverUnsupported;
    }
    if installed_kernel_driver_version().is_some() {
        DriverStatus::Present
    } else {
        DriverStatus::KernelDriverMissing
    }
}

pub fn check_driver_update() -> Option<DriverUpdate> {
    if !qcom_driver_mode().is_kernel() {
        return None;
    }
    let current = installed_kernel_driver_version()?;
    let (tag, _asset) = fetch_latest_linux_kernel_release().ok()?;
    let latest = version_from_tag(&tag)?;
    if version_lt(&current, &latest) {
        Some(DriverUpdate { current, latest })
    } else {
        None
    }
}

/// Install (or refresh) the udev rules by re-launching this binary through
/// `pkexec` with the fixed `--install-udev` flag. Only the binary's own
/// resolved path is passed — never user input.
pub fn download_and_install(log: &mut Vec<String>) -> Result<()> {
    if qcom_driver_mode().is_kernel() {
        install_kernel_driver(log)
    } else {
        install_udev_rules(log)
    }
}

fn install_udev_rules(log: &mut Vec<String>) -> Result<()> {
    if check_udev_rules() == DriverStatus::Present {
        log.push("[driver] udev rules already up to date".to_string());
        return Ok(());
    }

    let exe = std::env::current_exe().map_err(|e| {
        DriverError::Io(std::io::Error::new(
            e.kind(),
            format!("cannot resolve the LTBox executable path: {e}"),
        ))
    })?;
    if !exe.is_file() {
        return Err(DriverError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("LTBox executable not found at {}", exe.display()),
        )));
    }

    // Require pkexec — never silently fall back to a terminal `sudo` from the
    // GUI, which has no controlling terminal to prompt on.
    let pkexec = which_pkexec().ok_or_else(|| {
        DriverError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "pkexec not found — install polkit, or run `sudo ltbox --install-udev` in a terminal"
                .to_string(),
        ))
    })?;

    log.push(format!("[driver] pkexec {} --install-udev", exe.display()));
    let status = std::process::Command::new(pkexec)
        .arg(&exe)
        .arg("--install-udev")
        .status()
        .map_err(|e| DriverError::Io(std::io::Error::new(e.kind(), format!("pkexec: {e}"))))?;

    match status.code() {
        Some(0) => {}
        // polkit authorization denied / dialog dismissed → pkexec exits 126/127.
        Some(126 | 127) => return Err(DriverError::InstallCancelled),
        Some(code) => return Err(DriverError::InstallerFailed { exit_code: code }),
        None => {
            return Err(DriverError::Io(std::io::Error::other(
                "pkexec terminated by a signal",
            )));
        }
    }

    // Confirm the write actually landed before reporting success.
    if check_required_drivers() != DriverStatus::Present {
        return Err(DriverError::Io(std::io::Error::other(
            "udev rules still not in place after install",
        )));
    }
    log.push("[driver] udev rules installed".to_string());
    Ok(())
}

fn install_kernel_driver(log: &mut Vec<String>) -> Result<()> {
    let pkexec = which_pkexec().ok_or_else(|| {
        DriverError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "pkexec not found — install polkit or install the Qualcomm kernel driver package manually",
        ))
    })?;
    let dpkg = which_program("dpkg").ok_or_else(|| {
        DriverError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "dpkg not found — automatic Linux kernel-driver install is only supported on Debian-style systems",
        ))
    })?;

    live!(
        log,
        "[Driver] {}",
        ltbox_core::i18n::tr("live_driver_fetch_meta")
    );
    let (tag, asset) = fetch_latest_linux_kernel_release()?;
    live!(
        log,
        "[Driver] {}",
        tr_args!("live_driver_asset", name = &asset.name)
    );

    let tmp_dir =
        std::env::temp_dir().join(format!("ltbox_qcom_kernel_drv_{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)?;
    let zip_path = tmp_dir.join(&asset.name);
    let deb_path = tmp_dir.join(format!("{KERNEL_DEB_PACKAGE}_{tag}.deb"));
    let result = (|| {
        if asset.size > MAX_KERNEL_DRIVER_ZIP_BYTES {
            return Err(DriverError::Parse(format!(
                "driver asset too large: {} bytes",
                asset.size
            )));
        }
        download_file(&asset.browser_download_url, &asset.name, &zip_path, log)?;
        extract_first_deb(&zip_path, &deb_path)?;
        live!(
            log,
            "[Driver] {}",
            ltbox_core::i18n::tr("live_driver_running_package_installer")
        );
        let status = std::process::Command::new(pkexec)
            .arg(dpkg)
            .arg("-i")
            .arg(&deb_path)
            .status()
            .map_err(|e| DriverError::Io(std::io::Error::new(e.kind(), format!("pkexec: {e}"))))?;
        match status.code() {
            Some(0) => {}
            Some(126 | 127) => return Err(DriverError::InstallCancelled),
            Some(code) => return Err(DriverError::InstallerFailed { exit_code: code }),
            None => {
                return Err(DriverError::Io(std::io::Error::other(
                    "pkexec terminated by a signal",
                )));
            }
        }
        if check_kernel_driver() != DriverStatus::Present {
            return Err(DriverError::Io(std::io::Error::other(
                "kernel driver package still not installed after installer finished",
            )));
        }
        live!(
            log,
            "[Driver] {}",
            ltbox_core::i18n::tr("live_driver_install_finished")
        );
        Ok(())
    })();
    cleanup(&tmp_dir);
    result
}

fn fetch_latest_linux_kernel_release() -> Result<(String, GithubAsset)> {
    let meta_agent = ltbox_core::downloader::build_agent();
    let releases: Vec<GithubRelease> = meta_agent
        .get(KERNEL_RELEASES_API)
        .call()?
        .body_mut()
        .read_json()
        .map_err(|e| DriverError::Parse(e.to_string()))?;

    let mut matching: Vec<GithubRelease> = releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter(|r| r.tag_name.to_ascii_lowercase().contains(LINUX_TAG_NEEDLE))
        .filter(|r| r.assets.iter().any(|a| linux_kernel_asset_matches(&a.name)))
        .collect();
    matching.sort_unstable_by(|a, b| b.published_at.cmp(&a.published_at));
    let release = matching.into_iter().next().ok_or(DriverError::NoAsset)?;
    let tag = release.tag_name.clone();
    let asset = release
        .assets
        .into_iter()
        .find(|a| linux_kernel_asset_matches(&a.name))
        .ok_or(DriverError::NoAsset)?;
    Ok((tag, asset))
}

fn linux_kernel_asset_matches(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("qud_") && lower.ends_with("_all.zip")
}

fn installed_kernel_driver_version() -> Option<String> {
    let dpkg_query = which_program("dpkg-query")?;
    let out = std::process::Command::new(dpkg_query)
        .arg("-W")
        .arg("-f=${Version}")
        .arg(KERNEL_DEB_PACKAGE)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if parse_version(&version).is_some() {
        Some(version)
    } else {
        None
    }
}

fn version_from_tag(tag: &str) -> Option<String> {
    let seg = tag.rsplit('-').next()?;
    let v = seg.strip_prefix(['v', 'V'])?;
    parse_version(v)?;
    Some(v.to_string())
}

fn parse_version(v: &str) -> Option<Vec<u64>> {
    v.split('.')
        .map(|p| {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                p.parse::<u64>().ok()
            }
        })
        .collect()
}

fn version_lt(a: &str, b: &str) -> bool {
    let (a, b) = (
        parse_version(a).unwrap_or_default(),
        parse_version(b).unwrap_or_default(),
    );
    let n = a.len().max(b.len());
    for i in 0..n {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x < y;
        }
    }
    false
}

fn download_file(
    url: &str,
    name: &str,
    dst: &std::path::Path,
    log: &mut Vec<String>,
) -> Result<()> {
    use ltbox_core::downloader::{DownloadEvent, stream_with_progress};

    let agent = ltbox_core::downloader::build_agent();
    let display_name = name.to_string();
    stream_with_progress(&agent, url, dst, log, move |log, event| match event {
        DownloadEvent::Start => {
            live!(
                log,
                "[Driver] {}",
                tr_args!("live_driver_downloading", name = &display_name)
            );
        }
        DownloadEvent::ProgressPct {
            downloaded_mb,
            total_mb,
            pct,
            speed_mbps,
        } => {
            live!(
                log,
                "[Driver] {}",
                tr_args!(
                    "live_driver_progress_pct",
                    name = &display_name,
                    pct = format!("{pct:>3}"),
                    downloaded = format!("{downloaded_mb:.1}"),
                    total = format!("{total_mb:.1}"),
                    speed = format!("{speed_mbps:.1}"),
                )
            );
        }
        DownloadEvent::ProgressChunked {
            downloaded_mb,
            speed_mbps,
        } => {
            live!(
                log,
                "[Driver] {}",
                tr_args!(
                    "live_driver_progress_chunked",
                    name = &display_name,
                    downloaded = format!("{downloaded_mb:.1}"),
                    speed = format!("{speed_mbps:.1}"),
                )
            );
        }
        DownloadEvent::Done {
            downloaded_mb,
            elapsed_s,
        } => {
            live!(
                log,
                "[Driver] {}",
                tr_args!(
                    "live_driver_dl_done",
                    name = &display_name,
                    size = format!("{downloaded_mb:.1}"),
                    elapsed = format!("{elapsed_s:.1}"),
                )
            );
        }
    })
    .map_err(|e| DriverError::Http(format!("download: {e}")))
}

fn extract_first_deb(zip_path: &std::path::Path, out_path: &std::path::Path) -> Result<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(DriverError::Zip)?;
    for i in 0..archive.len() {
        let mut member = archive.by_index(i).map_err(DriverError::Zip)?;
        if !member.is_file() {
            continue;
        }
        let Some(path) = member.enclosed_name() else {
            continue;
        };
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("deb"))
        {
            if member.size() > MAX_KERNEL_DEB_BYTES {
                return Err(DriverError::Parse(format!(
                    "driver package too large: {} bytes",
                    member.size()
                )));
            }
            let mut out = std::fs::File::create(out_path)?;
            let copied = std::io::copy(
                &mut std::io::Read::by_ref(&mut member).take(MAX_KERNEL_DEB_BYTES + 1),
                &mut out,
            )
            .map_err(|e| DriverError::Http(format!("extract: {e}")))?;
            if copied > MAX_KERNEL_DEB_BYTES {
                return Err(DriverError::Parse(format!(
                    "driver package too large: {copied} bytes"
                )));
            }
            out.flush()?;
            return Ok(());
        }
    }
    Err(DriverError::NoAsset)
}

fn cleanup(path: &std::path::Path) {
    if let Err(e) = std::fs::remove_dir_all(path) {
        tracing::debug!("failed to clean driver temp dir {}: {e}", path.display());
    }
}

/// Locate `pkexec` on `PATH` without pulling in a `which` dependency.
fn which_pkexec() -> Option<std::path::PathBuf> {
    which_program("pkexec")
}

/// Whether `dpkg-query` is on `PATH` — the signal that this Linux host is
/// Debian-style and can use the Qualcomm kernel driver. Mirrors the gate in
/// [`check_kernel_driver`] and backs [`super::kernel_default_supported`].
pub(super) fn dpkg_available() -> bool {
    which_program("dpkg-query").is_some()
}

fn which_program(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_asset_matcher_accepts_qud_all_zip_only() {
        assert!(linux_kernel_asset_matches("qud_1.0.6.4_all.zip"));
        assert!(linux_kernel_asset_matches("QUD_1.0.6.4_ALL.ZIP"));
        assert!(!linux_kernel_asset_matches(
            "qud-win-v1.00.94.6_x86_64_arm64_signed.zip"
        ));
        assert!(!linux_kernel_asset_matches(
            "qcom_usb_kernel_drivers_x64.exe"
        ));
        assert!(!linux_kernel_asset_matches("qud_1.0.6.4_amd64.deb"));
    }

    #[test]
    fn version_from_linux_tag_extracts_dotted() {
        assert_eq!(
            version_from_tag("release-lnx-v1.0.6.4").as_deref(),
            Some("1.0.6.4")
        );
        assert_eq!(version_from_tag("release-lnx-v1..6").as_deref(), None);
        assert_eq!(version_from_tag("release-lnx-1.0.6.4").as_deref(), None);
    }
}
