//! The frame's GPU kernels, recorded thru valhall::ssa on the host (build.rs) and shipped as words, as ferros_clip's band coder is (the device never compiles them).
//!
//! One invocation a half-row: fluor's noise is a serial walk from the axis outward (noise.rs), so an invocation walks its row's half from the axis, one pixel a pass of the kernel's one loop, and at each pixel reads the chrome (its rows only) and the pane at its offset, folds them front to back with fluor's `under`, lays the noise under what is not opaque, and stores. A full frame is 2 × 2992 = 5984 invocations of 672 passes. The loop's carried values are the walk's generator and colour (two 32-bit halves each), the pass and the column.
//!
//! Every 64-bit step of the walk is done in 32-bit halves: Valhall's integer ALU is 32 bits wide. The seed's 64-bit multiply is the CPU's, a table of a u64 a texture row (`layout::SEED_AT`).
//!
//! The kernel subset of GPU-KISTS.md 1.1: no textures, no workgroup memory, no barriers; loads and stores of the client's own block only.

use crate::layout::{slot, AXIS};
use valhall::ssa::{self, Compiled, F, H, M, U};

/// What a kernel writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    /// fluor's G2t16 pixels (what fluor's canvas holds: the display's format 1).
    G2t,
    /// The display's baked pixels (format 2: the panel's Q16 codes, bake.rs).
    Baked,
    /// The baked codes dithered and packed into the four phases' words (the display's present, neon.rs `present_app16`).
    Phases,
}

/// A kernel's shape: what it writes, whether it reads the chrome and the pane (false: the noise alone), and whether its stores keep to a rectangle (`slot::COLS`, the rows a side: a damage rect) or cover every column its invocations walk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Shape {
    pub out: Out,
    pub layers: bool,
    pub clip: bool,
}

/// A 64-bit value as its two halves.
#[derive(Clone, Copy)]
struct W {
    lo: U,
    hi: U,
}

fn wconst(v: u64) -> W {
    W { lo: U::splat(v as u32), hi: U::splat((v >> 32) as u32) }
}
fn wuniform(s: u8) -> W {
    W { lo: ssa::uniform(s, 0), hi: ssa::uniform(s, 1) }
}
/// a + b, the low half's carry into the high (an unsigned compare's mask is all ones, so subtracting it adds one).
fn add(a: W, b: W) -> W {
    let lo = a.lo + b.lo;
    W { lo, hi: a.hi + b.hi - U::from_mask(lo.lt(a.lo)) }
}
/// a − b, the low half's borrow from the high.
fn sub(a: W, b: W) -> W {
    W { lo: a.lo - b.lo, hi: a.hi - b.hi + U::from_mask(a.lo.lt(b.lo)) }
}
fn and(a: W, b: W) -> W {
    W { lo: a.lo & b.lo, hi: a.hi & b.hi }
}
fn xor(a: W, b: W) -> W {
    W { lo: a.lo ^ b.lo, hi: a.hi ^ b.hi }
}
/// a >> s, 0 < s < 32.
fn shr(a: W, s: u32) -> W {
    W { lo: (a.lo >> s) | (a.hi << (32 - s)), hi: a.hi >> s }
}
/// a rotated left by 13.
fn rotl13(a: W) -> W {
    W { lo: (a.lo << 13) | (a.hi >> 19), hi: (a.hi << 13) | (a.lo >> 19) }
}
fn select(m: M, a: W, b: W) -> W {
    W { lo: U::select(m, a.lo, b.lo), hi: U::select(m, a.hi, b.hi) }
}

/// fluor's `under` (pixel.rs `impl_under`, Normal, 16 bits) on G2t16 halves: hi = α << 16 | R, lo = G << 16 | B. The top kept where it is opaque.
/// Every product fits 32 bits: (2^16 − α) · α' ≤ 2^16 · (2^16 − 1) and each deposit (2^16 − 1)² (fluor's own proof, `under_x8_normal_g2t16`).
fn under(t: W, b: W) -> W {
    let field = U::splat(0xFFFF);
    let ta = t.hi >> 16;
    let consumed = (U::splat(1 << 16) - ta).wrapping_mul(b.hi >> 16) >> 16;
    let r = (t.hi & field) + ((b.hi & field).wrapping_mul(consumed) >> 16);
    let g = (t.lo >> 16) + ((b.lo >> 16).wrapping_mul(consumed) >> 16);
    let bl = (t.lo & field) + ((b.lo & field).wrapping_mul(consumed) >> 16);
    let a = ta + consumed;
    let out = W { hi: (a << 16) | r, lo: (g << 16) | bl };
    select(ta.eq(field), t, out)
}

