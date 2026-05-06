//! HTTP download helpers for root-pipeline asset fetches.
//!
//! Blocking `ureq` wrapper that streams a URL to disk and appends progress
//! lines to a caller-owned log. Pairs with [`crate::github::GitHubClient`]
//! for release-asset URL resolution.

use std::io::{Read, Write};
use std::path::Path;

use crate::error::{LtboxError, Result};

const USER_AGENT: &str = "LTBox-rs/3.0";

/// Shared ureq agent (user-agent + per-stage timeouts) for all outbound HTTP
/// in this crate.
///
/// The previous `timeout_global(120 s)` covered the entire request lifecycle
/// — connect + redirects + headers + the multi-MB body read all shared one
/// budget. GitHub release downloads redirect from `github.com` to
/// `objects.githubusercontent.com` (S3), so a slow link burned most of the
/// budget on connect / TLS / redirect resolution before the body even
/// started, and the body read then tripped `timeout: global` mid-payload.
/// Split into per-stage timeouts so the connect / response phases get a
/// short cap each (fast-fail on a dead link) while the body has 10 minutes
/// to land — enough for the largest root-pipeline payload (Magisk APK,
/// KSU `.ko` + ksuinit, APatch APK→kpimg, GKI AnyKernel3 zips) on slow
/// network connections.
pub(crate) fn build_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_connect(Some(std::time::Duration::from_secs(15)))
        .timeout_recv_response(Some(std::time::Duration::from_secs(30)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(600)))
        .build()
        .new_agent()
}

/// Download `url` to `out_path` in 64 KiB chunks. Progress is throttled to
/// one log line per 10%. Creates missing parent dirs; overwrites existing file.
pub fn download_to_file(url: &str, out_path: &Path, log: &mut Vec<String>) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut resp = build_agent()
        .get(url)
        .call()
        .map_err(|e| LtboxError::Download(format!("GET {url}: {e}")))?;

    let display_name = out_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");
    // `live!` (vs the previous `log.push`) routes through the live
    // sink so the GUI streams every progress tick in real time —
    // otherwise long downloads (LKM nightly payloads, KSU manager
    // APKs) sat invisible until `*ExecDone` flushed the Vec.
    crate::live!(log, "[dl] {display_name} ← {url}");

    // None on chunked responses.
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
    let mut last_emit_at = std::time::Instant::now();
    let started_at = last_emit_at;

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| LtboxError::Download(format!("read: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;

        // Two emit gates so the GUI gets steady progress without log
        // spam:
        //   * Known content-length: every 5% bucket.
        //   * Unknown length (chunked) or just slow links: at least
        //     once every 750 ms with a running KB count + speed.
        let now = std::time::Instant::now();
        let dl_mb = downloaded as f64 / 1_000_000.0;
        let elapsed = now.duration_since(started_at).as_secs_f64().max(0.001);
        let speed_mbps = dl_mb / elapsed;
        if let Some(total) = total
            && total > 0
        {
            let pct = (downloaded * 100 / total) as i32;
            let bucket = pct / 5;
            if bucket > last_pct_bucket {
                last_pct_bucket = bucket;
                last_emit_at = now;
                let total_mb = total as f64 / 1_000_000.0;
                let bar = render_progress_bar(pct as u32, 24);
                crate::live!(
                    log,
                    "[dl] {display_name} {bar} {pct:>3}% ({dl_mb:.1}/{total_mb:.1} MB, {speed_mbps:.1} MB/s)"
                );
            }
        } else if now.duration_since(last_emit_at) >= std::time::Duration::from_millis(750) {
            last_emit_at = now;
            crate::live!(
                log,
                "[dl] {display_name} {dl_mb:.1} MB ({speed_mbps:.1} MB/s)"
            );
        }
    }

    let dl_mb = downloaded as f64 / 1_000_000.0;
    let elapsed = std::time::Instant::now()
        .duration_since(started_at)
        .as_secs_f64()
        .max(0.001);
    let speed_mbps = dl_mb / elapsed;
    crate::live!(
        log,
        "[dl] {display_name} done ({dl_mb:.1} MB in {elapsed:.1}s, avg {speed_mbps:.1} MB/s)"
    );
    Ok(())
}

/// 24-cell ASCII progress bar — `[████████····]`.  Renders nicely in
/// the iced text editor without depending on `indicatif` (which is
/// terminal-aware and would emit ANSI escapes the log panel can't
/// render).
fn render_progress_bar(pct: u32, width: usize) -> String {
    let pct = pct.min(100) as usize;
    let filled = pct * width / 100;
    let mut s = String::with_capacity(width + 2);
    s.push('[');
    for i in 0..width {
        s.push(if i < filled { '█' } else { '·' });
    }
    s.push(']');
    s
}
