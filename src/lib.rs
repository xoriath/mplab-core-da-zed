//! Zed debug adapter extension for Microchip MPLAB.
//!
//! This crate is built as `wasm32-wasip2` and loaded by Zed as a component.
//! The adapter id is `mplab`; users select it via `"adapter": "mplab"` in
//! `.zed/debug.json`. Launch schema lives in `debug_adapter_schemas/mplab.json`.
//!
//! # Architecture (phase 0 skeleton)
//!
//! The extension is currently a minimal stub that returns an error from
//! [`MplabExtension::get_dap_binary`]. Subsequent phases fill in:
//!
//! * **Phase 1** — bootstrap: fetch the shelf manifest, download and extract
//!   the MPLAB backend RCP + Zulu JRE on first use.
//! * **Phase 2** — launch config parsing + spawn of the RCP in DAP-direct mode
//!   (`-J-Ddebug.adapter.protocol.server.port=<P>`), with Zed connecting over TCP.
//! * **Phase 3** — pack service port (device → DFP resolution, install,
//!   ETag/Cache-Control-aware caching).
//! * **Phase 4** — user settings (`mplab.rcpPath`, `mplab.packRepo`, etc.).
//!
//! The extension is stateless by design; each call to
//! [`zed_extension_api::Extension::get_dap_binary`] recomputes what it needs.

use zed::{
    current_platform, DebugAdapterBinary, DebugConfig, DebugRequest, DebugScenario,
    DebugTaskDefinition, StartDebuggingRequestArguments, StartDebuggingRequestArgumentsRequest,
    Worktree,
};
use zed_extension_api as zed;

mod bootstrap;
mod http_cache;
mod launch_config;
mod pack;
mod paths;
mod settings;
mod shelf;

/// Extension object wired into Zed via [`zed::register_extension!`].
///
/// Carries no mutable state: every DAP session re-derives paths, re-reads
/// settings, and re-validates the launch configuration.
pub struct MplabExtension;

impl zed::Extension for MplabExtension {
    fn new() -> Self {
        Self
    }

    fn get_dap_binary(
        &mut self,
        adapter_name: String,
        config: DebugTaskDefinition,
        user_provided_debug_adapter_path: Option<String>,
        worktree: &Worktree,
    ) -> Result<DebugAdapterBinary, String> {
        ensure_adapter(&adapter_name)?;

        let parsed = launch_config::parse_config(&config.config)?;
        let request = launch_config::request_kind(&parsed)?;
        launch_config::validate_for_session(&parsed)?;
        let settings = settings::MplabSettings::load();

        if let Some(external) = launch_config::external_dap(&parsed)? {
            return Ok(DebugAdapterBinary {
                command: None,
                arguments: Vec::new(),
                envs: Vec::new(),
                cwd: None,
                connection: Some(launch_config::tcp_args(external.host, external.port)),
                request_args: StartDebuggingRequestArguments {
                    configuration: config.config,
                    request,
                },
            });
        }

        let (os, arch) = current_platform();
        let needs_shelf = settings.rcp_path.is_none() || settings.java_home.is_none();
        let shelf = if needs_shelf {
            Some(load_shelf(&settings.shelf_url)?)
        } else {
            None
        };

        // The command and --jdkhome both need to be absolute host paths:
        // Zed spawns the adapter with the worktree as cwd, not the
        // extension work dir, so relative paths from `bootstrap::*` would
        // not resolve. `paths::absolutize` is a no-op for inputs that
        // already look absolute (user-provided overrides).
        let command = if let Some(rcp_path) =
            user_provided_debug_adapter_path.or_else(|| settings.rcp_path.clone())
        {
            paths::absolutize(&rcp_path)
        } else {
            paths::absolutize(
                &bootstrap::ensure_mplab_backend(shelf.as_ref().expect("shelf loaded"), os, arch)?
                    .executable,
            )
        };

        let java_home = if let Some(java_home) = settings.java_home.clone() {
            paths::absolutize(&java_home)
        } else {
            paths::absolutize(
                &bootstrap::ensure_zulu_jre(
                    shelf.as_ref().expect("shelf loaded"),
                    &settings.jre_name,
                    os,
                    arch,
                )?
                .java_home,
            )
        };
        let tcp = launch_config::resolve_tcp()?;

        // Absolutize the NetBeans dir arguments. With the worktree as cwd,
        // `user`, `cache/rcp`, and `packs` would otherwise resolve under
        // the user's project — which is almost never what they want and
        // is a common cause of the RCP bailing out during startup (bad
        // userdir locks, missing packs dir, etc.). We pin them to the
        // extension work dir so install state is stable across projects.
        let mut settings = settings;
        settings.user_dir = paths::absolutize(&settings.user_dir);
        settings.cache_dir = paths::absolutize(&settings.cache_dir);
        settings.pack_repo = paths::absolutize(&settings.pack_repo);

        // Best-effort: fetch the pack index and install the DFP (for `device`)
        // and TP (for `tool`) before the RCP needs them. Failures here are
        // reported via eprintln but do NOT abort the session — the user still
        // gets a connection and the RCP will surface any missing-pack error.
        install_packs(&settings, &parsed);

        let arguments = launch_config::rcp_arguments(&java_home, tcp.port, &settings);

        Ok(DebugAdapterBinary {
            command: Some(command),
            arguments,
            envs: Vec::new(),
            cwd: Some(worktree.root_path()),
            connection: Some(tcp),
            request_args: StartDebuggingRequestArguments {
                configuration: config.config,
                request,
            },
        })
    }

