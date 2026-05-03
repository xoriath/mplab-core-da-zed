//! Per-OS path computations for the extension.
//!
//! The Zed `download_file` and `make_file_executable` host functions operate
//! on paths **relative to the extension's work directory**, so every helper
//! here returns a `String` built with forward slashes. We never call `chdir`
//! (wasip2 forbids it).
//!
//! Paths handed back to Zed that end up on the host (e.g. the spawn
//! `command` and the Java `--jdkhome` argument) need to be **absolute**,
//! because the spawned process runs with the worktree as its cwd — not the
//! extension work dir. [`absolutize`] joins a relative path onto
//! [`work_dir`] (populated from the `PWD` env var by
//! `zed_extension_api::register_extension!`).
//!
//! Layout under the extension work dir:
//! ```text
//! apps/mplab_backend/<ver>/...       (extracted RCP install)
//! apps/zulu-jre-25/<ver>/...         (extracted JRE; jre name is parameterised)
//! packs/                             (DFP repository, `-J-Dpackslib.packsfolder`)
//! cache/                             (scratch space, RCP --cachedir)
//! user/                              (RCP --userdir)
//! ```

use zed_extension_api::{Architecture, Os};

/// Root of all downloaded/installed apps, relative to the extension workdir.
pub(crate) const APPS_ROOT: &str = "apps";
/// Pack repository root (consumed by the Java backend via
/// `-J-Dpackslib.packsfolder`).
pub(crate) const PACKS_ROOT: &str = "packs";
/// Generic extension cache (HTTP cache sidecars, etc.).
pub(crate) const CACHE_ROOT: &str = "cache";
/// RCP `--userdir` location.
pub(crate) const USER_ROOT: &str = "user";
/// RCP `--cachedir` location.
pub(crate) const RCP_CACHE_ROOT: &str = "cache/rcp";

/// Fully resolved paths for an installed `mplab_backend`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RcpPaths {
    /// Install root, e.g. `apps/mplab_backend/2.0.20`.
    pub install_dir: String,
    /// RCP launcher executable, e.g. `apps/mplab_backend/2.0.20/bin/mplab_backend64.exe`.
    pub executable: String,
    /// Path to the marker file written after a successful install.
    pub marker: String,
}

/// Fully resolved paths for an installed JRE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JrePaths {
    /// Install root, e.g. `apps/zulu-jre-25/25.0.1`.
    pub install_dir: String,
    /// `JAVA_HOME` (one directory above `bin/java`).
    pub java_home: String,
    /// Path to the marker file written after a successful install.
    pub marker: String,
}

/// Where the `mplab_backend` lives for a given version.
///
/// `bin_dir` is the shelf-declared path (relative to the install root) that
/// contains the launcher binary. When supplied we trust it verbatim; when
/// absent we fall back to the OS-based layout that ships with mplab_backend
/// today. The fallback is also used by legacy/self-made shelves that omit
/// `installationLocation`, and by unit tests.
pub(crate) fn rcp_paths(version: &str, os: Os, bin_dir: Option<&str>) -> RcpPaths {
    let install_dir = format!("{APPS_ROOT}/mplab_backend/{version}");
    let binary_name = rcp_binary_name(os);
    let executable = match bin_dir {
        Some(bd) => format!("{install_dir}/{bd}/{binary_name}"),
        None => match os {
            // Fallbacks mirror what the RCP produces today. macOS unpacks as
            // a `.app` bundle whose launcher lives under
            // `Contents/Resources/mplab_backend/bin/`, not under
            // `Contents/MacOS/` (which only holds the native stub).
            Os::Windows | Os::Linux => format!("{install_dir}/mplab_backend/bin/{binary_name}"),
            Os::Mac => format!(
                "{install_dir}/mplab_backend.app/Contents/Resources/mplab_backend/bin/{binary_name}"
            ),
        },
    };
    let marker = format!("{install_dir}/.installed");
    RcpPaths {
        install_dir,
        executable,
        marker,
    }
}

