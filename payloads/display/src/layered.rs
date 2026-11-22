//! The layered show (docs/GPU-COMPOSE.md section 3.1, step 4, 2026-11-21): a window handed over as its layers rather than one flattened surface, so the background is parameters (the noise, generated where it is composed) and a pure scroll inside a pane re-renders nothing.
//!
//! The form: op 22's 50-byte form (rect, surface, level, format 2 `BAKED`, changed, mode, buffer, frame µs, ring) followed by `[n u8]` and n layers, frontmost first:
//! - a pixel layer, 29 bytes: `[0][source u64][format u8: 2][flags u8: bit 0 opaque][x u16][y u16][w u16][h u16][ring's source y u16][its height u16][origin u16][tiles u32]`: a surface of the caller's (or a mailbox buffer), `BAKED`, covering the screen rect; a ring of height 0 is none, else (as op 22's ring) its rows are the layer's own rows (source y = y, height = h) read from row y + (row − y + origin) mod h. The tiles' place is taken and not read yet (coverage, knob 10).
//! - the background, 27 bytes, last: `[1][x u16][y u16][w u16][h u16][axis u16][scroll i32][base u32, G#RRGGBB][shimmer u16][full u8][frame u32][generator u8]`: fluor's noise (`background_noise`'s parameters) under what the pixel layers leave clear, inside its clip (and inside the screen's 1-pixel edge unless `full`).
//! Op 5's feature byte (14) bit 1 says this display reads it. A show whose tail does not parse (a source not a live surface, a rect past the screen, a ring that is not its layer's own rows, more than `PIX` pixel layers, the background not last) is taken as the plain show its first 50 bytes are.
//!
//! Composed on the GPU when the GPU compose is on and the window is the only one up (gpu.rs `compose_layered`, gpu_layers.rs), else here on NEON (`row`): the pixel layers front to back in the panel's codes, fluor's own `noise_under` under them, baked by the display's bake. The two noises are different generators with the same statistics (GPU-COMPOSE.md, "Decided"): where a NEON patch meets a GPU frame the texture changes, which the owner ruled fine.

use crate::{draw, live_gen, surface_of, window_pixels, Ring, NO_RING, SURFACES};

/// The window format a layered window is kept under (`Win::format`; 0 to 2 are op 22's own).
pub const FORMAT: u8 = 3;
/// The most pixel layers a window carries (doc 3.1: chrome and content, double-buffered or not).
pub const PIX: usize = 4;
const PIX_BYTES: usize = 29;
const NOISE_BYTES: usize = 27;
/// Where the tail starts: past op 22's 50-byte form.
pub const TAIL: usize = 50;

#[derive(Clone, Copy)]
pub struct Pix {
    pub slot: usize,
    pub rect: draw::Rect,
    pub ring: Ring,
    pub opaque: bool,
}

#[derive(Clone, Copy)]
pub struct Noise {
    /// Its clip, inside the screen's 1-pixel edge unless full screen (fluor `lay`'s inset).
    pub clip: draw::Rect,
    pub axis: usize,
    /// Its scroll in texture rows, under the screen's height.
    pub scroll: usize,
    pub base: u32,
    pub shimmer: u16,
    pub frame: u32,
}

#[derive(Clone, Copy)]
pub struct Stack {
    pub pix: [Pix; PIX],
    pub n: usize,
    pub noise: Option<Noise>,
}

/// Each layered window's stack by its surface slot (as `WINS`): written by op 22 on the render thread, read by the publish's threads while it waits for them.
pub static mut STACKS: [Option<Stack>; SURFACES] = [None; SURFACES];

/// The pixel layers' sources a tail names (the receive thread's "Whose surface", main.rs `not_theirs`): each a surface the caller must hold. Read leniently: a malformed tail is refused whole later (`parse`).
pub fn sources(m: &[u8]) -> impl Iterator<Item = u64> + '_ {
    let n = if m.len() > TAIL { m[TAIL] as usize } else { 0 };
    let mut at = TAIL + 1;
    (0..n).map_while(move |_| {
        let kind = *m.get(at)?;
        let size = if kind == 0 { PIX_BYTES } else { NOISE_BYTES };
        let src = (kind == 0 && at + PIX_BYTES <= m.len()).then(|| u64::from_le_bytes(m[at + 1..at + 9].try_into().unwrap()));
        at += size;
        Some(src)
    })
    .flatten()
}

