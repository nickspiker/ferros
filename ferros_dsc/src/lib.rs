//! VESA DSC 1.1 picture parameter set: the 128 bytes a DSI panel needs to decode the compressed stream, packed from the encoder's configuration (DISPLAY-HK3.md §2 holds husky's two configs). Tested against the PPS words Android's DECON held for both of them (oracle/display/husky-hk3, the DSC block's PPS registers).

#![no_std]

/// The DSC 1.1 configuration (the fields of Linux's drm_dsc_config that reach the PPS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub bits_per_component: u8,
    pub line_buf_depth: u8,
    pub block_pred_enable: bool,
    pub convert_rgb: bool,
    pub simple_422: bool,
    pub vbr_enable: bool,
    /// In 1/16 bpp units (128 = 8.0 bpp).
    pub bits_per_pixel: u16,
    pub pic_height: u16,
    pub pic_width: u16,
    pub slice_height: u16,
    pub slice_width: u16,
    pub slice_chunk_size: u16,
    pub initial_xmit_delay: u16,
    pub initial_dec_delay: u16,
    pub initial_scale_value: u8,
    pub scale_increment_interval: u16,
    pub scale_decrement_interval: u16,
    pub first_line_bpg_offset: u8,
    pub nfl_bpg_offset: u16,
    pub slice_bpg_offset: u16,
    pub initial_offset: u16,
    pub final_offset: u16,
    pub flatness_min_qp: u8,
    pub flatness_max_qp: u8,
    pub rc_model_size: u16,
    pub rc_edge_factor: u8,
    pub rc_quant_incr_limit0: u8,
    pub rc_quant_incr_limit1: u8,
    pub rc_tgt_offset_high: u8,
    pub rc_tgt_offset_low: u8,
    /// Thresholds in full units (14 × 64 …); the PPS stores them >> 6.
    pub rc_buf_thresh: [u16; 14],
    /// (range_min_qp, range_max_qp, range_bpg_offset) with the offset as the raw 6-bit two's-complement value the driver tables carry.
    pub rc_range_params: [(u8, u8, u8); 15],
}

/// The fields every husky mode shares (DISPLAY-HK3.md §2).
const fn husky(pic_width: u16, pic_height: u16, slice_width: u16, initial_dec_delay: u16, scale_increment_interval: u16, scale_decrement_interval: u16, slice_bpg_offset: u16) -> Config {
    Config {
        bits_per_component: 8,
        line_buf_depth: 9,
        block_pred_enable: true,
        convert_rgb: true,
        simple_422: false,
        vbr_enable: false,
        bits_per_pixel: 128,
        pic_height,
        pic_width,
        slice_height: 187,
        slice_width,
        slice_chunk_size: slice_width,
        initial_xmit_delay: 512,
        initial_dec_delay,
        initial_scale_value: 32,
        scale_increment_interval,
        scale_decrement_interval,
        first_line_bpg_offset: 12,
        nfl_bpg_offset: 133,
        slice_bpg_offset,
        initial_offset: 6144,
        final_offset: 4336,
        flatness_min_qp: 3,
        flatness_max_qp: 12,
        rc_model_size: 8192,
        rc_edge_factor: 6,
        rc_quant_incr_limit0: 11,
        rc_quant_incr_limit1: 11,
        rc_tgt_offset_high: 3,
        rc_tgt_offset_low: 3,
        rc_buf_thresh: [896, 1792, 2688, 3584, 4480, 5376, 6272, 6720, 7168, 7616, 7744, 7872, 8000, 8064],
        rc_range_params: [(0, 4, 2), (0, 4, 0), (1, 5, 0), (1, 6, 62), (3, 7, 60), (3, 7, 58), (3, 7, 56), (3, 8, 56), (3, 9, 56), (3, 10, 54), (5, 11, 54), (5, 12, 52), (5, 13, 52), (7, 13, 52), (13, 15, 52)],
    }
}

/// husky WQHD: 1344×2992, two 672×187 slices per line.
pub const HUSKY_WQHD: Config = husky(1344, 2992, 672, 592, 5177, 9, 112);
/// husky FHD: 1008×2244, two 504×187 slices per line, scaled up in the panel.
pub const HUSKY_FHD: Config = husky(1008, 2244, 504, 508, 4482, 7, 150);

