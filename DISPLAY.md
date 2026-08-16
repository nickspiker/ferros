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

## The open question: is `G#FAC0_0000` physical or an IOVA?

DECON's RDMA reads through the **display SYSMMU** (`G#1984_0000`), so `G#FAC0_0000` is what *DECON* uses — an IOVA. Two cases:
- **Identity-mapped / SYSMMU bypass** → `G#FAC0_0000` is also the CPU physical address; we write pixels there directly. Easy. (ABL FBs are often physically contiguous and identity-mapped.)
- **Translated** → the CPU-physical address differs; we must read the display SYSMMU page tables (IOVA `G#FAC0_0000` → physical) to find where to write. The SYSMMU is write-protected but reads may be allowed — TBD.

### Deciding it SAFELY (next step)

A blind CPU read of `G#FAC0_0000` risks a fault/freeze if that region sits under an active DPU S2MPU (we've only disabled the HSI0/HSI2 S2MPUs, not the DPU's) — and with no screen yet and watchdogs disabled, a freeze is a silent physical-power-cycle. So do NOT read it blind. Options, cheapest first:
1. Check `G#FAC0_0000` against the live DT `/reserved-memory` and the DPU S2MPU's allowed ranges (from Android, read-only) — is it a known FB carveout, and is it CPU-reachable?
2. Read the display SYSMMU (`G#1984_0000`) context/page-table-base register (read, not write) to see if translation is even enabled; if disabled/bypass, IOVA==physical.
3. Only then attempt a guarded read, ideally once a recoverable-watchdog or a second breadcrumb channel exists.

## Re-trigger sequence (once the FB is writable) — from cal_9865

Command-mode frame kick (to be confirmed against `decon_reg.c`): update the DPP RDMA base if needed → set the DECON shadow-update/`GLOBAL_CON` trigger → DECON sends one frame over DSI (DSC-compressed) to the panel GRAM → new image latches. No continuous scanout needed; one trigger per screen change.

## Status

- [x] Identify panel/DPU (command mode + DSC, cal_9865)
- [x] Clone the display driver (Harbor `display-gs`)
- [x] Confirm ABL leaves DECON enabled + holding a frame; find FB base `G#FAC0_0000`
- [ ] Determine physical-vs-IOVA for `G#FAC0_0000` (safely — see above)
- [ ] Write test pattern + re-trigger one frame
- [ ] Minimal text glyph writer (reuse `ferros_hal::console` font) → boot breadcrumbs
