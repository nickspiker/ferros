# Wāteo on ferros: the destination

Wāteo (wateo.net) is data-only service at cost, run by its members, tied to the person and not the SIM ("Claude memories/wateo_net.md", "Claude memories/mvno_data_only.md").
This document fixes where ferros is going with it, so what is built before real service exists does not have to be redesigned when it arrives (owner, 2026-11-13: "I just want to make sure we have the destination defined so we don't fuck something up now and we'll need to redesign it again when we hook up real service").

Each part says **Decided** (the owner's words), **Destination** (proposed, the shape everything now must fit), or **Open** (the owner's call).

## The requirement

**Decided** (owner, 2026-11-13): "We can't have Android devices using data, has to be ferros only. How do we make sure the community can flash ferros, then use their identity to pay for their service?"

So: Wāteo's data reaches a person only thru ferros, on a device they attested on, paid for by their identity.
Android on the same device, a SIM moved to another device, or a device nobody attested on gets nothing.

**Decided** (owner, 2026-11-17): "We definitely do not want any physical SIMs ever, that defeats the entire purpose of the device being the key."
So every Wāteo line is a profile downloaded into the device's own embedded eUICC; no card, no removable part, ever.

## The SIM opens the door; the identity is the key

**Destination.** Keeping Android away from the eUICC cannot be the lock.
Under Android the modem is Android's, and its eSIM manager can enable any profile on the chip; the hv can fence DMA but cannot make Android's telephony stack forget a profile.
So the lock is on the network side, where nothing on the device can lift it:

- **A Wāteo profile reaches one place only.** The sponsor carrier's APN for Wāteo profiles is private: it routes to Wāteo's gateways and nowhere else, with no internet, no DNS, no carrier services. Attaching with the profile gets a device an address that can reach the gateways, and nothing else.
- **Data flows only in a tunnel to a gateway.** ferros opens it after attest ("The connection" below). Its credential is **the person's Wāteo key**: derived by the kernel from the person key (FACETS.md "The alias"), as a kist's alias is but for the system's own use, so no kist ever holds it: `derive_key("ferros 2026-11-13 wateo v0", person key)`.
- **The device's key names the device; it proves nothing.** The tunnel also carries the device's ira key, for accounting (which device used what). The ira is the device's one ID on ferros, a plate with no papers (owner, 2026-11-13: "on android, linux, mac and windows it's plate AND papers. ferros needs no papers as long as you keep android non rooted or hypervisored. ideally both"): its sources are chip identifiers that only the person's own rooted Android could scrape, which is the person harming themself. Against a stranger it holds while the bootloader is re-locked to the ferros key (the flasher's lock-prep, the person's own lock); unlocked, anyone holding the device boots their own image outside the hv and derives it. So nothing at the gateway rests on it.
- **What Android sees**: it can enable the profile and attach; without the person's Wāteo key no gateway opens a tunnel; no data passes and nothing is billed.
- **What a moved profile sees**: the same. A Wāteo profile on any device without the person's Wāteo key is a dead line.
- **What the person can do, deliberately** (owner, 2026-11-13: "I don't think we should restrict. If they want to hackjob android to get access to their own service, plus we can't really stop them without fucking up our open model"): a rooted Android of theirs that joins their fleet (approved on a fleet device) can open the person key and derive the Wāteo key, and use their service. That is the person's choice, paid by them; Wāteo does not try to tell ferros from anything else, which would need a vendor's hardware attestation.

The gateways see the Wāteo key and the device key, never the handle, the identity key, or any kist's alias.

## The profile is a pool's; the service is the person's

**Destination.**
- Wāteo holds a pool of eSIM profiles with its sponsor carriers (a Hologram account today: Verizon, AT&T, T-Mobile; "Claude memories/wateo_net.md"). A profile belongs to no member.
- A ferros device takes a profile from the pool once, thru ferros's own SGP.22 client after attest (the one place web encodings are allowed ferros-side, "Claude memories/feedback_no_web_encodings.md"), and keeps it for the device's life.
- Whoever attests on that device is whose service it is: pick up any device of yours, attest, and you are online as you, billed to you. The profile never moves with the person; the keys do.
- **The first download needs another way in.** SGP.22 fetches a profile from the supplier's SM-DP+ over the internet, so the first join comes over WiFi (or any other link) once; after that the device keeps its profiles and needs nothing but the cell.
- **This rests on the supplier issuing SGP.22 downloadable profiles to a phone's eUICC** (consumer remote provisioning: an activation code, or an SM-DP+ address and matching ID, per profile). A supplier whose eSIMs live only on its own cards, or whose profiles it switches from its servers (SGP.02, SGP.32), does not fit: see W4.

### The carrier switch

The hui of 2026-11-11 settled that members pick their carrier (AT&T, Verizon or T-Mobile) and switch with a button.

