//! raw-codec: a prototype lossless band coder for 10-bit camera RAW mosaics, MIPI RAW10 packed in, u16 samples out.
//! A prototype for measuring, not a format: the band's bitstream carries no header, the caller keeps the geometry and the parameters.
//! no_std and no alloc: every function takes slices and a caller's `Work`, so the same code links into a ferros std app or payload.
//!
//! The pipeline per row: unpack the packed row (fused, SWAR, optionally thru a 1024-entry table for the shot-noise mode), predict each sample from its own colour plane, wrap the residual to 10 bits, zigzag, then code.
//! Same-colour neighbours: Bayer at distance 2 (a left, b up, c up-left, d up-right); the quad array's 2 x 2 same-colour blocks use the nearest same-colour sample, 1 inside a block and 3 across to the previous block, in both directions.
//! Two coders:
//! - `Coder::Block`: one Rice parameter k per block of 2^5 (or 2^4, 2^6) residuals, from the block's sum by clz, sent as a short delta code; unary capped at 2^esc_log2 then the raw 10 bits.
//!   The predictor may be fixed or chosen per block from two or four candidates by least residual sum (`Predictor::Adapt2`, the default: Avg4 or Med, one bit a block).
//!   The residual, choice and k steps are fixed-width lane code with no data-dependent branches; only the bit packing is serial, two codes a put, even and odd blocks into two streams packed in lockstep.
//! - `Coder::Loco`: JPEG-LS (LOCO-I) regular mode, per-pixel context modelling (365 gradient contexts per CFA phase, bias correction, running A/N), the ratio reference; serial.
//! Bands are independent: each starts its own rows, k and contexts, so cores can split a frame by bands.
//! A band must start on a row that is a multiple of the CFA period (the caller's bands are powers of two of at least 4 rows).
#![no_std]

/// Sample depth.
pub const BITS: u32 = 10;
/// Widest row a `Work` holds (the quad array's 8160 fits).
pub const MAX_W: usize = 1 << 13;
const PAD: usize = 4;
const ROW: usize = MAX_W + 2 * PAD;
const MASK: i16 = (1 << BITS) - 1;
const HALF: i16 = 1 << (BITS - 1);
/// Value of the missing left neighbour in a band's first rows.
const EDGE: u16 = 1 << 6;
const MAX_BLOCK_LOG2: u32 = 6;
/// k a band starts from for the delta code.
const K0: u32 = 2;
/// JPEG-LS contexts per CFA phase after sign merging: (9^3 + 1) / 2.
const NQ: usize = 365;
/// Phases: 4 for Bayer, 16 for the quad array (4 colour blocks x 4 positions in the block).
const NPHASE: usize = 1 << 4;
const NCTX: usize = NQ * NPHASE;
/// JPEG-LS default gradient thresholds for a 10-bit range (FACTOR = (1023 + 128) >> 8 = 4): T1 = 6, T2 = 19, T3 = 72.
const T1: i32 = 6;
const T2: i32 = 19;
const T3: i32 = 72;
/// JPEG-LS halving period of the context counts.
const RESET: u16 = 1 << 6;
/// JPEG-LS code length limit 2 * (bpp + max(8, bpp)) = 40 for 10 bits.
const LIMIT: u32 = 40;

/// Colour filter period: `Bayer` a 2 x 2 cell, `Quad` the 4 x 4 cell of 2 x 2 same-colour blocks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cfa {
    Bayer,
    Quad,
}

impl Cfa {
    /// Distance from row y to the nearest earlier row of the same colour at the same column.
    /// Bayer: 2. Quad: inside a 2 x 2 block the row above (1) for the block's second row, the previous block's second row (3) for its first.
    #[inline(always)]
    fn dy(self, yb: usize) -> usize {
        match self {
            Cfa::Bayer => 2,
            Cfa::Quad => 3 - ((yb & 1) << 1),
        }
    }
    #[inline(always)]
    fn phase(self, x: usize, yb: usize) -> usize {
        match self {
            Cfa::Bayer => (x & 1) | ((yb & 1) << 1),
            Cfa::Quad => (x & 3) | ((yb & 3) << 2),
        }
    }
    /// Left same-colour distance at column x: Bayer 2; quad 1 for a block's second column, 3 for its first.
    #[inline(always)]
    fn dxl(self, x: usize) -> usize {
        match self {
            Cfa::Bayer => 2,
            Cfa::Quad => 3 - ((x & 1) << 1),
        }
    }
    /// Right same-colour distance at column x: Bayer 2; quad 1 for a block's first column, 3 for its second.
    #[inline(always)]
    fn dxr(self, x: usize) -> usize {
        match self {
            Cfa::Bayer => 2,
            Cfa::Quad => 1 + ((x & 1) << 1),
        }
    }
}

/// Predictor for the block coder (the LOCO coder always runs MED with its bias correction).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Predictor {
    /// The same-colour left neighbour a.
    Left,
    /// (a + b) / 2 with b the same-colour neighbour above.
    Avg,
    /// LOCO-I median edge detector: median(a, b, a + b - c), c the same-colour up-left.
    Med,
    /// (a + b + c + d) / 4 with d the same-colour up-right: the most noise averaging, biased on gradients.
    Avg4,
    /// a + (d - c) / 2: exact on any plane, noise gain 3/2 like Avg.
    Grad,
    /// b, the same-colour neighbour above.
    Up,
    /// Per block the cheaper of Avg4 and Med, one bit a block.
    Adapt2,
    /// Per block the cheapest of Avg4, Med, Avg and Up, two bits a block.
    Adapt4,
}

