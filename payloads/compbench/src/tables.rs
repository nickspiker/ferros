//! The bake's tables and the frame's fixed inputs, made the same way on the device (main.rs) and the host (tools/compbench-check): the display's bake.rs `set` for one white, written where the GPU reads them.

use crate::layout::*;

/// The ODT over the light table's scale (2^14 at the E peak): husky's (the display's odt.rs `ODT_HUSKY`) or akita's.
pub fn odt_scaled(akita: bool) -> [f32; 9] {
    const HUSKY: [[f32; 3]; 3] = [[1.359550, -0.372458, 0.012909], [-0.098444, 0.863965, -0.026116], [0.001826, -0.001101, 0.816481]];
    const AKITA: [[f32; 3]; 3] = [[1.3065, -0.3222, 0.0157], [-0.0987, 0.8829, -0.0309], [0.0018, -0.0024, 0.7971]];
    let m = if akita { AKITA } else { HUSKY };
    core::array::from_fn(|i| m[i / 3][i % 3] * (1. / (1 << 14) as f32))
}

/// The panel's gamma (odt.rs `panel_gamma`).
pub fn gamma(akita: bool) -> f32 {
    if akita { 2.23 } else { 2.19 }
}

/// The roll-off's gain for a white at `white_over_peak` of the E peak (bake.rs `set`).
pub fn gain(white_over_peak: f64) -> f64 {
    libm::atanh(libm::sqrt(white_over_peak))
}

/// x^p for x > 0 (odt.rs `pow`).
pub fn pow(x: f32, p: f32) -> f32 {
    if x <= 0. {
        return 0.;
    }
    libm::exp2f(p * libm::log2f(x))
}

/// bake.rs `TOP`: the baked code at the panel's peak.
pub const TOP: f32 = 65472.;

/// Each 16-bit g2t code's light, f16 bits, the E peak at 2^14 (bake.rs `Bake::light`).
pub fn light_entry(k: f64, c: usize, f16: impl Fn(f32) -> u16) -> u16 {
    let t = libm::tanh(k * libm::atanh(c as f64 * (1. / 65536.)));
    f16((t * t * 16384.) as f32)
}

/// √p in `CODE_STEPS` steps to the baked code (bake.rs `Bake::code`'s table), one past for the interpolation.
pub fn code_entry(i: usize, akita: bool) -> f32 {
    pow(i as f32 * (1. / CODE_STEPS as f32), 2. / gamma(akita)) * TOP
}

/// The scalar bake of one opaque G2t16 pixel, in the GPU kernel's order of operations (kernel.rs `bake`): the reference the GPU's codes are measured against.
pub fn bake_px(p: u64, light: &[u16], code: &[f32], odt: &[f32; 9], f16: impl Fn(u16) -> f32) -> [u32; 3] {
    let a = p >> 48;
    let l = [p >> 32 & 0xFFFF, p >> 16 & 0xFFFF, p & 0xFFFF].map(|d| f16(light[(a - d) as usize]));
    core::array::from_fn(|r| {
        let s = libm::fmaf(odt[3 * r], l[0], odt[3 * r + 1] * l[1]);
        let p = libm::fmaf(odt[3 * r + 2], l[2], s);
        // WHY: as the kernel's clamp (kernel.rs `bake`): the table's index stays inside it.
        let q = if p > 1. { 1. } else if p > 0. { p } else { 0. };
        let x = libm::sqrtf(q) * CODE_STEPS as f32;
        let fl = libm::floorf(x);
        let i = fl as usize;
        (code[i] + (code[i + 1] - code[i]) * (x - fl)) as u32
    })
}

/// A baked pixel (bake.rs's layout: R, G, B in lanes 0-2, the fourth an opaque pixel's 2).
pub fn baked(c: [u32; 3]) -> u64 {
    c[0] as u64 | (c[1] as u64) << 16 | (c[2] as u64) << 32 | 2 << 48
}

/// The present's word for one pixel's codes and threshold (neon.rs `present_pack`): each code plus the threshold, saturating, its high byte; packed B[31:22] G[21:12] R[11:2] A[1:0].
pub fn word(c: [u32; 3], t: u32) -> u32 {
    let v = |c: u32| ((c + t) >> 8) - ((c + t) >> 16);
    v(c[2]) << 24 | v(c[1]) << 14 | v(c[0]) << 4 | 3
}

