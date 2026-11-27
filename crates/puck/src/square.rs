//! A puck's square (docs/PUCKS.md, 2026-11-18): its face drawn by the puck itself into `d` × `d` pixels of fluor's G2t16 (α in bits 48-63, then each channel's premultiplied darkness of a VSF g2t code, R in bits 32-47), the form the display takes a square in, trims to the circle and places.
//!
//! The puck draws front to back: everything goes under what is already there (fluor's `under`), the list's first op in front, its plane last.
//! Text is drawn in the display's own typeface (ferros_glyphs), so a puck's words look like the system's.

use crate::draw::{self, Ctx, Geom, Target};
use crate::{theme, Colour};
use ferros_glyphs::Glyphs;

/// The theme's colours a list may name (G#RRGGBB, 8-bit VSF RGB gamma 2 codes, white at 255), by `theme` slot: text, text2, plane, ring, lit.
pub type Theme = [u32; theme::END as usize];

/// An 8-bit VSF RGB gamma 2 code (white 255) as fluor's 16-bit darkness, straight: the g2t code is 2^16 · tanh(c / 255) (the display's `g2t_colour` with the code's working value c / 255 of white), its darkness the complement; tanh(1) is 0.76, so the code never reaches 2^16.
fn dark(c: u8) -> u64 {
    65535 - (libm::tanhf(c as f32 * (1. / 255.)) * 65536.) as u64
}

/// A colour as an opaque G2t16 pixel.
pub fn solid(rgb: u32) -> u64 {
    0xFFFF << 48 | dark((rgb >> 16) as u8) << 32 | dark((rgb >> 8) as u8) << 16 | dark(rgb as u8)
}

/// `src` under `dst`, both premultiplied: `dst` keeps what it has and takes `src` in the room its α leaves (fluor's `under` on every lane, α and darkness alike).
#[inline]
pub fn under(dst: u64, src: u64) -> u64 {
    let da = dst >> 48;
    if da == 0xFFFF {
        return dst;
    }
    let room = 0xFFFF - da;
    let lane = |sh: u32| -> u64 { (dst >> sh & 0xFFFF) + ((src >> sh & 0xFFFF) * room + 0x7FFF) / 0xFFFF << sh };
    lane(48) | lane(32) | lane(16) | lane(0)
}

/// An opaque pixel at coverage `a` (Q16, 0 to 65535), every lane scaled.
#[inline]
pub fn cover(p: u64, a: u32) -> u64 {
    let a = a as u64;
    let lane = |sh: u32| -> u64 { ((p >> sh & 0xFFFF) * a + 0x7FFF) / 0xFFFF << sh };
    lane(48) | lane(32) | lane(16) | lane(0)
}

/// The square as a [`Target`].
pub struct Square<'a> {
    px: &'a mut [u64],
    d: usize,
    glyphs: &'a mut Glyphs,
    theme: &'a Theme,
    /// The last colour made a pixel, and that pixel.
    last: (Option<Colour>, u64, u32),
}

impl Square<'_> {
    /// A colour's G#RRGGBB and its opacity (0 to 255).
    fn rgb(&self, c: Colour) -> (u32, u8) {
        match c {
            Colour::Rgb { r, g, b, a } => ((r as u32) << 16 | (g as u32) << 8 | b as u32, a),
            Colour::Theme { slot, a } => (self.theme[slot as usize] & 0x00FF_FFFF, a),
        }
    }

    /// Colour `c` as an opaque pixel and its opacity in Q16, made once per colour (an op covers many pixels in one).
    fn pixel(&mut self, c: Colour) -> (u64, u32) {
        if self.last.0 != Some(c) {
            let (rgb, a) = self.rgb(c);
            self.last = (Some(c), solid(rgb), a as u32 * 257);
        }
        (self.last.1, self.last.2)
    }
}

impl Target for Square<'_> {
    fn clip(&self) -> (i32, i32, i32, i32) {
        (0, 0, self.d as i32, self.d as i32)
    }
    fn under(&mut self, x: i32, y: i32, cov: f32, c: Colour) {
        let (p, _) = self.pixel(c);
        let i = y as usize * self.d + x as usize;
        // `cov` already carries the colour's own opacity (draw.rs `shade`); coverage in [0, 1] to Q16.
        self.px[i] = under(self.px[i], cover(p, (cov * 65535.) as u32));
    }
    fn text(&mut self, cx: f32, cy: f32, px: f32, turn: (f32, f32), c: Colour, s: &str) {
        let (p, op) = self.pixel(c);
        let tw = self.glyphs.width(s, px);
        // The baseline's start in the text's own frame, (−width/2, +0.36 em) from the centre, turned onto the square (as the display centres its text).
        let (fx, fy) = (-tw * 0.5, px * 0.36);
        let (tc, ts) = turn;
        let d = self.d as i64;
        let buf = &mut *self.px;
        self.glyphs.cover(s, cx + tc * fx - ts * fy, cy + ts * fx + tc * fy, px, turn, |x, y, a| {
            // WHY the bounds: a glyph's pixels come from its outline wherever the face placed its words, and a word near the rim reaches past the square; the index would then be past it.
            if x >= 0 && y >= 0 && x < d && y < d {
                let i = y as usize * d as usize + x as usize;
                buf[i] = under(buf[i], cover(p, ((a as u64 * op as u64) >> 16) as u32));
            }
        });
    }
}

/// Draw a checked list (`crate::check`) into `px`, a square `d` pixels a side, the circle filling it turned by (cos, sin), its plane (the theme's) behind the list: the whole face, ready for the display. `px` is cleared first.
pub fn paint(px: &mut [u64], d: usize, turn: (f32, f32), list: &[u8], ctx: &Ctx, theme: &Theme, glyphs: &mut Glyphs) {
    px[..d * d].fill(0);
    let r = d as f32 * 0.5;
    let g = Geom { cx: r, cy: r, r, cos: turn.0, sin: turn.1 };
    let mut t = Square { px: &mut px[..d * d], d, glyphs, theme, last: (None, 0, 0) };
    draw::draw(list, &g, ctx, &mut t);
    // The plane last, under everything: the whole square, the display trims it to the circle.
    let plane = solid(theme[theme::PLANE as usize] & 0x00FF_FFFF);
    for p in t.px.iter_mut() {
        *p = under(*p, plane);
    }
}

/// Words centred on (cx, cy) in the square's pixels, `size` the glyphs' height, turned by `turn`, in `rgb` (G#RRGGBB) at opacity `op` (Q16), under what is there: for a puck that draws its face itself (the clock).
#[allow(clippy::too_many_arguments)]
pub fn words(px: &mut [u64], d: usize, glyphs: &mut Glyphs, cx: f32, cy: f32, size: f32, turn: (f32, f32), rgb: u32, op: u32, s: &str) {
    let p = solid(rgb);
    let tw = glyphs.width(s, size);
    let (fx, fy) = (-tw * 0.5, size * 0.36);
    let (tc, ts) = turn;
    let d = d as i64;
    glyphs.cover(s, cx + tc * fx - ts * fy, cy + ts * fx + tc * fy, size, turn, |x, y, a| {
        // WHY the bounds: as `Square::text`'s, a glyph near the rim reaches past the square.
        if x >= 0 && y >= 0 && x < d && y < d {
            let i = y as usize * d as usize + x as usize;
            px[i] = under(px[i], cover(p, ((a as u64 * op as u64) >> 16) as u32));
        }
    });
}

/// The widest `s` is at `size` pixels, in the typeface the squares draw in.
pub fn width(glyphs: &Glyphs, s: &str, size: f32) -> f32 {
    glyphs.width(s, size)
}
