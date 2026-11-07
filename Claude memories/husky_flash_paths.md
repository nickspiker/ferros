---
name: husky-flash-paths
description: Pro's boot_a flashing on 2026-11-06 night — fastboot's 64 MiB send fails every time; ~21 MiB prefix works if the footer is unchanged; Android dd works only when the running hv has no UFS firewall
metadata:
  type: project
---

On the Pro (husky) on 2026-11-06 night, `fastboot flash boot_a` of the signed 64 MiB whole-partition image failed every time ("Write to device failed in SendBuffer()"), after a first success the same evening; `-S` sparse made ABL answer "failed to flash partition (boot_a): -7". A 21 MiB image went through fine. Two working routes:
- **Prefix:** pad the gate image to the size the AVB footer on the device already names (read it from Android: last 64 bytes of boot_a), sign, check the new footer is byte-identical, and flash only the first (size + 4 KiB) bytes. Breaks when the new gate is bigger than the footer's size.
- **Android dd:** `adb push` the image, `dd` it onto /dev/block/by-name/boot_a as root, hash-check. Works only when the hv running under Android was built without `FERROS_HV_UFS=1`: the firewall refuses the write (the hash comes back as the old image's), as designed.
Scripts lived in the session scratchpad (flash_prefix.sh, dd_boot_a.sh). Untried: another cable/port, which may simply be the cause ([[feedback-usb-port-wedge]]).

**Why:** a whole night of oracle captures ran on a stale build before the failed flash was noticed (the scripts hid fastboot's output).
**How to apply:** always check fastboot's OKAY/FAILED for boot_a; prefer dd while the running build has no firewall; restore a firewall build last.
