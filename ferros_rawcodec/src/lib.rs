//! ferros_rawcodec: the lossless band codec for 10-bit camera RAW mosaics, format Zil (`FORMAT` = 0).
//! no_std and no alloc: every function takes slices and a caller's `Work`, so the same code links into a ferros payload, a std app and a host grading tool.
//! The encoder takes MIPI RAW10 packed rows, the decoder gives back u16 samples, bit exact.
//! Born from tools/raw-codec's `Params::DEFAULT` path (docs/RAW-VIDEO.md); its output is that prototype's byte for byte, less the prototype's 4-byte first-stream length (the container carries both streams' lengths).
//!
//! # The bitstream, format Zil
//!
//! Everything below is the format; a decoder written from this text alone decodes every band ever coded under `FORMAT` = 0.
//! The constants named are this crate's: BITS = 10, BAND_ROWS = 2^6, BLOCK = 2^5 (BLOCK_LOG2 = 5), ESC = 2^4 (ESC_LOG2 = 4), K0 = 2, EDGE = 2^6.
//!
//! ## Samples, frames and bands
//!
//! A frame is W x H samples of BITS = 10 bits, each 0 to 1023, with W a multiple of 4 and 8 <= W <= 2^13.
//! Its colour filter array has period 2 (`Cfa::Bayer`, a 2 x 2 cell of four colours) or 4 (`Cfa::Quad`, the 1:1 quad array: a 4 x 4 cell of 2 x 2 blocks, each block one colour).
//! Which colour sits where is the container's to record; the coder only needs the period.
//! The frame is cut into bands of BAND_ROWS = 2^6 rows from its row 0; the last band holds the H mod 2^6 rows left over when that is not 0.
//! Each band is coded alone: nothing carries from one band to the next (no sample, no k, no state), so bands code and decode on any core in any order.
//! Inside a band, rows are numbered y = 0 to R - 1 (R the band's rows) and columns x = 0 to W - 1; s(y, x) is a sample.
//! The rules use y as the row's CFA phase, so a band starts on a frame row that is a multiple of the period (every multiple of 2^6 is).
//! The coder takes any R >= 1; the recorder's bands are BAND_ROWS.
//! A band codes to two byte streams, A and B, of la and lb bytes; the streams hold no lengths, headers or markers, and the container keeps la and lb for every band.
//! (The encoder's input packing, MIPI RAW10, is not part of the format: 4 samples in 5 bytes, byte i of 0 to 3 the high 8 bits of sample i, byte 4 the low 2 bits of sample i at its bits 2i and 2i + 1.)
//!
//! ## Same-colour neighbours
//!
//! Row y's same-colour row above is u = y - dy, with dy = 2 for Bayer; for Quad dy = 3 when y is even (a block's first row: the previous block row's second row) and dy = 1 when y is odd (the block's own first row).
//! Row y has a row above when y >= dy: for Bayer every row but 0 and 1, for Quad every row but 0 and 2 (row 1's row above is row 0).
//! The left distance dl(x) is 2 for Bayer; for Quad 3 when x is even (to the previous block's second column) and 1 when x is odd.
//! The right distance dr(x) is 2 for Bayer; for Quad 1 when x is even and 3 when x is odd (to the next block's first column).
//! The neighbours of s(y, x): a = s(y, x - dl(x)), and in a row with a row above b = s(u, x), c = s(u, x - dl(x)), d = s(u, x + dr(x)).
//!
//! ## Edges
//!
//! Every row is extended by 4 samples on each side (columns -4 to -1 and W to W + 3), defined from the row's own samples and its row above, so the encoder and the decoder build them alike:
//! - right: s(y, W + i) = s(y, W - 4 + i) for i = 0 to 3, the row's last 4 samples repeated (the period divides 4, so every column keeps its colour);
//! - left, a row with a row above: s(y, -4 + i) = s(u, i) for i = 0 to 3, the first 4 samples of the row above (column -4 + i has the colour of column i in the row above, which is this row's colour too);
//! - left, a row without one: s(y, -4 + i) = EDGE = 64 for i = 0 to 3.
//! a reaches columns -3 to -1 at the row's start; c reaches the row above's own left extension, d its right one (up to column W + 2).
//!
//! ## Prediction and residual
//!
//! A row without a row above predicts every sample by Left: p = a.
//! A row with one predicts each block (below) by one of two, named by the block's selection bit:
//! - selection 0, Avg4: p = (a + b + c + d) >> 2, the floor of the mean of the four;
//! - selection 1, MED (LOCO-I's median edge detector): p = min(max(a + b - c, min(a, b)), max(a, b)), the median of a, b and a + b - c.
//! The residual wraps to the 10-bit ring and zigzags: e = ((s - p + 512) mod 1024) - 512, in -512 to 511, then z = 2e when e >= 0 and z = -2e - 1 when e < 0, in 0 to 1023.
//! The decoder inverts it: e = z >> 1 when z is even, e = -((z + 1) >> 1) when z is odd, s = (p + e) mod 1024.
//! A row decodes left to right, since a is a sample of the same row.
//!
//! ## Blocks, selection and k
//!
//! Each row's residuals are cut into blocks of BLOCK = 32 from column 0; j = 0, 1, 2, ... numbers a row's blocks, from 0 again in every row.
//! The row's last block holds the W mod 32 residuals left over when that is not 0 (a multiple of 4, so every block holds at least 4).
//! Selection, rows with a row above only: Z0 is the sum of the block's z under Avg4 and Z1 under MED; the selection is 1 when Z1 < Z0 and 0 otherwise (a tie takes Avg4).
//! The Rice parameter: with Z the sum of the block's z under its predictor and n the block's length, m = floor(Z / n), k = floor(log2 m) when m >= 1 and k = 0 when m = 0 (on a u32: 31 - clz(m | 1)); so k <= 9.
//! kp is the k of the previous block in the band's order (rows top to bottom, a row's blocks left to right, whichever stream they went to); the band's first block takes kp = K0 = 2.
//!
//! ## A block's bits
//!
//! In this order:
//! 0. the selection, one bit, in rows with a row above; rows without one send none;
//! 1. k as a delta code against kp: "1" when k = kp, "010" when k = kp + 1, "011" when k = kp - 1, otherwise "00" then k in 4 bits;
//! 2. each of the block's residuals left to right: with q = z >> k, when q < ESC = 16 the code is q 0 bits, one 1 bit, then z's low k bits; when q >= 16 it is the escape, 16 0 bits then z in its 10 bits.
//! A residual's code is never more than 26 bits; a decoder counts the leading 0 bits: 16 or more is the escape (it drops exactly 16, then reads 10), fewer is q.
//! The "00" form of the k code is taken for any other k, including a step of 2 or more; a decoder reading "011" when kp is 0 has a corrupt stream (a valid one never has it).
//!
//! ## Streams and bits
//!
//! Block j of every row goes to stream A when j is even and to stream B when j is odd; each stream is its blocks' bits back to back in the band's order.
//! Bits are packed most significant first: a stream's first bit is bit 7 (the 128s) of its byte 0; every field of more than one bit (k's 4 bits, z's low k bits, the escape's 10 bits) goes most significant bit first.
//! A stream ends with its last block's last bit, padded with 0 bits to a whole byte; its length is ceil(bits / 8), and B is empty (lb = 0) when W <= 32.
//! The streams are not independent: kp and the samples run thru the band's order, so a decoder takes the blocks in that order, each from its stream.
//! A decoder reading ahead past a stream's end sees 0 bits; at the band's end it has used exactly la bytes of A and lb of B.
//!
//! # Use
//!
//! `encode_band` codes one band of packed rows into the caller's buffer of `max_band_len` bytes, A then B behind it; `decode_band` takes the two streams back to samples.
//! `bands`, `band_rows` and `band_src` cut a frame; the recorder runs one `encode_band` per band on its worker cores, each worker with its own `Work`.
#![no_std]

