//! Minimum Rust port of MPLAB pack management.
//!
//! Scope is intentionally small: parse an index, resolve `device -> pack`,
//! download+verify+extract a `.pack`, and record an installed marker. The full
//! TypeScript PackManager features (components, examples, meta.pm, tool
//! mediator) remain out of scope for the MVP.

mod cache;
mod download;
pub(crate) mod index;
pub(crate) mod pdsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackRef {
    pub vendor: String,
    pub name: String,
    pub version: String,
    pub url: String,
    pub sha256: Option<String>,
    pub devices: Vec<String>,
    pub components: Vec<Component>,
    /// Value of the pdsc-level `atmel:tool-name` attribute when present.
    /// Tool packs (TPs) in Microchip's `index.idx` carry this attribute and
    /// ALSO list every device they support, so we rely on it to distinguish
    /// "owning" DFPs from TPs during device resolution.
    pub tool_name: Option<String>,
}

/// CMSIS-style component descriptor used by Microchip's tool-firmware packs.
/// All four `C*` attributes are present on `ToolFirmware` entries in
/// `index.idx`; we ignore everything else (e.g. `Cversion`, `condition`,
/// sub-elements like `files`) because the resolver only needs identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Component {
    pub c_vendor: String,
    pub c_class: String,
    pub c_group: String,
    pub c_sub: String,
}

/// Optional selector used when more than one pack in the index claims the
/// same device. Not wired into the current DAP flow (device-only rfind is
/// sufficient) but kept for future user-pinning support.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreferredPack {
    pub vendor: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
}

#[allow(dead_code)]
pub(crate) fn resolve_pack<'a>(
    packs: &'a [PackRef],
    device: &str,
    preferred: Option<&PreferredPack>,
) -> Option<&'a PackRef> {
    packs.iter().rfind(|pack| {
        pack.devices.iter().any(|d| d.eq_ignore_ascii_case(device))
            && preferred.is_none_or(|preferred| preferred.matches(pack))
    })
}

pub(crate) fn ensure_pack_installed(pack: &PackRef) -> Result<String, String> {
    download::ensure_installed(pack)
}

impl PreferredPack {
    #[allow(dead_code)]
    fn matches(&self, pack: &PackRef) -> bool {
        self.vendor
            .as_deref()
            .is_none_or(|vendor| vendor.eq_ignore_ascii_case(&pack.vendor))
            && self
                .name
                .as_deref()
                .is_none_or(|name| name.eq_ignore_ascii_case(&pack.name))
            && self
                .version
                .as_deref()
                .is_none_or(|version| version == pack.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_preferred_pack() {
        let packs = vec![
            PackRef {
                vendor: "Microchip".into(),
                name: "PIC18F-Q_DFP".into(),
                version: "1.0.0".into(),
                url: "https://example.test/1.pack".into(),
                sha256: None,
                devices: vec!["PIC18F47Q10".into()],
                components: Vec::new(),
                tool_name: None,
            },
            PackRef {
                vendor: "Microchip".into(),
                name: "PIC18F-Q_DFP".into(),
                version: "2.0.0".into(),
                url: "https://example.test/2.pack".into(),
                sha256: None,
                devices: vec!["PIC18F47Q10".into()],
                components: Vec::new(),
                tool_name: None,
            },
        ];
        let preferred = PreferredPack {
            vendor: None,
            name: None,
            version: Some("1.0.0".into()),
        };
        assert_eq!(
            resolve_pack(&packs, "PIC18F47Q10", None).unwrap().version,
            "2.0.0"
        );
        assert_eq!(
            resolve_pack(&packs, "PIC18F47Q10", Some(&preferred))
                .unwrap()
                .version,
            "1.0.0"
        );
    }
}
