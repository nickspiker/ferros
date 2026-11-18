//! A checked list drawn into a circle, front to back (the display's side).
//!
//! The display hands its pixels as a [`Target`]: one call per covered pixel with the coverage and the colour, which it puts under what is already there (fluor's `under`), so the list's first op ends up in front.
//! Every shape is a signed distance turned into coverage by one C¹ edge (`edge`), and every op is multiplied by the circle's own edge, so nothing reaches past the circle and its rim is antialiased like any circle's.
//! Coordinates arrive in Q14 of the radius and are drawn at the circle's size and turn ([`Geom`]): the same list is sharp at a few pixels and at the whole screen.

use crate::{curve_at, fmt, ops, Colour, Curve, Op, ONE, OSC};
use libm::{atan2f, cosf, floorf, sinf, sqrtf};

/// Where the circle is on the target: its centre and radius in pixels, and its turn (cos, sin), positive sin clockwise, as the display turns its icons and the clock.
#[derive(Clone, Copy, Debug)]
pub struct Geom {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    pub cos: f32,
    pub sin: f32,
}

/// What the display knows that a list does not: now (Eagle oscillations), the zone's offset the person's base shows (seconds east of UTC; ferros_sys::zone) and the base itself (10, 12 or 16).
#[derive(Clone, Copy, Debug)]
pub struct Ctx {
    pub now: i64,
    pub zone: i64,
    pub base: u8,
}

/// The pixels a list is drawn into.
pub trait Target {
    /// The pixel rectangle to draw in, half-open (x0, y0, x1, y1): the target's own size, or the region being redrawn.
    fn clip(&self) -> (i32, i32, i32, i32);
    /// Colour `c` at coverage `cov` (0 to 1, times the colour's own opacity) under pixel (x, y).
    fn under(&mut self, x: i32, y: i32, cov: f32, c: Colour);
    /// Words centred on (cx, cy) in pixels, `px` the glyphs' height, turned by `turn`, under what is there.
    fn text(&mut self, cx: f32, cy: f32, px: f32, turn: (f32, f32), c: Colour, s: &str);
}

/// Coverage from a signed distance d in pixels (negative inside): smoothstep across the pixel straddling the edge, 0 outside and 1 inside.
/// C¹ everywhere (its slope is 0 at both joins, matching the flat parts), so an edge that moves by a fraction of a pixel changes its pixels smoothly (AGENT.md "GUI Continuity").
/// The branches are the function's own pieces, each proven in range by the one before it, not a clamp.
#[inline]
pub fn edge(d: f32) -> f32 {
    let t = 0.5 - d;
    if t <= 0. {
        0.
    } else if t >= 1. {
        1.
    } else {
        t * t * (3. - 2. * t)
    }
}

impl Geom {
    /// Q14 radius units to pixels.
    fn len(&self, q: f32) -> f32 {
        q * self.r / ONE as f32
    }
    /// A point in the circle's units to the target's pixels.
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let (lx, ly) = (self.len(x), self.len(y));
        (self.cx + lx * self.cos - ly * self.sin, self.cy + lx * self.sin + ly * self.cos)
    }
}

