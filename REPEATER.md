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

## The two unknowns — SOLVED (live DT, 2026-08-16)

Mined from `/proc/device-tree` on the running device (Android + Magisk root):

- **Bus: `hsi2c@10CB0000`** = `hsi2c_11` from the zuma-usi.dtsi table. Compatible `samsung,exynos5-hsi2c`, status okay, `reg` = `G#10CB_0000` size `G#1000`.
- **Repeater: `eusb-repeater@3E`** — 7-bit address `G#3E`.
- **The bus is shared** with the entire battery/USB-C management chain: max77729 PMIC, max77759 charger/fuel-gauge/TCPC, pca9468 charge pump, all on `hsi2c_11`. Reads are safe; a stray write on this bus can touch power management. Never write blind.
- DT search quirk: the repeater node does NOT match `find -iname '*repeater*'` on the device's toybox find; list `hsi2c@*/` children and read their `compatible` instead.

<details><summary>Original mining procedure (for reference)</summary>

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

</details>

## MILESTONE: REPEATER BUS UP (2026-08-16)

`bridge run repeaterprobe.bin` succeeded **first try** against the live device: `REV_ID = G#3`. The HSI2C engine works, the bus is ours, and the no-SW_RST design bet paid off — ABL left the controller in master + auto mode with calibrated FS timing, and we rode it as-is.

**Healthy-boot baseline** (this boot enumerated normally, ~40s):

```
HSI2C controller state as ABL left it:
CTL=G#48  CONF=G#980110FF  TIMING_FS1=G#01F0FF00  TIMING_FS3=G#3E0000  TIMING_SLA=G#0  TRANS_STATUS=G#80001

Repeater registers (all 8-bit):
REV_ID=G#03                          GPIO0_CONFIG=G#10   GPIO1_CONFIG=G#00
UART_PORT1=G#02                      CONFIG_PORT1=G#10
U_TX_ADJUST_PORT1=G#7C               U_HS_TX_PRE_EMPHASIS_P1=G#3C
U_RX_ADJUST_PORT1=G#92               U_DISCONNECT_SQUELCH_PORT1=G#83
E_HS_TX_PRE_EMPHASIS_P1=G#C8         E_TX_ADJUST_PORT1=G#16
E_RX_ADJUST_PORT1=G#60               INT_STATUS_1=G#00   INT_STATUS_2=G#00
I2C_GLOBAL_CONFIG=G#00               INT_ENABLE_1=G#00   INT_ENABLE_2=G#00
BC_CONTROL=G#C0
```

The nonzero `*_ADJUST`/`*_EMPHASIS` values are ABL's applied tune set (the DT `repeater_tune*` table). On a flaky boot, run the same probe and diff against this block — a mismatch (or a dead bus) fingerprints the failure.

Payload gotcha for the record: register *names* were first emitted via a `const` pointer table and came out as NULs — the blob runs relocated from link base 0, so data-section absolute pointers are garbage. Pass byte-string literals at call sites (compiler emits PC-relative `adr`); this applies to every future payload.

## HSI2C engine — PROVEN ON HARDWARE (payloads/repeaterprobe)

The HSI2C master engine is ported and compiles clean — a faithful translation of the `i2c-exynos5.c` **polling / auto-mode** path (the controller drives START/ADDR/STOP itself once `ADDR` + `AUTO_CONF.len` + `MASTER_RUN` are set; no manual bit-banging). Register map and bit constants copied verbatim from the ref. Key decisions, both from the "do less than the C driver" principle:

- **No SW_RST in `init()`** — it would force recomputing the SCL timing. We ensure only `MASTER` + `AUTO_MODE` and *reuse ABL's calibrated `TIMING_*`* (same trick that fixed the PHY). The probe dumps `CTL`/`CONF`/`TIMING_*` first so we can see ABL's state before trusting it.
- **Polling, our own spin bound** — hardware `TIMEOUT_EN` disabled; we bound each phase with a spin counter (no jiffies/IRQs), matching the rest of ferros.
- `read_reg` = write reg pointer (repeated-start, `stop=false`) then read the byte (`stop=true`) — exactly the ref's 2-message read.

Written self-contained so it lifts straight into `ferros_hal::hsi2c` for the kernel once proven — and it is now proven (REV_ID + full 17-register baseline read over live I2C).

## THE TUNE FINDING (2026-08-16): ferros boots were running an untuned repeater

