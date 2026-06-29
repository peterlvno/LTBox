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
//! Windows keeps the existing exe-adjacent layout for continuity
//! with current v3 testers; Linux and other unixes always go
//! through XDG-style data dirs.
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

/// Per-flow exec-time scratch directory. Caller is responsible for
/// `remove_dir_all` on entry + `create_dir_all` before writes; this
/// helper only resolves the path. Slug is the flow identifier
/// (`"flash_arb"`, `"flash_country"`, `"root"`, …). Routes through
/// [`auto_output_root`] so AppImage / distro installs and the
/// Windows exe-adjacent layout stay consistent with every other
/// LTBox-owned write.
pub fn work_dir_for(slug: &str) -> PathBuf {
    if cfg!(windows) {
        auto_output_root().join(format!("work_{slug}"))
    } else {
        auto_output_root().join("work").join(slug)
    }
}

/// Remove every exec-time scratch directory created by [`work_dir_for`].
/// Call on a *successful* operation so the `work_*` scratch (firmware flash,
/// country change, ARB overlays, …) does not accumulate; a mid-flow abort
/// deliberately leaves it behind for inspection. Best-effort — errors ignored.
///
/// Direct removal only — no size accounting, so a successful op never pays for
/// a tree walk over (potentially large) decrypted images. `remove_dir_all`
/// refuses to follow a symlinked root, so this can't delete outside the LTBox
/// scratch tree. The Settings UI uses the separate
/// [`clean_temp_files_reporting`] when it needs a tally.
pub fn clean_work_dirs() {
    if cfg!(windows) {
        // `work_*` siblings of the exe; leave `output_*` / backups alone.
        let Ok(entries) = std::fs::read_dir(auto_output_root()) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("work_") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    } else {
        let _ = std::fs::remove_dir_all(auto_output_root().join("work"));
    }
}

/// Names under [`auto_output_root`] (Windows) that the user-facing "clean
/// temporary files" action removes: per-action auto-output piles
/// (`output_<slug>`) and exec-time scratch (`work_<slug>`). The persistent
/// `adb/` key dir and every `backup*` dump are deliberately excluded.
///
/// Only the Windows code paths call this, but it stays compiled on every
/// target: the callers reference it from inside a runtime `cfg!(windows)`
/// branch, which is still type-checked on Linux/macOS.
fn is_temp_entry_name(name: &str) -> bool {
    name.starts_with("work_") || name.starts_with("output_")
}

/// `true` only if `path` is a real directory — a symlink (even one pointing at
/// a directory) reports `false`. Uses `symlink_metadata` so the temp scan and
/// sweep never follow a symlinked root out of the LTBox tree.
fn is_real_dir(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
}

/// On-disk size in bytes of everything [`clean_temp_files_reporting`] would
/// remove. Drives the Settings button's enabled state and the size readout —
/// `0` means there is nothing to clean. `adb/` and `backup*` are never counted.
/// Symlinked roots are skipped, matching what the sweep can actually remove.
pub fn temp_files_size() -> u64 {
    let root = auto_output_root();
    if cfg!(windows) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|e| {
                is_temp_entry_name(&e.file_name().to_string_lossy())
                    && e.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
            })
            .map(|e| dir_size(&e.path()))
            .sum()
    } else {
        // Linux/macOS keep one tree per category under the XDG data dir.
        let work = root.join("work");
        let outputs = root.join("outputs");
        let work_size = if is_real_dir(&work) {
            dir_size(&work)
        } else {
            0
        };
        let outputs_size = if is_real_dir(&outputs) {
            dir_size(&outputs)
        } else {
            0
        };
        work_size + outputs_size
    }
}

/// Remove every temporary file the Settings "clean temporary files" action
/// targets — `work_*` scratch + `output_*` auto-output dirs (Windows) /
/// `work/` + `outputs/` (other OSes) — and report `(removed_roots,
/// freed_bytes)`. The persistent `adb/` key dir and all `backup*` dumps are
/// left in place. Symlinked roots are skipped (never followed), so the sweep
/// can't escape the LTBox tree. Best-effort: a failed delete is not counted.
pub fn clean_temp_files_reporting() -> (usize, u64) {
    let mut removed = 0usize;
    let mut freed = 0u64;
    let root = auto_output_root();
    if cfg!(windows) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            return (removed, freed);
        };
        for entry in entries.flatten() {
            if !is_temp_entry_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            // `file_type` doesn't follow links; skip a symlinked entry so we
            // never recurse-size or delete through it.
            if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let size = dir_size(&path);
            if std::fs::remove_dir_all(&path).is_ok() {
                removed += 1;
                freed += size;
            }
        }
    } else {
        for leaf in ["work", "outputs"] {
            let path = root.join(leaf);
            // Only act on a real directory — never follow a symlinked root
            // into unrelated user dirs.
            if !is_real_dir(&path) {
                continue;
            }
            let size = dir_size(&path);
            if std::fs::remove_dir_all(&path).is_ok() {
                removed += 1;
                freed += size;
            }
        }
    }
    (removed, freed)
}

/// Recursive on-disk size of `path` in bytes. Best-effort: entries that can't
/// be read are skipped (counted as 0) rather than aborting the walk.
///
/// Symlinks are **not** followed: [`std::fs::DirEntry::file_type`] reports the
/// link itself, so a symlinked directory is treated as a link and skipped —
/// matching what `remove_dir_all` actually deletes, and avoiding both symlink
/// cycles and escaping the temp tree into unrelated directories.
fn dir_size(path: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            total += dir_size(&entry.path());
        } else if file_type.is_file() {
            // `metadata()` follows links, but we only reach it for a real
            // file here, so the size is the file's own.
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
        // Symlinks / special files contribute 0 — they aren't recursed into.
    }
    total
}

/// Path to LTBox's owned ADB RSA private key. Persisted so the user
/// only has to tap "Allow USB debugging?" once per device — `adb_client`'s
/// `usb` backend mints a fresh in-memory key whenever the key file is
/// missing, which would re-trigger the on-device prompt on every
/// `AdbManager::new()` if we let it fall through to the default
/// `~/.android/adbkey`.
///
/// Stored under [`auto_output_root`] / `adb/adbkey` so it inherits the
/// same OS-specific writable-directory split as every other LTBox
/// asset.
pub fn adb_key_path() -> PathBuf {
    auto_output_root().join("adb").join("adbkey")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: outputs should never land next to a Linux/macOS
    /// installed binary. Windows is allowed to keep exe-adjacent
    /// layout for v3 continuity.
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

    /// `dir_size` sums nested files; the cleanup tally relies on it being
    /// accurate before the tree is removed.
    #[test]
    fn dir_size_sums_nested_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join("a.bin"), [0u8; 10]).expect("write a");
        let sub = root.join("nested");
        std::fs::create_dir_all(&sub).expect("mkdir nested");
        std::fs::write(sub.join("b.bin"), [0u8; 25]).expect("write b");
        assert_eq!(dir_size(root), 35);
        // Missing path measures as 0 rather than panicking.
        assert_eq!(dir_size(&root.join("does-not-exist")), 0);
    }

    /// The Windows temp-cleanup filter removes scratch + auto-output piles
    /// but must keep the persistent ADB key dir and every backup dump.
    #[cfg(windows)]
    #[test]
    fn temp_entry_filter_keeps_adb_and_backups() {
        assert!(is_temp_entry_name("work_root"));
        assert!(is_temp_entry_name("output_patch_arb"));
        assert!(!is_temp_entry_name("adb"));
        assert!(!is_temp_entry_name("backup_init_boot"));
        assert!(!is_temp_entry_name("backup_critical_1700000000"));
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
