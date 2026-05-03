//! Conditional HTTP GET support.

use crate::http_cache::meta::CacheMeta;
use zed_extension_api::http_client::{HttpMethod, HttpRequest, HttpResponse, RedirectPolicy};

pub(crate) fn get(url: &str, meta: Option<&CacheMeta>) -> Result<HttpResponse, String> {
    let mut builder = HttpRequest::builder()
        .method(HttpMethod::Get)
        .url(url)
        .redirect_policy(RedirectPolicy::FollowLimit(5));

    if let Some(meta) = meta {
        if let Some(etag) = meta.etag.as_deref() {
            builder = builder.header("If-None-Match", etag);
        }
        if let Some(last_modified) = meta.last_modified.as_deref() {
            builder = builder.header("If-Modified-Since", last_modified);
        }
    }

    let request = builder.build()?;
    request.fetch()
}
