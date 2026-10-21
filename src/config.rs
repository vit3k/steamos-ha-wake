use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    webhook_url: String,
    grace_secs: Option<u64>,
    retry_secs: Option<u64>,
    interface: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub webhook_url: String,
    /// How long to watch for a punktfunk session before firing.
    pub grace: Duration,
    /// How long to keep retrying the webhook before giving up.
    pub retry: Duration,
    /// Network interface whose wake source identifies a Wake-on-LAN. `None` = auto-detect.
    pub interface: Option<String>,
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME not set"))
}

pub fn path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("steamos-ha-wake/config.toml")
}

pub fn load() -> Result<Config, String> {
    let path = path();
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let f: File = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Config {
        webhook_url: f.webhook_url,
        grace: Duration::from_secs(f.grace_secs.unwrap_or(20)),
        retry: Duration::from_secs(f.retry_secs.unwrap_or(30)),
        interface: f.interface,
    })
}
