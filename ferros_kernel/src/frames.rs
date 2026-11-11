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

/// This board's frame ranges.
pub fn map() -> &'static [(usize, usize)] {
    if crate::board::akita() { &AKITA } else { &HUSKY }
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
