//! Presence: presses on physical input devices.
//!
//! Watches evdev devices that aren't virtual, plus the Steam Controller's hidraw nodes
//! (Steam owns the controller, so its evdev devices stay silent). Virtual devices, such as
//! punktfunk's and Steam Input's pads, are ignored, so remote input never counts.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const EV_KEY: u16 = 1;
/// `struct input_event` on 64-bit: timeval (16) + type (2) + code (2) + value (4).
const INPUT_EVENT_SIZE: usize = 24;
const VALVE_HID_ID: &str = "000028DE:";
/// Steam Controller (2026) state report; bytes 2..10 are the button bitfield.
const STEAM_STATE_REPORT: u8 = 0x42;
const STEAM_BUTTONS: std::ops::Range<usize> = 2..10;

/// Time of the latest physical press, shared between reader threads and waiters.
#[derive(Default)]
pub struct Presence {
    last: Mutex<Option<Instant>>,
    cond: Condvar,
}

impl Presence {
    /// Starts watching input devices in the background; new devices are picked up as they appear.
    pub fn watch() -> Arc<Self> {
        let p = Arc::new(Presence::default());
        let p2 = p.clone();
        thread::spawn(move || scan_loop(p2));
        p
    }

    fn press(&self) {
        *self.last.lock().unwrap() = Some(Instant::now());
        self.cond.notify_all();
    }

    /// Waits until a press newer than `since`, up to `deadline`, polling `keep_going` every second.
    /// True if a press arrived.
    pub fn wait_for_press(&self, since: Instant, deadline: Instant, keep_going: &dyn Fn() -> bool) -> bool {
        let mut last = self.last.lock().unwrap();
        loop {
            if last.is_some_and(|t| t > since) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline || !keep_going() {
                return false;
            }
            let wait = (deadline - now).min(Duration::from_secs(1));
            last = self.cond.wait_timeout(last, wait).unwrap().0;
        }
    }
}

fn scan_loop(p: Arc<Presence>) {
    let open: Arc<Mutex<HashSet<PathBuf>>> = Default::default();
    loop {
        for (dev, kind) in candidates() {
            if !open.lock().unwrap().insert(dev.clone()) {
                continue;
            }
            let (p, open) = (p.clone(), open.clone());
            thread::spawn(move || {
                match File::open(&dev) {
                    Ok(f) => {
                        crate::log!("watching {} ({kind:?})", dev.display());
                        match kind {
                            Kind::Evdev => read_evdev(f, &p),
                            Kind::SteamController => read_steam(f, &p),
                        }
                    }
                    Err(e) => crate::log!("can't open {}: {e}", dev.display()),
                }
                // Device gone (e.g. re-enumerated after resume); a later scan reopens it.
                open.lock().unwrap().remove(&dev);
            });
        }
        thread::sleep(Duration::from_secs(2));
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Evdev,
    SteamController,
}

pub fn candidates() -> Vec<(PathBuf, Kind)> {
    let mut out = vec![];
    for (class, dev_dir) in [("input", "/dev"), ("hidraw", "/dev")] {
        let Ok(dir) = fs::read_dir(format!("/sys/class/{class}")) else { continue };
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let sys = e.path();
            let kind = match class {
                "input" if name.starts_with("event") && is_physical_keyboardish(&sys) => Kind::Evdev,
                "hidraw" if is_valve(&sys) => Kind::SteamController,
                _ => continue,
            };
            let dev = Path::new(dev_dir).join(if class == "input" { format!("input/{name}") } else { name });
            out.push((dev, kind));
        }
    }
    out
}

/// Not virtual, and can emit at least one key or button.
fn is_physical_keyboardish(sys: &Path) -> bool {
    let real = fs::canonicalize(sys.join("device")).unwrap_or_default();
    if real.to_string_lossy().contains("/virtual/") {
        return false;
    }
    fs::read_to_string(sys.join("device/capabilities/key"))
        .is_ok_and(|caps| caps.split_whitespace().any(|w| w != "0"))
}

fn is_valve(sys: &Path) -> bool {
    fs::read_to_string(sys.join("device/uevent")).is_ok_and(|u| {
        u.lines().any(|l| l.strip_prefix("HID_ID=").is_some_and(|id| id.contains(VALVE_HID_ID)))
    })
}

fn read_evdev(mut f: File, p: &Presence) {
    let mut buf = [0u8; INPUT_EVENT_SIZE * 64];
    while let Ok(n) = f.read(&mut buf) {
        if n == 0 {
            return;
        }
        for ev in buf[..n].chunks_exact(INPUT_EVENT_SIZE) {
            let kind = u16::from_ne_bytes([ev[16], ev[17]]);
            let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
            if kind == EV_KEY && value == 1 {
                p.press();
            }
        }
    }
}

fn read_steam(mut f: File, p: &Presence) {
    let mut buf = [0u8; 256];
    let mut prev = [0u8; STEAM_BUTTONS.end - STEAM_BUTTONS.start];
    while let Ok(n) = f.read(&mut buf) {
        if n == 0 {
            return;
        }
        if n < STEAM_BUTTONS.end || buf[0] != STEAM_STATE_REPORT {
            continue;
        }
        let cur = &buf[STEAM_BUTTONS];
        if cur.iter().zip(prev).any(|(now, before)| now & !before != 0) {
            p.press();
        }
        prev.copy_from_slice(cur);
    }
}
