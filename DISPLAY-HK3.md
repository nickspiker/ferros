# DISPLAY-HK3 — the husky panel, from its driver

The Pixel 8 Pro (husky) panel is **google-hk3** (model HK3-A0B-00), on **DSIM0**, driven by DECON0. This is the driver-author's inventory of it, read from the stock driver with line citations, 2026-10-16. The live confirmation of every sequence below, as Android actually sends it, is `oracle/display/husky-hk3/` (register dumps and DSI packet traces per state). An earlier inventory in DISPLAY-DESIGN.md came from `panel-samsung-s6e3hc3-c10.c`, a sibling device's panel; it is superseded by this.

Sources (under `/mnt/Harbor/husky-kernel/private/`):
- **HK3** = `devices/google/shusky/display/panel-google-hk3.c` — the bound driver (`compatible = "google,hk3"`, HK3:2917, `shusky/dts/zuma-husky-display.dtsi:11-17`). `panel-gs-hk3.c` beside it is an unfinished port (`google,gs-hk3`), not bound; it lacks LP, HBM, self-refresh, op_hz, TE2 and refresh_ctrl hooks and differs in NS TE width (546 vs 1223 µs), FFC at init and enable order. Use HK3.
- **DRV** = `google-modules/display/samsung/panel/panel-samsung-drv.c` (the panel core: rails, reset, sysfs, debugfs), **DSIM** = `…/samsung/exynos_drm_dsim.c`, **DBG** = `…/samsung/exynos_drm_debug.c`.
- **DPU** = `devices/google/zuma/dts/zuma-drm-dpu.dtsi`, **TIM** = `…/zuma/dts/zuma-display-timing.dtsi`, **PMIC** = `…/zuma/dts/zuma-pmic.dtsi`, **SD** = `shusky/dts/zuma-shusky-display.dtsi`.

Byte notation: `@RR+oo: RR …` means first the global-parameter offset write `B0 00 oo RR`, then `RR …` (`@RR+2oo` means `B0 02 oo RR`). **U/L** = `F0 5A 5A` unlock / `F0 A5 A5` lock (HK3:334-335). **LATCH** = `F7 0F` (HK3:336). hk3 never uses the F1 or FC unlocks.

## 1. Modes (HK3:2308-2607)

Common to every mode: 8 bpc, `MIPI_DSI_CLOCK_NON_CONTINUOUS`, command mode, `vblank_usec` 120, underrun `te_idle_us` 350 / `te_var` 1 (HK3:2272), 70×155 mm, TE2 rising 0x10 / falling 0x30. `vscan` is 0 everywhere, so `is_vrr_mode()` is always false and every VRR branch in HK3 is dead code.

| Mode (line) | clock kHz | H act/fp/sa/bp (total) | V act/fp/sa/bp (total) | te_usec | DSC | idle_mode | build |
|---|---|---|---|---|---|---|---|
| 1344x2992x1 (2311) | 4545 | 1344/80/24/52 (1500) | 2992/12/4/22 (3030) | 0 | WQHD | unsupported | factory only |
| 1344x2992x5 (2340) | 22725 | hbp 52 (1500) | 3030 | 0 | WQHD | unsupported | factory only |
| 1344x2992x10 (2369) | 45147 | hbp 42 (1490) | 3030 | 0 | WQHD | unsupported | factory only |
| 1344x2992x30 (2398) | 135441 | hsa 22, hbp 44 (1490) | 3030 | 0 | WQHD | unsupported | factory only |
| **1344x2992x60** preferred (2429) | 270882 | 80/24/42 (1490) | 3030 | see §5 | WQHD | on self-refresh | all |
| 1344x2992x120 (2459) | 541764 | 1490 | 3030 | 273 | WQHD | on inactivity | all |
| 1008x2244x60 (2490) | 157320 | 1008/80/24/38 (1150) | 2244/12/4/20 (2280) | see §5 | FHD | on self-refresh | non-factory |
| 1008x2244x120 (2519) | 314640 | 1150 | 2280 | 273 | FHD | on inactivity | non-factory |
| LP 1344x2992x30 (2553) | 135441 | 80/24/42 (1490) | 3030 | 693 | WQHD | LP | all |
| LP 1008x2244x30 (2580) | 78660 | 1150 | 2280 | 693 | FHD | LP | non-factory |

