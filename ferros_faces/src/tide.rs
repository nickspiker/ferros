//! The tide puck's face (PUCKS.md "The tide puck"), the owner's tide widget on the deck (owner, 2026-11-24: "the tide one, the inksurf/tide android apk/widget is what the tides should look like on ferros"): inksurf's tide-widget/src/render.rs ported to the puck's circle, drawn per pixel into its square as the clock draws its own.
//!
//! What it shows, both at rest and in focus (the widget draws one chart at every size, and so does this):
//! - **The sky and the water over time**, now at the centre: a day across the middle half of the width, tapering to half a year either side at the rim (the widget's `TimeAxis`).
//!   Each column of the face averages its span of time in linear light: near now a crisp chart, months out the tide's density and the curves' glow.
//!   The sun up lights the sky's red and green, the moon up its blue: yellow day, white day with the moon up, blue moonlit night, black night.
//!   The water is cyan under its waterline, on the station's fixed scale (mean sea level plus and less the sum of its amplitudes, the most its tide can reach either way), so a day's water is read against the station's range.
//! - **The sun's and the moon's altitudes** as curves, the horizon on the centreline (which is also mid-tide), each at its year's highest at the top.
//! - **The centreline**, every channel flipped.
//! - **The now bar**, white with black edges, and now's time straddling it in the person's base.
//! - **Each high and low** within half a day of now: a bar flipping green and blue, its time straddling it, a high's in the lower third and a low's in the upper (each away from the water's crest or trough, as the widget sets them).
//! - **Sunrise and sunset** within half a day: their times turned on the day and night seam, from the circle's top (sunrise reads up, sunset down).
//! - **The hours** as full-height lines, a neutral grey of their own (owner, 2026-11-24: "since it's round, I'd put the hour ticks as vertical lines using whatever channel and opacity make sense rather than single ticks"); local midnight twice as wide and twice as opaque.
//! - **In focus** (the circle at least half the panel across), the station's name and its distance besides, below the now time.
//!
//! Where it departs from the widget, and why:
//! - **Sizes** are fractions of the circle (`hair`, `em`), never pixels: the widget's every line is one pixel at any size.
//! - **Edges** are ferros's C¹ edge (ferros_puck::draw::edge) on every line, band and waterline, so anything that moves with now changes its pixels smoothly; the widget snaps its bars to pixel columns.
//!   The day and night seam stays as the widget has it: a hard change of the sky's colour at sunrise and sunset, antialiased only by the column's average.
//! - **Columns are sampled at fixed instants**: stratum centres near now, a year of samples on an absolute 2^10 s grid further out, so a redraw shows the same glow (the widget jitters its samples per redraw).
//! - **The hours** are full-height lines (the owner's refinement above) where the widget has ticks at the top and bottom edges.
//! - **Times** are in the person's base and zone (memory time_zones_decided): dozenal as the widget's dozenal edition (the day's twelfth and its twelfth, the bar between, a trailing zil dropped); decimal as its hourly edition (HH:MM on the 24-hour clock, the colon on the bar), not the clock's h:mm am or pm, which is too wide for two times half a day apart on a circle; hex the seconds since midnight to G#100 s, the bar before the last place.
//!   The zone is the one in force now across the whole chart.
//! - **Colours**: the widget's solid primaries are VSF RGB primaries here, averaged in linear light, carried as gamma-2 codes and drawn as g2t (memory feedback_vsf_rgb_only); its sRGB curve has no place here.
//!
//! Compositing is front to back with `under` alone, as the widget does it: the labels first (each glyph pixel the fill beneath it with its ink's channels flipped), then the now bar, the high and low bars, the hours, and the sky with everything drawn into it last.

use alloc::vec;
use alloc::vec::Vec;
use ferros_glyphs::Glyphs;
use ferros_puck::draw::edge;
use ferros_puck::fmt;
use ferros_puck::square::{cover, under};
use ferros_sky::{Body, Crossing, Place, DAY, EAGLE_EPOCH_UNIX_SECS, SUNRISE};
use ferros_tide::{Extreme, Frame, Tide};
use libm::{floor, floorf, sqrt, sqrtf, tanhf};

/// The station's fixed scale: (lowest, highest) its tide can reach, metres above its datum.
pub fn scale(tide: &Tide) -> (f32, f32) {
    let reach: f64 = tide.amp.iter().sum();
    ((tide.z0 - reach) as f32, (tide.z0 + reach) as f32)
}

/// Half a day either side of now across the middle half of the width, half a year either side at the rim (the widget's).
const CENTRE_HALF: f64 = DAY / 2.;
const EDGE: f64 = 182.5 * DAY;

/// The horizontal time axis (the widget's `TimeAxis`): with u = −1..1 across the face, t − now = k·u / √(c − u²), nearly linear around now and steepening like a circle's edge toward the rim; t = ±half a day at u = ±½ and ±half a year at u = ±1 fix c and k in closed form.
struct Axis {
    now: f64,
    half: f64,
    k: f64,
    c: f64,
}

