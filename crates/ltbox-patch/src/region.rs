//! Region patching — binary pattern replacement for PRC↔ROW conversion.
//!
//! Patches vendor_boot.img and devinfo/persist country codes.

use fs_err as fs;
use std::path::{Path, PathBuf};

use crate::avb::AvbImageInfo;
use crate::{avb, key_map};
use ltbox_core::{LtboxError, Result};
use tracing::info;

/// AVB property key that carries the vendor_boot fingerprint — stock
/// Lenovo firmware embeds the full `ro.vendor.build.fingerprint` string
/// so the device model is a substring match away.
const VENDOR_BOOT_FINGERPRINT_KEY: &str = "com.android.build.vendor_boot.fingerprint";

/// Outcome of validating a vendor_boot image against a device model.
/// Matches v2 `_validate_device_model` three-way result: the fingerprint
/// may match, mismatch, or be absent entirely (older firmware).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelValidation {
    /// Fingerprint present and contains `device_model`. Safe to proceed.
    Match { fingerprint: String },
    /// Fingerprint present but does NOT contain `device_model`. The
    /// firmware is for a different model — caller should abort.
    Mismatch {
        fingerprint: String,
        device_model: String,
    },
    /// No fingerprint property in the AVB image. Caller decides — v2
    /// logged a warning and skipped validation.
    Missing,
}

/// Check an AVB-extracted `vendor_boot.img` against the ADB-reported
/// device model. Mirrors v2 `_validate_device_model` in
/// `bin/ltbox/actions/region.py`: pulls
/// `com.android.build.vendor_boot.fingerprint` from the image's AVB
/// props and looks for the device model as a substring.
///
/// Spaces in `device_model` are stripped to tolerate
/// `"TB 320FC"`-style reads from `ro.product.model`.
pub fn validate_device_model(info: &AvbImageInfo, device_model: &str) -> ModelValidation {
    let normalized = device_model.replace(' ', "");
    let fingerprint = info
        .props
        .iter()
        .find(|(k, _)| k == VENDOR_BOOT_FINGERPRINT_KEY)
        .and_then(|(_, v)| std::str::from_utf8(v).ok())
        .map(|s| s.trim_end_matches('\0').to_string());

    match fingerprint {
        None => ModelValidation::Missing,
        Some(fp) if normalized.is_empty() => ModelValidation::Match { fingerprint: fp },
        Some(fp) if fp.contains(&normalized) => ModelValidation::Match { fingerprint: fp },
        Some(fp) => ModelValidation::Mismatch {
            fingerprint: fp,
            device_model: normalized,
        },
    }
}

/// Direction of region conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionTarget {
    Prc,
    Row,
}

/// Vendor-boot region markers. `prc_patterns` are applied when the target is
/// ROW (PRC -> ROW); `row_patterns` are applied when the target is PRC
/// (ROW -> PRC).
#[derive(Debug, Clone)]
pub struct RegionPatternSet {
    pub prc_patterns: Vec<(Vec<u8>, Vec<u8>)>,
    pub row_patterns: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Default for RegionPatternSet {
    fn default() -> Self {
        let prc_dot = b".PRC".to_vec();
        let prc_i = b"IPRC".to_vec();
        let row_dot = b".ROW".to_vec();
        let row_i = b"IROW".to_vec();
        Self {
            prc_patterns: vec![
                (prc_dot.clone(), row_dot.clone()),
                (prc_i.clone(), row_i.clone()),
            ],
            row_patterns: vec![(row_dot, prc_dot), (row_i, prc_i)],
        }
    }
}

/// Region markers currently present in `vendor_boot.img`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedRegion {
    Prc,
    Row,
    Mixed,
    Unknown,
}

impl DetectedRegion {
    fn matches_target(self, target: RegionTarget) -> bool {
        matches!(
            (self, target),
            (Self::Prc, RegionTarget::Prc) | (Self::Row, RegionTarget::Row)
        )
    }
}

/// Final region-converted boot chain written by
/// [`build_region_converted_boot_chain`].
#[derive(Debug, Clone)]
pub struct RegionBootChainOutput {
    pub vendor_boot: PathBuf,
    pub vbmeta: PathBuf,
    pub source_region: DetectedRegion,
    pub target: RegionTarget,
    pub replacement_count: usize,
}