/// Every pixel within `reach` pixels of the local point (lx, ly) and inside the circle and the clip: `f` gives the coverage of each (local pixel coordinates, the circle's own axes) and the target takes it under.
fn shade(t: &mut impl Target, g: &Geom, lx: f32, ly: f32, reach: f32, mut f: impl FnMut(&mut dyn FnMut(f32, Colour), f32, f32)) {
    let (sx, sy) = (g.cx + lx * g.cos - ly * g.sin, g.cy + lx * g.sin + ly * g.cos);
    let (cx0, cy0, cx1, cy1) = t.clip();
    // The box is the shape's reach and the circle's, whichever is tighter on each side, inside the clip: max and min intersect rectangles here, they constrain no value that is drawn.
    let x0 = (floorf(sx - reach) as i32).max(floorf(g.cx - g.r - 1.) as i32).max(cx0);
    let x1 = (floorf(sx + reach) as i32 + 2).min(floorf(g.cx + g.r) as i32 + 2).min(cx1);
    let y0 = (floorf(sy - reach) as i32).max(floorf(g.cy - g.r - 1.) as i32).max(cy0);
    let y1 = (floorf(sy + reach) as i32 + 2).min(floorf(g.cy + g.r) as i32 + 2).min(cy1);
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = (x as f32 + 0.5 - g.cx, y as f32 + 0.5 - g.cy);
            let mask = edge(sqrtf(dx * dx + dy * dy) - g.r);
            if mask <= 0. {
                continue;
            }
            let (px, py) = (dx * g.cos + dy * g.sin, -dx * g.sin + dy * g.cos);
            let mut put = |cov: f32, c: Colour| {
                let cov = cov * mask * c.opacity() as f32 * (1. / 255.);
                if cov > 0. {
                    t.under(x, y, cov, c);
                }
            };
            f(&mut put, px, py);
        }
    }
}

/// The angle of local point (x, y) as a turn clockwise from the circle's up, in [0, 1).
fn turn_of(x: f32, y: f32) -> f32 {
    let a = atan2f(x, -y) * (1. / (2. * core::f32::consts::PI));
    a - floorf(a)
}

/// The local point at turn `a` on the circle of radius r about (cx, cy).
fn on_circle(cx: f32, cy: f32, r: f32, a: f32) -> (f32, f32) {
    let th = a * 2. * core::f32::consts::PI;
    (cx + r * sinf(th), cy - r * cosf(th))
}

/// A band of half-width hw along the circle of radius rm about (cx, cy), from turn `from` for `sweep` turns, round ended.
fn arc(t: &mut impl Target, g: &Geom, cx: f32, cy: f32, rm: f32, hw: f32, from: f32, sweep: f32, c: Colour) {
    if sweep <= 0. || hw <= 0. {
        return;
    }
    let ends = (on_circle(cx, cy, rm, from), on_circle(cx, cy, rm, from + sweep));
    shade(t, g, cx, cy, rm + hw + 1., |put, x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let d = sqrtf(dx * dx + dy * dy);
        let rel = turn_of(dx, dy) - from;
        let rel = rel - floorf(rel);
        let dist = if rel <= sweep {
            // The band: the absolute value folds both sides of the centre line, whose kink sits where coverage is already 1 for any band wider than a pixel.
            libm::fabsf(d - rm) - hw
        } else {
            // Past either end: the round cap, the nearer end's disc (the smaller of two distances is the union of the two caps, a shape, not a constrained value).
            let e = |p: (f32, f32)| sqrtf((x - p.0) * (x - p.0) + (y - p.1) * (y - p.1));
            e(ends.0).min(e(ends.1)) - hw
        };
        put(edge(dist), c);
    });
}

/// A Q16 turn as (cos, sin), clockwise on the glass (y down).
fn turn_cs(turn: u16) -> (f32, f32) {
    let th = turn as f32 * (2. * core::f32::consts::PI / 65536.);
    (cosf(th), sinf(th))
}

/// The point on curve `cv` (in pixels, local) at s in [0, 1] of its span, and its slope dy/dx there.
fn curve_point(g: &Geom, cv: &Curve, s: f32) -> (f32, f32, f32) {
    let (x0, x1) = (g.len(cv.x0 as f32), g.len(cv.x1 as f32));
    let (yb, yt) = (g.len(cv.yb as f32), g.len(cv.yt as f32));
    let (v, dv) = cv.at(s);
    let k = (yt - yb) / (cv.vhi as f32 - cv.vlo as f32);
    (x0 + s * (x1 - x0), yb + (v - cv.vlo as f32) * k, dv * k / (x1 - x0))
}