impl Predictor {
    /// The fixed predictors a block may choose from, as predict's ids.
    fn candidates(self) -> &'static [u8] {
        match self {
            Predictor::Left => &[0],
            Predictor::Avg => &[1],
            Predictor::Med => &[2],
            Predictor::Avg4 => &[3],
            Predictor::Grad => &[4],
            Predictor::Up => &[5],
            Predictor::Adapt2 => &[3, 2],
            Predictor::Adapt4 => &[3, 2, 1, 5],
        }
    }
}

const MAX_CAND: usize = 1 << 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coder {
    Block,
    Loco,
}

/// How the block coder picks k.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KPick {
    /// k = floor(log2(block mean)) by clz, no search.
    Clz,
    /// The cheapest of the three k around the clz guess, by summed code length.
    Best3,
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub predictor: Predictor,
    pub coder: Coder,
    /// Block of 2^block_log2 residuals per k, 4 to 6.
    pub block_log2: u32,
    /// Unary capped at 2^esc_log2 zeros, then the raw 10 bits.
    pub esc_log2: u32,
    /// Blocks run over one colour column class of the row, then the other (same-colour blocks), instead of the row's own order.
    pub split: bool,
    pub kpick: KPick,
    /// The block coder writes even blocks and odd blocks to two bit streams (the band: the first stream's length in 4 bytes, the first, the second), so two packing chains run side by side.
    pub two_streams: bool,
}

impl Params {
    pub const DEFAULT: Params = Params { predictor: Predictor::Adapt2, coder: Coder::Block, block_log2: 5, esc_log2: 4, split: false, kpick: KPick::Clz, two_streams: true };
}

/// Scratch for one band at a time: four row buffers (a quad row's same-colour row above is 3 back), a residual row per candidate predictor, a row's codes and the LOCO contexts.
/// About 210 KiB; the caller owns it (a Box on a std host, a static or a stack of its own elsewhere).
pub struct Work {
    rows: [[u16; ROW]; 4],
    m: [[u16; MAX_W]; MAX_CAND],
    t: [u16; MAX_W],
    sel: [u8; MAX_W >> 2],
    rc: RowCodes,
    ca: [u32; NCTX],
    cb: [i32; NCTX],
    cc: [i16; NCTX],
    cn: [u16; NCTX],
}

impl Work {
    pub const fn new() -> Work {
        Work {
            rows: [[0; ROW]; 4],
            m: [[0; MAX_W]; MAX_CAND],
            t: [0; MAX_W],
            sel: [0; MAX_W >> 2],
            rc: RowCodes { lens: [0; MAX_W], codes: [0; MAX_W], pcode: [0; MAX_W >> 1], plen: [0; MAX_W >> 1], hcode: [0; MAX_W >> 2], hlen: [0; MAX_W >> 2] },
            ca: [0; NCTX],
            cb: [0; NCTX],
            cc: [0; NCTX],
            cn: [0; NCTX],
        }
    }
    fn reset_contexts(&mut self) {
        // JPEG-LS initial A = max(2, (RANGE + 32) >> 6) = 16 for RANGE 1024.
        self.ca.fill(1 << 4);
        self.cb.fill(0);
        self.cc.fill(0);
        self.cn.fill(1);
    }
}

impl Default for Work {
    fn default() -> Self {
        Work::new()
    }
}

/// Bytes one stream may take: every sample codes in at most 57 bits plus a block's header, so 8 bytes a sample, plus the writer's 8-byte store past the end.
const fn max_stream_len(width: usize, rows: usize) -> usize {
    width * rows * 8 + (1 << 6)
}

/// Bytes the caller's output slice must hold for one band: the first stream's length, and room for each stream to take the worst case while both are written.
pub const fn max_encoded_len(width: usize, rows: usize) -> usize {
    4 + 2 * max_stream_len(width, rows)
}

// ---------------------------------------------------------------- bits

/// A cursor, the accumulator and its bit count: three registers a writer, so two writers in lockstep fit x86's sixteen without spilling.
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
        // SAFETY: encode_band asserted out.len() >= max_encoded_len and gave each writer max_stream_len bytes, 8 a sample plus 64; the cursor is whole bytes written past start, at most 57 + 8 header bits a sample so far, so cur + 8 stays inside the writer's slice.
        unsafe { (self.cur as *mut u64).write_unaligned(self.acc.to_be()) };
        let bytes = self.n >> 3;
        // SAFETY: the same bound, cur advances only over bytes just stored.
        self.cur = unsafe { self.cur.add(bytes as usize) };
        // n <= 7 + 56 = 63 here, so the shift is at most 56.
        self.acc <<= bytes << 3;
        self.n &= 7;
    }
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
        // WHY: the stream is external input and the last bytes of a band sit at the end of the caller's slice; an 8-byte read there would run past it.
        // Refill also claims bytes ahead of use, so near the end pos can pass the slice's length (those bytes read as zeros).
        // PROOF: without the check the read at pos > len - 8 leaves the slice (the caller's band is exactly its coded bytes); the zero fill is the stream's padding, and a corrupt stream decodes to wrong samples, never to a read outside the slice.
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
    fn consumed(&self) -> usize {
        (self.pos * 8 - self.n as usize + 7) >> 3
    }
}

