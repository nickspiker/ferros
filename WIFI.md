# WIFI — BCM4398 on PCIe ch1 (husky / zuma): bare-metal bring-up recipe

**Status (2026-10-15): goals 1-3 reached on husky, cold boot, from a driver process.** `tools/husky-wifi.sh` runs `payloads/pcie1` (EL0, hardware grants only) through the recipe below:
- survey: every PCIe ch1 CMU gate already on (G#0020_0000: CG_VAL=1, MANUAL=0, no auto-gating), PMU PHY control G#C0 (isolated), DBI answers G#A5A5_144D;
- PHY: external PLL lock 1 µs, PHY PLL lock 97 µs, CDR lock 29 µs, offset calibration 6.7 ms;
- link: LTSSM Polling.Active → **L0 in 81 µs**, link status G#3011_0000 = **Gen1 ×1** (Gen1 forced), RC id G#ECED_144D;
- config space: **G#14E4 : G#4444** (BCM4398 D11AX); BAR0 → chipcommon ChipID **G#6003_4398 = chip G#4398 rev 3**.
- NCI enumeration (stage 04 + `tools/nci-parse.py`, following nciutils.c nci_scan): EROM pointer G#1800_7118 (EROM1 G#1800_7000, OOBR G#1800_6000). Cores: chipcommon G#800 rev 76 @ G#1800_0000; PCIe2 G#83C rev 130 @ G#1800_1000; **ARM CA7 G#847 rev 13 @ G#1808_0000** (OOBR +G#240, DMP control at +8); **sysmem G#849 rev 14 @ G#1808_8000 with RAM at backplane 0..G#29_FFFF and G#2A_0000 + 4 MiB**; D11 G#812 ×3 @ G#1808_2000/4000/6000; all interfaces Booker. (The download path: hold the CA7 in reset with CPUHALT, write the image, NVRAM vars + random seed, the reset instruction, release — dhd_pcie.c `_dhdpcie_download_firmware` / `__dhdpcie_bus_download_state`; no FW_SIGNATURE on this board.)
- measured before writing (stage 05): BAR0 64 KiB, BAR1 4 MiB (both 64-bit); DAR security status G#0 (no secure boot, no signature file); CA7 core info G#D047, DMP control G#21 (clock on + CPUHALT), AMNI reset control bit 0 clear — after WL_REG_ON the ARM is already out of reset and halted, so dhd's core reset is skipped. OTP CIS (G#1801_115C..): vendor tuple `80 03 81 57 99` → VID 57 99 = module **USI G5SN V17** (`bcmdhd.cal_USI_G5SN_9957_V17`, byte-identical to `cal_ROW`/`JP`/`MMW`; `cal_NA` differs only in FTM ranging calibration). Chip rev 3 = **d0** (dhd_custom_cis.c chip_revs).
- RAM (stage 06): sysmem coreinfo G#0B20_1000 → 32 banks × 128 KiB = **4 MiB at backplane G#2A_0000** (dhd's NCI default: the sysmem's second slave port). BAR1 at G#6040_0000, window = cfg G#84 (4 MiB-aligned backplane base, dhdpcie_setbar1win); write/read round trips pass in both windows.
- **Firmware boots (stage 07-08, 2026-10-15):** `fw_bcmdhd.bin_4398_d0` written at G#2A_0000 and verified word for word (3.9 s through the syscall path), NVRAM vars below the length token at the top of RAM (`process_nvram_vars`, token `(~w << 16) | w`), a 128-byte RNG seed rTLV (type G#FEEDC0DE, no length checksum) drawn from the session wairua through the kernel's entropy capability, reset instruction (the image's first word) at address 0, CPUHALT cleared in the OOBR DMP control. The last RAM word first changes after 36 ms to an early, temporary value; the **real shared area appears at ~430 ms** (G#0061_0000: flags G#7025_0509 = IPC v9, console G#0052_375C, rings_info G#0061_008C, h2d/d2h mailboxes G#0061_0600/0604) — dhd only ever sees the second, because it reads after the boot interrupt. The firmware console (hnd_cons_t log ring) reads: `RTE (PCIE-MSGBUF) 24.100.671.82.2 (g998bb28 d0 release2) on BCM4398 r3 @ 39.960/258.400/1411.0MHz`, wl0 (5G) / wl1 (2G) / wl2 (2G+5G) attached, `Firmware boot took 429457 us`.
- **MSI:** the RC's DesignWare MSI receiver (DBI G#820/G#824 address, G#828 enable, G#82C mask, G#830 status) and the EP's MSI capability (cfg G#58, 64-bit) both point at G#F000 (boot ROM space, so a write that ever escaped interception lands nowhere); the boot-complete MSI arrives as RC MSI status bit 0, and the firmware logs `Single MSI enabled`. **The firmware then runs idle without trapping, waiting for the host's rings.**
- **Trap to avoid:** once booted the firmware disables backplane G#0..G#29_FFFF (reads return all ones); a *host* read there through BAR1 is an AXI slave error on the dongle's s_pcie master and the firmware takes a FIQ trap (`bp_to=2`). The shared area's flags carry it (PCIE_SHARED_TRAP G#400, trap_addr non-zero). Don't touch address 0 after release.
- **First ioctl (stage 09):** the five common rings in the fenced `pcie1-dma` region (dhd's smallest sizes: ctrl submit 64 × 40, rxpost 512 × 32, ctrl complete 64 × 24, tx complete 1024 × 24, rx complete 512 × 40), read/write indices left in TCM (rings_info pointers; no DMA'd index arrays, so every completion is checked by epoch = seq % 253 from 254 and a zero XOR over its six words), host_cap G#0080_1409 (IPC 9, hostready, deep sleep off, no trap on Unsupported Request) at shared +84, an 8-byte scratch at +52/+56, hostready on H2D doorbell 1 (BAR0 + G#2144), 8 ioctl response buffers (8 KiB) and 8 event buffers (2 KiB) posted, then WLC_GET_VAR (262) "ver" with doorbell 0 (BAR0 + G#2140): ack after 5.2 ms, completion 7 µs later — `wl0: Nov 21 2025 14:44:15 version 24.100.671.82.2 (g998bb28 d0 release2) FWID 01-eed23c89`. `cur_etheraddr` answers too (stage 10).
- **Scan (stage 11, first run):** `country` reads `#n` (no regulatory table in the firmware) → `clmload` of `bcmdhd_clm.blob_4398_d0` in 1408-byte `wl_dload_data_t` chunks (flag 1 << 12 | begin 2 / end 4, type 2) → status 0, `country` then reads `US`; `event_msgs_ext` GET/SET with WLC_E_ESCAN_RESULT (69) added; WLC_UP (2) → 0; `scan_ver` = 3 (the v3 escan params: version/length prefix, 72 bytes); `escan` (version 3, action 1, broadcast SSID, BSSID ff…ff, any BSS type, firmware-default times, all channels) → events of type 69 with status 8 (one BSS each) and a final 0. **8 BSSes on 2.4, 5 and 6 GHz in 4.7 s.** Event packets are bcm_event_t (ether type G#886C, big-endian wl_event_msg_t, data at +72; rx_dataoffset from shared +36); wl_bss_info is read only through its version-independent prefix (chanspec +72, RSSI +78, ctl_ch +88, ie_offset +116 / ie_length +120).
- **Join (stage 12, 2026-10-15): associated and keyed in 241 ms**, WPA2-PSK on 5 GHz, the firmware's own supplicant doing the 4-way handshake. Order (dhd wl_cfg80211_connect): receive buffers posted (rxpost ring 1, 64 × 2 KiB) → `sup_wpa` 1, `roam_off` 1, `buf_key_b4_m4` 1, WLC_SET_PM (86) 0, WLC_SET_INFRA (20) 1 → `wsec`/`auth`/`wpa_auth` cleared → `wpa_auth` G#C0, `auth` 0, `wsec` 4 (AES), `mfp` (1 if the AP's RSN caps have MFPC), `wpa_auth` G#80 (WPA2-PSK) → WLC_SET_WSEC_PMK (268): 263-byte wsec_pmk_t, key_len, flags 1 (passphrase), the passphrase → our RSN IE via `wpaie` (version 1, the AP's group cipher, pairwise CCMP, AKM PSK, caps) → `join_ver` (2 here) → `join` (v2: ssid +4, scan params +40, BSSID +64, bssid_cnt +70, chanspec_num +76, one chanspec +80). Events: 87, AUTH 3, 88, LINK 16 (flags 1), ASSOC 7, then **PSK_SUP 46 status 6 (KEYED) reason 0** = authorized. The credential is `keys/wifi.txt` (SSID line, passphrase line), an input file of the driver, never printed or committed.
- Two lessons: **with no receive buffers posted the handshake never starts** (split RX: the AP's EAPOL frames land in host buffers; associated, no PSK_SUP, beacon loss after 15 s). And a first 2.4 GHz join (40 MHz, control 11) lost the link waiting for the first beacon (WLC_E_LINK reason 8, NO_FIRST_BCN_RX) — before rx buffers existed, so not yet re-tested; the driver prefers 5 GHz.
- **DHCP (stage 13, 2026-10-15): the phone leases an address** — OFFER and ACK from the router, a /24 with router, DNS and a ~24 h lease — over a transmit flowring from the EL0 driver. Receive: a frame starts at `data_offset` if non-zero, else at pcie_shared `dma_rxoffset` (8 here), as a plain Ethernet frame (the "wrong EtherType" was reading from offset 0; 802.11 management frames also arrive, flagged G#02). Transmit: flowring 2 (msgbuf ring 5, H2D index slot 2) created on the control ring with MSG_TYPE_FLOW_RING_CREATE (G#03: DA broadcast, SA our MAC, flow_ring_id 2, 512 × 48 B, ring address) → create completion G#04 status 0; each frame is host_txbuf_post v1 (G#0F, 48 B: the 14-byte Ethernet header inline, flags G#01, seg_cnt 1, data address past the header, data_len − 14) + the flowring's TCM write index + doorbell 0 `G#FF000000 | 5 << 16 | wr`; tx completions (G#10) on D2H ring 3 report status 0. DISCOVER/REQUEST are broadcast with the BOOTP broadcast flag and a zero UDP checksum.
- **Photon on ferros needs only ARP + IPv4 + IPv6 + UDP** (decided with the owner 2026-10-15; IPv6 with neighbour discovery and SLAAC, as Photon binds dual-stack): no fgtw.org bootstrap, no seed, no TCP/HTTP/TLS/DNS. Peers come from physical pairing into the fleet (the phonebook is handed over and kept in the vault) and LAN broadcast discovery; addresses are hints, identity is proven by signatures. Next: the WiFi driver as a frame server, an IP process (ARP, IPv4, DHCP, IPv6 NDP/SLAAC, UDP) serving UDP to apps, then a no_std Photon core — each its own process.
- **The network stack runs (2026-10-15, `tools/husky-net.sh`):** three EL0 processes over capabilities — the WiFi driver serving Ethernet frames on `wlan` (stage 14: SEND / RECV / INFO; received frames wait in their DMA buffers until taken; `allmulti` on for IPv6 neighbour discovery), `payloads/ip` (ARP, IPv4 + DHCP + echo, IPv6 NDP + router solicitation + SLAAC + echo, UDP; serves `udp`), `payloads/nettest` (UDP echo on 7777). DHCP lease and a SLAAC global IPv6 address; from another machine: ping 3/3 over IPv4 and IPv6, UDP echo round trips over both. The firmware offers TX checksum offload (flags2 TXPOST_EXT) — not used yet; `ip` sums in software (RFC 1071). Rx frames: length = data_len from the offset (subtracting the offset clipped 8 bytes); completion flags don't separate 802.11 from Ethernet frames, so all go to `ip`.
- **At boot from the vault (2026-10-26):** init starts the WiFi driver and `ip` after the shell; the driver, given no input files, takes its firmware, NVRAM, CLM blob and credential from the vault scope `wifi.` and runs the whole bring-up (keyed about 8 s after its start). Both loops sleep 1 ms when idle instead of spinning a core. A restart of the driver re-joins at once, and the router then refuses a second join for about ten minutes (devbox_network_for_phone_tests): when iterating on `ip`, restart `ip` alone.
- **Client TCP + DNS (2026-10-26, owner: "we probably need the full TCP/UDP/IP"):** `ip` grew a client-only TCP (ops 4-7 on `udp`; two connections, 4 KiB receive window, 2 KiB send buffer, MSS 1200, go-back retransmission 1 s doubling to 8 s; nothing listens) and passes on the DHCP-given DNS server (INFO +51). `payloads/httptest` resolves a name through it and fetches over plain HTTP: Broadcom's LTO file, **189 047 bytes in 1.9 s, FNV-1a 64 equal to the host's fetch**. The window, not the link, bounds that rate (4 KiB per round trip). This widens the 2026-10-15 "UDP only" line for HTTP fetches (orbit data); Photon itself still needs only UDP. No TLS.
- **UDP ports and multicast (2026-10-26):** datagrams are kept only for ports a process has opened (a RECV on it or a SEND from it, the last eight), the oldest giving way when the 8-slot queue is full — before, eight stray LAN broadcasts to unread ports filled the queue for good. IPv4 multicast (224/4) and IPv6 link-scope multicast (ff02::) are delivered: `payloads/photon` hears the stock app's `pt_disc` LAN beacons (239.104.199.144 / ff02::68c7:9014, port 4384), parsed by photon_core on the phone.
Then: msgbuf rings, CLM blob, 802.11 association, an IP stack — each as processes.

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

SysMMU `sysmmu_hsi2` G#131C_0000 sits in front of PCIe ch1 only (port PCIe_CH1) and is disabled (DT `status = "disabled"`, `pcie_1` `use-sysmmu = "disabled"`); Linux never programs it. S2MPU `s2mpu_s0_hsi2` G#131F_0000 (v9, always-on) gates every HSI2 master (UFS, PCIe ch1). Register model, from pkvm-s2mpu kvm_s2mpu.h / hyp s2mpu.c: CTRL_PROT_EN_PER_VID_SET G#50 / CLR G#54 (cold_ufs used to write CLR = G#FF: protection off for all 8 VIDs), DECERR-on-deny G#70, contexts G#100/G#104 (context i → VID i, before any L1 write), L1 per VID per GiB at G#4000 + vid·G#200 + gb·8 (table address = PA >> 4) / +4 (attr: bit 0 table, bits 2:1 R/W for a 1 GiB block), 4 KiB tables of 4 bits per page (bit 2 R, bit 3 W, 128 KiB per GiB, aligned to their size), GiB 4..33 fixed no-access, invalidation G#1000 + sync G#102C/G#1030, faults G#2000 (bit per VID) and per VID PA/info (VID [26:24])/info2 (stream ID [23:0]).

**The fence (ferros_kernel/src/s2mpu.rs, 2026-10-15):** every VID may reach only the cold window G#8000_0000..G#8240_0000 (kernel buffers for UFS, the DMA pool for the WiFi driver); every other GiB is no-access. Verified: UFS link, vault open/verified and the devinfo write work, the firmware boots, no faults. **Measured (stage 10):** an ioctl whose request buffer sits outside the fence latches a VID 0 permission fault (FAULT_INFO G#004F_0002, stream 4) — **PCIe ch1 is VID 0, and so is UFS** (it keeps working with VIDs 1..7 denied). So the fence is: VID 0 may reach UFS's own DMA pages (`.bss.dma.ufs`, 8 KiB) and `pcie1-dma` (2 MiB), nothing else — not the kernel, not the USB/PT buffers, not the S2MPU table (kept outside `.bss.dma`); VIDs 1..7 nothing. Verified: vault open/verified, devinfo write, ioctls, and a probe read of the kernel image denied (PA G#8008_0000). Left shared: the WiFi chip can reach UFS's 8 KiB; separating them needs the PCIe-only SysMMU (sysmmu_hsi2) or the HSI2 SSMT (address unknown) to move stream 4 to another VID. The dongle's PCIe core also has host-memory protection windows (HMAP, pcie_core.h `hmapwindow*` G#580.., violations at G#600..), a second, device-side layer.

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

## The keeper: the driver started again after a failed join (2026-10-29)

When a join fails, the driver exits (`join failed (see events)`). Until tonight nothing started it again before the next boot, so a phone that missed its first join had no WiFi for the session. On 2026-10-29 the home access point refused every join for an evening: dozens of reboots and reloads each rejoined, and it throttles repeated joins.

Now the kernel's service loop keeps it. Once the shell is up, a `pcie1` that is gone is started again from the vault (`diag restart pcie1`'s path, a full fresh bring-up). The first restart comes 64 s after it went, and the wait doubles with each restart up to 1024 s, since a refused join only adds to a throttle. The wait goes back to 64 s once the driver has stayed up that long. `diag kill pcie1` stops the keeper for the session.

Meanwhile the IP process stays on cellular. Its `wlan` calls carry CALL_LIVE (KERNEL.md), so they fail at once while no driver runs, and while a restarted one is still bringing the chip up. It asks again every 4 s and takes WiFi back the moment the driver serves.

## The MAC: a random one, not the calibration file's placeholder (2026-10-31)

Ferros never set a MAC, so the chip joined as its calibration file's `macaddr=00:90:4c:50:10:01`, Broadcom's placeholder (the same on every unit; Android sets the board's own address at every start).

From the evening of 2026-10-29, pangolin refused that address: the key exchange never started, or association failed with status 2. Nothing showed as blocked in the router's settings. The credential matched throughout.

A random address joined at once: WPA2 keyed, DHCP, SLAAC, fgtw.org over IPv6 and IPv4, ping replies at TTL 63.

**Now one address per network (owner, 2026-10-31).** The driver sets `cur_etheraddr` while the radio is still down. The value is the first six bytes of BLAKE3, keyed by the phone's ira-derived key (`wifi mac per network v1`, the Ira right as pcie1's slot 17, home slot 49), over the network's SSID, made locally administered unicast.
- It's the same on a network every time and different everywhere else, and can't be linked across networks without the ira. pangolin's is `fe:8c:8a:25:71:6e`.
- It's keyed on the SSID, not the access point's MAC: the SSID is known before the radio comes up, and one network answers from a different MAC on each band and each mesh node.
- Without the Ira right or an SSID, it falls back to a random address.

**The router throttles rejoins.** A join a few seconds after the previous session was cut off (a driver killed, so no disassociation) is refused. The keeper's retry 64 s later joined. That's probably what happened to the placeholder on 2026-10-29, after dozens of reload rejoins under one address.
