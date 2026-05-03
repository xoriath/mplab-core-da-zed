//! Shared HTTP cache for all Microchip downloads.
//!
//! Zed API 0.7 exposes request/response headers through `http_client::fetch`,
//! but it does not expose HTTP status. We therefore infer `304 Not Modified`
//! as a successful conditional GET with an empty body while a cached body
//! exists. Microchip shelf/index/archive resources are never valid empty files.

mod cache_control;
mod conditional;
mod lock;
mod meta;

use sha2::{Digest, Sha256};

use crate::paths;

pub(crate) use meta::CacheMeta;

pub(crate) fn fetch(url: &str, bucket: &str) -> Result<String, String> {
    let path = cache_body_path(url, bucket);
    fetch_to_path(url, &path, bucket)?;
    Ok(path)
}

pub(crate) fn fetch_to_path(url: &str, body_path: &str, bucket: &str) -> Result<(), String> {
    let meta_path = format!("{body_path}.meta.json");
    let lock_path = format!("{}/http/{bucket}/.bucket.lock", paths::CACHE_ROOT);
    let _lock = lock::FileLock::acquire(lock_path)?;
    let now = now_secs();

    let existing_meta = meta::read(&meta_path).filter(|m| m.url == url);
    if let Some(existing) = existing_meta.as_ref() {
        if meta::is_fresh(existing, now, std::path::Path::new(body_path).exists()) {
            return Ok(());
        }
    }

    let body_exists = std::path::Path::new(body_path).exists();
    match conditional::get(url, existing_meta.as_ref()) {
        Ok(response) if response.body.is_empty() && existing_meta.is_some() && body_exists => {
            let mut refreshed = existing_meta.expect("checked is_some");
            apply_response_headers(
                &mut refreshed,
                url,
                &response.headers,
                now,
                "revalidated",
                None,
            );
            meta::write(&meta_path, &refreshed)
        }
        Ok(response) => {
            if let Some(parent) = std::path::Path::new(body_path).parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create cache dir {}: {e}", parent.display()))?;
            }
            std::fs::write(body_path, &response.body)
                .map_err(|e| format!("write cached body {body_path}: {e}"))?;

            let sha256 = Some(hex_sha256(&response.body));
            let mut fresh = CacheMeta {
                url: url.to_string(),
                etag: None,
                last_modified: None,
                fetched_at: now,
                max_age: None,
                expires_at: None,
                immutable: false,
                no_cache: false,
                no_store: false,
                must_revalidate: false,
                sha256,
                content_length: Some(response.body.len() as u64),
                status: "fresh".to_string(),
            };
            apply_response_headers(&mut fresh, url, &response.headers, now, "fresh", None);
            meta::write(&meta_path, &fresh)
        }
        Err(_err) if body_exists => Ok(()),
        Err(err) => Err(format!("fetch {url}: {err}")),
    }
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn apply_response_headers(
    meta: &mut CacheMeta,
    url: &str,
    headers: &[(String, String)],
    now: u64,
    status: &str,
    fallback_sha256: Option<String>,
) {
    let cache_control = cache_control::parse(header(headers, "cache-control"));
    meta.url = url.to_string();
    meta.etag = header(headers, "etag").map(str::to_string);
    meta.last_modified = header(headers, "last-modified").map(str::to_string);
    meta.fetched_at = now;
    meta.max_age = cache_control.max_age;
    meta.expires_at = cache_control
        .max_age
        .map(|max_age| now.saturating_add(max_age));
    meta.immutable = cache_control.immutable;
    meta.no_cache = cache_control.no_cache;
    meta.no_store = cache_control.no_store;
    meta.must_revalidate = cache_control.must_revalidate;
    if meta.sha256.is_none() {
        meta.sha256 = fallback_sha256;
    }
    meta.content_length = header(headers, "content-length")
        .and_then(|value| value.parse::<u64>().ok())
        .or(meta.content_length);
    meta.status = status.to_string();
}

fn header<'a>(headers: &'a [(String, String)], wanted: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
        .map(|(_, value)| value.as_str())
}

fn cache_body_path(url: &str, bucket: &str) -> String {
    format!(
        "{}/http/{bucket}/{}.bin",
        paths::CACHE_ROOT,
        hex_sha256(url.as_bytes())
    )
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_url_to_stable_cache_path() {
        let a = cache_body_path("https://example.test/a", "shelf");
        let b = cache_body_path("https://example.test/a", "shelf");
        assert_eq!(a, b);
        assert!(a.starts_with("cache/http/shelf/"));
        assert!(a.ends_with(".bin"));
    }

    #[test]
    fn finds_headers_case_insensitively() {
        let headers = vec![("ETag".to_string(), "abc".to_string())];
        assert_eq!(header(&headers, "etag"), Some("abc"));
    }
}
