---
name: sched-trace-and-past-deadline-spin
description: diag sched (kernel scheduler trace ring) is the tool for IPC/latency stalls; the 2026-10-23 drag stall was a server's sleep_until with a past deadline spinning its slice; reloads hang on the cold kernel — flash with husky-cold.sh instead
metadata:
  type: project
---

`ferros-bridge diag sched` dumps the kernel's scheduler trace (process.rs: a 4096-event static ring of call/deliver/reply/sleep/wake/switch/run/preempt/park/poll-miss/yield, per-pair round-trip waits, the last 512 raw events). It holds ~2-3 s, so dump it *during* the thing being measured (the map's `FERROS_MAP_START=drag` build drags a circle by itself at start; `ipc` measures bare round trips: 5 us display, 2 us touch).

**Why:** the quarter-second drag stall (2026-10-23) looked like IPC cost — 10 ms a move — but the bare round trip is microseconds. The ring showed the display server making ~500k `recv` poll-misses a second until the slice timer: its wake was `next_tick.min(next_flip)` and the 10 Hz flip cannot fire while a finger moves, so `next_flip` fell into the past and `sleep_until(past)` returned at once. Every client waited for the 10 ms preempt. Fix: the flip counts toward the wake only when a flip is possible; the kernel turns a past-deadline sleep into a yield.

**How to apply:** when a call to a healthy server takes ~SLICE_MS (10 ms), suspect the server spinning its slice (a stale deadline, a poll loop), not the kernel; trace it before fixing. A server's every wait must have a future deadline or a blocking recv. Crumbs: every syscall writes G#90+n entering / G#70+n leaving (bits 8-15 the process, 16-23 the one it returns into), D1/D2 round run_slice, B3/B4 round the idle wfe — PREV_EXIT after a watchdog reset names where the kernel stuck.

**Reload hang (SOLVED 2026-10-23): kernel stack overflow.** The main kernel stack was 64 KiB; the service loop needs ~95 KiB (DIAG `STACK_USED_MAX`, painted stack). A cold boot runs the service on the X3's own 256 KiB stack after the hop; a reload skips the hop and runs it on the main stack under main's frame, so a vault write or a spawn walked ~30 KB past the bottom into .bss — random hangs, a DECON that stopped completing frames, `UFS_LOST` reading as an address. Found with the .bss canary (DIAG `CANARY_BAD/AT/VAL`). Stack is 512 KiB now (aarch64-pixel8.ld). Rule: a kernel that misbehaves differently per boot path → check the stack depth and the canary before anything else.
