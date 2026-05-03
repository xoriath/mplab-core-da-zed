//! Minimal `Cache-Control` parser used by all Microchip downloads.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CacheControl {
    pub max_age: Option<u64>,
    pub immutable: bool,
    pub no_cache: bool,
    pub no_store: bool,
    pub must_revalidate: bool,
}

pub(crate) fn parse(value: Option<&str>) -> CacheControl {
    let mut out = CacheControl::default();
    let Some(value) = value else {
        return out;
    };

    for raw in value.split(',') {
        let directive = raw.trim();
        let lower = directive.to_ascii_lowercase();
        match lower.as_str() {
            "immutable" => out.immutable = true,
            "no-cache" => out.no_cache = true,
            "no-store" => out.no_store = true,
            "must-revalidate" => out.must_revalidate = true,
            _ => {
                if let Some(rest) = lower.strip_prefix("max-age=") {
                    if let Ok(seconds) = rest.trim_matches('"').parse::<u64>() {
                        out.max_age = Some(seconds);
                    }
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_directives() {
        let cc = parse(Some("public, max-age=3600, immutable, must-revalidate"));
        assert_eq!(cc.max_age, Some(3600));
        assert!(cc.immutable);
        assert!(cc.must_revalidate);
    }

    #[test]
    fn parses_no_cache_and_no_store() {
        let cc = parse(Some("no-cache, no-store"));
        assert!(cc.no_cache);
        assert!(cc.no_store);
    }
}
