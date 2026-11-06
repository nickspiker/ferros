---
name: akita-pixel8a
description: "Second ferros device: a Pixel 8a (akita, Tensor G3), bootloader unlocked + pKVM disabled 2026-11-05; \"could be anyone's\""
metadata:
  node_type: memory
  type: project
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-06T03:15:42.652Z
---

A Pixel 8a (akita, hw MP1.0, Tensor G3 like husky) joined as a ferros dev device on 2026-11-05. The owner called it a Pixel 8 at first; over USB it names itself "Pixel 8a" (product akita), so check `fastboot getvar product` rather than trusting a name.

- Bootloader unlocked that day (`fastboot flashing unlock`, which wiped it), pKVM disabled (`fastboot oem pkvm disable`, status "disabled (fastboot=kvm misc=auto)"). Bootloader akita-17.0-15199481, baseband g5300o-260317-260505-B-15346003, Android CP2A.260805.005, slot b active.
- Owner on whose it is: "Could be anyones": no particular person's phone, free to wipe for ferros. An identity still goes on it only when the owner says so ([[feedback-identity-belongs-to-a-person]]).
- Same SoC as husky, so the boot path, USB/PT and most of the kernel carry over. The panel, the touch controller, the cutout geometry (ferros_sys CUTOUT_BOTTOM, the display's GLASS_RADIUS are husky constants) and the board's rails differ.
- **Backed up 2026-11-06** (Magisk root on slot b's Android, `dd` read-only): persist, efs, efs_backup, modem_userdata, devinfo, pinfo, blenv, mfg_data (all zeros on this unit), fips, trusty_persist, misc, ufs_internal and the whole boot LU sdd, into /mnt/Harbor/tmp/akita-backup-2026-11-06/ with device- and host-side sha256 lists that match. Local only, never commit (IMEIs). Restore only with `fastboot flash <part> <img>` if damaged ([[husky-factory-partitions-backup]]). Magisk v31.1 on init_boot_b (stock init_boot in the factory image); Shell had to be added to Superuser by hand (`su: request rejected (2000)` until then). The akita factory image (CP2A.260805.005, SHA-256 checked) is at /mnt/Harbor/tmp/akita-factory/, its device trees dumped to text in dt/.

Status 2026-11-06 midday (all in ferros docs/BOARDS.md):
- ferros runs on akita with USB, the bridge, cold UFS, the flash log in `klog`, and the panel lit (its own framebuffer: ABL's splash is outside the DPU fence).
- **Hands-free loop works:** `tools/akita-boot.sh --flash-a` boots ferros on slot a (24 s); `ferros-bridge reboot fastboot` lands in fastboot (37 s, commit 8b313b1: the reset scrub faulted on read-only pool pages, husky too). So akita can be driven unattended.
- **ferros partition carved** (commit e6e5ad0, tools/pixel-carve.sh + tools/flash `ferros-flash`): userdata 32 GiB, ferros LU0 LBA 10838528..=31196154 (77.7 GiB). GPT backups in /mnt/Harbor/tmp/akita-gpt-2026-11-06*/. userdata was wiped (`fastboot -w`): slot b's Android needs setup again, USB debugging, the Magisk app and its Shell grant (init_boot_b is still patched).
- Not yet: vault on the partition (kernel uses husky's FERROS_BASE_LBA; owner picks akita's ira labels), init/drivers, the AK3B panel at 120 Hz (design-first: knobs inventory vs HK3), the X3 hop, the new kernel on husky.
Later 2026-11-06:
- Android on b set up again after the carve, Magisk app reinstalled, Shell granted by hand: root works over adb; Android sees `ferros` as sda35, userdata 32 GB. pKVM came back on with `fastboot -w` and stays on (Android's stock state; ferros boots fine either way).
- The gate: volume down → slot b active + reset (Android; fastboot is the way back); its marks come from board::keys (akita's side keys measured with `display ruler`, in docs/BOARDS.md "Side keys"); akita's panel does not hold one frame, so the gate keeps DECON triggering for its second.
- Open bug: on akita, power (screen off) wedges USB until a reboot.
