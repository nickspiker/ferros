//! The kernel's text: Oxanium Regular with the dozenal digit glyphs, anti-aliased and proportional, from an atlas rasterised at build time (build.rs `text_atlas`) — what the boot screens draw with, so they read in the same face as the rest of the system, with no rasteriser in the boot path.
//!
//! A line of text is composed front to back into a box of an XRGB8888 surface: the glyphs first, each one's coverage a layer of the text's colour, then the line's ground under them, so every pixel of the box is written once and nothing already on the surface is read; tatau's dozenal digits (U+0010-U+001B), radix point and arrows are in the atlas like any other character.

static ATLAS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/atlas.bin"));

fn u16at(o: usize) -> u16 {
    u16::from_le_bytes([ATLAS[o], ATLAS[o + 1]])
}
fn u32at(o: usize) -> u32 {
    u32::from_le_bytes([ATLAS[o], ATLAS[o + 1], ATLAS[o + 2], ATLAS[o + 3]])
}

const ENTRY: usize = 18;
const HEAD: usize = 10;

/// Pixels from a line's top to the baseline.
pub fn ascent() -> i32 {
    u16at(4) as i32
}

struct Glyph {
    advance: i32,
    xmin: i32,
    ymin: i32,
    w: usize,
    h: usize,
    off: usize,
}

fn glyph(c: char) -> Option<Glyph> {
    let n = u16at(8) as usize;
    let data = HEAD + n * ENTRY;
    (0..n).map(|i| HEAD + i * ENTRY).find(|&e| u32at(e) == c as u32).map(|e| Glyph {
        advance: u16at(e + 4) as i32,
        xmin: u16at(e + 6) as i16 as i32,
        ymin: u16at(e + 8) as i16 as i32,
        w: u16at(e + 10) as usize,
        h: u16at(e + 12) as usize,
        off: data + u32at(e + 14) as usize,
    })
}

/// How wide `s` draws, in pixels.
pub fn width(s: &str) -> i32 {
    s.chars().map(|c| glyph(c).or_else(|| glyph('?')).map(|g| g.advance).unwrap_or(0)).sum()
}

/// A surface: an XRGB8888 framebuffer, its width and height in pixels, its row pitch in pixels.
pub struct Surface {
    pub fb: *mut u32,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

/// Bits per field of the accumulator: fluor's G2t16, α + darkness, 16 bits each (α in the top field, then r, g, b).
/// At 16 bits the kernel's `>> D` rounding stays far below one 8-bit code, so the 8-bit result matches exact arithmetic (an opaque black ground at 8 bits would come out as code 1, not 0).
const D: u32 = 16;
const ONE: u64 = (1 << D) - 1;

/// fluor's `Blend::under` in Normal mode at 16 bits, the one compositing kernel (photon AGENT.md "Compositing"): `bottom` (α + straight darkness) composed under the accumulator `top` (α + premultiplied darkness).
fn under(top: u64, bottom: u64) -> u64 {
    if top >= ONE << (3 * D) {
        return top;
    }
    let top_a = top >> (3 * D);
    let consumed = ((ONE + 1 - top_a) * (bottom >> (3 * D))) >> D;
    let ch = |s: u32| (((top >> s) & ONE) + ((((bottom >> s) & ONE) * consumed) >> D)) << s;
    ((top_a + consumed) << (3 * D)) | ch(2 * D) | ch(D) | ch(0)
}

/// An XRGB8888 colour as a 16-bit layer at opacity `a16`: each channel's darkness is how far it sits below white, widened by 257 so 255 is ONE.
fn layer(rgb: u32, a16: u64) -> u64 {
    let dark = |s: u32| (0xFF - ((rgb >> s) as u64 & 0xFF)) * 257;
    (a16 << (3 * D)) | (dark(16) << (2 * D)) | (dark(8) << D) | dark(0)
}

impl Surface {
    /// Compose a line of text front to back into the box at (`x`, `top`), `w` by `h` pixels: the strings `runs` in `rgb`, one after the other from the pen at `x`, go into an empty accumulator first (each glyph pixel a layer of `rgb` at its coverage), then the opaque `ground` under them, and every pixel of the box is written once; returns where the pen ends.
    /// The box must lie inside the surface: a box outside it is the caller's bug and panics.
    pub fn line(&self, x: i32, top: i32, w: usize, h: usize, runs: &[&str], rgb: u32, ground: u32) -> i32 {
        assert!(x >= 0 && top >= 0 && x as usize + w <= self.width && top as usize + h <= self.height, "text box outside the surface");
        // The glyphs and where each one's bitmap starts in the box (left column, top row), resolved once for every row.
        let mut glyphs = alloc::vec::Vec::new();
        let mut pen = x;
        for s in runs {
            for c in s.chars() {
                let Some(g) = glyph(c).or_else(|| glyph('?')) else {
                    continue;
                };
                let (gx0, gy0) = (pen + g.xmin - x, ascent() - g.h as i32 - g.ymin);
                pen += g.advance;
                glyphs.push((gx0, gy0, g));
            }
        }
        let ground = layer(ground, ONE);
        let mut acc = alloc::vec![0u64; w];
        for by in 0..h {
            acc.fill(0);
            // The text first: each glyph's coverage on this row, a layer of `rgb`, under what the glyphs before it left.
            for (gx0, gy0, g) in &glyphs {
                let gy = by as i32 - gy0;
                // WHY: a glyph's rows come from the font's metrics, not from the box, so a row of the box can fall outside a glyph (above its top, below its foot).
                // PROOF: without it `g.off + gy * g.w` would read another glyph's coverage, or past the atlas's end for the last glyph.
                if gy < 0 || gy as usize >= g.h {
                    continue;
                }
                let row = g.off + gy as usize * g.w;
                for gx in 0..g.w {
                    let bx = (gx0 + gx as i32) as usize;
                    // WHY: a glyph can start left of the box (a negative xmin on the first letter) or run past its right edge (a label longer than the box); the cast makes a negative column huge.
                    // PROOF: without it `acc[bx]` panics on the label that runs long, or is written outside the box.
                    if bx < w {
                        acc[bx] = under(acc[bx], layer(rgb, ATLAS[row + gx] as u64 * 257));
                    }
                }
            }
            // Then the ground under the whole row, and the row onto the surface: light over the ground is α less darkness, back to 8 bits.
            let p = unsafe { self.fb.add((top as usize + by) * self.stride + x as usize) };
            for (bx, &a) in acc.iter().enumerate() {
                let px = under(a, ground);
                let light = |s: u32| (((px >> (3 * D)) - ((px >> s) & ONE)) >> 8) << (s / 2);
                // SAFETY: the box lies inside the surface (asserted above), so (top + by, x + bx) is a pixel of the framebuffer.
                unsafe { core::ptr::write_volatile(p.add(bx), (light(2 * D) | light(D) | light(0)) as u32) };
            }
        }
        pen
    }
}
