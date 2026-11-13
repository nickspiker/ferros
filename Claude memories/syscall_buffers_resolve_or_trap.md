---
name: syscall-buffers-resolve-or-trap
description: "owner 2026-11-08: syscall buffers resolve against the arc then the process's own DMA regions; anything else traps (dev builds park, production kills one shot) with a full report; BUILT (process.rs buf_trap, KISTS.md)"
metadata:
  type: project
---

Decided with the owner on 2026-11-08 after the wallpaper bug. The display's heap is a DMA region outside its arc. The kernel masked a heap pointer's reply into the arc, so the reads came back zeros and the display's own memory was overwritten. The owner: "I don't particularly care if it mutates itself", but asked for something better. Then: "so you can basically run it in debug/trap mode. I like!"

**The rule:** the kernel resolves every syscall buffer in this order:
- **In the arc:** used as-is, one compare.
- **Inside one of the process's own DMA regions** (as `dma_span` already does for the USB driver): used directly. This gives the process nothing new.
- **Anywhere else:** a trap.

A trap reports the process and thread, the syscall and its op, the address and length, the nearest region ("4 KiB past display-heap"), and the PC and LR at the svc. The report goes to the console and the flash log.
- **Dev builds** park the thread with its memory intact, to inspect over the bridge, then kill it by hand.
- **Production builds** kill it, one shot, no cleanup.

There is no ABI change: `ferros_sys::off()` already yields an out-of-range offset for such pointers.

**Why:** a silent wrap moves the symptom far from the cause. An error code gets ignored. A trap at the exact call is a segfault with an address.

**Built** (process.rs `span` + `buf_trap`, docs/KISTS.md "Syscall buffers resolve or trap"); payloads/buftrap is its test (on the Pro it parked with `TRAP syscall 1 ... past its arc`). The grants page is read-only and also traps (703674b3).

**How to apply:** a TRAP line on the console names the process, syscall, offset and nearest region: fix the caller. Never reintroduce a mask-wrap of an out-of-arc buffer.

Related: [[permissions-asked-once]], [[pages-allowed-when-equivalent]].