- Refresh ranges: {1, 10, 30, 60, 120} (plus 5 on factory builds); LP {1, 30} (HK3:2281-2291).
- **FHD is scaled inside the panel.** CASET/PASET in init are always 1343/2991 (HK3:1674-1677). On every enable: `U; C3 0D` (FHD) or `C3 0C` (WQHD); `@F2+01: F2 81` (FHD) or `F2 01` (WQHD); `L` (HK3:1798-1803). The driver's comment calls the F2 write "8/10bit config"; what its bits mean is UNCONFIRMED and is the lead for a 10-bit experiment.
- Seamless switches only when the resolution is unchanged (HK3:2089-2097); a resolution switch is a full modeset and HK3 waits a vsync before re-sending the PPS (HK3:1765-1775).

## 2. DSC / PPS (WQHD HK3:185-246, FHD HK3:249-310)

| Field | WQHD | FHD |
|---|---|---|
| pic_width × pic_height | 1344 × 2992 | 1008 × 2244 |
| slice_width × slice_height | 672 × 187 (2 per line, 16 rows) | 504 × 187 (2 per line, 12 rows) |
| slice_chunk_size | 672 | 504 |
| initial_dec_delay | 592 | 508 |
| scale_decrement_interval | 9 | 7 |
| scale_increment_interval | 5177 | 4482 |
| slice_bpg_offset | 112 | 150 |

Identical in both: DSC 1.1, bits_per_component 8, bits_per_pixel 128 (8.0 bpp in 1/16 units), line_buf_depth 9, convert_rgb, block_pred_enable; simple_422/native_422/native_420/vbr all 0; rc_tgt_offset 3/3, rc_edge_factor 6, rc_quant_incr_limit 11/11; initial_xmit_delay 512, first_line_bpg_offset 12, initial_offset 6144, rc_model_size 8192; flatness_min/max_qp 3/12, initial_scale_value 32, nfl_bpg_offset 133, final_offset 4336; second_line_bpg_offset, nsl_bpg_offset, second_line_offset_adj 0. rc_buf_thresh = {14, 28, 42, 56, 70, 84, 98, 105, 112, 119, 121, 123, 125, 126}. rc_range_params (min_qp, max_qp, bpg_offset): (0,4,2) (0,4,0) (1,5,0) (1,6,62) (3,7,60) (3,7,58) (3,7,56) (3,8,56) (3,9,56) (3,10,54) (5,11,54) (5,12,52) (5,13,52) (7,13,52) (13,15,52); the offsets 62…52 are 6-bit two's complement, −2…−12.

Mode wiring: `dsc_count` 2, `slice_count` 2, `slice_height` 187 (HK3:2293-2306). **Sending the PPS** (HK3:1777-1783), on every enable: `9D 01` as a DCS short write (Samsung "DSC on"; exact semantics UNCONFIRMED), then the 128-byte PPS as a DSI **type 0x0A** long packet (`mipi_dsi_picture_parameter_set`). On a fresh reset the PPS goes out **before** `11` (sleep out) — confirmed in the oracle's `ON` capture (`pps begin/end`, then `11`).

## 3. Command sets and transitions

