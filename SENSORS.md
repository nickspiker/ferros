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

**USF transport working end to end (2026-10-24).** ferros drives the AoC's service rings itself: `diag aoc svc <name>` reads the control block and a service's geometry (com.google.usf is a queue on mailbox 1, 256 slots × 1024 bytes each way, DOWN at IPC+G#5000, UP at IPC+G#45000, IPC = carve-out + image size = G#95C0_0000). `diag aoc usf` then opens a logical channel (the aoc_channel multiplexing: a `[channel 0][OPEN_CHANNEL 0][my channel]` control message, then `[my channel][payload]` per message), signals the AoC (write `1<<(mbox%16)` to the mailbox block's INTGR0, a32 G#1520_0000), and replays the sensor HAL's first USF request (a "get server by UUID" FlatBuffer, captured 2026-10-23). The AoC answers: 88 bytes on channel 1, byte-for-byte Android's response. The carve-out is Device memory in ferros's map, so the rings need no cache maintenance. Primitives: `aoc::ipc_write`/`ipc_read`/`mbox_signal`.

So every layer from bare metal to the sensor service is proven: GSA-loaded firmware, Trusty-started, its IPC rings driven directly, a USF request answered — no Linux, no Google Trusty client. What remains is the USF FlatBuffer protocol itself (enumerate sensors, open one, subscribe, decode the sample stream), which is the HAL's logic on top of this working transport; the captured exchanges in /mnt/Harbor/tmp/usf-captures-2026-10-23 are the reference, decoded by tools/usf-decode.py.

Earlier next-step note (now done): the AoC service rings (aoc_ipc: each service header is a 32-byte name, flags and two ring descriptors {offset, size, slots, tx, rx, wp, rp} in the IPC area; the AP is signalled through the AoC mailboxes at G#1520_0000 / G#1522_0000 / G#1524_0000), then USF over `com.google.usf` (the captured FlatBuffers protocol above).

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

## The sensors from cold (2026-10-24): rails, firmware, registry, stream

ferros brings the sensors up by itself, 4 s into the service loop, a step a pass so the X3 keeps serving USB and drawing (service.rs `SensorBoot`, usf.rs). The log goes to the flash log as `sensors:` lines.

1. **Rails.** The sub PMIC's LDO7S (1.8 V, `L7S_SENSORS_FPS`, control G#35) and LDO5S (3.3 V, `L5S_PROX`, G#33) are the AoC's `sensor_power_list`. Their voltages are already programmed but ABL leaves both off (bit 7 clear). Linux's AoC driver turns them on at probe, before the firmware loads (aoc.c `reset_sensor_power`); ferros does the same over ACPM.
2. **Firmware.** `aoc.fw.*` out of the vault a 2 MiB piece per pass (~50 ms each), then the SYSMMU, GSA's load and the Trusty start (above). The firmware is online 675 ms after the start, the same as under Android (6.040 s → 6.713 s in its dmesg): that time is the AoC booting itself.
3. **Registry.** A fresh AoC has a registry with only `log` in it and lists no sensors: the board's wiring is pushed from the AP. Android's sensor HAL does it on connect (sensors.usf.so `SensorHal::LoadRegistry`, decoded from the factory image's vendor.img): each `.reg` script whole, in readdir order, from /vendor/etc/sensors/registry, its append/, then the phone's own factory calibration in /mnt/vendor/persist/sensors/registry, as a LoadScript request (type 24 to the registry server, body {f0 the text and its NUL, f1 the board's CDT, f2 its MLB number, each left out when zero}), stopping at the first error; then a Set (type 22) of "/" with `loaded=1`, on which the AoC probes its buses. The AoC parses the scripts and filters their `?+value:mask` sections on the CDT sent with them, so the CDT matters: with none, husky_common and husky_dvt are skipped and no sensor appears. ferros reads it where libusf does, from ABL's device tree: /chosen/plat {product, stage, major, minor, variant} → product<<16 | stage<<12 | major<<8 | minor<<4 | variant = G#26100 on this phone. Messages over 1012 bytes go as kind-3 fragments {f0 1, f1 the whole length, f2 the offset, f3 the slice}, slices of the whole serialized message. The other registry types: 21 Get, 23 Remove, 27 ProdScript (the AoC dumps a subtree as script text), 40 ProdScriptNode. Not sent yet: the panel info Set (`/dev/display_info/0`: panel name, serial, `panel_is_original`), which the light sensor uses to decide on its factory calibration. The scripts go in the vault with tools/husky-aoc-reg.sh as `aoc.reg.0…14`; they are Google's files and this phone's calibration, so they live on Harbor (/mnt/Harbor/firmware/husky-aoc), never in the repo.
4. **Stream.** 26 sensors are listed 332 ms after `loaded=1`, all 48 shortly after. The fused Gravity sensor (handle 54, "Gravity Sensor") is subscribed every 64 ms with no batching (type 8, the captured template with period and latency patched); its samples come up `com.google.usf.non_wake_up` as kind-9 events, {u32 1, u32 subscriber handle, u32 subscription, u16 type, u8 count, u8 size} then {u64 timestamp, 3 × f32 m/s²} per sample. The last vector is SYS_FACTS 12–13 (`ferros_sys::Facts::gravity`); lying flat the phone reads (−0.08, 0.46, 9.80). The display turns the clock upright by it (12:00 and the digits up against gravity), holding the last turn while the phone lies within ~15° of flat.

5. **The front light and proximity sensor (TMD3719) needs the display.** It sits under the OLED and times its IR and light bursts to the panel: it needs (a) the panel's TE2 output, which ABL leaves off — the display programs it with the brightness and after every display on (`F0 5A 5A`, `B0 00 42 F2`, `F2 0D`, `B0 00 01 B9`, `B9 04` changeable, `B0 00 1E B9`, `B9 00 10 00 30` rising/falling, `F0 A5 A5`; panel-google-hk3.c `hk3_update_te2_internal`), and (b) the display's state from the AP: a type-1001 message to the DisplayInfo server (UUID a8 8e c6 24 10 40 2a ed ca b9 a5 aa d6 a2 ee d2; body {f0 DBV, f1 status 0 off / 1 on / 2 AOD / 3 HBM, f2 refresh Hz, f6 the panel's operating Hz, f8 PWM mode −1, f9 version 2, f10 luma [f32 ×3], f11 leakage [f32]}, zero scalars left out; decoded from sensors.usf.so `UsfMsgDisplayInfo` and the capture). Without them the AoC logs "Sync delay shouldn't be less than 0 after adjustment" and a watchdog with status 0, and nothing is measured. Android also holds the panel at 30 Hz or more while proximity is active (ProxMonitorService). With both, from a cold boot: "Far00" and the first lux. A proximity sample (type 3) is 15 floats, not a bool: Android's distance (0 near, 5 far), the processed pdata0/pdata1, this phone's factory baselines (191.94, 70.45), the raw photodiode counts, the 120 Hz burst delay (105) and some not yet named. Still to send: the DBV the display actually uses (a fixed 1353 for now), status changes with the power key, and the panel identity Set (`/dev/display_info/0`: panel name, serial, is_original) the light sensor checks its calibration against.

6. **Every sensor, 2026-10-24 night.** With the registry, the display's state and TE2 in place, 39 of the 41 sensors the AoC lists subscribe and stream (`diag usf sensors`, `diag usf subh <handle> <ms> <mode>`, `diag usf ev`); the two refusals (status 2) are the proximity-gated touch gestures, whose touch controller ferros keeps from the AoC. Each sample event carries its own count and size, so the formats below are read off the wire; values are the phone lying face up on the desk.

| USF type | sensor | sample (floats) | reading |
|---|---|---|---|
| 1 | ICM45631 accelerometer | x y z m/s² | (−0.03, 0.35, 9.78) |
| 2 | ICM45631 gyroscope | x y z rad/s | ~5e-4 (still) |
| 8 | ICM45631 temperature | °C | 34.9 |
| 22 | Gravity (fused) | x y z m/s² | the clock's |
| 23 | linear acceleration | x y z m/s² | ~0.03 |
| 20 / 21 / 27 | game / geomagnetic / rotation vector | quaternion (+ accuracy) | unit quaternions |
| 24 | orientation | azimuth pitch roll ° | 167.8, −2.2, −0.4 |
| 6 | magnetometers (MMC5633 ×2, fused) | x y z µT | (−7.6, −40.3, −87.2): 96 µT, uncalibrated or metal near the desk |
| 4 / 5 | ICP20100 pressure / its temperature | hPa / °C | 1003.45 / 34.0 |
| 56 / 67 | MLX90632 IR thermometer / extended | object °C, sensor °C, raw | 32.8, 31.8, … / 32.8, 32.0 |
| 36 | TMD3719 temperature | °C | 32.5 |
| 3 | TMD3719 proximity | 15 floats (above) | far, raw 231/111 |
| 9 / 54 | TMD3719 light / light raw | 15 floats | lux fields 0 tonight with 8090910, 200000, 1653120 after: to look at |
| 74 | ALSP fusion info | 8 floats | far, pdata0 23.96, baseline 191.94, DBV 1353 |
| 68 / 69 | AAD proximity / voice-call proximity | fused | far |
| 12 / 13 / 46 | VD6282 spectral / flicker / rear light | 16 floats / 1 / lux | channels; 12812; 0.027 |
| 48 / 14 | auto brightness / binned brightness | 16 floats / 3 | — |
| 30, 33, 70, … | step counter, stationary detect, motion state, tilt, lift, twist, pickup | events | 0 steps, stationary |

The AoC's own log is readable: `diag aoc log` drains the firmware's `logging` ring (256 × 128 bytes, full from boot until read), and it names what each driver is waiting for.

`diag usf` shows the rails, the stream's counters and the last vector; `diag usf reg <path>` reads a registry node; `diag usf raw <handle> <type>` sends a bare request and dumps the replies; `diag aoc services` lists the AoC's 94 services.

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

## Fingerprint: never (owner decision, 2026-10-24)

ferros supports biometrics only if the biometric material moves with the identity. Husky's fingerprint reader looks built to prevent exactly that, so it is marked **never, unless Google hands over the keys, or the raw sensor turns out reachable from ferros**.

What the device tree shows (measured): the reader is a Goodix optical under-display sensor (`goodix,fingerprint`, zuma-board-udfps.dtsi), lit at panel coordinates 672, 2261 (zuma-husky-touch.dtsi `st,udfps-coords`). Linux is given only its interrupt pin (gpa7-3), its reset pin (gpp1-0) and a 3.3 V rail (s_ldo15, `L15S_UDFPS_AVDD`, always on). It is given no data bus: every other SPI device on husky (touch spi_20, the speaker amps spi_19, UWB spi_18) sits under its own controller, while the fingerprint node sits under `&odm` with its SPI controller described nowhere.

What that implies (inferred, not verified on the phone): the sensor's SPI bus belongs to the secure world, capture, templates and matching run in a Trusty app, Android receives only a signed yes/no, and the templates are sealed to keys in Google's TEE, so they can never leave the device.

The one way back without Google: if the sensor's SPI controller proves reachable from the normal world (the fault-safe register probe used on the AoC block would settle it, once the controller is identified), ferros could capture raw images itself and keep its own templates in the vault, moving with the identity. Using Goodix's Trusty app through ferros's tipc client is possible too, but it only gives a device-bound yes/no, which the identity rule excludes.
