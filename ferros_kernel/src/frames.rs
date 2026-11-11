//! The frame pool's memory map (ARCS.md step 0, owner 2026-11-11: "2MB frame size across the board is totally fine by me"): every DRAM range a process's arc may take its 2 MiB frames from, per board, written once here and in docs/BOARDS.md "Memory map".
//!
//! A range is listed only with memblock's proof that nothing else owns it (memory feedback_reservation_proof_is_memblock: a device tree node proves nothing, memblock or /proc/iomem does), or because ferros has run processes there since before this map (the two arc pools it replaces).
//! The order is the order frames are taken in: the ranges ferros has used longest first, so the new ones fill only when they are needed.
//!
//! husky's proof is Android's own boot log of 2026-08-17 (/mnt/Harbor/tmp/linux_full_dmesg.txt, stock 6.1.124, "Machine model: ZUMA HUSKY MP"): memblock's memory ("Early memory node ranges": G#8000_0000-G#FFFF_FFFF and G#8_8000_0000-G#A_FFFF_FFFF, 12 GiB) and every reservation it made from the device tree ("OF: reserved mem", each with the size memblock was given, so a size-0 node like the hv's of 2026-10-17 would show).
//! In the high bank those reservations are one run, G#9_BE40_0000-G#9_FFFF_FFFF (vscaler, farawimg, famodel, faceauth_dsp, faceauth_tpu, vstream, faprev, vframe, gcma_camera), and the hv's 64 MiB at the top (G#A_FC00_0000, /proc/iomem "reserved" since 2026-10-19, HV.md).
//! The 2 MiB frame under the hv is left out too: the hv copies the device tree it hands Linux to G#A_FBF0_0000 (ferros_hv DTB_COPY).
//! akita has no memblock capture in the repo yet: only the two pools it has run processes from (BOARDS.md "Memory map" has the capture that would prove its high bank).

/// A frame: 2 MiB, the unit every arc is made of (ARCS.md).
pub const FRAME: usize = 1 << 21;

/// husky's frame ranges, [start, end), each 2 MiB aligned.
#[cfg(not(shim_handoff))]
const HUSKY: [(usize, usize); 4] = [
    // The page pool (cachemap POOL): the window's top 16 MiB, ferros's own since 2026-10-26; memblock: plain memory (G#8000_0000-G#8AFF_FFFF, no reservation).
    crate::cachemap::POOL,
    // The big pool (cachemap POOL_BIG): 320 MiB under gpu-mem; memblock: plain memory (G#9740_0000-G#B61F_FFFF, no reservation; sec_dram begins at G#B620_0000).
    crate::cachemap::POOL_BIG,
    // The high bank's first 4.97 GiB, up to the camera and video pools.
    (0x8_8000_0000, 0x9_BE40_0000),
    // The high bank past those pools, up to the frame under the hv.
    (0xA_0000_0000, 0xA_FA00_0000),
];

/// akita's frame ranges: the two pools ferros has run its processes from (BOARDS.md "Memory map": its high bank waits for a memblock capture).
#[cfg(not(shim_handoff))]
const AKITA: [(usize, usize); 2] = [crate::cachemap::POOL, crate::cachemap::POOL_BIG];

/// The handoff path (oracle only): its 2 MiB pool.
#[cfg(shim_handoff)]
const HUSKY: [(usize, usize); 1] = [crate::cachemap::POOL];
#[cfg(shim_handoff)]
const AKITA: [(usize, usize); 1] = [crate::cachemap::POOL];

/// This board's frame ranges. A `FERROS_FRAMES_HIGH=0` build keeps husky to the two old pools as well: the escape if its high bank ever misbehaves, as `FERROS_X3=0` is the X3's.
pub fn map() -> &'static [(usize, usize)] {
    if crate::board::akita() || option_env!("FERROS_FRAMES_HIGH") == Some("0") { &AKITA } else { &HUSKY }
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

/// The frames, by index across the map's ranges in order: which are taken, which are known to hold zeros (every frame given back is wiped; a frame not yet taken this boot holds whatever DRAM held), and how many of the first frames hold the level-3 tables (frame k's is table k % 512 of frame k / 512; never handed out, and not one run: husky's ten spill from the page pool's range into the big pool's).
struct Pool {
    used: [u64; WORDS],
    clean: [u64; WORDS],
    total: usize,
    free: usize,
    table_frames: usize,
    init: bool,
}