    fn dap_request_kind(
        &mut self,
        adapter_name: String,
        config: serde_json::Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest, String> {
        ensure_adapter(&adapter_name)?;
        launch_config::request_kind_from_value(&config)
    }

    fn dap_config_to_scenario(&mut self, config: DebugConfig) -> Result<DebugScenario, String> {
        ensure_adapter(&config.adapter)?;

        let scenario_config = match config.request {
            DebugRequest::Launch(launch) => serde_json::json!({
                "request": "launch",
                "program": launch.program,
                "cwd": launch.cwd,
                "args": launch.args,
                "envs": launch.envs,
                "stopOnEntry": config.stop_on_entry.unwrap_or(false),
                "tool": "PKOB nano",
                "device": "",
            }),
            DebugRequest::Attach(attach) => serde_json::json!({
                "request": "attach",
                "processId": attach.process_id,
                "stopOnEntry": config.stop_on_entry.unwrap_or(false),
                "tool": "PKOB nano",
                "device": "",
            }),
        };

        Ok(DebugScenario {
            label: config.label,
            adapter: config.adapter,
            build: None,
            config: scenario_config.to_string(),
            tcp_connection: None,
        })
    }
}

zed::register_extension!(MplabExtension);

fn ensure_adapter(adapter_name: &str) -> Result<(), String> {
    if adapter_name == launch_config::ADAPTER_NAME {
        Ok(())
    } else {
        Err(format!(
            "unknown debug adapter '{adapter_name}', expected '{}'",
            launch_config::ADAPTER_NAME
        ))
    }
}

fn load_shelf(url: &str) -> Result<shelf::Shelf, String> {
    let shelf_cache_path = http_cache::fetch(url, "shelf")?;

    let json = std::fs::read_to_string(&shelf_cache_path)
        .map_err(|e| format!("read cached shelf {shelf_cache_path}: {e}"))?;
    shelf::parse(&json)
}

/// Best-effort install of the Device Family Pack (DFP) and Tool Pack (TP)
/// needed by the current session. Errors are logged but never propagated —
/// the RCP itself will surface a missing-pack failure if it matters.
fn install_packs(settings: &settings::MplabSettings, config: &launch_config::LaunchConfig) {
    let Some(url) = settings.pack_index_url.as_deref() else {
        return;
    };
    let device = config.device.as_deref().unwrap_or("").trim();
    let tool = config.tool.as_deref().unwrap_or("").trim();
    if device.is_empty() && tool.is_empty() {
        return;
    }

    let cached_path = match http_cache::fetch(url, "packs") {
        Ok(path) => path,
        Err(err) => {
            eprintln!("mplab: pack index fetch failed: {err}");
            return;
        }
    };
    let bytes = match std::fs::read(&cached_path) {
        Ok(b) => b,
        Err(err) => {
            eprintln!("mplab: read cached pack index {cached_path}: {err}");
            return;
        }
    };

    let xml = match decode_pack_index(url, &bytes) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("mplab: decode pack index: {err}");
            return;
        }
    };

    let base = base_url(url);
    let packs = match pack::index::parse(&xml, &base) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("mplab: parse pack index: {err}");
            return;
        }
    };

    if !device.is_empty() {
        match pack::index::resolve_device(&packs, device) {
            Some(p) => {
                if let Err(err) = pack::ensure_pack_installed(p) {
                    eprintln!("mplab: install DFP for device '{device}': {err}");
                }
            }
            None => eprintln!("mplab: no DFP found in index for device '{device}'"),
        }
    }

    if !tool.is_empty() {
        match pack::index::resolve_tool(&packs, tool) {
            Some(p) => {
                if let Err(err) = pack::ensure_pack_installed(p) {
                    eprintln!("mplab: install TP for tool '{tool}': {err}");
                }
            }
            None => eprintln!("mplab: no TP found in index for tool '{tool}'"),
        }
    }
}

