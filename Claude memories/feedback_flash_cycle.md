---
name: Flash cycle workflow
description: How to build, flash, and pull logs on FP5 — timing and commands
type: feedback
---

The user manually enters fastboot mode (vol-down + power). Once in fastboot:

1. `fastboot flash boot_a ferros.img && fastboot set_active a && fastboot reboot`
2. Wait 18 seconds (10s nag screen + 2s pre-boot + 5s USB enum + 1s margin)
3. `cargo run -p ferros-bridge -- diag` to pull boot log

Recovery (back to Android): `fastboot set_active b && fastboot reboot`

**Why:** Reboot-to-fastboot from ferros is not working (SPMI arbiter blocks all PMIC reboot-reason registers). The user is the "human USB-to-fastboot adapter" and finds this tedious.
**How to apply:** Always build+mkimg before asking the user to enter fastboot. Batch changes to minimize flash cycles. Use `ferros-bridge diag | grep` to filter relevant output.