**Destination.**
- The choice is a person setting (Verizon, AT&T, T-Mobile, or auto), kept in the identity space, so it follows the person to every device they attest on (settings flow identity to device: "Claude memories/settings_identity_to_device_only.md").
- The switch happens on the device, with no server at switch time, in one of two ways, by what the supplier's profiles are (W5):
  - one profile that attaches to all three networks: the switch is the modem's network selection pinned to that carrier's PLMNs, seconds, nothing downloaded;
  - one profile per carrier: all three downloaded once at join, and the switch is enabling one (SGP.22 ES10c), the modem refreshing the SIM and attaching again in tens of seconds, offline.
- The tunnel does not change with the carrier: it is keyed by the person, so the gateways see the same member over any carrier; only the network under it changes.
- The cellular circle shows each carrier's live measures (signal, rate, loss: "Claude memories/link_measures_no_bars.md"), so the person sees what they switch to; "auto" may later choose by those same measures, from the modem's own reports, never polling.

## The connection

**Destination** (2026-11-17, owner: "Assume Wateo is just like another thing they bind to their identity, just like Photon").
No gateway asks a central server anything to forward a packet.

### The account

- Wāteo is bound to the identity the way a kist is: the Wāteo key above is its alias, and the fleet signs a binding for it as for a kist (FACETS.md "be you").
- **The account is the Wāteo key's public half.** Wāteo's ledger knows that key and its standing (paid up, credit, limits), never the handle or the identity key; it is unlinkable to every kist the person uses.

### Standing travels as a ticket

- The ledger signs a short-lived **ticket**: this Wāteo key, valid until a time, this tier or limit.
- While the person is in good standing the device renews it in the background, before it lapses.
- **A gateway holds no account list**, only the ledger's public key: it checks a ticket's signature and expiry locally, so any number of gateways at any number of places behave alike.
- **Cut-off**: when credit runs out the ledger stops renewing and tickets lapse on their own. For a cut-off at once (abuse, a device reported taken) the ledger pushes a short revocation list to every gateway: a push, never a poll.

### A tunnel

1. The device sends a hello carrying its ticket.
2. It proves it holds the Wāteo key by signing the gateway's challenge.
3. Both derive the session's keys from a hybrid exchange: X25519 and a post-quantum KEM.
4. Packets flow sealed with XChaCha20-Poly1305.

The gateway learns the Wāteo key and the device key, and nothing else.

### Many gateways, always connected

- The device holds a short list of gateways (those the private APN reaches) and the ledger's key.
- It keeps one tunnel up and a warm standby to a second gateway, kept alive cheaply; when the first dies it moves within a round trip.
- Each gateway hands out a **resumption ticket**, so a move to another gateway, or a return after the radio dropped, needs no full handshake.
- A gateway keeps only its live sessions; accounts live in the tickets.

### Metering

- Each gateway counts bytes per Wāteo key.
- The device and the gateway co-sign a **usage receipt** from time to time, so neither can inflate or hide traffic.
- The ledger collects receipts and debits credit: credit in, receipts out.

### The frames

**Decided** (owner, 2026-11-17): "the VSF rule doesn't apply to framing, in fact, if it's encrypted there's no need to frame since the chance of getting a shitttonnnn of 0's or 1's in a row is basically impossible when crypted, thus send it as lean and mean as you can. VSF is for the full meal deal once it comes out the other side fully assembled."
So:
- **The control plane is VSF**: the hello, tickets, resumption tickets, receipts, revocations, each a complete VSF file once assembled.
- **The data plane is lean**: a tunneled packet goes as the least the receiver needs to open it (a counter for the nonce and the replay window, the ciphertext, the tag), nothing more.

## Where the gateways sit

**Destination.**
- A phone's data does not leave at the nearest tower: the carrier tunnels it to a packet gateway in the mobile core (the P-GW or UPF, the anchor), and with a supplier like Hologram that anchor is in the supplier's core. Only there does the private APN hand traffic to Wāteo.
- So the route is decided by where the supplier's anchors are, not by where Wāteo's gateways are: Wāteo's gateways sit beside the supplier's breakout points, joined by a private link (IPsec, or a cloud interconnect where the supplier offers one), two or more at each for failover.
- A gateway elsewhere shortens nothing: the traffic has already passed the supplier's anchor.
- **Egress costs**: cloud providers charge for traffic leaving their network, a cost of the same order as the data itself and doubled when traffic comes in and goes out again. A colocated machine with flat-rate bandwidth beside the breakout point, or a provider with cheap egress, is to be priced against a cloud (W6).

## Membership and paying

**Decided** (owner, 2026-11-09): no dollars; payment in crypto; fiat holders swap thru an onboarding provider.

**Destination.**
- **Membership** is a record in the person's identity space (IDENTITY-SPACE.md), signed by Wāteo's association (the DUNA, once formed: "Claude memories/fgtw_duna_runanga.md"), naming the person's Wāteo public key and the device keys allowed to use it (Open: one device at a time or several, W1).
- **Paying** credits the membership; the ledger renews tickets while credit holds, and receipts debit it at cost.
- **Joining**, invite only for now: an existing member's sponsorship and a first payment, done in a ferros settings page (the system's, never a kist's), never on a website.
- The association signs memberships; it never learns the handle. A member's Wāteo key unlinks them from every kist they use.
- At cost includes paying the people who build and run it (the hui of 2026-11-11), and the supplier bills Wāteo in fiat while members pay in crypto: who converts, and how pay is set and approved, are open (W7, with the legal work).