/// Builder result. `Skipped` means the output folder was cleared and no files
/// were emitted because the source already matched the requested target.
#[derive(Debug, Clone)]
pub enum RegionBootChainBuild {
    Built(RegionBootChainOutput),
    Skipped {
        source_region: DetectedRegion,
        target: RegionTarget,
    },
}

/// Build the AVB-valid `vendor_boot.img` + `vbmeta.img` pair for region
/// conversion.
///
/// This is the v3 equivalent of v2 `convert_region_images()`: copy
/// `vendor_boot`, patch region bytes, re-apply the original `vendor_boot` AVB
/// hash footer, then rebuild `vbmeta` with descriptors from the original
/// vbmeta plus the patched chained image.
pub fn build_region_converted_boot_chain(
    firmware_dir: &Path,
    output_dir: &Path,
    target: RegionTarget,
    patterns: &RegionPatternSet,
) -> Result<RegionBootChainBuild> {
    if output_dir.exists() {
        fs::remove_dir_all(output_dir).map_err(|e| {
            LtboxError::Patch(format!(
                "Cannot clear region output {}: {e}",
                output_dir.display()
            ))
        })?;
    }
    fs::create_dir_all(output_dir).map_err(|e| {
        LtboxError::Patch(format!(
            "Cannot create region output {}: {e}",
            output_dir.display()
        ))
    })?;

    let vendor_boot_src = firmware_dir.join("vendor_boot.img");
    let vbmeta_src = firmware_dir.join("vbmeta.img");
    if !vendor_boot_src.is_file() {
        return Err(LtboxError::FileNotFound(format!(
            "{}",
            vendor_boot_src.display()
        )));
    }
    if !vbmeta_src.is_file() {
        return Err(LtboxError::FileNotFound(format!(
            "{}",
            vbmeta_src.display()
        )));
    }

    let vendor_boot_data = fs::read(&vendor_boot_src).map_err(|e| {
        LtboxError::Patch(format!("Cannot read {}: {e}", vendor_boot_src.display()))
    })?;
    let source_region = detect_region_in_data(&vendor_boot_data, patterns);
    info!("Region source={source_region:?}, target={target:?}");
    if source_region.matches_target(target) {
        info!("Source already matches target; output folder left empty");
        return Ok(RegionBootChainBuild::Skipped {
            source_region,
            target,
        });
    }

    let vendor_boot_out = output_dir.join("vendor_boot.img");
    let replacement_count = patch_vendor_boot(
        &vendor_boot_src,
        &vendor_boot_out,
        target,
        &patterns.prc_patterns,
        &patterns.row_patterns,
    )?;
    info!("Region replacements: {replacement_count}");
    if replacement_count == 0 {
        let _ = fs::remove_file(&vendor_boot_out);
        return Err(LtboxError::Patch(format!(
            "No region patterns were replaced in {} (source={source_region:?}, target={target:?})",
            vendor_boot_src.display()
        )));
    }

    let vendor_boot_info = avb::extract_image_avb_info(&vendor_boot_src)?;
    avb::add_hash_footer(&vendor_boot_out, &vendor_boot_info, None, None)?;
    info!(
        "Repaired vendor_boot AVB footer: {}",
        vendor_boot_out.display()
    );

    let vbmeta_info = avb::extract_image_avb_info(&vbmeta_src)?;
    let key_spec = key_map::key_spec_for_pubkey(vbmeta_info.public_key_sha1.as_deref())
        .ok_or_else(|| {
            LtboxError::Avb(format!(
                "vbmeta signing key not recognized (pubkey {:?}); only bundled Lenovo testkeys are supported",
                vbmeta_info.public_key_sha1
            ))
        })?;
    let vbmeta_out = output_dir.join("vbmeta.img");
    avb::rebuild_vbmeta_with_chained_images(
        &vbmeta_out,
        &vbmeta_src,
        &[vendor_boot_out.as_path()],
        key_spec,
        Some(vbmeta_info.algorithm.as_str()),
    )?;
    info!("Rebuilt vbmeta chain: {}", vbmeta_out.display());

    Ok(RegionBootChainBuild::Built(RegionBootChainOutput {
        vendor_boot: vendor_boot_out,
        vbmeta: vbmeta_out,
        source_region,
        target,
        replacement_count,
    }))
}

