# Kists on the map

Status: designed 2026-11-07; permissions decided and the first cut built 2026-11-08 ("Permissions" and "Built" below). The owner: "should be nice round circles on the home screen for each kist." A kist is a stowed app as this device runs it (GLOSSARY.md: the developer's signed capsule after the device checked, compiled and sealed it); its circle on the map is its kist circle (SHELL.md). The base OS bakes in one circle, settings (no_baked_apps): every other circle is a kist the person stowed, and the map knows nothing of it until then. Kists that live on the map as live circles and are never opened (the clock, tides) are pucks, proposed in PUCKS.md (2026-11-11).

## What exists to build on

- **The apps.** Photon (`std/photon`, tools/photon-build.sh) and Mahere (mahere's `mahere-ferros`, tools/mahere-build.sh) build for `aarch64-unknown-ferros` as the real apps (STD-TARGET.md) and run when spawned over the bridge. Lumis does not yet (below).
- **Spawning from the vault.** `SYS_SPAWN` starts a signed capsule stored in the vault by its key, handing the child copies of the caller's own grants (ferros_sys `spawn`, `spawn_unique`); the signature gate checks it. init's keepers use it for every driver.
- **A surface per app.** `--dma surface` (and the map passing its `surfaces` region at spawn) gives the child one full-screen surface of its own (WINDOWS.md "Built"); fluor's ferros host draws there and asks the display to show it (op 22).
- **The open circle as an app's region** (COMPOSITOR.md, 2026-11-07): the open circle's inside is the app's, its clean window made once.
- **Domains in the vault** (VAULT-OUT.md, decided 2026-11-07): each kist's data is its own domain, keyed by the kernel from the kist's developer key and option; the person's things across kists are the common area; a permission is asked once per person, per kist, per permission.

## The stowed list

- **Where:** the person's common area (VAULT-OUT.md), under the name `kists`, identity class: it is the person's (which apps they have and where their circles sit), never readable before attest, and it travels with them to their other devices as the arrangement does. The capsules themselves are device data (a capsule is the developer's, public), each kept in the vault under its own content address (blake3 of the signed capsule), so two people on one device who stowed the same app share one copy.
- **Its form:** one VSF document, a section `kist` per stowed kist, every field typed and named (no positions, no text carrying structure):
  - `capsule` (`hb`): the capsule's blake3, the vault key of its bytes;
  - `developer` (`ke`): the developer's Ed25519 key as the gate saw it, and `option` (`u`), together the kist's identity for its domain and its permissions;
  - `name` (`d`): the app's own name, for settings and the console (circles show icons, not names, SHELL.md);
  - `icon` (a `v` image: VSF RGB gamma 2 codes cut to a circle, as tools/app-icons makes them), so the map decodes nothing;
  - `wants`: a nested section per right the kist asks for (`display`, `touch`, `surface`, `udp`, `gps`, `entropy`, `ira`, a common-area name …), each with the person's answer (`granted`, a typed bool, and when, Eagle time `e`);
  - `share` and `ring` (`u`): its circle's size and place, as the map's arrangement keeps them today (`map.` scope) for the settings circle.
  - and, beside the kists, a section `pin` per developer the person trusted on first stow (below, "Trust on first stow").
- **Stow** (the word since 2026-11-11, "Stowing" below: one stower for every source; the stower is a kist of its own, or settings' page, never the map): the capsule arrives (over PT, a Photon transfer, a download), the gate checks its signature, the device compiles and seals it (KERNEL.md "Stow, compile, seal, go"), the vault keeps it by its blake3, and the stower appends a `kist` section to the person's `kists`, asking the person for each want once. A capsule of 47 MB (Mahere's development image) or 100 MB (Photon's) is past the kernel's 3 MiB store path; the vault process takes values in pieces (VAULT-OUT.md), and the stow goes thru it.

## The map lists them

At attest the map reads `kists` (it already holds the person's arrangement) and adds a kist circle per section after the settings circle, its icon inside, sized and placed by `share` and `ring`: the arrangement's placement fit (payloads/map) takes them as it takes settings. Nothing about a kist is compiled into the map; `CIRCLES` stays the settings circle alone, and the rest are data.

## A tap launches

- The map holds the display, touch and the surfaces region, not the rights a kist may have been granted (gps, udp, its vault domain), and it must never hold them: a map that can hand out the GPS is a map that can read it. So the map does not spawn a kist; it asks **init** (which holds every grant and already starts processes by vault key) over a `launch` call naming the kist by its `capsule` hash.
- init reads the person's `kists`, finds the section, checks the capsule's hash and the person's answers, and calls `SYS_SPAWN` with a code per declared want: an asked right where the person granted it, an empty slot where they did not, an implicit right always. The kernel lays the slots out ("The implicit set" below: Mahere 1 display, 2 touch, 3 surface, 4 ira, 5 its vault domain, 6 entropy, 7 gps, 8 udp; Photon as it declares them, 1 display, 2 touch, 3 surface, 4 udp, 5 its domain, 6 ira, 7 entropy). After its wants the kernel adds three slots of its own, the same for every kist and asked of no one (owner, 2026-11-12: "I feel like GPU should be implicit"; GPU-KISTS.md 1.6): `use gpu`, a block of the layer arena of its own, and a second surface for a GPU mailbox (ferros_sys::gpu `kist_slots`: wants + 1 to wants + 3; Mahere's 9 to 11), each an empty slot when there is none to give, all three empty on a production build; then sound out (Mahere's 12, Photon's 11). One process per kist (`spawn_unique` by the kist's name): a second tap brings the running one forward (built 2026-11-11: the kernel answers a running kist with its surface and generation, init passes them on with RUNNING, and the map raises that window, display op 59; WINDOWS.md "The helm").
- The window opens in the kist circle: the circle's flight to open (COMPOSITOR.md's flight, op 45/46) lands on the kist's surface as the open circle's region, the app drawing its frame inside the circle and the bars staying the map's. Closing (the circle's own gesture) flies back; the process keeps running until the person ends it or the device needs its memory (death notices, WINDOWS.md order of work).

## What each piece needs

1. **Vault process:** values in pieces for capsules (in progress, VAULT-OUT.md); the common area's `kists` name with its grants (settings and the stower write, the map and init read).
2. **Kernel:** a kist's vault domain at spawn (VAULT-OUT.md's domain keys) in place of today's fixed `STORES` list (`map`, `gps`, …, `test`); `SYS_SPAWN` streaming a large capsule from the vault into its arc as `bridge spawn` streams one over PT (the kernel's 4 MiB staging slot); badges and death notices (WINDOWS.md).
3. **init:** the `launch` endpoint above.
4. **map:** kist circles from `kists`, and the tap's `launch` call (another session owns payloads/map and payloads/display today; nothing here touches them).
5. **Stower:** the stow flow above.

## Permissions (owner, 2026-11-08)

0. **A grant is the permission.** A kist can do only what it is handed at spawn: no permission-check calls, no ambient authority.
1. **Wants are declared in the signed capsule**, each a named right with one line of the developer's reason. Nothing undeclared can be asked for: the kernel reads the declaration from the capsule it launches and refuses any asked right not declared in that slot. The implicit set ("The implicit set" below) needs no declaration.
2. **Asked once, ever, per person, per kist, per permission**, for every right but the implicit set, which is never asked. The kist is its gate identity (developer key and option), not its name; the answers are the person's (identity class), so every device of theirs knows them.
3. **Default off** for every want. An update that adds a want asks that want (off until answered); the other answers stand.
4. **The person's defaults**: a default per right (the Permissions page) sets where the first-launch question's switch starts. Out of the box every one is off.
5. **No lying**: a right said no to is an empty slot, never a fake, and a kist runs without it (Mahere without GPS; Lumis without the camera has no cameras to pick).
6. **Kists can never ask.** No call makes a prompt. The system asks once, at first launch: every unanswered asked want on one screen, each a switch (never the implicit set). (Replaced 2026-11-10, owner: "maybe I never share my location with anyone so there's no reason for Photon to ever ask me": what a kist needs to run at all is asked when it is stowed, everything else at the person's first action that needs it, under the gesture ticket; "The permission list" below. The first-launch screen is what is built.)
7. **Revoke = kill and respawn** without the right. Every kill and every end, of any process, is one shot dead: no end hooks, no signals, no graceful shutdown, no teardown code; the kernel reclaims by dropping (and wipes the arc as it does).
8. **Settings pages**: per kist (its card on the Deck since 2026-11-12, "The deck and the card" below; the Kists page before), per right (Permissions: who has the GPS, who has the network), and the defaults, in the system settings circle.
9. **Kist circles** on the map, round, one per kist; a tap asks init to launch it (init holds every grant; the map holds none of the kist's), the first launch showing the question first.

Later, not now: a system picker that hands a kist just the file the person chose (no permission needed: the choice is the grant); the console showing live use of a right.

## The implicit set (owner, 2026-11-12)

Owner, 2026-11-12, "Yes on the implicit set", to the proposal that a kist the person chose to run needs a window, their touches in it, its own storage, randomness, its own key sealed to this device and sound out, and that asking about those only teaches people to tap yes. So these rights are handed to every kist at launch without a question: **display, touch, surface, domain** (its own vault domain), **entropy, ira** (its own key sealed to this device) and **audio** (sound out). The GPU was already handed to every kist, and is no right ("I feel like GPU should be implicit", 2026-11-12; GPU-KISTS.md 1.6). Questions are for the rest: today gps and udp.

- **The set** is `ferros_kist::IMPLICIT`, by right code. The codes did not change, so an answer a person gave one of these before stays in their list and keeps its meaning; it is no longer read, and a "no" given then is a yes now.
- **The slots** (`ferros_kist::slots`, the one layout, which the kernel follows; ferros_sys::kist for a kist's own code). A capsule that declares none of the implicit set, as every capsule from 2026-11-12: slots 1 to 6 display, touch, surface, ira, domain, entropy (a right of code c in slot c + 1), its asked wants from slot 7 in the order it declares them, then the GPU's three, then audio. A kist that declares nothing has 1 to 6, the GPU at 7 to 9 and audio at 10. A right added to the implicit set later goes after audio.
- **Capsules from before** declare implicit rights as wants. A capsule that declares any implicit right gets its wants in the order it declares them, as before, each implicit one handed wherever it sits, then the GPU's three, then each implicit right it did not declare (audio, for every capsule stowed before 2026-11-12). Its code finds its grants where it always did. Such a declaration is a placement: never asked, never shown, its reason read by no one.
- **The two meet at Mahere**: it declared the six in code order and then gps and udp, so its capsule now declares only gps and udp and its slots are the same (tools/mahere-kist.sh). Photon keeps declaring its implicit rights as placements (tools/photon-capsule.sh): udp is slot 4 in the photon repo (src/platform/ferros_net.rs `UDP_SLOT`, frozen into every Photon release) and the ira 6 in std/photon, where the new layout would put udp at 7 and the ira at 4; that move waits for a Photon release that reads the new slots.
- **What is shown**: the first-start question, the Deck card's permissions, the Permissions page (each right's default and who has it) and the stower's offer screen list only asked rights; a kist that asks for nothing says so and launches at once. The bridge's `kist list` says "always" for an implicit want. The kernel hands the implicit set whatever init sends for it, so an init from before (which asks for an unanswered implicit want) still launches a kist once the map has asked.
- **Deploy order**: the kernel first, since a capsule that declares only asked rights (Mahere's from now on) on an older kernel would get gps in slot 1; then init and the map; then new capsules. Old capsules keep working at every step.

## The permission list (2026-11-12)

Every right a kist can hold, in three groups: implicit (never asked), asked, and never handed.
The person's side of identity (alias, be you, people, places) is FACETS.md; this is the whole list, those included.
**Decided** marks the owner's words; everything else is **Proposed** and waits for the owner, the decisions gathered at the end (P0 to P5).

**What stays as decided:** a grant is the permission (rule 0); wants declared in the signed capsule with a reason (1); asked once, ever, per person, per kist, per right, kept in the person's space (2; owner, 2026-11-07: "1 means 1"); default off (3); no lying, a right said no to is an empty slot (5); revoke is kill and relaunch (7).
**Decided 2026-11-10:** asked at first need, not at first launch (rule 6's note above); a kist can put a question only while holding the one-shot ticket the input path hands it with a fresh gesture.

### Implicit: every kist, never asked

**Decided** (owner, 2026-11-12: "Yes on the implicit set"; the GPU 2026-11-11 and 2026-11-12: "I feel like GPU should be implicit. If I'm running it on my machine..."; "Claude memories/rights_implicit_set.md", "gpu_implicit.md"). A kist the person chose to run gets these, and asking about them would only teach people to tap yes:

| right | code | what it is |
|---|---|---|
| display | 0 | the screen, thru its window |
| touch | 1 | the person's touches in its window |
| surface | 2 | a window of its own |
| ira | 3 | its own key sealed to this device (bound to its gate identity: "A kist's ira keys are its own") |
| domain | 4 | its own storage, under the person's key and its own (VAULT-OUT.md) |
| entropy | 5 | randomness |
| audio | 8 | sound out (AUDIO.md); the microphone is asked, below |
| the GPU | kernel slots | its own block of the layer arena and a mailbox ("A tap launches", GPU-KISTS.md) |

**Proposed** with them (FACETS.md; the owner accepted the model 2026-11-12): its **alias** (a key for this kist alone, the same on every device of the person's) and the device's **language, number base and time zone**.

**What changes:** the first-launch question and the Deck's cards stop showing these rows.
Proposed (P5): capsules keep declaring the ones they use in their slot order, as today, so nothing about the capsule format or the slot order moves; the stower answers them yes when it stows, and the pages leave them out.

### Asked: once, ever, at first need

Each row: the right, the sentence the person reads (the developer's reason goes under it, as today), and exactly what a yes grants.
New codes are appended after `audio` (8) in the order approved, so the codes in every list already kept keep their meaning.

| right | the person reads (proposed) | a yes grants exactly | status |
|---|---|---|---|
| **always on** | "keep running: start when you attest and stay on behind" | init starts it at attest on every device of the person's where it is stowed, without a tap; when memory runs short the device ends kists without it first | **decided** as a permission (owner, 2026-11-12: "Run on boot should be a permission"); words and grant proposed |
| **network: its places** | "connect to: maps.example.org, tiles.example.org" (the places it declared, by name) | ip's resolver (op 10 RESOLVE, `ferros_dns`) answers this kist only for the names its capsule declares, and ip opens connections for it only to addresses it resolved for it (or declared literal addresses); any other name is "not one of its places", never a fake address | proposed |
| **network: anywhere** | "connect anywhere on the internet" | ip's client as `udp` grants it today: any address. For browsers and peer to peer (Photon's peers are wherever its contacts' devices are) | proposed: today's `udp` (code 7) keeps its code with this meaning |
| **local network** | "find and talk to devices on your local network" | multicast and broadcast, and private and link-local addresses; neither network right above reaches these | proposed |
| **sensors** | "how you hold and move the device, and the light near it" | the accelerometer, gyroscope, ambient light and proximity streams (SENSORS.md) | proposed |
| **camera** | "the cameras" | frames from the camera driver (Lumis's ops, "Lumis" below); the console shows it while open | proposed |
| **microphone** | "the microphone, while it listens" | sound in from the audio service; the console shows it while open | proposed |
| **place** | "where you were last (no radio turns on for it)" | a send right on the place service's `place` endpoint (LOCATION.md) | proposed in LOCATION.md |
| **gps** | "where you are now (turns GPS on while it looks)" | the GPS claim, as today (code 6) | built; words proposed in LOCATION.md |
| **Bluetooth** | "Bluetooth: find and connect to things near you" | the bt driver's client ops, under the identity's address | proposed |
| **NFC** | "NFC: read and write tags you hold to it" | tag reads and writes while its window is in front and the screen is on | proposed |
| **USB** | "things plugged into the USB port" | a device on the port, once the person picks it | proposed |
| **be you** | "be you in Photon: your contacts find and recognise you there" | the person's handle proof and a binding of its alias to their identity, signed by the fleet (FACETS.md "Be you") | proposed |
| **people** | "your people: all of them, or the ones you choose" | the people served by the identity service, all or as chosen (FACETS.md "People") | proposed |
| **places** | "your places: home, work and the spots you saved" | the saved places, all or as chosen (FACETS.md "Places"); not where you are | proposed |
| **the hold** | "keep things for you" (making its declared kinds); "open your save states" (one question per kind it opens) | `items` and the per-kind opens of STORE.md "Two rights" | proposed in STORE.md |

**What a kist needs to run at all** is marked so in its declaration ("Claude memories/permissions_asked_once.md": a declared right carries whether it is needed to run) and is asked when it is stowed, with Stow and Cancel.
Everything else waits for the person's first action that needs it: Photon asks for the camera the first time the person beams, for place the first time they share where they are, never before.

**No notification permission.** **Decided** (owner, 2026-11-12, NOTIFY.md): reach is the view (a kist whose circle is in the deck's resting view can notify; out of view it is silent) and the kind is the system's (live, personal, status, lowered from what the kist asks, never raised), so there is no per-kist switch to ask.

**Picks need no right.** A kist with no facet still gets one address, one person, one place now or one item when the person picks it on the system's sheet (FACETS.md "Pick, don't share"); the pick is the grant.

### Never handed

**Proposed** (the owner received it 2026-11-12, not yet item by item; FACETS.md "The never list" has each with its reason): no right grants these, so nothing can be said yes to.

- which other kists the person uses;
- their fleet: devices, keys, names, how many;
- their identity's root: the handle, the identity seed, the person key, the identity and fleet keys, the ira;
- other kists' data;
- device identifiers (chip IDs, the UFS serial, the IMEI, radio addresses);
- fine device facts (the exact battery level, the model, storage left, signal measures, the serving cell): at most a coarse word (low power, a metered link).

### Last used on the card

**Proposed** (sent to the owner 2026-11-12 with the list): each asked right on a kist's card on the Deck shows when the kist last used it, in the person's words ("using now", "an hour ago", "never").
- **Written at the moment of use, by whoever served it**: the place service at a read (as LOCATION.md has it), the GPS at a claim, ip at a connection, the identity service at a facet's answer, the camera and the microphone at an open. An event, never a clock: no process looks at a timer to write it.
- **Kept per device** in the person's class (the cards read it from there), never sent to the kist.
- "Using now" stays as the claims show it today (GPS only, `SYS_CLAIM` op 2), and grows to every right with a live open (camera, microphone, GPS).
- Implicit rights show no row, so no time.

### Decisions for the owner

- **P0. The words**, row by row: the sentence the person reads for each asked right.
- **P1. The network split**: its declared places, anywhere (today's `udp`), and the local network, three rights; the resolver enforcing the first.
- **P2. Always on's grant**: started at attest without a tap, ended last when memory runs short; whether one that ends by itself is started again (today nothing is).
- **P3. The never list**, item by item.
- **P4. Last used**: written at use by the server, kept per device; or carried in the space so every device shows the latest anywhere (that would tell each device what the person did on the others).
- **P5. Implicit rights in capsules**: still declared in slot order and answered yes at stow (proposed, nothing moves), or dropped from declarations and given at fixed slots as the GPU's are.

## Built (2026-11-08)

- **The vocabulary and the documents** (`ferros_kist`, host-tested):
  - The rights by code (`RIGHTS`: display, touch, surface, ira, domain, entropy, gps, udp, audio), with a person's words for each (`MEANS`). `audio` (2026-11-11, AUDIO.md) was appended, so every code before it keeps its meaning in the lists people already keep.
  - The capsule's declaration: a section `kist` (`name`) and a section `wants`, a field per want named by its right, its value the reason (`a`, ASCII text: vsf encodes `x` only with std). The fields' order is the order of the wants in the slots ("The implicit set": since 2026-11-12 a capsule declares only what it is asked for). Both sit before the code, so they are read from the capsule's first bytes.
  - The person's list, `kists.list` (scope `kists.`, identity class; it stands in for the common area's `kists`): a section `defaults` (a field per right, 1 or 0) and a section `kist` per kist (`capsule` hb, `developer` ke, `option` hp, `name`, and a field per want: the reason, then once answered the answer `u` and when `e`).
- **The kernel** (process.rs, service.rs):
  - `Object::Domain`, a kist's own vault domain: its keys derived under the person's key from a one-way name of the process's ihi (the oracle over its developer key and option), through the vault process only, open only while the person is present.
  - `Object::Kists`, init's right to launch. `SYS_SPAWN` kist mode (x2 bit 1) names the capsule by its address in `capsules.` and gives a right code or an empty slot (`Object::Empty`: a zero token nothing finds) per want; the kernel makes each right's object itself. The service loop streams the capsule from the vault process a transfer a call (`KistJob`) into its arc, checks its blake3 against the address, runs the gate over it in place, checks every slot against the capsule's own declaration, then builds the process. Bit 2 is a relaunch: the running process killed (one shot dead), then launched again.
  - **A launch that does not start is told** (owner, 2026-11-11: Mahere's circle tapped, "nothing opened", the console alone saying "the arc pool has no room for the kist"). Init answers "started" as soon as the launch is queued; the job fails later, if it does. The job keeps why, one record a capsule (`NOT_STARTED`, process.rs `kist_not_started`), and rings the kists' bell; the record goes when that kist next starts. The reasons (`ferros_kist::launch::why`): 0 no room in the memory pool, 1 not stowed on this device (no capsule in the vault, or a masked kist with no seal here), 2 its checks refused it (signature, gate, a right it does not declare, native code in production), 3 its capsule could not be read, 4 anything else. `SYS_NOT_STARTED` 39 (ferros_sys `not_started`), for the Settings holder alone, as the claims' users list: op 0 the records (the reason, the capsule's address, the bell count it rang), op 1 the bell's word, op 2 the kists alive (2026-11-11, "Kist states" below).
  - **A kist that stops abnormally is told the same way** (owner, 2026-11-11: Mahere started, faulted at once, "no toast or anything, just no indication at all"). Where a process ends (`end_process`), a kist (a process the kist job started, by its capsule) is kept with reason 5 FAULT (the kernel's fault word: an abort, anything not a syscall, a syscall buffer it does not own) or 6 PANIC (exit G#DEAD, ferros_sys::panic_exit and std's abort; or 101, std's main panicking); a kist ended by `kill` from outside (the bridge's `diag kill`) with 7 KILLED. Any other exit value is the kist's own and keeps nothing; the person's relaunch kills without a record.
  - Scopes `capsules.` (the device's, by content address) and `kists.` (the person's).
  - init's grants page was full at 108: the GPU service has its own list now (`gpu_grants`) and the kernel starts it at home, before init. Its five slots went to the Kists right, serve and use `launch`, and `kists.`.
- **init** serves `launch`: [0 launch, or 1 relaunch][the capsule's address] → an answer (`ferros_kist::launch`). It reads the person's list itself, never the map's word, and launches nothing while a want is unanswered.
- **The map**:
  - A kist circle per kist in the list after the settings circle (up to 15), its icon the display's bundled one for that app until a kist carries its own, refreshed at a tap on the plane.
  - A tap launches through init. If any want is unanswered, the circle first opens on the question: each unanswered want a switch set from the person's default, its reason under it, then "start", which keeps the answers and launches.
  - A launch init answered "started" that the kernel did not start (2026-11-11): the map holds the kists' bell (`SYS_NOT_STARTED` op 1) and, when it rings, looks among the records for the kist it asked last with a bell count later than the one it saw before asking. Found, that circle opens on why, once the plane is at rest and no finger is down: "Mahere could not start", the reason under it (not enough memory; not stowed on this device; it did not pass its checks; it could not be read; something went wrong), in the person's language (ferros_lang `Say::CouldNotStart`, `Msg::NotEnoughMemory` and its neighbours). Nothing to tap: a tap outside closes it, a tap on the circle again asks again. While the job is under way nothing new shows (it is quick).
  - A kist that started and then stopped (reasons 5-7): the kist asked last opens its circle the same way, "Mahere stopped", "it crashed" (a fault or a panic, the kind on the console) or "it was closed from outside" (ferros_lang `Say::Stopped`, `Msg::ItCrashed`, `Msg::ClosedFromOutside`). Any other kist that stopped (the person elsewhere meanwhile) is said once, at its circle's next tap, before any launch; the tap after launches. A stop the kernel no longer keeps (that kist started again since) is dropped unsaid. The map takes each record once, by its bell count (`kists_read`).
  - Settings pages Kists (each kist, each want a switch; the Deck and a card per thing since 2026-11-12) and Permissions (each right: the default for new kists, then every kist that wants it). A changed answer is written to the list, and a running kist is relaunched.
- **The stower, for development**:
  - `ferros-bridge capsule … --kist NAME --option HEX --want RIGHT REASON …` signs a declaration in, with the option as the capsule's `hp`, so an update keeps the kist's identity.
  - `ferros-bridge kist stow <capsule>` (since 2026-11-11 the development fallback alone, "Stowing") checks the signature, stores the capsule in parts of 4 MiB (a `store` command's part form: offset and more, by address), and adds the kist to the attested person's list. `kist answer` and `kist list` are for testing.
  - tools/mahere-kist.sh and tools/photon-kist.sh make Mahere and Photon and, since 2026-11-11, stow them with `ferros send` ("Stowing").
- **One shot dead**: the kernel's kill, exit and restart paths give a process nothing on its way out (audited 2026-11-08). Arcs are now wiped as they are freed, so the attest screen, Hive, Ramps and wintest no longer wipe or close their window before they exit (the display drops a dead process's window).
- **Measured on the Pro** (2026-11-08): Mahere's 47 MB capsule stored in 12 s, Photon's 106 MB in 23 s (the vault process's heap raised to 24 MiB on the way: rewriting a capsule grows a manifestus buffer with the vault's entries). A launch reads the capsule from the vault in 3.8 to 3.9 s (Mahere) and 6.2 to 10.3 s (Photon), a transfer of 4 KiB a call through the kernel's channel. Launched from its circle (FERROS_MAP_START=kist, no one at the panel), with the question answered at the defaults (all off), Mahere started with its gps slot empty and opened its vault in its own domain; Photon with every want off started and ended at once, and with all seven granted ran. A relaunch killed the running Photon and started it again with the answers as they stood.
- **Not yet**: the question and the pages seen by eye; a kist's own icon in the list; Mahere and Photon running at once (each takes a 256 MiB arc, the big pool is 320 MiB: the second is refused for room); reading a capsule in larger transfers than 4 KiB.
- **No teardown left in processes** (owner, 2026-11-08: "a kill/end for all processes is just like everything else, one shot dead, no cleanup"):
  - **The kernel turns off what a process left on.** Each power switch remembers the process that last switched it on (`POWER_OWNER`, by epoch); when that process ends, any way, the switch goes off on the kernel's core (`power_offs_apply`: each pass of the service loop, and in `diag restart` between the kill and the new spawn). A switch another process switched on meanwhile is that process's, and stays on. The `android` switch is a launch request, not a power, and is never turned off this way. Seen on the Pro: `diag restart touch` and `diag restart gps` each logged "power …: off, its process ended (done)" between the kill and the new process, which switched it on again and served.
  - So USB's give-up, Bluetooth's and the sensors' bring-up failures, the GPU demo, GPS's STOP and the modem's DOWN now only exit. GPS keeps its last fix whenever it moves (every change hits disk), never on the way out. The modem has no kernel power switch yet, so DOWN leaves the CP as it was (the modem's `off` mode powers it down on purpose). httptest leaves ip's link mode as it set it.
  - fluor's ferros host no longer closes its window when an app closes (the display drops a dead process's window). In the ferros std (the ferros-rust fork), main's return and `process::exit` run no `rt::cleanup` (no flush, no at-exit), and stdout holds nothing back (a LineWriter of no capacity), so an exit loses no output. Destructors of `main`'s own locals still run when `main` returns: that is the app's code, not the platform's.
- **A kist's ira keys are its own** (owner, 2026-11-08): `SYS_IRA` for a process launched as a kist derives under its own context, then binds the key to the kist's gate identity (its domain name, from its ihi), so no kist can ask for a system service's key or another kist's. System processes derive as before, byte for byte. Mahere's vault secret changed with it (its data from before is development data only); relaunched under the new kernel it opened its vault and drew, with no trap.
- **Syscall buffers resolve or trap** (owner, 2026-11-08): every buffer a syscall names is in the caller's arc, or inside a DMA region it holds (used directly: the display's heap); anything else traps at that syscall. The report (process and thread, syscall and op, offset, address and length, the nearest region, PC and LR at the svc) goes to the console and the flash log; a development build parks the thread for good with its memory intact (`diag kill` ends it), a production build ends the process. Receive and reply buffers are checked when they are named, not when a later call fills them. payloads/buftrap is its test: on the Pro it parked with `TRAP syscall 1 … G#0 bytes past its arc; pc +G#10D8 lr +G#101C`.

## Kist states (2026-11-11)

The owner judged four dummy circles on the Pro and kept their glows (2026-11-11: "glow is good! you can nix the test circles and use those colours for realzies. For the ferros settings specifically, I think 703nm (I think that's VSF red?)"). The dummies (FERROS_DUMMY_KISTS) are gone; every kist circle now glows its kist's state, and the settings circle its own.

| state | glow (ferros_glow) | means, as built (the owner may correct these) |
|---|---|---|
| running | 500 nm, thin | a live process of the kist, in front or behind (deck's tap leaves it alive behind); a widget the display draws itself (the clock) too, as it is live on the glass and never a process |
| stopped | 600 nm, thin | the kernel keeps a record for it: its last process ended by a fault, a panic or a kill from outside, or its last launch did not start (no room, not here, its checks, its capsule unreadable); dropped when it next starts |
| not running | a ring of darkness | stowed and neither: not launched since boot, or ended by its own exit, the person's relaunch or the person's Stop on its card |
| for you | 400 nm, heavy, reach by count | something waiting on the person for that kist; nothing sets it yet (no notifications exist): the map's `waiting` list by capsule takes a count when something does |
| system | 703 nm, thin | the settings circle alone (built in, never a kist): VSF's red primary, so its light has no negative channel and no multiply |

For you is checked first, then running, then stopped. A kernel without op 2 below leaves kist circles with no state (the theme's glow as before).

- **The kernel** (process.rs): `SYS_NOT_STARTED` op 2, for the Settings holder alone, x1 the Settings token, x2/x3 a buffer and its length → how many kists are alive, each a capsule's address (32 bytes, whole ones as fit) of a process the kist job started that is not ending (`kists_live`). The kists' bell (op 1) now also rings at every kist's start (`kist_started`, not only when a record went) and every kist's end (`kist_stopped` for every exit, `kill_locked` for a kill), under the kernel lock, so a reader it wakes finds the process gone or ending. Not the facts' bell: that one rings only when the count of processes alive changes, as the service loop next looks, so a relaunch (a kill and a start between two looks) or one kist ending as another process starts rings nothing.
- **ferros_sys** `not_started::live`.
- **The map** (`state`, `send_states`): at each ring of the kists' bell (`kists_rang`) it reads the records (stopped) and the kists alive (op 2), sends display op 57 for every circle whose state changed, and draws when the map is on the glass. Every `send_scene` (a fresh scene has no states) sends them all, and at start the map reads them once, so a map started again finds a running kist running.
- **The display** takes state 5 (system) on op 57; an older display draws an unknown byte as no state.

## The deck and the card (2026-11-12)

The owner, 2026-11-12, of four ways to see a kist's info (twist a circle to flip it, drag it to the centre, an info circle in the quick row, Settings → Kists where each row opens the kist's card): "just 4." Then: "make sure the settings/items page lists kists and pucks. we may add more stuff to the home screen like contacts and shit so I want to make sure it says items in settings, not kists. and I think pucks distinct from kists being that you can't open a puck and it's basically a widget."

So the settings circle's Kists page is the **Deck** (owner, 2026-11-12, of names for the page: "Deck it is!": the home screen is the deck, what is on it is on deck; what a person keeps is in the hold, as items) (ferros_lang `Msg::PageDeck`, all sixteen languages; the map's page key `deck`), and a thing's **card** is the one place its info shows: no flip, no info circle. Wherever this document says "the Kists page", it is the Deck now.

**The list.** Everything on the deck as a circle, by kind: the kists first (opened: apps), then the pucks (live circles never opened, PUCKS.md: today the clock, known by its name, `widget`), each kind a group, so contacts can come as one more. Each row is the thing's name, and under it its kind ("kist", "puck") and a kist's state as its glow says it (running, stopped, not running; nothing before the kernel has said which kists are alive). The whole row opens its card. A restore or an unstow under way shows its bar under its row. Under the list, the **pandok** (the directory, kists.fgtw.org; "available" before): each listed kist, "stowed", or "stow" or "update" with its size. A kist offered over USB stays above everything, the list or a card alike ("Stowing" below), and opens the Deck on it as it opened the Kists page.

**A kist's card** (Back above it, which returns to the list; one unstowed meanwhile, here or on another device of theirs, closes it):
- its name and kind;
- **made by**: the developer's name as their first kist here gave it (the person's pin), then always their key's first four bytes (`G#…`);
- **version**: as its signed declaration writes it, in the person's base, leading zero parts left out (`write_version`); "-" for one stowed before versions;
- **size**: its capsule's, where known: the pandok listing that very capsule, else the job that brought it while the stower still holds it; else "-" (the person's list keeps no size and the map holds no right to `capsules.`; a stower op that sizes a stowed capsule without reading it is not built);
- **identity**: its developer key and its option, each shortened to its first four bytes (what its answers and its domain are kept by, unchanged by an update);
- **state**: running, stopped (the kernel's reason under it: not enough memory, not on this device, its checks, unreadable, it crashed, closed from outside; the map now keeps each record's reason, `ended`), or not running;
- **keeps**: "in its own domain" (its vault domain). Its size is not told to the map yet; the hold (STORE.md) is later;
- **permissions**: each asked want a switch (the implicit set has none, "The implicit set" above) with the developer's reason under it, "not asked yet" while unanswered, "using now" while it holds a claim (GPS only, today). The answers are the person's, asked once, ever, for all their devices; a switch turned off takes the right back (the list written, the kist killed and started again without it: `Choice::Answer`, unchanged);
- its actions: **stop**, while it runs; **update**, when the pandok lists a newer version of the same identity (the pandok's own rule: `Version`'s order, any listed version for one stowed before versions), its bar while it runs; **unstow**, two taps ("tap again: unstow", "its answers are kept for when you stow it again"), not while a job of its own is under way.

**A puck's card**: its name and kind, **shows** (what its circle shows: the clock, the time), its permissions as above (the clock asks for none), and **remove from the deck** in two taps. A puck is never a process, so it has no state and no stop.

**Underneath:**
- **Unstow** (and a puck's remove) is the stower's op 4 as uninstall was (`Choice::Unstow`, renamed from `Uninstall`): out of the person's list, its answers kept (`kept`), its capsule dropped, the removal carried to their other devices by the identity space. Its circle goes when the job is done, and the card with it.
- **Stop** is new: the map asks init's `launch` endpoint `[OP_STOP 2][capsule]` (ferros_kist::launch). Init finds the kist in the person's list by its capsule (never the map's word) and calls `ferros_sys::stop_kist`: `SYS_SPAWN` kist mode with x2 bit 5. The kernel kills the live process of that name only if it is that kist (a kist process with that capsule; a kist named as a system process stops nothing), one shot dead (`kill_locked`), launches nothing, and keeps no record, as for the person's relaunch, so the circle goes to not running, not stopped. Init answers STOPPED (6), NOT_RUNNING (7, nothing done), or NOT_STOWED, UNREADABLE, REFUSED. The kill rings the kists' bell, so the card and the circle's glow follow with nothing asked on a timer.
- **A permission switch** is the Kists page's (`Choice::Answer`): the answer written to the person's list (`kists.list`, identity class), then init's OP_RELAUNCH, which kills a running kist and starts it with the answers as they now stand (nothing for one not running).
- **Words**: ferros_lang `PageItems`, `KindKist`, `KindPuck`, `Pandok`, `NothingStowed`, `MadeBy`, `FactVersion`, `Size`, `Identity`, `Running`, `Stopped`, `NotRunning`, `Keeps`, `OwnDomain`, `Stop`, `Update`, `Unstow`, `TapAgainUnstow`, `AnswersKept`, `RemoveFromHome`, `TapAgainRemove`, `Shows`, `TheTime`, in all sixteen languages. The pandok's "stowed", "stow" and "update" now come from them too; its other lines ("reading the list of kists" and its neighbours) and the stower's stage words are still English outside ferros_lang.

**Not yet:** run on a device; a kist's own domain size; a capsule's size for one the pandok does not list once its job is gone; contacts as a kind; the map's `widget` (a puck by its name) renamed and replaced by the signed `puck` flag (PUCKS.md).

## Built before

- Mahere as a kist: mahere's `mahere-device` (the touch-device app, shared with Android) and `mahere-ferros` (its ferros shell), tools/mahere-build.sh; spawned over the bridge it starts, opens its vault from its ira key, and draws its first frame through fluor's ferros host (2026-11-07). Its development spawn uses the `test` vault scope until kists have domains.
- fluor's ferros host takes an input hook (`run_app_with`), so an app with gestures of its own (Mahere's two fingers) reads every contact.
- Fixed with it: since the handle table took the first 512 bytes of the grants page (2026-11-03), ferros_rt's grant count, kete's store scope search, fluor's grant slots and Photon's ip slot all read the old offsets, so no std process found its entropy, its vault scope, its display or its udp grant. All four read G#200 and G#204 + 32k now.

## Lumis

Lumis (the camera app) is not a fluor app yet: a Kotlin Camera2 front with a GLES preview over JNI and ashmem, and a portable Rust core (`lumis_core`: demosaic, stacking, colour, DNG/TIFF/JXL/VSF writers) that uses raw Bayer frames only and no ISP and no video codecs. As a kist it needs: a camera driver process serving frames (today the camera runs in the kernel's diagnostics, CAMERA.md), with ops to list the cameras, open and close one, set exposure, gain, focus and window, and a ring of cacheable frame buffers it maps, each with its metadata (the exposure taken, the frame's time and count); the sensors' facts as data (black and white levels, the CFA, colour matrices, the active array, ranges, focal lengths: a per-sensor table in the vault); its UI on fluor (the GLES preview, JNI and ashmem go); captures saved to its vault domain instead of MediaStore. Missing on the ferros side: the driver process itself, a second capture without a cold boot, the front end's 50 ms service, cacheable frame memory, the pixel clock (true exposure time) and the gain-to-ISO map, and the front and tele sensors.

## Whose key signs a kist (owner, 2026-11-10)

A kist's identity is its developer key and its option, so the key is its owner. Commons projects sign with the project's key: Photon's kist with Photon's release key (the one that signs Photon's update manifest and its binaries), as ferros's capsules carry ferros's; the owner's own apps (Lumis, Mahere) with a developer key of his. "Photon tho? that's everyones ... Same category as ferros. I don't own it."

**Gap: custody moving.** Who holds a commons project's key is the real ownership question, and it may move (to the project, the DUNA, a threshold of maintainers). A kist cannot follow a new key today: its updates must carry the same developer key, so a new key is a new kist and every person's answers start over. What it needs is a hand-off signed by the old key naming the new one, carried with the capsule, so a device moves the kist and its answers to the new key. Not designed further yet.

## Trust on first stow (owner, 2026-11-10)

No list of developers is built into the OS. A kist carries its developer's whole public key bundle, and the person's devices trust it from that developer's first kist on.

- **The capsule** (ferros_kist `capsule`, feature `bundle`): a complete VSF file signed with an egg per scheme (Ed25519, Falcon-512, SPHINCS+: `fgtw::release::release_slots` and `sign_release`), its header naming the developer's Ed25519 key (`ke`). The `kist` section holds:
  - `name` (`a`) and `option` (`hR`), as before;
  - `developer` (`a`): the developer's name, when the signer gives one (kistcat takes it from the developer's public record);
  - `kb`: the developer's public bundle, one field with a value per scheme, each by its own key type: `ke` (Ed25519, 32 bytes), `kvf` (Falcon-512, 897 bytes), `kvs` (SPHINCS+, 32 bytes). Read back by type marker, never by position, into an `fgtw::pq::KeyBundle`; anything else in the field, a scheme twice or a scheme missing refuses it. It sits under the eggs like every other byte of the file.
  Then `wants`, then the code (`masked`: `code`, `text`, and `fixed`, `bti`, `exact` when set). One builder writes it (`capsule::unsigned`): the bridge (`capsule --kist`) and `kistcat kist` both go thru it. A capsule from before `kb` still reads (`bundle_of` gives None).
- **At stow** (`capsule::admit`, the one rule for payloads/kists and the bridge's `kist stow`):
  0. the bundle is the one the capsule carries, and its Ed25519 key is the kist's developer (the header's `ke`); for Photon's release key, the bundle fgtw pins (a carried one must be that one);
  1. every egg verifies against it (`fgtw::pq::verify_eggs`, the release mask);
  2. its commitment (`KeyBundle::commit`) is the one the person's list pins for that developer, or none is pinned yet.
  A capsule without `kb` is refused ("it does not carry its developer's keys"), unless its developer is Photon's release key. A bundle that differs from the pin is refused: "this kist is signed by a different key than <developer name>'s other kists", the name the one the pin keeps (else the key's first bytes, G# hex).
- **The pin** is the person's: a section `pin` per developer in `kists.list` (`developer` ke, `bundle` hp: the commitment, `name` a when their first kist gave one, `pinned` e), written with the kist in the same write of the list (`Kists::trust`). It syncs to all their devices as a `PIN` fact (ferros_space, IDENTITY-SPACE.md). An unstow keeps it, as it keeps answers. A list from before has no pins and reads as none; a restore of a kist whose developer has no pin yet pins it then.
- **Photon's release key** stays trusted as fgtw pins it, for every person; it is not pinned in the list.
- **Signing a kist:** `kistcat kist --key <developer key .vsf> --masked <image.bin> --text-len N --name NAME --option HEX64 --version 0.1.0 [--want RIGHT REASON]... [--record <public record .vsf>] --out <capsule.vsf>`. The key is the developer's secret (`kistcat devkey new`: section `developer_key` { label a, seed kge }), its bundle derived under its label, never printed or copied; the record (checked with every egg, and that it names this key) gives the developer's name. The capsule is checked as the device checks it before it is written. tools/clock-kist.sh builds the Clock masked and signs it with the owner's key this way.
- **The version** (owner, 2026-11-10) is in the capsule's signed `kist` section: one field `version`, a `z` per part, as many parts as the developer uses ("we don't limit the .'s"), kept as written. It is read by its significant parts (ferros_kist `Version`, `significant`, `write_version`): leading zero parts mean nothing ("00000123=123", 0.1.0 is 1.0, all zeros is 0); the number of parts is the stage and more parts is newer ("ver 12 to ver 1.0 ... from alpha to beta and then from 45.0 to 1.0.0 when I think it's ready to go mainstream": 12 < 1.0 < 45.0 < 1.0.0); the same number of parts compares left to right as numbers. A person sees exactly what is compared, in their number base, leading zero parts left out. The person's list keeps each kist's stowed version (`version`, from the capsule, carried by the identity space), and the Kists page offers an update only when the listed version is newer; the stower does not itself refuse an older one (it never did).
- **Not yet:** a developer moving to a new key (the custody gap above): today a new bundle under the same Ed25519 key is refused, and a new Ed25519 key is a new kist. Two devices that each pinned a different bundle for one developer before either synced: the space's merge keeps the later write, and the other device then refuses that developer's kists signed the first way.

## The directory (owner, 2026-11-10)

A public, searchable list of kists, on ferros.org and on the device. "Passless" both ways: no account to stow, no approval to list. Metrics come later, many independent dimensions, never a single star score; no fees, no ads.

- **It vouches for nothing.** Anyone may list a kist. The device checks the capsule's blake3 against its entry and the capsule's developer signature (every egg, against the bundle it carries and the person's pin) at stow. The directory itself is to be signed by the ferros project key with every egg, only so that nothing between the bucket and the device can change it unseen; until that key is chosen it is published UNSIGNED (`kistcat build --unsigned`, `directory::build_open`: no signer in its header), the device reads it unchecked and its Kists page says so.
- **Where:** the R2 bucket `kists`, served at http://kists.fgtw.org/ over plain HTTP (the device has no TLS and fetches only exact paths). `/kists.vsf` is the directory; `/capsules/<blake3>.vsf` a capsule, kept by its own hash; `/index.html` the page. The bare address is served by the Worker `kists` (tools/kistcat/worker: `/` alone is routed to it, it returns index.html thru an R2 binding); every other path is R2's. ferros.org shows the same list in web/index.html, between the `kists: begin` and `kists: end` markers.
- **The document** (`ferros_kist::directory`, feature `directory`, no_std + alloc, the same code kistcat writes with and the device's stower will read with): a complete VSF document, its header stamped with Eagle time, naming the project key (`ke`) with one egg slot per scheme (`gm`: Ed25519, Falcon-512, SPHINCS+). Per kist a section `kist`:
  - `name` (`a`), `developer` (`ke`), `developer_name` (`a`, when the capsule's declaration names one: `kistcat kist --record` writes the name from the developer's public record), `option` (`hR`): together with the developer the kist's identity;
  - `version` (`z`, one value per part, as many parts as the developer uses: the capsule's own signed declaration's, as written; a manifest entry maps its artefact's `major`, `minor`, `patch` into three parts). Until 2026-11-10 an entry had `major`, `minor` and `patch`; no reader is kept for them;
  - where the capsule is: `path` (`a`, an exact path on kists.fgtw.org), or for a developer's own manifest `manifest` (`nu`, a plain-http URL), `platform` and `arch` (`a`): the device fetches the manifest, checks the developer signed it with every egg, and takes the artefact with that platform and arch;
  - `capsule` (`hb`, the capsule's blake3; for a manifest entry, what the artefact must hash to, or the directory is older than the manifest and nothing stows until it is rebuilt) and `size` (`u`, bytes);
  - `about` (`a`, one line), `icon` (`a`, a path to a VSF icon, optional).
  And per kist a section `metrics` naming it by `developer` and `option` (so its measures carry across versions), with no dimension yet.
- **Text is ASCII** (`a`), not `x`: vsf decodes `x` (Huffman) only with std and its tables, and the stower is no_std, as the person's list and the declaration already have it.
- **API:** `build_unsigned(kists, made, signer)`, `build_open(kists, made)` (UNSIGNED: no signer, provenance only, while no project key is chosen), `parse_directory(doc)` (its `signer` None for an unsigned one; shape only: every field present and of its type, no kist listed twice, measures only for listed kists, unknown sections skipped), `verify_directory(doc, bundle)` (`fgtw::release::verify_signed_by` against the project key's public bundle, the bridge's rule for a kist, then the parse).
- **tools/kistcat:** `kistcat build --key <secret .key.vsf> --out <dir> [--web web/index.html] ENTRY...` reads each local capsule (every egg verified as the device verifies it, `capsule::admit`: against the bundle it carries, or Photon's release bundle; its identity, name, developer's name, version, blake3 and size from the capsule itself) and each manifest (fetched over plain HTTP at build time, verified with every egg against its signer's pinned bundle, its ferros/arm64 artefact read), signs the directory with every egg of the project key (its VSF secret read thru `ferros_kist::secret`, section `developer_key` { label, seed kge }, the one reader the bridge and ferros-mkimg use; fgtw `SigningBundle::developer(seed, label)`, the ferros key's label FERROS_PROJECT; `release_slots`, `sign_release`: the bridge's machinery for a kist), checks it with `verify_directory`, writes kists.vsf, index.html and the ferros.org list, and prints the wrangler commands. An entry that cannot be listed is skipped with a warning (a manifest without the artefact, a capsule signed before every-egg kists); a capsule file that is missing stops the build. `kistcat build --unsigned` (in place of `--key`) writes it with no signature, said so on stderr, on the page ("The list is not signed yet") and by `kistcat verify <kists.vsf>`, which reads it unchecked; with `--key <secret .key.vsf>` it checks a signed directory as the device does. A raw key file (the form before 2026-11-10) is refused with the conversion, `kistcat devkey convert --raw <old> --label LABEL --secret <new .key.vsf>`, which writes the VSF form to a new file and leaves the old one. tools/kistcat/kists.sh is the source list.
- **The page** (kists.fgtw.org): the list (name, developer, version, what it is), a search box (a few lines of inline script, the only script), the numbers (kists, developers, last updated), and where kists run: ferros today; Linux, Android and macOS thru a host app once kists carry toka, not yet. ferros.org's copy runs no script.

**Open (2026-11-10):**
- Which key signs the directory, and the name its post-quantum halves are derived under (`FERROS_PROJECT` for now, part of the key once published). Once chosen, its public bundle is pinned (as `fgtw::release` pins the release key's) so the device and `kistcat verify` need no secret; today `verify` derives the bundle from the key file.
- Listable now (2026-11-10): the Clock, masked and signed by the owner's developer key (tools/clock-kist.sh writes target/kists/clock.capsule.vsf; tools/kistcat/kists.sh lists it, unsigned). Mahere's capsule is native and signed the old way (one Ed25519 egg, no `kb`): it lists once it is built masked and signed with `kistcat kist`. Photon's release manifest has no ferros/arm64 artefact yet.
- Photon's manifest writes `platform` and `arch` as `x`, which a no_std stower cannot decode, and its artefacts' URLs are https, which the device cannot fetch: the ferros artefact needs a plain-http URL, and either `a` text there or the stower reads manifests with std.
- Whether the Worker route takes `/` ahead of the bucket's custom domain: check after the first deploy that `/` is the page and `/kists.vsf` still comes from R2.
- Icons are VSF images, which a browser cannot show: the page lists without them.

## The stower (built 2026-11-10, not yet on a device)

Owner, 2026-11-10: "complete the on device compile and seal", with a progress bar and the stages said; then "We do need to do it correctly so I'd rather take our time and ship the first kist proper." payloads/kists stows, updates, unstows and restores; the map's Kists page asks it (`use kists`, the map's slot 8, init's 108). The kernel starts it once a person has attested, with its own grants (service.rs `kists_grants`: use udp, `capsules.`, `kists.`, serve kists, use fgtw, the Stow right), as it starts GPS: init's grants page is full. `tools/husky-shell.sh kists` stores and restarts it.

- **Stow is SANDBOX.md's lifecycle, nothing skipped**, each stage shown on the Kists page with a bar and a line:
  0. *Fetching*: plain HTTP GET (ip's client TCP, the IPv4 address from ip's resolver, op 10) straight into the vault, a transfer (256 KiB) a piece under the capsule's blake3 (`ferros_sys::store_set_addr_at`); never held whole; its address and its signed file hash computed as it passes (ferros_kist::fetch `Intake`). A status other than 200 is "not where it is kept", its body never read.
  1. *Checking its signature*: its blake3 and size the directory's, its identity (developer, option) the entry's, and every egg against the developer's pinned bundle (`fgtw::pq::verify_eggs`, the release mask). The last piece is held back until then, so nothing unchecked is ever committed; any failure drops the bytes.
  2. *Checking its code*: `SYS_SPAWN` stow mode (x2 bit 3, the `Stow` right) has the kernel read the capsule back from the vault into an arc (the bar again), gate it where it lies (`gate_located`), read its masked modes in place (`oracle::masked_modes`) and run the sandbox checker over its instruction span, once. **Native code is refused** ("this kist is a development build and can't be stowed from the network").
  3. *Sealing it to this device*: `oracle::seal_parts` (byte for byte `seal`'s), kept in the vault's `sealed` list.
  4. *Storing it*: into the person's list with its `source` (the URL) and `installed` (Eagle time, when it was stowed; the stored field keeps its name from before the word was stow, so lists written then keep their times); an update drops the old capsule.
  The kernel tells each of its stages on a word in the stower's arc and wakes it (`stow_tell`); nothing is polled.
- **Launch checks the seal only** (SANDBOX.md step 3): `spawn_kist` now launches masked kists, their arc masked, W^X and BTI as sealed, refusing one whose seal is not in `sealed`; native kists (the bridge's `kist stow`, the development door, unchanged) launch as before, in development builds only. A fixed-mode kist is refused at launch for now (its arc must be exactly the sealed 2^K; a staged arc holds the whole capsule).
- **Unstow** (two taps): out of the person's list, its answers kept (a section `kept`, `Kists::remove`; stowed again, nothing is asked), its capsule dropped; the space carries the removal to their other devices, and a kist's fact is stamped with its stow time so a stow after an unstow wins over the tombstone. Its seal stays in `sealed` (harmless: a seal names one exact image).
- **Restore**: the stower holds the identity service's doorbell (fgtw op 21); when the space's generation moves (or its first pull settles) it lists `capsules.` and fetches each capsule the person's list names that this device lacks: the fleet's devices first (not built: `fleet_fetch` is the seam, a `capsule_want` over Photon Transport beside `space_want`), then the kist's recorded `source`, then the directory by blake3, each through the same stages.
- **Keys** (payloads/kists): `PROJECT_BUNDLE`, the ferros project key's public bundle (until it is chosen the directory is published unsigned and read unchecked, and the page says "the list's signature is not checked yet"). No developer is pinned in the OS: a kist carries its developer's bundle and the person's list pins it at their first kist ("Trust on first stow"); Photon's release key stays trusted as fgtw pins it. A refused kist names why on the page: no bundle (6), not its developer's keys (18), another key than the pinned one (17, with the developer's name, carried in the status after the jobs).
- **Not yet**: a manifest entry (Photon's: the developer's manifest read on the device); the fleet-first fetch; compiling (toka) as distinct from checking masked native code; ip has no held receive, so the HTTP client asks ip again after a millisecond when nothing came (net.rs, the one timer), and ip's 4 KiB receive window bounds a download to about one window per round trip.

## Stowing (2026-11-11)

The owner, 2026-11-11: "how do we install kists over USB? Can we make a standard method so the end user doesn't have to get them from the [catalog]", then "Just seems to me that we should just make a proper checked path and start using it", and "the proper term for installing a kist is stow".

**One stower, every source.** Every kist reaches a person's list thru payloads/kists and the same checks, whatever carried it:
- **the directory** (kists.fgtw.org, the Kists page's "available"), built 2026-11-10 ("The stower" above);
- **a USB offer** (`ferros send <capsule>`, tools/ferros-send), built here;
- next, **a photon** (a kist sent by a friend in Photon, carried into `capsules.` by Photon and offered the same way) and **device to device** (the fleet's `capsule_want`, the restore's `fleet_fetch` seam): neither built; each only a new way to put the bytes in `capsules.` and offer them.

The checks are the directory's, nothing skipped: every egg against the developer bundle the capsule carries and the person's pin for that developer (`capsule::admit`; trust on first stow; a bundle other than the pinned one refused); the kist's identity (developer and option) from its signed declaration, so an update is the same identity and keeps the person's answers and its domain, and only its developer's key can sign one; its version as its declaration writes it (stages, `Version`; the stower refuses no older one, as before); then the kernel's stow mode (the gate where it lies, the checker over its masked code, the seal).

**The flow over USB:**
0. `ferros send` reads the capsule's name and version from its first bytes and finds the device (the first on USB, or `FERROS_BUS`).
1. It carries the capsule in, 4 MiB a part (`ferros.kist.offer` Write; USB.md "Offering a kist"), into `capsules.` under its blake3: refused unless a person is present (attested: the identity half of the vault open) and no other offer is waiting.
2. It offers it (Exec). The kernel keeps the offer (one at a time) and rings the stower's offers' bell (`SYS_OFFER`).
3. The stower takes it as a job: the capsule read back from the vault and hashed as it passes (its blake3 must be the address offered), then the checks above, each stage on the Kists page with its bar (`ferros_sys::stow_check` for the kernel's part).
4. Checked, it waits on the person (`STAGE_ASKING`). The map opens the Kists page on it (not over an app in front, nor under a finger: once the person is back on the map), above everything: its name and version, who made it ("by" the developer's name the capsule carries, else its key's first bytes), its size, "unchecked code, development build only" for native code, each right it asks for with the developer's reason ("asks for these once, at its first start"), then **Stow** and **Cancel**. No icon yet: a capsule carries none (the circles' icons are the display's own).
5. Stow puts it in the person's list as a directory stow does (its developer pinned at their first kist; `source` empty: carried in, nothing to fetch it again from; a restore finds it in the fleet or the directory by its blake3), and its circle appears. Cancel, or a refusal, drops its bytes (unless the person's list already names that capsule).
6. Each state goes to the kernel, which answers the host's read held until it changes: `ferros send` prints `stowed`, `refused: <why>` or `cancelled` (exit 0, 1, 2).

**The drag-to-place picker** (permissions_asked_once: tap or drag the icon to place it, then the rights it needs to run at all with Add or Cancel) is not built; the Kists page carries the confirmation until it is, and the wants are asked at first start as for a directory stow.

**Native code.** Production refuses native capsules (SANDBOX.md). Mahere, Photon and Genesis are native today (std apps do not build masked: SANDBOX.md "Masked std apps" scopes it), so a stow takes native code on a development build only, unchecked and unsealed (the kernel sees it in the first bytes and reads no further: `stow::NATIVE`), the offer saying so on the glass and on the host; a production build refuses it ("it is unchecked native code, which a production build does not stow").

**Our tools stow too.** tools/mahere-kist.sh, tools/photon-kist.sh and tools/clock-kist.sh build and sign as before, then `ferros send`. `ferros-bridge kist stow` stays as the development fallback alone (no check by the device's stower, no Stow on the glass, thru the development `store` capability): unattended loops with nobody at the screen need it; nothing else calls it.

**Words.** "Stow" on the glass (ferros_lang `Msg::Stow`, `Stowed`, `NotStowed`, `OfferedOverUsb`, `UncheckedDevOnly`, `AsksAtFirstStart`, `Say::ByDeveloper`, in all sixteen languages); "stowed" on the host.

**The rename (2026-11-12).** What still said "install" says stow: the kernel's `SYS_SPAWN` stow mode, `KistStow`, `stow_request`, `stow_tell`, `STOW_*`; ferros_sys `stow_kist`, `stow::*`; the stower (payloads/kists keeps its process name `kists`) with `KIND_STOW`, `KIND_UNSTOW` and `stow()`; the map's `stower.rs` (was installer.rs); `Kists::stow` and a kist's `stowed` time; the `Stow` right and `Object::Stow` (its code is still 22, so a grant made before means the same); `NOT_STOWED`; the bridge's `kist stow` (`kist install` still taken, and said to be the old name). The Deck page's stage words are ferros_lang's now, in all sixteen languages (`WaitingItsTurn`, `CheckingSignature`, `CheckingCode`, `SealingToDevice`, `StoringIt`, `Unstowed`, `NotUnstowed`, `Restored`, `NotRestored`, `Say::Fetching` and `FetchingOf`), with `NoKistsStowed` and `NotStowedHere` renamed and reworded; the reasons for a refusal are still English (ferros_kist `offer::words`). Kept: the person's list's stored field `installed` (a kist's time, so lists written before keep theirs), the owner's words where they are quoted, and "install" where it means something else (the kernel's `ferros.dev.install` to the UFS, the bridge's `seal` of a masked system capsule, `ferros-install`, software on the dev box).

**The first run (the Pro, 2026-11-12).** Mahere's 51 MiB capsule went in whole, was offered and checked (native code, taken unchecked) within 1.5 s, and then waited on the person for 15 minutes with nothing on the glass. Mahere had just crashed, so the map most likely had its circle open on "it crashed" (`show_fault`; the console lines that would say were read away before anyone looked); the offer's `show_page("kists")` set the page but left that circle open, which drew its own page, and the offer then counted as shown and never opened again. `show_page` now goes to the settings circle from an open kist circle (the way back to the map kept) and puts the page picker away: the Fleet page's departures had the same gap. A second `ferros send` killed the first mid held read and met its reply in the IN endpoint; the session request fixes that (USB.md "Offering a kist"). An offer still asking stops any other (status 2) until the person answers it on the Kists page, or leaves.

**To the end on the 8a (2026-11-12),** Stow and every tap injected (`ferros-bridge input`), the kists the Pro gets next (`tools/pro-kists.sh`, the same capsule files):
- **Mahere** (0.6.1, 51 MiB, 13 parts of 4 MiB at about half a second each, checked in 2.5 s): stowed; its first start asked its eight rights once; it drew on the GPU once akita's layer arena was whole (BOARDS.md "Memory map"), loaded cells over WiFi thru the resolver, and grew its arc to 674 MiB of its 692 MiB ceiling; pinch, full screen on and off (deck's swipe) and Home worked.
- **Photon** (0.151.1, 87 MiB, 22 parts): stowed, started (its rights asked once), reached its first screen; the card's Stop ended it.
- **The clock** (masked, 13 KiB): checked and sealed by the stower, stowed, its circle shows the time; its card is a puck's ("shows the time", asks for nothing).
- **The Deck's cards:** made by (the key's first bytes, the pin's name when it has one), version, size, identity, state, the rights as switches, Stop while it runs, Unstow (two taps) and "remove from the deck" for a puck.
- **Found on the way:** while `ferros send` waits for the person it holds the USB link, so no other bridge command (an injected tap) reaches the device: end it, tap, and the offer stays. tools/mahere-kist.sh signed with the bridge's default key (the ferros project key), so the 8a held Mahere under three keys (that one, the exposed August key, and now the owner's): three kists, each with its own answers, until two were unstowed; it signs with the owner's developer key and carries Mahere's version now. A capsule without a version shows "-" on its card. A part the vault did not keep reached the host as "not sent: " and no words; `ferros send` names the status now. The vault process (24 MiB of heap) died taking Photon's 16th part, and then at every open (its high water is 43 MiB on the 8a now), which stopped the GPU, WiFi and sensors processes with it, and the person's list of kists read back empty afterwards (the next stow wrote a new one); its heap is 56 MiB since, a stopgap: the open's working set grows with the vault's entries, and bounding it is manifestus's.

**Not yet:** the stower's reasons for a refusal in ferros_lang (ferros_kist `offer::words`, English still); an offer's bytes left in `capsules.` (named by no list, inert) when the device restarts mid-offer; `ferros send` on macOS and Windows (written portable, built on Linux only).