/// The tail of a show as a stack, or None (a plain show). The render thread's, at op 22.
/// WHY every check: the tail is the client's word, and the compose reads each layer's surface at the rows and columns it names (on the GPU thru a map that holds only the named surfaces read-only, so a bad one faults; on NEON thru the surfaces' window, where past the screen is past the surface).
pub fn parse(m: &[u8], width: usize, height: usize) -> Option<Stack> {
    if m.len() <= TAIL {
        return None;
    }
    let n = m[TAIL] as usize;
    let u16_at = |i: usize| u16::from_le_bytes([m[i], m[i + 1]]) as usize;
    let u32_at = |i: usize| u32::from_le_bytes([m[i], m[i + 1], m[i + 2], m[i + 3]]);
    let rect_at = |i: usize| -> Option<draw::Rect> {
        let (x, y, w, h) = (u16_at(i), u16_at(i + 2), u16_at(i + 4), u16_at(i + 6));
        (w > 0 && h > 0 && x + w <= width && y + h <= height).then(|| draw::Rect { x0: x as i32, y0: y as i32, x1: (x + w) as i32, y1: (y + h) as i32 })
    };
    let mut s = Stack { pix: [Pix { slot: 0, rect: draw::Rect::EMPTY, ring: NO_RING, opaque: false }; PIX], n: 0, noise: None };
    let mut at = TAIL + 1;
    for _ in 0..n {
        // A layer after the background: the background is the last.
        if s.noise.is_some() || at >= m.len() {
            return None;
        }
        match m[at] {
            0 if at + PIX_BYTES <= m.len() && s.n < PIX => {
                let slot = surface_of(u64::from_le_bytes(m[at + 1..at + 9].try_into().unwrap())).filter(|&k| live_gen(k) != 0)?;
                if m[at + 9] != ferros_sys::window::BAKED {
                    return None;
                }
                let rect = rect_at(at + 11)?;
                let (sy, sh, o) = (u16_at(at + 19), u16_at(at + 21), u16_at(at + 23));
                let ring = if sh == 0 {
                    NO_RING
                } else if sy == rect.y0 as usize && sh == (rect.y1 - rect.y0) as usize && o < sh {
                    Ring { x0: rect.x0 as usize, x1: rect.x1 as usize, y0: rect.y0 as usize, y1: rect.y1 as usize, o }
                } else {
                    return None;
                };
                s.pix[s.n] = Pix { slot, rect, ring, opaque: m[at + 10] & 1 != 0 };
                s.n += 1;
                at += PIX_BYTES;
            }
            1 if at + NOISE_BYTES <= m.len() => {
                let clip = rect_at(at + 1)?;
                let axis = u16_at(at + 9);
                let scroll = (u32_at(at + 11) as i32).rem_euclid(height as i32) as usize;
                let full = m[at + 21] != 0;
                // fluor `lay`: the 1-pixel edge left for the window's hairline unless full screen.
                let inset = if full { 0 } else { 1 };
                let edge = draw::Rect { x0: inset, y0: inset, x1: width as i32 - inset, y1: height as i32 - inset };
                let clip = clip.intersect(edge);
                if axis > width || clip.is_empty() {
                    return None;
                }
                s.noise = Some(Noise { clip, axis, scroll, base: u32_at(at + 15) & 0xFF_FFFF, shimmer: u16_at(at + 19) as u16, frame: u32_at(at + 22) });
                at += NOISE_BYTES;
            }
            _ => return None,
        }
    }
    Some(s)
}

/// fluor's G2t16 noise for the NEON path, a full screen of it (fluor's canvas is the whole screen, so the texture rows are the screen's); each row written only by the thread composing that row. Made by the render thread at the first layered show (`scratch_for`).
static mut NOISE_BUF: usize = 0;

