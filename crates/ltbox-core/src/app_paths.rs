//! Cross-platform helper for LTBox-owned writable directories.
//!
//! Centralises every "where do generated outputs / dumps / backups
//! land?" question so call sites stop reaching for `current_exe()`.
//! Necessary because:
//!
//! * AppImage mounts the bundle read-only — writes next to the
//!   executable would either fail or land on the squashfs-backed
//!   FUSE mount, depending on kernel.
//! * Distro-installed binaries live under `/usr/bin`, owned by root,
//!   non-writable for normal users.
//! * Even on Windows, `Program Files`-installed copies hit UAC the
//!   moment something tries to write next to `ltbox.exe`.
//!
//! ## Per-OS layout
//!
//! Per `PLAN_Linux_Support.md` D6, Windows MAY keep the existing
//! exe-adjacent layout for continuity with current testers; Linux
//! and other unixes always go through XDG-style data dirs.
//!
//! | OS      | Auto-output / backup root             |
//! |---------|---------------------------------------|
//! | Windows | `<exe-dir>` (existing v3 behaviour)   |
//! | Linux   | `$XDG_DATA_HOME/ltbox` (≈ `~/.local/share/ltbox`) |
//! | macOS   | `~/Library/Application Support/ltbox` |
//!
//! User-selected output folders (Partition Dump destination, Physical
//! Storage dump path, etc.) are NOT routed through here — those are
//! explicit picks and stay where the user chose.

use std::path::PathBuf;

/// Directory where auto-generated dumps / backups / per-action output
/// roots live. Caller `create_dir_all`s before writing.
///
/// Platform mapping documented at the module level.
pub fn auto_output_root() -> PathBuf {
    if cfg!(windows) {
        // v3 Windows behaviour: outputs sit next to ltbox.exe so a
        // single zip extracted into Downloads stays self-contained.
        // Falls back to "." only if `current_exe()` fails (which on
        // Windows would mean something is severely wrong anyway).
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        // dirs::data_dir() returns `$XDG_DATA_HOME` on Linux (default
        // `~/.local/share`) and `~/Library/Application Support` on
        // macOS. Matches what `settings_store` + the root pipeline's
        // `work_dir` already use elsewhere in the workspace.
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ltbox")
    }
}

/// Per-action output sub-directory under [`auto_output_root`].
///
/// `slug` is the action identifier (e.g. `"patch_arb"`,
/// `"region_convert"`). Final layout on Windows is
/// `<exe-dir>/output_<slug>` to match the legacy v3 path; on every
/// other OS it is `<XDG data>/ltbox/outputs/<slug>` to keep the
/// auto-output pile inside one tree.
pub fn auto_output_dir_for(slug: &str) -> PathBuf {
    if cfg!(windows) {
        auto_output_root().join(format!("output_{slug}"))
    } else {
        auto_output_root().join("outputs").join(slug)
    }
}

/// Directory for stock-image backups dumped during root + critical
/// flows. Same OS split as [`auto_output_root`].
///
/// `subdir` is the per-flow leaf (e.g. `backup_init_boot`,
/// `backup_critical_<ts>`). Returns `<root>/<subdir>` on Windows
/// (preserving the existing v3 layout) and
/// `<XDG data>/ltbox/backups/<subdir>` elsewhere so AppImage / distro
/// installs never write next to the executable.
pub fn backup_dir_for(subdir: &str) -> PathBuf {
    if cfg!(windows) {
        auto_output_root().join(subdir)
    } else {
        auto_output_root().join("backups").join(subdir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: outputs should never land next to a Linux/macOS
    /// installed binary. Windows is allowed to keep exe-adjacent
    /// layout per D6.
    #[test]
    fn non_windows_outputs_never_exe_adjacent() {
        if cfg!(windows) {
            return;
        }
        let exe_parent = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from));
        let Some(exe_parent) = exe_parent else {
            return;
        };
        let dir = auto_output_dir_for("patch_arb");
        assert!(
            !dir.starts_with(&exe_parent),
            "auto_output_dir_for landed under exe parent on a non-Windows host: {} ⊂ {}",
            dir.display(),
            exe_parent.display(),
        );
    }

    /// Backup helper must mirror the same exe-adjacency rule.
    #[test]
    fn non_windows_backups_never_exe_adjacent() {
        if cfg!(windows) {
            return;
        }
        let exe_parent = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from));
        let Some(exe_parent) = exe_parent else {
            return;
        };
        let dir = backup_dir_for("backup_init_boot");
        assert!(
            !dir.starts_with(&exe_parent),
            "backup_dir_for landed under exe parent on a non-Windows host: {} ⊂ {}",
            dir.display(),
            exe_parent.display(),
        );
    }

    /// Per-action dirs must be distinct so wizard outputs never
    /// collide.
    #[test]
    fn auto_output_dir_distinct_per_slug() {
        let a = auto_output_dir_for("patch_arb");
        let b = auto_output_dir_for("region_convert");
        assert_ne!(a, b);
    }

    /// On Windows the v3 layout is `<exe-dir>/output_<slug>`. Pin it
    /// so a future refactor doesn't quietly relocate Windows outputs
    /// without bumping the docs.
    #[cfg(windows)]
    #[test]
    fn windows_keeps_v3_exe_adjacent_layout() {
        let dir = auto_output_dir_for("patch_arb");
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .expect("output dir name");
        assert_eq!(name, "output_patch_arb");
    }
}