/// Where a JRE (zulu-jre-25 by default) lives.
///
/// `bin_dir` is the shelf-declared path to the JRE's `bin/` directory
/// relative to the install root. `JAVA_HOME` is `bin_dir` with a trailing
/// `/bin` stripped — the Java runtime expects that layout.
///
/// When `bin_dir` is absent we fall back to the legacy heuristic: on macOS
/// the archive unpacks as `<install>/Contents/Home/bin/java`; on
/// linux/windows the archive's top-level dir *is* `JAVA_HOME`.
///
/// `bin_dir` may contain a `*` wildcard (Zulu distributions embed a build
/// number in the top-level directory); the wildcard must be resolved by the
/// caller before the returned paths are used for I/O.
pub(crate) fn jre_paths(jre_name: &str, version: &str, os: Os, bin_dir: Option<&str>) -> JrePaths {
    let install_dir = format!("{APPS_ROOT}/{jre_name}/{version}");
    let java_home = match bin_dir {
        Some(bd) => {
            let trimmed = bd.strip_suffix("/bin").unwrap_or(bd);
            format!("{install_dir}/{trimmed}")
        }
        None => match os {
            Os::Mac => format!("{install_dir}/Contents/Home"),
            _ => install_dir.clone(),
        },
    };
    let marker = format!("{install_dir}/.installed");
    JrePaths {
        install_dir,
        java_home,
        marker,
    }
}

/// Filename of the RCP launcher on a given OS.
fn rcp_binary_name(os: Os) -> &'static str {
    match os {
        Os::Windows => "mplab_backend64.exe",
        _ => "mplab_backend",
    }
}

/// Absolute path of the extension work directory on the host.
///
/// Zed populates `PWD` (and the wasi cwd derived from it) with the absolute
/// host path of the per-extension work dir before invoking any entry
/// point. That value is what Zed's `download_file`, `make_file_executable`
/// and `std::fs::*` resolve relative paths against from inside the wasm
/// sandbox. It is also what the *host* OS sees when we hand a relative
/// path back to Zed for e.g. process spawning — except that the host then
/// resolves it against the worktree, not against our work dir. Hence the
/// need to pre-absolutize paths that leave the extension.
///
/// Returns `None` if neither `std::env::current_dir()` nor `$PWD` is
/// usable (practically only in non-wasi unit tests where the caller
/// didn't set `PWD`).
pub(crate) fn work_dir() -> Option<String> {
    if let Ok(pwd) = std::env::var("PWD") {
        if !pwd.is_empty() {
            return Some(pwd);
        }
    }
    std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Join `relative` onto the extension's work directory so the result is
/// absolute on the host.
///
/// If `relative` already looks absolute (Unix `/...`, a Windows drive-letter
/// path like `C:\...`, a UNC path `\\server\share\...`, or starts with a
/// path separator), it is returned as-is — user-provided overrides are
/// expected to already be absolute.
///
/// The work dir is normalised to forward slashes before joining so the
/// returned string is usable on every host (Windows' `CreateProcess`
/// accepts forward slashes, and Java's `--jdkhome` is agnostic).
pub(crate) fn absolutize(relative: &str) -> String {
    if looks_absolute(relative) {
        return relative.to_string();
    }
    match work_dir() {
        Some(wd) => {
            let wd = wd.replace('\\', "/");
            let wd = wd.trim_end_matches('/');
            format!("{wd}/{relative}")
        }
        // No work dir discoverable (very unusual — tests only): return the
        // input unchanged rather than fabricate something. Callers surface
        // spawn failures with a clear error if this happens.
        None => relative.to_string(),
    }
}

/// Conservative absolute-path detector that handles both POSIX and Windows
/// host conventions without relying on `std::path::Path::is_absolute`
/// (which inside a wasi sandbox only recognises `/...`).
fn looks_absolute(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    let bytes = p.as_bytes();
    // POSIX absolute and Windows rooted-relative (`/foo`, `\foo`). Both
    // are treated as "do not prefix" — users who hand us `/opt/...` or
    // `\Users\...` meant that literally.
    if matches!(bytes[0], b'/' | b'\\') {
        return true;
    }
    // Windows drive-letter: `C:\...` or `C:/...` (or just `C:` — rare but
    // still absolute-ish; callers wouldn't pass that).
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
    {
        return true;
    }
    false
}

/// Short string name of an OS (for shelf lookups).
pub(crate) fn os_name(os: Os) -> &'static str {
    match os {
        Os::Windows => "win32",
        Os::Linux => "linux",
        Os::Mac => "darwin",
    }
}

