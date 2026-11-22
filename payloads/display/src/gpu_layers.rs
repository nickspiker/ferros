//! The layered window's compose kernel (docs/GPU-COMPOSE.md 1.4 and 3.1, step 4), recorded on the host (build.rs) as the step 2 kernel is: a window's pixel layers front to back, the background noise generated under what is still clear, the front layer (the glass and the bars) in front of it all, presented into one phase.
//!
//! One workgroup of 2^`WG_LOG2` invocations a half-row a side of the noise's axis (compbench's `record_par`, the noise's parallel form: payloads/compbench/src/noise.rs, the Photon session's generator): each invocation hashes a run of `PX` pixels from the axis outward, the runs' sums joined by a Hillis-Steele scan in workgroup storage, each pixel's noise staged there as an opaque G2t16; then the half-row along the row, neighbouring invocations on neighbouring pixels, each pixel:
//! 0. the window's pixel layers front to back (`BAKED`, premultiplied, transparency Q14 in bits 49-62, bit 63 see-through): each layer's channels by what is still clear, the clear the product (`under_pm` in the panel's codes, as the doc's 1.4 has it; a partly covered pixel over a layer is mixed in codes, not light: the step-2 frames' `partial_run` does it in light, a difference to look at by eye);
//! 1. the noise under what is still clear, inside its clip, baked thru the display's own tables (bake.rs `Bake::light`, `Bake::code`, the ODT), as compbench's `bake`; what is still clear after it is black;
//! 2. the front layer and the present, as the step 2 kernel (gpu_kernel.rs).
//! No Porter-Duff over: `under` only, front to back; each phase word written once by one job.

use crate::gpu_layout::lslot as slot;
use crate::noise as nz;
use valhall::ssa::{self, Compiled, F, H, M, U};

/// The workgroup, log 2: compbench's measured best for the noise (GPU-COMPOSE.md knob 1).
pub const WG_LOG2: u32 = 7;
/// Pixels each invocation hashes: a half-row of up to 2^7 · 6 = 768 pixels (the Pro's 672, the 8a's 540).
pub const PX: u32 = 6;
/// The workgroup storage: the scan's 8 bytes an invocation, then the half-row's noise, 8 bytes a pixel.
pub const SHARED: u32 = 8 * (1 << WG_LOG2) + 8 * PX * (1 << WG_LOG2);
/// The pixel layers a window carries (doc 3.1: chrome and content, double-buffered or not).
pub const LAYERS: usize = 4;

fn lb(x: U) -> U {
    let x = x ^ (x >> 16);
    let x = x.wrapping_mul(U::splat(nz::M0));
    let x = x ^ (x >> 15);
    let x = x.wrapping_mul(U::splat(nz::M1));
    x ^ (x >> 16)
}

/// The scan's element (compbench kernel.rs `E`): R << 16 | G, and B with the segment flag in bit 31.
#[derive(Clone, Copy)]
struct E {
    rg: U,
    bf: U,
}
const FLAG: u32 = 1 << 31;
fn combine(x: E, y: E, mrg: U, mb: U) -> E {
    let starts = (y.bf & U::splat(FLAG)).ne(U::splat(0));
    let rg = U::select(starts, y.rg, (x.rg + y.rg) & mrg);
    let bf = U::select(starts, y.bf, ((x.bf + y.bf) & mb) | (x.bf & U::splat(FLAG)));
    E { rg, bf }
}

/// An opaque G2t16 (hi = α << 16 | R darkness, lo = G << 16 | B) to the panel's Q16 codes: compbench's `bake` (the display's bake.rs in scalar f32).
fn bake(lo: U, hi: U, odt: &[F; 9]) -> [U; 3] {
    let field = U::splat(0xFFFF);
    let a = hi >> 16;
    let light = |d: U| H::from_bits(ssa::load16(slot::LIGHT, a - d, 1, 0)).lo_f();
    let l = [light(hi & field), light(lo >> 16), light(lo & field)];
    core::array::from_fn(|r| {
        let p = odt[3 * r] * l[0] + odt[3 * r + 1] * l[1] + odt[3 * r + 2] * l[2];
        // WHY the clamp (compbench's own, free on the add): the code table's index is √p · 2^12 and its second read one past it, so p must lie in [0, 1] for both reads to stay in the table's 2^12 + 2 entries.
        let q = p.max(F::splat(0.)).min(F::splat(1.));
        let x = q.sqrt() * F::splat((1 << 12) as f32);
        let fl = x.floor();
        let i = fl.to_u32();
        let c0 = F::from_bits(ssa::load(slot::CODE, i, 2, 0));
        let c1 = F::from_bits(ssa::load(slot::CODE, i, 2, 4));
        (c0 + (c1 - c0) * (x - fl)).to_u32()
    })
}

