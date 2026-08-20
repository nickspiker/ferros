---
name: pipe-comms-acceptance-test
description: "The acceptance bar for \"PIPE comms is done\" = 1-minute full-duplex PRNG with BLAKE3 hash match on both ends. Zero bit errors over ~1.3M bits each way."
metadata: 
  node_type: memory
  type: project
  originSessionId: 8a47d2cf-0516-4c75-ab36-e6a43da45e08
---

PIPE comms gets declared "done" when this test passes:

1. Bridge drives a long PRNG stream into the wire for ~60 seconds (~1.3M bits at ~22 kHz slot rate)
2. FPGA decodes the bridge stream and computes BLAKE3 over what it received
3. FPGA drives its own long PRNG stream concurrently (full duplex via Claim 12 mirror-XOR)
4. Bridge decodes the FPGA stream and computes BLAKE3 over what it received
5. Both sides display / report their two digests (TX_hash, RX_hash)
6. Match across both ends = 1:1 delivery proven over ~1.3M bits → patent-demonstrable

**Why:** Any single bit slip across ~60 seconds of continuous traffic flips the whole BLAKE3 digest. No way to fudge it, no partial credit. This is the bar the patent demo needs to clear. Smaller intermediate tests (handshake fires, wire stays locked for 4096 slots, etc.) are useful milestones but don't prove 1:1 delivery — only the cryptographic end-to-end does.

**How to apply:** When planning Phase 2c/2d/2e slices, the work is "complete" only when this test passes. Intermediate metrics (ppm drift, EMA SE, lock holds N edges) are progress indicators, not done signals. Don't declare PIPE comms shippable on partial verification.

Related: [[pipe-mirror-xor-line-code]], [[pipe-pt-unified-stack]], [[pipe-handshake-is-entropy-request]]
