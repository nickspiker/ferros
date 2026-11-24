# Kists device to device

Status: design for the owner, 2026-11-24; three of its questions answered the same day ("Decided" below). Nothing here is built; founding rules say a design goes to the owner first. The decisions it builds on are the owner's of 2026-11-24 (memory kist_distribution_p2p): "where do we host the list? fgtw.org bootstrap I suppose? and then with clients? This is part of the bittorrent like network for distributing. I'd presume if my desktop has a copy and a friend wants it, I can serve it directly rather than going to brobdingnagian."

What it covers: how kists and pucks spread from device to device the way BitTorrent spreads files, built on what exists: the directory on R2, the fleet's capsule source, Photon Transport, the stower's checks and the BLAKE3 tree ferros_kist already walks. Words: a kist's capsule is the developer's signed VSF file (KISTS.md "Trust on first stow"); a puck is a kist too, so everything here applies to pucks unchanged.

The shape in one paragraph: a capsule is named by its BLAKE3 and signed by its developer, so the bytes are as good from a friend's device as from fgtw.org. The list of what exists is a bag of small developer-signed entries; fgtw.org holds the first copy and devices hand newer entries to each other when they meet. A fetch asks the person's own devices first, then their people's, then the local network, then the web, takes pieces from several of them at once, and checks every piece against the capsule's BLAKE3 tree as it lands. A device serves only what it has stowed, keeps nothing extra, and serves strangers only when the person opted in.

## Decided (owner, 2026-11-24)

- **One entry form** (open question 0): "One entry format, yes." Every developer, Photon included, publishes the developer-signed entry of 1.0; the directory's own per-kist fields and the manifest pointer go.
- **A revoked key's kists** (open question 2): "revoked key, also yes." A stowed kist whose developer key is revoked keeps running as checked and sealed, is offered no update, and its card says its developer withdrew the key.
- **Hosting, per developer** (revises open question 11): "I'd put Lumis and Mahere on holdmyoscilloscope and make fgtw.org point there since those are mine. Photon and possibly some other system apps will be hosted at kists.fgtw.org since Photon is not mine."
  - Capsules signed with the owner's own developer key (Lumis, Mahere) live on the holdmyoscilloscope R2, served at brobdingnagian.holdmyoscilloscope.com over plain HTTP for ferros. fgtw.org's copy of their signed entries points there.
  - Commons-key kists (Photon, and possibly other system apps, signed with a project key: kist_keys_follow_ownership) are hosted in the `kists` bucket at kists.fgtw.org.
  - So the web rung is per developer, not one bucket: each entry carries where its capsule lives on the web (`web`, 1.0), and fgtw.org lists entries without holding every capsule.
  - **Photon's deploy changes** (the Photon session's to make, recorded here): it uploads the ferros capsule to the holdmyoscilloscope bucket today (`photon/v<n>/photon-messenger-ferros-arm64-release.vsf`); it should go to the `kists` bucket at kists.fgtw.org, with Photon's entry naming it there.
- Every other open question in section 7 is still open.

## 0. What exists today

