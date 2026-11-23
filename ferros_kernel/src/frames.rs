//! The frame pool's memory map (ARCS.md step 0, owner 2026-11-11: "2MB frame size across the board is totally fine by me"): every DRAM range a process's arc may take its 2 MiB frames from, per board, written once here and in docs/BOARDS.md "Memory map".
//!
//! A range is listed only with memblock's proof that nothing else owns it (memory feedback_reservation_proof_is_memblock: a device tree node proves nothing, memblock or /proc/iomem does), or because ferros has run processes there since before this map (the two arc pools it replaces).
//! The order is the order frames are taken in: the ranges ferros has used longest first, so the new ones fill only when they are needed.
//!
//! husky's proof is Android's own boot log of 2026-08-17 (/mnt/Harbor/tmp/linux_full_dmesg.txt, stock 6.1.124, "Machine model: ZUMA HUSKY MP"): memblock's memory ("Early memory node ranges": G#8000_0000-G#FFFF_FFFF and G#8_8000_0000-G#A_FFFF_FFFF, 12 GiB) and every reservation it made from the device tree ("OF: reserved mem", each with the size memblock was given, so a size-0 node like the hv's of 2026-10-17 would show).
//! In the high bank those reservations are one run, G#9_BE40_0000-G#9_FFFF_FFFF (vscaler, farawimg, famodel, faceauth_dsp, faceauth_tpu, vstream, faprev, vframe, gcma_camera), and the hv's 64 MiB at the top (G#A_FC00_0000, /proc/iomem "reserved" since 2026-10-19, HV.md).
//! The 2 MiB frame under the hv is left out too: the hv copies the device tree it hands Linux to G#A_FBF0_0000 (ferros_hv DTB_COPY).
//! akita's proof is its own memblock under today's Android, under the hv, captured 2026-11-12 (/mnt/Harbor/tmp/akita-memblock-2026-11-12: debugfs memblock/memory and memblock/reserved, /proc/iomem, the boot log; Android CP2A.260805.005 on slot b): memory G#8000_0000-G#FFFF_FFFF and G#8_8000_0000-G#9_FFFF_FFFF, 8 GiB.
//! In its high bank memblock reserves the device tree copy the hv hands Linux (G#8_FFF0_0000, ferros_hv DTB_COPY), the hv's 64 MiB (G#9_0000_0000, nomap), and everything from G#9_BC04_0000 up: Linux's own early allocations under the pools, then the camera and video pools (vframe from G#9_C540_0000, farawimg, famodel, faprev, faceauth_dsp, gcma_camera, faceauth_tpu, vstream, vscaler to the top), the top being out of reach at the handoff anyway (ferros_hv build.rs).
//! So its two ranges stop at the 2 MiB frame under the device tree copy and at the frame under G#9_BC04_0000, as husky's stop under its pools and its hv.

/// A frame: 2 MiB, the unit every arc is made of (ARCS.md).
pub const FRAME: usize = 1 << 21;

/// husky's frame ranges, [start, end), each 2 MiB aligned.
#[cfg(not(shim_handoff))]
const HUSKY: [(usize, usize); 6] = [
    // The page pool (cachemap POOL): the window's top 16 MiB, ferros's own since 2026-10-26; memblock: plain memory (G#8000_0000-G#8AFF_FFFF, no reservation).
    crate::cachemap::POOL,
    // The layer arena until 2026-11-13 (cachemap POOL_LAYERS): 128 MiB under the big pool, ferros's own since 2026-11-05, on cold boots and after jumps alike; memblock: plain memory (G#9740_0000-G#B61F_FFFF, no reservation).
    crate::cachemap::POOL_LAYERS,
    // The big pool (cachemap POOL_BIG): 320 MiB under gpu-mem; memblock: plain memory (G#9740_0000-G#B61F_FFFF, no reservation; sec_dram begins at G#B620_0000).
    crate::cachemap::POOL_BIG,
    // The apps' surfaces' carve-out until 2026-11-12 (cachemap POOL_SURF): 128 MiB, ferros's own since 2026-10-27, on cold boots and after jumps alike; memblock: plain memory (the nearest reservations are seclog_mem ending G#C308_FFFF and the debug snapshot from G#D810_0000).
    crate::cachemap::POOL_SURF,
    // The high bank's first 4.97 GiB, up to the camera and video pools.
    (0x8_8000_0000, 0x9_BE40_0000),
    // The high bank past those pools, up to the frame under the hv.
    (0xA_0000_0000, 0xA_FA00_0000),
];

/// akita's frame ranges: the two pools ferros has run its processes from, then its high bank where memblock reserves nothing (2026-11-12; memblock: plain memory under both pools too).
#[cfg(not(shim_handoff))]
const AKITA: [(usize, usize); 6] = [
    crate::cachemap::POOL,
    // memblock: plain memory (G#9740_0000-G#B61F_FFFF on akita too, BOARDS.md "Memory map").
    crate::cachemap::POOL_LAYERS,
    crate::cachemap::POOL_BIG,
    // memblock: plain memory (G#C309_0000 to G#E7FF_FFFF reserves nothing on akita either).
    crate::cachemap::POOL_SURF,
    // The high bank up to the frame that holds the hv's copy of the device tree (G#8_FFF0_0000).
    (0x8_8000_0000, 0x8_FFE0_0000),
    // Past the hv's 64 MiB, up to Linux's own allocations and the camera and video pools (G#9_BC04_0000).
    (0x9_0400_0000, 0x9_BC00_0000),
];
/// Either board kept to the two old pools and the old carve-outs of the layer arena and the surfaces (`map` in a `FERROS_FRAMES_HIGH=0` build; after a jump the frames outside them are held, `held_after_jump`): what ferros ran from after jumps before ARCS, the GPU's memory and surfaces included.
#[cfg(not(shim_handoff))]
const LOW: [(usize, usize); 4] = [crate::cachemap::POOL, crate::cachemap::POOL_LAYERS, crate::cachemap::POOL_BIG, crate::cachemap::POOL_SURF];

