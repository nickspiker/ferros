//! Text: swash (no_std, its `libm` feature) rasterising the typeface's outlines in native f32, with the rotation applied to the outline before it is rasterised — antialiased at any angle, not a turned bitmap (owner, 2026-10-23: f32 and other native; text upright by the real angle when gravity is in). Coverage is kept in a glyph cache (a fixed arena, cleared whole when it fills), keyed by glyph, size (quarter pixels), turn (Q8 cos and sin) and the pen's quarter-pixel phase, so static text costs a lookup and a blit.
//!
//! Composition is front to back like every other scene pass: each coverage sample goes under what is there, premultiplied, so text drawn first stays on top and anything drawn after lands only where the glyph leaves room.

use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Format, Transform, Vector};
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
    arena: &'static mut [u8],
    used: usize,
    pub misses: u64,
}

impl Glyphs {
    /// From the typeface's bytes; the table and arena are allocated now (before the heap's mark) and kept for the process.
    pub fn new(ttf: &'static [u8]) -> Option<Glyphs> {
        let font = FontRef::from_index(ttf, 0)?;
        Some(Glyphs { font, table: alloc::vec![EMPTY; SLOTS].leak(), arena: alloc::vec![0u8; ARENA].leak(), used: 0, misses: 0 })
    }

    /// The width of `s` at `px` pixels, along its own baseline.
    pub fn width(&self, s: &str, px: f32) -> f32 {
        let (cm, gm) = (self.font.charmap(), self.font.glyph_metrics(&[]).scale(px));
        s.chars().map(|c| gm.advance_width(cm.map(c))).sum()
    }

    /// Draw `s` at `px` pixels with its baseline starting at (x, y), turned by (c, s) on the screen (clockwise for positive s, y down; (1, 0) upright), in `colour` (G#RRGGBB) under what is there. Returns the advance along the baseline.
    pub fn draw(&mut self, px_buf: &mut [u32], width: usize, height: usize, text: &str, x: f32, y: f32, px: f32, turn: (f32, f32), colour: u32) -> f32 {
        self.cover(text, x, y, px, turn, |sx, sy, a| {
            if sx >= 0 && sy >= 0 && (sx as usize) < width && (sy as usize) < height {
                let p = &mut px_buf[sy as usize * width + sx as usize];
                *p = crate::draw::under(*p, crate::draw::at_coverage(colour & 0x00FF_FFFF, a));
            }
        })
    }

    /// The coverage of `s` (0-255 per pixel, screen coordinates) handed to `put`, for any surface: the canvas (`draw`) or an app's 16-bit surface. Returns the advance.
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
            let mut big: Option<(i32, i32, u32, u32, alloc::vec::Vec<u8>)> = None;
            let e = match self.lookup(key) {
                Some(e) => e,
                None => {
                    self.misses += 1;
                    let ctx = ctx.get_or_insert_with(ScaleContext::new);
                    let mut scaler = ctx.builder(self.font).size(px).hint(false).build();
                    // The outline turned by (c, s): zeno's matrix is (xx, yx, xy, yy) column by column, and turning in the font's y-up space shows clockwise on the y-down screen — checked on the host with Latin text at 30° (the first try turned each glyph the other way from its baseline).
                    let tf = Transform::new(tc, -ts, ts, tc, 0.0, 0.0);
                    let offset = Vector::new(fx as f32 * 0.25, -(fy as f32) * 0.25);
                    match Render::new(&[Source::Outline]).format(Format::Alpha).offset(offset).transform(Some(tf)).render(&mut scaler, gid) {
                        Some(img) => {
                            let (l, t, w, h) = (img.placement.left, img.placement.top, img.placement.width, img.placement.height);
                            if (w * h) as usize > ARENA / 4 {
                                big = Some((l, t, w, h, img.data));
                                EMPTY
                            } else {
                                self.insert(key, l, t, w, h, &img.data)
                            }
                        }
                        None => EMPTY,
                    }
                }
            };
            // The glyph's pixel (left, −top) from the pen's whole pixel; its coverage from the cache, or from its own rendering when it is too big to keep.
            let (left, top, gw, gh, cov): (i64, i64, usize, usize, &[u8]) = match &big {
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

    fn insert(&mut self, key: u64, left: i32, top: i32, w: u32, h: u32, data: &[u8]) -> Entry {
        let n = (w * h) as usize;
        if n > ARENA / 4 || data.len() < n {
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
        if self.used + n > ARENA || slot.is_none() {
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
