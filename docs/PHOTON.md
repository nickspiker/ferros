# Photon on ferros

> **Superseded (owner, 2026-11-05):** Photon is one full app on every platform, ferros included: the real photon-messenger crate built as `std/photon` (STD-TARGET.md), running on the device since 2026-10-27. FGTW is attest, identity and the OS's sync. The `photon_core` code below is a reimplementation written before the real app ran here: nothing more is built on it, and it is kept only until the owner decides how to retire it (STATUS.md, "Photon and FGTW on ferros").

**The Photon app is not part of ferros** (owner, 2026-11-04: "Photon is just an app. on non ferros it just happens to have the FGTW bits in it too. Just like Mahere, or Lumis"). What this page built is the FGTW side, the identity and the fleet, and on ferros that is the OS's own: the process `fgtw` (payloads/fgtw), part of attest, started by init once a person attests, with the vault scope `fgtw.`. Photon the messenger is an app a person stows, and nothing of it is in the base OS. It was the process `photon` until the same day.

The device as a device of the owner's Photon fleet (Claude memories/photon_on_ferros_no_bootstrap.md; photon docs/pairing-v2.md "The ceremony over the mesh", docs/peers-are-fgtw.md Phase A′). Started 2026-10-26, owner: "the priority is allowing the new ferros device bind".

## What runs