/// The handoff path (oracle only): its 2 MiB pool.
#[cfg(shim_handoff)]
const HUSKY: [(usize, usize); 1] = [crate::cachemap::POOL];
#[cfg(shim_handoff)]
const AKITA: [(usize, usize); 1] = [crate::cachemap::POOL];
#[cfg(shim_handoff)]
const LOW: [(usize, usize); 1] = [crate::cachemap::POOL];

/// This board's frame ranges. A `FERROS_FRAMES_HIGH=0` build keeps either board to the two old pools and the surfaces' old carve-out: the escape if its high bank ever misbehaves, as `FERROS_X3=0` is the X3's.
/// The map is the same after a jump (the kernel maps every frame of it by its own address, so the tripwire reaches the high bank); what a jump changes is which frames are held (`held_after_jump`).
pub fn map() -> &'static [(usize, usize)] {
    if option_env!("FERROS_FRAMES_HIGH") == Some("0") {
        &LOW
    } else if crate::board::akita() {
        &AKITA
    } else {
        &HUSKY
    }
}

/// A `FERROS_FRAMES_JUMP_HOLD=1` build holds the frames outside the old pools (`LOW`) after a jump from Android (or a ferros restart, jump style): in the map and mapped, never given to an arc until `diag frames release`, so the tripwire (`trip_arm`, `trip_check`) can watch Android's memory for writes before any process lives there.
/// Every build did so from 2026-11-11 (the Pro: a boot that came from a jump ran about a minute after attest and reset with no panic recorded, read as a DMA master Android left running) until 2026-11-15, when the tripwire on the jumped Pro and every bus master accounted for (HV.md "The high bank after a jump") gave the high bank back to jumped sessions.
fn held_after_jump(a: usize) -> bool {
    option_env!("FERROS_FRAMES_JUMP_HOLD") == Some("1") && crate::platform::jumped() && !LOW.iter().any(|&(lo, hi)| a >= lo && a < hi)
}

/// The ranges as `diag frames` reads them: start, end, and size in MiB.
pub fn map_report() -> alloc::string::String {
    let mut s = alloc::string::String::new();
    let mut total = 0;
    for &(lo, hi) in map() {
        total += hi - lo;
        let _ = core::fmt::Write::write_fmt(&mut s, format_args!("range G#{lo:X}-G#{hi:X}: A#{} MiB\n", (hi - lo) >> 20));
    }
    let _ = core::fmt::Write::write_fmt(&mut s, format_args!("A#{} MiB in A#{} frames of 2 MiB\n", total >> 20, total / FRAME));
    s
}

// ---------------------------------------------------------------- the pool

/// The most frames a map may hold: 16 GiB, past the most DRAM of either board (husky's 12 GiB). A map past it is a mistake in this file, and `pool` panics on it.
const MAX_FRAMES: usize = 1 << 13;
const WORDS: usize = MAX_FRAMES / 64;
/// One level-3 table: 512 entries of 8 bytes.
const TABLE: usize = 1 << 12;
/// Tables in one frame.
const TABLES_PER_FRAME: usize = FRAME / TABLE;
/// A map of at most this many frames keeps its level-3 tables here, in the kernel's .bss (Normal memory the table walker reads), not in frames of its own.
const SMALL_TABLES: usize = 1 << 3;
#[repr(C, align(4096))]
struct Small([[u64; 512]; SMALL_TABLES]);
static mut SMALL: Small = Small([[0; 512]; SMALL_TABLES]);

/// The frames, by index across the map's ranges in order: which are taken, which are known to hold zeros (every frame given back is wiped; a frame not yet taken this boot holds whatever DRAM held), and how many of the first frames hold the level-3 tables (frame k's is table k % 512 of frame k / 512; never handed out, and not one run: husky's ten spill from the page pool's range into the big pool's).
/// `held`: frames in the map no arc may take (`held_after_jump`), counted in `used` too so no take finds them, and never scrubbed (they hold no process's bytes).
/// `armed`: frames the tripwire wrote its pattern into (`trip_arm`), free or held; a take clears the bit (the frame is zeroed then, since arming clears `clean`).
struct Pool {
    used: [u64; WORDS],
    clean: [u64; WORDS],
    held: [u64; WORDS],
    armed: [u64; WORDS],
    total: usize,
    free: usize,
    held_n: usize,
    table_frames: usize,
    init: bool,
}

/// Every access is under the kernel lock (process.rs `lock`), as the arcs that hold these frames are; the scrub before a reset reads it with every other core stopped.
static mut POOL: Pool = Pool { used: [0; WORDS], clean: [0; WORDS], held: [0; WORDS], armed: [0; WORDS], total: 0, free: 0, held_n: 0, table_frames: 0, init: false };

