//! Text: swash (no_std, its `libm` feature) scaling the typeface's outlines in native f32, the rotation applied to the outline, and the outline rasterised here at 16 bits (`raster`: exact area coverage, Q16; swash's own rasteriser stops at 8 bits) — antialiased at any angle, not a turned bitmap (owner, 2026-10-23: f32 and other native; text upright by the real angle when gravity is in). Coverage is kept in a glyph cache (a fixed arena, cleared whole when it fills), keyed by glyph, size (quarter pixels), turn (Q8 cos and sin) and the pen's quarter-pixel phase, so static text costs a lookup and a blit.
//!
//! Composition is front to back like every other scene pass: each coverage sample goes under what is there, premultiplied, so text drawn first stays on top and anything drawn after lands only where the glyph leaves room.

use swash::scale::outline::Outline;
use swash::scale::ScaleContext;
use swash::zeno::{Command, PathData, Point, Transform};
use swash::FontRef;

fn floor(v: f32) -> f32 {
    let t = v as i64 as f32;
    if t > v { t - 1.0 } else { t }
}

const SLOTS: usize = 4096;
const ARENA: usize = 2 << 20;

#[derive(Clone, Copy)]
struct Entry {
    key: u64,
    off: u32,
    left: i16,
    top: i16,
    w: u16,
    h: u16,
}
const EMPTY: Entry = Entry { key: 0, off: 0, left: 0, top: 0, w: 0, h: 0 };

pub struct Glyphs {
    font: FontRef<'static>,
    table: &'static mut [Entry],
    arena: &'static mut [u16],
    used: usize,
    pub misses: u64,
}

impl Glyphs {
    /// From the typeface's bytes; the table and arena are allocated now (before the heap's mark) and kept for the process.
    pub fn new(ttf: &'static [u8]) -> Option<Glyphs> {
        Self::with_arena(ttf, ARENA)
    }

    /// The same with a cache of `arena` samples (the console's worker keeps a small one: its text is a few sizes of a few words).
    pub fn with_arena(ttf: &'static [u8], arena: usize) -> Option<Glyphs> {
        let font = FontRef::from_index(ttf, 0)?;
        Some(Glyphs { font, table: alloc::vec![EMPTY; SLOTS].leak(), arena: alloc::vec![0u16; arena].leak(), used: 0, misses: 0 })
    }

    /// The width of `s` at `px` pixels, along its own baseline.
    pub fn width(&self, s: &str, px: f32) -> f32 {
        let (cm, gm) = (self.font.charmap(), self.font.glyph_metrics(&[]).scale(px));
        s.chars().map(|c| gm.advance_width(cm.map(c))).sum()
    }

    /// Draw `s` at `px` pixels with its baseline starting at (x, y), turned by (c, s) on the screen (clockwise for positive s, y down; (1, 0) upright), in `colour` (G#RRGGBB) under what is there. Returns the advance along the baseline.
    pub fn draw(&mut self, px_buf: &mut [crate::draw::Px], width: usize, height: usize, text: &str, x: f32, y: f32, px: f32, turn: (f32, f32), colour: u32) -> f32 {
        let (bx0, by0, bx1, by1) = crate::draw::bounds(width, height);
        let solid = crate::draw::opaque(colour & 0x00FF_FFFF);
        self.cover(text, x, y, px, turn, |sx, sy, a| {
            if sx >= bx0 && sy >= by0 && sx < bx1 && sy < by1 {
                let p = &mut px_buf[sy as usize * width + sx as usize];
                *p = crate::draw::under(*p, crate::draw::cover(solid, a));
            }
        })
    }

