---
name: owner-decisions-2026-11-22
description: Owner decisions 2026-11-22 - vault flush per commit; 3a key adopted; camera power stays in kernel; colour verified; screenshot slide 1/24; full-range focus; Mahere free rein on GUI + global scale; overnight button-up
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-23T05:08:46.504Z
---

Decisions the owner made on 2026-11-22 (each recorded where the work landed too):
- **Vault:** a UFS cache flush per commit ("first"), measured ~90 ms on a 25 MiB keep; see [[vault-flush-per-commit]].
- **Fleet:** fd1fa6a7 (a Pixel 3a XL on the bench) is his: "3a is fine. I did mean to add it." Its minted fleet key (fan-out revision 2762, 706917fe) was adopted by the Pro (forge 70ea52ab). Open: a key generation number in fgtw.org's slot (recommended) vs never resealing; keeping the fleet key's revision on disk (ferros_kept format change).
- **Camera:** the camera's power switch stays in the kernel ("cam power sounds like it needs to stay in kernel, totally cool"), which answers KERNEL-OUT decision 3. The ultrawide/tele B G / G R colour fix is verified by eye ("colour is all good"). Focus: full lens travel both ways ("enable full range both ways and I'll inspect it"), the list holding only tracked travel (coil-current safety). Tele focus via humbaba, decoded from an Android trace.
- **Slits:** speed is the goal; ideally 2 rows (1:1: the quad's middle two rows, R R G G over G G B B).
- **Screenshot:** the helm two-finger slide needs a 24th of the harmonic span, not a twelfth ("1/24th, let's try that"); the degauss "looks good".
- **Mahere:** its ferros GUI is the Mahere session's to make nice ("like we did for Photon since shit overlaps like crazy"); the global app scale must be honoured ("Yes, global scale should be honored").
- **Kernel-out tonight:** the touch handshake and the remaining screen pieces leave the kernel.
- **Dev box:** the USB card (ASMedia ASM1042, new) dies about hourly under use; tools/usb-card (keep-awake udev rule + an event-driven reviver) was written for him to install with sudo.

Still unanswered: the tele LATE_CONFIG recovery vs 48.7 fps; fixing the M1 build (broken since 4adb1e6d, ~270 errors); first sound and first buzz on the Pro (wait for him present).

Related: [[owner-decisions-2026-11-21-night]], [[kernel-out-moves]], [[husky-camera-bringup]].