| Set | Bytes, in order | Ref |
|---|---|---|
| **init** | `11` + 10 ms · U · [rev < EVT1.1: `@94+58: 94 0B F0 0B F0`] · AMP `@F4+4F: F4 50` · Vreg 4.5 V `@F4+31: F4 00 00 00 00 00` · L + **110 ms** · `35` (TE on, no parameter) · U · [rev < DVT1: `@BB+03: BB 41`] · TSP sync `@B9+3C: B9 19 09` · FFC off, 1368 Mbps `@C5+36: C5 10 10 50 05 4D 31 40 00 40 00 40 00` ×4 groups (36 bytes after C5) · TE width `@B9+04: B9 0B BB 00 2F 0B BB 00 2F 0B BB 00 2F` · [rev < MP: OPEC `@63+1D: 63 02 18`] · PMIC fast discharge off `@B1+18: B1 55 01`, `@B1+13: B1 80` · LATCH · L · `2A 00 00 05 3F` · `2B 00 00 0B AF` | HK3:1623-1679 |
| **display_on** | U · sync_begin `E4 00 2C 2C A2 00 00` · AMP `@F4+4F: F4 70` · Vreg 7.1 V `@F4+31: F4 1A ×5` (≥ DVT1; `1B ×5` before) · sync_end `E4 00 2C 2C 82 00 00` · L · `29` | HK3:1397-1412 |
| **display_off** | `28` · U · E4 begin · `@F4+4F: F4 50` · `@F4+31: F4 00 ×5` · E4 end · L | HK3:1414-1428 |
| post-init extras | ≥ DVT1: `U @F4+21: F4 1E; @F4+69: F4 78; @F4+17: F4 1E; LATCH; L` (HK3:1721-1736). E7 material: `U @CB+23F: CB 0A; @CB+245: CB 0A; LATCH; L` (HK3:1681-1690). PROTO1 only: lhbm luminance opr (HK3:1692-1719). | |
| **ffc** | On an HS-clock change: `U @C5+37: C5 10 50 05 4D 31 …` (1368) or `… 4E 74 …` (1346), `L`; then always `U @C5+36: C5 11 L` (FFC on). `C5 10` (off) precedes a pending clock change on self-refresh entry. | HK3:2187-2241 |
| refresh_ctrl | `U F7 02 L` (one frame insertion), VRR only, so a no-op here. | HK3:2250-2270 |

**`hk3_set_panel_feat`** (HK3:542-875) runs as one `U … LATCH L` batch over a feature bitmap (HBM, IRC_OFF, IRC_Z_MODE, EARLY_EXIT, OP_NS, FRAME_AUTO; HK3:36-44):

| Step | When | Bytes |
|---|---|---|
| TE | EE, OP_NS or vrefresh changed | Fixed TE (EE set, not `force_changeable_te`): `B9 51; @B9+02: B9 (vrefresh == peak ? 00 : 01); @B9+08: B9 0B 43 00 2F 0B 43 00 2F` (NS) or `0B BB 00 2F ×2` (HS). Changeable TE: `B9 04; @B9+04: B9 0B 43 00 2F` (NS) or `0B BB 00 2F` (HS). |
| TE2 | OP_NS changed | §5 |
| IRC (≥ EVT1) | IRC_Z changed | `@92+200: 92 {Z: E6 → BE 98, else F1 C1; off: 00 00}`; `@68+2F3: 68` + 6 bytes: Z E6 `97 87 87 FB FD F1`, Z other `82 70 23 91 88 3C`, off E6 `71 81 59 90 A2 80`, off other `77 81 23 8C 99 3C` |
| IRC (< EVT1) | IRC_OFF changed | `@92+19B: 92 07` (off) / `27` (on) |
| Op mode | OP_NS changed | `F2 01; 60 18` (NS, 60 Hz) or `60 00` (HS, 120 Hz) |
| Early exit, always | — | `BD 21 X 83 03 Y`: EE on → X = 00 (HBM) / 01; EE off → X = 80 (HBM) / 81; Y = 01 (HBM) / 03. Then `@BD+10: BD E`, `@BD+82: BD E E E E` with E = 22 (EE on) / 00. Then `@BD+(NS ? 4E : 1E): BD` + 12 bytes: HBM+NS `00 00 00 02 00 04 00 0A 00 16 00 76`; HBM+HS `00 00 00 01 00 03 00 0B 00 17 00 77`; NS `00 00 00 04 00 08 00 14 00 2C 00 EC`; HS `00 00 00 02 00 06 00 16 00 2E 00 EE` |
| AUTO rate | FRAME_AUTO | NS: `@BD+0C: BD 00 00`. HS: `@BD+92: BD 00 v` (60 Hz: 01 HBM / 02; 120 Hz: 00). Target `@BD+12: BD 00 00 t` (NS 30/10/1 Hz = 04/14/EC, HBM 02/0A/76; HS 06/16/EE, HBM 03/0B/77). Step `@BD+9E: BD` (NS `00 04 00 14 00 00`, HBM `00 02 00 0A 00 00`; HS `00 02 00 06 00 16`, HBM `00 01 00 03 00 0B`). `@BD+AE: BD` 3 bytes (NS → 30/10/1: `00 00 00` / `01 00 00` / `01 03 00`; HS60: `01 00 00` / `01 01 00` / `01 01 03`; HS120: `00 00 00` / `00 03 00` / `00 01 03`). Then `BD A3`. |
| MANUAL rate | otherwise | `BD 21; 60 v`. HS 120/60/30/10/5/1 Hz = 00/01/02/03/06/07. NS 60/30/10/5/1 Hz = 18/19/1B/1E/1F. |
| End | — | LATCH, L |

