# WIFI — BCM4398 on PCIe ch1 (husky / zuma): bare-metal bring-up recipe

**Status (2026-10-15): goals 1-3 reached on husky, cold boot, from a driver process.** `tools/husky-wifi.sh` runs `payloads/pcie1` (EL0, hardware grants only) through the recipe below:
- survey: every PCIe ch1 CMU gate already on (G#0020_0000: CG_VAL=1, MANUAL=0, no auto-gating), PMU PHY control G#C0 (isolated), DBI answers G#A5A5_144D;
- PHY: external PLL lock 1 µs, PHY PLL lock 97 µs, CDR lock 29 µs, offset calibration 6.7 ms;
- link: LTSSM Polling.Active → **L0 in 81 µs**, link status G#3011_0000 = **Gen1 ×1** (Gen1 forced), RC id G#ECED_144D;
- config space: **G#14E4 : G#4444** (BCM4398 D11AX); BAR0 → chipcommon ChipID **G#6003_4398 = chip G#4398 rev 3**.
- NCI enumeration (stage 04 + `tools/nci-parse.py`, following nciutils.c nci_scan): EROM pointer G#1800_7118 (EROM1 G#1800_7000, OOBR G#1800_6000). Cores: chipcommon G#800 rev 76 @ G#1800_0000; PCIe2 G#83C rev 130 @ G#1800_1000; **ARM CA7 G#847 rev 13 @ G#1808_0000** (OOBR +G#240, DMP control at +8); **sysmem G#849 rev 14 @ G#1808_8000 with RAM at backplane 0..G#29_FFFF and G#2A_0000 + 4 MiB**; D11 G#812 ×3 @ G#1808_2000/4000/6000; all interfaces Booker. (The download path: hold the CA7 in reset with CPUHALT, write the image, NVRAM vars + random seed, the reset instruction, release — dhd_pcie.c `_dhdpcie_download_firmware` / `__dhdpcie_bus_download_state`; no FW_SIGNATURE on this board.)
Next: TCM/firmware download, NVRAM + CLM, the msgbuf rings (DMA through S2MPU G#131F_0000), then 802.11 and an IP stack — each as processes.

**Recipe source:** The bring-up is written as a driver *process* holding hardware grants (`payloads/pcie1`, ferros_kernel process.rs `--hw`), not as kernel code. Goal 1: PCIe link up (Gen1 or better). Goal 2: the endpoint's vendor/device ID over config space. Goal 3: dhd's first BAR0 chip-id read. Firmware download, msgbuf rings, 802.11 and an IP stack come after.

Numbers use the project's `G#` notation. **[AMBIGUOUS]** / **[INFERENCE]** mark what the sources don't settle; the open questions are collected in §13.

Source tags (all under `/mnt/Harbor/husky-kernel/`):

| Tag | Path |
|---|---|
| **RC** | `private/google-modules/soc/gs/drivers/pci/controller/dwc/pcie-exynos-rc.c` |
| **RCH** | `.../dwc/pcie-exynos-rc.h` |
| **CMNH** | `.../dwc/pcie-exynos-common.h` |
| **CAL** | `.../dwc/pcie-exynos-zuma-rc-cal.c` |
| **DWH** | `aosp/drivers/pci/controller/dwc/pcie-designware-host.c` |
| **DW** | `aosp/drivers/pci/controller/dwc/pcie-designware.c` |
| **DWHDR** | `aosp/drivers/pci/controller/dwc/pcie-designware.h` |
| **ZDT** | `private/devices/google/zuma/dts/` |
| **SFR** | `private/google-modules/soc/gs/drivers/soc/google/cal-if/zuma/cmucal-sfr.c` |
| **DHD** | `private/google-modules/wlan/bcm4398/` |

## 0. Address map (ch1 = HSI2 "GEN3A_1", not HSI1)

Source: ZDT/zuma-pcie.dtsi:75-122, ZDT/zuma-board-pcie.dtsi:31-43.

| Block | Base | Size | Notes |
|---|---|---|---|
| ELBI | G#1312_0000 | G#2000 | app/ctrl regs |
| SOC | G#1313_0000 | G#5000 | soc_ctrl; clock select at +G#4000 / +G#4004 |
| PCS | G#1314_0000 | G#5000 | |
| UDBG | G#1315_0000 | G#D000 | external PLL at +G#C700 |
| PHY (PMA) | G#1316_0000 | G#3000 | |
| SYSREG_HSI2 | G#1302_0000 | G#2000 | not written by the ch1 zuma path |
| DBI | G#1340_0000 | G#38_1000 | iATU (unrolled) at DBI + G#30_0000 = **G#1370_0000** |
| CONFIG window | **G#60FF_E000** | G#2000 | the `config` reg (zuma-pcie.dtsi:85) |
| PCIe MEM | G#6000_0000 | G#FF_0000 | identity mapped (zuma-pcie.dtsi:104) |
| PMU | G#1546_0000 | | ch1 PHY control **G#1546_3EC8** (`pmu-offset`); ch2 +G#3ECC. **EL3 SMC only** |
| CMU_HSI2 | G#1300_0000 | G#8000 | SFR:37 |
| CMU_TOP | G#2604_0000 | G#8000 | SFR:11 |
| GPIO_HSI2 (gph3) | G#1304_0000 | | gph3 at offset 0 (`pinctrl-gs.c:283`) |
| GPIO_FAR_ALIVE (gpa5) | G#154E_0000 | | gpa5 at offset 0 (`pinctrl-gs.c:246`) |

Board DT for ch1: `num-lanes=1`, `max-link-speed=3`, `ep-device-type=EP_BCM_WIFI (1)`, `perst-delay-us=15000`, `use-msi="true"`, `use-l1ss="true"`, `pcie,wlan-gpio=<gph3 4>` (zuma-board-pcie.dtsi:31-43). `use-sysmmu = "disabled"` is an invalid string the driver treats as false (RC:2202-2215). `pcie-clk-num = phy-clk-num = 0` (zuma-pcie.dtsi:108-109): Linux never touches a CMU clock for PCIe.

## 1. Power domain

The `pcie_1` node has no `power-domains`. zuma-pm-domains.dtsi defines only `pd_hsi0` (G#1546_2A80) and `pd_hsi1` (G#1546_2B00, disabled); there is no pd_hsi2 (ZDT/zuma-pm-domains.dtsi:209-229). `pm_runtime_get_sync()` (RC:5658-5659) is a power no-op: **no ACPM/PMU power-domain call for ch1**. HSI2 also hosts UFS, which ferros already drives, so it is up.

## 2. Clocks (CMU)

Linux relies on the bootloader state plus hardware auto clock gating: the GEN3A_1 Q-channels are `VCLK_GATE` without `VCLK_QCH_DIS` (`clk-zuma.c:465+`), and `cal_vclk_enable`/`disable` return immediately for them (`clk/gs/composite.c:1247, 1282`). **Read these first, force them on only if gated.**

Gate register bits (SFR:8745-8936): bit 20 MANUAL, bit 21 CG_VAL, bit 28 ENABLE_AUTOMATIC_CLKGATING. "Clock on" = MANUAL=1 + CG_VAL=1, or MANUAL=0 + bit 28 = 0 (`cal-if/ra.c:412-433`). Q-channel bits (SFR:14149-14169): bit 0 ENABLE, bit 1 CLOCK_REQ, bit 2 IGNORE_FORCE_PM_EN.

| Register | Address | Purpose |
|---|---|---|
| GAT PHY_REFCLK_IN (GEN3A_1) | G#1300_201C | parent `MUX_CLKCMU_HSI2_PCIE_USER` |
| GAT DBI_ACLK / MSTR_ACLK / SLV_ACLK | G#1300_2088 / 208C / 2090 | |
| GAT SUB_CTRL APB / PHY_UDBG APB / PIPE_PAL APB | G#1300_2094 / 2098 / 209C | |
| GAT SF_PCIEPHY QCH_TM APB | G#1300_2020 | |
| GAT UASC_PCIE_GEN3A_DBI_1 ACLK/PCLK | G#1300_20EC / 20F0 | NOC→DBI path |
| GAT UASC_PCIE_GEN3A_SLV_1 ACLK/PCLK | G#1300_20F4 / 20F8 | NOC→config/MEM windows |
| GAT QE_PCIE_GEN3A ACLK/PCLK | G#1300_20BC / 20C0 | |
| QCH APB / AXI / DBG / PCS_APB / PMA_APB / UDBG | G#1300_3058 / 3060 / 3068 / 306C / 3070 / 3074 | |
| DMYQCH REF | G#1300_3000 | |
| QCH UASC DBI_1 / SLV_1, QE_PCIE_GEN3A | G#1300_30B4 / 30B8 / 3088 | |
| PLL_CON0_MUX_CLKCMU_HSI2_PCIE_USER | G#1300_0620 | MUX_SEL bit 4: 0 OSCCLK_HSI2, 1 CLKCMU_HSI2_PCIE (SFR:4834; cmucal-node.c:1161-1164). Preserve |
| CMU_TOP MUX / DIV / GATE CLKCMU_HSI2_PCIE | G#2604_10B4 / 18AC / 20DC | parents OSCCLK_CMU / PLL_SHARED2_D2 (cmucal-node.c:385-388). Preserve |

**[INFERENCE]** The PHY refclk comes from the UDBG "External PLL" fed from 38.4 MHz (CAL:333-338, 382-385), not this CMU path; the PHY_REFCLK_IN gate still feeds the sub-controller. **[AMBIGUOUS]** reset/ABL values of these gates — read and log them before the first DBI access; a gated block can hang the bus.

## 3. GPIO / pinmux

gph3 = `bank_type_6` (`pinctrl-gs.c:59-62`): CON +0 (4 bits/pin), DAT +4, PUD +8 (4 bits), DRV +C (4 bits), CONPDN +10 (2 bits), PUDPDN +14 (4 bits). gpa5 = `bank_type_7` (`pinctrl-gs.c:67-70`). PULL_NONE 0, PULL_DOWN 1, PULL_UP 3, PDN_PREV 3; DRV_X1 0, DRV_X4 3.

| Pin | Use | CON G#1304_0000 | PUD G#1304_0008 | DRV G#1304_000C | Source |
|---|---|---|---|---|---|
| gph3-0 | PERST# (output) | [3:0] = 1 | — | [3:0] = 3 | zuma-pinctrl.dtsi:941-946 |
| gph3-1 | CLKREQ# (func 2) | [7:4] = 2 | [7:4] = 3 | [7:4] = 3 | zuma-pinctrl.dtsi:932-939 |
| gph3-4 | WL_REG_ON (output) | [19:16] = 1 | [19:16] = 0 | [19:16] = 0 | zuma-board-wlan.dtsi:24-31 |
| gph3-5 | wl_dev_wake (input, pull-down) | [23:20] = 0 | [23:20] = 1 | 0 | zuma-board-wlan.dtsi:33-40 |
| gpa5-1 | wl_host_wake (EINT, pull-down) | G#154E_0000 [7:4] = G#F | G#154E_0008 [7:4] = 1 | 0 | zuma-board-wlan.dtsi:12-21 |

PERST and WL_REG_ON are driven raw (the DT active-low flag is ignored): 1 = high = PERST# deasserted / WL_REG_ON on (RC:3584, 3694; DHD/dhd_custom_google.c:579, 623). PERST = DAT G#1304_0004 bit 0; WL_REG_ON = DAT G#1304_0004 bit 4.

## 4. Linux order of operations

1. RC probe (RC:5572-5805): WL_REG_ON low (RC:2317-2323), PERST output-low (RC:2353-2358), `resumed_phydown` (RC:5714 → 3133-3158: IRQs off, PMU isolation release, PHY power-down, ch2 PHY off), DBI RC setup with the PHY down.
2. dhd init: WL_REG_ON high, then `msleep(WIFI_TURNON_DELAY = 200 ms)` (DHD/dhd_custom_google.c:563-588; DHD/dhd.h:3472-3474).
3. dhd enumerate: `set_carddetect(1)` → `exynos_pcie_pm_resume(1)` → `exynos_pcie_rc_poweron(1)` (DHD/dhd_linux_platdev.c:731 → dhd_custom_google.c:678 → RC:4144-4155 → RC:3809).
4. poweron: pinctrl active (RC:3876), soc+4000 = G#15 (RC:3879), PHY power-down clear (RC:3882), `establish_link` (RC:3896 → 3551).
5. PCI rescan → dhd probe → chip-id read (§11).

Net timing: WL_REG_ON↑ → ≥200 ms → PHY bring-up → PERST↑ → **15–17 ms** (RC:3589-3590) → LTSSM enable → L0 within ~48–58 ms. If you toggle WL_REG_ON, hold it low ≥20 ms (DHD/wb_regon_coordinator.h:89).

## 5. Step A: ch1 PHY into a known state

| # | Action | Address | Value | Source |
|---|---|---|---|---|
| A1 | ELBI IRQ0/1/2_EN | G#1312_0010 / 0014 / 0018 | 0 | RC:2552-2554 |
| A2 | **PMU PHY isolation release (EL3 SMC)** | G#1546_3EC8 | RMW mask G#1, val G#1 (bit 0 = `PCIE_PHY_BYPASS`) | RC:393-407, CMNH:52-53, RCH:121-122 |
| A3 | phy+204 &= ~G#C | G#1316_0204 | RMW | CAL:50-51 |
| A4 | phy+1044/1048/104C/1050/185C | | G#2A, G#AA, G#A8, G#80, G#0A; 1 µs | CAL:57-62 |
| A5 | phy+208 | G#1316_0208 | G#FF; 1 µs | CAL:64-65 |
| A6 | phy+580, phy+928 | | G#0A, G#AA | CAL:67-68 |
| A7 | phy+00C | G#1316_000C | G#0A; 50 µs | CAL:71-73 |
| A8 | UDBG C700 &= ~bit 0 | G#1315_C700 | RMW; 10 µs | CAL:75-77 |

SMC ABI (`exynos-pd_el3.c:66-77`; `exynos-smc.h:45`; `exynos-el3_mon.h:21-23`): x0 = G#8200_0504, x1 = address, x2 = 2 (RMW), x3 = mask, x4 = value; x0 = 0 on success.

## 6. Step B: WL_REG_ON and pinmux

B1 WL_REG_ON low, PERST low (RC:2322, 2357) → B2 WL_REG_ON high (DAT bit 4) → B3 **200 ms** → B4 §3 pinmux → B5 SOC+4000 = **G#15** (G#1313_4000, RC:3879).

## 7. Step C: PHY power-down clear (`phy_all_pwrdn_clear`, ch1)

C1 G#1315_C700 |= bit 0, 100 µs (CAL:164-166) → C2 G#1316_000C = 0 (CAL:167) → C3 G#1316_0928 = G#55, G#1316_0580 = G#02 (CAL:169-170) → C4 G#1316_0208 = 0 (CAL:172) → C5 G#1316_1044/1048/104C/1050/185C = 0, 10 µs (CAL:174-183).

## 8. Step D: zuma PHY configuration, ch1 (CAL:291-541, from `assert_phy_reset`, RC:3071-3086, 3565)

`customized-de-emphasis` absent (CAL:303-318 skipped); one lane (CAL:415-416).

| # | Address | Value / op | Delay / poll | CAL |
|---|---|---|---|---|
| D1 | G#1316_032C | G#3 | | 320 |
| D2 | G#1315_C700 | \|= bit 0 | | 322-323 |
| D3 | G#1315_C700 | &= ~bit 1 | | 325-326 |
| D4 | poll G#1315_C734 bit 2 (EXT_PLL_LOCK) | | 10 µs × ≤10000 (Linux only logs) | 329-330, 244-274 |
| D5 | G#1313_4004 | &= ~bit 2 (external PLL) | | 334-335 |
| D6 | G#1313_4004 | \|= G#30 | | 337-338 |
| D7 | G#1312_0080 | G#4 (RC) | | 341 |
| D8 | G#1312_03A4 | G#F, then G#D | 10 µs | 344-346 |
| D9 | G#1312_03A4 | G#F | 10 µs | 347-348 |
| D10 | G#1312_1404, 1408, 1400 | 1,1,1 then 0,0,0 | | 351-356 |
| D11 | G#1312_03D8 | 1 | | 359 |
| D12 | PHY +0000=G#88, +001C=G#66, +01F4=0, +0514=G#59, +051C=G#11, +062C=G#0E, +0644=G#22, +0688=G#03, +06D4=G#28, +0788/078C=G#64, +0790/0794=G#50, +0944/0948/094C/0950=G#05 | | | 364-380 |
| D13 | +0590=G#02, +07F8=G#B0, +0730=G#08 | | | 383-385 |
| D14 | +0344=G#C0 | | | 388 |
| D15 | +0040=G#04, +0204=G#03, +02D4=G#1F, +0358=G#10 | | | 391-394 |
| D16 | +0018=G#01 | | | 397 |
| D17 | +0514=G#5B, +0608=G#0C, +060C/0610/0614/0618=G#0F, +0510=G#80, +0688=G#03, +0644=G#23, +0624=G#11 | | | 400-409 |
| D18 | +0630=G#0F, +06D0=G#53 | | | 412-413 |
| D19 | lane: +1140/1144/1148=G#04, +114C=G#02, +1150..115C=0, +12CC=G#1C, +12DC=G#6C, +130C=G#29, +13B4=G#2F, +1A64/1A68/1A84/1A88=G#05, +1A98/1A9C=0, +1AA8=G#07, +1AB8/1ABC=0, +1AF8=G#90, +1B34/1BB0/1BB4=G#03, +1BC0/1BC4=G#06, +1BE8=G#01, +1BF8=G#04, +1C98=0, +1CA4=G#81 | | | 418-448 |
| D20 | BCM: +1B20=G#02, +1340=G#23, +002C=G#3C | | | 451-455 |
| D21 | +1350 \|= 1 | | | 465-467 |
| D22 | PCS +190=G#100B_0604, +154=G#0007_00D5, +100=G#1640_0000, +104=G#0860_0000, +114=G#1850_0000, +124=G#6070_0000, +174=G#7, +178=G#100 | | | 470-480 |
| D23 | BCM: PCS +17C=G#700 | | | 482-483 |
| D24 | BCM: PCS +110=G#0160_0202 | | | 510-511 |
| D25 | G#1312_1404 = 1 | | 10 µs | 515-516 |
| D26 | G#1312_1408 = 1, G#1312_1400 = 1 | | | 517-518 |
| D27 | poll G#1316_0A80 bit 0 (PLL lock) | | ≤100 ms, logged | 521-522 |
| D28 | poll G#1316_15C0 bit 4 (CDR lock) | | | 524-525 |
| D29 | poll G#1316_140C bit 7 (offset cal done) | | | 530-531 |
| D30 | G#1316_032C = 0 | | | 537 |
| D31 | G#1312_1078 = 1 | | | 540 |

## 9. Step E: RC setup, LTSSM, link-up (`establish_link`, RC:3551-3807)

| # | Address | Value | Source |
|---|---|---|---|
| E1 | PMU G#1546_3EC8 (EL3) | RMW mask G#400 val G#400 (bit 10, undocumented) | RC:3567-3568 |
| E2 | G#1315_C800 | G#421 (ch1) | RC:3570-3576 |
| E3 | G#1316_032C | 0 | RC:3578 |
| E4 | DBI G#1340_0890 | G#1_2000 (EQ off) | RC:3581; DWHDR:100-103 |
| E5 | PERST# high (G#1304_0004 bit 0) | | RC:3584 |
| E6 | wait 15–17 ms | | RC:3589-3590 |
| E7 | G#1312_03BC \|= G#11 | | RC:3592-3595 |
| E8 | G#1312_03A0 = G#2 | | RC:3596 |
| E9 | G#1312_0074 = 1 | | RC:3597 |
| E10 | G#1312_03A8 &= ~G#FFF | | RC:3600-3607 |
| E11 | G#1312_0474 = 1 | | RC:3610 |
| E12 | G#1312_1078 = 1 | | RC:3615 |
| E13 | `dw_pcie_setup_rc` (below) | | RC:3620 |
| E14 | `exynos_pcie_setup_rc` (below) | | RC:3621, 3160-3230 |
| E15 | G#1312_0C00 &= ~bit 2; 0C04 = 0C08 = G#FFFF_FFFF; 0C00 \|= 3 | | RC:3637-3647 |
| E17 | **LTSSM enable** G#1312_0054 = 1 | | RC:3669 |
| E18 | **poll** G#1312_02C8 & G#3F == **G#11 (L0)** | 10–12 µs × ≤4800 | RC:3670-3681 |

E13 (DBI offsets, DWH:712-787, DW:620-706): G#08BC |= 1; LNKCTL2 G#00A0 [3:0] = 3; LNKCAP G#007C [3:0] = 3; G#0710 clear bit 7, set bit 5, [21:16] = 1; G#080C [12:8] = 1; BAR0 G#0010 = 4, BAR1 G#0014 = 0; G#003C [15:8] = 1; G#0018 = (old & G#FF00_0000) | G#FF_0100; G#0004 low 16 = G#107; G#0010 = 0; G#000A (16-bit) = G#0604; G#080C |= bit 17; G#08BC &= ~1. (The DWC iATU setup is skipped; exynos programs it, §10.)

E14 (RC:3160-3230): G#08BC = 1 (stays); 16-bit G#0000 = G#144D, G#0002 = G#ECED; LNKCAP clear [17:15],[3:0], set G#3_8000 | 3; G#0B40 = G#1A; G#0B44 = G#EA; G#0044 = 0; LNKCTL2 [3:0] = 3; DEVCTL2 G#0098 [3:0] = 5.

LTSSM (CMNH:71-105): DETECT_QUIET 0 → DETECT_ACT 1 → POLL_ACTIVE 2 → POLL_CONFIG 4 → CFG G#7–G#B → CFG_IDLE G#C → **L0 G#11**; recovery G#D–G#10 during the speed change. On L0: wait ~3 ms, DBI G#1340_0080 speed [19:16] / width [25:20]. Linux retries up to 10 times unless Gen3 — **skip that check for Gen1-or-better**. **[INFERENCE]** Force Gen1: LNKCAP SLS = 1, LNKCTL2 TLS = 1, no G#080C bit 17. Then clear ELBI G#1312_0000/0004/0008 by write-back, program the iATU (§10), G#1312_0074 = 0.

## 10. Step F: iATU and config read of 01:00.0

Unrolled iATU at G#1370_0000 + region × G#200 (RCH:299-321).

Outbound 0, CFG0 (RC:1644-1659): +08 LOWER_BASE = G#60FF_E000, +0C = 0, +10 LIMIT = G#60FF_FFFF, +14 LOWER_TARGET = **G#0100_0000** (bus 1 << 24 | dev << 19 | fn << 16), +18 = 0, +00 CR1 = G#4 (CFG0), +04 CR2 = G#8000_0000.

Outbound 1, MEM (RC:1661-1681): +200 CR1 = 0, +208 = G#6000_0000, +20C = 0, +210 LIMIT = G#601F_FFFF (Linux: 2 MB; **[INFERENCE]** use G#60FE_FFFF to cover the window), +214 = G#6000_0000, +218 = 0, +204 CR2 = G#8000_0000.

Config read (RC:1798-1854): G#1312_0074 = 1; reprogram outbound 0 target; `readl(G#60FF_E000 + where)`; G#1312_0074 = 0. Expected: vendor **G#14E4**, device **G#4444** (`BCM4398_D11AX_ID`) or G#4398 (DHD/include/bcmdevs.h:29, 109; dhd_pcie.c:17741-17742). The RC's own DBI G#1340_0000 should read **G#ECED_144D** after E14.

## 11. Step G: BARs and dhd's first BAR0 read

Assign EP BARs inside G#6000_0000..G#60FE_FFFF: BAR0 (64-bit, cfg G#10/G#14; dhd maps 32 KB), BAR "1" (resource 2, cfg G#18/G#1C; TCM, ≥4 MB), set the RC type-1 memory base/limit at DBI G#0020, then EP cfg G#04 |= G#6 (DHD/dhd_pcie_linux.c:2030-2060). First BAR0 access (dhd_pcie.c:1391 → 1323 → 4437-4452): EP **config write G#80 = G#1800_0000** (`PCI_BAR0_WIN` = `SI_ENUM_BASE_DEFAULT`), then **read32(BAR0 + 0)** = chipcommon ChipID: chip = value & G#FFFF → **G#4398**, revision = [19:16].

## 12. S2MPU / SysMMU

SysMMU `sysmmu_hsi2` G#131C_0000 is disabled; S2MPU `s2mpu_s0_hsi2` G#131F_0000 (always-on; pKVM module not in tree, state as ABL/EL3 left it — **[AMBIGUOUS]**). Neither affects goals 1–3 (CPU→EP accesses); both matter for EP→DRAM DMA later.

## 13. What silently breaks the link / open questions

1. PMU G#1546_3EC8 bit 0 left 0 keeps the PHY isolated; bit 10 (E1) is set before PERST, meaning undocumented.
2. External PLL not locked (G#1315_C734 bit 2): Linux continues after 100 ms; check it yourself.
3. SOC+4004 bits [2],[5:4] wrong → TCXO instead of the external PLL; SOC+4000 = G#15 undocumented.
4. UDBG G#1315_C800 = G#421 undocumented, ch1-specific.
5. PMA reset (ELBI G#1400/1404/1408) and soft_pwr_rst (ELBI G#3A4) ordering and 10 µs gaps.
6. Gated CMU gates/Q-channels on the DBI/SLV/UASC/QE path → bus hang or SError.
7. PERST timing: WL_REG_ON high ≥200 ms before PERST↑, then 15–17 ms before LTSSM.
8. Refclk: CLKREQ pulled up on function 2; **[INFERENCE]** the SoC PHY drives the EP refclk ("ERIO on", D16).
9. Linux's Gen3 insistence — skip.
10. Lock polls D27–D29: log, don't abort.
11. INT bus ≥200 MHz while linked (`pcie-pm-qos-int`) — likely not needed for link-up.
12. wl_dev_wake is an input; bcm4398 never drives it.
13. BAR placement vs the 2 MB MEM iATU limit (§10).
14. SYSREG_HSI2: the ch1 path writes nothing there.
15. **ferros-specific:** on husky, reading an unpowered block makes the next EL3 call hang (BOOT.md EL3 poison rule) — read the CMU gates (§2) before touching ELBI/DBI.