    /// The coverage of `s` (Q16 per pixel, 0 to 65535, screen coordinates) handed to `put`, for any surface: the canvas (`draw`), the console, or an app's 16-bit surface. Returns the advance.
    pub fn cover(&mut self, text: &str, x: f32, y: f32, px: f32, turn: (f32, f32), mut put: impl FnMut(i64, i64, u32)) -> f32 {
        let (tc, ts) = turn;
        let (cm, gm) = (self.font.charmap(), self.font.glyph_metrics(&[]).scale(px));
        let mut ctx: Option<ScaleContext> = None;
        let (mut pen_x, mut pen_y, mut adv_total) = (x, y, 0.0f32);
        // The key: valid (bit 63), size in quarter pixels (16 bits, 47-62: to 16383 px), glyph (16 bits, 31-46), turn (16 bits, 15-30), the pen's quarter-pixel phase (4 bits, 11-14).
        let size_q = (px * 4.0) as u64 & 0xFFFF;
        let turn_q = ((((tc * 127.0) as i64) & 0xFF) << 8 | (((ts * 127.0) as i64) & 0xFF)) as u64;
        for ch in text.chars() {
            let gid = cm.map(ch);
            let (ix, iy) = (floor(pen_x), floor(pen_y));
            let (fx, fy) = (((pen_x - ix) * 4.0) as u64 & 3, ((pen_y - iy) * 4.0) as u64 & 3);
            let key = 1 << 63 | size_q << 47 | (gid as u64) << 31 | turn_q << 15 | fx << 13 | fy << 11;
            // A glyph too big for the cache (over a quarter of it) is drawn straight from its rendering: (left, top, w, h, coverage).
            let mut big: Option<(i32, i32, u32, u32, alloc::vec::Vec<u16>)> = None;
            let e = match self.lookup(key) {
                Some(e) => e,
                None => {
                    self.misses += 1;
                    let ctx = ctx.get_or_insert_with(ScaleContext::new);
                    let mut scaler = ctx.builder(self.font).size(px).hint(false).build();
                    // The outline turned by (c, s): zeno's matrix is (xx, yx, xy, yy) column by column, and turning in the font's y-up space shows clockwise on the y-down screen — checked on the host with Latin text at 30° (the first try turned each glyph the other way from its baseline).
                    let tf = Transform::new(tc, -ts, ts, tc, 0.0, 0.0);
                    // The outline scaled and turned by swash, its coverage by `raster` at 16 bits, offset by the pen's quarter-pixel phase (font space, y up).
                    let img = scaler.scale_outline(gid).and_then(|mut o| {
                        o.transform(&tf);
                        raster(&o, fx as f32 * 0.25, -(fy as f32) * 0.25)
                    });
                    match img {
                        Some((l, t, w, h, data)) => {
                            if (w * h) as usize > self.arena.len() / 4 {
                                big = Some((l, t, w, h, data));
                                EMPTY
                            } else {
                                self.insert(key, l, t, w, h, &data)
                            }
                        }
                        None => EMPTY,
                    }
                }
            };
            // The glyph's pixel (left, −top) from the pen's whole pixel; its coverage from the cache, or from its own rendering when it is too big to keep.
            let (left, top, gw, gh, cov): (i64, i64, usize, usize, &[u16]) = match &big {
                Some((l, t, w, h, d)) => (*l as i64, *t as i64, *w as usize, *h as usize, &d[..]),
                None => (e.left as i64, e.top as i64, e.w as usize, e.h as usize, &self.arena[e.off as usize..e.off as usize + e.w as usize * e.h as usize]),
            };
            let (gx, gy) = (ix as i64 + left, iy as i64 - top);
            for row in 0..gh {
                for col in 0..gw {
                    let a = cov[row * gw + col] as u32;
                    if a != 0 {
                        put(gx + col as i64, gy + row as i64, a);
                    }
                }
            }
            let adv = gm.advance_width(gid);
            pen_x += adv * tc;
            pen_y += adv * ts;
            adv_total += adv;
        }
        adv_total
    }