// ---------------------------------------------------------------- rows

/// Unpack one MIPI RAW10 row: 4 samples in 5 bytes, the high 8 bits in bytes 0 to 3, the four 2-bit lows in byte 4 (sample k at bits 2k, 2k + 1).
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

/// The same thru a 1024-entry table (the shot-noise mode's forward transform).
#[inline(always)]
fn unpack_row_lut(src: &[u8], dst: &mut [u16], lut: &[u16; 1 << BITS]) {
    // Every index is 8 high bits << 2 | 2 low bits, so below 1024: the table's length is the sample's range, the & is free and lets the compiler drop its check.
    let m = (1usize << BITS) - 1;
    for (g, d) in src.chunks_exact(5).zip(dst.chunks_exact_mut(4)) {
        let lo = g[4] as usize;
        d[0] = lut[(((g[0] as usize) << 2) | (lo & 3)) & m];
        d[1] = lut[(((g[1] as usize) << 2) | ((lo >> 2) & 3)) & m];
        d[2] = lut[(((g[2] as usize) << 2) | ((lo >> 4) & 3)) & m];
        d[3] = lut[(((g[3] as usize) << 2) | (lo >> 6)) & m];
    }
}

/// The left pad of a row is the same-phase samples of the row above (period 4 divides the pad), or EDGE in a band's first rows; the right pad repeats the row's own last 4 samples.
/// Encoder and decoder run the same, so the pads are part of the definition, not a guess.
fn left_pad(rows: &mut [[u16; ROW]; 4], ci: usize, ui: Option<usize>) {
    let mut p = [EDGE; PAD];
    if let Some(ui) = ui {
        p.copy_from_slice(&rows[ui][PAD..PAD + PAD]);
    }
    rows[ci][..PAD].copy_from_slice(&p);
}

fn right_pad(row: &mut [u16; ROW], w: usize) {
    row.copy_within(PAD + w - PAD..PAD + w, PAD + w);
}

// ---------------------------------------------------------------- prediction

/// LOCO-I MED as median(a, b, a + b - c); the min and max are the predictor's definition, not a range guard.
#[inline(always)]
fn med(a: i16, b: i16, c: i16) -> i16 {
    let p = a + b - c;
    p.max(a.min(b)).min(a.max(b))
}

#[inline(always)]
fn predict<const P: u8>(a: i16, b: i16, c: i16, d: i16) -> i16 {
    match P {
        0 => a,
        1 => (a + b) >> 1,
        2 => med(a, b, c),
        3 => (a + b + c + d) >> 2,
        // May leave 0..1023 on a steep edge: the residual's wrap is mod 1024, so any prediction round-trips exactly.
        4 => a + ((d - c) >> 1),
        _ => b,
    }
}

/// The same with the predictor chosen at run time (the decoder's per-block choice).
#[inline(always)]
fn predict_dyn(id: u8, a: i16, b: i16, c: i16, d: i16) -> i16 {
    match id {
        0 => predict::<0>(a, b, c, d),
        1 => predict::<1>(a, b, c, d),
        2 => predict::<2>(a, b, c, d),
        3 => predict::<3>(a, b, c, d),
        4 => predict::<4>(a, b, c, d),
        _ => predict::<5>(a, b, c, d),
    }
}

/// Residual wrapped to the 10-bit ring (x - pred mod 1024 in -512..511) and zigzagged to 0..1023.
/// The wrap is exact: the decoder adds it back mod 1024, and every sample is in 0..1023.
#[inline(always)]
fn zig(x: i16, pred: i16) -> u16 {
    let e = ((x - pred + HALF) & MASK) - HALF;
    ((e << 1) ^ (e >> 15)) as u16
}

#[inline(always)]
fn unzig(m: u16) -> i16 {
    ((m >> 1) as i16) ^ -((m & 1) as i16)
}

/// Residuals of a whole row, fixed-width lane code: every sample's neighbours are plain shifted loads of the row and the row above.
/// DXE / DXO: the left same-colour distance at even / odd columns (Bayer 2, 2; quad 3, 1); DRE / DRO the right one in the row above (Bayer 2, 2; quad 1, 3).
#[inline(always)]
fn residuals_t<const P: u8, const DXE: usize, const DXO: usize, const DRE: usize, const DRO: usize>(cur: &[u16; ROW], up: &[u16; ROW], w: usize, m: &mut [u16]) {
    let c0 = &cur[PAD..PAD + w];
    let le = &cur[PAD - DXE..PAD - DXE + w];
    let lo = &cur[PAD - DXO..PAD - DXO + w];
    let u0 = &up[PAD..PAD + w];
    let ue = &up[PAD - DXE..PAD - DXE + w];
    let uo = &up[PAD - DXO..PAD - DXO + w];
    let re = &up[PAD + DRE..PAD + DRE + w];
    let ro = &up[PAD + DRO..PAD + DRO + w];
    if DXE == DXO {
        // Bayer: one distance, plain shifted slices.
        for (((((m, &c), &l), &u), &ul), &ur) in m[..w].iter_mut().zip(c0).zip(le).zip(u0).zip(ue).zip(re) {
            *m = zig(c as i16, predict::<P>(l as i16, u as i16, ul as i16, ur as i16));
        }
        return;
    }
    // Quad: both distances loaded, the column's parity picks one (a fixed alternating blend mask in lane code).
    let it = m[..w].iter_mut().zip(c0).zip(le).zip(lo).zip(u0).zip(ue).zip(uo).zip(re).zip(ro).enumerate();
    for (i, ((((((((m, &c), &le), &lo), &u), &ue), &uo), &re), &ro)) in it {
        let odd = i & 1 == 1;
        let a = if odd { lo } else { le };
        let ul = if odd { uo } else { ue };
        let ur = if odd { ro } else { re };
        *m = zig(c as i16, predict::<P>(a as i16, u as i16, ul as i16, ur as i16));
    }
}

