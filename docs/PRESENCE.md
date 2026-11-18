# Proof of presence (design, 2026-11-17; not built)

The owner, 2026-11-17, on unlock at boot ("attest at boot"):

> from a security perspective we can change the UI to require a handle to be typed in to enable attest at boot (since my toddler could do it without me) BUT is there a way to do it crypto proper?

and later:

> I keep thinking there's a need for spaghettify("handle plaintext", proof that they actually typed the handle in) = user typed it in?

Three answers, in the order they can be built:
- **The retype gate** (built 2026-11-17, AUTH.md "Unlock at boot"): turning attest at boot on takes the handle typed again, compared with the person present's in the kernel.
  It answers the toddler.
- **(a) Kernel-tagged touch**: the handle provably typed on this glass, now, by a finger, not handed in by a process that knows it.
  It answers "proof that they actually typed the handle in".
- **(b) Unlock at boot under a release share**: the stored roots sealed under a key half of which lives on the person's other devices.
  It answers "crypto proper": a stolen device never unlocks by itself.

(c) says what each protects and what it does not, against the owner's threat model (harm, not observation).

## What the retype gate leaves open

The gate (AUTH.md; ahau_core::retype; SYS_UNLOCK op 11) compares a seed with the seed the kernel holds.
It cannot tell how the seed came to be: typed by a person on the keyboard, or handed in by code that already knows the handle.
- **The kernel takes the call from ahau alone** (by name, as PREF_DND is fgtw's), because fgtw holds the Unlock right and the seed for the whole session; without that check it could switch attest at boot on with no handle typed.
- **ahau itself** saw the handle at attest and keeps no copy (KEY_REGISTERS.md "Where they are now"), but a compromised ahau could keep one and call op 11 at any time.
- **Development builds inject touches** (INPUT-INJECT.md: the touch process's op 3 and payloads/inject), which reach ahau exactly as a finger's do; a production build has no injection op, but nothing in the kernel can tell the two apart.

What closes all three is the same thing: the kernel must see the keystrokes, not only their result.

## (a) Kernel-tagged touch

### The tag

Every contact event the touch controller produces is tagged by the kernel as it arrives, before any process sees it.
- **The key**: `K_touch`, derived from the wairua at the first presence prompt of a power session (`derive_key("ferros 2026-11-17 presence touch v0", wairua)`), held sealed under the wairua like the identity's keys (KEY_REGISTERS.md "Sealed under the wairua at rest").
  It dies with the power session, so a tag from one boot means nothing in the next.
- **The tag** is a chain: `tag_k = keyed_hash(K_touch, tag_{k-1}[..16] ‖ k ‖ t_k ‖ event_k)`, where `k` is the session's event counter (u64), `t_k` the counter time the kernel read it at (u64), and `event_k` the 12-byte contact record (id, phase, kind, pressure, x, y, major, minor; payloads/touch's record).
  At most 128 bytes a message, so it runs in registers (`keyreg_held_keyed`), the key never in memory.
  The chain fixes the order: no event can be dropped, reordered or spliced in from elsewhere without breaking every tag after it.
- **The kernel keeps only the head**: the counter and the last tag, 24 bytes.
  The events themselves go on to the touch endpoint as today, each with its counter and tag (16 bytes a record more).

### Where the kernel sees the events

Today it does not: the touch driver is an EL0 process (payloads/touch, KERNEL-OUT.md move 1) that reads the controller's FIFO over its own SPI port grant.
A tag the driver asks for is a tag on whatever the driver says, so the kernel has to read the FIFO itself.
- **Proposed**: the event read becomes a kernel call.
  The driver keeps its port for bring-up (the handshake, scan mode, the GPIO6 fix-up), then gives the grant up and reads events thru `SYS_TOUCH_READ`: the kernel runs the FIFO's read transaction on the port (ferros_spi, which it already links for the power switches) and returns the records with their counters and tags.
  The interrupt line stays the driver's to wait on.
- **Cost**: one syscall per FIFO drain, as now one MMIO burst; the transfer is at most 64 bytes, inside the FIFO.
- **What stays in EL0**: the table, the squash, CHANGES and HELD, every consumer.
  HELD's records are tagged too (a held contact's last tagged event).

