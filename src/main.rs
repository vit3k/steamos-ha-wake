//! Fires a Home Assistant webhook (the Wake Trigger) on a Local Wake: a boot, or a resume
//! from suspend that wasn't caused by Wake-on-LAN or a punktfunk streaming client.
//! See GLOSSARY.md and docs/adr/0001-remote-wake-detection.md.

mod config;
mod nic;
mod punktfunk;

use config::Config;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedFd;

#[derive(Debug, Clone, Copy)]
enum WakeKind {
    Boot,
    Resume,
}

impl WakeKind {
    fn as_str(self) -> &'static str {
        match self {
            WakeKind::Boot => "boot",
            WakeKind::Resume => "resume",
        }
    }
}

macro_rules! log {
    ($($t:tt)*) => { eprintln!($($t)*) };
}

fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "run".into());
    let result = match cmd.as_str() {
        "run" => run(),
        "probe" => probe(),
        _ => Err(format!("usage: steamos-ha-wake [run|probe]\nunknown command: {cmd}")),
    };
    if let Err(e) = result {
        log!("error: {e}");
        std::process::exit(1);
    }
}

/// Print what the daemon would see, for checking a new machine.
fn probe() -> Result<(), String> {
    println!("config: {}", config::path().display());
    let cfg = config::load();
    match &cfg {
        Ok(c) => println!("  ok: {c:?}"),
        Err(e) => println!("  error: {e}"),
    }
    let configured = cfg.as_ref().ok().and_then(|c| c.interface.as_deref());
    println!("wake-capable interfaces: {:?}", nic::wake_capable());
    for i in nic::watched(configured) {
        println!("  {i}: wakeup_count = {:?}", nic::wakeup_count(&i));
    }
    match std::fs::read_to_string("/sys/power/pm_wakeup_irq") {
        Ok(irq) => println!("last wakeup irq: {}", irq.trim()),
        Err(e) => println!("last wakeup irq: unavailable ({e})"),
    }
    match punktfunk::Client::new().active_sessions() {
        Ok(n) => println!("punktfunk active_sessions: {n}"),
        Err(e) => println!("punktfunk: unreachable ({e})"),
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let cfg = Arc::new(config::load()?);
    let ifaces = nic::watched(cfg.interface.as_deref());
    log!("watching wake sources: {ifaces:?}");
    if ifaces.is_empty() {
        log!("warning: no wake-capable interface; Wake-on-LAN can't be detected, only punktfunk sessions");
    }

    let conn = Connection::system().map_err(|e| format!("system bus: {e}"))?;
    let logind = Proxy::new(
        &conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .map_err(|e| format!("logind: {e}"))?;
    // Subscribe before handling boot so no suspend is missed.
    let signals = logind
        .receive_signal("PrepareForSleep")
        .map_err(|e| format!("subscribe PrepareForSleep: {e}"))?;

    // Bumped on every suspend so an in-flight wake handler stops.
    let generation = Arc::new(AtomicU64::new(0));

    if first_start_this_boot() {
        spawn_handler(WakeKind::Boot, None, cfg.clone(), generation.clone());
    } else {
        log!("already handled this boot; not firing (daemon restart or re-login)");
    }

    let mut inhibitor = inhibit(&logind);
    let mut snapshot = None;
    for msg in signals {
        let going_down: bool = match msg.body().deserialize() {
            Ok(b) => b,
            Err(e) => {
                log!("bad PrepareForSleep signal: {e}");
                continue;
            }
        };
        if going_down {
            generation.fetch_add(1, Ordering::SeqCst);
            snapshot = Some(nic::Snapshot::take(&ifaces));
            log!("suspending; snapshot {snapshot:?}");
            // Snapshot taken: let the suspend proceed.
            drop(inhibitor.take());
        } else {
            log!("resumed");
            spawn_handler(WakeKind::Resume, snapshot.take(), cfg.clone(), generation.clone());
            inhibitor = inhibit(&logind);
        }
    }
    Err("PrepareForSleep signal stream ended".into())
}

/// Takes a logind delay inhibitor so the pre-suspend snapshot happens before the system sleeps.
fn inhibit(logind: &Proxy) -> Option<OwnedFd> {
    logind
        .call("Inhibit", &("sleep", "steamos-ha-wake", "Record wake sources before suspend", "delay"))
        .inspect_err(|e| log!("warning: no sleep inhibitor ({e}); wake source snapshot may race suspend"))
        .ok()
}

/// True once per kernel boot, so a daemon restart or re-login doesn't count as a Wake.
fn first_start_this_boot() -> bool {
    let Ok(boot_id) = std::fs::read_to_string("/proc/sys/kernel/random/boot_id") else {
        return true;
    };
    let dir = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config::home().join(".local/state"))
        .join("steamos-ha-wake");
    let file = dir.join("boot_id");
    if std::fs::read_to_string(&file).is_ok_and(|s| s == boot_id) {
        return false;
    }
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, &boot_id)) {
        log!("warning: can't record boot id ({e})");
    }
    true
}