**Built and running:**
- **The directory** (KISTS.md "The directory"): one VSF document at http://kists.fgtw.org/kists.vsf in the R2 bucket `kists`, built by tools/kistcat (`kistcat build`, the source list tools/kistcat/kists.sh), served over plain HTTP with no redirect; `/` is the page (the Worker in tools/kistcat/worker). It is published UNSIGNED (the ferros project key is not chosen) and lists one kist, an older Clock (read 2026-11-24: 618 bytes, one `kist` section, its `metrics` section empty).
- **Capsules by BLAKE3:** `/capsules/<blake3>.vsf` on the same bucket, exact key. Only the directory's own entries are put there, so the nine kists on the Pro's list answered 404 on 2026-11-23 (IDENTITY-SPACE.md "A reset, observed").
- **The manifest pointer:** a directory entry may name a developer's signed manifest (`manifest` nu, `platform`, `arch`) in place of a `path`; the device takes the artefact with that platform and arch.
- **Photon's ferros capsule:** every Photon release deploy (photon scripts/release/deploy.sh, since dda35be9 on 2026-11-10) builds the ferros capsule from the frozen snapshot, signs it with Photon's release key (scripts/lib/ferros-kist.sh, ferros tools/photon-capsule.sh), checks it (`photon-manifest --check-capsule`), uploads it to the `holdmyoscilloscope` bucket at `photon/v<n>/photon-messenger-ferros-arm64-release.vsf`, and names it in the signed release manifest's `ferros`/`arm64` row over plain http (photon docs/updates.md "ferros"). The live manifest (v156, read 2026-11-24) carries that row.
- **Photon's peer-to-peer release notice** (photon docs/updates.md "BUILT (2026-11-19)"): every sealed pong carries a claim of the newest release manifest the device holds (`rel`), a device that claims less is handed the manifest itself (`release_tell`), byte for byte, and the receiver verifies its eggs alone. This is the model for passing the list (section 1).
- **The stower's checks** (payloads/kists; KISTS.md "The stower", "Stowing"): the capsule hashed as it streams (ferros_kist::fetch `Intake`), its BLAKE3 and size against what was asked, every egg against the developer bundle it carries and the person's pin for that developer (`capsule::admit`, trust on first stow), the last piece held back until those pass, then the kernel's stow mode (the gate where it lies, the checker over its masked code) and the seal. Every source goes thru these, whatever carried the bytes.
- **The BLAKE3 tree in ferros_kist** (ferros_kist/src/twin.rs, 2b5956ef): a capsule hashed in leaves of 2^16 bytes, each leaf's chaining value kept, the root merged by `hazmat::left_subtree_len` exactly as `blake3::hash` does it, and the signature's hash (the slot read as zeros) from the same leaves. This is the piece tree of section 2, already written and tested.
- **Photon Transport over UDP** (photon docs/PT.md; photon_core::pt, which payloads/fgtw speaks on 4383): VSF control packets (SPEC, ACK, NAK, COMPLETE), lean DATA packets (`[stream][seq][payload]`), a BLAKE3 per chunk in each ACK and one over the whole transfer. The sender blasts only after the receiver's SPEC ACK. ferros_pt is the same protocol for USB (512-byte chunks).
- **The local network:** fgtw beacons to Photon's multicast groups and the subnet broadcast on 4384 and hears the person's other devices there (payloads/fgtw main.rs `beacon`, members heard in the last two minutes). Bluetooth carries beacons, no data. ferros has no WAN peer path of its own (feedback_fgtw_devices_first).
- **The ip process** (payloads/ip): WiFi and the cellular call both up, a packet leaves on WiFi when WiFi holds an address of its family, else on cellular; `LINK` sets the mode for the whole device, not per send.

**Built, not yet run on two devices:**
- **The fleet capsule source** (IDENTITY-SPACE.md "The fleet source"; 2b53c2bb; frames in the fgtw crate, protocol.rs `capsule_want`, `capsule_piece`, `capsule_none`). The stower asks fgtw (op 38) for a piece of 2^16 bytes at an offset; fgtw sends a signed `capsule_want` over PT to the person's devices heard, the one that served the previous piece alone after the first, and holds the call (`capsule::ASK_US`, 2^23 µs) for a piece, every device out, or the time. The answering stower holds op 39, reads `capsules.<blake3>` and hands the piece back with op 40. Pieces come in order, one round trip each (about A#1600 for Photon's 106 MB capsule), into the same `Intake` as an HTTP fetch. The restore tries the fleet, then the kist's recorded `source`, then the directory's entry, then kists.fgtw.org by BLAKE3.

