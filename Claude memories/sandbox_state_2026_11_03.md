---
name: sandbox_state_2026_11_03
description: "Where sandboxing stands after 2026-11-03 — handles built and in the grants page, which processes run masked from the vault (init, touch, ahau, nfc, ip, sensors; hive/wintest on demand), which stay native and why, FERROS_RESCUE, and the traps (survey grep, diag quoting, scrub ordering)"
metadata:
  node_type: memory
  type: project
  originSessionId: fef83e98-ba6e-4773-9615-f79c25605c30
  modified: 2026-11-03T16:22:36.148Z
---

Owner 2026-11-03: "keep sandboxing" (vocabulary at the top of SANDBOX.md: capsule, native, masked, sealed, the sandbox, arc, grant/token/slot, handle, the gate/ihi).

**Built:** handles (a (base, mask) pair per grant in the first 512 bytes of the grants page, tokens at 0x204 + 32·slot, the page read-only at every level; checker forms ldp x23,x22,[x28,#16k] / and x21,xN,x22 / [x23,x21]; `ferros_sys::Handle<SLOT>`), both arc pools at 4 KiB pages (POOL_BIG ends at the secure carveout G#B600_0000), the masked link PIE with data page-aligned after text, the image header on hive/wintest, `diag arcs`.

**Masked and sealed in the vault, verified running:** init, touch, ahau (128 MiB arc, in-arc frame buffer, second surface for the display's labels through its own handle), nfc, ip, sensors (AoC IPC through a handle); hive and wintest spawned by hand. **Native:** display (25 pointer sites, 6 regions, NEON loops), modem, usb (12/4), pcie1 (9), bt (1), map (fluor's ferros host writes the surface by pointer), photon (std; its 100 MB capsule exceeds the vault's 3 MiB value limit, stored by the owner's path).

**Why:** a masked build of a driver that still holds raw pointers to a granted region dies at once (its accesses wrap into the arc). Masking usb that way killed the bridge; `FERROS_RESCUE=1` builds a kernel that mends the vault's usb/pcie1/bt with embedded native capsules at home start.

**How to apply:** migrate a driver by replacing its pointer accesses to granted regions with `Handle` calls (pointers INTO the arc are fine); build with `tools/ferros-masked-build.sh <name>` (it runs the checker), then `bridge capsule --masked --text-len N`, `store put <name>`, `bridge seal <bin> --text-len N`, `diag "restart <name>"` (diag subcommands with arguments must be ONE quoted argument: `diag "kill hive"`; `diag kill hive` does nothing). Survey pointer use with `cat payloads/<p>/src/*.rs | grep -c …` (grep -c on a single file prints no filename, so a per-file sum reads zero: the 2026-11-03 trap). Never allocate after `scrub_memory` zeroes the heap (it killed reboots to fastboot). Related: [[feedback_measure_before_fixing]], [[display_global_mask_then_layers]].