impl Axis {
    fn new(now: f64, d: usize) -> Axis {
        let r2 = (EDGE / CENTRE_HALF) * (EDGE / CENTRE_HALF);
        let c = (r2 - 1.) / (r2 - 4.);
        Axis { now, half: d as f64 * 0.5, k: 2. * CENTRE_HALF * sqrt(c - 0.25), c }
    }
    /// The time at face x (pixels from the left, 0 to d).
    fn t(&self, x: f64) -> f64 {
        let u = (x - self.half) / self.half;
        self.now + self.k * u / sqrt(self.c - u * u)
    }
    /// The face x of time t: the exact inverse, u = s·√(c / (1 + s²)) with s = (t − now) / k.
    fn x(&self, t: f64) -> f64 {
        let s = (t - self.now) / self.k;
        self.half + s * sqrt(self.c / (1. + s * s)) * self.half
    }
    /// Pixels per second at time t: dx/dt = (half / k)·√c·(1 + s²)^(−3/2).
    fn rate(&self, t: f64) -> f64 {
        let s = (t - self.now) / self.k;
        let q = 1. + s * s;
        self.half / self.k * sqrt(self.c) / (q * sqrt(q))
    }
    /// Seconds a pixel spans at the centre.
    fn centre_px(&self) -> f64 {
        self.k / (self.half * sqrt(self.c))
    }
}

/// The far columns' samples: a year either side of now on an absolute grid of 2^10 s (17 minutes), each the tide's height and the sun's and moon's altitudes, made once per station and place and again as now moves on (`fresh`).
pub struct Year {
    first: f64,
    h: Vec<f32>,
    sun: Vec<f32>,
    moon: Vec<f32>,
    /// Each body's highest over the grid's year, the top of its curve.
    sun_max: f32,
    moon_max: f32,
    key: (u64, u64, u64),
}

/// The grid's step and reach either side of its centre, as log2 seconds: 2^24 s (194 days) covers the axis's half year with 11 days to spare.
const STEP_LOG2: i32 = 10;
const REACH_LOG2: i32 = 24;
/// How far now may move from the grid's centre before it is made again (2^19 s, 6 days: inside the spare reach).
const RECENTRE_LOG2: i32 = 19;
/// Samples that share one tide frame (2^6, 18 hours: a frame's arguments run on linearly, exact for days).
const FRAME_LOG2: i32 = 6;

/// What a year's samples depend on: the tide (its mean level and its amplitudes) and the place.
fn year_key(tide: &Tide, place: Place) -> (u64, u64, u64) {
    let amp: f64 = tide.amp.iter().zip(tide.phase.iter()).map(|(a, p)| a * (1. + p)).sum();
    ((tide.z0 + amp).to_bits(), place.lat.to_bits(), place.lon.to_bits())
}

impl Year {
    fn build(tide: &Tide, place: Place, now: f64) -> Year {
        let step = (1u64 << STEP_LOG2) as f64;
        let first = floor((now - (1u64 << REACH_LOG2) as f64) / step) * step;
        let n = (1usize << (REACH_LOG2 + 1 - STEP_LOG2)) + 1;
        let (mut h, mut sun, mut moon) = (vec![0f32; n], vec![0f32; n], vec![0f32; n]);
        let mut fr = Frame::at(first);
        for i in 0..n {
            let t = first + i as f64 * step;
            if i & ((1 << FRAME_LOG2) - 1) == 0 {
                fr = Frame::at(t + step * (1 << (FRAME_LOG2 - 1)) as f64);
            }
            h[i] = tide.height_in(&fr, t) as f32;
            sun[i] = ferros_sky::altitude(Body::Sun, t, place) as f32;
            moon[i] = ferros_sky::altitude(Body::Moon, t, place) as f32;
        }
        let top = |v: &[f32]| v.iter().fold(0f32, |m, &a| if a > m { a } else { m });
        Year { first, sun_max: top(&sun), moon_max: top(&moon), h, sun, moon, key: year_key(tide, place) }
    }
    /// Whether this year still serves the station, the place and now.
    fn fresh(&self, tide: &Tide, place: Place, now: f64) -> bool {
        let centre = self.first + (1u64 << REACH_LOG2) as f64;
        self.key == year_key(tide, place) && libm::fabs(now - centre) < (1u64 << RECENTRE_LOG2) as f64
    }
}

/// What the puck keeps between squares: the year's samples and the face's two backgrounds (each a d × d grid of 10-bit gamma-2 codes, R in bits 20-29), grown as the circle asks.
pub struct Cache {
    year: Option<Year>,
    fill: Vec<u32>,
    full: Vec<u32>,
}

impl Cache {
    pub const fn new() -> Cache {
        Cache { year: None, fill: Vec::new(), full: Vec::new() }
    }
}

