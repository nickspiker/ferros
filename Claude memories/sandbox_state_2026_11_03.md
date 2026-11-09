---
name: sandbox_state_2026_11_03
description: "Where sandboxing stands after 2026-11-03 — three modes (fixed/ring/exact) built and verified on husky, BTI, handles, which processes run masked, FERROS_RESCUE, the vsf signature hole, and the traps (survey grep, diag quoting, scrub ordering, kernel/vault drift, entry x26)"
metadata:
  node_type: memory
  type: project
  originSessionId: fef83e98-ba6e-4773-9615-f79c25605c30
  modified: 2026-11-03T21:34:25.472Z
---

Owner 2026-11-03: "keep sandboxing", then the three modes ("I say we build it"). Vocabulary in SANDBOX.md: capsule, native, masked, sealed, the sandbox, arc, grant/token/slot, handle, fixed/ring/exact.

**Modes (all built and verified on the device):**
- **fixed:** `bfi x26, xN, #0, #K`, K sealed in the capsule (`--fixed K`); kernel allocates exactly 2^K and starts x26 at the base (both the queued frame and `ferros_el0_enter`). touch runs from the vault in fixed mode (K = 17).
- **ring:** `and x26, xN, x27`; x27 a mask; the original; handles use it.
- **exact:** flag-free `sub x25, xA, x28 / sub x26, x25, x27 / orn x26, x25, x26 / orr x26, xA, x26, asr #63`; x27 = the image's need to 16 bytes; out of bounds → all ones → faults (TCR EPD1). Needs BTI (`BTI=1 EXACT=1 tools/ferros-masked-build.sh`, `bridge ... --bti --exact`). cmp/csinv where flags are dead is still to do.
- **BTI:** rustc `-Z branch-protection=bti`, emitter `ret`→`br x30` + `bti j` after calls, checker refuses `ret`, code on guarded pages (bit 50), SCTLR_EL2.BT0 at boot. Verified: a call past `bti c` → EC G#0D.
- Measured (static instructions): touch ring 2466 / fixed 2339 / BTI 2544 / exact 3056; ip ring 8307 / fixed 7449 / exact 11333.

**Masked from the vault:** init, touch (fixed), ahau, nfc, ip, sensors, bt, modem, pcie1; native: display, usb, map, photon. Handles: (base, mask) table in the first 512 bytes of the grants page (tokens at 0x204 + 32·slot), exact-or-nothing, 64-byte surface porch, 64 KiB kernel-only guard above masked arcs, no negative offsets.

**vsf signature hole (owner found 2026-11-03):** the kernel gate uses `vsf::verification::verify_file_signature`; with the broken vsf any signature (even zeros) passed for small capsules (native dev code exposed; masked still needed the device seal; the streamed big-capsule gate uses ed25519-dalek directly). Kernel on both slots rebuilt against the fix; bridge test switches FERROS_ZERO_SIG / FERROS_RANDOM_SIG / FERROS_TAMPER_CAPSULE all refused from flash.

**Traps:** a grep -c survey over a single file prints no filename (sum reads 0); `diag "kill x"` must be one quoted argument; never allocate after `scrub_memory` zeroes the heap; after a power-off the device boots the FLASHED kernel, so flash whenever the vault's capsules change the ABI (the porch mismatch blacked the attest screen); `FERROS_RESCUE=1` kernel mends usb/pcie1/bt from known-good copies. Android's deep-sleep hang under the hv FOUND 2026-11-03: gs_perf_mon fails its probe on the hv's core 3 and leaves a dangling CPU-hotplug callback, so every suspend's first CPU down spins in freed memory; the hv renames its compatible + node (HV.md); forced suspend verified; sleep then aborts at EL3/ACPM (MIF blocked) on USB, unplugged untested; suspend trail at G#FD60_0F40 survives a watchdog reset or the jump, NOT `adb reboot`. Related: [[pages_allowed_when_equivalent]], [[feedback_vsf_concurrent_edits]].

**Stale as of 2026-11-09:** the "Masked from the vault" list above no longer holds. Every process capsule read off the Pro's vault that day (init, touch, ahau, nfc, ip, sensors, bt, modem, pcie1 included) had one `aarch64` code section and no `masked` section: all native. Check a capsule's sections (`store get <n> <file>`, `vsfinfo`) before assuming a process runs sandboxed.
