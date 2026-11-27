//! A pure-Rust H.264 encoder for the beam (BEAM.md: Photon's beam is H.264 Annex-B; every Photon decodes with OpenH264).
//!
//! Stage one is I_PCM: Constrained Baseline, every frame an IDR carrying its own parameter sets, every macroblock its raw samples. No transform, no prediction, no entropy coder, so it is small and exact: a decoder returns precisely the pixels given (the test oracle), at about 1.5 bytes per pixel.
//!
//! Stage two is real intra compression on the same skeleton: every macroblock I_16x16 (V/H/DC/Plane luma prediction, DC/H/V/Plane chroma prediction), the 4x4 integer transform with the luma DC Hadamard and chroma DC 2x2 transforms, quantisation at a fixed QP, and CAVLC. The encoder reconstructs bit-exactly as a decoder does (prediction runs from reconstructed neighbours), and that reconstruction is the test oracle: a decoder must return it exactly.

use alloc::vec::Vec;

/// A 4:2:0 picture, 8-bit, full-range BT.601 (what Photon's beam carries). Width and height even.
pub struct Picture<'a> {
    pub width: usize,
    pub height: usize,
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
}

struct Bits {
    out: Vec<u8>,
    acc: u8,
    n: u8,
}

impl Bits {
    fn new() -> Self {
        Self { out: Vec::new(), acc: 0, n: 0 }
    }
    fn bit(&mut self, b: bool) {
        self.acc = (self.acc << 1) | b as u8;
        self.n += 1;
        if self.n == 8 {
            self.out.push(self.acc);
            self.acc = 0;
            self.n = 0;
        }
    }
    fn bits(&mut self, v: u32, count: u32) {
        for i in (0..count).rev() {
            self.bit((v >> i) & 1 == 1);
        }
    }
    /// Unsigned Exp-Golomb.
    fn ue(&mut self, v: u32) {
        let x = v as u64 + 1;
        let len = 64 - x.leading_zeros();
        for _ in 1..len {
            self.bit(false);
        }
        for i in (0..len).rev() {
            self.bit((x >> i) & 1 == 1);
        }
    }
    /// Signed Exp-Golomb.
    fn se(&mut self, v: i32) {
        self.ue(if v > 0 { (2 * v - 1) as u32 } else { (-2 * v) as u32 });
    }
    fn aligned(&self) -> bool {
        self.n == 0
    }
    /// rbsp_trailing_bits: a stop bit, then zeros to the byte.
    fn trailing(mut self) -> Vec<u8> {
        self.bit(true);
        while !self.aligned() {
            self.bit(false);
        }
        self.out
    }
}

/// One NAL unit in Annex-B form: start code, header, and the payload with emulation prevention (a G#03 after any two zero bytes that precede a byte ≤ 3).
fn nal(out: &mut Vec<u8>, ref_idc: u8, kind: u8, rbsp: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1, (ref_idc << 5) | kind]);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
}

fn level_for(mbs: usize) -> u8 {
    match mbs {
        0..=396 => 21,
        397..=1620 => 30,
        1621..=3600 => 31,
        3601..=8192 => 40,
        _ => 51,
    }
}

/// Sequence parameter set: Constrained Baseline (profile 66, constraint_set0 and 1), frame_num in 4 bits, picture order count type 2, one reference frame, cropping when the size isn't a multiple of 16.
fn sps(width: usize, height: usize) -> Vec<u8> {
    let (mw, mh) = (width.div_ceil(16), height.div_ceil(16));
    let mut b = Bits::new();
    b.bits(66, 8);
    b.bits(0b1100_0000, 8);
    b.bits(level_for(mw * mh) as u32, 8);
    b.ue(0); // seq_parameter_set_id
    b.ue(0); // log2_max_frame_num_minus4
    b.ue(2); // pic_order_cnt_type
    b.ue(1); // max_num_ref_frames
    b.bit(false); // gaps_in_frame_num_value_allowed_flag
    b.ue(mw as u32 - 1);
    b.ue(mh as u32 - 1);
    b.bit(true); // frame_mbs_only_flag
    b.bit(true); // direct_8x8_inference_flag
    let (cr, cb) = ((mw * 16 - width) / 2, (mh * 16 - height) / 2);
    b.bit(cr != 0 || cb != 0); // frame_cropping_flag (offsets in 2-sample units for 4:2:0)
    if cr != 0 || cb != 0 {
        b.ue(0);
        b.ue(cr as u32);
        b.ue(0);
        b.ue(cb as u32);
    }
    b.bit(false); // vui_parameters_present_flag
    b.trailing()
}

/// Picture parameter set: CAVLC, one slice group, deblocking control present (each slice turns the filter off).
fn pps() -> Vec<u8> {
    let mut b = Bits::new();
    b.ue(0); // pic_parameter_set_id
    b.ue(0); // seq_parameter_set_id
    b.bit(false); // entropy_coding_mode_flag: CAVLC
    b.bit(false); // bottom_field_pic_order_in_frame_present_flag
    b.ue(0); // num_slice_groups_minus1
    b.ue(0); // num_ref_idx_l0_default_active_minus1
    b.ue(0); // num_ref_idx_l1_default_active_minus1
    b.bit(false); // weighted_pred_flag
    b.bits(0, 2); // weighted_bipred_idc
    b.se(0); // pic_init_qp_minus26
    b.se(0); // pic_init_qs_minus26
    b.se(0); // chroma_qp_index_offset
    b.bit(true); // deblocking_filter_control_present_flag
    b.bit(false); // constrained_intra_pred_flag
    b.bit(false); // redundant_pic_cnt_present_flag
    b.trailing()
}

/// Sample at (x, y) of a plane `w` wide and `h` high, edge-replicated past it (the cropped margin of a size that isn't a multiple of 16).
fn at(plane: &[u8], w: usize, h: usize, x: usize, y: usize) -> u8 {
    plane[y.min(h - 1) * w + x.min(w - 1)]
}