fn pool() -> &'static mut Pool {
    let p = unsafe { &mut *(&raw mut POOL) };
    if !p.init {
        p.init = true;
        p.total = map().iter().map(|&(lo, hi)| (hi - lo) / FRAME).sum();
        assert!(p.total <= MAX_FRAMES, "frames: the map holds more frames than the pool counts");
        // The tables first, in the first frames: one table per frame, those holding tables included (never mapped, so their own tables are spare).
        // A map of a few frames (the handoff path's one 2 MiB pool) keeps its tables in the kernel's own memory (`SMALL`): one frame of tables would leave it none for arcs.
        p.table_frames = if p.total <= SMALL_TABLES { 0 } else { (p.total * TABLE).div_ceil(FRAME) };
        for f in 0..p.table_frames {
            p.used[f / 64] |= 1 << (f % 64);
            zero(pa(f));
        }
        for f in p.table_frames..p.total {
            if held_after_jump(pa(f)) {
                p.used[f / 64] |= 1 << (f % 64);
                p.held[f / 64] |= 1 << (f % 64);
                p.held_n += 1;
            }
        }
        p.free = p.total - p.table_frames - p.held_n;
    }
    p
}

/// Frame `f` taken (under the kernel lock): marked used, out of the tripwire, zeroed unless known clean.
fn took(p: &mut Pool, f: usize, zeroed: bool) {
    let (w, b) = (f / 64, 1u64 << (f % 64));
    // A clean frame taken: the frames kept clean ahead may be short now (`clean_step`).
    if p.clean[w] & b != 0 {
        CLEAN_LOW.store(true, core::sync::atomic::Ordering::Relaxed);
    }
    p.used[w] |= b;
    p.armed[w] &= !b;
    p.free -= 1;
    if zeroed && p.clean[w] & b == 0 {
        zero(pa(f));
    }
    p.clean[w] &= !b;
}

/// Frame `f`'s address: its range's start plus its place in it.
pub fn pa(f: usize) -> usize {
    let mut k = f;
    for &(lo, hi) in map() {
        let n = (hi - lo) / FRAME;
        if k < n {
            return lo + k * FRAME;
        }
        k -= n;
    }
    panic!("frames: no frame G#{f:X}");
}

/// Frame `f`'s level-3 table (the one `cachemap::map_slot` fills when the frame joins an arc).
pub fn table(f: usize) -> usize {
    if pool().table_frames == 0 {
        return (&raw const SMALL) as usize + f * TABLE;
    }
    pa(f / TABLES_PER_FRAME) + (f % TABLES_PER_FRAME) * TABLE
}

/// The frame a level-3 table belongs to (`cachemap::slot_table` gives the table of a mapped slot).
pub fn of_table(t: usize) -> usize {
    let p = pool();
    if p.table_frames == 0 {
        return (t - (&raw const SMALL) as usize) / TABLE;
    }
    let j = (0..p.table_frames).find(|&j| t >= pa(j) && t < pa(j) + FRAME).expect("frames: not a frame's table");
    j * TABLES_PER_FRAME + (t - pa(j)) / TABLE
}

/// Frames free now, those given back and not yet wiped among them (`RETURNING`): a take finds them, wiping them first when it must.
pub fn free() -> usize {
    pool().free + returning().len()
}

/// The bytes arcs hold now (the scrub's count before a reset): frames given back and not yet wiped are counted, as the scrub zeroes them with the rest.
pub fn taken_bytes() -> usize {
    let p = pool();
    (p.total - p.table_frames - p.held_n - p.free) * FRAME
}

// ---------------------------------------------------------------- frames coming back (2026-11-18, lock-holds)

/// Frames given back (`give`) and not yet wiped. A 2 MiB frame's zeroing is about a tenth of a millisecond, and `give` made it under the kernel lock: a process's end held every core a frame of its arc at a time (the camera driver's region, 5.8 ms at its end, `diag lock` 2026-11-16; a 128 MiB arc about 5 ms).
/// So `give` now only empties the frame's table and queues the frame here, still marked used: no take finds it and a reset's scrub zeroes it with every frame an arc holds (`scrub`, `taken_bytes`), as the owner's rule wants (2026-11-08: the kernel wipes what a process held as it reclaims it).
/// Idle workers and the service loop wipe them with the lock let go and put them back clean (`reclaim_step`); a take that finds the pool short is handed waiting ones (`take_returning`, `take_run_search`), so no take is refused for frames that are on their way back.
/// Handed, not freed: a frame stays marked used from its give to its new owner, so a reset's scrub zeroes it whenever the reset comes, and only the frames a take hands out are zeroed then, each by its taker (until 2026-11-22 `reclaim_all` wiped every waiting frame under the kernel lock at once, about 90 us a frame: 138 ms after a dev run's 3 GiB arc came back).
/// Under the kernel lock; `RETURNING_N` mirrors its length for a look without it.
static mut RETURNING: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
static RETURNING_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn returning() -> &'static mut alloc::vec::Vec<usize> {
    unsafe { &mut *(&raw mut RETURNING) }
}

/// Frame `f`, given back and wiped (by `reclaim_step`, or now when `zero_now`), free again and known clean. Under the kernel lock.
fn reclaimed(p: &mut Pool, f: usize, zero_now: bool) {
    if zero_now {
        zero(pa(f));
    }
    p.used[f / 64] &= !(1 << (f % 64));
    p.clean[f / 64] |= 1 << (f % 64);
    p.free += 1;
}

