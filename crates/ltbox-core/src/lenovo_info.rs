//! Lenovo PTSTPD ConfigurationQuery client.
//!
//! Resolves a device serial number against the public
//! `getMachineSequenceInfo` endpoint and returns the `data` block as an
//! ordered list of `(key, value)` pairs ready for table rendering. Used
//! by the GUI's device-info popup.

use serde::Deserialize;

use crate::error::{LtboxError, Result};

const ENDPOINT: &str =
    "https://ptstpd.lenovo.com.cn/home/ConfigurationQuery/getMachineSequenceInfo";

/// Display order for known fields. Keys missing from the response are
/// skipped silently so a future schema addition does not break the
/// popup; unknown keys are not surfaced (intentional — keeps the table
/// tight and prevents accidental disclosure of any new private fields).
pub const FIELD_ORDER: &[&str] = &[
    "MachineNo",
    "MachineName",
    "MTM",
    "PackingLotNo",
    "ProductDate",
    "ScanDate",
    "SaleArea",
    "PurchaseDate",
    "Iwsor",
    "SaleOrder",
    "Brand",
    "CheckCode",
    "SN",
    "ProductModel",
    "ProductSeries",
    "ProductSmallClass",
    "ProductBigClass",
    "CataLogID",
    "ProductSeriesID",
    "ProductSmallClassID",
    "ProductBigClassID",
    "CusName",
    "ProductLineID",
    "ProductLineName",
    "MachineClass",
    "BrandCode",
    "Describe",
    "SalePath",
    "ResponsibleParty",
    "FeeMarkID",
    "FeeMarkName",
    "ExportRegionCode",
    "ExportRegionName",
    "download_url",
];

/// Slim representation of the response `data` block. `value` is `None`
/// when the upstream JSON returned `null`, so the renderer can choose
/// to leave the cell blank instead of printing the literal `"null"`.
#[derive(Debug, Clone, Default)]
pub struct MachineInfo {
    pub fields: Vec<(String, Option<String>)>,
}

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default, rename = "StatusCode")]
    status_code: Option<i64>,
    #[serde(default, rename = "Message")]
    message: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// Blocking GET against the PTSTPD endpoint. Pulls only the fields
/// listed in [`FIELD_ORDER`] in that order.
pub fn fetch_machine_info(serial: &str) -> Result<MachineInfo> {
    let trimmed = serial.trim();
    if trimmed.is_empty() {
        return Err(LtboxError::Other("empty serial".into()));
    }
    let agent = crate::downloader::build_agent();
    let url = format!("{ENDPOINT}?MachineNo={trimmed}");
    let mut resp = agent
        .get(&url)
        .call()
        .map_err(|e| LtboxError::Download(format!("Lenovo PTSTPD GET: {e}")))?;
    let env: Envelope = resp
        .body_mut()
        .read_json()
        .map_err(|e| LtboxError::Download(format!("Lenovo PTSTPD JSON: {e}")))?;
    if env.status_code != Some(200) {
        let msg = env.message.unwrap_or_else(|| "upstream error".to_string());
        return Err(LtboxError::Download(format!(
            "Lenovo PTSTPD status {:?}: {msg}",
            env.status_code
        )));
    }
    let data = env
        .data
        .as_ref()
        .and_then(|v| v.as_object())
        .ok_or_else(|| LtboxError::Download("Lenovo PTSTPD: missing data block".into()))?;
    let mut fields = Vec::with_capacity(FIELD_ORDER.len());
    for &key in FIELD_ORDER {
        let Some(val) = data.get(key) else { continue };
        let display = match val {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        };
        fields.push((key.to_string(), display));
    }
    Ok(MachineInfo { fields })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Field-order list must contain unique entries — duplicates would
    /// render the same key twice in the popup.
    #[test]
    fn field_order_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for &k in FIELD_ORDER {
            assert!(seen.insert(k), "duplicate field key: {k}");
        }
    }
}
