//! The scene's rasteriser, all integer (owner, 2026-10-22: "keep it integer"): damage rectangles, antialiased discs without a square root, icons sampled through a mip chain with bilinear filtering and their own alpha, and front-to-back compositing on premultiplied pixels.
//!
//! Pixels are fluor's words: G#AARRGGBB, colour premultiplied by alpha, composited front to back — what is drawn first is on top, and a pixel takes only what its alpha leaves room for. An empty pixel is 0; the panel ignores alpha, so whatever stays empty shows black.

/// A half-open pixel rectangle; empty when x0 >= x1 or y0 >= y1.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub const EMPTY: Rect = Rect { x0: i32::MAX, y0: i32::MAX, x1: i32::MIN, y1: i32::MIN };
    pub fn full(w: usize, h: usize) -> Rect {
        Rect { x0: 0, y0: 0, x1: w as i32, y1: h as i32 }
    }
    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
    pub fn union(self, o: Rect) -> Rect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        Rect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    pub fn clip(self, w: usize, h: usize) -> Rect {
        let r = Rect { x0: self.x0.max(0), y0: self.y0.max(0), x1: self.x1.min(w as i32), y1: self.y1.min(h as i32) };
        if r.is_empty() { Rect::EMPTY } else { r }
    }
    /// Widened to multiples of 4 horizontally (the publish skips empty pixels four at a time).
    pub fn align4(self, w: usize) -> Rect {
        if self.is_empty() {
            return self;
        }
        Rect { x0: self.x0 & !3, y0: self.y0, x1: ((self.x1 + 3) & !3).min(w as i32), y1: self.y1 }
    }
}

/// Empty the rectangle (transparent, 0).
pub fn clear(px: &mut [u32], width: usize, r: Rect) {
    if r.is_empty() {
        return;
    }
    for y in r.y0..r.y1 {
        let row = y as usize * width;
        px[row + r.x0 as usize..row + r.x1 as usize].fill(0);
    }
}

/// `src` (premultiplied) under `dst` (premultiplied): dst keeps what it has and takes src in the room its alpha leaves.
#[inline(always)]
pub fn under(dst: u32, src: u32) -> u32 {
    let da = dst >> 24;
    if da == 255 {
        return dst;
    }
    if dst == 0 {
        return src;
    }
    let room = 255 - da;
    let mix = |sh: u32| -> u32 {
        let d = dst >> sh & 0xFF;
        let s = src >> sh & 0xFF;
        (d + (s * room + 127) / 255).min(255) << sh
    };
    mix(24) | mix(16) | mix(8) | mix(0)
}

/// An opaque colour (G#RRGGBB) at coverage `cov` (0-255), premultiplied.
#[inline(always)]
pub fn at_coverage(colour: u32, cov: u32) -> u32 {
    if cov >= 255 {
        return 0xFF00_0000 | colour;
    }
    let m = |sh: u32| ((colour >> sh & 0xFF) * cov + 127) / 255 << sh;
    cov << 24 | m(16) | m(8) | m(0)
}