/// The noise's canvas made, before any row needs it. The render thread's.
pub fn scratch_for(width: usize, height: usize) {
    unsafe {
        if NOISE_BUF == 0 {
            NOISE_BUF = alloc::vec![0u64; width * height].leak().as_mut_ptr() as usize;
        }
    }
}

/// Window `k`'s row `y`, columns `a..b`, into `out[a..b]` as baked pixels (main.rs `layer_row`'s, for a layered window): its pixel layers front to back in the panel's codes, the noise under what they leave clear inside its clip (fluor's `noise_under`, baked by the display's bake), black past that; opaque.
pub fn row(k: usize, y: usize, a: usize, b: usize, width: usize, height: usize, out: &mut [u64]) {
    let Some(s) = (unsafe { STACKS[k] }) else {
        out[a..b].fill(0);
        return;
    };
    // What of each pixel is still clear, Q14.
    let mut clear = [16384u16; 1344];
    // The channels so far, in the row itself.
    out[a..b].fill(0);
    for p in &s.pix[..s.n] {
        if (y as i32) < p.rect.y0 || (y as i32) >= p.rect.y1 {
            continue;
        }
        // WHY the max and min: the layer's columns and the run's are two ranges; this is their overlap, not a clamp (an empty one skips the layer).
        let (lo, hi) = ((p.rect.x0 as usize).max(a), (p.rect.x1 as usize).min(b));
        if lo >= hi {
            continue;
        }
        let g = p.ring;
        let sy = if g.y1 != 0 && y >= g.y0 && y < g.y1 {
            let (r, h) = (y - g.y0 + g.o, g.y1 - g.y0);
            g.y0 + if r >= h { r - h } else { r }
        } else {
            y
        };
        let src = &window_pixels(p.slot, width, height)[sy * width..(sy + 1) * width];
        for x in lo..hi {
            let c = clear[x - a] as u64;
            let v = src[x];
            let see = v >> 63 != 0;
            let lt = if p.opaque { 0 } else if see { 16384 } else { v >> 49 & 0x3FFF };
            let chans = if see { 0 } else { v & ((1 << 48) - 1) };
            let add = |sh: u32| ((chans >> sh & 0xFFFF) * c >> 14) << sh;
            out[x] += add(0) | add(16) | add(32);
            clear[x - a] = (c * lt >> 14) as u16;
        }
    }
    if let Some(nz) = s.noise {
        let c = nz.clip;
        let (lo, hi) = ((c.x0 as usize).max(a), (c.x1 as usize).min(b));
        if (y as i32) >= c.y0 && (y as i32) < c.y1 && lo < hi && clear[lo - a..hi - a].iter().any(|&t| t != 0) {
            // SAFETY of the sharing: this thread writes only row y of the canvas (the clip below is that row), and each row is composed by one thread (main.rs `publish_app_mask`'s stripes); made at the first layered show (`scratch_for`).
            let all = unsafe { core::slice::from_raw_parts_mut(NOISE_BUF as *mut u64, width * height) };
            all[y * width + lo..y * width + hi].fill(0);
            let mut d = fluor::canvas::Damage::new();
            let clip = Some(fluor::paint::Clip::new(lo, y, hi, y + 1));
            let base = Some(nz.base);
            if nz.shimmer != 0 && nz.axis == width / 2 {
                fluor::paint::background_noise(&mut fluor::canvas::Canvas::new(all, width, height, &mut d), nz.shimmer as usize, true, nz.scroll as isize, clip, base);
            } else {
                fluor::paint::noise_under(&mut fluor::canvas::Canvas::new(all, width, height, &mut d), nz.axis, nz.scroll as isize, true, clip, base);
            }
            let seg = &mut all[y * width + lo..y * width + hi];
            crate::bake::now().g2t_in_place(seg);
            for (x, &p) in (lo..hi).zip(seg.iter()) {
                let t = clear[x - a] as u64;
                let add = |sh: u32| ((p >> sh & 0xFFFF) * t >> 14) << sh;
                out[x] += add(0) | add(16) | add(32);
            }
        }
    }
    // Opaque as a baked opaque pixel is marked (bake.rs: the transparency's lowest step).
    for o in &mut out[a..b] {
        *o |= 2 << 48;
    }
}