## While locked

**Decided** (2026-11-04, "Claude memories/radios_at_attest_gps_locked.md"): radios come up at attest; while locked only the signed locate may act on radio input, and LTE carries the locate's answer.

**Destination.** A locked device has no person key in the kernel, so it cannot open a person's tunnel.
The gateways take a second, narrow kind: a **device tunnel**, which reaches only fgtw.org's locate responder and nothing else, metered to the membership that names that device.
It cannot be opened with the ira key: anything that regenerates the ira could then open it and answer a locate with a false place.
Instead, at each attest ferros derives a **locate key** from the wairua (fresh each power-on, in registers only) and has the person's Wāteo key sign it, valid until the device next loses power; the ledger issues a narrow ticket for it (the locate responder only), and while locked the device tunnel opens with that key and ticket alone.
The trade: a device power-cycled while locked cannot answer a locate until its person attests again; a false place is worse than none.
That keeps locate working on Wāteo without letting a locked device, or Android, use the line.

## Getting the community onto ferros

**Destination** (the flasher, "Claude memories/ferros_flasher_tool.md"):
0. A host tool unlocks the bootloader, flashes ferros signed with the ferros commons key, writes the gate, and prepares the re-lock; the re-lock itself is the person's own act ("Claude memories/feedback_lock_prep_not_lock.md").
1. First boot: mint or attest an identity. Android stays off unless the person turns it on after attest (ANDROID-behind-attestation, "Claude memories/android_behind_attestation.md").
2. Join Wāteo on the device, over WiFi the first time: the profile downloads, the tunnel opens, data flows.

Which devices the community can use is which devices can be unlocked, flashed and re-locked: Pixels today.

## Rules for what is built before real service

These are what today's code must not break, so real service slots in:
- **No physical SIM in any Wāteo path.**
- **No data path that bypasses the tunnel once a Wāteo profile is active.** The modem driver and `ip` may use a carrier's open APN for a person's own SIM (bring your own SIM stays: README "Connectivity"), but a Wāteo profile's traffic goes only to the gateways.
- **Nothing keys service to the SIM, the ICCID, the IMEI or the EID.** Service is the person's keys; the profile is a pool's.
- **The modem stays ferros's.** Android runs only after a ferros attest (the gate's rule); nothing on ferros hands Android a Wāteo credential or the Wāteo key.
- **Nothing of a Hologram account (logins, ICCIDs, IMEIs, EIDs, the organisation) goes in the repo.**
- **No dollars anywhere**, in code, docs or pages.

## What to build, in order

0. The person key and the kernel's derivation (FACETS.md; built, on the Pro since 2026-11-16); the Wāteo key beside the kist alias, for the system only, and its binding.
1. **Reaching the eUICC**: the SGP.22 client speaks to the eSIM chip thru the modem's SIM interface (logical channels, APDUs), over the Samsung modem's closed protocol. This is ferros's own biggest unknown in the whole path, and it comes first.
2. The SGP.22 client: profile download, enable, disable, delete; profiles kept per device (rustls in one std process, trusting only the GSMA CI root).
3. The modem driver taking its APN and network selection from the active profile and the person's carrier setting (today: Verizon's table, RADIO-CONTROLS.md).
4. The tunnel in `ip`: the handshake, the ticket, the standby and resumption, receipts, lean data frames, the person tunnel and the device tunnel, and the rule that a Wāteo profile's traffic goes nowhere else.
5. The membership record (IDENTITY-SPACE.md) and the Wāteo page in settings, with the carrier choice.
6. Wāteo's side, not ferros's: the gateways, the ledger (tickets, receipts, the revocation push), the private APN with each sponsor, the association's signing.

## Open

- **W0. The tunnel's protocol**: proposed above, Wāteo's own small one (ticket-based, a hybrid post-quantum handshake, lean frames), rather than Photon Transport or WireGuard as they are.
- **W1. One device or several**: whether a member's service runs on one device at a time or on every device they attest on at once.
- **W2. The gateways**: run by Wāteo as its own service, or riding FGTW's network (FGTW's peers, "Claude memories/fgtw_duna_runanga.md"); either way placed beside the supplier's breakout points.
- **W3. Bring your own SIM**: kept as today (any carrier, the person's business) beside Wāteo, or Wāteo only on ferros devices.
- **W4. The supplier and SGP.22** (blocks build step 2): does Hologram issue SGP.22 downloadable profiles to a phone's eUICC, or only to its own cards and servers? If not, another supplier (an eSIM wholesaler issuing consumer profiles on the same carriers).
- **W5. The profiles' shape**: one profile reaching all three networks with the device choosing the network, or one profile per carrier; and whether a profile downloaded but disabled costs anything.
- **W6. Hosting**: where the supplier's packet gateways are, whether it offers cloud interconnect or only IPsec, and cloud against colocation by egress cost.
- **W7. Money's edges**: who turns members' crypto into the supplier's fiat, and how pay for the people who build and run it is set and approved.
