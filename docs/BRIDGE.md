# BRIDGE — host-side link to a running ferros kernel

`ferros-bridge` is `adb`, `fastboot`, and a few things neither of them have, rolled into one tool and carried over Photon Transport instead of vendor USB protocols.
It is the only way a dev machine talks to a live ferros kernel: probe it, read its boot log, peek its MMIO, pull its persistent rings, hot-reload a new kernel into RAM, or persist a signed kernel to storage — all over a single bulk USB pipe.

It is the host-side counterpart to the device-side DWC3 stack in `ferros_hal` (Pixel 8) and `ferros_hal_m1` (M1).
The kernel exposes capabilities; the bridge addresses them.

## Identity

The device enumerates as **VID G#1209 / PID G#4665** (pid.codes, "ferros").
The PID was requested via pid.codes PR #1208 (2026-05-13) and **merged 2026-09-28**, so any Linux host with an updated USB ID database identifies ferros by name in `lsusb`.
The ruling set pid.codes precedent: open-source software on closed third-party hardware (a Pixel phone) qualifies.

Every ferros-shipped USB device uses the **same** PID.
Roles and protocols are not multiplexed at the PID level — the VSF document's "PIPE message" section disambiguates them inside the transport.
See [PIPE.md](PIPE.md) and the Photon Transport stack docs for the layering rationale.

## How it relates to adb / fastboot

| ferros-bridge | Classic analog | What it does |
|---|---|---|
| `status` | `adb devices` / `fastboot devices` | Probe USB; confirm a ferros device is enumerated |
| `diag` | `adb logcat` | Pull boot diagnostics via the DIAG capability |
| `log` | `adb logcat -f` | Stream bulk-IN bytes from the device to stdout |
| `read <addr> [len]` | `fastboot oem` peek / `devmem` | Read MMIO registers via the MEM capability |
| `reboot [fastboot]` | `adb reboot` / `adb reboot bootloader` | Reboot normal, or into the bootloader |
| `reload <kernel>` | `fastboot boot` | RAM-boot a new kernel — no storage write |
| `install <kernel.signed>` | `fastboot flash boot` | Persist a signed kernel to UFS + stem entry |
| `beam <ring>` | `adb pull` | Pull persistent ring / ledger / vault snapshots |
| `run <payload.bin>` | *(no analog)* | Hot-load a program blob, CALL it on the device, print its output |
| `terminal` | `adb shell` | Interactive bidirectional PT session |
| `echo` / `send` / `ping` | `adb shell ping` (loosely) | Link/PT test harness (no real adb/fastboot equivalent) |
| `dbg` / `dbg2` | *(no analog)* | Live kernel counters / raw DWC3 event ring over EP0 — work even when the bulk path is wedged |
| `input <gesture> …` | `adb shell input` | Fingers and keys in shown frames, as a VSF script: tap, hold, swipe, drag, pinch, rotate, key, idle, shot, script (development builds; docs/INPUT-INJECT.md) |

The difference that matters: adb and fastboot are two separate stacks with two separate transports and two separate device modes.
ferros-bridge is one tool, one transport (PT), one device that never has to drop into a special "bootloader mode" to accept a new kernel — `reload` works against the running kernel.

## How verbs map to the wire

The wire protocol is the spec; bridge verbs are CLI sugar over it.
Every verb that touches device state rides a cap-addressed command — `[cap: 32][op: 1][params...]` where cap is a BLAKE3 credential from the registry in [ferros_pt/src/command.rs](ferros_pt/src/command.rs). That registry is the source of truth for what a device can be asked to do.

| Verb | Dev cap | Op(s) |
|---|---|---|
| `diag` | `ferros.dev.diag` | Read |
| `read` | `ferros.dev.mem` | Read |
| `reboot` | `ferros.dev.reboot` | Exec |
| `reload` | `ferros.dev.reload` | Write, then Exec (one-way jump) |
| `run` | `ferros.dev.run` | Write, then Exec (call-and-return) |
| `beam` | `ferros.dev.beam` | Read |
| `install` | store path | Write |

`ferros.dev.store` and `ferros.dev.usb` are registered but have no verb yet.
Everything else (`status`, `ping`, `echo`, `send`, `log`, `terminal`, `dbg`, `dbg2`) lives below the cap layer: transport plumbing and EP0 vendor-request debug that address the link, not device objects.
Dev caps are unauthenticated BLAKE3 hashes of well-known names; production delegates real credentials over the identical wire format.

