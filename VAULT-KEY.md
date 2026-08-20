# VAULT-KEY — ira, wairua, and anchor-key sourcing on husky

**Status:** settled design, 2026-08-19. Supersedes the earlier passphrase/Argon2 draft and the intermediate Titan-HMAC draft. ferros is **passless** (no passphrase, no PIN, no FIDO, no passkeys, ever). Terminology per [GLOSSARY.md](GLOSSARY.md), which is authoritative. Numbers use `[base36]#` notation per project convention.

---

## Two key classes, one data class

[GLOSSARY.md](GLOSSARY.md) defines exactly two lifetime classes of device *key*:

- ***ira*** — the permanent identifier, begotten once, the device's lifelong identity. Survives factory reset, reflash, updates, everything. Root of *mana*, of attestation commitments, and of the vault's persistent key — the `system_persistent_secret` in the oracle formula.
- ***wairua*** — the per-session secret, drawn from fresh physical entropy at the first handshake after power-on, held in volatile registers until power interruption. Never persisted, reborn each boot.

The third class is **flash** — the storage medium, where data lives. It is not a key class: nothing on flash is secret or trusted by location, it is ciphertext, and each piece of data's *effective* lifetime is inherited from whichever key encrypted it:

- **ira-encrypted flash** = permanent data. The vault. Survives everything its key survives, which is everything.
- **wairua-encrypted flash** = session-scoped data. The bytes physically persist but become unrecoverable noise the instant power drops — [KEY_REGISTERS.md](KEY_REGISTERS.md)'s "freeze IS the destruct" extended to storage. Crypto-erase for free: no wipe pass, no destructor, nothing to do in the critical path.
- Mixing (a key derived from ira ‖ wairua, or layered) is available per goal; the lifetime is the shorter-lived ingredient's.

**Keys never live in the data class.** ira and wairua are *derived into registers* at boot, never stored. That rule is what dissolves the chimera an earlier draft built: the StrongBox HMAC's odd "survives reboot, dies on factory reset" lifetime is exactly what key-material-stored-as-data looks like — a wrapped key blob in Android's flash, inheriting flash-data lifetime. Keystore hardware exists to warehouse keys for an OS that cannot derive its own; ferros derives, and it has its own SSD partition for everything worth storing — so the entire StrongBox apparatus solves a problem ferros does not have.

**Final verdict on StrongBox: worthless to ferros, by syllogism.** The vault ciphertext is on the disk; the disk is in the device; therefore the only attacker confidentiality must resist is one physically holding the device. Against that attacker StrongBox offers exactly two things: extraction-resistance (moot — the key and the data travel in the same pocket) and auth-gated, throttled key use (a defense for low-entropy lockscreen secrets, and passless ferros has none). The root cause is a threat-model inversion: StrongBox is built on Google's assumption that *the holder of the device is the adversary* (the DRM posture); ferros is built on the opposite — possession plus recognition IS the owner. A chip whose job is to distrust the hand holding it has no job in a passless OS. The single Google-silicon remnant with any value is the Titan factory attestation *signature* (the fleet possession proof above) — which is not StrongBox, is chainload-era only, and is garnish the design drops at zero cost. Concretely on husky it also fails: **factory reset wipes userdata, not the ferros partition.** The vault ciphertext on sda35 survives an Android reset, so rooting the vault in a reset-mortal key blob means a thief (or a fat-fingered settings menu) destroys the owner's vault while every ciphertext byte sits intact. Key suicide is not the anti-theft mechanism. The vault root is *ira*-derived: as permanent as the partition it protects.

## The ira on husky

### The criteria (owner's ranking, most important first)

0. **Unchangeable by anyone** — eFuses, serial numbers, IMEIs. Survives reset, reflash, updates. This is #0 and of MOST importance.
1. **Not readable on stock Android.**
2. **Nobody knows** — never existed outside the hardware, no key supplied by or extractable to anyone.
3. **Forgeable exactly once** — MINT in the proof-of-work sense (like the spaghettify handle proof), not flash-write. Likely impossible on husky.

### Candidates, sorted by lifetime class first