/// Pack the 128-byte PPS (DSC 1.1 §4.1 / drm_dsc_pps_payload_pack). Reserved bytes are zero.
pub fn pps(c: &Config) -> [u8; 128] {
    let mut p = [0u8; 128];
    let be = |p: &mut [u8; 128], i: usize, v: u16| {
        p[i] = (v >> 8) as u8;
        p[i + 1] = v as u8;
    };
    p[0] = 0x11; // dsc_version_major 1, minor 1
    p[1] = 0; // pps_identifier
    p[3] = c.bits_per_component << 4 | (c.line_buf_depth & 0xF);
    p[4] = (c.block_pred_enable as u8) << 5 | (c.convert_rgb as u8) << 4 | (c.simple_422 as u8) << 3 | (c.vbr_enable as u8) << 2 | ((c.bits_per_pixel >> 8) as u8 & 0x3);
    p[5] = c.bits_per_pixel as u8;
    be(&mut p, 6, c.pic_height);
    be(&mut p, 8, c.pic_width);
    be(&mut p, 10, c.slice_height);
    be(&mut p, 12, c.slice_width);
    be(&mut p, 14, c.slice_chunk_size);
    be(&mut p, 16, c.initial_xmit_delay & 0x3FF);
    be(&mut p, 18, c.initial_dec_delay);
    p[21] = c.initial_scale_value & 0x3F;
    be(&mut p, 22, c.scale_increment_interval);
    be(&mut p, 24, c.scale_decrement_interval & 0xFFF);
    p[27] = c.first_line_bpg_offset & 0x1F;
    be(&mut p, 28, c.nfl_bpg_offset);
    be(&mut p, 30, c.slice_bpg_offset);
    be(&mut p, 32, c.initial_offset);
    be(&mut p, 34, c.final_offset);
    p[36] = c.flatness_min_qp & 0x1F;
    p[37] = c.flatness_max_qp & 0x1F;
    be(&mut p, 38, c.rc_model_size);
    p[40] = c.rc_edge_factor & 0xF;
    p[41] = c.rc_quant_incr_limit0 & 0x1F;
    p[42] = c.rc_quant_incr_limit1 & 0x1F;
    p[43] = (c.rc_tgt_offset_high & 0xF) << 4 | (c.rc_tgt_offset_low & 0xF);
    for (i, t) in c.rc_buf_thresh.iter().enumerate() {
        p[44 + i] = (t >> 6) as u8;
    }
    for (i, &(min_qp, max_qp, bpg)) in c.rc_range_params.iter().enumerate() {
        be(&mut p, 58 + 2 * i, (min_qp as u16 & 0x1F) << 11 | (max_qp as u16 & 0x1F) << 6 | (bpg as u16 & 0x3F));
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PPS words Android's DECON held (DSC block PPS00..PPS71), oracle W120_noidle / F120_noidle.
    const WQHD_WORDS: [u32; 18] = [0x11000089, 0x30800bb0, 0x054000bb, 0x02a002a0, 0x02000250, 0x00201439, 0x0009000c, 0x00850070, 0x180010f0, 0x030c2000, 0x060b0b33, 0x0e1c2a38, 0x46546269, 0x7077797b, 0x7d7e0102, 0x01000940, 0x09be19fc, 0x19fa19f8];
    const FHD_WORDS: [u32; 18] = [0x11000089, 0x308008c4, 0x03f000bb, 0x01f801f8, 0x020001fc, 0x00201182, 0x0007000c, 0x00850096, 0x180010f0, 0x030c2000, 0x060b0b33, 0x0e1c2a38, 0x46546269, 0x7077797b, 0x7d7e0102, 0x01000940, 0x09be19fc, 0x19fa19f8];

    fn words(p: &[u8; 128]) -> [u32; 18] {
        core::array::from_fn(|i| u32::from_be_bytes([p[4 * i], p[4 * i + 1], p[4 * i + 2], p[4 * i + 3]]))
    }

    #[test]
    fn wqhd_matches_android() {
        assert_eq!(words(&pps(&HUSKY_WQHD)), WQHD_WORDS);
    }

    #[test]
    fn fhd_matches_android() {
        assert_eq!(words(&pps(&HUSKY_FHD)), FHD_WORDS);
    }

    #[test]
    fn remaining_range_params_and_reserved() {
        let p = pps(&HUSKY_WQHD);
        // Params 7..15 follow the same encoding past the registers DECON exposes; the tail is reserved.
        assert_eq!(&p[72..88], &[0x1a, 0x38, 0x1a, 0x78, 0x1a, 0xb6, 0x2a, 0xf6, 0x2b, 0x34, 0x2b, 0x74, 0x3b, 0x74, 0x6b, 0xf4]);
        assert!(p[88..].iter().all(|&b| b == 0));
    }
}
