//! Presence: presses on physical input devices.
//!
//! Watches evdev devices that aren't virtual, plus the Steam Controller's hidraw nodes
//! (Steam owns the controller, so its evdev devices stay silent). Virtual devices, such as
//! punktfunk's and Steam Input's pads, are ignored, so remote input never counts.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const EV_KEY: u16 = 1;
/// KEY_POWER, KEY_SLEEP, KEY_WAKEUP: power-state keys, not Presence. The ACPI power button
/// device emits KEY_WAKEUP on every S3 resume, Wake-on-LAN included.
const POWER_KEYS: [u16; 3] = [116, 142, 143];
/// `struct input_event` on 64-bit: timeval (16) + type (2) + code (2) + value (4).
const INPUT_EVENT_SIZE: usize = 24;
const VALVE_HID_ID: &str = "000028DE:";
/// Steam Controller (2026) state report. Bytes 2..6 are button and touch bits, 6..10 the
/// analog triggers (a pull sets new bits too), 10..18 the sticks as four little-endian i16.
const STEAM_STATE_REPORT: u8 = 0x42;
const STEAM_BUTTONS: std::ops::Range<usize> = 2..10;
const STEAM_STICKS: std::ops::Range<usize> = 10..18;
/// Idle stick noise stays under ~1200; a deliberate move reaches ±32767.
const STICK_THRESHOLD: i16 = 8000;
/// The Steam button: bit 0x01 of report byte 4.
const STEAM_BUTTON: (usize, u8) = (4, 0x01);
/// A powered-on controller streams ~260 state reports/s. It goes silent when the PC suspends
/// and stays silent after a Wake-on-LAN, apart from a burst of ~17 stale reports at resume.
/// Silence followed by a sustained stream means someone turned the controller on. The stream
/// must last a while: stale reports left in the hidraw buffer (up to 64) arrive all at once.
const CONTROLLER_SILENCE: Duration = Duration::from_secs(2);
const CONTROLLER_ON_REPORTS: u32 = 50;
const CONTROLLER_ON_LASTING: Duration = Duration::from_millis(500);
const CONTROLLER_ON_WITHIN: Duration = Duration::from_secs(2);

/// Time and source of the latest physical press, shared between reader threads and waiters.
pub struct Presence {
    last: Mutex<Option<(Instant, String)>>,
    cond: Condvar,
    /// Receives the source of each Steam button press.
    steam_button: Option<Sender<String>>,
}

impl Presence {
    /// Starts watching input devices in the background; new devices are picked up as they appear.
    pub fn watch(steam_button: Option<Sender<String>>) -> Arc<Self> {
        let p = Arc::new(Presence { last: Mutex::new(None), cond: Condvar::new(), steam_button });
        let p2 = p.clone();
        thread::spawn(move || scan_loop(p2));
        p
    }

    fn press(&self, source: String) {
        *self.last.lock().unwrap() = Some((Instant::now(), source));
        self.cond.notify_all();
    }

    fn steam_button(&self, source: String) {
        if let Some(tx) = &self.steam_button {
            let _ = tx.send(source);
        }
    }

    /// Waits until a press newer than `since`, up to `deadline`, polling `keep_going` every second.
    /// The press's source, if one arrived.
    pub fn wait_for_press(&self, since: Instant, deadline: Instant, keep_going: &dyn Fn() -> bool) -> Option<String> {
        let mut last = self.last.lock().unwrap();
        loop {
            if let Some((t, source)) = &*last
                && *t > since
            {
                return Some(source.clone());
            }
            let now = Instant::now();
            if now >= deadline || !keep_going() {
                return None;
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
                            Kind::Evdev => read_evdev(f, &dev, &p),
                            Kind::SteamController => read_steam(f, &dev, &p),
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

fn read_evdev(mut f: File, dev: &Path, p: &Presence) {
    let mut buf = [0u8; INPUT_EVENT_SIZE * 64];
    while let Ok(n) = f.read(&mut buf) {
        if n == 0 {
            return;
        }
        for ev in buf[..n].chunks_exact(INPUT_EVENT_SIZE) {
            let secs = i64::from_ne_bytes(ev[0..8].try_into().unwrap());
            let kind = u16::from_ne_bytes([ev[16], ev[17]]);
            let code = u16::from_ne_bytes([ev[18], ev[19]]);
            let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
            if kind == EV_KEY && value == 1 && !POWER_KEYS.contains(&code) {
                p.press(format!("{} key {code} (kernel time {secs})", dev.display()));
            }
        }
    }
}

/// Time since boot including suspend. `Instant` stops while suspended, which would hide the
/// controller's silence across a suspend.
pub fn boottime() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid pointer to a timespec; CLOCK_BOOTTIME always exists on Linux.
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

fn read_steam(mut f: File, dev: &Path, p: &Presence) {
    let mut buf = [0u8; 256];
    let mut prev = [0u8; STEAM_BUTTONS.end - STEAM_BUTTONS.start];
    let mut stick_was_out = false;
    let mut last_report = boottime();
    // Start and report count of a stream that began after silence.
    let mut waking: Option<(Duration, u32)> = None;
    while let Ok(n) = f.read(&mut buf) {
        if n == 0 {
            return;
        }
        let now = boottime();
        if now - last_report > CONTROLLER_SILENCE {
            waking = Some((now, 0));
        }
        last_report = now;
        if n < STEAM_STICKS.end || buf[0] != STEAM_STATE_REPORT {
            continue;
        }

        if let Some((start, count)) = &mut waking {
            *count += 1;
            if now - *start > CONTROLLER_ON_WITHIN {
                waking = None;
            } else if *count >= CONTROLLER_ON_REPORTS && now - *start >= CONTROLLER_ON_LASTING {
                // Presence only: moving an idle controller can also restart its stream, so this
                // is not proof of a Steam button press.
                p.press(format!("{} controller turned on", dev.display()));
                waking = None;
            }
        }

        let cur = &buf[STEAM_BUTTONS];
        if cur.iter().zip(prev).any(|(now, before)| now & !before != 0) {
            p.press(format!("{} buttons {:02x?} -> {:02x?}", dev.display(), prev, cur));
        }
        let (byte, bit) = STEAM_BUTTON;
        let steam_bit = |b: &[u8]| b[byte - STEAM_BUTTONS.start] & bit != 0;
        if steam_bit(cur) && !steam_bit(&prev) {
            p.steam_button(format!("{} Steam button", dev.display()));
        }
        prev.copy_from_slice(cur);

        let stick_out = buf[STEAM_STICKS]
            .chunks_exact(2)
            .any(|a| i16::from_le_bytes([a[0], a[1]]).saturating_abs() > STICK_THRESHOLD);
        if stick_out && !stick_was_out {
            p.press(format!("{} stick moved", dev.display()));
        }
        stick_was_out = stick_out;
    }
}
