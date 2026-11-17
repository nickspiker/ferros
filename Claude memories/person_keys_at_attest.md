---
name: person-keys-at-attest
description: "Every person key derived from the roots goes to the kernel at attest (the bind), never after the identity space's first pull"
metadata:
  type: project
---

Owner, 2026-11-17: "all my person keys should be immediate as well, no reason why not."

**Why:** fgtw handed the person key to the kernel only in space_step after the space's first pull settled (fgtw.org or a sibling's copy). A kist launched within about a minute of attest waited on the network, and Photon sat on "Waiting for your identity" (the Pro, t10614890: no pull settled in 74 s, be you never sent).

**How to apply:** person keys come from the roots, so they go to the kernel the moment the roots arrive; the space pull runs alongside and gates only what really comes from the space. Related: [[identity_not_on_disk_default]], [[ferros_attest_os_level]].
