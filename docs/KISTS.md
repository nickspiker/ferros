# Kists on the map

Status: designed 2026-11-07; permissions decided and the first cut built 2026-11-08 ("Permissions" and "Built" below). The owner: "should be nice round circles on the home screen for each kist." A kist is an installed app as this device runs it (GLOSSARY.md: the developer's signed capsule after the device checked, compiled and sealed it); its circle on the map is its kist circle (SHELL.md). The base OS bakes in one circle, settings (no_baked_apps): every other circle is a kist the person installed, and the map knows nothing of it until then.

## What exists to build on

- **The apps.** Photon (`std/photon`, tools/photon-build.sh) and Mahere (mahere's `mahere-ferros`, tools/mahere-build.sh) build for `aarch64-unknown-ferros` as the real apps (STD-TARGET.md) and run when spawned over the bridge. Lumis does not yet (below).
- **Spawning from the vault.** `SYS_SPAWN` starts a signed capsule stored in the vault by its key, handing the child copies of the caller's own grants (ferros_sys `spawn`, `spawn_unique`); the signature gate checks it. init's keepers use it for every driver.
- **A surface per app.** `--dma surface` (and the map passing its `surfaces` region at spawn) gives the child one full-screen surface of its own (WINDOWS.md "Built"); fluor's ferros host draws there and asks the display to show it (op 22).
- **The open circle as an app's region** (COMPOSITOR.md, 2026-11-07): the open circle's inside is the app's, its clean window made once.
- **Domains in the vault** (VAULT-OUT.md, decided 2026-11-07): each kist's data is its own domain, keyed by the kernel from the kist's developer key and option; the person's things across kists are the common area; a permission is asked once per person, per kist, per permission.

## The installed list

- **Where:** the person's common area (VAULT-OUT.md), under the name `kists`, identity class: it is the person's (which apps they have and where their circles sit), never readable before attest, and it travels with them to their other devices as the arrangement does. The capsules themselves are device data (a capsule is the developer's, public), each kept in the vault under its own content address (blake3 of the signed capsule), so two people on one device who installed the same app share one copy.
- **Its form:** one VSF document, a section `kist` per installed kist, every field typed and named (no positions, no text carrying structure):
  - `capsule` (`hb`): the capsule's blake3, the vault key of its bytes;
  - `developer` (`ke`): the developer's Ed25519 key as the gate saw it, and `option` (`u`), together the kist's identity for its domain and its permissions;
  - `name` (`d`): the app's own name, for settings and the console (circles show icons, not names, SHELL.md);
  - `icon` (a `v` image: VSF RGB gamma 2 codes cut to a circle, as tools/app-icons makes them), so the map decodes nothing;
  - `wants`: a nested section per right the kist asks for (`display`, `touch`, `surface`, `udp`, `gps`, `entropy`, `ira`, a common-area name …), each with the person's answer (`granted`, a typed bool, and when, Eagle time `e`);
  - `share` and `ring` (`u`): its circle's size and place, as the map's arrangement keeps them today (`map.` scope) for the settings circle.
- **Install** (the installer is a kist of its own, or settings' page, never the map): the capsule arrives (over PT, a Photon transfer, a download), the gate checks its signature, the device compiles and seals it (KERNEL.md "Install, compile, seal, go"), the vault keeps it by its blake3, and the installer appends a `kist` section to the person's `kists`, asking the person for each want once. A capsule of 47 MB (Mahere's development image) or 100 MB (Photon's) is past the kernel's 3 MiB store path; the vault process takes values in pieces (VAULT-OUT.md), and the install goes through it.

## The map lists them

At attest the map reads `kists` (it already holds the person's arrangement) and adds a kist circle per section after the settings circle, its icon inside, sized and placed by `share` and `ring`: the arrangement's placement fit (payloads/map) takes them as it takes settings. Nothing about a kist is compiled into the map; `CIRCLES` stays the settings circle alone, and the rest are data.

## A tap launches

- The map holds the display, touch and the surfaces region, not the rights a kist may have been granted (gps, udp, its vault domain), and it must never hold them: a map that can hand out the GPS is a map that can read it. So the map does not spawn a kist; it asks **init** (which holds every grant and already starts processes by vault key) over a `launch` call naming the kist by its `capsule` hash.
- init reads the person's `kists`, finds the section, checks the capsule's hash and the person's answers, and calls `SYS_SPAWN` with exactly the granted rights, in the kist's documented slot order (Mahere: display, touch, surface, ira, its vault domain, entropy, then gps if granted; Photon: display, touch, surface, udp, its domain, ira, entropy), plus a fresh surface. One process per kist (`spawn_unique` by the kist's name): a second tap brings the running one forward.
- The window opens in the kist circle: the circle's flight to open (COMPOSITOR.md's flight, op 45/46) lands on the kist's surface as the open circle's region, the app drawing its frame inside the circle and the bars staying the map's. Closing (the circle's own gesture) flies back; the process keeps running until the person ends it or the device needs its memory (death notices, WINDOWS.md order of work).

## What each piece needs

1. **Vault process:** values in pieces for capsules (in progress, VAULT-OUT.md); the common area's `kists` name with its grants (settings and the installer write, the map and init read).
2. **Kernel:** a kist's vault domain at spawn (VAULT-OUT.md's domain keys) in place of today's fixed `STORES` list (`map`, `gps`, …, `test`); `SYS_SPAWN` streaming a large capsule from the vault into its arc as `bridge spawn` streams one over PT (the kernel's 4 MiB staging slot); badges and death notices (WINDOWS.md).
3. **init:** the `launch` endpoint above.
4. **map:** kist circles from `kists`, and the tap's `launch` call (another session owns payloads/map and payloads/display today; nothing here touches them).
5. **Installer:** the install flow above.

## Permissions (owner, 2026-11-08)

0. **A grant is the permission.** A kist can do only what it is handed at spawn: no permission-check calls, no ambient authority.
1. **Wants are declared in the signed capsule**, each a named right with one line of the developer's reason. Nothing undeclared can be granted: the kernel reads the declaration from the capsule it launches and refuses any right not declared in that slot.
2. **Asked once, ever, per person, per kist, per permission.** The kist is its gate identity (developer key and option), not its name; the answers are the person's (identity class), so every device of theirs knows them.
3. **Default off** for every want. An update that adds a want asks that want (off until answered); the other answers stand.
4. **The person's defaults**: a default per right (the Permissions page) sets where the first-launch question's switch starts. Out of the box every one is off.
5. **No lying**: a right said no to is an empty slot, never a fake, and a kist runs without it (Mahere without GPS; Lumis without the camera has no cameras to pick).
6. **Kists can never ask.** No call makes a prompt. The system asks once, at first launch: every unanswered want on one screen, each a switch.
7. **Revoke = kill and respawn** without the right. Every kill and every end, of any process, is one shot dead: no end hooks, no signals, no graceful shutdown, no teardown code; the kernel reclaims by dropping (and wipes the arc as it does).
8. **Settings pages**: per kist (Kists), per right (Permissions: who has the GPS, who has the network), and the defaults, in the system settings circle.
9. **Kist circles** on the map, round, one per kist; a tap asks init to launch it (init holds every grant; the map holds none of the kist's), the first launch showing the question first.

Later, not now: a system picker that hands a kist just the file the person chose (no permission needed: the choice is the grant); the console showing live use of a right.

## Built (2026-11-08)

- **The vocabulary and the documents** (`ferros_kist`, host-tested):
  - The rights by code (`RIGHTS`: display, touch, surface, ira, domain, entropy, gps, udp), with a person's words for each (`MEANS`).
  - The capsule's declaration: a section `kist` (`name`) and a section `wants`, a field per want named by its right, its value the reason (`a`, ASCII text: vsf encodes `x` only with std). The fields' order is the slot order. Both sit before the code, so they are read from the capsule's first bytes.
  - The person's list, `kists.list` (scope `kists.`, identity class; it stands in for the common area's `kists`): a section `defaults` (a field per right, 1 or 0) and a section `kist` per kist (`capsule` hb, `developer` ke, `option` hp, `name`, and a field per want: the reason, then once answered the answer `u` and when `e`).
- **The kernel** (process.rs, service.rs):
  - `Object::Domain`, a kist's own vault domain: its keys derived under the person's key from a one-way name of the process's ihi (the oracle over its developer key and option), through the vault process only, open only while the person is present.
  - `Object::Kists`, init's right to launch. `SYS_SPAWN` kist mode (x2 bit 1) names the capsule by its address in `capsules.` and gives a right code or an empty slot (`Object::Empty`: a zero token nothing finds) per want; the kernel makes each right's object itself. The service loop streams the capsule from the vault process a transfer a call (`KistJob`) into its arc, checks its blake3 against the address, runs the gate over it in place, checks every slot against the capsule's own declaration, then builds the process. Bit 2 is a relaunch: the running process killed (one shot dead), then launched again.
  - Scopes `capsules.` (the device's, by content address) and `kists.` (the person's).
  - init's grants page was full at 108: the GPU service has its own list now (`gpu_grants`) and the kernel starts it at home, before init. Its five slots went to the Kists right, serve and use `launch`, and `kists.`.
- **init** serves `launch`: [0 launch, or 1 relaunch][the capsule's address] → an answer (`ferros_kist::launch`). It reads the person's list itself, never the map's word, and launches nothing while a want is unanswered.
- **The map**:
  - A kist circle per kist in the list after the settings circle (up to 15), its icon the display's bundled one for that app until a kist carries its own, refreshed at a tap on the plane.
  - A tap launches through init. If any want is unanswered, the circle first opens on the question: each unanswered want a switch set from the person's default, its reason under it, then "start", which keeps the answers and launches.
  - Settings pages Kists (each kist, each want a switch) and Permissions (each right: the default for new kists, then every kist that wants it). A changed answer is written to the list, and a running kist is relaunched.
- **The installer, for development**:
  - `ferros-bridge capsule … --kist NAME --option HEX --want RIGHT REASON …` signs a declaration in, with the option as the capsule's `hp`, so an update keeps the kist's identity.
  - `ferros-bridge kist install <capsule>` checks the signature, stores the capsule in parts of 4 MiB (a `store` command's part form: offset and more, by address), and adds the kist to the attested person's list. `kist answer` and `kist list` are for testing.
  - tools/mahere-kist.sh and tools/photon-kist.sh make and install Mahere and Photon.
- **One shot dead**: the kernel's kill, exit and restart paths give a process nothing on its way out (audited 2026-11-08). Arcs are now wiped as they are freed, so the attest screen, Hive, Ramps and wintest no longer wipe or close their window before they exit (the display drops a dead process's window).
- **Measured on the Pro** (2026-11-08): Mahere's 47 MB capsule stored in 12 s, Photon's 106 MB in 23 s (the vault process's heap raised to 24 MiB on the way: rewriting a capsule grows a manifestus buffer with the vault's entries). A launch reads the capsule from the vault in 3.8 to 3.9 s (Mahere) and 6.2 to 10.3 s (Photon), a transfer of 4 KiB a call through the kernel's channel. Launched from its circle (FERROS_MAP_START=kist, no one at the panel), with the question answered at the defaults (all off), Mahere started with its gps slot empty and opened its vault in its own domain; Photon with every want off started and ended at once, and with all seven granted ran. A relaunch killed the running Photon and started it again with the answers as they stood.
- **Not yet**: the question and the pages seen by eye; a kist's own icon in the list; Mahere and Photon running at once (each takes a 256 MiB arc, the big pool is 320 MiB: the second is refused for room); reading a capsule in larger transfers than 4 KiB.
- **Teardown still in processes** (found by the 2026-11-08 audit, left for the owner: each powers hardware or ends a session, and moving it is a decision):
  - payloads/usb `give_up` switches the controller's power off before it exits; payloads/bt and payloads/sensors switch theirs off when bring-up fails; payloads/gpu's one-shot demo resets the GPU before it exits. A kernel rule (a power switch turned off when the last process holding it ends) would take all of these.
  - payloads/gps op 1 STOP keeps the last fix, powers the chip down and exits; payloads/modem's rmnet op 5 DOWN powers the CP down and exits. The kernel's radio switch already kills without asking (and powers Bluetooth, NFC and GPS down itself); the last fix would then need keeping when it is taken.
  - payloads/httptest puts ip's link mode back before it exits.
  - Outside this repo: fluor's ferros host sends op 23 when an app closes (Photon, fluordemo), and Rust's std runs main's destructors and flushes stdout at its return (ferros_rt `rt_main`).

## Built before

- Mahere as a kist: mahere's `mahere-device` (the touch-device app, shared with Android) and `mahere-ferros` (its ferros shell), tools/mahere-build.sh; spawned over the bridge it starts, opens its vault from its ira key, and draws its first frame through fluor's ferros host (2026-11-07). Its development spawn uses the `test` vault scope until kists have domains.
- fluor's ferros host takes an input hook (`run_app_with`), so an app with gestures of its own (Mahere's two fingers) reads every contact.
- Fixed with it: since the handle table took the first 512 bytes of the grants page (2026-11-03), ferros_rt's grant count, kete's store scope search, fluor's grant slots and Photon's ip slot all read the old offsets, so no std process found its entropy, its vault scope, its display or its udp grant. All four read G#200 and G#204 + 32k now.

## Lumis

Lumis (the camera app) is not a fluor app yet: a Kotlin Camera2 front with a GLES preview over JNI and ashmem, and a portable Rust core (`lumis_core`: demosaic, stacking, colour, DNG/TIFF/JXL/VSF writers) that uses raw Bayer frames only and no ISP and no video codecs. As a kist it needs: a camera driver process serving frames (today the camera runs in the kernel's diagnostics, CAMERA.md), with ops to list the cameras, open and close one, set exposure, gain, focus and window, and a ring of cacheable frame buffers it maps, each with its metadata (the exposure taken, the frame's time and count); the sensors' facts as data (black and white levels, the CFA, colour matrices, the active array, ranges, focal lengths: a per-sensor table in the vault); its UI on fluor (the GLES preview, JNI and ashmem go); captures saved to its vault domain instead of MediaStore. Missing on the ferros side: the driver process itself, a second capture without a cold boot, the front end's 50 ms service, cacheable frame memory, the pixel clock (true exposure time) and the gain-to-ISO map, and the front and tele sensors.
