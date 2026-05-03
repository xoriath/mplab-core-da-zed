//! Pack archive download and extraction.

use std::io::Read;

use sha2::{Digest, Sha256};

use super::{cache, PackRef};
use crate::http_cache;

pub(crate) fn ensure_installed(pack: &PackRef) -> Result<String, String> {
    let install_dir = cache::installed_dir(pack);
    if cache::is_installed(pack) {
        return Ok(install_dir);
    }

    let archive = cache::archive_cache_path(pack);
    http_cache::fetch_to_path(&pack.url, &archive, "packs")?;
    verify_sha256(&archive, pack.sha256.as_deref())?;
    extract_zip(&archive, &install_dir)?;
    cache::write_marker(pack)?;
    Ok(install_dir)
}

fn verify_sha256(path: &str, expected: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let bytes = std::fs::read(path).map_err(|e| format!("read pack archive {path}: {e}"))?;
    let digest = Sha256::digest(&bytes);
    let mut actual = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut actual, "{byte:02x}");
    }
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch for {path}: expected {expected}, got {actual}"
        ))
    }
}

fn extract_zip(archive_path: &str, install_dir: &str) -> Result<(), String> {
    let file = std::fs::File::open(archive_path)
        .map_err(|e| format!("open pack archive {archive_path}: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("read zip archive {archive_path}: {e}"))?;

    std::fs::create_dir_all(install_dir)
        .map_err(|e| format!("create pack install dir {install_dir}: {e}"))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("read zip entry {i} from {archive_path}: {e}"))?;
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let out = std::path::Path::new(install_dir).join(enclosed);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|e| format!("create zip dir {}: {e}", out.display()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create zip parent {}: {e}", parent.display()))?;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| format!("read zip entry {}: {e}", entry.name()))?;
        std::fs::write(&out, bytes)
            .map_err(|e| format!("write zip entry {}: {e}", out.display()))?;
    }

    Ok(())
}
