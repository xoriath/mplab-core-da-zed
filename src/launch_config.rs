//! Debug launch configuration parsing and RCP argv construction.

use crate::settings::MplabSettings;
use serde::Deserialize;
use zed_extension_api as zed;
use zed_extension_api::{
    StartDebuggingRequestArgumentsRequest, TcpArguments, TcpArgumentsTemplate,
};

pub(crate) const ADAPTER_NAME: &str = "mplab";
pub(crate) const DEFAULT_DEBUG_HOST: &str = "127.0.0.1";
pub(crate) const DEFAULT_JRE_NAME: &str = "zulu-jre-25";
pub(crate) const DEFAULT_LOG_LEVEL: &str = "900";
pub(crate) const DAP_CONNECT_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LaunchConfig {
    pub request: Option<String>,
    pub program: Option<String>,
    pub device: Option<String>,
    pub tool: Option<String>,
    pub debug_server: Option<DebugServer>,
    pub debug_server_host: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum DebugServer {
    Port(u16),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalDap {
    pub host: u32,
    pub port: u16,
}

pub(crate) fn parse_config(json: &str) -> Result<LaunchConfig, String> {
    serde_json::from_str(json).map_err(|e| format!("failed to parse MPLAB debug config: {e}"))
}

pub(crate) fn request_kind_from_value(
    config: &serde_json::Value,
) -> Result<StartDebuggingRequestArgumentsRequest, String> {
    match config.get("request").and_then(|v| v.as_str()) {
        Some("launch") => Ok(StartDebuggingRequestArgumentsRequest::Launch),
        Some("attach") => Ok(StartDebuggingRequestArgumentsRequest::Attach),
        Some(other) => Err(format!(
            "unsupported MPLAB debug request '{other}'; expected 'launch' or 'attach'"
        )),
        None => Err("MPLAB debug config must include request: 'launch' or 'attach'".to_string()),
    }
}

pub(crate) fn request_kind(
    config: &LaunchConfig,
) -> Result<StartDebuggingRequestArgumentsRequest, String> {
    match config.request.as_deref() {
        Some("launch") => Ok(StartDebuggingRequestArgumentsRequest::Launch),
        Some("attach") => Ok(StartDebuggingRequestArgumentsRequest::Attach),
        Some(other) => Err(format!(
            "unsupported MPLAB debug request '{other}'; expected 'launch' or 'attach'"
        )),
        None => Err("MPLAB debug config must include request: 'launch' or 'attach'".to_string()),
    }
}

pub(crate) fn validate_for_session(config: &LaunchConfig) -> Result<(), String> {
    let request = request_kind(config)?;

    require_non_empty(config.device.as_deref(), "device")?;
    require_non_empty(config.tool.as_deref(), "tool")?;

    if matches!(request, StartDebuggingRequestArgumentsRequest::Launch)
        && config.program.as_deref().is_none_or(str::is_empty)
    {
        // Some MPLAB flows can resolve the program from `project` + `configuration`,
        // so this is intentionally not fatal yet. The backend gives the final error.
    }

    Ok(())
}

pub(crate) fn external_dap(config: &LaunchConfig) -> Result<Option<ExternalDap>, String> {
    let Some(server) = &config.debug_server else {
        return Ok(None);
    };

    let default_host = config
        .debug_server_host
        .as_deref()
        .unwrap_or(DEFAULT_DEBUG_HOST);
    match server {
        DebugServer::Port(port) => Ok(Some(ExternalDap {
            host: parse_ipv4(default_host)?,
            port: *port,
        })),
        DebugServer::String(value) => parse_debug_server_string(value, default_host).map(Some),
    }
}

pub(crate) fn tcp_args(host: u32, port: u16) -> TcpArguments {
    TcpArguments {
        port,
        host,
        timeout: Some(DAP_CONNECT_TIMEOUT_MS),
    }
}

pub(crate) fn resolve_tcp() -> Result<TcpArguments, String> {
    let mut tcp = zed::resolve_tcp_template(TcpArgumentsTemplate {
        port: None,
        host: None,
        timeout: Some(DAP_CONNECT_TIMEOUT_MS),
    })?;
    tcp.timeout = Some(DAP_CONNECT_TIMEOUT_MS);
    Ok(tcp)
}

pub(crate) fn rcp_arguments(
    java_home: &str,
    dap_port: u16,
    settings: &MplabSettings,
) -> Vec<String> {
    let mut args = vec![
        "--jdkhome".to_string(),
        java_home.to_string(),
        // Suppress the NetBeans launcher's console reuse. Without this, if
        // Zed spawns us with an inherited console, the launcher prints
        //   "The launcher has determined that the parent process has a
        //    console... Use '--console suppress' to suppress console output."
        // and exits before the DAP handshake completes.
        "--console".to_string(),
        "suppress".to_string(),
        "--nosplash".to_string(),
        "--nogui".to_string(),
        format!("-J-Ddebug.adapter.protocol.server.port={dap_port}"),
        "-J-Dmicrochip.connect=false".to_string(),
        format!(
            "-J-Dload.elf.symbols.on.demand={}",
            settings.symbol_loading_value()
        ),
        "--userdir".to_string(),
        settings.user_dir.clone(),
        "--cachedir".to_string(),
        settings.cache_dir.clone(),
        format!("-J-Dpackslib.packsfolder={}", settings.pack_repo),
        format!("-J-D.level={}", settings.jul_log_level()),
    ];
    args.extend(settings.extra_args.iter().cloned());
    args
}

fn parse_debug_server_string(value: &str, default_host: &str) -> Result<ExternalDap, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("debugServer must not be empty".to_string());
    }

    if let Ok(port) = value.parse::<u16>() {
        return Ok(ExternalDap {
            host: parse_ipv4(default_host)?,
            port,
        });
    }

    let (host, port) = value
        .rsplit_once(':')
        .ok_or_else(|| format!("debugServer '{value}' must be a TCP port or host:port"))?;
    let port = port
        .parse::<u16>()
        .map_err(|e| format!("invalid debugServer port in '{value}': {e}"))?;

    Ok(ExternalDap {
        host: parse_ipv4(host)?,
        port,
    })
}