/// The present's blue-noise field (the display's neon.rs `BLUE`) and each phase's turn of its two low bits.
pub const BLUE: [[u8; 16]; 16] = [
    [78, 190, 245, 170, 143, 39, 192, 90, 172, 247, 163, 138, 7, 187, 133, 236],
    [3, 152, 47, 71, 210, 120, 234, 0, 213, 114, 20, 203, 83, 55, 208, 98],
    [221, 112, 135, 229, 17, 96, 159, 70, 140, 48, 97, 232, 176, 121, 23, 165],
    [41, 201, 26, 82, 174, 50, 200, 30, 184, 243, 155, 66, 33, 149, 240, 69],
    [88, 252, 158, 195, 129, 248, 108, 224, 80, 118, 15, 188, 91, 215, 110, 183],
    [14, 123, 60, 102, 5, 73, 147, 13, 169, 44, 207, 130, 251, 8, 51, 142],
    [168, 218, 34, 209, 238, 186, 38, 217, 134, 239, 100, 59, 160, 77, 199, 235],
    [99, 74, 179, 139, 53, 164, 122, 92, 64, 180, 4, 227, 31, 173, 119, 29],
    [154, 246, 11, 113, 84, 22, 233, 191, 25, 153, 115, 193, 141, 94, 220, 57],
    [136, 43, 194, 231, 151, 211, 105, 54, 253, 85, 219, 72, 49, 249, 12, 189],
    [212, 81, 124, 65, 35, 178, 2, 145, 204, 36, 131, 19, 205, 157, 76, 111],
    [228, 6, 167, 206, 95, 241, 128, 67, 166, 109, 185, 237, 101, 127, 181, 32],
    [150, 106, 250, 21, 146, 46, 216, 89, 244, 10, 63, 161, 45, 1, 242, 62],
    [177, 40, 137, 61, 196, 116, 18, 182, 42, 148, 230, 87, 214, 144, 198, 93],
    [226, 202, 86, 223, 175, 79, 156, 225, 107, 197, 125, 28, 171, 68, 117, 16],
    [56, 126, 27, 104, 9, 254, 58, 132, 24, 75, 52, 222, 103, 255, 37, 162],
];
pub const TURN: [u8; 4] = [0, 2, 3, 1];

/// A run's uniforms: the caller's slots (`slot`), then the kernel's constants.
pub struct Run {
    pub y0: u32,
    pub rows: u32,
    pub padded: u32,
    pub x0: u32,
    pub x1: u32,
    pub first_left: bool,
    pub steps: (u32, u32),
    /// Sides dispatched (1 or 2): the grid is `padded × sides / 16` groups.
    pub sides: u32,
}

impl Run {
    /// The whole frame: every row, both halves, every column.
    pub fn frame() -> Run {
        Run { y0: 0, rows: H as u32, padded: H as u32, x0: 0, x1: W as u32, first_left: false, steps: (AXIS as u32, AXIS as u32), sides: 2 }
    }
    /// A damage rect [x0, x1) × [y0, y1): its rows padded to whole groups, the sides it reaches, each walking from the axis to the rect's far edge on that side.
    pub fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Run {
        let rows = (y1 - y0) as u32;
        let padded = (rows + 15) & !15;
        let (right, left) = (x1 > AXIS, x0 < AXIS);
        let steps = (if right { (x1 - AXIS) as u32 } else { 0 }, if left { (AXIS - x0) as u32 } else { 0 });
        Run { y0: y0 as u32, rows, padded, x0: x0 as u32, x1: x1 as u32, first_left: !right, steps, sides: right as u32 + left as u32 }
    }
    pub fn groups(&self) -> u32 {
        self.padded * self.sides / 16
    }
}

/// The uniforms for `run`: `va` the block's GPU address, `scroll` the noise's texture scroll, `offset` the pane's row at the screen's top, the walk's mask, speckle and base (fluor's theme, 16-bit lanes), the ODT, then the kernel's `consts`.
#[allow(clippy::too_many_arguments)]
pub fn uniforms(out: &mut [u64; 64], run: &Run, va: u64, out_at: usize, scroll: u32, offset: u32, walk: (u64, u64, u64), odt: &[f32; 9], consts: &[u64]) -> usize {
    use crate::layout::slot::*;
    let pair = |a: u32, b: u32| a as u64 | (b as u64) << 32;
    out[OUT as usize] = va + out_at as u64;
    out[PANE as usize] = va + PANE_AT as u64;
    out[CHROME as usize] = va + CHROME_AT as u64;
    out[SEED as usize] = va + SEED_AT as u64;
    out[LIGHT as usize] = va + LIGHT_AT as u64;
    out[CODE as usize] = va + CODE_AT as u64;
    out[BLUE as usize] = va + BLUE_AT as u64;
    for k in 0..4 {
        out[PHASE as usize + k] = va + (PHASES_AT + k * PHASE_BYTES) as u64;
    }
    out[SCROLL as usize] = pair(scroll, offset);
    out[ROWS as usize] = pair(run.y0, run.padded);
    out[COLS as usize] = pair(run.x0, run.x1);
    out[SIDE as usize] = pair(run.first_left as u32, run.rows);
    out[STEPS as usize] = pair(run.steps.0, run.steps.1);
    out[MASK as usize] = walk.0;
    out[SPECKLE as usize] = walk.1;
    out[BASE as usize] = walk.2;
    out[STIR_RIGHT as usize] = STIR;
    out[STIR_LEFT as usize] = STIR.wrapping_neg();
    for i in 0..5 {
        let w = |j: usize| if j < 9 { odt[j].to_bits() } else { 0 };
        out[ODT as usize + i] = pair(w(2 * i), w(2 * i + 1));
    }
    let n = RESERVED as usize;
    out[n..n + consts.len()].copy_from_slice(consts);
    n + consts.len()
}
