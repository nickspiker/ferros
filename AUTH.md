# Auth: the device and the person

What a ferros phone keeps for itself, what it keeps for the person it belongs to, and how the person's part opens. Owner decisions of 2026-10-31; code in `ferros_kernel/src/kvault.rs` (the vault), `process.rs` (`SYS_UNLOCK`), `payloads/ahau` (the attest screen), `payloads/init`.

## Two classes in the vault

The vault holds two classes of entry.

**The device class** is sealed under keys from the ira and is open from boot. It holds what the phone needs before anyone is present:

| What | Vault names |
|---|---|
| Drivers and firmware | the capsules (`display`, `touch`, `pcie1`, `ip`, `modem`, `nfc`, `bt`, …), `wifi.fw`, `wifi.nvram`, `wifi.clm`, `gnss.*` |
| The attest front end | the `display`, `touch`, `ahau` capsules |
| Hardware-owned settings | `device` (SETTINGS.md): language, number base, time zone, keyboard layout, brightness, dither, theme, proximity wake, unlock at boot |
| The phone's own bookkeeping | `sealed`, the kernel's reserved names (`\0…`): the name list, the phone-only secret, the binding flag, the identity class's address list |

**The identity class** is the person's. It is sealed under a key bound to this phone and to the person, and opens only while that person is present:

| What | Vault names |
|---|---|
| The home screen | `map.place.*` (where each circle sits), `map.view` (the camera), everything else under `map.` |
| The Photon client | `photon.*`: the chain and fleet key when kept, its phonebook |
| The attest screen's own | `ahau.*` |
| Networks | `wifi.cred` (networks travel with the person; the per-network MAC stays the device's, from the ira) |
| Next | the person's installed apps (their capsules), device pairings (car, headphones), every app's data |

The rule is in `kvault.rs` `IDENTITY`: a name under one of those prefixes is identity class, and everything else is device class.

## The identity key

`key = BLAKE3-keyed(derive_key("ferros identity vault v1", ira), phone-only secret ‖ identity seed)`, then one address key and one payload key derived from it.

- **The ira** binds the key to this phone. It comes from per-die values that are not secret: Android publishes most of them in sysfs, and Google has the chip IDs. So it binds, it does not protect.
- **The phone-only secret** is 32 bytes from the session's wairua, made at the first unlock and kept in the device class. Google never sees it.
  - Android cannot read it once the gate's hypervisor nets the ferros partition (planned: no Android reads or writes of the ferros partition; the calibration data, chip ID, devinfo and UFS serial stay open because Android needs them).
  - With the net in place and the bootloader locked, getting to the identity key takes a desoldered UFS and the chip's values as well as the handle.
- **The identity seed** is BLAKE3 over the typed handle: milliseconds, so the space opens with no visible wait. The memory-hard handle proof is the Photon client's (the public registry, squatting) and is made behind the screen.

## One identity per phone

The network owns the assignment: a phone belongs to the identity whose fleet bound it, and never reassigns itself.

- **Not bound.** The first handle typed unlocks, and its space is created. The phone then joins that identity's fleet (the look-up, the pairing words, a sponsor adds it). A different handle typed later, still unbound, wipes the earlier one's leftovers first.
- **Bound.** When the fleet's chain names this phone, the Photon client binds it (`SYS_UNLOCK` op 3; the attest screen does too when it sees membership). From then on, only that identity's handle opens the phone; any other is refused ("This phone belongs to another identity").
  - The check needs no stored verifier: a seed whose key opens no name list is not the bound identity's.
- **Released.** A completed departure (the fleet's consented Remove, PHOTON.md "Leaving the fleet") releases the phone (op 4):
  - every identity entry is deleted by its address;
  - the binding and any kept seed are gone;
  - the phone-only secret is deleted, so whatever the address list missed can never be opened again.

  `diag "identity release"` does the same for a developer starting the phone over.

## Unlocking and locking

- **At attest.** ahau derives the seed and unlocks (op 1). On a bound phone the screen closes at once; the proof follows behind it, then the roots go to the Photon client. On an unbound phone the look-up and join run as before.
- **The boot order** (init):
  1. Display, touch, the attest screen.
  2. WiFi (the chip comes up; it joins once `wifi.cred` can be read), IP, NFC, Bluetooth, the modem.
  3. A wait for a person (op 0), then the map and the Photon client. They read the identity class.
- **Unlock at boot** (`device.unlock_at_boot`, opt-in, warned): while it is on, the kernel keeps the seed in the device class and unlocks at boot, before init starts. Off, the kept seed is deleted at boot.
- **Locking.** Only when power goes, or when the phone switches OS. Every reset the kernel orders drops the key first, its bytes overwritten (`platform::before_reset`), because a warm reset keeps DRAM. There are no timers.
- **The syscall.** `SYS_UNLOCK` (24), with the Unlock right, which only ahau, init and the Photon client hold:

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
| Unbound | not recognized | The earlier attempt's space wiped; the proof computed on the X3 and kept; then: an identity exists → join; none → mint, typed twice; no answer → still looking, "go on" offered | The proof once, about 5 s |

The handle proof is public (beacons and the registry carry it), so keeping it in the identity's sealed space (`ahau.proof`) costs nothing. The identity seed is never kept unless unlock at boot is on.

## What the attest screen says

Owner, 2026-10-31: simple and direct.

| When | Text | Button |
|---|---|---|
| Unbound, waiting | This device is unbound. Type your handle to bind. | bind |
| Bound, waiting | Type your handle. | unlock |
| Denied | Denied. This device is bound to another identity. | |
| The proof running | Computing identity keys. | |
| Looking up | Finding this identity. | |
| An identity exists | Use an existing device to bind this device to your fleet. (the pairing words below) | go on |
| None exists | This handle is unclaimed. Minting is permanent. Type your handle again to mint your identity. | mint |
| The two differ | They don't match. Try again. | |
| Minting | Minting. | |
| Mint failed | Minting failed. Enter to retry. | |
| No answer | No answer yet. Still looking. | go on |
| Wrong slot | This handle's record doesn't match. Start over with another. | start over |
| Bad character | This keyboard can't type that character. | |

## Known gaps

- **The hypervisor net is not built yet.** Until it is, Android with root can read the ferros partition, and so the phone-only secret.
- **A hang the watchdog resets** cannot run `before_reset`, so the key can survive in DRAM into the next boot. Planned: the gate's hypervisor scrubs the kernel's heap before it boots Android.
- **Installed apps and device pairings** are not in the identity class yet (their capsules are device class today).
- **No way back to the attest screen** without a power cycle once a person is unlocked.
