# eUSB2 Repeater Bring-Up (Pixel 8 / Tensor G3) — Zila (1)

## Why this matters

Enumeration on the Pixel is a coin flip: some boots the analog path never lights up and USB stays silent for 30–240s while the watchdog re-inits the PHY. The forensics (2026-08-15, [pixel8_boot_findings]) showed failed inits look *identical* to healthy ones at the link controller — LTSTATE `G#FFFF4`, LINKDBG `G#115` both ways. The failure is the **analog path**, and the analog path in front of the eUSB2 PHY is the **repeater**.

ABL initializes the repeater over I2C before it jumps to us. Usually its state survives the jump — but not always, and when it doesn't, nothing we do to the PHY brings the wire up. Owning the repeater init ourselves is the root-cause fix for the flakiness. It is the #1 blocker: until enumeration is deterministic, every hardware test rides a slow, unreliable coin flip.

## The chip

`samsung,eusb-repeater` — a TI-style eUSB2-to-USB2 repeater on an I2C bus, 8-bit register space. Reference driver mined into [tools/pixel8/phy-ref/eusb_repeater.c](tools/pixel8/phy-ref/) (1146 lines, gs-google `android-gs-shusky-5.15`).

### Registers (8-bit addresses)

| Reg | Addr | Purpose |
|---|---|---|
| `GPIO0_CONFIG` | `G#00` | GPIO0 config |
| `GPIO1_CONFIG` | `G#40` | GPIO1 config |
| `UART_PORT1` | `G#50` | UART port |
| `CONFIG_PORT1` | `G#60` | port config; `REG_DISABLE_P1` = bit 6 |
| `U_TX_ADJUST_PORT1` | `G#70` | USB TX adjust (tuning) |
| `U_HS_TX_PRE_EMPHASIS_P1` | `G#71` | HS TX pre-emphasis |
| `U_RX_ADJUST_PORT1` | `G#72` | RX adjust |
| `U_DISCONNECT_SQUELCH_PORT1` | `G#73` | disconnect squelch threshold |
| `E_HS_TX_PRE_EMPHASIS_P1` | `G#77` | eUSB HS TX pre-emphasis |
| `E_TX_ADJUST_PORT1` | `G#78` | eUSB TX adjust |
| `E_RX_ADJUST_PORT1` | `G#79` | eUSB RX adjust |
| `REV_ID` | `G#B0` | **chip revision — the "hello world" probe target** |
| `I2C_GLOBAL_CONFIG` | `G#B2` | global config |
| `INT_ENABLE_1/2` | `G#B3`/`G#B4` | interrupt enables |
| `BC_CONTROL` | `G#B6` | battery-charging control |
| `INT_STATUS_1/2` | `G#A3`/`G#A4` | interrupt status |

The bring-up milestone is one successful **read of `REV_ID` (`G#B0`)**. If that returns a sane nonzero revision, the HSI2C driver works and the whole bus is ours; everything after is tuning writes (the `*_ADJUST`/`*_EMPHASIS` registers, values from the DT `tune_param` table).

### I2C access pattern (from the ref)

Classic 8-bit-register device:
- **Write reg:** `START, [addr<<1|0], reg, data…, STOP` — one message.
- **Read reg:** `START, [addr<<1|0], reg, REPEATED-START, [addr<<1|1], data…, STOP` — two messages, repeated start (no STOP between).

Retry up to `TUSB_I2C_RETRY_CNT` with 1ms between attempts.

## The HSI2C controller

The repeater hangs off an Exynos **HSI2C** (USI-I2C) controller. Same programming model as the mainline `i2c-exynos5` driver. Register offsets from the controller base:

| Reg | Off | Reg | Off |
|---|---|---|---|
| `CTL` | `G#00` | `MANUAL_CMD` | `G#4C` |
| `FIFO_CTL` | `G#04` | `TRANS_STATUS` | `G#50` |
| `INT_ENABLE` | `G#20` | `TIMING_HS1..3` | `G#54/58/5C` |
| `INT_STATUS` | `G#24` | `TIMING_FS1..3` | `G#60/64/68` |
| `FIFO_STATUS` | `G#30` | `TIMING_SLA` | `G#6C` |
| `TXDATA` | `G#34` | `ADDR` | `G#70` |
| `RXDATA` | `G#38` | | |
| `CONF` | `G#40` | | |
| `AUTO_CONF` | `G#44` | | |

Minimal master-transfer flow (mirrors `i2c-exynos5.c`):
1. `CTL`: reset FIFOs, set master mode.
2. `TIMING_*`: from the input clock — ABL already clocked the bus, so **read back ABL's timing values first** and reuse them (same trick that worked for the PHY: don't recompute what ABL calibrated).
3. `ADDR`: slave address in the right field.
4. `CONF` / `AUTO_CONF`: set length, read/write direction, master-run bit.
5. Poll `FIFO_STATUS` / `TRANS_STATUS`; push/pull `TXDATA`/`RXDATA`.
6. Watch `TRANS_STATUS` for NO_ACK / transfer-done.

## Reference tree

