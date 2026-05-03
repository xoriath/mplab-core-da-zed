//! Bootstrap: ensure the RCP backend and a JRE are installed locally.
//!
//! Design notes:
//! * `download_file` extracts archives on the fly when a matching
//!   `DownloadedFileType` is used, but does **not** expose headers or the raw
//!   bytes. That means we cannot verify the shelf-declared SHA-256 against
//!   the downloaded payload in Phase 1. We still carry the hash through so
//!   Phase 3 (`http_cache`) can re-fetch-and-verify using
//!   `http_client::fetch` + a pure-Rust unzipper.
//! * The shelf may advertise either `.zip` or `.tar.gz`/`.tgz` assets
//!   depending on the platform (e.g. JREs on linux/mac are tarballs). We
//!   inspect the URL's file extension to pick the right
//!   `DownloadedFileType` so the host unpacks the archive correctly.
//! * The shelf's `installationLocation.perOSInfo[].binDir` is authoritative
//!   for locating the launcher binary inside the extracted archive. Both
//!   the RCP (nested under `mplab_backend/` or `mplab_backend.app/...`) and
//!   the JRE (nested under `zulu25.*-...`/`zulu-25.jre/...`) ship with a
//!   wrapper directory, and the wrapper name isn't always predictable from
//!   the version alone. We trust `binDir` verbatim and, when it contains a
//!   `*`, resolve the wildcard against the actually-extracted filesystem.
//! * A marker file `<install_dir>/.installed` is written on success with
//!   `version\n[sha256]\n`. If the marker is present and readable we
//!   short-circuit.
//! * On unix we `chmod +x` the launcher so it can be spawned.

use crate::paths::{self, JrePaths, RcpPaths};
use crate::shelf::{self, Shelf};
use zed_extension_api as zed;
use zed_extension_api::{Architecture, DownloadedFileType, Os};

/// Ensure the MPLAB backend RCP is installed for the given platform.
///
/// Returns fully-resolved paths to the install dir and launcher executable.
pub(crate) fn ensure_mplab_backend(
    shelf: &Shelf,
    os: Os,
    arch: Architecture,
) -> Result<RcpPaths, String> {
    let sel = shelf::select(shelf, "mplab_backend", os, arch)?;
    // First compute paths with the literal (possibly-wildcarded) bin_dir so
    // we know where to extract and whether the marker matches.
    let initial = paths::rcp_paths(&sel.version, os, sel.bin_dir.as_deref());

    if marker_matches(&initial.marker, &sel.version) {
        // The marker is keyed by version, not by bin_dir, so we still need
        // to resolve any wildcard in the shelf's bin_dir so callers can
        // spawn the (already-extracted) executable.
        return resolve_rcp_paths(&sel.version, os, &sel.bin_dir, &initial);
    }

    std::fs::create_dir_all(&initial.install_dir)
        .map_err(|e| format!("create install dir {}: {e}", initial.install_dir))?;
    let file_type = detect_archive_type(&sel.url).ok_or_else(|| {
        format!(
            "download mplab_backend {}: unsupported archive extension in url '{}'",
            sel.version, sel.url
        )
    })?;
    zed::download_file(&sel.url, &initial.install_dir, file_type)
        .map_err(|e| format!("download mplab_backend {} failed: {e}", sel.version))?;

    // mplab_backend's bin_dir currently never contains a wildcard, but run
    // the resolver unconditionally so this remains correct if the shelf
    // ever grows one.
    let resolved = resolve_rcp_paths(&sel.version, os, &sel.bin_dir, &initial)?;

    if !matches!(os, Os::Windows) {
        let _ = zed::make_file_executable(&resolved.executable);
    }

    write_marker(&resolved.marker, &sel.version, sel.sha256.as_deref())?;
    Ok(resolved)
}