    fn lookup(&self, key: u64) -> Option<Entry> {
        let mut i = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 52) as usize % SLOTS;
        for _ in 0..16 {
            let e = self.table[i];
            if e.key == key {
                return Some(e);
            }
            if e.key == 0 {
                return None;
            }
            i = (i + 1) % SLOTS;
        }
        None
    }

    fn insert(&mut self, key: u64, left: i32, top: i32, w: u32, h: u32, data: &[u16]) -> Entry {
        let n = (w * h) as usize;
        if n > self.arena.len() / 4 || data.len() < n {
            return EMPTY;
        }
        // Full (the arena, or the probe run): everything goes; the cache refills from what is drawn next.
        let mut slot = None;
        let mut i = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 52) as usize % SLOTS;
        for _ in 0..16 {
            if self.table[i].key == 0 {
                slot = Some(i);
                break;
            }
            i = (i + 1) % SLOTS;
        }
        if self.used + n > self.arena.len() || slot.is_none() {
            for e in self.table.iter_mut() {
                *e = EMPTY;
            }
            self.used = 0;
            slot = Some((key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 52) as usize % SLOTS);
        }
        let off = self.used;
        self.arena[off..off + n].copy_from_slice(&data[..n]);
        self.used += n;
        let e = Entry { key, off: off as u32, left: left as i16, top: top as i16, w: w as u16, h: h as u16 };
        self.table[slot.unwrap()] = e;
        e
    }
}

fn ceil(v: f32) -> f32 {
    -floor(-v)
}

/// A glyph's outline (scaled and turned by swash; font space, y up) to its coverage at 16 bits, offset by (`ox`, `oy`): (left, top, width, height, coverage Q16 row by row), placed as swash placed its masks (`left` from the pen, `top` up from the baseline). Swash's own rasteriser stops at 8 bits; the console and every fluor surface carry 16-bit α (owner, 2026-11-07), so the coverage is computed here, exactly: the area of each pixel inside the outline, by signed-area accumulation (font-rs's method). Each edge, flattened to lines, deposits into the cells it crosses the area between itself and the cell's right side, signed by its direction; a running sum along the row is then each pixel's winding area. None for a glyph with no ink (a space).
fn raster(outline: &Outline, ox: f32, oy: f32) -> Option<(i32, i32, u32, u32, alloc::vec::Vec<u16>)> {
    // The box of every point, control points included, so the flattened curves (inside their control hulls) stay inside it.
    let b = outline.bounds();
    if !(b.max.x > b.min.x && b.max.y > b.min.y) {
        return None;
    }
    let left = floor(b.min.x + ox) as i32;
    let top = ceil(b.max.y + oy) as i32;
    let w = (ceil(b.max.x + ox) as i32 - left) as usize;
    let h = (top - floor(b.min.y + oy) as i32) as usize;
    // Two spare cells a row: an edge on the box's right side deposits into the cell past it.
    let stride = w + 2;
    let mut acc = alloc::vec![0f32; stride * h];
    let map = |p: Point| (p.x + ox - left as f32, top as f32 - (p.y + oy));
    let (mut start, mut at) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
    for cmd in outline.path().commands() {
        match cmd {
            Command::MoveTo(p) => {
                line(&mut acc, stride, h, at, start);
                start = map(p);
                at = start;
            }
            Command::LineTo(p) => {
                let q = map(p);
                line(&mut acc, stride, h, at, q);
                at = q;
            }
            Command::QuadTo(c, p) => {
                let (c, q) = (map(c), map(p));
                // Flattened finely enough that a chord strays under 1/16 pixel from the curve: a quadratic's deviation over n pieces is |p0 − 2c + p1| / 8n².
                let (ddx, ddy) = (at.0 - 2.0 * c.0 + q.0, at.1 - 2.0 * c.1 + q.1);
                let n = 1 + libm::sqrtf(2.0 * libm::sqrtf(ddx * ddx + ddy * ddy)) as usize;
                let p0 = at;
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    let s = (u * u * p0.0 + 2.0 * u * t * c.0 + t * t * q.0, u * u * p0.1 + 2.0 * u * t * c.1 + t * t * q.1);
                    line(&mut acc, stride, h, at, s);
                    at = s;
                }
            }
            Command::CurveTo(c0, c1, p) => {
                let (c0, c1, q) = (map(c0), map(c1), map(p));
                // A cubic's deviation over n pieces is at most 3/4 of its largest second difference over n²; under 1/16 pixel.
                let d = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| { let (x, y) = (a.0 - 2.0 * b.0 + c.0, a.1 - 2.0 * b.1 + c.1); libm::sqrtf(x * x + y * y) };
                let dd = d(at, c0, c1).max(d(c0, c1, q));
                let n = 1 + libm::sqrtf(12.0 * dd) as usize;
                let p0 = at;
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    let (a, bb, cc, dd) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    let s = (a * p0.0 + bb * c0.0 + cc * c1.0 + dd * q.0, a * p0.1 + bb * c0.1 + cc * c1.1 + dd * q.1);
                    line(&mut acc, stride, h, at, s);
                    at = s;
                }
            }
            Command::Close => {
                line(&mut acc, stride, h, at, start);
                at = start;
            }
        }
    }
    line(&mut acc, stride, h, at, start);
    let mut out = alloc::vec![0u16; w * h];
    for y in 0..h {
        let mut s = 0.0f32;
        for x in 0..w {
            s += acc[y * stride + x];
            // Nonzero winding: a pixel inside two contours of the same direction sums past one, and is one whole pixel covered, no more (the fill rule's definition, not a guard).
            let a = libm::fabsf(s);
            out[y * w + x] = if a >= 1.0 { 0xFFFF } else { (a * 65535.0 + 0.5) as u16 };
        }
    }
    Some((left, top, w as u32, h as u32, out))
}

