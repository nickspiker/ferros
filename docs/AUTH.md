# Auth: the device and the person

What a ferros device keeps for itself, what it keeps for the person it belongs to, and how the person's part opens. Owner decisions of 2026-10-31; code in `ferros_kernel/src/kvault.rs` (the vault), `process.rs` (`SYS_UNLOCK`), `payloads/ahau` (the attest screen), `payloads/init`.

## Two classes in the vault

The vault holds two classes of entry.

**The device class** is sealed under keys from the ira and is open from boot. It holds what the device needs before anyone is present:

| What | Vault names |
|---|---|
| Drivers and firmware | the capsules (`display`, `touch`, `pcie1`, `ip`, `modem`, `nfc`, `bt`, …), `wifi.fw`, `wifi.nvram`, `wifi.clm`, `gps.*` |
| The attest front end | the `display`, `touch`, `ahau` capsules |
| Hardware-owned settings | `device` (SETTINGS.md): language, number base, time zone, keyboard layout, brightness, dither, theme, proximity wake, unlock at boot |
| A network joined before anyone attests | `wifi.net` (ATTEST-UI.md "WiFi connect": joined under a random MAC; moved into the person's on attest, still to build) |
| The device's own bookkeeping | `sealed`, the kernel's reserved names (`\0…`): the name list, the device-only secret, the binding flag, the identity class's address list, the kept tags (`\0tag.`, "Attest with a tag") |

**The identity class** is the person's. It is sealed under a key bound to this device and to the person, and opens only while that person is present:

| What | Vault names |
|---|---|
| The deck | `map.place.*` (where each circle sits), `map.view` (the camera), everything else under `map.` |
| fgtw (the identity service) | `fgtw.*`: the chain and fleet key when kept, its phonebook |
| The attest screen's own | `ahau.*` |
| Networks | `wifi.cred` (networks travel with the person; the per-network MAC stays the device's, from the ira) |
| Where the device has been | `gps.last`, the GPS driver's last fix (2026-11-04). No GPS runs while nobody has attested. The replays the chip is aided from (`gps.replay`, `gps.replay.sky`) stay device class: Broadcom's firmware, gpsd's configuration and the orbits. Their position messages are zeroed (the gps driver's scrub, input G#02), so they keep no place. |
| Next | the person's stowed apps (their capsules), device pairings (car, headphones), every app's data |

The rule is in `kvault.rs` `IDENTITY`: a name under one of those prefixes is identity class, and everything else is device class.

## The identity key

`key = BLAKE3-keyed(derive_key("ferros identity vault v1", ira), device-only secret ‖ identity seed)`, then one address key and one payload key derived from it.

- **The ira** binds the key to this device. It comes from per-die values that are not secret: Android publishes most of them in sysfs, and Google has the chip IDs. So it binds, it does not protect.
- **The device-only secret** is 32 bytes from the session's wairua, made at the first unlock and kept in the device class. Google never sees it.
  - Android cannot read it once the gate's hypervisor nets the ferros partition (planned: no Android reads or writes of the ferros partition; the calibration data, chip ID, devinfo and UFS serial stay open because Android needs them).
  - With the net in place and the bootloader locked, getting to the identity key takes a desoldered UFS and the chip's values as well as the handle.
- **The identity seed** is BLAKE3 over the typed handle: milliseconds, so the space opens with no visible wait. The memory-hard handle proof is fgtw's (the public registry, squatting) and is made behind the screen.

## No identity, no device

A device not attached to an identity is not usable (owner, 2026-11-04: "if it's not attached to an identity, not usable. No way to save settings or anything, plus it completely defeats the point."). Until the attest screen ends with the device bound to an identity, by the fleet adding it (the words or the tap) or by a mint making it the identity's first device, nothing past the attest screen runs: no deck, no settings, no apps, and no way past the screen (`go on` was removed the same day). Before that only the device's own drivers and the attest's own processes run (ahau, and fgtw once an identity is unlocked). init enforces it (`wait_person`), and the kernel's `diag restart` refuses the map the same way.

## One identity per device

The network owns the assignment: a device belongs to the identity whose fleet bound it, and never reassigns itself.

- **Not bound.** The first handle typed unlocks, and its space is created. The device then joins that identity's fleet (the look-up, the pairing words, a sponsor adds it). A different handle typed later, still unbound, wipes the earlier one's leftovers first.
- **Bound.** When the fleet's chain names this device, fgtw (the identity service) binds it (`SYS_UNLOCK` op 3; the attest screen does too when it sees membership). From then on, only that identity's handle opens the device; any other is refused ("This device belongs to another identity").
  - The check needs no stored verifier: a seed whose key opens no name list is not the bound identity's.
- **Released.** A completed departure (the fleet's consented Remove, PHOTON.md "Leaving the fleet") releases the device (op 4):
  - every identity entry is deleted by its address;
  - the binding and any kept seed are gone;
  - every kept tag is deleted (they open only this identity);
  - the device-only secret is deleted, so whatever the address list missed can never be opened again.

  `diag "identity release"` does the same for a developer starting the device over.

## Joining and leaving a fleet: devices first, words last (owner, 2026-11-10)

The fleet is the person's devices, and they ask each other: fgtw.org (the bootstrap) only helps them find one another and holds a request for a device that cannot be reached (owner: "should be contacting the actual FGTW first (my other devices and friends) since my devices are the ones that should be notifying and asking").

- **Transports, in order:** an NFC tap; a BLE broadcast; the LAN; directly over the FGTW peer network (traversal, relay) to each fleet device wherever it is; fgtw.org's slot last, for a device offline. Each carries the same signed request.
- **Every fleet device that receives a request notifies the person** and asks there (one press to approve); the first approval settles it and the others clear. Nothing waits on the person opening a page.
- **An NFC tap is consent, both ways.** Joining: the tap with a fleet member binds the newcomer (as built). Leaving: the reverse of add, so a tap between the leaving device (its leave screen up: its own consent, which fgtw.org requires on every Remove) and an attested fleet device with its screen on approves the departure; the staying device says what happened and asks nothing more. A lost or stolen device still cannot be removed, only locked or shrunk.
- **Words are the last resort** (owner: "it's very clear that's a last effort if NFC fails and BLE fails ... it shouldn't be showing those unless we really need to"; "there's no typing of words"). They came from Photon's pairing-v2, where they were the first transport in 2026-07. They are not shown while a tap, BLE or a fleet device's notification can carry the request, and then only when the person asks for them. Leaving needs no words at all: the request is signed by a current member, so it cannot be a stranger's.
- **Approve or Deny, ready** (owner: "go right to the fleet page and have the unbind request 'Approve' or 'Deny' screen ready to go, and upon deny another request will have to use a new salt"): a notification opens the Fleet page on the request, on ferros and in the Photon app alike. A denied request's salt is refused for good, so asking again is a new request (PHOTON.md "Leaving the fleet").
- **No reachability internals on these screens:** not whether fgtw.org answered, not which devices are nearby. The leaving screen says it is asking the person's devices, and what to do (tap, or approve on any of them).

Not built yet (2026-11-10): ferros's fgtw reaches siblings on the LAN and fgtw.org only, nothing notifies, and the join and leave screens still lead with words.

## Unlocking and locking

- **At attest.** ahau derives the seed and unlocks (op 1). On a bound device the screen closes at once; the proof follows behind it, then the roots go to fgtw (the identity service). On an unbound device the look-up and join run as before. A kept tag held to the device does the same without the handle (op 6, "Attest with a tag" below).
- **The boot order** (init):
  1. Display, touch, the attest screen.
  2. WiFi (the chip comes up; it joins once `wifi.cred` can be read, or the device's `wifi.net` from boot, or a network the page hands in), IP, NFC, Bluetooth, the modem.
  3. A wait for a person (op 0, unlocked), then fgtw, the identity service: part of attest, it looks the identity up and joins its fleet.
  4. A wait for the device to be bound to that identity (op 0, bound), then the map. Both read the identity class.
- **Unlock at boot** (`device.unlock_at_boot`, opt-in, warned): while it is on, the kernel keeps the seed in the device class and unlocks at boot, before init starts. Off, the kept seed is deleted at boot. It and the kept tags are independent: with it on a tag is never needed (the device is unlocked before the attest screen would show), switching it off leaves the tags kept, and a release deletes both.
- **Locking.** Only when power goes, or when the device switches OS. Every reset the kernel orders drops the key first, its bytes overwritten (`platform::before_reset`), because a warm reset keeps DRAM. There are no timers.
- **The syscall.** `SYS_UNLOCK` (24), with the Unlock right, which only ahau, init and fgtw (the identity service) hold; ops 7 to 9 also with Settings (the map's settings pages, which keep tags and hold no right to unlock):

| Op | Does | Returns |
|---|---|---|
| 0 | state | bit 0 unlocked, bit 1 bound, bit 2 the person key held (FACETS.md) |
| 1 | unlock with a seed | the entry count, NOTHING (belongs to another identity) or ERR |
| 2 | lock | |
| 3 | bind | |
| 4 | release | |
| 5 | the person's export (the departure's "save first") | the document's length, or ERR |
| 6 | unlock by a tag: its UID in, the seed out (Unlock right only) | as op 1; ERR is the one refusal |
| 7 | keep a tag for the identity present (its UID) | 0 kept, 1 kept already, ERR |
| 8 | forget a tag (the start of its id), or every tag | how many |
| 9 | list the kept tags | how many; each the start of its id and when it was kept |
| 10 | the person key from the identity service (Unlock right only; FACETS.md D1): 32 bytes, held beside the identity key and wiped with it at lock; the kernel derives each kist's alias from it | 0, or ERR while nobody is present |

## Attest with a tag (owner, 2026-11-05 and 2026-11-11)

The owner keeps a plain NTAG-type tag (NFC-A, Type 2, a 7-byte UID, no crypto of its own) in his glasses. Held to the device at the attest screen it attests without the handle being typed; without the tag nothing is remembered ("Would be nice to have NFC be on if the screen is on at the attest and have that NFC locked capsule for easy handle entry ... and ideally something in settings to add a tag"). The threat view is the owner's: it takes his device and his glasses, and a handle is a name, not a password.

**What is kept, where, under what key** (kernel, `kvault.rs` `tag_keep`, `tag_unlock`):
- One record per tag, in the device class, kept by address under the reserved prefix `\0tag.`: the address is the tag's id, so the list of kept tags is a list of ids and nothing names a UID.
- The tag's id: BLAKE3 `derive_key("ferros 2026-11-11 tag unlock id v0", device key ‖ UID)`. The device key is `derive("ferros 2026-11-11 tag unlock device")` from the ira, in the kernel; the ira never leaves its registers.
- The seal key: BLAKE3 `derive_key("ferros 2026-11-11 tag unlock v0", device key ‖ device-only secret ‖ UID)`. The device-only secret too, as the identity key takes it, so the chips' published values and the tag are not enough without this device's own 32 bytes, and a release (which renews the secret) leaves any record it missed unopenable.
- The record is VSF (section `tag_unlock`): `added`, the Eagle time it was kept, and `seal`, the identity seed sealed with XChaCha20-Poly1305 under the seal key, a fresh 24-byte nonce from the kernel's entropy in front, the tag's id as associated data. The vault seals the record again under the device class, as every entry.
- The inner seal is what binds the record to the tag. Without its check a wrong tag would give a garbage seed, and on a device not bound yet a garbage seed opens (makes) a junk identity space; with it, a wrong tag is a clean refusal.
- To seal a seed the kernel needs it when a tag is kept, from the settings page, which never sees the seed. So while a person is present the kernel holds their seed beside the identity key, and wipes both when it locks. The identity service holds the same seed in RAM for as long.

**Keeping a tag** (the map, the NFC page, reached from Connections' NFC row or the NFC console circle): "add a tag" turns the page into a reader for one tag. The NFC driver's next unlock tag is kept (op 7) for the identity present, and the page says so. The tap on the page is the consent (above, "An NFC tap is consent"). The page lists the kept tags, each by the first four bytes of its id and when it was kept, each with "remove" (op 8); under them the warning, as attest at boot's: whoever holds the device and the tag holds the identity.

**At attest** (ahau): while the entry is up, the NFC driver's TAG (op 9) is held by a thread of ahau's; discovery reads tags only while the screen is on. The tag's UID goes from the driver to ahau and from ahau to the kernel (op 6), nowhere else, and is never logged. Opened, the kernel unlocks exactly as op 1 does (the same binding rule: a bound device opens only its own identity) and hands back the seed; ahau then goes on as after a typed handle (the proof from its space, the roots to the identity service). Refused (a tag kept for nothing here, a record that does not open, another identity's): "Denied. This tag does not open this device.", cleared by the next press, and the same tag is not tried again while the denial stands.

**Removing.** One tag from the NFC page, or every tag: op 8. A release deletes every tag with the identity, and so does another handle unlocking a device not bound yet (its earlier attempt's leftovers go, and the tags open only that attempt's identity).

**Which tags:** NFC-A, Type 2, with a double or triple size UID (7 or 10 bytes), which a tag keeps for life; a 4-byte UID may be random at every read (ISO/IEC 14443-3). Later (owner): "would be nice to have PIPE on tag".

## What the attest screen does

"Recognized" means this device holds a space for the handle: the key from its seed opens a name list here.

| Device | Handle | Result | Time |
|---|---|---|---|
| Bound | recognized (its own) | Opens: the screen goes, straight to the deck | Instant |
| Bound | not recognized | Denied | Instant |
| Unbound | recognized (an earlier attempt, join not finished) | Back to finding the identity and the join | Instant (the proof is kept in its space) |
| Unbound | not recognized | The earlier attempt's space wiped; the proof computed on the X3 and kept; then: an identity exists → join; none → mint, typed twice; no answer → still looking, the screen waits | The proof once, about 5 s |

The handle proof is public (beacons and the registry carry it), so keeping it in the identity's sealed space (`ahau.proof`) costs nothing. The identity seed is never kept unless unlock at boot is on.

## What the attest screen says

Owner, 2026-10-31: simple and direct. "Owner" rows are the owner's words (or "close to that", as given); "proposed" rows are mine, written in the same style and waiting for the owner's correction.

| When | Text | Button | Words |
|---|---|---|---|
| Unbound, waiting | This device is unbound. Type your handle to bind. | bind | owner |
| Bound, waiting | Type your handle. | unlock | proposed |
| Denied | Denied. This device is bound to another identity. | | owner ("DENIED"); the second sentence proposed |
| A tag refused | Denied. This tag does not open this device. | | proposed (the owner's "Denied." first, as above) |
| The proof running | Computing identity keys. | | owner |
| Looking up | Finding this identity. | | owner |
| An identity exists | Use an existing device to bind this device to your fleet. (the pairing words below) |  | owner |
| None exists | This handle is unclaimed. Minting is permanent. Type your handle again to mint your identity. | mint | owner |
| The two differ | They don't match. Try again. | | proposed |
| Minting | Minting. | | proposed |
| Mint failed | Minting failed. Enter to retry. | | proposed |
| No answer | No answer yet. Still looking. |  | proposed |
| Wrong slot | This handle's record doesn't match. Start over with another. | start over | proposed |

No step after the first entry has a way past the screen (owner, 2026-11-04, removing "go on": "I do not see a point. seems like another security nightmare"): the words screen and "No answer yet" wait, and close only when the fleet binds the device; `back` returns to the entry.

If the glossary's proposals are ruled on (GLOSSARY.md, "Proposed: the auth and fleet words"), "bind" in the first and sixth rows may become "join".

## Next: apps and pairings in the identity class (plan, 2026-10-31)

Not built; for the owner to approve before it is.

**The rule stays one rule.** A name is identity class when it starts with a prefix in `kvault.rs` `IDENTITY`. Moving a thing to the person is adding its prefix there and moving its names under it, never a second mechanism.

**Stowed apps.**
- What the person stows lives under `app.`: the capsule at `app.<name>`, the app's own data under `app.<name>.` (its store scope, one per app, the way `map.` and `fgtw.` are scoped today).
- Drivers and the attest front end stay where they are (`display`, `touch`, `usb`, `pcie1`, `ahau`, …), device class: they run before anyone is present.
- The map starts an app by its vault name, as it starts `ramps` today, so the change there is the name (`app.ramps`) and the moment: the map already runs only once an identity is unlocked (init's `wait_unlocked`), so an app's capsule is readable exactly when it can be started.
- The `sealed` list (the seals of installed masked capsules) stays device class: a seal is keyed with the OS-only key, so it says nothing about the person, and the kernel needs it before anyone attests to spawn the device's own masked processes.
- Locked, the device holds no list of the person's apps at all: their names are inside the identity class.

**Pairings** (owner: pairings move with the identity, like networks).
- Bluetooth link keys under `pair.bt.<peer>`, identity class. The `bt` driver gets an identity-class scope (`pair.`) next to its device-class one. It runs the beacon before anyone is present and reads the pairings once an identity unlocks (the way `pcie1` waits for `wifi.cred` today).
- The address the person's devices know: the identity's, not the chip's (`radio-addresses-belong-to-identity`). Derived from the identity key, never stored, so it moves with the person and never sits on the device locked.
- WiFi credentials are already identity class (`wifi.cred`); the per-network MAC stays the device's, from the ira.

**Moving with the person.** Every identity-class entry is what a new device of the fleet should receive: the fleet's chain carries the identity key's derivations, and the entries themselves travel sealed over Photon's fleet channel. That transport is Photon's and is not designed yet; the vault side is only "copy every `IDENTITY` name".

## Radios while locked (owner, 2026-11-04)

Every radio is up at the attest screen: the modem, the WiFi chip, Bluetooth and NFC. Being up while locked is not a larger surface than being up after attest, and a locked device has less to lose (no identity key in memory, the person's half of the vault sealed). Three conditions make it safe:

1. **Every radio's DMA is fenced.**
   - The kernel owns the fences, and no process may write one (`s2mpu::is_fence`, refused in `SYS_MMIO_WRITE`).
   - The modem (HSI1) is held to its carve-outs, the WiFi chip (HSI2) to its windows, and BLK_MISC's masters are denied everything (2026-11-04).
   - The modem's fence matters most: any base station reaches the modem before anyone attests.
2. **No stable identifier goes on the air while locked.**
   - Measured 2026-11-04: the WiFi driver neither scans nor joins until `wifi.cred` opens (`wait_cred`), so the chip is up but sends nothing. Since 2026-11-09 it scans when the network page asks, and joins a network handed in before attest (or kept from one, `wifi.net`) under a fresh random MAC each time; with no credential and no page asking it still sends nothing. Bluetooth advertises only when asked, under a fresh non-resolvable random address each time, and refuses to while locked. A locate beacon would rotate its keys.
   - Radio addresses that last belong to the identity (worn by the person, not the device).
3. **Nothing acts on radio input while locked unless it carries the fleet's signature.**
   - No Bluetooth pairing, no NFC action, no listening socket, no clock set from the network.
   - The one exception, the owner's (2026-11-11): a kept tag at the attest screen ("Attest with a tag"). It carries no signature; what stands in for one is the record only this device can open, sealed under the tag's UID and the device's own keys. The NFC driver hands the UID to the attest screen alone (fgtw, the endpoint's other holder, starts once a person is present), and the attest screen hands it to the kernel alone.
   - fgtw, the only thing that acts on what the radios bring, waits for a person. And since 2026-11-04 the drivers refuse it themselves, from SYS_FACTS 83 (a person present): `ip` answers applications nothing but its addresses, `bt` refuses the beacon, `nfc` refuses the tap secret.

Data while locked: the cellular link, and a WiFi network joined before attest (`wifi.net`, the owner's call of 2026-11-09: binding needs a network). The person's own networks (`wifi.cred`) wait for them. After a jump from Android no radio starts at all, locked or not (their drivers meet Android's hardware state).

## Locate (plan, 2026-11-04)

No GPS runs while nobody has attested, with one exception: locate, for a lost or stolen bound device. The design is write-only. The locked device can produce its location, but it can never read it back.

- **The keys it keeps.** At bind time the device keeps the fleet's public keys in the device class. Public keys are fine there.
- **The request.** A locate request comes from one of the fleet's other devices, signed with the fleet key and stamped with Eagle time. Without a valid request the GPS stays off, so there is no standing tracking, and no stranger can start one. A stale stamp is refused, so a request cannot be replayed to drain the battery.
- **The answer.**
  - The device powers the GPS, takes a fix, and seals it to the fleet's public key (X25519, XChaCha20-Poly1305).
  - It sends the sealed fix over the cellular link: while locked, `ip` lets only the locate responder open a connection, and only to fgtw.org.
  - It then drops the fix from memory and powers the GPS off. Nothing readable is ever stored.
  - With no network, it may keep the sealed fix to send later: the device cannot open it, only the fleet's private key can.
- **A cold start.** There is no remembered position while locked (`gps.last` is the person's), so the first fix starts cold (5-8 minutes, GPS.md). The WiFi access points and cell towers it sees can go in the same sealed answer for a rough place indoors.
- **Later: a crowd relay over BLE.** The locked device beacons under rotating keys that only its fleet can link. Other ferros and Photon devices that hear it report their own position, sealed to that fleet. This works with no network on the lost device at all.
- **Only a bound device has locate.** An unbound one has no fleet to answer to.

To check before building:
- whether the fleet's chain already carries a signing key and an X25519 key that fit;
- the modem's registration while locked against the carrier's terms (data-only, MVNO.md).

## Known gaps

- **The hypervisor net is not built yet.** Until it is, Android with root can read the ferros partition, and so the device-only secret.
- **A hang the watchdog resets** cannot run `before_reset`, so the key can survive in DRAM into the next boot. Planned: the gate's hypervisor scrubs the kernel's heap before it boots Android.
- **Stowed apps and device pairings** are not in the identity class yet (their capsules are device class today); the plan is above.
- **No way back to the attest screen** without a power cycle once a person is unlocked.
