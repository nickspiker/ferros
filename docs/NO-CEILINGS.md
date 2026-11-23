# No ceilings: memory, the kernel and the slot tables

Status: design for the owner, 2026-11-23, on forge 17ea0da6. Nothing here is built. Building starts after the owner's read of it.

The owner, 2026-11-23: "The whole design premise is unlimited and compact." And: "if I open notepad and it requests 1mb of memory and it's happy, then I go and open a video ... and it malloc's 4GB of RAM, it better work." On the order: "Yes, mem, kernel, slot tables, all the things."

Founding rule 3 (CLAUDE.md): no fixed ceilings anywhere; a process gets RAM and space as it uses them, up to what the device has; running out is a clear refusal, never a guessed constant.

The source list is docs/CAPS-AUDIT.md (rows M0 to M25, N0 to N11, U0 to U13, S8 to S13). Storage (S0 to S7) is ONE-VAULT.md's and is not repeated here. Line numbers are forge 17ea0da6's.

Words used below:
- **The pool**: every 2 MiB frame of DRAM the kernel proved free (frames.rs, ARCS.md step 0): about 9.4 GiB on the Pro, 5.3 GiB on the 8a. It is "what the device has".
- **P**: the pool's size rounded up to a power of two: 16 GiB on the Pro, 8 GiB on the 8a. The most any one thing could ever hold.
- **A window**: a range of virtual addresses, a power of two aligned to its size, given out by the buddy over the windows' space (process.rs:810 `ARCS`). It costs nothing until frames are mapped in it. An arc is a window today; this design makes every growing thing one.

## What is built that this stands on

- **Arcs grow already** (ARCS.md step 2, process.rs:2054 SYS_ARC): ferros_rt's heap asks the kernel for frames as it runs out (ferros_rt/src/alloc.rs:49 `grow`), from anywhere in the pool, refused only at the arc's ceiling (`ARC_WHY_CEILING`) or an empty pool (`ARC_WHY_MEMORY`). Frames for a big grow are zeroed with the kernel lock let go (process.rs:8251-8258, `frames::prepare`).
- **The ceiling is the cap.** A growing image's declared heap sizes its window (process.rs:919 `arc_alloc_room`: `(ceiling + guard).next_power_of_two()`) and bounds every grow (process.rs:8387). The declaration is `__ferros_heap` (ferros_rt/ferros.ld:31, default 32 MiB) for std apps, `entry!(main, grows N)` (ferros_sys/src/lib.rs:3547) for no_std payloads, and for every payload on ferros_heap's `LockedHeap` a static array, because that allocator has no grow path at all (ferros_heap/src/lib.rs:337).
- **The windows' space is 64 GiB** (cachemap.rs:33 `ARC_VA`) inside a 39-bit space (cachemap.rs:501-503, T0SZ 25: one level-1 table, 512 GiB), its level-2 tables a static `[Table; 64]` (cachemap.rs:35). With ceilings gone, every growing image's window would be P: four apps on the Pro and the fifth spawn is "arc pool exhausted".

So the order is forced: the address space first, then the ceilings can go.

## 0. Memory that grows on demand

### 0.0 The address space: 48 bits

