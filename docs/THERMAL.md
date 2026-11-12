# THERMAL: the SoC's heat under ferros

Written 2026-11-12, after Mahere's map on the GPU took the Pro's SoC to 82-97 °C within minutes and the X3 ran at 364 MHz for seconds at a time while the kernel's own throttle never engaged (GPU-KISTS.md, second pass).
Everything here comes from the reference tree (/mnt/Harbor/ferros-ref/soc-gs: drivers/thermal/samsung, drivers/thermal/google, arch/arm64/boot/dts/google/zuma-*.dts*, include/dt-bindings/soc/google/zuma-bcl.h) and from ferros's own code; none of it has run on the device yet.

## 0. What the SoC has

### The sensors

zuma has two TMU blocks, TOP at G#100A_0000 (the CPU clusters) and SUB at G#100B_0000 (the rest), each with its own interrupt line.
The ACPM firmware owns both and their trim fuses; the AP asks over ACPM IPC channel 9 (exynos_acpm_tmu.c), as ferros has since 2026-10-23 (SENSORS.md).
There is no AP-side register path: TMU_IPC_REG_READ exists, but the TMU's register map is not in the tree.

| id | zone | heats with | probes | Android's control trip | Android's cooling |
|---|---|---|---|---|---|
| 0 | BIG | the X3 (CPU8, CPUCL2) | 2 | 90 °C passive | cpufreq_domain2 (the X3) |
| 1 | MID | the A715s (CPU4-7, CPUCL1) | 4 | 90 °C passive | cpufreq_domain1 |
| 2 | LITTLE | the A510s (CPU0-3, CPUCL0) | 2 | 95 °C passive | cpufreq_domain0 |
| 3 | G3D | the GPU (Mali-G715) | 3 | 90 °C passive | the Mali devfreq |
| 4 | ISP | the camera's ISP | 1 | none (alerts 70-105 °C) | isp_cooling, not mapped |
| 5 | TPU | the TPU | 3 | 90 °C passive | tpu_cooling |
| 6 | AUR | the AoC's audio core | 3 | 90 °C passive | gxp_cooling |

Each zone's TMU reading carries the zone's hottest probe plus a per-zone junction offset (zuma-b0-ipop.dts `junction_offset`, -5 to -10 °C on most probes, +5 on one BIG probe), applied by the firmware.

### Android's trips (zuma-b0-ipop.dts, the IPOP package; FOPLP differs only in gains and offsets)

Every zone has the same ladder, in °C:

| trip | BIG | MID | LITTLE | G3D | TPU | AUR | what it does |
|---|---|---|---|---|---|---|---|
| cold / switch-on | 20 / 70 | 20 / 70 | 20 / 70 | 20 / 70 | 20 / 70 | 20 / 70 | the governor starts tracking |
| early throttle | 80 | | | | | | BIG only: the firmware starts trimming 10 °C before control |
| control (passive) | 90 | 90 | 95 | 90 | 90 | 90 | the PI loop's set point; the cooling map hangs here |
| alert | 93 | 93, 95 | 97, 98 | 95, 98 | 95, 98 | 95, 98 | interrupt points for the governor |
| sw max effort | 97 | 97 | 99 | 102 | 102 | 102 | the governor at full cut |
| dfs | 100 | 100 | 100 | 105 | 105 | 105 | the firmware's emergency frequency drop (`dfs_on` in its shared state) |
| hot | 115 | 115 | 115 | 115 | 115 | 115 | Linux's "hot": shutdown territory |

