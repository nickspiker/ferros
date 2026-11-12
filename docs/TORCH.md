# TORCH: the rear flash LED as a torch

Owner, 2026-11-11: "would be nice to have the torch", then "can we vary the brightness of it at all?".
The quick settings' ring calls the `torch` endpoint; `payloads/torch` serves it behind the kernel's `torch` power switch (`ferros_kernel/src/torch.rs`), the KERNEL-OUT.md pattern from the start.
Torch mode only: the chip's flash and IR modes and its strobe input are never set; the strobe on the torch page is the driver's own, torch mode switched on and off by its clock ("Settings" below).

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
- `3` APPLY: its settings (preferences 42-45) read again and put in effect at the level it has. Reply: [the level now]. The map sends it when the torch page changes one while the torch is lit.

Lighting: the current (G#05, bit 7 clear) is written before torch mode is enabled (G#01 = G#89: TX pin enable as reset leaves it, torch mode, LED1), so it never lights at the reset code first. A change while lit writes G#05 only (the chip's own ramp, 1 ms out of reset, smooths it). Any failure leaves it dark. Latched fault flags go to the console.

**Two threads** (2026-11-11, for the strobe and the timeout). The main thread receives and answers; the light thread (`light`) owns the bus, the switch and the clock and does everything that touches the chip, so a strobe edge never meets a request's write on the bus. A request is a word (`WANT`: its number, the level, a report bit) and its answer another (`NOW`: the request taken, the level now), each a doorbell (`wait` and `wake`); the main thread waits only on the light thread's own work, a bus write or two.

## Settings: the strobe, the lit share, the timeout (2026-11-11)

Owner, 2026-11-11: "like flashlight we may want to control the brightness and strobe frequency and duty cycle and timeout". Set on the torch page (QUICK.md "Quick pages"), kept on the device (preferences 42-45, SETTINGS.md), one encoding shared by the map and the torch (ferros_sys::torch), read by the torch at every request:

| Setting | Kept as | Range |
|---|---|---|
| Level its page's on lights it at | `torch_level`, 1-255 (0: never set, the whole dimmer) | the dimmer above |
| Strobe | `torch_strobe`, 0 steady, else 32nds of an octave up from a flash every 4 s | 1/4 to 16 flashes a second (codes 1-193) |
| Lit share of each flash | `torch_duty`, a signed byte, 256ths either side of a half | 8/256 to 248/256 |
| Turns off after | `torch_timeout`, 0 never, else 16ths of an octave up from 16 s | 16 s to 2^14 s, about 4 h 33 min (codes 1-161) |

### The strobe

- **What it is.** Torch mode switched on and off by the driver, one Enable write an edge (G#89 lit, G#80 standby; HWEN stays high, so the current stays in its register thru the dark part). The chip's own strobe (its STROBE pin and the Enable register's strobe bit) is a flash trigger, not a rhythm, and is never used.
- **Its clock.** The light thread works out from the timer count where in the period it is, counted in whole periods from an anchor (the light coming on, or the strobe or its share changing), writes Enable when that differs from the chip's mode, and sleeps on the kernel's deadline to the next edge (`wait_world` on its request word with that deadline). No edge waits on another, so the rhythm never drifts, and a late wake lands on the right part of the period. A level change mid-strobe keeps the anchor and the rhythm.
- **Its limits.** At 16 a second each edge is a bus write of three bytes at about 192 kHz (under 0.2 ms) and a wake; 32 wakes a second. The chip ramps the torch current up over 1 ms out of reset (its Timing register, never changed), so a flash shorter than a couple of milliseconds lights dimmer than asked: the lit share stops at 8/256, 1.95 ms at 16 a second.
- **Not measured.** The edges' jitter on the device, and whether the 1 ms ramp shows as soft edges to the eye.

### The timeout: why this is a timer, and the one kind allowed

The owner's rule is no polling and no timers unless proven necessary: push or doorbell, never a look every so often. The timeout is that case:
- **The event is the time itself.** Nothing in the world rings when four minutes have passed; there is no doorbell to wait on but the clock. Moving it elsewhere (the map) moves the same timer and adds a process that must outlive the torch's request.
- **Why have it.** A torch left on in a pocket draws up to 240 mA (the TT part at the cap) until someone notices: about a twentieth of the Pro's 5050 mAh an hour, for hours, and heat beside the camera. It is the person's choice, off ("never") unless set.
- **How it is kept honest.** One deadline on the light thread, the kernel's (`wait_world`'s deadline, the same call that waits for the next request): the thread sleeps until the request word changes or the count reaches the end, whichever is first, and does nothing in between. No wake that finds nothing to do. Every request (a tap, a pull, a page change) counts it again from then: any touch of the torch is the person using it.
- **When it ends.** Dark as level 0 is (standby, then the switch off), said on the console (`torch: its timeout ended; dark`). The process then waits for its next call before it ends (its main thread is in a receive nothing interrupts), so a torch put out by its timeout keeps its arc until then; the quick row asks its level as it next shows (it was last lit), which serves that call, reads dark, and lets it end.

## Who starts it

init's grants page is full (108 slots), so init cannot hand out a `torch` server's grants, and a process that sits idle holding an arc for a torch that is mostly off is the wrong shape anyway.
So the kernel starts it on demand (process.rs `ON_DEMAND`, service.rs): a call on `torch` that finds no server receiving asks the service loop (a `sev`, never a timer), which starts `torch` from its vault capsule with its own grants (`torch_grants`). The process ends itself once it is dark and nothing is waiting, so the arc comes back.
- A waiting call (no CALL_LIVE) is served by the server once it receives: the caller sees one slower answer.
- A live call (CALL_LIVE) gets ERR at once, as for any server not receiving, and the start goes ahead: the next call finds it. The torch so started waits for that next call (it does not end until it has served one and is dark). So the quick settings should call `torch` waiting (`call`, not `call_live`): the first tap lights it after the start.
- A call that lands between the process's last look and its end stays queued; the kernel sees it as the process ends (`fail_orphaned_calls`) and starts another.
- If it cannot start (no capsule stored, no arc free), its waiting calls are answered ERR and the next call tries again.
- `diag restart torch` and a `bridge spawn` by hand work as for any process.

The capsule: `tools/husky-shell.sh torch` builds, signs and stores it (it is not restarted: the next call starts it).

## On the device (not yet run)

epcall calls live, so with no torch running its first call answers ERR and starts one; spawning it by hand first avoids that. After `[0, 0]` the process ends, and the next call starts another from the stored capsule.

```
tools/husky-shell.sh torch                                          # build, sign, store the capsule
ferros-bridge spawn torch.bin --name torch --serve torch --hw hsi2c14 --power torch   # or let the first call start it
ferros-bridge spawn epcall.bin 0020 --name epcall --use torch       # [0, 32]: lit, dim (code 1)
ferros-bridge spawn epcall.bin 02 --name epcall --use torch         # report: device ID, Enable G#89, flags
ferros-bridge spawn epcall.bin 00FF --name epcall --use torch       # [0, 255]: the cap
ferros-bridge spawn epcall.bin 0000 --name epcall --use torch       # [0, 0]: dark, and the process ends
ferros-bridge diag console
```

The settings from the bench (kept in the device document as the torch page keeps them), then the strobe seen with the torch lit:

```
ferros-bridge diag "pref torch_strobe 129"                          # 129: four octaves up from 1/4 Hz, 4 flashes a second
ferros-bridge diag "pref torch_duty 0"                              # half lit
ferros-bridge diag "pref torch_timeout 17"                          # 17: one octave up from 16 s, 32 s
ferros-bridge spawn epcall.bin 0080 --name epcall --use torch       # lit at 128: strobing, dark again 32 s later
ferros-bridge spawn epcall.bin 03 --name epcall --use torch         # apply: the settings read again at once
```
