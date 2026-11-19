---
name: husky-flash-paths
description: Pro's boot_a flashing on 2026-11-06 night — fastboot 64 MiB send failed every time: THE CABLE (fixed on another cable, USB 3 port); ~21 MiB prefix works if the footer is unchanged; Android dd works only when the running hv has no UFS firewall
metadata:
  type: project
---

On the Pro (husky) on 2026-11-06 night, `fastboot flash boot_a` of the signed 64 MiB whole-partition image failed every time ("Write to device failed in SendBuffer()"), after a first success the same evening; `-S` sparse made ABL answer "failed to flash partition (boot_a): -7". A 21 MiB image went through fine. Two working routes:
- **Prefix:** pad the gate image to the size the AVB footer on the device already names (read it from Android: last 64 bytes of boot_a), sign, check the new footer is byte-identical, and flash only the first (size + 4 KiB) bytes. Breaks when the new gate is bigger than the footer's size.
- **Android dd:** `adb push` the image, `dd` it onto /dev/block/by-name/boot_a as root, hash-check. Works only when the hv running under Android was built without `FERROS_HV_UFS=1`: the firewall refuses the write (the hash comes back as the old image's), as designed.
Scripts lived in the session scratchpad (flash_prefix.sh, dd_boot_a.sh). **It was the cable (2026-11-07 morning):** on another cable, in a USB 3 port (bus 4, 5000M), the same 64 MiB flash went through in 0.15 s. The routes below are fallbacks only ([[feedback-usb-port-wedge]]).

**Why:** a whole night of oracle captures ran on a stale build before the failed flash was noticed (the scripts hid fastboot's output).
**How to apply:** always check fastboot's OKAY/FAILED for boot_a; prefer dd while the running build has no firewall; restore a firewall build last.

**2026-11-18 (pro-flash-1118, b971/b972):** the whole 64 MiB send failed again twice ("Write to device failed in SendBuffer()"), the Pro in fastboot behind the hub at 2-2.2 (5000M; ferros itself enumerates at 1-2.2). The prefix route took both slots in under a second each: the gate padded with zeros to the image size the device's footer names (22040576), signed with `ferros-mkimg avb-footer` (template target/avb/boot.img), its last 64 bytes checked identical to the device's footer, the first size + 4 KiB flashed. Proof before flashing: prefix + the device's old tail (from `bridge part dump`) = the whole signed image, byte for byte; after: both slots dumped back, same blake3 as the signed image (kept in keys/husky/images as boot_pro-flash-1118_718df3bb_vaultcarried.img). The prefix route breaks once a gate outgrows the footer's size.
**Carried vault (same day):** the Pro's flashed builds since forge 383c6497 carry a NATIVE vault capsule in the kernel (FERROS_VAULT_CAPSULE, VAULT-OUT.md); the stored `vault` entry is a masked one that may never have been sealed. A kernel built without it starts the stored one instead, so build every Pro kernel with FERROS_VAULT_CAPSULE=<a native vault capsule from the same tree> (check: two "ferrosRT" image headers in ferros.bin, usb and vault).
