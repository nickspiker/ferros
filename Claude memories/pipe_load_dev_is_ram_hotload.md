---
name: pipe-load-dev-is-ram-hotload
description: "pipe-host `load_dev` streams the dev image into SRAM at 0x20000000 via VSF — zero flash wear. Don't fall back to BOOTSEL+UF2 just because the ping verify fails."
metadata: 
  node_type: memory
  type: reference
  originSessionId: 8a47d2cf-0516-4c75-ab36-e6a43da45e08
---

`cargo run -p pipe-host --release --bin bridge -- load_dev <elf-or-bin>` is the RAM hotload for the RP2040 bridge firmware. It uses a tiny flash-resident "flash manager" image that:
1. Receives the dev image over USB via VSF framing
2. Streams up to 176 KiB of raw bytes into SRAM at `DEV_IMAGE_DEST = 0x2000_0000`
3. BLAKE3-verifies the SRAM range, jumps to it

The dev firmware is linked against `memory-ram.x` so its `FLASH` origin IS SRAM (0x20000000). **No bytes hit flash during iteration.** See [tools/pipe-bridge-rp2040/src/main.rs:95-110](tools/pipe-bridge-rp2040/src/main.rs#L95-L110) for the in-firmware comments that spell this out.

The host-side `load_dev` ends with a "verifying with ping" step that ALMOST ALWAYS fails on first try with `error: ping after load_dev: never got pong`. **This MAY BE a host-side race** (sometimes the stream succeeded and the bridge is running the new firmware; the host's ping just timed out). Confirm the new firmware is running with `info` (NOT `wire_mirror` — that takes 8 s and may hang) — if `info` returns up:N with a small N, the load worked; if `info` also hangs, the firmware is genuinely wedged → BOOTSEL.

**CRITICAL: `load_dev` requires the SRAM-linked build.** The default `cargo build --release` in `tools/pipe-bridge-rp2040/` produces a FLASH-linked ELF (uses `memory-flash.x`, vector_table targets 0x10000000). Streaming that into SRAM at 0x20000000 and jumping to it WILL wedge the chip — vector_table entries point to flash addresses but the code is in SRAM. You MUST build with `--features ram-image` for load_dev. Confirmed in 2026-05-27 PIPE session: every load_dev with the default flash build wedged the bridge, requiring BOOTSEL recovery. The "no pong" memory above is RIGHT when the SRAM build is used; it's a host race. With the wrong build, the wedge IS real.

For `load_dev` to be safe and fast:
```
cd tools/pipe-bridge-rp2040 && cargo build --release --features ram-image
cd ../.. && cargo run -p pipe-host --release --bin bridge -- load_dev tools/pipe-bridge-rp2040/target/thumbv6m-none-eabi/release/pipe-bridge-rp2040
```

**Don't fall back to BOOTSEL+UF2 just because of the pong failure** — that's the only path that actually wears flash, and a 100k-cycle QSPI rated for years of normal use can be burned through in a single bad iteration session. (Confirmed wasted ~6-7 flash writes in the 2026-05-26 PIPE session before catching this.) Related: [[measure-before-fixing]] — same root cause, building "fixes" on top of false premises about the system.