The governor is not Linux's: every CPU and GPU zone says `use-acpm-gov`, and on zuma the firmware runs the loop itself every 25-50 ms (`polling_delay_on`), a PI controller over power (k_po, k_pu, k_i, `sustainable_power`: BIG 2000 mW, MID 1000, LITTLE 200, G3D 200, TPU 200), stepping each zone's cooling state; Linux reads the state from the firmware's shared memory and applies it as a frequency cap (freq_qos on the cpufreq policy, the Mali devfreq for the GPU).
BIG and MID add MPMM (`use-acpm-mpmm-throttle`: Arm's per-core activity-monitor throttle, no clock change) and BIG a hard limit thru the PID.
The firmware learns all this from Linux at probe: TMU_IPC_THRESHOLD, HYSTERESIS, INTEN and TMU_CONTROL per zone, TMU_IPC_SET_GOV_CONFIG, SET_PI_PARAM, the MPMM levels.
On a cold ferros boot Linux never ran, so the firmware's governor holds whatever its defaults are; after a jump from Android it holds Android's configuration (not measured either way: `diag thermal hw` now reads its target and trip counts).

### The battery-current limiter (BCL): the hardware's own clock divider

Separate from heat, zuma has a clock divider in each CPU cluster and in the GPU, TPU and AUR, driven by the PMICs' warning lines (google_bcl, zuma-bcl.h):
- OCP_WARN and SOFT_OCP_WARN of each big buck: S2MPG14 B2M (the X3), B3M (the A715s), B7M (the TPU), S2MPG15 B2S (the GPU), wired to gpa10-0..3 and gpa9-2..3 and to the dividers.
- SMPL_WARN: the battery or VSYS falling under its threshold (Android sets 2.9 V).
- VDROOP1 and VDROOP2 (gpa9-5, gpa9-6): the IF-PMIC's droop and battery over-current (BATOILO) signals.

The divider steps the domain's clock down in hardware, within microseconds, with no software in the loop.
Its registers sit in each cluster's block: CPUCL0 G#29C0_0000, CPUCL1 G#29D0_0000, CPUCL2 G#29D8_0000, G3D G#1EE0_0000 (+ G#830 CLKDIVSTEP, G#834-G#83C the OCP, VDROOP and SMPL filters, G#840 / G#844 the heavy and light step, G#848 the status; CPUCL0's status at G#83C, G3D's at G#854).
ACPM's power-management firmware arms them on every boot (flexpmu_cal_system_zuma.h `pmucal_lpm_init`): CPUCL1 and CPUCL2 CLKDIVSTEP = G#210407, CON_HEAVY G#FFF0_41C0, CON_LIGHT G#F0_41C3, OCP and VDROOP filters G#C007_F8FF, SMPL filter G#8000_FFFF.
Android's BCL driver then writes each cluster's CLKDIVSTEP to G#1 (`cpu2_clkdivstep = <0x1>`, zuma-board-bcl.dtsi) and sets the PMIC's warning levels (B2M 8 A two-phase, B3M and B7M 13.5 A three-phase, B2S 8 A; SMPL 2.9 V with 200 mV hysteresis).
ferros does neither: the divider runs on the firmware's G#210407 and the PMIC's warning registers hold whatever ABL or the reset left.
The field layout of CLKDIVSTEP is not in the tree; that Android replaces G#210407 with G#1 at every probe suggests it narrows what triggers the divider or how far it divides.

## 1. The 364 MHz drop: who did it

The X3 at 364 MHz is 2914 / 8 (2910 / 8 is 363.75): a power-of-two division of the rate the clock policy asked.
It is not a DVFS rate: the X3's lowest operating point is 500 MHz (zuma-cpu.dtsi), and ACPM's DVFS answers in operating points.
So it is not ferros's DVFS (the clock policy asks 500, 2000 or 2914 MHz, the step throttle 1400), and not ACPM's thermal governor either, which caps thru DVFS and its OPPs.
What divides a cluster's clock below its table is the BCL divider above.
The timing says the same: raising the A715s from 1065 to 2367 MHz took the X3 to 364 MHz at once, faster than any heat moves.
By Android's own energy model (zuma-b0-ipop.dts `pixel_em`, the default profile) an A715 draws 208 mW at 1065 MHz and 1514 mW at 2367, so that one request added about 5.2 W to a SoC already carrying the X3 (3.7 W at 2914 busy) and the GPU at 890 MHz: a step in battery current big enough to sag VSYS or trip the IF-PMIC's over-current, which reach every cluster's divider (VDROOP, SMPL_WARN), not just the A715s'.
The long dives under steady load fit the same divider fed by the X3's own buck (B2M's OCP_WARN, whose level ferros never set).
Which line it was is not yet measured; the status word says, and ferros now keeps it (below).

Why the kernel's throttle never engaged, from the code as it stood:
- It looked at the CPU zones only (BIG, MID, LITTLE), at 95 °C. Under Mahere the GPU's zone is the likely hottest; it never counted.
- When it did fire, it set the X3 to 1.4 GHz by hand, and the clock policy's next profile switch (a touch, the frames speeding up or slowing down) asked 2.0 or 2.914 GHz straight over it. Fixed: every request now goes thru the thermal cap (`thermal::capped`).
- "`diag thermal` listed only the LITTLE zone": no path in the code prints only one zone; every zone printed a reading or an error. A reply matched to the wrong request could have carried one zone's reading under another's name: the reply's zone is now checked (`acpm::tmu_read`, E_ZONE), and each zone counts its failed reads.

## 2. What ferros reads now (built 2026-11-12, kernel thermal.rs)

