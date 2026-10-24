# Sensors on husky

What the Pixel 8 Pro has, read from Android's sensor service on the phone (`adb shell dumpsys sensorservice`, 2026-10-23). Every hardware sensor carries the handle prefix G#0101: all of them are served by one sensor HAL, the AoC's (Google's always-on sensor hub). None of them appears in the application processor's device tree. Android reaches them only through the AoC's firmware and its mailbox, so for ferros they are one project: the AoC sensor hub.

| Sensor | Part | Vendor | What it gives |
|---|---|---|---|
| Accelerometer, gyroscope, IMU temperature, motion/stationary detect | ICM45631 | InvenSense | the gravity vector for gravity-up UI (the clock's zero and digits), rotation, steps |
| Ambient light, proximity | TMD3719 | AMS | the proximity-gated wake (SHELL.md), auto brightness |
| Non-contact infrared thermometer, normal and extended range | MLX90632 | Melexis | an object's emissive temperature (the "Thermometer" sensor by the cameras) |
| Rear light sensor (spectral) | VD6282 | STMicro | the light the cameras see (white balance) |
| Magnetometers ×2 | MMC56X3X | MEMSIC | the compass |
| Pressure, its temperature | ICP20100 | InvenSense | altitude, weather |
| Camera vsync ×4 | — | Google | frame timing for sensor fusion |

The rest of the list (gravity, rotation vector, step counter, lift to wake, double twist, …) is Google's fusion running on the AoC from these parts.

Outside the AoC, ferros already reads: the battery fuel gauge (MAX77759, I2C: charge, temperature, voltage, current), the RTC (s2mpg14 via ACPM), touch (FTM5), the keys. The SoC's own thermal zones (TMU) are behind ACPM, which ferros speaks: read since 2026-10-23 (below).

## The way in

An hv oracle capture first: Android under the hv, recording how it boots the AoC and asks it for streams. Then one of two routes:
- Run Google's AoC firmware and speak its sensor protocol.
- Keep the AoC off and drive the sensors' buses directly from ferros, if the AP can reach them.

The capture decides which.

## Starting the AoC: GSA loads, Trusty starts (2026-10-23)

On a cold ferros the AoC has never run: Linux's `aoc` driver starts it, every boot. Its sequence (Google's `kernel/google-modules/aoc` at /mnt/Harbor/ferros-ref/aoc, GSA in soc-gs `drivers/soc/google/gsa`):
1. Copy `/vendor/firmware/aoc.bin` (29 MB, Google-signed: a 4 KiB header GSA checks, then the body) into the `aoc_reserve` carve-out at G#9400_0000.
2. Ask GSA, Google's security processor, to authenticate and lock it: `GSA_MB_CMD_LOAD_AOC_FW_IMG` on GSA's non-secure mailbox (`gsa_load_aoc_fw_image`, gsa_core.c). ferros can do this itself.
3. Take the AoC out of reset: not a mailbox command. `gsa_send_aoc_cmd` (GSA_AOC_START and the reset/shutdown commands) goes over Trusty IPC to the secure-world service `com.android.trusty.gsa.hwmgr.aoc` (gsa_core.c, hwmgr-ipc.h). There is no other way in.
4. Wait for the AoC to come online, find its services in shared memory (`aoc_ipc`, in the husky kernel tree at /mnt/Harbor/husky-kernel/private/google-modules/aoc_ipc) and talk USF on the `com.google.usf` service (below).

So Google's sensor firmware means a Trusty client in ferros: FF-A messaging to the secure world, Trusty's virtio rings in shared memory, Trusty IPC on top, and giving Trusty CPU time when it asks (Linux's driver is ~7000 lines of C in the husky tree's `trusty/drivers/trusty`). The first step answers: `diag ffa` from cold ferros at EL2 gets FFA_VERSION 1.1, FFA_ID_GET success (ferros is partition 0) and FFA_FEATURES success for RXTX_MAP (2026-10-23).

The other route stays open and unmeasured: keep the AoC in reset and drive the sensors' buses from the AP, if the AoC block's own I2C/I3C/SPI controllers answer the AP through the `blk_aoc` window (G#1700_0000, 32 MiB) and their addresses and the sensors' wiring can be read out of aoc.bin. That avoids the TEE and Google's firmware, but every sensor driver (ICM45631, TMD3719, MLX90632, ...) becomes ferros's.

## The direct route, measured: closed (2026-10-23)

The AoC's own peripheral table (in aoc.bin's SRAM image) gives its controllers' bases in the AoC's address space; the AP sees them at that base + G#1700_0000. The registry scripts on the phone (`/vendor/etc/sensors/registry/husky_*.reg`) give the wiring:

| Bus | Sensor | Address | Interrupt (AoC GPIO pin) |
|---|---|---|---|
| i2c0 | TMD3719 light/proximity | G#39 | 45 |
| i2c0 | MMC5633 magnetometer 0 | G#30 | — |
| i2c0 | ICP20100 pressure | G#63 | 47 |
| i2c0 | VD6282 spectral | G#20 | 56–59 |
| i2c0 | MLX90632 infrared thermometer | G#3B | — |
| i3c0 | ICM45631 IMU | G#68 | 43 |
| i3c0 | MMC5633 magnetometer 1 | G#30 | — |

Controllers: I2C0–3 at AoC G#01A0_6000.., SPI0–4 G#01A0_A000.., five DesignWare I3C masters G#01A0_F000.., two Silvaco I3C G#01C0_A000/B000, a DesignWare APB GPIO at G#01C0_0000.

From the AP (`diag aoc`, aoc.rs), with the AoC block powered and acknowledged: every read inside the block aborts (SRAM, PCU, I2C, I3C, SPI, DMA) except the GPIO's first page. The GPIO reads live pin levels (the sensors' interrupt lines sit high, as idle open-drain lines do) and takes writes, but its configuration has no hardware-control mode and the pads do not follow it: the pin mux is elsewhere and walled off. EL3's privileged register read refuses the block (error −22). So the AP cannot reach the sensor buses by any path found; only the AoC can.

