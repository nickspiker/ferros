# Pixel 8 Display — bring-up notes — Zil (0)

## Goal

Boot-time breadcrumbs on the physical screen — so a hang during a risky MMIO experiment (e.g. the UFS vendor region, [UFS.md]) leaves visible evidence instead of a silent freeze. Longer term: a real console/compositor for the phone OS.

## The panel

- **google-shoreline**, 1080x2400, **command mode**, **DSC** (Display Stream Compression). Command mode = the panel has its own GRAM and self-refreshes; DECON only pushes a frame when triggered. This is NOT a video-mode linear framebuffer that scans out continuously.
- Zuma DPU driver = **cal_9865** in the google display module (cloned to `/mnt/Harbor/ferros-ref/display-gs`, branch android-gs-shusky-5.15-android15-qpr1). Register maps: `samsung/cal_9865/regs-decon.h`, `regs-dpp.h`; sequences: `decon_reg.c`, `dpp_reg.c`, `dsim_reg.c`.

## Block addresses (zuma-drm-dpu.dtsi)

```
DECON0 MAIN    G#1947_0000 (0x6000)
DPP0 DMA (L0)  G#1990_0000 — RDMA_BASEADDR_P0 @ +G#40 = the FB pointer
DPP L7..       G#19D0_0000
DSIM0          G#1944_0000 (DSI) + G#1946_0000 (DPHY)
Display SYSMMU G#1984_0000 — write-protected by S2MPU (locks CPU on WRITE; reads TBD)
```

## What ABL leaves (measured 2026-08-16, `payloads/dispprobe`, READ ONLY)

```
DECON_VERSION      = 07060000   DECON v7.6, accessible
GLOBAL_CON         = 00000133   DECON_EN=1 RUN=1 IDLE=1 CMD_MODE=1
FRAME_COUNT        = 3 (static) sent the splash (3 frames), now idle/holding
DPP0_RDMA_BASE_P0  = FAC00000   framebuffer base DECON DMAs from
```

**This is the good news:** ABL leaves DECON *fully configured* — enabled, command mode, holding the splash, DMA pointed at a real surface. We do NOT need to bring up DECON/DSIM/DSC/panel from scratch. Breadcrumbs reduce to: **write new pixels into the FB, then re-trigger one DECON frame** (a small `decon_reg_start` / shadow-update + trigger sequence from `cal_9865/decon_reg.c`).

## `G#FAC0_0000` is an IOVA behind the DPU SYSMMU (confirmed via DT)

DECON0 in the DT has `iommus = <&sysmmu_dpuf0>, <&sysmmu_dpuf1>`, and there is NO reserved-memory carveout at `G#FAC0_0000` (the only 0xF-range rmem is `G#FD80_0000`). So `G#FAC0_0000` is a **DPU-SYSMMU IOVA**, not a CPU-physical address. DPU SYSMMU bases (zuma-sysmmu.dtsi):

```
sysmmu_dpuf0  G#1984_0000   (the "write locks the CPU" one from the boot notes)
sysmmu_dpuf1  G#19C4_0000
```

## The plan — same pattern as the USB SYSMMU bypass we already ship

We don't fight the IOVA translation; we replace the surface. Exactly what `kernel_main` already does for DWC3 (disable S2MPU → disable SYSMMU → DMA physical addresses):

1. **Disable the DPU S2MPU.** The DPU SYSMMU at `G#1984_0000` currently "locks the CPU on write" because a DPU-block S2MPU protects it. Find that S2MPU (an `s2mpu_*dpu*` in zuma-sysmmu/s2mpu DTS) and disable it (write 0 to CTRL0 — the proven sequence). Then the SYSMMU CTRL becomes writable.
2. **Disable/bypass the DPU SYSMMU** (`G#1984_0000` + `G#19C4_0000`): Samsung SysMMU v9 `MMU_CTRL` (offset 0) bit0=0 → translation off (same as the USB SysMMU bypass at `G#1104_0000`).
3. **Point DECON at our own buffer.** Allocate a physical pixel buffer in kernel DRAM (known physical address), write `DPP0 RDMA_BASEADDR_P0` (`G#1990_0040`) to it. With the SYSMMU bypassed, DECON DMAs that physical address directly.
4. **Write pixels** (reuse `ferros_hal::console` 8x16 font for text), then **re-trigger one DECON frame** (`decon_reg.c` shadow-update + `GLOBAL_CON` trigger) → DSC-compressed frame → panel GRAM → latched. One trigger per screen change (command mode).

## The catch: chicken-and-egg + freeze risk — safety net now DONE

Every step above is a NEW MMIO write on this device, and there's no screen yet to show a breadcrumb if one hangs. **The recoverable cluster watchdog (b602854) is now live and proven**: a hang auto-resets in ~87s and the phone re-enumerates on its own (verified with `payloads/hangtest`). So the display bring-up is no longer freeze-on-mistake — a bad DPU S2MPU / SYSMMU / DECON write that hangs recovers automatically. That was the one blocking piece of safety infrastructure; the SYSMMU-bypass steps above can now be attempted one at a time.

## Effort estimate

Feasible and the path is known, but it's a **multi-step bring-up** (~a few focused hardware sessions), not a one-shot: DPU S2MPU discovery + disable, SYSMMU bypass, DECON RDMA re-point, pixel write, frame re-trigger — each an iteration. ABL having left DECON configured (command mode, holding a frame) removes the hardest part (no DSIM/DSC/panel bring-up from scratch).

## Status

- [x] Identify panel/DPU (command mode + DSC, cal_9865)
- [x] Clone the display driver (Harbor `display-gs`)
- [x] Confirm ABL leaves DECON enabled + holding a frame; find FB base `G#FAC0_0000`
- [x] Determine physical-vs-IOVA: it's a DPU-SYSMMU IOVA (DT `iommus`, no rmem carveout)
- [ ] Recoverable cluster watchdog first (safety net for all further pokes)
- [ ] Find + disable the DPU S2MPU (unlocks SYSMMU CTRL writes)
- [ ] Bypass DPU SYSMMU (`G#1984_0000`/`G#19C4_0000`, MMU_CTRL bit0=0)
- [ ] Re-point DECON DPP RDMA base to our physical buffer
- [ ] Write test pattern + re-trigger one frame
- [ ] Minimal text glyph writer (reuse `ferros_hal::console` font) → boot breadcrumbs