/// One line's signed area into the accumulation rows (`raster`), from `p0` to `p1` in the glyph's pixel space (x right, y down, inside the box): row by row, the trapezoid between the line and each cell's right side, positive going down. font-rs's accumulation.
fn line(acc: &mut [f32], stride: usize, h: usize, p0: (f32, f32), p1: (f32, f32)) {
    if p0.1 == p1.1 {
        return;
    }
    let (dir, p0, p1) = if p0.1 < p1.1 { (1.0, p0, p1) } else { (-1.0, p1, p0) };
    let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
    let mut x = p0.0;
    let y1 = ceil(p1.1) as usize;
    for y in p0.1 as usize..y1.min(h) {
        let row = y * stride;
        let top = if (y as f32) > p0.1 { y as f32 } else { p0.1 };
        let bottom = if ((y + 1) as f32) < p1.1 { (y + 1) as f32 } else { p1.1 };
        let dy = bottom - top;
        let xnext = x + dxdy * dy;
        let d = dy * dir;
        let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
        let x0f = floor(x0);
        let x0i = x0f as usize;
        let x1c = ceil(x1);
        let x1i = x1c as usize;
        if x1i <= x0i + 1 {
            // Within one cell: the part right of the line's middle stays in it, the rest carries to the next.
            let xm = 0.5 * (x + xnext) - x0f;
            acc[row + x0i] += d - d * xm;
            acc[row + x0i + 1] += d * xm;
        } else {
            // Across cells: a triangle in the first, a full slope through the middle ones, a triangle into the last.
            let s = 1.0 / (x1 - x0);
            let fx0 = x0 - x0f;
            let a0 = 0.5 * s * (1.0 - fx0) * (1.0 - fx0);
            let fx1 = x1 - x1c + 1.0;
            let am = 0.5 * s * fx1 * fx1;
            acc[row + x0i] += d * a0;
            if x1i == x0i + 2 {
                acc[row + x0i + 1] += d * (1.0 - a0 - am);
            } else {
                let a1 = s * (1.5 - fx0);
                acc[row + x0i + 1] += d * (a1 - a0);
                for xi in x0i + 2..x1i - 1 {
                    acc[row + xi] += d * s;
                }
                let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                acc[row + x1i - 1] += d * (1.0 - a2 - am);
            }
            acc[row + x1i] += d * am;
        }
        x = xnext;
    }
}
