# Boards

ferros runs on two Tensor G3 (zuma) phones. The kernel tells them apart at entry from the root `model` of the device tree ABL hands over (`ferros_kernel/src/board.rs`), before anything board-specific runs.

| | husky (Pixel 8 Pro) | akita (Pixel 8a) |
|---|---|---|
| Model in the device tree | ZUMA HUSKY … | ZUMA AKITA MP board based on ZUMA (board id 40406, hardware MP1.0) |
| DRAM | 12 GB, top at G#B_0000_0000 | 8 GB, top at G#A_0000_0000 |
| The hv's 64 MiB (report page, gate mark, the jump's mailbox and entropy pool at fixed offsets) | the top of DRAM, G#A_FC00_0000 | G#9_0000_0000, mid high bank: the top of DRAM hung the core reading it at the handoff on the August chain |
| Keys | vol down gpa4-1, vol up gpa6-2, power gpa4-0 | the same |
| UFS VCC enable | gpp0-1 (zuma-ufs.dtsi) | the same (not overridden by its overlay) |
| eUSB repeater | hsi2c_11, address G#3E | the same; its own tune (below) |
| Type-C controller | MAX77759 at G#25 on hsi2c_11 | the same |
| Fuel gauge | MAX77759 at G#36 on hsi2c_11 | the same |
| Low 2 GB carve-outs | zuma's | the same (they come from the SoC's tree) |
| Panel, touch, cutout | HK3, FTM5, 1344 × 2992 | AK3B (docs/AK3B.md), 1080 × 2400; touch answers (below) |
| Touch controller | ST FTM5 on spi20, reset gpp21-2 | Goodix "nottingham" (Berlin family, `goodix_brl_spi.c`) on the same spi20 (10 MHz) and the same reset gpp21-2, interrupt gpn0-0 (falling edge), and the same rails as husky's touch (vdd LDO25M at 1.8 V, avdd LDO4M at 3.0 V, main PMIC), so its power-up is husky's and only the protocol is new; coordinates 0-1079 × 0-2399; firmware `goodix_firmware_ak3b.bin` from Android's vendor partition |
| Side keys, panel rows (board::keys) | power 933..=1124, volume rocker 1279..=1660 (measured 2026-11-06) | power 629..=797, volume rocker 932..=1271 (measured 2026-11-06) |

akita's repeater tune, from its MP board overlay (`repeater_tune1..5`): G#50 = G#0A, G#70 = G#5F, G#71 = G#2E, G#73 = G#80, G#79 = G#40 (ferros_hal `AKITA_REPEATER_TUNE`).


## Side keys (calibration)

Where each side key sits, in panel rows, top and bottom edge inclusive (`board::keys`). The gate draws its marks from them: the power triangle centred on the power key and as tall as it, the volume-down chevron with its tip on the rocker's bottom edge and a quarter of the rocker tall (ready.rs `marks`). Anything else that points at a key (a hint, a press echo) takes them from there too.

Measured with `ferros-bridge display ruler`: every row shows its own number in binary across the width (most significant bit left, white a 1), and the person reads the row level with each key's edge off the glass. The same screen then draws the stored edges as 1 px hairlines (power green, rocker red) to check them. akita, by the owner, 2026-11-06: power 629..=797 (001001110101..=001100011101), rocker 932..=1271 (001110100100..=010011110111); checked with the hairlines. husky, by the owner, 2026-11-06: power 933..=1124 (001110100101..=010001100100), rocker 1279..=1660 (010011111111..=011001111100); its marks placed by eye had sat at 1050 and 1700, the chevron 40 rows below the rocker. On husky the hv draws the gate (ferros_hv gate.rs), with the same spans and the same derivation.

## akita's facts, and where they came from

Google's sources for akita's own parts, in the reference tree: the panel driver (`/mnt/Harbor/ferros-ref/kdev-akita`, kernel/devices/google/akita, branch android-gs-akita-android16) and the touch driver (`/mnt/Harbor/ferros-ref/touch-goodix`, kernel/google-modules/touch/goodix_touch, the same branch).

From its factory image (CP2A.260805.005, `/mnt/Harbor/tmp/akita-factory/`, SHA-256 checked against Google's): `vendor_kernel_boot.img` carries four zuma base trees, `dtbo.img` nineteen akita board overlays (DEV, Proto, EVT, DVT, PVT, MP); this phone is the MP one. Dumped to text in `/mnt/Harbor/tmp/akita-factory/dt/` by a scratch walker (no `dtc` on the host).

The phone itself, 2026-11-05: bootloader akita-17.0-15199481, baseband g5300o-260317-260505-B-15346003, unlocked that day (`fastboot flashing unlock`, which wipes it), pKVM disabled.

## How akita boots for now

Storage-off until 2026-11-06, when its per-unit partitions were backed up (below); since then the UFS link comes up for the flash log in `klog`, and the rest stays off:
- the UFS link, the flash log in `klog` and (since 2026-11-06) the vault in its ferros partition, but no write protect and no slot mark; the gate's Android choice (volume down) makes slot b active and resets into its Android (since 2026-11-06, devinfo checked to have husky's layout), and fastboot is the way back;
- no ira (its sources include a devinfo read);
- no init: every driver process assumes husky's panel, touch and rails. The kernel starts the usb process itself, from a native capsule it carries (`target/akita-usb.capsule.vsf`, made by `tools/akita-boot.sh`).

