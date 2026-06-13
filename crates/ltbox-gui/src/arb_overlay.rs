//! TB323FU efisp ARB-overlay provisioning: pick the efisp asset variant
//! and build the per-LUN overlay set. Extracted from main.rs.

use crate::*;

/// GBL EFI asset suffix for a TB323FU target firmware, by region (`is_prc`) and
/// whether the anti-rollback build is needed (`arb`). Picks the
/// `*_prc.efi` / `*_row.efi` asset (or `*_prc_arb.efi` / `*_row_arb.efi`) from
/// the gbl_root_baldur release. The `_arb` GBL roots trust at the testkey so it
/// accepts the testkey-re-signed boot chain LTBox stages on a downgrade. The
/// region comes from the vendor_boot `product_region` DTB marker — TB323FU's AVB
/// fingerprint carries no `_PRC`/`_ROW` token.
pub(crate) fn efisp_asset_suffix(is_prc: bool, arb: bool) -> &'static str {
    match (is_prc, arb) {
        (true, false) => "_prc.efi",
        (true, true) => "_prc_arb.efi",
        (false, false) => "_row.efi",
        (false, true) => "_row_arb.efi",
    }
}

/// A dumped `efisp` partition counts as empty (un-provisioned) when every byte
/// is zero — the stock/erased state. A GBL-provisioned `efisp` carries the EFI
/// payload, so it has non-zero bytes. The TB323FU root gate refuses to proceed
/// on an empty `efisp`.
pub(crate) fn efisp_is_empty(data: &[u8]) -> bool {
    data.iter().all(|&b| b == 0)
}

/// One staged ARB overlay: (GPT label, UFS LUN, patched image path).
pub(crate) type ArbOverlay = (String, u8, std::path::PathBuf);

