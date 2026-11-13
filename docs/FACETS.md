# Facets: what a kist may know about the person

Status: a proposal for the owner to mark up, 2026-11-12.
Built 2026-11-12: "To build" steps 0 to 2 (the alias derivation and the person key in fgtw, the kernel's alias call and its hand-over op, the identity service opening or minting the person key). Built 2026-11-13: be you and people end to end on ferros's side (the binding, the people, the `facets` endpoint, the rights, the People page and the add-a-person sheet, a kist's people all or chosen, std/photon's main up to Photon's seam); see "Built" under "To build". Not built: places (`SPOT`), the pick sheets, Photon's side.
Each part says **Decided** (with the owner's words), or **Proposed** (mine, not yet answered); the decisions the owner must make are gathered at the end ("Decisions for the owner", D0 to D9).
The permission list this rests on is KISTS.md's "The permission list"; Photon's move onto its own key is PHOTON.md's "Photon's own key: the migration".

## How this came up

Photon, stowed on ferros, opened on its own handle screen: the person had attested at boot and Photon asked who they were all over again (PHOTON.md "Photon and the person's identity").
The fix that looked obvious, handing Photon the identity's roots, hands it everything: the identity seed is the root of the signing key, the vault seed and the fleet, and PORTAL.md section 8 says "An app never signs as the person."

The owner, 2026-11-12: "By default all kists get my identity... Could act as you? I mean, kinda?"
The answer he took: every kist acts for you in its lane, and the line between acting for you and being you is the key.
Then: "Yes, plus then we get into granularity, like does it know my friends? My address? What kists I use?", and to writing it up: "Please."

This document is that write-up: what a kist knows about you by default, what you can hand it, what it never gets, and how each of those is made and kept.

## The model in plain words

**A kist knows nothing about you except what you hand it.**
By default it has a stranger's view: it runs on your device, draws in its circle and keeps its own things, and it cannot tell who you are, whom you know, where you live or what else you run.

**But it recognises you.**
Every kist gets a private alias: a key made for that kist alone, the same on every device you own, so the kist knows "the same person as yesterday, on another device" without learning anything else.
Think of a loyalty number one shop gives you: the shop knows you are a regular, and the number means nothing at the shop next door.

**What else it knows is a facet you hand it.**
Each facet is one slice of you, asked once, ever, the first time the kist needs it (KISTS.md "Permissions"; "Claude memories/permissions_asked_once.md"):

| facet | what the kist learns | default |
|---|---|---|
| alias | a key of its own that stands for you in it | implicit |
| language, number base, time zone | how to speak to you | implicit |
| be you (your public identity) | who you are to everyone else, so others find and recognise you in it | asked |
| people | which of your people it may know: all, or the ones you choose | asked |
| places | your home, work and saved spots: all, or the ones you choose | asked |
| the hold | the kinds of your items it may keep and open (STORE.md) | asked, per kind |

**Pick, don't share.**
A kist that needs one thing (an address for one delivery, one friend to send a photo to, one photo to edit) gets that one thing from a sheet the system shows, never the whole book.

**Some things no kist ever gets**, asked or not: which other kists you use, your fleet, your identity's root, other kists' data (the full list is "The never list" below).

Examples, as proposed:
- **Photon**: its alias, be you, your people (all, or chosen), the network, your local network, the camera and the microphone for a beam, and a pick when you share a place or a photo.
- **Mahere** (the map): its alias, where you are (GPS) and where you were last (place), its declared map servers, and your places if you want home and work on the map. Never be you, never your people: a map has no one to show you to.
- **A game**: its alias (so its saves follow you), the hold for its save states, nothing else.

## The alias (implicit)

**Proposed.** The kist's alias is a seed, derived from the person's root and the kist's identity, from which the kist derives its keys the way a device key's seed gives its keys (fgtw's `SigningBundle`: Ed25519, Falcon-512, SPHINCS+, and X25519 for agreement).

`alias = derive_key("ferros 2026-11-12 kist alias v0", root ‖ developer key ‖ option)`

- **For that kist alone.** The kist's identity is its gate identity, the developer's Ed25519 key and the option (KISTS.md "Whose key signs a kist"), the same pair its answers and its vault domain are kept by. A rebuild signed by another developer is another kist and gets another alias, as it gets another domain.
- **The same on every device.** The derivation takes nothing of the device, so Photon on the Pro, on the 8a and on the desktop holds one alias. That is what lets a kist recognise the person everywhere, and it is also the kist's own sync key: whatever it keeps for the person across their devices (its notes-to-self, its settings) can be sealed under a key derived from its alias, with no fleet key and no help from the system.
- **One-way.** Holding an alias reveals nothing of the root or of any other kist's alias.
- **Not a device identifier.** The kist's ira key ("its own device-sealed key", implicit since 2026-11-12) is the per-device one; the alias is the per-person one. Together they let a kist tell your devices apart from each other and know they are all yours, and nothing outside its lane.

**What the root is, D0.**
The obvious root is the identity seed, and it has a catch the owner must weigh.
The identity seed is BLAKE3 over the handle (`ihi::handle_to_hash`), and photon's own audit says so plainly (photon docs/fleet-identity-remediation.md, "the binding proves nothing about ownership"): anyone who knows the handle computes it.
Every friend who typed the handle to add the person computed it once (photon docs/identity-profile.md: "type it once → derive → pin → drop").
So an alias derived from the identity seed:
- can be computed, and signed with, by anyone who knows the handle and the kist's identity (both public for a commons kist);
- links across kists for that same someone: they can compute the person's alias in every kist and match them.
That is no weaker than today (the identity key that friends pin is `Ed25519(identity_seed)`, the same exposure), but it makes unlinkability hold only against people who never knew the handle.

The alternative is **a person key**: 32 random bytes minted at the identity's genesis (for an existing identity, by the first device that runs this), never derived from the handle.
- It is kept by the fleet: sealed under the fleet key in the fleet's state, as Photon's roster is (photon docs/fleet-key.md), re-sealed at every shrink with the rest of the state, so a removed device never sees a new copy.
- Each device's identity service opens it at attest with the fleet key it already recovers (PHOTON.md "Nothing on disk by default"), and holds it in RAM only, as the roots.
- Unlike the fleet key it does not change at a shrink: an alias must stay put. Changing it is an act of its own (the person suspects it leaked), which changes every alias and needs every kist that published one to sign its move (the same record as Photon's migration, PHOTON.md).
- Total loss of every device loses it, as it loses the fleet key today; TOKEN's custodian recovery (hardened Shamir, "Claude memories/token_recovery_shamir_not_oprf.md") is where it would come back from.
**Recommended: the person key.** Its cost is one more secret the fleet carries; what it buys is an alias nobody can compute from a handle, which is the whole point of having one.

**Who derives it, D1.**
The brief this was written from had fgtw (the identity service) derive aliases inside itself.
The owner's rule of 2026-11-07 (VAULT-OUT.md "The rule") is that the kernel is the only one that issues and manages keys: "Nothing else derives a key."
So, proposed: **the kernel derives each kist's alias and hands it to that kist alone**, as it already binds a kist's ira keys to the kist's gate identity (KISTS.md "A kist's ira keys are its own").
- Under the identity seed (D0's first answer) the kernel holds the seed already while the person is present (AUTH.md "Attest with a tag").
- Under the person key the identity service hands it to the kernel once it opens it, as the attest screen hands the seed (a new `SYS_UNLOCK` op, Unlock right only), and the kernel wipes it at lock with the identity key.
- A kist asks for its alias as it asks for its ira key: `SYS_IRA` with a new op (or a sibling call), answered only to a kist launched by the kist job, only while the person is present, from the kist's gate identity as the kernel stamped it. No kist can name another kist's identity.
- The identity service never holds an alias; it holds the root for its own work (the fleet, the space) and nothing a kist hands back is ever a root.

**Aliases and a developer's key moving.** A kist that moves to a new developer key (KISTS.md "Gap: custody moving") would get new aliases for every person.
Whatever hand-off is designed there must carry the kist's first identity, and the alias derive from that, so the person's alias in the kist survives the move.
Photon is the case that matters: its alias derivation names Photon's kist identity as a constant (PHOTON.md), on every platform, and that constant must outlive the release key.

## Language, number base, time zone (implicit)

**Proposed**, as the owner accepted it ("a small fingerprint, accepted"): every kist may read the device's language, number base and time zone, as every process reads them from the kernel's preferences today (SETTINGS.md; the identity service writes them from the person's LANG and ZONE facts, IDENTITY-SPACE.md).
They are a small fingerprint (sixteen languages, a few bases, a few hundred zones), and a kist that speaks to a person needs them.
Never handed with them: the region the zone was taken from (a cell, a GPS region: TIME.md), or anything of the device's settings beyond those three.

## Be you: your public identity (asked)

**Proposed.** The facet that lets others find and recognise the person in this kist.

**What the kist gets:**
- the person's handle proof (public already: it is their slot, beacons and the registry carry it);
- a **binding**: one signed record saying "this alias speaks for the identity at this handle proof, in this kist", made by the identity service and signed with this device's fleet key (every egg), its fields typed (`hp` the handle proof, `ke` the developer, `option`, the alias's public bundle `kb`, when, Eagle time `e`), a complete VSF file.

**What it lets the kist do:** present its alias, with the binding, to anyone.
The other side checks the binding against the person's chain (which they fetch by the handle proof, as Photon fetches a contact's chain today): the signer must fold as a current member.
So a contact's copy of the same kist knows "this alias is Ana", and nothing more of Ana's.

**What it does not let the kist do:** sign anything as the identity, change the fleet, mint, leave, add a device.
The binding vouches for the alias inside its kist; the alias is not a member of the fleet and the chain never names it.

**Where bindings go.** Proposed: nowhere public.
A slot keyed by the handle proof and the kist's identity would let anyone holding the handle proof ask "does this person use Lumis?", which is the never list's "which other kists you use".
So a binding travels with what the kist sends (Photon hands it to a contact with its first photon, and with the migration's rotation, PHOTON.md), and each verifier keeps the one it was handed.
A kist that wants strangers to find the person in it (a public profile, a game's leaderboard) needs a published binding; that is a choice the person makes in that kist with its own words, under its own slot, never the system's default (D7).

**The person-facing words** (KISTS.md): "be you in Photon: your contacts find and recognise you there".
Photon yes, Mahere no.

**The fleet is public to whoever holds the handle proof.** Honest note: the chain at fgtw.org is readable by anyone who has the handle proof (its member keys, and so how many devices the person has).
A kist with be you and a network path to fgtw.org can read it like any stranger with the handle proof can.
That is the chain's design, not the facet's; the never list below means the system never hands the fleet to a kist, and keeping the chain itself private is a change to fgtw.org's public read (D8).

## People (asked)

**Proposed.**

**What a person is**, as the shared model has it (people_not_devices_split, 2026-11-10): an identity, a handle proof with the genesis its chain was pinned at, the name they publish (photon docs/identity-profile.md), the name the person gave them (a petname, theirs alone), and an avatar when they granted one.
Never their handle: it exists at rest nowhere, friends' devices included (identity-profile.md "The shift").

**Where the person's people live.** Today, in Photon's own vault: a contact row per friend, Photon's alone.
Proposed: **the person's people are the person's**, facts in their identity space (IDENTITY-SPACE.md: a `PERSON` kind, the handle proof, the pin, the names), synced to every device of theirs by the identity service, and served by it.
Photon is where friendships are made (the CLUTCH ceremony), so this moves Photon's contact list out of its vault into the space, on ferros; on other platforms Photon is the identity service and keeps them where it does (D5).

**What the kist gets:**
- **all**: every person, kept current (a new friend made later is in);
- **chosen**: the people the person picked on the kist's card, kept as a fact in the space (by kist identity), so every device knows the same choice; a new friend is not in until picked.
Per person: the handle proof, the pin, the published name and the petname, the avatar if the friend granted one.
Not: their fleet, their devices, how they were met, which other kists know them.

**Adding a person** is the person's act, never a kist's.
A kist that makes a friend (Photon after a ceremony, a game after a match) proposes it; the system shows a sheet under the gesture ticket ("add Ana to your people?"), and only the sheet writes the fact.
So no right to add people exists to be asked for.

**People and aliases.** A kist that holds people knows its contacts by their handle proofs.
To find a friend in the same kist, the friend's copy of the kist must hold be you and hand over its binding: a friend who did not let a kist be them is not findable in it, even by someone who has them in their people.

## Places (asked)

**Proposed.** The person's saved places: home, work, and spots they keep (a parking spot, a friend's house).
They are the person's (identity space; the kind needs a name other than `PLACE`, which is the deck's circle placement: D9 proposes `SPOT`), kept and edited on the system's places page.

**Separate from where the person is.** `place` (passive, the last place the device knew) and `gps` (active) are LOCATION.md's two rights: where you are now and where you were last.
The places facet is where you keep coming back to: a kist with your home address does not know whether you are home, and a kist with GPS does not know which point is home.

**All or chosen**, as for people.
Per place: its name, the person's words for it ("home"), and the place (a Dymaxion coordinate, as LOCATION.md keeps one), and an address when the person typed one.

## The hold (asked, per kind)

STORE.md's two rights, unchanged: a kist keeps for itself in its own domain (implicit), and keeps for the person in the hold with `items`, the kinds it makes and opens declared in its capsule and each opened kind asked once.
The facet is that: which kinds of the person's items the kist may see.
Most kists need none of it: the pick (below) hands them one item.

## Pick, don't share

**Proposed**, on the clipboard's model (owner, 2026-11-10, "Claude memories/clipboard_capability_handoff.md": "a capability handoff, brokered by whoever owns the trusted input path") and STORE.md's "Handing an item from one kist to another".

A kist that needs one thing from a facet it does not hold asks for a **sheet**: its button, under the one-shot ticket the input path hands it with the tap, opens the system's sheet inside the kist's circle.
No gesture, no sheet: a kist cannot raise one in the background or at launch.

| sheet | what the person sees | what the kist gets |
|---|---|---|
| an address | their places, and a line to type one for this time only | one address (the place and its words), as a value; nothing of the others |
| a person | their people, and a line to type a handle proof's words | one person (handle proof, pin, names), as a value |
| where I am | the place now, rounded as they choose | one place, once; not the `place` right |
| an item | the hold's find page, filtered to the kinds the kist takes (STORE.md) | a handle to that one item |

- **Once is once.** The kist keeps what it was handed (nothing can stop a kist remembering a value), and the sheet says so ("Mahere gets this address").
  It gets no standing right: next time is another pick.
- **No lying.** A kist gets exactly what the person picked; cancel is an empty answer, never a made-up one (KISTS.md rule 5).
- **Picks are the default way.** A kist that asks for a whole facet when it needs one value at a time is asking for too much, and the facet's question is where the person sees that.

## The never list

**Proposed**, as the owner received it on 2026-11-12 (not yet item by item). No kist gets these, asked or not; there is no right that grants them, so there is nothing to say yes to:

0. **Which other kists you use.** The Deck, the stowed list (`kists.list`) and the pandok's "stowed" marks are the system's; a kist's own card is the only place its name sits beside the others, and that card is the system's page. (Today the map and the stower hold these, never a kist.)
1. **Your fleet**: your devices, their keys, their names, how many there are, which one this is. (See the honest note under "be you": the chain is readable to whoever holds the handle proof.)
2. **Your identity's root**: the handle, the identity seed, the person key (D0), the identity key, the fleet key, the ira.
3. **Other kists' data**: another kist's domain, or anything another kist keeps for itself; the hold's items only by kind, as granted, or by a pick.
4. **Device identifiers**: the chip's IDs, the UFS serial, the IMEI, the WiFi and Bluetooth addresses (worn per network, from the ira or the identity: AUTH.md "Pairings"), anything stable across kists. The kist's own ira key is the only per-device key it holds, and it is its alone.
5. **Fine device facts**: the exact battery level, the exact model, storage left, signal measures, the serving cell. Where a kist needs to behave (save power, keep a download small) the system may give it a coarse word (low power, on a metered link), never a number.
6. **Your facets it was not handed**: the system never infers one for it (no "people you photon most" for a game).

## Unlinkability: what an observer of two kists can and can't tell

Say the same company runs two kists (or two companies compare notes), and both kists run for the same person.

**They can't correlate by anything the system hands out:**
- the aliases are unrelated keys (one-way, per kist identity; under D0's person key, nobody computes them from the handle);
- each kist's ira key is per kist, per device;
- no device identifier reaches either;
- neither learns the other is stowed.

**They can correlate by:**
- **be you**, if the person handed it to both: the handle proof is the same in both, by design (that facet is "who you are to everyone");
- **people or places** handed to both: the same friends' handle proofs, the same home;
- **the network**: the same address talking to both servers at the same moments. ferros does not hide where traffic comes from; that is a network question (a relay, the FGTW peer network), not a facet's;
- **the small fingerprint**: language, base and zone, accepted by the owner;
- **what the person types or shows**: a name typed into both, a face in front of both cameras.
- Under D0's identity seed answer, also **anyone who knows the handle** (a friend, an employee of a friend's kist who saw it typed) can compute both aliases and match them.

**One kist across the person's devices is linked on purpose**: the same alias everywhere.
A person who wants a kist not to know two devices are theirs stows it on one of them only.

## How answers move with the person

**Decided** for answers (2026-11-07/08, KISTS.md; IDENTITY-SPACE.md): every answer is a fact in the person's identity space (ANSWER, by developer, option and right), asked once, ever, for all their devices.
**Proposed** for the facets' own state:
- **The choices** (which people, which places a kist may know): facts beside the answers (by kist identity), merged as every fact (the latest write wins, a removal sticks).
- **The people and the places themselves**: facts in the space (`PERSON`, `SPOT`), so a new device of the fleet has them at its first pull.
- **Aliases**: not stored anywhere, not synced; derived the same on every device from the root and the kist's identity. A device that has the root has every alias.
- **Bindings** (be you): made on whichever device the kist first needs one; the record names no device beyond its signer, and any device's identity service can make another, so nothing needs syncing.
- **Picks**: not kept by the system; the kist keeps what it was given. STORE.md's item picks are the exception it proposes (its open question 3).
- **Last used** (KISTS.md "Last used on the card"): per device, as it happened there; whether the cards show the latest across all the person's devices is KISTS.md's decision P4.

## On other platforms

On Android, Linux, macOS and Windows there is no kernel of ours between apps and the person's roots: tohu's session holds the 96 bytes for every app the person runs (photon docs/one-identity.md), and an app of the same user can read it.
There, Photon is also the identity service (it carries FGTW), so it holds the root and derives its own alias with the same function, and gets the same bytes ferros's kernel would hand it.
The facets are a wall on ferros and a convention elsewhere; Photon keeping to its alias outside its FGTW part is what makes one Photon behave the same everywhere ("Claude memories/feedback_photon_is_the_full_app.md").

## To build (when approved)

0. **fgtw** (shared by the kernel's derivation, the identity service and Photon everywhere): the alias derivation and its known answers; the binding record (VSF schema, signer every egg, verify against a folded chain); the person key (D0) in the fleet's sealed state.
1. **Kernel**: the alias call (beside `SYS_IRA`), stamped by kist identity; under D0's person key, the op that takes it from the identity service and its wipe at lock.
2. **Identity service** (payloads/fgtw): a `facets` endpoint each kist's grant makes a use of, the kernel stamping the caller: op 0 the binding (be you granted), op 1 the people (granted: all or chosen), op 2 the places (granted). Each answer only while the person is present.
3. **The space** (ferros_space, fgtw::space): `PERSON`, `SPOT` and the per-kist choices.
4. **The map**: the sheets (address, person, where I am), the people and places pages, and each kist's card rows (KISTS.md).
5. **Photon**: PHOTON.md's migration.

### Built (2026-11-12, steps 0 to 2)

- **fgtw `alias`**: `seed(person key, developer, option)` is BLAKE3 `derive_key("ferros 2026-11-12 kist alias v0", …)` over three fixed 32-byte fields (96 bytes, no lengths: the types fix the widths, so no two triples hash alike); its known answer computed apart with `b3sum --derive-key`. `bundle(seed)`: the Ed25519 half is the seed itself, Falcon-512 and SPHINCS+ under `FERROS_KIST_ALIAS` (`SigningBundle::developer`'s domains). `agreement_secret(seed)`: X25519, one way from the seed.
- **fgtw `person`**: `mint(eggs)` takes at least two non-empty eggs, each framed by its length, under `"ferros 2026-11-12 person key mint v0"`. The key is kept as a fact of the person's space of fgtw's own kind (255, counted down from the top so it never meets ferros_space's kinds), its id one way from the key, its value the mint stamp (`e`), the fleet key's fingerprint (`hb`) and the key sealed XChaCha20-Poly1305 under a key derived from the fleet key (`v'X'`; the handle proof, the fingerprint and the id bound in). So the space a device keeps in its vault (`fgtw.space`, kept whatever unlock at boot says) holds the person key only sealed under the fleet key, which is in RAM only. `find` takes the first minted (the mint stamp, then the id), and never a later one while that one does not open here; `put` with the same mint stamp re-seals it under a new fleet key. Two devices minting at once settle on the first minted.
- **Kernel**: `SYS_UNLOCK` op 10 (Unlock right) takes the person key into the identity's keys (kvault `IdKeys.person`), wiped with them at lock; op 0's bit 2 says it is held. `SYS_IRA` with x4 bit 1 answers a kist with its alias: `spawn_kist` keeps the gate's developer key and option on the process (`Process.gate`, public values), and the kernel derives from those and the person key, never from anything the caller names; refused to a process the kist job did not launch, while nobody is present, and before the person key is handed in. `diag identity` says whether the person key is held, checks the kernel's derivation against fgtw's known answer, and counts aliases given; the flash log names each kist given or refused one, never a byte of it. `ferros_sys::alias`, `give_person_key`, `person_key_held`.
- **Identity service** (payloads/fgtw space.rs `space_person`): at each join once the first pull is done, opens the person key from the space with the fleet key, re-seals it when the space has it under an earlier fleet key and this service holds it, mints it when the identity has none (eggs: the kernel's entropy, this process's own timing jitter, the fleet key) and writes it into the space (to the fleet and fgtw.org's existing space slot, sealed), then hands it to the kernel whenever the kernel does not hold it (the kernel wipes it at every lock). It derives no alias.
- **Test kist**: payloads/aliastest, two capsules from tools/aliastest-kist.sh ("Alias A", "Alias B").
- **Tested** on the host (fgtw: the known answer, every field moving the alias, the bundle, mint's two-egg rule, seal and open thru the space's document, a shrink re-sealing, two mints settling on the first, a swapped wrap refused); the kernel and the identity service build.
- **On glass, the 8a, 2026-11-13** (kernel from this branch merged with forge d69fb2d5): before attest `diag identity` said no person key, a hand-over was refused, and the same code spawned as a plain process was refused ("not a kist launched by the kist job"); after attest, before any person key, Alias A launched by the kist job was refused. The 8a's identity service had no IPv4 that session (it never said "the network is up", its verdict grey), so it never held the fleet key and the space's half (open, mint, hand over) did not run on glass. The kernel's half was tested with a fixed scratch test key handed over by a development process with the Unlock right (never committed): Alias A and Alias B got different fingerprints, A the same twice, and both the same after a reboot and a fresh attest (the key wiped at the reboot, A refused again until it was handed over). Both fingerprints equal the ones computed on the host with `b3sum --derive-key` from the same test key, the ferros developer key and each option, so the kernel derives exactly `fgtw::alias::seed`.
- **Still to run on glass**: the identity service opening or minting the real person key, on a device that reaches fgtw.org. The glass test: `diag identity` before attest (no person key), `tools/aliastest-kist.sh plain` (refused: not launched by the kist job), attest, the identity service's "person key ... handed to the kernel", `diag identity` (held, known answer ok), `stow`, `launch a`, `launch b` (two fingerprints), a reboot and attest, `launch a` again (the same fingerprint).

### Built (2026-11-13: be you and people, steps 0 and 2 to 4, and std/photon's side of 5)

- **fgtw `binding`** (branch `facets`): `Binding { handle_proof, developer, option, alias, when, device, sig }`, signed by a member of the person's fleet with every egg the chain knows it holds (`declared_mask`), over a domain, the handle proof, the kist's identity, the alias bundle's commitment, the time and the signer, all fixed widths. A complete VSF file (`hP`, `ke`, `hp`, the bundle as fgtw carries one, `e`, `gm`). `verify(&chain)`: the chain's genesis is that handle proof, the signer folds as a current member, every egg verifies against the chain's bundle for it and covers its declared mask. `possession` and `holds`: the alias's own eggs (Ed25519 and Falcon-512, so the request fits one ferros call) over its bundle's commitment, so a binding is only made for a key its asker holds.
- **fgtw `people`**: `Person { handle_proof, identity, genesis, members, name }`, the pin-set as the space keeps it (wrapped `v'W'`), and `doc`/`from_doc`, the document a kist is handed, without fleets.
- **ferros_space**: `PERSON` (by handle proof), `PEOPLE` (a kist's people all or chosen, all while unset), `CHOSEN` (a person picked for a kist, a tombstone once unpicked); `people_for` gives a kist all of them or the chosen ones still the person's. Space-only facts, synced as every fact.
- **Rights**: `be_you` (10) and `people` (11) appended to ferros_kist's `RIGHTS`, both asked, their words in the sixteen languages (ferros_lang), each a send right on `facets`.
- **Kernel**: the server of `fgtw` gets the receive right on `facets` as the slot after its last (spawn and a keeper's restart). Every call on `facets` goes in behind the kernel's stamp (process.rs `facets_stamp`, ferros_sys::facets): a kist or not, which facets the person granted it at launch, the person present, its developer key and option, and its alias's Ed25519 key.
- **Identity service** (payloads/fgtw facets.rs): op 0 be you (the bundle must be the alias the kernel stamped and its eggs must hold it; signed in the loop, the call held until then, kept for the run; answered with the handle proof, the pinned identity key and the binding), op 1 the people. Only while the person is present and the service is a bound member.
- **The People page** (the map, a settings page after Fleet): the person's people with the name given each, a remove in two taps, "add a person". **The add-a-person sheet**: the attest screen's keyboard takes their handle as typed, never masked; the seed and the memory-hard proof are computed there, not in the identity service (the proof's scratch is 24 MiB, the service's heap 2 MiB), handed in and wiped at once with the text; the identity service pins the identity key the seed gives, wipes the seed, asks fgtw.org's existing lookup by the proof, takes the chain only when its genesis names that key, and writes the person with the name typed next. Nothing new on fgtw.org.
- **A kist's card**, once the person granted it people: all of them or the chosen, each person a checkbox.
- **std/photon**: its main asks the kernel for the alias, the identity service for the binding (checked to name that alias and handle proof) and the people, builds `PhotonKeys` (photon aa53a370, `legacy_seed` None) and holds them at the seam: Photon has no way in for them yet (`PhotonApp::set_photon_keys`, `set_people` wanted from the Photon session), so it runs on its own handle screen meanwhile. Photon's capsule wants be you (slot 8) and people (slot 9).
- **On glass** (the 8a, 2026-11-13, this branch's kernel flashed, fgtw, ahau, map, init and the stower stored): at attest the identity service minted the person key for the identity and handed it to the kernel; Photon launched from its circle asked be you and people once on the first-launch question, then "facets: be you to the kist … given" (the binding signed in the loop, 9955 bytes, about 10 s the first time, at once on a relaunch), "people … given" (0 of them), and "RESUME: PhotonKeys in hand; the vault opens under the messenger root": no handle screen. The People page and its add: a handle typed on the keyboard, its proof handed over and the text wiped, fgtw.org's lookup by proof, "nobody has this handle" for one nobody holds. Alias A and Alias B unstowed from the Deck.
- **Found on glass: an older stower drops the new rights.** The stower (payloads/kists) rewrites the person's list at an unstow with its own ferros_kist, and one built before `be_you` and `people` existed dropped both wants from Photon's entry, so the space took the answers as removed. Every process that writes the list (the stower, the map, init's reader, the identity service) must be from this branch before a kist declaring them is stowed; so must the identity service of every other device of the person's fleet, whose space join reads kist facts with its own vocabulary. `ferros-bridge kist stow <capsule> --stored` rewrites a list entry without sending a capsule the device holds already (Photon's 92 MB last part ran past the USB driver's 16 s on a busy vault).
- **Tested** on the host: fgtw (the binding: every egg, every field covered, a stranger and a departed member refused, eggs short of the declared mask refused, an Ed25519-only member, possession, the request's size, the document's hash; people: the space's form and a kist's without fleets), ferros_space (people cross the fleet sealed, a rename from a device whose clock runs behind wins, a removal sticks, a kist's all or chosen crosses the fleet, a removed person leaves every chosen list), ferros_lang and ferros_kist. The kernel, the identity service, the attest screen and the map build.

## Decisions

**Delegated** (owner, 2026-11-12: "We can discuss the security issues with handles and decide how to split it. use your best judgement."). Decided by Claude on that delegation, each open to the owner's change; the reasoning is the owner's threat model ("Claude memories/threat_model_harm_not_observation.md": design against harm and forgery, not against being watched).

- **D0. The person key.** Aliases derive from 32 random bytes minted at the identity's genesis (for an existing identity, by its first device that runs this), kept by the fleet under the fleet key, never from the handle. The deciding reason is forgery, not privacy: under the identity seed anyone who ever typed the handle could sign as the person's alias in any kist, which is harm; the cost is one more secret the fleet already knows how to carry.
- **D1. The kernel derives.** The owner's rule of 2026-11-07 stands: only the kernel issues keys. The identity service hands the person key to the kernel at unlock (Unlock right only) and the kernel wipes it at lock; a kist asks for its alias beside `SYS_IRA`, stamped by its gate identity.
- **D2. Accepted**: the alias and language, number base and time zone are implicit.
- **D3. Accepted** as written: "be you in Photon: your contacts find and recognise you there", a binding that vouches an alias in its kist and nothing more.
- **D4. Accepted**: pick, don't share, with the four sheets.
- **D5. Accepted**: the person's people live in the identity space, and a person is added only thru the system's sheet. Photon's contact list moves there on ferros with the migration, not before.
- **D6. The never list: items 0 to 4 kept** (other kists, the fleet, the identity's root, other kists' data, device identifiers): each is a lever for a swap or a forgery. **Items 5 and 6 kept for now but marked soft**: they guard against observation more than harm, so a kist that shows a real need may be given a coarse word or a fact as an asked right later.
- **D7. Accepted**: no public binding slot; bindings travel with what a kist sends.
- **D8. Leave the chain's public read as it is.** Seeing how many devices a person has harms no one; gating it is work for no gain under this threat model.
- **D9. Accepted**: "facet", "alias", "be you", "the person key", `PERSON`, `SPOT`.

**The split, in one line**: on ferros no kist ever holds the person's roots (the handle, the identity seed, the person key, the identity key, the fleet key, the ira); a kist holds its alias (derived by the kernel, its own), its ira key (per device, its own) and the facets the person handed it. Until Photon's migration lands (PHOTON.md), Photon on ferros keeps its own handle screen (option C there), so the roots it holds are only those the person typed into it, as on every other platform.

## The proposals as they were put

- **D0. What aliases derive from.** The identity seed (simple; anyone who knows the handle computes every alias) or a person key (random, minted at genesis, kept by the fleet under the fleet key, never from the handle). Recommended: the person key.
- **D1. Who derives them on ferros.** The kernel (your rule of 2026-11-07: only the kernel issues keys), or the identity service (the brief). Recommended: the kernel.
- **D2. The implicit set's two additions**: the alias, and language, number base and time zone. Taken as accepted 2026-11-12; confirm.
- **D3. Be you's words and reach**: "be you in Photon: your contacts find and recognise you there"; a binding that vouches an alias in its kist and nothing more.
- **D4. Pick, don't share** as the default way a kist gets one value, with the four sheets.
- **D5. Where the person's people live**: in the identity space (proposed), moving Photon's contact list there on ferros; and adding a person only thru the system's sheet.
- **D6. The never list**, item by item (0 to 6 above).
- **D7. No public binding slot**: bindings travel with what a kist sends, so nobody can ask which kists a person uses.
- **D8. The chain's public read**: leave it (anyone with the handle proof sees how many devices the person has) or gate it later. Not needed for facets; recorded because the never list's "your fleet" is only as private as the chain.
- **D9. Names**: "facet", "alias", "be you", "the person key", and `PERSON` and `SPOT` for the space's new kinds.
