---
name: Seed SD fallback — must implement and test
description: SD card fallback in ferros_seed boot path — UFS-primary with SD confirmation at P+1, not yet implemented
type: project
---

Seed SD fallback is designed but not yet implemented. MUST be done before production.

**Design (agreed):**
1. Binary search UFS kernel ring (8 reads)
2. Read SD at same position (1 read, confirmation)
3. Read SD at P+1 (1 read, check if SD is ahead)
4. If SD ahead → linear scan SD forward, boot from SD
5. If UFS corrupt → full SD binary search

**Blockers:**
- SD driver (`ferros_hal/src/sdmmc.rs`) uses `alloc::vec::Vec` and `ferros_vault::device::Device` trait
- Needs alloc-free cleanup for the seed (the Vec usage is trivial — two `Vec::new()` for DeviceInfo)
- SD init requires GCC clocks + RPMh LDO power — those modules are already alloc-free

**Why:** If UFS has bad sectors or stopped accepting writes, SD is the only recovery path after ABL loads the seed. Without this, a UFS failure beyond the boot partition = bricked device.

**How to apply:** Before any production deployment, implement and test the SD fallback path in `ferros_seed/src/main.rs`. Test scenarios: UFS-only, SD-only, UFS-behind-SD, both-corrupt.