Once a second on the service loop, on the kernel's own core, where the read was already made:
- Every zone over ACPM (28 us each), its peak since boot, and its failed reads with the last error.
- The kernel's core's clock, measured: 2^16 dependent adds with IRQs held off, timed on the system counter (22 us at 2.9 GHz, 180 us at 364 MHz).
- The rate the firmware holds for the X3 (acpm::dvfs_get).
- Each divider's status word (CPUCL0 always, CPUCL1 while a worker is up, CPUCL2 while the kernel runs on the X3, G3D while its domain is on), the last value and every bit seen.

From those, two counters name the culprit:
- slow seconds: the X3 measured more than an eighth under the firmware's rate. No DVFS rate does that; a divider does.
- firmware-capped seconds: the firmware holding the X3 more than an eighth under what ferros asked. That is the ACPM governor (or another firmware limit).
- (ferros's own caps are counted apart, as capped seconds.)

The cost: about 0.25 ms of the X3 a second (seven zone reads, one rate read, the add chain, four register reads).
Not an interrupt: the TMU's threshold interrupts go to the ACPM firmware (it owns the TMU), and the die's heat moves over seconds; the divider and the firmware's own trips act in microseconds and milliseconds below it.

Where it shows:
- `bridge diag thermal`: every zone (now, peak, status, probes, failed reads), the hottest named, the kernel's core's clock against the firmware's rate and ferros's ask, the slow, firmware-capped and capped seconds, the dividers (last, seen), the policy and its caps.
- `bridge diag "thermal hw"`: read only, what arms the hardware's protections: each cluster's divider configuration words, the PMICs' OCP_WARN, SOFT_OCP_WARN and debounce registers and SMPL_WARN_CTRL, and the ACPM governor's own view (TMU_IPC_GET_TARGET_FREQ, and each zone's eight trip counters, TMU_IPC_GET_TRIP_CNT, as Linux reads them from sysfs).
- `bridge diag "thermal reset"`: peaks, counts and seen bits start again.
- DIAG: SOC_C (the hottest zone, °C), SOC_ZONE (which, 0-6), THROTTLED_MHZ (the X3's uncapped rate while ferros caps it), CORE_MHZ, X3_HELD_MHZ, X3_SLOW_S, X3_FW_CAP_S, CLKDIV_CL2_SEEN.
- SYS_FACTS: fact 10 the hottest zone's °C (as before), fact 86 which zone (ferros_sys `Facts::soc_hottest_zone`), fact 11 whether ferros's policy caps a domain.
  The device circle's "chip" row still shows only the degrees: naming the zone there needs the zones' names as words in every language (owner decision 8).

The test that settles section 1: run Mahere as on 2026-11-12, then `diag thermal` and `diag "thermal hw"`.
X3_SLOW_S climbing with CLKDIV_CL2_SEEN non-zero is the divider; the seen bits, against the OCP, VDROOP and SMPL filters, say which line; X3_FW_CAP_S climbing instead is the firmware's governor.
Repeat with `diag dvfs set 3 2367000` to reproduce the instant drop.

## 3. The policy

### What is built

The thermal policy acts only thru caps: the clock policy asks each domain's rate thru `thermal::capped`, which returns the lower of what it wants and the domain's cap, so the two never fight.
`bridge diag "thermal policy step|smooth|off"`; every boot starts at `step`.

- **step** (the default, as since 2026-10-23, now kept across profile switches): the hottest CPU zone at 95 °C caps the X3 at 1.4 GHz; below 85 the cap goes.
- **smooth** (built, off by default, never run on the device): one controller per domain, Android's zones and trips:

| domain | zone | trip | floor (zuma-cpu.dtsi, zuma-gpu.dtsi) |
|---|---|---|---|
| X3 (CPUCL2) | BIG | 90 °C | 500 MHz |
| A715s (CPUCL1) | MID | 90 °C | 402 MHz |
| A510s (CPUCL0) | LITTLE | 95 °C | 324 MHz |
| GPU (G3D) | G3D | 90 °C | 150 MHz |

  Once a second each cap moves by s·e/(|e| + 4): e the zone's temperature less its trip, s a sixteenth of the rate the clock policy wants.
  A degree over takes a fifth of s off, four degrees half, far over nearly all; under the trip the cap climbs back the same way.
  The slope is continuous in e, so nothing jumps as a zone crosses its trip, and no second moves a clock by more than a sixteenth: a frame's time stretches by at most about 6 % from one second to the next, the frame rate sliding instead of falling off a cliff.
  The cap stays between the domain's floor (under it the device resets: CPUCL0 at 300 MHz, 2026-10-25) and the clock policy's rate (above it the cap would wind up, then take seconds to unwind when heat comes).
  It is integral control on the rate: it settles where the heat the clocks make equals what the phone sheds at the trip, which is exactly the sustained rate.
  What it does not do (Android's governor does): work in watts against a power table, MPMM, the early throttle at 80 °C, the TPU and AUR zones.
- **off**: no cap (for measuring heat); the firmware's trips and the hardware's divider remain.
- `bridge diag "thermal at <C>"` moves the step trip and caps every smooth trip at that temperature, to test either policy on a cool device.
- While `diag dvfs policy off` holds the clocks by hand, neither policy asks anything.

### What 120 frames a second costs in heat

From Android's energy model (`pixel_em`, mW for one busy core):

| core | rate | power | energy a cycle |
|---|---|---|---|
| X3 | 2914 MHz | 3714 mW | 1.27 nJ |
| X3 | 2556 MHz | 1911 mW | 0.75 nJ |
| X3 | 2049 MHz | 1034 mW | 0.50 nJ |
| A715 | 2367 MHz | 1514 mW | 0.64 nJ |
| A715 | 1572 MHz | 449 mW | 0.29 nJ |
| A715 | 1328 MHz | 320 mW | 0.24 nJ |
| A715 | 1065 MHz | 208 mW | 0.20 nJ |
| A510 | 1425 MHz | 57 mW | 0.04 nJ |

Measured on ferros (POWER.md): the X3 from 2914 to 2000 MHz saved 0.66 W at rest, its top step being the steepest.
Android's own sustainable-power figures for the zones it governs add to about 3.4 W (BIG 2, MID 1, LITTLE, G3D and TPU 0.2 each): roughly what the Pro sheds at its trips, before the display (about 1.2 W lit).

Against that, a whole-screen GPU app at 120:
- The panel stage on NEON: about 4.6 G cycles a second (GPU-KISTS.md). On the four A715s at 1328 MHz (5.3 G cycles a second, their IPC about 0.87 of the X3's) it just fits, for about 1.3 W; at 2367 MHz it would run with room to spare at about 6 W; on the X3 at 2914 it does not fit at all.
- Mahere's plan and the display's other work on the X3: at 2914 MHz up to 3.7 W, at 2049 about 1 W.
- The GPU at 890 MHz: not in the model; Android's GPU table pairs 890 MHz with MIF at 3172 MHz (ferros runs MIF at 2028). Measure it: `diag odpm` over the run, the S2S_VDD_G3D and S8S_VDD_G3D_L2 rails.

So 120 with every core and the GPU at their tops is a burst of a minute or two, then the trips; sustained, it is two to three times the budget.
Cycles are cheapest wide and slow: an A715 at 1.3 GHz spends a fifth of what the X3 at 2.9 GHz spends a cycle.
The panel stage on the GPU (GPU-KISTS.md's first choice) moves 4.6 G CPU cycles a second onto silicon built for it.

## 4. What the owner decides

0. **The policy at boot:** step (as today), smooth (after a device test), or none of ferros's own beside the firmware's.
1. **Which zones count:** the step policy looks at the CPU zones only; add the GPU's (the likely hottest under Mahere)?
2. **The trips:** Android's (90, 90, 95, 90 °C) or ferros's own; and whether ferros's policy may lower the GPU, which a 120 frame budget feels first.
3. **The A715s' active clock:** 1065 MHz (0.8 W for four busy), 1328-1572 (1.3-1.8 W), 2367 (6 W, which also trips the divider at once).
4. **The X3's top:** 2914 MHz (3.7 W busy), 2556 (1.9 W) or 2049 (1 W) in the active profile; the top step buys 14 % more speed for twice the power.
5. **The GPU's clock:** 890 MHz whenever a `gpu` process lives (today), per job, or by queue depth; and the MIF Android pairs with it.
6. **Sustained or burst:** 120 for as long as the heat allows, then sliding (smooth), or clocks chosen so a lower rate holds forever.
7. **The divider and the firmware's governor:** leave both as the firmware arms them (today), configure them as Android does (CLKDIVSTEP G#1, the PMIC's warning levels, the governor's trips and gains: writes to cluster and PMIC registers, to be tested one at a time), or switch the divider's triggers off (a sag the divider would have absorbed then reaches the PMIC's own protection, which ends in a reset rather than a slow frame).
8. **The chip row's words:** whether the device circle names the hottest part (the X3, the GPU, …), which needs those names translated.
