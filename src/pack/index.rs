//! Parser for the Microchip `index.idx` pack index.
//!
//! The real index ships `<pdsc>` elements under a root `<idx>`:
//!
//! ```xml
//! <idx xmlns:atmel="http://packs.download.atmel.com/pack-idx-atmel-extension">
//!   <pdsc url="packs.download.microchip.com"
//!         name="Microchip.SAMV71_DFP.pdsc"
//!         version="4.13.257"
//!         atmel:name="SAMV71_DFP"
//!         atmel:tool-name="Simulator">      <!-- only on tool packs -->
//!     <atmel:releases>
//!       <atmel:release version="4.13.257">  <!-- first release = latest -->
//!         <atmel:devices>
//!           <atmel:device name="ATSAMV71Q21B" .../>
//!         </atmel:devices>
//!         <atmel:components>
//!           <atmel:component Cclass="ToolFirmware" Cgroup="MPLABX"
//!                            Csub="Simulator" Cvendor="Microchip"/>
//!         </atmel:components>
//!       </atmel:release>
//!     </atmel:releases>
//!   </pdsc>
//! </idx>
//! ```
//!
//! The archive URL is synthesised as
//! `https://{pdsc@url}/{vendor}.{name}.{version}.atpack` when the `url`
//! attribute carries only the host. For robustness we also accept plain
//! `<pack>` elements so our simpler in-tree fixtures keep working.

use super::{Component, PackRef};

pub(crate) fn parse(xml: &str, base_url: &str) -> Result<Vec<PackRef>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("parse pack index XML: {e}"))?;
    let mut packs = Vec::new();

    for node in doc.descendants().filter(|n| is_pack_node(*n)) {
        if let Some(pack) = parse_pack_node(node, base_url) {
            packs.push(pack);
        }
    }

    Ok(packs)
}

/// Resolve a Device Family Pack (DFP) that owns `device`. Tool packs (TPs)
/// also list their supported devices in the index, but they advertise a
/// pdsc-level `atmel:tool-name` attribute; we skip those so a DFP is chosen
/// even when a later-in-file TP mentions the same device.
pub(crate) fn resolve_device<'a>(packs: &'a [PackRef], device: &str) -> Option<&'a PackRef> {
    packs.iter().rfind(|pack| {
        pack.tool_name.is_none() && pack.devices.iter().any(|d| d.eq_ignore_ascii_case(device))
    })
}

/// Resolve the tool firmware pack that advertises `tool_name`. Matches the
/// pdsc-level `atmel:tool-name` attribute first (present on every Microchip
/// tool pack in `index.idx`); falls back to component-level
/// `ToolFirmware/MPLABX` entries for fixtures that lack the shortcut.
/// Matching is case-insensitive on the user-provided `tool_name` to absorb
/// config drift.
pub(crate) fn resolve_tool<'a>(packs: &'a [PackRef], tool_name: &str) -> Option<&'a PackRef> {
    packs
        .iter()
        .rfind(|pack| {
            pack.tool_name
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(tool_name))
        })
        .or_else(|| {
            packs
                .iter()
                .rfind(|pack| pack_advertises_tool(pack, tool_name))
        })
}

fn pack_advertises_tool(pack: &PackRef, tool_name: &str) -> bool {
    pack.components.iter().any(|component| {
        component.c_vendor == "Microchip"
            && component.c_class == "ToolFirmware"
            && component.c_group == "MPLABX"
            && component.c_sub.eq_ignore_ascii_case(tool_name)
    })
}

fn is_pack_node(node: roxmltree::Node<'_, '_>) -> bool {
    let name = node.tag_name().name();
    name == "pdsc" || name == "pack"
}