| candidate | class | 0 permanent | 1 hidden from stock | 2 nobody knows | usable as |
|---|---|---|---|---|---|
| IMEI (modem NV) | ira | YES | no (`*#06#`) | no (Google, carrier) | ira ingredient; the network-facing name |
| chip-ID (MMIO `G#1000_0000`, `ro.boot.hw.soc.id`) | ira | YES | no (boot prop) | no (Google) | ira ingredient; bare-metal readable today |
| UFS serial (device descriptor) | ira | YES | mostly (root/adb) | no (Google, vendor) | ira ingredient once UFS is up |
| Titan M2 attestation key | ira | YES | yes (sign-only) | yes | possession proof (challenge-response); ECDSA non-determinism is irrelevant there and only disqualifies it for key derivation |
| StrongBox HMAC key (ours, measured deterministic) | key-in-flash (data class) | **no — wiped on factory reset** | yes | yes | **disqualified as anchor root:** a wrapped key blob in Android's flash, so it inherits flash-data lifetime; violates criterion 0 |
| SMCCC TRNG (bare-metal callable, proven) | wairua | n/a — fresh each boot | n/a | n/a | the wairua source: boot-time entropy into volatile registers |
| PIPE OTP key (endgame) | ira | YES | n/a | YES (TRNG-born, never leaves) | true ira, and the only candidate with MINT (KOHI→HORO→TATARI→TAHU) |

No permanent husky value is also nobody-knows: everything that survives a reset is known to Google or public. That is measured fact, and it is precisely the **vendor-provisioned sovereignty gap** the GLOSSARY interim row already names (interim ira "permanent, per-device, unextractable, but vendor-provisioned — the sovereignty gap *whakaira* closes"). We take the gap with eyes open; PIPE closes it.

**Interim husky ira** = `BLAKE3-derive_key("ferros.ira.husky.v0", <un-quantized per-device identifiers>)` — as many *un-quantized* permanent identifiers as the boot phase can reach. The selection rule below is the load-bearing part: **only un-quantized sources count.** Permanent by construction; secret from nobody with the phone in hand — acknowledged, interim, and exactly why the enforcement below is external.

### Source selection: un-quantized only (the ASV gravestone)

The hard-won rule from the source hunt: **a source is worth keying on only if it is *un-quantized*.** The moment the fab bins a value for manufacturing consistency, its distribution is peaked, its min-entropy (`−log₂` of the *most likely* value, not `log₂` of the field width) collapses to a bit or two, and — fatally — we cannot even measure that from a single device. Binned values are device-*class* metadata, not device-*identity*.

**Gravestone — do NOT re-chase these as key material** (all confirmed binned/quantized/constant in gs-chipid.c and friends): ASV table (`G#9000`), HPM/ASV curve (`G#A000`), DVFS version (`G#900C`), lot_id / lot_id2, EVT/package revision, product_id/SoC-type, PMIC revision. They cluster by design; ASV *is engineered to be common*. Dinner for everyone, not just us.

### husky source inventory (from the reference-kernel sweep)

Sorted by our two axes — un-quantized (keyable) vs binned (metadata), and which vendor knows it (dispersion). All "chipid block" offsets are bare-metal `readb`/`readl` at EL2 in the `G#1000_0000` region (`gs-chipid.c`), no driver.

**Gold — bare-metal, un-quantized, per-device (key on these):**

| source | offset / path | width | party who knows | status |
|---|---|---|---|---|
| die `unique_id` | chipid `G#4`–`G#B` | 64-bit field, ~40-bit effective (`%010llX`) | Google | **keyed now** |
| `ap_hw_tune` | chipid `G#C300`–`G#C31F` | 32 bytes, per-*device* fuse trim | Google | instrument first, key after stability probe |
| `raw_str` aggregate | chipid (mixed) | 172 B | Google | superset of the two above |

`ap_hw_tune` is per-*unit* analog trim (gs-chipid.c:422–438), unlike the binned ASV beside it — the one genuinely new find. It rides the stability probe before entering `derive_ira`: one drifting bit bricks the vault, and its entropy is unmeasurable from one device, so it is identity hardening, never a claimed secret.

**Dispersion tier — per-device-unique, driver cost, each behind a *different vendor* (fold in as drivers land):**

| source | party | access |
|---|---|---|
| UFS serial / CID | Samsung (flash) | UFS descriptor (driver in hand) |
| WiFi + BT MAC | Broadcom | radio driver (later) |
| fingerprint serial | Goodix | SPI (later) |
| battery / fuel-gauge ROM ID | Maxim/ADI | I²C (same bus as the salt probe) |
| display panel serial | Samsung Display | DSI (later) |
| IMEI | Samsung modem + carrier | modem (later) |
| GSC attestation key | Google | SPI `/dev/gsc` (chainload; possession proof, not key material) |

