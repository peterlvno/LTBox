//! Unroot worker: restore stock boot/init_boot + vbmeta from a backup
//! folder over EDL. Extracted from the update_unroot handler.

use crate::{
    ConnectionStatus, LiveLabels, UnrootType, find_edl_loader, phase_marker, transition_to_edl,
};
use ltbox_core::{live, tr_args};

pub(crate) fn unroot_worker(
    folder: String,
    unroot_type: UnrootType,
    loader_override: Option<String>,
    conn: ConnectionStatus,
    ll: LiveLabels,
) -> Result<Vec<String>, String> {
    let mut log = Vec::new();
    let dir = std::path::Path::new(&folder);

    let (boot_name, base_part) = match unroot_type {
        UnrootType::MagiskLkm => ("init_boot.img", "init_boot"),
        UnrootType::APatchGki => ("boot.img", "boot"),
    };
    let boot_path = dir.join(boot_name);
    let vbmeta_path = dir.join("vbmeta.img");
    if !boot_path.exists() {
        return Err(format!("{boot_name} not found in selected folder"));
    }
    if !vbmeta_path.exists() {
        return Err("vbmeta.img not found in selected folder".to_string());
    }
    live!(
        log,
        "[Unroot] {}",
        tr_args!("live_unroot_backup_pair", boot = boot_name)
    );

    // Slot resolution must succeed —
    // unroot writes init_boot_<slot> +
    // vbmeta_<slot> from the user's
    // backup folder. Defaulting to `_a`
    // when the device was on `_b`
    // restored stale stock blobs to the
    // wrong slot and left the active
    // slot still rooted, with no clear
    // signal to the user.
    let slot =
        ltbox_device::controller::poll_active_slot(std::time::Duration::from_secs(30), &mut log)
            .map_err(|e| format!("Unroot slot resolve: {e}"))?;

    // Decoupled loader — explicit picker /
    // Settings default takes priority. Fall back
    // to scanning the backup folder only when no
    // override was set, preserving v3-pre-decouple
    // behaviour for users who still ship a loader
    // alongside the backup images.
    let loader = match loader_override.clone() {
        Some(p) => std::path::PathBuf::from(p),
        None => find_edl_loader(dir)
            .or_else(|| dir.parent().and_then(find_edl_loader))
            .ok_or_else(|| format!("xbl_s_devprg_ns.melf not found under {}", dir.display()))?,
    };
    live!(
        log,
        "[Unroot] {}",
        tr_args!(
            "live_unroot_loader_path",
            path = loader.display().to_string()
        )
    );

    // Boot + vbmeta resolve through the
    // hardcoded LUN map; GPT-by-name reads
    // the slot's start sector from the
    // device. No rawprogram parse needed —
    // the loader's parent dir may not even
    // contain a firmware XML pair.
    let boot_label = format!("{base_part}{slot}");
    let vbm_label = format!("vbmeta{slot}");
    let boot_lun = ltbox_core::partition_lun::lun_for_partition(base_part)
        .ok_or_else(|| format!("No hardcoded LUN for {base_part}"))?;
    let vbm_lun = ltbox_core::partition_lun::lun_for_partition("vbmeta")
        .ok_or_else(|| "No hardcoded LUN for vbmeta".to_string())?;
    live!(
        log,
        "[Unroot] {}",
        tr_args!(
            "log_unroot_lun_resolved",
            boot_label = boot_label,
            boot_lun = boot_lun,
            vbm_label = vbm_label,
            vbm_lun = vbm_lun,
        )
    );

    live!(
        log,
        "[Unroot] {}",
        phase_marker(1, 3, &ll.op_unroot_phase[0])
    );
    transition_to_edl(conn, &ll, &mut log)?;

    live!(
        log,
        "[Unroot] {} ({})",
        phase_marker(2, 3, &ll.op_unroot_phase[1]),
        tr_args!("live_unroot_backup_pair", boot = boot_name)
    );
    let mut session = ltbox_device::edl::EdlSession::open(&loader, true, &mut log)
        .map_err(|e| format!("EDL session error: {e}"))?;
    session
        .flash_partition(&boot_label, &boot_path, 0, boot_lun, &mut log)
        .map_err(|e| format!("Flash {boot_label} failed: {e}"))?;
    session
        .flash_partition(&vbm_label, &vbmeta_path, 0, vbm_lun, &mut log)
        .map_err(|e| format!("Flash {vbm_label} failed: {e}"))?;

    println!();
    live!(
        log,
        "[Unroot] {}",
        phase_marker(3, 3, &ll.op_unroot_phase[2])
    );
    session
        .reset(&mut log)
        .map_err(|e| format!("Reset failed: {e}"))?;
    live!(log, "[Unroot] {}", ll.unroot_completed);
    Ok(log)
}