#[cfg(all(feature = "neon", target_arch = "aarch64"))]
mod neon;

/// The format's name: Zil, the first (dozenal versioning, counted from 0).
pub const FORMAT: u32 = 0;
/// Sample depth.
pub const BITS: u32 = 10;
/// Rows a band: the recorder's unit of work and the frame's cut.
pub const BAND_ROWS: usize = 1 << 6;
/// Residuals a block share one k and one selection bit: 2^BLOCK_LOG2.
pub const BLOCK_LOG2: u32 = 5;
pub const BLOCK: usize = 1 << BLOCK_LOG2;
/// The unary part caps at 2^ESC_LOG2 zeros, then the raw 10 bits.
pub const ESC_LOG2: u32 = 4;
pub const ESC: u32 = 1 << ESC_LOG2;
/// k a band starts from for the delta code.
pub const K0: u32 = 2;
/// Value of the left extension in a row without a same-colour row above.
pub const EDGE: u16 = 1 << 6;
/// Narrowest row.
pub const MIN_W: usize = 1 << 3;
/// Widest row a `Work` holds (the quad array's 8160 fits).
pub const MAX_W: usize = 1 << 13;

/// Extension each side of a row in the row buffers.
const PAD: usize = 1 << 2;
const ROW: usize = MAX_W + 2 * PAD;
const MASK: i16 = (1 << BITS) - 1;
const HALF: i16 = 1 << (BITS - 1);
const MAX_BLOCKS: usize = MAX_W >> BLOCK_LOG2;
/// A block's longest header: the selection bit and the 6-bit k code.
const HEAD_MAX: u32 = 1 + 6;
/// A residual's longest code: the escape.
const CODE_MAX: u32 = ESC + BITS;
/// A block's most bytes: header and BLOCK escapes.
const BLOCK_MAX_BYTES: usize = (HEAD_MAX as usize + BLOCK * CODE_MAX as usize).div_ceil(8);

