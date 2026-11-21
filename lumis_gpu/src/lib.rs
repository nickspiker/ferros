//! Lumis's GPU kernels (the owner's GPU plan, 2026-11-20: "you render every px all at once thru the gpu when anything within that damage rectangle changes, that's it. Straight shot."). Each is one invocation a pixel of a rectangle, computing that pixel from Lumis's own formulas, the scale and every size a uniform: nothing pre-rendered, nothing cached at a size.
//! Recorded with valhall's SSA recorder (as ferros_clip's band coder is), run by std/lumis thru the GPU queue, and checked on the host in valhall's emulator against lumis_core's CPU drawing (tests/), to look the same, not bit for bit.
//!
//! The pixels here are a u32 each, R | G << 8 | B << 16, the low three bytes sRGB-free gamma-2 codes as lumis_core draws them; the layer's own format is std/lumis's to write from them.

use valhall::ssa::{self, Compiled, F, U};

/// A slot's uniforms (lumis_core `ui::draw::Slot`), each a 64-bit slot of two 32-bit words, floats by their bits.
pub mod slot {
    /// The pixels' address (64 bits).
    pub const OUT: u8 = 0;
    /// (x0, y0): the rectangle's top left, pixels.
    pub const ORIGIN: u8 = 1;
    /// (run, rise): its size, pixels.
    pub const SIZE: u8 = 2;
    /// (stride, 0): pixels from one row to the next.
    pub const STRIDE: u8 = 3;
    /// (big, glow): the corners' size and the glow's depth.
    pub const SHAPE: u8 = 4;
    /// (k, f) of red, green and blue.
    pub const CH: [u8; 3] = [5, 6, 7];
    /// (red's colour, green's colour).
    pub const COLOUR_RG: u8 = 8;
    /// (blue's colour, the alpha's fa).
    pub const COLOUR_B_FA: u8 = 9;
    /// The caller's slots.
    pub const SLOTS: u8 = 10;
}

fn f(v: f32) -> F {
    F::splat(v)
}

/// A float uniform word.
fn uf(slot: u8, word: u8) -> F {
    F::from_bits(ssa::uniform(slot, word))
}

/// lumis_core `buttons::rim_weight` at (x, y) of a `run` x `rise` rectangle, corners over `big`, glow `width` deep, at power 5.
fn rim_weight(x: F, y: F, run: F, rise: F, big: F, width: F) -> F {
    // The sigmoid's steepness k = 2^2, as the CPU's.
    let k = (1 << 2) as f32;
    let norm = (1. + k * k).sqrt();
    let s = |t: F| t * norm / (t * t * (k * k) + 1.).sqrt();
    let u = (x * 2. + 1.) / run - 1.;
    let v = (y * 2. + 1.) / rise - 1.;
    let r = big * (s(u) * s(v) * 0.25 + 0.75);
    let dx = x.min(run - 1. - x);
    let dy = y.min(rise - 1. - y);
    let (cx, cy) = (r - dx, r - dy);
    // The corner's squircle, (cx^3 + cy^3)^(1/3) by the hardware's log2 and exp2 (Valhall has no cube root).
    let z = cx * cx * cx + cy * cy * cy;
    let corner = r - (z.log2_hw() * (1. / 3.)).exp2_hw();
    let d = F::select(dx.lt(r), F::select(dy.lt(r), corner, dx.min(dy)), dx.min(dy));
    // WHY the max: past `width` in from the edge the rim has no weight (the CPU's max, the button's flat face).
    (f(1.) - d / width).max(f(0.)).powi(5)
}

/// One slot (lumis_core `ui::draw::Slot`) over what is in the pixels, an invocation a pixel of its rectangle (ids 0 to run x rise - 1, row by row).
pub fn slot_kernel() -> Compiled {
    use slot::*;
    ssa::begin();
    ssa::reserve(SLOTS);
    let g = ssa::gid();
    let run_u = ssa::uniform(SIZE, 0);
    let (run, rise) = (F::from_u32(run_u), F::from_u32(ssa::uniform(SIZE, 1)));
    // The pixel's row and column from its id, with no integer division (Valhall has none): (id + 1/2) / run is at least half a pixel from a whole number, so its floor is the row exactly while run x rise is under 2^24.
    let yf = ((F::from_u32(g) + 0.5) / run).floor();
    let yu = yf.to_u32();
    let xu = g - yu.wrapping_mul(run_u);
    let (x, y) = (F::from_u32(xu), yf);
    let w = rim_weight(x, y, run, rise, uf(SHAPE, 0), uf(SHAPE, 1));
    let w3 = w * w * w;
    let a = w3 * 5.;
    let a = ((a * a * a) - 0.04).abs() * uf(COLOUR_B_FA, 1);
    let a = a.min(f(1.));
    let at = (ssa::uniform(ORIGIN, 1) + yu).wrapping_mul(ssa::uniform(STRIDE, 0)) + ssa::uniform(ORIGIN, 0) + xu;
    let under = ssa::load(OUT, at, 2, 0);
    let colour = [uf(COLOUR_RG, 0), uf(COLOUR_RG, 1), uf(COLOUR_B_FA, 0)];
    let mut out = U::splat(0);
    for c in 0..3 {
        let l = w3 * uf(CH[c], 0);
        let l = f(1.) - ((l * l * l) - 0.5).abs() * uf(CH[c], 1);
        let was = F::from_u32((under >> (8 * c as u32)) & U::splat(0xFF));
        // WHY the min: the CPU's `as u8` saturates a channel past 255 at 255 (its light over what is under can pass it); the same cast's bound here, nothing more.
        let v = (l.max(f(0.)) * colour[c] + was * a).min(f(255.));
        out = out | (v.to_u32() << (8 * c as u32));
    }
    ssa::store_at(OUT, at, 2, 0, out);
    ssa::compile()
}