/// A frame waiting to come back handed to a take that found the pool short, None when none waits: out of the queue and still marked used (as it was since its give), so it goes from its old owner to its new one without ever being free. Zeroed now when `zero_now` (one frame under the kernel lock, the cost of a take of a frame not known clean); a taker that fills it whole or zeroes it itself passes false. Under the kernel lock.
fn take_returning(zero_now: bool) -> Option<usize> {
    let f = returning().pop()?;
    RETURNING_N.store(returning().len(), core::sync::atomic::Ordering::Relaxed);
    if zero_now {
        zero(pa(f));
    }
    Some(f)
}

/// One frame given back wiped with the kernel lock let go, then freed under it: idle workers in place of their sleep (smp.rs), and the service loop each pass. False when none waits.
/// Between the two holds the frame is out of the queue and still marked used, as the background wipe holds its frames (`wipe_step`): a take short of frames meanwhile does not see it, for the tenth of a millisecond the zeroing takes.
pub fn reclaim_step() -> bool {
    if RETURNING_N.load(core::sync::atomic::Ordering::Relaxed) == 0 {
        return false;
    }
    let Some(f) = crate::process::with_lock(|| {
        let f = returning().pop();
        RETURNING_N.store(returning().len(), core::sync::atomic::Ordering::Relaxed);
        f
    }) else {
        return false;
    };
    // SAFETY of the zeroing without the lock: the frame is marked used and out of the queue, so it is no one's to take, map or wipe meanwhile; `give`'s caller unmapped it and every core forgot it first.
    zero(pa(f));
    crate::process::with_lock(|| reclaimed(pool(), f, false));
    true
}

/// Free frames kept known clean ahead of the takes (`clean_step`): 2^7, 256 MiB, past the largest arc a spawn or an arc's growth asks for at once (a GPU block of 128 MiB, cambench's 72 MiB).
const CLEAN_AHEAD: u32 = 1 << 7;
/// Whether `clean_step` may have work: cleared when the frames known clean reach `CLEAN_AHEAD` or none is left to zero, set again by a take of a clean frame (`took`), so an idle worker looks without the kernel lock while there is nothing to do.
static CLEAN_LOW: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// One free frame not known clean zeroed with the kernel lock let go, while fewer than `CLEAN_AHEAD` free frames are known clean: idle workers in place of their sleep (smp.rs), as the background wipe.
/// Why (the Pro, 2026-11-18, `diag lock`): a frame never taken this boot is zeroed by its take under the kernel lock, so a spawn's arc held it 12.3 ms at boot, an arc's growth 3.5 ms and a kist's launch 3.2 ms; with frames zeroed ahead the takes find clean ones (`take_first`) and zero nothing.
/// Not held or armed frames (the tripwire's), as `wipe_step`. False when there is nothing to do.
pub fn clean_step() -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    if !CLEAN_LOW.load(Relaxed) {
        return false;
    }
    let f = crate::process::with_lock(|| {
        let p = pool();
        let f = unclean_free(p).filter(|_| clean_free_of(p) < CLEAN_AHEAD as usize);
        if f.is_none() {
            CLEAN_LOW.store(false, Relaxed);
        }
        f.map(|f| hold_for_zeroing(p, f))
    });
    let Some(f) = f else { return false };
    zero(pa(f));
    crate::process::with_lock(|| {
        let p = pool();
        reclaimed(p, f, false);
    });
    true
}

/// Free frames known clean now (a take of one zeroes nothing). Under the kernel lock.
fn clean_free_of(p: &Pool) -> usize {
    p.used.iter().zip(p.clean.iter()).map(|(&u, &c)| (!u & c).count_ones() as usize).sum()
}

/// `clean_free_of` the pool, for a caller deciding whether a growth would zero frames under the kernel lock (`prepare`). Under the kernel lock.
pub fn clean_free() -> usize {
    clean_free_of(pool())
}

/// The first free frame neither clean nor armed (the tripwire's), below the pool's end; None when there is none.
fn unclean_free(p: &Pool) -> Option<usize> {
    let w = (0..WORDS).find(|&w| !(p.used[w] | p.clean[w] | p.armed[w]) != 0)?;
    Some(w * 64 + (!(p.used[w] | p.clean[w] | p.armed[w])).trailing_zeros() as usize).filter(|&f| f < p.total)
}

/// Free frame `f` out of the pool while it is zeroed with the kernel lock let go: marked used, so no take finds it and a reset's scrub zeroes it with the rest. `reclaimed` puts it back clean. Under the kernel lock.
fn hold_for_zeroing(p: &mut Pool, f: usize) -> usize {
    p.used[f / 64] |= 1 << (f % 64);
    p.free -= 1;
    f
}

