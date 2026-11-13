# Wāteo on ferros: the destination

Wāteo (wateo.net) is data-only service at cost, run by its members, tied to the person and not the SIM ("Claude memories/wateo_net.md", "Claude memories/mvno_data_only.md").
This document fixes where ferros is going with it, so what is built before real service exists does not have to be redesigned when it arrives (owner, 2026-11-13: "I just want to make sure we have the destination defined so we don't fuck something up now and we'll need to redesign it again when we hook up real service").

Each part says **Decided** (the owner's words), **Destination** (proposed, the shape everything now must fit), or **Open** (the owner's call).

## The requirement

**Decided** (owner, 2026-11-13): "We can't have Android devices using data, has to be ferros only. How do we make sure the community can flash ferros, then use their identity to pay for their service?"

So: Wāteo's data reaches a person only thru ferros, on a device they attested on, paid for by their identity.
Android on the same device, a SIM moved to another device, or a device nobody attested on gets nothing.

## The SIM opens the door; the identity is the key

**Destination.** Keeping Android away from the eUICC cannot be the lock.
Under Android the modem is Android's, and its eSIM manager can enable any profile on the chip; the hv can fence DMA but cannot make Android's telephony stack forget a profile.
So the lock is on the network side, where nothing on the device can lift it:

- **A Wāteo profile reaches one place only.** The sponsor carrier's APN for Wāteo profiles is private: it routes to Wāteo's gateway and nowhere else, with no internet, no DNS, no carrier services. Attaching with the profile gets a device an address that can reach the gateway, and nothing else.
- **Data flows only in a tunnel to the gateway.** ferros opens it after attest. The tunnel's one credential is **the person's Wāteo key**: derived by the kernel from the person key (FACETS.md "The alias"), as a kist's alias is but for the system's own use, so no kist ever holds it: `derive_key("ferros 2026-11-13 wateo v0", person key)`.
- **The device's key names the device; it proves nothing.** The tunnel also carries the device's ira key, for accounting (which device used what). The ira is the device's one ID on ferros, a plate with no papers (owner, 2026-11-13: "on android, linux, mac and windows it's plate AND papers. ferros needs no papers as long as you keep android non rooted or hypervisored. ideally both"): its sources are chip identifiers that only the person's own rooted Android could scrape, which is the person harming themself. Against a stranger it holds while the bootloader is re-locked to the ferros key (the flasher's lock-prep, the person's own lock); unlocked, anyone holding the device boots their own image outside the hv and derives it. So nothing at the gateway rests on it.
- **What Android sees**: it can enable the profile and attach; without the person's Wāteo key the gateway refuses the tunnel; no data passes and nothing is billed.
- **What a moved SIM sees**: the same. A Wāteo profile in any device without the person's Wāteo key is a dead line.
- **What the person can do, deliberately** (owner, 2026-11-13: "I don't think we should restrict. If they want to hackjob android to get access to their own service, plus we can't really stop them without fucking up our open model"): a rooted Android of theirs that joins their fleet (approved on a fleet device) can open the person key and derive the Wāteo key, and use their service. That is the person's choice, paid by them; Wāteo does not try to tell ferros from anything else, which would need a vendor's hardware attestation.

The gateway sees the Wāteo key and the device key, never the handle, the identity key, or any kist's alias.

## The SIM is a pool's; the service is the person's

**Destination.**
- Wāteo holds a pool of eSIM profiles with its sponsor carriers (a Hologram IoT account today: Verizon, AT&T, T-Mobile; "Claude memories/wateo_net.md"). A profile belongs to no member.
- A ferros device takes a profile from the pool once, thru ferros's own SGP.22 client after attest (the one place web encodings are allowed ferros-side, "Claude memories/feedback_no_web_encodings.md"), and keeps it for the device's life.
- Whoever attests on that device is whose service it is: pick up any device of yours, attest, and you are online as you, billed to you. The profile never moves with the person; the keys do.
- Carriers the person picks: the pool can hold profiles from several sponsors, and the device can switch profiles; which sponsor is a setting, the identity is the same.

## Membership and paying

**Decided** (owner, 2026-11-09): no dollars; payment in crypto; fiat holders swap thru an onboarding provider.

