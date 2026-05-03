//! Minimal PDSC parser used to discover devices in a downloaded pack.
//!
//! Not yet wired into the DAP flow — the index-level device list from
//! `pack::index` is what we use today. Retained for future post-extract
//! validation of downloaded packs.

#[allow(dead_code)]
pub(crate) fn devices(xml: &str) -> Result<Vec<String>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("parse PDSC XML: {e}"))?;
    let mut devices = Vec::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("device")) {
        if let Some(name) = node.attribute("Dname") {
            devices.push(name.to_string());
        }
    }
    devices.sort();
    devices.dedup();
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PDSC: &str = include_str!("../../tests/fixtures/sample.pdsc");

    #[test]
    fn parses_devices() {
        let devices = devices(PDSC).unwrap();
        assert_eq!(devices, vec!["PIC18F47Q10", "PIC18F57Q10"]);
    }
}
