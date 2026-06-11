//! Qualcomm 9008 EDL WinUSB driver detection + auto-install on Windows.
//!
//! LTBox requires `qcserlib.inf` from Qualcomm's userspace driver bundle,
//! then runs the signed per-arch installer through Windows UAC.

use std::path::Path;
use std::process::Command;

use ltbox_core::i18n::tr;
use ltbox_core::{live, tr_args};

use super::{DriverError, DriverStatus, DriverUpdate, Result};

/// `Command::new` with no console window.
fn silent_command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// INFs whose absence triggers a missing-driver banner.
const REQUIRED_INFS: &[&str] = &["qcserlib.inf"];

#[derive(Debug, serde::Deserialize)]
struct GithubRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
    #[serde(default)]
    draft: bool,
}

#[derive(Debug, serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

const RELEASES_API: &str =
    "https://api.github.com/repos/qualcomm/qcom-usb-userspace-drivers/releases?per_page=10";
/// Windows release tags carry a `win` token (`release-win-v1.0.2.0`); the
/// repo also publishes Linux-only tags that ship no `.exe` installer.
const WIN_TAG_NEEDLE: &str = "win";

/// Signed installer asset name for the host architecture. The release
/// ships one self-extracting `.exe` per arch.
fn arch_installer_asset() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "qcom_usb_userspace_drivers_arm64.exe"
    } else if cfg!(target_arch = "x86") {
        "qcom_usb_userspace_drivers_x86.exe"
    } else {
        "qcom_usb_userspace_drivers_x64.exe"
    }
}

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

/// Fetch the latest non-draft Windows release that ships the host-arch
/// installer, returning `(tag_name, asset_download_url)`. Shared by the
/// installer and the update check so both resolve to the same release.
fn fetch_latest_win_release() -> Result<(String, String)> {
    let meta_agent = ltbox_core::downloader::build_agent();

    let releases: Vec<GithubRelease> = meta_agent
        .get(RELEASES_API)
        .call()?
        .body_mut()
        .read_json()
        .map_err(|e| DriverError::Parse(e.to_string()))?;

    let asset_name = arch_installer_asset();
    let release = releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter(|r| r.tag_name.to_ascii_lowercase().contains(WIN_TAG_NEEDLE))
        .find(|r| {
            r.assets
                .iter()
                .any(|a| a.name.eq_ignore_ascii_case(asset_name))
        })
        .ok_or(DriverError::NoAsset)?;

    let tag = release.tag_name.clone();
    let asset_url = release
        .assets
        .into_iter()
        .find(|a| a.name.eq_ignore_ascii_case(asset_name))
        .map(|a| a.browser_download_url)
        .ok_or(DriverError::NoAsset)?;
    Ok((tag, asset_url))
}

/// Check whether a newer signed driver release exists than the one
/// installed. Returns `Some` only when a driver is present locally AND the
/// latest Windows release is strictly newer. Any failure — no driver
/// installed, version unparseable, offline, GitHub unreachable — collapses
/// to `None` so the caller can fail silently (no banner).
pub fn check_driver_update() -> Option<DriverUpdate> {
    let current = installed_driver_version()?;
    let (tag, _url) = fetch_latest_win_release().ok()?;
    let latest = version_from_tag(&tag)?;
    if version_lt(&current, &latest) {
        Some(DriverUpdate { current, latest })
    } else {
        None
    }
}

/// Parse the dotted version from a Windows release tag such as
/// `release-win-v1.0.2.0` → `1.0.2.0`. Repo tags always carry a `v`-prefixed
/// version as the final `-`-delimited segment, so require exactly that: the
/// last segment must start with `v`/`V`, and the remainder must be a strict
/// dotted version. This rejects sign-prefixed / non-`v` garbage
/// (`release-win-+1.2`, `…-v+1.2`, `…-v-1.2`) as `None` rather than
/// extracting a truncated value.
fn version_from_tag(tag: &str) -> Option<String> {
    let seg = tag.rsplit('-').next()?;
    let v = seg.strip_prefix(['v', 'V'])?;
    parse_version(v)?;
    Some(v.to_string())
}

/// Parse a strict dotted version into numeric components, or `None` when
/// malformed — every `.`-separated component must be non-empty and ASCII
/// digits only. `"1.0.2.0"` → `[1, 0, 2, 0]`; `"1..2"`, `"1."`, `".2"`,
/// `"."`, `""`, `"1.x"`, and sign-prefixed `"+1"` / `"1.+2"` → `None`.
/// The explicit digit gate is required because `u64::from_str` otherwise
/// accepts a leading `+`.
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

/// `true` when `a` is strictly older than `b`, comparing component-wise
/// with missing trailing components treated as 0 (so `1.0` == `1.0.0`).
/// Inputs are pre-validated by the parsers above; a malformed string here
/// degrades to an empty (all-zero) version rather than panicking.
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