The value of the dispersion tier is **not secrecy** — every entry is enrolled with *some* vendor — it is that no single party (Google included) holds them all. Reconstructing the full ira would require compelling seven independent vendors. "More is better" means more *un-quantized sources across more vendors*, hardening identity and dispersing the warrant surface; it does **not** mean piling on binned fields, which adds nothing.

**Secret tier (unchanged):** the 256-bit TRNG salt carries real secrecy + erasability; the un-quantized analog physics (`ap_hw_tune`, hot pixels, PRNU, gyro bias) are the only *un-enrolled* entropy, fuzzy, needing extraction. Neither is replaced by any number of serials.

### The enforcement rule (#0 and of MOST importance)

**The ira's meaning is enforced by the network, never by the device.** The TOKEN lives with the fleet. The fleet/custodes registry maps `device ira → owning handle`. Because the ira survives every reset, wipe, and update, the network can validate who a device belongs to long after any number of resets.

- The owner may tell the network to assign that ira/IMEI to a different handle — ownership transfer is owner→network, always.
- **The device itself NEVER assigns or reassigns its own ownership. Not by any means. Ever.** A wiped device does not get a vote on whether it is owned.
- Theft story: thief resets → flash is laundered but the ira is unchanged → registry still says "belongs to handle Y" → no pristine attestable phone. FRP's shape — permanent hardware ID plus off-device ownership record — but sovereign, on the mesh, no Google in the loop.
- The Titan attestation key hardens the claim: the network can challenge the device to *prove* it is physically the hardware behind the asserted ira (sign-only, permanent, survives reset). Impersonating husky's ira from other hardware then requires Titan's key, not just the public IMEI string.

### The warrant analysis, and the pin-the-leaf rule

A warrant compels everything Google knows: IMEI, serials, chip-ID, carrier linkage, and the attestation *public* key with its certificates. So the interim ira is warrant-transparent (identification and linkage forever), and since the interim AnchorKey derives from the same identifiers, vault confidentiality falls to warrant + physical seizure together — the known interim gap PIPE closes. What a warrant cannot compel, at Google's word, is the attestation *private* key: generated in-chip at manufacture, never leaves, Google holds only the public half. Google can name the device but cannot be the device — the one warrant-proof property husky has.

That property survives contact with a warrant only under one rule: **the fleet pins the device's leaf public key at registration, and the pinned leaf is the verification floor.** Google's CA chain is kept and layered *on top* as a second, independent signal — "Google vouches this is genuine Pixel silicon," useful against counterfeit or emulated hardware, and informative when it fails — but chain validity alone is never sufficient, because a warrant could compel Google, as CA, to issue a valid-chaining certificate for a key Google controls. Pinning at the owner's trusted moment (phone in hand, Phase 2) collapses the load-bearing trust to the single claim "this private key exists only in this chip." Residual trust at Google's word: no second copy, no signing oracle, and Titan firmware is Google-updatable — but updates flow through Android, which ferros-as-gatekeeper never boots. Producing a signature requires the keystore2→citadeld stack, so possession proofs are mintable only during the chainload era unless ferros ever drives `/dev/gsc0` itself. Bonus credential, genuinely warrant-resistant, never load-bearing — the TOKEN is.

### The physics leg: unenrolled sensor fingerprints

A third credential category, complementary to both above: per-device manufacturing variation read straight from the silicon — camera dark-frame fingerprints (hot pixels, PRNU fixed-pattern noise), gyro/accelerometer bias offsets, RTC/oscillator skew, panel mura/dead pixels, mic/speaker frequency response, flash-cell latency patterns. These are warrant-proof for the opposite reason the Titan key is: not "Google designed itself out" but "Google never had it" — the values were never enrolled, never transmitted, never existed off-device. Mixing a few stably-extracted bits into the interim ira derivation makes the **AnchorKey non-derivable from a warrant alone**: confidentiality then requires physical seizure, not paperwork.

Caveats: the readings are fuzzy (hot pixels accumulate with age, PRNU is noisy per-shot), so they need fuzzy extraction with error-correcting helper data — the helper data is stored ira-encrypted and leaks a small amount by construction. They are readable by anyone holding the device, so they are a nobody-knows ira *ingredient*, not a possession proof — they cannot sign a fleet challenge. Interim reads go through the chainload crutch (bare-metal camera is far off). The full composition is three-legged, each leg covering the others' failures: identifiers (permanent, Google-known) name the device; the pinned Titan leaf (chain layered on top) proves possession remotely; sensor physics (unenrolled) keeps the ira partly secret from paperwork.