fn curve(t: &mut impl Target, g: &Geom, cv: &Curve) {
    let (x0, x1) = (g.len(cv.x0 as f32), g.len(cv.x1 as f32));
    let (yb, yt) = (g.len(cv.yb as f32), g.len(cv.yt as f32));
    let hw = g.len(cv.width as f32) * 0.5;
    let (mx, my) = ((x0 + x1) * 0.5, (yb + yt) * 0.5);
    let (hx, hy) = ((x1 - x0) * 0.5, libm::fabsf(yb - yt) * 0.5);
    // The curve may leave its box between samples (a Hermite piece overshoots), so the reach is the whole circle's height: the circle's mask bounds it anyway.
    let reach = sqrtf(hx * hx + hy * hy) + hw + g.r;
    let (tc, ts) = turn_cs(cv.turn);
    // The box's centre in the circle's frame (the curve's frame turned clockwise by `turn`).
    let (mx, my) = (mx * tc - my * ts, mx * ts + my * tc);
    let down = if yb > yt { 1. } else { -1. };
    shade(t, g, mx, my, reach, |put, x, y| {
        // The pixel in the curve's own frame: turned back.
        let (x, y) = (x * tc + y * ts, -x * ts + y * tc);
        let s = (x - x0) / (x1 - x0);
        if !(0. ..=1.).contains(&s) {
            return;
        }
        let (_, cy, slope) = curve_point(g, cv, s);
        // Distance across the line, from the vertical gap and the slope (exact for a straight piece, close for a curved one well inside a pixel).
        let across = (y - cy) / sqrtf(1. + slope * slope);
        // Front first: the line, then the fill behind it.
        if hw > 0. {
            put(edge(libm::fabsf(across) - hw), cv.line);
        }
        if let Some(f) = cv.fill {
            // Inside the fill: past the line toward the baseline, and not past the baseline; the product of two edges is the intersection, C¹ as each is.
            put(edge(-across * down) * edge((y - yb) * down), f);
        }
    });
}

/// The text a typed value shows, put as text.
fn words(t: &mut impl Target, g: &Geom, cx: i16, cy: i16, size: u16, c: Colour, s: &str) {
    let (px, py) = g.point(cx as f32, cy as f32);
    t.text(px, py, g.len(size as f32), (g.cos, g.sin), c, s);
}

/// The fraction of a period of `period_s` seconds elapsed at `now` since `t0`, in [0, 1).
fn phase(now: i64, t0: i64, period_s: u32) -> f32 {
    let p = period_s as i64 * OSC;
    (now - t0).rem_euclid(p) as f32 / p as f32
}