/// Colour filter period: `Bayer` a 2 x 2 cell, `Quad` the 4 x 4 cell of 2 x 2 same-colour blocks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cfa {
    Bayer,
    Quad,
}

impl Cfa {
    /// Rows (and columns) a cell: 2 or 4.
    pub const fn period(self) -> usize {
        match self {
            Cfa::Bayer => 2,
            Cfa::Quad => 4,
        }
    }
    /// Distance from row y to its same-colour row above.
    #[inline(always)]
    fn dy(self, y: usize) -> usize {
        match self {
            Cfa::Bayer => 2,
            Cfa::Quad => 3 - ((y & 1) << 1),
        }
    }
    /// Left same-colour distance at column x.
    #[inline(always)]
    fn dxl(self, x: usize) -> usize {
        match self {
            Cfa::Bayer => 2,
            Cfa::Quad => 3 - ((x & 1) << 1),
        }
    }
    /// Right same-colour distance at column x.
    #[inline(always)]
    fn dxr(self, x: usize) -> usize {
        match self {
            Cfa::Bayer => 2,
            Cfa::Quad => 1 + ((x & 1) << 1),
        }
    }
}

// ---------------------------------------------------------------- frames

/// Bytes of one packed RAW10 row of `width` samples.
pub const fn row_bytes(width: usize) -> usize {
    (width >> 2) * 5
}

/// Bands of a frame `height` rows high: ceil(height / BAND_ROWS).
pub const fn bands(height: usize) -> usize {
    height.div_ceil(BAND_ROWS)
}

/// The frame row band k starts on.
pub const fn band_first_row(k: usize) -> usize {
    k * BAND_ROWS
}

/// Rows of band k: BAND_ROWS, the last band the rows left over.
pub const fn band_rows(height: usize, k: usize) -> usize {
    let left = height - k * BAND_ROWS;
    // The band's length by definition, not a guard: whole bands, then the remainder.
    if left < BAND_ROWS { left } else { BAND_ROWS }
}

/// Bytes of a packed frame (`stride` bytes a row) that band k reads: its first row's start to its last row's last sample.
pub const fn band_src(k: usize, height: usize, width: usize, stride: usize) -> core::ops::Range<usize> {
    let start = band_first_row(k) * stride;
    start..start + (band_rows(height, k) - 1) * stride + row_bytes(width)
}

/// Bytes one stream may take while written: its blocks at their longest (each stream holds at most ceil(blocks / 2) of a row's blocks, each at most BLOCK_MAX_BYTES), plus the writer's 8-byte store past its last byte.
const fn max_stream_len(width: usize, rows: usize) -> usize {
    rows * (width.div_ceil(BLOCK)).div_ceil(2) * BLOCK_MAX_BYTES + 8
}

/// Bytes the encoder's output slice must hold for one band: room for each stream at its worst while both are written side by side.
/// Coded, a band is la + lb bytes, A at the slice's start and B right behind it.
pub const fn max_band_len(width: usize, rows: usize) -> usize {
    2 * max_stream_len(width, rows)
}

// ---------------------------------------------------------------- work

/// The encoder's per-row scratch: every residual's code and length, two codes joined for one put, every block's header.
struct RowCodes {
    lens: [u16; MAX_W],
    codes: [u16; MAX_W],
    /// Two codes joined, the first above the second, and their summed length.
    pcode: [u64; MAX_W >> 1],
    plen: [u8; MAX_W >> 1],
    hcode: [u8; MAX_BLOCKS],
    hlen: [u8; MAX_BLOCKS],
}

/// Scratch for one band at a time: four row buffers (a quad row's same-colour row above is 3 back), a residual row per candidate predictor (two), the decoder's selections and the encoder's row codes.
/// About 165 KiB; one per worker, owned by the caller (a Box on a std host, a static or a stack of its own elsewhere).
/// Nothing in it carries from one band to the next: every band rebuilds what it reads.
pub struct Work {
    rows: [[u16; ROW]; 4],
    m: [[u16; MAX_W]; 2],
    sel: [u8; MAX_BLOCKS],
    rc: RowCodes,
}

impl Work {
    pub const fn new() -> Work {
        Work {
            rows: [[0; ROW]; 4],
            m: [[0; MAX_W]; 2],
            sel: [0; MAX_BLOCKS],
            rc: RowCodes { lens: [0; MAX_W], codes: [0; MAX_W], pcode: [0; MAX_W >> 1], plen: [0; MAX_W >> 1], hcode: [0; MAX_BLOCKS], hlen: [0; MAX_BLOCKS] },
        }
    }
}

impl Default for Work {
    fn default() -> Self {
        Work::new()
    }
}

// ---------------------------------------------------------------- bits