fn residuals_p<const P: u8>(cfa: Cfa, cur: &[u16; ROW], up: &[u16; ROW], w: usize, m: &mut [u16]) {
    match cfa {
        Cfa::Bayer => residuals_t::<P, 2, 2, 2, 2>(cur, up, w, m),
        Cfa::Quad => residuals_t::<P, 3, 1, 1, 3>(cur, up, w, m),
    }
}

fn residuals(cfa: Cfa, id: u8, cur: &[u16; ROW], up: &[u16; ROW], w: usize, m: &mut [u16]) {
    match id {
        0 => residuals_p::<0>(cfa, cur, up, w, m),
        1 => residuals_p::<1>(cfa, cur, up, w, m),
        2 => residuals_p::<2>(cfa, cur, up, w, m),
        3 => residuals_p::<3>(cfa, cur, up, w, m),
        4 => residuals_p::<4>(cfa, cur, up, w, m),
        _ => residuals_p::<5>(cfa, cur, up, w, m),
    }
}

/// Reorder a row's residuals into its two colour column classes (Bayer: even columns then odd; quad: columns 0, 1 mod 4 then 2, 3 mod 4).
fn split_row(cfa: Cfa, m: &[u16], t: &mut [u16], w: usize) {
    let h = w >> 1;
    let (t0, t1) = t[..w].split_at_mut(h);
    match cfa {
        Cfa::Bayer => {
            for ((p, a), b) in m[..w].chunks_exact(2).zip(t0.iter_mut()).zip(t1.iter_mut()) {
                *a = p[0];
                *b = p[1];
            }
        }
        Cfa::Quad => {
            for ((p, a), b) in m[..w].chunks_exact(4).zip(t0.chunks_exact_mut(2)).zip(t1.chunks_exact_mut(2)) {
                a.copy_from_slice(&p[..2]);
                b.copy_from_slice(&p[2..]);
            }
        }
    }
}

fn unsplit_row(cfa: Cfa, t: &[u16], m: &mut [u16], w: usize) {
    let h = w >> 1;
    let (t0, t1) = t[..w].split_at(h);
    match cfa {
        Cfa::Bayer => {
            for ((p, a), b) in m[..w].chunks_exact_mut(2).zip(t0.iter()).zip(t1.iter()) {
                p[0] = *a;
                p[1] = *b;
            }
        }
        Cfa::Quad => {
            for ((p, a), b) in m[..w].chunks_exact_mut(4).zip(t0.chunks_exact(2)).zip(t1.chunks_exact(2)) {
                p[..2].copy_from_slice(a);
                p[2..].copy_from_slice(b);
            }
        }
    }
}

// ---------------------------------------------------------------- block coder