/// IDR slice header: the whole picture in one I slice, slice QP `qp`, deblocking off (I_PCM is exact; intra reconstruction stays the plain sum of prediction and residual).
fn slice_header(b: &mut Bits, idr_pic_id: u32, qp: i32) {
    b.ue(0); // first_mb_in_slice
    b.ue(7); // slice_type: I, every slice of the picture
    b.ue(0); // pic_parameter_set_id
    b.bits(0, 4); // frame_num (0 for an IDR)
    b.ue(idr_pic_id);
    b.bit(false); // no_output_of_prior_pics_flag
    b.bit(false); // long_term_reference_flag
    b.se(qp - 26); // slice_qp_delta (pic_init_qp is 26)
    b.ue(1); // disable_deblocking_filter_idc: off
}

/// One access unit: SPS, PPS and an IDR slice of I_PCM macroblocks. `idr_pic_id` must differ between consecutive IDRs (alternate 0 and 1).
pub fn encode_pcm_idr(pic: &Picture, idr_pic_id: u32) -> Vec<u8> {
    let (w, h) = (pic.width, pic.height);
    let (mw, mh) = (w.div_ceil(16), h.div_ceil(16));
    let mut au = Vec::with_capacity(mw * mh * 386 + 64);
    nal(&mut au, 3, 7, &sps(w, h));
    nal(&mut au, 3, 8, &pps());
    let mut b = Bits::new();
    slice_header(&mut b, idr_pic_id, 26);
    let (cw, ch) = (w / 2, h / 2);
    for my in 0..mh {
        for mx in 0..mw {
            b.ue(25); // mb_type: I_PCM
            while !b.aligned() {
                b.bit(false); // pcm_alignment_zero_bit
            }
            for y in 0..16 {
                for x in 0..16 {
                    b.out.push(at(pic.y, w, h, mx * 16 + x, my * 16 + y));
                }
            }
            for plane in [pic.u, pic.v] {
                for y in 0..8 {
                    for x in 0..8 {
                        b.out.push(at(plane, cw, ch, mx * 8 + x, my * 8 + y));
                    }
                }
            }
        }
    }
    nal(&mut au, 3, 5, &b.trailing());
    au
}

// ---------------------------------------------------------------------------------------------
// Stage two: I_16x16 intra + CAVLC.
// ---------------------------------------------------------------------------------------------

/// A decoded-domain picture the encoder owns: what a decoder will output for the access unit (cropped to the picture size).
pub struct Reconstruction {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// One access unit: SPS, PPS and an IDR slice of I_16x16 macroblocks at quantiser `qp` (10 to 51, lower is finer; below 10 is raised to 10). `idr_pic_id` must differ between consecutive IDRs (alternate 0 and 1).
pub fn encode_intra_idr(pic: &Picture, idr_pic_id: u32, qp: u8) -> Vec<u8> {
    encode_intra_idr_with_recon(pic, idr_pic_id, qp).0
}

/// [`encode_intra_idr`], also returning the encoder's own reconstruction: exactly the picture a conforming decoder outputs.
pub fn encode_intra_idr_with_recon(pic: &Picture, idr_pic_id: u32, qp: u8) -> (Vec<u8>, Reconstruction) {
    let qp = qp.clamp(MIN_QP, 51) as i32;
    let (w, h) = (pic.width, pic.height);
    let (mw, mh) = (w.div_ceil(16), h.div_ceil(16));
    let mut e = Intra::new(mw, mh, qp);
    let mut au = Vec::with_capacity(mw * mh * 64 + 64);
    nal(&mut au, 3, 7, &sps(w, h));
    nal(&mut au, 3, 8, &pps());
    let mut b = Bits::new();
    slice_header(&mut b, idr_pic_id, qp);
    for my in 0..mh {
        for mx in 0..mw {
            e.macroblock(&mut b, pic, mx, my);
        }
    }
    nal(&mut au, 3, 5, &b.trailing());
    let crop = |plane: &[u8], stride: usize, cw: usize, ch: usize| {
        let mut out = Vec::with_capacity(cw * ch);
        for y in 0..ch {
            out.extend_from_slice(&plane[y * stride..y * stride + cw]);
        }
        out
    };
    let rec = Reconstruction {
        width: w,
        height: h,
        y: crop(&e.y, mw * 16, w, h),
        u: crop(&e.u, mw * 8, w / 2, h / 2),
        v: crop(&e.v, mw * 8, w / 2, h / 2),
    };
    (au, rec)
}

/// Frame zig-zag scan: scan index to raster position (x + 4y) in a 4x4 block.
const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// luma4x4BlkIdx to the block's (x, y) in 4x4-block units inside the macroblock (8x8 quadrants in raster, 4x4s in raster inside each).
const BLK_XY: [(usize, usize); 16] = [(0, 0), (1, 0), (0, 1), (1, 1), (2, 0), (3, 0), (2, 1), (3, 1), (0, 2), (1, 2), (0, 3), (1, 3), (2, 2), (3, 2), (2, 3), (3, 3)];

/// Forward quantiser multipliers by QP % 6 for positions (even, even), (odd, odd) and mixed.
const MF: [[i64; 3]; 6] = [[13107, 5243, 8066], [11916, 4660, 7490], [10082, 4194, 6554], [9362, 3647, 5825], [8192, 3355, 5243], [7282, 2893, 4559]];

/// Dequantiser normAdjust4x4 by QP % 6 for the same three position classes (clause 8.5.9, flat scaling lists).
const V: [[i32; 3]; 6] = [[10, 16, 13], [11, 18, 14], [13, 20, 16], [14, 23, 18], [16, 25, 20], [18, 29, 23]];

/// QPc from qPi (Table 8-15), chroma_qp_index_offset 0.
const CHROMA_QP: [u8; 52] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 29, 30, 31, 32, 32, 33, 34, 34, 35, 35, 36, 36, 37, 37, 37, 38, 38, 38, 39, 39, 39, 39];