/// A cursor, the accumulator and its bit count: three registers a writer, so two writers in lockstep fit without spilling.
struct BitWriter<'a> {
    cur: *mut u8,
    start: *mut u8,
    acc: u64,
    n: u32,
    _out: core::marker::PhantomData<&'a mut [u8]>,
}

impl<'a> BitWriter<'a> {
    fn new(out: &'a mut [u8]) -> Self {
        let p = out.as_mut_ptr();
        BitWriter { cur: p, start: p, acc: 0, n: 0, _out: core::marker::PhantomData }
    }
    /// Append the low `len` bits of `code` (1 <= len <= 56, code < 2^len), most significant first.
    /// Branchless: the whole 64-bit accumulator is stored at every put and the cursor advances by the whole bytes.
    #[inline(always)]
    fn put(&mut self, code: u64, len: u32) {
        self.acc |= code << (64 - self.n - len);
        self.n += len;
        // SAFETY: encode_band asserted out.len() >= max_band_len and gave each writer max_stream_len bytes, its blocks at their longest plus 8; cur is the whole bytes written so far, so cur + 8 stays inside the writer's slice.
        unsafe { (self.cur as *mut u64).write_unaligned(self.acc.to_be()) };
        let bytes = self.n >> 3;
        // SAFETY: the same bound, cur advances only over bytes just stored.
        self.cur = unsafe { self.cur.add(bytes as usize) };
        // n <= 7 + 56 = 63 here, so the shift is at most 56.
        self.acc <<= bytes << 3;
        self.n &= 7;
    }
    /// Bytes written, the last partial byte's low bits 0 (the stream's padding).
    fn finish(self) -> usize {
        // SAFETY: cur and start point into the same slice, cur never below start.
        let pos = unsafe { self.cur.offset_from(self.start) } as usize;
        pos + ((self.n as usize + 7) >> 3)
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    n: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        let mut r = BitReader { data, pos: 0, acc: 0, n: 0 };
        r.refill();
        r
    }
    #[inline(always)]
    fn load(&self) -> u64 {
        // WHY: the stream is external input and its last bytes sit at the end of the caller's slice; an 8-byte read there would run past it.
        // Refill also claims bytes ahead of use, so near the end pos can pass the slice's length (those bytes read as zeros).
        // PROOF: without the check the read at pos > len - 8 leaves the slice (a stream is exactly its coded bytes); the zero fill is the stream's padding, and a corrupt stream decodes to wrong samples, never to a read outside the slice.
        let len = self.data.len();
        let mut b = [0u8; 8];
        if self.pos + 8 <= len {
            b.copy_from_slice(&self.data[self.pos..self.pos + 8]);
        } else {
            let start = self.pos.min(len);
            b[..len - start].copy_from_slice(&self.data[start..]);
        }
        u64::from_be_bytes(b)
    }
    /// After a refill at least 56 bits are valid.
    #[inline(always)]
    fn refill(&mut self) {
        let v = self.load();
        self.acc |= v >> self.n;
        let bytes = (63 - self.n) >> 3;
        self.pos += bytes as usize;
        self.n += bytes << 3;
    }
    #[inline(always)]
    fn peek_lz(&self) -> u32 {
        self.acc.leading_zeros()
    }
    #[inline(always)]
    fn skip(&mut self, len: u32) {
        self.acc <<= len;
        self.n -= len;
    }
    /// Read `len` bits, 0 <= len <= 56.
    #[inline(always)]
    fn get(&mut self, len: u32) -> u32 {
        // A zero-length read is a shift by 64 in the naive form; the double shift keeps it defined.
        let v = (self.acc >> 1) >> (63 - len);
        self.skip(len);
        v as u32
    }
    /// Bytes used so far, a partial byte counted whole.
    fn consumed(&self) -> usize {
        (self.pos * 8 - self.n as usize + 7) >> 3
    }
}

// ---------------------------------------------------------------- rows

/// Unpack one MIPI RAW10 group: 4 samples in 5 bytes, the high 8 bits in bytes 0 to 3, the four 2-bit lows in byte 4 (sample i at bits 2i, 2i + 1).
/// SWAR in one u64 of four 16-bit lanes: the four high bytes spread to the lanes by two shift-or-mask steps, the low byte's four 2-bit fields spread the same way, one shift and one or.
#[inline(always)]
fn unpack_group(g: &[u8]) -> u64 {
    let mut h = u32::from_le_bytes([g[0], g[1], g[2], g[3]]) as u64;
    h = (h | (h << 16)) & 0x0000_FFFF_0000_FFFF;
    h = (h | (h << 8)) & 0x00FF_00FF_00FF_00FF;
    let mut l = g[4] as u64;
    l = (l | (l << 28)) & 0x0000_000F_0000_000F;
    l = (l | (l << 14)) & 0x0003_0003_0003_0003;
    (h << 2) | l
}