/// Testkey re-sign overlays for an AVB flash — used by the TB323FU anti-rollback
/// path and the non-TB323FU key2 / cross-region re-sign. The device-committed
/// boot + vbmeta_system indices come from `device_floors` (component-wise across
/// both slots on EDL-start) or are read from the active slot here.
///
/// Layout-aware: it re-signs exactly the partitions the (base) vbmeta chains —
/// each needs a matching `<part>.img` — so packages without recovery (or with a
/// different chained set) work too. boot / vbmeta_system bump to the device
/// floor (`max`, never lowered); other chained partitions (e.g. recovery) are
/// re-signed only; the vbmeta is rebuilt with every chained descriptor repointed
/// at the testkey and flashed LAST (it ties the chain together — shrinks the
/// partial-write brick window). `vbmeta_base` overrides the rebuild base: for a
/// key2 cross-region install the caller passes the region-converted, testkey
/// vbmeta so its recomputed vendor_boot hash is preserved; otherwise the
/// firmware's own `vbmeta.img`.
///
/// `force_resign` re-signs even without a downgrade (key2 firmware on a testkey
/// device). Returns `(overlays, need)`; `need` is the downgrade flag — the
/// TB323FU caller swaps the efisp GBL to its `_arb` (testkey-root) variant.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_tb323fu_arb_overlays(
    session: &mut ltbox_device::edl::EdlSession,
    fw_dir: &std::path::Path,
    work_dir: &std::path::Path,
    slot: Option<&str>,
    device_floors: Option<(u64, u64)>,
    force_resign: bool,
    vbmeta_base: Option<&std::path::Path>,
    log: &mut Vec<String>,
) -> std::result::Result<(Vec<ArbOverlay>, bool), String> {
    const KEY: &str = "testkey_rsa4096";
    const ALGO: &str = "SHA256_RSA4096";

    let lun_of = |label: &str| -> std::result::Result<u8, String> {
        ltbox_core::partition_lun::lun_for_partition(label)
            .ok_or_else(|| format!("no hardcoded LUN for {label}"))
    };
    let idx_of = |path: &std::path::Path| -> std::result::Result<u64, String> {
        Ok(ltbox_patch::avb::extract_image_avb_info(path)
            .map_err(|e| format!("AVB inspect {}: {e}", path.display()))?
            .rollback_index)
    };

    // 1. Rechain base vbmeta (caller override for cross-region, else firmware's)
    //    and the partitions it chains. Re-sign + rechain only the ones we can
    //    handle: a plain partition name, an install image, and a resolvable A/B
    //    GPT label/LUN. Other chained partitions (e.g. vbmeta_vendor) keep their
    //    stock chain descriptor + stock image.
    let inst_vbmeta = vbmeta_base
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| fw_dir.join("vbmeta.img"));
    if !inst_vbmeta.exists() {
        return Err(format!("install image missing: {}", inst_vbmeta.display()));
    }
    let chained = ltbox_patch::avb::chain_partitions(&inst_vbmeta)
        .map_err(|e| format!("vbmeta chain partitions: {e}"))?;
    let inst_img = |p: &str| fw_dir.join(format!("{p}.img"));
    let has = |name: &str| chained.iter().any(|c| c.name == name);
    // GPT label for a chained partition: `_a` for A/B, the unsuffixed name for a
    // `DO_NOT_USE_AB` chain (AVB verifies the unsuffixed partition).
    let label_of = |c: &ltbox_patch::avb::ChainPartition| -> String {
        if c.do_not_use_ab {
            c.name.clone()
        } else {
            format!("{}_a", c.name)
        }
    };
    // The floor read + index bump below assume A/B slots for the rollback-
    // protected partitions, so reject a non-A/B boot / vbmeta_system layout.
    if chained
        .iter()
        .any(|c| (c.name == "boot" || c.name == "vbmeta_system") && c.do_not_use_ab)
    {
        return Err("non-A/B boot/vbmeta_system rollback layout is unsupported".to_string());
    }
    // 2. Device-committed per-location indices (boot + vbmeta_system). On an
    //    EDL-start flash the caller passes component-wise maxima already read
    //    across BOTH slots; otherwise read the ACTIVE slot here (a first-time
    //    user may still be on `_b`, so don't assume `_a`).
    let (dev_boot_idx, dev_vbs_idx) = match device_floors {
        Some(floors) => floors,
        None => {
            let dev_boot = format!("boot{}", active_slot_suffix(slot));
            let dev_vbs = format!("vbmeta_system{}", active_slot_suffix(slot));
            let dev_boot_img = work_dir.join(format!("dev_{dev_boot}.img"));
            let dev_vbs_img = work_dir.join(format!("dev_{dev_vbs}.img"));
            session
                .dump_partition(&dev_boot, &dev_boot_img, 0, lun_of(&dev_boot)?, log)
                .map_err(|e| format!("dump device {dev_boot}: {e}"))?;
            session
                .dump_partition(&dev_vbs, &dev_vbs_img, 0, lun_of(&dev_vbs)?, log)
                .map_err(|e| format!("dump device {dev_vbs}: {e}"))?;
            let b = idx_of(&dev_boot_img)?;
            let v = idx_of(&dev_vbs_img)?;
            let _ = std::fs::remove_file(&dev_boot_img);
            let _ = std::fs::remove_file(&dev_vbs_img);
            (b, v)
        }
    };

    // 3. Rollback-protected install indices (boot + vbmeta_system when chained).
    let inst_boot_idx = if has("boot") {
        idx_of(&inst_img("boot"))?
    } else {
        0
    };
    let inst_vbs_idx = if has("vbmeta_system") {
        idx_of(&inst_img("vbmeta_system"))?
    } else {
        0
    };
    ltbox_core::live!(
        log,
        "[ARB] {}",
        tr_args!(
            "live_arb_tb323_indices",
            boot_i = inst_boot_idx.to_string(),
            boot_d = dev_boot_idx.to_string(),
            vbs_i = inst_vbs_idx.to_string(),
            vbs_d = dev_vbs_idx.to_string()
        )
    );

    // 4. need = a rollback-protected install image is behind the device index.
    let need = (has("boot") && inst_boot_idx < dev_boot_idx)
        || (has("vbmeta_system") && inst_vbs_idx < dev_vbs_idx);
    if !need && !force_resign {
        ltbox_core::live!(
            log,
            "[ARB] {}",
            ltbox_core::i18n::tr("live_arb_tb323_skip_uptodate")
        );
        return Ok((Vec::new(), false));
    }

    // 5. A re-sign is needed: validate + select the chained partitions to
    //    re-sign. Two distinct failure modes:
    //   * Missing image — the firmware chains the partition but ships no
    //     `<part>.img`, so the testkey root would delegate to a partition the
    //     package may never flash. Abort: a stale child fails AVB verification.
    //   * Image present but the LUN is not in the static map (e.g. vbmeta_vendor)
    //     — no overlay can be staged, so leave it stock: rawprogram still flashes
    //     the firmware's own image, which matches its preserved (firmware-key)
    //     chain descriptor under the testkey-signed root.
    // The rollback-protected boot / vbmeta_system MUST be re-signable.
    let mut to_resign: Vec<&ltbox_patch::avb::ChainPartition> = Vec::new();
    for c in &chained {
        if c.name.is_empty()
            || !c
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(format!(
                "unsafe chain partition name in vbmeta: {:?}",
                c.name
            ));
        }
        if !inst_img(&c.name).exists() {
            return Err(format!(
                "vbmeta chains {} but its install image is missing: {}",
                c.name,
                inst_img(&c.name).display()
            ));
        }
        if ltbox_core::partition_lun::lun_for_partition(&label_of(c)).is_some() {
            to_resign.push(c);
        } else if c.name == "boot" || c.name == "vbmeta_system" {
            return Err(format!(
                "rollback-protected {} is chained but its LUN is unknown",
                c.name
            ));
        }
    }

    // 6. Re-sign each handled chained partition to the testkey. boot /
    //    vbmeta_system bump to the device floor (never lower the image's own
    //    claim, hence max()); others are re-signed only. Flash boot LAST among
    //    them (just before vbmeta_a) to shrink the partial-write brick window.
    let boot_target = inst_boot_idx.max(dev_boot_idx);
    let vbs_target = inst_vbs_idx.max(dev_vbs_idx);
    // Re-sign with the testkey's OWN algorithm (derived from KEY), not the source
    // image's: a key2 image may use a different RSA size, which avbtool would
    // reject against the testkey.
    let key_algo = ltbox_patch::avb::algorithm_for_key_spec(KEY)
        .ok_or_else(|| format!("unknown AVB algorithm for {KEY}"))?;
    let mut ordered = to_resign.clone();
    ordered.sort_by_key(|c| u8::from(c.name == "boot"));
    let mut overlays: Vec<ArbOverlay> = Vec::new();
    for &c in &ordered {
        let name = c.name.as_str();
        let out = work_dir.join(format!("{name}.arb.img"));
        std::fs::copy(inst_img(name), &out).map_err(|e| format!("copy {name}: {e}"))?;
        let target = match name {
            "boot" => Some(boot_target),
            "vbmeta_system" => Some(vbs_target),
            _ => None,
        };
        ltbox_patch::avb::resign_image(&out, KEY, &key_algo, target)
            .map_err(|e| format!("resign {name}: {e}"))?;
        let label = label_of(c);
        let lun = lun_of(&label)?;
        overlays.push((label, lun, out));
    }

    // 7. Rebuild vbmeta on the base, repointing the re-signed chained descriptors
    //    at the testkey (others keep their stock pubkey); flash it LAST.
    let out_vbmeta = work_dir.join("vbmeta.arb.img");
    let chained_refs: Vec<&str> = to_resign.iter().map(|c| c.name.as_str()).collect();
    ltbox_patch::avb::rebuild_vbmeta_rechained(
        &out_vbmeta,
        &inst_vbmeta,
        &chained_refs,
        KEY,
        KEY,
        ALGO,
    )
    .map_err(|e| format!("rebuild vbmeta: {e}"))?;
    overlays.push(("vbmeta_a".to_string(), lun_of("vbmeta_a")?, out_vbmeta));
    ltbox_core::live!(
        log,
        "[ARB] {}",
        tr_args!(
            "live_arb_tb323_resigned",
            boot = boot_target.to_string(),
            vbs = vbs_target.to_string()
        )
    );

    Ok((overlays, need))
}
