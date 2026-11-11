# TORCH: the rear flash LED as a torch

Owner, 2026-11-11: "would be nice to have the torch", then "can we vary the brightness of it at all?".
The quick settings' ring calls the `torch` endpoint; `payloads/torch` serves it behind the kernel's `torch` power switch (`ferros_kernel/src/torch.rs`), the KERNEL-OUT.md pattern from the start.
Torch mode only: flash, strobe and IR modes are never set.

## The hardware (husky and akita alike)

From the Android device trees in /mnt/Harbor/ferros-ref (the `kdev-shusky` and `kdev-akita` trees):

| | What | Source |
|---|---|---|
| Chip | TI LM3644, a dual-output flash LED driver (LWIS node `flash-lm3644`, `google,lwis-i2c-device`) | zuma-husky-camera.dtsi `flash0: flash@0`; zuma-akita-camera.dtsi the same |
| Bus | hsi2c_14, PERIC0 USI14 at G#1098_0000; pins gpp16-0/1, function 2, no pull (`hsi2c14_bus`); DT clock 390 kHz | zuma-usi.dtsi `hsi2c_14`; the camera dtsi's `&hsi2c_14` |
| Address | G#63, 8-bit registers and values | the flash node's `i2c-addr`, `reg-*-bitwidth` |
| Shares the bus with | the camera PMIC, SLG51002 at G#75 | zuma-akita-camera-pmic.dtsi; ferros camera.rs `PMIC_BUS` |
| Enable | HWEN on gpp8-2 (GPIO_PERIC0 + G#100, pin 2), active high; idle state input, no pull (`flash_en`, part of hsi2c_14's default pin state) | `enable-gpios = <&gpp8 2 GPIO_ACTIVE_HIGH>` |
| Strobe | husky: none in the DT. akita: gpp24-0, `GPIO_ACTIVE_LOW` (Android drives it low while the flash is enabled). ferros never touches it: the strobe input is ignored while the Enable register's strobe bit is clear, which it is out of reset and stays | zuma-akita-camera.dtsi `FLASH_STROBE GPP24[0]` |
| Rails | none listed for the flash node: the chip's VIN is not a switchable rail, so the off path is HWEN, not a supply | the flash node has no `-supply` |
| Driver logic | none in the open: LWIS only passes the closed camera HAL's register writes thru | LWIS |

The registers, from TI's LM3644 datasheet (register map), not yet read on the device: Enable G#01 (bit 7 TX pin enable, 1 out of reset; 6 strobe type; 5 strobe enable; 4 TORCH/TEMP pin enable; 3-2 mode: 00 standby, 01 IR, 10 torch, 11 flash; 1 LED2 enable; 0 LED1 enable), LED1 torch brightness G#05 (bit 7 LED2's torch current tied to LED1's, 6-0 the code), Flags G#0A and G#0B (cleared by reading), device ID G#0C.
HWEN low puts the chip in shutdown and every register back to its default (standby).

### Torch current and the cap

- The torch code is 7 bits: the LM3644's current is 0.977 mA + 1.4 mA per code (128 steps, 0.98 to 179 mA per output); the LM3644TT's is twice that (1.95 to 357.6 mA). Which part husky carries cannot be told from the source; the payload prints the device ID at the first light.
- Thermal guidance in the DT: none for the torch. The power hints' `LimitFlashCurrent` (`vendor.camera.max_flash_current`, 100 or 1500 mA) is the flash's. The power profiles (husky and akita `power_profile.xml`) put `camera.flashlight` at 240.47 mA from the battery: Android's own torch.
- So ferros drives LED1 alone (LED2 never enabled: whether an LED is on it is not known) and caps the code at 85, where the worst case, the TT part, draws 1.954 + 85 x 2.8 = 239.95 mA, Android's torch. On the plain part code 85 is 119.98 mA, about half. Derived in payloads/torch `TOP`; a measured Android torch (the hv oracle on hsi2c_14 during a torch-on, or the gauge's current) is what would move it.

### The dimmer: how fine it really is

Light goes with current and the eye with its logarithm, so equal turns of the ring should be equal ratios of current: I(L) = I(0) x (I(85) / I(0))^((L - 1) / 254) for levels 1 to 255, each rounded to the chip's step (payloads/torch `LEVELS`, computed at build time).

- **86 codes (0 to 85) are allowed; the curve uses 79 of them.** The 7 skipped (62, 67, 71, 75, 78, 81, 84) are near the top, where one level moves more than one step.
- The range is 122.8 to 1, about 6.9 stops, from 0.98 mA (TT 1.95) to 120 mA (TT 240).
- The bottom is coarse because the chip's lowest steps are big ratios: code 0 to 1 is 2.4x, 1 to 2 is 1.6x. So levels 1-29 are code 0, 30-61 code 1, 62-81 code 2; from about level 150 up nearly every level is its own step.