#[inline(always)]
fn unpack_row(src: &[u8], dst: &mut [u16]) {
    for (g, d) in src.chunks_exact(5).zip(dst.chunks_exact_mut(4)) {
        let r = unpack_group(g);
        d[0] = r as u16;
        d[1] = (r >> 16) as u16;
        d[2] = (r >> 32) as u16;
        d[3] = (r >> 48) as u16;
    }
}

/// The left extension: the row above's first 4 samples, or EDGE in a row without one (the format's Edges).
fn left_pad(rows: &mut [[u16; ROW]; 4], ci: usize, ui: Option<usize>) {
    let mut p = [EDGE; PAD];
    if let Some(ui) = ui {
        p.copy_from_slice(&rows[ui][PAD..PAD + PAD]);
    }
    rows[ci][..PAD].copy_from_slice(&p);
}

/// The right extension: the row's own last 4 samples.
fn right_pad(row: &mut [u16; ROW], w: usize) {
    row.copy_within(PAD + w - PAD..PAD + w, PAD + w);
}

// ---------------------------------------------------------------- prediction

/// Predictor ids.
const LEFT: u8 = 0;
const AVG4: u8 = 1;
const MED: u8 = 2;

/// LOCO-I MED as median(a, b, a + b - c); the min and max are the predictor's definition, not a range guard.
#[inline(always)]
fn med(a: i16, b: i16, c: i16) -> i16 {
    let p = a + b - c;
    p.max(a.min(b)).min(a.max(b))
}

#[inline(always)]
fn predict<const P: u8>(a: i16, b: i16, c: i16, d: i16) -> i16 {
    match P {
        LEFT => a,
        AVG4 => (a + b + c + d) >> 2,
        _ => med(a, b, c),
    }
}

/// Residual wrapped to the 10-bit ring (x - pred mod 1024 in -512..511) and zigzagged to 0..1023.
#[inline(always)]
fn zig(x: i16, pred: i16) -> u16 {
    let e = ((x - pred + HALF) & MASK) - HALF;
    ((e << 1) ^ (e >> 15)) as u16
}

#[inline(always)]
fn unzig(m: u16) -> i16 {
    ((m >> 1) as i16) ^ -((m & 1) as i16)
}

/// Residuals of a row from column x0 (even) to w, fixed-width lane code: every sample's neighbours are plain shifted loads of the row and the row above.
/// DXE / DXO: the left same-colour distance at even / odd columns (Bayer 2, 2; quad 3, 1); DRE / DRO the right one in the row above (Bayer 2, 2; quad 1, 3).
#[inline(always)]
fn residuals_scalar<const P: u8, const DXE: usize, const DXO: usize, const DRE: usize, const DRO: usize>(cur: &[u16; ROW], up: &[u16; ROW], x0: usize, w: usize, m: &mut [u16]) {
    let c0 = &cur[PAD + x0..PAD + w];
    let le = &cur[PAD + x0 - DXE..PAD - DXE + w];
    let lo = &cur[PAD + x0 - DXO..PAD - DXO + w];
    let u0 = &up[PAD + x0..PAD + w];
    let ue = &up[PAD + x0 - DXE..PAD - DXE + w];
    let uo = &up[PAD + x0 - DXO..PAD - DXO + w];
    let re = &up[PAD + x0 + DRE..PAD + DRE + w];
    let ro = &up[PAD + x0 + DRO..PAD + DRO + w];
    let m = &mut m[x0..w];
    if DXE == DXO {
        // Bayer: one distance, plain shifted slices.
        for (((((m, &c), &l), &u), &ul), &ur) in m.iter_mut().zip(c0).zip(le).zip(u0).zip(ue).zip(re) {
            *m = zig(c as i16, predict::<P>(l as i16, u as i16, ul as i16, ur as i16));
        }
        return;
    }
    // Quad: both distances loaded, the column's parity picks one (a fixed alternating blend mask in lane code; x0 is even, so i's parity is x's).
    let it = m.iter_mut().zip(c0).zip(le).zip(lo).zip(u0).zip(ue).zip(uo).zip(re).zip(ro).enumerate();
    for (i, ((((((((m, &c), &le), &lo), &u), &ue), &uo), &re), &ro)) in it {
        let odd = i & 1 == 1;
        let a = if odd { lo } else { le };
        let ul = if odd { uo } else { ue };
        let ur = if odd { ro } else { re };
        *m = zig(c as i16, predict::<P>(a as i16, u as i16, ul as i16, ur as i16));
    }
}

#[inline(always)]
fn residuals_t<const N: bool, const P: u8, const DXE: usize, const DXO: usize, const DRE: usize, const DRO: usize>(cur: &[u16; ROW], up: &[u16; ROW], w: usize, m: &mut [u16]) {
    #[cfg(all(feature = "neon", target_arch = "aarch64"))]
    if N {
        let x0 = neon::residuals::<P, DXE, DXO, DRE, DRO>(cur, up, w, m);
        residuals_scalar::<P, DXE, DXO, DRE, DRO>(cur, up, x0, w, m);
        return;
    }
    residuals_scalar::<P, DXE, DXO, DRE, DRO>(cur, up, 0, w, m);
}

