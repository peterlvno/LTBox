//! HTTP download helpers for root-pipeline asset fetches.
//!
//! Blocking `ureq` wrapper that streams a URL to disk and appends progress
//! lines to a caller-owned log. Pairs with [`crate::github::GitHubClient`]
//! for release-asset URL resolution.

use std::path::Path;

use crate::error::{LtboxError, Result};

/// Shared `ltbox/<version>` user agent for every outbound request. The
/// `probe_connectivity` startup check builds its own short-timeout agent but
/// reuses this string, so the user agent has a single definition.
pub const USER_AGENT: &str = concat!("ltbox/", env!("CARGO_PKG_VERSION"));

/// Process-wide shared `ureq::Agent`. Reuses TLS roots + the connection
/// pool across every outbound HTTP request in the workspace (downloader,
/// github / nightly.link clients, lenovo PTSTPD, lenovo OTA). Building a
/// fresh agent per call rebuilt the rustls config + spun up a new pool
/// each time, which on a Magisk-update flow alone meant 5+ redundant
/// TLS-config setups in seconds.
///
/// Per-stage timeouts (15 s connect, 30 s recv-response, 600 s recv-body)
/// replace the prior `timeout_global(120 s)` that guillotined slow-link
/// downloads mid-body — see commit history for the upstream bug
/// (`timeout: global` mid-payload on Lenovo / GitHub-release pulls).
fn shared_agent() -> &'static ureq::Agent {
    use std::sync::OnceLock;
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .timeout_connect(Some(std::time::Duration::from_secs(15)))
            .timeout_recv_response(Some(std::time::Duration::from_secs(30)))
            .timeout_recv_body(Some(std::time::Duration::from_secs(600)))
            .build()
            .new_agent()
    })
}

/// Clone the process-wide shared `ureq::Agent` handle (cheap, `Arc`-backed).
/// Reuse this for every outbound HTTP request in the workspace — including
/// other crates — so they share TLS roots, the connection pool, and a single
/// `ltbox/<version>` user agent.
pub fn build_agent() -> ureq::Agent {
    shared_agent().clone()
}

/// Event emitted by [`stream_with_progress`] at each progress
/// throttle gate. Callers map these into log lines (and / or telemetry
/// counters) — the streamer keeps no opinions about formatting or
/// i18n.
pub enum DownloadEvent {
    /// Stream opened, before any bytes have been read.
    Start,
    /// Known `Content-Length`: a new 5 % bucket boundary fired.
    ProgressPct {
        downloaded_mb: f64,
        total_mb: f64,
        pct: i32,
        speed_mbps: f64,
    },
    /// Unknown length (chunked or no header): 750 ms tick fired.
    ProgressChunked { downloaded_mb: f64, speed_mbps: f64 },
    /// Body fully read + flushed to disk.
    Done { downloaded_mb: f64, elapsed_s: f64 },
}

/// Stream `url` to `out_path` in 64 KiB chunks; the caller's
/// `on_event` closure handles all progress logging / formatting.
/// Centralises the byte loop + 5 %-bucket + 750 ms-tick throttle so
/// secondary consumers (e.g. the Windows driver installer) don't
/// re-implement the streaming logic just to swap the log prefix and
/// i18n keys.
///
/// Creates missing parent dirs; overwrites existing file.
pub fn stream_with_progress<F>(
    agent: &ureq::Agent,
    url: &str,
    out_path: &Path,
    log: &mut Vec<String>,
    mut on_event: F,
) -> Result<()>
where
    F: FnMut(&mut Vec<String>, DownloadEvent),
{
    use std::io::{Read, Write};

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| LtboxError::Download(format!("GET {url}: {e}")))?;
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

    on_event(log, DownloadEvent::Start);

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| LtboxError::Download(format!("read: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;

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
                on_event(
                    log,
                    DownloadEvent::ProgressPct {
                        downloaded_mb: dl_mb,
                        total_mb,
                        pct,
                        speed_mbps,
                    },
                );
            }
        } else if now.duration_since(last_emit_at) >= std::time::Duration::from_millis(750) {
            last_emit_at = now;
            on_event(
                log,
                DownloadEvent::ProgressChunked {
                    downloaded_mb: dl_mb,
                    speed_mbps,
                },
            );
        }
    }

    let elapsed_s = started_at.elapsed().as_secs_f64().max(0.001);
    let dl_mb = downloaded as f64 / 1_000_000.0;
    on_event(
        log,
        DownloadEvent::Done {
            downloaded_mb: dl_mb,
            elapsed_s,
        },
    );
    Ok(())
}

/// Download `url` to `out_path` in 64 KiB chunks. Progress is throttled to
/// one log line per 5 %. Creates missing parent dirs; overwrites existing file.
pub fn download_to_file(url: &str, out_path: &Path, log: &mut Vec<String>) -> Result<()> {
    let display_name = out_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download")
        .to_string();
    let url_for_start = url.to_string();
    let agent = build_agent();
    stream_with_progress(&agent, url, out_path, log, move |log, event| {
        // `live!` (vs the previous `log.push`) routes through the live
        // sink so the GUI streams every progress tick in real time —
        // otherwise long downloads (LKM nightly payloads, KSU manager
        // APKs) sat invisible until `*ExecDone` flushed the Vec.
        match event {
            DownloadEvent::Start => {
                crate::live!(
                    log,
                    "[dl] {}",
                    crate::tr_args!(
                        "live_download_start",
                        name = &display_name,
                        url = &url_for_start
                    )
                );
            }
            DownloadEvent::ProgressPct {
                downloaded_mb,
                total_mb,
                pct,
                speed_mbps,
            } => {
                let bar = render_progress_bar(pct as u32, 24);
                crate::live!(
                    log,
                    "[dl] {}",
                    crate::tr_args!(
                        "live_download_progress_pct",
                        name = &display_name,
                        bar = &bar,
                        pct = format!("{pct:>3}"),
                        downloaded = format!("{downloaded_mb:.1}"),
                        total = format!("{total_mb:.1}"),
                        speed = format!("{speed_mbps:.1}")
                    )
                );
            }
            DownloadEvent::ProgressChunked {
                downloaded_mb,
                speed_mbps,
            } => {
                crate::live!(
                    log,
                    "[dl] {}",
                    crate::tr_args!(
                        "live_download_progress_chunked",
                        name = &display_name,
                        downloaded = format!("{downloaded_mb:.1}"),
                        speed = format!("{speed_mbps:.1}")
                    )
                );
            }
            DownloadEvent::Done {
                downloaded_mb,
                elapsed_s,
            } => {
                let avg = downloaded_mb / elapsed_s.max(0.001);
                crate::live!(
                    log,
                    "[dl] {}",
                    crate::tr_args!(
                        "live_download_done",
                        name = &display_name,
                        size = format!("{downloaded_mb:.1}"),
                        elapsed = format!("{elapsed_s:.1}"),
                        avg = format!("{avg:.1}")
                    )
                );
            }
        }
    })
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