/// Frames made ready for a take of `n` with the kernel lock let go (2026-11-22, kernlock): until `n` free frames are known clean, the frames given back are wiped (`reclaim_step`) and then free frames never taken are zeroed (as `clean_step`, past `CLEAN_AHEAD`), one at a time, each in two short holds of the lock with its 2 MiB zeroed between them.
/// Why: a take zeroes every frame not known clean under the lock, about 90 us a frame (the Pro, b1129: 138 ms for a 3 GiB staging arc, `diag lock`, while a camera streamed; a stream dies on a hold past its frame). Called first, the take that follows finds clean frames and zeroes none, its hold the bookkeeping and the mapping alone (about 1.2 us a frame: a GPU block's 224 frames in 277 us, clean).
/// Best effort, never a refusal: it stops when no frame is left to wipe or zero, and a take on another core meanwhile may use some of these frames, so the take that follows zeroes at most what was used meanwhile (as before, under the lock).
/// At most `n` frames wiped or zeroed here, so it ends even while other cores take and give back frames without a pause.
/// Call it without the kernel lock: the service loop as is, a syscall thru process.rs `unlocked`.
pub fn prepare(n: usize) {
    for _ in 0..n {
        let ready = crate::process::with_lock(|| clean_free_of(pool()) >= n);
        if ready {
            return;
        }
        if reclaim_step() {
            continue;
        }
        let f = crate::process::with_lock(|| {
            let p = pool();
            unclean_free(p).map(|f| hold_for_zeroing(p, f))
        });
        let Some(f) = f else { return };
        zero(pa(f));
        crate::process::with_lock(|| reclaimed(pool(), f, false));
    }
}

/// A free frame, zeroed, now taken; None when every frame is taken. The first free in the map's order, so the ranges ferros has used longest fill first.
pub fn take() -> Option<usize> {
    take_first().or_else(|| take_returning(true))
}

fn take_first() -> Option<usize> {
    let p = pool();
    // A frame known clean first (`clean_step` keeps some ready): its take zeroes nothing under the kernel lock. A clean frame is a real one (only `give` and the wipes mark one), so no bound is needed on it.
    if let Some(w) = p.used.iter().zip(p.clean.iter()).position(|(&u, &c)| !u & c != 0) {
        let f = w * 64 + (!p.used[w] & p.clean[w]).trailing_zeros() as usize;
        took(p, f, true);
        return Some(f);
    }
    let w = p.used.iter().position(|&w| w != u64::MAX)?;
    let f = w * 64 + (!p.used[w]).trailing_zeros() as usize;
    if f >= p.total {
        return None;
    }
    took(p, f, true);
    Some(f)
}

/// A free frame from the far end of the map, zeroed, now taken: a surface's (process.rs `surface_take`), so surfaces fill the pool from its top while arcs fill it from its bottom (`take`), and a big arc's growth and a run of windows never meet until the pool is nearly full. None when every frame is taken.
pub fn take_top() -> Option<usize> {
    take_top_first().or_else(|| take_returning(true))
}

fn take_top_first() -> Option<usize> {
    let p = pool();
    // A frame known clean first, from the top, as `take_first` from the bottom.
    if let Some(w) = (0..WORDS).rev().find(|&w| !p.used[w] & p.clean[w] != 0) {
        let f = w * 64 + 63 - (!p.used[w] & p.clean[w]).leading_zeros() as usize;
        took(p, f, true);
        return Some(f);
    }
    // The words from the last one that holds a frame down: bits past `total` in it are no frames, so they are counted taken here.
    let last = (p.total - 1) / 64;
    let past = if (p.total % 64) == 0 { 0 } else { u64::MAX << (p.total % 64) };
    let w = (0..=last).rev().find(|&w| (p.used[w] | if w == last { past } else { 0 }) != u64::MAX)?;
    let free = !(p.used[w] | if w == last { past } else { 0 });
    let f = w * 64 + 63 - free.leading_zeros() as usize;
    took(p, f, true);
    Some(f)
}

/// `n` free frames side by side in one range of the map, the run ending at or below the address `below`, now taken: one physically contiguous run, for a device that writes it by DMA with no IOMMU in front of it (the camera's front end, its SYSMMUs in bypass: process.rs `cam_ask`). Searched from the top of the map down, as surfaces and GPU blocks are taken (`take_top`), so the low ranges arcs grow into stay theirs.
/// Not zeroed here (2026-11-18): the caller zeroes them itself with the kernel lock let go (`wipe`), so the run's first frame comes with, frame by frame, whether it still needs zeroing (not known clean). Taken, so nothing else is given them meanwhile. None when no range holds such a run now.
pub fn take_run_unzeroed(n: usize, below: usize) -> Option<(usize, alloc::vec::Vec<bool>)> {
    take_run_search(n, below, false).or_else(|| if returning().is_empty() { None } else { take_run_search(n, below, true) })
}

/// Frame `f` zeroed by its own address (the kernel's), as a take zeroes it: for a caller holding it taken (`take_run_unzeroed`).
pub fn wipe(f: usize) {
    zero(pa(f));
}

/// The run's search (`take_run_unzeroed`). With `waiting`, frames waiting to come back count as free too: one in the run is taken out of the queue (still marked used) and reported as needing zeroing, which its taker does with the lock let go.
fn take_run_search(n: usize, below: usize, waiting: bool) -> Option<(usize, alloc::vec::Vec<bool>)> {
    let p = pool();
    let mut back = [0u64; WORDS];
    if waiting {
        for &f in returning().iter() {
            back[f / 64] |= 1 << (f % 64);
        }
    }
    let is_back = |f: usize| back[f / 64] >> (f % 64) & 1 == 1;
    let used = |p: &Pool, f: usize| p.used[f / 64] >> (f % 64) & 1 == 1 && !is_back(f);
    let mut starts = [0usize; 8];
    let ranges = map();
    let mut first = 0;
    for (k, &(lo, hi)) in ranges.iter().enumerate() {
        starts[k] = first;
        first += (hi - lo) / FRAME;
    }
    for (k, &(lo, hi)) in ranges.iter().enumerate().rev() {
        let count = (hi - lo) / FRAME;
        let mut run = 0;
        // From the range's top frame down: `run` free frames end at the one above `f`.
        for j in (0..count).rev() {
            let f = starts[k] + j;
            if lo + (j + 1) * FRAME > below || used(p, f) {
                run = 0;
                continue;
            }
            run += 1;
            if run == n {
                // Whether each still needs zeroing, read before `took` clears its clean mark (a waiting frame's was cleared at its first take).
                let dirty = (f..f + n).map(|g| p.clean[g / 64] >> (g % 64) & 1 == 0).collect();
                for g in f..f + n {
                    if !is_back(g) {
                        took(p, g, false);
                    }
                }
                if waiting {
                    returning().retain(|&g| g < f || g >= f + n);
                    RETURNING_N.store(returning().len(), core::sync::atomic::Ordering::Relaxed);
                }
                return Some((f, dirty));
            }
        }
    }
    None
}

