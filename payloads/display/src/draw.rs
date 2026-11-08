//! The scene's rasteriser, all integer (owner, 2026-10-22: "keep it integer"): damage rectangles, antialiased discs without a square root, icons sampled through a mip chain with bilinear filtering and their own alpha, and front-to-back compositing on premultiplied pixels.
//!
//! Pixels are 64-bit (the 16-bit canvas, owner, 2026-10-24): R, G, B in bits 0-15, 16-31, 32-47 as VSF RGB gamma-2 working values — white at Q14 (16384), up to 4.0 (16× white in light) — and alpha in bits 48-63, colour premultiplied by it; the same layout as an app's surface. Composited front to back — what is drawn first is on top, and a pixel takes only what its alpha leaves room for. An empty pixel is 0 and shows black. Colours handed in (the theme's) are 8-bit codes G#RRGGBB, white at 255, widened here.

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
    pub fn intersect(self, o: Rect) -> Rect {
        let r = Rect { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) };
        if r.is_empty() { Rect::EMPTY } else { r }
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
/// The rectangle every primitive here draws inside: the whole surface, except while a widget's region alone is redrawn (the clock between frames) — then only that region is cleared and drawn, so what lies outside it is left exactly as the last full frame made it.
pub static mut CLIP: Rect = Rect { x0: 0, y0: 0, x1: i32::MAX, y1: i32::MAX };

/// The scene drawn by rows across the publish threads (main.rs `scene_split`, COMPOSITOR.md): while `BANDED`, each thread draws inside its own clip, told by its stack (the workers' stacks lie from `WORKER_STACKS.0`, `WORKER_STACKS.1` bytes each, `WORKER_STACKS.2` of them; any other thread is the render thread, the last clip).
pub static mut BANDED: bool = false;
pub static mut WORKER_STACKS: (usize, usize, usize) = (0, 1, 0);
pub static mut BAND_CLIP: [Rect; 8] = [Rect::EMPTY; 8];

/// This thread's place among the publish threads: a worker by its stack, else the render thread (the one past the workers).
#[inline(always)]
pub fn thread_slot() -> usize {
    let sp: usize;
    unsafe { core::arch::asm!("mov {}, sp", out(reg) sp, options(nomem, nostack, preserves_flags)) };
    let (base, size, n) = unsafe { WORKER_STACKS };
    let i = sp.wrapping_sub(base) / size;
    // WHY the test: a thread outside the workers' stacks lands past the last of them; it is the render thread.
    if i < n { i } else { n }
}

/// `CLIP` on this surface: (x0, y0, x1, y1), half-open; while the scene is drawn by rows, this thread's own band of it.
#[inline(always)]
pub fn bounds(width: usize, height: usize) -> (i64, i64, i64, i64) {
    let c = unsafe { if BANDED { BAND_CLIP[thread_slot()] } else { CLIP } }.clip(width, height);
    (c.x0 as i64, c.y0 as i64, c.x1 as i64, c.y1 as i64)
}

/// A canvas pixel: R, G, B, A, 16 bits each (module doc).
pub type Px = u64;
const ALPHA: u64 = 0xFFFF << 48;

/// An 8-bit code (white at 255) as a 16-bit working value (white at Q14).
#[inline(always)]
pub fn wide8(c: u32) -> u64 {
    ((c as u64 & 0xFF) * 16384 + 127) / 255
}

/// G#RRGGBB (8-bit codes) as an opaque pixel.
#[inline(always)]
pub fn opaque(colour: u32) -> Px {
    ALPHA | wide8(colour >> 16) | wide8(colour >> 8) << 16 | wide8(colour) << 32
}

