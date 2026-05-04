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

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use ltbox_core::i18n::tr;
use ltbox_core::live;

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
/// each `.inf` under a `Windows10/` folder.
///
/// Every milestone routes through `live!` so the GUI streams progress in
/// real time — the previous `log.push` only surfaced after the whole task
/// returned, so a stalled download looked indistinguishable from a fast
/// success until the final timeout error fired.
///
/// Two ureq agents:
///   * `meta_agent` — 30 s global, used for the small JSON release listing.
///   * `dl_agent` — no global cap; per-stage `connect` / `recv-response` /
///     `recv-body` timeouts so a slow link can finish a multi-MB ZIP
///     without the previous 30-s "timeout: global" guillotine cutting the
///     body read partway through.
pub fn download_and_install(log: &mut Vec<String>) -> Result<()> {
    live!(log, "[Driver] {}", tr("live_driver_fetch_meta"));
    let meta_agent = ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .new_agent();

    let release: GithubRelease = meta_agent
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

    live!(
        log,
        "[Driver] {}",
        tr("live_driver_asset").replace("{name}", &asset_name)
    );

    let tmp_dir = std::env::temp_dir().join(format!("ltbox_qcom_drv_{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir)?;
    let zip_path = tmp_dir.join(&asset_name);

    // No global cap — stalls/slow links should still finish a 10–20 MB
    // ZIP without the previous "timeout: global" guillotine.
    let dl_agent = ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_connect(Some(std::time::Duration::from_secs(15)))
        .timeout_recv_response(Some(std::time::Duration::from_secs(30)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(300)))
        .build()
        .new_agent();

    download_with_progress(&dl_agent, &asset_url, &asset_name, &zip_path, log)?;

    live!(log, "[Driver] {}", tr("live_driver_extracting"));
    let extract_dir = tmp_dir.join("extracted");
    std::fs::create_dir_all(&extract_dir)?;
    extract_zip(&zip_path, &extract_dir)?;

    let mut inf_files: Vec<PathBuf> = Vec::new();
    walk_collect_infs(&extract_dir, &mut inf_files);
    if inf_files.is_empty() {
        cleanup(&tmp_dir);
        return Err(DriverError::NoInf);
    }

    let mut succeeded = 0usize;
    let mut failed = 0usize;
    for inf in &inf_files {
        let name = inf
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        live!(
            log,
            "[Driver] {}",
            tr("live_driver_installing_inf").replace("{name}", &name)
        );
        let out = silent_command("pnputil")
            .arg("/add-driver")
            .arg(inf)
            .arg("/install")
            .output();
        match out {
            Ok(o) if o.status.success() => {
                succeeded += 1;
            }
            Ok(o) => {
                failed += 1;
                // pnputil writes its diagnostics to stdout, not stderr,
                // so logging only stderr left every failure as a blank
                // "failed: " line. Decode both, prefer stdout when
                // populated, fall back to a friendly hint when both are
                // empty (typical when pnputil bails on UAC before
                // emitting any text).
                let exit = o.status.code().unwrap_or(-1);
                let stdout = decode_console(&o.stdout);
                let stderr = decode_console(&o.stderr);
                let detail = if !stdout.trim().is_empty() {
                    stdout.trim().to_string()
                } else if !stderr.trim().is_empty() {
                    stderr.trim().to_string()
                } else {
                    tr("live_driver_pnputil_no_diag")
                };
                live!(
                    log,
                    "[Driver] {}",
                    tr("live_driver_pnputil_failed")
                        .replace("{name}", &name)
                        .replace("{exit}", &exit.to_string())
                        .replace("{detail}", &detail)
                );
            }
            Err(e) => {
                failed += 1;
                live!(
                    log,
                    "[Driver] {}",
                    tr("live_driver_pnputil_spawn_failed")
                        .replace("{name}", &name)
                        .replace("{error}", &e.to_string())
                );
            }
        }
    }

    cleanup(&tmp_dir);

    // All installs flopped → surface as hard failure so the GUI shows
    // the red banner instead of the green "install complete" toast.
    if succeeded == 0 && failed > 0 {
        live!(
            log,
            "[Driver] {}",
            tr("live_driver_all_failed").replace("{count}", &failed.to_string())
        );
        return Err(DriverError::PnputilAllFailed { count: failed });
    }

    let total = succeeded + failed;
    live!(
        log,
        "[Driver] {}",
        tr("live_driver_install_finished")
            .replace("{succeeded}", &succeeded.to_string())
            .replace("{total}", &total.to_string())
    );
    Ok(())
}

/// Decode bytes captured from a Windows console subprocess. Tries UTF-8
/// first, then falls back to lossy UTF-8 (which keeps ASCII intact and
/// only mangles the high-byte ranges) so localized pnputil output at
/// least surfaces something instead of a blank "failed: " tail.
fn decode_console(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        s.to_string()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Stream `url` to `out_path` in 64 KiB chunks, emitting a progress line
/// every 5 % bucket (or every 750 ms for chunked / unknown-length
/// responses). Mirrors `ltbox_core::downloader::download_to_file` but
/// kept local to the driver crate so this stays self-contained for the
/// Windows-only `pnputil` install path.
fn download_with_progress(
    agent: &ureq::Agent,
    url: &str,
    display_name: &str,
    out_path: &Path,
    log: &mut Vec<String>,
) -> Result<()> {
    live!(
        log,
        "[Driver] {}",
        tr("live_driver_downloading").replace("{name}", display_name)
    );
    let mut resp = agent.get(url).call()?;
    let total: Option<u64> = resp
        .headers()
        .get(ureq::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(out_path)?;
    let mut buf = [0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    let mut last_pct_bucket: i32 = -1;
    let started_at = std::time::Instant::now();
    let mut last_emit_at = started_at;

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| DriverError::Http(format!("download read: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;

        let now = std::time::Instant::now();
        let dl_mb = downloaded as f64 / 1_000_000.0;
        let elapsed = now.duration_since(started_at).as_secs_f64().max(0.001);
        let speed = dl_mb / elapsed;
        if let Some(total) = total
            && total > 0
        {
            let pct = (downloaded * 100 / total) as i32;
            let bucket = pct / 5;
            if bucket > last_pct_bucket {
                last_pct_bucket = bucket;
                last_emit_at = now;
                let total_mb = total as f64 / 1_000_000.0;
                live!(
                    log,
                    "[Driver] {}",
                    tr("live_driver_progress_pct")
                        .replace("{name}", display_name)
                        .replace("{pct}", &format!("{pct:>3}"))
                        .replace("{downloaded}", &format!("{dl_mb:.1}"))
                        .replace("{total}", &format!("{total_mb:.1}"))
                        .replace("{speed}", &format!("{speed:.1}"))
                );
            }
        } else if now.duration_since(last_emit_at) >= std::time::Duration::from_millis(750) {
            last_emit_at = now;
            live!(
                log,
                "[Driver] {}",
                tr("live_driver_progress_chunked")
                    .replace("{name}", display_name)
                    .replace("{downloaded}", &format!("{dl_mb:.1}"))
                    .replace("{speed}", &format!("{speed:.1}"))
            );
        }
    }

    let elapsed = started_at.elapsed().as_secs_f64().max(0.001);
    let dl_mb = downloaded as f64 / 1_000_000.0;
    live!(
        log,
        "[Driver] {}",
        tr("live_driver_dl_done")
            .replace("{name}", display_name)
            .replace("{size}", &format!("{dl_mb:.1}"))
            .replace("{elapsed}", &format!("{elapsed:.1}"))
    );
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