/// Frame `f`'s lines cleaned and invalidated to the point of coherency (`dc civac`): what the kernel's own alias left in the caches (the zeroing's lines) written out and dropped, so a device that writes the frame by DMA, and a process that reads it thru a non-cacheable mapping, see DRAM and nothing a later eviction could overwrite.
pub fn clean_invalidate(f: usize) {
    let a = pa(f);
    let mut x = a;
    unsafe { core::arch::asm!("dsb sy") };
    while x < a + FRAME {
        unsafe { core::arch::asm!("dc civac, {x}", x = in(reg) x) };
        x += 64;
    }
    unsafe { core::arch::asm!("dsb sy") };
}

/// A free frame for a move's copy (process.rs `arc_move`), now taken and not zeroed first: the copy fills it whole.
pub fn take_for_copy() -> Option<usize> {
    take_for_copy_first().or_else(|| take_returning(false))
}

fn take_for_copy_first() -> Option<usize> {
    let p = pool();
    let w = p.used.iter().position(|&w| w != u64::MAX)?;
    let f = w * 64 + (!p.used[w]).trailing_zeros() as usize;
    if f >= p.total {
        return None;
    }
    took(p, f, false);
    Some(f)
}

/// Frame `f` back to the pool (owner, 2026-11-08: the kernel wipes what a process held as it reclaims it): its table emptied now, the frame queued to be wiped and freed with the kernel lock let go (`RETURNING`, since 2026-11-18). Only once no window maps it and every core's TLB has forgotten it (`cachemap::unmapped`). Under the kernel lock.
pub fn give(f: usize) {
    unsafe { core::ptr::write_bytes(table(f) as *mut u8, 0, TABLE) };
    returning().push(f);
    RETURNING_N.store(returning().len(), core::sync::atomic::Ordering::Relaxed);
}

/// Zero a frame by its own address (Normal, the kernel's: cachemap `frame_block`), 64 bytes at a time with `dc zva` (the block DCZID_EL0 gives on these cores, as platform::scrub_memory steps).
fn zero(a: usize) {
    let mut x = a;
    while x < a + FRAME {
        unsafe { core::arch::asm!("dc zva, {x}", x = in(reg) x) };
        x += 64;
    }
}

/// Every frame a process holds, zeroed and cleaned to DRAM before a reset (platform::scrub_memory, every other core stopped): the tables hold no process's bytes, and neither does a free frame: one given back was wiped (`give`) or is still marked used until it is (`RETURNING`), and so zeroed here, one never taken this boot holds what DRAM held before it (after a jump, Android's, until the background wipe passes it; held and armed frames likewise), never a ferros process's, since every take zeroes first. A reset before the wipe is done so leaves only Android's leftovers in the never-taken frames, which the power cycle that ends a jumped session decays (HV.md). Returns the bytes zeroed.
pub fn scrub() -> usize {
    let p = pool();
    let mut n = 0;
    for f in p.table_frames..p.total {
        if (p.used[f / 64] & !p.held[f / 64]) & 1 << (f % 64) != 0 {
            let a = pa(f);
            let mut x = a;
            while x < a + FRAME {
                unsafe { core::arch::asm!("dc zva, {x}", "dc cvac, {x}", x = in(reg) x) };
                x += 64;
            }
            n += FRAME;
        }
    }
    n
}

/// `diag frames test`, one frame: frame `f` written with its own addresses (each word its address, inverted in odd frames), read back, then wiped and marked clean, if it is free; None for a frame an arc or the tables hold, or past the pool. Some(how many words were wrong, the first wrong word's address). Under the kernel lock, a frame a hold (service.rs `diag frames test` takes 256 frames a call).
/// The proof a new range is memory ferros reaches end to end (akita's high bank: memblock says nobody owns it, this says the kernel's stores land and read back).
pub fn test_frame(f: usize) -> Option<(usize, usize)> {
    let p = pool();
    if f < p.table_frames || f >= p.total || p.used[f / 64] & 1 << (f % 64) != 0 {
        return None;
    }
    let a = pa(f);
    let flip = if f & 1 == 1 { u64::MAX } else { 0 };
    let words = unsafe { core::slice::from_raw_parts_mut(a as *mut u64, FRAME / 8) };
    for (k, w) in words.iter_mut().enumerate() {
        unsafe { core::ptr::write_volatile(w, (a + k * 8) as u64 ^ flip) };
    }
    let (mut wrong, mut first) = (0, 0);
    for (k, w) in words.iter().enumerate() {
        if unsafe { core::ptr::read_volatile(w) } != (a + k * 8) as u64 ^ flip {
            if wrong == 0 {
                first = a + k * 8;
            }
            wrong += 1;
        }
    }
    zero(a);
    p.clean[f / 64] |= 1 << (f % 64);
    Some((wrong, first))
}

