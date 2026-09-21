//! Independent, restart-only inbound MCP configuration. No implicit capabilities.
use crate::common::{
    config::AppConfig,
    errors::{HarnessError, Result},
};
use std::{collections::BTreeSet, net::IpAddr, path::Path};

pub const TOOLS: [&str; 3] = ["memory_list_facts", "memory_get_fact", "memory_search"];

#[derive(Clone)]
pub struct Settings {
    pub host: IpAddr,
    pub port: u16,
    pub tools: BTreeSet<String>,
    pub max_keys: usize,
    pub max_request_bytes: usize,
    pub max_result_bytes: usize,
    pub max_results: usize,
    pub max_query_chars: usize,
    pub request_timeout_sec: u64,
    pub concurrency: usize,
    pub requests_per_minute: usize,
    pub global_requests_per_minute: usize,
}

pub(super) fn invalid() -> HarnessError {
    HarnessError::new("MCP_SERVER_CONFIG_INVALID", "invalid private MCP server configuration")
}

impl Settings {
    pub fn load(cfg: &AppConfig) -> Result<Self> {
        let defaults = AppConfig::from_str(AppConfig::embedded_default(), Path::new("defaults"))?;
        let value = |name: &str| cfg.get(name).or_else(|| defaults.get(name));
        let number = |name: &str, min: u64, max: u64| -> Result<u64> {
            value(name)
                .and_then(|v| v.as_u64())
                .filter(|v| (min..=max).contains(v))
                .ok_or_else(invalid)
        };
        let host: IpAddr = value("mcp.server.host")
            .and_then(|v| v.as_str())
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        if !host.is_loopback() {
            return Err(invalid());
        }
        let items = value("mcp.server.tools")
            .and_then(|v| v.as_sequence())
            .ok_or_else(invalid)?;
        let mut tools = BTreeSet::new();
        for item in items {
            let name = item.as_str().filter(|s| TOOLS.contains(s)).ok_or_else(invalid)?;
            if !tools.insert(name.to_owned()) {
                return Err(invalid());
            }
        }
        Ok(Self {
            host,
            tools,
            port: number("mcp.server.port", 1024, 65535)? as u16,
            max_keys: number("mcp.server.max_keys", 1, 32)? as usize,
            max_request_bytes: number("mcp.server.max_request_bytes", 1024, 65536)? as usize,
            max_result_bytes: number("mcp.server.max_result_bytes", 1024, 262144)? as usize,
            max_results: number("mcp.server.max_results", 1, 64)? as usize,
            max_query_chars: number("mcp.server.max_query_chars", 1, 512)? as usize,
            request_timeout_sec: number("mcp.server.request_timeout_sec", 1, 30)?,
            concurrency: number("mcp.server.concurrency", 1, 8)? as usize,
            requests_per_minute: number("mcp.server.requests_per_minute", 1, 120)? as usize,
            global_requests_per_minute: number("mcp.server.global_requests_per_minute", 1, 600)? as usize,
        })
    }
}
