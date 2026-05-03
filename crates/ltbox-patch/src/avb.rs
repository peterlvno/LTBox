//! AVB patching — wraps avbtool-rs library for image signing operations.

use fs_err as fs;
use std::path::{Path, PathBuf};

use ltbox_core::{LtboxError, Result};
use tracing::info;

/// Parsed AVB image metadata.
#[derive(Debug, Clone)]
pub struct AvbImageInfo {
    pub partition_size: u64,
    pub algorithm: String,
    pub rollback_index: u64,
    pub flags: u32,
    pub partition_name: Option<String>,
    pub salt: Option<Vec<u8>>,
    pub public_key_sha1: Option<String>,
    pub props: Vec<(String, Vec<u8>)>,
}

/// Render `avbtool info_image`-style metadata for one or more images.
pub fn image_info_report(image_paths: &[PathBuf]) -> Result<String> {
    if image_paths.is_empty() {
        return Err(LtboxError::Avb("No image files selected".to_string()));
    }

    let mut reports = Vec::with_capacity(image_paths.len());
    for path in image_paths {
        let report = avbtool_rs::info::generate_info_report(path)
            .map_err(|e| LtboxError::Avb(format!("info_image {}: {e}", path.display())))?;
        reports.push(report.trim_end().to_string());
    }
    Ok(reports.join("\n================================================================\n\n"))
}

/// Extract AVB metadata from an image.
pub fn extract_image_avb_info(image_path: &Path) -> Result<AvbImageInfo> {
    let info = avbtool_rs::image::inspect_avb_image(image_path)
        .map_err(|e| LtboxError::Avb(format!("inspect {}: {e}", image_path.display())))?;

    let file_size = fs::metadata(image_path).map(|m| m.len()).unwrap_or(0);
    let partition_size = if info.footer.is_some() {
        file_size
    } else {
        avbtool_rs::image::compute_vbmeta_blob_size(&info.header).unwrap_or(0)
    };

    let mut partition_name = None;
    let mut salt = None;
    let mut props = Vec::new();

    for desc in &info.descriptors {
        match desc {
            avbtool_rs::info::DescriptorInfo::Hash {
                partition_name: pn,
                salt: s,
                ..
            } if partition_name.is_none() => {
                partition_name = Some(pn.clone());
                salt = Some(s.clone());
            }
            avbtool_rs::info::DescriptorInfo::Hashtree {
                partition_name: pn,
                salt: s,
                ..
            } if partition_name.is_none() => {
                partition_name = Some(pn.clone());
                salt = Some(s.clone());
            }
            avbtool_rs::info::DescriptorInfo::Property { key, value } => {
                props.push((key.clone(), value.clone()));
            }
            _ => {}
        }
    }

    Ok(AvbImageInfo {
        partition_size,
        algorithm: info.algorithm_name.clone(),
        rollback_index: info.header.rollback_index,
        flags: info.header.flags,
        partition_name,
        salt,
        public_key_sha1: info.public_key_sha1.clone(),
        props,
    })
}

/// Resign an image. `key_spec` → bundled name (`testkey_rsa2048` / …)
/// or filesystem path to a PEM; passed to `avbtool_rs::crypto::load_key_from_spec`.
pub fn resign_image(
    image_path: &Path,
    key_spec: &str,
    algorithm: &str,
    rollback_index: Option<u64>,
) -> Result<()> {
    avbtool_rs::resign::resign_image_with_options(
        image_path,
        key_spec,
        Some(algorithm),
        false,
        rollback_index,
        false,
    )
    .map_err(|e| LtboxError::Avb(format!("resign failed: {e}")))?;
    Ok(())
}

/// Erase AVB footer from an image.
pub fn erase_footer(image_path: &Path) -> Result<()> {
    avbtool_rs::footer::erase_footer(image_path, false)
        .map_err(|e| LtboxError::Avb(format!("erase_footer failed: {e}")))?;
    Ok(())
}