fn residuals<const N: bool, const P: u8>(cfa: Cfa, cur: &[u16; ROW], up: &[u16; ROW], w: usize, m: &mut [u16]) {
    match cfa {
        Cfa::Bayer => residuals_t::<N, P, 2, 2, 2, 2>(cur, up, w, m),
        Cfa::Quad => residuals_t::<N, P, 3, 1, 1, 3>(cur, up, w, m),
    }
}

// ---------------------------------------------------------------- block coder

/// k delta code per block: same k "1", k + 1 "010", k - 1 "011", else "00" and k in 4 bits; returns (code, length).
#[inline(always)]
fn k_code(k: u32, kprev: u32) -> (u32, u32) {
    // The signed step from the previous k (both at most 9), not an overflow guard.
    let d = k.wrapping_sub(kprev) as i32;
    match d {
        0 => (1, 1),
        1 => (2, 3),
        -1 => (3, 3),
        _ => (k, 6),
    }
}

#[inline(always)]
fn get_k(br: &mut BitReader, kprev: u32) -> u32 {
    let top = (br.acc >> 61) as u32;
    if top >= 4 {
        br.skip(1);
        kprev
    } else if top >= 2 {
        br.skip(3);
        // A corrupt stream can step k below 0 or past 15: the mask keeps every later shift defined, and the samples it gives are wrong, as a corrupt stream's are.
        kprev.wrapping_add(1 - ((top & 1) << 1)) & 15
    } else {
        br.skip(2);
        br.get(4)
    }
}

/// Sum of a block's residuals: at most 2^5 x 1023, inside a u32.
#[inline(always)]
fn block_sum<const N: bool>(v: &[u16]) -> u32 {
    #[cfg(all(feature = "neon", target_arch = "aarch64"))]
    if N {
        return neon::block_sum(v);
    }
    v.iter().map(|&v| v as u32).sum()
}

/// Each residual's code and length under k, fixed-width lanes with no branch, from index i0 on.
/// u16 lanes: a residual is below 2^10, a length at most 2^4 + 10, a code below 2^10 (k <= 9).
#[inline(always)]
fn codes_scalar(v: &[u16], k: u32, lens: &mut [u16], codes: &mut [u16]) {
    let k16 = k as u16;
    let qm16 = ESC as u16;
    let lowmask = (1u16 << k16) - 1;
    for ((&v, l), c) in v.iter().zip(lens.iter_mut()).zip(codes.iter_mut()) {
        let q = v >> k16;
        // esc is all ones when the unary would reach ESC: then ESC zeros and the raw value.
        let esc = 0u16.wrapping_sub((q >= qm16) as u16);
        let len_n = q + 1 + k16;
        let code_n = (1 << k16) | (v & lowmask);
        *l = (len_n & !esc) | ((qm16 + BITS as u16) & esc);
        *c = (code_n & !esc) | (v & esc);
    }
}

#[inline(always)]
fn codes<const N: bool>(v: &[u16], k: u32, lens: &mut [u16], codes: &mut [u16]) {
    #[cfg(all(feature = "neon", target_arch = "aarch64"))]
    if N {
        let i0 = neon::codes(v, k, lens, codes);
        codes_scalar(&v[i0..], k, &mut lens[i0..], &mut codes[i0..]);
        return;
    }
    codes_scalar(v, k, lens, codes);
}

/// One row's block headers, codes and lengths.
/// SEL: the row has a row above, so each block takes the cheaper of m0 (Avg4) and m1 (MED) and sends one selection bit; without it m0 is Left and m1 is not read.
fn block_codes<const N: bool, const SEL: bool>(m0: &[u16], m1: &[u16], w: usize, kprev: &mut u32, rc: &mut RowCodes) {
    let mut x = 0;
    let mut j = 0;
    while x < w {
        // The block's length, not a guard: whole blocks, then the row's remainder (4080 = 127 x 32 + 16).
        let n = if w - x < BLOCK { w - x } else { BLOCK };
        let s0 = block_sum::<N>(&m0[x..x + n]);
        let (best, s) = if SEL {
            let s1 = block_sum::<N>(&m1[x..x + n]);
            // Strictly less: a tie keeps Avg4.
            let b = s1 < s0;
            (b as u32, if b { s1 } else { s0 })
        } else {
            (0, s0)
        };
        let chunk = if best == 1 { &m1[x..x + n] } else { &m0[x..x + n] };
        // A row's tail block may be short: one divide a row.
        let mean = if n == BLOCK { s >> BLOCK_LOG2 } else { s / n as u32 };
        let k = 31 - (mean | 1).leading_zeros();
        let (kc, kl) = k_code(k, *kprev);
        *kprev = k;
        rc.hcode[j] = ((best << kl) | kc) as u8;
        rc.hlen[j] = (SEL as u32 + kl) as u8;
        codes::<N>(chunk, k, &mut rc.lens[x..x + n], &mut rc.codes[x..x + n]);
        // Join neighbours into pairs (u64 lanes, a per-lane variable shift: NEON ushl); n is a multiple of 4.
        let (l2, c2) = (&rc.lens[x..x + n], &rc.codes[x..x + n]);
        for (((pc, pl), l), c) in rc.pcode[x >> 1..(x + n) >> 1].iter_mut().zip(rc.plen[x >> 1..(x + n) >> 1].iter_mut()).zip(l2.chunks_exact(2)).zip(c2.chunks_exact(2)) {
            *pc = ((c[0] as u64) << l[1]) | c[1] as u64;
            *pl = (l[0] + l[1]) as u8;
        }
        x += n;
        j += 1;
    }
}