/// Short string name of an architecture (for shelf lookups).
pub(crate) fn arch_name(arch: Architecture) -> &'static str {
    match arch {
        Architecture::Aarch64 => "arm64",
        Architecture::X8664 => "x64",
        Architecture::X86 => "x86",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rcp_executable_windows_with_shelf_bin_dir() {
        let p = rcp_paths("2.0.20", Os::Windows, Some("mplab_backend/bin"));
        assert_eq!(p.install_dir, "apps/mplab_backend/2.0.20");
        assert_eq!(
            p.executable,
            "apps/mplab_backend/2.0.20/mplab_backend/bin/mplab_backend64.exe"
        );
        assert_eq!(p.marker, "apps/mplab_backend/2.0.20/.installed");
    }

    #[test]
    fn rcp_executable_linux_with_shelf_bin_dir() {
        let p = rcp_paths("2.0.20", Os::Linux, Some("mplab_backend/bin"));
        assert_eq!(
            p.executable,
            "apps/mplab_backend/2.0.20/mplab_backend/bin/mplab_backend"
        );
    }

    #[test]
    fn rcp_executable_mac_with_shelf_bin_dir() {
        let p = rcp_paths(
            "2.0.20",
            Os::Mac,
            Some("mplab_backend.app/Contents/Resources/mplab_backend/bin"),
        );
        assert_eq!(
            p.executable,
            "apps/mplab_backend/2.0.20/\
             mplab_backend.app/Contents/Resources/mplab_backend/bin/mplab_backend"
        );
    }

    #[test]
    fn rcp_executable_fallback_windows() {
        // No shelf `binDir`: fall back to the current MPLAB layout
        // (`mplab_backend/bin/` under the install root).
        let p = rcp_paths("2.0.20", Os::Windows, None);
        assert_eq!(
            p.executable,
            "apps/mplab_backend/2.0.20/mplab_backend/bin/mplab_backend64.exe"
        );
    }

    #[test]
    fn rcp_executable_fallback_mac_uses_app_bundle_resources() {
        let p = rcp_paths("2.0.20", Os::Mac, None);
        assert_eq!(
            p.executable,
            "apps/mplab_backend/2.0.20/\
             mplab_backend.app/Contents/Resources/mplab_backend/bin/mplab_backend"
        );
    }

    #[test]
    fn jre_java_home_strips_trailing_bin() {
        let p = jre_paths(
            "zulu-jre-25",
            "25.0.1",
            Os::Windows,
            Some("zulu25.30.17-ca-jre25.0.1-win_x64/bin"),
        );
        assert_eq!(p.install_dir, "apps/zulu-jre-25/25.0.1");
        assert_eq!(
            p.java_home,
            "apps/zulu-jre-25/25.0.1/zulu25.30.17-ca-jre25.0.1-win_x64"
        );
    }

    #[test]
    fn jre_java_home_mac_with_nested_contents_home_bin() {
        let p = jre_paths(
            "zulu-jre-25",
            "25.0.1",
            Os::Mac,
            Some("zulu25.30.17-ca-jre25.0.1-macosx_aarch64/zulu-25.jre/Contents/Home/bin"),
        );
        assert_eq!(
            p.java_home,
            "apps/zulu-jre-25/25.0.1/\
             zulu25.30.17-ca-jre25.0.1-macosx_aarch64/zulu-25.jre/Contents/Home"
        );
    }

    #[test]
    fn jre_java_home_fallback_mac_has_contents_home() {
        let p = jre_paths("zulu-jre-25", "25.0.1", Os::Mac, None);
        assert_eq!(p.java_home, "apps/zulu-jre-25/25.0.1/Contents/Home");
    }

    #[test]
    fn jre_java_home_fallback_linux_equals_install_dir() {
        let p = jre_paths("zulu-jre-25", "25.0.1", Os::Linux, None);
        assert_eq!(p.java_home, p.install_dir);
    }

    #[test]
    fn jre_java_home_fallback_windows_equals_install_dir() {
        let p = jre_paths("zulu-jre-25", "25.0.1", Os::Windows, None);
        assert_eq!(p.java_home, "apps/zulu-jre-25/25.0.1");
    }

    #[test]
    fn jre_java_home_preserves_bin_dir_without_trailing_bin() {
        // If the shelf ever ships a `binDir` that doesn't end in `/bin`
        // (defensive), we still want to treat it as JAVA_HOME rather than
        // accidentally stripping a path segment.
        let p = jre_paths("zulu-jre-25", "25.0.1", Os::Linux, Some("some/other/dir"));
        assert_eq!(p.java_home, "apps/zulu-jre-25/25.0.1/some/other/dir");
    }

    #[test]
    fn looks_absolute_posix() {
        assert!(looks_absolute("/usr/local/bin/java"));
        assert!(looks_absolute("/"));
    }

    #[test]
    fn looks_absolute_windows() {
        assert!(looks_absolute(r"C:\Program Files\Java\bin\java.exe"));
        assert!(looks_absolute("C:/Program Files/Java/bin/java.exe"));
        assert!(looks_absolute(r"\\server\share\file"));
        assert!(looks_absolute(r"\Users\foo"));
    }

    #[test]
    fn looks_absolute_relative_inputs() {
        assert!(!looks_absolute(""));
        assert!(!looks_absolute("apps/mplab_backend/2.0/bin/mplab_backend"));
        assert!(!looks_absolute("relative\\path"));
        assert!(!looks_absolute("C:")); // drive letter without separator, unsupported
        assert!(!looks_absolute("abc"));
    }

    #[test]
    fn absolutize_leaves_absolute_paths_untouched() {
        assert_eq!(absolutize("/opt/java"), "/opt/java");
        assert_eq!(absolutize(r"C:\Java\bin"), r"C:\Java\bin");
    }

    #[test]
    fn absolutize_prefixes_work_dir_and_normalises_slashes() {
        // SAFETY: tests run serially for env vars; this is a small block and
        // we restore afterwards.
        let saved = std::env::var("PWD").ok();
        // Use a path with a Windows-style separator to confirm we normalise.
        unsafe {
            std::env::set_var("PWD", r"C:\work\extension");
        }
        let got = absolutize("apps/mplab_backend/1.0/bin/mplab_backend64.exe");
        assert_eq!(
            got,
            "C:/work/extension/apps/mplab_backend/1.0/bin/mplab_backend64.exe"
        );
        match saved {
            Some(v) => unsafe { std::env::set_var("PWD", v) },
            None => unsafe { std::env::remove_var("PWD") },
        }
    }

    #[test]
    fn absolutize_trims_trailing_slash_on_work_dir() {
        let saved = std::env::var("PWD").ok();
        unsafe {
            std::env::set_var("PWD", "/home/foo/work/");
        }
        assert_eq!(absolutize("apps/x/1.0"), "/home/foo/work/apps/x/1.0");
        match saved {
            Some(v) => unsafe { std::env::set_var("PWD", v) },
            None => unsafe { std::env::remove_var("PWD") },
        }
    }

    #[test]
    fn os_and_arch_names() {
        assert_eq!(os_name(Os::Windows), "win32");
        assert_eq!(os_name(Os::Linux), "linux");
        assert_eq!(os_name(Os::Mac), "darwin");
        assert_eq!(arch_name(Architecture::X8664), "x64");
        assert_eq!(arch_name(Architecture::Aarch64), "arm64");
        assert_eq!(arch_name(Architecture::X86), "x86");
    }
}
