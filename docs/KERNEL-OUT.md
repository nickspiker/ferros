# What leaves the kernel

The kernel keeps only what a kernel must, plus the killswitch (owner, 2026-10-31). Everything else moves into processes, a module at a time, each move verified on the device before the kernel's copy goes. This is the plan and the record; USB.md has the first move in detail.

## What stays

About 8k lines:
- **Processes:** scheduling, capabilities, IPC, syscalls (`process.rs`), the cores (`smp.rs`, `x3.rs`), the page tables (`cachemap.rs`).
- **Protection:** the DMA fences (`s2mpu.rs`), the capsule gate (`oracle.rs`).
- **Keys:** the ira, the wairua, KOHI, held in registers (`keys`, `keyreg`, `ira`, `wairua`, `kohi`).
- **The vault and auth:** `kvault`, `vault_boot`, `ufs_device` (AUTH.md: auth stays mostly in the kernel).
- **The killswitch,** reset and the watchdogs (`killswitch`, `platform`), the flash log.
- **The boot before any process exists:** the Ready? gate, the checklist screen and its text, slot marking, the modem's handover block (read while ABL's device tree is there).
- **Thin brokers:** the instructions only EL2 may issue — SMC/FF-A into the secure world, the EL3 register window (`priv_reg`), ACPM to the PMICs — behind narrow power switches or syscalls, never a general SMC from EL0.

## The pattern

Each move so far followed the same shape:
1. **Grants.** The block's registers mapped for EL0 (`HARDWARE`, `DMA`; blocks in the first GiB through `cachemap.rs` `L2_LO`), its interrupt (`IRQS`).
2. **A power switch** (`POWERS`, `SYS_POWER`) for what only the kernel may do: PMIC rails over ACPM, EL3 registers, clock gates outside the block, pin modes in banks other drivers share. It runs on the kernel's core, so buses the kernel also uses (hsi2c_11: the battery gauge, the TCPC, the eUSB2 repeater) are never driven from two cores at once.
3. **The protocol in the process,** using a shared no_std crate where the kernel's code was already clean (`ferros_hal::usb`).
4. **init starts it** from a vault capsule, with a slot list in `payloads/init` and `service.rs` `home_grants`/`shell_grants`.
5. **The kernel's copy goes,** including its `diag` subcommands, once the process has run through a cold boot.

## The order