fn parse_pack_node(node: roxmltree::Node<'_, '_>, base_url: &str) -> Option<PackRef> {
    // Real pdsc ships `name="<vendor>.<shortname>.pdsc"`. Older/synthetic
    // `<pack>` entries carry vendor/name/version as separate attributes or
    // child elements. Prefer the structured fields when they exist, then fall
    // back to splitting the composite `name` attribute.
    let composite_name = node.attribute("name").map(str::to_owned);
    let atmel_short = attribute_local(node, "name", Some(ATMEL_NS));

    let vendor = attr_or_child(node, "vendor").or_else(|| {
        composite_name
            .as_deref()
            .and_then(split_vendor)
            .map(str::to_owned)
    });
    let name = atmel_short
        .or_else(|| attr_or_child(node, "shortName"))
        .or_else(|| composite_name.as_deref().and_then(split_name))
        .or_else(|| attr_or_child(node, "name"));
    let version = attr_or_child(node, "version");

    let (vendor, name, version) = match (vendor, name, version) {
        (Some(v), Some(n), Some(ver)) => (v, n, ver),
        _ => return None,
    };

    let explicit_url = attr_or_child(node, "downloadUrl")
        .or_else(|| attr_or_child(node, "href"))
        .or_else(|| attr_or_child(node, "url").filter(|value| is_archive_url(value)));
    let url = explicit_url.unwrap_or_else(|| synth_archive_url(node, &vendor, &name, &version));
    let sha256 = attr_or_child(node, "sha256")
        .or_else(|| attr_or_child(node, "checksum"))
        .map(|value| value.strip_prefix("sha256:").unwrap_or(&value).to_owned());

    let release = latest_release(node);
    let scope = release.unwrap_or(node);
    let mut devices = collect_devices(scope);
    let mut components = collect_components(scope);
    // Back-compat for synthetic <pack> fixtures where devices/components sit
    // directly under the pack element rather than inside a release.
    if release.is_some() {
        if devices.is_empty() {
            devices = collect_devices(node);
        }
        if components.is_empty() {
            components = collect_components(node);
        }
    }

    let tool_name = attribute_local(node, "tool-name", Some(ATMEL_NS));

    Some(PackRef {
        vendor,
        name,
        version,
        url: absolutize_url(&url, base_url),
        sha256,
        devices,
        components,
        tool_name,
    })
}

const ATMEL_NS: &str = "http://packs.download.atmel.com/pack-idx-atmel-extension";

fn latest_release<'a, 'input>(
    pdsc: roxmltree::Node<'a, 'input>,
) -> Option<roxmltree::Node<'a, 'input>> {
    let releases = pdsc
        .children()
        .find(|child| child.tag_name().name() == "releases")?;
    releases
        .children()
        .find(|child| child.tag_name().name() == "release")
}

fn synth_archive_url(
    node: roxmltree::Node<'_, '_>,
    vendor: &str,
    name: &str,
    version: &str,
) -> String {
    let host = node.attribute("url");
    match host {
        Some(host) if !host.is_empty() => {
            format!(
                "https://{}/{vendor}.{name}.{version}.atpack",
                host.trim_end_matches('/')
            )
        }
        _ => format!("{vendor}.{name}.{version}.atpack"),
    }
}

fn is_archive_url(value: &str) -> bool {
    value.ends_with(".atpack") || value.ends_with(".pack") || value.ends_with(".zip")
}

fn split_vendor(composite: &str) -> Option<&str> {
    composite.split('.').next().filter(|part| !part.is_empty())
}

fn split_name(composite: &str) -> Option<String> {
    // Input: "Microchip.SAMV71_DFP.pdsc" -> "SAMV71_DFP". Drop the trailing
    // ".pdsc" (or any single trailing segment) and any leading vendor segment.
    let parts: Vec<&str> = composite.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let tail = parts[parts.len() - 1];
    let end = if tail.eq_ignore_ascii_case("pdsc") {
        parts.len() - 1
    } else {
        parts.len()
    };
    let middle = &parts[1..end];
    if middle.is_empty() {
        None
    } else {
        // Some packs have dots inside the short name (rare); rejoin them.
        Some(middle.join("."))
    }
}

