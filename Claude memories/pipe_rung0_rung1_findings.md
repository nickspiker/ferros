---
name: pipe-rung0-rung1-findings
description: Rung 0 (slot_phase parity fix) and Rung 1 (256-bit deterministic verify) shipped. Best result 255/256 bits and 109 consecutive bits clean. Slot tracker still drifts unpredictably 32-130 bits in.
metadata: 
  node_type: memory
  type: project
  originSessionId: 8a47d2cf-0516-4c75-ab36-e6a43da45e08
---

Session 2026-05-27 shipped two layered bridge fixes + the FPGA 256-bit pattern + host bit-exact verify. Commits: `9de02ee` (Rung 0+1 core), `8d3d3bd` (longest-perfect-run metric + diagnostic counters).

**Status**: Claims 7 + 12 demonstrated at hardware level. Bit-exact protocol decode confirmed for runs of 100+ consecutive bits. Reliable 256/256 is NOT achieved — slot tracker drifts unpredictably after 32-130 enclave slots.

## What works (shipped at HEAD)

- **Rung 0**: per-edge `pos_in_slot = own_cyc % slot_cyc` (was cumulative). Slot_phase parity inversion gone. TX=0 → decode = G#00000000 (was G#FFFFFFFF the whole run).
- **Rung 1 FPGA**: 256-bit TX_PATTERN = "The quick brown fox jumps over t" (LSB-first per byte). Flashed to FPGA SPI.
- **Rung 1 bridge**: `fpga_decoded_log: [u8; 32]` + count captures first 256 decoded bits. fpga_tx_pos extended to mod-256.
- **Rung 1 host**: 256-rotation match search, per-32-bit-window breakdown, hex dump, longest-perfect-run metric.
- **slots_crossed per-edge fix**: same per-edge math for slot tracker. SLOTS_CROSSED_MAX=32 clamp prevents runaway loops on huge own_cyc spurs at startup.
- **100% lock rate** when preceded by ~4 s IDLE-clear (TX=0 reset). Without IDLE-clear: every-other-run fails to lock because FPGA's data_mode persists 4096 silent host slots (~3 s) across runs.

## Best results observed

- Single run: **255/256 bits clean (99.6%) at rotation +25** — patent demo passing. Not reproducible in 30+ subsequent runs (max 85%).
- Longest contiguous clean run: **109 bits at slot 0** (rotation +252) — strong protocol-works evidence.
- Reliably observe ≥32 consecutive clean bits in ~30-40% of locked runs.

## What didn't work

- **Narrow parity_ambiguous filter** (own_cyc within ±slot_cyc/16 of N+0.5×slot_cyc): 1500-2500 hits/run (5-10% of edges) but NO net improvement. Most slot_phase parity flips originate elsewhere.
- **Absolute-time slot_phase re-sync** (slot_phase = total_cyc_since_start / current_slot_cyc % 2 at re-anchor): MUCH worse. 300-1300 disagreements per run. Per-edge tracker is right; absolute-time derivation breaks because CDR slews slot_cyc mid-run.
- **Per-32-bit window match diagnostic**: shipped, useful. Shows: bad runs (47-55%) have all-low windows = FPGA stayed in IDLE the whole test; mid-tier runs (60-85%) have early perfect windows then degrade to chance.

## Open root causes

1. **SM1 startup race**: FPGA handshake doesn't fire reliably with sparse trigger (TX=00000001). Needs TX=FFFFFFFF for reliable data_mode entry. Bridge drives 1 for 32 host slots before pattern repeats — FPGA samples a 1 within that window usually.
2. **Mid-run slot_phase parity drift**: real edges occasionally land at `own_cyc ≈ N+0.5*slot_cyc` (PIO timing noise + RC asymmetry interaction). slots_crossed rounds to N or N+1 randomly. Parity flips persist (no self-correction).
3. **FPGA data_mode flap**: silent_slot_cnt reaches 4096 if bridge has occasional silent slots → data_mode drops, decode goes to all-0s mid-stream.

## Next-session high-leverage path

**Capture falling edges via second PIO state machine** (2× reference points per period). Currently bridge captures BOTH polarities but only treats rising edges as anchor points for CDR. With both polarities anchoring slot tracker, every wire transition is a slot-boundary observation — eliminates the (N+0.5)*slot_cyc ambiguity entirely. Major PIO + bridge state-machine refactor.

Alternative: post-hoc parity-flip-correction in host (search for "parity flipped at slot K" interpretations of decoded log, find max-match). Simpler but doesn't prove real-time decode works.

## Run commands

```bash
# Always preceded by IDLE-clear for reliable lock:
bridge wire_mirror 16777216 13 9 00000000  # forces FPGA back to IDLE
sleep 4                                     # wait for silent_slot_cnt to saturate
bridge wire_mirror 16777216 13 9 FFFFFFFF  # the actual test
```

For a single 100+ bit clean run, expect to run wire_mirror 5-10 times. The protocol IS working; the slot tracker just doesn't hold for the full 256-slot window every time.

Related: [[pipe-load-dev-is-ram-hotload]] (load_dev REQUIRES --features ram-image build, else wedges bridge), [[pipe-clean-clock-test]] (TX=0 isolation test, now passing with Rung 0 fix), [[pipe-session-2026-05-27-findings]] (CDR + slot tracker + diagnostic infrastructure that made all this debuggable), [[pipe-comms-acceptance-test]] (1-minute BLAKE3-clean — the actual acceptance bar, still a ways off).