### The submission

A presence prompt (the keyboard asking for the person's own handle: kind 4 today, the mint's retype, the others below) runs like this:
0. ahau opens the prompt with the kernel (`presence_open(kind)`): the kernel notes the counter `k0` and returns a fresh 32-byte nonce from KOHI, which names this prompt.
   Only one prompt is open at a time; opening another, or a lock, closes it.
1. The person types; ahau keeps every tagged record from `k0` on, as it already reads them (`call(touch, [0 or 1])`).
2. Enter: ahau submits the handle as typed (its UTF-8) and the records from `k0` to the enter's lift (`presence_submit(nonce, handle, records)`).
3. The kernel checks, in this order, refusing on the first failure with one typed reason:
   - the prompt is the open one (its nonce) and the records start at `k0`, with no counter missing up to the last;
   - every tag, recomputed in registers from the head it kept at `k0`;
   - the decode: replaying the records thru the keyboard yields exactly the handle submitted (below);
   - the seed of that handle (ahau_core::identity_seed) is the person present's (`retype_matches`), for prompts about the person's own handle.
4. Only then does the prompt's action happen, in the kernel (attest at boot on), or does the kernel hand back the presence proof for the action that runs elsewhere (a mint's genesis).

The records a submission carries are about 28 bytes each; a twenty-character handle with its shifts is about 50 taps, each an enter, a few moves and a leave: a few KiB, one call.

### The decode

The kernel must turn the touches into the characters ahau showed, or the proof proves nothing about the handle.
- **One decoder, linked twice**: honeycomb's `Keyboard` and `Touches` (the geometry, the nearest-centre hit test, shift and page state, the per-finger tracker) plus the entry line's editing (insert at the blinkey, delete before it, the blinkey moved by a tap or a drag on the line) as one no_std crate, used by ahau to type and by the kernel to replay.
  ahau's `insert_at` and `delete_before` move into it.
  A decoder in each would drift, and a drift reads as "they don't match" for a handle typed correctly.
- **The geometry is the kernel's**: the panel's size, `device.layout`, `device.scale`; never ahau's word for them.
- **Two-finger scrolls** of the extent (digits, symbols) are touches too, replayed the same way.
- **What the decode does not need**: timing.
  Tap versus swipe is by distance, never time (input_rhe_on_glass), so replaying the records gives the same keys whatever the clock.

### The proof

Once the checks pass, the kernel derives the proof from a small VSF document, never from bytes run together (the owner, 2026-11-17: "is | not ,"; typed fields leave no boundary to argue about):

