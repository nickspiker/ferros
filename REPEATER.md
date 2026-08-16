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

## The two unknowns (grab from live DT, then this is unblocked)

Both come straight from the device tree — read as root from Android (Magisk):

```bash
# repeater node: its 7-bit I2C address is the 'reg' property; its parent node is the HSI2C bus
adb shell su -c 'find /proc/device-tree -name "*eusb-repeater*" -o -name "*repeater*"'
adb shell su -c 'REP=<node>; od -A n -t x1 $REP/reg'                 # -> 7-bit repeater address
# parent bus node holds the HSI2C controller base:
adb shell su -c 'od -A n -t x1 /proc/device-tree/<parent-i2c-bus>/reg'   # -> HSI2C base + size
adb shell su -c 'cat /proc/device-tree/<parent-i2c-bus>/compatible'      # confirm exynos hsi2c/usi
```

Fill these into the probe payload's two constants and iterate.

## Bring-up plan (iterate over the RUN channel, no kernel reflash)

The beauty: this is testable as a **payload**. `bridge run repeaterprobe.bin` runs against the already-enumerated device and reports back — no reflash, no re-rolling enumeration per iteration. The hard 80% (HSI2C bring-up + talking to the chip) is fully iterable this way.

1. **`repeaterprobe` payload** — bring up HSI2C (reuse ABL timing), read `REV_ID` (`G#B0`), report it. First success = the driver works.
2. Read the full tuning register set; compare against a healthy-boot baseline (the same before/after discipline `usbprobe` established).
3. Add a repeater re-init + tune sequence; prove via payload that a flaky link can be *rescued* by re-tuning the repeater while up.
4. **Fold into the kernel**: run repeater init in `kernel_main` before the eUSB2 PHY init, gated so it doesn't fight ABL's setup when that survived. Target: deterministic enumeration, sub-5s, no watchdog retries.

## What NOT to do

- Don't guess the HSI2C base — a blind write to the wrong controller is exactly the class of mistake the S2MPU/PMU episode ([pixel8_boot_findings]) warned about. Read it from DT.
- Don't recompute I2C timing — read ABL's calibrated `TIMING_*` values and reuse them.
- Probe (read `REV_ID`) before any write. Reads are safe; a write to a mis-identified bus is not.
