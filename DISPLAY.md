# Pixel 8 Display — bring-up notes — Zil (0)

## Goal

Boot-time breadcrumbs on the physical screen — so a hang during a risky MMIO experiment (e.g. the UFS vendor region, [UFS.md]) leaves visible evidence instead of a silent freeze. Longer term: a real console/compositor for the phone OS.

## The panel

- husky (Pixel 8 Pro): **Samsung S6E3HC3** (DT `zuma-shusky-display.dtsi`: `samsung,s6e3hc3` / `-c10` by revision), **1344x2992**, **command mode**, **DSC** (Display Stream Compression). (The google-shoreline 1080x2400 noted here first is the Pixel 8 / shiba panel, from the original purchase note.) Command mode = the panel has its own GRAM and self-refreshes; DECON only pushes a frame when triggered. This is NOT a video-mode linear framebuffer that scans out continuously.
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

First run: FRAME_COUNT `G#4C -> G#4D`, shadow request consumed, INT_PEND = FRAME_START|FRAME_DONE, INT_PEND_EXTRA = 0 (no timeout or resource conflict). Every register says the frame reached the panel, and on 2026-10-14 the owner confirmed by eye: the amber console text is on the screen.

## Cold boot (slot b, no Linux) — ferros draws on ABL's splash surface (2026-10-14)

