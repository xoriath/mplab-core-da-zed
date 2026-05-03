//! On-disk pack repository layout helpers.

use crate::paths;

use super::PackRef;

pub(crate) fn installed_dir(pack: &PackRef) -> String {
    format!(
        "{}/{}/{}/{}",
        paths::PACKS_ROOT,
        sanitize(&pack.vendor),
        sanitize(&pack.name),
        sanitize(&pack.version)
    )
}

pub(crate) fn marker_path(pack: &PackRef) -> String {
    format!("{}/.installed", installed_dir(pack))
}

pub(crate) fn archive_cache_path(pack: &PackRef) -> String {
    format!(
        "{}/pack-archives/{}.{}.{}.pack",
        paths::CACHE_ROOT,
        sanitize(&pack.vendor),
        sanitize(&pack.name),
        sanitize(&pack.version)
    )
}

pub(crate) fn is_installed(pack: &PackRef) -> bool {
    let marker = marker_path(pack);
    std::fs::read_to_string(marker)
        .map(|contents| contents.lines().next() == Some(pack.version.as_str()))
        .unwrap_or(false)
}

pub(crate) fn write_marker(pack: &PackRef) -> Result<(), String> {
    let marker = marker_path(pack);
    if let Some(parent) = std::path::Path::new(&marker).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create pack marker dir {}: {e}", parent.display()))?;
    }
    std::fs::write(marker, format!("{}\n{}\n", pack.version, pack.url))
        .map_err(|e| format!("write pack marker for {}: {e}", pack.name))
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_pack_layout() {
        let pack = PackRef {
            vendor: "Microchip".into(),
            name: "PIC18F-Q_DFP".into(),
            version: "1.2.3".into(),
            url: "https://packs.example/PIC18F-Q_DFP.pack".into(),
            sha256: None,
            devices: vec!["PIC18F47Q10".into()],
            components: Vec::new(),
            tool_name: None,
        };
        assert_eq!(installed_dir(&pack), "packs/Microchip/PIC18F-Q_DFP/1.2.3");
        assert_eq!(
            archive_cache_path(&pack),
            "cache/pack-archives/Microchip.PIC18F-Q_DFP.1.2.3.pack"
        );
    }
}