The full gs-google kernel source (branch `android-gs-shusky-5.15-android15-qpr1`, the exact branch our device runs) is cloned on this machine at **`/mnt/Harbor/ferros-ref/soc-gs`** — all drivers *and* the SoC device tree, 31M shallow clone. `tools/pixel8/phy-ref/` is the old piecemeal subset; the Harbor tree is the whole thing. Grep it instead of fetching files one at a time. Driver: `drivers/phy/samsung/eusb_repeater.c`; SoC DTS: `arch/arm64/boot/dts/google/zuma-usi.dtsi` (HSI2C controllers), `zuma-usb.dtsi` (USB/PHY). The husky *device* overlay (which binds the repeater to a specific bus + address) is NOT in this tree — it ships as a compiled dtbo on the device; read it live (below).

## The two unknowns (one lives in the SoC DTS, one needs live DT)

**Candidate HSI2C bases (from `zuma-usi.dtsi`, authoritative for Tensor G3):**

```
hsi2c_0  @ G#10C8_0000    hsi2c_9  @ G#10C9_0000    hsi2c_15 @ G#111B_0000
hsi2c_2  @ G#1089_0000    hsi2c_10 @ G#10CA_0000    hsi2c_16 @ G#111C_0000
hsi2c_3  @ G#108A_0000    hsi2c_11 @ G#10CB_0000
hsi2c_4  @ G#108B_0000    hsi2c_12 @ G#10CC_0000
hsi2c_5  @ G#108C_0000    hsi2c_13 @ G#10CE_0000
hsi2c_6  @ G#108D_0000    hsi2c_14 @ G#1098_0000
```

The repeater sits on exactly one of these. Which one + the repeater's 7-bit address are set by the husky overlay, so read them from the live DT (Android + Magisk root):

```bash
# repeater node: 'reg' is its 7-bit I2C address; its parent node is the HSI2C bus
adb shell su -c 'find /proc/device-tree -iname "*repeater*" -o -iname "*eusb*"'
adb shell su -c 'REP=<node>; od -A n -t x1 $REP/reg'                     # -> 7-bit repeater address
adb shell su -c 'od -A n -t x1 /proc/device-tree/<parent-i2c-bus>/reg'   # -> HSI2C base (match to table above)
adb shell su -c 'cat /proc/device-tree/<parent-i2c-bus>/compatible'      # confirm samsung hsi2c/usi
```

Alternatively, decompile the on-device dtbo (`dtc`/`fdtget` on the extracted `dtbo.img`) — no reboot needed, but the root command above is faster. Fill both into the probe payload's constants and iterate.

## HSI2C engine — WRITTEN (payloads/repeaterprobe)

The HSI2C master engine is ported and compiles clean — a faithful translation of the `i2c-exynos5.c` **polling / auto-mode** path (the controller drives START/ADDR/STOP itself once `ADDR` + `AUTO_CONF.len` + `MASTER_RUN` are set; no manual bit-banging). Register map and bit constants copied verbatim from the ref. Key decisions, both from the "do less than the C driver" principle:

- **No SW_RST in `init()`** — it would force recomputing the SCL timing. We ensure only `MASTER` + `AUTO_MODE` and *reuse ABL's calibrated `TIMING_*`* (same trick that fixed the PHY). The probe dumps `CTL`/`CONF`/`TIMING_*` first so we can see ABL's state before trusting it.
- **Polling, our own spin bound** — hardware `TIMEOUT_EN` disabled; we bound each phase with a spin counter (no jiffies/IRQs), matching the rest of ferros.
- `read_reg` = write reg pointer (repeated-start, `stop=false`) then read the byte (`stop=true`) — exactly the ref's 2-message read.

Written self-contained so it lifts straight into `ferros_hal::hsi2c` for the kernel once proven. **Two placeholders remain** (`HSI2C_BASE`, `REPEATER_ADDR`) — fill from DT (above) and it's ready to run. (Note: with the placeholders in place the compiler DCEs the whole engine down to the guard message — that's expected; real addresses restore the full ~1.1KB blob.)

## Bring-up plan (iterate over the RUN channel, no kernel reflash)

The beauty: this is testable as a **payload**. `bridge run repeaterprobe.bin` runs against the already-enumerated device and reports back — no reflash, no re-rolling enumeration per iteration. The hard 80% (the HSI2C engine) is done.

1. **Fill the two DT constants**, `bridge run repeaterprobe.bin` — expect the config dump then `REV_ID=…` + `REPEATER BUS UP`. First success = the engine works and the bus is ours. A `no ACK / timeout` with `TRANS_STATUS`/`ERR_STATUS` means wrong base or address — try the next `hsi2c@` candidate.
2. Read the full tuning register set; compare against a healthy-boot baseline (the before/after discipline `usbprobe` established).
3. Add a repeater re-init + tune sequence; prove via payload that a flaky link can be *rescued* by re-tuning the repeater while up.
4. **Fold into the kernel**: extract the engine to `ferros_hal::hsi2c`, run repeater init in `kernel_main` before the eUSB2 PHY init, gated so it doesn't fight ABL's setup when that survived. Target: deterministic enumeration, sub-5s, no watchdog retries.

## What NOT to do

- Don't guess the HSI2C base — a blind write to the wrong controller is exactly the class of mistake the S2MPU/PMU episode ([pixel8_boot_findings]) warned about. Read it from DT.
- Don't recompute I2C timing — read ABL's calibrated `TIMING_*` values and reuse them.
- Probe (read `REV_ID`) before any write. Reads are safe; a write to a mis-identified bus is not.