/// k delta code per block: same k "1", k + 1 "010", k - 1 "011", else "00" and k in 4 bits; returns (code, length).
#[inline(always)]
fn k_code(k: u32, kprev: u32) -> (u32, u32) {
    // The signed step from the previous k (both at most 15), not an overflow guard.
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

/// The block coder's per-row scratch: every sample's code and length, every block's header.
struct RowCodes {
    lens: [u16; MAX_W],
    codes: [u16; MAX_W],
    /// Two codes joined, the first above the second, and their summed length: one put each.
    pcode: [u64; MAX_W >> 1],
    plen: [u8; MAX_W >> 1],
    hcode: [u16; MAX_W >> 2],
    hlen: [u8; MAX_W >> 2],
}

/// Code one row's residual sequence in blocks.
/// `seqs`: the row's residuals under each candidate predictor (one for a fixed predictor); per block the candidate with the least sum is taken and its index sent in `sel_bits` bits.
/// First pass, per block: the sums (lane adds), k by clz of the mean, the header, then each residual's code and length (lane code, no branch).
/// Second pass, the serial bit packing: with two streams, even blocks to the first and odd blocks to the second in lockstep, so the two writers' dependency chains interleave in the instruction stream (an out-of-order core overlaps them, an in-order core issues them side by side).
fn block_row(seqs: &[&[u16]], sel_bits: u32, p: &Params, kprev: &mut u32, rc: &mut RowCodes, bws: &mut [BitWriter; 2]) {
    let bl = 1usize << p.block_log2;
    let w = seqs[0].len();
    block_codes(seqs, sel_bits, p, kprev, rc);
    let nb = w.div_ceil(bl);
    // Pairs: each code is at most 2^4 + 10 bits, a pair at most 52, inside put's 56; a block's length is even (widths are multiples of 4, blocks powers of two).
    let bp = bl >> 1;
    let wp = w >> 1;
    let mut j = 0;
    while j < nb {
        let p0 = j * bp;
        // Block lengths as block_codes cut them: whole, the row's last one the remainder.
        let n0 = bp.min(wp - p0);
        if p.two_streams && j + 1 < nb {
            let p1 = p0 + bp;
            let n1 = bp.min(wp - p1);
            let [a, b] = bws;
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
            let a = &mut bws[0];
            a.put(rc.hcode[j] as u64, rc.hlen[j] as u32);
            for (&c0, &l0) in rc.pcode[p0..p0 + n0].iter().zip(&rc.plen[p0..p0 + n0]) {
                a.put(c0, l0 as u32);
            }
            j += 1;
        }
    }
}

fn block_codes(seqs: &[&[u16]], sel_bits: u32, p: &Params, kprev: &mut u32, rc: &mut RowCodes) {
    let bl = 1usize << p.block_log2;
    let qm = 1u32 << p.esc_log2;
    let w = seqs[0].len();
    let mut x = 0;
    let mut j = 0;
    while x < w {
        // The block length, not a guard: whole blocks, then the row's remainder (4080 = 127 x 32 + 16).
        let n = bl.min(w - x);
        let mut best = 0usize;
        let mut s = u32::MAX;
        for (i, q) in seqs.iter().enumerate() {
            let si: u32 = q[x..x + n].iter().map(|&v| v as u32).sum();
            // Strictly less: ties keep the earlier candidate, as the decoder needs nothing to know.
            if si < s {
                s = si;
                best = i;
            }
        }
        let chunk = &seqs[best][x..x + n];
        // A row's tail block may be short (4080 is not a multiple of 32): one divide a row.
        let mean = if n == bl { s >> p.block_log2 } else { s / n as u32 };
        let mut k = 31 - (mean | 1).leading_zeros();
        if p.kpick == KPick::Best3 {
            // Three candidate k around the guess: k0 = guess - 1 (0 at the bottom), k0 + 1, k0 + 2; cost = unary bits + n * (k + 1).
            let k0 = k - (k > 0) as u32;
            let (mut s0, mut s1, mut s2) = (0u32, 0u32, 0u32);
            for &v in chunk {
                let v = v as u32;
                s0 += v >> k0;
                s1 += v >> (k0 + 1);
                s2 += v >> (k0 + 2);
            }
            let nn = n as u32;
            let c0 = s0 + nn * (k0 + 1);
            let c1 = s1 + nn * (k0 + 2);
            let c2 = s2 + nn * (k0 + 3);
            k = k0;
            let mut best = c0;
            if c1 < best {
                best = c1;
                k = k0 + 1;
            }
            if c2 < best {
                k = k0 + 2;
            }
        }
        let (kc, kl) = k_code(k, *kprev);
        *kprev = k;
        rc.hcode[j] = ((best as u32) << kl | kc) as u16;
        rc.hlen[j] = (sel_bits + kl) as u8;
        // u16 lanes: a residual is below 2^10, a length at most 2^4 + 10, a code below 2^12 (k <= 11).
        let k16 = k as u16;
        let qm16 = qm as u16;
        let lowmask = (1u16 << k16) - 1;
        for ((&v, l), c) in chunk.iter().zip(rc.lens[x..x + n].iter_mut()).zip(rc.codes[x..x + n].iter_mut()) {
            let q = v >> k16;
            // esc is all ones when the unary would reach qm: then qm zeros and the raw value.
            let esc = 0u16.wrapping_sub((q >= qm16) as u16);
            let len_n = q + 1 + k16;
            let code_n = (1 << k16) | (v & lowmask);
            *l = (len_n & !esc) | ((qm16 + BITS as u16) & esc);
            *c = (code_n & !esc) | (v & esc);
        }
        // Join neighbours into pairs (u64 lanes, a per-lane variable shift: AVX2 vpsllvq, NEON ushl).
        let (l2, c2) = (&rc.lens[x..x + n], &rc.codes[x..x + n]);
        for (((pc, pl), l), c) in rc.pcode[x >> 1..(x + n) >> 1].iter_mut().zip(rc.plen[x >> 1..(x + n) >> 1].iter_mut()).zip(l2.chunks_exact(2)).zip(c2.chunks_exact(2)) {
            *pc = ((c[0] as u64) << l[1]) | c[1] as u64;
            *pl = (l[0] + l[1]) as u8;
        }
        x += n;
        j += 1;
    }
}

fn block_row_decode(seq: &mut [u16], sel: &mut [u8], sel_bits: u32, p: &Params, kprev: &mut u32, brs: &mut [BitReader; 2]) {
    let bl = 1usize << p.block_log2;
    let qm = 1u32 << p.esc_log2;
    for (j, (chunk, s)) in seq.chunks_mut(bl).zip(sel.iter_mut()).enumerate() {
        // Block j's stream: even blocks in the first, odd in the second with two streams (a row's last block of an odd count is even, so in the first, as the encoder put it), all in the first with one.
        let br = &mut brs[(p.two_streams & (j & 1 == 1)) as usize];
        br.refill();
        *s = br.get(sel_bits) as u8;
        let k = get_k(br, *kprev);
        *kprev = k;
        for v in chunk.iter_mut() {
            br.refill();
            let q = br.peek_lz();
            if q >= qm {
                br.skip(qm);
                *v = br.get(BITS) as u16;
            } else {
                br.skip(q + 1);
                *v = ((q << k) | br.get(k)) as u16;
            }
        }
    }
}

// ---------------------------------------------------------------- LOCO-I / JPEG-LS regular mode

#[inline(always)]
fn quant(d: i32) -> i32 {
    (d >= 1) as i32 + (d >= T1) as i32 + (d >= T2) as i32 + (d >= T3) as i32 - (d <= -1) as i32 - (d <= -T1) as i32 - (d <= -T2) as i32 - (d <= -T3) as i32
}

/// Context index (0..365, the sign merged so the first nonzero gradient is positive) and the sign.
#[inline(always)]
fn loco_ctx(a: i32, b: i32, c: i32, d: i32) -> (usize, i32) {
    let q = quant(d - b) * 81 + quant(b - c) * 9 + quant(c - a);
    let s = (q >> 31) | 1;
    ((q * s) as usize, s)
}

/// Smallest k with N << k >= A, by clz: the bit-length difference, plus one if still short.
#[inline(always)]
fn loco_k(a: u32, n: u32) -> u32 {
    // max with 0: A < N gives a negative bit-length difference and k = 0 is the definition's answer there.
    let k = ((n.leading_zeros() as i32) - (a.leading_zeros() as i32)).max(0) as u32;
    k + ((n << k) < a) as u32
}

/// JPEG-LS context update after coding error e (the sign-merged, wrapped one).
#[inline(always)]
fn loco_update(w: &mut Work, ctx: usize, e: i32) {
    let mut b = w.cb[ctx] + e;
    let mut a = w.ca[ctx] + e.unsigned_abs();
    let mut n = w.cn[ctx];
    if n == RESET {
        a >>= 1;
        b = if b >= 0 { b >> 1 } else { -((1 - b) >> 1) };
        n >>= 1;
    }
    n += 1;
    let n32 = n as i32;
    let mut c = w.cc[ctx];
    // The bias correction C stays in -128..127 and B in -N+1..0: JPEG-LS's own bounds (A.6.2), part of the model.
    if b <= -n32 {
        b += n32;
        if c > -128 {
            c -= 1;
        }
        if b <= -n32 {
            b = -n32 + 1;
        }
    } else if b > 0 {
        b -= n32;
        if c < 127 {
            c += 1;
        }
        if b > 0 {
            b = 0;
        }
    }
    w.ca[ctx] = a;
    w.cb[ctx] = b;
    w.cc[ctx] = c;
    w.cn[ctx] = n;
}

/// One sample's LOCO prediction: context, sign, corrected prediction, k.
#[inline(always)]
fn loco_pred(w: &Work, phase: usize, a: i32, b: i32, c: i32, d: i32) -> (usize, i32, i32, u32) {
    let (q, s) = loco_ctx(a, b, c, d);
    let ctx = phase * NQ + q;
    let px = med(a as i16, b as i16, c as i16) as i32 + s * w.cc[ctx] as i32;
    // JPEG-LS clips the corrected prediction to the sample range (A.4.2): the bias can push it outside, and the wrap below assumes it inside.
    let px = px.clamp(0, MASK as i32);
    let k = loco_k(w.ca[ctx], w.cn[ctx] as u32);
    (ctx, s, px, k)
}

#[inline(always)]
fn loco_neighbours(cfa: Cfa, cur: &[u16; ROW], up: &[u16; ROW], has_up: bool, x: usize) -> (i32, i32, i32, i32) {
    let i = PAD + x;
    let a = cur[i - cfa.dxl(x)] as i32;
    if has_up {
        (a, up[i] as i32, up[i - cfa.dxl(x)] as i32, up[i + cfa.dxr(x)] as i32)
    } else {
        (a, a, a, a)
    }
}

fn loco_row_encode(w: &mut Work, cfa: Cfa, ci: usize, ui: usize, has_up: bool, yb: usize, width: usize, bw: &mut BitWriter) {
    for x in 0..width {
        let (a, b, c, d) = loco_neighbours(cfa, &w.rows[ci], &w.rows[ui], has_up, x);
        let xv = w.rows[ci][PAD + x] as i32;
        let (ctx, s, px, k) = loco_pred(w, cfa.phase(x, yb), a, b, c, d);
        let e = s * (xv - px);
        let e = ((e + HALF as i32) & MASK as i32) - HALF as i32;
        let alt = k == 0 && 2 * w.cb[ctx] <= -(w.cn[ctx] as i32);
        let me: i32 = if alt {
            if e >= 0 { 2 * e + 1 } else { -2 * (e + 1) }
        } else if e >= 0 {
            2 * e
        } else {
            -2 * e - 1
        };
        let me = me as u32;
        let q = me >> k;
        if q < LIMIT - BITS - 1 {
            bw.put(((1u32 << k) | (me & ((1u32 << k) - 1))) as u64, q + 1 + k);
        } else {
            bw.put(1, LIMIT - BITS);
            bw.put((me - 1) as u64, BITS);
        }
        loco_update(w, ctx, e);
    }
}

fn loco_row_decode(w: &mut Work, cfa: Cfa, ci: usize, ui: usize, has_up: bool, yb: usize, width: usize, br: &mut BitReader) {
    for x in 0..width {
        let (a, b, c, d) = loco_neighbours(cfa, &w.rows[ci], &w.rows[ui], has_up, x);
        let (ctx, s, px, k) = loco_pred(w, cfa.phase(x, yb), a, b, c, d);
        br.refill();
        let q = br.peek_lz();
        let me = if q < LIMIT - BITS - 1 {
            br.skip(q + 1);
            (q << k) | br.get(k)
        } else {
            br.skip(LIMIT - BITS);
            br.get(BITS) + 1
        } as i32;
        let alt = k == 0 && 2 * w.cb[ctx] <= -(w.cn[ctx] as i32);
        let e = if alt { if me & 1 == 1 { (me - 1) >> 1 } else { -(me >> 1) - 1 } } else if me & 1 == 0 { me >> 1 } else { -((me + 1) >> 1) };
        w.rows[ci][PAD + x] = ((px + s * e) & MASK as i32) as u16;
        loco_update(w, ctx, e);
    }
}

// ---------------------------------------------------------------- bands

/// Encode one band of `rows` rows of packed RAW10 (`stride` bytes a row) into `out`, returning the bytes written.
/// `lut`: the optional forward table of the shot-noise mode (sample to code value, each below 1024).
/// Panics if width is not a multiple of 4 or above MAX_W, if src is short, or if out is shorter than max_encoded_len: a caller's bug, loud.
pub fn encode_band(src: &[u8], stride: usize, width: usize, rows: usize, cfa: Cfa, p: &Params, lut: Option<&[u16; 1 << BITS]>, work: &mut Work, out: &mut [u8]) -> usize {
    let rb = width * 5 / 4;
    assert!(width & 3 == 0 && width >= 8 && width <= MAX_W, "width {width}");
    assert!(rows >= 1 && src.len() >= (rows - 1) * stride + rb, "short band source");
    assert!(out.len() >= max_encoded_len(width, rows), "output below max_encoded_len");
    // The packer puts two codes at a time: each at most 2^esc_log2 + 10 bits, so esc_log2 <= 4 keeps a pair inside put's 56.
    assert!(p.block_log2 >= 2 && p.block_log2 <= MAX_BLOCK_LOG2 && p.esc_log2 <= 4);
    assert!(!p.split || p.predictor.candidates().len() == 1, "split blocks take one fixed predictor");
    let half = max_stream_len(width, rows);
    let (sa, sb) = out[4..4 + 2 * half].split_at_mut(half);
    let mut bws = [BitWriter::new(sa), BitWriter::new(sb)];
    let mut kprev = K0;
    if p.coder == Coder::Loco {
        work.reset_contexts();
    }
    for yb in 0..rows {
        let ci = yb & 3;
        let dy = cfa.dy(yb);
        let has_up = yb >= dy;
        // Without a row above (has_up false) ui is never read for a sample; the wrap keeps it a valid ring index all the same.
        let ui = yb.wrapping_sub(dy) & 3;
        let s = &src[yb * stride..yb * stride + rb];
        let dst = &mut work.rows[ci][PAD..PAD + width];
        match lut {
            Some(l) => unpack_row_lut(s, dst, l),
            None => unpack_row(s, dst),
        }
        left_pad(&mut work.rows, ci, has_up.then_some(ui));
        match p.coder {
            Coder::Block => {
                // A band's first rows of each phase have no row above: left only, no choice sent.
                let (cands, uix) = if has_up { (p.predictor.candidates(), ui) } else { (Predictor::Left.candidates(), ci) };
                for (&id, m) in cands.iter().zip(work.m.iter_mut()) {
                    residuals(cfa, id, &work.rows[ci], &work.rows[uix], width, m);
                }
                let mut seqs: [&[u16]; MAX_CAND] = [&[]; MAX_CAND];
                if p.split {
                    split_row(cfa, &work.m[0], &mut work.t, width);
                    seqs[0] = &work.t[..width];
                } else {
                    for (s, m) in seqs.iter_mut().zip(work.m.iter()) {
                        *s = &m[..width];
                    }
                }
                block_row(&seqs[..cands.len()], cands.len().trailing_zeros(), p, &mut kprev, &mut work.rc, &mut bws);
            }
            Coder::Loco => loco_row_encode(work, cfa, ci, ui, has_up, yb, width, &mut bws[0]),
        }
        right_pad(&mut work.rows[ci], width);
    }
    // The band: the first stream's length (4 bytes, little-endian), the first stream, the second moved down behind it.
    let [a, b] = bws;
    let la = a.finish();
    let lb = b.finish();
    out[..4].copy_from_slice(&(la as u32).to_le_bytes());
    out.copy_within(4 + half..4 + half + lb, 4 + la);
    4 + la + lb
}

/// Decode one band into `dst` (u16 samples, `dst_stride` samples a row), returning the bytes of `data` it used.
/// The samples are the coded values: the raw samples in lossless mode, the code values in the shot-noise mode (map them thru the inverse table).
pub fn decode_band(data: &[u8], width: usize, rows: usize, cfa: Cfa, p: &Params, work: &mut Work, dst: &mut [u16], dst_stride: usize) -> usize {
    assert!(width & 3 == 0 && width >= 8 && width <= MAX_W, "width {width}");
    assert!(dst.len() >= (rows - 1) * dst_stride + width, "short band destination");
    assert!(!p.split || p.predictor.candidates().len() == 1, "split blocks take one fixed predictor");
    // A corrupt first length past the band panics in the slice below: loud, never a read outside data.
    let la = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let mut brs = [BitReader::new(&data[4..4 + la]), BitReader::new(&data[4 + la..])];
    let mut kprev = K0;
    if p.coder == Coder::Loco {
        work.reset_contexts();
    }
    for yb in 0..rows {
        let ci = yb & 3;
        let dy = cfa.dy(yb);
        let has_up = yb >= dy;
        // Without a row above (has_up false) ui is never read for a sample; the wrap keeps it a valid ring index all the same.
        let ui = yb.wrapping_sub(dy) & 3;
        left_pad(&mut work.rows, ci, has_up.then_some(ui));
        match p.coder {
            Coder::Block => {
                let (cands, uix) = if has_up { (p.predictor.candidates(), ui) } else { (Predictor::Left.candidates(), ci) };
                let sel_bits = cands.len().trailing_zeros();
                if p.split {
                    block_row_decode(&mut work.t[..width], &mut work.sel, sel_bits, p, &mut kprev, &mut brs);
                    unsplit_row(cfa, &work.t, &mut work.m[0], width);
                } else {
                    block_row_decode(&mut work.m[0][..width], &mut work.sel, sel_bits, p, &mut kprev, &mut brs);
                }
                for x in 0..width {
                    let i = PAD + x;
                    let dx = cfa.dxl(x);
                    let a = work.rows[ci][i - dx] as i16;
                    let b = work.rows[uix][i] as i16;
                    let c = work.rows[uix][i - dx] as i16;
                    let d = work.rows[uix][i + cfa.dxr(x)] as i16;
                    // A corrupt stream can name a candidate past the list: the mask keeps the index inside it (the list's length is a power of two).
                    let id = cands[work.sel[x >> p.block_log2] as usize & (cands.len() - 1)];
                    let pv = predict_dyn(id, a, b, c, d);
                    work.rows[ci][i] = ((pv + unzig(work.m[0][x])) & MASK) as u16;
                }
            }
            Coder::Loco => loco_row_decode(work, cfa, ci, ui, has_up, yb, width, &mut brs[0]),
        }
        right_pad(&mut work.rows[ci], width);
        dst[yb * dst_stride..yb * dst_stride + width].copy_from_slice(&work.rows[ci][PAD..PAD + width]);
    }
    4 + la + brs[1].consumed()
}

// ---------------------------------------------------------------- shot-noise quantisation

/// Build the shot-noise mode's tables: `fwd` sample to code value, `inv` code value to the reconstructed sample; returns the number of code values.
/// The noise model: sigma(x)^2 = g * max(x - black, 0) + r^2 in DN^2 (g DN per electron, r read noise in DN), given in Q16 (g_q16 = g * 2^16, r2_q16 = r^2 * 2^16).
/// The step at x is sqrt(1 + (s sigma(x))^2) with s = 2^-s_log2: s times the noise where the noise is well above a DN, never finer than the 1-DN integer step where it is not, C-infinity in between.
/// The code value is the integral of 1 / step, a generalised Anscombe transform, in closed form: with S = 2^s_log2 and T(u) = S^2 + r^2 + g u, f(x) = (2 S / g) (sqrt(T(x - black)) - sqrt(T(0))) above black and S (x - black) / sqrt(T(0)) below it (zero electrons: read noise only, the same slope at black, so f is C1).
/// Integer only (u64 isqrt), so it builds the same on every core; inv is the centroid of each code value's samples.
pub fn shot_tables(black: u32, g_q16: u64, r2_q16: u64, s_log2: u32, fwd: &mut [u16; 1 << BITS], inv: &mut [u16; 1 << BITS]) -> usize {
    assert!(g_q16 > 0, "the shot-noise model needs a gain");
    let ss = 1u64 << (2 * s_log2);
    let t0 = (ss << 16) + r2_q16;
    // sqrt(T * 2^16 * 2^16) = sqrt(T) * 2^16.
    let sq0 = (t0 << 16).isqrt() as i64;
    let s = 1i64 << s_log2;
    let f = |x: u32| -> i64 {
        // Q16 code value.
        if x >= black {
            let t = t0 + g_q16 * (x - black) as u64;
            let sq = (t << 16).isqrt() as i64;
            ((2 * s) * (sq - sq0) << 16) / g_q16 as i64
        } else {
            -((s * (black - x) as i64) << 32) / sq0
        }
    };
    let f0 = f(0);
    let white = (1u32 << BITS) - 1;
    for x in 0..white {
        fwd[x as usize] = ((f(x) - f0 + (1 << 15)) >> 16) as u16;
    }
    // White is a clipped sample, not a measured one: it keeps a code value of its own so a saturated sample comes back exactly white (f' < 1 keeps fwd[white - 1] below white, so the code stays below 1024).
    fwd[white as usize] = fwd[white as usize - 1] + 1;
    // Centroid of each code value's run of samples (fwd never decreases and never steps by more than 1, as f' <= 1), rounded half to even so two-sample runs do not all lean up.
    let mut x = 0usize;
    let mut ncodes = 0usize;
    while x < (1 << BITS) {
        let c = fwd[x] as usize;
        let start = x;
        let mut sum = 0usize;
        while x < (1 << BITS) && fwd[x] as usize == c {
            sum += x;
            x += 1;
        }
        let cnt = x - start;
        let (q, r) = (sum / cnt, sum % cnt);
        let up = (2 * r > cnt) | ((2 * r == cnt) & (q & 1 == 1));
        inv[c] = (q + up as usize) as u16;
        ncodes = c + 1;
    }
    for v in inv[ncodes..].iter_mut() {
        *v = (1 << BITS) - 1;
    }
    ncodes
}
