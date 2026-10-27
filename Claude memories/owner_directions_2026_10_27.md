---
name: owner_directions_2026_10_27
description: "Owner's standing directions from the std-target day (2026-10-27): a windowing manager is needed (an app's op-22 frame interfered with the map's buffer), multiple arcs per process and growing one come later, every driver moves to userspace eventually, boot gets an attest screen (settings and capsules encrypted under the identity), HashMap only with proof it beats a linear lookup, Photon's AGENT.md applies to every repo (imported by ~/.claude/CLAUDE.md)"
metadata:
  type: project
---

Said while reviewing the first fluor frame on the phone and the std work:
- **Windowing manager**: the half-screen grey frame showed, "blitted oddly and interfered with the main buffer" — the app-surface/op 22 arrangement is not a compositor; one is coming. Design before building (the display server owns the panel; apps are places, SHELL.md).
- **Memory**: the 512 MiB big arc pool is fine for now; later a process gets several arcs and can grow one.
- **Drivers to userspace**: all of them, eventually; keep kernel-side driver code movable.
- **Attest at boot**: a nice attest screen first thing; user settings and probably capsules are encrypted under that identity (the ira unlocks drivers and key apps, the identity unlocks Photon, Lumis settings and the rest: a lock screen before anything works). "However you want to sort that out."
- **HashMap**: fine when it is faster AND better for the case, with proof; otherwise a linear scan or a plain lookup (Photon's AGENT.md).
- **Rules everywhere**: Photon's AGENT.md holds the general rules; `~/.claude/CLAUDE.md` imports it so every repo's session reads it first.

**How to apply:** no app draws over the map until the windowing design is agreed; plan Photon's memory on arcs, not a special heap; put an attest screen in the boot path before Photon ships; read Photon's AGENT.md as the house style for every repo.
