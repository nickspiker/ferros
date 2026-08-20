---
name: pipe-kore-state
description: "PIPE chip operational states are te reo Maori (KORE/WHARA/HARA/ORA/NGARO); chip_state is 2-bit, NGARO is silent (no encoding); cosmology runs KORE -> KOHI -> BLAKE3 -> TAHU -> ORA"
metadata: 
  node_type: memory
  type: project
  originSessionId: e1f7e536-25a2-41fa-b58f-7964fd13e2e6
---

In the PIPE project, all chip operational states are named in te reo Maori (swept project-wide 2026-06-07). The full state set:

| State | Meaning | chip_state code | valid_count |
|-------|---------|------------------|-------------|
| KORE  | primordial void; unprovisioned, storage all zero | 2'b00 | 0 |
| WHARA | bruise/injury; at byzantine minimum (2 valid pairs) | 2'b01 | 2 |
| HARA  | breach/error; single fault tolerated (3 valid pairs) | 2'b10 | 3 |
| ORA   | alive/well; fully healthy (4 valid pairs) | 2'b11 | 4 |
| NGARO | lost/vanished/silent; byzantine fail | (no code) | < 2 and not all_kore |

**Why these names:** Te Pō/Te Kore is the primordial darkness in Maori creation cosmology; ORA is the standard "alive/well" word ("kei te ora" = "are you alive"); WHARA / HARA pair as injury (bruise) and breach (error). NGARO = lost, silenced, vanished -- exact match for "indistinguishable from a chip not physically present" wire behaviour. Previous English names (VIRGIN/HEALTHY/DEGRADED/DARK) had USPTO-inappropriate connotation or were inconsistent with the existing Maori cosmology of KOHI/TAHU/ihi/ira/wairua.

**chip_state encoding (2 bits):**
- The numeric value rises with chip vitality. KORE = 0 means "all storage zero, no identity"; ORA = 3 means "all four pairs verified, fully healthy."
- NGARO is **not** assigned a numeric code because the chip in NGARO state does not transmit. The host infers NGARO from the absence of any wire response within the protocol timeout, which is indistinguishable at the wire from a chip not physically present. See [[pipe-fucked-state-silent-wire]] (renamed to NGARO).
- Internal to silicon, `chip_state_ngaro` is a separate 1-bit predicate orthogonal to `chip_state`. It is derived as `~byzantine_ok & ~all_kore`. The two signals can both be true simultaneously (e.g., 2 valid pairs disagreeing -> chip_state = WHARA AND chip_state_ngaro = 1). The chip in NGARO state is gated silent on the wire regardless of chip_state value.

**Cosmology arc:** `KORE -> KOHI -> BLAKE3 -> TAHU -> ORA` (void -> gather -> condition -> ignite -> alive). The chip is born the way the Maori cosmos is. HARA and WHARA are post-provisioning operational states with single-pair or two-pair damage; NGARO is the silent failure terminal state.

**How to apply:**
- Use uppercase for state names in enums, comments, and protocol fields (KORE, WHARA, HARA, ORA, NGARO).
- Use lowercase for signal names (chip_ngaro, pair_kore, all_kore, c_pair_kore, c_all_kore).
- The predicate "chip can respond on the wire" = `!chip_state_ngaro` (KORE chips can respond saying "I'm KORE", they just have no identity to sign with).
- The predicate "chip can authenticate" = `chip_state >= WHARA` (i.e., chip_state >= 2'd1) = byzantine quorum met.
- The predicate "chip is fully healthy" = `chip_state == ORA` (chip_state == 2'd3) = all 4 pairs valid.
- If introducing the state machine to a reader unfamiliar with te reo, gloss each term on first use as the table above.
- If any new VIRGIN / HEALTHY / DEGRADED-1/2 / DARK / FUCKED appears in commits, comments, or docs, sweep it. The convention is locked in.

Files touched in the sweeps (2026-06-07):
- patent.tex: Terminology entry expanded for all 5 states with te reo glosses; state table updated with numeric encoding paragraph; Detailed Description narrative updated; FIG. 2 description; FIG. 7 description; Claim 22 (drive-enable AND-gate) chip-alive -> chip-operational rename.
- rtl/inverted_pair_verify.v: 2-bit chip_state encoding, separate chip_ngaro output, ST_KORE/WHARA/HARA/ORA localparams.
- rtl/ira_fsm.v: chip_state input 3-bit -> 2-bit, chip_state_ngaro input added, ST_NGARO terminal state.
- rtl/l3_session_fsm.v: chip_state_in 3-bit -> 2-bit, chip_state_ngaro_in added, Q_HEALTH state_block layout updated.
- rtl/oled_mode_fsm.v: chip_state display strings updated to NGARO/KORE/WHARA/HARA/ORA with ngaro-priority.
- rtl/otp.v, rtl/otp_persistent.v: VIRGIN -> KORE.
- rtl/measure/*.v: testbenches updated for new encoding (T2/T3/T4/T5/T6/T7/T8/T9/T10/T11 in inverted_pair_verify_tb; ira_ceremony_tb pass condition).
- IRA.md, PROTOCOL.md, MEAL.md, TRNG.md, README.md, patent/claims.md: state names swept.

Verification:
- inverted_pair_verify_tb: 33/33 PASS
- ira_ceremony_tb: PASS (KORE -> ORA ceremony completes, ira matches blake3_ira, chip_state = 3, chip_ngaro = 0)
- l3_session_fsm_tb: 8/9 PASS (pre-existing BLAKE3 handshake bug unrelated to rename)
