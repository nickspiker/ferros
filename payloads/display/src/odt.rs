//! The present stage's ODT: the panel's measured transfer and primaries and the tables `neon::sixteen_canvas` runs on (`odt_tables`). Its own file so the GPU driver builds the same tables for its present-stage kernel and checks it against the NEON path (payloads/gpu, GPU.md "The present stage"); the display includes it as `mod odt`.

use crate::neon;

/// x^p for x > 0, through exp2/log2.
pub fn pow(x: f32, p: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    libm::exp2f(p * libm::log2f(x))
}

/// The panel's measured power law (γ 2.19 in 8- and 10-bit codes, to 10-bit code 8; DISPLAY-CALIB.md).
pub const PANEL_GAMMA: f32 = 2.19;

/// The ODT, VSF RGB → panel linear RGB (`tools/panel-white-e.py`, the DBV 3307 readings): the panel's three primaries from the profile's spectra under VSF's observer, the matrix they make inverted; rows are panel R, G, B, columns VSF R, G, B. Absolute (E in is E on the glass), so its row sums are E's gains. Model tier, from one unit (pixel 8 pro calibration data/README.md). Applied to every pixel by `neon::Odt` (neon.rs), with no clamp: out-of-gamut channels land at 0 or at the panel's top through the output tables (owner, 2026-10-24); every sRGB colour lands inside, so only colour beyond sRGB ever meets them.
pub const ODT: [[f32; 3]; 3] = [[1.359550, -0.372458, 0.012909], [-0.098444, 0.863965, -0.026116], [0.001826, -0.001101, 0.816481]];
pub const E_GAIN: [f32; 3] = [ODT[0][0] + ODT[0][1] + ODT[0][2], ODT[1][0] + ODT[1][1] + ODT[1][2], ODT[2][0] + ODT[2][1] + ODT[2][2]];

/// The ODT's light scale: the E peak at 2^14 in f16, so 2^-28 of the peak is f16's smallest normal.
pub const F16_SCALE: f64 = 16384.0;

/// An f32 to f16 bits, by the cores' own convert (Rust's f16 is unstable).
pub fn f32_to_f16(x: f32) -> u16 {
    let h: u32;
    unsafe { core::arch::asm!("fcvt {h:h}, {x:s}", "umov {o:w}, {h:v}.h[0]", h = out(vreg) _, x = in(vreg) x, o = out(reg) h, options(pure, nomem, nostack)) };
    h as u16
}

/// The ODT's constants and tables (`neon::Odt`) for a white and the E peak it rolls off into (`build_lut`'s k = atanh(√(white/peak))):
/// - the app light's k/16384 and 2^14;
/// - the output tables: per exponent, twice the code at the octave's foot, 2·16368·2^((e − 29)/2.19) (e = 0: zero and subnormals, under 2^-28 of the peak, code 0; e = 30, 31: past the peak, the most a u16 holds, so the dither saturates); per top six mantissa bits, the nudge (1 + i/64)^(1/2.19) in Q15 and its slope to the next.
pub fn odt_tables(white: f32, peak_e: f32) -> neon::Odt {
    use core::arch::aarch64::*;
    let k = libm::atanh(libm::sqrt((white / peak_e).min(0.999) as f64));
    let h = |x: f64| f32_to_f16(x as f32);
    let gm = 1.0 / PANEL_GAMMA;
    let nudge = |i: usize| (pow(1.0 + i as f32 / 64.0, gm) * 32768.0 + 0.5) as u32;
    let (mut nudge_hi, mut nudge_lo, mut slope) = ([0u8; 64], [0u8; 64], [0u8; 64]);
    for i in 0..64 {
        let n = nudge(i);
        nudge_hi[i] = (n >> 8) as u8;
        nudge_lo[i] = n as u8;
        slope[i] = (nudge(i + 1) - n) as u8;
    }
    let (mut scale_hi, mut scale_lo) = ([0u8; 32], [0u8; 32]);
    for e in 1..32 {
        let s = if e < 30 { (2.0 * 16368.0 * pow(2.0, (e as f32 - 29.0) * gm) + 0.5) as u32 } else { 65535 };
        scale_hi[e] = (s >> 8) as u8;
        scale_lo[e] = s as u8;
    }
    let kk = |r: usize, c: usize| (ODT[r][c] - if r == c { E_GAIN[r] } else { 0.0 }) as f64;
    let c0 = [E_GAIN[0] as f64, E_GAIN[1] as f64, E_GAIN[2] as f64, kk(0, 0), kk(0, 2), kk(1, 0), kk(1, 2), kk(2, 0)].map(h);
    let c1 = [kk(2, 2), 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0].map(h);
    let kf = [(k / 16384.0) as f32, F16_SCALE as f32, 0.0, 0.0];
    unsafe {
        neon::Odt {
            nudge_hi: vld1q_u8_x4(nudge_hi.as_ptr()),
            nudge_lo: vld1q_u8_x4(nudge_lo.as_ptr()),
            slope: vld1q_u8_x4(slope.as_ptr()),
            scale_hi: vld1q_u8_x2(scale_hi.as_ptr()),
            scale_lo: vld1q_u8_x2(scale_lo.as_ptr()),
            c0: vld1q_u16(c0.as_ptr()),
            c1: vld1q_u16(c1.as_ptr()),
            k: vld1q_f32(kf.as_ptr()),
        }
    }
}