Refresh policy: FRAME_AUTO iff `idle_vrefresh != 0`; EARLY_EXIT iff `vrefresh == op_hz` or `idle_vrefresh != 0` (HK3:916-966). `change_frequency` rejects `vrefresh > op_hz` (968-990). Min idle rate: `min_vrefresh` rounds up to 1, 10 or 30 (above 30 → 0); 0 if idle is off, dimming is on, the `idle_delay_ms` window hasn't passed, or it would be ≥ the target (513-540). `set_op_hz` accepts 60 or 120 only, needs `vrefresh ≤ hz`; 60 sets OP_NS (2099-2131).

Self-refresh and idle: `set_self_refresh` (1057-1125) on entry sets auto mode at the min idle rate (and a PANEL_IDLE_ENTER uevent); on exit manual mode, waiting a vblank if `panel_need_handle_idle_exit`. `update_idle_state` (1879-1923) on each commit: if AUTO and > 17 ms since the last commit, either `U LATCH L` (only with `force_changeable_te` and `idle_delay_ms` 0) or drop to manual. The oracle's `*_idle` captures show this loop as Android lives in it.

**LP (AOD) entry**, `hk3_set_lp_mode` (1502-1573): disable features (HS, manual, changeable TE) → wait vsync → display_off → `51 03 55` (DBV 853, "50 nits") → wait vsync → `53 24` (AOD on) → binned LP set → `U · B9 51 · @B9+08: B9 0B E0 00 2F 0B E0 00 2F · @B9+02: B9 00 · @BD+18: BD 04 00 74 · @BD+B8: BD 00 08 · @BD+C8: BD 03 · BD A7 · @BD+E8: BD 00 · @BD+10: BD 22 · @BD+82: BD 22 22 22 22 · LATCH L` → display_on. Binned LP (347-369): DBV ≤ 766 → "low" `U @94+52: 94 01 07 6A 02 L` (10 nits), else "high" `94 00 07 6A 02` (50 nits).

**LP exit**, `set_nolp_mode` (1575-1621): `U BD 21; B9 04; @B9+04: B9 0B E0 00 2F; @60+01: 60 00 LATCH L` → wait vsync (30 Hz) → display_off → wait vsync → `U @94+52: 94 00 L` → `53 20` → set_panel_feat (enforce) → `53` → change_frequency → display_on.

**Enable** (1738-1827): reset if the panel was off → PPS → init + extras if reset → C3/F2 → E7 gamma fix → LP ? set_lp_mode : (update_panel_feat → `53` → change_frequency → [reset or BLANK: wait vsync (60 Hz), display_on]). The core then runs ffc, TE2 and the backlight update (DRV:4149-4226). **Disable** (1829-1873): skipped during mode switches; core forces HBM/LHBM off; disable features (60 Hz); sleep one frame + 1 ms; display_off + 20 ms; if going fully off, `10` + 100 ms.

## 4. Brightness, HBM, ACL, local HBM, thermal, OPR

