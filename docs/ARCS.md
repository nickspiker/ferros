# ARCS: process memory that scales on demand

Approved 2026-11-11 (owner: "Looks good, build it!"): 2 MiB frames for every arc ("2MB frame size across the board is totally fine by me"), a move at any moment ("a move can happen at any moment"), and the Mahere changes below.
KERNEL.md module 3 (ring memory) and SANDBOX.md (the three masking modes) are the design this serves; this doc is how the kernel gets from today's arcs to theirs.

## Why (2026-11-10, Mahere would not open)

Mahere installed on the Pro and every tap on its circle was refused: "the arc pool has no room for the kist".
Its image is 45 MB and its build declares a 160 MB heap (`__ferros_heap`), so it asked for 205 MB, which the buddy allocator rounds to a 256 MB block.
The big pool is 320 MB (G#A000_0000 to G#B400_0000) and ahau alone holds 128 MB of it, so no 256 MB block can exist.
Photon's development image is 91 MB on its own.
Owner: "I'm trying to figure out why we cannot scale on demand, that was the whole point of ring memory and rotating".

## What the design says, and what was built

| | The design (KERNEL.md, SANDBOX.md) | Built |
|---|---|---|
| Arcs per process | one or more; "fragments are just more arcs" | one |
| Size | ring mode doubles in place (x27), exact grows by any amount (x27 a limit) | fixed at spawn: the image plus the heap the build declared |
| Where | anywhere in a wrapping address space | a physically contiguous power-of-two block from a buddy allocator |
| The memory it comes from | the device's | two pools carved at boot: 16 MB (G#8300_0000) and 320 MB (G#A000_0000); the rest of the 12 GB (Pro) is in no pool |
| Rotation | continuous | not built |

So the wall Mahere met is the scaffolding, not ring memory: a size fixed at spawn, physical contiguity, and two boot-time puddles.

## Proposal

### 0. One pool: all free DRAM

Every byte of DRAM that memblock proves free (never a device tree's reserved-memory node alone: memory feedback_reservation_proof_is_memblock) goes into one frame pool, in 2 MiB frames with 4 KiB frames carved from them for small processes.
The boot carve-outs (kernel, the hv, the DMA area, gpu-mem, the modem's and the secure world's) stay where they are, listed once, per board.

### 1. An arc grows in place, or moves

- **Grow:** a process asks (a syscall; ferros_rt's allocator asks when its heap runs out, so a build declares at most a ceiling, never a reservation). If the frames right after the arc are free, they join it: ring mode's mask doubles, exact mode's limit moves. Nothing else changes.
- **Move (owner, 2026-11-11: "memory speed is very fast so a simple move of application memory and change of address will be likely imperceptible on a large app launch"):** if the neighbours are taken but enough is free elsewhere, the kernel stops the process's threads, copies the arc to a place it fits with room to grow, and sets the new base.
  - Fixed and ring modes keep only the low bits of a pointer (`bfi`, `and` with x27), so every stored pointer is already an offset into the arc: the move is the copy plus the base register (x28, x26's high bits). No pointer is rewritten.
  - Exact mode and native std processes (Mahere, Photon today) hold whole addresses: for them the arc is a window of virtual addresses the MMU maps (owner, 2026-11-03: pages allowed where equivalent and faster), and a move is the copy plus remapping the same window to the new frames; the process never sees an address change.
  - **Measured on the Pro (2026-11-11, payloads/copybench, one Cortex-X3, 16 MiB copied 16 times):** memcpy best 19.4 GB/s, mean 16.1 GB/s; a 16-byte-word loop best 18.9, mean 16.8; read back the same. Mahere's whole 205 MB moves in about 11-13 ms, under one frame at 60 Hz; a few tens of MB in 1-3 ms. A move split across cores may come nearer LPDDR5X's ~50 GB/s peak: not measured yet.
- **Shrink and give back:** the same call in reverse, and every end returns the whole arc (one shot dead, as now). ahau gives back its frames after attest.
- **Compaction is the same move,** done when a spawn or a grow finds enough free memory but no single run of it.
- **Rotation** (KERNEL.md's continuous rotation) becomes the same move on a schedule the owner sets, once it is cheap enough to measure as nothing.

### 2. What never moves

A region a device can write (WiFi's rings, the modem's shared memory, the GPU's, USB's DMA) stays in the DMA area, pinned, behind its S2MPU fence: a device holds physical addresses and a move would let it write into whatever lands there next.
Only CPU-only memory moves, which is every app and most of every driver.

### 3. What does not change

Capsules, seals, the three masking modes, the checker, grants and capability IPC all act on the arc as a window; the window simply stops being a fixed physical block.

## Steps (each measured on the Pro before the next; flashes with the owner around)

0. The memory map: every free range proved by memblock on husky and akita, listed per board (BOARDS.md); the frame pool over them.
1. Spawn from the frame pool (contiguous runs, as now, just from a big pool): Mahere and Photon fit on day one.
2. The grow and shrink syscall; ferros_rt's allocator grows on demand; `__ferros_heap` becomes a ceiling.
3. Move: masked processes first (base register only), then native ones under the MMU window; measure the copy.
4. Compaction on a failed spawn or grow; ahau's give-back after attest.
5. Rotation, if the measurements say it is free.

## Built (2026-11-11, branch `arcs`; on the device: not yet)

Every step below compiles and leaves the kernel bootable on its own; none has run on a device yet, so every figure still to measure says so.

### Found while building: every mode needs the window

The proposal moved fixed and ring arcs by their base register alone, since masked code keeps only the low bits of a data pointer.
Its code pointers are whole addresses: the return addresses `bl` leaves in x30 and on the stack, the function pointers its relocations filled at start (vtables), and an indirect branch is not masked (SANDBOX.md: `br`, `blr`, `ret` are allowed as they are).
After a copy to a new base the first `ret` would jump into the old frames: freed, so a fault, or another process's code.
So every arc, masked or native, is now a window of virtual addresses, and its frames lie wherever the pool has them: a move is the copy and a remap, and no register, pointer or kernel record changes.

### Step 0: the memory map (built)

`ferros_kernel/src/frames.rs` and BOARDS.md "Memory map": per board, the ranges and their proof, in the order frames are taken; `diag frames` reads them back.
husky: the two old pools (336 MiB) and the high bank's 9092 MiB that Android's memblock left unreserved (its boot log of 2026-08-17).
akita: the two old pools only, until a memblock capture proves its high bank (the commands are in BOARDS.md).

### Step 1: one frame pool, every arc a window (built)

- **The pool** (frames.rs): a bitmap of 2 MiB frames over the map, taken first-free in the map's order (the old pools fill before the high bank), wiped as they come back, zeroed on first use; the scrub before a reset zeroes only the frames arcs hold.
- **The windows** (cachemap ARC_VA): 64 GiB of addresses from G#40_0000_0000, past every physical address, given out by a buddy allocator as powers of two aligned to their size (the old buddy code, over addresses now instead of memory).
- **The page tables:** every frame owns one level-3 table (held in the pool's first frames: 10 on husky, 1 on akita), sized from the map rather than a fixed 256; a window's 2 MiB slot points at the table of the frame it holds. W^X, the read-only grants page, BTI's guarded pages and the stack's guard page are set in those tables exactly as before. The frames themselves are mapped once more at their own addresses, the kernel's alone (zeroing, copying, the scrub).
- **Sizes:** a masked arc is a power of two (64 KiB at least) aligned to it, as its mask needs, in whole frames, the rest of its last frame and the unmapped rest of its window its guard; a native arc is a whole number of frames (Mahere's 205 MB takes 206 MiB, not 256).
- **Offsets a syscall names as words** (SYS_WAIT, SYS_WAKE, the bells, the installer's word, a thread's entry and stack) are checked against the arc's size, not masked: a native arc is no power of two now. Outside it, a word traps the caller (`bad_buf`) and a thread is refused.
- **Escape:** a `FERROS_FRAMES_HIGH=0` build keeps husky to the two old pools.
- **A RELOAD on akita** carries the board in x0 bit 63: a reloaded kernel has no device tree, and husky's map names DRAM akita does not have.

## For the owner

- A process's ceiling: from its build (`__ferros_heap` as a cap), from a person's setting per kist, or none (the device's free memory is the only limit).
- Mahere's own changes (owner, 2026-11-11: "a few things should be redesigned in mahere, check"): draw into a display surface instead of three full-screen frames of its own (about 32 MB back), and its heap as a ceiling once step 2 exists.
