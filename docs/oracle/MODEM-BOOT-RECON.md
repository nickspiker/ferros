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

## Boot channel G#F1: captured (2026-10-19)

Captured on Android under the hv with only the IPC control page G#EA40_0000 trapped (`FERROS_HV_MODEM_RING=1 FERROS_HV_ORACLE_STEP=1`, see below), decoded with `tools/oracle-decode-modem-ring.py`; the modem still came up ONLINE under the trap.
The raw trace stays local (`/mnt/Harbor/tmp/modem-capture`): the NV images it carries are per-unit.

Every frame is a 12-byte SIT header {G#ABCD, u16 frame seq, u16 cfg (G#C000 AP->CP, 0 CP->AP), u16 length incl. header, u8 channel G#F1, u8 channel seq, 2 spare}, then a payload whose first u16 is the command.
AP commands are G#Axxx and the CP answers each with the same number in G#Cxxx; a data frame is {u16 G#AnnB, u16 chunk length + 8, u32 section size, u32 offset, data}, at most G#C000 data bytes, and each one is acked with G#CnnB before the next.
The data is the raw section bytes, unmodified (first chunks match `modem.bin` at the TOC offsets).

The whole conversation, in order (the ROM stage before it is the BOOT section at G#EA41_0000 + doorbell, section 3):

| AP | CP | Meaning |
|---|---|---|
| G#A110 | G#C110 | hello |
| | G#C11B | CP bootloader ready |
| G#A11D | G#C11D | |
| G#A120 / data G#A12B x1998 / G#A321 {u16 0, u32 CRC} / G#A12D | G#C120 / G#C12B each / G#C320 / G#C12D | MAIN, G#5DA_0C00 bytes; G#A321 carries MAIN's CRC from the TOC (G#BEB4_3318) |
| G#A130 / G#A13B x96 / G#A13D | acks | VSS, G#47_CC0C bytes |
| G#A140 / G#A14B x1 / G#A14D | acks | APM, G#B498 bytes |
| G#A150 / G#A15B x11 / G#A15D | acks | NV_NORM, G#8_0000 bytes = `/mnt/vendor/efs/nv_normal.bin` |
| G#A160 / G#A16B x11 / G#A16D | acks | NV_PROT, G#8_0000 bytes = `/mnt/vendor/efs/nv_protected.bin` |
| G#A170 / G#A17B x11 / G#A17D | acks | REPLAY, G#8_0000 bytes = `/mnt/vendor/modem_userdata/replay_region.bin` |
| G#A400 | G#C400 | start: the rings reset and the main firmware runs |

Section n's begin/data/end are G#A1n0 / G#A1nB / G#A1nD with n = the TOC entry id.
`modem.bin` (in the `modem` partition, `images/default/`) starts with a TOC of 32-byte entries {name[12], u32 file offset, u32 CP load address, u32 size, u32 CRC, u32 id}:

| Name | File offset | Load | Size | CRC | Id |
|---|---|---|---|---|---|
| BOOT | G#410 | 0 | G#1_6800 | G#3842_82CC | 1 |
| MAIN | G#1_6C10 | G#4001_0000 | G#5DA_0C00 | G#BEB4_3318 | 2 |
| VSS | G#5DB_7810 | G#4F90_0000 | G#47_CC0C | G#CDB6_2B7F | 3 |
| APM | G#623_441C | G#202_E000 | G#B498 | G#9413_0678 | 4 |
| NV_NORM | not in file | G#4D60_0000 | G#8_0000 | | 5 |
| NV_PROT | not in file | G#4D68_0000 | G#8_0000 | | 6 |
| REPLAY | not in file | G#5A40_0000 | G#8_0000 | | 6 (sent as section 7) |
| INFO | G#623_F8B4 | G#4000_9000 | G#D0 | G#FDA3_B5AD | 0 (not sent) |

After G#C400 the CP opens channel G#29 (41), a remote-file service: it asks for paths such as `/vendor/firmware/carrierconfig/manifests/...` and the AP streams the files back (Android's `rfsd`).
A cold ferros modem boot therefore needs, besides section 1-2: `modem.bin` (or its sections), the three per-unit NV files, and later a channel-41 file server.

### Capture method

`FERROS_HV_MODEM_RING=1` makes the hv log each frame the AP puts in the RAW TX ring (bracketed by its write of the TX head, IPC+G#18) and the CP frames it consumes from the RAW RX ring (bracketed by its write of the RX tail, IPC+G#24).
`FERROS_HV_ORACLE_STEP=1` keeps a trapped page trapped through accesses without a syndrome (the driver's memcpy into IPC+G#82C returned the page to the guest on the first try): the hv remaps the page, single-steps the guest over that one instruction with debug exceptions routed to EL2 and interrupts masked, and unmaps the page again at the step exception.

```bash
FERROS_HV_NO_PMU_READ=1 FERROS_HV_STAGE2=1 FERROS_HV_ORACLE=1 FERROS_HV_MODEM_RING=1 FERROS_HV_ORACLE_STEP=1 FERROS_HV_ORACLE_CAP=6000 FERROS_HV_ORACLE_PAGES="G#EA40_0000" FERROS_HV_ORACLE_QUIET="G#EA40_0018,G#EA40_001C,G#EA40_0020,G#EA40_0024" HV_CMDLINE="loglevel=4" tools/husky-hv.sh --flash
```

## ferros cold bring-up: `diag modem sN` (written 2026-10-19 night, not yet run)

`ferros_kernel/src/modem.rs` runs stages 0..=N, each repeating the ones before it so a hang points at the stage that caused it (the payloads/pcie1 pattern, ported to ch0):

| Stage | What | Pass looks like |
|---|---|---|
| s0 | read only: pd_hsi1 status (EL3 read), CMU_HSI1 gates on the ch0 path, PMU G#3EC4, GPIO, S2MPU/SysMMU HSI1, ELBI LTSSM, DBI id, boot_stage | HSI1 powered, every gate open |
| s1 | power_on_cp by GPIO with Linux's delays, then a second of CP2AP pin changes | PS_HOLD rises (the CP is running its ROM) |
| s2 | PERST low, CLKREQ, PMU isolation release, SOC+G#4000, ch0 power-down clear, ch0 PHY table, PLL/CDR/offset-cal polls | the three locks |
| s3 | PMU bit 10, UDBG G#C800, RC setup (Gen1 x1), IOCC, PERST high + 18 ms, LTSSM | L0 (G#11) |
| s4 | iATU (CFG0, MEM G#4000_0000 -> bus G#14E0_0000), CP config space, BAR0, doorbell address from its readback, bridge window, memory + bus master, MSI to G#F620_0000 | vendor G#144D (Linux reads device G#A5A5), doorbell G#4006_0000 |
| s5 | HSI1 S2MPU protection off, BOOT section from `modem_a` into G#EA41_0000, control words, magic G#BDBD, image address/size in the MSI page, doorbell G#1_0000, poll boot_stage | G#3FFF |

After s5, the next steps are the link cycle (drop the link, wait for CP2AP_WAKEUP, Gen3 x2, doorbell G#1_000E) and the channel-G#F1 download in the captured order.
`diag modem files` and `diag modem readmain` check the flash path first (expected MAIN BLAKE3 prefix 14df88e07f8aad008ab8fff8a4256d12).

## Stages 6-7: the link cycle works, the boot channel answers once (2026-10-20)

**Stage 6, the link cycle, works.** After the ROM stage the CP jumps to the BOOT image it pulled and re-initialises its endpoint, so the link must be dropped and retrained. Two steps of `exynos_pcie_rc_poweroff` (pcie-exynos-rc.c 4005-4050) are easy to miss and the retrain fails without either: the PHY input clock goes back to the oscillator (`SOC + G#4000 &= ~3`) and a SOFT_PWR_RESET pulse runs (ELBI G#3A4 bit 1 cleared, 20 us, set) before `phy_all_pwrdn`. With them:

```
cycle: link dropped, waiting for the CP
cycle: CP2AP_WAKEUP high after A#156 ms
link up on attempt A#1 (Gen3 x2); status G#3021_0000   (DL Link Active, 2 lanes)
link ack rung (G#1_000E)
```

Asking for Gen1 x1 on the second link never trains (eight attempts bounce POLL_ACTIVE <-> POLL_CONFIG); Gen3 x2 trains first try and negotiates down to 2.5 GT/s x2. `s5100_poweroff_pcie` refuses to drop the link when CP2AP_WAKEUP already reads 1 (`modem_ctrl_s5100.c:1661-1675`), and after the ROM stage it reads 0 for the first ~150 ms and 1 afterwards — so whether Linux cycles the link depends on when it looks.

**Stage 7, the boot channel, answers the first frame and then nothing.** The rings have to be initialised exactly as `init_legacy_link` does (link_device_memory_legacy.c:228) or the first frame has nowhere to go: magic (IPC+0) and mem_access (IPC+4) to zero, all four cursors of both ring pairs (IPC+G#8..G#24) to zero, then magic G#BDBD and mem_access 1, both read back. On a cold boot those cursors hold G#FFFF_FFFF.

With that, `A110` is answered with `C110` — the boot channel is alive both ways. Every frame after it is **consumed** (the CP advances the tx tail to the head within ~500 us) and **never answered**, whatever it is: `A11D`, `A120`, a data frame. The rx head stays at G#10.

Ruled out:
- The per-frame signal. In the whole Android capture the AP writes `ap2cp_msg` exactly once (zero, at `init_legacy_link`) and never again, and rings no doorbell per boot-channel frame: the CP polls the ring. Writing G#81 + doorbell per frame and not writing it behave identically — the first frame is answered either way.
- The FMT ring: empty throughout (head 0, tail 0), so no reply is hiding there.
- The root complex's MSI status left set (DBI G#830, write-1-to-clear): clearing it after each reply changes nothing.
- The CP's unprompted `C11B` being a required gate: it never comes here, and skipping it does not unblock the next frame.

G#A110 / G#A11B / G#A11D are section 1 (BOOT, TOC id 1) begin / data / end on this channel, not a handshake — the per-section pattern is G#A1n0 / G#A1nB / G#A1nD with n the TOC id, so the opening pair is BOOT's, which the ROM already has.

**That capture was taken (cursors not quiet) and explains the gap: the CP moves the AP's tx cursors itself.** Between `C110` and `C11B`, with no write from the AP, both the tx head and the tx tail go from G#10 to G#438 — the CP repositions the ring from its side, and only then does the AP write `A11D` at G#438. The AP therefore has to re-read the head before every frame (ferros does) and, more importantly, **wait for `C11B` before sending the next frame**, because that is when the cursors move.

The capture also corrects one control-message field: Linux writes `ap2cp_united_status` (IPC+G#808) as **zero**, not with a DS-detect value in bits [15:14]. ferros matches it now; it made no difference to the stall.

So the single open question is narrow: **what makes the CP emit `C11B` and reposition the cursors.** It happens ~30 ms after `C110` under Linux and never here, and pushing on without it writes the next frame at an offset the CP has not moved to, which is why everything after is consumed in silence. Everything else about the channel is understood and matches.

## 2026-10-24 night: the boot channel runs through — MAIN, VSS and APM loaded

The stall was ours. Decoded from cbd (its own symbol table rides in `.gnu_debugdata`: `build_std_dload_control`, `std_boot_load_cp_images`, `std_udl_req_resp`, `std_dl_tx_frame_sit`) and re-read off the captures: **stage 1 is the TOC itself** — G#A110 → G#C110, one data frame **G#A11B carrying modem.bin[0..G#410]** (the table of contents and its signature blob) → its ack G#C11B, then G#A11D → G#C11D. The "unprompted C11B" was that ack: the hv capture had missed the one A11B frame (one gap in 2498 TX frames, exactly G#428 bytes: header + {A11B, len, total, offset} + G#410), ferros never sent it, and the CP waited for it and dropped everything after. Corrections to the notes above: A110/A11B/A11D are the TOC stage, not BOOT; C11B is an ack; the CP never moves the AP's cursors; n in A1nX is the stage number (TOC index), not the TOC id (they agree up to APM; REPLAY is stage 7).

Also fixed: the frame counter is a u16 in header bytes 2–3 (the channel's own sequence its low byte); every frame occupies a multiple of 8 bytes in the ring (zero filled, both directions); the CRC request G#A3n1 is answered G#C3n0 (G#C320 for MAIN). And the endpoint's config space reads all ones for up to ~1 ms after the link trains: `endpoint` now waits for it.

Measured: TOC acked; MAIN G#5DA0C00 bytes in 1998 frames, 12.6 s, its CRC accepted (G#C320); VSS G#47CC0C in 96 frames; APM G#B498 in 1. `diag modem s7` stops there on purpose: the three NV stages (calibration, the IMEIs) are per-unit files on f2fs, and G#A400 starts the CP's main firmware, which without its own NV would come up on the air with no identity. cbd stands in all-G#FF NV when a file is missing; ferros does not, until the real NV (or an owner decision) is in hand.

**The NV files are readable (2026-10-25):** `ferros_f2fs` (read-only f2fs: the newer valid checkpoint pack, the NAT through the checkpoint's journal, compacted or normal summaries, inline data and dentries, direct/indirect node trees) reads `efs/nv_normal.bin`, `efs/nv_protected.bin` and `modem_userdata/replay_region.bin` (each G#8_0000 bytes) from the phone's flash in 20 ms (`diag modem files` prints sizes and a short BLAKE3 of each, never contents). Checked twice: on host images of both partitions against `dump.f2fs` and against Android's own `cat` of the files, and on the device, where ferros's reads hash the same as Android's (taken with no boot of Android between). The modem rewrites the efs files while Android runs (the hashes move between Android sessions); `replay_region.bin` moves inode as it is replaced. The `.md5` files beside the NV files hold 32 hex digits that are not the plain MD5 of the file (salted or over something else; not needed). **The whole download runs (2026-10-25 night, `diag modem s7`):** TOC, MAIN (1998 frames, 12.6 s, CRC accepted G#C320), VSS (96), APM (1), then NV_NORM, NV_PROT and REPLAY as sections 5, 6, 7 (11 frames each, 56 ms each) straight from the f2fs files, every frame acked; the CP's message word reads G#83 and boot_stage G#3FFF after. ferros stops there: **G#A400 START waits for the owner's go-ahead**, since it puts this phone on the air under its own identity, and after it the CP opens channel G#29 (the remote-file service) that ferros does not serve yet. `diag modem off` powers the CP down again.

## 2026-10-28 night: START, and the main firmware's first word

`diag modem s8` = stage 7, then START (G#A400; the owner cleared START and data bring-up on 2026-10-26, nothing voice, SMS, MMS or emergency), then ten seconds of watching, answering nothing.
- **START is acked:** G#C400.
- **INIT_START:** 373 ms later `cp2ap_msg` goes from G#83 to G#C1, which is INIT_START (`cmd | G#C0`, link_device_memory.h). `cp2ap_status` stays 0, `boot_stage` G#3FFF, PHONE_ACTIVE and CP2AP_WAKEUP both 1.
- **Then nothing more in ten seconds:** no frame on the RAW or FMT rings, no further message. That is as the driver has it: the main firmware waits for the AP's answer to INIT_START, PIF_INIT_DONE (G#CD), which the AP sends only after initialising pktproc, the DL/UL packet rings in G#E800_0000 (link_device.c 457-520).
- **Afterwards:** `diag modem off` powers the CP down.

**Next:** pktproc's ring layout and init from the GPL cpif source, then PIF_INIT_DONE. Then PHONE_START → INIT_END (link_device.c 526-640), then the channel-41 remote-file service.

### What the AP owes the CP after INIT_START (read from the GPL source, 2026-10-28)

**Sources:** the zuma driver is `radio-s5300/` in the reference tree (built as `s5300:cpif`, kdev-shusky/BUILD.bazel:104), not the older soc-gs cpif. The DT is `kdev-zuma/dts/zuma-cp-s5300-sit.dtsi`. Line references below are into those two. Spot-checked: the INIT_START handler, the capability bits, the DT's pktproc values.

**INIT_START (G#C1) → PIF_INIT_DONE (G#CD), `cmd_init_start_handler` (link_device.c 457-522):**
1. `pktproc_init`: the four DL queues. Per queue: descriptors cleared, `fore_ptr = rear_ptr = 0`; `cp_desc_pbase`, `cp_buff_pbase` and `num_desc` written into the queue's info entry (32-bit addresses, `pktproc_use_36bit_addr` 0); empty buffers pre-posted, `fore_ptr` advanced. No doorbell.
2. Capabilities: `ap_cap[0]` = G#3 (bit 0 PKTPROC_UL, bit 1 CH_EXTENSION) at IPC+G#A0, `ap_cap[1]` = 0 at IPC+G#A8. The CP's own are at IPC+G#A4 and +G#AC. Already done once at probe: IPC+G#70 = G#A0 (the capability pointer), IPC+G#64 = G#40_0000 (srinfo).
3. PIF_INIT_DONE: `ap2cp_msg` = G#CD, then the message doorbell (G#1_0000).

**The DL info header** at G#E800_0000 (`struct pktproc_info_v2`, little-endian, link_rx_pktproc.h 51-72):

| field | value / layout |
|---|---|
| word 0 | `num_queues`[3:0] 4, `desc_mode`[5:4] 1 (sktbuf), `irq_mode`[7:6], `max_packet_size`[23:8] 1584 |
| `q_info[i]`, i = 0..3, 20 bytes each from +4 | {`cp_desc_pbase`, `num_desc`, `cp_buff_pbase`, `fore_ptr`, `rear_ptr`} |

- **Layout:** info at +0 (G#1000), descriptors from +G#1000, buffers from +G#10_0000 (G#1B0_0000, G#6C_0000 a queue).
- **Addresses:** the CP sees the region at G#2000_0000 (`pktproc_cp_base`): AP physical = CP address − G#2000_0000 + G#E800_0000.
- **Sktbuf descriptor** (16 bytes): u64 {`cp_data_paddr`[35:0], `control` byte 5 (b7 HEAD on the first, b3 RINGEND on the last), `status` byte 6 (b0 DONE)}, u16 length @8, u16 filter @10, u16 info @12, u8 `channel_id` @14, u8 itg @15.
- **Who moves what:** the AP owns `fore_ptr` (it posts empty buffers), the CP owns `rear_ptr` (it advances past filled ones). A per-queue MSI tells the AP data is there.
- **Size estimate:** about 3456 descriptors a queue at a 2048-byte stride. This assumes Linux's skb padding: 1584 plus the skb overhead, rounded to a power of two. Not confirmed (kernel headers are not in the tree); on bare metal the stride is ours to choose.

**PHONE_START (G#C8) → INIT_END (G#C2) (link_device.c 526-647):**
1. **The capability check:** `(ap ^ cp) & ap` must be 0, so the CP must report at least G#3.
2. **`pktproc_init_ul`:** UL info at CP G#21C0_0000, two queues (q0 1024-byte, 1760 descriptors; q1 2048-byte, 880). The AP writes the queue entries and *reads* `end_bit_owner` and `cp_quota`, which the CP has filled.
3. **`init_legacy_link`:** IPC magic G#AA, mem_access 1, the FMT and RAW cursors zeroed.
4. **INIT_END:** G#C2.

**Channels:**
- **Data, pktproc only:** PS channels G#B5-G#D3 (181-211, `rmnet`). Raw IP, the channel in the descriptor.
- **Legacy RAW ring:** channel 41 (`umts_rfs0`, the remote-file service), as are DUN 21, DM 81 and loopback 82.
- **Legacy FMT ring:** FMT 245/246.

**Open:** how the CP maps its G#2000_0000 window onto G#E800_0000 (`use_dedicated_baaw` is 0 on husky, so probably fixed in the CP firmware); which MSI data value the CP uses per DL queue (the AP never tells it); whether the DL info header must be rewritten at INIT_START (Linux writes it once, at probe).

**Next on the bench:**
1. Write the DL info and descriptors.
2. Answer PIF_INIT_DONE, and watch for PHONE_START and the CP's capability words.
3. Answer INIT_END; the channel-41 requests then arrive on the RAW ring.

## 2026-10-29: ONLINE, and the handover block that makes the CP speak

**The handshake (`diag modem s9`, `sa`):** INIT_START (G#C1) 374 ms after START → PIF_INIT_DONE (G#CD), PHONE_START (G#C8) 6 ms later → INIT_END (G#C2); PHONE_ACTIVE stays high, no CRASH_EXIT. Corrections to the notes above:
- husky's DT claims no AP capabilities (`ap_capability_0/1 = 0`), so neither do we; the CP reports part 0 = G#7, part 1 = 0 (IPC+G#A4, +G#AC — the AP's words are +G#A0, +G#A8).
- With no `LINK_ATTR_SBD_IPC` the INIT_START handler is just `pktproc_init` and the capabilities. Stage `a` lays out the DL rings as Linux would (four queues of 3456 2 KiB buffers in place at G#E800_0000, the SysMMU in bypass so IOVA = physical) and runs `init_legacy_link` at PHONE_START; the CP does not need pktproc to come ONLINE.
- `ap2cp_united_status` is G#0 then G#4000 (dual-SIM detect = 1) in both captures; an earlier reading missed the second write.

**The CP stays silent after INIT_END until it has the handover block.** `struct t_handover_block_info` (G#A1 bytes at IPC+G#82C, `ap2cp_handover_block_info`) is the first thing cbd writes before every boot (the memcpy at the top of both captures), via `IOCTL_HANDOVER_BLOCK_INFO`. Decoded from cbd (`update_handover_block_info`, `scan_cdt_property_data`; the vendor image's `/bin/cbd` carries its symbols in `.gnu_debugdata`):

| offset | field | source on a normal boot of a user build |
|---|---|---|
| G#00 | version | 1 |
| G#04 | project_id | cdt[0] (platform) |
| G#08 | revision | `hwinfo` from `hardware_config.json` (absent on husky's radio build → 0) |
| G#0C, G#10 | major_id, minor_id | cdt[3], cdt[4] |
| G#14, G#18 | modem_sku, modem_hw | cdt[6], cdt[7] |
| G#1C | cpinfo0 | 0 (set for factory boot modes only) |
| G#20 | cpinfo1 | 0 (set after an EFS clear only) |
| G#24 | cpinfo2 | `/chosen/config/modem_flag` (absent on husky) |
| G#28 | rf_sub | cdt[8] |
| G#2C | rf_config | `rfid` from `hardware_config.json`, else `/chosen/plat/rfid` |
| G#30-G#38 | reserved | cdt[1], cdt[2], cdt[5] |
| G#3C | reserved | 0 (a userdebug build puts a marker here) |
| G#40, G#50 | cpid[2] | `/chosen/config/imei1`, `imei2`, 16 bytes each (per-unit: never printed or stored) |
| G#60 | cpsig | 64 bytes of `/mnt/vendor/persist/modem/cpsha` (persist is f2fs), then a NUL |

cdt[] is bootconfig `androidboot.cdt_hwid`, parsed by cbd as `0x%04x%02x%02x%04x%02x%02x%02x%02x%04x%08x`. ferros assembles the block in place at a real boot from ABL's tree (a reload has none; the IPC page keeps it) and adds cpsig at the ROM stage. A zeroed block changed nothing; the real one did.

**What the CP says, unanswered (90 s watch):** 1.2 s after INIT_END, two frames on the FMT ring, then nothing:
- ch 130 (`oem_ipc1`): a protobuf setting `persist.vendor.modem.experiment.volte_mif_off` to "1" — the CP pushing a property to the AP's property manager.
- ch 132 (`oem_ipc3`): a protobuf of six {index, ~40 000} entries and two 32-bit words, probably a memory-clock (MIF) request for the clock manager.

Both are served on Android by `/vendor/bin/shared_modem_platform` (Rust; opens `/dev/oem_ipc1` and `/dev/oem_ipc3`). No remote-file request, no RIL frame, no DL packet follows. In the Android capture the CP's first channel-41 frame carries link frame number G#34, so about 51 frames had already passed on FMT by then: the CP expects an FMT conversation (the platform services and the SIT RIL on channels 245/246) before it asks for files.

**Next:** the oem_ipc protocol from `shared_modem_platform`, then the RIL's opening messages from `libsitril.so` (vendor image, `/lib64`). Data only: nothing voice, SMS or emergency.

## 2026-10-29 evening: the data stack registers (LTE and NR) from a cold ferros

`diag modem s10` (and `s11`) now ends with the Verizon stack (stack 1, RIL channel 246) home on the network: about 1.2 s after SIT_SET_ALLOW_NETWORK the CP reports a physical-channel config and NITZ, its PS registration reads `01 00 00 14` (home, NR) at 15 s and `01 00 00 0E` (home, LTE) at 30 s. Nothing is replayed from Android; the sequence is ferros's own (radio on, the carrier-config manifest handshake, UICC subscription, a radio cycle, the six Verizon data profiles, the allowed networks).

**What held it back was the UE-capability file service, not any RIL message.** The CP reads its UE-capability files (`WILDCARD.binarypb`, then `VZW.binarypb`, from the modem image's `/images/default/uecap`) over the modem service on channel 130 (`UecapFileMessage`). Its read request names a block size (field 2: 4000 on this firmware), and it books one token per block it expects: its next request's token comes exactly ⌈size / 4000⌉ later (6 then 131 for the 496 290-byte WILDCARD file). Android's `shared_modem_platform` answers block k at offset 4000·k with the request's token + k. ferros had answered in 1 KiB blocks, all under the request's token. The CP took that without complaint and then never searched: both stacks stayed `00 00 00 0E` (not registered, not searching) through every RIL experiment.

How it was found: a kprobe trace of Android's boot in both directions (cpif `txq_write` and `pass_skb_to_demux`, the frames' first 512 / 256 bytes), then a replay of all 264 of Android's AP→CP requests on channels 129, 132, 245 and 246 at Android's own times after ONLINE, with ferros's own setup actions off. That replay still left the stack not searching, so the RIL side was cleared. The difference left was in what ferros answers, and the trace's channel-130 responses showed the block and token pattern.

A 4000-byte block does not fit one SIT frame, so the link layer now sends multi-frame packets as cpif does (`exynos_build_fr_config`, `modify_next_frame`). Each frame holds at most 2 KiB with its header. The first carries cfg `(G#80 | packet index) << 8 | frames after it`, with a 6-bit index per channel counting from 1. Each next frame carries the count less one, and the last swaps G#80 for G#40 with a count of 0. Android's trace shows `81 01`, then `41 00`, for a two-frame packet (cfg bytes high, low).

Checked and matching Android on the way, so not the cause:
- **Handover block:** identical. A kprobe at `update_handover_block_info` gave version 1, project 4, major 1, G#30/G#34 = 2/6, everything else zero in the board words; ferros builds the same from `androidboot.cdt_hwid`.
- **IMSI-encryption key:** SIT_SET_CARRIER_INFO_IMSI_ENCRYPTION fails with error 22 on Android as well.
- **Data profiles:** the two id −1 data profiles get error 2 on Android too.
- **PREFERRED_DATA_MODEM:** Android sends it on channel 245 (stack 0) with value 1, and never sends SET_PS_SERVICE.

**Next:** the data call (SIT_SETUP_DATA_CALL on the internet profile) and packets over pktproc DL/UL.