/// Draw a checked list (`crate::check`) into the circle `g`, front to back.
pub fn draw(list: &[u8], g: &Geom, ctx: &Ctx, t: &mut impl Target) {
    for op in ops(list) {
        match op {
            Op::Disc { cx, cy, r, c } => {
                let (cx, cy, r) = (g.len(cx as f32), g.len(cy as f32), g.len(r as f32));
                shade(t, g, cx, cy, r + 1., |put, x, y| put(edge(sqrtf((x - cx) * (x - cx) + (y - cy) * (y - cy)) - r), c));
            }
            Op::Ring { cx, cy, outer, inner, c } => {
                let (cx, cy) = (g.len(cx as f32), g.len(cy as f32));
                let (ro, ri) = (g.len(outer as f32), g.len(inner as f32));
                let (rm, hw) = ((ro + ri) * 0.5, (ro - ri) * 0.5);
                shade(t, g, cx, cy, ro + 1., |put, x, y| put(edge(libm::fabsf(sqrtf((x - cx) * (x - cx) + (y - cy) * (y - cy)) - rm) - hw), c));
            }
            Op::Arc { cx, cy, r, width, from, sweep, c } => {
                arc(t, g, g.len(cx as f32), g.len(cy as f32), g.len(r as f32), g.len(width as f32) * 0.5, from as f32 / 65536., sweep as f32 / 65536., c);
            }
            Op::Capsule { ax, ay, bx, by, r, c } => {
                let (ax, ay, bx, by, r) = (g.len(ax as f32), g.len(ay as f32), g.len(bx as f32), g.len(by as f32), g.len(r as f32));
                let (ex, ey) = (bx - ax, by - ay);
                let l2 = ex * ex + ey * ey;
                let half = sqrtf(l2) * 0.5;
                shade(t, g, (ax + bx) * 0.5, (ay + by) * 0.5, half + r + 1., |put, x, y| {
                    // The nearest point of the segment: its parameter limited to the segment's two ends, which is what a segment is, not a value constrained after the fact.
                    let u = if l2 > 0. { (((x - ax) * ex + (y - ay) * ey) / l2).clamp(0., 1.) } else { 0. };
                    let (qx, qy) = (ax + u * ex - x, ay + u * ey - y);
                    put(edge(sqrtf(qx * qx + qy * qy) - r), c);
                });
            }
            Op::Curve(cv) => curve(t, g, &cv),
            Op::Text { cx, cy, size, c, s } => words(t, g, cx, cy, size, c, s),
            Op::Sweep { cx, cy, r, width, from, t0, period, c } => {
                arc(t, g, g.len(cx as f32), g.len(cy as f32), g.len(r as f32), g.len(width as f32) * 0.5, from as f32 / 65536., phase(ctx.now, t0, period), c);
            }
            Op::Marker { curve: i, t0, t1, r, c } => {
                let Some(cv) = curve_at(list, i) else { continue };
                if ctx.now < t0 || ctx.now > t1 {
                    continue;
                }
                let s = ((ctx.now - t0) as f64 / (t1 - t0) as f64) as f32;
                let (mx, my, _) = curve_point(g, &cv, s);
                let (tc, ts) = turn_cs(cv.turn);
                let (mx, my) = (mx * tc - my * ts, mx * ts + my * tc);
                let r = g.len(r as f32);
                shade(t, g, mx, my, r + 1., |put, x, y| put(edge(sqrtf((x - mx) * (x - mx) + (y - my) * (y - my)) - r), c));
            }
            Op::Moment { cx, cy, size, c, t: when, places } => {
                let mut b = [0u8; fmt::OUT];
                let s = fmt::moment(when, ctx.zone, ctx.base, places, &mut b);
                words(t, g, cx, cy, size, c, s);
            }
            Op::Number { cx, cy, size, c, value, places, unit } => {
                let mut b = [0u8; fmt::OUT];
                let s = fmt::number(value, places, unit, ctx.base, &mut b);
                words(t, g, cx, cy, size, c, s);
            }
        }
    }
}

/// The next moment after `now` at which the list's time-anchored ops move by half a pixel at radius `r_px` (a sweep's end, a marker along its curve), or None when nothing in it moves on its own: the display redraws the circle then and only then, so a puck's face moves with no puck waking and no frame drawn for nothing.
pub fn next_change(list: &[u8], r_px: f32, now: i64) -> Option<i64> {
    // WHY the floors of a pixel (here and the marker's span): a sweep of radius 0 or a circle drawn below a pixel would divide by zero and ask for a redraw at once, forever; under a pixel nothing it shows moves anyway.
    let half_px_turn = |radius_px: f32| -> f64 { 1. / (4. * core::f64::consts::PI * radius_px.max(1.) as f64) };
    let mut best: Option<i64> = None;
    let mut take = |at: i64| best = Some(best.map_or(at, |b: i64| b.min(at)));
    for op in ops(list) {
        match op {
            Op::Sweep { r, period, .. } => {
                let step = (period as f64 * OSC as f64 * half_px_turn(r as f32 * r_px / ONE as f32)) as i64;
                // WHY the floor of one second: a sweep on a circle a few pixels wide would ask for a redraw faster than any frame; a second is the finest the moments it serves (the clock's) need.
                take(now + step.max(OSC));
            }
            Op::Marker { curve: i, t0, t1, .. } => {
                let Some(cv) = curve_at(list, i) else { continue };
                if now >= t1 {
                    continue;
                }
                if now < t0 {
                    take(t0);
                    continue;
                }
                let span_px = (cv.x1 as f32 - cv.x0 as f32) * r_px / ONE as f32;
                let step = ((t1 - t0) as f64 / (2. * span_px.max(1.) as f64)) as i64;
                take(now + step.max(OSC));
            }
            _ => {}
        }
    }
    best
}
