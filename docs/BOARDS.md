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

Storage-off until 2026-11-06, when its per-unit partitions were backed up (below); since then the UFS link comes up for the flash log in `klog`, and the rest stays off:
- the UFS link and the flash log in `klog`, but no vault, no write protect, no slot mark, no Android switch from the gate;
- no ira (its sources include a devinfo read);
- no init: every driver process assumes husky's panel, touch and rails. The kernel starts the usb process itself, from a native capsule it carries (`target/akita-usb.capsule.vsf`, made by `tools/akita-boot.sh`).

`tools/akita-boot.sh` builds that capsule and the bare kernel (no hv: it lives in husky's top RAM) and boots it, naming the 8a by serial on every fastboot call. `fastboot boot` (from RAM) is not taken by akita's ABL, as husky's wasn't: it says "Booting" and comes back to fastboot. `--flash-a` flashes boot_a only and switches to slot a; slot b keeps Android, and since this ferros marks no slot, ABL falls back to it once slot a's retries run out.

## Bring-up, 2026-11-05 night to 2026-11-06 morning: ferros on akita's USB

ferros enumerates on akita (1209:4665) and answers the bridge: `diag`, `diag console`, `diag "part find <name>"`. What it took, in the order it was found:

1. **The per-unit partitions backed up first**, through Magisk root on slot b's Android (v31.1 on init_boot_b; adb's Shell had to be granted by hand): persist, efs, efs_backup, modem_userdata, devinfo, pinfo, blenv, mfg_data, fips, trusty_persist, misc, ufs_internal, klog and the whole boot LU sdd, into `/mnt/Harbor/tmp/akita-backup-2026-11-06/` with device- and host-side SHA-256 lists that match. Local only.
2. **The X3 hop hangs on akita**: the kernel never came back from `x3::hand_over`. akita stays on CPU0 (`board::x3`).
3. **"The kernel's core" follows the hop** (`process::kernel_mask`): it was the X3 always, so with the kernel on CPU0 a thread pinned to big waited for a core that never started, and every `SYS_POWER` was refused. The same flaw broke husky's `FERROS_X3=0`.
4. **No ira, no ihi**: akita has no vault yet, so no ira and no oracle secret, and `spawn_capsule` refused the usb capsule. `spawn_capsule_no_ihi` checks the signature and runs native code without an ihi (masked code and production builds still refused).
5. **A build that did not build**: `x3::local`'s `.rept 64` asm, inlined into `service::run`, threw off LLVM's branch relaxation, and one `tbnz` landed out of its ±32 KiB range ("fixup value out of range") in some builds. `x3::local` is never inlined now.
6. **Storage on**: the cold UFS link comes up on akita (45 ms) and its GPT reads. The flash log goes to akita's `klog` partition (LU 0, LBA 32774..=36869, found through the GPT, never husky's fixed place), and the processes' console lines follow it there. Read it back from Android with `dd if=/dev/block/by-name/klog`. Vault, write protect, slot mark and the gate's Android switch stay off on akita.

What does not work as a channel on akita:
- `fastboot boot`: ABL says "Booting" and stays in fastboot.
- The RAM log (pstore): akita's ABL clears it at every reset, even an Android-to-Android reboot.
- The panel, until it was fixed the same morning: ABL leaves akita's splash framebuffer at G#FAC0_0000 (1080 × 2400, IOVA = physical), outside the regions the display engine's fence lets it read (s2mpu::fence_dpu: display-fb and display-phases, in the GiB below). Fenced at the service's start, every scan-out read was refused: DECON went on at 60 Hz over a framebuffer the panel never saw, black a few frames into the boot. akita now draws into ferros's own framebuffer (display-fb, G#8400_0000) from the first adoption (`use_own_fb` at entry), and the panel stays lit.

The probes (`FERROS_PROBE=<n>` builds, board.rs) were the channel until USB: at checkpoint n, `probe` goes to the bootloader (fastboot reappearing is the one bit), `probe_log` resets plainly (ABL counts slot a down to Android), `probe_starve` stops the watchdog pets. Checkpoints: 1 board, 2 safety armed, 3 gate, 4 ring, 41-45 board read to the X3 hop, 5 service, 6 home, 7 / 71 / 72 the usb spawn, 81-85 the USB power-up, 86 a process ending, 9 starve after the spawn. A probe taken after the usb process existed reset without stopping in fastboot, and so did `bridge reboot fastboot`: the memory scrub before every reset (platform::scrub_memory) took a permission fault on the first arc's grants page (read-only at every level, the kernel's too; FAR G#8300_0000, read back as PREV_EXC and PREV_FAULT_FAR after the next boot), and the fault's plain reset, which clears the PMU reboot mode, replaced the ordered one. The scrub now returns both pools to kernel-writable pages first (cachemap::pools_to_kernel). Since 2026-11-06, `bridge reboot fastboot` lands akita in fastboot in 37 s, so `tools/akita-boot.sh --flash-a` and the bridge make a loop that needs no hands. The same fault was waiting on husky whenever a process was alive.
