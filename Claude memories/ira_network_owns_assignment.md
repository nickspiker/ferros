---
name: ira-network-owns-assignment
description: "Two secret classes only (ira permanent / wairua per-session); AnchorKey is ira-rooted and survives factory reset; the FLEET holds ira→handle assignment, the device NEVER assigns itself"
metadata: 
  node_type: memory
  type: project
  originSessionId: 873bc7fe-6da9-4b85-b1db-5d51c15b7843
---

Settled 2026-08-19 after many corrections. GLOSSARY.md is authoritative; there are exactly TWO device-secret lifetime classes:

- ***ira*** — permanent identifier, begotten once, survives factory reset/reflash/everything. Root of mana, attestation, and the vault AnchorKey (`system_persistent_secret` in the oracle formula). Interim husky ira = BLAKE3 over permanent identifiers (chip-ID MMIO G#1000_0000, UFS serial, IMEI) — permanent but vendor-known, the exact sovereignty gap whakaira/PIPE closes ([[pipe_ira_is_meaningless_alone]]).
- ***wairua*** — per-power-session secret, fresh TRNG at first handshake after power-on (husky: SMCCC TRNG, proven bare-metal callable), volatile registers, dies at power interruption, never persisted.
- **Flash is the third class — the DATA class, never a key class.** Data on flash is ciphertext whose effective lifetime is inherited from its key: ira-encrypted = permanent, wairua-encrypted = session-scoped with free crypto-erase at power loss. Keys are derived into registers, NEVER stored. The StrongBox HMAC's weird "survives reboot, dies on factory reset" lifetime = key-material-stored-as-data (a wrapped blob in Android's flash); disqualified as anchor root — factory reset wipes userdata but NOT the ferros partition (sda35), so a reset-mortal AnchorKey destroys the owner's vault while its ciphertext survives. Keystore/StrongBox warehouses keys for an OS that can't derive its own — a problem ferros does not have.
- **The TOKEN lives with the fleet ("#0 and of MOST importance").** The network/custodes registry maps ira→owning handle across any number of resets. Owner may reassign an ira/IMEI to a different handle via the network; **the device itself NEVER assigns or reassigns its ownership, not by any means, ever.** A factory-resetting thief must NOT get a pristine attestable phone. Titan attestation key (permanent, sign-only, survives reset) = possession proof for network challenge-response; its ECDSA non-determinism only disqualifies key derivation, not challenge-response.
- AnchorKey = BLAKE3-derive_key("ferros.vault.anchor.v0", ira). Passless — no passphrase/PIN/Argon2 anywhere. Anti-theft = fleet registry + TOKEN, never key suicide.
- **Release share (the zeroable key lives OFF-device):** flash can't be trusted to forget (UFS wear-leveling remaps blocks; stale key copies survive overwrite), and remote-killing a seized device is best-effort (Faraday bag). So sensitive vault domains derive from `ira ‖ release_share` where the share is held by the fleet or Shamir K-of-N friends ([[token_recovery_shamir_not_oprf]]) — ira-class lifetime at its home, wairua-class residency on the phone (registers only, never persisted). Killswitch reverses direction: zero the share AT HOME and the tier dies everywhere, even on a phone offline since seizure. Vault tiers: ira-only domain opens offline; share-gated domain for what matters. This is the software interim of the TOKEN (endgame: same split, Bob's key becomes PIPE silicon in the owner's pocket).

**Why:** I was corrected three times on this thread: "reset destroying the ira is arguably correct" ("are you daft?"), dismissing reset-permanence (criterion #0), and conflating wairua and ira (the third-class chimera).

**How to apply:** Every device secret belongs to exactly one class — ask "ira or wairua?" before assigning any key a lifetime. Never propose device-side ownership decisions or reset-mortal persistent roots. Full design in ferros/VAULT-KEY.md; definitions in ferros/GLOSSARY.md.