`tools/akita-boot.sh` builds that capsule and the bare kernel (no hv: it lives in husky's top RAM) and boots it, naming the 8a by serial on every fastboot call. `fastboot boot` (from RAM) is not taken by akita's ABL, as husky's wasn't: it says "Booting" and comes back to fastboot. `--flash-a` flashes boot_a only and switches to slot a; slot b keeps Android, and since this ferros marks no slot, ABL falls back to it once slot a's retries run out.

## Bring-up, 2026-11-05 night to 2026-11-06 morning: ferros on akita's USB

ferros enumerates on akita (1209:4665) and answers the bridge: `diag`, `diag console`, `diag "part find <name>"`. What it took, in the order it was found:

1. **The per-unit partitions backed up first**, through Magisk root on slot b's Android (v31.1 on init_boot_b; adb's Shell had to be granted by hand): persist, efs, efs_backup, modem_userdata, devinfo, pinfo, blenv, mfg_data, fips, trusty_persist, misc, ufs_internal, klog and the whole boot LU sdd, into `/mnt/Harbor/tmp/akita-backup-2026-11-06/` with device- and host-side SHA-256 lists that match. Local only.
2. **The X3 hop seemed to hang on akita** (the kernel never came back from `x3::hand_over`, read from probe 5), so akita stayed on CPU0 for the day. It was a misreading: a successful hop never reaches probe 5 (CPU0 parks), and the other faults below could stop the boot. Its X3 probe looked dead too, only because the probe's completion word never matched its constant (x3.rs, since 2026-10-22, on husky as well); with marks and a PSCI AFFINITY_INFO timeline the X3 was seen to run the whole probe and power off. Since the afternoon of 2026-11-06 akita's kernel runs on the X3 like husky's (~2.9 GHz, double CPU0's), checked over five boots with the vault, the store and `reboot fastboot`.
3. **"The kernel's core" follows the hop** (`process::kernel_mask`): it was the X3 always, so with the kernel on CPU0 a thread pinned to big waited for a core that never started, and every `SYS_POWER` was refused. The same flaw broke husky's `FERROS_X3=0`.
4. **No ira, no ihi**: akita has no vault yet, so no ira and no oracle secret, and `spawn_capsule` refused the usb capsule. `spawn_capsule_no_ihi` checks the signature and runs native code without an ihi (masked code and production builds still refused).
5. **A build that did not build**: `x3::local`'s `.rept 64` asm, inlined into `service::run`, threw off LLVM's branch relaxation, and one `tbnz` landed out of its ±32 KiB range ("fixup value out of range") in some builds. `x3::local` is never inlined now.
6. **Storage on**: the cold UFS link comes up on akita (45 ms) and its GPT reads. The flash log goes to akita's `klog` partition (LU 0, LBA 32774..=36869, found through the GPT, never husky's fixed place), and the processes' console lines follow it there. Read it back from Android with `dd if=/dev/block/by-name/klog`. Vault, write protect and slot mark stay off on akita.