fn attr_or_child(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    node.attribute(name)
        .map(str::to_owned)
        .or_else(|| child_text(node, name))
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    node.children()
        .find(|child| child.tag_name().name() == name)
        .and_then(|child| child.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn attribute_local(
    node: roxmltree::Node<'_, '_>,
    local: &str,
    namespace: Option<&str>,
) -> Option<String> {
    node.attributes().find_map(|attr| {
        if attr.name() != local {
            return None;
        }
        match namespace {
            Some(ns) if attr.namespace() == Some(ns) => Some(attr.value().to_owned()),
            None if attr.namespace().is_none() => Some(attr.value().to_owned()),
            _ => None,
        }
    })
}

fn collect_devices(scope: roxmltree::Node<'_, '_>) -> Vec<String> {
    let mut devices = Vec::new();
    for descendant in scope.descendants() {
        let local = descendant.tag_name().name();
        if local != "device" && local != "deviceRef" {
            continue;
        }
        if let Some(name) = descendant
            .attribute("name")
            .or_else(|| descendant.attribute("Dname"))
        {
            devices.push(name.to_string());
        } else if let Some(text) = descendant
            .text()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            devices.push(text.to_string());
        }
    }
    devices.sort();
    devices.dedup();
    devices
}

fn collect_components(scope: roxmltree::Node<'_, '_>) -> Vec<Component> {
    let mut components = Vec::new();
    for descendant in scope.descendants() {
        if descendant.tag_name().name() != "component" {
            continue;
        }
        let c_vendor = descendant.attribute("Cvendor");
        let c_class = descendant.attribute("Cclass");
        let c_group = descendant.attribute("Cgroup");
        let c_sub = descendant.attribute("Csub");
        if let (Some(c_vendor), Some(c_class), Some(c_group), Some(c_sub)) =
            (c_vendor, c_class, c_group, c_sub)
        {
            components.push(Component {
                c_vendor: c_vendor.to_string(),
                c_class: c_class.to_string(),
                c_group: c_group.to_string(),
                c_sub: c_sub.to_string(),
            });
        }
    }
    components
}

fn absolutize_url(url: &str, base_url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            url.trim_start_matches('/')
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = include_str!("../../tests/fixtures/sample-index.xml");
    const INDEX_WITH_COMPONENTS: &str =
        include_str!("../../tests/fixtures/sample-index-with-components.xml");
    const REAL_INDEX: &str = include_str!("../../tests/fixtures/sample-index-real.xml");

    #[test]
    fn parses_sample_index() {
        let packs = parse(INDEX, "https://packs.download.microchip.com").unwrap();
        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0].vendor, "Microchip");
        assert!(packs[0].devices.contains(&"PIC18F47Q10".to_string()));
    }

    #[test]
    fn resolves_latest_device_pack() {
        let packs = parse(INDEX, "https://packs.download.microchip.com").unwrap();
        let pack = resolve_device(&packs, "PIC18F47Q10").unwrap();
        assert_eq!(pack.version, "2.0.0");
    }

    #[test]
    fn parses_components_on_tool_packs() {
        let packs = parse(
            INDEX_WITH_COMPONENTS,
            "https://packs.download.microchip.com",
        )
        .unwrap();
        // Two SAM_TP packs, one "bogus" TP, one DFP.
        assert_eq!(packs.len(), 4);
        let tp_1_5 = packs
            .iter()
            .find(|p| p.name == "SAM_TP" && p.version == "1.5.23")
            .expect("SAM_TP 1.5.23 in fixture");
        // Both Simulator and PICkit4 components captured.
        assert_eq!(tp_1_5.components.len(), 2);
        assert!(tp_1_5
            .components
            .iter()
            .any(|c| c.c_sub == "Simulator" && c.c_vendor == "Microchip"));
    }

    #[test]
    fn resolve_tool_picks_latest_matching_pack() {
        let packs = parse(
            INDEX_WITH_COMPONENTS,
            "https://packs.download.microchip.com",
        )
        .unwrap();
        let pack = resolve_tool(&packs, "Simulator").expect("Simulator TP");
        assert_eq!(pack.vendor, "Microchip");
        assert_eq!(pack.name, "SAM_TP");
        assert_eq!(pack.version, "1.6.0");
    }

    #[test]
    fn resolve_tool_matches_csub_case_insensitively() {
        let packs = parse(
            INDEX_WITH_COMPONENTS,
            "https://packs.download.microchip.com",
        )
        .unwrap();
        let pack = resolve_tool(&packs, "simulator").expect("case-insensitive match");
        assert_eq!(pack.name, "SAM_TP");
    }

    #[test]
    fn resolve_tool_skips_non_microchip_vendors() {
        let packs = parse(
            INDEX_WITH_COMPONENTS,
            "https://packs.download.microchip.com",
        )
        .unwrap();
        let pack = resolve_tool(&packs, "Simulator").unwrap();
        assert_eq!(pack.vendor, "Microchip");
    }

    #[test]
    fn resolve_tool_returns_none_for_unknown_tool() {
        let packs = parse(
            INDEX_WITH_COMPONENTS,
            "https://packs.download.microchip.com",
        )
        .unwrap();
        assert!(resolve_tool(&packs, "NoSuchTool").is_none());
    }

    #[test]
    fn resolves_dfp_by_device_against_component_fixture() {
        let packs = parse(
            INDEX_WITH_COMPONENTS,
            "https://packs.download.microchip.com",
        )
        .unwrap();
        let pack = resolve_device(&packs, "ATSAMV71Q21B").unwrap();
        assert_eq!(pack.name, "SAMV71_DFP");
    }

    // --- Real Microchip index.idx shape ------------------------------------

    #[test]
    fn parses_real_pdsc_shape() {
        let packs = parse(REAL_INDEX, "https://packs.download.microchip.com/").unwrap();
        // Three pdsc entries: a DFP, a mEDBG TP (that also lists the device),
        // and a Simulator TP.
        assert_eq!(packs.len(), 3);

        let dfp = packs.iter().find(|p| p.name == "SAMV71_DFP").unwrap();
        assert_eq!(dfp.vendor, "Microchip");
        assert_eq!(dfp.version, "4.13.257");
        assert_eq!(
            dfp.url,
            "https://packs.download.microchip.com/Microchip.SAMV71_DFP.4.13.257.atpack"
        );
        assert!(dfp.devices.contains(&"ATSAMV71Q21B".to_string()));
        assert!(dfp.tool_name.is_none());

        let tp = packs.iter().find(|p| p.name == "Simulator_TP").unwrap();
        assert_eq!(tp.version, "1.8.730");
        assert_eq!(tp.tool_name.as_deref(), Some("Simulator"));
        assert!(tp
            .components
            .iter()
            .any(|c| c.c_sub == "Simulator" && c.c_class == "ToolFirmware"));
    }

    #[test]
    fn resolve_real_device_against_pdsc_fixture() {
        let packs = parse(REAL_INDEX, "https://packs.download.microchip.com/").unwrap();
        let pack = resolve_device(&packs, "ATSAMV71Q21B").unwrap();
        assert_eq!(pack.name, "SAMV71_DFP");
        assert_eq!(pack.version, "4.13.257");
    }

    #[test]
    fn resolve_device_skips_tool_packs_that_list_the_device() {
        // Regression: a TP located AFTER the DFP that lists ATSAMV71Q21B in
        // its supported-devices section must not be picked as the DFP.
        let packs = parse(REAL_INDEX, "https://packs.download.microchip.com/").unwrap();
        let pack = resolve_device(&packs, "ATSAMV71Q21B").unwrap();
        assert_eq!(pack.name, "SAMV71_DFP");
        assert!(pack.tool_name.is_none());
    }

    #[test]
    fn resolve_real_tool_against_pdsc_fixture() {
        let packs = parse(REAL_INDEX, "https://packs.download.microchip.com/").unwrap();
        let pack = resolve_tool(&packs, "Simulator").unwrap();
        assert_eq!(pack.name, "Simulator_TP");
        assert_eq!(pack.version, "1.8.730");
    }

    #[test]
    fn resolve_tool_prefers_pdsc_tool_name_attribute() {
        // The mEDBG TP has `atmel:tool-name="mEDBG"`. Matching must be
        // case-insensitive on the user-provided name.
        let packs = parse(REAL_INDEX, "https://packs.download.microchip.com/").unwrap();
        let pack = resolve_tool(&packs, "medbg").unwrap();
        assert_eq!(pack.name, "mEDBG_TP");
    }

    #[test]
    fn picks_latest_release_components_not_historical() {
        // The real Simulator_TP has multiple <atmel:release> entries; only the
        // first (latest) should contribute components.
        let packs = parse(REAL_INDEX, "https://packs.download.microchip.com/").unwrap();
        let tp = packs.iter().find(|p| p.name == "Simulator_TP").unwrap();
        assert_eq!(tp.components.len(), 1);
        assert_eq!(tp.components[0].c_sub, "Simulator");
    }

    #[test]
    fn splits_composite_name_attribute() {
        assert_eq!(split_vendor("Microchip.SAMV71_DFP.pdsc"), Some("Microchip"));
        assert_eq!(
            split_name("Microchip.SAMV71_DFP.pdsc").as_deref(),
            Some("SAMV71_DFP")
        );
        assert_eq!(split_name("Vendor.Short").as_deref(), Some("Short"));
        assert_eq!(split_name("OnlyOne"), None);
    }
}
