//! `qsahara_device_programmer.xml` parser.
//!
//! Lenovo's TB323FU firmware (kaanapali chipset) ships a multi-image
//! Sahara handoff: instead of a single `xbl_s_devprg_ns.melf`, the
//! programmer is split across 8 ELF / MBN payloads keyed by Sahara
//! image-id. The shipped XML enumerates them so the host knows which
//! file maps to which id.
//!
//! Example:
//!
//! ```xml
//! <sahara_config>
//!   <chipset>kaanapali</chipset>
//!   <images>
//!     <image image_id="13" image_path="prog_firehose_ddr.elf"/>
//!     <image image_id="21" image_path="xbl_sc.elf"/>
//!     ...
//!   </images>
//! </sahara_config>
//! ```
//!
//! [`parse`] returns the entries in document order. [`load_image_slots`]
//! reads each file relative to the XML's directory and returns a
//! `Vec<Option<Vec<u8>>>` sized to `max_id + 1`, with `Some(bytes)` at
//! every populated id and `None` everywhere else — the exact shape
//! `qdl::sahara::sahara_run` expects.

use std::path::{Path, PathBuf};

use crate::error::{LtboxError, Result};

/// Filename Lenovo uses for the multi-image manifest.
pub const MANIFEST_FILENAME: &str = "qsahara_device_programmer.xml";

/// One `<image>` entry from the manifest.
#[derive(Debug, Clone)]
pub struct SaharaImage {
    /// Sahara protocol image-id the device asks for during cmd-mode.
    pub image_id: u32,
    /// Filename relative to the XML's parent directory.
    pub image_path: String,
}

/// Whether the file at `path` looks like a `qsahara_device_programmer.xml`
/// manifest (the only XML the EDL loader picker accepts).
pub fn is_manifest_filename(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case(MANIFEST_FILENAME))
        .unwrap_or(false)
}

/// Parse a `qsahara_device_programmer.xml` body into ordered
/// `SaharaImage` entries.
pub fn parse(xml: &str) -> Result<Vec<SaharaImage>> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| LtboxError::Other(format!("Sahara XML parse: {e}")))?;
    let images_node = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name("images"))
        .ok_or_else(|| LtboxError::Other("Sahara XML: missing <images>".to_string()))?;
    let mut out = Vec::new();
    for child in images_node.children().filter(|n| n.has_tag_name("image")) {
        let image_id = child
            .attribute("image_id")
            .ok_or_else(|| LtboxError::Other("Sahara XML: <image> missing image_id".to_string()))?
            .parse::<u32>()
            .map_err(|e| LtboxError::Other(format!("Sahara XML image_id parse: {e}")))?;
        let image_path = child
            .attribute("image_path")
            .ok_or_else(|| LtboxError::Other("Sahara XML: <image> missing image_path".to_string()))?
            .to_string();
        out.push(SaharaImage {
            image_id,
            image_path,
        });
    }
    if out.is_empty() {
        return Err(LtboxError::Other(
            "Sahara XML: <images> contained no <image> entries".to_string(),
        ));
    }
    Ok(out)
}

/// Slot array sized to `(max image-id + 1)` with `Some(bytes)` at every
/// populated id and `None` everywhere else — exact shape
/// `qdl::sahara::sahara_run` expects.
pub type ImageSlots = Vec<Option<Vec<u8>>>;

/// Read the manifest at `xml_path`, then read each referenced image
/// from the manifest's parent directory and return the slot array.
///
/// Returns `(slots, image_paths)` so callers can log both sides
/// (which IDs were populated + which files were read).
pub fn load_image_slots(xml_path: &Path) -> Result<(ImageSlots, Vec<PathBuf>)> {
    let xml_body = std::fs::read_to_string(xml_path)
        .map_err(|e| LtboxError::Other(format!("Sahara XML read: {e}")))?;
    let entries = parse(&xml_body)?;
    let parent = xml_path
        .parent()
        .ok_or_else(|| LtboxError::Other("Sahara XML has no parent directory".to_string()))?;
    // `image_id` is a u32 from untrusted manifest XML. Without a cap, a
    // hostile / corrupted entry could pre-allocate up to (u32::MAX + 1) ×
    // size_of::<Option<Vec<u8>>>() bytes (~32 GiB on 64-bit) before we
    // ever look at the referenced files — easy OOM / DoS vector. The
    // Sahara protocol's image IDs sit in a small enumerated range (the
    // qdl-rs upstream defines ~30 well-known IDs; production Qualcomm
    // SoCs use double-digit values), so clamp at 256 which leaves
    // headroom for vendor extensions without exposing the OOM.
    const MAX_SAHARA_IMAGE_ID: u32 = 256;
    let raw_max_id = entries.iter().map(|e| e.image_id).max().unwrap_or(0);
    if raw_max_id > MAX_SAHARA_IMAGE_ID {
        return Err(LtboxError::Other(format!(
            "Sahara manifest image_id {raw_max_id} exceeds supported cap \
             {MAX_SAHARA_IMAGE_ID} ({}); manifest may be malformed",
            xml_path.display(),
        )));
    }
    let max_id = raw_max_id as usize;
    let mut slots: ImageSlots = vec![None; max_id + 1];
    let mut paths: Vec<PathBuf> = Vec::with_capacity(entries.len());
    for entry in &entries {
        let img_path = crate::safe_path::safe_join(parent, &entry.image_path)?;
        let bytes = std::fs::read(&img_path).map_err(|e| {
            LtboxError::Other(format!(
                "Sahara image read failed for id={} ({}): {e}",
                entry.image_id,
                img_path.display()
            ))
        })?;
        slots[entry.image_id as usize] = Some(bytes);
        paths.push(img_path);
    }
    Ok((slots, paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lenovo_kaanapali_manifest() {
        let xml = r#"<?xml version="1.0" ?>
<sahara_config>
    <chipset>kaanapali</chipset>
    <images>
        <image image_id="36" image_path="multi_image_qti.mbn"/>
        <image image_id="37" image_path="multi_image.mbn"/>
        <image image_id="21" image_path="xbl_sc.elf"/>
        <image image_id="13" image_path="prog_firehose_ddr.elf"/>
    </images>
</sahara_config>"#;
        let imgs = parse(xml).unwrap();
        assert_eq!(imgs.len(), 4);
        assert_eq!(imgs[0].image_id, 36);
        assert_eq!(imgs[0].image_path, "multi_image_qti.mbn");
        assert_eq!(imgs[3].image_id, 13);
    }

    #[test]
    fn parse_rejects_empty_images() {
        let xml = r#"<sahara_config><chipset>x</chipset><images/></sahara_config>"#;
        assert!(parse(xml).is_err());
    }

    #[test]
    fn manifest_filename_matches_case_insensitive() {
        assert!(is_manifest_filename(Path::new(
            "qsahara_device_programmer.xml"
        )));
        assert!(is_manifest_filename(Path::new(
            "QSAHARA_DEVICE_PROGRAMMER.XML"
        )));
        assert!(!is_manifest_filename(Path::new("foo.xml")));
        assert!(!is_manifest_filename(Path::new("xbl_s_devprg_ns.melf")));
    }
}
