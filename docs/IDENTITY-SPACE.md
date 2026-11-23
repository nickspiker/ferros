# The person's identity space

Status: designed and built 2026-11-08 (host-tested; on the device once the owner reboots and attests, below). Owner, 2026-11-08: "when I attest, the key matches, it syncs with my fleet, pulls a kist roster and then fetches the respective capsules from fgtw.org or my fleet … and throws it on my home screen in the same exact spot and rotation it was before I factory reset this thing." And: "on all my ferros devices the homescreen layout should sync, unless of course I go into settings and uncheck that."

That day the Pro's vault process store was wiped by a bug, and the kists, their answers and the arrangement went with it: nothing of the person's was anywhere but that one device. This is the fix for that class: whatever is the person's lives in their identity space, and every device of their fleet holds it.

## What is in it

One set of typed facts per person (fgtw::space, ferros's vocabulary in ferros_space):

| Kind | Which | What | This device's copy |
|---|---|---|---|
| KIST | developer key, option | capsule (blake3), name, wants in slot order with their reasons | a `kist` section of `kists.list` |
| ANSWER | developer, option, right | granted or not; stamped when it was given | a want's answer in `kists.list`, or (an unstowed kist's) its `kept` section: a kept answer is a fact like any other since 2026-11-12, so one the person forgot on the unstow sheet goes from the space and every device (KISTS.md "The unstow sheet") |
| DEFAULT | right | on or off for new kists | `kists.list`'s `defaults` |
| PLACE | scope, circle (settings, or a kist by developer and option) | x, y, radius, turn | `map.place.<label>` |
| VIEW | scope | x, y, turn | `map.view` |
| DECK_SYNC | a device key | on or off | none: set from the Fleet page |
| ZONE | the fleet | automatic, and the zone by hand (an IANA id) | none: the identity service writes it to the kernel of each device that follows it (docs/TIME.md); chosen on the clock page |
| LANG | the fleet | the language (`wl`, its ISO 639-3 code) | none: the identity service writes it to the kernel of each device that follows it (SETTINGS.md); chosen on the language page |
| BASE | the fleet | the number base (`u`, tatau's code: 10, 12, 16, G#8C) | none: the identity service writes it to the kernel of each device that follows it (SETTINGS.md "The number base"); chosen on the units page. Its codec, and the language's, is fgtw::prefs, which Photon reads too |
| DND | the fleet | do not disturb, on or off (`u`) | none: the identity service writes it to the kernel of every device (preference 40, SETTINGS.md); switched on the quick row (QUICK.md) |
| PIN | developer key | the commitment of the key bundle that developer's first kist carried (`hp`), and their name when it gave one | a `pin` section of `kists.list` (KISTS.md "Trust on first stow") |
| PERSON | handle proof | one of the person's people (fgtw::people::Person): the pinned identity key, the genesis, the fleet it folded to, the name the person gave them | none: the identity service serves it (the People page, a kist granted people; docs/FACETS.md "People") |
| PEOPLE | developer, option | a kist's people: all, or the chosen (all while unset) | none: the kist's card |
| CHOSEN | developer, option, handle proof | a person picked for a kist (a tombstone once unpicked) | none: the kist's card |

The scope of a place and the view is the fleet's on every device whose deck sync is on (the default) and the device's own key on one switched off, so that device keeps its own arrangement, and gets it back after its own reset too. Kists, answers, defaults and pins always follow the person. A pin carries no stamp of its own into the sync, so a device that pinned another bundle for a developer the space already pins takes the space's: the first pin to reach the person's space stands (two pins made apart and merged before either device synced are settled by the space's merge, the later write).

Not in it: device firmware (below), the wallpaper (made per device, VAULT-OUT.md "First use"), the identity's roots (identity_not_on_disk_default), anything of a kist's own domain (its data syncs, if ever, as the kist's own business).

Next to add when they exist: the WiFi networks (`wifi.cred`; a credential carried by the space reaches a device only once it is online some other way: cellular, or a sibling over Bluetooth).

## Conflicts

- **The merge** keeps, per fact, the write greatest by (Eagle time written, writing device's key, gone over present, value bytes): a total order, so merging is commutative, associative and idempotent, and two devices that have seen the same writes hold the same space whatever order they came in. A removal is a tombstone and sticks.
- **A change on a device** is stamped past the write it replaces (a Lamport step), so the person's latest action wins even against a sibling whose clock runs ahead. An answer keeps its own time.
- **The device's copy carries no stamps**, so each device keeps a baseline (`fgtw.space_base`, never sent): what its copy held when it last agreed with the space. A value that moved from its baseline is the person's change here; one that did not takes the space's. A value with no baseline (a fresh device, a stow before the first sync) takes the space's when the space has one, unless the fact carries a later stamp of its own.
- **A locked vault is never an empty one**: a read the vault refuses fails the sync, and nothing is written or removed.
- **A fresh device joins nothing before its first pull** (fgtw.org answered, or a sibling's copy arrived), so its empty copy and its all-off defaults never go out as the person's choices.

## Where it travels

- **Kept** on each device in the identity service's scope, the person's class: `fgtw.space`, `fgtw.space_base`. Not roots, so kept whatever unlock-at-boot says.
- **Fleet first**: `space_push` (the sealed space) and `space_want` (a digest; answered only by a sibling that holds something else) between the fleet's own devices, over the LAN through Photon Transport, from members only. Sealed XChaCha20-Poly1305 under a key derived from the fleet key, the handle proof and the key's fingerprint bound in.
- **fgtw.org keeps one copy**, the backup a total loss recovers from: a new fleet-sealed slot `space_put` / `space_get` beside `fstate` (fgtw-bootstrap: the two slots are one implementation; member-gated write, monotonic stamp, 1 MiB, public read of ciphertext). Chosen over riding inside `fstate`: that slot is Photon's roster and settings, written whole by every Photon client, and an older client would drop a section it does not know, and the fstate codec needs std (`x` text) that the device's no_std identity service lacks. A put is always pull, merge, put, so a sibling's copy there is never written over. Until the worker is deployed the seed answers `unknown_op`, which the device says once and treats as "no copy there": the space then lives on the device and its fleet.
- **Rotation**: a copy sealed under a fleet key since rotated does not open; each device re-seals its own copy under the new key on its next push.

## A rotation that left the person key behind (2026-11-17)

Measured on the Pro (boot b925, console in /mnt/Harbor/tmp/scroll-R/runs/b925-attest.log) and on the desktop's Photon log.
- 21:44:04 UTC the desktop Photon opened the person key (id 22415a56) with the fleet key it started with.
- 21:44:28 its shrink heal (Photon devices.rs, the fan-out held more wraps than members after the Pro's removal and re-add on 2026-11-10) minted a new fleet key, re-sealed its own `fstate` from the old key to the new, and overwrote the old key. The person fact in the identity space was never re-sealed.
- The Pro attested later, recovered the new key from the fan-out, and could not open the person fact: fgtw's settle waits (`WaitingElsewhere`) and never mints over it, so no person key reached the kernel and Photon sat on "Waiting for your identity".
- fgtw.org's space copy, sealed under the old key, did not open on the Pro; the Pro then pushed its own copy over it (48 facts, sealed under the new key). The person fact rode in it untouched, still sealed under the old key, so fgtw.org holds it; the previous document as a whole is gone.
- The old key survives only on a device that has not synced the fan-out since 21:44:28 (the MacBook or the Android, if Photon has not run there), or in dead blocks of the desktop's vault (reflink copies taken 14:34 PST).

Three gaps, none fixed yet:
- G0. **Every key change must carry the person fact across** (Photon, the shrink heal and every mint; and any device replacing its key with a newer one from the fan-out while it still holds the old one): open the person fact with the old key, seal it under the new, push the space. The heal for today is the same code run on a device that still holds the old key.
- G1. **fgtw.org takes a space sealed under any key** (fgtw-bootstrap `handle_slot_put`): it checks the signature, membership, scheme floor, an optional `prev` and a rising stamp, but never the space's `kfp` (in the clear) against the current fan-out's. It should refuse a space sealed under anything but the current key.
- G2. **A device pushes over a copy it cannot open** (payloads/fgtw space.rs `space_seed`): a copy that does not open still settles the first pull, and the push sends no `prev`. A copy that does not open should hold the push until the device holds the key it was sealed under, or a newer copy arrives.

## At attest

1. The identity service (payloads/fgtw) has the chain and the fleet key.
2. It loads its kept copy, asks the fleet's devices heard (`space_want`) and fgtw.org (`space_get`), and merges what comes.
3. Once that first pull is done, every 4 s it joins the device's copy (ferros_space::sync): the person's changes here into the space (and on to the fleet and fgtw.org), the space's into `kists.list`, `map.place.*` and `map.view`. A kist that arrives in one pass has its answers and its circle's place in the next.
4. The map asks the generation once a second (fgtw op 17) and reads its circles and the view again when it moves, so the deck comes back in the same spots and turns, live, without a restart.
5. fgtw.org is asked again every 64 s for a sibling's change made out of earshot.

The Fleet page has the switch: "deck: the same on all your devices" / "this device's own" (fgtw op 18), shown once the first pull is done.

## Capsules (built 2026-11-10, the fleet source 2026-11-23; docs/KISTS.md "The stower")

A kist in the person's list whose capsule this device lacks is fetched by its blake3, checked by the gate, and kept in `capsules.`; its answers came with the list, so nothing is asked again. The fetch takes sources in order, each a small trait (`fetch(blake3, offset, buf)`, `size(blake3)`), so a source is added without touching the rest:

1. this device's own `capsules.` (a stow that survived);
2. a fleet device that holds it (a `capsule_want` over Photon Transport, answered in pieces from its `capsules.`);
3. fgtw.org, once it serves capsules: its blob slots cap at 1 MiB and Photon's capsule is 106 MB, so that is an owner decision (R2 by blake3, public, the gate's signature is the trust);
4. development: the bridge (`ferros-bridge kist stow`, which keeps the answers of the same developer and option).

Whatever arrives goes thru the stower's stages (payloads/kists): its blake3 and size, every egg against the developer bundle it carries and that bundle the one the person's list pins for its developer (KISTS.md "Trust on first stow"), then the kernel's checker over its masked code and the seal, which is all a launch checks. The stower holds `capsules.` (write); the map and the identity service never do. It follows the identity service's doorbell: when the space's generation moves, it fetches what the list names and this device lacks, from the person's other devices first (source 2, "The fleet source" below), then the kist's recorded source (`source`, carried in its KIST fact), then the directory's entry, then the directory's host by blake3. Until a capsule is back its circle is on the map and a tap is refused by the kernel (no capsule); `ferros-bridge kist stow <capsule>` still restores a development capsule with its answers.

### The fleet source (built 2026-11-23, not yet run on two devices)

A capsule from the person's own devices, before any server (owner, 2026-11-10: "BLE, then LAN, then WAN, then hopefully we shouldn't need to but fgtw.org").
- **The frames** (the fgtw crate, protocol.rs, 51cf36c): `capsule_want` (hp, the capsule's blake3 `cap`, the offset `at`), `capsule_piece` (hp, cap, at, the whole length `len`, the bytes `v`), `capsule_none` (hp, cap), each signed with the device's Ed25519 key. Each is taken only from a member of the chain under the same handle proof, not locked out.
- **Asking** (payloads/kists `fleet_get`, payloads/fgtw op 38): the stower asks fgtw for a piece at an offset; fgtw sends a `capsule_want` over Photon Transport to the person's devices heard in the last two minutes (on this network and any heard beyond it: ferros has no WAN peer path of its own yet), the device that served the piece before it alone after the first, and holds the call. A piece answers it; every device out (a `capsule_none`, or its frame given up on by PT) answers nobody at once; a device that takes the frame and never answers (an fgtw from before `capsule_want`, the Photon app) ends it at the asked devices' time, about 8 s (`capsule::ASK_US`, the space's fan-out ask's rule). Each piece goes into the same body as an HTTP fetch's (`ferros_kist::fetch::Intake`, hashed as it passes, into `capsules.` in vault pieces, the last held back), so the checks are the same: the blake3, the size, the eggs against the bundle the list pins, the kernel's checker and the seal. Nobody, or a broken transfer, and the restore goes on to the next source.
- **Answering** (payloads/kists `fleet_answer`, fgtw ops 39 and 40): the stower holds op 39 on a thread of its own; a member's `capsule_want` is handed to it, it reads 2^16 bytes of `capsules.<blake3>` from the offset (or finds none), and hands them back with op 40, which fgtw signs and sends to that member. fgtw holds no `capsules.` grant: init's grant page is full (payloads/init, slot 108 the last), and the stower already holds the scope. Until a stower has held op 39, fgtw answers every `capsule_want` with none, so an older stower never stalls an asker.
- **The pieces**: 2^16 bytes, one `capsule_piece` frame each, which PT carries in its own BLAKE3-checked chunks; one round trip a piece (Photon's 106 MB capsule is about 1600 of them).
- **Host tests**: the frames' round trip (the fgtw crate, `capsule_frames_round_trip`); fgtw's bookkeeping (payloads/fgtw/src/capsule.rs, pure, run thru a scratch crate as route.rs's are): a piece only from an asked member at the asked offset and within its length, nobody at once when every member is out, nobody at the time for a silent one, a none from the device that served asks the fleet again, wants held for the stower and handed once.

**The two-device test** (waits for the phones). The Pro and the 8a hold different identities, so the 8a is not the sibling: the asker and the answerer must be the same person's.
0. The Pro: fgtw (native) and the stower (masked, then `bridge seal`) stored, or flashed with a candidate.
1. The sibling: the owner's desktop Photon, a member of his fleet, given a `capsule_want` answer (the Photon repo: the fgtw crate's frames are there) serving from a folder of capsules named by their blake3 (the nine the Pro's list names, as built). Or a second ferros device the owner adds to his own fleet, his call.
2. The Pro restores: its `capsules.` is empty today, so the next restore pass asks. Expected on its console: `fgtw: capsules: A#1 fleet device(s) asked for a capsule, before fgtw.org`, `fgtw: capsules: <the desktop> has the capsule asked for`, `kists: a capsule from a fleet device`, then `kists: restored <name>, checked and sealed`, and the kist runs.
3. The failures: a capsule the desktop lacks (its none, then HTTP); the desktop's Photon closed (the ask's 8 s, then HTTP); a corrupted capsule in the folder (refused by the blake3, not kept, then HTTP).
4. The other direction: the desktop asks the Pro for a capsule it holds, and checks the bytes' blake3.

### A reset, observed (the Pro, 2026-11-23)

The vault process started afresh (`diag "vault fresh process yes"`; the kernel's vault, the shell and the device settings kept), the device's firmware and calibration scopes put back from an export, the kept seed gone, and the owner typed his handle and attested. What came back by itself:
- **The person's list of kists** (`kists.list`): all nine (Kūaha, Tide, Compass, Thermometer, Lumis, Mahere, Clock, Aemulus, Photon) with their versions, capsules' blake3 and every answer (`granted`, `always`); no right was asked again.
- **The deck's layout** (`map.deck`, `map.view`, the made wallpaper), the owner: "Layout after reset works."
- **The identity service's space** (`fgtw.space`, `fgtw.space_base`) and ahau's proof and wallpaper.

What did not:
- **Every kist's capsule.** `capsules.` stayed empty, so no kist and no puck ran. The restore (payloads/kists/src/main.rs `restore`) had no source to fetch from:
  - the fleet source was a stub returning false (`fleet_fetch`, built since: "The fleet source" above);
  - every one of the nine was stowed over USB (`ferros send`), which records no `source` URL (main.rs, the offer's stow: "Carried in over USB: no URL to fetch it again from");
  - the directory (kists.fgtw.org/kists.vsf) lists only an older Clock, and the restore tried only a listed entry's path;
  - kists.fgtw.org serves none of the nine capsules by their blake3: each `/capsules/<blake3>.vsf` answered 404.
- **Each kist's own domain** (Photon's people and messages, Mahere's tiles, Lumis's own settings): empty. A kist's own domain is device data: nothing syncs it, to another device or back after a reset. Whether Photon's people come back from the space once its capsule runs (its `people` and `be_you` rights) is not yet seen. Which of a kist's own data belongs in the identity space (Photon's people and messages, Mahere's tiles) is a design question for the owner; nothing of it is built.
- The device settings (base, language, theme, scale) were in the kernel's vault, which the reset kept, so this reset did not test their restore from the space.

Since (forge, after this): a restore also asks for any capsule the list names by its blake3 at `http://kists.fgtw.org/capsules/<blake3>.vsf` (`ferros_kist::directory::capsule_path`, the path tools/kistcat publishes at), listed in the directory or not; the stower's checks are the trust whoever served it.

### What is left

- **fgtw.org (R2) must hold the capsules.** The bucket `kists` serves `/capsules/<blake3>.vsf` by exact key (plain HTTP, no redirect, tools/kistcat/worker); today only the directory's own entries are put there, by the wrangler commands tools/kistcat/kists.sh prints. Each kist the person stows needs its capsule there by blake3 (unlisted is enough: the restore no longer needs a directory entry), within R2's single-object limit (Photon's 106 MB capsule fits). Publishing the owner's own-key capsules (Lumis, Mahere, the pucks, Aemulus) is his decision and not yet approved; a stow over USB could also put its capsule there, which would make every stowed kist restorable without the directory.
- **The fleet source on two devices**: built ("The fleet source" above), the test waits for the phones and a sibling of the owner's own fleet.
- **Kist domains** (Photon's people and messages, Mahere's tiles): device data nothing syncs; which belong in the identity space is the owner's design question.
- **The restore's own report**: no `kists: restoring` line reached the console after the attest; whether the restore was queued at all, and what each job ended with, is to be read on the device (the Kists page's states) once it is back.

## Device firmware (designed, not built)

Never synced: it is the device's, and it is on the device's own partitions. Each driver refills its own vault entries on a miss, through the read-only partition grants that exist (process.rs `Object::Part`, `PARTS`; BT already reads its patch from `vendor_a:/firmware/brcm/BCM.hcd` every start):

| Vault | Source | Driver |
|---|---|---|
| `aoc.reg.0` … `aoc.reg.14` | `vendor_a:/etc/sensors/registry/` (five scripts, then `append/`'s two) and `persist:/sensors/registry/` (eight calibrations), in tools/husky-aoc-reg.sh's order; checked on the Pro 2026-11-08: `persist:/sensors/registry/icm45631_accel_fac_cal.reg`, 277 bytes | sensors (init passes it slots 74 persist and 76 vendor_a) |
| `wifi.fw`, `wifi.nvram`, `wifi.clm` | `vendor_a:/firmware/` as firmware/husky/MANIFEST.md pins them (BLAKE3 checked before keeping) | pcie1 (slot 76) |
| `gpu.fw` | `vendor_a:/firmware/` (the Mali CSF image) | gpu: started by the kernel (`gpu_grants`), so a kernel change: batch it with the next flash |
| `gps.replay`, `gps.replay.sky` | not on a partition (captured sessions, GPS.md): stay on Harbor until the GPS engine's own start replaces them | |

Kept in the vault after the first read, since partfs reads one 4 KiB block per UFS command. The kernel's own `aoc.fw.*` and `tpu.fw` are in its own vault, which the vault process's reset does not touch.

## Built (2026-11-08)

- fgtw (`space`, `protocol::space_push`/`space_want`, `client::push_space`/`pull_space`), host-tested: merge order, document round trip, the baseline, a fresh device taking the space, changes both ways with a clock running ahead, a tombstone on first sight, a number read back at another width matched as the same fact, the seal refusing another fleet, key or byte.
- fgtw-bootstrap: the `space` slot. Committed, **not deployed**.
- ferros_space, host-tested: an arranged device's sealed space brings an empty vault back to the same list, answers, defaults, places and view byte for byte, nothing asked again; a move and a revoke cross between two devices; a device switched off keeps its own arrangement and gets it back after its own reset; a locked vault syncs nothing. With the Pro's own data read off it by `ferros-bridge store get` (2 kists, 3 places and the view): sealed (6 KB), opened into an empty vault, every place and the view the same.
- payloads/fgtw (space.rs; slots 9 `map.` and 10 `kists.`, init passes its 20 and 107), payloads/init, payloads/map (the reload, the switch): built, not yet on the device.

## The end test (needs the owner)

1. Deploy fgtw-bootstrap (its `deploy.sh`), so fgtw.org keeps the space.
2. Store the new `fgtw`, `init` and `map` capsules (tools/husky-shell.sh), reboot, attest. The console shows `fgtw: space: …` lines; the Fleet page shows the switch.
3. Arrange the deck (move and turn the circles), answer the kists' questions; wait for `fgtw.org holds ours`.
4. With the owner's OK: the vault process's fresh start (payloads/vault FRESH, input 01). The network needs `wifi.fw`, `wifi.nvram`, `wifi.clm` (the device's) and `wifi.cred` (the person's) back before the pull can reach anything: by hand from Harbor until the refill exists (or cellular). Reboot, attest. The deck comes back in the same spots and turns; the kists are listed with their answers; their capsules come back with `ferros-bridge kist stow` until the capsule fetch exists. Device firmware is restored by hand from Harbor until the refill exists.

### Run on the Pro, 2026-11-18

A wipe the owner chose instead of converting the vault to VSF (owner: "You can nuke both.").
- The device-only values were exported first (keys/husky/vault-export-2026-11-18/: GPS replays, WiFi and GPU firmware, AoC registry scripts, camera inputs); the identity's values were not exported (they are the space's to bring back).
- `diag "vault fresh yes"` zeroed the kernel vault's two copies, the vault process's region start and the ledger's two copies; the next boot (b939) opened a blank kernel vault (0 live entries, 0 live blocks, genesis written) and a blank vault-process region; capsules stored fresh, the device values restored.
- b940: ferros's own boot to the attest screen 983 ms (from its entry; ABL and the gate 24.6 s before it). A wrong handle typed on purpose: fgtw.org "not_found", refused, nothing minted.
- The owner's handle: the handle proof 3.9 s on the X3 (memory-hard, by design); fgtw.org's chain (29 ops, this device at op 28) in 0.70 s; the fleet key from the fan-out; member of the fleet 5.6 s after the submit; the first pull (fgtw.org's copy, 48 facts) in 0.57 s; 32 writes to the deck (places, the view, kist answers, the list) and the person key opened, 6.2 s after the submit, 2.27 s after the proof.
- The deck came back in the same spots. Clock was refetched from the directory; Lumis, Mahere and Photon came back by auto-stow (developer kists are not in the directory).
- Not restored by the space: the kept NFC tag (the ring) and this device's wallpaper (wallgen ran out of heap writing it; branch wallgen-heap). Per-device state that should survive a wipe has no home in the space yet.