/// Load a G2t16 pixel: word `index` of slot `s`'s u64 array.
fn load_px(s: u8, index: U) -> W {
    W { lo: ssa::load(s, index, 3, 0), hi: ssa::load(s, index, 3, 4) }
}
fn store_px(s: u8, index: U, p: W) {
    ssa::store_at(s, index, 3, 0, p.lo);
    ssa::store_at(s, index, 3, 4, p.hi);
}

/// One channel's darkness (opaque: α is whole) to its panel light, then the three lights thru the ODT to the panel's Q16 codes: the display's bake (bake.rs `g2t_row`, `code`) in scalar f32, the table's steps read linearly.
fn bake(acc: W, odt: &[F; 9]) -> [U; 3] {
    let field = U::splat(0xFFFF);
    let a = acc.hi >> 16;
    // The light index α − darkness; darkness ≤ α is fluor's invariant and the composite's own (it is ours, never another process's).
    let light = |d: U| H::from_bits(ssa::load16(slot::LIGHT, a - d, 1, 0)).lo_f();
    let l = [light(acc.hi & field), light(acc.lo >> 16), light(acc.lo & field)];
    core::array::from_fn(|r| {
        let p = odt[3 * r] * l[0] + odt[3 * r + 1] * l[1] + odt[3 * r + 2] * l[2];
        // WHY the clamp (free: the destination clamp of the add that made p): the code table's index is √p · 2^12 and its second read one past it, so p must lie in [0, 1] for both to stay inside the table's 2^12 + 2 entries (a read past it is another buffer's bytes, or a fault). The CPU's bake does the same at its ends: a negative light (out of gamut) is NaN and casts to code 0, and past the peak its cast saturates (bake.rs `code`); here past the peak is the peak's code (65472 where the CPU's reaches 65535, a quarter of an 8-bit code).
        let q = p.max(F::splat(0.)).min(F::splat(1.));
        let x = q.sqrt() * F::splat(crate::layout::CODE_STEPS as f32);
        let fl = x.floor();
        let i = fl.to_u32();
        let c0 = F::from_bits(ssa::load(slot::CODE, i, 2, 0));
        let c1 = F::from_bits(ssa::load(slot::CODE, i, 2, 4));
        (c0 + (c1 - c0) * (x - fl)).to_u32()
    })
}

