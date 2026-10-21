//! Asks the local punktfunk host whether a streaming client is present.

use crate::config::home;
use std::time::Duration;

const DEFAULT_URL: &str = "https://127.0.0.1:47990";

/// Reads `KEY=value` from a punktfunk state file.
fn read_kv(file: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(home().join(".config/punktfunk").join(file)).ok()?;
    text.lines()
        .find_map(|l| l.trim().strip_prefix(key)?.strip_prefix('='))
        .map(|v| v.trim().to_string())
}

/// Base URL of the management API, always ending in `/api/v1` and pointed at loopback.
fn api_base() -> String {
    let url = read_kv("mgmt-endpoint", "PUNKTFUNK_MGMT_URL").unwrap_or_else(|| DEFAULT_URL.into());
    // Admin routes are loopback-only; a wildcard bind address is reachable there too.
    let url = url.replace("://0.0.0.0", "://127.0.0.1").replace("://[::]", "://[::1]");
    let url = url.trim_end_matches('/');
    if url.ends_with("/api/v1") { url.to_string() } else { format!("{url}/api/v1") }
}

pub struct Client {
    agent: ureq::Agent,
}

impl Client {
    pub fn new() -> Self {
        // The host uses a self-signed certificate and we only ever talk to loopback.
        let tls = ureq::tls::TlsConfig::builder().disable_verification(true).build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(2)))
            .build()
            .into();
        Client { agent }
    }

    /// Number of active sessions, or an error if the API is unreachable or rejects us.
    pub fn active_sessions(&self) -> Result<u64, String> {
        let token = read_kv("mgmt-token", "PUNKTFUNK_MGMT_TOKEN").ok_or("no mgmt-token")?;
        let status: serde_json::Value = self
            .agent
            .get(format!("{}/status", api_base()))
            .header("Authorization", format!("Bearer {token}"))
            .call()
            .map_err(|e| e.to_string())?
            .body_mut()
            .read_json()
            .map_err(|e| e.to_string())?;
        status["active_sessions"].as_u64().ok_or_else(|| "no active_sessions in /status".into())
    }
}