**Missing:**
- every stowable capsule on its developer's web host (the restore has nowhere else to go when the fleet is silent): the owner's own kists on holdmyoscilloscope, commons kists on kists.fgtw.org ("Decided" above); Photon's ferros capsule is on holdmyoscilloscope today and moves to kists.fgtw.org (Photon's deploy);
- the stower reading a manifest entry (Photon's manifest writes `platform` and `arch` as `x`, which a no_std stower cannot decode: KISTS.md "Open"), so Photon is not in the directory;
- a developer-signed entry of its own (today the directory's entries are written and vouched for by whoever runs kistcat);
- a developer's key hand-off or revocation (KISTS.md "Whose key signs a kist", the custody gap);
- contacts as a source, the local network beyond the fleet, several sources at once, verification per piece, resuming;
- any serving policy beyond "a member of the same fleet";
- a per-send choice of link in ip (to serve on WiFi only).

## 1. The list

### 1.0 What an entry is

One kist version, as its developer states it: a complete VSF file signed by the developer with every egg (Ed25519, Falcon-512, SPHINCS+: `fgtw::release::release_slots`, `sign_release`, the release mask, as a capsule is), its header naming the developer's Ed25519 key (`ke`), stamped with Eagle time, its provenance `hp`. One section `kist_entry`:
- `option` (`hR`): with the developer key, the kist's identity;
- `name` (`a`), `developer_name` (`a`, optional);
- `version` (`z`, one value per part, as written: kist_version_stages);
- `capsule` (`hb`, the capsule's BLAKE3) and `size` (`u`);
- `kb`: the developer's public bundle (`ke`, `kvf`, `kvs`, read by type marker, as the capsule's), so the entry verifies alone, with no capsule in hand;
- `about` (`a`), `icon` (`hb`, the icon's own BLAKE3, fetched like a capsule; today a path);
- `web` (`nu`, one or more): plain-http URLs where the developer hosts the capsule, the web rung of the fetch (decided 2026-11-24: per developer). The owner's own kists name brobdingnagian.holdmyoscilloscope.com, commons kists name kists.fgtw.org. Where the bytes come from is never trusted: they are checked against the entry's BLAKE3 and the capsule's own signature, as from any peer.

About 10 KB, most of it the SPHINCS+ egg (7856 bytes). `kistcat kist` writes it beside the capsule in the same run with the same key (`--entry <out>`); Photon's deploy writes one thru tools/photon-capsule.sh as it signs the capsule. At stow the capsule's own signed declaration must agree with the entry (developer, option, name, version); one that does not is refused and the entry dropped.

Metrics are not in an entry: they are about the kist, not the developer's word, and stay in the directory's `metrics` sections by developer and option.

### 1.1 The bootstrap copy: fgtw.org

kists.fgtw.org keeps the directory as today, its content changed: per kist the newest entry, carried byte for byte (one `v` blob per entry, as Photon's `release_tell` carries its manifest), plus the `metrics` sections. The directory's own signature (the ferros project key, once chosen) then says only "fgtw.org held these at this time"; every entry verifies on its own whoever passed it. kistcat builds it from entry files as it builds it from capsules today. fgtw.org lists entries whose capsules live elsewhere (the owner's on holdmyoscilloscope) and hosts only commons capsules itself. The manifest pointer form goes: one entry form for every developer (decided 2026-11-24, "One entry format, yes.").

A device reads the bootstrap copy when the person browses the Kists page's "available" (as now), at a stow, and on the freshness events of section 4. It keeps from it only the entries of kists the person has stowed (1.4).

### 1.2 Passing entries between devices

Push on contact and on change, never polled. The same shape as Photon's release notice and the identity space's `space_want`:
- **The claim.** On a link coming up (a fleet device or a person's device first heard on this network, a pong from one beyond it), each side sends one signed VSF `kist_claims`: per kist it holds an entry for, (developer, option, version). For 50 kists about 4 KB, once per link edge.
- **The hand-over.** For each kist both hold, the side holding the newer entry sends it (`kist_entries`, the entries as blobs). The receiver verifies each alone (eggs against the `kb` it carries, that `kb` against the person's pin for that developer, as the stower's admit does) and keeps only the newest per kist.
- **On change.** When a device comes to hold a newer entry (its own stow or update, fgtw.org's copy, a peer's hand-over), it hands it at once to every peer heard whose claim was less: fleet first, then the person's people.
- **Once per link.** A device is handed a given entry once per link; a giver is never handed back what it gave.
- **What travels:** only entries for kists the receiver claims to hold. A device learns of a kist it does not hold from the bootstrap copy or from a person's explicit offer (a friend sending a kist in Photon, KISTS.md "Stowing"), never from gossip. This bounds a stranger developer's flood to the devices that chose that developer's kist (open question 4 asks about discovery thru friends).

### 1.3 Which entry is newest

Per kist identity (developer key, option):
- only entries whose every egg verifies, whose `kb` is the person's pin for that developer (or none is pinned yet), and whose key is not revoked or handed off (1.5);
- the greatest version by the stage rule (ferros_kist `Version`, `significant`): more parts is newer, then left to right as numbers (12 < 1.0 < 45.0 < 1.0.0);
- two entries of the same version naming different capsules are the developer's own equivocation: the device keeps the one it held first, never switches on it, and says so on the developer page (open question 3);
- an entry older than the one held is dropped and never passed on.

The stower never refuses an older capsule offered by the person (KISTS.md); the list simply never offers one.

### 1.4 Where a device keeps it

In the stower's scope, one VSF document `kists.entries`: the newest verified entry per kist on the person's list, written on change. Nothing for kists the person has not stowed. It is public and derivable (from fgtw.org or any peer), so it is device class, and a reset device refills it from its first peer or from fgtw.org.

### 1.5 A developer's key: hand-off and revocation

Two more signed records, passed with entries and ahead of them (a device always sends key records first on a link):
- **`kist_handoff`**: signed by the old bundle with every egg, naming the new bundle (`kb`) and the time; countersigned by the new bundle (a second VSF file wrapping the first, signed by the new key), so neither side can claim the other alone. A device that verifies one moves the pin from the old bundle to the new one (the person's `PIN` fact, IDENTITY-SPACE.md), keeps every kist's answers and domain, and keeps each kist's thing identity `ht` as the first key gave it (KISTS.md: "a signed hand-off carries the original `ht` forward"). After it, no entry under the old key is taken, whatever its stamp.
- **`kist_revoke`**: signed by the key itself: "nothing more under this key is mine". Sticky, like the space's tombstone: once a device holds it, no entry and no capsule under that key is taken again. A thief holding the key can revoke it too; that only stops updates, the safe direction.

**Theft.** A stolen key can sign updates and could hand off to the thief's key. The defence is pre-rotation: a developer's bundle commits to its successor (`next`, the commitment `KeyBundle::commit` of the next bundle, kept offline), and a hand-off is taken only when the new bundle matches the old bundle's `next`. The thief, lacking the next key, cannot hand off; the developer, holding it, hands off and so cuts the stolen key off on every device the record reaches. Bundles today carry no `next` (open question 1). Without it, two hand-offs from one key are refused both and the person is told.

**What a revocation does to a stowed kist:** it keeps running as checked and sealed; no update is offered; its card says its developer withdrew the key (decided 2026-11-24, "revoked key, also yes."). Stopping or unstowing it stays the person's choice.

## 2. Fetching

### 2.0 The order of sources

0. this device's own `capsules.` (a capsule kept from before);
1. the person's own devices (the fleet source, built);
2. their people's devices that claim the kist (phase 1);
3. devices on this network that answer a who-has (fleet, people, or a "support the network" server: phase 3);
4. the web, per developer: the entry's `web` URLs (holdmyoscilloscope for the owner's own kists, kists.fgtw.org for commons kists), and for a commons kist http://kists.fgtw.org/capsules/<blake3>.vsf by its BLAKE3 alone.

The order is preference, not a queue. A fetch asks every peer source it has together and spreads the pieces over those that answer (2.2). The web joins only when no peer source holds the capsule, or every peer source asked has stalled (no piece within PT's own retransmit time, a real deadline). "Hopefully we shouldn't need to but fgtw.org" (owner, 2026-11-10).

### 2.1 Pieces: the BLAKE3 tree

A capsule's BLAKE3 is already the root of a tree, and twin.rs already cuts it at 2^16-byte leaves, each leaf a subtree of BLAKE3's own tree. So pieces verify alone without changing any capsule's address:
- **The piece list**: a complete VSF file `capsule_tree`: `capsule` (`hb`), `size` (`u`), and `leaves`, one `hb` per piece in piece order (each a leaf's chaining value). For Photon's capsule about A#1600 pieces, about 52 KB. It needs no signature: merged up the tree (twin.rs `root`, `node`) it must give the capsule's BLAKE3, so a list from anyone is checked against the name the person's list holds. A capsule of one piece has no list; its BLAKE3 checks it whole.
- **A piece**: hashed with its offset (`set_input_offset(k << 16)`, `finalize_non_root`) and compared with leaf k. A wrong piece is known the moment it lands, and who sent it.
- **Where the list comes from**: made at stow at no extra pass. Twin already holds every leaf's chaining value while it hashes (the kernel's kist job hashes each capsule thru it, ferros_kernel service.rs); the stower's `Intake` hashing thru Twin too, with an accessor for the leaves, gives the list as the bytes pass. It is kept beside the capsule, `capsules.<blake3>` and its list in the same scope (0.05% of the capsule, no new place). Each capsule's list is published beside it on its developer's host (`<capsule URL>` with `.tree.vsf` in place of `.vsf`; kistcat writes it, and Photon's deploy for Photon's). Any source that serves the capsule serves its list.
- This is Bao's verified slicing with the outboard kept at its leaf level: one flat list fetched once in place of a proof path with each piece.

### 2.2 Several sources at once

Because a device serves only capsules it has stowed whole (3.4), every source holds every piece: no bitfields of who has what, no rarest-first, no choking. The scheduler is short:
- piece 0 first, then the list, from whoever answers first;
- the rest dealt to the sources in runs, each source kept as busy as its own PT window drains (a fast source gets more runs, a slow one fewer, measured by what arrives, no constant);
- a run that stalls goes to another source; the last few pieces go to two sources and the first copy wins;
- a source that sends a piece that fails its leaf is dropped for this fetch (section 5).

**The signature before the bulk.** The signature's slot is in the capsule's header, inside piece 0. With the verified list and piece 0, the stower has the capsule's whole file hash (twin.rs: the leaf the slot touches hashed again with the slot as zeros, every other leaf from the list) and checks every egg, the developer and option, and the pin, after about 120 KB. A capsule signed by the wrong key is refused before the other 99.9% is fetched, and nothing unchecked is ever committed: pieces are written only after the eggs pass.

### 2.3 Resuming

The store is the only state (photon docs/PT.md "The bitmapless transfer"): pieces land in `capsules.<blake3>` at their offsets, the list beside it, no bitmap and no progress record. On a restart (of the stower, the device, or a fetch put off by a network change), the restore pass finds a capsule on the person's list that is not whole, reads the list, hashes each piece the vault holds against its leaf and fetches only those that fail. The tree check is the presence test, exact even for a piece that honestly is all zeros. A partial capsule is inert (no seal, so no launch) and is dropped when the kist leaves the person's list.

To measure before building: what the vault returns for a range of `capsules.<blake3>` never written, and whether `store_set_addr_at` takes pieces out of order.

### 2.4 The checks, step by step

| Step | What is checked | Against what | A failure |
|---|---|---|---|
| the entry | every egg; `kb` | the entry's own `kb`; the person's pin for that developer | entry dropped, never passed on |
| the piece list | merged to its root | the capsule's BLAKE3 on the person's list (or the entry) | list dropped, its source dropped |
| piece 0 | its leaf | the list | piece refetched elsewhere, source dropped |
| the signature | every egg over the file hash; the developer and option; the pin | the capsule's `kb`; the entry; the person's list (`capsule::admit`) | the fetch ends, nothing kept: "signed by a different key than <developer>'s other kists", as today |
| each piece | its leaf | the list | as piece 0 |
| the whole | BLAKE3 and size | the name asked for | cannot fail if every leaf passed; kept as the stower's check today |
| the code | the gate where it lies, the checker over its masked code | the kernel's stow mode (`SYS_SPAWN` stow, `stow_check`) | refused, bytes dropped |
| the seal | `oracle::seal_parts` | this device | as today |

From the signature on these are the stower's stages unchanged (KISTS.md "The stower", stages 1 to 4); only the fetching stage changes.

### 2.5 The transport

Photon Transport over UDP (4383), on the local network and, once ferros has a WAN peer path, over the internet; plain HTTP to the web hosts (the stower's net.rs, which needs a `Range` request for pieces).

The framing rule (feedback_vsf_not_for_framing): control is complete signed VSF, data goes lean.
- **Control**, each a complete VSF file signed with the device's key, as fgtw's frames are today: `capsule_want` (capsule, first piece, count), `capsule_none`, `capsule_who` (2.6), `capsule_have`, `kist_claims`, `kist_entries`, `kist_handoff`, `kist_revoke`.
- **Data**: a reply carries a run of pieces as one complete VSF file (`capsule_piece`: capsule, first piece, count, the bytes as one blob), so one header covers up to a mebibyte, and PT carries it in its own lean DATA packets with its per-chunk BLAKE3. The reply needs no device signature: the tree proves the bytes, and PT's peer names the sender for blame. Today's `capsule_piece` (one 2^16 piece, signed) becomes this with a count.
- **Not sealed.** A capsule is public, so sealing its bytes hides nothing that matters (threat_model_harm_not_observation) and costs a pass. On a link that is sealed anyway (Photon's lanes to a person's devices), PT's frames are lean ciphertext and nothing here changes.
- **No amplification.** A `capsule_want` is small and its answer large, but PT blasts only after the asker's SPEC ACK, so a want with a forged source address draws one SPEC to the victim, not the capsule.

### 2.6 Finding peers that hold it

- **The fleet**: the members heard (built).
- **People**: their devices heard on this network (their beacons carry their handle proof, which the person's `PERSON` facts know), and once there is a WAN peer path, their devices' last-known addresses. The peer path itself is Photon's open gap (photon_on_ferros_no_bootstrap: FindNode and Query exist, no peer answers them).
- **The local network** (phase 3): a signed `capsule_who` (the capsule's BLAKE3) to the beacon groups and the subnet broadcast on 4384, answered with a unicast `capsule_have` only by a device that holds it whole and may serve the asker (3.0). Nobody announces what they hold unasked.
- **Strangers beyond the local network**: not in this design. Later, the FGTW peer network's own lookup among devices that opted in; never a tracker at fgtw.org listing who runs what (open question 9).

## 3. Serving

### 3.0 Who may fetch

| Asker | Served | Decided by |
|---|---|---|
| the person's own devices | always | a member of the chain under the same handle proof, not locked out (`capsule_member`, built) |
| the person's people | on by default, one switch off | the asking device's key in the fleet of a `PERSON` fact (fgtw::people) |
| anyone | only with "support the network" on | the device's own switch, off by default |

"Support the network" is its own switch, apart from custodians: custodians are a trust choice about named people for recovery; serving is a contribution to strangers (owner, 2026-11-24). It may later also cover relaying and bootstrap. Recommended per device, not per person (open question 6), since it is about this device's link and power.

### 3.1 What a request reveals, and to whom

- **A want** shows the asked device which capsule (so which kist and version) and which device asks. Fleet and people: their device key, so the person, as every frame between them already does. To a stranger under support the network: a key made for that one fetch, never the device's key, since the server needs no identity to serve public bytes.
- **A who-has** shows everyone on the local network that someone here wants that capsule, and from which address.
- **A claim** shows the peer which kists and versions this device holds: only fleet and people get claims.
- **A have** shows the asker that this device runs that kist. To a stranger, a device answers only for the capsule of the newest entry it holds: one running an old version it has not yet replaced does not tell strangers so.

The threat model is harm, not being seen (threat_model_harm_not_observation); the one harm here is pointing an attacker at a device behind on an update, and the last rule closes it for strangers.

### 3.2 Which link

WiFi preferred, cellular off for serving by default ("Whatever. WiFi preferred", owner, 2026-11-24), granular later:
- the fleet is served on any link (the person's own restore; open question 7);
- people and strangers only when the reply would leave on WiFi. ip needs a per-send flag, "WiFi only, else not at all", so the answer is refused rather than quietly carried on cellular when WiFi drops mid-transfer;
- fetching is allowed on either link, WiFi preferred: the person asked for the kist.

### 3.3 Desktops

A desktop serves only kists it actually runs ("Only if the desktop actually uses them so probably not. I'm a fan of supporting the network but not bloat."). Today a desktop runs Photon natively and no ferros capsule, so it serves none. Once kists carry toka and run on Linux thru a host app (KISTS.md "The directory", the page), a desktop serves the kists it runs, with the same rules. The phase 0 test's desktop serving a folder of capsules is a test rig, not this (open question 8).

### 3.4 Storage

A device serves what it has stowed, whole and admitted, and keeps nothing extra: no cache of other people's kists, no seeding store, no reserved region (founding rules 2 and 3). The capsule, its piece list and the entries all live in the vault, in the stower's scopes, and go when the kist leaves the person's list. A partial capsule is never served, and a capsule that failed its checks was never kept, so a device never relays bytes it did not check.

## 4. Freshness and withholding

A peer cannot forge an entry or a piece. It can withhold: claim an older version, or say none. The defence is to ask more than one source, fgtw.org among them, at moments that matter, triggered by events, never by a timer (feedback_no_polling):
0. a stow or an update offer: before the fetch, the newest entry for that kist from every peer heard and from fgtw.org;
1. the restore pass after attest (the space's first pull settled);
2. the person opening the Kists page or a kist's card: a look is a question;
3. joining a network (a WiFi association, the cellular call coming up): new peers and a new path to fgtw.org;
4. a peer first heard on a link (the claim exchange, 1.2);
5. a fleet sibling's space change that moves a kist's version (the person updated on another device);
6. any newer entry heard from anyone, at once (push).

fgtw.org is asked at 0 to 3 when reachable. Two sources disagreeing is not an offence: the newer verified entry wins and the other was stale or withholding, which the device cannot tell apart and does not punish.

What stays open: a device that meets no event for a long time (no new network, nobody opening a page, no reboot) learns of an update only when a peer pushes it. A held call at fgtw.org answered on change would close that (a doorbell, not a poll; Photon's hub subscription is one), and needs a connection kept open (open question 10). The identity space's 64 s re-ask of fgtw.org (IDENTITY-SPACE.md "At attest", step 5) is a timer the list must not copy, and should itself go to events.

A device whose every source is controlled by one party (its network and its peers) can be held on an old version; fgtw.org over another link (cellular), at event 3, is the way out.

## 5. Abuse

- **Bad pieces:** a piece that fails its leaf drops its source for this fetch, and the piece goes to another. A fleet device that does it is a bug or a compromise: the Fleet page tells the person. A person's device or a stranger: the device stops asking that key for the rest of this boot; nothing is written down beyond that and nothing is shared.
- **Bad entries and records:** one that does not verify is dropped, and the peer that sent it gets no more of this device's attention on this link.
- **Too many asks:** rate by what is real, not by made-up quotas. The stower answers one want at a time (fgtw queues them, built), in order fleet, then people, then strangers; a stranger's want is answered only while nothing of the person's own waits, on WiFi, and never faster than the PT window to that asker drains. A queue that outgrows what the uplink drains refuses strangers first. Nothing here is a constant to tune.
- **Spoofing:** a want with a forged address draws one SPEC (2.5); a want under someone else's key fails its signature.
- **No central bans.** Each device decides from what it saw; no list of bad peers exists anywhere, fgtw.org included. A bad developer is the person's call (unstow, and the pin keeps their other kists recognisable).

## 6. Phased build

Each phase ends with a test on devices that proves it, run by the owner or with his OK.

### Phase 0: the fleet fetch on two devices

Build: nothing new; the test of what 2b53c2bb built, with the gaps it finds fixed. Also first: every capsule on the person's lists on its developer's web host (the owner's kists on holdmyoscilloscope, commons kists on kists.fgtw.org), so the fallback exists.

Test (IDENTITY-SPACE.md "The two-device test", unchanged): the Pro and a sibling of the same person's fleet (the owner's desktop Photon answering `capsule_want` from a folder of the nine capsules, or a second ferros device in his fleet; not the 8a, another identity).
0. The Pro's `capsules.` empty; restore runs. Its console shows the fleet asked before fgtw.org, the desktop holding it, `kists: a capsule from a fleet device`, then `kists: restored <name>, checked and sealed`, and the kist runs.
1. A capsule the sibling lacks: its none, then HTTP. The sibling closed: the 2^23 µs ask, then HTTP. A corrupted capsule: refused by its BLAKE3, not kept, then HTTP.
2. The reverse: the desktop asks the Pro and checks the BLAKE3.
3. Measured and written down: Photon's capsule end to end over the fleet (seconds, MB/s), the round trips, and what a sibling switched off half way leaves (today: a restart from 0 over HTTP).

### Phase 1: people

Build: a person's devices as a source (their beacons matched to `PERSON` facts; the stower's asks to them after the fleet's); the serving switch for people; the claims and entries exchange between fleet and people, with the entry format (1.0), `kistcat kist --entry`, and Photon's deploy writing its entry; `kists.entries`.

Test: two devices of two identities that are each other's people (with the owner's choice of identities), on one network.
0. Device A stows the Clock from the directory; B lacks it; B stows it: A serves it, B's console names a person's device as the source, and it runs.
1. A's people switch off: A answers none, B goes on to fgtw.org.
2. A third device of neither person asks A: refused.
3. A newer Clock entry given to A only: on the next link edge B is handed it and offers the update, without fgtw.org.

### Phase 2: pieces from several sources

Build: the piece list (kept at stow, published beside each capsule on its web host), verification per piece, the signature check after piece 0, the scheduler, runs of pieces per reply, `Range` on the HTTP client, resuming from the vault, ip's "WiFi only" send.

Test: the Pro fetches Photon's capsule from a fleet sibling, a person's device and kists.fgtw.org at once.
0. The console shows pieces from each source and the split; the capsule stows and runs; faster than phase 0's single source.
1. One source switched off half way: the others finish.
2. The Pro rebooted half way: after attest the fetch resumes; the count of pieces fetched again is zero, or the one in flight.
3. A source serving a capsule with one byte flipped (a development-build hook): that piece refetched elsewhere, the source dropped, the capsule stows.
4. A capsule whose eggs do not verify: refused after piece 0 and the list, with no more than about 120 KB fetched.
5. The Pro's WiFi off, cellular on: people's asks answered none; the fleet's served.

### Phase 3: support the network, the local network, and the list between peers

Build: the "support the network" switch and serving strangers under it (a one-fetch key on the asker's side); `capsule_who` and `capsule_have` on the local network; the list passed between all peers with key records first; `kist_handoff` and `kist_revoke`, and pre-rotation if approved.

Test:
0. A device of neither person on the network with support the network on: a who-has is answered and it serves; switched off: silent.
1. A newer entry on fgtw.org that a peer withholds: opening the Kists page brings it from fgtw.org.
2. A hand-off record for a test developer: the pin moves, the kist's answers and its place on the deck stay, its `ht` is unchanged, the next update under the new key stows; an entry under the old key is refused.
3. A revocation: no update taken under the key, the kist runs on, its card says so.

## 7. Open questions for the owner

Questions 0, 2 and 11 are answered ("Decided" at the top); the rest are open.

0. **One entry form.** DECIDED 2026-11-24: "One entry format, yes." Photon included.
1. **Pre-rotation.** Add `next` (the commitment of the successor bundle) to every developer bundle, so a stolen key cannot hand off? Recommended: yes, before more developers pin (the bundles out today, the owner's and Photon's release key's, rotate once by a hand-off to bundles that carry it).
2. **A revoked key's kists.** DECIDED 2026-11-24: "revoked key, also yes." They keep running, get no update, and the card says so.
3. **Same version, different capsule.** Keep the first, never switch, note it on the developer page? Recommended: yes.
4. **Discovery thru people.** Should a device learn of kists its people run (not just updates to kists it holds)? Recommended: not now; it is a product question, and leaving it out keeps a stranger developer's flood to devices that chose them.
5. **Serving partial capsules.** Recommended: no; only stowed and admitted, so nothing unchecked is ever relayed. It also keeps the scheduler free of who-has-which-piece.
6. **"Support the network" per device or per person.** Recommended: per device, off by default, on the device's Network page: it is about this device's link, power and uplink, and a desktop and a device in a pocket differ.
7. **Cellular.** The fleet served on any link, people and strangers on WiFi only, fetching on either? Recommended: yes, until the granular settings.
8. **The desktop as a capsule source for its own fleet.** Should a desktop Photon keep the person's ferros capsules so the person's devices restore from it? Recommended: no (it runs none of them; "not bloat"); the folder rig is for the phase 0 test only.
9. **Strangers beyond the local network.** Recommended: later, by the FGTW peer network's own lookup among devices that opted in, never a tracker at fgtw.org.
10. **A doorbell at fgtw.org for updates** (a held call answered on change) for devices that meet no event. Recommended: not now; events cover a device in use, and a connection held open costs power on a device in a pocket. Revisit with measurements.
11. **Where capsules are hosted.** DECIDED 2026-11-24, revised from "everything on kists.fgtw.org": "I'd put Lumis and Mahere on holdmyoscilloscope and make fgtw.org point there since those are mine. Photon and possibly some other system apps will be hosted at kists.fgtw.org since Photon is not mine." Per developer; the entry carries its `web` URL. Still open within it: whether the owner's other own-key kists (the pucks, Aemulus) go on holdmyoscilloscope as Lumis and Mahere do (recommended: yes, the same rule), and whether a stow over USB puts its capsule on the web at all (recommended: no; the fleet carries it, and publishing is the developer's act).
12. **Listing without the owner** (passless listing): a fgtw.org endpoint that takes a developer's verified entry and capsule. R2 storage then costs money, which the DUNA prices at cost (fgtw_duna_runanga). Recommended: after phase 3; until then kistcat, run by the owner, is the door.
13. **The piece size.** 2^16, the size twin.rs and the fleet source already use. Recommended: fixed as a protocol constant (it is the tree's shape, not a ceiling).