/// The rows a disc covers, with each row's horizontal reach found without a square root: the reach only shrinks moving away from the centre row, so it is walked down a pixel at a time as the rows go (O(r) for the whole disc). Calls `row(y, x_outer_lo, x_outer_hi, x_inner_lo, x_inner_hi)` with the outer span (every pixel the edge may touch) and the inner span (pixels wholly inside), both inclusive, unclipped.
fn spans(cx: i64, cy: i64, r: i64, mut row: impl FnMut(i64, i64, i64, i64, i64)) {
    // All in Q8 pixels; a pixel's centre is (x·256 + 128).
    let (ro, ri) = (r + 256, (r - 256).max(0));
    let (ro2, ri2) = (ro * ro, ri * ri);
    let ytop = (cy - ro) >> 8;
    let ybot = (cy + ro) >> 8;
    let ymid = cy >> 8;
    // Walk up from the middle row, then down from it, each side with its own shrinking reach.
    for dir in [-1i64, 1] {
        let (mut ho, mut hi) = ((ro >> 8) + 2, (ri >> 8) + 2);
        let mut y = if dir < 0 { ymid } else { ymid + 1 };
        while (dir < 0 && y >= ytop) || (dir > 0 && y <= ybot) {
            let dy = (y << 8) + 128 - cy;
            let dy2 = dy * dy;
            if dy2 > ro2 {
                break;
            }
            // Shrink the outer reach until the pixel past it is outside the outer radius; the same for the inner.
            let fits = |h: i64, r2: i64| {
                let dx = (h << 8).max(0);
                dx * dx + dy2 <= r2
            };
            while ho > 0 && !fits(ho - 1, ro2) {
                ho -= 1;
            }
            while hi > 0 && !fits(hi, ri2) {
                hi -= 1;
            }
            let c = cx >> 8;
            let inner = dy2 <= ri2;
            row(y, c - ho - 1, c + ho + 1, if inner { c - hi + 1 } else { 1 }, if inner { c + hi - 1 } else { 0 });
            y += dir;
        }
    }
}

/// A disc of `colour` (opaque G#RRGGBB) centred at (cx, cy) with radius r, all in Q8 pixels, drawn under what is there, antialiased: an edge pixel's coverage is ½ − (d² − r²)/(2r), the first-order distance from the edge, with one reciprocal per disc and no division or root per pixel. Returns the rectangle it touched.
pub fn disc(px: &mut [u32], width: usize, height: usize, cx: i64, cy: i64, r: i64, colour: u32) -> Rect {
    if r < 128 {
        return Rect::EMPTY;
    }
    let (w, h) = (width as i64, height as i64);
    let r2 = r * r;
    // 255 / (2r · 256) in Q24, so cov = 128 − ((d² − r²) · k >> 24).
    let k = ((255i64 << 24) / (2 * r)) >> 8;
    let opaque = 0xFF00_0000 | colour;
    spans(cx, cy, r, |y, xo0, xo1, xi0, xi1| {
        if y < 0 || y >= h {
            return;
        }
        let rowb = y as usize * width;
        let dy = (y << 8) + 128 - cy;
        let dy2 = dy * dy;
        for x in xo0.max(0)..=xo1.min(w - 1) {
            let p = &mut px[rowb + x as usize];
            if *p >> 24 == 255 {
                continue;
            }
            if x >= xi0 && x <= xi1 {
                *p = if *p == 0 { opaque } else { under(*p, opaque) };
                continue;
            }
            let dx = (x << 8) + 128 - cx;
            let cov = 128 - ((dx * dx + dy2 - r2) * k >> 24);
            if cov <= 0 {
                continue;
            }
            *p = under(*p, at_coverage(colour, cov.min(255) as u32));
        }
    });
    Rect { x0: ((cx - r) >> 8) as i32 - 2, y0: ((cy - r) >> 8) as i32 - 2, x1: ((cx + r) >> 8) as i32 + 3, y1: ((cy + r) >> 8) as i32 + 3 }.clip(width, height)
}