/// Read an `.inf` as text, honouring a UTF-16LE BOM (some signed INFs ship
/// UTF-16) and falling back to lossy UTF-8/ANSI otherwise.
fn read_inf_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let u16s: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Some(String::from_utf16_lossy(&u16s))
    } else {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Strip exactly one balanced surrounding `"` pair, leaving an unbalanced
/// quote untouched so it fails downstream validation.
fn strip_balanced_quotes(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Parse the version half of an INF `DriverVer = MM/DD/YYYY,V.V.V.V` line
/// (case-insensitive key, optional spaces, comma-separated date+version).
fn parse_driver_ver(inf_text: &str) -> Option<String> {
    for line in inf_text.lines() {
        let line = line.trim();
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("DriverVer") {
            continue;
        }
        // INF allows the whole value to be wrapped in one quote pair
        // (`DriverVer = "date,version"`). Strip a *balanced* pair only — an
        // unbalanced quote is left in place so it fails the strict parse.
        let val = strip_balanced_quotes(val.trim());
        // `DriverVer` is `date,version` (or just `version`). Take everything
        // after the FIRST comma — a stray extra comma then stays inside a
        // component (`1.+2,1.2` → component `+2,1`) and fails the parse,
        // rather than `rsplit` silently grabbing a clean trailing fragment.
        let ver = val.split_once(',').map(|(_, v)| v).unwrap_or(val).trim();
        // Strict parse: a malformed `DriverVer` version yields `None`
        // rather than a truncated comparison value.
        if parse_version(ver).is_some() {
            return Some(ver.to_string());
        }
    }
    None
}

/// Highest `DriverVer` among installed `qcserlib.inf` DriverStore copies,
/// or `None` when the driver is not installed. Windows may stage several
/// `qcserlib.inf_*` folders (multiple versions); the max is the effective
/// one for an update comparison.
pub fn installed_driver_version() -> Option<String> {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let repo = Path::new(&system_root)
        .join("System32")
        .join("DriverStore")
        .join("FileRepository");
    let entries = std::fs::read_dir(&repo).ok()?;
    let mut best: Option<String> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if !name.starts_with("qcserlib.inf") {
            continue;
        }
        let inf = entry.path().join("qcserlib.inf");
        let Some(text) = read_inf_text(&inf) else {
            continue;
        };
        if let Some(v) = parse_driver_ver(&text) {
            best = match best {
                Some(b) if !version_lt(&b, &v) => Some(b),
                _ => Some(v),
            };
        }
    }
    best
}

/// Download the host-arch userspace-driver installer and run it elevated.
pub fn download_and_install(log: &mut Vec<String>) -> Result<()> {
    live!(log, "[Driver] {}", tr("live_driver_fetch_meta"));
    let asset_name = arch_installer_asset();
    let (_tag, asset_url) = fetch_latest_win_release()?;

    live!(
        log,
        "[Driver] {}",
        tr_args!("live_driver_asset", name = asset_name)
    );

    let tmp_dir = std::env::temp_dir().join(format!("ltbox_qcom_drv_{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)?;
    let exe_path = tmp_dir.join(asset_name);

    let dl_agent = ltbox_core::downloader::build_agent();

    download_with_progress(&dl_agent, &asset_url, asset_name, &exe_path, log)?;

    live!(log, "[Driver] {}", tr("live_driver_running_installer"));
    let result = run_installer_elevated(&exe_path, log);
    cleanup(&tmp_dir);
    result?;

    live!(log, "[Driver] {}", tr("live_driver_install_finished"));
    Ok(())
}

/// Run the signed installer through UAC and map cancel vs failure.
fn run_installer_elevated(exe: &Path, log: &mut Vec<String>) -> Result<()> {
    // Escape for a PowerShell single-quoted string literal (`'` → `''`).
    // The temp path is process-id-derived so quotes are not expected, but
    // escape defensively rather than trust the environment.
    let exe_str = exe.to_string_lossy().replace('\'', "''");
    // `$p.ExitCode` can be `$null` for some self-extracting installers that
    // hand off to a detached child; `exit $null` would silently become exit
    // 0 and report a false success. Treat a null exit code as a failure
    // (exit 1) so the caller surfaces `InstallerFailed` instead of a green
    // toast over a driver that never actually installed.
    let script = format!(
        "try {{ $p = Start-Process -FilePath '{exe_str}' -Verb RunAs -Wait -PassThru \
         -ErrorAction Stop; if ($null -eq $p.ExitCode) {{ exit 1 }} else {{ exit $p.ExitCode }} }} \
         catch {{ exit 1223 }}"
    );

    let out = silent_command("powershell")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(&script)
        .output()
        .map_err(DriverError::Io)?;

    let code = out.status.code().unwrap_or(-1);
    match code {
        0 => Ok(()),
        // ERROR_CANCELLED — the user dismissed the UAC elevation prompt.
        1223 => {
            live!(log, "[Driver] {}", tr("live_driver_install_cancelled"));
            Err(DriverError::InstallCancelled)
        }
        other => {
            live!(
                log,
                "[Driver] {}",
                tr_args!("live_driver_installer_failed", exit = other)
            );
            Err(DriverError::InstallerFailed { exit_code: other })
        }
    }
}

/// Stream the installer download with driver-flow log formatting.
fn download_with_progress(
    agent: &ureq::Agent,
    url: &str,
    display_name: &str,
    out_path: &Path,
    log: &mut Vec<String>,
) -> Result<()> {
    use ltbox_core::downloader::{DownloadEvent, stream_with_progress};
    let display_name = display_name.to_string();
    stream_with_progress(agent, url, out_path, log, move |log, event| match event {
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

    /// Host-arch installer asset name resolves to one of the three
    /// shipped variants.
    #[test]
    fn arch_asset_is_known() {
        let name = arch_installer_asset();
        assert!(
            matches!(
                name,
                "qcom_usb_userspace_drivers_x64.exe"
                    | "qcom_usb_userspace_drivers_arm64.exe"
                    | "qcom_usb_userspace_drivers_x86.exe"
            ),
            "unexpected asset name: {name}"
        );
    }

    #[test]
    fn version_from_tag_extracts_dotted() {
        assert_eq!(
            version_from_tag("release-win-v1.0.2.0").as_deref(),
            Some("1.0.2.0")
        );
        assert_eq!(version_from_tag("v2.3.4").as_deref(), Some("2.3.4"));
        assert_eq!(version_from_tag("release-linux").as_deref(), None);
        assert_eq!(version_from_tag("").as_deref(), None);
        // Malformed dotted versions are rejected, not silently truncated.
        assert_eq!(version_from_tag("release-win-v1..2").as_deref(), None);
        assert_eq!(version_from_tag("release-win-v1.").as_deref(), None);
        assert_eq!(version_from_tag("release-win-v1.x").as_deref(), None);
        // `u64::from_str` accepts a leading `+`; the digit gate rejects it.
        assert_eq!(version_from_tag("release-win-v1.+2").as_deref(), None);
        // Final segment must be `v`-prefixed; sign-prefixed, `v`-less, and
        // `rsplit`-cleaned signed tags all reject.
        assert_eq!(version_from_tag("release-win-+1.2").as_deref(), None);
        assert_eq!(version_from_tag("release-win-v+1.2").as_deref(), None);
        assert_eq!(version_from_tag("release-win-v-1.2").as_deref(), None);
        assert_eq!(version_from_tag("release-win-1.0.2.0").as_deref(), None);
    }

    #[test]
    fn parse_driver_ver_handles_spacing_and_date() {
        assert_eq!(
            parse_driver_ver("[Version]\nDriverVer = 09/27/2023,1.0.2.0\n").as_deref(),
            Some("1.0.2.0")
        );
        assert_eq!(
            parse_driver_ver("driverver=01/01/2020, 2.0.0.1").as_deref(),
            Some("2.0.0.1")
        );
        // No DriverVer line → None.
        assert_eq!(parse_driver_ver("[Version]\nClass=USB\n"), None);
        // Malformed version components → None (not a truncated value).
        assert_eq!(parse_driver_ver("DriverVer=09/27/2023,1..2"), None);
        assert_eq!(parse_driver_ver("DriverVer=09/27/2023,1."), None);
        assert_eq!(parse_driver_ver("DriverVer=09/27/2023,."), None);
        // Sign-prefixed components are rejected by the digit gate.
        assert_eq!(parse_driver_ver("DriverVer=09/27/2023,+1"), None);
        assert_eq!(parse_driver_ver("DriverVer=09/27/2023,1.+2"), None);
        // Take the version after the FIRST comma — a stray extra comma keeps
        // the bad fragment in a component instead of grabbing a clean tail.
        assert_eq!(parse_driver_ver("DriverVer=09/27/2023,1.+2,1.2"), None);
        // Unbalanced quotes are not trimmed → fail the strict parse.
        assert_eq!(parse_driver_ver("DriverVer=09/27/2023,\"1.2"), None);
        assert_eq!(parse_driver_ver("DriverVer=09/27/2023,1.2\""), None);
        // A balanced quote pair wrapping the whole value is stripped.
        assert_eq!(
            parse_driver_ver("DriverVer=\"09/27/2023,1.0.2.0\"").as_deref(),
            Some("1.0.2.0")
        );
    }

    #[test]
    fn version_lt_compares_componentwise() {
        assert!(version_lt("1.0.1.0", "1.0.2.0"));
        assert!(version_lt("1.0", "1.0.0.1"));
        assert!(!version_lt("1.0.2.0", "1.0.2.0"));
        assert!(!version_lt("1.0.0", "1.0"));
        assert!(!version_lt("2.0.0.0", "1.9.9.9"));
    }
}