fn spawn_handler(kind: WakeKind, snapshot: Option<nic::Snapshot>, cfg: Arc<Config>, generation: Arc<AtomicU64>) {
    let my_gen = generation.load(Ordering::SeqCst);
    thread::spawn(move || {
        let current = || generation.load(Ordering::SeqCst) == my_gen;
        handle_wake(kind, snapshot, &cfg, &current);
    });
}

fn handle_wake(kind: WakeKind, snapshot: Option<nic::Snapshot>, cfg: &Config, current: &dyn Fn() -> bool) {
    let kind_s = kind.as_str();

    if let WakeKind::Resume = kind {
        match &snapshot {
            Some(s) => {
                if let Some(iface) = s.woke_by() {
                    log!("{kind_s}: Remote Wake (Wake-on-LAN via {iface}); not firing");
                    return;
                }
            }
            None => log!("{kind_s}: no pre-suspend snapshot; skipping Wake-on-LAN check"),
        }
    }

    // Grace window: a streaming client connecting now means this was a Remote Wake.
    let pf = punktfunk::Client::new();
    let deadline = Instant::now() + cfg.grace;
    let mut last_err = None;
    while Instant::now() < deadline {
        if !current() {
            log!("{kind_s}: suspended again during grace window; abandoning");
            return;
        }
        match pf.active_sessions() {
            Ok(0) => last_err = None,
            Ok(n) => {
                log!("{kind_s}: Remote Wake (punktfunk has {n} active session(s)); not firing");
                return;
            }
            Err(e) => last_err = Some(e),
        }
        thread::sleep(Duration::from_secs(1));
    }
    if let Some(e) = last_err {
        log!("{kind_s}: punktfunk unreachable at end of grace window ({e}); treating as no session");
    }

    log!("{kind_s}: Local Wake; firing Wake Trigger");
    fire(kind, cfg, current);
}

/// POSTs the webhook, retrying with backoff until `cfg.retry` runs out.
fn fire(kind: WakeKind, cfg: &Config, current: &dyn Fn() -> bool) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();
    let deadline = Instant::now() + cfg.retry;
    let mut backoff = Duration::from_secs(1);
    loop {
        if !current() {
            log!("suspended again before webhook succeeded; abandoning");
            return;
        }
        match agent.post(&cfg.webhook_url).send_json(serde_json::json!({ "wake": kind.as_str() })) {
            Ok(_) => {
                log!("Wake Trigger fired");
                return;
            }
            Err(e) if Instant::now() + backoff < deadline => {
                log!("webhook failed ({e}); retrying in {backoff:?}");
                thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(8));
            }
            Err(e) => {
                log!("webhook failed ({e}); giving up");
                return;
            }
        }
    }
}