/// A glow around a disc of radius r centred at (cx, cy) (Q8 pixels): emitted light added behind whatever is already there (alpha stays as it is, so drawn after the circles it lands only where they leave room), falling off as 1/(distance from the edge) — `peak` one pixel out, half that two out, a quarter four out: a step down for every doubling of the distance (owner, 2026-10-23: "imgpx-FF0000-770000-330000"). No root anywhere, as in `disc`: with q = d² − r² = e·(2r + e) for an edge distance e, the brightness is peak · 2r · s / q — exactly peak·s/e near the edge, and a little faster far out, which bounds the reach. `scale_q8` is s, the distance at which the glow is still at peak, in Q8 pixels (256 = one pixel); `floor` the brightness (0-255) below which it stops. Gamma-space values, as the canvas holds them. Returns the rectangle it touched.
pub fn glow(px: &mut [u32], width: usize, height: usize, cx: i64, cy: i64, r: i64, colour: u32, peak: u32, scale_q8: i64, floor: u32) -> Rect {
    if r < 128 || peak == 0 {
        return Rect::EMPTY;
    }
    let (w, h) = (width as i64, height as i64);
    let r2 = r * r;
    let num = peak as i64 * 2 * r * scale_q8;
    // The farthest q that is still at or above the floor, and a radius that surely contains it: q ≥ 2r·e, so e ≤ q / 2r.
    let q_max = num / floor.max(1) as i64;
    let reach = r + q_max / (2 * r) + 256;
    let (cr, cg, cb) = ((colour >> 16 & 0xFF) as i64, (colour >> 8 & 0xFF) as i64, (colour & 0xFF) as i64);
    spans(cx, cy, reach, |y, xo0, xo1, _, _| {
        if y < 0 || y >= h {
            return;
        }
        let rowb = y as usize * width;
        let dy = (y << 8) + 128 - cy;
        let dy2 = dy * dy;
        let (x0, x1) = (xo0.max(0), xo1.min(w - 1));
        // One pixel of glow; false once the walk has reached the circle (q ≤ 0), where it stops.
        let mut put = |x: i64| -> bool {
            let dx = (x << 8) + 128 - cx;
            let q = dx * dx + dy2 - r2;
            if q <= 0 {
                return false;
            }
            if q > q_max {
                return true;
            }
            let p = &mut px[rowb + x as usize];
            let a = (*p >> 24) as i64;
            if a == 255 {
                return true;
            }
            let i = (num / q).min(peak as i64);
            // Into the room the pixel has left (255 − alpha), added and saturated per channel; alpha untouched.
            let room = 255 - a;
            let add = |c: i64, sh: u32| -> u32 {
                let have = (*p >> sh & 0xFF) as i64;
                ((have + c * i * room / 65025).min(255) as u32) << sh
            };
            *p = (*p & 0xFF00_0000) | add(cr, 16) | add(cg, 8) | add(cb, 0);
            true
        };
        // From each end of the row inward to the circle's edge: the interior is never visited (zoomed in, it is most of the screen, and visiting it pixel by pixel was the lag, 2026-10-23).
        let mut left_stop = x1 + 1;
        let mut x = x0;
        while x <= x1 {
            if !put(x) {
                left_stop = x;
                break;
            }
            x += 1;
        }
        let mut x = x1;
        while x > left_stop {
            if !put(x) {
                break;
            }
            x -= 1;
        }
    });
    Rect { x0: ((cx - reach) >> 8) as i32 - 1, y0: ((cy - reach) >> 8) as i32 - 1, x1: ((cx + reach) >> 8) as i32 + 2, y1: ((cy + reach) >> 8) as i32 + 2 }.clip(width, height)
}

/// One ring of the clock face: how far round it is (Q16 of a turn, clockwise from 12:00), which way this period goes (false: medium grey sweeps in over the dark; true: the dark sweeps back in over the grey — the ring alternates every time it wraps), and its tick count.
#[derive(Clone, Copy)]
pub struct Ring {
    pub frac_q16: u32,
    pub dark_sweep: bool,
    pub ticks: usize,
}

/// The clock's colours: the face, the swept fill, the strokes between the rings, the ticks.
pub const CLOCK_FACE: u32 = 0x00_2A_2A_2A;
pub const CLOCK_FILL: u32 = 0x00_70_70_70;
pub const CLOCK_STROKE: u32 = 0;
pub const CLOCK_TICK: u32 = 0x00_FF_FF_FF;
/// The face's bands from the outside in, in 256ths of the radius: outer ring, stroke, middle ring, stroke, inner ring, stroke, then the centre (where the time is written, by the caller).
const BANDS: [i64; 7] = [256, 214, 208, 172, 166, 130, 124];

/// Blend two colours by `c` of 256 (the first at c = 256).
fn mix(a: u32, b: u32, c: i64) -> u32 {
    let c = c.clamp(0, 256) as u32;
    let m = |sh: u32| (((a >> sh & 0xFF) * c + (b >> sh & 0xFF) * (256 - c)) >> 8) << sh;
    m(16) | m(8) | m(0)
}

