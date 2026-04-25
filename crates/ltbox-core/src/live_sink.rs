//! Process-wide live-log sink — independent of the Windows stdout
//! pipe tap.
//!
//! The GUI's `stdout_tap` swaps `STD_OUTPUT_HANDLE` for an
//! `os_pipe`-backed reader thread on Windows. That works for native
//! `println!` from external crates we don't control (`pbr` progress,
//! `qdl` chatter, `magiskboot` …), but it has two failure modes for
//! our own `live!` calls:
//!
//! 1. Rust's stdio init order can capture the *original* (invalid in
//!    GUI subsystem) handle before the swap if anything writes to
//!    stdout during early startup.
//! 2. Heavy bursts on the heavy-thread pool can fill the pipe buffer
//!    faster than the reader drains, blocking the writer for long
//!    stretches and making the live log appear frozen.
//!
//! This sink is the in-process belt to the tap's suspenders: every
//! `live!` call also pushes the formatted line into a shared
//! `Mutex<Vec<String>>`, and the GUI subscription drains it directly
//! every tick. No pipe, no handle dance, no third-party stdio
//! plumbing.

use std::sync::{Mutex, OnceLock};

const MAX_BUFFERED: usize = 4_096;

static SINK: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn buffer() -> &'static Mutex<Vec<String>> {
    SINK.get_or_init(|| Mutex::new(Vec::new()))
}

/// Append one fully-formatted log line. Used by the `live!` macro.
/// Bounded at [`MAX_BUFFERED`] entries — once a long-running flow
/// outpaces the GUI drain (drops/freezes), we discard the oldest
/// lines instead of unbounded growth that would OOM a 24h CI run.
pub fn push(line: String) {
    if let Ok(mut g) = buffer().lock() {
        if g.len() >= MAX_BUFFERED {
            let drop = g.len() - MAX_BUFFERED + 1;
            g.drain(..drop);
        }
        g.push(line);
    }
}

/// Take every queued line since the last drain, returning ownership
/// to the caller (typically the GUI subscription). Empty Vec when
/// nothing is pending; never blocks beyond the mutex acquisition.
pub fn drain() -> Vec<String> {
    if let Ok(mut g) = buffer().lock() {
        return std::mem::take(&mut *g);
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test` runs unit tests in parallel inside the same
    /// process, so the static `SINK` is shared. Serialise via a local
    /// mutex so push/drain assertions don't see another test's lines.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_for_test() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn push_and_drain_roundtrip() {
        let _g = lock_for_test();
        let _ = drain();
        push("alpha".into());
        push("beta".into());
        let pulled = drain();
        assert_eq!(pulled, vec!["alpha".to_string(), "beta".to_string()]);
        assert!(drain().is_empty());
    }

    #[test]
    fn push_above_cap_drops_oldest() {
        let _g = lock_for_test();
        let _ = drain();
        for i in 0..(MAX_BUFFERED + 5) {
            push(format!("line {i}"));
        }
        let pulled = drain();
        assert_eq!(pulled.len(), MAX_BUFFERED);
        // Oldest 5 dropped; first surviving line is `line 5`.
        assert_eq!(pulled[0], "line 5");
        assert_eq!(
            pulled.last().unwrap(),
            &format!("line {}", MAX_BUFFERED + 4)
        );
    }
}
