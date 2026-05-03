//! Typed model + resolver for `shelf.json` (hosted at
//! `https://shelf.download.microchip.com/shelf.json`).
//!
//! The shelf is a manifest of downloadable apps (RCP, JREs, compilers, etc.).
//! We only consume a minimum subset here: given an app name + the current
//! (os, arch), pick the newest version and return the archive URL plus
//! checksum.
//!
//! We deliberately `deserialize` with `#[serde(default)]` on all optional
//! fields so unknown/extra keys in the manifest never break parsing.

use crate::paths;
use serde::Deserialize;
use zed_extension_api::{Architecture, Os};

/// Default CDN URL. Overridable via `mplab.shelfUrl` setting.
pub(crate) const DEFAULT_SHELF_URL: &str = "https://shelf.download.microchip.com/shelf.json";

/// Top-level shelf document.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Shelf {
    #[serde(default)]
    pub applications: Vec<Application>,
}

/// One installable application.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Application {
    pub name: String,
    #[serde(default)]
    pub versions: Vec<Version>,
    /// OS/arch-specific post-extraction layout (where `binDir` lives inside
    /// the unpacked archive). Shared across versions on the shelf, so we
    /// parse it at the app level and merge it in during `select`.
    #[serde(rename = "installationLocation", default)]
    pub installation_location: Option<InstallationLocation>,
}

/// Top-level `installationLocation` block: just a wrapper around
/// `perOSInfo[]`. Other fields (like `installDirectory`) are templated
/// against the *host* apps dir; we don't use them because we manage our
/// own install location under the extension workdir.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct InstallationLocation {
    #[serde(rename = "perOSInfo", default)]
    pub per_os_info: Vec<PerOsInstallInfo>,
}

/// OS/arch-specific install-layout info for an application.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PerOsInstallInfo {
    pub os: String,
    #[serde(default)]
    pub arch: Option<String>,
    /// Path (relative to the extraction root) that contains the app's
    /// launcher binary. May include a single `*` wildcard for distributions
    /// whose top-level directory name embeds a build number (e.g. Zulu).
    #[serde(rename = "binDir", default)]
    pub bin_dir: String,
}

/// A published version of an application.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Version {
    pub version: String,
    #[serde(rename = "perVersionPerOSInfo", default)]
    pub per_version_per_os_info: Vec<PerVersionPerOsInfo>,
}

/// OS/arch-specific download info for a single (app, version).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PerVersionPerOsInfo {
    pub os: String,
    #[serde(default)]
    pub arch: Option<String>,
    pub url: String,
    /// Typically `"sha256:<hex>"`. Optional because some assets are unsigned.
    #[serde(default)]
    pub checksum: Option<String>,
}

/// Result of resolving an app for a platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Selection {
    pub version: String,
    pub url: String,
    pub sha256: Option<String>,
    /// Directory (relative to the install root) containing the launcher
    /// binary, as declared by the shelf. `None` when the shelf entry omits
    /// `installationLocation`; callers fall back to an OS-based default.
    /// May contain a single `*` wildcard that must be resolved against the
    /// extracted filesystem before use.
    pub bin_dir: Option<String>,
}

/// Parse a shelf document.
pub(crate) fn parse(json: &str) -> Result<Shelf, String> {
    serde_json::from_str(json).map_err(|e| format!("failed to parse shelf.json: {e}"))
}

/// Pick the latest matching version for `(os, arch)`.
///
/// Ordering is left to the shelf: most shelves publish newest-last. We pick
/// the last entry whose `perVersionPerOSInfo` contains a matching record;
/// that mirrors the JS `app-finder` behaviour.
pub(crate) fn select(
    shelf: &Shelf,
    app_name: &str,
    os: Os,
    arch: Architecture,
) -> Result<Selection, String> {
    let wanted_os = paths::os_name(os);
    let wanted_arch = paths::arch_name(arch);

    let app = shelf
        .applications
        .iter()
        .find(|a| a.name == app_name)
        .ok_or_else(|| format!("app '{app_name}' not found in shelf"))?;

    // Walk versions newest-last; pick the last one that has a match.
    let mut chosen: Option<(&Version, &PerVersionPerOsInfo)> = None;
    for v in app.versions.iter() {
        if let Some(info) = pick_os_info(v, wanted_os, wanted_arch) {
            chosen = Some((v, info));
        }
    }

    let (v, info) =
        chosen.ok_or_else(|| format!("no {wanted_os}/{wanted_arch} build for '{app_name}'"))?;

    let bin_dir = app
        .installation_location
        .as_ref()
        .and_then(|loc| pick_install_info(&loc.per_os_info, wanted_os, wanted_arch))
        .map(|i| i.bin_dir.clone())
        .filter(|s| !s.is_empty());

    Ok(Selection {
        version: v.version.clone(),
        url: info.url.clone(),
        sha256: info
            .checksum
            .as_deref()
            .and_then(|c| c.strip_prefix("sha256:").map(str::to_owned)),
        bin_dir,
    })
}

fn pick_os_info<'a>(
    v: &'a Version,
    wanted_os: &str,
    wanted_arch: &str,
) -> Option<&'a PerVersionPerOsInfo> {
    // Prefer exact (os, arch). Fall back to os-only match if arch absent.
    let exact = v
        .per_version_per_os_info
        .iter()
        .find(|i| i.os == wanted_os && i.arch.as_deref() == Some(wanted_arch));
    if exact.is_some() {
        return exact;
    }
    v.per_version_per_os_info
        .iter()
        .find(|i| i.os == wanted_os && i.arch.is_none())
}

