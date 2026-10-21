# 1. Detect Remote Wake by NIC wake source plus a streaming grace window

## Status

Accepted

## Context

The Wake Trigger must fire on a Local Wake and never on a Remote Wake. When unsure, it must not fire. A Remote Wake comes in two forms: a plain Wake-on-LAN packet (from HA, a phone app, etc.) and a punktfunk client, which sends WoL every 6 s for up to 90 s and then connects. Kernel wake-source accounting (`wakeup_count` per device) can identify a NIC wake, but on AMD s2idle several sources can share one IRQ, so it may be unreliable.

## Decision

Two checks, both must pass to fire:

1. After resume, if the network card's `wakeup_count` increased across the suspend, treat it as a Remote Wake and skip.
2. Otherwise wait a grace window (20 s), polling the punktfunk management API (`GET /api/v1/status`). If `active_sessions > 0` at any point, skip. Else fire. If the API is unreachable for the whole window, fire: no client could have connected.

A boot from fully off skips check 1 because the machine is never woken remotely from off.

## Consequences

- A Local Wake fires late, by the length of the grace window.
- A Local Wake followed right away by a punktfunk connection from another device is treated as remote. This is acceptable, since a missed fire is better than a false one.
- Alternatives rejected: grace window only (a plain WoL would fire); NIC check only (unreliable IRQ attribution, and blind to some streaming starts).