/// Patch vendor_boot.img for region conversion.
/// Returns the number of pattern replacements made.
pub fn patch_vendor_boot(
    input: &Path,
    output: &Path,
    target: RegionTarget,
    prc_patterns: &[(Vec<u8>, Vec<u8>)],
    row_patterns: &[(Vec<u8>, Vec<u8>)],
) -> Result<usize> {
    let mut data = fs::read(input)
        .map_err(|e| LtboxError::Patch(format!("Cannot read {}: {e}", input.display())))?;

    let replacements: &[(Vec<u8>, Vec<u8>)] = match target {
        RegionTarget::Row => prc_patterns,
        RegionTarget::Prc => row_patterns,
    };

    let mut total_count = 0;
    for (from, to) in replacements {
        let count = count_occurrences(&data, from);
        if count > 0 {
            info!("Replacing pattern {} ({count} occurrences)", hex_str(from));
            data = replace_all(&data, from, to)?;
            total_count += count;
        }
    }

    fs::write(output, &data)
        .map_err(|e| LtboxError::Patch(format!("Cannot write {}: {e}", output.display())))?;

    Ok(total_count)
}

fn detect_region_in_data(data: &[u8], patterns: &RegionPatternSet) -> DetectedRegion {
    let prc_count: usize = patterns
        .prc_patterns
        .iter()
        .map(|(from, _)| count_occurrences(data, from))
        .sum();
    let row_count: usize = patterns
        .row_patterns
        .iter()
        .map(|(from, _)| count_occurrences(data, from))
        .sum();

    match (prc_count > 0, row_count > 0) {
        (true, false) => DetectedRegion::Prc,
        (false, true) => DetectedRegion::Row,
        (true, true) => DetectedRegion::Mixed,
        (false, false) => DetectedRegion::Unknown,
    }
}

/// Detect country code in a binary image (devinfo/persist).
/// Scans for patterns like "CNXX", "KRXX", "CNXE" etc.
pub fn detect_country_code(image_path: &Path, known_codes: &[&str]) -> Result<Option<String>> {
    let data = fs::read(image_path)
        .map_err(|e| LtboxError::Patch(format!("Cannot read {}: {e}", image_path.display())))?;

    for code in known_codes {
        let code_bytes = code.as_bytes();
        // Stock firmware isn't consistent with the EU suffix rule — accept either.
        for suffix in [b"XE", b"XX"] {
            let mut pattern = code_bytes.to_vec();
            pattern.extend_from_slice(suffix);
            if data.windows(pattern.len()).any(|w| w == pattern.as_slice()) {
                return Ok(Some(code.to_string()));
            }
        }
    }

    Ok(None)
}

