# Photon on ferros

> **Superseded (owner, 2026-11-05):** Photon is one full app on every platform, ferros included: the real photon-messenger crate built as `std/photon` (STD-TARGET.md), running on the device since 2026-10-27. FGTW is attest, identity and the OS's sync. The `photon_core` code below is a reimplementation written before the real app ran here: nothing more is built on it, and it is kept only until the owner decides how to retire it (STATUS.md, "Photon and FGTW on ferros").

**The Photon app is not part of ferros** (owner, 2026-11-04: "Photon is just an app. on non ferros it just happens to have the FGTW bits in it too. Just like Mahere, or Lumis"). What this page built is the FGTW side, the identity and the fleet, and on ferros that is the OS's own: the process `fgtw` (payloads/fgtw), part of attest, started by init once a person attests, with the vault scope `fgtw.`. Photon the messenger is an app a person installs, and nothing of it is in the base OS. It was the process `photon` until the same day.

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
- **The seed over plain HTTP.** fgtw.org serves port 80 (Claude memories/fgtw_plain_http_for_ferros.md). The client resolves it through the DHCP-given DNS server and POSTs over the `ip` process's TCP (ops 4-7), as payloads/httptest does: `SeedHttp`, ferros's `fgtw::client::FgtwTransport`. Everything above the wire is fgtw's own, built without std (feature `protocol`, fgtw c5b779b): `fetch`, `ensure_member`, `mint_fleet_key`; the verdict is `fgtw::attest::attest_verdict` (e1deb7d), the function Photon's probe runs too. fgtw's clock is set from the kernel's UTC before each call, its entropy is the kernel's stream (grant slot 5).
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

Verified on the host: photon-interop shows the app parsing the device's `depart_req` and its consent verifying under every scheme the device holds. Not yet run against a live approver.

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