The husky DT `hs_tune_eusb` node (read from the live DT, format `[reg, value, shift, mask]`) is applied by the **Android kernel driver at probe — ABL does not apply it**. Diffing DT-intended vs live registers on a ferros boot: **7 of 8 differ**. Every ferros (and fastboot) boot runs Google's analog eye calibration missing:

| Reg | Name | DT wants | ABL/POR state |
|---|---|---|---|
| `G#50` | eusb_mode_control | `G#0A` | `G#02` |
| `G#70` | u_tx_adjust_port1 | `G#3C` | `G#7C` |
| `G#71` | u_hs_tx_pre_emphasis_p1 | `G#2C` | `G#3C` |
| `G#72` | u_rx_adjust_port1 | `G#90` | `G#92` |
| `G#73` | u_disconnect_squelch_port1 | `G#83` | `G#83` |
| `G#77` | e_hs_tx_pre_emphasis_p1 | `G#00` | `G#C8` |
| `G#78` | e_tx_adjust_port1 | `G#0B` | `G#16` |
| `G#79` | e_rx_adjust_port1 | `G#40` | `G#60` |

`payloads/repeatertune` applies the table with write+readback verify (REV_ID-gated — no writes unless the repeater positively identifies): **all 8 verified on hardware, with the USB link staying up through its own re-tune**. These were the first I2C writes from ferros.

**Sober caveat:** a hot-reloaded kernel that tunes before PHY init still took 3 PHY attempts / ~57s to re-enumerate, same `LTSTATE=G#FFFF4`/`LINKDBG=G#115` fingerprint. The tune is correct to apply but is not yet proven to close the coin flip — cold-boot statistics with the tuned kernel *flashed* are the real test.

## Bring-up plan (iterate over the RUN channel, no kernel reflash)

1. ~~Fill the two DT constants, read REV_ID~~ **DONE** — `REV_ID=G#3`, `REPEATER BUS UP`.
2. ~~Read the full tuning register set (healthy-boot baseline)~~ **DONE** — baseline block above.
3. ~~Diff intended vs actual tune~~ **DONE** — better than a flaky-boot diff: NO ferros boot ever had the tune (table above).
4. ~~Repeater tune sequence proven via payload~~ **DONE** — `repeatertune`, 8/8 verified live.
5. ~~Fold into the kernel~~ **DONE** — `ferros_hal::hsi2c` + `kernel_main` snapshots then applies the tune (REV_ID-gated) before PHY init; DIAG reports `REP_*` + `REP_TUNED` (`G#8` = full success, `G#FF` = gate skipped). Validated via hot-reload.
6. **Flash the tuned kernel to slot A** (needs fastboot) and gather cold-boot enumeration statistics vs the 13–240s historical spread. This decides whether the tune closes the coin flip or the remaining flakiness lives elsewhere (PHY init sequence, watchdog bounds, host timing).

## Incident notes: RUN hangs + A/B fallback (2026-08-16, partially resolved)

During reboot-cycle testing, `bridge run` hung indefinitely twice immediately after fresh enumerations, and the device later dropped off the bus needing a physical power-cycle. Controlled follow-up after the power-cycle:

- Cold boot: `dbg` clean, `diag` + `run` worked 4/4. Resting-state `out_armed=0` is normal (lazy re-arm); `DEPCMD failures=A#2` at init is boot noise.
- Warm PSCI-reset boot (89s flaky enumeration, 4 PHY attempts): `diag` AND `run` both worked — **warm reset is NOT the trigger.**
- Remaining suspect for the 2 hangs: firing the first PT command within the immediate post-enumeration window (the failing loop ran `run` the same second `status` first succeeded). Unreproduced under controlled timing; keep host-side timeouts on all bridge commands so a hang can never again leave the device held-open/wedged.
- The "no enumeration for 6 minutes" that followed was **ABL A/B rollback doing its job**: repeated warm resets exhausted slot A's retry counter and it silently booted Android from slot B. Not a wedge. Recovery: fastboot `--set-active=a`.

## What NOT to do

- Don't guess the HSI2C base — a blind write to the wrong controller is exactly the class of mistake the S2MPU/PMU episode ([pixel8_boot_findings]) warned about. Read it from DT.
- Don't recompute I2C timing — read ABL's calibrated `TIMING_*` values and reuse them.
- Probe (read `REV_ID`) before any write. Reads are safe; a write to a mis-identified bus is not.