On a cold boot ABL leaves:
- the DPU power domains on (EL3 reads of pd-dpub/dpuf0/dpuf1 = 1);
- DECON v7.6 running its splash on window 5 / DPP0, at the panel's native 1344x2992;
- the layer already linear XRGB8888 (IN_CTRL_0 G#FF400700);
- both DPU SYSMMUs with translation off (VM0 ctrl G#24, no page table), so the layer's DMA address G#FAC0_0000 is physical (15.3 MB contiguous).

`display::show` treats "translation off" as an identity mapping and otherwise runs exactly as after a handoff. FRAME_COUNT 4 -> 5, and the owner saw the text. Each cold boot also paints its status screen automatically. Text is smaller than after a handoff because nothing scales it: the console draws 8x16 glyphs at 2x straight onto the native grid.

## DSI command path (2026-10-16) — ferros speaks to the panel

The panel is google-hk3 on DSIM0 (DISPLAY-HK3.md; the oracle of every transition is `oracle/display/husky-hk3/`). `dsim.rs` sends DCS packets through DSIM0's SFR packet FIFOs between presents (the link is idle while the painter masks the TE trigger). First run on a cold boot: brightness `51 00 C4` / `51 0C EB` / `51 05 49`, dimming `53 28` / `53 20`, display off `28` and on `29` — every packet accepted and drained by the FIFO. ABL leaves the DSIM link configured as Android runs it (lanes, format, PLL identical; only interrupt masks and the command-mode TE timers differ). `bridge display dcs '<hex> / <hex> wNN / ...'` plays any oracle line; `bridge display dsim` dumps the link registers. Reads work too (`bridge display read <reg> [len]`): the panel reports power mode G#9F, its extinfo, and a brightness readback equal to the DBV written. `bridge display terate` counts TE pulses on gpp0-3 for half a second: ABL leaves the panel at 60 Hz, and replaying the oracle's W120 / W60 transitions switches it to 120 and back, measured.

## Status

Driver-wise (2026-10-15): ferros pushes frames on ABL's pipeline (adopt DECON, write ABL's surface, shadow update + trigger); it owns no power-up from off, no DSI command path (brightness G#51, display off/on G#28/G#29, sleep in/out G#10/G#11), no framebuffer of its own, no tear-effect sync, and it lives in the kernel rather than as a process. On cold boot the SYSMMU/S2MPU items below are moot (ABL leaves translation off). **Before the display driver work starts: a knobs inventory (everything DECON/DPP/DSIM/DSC/panel lets us set) and the ideal design, for the owner to review — no code first.**

- [x] Identify panel/DPU (command mode + DSC, cal_9865)
- [x] Clone the display driver (Harbor `display-gs`)
- [x] Confirm ABL leaves DECON enabled + holding a frame; find FB base `G#FAC0_0000`
- [x] Determine physical-vs-IOVA: it's a DPU-SYSMMU IOVA (DT `iommus`, no rmem carveout)
- [x] Recoverable cluster watchdog first (safety net for all further pokes)
- [ ] Find + disable the DPU S2MPU (unlocks SYSMMU CTRL writes)
- [ ] Bypass DPU SYSMMU (`G#1984_0000`/`G#19C4_0000`, MMU_CTRL bit0=0)
- [x] Re-point a DPP layer at linear pixels (adoption: keep its IOVA + mapping, switch AFBC off)
- [x] Write pixels + trigger one frame (registers confirm, visually confirmed 2026-10-14)
- [x] Minimal text glyph writer (`ferros_hal::console`, `bridge display "text"`)
- [x] DSI command path: DCS writes to the panel from ferros (2026-10-16; brightness, dimming, off/on)
- [x] DCS reads (2026-10-16): power mode G#9F, extinfo 0a 80 20 (= Android's), pixel format, TE on, and brightness readback G#52 returning exactly the DBV written — the writes are proven by the panel itself
- [x] Refresh-rate switches W120 ⇄ W60 from the oracle's packets, measured on the TE pin (gpp0-3): ABL leaves the panel at 60 Hz; the W120 transition gives 120 Hz, W60 gives 60 (`bridge display terate`)
- [x] The panel's auto-rate idle floors (`bridge display idle 30|10|1`, the oracle's auto-mode sets): measured on the TE pin with changeable TE at 30 Hz, 10 Hz and ~1 Hz after 1.5 s of stillness; `display idle 0` returns to manual 120. Android's fixed-TE setting keeps the pin at the op rate while the panel idles underneath, which is why the measurement switches TE mode and restores it
- [x] AOD (LP) in and out from ferros, the oracle's AOD_enter / AOD_exit sets: in LP the TE pin shows 30 Hz with a 687 µs pulse (constant 693), the display stays on and the brightness readback is 853, the AOD DBV; exit gives 120 Hz / 273 µs and frames flow
- [x] NS 60 Hz op mode in and out (the oracle's OPHZ60_NS / OPHZ120_HS sets): TE pulse 1201 µs (constant 1223; HS 60 is 8.5 ms); HBM on/off sets replayed with the panel healthy (its `53` register does not read back)
- [x] `bridge display mode 60|120`: the hk3 command set for the rate (hk3.rs, from the oracle), then DSIM's command-TE timers for it, then the TE pin measured
- [x] DECON brought to Android's 120 Hz state where it is safe to (`bridge display tune`: SRAM allocation, out-FIFO QoS, interrupt enables), verified by a live register diff against the oracle
- [x] 10 bpc compose (2026-10-16): `bridge display modeset fhd|wqhd 10` — `GLOBAL_CON.TEN_BPC_MODE`, enhance path = DQE + dither, the DQE top block sized for the raster (IMG_SIZE, FRM_SIZE, FRM_PXL_NUM), its display dither on, CGC and ATC off, the layer's DPP and HDR-common `BPC_MODE` bits, the DQE shadow request at start. Frames push (FRAME_COUNT 1→5 over four shows), no frame timeout, shadow request clears; DECON, DPP0 and HDRCOMM0 match Android's F120/W120 dumps except status words, the DPP IRQ enable and Android's AFBC bit in SRAMC0. The earlier wedge was `TEN_BPC` + dither **without the DQE in the path and sized** — DECON0's dither bit means "the DQE output is dithered", so it stalls on an unsized DQE. `… 8` returns to ABL's path. What 10 bpc buys on an 8 bpc DSC link is blend precision + dither (DISPLAY-DESIGN.md §6); a 10 bpc *link* (10 bpc PPS to the panel) is still the step-5 experiment.
- [x] The DSC PPS packed by ferros (`ferros_dsc`), word for word what Android's DECON holds for both rasters
- [x] **The display server, first cut** (2026-10-16): `payloads/display` — an EL0 process holding the `display-fb` and `display-heap` regions and grants for DECON0 (main/win/wincon), layer 0's DMA/DPP and the TE GPIO bank; it re-points the layer at its framebuffer, paints the shell's first-boot screen (SHELL.md 3) with fluor's no_std canvas and Open Sans through fontdue, and presents through DECON's TE trigger once a second with the uptime in the status line. Measured: typeface parse 18–28 ms, 14.6 MiB of heap for it (hence the 16 MiB cacheable `display-heap` region — a parse over the non-cacheable framebuffer region ran for minutes), FRAME_COUNT advancing, RDMA_IRQ FRAME_DONE only. The payload is linked `-pie` and relocates itself at entry (`ferros_sys::relocate`): fontdue's `dyn OutlineBuilder` vtable was an absolute address that faulted unrelocated — the first payload past that gap. Spawn line in the payload's header. Not yet: a `display` endpoint for clients, the shell's other screens, TE-paced presents, mode/brightness/idle control moving in from the kernel's bench ops, the real clock.
- [x] **Our own framebuffer** (2026-10-16): `cachemap::DISPLAY_FB` = 32 MiB of Normal non-cacheable, EL0 read/write DRAM at G#8400_0000 (free on a cold boot; the first Linux carveout is at G#9000_0000), granted as the DMA region `display-fb`. `bridge display fb own` re-points the base layer's DMA at it (translation is off, so the DMA address is physical), stride = width, offset 0; the painter draws there. Proof: RDMA_IRQ reads FRAME_DONE only (no READ_SLAVE_ERR / AXI_ADDR_ERR — nothing fences the DPU out of that DRAM), FRAME_COUNT advances, and a blue fill in it costs the panel 0.4 W over black. `display fb abl` puts ABL's base/stride/offset back.
- [ ] **10 bpc link — ready for eyes** (2026-10-16): `bridge display modeset wqhd link10 [f2=XX]` sends the panel a 10 bpc / 8 bpp PPS (ferros_dsc::ten_bpc: Annex E's 10 bpc column) and encodes the same in DECON, blend at 10 bpc with no dither. Blind result: DECON runs (frames push, no timeout), PPS word 0 reads G#1100_AB (bpc 10, line buffer 11), the panel takes the PPS and still answers C3 = 0C and F2 = 01. Whether the picture is right needs the owner's eyes: run it, look at a ramp (`display fill` greys, or the console text), then try the F2 byte the driver calls "8/10bit config" (`f2=00`, `f2=03`, `f2=11` are the candidates; `f2=01` is the stock WQHD value, `81` FHD). `display modeset wqhd 10` is the way back. If the panel shows garbage in every variant, 10 bpc on the link is a documented refusal and the 10 bpc blend + dither (default now) is the answer.
- [x] Power profile (2026-10-16): DISPLAY-POWER.md — every mode, scene and brightness measured from the battery with the charger's buck off (`bridge display power … battery`, `display fill`, `tools/display-power-sweep.sh`). Still to measure there: the SoC floor with the core asleep, the Android baseline.
- [x] Link rate: `bridge display modeset fhd|wqhd 1346|1368` moves the DPHY between ABL's 1368 Mbps and Android's 1346 (PLL PMSK + lock count, lane timing row, the panel's FFC table off → new table → on, the TE timers from the rate), both ways, both rasters, verified: PLL words and CMD_TE_CTRL1 read back as Android's at 1346, panel C3 and TE unchanged. DISPLAY-HK3.md §7.
- [x] FHD modes, by the full modeset (`dpu.rs`, DISPLAY-MODESET.md; `bridge display modeset fhd|wqhd`): DECON stopped and soft-reset, DSIM stopped with its PLL and lanes, both re-initialised with the raster's geometry and started, the panel's PPS and raster select just after a TE. Verified both ways: the panel reports `C3` = 0D at FHD and 0C at WQHD, DECON's frame count restarts and keeps advancing, TE 120 Hz / 272 µs, display on. (A one-frame live rewrite of the same registers wedged the hardware; that is why the modeset exists.)
