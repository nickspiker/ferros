//! The kernel's text: Oxanium Regular with the dozenal digit glyphs, anti-aliased and proportional, from an atlas rasterised at build time (build.rs `text_atlas`) — what the boot screens draw with, so they read in the same face as the rest of the system, with no rasteriser in the boot path.
//!
//! Text is blended into an XRGB8888 surface over whatever is there, each glyph's coverage scaling the colour; tatau's dozenal digits (U+0010-U+001B), radix point and arrows are in the atlas like any other character.

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

impl Surface {
    /// Fill a rectangle with one colour.
    pub fn fill(&self, x: usize, y: usize, w: usize, h: usize, rgb: u32) {
        for yy in y..(y + h).min(self.height) {
            for xx in x..(x + w).min(self.width) {
                unsafe { core::ptr::write_volatile(self.fb.add(yy * self.stride + xx), rgb) };
            }
        }
    }

    /// Draw `s` with its line's top at `top` and its pen starting at `x`, in `rgb` blended over what is there; returns where the pen ends.
    pub fn draw(&self, x: i32, top: i32, s: &str, rgb: u32) -> i32 {
        let base = top + ascent();
        let mut pen = x;
        for c in s.chars() {
            let Some(g) = glyph(c).or_else(|| glyph('?')) else {
                continue;
            };
            let x0 = pen + g.xmin;
            let y0 = base - g.h as i32 - g.ymin;
            for gy in 0..g.h {
                let y = y0 + gy as i32;
                if y < 0 || y as usize >= self.height {
                    continue;
                }
                for gx in 0..g.w {
                    let x = x0 + gx as i32;
                    let a = ATLAS[g.off + gy * g.w + gx] as u32;
                    if a == 0 || x < 0 || x as usize >= self.width {
                        continue;
                    }
                    let p = unsafe { self.fb.add(y as usize * self.stride + x as usize) };
                    let under = unsafe { core::ptr::read_volatile(p) };
                    let mix = |sh: u32| (((rgb >> sh) & 0xFF) * a + ((under >> sh) & 0xFF) * (255 - a)) / 255 << sh;
                    unsafe { core::ptr::write_volatile(p, mix(16) | mix(8) | mix(0)) };
                }
            }
            pen += g.advance;
        }
        pen
    }
}