impl Default for Cache {
    fn default() -> Cache {
        Cache::new()
    }
}

/// What the face is drawn from.
pub struct Chart<'a> {
    pub tide: &'a Tide,
    /// The station's fixed scale (`scale`).
    pub scale: (f32, f32),
    /// The place: the sun and the moon.
    pub place: Place,
    /// Now, Eagle seconds.
    pub now: f64,
    /// The zone's offset the person's base shows (seconds east, ferros_sys::zone) and the base (10, 12 or 16).
    pub zone: i64,
    pub base: u8,
    /// In focus: the station's name and how far it is (metres).
    pub station: Option<(&'a str, f64)>,
}

// The palette: the widget's solid primaries as VSF RGB, linear light (R, G, B).
const WATER: [f32; 3] = [0., 1., 1.];
const SKY_SUN: [f32; 3] = [1., 1., 0.];
const SKY_MOON: [f32; 3] = [0., 0., 1.];
const SUN: [f32; 3] = [1., 1., 0.];
const MOON: [f32; 3] = [0., 0., 1.];
const STROKE: [f32; 3] = [0., 0., 0.];
/// The channels each kind of words flips in the fill beneath them (R, G, B): now every channel, the tide's times green and blue, the sun's red and green.
const NOW_INK: [bool; 3] = [true, true, true];
const TIDE_INK: [bool; 3] = [false, true, true];
const SUN_INK: [bool; 3] = [true, true, false];
/// The hours' grey: the middle of the code range, the one colour that shows on the black night, the white sky and every primary between; an hour at 2^-2 opacity, midnight at 2^-1.
const HOUR_CODE: f32 = 0.5;
const HOUR_ALPHA_LOG2: i32 = 2;
const MIDNIGHT_ALPHA_LOG2: i32 = 1;

/// A line's width: 2^-9 of the circle's diameter (the widget's one pixel of its panel's 180 rows, a little lighter).
fn hair(d: usize) -> f32 {
    d as f32 * (1. / (1 << 9) as f32)
}
/// The words' size: 2^-4 of the diameter (the widget's 0.09 of its height, for a face as wide as it is tall).
fn em(d: usize) -> f32 {
    d as f32 * (1. / (1 << 4) as f32)
}

/// The coverage of the band [a, b] at the pixel centred on c, by ferros's C¹ edge on each side: exact in area, smooth as the band moves.
#[inline]
fn band(c: f32, a: f32, b: f32) -> f32 {
    edge(c - b) - edge(c - a)
}

/// A 10-bit gamma-2 code from linear light.
#[inline]
fn code10(l: f32) -> u32 {
    (sqrtf(l) * 1023. + 0.5) as u32
}
fn pack(l: [f32; 3]) -> u32 {
    code10(l[0]) << 20 | code10(l[1]) << 10 | code10(l[2])
}
fn unpack(p: u32) -> [f32; 3] {
    [(p >> 20 & 1023) as f32 * (1. / 1023.), (p >> 10 & 1023) as f32 * (1. / 1023.), (p & 1023) as f32 * (1. / 1023.)]
}

/// Darkness of a gamma-2 code s (white at 1): its g2t code 2^16·tanh(s), complemented (ferros_puck::square's `dark`, from a code of any precision).
#[inline]
fn dark(s: f32) -> u64 {
    65535 - (tanhf(s) * 65536.) as u64
}
/// Gamma-2 codes as an opaque G2t16 pixel.
#[inline]
fn opaque(s: [f32; 3]) -> u64 {
    0xFFFF << 48 | dark(s[0]) << 32 | dark(s[1]) << 16 | dark(s[2])
}
/// Codes with the channels `ink` names flipped (s to 1 − s: the widget's XOR, in code space as its was).
#[inline]
fn flip(s: [f32; 3], ink: [bool; 3]) -> [f32; 3] {
    core::array::from_fn(|k| if ink[k] { 1. - s[k] } else { s[k] })
}

/// A grid's gamma-2 codes at face point (x, y), bilinear between its texel centres (x + 0.5, y + 0.5).
fn sample(g: &[u32], d: usize, x: f32, y: f32) -> [f32; 3] {
    let (u, v) = (x - 0.5, y - 0.5);
    let (fu, fv) = (floorf(u), floorf(v));
    let (au, av) = (u - fu, v - fv);
    // WHY the bounds: a turned square's pixels near the rim land a pixel or two past the face's grid (the circle's edge trims them); the rim's texel stands in.
    let last = d as i32 - 1;
    let at = |i: i32, j: i32| unpack(g[j.clamp(0, last) as usize * d + i.clamp(0, last) as usize]);
    let (i, j) = (fu as i32, fv as i32);
    let (p00, p10, p01, p11) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1));
    core::array::from_fn(|k| (p00[k] * (1. - au) + p10[k] * au) * (1. - av) + (p01[k] * (1. - au) + p11[k] * au) * av)
}