/// Decode the cached pack index bytes into XML. The Microchip index is
/// distributed as `.idx.gz`; we also accept bare `.idx`/xml by sniffing
/// the gzip magic.
fn decode_pack_index(url: &str, bytes: &[u8]) -> Result<String, String> {
    let is_gzip =
        url.ends_with(".gz") || (bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b);
    if is_gzip {
        use std::io::Read;
        let mut out = String::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_string(&mut out)
            .map_err(|e| format!("gunzip: {e}"))?;
        Ok(out)
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|e| format!("not utf-8: {e}"))
    }
}

/// Strip the filename from a URL, keeping the trailing `/`. Used as the
/// base URL for resolving relative pack download paths.
fn base_url(url: &str) -> String {
    match url.rfind('/') {
        Some(i) => url[..=i].to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_strips_filename() {
        assert_eq!(
            base_url("https://packs.download.microchip.com/index.idx.gz"),
            "https://packs.download.microchip.com/"
        );
        assert_eq!(base_url("https://host/a/b/c.xml"), "https://host/a/b/");
        assert_eq!(base_url("no-slash"), "");
    }

    #[test]
    fn decode_pack_index_accepts_plain_xml() {
        let xml = b"<index/>";
        let out = decode_pack_index("https://host/index.xml", xml).expect("plain xml");
        assert_eq!(out, "<index/>");
    }

    #[test]
    fn decode_pack_index_gunzips_gz_suffix() {
        use flate2::{write::GzEncoder, Compression};
        use std::io::Write;

        let source = b"<index><pack/></index>";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(source).unwrap();
        let gzipped = encoder.finish().unwrap();

        let out =
            decode_pack_index("https://host/index.idx.gz", &gzipped).expect("gunzip by suffix");
        assert_eq!(out.as_bytes(), source);
    }

    #[test]
    fn decode_pack_index_gunzips_by_magic_without_suffix() {
        use flate2::{write::GzEncoder, Compression};
        use std::io::Write;

        let source = b"<x/>";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(source).unwrap();
        let gzipped = encoder.finish().unwrap();

        let out = decode_pack_index("https://host/index", &gzipped).expect("gunzip by magic bytes");
        assert_eq!(out.as_bytes(), source);
    }

    #[test]
    fn install_packs_noop_when_index_url_none() {
        let mut settings = settings::MplabSettings::load();
        settings.pack_index_url = None;
        let config = launch_config::LaunchConfig {
            request: Some("launch".into()),
            program: Some("foo.elf".into()),
            device: Some("ATSAMV71Q21B".into()),
            tool: Some("Simulator".into()),
            debug_server: None,
            debug_server_host: None,
        };
        // Must return without panicking and without hitting the network.
        install_packs(&settings, &config);
    }

    #[test]
    fn install_packs_noop_when_device_and_tool_empty() {
        let settings = settings::MplabSettings::load();
        let config = launch_config::LaunchConfig {
            request: Some("launch".into()),
            program: Some("foo.elf".into()),
            device: None,
            tool: None,
            debug_server: None,
            debug_server_host: None,
        };
        install_packs(&settings, &config);
    }
}
