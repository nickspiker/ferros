//! The front end's PDMA job, moved into the camera's own region (docs/CAMERA.md "The driver"; owner, 2026-11-14: "Can we get Lumis wired up?").
//!
//! The job is the 'CPR3' image tools/camera-pdma-image.py built in October for fixed physical addresses: the descriptor ring at G#8800_0000, the register-write program at G#8801_0000, its twelve block-load sources from G#8802_0000, and one 16 MiB output slice per output address from G#A000_0000 (slot k at G#A000_0000 + k x 16 MiB, its offset in the captured IOVA's 16 MiB granule kept).
//! Those addresses are other things' memory now (the frame pool's ranges, the GPU's memory, the secure carveout: docs/CAMERA.md "Where it stands"), so the driver lays the job out in a region of frames the kernel gave it, and this moves every address the engine will follow there: the ring's list address, each block load's source, and the 25 address registers by name (with their high halves, where the program writes them as zero, for a region past 4 GiB).
//! The front end's SYSMMUs stay in bypass, so a bus address is a physical address; the region is one run of frames side by side (frames.rs `take_run`), and the camera's S2MPUs admit those frames and nothing else (s2mpu.rs `fence_cam`).
//!
//! Format: 'CPR3', u32 piece count, then per piece {u32 physical address, u32 length, the bytes padded to 4}.

/// Where the October job lived: its pieces in the 2 MiB from G#8800_0000.
pub const OLD_JOB: u64 = 0x8800_0000;
/// One frame of the pool (frames.rs): the job's pieces all fit in the region's first.
pub const FRAME: u64 = 1 << 21;
/// The October output slices: slot k at `OLD_SLICES + k * OLD_SLICE`.
pub const OLD_SLICES: u64 = 0xA000_0000;
pub const OLD_SLICE: u64 = 1 << 24;
pub const SLOTS: usize = 32;
/// The register program's addresses are offsets into the front end (tools/oracle-decode-pdma.py).
pub const ISPFE: u32 = 0x1C30_0000;
/// The ring's descriptors: {flags G#C003, list address low, list address high, length}.
pub const RING_ENTRY: usize = 16;
/// Where the program sits inside the job's frame (the ring before it).
pub const LIST_OFFSET: u64 = 0x1_0000;

/// The front end's address registers, all 25 (tools/camera-pdma-image.py `ADDR_ALL`: the main camera's list and the hv's live Linux snapshot, 2026-10-22).
/// Each has its high half at +4, written as zero by the program (read from the main camera's list, 2026-11-14: 24 of the 25; G#1C35_4ECC's +4 holds G#C000_0000, so it is something else there).
pub const ADDR_REGS: [u32; 25] = [
    0x1C32_0510, 0x1C32_05C0, 0x1C32_2110, 0x1C32_21C0, 0x1C34_00D0, 0x1C35_1820, 0x1C35_48A0, 0x1C35_4A84, 0x1C35_4ECC, 0x1C35_4FC8,
    0x1C35_5168, 0x1C35_5178, 0x1C35_5180, 0x1C35_5188, 0x1C35_5190, 0x1C35_5198, 0x1C35_51A0, 0x1C35_51A8, 0x1C35_51B0, 0x1C35_51C0,
    0x1C35_51C8, 0x1C35_51E0, 0x1C35_51E8, 0x1C35_51F8, 0x1C35_5200,
];
/// The image body's register (loch0's image path) and the PD path's: where the picture and the phase-detect rows land (docs/oracle/CAMERA.md "First frame").
pub const IMAGE_REG: u32 = 0x1C32_05C0;
pub const PD_REG: u32 = 0x1C32_21C0;
/// The October slots the image body and the PD path were given (tools/camera-pdma-image.py pinned the image to slot 2 for every camera; the PD path took slot 0).
pub const OLD_SLOT_IMAGE: usize = 2;
pub const OLD_SLOT_PD: usize = 0;

const KIB: u64 = 1 << 10;
const MIB: u64 = 1 << 20;