| Field | Type | What |
|---|---|---|
| `handle` | `x` | the handle as typed (vsf's text type, the bytes ahau_core::x_bytes makes) |
| `events` | `hb` | BLAKE3 over the records from `k0` with their tags, in order |
| `prompt` | unsigned | which prompt (the kinds below) |
| `nonce` | `hR` | the prompt's nonce |

Section `presence`, inside a complete VSF document stamped with the Eagle time and its provenance hash.
`proof = spaghettify(the document's bytes)`.
- The document exists only in kernel memory for the derivation and is wiped after; the handle is never stored or sent.
- The proof is one-way (spaghettify: BLAKE3's XOF, the chaos amplifier, the smear hash; ihi) and says nothing of the handle or the events.
- What it is for: a commitment the action can carry.
  The kernel logs the start of it with the action in the flash log (soon the ledger), and an action that leaves the device (a mint's genesis, a fleet approval) can carry it signed by the device key, so the fleet's other devices see that this device's kernel saw a person type.
  They cannot check it themselves (the key died with the session); they trust the device's kernel, as they already trust its signatures.
- The owner's word for it is spaghettify; in ihi, `spaghettify` is the chaos hash and `handle_proof` is the memory-hard one (24.9 MB, 17 rounds).
  The proof needs no memory-hardness: its inputs are not guessable offline (the events' hash and a fresh nonce), so spaghettify alone is right.

### Which prompts ask for it

Required, because each gives something lasting to whoever holds the device:
- turning attest at boot on (the retype gate's prompt, kind 4);
- minting a new identity (the second entry of the handle);
- keeping a tag to attest with (it is attest at boot with a tag held to the device);
- approving a new device into the fleet (the sponsor's side of an add, today a tap on Approve);
- starting this device's departure, and a developer's release.

Not asked:
- attest itself: the handle typed at attest already is the proof of knowing it, and the kernel has nothing to compare it with before the unlock;
- turning anything off (attest at boot, a radio, a kept tag removed);
- adding a person (their handle, the People page): it gives no one anything of this person's.

### Development builds

Scripted tests drive the glass from the bridge (INPUT-INJECT.md).
- **In a development build**, an injected record carries a tag under a second key, `K_inject` (`derive_key("ferros 2026-11-17 presence inject v0", wairua)`), and a flag in the tag's input that says so; the touch process's op 3 asks the kernel to tag what it injects.
  The kernel accepts a chain that mixes real and injected records, decodes it the same way, and writes "presence: injected input accepted (development build)" to the flash log with the prompt's kind every time.
  So the bridge's `input` commands test the whole path, the decode included.
- **In a release build** (`FERROS_PRODUCTION`, as the inject op and the kernel's diag commands are), `K_inject` and the code that checks it are compiled out, not refused: a release kernel has no key an injected record could be tagged under.

### The extra egg

The records of a typed handle carry what no program chose: where each finger landed to a fraction of a key, its pressure and contact size, the counter time between strokes.
That is a few bits per stroke of physical jitter, and the kernel holds it, tagged, at the moment it decides.
- Folded into KOHI as one more egg (feedback_single_egg_rule: never one basket), never credited as full entropy: `events`' hash goes in as is.
- It matters most where a key is minted from the person's act: the person key's mint (FACETS.md D0), whose eggs today are KOHI, the jitter egg EL0 hands in, and the fleet key.

## (b) Unlock at boot under a release share

### Today

Attest at boot keeps the identity seed in the kernel's vault, device class (`\0id.unlock`), sealed under keys from the ira.
The ira is a function of the chip's published values (VAULT-KEY.md: binds, does not protect), and the device-only secret beside it is on the same flash.
Everything the person has on the device opens from that seed (the identity class is sealed under a key from it), so the setting's warning is exact: whoever holds the device holds the identity, offline, forever.

### The design

The seed is kept sealed under a key one half of which is never on the device at rest: the **release share**, held by the person's other devices (VAULT-KEY.md "The release share", ira_network_owns_assignment).

```
K_boot = derive_key("ferros 2026-11-17 unlock at boot v1", ira-key ‖ device-only secret ‖ share)
stored = XChaCha20-Poly1305(K_boot, seed), the share's id as associated data
```

- **The share**: 32 bytes from KOHI, drawn when attest at boot is turned on, one per device.
  Its id is `derive_key("ferros 2026-11-17 release share id v0", share)`, which the device keeps beside `stored` to ask for it by.
  The device never keeps the share: it is sent to the fleet and wiped, and at boot it lives in registers only long enough to make `K_boot` (wairua-class residency, ira-class lifetime at its home).
- **Where it lives**: on each fleet device that approved it, in that device's own vault (identity class, named by the device it is for and the share's id), never in the synced identity space, which reaches the device the share is for.
  More than one holder so one device off or lost is not the end of it; K-of-N Shamir across devices and friends later (token_recovery_shamir_not_oprf), the TOKEN's endgame (PIPE in the owner's pocket holds it).
- **Turning it on** follows the departure's pattern (PHOTON.md "Leaving the fleet"):
  0. the retype gate, and (a) once it is built, on this device;
  1. this device sends a signed `share_req` (its device key, the share sealed to each member's X25519, a fresh salt) to every fleet device: an NFC tap, BLE, the LAN, the FGTW peer network, fgtw.org's slot last;
  2. each fleet device that receives it notifies the person, and the Fleet page shows it with **Approve** or **Deny** ready;
  3. Approve keeps the share and answers signed; Deny records the salt as denied, so asking again is a new request;
  4. the first approval turns the setting on here (`stored` written); until then the setting says it is waiting, and nothing is kept.
  A fleet of one device cannot turn it on: there is nobody to hold the share (open question 2).
- **At boot**, before the attest screen would show (the kernel's `boot_unlock` today):
  0. the kernel finds `stored` and its share id, and asks for the share; the question is signed by the device key (from the ira, so it exists before anyone attests);
  1. it goes to the fleet on the LAN first (UDP 4383, photon_on_ferros_no_bootstrap), then over fgtw.org (which relays the question to a fleet device online and never holds a share at rest);
  2. a fleet device holding the share checks the question's signature against the chain (a member, not removed), that the share was not zeroed, and answers it sealed to an ephemeral X25519 key in the question, signed by its own device key;
  3. the kernel opens it into registers, makes `K_boot`, opens `stored`, unlocks as `boot_unlock` does now, and wipes the share and `K_boot`.
  The answer carries the fleet's signature, so taking it while locked keeps AUTH.md's rule 3 ("Nothing acts on radio input while locked unless it carries the fleet's signature").
- **No fleet reachable**: no share, so no unlock; the attest screen shows as if the setting were off, and the handle typed opens the device as always.
  The person loses nothing but the convenience, and a device on a plane or in a basement still opens.
- **Zeroing**: the Fleet page's lost-device action (and any device's "forget this device's share") deletes the share on every holder and tells the others.
  From then on the device's `stored` is ciphertext under a key nobody has, wherever the device is, whether it was offline since it was taken or not.
  Turning the setting off on the device itself deletes `stored` and asks the fleet to drop the share.
- **What the fleet devices answer automatically**: the share, to its own device's signed question, while not zeroed.
  No person approves each boot (that would not be unlock at boot); the person approved once, when turning it on.

### Costs

- **A network before attest**: the LAN fetch needs WiFi up while locked, and the person's networks (`wifi.cred`) are identity class, so the home network must be kept as the device's own `wifi.net` too (ATTEST-UI.md "WiFi connect"), or the fetch goes over cellular to fgtw.org.
  Either way a pre-attest process (or the kernel thru `ip`) sends and receives for it: the first thing besides locate (AUTH.md "Locate") to speak before a person is present.
- **Another device on**: a fleet device must be awake and answering, at home or online.
  A person with one device in their pocket and the other off unlocks by typing, as today.
- **Time**: a LAN round trip and a seal, milliseconds; fgtw.org's relay, seconds; the boot waits for neither longer than a short window before showing the attest screen (no timer past that: the screen is simply there, and a share arriving later unlocks as a tag does).
- **Code**: the share request and answer frames (fgtw's protocol crate, shared with Photon), X25519 and Ed25519 signing in the kernel or in a pre-attest process (KEY_REGISTERS.md open question 0 asks the same of the fan-out), the Fleet page's approval and zeroing, and `stored`'s new format with its old reader removed the day it lands.
- **The fgtw roots kept with it** (`fgtw.identity_seed`, `fgtw.handle_proof`, `fgtw.chain`, `fgtw.fleet_key`) are identity class, sealed under the key the seed makes, so they are as safe as `stored` without more work.

## (c) What is protected, and what is not

The owner's threat model (threat_model_harm_not_observation): being watched is fine; intent to harm is the line.
A handle is a name the person proves, not a password (feedback_handle_not_password): people near the owner may know it, and that is not a breach.

| Who | Retype gate (built) | (a) tagged touch | (b) release share |
|---|---|---|---|
| A toddler with the unlocked device | cannot turn attest at boot on, mint, or keep a tag | the same, and a scripted tap cannot either | n/a |
| A process that knows the handle (a compromised ahau, a kist that learned it) | ahau can still call op 11 with the seed | cannot: no tag without a finger on this glass, this session | n/a |
| fgtw (holds the seed all session) | refused by name | refused: no tagged chain | n/a |
| A development build's injection | passes, as typing | passes, logged, and only in a development build | n/a |
| An adult who knows the handle, holding the unlocked device | can turn it on | can turn it on (they typed it) | can turn it on only with another fleet device's Approve |
| A thief with the device, offline | (with it on) holds the identity | the same | cannot unlock: no share; the attest screen asks for the handle |
| A thief with the device, online, before the owner acts | holds the identity | the same | unlocks until the share is zeroed, then never again |
| A thief at the owner's home, on the LAN | holds the identity | the same | unlocks: the fleet answers its own device at home (open question 3) |
| A lab with the flash and the chip's values | the seed, from `\0id.unlock` | the same | ciphertext without the share |
| A compromised kernel (EL2) | everything | everything | everything the kernel sees after a share arrives |
| Someone watching the handle typed | not designed against | not designed against | not designed against |

What none of this changes:
- The handle is the identity's root; anyone who knows it can attest on a device not bound to someone else.
  Presence proves a finger typed it here, not whose finger.
- Off is always one tap: turning protections off is never gated, because the harm runs the other way.
- The bridge (USB, development) can still set any preference and store any document; BRIDGE.md's signed commands are its answer, not this.

## Open questions

0. **The touch read in the kernel** (a): `SYS_TOUCH_READ` moves the FIFO read back across the line KERNEL-OUT.md drew (the transaction only, the driver keeps the rest).
   Or keep the read in the driver and treat the touch driver as trusted, tagging at the driver's hand-off: it protects against everything but a compromised driver, for no kernel change in the SPI path.
1. **The decoder crate** (a): honeycomb plus the entry line's editing, shared by ahau and the kernel; the kernel then links honeycomb's geometry (no font, no drawing).
   Is the kernel the right place, or a small presence process holding `K_touch`'s derivations thru a kernel call, which the kernel trusts as it trusts the vault process?
2. **A fleet of one** (b): no other device to hold the share.
   Options: a friend's device (K-of-N, Shamir) or fgtw.org holding the share for a fee (it then answers any question signed by the device, so a thief online unlocks until the owner zeroes it from another device or the web), or attest at boot is simply not offered until there is a second device.
3. **At home on the LAN** (b): a thief who takes the device and powers it at the owner's home gets the share from the owner's own devices.
   Options: answer only while the holder's own screen has been unlocked recently (the person is around, which a thief at home also is), a proximity check (BLE, NFC tap) the owner can turn on, or accept it (the threat model: an evil maid at home is already inside).
4. **Over fgtw.org** (b): relay the question to a fleet device online (no share at rest on fgtw.org), or allow LAN-only answers as a per-device choice ("unlock at boot only at home").
5. **The fleet key with it** (b; KEY_REGISTERS.md open question 8): keep the fleet key sealed under `K_boot` beside the seed, so the vault process never sees it, when attest at boot is on.
6. **What the proof is carried in** (a): the flash log until the ledger is built (LEDGER.md), the genesis for a mint, the add's approval frame for a fleet add; does Photon's side of each frame need a field for it, and should Photon on other platforms produce one of its own (Android has no kernel to tag touches; there the proof would be a weaker app-level one, or none)?
7. **Shape writing** (a; input_rhe_on_glass): a traced word is decoded by a dictionary, not a nearest-centre tap.
   Handles are typed by tapping (exact bytes), so presence prompts take taps only; a trace on a presence prompt is refused, or the decoder grows the trace decoder too.
8. **The tag's cost on every touch** (a): about three compressions a record (the check, the pad, the hash), around 1,400 cycles on the X3 (KEY_REGISTERS.md's table), for every record of every touch, all session, whether or not a prompt is open.
   Or tag only while a prompt is open, which needs the kernel to know when one is (it does: `presence_open`), and leaves the rest of the session's touches untagged.
9. **The retype gate once (a) is built**: op 11 takes a seed today; it would take the submission instead, and the seed-only form goes the same day (no compatibility path for a weaker form).
10. **The kept seed's timing** (b and today): the seed is kept at the next unlock that has the setting on (the next typed attest), not when the setting turns on, because fgtw keeps its roots on disk only when the roots are handed to it.
    With (b) the share request needs the seed sealed at once; fgtw then writes its roots when it sees the setting turn on, not only at op 4.
