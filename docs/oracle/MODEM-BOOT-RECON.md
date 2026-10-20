# Modem (Shannon S5300 "CP") cold boot over PCIe ch0: source recon

This is a source-only reading of the GPL modem interface driver, the Exynos PCIe RC driver, and the zuma/husky device tree for the build that ships on husky (`android-gs-shusky-6.1-android15-qpr2`).
Nothing here has been run or captured on hardware yet.
Where the source cannot settle a value, the doc says so and names the oracle capture that would settle it.

Sources (all under `/mnt/Harbor/ferros-ref`):
- `radio-s5300` is `kernel/google-modules/radio/samsung/s5300` at branch `android-gs-shusky-6.1-android15-qpr2` (commit 9b16eed).
- `kdev-zuma` and `kdev-shusky` are `kernel/devices/google/{zuma,shusky}` at the same branch; they hold the authoritative DTS and defconfig.
- `soc-gs-6.1` is a `git archive` extract of `kernel/google-modules/soc/gs` at the same branch (PCIe RC, PHY cal, PCIe IOMMU, PMU/SMC helpers).
- `soc-gs` (branch 5.15-qpr1) was used only to cross-check; its DTS differs from `kdev-zuma` only in `srinfo_offset`/`srinfo_size` and a few iod attrs.

Build facts that select the code paths: `shusky_defconfig` sets `SEC_MODEM_S5100`, `LINK_DEVICE_PCIE_IOMMU`, `LINK_DEVICE_PCIE_IOCC`, `CP_PKTPROC(_UL)`, and does not set `CP_WRESET_WA`, `CP_SECURE_BOOT`, `GS_S2MPU`, `MCU_IPC`, `BOOT_DEVICE_SPI` or `CP_BTL` (kdev-shusky/shusky_defconfig:1-13).
So the non-`CP_WRESET_WA` branches of `modem_ctrl_s5100.c` are the live ones, and no SMC is compiled into the modem driver's boot path.

## Hardware map

| Thing | Value | Source |
|---|---|---|
| CP IPC shared memory (`cp_rmem`, region IPC + GNSS_FW) | DRAM G#EA40_0000, size G#C0_0000 (IPC = first G#80_0000, GNSS_FW = next G#40_0000) | kdev-zuma/dts/zuma-rmem.dtsi:109-113, zuma-cp-s5300-sit.dtsi:478-540 |
| MSI / boot-register page (`cp_msi_rmem`) | DRAM G#F620_0000, size G#1000, uncached | zuma-rmem.dtsi:115-119 |
| Packet processor DL+UL (`cp_rmem_1`) | DRAM G#E800_0000, size G#200_0000 | zuma-rmem.dtsi:121-125 |
| AoC voice window (`cp_aoc_rmem`) | G#197F_D000, size G#3000 (not needed to boot) | zuma-rmem.dtsi:127-131 |
| PCIe ch0 ELBI / SOC / UDBG / PHY / SYSREG / DBI / PCS | G#1210_0000 / G#1211_0000 / G#1213_0000 / G#1214_0000 / G#1202_0000 / G#1240_0000 / G#1212_0000 | kdev-zuma/dts/zuma-pcie.dtsi:12-24 |
| PCIe ch0 config window | CPU G#40FF_E000, size G#2000 | zuma-pcie.dtsi:22 |
| PCIe ch0 memory window | CPU G#4000_0000 = bus G#14E0_0000, size G#FF_0000 | zuma-pcie.dtsi:40 |
| CP doorbell (EP BAR0 target) | bus G#14E6_0000 (`pci_db_addr`) | zuma-cp-s5300-sit.dtsi:30 |
| PCIe HSI1 SysMMU (stage 1) | G#1206_0000 | zuma-pcie.dtsi:60-72 |
| PCIe HSI1 S2MPU (stage 2, pKVM-owned on Android) | G#1209_0000 | zuma-s2mpu.dtsi:85-90 |
| PMU (ALIVE) base | G#1546_0000; PCIe ch0 `pmu-offset` G#3EC4 | zuma.dtsi:464, zuma-pcie.dtsi:56 |
| GPIO_ALIVE / GPIO_HSI1 controllers | G#154D_0000 / G#1204_0000 | soc-gs pinctrl-gs.c:216-224, 274-277 |

