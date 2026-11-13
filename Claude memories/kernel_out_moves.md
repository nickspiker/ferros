---
name: kernel-out-moves
description: "Moving drivers out of the ferros kernel (KERNEL-OUT.md, USB.md): USB+PT, touch bring-up and the sensors' USF run as processes since 2026-10-31; the pattern, the traps found, and the decisions left to the owner"
metadata:
  node_type: memory
  type: project
  originSessionId: fef83e98-ba6e-4773-9615-f79c25605c30
  modified: 2026-11-01T05:00:22.486Z
---

Owner goal (2026-10-31): the kernel does only kernel things, plus the killswitch. The plan and the record are in ferros KERNEL-OUT.md; USB in USB.md.

**State after the night of 2026-10-31.** Slot b carries this kernel; slot a is untouched as the escape.
- `payloads/usb` serves PT from boot (init slots 86-90), about 20 MB/s each way, interrupt-driven. The kernel's USB runs only until init starts it, and again if it ends.
- `payloads/touch` brings its controller up through a `touch` power switch (slot 91). A finger on the glass was not verified that night.
- `payloads/sensors` holds the USF conversation (slots 92-96). It publishes a block the kernel reads for SYS_FACTS, SYS_LIGHT and the proximity wake rule (`sensorlink.rs`). The kernel still starts the AoC.

**The pattern:** grants, then a power switch for the privileged part, then the protocol in the process, then init starts it, then the kernel's copy goes. Each move keeps the kernel's copy as a fallback that resumes when the process ends.

**Traps found:**
- `kill` has its own path (`kill_locked`) beside `end_process`. A hook for "a process ended" must go in both; the sensors block was read after a kill until it did.
- Syscall buffers are arc offsets. Memory outside the arc (a DMA region) needs `DMA_ADDR | address`, checked against the caller's grants (SYS_PT).
- Decoding packets straight out of non-cacheable memory cost the CPU 2.2 s per 2 MiB. Only what the controller DMAs stays non-cacheable.
- The PT ceiling was the host: one transfer in flight, and unoptimised BLAKE3 in the debug bridge.
- A hot reload issued 8 s into a fresh boot hung the new kernel with the lock held by a worker. Reload a minute in.

**Blocked on the owner** (KERNEL-OUT.md "Decisions needed"):
- removing the kernel USB fallback;
- an Ffa capability for the AoC's start;
- the camera harness;
- the diag subcommands;
- hsi2c_11's owner;
- the boot modeset.

Related: [[smp-first-cut]], [[husky-cold-slot-b]], [[feedback-measure-before-fixing]].

**Row 7 built 2026-11-10 (forge merge of kernel-out-small-drivers):** shared `ferros_spi` crate (kernel keeps the clock paths); `payloads/amp` behind an `amp` power switch (spawned by hand, `usi1-spi` grant; verified on the Pro: both CS35L41 read DEVID G#35A40); haptics moved into the NFC process (ops 7 report, 8+n ROM waveform), reads lagged three behind on the first run (stale RX FIFO bytes), fixed in fb35747 and verified on the Pro: DEVID G#40A260, REVID G#A1; no buzz tested yet (needs a hand on the device); touch on ferros_spi (commit d4c7e25) merged but its capsule NOT deployed: cold-boot path, test only with the owner present.