/// The bytes the map holds, from the map alone (no lock: a sum over constants): the most any arc could ever be given on this board, for a ceiling that only sizes a window of addresses (process.rs `image_tail_ceiling`).
pub fn map_bytes() -> usize {
    map().iter().map(|&(lo, hi)| hi - lo).sum()
}

/// Every frame the map holds, the tables' too (`test_frame`'s range).
pub fn total() -> usize {
    pool().total
}

/// `diag frames`'s second half: how the pool stands.
pub fn pool_report() -> alloc::string::String {
    let p = pool();
    let armed: u32 = p.armed.iter().map(|w| w.count_ones()).sum();
    alloc::format!(
        "pool: A#{} frames, A#{} free, A#{} coming back (given back, to be wiped), A#{} taken by arcs, A#{} holding the tables, A#{} held (after a jump), A#{armed} armed by the tripwire\n",
        p.total - p.table_frames,
        p.free,
        returning().len(),
        p.total - p.table_frames - p.held_n - p.free - returning().len(),
        p.table_frames,
        p.held_n
    )
}

/// `diag frames release`: every held frame given to the pool (free, not clean: a take zeroes it, the background wipe zeroes it first). Under the kernel lock. Returns how many.
pub fn release_held() -> usize {
    let p = pool();
    let n = p.held_n;
    for w in 0..WORDS {
        p.used[w] &= !p.held[w];
        p.held[w] = 0;
    }
    p.free += n;
    p.held_n = 0;
    // Free and not clean: frames for `clean_step` to zero ahead.
    CLEAN_LOW.store(true, core::sync::atomic::Ordering::Relaxed);
    n
}

// ---------------------------------------------------------------- the background wipe

/// The background wipe (owner, 2026-11-15: "zero the RAM"): after a jump from Android, every frame no arc has taken still holds what Android left in DRAM.
/// Once a person has attested (the deck is up: service.rs), idle workers zero those frames one at a time (`wipe_step`, in place of their sleep: no timer, no core woken for it), so Android's data does not sit in DRAM for the session.
/// It skips held and armed frames, and never runs on a cold boot (its never-taken frames hold no session's data: a warm reset's scrub zeroed what arcs held).
static WIPE_GO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static WIPE_DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The next frame the wipe looks at, frames wiped, the counter at the first and at the last, under the kernel lock.
static mut WIPE: (usize, usize, u64, u64) = (0, 0, 0, 0);

/// The wipe allowed from now (the service loop, once a person is in, after a jump only).
pub fn wipe_allow() {
    if crate::platform::jumped() {
        WIPE_GO.store(true, core::sync::atomic::Ordering::Release);
    }
}

/// One frame of the wipe, on an idle worker: the next free frame that is not clean taken out of the pool under the lock, zeroed and cleaned to DRAM outside it (2 MiB, about a tenth of a millisecond, so work queued meanwhile waits at most that), then given back clean. False when there is nothing to do (not allowed yet, or done).
pub fn wipe_step() -> bool {
    use core::sync::atomic::Ordering::Acquire;
    if !WIPE_GO.load(Acquire) || WIPE_DONE.load(Acquire) {
        return false;
    }
    let f = crate::process::with_lock(|| {
        let p = pool();
        let w = unsafe { &mut *(&raw mut WIPE) };
        if w.0 < p.table_frames {
            w.0 = p.table_frames;
            w.2 = crate::platform::cntpct();
        }
        while w.0 < p.total {
            let f = w.0;
            w.0 += 1;
            let (i, b) = (f / 64, 1u64 << (f % 64));
            if (p.used[i] | p.clean[i] | p.armed[i]) & b == 0 {
                p.used[i] |= b;
                p.free -= 1;
                return Some(f);
            }
        }
        w.3 = crate::platform::cntpct();
        WIPE_DONE.store(true, core::sync::atomic::Ordering::Release);
        None
    });
    let Some(f) = f else {
        return false;
    };
    let a = pa(f);
    let mut x = a;
    while x < a + FRAME {
        unsafe { core::arch::asm!("dc zva, {x}", "dc cvac, {x}", x = in(reg) x) };
        x += 64;
    }
    unsafe { core::arch::asm!("dsb sy") };
    crate::process::with_lock(|| {
        let p = pool();
        let (i, b) = (f / 64, 1u64 << (f % 64));
        p.used[i] &= !b;
        p.clean[i] |= b;
        p.free += 1;
        unsafe { (*(&raw mut WIPE)).1 += 1 };
    });
    true
}

/// The wipe's line for `diag frames` and the flash log: not started, under way, or done with its frames and time.
pub fn wipe_report() -> alloc::string::String {
    let (next, n, t0, t1) = unsafe { *(&raw const WIPE) };
    let f = crate::platform::cntfrq();
    if !WIPE_GO.load(core::sync::atomic::Ordering::Acquire) {
        alloc::string::String::from("wipe: not started (after a jump, once a person is in)\n")
    } else if WIPE_DONE.load(core::sync::atomic::Ordering::Acquire) {
        alloc::format!("wipe: done, A#{n} frames (A#{} MiB) zeroed in A#{} ms on idle workers\n", n * 2, t1.wrapping_sub(t0) * 1000 / f)
    } else {
        alloc::format!("wipe: under way, A#{n} frames zeroed, at frame A#{next}\n")
    }
}