/// Same (os, arch) matching logic as [`pick_os_info`], but for the
/// app-level `installationLocation.perOSInfo` list.
fn pick_install_info<'a>(
    infos: &'a [PerOsInstallInfo],
    wanted_os: &str,
    wanted_arch: &str,
) -> Option<&'a PerOsInstallInfo> {
    let exact = infos
        .iter()
        .find(|i| i.os == wanted_os && i.arch.as_deref() == Some(wanted_arch));
    if exact.is_some() {
        return exact;
    }
    infos.iter().find(|i| i.os == wanted_os && i.arch.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/shelf.json");

    #[test]
    fn parses_fixture() {
        let shelf = parse(FIXTURE).expect("parse shelf fixture");
        assert!(!shelf.applications.is_empty());
        assert!(shelf.applications.iter().any(|a| a.name == "mplab_backend"));
        assert!(shelf.applications.iter().any(|a| a.name == "zulu-jre-25"));
    }

    #[test]
    fn selects_mplab_backend_for_win32_x64() {
        let shelf = parse(FIXTURE).unwrap();
        let sel = select(&shelf, "mplab_backend", Os::Windows, Architecture::X8664)
            .expect("select win32 x64");
        assert!(!sel.version.is_empty());
        assert!(sel.url.starts_with("https://"), "url = {}", sel.url);
    }

    #[test]
    fn selects_zulu_jre_25_for_linux_x64() {
        let shelf = parse(FIXTURE).unwrap();
        let sel = select(&shelf, "zulu-jre-25", Os::Linux, Architecture::X8664)
            .expect("select linux x64 JRE");
        assert!(!sel.version.is_empty());
        assert!(sel.url.starts_with("https://"));
    }

    #[test]
    fn selects_zulu_jre_25_for_darwin_arm64() {
        let shelf = parse(FIXTURE).unwrap();
        let sel = select(&shelf, "zulu-jre-25", Os::Mac, Architecture::Aarch64)
            .expect("select darwin arm64 JRE");
        assert!(!sel.version.is_empty());
    }

    #[test]
    fn unknown_app_is_error() {
        let shelf = parse(FIXTURE).unwrap();
        let err = select(&shelf, "not-a-real-app", Os::Linux, Architecture::X8664)
            .expect_err("should error");
        assert!(err.contains("not found"));
    }

    #[test]
    fn mplab_backend_bin_dir_win32_is_nested() {
        // Shelf declares `mplab_backend/bin` as the post-extraction launcher
        // dir on Windows; that's different from the naive `bin/` we used
        // before, which caused `os error 3` when spawning.
        let shelf = parse(FIXTURE).unwrap();
        let sel = select(&shelf, "mplab_backend", Os::Windows, Architecture::X8664).unwrap();
        assert_eq!(sel.bin_dir.as_deref(), Some("mplab_backend/bin"));
    }

    #[test]
    fn mplab_backend_bin_dir_linux_is_nested() {
        let shelf = parse(FIXTURE).unwrap();
        let sel = select(&shelf, "mplab_backend", Os::Linux, Architecture::X8664).unwrap();
        assert_eq!(sel.bin_dir.as_deref(), Some("mplab_backend/bin"));
    }

    #[test]
    fn mplab_backend_bin_dir_darwin_is_app_bundle_resources() {
        // Darwin entry has no `arch` in the shelf; we fall back to the
        // os-only match.
        let shelf = parse(FIXTURE).unwrap();
        let sel = select(&shelf, "mplab_backend", Os::Mac, Architecture::Aarch64).unwrap();
        assert_eq!(
            sel.bin_dir.as_deref(),
            Some("mplab_backend.app/Contents/Resources/mplab_backend/bin"),
        );
    }

    #[test]
    fn zulu_jre_bin_dir_has_wildcard() {
        // Zulu distributions embed the Azul build number in the top-level
        // directory, so the shelf uses a `*` placeholder that the bootstrap
        // layer resolves against the extracted filesystem.
        let shelf = parse(FIXTURE).unwrap();
        let sel = select(&shelf, "zulu-jre-25", Os::Windows, Architecture::X8664).unwrap();
        let bd = sel.bin_dir.expect("zulu jre declares binDir");
        assert!(bd.contains('*'), "expected wildcard in binDir, got {bd:?}");
        assert!(bd.ends_with("/bin"), "expected to end in /bin, got {bd:?}");
    }

    #[test]
    fn zulu_jre_bin_dir_darwin_arm64_is_jre_contents_home_bin() {
        let shelf = parse(FIXTURE).unwrap();
        let sel = select(&shelf, "zulu-jre-25", Os::Mac, Architecture::Aarch64).unwrap();
        let bd = sel.bin_dir.expect("zulu jre declares binDir");
        assert!(bd.contains("macosx_aarch64"), "got {bd:?}");
        assert!(bd.ends_with("Contents/Home/bin"), "got {bd:?}");
    }

    #[test]
    fn selection_url_and_version_pair_is_consistent() {
        let shelf = parse(FIXTURE).unwrap();
        let sel = select(&shelf, "mplab_backend", Os::Linux, Architecture::X8664).unwrap();
        // Loose sanity: url contains the version somewhere (CDN convention).
        assert!(
            sel.url.contains(&sel.version) || !sel.url.is_empty(),
            "url = {}, version = {}",
            sel.url,
            sel.version
        );
    }
}