## 1. Power-on / reset of the CP

Probe (`s5100_get_pdata`, modem_ctrl_s5100.c:2048-2125) requests every CP GPIO by DT name, makes every `AP2CP_*` an output driven 0 and every `CP2AP_*` an input (line 2104); DT polarity flags (G#1) are ignored, levels below are raw pad levels.

| Driver name | DT property | Pad | Reg bank (CON at +0, DAT at +4) | Dir | Pinctrl |
|---|---|---|---|---|---|
| AP2CP_CP_PWR | gpio_ap2cp_cp_pwr_on | gph1-2 | G#1204_0020 | out | pud 0, pdn-con 3 |
| AP2CP_NRESET | gpio_ap2cp_nreset_n | gph1-7 | G#1204_0020 | out | pud 0, pdn-con 3 |
| AP2CP_CP_WRST_N | gpio_ap2cp_cp_wrst_n | gph1-1 | G#1204_0020 | out | pud 0, pdn-con 3 |
| AP2CP_AP_ACTIVE | gpio_ap2cp_pda_active | gph1-0 | G#1204_0020 | out | pud 0, pdn-con 3 |
| AP2CP_WAKEUP | gpio_ap2cp_wake_up | gph0-2 | G#1204_0000 | out | pud 0, pdn-con 3 |
| AP2CP_DUMP_NOTI | gpio_ap2cp_dump_noti | gph0-3 | G#1204_0000 | out | pud 0, pdn-con 3 |
| AP2CP_PM_WRST_N | gpio_ap2cp_pm_wrst_n | gpa0-2 | G#154D_0000 | out | func 1, pud 0 |
| CP2AP_CP_WRST_N | gpio_cp2ap_cp_wrst_n | gpa0-1 | G#154D_0000 | in | func 0, pud 0 |
| CP2AP_PS_HOLD | gpio_cp2ap_cp_ps_hold | gpa6-4 | G#154D_00A0 | in | pud 0 |
| CP2AP_WAKEUP | gpio_cp2ap_wake_up | gpa6-5 | G#154D_00A0 | in (EINT) | func G#F, pull-down |
| CP2AP_CP_ACTIVE ("PHONE_ACTIVE") | gpio_cp2ap_phone_active | gpa7-0 | G#154D_00C0 | in (EINT) | func G#F, pud 0 |
| PCIe PERST# (RC driver) | `gpios` of pcie@12100000 | gph0-0 | G#1204_0000 | out, init low | func 1, drv X4 |
| PCIe CLKREQ# | pinctrl `pcie0_clkreq` | gph0-1 | G#1204_0000 | func 2 | pud 3, drv X4 |

Pin sources: kdev-zuma/dts/zuma-board-cp-s5300-sit.dtsi:39-154 and zuma-pinctrl.dtsi pcie0_clkreq/pcie0_perst (soc-gs copy at lines 843-858).
`gpa3-0`/`gpa3-1` are a bit-banged SPMI bus to the S5910 RF clock buffer, used only by `power_shutdown_cp` (s5910 `seq` G#215=G#0F, wait 1000 ms, G#20E=G#04, G#240=G#10); no `on_seq` exists in the husky DT, so it plays no part in power-on.
No regulator, clock, PMIC or SMC call appears anywhere in the modem driver's power-on path; the CP's own PMIC is sequenced purely by the GPIOs.

`mif_gpio_set_value(gpio, level, ms)` waits `ms` only if the pad actually changed (modem_utils.c:612-641).
`power_on_cp` (modem_ctrl_s5100.c:992-1037), reached from `IOCTL_POWER_ON` (bootdump_io_device.c:151-157), does in order:
1. `clear_boot_stage`: write 0 to G#F620_0010 (`boot_stage`) (lines 968-990).
2. AP2CP_AP_ACTIVE (gph1-0) = 1, AP2CP_DUMP_NOTI (gph0-3) = 0.
3. Zero `ap2cp_msg` (IPC+G#800) and `cp2ap_msg` (IPC+G#804).
4. `gpio_power_off_cp` (lines 906-918): WAKEUP=1 (10 ms), WAKEUP=0, NRESET=0, CP_WRST_N=0, CP_PWR=0 (30 ms), PM_WRST_N=0 (50 ms).
5. `gpio_power_offon_cp` (lines 921-934): PM_WRST_N=1 (10 ms), CP_PWR=1 (10 ms), NRESET=1 (10 ms), CP_WRST_N=1.

`IOCTL_POWER_RESET`(normal) instead runs `power_reset_cp` (modem_ctrl_s5100.c:1152-1181), which drops the PCIe link and runs only steps 4-5.
After step 5 the CP boot ROM runs and waits to be found on PCIe; the AP does nothing else until `IOCTL_START_CP_BOOTLOADER`.

## 2. PCIe ch0 link bring-up for the modem

Driver: `samsung,exynos-pcie-rc` (soc-gs-6.1/drivers/pci/controller/dwc/pcie-exynos-rc.c), DesignWare core, ip-ver G#98_6500, 2 lanes, max Gen3 (zuma-pcie.dtsi:12-58; board overrides zuma-board-pcie.dtsi:10-22 set `use-cache-coherency`, `use-msi`, `use-sysmmu`, `use-l1ss`, `perst-delay-us` = 18000, `ep-device-type` = EP_SAMSUNG_MODEM; shusky sets `support-msi64-addressing`).
`pcie-clk-num` and `phy-clk-num` are 0 and the node has no power domain, so Linux enables no clock or power domain for it: HSI1 runs on whatever the bootloader left (to be confirmed by an oracle dump of CMU_HSI1).
There is no PHY firmware: the PHY is a fixed MMIO register table.

The modem driver calls `s5100_poweron_pcie` (modem_ctrl_s5100.c:1758-1874), which sets the RC's MSI target to `cp_msi_rmem` G#F620_0000 (line 1816) and calls `exynos_pcie_poweron(0, spd, width)` with Gen1 x1 for the ROM stage and Gen3 x2 afterwards (line 1817).
`exynos_pcie_rc_poweron` (pcie-exynos-rc.c:3792-3960) in state LINK_DOWN:
1. Enable the HSI1 PCIe SysMMU with identity maps of every CP shmem region (`cpif_pcie_iommu_enable_regions`, link_device_pcie_iommu.c:16-51; iova == pa).
2. Select pinctrl "active" (PERST gph0-0, CLKREQ gph0-1), write SOC+G#4000 = G#15 (line 3862), run `phy_all_pwrdn_clear` (ch0 branch of pcie-exynos-zuma-rc-cal.c:154-224).
3. `exynos_pcie_rc_establish_link` (lines 3536-3790), below.
4. First time only: rescan bus, program MSI (`exynos_pcie_rc_msi_init`, lines 3367-3460): DBI G#820/G#824 = G#F620_0000, ELBI G#18 MSI edge select, DBI G#828 (MSI_INTR0_ENABLE) = G#F1, unmask in DBI G#82C.
5. PCS+G#150 = G#3_00D5 (PLL and bias always on, line 3943).

`exynos_pcie_rc_establish_link`, per attempt (up to 10):
1. PHY config, ch0 branch of `exynos_pcie_rc_pcie_phy_config` (pcie-exynos-zuma-rc-cal.c:533-714): PHY+G#32C = 3; UDBG G#C710 bit1 = 0 and G#C700 bit1 = 0 (external PLL init); poll UDBG G#C704 bit3 (ext PLL lock); SOC G#4004 bit2 = 0, bits[5:4] = 3; ELBI G#80 = 4 (RC); ELBI G#3A4 = F, D, F (soft power reset); PMA reset ELBI G#1404/G#1408/G#1400 pulse; a fixed table of PMA writes and the PCS table (PCS G#190, G#154, G#100-G#17C); release PMA reset; poll PHY G#A80 bit0 (PLL), G#15C0 bit4 (CDR), G#140C bit7 (offset cal), each up to 10000 x 10 us; UDBG G#C710 bits[9:8] = 3; PHY+G#32C = 0; ELBI G#1078 = 1.
2. PMU RMW: address G#1546_3EC4, mask G#400, value G#400, done by SMC (see section 4 table) (line 3552).
3. UDBG G#C800 bits[6:5] = 0; PHY G#32C = 0; DBI G#890 = G#1_2000 (equalisation off).
4. PERST# (gph0-0) = 1, then sleep `perst-delay-us` = 18 ms (lines 3569-3575).
5. ELBI G#3BC |= G#11, ELBI G#3A0 = 2, ELBI G#74 = 1 (xfer pending), clear ELBI G#3A8 bits[11:0] (Q-channel gating off), ELBI G#474 = 1, ELBI G#1078 = 1.
6. `dw_pcie_setup_rc` plus `exynos_pcie_setup_rc` (lines 3145-3215): DBI G#8BC = 1 (RO write enable), DBI shadow G#10_0010 and G#10_0030 = 0 (no RC BAR/ROM), vendor/device ID, LNKCAP L1 exit + 8 GT/s, DBI G#B40 = 26 MHz aux clock, L1SS timing at G#B44, PM_CTRL = 0, LNKCTL2 target speed, DEVCTL2 completion timeout 44 ms.
7. IOCC (`exynos_pcie_rc_set_iocc`, lines 1974-2028): DBI G#8E8 = G#1010_1010 and SYSREG G#1202_0704 bits[1:0] = 3.
8. Lane width in PORT_LINK_CONTROL and LINK_WIDTH_SPEED_CONTROL, then ELBI G#54 = 1 (LTSSM enable, line 3654).
9. Poll ELBI G#2C8 bits[5:0] for G#11 (L0) every ~10 us, max 4800 polls (~48 ms); on timeout PERST# = 0, ELBI G#54 = 0, retry.
10. If target > Gen1 wait 3 ms and require negotiated speed >= target; clear ELBI G#0/G#4/G#8 IRQ status; enable IRQs.
11. iATU: region 0 (DBI G#30_0000..) CFG0 from CPU G#40FF_E000 to bus 1 dev 0 fn 0; region 1 (DBI G#30_0200..) MEM from CPU G#4000_0000 (limit +2 MiB) to bus G#14E0_0000 (lines 1644-1681, 3782-3784); ELBI G#74 = 0.

EP side (`s51xx_pcie_probe`, s51xx_pcie.c:366-461): write EP BAR0 = G#14E6_0000, read it back, `dbaddr_offset` = G#14E6_0000 minus the readback, enable memory + bus master.
The doorbell is a 32-bit MMIO write to (root-port memory window start + `dbaddr_offset`) (lines 419-426), which with the DT window should be CPU G#4006_0000 = bus G#14E6_0000; confirm on the Linux oracle from the `db_addr : ... val : ... offset` and `doorbell_addr ... (start ... offset ...)` log lines.
On every later power-on `s51xx_pcie_restore_state` rewrites BAR0 to that value if the EP lost it (lines 264-273).
The MSI address range G#F620_0000 is captured by the DWC as MSIs; offsets G#10-G#1F of the same page are ordinary DRAM the CP writes over PCIe.

## 3. Boot download protocol

IPC region layout (base G#EA40_0000; offsets from DT, kdev-zuma/dts/zuma-cp-s5300-sit.dtsi:69-102 and link_device_memory_legacy.c:90-201):

| Offset | Content |
|---|---|
| G#0 | magic: AP writes G#BDBD (`SHM_BOOT_MAGIC`) before boot and reads it back |
| G#4 | mem_access |
| G#8 / G#C / G#10 / G#14 | FMT txq head / tail, rxq head / tail |
| G#18 / G#1C / G#20 / G#24 | RAW txq head / tail, rxq head / tail |
| G#64 | AP writes `srinfo_offset` = G#40_0000 here |
| G#70 | AP writes `capability_offset` = G#A0 here |
| G#A0 | AP/CP capability words (both 0 on husky) |
| G#800 / G#804 | `ap2cp_msg` / `cp2ap_msg` |
| G#808 / G#80C | `ap2cp_united_status` / `cp2ap_united_status` |
| G#824 / G#82C | ap2cp kernel time / handover block info |
| G#1000 | FMT tx (G#1000) then FMT rx (G#1000) |
| G#3000 | RAW tx ring, G#1F_D000 bytes (boot channel AP->CP) |
| G#1_0000 | ROM boot image (inside the RAW tx ring, used before the ring is) |
| G#20_0000 | RAW rx ring, G#20_0000 bytes (boot channel CP->AP) |
| G#40_0000 | SR info, G#30_0000 bytes |

MSI page registers (`struct msi_reg_type`, modem_ctrl.h:13-22) at G#F620_0000: +0 msi_data, +4 msi_check, +8 err_report, +C reserved, +10 boot_stage, +14 img_addr_lo, +18 img_addr_hi, +1C img_size.
`boot_stage` bits (modem_ctrl.h:24-43): 0 ROM, 1 PCI linkup start, 2 PHY init done, 3 DBI done, 4 LTSSM disable, 5 LTSSM enable, 6 MSI start, 7 wait doorbell, 8 download PBL, 9 PBL done, 10 security start, 11 check BL1 ID, 12 jump BL1, 13 done; success = G#3FFF.

Doorbell values are `int_ap2cp_* | G#1_0000` (modem_ctrl_s5100.c:735-747): message G#1_0000, PCIe link ack G#1_000E.
AP->CP commands go into `ap2cp_msg` as `cmd | G#C0`, then the message doorbell (link_device.c:2686-2719, link_device_memory.h:58-87,436-468): INIT_START G#C1, INIT_END G#C2, PHONE_START G#C8, CRASH_EXIT G#C9, PIF_INIT_DONE G#CD.
CP->AP uses MSI vector 0 (`cp2ap_msg` handler) and vector 1 (`cp2ap_united_status` handler) of 4 allocated (modem_ctrl_s5100.c:750-776); the command is read from IPC+G#804.

Sequence as driven by the vendor daemon `cbd` (closed source; it owns the image list and the boot-channel payload):
1. `IOCTL_POWER_ON` or `IOCTL_POWER_RESET`(normal): section 1 (which one `cbd` uses is not visible).
2. `IOCTL_LOAD_CP_IMAGE` (`link_load_cp_image`, link_device.c:2042-2120): copies the first image into IPC + round_up(G#3000, 64 KiB) = G#1_0000, i.e. DRAM G#EA41_0000, at most G#7F_0000 bytes, and records its size.
3. `IOCTL_START_CP_BOOTLOADER`(normal) runs `start_normal_boot` (modem_ctrl_s5100.c:1335-1412):
   a. `init_control_messages` (lines 642-679): write G#40_0000 to IPC+G#64 and G#A0 to IPC+G#70, zero both united-status words and the capability words, put DS-detect in `ap2cp_united_status` bits[15:14].
   b. AP2CP_AP_ACTIVE = 1; write and verify magic G#BDBD at IPC+G#0 (`link_start_normal_boot`, link_device.c:2551-2580).
   c. `register_pcie` (lines 806-900): identity-map shmem in the PCIe SysMMU, sleep 200 ms, PCIe on at Gen1 x1 (section 2), EP probe, request MSIs.
   d. `set_cp_rom_boot_img` (lines 1274-1305): G#F620_0014 = G#EA41_0000, G#F620_0018 = 0, G#F620_001C = image size, then doorbell G#1_0000.
   e. Poll G#F620_0010 for G#3FFF every 20 ms, 200 tries (`check_cp_status`, lines 1232-1272): the CP ROM pulls the image from AP DRAM over PCIe and jumps to it.
   f. PCIe off (`s5100_poweroff_pcie`, lines 1647-1756): wait 30 ms, AP2CP_WAKEUP = 0 (5 ms), RC poweroff (PME turn-off, PERST# low, LTSSM off, PHY power-down).
   g. Poll CP2AP_WAKEUP (gpa6-5) for 1, every 20 ms, 200 tries.
   h. PCIe on at Gen3 x2: AP2CP_WAKEUP = 1 (5 ms), RC poweron, restore EP config/BAR0, doorbell G#1_000E (link ack).
4. `cbd` then writes the remaining images to `/dev/umts_boot0` (channel 241, G#F1): each `write()` becomes frames with a 12-byte SIT header {u16 G#ABCD, u16 frame seq, u16 cfg = G#C000, u16 len incl. header, u8 ch = G#F1, u8 ch seq, bytes 10-11 unused} placed in the RAW tx ring with no doorbell; CP replies arrive in the RAW rx ring and are polled (bootdump_io_device.c:454-600, link_device.c:1416-1426,1901-1940, link_device_memory_legacy.c:271-330, modem_io_device.c:554-598, include/exynos_ipc.h:13-50).
   The file names, their order and the request/response format on channel 241 are not in any GPL source; they come from `cbd` and the CP bootloader.
5. The main CP firmware raises CP->AP INIT_START; the AP initialises pktproc (DL/UL rings in G#E800_0000) and answers PIF_INIT_DONE (link_device.c:457-520).
6. CP raises PHONE_START; the AP checks capabilities, re-inits the legacy link and answers INIT_END (if RIL has the channels open), then completes `init_cmpl` (link_device.c:526-640).
7. `IOCTL_COMPLETE_NORMAL_BOOTUP` (`complete_normal_boot`, modem_ctrl_s5100.c:1414-1481) waits up to 15 s for step 6, enables L1.2, arms the PHONE_ACTIVE (gpa7-0) and CP2AP_WAKEUP (gpa6-5) interrupts and marks the modem ONLINE.

Image source: the `modem` partition (A/B, ext4, by-name `modem`, mounted read-only at `/mnt/vendor/modem_img`; device/google/zuma conf/fstab.modem).
Exact file names inside it are not in the public sources; list the partition (readable cold via ferros UFS) or capture `cbd` from the Linux oracle.

## 4. Signing and verification

On this PCIe path the AP verifies nothing and makes no SMC for the modem images: `security_req` (`IOCTL_REQ_SECURITY`, SMC G#8200_0700 family) is wired only for `LINKDEV_SHMEM` (link_device.c:3720-3726, 2253-2350), and `CP_SECURE_BOOT` is off in the shusky defconfig.
The CP ROM reports a security step itself in `boot_stage` bits 10-12 (security start, check BL1 ID, jump BL1), so the first image is checked inside the CP before it runs.
Whether later images are checked by the CP bootloader is not visible from the AP side.

## 5. Minimal bare-metal sequence

| # | Step | Mechanism | EL3? |
|---|---|---|---|
| 1 | Keep DRAM G#EA40_0000+G#C0_0000, G#F620_0000+G#1000, G#E800_0000+G#200_0000 out of ferros' allocator; map IPC uncached or do cache maintenance | ferros memory map | no |
| 2 | Program the 11 CP pads and PERST/CLKREQ as in section 1 table, AP2CP outputs low | GPIO_HSI1 G#1204_0000, GPIO_ALIVE G#154D_0000 | no, but confirm GPIO_ALIVE writes stick from EL2 |
| 3 | Zero boot_stage, set AP_ACTIVE/DUMP_NOTI, run the off-then-on GPIO sequence with its delays | GPIO DAT writes | no |
| 4 | Load the first CP image at G#EA41_0000; write G#BDBD at G#EA40_0000; write the control-message fields | DRAM | no |
| 5 | PCIe ch0 PHY config, PERST# high, 18 ms, RC setup, LTSSM, L0 at Gen1 x1, iATU, MSI target G#F620_0000 | PCIe MMIO | no |
| 5a | PMU G#1546_3EC4 RMW mask G#400 value G#400 | SMC G#8200_0504 (x1 = addr, x2 = 2 (RMW), x3 = mask, x4 = value), `rmw_priv_reg` in soc-gs drivers/soc/google/exynos-pd_el3.c | yes on Linux: EL3 "priv reg" service; ferros at EL2 can issue the same SMC, or test a direct write |
| 6 | SysMMU HSI1 (G#1206_0000) and S2MPU HSI1 (G#1209_0000): either bypass or identity-map the CP regions | MMIO | no, but S2MPU state is set by the bootloader and normally owned by pKVM; check it does not block the CP's DMA |
| 7 | Enumerate the EP, BAR0 = G#14E6_0000, memory + bus master on | config space via CPU G#40FF_E000 | no |
| 8 | Write G#F620_0014/18/1C, doorbell G#1_0000, poll G#F620_0010 for G#3FFF | DRAM + doorbell MMIO | no |
| 9 | Drop the link, wait CP2AP_WAKEUP high, bring the link up at Gen3 x2, rewrite BAR0, doorbell G#1_000E | as step 5/7 | no |
| 10 | Feed the remaining images over the RAW ring on channel G#F1 using the `cbd` protocol | DRAM rings | no, but protocol unknown |
| 11 | Answer INIT_START with PIF_INIT_DONE and PHONE_START with INIT_END; watch PHONE_ACTIVE | DRAM + doorbell + MSI | no |

The only EL3-mediated access in the whole Linux path is the PMU write in step 5a; nothing EL3-only is required for image verification.
The open items are the step-10 protocol, the HSI1 clock/S2MPU state left by the bootloader, the exact doorbell CPU address, and the image file names.
Oracle captures that close them: the MMIO trace of the ch0 PCIe and PMU G#1546_3EC4 accesses during Linux modem boot, the IPC RAW rings (G#EA40_3000 and G#EA60_0000) while `cbd` runs, and a listing of the `modem` partition.

## Source lines used

- radio-s5300/modem_ctrl_s5100.c: 104-120, 642-679, 733-804, 806-900, 906-934, 968-1037, 1059-1097, 1152-1181, 1232-1305, 1335-1481, 1647-1874, 2048-2125.
- radio-s5300/modem_ctrl.h: 10-43; modem_prj.h: 56-132; modem_utils.c: 612-641.
- radio-s5300/link_device.c: 457-520, 526-640, 1416-1426, 1901-1940, 2013-2120, 2253-2350, 2551-2580, 2686-2724, 3602-3650, 3700-3736, 3891-3940, 4195-4260.
- radio-s5300/link_device_memory.h: 58-87, 195-196, 405-410, 436-468, 600-623; link_device_memory_legacy.c: 90-201, 271-330; link_device_pcie_iommu.c: 16-51.
- radio-s5300/s51xx_pcie.c: 60-140, 240-280, 366-461; bootdump_io_device.c: 138-300, 454-600; modem_io_device.c: 554-598; include/exynos_ipc.h: 13-50; Kbuild; Kconfig: 20-161.
- soc-gs-6.1/drivers/pci/controller/dwc/pcie-exynos-rc.c: 393-428, 1471-1507, 1644-1681, 1974-2028, 2083-2085, 2262-2271, 2295-2307, 3056-3071, 3145-3215, 3367-3460, 3536-3790, 3792-3960, 3962-4065, 4606-4623, 5429-5438, 5728.
- soc-gs-6.1/drivers/pci/controller/dwc/pcie-exynos-zuma-rc-cal.c: 154-224, 244-274, 276-715; pcie-exynos-rc.h: 19-313; pcie-exynos-common.h: 20, 52-53, 89-92.
- soc-gs-6.1/drivers/iommu/exynos-pcie-iommu-zuma.c: 204-230; soc-gs (6.1 branch) drivers/soc/google/exynos-pd_el3.c: set/get/rmw_priv_reg; include/linux/soc/samsung/exynos-smc.h: 45; include/dt-bindings/soc/google/exynos-cpif.h: 84-95, 136-145, 319-322.
- soc-gs drivers/pinctrl/gs/pinctrl-gs.c: 59-60, 216-224, 274-277 (identical on the 6.1 branch).
- kdev-zuma/dts: zuma-cp-s5300-sit.dtsi 25-102, 478-540; zuma-board-cp-s5300-sit.dtsi 16-154; zuma-rmem.dtsi 109-131; zuma-pcie.dtsi 12-72; zuma-board-pcie.dtsi 10-29; zuma-s2mpu.dtsi 85-90; zuma.dtsi 464; zuma-board.dtsi 10-12.
- kdev-shusky: shusky_defconfig 1-13; dts/zuma-shusky-pcie.dtsi 9-11; dts/zuma-shusky-cp-features.dtsi 10-12; dts/zuma-husky-common.dtsi 8-24.
- device/google/zuma (gitiles, main): conf/fstab.modem.