/// The finest quantiser used: at QP 10 the largest possible luma DC level (a flat 255 residual over the whole macroblock) is 2040, inside [`MAX_LEVEL`]; finer QPs would need level_prefix 16 and up, which Baseline forbids, and would push a decoder's 16-bit intermediates.
const MIN_QP: u8 = 10;

/// Largest level magnitude written: keeps every levelCode inside the 12-bit escape that Baseline allows (level_prefix at most 15) for any suffixLength.
const MAX_LEVEL: i32 = 2063;

/// Position class of raster index `p` in a 4x4 block: 0 when x and y are both even, 1 when both odd, else 2.
fn class(p: usize) -> usize {
    match ((p & 1), ((p >> 2) & 1)) {
        (0, 0) => 0,
        (1, 1) => 1,
        _ => 2,
    }
}

fn clip1(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Forward 4x4 core transform Cf·X·Cfᵀ, raster in and out.
fn fdct(x: &[i32; 16]) -> [i32; 16] {
    let mut t = [0i32; 16];
    for r in 0..4 {
        let a = &x[r * 4..r * 4 + 4];
        let (s03, d03, s12, d12) = (a[0] + a[3], a[0] - a[3], a[1] + a[2], a[1] - a[2]);
        t[r * 4] = s03 + s12;
        t[r * 4 + 1] = 2 * d03 + d12;
        t[r * 4 + 2] = s03 - s12;
        t[r * 4 + 3] = d03 - 2 * d12;
    }
    let mut o = [0i32; 16];
    for c in 0..4 {
        let (a0, a1, a2, a3) = (t[c], t[4 + c], t[8 + c], t[12 + c]);
        let (s03, d03, s12, d12) = (a0 + a3, a0 - a3, a1 + a2, a1 - a2);
        o[c] = s03 + s12;
        o[4 + c] = 2 * d03 + d12;
        o[8 + c] = s03 - s12;
        o[12 + c] = d03 - 2 * d12;
    }
    o
}

/// Inverse 4x4 transform of scaled coefficients `d` (clause 8.5.12.2: rows, then columns, then (x + 32) >> 6), added to `pred` and clipped.
fn idct_add(d: &[i32; 16], pred: &[i32; 16]) -> [u8; 16] {
    let mut t = [0i32; 16];
    for r in 0..4 {
        let a = &d[r * 4..r * 4 + 4];
        let (e0, e1, e2, e3) = (a[0] + a[2], a[0] - a[2], (a[1] >> 1) - a[3], a[1] + (a[3] >> 1));
        t[r * 4] = e0 + e3;
        t[r * 4 + 1] = e1 + e2;
        t[r * 4 + 2] = e1 - e2;
        t[r * 4 + 3] = e0 - e3;
    }
    let mut o = [0u8; 16];
    for c in 0..4 {
        let (a0, a1, a2, a3) = (t[c], t[4 + c], t[8 + c], t[12 + c]);
        let (e0, e1, e2, e3) = (a0 + a2, a0 - a2, (a1 >> 1) - a3, a1 + (a3 >> 1));
        let r = [e0 + e3, e1 + e2, e1 - e2, e0 - e3];
        for (y, v) in r.into_iter().enumerate() {
            o[y * 4 + c] = clip1(((v + 32) >> 6) + pred[y * 4 + c]);
        }
    }
    o
}

/// 4x4 Hadamard (rows then columns), used for the luma DC transform both ways and for SATD.
fn hadamard4(x: &[i32; 16]) -> [i32; 16] {
    let mut t = [0i32; 16];
    for r in 0..4 {
        let a = &x[r * 4..r * 4 + 4];
        let (s01, d01, s23, d23) = (a[0] + a[1], a[0] - a[1], a[2] + a[3], a[2] - a[3]);
        t[r * 4] = s01 + s23;
        t[r * 4 + 1] = s01 - s23;
        t[r * 4 + 2] = d01 - d23;
        t[r * 4 + 3] = d01 + d23;
    }
    let mut o = [0i32; 16];
    for c in 0..4 {
        let (a0, a1, a2, a3) = (t[c], t[4 + c], t[8 + c], t[12 + c]);
        let (s01, d01, s23, d23) = (a0 + a1, a0 - a1, a2 + a3, a2 - a3);
        o[c] = s01 + s23;
        o[4 + c] = s01 - s23;
        o[8 + c] = d01 - d23;
        o[12 + c] = d01 + d23;
    }
    o
}

/// Quantise one coefficient: sign(w) · ((|w| · mf + f) >> shift), clamped to what CAVLC can carry.
fn quant(w: i32, mf: i64, f: i64, shift: u32) -> i32 {
    let q = ((w.unsigned_abs() as i64 * mf + f) >> shift) as i32;
    let q = q.min(MAX_LEVEL);
    if w < 0 { -q } else { q }
}

/// Sum of absolute Hadamard-transformed differences over a `size`x`size` block (raster, row stride `size`): the mode-decision cost.
fn satd(src: &[i32], pred: &[i32], size: usize) -> u32 {
    let mut cost = 0u32;
    for by in (0..size).step_by(4) {
        for bx in (0..size).step_by(4) {
            let mut d = [0i32; 16];
            for y in 0..4 {
                for x in 0..4 {
                    let i = (by + y) * size + bx + x;
                    d[y * 4 + x] = src[i] - pred[i];
                }
            }
            cost += hadamard4(&d).iter().map(|v| v.unsigned_abs()).sum::<u32>();
        }
    }
    cost
}

/// Neighbour samples of a block at (x0, y0), `n` wide, in a reconstructed plane: the row above, the column left, the corner, and which of them exist.
struct Edges {
    top: [i32; 16],
    left: [i32; 16],
    corner: i32,
    has_top: bool,
    has_left: bool,
}

impl Edges {
    fn of(plane: &[u8], stride: usize, x0: usize, y0: usize, n: usize) -> Self {
        let mut e = Edges { top: [0; 16], left: [0; 16], corner: 0, has_top: y0 > 0, has_left: x0 > 0 };
        if e.has_top {
            for i in 0..n {
                e.top[i] = plane[(y0 - 1) * stride + x0 + i] as i32;
            }
        }
        if e.has_left {
            for i in 0..n {
                e.left[i] = plane[(y0 + i) * stride + x0 - 1] as i32;
            }
        }
        if e.has_top && e.has_left {
            e.corner = plane[(y0 - 1) * stride + x0 - 1] as i32;
        }
        e
    }
    /// p[i, -1], with i = -1 the corner.
    fn t(&self, i: isize) -> i32 {
        if i < 0 { self.corner } else { self.top[i as usize] }
    }
    /// p[-1, i], with i = -1 the corner.
    fn l(&self, i: isize) -> i32 {
        if i < 0 { self.corner } else { self.left[i as usize] }
    }
}

/// Intra_16x16 prediction (clause 8.3.3) for `mode` (0 vertical, 1 horizontal, 2 DC, 3 plane), or None when its neighbours are missing.
fn pred16(e: &Edges, mode: u32) -> Option<[i32; 256]> {
    let mut p = [0i32; 256];
    match mode {
        0 if e.has_top => (0..256).for_each(|i| p[i] = e.top[i % 16]),
        1 if e.has_left => (0..256).for_each(|i| p[i] = e.left[i / 16]),
        2 => {
            let st: i32 = e.top.iter().sum();
            let sl: i32 = e.left.iter().sum();
            let dc = match (e.has_top, e.has_left) {
                (true, true) => (st + sl + 16) >> 5,
                (false, true) => (sl + 8) >> 4,
                (true, false) => (st + 8) >> 4,
                _ => 128,
            };
            p = [dc; 256];
        }
        3 if e.has_top && e.has_left => {
            let (mut hh, mut vv) = (0i32, 0i32);
            for k in 0..8isize {
                hh += (k as i32 + 1) * (e.t(8 + k) - e.t(6 - k));
                vv += (k as i32 + 1) * (e.l(8 + k) - e.l(6 - k));
            }
            let a = 16 * (e.left[15] + e.top[15]);
            let (b, c) = ((5 * hh + 32) >> 6, (5 * vv + 32) >> 6);
            for y in 0..16 {
                for x in 0..16 {
                    p[y * 16 + x] = clip1((a + b * (x as i32 - 7) + c * (y as i32 - 7) + 16) >> 5) as i32;
                }
            }
        }
        _ => return None,
    }
    Some(p)
}

/// 4:2:0 chroma intra prediction (clause 8.3.4) for `mode` (0 DC, 1 horizontal, 2 vertical, 3 plane), or None when its neighbours are missing.
fn pred8(e: &Edges, mode: u32) -> Option<[i32; 64]> {
    let mut p = [0i32; 64];
    match mode {
        0 => {
            for (bx, by) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)] {
                let st: i32 = e.top[bx * 4..bx * 4 + 4].iter().sum();
                let sl: i32 = e.left[by * 4..by * 4 + 4].iter().sum();
                let (t, l) = (e.has_top, e.has_left);
                // The corner blocks use both edges; the top-right block prefers the top, the bottom-left block the left.
                let dc = if bx == by {
                    match (t, l) {
                        (true, true) => (st + sl + 4) >> 3,
                        (false, true) => (sl + 2) >> 2,
                        (true, false) => (st + 2) >> 2,
                        _ => 128,
                    }
                } else if bx == 1 {
                    if t { (st + 2) >> 2 } else if l { (sl + 2) >> 2 } else { 128 }
                } else if l {
                    (sl + 2) >> 2
                } else if t {
                    (st + 2) >> 2
                } else {
                    128
                };
                for y in 0..4 {
                    for x in 0..4 {
                        p[(by * 4 + y) * 8 + bx * 4 + x] = dc;
                    }
                }
            }
        }
        1 if e.has_left => (0..64).for_each(|i| p[i] = e.left[i / 8]),
        2 if e.has_top => (0..64).for_each(|i| p[i] = e.top[i % 8]),
        3 if e.has_top && e.has_left => {
            let (mut hh, mut vv) = (0i32, 0i32);
            for k in 0..4isize {
                hh += (k as i32 + 1) * (e.t(4 + k) - e.t(2 - k));
                vv += (k as i32 + 1) * (e.l(4 + k) - e.l(2 - k));
            }
            let a = 16 * (e.left[7] + e.top[7]);
            let (b, c) = ((34 * hh + 32) >> 6, (34 * vv + 32) >> 6);
            for y in 0..8 {
                for x in 0..8 {
                    p[y * 8 + x] = clip1((a + b * (x as i32 - 3) + c * (y as i32 - 3) + 16) >> 5) as i32;
                }
            }
        }
        _ => return None,
    }
    Some(p)
}