/// The serial bit packing of one row: even blocks to stream A and odd blocks to stream B in lockstep, so the two writers' dependency chains interleave in the instruction stream.
/// Each code is at most 2^4 + 10 bits, a pair at most 52, inside put's 56.
fn pack_row(rc: &RowCodes, w: usize, bws: &mut [BitWriter; 2]) {
    let nb = w.div_ceil(BLOCK);
    let bp = BLOCK >> 1;
    let wp = w >> 1;
    let mut j = 0;
    while j < nb {
        let p0 = j * bp;
        // Block lengths in pairs as block_codes cut them: whole, the row's last one the remainder.
        let n0 = if wp - p0 < bp { wp - p0 } else { bp };
        let [a, b] = bws;
        if j + 1 < nb {
            let p1 = p0 + bp;
            let n1 = if wp - p1 < bp { wp - p1 } else { bp };
            a.put(rc.hcode[j] as u64, rc.hlen[j] as u32);
            b.put(rc.hcode[j + 1] as u64, rc.hlen[j + 1] as u32);
            let (ca, la) = (&rc.pcode[p0..p0 + n0], &rc.plen[p0..p0 + n0]);
            let (cb, lb) = (&rc.pcode[p1..p1 + n1], &rc.plen[p1..p1 + n1]);
            for (((&c0, &l0), &c1), &l1) in ca.iter().zip(la).zip(cb).zip(lb) {
                a.put(c0, l0 as u32);
                b.put(c1, l1 as u32);
            }
            // Block j is whole wherever a block follows it; the second may be the row's short tail.
            for (&c0, &l0) in ca[n1..].iter().zip(&la[n1..]) {
                a.put(c0, l0 as u32);
            }
            j += 2;
        } else {
            // The row's last block of an odd count: even, so stream A.
            a.put(rc.hcode[j] as u64, rc.hlen[j] as u32);
            for (&c0, &l0) in rc.pcode[p0..p0 + n0].iter().zip(&rc.plen[p0..p0 + n0]) {
                a.put(c0, l0 as u32);
            }
            j += 1;
        }
    }
}

/// One row's residuals and selections from the two streams, block j from stream j & 1.
fn block_row_decode<const SEL: bool>(seq: &mut [u16], sel: &mut [u8], kprev: &mut u32, brs: &mut [BitReader; 2]) {
    for (j, (chunk, s)) in seq.chunks_mut(BLOCK).zip(sel.iter_mut()).enumerate() {
        let br = &mut brs[j & 1];
        br.refill();
        *s = br.get(SEL as u32) as u8;
        let k = get_k(br, *kprev);
        *kprev = k;
        for v in chunk.iter_mut() {
            br.refill();
            let q = br.peek_lz();
            if q >= ESC {
                br.skip(ESC);
                *v = br.get(BITS) as u16;
            } else {
                br.skip(q + 1);
                *v = ((q << k) | br.get(k)) as u16;
            }
        }
    }
}

// ---------------------------------------------------------------- bands

/// Encode one band of `rows` packed RAW10 rows (`stride` bytes a row, `width` samples) into `out`: stream A at out[0..la], stream B right behind it at out[la..la + lb]; returns (la, lb).
/// `out` must hold `max_band_len(width, rows)` bytes; the bytes past la + lb are scratch.
/// Panics if width is not a multiple of 4 or outside MIN_W..=MAX_W, if rows is 0, if src is short, or if out is shorter than max_band_len: a caller's bug, loud.
pub fn encode_band(src: &[u8], stride: usize, width: usize, rows: usize, cfa: Cfa, work: &mut Work, out: &mut [u8]) -> (usize, usize) {
    encode_band_with::<NEON>(src, stride, width, rows, cfa, work, out)
}

/// Whether this build's `encode_band` takes the NEON lane steps (the `neon` feature on aarch64).
pub const NEON: bool = cfg!(all(feature = "neon", target_arch = "aarch64"));

