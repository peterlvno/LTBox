//! `ltbox-core` — domain layer shared across LTBox crates.
//!
//! Config loader, AES-CBC `.x` decryption, GitHub client, i18n, and
//! rawprogram XML parser. Every fallible API returns [`Result<T>`] /
//! [`LtboxError`]. Port of the non-UI parts of Python LTBox v2.x.

pub mod config;
pub mod crypto;
pub mod downloader;
pub mod error;
pub mod github;
pub mod i18n;
pub mod runtime;
pub mod xml_catalog;

pub use error::{LtboxError, Result};

/// Echo a line to BOTH `println!` (so the GUI's stdout tap can stream
/// it live) AND the caller's `&mut Vec<String>` log (so the
/// `*ExecDone` flush still has the full audit trail even when the tap
/// drops lines or the OS rate-limits the pipe).
///
/// Earlier revisions only printed and intentionally avoided the push
/// to dodge "tap-already-rendered" duplicates in the live panel. That
/// turned out to mask real outages — on Windows GUI subsystem the tap
/// occasionally lost long stretches of output for unroot / LKM root
/// flows and the live log went silent end-to-end, leaving the user
/// staring at a frozen wizard. The dup risk is handled by
/// `App::log_extend` adjacent-dedup; the resilience win is worth the
/// trivial double-bookkeeping.
///
/// Lives in `ltbox-core` so every downstream crate (`ltbox-device`,
/// `ltbox-patch`, `ltbox-gui`) can emit through the same path without
/// redefining the macro or taking a circular dep. `#[macro_export]`
/// puts it at the crate root, reachable as `ltbox_core::live!(…)`.
#[macro_export]
macro_rules! live {
    ($log:expr, $($arg:tt)*) => {{
        let _line = format!($($arg)*);
        println!("{}", _line);
        $log.push(_line);
    }};
}