/// One chroma component of a macroblock after transform and quantisation: DC levels (raster 2x2) and AC levels per 4x4 block (raster, position 0 unused).
struct ChromaCoeffs {
    dc: [i32; 4],
    ac: [[i32; 16]; 4],
}

/// Encoder state across one picture: reconstructed planes (macroblock-aligned) and the per-4x4-block TotalCoeff that CAVLC's nC context reads.
struct Intra {
    mw: usize,
    qp: i32,
    qpc: i32,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    nz_y: Vec<u8>,
    nz_u: Vec<u8>,
    nz_v: Vec<u8>,
}

impl Intra {
    fn new(mw: usize, mh: usize, qp: i32) -> Self {
        Intra {
            mw,
            qp,
            qpc: CHROMA_QP[qp as usize] as i32,
            y: alloc::vec![0; mw * 16 * mh * 16],
            u: alloc::vec![0; mw * 8 * mh * 8],
            v: alloc::vec![0; mw * 8 * mh * 8],
            nz_y: alloc::vec![0; mw * 4 * mh * 4],
            nz_u: alloc::vec![0; mw * 2 * mh * 2],
            nz_v: alloc::vec![0; mw * 2 * mh * 2],
        }
    }

    /// nC (clause 9.2.1) for the block at (bx, by) in a grid `gw` blocks wide: the rounded mean of the left and upper neighbours' TotalCoeff, whichever exist.
    fn nc(grid: &[u8], gw: usize, bx: usize, by: usize) -> i32 {
        let a = (bx > 0).then(|| grid[by * gw + bx - 1] as i32);
        let b = (by > 0).then(|| grid[(by - 1) * gw + bx] as i32);
        match (a, b) {
            (Some(a), Some(b)) => (a + b + 1) >> 1,
            (Some(a), None) => a,
            (None, Some(b)) => b,
            _ => 0,
        }
    }

