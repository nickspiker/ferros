---
name: pipe-clean-clock-test
description: "`wire_mirror MAX FRAC P 00000000` is the isolated-bug test for slot_phase parity drift. Wire = perfect 50% duty cycle clock, decode SHOULD be all zeros; currently shows all 1s = slot_phase inverted."
metadata: 
  node_type: memory
  type: reference
  originSessionId: 8a47d2cf-0516-4c75-ab36-e6a43da45e08
---

To verify bridge slot tracker health independent of FPGA handshake / mirror-XOR / data pattern complexity:

```
cargo run -p pipe-host --release --bin bridge -- wire_mirror 16777216 13 9 00000000
```

Bridge sends all-zeros as TX_PATTERN. This means:
- During acquisition (bridge silent): wire pulldown LOW during host, FPGA IDLE drives HIGH during enclave → wire toggles 1-slot period (clean 50% duty cycle clock at slot rate)
- After mirror_start (bridge drives 0 every host slot): wire pattern UNCHANGED (bridge-driven LOW ≡ pulldown LOW). No SM1 startup race window because there's nothing to RACE; the wire pattern is identical pre/post mirror_start.
- FPGA stays in IDLE forever (no 1 bit ever drives the handshake)

Expected behaviour if slot tracker is healthy:
- `mid-slot rejected: <5%` (= all edges are at real slot boundaries)
- `wrong polarity: ~0%` (= bridge's polarity predictions match reality)
- `FPGA decode: G#00000000` (= bridge's last_driven_bit XOR'd with wire matches IDLE-mode expectation, all zeros)
- `WIRE LOG: 64/64 match` (always — wire IS that clean)

Currently observed (as of forge-branch HEAD 2026-05-27):
- mid-slot rejected: 55-58% (= bridge thinks most edges are mid-slot, because `cumulative_cyc_since_anchor % slot_cyc` integrates ppm-level CDR error indefinitely and eventually drifts off any real slot boundary)
- wrong polarity: ~1% (small after the clean clock)
- FPGA decode: G#FFFFFFFF (= ALL ONES — slot_phase parity is INVERTED. Bridge is reading the wire DURING HOST SLOTS thinking they're enclave slots. `1 XOR 0 XOR 0 = 1` per inverted host samples → all ones)
- WIRE LOG: 64/64 match (= bridge's tracker DID get the wire pattern right for the first 256 slots before drifting later in the run)

**The bug**: slot_phase parity drifts mid-run because cumulative_cyc_since_anchor / slot_cyc rounds inconsistently as slot_cyc updates from CDR slew. Once parity flips, the slot tracker's near-boundary check (`cumulative % slot_cyc`) rejects real edges as "mid-slot", suppressing the re-anchor logic that would restore alignment → cascade failure.

**Naive fix that broke the bridge** (forge-branch attempt 2026-05-27, reverted): inline a period-narrow-window check before the mid-slot full-skip, override accept_for_cdr=true on pass. Failed because `prev_own_cyc` only updates on accepted edges → on rejected-edge override-checks, `period_now_pre = own_cyc + STALE prev_own_cyc` gave garbage values, override misfired or didn't fire when needed. Bridge eventually wedged.

**Path that probably works**: replace `cumulative_cyc_since_anchor % slot_cyc` with `own_cyc % slot_cyc` for the near-boundary check. Per-edge interval modulo slot_cyc, not cumulative. No long-term drift integration. Single-slot edges always have own_cyc ≈ slot_cyc → own_cyc % slot_cyc ≈ 0 → always near-boundary. Related: [[measure-before-fixing]] (this attempt should have been instrumented FIRST), [[pipe-load-dev-is-ram-hotload]] (bridge wedged → confirm SRAM firmware actually loaded before claiming load_dev was the problem).