## Starting the AoC through GSA's mailbox (2026-10-23, in progress)

GSA's non-secure mailbox (gsa.rs) takes AoC commands on this firmware, no Trusty: command 91 with 0 answers the state. ferros loads the phone's own aoc.bin from the vault (`tools/husky-aoc-fw.sh`, 14 pieces of 2 MiB; a 4 MiB piece reset the phone, not yet measured why), the body at the carve-out's start and the 4 KiB signed header beside it, and GSA's load (90) authenticates it in 30 ms. The states over the mailbox: 0 inactive, 1 loaded, start (1) gives 4, releasing the A32 (5) gives 2, running; shutdown (4) returns to 1. `diag aoc boot`, then `diag gsa aoc 5`.

ferros now does all of this and the firmware still does not come online: the parameter block it wrote (magic G#0A0C_DA7A at G#95C0_0000) is never replaced by the firmware's control block (magic G#A0C0_0A0C), no AoC mailbox traffic, no watchdog. Linux's kernel log for the same image: online 0.65 s after start with 94 services.

**The SYSMMU is not the blocker (solved 2026-10-23).** The firmware carries its own IOMMU table in the signed header (aoc_firmware.c `_aoc_fw_iommu_entry`: image_config at header G#580, then iommu_offset/size, then 64-bit entries iova[19:0]<<12 / pa[43:20]<<12 / size[63:44]<<12). For husky's aoc.bin: G#9800_0000→carve-out (48 MiB), G#9B00_0000→G#9700_0000, G#9E00_0000→ACPM SRAM G#1510_0000, and the mailbox/GPIO/CMU windows. `diag aoc boot` now builds ferros's own first-level table (1 MiB v9 sections, read+write bits set, in ferros DMA memory — a table in the carve-out is locked by GSA on load) from exactly those entries and replays Linux's enable (sysmmu@19090000 VM0, oracle capture). Result: VM0 control G#25, no SYSMMU fault (status 0) through the whole start. The first try faulted (access fault G#4 at G#9800_1000 for want of the permission bits; then page fault G#2 at G#9E11_0024 for want of the ACPM window) — both gone once the table came from the firmware's entries.

**The blocker: the start handshake genuinely needs Trusty (2026-10-23, decisive).** In the oracle capture Linux sends GSA mailbox command G#5A (90, LOAD_AOC) exactly once and **never** G#5B (91, the AoC command) — it loads over the mailbox but starts the AoC over Trusty IPC (`gsa_send_aoc_cmd` → `com.android.trusty.gsa.hwmgr.aoc`, gsa_core.c). ferros's mailbox command 91 is *accepted* by GSA (state goes loaded → 4, release A32 → 2) but does not actually run the firmware; the state transitions are real but the AoC never executes. So the no-TEE route reaches load + authenticate + SYSMMU and stops: there is no mailbox path to start the AoC. Starting it means a Trusty client in ferros (FF-A + trusty-virtio + tipc), the thing the direct route was meant to avoid.

**The Trusty client (2026-10-24, built; blocked at tipc GO_ONLINE).** ferros now carries a Trusty client (`ffa.rs`, `trusty.rs`, `diag trusty aoc`), every layer verified on hardware except the last: FF-A up (version negotiated at 1.0 to match the 1.0 memory-transaction descriptor; RX/TX pair mapped, unmapped first so a warm reload re-maps), `FFA_MEM_SHARE` handles the secure world honours (the descriptor page shared this way is read AND written by Trusty — GET_DESCR fills our resource table), the Trusty API version selected (5), the scheduler-share region registered, the tipc virtio device found and parsed (id 13, two 32-entry vrings, align G#1000), its vrings + 32 rx buffers + a tx buffer shared and posted, DRIVER_OK written, VDEV_RESET then VIRTIO_START (returns 0).

The hv SMC oracle of Android's own bring-up (local, /mnt/Harbor/tmp/trusty-smc-2026-10-24) gave the one non-obvious fix: **the queue kick goes through the NOP queue** — `SMC_SC_NOP(0x32000019 NC_VDEV_KICK_VQ, vdev_notify, vring_notify)` — never the direct `SMC_SC_VDEV_KICK_VQ`, which returns -7 at API 5. ferros now kicks that way. Its shares (0x50, one range, WB inner-shareable, RW, to 0x8000) and the ordering match Android's exactly.

It still does not come online: GO_ONLINE never lands in the rx vring, so the GSA START message can't be sent. The capture can't show why — GO_ONLINE is a memory write by Trusty, not an SMC. Everything the available source documents now matches; the remaining difference is inside Trusty's secure-side virtio/tipc device-start, which is in no local tree. **Next step: fetch AOSP Trusty (external/trusty — lib/virtio, the tipc device, the VIRTIO_START / vq-init path) for this phone's TEE build and read what makes a started vdev deliver GO_ONLINE.** Two smaller leads from the capture: Android also does a `FFA_MEM_LEND` and a 9-range `FFA_MEM_SHARE` before GET_DESCR (other subsystems, probably not the vrings), and Android's kick-NOP returns -14 (call again) which a client must repeat. Testing note: warm reloads leak shares into the secure world's table (MEM_SHARE then returns -2), so each Trusty test needs a cold boot (flash the gate image and `fastboot reboot`; `husky-cold.sh` hangs in its fastboot-entry retries when the phone is already in fastboot).

**SOLVED 2026-10-24 — the AoC runs under ferros.** The last blocker was in AOSP Trusty's secure side (trusty/lk/trusty, cloned at /mnt/Harbor/ferros-ref/trusty-lk): `lib/trusty/tipc_virtio_dev.c` `validate_descr` requires the vdev status to be exactly ACKNOWLEDGE|DRIVER|DRIVER_OK (0x07) — FEATURES_OK, which only modern devices set, fails it — and `trusty_virtio.c` `virtio_start` discards the probe's error and returns 0, so the queues silently never existed. With 0x07 the tipc device probes, sends GO_ONLINE, and the rest follows. The working sequence, all ferros (no Linux, no Trusty client of Google's):
1. `diag aoc boot`: the vault's aoc.bin into the carve-out, the AoC SYSMMU from the firmware's own IOMMU table, GSA mailbox load (cmd 90), the parameter block. GSA state 1 (loaded). No mailbox start — it moves GSA into a state the real start refuses.
2. `diag trusty aoc`: FF-A (version 1.0), API 5, scheduler share, GET_DESCR, VDEV_RESET, vrings + buffers shared, status 0x07, VIRTIO_START, kick via the NOP queue, GO_ONLINE, CONN_REQ `com.android.trusty.gsa.hwmgr.aoc` → CONN_RSP (remote 1000), HWMGR_CMD_STATE_CMD + GSA_AOC_START → reply cmd G#80000001 err 0 state 2.
3. The firmware replaces the parameter block with its control block at carve-out + image size: magic G#A0C00A0C, hw G#41A53, 94 services (Android: "HW Revision 0x41a53", "online with 94 services"). Service 0 `control`, 1 `com.google.usf`, 2 `com.google.usf.non_wake_up`, 3-4 CHRE, 5 `usf_sh_mem_doorbell`, then the audio services.

Next: the AoC service rings (aoc_ipc: each service header is a 32-byte name, flags and two ring descriptors {offset, size, slots, tx, rx, wp, rp} in the IPC area; the AP is signalled through the AoC mailboxes at G#1520_0000 / G#1522_0000 / G#1524_0000), then USF over `com.google.usf` (the captured FlatBuffers protocol above).

**Both no-TEE routes are now exhausted.** The direct buses are firewalled from the AP (above); the firmware route loads but cannot start without Trusty. Bringing the husky sensors up therefore needs one of: (a) a Trusty client, then Google's firmware over USF; (b) opening the AoC's bus controllers to the AP, which every probe so far says is walled off (would need a way through the AoC block's own access control, not found). The decision is the owner's.

Capture logistics learned: root on this phone is Magisk in slot b's init_boot, so making slot a active (`fastboot --set-active=a`) boots without root; after an oracle flash, put the image on both boot slots and leave b active. DRAM does not survive the trip through fastboot, so a capture is read from Android's pstore after a forced panic.

## Capturing USF (2026-10-23)

Android's sensors HAL (`android.hardware.sensors-service.multihal`, libraries `sensors.usf.so` + `libusf.so`) talks to the AoC through one character device, `/dev/acd-com.google.usf` (plus `.non_wake_up`), served by `aoc_channel_dev.ko` (Google's `kernel/google-modules/aoc`, cloned at /mnt/Harbor/ferros-ref/aoc). The messages are FlatBuffers (libusf carries FlatBuffers 1.12; its message types include sensor list, sensor info, get server, get time, self test, debug buffer and an I2C transfer). The sensor rails are the AP's to switch: the AoC platform device holds the sub-PMIC's LDO5 (3.3 V) and LDO7 (1.8 V), which ferros reaches over ACPM.

The capture needs no tools on the phone, only the handoff kernel's kprobes (root): one probe on `aoc_service_write` (every message the AP sends to any AoC service: x1 is the kernel buffer, x2 its length) and one inside the channel driver's demux thread right after its `aoc_service_read` (`aocc_demux_kthread+0xfc` in this build: x0 the length, x22 the buffer). Each takes 1 KiB of the message as four x32[64] arrays:

```
p:usf_tx aoc_service_write len=%x2 a=+0(%x1):x32[64] b=+256(%x1):x32[64] c=+512(%x1):x32[64] d=+768(%x1):x32[64]
p:usf_rx aocc_demux_kthread+0xfc len=%x0 a=+0(%x22):x32[64] b=+256(%x22):x32[64] c=+512(%x22):x32[64] d=+768(%x22):x32[64]
```

The offset is from the module's disassembly (`llvm-objdump -d --disassemble-symbols=aocc_demux_kthread aoc_channel_dev.ko`): re-derive it after any rebuild of the handoff kernel's modules. `stop`/`start vendor.sensors-hal-multihal` replays the HAL's whole start-up; Android's sensor service reconnects by itself (43 sensors running again after it). `tools/usf-decode.py` reads the trace: each message is a u32 channel and a FlatBuffer, decoded without a schema.

What the first capture shows: a USF message is a table {kind (1 request, 2 response), payload: [ubyte]} whose payload is itself a FlatBuffer {type, transaction, server, body: [ubyte]}. The HAL first opens servers by 16-byte UUID (type 3; the AoC answers a small handle), then reads a registry tree (type 21: nodes `persist`, `fac_cal`, `power`, `tbn`, `vsc`, `dev`, `log`, and the board's loaded register script), then subscribes. Streams arrive on channels with bit 31 set, as floats. Captures stay local (/mnt/Harbor/tmp/usf-captures-*): they are this phone's calibration and sensor data.

## SoC temperature (TMU over ACPM) — working 2026-10-23

The TMU and its trim fuses belong to the ACPM firmware; the AP asks on IPC channel 9 (zuma's `acpm_tmu` node; the driver is `exynos_acpm_tmu.c`). TMU_IPC_READ_TEMP (type 2) with the zone id in byte 2 of word 1 answers in whole °C: word 1's top byte is the zone's hottest probe, word 2 byte 0 its status, and the next seven bytes each probe (zero past the last). No TMU_IPC_INIT is needed on a cold boot (it answers capabilities G#3: APM interrupt + divider). One zone takes 28 us.

| id | zone | probes |
|---|---|---|
| 0 | BIG (the X3) | 2 |
| 1 | MID | 4 |
| 2 | LITTLE | 2 |
| 3 | G3D (GPU) | 3 |
| 4 | ISP | 1 |
| 5 | TPU | 3 |
| 6 | AUR | 3 |

At idle with the X3 spinning at 2.91 GHz every zone reads 48-52 °C.

ferros (`acpm::tmu_read`, the service loop's `thermal_poll`) reads every zone once a second on the kernel's core and keeps them for `diag thermal`, DIAG `SOC_C` and fact 10 (the device circle's "chip" row). The throttle: the hottest CPU zone at 95 °C (Linux's control temperature for the big cluster; its "hot" trip is 115) takes the X3 to 1.4 GHz, and below 85 gives its rate back; fact 11 and DIAG `THROTTLED_MHZ` say when it holds. `diag thermal at <C>` moves the trip to test it on a cool phone (tested both ways at 50 and back at 95). Linux's own governor (PI control, MPMM, the GPU/TPU zones) is not reproduced: the mid and little clusters already run at 1.065 and 1.425 GHz, the GPU at 150 MHz.
