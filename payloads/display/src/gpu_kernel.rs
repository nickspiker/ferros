//! The compose kernel (docs/GPU-COMPOSE.md 1.4, step 2), recorded thru valhall::ssa on the host (build.rs) and shipped as words in the display's image, as compbench's and ferros_clip's are: the device compiles nothing.
//!
//! One invocation a pixel of the owed rect, 16 neighbouring pixels a warp (a 16-pixel block of a row), the blocks row after row. Each pixel, front to back:
//! 0. The front layer (the glass's edge, the hole's knockout, the bars), made on the CPU (gpu.rs `front_row`, the same arithmetic as main.rs `glass_front` and `bars_front` up to the scene): the channels so far and what of the scene still shows, Q14.
//! 1. The scene: the window covering the pixel (step 2's windows never overlap, gpu.rs `eligible`), read thru its ring as main.rs `src_row` reads it; a see-through pixel of a window that is not opaque is black, as `clear_mark` leaves it; no window, black.
//! 2. The scene under the front (`under_pm` with the scene opaque: each channel the front's plus the scene's by what still shows), integer, so the words equal NEON's.
//! 3. The present (neon.rs `present_app16`): each code plus its threshold (BLUE ^ the phase's turn, 0 for a pixel with the no-dither bit), saturating, its high byte, the channels' masks, packed B[31:22] G[21:12] R[11:2] A[1:0].
//!
//! No Porter-Duff over: `under` only, front to back (AGENT.md "Compositing"). Each phase word is written once by one job (feedback_painters_needs_proof).

use crate::gpu_layout::{slot, WINDOWS};
use valhall::ssa::{self, Compiled, F, M, U};

/// The kernel, compiled.
pub fn record() -> Compiled {
    ssa::begin();
    ssa::reserve(slot::RESERVED);
    ssa::uniform_slots(64);
    let gid = ssa::gid();
    let low = U::splat(0xFFFF);
    // Where this invocation is: block b of the grid, lane i in it; the block's row by a multiply with the reciprocal of the blocks a row (exact: (b + 1/2) / n is at least 1/(2n) from an integer, far above f32's error at these sizes), its column by the remainder.
    let b = gid >> 4;
    let blocks = ssa::uniform(slot::GRID, 0);
    let inv = F::from_bits(ssa::uniform(slot::GRID, 1));
    let row = ((F::from_u32(b) + F::splat(0.5)) * inv).floor().to_u32();
    let col = b - row.wrapping_mul(blocks);
    let (r0, r1) = (ssa::uniform(slot::RECT, 0), ssa::uniform(slot::RECT, 1));
    let (x0, y0, x1) = (r0 & low, r0 >> 16, r1 & low);
    let x = x0 + (col << 4) + (gid & U::splat(15));
    let y = y0 + row;
    let w = ssa::uniform(slot::SCREEN, 0);
    let at = y.wrapping_mul(w) + x;
    ssa::when(x.lt(x1), (), || {
        // 0. The front.
        let (flo, fhi) = (ssa::load(slot::FRONT, at, 3, 0), ssa::load(slot::FRONT, at, 3, 4));
        // 1. The scene.
        let zero = U::splat(0);
        let mut p = (zero, zero);
        for k in 0..WINDOWS as u8 {
            let s = slot::WIN + 4 * k;
            let (a, c) = (ssa::uniform(s + 1, 0), ssa::uniform(s + 1, 1));
            let inside: M = x.ge(a & low) & y.ge(a >> 16) & x.lt(c & low) & y.lt(c >> 16);
            p = ssa::when(inside, p, || {
                let (ra, rc) = (ssa::uniform(s + 2, 0), ssa::uniform(s + 2, 1));
                let (o, flags) = (ssa::uniform(s + 3, 0), ssa::uniform(s + 3, 1));
                let (rx0, rx1, ry0, ry1) = (ra & low, ra >> 16, rc & low, rc >> 16);
                let ringed = x.ge(rx0) & x.lt(rx1) & y.ge(ry0) & y.lt(ry1);
                // `src_row`: (y − y0 + o) mod H, the sum under 2H.
                let (r, h) = (y - ry0 + o, ry1 - ry0);
                let sy = U::select(ringed, ry0 + U::select(r.ge(h), r - h, r), y);
                let i = sy.wrapping_mul(w) + x;
                let (lo, hi) = (ssa::load(s, i, 3, 0), ssa::load(s, i, 3, 4));
                // A see-through pixel of a window not opaque: nothing (main.rs `clear_mark`); an opaque window's pixel as it is.
                let keep = U::from_mask((hi >> 31).eq(zero) | (flags & U::splat(1)).ne(zero));
                (lo & keep, hi & keep)
            });
        }
        // 2. Under the front: `PASS` sends the scene out whole, its own no-dither bit kept; else each channel the front's plus the scene's by what shows (Q14), the front's no-dither bit.
        let t = fhi >> 17;
        let pass = t.eq(U::splat(0x7FFF));
        let ch = |f: U, s: U| U::select(pass, s, f + (s.wrapping_mul(t) >> 14));
        let cr = ch(flo & low, p.0 & low);
        let cg = ch(flo >> 16, p.0 >> 16);
        let cb = ch(fhi & low, p.1 & low);
        let flag = U::select(pass, p.1 >> 16, fhi >> 16) & U::splat(1);
        // 3. The present.
        let sc = ssa::uniform(slot::SCREEN, 1);
        let th = (ssa::load8(slot::BLUE, ((y & U::splat(15)) << 4) | (x & U::splat(15)), 0, 0) ^ (sc & U::splat(3))) & (flag - U::splat(1));
        let code = |c: U, m: U| {
            let s = c + th;
            ((s >> 8) - (s >> 16)) & m
        };
        let (mr, mg, mb) = ((sc >> 8) & U::splat(0xFF), (sc >> 16) & U::splat(0xFF), sc >> 24);
        let word = (code(cb, mb) << 24) | (code(cg, mg) << 14) | (code(cr, mr) << 4) | U::splat(3);
        ssa::store_at(slot::PHASE, at, 2, 0, word);
    });
    ssa::compile()
}