/// Whether the wipe has finished (the service loop logs it once).
pub fn wipe_done() -> bool {
    WIPE_DONE.load(core::sync::atomic::Ordering::Acquire)
}

// ---------------------------------------------------------------- the tripwire

/// The tripwire (`diag frames trip`, 2026-11-15): every free or held frame written with a pattern, then read back on command, so a bus master writing anywhere in them is caught with its address, its extent and what it wrote.
/// The pattern: each 8-byte word holds its own address XOR this salt, so a changed word names itself, and a word copied from elsewhere in the pattern still names where it came from (the word XOR the salt).
/// The salt's top bits are no physical address (the SoC decodes 36 bits), so neither zeros nor a pointer a device writes can pass for the pattern.
pub const TRIP_SALT: u64 = 0x7472_6970_0000_0000;

/// Lines in a frame.
const LINES: usize = FRAME / 64;

/// One run of changed lines a check found: `[start, end)`, the words changed in it, and the first changed line's address and its eight words as read.
pub struct TripRun {
    pub start: usize,
    pub end: usize,
    pub words: usize,
    pub line: usize,
    pub sample: [u64; 8],
}

/// Frame `f`'s lines written with the pattern and cleaned to DRAM (the frame is the kernel's by its own address: cachemap `frame_block`, Normal write-back), then armed and marked not clean (a take zeroes it). Only a frame no arc holds (free or held); false for any other.
/// Under the kernel lock, a frame a hold (service.rs `diag frames trip arm` takes 256 a call).
pub fn trip_arm(f: usize) -> bool {
    let p = pool();
    let (w, b) = (f / 64, 1u64 << (f % 64));
    if f < p.table_frames || f >= p.total || p.used[w] & !p.held[w] & b != 0 {
        return false;
    }
    let a = pa(f);
    let words = unsafe { core::slice::from_raw_parts_mut(a as *mut u64, FRAME / 8) };
    for (k, x) in words.iter_mut().enumerate() {
        *x = (a + k * 8) as u64 ^ TRIP_SALT;
    }
    clean_lines(a, LINES);
    p.armed[w] |= b;
    p.clean[w] &= !b;
    true
}

/// Frame `f` read back from DRAM against the pattern if it is armed: every changed line added to `runs` (merged with the run before when it continues it; past `most` runs only counted in `more`) and armed again, so the next check reports only what was written since this one. None when the frame is not armed; else the words changed.
/// Each line is invalidated first (`dc civac`: armed lines are clean, so nothing is written back), so the reads see DRAM and not a line the cache kept from the arm or fetched since.
/// Under the kernel lock, a frame a hold.
pub fn trip_check(f: usize, runs: &mut alloc::vec::Vec<TripRun>, most: usize, more: &mut usize) -> Option<usize> {
    let p = pool();
    if f >= p.total || p.armed[f / 64] & 1 << (f % 64) == 0 {
        return None;
    }
    let a = pa(f);
    unsafe { core::arch::asm!("dsb sy") };
    for l in 0..LINES {
        unsafe { core::arch::asm!("dc civac, {x}", x = in(reg) a + l * 64) };
    }
    unsafe { core::arch::asm!("dsb sy") };
    let mut changed = 0;
    for l in 0..LINES {
        let line = a + l * 64;
        let mut got = [0u64; 8];
        let mut n = 0;
        for (k, g) in got.iter_mut().enumerate() {
            *g = unsafe { core::ptr::read_volatile((line + k * 8) as *const u64) };
            n += (*g != (line + k * 8) as u64 ^ TRIP_SALT) as usize;
        }
        if n == 0 {
            continue;
        }
        changed += n;
        let joins = runs.last().is_some_and(|r| r.end == line);
        if joins {
            let r = runs.last_mut().unwrap();
            r.end += 64;
            r.words += n;
        } else if runs.len() < most {
            runs.push(TripRun { start: line, end: line + 64, words: n, line, sample: got });
        } else {
            *more += 1;
        }
        for k in 0..8 {
            unsafe { core::ptr::write_volatile((line + k * 8) as *mut u64, (line + k * 8) as u64 ^ TRIP_SALT) };
        }
        clean_lines(line, 1);
    }
    Some(changed)
}

/// Frame `f` out of the tripwire: zeroed, cleaned to DRAM and marked clean, if it is armed. Under the kernel lock, a frame a hold.
pub fn trip_off(f: usize) -> bool {
    let p = pool();
    let (w, b) = (f / 64, 1u64 << (f % 64));
    if f >= p.total || p.armed[w] & b == 0 {
        return false;
    }
    let a = pa(f);
    let mut x = a;
    while x < a + FRAME {
        unsafe { core::arch::asm!("dc zva, {x}", "dc cvac, {x}", x = in(reg) x) };
        x += 64;
    }
    unsafe { core::arch::asm!("dsb sy") };
    p.armed[w] &= !b;
    p.clean[w] |= b;
    true
}

/// `n` lines from `a` cleaned to the point of coherency (`dc cvac`), then a barrier: what the CPU wrote is in DRAM, where a device's write would land.
fn clean_lines(a: usize, n: usize) {
    for l in 0..n {
        unsafe { core::arch::asm!("dc cvac, {x}", x = in(reg) a + l * 64) };
    }
    unsafe { core::arch::asm!("dsb sy") };
}