pub fn clear(px: &mut [Px], width: usize, r: Rect) {
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
pub fn under(dst: Px, src: Px) -> Px {
    let da = dst >> 48;
    if da == 0xFFFF {
        return dst;
    }
    if dst == 0 {
        return src;
    }
    let room = 0xFFFF - da;
    let mix = |sh: u32| -> u64 {
        let d = dst >> sh & 0xFFFF;
        let s = src >> sh & 0xFFFF;
        (d + (s * room + 0x7FFF) / 0xFFFF).min(0xFFFF) << sh
    };
    mix(48) | mix(32) | mix(16) | mix(0)
}

/// An opaque colour (G#RRGGBB, 8-bit codes) at coverage `cov` (0-255), premultiplied: the 16-bit form at `cov` × 257 (255 to 65535).
#[inline(always)]
pub fn at_coverage(colour: u32, cov: u32) -> Px {
    at_cover16(colour, cov * 257)
}

/// An opaque colour (G#RRGGBB, 8-bit codes) at 16-bit coverage `cov` (0-65535: the glyph engine's), premultiplied.
#[inline(always)]
pub fn at_cover16(colour: u32, cov: u32) -> Px {
    if cov >= 65535 {
        return opaque(colour);
    }
    let a = cov as u64;
    let m = |c: u32| (wide8(c) * a + 0x7FFF) / 0xFFFF;
    a << 48 | m(colour >> 16) | m(colour >> 8) << 16 | m(colour) << 32
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
pub fn disc(px: &mut [Px], width: usize, height: usize, cx: i64, cy: i64, r: i64, colour: u32) -> Rect {
    if r < 128 {
        return Rect::EMPTY;
    }
    let (bx0, by0, bx1, by1) = bounds(width, height);
    let r2 = r * r;
    // 255 / (2r · 256) in Q24, so cov = 128 − ((d² − r²) · k >> 24).
    let k = ((255i64 << 24) / (2 * r)) >> 8;
    let solid = opaque(colour);
    spans(cx, cy, r, |y, xo0, xo1, xi0, xi1| {
        if y < by0 || y >= by1 {
            return;
        }
        let rowb = y as usize * width;
        let dy = (y << 8) + 128 - cy;
        let dy2 = dy * dy;
        for x in xo0.max(bx0)..=xo1.min(bx1 - 1) {
            let p = &mut px[rowb + x as usize];
            if *p >> 48 == 0xFFFF {
                continue;
            }
            if x >= xi0 && x <= xi1 {
                *p = if *p == 0 { solid } else { under(*p, solid) };
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
/// log2 of q > 0 in Q16 octaves, without a divide or a table (Mitchell): the top bit's place is the integer part, the bits under it the fraction, plus m·(1 − m)·0.3466 (one multiply), which takes the straight-line fraction's kinks at each octave down to under a hundredth of an octave.
#[inline(always)]
fn log2_q16(q: u64) -> i64 {
    let lz = q.leading_zeros();
    let m = ((q << lz) << 1 >> 48) as i64;
    (((63 - lz) as i64) << 16) + m + ((m * (65536 - m) >> 16) * 89 >> 8)
}

/// The glow, falling off in log distance (owner, 2026-10-24: no divide, no table): down by the same step every octave of q = d² − r² (the squared distance past the edge), from full `peak` to exactly zero over 2^`oct_log2` octaves — no floor, no tail cut — and reaching 1/`frac` of the radius past the edge, whatever the size (owner: smaller circle, thinner glow; the glow is part of the circle's shape at every zoom). At d = r(1 + 1/frac), q = r²·(2·frac + 1)/frac², which fixes where it ends; full light holds to that over 2^(2^oct_log2). A layer of the glow's colour at α = its intensity, put under what is there (`under`, gamma-2 values), so a circle's opaque interior takes nothing and needs no test (the linear-light try looked worse, 2026-10-24).
pub fn glow_log(px: &mut [Px], width: usize, height: usize, cx: i64, cy: i64, r: i64, colour: u32, peak: u32, frac: i64, oct_log2: u32) -> Rect {
    if r < 128 || peak == 0 {
        return Rect::EMPTY;
    }
    let (bx0, by0, bx1, by1) = bounds(width, height);
    let r2 = r * r;
    // Zero at q_max (1/frac of the radius past the edge), full light to q_max over 2^octaves.
    let q_max = r2 * (2 * frac + 1) / (frac * frac);
    let q0 = (q_max >> (1 << oct_log2)).max(1);
    let l0 = log2_q16(q0 as u64);
    let span = 1i64 << (16 + oct_log2);
    let reach = r + r / frac + 256;
    let peak = peak as i64;
    spans(cx, cy, reach, |y, xo0, xo1, _, _| {
        if y < by0 || y >= by1 {
            return;
        }
        let rowb = y as usize * width;
        let dy = (y << 8) + 128 - cy;
        let dy2 = dy * dy;
        let (x0, x1) = (xo0.max(bx0), xo1.min(bx1 - 1));
        let mut put = |x: i64| -> bool {
            let dx = (x << 8) + 128 - cx;
            let q = dx * dx + dy2 - r2;
            if q <= 0 {
                return false;
            }
            let t = (log2_q16(q as u64) - l0).clamp(0, span);
            // The intensity in 2⁻⁸ steps of `peak`'s (16 bits, as the canvas): whole steps of 1/255 banded the glows' long gradients.
            let i = (peak << 8) * (span - t) >> (16 + oct_log2);
            // The glow is a layer of its own colour at α = its intensity (i in 2⁻⁸ steps of 255, at most peak · 2⁸ ≤ G#FF00), under what is there (owner, 2026-11-07, option 3): over black the light is what the additive glow gave, overlapping glows stack as 1 − (1 − a)(1 − b) and never pass white, and the pixel carries the glow's opacity, so whatever goes under it later (a wallpaper) shows in exactly the room it leaves.
            let p = &mut px[rowb + x as usize];
            *p = under(*p, at_cover16(colour, (i * 0xFFFF / (255 << 8)) as u32));
            true
        };
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

pub fn glow(px: &mut [Px], width: usize, height: usize, cx: i64, cy: i64, r: i64, colour: u32, peak: u32, scale_q8: i64, floor: u32) -> Rect {
    if r < 128 || peak == 0 {
        return Rect::EMPTY;
    }
    let (bx0, by0, bx1, by1) = bounds(width, height);
    let r2 = r * r;
    let num = peak as i64 * 2 * r * scale_q8;
    // The farthest q that is still at or above the floor, and a radius that surely contains it: q ≥ 2r·e, so e ≤ q / 2r.
    let q_max = num / floor.max(1) as i64;
    let reach = r + q_max / (2 * r) + 256;
    spans(cx, cy, reach, |y, xo0, xo1, _, _| {
        if y < by0 || y >= by1 {
            return;
        }
        let rowb = y as usize * width;
        let dy = (y << 8) + 128 - cy;
        let dy2 = dy * dy;
        let (x0, x1) = (xo0.max(bx0), xo1.min(bx1 - 1));
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
            // The intensity in 2⁻⁸ steps of `peak`'s (16 bits, as the canvas): whole steps of 1/255 banded the glows' long gradients. The `min` is the falloff's own peak: 1/distance grows without bound toward the edge and the glow holds its peak there.
            let i = ((num << 8) / q).min((peak as i64) << 8);
            // A layer of its own colour at α = its intensity, under what is there, as in `glow_log` (owner, 2026-11-07, option 3).
            *p = under(*p, at_cover16(colour, (i * 0xFFFF / (255 << 8)) as u32));
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

/// One ring of the clock face: how far round it is (Q16 of a turn, clockwise from 12:00; the sweep clicks back to dark when it wraps), whether its fill is the lighter grey (the hour ring after noon), and its tick count.
#[derive(Clone, Copy)]
pub struct Ring {
    pub frac_q16: u32,
    pub lighter: bool,
    pub ticks: usize,
}

/// The clock's colours come from the theme (ferros_sys::theme): the face is its plane, then the swept fill, the fill after noon, the strokes between the rings, the ticks ahead of the fill and those it has passed.
/// The face's bands from the outside in, in 256ths of the radius: each ring then its stroke, outermost first, then the centre (where the time is written, by the caller). Three rings (decimal: seconds, minutes, hours), four (dozenal: the 50 s unit's twelfths, the 50 s units, the twelfths of a dozenal hour, the dozenal hours) or six (the dozenal clock in focus: two more places); the centre is the same size in all.
const BANDS3: [i64; 7] = [256, 214, 208, 172, 166, 130, 124];
const BANDS4: [i64; 9] = [256, 222, 216, 188, 182, 156, 150, 130, 124];
/// Six rings (the dozenal clock in focus, its day in twelfths six times over): narrower, the centre the same size.
const BANDS6: [i64; 13] = [256, 240, 234, 218, 212, 196, 190, 174, 168, 152, 146, 130, 124];

/// The bands for an `n`-ring face.
fn bands_for(n: usize) -> &'static [i64] {
    match n {
        6 => &BANDS6,
        4 => &BANDS4,
        _ => &BANDS3,
    }
}

/// Ring `i` of an `n`-ring face of radius r (Q8): its outer and inner radius, Q8.
pub fn clock_ring_radii(r: i64, n: usize, i: usize) -> (i64, i64) {
    let bands = bands_for(n);
    (r * bands[2 * i] >> 8, r * bands[2 * i + 1] >> 8)
}

/// The centre's radius (where the digits are), Q8.
pub fn clock_centre_radius(r: i64, n: usize) -> i64 {
    let bands = bands_for(n);
    r * bands[2 * n] >> 8
}

/// The face's direction at `frac_q16` of a turn from 12:00, (sin, −cos) Q14, from the 60-step table, interpolated (as the sweep's boundary is drawn).
pub fn clock_dir(dirs: &[(i64, i64); 60], frac_q16: u32) -> (i64, i64) {
    let pos = frac_q16 as i64 * 60;
    let (k, t) = ((pos >> 16) as usize, pos & 0xFFFF);
    let (a, b) = (dirs[k % 60], dirs[(k + 1) % 60]);
    (a.0 + ((b.0 - a.0) * t >> 16), a.1 + ((b.1 - a.1) * t >> 16))
}

/// A premultiplied pixel at coverage `cov` (Q16, 0 to 65536 whole): every lane, α included, scaled. Each lane stays in its field (G#FFFF × 2¹⁶ >> 16).
#[inline(always)]
fn at_cov(p: Px, cov: i64) -> Px {
    let c = cov as u64;
    let s = |sh: u32| ((p >> sh & 0xFFFF) * c >> 16) << sh;
    s(48) | s(32) | s(16) | s(0)
}

/// The clock face (the map's clock circle) at (cx, cy), radius r, Q8 pixels: three rings, each a sweep from 12:00 in the fill colour over the face colour, separated by black strokes, with white ticks; under what is there (the caller writes the time in the centre first). All squared distances and cross products — no root, no angle: a pixel is inside a sweep of angle θ when it is anticlockwise of θ's direction (one cross product), on the right half for θ ≤ ½ turn, either for more. `dirs[k]` is the unit direction (sin, −cos) of k/60 of a turn, Q14. Returns the rectangle it touched.
/// `(tc, ts)`: the face's turn, Q14 (the circle's own turn on top of the camera's): ticks, sweeps and the zero mark turn with it.
pub fn clock(px: &mut [Px], width: usize, height: usize, cx: i64, cy: i64, r: i64, rings: &[Ring], dirs: &[(i64, i64); 60], tc: i64, ts: i64) -> Rect {
    if r < 2048 {
        return Rect::EMPTY;
    }
    let (w, h) = (width as i64, height as i64);
    let (bx0, by0, bx1, by1) = bounds(width, height);
    let th = ferros_sys::theme::current();
    let (clock_face, clock_fill, clock_fill_pm, clock_stroke, clock_tick, clock_tick_passed) = (th.plane, th.clock_fill, th.clock_fill_pm, th.clock_stroke, th.clock_tick, th.clock_tick_passed);
    let n = rings.len().clamp(1, 6);
    let bands = bands_for(n);
    // The centre's band index: 2n (each ring and its stroke before it).
    let cb = 2 * n;
    let rad: [i64; 13] = core::array::from_fn(|i| if i < bands.len() { r * bands[i] >> 8 } else { 0 });
    // A face direction to the screen: turned by (tc, ts).
    let turn = |(ux, uy): (i64, i64)| ((ux * tc - uy * ts) >> 14, (ux * ts + uy * tc) >> 14);
    // The tick segments: each ring's ticks across it, and the zero mark at the top of the centre — (direction, inner radius, outer radius).
    // A tick the ring's fill has reached is black (owner, 2026-10-24): tick k of a ring with t ticks at k/t of a turn, reached once the sweep is past it (the 12:00 tick as soon as it has begun).
    let mut segs: [((i64, i64), i64, i64, u32); 256] = [((0, 0), 0, 0, 0); 256];
    let mut nseg = 0;
    for (ri, ring) in rings.iter().take(n).enumerate() {
        let t = ring.ticks.max(1);
        for k in 0..t {
            if nseg < segs.len() - 1 {
                let reached = ring.frac_q16 >= 1 << 16 || (ring.frac_q16 > 0 && (ring.frac_q16 as u64) * t as u64 >= (k as u64) << 16);
                segs[nseg] = (turn(dirs[(k * 60 / t) % 60]), rad[2 * ri + 1] + 256, rad[2 * ri] - 256, if reached { clock_tick_passed } else { clock_tick });
                nseg += 1;
            }
        }
    }
    // The zero mark: from 70 % of the centre's radius to a pixel inside its edge, on the face's 12:00.
    segs[nseg] = (turn(dirs[0]), rad[cb] * 7 / 10, rad[cb] - 256, clock_tick);
    nseg += 1;
    // Ticks first (front to back: what is drawn first stays on top): radial 1-pixel lines across each ring, inset a pixel at both ends.
    for &((ux, uy), r_in, r_out, tick) in &segs[..nseg] {
        {
            let (ex, ey) = (cx + (ux * r_out >> 14), cy + (uy * r_out >> 14));
            let (sx, sy) = (cx + (ux * r_in >> 14), cy + (uy * r_in >> 14));
            // Along the tick's own length, a pixel at a time on its major axis, touching the three pixels across it: a box around a diagonal tick is its length squared, a million pixels a tick when zoomed in.
            let mut touch = |x: i64, y: i64| {
                if x < bx0 || y < by0 || x >= bx1 || y >= by1 {
                    return;
                }
                let (dx, dy) = ((x << 8) + 128 - cx, (y << 8) + 128 - cy);
                let along = (dx * ux + dy * uy) >> 14;
                if along < r_in || along > r_out {
                    return;
                }
                let perp = ((ux * dy - uy * dx) >> 14).abs();
                let cov = 256 - perp;
                if cov <= 0 {
                    return;
                }
                let p = &mut px[y as usize * width + x as usize];
                *p = under(*p, at_coverage(tick, cov.min(255) as u32));
            };
            let (ldx, ldy) = (ex - sx, ey - sy);
            if ldx.abs() >= ldy.abs() {
                let (a, b) = ((sx.min(ex) >> 8).max(0), (sx.max(ex) >> 8).min(w - 1));
                for x in a..=b {
                    let yc = if ldx == 0 { sy } else { sy + (((x << 8) + 128 - sx) * ldy) / ldx };
                    for y in (yc >> 8) - 1..=(yc >> 8) + 1 {
                        touch(x, y);
                    }
                }
            } else {
                let (a, b) = ((sy.min(ey) >> 8).max(0), (sy.max(ey) >> 8).min(h - 1));
                for y in a..=b {
                    let xc = sx + (((y << 8) + 128 - sy) * ldx) / ldy;
                    for x in (xc >> 8) - 1..=(xc >> 8) + 1 {
                        touch(x, y);
                    }
                }
            }
        }
    }
    // The sweep boundary of each ring: θ's direction (sin θ, −cos θ), Q14, from the 60-step table, interpolated.
    let bound: [(i64, i64, bool); 6] = core::array::from_fn(|i| {
        let f = rings.get(i).map(|r| r.frac_q16).unwrap_or(0) as i64;
        let pos = f * 60;
        let (k, t) = ((pos >> 16) as usize, pos & 0xFFFF);
        let (a, b) = (dirs[k % 60], dirs[(k + 1) % 60]);
        (a.0 + ((b.0 - a.0) * t >> 16), a.1 + ((b.1 - a.1) * t >> 16), f > 1 << 15)
    });
    let full: [bool; 6] = core::array::from_fn(|i| rings.get(i).map(|r| r.frac_q16 >= 1 << 16).unwrap_or(false));
    // The face's colours as opaque canvas pixels, 16 bits a channel: everything below composes on these, front to back, and nothing is rounded back to 8 bits.
    let (face_px, fill_px, fill_pm_px, stroke_px) = (opaque(clock_face), opaque(clock_fill), opaque(clock_fill_pm), opaque(clock_stroke));
    // A ring at a pixel: the sweep's fill in front at its coverage (antialiased on its leading edge and along 12:00, Q16), the face under it.
    let ring_px = |i: usize, dx: i64, dy: i64| -> Px {
        // Into the face's own frame (the turn undone), where 12:00 is up.
        let (dx, dy) = ((dx * tc + dy * ts) >> 14, (dy * tc - dx * ts) >> 14);
        let (bx, by, past_half) = bound[i];
        // WHY the clamps: a half-plane's coverage of a pixel is its signed distance from the pixel's centre plus half a pixel, and it covers the pixel wholly (or not at all) from half a pixel on; these are the coverage's definition, not a guard (distances in Q8, coverage in Q16).
        let cov_x = ((dx << 8) + (1 << 15)).clamp(0, 1 << 16);
        let cov_line = ((-((bx * dy - by * dx) >> 14) << 8) + (1 << 15)).clamp(0, 1 << 16);
        let swept = if full[i] { 1 << 16 } else if rings[i].frac_q16 == 0 { 0 } else if past_half { cov_x.max(cov_line) } else { cov_x.min(cov_line) };
        let fill = if rings[i].lighter { fill_pm_px } else { fill_px };
        if swept == 0 {
            face_px
        } else if swept == 1 << 16 {
            fill
        } else {
            under(at_cov(fill, swept), face_px)
        }
    };
    // Each band at a pixel, by index: even below the centre the rings, odd the strokes, `cb` the centre.
    let band_px = |b: usize, dx: i64, dy: i64| -> Px {
        if b >= cb {
            face_px
        } else if b % 2 == 0 {
            ring_px(b / 2, dx, dy)
        } else {
            stroke_px
        }
    };
    let r2: [i64; 13] = core::array::from_fn(|i| rad[i] * rad[i]);
    // 2¹⁶ / (2 rad) in Q24 per boundary, as in `disc` at 16 bits: the first-order distance to a boundary from d² with a multiply, as Q16 coverage.
    let kb: [i64; 13] = core::array::from_fn(|i| ((1i64 << 40) / (2 * rad[i].max(1))) >> 8);
    spans(cx, cy, r, |y, xo0, xo1, _, _| {
        if y < by0 || y >= by1 {
            return;
        }
        let rowb = y as usize * width;
        let dy = (y << 8) + 128 - cy;
        let dy2 = dy * dy;
        for x in xo0.max(bx0)..=xo1.min(bx1 - 1) {
            let p = &mut px[rowb + x as usize];
            if *p >> 48 == 0xFFFF {
                continue;
            }
            let dx = (x << 8) + 128 - cx;
            let d2 = dx * dx + dy2;
            if d2 > r2[0] + 2 * rad[0] * 256 {
                continue;
            }
            // The band: the first boundary the pixel is outside of, counted from the inside.
            let mut b = cb;
            while b > 0 && d2 > r2[b] {
                b -= 1;
            }
            // Its coverage across the nearer boundary, Q16 (at least half: the pixel's centre is inside its band, except outside the face). Outside the face (b = 0's outer edge) it fades to nothing; elsewhere the band is in front at its coverage and its neighbour across that boundary under it, so the face stays opaque.
            let out_cov = (1 << 15) - ((d2 - r2[b]) * kb[b] >> 24);
            let in_cov = if b < cb { (1 << 15) + ((d2 - r2[b + 1]) * kb[b + 1] >> 24) } else { 1 << 16 };
            // Half a pixel or more past the face's edge: nothing. (Until 2026-11-07 such a pixel fell through to the band's own colour, opaque: the walk reaches a pixel past the edge, so a hard rim stood outside the antialiased one, the face's edge stepped.)
            if b == 0 && out_cov <= 0 {
                continue;
            }
            let here = band_px(b, dx, dy);
            let face = if out_cov < 1 << 16 {
                if b == 0 {
                    at_cov(here, out_cov)
                } else {
                    under(at_cov(here, out_cov), band_px(b - 1, dx, dy))
                }
            } else if in_cov < 1 << 16 && b < cb {
                under(at_cov(here, in_cov), band_px(b + 1, dx, dy))
            } else {
                here
            };
            *p = under(*p, face);
        }
    });
    Rect { x0: ((cx - r) >> 8) as i32 - 2, y0: ((cy - r) >> 8) as i32 - 2, x1: ((cx + r) >> 8) as i32 + 3, y1: ((cy + r) >> 8) as i32 + 3 }.clip(width, height)
}

/// One icon's mip chain: level 0 is the bundle's size, each next one half that, averaged 2×2, down to 4×4. Texels are canvas pixels (`Px`: 16 bits a channel, premultiplied) with a transparent border of two texels around every level so the sampler never tests bounds: `rgba` in the icon's colours, `grey` in its photopic grey (the map at rest). Sixteen bits all the way down (owner, 2026-10-24: an 8-bit image averaged down gains precision — 256×256 to 8×8 averages 1024 samples a texel): level 0 is the bundle's 8-bit codes widened, and every 2×2 average keeps its fraction instead of rounding back to 8 bits.
#[derive(Clone, Copy)]
pub struct Mips {
    pub size: usize,
    pub levels: usize,
    /// Level l starts at `offset[l]` texels; its rows are `size >> l` + 4 texels long (the border).
    pub offset: [usize; 8],
    pub rgba: &'static [Px],
    pub grey: &'static [Px],
}

const PAD: usize = 2;

impl Mips {
    /// From the bundle's straight-alpha RGBA and grey planes.
    /// `decode`: each bundle code's 16-bit canvas value (gamma2tanh or gamma 2 codes to working values, white at Q14, highlights above it kept).
    pub fn build(rgba_in: &[u8], grey_in: &[u8], size: usize, decode: &[u16; 256]) -> Mips {
        // The bundle's code to its 16-bit canvas value (main.rs `icon_decode`).
        let dec = |c: u8| decode[c as usize] as u64;
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
        let rgba: &'static mut [Px] = alloc::vec![0u64; total].leak();
        let grey: &'static mut [Px] = alloc::vec![0u64; total].leak();
        let at = |l: usize, x: usize, y: usize| offset[l] + (y + PAD) * ((size >> l) + 2 * PAD) + x + PAD;
        for y in 0..size {
            for x in 0..size {
                let i = y * size + x;
                let a = rgba_in[i * 4 + 3] as u64;
                // Premultiplied in 16 bits: the widened code times alpha.
                let pm = |c: u8| (dec(c) * a + 127) / 255;
                rgba[at(0, x, y)] = (a * 257) << 48 | pm(rgba_in[i * 4]) | pm(rgba_in[i * 4 + 1]) << 16 | pm(rgba_in[i * 4 + 2]) << 32;
                let g = pm(grey_in[i]);
                grey[at(0, x, y)] = (a * 257) << 48 | g | g << 16 | g << 32;
            }
        }
        let mut s = size;
        for l in 1..levels {
            let n = s / 2;
            for y in 0..n {
                for x in 0..n {
                    for plane in [&mut *rgba, &mut *grey] {
                        let mut acc = [0u64; 4];
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let t = plane[at(l - 1, 2 * x + dx, 2 * y + dy)];
                            for c in 0..4 {
                                acc[c] += t >> (16 * c) & 0xFFFF;
                            }
                        }
                        plane[at(l, x, y)] = (0..4).fold(0u64, |w, c| w | ((acc[c] + 2) / 4) << (16 * c));
                    }
                }
            }
            s = n;
        }
        Mips { size, levels, offset, rgba, grey }
    }
}

/// Bilinear between four texels, two channels per multiply (R and B, then G and A, each pair riding one word's two 32-bit halves); `fu`, `fv` in 256ths.
#[inline(always)]
fn bilerp(p00: Px, p10: Px, p01: Px, p11: Px, fu: u32, fv: u32) -> Px {
    let (fu, fv) = (fu as u64, fv as u64);
    let (w00, w10, w01, w11) = ((256 - fu) * (256 - fv) >> 8, fu * (256 - fv) >> 8, (256 - fu) * fv >> 8, fu * fv >> 8);
    const M: u64 = 0x0000_FFFF_0000_FFFF;
    let rb = (p00 & M) * w00 + (p10 & M) * w10 + (p01 & M) * w01 + (p11 & M) * w11;
    let ga = (p00 >> 16 & M) * w00 + (p10 >> 16 & M) * w10 + (p01 >> 16 & M) * w01 + (p11 >> 16 & M) * w11;
    (rb >> 8 & M) | (ga >> 8 & M) << 16
}

/// An icon inside the disc of radius `r` (Q8) at (cx, cy), turned by (c, s) Q14 — screen to icon is the inverse turn — under what is there, always in its own colours: a circle's graphic stays the same for recognisability, and its glow says its state (owner, 2026-11-08; until then a quiet circle drew its grey). The mip level is the smallest at least as wide as the disc, so a texel never shrinks more than 2:1; between texels, bilinear. Returns the rectangle it touched.
pub fn icon(px: &mut [Px], width: usize, height: usize, cx: i64, cy: i64, r: i64, c: i64, s: i64, m: &Mips) -> Rect {
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
    let tex = &m.rgba[m.offset[l]..m.offset[l] + stride * stride];
    // One texel in Q8 screen pixels at radius r, and the trim: the larger of 2 px and 2 texels.
    let texel = (2 * r) / size.max(1);
    let r_art = r + (2 * 256i64).max(2 * texel);
    // Texels per screen pixel in Q16, and the four steps of the inverse turn — for the art's larger circle.
    let k = (size << 24) / (2 * r_art);
    let (du_dx, dv_dx) = (c * k >> 14, -s * k >> 14);
    let (du_dy, dv_dy) = (s * k >> 14, c * k >> 14);
    let (bx0, by0, bx1, by1) = bounds(width, height);
    // Texel space with the border: centre at size/2 + PAD, bilinear taps at −½.
    let half = ((size << 16) >> 1) + ((PAD as i64) << 16) - (1 << 15);
    let (lo, hi) = (0i64, (size + 2 * PAD as i64 - 1) << 16);
    // The edge's coverage, as `disc` computes it: 128 − (d² − r²)·k >> 24, with k = 255 / 2r in Q24.
    let r2 = r * r;
    let kc = ((255i64 << 24) / (2 * r)) >> 8;
    spans(cx, cy, r, |y, xo0, xo1, xi0, xi1| {
        if y < by0 || y >= by1 {
            return;
        }
        let rowb = y as usize * width;
        let x0 = xo0.max(bx0);
        let x1 = xo1.min(bx1 - 1);
        if x0 > x1 {
            return;
        }
        let (ox, oy) = ((x0 << 8) + 128 - cx, (y << 8) + 128 - cy);
        let dy2 = oy * oy;
        let mut u = half + ((ox * du_dx + oy * du_dy) >> 8);
        let mut v = half + ((ox * dv_dx + oy * dv_dy) >> 8);
        let row = &mut px[rowb + x0 as usize..=rowb + x1 as usize];
        for (n, p) in row.iter_mut().enumerate() {
            if *p >> 48 != 0xFFFF && u >= lo && v >= lo && u < hi && v < hi {
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
                        let c = cov as u64;
                        let sc = |sh: u32| ((t >> sh & 0xFFFF) * c + 127) / 255 << sh;
                        t = sc(48) | sc(32) | sc(16) | sc(0);
                    }
                }
                if t >> 48 != 0 {
                    *p = if *p == 0 { t } else { under(*p, t) };
                }
            }
            u += du_dx;
            v += dv_dx;
        }
    });
    Rect { x0: ((cx - r) >> 8) as i32 - 2, y0: ((cy - r) >> 8) as i32 - 2, x1: ((cx + r) >> 8) as i32 + 3, y1: ((cy + r) >> 8) as i32 + 3 }.clip(width, height)
}

/// A C∞ step from 0 at t ≤ 0 to 1 at t ≥ 1: the bump exp(−1/t) against its mirror. Every derivative is zero at both ends, so a disc's edge has no kink and leaves nothing past itself (the bump is defined as 0 for t ≤ 0; the two tests are its definition, not a clamp).
#[inline]
pub fn bump_step(t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let (a, b) = (libm::expf(-1.0 / t), libm::expf(-1.0 / (1.0 - t)));
    a / (a + b)
}


include!(concat!(env!("OUT_DIR"), "/logo.rs"));

/// The ferros mark inside the disc of radius `r` (Q8) at (cx, cy), turned by (c, s) Q14 (screen to mark is the inverse turn, as `icon` has it), under what is there: its pieces (`LOGO`, build.rs from logo.vsf) drawn from their geometry at this size, so it is sharp at every zoom, in place of the bundle's raster (owner, 2026-11-07).
/// A piece's coverage at a pixel is the product of its edges' C∞ steps (`bump_step` across two pixels centred on the edge, slope 1 there as area coverage rises): its outer arc, its inner arc if it is a ring, and its two radial sides if it does not go all the way round, each side a half-plane through the centre (both for a piece under half a turn, either for one over). Where two pieces meet they share the edge and their steps sum to exactly one, and pieces never overlap, so their coverages add into one layer (premultiplied, its α their sum) and that layer goes under the pixel: no seam, and nothing laid over anything.
/// By spans, row by row: only the pixels within a step of some edge (an arc's band, a side's band) are worked out one by one; between them nothing changes, so each run takes the value of its first pixel and is filled. The first cut worked out every pixel with an arctangent and a sine per piece, about 46 ms a frame at a large zoom (2026-11-07).
/// `lit`: the mark's own colours, else its grey (the map at rest). Returns the rectangle it touched.
pub fn logo(px: &mut [Px], width: usize, height: usize, cx: i64, cy: i64, r: i64, c: i64, s: i64, lit: bool) -> Rect {
    const N: usize = LOGO.len();
    // The longest row this draws into: the screen's width, or a sprite's side (SPRITE_MAX).
    const ROW: usize = 2048;
    assert!(width <= ROW, "logo: a row wider than its edge map");
    if r < 256 {
        return Rect::EMPTY;
    }
    let (rf, cxf, cyf) = (r as f32 * (1.0 / 256.0), cx as f32 * (1.0 / 256.0), cy as f32 * (1.0 / 256.0));
    let (cf, sf) = (c as f32 * (1.0 / 16384.0), s as f32 * (1.0 / 16384.0));
    let edge = |d: f32| bump_step((d + 1.0) * 0.5);
    // Each piece in screen pixels: its radii, its sides' inward normals turned onto the screen (start side toward increasing angle, end side toward decreasing; d = n · (dx, dy)), whether it goes all the way round, whether its sides are a union (over half a turn), and its colour.
    let turn = |nx: f32, ny: f32| (cf * nx - sf * ny, sf * nx + cf * ny);
    let mut pieces = [(0f32, 0f32, (0f32, 0f32), (0f32, 0f32), false, false, [0u16; 3]); N];
    for (k, &(inner, outer, start, sweep, colour, grey)) in LOGO.iter().enumerate() {
        let (a0, a1) = (start * core::f32::consts::TAU, (start + sweep) * core::f32::consts::TAU);
        let n0 = turn(-libm::sinf(a0), libm::cosf(a0));
        let n1 = turn(libm::sinf(a1), -libm::cosf(a1));
        pieces[k] = (inner * rf, outer * rf, n0, n1, sweep >= 1.0, sweep > 0.5, if lit { colour } else { [grey; 3] });
    }
    // The exact layer at an offset from the centre.
    let exact = |dx: f32, dy: f32| -> Px {
        let rho = libm::sqrtf(dx * dx + dy * dy);
        let (mut lr, mut lg, mut lb, mut la) = (0f32, 0f32, 0f32, 0f32);
        for &(rin, rout, n0, n1, whole, union, col) in &pieces {
            if rho > rout + 1.0 || rho < rin - 1.0 {
                continue;
            }
            let mut cov = edge(rout - rho);
            if rin > 0.0 {
                cov *= edge(rho - rin);
            }
            if !whole {
                let (e0, e1) = (edge(n0.0 * dx + n0.1 * dy), edge(n1.0 * dx + n1.1 * dy));
                cov *= if union { e0 + e1 - e0 * e1 } else { e0 * e1 };
            }
            lr += cov * col[0] as f32;
            lg += cov * col[1] as f32;
            lb += cov * col[2] as f32;
            la += cov;
        }
        // The casts saturate (a float to an integer stops at the type's ends): the steps sum to one only to the float's last bit, and α must not carry past its field.
        (lr as u16 as u64) | (lg as u16 as u64) << 16 | (lb as u16 as u64) << 32 | ((la * 65535.0) as u16 as u64) << 48
    };
    // The arcs' radii, each once, and the sides' normals, each once (pieces that meet share them).
    let (mut radii, mut nr) = ([0f32; 2 * N], 0);
    let (mut sides, mut ns) = ([(0f32, 0f32); 2 * N], 0);
    for &(rin, rout, n0, n1, whole, _, _) in &pieces {
        for rr in [rin, rout] {
            if rr > 0.0 && !radii[..nr].iter().any(|&q| libm::fabsf(q - rr) < 1e-3) {
                radii[nr] = rr;
                nr += 1;
            }
        }
        if !whole {
            for n in [n0, n1] {
                // A side's line, either way round: the end side of one piece is the start side of the next, its normal reversed.
                if !sides[..ns].iter().any(|&q| libm::fabsf(q.0 - n.0) + libm::fabsf(q.1 - n.1) < 1e-3 || libm::fabsf(q.0 + n.0) + libm::fabsf(q.1 + n.1) < 1e-3) {
                    sides[ns] = n;
                    ns += 1;
                }
            }
        }
    }
    // WHY the bounds: the disc follows the map's camera and is often partly off the screen (or the clip); its box is cut to them so no index lands outside the canvas.
    let (bx0, by0, bx1, by1) = bounds(width, height);
    let (y0, y1) = (((cyf - rf - 2.0) as i64).max(by0), ((cyf + rf + 2.0) as i64).min(by1 - 1));
    let (mut tx0, mut tx1) = (i64::MAX, i64::MIN);
    let mut near = [false; ROW];
    for y in y0..=y1 {
        let dy = y as f32 + 0.5 - cyf;
        let reach = rf + 1.0;
        if dy * dy >= reach * reach {
            continue;
        }
        let half = libm::sqrtf(reach * reach - dy * dy);
        let (xa, xb) = (((cxf - half - 1.0) as i64).max(bx0), ((cxf + half + 1.0) as i64).min(bx1 - 1));
        if xa > xb {
            continue;
        }
        let (xa, xb) = (xa as usize, xb as usize);
        near[xa..=xb].fill(false);
        // The columns within a step of an edge, a pixel wider each way: x's centre is at dx = x + 0.5 − cx.
        let mut mark = |lo: f32, hi: f32| {
            let (a, b) = ((cxf + lo - 1.5) as i64, (cxf + hi + 0.5) as i64);
            let (a, b) = (a.max(xa as i64), b.min(xb as i64));
            if a <= b {
                near[a as usize..=b as usize].fill(true);
            }
        };
        for &rr in &radii[..nr] {
            // The band ρ ∈ (rr − 1, rr + 1) on this row: |dx| from √((rr − 1)² − dy²) to √((rr + 1)² − dy²).
            let (o2, i2) = ((rr + 1.0) * (rr + 1.0) - dy * dy, (rr - 1.0) * (rr - 1.0) - dy * dy);
            if o2 > 0.0 {
                let (o, i) = (libm::sqrtf(o2), if i2 > 0.0 { libm::sqrtf(i2) } else { 0.0 });
                mark(-o, -i);
                mark(i, o);
            }
        }
        for &(nx, ny) in &sides[..ns] {
            // The band |nx·dx + ny·dy| < 1 on this row; a side nearly along the row covers it whole where it is within a step.
            if libm::fabsf(nx) > 1e-6 {
                let (p, q) = ((-1.0 - ny * dy) / nx, (1.0 - ny * dy) / nx);
                if p < q { mark(p, q) } else { mark(q, p) }
            } else if libm::fabsf(ny * dy) < 1.0 {
                mark(-half - 1.0, half + 1.0);
            }
        }
        // Runs: a pixel near an edge on its own, else the run to the next one at its first pixel's value.
        let row = y as usize * width;
        let mut x = xa;
        while x <= xb {
            let dx = x as f32 + 0.5 - cxf;
            if near[x] {
                let l = exact(dx, dy);
                if l >> 48 != 0 {
                    px[row + x] = under(px[row + x], l);
                }
                x += 1;
                continue;
            }
            let mut e = x;
            while e + 1 <= xb && !near[e + 1] {
                e += 1;
            }
            let l = exact(dx, dy);
            if l >> 48 != 0 {
                for p in &mut px[row + x..=row + e] {
                    *p = under(*p, l);
                }
            }
            x = e + 1;
        }
        tx0 = tx0.min(xa as i64);
        tx1 = tx1.max(xb as i64);
    }
    if tx0 > tx1 {
        return Rect::EMPTY;
    }
    Rect { x0: tx0 as i32, y0: y0 as i32, x1: tx1 as i32 + 1, y1: y1 as i32 + 1 }
}

/// The ferros mark while something moves (owner, 2026-11-07: "a simple distance squared check to determine the ring and then a segment based on angle instead of rendering and filling shapes"; no antialiasing in motion), as `logo` places it. Each row is cut at its breakpoints — where it crosses each piece's arcs (a root per arc a row, from the squared radii) and each piece's sides (lines through the centre: one division a side a row, no angle) — and each run between two breakpoints is one piece's colour throughout, found once at its middle by the squared distance and the sides' signs, and filled straight under what is there (the pieces are opaque). The mark at rest is `logo`'s, antialiased.
pub fn logo_hard(px: &mut [Px], width: usize, height: usize, cx: i64, cy: i64, r: i64, c: i64, s: i64, lit: bool) -> Rect {
    const N: usize = LOGO.len();
    if r < 256 {
        return Rect::EMPTY;
    }
    let (rf, cxf, cyf) = (r as f32 * (1.0 / 256.0), cx as f32 * (1.0 / 256.0), cy as f32 * (1.0 / 256.0));
    let (cf, sf) = (c as f32 * (1.0 / 16384.0), s as f32 * (1.0 / 16384.0));
    let turn = |nx: f32, ny: f32| (cf * nx - sf * ny, sf * nx + cf * ny);
    // Each piece in screen pixels: its squared radii, its sides' inward normals turned onto the screen, whether it goes all the way round, whether its sides are a union (over half a turn), and its colour as an opaque pixel.
    let mut pieces = [(0f32, 0f32, (0f32, 0f32), (0f32, 0f32), false, false, [0u64; 2]); N];
    let mut bps_r = [0f32; 2 * N];
    let mut nr = 0;
    let mut sides = [(0f32, 0f32); 2 * N];
    let mut ns = 0;
    for (k, &(inner, outer, start, sweep, colour, grey)) in LOGO.iter().enumerate() {
        let (a0, a1) = (start * core::f32::consts::TAU, (start + sweep) * core::f32::consts::TAU);
        let n0 = turn(-libm::sinf(a0), libm::cosf(a0));
        let n1 = turn(libm::sinf(a1), -libm::cosf(a1));
        let col = if lit { colour } else { [grey; 3] };
        let p = ALPHA | col[0] as u64 | (col[1] as u64) << 16 | (col[2] as u64) << 32;
        let (rin, rout) = (inner * rf, outer * rf);
        pieces[k] = (rin * rin, rout * rout, n0, n1, sweep >= 1.0, sweep > 0.5, [p, p]);
        for rr in [rin, rout] {
            if rr > 0.0 && !bps_r[..nr].iter().any(|&q| libm::fabsf(q - rr) < 1e-3) {
                bps_r[nr] = rr;
                nr += 1;
            }
        }
        if sweep < 1.0 {
            for n in [n0, n1] {
                if !sides[..ns].iter().any(|&q| libm::fabsf(q.0 - n.0) + libm::fabsf(q.1 - n.1) < 1e-3 || libm::fabsf(q.0 + n.0) + libm::fabsf(q.1 + n.1) < 1e-3) {
                    sides[ns] = n;
                    ns += 1;
                }
            }
        }
    }
    // The piece a point (offset from the centre) is in, by its squared distance and its sides' signs.
    let at = |dx: f32, dy: f32| -> Option<[u64; 2]> {
        let d2 = dx * dx + dy * dy;
        for &(ri2, ro2, n0, n1, whole, union, p) in &pieces {
            if d2 < ri2 || d2 >= ro2 {
                continue;
            }
            if !whole {
                let (s0, s1) = (n0.0 * dx + n0.1 * dy >= 0.0, n1.0 * dx + n1.1 * dy >= 0.0);
                if !(if union { s0 || s1 } else { s0 && s1 }) {
                    continue;
                }
            }
            return Some(p);
        }
        None
    };
    // WHY the bounds: the disc follows the map's camera and is often partly off the screen (or the clip); its rows and columns are cut to them so no index lands outside the canvas.
    let (bx0, by0, bx1, by1) = bounds(width, height);
    let (y0, y1) = (((cyf - rf - 1.0) as i64).max(by0), ((cyf + rf + 1.0) as i64).min(by1 - 1));
    let (mut tx0, mut tx1) = (i64::MAX, i64::MIN);
    for y in y0..=y1 {
        let dy = y as f32 + 0.5 - cyf;
        if dy * dy >= rf * rf {
            continue;
        }
        // The breakpoints on this row, as offsets from the centre: the arcs' crossings and the sides'.
        let mut b = [0f32; 4 * N + 2];
        let mut nb = 0;
        for &rr in &bps_r[..nr] {
            let q = rr * rr - dy * dy;
            if q > 0.0 {
                let h = libm::sqrtf(q);
                b[nb] = -h;
                b[nb + 1] = h;
                nb += 2;
            }
        }
        for &(nx, ny) in &sides[..ns] {
            // The line nx·dx + ny·dy = 0 crosses the row at dx = −ny·dy / nx; one along the row (nx near 0) crosses it nowhere that matters.
            if libm::fabsf(nx) > 1e-6 {
                b[nb] = -ny * dy / nx;
                nb += 1;
            }
        }
        // In order: a short list, by insertion.
        for i in 1..nb {
            let v = b[i];
            let mut j = i;
            while j > 0 && b[j - 1] > v {
                b[j] = b[j - 1];
                j -= 1;
            }
            b[j] = v;
        }
        let row = y as usize * width;
        // Never empty: the outermost arc is the disc's own, crossed by every row inside it.
        for i in 1..nb {
            let (lo, hi) = (b[i - 1], b[i]);
            let Some(p) = at((lo + hi) * 0.5, dy) else { continue };
            // The run's pixels: those whose centres fall in [lo, hi), cut to the bounds.
            let (xa, xb) = ((libm::ceilf(cxf + lo - 0.5) as i64).max(bx0), (libm::ceilf(cxf + hi - 0.5) as i64).min(bx1));
            if xa >= xb {
                continue;
            }
            let run = &mut px[row + xa as usize..row + xb as usize];
            let n2 = run.len() & !1;
            let mut k = 0;
            while k < n2 {
                unsafe { crate::neon::under2(run.as_mut_ptr().add(k), p.as_ptr()) };
                k += 2;
            }
            for q in &mut run[n2..] {
                *q = under(*q, p[0]);
            }
            tx0 = tx0.min(xa);
            tx1 = tx1.max(xb);
        }
    }
    if tx0 > tx1 {
        return Rect::EMPTY;
    }
    Rect { x0: tx0 as i32, y0: y0 as i32, x1: tx1 as i32, y1: y1 as i32 + 1 }
}

/// A disc while something moves (owner, 2026-11-07: no antialiasing in motion): every pixel whose centre is inside the radius, solid, under what is there, each row one run (its reach from one root); the disc at rest is `disc`'s, antialiased. Q8 pixels as `disc`.
pub fn disc_hard(px: &mut [Px], width: usize, height: usize, cx: i64, cy: i64, r: i64, colour: u32) -> Rect {
    if r < 128 {
        return Rect::EMPTY;
    }
    let (bx0, by0, bx1, by1) = bounds(width, height);
    let p = [opaque(colour); 2];
    let (rf, cxf, cyf) = (r as f32 * (1.0 / 256.0), cx as f32 * (1.0 / 256.0), cy as f32 * (1.0 / 256.0));
    // WHY the bounds: the disc follows the camera and is often partly off the screen (or the clip); its rows and columns are cut to them.
    let (y0, y1) = (((cyf - rf) as i64 - 1).max(by0), ((cyf + rf) as i64 + 1).min(by1 - 1));
    for y in y0..=y1 {
        let dy = y as f32 + 0.5 - cyf;
        let q = rf * rf - dy * dy;
        if q <= 0.0 {
            continue;
        }
        let h = libm::sqrtf(q);
        // The pixels whose centres fall in [cx − h, cx + h].
        let (xa, xb) = ((libm::ceilf(cxf - h - 0.5) as i64).max(bx0), (libm::floorf(cxf + h - 0.5) as i64 + 1).min(bx1));
        if xa >= xb {
            continue;
        }
        let run = &mut px[y as usize * width + xa as usize..y as usize * width + xb as usize];
        let n2 = run.len() & !1;
        let mut k = 0;
        while k < n2 {
            unsafe { crate::neon::under2(run.as_mut_ptr().add(k), p.as_ptr()) };
            k += 2;
        }
        for q in &mut run[n2..] {
            *q = under(*q, p[0]);
        }
    }
    Rect { x0: ((cx - r) >> 8) as i32 - 2, y0: ((cy - r) >> 8) as i32 - 2, x1: ((cx + r) >> 8) as i32 + 3, y1: ((cy + r) >> 8) as i32 + 3 }.clip(width, height)
}
