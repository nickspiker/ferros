---
name: android-id-userkey-seed
description: "The reset-volatile SSAID \"userkey\" is the seed that produces every app's ANDROID_ID; back it up before wipes OR (permanent fix) derive Photon identity from the ira"
metadata: 
  node_type: memory
  type: project
  originSessionId: 873bc7fe-6da9-4b85-b1db-5d51c15b7843
---

Found 2026-08-20 on husky. "The thing that produces the ANDROID_ID" (that orphaned a Photon device on factory reset) is the **SSAID userkey** — a per-device 32-byte secret in `/data/system/users/0/settings_ssaid.xml` (ABX binary format, root-readable).

- Derivation: `android_id = HMAC(userkey, app_signing_certificate)`. Proven on-device: gms/vending/calendar (same Google signer) all share one id; videos (different signer) another; adb shell yet another. Same userkey + same signing key -> same ANDROID_ID, deterministically.
- So you don't need Photon installed to preserve its future id — back up the **userkey** and always sign Photon with the same dev key; it re-derives the identical id on any device after restore.
- **Reset-volatile:** factory reset regenerates the userkey -> all android_ids change -> apps orphan. THIS is what orphaned the prior device. Backup+restore is a manual crutch (push settings_ssaid.xml back as system:system 0600 with correct SELinux label BEFORE apps first request ids).
- Backup at /mnt/Harbor/Code/keys/husky-identity-backup/settings_ssaid.xml (private keys store, 0700/0600, NOT a repo). The userkey value lives ONLY there — never in memory or any repo. Photon NOT yet installed (checked 341 packages).

**Why:** Photon's interim device identity currently rides ANDROID_ID, which is reset-volatile — the exact fragility the ira ([[ira-network-owns-assignment]]) exists to kill.

**How to apply:** permanent fix = derive Photon's device identity from the reset-permanent chip-ID ira (`derive_ira`), not ANDROID_ID. Until then, back up the userkey before every wipe (wipes are ~100% likely on this project). USB caveat: eSIM/modem provisioning can drop DWC3 device-mode USB mid-session (replug to recover) — see [[husky-wifi-symbol-mismatch]] context.

**Restored 2026-11-02 after a factory reset:** `settings_ssaid.xml` (the 08-20 file, which carries Photon's entry) is the one Photon's identity is bound to; `settings_ssaid.fresh-2026-10-29.xml` is NOT (Photon saw a new device under it). Procedure, no reboot: `adb push` to /data/local/tmp, then as root `cp` over /data/system/users/0/settings_ssaid.xml, `chown system:system`, `chmod 0600`, `restorecon`, then `stop; sleep 3; start` (Android stays up; a reboot would land in ferros's gate). The shell's android_id does not match the 08-20 record afterwards, so it is not a valid check; Photon recognising the device is.
