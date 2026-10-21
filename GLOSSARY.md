# Glossary

**Wake**
The machine coming up from off (boot) or from suspend. Screen-off or idle dimming while the system stays up is not a Wake.

**Local Wake**
A Wake started by someone physically at the machine: power button, controller, keyboard. A boot from fully off is always a Local Wake: the machine is only ever woken remotely from suspend.

**Remote Wake**
A Wake started from elsewhere: a Wake-on-LAN packet, a streaming client (e.g. punktfunk) connecting, or any other network-originated start.

**Wake Trigger**
The Home Assistant automation that turns on living-room media gear. Fired on a Local Wake and on any press of the physical Steam button, at most once a minute. Never fired on a Remote Wake; when the kind of Wake is uncertain, it is not fired.

**Presence**
Evidence that someone is physically at the machine: a press on a real keyboard, mouse or controller, or the controller being turned on. Input arriving over a streaming session is not Presence. A Wake followed by Presence is treated as a Local Wake.
