//! KernelSU LKM patching — replaces `init` in `init_boot.img` with
//! KernelSU's bootstrap binary and stages `kernelsu.ko` so the stock
//! kernel loads the module at boot. Works for KernelSU, KSU-Next, and forks.

use std::path::{Path, PathBuf};

use ltbox_core::i18n::tr;
use ltbox_core::{LtboxError, Result};

use crate::boot;

/// Patch `init_boot.img` with KernelSU. `work_dir` must contain
/// `init_boot.img`, `init` (ksuinit), and `kernelsu.ko`.
/// Writes `work_dir/new-boot.img`; caller handles AVB resign + flash.
pub fn patch_init_boot(work_dir: &Path, log: &mut Vec<String>) -> Result<PathBuf> {
    let img_name = "init_boot.img";
    let img_path = work_dir.join(img_name);
    if !img_path.exists() {
        return Err(LtboxError::Patch(format!(
            "init_boot.img not found in {}",
            work_dir.display()
        )));
    }
    for needed in ["init", "kernelsu.ko"] {
        if !work_dir.join(needed).exists() {
            return Err(LtboxError::Patch(format!(
                "KSU payload '{needed}' missing from {}",
                work_dir.display()
            )));
        }
    }

    log.push(format!("[KSU] {}", tr("log_ksu_unpack_initboot")));
    boot::unpack(&img_path, work_dir)?;

    let ramdisk = work_dir.join("ramdisk.cpio");
    if !ramdisk.exists() {
        return Err(LtboxError::Patch(
            "ramdisk.cpio missing after unpack — boot image has no ramdisk".into(),
        ));
    }

    // Refuse to double-patch: init.real only exists after a prior KSU run.
    let existing_real = boot::cpio(work_dir, "ramdisk.cpio", &["exists init.real"])?;
    if existing_real == 0 {
        return Err(LtboxError::Patch(
            "init_boot.img is already KernelSU-patched — flash stock first".into(),
        ));
    }

    // Move stock init → init.real so ksuinit can chain to it. Loose-ramdisk
    // images have no top-level init, so skip the rename there.
    let has_init = boot::cpio(work_dir, "ramdisk.cpio", &["exists init"])?;
    if has_init == 0 {
        log.push(format!("[KSU] {}", tr("log_ksu_cpio_mv_init")));
        boot::cpio_checked(work_dir, "ramdisk.cpio", &["mv init init.real"])?;
    } else {
        log.push(format!("[KSU] {}", tr("log_ksu_no_stock_init")));
    }

    log.push(format!("[KSU] {}", tr("log_ksu_cpio_add")));
    boot::cpio_checked(work_dir, "ramdisk.cpio", &["add 0755 init init"])?;
    boot::cpio_checked(
        work_dir,
        "ramdisk.cpio",
        &["add 0755 kernelsu.ko kernelsu.ko"],
    )?;

    log.push(format!("[KSU] {}", tr("log_ksu_repack_initboot")));
    boot::repack(img_name, work_dir)?;

    let new_boot = work_dir.join("new-boot.img");
    if !new_boot.exists() {
        return Err(LtboxError::Patch(
            "magiskboot repack produced no new-boot.img".into(),
        ));
    }
    log.push(format!("[KSU] {}", tr("log_ksu_patch_complete")));
    Ok(new_boot)
}