/// The clock face (the map's clock circle) at (cx, cy), radius r, Q8 pixels: three rings, each a sweep from 12:00 in the fill colour over the face colour, separated by black strokes, with white ticks; under what is there (the caller writes the time in the centre first). All squared distances and cross products — no root, no angle: a pixel is inside a sweep of angle θ when it is anticlockwise of θ's direction (one cross product), on the right half for θ ≤ ½ turn, either for more. `dirs[k]` is the unit direction (sin, −cos) of k/60 of a turn, Q14. Returns the rectangle it touched.
pub fn clock(px: &mut [u32], width: usize, height: usize, cx: i64, cy: i64, r: i64, rings: &[Ring; 3], dirs: &[(i64, i64); 60]) -> Rect {
    if r < 2048 {
        return Rect::EMPTY;
    }
    let (w, h) = (width as i64, height as i64);
    let rad: [i64; 7] = core::array::from_fn(|i| r * BANDS[i] >> 8);
    // Ticks first (front to back: what is drawn first stays on top): radial 1-pixel lines across each ring, inset a pixel at both ends.
    for (ri, ring) in rings.iter().enumerate() {
        let (r_out, r_in) = (rad[2 * ri] - 256, rad[2 * ri + 1] + 256);
        let n = ring.ticks.max(1);
        for k in 0..n {
            let (ux, uy) = dirs[(k * 60 / n) % 60];
            let (ex, ey) = (cx + (ux * r_out >> 14), cy + (uy * r_out >> 14));
            let (sx, sy) = (cx + (ux * r_in >> 14), cy + (uy * r_in >> 14));
            let (x0, x1) = ((sx.min(ex) >> 8) - 1, (sx.max(ex) >> 8) + 1);
            let (y0, y1) = ((sy.min(ey) >> 8) - 1, (sy.max(ey) >> 8) + 1);
            for y in y0.max(0)..=y1.min(h - 1) {
                for x in x0.max(0)..=x1.min(w - 1) {
                    let (dx, dy) = ((x << 8) + 128 - cx, (y << 8) + 128 - cy);
                    let along = (dx * ux + dy * uy) >> 14;
                    if along < r_in || along > r_out {
                        continue;
                    }
                    let perp = ((ux * dy - uy * dx) >> 14).abs();
                    let cov = 256 - perp;
                    if cov <= 0 {
                        continue;
                    }
                    let p = &mut px[y as usize * width + x as usize];
                    *p = under(*p, at_coverage(CLOCK_TICK, cov.min(255) as u32));
                }
            }
        }
    }
    // The sweep boundary of each ring: θ's direction (sin θ, −cos θ), Q14, from the 60-step table, interpolated.
    let bound: [(i64, i64, bool); 3] = core::array::from_fn(|i| {
        let f = rings[i].frac_q16 as i64;
        let pos = f * 60;
        let (k, t) = ((pos >> 16) as usize, pos & 0xFFFF);
        let (a, b) = (dirs[k % 60], dirs[(k + 1) % 60]);
        (a.0 + ((b.0 - a.0) * t >> 16), a.1 + ((b.1 - a.1) * t >> 16), f > 1 << 15)
    });
    let full: [bool; 3] = core::array::from_fn(|i| rings[i].frac_q16 >= 1 << 16);
    // A ring's colour at a pixel: the sweep's coverage (antialiased on its leading edge and along 12:00), fill or face by the period's direction.
    let ring_colour = |i: usize, dx: i64, dy: i64| -> u32 {
        let (bx, by, past_half) = bound[i];
        let cov_x = (dx + 128).clamp(0, 256);
        let cov_line = (-((bx * dy - by * dx) >> 14) + 128).clamp(0, 256);
        let swept = if full[i] { 256 } else if rings[i].frac_q16 == 0 { 0 } else if past_half { cov_x.max(cov_line) } else { cov_x.min(cov_line) };
        if rings[i].dark_sweep { mix(CLOCK_FACE, CLOCK_FILL, swept) } else { mix(CLOCK_FILL, CLOCK_FACE, swept) }
    };
    // Each band's colour, by index: 0, 2, 4 the rings; 1, 3, 5 the strokes; 6 the centre.
    let band_colour = |b: usize, dx: i64, dy: i64| -> u32 {
        match b {
            0 | 2 | 4 => ring_colour(b / 2, dx, dy),
            1 | 3 | 5 => CLOCK_STROKE,
            _ => CLOCK_FACE,
        }
    };
    let r2: [i64; 7] = core::array::from_fn(|i| rad[i] * rad[i]);
    // 255 / (2 rad) in Q24 per boundary, as in `disc`: the first-order distance to a boundary from d² with a multiply.
    let kb: [i64; 7] = core::array::from_fn(|i| ((255i64 << 24) / (2 * rad[i].max(1))) >> 8);
    spans(cx, cy, r, |y, xo0, xo1, _, _| {
        if y < 0 || y >= h {
            return;
        }
        let rowb = y as usize * width;
        let dy = (y << 8) + 128 - cy;
        let dy2 = dy * dy;
        for x in xo0.max(0)..=xo1.min(w - 1) {
            let p = &mut px[rowb + x as usize];
            if *p >> 24 == 255 {
                continue;
            }
            let dx = (x << 8) + 128 - cx;
            let d2 = dx * dx + dy2;
            if d2 > r2[0] + 2 * rad[0] * 256 {
                continue;
            }
            // The band: the first boundary the pixel is outside of, counted from the inside.
            let mut b = 6;
            while b > 0 && d2 > r2[b] {
                b -= 1;
            }
            // Its coverage across the nearer boundary: outside the face (b = 0's outer edge) fades to nothing; elsewhere it blends with the neighbouring band.
            let out_cov = 128 - ((d2 - r2[b]) * kb[b] >> 24);
            let in_cov = if b < 6 { 128 + ((d2 - r2[b + 1]) * kb[b + 1] >> 24) } else { 256 };
            let here = band_colour(b, dx, dy);
            let (colour, alpha) = if out_cov < 256 && (b > 0 || out_cov > 0) {
                if b == 0 {
                    (here, out_cov.clamp(0, 255))
                } else {
                    (mix(here, band_colour(b - 1, dx, dy), out_cov), 255)
                }
            } else if in_cov < 256 && b < 6 {
                (mix(here, band_colour(b + 1, dx, dy), in_cov), 255)
            } else {
                (here, 255)
            };
            if alpha <= 0 {
                continue;
            }
            *p = under(*p, at_coverage(colour, alpha as u32));
        }
    });
    Rect { x0: ((cx - r) >> 8) as i32 - 2, y0: ((cy - r) >> 8) as i32 - 2, x1: ((cx + r) >> 8) as i32 + 3, y1: ((cy + r) >> 8) as i32 + 3 }.clip(width, height)
}

