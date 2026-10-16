//! A pure-Rust H.264 encoder for the beam (BEAM.md: Photon's beam is H.264 Annex-B; every Photon decodes with OpenH264).
//!
//! Stage one is I_PCM: Constrained Baseline, every frame an IDR carrying its own parameter sets, every macroblock its raw samples. No transform, no prediction, no entropy coder, so it is small and exact: a decoder returns precisely the pixels given (the test oracle), at about 1.5 bytes per pixel. Compression (intra prediction + CAVLC) builds on this same bitstream skeleton.

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

/// One access unit: SPS, PPS and an IDR slice of I_PCM macroblocks. `idr_pic_id` must differ between consecutive IDRs (alternate 0 and 1).
pub fn encode_pcm_idr(pic: &Picture, idr_pic_id: u32) -> Vec<u8> {
    let (w, h) = (pic.width, pic.height);
    let (mw, mh) = (w.div_ceil(16), h.div_ceil(16));
    let mut au = Vec::with_capacity(mw * mh * 386 + 64);
    nal(&mut au, 3, 7, &sps(w, h));
    nal(&mut au, 3, 8, &pps());
    let mut b = Bits::new();
    b.ue(0); // first_mb_in_slice
    b.ue(7); // slice_type: I, every slice of the picture
    b.ue(0); // pic_parameter_set_id
    b.bits(0, 4); // frame_num (0 for an IDR)
    b.ue(idr_pic_id);
    b.bit(false); // no_output_of_prior_pics_flag
    b.bit(false); // long_term_reference_flag
    b.se(0); // slice_qp_delta
    b.ue(1); // disable_deblocking_filter_idc: off (I_PCM is exact)
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