## The wairua on husky

Per the GLOSSARY interim row: SoC TRNG read at boot into a volatile register. Husky provides this today — the SMCCC TRNG call is standard SMC and already proven callable from bare-metal ferros. First thing after boot, draw the wairua; it lives in registers per [KEY_REGISTERS.md](KEY_REGISTERS.md) (session working keys are wairua-derived and session-scoped; freeze IS the destruct because the wairua never touches durable medium). One precision: ARMv9 has no dedicated architectural key registers — the crypto-extension instructions (AES/SHA) operate on the ordinary NEON V-registers, so "the crypto registers" ARE designated V-regs, held under the KEY_REGISTERS.md protocol (inverted-pair guard, cleared at session end). It has no role in vault persistence and this document mentions it only to keep the classes straight.

## The AnchorKey

### Two different keys, don't conflate

- **Authenticity key (seed, Ed25519, public).** Baked into `ferros_seed` (`.rodata.pubkey`), answers "is this the real ferros kernel." Built and working.
- **Confidentiality key (AnchorKey, 32-byte symmetric secret).** HMAC of the anchor ring; root of object/index encryption and keyed addressing per [VAULT-INDEX.md](VAULT-INDEX.md). Currently the `[G#5A; 32]` test constant.

### Derivation (passless, ira-rooted)

```
anchor_key = BLAKE3-derive_key("ferros.vault.anchor.v0", ira)
```

No passphrase anywhere — authentication is recognition + possession, and the possession factor is (endgame) the TOKEN, not anything typed. No Argon2 — memory-hard KDFs defend low-entropy human secrets, and a passless system has none; the memory-hard proof ferros *does* want is spaghettify at the handle layer, a different job.

Properties: permanent (survives factory reset alongside the ferros partition it protects), device-derived, boot-derivable at EL2 with zero Linux/Google dependency (chip-ID is MMIO). Interim weakness, stated plainly: since the interim ira is derivable by anyone holding the phone, the interim AnchorKey confers *structural* confidentiality (opaque to Android/Google's stack, keyed addressing, no cross-device convergence) but not confidentiality against a determined physical attacker. That gap is closed by the endgame ira being an actual secret (PIPE OTP), not by making the interim key mortal. Meanwhile the real interim protections are ferros-as-sole-gatekeeper plus the fleet registry above.

### The release share: the zeroable key lives off-device

Two facts force one design. First, **flash cannot be trusted to forget**: UFS wear-leveling remaps LBAs across rotating physical blocks, so overwriting a key's address does not erase the cells that held it — stale copies survive in remapped blocks, and purge/sanitize commands are the FTL vendor's word. Registers forget perfectly but die at power-off; Titan is Google's. Husky has no owner-controlled local store that is both persistent and reliably zeroable (Apple builds dedicated effaceable storage for exactly this; we don't get one). Second, remote-killing a *device* is best-effort — a seized phone is in a Faraday bag before the kill command can land.

Both are solved by putting the zeroable key **off the device**: a release share held by the fleet, or Shamir K-of-N across friends (Shamir, consistent with custodes — not OPRF). You hold the safe, Bob has the key.

```
domain_key = BLAKE3-derive_key("ferros.vault.domain.<name>", ira ‖ release_share)
```

- The share has **ira-class lifetime at its home** (the fleet persists it durably) and **wairua-class residency on the phone** (fetched over the wire at unlock, registers only, never persisted). The two-key-class taxonomy holds per location; no key ever enters the data class.
- **The killswitch reverses direction**: don't push death to the device, withhold life from it. Zeroing the share at home kills the share-gated tier everywhere, permanently, including on a phone offline since the moment of seizure — the phone never held the whole key.
- Warrant math: Google has neither half. Seizing the owner yields the safe without the key; compelling Bob yields a key to a safe he never had — and Bob is K-of-N across households or jurisdictions, each free to refuse under duress, which puts human judgment in the release path.
- Cost: the share-gated tier needs network at unlock. So **the vault tiers**: an ira-only domain opens offline (music, maps — the phone stays usable in airplane mode), a share-gated domain protects what matters (messages, keys, ledger). Per-domain availability-vs-seizure-resistance is a policy knob, not a global compromise.
- This is the software interim of the TOKEN itself: endgame, Bob's key moves into the owner's pocket as separate silicon — PIPE over one wire instead of the fleet over radio, same derivation, same safe-and-key split. It also raises the radio stack's priority: the share-gated tier is only as usable as the link that fetches the share.

