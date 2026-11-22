//! Lumis's GPU kernels (the owner's GPU plan, 2026-11-20: "you render every px all at once thru the gpu when anything within that damage rectangle changes, that's it. Straight shot."). Each is one invocation a pixel of a rectangle, computing that pixel from Lumis's own formulas, the scale and every size a uniform: nothing pre-rendered, nothing cached at a size.
//! Recorded with valhall's SSA recorder (as ferros_clip's band coder is), run by std/lumis thru the GPU queue, and checked on the host in valhall's emulator against lumis_core's CPU drawing (tests/), to look the same, not bit for bit.
//!
//! The pixels here are a u32 each, R | G << 8 | B << 16, the low three bytes sRGB-free gamma-2 codes as lumis_core draws them; the layer's own format is std/lumis's to write from them.

use valhall::ssa::{self, F, U};
/// A recorded kernel: its words, the constants its uniforms end with, and whether it runs at 64 registers a thread.
pub use valhall::ssa::Compiled;

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

/// The 8-bit-code pixels' uniforms for `to_g2t16_kernel`.
pub mod g2t {
    /// The source pixels (a u32 each, R | G << 8 | B << 16) and the destination's (fluor's G2t16, a u64 each).
    pub const SRC: u8 = 0;
    pub const DST: u8 = 1;
    /// A code's darkness: 256 u32, entry v fluor's 16-bit darkness for 8-bit code v (std/lumis's `lut`, white less its light).
    pub const LUT: u8 = 2;
    /// (x0, y0), (run, rise): the rectangle, the same place in both.
    pub const ORIGIN: u8 = 3;
    pub const SIZE: u8 = 4;
    /// (the source's stride, the destination's), pixels.
    pub const STRIDES: u8 = 5;
    pub const SLOTS: u8 = 6;
    /// fluor's G2t16 opaque: α = 2^16 - 1 in the top 16 bits.
    pub const ALPHA_OPAQUE: u32 = 0xFFFF;
}

/// A rectangle of 8-bit-code pixels into fluor's G2t16 (`α << 48 | R << 32 | G << 16 | B`, each a 16-bit darkness), opaque, each channel thru the code's table: what the host then bakes and shows (`FluorApp::own_frame`).
pub fn to_g2t16_kernel() -> Compiled {
    use g2t::*;
    ssa::begin();
    ssa::reserve(SLOTS);
    let g = ssa::gid();
    let run_u = ssa::uniform(SIZE, 0);
    let run = F::from_u32(run_u);
    // The row and column from the id, as `slot_kernel` takes them.
    let yu = ((F::from_u32(g) + 0.5) / run).floor().to_u32();
    let xu = g - yu.wrapping_mul(run_u);
    let (x, y) = (ssa::uniform(ORIGIN, 0) + xu, ssa::uniform(ORIGIN, 1) + yu);
    let px = ssa::load(SRC, y.wrapping_mul(ssa::uniform(STRIDES, 0)) + x, 2, 0);
    let dark = |shift: u32| ssa::load(LUT, (px >> shift) & U::splat(0xFF), 2, 0);
    let (r, gr, b) = (dark(0), dark(8), dark(16));
    let at = y.wrapping_mul(ssa::uniform(STRIDES, 1)) + x;
    ssa::store_at(DST, at, 3, 0, (gr << 16) | b);
    ssa::store_at(DST, at, 3, 4, U::splat(ALPHA_OPAQUE << 16) | r);
    ssa::compile()
}

/// The menu's uniforms for `menu_kernel` (lumis_core `ui::draw::compose_cpu`, the same composition on the CPU).
pub mod menu {
    /// The window's pixel 0 in the surface (fluor's G2t16, a u64 each).
    pub const DST: u8 = 0;
    /// The splash, the ink's light (three bytes a pixel each) and its coverage (a byte a pixel).
    pub const BG: u8 = 1;
    pub const INK: u8 = 2;
    pub const COVER: u8 = 3;
    /// Each pixel row's slot: a u32 index into `SLOTS`, 0 the empty record (no slot).
    pub const ROWS: u8 = 4;
    /// The slots, `RECORD_WORDS` u32 a record (`record`), record 0 empty.
    pub const SLOTS: u8 = 5;
    /// A code's darkness, as `g2t::LUT`.
    pub const LUT: u8 = 6;
    /// (width, width x height): the frame, and the invocations that draw it.
    pub const SIZE: u8 = 7;
    /// (the surface's stride in pixels, 0).
    pub const STRIDE: u8 = 8;
    pub const SLOT_COUNT: u8 = 9;
    /// A slot's record: x0, y0, x1, y1 (u32), then big, glow, each channel's k, f and colour, and fa (f32 by their bits).
    pub const RECORD_WORDS: usize = 1 << 4;

    /// lumis_core's slot as its record.
    pub fn record(x0: u32, y0: u32, x1: u32, y1: u32, big: f32, glow: f32, ch: [(f32, f32, f32); 3], fa: f32) -> [u32; RECORD_WORDS] {
        let mut r = [0u32; RECORD_WORDS];
        r[..4].copy_from_slice(&[x0, y0, x1, y1]);
        r[4] = big.to_bits();
        r[5] = glow.to_bits();
        for (c, (k, f, colour)) in ch.iter().enumerate() {
            r[6 + 3 * c] = k.to_bits();
            r[7 + 3 * c] = f.to_bits();
            r[8 + 3 * c] = colour.to_bits();
        }
        r[15] = fa.to_bits();
        r
    }
}