**Destination.**
- **Membership** is a record in the person's identity space (IDENTITY-SPACE.md), signed by Wāteo's association (the DUNA, once formed: "Claude memories/fgtw_duna_runanga.md"), naming the person's Wāteo public key and the device keys allowed to use it (Open: one device at a time or several).
- **Paying** credits the membership; the gateway checks membership and credit per tunnel, and meters use against the Wāteo key at cost.
- **Joining**, invite only for now: an existing member's sponsorship and a first payment, done in a ferros settings page (the system's, never a kist's), never on a website.
- The association signs memberships; it never learns the handle. A member's Wāteo key unlinks them from every kist they use.

## While locked

**Decided** (2026-11-04, "Claude memories/radios_at_attest_gps_locked.md"): radios come up at attest; while locked only the signed locate may act on radio input, and LTE carries the locate's answer.

**Destination.** A locked device has no person key in the kernel, so it cannot open a person's tunnel.
The gateway takes a second, narrow kind: a **device tunnel**, which reaches only fgtw.org's locate responder and nothing else, metered to the membership that names that device.
It cannot be opened with the ira key: anything that regenerates the ira could then open it and answer a locate with a false place.
Instead, at each attest ferros derives a **locate key** from the wairua (fresh each power-on, in registers only) and has the person's Wāteo key sign it, valid until the device next loses power; while locked, the device tunnel opens with that key alone.
The trade: a device power-cycled while locked cannot answer a locate until its person attests again; a false place is worse than none.
That keeps locate working on Wāteo without letting a locked device, or Android, use the line.

## Getting the community onto ferros

**Destination** (the flasher, "Claude memories/ferros_flasher_tool.md"):
0. A host tool unlocks the bootloader, flashes ferros signed with the ferros commons key, writes the gate, and prepares the re-lock; the re-lock itself is the person's own act ("Claude memories/feedback_lock_prep_not_lock.md").
1. First boot: mint or attest an identity. Android stays off unless the person turns it on after attest (ANDROID-behind-attestation, "Claude memories/android_behind_attestation.md").
2. Join Wāteo on the device: the profile downloads, the tunnel opens, data flows.

Which devices the community can use is which devices can be unlocked, flashed and re-locked: Pixels today.

## Rules for what is built before real service

These are what today's code must not break, so real service slots in:
- **No data path that bypasses the tunnel once a Wāteo profile is active.** The modem driver and `ip` may use a carrier's open APN for a person's own SIM (bring your own SIM stays: README "Connectivity"), but a Wāteo profile's traffic goes only to the gateway.
- **Nothing keys service to the SIM, the ICCID, the IMEI or the EID.** Service is the person's keys; the profile is a pool's.
- **The modem stays ferros's.** Android runs only after a ferros attest (the gate's rule); nothing on ferros hands Android a Wāteo credential or the Wāteo key.
- **Nothing of a Hologram account (logins, ICCIDs, IMEIs, EIDs) goes in the repo.**
- **No dollars anywhere**, in code, docs or pages.

## What to build, in order

0. The person key and the kernel's derivation (FACETS.md; built, not yet on a device); the Wāteo key beside the kist alias, for the system only.
1. The SGP.22 client: profile download, enable, disable, delete; profiles kept per device.
2. The modem driver taking its APN from the active profile (today: Verizon's table, RADIO-CONTROLS.md).
3. The tunnel in `ip` (Open: protocol), with the person tunnel and the device tunnel, and the rule that a Wāteo profile's traffic goes nowhere else.
4. The membership record (IDENTITY-SPACE.md) and the Wāteo page in settings.
5. Wāteo's side, not ferros's: the gateway and its metering, the private APN with each sponsor, the association's signing.

## Open

- **W0. The tunnel's protocol**: Photon Transport over UDP (ferros's own, already in `ip`'s neighbourhood) or a WireGuard-shaped one (well understood, existing gateway software).
- **W1. One device or several**: whether a member's service runs on one device at a time or on every device they attest on at once.
- **W2. The gateway**: run by Wāteo as its own service, or riding fgtw.org's network (FGTW's peers, "Claude memories/fgtw_duna_runanga.md").
- **W3. Bring your own SIM**: kept as today (any carrier, the person's business) beside Wāteo, or Wāteo only on ferros devices.