### The effaceable salt: local zeroable ingredient (candidate hunt)

The tiering above exposes a gap: the ira is permanent by design (criterion 0), so `derive(ira)` is forever recomputable and **the ira-only tier can never be crypto-erased** — and physically overwriting the data blocks is defeated by the same FTL rotation. Every tier needs an effaceable ingredient somewhere: the share-gated tier has Bob; the local tier needs an **effaceable salt** — `local_key = BLAKE3-derive_key("ferros.vault.local.v0", ira ‖ salt)`; efface the salt (per the erase operation below) and the tier is dead. This is Apple's effaceable-storage/media-key design without their dedicated silicon.

**The one selection criterion: in-place erase.** The salt store must sit on a medium where an overwrite physically destroys the old bits — EEPROM, a PMU register, a SIM EF — not flash, where the FTL rotates the write to a fresh block and leaves the old value in a remapped one. This is the whole point and the only requirement; removability, capacity, and secrecy are all secondary or irrelevant. The trust involved is bounded and honest: an in-place erase trusts the part's controller (PMIC, UICC/modem vendor) that the erase is real — a smaller, more auditable trust than flash, where we do not have to trust anything because we already *know* the FTL does not erase. "Trust the vendor's erase" strictly beats "know the erase is fake."

**The erase operation: overwrite with fresh random data, 3 passes — never a fixed pattern.** A known pattern (all-zeros) is subtractable: a lab that knows the overwrite value can subtract it and recover the prior state from partial-erase remanence (floating-gate residual charge, hysteresis). Random passes make overwrite-entropy indistinguishable from residual-signal and saturate the cell so less residual survives. This only works *because* the medium is in-place — 3 random passes on flash is theater, since the FTL scatters each pass to a fresh block while the original bits sit untouched in a remapped one; the multi-pass-random discipline and the in-place requirement are two halves of one thing. It matters here specifically because the salt is fully reconstructive — `local_key = derive(ira ‖ salt)` with a recomputable ira, so recovering the physical salt bits recovers the whole tier, with no crypto-erase safety net behind it. For 32 bytes the passes cost nothing. (Assumes the controller lands all passes on the same physical cell — the same bounded vendor trust as the erase itself.)

Husky inventory for "persists across reboots, in-place-erasable, no StrongBox":

- **DRAM** — no: measured dead across reset (chainload scratch experiments).
- **Main UFS** — persists, but the zero is a lie (FTL rotation).
- **RPMB** — textbook answer, locked to us: auth key fuse-derived, Google-provisioned at first boot, held by Trusty. StrongBox-adjacent borrowing.
- **PMU/PMIC alive-domain scratch registers (candidate 1).** INFORM-style registers (how reboot-reason survives warm reset). Zero = one MMIO write to an actual register — no FTL, a real zero. Survives warm reboot; unknown whether it survives full shutdown with the sealed battery attached — probe. Capacity tens of bytes; one salt is all that's needed.
- **Peripheral EEPROM / battery-backed RAM (candidate 2).** Fuel-gauge learned-parameter storage, NFC controller config EEPROM. EEPROM erases cells in place — no wear-leveling, an overwrite is physical. Survives battery pulls. Costs an I2C driver and care not to brick the peripheral; residual risk is decap-lab charge remanence only.
- **SIM card (candidate 3, optional — not an OOBE dependency).** UICC EEPROM writes cells in place — real zero, the selection criterion met — and as a bonus a physical SIM is the only *removable* component (eject and the salt leaves the device entirely), though removability is extra, not the point. Survives reboot, factory reset, battery drain. Two forms:
  - (a) **Raw salt in a writable EF, stock SIM, works today.** EF_ADN (phonebook) / EF_SMS are writable under CHV1 (the SIM PIN), and with SIM PIN disabled — the common case — the phone reads/writes them freely through the modem. ~250 records × ~28 bytes; a 32-byte salt fits trivially. No custom card, no ADM. But world-readable to anything with the (blank) PIN — fine for a salt (confidentiality was never its job), useless as a secure element.
  - (b) **Our own applet, custom card only.** A blank programmable SIM (known ADM keys) can carry a JavaCard applet — key held in-card, challenge-response out, rate-limiting, refusal — an owner-controlled secure element in a socket Google doesn't own; a slot-sized interim PIPE (host-asks-enclave-answers). Challenge-response beats raw storage because it never exposes the key to the modem bus.
  - **eSIM does neither for us.** Same UICC logically, but soldered (loses removability) and its security domains are the EUM's / carrier's (no applet, no owner-writable scratch) — it links the modem and nothing more.
  - Caveat for both: on Pixel the SIM wires to the *modem*, so every APDU transits proprietary modem firmware — bare-metal access rides the cellular stack (interim: AT/QMI via the chainload crutch).