## Commands

Authoritative list is the dispatch in [tools/ferros-bridge/src/main.rs](tools/ferros-bridge/src/main.rs).
All numeric I/O follows project convention: hex is `G#`, decimal is `A#`.

### status
```
ferros-bridge status
```
Opens the USB link and reports whether a ferros device is connected and ready.
Pure enumeration probe — no PT traffic. Exits non-zero if no device.

### diag
```
ferros-bridge diag
```
Sends `DIAG Read`, waits for COMPLETE, then receives the boot-diagnostic payload via PT and writes it to stdout.
This is the first thing to run after a boot to confirm the kernel reached its USB event loop.

### deaths
```
ferros-bridge deaths [n] [--blocks k] [--elf <name>=<path>]... [--dir <dir>]... [--kernel <elf>] [--exits] [--raw] [--from <file.vsf>]
```
The newest n deaths (16 by default; panics and faults, plain exits too with `--exits`) in the flash log's newest k blocks (2^10 by default, about ten thousand entries; the ring is 2^14), from `diag "log deaths n k"`: each process's end, panic or fault, and the kernel's own panic found at the next boot (KERNEL.md "A process that dies", LEDGER.md "Deaths").
Each shows how it ended, the message and `file:line:column`, the thread, ESR and FAR, the exit value and the last console line, then every frame named from the ELF: the function, its file and line, the calls inlined into it.
- The ELF is found by the process's name under the repository (`FERROS_REPO`, else the working directory): `target/el0/aarch64-unknown-none/release/<name>`, `target/masked/<name>/<name>.elf`, `std/*/target/aarch64-unknown-ferros/release/<name>`, lowercased and as `puck_<name>` too (the kist `Compass` is `puck_compass`), or as given by `--elf` and `--dir`. The kernel's own frames use `--kernel` (default `target/aarch64-unknown-none/release/ferros_kernel`).
- A frame is an offset into the arc; the image starts at the death's `code` (G#1000) and payloads and std programs link at 0, so the ELF's address is the frame less G#1000, and a return address is named at its call (4 bytes before).
- A masked capsule's ELF is the masked build's, linked after the masking: its addresses are the arc's, and its line tables come thru ferros-mask's assembly.
- Nothing on the device names the build that ran yet: the ELF used and its time are printed, and one built after the death is flagged.
- `--raw` writes the answer's VSF documents; `--from` reads a saved one instead of the device.