| Item | Detail | Ref |
|---|---|---|
| `51` DBV | 2 bytes big-endian, 0–4095. DBV 0 sends `22` (all pixels off); the next non-zero first sends `13`. In LP it switches the binned-LP set instead. | HK3:1354-1395 |
| dbv_extra_frame | After a commit that changes the level or global HBM, the core waits one extra vblank before the frame. | HK3:2873, DRV:2725-2740 |
| Levels | normal 2–1000 nits at DBV 196–3307 (0–63 %); HBM 1000–1600 nits at 3308–4095 (63–100 %); max 4095; default 1353 = 140 nits; `bl_range` {94, 180, 270, 360, 3307}; HDR10 + HLG; luminance max/avg/min 10 000 000 / 1 200 000 / 5 | HK3:2837-2884 |
| `53` WRCTRLD | BCTRL 0x20 (always) \| HBM 0xC0 \| LHBM 0x10 \| DIM 0x08. AOD on/off = `53 24` / `53 20`. | HK3:312-315 |
| HBM | `set_hbm_mode` sets FEAT_HBM; HBM_ON_IRC_OFF sets IRC_Z (≥ EVT1) or IRC_OFF. Off: `53` then feat; on: feat then `53`. Oracle `BR_HBM_on`: `53 E0`. | HK3:1947-1985 |
| Dimming | rewrites `53` (`53 28` on, `53 20` off); skipped in LP | HK3:1987-1998 |
| ACL `55 v` | no unlock; only if HBM on, DBV ≥ threshold, mode ≠ OFF. ≥ EVT1: ENHANCED 3865 → `03` (17 %); NORMAL: 3570–3962 → `01` (7.5 %), ≥ 3963 → `02` (12 %). Re-evaluated after each DBV write; default NORMAL. | HK3:1284-1352 |
| ZA | `U @92+16C: 92 v L`, v = `11` on (`21` on PROTO1), `00` off; enabled whenever ACL > 0 unless `force_za_off` | HK3:1250-1282 |
| OPR | `U @91+E7`, DCS read `91` (2 bytes), `L`; percent = ((b0 << 8) \| b1) × 100 / 0x3FF | HK3:1221-1248 |
| Local HBM | `53` bit 0x10; brightness `U @95+321: 95 Rc GBc Rf Gf Bf L` (read back from 0x95 at init); overdrive by luminance band; histogram ROI d 766 / r 115 (FHD 575 / 87). The fingerprint spot, not a general tool. | HK3:2000-2081, 2623-2704 |
| Thermal | `U @67+03: 67 <°C> L` for 10–49 °C, ≥ EVT1.1, normal state only; from thermal zone `disp_therm`, every 30 s | HK3:371-412 |

## 5. TE / TE2

- TE width (HK3:323-327): AOD 693, 120 Hz 273, 60 Hz HS **8500**, 60 Hz NS 1223 µs. **Measured on the pin from ferros (2026-10-16, `bridge display terate`): 120 Hz 272 µs, 60 Hz HS 8.3–8.6 ms** — the constants hold. `get_te_usec` returns the HS/NS value at 60 Hz, else the mode's (1430-1440).
- TE width registers: HS `0B BB 00 2F`, NS `0B 43 00 2F`, AOD `0B E0 00 2F` (reading them as line positions is UNCONFIRMED).
- TE2 option `B9`: changeable `04` / fixed `51` (414-427); fixed in LP, or with EE on and auto rate < 30. Offsets rising 0x10, falling 0x30 (HS) / 0x25 (NS). `update_te2` (429-490): `U @F2+42: F2 0D; @B9+01: B9 opt; @B9+(fixed 22 / changeable 1E): B9 rH rL fH fL` (twice when fixed) `L` — seen in the oracle after every mode switch.
- Wait helpers (1453-1495): one vblank, then te_us × 1.05, + 1 ms; the changeable variant measures two vblanks and accepts |Δ − period| < 2 ms.

## 6. Power rails and reset

| Item | Value | Ref |
|---|---|---|
| reset | `gpa6-0`, active high; timing {1, 1, 5} ms: high 1 → low 1 → high 5 | SD:50, DRV:594-637 |
| VCI | `m_ldo14_reg` (S2MPG14 LDO14 "L14M_DISP_VCI"), DT range 2.5–3.3 V | SD:51, PMIC:391-398 |
| VDDI | `s_bucka_reg` (S2MPG15 BUCKA), 1.7–2.1 V, always-on | SD:52, PMIC:860-865 |
| VDDD | `m_ldo22_reg` (S2MPG14 LDO22 "L22M_DISP_VDDD"), 0.725–1.3 V | SD:53, PMIC:476-483 |

Operating voltages are UNCONFIRMED (the DT gives ranges only). Power-on: VDDI + 1 ms → VCI + 10 ms → reset → core reads extinfo `DA DB DC A1` and the DDIC id (`U`, read `D6` 5 bytes, `L`) → panel_init → VDDD on + 1 ms (DRV:660-783). Power-off: VDDD off + 1 ms → reset low → VCI off + 1 ms → VDDI off + 1 ms (DRV:701-804). The panel's own Vreg 7.1 / 4.5 V and AMP `70` / `50` writes are in §3. Revision decodes from the DB byte (HK3:2165-2174).

