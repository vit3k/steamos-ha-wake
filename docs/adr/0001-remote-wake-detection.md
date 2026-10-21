# 1. Detect Local Wake by physical input, not by wake source

## Status

Accepted. Supersedes the first version of this ADR (NIC wake source plus a punktfunk grace window).

## Context

The Wake Trigger must fire on a Local Wake and never on a Remote Wake. When unsure, it must not fire.

The first design tried to identify a Remote Wake from the kernel's wake-source accounting (the network card's `wakeup_count`). On the target box this is useless. It sleeps in ACPI S3 (`mem_sleep = deep`), where the firmware handles the wake and the kernel credits no device: after a confirmed Wake-on-LAN resume every `wakeup_count` was still 0, and the only counters that moved (USB controller, `gpe6D`) move on every resume. A Remote Wake can't be told from a Local one by how the machine woke.

## Decision

Look for Presence instead: someone physically at the machine.

1. After a Wake, wait up to 120 s for a press on a physical input device:
   - evdev devices not under `/devices/virtual/` (keyboard, mouse, power button), key presses only;
   - the Steam Controller (2026) through its Puck's hidraw nodes, because Steam owns the controller and its evdev devices stay silent. A press is a newly set bit in bytes 2–9 of input report `0x42`.
2. On the first press, if punktfunk reports an active session, skip. Otherwise fire.
3. No press within the window: no fire.

Virtual devices (punktfunk's and Steam Input's virtual pads) never count, so remote input can't fake Presence.

## Consequences

- Every kind of Remote Wake is covered (plain WoL, punktfunk, anything else) without depending on how the machine woke.
- A Local Wake fires on the first press, not after a fixed delay.
- Waking the box and touching nothing never fires. Accepted: a missed fire beats a false one.
- The Steam Controller report layout is reverse-engineered from one capture; a firmware change could move the button bits.
- Rejected: NIC wake source (no attribution in S3); punktfunk-only (plain WoL fires); switching to s2idle (system change, higher sleep power, attribution still unproven).