/// Ensure a Zulu JRE (default `zulu-jre-25`) is installed.
pub(crate) fn ensure_zulu_jre(
    shelf: &Shelf,
    jre_name: &str,
    os: Os,
    arch: Architecture,
) -> Result<JrePaths, String> {
    let sel = shelf::select(shelf, jre_name, os, arch)?;
    let initial = paths::jre_paths(jre_name, &sel.version, os, sel.bin_dir.as_deref());

    if marker_matches(&initial.marker, &sel.version) {
        return resolve_jre_paths(jre_name, &sel.version, os, &sel.bin_dir, &initial);
    }

    std::fs::create_dir_all(&initial.install_dir)
        .map_err(|e| format!("create install dir {}: {e}", initial.install_dir))?;
    let file_type = detect_archive_type(&sel.url).ok_or_else(|| {
        format!(
            "download {jre_name} {}: unsupported archive extension in url '{}'",
            sel.version, sel.url
        )
    })?;
    zed::download_file(&sel.url, &initial.install_dir, file_type)
        .map_err(|e| format!("download {jre_name} {} failed: {e}", sel.version))?;

    let resolved = resolve_jre_paths(jre_name, &sel.version, os, &sel.bin_dir, &initial)?;

    // Make `java` executable on unix (best-effort; exact layout varies by archive).
    if !matches!(os, Os::Windows) {
        let java_bin = format!("{}/bin/java", resolved.java_home);
        let _ = zed::make_file_executable(&java_bin);
    }

    write_marker(&resolved.marker, &sel.version, sel.sha256.as_deref())?;
    Ok(resolved)
}

/// If the shelf-provided `bin_dir` contains a `*` wildcard, replace it with
/// the matching extracted directory name and return freshly-computed paths.
/// Otherwise return `initial` unchanged.
fn resolve_rcp_paths(
    version: &str,
    os: Os,
    bin_dir: &Option<String>,
    initial: &RcpPaths,
) -> Result<RcpPaths, String> {
    let Some(bd) = bin_dir.as_deref() else {
        return Ok(initial.clone());
    };
    if !bd.contains('*') {
        return Ok(initial.clone());
    }
    let concrete = expand_wildcard(&initial.install_dir, bd)
        .map_err(|e| format!("resolve mplab_backend binDir '{bd}': {e}"))?;
    Ok(paths::rcp_paths(version, os, Some(&concrete)))
}

/// JRE equivalent of [`resolve_rcp_paths`]. Zulu archives embed a build
/// number in the wrapper directory (`zulu25.30.17-ca-jre25.0.1-win_x64`),
/// so this is the hot path.
fn resolve_jre_paths(
    jre_name: &str,
    version: &str,
    os: Os,
    bin_dir: &Option<String>,
    initial: &JrePaths,
) -> Result<JrePaths, String> {
    let Some(bd) = bin_dir.as_deref() else {
        return Ok(initial.clone());
    };
    if !bd.contains('*') {
        return Ok(initial.clone());
    }
    let concrete = expand_wildcard(&initial.install_dir, bd)
        .map_err(|e| format!("resolve {jre_name} binDir '{bd}': {e}"))?;
    Ok(paths::jre_paths(jre_name, version, os, Some(&concrete)))
}

/// Resolve a single `*` wildcard in a shelf `binDir` against the actually-
/// extracted filesystem.
///
/// Supports exactly one wildcard, which must live in the *first* path
/// segment (matching every shelf entry we care about: Zulu archives have
/// the wildcard in their top-level directory name). We list the immediate
/// children of `install_dir`, keep those whose name starts with the part
/// of `binDir` before the `*` and ends with the part between the `*` and
/// the next `/`, and require a unique match.
fn expand_wildcard(install_dir: &str, bin_dir: &str) -> Result<String, String> {
    let star = bin_dir
        .find('*')
        .ok_or_else(|| "no wildcard to expand".to_string())?;
    if bin_dir[star + 1..].contains('*') {
        return Err("only a single '*' wildcard is supported".into());
    }

    // Slice `binDir` into: [prefix-within-first-segment]*[suffix-within-first-segment]/[tail]
    let first_slash_after_star = bin_dir[star + 1..]
        .find('/')
        .map(|i| star + 1 + i)
        .unwrap_or(bin_dir.len());
    let prefix = &bin_dir[..star];
    let segment_suffix = &bin_dir[star + 1..first_slash_after_star];
    let tail = &bin_dir[first_slash_after_star..]; // includes leading '/' or is empty

    if prefix.contains('/') {
        return Err("wildcard must live in the first path segment of binDir".into());
    }

    let entries = std::fs::read_dir(install_dir)
        .map_err(|e| format!("read_dir {install_dir}: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("read_dir {install_dir}: {e}"))?;

    let mut matches: Vec<String> = entries
        .iter()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if name.starts_with(prefix) && name.ends_with(segment_suffix) {
                // Guard against prefix+suffix overlapping when the entry
                // name is shorter than their combined length.
                if name.len() >= prefix.len() + segment_suffix.len() {
                    return Some(name);
                }
            }
            None
        })
        .collect();

    match matches.len() {
        0 => Err(format!(
            "no directory in '{install_dir}' matches pattern '{prefix}*{segment_suffix}'"
        )),
        1 => Ok(format!("{}{}", matches.remove(0), tail)),
        _ => Err(format!(
            "pattern '{prefix}*{segment_suffix}' is ambiguous in '{install_dir}': {matches:?}"
        )),
    }
}

