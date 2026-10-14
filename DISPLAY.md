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

## Adopted after a live handoff (2026-10-13) — ferros draws on the panel

The cold-boot plan above is superseded on husky by adoption: hand off from a *running* Android (`tools/husky-hotload.sh --live`: screen on, DECON hibernation off via `/sys/devices/platform/19470000.drmdecon/hibernation`), so DECON0, DSIM, DSC and the panel are all still configured, then `ferros-bridge display "text"` (`ferros_kernel/src/shim_display.rs`). After a normal `adb reboot ferros` the DPU is powered down and every DECON register reads 0. Without `hibernation=0` the same is true even with the screen on: in command mode the driver powers the DPU off a few idle frames after each update.

What it does, all from measured state:
- **Picks a layer:** the first enabled window that covers the whole screen with an unscaled DPP layer (image size == window size). The window stack depends on the screen. One case seen: 7 windows, with win0 being a 209x466 layer scaled up by the DPP.
- **Finds its DPP:** WIN_CHMAP is `(DPUF << 3) | index`, giving DPUF0 L0..L6 at `G#1990_0000 + i*G#1000` and DPUF1 L7.. at `G#19D0_0000 + i*G#1000`. The SYSMMUs are dpuf0 `G#1984_0000` and dpuf1 `G#19C4_0000` (samsung v9, VM0, FLPT PPN at `+G#8404`).
- **Translates and draws:** it walks the layer's IOVA through that SYSMMU's tables (1 MiB sections, level-2 tables for 4/64 KiB pages; entries hold the address shifted right by 4). It draws through the HAL console over the physically contiguous prefix (8 MiB) and fills the rest page by page. Android's surfaces were seen above 4 GiB (`G#8_D100_0000`), which is fine with the MMU off.
- **Switches the layer to linear:** RDMA_IN_CTRL_0 goes from `G#FF400108` (RGBA8888 + AFBC) to `G#FF400700` (linear XRGB8888, matching the console's `0x00RRGGBB`). Every other window is turned off.
- **Pushes one frame:** SHD_REG_UP_REQ (window bits + GLOBAL + CMP), unmask TRIG_CON HW_TRIG_EN, wait for FRAME_COUNT to move, then mask the trigger again.

First run: FRAME_COUNT `G#4C -> G#4D`, shadow request consumed, INT_PEND = FRAME_START|FRAME_DONE, INT_PEND_EXTRA = 0 (no timeout or resource conflict). Every register says the frame reached the panel; the human-eye check is still pending.

## Status

- [x] Identify panel/DPU (command mode + DSC, cal_9865)
- [x] Clone the display driver (Harbor `display-gs`)
- [x] Confirm ABL leaves DECON enabled + holding a frame; find FB base `G#FAC0_0000`
- [x] Determine physical-vs-IOVA: it's a DPU-SYSMMU IOVA (DT `iommus`, no rmem carveout)
- [x] Recoverable cluster watchdog first (safety net for all further pokes)
- [ ] Find + disable the DPU S2MPU (unlocks SYSMMU CTRL writes)
- [ ] Bypass DPU SYSMMU (`G#1984_0000`/`G#19C4_0000`, MMU_CTRL bit0=0)
- [x] Re-point a DPP layer at linear pixels (adoption: keep its IOVA + mapping, switch AFBC off)
- [x] Write pixels + trigger one frame (registers confirm; visual check pending)
- [x] Minimal text glyph writer (`ferros_hal::console`, `bridge display "text"`)