/// The whole menu, an invocation a pixel of the frame (ids 0 to width x height - 1, row by row, past it the last pixel again): the splash, its row's slot over it where the pixel is inside the slot, the ink over both (`light + under (255 - coverage) / 255`), into fluor's G2t16 thru the code table, opaque. Nothing kept from a frame before: every pixel from its parts each time (the owner, 2026-11-20: "you render every px all at once thru the gpu ... Straight shot").
pub fn menu_kernel() -> Compiled {
    use menu::*;
    ssa::begin();
    ssa::reserve(SLOT_COUNT);
    let g = ssa::gid();
    let n = ssa::uniform(SIZE, 1);
    // WHY the select: a job's invocations come in groups of 16, so past a frame whose pixels are not a multiple of 16 they would draw outside it; those draw its last pixel again, the same value.
    let g = U::select(g.lt(n), g, n - U::splat(1));
    let w_u = ssa::uniform(SIZE, 0);
    // The row and column from the id, as `slot_kernel` takes them.
    let yu = ((F::from_u32(g) + 0.5) / F::from_u32(w_u)).floor().to_u32();
    let xu = g - yu.wrapping_mul(w_u);
    let r = ssa::load(ROWS, yu, 2, 0);
    let rec = r << 4;
    let word = |k: i16| ssa::load(SLOTS, rec, 2, 4 * k);
    let fword = |k: i16| F::from_bits(word(k));
    let (x0, y0, x1, y1) = (word(0), word(1), word(2), word(3));
    let inside = xu.ge(x0) & xu.lt(x1) & yu.ge(y0) & yu.lt(y1);
    let (run, rise) = (F::from_u32(x1) - F::from_u32(x0), F::from_u32(y1) - F::from_u32(y0));
    let (x, y) = (F::from_u32(xu) - F::from_u32(x0), F::from_u32(yu) - F::from_u32(y0));
    let w = rim_weight(x, y, run, rise, fword(4), fword(5));
    let w3 = w * w * w;
    let a = w3 * 5.;
    let a = (((a * a * a) - 0.04).abs() * fword(15)).min(f(1.));
    let p = yu.wrapping_mul(w_u) + xu;
    let p3 = p.wrapping_mul(U::splat(3));
    let through = F::from_u32(U::splat(255) - ssa::load8(COVER, p, 0, 0)) * (1. / 255.);
    let mut dark = [U::splat(0); 3];
    for c in 0..3 {
        let bg = F::from_u32(ssa::load8(BG, p3, 0, c as i16));
        let l = w3 * fword(6 + 3 * c as i16);
        let l = f(1.) - ((l * l * l) - 0.5).abs() * fword(7 + 3 * c as i16);
        // WHY the min: the CPU's `as u8` saturates a channel past 255 (a slot's light over what is under it can pass it, as `slot_kernel` says).
        let slot = (l.max(f(0.)) * fword(8 + 3 * c as i16) + bg * a).min(f(255.)).floor();
        let under = F::select(inside, slot, bg);
        // WHY the min: the ink's light and coverage are each rounded to a code, so the sum can pass 255 by one where the CPU's `as u8` saturates; the code table has 256 entries.
        let code = (F::from_u32(ssa::load8(INK, p3, 0, c as i16)) + under * through).min(f(255.)).to_u32();
        dark[c] = ssa::load(LUT, code, 2, 0);
    }
    let at = yu.wrapping_mul(ssa::uniform(STRIDE, 0)) + xu;
    ssa::store_at(DST, at, 3, 0, (dark[1] << 16) | dark[2]);
    ssa::store_at(DST, at, 3, 4, U::splat(g2t::ALPHA_OPAQUE << 16) | dark[0]);
    ssa::compile()
}

/// A frame of Lumis's own drawing (three bytes a pixel, R, G, B: lumis_core's PixelSurface, the camera screen) for `frame_kernel`.
pub mod frame {
    /// The window's pixel 0 in the surface (fluor's G2t16), the frame, a code's darkness (as `g2t::LUT`).
    pub const DST: u8 = 0;
    pub const SRC: u8 = 1;
    pub const LUT: u8 = 2;
    /// (width, width x height), and (the surface's stride in pixels, 0), as `menu`'s.
    pub const SIZE: u8 = 3;
    pub const STRIDE: u8 = 4;
    pub const SLOT_COUNT: u8 = 5;
}

/// Lumis's frame into fluor's G2t16, opaque, an invocation a pixel (ids 0 to width x height - 1, past it the last pixel again), each channel thru the code table: the whole frame each time, as the menu's.
pub fn frame_kernel() -> Compiled {
    use frame::*;
    ssa::begin();
    ssa::reserve(SLOT_COUNT);
    let g = ssa::gid();
    let n = ssa::uniform(SIZE, 1);
    // WHY the select: as `menu_kernel`'s, the invocations come in groups of 16.
    let g = U::select(g.lt(n), g, n - U::splat(1));
    let w_u = ssa::uniform(SIZE, 0);
    let yu = ((F::from_u32(g) + 0.5) / F::from_u32(w_u)).floor().to_u32();
    let xu = g - yu.wrapping_mul(w_u);
    let p3 = g.wrapping_mul(U::splat(3));
    let dark = |c: i16| ssa::load(LUT, ssa::load8(SRC, p3, 0, c), 2, 0);
    let (r, gr, b) = (dark(0), dark(1), dark(2));
    let at = yu.wrapping_mul(ssa::uniform(STRIDE, 0)) + xu;
    ssa::store_at(DST, at, 3, 0, (gr << 16) | b);
    ssa::store_at(DST, at, 3, 4, U::splat(g2t::ALPHA_OPAQUE << 16) | r);
    ssa::compile()
}