/// One icon's mip chain: level 0 is the bundle's size, each next one half that, averaged 2×2, down to 4×4. Texels are packed premultiplied words (G#AARRGGBB), with a transparent border of two texels around every level so the sampler never tests bounds: `rgba` in the icon's colours, `grey` in its photopic grey (the map at rest) — VSF RGB gamma 2 codes, as the bundle holds them.
#[derive(Clone, Copy)]
pub struct Mips {
    pub size: usize,
    pub levels: usize,
    /// Level l starts at `offset[l]` texels; its rows are `size >> l` + 4 texels long (the border).
    pub offset: [usize; 8],
    pub rgba: &'static [u32],
    pub grey: &'static [u32],
}

const PAD: usize = 2;

impl Mips {
    /// From the bundle's straight-alpha RGBA and grey planes.
    /// `decode`: a code-to-canvas table applied to the colour and grey bytes as they come in (the bundle's gamma2tanh codes to the gamma 2 values the canvas holds), or none for codes that are canvas values already.
    pub fn build(rgba_in: &[u8], grey_in: &[u8], size: usize, decode: Option<&[u8; 256]>) -> Mips {
        let dec = |c: u8| decode.map_or(c, |t| t[c as usize]);
        let mut total = 0;
        let mut offset = [0usize; 8];
        let mut levels = 0;
        let mut s = size;
        while s >= 4 && levels < 8 {
            offset[levels] = total;
            total += (s + 2 * PAD) * (s + 2 * PAD);
            levels += 1;
            s /= 2;
        }
        let rgba: &'static mut [u32] = alloc::vec![0u32; total].leak();
        let grey: &'static mut [u32] = alloc::vec![0u32; total].leak();
        let at = |l: usize, x: usize, y: usize| offset[l] + (y + PAD) * ((size >> l) + 2 * PAD) + x + PAD;
        for y in 0..size {
            for x in 0..size {
                let i = y * size + x;
                let a = rgba_in[i * 4 + 3] as u32;
                let pm = |c: u8| (dec(c) as u32 * a + 127) / 255;
                rgba[at(0, x, y)] = a << 24 | pm(rgba_in[i * 4]) << 16 | pm(rgba_in[i * 4 + 1]) << 8 | pm(rgba_in[i * 4 + 2]);
                let g = pm(grey_in[i]);
                grey[at(0, x, y)] = a << 24 | g << 16 | g << 8 | g;
            }
        }
        let mut s = size;
        for l in 1..levels {
            let n = s / 2;
            for y in 0..n {
                for x in 0..n {
                    for plane in [&mut *rgba, &mut *grey] {
                        let mut acc = [0u32; 4];
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let t = plane[at(l - 1, 2 * x + dx, 2 * y + dy)];
                            for c in 0..4 {
                                acc[c] += t >> (8 * c) & 0xFF;
                            }
                        }
                        plane[at(l, x, y)] = (0..4).fold(0, |w, c| w | ((acc[c] + 2) / 4) << (8 * c));
                    }
                }
            }
            s = n;
        }
        Mips { size, levels, offset, rgba, grey }
    }
}

