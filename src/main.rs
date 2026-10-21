//! Fires a Home Assistant webhook (the Wake Trigger) on a Local Wake: a boot or resume
//! followed by Presence (a press on a physical input device) while no punktfunk session runs.
//! See GLOSSARY.md and docs/adr/0001-remote-wake-detection.md.

macro_rules! log {
    ($($t:tt)*) => { eprintln!($($t)*) };
}
pub(crate) use log;

mod config;
mod presence;
mod punktfunk;

use config::Config;
use presence::Presence;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use zbus::blocking::{Connection, Proxy};

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

/// Print what the daemon would see, then report physical presses for 120 s.
fn probe() -> Result<(), String> {
    println!("config: {}", config::path().display());
    match config::load() {
        Ok(c) => println!("  ok: {c:?}"),
        Err(e) => println!("  error: {e}"),
    }
    println!("input devices counted as Presence:");
    for (dev, kind) in presence::candidates() {
        println!("  {} ({kind:?})", dev.display());
    }
    match punktfunk::Client::new().active_sessions() {
        Ok(n) => println!("punktfunk active_sessions: {n}"),
        Err(e) => println!("punktfunk: unreachable ({e})"),
    }
    println!("press something on a physical device within 120 s...");
    let p = Presence::watch();
    let start = Instant::now();
    if let Some(source) = p.wait_for_press(start, start + Duration::from_secs(120), &|| true) {
        println!("Presence detected after {:.1?}: {source}", start.elapsed());
    } else {
        println!("no Presence detected");
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let started = Instant::now();
    let cfg = Arc::new(config::load()?);
    let presence = Presence::watch();

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
    let spawn = |kind, since| {
        let (cfg, presence, generation) = (cfg.clone(), presence.clone(), generation.clone());
        let my_gen = generation.load(Ordering::SeqCst);
        thread::spawn(move || {
            let current = || generation.load(Ordering::SeqCst) == my_gen;
            handle_wake(kind, since, &cfg, &presence, &current);
        });
    };

    if first_start_this_boot() {
        spawn(WakeKind::Boot, started);
    } else {
        log!("already handled this boot; not firing (daemon restart or re-login)");
    }

    let mut suspended_at = started;
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
            suspended_at = Instant::now();
            log!("suspending");
        } else {
            log!("resumed");
            spawn(WakeKind::Resume, suspended_at);
        }
    }
    Err("PrepareForSleep signal stream ended".into())
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

/// `since`: presses before this instant (e.g. the one that put the box to sleep) don't count.
fn handle_wake(kind: WakeKind, since: Instant, cfg: &Config, presence: &Presence, current: &dyn Fn() -> bool) {
    let kind_s = kind.as_str();
    log!("{kind_s}: waiting up to {:?} for Presence", cfg.presence);
    let Some(source) = presence.wait_for_press(since, Instant::now() + cfg.presence, current) else {
        if current() {
            log!("{kind_s}: no Presence; treating as Remote Wake, not firing");
        } else {
            log!("{kind_s}: suspended again before Presence; abandoning");
        }
        return;
    };
    log!("{kind_s}: Presence from {source}");
    match punktfunk::Client::new().active_sessions() {
        Ok(0) => {}
        Ok(n) => {
            log!("{kind_s}: Presence, but punktfunk has {n} active session(s); not firing");
            return;
        }
        Err(e) => log!("{kind_s}: punktfunk unreachable ({e}); treating as no session"),
    }
    log!("{kind_s}: Local Wake; firing Wake Trigger");
    fire(kind, cfg, current);
}

/// POSTs the webhook, retrying with backoff until `cfg.retry` runs out.
fn fire(kind: WakeKind, cfg: &Config, current: &dyn Fn() -> bool) {
    if cfg.dry_run {
        log!("dry_run: would POST {}", cfg.webhook_url);
        return;
    }
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
