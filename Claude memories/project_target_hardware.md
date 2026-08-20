---
name: Target hardware pivot
description: Primary development targets are now Pixel 8 and M1 MacBook Air. FP5 is on ice.
type: project
---

Primary targets are Pixel 8 (purchased 2026-03-23, in transit) and M1 MacBook Air.

FP5 is deprioritized — device is bricked (EDL mode, all partitions erased), waiting on Fairphone support ticket for firehose programmer. Will resume only if Fairphone responds.

**Why:**
- FP5: EL1 only (QHEE owns EL2), TZ blocks GIC/SPMI, fastboot unsigned images won't boot, currently bricked
- Pixel 8: EL2 available (pKVM disableable via fastboot), official bootloader unlock, GrapheneOS RE, $282 purchased on Swappa (unlocked, G9BQD, Obsidian)
- M1 Air: EL2 from day one via m1n1, no signing ceremony, proxy mode for live hardware exploration, same ARM64 ISA

**How to apply:**
- All new HAL work targets Pixel 8 and M1 first
- FP5-specific code stays but gets no new investment
- Next step: HAL trait split (ferros_hal → traits + ferros_hal_qcm6490 + ferros_hal_pixel8 + ferros_hal_m1)
