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

/// The ultrawide's outputs (docs/CAMERA.md "The other cameras"), measured on the Pro (target/cam/run-uw1..4.txt, 2026-10-22: every October slice patterned, sampled every 256 bytes after a ten-frame run, the same extents in all four runs).
/// Its job was built by the later image builder, which kept each output's offset in its 16 MiB IOVA granule, so an output lies at its captured offset inside its slot rather than at the slot's start (`UW_SLOT_SKIP`, the lowest offset any address register names there; nothing was written below it, except in slot 9, which slot 8 runs on into).
/// Seen written, from the skip: 0 two 16-byte headers to +G#2100; 1 the PD path (4000 x 750 at G#13A0 a line, G#398000 bytes); 2 the image body (4000 x 3000 RAW10 at G#13C0 a line, G#E77100); 3 to +G#D0100; 4 to +G#6100; 5 to +G#9EF000; 6 to +G#6C300; 7 to +G#5D4000; 8 to its end (+G#FFFE00, running on into 9); 9 to +G#9F0800.
/// The four runs wrote every output to the same byte, so each size is what was written rounded up, not twice it as `MAIN_SLOT_BYTES` has: to a power of two under a frame, to whole frames past one (59 KiB of room at the least, slot 9), the image exactly its buffer, slot 8 exactly its granule so 9 follows it as it did in October.
/// Twice would make the region 174 MiB with a ring of four; this is 120 MiB, so a ring of one (72 MiB) still opens on a pool with little free (a jumped session's).
/// The driver patterns every output at each open and says after the eighth frame how far each was written, so a run re-measures this.
pub const UW_SLOT_BYTES: [u64; SLOTS] = [
    16 * KIB, 4 * MIB, 16 * MIB, MIB, 32 * KIB, 10 * MIB, 512 * KIB, 6 * MIB, 16 * MIB, 10 * MIB, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
pub const UW_SLOT_SKIP: [u64; SLOTS] = [
    0x48_7000, 0xC0_0000, 0, 0xE7_8000, 0xFE_0000, 0x4F_2000, 0xAF_4000, 0x9F_4000, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
/// The front camera's outputs: never measured (the sensor has not answered ferros), so each slot its job names (0 to 7, read from target/cam/front.pdma) keeps its whole 16 MiB granule, every captured offset as it is, as October's layout had them.
pub const FRONT_SLOT_BYTES: [u64; SLOTS] = [
    16 * MIB, 16 * MIB, 16 * MIB, 16 * MIB, 16 * MIB, 16 * MIB, 16 * MIB, 16 * MIB, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
/// akita's main camera's outputs (docs/CAMERA.md "akita's cameras"): not measured, so each slot runs from the lowest offset its job names to its granule's end, in whole frames (tools/cam-oracle pdma-image `--own-image`, from list 1 of the 2026-11-15 capture: target/cam/akita-main.pdma, local).
/// Its job was built with the image body alone in slot 2: Android put the PD output (G#1C32_21C0) at the start of the image's granule and the 20.2 MB image 5 MiB in, so a ring in slot 2 would have had the PD path write over buffer 0; the PD output is slot 1 now.
pub const AKITA_MAIN_SLOT_BYTES: [u64; SLOTS] = [
    14 * MIB, 16 * MIB, 0, 16 * MIB, 2 * MIB, 2 * MIB, 4 * MIB, 4 * MIB, 2 * MIB, 16 * MIB, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
pub const AKITA_MAIN_SLOT_SKIP: [u64; SLOTS] = [
    0x3E_0000, 0, 0, 0x10_0000, 0xE7_0000, 0xE0_0000, 0xD7_8000, 0xD8_0000, 0xE0_0000, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
/// akita's front camera's outputs, the same way, from list 2 of the same capture (target/cam/akita-front.pdma, local): no PD output in its program.
pub const AKITA_FRONT_SLOT_BYTES: [u64; SLOTS] = [
    16 * MIB, 12 * MIB, 0, 4 * MIB, 2 * MIB, 8 * MIB, 2 * MIB, 8 * MIB, 12 * MIB, 12 * MIB, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
pub const AKITA_FRONT_SLOT_SKIP: [u64; SLOTS] = [
    0x10_0000, 0x47_8000, 0, 0xD6_0000, 0xE6_D000, 0x88_0000, 0xFF_4000, 0x98_0000, 0x50_0000, 0x48_0000, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// No skips: every output at its slot's start (the main camera's job), or its slot's whole granule (the front camera's).
pub const NO_SKIP: [u64; SLOTS] = [0; SLOTS];

/// Where each October slot lands in the region: (offset, bytes), (0, 0) for none; the offset in its 16 MiB granule each slot's first byte stands for (`skip`, 0 for the main camera's); and the region's whole size, in frames.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub slots: [(u64, u64); SLOTS],
    pub skip: [u64; SLOTS],
    pub bytes: u64,
}

/// The job's frame first, then each slot in order, each aligned to its own size up to a frame (sizes under a frame are powers of two, the rest whole frames), the whole rounded up to frames.
pub const fn layout(sizes: &[u64; SLOTS]) -> Layout {
    layout_at(sizes, &NO_SKIP)
}

/// `layout` with each slot standing for its granule from `skip[k]` on.
pub const fn layout_at(sizes: &[u64; SLOTS], skip: &[u64; SLOTS]) -> Layout {
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
    Layout { slots, skip: *skip, bytes: (off + FRAME - 1) & !(FRAME - 1) }
}

/// The main camera's region.
pub const MAIN: Layout = layout(&MAIN_SLOT_BYTES);

/// One frame of the image ring (docs/CAMERA.md "The view"): the image body, 4080 x 3072 RAW10 at G#1400 a line (15 MiB), in 16 MiB, eight frames of the pool, so the view maps whole frames of image and nothing else.
pub const IMAGE_BYTES: u64 = 16 * MIB;
/// The most image buffers a ring holds: the view's window (kernel cachemap `CAM_VIEW_VA`, 128 MiB).
pub const RING_MOST: usize = 1 << 3;

/// The main camera's region with an image ring of `n` buffers (1 to `RING_MOST`): slot 2, the image body, made `n` buffers of `IMAGE_BYTES` side by side, the job's own address in the first; every other slot as `MAIN` has it. The ring's buffer k is at `slots[OLD_SLOT_IMAGE].0 + k * IMAGE_BYTES`.
pub const fn main_ring(n: usize) -> Layout {
    ring(&MAIN_SLOT_BYTES, &NO_SKIP, n)
}

/// Any camera's region with an image ring of `n` buffers: its slot sizes and skips (`MAIN_SLOT_BYTES`, `UW_SLOT_BYTES` with `UW_SLOT_SKIP`, ..), slot 2 made `n` buffers of `IMAGE_BYTES` side by side. Every camera's image body is slot 2, at its granule's start (tools/camera-pdma-image.py pinned it there for every camera).
pub const fn ring(sizes: &[u64; SLOTS], skip: &[u64; SLOTS], n: usize) -> Layout {
    with_image(sizes, skip, n as u64 * IMAGE_BYTES)
}

/// Any camera's region with `image` bytes in slot 2, the image body's (a whole number of frames of the pool, so the view maps image and nothing else): a ring of buffers sized to a mode's frame (docs/CAMERA.md "Modes, the bench and the slit": the main's 1:1 RAW10 frame is 60 MiB, a slit's ring hundreds of small buffers), every other slot as `sizes` has it.
pub const fn with_image(sizes: &[u64; SLOTS], skip: &[u64; SLOTS], image: u64) -> Layout {
    let mut s = *sizes;
    s[OLD_SLOT_IMAGE] = (image + FRAME - 1) & !(FRAME - 1);
    layout_at(&s, skip)
}

/// A mode's image buffer: its rows of `stride` bytes rounded up to whole frames of the pool (the binned main's 3072 lines of G#1400 are 15 MiB, so 16 MiB as `IMAGE_BYTES`; its 1:1 6144 lines of G#2800 exactly 60 MiB).
pub const fn buffer_bytes(rows: u64, stride: u64) -> u64 {
    (rows * stride + FRAME - 1) & !(FRAME - 1)
}

/// A slit ring's buffer: its rows and a 64-byte record the driver writes after them at the frame's DMA_DONE (the frame's count and time: the consumer's check that the buffer holds the frame it expects), rounded up to a 4 KiB page (a 4-row binned slit of the main, 20 KiB of lines, takes 24 KiB; a ring's buffers lie side by side, the ring in whole frames of the pool, `with_image`).
pub const fn slit_bytes(rows: u64, stride: u64) -> u64 {
    (rows * stride + 64 + 4095) & !4095
}

/// The job's register program, the piece at `OLD_JOB + LIST_OFFSET`: where its bytes start in the image and their length.
pub fn list_piece(job: &[u8]) -> Result<(usize, usize), Error> {
    let p = pieces(job)?;
    p.iter().find(|x| x.0 == OLD_JOB + LIST_OFFSET).map(|x| (x.1, x.2)).ok_or(Error::NoList)
}

/// The words of the program (a `list_piece`'s bytes) that write the image body's address register and its high half (`IMAGE_REG`, `IMAGE_REG` + 4), as word indices into it: each copy of the program the ring gets names its own buffer there (docs/CAMERA.md "The view"). An error when the program writes either not exactly once.
pub fn image_words(list: &[u8]) -> Result<(usize, usize), Error> {
    let words = list.len() / 4;
    let (mut lo, mut hi) = ([0usize; 2], [0usize; 2]);
    let (mut nl, mut nh) = (0, 0);
    each_write(words, &|i| w32(list, i * 4), &mut |i, reg| {
        // WHY the bounds: a second write of either is counted but not kept; the count refuses it below.
        if reg == IMAGE_REG {
            if nl < 2 {
                lo[nl] = i;
            }
            nl += 1;
        } else if reg == IMAGE_REG + 4 {
            if nh < 2 {
                hi[nh] = i;
            }
            nh += 1;
        }
    })?;
    if nl != 1 || nh != 1 {
        return Err(Error::ImageWrites(nl as u32, nh as u32));
    }
    Ok((lo[0], hi[0]))
}

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
    /// The program writes the image body's address register, or its high half, other than once each (how many times each): the ring could not name its buffers in it.
    ImageWrites(u32, u32),
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
        // WHY the check: an address below the slot's skip, or past its room, is one this layout was not measured on; moved anyway it would land in another slot or past the region (where the fence faults it), so the job is refused instead.
        if size == 0 || off < l.skip[k] || off - l.skip[k] >= size {
            return Err(Error::Unplaced(old));
        }
        return Ok(base + at + off - l.skip[k]);
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

    #[test]
    fn a_ring_layout() {
        assert_eq!(main_ring(1).slots, MAIN.slots);
        assert_eq!(main_ring(1).bytes, MAIN.bytes);
        for n in 1..=RING_MOST {
            let l = main_ring(n);
            let (img, size) = l.slots[OLD_SLOT_IMAGE];
            assert_eq!(size, n as u64 * IMAGE_BYTES);
            assert_eq!(img % FRAME, 0, "the ring starts on a frame, so the view maps whole frames");
            // Every other slot keeps its size, apart from the ring and each other.
            for k in 0..SLOTS {
                let (o, s) = l.slots[k];
                if k != OLD_SLOT_IMAGE {
                    assert_eq!(s, MAIN_SLOT_BYTES[k]);
                }
                if s != 0 && k != OLD_SLOT_IMAGE {
                    assert!(o + s <= img || img + size <= o, "slot {k} and the ring of {n}");
                }
            }
            println!("ring of {n}: {} MiB, {} frames, the ring at {} MiB", l.bytes >> 20, l.bytes / FRAME, img >> 20);
        }
        // A 4080 x 3072 RAW10 frame at G#1400 a line fits a buffer.
        assert!(3072 * 0x1400 <= IMAGE_BYTES);
    }

    #[test]
    fn the_image_words() {
        let mut job = synthetic();
        relocate(&mut job, 0x9_2000_0000, &main_ring(4)).unwrap();
        let (at, len) = list_piece(&job).unwrap();
        let (lo, hi) = image_words(&job[at..at + len]).unwrap();
        assert_eq!((lo, hi), (2, 5));
        let img = 0x9_2000_0000 + main_ring(4).slots[OLD_SLOT_IMAGE].0;
        assert_eq!((w32(&job, at + lo * 4), w32(&job, at + hi * 4)), (img as u32, (img >> 32) as u32));
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
        // The ring: the program names the image body once, and each of its copies can name its own buffer (docs/CAMERA.md "The view").
        let mut job = orig.clone();
        let l = main_ring(4);
        relocate(&mut job, 0xA000_0000, &l).expect("the main job moves into a ring's region");
        let (at, len) = list_piece(&job).unwrap();
        let (lo, hi) = image_words(&job[at..at + len]).expect("the image body is written once");
        assert_eq!(w32(&job, at + lo * 4) as u64, 0xA000_0000 + l.slots[OLD_SLOT_IMAGE].0);
        assert_eq!(w32(&job, at + hi * 4), 0);
        let p = pieces(&job).unwrap();
        for &(pa, _, n) in p.iter() {
            println!("piece G#{pa:X}: {n} bytes");
        }
        println!("the program: {len} bytes, the image's words {lo} and {hi}; a ring of 4: {} MiB", l.bytes >> 20);
    }

    /// A skip moves an output to its slot's start less the skip, and refuses one below it.
    #[test]
    fn a_skip_moves_and_refuses() {
        let l = ring(&UW_SLOT_BYTES, &UW_SLOT_SKIP, 1);
        let s0 = l.slots[0].0;
        assert_eq!(moved(0xA048_7000, 0xA000_0000, &l), Ok(0xA000_0000 + s0));
        assert_eq!(moved(0xA048_9000, 0xA000_0000, &l), Ok(0xA000_0000 + s0 + 0x2000));
        assert_eq!(moved(0xA048_6000, 0xA000_0000, &l), Err(Error::Unplaced(0xA048_6000)));
        // Slot 9 follows slot 8 whole, as in October, so 8's run on into 9 lands where it did.
        assert_eq!(l.slots[9].0, l.slots[8].0 + 16 * MIB);
        assert_eq!(moved(0xA900_0000, 0, &l), Ok(moved(0xA800_0000, 0, &l).unwrap() + 16 * MIB));
    }

    /// The 1:1 rings (docs/CAMERA.md "Modes, the bench and the slit"): the main's and the ultrawide's region with four 1:1 RAW10 buffers, the long ring's frames and the program copies' (two more), inside the camera's window (kernel cachemap `CAM_VA`, 512 MiB since the bench), and the view of the four inside the view's (512 MiB); a slit ring of 2^10 buffers of a 64-row 1:1 slit too.
    #[test]
    fn the_full_rings_fit_the_window() {
        const WINDOW: u64 = 512 * MIB;
        for (name, sizes, skip, (w, h)) in [("main", MAIN_SLOT_BYTES, NO_SKIP, (8160u64, 6144u64)), ("ultrawide", UW_SLOT_BYTES, UW_SLOT_SKIP, (8000, 6000))] {
            let stride = (w * 10 / 8 + 255) & !255;
            let buf = buffer_bytes(h, stride);
            let l = with_image(&sizes, &skip, 4 * buf);
            println!("{name} 1:1: G#{stride:X} a line, a buffer {} MiB, the region with four {} MiB", buf >> 20, l.bytes >> 20);
            assert!(l.bytes + 3 * FRAME <= WINDOW && 4 * buf <= WINDOW);
            assert_eq!(l.slots[OLD_SLOT_IMAGE].0 % FRAME, 0);
        }
        assert_eq!(buffer_bytes(6144, 0x2800), 60 * MIB);
        assert_eq!(buffer_bytes(3072, 0x1400), IMAGE_BYTES);
        assert_eq!(slit_bytes(4, 0x1400), 24 * KIB);
        // A slit ring of 2^8 of the tallest slit the bench sweeps (64 rows 1:1, 644 KiB each), and 2^12 of the smallest (4 rows binned).
        for (n, rows, stride) in [(1u64 << 8, 64, 0x2800), (1 << 12, 4, 0x1400)] {
            let l = with_image(&MAIN_SLOT_BYTES, &NO_SKIP, n * slit_bytes(rows, stride));
            assert!(l.bytes + 3 * FRAME <= WINDOW, "{} MiB", l.bytes >> 20);
        }
    }

    /// The kernel's window for the camera's region (kernel cachemap `CAM_VA`, 224 MiB on 2026-11-15, 512 MiB since the bench) holds every camera's region with a ring of four and the long ring's frame; no two slots overlap and each is aligned to its size up to a frame.
    #[test]
    fn every_ring_fits_the_window() {
        const WINDOW: u64 = 224 * MIB;
        for (name, sizes, skip) in [("main", MAIN_SLOT_BYTES, NO_SKIP), ("ultrawide", UW_SLOT_BYTES, UW_SLOT_SKIP), ("front", FRONT_SLOT_BYTES, NO_SKIP)] {
            for n in 1..=RING_MOST >> 1 {
                let l = ring(&sizes, &skip, n);
                assert!(l.bytes + FRAME <= WINDOW, "{name}, a ring of {n}: {} MiB", l.bytes >> 20);
                for a in 0..SLOTS {
                    let (o, s) = l.slots[a];
                    if s == 0 {
                        continue;
                    }
                    assert_eq!(o % s.min(FRAME), 0, "{name} slot {a}");
                    for b in a + 1..SLOTS {
                        let (p, t) = l.slots[b];
                        assert!(t == 0 || o + s <= p || p + t <= o, "{name} slots {a} and {b}");
                    }
                }
            }
            println!("{name}: a ring of 1 {} MiB, of 4 {} MiB", ring(&sizes, &skip, 1).bytes >> 20, ring(&sizes, &skip, 4).bytes >> 20);
        }
    }

    /// The ultrawide's local job (target/cam/uw.pdma, local), when it is on this machine: every address it names lands in its region, in slots 0 to 9, and each copy of its program can name its own ring buffer.
    #[test]
    fn the_local_uw_job() {
        let Ok(orig) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/uw.pdma") else { return };
        for n in [1, RING_MOST >> 1] {
            let l = ring(&UW_SLOT_BYTES, &UW_SLOT_SKIP, n);
            for base in [0xA000_0000u64, 0x9_2000_0000] {
                let mut job = orig.clone();
                let m = relocate(&mut job, base, &l).expect("the ultrawide's job moves");
                assert_eq!((m.addr_regs, m.loads, m.slots), (25, 12, (1 << 10) - 1));
                let (at, len) = list_piece(&job).unwrap();
                let (lo, hi) = image_words(&job[at..at + len]).expect("the image body is written once");
                assert_eq!(w32(&job, at + lo * 4) as u64 | (w32(&job, at + hi * 4) as u64) << 32, base + l.slots[OLD_SLOT_IMAGE].0);
            }
            println!("the ultrawide, a ring of {n}: {} MiB", l.bytes >> 20);
        }
        // A 4000 x 3000 RAW10 frame at G#13C0 a line fits a buffer.
        assert!(3000 * 0x13C0 <= IMAGE_BYTES);
    }

    /// akita's local jobs (target/cam/akita-main.pdma, akita-front.pdma, built with `--own-image`), when they are on this machine: every address each names lands in its region, the image body in slot 2's ring buffer 0, and the program writes the image once.
    #[test]
    fn the_local_akita_jobs() {
        for (path, sizes, skip, image, regs, slots) in [
            ("/mnt/Harbor/Code/ferros/target/cam/akita-main.pdma", &AKITA_MAIN_SLOT_BYTES, &AKITA_MAIN_SLOT_SKIP, buffer_bytes(3472, 0x16C0), 25, (1u32 << 10) - 1),
            ("/mnt/Harbor/Code/ferros/target/cam/akita-front.pdma", &AKITA_FRONT_SLOT_BYTES, &AKITA_FRONT_SLOT_SKIP, buffer_bytes(3072, 0x14C0), 20, (1u32 << 10) - 1),
        ] {
            let Ok(orig) = std::fs::read(path) else { continue };
            for n in [1u64, 4] {
                let l = with_image(sizes, skip, n * image);
                let mut job = orig.clone();
                let base = 0x9_2000_0000;
                let m = relocate(&mut job, base, &l).expect(path);
                assert_eq!((m.addr_regs, m.slots), (regs, slots), "{path}");
                let (at, len) = list_piece(&job).unwrap();
                let (lo, hi) = image_words(&job[at..at + len]).expect("the image body is written once");
                assert_eq!(w32(&job, at + lo * 4) as u64 | (w32(&job, at + hi * 4) as u64) << 32, base + l.slots[OLD_SLOT_IMAGE].0);
                println!("{path}: a ring of {n}, {} MiB", l.bytes >> 20);
            }
        }
    }

    /// The front camera's local job (target/cam/front.pdma), when it is on this machine: it moves into its region of whole granules.
    #[test]
    fn the_local_front_job() {
        let Ok(orig) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/front.pdma") else { return };
        let l = ring(&FRONT_SLOT_BYTES, &NO_SKIP, RING_MOST >> 1);
        let mut job = orig.clone();
        let m = relocate(&mut job, 0xA000_0000, &l).expect("the front camera's job moves");
        assert_eq!((m.addr_regs, m.loads, m.slots), (25, 12, (1 << 8) - 1));
        let (at, len) = list_piece(&job).unwrap();
        assert!(image_words(&job[at..at + len]).is_ok());
        println!("the front camera, a ring of 4: {} MiB", l.bytes >> 20);
    }
}
