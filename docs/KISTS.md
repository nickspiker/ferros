# Kists on the map

Status: a design, 2026-11-07, nothing of it built but the pieces named under "Built". The owner: "should be nice round circles on the home screen for each kist." A kist is an installed app as this device runs it (GLOSSARY.md: the developer's signed capsule after the device checked, compiled and sealed it); its circle on the map is its kist circle (SHELL.md). The base OS bakes in one circle, settings (no_baked_apps): every other circle is a kist the person installed, and the map knows nothing of it until then.

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

## Built

- Mahere as a kist: mahere's `mahere-device` (the touch-device app, shared with Android) and `mahere-ferros` (its ferros shell), tools/mahere-build.sh; spawned over the bridge it starts, opens its vault from its ira key, and draws its first frame through fluor's ferros host (2026-11-07). Its development spawn uses the `test` vault scope until kists have domains.
- fluor's ferros host takes an input hook (`run_app_with`), so an app with gestures of its own (Mahere's two fingers) reads every contact.
- Fixed with it: since the handle table took the first 512 bytes of the grants page (2026-11-03), ferros_rt's grant count, kete's store scope search, fluor's grant slots and Photon's ip slot all read the old offsets, so no std process found its entropy, its vault scope, its display or its udp grant. All four read G#200 and G#204 + 32k now.

## Lumis

Lumis (the camera app) is not a fluor app yet: a Kotlin Camera2 front with a GLES preview over JNI and ashmem, and a portable Rust core (`lumis_core`: demosaic, stacking, colour, DNG/TIFF/JXL/VSF writers) that uses raw Bayer frames only and no ISP and no video codecs. As a kist it needs: a camera driver process serving frames (today the camera runs in the kernel's diagnostics, CAMERA.md), with ops to list the cameras, open and close one, set exposure, gain, focus and window, and a ring of cacheable frame buffers it maps, each with its metadata (the exposure taken, the frame's time and count); the sensors' facts as data (black and white levels, the CFA, colour matrices, the active array, ranges, focal lengths: a per-sensor table in the vault); its UI on fluor (the GLES preview, JNI and ashmem go); captures saved to its vault domain instead of MediaStore. Missing on the ferros side: the driver process itself, a second capture without a cold boot, the front end's 50 ms service, cacheable frame memory, the pixel clock (true exposure time) and the gain-to-ISO map, and the front and tele sensors.
