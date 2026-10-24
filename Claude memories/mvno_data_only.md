---
name: mvno-data-only
description: the ferros/Photon MVNO is data-only — no POTS, no SMS, no MMS, no 911; emergency services go through Photon (sketched, optional, not legally required)
metadata:
  type: feedback
---

The MVNO ("peoples democratic utility") is **data-only: no POTS, no phone numbers, no SMS, no MMS, no 911.** Emergency services are sketched through Photon as an easy optional add (not legally required for data-only). Stated in README.md ("Data-only") and GLYPH.md.

**Why:** I twice planned around telephony anyway (listed E911 and number porting as costs, suggested moving the phone number into a service); the owner corrected it 2026-10-24 ("remember").

**How to apply:** never assume voice/SMS/numbers in cellular, MVNO, compliance or identity plans. Data-only drops E911, number porting, voice CPNI and USF; CALEA likely still applies to a full MVNO with its own packet core — comply and move on, no legal review needed (owner, 2026-10-24: interception is their time, not his; traffic is Photon-encrypted). Consistent with [[threat_model_harm_not_observation]]. Check README.md before reasoning about ferros telephony. Related: [[radio_addresses_belong_to_identity]], [[cellular_ttl_63]].