    fn macroblock(&mut self, b: &mut Bits, pic: &Picture, mx: usize, my: usize) {
        let (w, h) = (pic.width, pic.height);
        let ls = self.mw * 16;
        let (x0, y0) = (mx * 16, my * 16);

        // Luma: pick the 16x16 mode with the least SATD, then transform and quantise against it.
        let mut src = [0i32; 256];
        for y in 0..16 {
            for x in 0..16 {
                src[y * 16 + x] = at(pic.y, w, h, x0 + x, y0 + y) as i32;
            }
        }
        let edges = Edges::of(&self.y, ls, x0, y0, 16);
        let (mut mode, mut pred, mut best) = (2u32, [0i32; 256], u32::MAX);
        for m in [2u32, 0, 1, 3] {
            if let Some(p) = pred16(&edges, m) {
                let c = satd(&src, &p, 16);
                if c < best {
                    (mode, pred, best) = (m, p, c);
                }
            }
        }
        let (qp, qpc) = (self.qp, self.qpc);
        let (m6, q6) = ((qp % 6) as usize, qp / 6);
        let qbits = 15 + q6 as u32;
        let f = (1i64 << qbits) / 3;
        let mut coef = [[0i32; 16]; 16]; // per 4x4 block in raster block order (bx + 4·by)
        for by in 0..4 {
            for bx in 0..4 {
                let mut d = [0i32; 16];
                for y in 0..4 {
                    for x in 0..4 {
                        let i = (by * 4 + y) * 16 + bx * 4 + x;
                        d[y * 4 + x] = src[i] - pred[i];
                    }
                }
                coef[by * 4 + bx] = fdct(&d);
            }
        }
        let mut dcm = [0i32; 16];
        for i in 0..16 {
            dcm[i] = coef[i][0];
        }
        let dct = hadamard4(&dcm);
        let mut dcq = [0i32; 16]; // raster over blocks
        for i in 0..16 {
            dcq[i] = quant(dct[i] >> 1, MF[m6][0], 2 * f, qbits + 1);
        }
        let mut acq = [[0i32; 16]; 16];
        let mut any_ac = false;
        for (blk, c) in coef.iter().enumerate() {
            for p in 1..16 {
                acq[blk][p] = quant(c[p], MF[m6][class(p)], f, qbits);
                any_ac |= acq[blk][p] != 0;
            }
        }
        let cbp_luma = if any_ac { 15u32 } else { 0 };

        // Luma reconstruction exactly as clause 8.5.10 and 8.5.12 decode it.
        let hd = hadamard4(&dcq);
        let dc_scale = 16 * V[m6][0];
        let mut dcy = [0i32; 16];
        for i in 0..16 {
            dcy[i] = if q6 >= 6 { (hd[i] * dc_scale) << (q6 - 6) } else { (hd[i] * dc_scale + (1 << (5 - q6))) >> (6 - q6) };
        }
        for by in 0..4 {
            for bx in 0..4 {
                let blk = by * 4 + bx;
                let mut d = [0i32; 16];
                d[0] = dcy[blk];
                for p in 1..16 {
                    d[p] = (acq[blk][p] * V[m6][class(p)]) << q6;
                }
                let mut pr = [0i32; 16];
                for y in 0..4 {
                    for x in 0..4 {
                        pr[y * 4 + x] = pred[(by * 4 + y) * 16 + bx * 4 + x];
                    }
                }
                let r = idct_add(&d, &pr);
                for y in 0..4 {
                    for x in 0..4 {
                        self.y[(y0 + by * 4 + y) * ls + x0 + bx * 4 + x] = r[y * 4 + x];
                    }
                }
            }
        }

        // Chroma: one mode for both components, least summed SATD.
        let cs = self.mw * 8;
        let (cx0, cy0) = (mx * 8, my * 8);
        let (cw, ch) = (w / 2, h / 2);
        let mut csrc = [[0i32; 64]; 2];
        for (k, plane) in [pic.u, pic.v].into_iter().enumerate() {
            for y in 0..8 {
                for x in 0..8 {
                    csrc[k][y * 8 + x] = at(plane, cw, ch, cx0 + x, cy0 + y) as i32;
                }
            }
        }
        let cedges = [Edges::of(&self.u, cs, cx0, cy0, 8), Edges::of(&self.v, cs, cx0, cy0, 8)];
        let (mut cmode, mut cpred, mut cbest) = (0u32, [[0i32; 64]; 2], u32::MAX);
        for m in [0u32, 1, 2, 3] {
            if let (Some(pu), Some(pv)) = (pred8(&cedges[0], m), pred8(&cedges[1], m)) {
                let c = satd(&csrc[0], &pu, 8) + satd(&csrc[1], &pv, 8);
                if c < cbest {
                    (cmode, cpred, cbest) = (m, [pu, pv], c);
                }
            }
        }
        let (c6, cq6) = ((qpc % 6) as usize, qpc / 6);
        let cqbits = 15 + cq6 as u32;
        let cf = (1i64 << cqbits) / 3;
        let mut cc = [ChromaCoeffs { dc: [0; 4], ac: [[0; 16]; 4] }, ChromaCoeffs { dc: [0; 4], ac: [[0; 16]; 4] }];
        let (mut any_cdc, mut any_cac) = (false, false);
        for k in 0..2 {
            let mut blocks = [[0i32; 16]; 4];
            for (bi, (bx, by)) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                let mut d = [0i32; 16];
                for y in 0..4 {
                    for x in 0..4 {
                        let i = (by * 4 + y) * 8 + bx * 4 + x;
                        d[y * 4 + x] = csrc[k][i] - cpred[k][i];
                    }
                }
                blocks[bi] = fdct(&d);
            }
            let (a, bb, c, d) = (blocks[0][0], blocks[1][0], blocks[2][0], blocks[3][0]);
            let fdc = [a + bb + c + d, a - bb + c - d, a + bb - c - d, a - bb - c + d];
            for i in 0..4 {
                cc[k].dc[i] = quant(fdc[i], MF[c6][0], 2 * cf, cqbits + 1);
                any_cdc |= cc[k].dc[i] != 0;
            }
            for bi in 0..4 {
                for p in 1..16 {
                    cc[k].ac[bi][p] = quant(blocks[bi][p], MF[c6][class(p)], cf, cqbits);
                    any_cac |= cc[k].ac[bi][p] != 0;
                }
            }
        }
        let cbp_chroma = if any_cac { 2u32 } else if any_cdc { 1 } else { 0 };

