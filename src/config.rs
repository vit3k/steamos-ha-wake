use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    webhook_url: String,
    suspend_webhook_url: Option<String>,
    presence_secs: Option<u64>,
    retry_secs: Option<u64>,
    dry_run: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub webhook_url: String,
    /// Called before every suspend, if set.
    pub suspend_webhook_url: Option<String>,
    /// How long after a Wake a physical press still counts as Presence.
    pub presence: Duration,
    /// How long to keep retrying the webhook before giving up.
    pub retry: Duration,
    /// Log instead of calling the webhook.
    pub dry_run: bool,
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
        suspend_webhook_url: f.suspend_webhook_url,
        presence: Duration::from_secs(f.presence_secs.unwrap_or(120)),
        retry: Duration::from_secs(f.retry_secs.unwrap_or(30)),
        dry_run: f.dry_run.unwrap_or(false),
    })
}
