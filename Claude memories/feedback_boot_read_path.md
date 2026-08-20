---
name: Boot read path — SD is cold
description: Full powerup boot is all UFS reads, SD never touched unless BLAKE3 fails. Design for untestably-fast cold boot.
type: feedback
---

Happy-path boot is pure UFS sequential reads. SD sits completely cold.
- 8 reads: stem binary search (256 entries)
- 16 reads: spine binary search (65536 entries)
- ~2-6 reads: HAMT traversal to kernel object
- ~2048 reads: stream 8MB kernel to DRAM
- BLAKE3 verified inline on each block

SD only wakes on BLAKE3 hash failure (fallback read, verify, flag for repair).
Target: entire boot under framebuffer render time.

**Why:** The user explicitly called this out as a design goal — boot should be "literally untestable" fast. SD adds zero latency on the happy path.

**How to apply:** Never add SD reads to the normal boot path. Mirror writes always touch both disks, but reads are UFS-primary with SD as error recovery only.
