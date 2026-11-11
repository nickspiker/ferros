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

### Step 2: grow and shrink on demand (built)

- **The call:** SYS_ARC = 40 (`ferros_sys::arc`: `grow`, `shrink`, `size`). Op 0 grows by at least x1 bytes and answers the new top and the old one (the arc's size, or exact mode's limit), the bytes between new, zeroed and the caller's; op 1 keeps the first x1 bytes and gives the rest back; op 2 answers the top and the ceiling. No capability: a process reaches only its own arc, up to its own ceiling.
- **By mode:** native grows by whole frames; ring doubles its mask (x27) until the bytes fit; exact moves its limit (x27); fixed never changes (its ceiling is its size). The new frames come from anywhere in the pool. A masked arc's mask or limit must change in every thread at once: with another of its threads at EL0 elsewhere the process is held first (step 3).
- **Refusals** (x0 = ERR, x1 the reason, a console line naming the process): past the ceiling, too few free frames, under what it was spawned with, over a word the kernel holds (a bell, the sensors' block), no such op.
- **The ceiling:** an image whose header sets bit 0 of its word at offset 4 grows on demand; ferros_rt's `_entry` sets it, `ferros_sys::entry!(main)` does not, `ferros_sys::entry!(main, grows CEILING)` does for a no_std payload (the display, since 2026-11-11: COMPOSITOR.md "Memory"). Its declared heap (`__ferros_heap`) is then a ceiling: the kernel maps 4 MiB of it at spawn (or less when it declared less; enough for the 1 MiB main stack, the TLS blocks and the first allocations), and the window is sized for the whole. An image without the bit (every no_std payload, and std images built before this) gets its whole heap at spawn as before.
- **ferros_rt:** when the free list cannot serve an allocation, `alloc` asks the kernel for at least the allocation, a frame, and a quarter of the arc (a growing heap asks a few times, not once a frame), or only the allocation when the quarter would pass the ceiling, adds the new bytes to the free list and tries again; one grow at a time. Refused, the allocation fails as it always did. A new ferros_rt on an old kernel finds no SYS_ARC and keeps the heap it was given; an old image on this kernel gets its whole heap.
- **Shrink** never goes under the arc's top at spawn (grants page, code, input, first stack), and refuses while the kernel holds a word past the new end. Every core's TLB forgets what was taken before its frames are wiped.
- **To see it:** each grow and shrink is one console line (`<name>: arc grew G#… to G#… (N frames free)`); `diag arcs` shows each arc's size, frames, ceiling, window and how often it grew and shrank.

### Step 3: hold and move (built; the copy and the pause to measure on the device)

- **Hold** (process.rs `hold`): the kernel takes a process off the cores for work that must see none of its threads run. None of them is picked to run meanwhile (they wait in the queue); every core running one now gets SGI 3 (`.Lirq_hold`, enabled on the workers and the kernel's core), which parks the thread as a slice's end does; a thread in the kernel already parks at its trap's end. The last of them to leave a core does the work under the kernel lock (`hold_try`) and lets them all run again. Nothing polls and nothing waits on a timer: without the SGI (a core in the kernel when it lands) the slice's end, 10 ms at most, is the bound.
- **A held SYS_ARC:** a masked arc's grow or shrink while another of its threads runs elsewhere holds the process; the caller waits (`State::Held`, `diag threads` shows it) and is answered when the work is done, every thread's x27 changed together. Step 2's refusal for that case (`ARC_WHY_BUSY`) is no longer answered.
- **Move** (`arc_move`): each frame of the arc copied by its own address to a frame the pool gives (the first free in the map's order), the copy's level-3 table written from the old one with every page's permissions kept (W^X, the read-only grants page, BTI's guarded pages, the kernel-only guards), the slot pointed at it, every core's TLB told, the code's new copies made visible to the instruction side (CTR_EL0's IDC and DIC say what is needed), the old frames wiped back into the pool. The window does not change, so no register (x28, x26, x27), no pointer the process holds and no record the kernel keeps (grants, bells, waits, the installer's word, the sensors' block, capability objects) is rewritten, for every mode, masked or native. DMA regions are not in an arc and never move.
- **To force one:** `diag "move <name>"` (several names at once). The console line says `<name>: arc moved, N MiB copied in N us, held N us in all (waiting N us for its threads to leave)`, also in the pstore; `diag arcs` counts each arc's moves.
- **The copy:** a plain memcpy on the core that does the hold's work (often an A715 worker, not the X3 that copybench measured at 16-19 GB/s). Its time and the whole pause: to measure on the device.
- **Native processes move as safely as masked ones** (the window), so none is left unmovable.

### Step 4: compaction, and giving back (built)

- **Compaction is not needed.** A spawn or a grow takes free frames from anywhere in the pool (the window makes them one arc), so it never fails for want of a run: it fails only when too few frames are free at all, and no move makes more. Such a refusal says so (the spawn's "arc pool exhausted", the kist's ROOM reason, SYS_ARC's `ARC_WHY_MEMORY`). The move stays for rotation (step 5) and for anything that later wants frames together (a 2 MiB block mapping, a DMA region carved from the pool).
- **The windows' address space** could in principle fragment (64 GiB, given out as powers of two); a window cannot move without changing every address its process holds. At today's sizes (the largest ceiling a few hundred MiB) it is far from full.
- **A masked arc's power-of-two slack goes back at spawn:** the frames wholly between its image (code, zeroed tail, input) and its first stack's guard page are given back at once (masked code reaches only what it was linked with). ahau's 128 MiB arc holds a 107 MiB image: 18 MiB back. Exact mode keeps its slack (its limit grows into it); a native arc is whole frames already.
- **SYS_ARC op 3, `ARC_RELEASE(offset, length)`** (`ferros_sys::arc::release`): the frames wholly inside the range given back while the arc keeps its size; the range is a hole the process must not touch again (a fault ends it) until a grow over it maps it afresh. Never the first frame (the grants page, which the kernel writes into); refused while the kernel holds a word there (a bell, the sensors' block). No syscall buffer may lie in a hole (`span`, `word`), and the installer's word is checked against them before each write. `diag arcs` shows how much each arc gave back inside.
- **What ahau can give back after attest** (read from its build of 2026-11-11, `llvm-nm`; arc offsets are these plus the code offset, G#1000, and move with every build, so ahau would compute them from its symbols): after attest ahau stays as the keyboard for words the identity service asks for (main.rs `host_words`), and in that mode it draws the keyboard and the words but not the attest mark (main.rs `all`: the mark is skipped in `Mode::Words`).
  - The attest mark's buffers, 34 MiB: FRESH (8 MiB, image offset G#3BF_6C80), FRESH_MAP (24 MiB, G#52F_AC80), FRESH_COARSE (2 MiB), FRESH_MASK (128 KiB). Before giving them back ahau must have stopped every thread that renders the mark (its helpers, HELP_STOPPED, and the mark's own thread) and must never start a render again.
  - Most of HEAP (25 MiB, G#22F_6C80): the handle proof's scratch, and also ahau's global allocator (a bump with marks), which the keyboard's keystrokes and words still use after attest, so only what lies above a cap ahau sets on that bump can go (a few MiB kept below it).
  - To keep: BUF (32 MiB, the frame the keyboard draws into), BOARDS (15 MiB, the keyboard), the stacks and the small statics.
  - In all about 55 MiB more, on top of the 18 MiB of slack returned at spawn. Not built: the change is ahau's (and a masked ahau must be sealed again on each device after it is stored: memory masked_capsule_needs_seal), and which of its threads may touch the mark after attest is for the owner to confirm, not to guess.

## For the owner

- A process's ceiling: from its build (`__ferros_heap` as a cap), from a person's setting per kist, or none (the device's free memory is the only limit).
- Mahere's own changes (owner, 2026-11-11: "a few things should be redesigned in mahere, check"): draw into a display surface instead of three full-screen frames of its own (about 32 MB back), and its heap as a ceiling once step 2 exists.