- **`photon_core`** (no_std, host-tested, byte-pinned against the stock app by `tools/photon-interop`):
  - `wire.rs` — ping / pong / photon / ack frames.
  - `pt.rs` — Photon Transport over UDP: a frame up to 1024 bytes as one datagram with its packet ack; larger as a signed `pt_spec`, DATA chunks, chunk acks and a `pt_done`. Interop: a 30 KB frame device → app and 45 KB app → phone through Photon's own `PTManager`, plus packet acks both ways.
  - `join.rs` — the device's keys across the three families (Ed25519, Falcon-512, SLH-DSA-SHA2-128s), derived and signing exactly as fgtw's `SigningBundle`; the LAN beacon; the binding request and its `bind_req` envelope; `fleet_resp` / `fanout_resp` parsing, the chain folded against our roots (fgtw's fold), the fan-out opened with our ira; the 23 pairing words; eagle time from a UTC clock.
  - `fgtw` (the fleet substrate) and `voca` (the wordlist) build `no_std` since 2026-10-26 so the device runs the same fold and codec the desktop and the worker run.
- **`payloads/fgtw`** — the client process: beacons as the app does (`pt_disc` to 239.104.199.144 and ff02::68c7:9014, port 4384), hears the fleet's devices beaconing under the same handle, hands the nearest its binding request over PT, takes the chain and the fan-out back, keeps them in the vault, answers pings once bound. 128 KiB thread stack, 160 KiB heap in `.data` (sized to a 1 MiB arc).
- **Kernel**: `SYS_IRA` (23) with an Ira capability (spawn kind 9, `ferros-bridge --ira`) hands a process `blake3::derive_key("ferros process ira v1 " ‖ domain, ira)` — the Photon device key's root, the same after a factory reset ([[ira_network_owns_assignment]]); the vault scope `photon`.

## Identity and the vault

| key | what | from |
|---|---|---|
| ira-derived `fgtw device` | the device key's fingerprint | the kernel, every boot |
| `fgtw.identity_seed`, `fgtw.handle_proof` | the identity roots | by default NOT on disk: the attest screen hands them to the client in RAM (op 4). Stored only when `device.unlock_at_boot` is on (opt-in, warned); wiped from disk whenever it is off. Never pushed from another machine. |
| `fgtw.chain` | the fleet's membership chain as last adopted | a sponsor's `fleet_resp` |
| `fgtw.fleet_key` | the fleet key | a sponsor's `fanout_resp`, opened with our ira |

The handle itself never reaches the device: `ihi::handle_to_hash` runs over vsf's `x` text codec, whose 9 MB tables stay off the device.

## The join, as built

1. The person attests on the device (payloads/ahau, at boot): their handle, twice the first time, then the handle proof (6.5 s on the X3). The roots go into the vault; the client, started by init, finds them within two seconds.
2. The client prints its 23 pairing words on the console (device key masked to the fleet; public) and beacons every 3 s.
3. A fleet device heard beaconing → the client hands it the binding request (`bind_req`, about 10.7 KB over PT), re-minted every 210 s as the app re-posts.
4. The sponsor (a desktop on today's Photon) keeps the request, shows a toast, and lists the device on Settings → Fleet → Add device as a nearby candidate. The human types the words (or taps the candidate: the two-phase confirm).
5. The sponsor binds into the chain it holds and pushes `fleet_resp`; the client folds it against its roots and finds itself: bound. The sponsor grows the fan-out and pushes `fanout_resp`; the client opens it: the fleet key.
6. Bound, the client pings the fleet's devices every 10 s and answers pings with tail-less pongs.

Done to step 3 on the first night; steps 4–6 need the device's owner at one of their own fleet's devices.

**2026-10-27: the roots must be the device owner's, entered on the device.** On 2026-10-26 a session pushed the desktop's own Photon roots (the owner's identity) onto the husky with `tools/photon-interop` `roots` + `store put`; the husky is the owner's wife's device. The vault was wiped and the join is done properly: she attests on the device with her own handle and binds it from one of her own devices. The `roots` tool stays for interop tests on the dev box only, never for a device that belongs to someone (Claude memories/feedback_identity_belongs_to_a_person.md).

## Nothing on disk by default (2026-10-28)

The owner's rule: no form of the handle on this device. A stored identity seed is the identity's root key and a fast target — on a desktop Ryzen, a stolen seed checks `x`-encode + BLAKE3 at 45 M handle guesses/s across 32 threads (3.1 M on one), so a memorable two-word handle falls in seconds (the handle proof is the memory-hard wall, ~1 s/guess; the seed is not). The vault is sealed under the ira and ferros unseals it itself at boot, so a stored seed on a stolen phone IS the identity.

So by default the device stores nothing of the identity. The handle is typed each boot; the attest screen derives the seed and proof and hands them to fgtw (the identity service) in RAM (the `photon` endpoint's op 4, `[4][seed 32][proof 32]`); the client looks the identity up, and the fleet key is recovered from the published fan-out into RAM (`recover_fleet_key`). Wiped or stolen, there are no keys anywhere, all derived again from the handle and the ira.

Opt-in: `device.unlock_at_boot` (SETTINGS.md) stores the seed, proof, chain and fleet key, so the device unlocks without the handle being typed. The attest screen's entry carries the toggle and, when on, warns "if this device is lost, whoever holds it becomes you." Turning it off (or booting with it off) wipes every identity value from the vault (`Client::forget_disk`).

## The verdict and the seed (2026-10-28)

The attest screen no longer decides anything about an identity. A first entry derives the roots and writes them; the client looks the identity up (pairing-v2: peers first, the seed last) and says where the device stands, the `photon` endpoint's state byte: 0 no roots, 1 joining (the identity has a fleet; words shown), 2 bound waiting for the fleet key, 3 a member with the key, 4 looking, 5 fresh (fgtw.org holds no chain and no fleet device is heard), 6 wrong fleet (the seed's chain for the handle has another identity's genesis), 7 GREY (nothing answers; never a verdict).
- **The seed over plain HTTP.** fgtw.org serves port 80 (Claude memories/fgtw_plain_http_for_ferros.md). The client resolves it with ip's resolver (op 10 RESOLVE) and POSTs over the `ip` process's TCP (ops 4-7), as payloads/httptest does: `SeedHttp`, ferros's `fgtw::client::FgtwTransport`. Everything above the wire is fgtw's own, built without std (feature `protocol`, fgtw c5b779b): `fetch`, `ensure_member`, `mint_fleet_key`; the verdict is `fgtw::attest::attest_verdict` (e1deb7d), the function Photon's probe runs too. fgtw's clock is set from the kernel's UTC before each call, its entropy is the kernel's stream (grant slot 5).
- **A beacon under our handle** proves the fleet exists, whatever the seed said: the verdict becomes joining.
- **Mint** (op 2, the attest screen's after the permanence warning and the retype): fgtw's `ensure_member` (genesis signed by this device and the identity, published, fetched back; only a chain naming this device makes it a member, the seed's slot settling on one genesis), then `mint_fleet_key` with this device as the only member.
- **Checked live 2026-10-28** on the husky with its owner's roots and no fleet device heard: the client resolved fgtw.org, fetched her chain over plain HTTP, verified and folded it, and said "this identity has a fleet". Mint is not tested against fgtw.org: a test mint is a permanent identity.

## Leaving the fleet (2026-10-31)

Removal is bilateral (photon docs/pairing-v2.md "Chain changes"), the same as in the app's sign-out. The frames are fgtw's (`fgtw::protocol`), shared with the app.

**Revised 2026-11-10 (owner), for ferros and the Photon app alike; not built yet.** The design below replaces the words of the 2026-10-31 version (three approval words typed on a surviving device). Words carried no key material and selected nothing a signature did not already settle; the request is signed by a current member's key, checked against the chain. The transports and their order are AUTH.md's "Joining and leaving a fleet".

1. Op 5 `[5][intent]` on the `photon` endpoint starts the departure. Intent is 1 (to a new owner: the approver also releases the device, so another fleet can bind it) or 2 (to the desk).
2. The client builds this device's consent eggs over `fleet::departreq_signing_bytes` with a fresh salt in what they sign, and the signed `depart_req` carrying it. The salt names this request: one denied is never accepted again, so asking again means a new request with a new salt.
3. The `depart_req` goes to every fleet device: an NFC tap, a BLE broadcast, the LAN, directly over the FGTW peer network wherever each device is, and fgtw.org's slot last for a device offline.
4. The state reads 8 (leaving). The leaving screen says it is asking the person's devices: tap one, or approve on any of them. No words, no reachability lines.
5. Every fleet device that receives it notifies the person (ferros and the Photon app), and the notification opens the Fleet page with the request on it: which device, and **Approve** or **Deny**, ready. An NFC tap from an attested fleet device with its screen on is Approve.
6. Approve publishes the consented Remove. Deny records the request's salt as denied and tells the fleet, so every device clears the request and refuses that salt; the leaving device hears it, says so, and a new request takes a new salt.
7. The leaving device checks the seed's chain every 16 s, and any chain a sibling pushes. Once the chain folds without this device, the identity's roots, chain and key are forgotten here.

**Since 2026-11-10 both halves run on fgtw's shared machine** (`fgtw::flow::depart`, the one Photon runs; payloads/fgtw/src/depart.rs carries out its actions). What it adds here: the identity's last device cannot ask to leave (op 5 answers 4, the Fleet page says so in `fgtw::text`'s words), a frame given up on is handed again when its device is next heard or a network comes up, and an approval by NFC tap reads the leaving device's tag, which serves the request's salt while it stands. An approval here hands the leaving device the new chain at once (`fleet_resp`); its 16 s read of fgtw.org's chain stays for an approval the push cannot reach (another network: ferros has no relay). A request its device gave up (its space entry gone) stops being asked. The machine's state is kept as `fgtw.depart` without the handle proof; a departure kept in the form before it (the signed request alone) is taken up with the same stamp, salt and frame, and the old denied list (`fgtw.denied`) is read into it once and deleted.

Verified on the host: photon-interop shows the app parsing the device's `depart_req` and its consent verifying under every scheme the device holds. Not yet run against a live approver.

## Photon's device key on ferros (2026-11-12)

Photon, the stowed kist, took its device key from the machine fingerprint (photon `get_machine_fingerprint`), and on ferros, with no files, that fell to tohu's last resort: the bytes `unknown`.
So every ferros device's Photon had one device key (the same Ed25519, Falcon and SPHINCS+ keys), and anyone could derive it.
A ferros Photon in a fleet would have been a member anyone could sign as, and the fleet key wrapped to it anyone could open.
Its capsule declared the ira "to tie its keys to this device", and nothing read it.

Now std/photon's main derives the key from its ira grant (`ira_key("photon device")`, slot 6) and hands it to Photon before it starts (`set_device_keypair`, `set_signing_bundle`), as Android's shell hands in its own.
The kernel binds a kist's ira keys to the kist's gate identity (KISTS.md, "A kist's ira keys are its own"), so the key is Photon's on this device: no other kist and no system service derives it, and it is not fgtw's device key (`fgtw device`).
A person who says no to the ira gets no Photon: it says so on the console and ends, as it did before with every want off.
Whatever Photon kept on a ferros device before is under the old key and is not read again (development data only).

## Photon and the person's identity (proposal, 2026-11-12: the owner's decision)

**Answered the same day:** the owner took the facets model (FACETS.md) over A to C below: no kist is handed the roots, and Photon signs as a key of its own, its alias, on every platform. None of A to C as written: Photon moves to a key of its own on every platform, the migration below; the plan is "Photon's own key: the migration" below.

**Today.** The person attests at boot, and fgtw holds their roots in RAM (op 4).
Photon, launched from its circle, is told none of it and opens on its own handle screen: the person types the handle a second time, Photon computes the proof again (4.7 s on the X3), and nothing on ferros stops Photon attesting an identity other than the device's.

**What Photon needs to open on its contacts** is exactly what fgtw holds:
- `identity_seed`, which Photon also uses as its `vault_seed` (tohu's `SessionIdentity`; photon handle_query's derivation). ahau's seed is the same bytes (`ahau_core::identity_seed` is tested byte-exact against `ihi::handle_to_hash`).
- `handle_proof`, which is public.

It needs no handle: Photon never shows it and drops the string after the proof.
It needs neither the ira nor the wairua, and no fleet key (it recovers that from the fan-out with its own device key, above).
On Photon's side the change is one place: on ferros `tohu::session()` answers with the system's roots, and Photon's resume (photon driver.rs, "Auto-resume from the remembered session roots") paints its contacts from them, with no handle screen.

**Why it is the owner's call.** The identity seed is not a key derived for an app; it is the identity's root.
0. The identity's signing key is `Keypair::from_seed(identity_seed)` (fgtw `genesis_identity_pubkey`). Whoever holds the seed signs chain changes as the person: adding a device to their fleet, for good.
1. PORTAL.md, section 8: "An app never signs as the person."
2. A seed gives up its handle to an offline guess in seconds ("Nothing on disk by default", above: 45 M guesses/s), so the handle goes with it.
3. Photon is the most exposed process on the device: it parses what strangers send (pictures thru `image` and rav1d, PDFs thru hayro, sound thru symphonia, VSF off the network).
4. An answer is kept per kist (developer key and option), so it survives updates: whoever holds Photon's release key (a commons key, KISTS.md "Whose key signs a kist") can ship a build that inherits the right on every ferros device whose person said yes.

Against that: Photon holds these roots on every other platform (tohu's session, the same 96 bytes), and the owner's scheme has attest unlock "Photon's keys" (STD-TARGET.md, "Encryption, as the owner laid it out").

**The options.**
- **A. Hand Photon the roots.**
  - A right `identity` appended to ferros_kist's `RIGHTS` (code 9), its words "to be you" in the 16 languages (ferros_lang).
  - The kernel's `kist_object` makes it a use of a new endpoint `identity` served by fgtw. Never the `fgtw` endpoint itself: its holders can mint, replace the roots and leave the fleet.
  - Op 0 → `[state][identity_seed 32][handle_proof 32]` while fgtw holds roots, else `[state]` alone (the state as op 1 gives it).
  - std/photon's main asks once at start and puts the roots in tohu's session, given a ferros arm that keeps them in the process's own memory (gone with the process). Never written to Photon's vault.
  - Photon's capsule wants it: "to be you, so your contacts open without your handle typed again".
  - Risks 0 to 4 above, taken as on every other platform.
- **A′. A, with the answer to `identity` kept per capsule** (its blake3), not per kist, so each update of Photon asks this one right again. A question per update; closes risk 4.
- **B. fgtw signs for Photon.** Photon would hold what is derived from the seed but not the signing key, and ask fgtw to sign as the person. Not possible as Photon is built: about 600 places derive straight from the seed (contacts' vault keys, avatar and blob names, word masks, cloud keys), and each must match the person's Photon on their other devices, so ferros alone cannot hand Photon a sub-key instead. It needs the derivation split on every platform: a Photon project, not a ferros one.
- **C. Keep Photon's own prompt** (today). Read-only, fgtw could serve the public proof under a right of its own, so Photon refuses a handle that is not the device's person. A second typing of the handle stays.

**The device key, under A.** Photon's device key is its own (above), not fgtw's.
So Photon is a second member of the person's fleet on the same device: resumed with the roots, it finds its key outside the chain and asks to join (its KnownHandle screen), once, approved on a fleet device.
The other ways, also the owner's: Photon signs device envelopes thru fgtw (one member per device, a new device signer in Photon), or the Fleet page shows Photon's key under its device (people_not_devices: a device is named from its key).

**Recommendation:** A′, Photon a member of its own, approved once.
The build is the pieces in A plus init's grant of the new endpoint to fgtw; tohu gains its ferros arm, and Photon's library is not touched.

**Built:** the device key alone (above). No kist is handed anything of the identity.

## Photon's own key: the migration (proposal, 2026-11-12)

Status: a plan for the owner, nothing built.
The decisions are gathered at the end (M0 to M7).

The owner's direction (2026-11-12, FACETS.md "How this came up"): a kist acts for the person in its lane and is never them; the line is the key.
For Photon that means: Photon gets its alias (a key for Photon alone, the same on every device of the person's), the person's public identity (be you) and their people, all or chosen; never the identity seed, the handle or the fleet.
And because Photon is one app on every platform ("Claude memories/feedback_photon_is_the_full_app.md"), it moves onto its alias everywhere, not only on ferros: a ferros Photon on the alias and a desktop Photon on the seed would be two different people to every contact.

### What Photon holds, after

- **On ferros:** its alias (the kernel derives it and hands it to Photon alone, FACETS.md "The alias"), its device key (its ira key, as built above), the person's handle proof and its binding (be you), the person's people (people). No handle screen: Photon opens on its contacts.
- **On Android and the desktop:** Photon is also the identity service there (it carries FGTW), so its FGTW part keeps the root as today (tohu's session) and does the fleet's work. Its messenger part holds exactly what ferros's Photon holds, the alias derived by Photon itself with the same function and the same constant for Photon's kist identity (M1), so the bytes are the ones ferros's kernel hands out.
- So the messenger code is the same everywhere: it takes `PhotonKeys` (the alias, the handle proof, the binding) where it takes the session's `identity_seed` today.

### What the seed does in Photon today

The "about 600 places": lines in photon's `src` that name `identity_seed` or `vault_seed`, 716 in about 50 files, some 23 of them tests (photon 4bf11856, 2026-11-12).
Photon sets `vault_seed = identity_seed` (handle_query's FirstAttest; launch.rs), so both are the one seed.
Grouped by what they derive (the groups overlap a little; the counts are a size, not a census):

| | what the seed gives today | where | lines | after | work |
|---|---|---|---|---|---|
| 0 | **the party id and the friendship secret**: `identity_party_id`, Ed25519 of the seed, the key friends pin and the id that sorts party slots and keys conversations; `identity_friendship_secret`, the static identity DH, the 21st CLUTCH egg; siblings' ceremony mixing the shared seed | crypto/clutch.rs; ui/photon_app status, render, molecules, sync, conversation, group_wave | 115 | the pinned party id stays as the identifier (M2); new signatures and new ceremonies' DH by the alias; siblings mix the alias (it is the same on each) | medium |
| 1 | **the fleet** (FGTW's work, not the messenger's): the genesis identity key and the binding request's identity signature, the fan-out KEK's seed half, the pairing words, devices' default names, sponsoring, departure, root conversion | network/fgtw/fleet.rs, bootstrap.rs; ui/photon_app devices, root_conversion, ceremony, bonds | 160 | stays on the root, as the identity service's. On ferros the fgtw process does it already; elsewhere Photon's FGTW part. The change is a boundary: the messenger never calls these | small to medium |
| 2 | **vault addresses**: `vault_key(domain, seed)` for each contact's entries, drafts, the booted record, molecules, fleet settings | storage/*.rs, network/handle_query.rs | 220 | from the alias; each device moves its entries once (root_conversion's pattern: read under the old address, write under the new, delete the old) | medium |
| 3 | **blob names**: the keyed addresses of attachments, recordings, kept beams (`blob_name_key_of`) | storage/mod.rs, wave/* | 66 | the alias; the same one-time move | small |
| 4 | **the person's own avatar**: its wall address, the 64-byte avatar pin every contact keeps, its encryption key | ui/avatar.rs, avatar_scoped.rs, group_avatar.rs | 130 | the alias; the new pin rides the rotation to each contact (or identity-profile.md's profile grants, stage C, replace it) | medium |
| 5 | **the roster backup**: `contacts_storage_key` and `contacts_encryption_key`, the seed with the fleet key, for the fleet's contact copy at fgtw.org | storage/cloud.rs | 35 | a key from the alias, which every Photon of the person's holds (ferros's included, which holds no fleet key); a slot of its own, written under the alias's signature (M5) | medium to large: a worker slot |
| 6 | **notes to self**: the self atom's era 0 root and history key | ui/photon_app/molecules.rs | 2 | from the alias, as era 1 | small |
| 7 | **submitted logs**: their key and retrieval tag, from the handle on purpose, so a maintainer handed a handle can find and open that person's logs | lib.rs, network/fgtw/blob.rs | 5 | M6 | small |
| 8 | **plumbing**: the session's seed passed down to all of the above | handle_query, launch, driver, input, settings, photon_app.rs and others | 120 | `PhotonKeys` in place of `SessionIdentity` in the messenger's code; the compiler walks every site | medium, mechanical |

**The size of it.** About three quarters of the lines go by a type change the compiler walks (groups 2, 3, 4, 6, 8).
The design is in three places: the rotation (group 0, below), the roster's slot (group 5) and the line between the messenger and FGTW (group 1).
It is a Photon project, done in the photon repo with ferros's three pieces beside it ("ferros's part").

### How contacts keep trusting the person

**What a contact holds today** (photon docs/identity-profile.md "The pin-set"): the handle proof, the pinned identity key (the party id, Ed25519 of the seed), the pinned genesis hash, and the chain's folded member set; a photon is taken from a device in that set (molecules.rs: "the caller still holds the signer to the member's trusted devices").

**The rotation** (proposed). Each of the person's Photons tells each contact, over the friendship they already have (so authenticated exactly as photons are today), "my Photon key is this alias":
- the **binding**: "this alias speaks for this handle proof in Photon", signed by a device of the person's chain with every egg (FACETS.md "Be you"). This is the signature that carries the weight: as in photon's succession design (identity-succession.md: "Its trust anchor is a device key from the old chain"), a device the contact already trusts vouches for the new key, and nobody who only knows the handle can make it;
- the **alias's own signature** over the same bytes (it holds the key it names);
- the **old identity key's signature** too, for continuity with the pin; worth little alone, since anyone who knows the handle can make it.

The contact checks the binding's signer against the chain it pinned and the alias's signature, and keeps the alias beside the pin.
From then on records the person publishes (the profile, the avatar wall, their sovereign records: "the subject signs") are signed by the alias, and new ceremonies with them use the alias's DH.

**Photon's devices** (proposed, M3): the alias signs a list of Photon's device keys, one per device of the person's where Photon runs, and a contact takes photons from a device in that list.
- On the desktop and Android Photon's device key is the device's own member key, as today, so it is in both the chain and the list.
- On ferros Photon's device key is its ira key (built, above), in the list only: never a member of the fleet.
- Why not A′ (Photon a member of the fleet, above): a member can sponsor devices into the fleet (fgtw: adding a device takes a sponsor in the folded member set), so a Photon that is a member can make any device the person's. That is being the person, which is what facets keep from every kist.

### The order: readers, then speakers, then the cutover

0. **fgtw first**: the alias derivation with known answers (so the kernel and Photon on every platform derive the same bytes), the binding, the device list, their checks; host-tested. Under M0's person key, the identity service on whichever device of the person's runs it first mints the person key and seals it into the fleet's state before any alias is derived.
1. **Readers everywhere**: every Photon, on every platform, learns to take a rotation, check it, keep the alias and take photons from a device in an alias-signed list, while still speaking only the old way. Released before anyone speaks: nothing has changed yet for any contact.
2. **The person's fleet speaks.** Once every device in the person's fleet runs a reader (each says its Photon version in fleet presence, as siblings already trade state), the first to see it sends the rotation to each contact and publishes the device list. Each sibling, seeing itself named, moves its local entries once (groups 2, 3, 4, 6) and signs new records with the alias. The alias is the same everywhere, so no sibling waits on another's key.
   - ferros's Photon comes up on the alias from its first launch (the kernel hands it in), so it is a speaker from the start, named in the list as soon as it or a sibling publishes it.
   - A device that is off (one in a drawer) is not in the list until it runs and updates; until the cutover its photons the old way are still taken.
3. **The cutover.** When every member of the person's fleet has signed into the alias's device list, the list says so, and every contact stops taking that person's photons the old way (M4).
   - A device still on an old Photon after that is refused, and shows "update Photon on this device to keep talking" (the Deck's update, the pandok listing the newer version). No device is left speaking an old key unheard: it is told.
   - A contact who never takes the reader release cannot hear the person after the cutover; the person's Photon shows that contact as needing an update. The network is small, and the owner knows his peers.
4. **The seed leaves the messenger.** Groups 0 and 2 to 8 take `PhotonKeys`; the root stays only in Photon's FGTW part, on the platforms that have one. On ferros std/photon's main hands `PhotonKeys` in before Photon starts, as it hands in the device key (`set_device_keypair`), so tohu needs no ferros arm for the session.
5. **The old way's reader** (M7): photon's own precedent keeps a reader of a signed binary record (identity-succession.md "Sunset decision": the v1 verify path stays), so taking the old way could stay behind the cutover as a reader, or go after a census.

### ferros's part

- **Kernel**: the alias call (FACETS.md "To build" 1).
- **Identity service** (payloads/fgtw): the `facets` endpoint, the binding and the people (FACETS.md "To build" 2).
- **std/photon**: its main asks the kernel for the alias and the identity service for the handle proof and the binding, and hands them in; Photon's capsule wants be you and people (its words in KISTS.md "The permission list").
- **Nothing to move on ferros devices**: whatever Photon kept there is development data, as with the device key above.

### Decisions for the owner

- **M0. The root** (FACETS.md D0): the identity seed, or the person key. Either way Photon on the desktop and Android derives its alias itself: from the session's seed, or from the person key its FGTW part opens with the fleet key.
- **M1. Photon's kist identity, fixed**: the alias derivation names Photon's developer key and option as first stowed, as a constant on every platform, and keeps it if the release key ever moves (KISTS.md "Gap: custody moving").
- **M2. The party id**: keep the pinned identity key as Photon's identifier for a person, so friendship ids and conversation tokens stay and no friendship is made again (recommended); or move it to the alias, a flag day like identity-profile.md's stage A, every friendship re-made.
- **M3. Photon's devices**: vouched by the alias's device list (recommended), or members of the fleet (A′).
- **M4. The cutover**: contacts stop taking a person's photons the old way once every device of that person's fleet is in the list.
- **M5. The roster's slot**: a new fgtw.org slot for the alias-sealed contact copy, and the fleet-key copy retired.
- **M6. Logs**: keyed from the handle as today (a maintainer handed a handle finds that person's logs), or from the alias (nobody finds them unless the person hands over the tag).
- **M7. The old way's reader**: kept behind the cutover (photon's precedent), or removed after a census.

## Findings

- **Multicast needs a join.** The device heard another LAN device's beacons but never the dev box's; broadcast arrived, IPv4 multicast did not. The WiFi firmware passes all multicast; the access point's IGMP snooping was the filter. `ip` op 8 JOIN sends an IGMPv2 membership report (IPv4) or an MLDv1 listener report with the Router Alert option (IPv6) now and every 60 s; the client joins both beacon groups at start.
- **The arc pool.** 8 MiB held four guarded 1 MiB arcs; a fifth process was refused. Moved to 16 MiB at G#8300_0000 (the heap's second region shrank to 10 MiB). An app heap region in the kernel is the next step: the client's chain and PT buffers live in a 160 KiB `.data` heap only because its code is 660 KB and the arc must stay under 1 MiB.
- **The `ip` queue.** 8 datagram slots lost most of a PT blast; 64 in `.data` now.
- **Console.** The display's per-second stats line alone filled the kernel's console ring; it is ten-secondly now.
- **The dev box's firewall** drops unsolicited inbound UDP on WiFi (confirmed: nothing the device sends to a spare port arrives; wired devices' multicast does). The device's `bind_req` to the desktop's 4383 dies at the door until the desktop has sent to the device's 4383 first — which is why the desktop now answers a joiner's beacon with a unicast beacon from its main socket; but the beacon that would trigger that is itself dropped. On this box the fix is the owner's: `sudo firewall-cmd --add-port=4383-4384/udp` (add `--permanent` to keep it).
- **The desktop's log** is `/tmp/photon-nick/photon.log.vsf` (`cargo run -q --bin photonlog -- <path> -g LAN` in the photon repo).

## Next

- The owner binds the device from the desktop (step 4); then presence (step 6) is the first visible result on the Fleet page.
- Notes-to-self: a `hist_page` to a sibling sealed under the fleet epoch key (epoch from a sibling's `ckpt_state` push), the conversation token derived locally.
- The pairing words on the device's screen (the display server's text op), not only the console.
- An app heap region; the client started by init at boot once bound.
- The Photon side: attest degrading to the held chain when fgtw.org is down; `fstate` over the mesh.