/// Every access is under the kernel lock (process.rs `lock`), as the arcs that hold these frames are; the scrub before a reset reads it with every other core stopped.
static mut POOL: Pool = Pool { used: [0; WORDS], clean: [0; WORDS], total: 0, free: 0, table_frames: 0, init: false };

fn pool() -> &'static mut Pool {
    let p = unsafe { &mut *(&raw mut POOL) };
    if !p.init {
        p.init = true;
        p.total = map().iter().map(|&(lo, hi)| (hi - lo) / FRAME).sum();
        assert!(p.total <= MAX_FRAMES, "frames: the map holds more frames than the pool counts");
        // The tables first, in the first frames: one table per frame, those holding tables included (never mapped, so their own tables are spare).
        p.table_frames = (p.total * TABLE).div_ceil(FRAME);
        for f in 0..p.table_frames {
            p.used[f / 64] |= 1 << (f % 64);
            zero(pa(f));
        }
        p.free = p.total - p.table_frames;
    }
    p
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
    pool();
    pa(f / TABLES_PER_FRAME) + (f % TABLES_PER_FRAME) * TABLE
}

/// The frame a level-3 table belongs to (`cachemap::slot_table` gives the table of a mapped slot).
pub fn of_table(t: usize) -> usize {
    let p = pool();
    let j = (0..p.table_frames).find(|&j| t >= pa(j) && t < pa(j) + FRAME).expect("frames: not a frame's table");
    j * TABLES_PER_FRAME + (t - pa(j)) / TABLE
}

/// Frames free now.
pub fn free() -> usize {
    pool().free
}

/// The bytes arcs hold now (the scrub's count before a reset).
pub fn taken_bytes() -> usize {
    let p = pool();
    (p.total - p.table_frames - p.free) * FRAME
}

/// A free frame, zeroed, now taken; None when every frame is taken. The first free in the map's order, so the ranges ferros has used longest fill first.
pub fn take() -> Option<usize> {
    let p = pool();
    let w = p.used.iter().position(|&w| w != u64::MAX)?;
    let f = w * 64 + (!p.used[w]).trailing_zeros() as usize;
    if f >= p.total {
        return None;
    }
    p.used[w] |= 1 << (f % 64);
    p.free -= 1;
    if p.clean[w] & 1 << (f % 64) == 0 {
        zero(pa(f));
    }
    p.clean[w] &= !(1 << (f % 64));
    Some(f)
}

/// A free frame for a move's copy (process.rs `arc_move`), now taken and not zeroed first: the copy fills it whole.
pub fn take_for_copy() -> Option<usize> {
    let p = pool();
    let w = p.used.iter().position(|&w| w != u64::MAX)?;
    let f = w * 64 + (!p.used[w]).trailing_zeros() as usize;
    if f >= p.total {
        return None;
    }
    p.used[w] |= 1 << (f % 64);
    p.clean[w] &= !(1 << (f % 64));
    p.free -= 1;
    Some(f)
}

/// Frame `f` back to the pool, wiped (owner, 2026-11-08: the kernel wipes what a process held as it reclaims it), its table too. Only once no window maps it and every core's TLB has forgotten it (`cachemap::unmapped`).
pub fn give(f: usize) {
    let p = pool();
    zero(pa(f));
    unsafe { core::ptr::write_bytes(table(f) as *mut u8, 0, TABLE) };
    p.used[f / 64] &= !(1 << (f % 64));
    p.clean[f / 64] |= 1 << (f % 64);
    p.free += 1;
}

/// Zero a frame by its own address (Normal, the kernel's: cachemap `frame_block`), 64 bytes at a time with `dc zva` (the block DCZID_EL0 gives on these cores, as platform::scrub_memory steps).
fn zero(a: usize) {
    let mut x = a;
    while x < a + FRAME {
        unsafe { core::arch::asm!("dc zva, {x}", x = in(reg) x) };
        x += 64;
    }
}

/// Every frame a process holds, zeroed and cleaned to DRAM before a reset (platform::scrub_memory, every other core stopped): free frames are wiped already, and the tables hold no process's bytes. Returns the bytes zeroed.
pub fn scrub() -> usize {
    let p = pool();
    let mut n = 0;
    for f in p.table_frames..p.total {
        if p.used[f / 64] & 1 << (f % 64) != 0 {
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

/// `diag frames`'s second half: how the pool stands.
pub fn pool_report() -> alloc::string::String {
    let p = pool();
    alloc::format!("pool: A#{} frames, A#{} free, A#{} taken by arcs, A#{} holding the tables\n", p.total - p.table_frames, p.free, p.total - p.table_frames - p.free, p.table_frames)
}