        // Chroma reconstruction (clause 8.5.11).
        for k in 0..2 {
            let c = &cc[k].dc;
            let fdc = [c[0] + c[1] + c[2] + c[3], c[0] - c[1] + c[2] - c[3], c[0] + c[1] - c[2] - c[3], c[0] - c[1] - c[2] + c[3]];
            for (bi, (bx, by)) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                let mut d = [0i32; 16];
                d[0] = ((fdc[bi] * 16 * V[c6][0]) << cq6) >> 5;
                for p in 1..16 {
                    d[p] = (cc[k].ac[bi][p] * V[c6][class(p)]) << cq6;
                }
                let mut pr = [0i32; 16];
                for y in 0..4 {
                    for x in 0..4 {
                        pr[y * 4 + x] = cpred[k][(by * 4 + y) * 8 + bx * 4 + x];
                    }
                }
                let r = idct_add(&d, &pr);
                let plane = if k == 0 { &mut self.u } else { &mut self.v };
                for y in 0..4 {
                    for x in 0..4 {
                        plane[(cy0 + by * 4 + y) * cs + cx0 + bx * 4 + x] = r[y * 4 + x];
                    }
                }
            }
        }

        // Syntax: mb_type carries the luma mode and both coded block patterns; then the chroma mode, a zero QP delta, and the residual.
        b.ue(1 + mode + 4 * cbp_chroma + if cbp_luma == 15 { 12 } else { 0 });
        b.ue(cmode); // intra_chroma_pred_mode
        b.se(0); // mb_qp_delta
        let gw = self.mw * 4;
        let mut scan = [0i32; 16];
        for (k, &z) in ZIGZAG.iter().enumerate() {
            scan[k] = dcq[z];
        }
        residual_block(b, &scan, Self::nc(&self.nz_y, gw, mx * 4, my * 4), 16, false);
        for &(bx, by) in BLK_XY.iter() {
            let (gx, gy) = (mx * 4 + bx, my * 4 + by);
            let mut total = 0;
            if cbp_luma == 15 {
                let raster = by * 4 + bx;
                let mut ac = [0i32; 15];
                for k in 0..15 {
                    ac[k] = acq[raster][ZIGZAG[k + 1]];
                }
                total = residual_block(b, &ac, Self::nc(&self.nz_y, gw, gx, gy), 15, false);
            }
            self.nz_y[gy * gw + gx] = total;
        }
        if cbp_chroma != 0 {
            for k in 0..2 {
                residual_block(b, &cc[k].dc, -1, 4, true);
            }
        }
        let cgw = self.mw * 2;
        for k in 0..2 {
            for (bi, (bx, by)) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
                let (gx, gy) = (mx * 2 + bx, my * 2 + by);
                let grid = if k == 0 { &mut self.nz_u } else { &mut self.nz_v };
                let mut total = 0;
                if cbp_chroma == 2 {
                    let mut ac = [0i32; 15];
                    for j in 0..15 {
                        ac[j] = cc[k].ac[bi][ZIGZAG[j + 1]];
                    }
                    total = residual_block(b, &ac, Self::nc(grid, cgw, gx, gy), 15, false);
                }
                grid[gy * cgw + gx] = total;
            }
        }
    }
}