### read
```
ferros-bridge read <hex-addr> [len]
```
Reads `len` bytes (default A#4) of MMIO at `<hex-addr>` via `MEM Read`.
Params are `[addr:8 BE][len:4 BE]`. Output is a hex dump, 16 bytes per row, addressed from `<hex-addr>`.
Leading `0x` on the address is tolerated on input but never emitted.

### log
```
ferros-bridge log
```
Continuously streams raw bulk-IN data to stdout until USB error or Ctrl-C.
Unlike `diag` (one-shot, PT-framed) this is a dumb live tap on the IN endpoint.

### reboot
```
ferros-bridge reboot [normal|fastboot]
```
Sends `REBOOT Exec` with a mode byte (`normal`=G#00, `fastboot`/`bootloader`=G#01).
The device reboots immediately, so no COMPLETE comes back — a USB disconnect mid-send is the expected success signal, reported as "Device is rebooting".

### reload
```
ferros-bridge reload <kernel>
```
Hot-reloads a kernel into RAM and jumps to it. No storage write, no signature check — dev-loop fast path.

- Accepts an ELF (runs `llvm-objcopy`/`rust-objcopy -O binary` internally to flatten it) or a raw `.bin`/`.img`.
- Validates the MZ magic `G#91005A4D` before sending.
- Step 1: `RELOAD Write` blasts the binary; device replies with the staged byte count.
- Step 2: `RELOAD Exec` triggers the jump. The new kernel boots; the link drops, which is success.

### install
```
ferros-bridge install <kernel.signed>
```
Persists a **signed** kernel to UFS and writes a signed stem (boot) entry. This is the durable counterpart to `reload`.

The `.signed` file carries a A#108-byte trailer: `[size:4 LE][hash:32][sig:64]["FERROSIG"]`.
The bridge verifies the FERROSIG magic and the local BLAKE3 before sending anything.

- Step 1: stage the kernel via `RELOAD Write` (reuses the proven staging path); device returns staged size.
- Step 2: `INSTALL Exec` with params `[size:4 LE][hash:32][sig:64]` (A#100 bytes). The kernel re-verifies the signature on-device, writes to UFS, and updates the stem entry.
- Returns `OK` on success or `ERR:...`. The on-device verify is the authority — local verify only catches a corrupt file early.

### beam
```
ferros-bridge beam <ring> [~N | gen:N | all] [count]
```
Pulls raw blocks out of a persistent ring (the `adb pull` of ferros). Rings: `kernel-ring`, `vault-root`, `ledger`, `state`.

- `~N` — N entries back from the latest (default: latest)
- `gen:N` — absolute generation number
- `all` — every entry
- bare number — entry count (default A#1)

Params are `[ring_id:1][mode:1][offset:4 BE][count:4 BE]`; sent as `BEAM Read`, response streamed to stdout (reported in 4096-byte blocks).

### terminal
```
ferros-bridge terminal
```
Interactive session: each line typed is sent as a PT transfer, COMPLETE reported per line. Ctrl-C / EOF to exit.

### echo / send / ping (test harness)
```
ferros-bridge echo          # send a 256-byte ramp, verify COMPLETE BLAKE3 matches (Layer-2 check)
ferros-bridge send <hex>    # send arbitrary hex bytes via one PT transfer
ferros-bridge ping [count]  # raw USB ping-pong, no PT (default A#256 round-trips)
```
Diagnostics for bringing up the link, not part of the operator workflow.

`ping` is the lowest-level probe: it sends a 4-byte `PING` on bulk OUT and reads bulk IN back, `count` times (each round-trip bounded by a A#1024 ms recv timeout), then reports `OK/total` and round-trip latency (avg/min/max).
Use it to isolate the **USB link itself** from the PT layer — if `ping` flows but `echo` doesn't, the fault is in PT framing, not the wire.

### run
```
ferros-bridge run <payload.bin> [hex-input]
```
Hot-load a program: stages the blob over PT (write-verified, into the same staging slot `reload` uses — last Write wins), then the kernel **calls** it and ships back its output.
Payload ABI: `extern "C" fn(in_ptr, in_len, out_ptr, out_cap) -> u64`, entry at blob offset 0, PC-relative code only, `no_std`.
The optional hex argument becomes the payload's input; the response is `[ret: 8 LE][out bytes]`, printed as UTF-8 when clean, hexdump otherwise.
Template payload: [payloads/hello/](payloads/hello/) — build with the aarch64-unknown-none target, objcopy to a flat `.bin`.
Unlike `reload`, the kernel survives — `run` is the experiment primitive: poke registers, time something, dump state, all without a kernel rebuild.
A payload that never returns hangs the kernel (no preemption exists); recover with a hard reboot.

### dbg / dbg2
```
ferros-bridge dbg     # PT dispatch counters: per-stage SPEC/ACK/DATA/COMPLETE progress + driver state
ferros-bridge dbg2    # raw DWC3 event ring (last 12 events, decoded) + ep2 TRB snapshots + last DEPCMD failure
```
EP0 vendor-request readouts (`G#5A` / `G#5B`) — they ride the control endpoint, so they work **even when the bulk path is wedged**.
`dbg` answers "which PT stage did my command die at"; `dbg2` answers "what did the DWC3 hardware actually see".
These found the ENDTRANSFER bug in one boot; reach for them before adding any kernel-side printf.

### kist stow (development fallback)
```
ferros-bridge kist stow target/mahere.capsule.vsf
```
Kists are stowed with `ferros send <capsule>` (tools/ferros-send; KISTS.md "Stowing", USB.md "Offering a kist"): the device's stower checks the capsule and the person says Stow on its screen.
`kist stow` skips both, thru the development `store` capability, for unattended loops with nobody at the screen; a production build answers neither.
Every script stows with `ferros send` (owner, 2026-11-11: "a proper checked path and start using it").

## Transport contract

ferros-bridge speaks **Photon Transport (PT)** — it does not invent its own framing.
It reuses PT layers L2–L4; see the PT stack docs for the full model.

Two primitives carry every command, and both live in tools/ferros-link (with the link itself, `UsbLink`), the library ferros-send shares; the bridge keeps only its development vendor requests on EP0 (`dbg`, `dbg2`, the gate) in its own usb.rs:

- **`pt_send` (host → device, blast mode):** send SPEC, wait for SPEC ACK (seq=MAX), then blast all DATA packets back-to-back with **A#1 ms pacing** between packets (no per-packet ACK — USB guarantees delivery), then wait for COMPLETE. COMPLETE carries the transfer's BLAKE3 so the host can confirm end-to-end integrity. COMPLETE timeout scales with size: ~A#32768 ms base + A#64 ms per packet.
- **`pt_recv` (device → host, blast mode):** read SPEC, allocate, then receive the DATA blast silently. The **SPEC ACK is skipped** — the kernel auto-blasts via its idle-poll pump and an OUT ACK could deadlock against a busy ep2. Completion is detected by `all_received()` or a FIN packet; per-chunk BLAKE3 is verified on `finish()`.

- **A session first (2026-11-12):** `UsbLink::open` sends the vendor request `G#5D` (bmRequestType `G#40`, no data) before anything else, and the USB driver (payloads/usb) drops whatever an earlier host process left half done: a command half sent (the OUT run it was filling, ended by its own resource index), a reply half read (the IN transfer ended), a command still with the kernel (its reply dropped when it comes, the new SPEC held until then). A driver from before stalls the request, which the host passes over. While it waits for a SPEC ACK the host also passes over a packet of an old reply the controller had already loaded. The claim on the interface is exclusive, so a session never cuts a live one. Before it, a `ferros send` killed in a held read left its reply in the IN endpoint, the next session read it as its SPEC ACK, and the port had to be kicked; one killed mid-DATA left the OUT run live and the next SPEC went into it.

Direction asymmetry is deliberate and matches the "host asks, enclave answers" model: the bridge is always the initiator.
Every command is host→device first (the request), and only then device→host (the response).

### USB quirks the bridge handles for you
- **512-byte padding** on send — DWC3 short-packet workaround (see `usb.rs`).
- **objcopy fallback** — `reload` tries `llvm-objcopy`, then `rust-objcopy`.
- **Disconnect-as-success** — `reboot`/`reload` treat a recv/transfer error after the trigger as the device doing what it was told.

## Session clock

At startup the bridge calibrates a session clock once via `nunc` (eagle_time), then advances it locally from a monotonic `Instant`.
If clock calibration fails the bridge exits — it will not run with an uncalibrated clock.

## Running it

Setup (Fedora dev host), per [CLAUDE.md](../CLAUDE.md):
- udev rule grants `dialout` access and auto-loads the needed modules.
- The device must be booted and past USB init (run `status`, then `diag`).

Typical dev loop:
```bash
# Build + flatten (M1 example)
cargo build -p ferros_kernel --target aarch64-unknown-none --release --no-default-features --features m1
aarch64-linux-gnu-objcopy -O binary \
    target/aarch64-unknown-none/release/ferros_kernel \
    target/aarch64-unknown-none/release/ferros_kernel.bin

# Confirm the device, read the boot log
cargo run -p ferros-bridge -- status      # → VID G#1209 PID G#4665
cargo run -p ferros-bridge -- diag

# Hot-reload (RAM, no persist) — the inner-loop command
cargo run -p ferros-bridge -- reload target/aarch64-unknown-none/release/ferros_kernel

# Persist a signed kernel
cargo run -p ferros-bridge -- install ferros_kernel.signed
```

## Over the fleet (designed 2026-11-17, not built)

Owner, 2026-11-17: "I'm sure our bridge and commands would be easy to make thru Photons fleet identity network, thus LTE/WAN, WiFi/LAN rather than strictly thru a USB cable. Prolly can't fastboot over cellular tho..."
And: "I'm sure we'll add profiles so no bridge over WAN for specific devices, like we keep the bridge in Photon disabled until you manually enable it."

The bridge stays the bridge: the tool that connects a dev box to a device, whatever carries it.
USB becomes one link among several.

### The same commands, any link
- The commands do not change: the cap-addressed `[cap][op][params]` of [ferros_pt/src/command.rs](../ferros_pt/src/command.rs), each one a complete VSF document, as are the replies.
- Off the cable they ride the fleet's own channel: the FGTW peer network Photon already uses (LAN beacon, direct UDP, WAN), device to device first, fgtw.org never in the path ([PHOTON.md](PHOTON.md), Claude memories/feedback_fgtw_devices_first.md).
- The channel's encrypted frames stay lean (counter, ciphertext, tag); VSF is for the assembled command and reply (Claude memories/feedback_vsf_not_for_framing.md).
- The bridge picks the link: `FERROS_PORT` for USB as today, a fleet device name for the rest; a command means the same on either.

### Who may send
- **A device of the same fleet.** The sender is a member of the target's membership chain and signs each command with its device key; the target checks the signature against the chain it holds. A dev box takes part the way any device does: it runs Photon and is bound into the owner's fleet.
- **Holding the bridge right on that target.** Being in the fleet is not enough. The right is granted on the target, per sending device, by the person present and attested there, and taken back the same way.
- **Off until enabled by hand.** No device answers the bridge over the fleet, and no Photon sends it, until a person turns it on. The Photon app's side starts disabled, as the owner keeps it today.
- **Only while the target is attested.** Before attest, or locked, the device holds no fleet key and is in no fleet, so the fleet bridge is closed; USB answers as it does today.
- **Every command is signed and numbered.** A replayed or reordered command is refused. The unauthenticated development caps stay a USB-only development convenience; nothing reaches them over the fleet.
- **Every command received over the fleet goes in the ledger** ([LEDGER.md](LEDGER.md)): which device sent it, which cap and op, when, and the answer.

### Profiles
The right carries a profile: which links and which caps that sender may use on that target.
- Links: USB only, LAN (the same network segment), or anywhere (WAN, cellular).
- Caps: from read only (`diag`, `beam`, `read`) up to the ones that change the device (`store`, restart, `reload`, `install`, `reboot`).
- A device can refuse the bridge over WAN to everyone, whatever any right says. That is a setting of the device itself, so a lost or borrowed sender cannot widen it.
- Profiles are settings in spec VSF, set from the identity and flowing to the device only (Claude memories/settings_identity_to_device_only.md).

### A reload that does not come back
A failed reload over the fleet cuts its own connection, with nobody at a cable to recover.
- **`reload` (RAM) over the fleet arms a return.** The new kernel must rejoin the fleet and receive the sender's confirm before a deadline. If it doesn't, the device reboots into the kernel installed on storage (the stem's), which is known to boot. This deadline is a real one, so a timer is right here (Claude memories/feedback_no_polling.md).
- **`install` over the fleet keeps the prior kernel.** The new stem entry is committed only after the new kernel has booted and been confirmed the same way; until then the next boot takes the prior entry.
- The confirm is the sender's signed command, not the device's own report, so a kernel that boots but cannot reach the network still falls back.

### What stays on the cable
- fastboot and everything else in the bootloader: flashing partitions, slots, unlocking. These are the bootloader's own USB protocol, and nothing of ferros runs yet.
- A device that never reaches the network after a fallback.
- `dbg` / `dbg2`: they read the USB controller itself over EP0.
- The development caps (`run`, `kist stow` thru the store, the unauthenticated dev set).

### Open
- B0. Whether a profile may ever allow `reload` and `install` over WAN, or LAN at most.
- B1. The return deadline after a fleet reload, and the confirm's form (one signed command, or the sender's next ordinary command).
- B2. Whether the March 2026 USB session design (Claude memories/project_usb_auth_design.md: Ed25519 challenge, X25519 session) is replaced by the same signed commands on USB, so there is one rule for every link.
  Once it is built, `diag log` needs the bridge right: until then whoever holds the cable reads the flash log opened (LEDGER.md "Reading it"), and the log's seal covers only a chip taken off the board.
- B3. Where the right is granted: the target's Fleet page with the sender listed, or an NFC tap between the two.

## Known limitations
- Polling only — no GIC interrupts on the device side; the bridge paces sends to match.
- Single-TRB device model — no TRB rings yet; throughput is pacing-bound, not bandwidth-bound.
- No stall recovery — a wedged endpoint needs a power cycle.
- Mirrors the device-side caveats in [CLAUDE.md](../CLAUDE.md) Phase 4 "Known limitations" — keep the two lists in sync.