/// The kernel of `shape`, compiled.
/// Grid: `slot::ROWS`'s padded rows a side, one side or two (16 invocations a group). Invocation g: side index s = g ≥ padded, the side `first side` XOR s (0 right of the axis, 1 left), its row r = g − s · padded, screen row y0 + r; rows past `rows` store nothing (the clip kernels; the full ones run whole groups of rows).
pub fn record(shape: Shape) -> Compiled {
    ssa::begin();
    ssa::reserve(slot::RESERVED);
    ssa::uniform_slots(64);
    let gid = ssa::gid();
    let (y0, padded) = (ssa::uniform(slot::ROWS, 0), ssa::uniform(slot::ROWS, 1));
    let s = U::from_mask(gid.ge(padded));
    let left = ssa::uniform(slot::SIDE, 0) ^ (s & U::splat(1));
    let is_left = left.eq(U::splat(1));
    let r = gid - (padded & s);
    let y = y0 + r;
    let w = U::splat(crate::layout::W as u32);
    // The texture row: (y − scroll) mod H, the scroll under H.
    let scroll = ssa::uniform(slot::SCROLL, 0);
    let h = U::splat(crate::layout::H as u32);
    let t = U::select(y.ge(scroll), y - scroll, y + h - scroll);
    let seed = load_px(slot::SEED, t);
    let mask = wuniform(slot::MASK);
    let speckle = wuniform(slot::SPECKLE);
    let base = wuniform(slot::BASE);
    let stir = select(is_left, wuniform(slot::STIR_LEFT), wuniform(slot::STIR_RIGHT));
    let steps = U::select(is_left, ssa::uniform(slot::STEPS, 1), ssa::uniform(slot::STEPS, 0));
    // A column a pass: rightward from the axis, or leftward from the column before it (one less, as an add of all ones).
    let dx = U::select(is_left, U::splat(u32::MAX), U::splat(1));
    let x_start = U::select(is_left, U::splat(AXIS as u32 - 1), U::splat(AXIS as u32));
    let row = y.wrapping_mul(w);
    let pane_row = (y + ssa::uniform(slot::SCROLL, 1)).wrapping_mul(w);
    let chrome = y.lt(U::splat(crate::layout::CHROME_ROWS as u32));
    let in_rows = r.lt(ssa::uniform(slot::SIDE, 1));
    let (x0, x1) = (ssa::uniform(slot::COLS, 0), ssa::uniform(slot::COLS, 1));
    let odt: [F; 9] = core::array::from_fn(|i| F::from_bits(ssa::uniform(slot::ODT + (i / 2) as u8, (i % 2) as u8)));
    // The present's thresholds: this row's sixteen (neon.rs `row_thresholds_at`).
    let blue_row = (y & U::splat(15)) << 4;
    let colour = and(seed, mask);
    let init = (seed.lo, seed.hi, colour.lo, colour.hi, U::splat(0), x_start);
    ssa::iterate(
        crate::layout::AXIS as u32,
        init,
        |_, (rlo, rhi, clo, chi, i, x)| {
            // fluor noise.rs `Walk::step`: the generator stirred (an add going right, a subtract going left: the add of STIR's negation), then the colour a speckle or moved by the step, the lanes wrapping at the mask.
            let rng = W { lo: rlo, hi: rhi };
            let rng = xor(rng, add(rotl13(rng), stir));
            let top = U::splat(0x00FF_FFFF);
            let speck = rng.hi.lt(top) | (rng.hi.eq(top) & rng.lo.ne(U::splat(u32::MAX)));
            let c = W { lo: clo, hi: chi };
            let stepped = and(sub(and(add(c, and(rng, wconst(crate::layout::STEP))), mask), and(shr(rng, crate::layout::DOWN), wconst(crate::layout::STEP))), mask);
            let c = select(speck, and(shr(rng, 8), speckle), stepped);
            // `Walk::pixel`: the base added (no lane carries: fluor's proof), the visible value turned to darkness, opaque.
            let noise = W { lo: (c.lo + base.lo) ^ U::splat(u32::MAX), hi: ((c.hi + base.hi) ^ U::splat(0xFFFF)) | U::splat(0xFFFF_0000) };
            let at = row + x;
            let write = || {
                let px = if shape.layers {
                    let none = W { lo: U::splat(0), hi: U::splat(0) };
                    // Front to back: the chrome first (its rows only), the pane under it, then the noise under both.
                    let acc = ssa::when(chrome, none, || load_px(slot::CHROME, at));
                    let acc = under(acc, load_px(slot::PANE, pane_row + x));
                    // fluor noise.rs `put`: an empty pixel takes the noise as it is (its store is exact; `under` into nothing would floor each darkness by one).
                    let empty = (acc.lo | acc.hi).eq(U::splat(0));
                    select(empty, noise, under(acc, noise))
                } else {
                    noise
                };
                match shape.out {
                    Out::G2t => store_px(slot::OUT, at, px),
                    Out::Baked | Out::Phases => {
                        let [cr, cg, cb] = bake(px, &odt);
                        if shape.out == Out::Baked {
                            // The baked pixel: R, G, B in its low three lanes, the fourth its transparency as bake.rs writes an opaque one's (2: Q14 1, shifted past the no-dither bit).
                            store_px(slot::OUT, at, W { lo: (cg << 16) | cr, hi: U::splat(2 << 16) | cb });
                        } else {
                            let th = ssa::load8(slot::BLUE, blue_row + (x & U::splat(15)), 0, 0);
                            for (k, turn) in [0u32, 2, 3, 1].into_iter().enumerate() {
                                let tk = th ^ U::splat(turn);
                                // The dither's add saturates at the top code (vqaddq_u16): (c + t) >> 8 less one where the sum passed 2^16, exact and branchless.
                                let code = |c: U| {
                                    let s = c + tk;
                                    (s >> 8) - (s >> 16)
                                };
                                let word = (code(cb) << 24) | (code(cg) << 14) | (code(cr) << 4) | U::splat(3);
                                ssa::store_at(slot::PHASE + k as u8, at, 2, 0, word);
                            }
                        }
                    }
                }
            };
            if shape.clip {
                let inside = in_rows & x.ge(x0) & x.lt(x1);
                ssa::when(inside, (), || write());
            } else {
                write();
            }
            let i1 = i + U::splat(1);
            ((rng.lo, rng.hi, c.lo, c.hi, i1, x + dx), i1.lt(steps))
        },
        |_, s| s,
    );
    ssa::compile()
}