/// The kernel, compiled. Grid: `slot::SIDE`'s rows a side, one side or two, a workgroup a row a side (the first `rows` workgroups the first side).
pub fn record() -> Compiled {
    let wg = 1u32 << WG_LOG2;
    let scan_bytes = 8 * wg;
    ssa::begin();
    ssa::reserve(slot::RESERVED);
    ssa::uniform_slots(64);
    ssa::workgroup(WG_LOG2, SHARED.next_power_of_two());
    let g = ssa::group_id();
    let i = ssa::local_id();
    let zero = U::splat(0);
    let low = U::splat(0xFFFF);
    let (first_left, rows) = (ssa::uniform(slot::SIDE, 0), ssa::uniform(slot::SIDE, 1));
    let s = U::from_mask(g.ge(rows));
    let left = first_left ^ (s & U::splat(1));
    let is_left = left.eq(U::splat(1));
    let r0 = ssa::uniform(slot::RECT, 0);
    let y = (r0 >> 16) + g - (rows & s);
    let (axis, scroll) = (ssa::uniform(slot::AXIS, 0), ssa::uniform(slot::AXIS, 1));
    let (texh, frame) = (ssa::uniform(slot::TEX, 0), ssa::uniform(slot::TEX, 1));
    // The texture row: (y − scroll) mod its height, the scroll under it.
    let t = U::select(y.ge(scroll), y - scroll, y + texh - scroll);
    let (mlo, mhi) = (ssa::uniform(slot::MASK, 0), ssa::uniform(slot::MASK, 1));
    let (slo, shi) = (ssa::uniform(slot::SPECKLE, 0), ssa::uniform(slot::SPECKLE, 1));
    let (blo, bhi) = (ssa::uniform(slot::BASE, 0), ssa::uniform(slot::BASE, 1));
    let mask = [mhi & low, mlo >> 16, mlo & low];
    let spk_mask = [shi & low, slo >> 16, slo & low];
    let base = [bhi & low, blo >> 16, blo & low];
    // fluor's shimmer: added to each row's starting colour, in the blue lane (noise.rs `background_noise`).
    let shimmer = [zero, zero, ssa::uniform(slot::SHIMMER, 0)];
    let mrg = (mask[0] << 16) | mask[1];
    let mb = mask[2];
    let n = U::select(is_left, ssa::uniform(slot::STEPS, 1), ssa::uniform(slot::STEPS, 0));
    let rk = (t << (nz::K_BITS + 1)) | (left << nz::K_BITS);
    let srow = lb(rk ^ U::splat(nz::SC) ^ frame.wrapping_mul(U::splat(nz::GOLD)));
    let draw = U::splat(nz::DRAW);
    let grain = U::splat(nz::GRAIN);
    let start = |a: U, b: U| [a, b, (a ^ b) >> 12];
    let delta = |a: U, b: U| [(a & draw) - ((a >> 9) & draw), ((a >> 18) & draw) - (b & draw), ((b >> 9) & draw) - ((b >> 18) & draw)];
    // Pass one: this invocation's run, hashed and summed from nothing.
    let k0 = i.wrapping_mul(U::splat(PX));
    let mut ab: [(U, U); PX as usize] = [(zero, zero); PX as usize];
    let mut acc = [zero; 3];
    let mut flag = M::splat(false);
    for (q, e) in ab.iter_mut().enumerate() {
        let k = k0 + U::splat(q as u32);
        let key = rk | k;
        let (a, b) = (lb(key ^ U::splat(nz::SA)), lb(key ^ U::splat(nz::SB)));
        let spk = ((a >> 27) | (b >> 29)).eq(zero);
        let valid = k.lt(n);
        let (st, d) = (start(a, b), delta(a, b));
        for c in 0..3 {
            let v = U::select(spk, st[c] & mask[c], (acc[c] + d[c]) & mask[c]);
            acc[c] = U::select(valid, v, acc[c]);
        }
        flag = flag | (spk & valid);
        *e = (a, b);
    }
    let mut e = E { rg: (acc[0] << 16) | acc[1], bf: acc[2] | (U::from_mask(flag) & U::splat(FLAG)) };
    ssa::shared_store(i, 3, 0, e.rg);
    ssa::shared_store(i, 3, 4, e.bf);
    for step in 0..WG_LOG2 {
        let off = 1u32 << step;
        ssa::barrier();
        let has = i.ge(U::splat(off));
        let from = U::select(has, i - U::splat(off), i);
        let x = E { rg: ssa::shared_load(from, 3, 0), bf: ssa::shared_load(from, 3, 4) };
        let joined = combine(x, e, mrg, mb);
        e = E { rg: U::select(has, joined.rg, e.rg), bf: U::select(has, joined.bf, e.bf) };
        ssa::barrier();
        ssa::shared_store(i, 3, 0, e.rg);
        ssa::shared_store(i, 3, 4, e.bf);
    }
    ssa::barrier();
    let rk0 = (t << (nz::K_BITS + 1)) | U::splat(nz::K_START);
    let (sa, sb) = (lb(rk0 ^ U::splat(nz::SA)), lb(rk0 ^ U::splat(nz::SB)));
    let st0 = start(sa, sb);
    let first = i.eq(zero);
    let prev = U::select(first, zero, i - U::splat(1));
    let before = E { rg: U::select(first, zero, ssa::shared_load(prev, 3, 0)), bf: U::select(first, zero, ssa::shared_load(prev, 3, 4)) };
    let restart = (before.bf & U::splat(FLAG)).ne(zero);
    let pre = [before.rg >> 16, before.rg & low, before.bf & low];
    let mut c: [U; 3] = core::array::from_fn(|ch| U::select(restart, pre[ch], (st0[ch] + shimmer[ch] + pre[ch]) & mask[ch]));
    // Pass two: each pixel's noise (G2t16, opaque) into the storage after the scan's.
    for (q, &(a, b)) in ab.iter().enumerate() {
        let spk = ((a >> 27) | (b >> 29)).eq(zero);
        let (st, d) = (start(a, b), delta(a, b));
        let mut tw = (a ^ b ^ srow).wrapping_mul(U::splat(nz::GOLD));
        tw = tw ^ (tw >> 15);
        let twk = [((tw >> 23) << 7) | (a & grain), (((tw >> 14) & draw) << 7) | (b & grain), (((tw >> 5) & draw) << 7) | ((a >> 7) & grain)];
        let mut v = [zero; 3];
        for ch in 0..3 {
            c[ch] = U::select(spk, st[ch] & mask[ch], (c[ch] + d[ch]) & mask[ch]);
            v[ch] = U::select(spk, twk[ch] & spk_mask[ch], c[ch]) + base[ch];
        }
        let k = k0 + U::splat(q as u32);
        ssa::shared_store(k, 3, scan_bytes as i16, ((v[1] << 16) | v[2]) ^ U::splat(u32::MAX));
        ssa::shared_store(k, 3, scan_bytes as i16 + 4, v[0] ^ U::splat(u32::MAX));
    }
    ssa::barrier();
    // Out along the row.
    let w = ssa::uniform(slot::SCREEN, 0);
    let r1 = ssa::uniform(slot::RECT, 1);
    let (x0, x1) = (r0 & low, r1 & low);
    let (c0, c1) = (ssa::uniform(slot::CLIP, 0), ssa::uniform(slot::CLIP, 1));
    let odt: [F; 9] = core::array::from_fn(|k| F::from_bits(ssa::uniform(slot::ODT + (k / 2) as u8, (k % 2) as u8)));
    let sc = ssa::uniform(slot::SCREEN, 1);
    for q in 0..PX {
        let j = U::splat(q * wg) + i;
        let x = U::select(is_left, axis - U::splat(1) - j, axis + j);
        let inside = j.lt(n) & x.ge(x0) & x.lt(x1);
        ssa::when(inside, (), || {
            let at = y.wrapping_mul(w) + x;
            // 0. The pixel layers, front to back, in codes.
            let mut acc = [zero; 3];
            let mut clear = U::splat(16384);
            for l in 0..LAYERS as u8 {
                let s = slot::LAYER + 4 * l;
                let (a, cc) = (ssa::uniform(s + 1, 0), ssa::uniform(s + 1, 1));
                let hit: M = x.ge(a & low) & y.ge(a >> 16) & x.lt(cc & low) & y.lt(cc >> 16) & clear.ne(zero);
                let (acc2, clear2) = ssa::when(hit, (acc, clear), || {
                    let (ra, rc) = (ssa::uniform(s + 2, 0), ssa::uniform(s + 2, 1));
                    let (o, flags) = (ssa::uniform(s + 3, 0), ssa::uniform(s + 3, 1));
                    let (rx0, rx1, ry0, ry1) = (ra & low, ra >> 16, rc & low, rc >> 16);
                    let ringed = x.ge(rx0) & x.lt(rx1) & y.ge(ry0) & y.lt(ry1);
                    let (r, h) = (y - ry0 + o, ry1 - ry0);
                    let sy = U::select(ringed, ry0 + U::select(r.ge(h), r - h, r), y);
                    let ix = sy.wrapping_mul(w) + x;
                    let (lo, hi) = (ssa::load(s, ix, 3, 0), ssa::load(s, ix, 3, 4));
                    // `trans`: see-through (bit 63) is all clear; else bits 49-62; an opaque layer (flags bit 0) none.
                    let see = (hi >> 31).ne(zero);
                    let lt = U::select(see, U::splat(16384), (hi >> 17) & U::splat(0x3FFF)) & U::from_mask((flags & U::splat(1)).eq(zero));
                    let chans = [lo & low, lo >> 16, hi & low];
                    let mut out = acc;
                    for ch in 0..3 {
                        // A see-through pixel's channels add nothing, whatever its client left under the bit.
                        let v = U::select(see, zero, chans[ch]);
                        out[ch] = acc[ch] + (v.wrapping_mul(clear) >> 14);
                    }
                    (out, (clear.wrapping_mul(lt)) >> 14)
                });
                acc = acc2;
                clear = clear2;
            }
            // 1. The noise under what is still clear, inside its clip.
            let in_clip = x.ge(c0 & low) & y.ge(c0 >> 16) & x.lt(c1 & low) & y.lt(c1 >> 16) & clear.ne(zero);
            let acc = ssa::when(in_clip, acc, || {
                let (nlo, nhi) = (ssa::shared_load(j, 3, scan_bytes as i16), ssa::shared_load(j, 3, scan_bytes as i16 + 4));
                let nz = bake(nlo, nhi, &odt);
                let mut out = acc;
                for ch in 0..3 {
                    out[ch] = acc[ch] + (nz[ch].wrapping_mul(clear) >> 14);
                }
                out
            });
            // 2. The front and the present (gpu_kernel.rs).
            let (flo, fhi) = (ssa::load(slot::FRONT, at, 3, 0), ssa::load(slot::FRONT, at, 3, 4));
            let ft = fhi >> 17;
            let pass = ft.eq(U::splat(0x7FFF));
            let under = |f: U, s: U| U::select(pass, s, f + (s.wrapping_mul(ft) >> 14));
            let cr = under(flo & low, acc[0]);
            let cg = under(flo >> 16, acc[1]);
            let cb = under(fhi & low, acc[2]);
            let flag = U::select(pass, zero, fhi >> 16) & U::splat(1);
            let th = (ssa::load8(slot::BLUE, ((y & U::splat(15)) << 4) | (x & U::splat(15)), 0, 0) ^ (sc & U::splat(3))) & (flag - U::splat(1));
            let code = |c: U, m: U| {
                let s = c + th;
                ((s >> 8) - (s >> 16)) & m
            };
            let (mr, mg, mb) = ((sc >> 8) & U::splat(0xFF), (sc >> 16) & U::splat(0xFF), sc >> 24);
            let word = (code(cb, mb) << 24) | (code(cg, mg) << 14) | (code(cr, mr) << 4) | U::splat(3);
            ssa::store_at(slot::PHASE, at, 2, 0, word);
        });
    }
    ssa::compile()
}