/// One CAVLC residual block (clause 9.2): `coeffs` in scan order, `nc` the context (-1 for chroma DC), `max` the block's coefficient count. Returns TotalCoeff.
fn residual_block(b: &mut Bits, coeffs: &[i32], nc: i32, max: usize, chroma_dc: bool) -> u8 {
    // Nonzero levels from highest frequency down, with their scan positions.
    let mut lv = [0i32; 16];
    let mut pos = [0usize; 16];
    let mut total = 0usize;
    for i in (0..max).rev() {
        if coeffs[i] != 0 {
            lv[total] = coeffs[i];
            pos[total] = i;
            total += 1;
        }
    }
    let mut t1 = 0usize;
    while t1 < total.min(3) && lv[t1].abs() == 1 {
        t1 += 1;
    }
    let table = match nc {
        -1 => 4,
        0..=1 => 0,
        2..=3 => 1,
        4..=7 => 2,
        _ => 3,
    };
    let (code, len) = COEFF_TOKEN[table][total][t1];
    b.bits(code as u32, len as u32);
    if total == 0 {
        return 0;
    }
    for &l in &lv[..t1] {
        b.bit(l < 0); // trailing_ones_sign_flag
    }
    let mut suffix_len: u32 = if total > 10 && t1 < 3 { 1 } else { 0 };
    for i in t1..total {
        let l = lv[i];
        let mut code = if l > 0 { 2 * l - 2 } else { -2 * l - 1 };
        if i == t1 && t1 < 3 {
            code -= 2;
        }
        let code = code as u32;
        let (prefix, suffix, suffix_bits) = if suffix_len == 0 {
            if code < 14 {
                (code, 0, 0)
            } else if code < 30 {
                (14, code - 14, 4)
            } else {
                (15, code - 30, 12)
            }
        } else if code < (15 << suffix_len) {
            (code >> suffix_len, code & ((1 << suffix_len) - 1), suffix_len)
        } else {
            (15, code - (15 << suffix_len), 12)
        };
        for _ in 0..prefix {
            b.bit(false);
        }
        b.bit(true);
        b.bits(suffix, suffix_bits);
        if suffix_len == 0 {
            suffix_len = 1;
        }
        if l.unsigned_abs() > (3 << (suffix_len - 1)) && suffix_len < 6 {
            suffix_len += 1;
        }
    }
    let total_zeros = pos[0] + 1 - total; // zeros below the highest nonzero coefficient
    if total < max {
        let (code, len) = if chroma_dc { TOTAL_ZEROS_CHROMA_DC[total][total_zeros] } else { TOTAL_ZEROS[total][total_zeros] };
        b.bits(code as u32, len as u32);
    }
    let mut zeros_left = total_zeros;
    for i in 0..total - 1 {
        if zeros_left == 0 {
            break;
        }
        let run = pos[i] - pos[i + 1] - 1;
        let (code, len) = RUN_BEFORE[zeros_left.min(7)][run];
        b.bits(code as u32, len as u32);
        zeros_left -= run;
    }
    total as u8
}

