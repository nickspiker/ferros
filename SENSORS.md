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

It does not come online yet: the parameter block stays where it was written (the firmware writes its control block, magic G#A0C0_0A0C, over it), no mailbox interrupt, no AoC watchdog. Linux's kernel log for the same image: online 0.65 s after start with 94 services.

The oracle capture of Android's bring-up (hv traps on the power request, GSA's mailbox, the AoC GPIO and mailbox, the AoC SYSMMU, SSMT and S2MPU; local only, /mnt/Harbor/tmp/aoc-oracle-2026-10-23) shows what ferros does not do yet: Linux enables the AoC's SYSMMU (sysmmu@19090000, v9.0) before the load — the page table base into G#1909_8404 (G#8C880, i.e. a table at G#8C88_0000), G#20F84 into G#1909_8408, the default stream (G#10B at G#1909_2000) and 37 stream entries (G#1909_2010 + 16·n: config, G#301, the stream id, mask G#3F), then VM 0's control G#24 → G#25 (G#1909_8000) and G#1 at G#1909_8010. It wraps the load in GSA's wakelock (150/151) and drops the power request after the AoC is up. It never touches the AoC GPIO page or S2MPU during bring-up. The next step is to reproduce the SYSMMU enable; what the AoC's non-secure page table must map is the open question (the secure side, G#190A_0000, is GSA's and unreadable).

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
