---
name: pipe-session-2026-05-27-findings
description: PIPE comms diagnostic findings from 2026-05-26→27 session. CDR is solid; the two remaining root-cause bugs are SM1 startup race + slot_phase parity drift.
metadata: 
  node_type: memory
  type: project
  originSessionId: 8a47d2cf-0516-4c75-ab36-e6a43da45e08
---

After this session's commits (last: 8918025), the PIPE bridge has:

- **Narrow-window-multi-N CDR** (128450b, Nick's idea): locks in 2-3 edges, drift -300 to -1000 ppm (was +94000 ppm). Anchors to seed_period_cyc, accepts ±2% of integer multiples N=1..8, slews live interval_fp_period for real drift tracking. The CDR foundation is now solid.
- **Slot timing re-anchor** (426fab1): resets cumulative_cyc_since_anchor on every accepted period sample, bounding worst-case slot-phase drift between samples to ≤8 slots.
- **slot_wire_log diagnostic** (8918025): captures raw wire state at the FIRST 256 slot boundaries crossed after mirror_started, surfaces as bit vector with host-side comparison against expected DATA / IDLE Claim-12 wire patterns for the given (bridge_tx × FPGA_DEADBEEF) combo.
- **FPGA TX_PATTERN** = 0xDEADBEEF flashed to FPGA SPI flash (persistent across resets).

**Why:** the symmetric-AAAAAAAA test pattern was hiding decode quality completely; switching to DEADBEEF + the slot_wire_log finally gives ground-truth diagnostic data. Earlier "decode = 0x55555555" results turned out to be the IDLE-pattern artefact, indistinguishable from any rotation of AAAAAAAA-DATA-mode.

**How to apply:** when working on PIPE decode bugs, run `cargo run -p pipe-host --release --bin bridge -- wire_mirror 16777216 13 9 55555555` and look at the WIRE LOG section. Three outcomes:
- matches DATA pattern (>90%) → handshake fired, slot tracker aligned, decode should work
- matches IDLE pattern (>90%) → handshake never fired, FPGA stayed in IDLE the whole test
- matches neither → slot tracker drifted, OR FPGA bitstream isn't what we think

**Two ROOT-CAUSE bugs remain, both visible in the wire log:**

1. **SM1 startup race** — Run 1 shows `01010101` (bridge silent from slot 1), Run 5 shows `10011001` (bridge correctly driving 55555555 → IDLE-pattern, since handshake separately doesn't fire). Same code, same FPGA, different runs. SM1 init non-deterministic.

2. **slot_phase parity drift** — wire log can show `01010101...01001010 11011010 10101010` where the alternating pattern FLIPS PARITY mid-run. Slot tracker's slots_crossed math occasionally off by 1 (e.g., when edge timing lands near slot_cyc/2 boundary), inverting slot_phase forever. Once parity is inverted, all decode is wrong.

Both these are SEPARATE from the FPGA-side handshake reliability question (which is also a problem: even when bridge drives correct 55555555 pattern → wire shows IDLE-shape, suggesting FPGA missed the bridge-=1 mid-slot sample → no data_mode).

**Next session targets:** (a) SM1 PC + FDEBUG TXSTALL + TXFLEVEL instrumentation — but READ THE REGISTERS FROM CORE 0 after fast_sync returns, NOT inside fast_sync's interrupt::free block. Reads from core 1 in that context crashed the firmware this session (= "load_dev no pong" then bridge wedges). (b) FPGA handshake debug — maybe the mid-host-slot sample timing is off due to clock skew between bridge's SM1 ticks and FPGA's enclave_clk.

Related: [[pipe-load-dev-is-ram-hotload]] (load_dev = SRAM, the "no pong" verify is a host race UNLESS firmware actually crashes; if subsequent `info` calls hang, the firmware IS broken — BOOTSEL to recover). [[pipe-comms-acceptance-test]] (acceptance = 1 minute BLAKE3-clean full-duplex — long way to go but the foundation finally exists).