/// The main camera's outputs: the bytes each October slot gets in the region, 0 for a slot the job never names.
/// Measured on the Pro (target/cam/run-rate.txt, 2026-10-21: every slice patterned, sampled every 256 bytes after a ten-frame run): slots 0 to 21 are written and 22 to 31 never are; each size is the power of two at or past twice the last byte seen written, except the two frames, which get exactly theirs.
/// 0 the PD path (4080 x 768 RAW10, 3.75 MiB); 1, 3, 7 and 20 a 16-byte header each; 2 the image body (4080 x 3072 RAW10, 15 MiB); 4 to 6, 8 to 14, 17 to 19, 21 statistics of 6 KiB to 1.05 MiB; 15 and 16 written to the end of their 16 MiB and to 8 MiB, with data of one kind, so 15 may run on into 16 as it did in October: both keep 16 MiB, side by side in that order.
pub const MAIN_SLOT_BYTES: [u64; SLOTS] = [
    4 * MIB, 4 * KIB, 16 * MIB, 4 * KIB, 16 * KIB, 64 * KIB, 4 * MIB, 4 * KIB, 64 * KIB, MIB, MIB, 32 * KIB, 32 * KIB, 32 * KIB, MIB, 16 * MIB,
    16 * MIB, MIB, 4 * MIB, 2 * MIB, 4 * KIB, MIB, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Where each October slot lands in the region: (offset, bytes), (0, 0) for none; and the region's whole size, in frames.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub slots: [(u64, u64); SLOTS],
    pub bytes: u64,
}

/// The job's frame first, then each slot in order, each aligned to its own size up to a frame (sizes are powers of two), the whole rounded up to frames.
pub const fn layout(sizes: &[u64; SLOTS]) -> Layout {
    let mut slots = [(0u64, 0u64); SLOTS];
    let mut off = FRAME;
    let mut k = 0;
    while k < SLOTS {
        let s = sizes[k];
        if s != 0 {
            let align = if s < FRAME { s } else { FRAME };
            off = (off + align - 1) & !(align - 1);
            slots[k] = (off, s);
            off += s;
        }
        k += 1;
    }
    Layout { slots, bytes: (off + FRAME - 1) & !(FRAME - 1) }
}

/// The main camera's region.
pub const MAIN: Layout = layout(&MAIN_SLOT_BYTES);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Error {
    /// Not a 'CPR3'.
    NotJob,
    /// A piece or a program record runs past the end.
    Truncated,
    /// More pieces than `PIECES`.
    TooManyPieces,
    /// A piece outside the October job's first frame.
    PieceOutside(u64),
    /// No ring at G#8800_0000, or no program at G#8801_0000.
    NoRing,
    NoList,
    /// An address the engine would follow that is in no slot this layout gives room to (or past the room it gives): the job is not the one the layout was measured on.
    Unplaced(u64),
    /// A block load whose high half is not zero: not the record this was decoded from.
    LoadHigh(u32),
    /// More address-register writes than the 25 registers once each, with room to spare: not the job this was built for.
    TooManyAddresses,
}

/// The most pieces a job holds: the ring, the program and its block-load sources (the main camera's has 14).
pub const PIECES: usize = 1 << 6;

/// A job's pieces: (October physical address, where its bytes start in the image, their length).
pub struct Pieces {
    pub list: [(u64, usize, usize); PIECES],
    pub n: usize,
}

impl Pieces {
    pub fn iter(&self) -> impl Iterator<Item = &(u64, usize, usize)> {
        self.list[..self.n].iter()
    }
}

fn w32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn put32(b: &mut [u8], i: usize, v: u32) {
    b[i..i + 4].copy_from_slice(&v.to_le_bytes());
}

pub fn pieces(job: &[u8]) -> Result<Pieces, Error> {
    if job.len() < 8 || &job[..4] != b"CPR3" {
        return Err(Error::NotJob);
    }
    let n = w32(job, 4) as usize;
    if n > PIECES {
        return Err(Error::TooManyPieces);
    }
    let mut p = Pieces { list: [(0, 0, 0); PIECES], n };
    let mut at = 8;
    for k in 0..n {
        if at + 8 > job.len() {
            return Err(Error::Truncated);
        }
        let (pa, len) = (w32(job, at) as u64, w32(job, at + 4) as usize);
        at += 8;
        if at + len > job.len() {
            return Err(Error::Truncated);
        }
        if pa < OLD_JOB || pa + len as u64 > OLD_JOB + FRAME {
            return Err(Error::PieceOutside(pa));
        }
        p.list[k] = (pa, at, len);
        at += (len + 3) & !3;
    }
    Ok(p)
}

