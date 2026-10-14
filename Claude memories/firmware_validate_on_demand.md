---
name: firmware_validate_on_demand
description: Firmware-chain checking on husky is an on-demand VALIDATE screen (slots A/B + each part vs known-good hashes, with an explicit spoofable disclaimer), NOT a per-boot hash or sealing — owner decision 2026-10-14
metadata:
  type: feedback
---

**Decision (owner, 2026-10-14):** ferros does NOT hash the boot chain on every boot, and does not seal the vault to firmware measurements. Instead there is an on-demand **validate** screen/command:
- It shows which slots exist (A and/or B).
- For every firmware part (bl1, pbl, bl2, bl31, abl, tzsw, gsa, gsa_bl1, ldfw, ...) and each slot, it shows known-good (named audited/published build) / unknown / missing, plus the raw hash.
- It always carries the disclaimer: *for informational purposes only — could be spoofed by EL3 (or anything that runs before ferros) if Google's firmware were hostile.*

**Why:**
1. **Boot budget:** total boot time, excluding Google's own chain and the unlock nag, must stay under 1 s. Hashing about 14 MB per slot every boot doesn't fit, and with the data cache off it's worse.
2. **The owner flashes:** boot-time enforcement would fight that.
3. **The trust floor:** on a Pixel the floor is Google-signed EL3/ABL/security chip. Anything that can rewrite ferros can fake a check, so a "secure" green indicator would be theater. The real value is spotting swapped or downgraded Google-signed builds (old signed firmware with a known vulnerability is the realistic route to a hostile EL3), and an on-demand check covers that.

**How to apply:**
- Never show a "secure"/green badge.
- Phrase results as "matches known-good build X", never "safe".
- Measure both slots plus wherever the boot ROM actually loads from (likely the UFS boot areas, not only GPT partitions).
- Known-good list = hashes from our own reverse-engineering audit and the published factory images (bl31 of ripcurrent-16.4-14540574 = SHA-256 cf009f04c52e7d092bd98dfea0ddf25edac5dfb41fa67f0d52c856e68e62d886, both slots matched on 2026-10-14).
- Needs disk access: possible on the handoff path now, on cold slot b once the UFS link works.
- Full chain-of-trust guarantees are only possible on own silicon (PIPE/TOKEN). See [[threat_model_harm_not_observation]], [[husky_cold_slot_b]].