/// Rebuild `vbmeta.img` using the original as a template, with hash
/// descriptors recomputed from the current bytes of `chained_images`.
/// `key_spec` follows the [`resign_image`] convention.
pub fn rebuild_vbmeta_with_chained_images(
    output_path: &Path,
    original_vbmeta_path: &Path,
    chained_images: &[&Path],
    key_spec: &str,
    algorithm: Option<&str>,
) -> Result<()> {
    avbtool_rs::builder::rebuild_vbmeta_image(
        output_path,
        original_vbmeta_path,
        chained_images,
        key_spec,
        algorithm,
    )
    .map_err(|e| LtboxError::Avb(format!("rebuild_vbmeta_image: {e}")))?;
    preserve_original_vbmeta_size(output_path, original_vbmeta_path)?;
    Ok(())
}

fn preserve_original_vbmeta_size(output_path: &Path, original_vbmeta_path: &Path) -> Result<()> {
    let original_size = fs::metadata(original_vbmeta_path)?.len();
    let output_size = fs::metadata(output_path)?.len();
    if output_size < original_size {
        let file = fs::OpenOptions::new().write(true).open(output_path)?;
        file.set_len(original_size)?;
    }
    Ok(())
}

/// Add hash footer. `key_spec` follows [`resign_image`]; pass `None`
/// for the NONE-algorithm path (no signing).
pub fn add_hash_footer(
    image_path: &Path,
    info: &AvbImageInfo,
    key_spec: Option<&str>,
    new_rollback_index: Option<u64>,
) -> Result<()> {
    let rollback = new_rollback_index.unwrap_or(info.rollback_index);
    // Must bail loudly — the hash footer pins this name into the re-signed blob
    // and the bootloader refuses to mount if it doesn't match the recorded name.
    let name = info.partition_name.as_deref().ok_or_else(|| {
        LtboxError::Avb(format!(
            "Cannot add AVB hash footer to {}: no partition_name in AVB info (source image has no Hash/Hashtree descriptor)",
            image_path.display()
        ))
    })?;
    info!("Adding AVB footer: partition={name}, rollback={rollback}");

    let salt_bytes = info.salt.clone();

    let properties = info
        .props
        .iter()
        .map(|(k, v)| avbtool_rs::builder::PropertySpec {
            key: k.clone(),
            value: v.clone(),
        })
        .collect();

    let args = avbtool_rs::footer::HashFooterArgs {
        partition_size: Some(info.partition_size),
        dynamic_partition_size: false,
        partition_name: name.to_string(),
        hash_algorithm: "sha256".to_string(),
        salt: salt_bytes,
        chain_partitions: Vec::new(),
        algorithm_name: info.algorithm.clone(),
        key_spec: key_spec.map(|s| s.to_string()),
        public_key_metadata: None,
        rollback_index: rollback,
        flags: info.flags,
        rollback_index_location: 0,
        properties,
        kernel_cmdlines: Vec::new(),
        include_descriptors_from_images: Vec::new(),
        release_string: None,
        append_to_release_string: None,
        output_vbmeta_image: None,
        do_not_append_vbmeta_image: false,
        use_persistent_digest: false,
        do_not_use_ab: false,
    };

    avbtool_rs::footer::add_hash_footer(image_path, &args)
        .map_err(|e| LtboxError::Avb(format!("add_hash_footer failed: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{key_map, region};

    #[test]
    fn image_info_report_accepts_non_avb_img() {
        let tmp = tempfile::tempdir().unwrap();
        let image = tmp.path().join("plain.img");
        fs::write(&image, [0u8; 16]).unwrap();

        let report = image_info_report(&[image]).unwrap();

        assert!(report.contains("AVB image type:"));
        assert!(report.contains("No AVB metadata found."));
    }

    #[test]
    fn image_info_report_requires_selection() {
        let err = image_info_report(&[]).unwrap_err().to_string();
        assert!(err.contains("No image files selected"));
    }

    #[test]
    fn preserve_original_vbmeta_size_pads_short_rebuild_output() {
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("vbmeta.img");
        let output = tmp.path().join("vbmeta.rebuilt.img");
        fs::write(&original, vec![0u8; 8192]).unwrap();
        fs::write(&output, vec![1u8; 4096]).unwrap();

        preserve_original_vbmeta_size(&output, &original).unwrap();

        assert_eq!(fs::metadata(&output).unwrap().len(), 8192);
        let data = fs::read(&output).unwrap();
        assert!(data[..4096].iter().all(|b| *b == 1));
        assert!(data[4096..].iter().all(|b| *b == 0));
    }

    #[test]
    fn preserve_original_vbmeta_size_never_truncates_larger_output() {
        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("vbmeta.img");
        let output = tmp.path().join("vbmeta.rebuilt.img");
        fs::write(&original, vec![0u8; 4096]).unwrap();
        fs::write(&output, vec![1u8; 8192]).unwrap();

        preserve_original_vbmeta_size(&output, &original).unwrap();

        assert_eq!(fs::metadata(&output).unwrap().len(), 8192);
    }

    #[test]
    fn real_firmware_avb_matrix_when_available() {
        let Some(dir) = std::env::var_os("LTBOX_REAL_FIRMWARE_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let original_vbmeta = dir.join("vbmeta.img");
        let original_vendor_boot = dir.join("vendor_boot.img");
        let original_boot = dir.join("boot.img");
        let original_vbmeta_system = dir.join("vbmeta_system.img");
        if !original_vbmeta.exists()
            || !original_vendor_boot.exists()
            || !original_boot.exists()
            || !original_vbmeta_system.exists()
        {
            return;
        }

        for target_rollback in [None, Some(1_800_000_000u64)] {
            let tmp = tempfile::tempdir().unwrap();
            let vbmeta = tmp.path().join("vbmeta.img");
            let vendor_boot = tmp.path().join("vendor_boot.img");
            let patched_vendor_boot = tmp.path().join("vendor_boot.patched.img");
            let rebuilt_vbmeta = tmp.path().join("vbmeta.rebuilt.img");
            let boot = tmp.path().join("boot.img");
            let vbmeta_system = tmp.path().join("vbmeta_system.img");
            fs::copy(&original_vbmeta, &vbmeta).unwrap();
            fs::copy(&original_vendor_boot, &vendor_boot).unwrap();
            fs::copy(&original_boot, &boot).unwrap();
            fs::copy(&original_vbmeta_system, &vbmeta_system).unwrap();

            for image in [&boot, &vbmeta_system] {
                let info = extract_image_avb_info(image).unwrap();
                if let Some(target) = target_rollback {
                    let key_spec = key_map::key_spec_for_pubkey(info.public_key_sha1.as_deref())
                        .expect("real fixture rollback key should be known");
                    resign_image(image, key_spec, &info.algorithm, Some(target)).unwrap();
                    assert_eq!(
                        extract_image_avb_info(image).unwrap().rollback_index,
                        target
                    );
                } else {
                    assert_eq!(
                        extract_image_avb_info(image).unwrap().rollback_index,
                        info.rollback_index
                    );
                }
            }

            let vendor_boot_info = extract_image_avb_info(&vendor_boot).unwrap();
            let patterns = region::RegionPatternSet::default();
            let replaced = region::patch_vendor_boot(
                &vendor_boot,
                &patched_vendor_boot,
                region::RegionTarget::Prc,
                &patterns.prc_patterns,
                &patterns.row_patterns,
            )
            .unwrap();
            assert!(replaced > 0);
            add_hash_footer(&patched_vendor_boot, &vendor_boot_info, None, None).unwrap();

            let vbmeta_info = extract_image_avb_info(&vbmeta).unwrap();
            let key_spec = key_map::key_spec_for_pubkey(vbmeta_info.public_key_sha1.as_deref())
                .expect("real fixture vbmeta key should be known");
            rebuild_vbmeta_with_chained_images(
                &rebuilt_vbmeta,
                &vbmeta,
                &[patched_vendor_boot.as_path()],
                key_spec,
                Some(&vbmeta_info.algorithm),
            )
            .unwrap();

            assert_eq!(
                fs::metadata(&rebuilt_vbmeta).unwrap().len(),
                fs::metadata(&vbmeta).unwrap().len()
            );
            let report = image_info_report(&[rebuilt_vbmeta]).unwrap();
            assert!(report.contains("vendor_boot"));
        }
    }
}