fn parse_ipv4(host: &str) -> Result<u32, String> {
    let host = host.trim();
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(ipv4_to_u32([127, 0, 0, 1]));
    }

    let mut octets = [0_u8; 4];
    let mut count = 0;
    for part in host.split('.') {
        if count == 4 {
            return Err(format!("host '{host}' is not an IPv4 address"));
        }
        octets[count] = part
            .parse::<u8>()
            .map_err(|e| format!("host '{host}' is not an IPv4 address: {e}"))?;
        count += 1;
    }

    if count != 4 {
        return Err(format!("host '{host}' is not an IPv4 address"));
    }

    Ok(ipv4_to_u32(octets))
}

fn ipv4_to_u32(octets: [u8; 4]) -> u32 {
    u32::from_be_bytes(octets)
}

fn require_non_empty(value: Option<&str>, field: &str) -> Result<(), String> {
    match value.map(str::trim) {
        Some(value) if !value.is_empty() => Ok(()),
        _ => Err(format!(
            "MPLAB debug config must include non-empty '{field}'"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_request_kind() {
        let cfg = parse_config(r#"{"request":"launch"}"#).unwrap();
        assert_eq!(
            request_kind(&cfg).unwrap(),
            StartDebuggingRequestArgumentsRequest::Launch
        );
    }

    #[test]
    fn parses_external_port() {
        let cfg = parse_config(r#"{"request":"launch","debugServer":4712}"#).unwrap();
        let external = external_dap(&cfg).unwrap().unwrap();
        assert_eq!(external.port, 4712);
        assert_eq!(external.host, ipv4_to_u32([127, 0, 0, 1]));
    }

    #[test]
    fn parses_external_host_port() {
        let cfg =
            parse_config(r#"{"request":"launch","debugServer":"192.168.1.10:5555"}"#).unwrap();
        let external = external_dap(&cfg).unwrap().unwrap();
        assert_eq!(external.port, 5555);
        assert_eq!(external.host, ipv4_to_u32([192, 168, 1, 10]));
    }

    #[test]
    fn uses_debug_server_host_for_port_only_string() {
        let cfg = parse_config(
            r#"{"request":"launch","debugServer":"4712","debugServerHost":"10.0.0.2"}"#,
        )
        .unwrap();
        let external = external_dap(&cfg).unwrap().unwrap();
        assert_eq!(external.port, 4712);
        assert_eq!(external.host, ipv4_to_u32([10, 0, 0, 2]));
    }

    #[test]
    fn builds_rcp_arguments() {
        let args = rcp_arguments("apps/zulu-jre-25/25.0.1", 53123, &MplabSettings::default());
        assert!(args.contains(&"--jdkhome".to_string()));
        assert!(args.contains(&"apps/zulu-jre-25/25.0.1".to_string()));
        assert!(args.contains(&"-J-Ddebug.adapter.protocol.server.port=53123".to_string()));
        assert!(args.contains(&"-J-Dpackslib.packsfolder=packs".to_string()));
    }

    #[test]
    fn rcp_arguments_suppress_console_pairing() {
        // `--console suppress` must be passed as two adjacent argv
        // entries, in that order, so the NetBeans launcher doesn't reuse
        // Zed's inherited console and exit before the DAP handshake.
        let args = rcp_arguments("jh", 1, &MplabSettings::default());
        let idx = args
            .iter()
            .position(|a| a == "--console")
            .expect("--console arg present");
        assert_eq!(args[idx + 1], "suppress");
    }

    #[test]
    fn rcp_arguments_apply_settings() {
        let settings = MplabSettings {
            user_dir: "custom-user".into(),
            cache_dir: "custom-cache".into(),
            pack_repo: "custom-packs".into(),
            log_level: "INFO".into(),
            symbol_loading: crate::settings::SymbolLoading::PreProcessed,
            extra_args: vec!["-J-Dcustom=true".into()],
            ..MplabSettings::default()
        };
        let args = rcp_arguments("java-home", 1234, &settings);
        assert!(args.contains(&"custom-user".to_string()));
        assert!(args.contains(&"custom-cache".to_string()));
        assert!(args.contains(&"-J-Dpackslib.packsfolder=custom-packs".to_string()));
        assert!(args.contains(&"-J-D.level=800".to_string()));
        assert!(args.contains(&"-J-Dload.elf.symbols.on.demand=false".to_string()));
        assert!(args.contains(&"-J-Dcustom=true".to_string()));
    }

    #[test]
    fn validates_required_device_and_tool() {
        let cfg = parse_config(r#"{"request":"launch","device":"PIC18F47Q10","tool":"Simulator"}"#)
            .unwrap();
        validate_for_session(&cfg).unwrap();

        let missing = parse_config(r#"{"request":"launch","tool":"Simulator"}"#).unwrap();
        assert!(validate_for_session(&missing)
            .unwrap_err()
            .contains("device"));
    }
}
