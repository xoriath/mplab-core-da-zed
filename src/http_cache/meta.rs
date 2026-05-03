//! Sidecar metadata for cached HTTP bodies.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CacheMeta {
    pub url: String,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
    pub fetched_at: u64,
    #[serde(default)]
    pub max_age: Option<u64>,
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub immutable: bool,
    #[serde(default)]
    pub no_cache: bool,
    #[serde(default)]
    pub no_store: bool,
    #[serde(default)]
    pub must_revalidate: bool,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub content_length: Option<u64>,
    pub status: String,
}

pub(crate) fn read(path: &str) -> Option<CacheMeta> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

pub(crate) fn write(path: &str, meta: &CacheMeta) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create metadata dir {}: {e}", parent.display()))?;
    }
    let contents = serde_json::to_string_pretty(meta)
        .map_err(|e| format!("serialize cache metadata {path}: {e}"))?;
    std::fs::write(path, contents).map_err(|e| format!("write cache metadata {path}: {e}"))
}

pub(crate) fn is_fresh(meta: &CacheMeta, now: u64, body_exists: bool) -> bool {
    if !body_exists || meta.no_store || meta.no_cache {
        return false;
    }
    if meta.immutable {
        return true;
    }
    meta.expires_at.is_some_and(|expires| now < expires)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_is_fresh_when_body_exists() {
        let meta = CacheMeta {
            url: "https://example.test/file".into(),
            etag: None,
            last_modified: None,
            fetched_at: 1,
            max_age: None,
            expires_at: None,
            immutable: true,
            no_cache: false,
            no_store: false,
            must_revalidate: false,
            sha256: None,
            content_length: None,
            status: "fresh".into(),
        };
        assert!(is_fresh(&meta, 100, true));
        assert!(!is_fresh(&meta, 100, false));
    }
}
