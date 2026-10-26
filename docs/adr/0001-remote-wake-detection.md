# 1. Detect Local Wake by physical input, not by wake source

## Status

Accepted. Supersedes the first version of this ADR (NIC wake source plus a punktfunk grace window).

## Context

The Wake Trigger must fire on a Local Wake and never on a Remote Wake. When unsure, it must not fire.

The first design tried to identify a Remote Wake from the kernel's wake-source accounting (the network card's `wakeup_count`). On the target box this is useless. It sleeps in ACPI S3 (`mem_sleep = deep`), where the firmware handles the wake and the kernel credits no device: after a confirmed Wake-on-LAN resume every `wakeup_count` was still 0, and the only counters that moved (USB controller, `gpe6D`) move on every resume. A Remote Wake can't be told from a Local one by how the machine woke.

## Decision

Look for Presence instead: someone physically at the machine.

1. After a Wake, wait up to 120 s for input from a physical device:
   - evdev devices not under `/devices/virtual/` (keyboard, mouse), key presses only. Power-state keys (`KEY_POWER`, `KEY_SLEEP`, `KEY_WAKEUP`) don't count: the ACPI power button device emits `KEY_WAKEUP` on every S3 resume, Wake-on-LAN included.
   - the Steam Controller (2026) through its Puck's hidraw nodes, because Steam owns the controller and its evdev devices stay silent. Report `0x42`: a newly set bit in bytes 2–9 (buttons, touch, triggers) or a stick beyond ±8000.
   - the Steam Controller turning on. The Steam-button press that wakes the PC happens while it sleeps and is never seen, but the controller then reconnects and streams ~260 reports/s at once. After a Wake-on-LAN it stays silent, apart from a burst of stale buffered reports. So: ≥2 s of silence (measured on `CLOCK_BOOTTIME`, which runs during suspend) followed by a stream of ≥50 reports lasting ≥500 ms counts.
2. On the first press, if punktfunk reports an active session, skip. Otherwise fire.
3. No press within the window: no fire.

Virtual devices (punktfunk's and Steam Input's virtual pads) never count, so remote input can't fake Presence.

## Consequences

- Every kind of Remote Wake is covered (plain WoL, punktfunk, anything else) without depending on how the machine woke.
- A Local Wake by the Steam Controller fires right at resume; by other devices, on the first press.
- Waking the box with the power button or keyboard and touching nothing never fires. Accepted: a missed fire beats a false one.
- Turning the controller on while the PC is already awake also counts as Presence, but only matters inside the window after a Wake. It is not treated as a Steam button press: an idle controller stops streaming after a while, and moving it (e.g. for gyro) restarts the stream the same way.
- The Steam Controller report layout is reverse-engineered from one capture; a firmware change could move the button bits.
- Rejected: NIC wake source (no attribution in S3); punktfunk-only (plain WoL fires); switching to s2idle (system change, higher sleep power, attribution still unproven).