| Level | Code | Plain (mA) | TT (mA) |
|---|---|---|---|
| 1 | 0 | 0.98 | 1.95 |
| 30 | 1 | 2.38 | 4.75 |
| 62 | 2 | 3.78 | 7.55 |
| 96 | 4 | 6.58 | 13.15 |
| 128 | 7 | 10.78 | 21.55 |
| 160 | 13 | 19.18 | 38.35 |
| 192 | 25 | 35.98 | 71.95 |
| 224 | 47 | 66.78 | 133.55 |
| 240 | 64 | 90.58 | 181.15 |
| 255 | 85 | 119.98 | 239.95 |

## The kernel's part: the `torch` switch

`torch.rs` `power`:
- **On:** hsi2c_14's clock gates opened if something closed them, its pins to I2C (gpp16-0/1 function 2, no pull), USI14's clock from the oscillator, the controller brought up (`hsi2c::bring_up`, ~192 kHz as the camera tools run it), then HWEN an output, high, and 2 ms for the chip to leave shutdown.
- **Off:** HWEN driven low. One GPIO write: no bus, no PMIC, nothing that can fail.

The process's grants: `serve torch`, the `hsi2c14` hardware block, the `torch` switch. No new syscall.

### One owner for hsi2c_14

The camera PMIC is on the same bus, driven by the kernel's `diag cam …` tools.
- While the `torch` switch is on the bus is the torch process's, and every `diag cam …` is refused with a line saying so (service.rs).
- While a camera tool runs the switch cannot come on: both run on the kernel's core, and the service loop runs no process slice (so serves no SYS_POWER) while it runs a diag.
- The torch process touches the bus only between its switch's on and off. That part is the process's discipline, not enforced: the grant is the whole controller block.
- gpp8's other pins are the tele camera's bus (hsi2c_4); only the kernel writes gpp8, the camera tools and this switch, both on its core.

### The off path: is dark guaranteed?

- **Level 0:** the process writes standby, then switches off: HWEN low, the chip in shutdown.
- **A process end with the torch lit** (killed, faulted, one shot dead): the kernel turns off every switch it left on, on the service loop's next pass (process.rs `power_offs_apply`), so HWEN goes low within a pass (milliseconds). Until then the LED stays at its torch current, which is a continuous-use current anyway.
- **A kernel crash or reset:** the pins return to inputs, and the chip's own HWEN pulldown holds it in shutdown (Android's idle state for gpp8-2 is the same: input, no pull).
- **What is not guaranteed:** that HWEN is the only way to light the LED. The STROBE and TORCH/TEMP inputs are disabled by the register defaults HWEN low restores; a board that wired HWEN high in hardware would defeat this, and the DT says it is a GPIO.

## The process: payloads/torch

The `torch` endpoint, first byte the op:
- `0` SET + level (u8): 0 off, 1-255 the dimmer. Reply: [the level now], 0 when it could not be lit (the console says why).
- `1` LEVEL. Reply: [the level now].
- `2` REPORT (development): [level, code, device ID, Enable, Flags G#0A, Flags G#0B], read now; zeros while dark.

Lighting: the current (G#05, bit 7 clear) is written before torch mode is enabled (G#01 = G#89: TX pin enable as reset leaves it, torch mode, LED1), so it never lights at the reset code first. A change while lit writes G#05 only (the chip's own ramp, 1 ms out of reset, smooths it). Any failure leaves it dark. Latched fault flags go to the console.

## Who starts it

init's grants page is full (108 slots), so init cannot hand out a `torch` server's grants, and a process that sits idle holding an arc for a torch that is mostly off is the wrong shape anyway.
So the kernel starts it on demand (process.rs `ON_DEMAND`, service.rs): a call on `torch` that finds no server receiving asks the service loop (a `sev`, never a timer), which starts `torch` from its vault capsule with its own grants (`torch_grants`). The process ends itself once it is dark and nothing is waiting, so the arc comes back.
- A waiting call (no CALL_LIVE) is served by the server once it receives: the caller sees one slower answer.
- A live call (CALL_LIVE) gets ERR at once, as for any server not receiving, and the start goes ahead: the next call finds it.
- A call that lands between the process's last look and its end stays queued; the kernel sees it as the process ends (`fail_orphaned_calls`) and starts another.
- If it cannot start (no capsule stored, no arc free), its waiting calls are answered ERR and the next call tries again.
- `diag restart torch` and a `bridge spawn` by hand work as for any process.

The capsule: `tools/husky-shell.sh torch` builds, signs and stores it (it is not restarted: the next call starts it).

## On the device (not yet run)

```
tools/husky-shell.sh torch                                          # build, sign, store the capsule
ferros-bridge spawn torch.bin --name torch --serve torch --hw hsi2c14 --power torch   # or let the first call start it
ferros-bridge spawn epcall.bin 0020 --name epcall --use torch       # [0, 32]: lit, dim (code 1)
ferros-bridge spawn epcall.bin 02 --name epcall --use torch         # report: device ID, Enable G#89, flags
ferros-bridge spawn epcall.bin 00FF --name epcall --use torch       # [0, 255]: the cap
ferros-bridge spawn epcall.bin 0000 --name epcall --use torch       # [0, 0]: dark, and the process ends
ferros-bridge diag console
```