**OOBE constraint (design lock): the local effaceable salt must not depend on a physical SIM.** Default it to an on-board store (candidate 1 PMU scratch, or candidate 2 peripheral EEPROM) so the out-of-box experience works with zero SIM inserted. Physical SIM (a/b) and NFC tags (external tap-to-unlock salt carriers) are *optional owner add-ons*, never requirements. The real owner-controlled secure element is **PIPE**, not any SIM — every SIM path is an interim half-measure that trades away removability, confidentiality, or key ownership.
  - **SIM OTA bonus:** physical SIMs accept remote management via binary SMS — secured APDUs authenticated with OTA keys held by the card issuer (the Simjacker surface, proof the channel works). On our own programmable card WE hold the OTA keys, so the fleet can zero or rotate the salt with a single SMS — no data connection, ri

Scope honestly: with an unlocked bootloader, anything our kernel reads, a thief's fastboot-booted kernel reads. The salt's job is not confidentiality-in-hand (nothing local gets that; that is the share's job) — it is making **erase** real: instant, offline, no network, no FTL trust. Panic-wipe that actually wipes.

**Probe plan (next phone-in-hand session):** from the chainloaded kernel or rooted adb, enumerate PMU scratch registers and I2C peripheral storage, write a pattern, cycle warm reboot → full shutdown → battery drain, record what sticks and what zeroes. For the SIM: confirm EF write access via AT/QMI on the current card, and order blank programmable SIMs (known ADM keys) for the applet path.

### Phased map

**Phase 0 — now.** Chainload, `[G#5A; 32]` constant. Fine for persistence proofs, not a real key.

**Phase 1 — ira-derived AnchorKey.** Read chip-ID at MMIO `G#1000_0000` in the kernel (bare-metal, no chainload fetch, no keystore stack), derive the interim ira and the AnchorKey, open the vault. Fold in UFS serial (already have the device descriptor path) for a second permanent ingredient. Simpler than every previous plan — no Linux-as-key-fetcher required.

**Phase 2 — fleet registration.** When the mesh/custodes exists: register husky's ira (identifiers + Titan attestation pubkey) under the owner's handle. From then on the anti-theft property is live — resets no longer launder the device.

**Phase 3 — PIPE (endgame).** The TOKEN's OTP key is TRNG-born at KOHI, sealed at TAHU, never leaves, and its ragu output feeds `ferros_vault::anchor` directly — the same derivation with a true-secret ira swapped in. Only candidate scoring on all four criteria including MINT, and it moves possession off the phone: a thief holding the phone still lacks the TOKEN; custodes (hardened Shamir, not OPRF) recovers an owner who loses everything.

## What this kills from earlier drafts

- Passphrase/PIN phases, Argon2, Weaver-as-mixer — passless, gone.
- "Display gates the vault because we need PIN entry" — display is still next, but for a debug console, not a PIN pad.
- **Titan StrongBox HMAC as anchor root** — measured deterministic and nobody-knows, but wiped on factory reset: wrong lifetime class for an ira-rooted key, and it would let an Android-side reset destroy a vault whose ciphertext survives on sda35. Kept only as a data point; its permanent sibling (the attestation key) serves challenge-response instead.
- The "third key class" ("survives reboot, dies on reset") — resolved, not just banished: that lifetime is key-material-stored-as-data. The classes are two key classes (*ira*, *wairua*) plus flash as the data class; every key belongs to exactly one key class and no key ever lives in the data class.
- "Reset destroying the ira is arguably correct" — the worst error of the thread. Nothing ira-rooted dies on reset, ever; anti-theft is the fleet registry + TOKEN, not key suicide.

## Near-term concrete step

Phase 1 in the kernel: read chip-ID via MMIO at `G#1000_0000` (gs-chipid layout), derive `ira = BLAKE3-derive_key("ferros.ira.husky.v0", chip_id)` and `anchor_key = BLAKE3-derive_key("ferros.vault.anchor.v0", ira)`, replace the `[G#5A; 32]` constant in the shim, reseal, and prove reopen-across-reboot with the derived key. Host-testable first by injecting a fake chip-ID through the same derivation path.