## 7. DSIM0 / DPHY / DECON

| Node | Values | Ref |
|---|---|---|
| drmdsim0 | `samsung,exynos-dsim`; reg dsi G#1944_0000 +300, dphy G#1946_0100 +700, dphy-extra (bias) G#1946_0000 +100; id 0; IRQ `IRQ_DPUB_DSIM0_DPUB`; phy `mipi_phy_dsim0_m4m4 0`; `te_from` 0; `te-gpio` gpp0-3 (pinctrl `hw_te_on/off` = function 0xf / 0) | DPU:424-454 |
| DPHY | `samsung,mipi-phy-m4m4` @ G#1946_0000, isolation 0x3eb8 | DPU:417-422 |
| Lanes / format | 4 lanes, RGB888 | HK3:2869 |
| PLL | both resolutions: p 1, m 0x6F, s 2, k 0x5400; **hs 1368 Mbps, esc 20 MHz**; 24.576 MHz in → 2736 MHz VCO (formula UNCONFIRMED, numbers match). `hs-clock-rates` is empty, so the 1346 Mbps FFC table is unused. | SD:13-35, TIM:11-20, DSIM:619-645 |
| DPHY tuning (live) | clk_prepare 53, hs_prepare 53 ×4, cap peaking 7 (on, forward) ×5, de-emphasis off, 400 mV reference, driver resistors setting 2 | oracle `dphy.txt` |
| DECON0 | main G#1947_0000 (+6000), win G#1948_0000, sub G#1949_0000, wincon G#194A_0000, dqe G#194D_0000, dqe-cgc G#194F_0000, cgc-dma G#1990_E000; `ppc` 2; hibernation. decon1 is moved to DP on husky, so **decon0 drives dsim0**. | DPU:528-611, HD:27-29 |
| Underrun budget | 1e9 × 100 / (fps × (100 + te_var)) − 350 µs must cover the frame transfer | DSIM:1160-1192 |

## 8. sysfs and debugfs (the Android driver's control surface)

`/sys/class/drm/card0-DSI-1/panel/`: refresh_rate (RO), op_hz (RW 60/120), panel_idle (RW), min_vrefresh (RW; 0 = 1 Hz idle), idle_delay_ms, force_power_on, refresh_ctrl (WO, ignored by hk3), te2_timing / te2_lp_timing (RW), power_mode (DCS read `0A`), power_state, time_in_state / available_disp_stats (state 0 On, 1 HBM, 2 LP, 3 Off), serial_number (D6), panel_extinfo, panel_model, panel_pwr_vreg (reads back `@F4+31`, expect `1a1a1a1a1a`), error counts (DRV:2312-2338 and the handlers cited by the inventory). Backlight `/sys/class/backlight/panel0-backlight/`: brightness, hbm_mode, dimming_on, local_hbm_mode, lp_state, state, acl_mode.

debugfs `/sys/kernel/debug/dri/0/`: `DSI-1/panel/cmdsets/{init, lp/low, lp/high}` (packet type, bytes, waits), `DSI-1/panel/reg/{address,type,count,flags,payload}` (raw DCS write/read), `force_changeable_te(2)`, `force_za_off`, `reset_panel`; `crtc-0/reg_dump` (DECON + DSIM dump when on), `te_count`, `frame_done_count`, `underrun_cnt`; `dsim0/{state, force_set_hs_clk, dphy/<diag>/{desc,help,value}}`. Tracepoints `dpu/dsi_tx`, `dsi_rx`, `dsi_label_scope`, `panel/te2_update_settings` log every DSI packet with bytes and delay — how the oracle was captured.

## 9. Driver bugs noticed (don't copy them)

- `hk3_set_panel_feat` reads `changed_feat` before initialising it on the VRR path (dead here).
- `update_panel_feat` tests `mode.type & DRM_MODE_FLAG_NS`; the gs port tests `.flags`.
- `35` (TE on) is sent with no mode byte.