/// Infer the archive format from a download URL.
///
/// The shelf mixes `.zip` (Windows RCP, Windows JRE) and `.tar.gz` (linux/mac
/// JRE, some RCPs) assets, and the Zed host will fail with
/// `Encountered an unexpected header` if we lie about the type. We strip any
/// query string / fragment first, then look at the file suffix case-insensitively.
///
/// Returns `None` for unknown extensions so the caller can raise a precise
/// error instead of silently falling back to `Zip`.
fn detect_archive_type(url: &str) -> Option<DownloadedFileType> {
    // Drop `?query` and `#fragment`.
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();

    if path.ends_with(".zip") {
        Some(DownloadedFileType::Zip)
    } else if path.ends_with(".tar.gz") || path.ends_with(".tgz") {
        Some(DownloadedFileType::GzipTar)
    } else if path.ends_with(".gz") {
        // Bare `.gz` (single-file gzip). Uncommon for our assets but supported.
        Some(DownloadedFileType::Gzip)
    } else {
        None
    }
}

fn marker_matches(path: &str, expected_version: &str) -> bool {
    match std::fs::read_to_string(path) {
        Ok(contents) => contents
            .lines()
            .next()
            .map(|first| first.trim() == expected_version)
            .unwrap_or(false),
        Err(_) => false,
    }
}

