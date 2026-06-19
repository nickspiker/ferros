# BRIDGE — host-side link to a running ferros kernel

`ferros-bridge` is `adb`, `fastboot`, and a few things neither of them have, rolled into one tool and carried over Photon Transport instead of vendor USB protocols.
It is the only way a dev machine talks to a live ferros kernel: probe it, read its boot log, peek its MMIO, pull its persistent rings, hot-reload a new kernel into RAM, or persist a signed kernel to storage — all over a single bulk USB pipe.

It is the host-side counterpart to the device-side DWC3 stack in `ferros_hal` (Pixel 8) and `ferros_hal_m1` (M1).
The kernel exposes capabilities; the bridge addresses them.

## Identity

The device enumerates as **VID G#1209 / PID G#4665** (pid.codes, "ferros").
The PID was requested via pid.codes PR #1208 (2026-05-13) and is **pending merge** — once merged, any updated Linux host identifies ferros by name in `lsusb` from the canonical USB ID database.
Until then the value is reserved-in-spirit by the open PR.

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
| `terminal` | `adb shell` | Interactive bidirectional PT session |
| `echo` / `send` / `ping` | `adb shell ping` (loosely) | Link/PT test harness (no real adb/fastboot equivalent) |

The difference that matters: adb and fastboot are two separate stacks with two separate transports and two separate device modes.
ferros-bridge is one tool, one transport (PT), one device that never has to drop into a special "bootloader mode" to accept a new kernel — `reload` works against the running kernel.

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

## Transport contract

ferros-bridge speaks **Photon Transport (PT)** — it does not invent its own framing.
It reuses PT layers L2–L4; see the PT stack docs for the full model.

Two primitives carry every command:

- **`pt_send` (host → device, blast mode):** send SPEC, wait for SPEC ACK (seq=MAX), then blast all DATA packets back-to-back with **A#1 ms pacing** between packets (no per-packet ACK — USB guarantees delivery), then wait for COMPLETE. COMPLETE carries the transfer's BLAKE3 so the host can confirm end-to-end integrity. COMPLETE timeout scales with size: ~A#32768 ms base + A#64 ms per packet.
- **`pt_recv` (device → host, blast mode):** read SPEC, allocate, then receive the DATA blast silently. The **SPEC ACK is skipped** — the kernel auto-blasts via its idle-poll pump and an OUT ACK could deadlock against a busy ep2. Completion is detected by `all_received()` or a FIN packet; per-chunk BLAKE3 is verified on `finish()`.

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

Setup (Fedora dev host), per [CLAUDE.md](CLAUDE.md):
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

## Known limitations
- Polling only — no GIC interrupts on the device side; the bridge paces sends to match.
- Single-TRB device model — no TRB rings yet; throughput is pacing-bound, not bandwidth-bound.
- No stall recovery — a wedged endpoint needs a power cycle.
- Mirrors the device-side caveats in [CLAUDE.md](CLAUDE.md) Phase 4 "Known limitations" — keep the two lists in sync.
