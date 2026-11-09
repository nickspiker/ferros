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
| The device's own bookkeeping | `sealed`, the kernel's reserved names (`\0…`): the name list, the device-only secret, the binding flag, the identity class's address list |

**The identity class** is the person's. It is sealed under a key bound to this device and to the person, and opens only while that person is present:

| What | Vault names |
|---|---|
| The home screen | `map.place.*` (where each circle sits), `map.view` (the camera), everything else under `map.` |
| fgtw (the identity service) | `fgtw.*`: the chain and fleet key when kept, its phonebook |
| The attest screen's own | `ahau.*` |
| Networks | `wifi.cred` (networks travel with the person; the per-network MAC stays the device's, from the ira) |
| Where the device has been | `gps.last`, the GPS driver's last fix (2026-11-04). No GPS runs while nobody has attested. The replays the chip is aided from (`gps.replay`, `gps.replay.sky`) stay device class: Broadcom's firmware, gpsd's configuration and the orbits. Their position messages are zeroed (the gps driver's scrub, input G#02), so they keep no place. |
| Next | the person's installed apps (their capsules), device pairings (car, headphones), every app's data |

The rule is in `kvault.rs` `IDENTITY`: a name under one of those prefixes is identity class, and everything else is device class.

## The identity key

`key = BLAKE3-keyed(derive_key("ferros identity vault v1", ira), device-only secret ‖ identity seed)`, then one address key and one payload key derived from it.

- **The ira** binds the key to this device. It comes from per-die values that are not secret: Android publishes most of them in sysfs, and Google has the chip IDs. So it binds, it does not protect.
- **The device-only secret** is 32 bytes from the session's wairua, made at the first unlock and kept in the device class. Google never sees it.
  - Android cannot read it once the gate's hypervisor nets the ferros partition (planned: no Android reads or writes of the ferros partition; the calibration data, chip ID, devinfo and UFS serial stay open because Android needs them).
  - With the net in place and the bootloader locked, getting to the identity key takes a desoldered UFS and the chip's values as well as the handle.
- **The identity seed** is BLAKE3 over the typed handle: milliseconds, so the space opens with no visible wait. The memory-hard handle proof is fgtw's (the public registry, squatting) and is made behind the screen.

## No identity, no device

A device not attached to an identity is not usable (owner, 2026-11-04: "if it's not attached to an identity, not usable. No way to save settings or anything, plus it completely defeats the point."). Until the attest screen ends with the device bound to an identity, by the fleet adding it (the words or the tap) or by a mint making it the identity's first device, nothing past the attest screen runs: no home screen, no settings, no apps, and no way past the screen (`go on` was removed the same day). Before that only the device's own drivers and the attest's own processes run (ahau, and fgtw once an identity is unlocked). init enforces it (`wait_person`), and the kernel's `diag restart` refuses the map the same way.

## One identity per device

The network owns the assignment: a device belongs to the identity whose fleet bound it, and never reassigns itself.

- **Not bound.** The first handle typed unlocks, and its space is created. The device then joins that identity's fleet (the look-up, the pairing words, a sponsor adds it). A different handle typed later, still unbound, wipes the earlier one's leftovers first.
- **Bound.** When the fleet's chain names this device, fgtw (the identity service) binds it (`SYS_UNLOCK` op 3; the attest screen does too when it sees membership). From then on, only that identity's handle opens the device; any other is refused ("This device belongs to another identity").
  - The check needs no stored verifier: a seed whose key opens no name list is not the bound identity's.
- **Released.** A completed departure (the fleet's consented Remove, PHOTON.md "Leaving the fleet") releases the device (op 4):
  - every identity entry is deleted by its address;
  - the binding and any kept seed are gone;
  - the device-only secret is deleted, so whatever the address list missed can never be opened again.

  `diag "identity release"` does the same for a developer starting the device over.

## Unlocking and locking

- **At attest.** ahau derives the seed and unlocks (op 1). On a bound device the screen closes at once; the proof follows behind it, then the roots go to fgtw (the identity service). On an unbound device the look-up and join run as before.
- **The boot order** (init):
  1. Display, touch, the attest screen.
  2. WiFi (the chip comes up; it joins once `wifi.cred` can be read, or the device's `wifi.net` from boot, or a network the page hands in), IP, NFC, Bluetooth, the modem.
  3. A wait for a person (op 0, unlocked), then fgtw, the identity service: part of attest, it looks the identity up and joins its fleet.
  4. A wait for the device to be bound to that identity (op 0, bound), then the map. Both read the identity class.
- **Unlock at boot** (`device.unlock_at_boot`, opt-in, warned): while it is on, the kernel keeps the seed in the device class and unlocks at boot, before init starts. Off, the kept seed is deleted at boot.
- **Locking.** Only when power goes, or when the device switches OS. Every reset the kernel orders drops the key first, its bytes overwritten (`platform::before_reset`), because a warm reset keeps DRAM. There are no timers.
- **The syscall.** `SYS_UNLOCK` (24), with the Unlock right, which only ahau, init and fgtw (the identity service) hold:

| Op | Does | Returns |
|---|---|---|
| 0 | state | bit 0 unlocked, bit 1 bound |
| 1 | unlock with a seed | the entry count, NOTHING (belongs to another identity) or ERR |
| 2 | lock | |
| 3 | bind | |
| 4 | release | |

## What the attest screen does

"Recognized" means this device holds a space for the handle: the key from its seed opens a name list here.

| Device | Handle | Result | Time |
|---|---|---|---|
| Bound | recognized (its own) | Opens: the screen goes, straight to the home screen | Instant |
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

**Installed apps.**
- What the person installs lives under `app.`: the capsule at `app.<name>`, the app's own data under `app.<name>.` (its store scope, one per app, the way `map.` and `fgtw.` are scoped today).
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
- **Installed apps and device pairings** are not in the identity class yet (their capsules are device class today); the plan is above.
- **No way back to the attest screen** without a power cycle once a person is unlocked.
