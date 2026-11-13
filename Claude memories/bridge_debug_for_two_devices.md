---
name: bridge-debug-for-two-devices
description: With two ferros devices plugged in use target/debug/ferros-bridge; the release bridge (Nov 4) ignores FERROS_BUS and hits whichever device enumerates first
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-10T21:25:48.387Z
---

2026-11-10: `target/release/ferros-bridge` was built 2026-11-04, before `FERROS_BUS` existed (tools/ferros-bridge/src/usb.rs). With the Pro and the 8a both on USB it silently talked to the first device found, so a "store on both" went to one device twice.

**How to apply:** use `B=target/debug/ferros-bridge` for any `FERROS_BUS=<n>` command. Check which device a bus is with `diag log` (boot number and uptime) before trusting it, since bus numbers move when a cable is replugged. Related: [[feedback-usb-port-wedge]].