/// One instant of a column: the waterline's row, the sky's colour, the sun's and moon's curves' rows.
#[derive(Clone, Copy)]
struct Slice {
    y: f32,
    sky: [f32; 3],
    sun_y: f32,
    moon_y: f32,
}

/// A column's backgrounds into `fill` (sky and water) and `full` (with the waterline, the curves and the centreline), each `d` rows of linear light: the average of its slices, each slice a one-pixel-wide picture of its instant (the widget's `column`), in linear light.
/// Within a slice the curves and the waterline go front to back over its sky and water: the sun's colour, its black edges, the moon's, its edges, the waterline's stroke, then the sky above the waterline and the water below it.
fn column(slices: &[Slice], d: usize, hair: f32, fill: &mut [[f32; 3]], full: &mut [[f32; 3]], sky_d: &mut [[f32; 3]], water_d: &mut [f32]) {
    for r in 0..d {
        fill[r] = [0.; 3];
        full[r] = [0.; 3];
    }
    for r in 0..=d {
        sky_d[r] = [0.; 3];
        water_d[r] = 0.;
    }
    let (rows, n) = (d as isize, slices.len() as f32);
    for s in slices {
        // The waterline's row: the sky whole above it, the water whole below, and that row the share of each its edge gives.
        let r0 = floorf(s.y) as isize;
        let sky_to = r0.clamp(0, rows) as usize;
        // WHY the clamps: a height past the station's scale (a nodal factor above one carries a constituent past its listed amplitude) puts the waterline above the face's top or below its bottom; the water then fills the column, or the sky does.
        for k in 0..3 {
            sky_d[0][k] += s.sky[k];
            sky_d[sky_to][k] -= s.sky[k];
        }
        let water_from = (r0 + 1).clamp(0, rows) as usize;
        water_d[water_from] += 1.;
        let base = |r: usize| -> [f32; 3] {
            let w = 1. - edge(r as f32 + 0.5 - s.y);
            core::array::from_fn(|k| w * WATER[k] + (1. - w) * s.sky[k])
        };
        // The waterline's own row, when it is on the face (the clamps' case above aside).
        if (0..rows).contains(&r0) {
            let b = base(r0 as usize);
            for k in 0..3 {
                fill[r0 as usize][k] += b[k];
                full[r0 as usize][k] += b[k];
            }
        }
        // The overlays, front first: (top, bottom, colour).
        let over: [(f32, f32, [f32; 3]); 5] = [
            (s.sun_y - hair, s.sun_y + hair, SUN),
            (s.sun_y - 2. * hair, s.sun_y + 2. * hair, STROKE),
            (s.moon_y - hair, s.moon_y + hair, MOON),
            (s.moon_y - 2. * hair, s.moon_y + 2. * hair, STROKE),
            (s.y - 0.5 * hair, s.y + 0.5 * hair, STROKE),
        ];
        // The rows any overlay reaches (an edge reaches half a pixel past its band), each band's rows a run.
        for (o, &(top, bottom, _)) in over.iter().enumerate() {
            // WHY the bounds: a curve at its year's highest, or a body far below the horizon, puts a band at or past the face's top or bottom row.
            let (from, to) = ((floorf(top - 1.) as isize + 1).max(0), (libm::ceilf(bottom) as isize).min(rows));
            for r in from..to {
                // A row two bands reach is done once, by the first of them that reaches it.
                if over[..o].iter().any(|&(t, b, _)| (floorf(t - 1.) as isize + 1) <= r && r < libm::ceilf(b) as isize) {
                    continue;
                }
                let c = r as f32 + 0.5;
                let b = base(r as usize);
                // Front to back: each band takes the room the ones before it left (`under`), the base the rest.
                let (mut acc, mut alpha) = ([0f32; 3], 0f32);
                for &(t, bo, ink) in &over {
                    let cov = band(c, t, bo);
                    if cov > 0. {
                        let take = (1. - alpha) * cov;
                        for k in 0..3 {
                            acc[k] += take * ink[k];
                        }
                        alpha += take;
                    }
                }
                for k in 0..3 {
                    // What the overlays change on this row: the delta over its base.
                    full[r as usize][k] += acc[k] + (1. - alpha) * b[k] - b[k];
                }
            }
        }
    }
    let (mut sky_run, mut water_run) = ([0f32; 3], 0f32);
    let inv = 1. / n;
    for r in 0..d {
        water_run += water_d[r];
        for k in 0..3 {
            sky_run[k] += sky_d[r][k];
            let whole = sky_run[k] + water_run * WATER[k];
            fill[r][k] = (fill[r][k] + whole) * inv;
            full[r][k] = (full[r][k] + whole) * inv;
        }
    }
}

