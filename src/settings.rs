//! User-adjustable MPLAB extension settings.
//!
//! `zed_extension_api` 0.7 exposes typed accessors for built-in settings
//! categories, but does not publicly expose the raw `get_settings` import for
//! extension-defined categories. This module centralizes the defaults and the
//! launch-time interpretation so wiring stays small now and can be connected to
//! a public custom-settings API later without touching the DAP logic.

use crate::{launch_config, paths, shelf};

/// Microchip's public pack catalog. The `.idx.gz` payload is a gzip-compressed
/// XML document listing every pack URL plus the devices and tool-firmware
/// components it provides.
pub(crate) const DEFAULT_PACK_INDEX_URL: &str = "https://packs.download.microchip.com/index.idx.gz";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MplabSettings {
    pub rcp_path: Option<String>,
    pub java_home: Option<String>,
    pub user_dir: String,
    pub cache_dir: String,
    pub pack_repo: String,
    pub shelf_url: String,
    pub pack_index_url: Option<String>,
    pub jre_name: String,
    pub log_level: String,
    pub symbol_loading: SymbolLoading,
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum SymbolLoading {
    OnDemand,
    PreProcessed,
}

impl MplabSettings {
    pub(crate) fn load() -> Self {
        // See module docs: defaults only until the Zed API exposes custom
        // settings to extension authors.
        Self::default()
    }

    pub(crate) fn symbol_loading_value(&self) -> &'static str {
        match self.symbol_loading {
            SymbolLoading::OnDemand => "true",
            SymbolLoading::PreProcessed => "false",
        }
    }

    pub(crate) fn jul_log_level(&self) -> String {
        jul_log_level(&self.log_level).to_string()
    }
}

impl Default for MplabSettings {
    fn default() -> Self {
        Self {
            rcp_path: None,
            java_home: None,
            user_dir: paths::USER_ROOT.to_string(),
            cache_dir: paths::RCP_CACHE_ROOT.to_string(),
            pack_repo: paths::PACKS_ROOT.to_string(),
            shelf_url: shelf::DEFAULT_SHELF_URL.to_string(),
            pack_index_url: Some(DEFAULT_PACK_INDEX_URL.to_string()),
            jre_name: launch_config::DEFAULT_JRE_NAME.to_string(),
            log_level: "WARNING".to_string(),
            symbol_loading: SymbolLoading::OnDemand,
            extra_args: Vec::new(),
        }
    }
}

pub(crate) fn jul_log_level(value: &str) -> &'static str {
    match value.trim().to_ascii_uppercase().as_str() {
        "OFF" => "2147483647",
        "SEVERE" => "1000",
        "WARNING" | "" => launch_config::DEFAULT_LOG_LEVEL,
        "INFO" => "800",
        "CONFIG" => "700",
        "FINE" => "500",
        "FINER" => "400",
        "FINEST" => "300",
        "ALL" => "-2147483648",
        _ => launch_config::DEFAULT_LOG_LEVEL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_runtime_paths() {
        let settings = MplabSettings::default();
        assert_eq!(settings.user_dir, "user");
        assert_eq!(settings.cache_dir, "cache/rcp");
        assert_eq!(settings.pack_repo, "packs");
        assert_eq!(settings.shelf_url, shelf::DEFAULT_SHELF_URL);
        assert_eq!(settings.jre_name, launch_config::DEFAULT_JRE_NAME);
        assert_eq!(
            settings.pack_index_url.as_deref(),
            Some(DEFAULT_PACK_INDEX_URL)
        );
    }

    #[test]
    fn maps_java_logging_levels() {
        assert_eq!(jul_log_level("OFF"), "2147483647");
        assert_eq!(jul_log_level("warning"), "900");
        assert_eq!(jul_log_level("INFO"), "800");
        assert_eq!(jul_log_level("ALL"), "-2147483648");
        assert_eq!(jul_log_level("unexpected"), "900");
    }
}