| # | What | Lines | Kernel dependencies (outside its own block) | Becomes |
|---|---|---|---|---|
| 0 | USB + PT (`ferros_hal::usb`, the service loop's USB, `ptlink.rs`) | ~1.5k + the loop | eUSB2 PHY, TCPC switch, DMA path | **Done** (USB.md): the process from boot, interrupt-driven, about 29 MB/s; the kernel's USB removed 2026-11-01. |
| 1 | Touch bring-up (`touch.rs` `up`) | ~0.9k | ACPM rails (LDO25M, LDO4M), HSI0 USI2 clock gates, the reset GPIO | A `touch` power switch (clocks, rails, reset); the controller handshake (ready event, GPIO6 fix-up, scan mode) in `payloads/touch`, which already speaks SPI to it. The `diag touch …` tools go with it. |
| 2 | The PT command chain in `service.rs` (~2,000 lines) | ~2k | everything it reaches | Each `diag` subcommand leaves with its module. The rest — store, spawn, seal, mem, display, reload, reboot — stays behind `SYS_PT` until a dev-command server takes the dev-only ones; reload and reboot remain kernel. |
| 3 | Camera (`camera`, `_snap`, `_events`, `_close`, `_rx`, `_rx_uw`) | ~6.5k | hsi2c (polled, portable), `priv_reg` ×3, ACPM ×3 (the camera PMIC), `display::show` for diag output | `payloads/camera`, a diag process at first (`bridge spawn camera <command>`): ~20 register windows granted, a `camera` power switch for the PMIC and EL3 writes, hsi2c moved into a shared crate. `camera_snap` is captured oracle data: it moves as data, not code. Nothing at boot uses the camera, so this move cannot break a boot. |
| 4 | Sensors (`usf`, `aoc`, `trusty`, `gsa`, `ffa`, `tpu`) | ~2.7k | FF-A SMCs and memory sharing (trusty), GSA's mailbox (MMIO), ACPM (AoC rails), the proximity gate on the power key | **Done** 2026-11-01: `payloads/sensors` holds the whole USF conversation and publishes the samples the kernel answers `SYS_FACTS`, `SYS_LIGHT` and the wake rule with; `usf.rs` is gone. The kernel keeps the AoC's start (`aoc`, `trusty`, `gsa`, `ffa`) behind the `sensors` switch (decision 2). `tpu` is not a sensor and stays for now. |
| 5 | Power (`odpm`, `pd`, `rtc`, the loop's gauge/power/thermal polls, `clock_policy`) | ~1.1k + polls | ACPM throughout, hsi2c_11 (gauge, shared with the TCPC) | A power server for ODPM, the RTC, the gauge and power-domain gating. Thermal throttling keeps a crude kernel fallback (a safety job). DVFS (`clock_policy`) sits beside the scheduler and may stay. |
| 6 | Display (`display`, `dpu`, `dsim`) | ~1.3k | `priv_reg` (power-domain check), the boot screen | **Mostly done** 2026-11-01: the boot modeset is the display server's (`payloads/display` modeset.rs) and `dpu.rs`, `dsim.rs` and `hk3.rs` are gone with the kernel's DSI tools. `display.rs` stays for the boot screen and checklist (`show` on ABL's pipe) and its remaining dev tools (fill, patches, dither, TE rate). |
| 7 | Small drivers (`amp`, `haptics`, `gps`, `spi`, `fp`), the radios' power switches | ~1k | spi (portable), ACPM, `aoc::probe32` (haptics) | Each its own process; `spi.rs` becomes a shared crate (touch, amp, gps, nfc all use it). `fp` is a bus hunt: a diag payload. |
| 8 | `partfs` (ext4/f2fs on Android's partitions) | ~0.4k | the UFS link under the kernel lock | A read-only file server, once a process can be given block access to a partition without the vault's LU. |
| 9 | Policy in the service loop: the AoC boot sequence, the home start | — | — | init (restart policy is init's). **The WiFi keeper moved 2026-11-01:** init's keepers restart `usb` and `pcie1` with `spawn_unique` (USB.md step 5). |
| 10 | Probes: `cold_ufs`, `shim.rs` | ~0.6k | — | **`cold_ufs` done** 2026-11-01: the link-startup probe (`bridge diag ufs`) is gone; the cold boot's power-on and bring-up stay. `shim.rs` stays while the Linux register oracle is used. |

## Rules for each move

- **Measure first.** The process's version is verified on the device against the kernel's (the same register reads, the same results) before the kernel's copy is removed.
- **Boot paths last.** A move that sits on the cold boot's path (touch, display, sensors' proximity) goes through a slot b flash and a cold boot before the kernel's copy goes; slot a stays the last known-good escape while it settles.
- **No general escape hatches.** A power switch does one device's sequence. No syscall hands EL0 an arbitrary SMC, EL3 register, or PMIC register.

## Decisions needed (owner)

What blocks the rest of the list after the night of 2026-10-31. Each is a choice, not more work of the same kind.

1. **[decided 2026-11-01] The kernel's USB fallback:** removed with the owner present (USB.md step 5); init's keepers start the driver again.
2. **[decided 2026-11-01] Sensors phase 2: the AoC's start.** Not an `Ffa` capability: the kernel can check which Trusty calls a process makes but not what its messages say, so that would be a new door into the secure world. Instead the AoC's start is the `sensors` power switch's "on" (the pattern of every other move): the process asks, the kernel's service loop runs its start a step a pass, and the kernel has no USF of its own. Done the same day (see the record).
3. **The camera harness.**
   - `camera*.rs` (6.5k lines) is the bring-up tool, not a driver. It DMAs into fixed physical regions and draws its viewfinder on the panel directly.
   - Options:
     - port it as it is (hours, and it changes the tool you are using);
     - leave it until the pipeline is understood, then write the userspace driver fresh and delete the harness;
     - delete it now.
   - Proposed: the second.
4. **[decided 2026-11-01] The `diag` subcommands.** Three buckets. Deleted, their job done: the touch tools (and `touch.rs`'s own bring-up, the `touch` switch kept), the AoC's bring-up tools (`aoc` keeps a status), `gsa`, `ffa`, `trusty`, `fp` and `tpu`. Kept, the kernel inspecting itself: `smp`, `x3`, `sched`, `threads`, `cpu`, `heap`, `console`, `log`, `rep`, `boot`, `ready`, `bench`, `facts`, `screen`, `thermal`, `rtc`, the vault and auth tools, `android`, the power measuring. Moved with their module when it moves: `amp`, `haptics`, `gps`, `modem`, `cam` (decision 3). And every development command is compiled out of a production build, not only refused there. Done the same day (see the record).
5. **hsi2c_11.**
   - The battery gauge, the Type-C controller (the USB switch) and the eUSB2 repeater share it. Everything on it runs on the kernel's core today, so nothing collides.
   - Moving the gauge out means one process owning the bus: a charger process, with the usb switch asking it to close the Type-C switch.
   - Proposed: leave it in the kernel until there is a charger process for other reasons (charging policy).
6. **[decided 2026-11-01] The boot modeset.** It moves into the display server's start, run only while the pipe is still at 8 bpc (so a restart does not redo it); `dpu`/`dsim` then leave the kernel with their `diag` tools. On the attest screen's path, so with the owner present and through a slot b cold boot. The display server's grants for it are in place (slots 97-100: `decon0-sub`, `dqe0`, `dqe0-cgc`, `dpu-sysreg`). Done the same day (see the record).

7. **The speaker amplifiers (`amp.rs`, found 2026-11-04).** The protocol is easy to move: 32-bit SPI reads of the CS35L41's registers. The pins are the problem.
   - The right amplifier's chip select is a GPIO (gpn1-0), toggled on every transaction. It sits in the bank the touch driver holds (`gpio-gpn0` covers gpn0 and gpn1).
   - Its reset (gph2-1) sits in HSI1's bank, which the modem driver holds.
   - A second process writing those banks on every transfer would race their owners' read-modify-writes.
   - Options:
     - a GPIO server that owns every shared bank;
     - a narrow kernel call that drives one granted pin;
     - leave `amp` in the kernel until audio needs it (nothing plays sound yet).
   - Proposed: the third, then the second once audio is built (the power switch does the rail and both resets, the process asks the kernel for its chip select).
8. **Haptics (`haptics.rs`, found 2026-11-04).** The CS40L26 is on hsi2c_0, which the NFC driver owns (`hsi2c0`). Today `diag haptic` in the kernel and the NFC process can drive that bus at the same time. Nothing runs `diag haptic` unasked, but it is a real collision.
   - Options:
     - the haptics protocol inside the NFC driver, so the bus has one owner;
     - an I2C bus server;
     - the kernel keeps haptics and the NFC driver gives up the bus while `diag haptic` runs.
   - Proposed: the first, as a "bus owner serves its neighbours" pattern. The same question will come for hsi2c_11 (decision 5).
9. **The power group (`odpm`, `rtc`, `pd`, the gauge, found 2026-11-04).** Every one of them is ACPM or SPMI underneath, which only the kernel may issue ("Thin brokers").
   - Moving them means a broker shaped per device (a read-only ODPM sample call, an RTC read and set, the domain switches as power switches) rather than processes doing the protocol.
   - Proposed: leave them until a power server has a consumer (a battery dashboard, charging policy), then move them together with hsi2c_11's gauge.
10. **`partfs` (found 2026-11-04).** It reads ext4 and f2fs files on Android's partitions for the modem driver (its image, NV, carrier configs) and the Bluetooth driver (its patch).
    - Moving it needs a way to give a process read access to one partition's blocks without the vault's LU (an `ufs` block grant, read only, per partition).
    - Proposed: build the block grant when a second consumer of Android's partitions appears; until then the kernel's reader is one module serving two drivers.
## Ready without the owner (2026-11-07, night)

What on this list and VAULT-OUT.md's "Still to do" needs no decision, in the order it is being built, most value for least risk first.
Each is one commit, kernel and payload together, and none is on the cold boot's path unless said.

0. **The kernel's own channel to the vault process, and the bridge's `store` commands through it.**
   The kernel becomes a caller of the `vault` endpoint with no thread behind it: its reply lands in a mailbox the service loop reads, and the PT reply waits for it (the USB process already polls for a reply that is not ready).
   Value: `store ls`, `get` and `rm` of a moved name find it again, and `put` writes straight into the vault process, past the old 3 MiB limit.
   Risk: low; a development door, compiled out of a production build, and with no vault process the kernel's vault answers as before.
   It is also the channel the next three need.
1. **Release drops the person's domains in the vault process.**
   At release (a completed departure) the kernel stamps a drop of each of the person's domains while their keys still exist, and the service loop sends them before its restart.
   Value: a departed person's entries leave the disk, not only become unopenable.
   Risk: low; runs only at release, and a failed drop still restarts (the renewed device-only secret already makes the entries unopenable).
2. **The identity export through the vault process** (`SYS_UNLOCK` op 5).
   Forwarded as a store call is, stamped with the person's domains; the vault builds the document and hands it out a transfer at a time (the op gains an offset).
   Value: the departure's "save first" finds what moved. Nothing calls it yet, so the risk is nil.
3. **The vault's capsule in the kernel image** (VAULT-OUT.md decision 0), optional at build time: with `FERROS_VAULT_CAPSULE` set the kernel embeds that capsule and starts the vault from it; without it, from its own vault as today.
   Risk: on the boot path, so it waits for a slot b cold boot with the owner present before the stored copy goes.

Not ready, and why:
- **The kernel's own names behind its channel, then `kvault`'s storage path retired** (VAULT-OUT.md step 4): the kernel reads `init`, `sealed` and the capsules before any process runs, and the AoC firmware in the service loop's sensor steps. Moving them needs the vault started from the kernel image (3, verified from cold) and a synchronous read for the boot. After 3.
- **`amp`** waits on decision 7, **`haptics`** on decision 8, the power group on 9, `partfs` on 10, the camera on 3.
- **`spi.rs` as a shared crate** has value only once `amp` leaves (the kernel's last SPI user besides the power switches' pin setup); with decision 7 open, it waits.
- **gps**: already a process (`payloads/gps`); the kernel keeps only its power switch. **fp**: deleted (decision 4).
- **Policy in the service loop** (move 9): the AoC's start runs a step a pass in the kernel because it is ACPM, GSA and Trusty underneath (thin brokers); what is policy in it is the 4 s wait, too small to move alone.

## Record

- 2026-10-31: USB steps 1-5 (partial) — USB.md.
- 2026-10-31: **touch (move 1).**
  - The kernel keeps a `touch` power switch (touch.rs `power`: the port's clocks, reset low, DVDD, 1 ms, AVDD, 15 ms, reset high; init slot 91). The handshake moved into `payloads/touch`: the ready event, the GPIO6 fix-up, the chip ID, scan mode.
  - The kernel no longer brings touch up in `start_home`.
  - The touch payload now links position-independent and relocates at entry, because its console lines format.
  - Verified from cold by hand (`diag "touch off"`, then `restart touch`) and through a slot b cold boot: controller ready after 12 ms, chip G#3640, scan mode set, serving.
  - Not yet verified: a finger on the glass (nobody here). The kernel's `diag touch …` tools stay until then.
- 2026-10-31: **sensors, phase 1 (move 4): the USF conversation.**
  - `payloads/sensors` speaks USF over the AoC's IPC rings: it opens ferros's channel, pushes the registry from the vault (`aoc.` scope), subscribes to gravity, proximity and spectral, tells the AoC the display's state, and follows the screen and the light-scope preference.
  - Grants: the IPC block mapped for EL0 (`aoc-ipc`, `cachemap.rs` `AOC_IPC`), the AoC mailboxes (`aoc-mbox`), the `aoc.` vault scope, the Sensors right, the `sensors` switch; init slots 92-96.
  - What it reads goes into one block in its own memory (`ferros_sys::sensor_block`), published with `SYS_SENSORS` 26. The kernel reads the block in place under its lock for `SYS_FACTS`, `SYS_LIGHT` and the power key's proximity rule (`sensorlink.rs`), and forgets it when the process ends — in `end_process`, in `kill`, and every 4 s if no sensors process runs.
  - The kernel keeps the AoC's start (rails, GSA, Trusty: phase 2). With the switch held it stops polling USF and skips its own USF steps; without it, or once the process is gone, it opens the channel again and subscribes.
  - `diag facts` shows the sensor words a process gets and which side answered.
  - Verified on the device:
    - by hand: start, the facts switch to the process (gravity 9.81 m/s² on z, flat on the table), kill, the kernel takes the conversation back within seconds;
    - a slot b cold boot: the kernel starts the AoC and hands USF over, and the process pushes the registry itself (46 sensors after 321 ms), subscribes and serves;
    - the light scope through the process: 96,162 samples in the ring.
  - A bug found and fixed on the way: `kill` did not run the end hook, so the kernel kept reading a killed process's memory.
  - Still in the kernel for now: its own USF code as the fallback, and the `diag usf …` tools (they talk on the kernel's session).
- 2026-11-01: **sensors, phase 2 (move 4, decision 2): the kernel keeps only the AoC's start.**
  - `usf.rs` is gone (1,109 lines), with the kernel's USF fallback, its take-back, the kernel telling the AoC the screen's state and the light scope, and the `diag usf …` and `diag sensors up` tools. `SYS_FACTS`, `SYS_LIGHT` and the power key's proximity rule read only the process's block; with none published they answer "unknown" and the wake fails open.
  - The `sensors` switch's on asks for the AoC's start (sensorlink.rs `START`); the service loop runs it a step a pass (`SensorBoot`: rails, the firmware from the vault, GSA's load, the Trusty start, online), no sooner than 4 s into the boot. Off undoes nothing. The rails moved to `aoc::rails`, the board's CDT/MLB reader to `sensorlink::read_board`.
  - The sensors process waits up to 16 s for the AoC, then ends; init's keeper (`KEPT`, now three: usb, pcie1, sensors) starts another, whose switch asks again.
  - Verified on the device: after a reload (the AoC already up: "online already", the process serving); `kill sensors`, back in about 3 s; a slot b cold boot, the AoC started from nothing on the switch (online 662 ms after GSA's start), 46 sensors, gravity, proximity and spectral through the process.
- 2026-11-01: **the boot modeset (move 6, decision 6).**
  - The display server takes ABL's 8 bpc pipe to the 10 bpc blend at its start (`payloads/display` modeset.rs: the kernel's quiet `dpu::modeset_with` path word for word, then the DQE's dither in shift mode), only while the pipe is still at 8 bpc. New grants: `decon0-sub`, `dqe0`, `dqe0-cgc`, `dpu-sysreg` (init slots 97-100, the server's 19-22). The kernel's `start_home` no longer runs it.
  - `dpu.rs`, `dsim.rs`, `hk3.rs` and `display::resolution` are gone (830 lines), with the `display` commands that used them (G#02 DCS script, G#03 DSIM dump, G#04 DCS read, G#06 refresh, G#08 raster, G#09 idle floor, G#0A modeset) and the bridge's `display dcs|dsim|read|mode|res|idle|modeset`; the kernel no longer depends on `ferros_dsi` or `ferros_dsc`.
  - Verified: slot b cold boots, 8 to 10 bpc in 63-64 ms with nothing stuck, the attest screen up, no banding (owner's eyes); a reload finds 10 bpc and skips it.
- 2026-11-01: **the `diag` tools (decision 4).**
  - Gone: the `touch` tools and `touch.rs`'s bring-up (923 lines to 63: the `touch` power switch alone), the AoC's bring-up tools with the `aoc.rs`, `trusty.rs` and `gsa.rs` code only they used (`diag aoc` keeps a status), `diag gsa|ffa|trusty`, `fp.rs` with the shim's `FERROS_FP_SCAN`, and `tpu.rs` with its Trusty channel.
  - A production build (FERROS_PRODUCTION=1) compiles every development command out (`#[cfg(not(ferros_production))]` on the DIAG subcommands and on each development door's body), where before it only refused them: the production kernel's code is 643 KB against the development build's 1.09 MB. What only the tools used is dead there, allowed by a production-only crate attribute.
  - Verified: a reload; a slot b cold boot (touch ready after 12 ms through its switch, chip G#3640, scan mode set; the display's modeset; sensors serving); the production configuration compiles.
- 2026-11-04: **the DMA fences are the kernel's alone.**
  - The modem driver turned HSI1's S2MPU protection off itself before the ROM boot. With ABL leaving it off anyway, the modem could DMA anywhere.
  - The kernel now fences HSI1 to the modem's carve-outs, denies BLK_MISC's masters, and fences the GPU to `gpu-mem`, all on a cold boot before init starts any driver.
  - `SYS_MMIO_WRITE` refuses every S2MPU, whatever the grant (`s2mpu::is_fence`). A driver may read its fence, never write it. This is the "Protection" line of "What stays", enforced.
- 2026-11-04: **the GPU driver starts as a process** (`payloads/gpu`, GPU.md). Nothing GPU-side was ever in the kernel but the power gating (`pd.rs`), which no longer gates the GPU.