What does not work as a channel on akita:
- `fastboot boot`: ABL says "Booting" and stays in fastboot.
- The RAM log (pstore): akita's ABL clears it at every reset, even an Android-to-Android reboot.
- The panel, until it was fixed the same morning: ABL leaves akita's splash framebuffer at G#FAC0_0000 (1080 × 2400, IOVA = physical), outside the regions the display engine's fence lets it read (s2mpu::fence_dpu: display-fb and display-phases, in the GiB below). Fenced at the service's start, every scan-out read was refused: DECON went on at 60 Hz over a framebuffer the panel never saw, black a few frames into the boot. akita now draws into ferros's own framebuffer (display-fb, G#8400_0000) from the first adoption (`use_own_fb` at entry), and the panel stays lit.

The probes (`FERROS_PROBE=<n>` builds, board.rs) were the channel until USB: at checkpoint n, `probe` goes to the bootloader (fastboot reappearing is the one bit), `probe_log` resets plainly (ABL counts slot a down to Android), `probe_starve` stops the watchdog pets. Checkpoints: 1 board, 2 safety armed, 3 gate, 4 ring, 41-45 board read to the X3 hop, 5 service, 6 home, 7 / 71 / 72 the usb spawn, 81-85 the USB power-up, 86 a process ending, 9 starve after the spawn. A probe taken after the usb process existed reset without stopping in fastboot, and so did `bridge reboot fastboot`: the memory scrub before every reset (platform::scrub_memory) took a permission fault on the first arc's grants page (read-only at every level, the kernel's too; FAR G#8300_0000, read back as PREV_EXC and PREV_FAULT_FAR after the next boot), and the fault's plain reset, which clears the PMU reboot mode, replaced the ordered one. The scrub now returns both pools to kernel-writable pages first (cachemap::pools_to_kernel). Since 2026-11-06, `bridge reboot fastboot` lands akita in fastboot in 37 s, so `tools/akita-boot.sh --flash-a` and the bridge make a loop that needs no hands. The same fault was waiting on husky whenever a process was alive.

## akita's ferros partition (2026-11-06)

Carved by `tools/pixel-carve.sh akita 32` (the `ferros-flash` library and CLI, tools/flash): userdata keeps its start and shrinks to 32 GiB, `ferros` (type BLAKE3("ferros"), A0B51225-61C5-0F5A-FFE7-1B644F9CA954) takes the rest from the next 1 MiB boundary.

| | LU 0, 4 KiB LBAs | size |
|---|---|---|
| userdata | A#2449894..=A#10838501 | 32 GiB |
| ferros | A#10838528..=A#31196154 | 77.7 GiB |

akita's table had 34 entries, all used, and reserves four blocks for the array at each end (room for 128), so the count grew to 64 in place and nothing moved: the write was 9 bytes in each header and 48 in each array (the userdata end and the new entry). The backup header's block carries a stale copy of entries after its 92 bytes; the edit keeps it. The script saved both ends first, with hashes checked against the device (`/mnt/Harbor/tmp/akita-gpt-2026-11-06-100644/`, and the first read in `akita-gpt-2026-11-06/`), wrote the primary first and the backup last, read both back after flushing the device's cache, then `fastboot -w`. Checked by the bootloader (`partition-size:ferros` G#13_6A1F_B000) and by ferros (`diag "part find ferros"`).

The vault is on it since the same day. The kernel finds the partition by its GPT name at boot (ufs_device::locate; on husky only where it always was, else no vault), and the flash log (+1 GiB) and the vault's two mirrors (+2 GiB) take their places from it, refused if the partition cannot hold them. akita's ira has its own context, `ferros.ira.akita.v0` (owner: its own, husky's untouched), after its seven sources (chip ID, the AP tune, ASV and HPM tables, the speed bin, devinfo's records, the UFS serial) read alike in both reads of each of eight full boots (`diag "ira sources"`: agreement, health and a short hash per source, never the values). First boot: the vault created in 18 s; every boot after, opened and verified in 15 ms, a stored value read back across a reset. Write protect stays off on akita: it would hold the bootloader LUs read-only across the warm reset into fastboot, and flashing is how akita is worked on. Slot marking is on only when ferros came through the hv's gate (below), which shares Android's slot; a bare ferros on slot a marks nothing, so the fallback to slot b's Android stays.

## The hv on akita (2026-11-06 night)

The same layout as husky: one boot image on Android's slot (b) carrying the hv, akita's own Android kernel and ferros; the hv's gate chooses at boot (`tools/akita-hv.sh`, built with `FERROS_HV_BOARD=akita`). Checked the same night without hands: ferros through the gate (diag's first line "through the hv's gate", the timer's answer, the vault verified, every core up), and Android under the hv by `diag "android here"` (boot completed, `hv@900000000` reserved in its memory map, its own KVM refused EL2). Still to check, with hands: volume down at the gate, and the chords under Android (power + volume up jumps to ferros, both volumes kill).

What kept it from booting all evening was AVB, not the hv. akita's boot partitions end in Google's AVB footer, which puts the image's vbmeta at G#FD_C000 on slot b. The gate image is 20 MB: flashed bare, it buries that vbmeta under its own bytes, the footer still points at it, and ABL logs `boot_b: Error verifying vbmeta image: invalid vbmeta header` and drops to fastboot ("fastboot enter reason: boot failure") without running anything. Every smaller image flashed after it fails the same way, since the footer and the buried offset both stay, until Google's whole image is flashed back. That looked like "after any crash, ferros dies a second in until Android boots once" and cost the evening's probes, none of which ran.

So every image for an akita boot partition is flashed whole, with its own footer: `ferros-mkimg avb-footer --key <avb.vsf> --google-boot <Google's boot.img> --image <boot.img> -o <partition.img>` writes the image, a vbmeta at the next 4 KiB holding its hash (Google's boot properties and rollback index kept), signed with the owner's key, zeros, and the footer in the last 64 bytes, and checks all of it as ABL will before writing. akita does not chain `boot` to that key, and an unlocked ABL boots it anyway (orange state), loading exactly the image's length. Before blaming any boot on akita, read ABL's log once (`fastboot oem dmesg`, one call per bootloader session): "invalid vbmeta header" is this; "BL1 requested" is a fastboot stamped on purpose (a probe, ferros's own exit); the DSS block marked `[PRE]` is the previous session's crash, not this one's.

The slot: ferros through the gate marks slot b successful in devinfo (`slot::mark_booted_slot`, as on husky), since `fastboot --set-active` leaves a slot three tries and every unmarked ferros boot spends one, until ABL would retire Android's slot with it (it was down to two). Checked: b's word G#0402 → G#0602, then ALREADY on the next boot.

## akita's touch (2026-11-06 night)

A Goodix GT9895 ("nottingham", the Berlin family), driven by the same touch process as husky's FTM5 (`payloads/touch/src/goodix.rs`), which picks it by the board fact (SYS_FACTS 84: 1 husky, 2 akita). The kernel's `touch` switch powers it as husky's with akita's AVDD at 3.0 V and Goodix's 3 ms between the rails. The protocol is Google's driver's: reads `F1` + a 32-bit address + three dummy bytes, writes `F0` + address + data, each inside the 64-byte FIFO; the device confirm (G#AA × 8 written and read back at G#1_0000); the firmware's version (G#1_0014) and its own description (G#1_0070), parsed for the touch-data address; then, on each falling edge of INT (gpn0-0), one frame holding every contact on the glass, read, turned into enter / move / leave against the last frame, and handed back with a zero.

Checked under the hv, without a finger: device confirm answered; firmware 9895 G#1F010011; 16 drive × 34 sense lines (the tree's `goog,channel-num`); touch data at G#10308, 14-byte points; `touchtest` read the endpoint (panel 1079 × 2399, no contact events with nothing on the glass). Still to do: contacts under a finger (`ferros-bridge spawn touch.bin 00 --name touch --serve touch --hw usi2-spi --hw gpio-gpn0 --irq touch-int --power touch`, then `touchtest`), the screen-off sleep command (`brl_suspend`, through the firmware's command address), and the ESD tick the driver writes. Nothing starts the process on akita yet: there is no init there.

## akita's radios, motor and GNSS against husky's (2026-11-06 night)

From akita's device tree (the factory image's overlays, `__fixups__` resolved): WiFi on PCIe channel 1 with WL_REG_ON gph3-4, host wake gpa5-1, device wake gph3-5 (husky's); Bluetooth on the same lines (BT_REG_ON gpn8-0, host wake gpn6-0, device wake gpn9-0, BLE_DBO gpp24-3); NFC an ST21NFC on hsi2c_0 at G#08, reset gph1-6, interrupt gpa7-6, the PMU clock pad G#3E84 (husky's pins; akita's secure element is a separate ST54 on SPI); the vibration motor a CS40L26A on hsi2c_0 at G#43, reset gph2-3, interrupt gpa6-3 (husky's). GNSS is not husky's: a Samsung part (`samsung,gnss-spi` on spi_21, with the modem's `gnssif`) where husky has the BCM4776, so its driver is new work.

Checked under the hv, without hands: the motor answers husky's `diag haptic up` (CS40L26A, revision A1, its ROM in standby; no waveform played yet, it buzzes). NFC comes up with husky's process unchanged (firmware 02092700, RF config A911, listening A and B, polling A, B and V), three runs of four; Android's own sequence on akita, traced from its i2c events, is the one the process sends. The fourth, on a boot that had come through fastboot and the touch and motor bring-ups, got the chip's reset notification and then no response to CORE_RESET; not seen again.

WiFi: husky's staged driver (payloads/pcie1, stages 00 and 03) brings PCIe channel 1 up on akita unchanged — PHY locked, the link trained to L0 in A#122 µs — and the chip is not husky's: endpoint G#14E4:4449, chipcommon G#10224383, a **BCM4383** revision 2 where husky has the BCM4398. Stages 04 and on (the enumeration ROM, the firmware download through the CA7 path) are written for the BCM4398 and were not run; the BCM4383's firmware, NVRAM and CLM come from akita's own vendor partition. Android picks the `a3` variant for revision 2 (`fw_bcmdhd.bin_4383_a3`, `bcmdhd_clm.blob_4383_a3`; the board's calibration is a `bcmdhd.cal_*`); the whole set, with the Bluetooth patches (`brcm/BCM.hcd` and two others), is copied to /mnt/Harbor/tmp/akita-wifi-fw-2026-11-06/ with its hashes, local only. Google's driver for it is `google-modules/wlan/bcm4383`. Bluetooth (the same combo chip) waits for WiFi's REG_ON, as on husky, so it follows WiFi.
