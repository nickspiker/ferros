---
name: pipe-comms-status
description: "PIPE mirror-XOR comms bring-up, merged from the 2026-05-26..28 session logs: 256/256 bit-clean Claims 7+12 reliable, late-window slip is the bridge's slot tracker, SM2 PIO sampler is the path; open items"
metadata:
  type: project
---

Merged 2026-11-12 from four dated session memories (pipe-session-2026-05-27-findings, pipe-clean-clock-test, pipe-rung0-rung1-findings, pipe-2026-05-28-session). Rig: RP2040 bridge (pipe-host `bridge`, GP0) + FPGA enclave over one wire.

**Where it stands (HEAD aaf0b14 / 180782a / 326e468, 2026-05-28):**
- Claims 7, 7a, 7b and 12 demonstrated bit-clean over 256 enclave slots, repeatable: 100% lock with the IDLE-clear preamble, 4-5 of 5 runs PASS 256/256.
- Late window (last 256 of ~10k slots) slips to ~47-72%. Measured cause: the bridge's edge-driven slot tracker, NOT wire physics. Two audible tests (FPGA divider tap; FPGA tristated + bridge SM1 square wave) were both rock stable, so both clocks are clean; the late mid-slot edges come from bridge/FPGA ownership handoffs.
- Right architecture: use SM1's own clock as the slot truth and sample the wire at SM1-aligned times. Built as PIO0 SM2 wire sampler (Architecture B, commit 180782a, 6th wire_mirror arg = 1): captures 0xAA bit-for-bit (LSB-first, slot 0 = enclave LOW); decode reached only 52-64% so far.
- Counter-FPGA listen test (326e468, `rtl/measure/top_pipe_counter.v` + host `wire_count_decode`/`wire_count_watch`): median period + P-loop + outlier rejection + sync detection decodes a 1 Hz 32-bit counter cleanly (15/15 reads); this pipeline drops into mirror-XOR decode unchanged.

**Run it:** `bridge wire_mirror 16777216 13 9 FFFFFFFF` (the host's built-in preamble holds TX=0 for 2 s first so the FPGA's silent_slot_cnt returns data_mode to IDLE). Isolation test for tracker health: TX = `00000000` (wire is a clean 50% clock, decode must be all zeros). Diagnostics: WIRE LOG (first 256 slots vs DATA/IDLE patterns), raw own_cyc first/last 256 rings, per-32-bit window match, longest perfect run.

**Lessons (don't redo):**
- Symmetric AAAAAAAA hid decode quality; use DEADBEEF or the 256-bit "The quick brown fox jumps over t" pattern (in FPGA SPI flash).
- Per-edge `own_cyc % slot_cyc` fixed parity inversion (Rung 0); later replaced by a cumulative tracker with a quarter-slot tolerance snap. Absolute-time re-sync and a narrow parity-ambiguous filter both made it worse.
- Removing the "glitches odd → flip polarity" code took decode 73% → 256/256 (it over-flipped).
- out_buf 2048 → 4096: responses over 2 KB were silently dropped.
- Slowing the wire 8x (WIRE_DIV_LOG2 11) improved edge alignment (82%) but broke decode, and the FPGA only slowed 2x: reverted, causes unknown.
- `Instant::now().as_micros()` on this bridge's embassy_time gave wrong slot counts; verify the tick rate by measurement before using it.
- Never read RP2040 registers from core 1 inside fast_sync's interrupt::free block (crashed the firmware); read from core 0 after it returns.

**Open, in order:** (0) FPGA-state mystery: sampler sees enclave LOW with bridge driving 1 yet decode says IDLE, inconsistent with `top_pipe_mirror_xor.v` latch logic (bridge_last_decoded and data_mode set in one always block); add LEDs for data_mode and bridge_last_decoded first. (1) sampler midpoint sweep (nop[N], N in 3..11). (2) sampler only decodes on the first run after an FPGA power cycle. (3) 99.6%+ over 10 trials, then 4096-bit and the 1-minute test ([[pipe-comms-acceptance-test]]).

Related: [[pipe-load-dev-is-ram-hotload]], [[feedback-measure-before-fixing]], [[pipe-mirror-xor-line-code]].