/// coeff_token (code, length) by nC class (0-1, 2-3, 4-7, 8+, chroma DC), TotalCoeff and TrailingOnes: Table 9-5, transcribed from OpenH264's g_kuiVlcCoeffToken.
const COEFF_TOKEN: [[[(u8, u8); 4]; 17]; 5] = [
    [
        [(1, 1), (0, 0), (0, 0), (0, 0)],
        [(5, 6), (1, 2), (0, 0), (0, 0)],
        [(7, 8), (4, 6), (1, 3), (0, 0)],
        [(7, 9), (6, 8), (5, 7), (3, 5)],
        [(7, 10), (6, 9), (5, 8), (3, 6)],
        [(7, 11), (6, 10), (5, 9), (4, 7)],
        [(15, 13), (6, 11), (5, 10), (4, 8)],
        [(11, 13), (14, 13), (5, 11), (4, 9)],
        [(8, 13), (10, 13), (13, 13), (4, 10)],
        [(15, 14), (14, 14), (9, 13), (4, 11)],
        [(11, 14), (10, 14), (13, 14), (12, 13)],
        [(15, 15), (14, 15), (9, 14), (12, 14)],
        [(11, 15), (10, 15), (13, 15), (8, 14)],
        [(15, 16), (1, 15), (9, 15), (12, 15)],
        [(11, 16), (14, 16), (13, 16), (8, 15)],
        [(7, 16), (10, 16), (9, 16), (12, 16)],
        [(4, 16), (6, 16), (5, 16), (8, 16)],
    ],
    [
        [(3, 2), (0, 0), (0, 0), (0, 0)],
        [(11, 6), (2, 2), (0, 0), (0, 0)],
        [(7, 6), (7, 5), (3, 3), (0, 0)],
        [(7, 7), (10, 6), (9, 6), (5, 4)],
        [(7, 8), (6, 6), (5, 6), (4, 4)],
        [(4, 8), (6, 7), (5, 7), (6, 5)],
        [(7, 9), (6, 8), (5, 8), (8, 6)],
        [(15, 11), (6, 9), (5, 9), (4, 6)],
        [(11, 11), (14, 11), (13, 11), (4, 7)],
        [(15, 12), (10, 11), (9, 11), (4, 9)],
        [(11, 12), (14, 12), (13, 12), (12, 11)],
        [(8, 12), (10, 12), (9, 12), (8, 11)],
        [(15, 13), (14, 13), (13, 13), (12, 12)],
        [(11, 13), (10, 13), (9, 13), (12, 13)],
        [(7, 13), (11, 14), (6, 13), (8, 13)],
        [(9, 14), (8, 14), (10, 14), (1, 13)],
        [(7, 14), (6, 14), (5, 14), (4, 14)],
    ],
    [
        [(15, 4), (0, 0), (0, 0), (0, 0)],
        [(15, 6), (14, 4), (0, 0), (0, 0)],
        [(11, 6), (15, 5), (13, 4), (0, 0)],
        [(8, 6), (12, 5), (14, 5), (12, 4)],
        [(15, 7), (10, 5), (11, 5), (11, 4)],
        [(11, 7), (8, 5), (9, 5), (10, 4)],
        [(9, 7), (14, 6), (13, 6), (9, 4)],
        [(8, 7), (10, 6), (9, 6), (8, 4)],
        [(15, 8), (14, 7), (13, 7), (13, 5)],
        [(11, 8), (14, 8), (10, 7), (12, 6)],
        [(15, 9), (10, 8), (13, 8), (12, 7)],
        [(11, 9), (14, 9), (9, 8), (12, 8)],
        [(8, 9), (10, 9), (13, 9), (8, 8)],
        [(13, 10), (7, 9), (9, 9), (12, 9)],
        [(9, 10), (12, 10), (11, 10), (10, 10)],
        [(5, 10), (8, 10), (7, 10), (6, 10)],
        [(1, 10), (4, 10), (3, 10), (2, 10)],
    ],
    [
        [(3, 6), (0, 0), (0, 0), (0, 0)],
        [(0, 6), (1, 6), (0, 0), (0, 0)],
        [(4, 6), (5, 6), (6, 6), (0, 0)],
        [(8, 6), (9, 6), (10, 6), (11, 6)],
        [(12, 6), (13, 6), (14, 6), (15, 6)],
        [(16, 6), (17, 6), (18, 6), (19, 6)],
        [(20, 6), (21, 6), (22, 6), (23, 6)],
        [(24, 6), (25, 6), (26, 6), (27, 6)],
        [(28, 6), (29, 6), (30, 6), (31, 6)],
        [(32, 6), (33, 6), (34, 6), (35, 6)],
        [(36, 6), (37, 6), (38, 6), (39, 6)],
        [(40, 6), (41, 6), (42, 6), (43, 6)],
        [(44, 6), (45, 6), (46, 6), (47, 6)],
        [(48, 6), (49, 6), (50, 6), (51, 6)],
        [(52, 6), (53, 6), (54, 6), (55, 6)],
        [(56, 6), (57, 6), (58, 6), (59, 6)],
        [(60, 6), (61, 6), (62, 6), (63, 6)],
    ],
    [
        [(1, 2), (0, 0), (0, 0), (0, 0)],
        [(7, 6), (1, 1), (0, 0), (0, 0)],
        [(4, 6), (6, 6), (1, 3), (0, 0)],
        [(3, 6), (3, 7), (2, 7), (5, 6)],
        [(2, 6), (3, 8), (2, 8), (0, 7)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
        [(0, 0), (0, 0), (0, 0), (0, 0)],
    ],
];
/// total_zeros (code, length) for 4x4 blocks by TotalCoeff and total_zeros: Tables 9-7 and 9-8, from OpenH264's g_kuiVlcTotalZeros.
const TOTAL_ZEROS: [[(u8, u8); 16]; 16] = [
    [(0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(1, 1), (3, 3), (2, 3), (3, 4), (2, 4), (3, 5), (2, 5), (3, 6), (2, 6), (3, 7), (2, 7), (3, 8), (2, 8), (3, 9), (2, 9), (1, 9)],
    [(7, 3), (6, 3), (5, 3), (4, 3), (3, 3), (5, 4), (4, 4), (3, 4), (2, 4), (3, 5), (2, 5), (3, 6), (2, 6), (1, 6), (0, 6), (0, 0)],
    [(5, 4), (7, 3), (6, 3), (5, 3), (4, 4), (3, 4), (4, 3), (3, 3), (2, 4), (3, 5), (2, 5), (1, 6), (1, 5), (0, 6), (0, 0), (0, 0)],
    [(3, 5), (7, 3), (5, 4), (4, 4), (6, 3), (5, 3), (4, 3), (3, 4), (3, 3), (2, 4), (2, 5), (1, 5), (0, 5), (0, 0), (0, 0), (0, 0)],
    [(5, 4), (4, 4), (3, 4), (7, 3), (6, 3), (5, 3), (4, 3), (3, 3), (2, 4), (1, 5), (1, 4), (0, 5), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(1, 6), (1, 5), (7, 3), (6, 3), (5, 3), (4, 3), (3, 3), (2, 3), (1, 4), (1, 3), (0, 6), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(1, 6), (1, 5), (5, 3), (4, 3), (3, 3), (3, 2), (2, 3), (1, 4), (1, 3), (0, 6), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(1, 6), (1, 4), (1, 5), (3, 3), (3, 2), (2, 2), (2, 3), (1, 3), (0, 6), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(1, 6), (0, 6), (1, 4), (3, 2), (2, 2), (1, 3), (1, 2), (1, 5), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(1, 5), (0, 5), (1, 3), (3, 2), (2, 2), (1, 2), (1, 4), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(0, 4), (1, 4), (1, 3), (2, 3), (1, 1), (3, 3), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(0, 4), (1, 4), (1, 2), (1, 1), (1, 3), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(0, 3), (1, 3), (1, 1), (1, 2), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(0, 2), (1, 2), (1, 1), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(0, 1), (1, 1), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
];
/// total_zeros for 4:2:0 chroma DC by TotalCoeff and total_zeros: Table 9-9a, from OpenH264's g_kuiVlcTotalZerosChromaDc.
const TOTAL_ZEROS_CHROMA_DC: [[(u8, u8); 4]; 4] = [
    [(0, 0), (0, 0), (0, 0), (0, 0)],
    [(1, 1), (1, 2), (1, 3), (0, 3)],
    [(1, 1), (1, 2), (0, 2), (0, 0)],
    [(1, 1), (0, 1), (0, 0), (0, 0)],
];
/// run_before (code, length) by min(zerosLeft, 7) and run_before: Table 9-10, from OpenH264's g_kuiVlcRunBefore.
const RUN_BEFORE: [[(u8, u8); 15]; 8] = [
    [(0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(1, 1), (0, 1), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(1, 1), (1, 2), (0, 2), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(3, 2), (2, 2), (1, 2), (0, 2), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(3, 2), (2, 2), (1, 2), (1, 3), (0, 3), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(3, 2), (2, 2), (3, 3), (2, 3), (1, 3), (0, 3), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(3, 2), (0, 3), (1, 3), (3, 3), (2, 3), (5, 3), (4, 3), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
    [(7, 3), (6, 3), (5, 3), (4, 3), (3, 3), (2, 3), (1, 3), (1, 4), (1, 5), (1, 6), (1, 7), (1, 8), (1, 9), (1, 10), (1, 11)],
];