/// What the move did, for the driver's console line: address registers moved, their high halves written, high halves the program does not hold as zero (left as captured; they matter only for a region past 4 GiB), block loads moved, ring descriptors moved, and which October slots the job names (bit k).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Moved {
    pub addr_regs: u32,
    pub highs: u32,
    pub highs_left: u32,
    pub loads: u32,
    pub ring: u32,
    pub slots: u32,
}

/// An October address, moved into a region at physical `base` laid out as `l`.
pub fn moved(old: u64, base: u64, l: &Layout) -> Result<u64, Error> {
    if (OLD_JOB..OLD_JOB + FRAME).contains(&old) {
        return Ok(base + (old - OLD_JOB));
    }
    if (OLD_SLICES..OLD_SLICES + SLOTS as u64 * OLD_SLICE).contains(&old) {
        let k = ((old - OLD_SLICES) / OLD_SLICE) as usize;
        let off = old % OLD_SLICE;
        let (at, size) = l.slots[k];
        if size == 0 || off >= size {
            return Err(Error::Unplaced(old));
        }
        return Ok(base + at + off);
    }
    Err(Error::Unplaced(old))
}

/// The October slot an address register's captured value names, if any.
pub fn slot_of(old: u64) -> Option<usize> {
    (OLD_SLICES..OLD_SLICES + SLOTS as u64 * OLD_SLICE).contains(&old).then(|| ((old - OLD_SLICES) / OLD_SLICE) as usize)
}

/// The register a program word writes, for each (word index, register) of the list: op 2 is (register, value) pairs, op 1 a burst of consecutive registers from a start (docs/oracle/CAMERA.md "The front end's register-write list, decoded").
/// `f` is called with (the value's word index, its register); it returns nothing. Op 9 records and markers are skipped here.
fn each_write(words: usize, w: &dyn Fn(usize) -> u32, f: &mut dyn FnMut(usize, u32)) -> Result<(), Error> {
    let mut i = 0;
    while i < words {
        let h = w(i);
        if h == 0 {
            break;
        }
        let (cnt, op) = ((h >> 14) as usize, h & 0x3FFF);
        if i + 1 + cnt > words {
            return Err(Error::Truncated);
        }
        match op {
            2 => {
                let mut k = i + 1;
                while k + 1 <= i + cnt {
                    f(k + 1, ISPFE.wrapping_add(w(k)));
                    k += 2;
                }
            }
            1 if cnt > 0 => {
                let start = ISPFE.wrapping_add(w(i + 1));
                for j in 0..cnt - 1 {
                    f(i + 2 + j, start.wrapping_add(4 * j as u32));
                }
            }
            _ => {}
        }
        i += 1 + cnt;
    }
    Ok(())
}