/// A slice at instant t: the tide from frame `fr`, the sun and moon at the place.
fn slice_at(c: &Chart, fr: &Frame, t: f64, geo: &Geo, sun_max: f32, moon_max: f32) -> Slice {
    let h = c.tide.height_in(fr, t) as f32;
    let sun = ferros_sky::altitude(Body::Sun, t, c.place) as f32;
    let moon = ferros_sky::altitude(Body::Moon, t, c.place) as f32;
    geo.slice(h, sun, moon, sun_max, moon_max)
}

/// The face's vertical geometry: the diameter, the scale, the centreline.
struct Geo {
    d: f32,
    lo: f32,
    hi: f32,
    hair: f32,
}

impl Geo {
    fn slice(&self, h: f32, sun: f32, moon: f32, sun_max: f32, moon_max: f32) -> Slice {
        let mid = self.d * 0.5;
        let y = self.d * (1. - (h - self.lo) / (self.hi - self.lo));
        let up = |a: f32| if a > 0. { 1. } else { 0. };
        let (su, mu) = (if sun > SUNRISE as f32 { 1. } else { 0. }, up(moon));
        let sky = core::array::from_fn(|k| su * SKY_SUN[k] + mu * SKY_MOON[k]);
        let alt_y = |a: f32, max: f32| mid - a / max * (mid - self.hair);
        Slice { y, sky, sun_y: alt_y(sun, sun_max), moon_y: alt_y(moon, moon_max) }
    }
}

/// Columns spanning less than this are sampled directly (2^13 s, 2.3 hours), wider ones from the year's grid.
const DIRECT_LOG2: i32 = 13;
/// A direct column's samples: one per 2^9 s of its span, at least 2^3.
const SAMPLE_LOG2: i32 = 9;
const SAMPLES_MIN: usize = 1 << 3;

/// The local midnight at or before Eagle second t, in the zone.
fn midnight_before(t: f64, zone: i64) -> f64 {
    let local = t + (EAGLE_EPOCH_UNIX_SECS + zone) as f64;
    t - (local - floor(local / DAY) * DAY)
}

/// The step between the hour lines, seconds: an hour in decimal, a dozenal hour (the day's twelfth) in dozenal, G#1000 s in hex.
fn hour_step(base: u8) -> f64 {
    match base {
        12 => DAY / 12.,
        16 => (1 << 12) as f64,
        _ => 3600.,
    }
}

/// How often the now time's words change, seconds: the dozenal edition's second place (the day's 144th, 10 minutes), the decimal minute, the hex G#100 s.
pub(crate) fn word_step(base: u8) -> f64 {
    match base {
        12 => DAY / 144.,
        16 => (1 << 8) as f64,
        _ => 60.,
    }
}

/// How a time's words sit on its bar.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Split {
    /// One part, centred on the bar (a dozenal time whose twelfth is zil).
    Lone,
    /// The colon on the bar, the parts either side (decimal).
    Colon,
    /// The bar between the parts, a little air either side (dozenal, hex).
    Gap,
}

/// A time's words in the person's base as its bar splits them: dozenal the day's twelfth and its twelfth (a trailing zil dropped, as the widget's dozenal edition); decimal HH:MM on the 24-hour clock (the widget's hourly edition: the clock's h:mm and am or pm is half as wide again, and two lows half a day apart collided on a deck-sized circle); hex the seconds since midnight to G#100 s, the bar before the last place.
pub(crate) struct Time {
    pub(crate) a: Text,
    pub(crate) split: Split,
    pub(crate) b: Text,
}

/// A short run of words, held without a heap.
#[derive(Clone, Copy)]
pub(crate) struct Text {
    b: [u8; fmt::OUT],
    n: usize,
}

impl Text {
    const fn new() -> Text {
        Text { b: [0; fmt::OUT], n: 0 }
    }
    fn of(s: &str) -> Text {
        let mut t = Text::new();
        t.push_str(s);
        t
    }
    fn push(&mut self, c: char) {
        let mut tmp = [0u8; 4];
        for &x in c.encode_utf8(&mut tmp).as_bytes() {
            // WHY the bound: OUT holds every time written here (a dozen bytes at most) and a station's name is cut at it, never written past.
            if self.n < fmt::OUT {
                self.b[self.n] = x;
                self.n += 1;
            }
        }
    }
    fn push_str(&mut self, s: &str) {
        for c in s.chars() {
            self.push(c);
        }
    }
    pub(crate) fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
}

pub(crate) fn time_words(t: f64, zone: i64, base: u8) -> Time {
    let (mut a, mut b) = (Text::new(), Text::new());
    let sod = fmt::second_of_day(crate::osc(t), zone);
    let split = match base {
        12 => {
            let u = sod * 144 / 86400;
            a.push(fmt::digit((u / 12) as u32, 12));
            if u % 12 == 0 {
                Split::Lone
            } else {
                b.push(fmt::digit((u % 12) as u32, 12));
                Split::Gap
            }
        }
        16 => {
            let v = sod >> 8;
            let hi = v >> 4;
            if hi >= 16 {
                a.push(fmt::digit((hi >> 4) as u32, 16));
            }
            a.push(fmt::digit((hi & 0xF) as u32, 16));
            b.push(fmt::digit((v & 0xF) as u32, 16));
            Split::Gap
        }
        _ => {
            let (h, m) = (sod / 3600, sod / 60 % 60);
            for (into, v) in [(&mut a, h), (&mut b, m)] {
                into.push(fmt::digit((v / 10) as u32, 10));
                into.push(fmt::digit((v % 10) as u32, 10));
            }
            Split::Colon
        }
    };
    Time { a, split, b }
}

