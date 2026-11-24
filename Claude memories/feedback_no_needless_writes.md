---
name: feedback-no-needless-writes
description: Owner 2026-11-24 - destroying data means destroying its keys, never deleting or overwriting it; flash has finite writes; sealed data left behind is "sand on a beach"
metadata:
  type: feedback
---

Owner, 2026-11-24, on unbind's bulk erase: "whoever's dumbfuck idea to add more writes to an SSD that has a fixed number of writes is beyond me but it's dead wrong. Sand on a beach my friend."

**The rule:** data at rest is sealed, so making it unreadable is a key operation: wipe the keys in memory and renew the device-only secret (one small write). Never delete, overwrite or "clean up" sealed data to destroy it; once its keys are gone it is noise, and the vault's normal rotation (the reap trailing the plow) reclaims the space when it needs it. Applies to unbind, shred, revoke, lock and any future wipe.

**Unbind, the owner's sequence:** handle entry, a request to the fleet, consent from an existing fleet device, the device signs itself out, the fleet rotates its key, the device wipes its keys from memory. No disk action beyond renewing the device secret.

**How to apply:** before adding any write in a destroy, wipe, cleanup or "tidy" path, ask whether a key operation already makes it moot; it almost always does. Related: [[storage-seed-only-fixed]], [[attest-to-approve-settings]], [[vault-flush-per-commit]].

**Nothing without consent (owner, 2026-11-24):** "if it does not get my consent from my other devices or a correct handle entry, nothing gets wiped. Lock is fine, but no wipe and certainly no unbind." Every path that destroys keys, renews the device secret or unbinds is gated by a correct handle and the fleet's consent, enforced in the kernel and fgtw; anything short of that changes nothing, at most a lock.

**The salt lives in the security chip (owner, 2026-11-24):** "No? The TPM holds the salt." Identity-scope keys = derive(addr, seed, device secret, salt); shred and unbind destroy only the salt, inside the chip (a real erase, no FTL copies). TPM: an NV index holds it; StrongBox/Secure Enclave: salt = HMAC(non-exportable chip key, label), delete the key. No chip: the salt in the vault, accepting the flash leftover. ferros on the Pro: the Titan M2 would hold it (not driven yet, a later project); meanwhile the device-only secret in the vault, with UFS UNMAP + purge proposed as the interim.
