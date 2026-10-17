# DISPLAY-MODESET — how a resolution change is done, from the stock driver

Read from the husky display driver 2026-10-16 (`samsung/` = `/mnt/Harbor/husky-kernel/private/google-modules/display/samsung/`, `hk3` = `…/shusky/display/panel-google-hk3.c`). This is the sequence ferros's `dpu` module reproduces. A refresh-rate change at the same resolution is seamless (panel commands only, DISPLAY-HK3.md §3); a **resolution change is a full modeset**: DECON stopped and soft-reset, DSIM stopped, the DPHY isolated and its PLL re-locked, both re-initialised, the panel kept powered and in sleep-out the whole time (it keeps refreshing from its own frame memory; it never gets `28`/`29` or `10`/`11`, only `9D 01`, the PPS and `C3`/`F2`). A one-frame live switch of the same registers wedges the hardware (DISPLAY.md), which is why the order below matters.

## Order (exynos_atomic_commit_tail, `samsung/exynos_drm_fb.c:784`)

1. Panel bridge disable: `panel_state = MODESET`; hk3's disable returns at once ("RRS in progress, skip", `hk3:1838-1844`).
2. DECON disable (`exynos_drm_decon.c:1757-1808`): wait for frame done / idle; windows off, DPPs off; `decon_reg_stop(reset = true)`.
3. DSIM disable (`exynos_drm_dsim.c:499-547`): `dsim_reg_stop`, PHY power off = DPHY isolation on (PMU G#3EB8 bit 0 cleared, `phy-exynos-mipi-dsim.c:202-211`).
4. Mode set: software only (the new DSIM config; the PLL entry is the same PMSK at 1368 Mbps for both rasters).
5. DECON enable (`:1622-1665`): config with the new size and DSC; `decon_reg_init`; interrupts on. No `decon_reg_start` yet.
6. DSIM enable (`:302-341`): PHY power on (isolation off), `dsim_reg_init`, `dsim_reg_start`.
7. Panel (`hk3_enable`, `hk3:1738-1827`, `needs_reset` false): wait one TE then 273 × 1.05 µs; `9D 01`; the PPS (type G#0A, 128 bytes); `F0 5A 5A`, `C3 0D` (FHD) / `0C` (WQHD), `B0 00 01 F2`, `F2 81` / `01`, `F0 A5 A5`; the full feature block (`hk3_set_panel_feat`, enforce); `53 xx`; FFC on (`U @C5+36: C5 11 L`). No display-on, no TE2 update, no frequency change during the switch.
8. Planes: windows programmed, `SHD_REG_UP_REQ |= 0x3FFF`, **`decon_reg_start`**. The first new-size frame goes out on the next TE.

## DECON (main G#1947_0000, sub G#1949_0000; `cal_9865/decon_reg.c`)

**stop** (`:1989-2034`): TRIG_CON (G#30) set `HW_TRIG_MASK_DECON` (bit 4), clear `HW_TRIG_EN` (bit 0); GLOBAL_CON (G#20) `DECON_EN_F` (bit 0) = 0; SHD_REG_UP_REQ (G#50) |= GLOBAL (bit 31) | CMP (bit 20); poll `RUN_STATUS` (bit 4) == 0 every 10 µs, up to 1.5e6 / fps µs. If that fails: instant off (`DECON_EN` bit 1 and `_F` = 0, same request, poll). Then `SRESET` (bit 28), poll until it self-clears (2 ms); clear INT_PEND (G#70) bits 21, 20, 13, 12 and INT_PEND_EXTRA (G#74) bits 4, 0.

**init** (`:1911-1957`): CLOCK_CON (G#5000) clear the AUTO_CG and QACTIVE masks; SRAM_EN_OF_PRI_0/1 (G#300/304) = decon0's primary SRAMs 0–10 (G#1111_1111 / G#111), SEC 0; GLOBAL_CON OPERATION_MODE (bit 8) = command; BLD_BG_IMG_SIZE_PRI (G#220) = H << 16 | W; the urgent set OF_URGENT_EN / OF_RD_URGENT_0/1 / OF_DTA_CONTROL / OF_DTA_THRESHOLD (G#2C0–2D4) as the oracle shows; TRIG_CON = `HW_TRIG_EN` | HW_TRIG_SEL DDI0 (bits 25:24 = 0) | `HW_TRIG_MASK_DECON`; OF_PIXEL_ORDER (G#2A0) = RGB; data path: DSIMIF_SEL(0) (sub + G#8000) = 0, DATA_PATH_CON_0 (G#200) [7:0] = G#B1 (DSCC | DSC1 | DSC0 | DSIMIF0); DSC init (below); GLOBAL_CON `DECON_EN_F` = 0; clear every pend. Interrupts: INT_EN (G#60) |= bits 21, 20, 13, 12, 4, 0; INT_EN_EXTRA (G#64) = bits 4 | 0.

**DSC init** (DSC0 at sub + 0, DSC1 at sub + G#1000, each encoder one slice): CONTROL1 (+4) = G#222 (byte swap, flatness 2, SLICE_MODE_CH, not dual slice); CONTROL3 (+C) = 3 << 12 | comp, comp = slice width / 3 (224 WQHD, 168 FHD); PPS registers +G#40 … +G#94 = the 128-byte PPS as big-endian words (ferros_dsc::pps; the stock packer skips PPS52_55 at +G#74 by a loop bug — write it). Output FIFO: OF_SIZE_0 (G#290) = H << 16 | comp; OF_TH_TYPE (G#29C) [2:0] = 5; OF_SIZE_1 (G#294) = comp; OF_SIZE_2 (G#298) = slice height << 16 | comp.

**start** (`:1959`): GLOBAL_CON |= `DECON_EN` | `DECON_EN_F`; SHD_REG_UP_REQ |= GLOBAL | CMP (the flush added every window bit first); clear INT_PEND FRAME_START (bit 12); poll `RUN_STATUS` == 1 (2 ms); TRIG_CON unmask (EN = 1, MASK = 0) — ferros's painter then keeps its own masked-between-presents protocol.

## DSIM (link G#1944_0000, DPHY G#1946_0100, bias G#1946_0000, sysreg G#1942_1000; `cal_9865/dsim_reg.c`)

**stop** (`:2178`): INTSRC (G#50) = ~0, INTMSK (G#54) all masked; CLK_CTRL (G#20) `TX_REQUEST_HSCLK` (bit 20) = 0, poll DPHY_STATUS (G#1C) `STOPSTATE_CLK` (bit 8), 2 ms; CLK_CTRL `CLOCK_SEL` (bit 26) = 0 (OSC); sysreg +8 `M_RESETN` (bit 0) = 0; lanes off: MC_GNR_CON0 (DPHY + G#200) and MD_GNR_CON0(i) (+G#300 + G#100 i) `PHY_ENABLE` (bit 0) = 0, poll READY (bit 1) = 0; CLK_CTRL LANE_ESCCLK_EN [12:8] = 0, ESCCLK_EN (bit 16) = 0, ESC prescaler = G#FF; PLL_CON0 `PLL_EN` (bit 12) = 0, poll PLL_STAT0 (DPHY + G#40) bit 0 == 0; SWRST (G#04) bit 0, poll until clear.

**init** (`:2128`): sysreg +8 bit 4 = 0; CLK_CTRL CLOCK_SEL = 0; SWRST; CONFIG (G#4C) LANES_EN [4:0] = G#1F; CLK_CTRL LANE_ESCCLK_EN = G#1F, WORDCLK_EN (bit 17); sysreg M_RESETN = 0; clocks (`:1748`, skipped if PLL_STAT0 says locked): BIAS_CON0..5 = {10, 110, 3223, 0, 200, 2}; PLL_CON0..9 = {0, 0, 0, 0, 0, 500, G#8E, G#3D40, G#1E00, G#1300}; CLK_CTRL ESC_PRESCALER = 5, ESCCLK_EN; DPHY timing for the 1370 Mbps row {54, 21, 5, 66, 54, 8, 67, 41, 8}, HSTX_CLK_SEL = 1, ULPS exit G#1A9 (MC_TIME_CON4/0/1/2/3, MC_DESKEW_CON0 = 0, MD_TIME_CON4/0/1/2/3 × 4: the live values are in the oracle's DPHY dump); MC_GNR_CON0/1 = {0, 1334}; MC_ANA_CON0..3 = {G#7122, 0, 2, 0}; MD_GNR × 4; MD_ANA × 4 = {G#7122, 0, 0, 0}; PMSK: PLL_CON1 K = G#5400, PLL_CON0 P = 1 / S = 2, PLL_CON2 M = G#6F; PLL_CON6 WCLK_BUF_SFT = 3; PLL_CON7 lock count 500; clear INTSRC PLL_STABLE, PLL_EN = 1, poll PLL_STAT0 lock (2 ms). Lanes on (clock + 4 data): PHY_ENABLE = 1, poll READY. Sysreg M_RESETN = 1. CLK_CTRL CLOCK_SEL = 1 (word clock). Config (`:1586`): SFR_CTRL (G#64) SHADOW_REG_READ_EN (bit 1); CLK_CTRL NONCONT_CLOCK_LANE (bit 25); DESKEW HW_EN = 0; TIMEOUT (G#28) = G#00FF_FFFF; ESCMODE (G#2C) CMD_LPDT = 0, STOP_STATE_CNT = G#A; UNDERRUN_CTRL (G#34) = the underrun count (oracle: WQHD G#63A, FHD G#ED8 at 120 Hz); OPTION_SUITE (G#10C) TE_ON_CMD_ALLOW (bit 10); THRESHOLD (G#38) = align(ceil(slice width / 3), 4) × 2 (448 / 336); RESOL (G#3C) = H << 16 | threshold; NUM_OF_TRANSFER (G#30) = H; CONFIG: NUM_OF_DATA_LANE = 3, EOTP = 1, PER_FRAME_READ = 0, PIXEL_FORMAT = G#3E, VC = 0; SFR_CTRL SHADOW_EN (bit 0); CONFIG VIDEO_MODE (bit 18) = 0, CPRS_EN (bit 19) = 1; CPRS_CTRL (G#74) = G#A (multi-slice packet, 2 slices); SLICE01 (G#78) = slice width << 16 | slice width; CMD_CONFIG (G#80) PKT_GO_EN = 0; CMD_TE_CTRL0 (G#84) = W × 2 / 100; CMD_TE_CTRL1 (G#88) = (1368 × 95 × 100 / fps / 16) << 16 | 1368 × 110 × 100 / fps / 16.

**start** (`:2170`): CLK_CTRL `TX_REQUEST_HSCLK` = 1 (no ready poll with the non-continuous clock); unmask interrupts, clear INTSRC.

**Packets:** `9D 01` and the PPS go out unqueued through PKTHDR/PAYLOAD, after `dsim_reg_start` and before `decon_reg_start`. The DSIM's shadow registers latch with the frame; no explicit DSIM shadow request exists.

## Open

- Whether the DDIC strictly needs the PPS/`C3`/`F2` aligned just after a TE (the driver does it; nothing states the requirement).
- Whether DECON's SRESET also resets the DSC sub-block registers.
