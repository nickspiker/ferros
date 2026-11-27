# Auth: the device and the person

What a ferros device keeps for itself, what it keeps for the person it belongs to, and how the person's part opens. Owner decisions of 2026-10-31; code in `kernel/src/kvault.rs` (the vault), `process.rs` (`SYS_UNLOCK`), `payloads/ahau` (the attest screen), `payloads/init`.

## Two classes in the vault

The vault holds two classes of entry.

**The device class** is sealed under keys from the ira and is open from boot. It holds what the device needs before anyone is present:

| What | Vault names |
|---|---|
| Drivers and firmware | the capsules (`display`, `touch`, `pcie1`, `ip`, `modem`, `nfc`, `bt`, …), `wifi.fw`, `wifi.nvram`, `wifi.clm`, `gps.*` |
| The attest front end | the `display`, `touch`, `ahau` capsules |
| Hardware-owned settings | `device` (SETTINGS.md): language, number base, time zone, keyboard layout, brightness, dither, theme, proximity wake, unlock at boot |
| A network joined before anyone attests | `wifi.net` (ATTEST-UI.md "WiFi connect": while locked joined under this boot's address; after attest under the person's, as `wifi.cred`; moved into the person's on attest, still to build) |
| The device's own bookkeeping | `sealed`, the kernel's reserved names (`\0…`): the name list, the device-only secret, the binding flag, the identity class's address list, the kept tags (`\0tag.`, "Attest with a tag") |

**The identity class** is the person's. It is sealed under a key bound to this device and to the person, and opens only while that person is present:

| What | Vault names |
|---|---|
| The deck | `map.place.*` (where each circle sits), `map.view` (the camera), everything else under `map.` |
| fgtw (the identity service) | `fgtw.*`: the chain and fleet key when kept, its phonebook |
| The attest screen's own | `ahau.*` |
| Networks | `wifi.cred` (networks travel with the person; the per-network MAC is worn by the person present, theirs and this device's together: WIFI.md "The MAC") |
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

The owner, 2026-11-24, asked whether his wife could use his device: "Nope. 1 device, 1 identity. My wife wants to use it? I need to unbind first, then she needs to bind it to her fleet."

The network owns the assignment: a device belongs to the identity whose fleet bound it, and never reassigns itself. Another person uses it only after its identity unbinds it ("Unbind this device" below) and theirs binds it.

**The attest screen's state line** (owner, 2026-11-24), above the handle box, never saying whose: unbound, "This device is unbound. Present an identity to bind it to."; bound, "This device is bound. Present its identity to unlock it." An approve keeps the line and puts what it approves under it.

- **Not bound.** The first handle typed unlocks, and its space is created. The device then joins that identity's fleet (the look-up, the pairing words, a sponsor adds it). A different handle typed later, still unbound, gets its own space; the earlier one's leftovers stay where they are, unopened, and the vault's rotation takes their room (owner, 2026-11-24: no needless writes; until then they were deleted first).
- **Bound.** When the fleet's chain names this device, fgtw (the identity service) binds it (`SYS_UNLOCK` op 3; the attest screen does too when it sees membership). From then on, only that identity's handle opens the device; any other is refused: "Denied. This handle does not unlock this device."
  - The check needs no stored verifier: a seed whose key opens no name list is not the bound identity's.
  - **A refusal says nothing** (checked 2026-11-24, from the code): the kernel cannot tell the device's own handle mistyped from another identity's, since both are seeds whose key opens no name list here, so both get the same refusal at the same point, before any proof is made, anything is looked up on the network or anything is written. Until 2026-11-24 the words were "Denied. This device is bound to another identity.", which named a typo another identity; and the kernel marked a person present (and set the flash log's personal key and the migration flag) before the check, undoing it with the lock. Now nothing is marked before the check, and any refusal or failed read drops the keys at once.
- **Unbound.** Only by the person's unbind with the fleet's consent ("Unbind this device" below), or a shred, which leaves the device in its fleet but no longer bound here ("The Security page" below): the device-only secret renewed, nothing deleted. `diag "identity release"` is gone (owner, 2026-11-24: "if it does not get my consent from my other devices or a correct handle entry, nothing gets wiped"): the cable is neither.

## Joining and leaving a fleet: devices first, words last (owner, 2026-11-10)

The fleet is the person's devices, and they ask each other: fgtw.org (the bootstrap) only helps them find one another and holds a request for a device that cannot be reached (owner: "should be contacting the actual FGTW first (my other devices and friends) since my devices are the ones that should be notifying and asking").

- **Transports, in order:** an NFC tap; a BLE broadcast; the LAN; directly over the FGTW peer network (traversal, relay) to each fleet device wherever it is; fgtw.org's slot last, for a device offline. Each carries the same signed request.
- **Every fleet device that receives a request notifies the person** and asks there (one press to approve); the first approval settles it and the others clear. Nothing waits on the person opening a page.
- **An NFC tap is consent, both ways.** Joining: the tap with a fleet member binds the newcomer (as built). Leaving: the reverse of add, so a tap between the leaving device (its leave screen up: its own consent, which fgtw.org requires on every Remove) and an attested fleet device with its screen on approves the departure (on ferros the staying device's NFC reader reads for that tap only while the departure's Approve and Deny are on its glass, fgtw op 8 [2]; 2026-11-22); the staying device says what happened and asks nothing more. A lost or stolen device still cannot be removed, only locked or shrunk.
- **Words are the last resort** (owner: "it's very clear that's a last effort if NFC fails and BLE fails ... it shouldn't be showing those unless we really need to"; "there's no typing of words"). They came from Photon's pairing-v2, where they were the first transport in 2026-07. They are not shown while a tap, BLE or a fleet device's notification can carry the request, and then only when the person asks for them. Leaving needs no words at all: the request is signed by a current member, so it cannot be a stranger's.
- **Approve or Deny, ready** (owner: "go right to the fleet page and have the unbind request 'Approve' or 'Deny' screen ready to go, and upon deny another request will have to use a new salt"): a notification opens the Fleet page on the request, on ferros and in the Photon app alike. A denied request's salt is refused for good, so asking again is a new request (PHOTON.md "Leaving the fleet").
- **No reachability internals on these screens:** not whether fgtw.org answered, not which devices are nearby. The leaving screen says it is asking the person's devices, and what to do (tap, or approve on any of them).

Not built yet (2026-11-10): ferros's fgtw reaches siblings on the LAN and fgtw.org only, nothing notifies, and the join and leave screens still lead with words.

**Nothing is announced before the new device's person confirms** (owner, 2026-11-22, after a fleet notification reached his devices before he had confirmed on the new one): a handle typed alone rings none of the person's devices.

1. **The look-up** is a question, not an announcement: the signed `fleet_req` to this network and the fleet peers known, then fgtw.org. A fleet device answers it and notifies nobody.
2. **The confirmation.** When the identity has a fleet, the attest screen asks: "Yes, I really would like to add this device and this is my identity and not a typo" (the identity service's state 11, `V_CONFIRM`). Until it is pressed the identity service (payloads/fgtw) mints no binding request, so none is handed to a fleet device on the LAN, none is posted to fgtw.org's registry, no tap secret is served on the NFC tag and no beacon id is advertised over BLE; it sends no LAN beacon under the handle, says no pairing words, and takes no select. The press is fgtw op 32 `[32][1]`; back from the next step withdraws (`[32][0]`: the request dropped, the radios stopped).
3. **The request falls on deaf ears unless a Fleet page is open** (owner, 2026-11-22, via the Photon session: a stranger typing the person's handle again and again from anywhere must never make their devices light up): it keeps going out, and a fleet device lists it only while its Fleet page is open; no toast, light or notification anywhere else. On ferros the add machine's "asks to join" is a console line only, and the system question (the red Deck button) comes only once the person at that page has selected the device. This supersedes, for joining, the 2026-11-10 bullet above that every fleet device notifies.
   - **fgtw.org last** (the owner's HARD rule of 2026-11-18, peers first: fgtw.org only when no LAN broadcast is heard and no WAN peer answers). After the yes the request goes to the fleet's devices on this network (each one heard beaconing is handed it) and out over NFC and BLE; fgtw.org's registry gets it only when no fleet device has been heard on this network within about 4 s of the mint (2^22 µs, past the 3 s beacon period; ferros has no WAN peer path yet, so that is the whole test), or at once when the person asks for the words (op 32 `[32][2]`, "Not in range?"), the explicit edge.
   - **Not built:** carrying the request itself over BLE or NFC. A fleet device matches a BLE beacon id or a tap secret against the requests it already holds (from the LAN, or fgtw.org's registry once the person looks there), so an add where the two devices share no network and the registry has no copy cannot complete yet; until 2026-11-22 the registry copy, posted at once, covered that case.
4. **Then the new device advises** NFC first: the two devices together, the other one's screen on and its Fleet page open, in Photon or ferros (ferros's Fleet page opens the add flow, its NFC reader and BLE scan, only once the person pressed "bind device" and approved it on the attest screen, and only while they have the page on the glass: the Settings circle open on it, the screen on, no app in front, and not on a visit a departure opened; the Settings circle opens at its root list, so the page is always picked, never reopened onto; the security review of 2026-11-22, and "Bind device" below). Otherwise the click to add over BLE: on that page a touch on this device's name selects it (this screen turns green), and the green button adds it.
5. **The words are for odd cases only**, never shown by default (shoulder surfing): behind "Not in range? Show words to copy/paste or dictate". Pressing it is also what puts the request in fgtw.org's registry, where a fleet device not in range finds it by the words. On a fleet device the "type its words" choice is the other half.

Built 2026-11-22 on ferros (ahau, payloads/fgtw, ferros_lang); Photon's own join screen is Photon's to follow.

## Unbind this device (owner, 2026-11-22; "unbind" since 2026-11-24)

The owner's word for it (2026-11-24): "I need to unbind first, then she needs to bind it to her fleet." The Fleet page's button says "unbind this device" (until then "remove this device", the fleet's shared words), and the approve "Approve: unbind this device." What it does is unchanged: the identity's hold on the device is erased, and after it the device is unbound, ready for another person to bind. The code keeps the word remove (fgtw op 33, `SYS_UNLOCK` op 15, ahau_core::approve::REMOVE_DEVICE).

On the device being unbound: press "unbind this device" on the Fleet page, and approve it on the attest screen (since 2026-11-24, "Approve on the attest screen" below: the device locks at once and the handle typed there approves the removal; until then a second handle-entry screen took it).
No Fleet page needs to be open on the other devices before the request reaches them; nothing is destroyed here until one of them consents.

**The owner's order and rule** (2026-11-24): the handle typed (the approve); a request to the fleet; consent from an existing fleet device, waited for with no timer acting in its place (the last device still refuses); the device signs itself out of the chain; the fleet rotates its key on the consenting device's side (rotate on remove); the device wipes its keys from memory. "if it does not get my consent from my other devices or a correct handle entry, nothing gets wiped. Lock is fine, but no wipe and certainly no unbind." And on the bulk erase this section described until then: "whoever's dumbfuck idea to add more writes to an SSD that has a fixed number of writes is beyond me but it's dead wrong. Sand on a beach my friend." The shared sequence is fgtw's (`fgtw::security::UNBIND`), Photon's too.

1. **The check.** The approve: the map's Remove asks the kernel (`SYS_UNLOCK` op 16, `REMOVE_DEVICE`), the device locks, and the handle typed on the attest screen unlocks it, so it is the device's own identity's. The kernel then holds the approved removal for the identity service. The attest screen hands the roots over as at any attest, waits until the identity service holds the identity as a member again (the lock wiped what it held), and hands it the handle's seed and its memory-hard proof (fgtw op 33 `[33][0][seed][proof]`; the proof from the identity's space, or made, seconds on the X3). The identity service compares the seed with the identity in hand, and fgtw's removal gate (fgtw e5766f4: `UserOp::Remove`, then `Event::Unbind` with the proof) sends nothing unless the proof is this identity's. The kernel compares the seed again in registers before it erases anything (`SYS_UNLOCK` op 15, service.rs op 18) and erases only with the approved removal held, which it takes (one approve, one removal). Refused (the identity's last device, no fleet), the attest screen says why and Done closes it, nothing changed.
2. **The request to the fleet.** The departure machine's signed request (intent 1, to a new owner), handed by Photon Transport to every fleet device heard here and put in the identity space, whose push reaches the fleet and fgtw.org in the same pass. The request goes out thru fgtw's shared departure machine (the removal gate since fgtw e5766f4): payloads/fgtw depart.rs `remove_unbind` drives it. A bare Leave (fgtw op 5) is refused by that gate. The attest screen then says the fleet is asked, and Done closes it: the device stays usable and bound while it waits.
3. **Consent.** A member's person approves (the Fleet page's Approve on any of the person's devices, or the tap). Waited for however long it takes (until 2026-11-24 the erase ran about 4 s after the request went out, consented or not). A deny ends it with nothing changed; another unbind is a new request.
4. **The sign-out.** The approver publishes the consented Remove: the op carries this device's own signature over its departure (the request's consent) with the approver's beside it, and frees this device's brand on fgtw.org (another identity's add needs it: fgtw-bootstrap refuses a bare self-departure, `consent_required`, and a device branded to another identity, `device_owned`, `plate_owned`).
5. **The rotation.** The approving device mints a fresh fleet key wrapped to the live members alone (`fgtw::security::rotate_after_departure`, payloads/fgtw depart.rs `rotate_after`) and seals the space and the person key again under it at once, so this device opens nothing sealed after.
6. **Forgetting.** Once the fold no longer lists this device (`Action::Wipe`, depart.rs `depart_wipe`), the kernel compares the typed seed again in registers (`SYS_UNLOCK` op 15, service.rs op 18), takes the approve, renews the device-only secret (kvault `renew`: the identity locked, its keys wiped from memory, one write: the new secret with the bound mark off in its one document) and powers off (PSCI SYSTEM_OFF, the killswitch's). Nothing is deleted: everything of the person's here is sealed under keys made from the old secret, which no longer exists ("The Security page" below has the proof).

A wrong handle, no consent, a deny, no device reachable, a lock or a power cut before step 6 changes nothing: the device stays bound, its keys and secret stay. The typed seed waits in the identity service's RAM only, so a lock or a power cut drops the unbind there; if the fleet's consent lands after it, the device is out of the chain but still bound and whole here, and nothing is destroyed until its person approves an unbind again (the kernel then renews with that approve held, op 4, gated as op 15).

The identity's last device refuses (an identity must live somewhere).

**Dry run:** fgtw op 33 `[33][1]` runs every check and says on the console what each step would take (`fgtw: remove (dry run): ...`, the kernel's account under `kernel:`), changing nothing and needing no handle.
From the host: `ferros-bridge spawn <removedry.bin> --name removedry --use fgtw` (payloads/removedry), then the console.
`diag "identity remove"` gives the kernel's half alone.

## Unlocking and locking

- **At attest.** ahau derives the seed and unlocks (op 1). On a bound device the screen closes at once; the proof follows behind it, then the roots go to fgtw (the identity service). On an unbound device the look-up and join run as before. A kept tag held to the device does the same without the handle (op 6, "Attest with a tag" below).
- **The boot order** (init):
  1. Display, touch, the attest screen.
  2. WiFi (the chip comes up; it joins once `wifi.cred` can be read, or the device's `wifi.net` from boot, or a network the page hands in), IP, NFC, Bluetooth, the modem.
  3. A wait for a person (the identity's doorbell, op 11: unlocked), then fgtw, the identity service: part of attest, it looks the identity up and joins its fleet.
  4. A wait for the device to be bound to that identity (the doorbell again: bound), then the map. Both read the identity class.

  Between 1 and 2 init waits for the attest screen's window (the display's op 75, at most 4 s; WINDOWS.md), except when the identity was unlocked and bound at boot: there is no attest screen then.
- **The identity's doorbell** (2026-11-16, owner: "Yuppers"). Until then init asked op 0 64 times a second while the attest screen was up, up to 16 ms behind a tap and each ask a read of the bound mark from the flash under the kernel lock. Now a process with the Unlock right hands the kernel a word (op 11), and the kernel writes the state into it at once and again at every change it makes itself: an unlock (by the handle or a tag), a lock, a bind, a release, the person key handed in (service.rs `store_unlock`, `store_tag`; process.rs `identity_ring`, one of the bells of SYS_BELL). A waiter sleeps on its word (`ferros_sys::wait_world`) and reads the state from the word itself. A change the state does not show (the same person key twice, a tag kept) rings nothing.
- **Unlock at boot** (`device.unlock_at_boot`, opt-in, warned): while it is on, the kernel keeps the seed in the device class and unlocks at boot, before init starts. Off, the kept seed is deleted at boot. It and the kept tags are independent: with it on a tag is never needed (the device is unlocked before the attest screen would show), switching it off leaves the tags kept, and a release deletes both.
  - **Two keeps** (2026-11-18): the kernel keeps the seed (`\0id.unlock`, device class) at a typed unlock with the setting on (op 1) and at the retype gate's match (op 12), never at a tag's (op 6, "No tags" below); the identity service (fgtw) keeps its roots, the fleet key and the chain (`fgtw.identity_seed`, `fgtw.handle_proof`, `fgtw.fleet_key`, `fgtw.chain`, identity class, each its document) whenever the setting is on and it holds them.
    It follows the setting at every pass of its loop (`follow_persist`): turned on mid-session it keeps what it holds at once, turned off it deletes them at once.
    Until 2026-11-18 it read the setting only at its start and when roots were handed in, so the two keeps could part: on the Pro (b970) the kernel unlocked at boot from its kept seed while fgtw had none of its roots (its scope held the space and nothing else), and with no attest screen nothing could hand them over.
  - **Never stuck** (2026-11-18): a boot that unlocks shows no attest screen only while fgtw holds the roots (ahau asks its state, op 1, before it decides; `ahau_core::attest_screen`).
    Unlocked and bound with fgtw holding none, the attest screen comes up over whatever started, and the handle typed (or a kept tag) unlocks again, hands the roots over and fgtw keeps them.
    Any way the keeps can still part (a reset between the kernel's keep and fgtw's, a write the vault refused, fgtw started while the setting was off) ends there, never in a device that cannot attest.
  - **On and off are an approve on the attest screen** (owner, 2026-11-24; "Approve on the attest screen" below).
    The Security page's checkbox asks the kernel for it (`SYS_UNLOCK` op 16, `ATTEST_AT_BOOT_ON` or `ATTEST_AT_BOOT_OFF`); the device locks at once, and the handle typed on the attest screen unlocks it.
    That unlock is the approve: for on, the kernel keeps the seed typed then for unlock at boot (`\0id.unlock`, its `unlock` document sealed in the device class, as a typed attest with the setting on keeps it) and turns the setting on; for off, it keeps nothing and turns it off.
    `SYS_SET_PREF` refuses the setting both ways, so the plain settings write is no way around the approve.
    The bridge (`diag pref`, a `device` document stored over USB) still sets anything, in a development build only: a production build closes those USB doors outright (service.rs, "Production builds ... close the development doors over USB").
    From 2026-11-17 to 2026-11-24 turning it on took the retype gate instead: the handle typed again on a second entry screen (ahau's keyboard, words kind 5), compared by the kernel (`SYS_UNLOCK` op 12). Its Enter submitted, where the attest screen's types a newline (a handle may hold one: owner, 2026-10-28, "enter is a valid char"), so a handle with a newline could never be typed there, and the owner met the difference (2026-11-24). The gate, its op and its screen are gone: one screen, one input path.
  - **It takes effect at once**: the kernel keeps the seed at the approve's unlock, and the identity service (fgtw) sees the setting turn on at its next pass and keeps the roots, the fleet key and the chain it holds, each as its document (`follow_persist`).
    So the next boot unlocks with no attest typed in between.
    Turned off, fgtw deletes them at once, and the kernel deletes its kept seed at the next boot.
    A window remains: a power cut between the kernel's keep and fgtw's (one pass of its loop) boots unlocked with no roots for fgtw (the Fleet page says no identity in hand); turning attest at boot off and rebooting brings the attest screen back.
  - **No tags** (owner, 2026-11-18): a kept tag never turns it on and never keeps the seed.
    A tag's unlock (op 6) unlocks without keeping, whatever the setting (`tag_unlock`). A tag approves other changes ("Approve on the attest screen"), never this one: while attest at boot on waits, the attest screen offers no tag and the kernel refuses one, the request waiting for the handle typed.
    The seed a tag unlock hands the attest screen is wiped once handed to fgtw; until kernel-tagged touch (below) the kernel cannot tell a typed seed from one a process knows.
  - **The next layer is kernel-tagged touch** (PRESENCE.md (a), a design not built): the kernel tags every touch event, and the approve's unlock takes the tagged keystrokes instead of a seed, so the handle must have been typed on this glass, this session.
    The crypto-proper answer for the stored seed (sealed under a release share the person's fleet holds, so an offline stolen device never unlocks by itself) is PRESENCE.md (b).
- **Locking.** Only when power goes, or when the device switches OS. Every reset the kernel orders drops the key first, its bytes overwritten (`platform::before_reset`), because a warm reset keeps DRAM. There are no timers.
  - **How the kernel holds them** (2026-11-17): the seed, the identity's three vault keys and the person key only sealed under the wairua, opened in registers by lifted code (KEY_REGISTERS.md "Sealed under the wairua at rest"); the exceptions, with how long each touches memory, are listed there.
  - **What a lock wipes** (2026-11-17): the kernel overwrites the identity's keys, the seed and the person key in place and cleans the lines to DRAM (`keys::wipe_to_dram`), and rings the identity's bell (SYS_BELL 13, `BELL_IDENTITY`: the lock epoch, moved at every lock).
    The identity service (fgtw) hears it and overwrites the roots, the fleet key and the person key it holds, and goes back to no identity in hand without touching the vault.
    The attest screen (ahau) keeps no copy of the roots once fgtw has them.
    Moving these keys into the kernel alone: KEY_REGISTERS.md "The identity's keys in the kernel alone".
- **The syscall.** `SYS_UNLOCK` (24), with the Unlock right, which only ahau, init and fgtw (the identity service) hold; ops 7 to 9 also with Settings (the map's settings pages, which keep tags and hold no right to unlock), and ops 13, 16, 17, 19 and 20 with Settings (the Android page, and asking for an approve):

| Op | Does | Returns |
|---|---|---|
| 0 | state | bit 0 unlocked, bit 1 bound, bit 2 the person key held (FACETS.md) |
| 1 | unlock with a seed | the entry count, NOTHING (belongs to another identity) or ERR |
| 2 | lock | |
| 3 | bind | |
| 4 | renew the device-only secret after an unbind the fleet consented to: the identity service alone, a person present, their approve of the unbind held (taken); anything else only locks | 0, ERR |
| 5 | the person's export (the departure's "save first") | the document's length, or ERR |
| 6 | unlock by a tag: its UID in, the seed out (Unlock right only) | as op 1; ERR is the one refusal |
| 7 | keep a tag for the identity present (its UID) | 0 kept, 1 kept already, ERR |
| 8 | forget a tag (the start of its id), or every tag | how many |
| 9 | list the kept tags | how many; each the start of its id and when it was kept |
| 10 | the person key from the identity service (Unlock right only; FACETS.md D1): 32 bytes, held beside the identity key and wiped with it at lock; the kernel derives each kist's alias from it | 0, or ERR while nobody is present |
| 11 | the identity's doorbell: x2 a word in the caller's arc, 8-aligned; the state written there now (read live, so an unlock at boot is in it) and at every change, bits 0-2 as op 0 with a change count above bit 8, waking whatever of the process waits on it. One word a process (a second moves it); any number of processes | 0, or ERR with no vault |
| 13 | Android on this device (the Settings right; docs/BOOT.md): start now, off, the state, what it starts into (refused while its guard is on), Android's recovery request; Android on is an approve (op 16) | 1 done, 0 refused |
| 15 | unbind (remove) this device once the fleet consented (the identity service alone; "Unbind this device" above), the typed seed matched and the person's approve held (taken): the secret renewed and the power off; flag bit 1 a shred ("The Security page" below: no consent needed, the SHRED approval, a restart) | 1 removed, 0 the seed is not the person present's, ERR |
| 16 | ask for an approve (the Settings right, never a kist; "Approve on the attest screen" below): x2 a request's word; only while a person is present on a bound device and none waits; the identity locked at once and the attest screen started again | 0, ERR refused |
| 17 | the request waiting (the Unlock or the Settings right) | its word, 0 none |
| 19 | the "require attest" guards: x2 0 read (Settings or Unlock), x2 1 guard x3 on (Settings; kept in the device's settings document) | the guards' bits, ERR |
| 20 | take an approval a process carries out itself (developer mode, the add flow's opening, a revoke or a reinstatement; Settings or Unlock): x2 its kind, x3 1 to look without taking | the approved word (taken unless looking), NOTHING while it waits on the attest screen, 0 neither |
| 21 | lock (the Settings right, never a kist; the Security page's lock): the identity locked at once and the attest screen started again, as op 16 does, with nothing asked. It only tightens, so it needs no approve | 0, ERR nobody present |

Op 12 (the retype gate, 2026-11-17) and op 14 (the Android disclaimer's typed confirmation, 2026-11-21) were removed on 2026-11-24 with the second handle-entry screen they answered.

## Approve on the attest screen (owner, 2026-11-24)

The owner, after the attest-at-boot confirm screen took Enter differently from the attest screen: asking for such a setting "should immediately lock the device anyways and drop to the attest screen, pending success to apply the setting and the button instead of attest or in the weird case you designed 'cancel' should say 'approve' or similar and keep the user locked out if fail."

**Why the Enter differed.** The confirm screen was not a copy of the attest screen's code but the same process in another mode: after attest ahau stays as the keyboard for words the identity service asks for (`host_words`, `Mode::Words`), and the retype gate (2026-11-17) was added there as words kind 5, followed by the Android disclaimer (kind 6) and the removal (kind 7). In that mode Enter submits the words, as it does for a newcomer's pairing words or a WiFi passphrase. On the attest entry Enter is a character of the handle (owner, 2026-10-28: "enter is a valid char"), typed as a newline, and the button attests. So the three confirm screens took a handle by other rules than attest: a handle with a newline in it could never be typed there. One screen and one input path now: every approval is the attest screen itself.

**How it goes.**
1. A settings page asks the kernel for a change (`SYS_UNLOCK` op 16): a request is a kind and one number (ahau_core::approve), never text. The kernel takes it only while a person is present on a bound device and none waits, and checks what only it knows (a cap the battery takes; Android allowed here before starting into it).
2. The kernel keeps the request and locks at once, as any lock: the identity's keys wiped, the identity service's bell rung (fgtw wipes its roots, the fleet key and the person key), an approved request not yet taken dropped. It starts the attest screen again over everything (the service loop's restart of `ahau`, as `diag restart ahau`).
3. The fresh attest screen reads the request (op 17) and, under its state line, says what it approves in the person's own words for its kind (ferros_lang `Approve*`, all sixteen languages). Its button says approve, and it is the only one: no Decline (owner, 2026-11-24, after testing the approve on the Pro: "Don't think there needs to be a decline. You can toggle off again if you screw up."; Decline and its `SYS_UNLOCK` op 18 were removed the same day). A kept tag approves as the typed handle does (owner, 2026-11-24, after using it: "in the new case we just added, seems you need to type your handle in, cannot use a tag, prolly should be able to"): its unlock applies the request when it opens, and a refused tag leaves the device locked with nothing applied. The exception is attest at boot on, which keeps the seed for the next boot: a tag never turns it on (owner, 2026-11-18: "no tags"), so for it the attest screen offers no tag and the kernel refuses one, the request waiting for the handle typed.
4. **Approve:** the handle typed unlocks (op 1). Opened, the kernel applies exactly the request, once, at that unlock, and the screen goes as at any attest (the roots to the identity service). The removal and developer mode are carried out by a process instead: the kernel holds the approval for it until it is taken or the next lock.
5. **A failed attest** (another identity's handle, a typo: the kernel refuses it): the request is dropped, nothing applied, and the screen is the plain attest screen, the device locked until the person attests.

**Who holds the request across the lock: the kernel**, in RAM (process.rs `APPROVAL`).
- Not the requester: the map asked and is still running under the attest screen, but nothing it can call changes a request that waits. Op 16 takes one only while a person is present, and asking locks, so while one waits nobody is present and every other ask is refused.
- Not ahau: it is started again for the approve and only reads the request (op 17); it writes nothing of it. The words it shows come from the kind, so a requester cannot put its own text on the attest screen.
- Not the vault: nothing of it touches the flash. A power cut drops it, and the next boot is a plain attest screen with nothing applied.
- The writers: op 16 sets it; a failed unlock (typed or by a tag) drops it; the approve's unlock (typed, or by a tag for every kind but attest at boot on) applies and clears it.
- Changing one's mind: a toggle is toggled back (the owner's words above). The ones that are not a toggle, unbind this device and developer mode (no off in the interface), are left by not attesting until a power cycle, which drops the request (it is in RAM alone); the attest screen offers no other way out.

**Routed through it (built 2026-11-24):**

| Kind | Asked by | Applied at the approve |
|---|---|---|
| Attest at boot on | the Security page's checkbox | the unlock keeps the seed (`\0id.unlock`), the setting on |
| Attest at boot off | the same | the unlock keeps nothing, the setting off (the kept seed goes at the next boot, fgtw's keeps at once) |
| Android on | the Android page: the disclaimer's agreement, and on again for a person who accepted before | the acceptance kept in the person's identity class if it is not yet (service.rs op 22), Android allowed here |
| Unbind this device | the Fleet page's unbind, or the Security page's | held for the identity service: the attest screen asks it (fgtw op 33) once it holds the identity again; the fleet is asked, and only once a member consents does the kernel renew the secret, with the approval held ("Unbind this device" above) |
| Bind device | the Fleet page's button | held for the identity service, which takes it as it opens the add flow ("Bind device" below) |
| Shred | the Security page | held for the identity service: the attest screen hands it the seed (fgtw op 33 [2]) once it holds the roots again, and the kernel renews the secret only with the approval held ("The Security page" below) |
| Revoke, reinstate | the Security page, a row for each other device (the number the first seven bytes of its key, `ahau_core::approve::device_arg`) | held for the identity service, which takes it once it holds the roots and the fleet key again and signs the chain op |
| A guard off, a guarded change | "Require attest" below | the guard off, the cap, what the device starts into; developer mode held for the map, which turns it on |

The kernel refuses the plain way to each: `SYS_SET_PREF` refuses attest at boot both ways, op 13 refuses Android on (its request 2), the removal's erase needs the approval held.

**Bind device** (owner, 2026-11-24: "the fleet page does nothing unique until you press 'bind new device' or similar, then it does the scan/tap enable, all that"; then, on the button's words: "might not be new. just bind device is fine.").
- The Fleet page is passive: showing it opens nothing, no NFC read for an add, no BLE add scan, no list of devices asking. Until 2026-11-24 the add flow opened whenever the page was on the glass.
- Its "bind device" button asks for the approve (`FLEET_ADD`), which locks the device; the lock wipes the identity service's add machine and closes its flow (payloads/fgtw `locked_out`).
- The kernel applies the approve by holding it for the identity service. The map looks at it (op 20, looking only) and, once the identity service holds the identity as a member again, tells it the flow is on the glass, as any opening is (fgtw op 8 [1]). The identity service opens the flow only by taking the approve from the kernel as it opens it (op 20, add.rs `add_page`), so one approve opens it once, and no other way opens it.
- Then everything is as before: NFC with the two together, the touch on a name and the green button, the words only behind "Not in range?". It closes as before: Close or Done, Back, or the page left; opening it again is another approve.

**Not routed, for the owner to decide:**
- **Approving another device's departure** (the Fleet page's Approve and Deny), **start Android now** (two taps; Android already allowed by an approve), **Android's recovery request** (allow once: a wipe of Android's data), **keeping and forgetting tags**.
- **Identity export or reveal:** none exists in the interface. The kernel's export (op 5, the departure's "save first") has no caller yet; when the Fleet page's save first is built it should be an approve.

**While the attest screen is up after an approve's lock** (audited 2026-11-24, after the owner asked: "Is that an overlay? Just want to make sure a notification from photon or a pulldown and a radio setting tap... you see where I'm going there."). At boot nothing but the drivers and the attest screen runs; an approve's lock leaves the deck, the kists and the identity service running under it, so:
- **Drawn:** the attest screen is an opaque full-screen window at the system level (payloads/ahau `show`, `window::SYSTEM`), which only a Settings holder that is no kist may take (payloads/display `level_not_theirs`, `system`): no kist draws over it. The display's own helm and console circles stay around it, as at boot.
- **Touches:** the touch server serves raw contacts to whoever holds its endpoint, and every kist does (the implicit set), as does the deck. Found and fixed: the deck read the glass under the attest screen and acted on it (its settings, the quick row, a console circle's page; a radio circle tapped switched twice, once by each), and a running kist could read where the handle was typed. Now the deck takes no touches while nobody is present (payloads/map, fact 83), and the touch server answers a kist's call with an empty glass while nobody is present (payloads/touch, SYS_CALLER then fact 83). The attest screen's own keys never go thru the display's keyboard path. Also found and fixed: any caller could set the quick row's state (display op 56), opening the row over the attest screen (its circles then took the touches meant for the attest screen's keys) and setting its screenshot bit, which op 84 keeps from every kist; op 56 and the helm's light (op 54) are the system's alone now.
- **Notifications:** a kist's notes glow on the deck's circles, which are under the attest screen; the helm's Deck button showed the latest note's kist icon, and now shows nothing while nobody is present (map notes.rs `notes_view`). The kernel's restart notes wait for a person (service.rs).
- **The radio switches:** a tap on a radio circle reaches the switch (ahau, preference 15) and nothing else now that the deck takes no touches; the WiFi circle swiped in opens the attest screen's own network page, as at boot.
- **A look-alike:** a kist cannot take the system level. A full screen of its own could imitate the attest screen while the device is unlocked; what it cannot draw is the attest mark and the keys' rainbow, made from the device's own `attest mark` key (ahau `start_fresh_mark`, `ira_key`), which no kist can ask for (a kist's ira keys are derived under a context of their own and bound to its gate identity, kernel process.rs SYS_IRA). The person's own picture forming as they type is the mark of the real screen; a fake shows another.
- **Running while locked:** the kists keep running but read nothing of the person's (the vault process drops what it holds at the lock epoch's move), reach no network (ip serves applications nothing while nobody is present, read once a second, so an application keeps it for up to a second after the lock) and now read no touches; the identity service wipes the roots, the fleet key and the person key (fgtw `locked_out`). Not checked yet: whether a running kist keeps sound, the cameras or GPS while nobody is present (no person check was found in their servers), and what a kist read into its own memory before the lock stays there.

## The Security page (owner, 2026-11-24)

The owner: "I'd like to get it synchronized in the FGTW crate so ferros and Photon render the same. So yes please, build the lock, revoke, shred, all that stuff needs to work on both." And on the words: "not retire, unbind/bind, shit like that. retire means never again which is wrong."

The page is one model in fgtw (`fgtw::security`, fgtw c7a1bff): the rows in order, when each is enabled and why not, the confirmation each needs and the words in all sixteen languages; Photon's Settings draws the same rows. ferros draws them on the settings circle's Security page (payloads/map `sec_press`), then its own row, attest at boot.

| Row | Confirmation | Carried out by | What it does |
|---|---|---|---|
| lock | none (it only tightens) | the kernel (`SYS_UNLOCK` op 21) | the identity locked as at any lock (its keys wiped, the identity service's bell), the attest screen started again; nothing deleted |
| revoke, one row a device | the approve (REVOKE) | the identity service (payloads/fgtw security.rs) | the chain's Lock op, the worker's refusal that survives a wipe of that device (`client::device_lock`), a fresh fleet key wrapped to the live members alone (rotate on remove, photon docs/device-lifecycle.md 3), adopted here, the space and the person key sealed again under it to the fleet and to fgtw.org at once (`space_reseal`): the revoked device can no longer announce, write, or open anything sealed after |
| reinstate, a revoked device's row | the approve (REINSTATE) | the identity service | the chain's Unlock op, the refusal lifted, the fan-out grown to it with the key held (never a mint) |
| shred | the approve (SHRED) | the identity service, then the kernel (op 15, the shred bit) | below |
| unbind this device | the approve (REMOVE_DEVICE) | as "Unbind this device" above | the same button as the Fleet page's |

"Revoke", not "unbind remotely": an unbound device may be bound again by anyone, a revoked one is barred until its fleet reinstates it. The device a revoke names goes to the attest screen as a number (seven bytes of its key) and the screen says "revoke one of your devices", never a name a requester chose.

**Shred** ("wipe and refetch from my fleet"; the owner tests it himself on the Pro, 2026-11-24: "and then I'll do the wipe myself to test it"; and: "if you cannot attest, there's ZERO reason my shit should get wiped and pretty sure we discussed, that's mostly key management formalities and proper lifecycle/design."). A correct attest (the approve, SHRED) and nothing else: it leaves no fleet, so it needs no consent.
- Destroyed: the keys. The identity's keys in registers and the person key (the lock), and the device-only secret, renewed (one write). Everything of the person's on this device is sealed under keys made from it, so none of it opens again: the kernel's identity class, the person's domain in every scope (`map.`, `gps.`, `wifi.`, `fgtw.`, `ahau.`, `kists.`, the hold), every kist's own domain, the kept tags, the kept seed, the bound mark (in the secret's own document). Nothing is deleted or overwritten; the vault's rotation reclaims the room.
- Kept: the device class: the drivers' and firmware capsules, `wifi.fw`, `wifi.nvram`, `wifi.clm`, the GPS replays, the sensors' registry and calibration (`aoc.reg.*`), the kernel's own vault (`aoc.fw.*`, `tpu.fw`), the device's settings document (language, base, zone, theme, brightness, guards, the setting unlock at boot itself), `wifi.net`, the seals, the shell's capsules, the ledger and flash log, devinfo, and Android. Unlike `diag "vault fresh process yes"` (a17346c5, IDENTITY-SPACE.md "A reset, observed"), which started the whole vault process over and needed the firmware and calibration scopes put back from an export, a shred touches the vault process only by the person's domains.
- No departure: the chain still lists this device, its brand at fgtw.org stays the identity's, the fan-out still wraps to its key. So the device restarts (not the removal's power off) at a plain attest screen; the handle typed there unlocks a device that is no longer bound, the look-up finds it a member already, the identity service binds it again (op 3), recovers the fleet key from the fan-out and pulls the space from the fleet and fgtw.org; the deck, the kists' list and answers come back, and the kists' capsules from the fleet or kists.fgtw.org (IDENTITY-SPACE.md "Capsules"). A kist's own data (its domain) is device data and does not come back (IDENTITY-SPACE.md "What is left").

**No wipe before attest** (owner, 2026-11-24: "if you cannot attest, there's ZERO reason my shit should get wiped"). Built on 2026-11-24 and removed the same day: two presses on the attest screen released the identity with neither the handle nor consent. A device whose handle is lost is let go by its fleet (a revoke, or an approved departure), never by whoever holds it.

**Nothing destroyed without the person** (owner, 2026-11-24): every path that can destroy keys or renew the device-only secret, and its gate (in the kernel and the identity service, not the interface):

| Path | What it destroys | Gate |
|---|---|---|
| `SYS_UNLOCK` op 15, an unbind (ferros_kernel process.rs, `SYS_UNLOCK if f.x[1] == 15`, the approval taken at `let approved = ...`) | the secret renewed, the power off | the identity service alone (`fgtw` by name, never a kist); a person present; the typed seed equal to theirs, in registers; their approve of the unbind (REMOVE_DEVICE) held and taken; called by the identity service only once the consented Remove has folded this device out (payloads/fgtw depart.rs `depart_wipe`, remove.rs `remove_erase`) |
| op 15, a shred | the secret renewed, a restart | as above, the SHRED approve; no consent (it leaves no fleet) |
| `SYS_UNLOCK` op 4 (process.rs, "Op 4, the device-only secret renewed") | the secret renewed, a restart | the identity service alone, a person present, the approve of an unbind held and taken; anything else locks and destroys nothing. Its one caller is the departure's `depart_wipe` without a typed seed in hand |
| kvault `renew` (service.rs op 4 and 19) | the secret renewed; on a device that kept a plain seed before 2026-11-24, that seed deleted (a root sealed by the device alone) | reached only thru the two ops above |
| kvault `boot_unlock` with unlock at boot off | the kept seed deleted | the setting, which only an approve turns off (`ATTEST_AT_BOOT_OFF`); a development build's bridge can write it too |
| op 8, forget a tag (or every tag) | tag records deleted | the Settings right with a person present: the person's own choice on the NFC page |
| `diag "vault fresh yes"`, `diag "vault fresh process yes"` | every vault, or the vault process's | development builds only, over the cable; a production build refuses both. For the owner: keep them as the developer's way back, or remove them too |
| the lock (op 2, op 21, op 16, a reset) | the keys in memory only | allowed always ("Lock is fine") |

**The consent is the identity service's to check, not the kernel's:** the kernel folds no chain, so it takes the identity service's word (the only process that may call op 15 or op 4 to destroy) that a member consented. The fold is the consented Remove's signatures, checked by fgtw's own fold.

**The device-only secret is the salt** (owner, 2026-11-24: "ideally we keep the salt on disk too and that's the only thing that gets actually wiped but I'm open to ideas"; then the security chip should hold it: the Pro's Titan M2, which ferros does not drive yet, a later project).
- Where it lives: `\0id.secret`, a device-class value of the kernel's vault (kvault.rs), on manifestus in two mirror regions of the ferros partition (+2 GiB and +2 GiB + 256 MiB). manifestus writes a new copy and leaves the old one where it was, so the old secret stays on the flash in both mirrors, sealed under the device class's key (from the ira), until the reap takes its blocks. Against anyone without this device's ira that is noise; against someone holding the ira (a kernel compromise, or the chip's values with the UFS desoldered) the old secret reads back until reaped.
- How it is made (since 2026-11-24, the first secret and every renewal): two independent eggs under the owner's single egg rule, KOHI gathered at that moment for this secret alone (the firmware TRNG, RNDR where present, CPU timing jitter; health-tested, the byzantine threshold) and the wairua's keyed stream over that fresh value, mixed with BLAKE3 (kvault `fresh_secret`, ferros_pieces `device_secret_mix`). Too few healthy sources (NGARO) refuses the renewal, nothing changed. Until then it was the wairua's stream over the cycle counter alone, so whoever held the wairua could find it by trying counter values (owner: "yes, you should" fix it). The ira seals the stored copy and never makes its contents.
- What depends on it: the identity root is `BLAKE3-keyed(derive_key(ira), device-only secret ‖ identity seed)` (kvault `unlock`, keys.rs `id_root`); the identity class's payload and address keys, the person's domain keys in the vault process (every identity-class scope and every kist's own domain, process.rs `domain_keys`), and the flash log's personal key all derive from that root. The kept tags' seal keys take the secret directly (`TAG_SEAL_CONTEXT`), the kept seed's too (`KEPT_CONTEXT`, since 2026-11-24), and the bound mark rides in the secret's own document. The person key is kept nowhere on the device but in the space, which is identity-class. So once the secret is renewed nothing of the person's opens, with the handle or without.
- Not built, proposed: after a renewal is written and verified in both mirrors, a UFS UNMAP of the old secret's blocks in both, then the purge (`fPurgeEnable`, waited on `bPurgeStatus`) so the device's flash erases what was unmapped. UNMAP writes nothing; the purge erases physically. Neither exists in ferros_hal's UFS driver yet (no UNMAP command, no purge flag). For the coordinator's go.

**For the owner to decide:** the button words' case (ferros's lower case, which Photon now shows too); the development `vault fresh` doors above.

## Require attest (owner, 2026-11-24)

The owner: "for any debatable settings you could add a toggle that requires attest, and to untoggle, of course requires an attest but then after that your kid can change it all day long until you toggle 'require attest' on again."

- **A guard a setting.** Each guardable setting has a "require attest" checkbox beside it. The guards are a device setting, `guards` in the device's settings document (SETTINGS.md), one bit each (ahau_core::approve::guard), kept at every change.
- **On** is one tap (op 19 [1]): tightening is always safe, so it needs no attest.
- **While on**, the setting changes only by an approve: the kernel refuses its plain write, the page asks for the approve with the value chosen, and the kernel applies that value at the approve's unlock.
- **Off** is an approve (`GUARD_OFF`). From then on anyone holding the unlocked device may change that setting, until the guard is turned on again.
- All guards are off out of the box.

**Built (2026-11-24):**

| Guard | Bit | Beside | Enforced by |
|---|---|---|---|
| The charge cap | 0 | the battery page's slider | the kernel (`SYS_CONTROL` refuses the cap) |
| What the device starts into | 1 | the Android page's choice | the kernel (op 13 refuses requests 4 and 5) |
| Developer mode | 2 | About's version line, on a release build while it is off | the map (Developer is the map's own entry); the approve is the kernel's, the map takes it (op 20) |

**Proposed guardable, for the owner to choose:** the permissions answers (a kist's rights), the deck layout and its arrangement, the radios' switches (note: the attest screen switches them too, while locked), proximity wake, the time zone and the language, the WiFi networks kept, kists stowed and unstowed, do not disturb, the helm.

## Attest with a tag (owner, 2026-11-05 and 2026-11-11)

The owner keeps a plain NTAG-type tag (NFC-A, Type 2, a 7-byte UID, no crypto of its own) in his glasses. Held to the device at the attest screen it attests without the handle being typed; without the tag nothing is remembered ("Would be nice to have NFC be on if the screen is on at the attest and have that NFC locked capsule for easy handle entry ... and ideally something in settings to add a tag"). The threat view is the owner's: it takes his device and his glasses, and a handle is a name, not a password.

**What is kept, where, under what key** (kernel, `kvault.rs` `tag_keep`, `tag_unlock`):
- One record per tag, in the device class, kept by address under the reserved prefix `\0tag.`: the address is the tag's id, so the list of kept tags is a list of ids and nothing names a UID.
- The tag's id: BLAKE3 `derive_key("ferros 2026-11-11 tag unlock id v0", device key ‖ UID)`. The device key is `derive("ferros 2026-11-11 tag unlock device")` from the ira, in the kernel; the ira never leaves its registers.
- The seal key: BLAKE3 `derive_key("ferros 2026-11-11 tag unlock v0", device key ‖ device-only secret ‖ UID)`. The device-only secret too, as the identity key takes it, so the chips' published values and the tag are not enough without this device's own 32 bytes, and a release (which renews the secret) leaves any record it missed unopenable.
- The record is VSF (section `tag_unlock`): `added`, the Eagle time it was kept, and `seal`, the identity seed sealed with XChaCha20-Poly1305 under the seal key, a fresh 24-byte nonce from the kernel's entropy in front, the tag's id as associated data. The vault seals the record again under the device class, as every entry.
- The inner seal is what binds the record to the tag. Without its check a wrong tag would give a garbage seed, and on a device not bound yet a garbage seed opens (makes) a junk identity space; with it, a wrong tag is a clean refusal.
- To seal a seed the kernel needs it when a tag is kept, from the settings page, which never sees the seed. So while a person is present the kernel holds their seed beside the identity key, and wipes both when it locks. The identity service holds the same seed in RAM for as long, and wipes it when the kernel locks (its bell).

**Keeping a tag** (the map, the NFC page, reached from Connections' NFC row or the NFC console circle): "add a tag" turns the page into a reader for one tag. The NFC driver's next unlock tag is kept (op 7) for the identity present, and the page says so. The tap on the page is the consent (above, "An NFC tap is consent"). The page lists the kept tags, each by the first four bytes of its id and when it was kept, each with "remove" (op 8); under them the warning, as attest at boot's: whoever holds the device and the tag holds the identity.

**At attest** (ahau): while the entry is up, the NFC driver's TAG (op 9) is held by a thread of ahau's; discovery reads tags only while the screen is on. The tag's UID goes from the driver to ahau and from ahau to the kernel (op 6), nowhere else, and is never logged. Opened, the kernel unlocks exactly as op 1 does (the same binding rule: a bound device opens only its own identity) and hands back the seed; ahau then goes on as after a typed handle (the proof from its space, the roots to the identity service). Refused (a tag kept for nothing here, a record that does not open, another identity's): "Denied. This tag does not open this device.", cleared by the next press, and the same tag is not tried again while the denial stands. Nothing here gives up on a timer: the hold lasts as long as the entry is up, and a finger landing on the screen while the entry is empty rings the driver's PROBE (op 11), which starts again a discovery that stopped and brings a chip that stopped answering up again from a reset pulse (NFC.md "A chip that stops answering"; owner, 2026-11-17: "why's it time out?").

**Removing.** One tag from the NFC page, or every tag: op 8. A renewal of the device-only secret (an unbind, a shred) leaves every kept tag unopenable without deleting it: their seal keys take the old secret.

**Which tags:** NFC-A, Type 2, with a double or triple size UID (7 or 10 bytes), which a tag keeps for life; a 4-byte UID may be random at every read (ISO/IEC 14443-3). Later (owner): "would be nice to have PIPE on tag".

## What the attest screen does

"Recognized" means this device holds a space for the handle: the key from its seed opens a name list here.

| Device | Handle | Result | Time |
|---|---|---|---|
| Bound | recognized (its own) | Opens: the screen goes, straight to the deck | Instant |
| Bound | not recognized | Denied | Instant |
| Unbound | recognized (an earlier attempt, join not finished) | Back to finding the identity and the join | Instant (the proof is kept in its space) |
| Unbound | not recognized | A space for this handle (an earlier attempt's left unopened, nothing deleted); the proof computed on the X3 and kept; then: an identity exists → join; none → mint, typed twice; no answer → still looking, the screen waits | The proof once, about 5 s |

The handle proof is public (beacons and the registry carry it), so keeping it in the identity's sealed space (`ahau.proof`) costs nothing. The identity seed is never kept unless unlock at boot is on.

## What the attest screen says

Owner, 2026-10-31: simple and direct. "Owner" rows are the owner's words (or "close to that", as given); "proposed" rows are mine, written in the same style and waiting for the owner's correction.

| When | Text | Button | Words |
|---|---|---|---|
| Unbound, waiting | This device is unbound. Present an identity to bind it to. | attest | owner (2026-11-24) |
| Bound, waiting | This device is bound. Present its identity to unlock it. | attest | owner (2026-11-24) |
| Denied | Denied. This handle does not unlock this device. | | owner ("DENIED"); the second sentence proposed, the same for a typo and another identity |
| A tag refused | Denied. This tag does not open this device. | | proposed (the owner's "Denied." first, as above) |
| The proof running | Computing identity keys. | | owner |
| Looking up | Finding this identity. | | owner |
| An identity exists | This identity already has devices. Add this one? Nothing reaches them until you say yes. | Yes, I really would like to add this device and this is my identity and not a typo | the button owner (2026-11-22); the text proposed |
| Confirmed | Put the two devices together, the other one's screen on, with "bind device" pressed and approved on its Fleet page. Or touch this device's name on that page, then its green button. (this device's name below) | Not in range? Show words to copy/paste or dictate | the button owner (2026-11-22); the text proposed, after the owner's |
| Words asked for | (as above, the pairing words below the name) |  | owner |
| None exists | This handle is unclaimed. Minting is permanent. Type your handle again to mint your identity. | mint | owner |
| The two differ | They don't match. Try again. | | proposed |
| Minting | Minting. | | proposed |
| Mint failed | Minting failed. Enter to retry. | | proposed |
| No answer | No answer yet. Still looking. |  | proposed |
| Wrong slot | This handle's record doesn't match. Start over with another. | start over | proposed |
| An approve waits | (the state line) Approve: (what, from the request's kind; "Approve on the attest screen") | approve (no decline) | the button the owner's ("should say 'approve' or similar", 2026-11-24); the text proposed |
| An approved removal refused | (why: the identity's only device, no fleet) | done | proposed |

No step after the first entry has a way past the screen (owner, 2026-11-04, removing "go on": "I do not see a point. seems like another security nightmare"): the join's screens and "No answer yet" wait, and close only when the fleet binds the device; `back` returns to the entry (from a confirmed join it withdraws the request).

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
- WiFi credentials are already identity class (`wifi.cred`). The per-network MAC is worn by whoever is attested (owner, 2026-11-09): derived by the kernel from the person's identity and this device's ira, over the network's name, the same on every join of that network by that person on this device (WIFI.md "The MAC"), whichever vault entry the credential came from.

**Moving with the person.** Every identity-class entry is what a new device of the fleet should receive: the fleet's chain carries the identity key's derivations, and the entries themselves travel sealed over Photon's fleet channel. That transport is Photon's and is not designed yet; the vault side is only "copy every `IDENTITY` name".

## Radios while locked (owner, 2026-11-04)

Every radio is up at the attest screen: the modem, the WiFi chip, Bluetooth and NFC. Being up while locked is not a larger surface than being up after attest, and a locked device has less to lose (no identity key in memory, the person's half of the vault sealed). Three conditions make it safe:

1. **Every radio's DMA is fenced.**
   - The kernel owns the fences, and no process may write one (`s2mpu::is_fence`, refused in `SYS_MMIO_WRITE`).
   - The modem (HSI1) is held to its carve-outs, the WiFi chip (HSI2) to its windows, and BLK_MISC's masters are denied everything (2026-11-04).
   - The modem's fence matters most: any base station reaches the modem before anyone attests.
2. **No stable identifier goes on the air while locked.**
   - Measured 2026-11-04: the WiFi driver neither scans nor joins until `wifi.cred` opens (`wait_cred`), so the chip is up but sends nothing. Since 2026-11-09 it scans when the network page asks, and joins a network handed in before attest (or kept from one, `wifi.net`) under a MAC that lasts only this boot (since 2026-11-14 the kernel's per-power-session key from the wairua, the same on every restart within the boot, so restarts take no new DHCP leases; a fresh random one before); with no credential and no page asking it still sends nothing. Bluetooth advertises only when asked, under a fresh non-resolvable random address each time, and refuses to while locked. A locate beacon would rotate its keys.
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
- **Back to the attest screen** once a person is unlocked: an approve (the kernel's lock and the attest screen started again, "Approve on the attest screen"), the Security page's lock (op 21, since 2026-11-24), or a power cycle.
