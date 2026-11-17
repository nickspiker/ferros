---
name: person-keys-at-attest
description: "The person key goes to the kernel at attest (the bind) from this device's own copy of the space; only a fresh device waits for the pull; it must never be reachable outside the kernel"
metadata:
  type: project
---

Owner, 2026-11-17: "all my person keys should be immediate as well, no reason why not." And: "As long as all this is priveliged and in the kernel so once we lock it there's ZERO way someone can get into those keys."

The person key is not derived from the roots: one random key per identity, minted once (several eggs) by the identity's first device, kept in the identity space sealed under the fleet key, opened by every other device from the space (fgtw person.rs settle/find, payloads/fgtw space.rs space_person). The space's first pull decides which key exists (mint race: earliest stamp then smaller id wins; a fleet-key change means wait, never mint).

**Why:** fgtw handed the key to the kernel only after the first pull settled, so a kist launched within a minute of attest waited on the network (the Pro, t10614890: no pull settled in 74 s, Photon stuck on "Waiting for your identity").

**How to apply:** at the bind, open the person key from this device's own vault copy of the space and hand it to the kernel at once; never mint before the pull settles; the pull may re-hand a different key once if the fleet's space says another won (kist aliases change once). The key lives in the kernel; any user-process copy (fgtw) is wiped at once after hand-in. Related: [[identity_not_on_disk_default]], [[ferros_attest_os_level]], [[keys_in_registers]].