/// Move a job (its image, in place) into a region at physical `base` laid out as `l`: the ring's list addresses, the block loads' sources, the address registers and their high halves. The pieces then go into the region at `offset(pa) = pa - OLD_JOB` (`pieces`).
pub fn relocate(job: &mut [u8], base: u64, l: &Layout) -> Result<Moved, Error> {
    let p = pieces(job)?;
    let ring = p.iter().find(|x| x.0 == OLD_JOB).copied().ok_or(Error::NoRing)?;
    let list = p.iter().find(|x| x.0 == OLD_JOB + LIST_OFFSET).copied().ok_or(Error::NoList)?;
    let mut m = Moved::default();
    // The ring: every descriptor's list address.
    for e in 0..ring.2 / RING_ENTRY {
        let at = ring.1 + e * RING_ENTRY;
        let old = w32(job, at + 4) as u64 | (w32(job, at + 8) as u64) << 32;
        let new = moved(old, base, l)?;
        put32(job, at + 4, new as u32);
        put32(job, at + 8, (new >> 32) as u32);
        m.ring += 1;
    }
    let words = list.2 / 4;
    let base_at = list.1;
    // The address registers, and the high half each needs (the register at +4, from its moved address).
    // WHY the bounds on these tables: a program names each of the 25 address registers once (the main camera's does); a program with more address writes than a table holds is refused as not the job this was built for.
    let mut writes = [(0usize, 0u32, 0u64); 32];
    let mut nw = 0;
    let mut over = false;
    {
        let snap: &[u8] = &job[base_at..base_at + words * 4];
        each_write(words, &|i| w32(snap, i * 4), &mut |i, reg| {
            let v = w32(snap, i * 4);
            if ADDR_REGS.contains(&reg) && v != 0 {
                if nw < writes.len() {
                    writes[nw] = (i, reg, v as u64);
                    nw += 1;
                } else {
                    over = true;
                }
            }
        })?;
    }
    if over {
        return Err(Error::TooManyAddresses);
    }
    let mut highs = [(0u32, 0u32); 32];
    for (k, &(i, reg, old)) in writes[..nw].iter().enumerate() {
        let new = moved(old, base, l)?;
        if let Some(s) = slot_of(old) {
            m.slots |= 1 << s;
        }
        highs[k] = (reg + 4, (new >> 32) as u32);
        put32(job, base_at + i * 4, new as u32);
        m.addr_regs += 1;
    }
    let mut his = [(0usize, 0u32, 0u32); 32];
    let mut n = 0;
    {
        let snap: &[u8] = &job[base_at..base_at + words * 4];
        each_write(words, &|i| w32(snap, i * 4), &mut |i, reg| {
            if let Some(&(_, hi)) = highs[..nw].iter().find(|h| h.0 == reg) {
                if n < his.len() {
                    his[n] = (i, hi, w32(snap, i * 4));
                    n += 1;
                }
            }
        })?;
    }
    for &(i, hi, captured) in &his[..n] {
        if captured == 0 {
            put32(job, base_at + i * 4, hi);
            m.highs += 1;
        } else if hi != 0 {
            m.highs_left += 1;
        }
    }
    // The block loads: op 9, {source low, source high, length, register}.
    let mut i = 0;
    while i < words {
        let h = w32(job, base_at + i * 4);
        if h == 0 {
            break;
        }
        let (cnt, op) = ((h >> 14) as usize, h & 0x3FFF);
        if op == 9 && cnt == 4 {
            let at = base_at + (i + 1) * 4;
            if w32(job, at + 4) != 0 {
                return Err(Error::LoadHigh(w32(job, at + 4)));
            }
            let new = moved(w32(job, at) as u64, base, l)?;
            put32(job, at, new as u32);
            put32(job, at + 4, (new >> 32) as u32);
            m.loads += 1;
        }
        i += 1 + cnt;
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_layout() {
        let l = MAIN;
        // The job's frame, then slot 0 (4 MiB) on a frame, the image (slot 2) on a frame and whole.
        assert_eq!(l.slots[0], (FRAME, 4 * MIB));
        assert_eq!(l.slots[2].1, 16 * MIB);
        assert_eq!(l.slots[2].0 % FRAME, 0);
        // 15 and 16 side by side, so 15 runs on into 16 as it did in October.
        assert_eq!(l.slots[16].0, l.slots[15].0 + 16 * MIB);
        // No two slots overlap, and every one is aligned to its size up to a frame.
        for a in 0..SLOTS {
            let (o, s) = l.slots[a];
            if s == 0 {
                continue;
            }
            assert_eq!(o % s.min(FRAME), 0, "slot {a}");
            assert!(o + s <= l.bytes);
            for b in a + 1..SLOTS {
                let (p, t) = l.slots[b];
                assert!(t == 0 || o + s <= p || p + t <= o, "slots {a} and {b}");
            }
        }
        println!("main layout: {} MiB, {} frames", l.bytes >> 20, l.bytes / FRAME);
        assert!(l.bytes <= 80 * MIB);
    }

    fn synthetic() -> Vec<u8> {
        let mut list: Vec<u32> = vec![];
        // op 2: the image register and its high half, a plain register.
        list.extend([2 << 14 | 2, IMAGE_REG - ISPFE, 0xA200_0000]);
        list.extend([2 << 14 | 2, IMAGE_REG + 4 - ISPFE, 0]);
        list.extend([2 << 14 | 2, 0x2_0000, 0x1234]);
        // op 1: a burst from G#1C35_51C0 (address), +4 (high), +8 (address 51C8), +C (high).
        list.extend([5 << 14 | 1, 0x5_51C0, 0xAF00_0000, 0, 0xB000_0100, 0]);
        // op 9: a block load from the job's sources.
        list.extend([4 << 14 | 9, 0x8802_0000, 0, 0x10, 0x6_95FC]);
        list.push(0);
        let list: Vec<u8> = list.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut ring = vec![];
        for _ in 0..4 {
            for w in [0xC003u32, 0x8801_0000, 0, list.len() as u32] {
                ring.extend_from_slice(&w.to_le_bytes());
            }
        }
        let mut job = b"CPR3".to_vec();
        job.extend_from_slice(&3u32.to_le_bytes());
        for (pa, b) in [(0x8800_0000u32, &ring), (0x8801_0000, &list), (0x8802_0000, &vec![7u8; 16])] {
            job.extend_from_slice(&pa.to_le_bytes());
            job.extend_from_slice(&(b.len() as u32).to_le_bytes());
            job.extend_from_slice(b);
        }
        job
    }

    #[test]
    fn a_job_moves_high() {
        let mut job = synthetic();
        let base = 0x9_2000_0000u64;
        let m = relocate(&mut job, base, &MAIN).unwrap();
        assert_eq!(m, Moved { addr_regs: 3, highs: 3, highs_left: 0, loads: 1, ring: 4, slots: 1 << 2 | 1 << 15 | 1 << 16 });
        let p = pieces(&job).unwrap();
        let list = p.iter().find(|x| x.0 == OLD_JOB + LIST_OFFSET).unwrap();
        let w = |i: usize| w32(&job, list.1 + i * 4);
        let img = base + MAIN.slots[2].0;
        assert_eq!((w(2), w(5)), (img as u32, (img >> 32) as u32));
        assert_eq!(w(8), 0x1234);
        let s15 = base + MAIN.slots[15].0;
        let s16 = base + MAIN.slots[16].0 + 0x100;
        assert_eq!((w(11), w(12), w(13), w(14)), (s15 as u32, (s15 >> 32) as u32, s16 as u32, (s16 >> 32) as u32));
        let src = base + 0x2_0000;
        assert_eq!((w(16), w(17)), (src as u32, (src >> 32) as u32));
        let ring = p.iter().find(|x| x.0 == OLD_JOB).unwrap();
        let r = |i: usize| w32(&job, ring.1 + i * 4);
        assert_eq!((r(1), r(2)), ((base + LIST_OFFSET) as u32, ((base + LIST_OFFSET) >> 32) as u32));
    }

    #[test]
    fn an_unplaced_address_refuses() {
        let mut job = synthetic();
        let mut sizes = MAIN_SLOT_BYTES;
        sizes[15] = 0;
        assert_eq!(relocate(&mut job, 0xA000_0000, &layout(&sizes)), Err(Error::Unplaced(0xAF00_0000)));
    }

    /// The local job, when it is on this machine (target/cam/cam.pdma, the main camera's, list 2): every address it names lands in the region, in the slots the layout measured.
    #[test]
    fn the_local_job() {
        let Ok(orig) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/cam.pdma") else { return };
        for base in [0xA000_0000u64, 0x9_2000_0000] {
            let mut job = orig.clone();
            let m = relocate(&mut job, base, &MAIN).expect("the main job moves");
            println!("base G#{base:X}: {m:?}");
            assert_eq!(m.addr_regs, 25);
            assert_eq!(m.loads, 12);
            assert_eq!(m.ring, 1024);
            assert_eq!(m.slots, (1 << 22) - 1, "slots 0 to 21, as October measured");
            // G#1C35_4ECC's +4 holds G#C000_0000: left, and counted only for a region past 4 GiB.
            assert_eq!(m.highs, 24);
            assert_eq!(m.highs_left, if base >> 32 != 0 { 1 } else { 0 });
        }
    }
}
