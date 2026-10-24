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