/// One run of words on the face: its centre (face pixels), its turn on the face (cos, sin), its size, the channels it flips.
struct Run {
    text: Text,
    cx: f32,
    cy: f32,
    turn: (f32, f32),
    size: f32,
    ink: [bool; 3],
}

/// A time's runs straddling the bar at face x `bar`, centred on row `cy`.
#[allow(clippy::too_many_arguments)]
fn straddle(runs: &mut Vec<Run>, tm: &Time, bar: f32, cy: f32, size: f32, air: f32, ink: [bool; 3], glyphs: &Glyphs) {
    let w = |t: &Text| glyphs.width(t.as_str(), size);
    let run = |text: Text, cx: f32| Run { text, cx, cy, turn: (1., 0.), size, ink };
    match tm.split {
        Split::Lone => runs.push(run(tm.a, bar)),
        Split::Colon => {
            let colon = Text::of(":");
            let wc = w(&colon);
            runs.push(run(colon, bar));
            runs.push(run(tm.a, bar - 0.5 * wc - 0.5 * w(&tm.a)));
            runs.push(run(tm.b, bar + 0.5 * wc + 0.5 * w(&tm.b)));
        }
        Split::Gap => {
            runs.push(run(tm.a, bar - air - 0.5 * w(&tm.a)));
            runs.push(run(tm.b, bar + air + 0.5 * w(&tm.b)));
        }
    }
}

