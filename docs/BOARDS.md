# Boards

ferros runs on two Tensor G3 (zuma) phones. The kernel tells them apart at entry from the root `model` of the device tree ABL hands over (`ferros_kernel/src/board.rs`), before anything board-specific runs.

| | husky (Pixel 8 Pro) | akita (Pixel 8a) |
|---|---|---|
| Model in the device tree | ZUMA HUSKY … | ZUMA AKITA MP board based on ZUMA (board id 40406, hardware MP1.0) |
| DRAM | 12 GB, top at G#B_0000_0000 | 8 GB, top at G#A_0000_0000 |
| The hv's high RAM (gate mark G#A_FFFF_FF10, the jump's mailbox and entropy pool) | yes | no: past the end of DRAM |
| Keys | vol down gpa4-1, vol up gpa6-2, power gpa4-0 | the same |
| UFS VCC enable | gpp0-1 (zuma-ufs.dtsi) | the same (not overridden by its overlay) |
| eUSB repeater | hsi2c_11, address G#3E | the same; its own tune (below) |
| Type-C controller | MAX77759 at G#25 on hsi2c_11 | the same |
| Fuel gauge | MAX77759 at G#36 on hsi2c_11 | the same |
| Low 2 GB carve-outs | zuma's | the same (they come from the SoC's tree) |
| Panel, touch, cutout | HK3, FTM5, 1344 × 2992 | different, not brought up |

akita's repeater tune, from its MP board overlay (`repeater_tune1..5`): G#50 = G#0A, G#70 = G#5F, G#71 = G#2E, G#73 = G#80, G#79 = G#40 (ferros_hal `AKITA_REPEATER_TUNE`).

## akita's facts, and where they came from

From its factory image (CP2A.260805.005, `/mnt/Harbor/tmp/akita-factory/`, SHA-256 checked against Google's): `vendor_kernel_boot.img` carries four zuma base trees, `dtbo.img` nineteen akita board overlays (DEV, Proto, EVT, DVT, PVT, MP); this phone is the MP one. Dumped to text in `/mnt/Harbor/tmp/akita-factory/dt/` by a scratch walker (no `dtc` on the host).

The phone itself, 2026-11-05: bootloader akita-17.0-15199481, baseband g5300o-260317-260505-B-15346003, unlocked that day (`fastboot flashing unlock`, which wipes it), pKVM disabled.

## How akita boots for now

Storage-off until its per-unit partitions are backed up (persist, efs, efs_backup, devinfo, mfg_data: factory calibration and the modem's IMEIs, in no factory image):
- no UFS power-up, so no flash log, no vault, no write protect, no slot mark, no Android switch from the gate;
- no ira (its sources include a devinfo read);
- no init: every driver process assumes husky's panel, touch and rails. The kernel starts the usb process itself, from a native capsule it carries (`target/akita-usb.capsule.vsf`, made by `tools/akita-boot.sh`).

`tools/akita-boot.sh` builds that capsule and the bare kernel (no hv: it lives in husky's top RAM) and boots it, naming the 8a by serial on every fastboot call. `fastboot boot` (from RAM) is not taken by akita's ABL, as husky's wasn't: it says "Booting" and comes back to fastboot. `--flash-a` flashes boot_a only and switches to slot a; slot b keeps Android, and since this ferros marks no slot, ABL falls back to it once slot a's retries run out.

## First boot, 2026-11-05 night

Flashed to slot a: nothing on USB afterwards, not ferros, not fastboot, and no fall back to Android over the next minutes. A hang before ferros arms its watchdogs would have been reset by ABL's and counted down to slot b, so the likely state is ferros running, petting its watchdogs, with no USB, or the phone off. Not yet looked at: the panel (the boot ring on ABL's splash shows which step it reached).