/// `encode_band` with the scalar lane steps whatever the build: the reference a NEON build is checked against byte for byte where it runs (std/rawrec job 4).
pub fn encode_band_scalar(src: &[u8], stride: usize, width: usize, rows: usize, cfa: Cfa, work: &mut Work, out: &mut [u8]) -> (usize, usize) {
    encode_band_with::<false>(src, stride, width, rows, cfa, work, out)
}

fn encode_band_with<const N: bool>(src: &[u8], stride: usize, width: usize, rows: usize, cfa: Cfa, work: &mut Work, out: &mut [u8]) -> (usize, usize) {
    let rb = row_bytes(width);
    assert!(width & 3 == 0 && width >= MIN_W && width <= MAX_W, "width {width}");
    assert!(rows >= 1 && src.len() >= (rows - 1) * stride + rb, "short band source");
    let half = max_stream_len(width, rows);
    assert!(out.len() >= 2 * half, "output below max_band_len");
    let (sa, sb) = out[..2 * half].split_at_mut(half);
    let mut bws = [BitWriter::new(sa), BitWriter::new(sb)];
    let mut kprev = K0;
    for y in 0..rows {
        let ci = y & 3;
        let dy = cfa.dy(y);
        let has_up = y >= dy;
        // Without a row above ui is never read; the wrap keeps it a valid ring index all the same.
        let ui = y.wrapping_sub(dy) & 3;
        unpack_row(&src[y * stride..y * stride + rb], &mut work.rows[ci][PAD..PAD + width]);
        left_pad(&mut work.rows, ci, has_up.then_some(ui));
        let [m0, m1] = &mut work.m;
        if has_up {
            residuals::<N, AVG4>(cfa, &work.rows[ci], &work.rows[ui], width, m0);
            residuals::<N, MED>(cfa, &work.rows[ci], &work.rows[ui], width, m1);
            block_codes::<N, true>(&m0[..width], &m1[..width], width, &mut kprev, &mut work.rc);
        } else {
            // A band's first rows of each phase have no row above: Left only, no selection sent.
            residuals::<N, LEFT>(cfa, &work.rows[ci], &work.rows[ci], width, m0);
            block_codes::<N, false>(&m0[..width], &m0[..width], width, &mut kprev, &mut work.rc);
        }
        pack_row(&work.rc, width, &mut bws);
        right_pad(&mut work.rows[ci], width);
    }
    let [a, b] = bws;
    let la = a.finish();
    let lb = b.finish();
    // Stream B moves down behind A.
    out.copy_within(half..half + lb, la);
    (la, lb)
}

/// Decode one band from its two streams into `dst` (u16 samples, `dst_stride` samples a row); returns the bytes of A and of B it used, la and lb for a whole stream.
/// Panics on a bad geometry or a short dst; a corrupt stream decodes to wrong samples, never to a read outside a slice.
pub fn decode_band(a: &[u8], b: &[u8], width: usize, rows: usize, cfa: Cfa, work: &mut Work, dst: &mut [u16], dst_stride: usize) -> (usize, usize) {
    assert!(width & 3 == 0 && width >= MIN_W && width <= MAX_W, "width {width}");
    assert!(rows >= 1 && dst.len() >= (rows - 1) * dst_stride + width, "short band destination");
    let mut brs = [BitReader::new(a), BitReader::new(b)];
    let mut kprev = K0;
    for y in 0..rows {
        let ci = y & 3;
        let dy = cfa.dy(y);
        let has_up = y >= dy;
        // Without a row above ui is never read; the wrap keeps it a valid ring index all the same.
        let ui = y.wrapping_sub(dy) & 3;
        left_pad(&mut work.rows, ci, has_up.then_some(ui));
        if has_up {
            block_row_decode::<true>(&mut work.m[0][..width], &mut work.sel, &mut kprev, &mut brs);
            for x in 0..width {
                let i = PAD + x;
                let dl = cfa.dxl(x);
                let a = work.rows[ci][i - dl] as i16;
                let b = work.rows[ui][i] as i16;
                let c = work.rows[ui][i - dl] as i16;
                let d = work.rows[ui][i + cfa.dxr(x)] as i16;
                let pv = if work.sel[x >> BLOCK_LOG2] == 0 { predict::<AVG4>(a, b, c, d) } else { predict::<MED>(a, b, c, d) };
                work.rows[ci][i] = ((pv + unzig(work.m[0][x])) & MASK) as u16;
            }
        } else {
            block_row_decode::<false>(&mut work.m[0][..width], &mut work.sel, &mut kprev, &mut brs);
            for x in 0..width {
                let i = PAD + x;
                let pv = work.rows[ci][i - cfa.dxl(x)] as i16;
                work.rows[ci][i] = ((pv + unzig(work.m[0][x])) & MASK) as u16;
            }
        }
        right_pad(&mut work.rows[ci], width);
        dst[y * dst_stride..y * dst_stride + width].copy_from_slice(&work.rows[ci][PAD..PAD + width]);
    }
    (brs[0].consumed(), brs[1].consumed())
}