fn write_marker(path: &str, version: &str, sha256: Option<&str>) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create dir {}: {e}", parent.display()))?;
    }
    let body = format!("{}\n{}\n", version, sha256.unwrap_or(""));
    std::fs::write(path, body).map_err(|e| format!("write marker {path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn marker_round_trip() {
        let tmp = std::env::temp_dir().join("zed-mplab-marker-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let marker = tmp.join(".installed");
        let marker_s = marker.to_string_lossy().replace('\\', "/");

        assert!(!marker_matches(&marker_s, "1.2.3"));
        write_marker(&marker_s, "1.2.3", Some("deadbeef")).unwrap();
        assert!(marker_matches(&marker_s, "1.2.3"));
        assert!(!marker_matches(&marker_s, "9.9.9"));

        // Second line should contain the sha.
        let mut f = std::fs::File::create(&marker).unwrap();
        writeln!(f, "4.5.6").unwrap();
        assert!(marker_matches(&marker_s, "4.5.6"));
    }

    #[test]
    fn marker_missing_is_not_a_match() {
        let missing = "nonexistent-dir-zed-mplab/.installed";
        assert!(!marker_matches(missing, "anything"));
    }

    #[test]
    fn detect_zip_urls() {
        assert!(matches!(
            detect_archive_type("https://x/foo.zip"),
            Some(DownloadedFileType::Zip)
        ));
        assert!(matches!(
            detect_archive_type("https://x/Foo.ZIP"),
            Some(DownloadedFileType::Zip)
        ));
        assert!(matches!(
            detect_archive_type("https://x/foo.zip?token=abc"),
            Some(DownloadedFileType::Zip)
        ));
        assert!(matches!(
            detect_archive_type("https://x/foo.zip#frag"),
            Some(DownloadedFileType::Zip)
        ));
    }

    #[test]
    fn detect_tar_gz_urls() {
        assert!(matches!(
            detect_archive_type("https://x/foo.tar.gz"),
            Some(DownloadedFileType::GzipTar)
        ));
        assert!(matches!(
            detect_archive_type("https://x/foo.TAR.GZ"),
            Some(DownloadedFileType::GzipTar)
        ));
        assert!(matches!(
            detect_archive_type("https://x/foo.tgz"),
            Some(DownloadedFileType::GzipTar)
        ));
        assert!(matches!(
            detect_archive_type(
                "https://shelf.download.microchip.com/apps/mplab_backend/0.2.655/\
                 mplab_backend-linux-x64.tar.gz?x=1"
            ),
            Some(DownloadedFileType::GzipTar)
        ));
    }

    #[test]
    fn detect_bare_gz_url() {
        assert!(matches!(
            detect_archive_type("https://x/foo.json.gz"),
            Some(DownloadedFileType::Gzip)
        ));
    }

    #[test]
    fn detect_unknown_returns_none() {
        assert!(detect_archive_type("https://x/foo").is_none());
        assert!(detect_archive_type("https://x/foo.tar").is_none());
        assert!(detect_archive_type("https://x/foo.7z").is_none());
        assert!(detect_archive_type("").is_none());
    }

    /// Create an isolated temp dir for a single test and return its string
    /// path (forward-slash, consistent with the rest of the module).
    fn scratch_dir(tag: &str) -> String {
        let p = std::env::temp_dir().join(format!(
            "zed-mplab-bootstrap-{tag}-{}",
            // Collision-resistant enough for serial cargo tests.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p.to_string_lossy().replace('\\', "/")
    }

    #[test]
    fn expand_wildcard_resolves_zulu_style_prefix() {
        let dir = scratch_dir("zulu");
        std::fs::create_dir_all(format!("{dir}/zulu25.30.17-ca-jre25.0.1-win_x64/bin")).unwrap();

        let resolved = expand_wildcard(&dir, "zulu25.*-win_x64/bin").unwrap();
        assert_eq!(resolved, "zulu25.30.17-ca-jre25.0.1-win_x64/bin");
    }

    #[test]
    fn expand_wildcard_resolves_deeply_nested_tail() {
        // Darwin Zulu shape: tail reaches into Contents/Home/bin under the
        // wrapper directory. The wildcard only matches the top-level entry.
        let dir = scratch_dir("zulu-mac");
        std::fs::create_dir_all(format!(
            "{dir}/zulu25.30.17-ca-jre25.0.1-macosx_aarch64/zulu-25.jre/Contents/Home/bin"
        ))
        .unwrap();

        let resolved = expand_wildcard(
            &dir,
            "zulu25.*-macosx_aarch64/zulu-25.jre/Contents/Home/bin",
        )
        .unwrap();
        assert_eq!(
            resolved,
            "zulu25.30.17-ca-jre25.0.1-macosx_aarch64/zulu-25.jre/Contents/Home/bin"
        );
    }

    #[test]
    fn expand_wildcard_errors_when_no_match() {
        let dir = scratch_dir("no-match");
        std::fs::create_dir_all(format!("{dir}/unrelated")).unwrap();
        let err = expand_wildcard(&dir, "zulu25.*-win_x64/bin").unwrap_err();
        assert!(err.contains("no directory"), "{err}");
    }

    #[test]
    fn expand_wildcard_errors_when_ambiguous() {
        let dir = scratch_dir("ambiguous");
        std::fs::create_dir_all(format!("{dir}/zulu25.1-win_x64")).unwrap();
        std::fs::create_dir_all(format!("{dir}/zulu25.2-win_x64")).unwrap();
        let err = expand_wildcard(&dir, "zulu25.*-win_x64/bin").unwrap_err();
        assert!(err.contains("ambiguous"), "{err}");
    }

    #[test]
    fn expand_wildcard_rejects_multiple_stars() {
        let dir = scratch_dir("multi-star");
        let err = expand_wildcard(&dir, "a/*/*/b").unwrap_err();
        assert!(
            err.contains("single '*'") || err.contains("first path segment"),
            "{err}"
        );
    }

    #[test]
    fn expand_wildcard_rejects_wildcard_beyond_first_segment() {
        let dir = scratch_dir("deep-star");
        let err = expand_wildcard(&dir, "fixed/prefix-*-suffix/bin").unwrap_err();
        assert!(err.contains("first path segment"), "{err}");
    }
}
