//! Wake-on-LAN detection through the kernel's per-device wakeup accounting.

use std::fs;
use std::path::PathBuf;

/// Path to the `wakeup_count` of the interface's underlying device.
fn count_path(iface: &str) -> PathBuf {
    PathBuf::from(format!("/sys/class/net/{iface}/device/power/wakeup_count"))
}

/// Physical interfaces that can wake the system (have wakeup accounting and are enabled).
pub fn wake_capable() -> Vec<String> {
    let Ok(dir) = fs::read_dir("/sys/class/net") else { return vec![] };
    let mut out: Vec<String> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            let wakeup = format!("/sys/class/net/{name}/device/power/wakeup");
            count_path(name).exists()
                && fs::read_to_string(wakeup).is_ok_and(|s| s.trim() == "enabled")
        })
        .collect();
    out.sort();
    out
}

/// Interfaces to watch: the configured one, or every wake-capable one.
pub fn watched(configured: Option<&str>) -> Vec<String> {
    match configured {
        Some(i) => vec![i.to_string()],
        None => wake_capable(),
    }
}

pub fn wakeup_count(iface: &str) -> Option<u64> {
    fs::read_to_string(count_path(iface)).ok()?.trim().parse().ok()
}

/// Snapshot of wakeup counts taken just before suspend.
#[derive(Debug, Clone)]
pub struct Snapshot(Vec<(String, Option<u64>)>);

impl Snapshot {
    pub fn take(ifaces: &[String]) -> Self {
        Snapshot(ifaces.iter().map(|i| (i.clone(), wakeup_count(i))).collect())
    }

    /// The first interface whose wakeup count rose since the snapshot.
    pub fn woke_by(&self) -> Option<&str> {
        self.0.iter().find_map(|(iface, before)| {
            let after = wakeup_count(iface)?;
            (after > (*before)?).then_some(iface.as_str())
        })
    }
}
