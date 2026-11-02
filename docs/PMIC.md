# PMIC — regulators and the battery gauge on husky, for ferros

Read from the husky kernel tree 2026-10-16 (paths under `/mnt/Harbor/husky-kernel/private/google-modules/soc/gs/` unless noted; `bms/` is `google-modules/bms`). What the display's cold power-up (DISPLAY-DESIGN.md §4 step 4) and power profiling (§5) need. Nothing here is implemented in ferros yet; UNCONFIRMED where the source doesn't settle it.

## The PMICs are reached through the ACPM firmware, not a bus

S2MPG14 (main) and S2MPG15 (sub) have no AP-side bus driver: the "i2c-acpm" bus is a stub (`drivers/i2c/busses/i2c-acpm.c:31-35`), and every register access is an IPC message to the APM firmware, which owns the real PMIC bus (SPEEDY/I3C in the APM block at G#1550_0000; driving it directly would collide with the firmware's own DVFS traffic). The bootloader loads and starts ACPM; the kernel only maps it and reads its descriptors (`acpm/acpm_ipc.c:1048-1137`). No registration or claim is sent to the firmware. Nothing in EL3 exposes the PMICs (`exynos-smc.h` has no PMIC/ACPM/I2C SMC), so an EL2 client drives the IPC itself.

**Addresses** (`devices/google/zuma/dts/zuma.dtsi:520-536`): mailbox AP→APM G#1511_0000 (+1000); APM SRAM as the AP sees it G#1570_0000 (+58000), descriptor table at SRAM + G#A000; the first G#5000 of SRAM is likely secure (inferred from `dump-base` G#1570_5000), touch only what the descriptors point to. IRQ SPI 80 (unused when polling).

**Mailbox** (`acpm_ipc.h:105-117`): INTGR1 G#40 doorbell (write `1 << channel`), INTCR0 G#24 clear, INTMR0 G#28 mask, INTSR0 G#2C status.

**Descriptors** (`fw_header/framework.h:36-134`, offsets from the SRAM base): `acpm_framework` at +G#A000: `plugins` @0, `num_plugins` @4, `ipc_channels` @8, `num_ipc_channels` @C, `ipc_ap_max` @18. Each `ipc_channel` is 72 bytes: `id`, flags (`ap_poll` etc.), owner, type, then `channel_info` = `rx_rear, rx_front, rx_base, rx_indr_buf, rx_indr_buf_base, rx_indr_buf_size, tx_rear, tx_front, tx_base, q_len, q_elem_size, credit, mbox_addr`. Directions are the firmware's: **the AP transmits on `rx_*` and receives on `tx_*`** (`acpm_ipc.c:968-981`). `q_elem_size` is bytes; a PMIC message is 4 words.

**Channel 2** carries both PMICs (and the meter/BCL users): serialise all PMIC traffic on it. S2MPG14 is SPEEDY channel 0, S2MPG15 channel 1 (`zuma-pmic.dtsi:28-31, 629-632`). It must be a polling channel (`ap_poll` set in its descriptor: confirm at runtime).

**Message** (`include/soc/google/acpm_mfd.h`, `acpm_mfd.c:15-274`):
- `cmd[0]`: [7:0] register, [11:8] type (0 top/common, 1 PMIC, 2 RTC, G#A meter, G#B WLWP, G#C GPIO), [15:12] SPEEDY channel, [21:16] sequence number 1..63 (never 0).
- `cmd[1]`: [7:0] function (0 read, 1 write, 2 update, 3 bulk read, 4 bulk write); write value [15:8]; update value [15:8] mask [23:16]; bulk count [15:8] (≤ 8, data in `cmd[2..3]`).
- `cmd[3]`: optional time stamp.
- Response, same element: `cmd[1]` [15:8] read data, [31:24] return code (0 ok, 1 rx fail, 2 tx fail).

**Transaction** (`acpm_ipc.c:679-857, 434-466`): read TX front and RX front; wait while `(tx_front + 2) % q_len == rx_front`; write the 4 words at `tx_base + q_elem_size × tx_front`; `tx_front = (tx_front + 1) % q_len`; doorbell `1 << 2` to INTGR1; poll the RX queue from rear to front for the element whose `cmd[0]` sequence matches; copy it; set RX rear = front; write `1 << 2` to INTCR0. The kernel allows ~100 ms with retries. Front/rear may be non-zero after the bootloader: always read them. A good first probe: chip id, type 0, register G#0B.

## The display rails

V = min + sel × step; enable by writing the enable mask into the control register (`s2mpg14-regulator.c:75-82, 219-227, 965`).

| Rail | PMIC, SPEEDY ch, type | Control register | Enable | Vsel | Scale | DT |
|---|---|---|---|---|---|---|
| VCI = L14M "DISP_VCI" | S2MPG14, 0, 1 | `L14M_CTRL` G#38 | bit 7 | [5:0] | 1.8 V + 25 mV/step | 2.5–3.3 V, initial on (`zuma-pmic.dtsi:391-399`) |
| VDDD = L22M "DISP_VDDD" | S2MPG14, 0, 1 | `L22M_CTRL` G#40 | bit 7 | [5:0] | 725 mV + 12.5 mV/step | 725 mV–1.3 V; husky: initial on (`zuma-shusky-pmic.dtsi:46-50`) |
| VDDI = BUCKA | S2MPG15, 1, 1 | `BUCKA_CTRL` G#29 (opmode [7:6], 3 = on) | `BUCKA_OUT` G#2A, 8-bit | 600 mV + 12.5 mV/step | 1.7–2.1 V, always-on (`zuma-pmic.dtsi:860-872`) |

Husky sets no `vddd-normal-microvolt`: the bootloader's programmed values stand. Read the vsel registers back before trusting a voltage.

## The battery gauge: MAX77759 FG on hsi2c_11

Not on ACPM. I²C address G#36 (charger G#69, PMIC G#66) on `hsi2c_11` = G#10CB_0000, an exynos5-hsi2c USI v2 controller at 400 kHz (`zuma-shusky-battery.dtsi:125-170`, `zuma-usi.dtsi:268-288`); USI mode via `sysreg_peric1` G#10C2_1000 + C; pins gpp21[0:1]; clocks `VDOUT_CLK_PERIC1_USI11_USI`, `GATE_PERIC1_TOP0_USI11_USI`. The bootloader probably leaves it configured since it reads the gauge for charging (UNCONFIRMED).

16-bit registers, 8-bit addresses, little-endian on the wire (`bms/max_m5.c:1615-1618`, `max_m5_reg.h:209-234`):

| Quantity | Register | Conversion |
|---|---|---|
| RepSOC | G#06 | raw >> 8 = % |
| Temp | G#08 | raw / 256 °C |
| VCell | G#09 | µV = raw × 78.125 |
| Current | G#0A (signed) | µA = −(s16 × 156250 / RSense), RSense in 10 µΩ units; DT `maxim,rsense-default = 500` = 5 mΩ, so one LSB is 312.5 µA, positive = charging |
| AvgCurrent | G#0B | same |
| AvgVCell | G#19 | as VCell |

## For ferros

1. **Battery current from ferros** (power profiling) — DONE 2026-10-16: `ferros_kernel/src/hsi2c.rs`, a polled master on hsi2c_11 reusing the bootloader's bring-up (confirmed: sysreg mode 4, mux/gates on, DIV /1 so the controller runs at 400 MHz with FS3 divider G#3E for 400 kHz, AUTO_MODE set); `bridge display power [n] [sixteenths] [battery]` reads SOC, temperature, VCell and current, `display i2c <target> <reg> [val]` any 8-bit register. On USB the battery current reads 0 ± 5 mA — the charger carries the load — so `battery` switches the MAX77759 charger's MODE (CHG_CNFG_00 G#B9 [3:0], 5 = charger + buck on) to 0 for the run and restores it in the kernel; the system then draws from the cell (662 mA at 4.28 V idle) and the gauge is a power meter. Results: DISPLAY-POWER.md.
2. **ACPM channel 2 client**: read the descriptors, probe the chip id, then the display rails' control registers; turning VCI/VDDD off and on is the cold power-up's first half (DISPLAY-HK3.md §6 has the order and delays).

## hsi2c_11, the controller in front of the gauge (`soc/gs/drivers/i2c/busses/i2c-exynos5.c`)

Registers from G#10CB_0000: CTL G#00 (FUNC_MODE_I2C b0, MASTER b3, RXCHON b6, TXCHON b7, SW_RST b31), FIFO_CTL G#04 (RXFIFO_EN b0, TXFIFO_EN b1, RX_TRIG << 4, TX_TRIG << 16), TRAILING_CTL G#08, INT_ENABLE G#20, INT_STATUS G#24 (W1C; TRANSFER_DONE b7, NODEV b10, errors [11:8]), FIFO_STATUS G#30 (TX_LVL [6:0], TX_EMPTY b8, RX_EMPTY b24), TX_DATA G#34, RX_DATA G#38, CONF G#40 (AUTO_MODE b31), AUTO_CONF G#44 (TRANS_LEN [15:0], READ b16, STOP_AFTER_TRANS b17, MASTER_RUN b31), TIMEOUT G#48, TRANS_STATUS G#50 (MASTER_BUSY b17), TIMING_FS1..3 G#60/64/68 (FS3 [23:16] = clock divider), ADDR G#70 (target << 10, MASTER_ID [31:24] = 7), USI_CON G#C4 (b0 USI_RESET, 1 at reset: write 0 first).

**A word read at G#36, register r** (the driver's polled auto-mode path, lines 769-1035): write phase FIFO_CTL G#0001_0013, CTL G#88, INT_STATUS cleared, ADDR G#0700_D800, INT_ENABLE G#80, AUTO_CONF 1 then G#8000_0001 (run, no stop), TX_DATA = r, wait INT_STATUS b7 and FIFO_STATUS b8, clear; read phase FIFO_CTL G#0002_0023, CTL G#48, ADDR as before, AUTO_CONF G#0003_0002 then G#8003_0002 (read, stop), take RX_DATA twice while RX_EMPTY is clear, LSB first. Check NODEV / the error bits and MASTER_BUSY = 0 between transfers (the driver doesn't; it should).

**Bring-up if the bootloader didn't** (probe: lines 1331-1361): sysreg_peric1 G#10C2_100C [2:0] = 4 (I2C); CMU_PERIC1 G#10C0_0000: MUX G#640 b4 = 1, DIV G#1810 [3:0] = 1 (200 MHz), gates G#2048 / G#204C / G#2094, QCH G#3040; gpp21 pins 0/1 function 2 (CON G#10C4_0040 [7:0] = G#22), no pull; then USI_CON = 0, INT_STATUS cleared, CTL SW_RST pulse, timing for 400 kHz at 200 MHz (FS3 [23:16] = G#1F, FS2 [7:0] = G#E0, FS1 [23:16] = G#F0), CTL = 8, CONF |= AUTO_MODE. Signs it is already up: sysreg mode 4, MUX/DIV as above, USI_CON b0 = 0, CONF b31, CTL b3, FS3 divider nonzero, MASTER_BUSY clear. The MAX77759's regmap is 8-bit register, 16-bit little-endian value, so the read above is wire-identical to an SMBus read-word.
