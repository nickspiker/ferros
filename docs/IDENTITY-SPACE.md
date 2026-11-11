# The person's identity space

Status: designed and built 2026-11-08 (host-tested; on the device once the owner reboots and attests, below). Owner, 2026-11-08: "when I attest, the key matches, it syncs with my fleet, pulls a kist roster and then fetches the respective capsules from fgtw.org or my fleet … and throws it on my home screen in the same exact spot and rotation it was before I factory reset this thing." And: "on all my ferros devices the homescreen layout should sync, unless of course I go into settings and uncheck that."

That day the Pro's vault process store was wiped by a bug, and the kists, their answers and the arrangement went with it: nothing of the person's was anywhere but that one device. This is the fix for that class: whatever is the person's lives in their identity space, and every device of their fleet holds it.

## What is in it

One set of typed facts per person (fgtw::space, ferros's vocabulary in ferros_space):

| Kind | Which | What | This device's copy |
|---|---|---|---|
| KIST | developer key, option | capsule (blake3), name, wants in slot order with their reasons | a `kist` section of `kists.list` |
| ANSWER | developer, option, right | granted or not; stamped when it was given | a want's answer in `kists.list` |
| DEFAULT | right | on or off for new kists | `kists.list`'s `defaults` |
| PLACE | scope, circle (settings, or a kist by developer and option) | x, y, radius, turn | `map.place.<label>` |
| VIEW | scope | x, y, turn | `map.view` |
| HOME_SYNC | a device key | on or off | none: set from the Fleet page |
| ZONE | the fleet | automatic, and the zone by hand (an IANA id) | none: the identity service writes it to the kernel of each device that follows it (docs/TIME.md); chosen on the clock page |
| LANG | the fleet | the language (`wl`, its ISO 639-3 code) | none: the identity service writes it to the kernel of each device that follows it (SETTINGS.md); chosen on the language page |
| DND | the fleet | do not disturb, on or off (`u`) | none: the identity service writes it to the kernel of every device (preference 40, SETTINGS.md); switched on the quick row (QUICK.md) |
| PIN | developer key | the commitment of the key bundle that developer's first kist carried (`hp`), and their name when it gave one | a `pin` section of `kists.list` (KISTS.md "Trust on first install") |

The scope of a place and the view is the fleet's on every device whose home-screen sync is on (the default) and the device's own key on one switched off, so that device keeps its own arrangement, and gets it back after its own reset too. Kists, answers, defaults and pins always follow the person. A pin carries no stamp of its own into the sync, so a device that pinned another bundle for a developer the space already pins takes the space's: the first pin to reach the person's space stands (two pins made apart and merged before either device synced are settled by the space's merge, the later write).

Not in it: device firmware (below), the wallpaper (made per device, VAULT-OUT.md "First use"), the identity's roots (identity_not_on_disk_default), anything of a kist's own domain (its data syncs, if ever, as the kist's own business).

Next to add when they exist: the WiFi networks (`wifi.cred`; a credential carried by the space reaches a device only once it is online some other way: cellular, or a sibling over Bluetooth).

## Conflicts

- **The merge** keeps, per fact, the write greatest by (Eagle time written, writing device's key, gone over present, value bytes): a total order, so merging is commutative, associative and idempotent, and two devices that have seen the same writes hold the same space whatever order they came in. A removal is a tombstone and sticks.
- **A change on a device** is stamped past the write it replaces (a Lamport step), so the person's latest action wins even against a sibling whose clock runs ahead. An answer keeps its own time.
- **The device's copy carries no stamps**, so each device keeps a baseline (`fgtw.space_base`, never sent): what its copy held when it last agreed with the space. A value that moved from its baseline is the person's change here; one that did not takes the space's. A value with no baseline (a fresh device, an install before the first sync) takes the space's when the space has one, unless the fact carries a later stamp of its own.
- **A locked vault is never an empty one**: a read the vault refuses fails the sync, and nothing is written or removed.
- **A fresh device joins nothing before its first pull** (fgtw.org answered, or a sibling's copy arrived), so its empty copy and its all-off defaults never go out as the person's choices.

## Where it travels

- **Kept** on each device in the identity service's scope, the person's class: `fgtw.space`, `fgtw.space_base`. Not roots, so kept whatever unlock-at-boot says.
- **Fleet first**: `space_push` (the sealed space) and `space_want` (a digest; answered only by a sibling that holds something else) between the fleet's own devices, over the LAN through Photon Transport, from members only. Sealed XChaCha20-Poly1305 under a key derived from the fleet key, the handle proof and the key's fingerprint bound in.
- **fgtw.org keeps one copy**, the backup a total loss recovers from: a new fleet-sealed slot `space_put` / `space_get` beside `fstate` (fgtw-bootstrap: the two slots are one implementation; member-gated write, monotonic stamp, 1 MiB, public read of ciphertext). Chosen over riding inside `fstate`: that slot is Photon's roster and settings, written whole by every Photon client, and an older client would drop a section it does not know, and the fstate codec needs std (`x` text) that the device's no_std identity service lacks. A put is always pull, merge, put, so a sibling's copy there is never written over. Until the worker is deployed the seed answers `unknown_op`, which the device says once and treats as "no copy there": the space then lives on the device and its fleet.
- **Rotation**: a copy sealed under a fleet key since rotated does not open; each device re-seals its own copy under the new key on its next push.

## At attest

1. The identity service (payloads/fgtw) has the chain and the fleet key.
2. It loads its kept copy, asks the fleet's devices heard (`space_want`) and fgtw.org (`space_get`), and merges what comes.
3. Once that first pull is done, every 4 s it joins the device's copy (ferros_space::sync): the person's changes here into the space (and on to the fleet and fgtw.org), the space's into `kists.list`, `map.place.*` and `map.view`. A kist that arrives in one pass has its answers and its circle's place in the next.
4. The map asks the generation once a second (fgtw op 17) and reads its circles and the view again when it moves, so the home screen comes back in the same spots and turns, live, without a restart.
5. fgtw.org is asked again every 64 s for a sibling's change made out of earshot.

The Fleet page has the switch: "home screen: the same on all your devices" / "this device's own" (fgtw op 18), shown once the first pull is done.

## Capsules (built 2026-11-10 but the fleet source; docs/KISTS.md "The installer")

A kist in the person's list whose capsule this device lacks is fetched by its blake3, checked by the gate, and kept in `capsules.`; its answers came with the list, so nothing is asked again. The fetch takes sources in order, each a small trait (`fetch(blake3, offset, buf)`, `size(blake3)`), so a source is added without touching the rest:

1. this device's own `capsules.` (an install that survived);
2. a fleet device that holds it (a `capsule_want` over Photon Transport, answered in pieces from its `capsules.`);
3. fgtw.org, once it serves capsules: its blob slots cap at 1 MiB and Photon's capsule is 106 MB, so that is an owner decision (R2 by blake3, public, the gate's signature is the trust);
4. development: the bridge (`ferros-bridge kist install`, which keeps the answers of the same developer and option).

Whatever arrives goes thru the installer's stages (payloads/kists): its blake3 and size, every egg against the developer bundle it carries and that bundle the one the person's list pins for its developer (KISTS.md "Trust on first install"), then the kernel's checker over its masked code and the seal, which is all a launch checks. The installer holds `capsules.` (write); the map and the identity service never do. It follows the identity service's doorbell: when the space's generation moves, it fetches what the list names and this device lacks, from the kist's recorded source (`source`, carried in its KIST fact) then the directory by blake3; the fleet source (2) is a seam, not built. Until a capsule is back its circle is on the map and a tap is refused by the kernel (no capsule); `ferros-bridge kist install <capsule>` still restores a development capsule with its answers.

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
3. Arrange the home screen (move and turn the circles), answer the kists' questions; wait for `fgtw.org holds ours`.
4. With the owner's OK: the vault process's fresh start (payloads/vault FRESH, input 01). The network needs `wifi.fw`, `wifi.nvram`, `wifi.clm` (the device's) and `wifi.cred` (the person's) back before the pull can reach anything: by hand from Harbor until the refill exists (or cellular). Reboot, attest. The home screen comes back in the same spots and turns; the kists are listed with their answers; their capsules come back with `ferros-bridge kist install` until the capsule fetch exists. Device firmware is restored by hand from Harbor until the refill exists.
