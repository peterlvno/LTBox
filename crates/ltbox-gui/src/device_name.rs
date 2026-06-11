//! Device-name / build-string helpers: normalize ro.* props and TWRP
//! product strings into a friendly device label. Extracted from main.rs.

/// Trim Lenovo build-display to the ROM + version tail. Example:
/// `TB322FC_..._ZUXOS_1.5.10.183_ST_...` → `ZUXOS_1.5.10.183_ST_...`.
/// ROW firmware uses `_ZUI_`. No marker → passthrough.
pub(crate) fn trim_build_display(s: &str) -> String {
    if let Some(i) = s.find("_ZUXOS_") {
        return s[i + 1..].to_string();
    }
    if let Some(i) = s.find("_ZUI_") {
        return s[i + 1..].to_string();
    }
    s.to_string()
}

/// True if the ADB product name is a TWRP recovery build. Lenovo stock
/// never uses this prefix, so it's reliable without `ro.bootmode`.
pub(crate) fn is_twrp_product(product: &str) -> bool {
    product.to_ascii_lowercase().starts_with("twrp_")
}

/// Strip a leading `twrp_` (any case) from a product name.
pub(crate) fn strip_twrp_prefix(product: &str) -> String {
    if is_twrp_product(product) {
        product[5..].to_string()
    } else {
        product.to_string()
    }
}

/// Normalize a raw `getprop` value for display: trim surrounding whitespace
/// and treat an empty or whitespace-only result (what `getprop` prints for an
/// absent property) as `None`.
pub(crate) fn non_empty_prop(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Pick the dashboard device name from the LGSI market-name properties in
/// priority order, falling back to the legacy `kirby_en` property. `getprop`
/// is invoked lazily, so the probe stops at the first populated property.
///
/// `getprop(name)` returns the raw `getprop <name>` output (empty/whitespace
/// when the property is absent).
pub(crate) fn select_device_name<F: FnMut(&str) -> String>(mut getprop: F) -> String {
    [
        "ro.vendor.config.lgsi.en.market_name",
        "ro.vendor.config.lgsi.market_name",
        "ro.config.lgsi.market_name",
        "ro.vendor.config.lgsi.kirby_en",
    ]
    .into_iter()
    .find_map(|prop| non_empty_prop(&getprop(prop)))
    .unwrap_or_default()
}