/// Patch country code in a binary image.
/// Returns true if any replacement was made.
pub fn patch_country_code(
    input: &Path,
    output: &Path,
    old_code: &str,
    new_code: &str,
    eu_codes: &[&str],
) -> Result<bool> {
    let mut data = fs::read(input)
        .map_err(|e| LtboxError::Patch(format!("Cannot read {}: {e}", input.display())))?;

    // Write-suffix is EU-aware: EU new_code → `XE`, else `XX`.
    let new_suffix = if eu_codes.contains(&new_code) {
        "XE"
    } else {
        "XX"
    };
    let to = format!("{new_code}{new_suffix}");

    // Scan both `XE` and `XX` for old_code — stock Lenovo firmware mixes them
    // (e.g. `FRXX` in the wild). Widen unconditionally; false positives are free.
    let mut total_count = 0usize;
    for old_suffix in ["XE", "XX"] {
        let from = format!("{old_code}{old_suffix}");
        let n = count_occurrences(&data, from.as_bytes());
        if n == 0 {
            continue;
        }
        info!("Replacing country code {from} → {to} ({n} occurrences)");
        data = replace_all(&data, from.as_bytes(), to.as_bytes())?;
        total_count += n;
    }
    if total_count == 0 {
        fs::copy(input, output).map_err(|e| LtboxError::Patch(format!("Copy failed: {e}")))?;
        return Ok(false);
    }

    // Bail if expected suffix isn't present — better than silently shipping wrong-suffix devinfo.
    let written = count_occurrences(&data, to.as_bytes());
    if written == 0 {
        return Err(LtboxError::Patch(format!(
            "Post-patch verification failed: expected `{to}` not present in output"
        )));
    }

    fs::write(output, &data)
        .map_err(|e| LtboxError::Patch(format!("Cannot write {}: {e}", output.display())))?;

    Ok(true)
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || needle.len() > haystack.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

/// In-place pattern substitution, same-length only.
///
/// Unequal-length replacement would shift every byte after the match and
/// break AVB digests of the containing image — safer to refuse than to
/// let the caller ship a corrupt vendor_boot. Python v2 used
/// `bytes.replace` which accepts unequal lengths silently; the Rust port
/// surfaces the mismatch instead of panicking (the prior `assert_eq!`
/// took down the GUI thread on a user-edited `config.json`).
fn replace_all(data: &[u8], from: &[u8], to: &[u8]) -> Result<Vec<u8>> {
    if from.len() != to.len() {
        return Err(LtboxError::Patch(format!(
            "region pattern length mismatch: from={} to={}",
            from.len(),
            to.len()
        )));
    }
    let mut result = data.to_vec();
    let mut pos = 0;
    while pos + from.len() <= result.len() {
        if &result[pos..pos + from.len()] == from {
            result[pos..pos + from.len()].copy_from_slice(to);
            pos += from.len();
        } else {
            pos += 1;
        }
    }
    Ok(result)
}

fn hex_str(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_all_works() {
        let data = b"hello.PRC.world.PRC.end";
        let result = replace_all(data, b".PRC", b".ROW").unwrap();
        assert_eq!(&result, b"hello.ROW.world.ROW.end");
    }

    #[test]
    fn replace_all_rejects_length_mismatch() {
        let data = b"hello.PRC.end";
        let err = replace_all(data, b".PRC", b".RO").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("length mismatch"), "unexpected: {msg}");
    }

    #[test]
    fn count_occurrences_works() {
        assert_eq!(count_occurrences(b"AABBAABB", b"BB"), 2);
        assert_eq!(count_occurrences(b"AAAA", b"BB"), 0);
    }

    #[test]
    fn default_patterns_detect_region() {
        let patterns = RegionPatternSet::default();
        assert_eq!(
            detect_region_in_data(b"abc IPRC def", &patterns),
            DetectedRegion::Prc
        );
        assert_eq!(
            detect_region_in_data(b"abc IROW def", &patterns),
            DetectedRegion::Row
        );
        assert_eq!(
            detect_region_in_data(b".PRC and .ROW", &patterns),
            DetectedRegion::Mixed
        );
        assert_eq!(
            detect_region_in_data(b"no marker", &patterns),
            DetectedRegion::Unknown
        );
    }

    #[test]
    fn detect_country_in_buffer() {
        let data = b"\x00\x00CNXX\x00\x00";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.img");
        fs::write(&path, data).unwrap();

        let code = detect_country_code(&path, &["CN", "KR", "US"]).unwrap();
        assert_eq!(code, Some("CN".to_string()));
    }

    #[test]
    fn real_firmware_builds_region_boot_chain_when_available() {
        let Some(dir) = std::env::var_os("LTBOX_REAL_FIRMWARE_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let vendor_boot_src = dir.join("vendor_boot.img");
        let vbmeta_src = dir.join("vbmeta.img");
        if !vendor_boot_src.is_file() || !vbmeta_src.is_file() {
            return;
        }

        let patterns = RegionPatternSet::default();
        let data = fs::read(&vendor_boot_src).unwrap();
        if detect_region_in_data(&data, &patterns) != DetectedRegion::Row {
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let output_dir = tmp.path().join("region");
        fs::create_dir_all(&output_dir).unwrap();
        fs::write(output_dir.join("vendor_boot.patched.img"), b"stale").unwrap();

        let built =
            build_region_converted_boot_chain(&dir, &output_dir, RegionTarget::Prc, &patterns)
                .unwrap();

        let RegionBootChainBuild::Built(output) = built else {
            panic!("ROW firmware should build a PRC output pair");
        };
        assert!(output.vendor_boot.is_file());
        assert!(output.vbmeta.is_file());
        assert!(output.replacement_count > 0);
        assert!(!output_dir.join("vendor_boot.patched.img").exists());

        let source_vendor_boot = fs::read(&vendor_boot_src).unwrap();
        let output_vendor_boot = fs::read(&output.vendor_boot).unwrap();
        assert_ne!(source_vendor_boot, output_vendor_boot);

        assert_eq!(
            fs::metadata(&output.vbmeta).unwrap().len(),
            fs::metadata(&vbmeta_src).unwrap().len()
        );
        let report = avb::image_info_report(&[output.vbmeta]).unwrap();
        assert!(report.contains("vendor_boot"));
    }
}