- **The change:** TCR's T0SZ from 25 to 16 (48-bit virtual addresses, 256 TiB), and one level-0 table whose entry 0 is today's level-1 table. Everything below 512 GiB keeps its address: the identity map, ARC_VA, the surfaces', pucks', camera's and GPU blocks' windows. The new windows' space is L0 entries 1 to 511: G#80_0000_0000 to G#1_0000_0000_0000, 255.5 TiB.
- **Tables on demand:** the level-1 table of each 512 GiB and the level-2 table of each GiB a window maps into are taken when the first frame lands there, from a "table frame" (one 2 MiB frame of the pool cut into 512 tables, as frames.rs already holds each frame's own level-3 table), and given back when the last window in them goes. The static `L2_ARCS` (256 KiB of .bss) goes. The buddy's free lists grow from 40 orders to 48 (process.rs:810).
- **Cost:** one more level in a page walk on a TLB miss (the walk caches hold the upper levels; to measure, step 2). 4 KiB of table per GiB a window touches: a small process touches two GiBs (its heap's and its stack's), 8 KiB.
- **Not needed for this:** per-process tables with ASIDs (pages_allowed_when_equivalent). They would give every process its own 48 bits and end the count below, and are the right home for native isolation later, but nothing here waits on them.

### 0.1 Every window holds what the device holds

- **A process's window is 2P** (32 GiB on the Pro, 16 GiB on the 8a): the lower half the image and the heap, growing up; the upper half the stacks, growing down (0.4). The heap can reach P, which is all the pool there is, so no allocation is ever refused for addresses, only for frames.
- **The count:** 8176 process windows on the Pro, 16352 on the 8a, against a few dozen processes today. That is a REAL bound (the address bits); past it the refusal says "no addresses left" by name. Per-process tables would lift it.
- **Masked arcs:** x27 covers the whole window from spawn (mask 2P - 1 in ring mode; exact mode's limit the window less its guard) and never changes. A grow is then only frames mapped: no thread is held for it (process.rs:8270, the hold stays for moves), and a wild pointer lands in its own window, mapped or not, never in another's. Unmapped slots fault as `ARC_RELEASE`'s holes do today. The 64 KiB guard above a masked arc (process.rs:884) becomes the window's top 64 KiB, unmapped.
- **What changes for ring mode:** a pointer past the arc's size no longer wraps into the arc; it faults in the window's unmapped part. Louder, and the same isolation (decision 2).
- **Exact mode** then buys nothing for an arc that ring does not (its point was a non-power-of-two size; a window costs no frames past what is mapped), at +24 to 36% code (SANDBOX.md "Modes"). It stays for handles to odd-sized MMIO (decision 2).
- **Fixed mode** is unchanged: the arc is the sealed 2^K its `bfi` encodes, and never grows. The size is the code, chosen by the capsule's build (decision 3).
- **Native arcs** (std apps, the native payloads) get the same window. Their pointers are whole addresses, so nothing in them changes but where the stack lies (0.4).
- **Compatibility:** an image header bit (bit 1 of the word at offset 4, beside bit 0's "grows") says "window layout". Images without it keep today's layout on the new kernel, as images without bit 0 kept a fixed heap when grows came in (process.rs:10331 `image_grows`).

### 0.2 std apps: no `__ferros_heap`

- ferros.ld:31's default becomes "the pool" (the word all ones; the kernel already takes `min(declared, map_bytes)`, process.rs:10356-10362), and every `--defsym __ferros_heap=` line goes (CAPS-AUDIT M1: Photon, Lumis, basecalc, fluordemo, rawrec, rawbench, membench, probe).
- ferros_rt's grow (alloc.rs:49) is unchanged: a quarter of the heap or the allocation, whichever is more, frames zeroed outside the lock. `layout::grow_bytes` stops asking the ceiling once it is the pool.
- The kernel's leftovers: `n > 1 << 36` refused as CEILING (process.rs:8378) and `image_tail`'s `.min(1 << 30)` (process.rs:10343) both go; ferros_rt's TLSF `FL_MAX_LOG2` 36 (ferros_rt/src/tlsf.rs:30) becomes 47, one class per bit of the space (3 KiB more in each heap's static lists).
- **Cost:** none in frames. The notepad maps its first 4 MiB (`HEAP_START`, process.rs:10328) and what it asks; the video's 4 GiB is 2048 frames zeroed with the lock let go (about 0.2 s at 90 us a frame, process.rs:8251), refused only if the pool has fewer.

### 0.3 no_std payloads: ferros_heap grows

- **The allocator:** ferros_heap's `LockedHeap` (the first-fit list) is replaced for processes by the TLSF the kernel and ferros_rt already run (ferros_heap/src/lib.rs includes ferros_rt/src/tlsf.rs by path), with ferros_rt's grow and give-back: on a failed allocation, SYS_ARC `ARC_GROW` by `grow_bytes`, the new bytes joined to the top region; a large free top given back with `ARC_SHRINK`. One copy of that logic, shared, not a second one written for no_std.
- **The payloads:** every static heap goes (CAPS-AUDIT M3: vault 56 MiB, modem 16, fgtw 8, kists 4, pucks 2, sensors 2, camera 1, gps 1, gnss_kepler, inject, init, pcie1, wallgen 248 MiB) and each declares `entry!(main, grows)` with no number (the pool). The payloads that grow with their own code (display's `Bump::more`, payloads/display/src/main.rs:733; ip, payloads/ip/src/main.rs:845; map) keep it, their ceilings dropped (M2: display 2^28, map 2^22, ip 2^25, pucks and clock 2^26).
- **ahau's bump** (25 MiB) stays as it is until its give-back after attest (ARCS.md step 4, its own change).
- **Cost:** memory goes down. wallgen's 248 MiB and the vault's 56 MiB stop being held while idle: every payload starts at 4 MiB of heap and holds what it uses.

### 0.4 Stacks

Today: a no_std payload's stack is 64 KiB at the arc's top (process.rs:9419 `STACK`; vault's own 256 KiB, payloads/vault/src/main.rs:27); a std app's main stack is 1 MiB from its heap (ferros_rt/src/start.rs:47) and every thread 1 MiB committed whole (ferros_rt/src/thread.rs:23; Photon's 79 threads hold 79 MiB of stacks).

- **A stack is a reach of addresses mapped as it is touched.** Both targets probe the stack page by page (targets/aarch64-unknown-ferros.json:45, targets/aarch64-ferros.json:29, `stack-probes: inline`), so a stack that deepens faults first in the page just under its mapped bottom. The kernel maps that page and returns to the faulting instruction. A fault anywhere else is a fault, as today.
- **4 KiB pages, not 2 MiB frames:** a stack per frame would cost Photon 158 MiB. So the kernel gains a page pool: a frame cut into 512 pages, a stack's 2 MiB slot given its own level-3 table whose entries point at pages as they are touched. A thread then costs the pages it used and one table.
- **Reaches:** the window's upper half (P) is the stacks'. The entry thread's reach is all of it at first; each new thread takes the untouched lower half of the largest reach (whose own stack lives at its top), and a thread that ends gives its reach back to its buddy. No thread count is fixed: every stack reaches at least P divided by the next power of two of the threads alive at its birth (Photon's 79: 128 MiB each on the Pro, 64 MiB on the 8a). A stack that runs to its reach's bottom faults with its thread named.
- **Syscall buffers on an untouched stack:** the kernel's buffer resolution (process.rs:1653 `span`) maps the missing stack pages before it writes, as the process's own touch would have.
- **Masked processes** follow the same rule inside the same window: x27 covers it (0.1), sp is masked into it as now (SANDBOX.md "The stack").
- **ferros_rt:** the main thread runs on the entry stack (no 1 MiB from the heap), threads ask the kernel for a reach (a new SYS_ARC op) instead of a heap block, and the guard page under each (`ARC_GUARD`, ferros_rt/src/thread.rs:28) becomes the reach's end. std's `stack_size` is then a hint the kernel ignores.
- **The one refusal that cannot be an answer:** a stack page touched when the pool is empty. The thread's process ends with "stack: the device is out of memory", named on the console and in the flash log. Every other "out" is a call's answer (0.5).

### 0.5 Running out: a refusal, said plainly

No frame is ever mapped on first touch except a stack's page: every heap frame is taken at a grow, so "out" shows at an allocation call, which can be refused.

- **The kernel:** SYS_ARC's grow answers `ARC_WHY_MEMORY` (as today). A new op `ARC_FREE` answers the pool's free bytes and what a kist may still take (decision 1), so an app sizing a ring of frames asks once before it plans, not on a timer.
- **std apps:** ferros_rt sets the allocation error hook in the ferros-rust fork's `sys` layer: "out of memory: <process> asked N bytes, the device has M free" on the console and in the flash log (CONSOLE_FLASH), then std's abort. An app that allocates by a person's request (Lumis's integrator, Mahere's load, Photon's attachment, the video's frame metadata) uses `try_reserve` and shows the refusal on its page instead of ending.
- **no_std servers:** work sized by a caller (the vault rewriting a capsule or building an export, fgtw's offers, ip's connection buffers) uses `try_reserve` and answers the caller "out of memory" in its status, instead of the panic today (CAPS-AUDIT M3: vault at 8 MiB on Mahere's capsule, fgtw on a 573 KB offer).
- **Caches give memory back under pressure:** when free frames fall under the headroom of decision 1, the kernel rings a bell (a doorbell, never a poll: feedback_no_polling). The caches that only cost speed (the display's sprites and glyph arena, kete's 64 MiB `ReadCache`, the display's wallpaper loader) wait on it and shrink, so a 4 GiB ask can take memory a cache held (decision 9).
- **No OOM killer:** nothing is ended to make room. A leaking app is refused at its next grow and everyone else's grows are refused with it until it stops; `diag arcs` names who holds what. ASID.md's "no OOM killer (arc bounded at spawn)" changes to "(refused at the grow)".

## 1. The kernel's own heap

Today: 12 MiB after the kernel's stack (ferros_kernel/src/main.rs:186) and 10 MiB in the window's second region (cachemap.rs:64 `HEAP2`, joined at main.rs:248), TLSF (ferros_heap/src/kernel.rs) over regions it never grows. A failed allocation is recorded (`last_fail`) and then panics: a reset (4 and 5 MiB values did, 2026-10-31). `STORE_VALUE_MAX` 3 MiB (service.rs:54) and `STAGE_MAX` 4 MiB (service.rs:52) exist to stay under it.

- **The kernel heap becomes a window of its own**, EL2-only, reach P, at the first window of the 48-bit space: frames mapped at its top as it runs low, exactly as ferros_rt grows (the old top's sentinel becomes the new block's header, so it stays one region and any single allocation up to the pool works). Taken unzeroed (TLSF needs no zeroes), so a grow under the kernel lock costs a frame take and a table write, not a wipe.
- **Its two regions today** stay as its first memory, for the boot before the pool exists; `HEAP2` can later join the pool instead.
- **Shrink:** the free top past a cushion given back, as ferros_rt's `shrink` does, so a 64 MiB value stored once does not hold 64 MiB for the boot.
- **The one risk, to measure first:** the heap's addresses are no longer physical. Any kernel code that hands a heap buffer's address to a device would DMA to the wrong place. The UFS path uses its own static bounce (process.rs:2148 `UFS_RUN`) and the drivers are processes, but every `as_ptr` that reaches a register is checked before step 4 (decision 8).
- **Refusals instead of a reset,** where the size is someone else's: a call's message copy (gone with 1.0 below), a stored value, a staged image, a spawn's input and objects, the partition reader's reply, a diag's text. Each takes `try_reserve` and answers ERR with "kernel out of memory" on the console. Allocations of the kernel's own small records (a process table entry, an endpoint) keep the panic: failing there means the device is truly out, and the panic says so with `last_fail`.
- **Then:** `STORE_VALUE_MAX` goes (S9: values sealed in pieces, as the vault process does) and `STAGE_MAX` goes (S11: an image staged into an arc of its size).
- **Cost:** the kernel holds what it uses: about its present peak (`diag heap`), not 22 MiB, once the shrink is in.

### 1.0 IPC at its length, and the grants area

These are the kernel's own buffers sized to today.

- **Messages (M17):** a call reads at most 4096 bytes of the caller's message and cuts the rest without a word (process.rs:7621, :7749, :7796, :7819, :7910; then `deliver` cuts again to the server's buffer, :5788, and a reply to the caller's, :8023). The shape: the message stays in the caller's memory (the caller is blocked until it is delivered) and is copied once, arc to arc, at delivery, at its length. A server whose receive buffer is shorter gets the length and "too long" instead of a cut, and receives it again with a buffer that fits. A reply too long for the caller's buffer answers its full length the same way. No kernel heap copy, so no size bound there; the copy itself is done in pieces with the lock let go between them, which is what `REPLY_MAX` (process.rs:4599) bounds today (M10 becomes a piece size, not a limit).
- **The grants page (M18, M19):** one 4 KiB page holds the handle table, the tokens (109 at most, G#204 to G#FA4) and the preferences (process.rs:9415-9418, :2536). The shape: page 0 keeps the handles, the token count, the preferences and the ihi; the tokens move to the pages after it, as many as they need, and the code starts after them (every image is position-independent and relocates itself). `SPAWN_SLOTS` 64 (ferros_sys/src/lib.rs:2300) goes with the message limit.
- **Handles stay 32:** a masked process selects a handle with one `ldp x23, x22, [x28, #16k]` (ferros_sys/src/lib.rs:3443), whose immediate reaches 504 bytes. That is REAL (the instruction's encoding); more regions are reached thru the kernel's calls or a second handle table a masked process selects by a slot.
- **Cost:** a few hundred bytes of tokens per process with many grants; the kernel's message copies go away.

## 2. Slot tables

One shape for all of them: **a slot becomes a window**, given out from the windows' space by the buddy, sized to what was asked (a power of two, so a masked holder reaches it thru a handle), its frames from the pool, recorded in a kernel `Vec` of windows (an `Object::Window` capability naming its index and epoch, found by index: no HashMap). Anything that grows in place gets a reach of P; anything sized once gets its size. The 64-byte porch at the start of every window a handle reaches stays (SANDBOX.md "Handles": an overrun of up to 63 bytes lands in the next window's porch).

The cost in every row: a `Vec` entry (tens of bytes) and a 4 KiB table per GiB a window touches; frames only as used. What changes is who names a region by a slot number.

### 2.0 The kernel's

| Row | Today | As big as needed | Who changes |
|---|---|---|---|
| Surfaces (M11) | 16 slots of 32 MiB in one window (cachemap.rs:153, :157, :161), 4 a process asks for (process.rs:3073), sixteen entries of the static `DMA` table (process.rs:3068) | a window sized to the request (a 4K panel's 63 MiB surface: 64 MiB); no count a process or the device; refused at the pool | kernel; the display and the GPU name a surface by its window, not `SURF_VA + k * 32 MiB` and the GPU's G#C000_0000 offset; fluor's ferros host, Mahere and Lumis follow |
| Pucks (M13) | 16 squares of 64 MiB (cachemap.rs:187, :189), the display's `N` 16 (payloads/display/src/puck.rs:18) | as surfaces; the display's per-puck arrays `Vec`s | kernel, display |
| GPU blocks (M12) | 16 slots of 512 MiB (cachemap.rs:233, :235) | a reach of P each (a block grows in place, so its addresses never move), a `Vec` of blocks | kernel; payloads/gpu: its slots' page-table pages (`TABLE_PAGES` 160, payloads/gpu/src/main.rs:175) and loaded kernels (`KERNELS_MOST` 16, `RING_PAGES` 4, serve.rs:35-38) taken as a client needs them; ferros_gpuq's queue depth (2^4) and uniforms (64 words, lib.rs:26-28) chosen by the client at open |
| GPU memory (M8 `GPU_MEM`) | 32 MiB of fixed non-cacheable DRAM (cachemap.rs `GPU_MEM`) for the firmware, tables and rings | non-cacheable frames from the pool as the GPU service asks, pinned (never moved), admitted by the GPU's S2MPU fence as surfaces are (s2mpu.rs `gpu_admit`) | kernel, payloads/gpu |
| Camera (M14) | 512 MiB for the run and 512 MiB for the view in one GiB (cachemap.rs:208, :221); the fence for one run of 1 GiB at most (s2mpu.rs:222 `CAM_MOST`) | the run a window of reach P and the view its read-only twin; the fence's tables per GiB as the run needs them (as `GPU_GBS`, s2mpu.rs:156) | kernel, camera driver, Lumis. The run is one physically contiguous run today (frames.rs:382 `take_run_unzeroed`): whether the front end's SysMMUs can take scattered frames is measured first; until then a run is refused when no contiguous run is free, said so |
| Camera and block reserves | `CAM_RESERVE` 32 frames (process.rs:3352), `BLOCK_RESERVE` | the measured headroom of decision 1, one rule for all | kernel |
| Mailboxes (M15) | six names compiled in, one 2 MiB frame each (ferros_mailbox/src/lib.rs:29, :48) | registered by name at run time (a writer's first publish), each a window of reach P, frames as the publication grows | kernel, ferros_mailbox, each sensor driver |
| Display carve-outs (M8) | `DISPLAY_FB` 32, `DISPLAY_HEAP` 32, `DISPLAY_PHASES` 64, `DISPLAY_SPRITES` 64, `DISPLAY_CANVAS` 32 MiB (cachemap.rs) | the framebuffers and phases: pinned frames sized from the panel's mode at modeset (contiguous, or thru the DPU's SysMMU: measured first), the DPU fenced to them; the sprites and canvas: the display's own heap (its arc), as its heap moved there on 2026-11-11; `display-heap`'s grant dropped | kernel, display |
| Driver DMA (M8 `USB_DMA` 8 MiB, `DMA_POOL` 2 MiB) | fixed regions | pinned frames from the pool sized by the driver's ask, fenced by its S2MPU; one driver at a time, measured | kernel, usb, pcie1 |
| Extent grants (S8) | 8 at once (process.rs:2214) | a `Vec` | kernel |
| GNSS tracks (U7) | 64 (process.rs:2034, ferros_sys/src/lib.rs:1710) | as long as the receiver reports, thru the `gps` mailbox's growing payload | kernel, gps, gnss_kepler |
| Preferences (U10) | 48 slots in the grants page (process.rs:2565) | settings as a VSF vault value (memory settings_identity_to_device_only); low, after ONE-VAULT | kernel, Settings |
| Kernel notices (U6) | 2^11 words for the glass (glass.rs:234) | the flash log keeps them all; the glass shows a screen's worth | kernel |

### 2.1 The processes'

All payload changes, on top of growing heaps (0.3); none needs the kernel.

| Row | Today | As big as needed |
|---|---|---|
| Deck circles (U0) | `MAX` 16 (payloads/map/src/main.rs:206), cut by `.min(MAX)` (:2130, :2157); camera `MAX`, the display's `ICON_SLOTS` 16 (payloads/display/src/main.rs:944), kists' `JOBS_MAX` 16 | `Vec`s from the person's kist list; the icon cache an LRU in the display's heap, given back under pressure (0.5) |
| The deck's documents (U1, U2) | `DECK_MAX`, `ICONS_MAX` 64 KiB (map :149-151); `CIRCLES_MAX`, `INDEX_MAX` 1024 (ferros_kept) | read at the value's length; no count bound on the person's own sealed document |
| TCP (N0) | 16 connections (payloads/ip/src/main.rs:623) | a `Vec` of connections; each one's buffers as now (1 MiB receive is the window the scaling says, not a cap on data) |
| UDP and DNS (N1) | 64 datagrams queued, 8 app ports, `HELDN` 16, `WATCHN` 8, `WATCH_PORTS` 16, `READERN` 16, `RECN` 32, `RQN` 16 (ip :74 to :522) | `Vec`s; the queue bounded by memory, oldest dropped only when a grow is refused |
| WiFi networks (U4) | 16 remembered, the oldest dropped silently (ferros_detail/src/network.rs:54); 16 shown (payloads/map/src/wifi.rs:21) | as many as the person keeps; the page scrolls |
| People (U3) | the first 32 (map :395, :429) | the whole list, paged |
| Notes waiting (U5 `WAITING`) | a nibble, 16 (ferros_kist/src/note.rs) | a wider number |
| Items (U8) | `ITEMS_KINDS_MOST` 256, `ITEMS_USAGE_KISTS` 64 (process.rs:5079, :5100) | stamps and usage lists in pieces |
| The person's export (S10) | 4 MiB whole, values past 1 MiB left out (payloads/vault/src/main.rs:1977-1978) | written as a VSF document piece by piece; nothing left out |
| A host command (S13) | 6 MiB (payloads/usb/src/main.rs:135) | streamed into the vault as it arrives |
| Photon (N10, Photon's tree) | `MAX_ATTACH` 256 MiB, `REPLY_MOST` 16 MiB | attachments already go in 256 KiB chunks: the cap goes; an HTTP body streamed into the vault |
| RAW row width (U12, vsf) | `MAX_W` 8192 (vsf src/visual/cfa_band.rs:101) | rows in blocks of any count; vsf's owner's change (memory feedback_vsf_concurrent_edits) |

## 3. The order, and how each step is checked on the devices

Each step on the 8a first, then the Pro: the 8a's smaller pool (5.3 GiB) reaches the refusal paths sooner, which is what each step must show. Each kernel step is tried by RAM reload, then flashed; capsules stored under a RAM kernel need it flashed before a cold boot (memory stored_capsules_need_flashed_kernel). Payload steps are a store, and a seal for masked ones (memory masked_capsule_needs_seal).

0. **Measure, change nothing.** On both devices: `diag arcs` (every process's size, ceiling, window, grows), `diag threads`, `diag frames`, `diag heap` (the kernel heap's peak), and copybench plus a random-access walk over 1 GiB (TLB misses) as the baseline for step 2. Recorded in this doc. No flash.
1. **ferros_heap grows; the static heaps go** (0.3). Payloads only, on the SYS_ARC every flashed kernel has. Checked: `diag arcs` shows each payload starting at 4 MiB and its grow lines; the vault rewrites Mahere's capsule; fgtw takes a 573 KB offer; the modem's data call comes up; the camera streams; a test payload grows until refused and says so while the rest runs on. The frames held at idle before and after. No flash.
2. **48-bit addresses, windows of 2P, tables on demand, masked x27 over the window, the header's layout bit** (0.0, 0.1). Kernel. Checked: every process spawns at its new base; old images (no layout bit) run as before; the TLB walk against step 0's baseline; 300 test spawns of 2P windows each; the masked stack (ip, init, modem, ahau) runs with no hold on a grow. Flash.
3. **std ceilings gone** (0.2, 0.5's hook). ferros_rt and the apps rebuilt, kists stowed again. Checked, the owner's test as he said it: a small app holding 1 MiB, then an app that allocates 4 GiB on the Pro and gets it; then allocations until the pool is empty, each refused with the hook's line and the rest of the device running; Lumis's full-resolution integrator. Needs step 2 flashed.
4. **The kernel heap grows** (1). Kernel, after the DMA check of decision 8. Checked: `diag heap` grows and shrinks; a 64 MiB value stored thru the kernel; a fill test whose refusal is an ERR on the console, not a reset. Then `STORE_VALUE_MAX` and `STAGE_MAX` go. Flash.
5. **IPC at its length, the grants area** (1.0). Kernel, ferros_sys, the servers that take a fixed receive buffer (ip's `MSG_MAX`, payloads/ip/src/main.rs:639). Checked: a 1 MiB call to an echo server; a TCP send of 64 KiB in one call; "too long" to a short buffer, never a cut; init spawned with more grants than a page holds. Flash.
6. **Stacks** (0.4). Kernel (the page pool, the fault that grows, reaches) and ferros_rt. Checked: payloads/stacktest recursing 256 MiB deep; 1024 threads in one process; Photon's stacks' frames before and after; a syscall into an untouched stack buffer. Flash.
7. **The processes' tables** (2.1). Payloads only, after step 1. Checked by counting past each old number: 17 kists on the deck, 17 TCP connections, a ninth UDP port, 17 remembered networks, 33 people.
8. **The kernel's tables** (2.0), one at a time, each measured first: surfaces and pucks, then GPU blocks and the GPU's memory, then the camera, then mailboxes, then the display's and drivers' carve-outs. Checked: 17 surfaces and a 64 MiB surface; 17 pucks; a block past 512 MiB (Mahere at full detail) and 17 clients holding blocks; a run of 16 full RAW10 frames (about 1 GiB); a seventh mailbox registered by name. Flash each.

The owner's order holds (memory, the kernel, slot tables). Stacks come after the kernel's heap because they are the largest kernel change in the memory part (a page pool and a fault that grows) and nothing but deep recursion meets the stack's cap today. Step 7 can run beside steps 2 to 6: it touches no kernel.

## 4. What stays

### REAL (kept, with why)

| Row | Bound | Why it stays |
|---|---|---|
| S14 | spine ring 2^8, four root blocks | RING.md; ONE-VAULT decision 4 |
| S15 | the seed's 64 KiB heap | founding rule 2: the seed is the one fixed thing |
| S16 | 1 MiB a UFS command, 8 queued | a transfer chunk, streamed |
| M16 | the frame bitmap, 2^13 frames (16 GiB; frames.rs:92) | sized past both boards and panics loud past them; better made from each board's map at build, so a new board never meets it |
| M20 | 16 GiBs the GPU's fence spans (s2mpu.rs:156) | sized past both boards' GiBs; goes with step 8's per-GiB tables for any board |
| M21 | 7 GPU clients resident | the Mali's 8 address spaces, AS0 the MCU's |
| M22 | per-core arrays | the SoC's cores |
| M23 | modem and AoC carve-outs | the firmware's |
| M24 | runs under 4 GiB on request | a device's address width |
| M25 | the hv's 64 MiB | HV.md |
| N3 | MSS 1460, TCP's window, scaling 14, names 255 | Ethernet, TCP, RFC 7323, RFC 1035 |
| N8 | PT chunk 512, 26 streams, 32 NAKs | USB high-speed bulk, stream letters, the NAK field |
| U13 | a zone's name, 64 bytes | IANA names |
| new | 32 handles a masked process selects | the `ldp` immediate's reach (1.0) |
| new | process windows: 8176 on the Pro, 16352 on the 8a | 48 address bits; lifted by per-process tables |

### SAFETY (kept: bounds on what someone else sends, AGENT.md Rule 0)

| Row | Bound | Size |
|---|---|---|
| M10 | `STORE_MAX` 256 KiB, `STORE_GET_MAX` 1 MiB, `REPLY_MAX` 8 MiB | right as piece sizes; with 1.0 none limits a total |
| N2 | IPv4 reassembly: `RN` 4 at once, 64 KiB each | **resize `RN` to 2^4**: 4 is tight for a fleet's pongs (the audit's finding); 64 KiB is IPv4's own |
| N4 | 8 CNAME hops, 256 s negative cache | right |
| N5 | a peer's PT transfer 4 MiB; fgtw's 2 MiB in flight; `IN_BUDGET` heap / 2 | **resize `IN_BUDGET`**: it is tied to fgtw's fixed heap; with 0.3 it becomes a share of the pool's free bytes (`ARC_FREE`), the 2 MiB per transfer stays |
| N6 | fgtw's chain, bind, eggs, address, candidate and binding bounds | right (a hundred times a real chain) |
| N7 | the people document 1 MiB | right on a fetched copy; **the person's own list** is read in pieces so it can pass it |
| N9 | HTTP headers 8 KiB | right |
| N10 | Photon's peer store (16 devices a handle, 4096 peers), relay frame 1 MiB | right |
| N11 | timeouts | each bounds a silent peer, never a moving transfer |
| U5 | a note's 1 KiB and 256 words, 64 posts between looks | a kist's words and rate; right |
| U8 | `MAKINGS_MOST` 8 | a kist cannot fill the vault's heap; with 0.3 it stays as a per-kist bound on the vault's memory, its size the vault's to measure |
| U9 | a card's 4 KiB, a find's page of 256 | design (STORE.md "The card"); bodies and finds are unbounded |
| U11 | a process's last line, a death record, a detail block | a process's own words into the kernel; right |

## Decisions for the owner

0. **48-bit addresses** (0.0): one more level in each page walk on a TLB miss, measured at step 2 before it is kept. The alternative, per-process tables with ASIDs, is larger work and also brings native isolation; it can come later on top.
1. **Who is refused first when the device runs out.** (a) First come, no reserve: the notepad's 4 GiB can leave the display without its next frame of memory. (b) Recommended: kists' grows are refused while the pool's free bytes are under the system processes' headroom, the sum of each system process's measured peak above its present size (the kernel tracks each arc's peak), a number the device measures, not a constant. It replaces `CAM_RESERVE` and `BLOCK_RESERVE`.
2. **Masked windows:** x27 over the whole window from spawn, so grows never hold threads and an out-of-range pointer faults instead of wrapping in ring mode; and exact mode kept for odd-sized handles only, ring for growers.
3. **Fixed mode** keeps its sealed 2^K (the size is in its `bfi`s): a fixed process never grows. Kept for drivers whose needs are known (touch), or retired for anything that allocates.
4. **Stacks** (0.4): reaches halved from the window's stack half, mapped a page at a time from a new 4 KiB page pool, std's `stack_size` ignored. Alternative for native processes: each thread's stack its own window of P, which costs addresses (a window a thread) and is simpler in the kernel.
5. **IPC:** a message read from the caller at delivery at its length; a short buffer told "too long" with the length. An ABI change for every server's receive loop.
6. **Regions as windows** (2.0): `Object::Window` replaces the static `DMA` table's slots for surfaces, pucks, blocks, the camera and mailboxes, and the display and the GPU stop naming surfaces by slot number. The largest cross-cutting change here.
7. **The grants area:** tokens off page 0 onto pages after it, the code offset moving per process; handles stay at 32.
8. **The kernel heap at addresses that are not physical:** needs the check that no kernel heap buffer is handed to a device by address (step 4's first act).
9. **The pressure bell:** caches shrink when free frames fall under the headroom, so a large ask takes memory the caches held. Which caches, and how far each shrinks.