/// Bilinear between four packed texels, two channels per multiply (red-blue and alpha-green each ride one word's two 16-bit halves); `fu`, `fv` in 256ths.
#[inline(always)]
fn bilerp(p00: u32, p10: u32, p01: u32, p11: u32, fu: u32, fv: u32) -> u32 {
    let (w00, w10, w01, w11) = ((256 - fu) * (256 - fv) >> 8, fu * (256 - fv) >> 8, (256 - fu) * fv >> 8, fu * fv >> 8);
    let rb = (p00 & 0x00FF_00FF) * w00 + (p10 & 0x00FF_00FF) * w10 + (p01 & 0x00FF_00FF) * w01 + (p11 & 0x00FF_00FF) * w11;
    let ag = (p00 >> 8 & 0x00FF_00FF) * w00 + (p10 >> 8 & 0x00FF_00FF) * w10 + (p01 >> 8 & 0x00FF_00FF) * w01 + (p11 >> 8 & 0x00FF_00FF) * w11;
    (rb >> 8 & 0x00FF_00FF) | (ag & 0xFF00_FF00)
}

/// An icon inside the disc of radius `r` (Q8) at (cx, cy), turned by (c, s) Q14 — screen to icon is the inverse turn — under what is there. The mip level is the smallest at least as wide as the disc, so a texel never shrinks more than 2:1; between texels, bilinear. `lit`: the icon's own colours, else its grey (the map at rest). Returns the rectangle it touched.
pub fn icon(px: &mut [u32], width: usize, height: usize, cx: i64, cy: i64, r: i64, c: i64, s: i64, m: &Mips, lit: bool) -> Rect {
    if r < 256 {
        return Rect::EMPTY;
    }
    // The art is sampled as if its circle were larger than the one drawn — by 2 px, or 2 texels of the chosen level if that is more — so the drawn edge always lands on fully opaque art: the icon's own antialiased rim (the bundle's circle cut, bilinear taps into the transparent border) never reaches the screen, and the edge's antialiasing is this disc's coverage alone (owner, 2026-10-23: "come in at least 2 px before you start drawing, so there's a trim").
    let diameter = ((2 * r) >> 8) as usize;
    let mut l = 0;
    while l + 1 < m.levels && (m.size >> (l + 1)) >= diameter {
        l += 1;
    }
    let size = (m.size >> l) as i64;
    let stride = size as usize + 2 * PAD;
    let tex = &(if lit { m.rgba } else { m.grey })[m.offset[l]..m.offset[l] + stride * stride];
    // One texel in Q8 screen pixels at radius r, and the trim: the larger of 2 px and 2 texels.
    let texel = (2 * r) / size.max(1);
    let r_art = r + (2 * 256i64).max(2 * texel);
    // Texels per screen pixel in Q16, and the four steps of the inverse turn — for the art's larger circle.
    let k = (size << 24) / (2 * r_art);
    let (du_dx, dv_dx) = (c * k >> 14, -s * k >> 14);
    let (du_dy, dv_dy) = (s * k >> 14, c * k >> 14);
    let (w, h) = (width as i64, height as i64);
    // Texel space with the border: centre at size/2 + PAD, bilinear taps at −½.
    let half = ((size << 16) >> 1) + ((PAD as i64) << 16) - (1 << 15);
    let (lo, hi) = (0i64, (size + 2 * PAD as i64 - 1) << 16);
    // The edge's coverage, as `disc` computes it: 128 − (d² − r²)·k >> 24, with k = 255 / 2r in Q24.
    let r2 = r * r;
    let kc = ((255i64 << 24) / (2 * r)) >> 8;
    spans(cx, cy, r, |y, xo0, xo1, xi0, xi1| {
        if y < 0 || y >= h {
            return;
        }
        let rowb = y as usize * width;
        let x0 = xo0.max(0);
        let x1 = xo1.min(w - 1);
        if x0 > x1 {
            return;
        }
        let (ox, oy) = ((x0 << 8) + 128 - cx, (y << 8) + 128 - cy);
        let dy2 = oy * oy;
        let mut u = half + ((ox * du_dx + oy * du_dy) >> 8);
        let mut v = half + ((ox * dv_dx + oy * dv_dy) >> 8);
        let row = &mut px[rowb + x0 as usize..=rowb + x1 as usize];
        for (n, p) in row.iter_mut().enumerate() {
            if *p >> 24 != 255 && u >= lo && v >= lo && u < hi && v < hi {
                let i = (v >> 16) as usize * stride + (u >> 16) as usize;
                let mut t = bilerp(tex[i], tex[i + 1], tex[i + stride], tex[i + stride + 1], (u >> 8 & 0xFF) as u32, (v >> 8 & 0xFF) as u32);
                // Outside the inner span: the pixel straddles the edge, so the texel is scaled by the disc's coverage (premultiplied: every channel).
                let x = x0 + n as i64;
                if x < xi0 || x > xi1 {
                    let dx = (x << 8) + 128 - cx;
                    let cov = 128 - ((dx * dx + dy2 - r2) * kc >> 24);
                    if cov <= 0 {
                        u += du_dx;
                        v += dv_dx;
                        continue;
                    }
                    if cov < 255 {
                        let c = cov as u32;
                        let sc = |sh: u32| ((t >> sh & 0xFF) * c + 127) / 255 << sh;
                        t = sc(24) | sc(16) | sc(8) | sc(0);
                    }
                }
                if t >> 24 != 0 {
                    *p = if *p == 0 { t } else { under(*p, t) };
                }
            }
            u += du_dx;
            v += dv_dx;
        }
    });
    Rect { x0: ((cx - r) >> 8) as i32 - 2, y0: ((cy - r) >> 8) as i32 - 2, x1: ((cx + r) >> 8) as i32 + 3, y1: ((cy + r) >> 8) as i32 + 3 }.clip(width, height)
}