/// Draw the face into `px` (`d` a side, cleared) at the view's turn: the words, then each pixel's bars, hours and sky under them.
pub fn paint(px: &mut [u64], d: usize, (tc, ts): (f32, f32), c: &Chart, cache: &mut Cache, glyphs: &mut Glyphs) {
    let (r, df) = (d as f32 * 0.5, d as f32);
    let (hair, em) = (hair(d), em(d));
    let axis = Axis::new(c.now, d);
    if !cache.year.as_ref().is_some_and(|y| y.fresh(c.tide, c.place, c.now)) {
        cache.year = Some(Year::build(c.tide, c.place, c.now));
    }
    let Some(year) = cache.year.as_ref() else { return };
    let geo = Geo { d: df, lo: c.scale.0, hi: c.scale.1, hair };

    // The backgrounds, column by column; a circle pinched small gives back what is more than four times what it needs.
    for g in [&mut cache.fill, &mut cache.full] {
        g.resize(d * d, 0);
        if g.capacity() > 4 * d * d {
            g.shrink_to_fit();
        }
    }
    let (mut fill, mut full) = (vec![[0f32; 3]; d], vec![[0f32; 3]; d]);
    let (mut sky_d, mut water_d) = (vec![[0f32; 3]; d + 1], vec![0f32; d + 1]);
    let mut slices: Vec<Slice> = Vec::new();
    let step = (1u64 << STEP_LOG2) as f64;
    let mid = d / 2;
    for x in 0..d {
        let (a, b) = (axis.t(x as f64), axis.t(x as f64 + 1.));
        slices.clear();
        if b - a < (1u64 << DIRECT_LOG2) as f64 {
            // The widget's floor on a column's samples: a column near now spans a few minutes, and eight instants still share a waterline crossing it out smoothly.
            let n = (((b - a) as usize) >> SAMPLE_LOG2).max(SAMPLES_MIN);
            let fr = Frame::at(0.5 * (a + b));
            for i in 0..n {
                let t = a + (b - a) * (i as f64 + 0.5) / n as f64;
                slices.push(slice_at(c, &fr, t, &geo, year.sun_max, year.moon_max));
            }
        } else {
            let (i0, i1) = (libm::ceil((a - year.first) / step) as usize, libm::ceil((b - year.first) / step) as usize);
            for i in i0..i1 {
                slices.push(geo.slice(year.h[i], year.sun[i], year.moon[i], year.sun_max, year.moon_max));
            }
        }
        column(&slices, d, hair, &mut fill, &mut full, &mut sky_d, &mut water_d);
        // The centreline: mid-tide and the horizon, every channel flipped by its coverage (in code space, as the bars flip).
        let cm = df * 0.5;
        // The rows its edges reach: the middle two, and one either side (a face under four pixels has none to spare, and takes what it has).
        for row in (mid as isize - 2).max(0) as usize..(mid + 2).min(d) {
            let cov = band(row as f32 + 0.5, cm - 0.5 * hair, cm + 0.5 * hair);
            if cov > 0. {
                let s: [f32; 3] = core::array::from_fn(|k| sqrtf(full[row][k]));
                full[row] = core::array::from_fn(|k| {
                    let f = 1. - s[k];
                    full[row][k] * (1. - cov) + f * f * cov
                });
            }
        }
        for row in 0..d {
            cache.fill[row * d + x] = pack(fill[row]);
            cache.full[row * d + x] = pack(full[row]);
        }
    }

    // The hours: each line a fixed span of time (a hair at the centre), so it narrows with the axis toward the rim and the ink per hour stays the same; each column's coverage the sum of the lines' C¹ bands at its centre.
    let line_s = hair as f64 * axis.centre_px();
    let (mut hours, mut nights) = (vec![0f32; d], vec![0f32; d]);
    {
        let (from, to) = (axis.t(0.), axis.t(df as f64));
        let hstep = hour_step(c.base);
        let mut day = midnight_before(from, c.zone);
        while day < to {
            let mut k = 0;
            while (k as f64) * hstep < DAY - 0.5 {
                let t = day + k as f64 * hstep;
                let (x, w) = (axis.x(t) as f32, (line_s * axis.rate(t)) as f32 * if k == 0 { 2. } else { 1. });
                let (lo, hi) = (x - 0.5 * w, x + 0.5 * w);
                let into = if k == 0 { &mut nights } else { &mut hours };
                // WHY the bounds: a line near the rim reaches past the face's first or last column.
                for col in (floorf(lo - 0.5).max(0.) as usize)..(libm::ceilf(hi + 0.5).min(df) as usize) {
                    into[col] += band(col as f32 + 0.5, lo, hi);
                }
                k += 1;
            }
            day += DAY;
        }
    }

    // The highs and lows within half a day, and the sun's crossings of the horizon.
    let mut ex = [Extreme { t: 0., h: 0., high: false }; 1 << 3];
    let ne = c.tide.extremes(c.now - CENTRE_HALF, c.now + CENTRE_HALF, &mut ex);
    let mut cs = [Crossing { t: 0., rising: false }; 1 << 3];
    let ns = ferros_sky::crossings(Body::Sun, c.now - CENTRE_HALF, c.now + CENTRE_HALF, c.place, SUNRISE, &mut cs);

    // The words, front first: now, the tide's times, the sun's; in focus the station. Each bar is open but for its words' rows.
    let mut runs: Vec<Run> = Vec::new();
    let gap = |cy: f32| (cy - em * (0.5 + 0.15), cy + em * (0.5 + 0.15));
    let air = 2. * hair;
    straddle(&mut runs, &time_words(c.now, c.zone, c.base), r, r, em, air, NOW_INK, glyphs);
    let mut gaps: Vec<(f32, f32)> = vec![gap(r)];
    let mut bars: Vec<(f32, f32, f32)> = Vec::new();
    for e in &ex[..ne] {
        let x = axis.x(e.t) as f32;
        // A high's time below the water's crest, a low's above its trough (the widget's thirds).
        let cy = if e.high { df * (2. / 3.) } else { df * (1. / 3.) };
        straddle(&mut runs, &time_words(e.t, c.zone, c.base), x, cy, em, air, TIDE_INK, glyphs);
        let (g0, g1) = gap(cy);
        bars.push((x, g0, g1));
    }
    for s in &cs[..ns] {
        let x = axis.x(s.t) as f32;
        let tm = time_words(s.t, c.zone, c.base);
        let mut text = tm.a;
        if tm.split == Split::Colon {
            text.push(':');
        }
        text.push_str(tm.b.as_str());
        let len = glyphs.width(text.as_str(), em);
        // From the circle's top at its column, a quarter of the words' size below it: sunrise turned to read up, sunset down.
        let dx = x - r;
        let top = r - sqrtf(r * r - dx * dx) + em * (1. / (1 << 2) as f32);
        let turn = if s.rising { (0., -1.) } else { (0., 1.) };
        runs.push(Run { text, cx: x, cy: top + 0.5 * len, turn, size: em, ink: SUN_INK });
    }
    if let Some((name, metres)) = c.station {
        let size = em * (3. / 4.);
        let cy = df * (13. / 16.);
        runs.push(Run { text: Text::of(name), cx: r, cy, turn: (1., 0.), size, ink: NOW_INK });
        let mut b = [0u8; fmt::OUT];
        let km = Text::of(fmt::number((metres / 1000. * 65536.) as i64, 0, fmt::unit::KM, c.base, &mut b));
        runs.push(Run { text: km, cx: r, cy: cy + size * 1.25, turn: (1., 0.), size, ink: NOW_INK });
        gaps.push((cy - size * 0.65, cy + size * 1.9));
    }

    // Face point to the square's pixels, and back.
    let to_square = |fx: f32, fy: f32| (r + (fx - r) * tc - (fy - r) * ts, r + (fx - r) * ts + (fy - r) * tc);
    let to_face = |sx: f32, sy: f32| {
        let (dx, dy) = (sx - r, sy - r);
        (r + dx * tc + dy * ts, r + dy * tc - dx * ts)
    };

    for w in &runs {
        let s = w.text.as_str();
        if s.is_empty() {
            continue;
        }
        // The words' own turn on the face, then the face's on the square.
        let (wc, ws) = w.turn;
        let turn = (tc * wc - ts * ws, ts * wc + tc * ws);
        let tw = glyphs.width(s, w.size);
        // The baseline's start in the words' own frame, (−width/2, +0.36 em) from their centre, turned onto the square (ferros_puck::square::words).
        let (fx, fy) = (-tw * 0.5, w.size * 0.36);
        let (cx, cy) = to_square(w.cx, w.cy);
        let (fill, ink, dd) = (&cache.fill, w.ink, d as i64);
        glyphs.cover(s, cx + turn.0 * fx - turn.1 * fy, cy + turn.1 * fx + turn.0 * fy, w.size, turn, |x, y, a| {
            // WHY the bounds: a glyph's pixels come from its outline, and words near the rim reach past the square.
            if x >= 0 && y >= 0 && x < dd && y < dd {
                let (gx, gy) = to_face(x as f32 + 0.5, y as f32 + 0.5);
                let p = opaque(flip(sample(fill, d, gx, gy), ink));
                let i = y as usize * d + x as usize;
                px[i] = under(px[i], cover(p, a));
            }
        });
    }

    // Every pixel: the now bar, the tide's bars, the hours, the sky.
    let (white, black, grey) = (opaque([1.; 3]), opaque([0.; 3]), opaque([HOUR_CODE; 3]));
    let (ha, ma) = (1. / (1 << HOUR_ALPHA_LOG2) as f32, 1. / (1 << MIDNIGHT_ALPHA_LOG2) as f32);
    for y in 0..d {
        for x in 0..d {
            let i = y * d + x;
            if px[i] >> 48 == 0xFFFF {
                continue;
            }
            let (gx, gy) = to_face(x as f32 + 0.5, y as f32 + 0.5);
            let (dx, dy) = (gx - r, gy - r);
            // Past the rim the display trims it: nothing drawn.
            if dx * dx + dy * dy > (r + 1.) * (r + 1.) {
                continue;
            }
            let mut acc = px[i];
            // Where a bar shows: everywhere but its words' rows.
            let open = |g0: f32, g1: f32| 1. - band(gy, g0, g1);
            // The now bar: a hair of white in a band of black three hairs wide, one layer (a partition of the pixel, premultiplied).
            let vis = gaps.iter().fold(1., |v, &(g0, g1)| v * open(g0, g1));
            let (cw, cb) = (band(gx, r - 0.5 * hair, r + 0.5 * hair) * vis, band(gx, r - 1.5 * hair, r + 1.5 * hair) * vis);
            if cb > 0. {
                let a = |v: f32| (v * 65535.) as u32;
                let p = cover(white, a(cw)) + cover(black, a(cb - cw));
                acc = under(acc, p);
            }
            // The highs' and lows' bars: green and blue flipped from what lies behind.
            for &(bx, g0, g1) in &bars {
                let cov = band(gx, bx - 0.5 * hair, bx + 0.5 * hair) * open(g0, g1);
                if cov > 0. {
                    let p = opaque(flip(sample(&cache.full, d, gx, gy), TIDE_INK));
                    acc = under(acc, cover(p, (cov * 65535.) as u32));
                }
            }
            // The hours: grey of their own at their own opacity.
            let col = |v: &[f32]| {
                let u = gx - 0.5;
                let f = floorf(u);
                // WHY the bounds: as `sample`'s, a turned square's rim pixels land past the first or last column.
                let last = d as i32 - 1;
                let (j, a) = (f as i32, u - f);
                v[j.clamp(0, last) as usize] * (1. - a) + v[(j + 1).clamp(0, last) as usize] * a
            };
            let alpha = col(&hours) * ha + col(&nights) * ma;
            if alpha > 0. {
                acc = under(acc, cover(grey, (alpha * 65535.) as u32));
            }
            // The sky, the water and the curves, opaque.
            acc = under(acc, opaque(sample(&cache.full, d, gx, gy)));
            px[i] = acc;
        }
    }
}

/// When the face next changes (Eagle seconds): now moving half a pixel along the axis at the centre, or now's words turning over, whichever is first.
pub fn deadline(c: &Chart, d: usize) -> f64 {
    let axis = Axis::new(c.now, d);
    let half_px = 0.5 * axis.centre_px();
    let w = word_step(c.base);
    let local = c.now + (EAGLE_EPOCH_UNIX_SECS + c.zone) as f64;
    let next_word = c.now + (w - (local - floor(local / w) * w));
    let a = c.now + half_px;
    if a < next_word { a } else { next_word }
}
