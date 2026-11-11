//! The wallpaper page (owner, 2026-11-10: "It would [change everyone's wallpaper], which is why I wanted to fidoodle with it some before we ship it"): a person's default wallpaper drawn by the device's own crate (`wallpaper::params_seen`, `render_rows`, `mark_rows`, `refine_rows`), from a seed, under the `wallpaper::Knobs` the sliders set, at husky's frame (1344 × 2992) or akita's (1080 × 2400).
//!
//! Left: the picture fitted to the window (C shows it 1:1 instead, a crop the arrows pan, Shift for four times the step), and beside it the seed, c, the time each pass took, and every attempt the survey looked at with its numbers and why it failed, or that it passed and was kept.
//! Right: one slider row per knob, its value in its own unit; the first is the frame (husky or akita), the next five the anchor (Today, the device's; α at the centre; α at a third) and its own knobs, then the constants `candidate` and `survey` draw from, in the crate's order.
//! Keys: N and P the next and previous seed (a counter, spread over 32 bytes by `wallpaper::seed_of`), R a random seed (32 bytes from the host's /dev/urandom), H types a seed in hex (64 digits are its bytes, fewer a counter; Enter takes it, Escape drops it), V the other frame, A shows or hides the anchor's point on the picture, S prints the knobs as the crate's constants (Rust source, ready to paste over wallpaper/src/lib.rs's), X puts every knob back to `Knobs::DEFAULT`.
//!
//! The drawing is never on the UI thread: a change sends a job to one worker thread, which drops any job a newer one has replaced, works out the seed's choices, draws a quick pass at a quarter of the size, then the first pass at full size and the refined one (the device's three passes), each banded across the host's cores with rayon, and hands each back thru fluor's wake-sender as it lands; a newer job stops an older one between bands. The window shows the newest picture it has until the next one lands.
//! The picture is the display's working values (√light × 16384 a channel), shown as screencap writes them: light = (v / 16384)², a gamma2tanh code (vsf's own encoder) at 8 bits, as α + darkness.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Instant;

use rayon::prelude::*;
use wallpaper::{Anchor, Knobs, Params, Survey};

use crate::{DIM, LABEL, fail, smooth_min};

use fluor::canvas::Canvas;
use fluor::coord::Coord;
use fluor::event::{ElementState, Key, KeyEvent, NamedKey};
use fluor::geom::Viewport;
use fluor::host::WakeSender;
use fluor::host::app::Context;
use fluor::host::chrome::HitId;
use fluor::pixel::{Blend, BlendMode};
use fluor::text::TextStyle;
use fluor::widgets::Slider;

/// The frames the wallpaper is judged at: the devices' panels.
pub const DEVICES: [(&str, usize, usize); 2] = [("husky", 1344, 2992), ("akita", 1080, 2400)];

/// The quick pass's scale: one pixel per 4 × 4 of the frame.
const QUICK: usize = 1 << 2;
/// Rows a band, the unit the passes are shared out in (the display's own band, payloads/display `wall_band`).
const BAND: usize = 1 << 6;

/// A wallpaper seed: its 32 bytes, and the counter it was spread from (`wallpaper::seed_of`) when it was.
#[derive(Clone, Copy, Debug)]
pub struct Seed {
    pub bytes: [u8; 32],
    pub counter: Option<u64>,
}

impl Seed {
    pub fn counter(n: u64) -> Seed {
        Seed { bytes: wallpaper::seed_of(n), counter: Some(n) }
    }

    /// A seed as a person writes it: 64 hex digits (a `G#` prefix and `_` allowed) are its bytes; fewer, or a decimal number (`A#` allowed), a counter (`G#` hex or decimal).
    pub fn parse(s: &str) -> Result<Seed, String> {
        let t: String = s.chars().filter(|&c| c != '_').collect();
        let (hex, digits) = match t.strip_prefix("G#").or_else(|| t.strip_prefix("g#")) {
            Some(d) => (true, d.to_string()),
            None => (false, t.strip_prefix("A#").unwrap_or(&t).to_string()),
        };
        if digits.len() == 64 && digits.chars().all(|c| c.is_ascii_hexdigit()) {
            let mut b = [0u8; 32];
            for (k, v) in b.iter_mut().enumerate() {
                *v = u8::from_str_radix(&digits[k * 2..k * 2 + 2], 16).map_err(|e| e.to_string())?;
            }
            return Ok(Seed { bytes: b, counter: None });
        }
        let n = if hex { u64::from_str_radix(&digits, 16) } else { digits.parse::<u64>() };
        n.map(Seed::counter).map_err(|_| format!("not a seed: {s} (64 hex digits, or a counter)"))
    }

    /// The bytes as G# hex, in two halves of 32 digits.
    fn hex(&self) -> [String; 2] {
        let h: String = self.bytes.iter().map(|b| format!("{b:02X}")).collect();
        [format!("G#{}", &h[..32]), format!("  {}", &h[32..])]
    }
}

/// 32 bytes from the host's entropy (a read, no file kept).
fn random_seed() -> Seed {
    let mut b = [0u8; 32];
    let mut f = std::fs::File::open("/dev/urandom").unwrap_or_else(|e| fail(&format!("/dev/urandom: {e}")));
    f.read_exact(&mut b).unwrap_or_else(|e| fail(&format!("/dev/urandom: {e}")));
    Seed { bytes: b, counter: None }
}

/// One knob as a slider: its label, its range, its step (0 continuous), and whether the slider moves along it in log₂.
struct Dial {
    label: &'static str,
    lo: f32,
    hi: f32,
    step: f32,
    log: bool,
}

const fn dial(label: &'static str, lo: f32, hi: f32, step: f32, log: bool) -> Dial {
    Dial { label, lo, hi, step, log }
}

const PI: f32 = core::f32::consts::PI;

const DIALS: [Dial; 39] = [
    dial("frame (V)", 0., 1., 1., false),
    dial("anchor", 0., 2., 1., false),
    dial("α preimages", 0., (1 << 4) as f32, 1., false),
    dial("α at x", 0., 1., 0., false),
    dial("α at y", 0., 1., 0., false),
    dial("seed mirrors", 0., 1., 1., false),
    dial("cardioid θ from", 0., 1. / 2., 0., false),
    dial("cardioid θ to", 0., 1. / 2., 0., false),
    dial("cardioid ε from", -10., -1., 0., false),
    dial("cardioid ε to", -10., -1., 0., false),
    dial("bulb φ from", 0., 1., 0., false),
    dial("bulb φ to", 0., 1., 0., false),
    dial("bulb ε from", 0., 1. / 2., 0., false),
    dial("bulb ε to", 0., 1. / 2., 0., false),
    dial("β preimages", 0., (1 << 5) as f32, 1., false),
    dial("pull from", 0., 1., 0., false),
    dial("pull to", 0., 1., 0., false),
    dial("view size from", 1. / (1 << 3) as f32, (1 << 2) as f32, 0., true),
    dial("view size to", 1. / (1 << 3) as f32, (1 << 2) as f32, 0., true),
    dial("far zoom from", 1., (1 << 3) as f32, 0., true),
    dial("far zoom to", 1., (1 << 3) as f32, 0., true),
    dial("far turn from", 0., PI, 0., false),
    dial("far turn to", 0., PI, 0., false),
    dial("far shift", 0., 1., 0., false),
    dial("warm hue from", -PI / 2., PI / 2., 0., false),
    dial("warm hue to", -PI / 2., PI / 2., 0., false),
    dial("far hue from", -PI, PI, 0., false),
    dial("far hue to", -PI, PI, 0., false),
    dial("hue step from", 0., PI, 0., false),
    dial("hue step to", 0., PI, 0., false),
    dial("saturation from", 0., 1., 0., false),
    dial("saturation to", 0., 1., 0., false),
    dial("gradient spread", 1. / (1 << 2) as f32, (1 << 3) as f32, 0., true),
    dial("attempts", 1., (1 << 6) as f32, 1., false),
    dial("near from", 0., 1., 0., false),
    dial("near to", 0., 1., 0., false),
    dial("emptiest third", 0., 1. / 2., 0., false),
    dial("inside under", 1. / (1 << 10) as f32, 1. / (1 << 2) as f32, 0., true),
    dial("near within", 1. / (1 << 10) as f32, 1. / (1 << 4) as f32, 0., true),
];
const N_DIALS: usize = DIALS.len();
const D_FRAME: usize = 0;

const ANCHORS: [Anchor; 3] = [Anchor::Today, Anchor::AlphaCentre, Anchor::AlphaAt];

/// Dial `i`'s value (the frame `dev` is the first).
fn get(k: &Knobs, dev: usize, i: usize) -> f32 {
    match i {
        0 => dev as f32,
        1 => k.anchor as usize as f32,
        2 => k.alpha_steps as f32,
        3 | 4 => k.at[i - 3],
        5 => k.at_mirror as u32 as f32,
        6 | 7 => k.cardioid_t[i - 6],
        8 | 9 => k.cardioid_eps_log2[i - 8],
        10 | 11 => k.bulb_phi[i - 10],
        12 | 13 => k.bulb_eps[i - 12],
        14 => k.back_steps as f32,
        15 | 16 => k.pull[i - 15],
        17 | 18 => k.view_span[i - 17],
        19 | 20 => k.far_zoom[i - 19],
        21 | 22 => k.far_turn[i - 21],
        23 => k.far_shift,
        24 | 25 => k.warm_hue[i - 24],
        26 | 27 => k.far_hue[i - 26],
        28 | 29 => k.hue_step[i - 28],
        30 | 31 => k.sat[i - 30],
        32 => k.grad_spans,
        33 => k.attempts as f32,
        34 | 35 => k.near[i - 34],
        36 => k.near_third,
        37 => k.inside,
        _ => k.near_span,
    }
}

/// Sets dial `i` to `v`.
fn set(k: &mut Knobs, dev: &mut usize, i: usize, v: f32) {
    match i {
        0 => *dev = v as usize,
        1 => k.anchor = ANCHORS[v as usize],
        2 => k.alpha_steps = v as u32,
        3 | 4 => k.at[i - 3] = v,
        5 => k.at_mirror = v != 0.,
        6 | 7 => k.cardioid_t[i - 6] = v,
        8 | 9 => k.cardioid_eps_log2[i - 8] = v,
        10 | 11 => k.bulb_phi[i - 10] = v,
        12 | 13 => k.bulb_eps[i - 12] = v,
        14 => k.back_steps = v as u32,
        15 | 16 => k.pull[i - 15] = v,
        17 | 18 => k.view_span[i - 17] = v,
        19 | 20 => k.far_zoom[i - 19] = v,
        21 | 22 => k.far_turn[i - 21] = v,
        23 => k.far_shift = v,
        24 | 25 => k.warm_hue[i - 24] = v,
        26 | 27 => k.far_hue[i - 26] = v,
        28 | 29 => k.hue_step[i - 28] = v,
        30 | 31 => k.sat[i - 30] = v,
        32 => k.grad_spans = v,
        33 => k.attempts = v as u32,
        34 | 35 => k.near[i - 34] = v,
        36 => k.near_third = v,
        37 => k.inside = v,
        _ => k.near_span = v,
    }
}

/// Dial `i`'s value as a person reads it, in its own unit.
fn show(k: &Knobs, dev: usize, i: usize) -> String {
    let v = get(k, dev, i);
    match i {
        0 => format!("{} {}×{}", DEVICES[dev].0, DEVICES[dev].1, DEVICES[dev].2),
        1 => ["today", "α centre", "α at a third"][v as usize].to_string(),
        2 | 14 => format!("A#{}", v as u32),
        3 => format!("{v:.3} w"),
        4 => format!("{v:.3} h"),
        5 => if k.at_mirror { "either side" } else { "as set" }.to_string(),
        6 | 7 => format!("{v:.3} t, {:.0}°", v * 360.),
        8 | 9 => format!("2^{v:.2} = {:.4}", v.exp2()),
        10 | 11 => format!("{v:.3} ½t, {:.0}°", v * 180.),
        12 | 13 => format!("{v:.3} r"),
        15 | 16 | 30 | 31 => format!("{v:.3}"),
        17 | 18 => format!("{v:.3} /span"),
        19 | 20 => format!("{v:.2} ×"),
        21 | 22 | 24..=29 => format!("{:+.1}°", v.to_degrees()),
        23 => format!("{v:.3} far span"),
        32 => format!("{v:.2} spans"),
        33 => format!("A#{}", v as u32),
        34..=36 => format!("{:.1} %", v * 100.),
        37 => format!("{:.2} %", v * 100.),
        _ => format!("1/{:.0} span", 1. / v),
    }
}

/// A slider's position for dial `i`'s value.
fn to_unit(k: &Knobs, dev: usize, i: usize) -> f32 {
    let (d, v) = (&DIALS[i], get(k, dev, i));
    if d.log { (v / d.lo).log2() / (d.hi / d.lo).log2() } else { (v - d.lo) / (d.hi - d.lo) }
}

/// Dial `i`'s value at slider position `u`, on its step.
fn from_unit(i: usize, u: f32) -> f32 {
    let d = &DIALS[i];
    let v = if d.log { d.lo * ((d.hi / d.lo).log2() * u).exp2() } else { d.lo + u * (d.hi - d.lo) };
    if d.step == 0. { v } else { d.lo + ((v - d.lo) / d.step).round() * d.step }
}

/// The knobs as the crate's source: its constants (paste over wallpaper/src/lib.rs's), a changed one marked with what it was, and the anchor's fields for `Knobs::DEFAULT`.
fn rust_source(k: &Knobs) -> String {
    let d = Knobs::DEFAULT;
    let pair = |a: [f32; 2]| format!("[{:?}, {:?}]", a[0], a[1]);
    let was = |now: String, then: String| if now == then { String::new() } else { format!(" // was {then}") };
    let mut s = String::from("// tools/tune, the wallpaper page: the knobs as wallpaper/src/lib.rs's constants.\n");
    let pairs: [(&str, [f32; 2], [f32; 2]); 13] = [
        ("CARDIOID_T", k.cardioid_t, d.cardioid_t),
        ("CARDIOID_EPS_LOG2", k.cardioid_eps_log2, d.cardioid_eps_log2),
        ("BULB_PHI", k.bulb_phi, d.bulb_phi),
        ("BULB_EPS", k.bulb_eps, d.bulb_eps),
        ("PULL", k.pull, d.pull),
        ("VIEW_SPAN", k.view_span, d.view_span),
        ("FAR_ZOOM", k.far_zoom, d.far_zoom),
        ("FAR_TURN", k.far_turn, d.far_turn),
        ("WARM_HUE", k.warm_hue, d.warm_hue),
        ("FAR_HUE", k.far_hue, d.far_hue),
        ("HUE_STEP", k.hue_step, d.hue_step),
        ("SAT", k.sat, d.sat),
        ("NEAR", k.near, d.near),
    ];
    for (name, now, then) in pairs {
        s += &format!("const {name}: [f32; 2] = {};{}\n", pair(now), was(pair(now), pair(then)));
    }
    let ones: [(&str, f32, f32); 5] = [("FAR_SHIFT", k.far_shift, d.far_shift), ("GRAD_SPANS", k.grad_spans, d.grad_spans), ("NEAR_THIRD", k.near_third, d.near_third), ("INSIDE", k.inside, d.inside), ("NEAR_SPAN", k.near_span, d.near_span)];
    for (name, now, then) in ones {
        s += &format!("const {name}: f32 = {now:?};{}\n", was(format!("{now:?}"), format!("{then:?}")));
    }
    s += &format!("const BACK_STEPS: u32 = {};{}\n", k.back_steps, was(k.back_steps.to_string(), d.back_steps.to_string()));
    s += &format!("pub const ATTEMPTS: u32 = {};{}\n", k.attempts, was(k.attempts.to_string(), d.attempts.to_string()));
    s += "// Knobs::DEFAULT's anchor fields:\n";
    s += &format!("        anchor: Anchor::{:?},\n        alpha_steps: {},\n        at: {},\n        at_mirror: {},\n", k.anchor, k.alpha_steps, pair(k.at), k.at_mirror);
    s
}

/// One drawing to make: the seed, the frame, the knobs; `serial` names it, so a newer one replaces it.
struct Job {
    serial: u64,
    seed: [u8; 32],
    w: usize,
    h: usize,
    knobs: Knobs,
}

/// A pass the worker finished: which job and which pass (0 quick, 1 first, 2 refined), the picture (`w` × `h` working values; the frame is `full`), the choices, every attempt the survey looked at, the knobs it ran under, and how long the pass took.
pub struct Done {
    serial: u64,
    stage: usize,
    w: usize,
    h: usize,
    full: (usize, usize),
    px: Vec<u64>,
    params: Params,
    seen: Vec<(u32, Survey)>,
    knobs: Knobs,
    ms: f32,
}

/// The passes of one job, each handed to `out` as it lands; `stale` is asked between bands, and a pass it stops is not handed on.
fn passes(job: &Job, stale: &(dyn Fn() -> bool + Sync), out: &mut dyn FnMut(Done)) {
    let (w, h) = (job.w, job.h);
    let t = Instant::now();
    let mut seen = Vec::new();
    let p = wallpaper::params_seen(&job.seed, w, h, &job.knobs, |c| seen.push((c.attempt, c.survey)));
    // The quick pass: the same choices at a quarter of the size, every per-pixel step four times as long (as `survey` looks at a sixteenth).
    let mut q = p;
    (q.width, q.height) = (w / QUICK, h / QUICK);
    let k = QUICK as f32;
    q.step = [p.step[0] * k, p.step[1] * k];
    q.far_step = [p.far_step[0] * k, p.far_step[1] * k];
    q.grad = [p.grad[0] * k, p.grad[1] * k];
    let quick = draw(&q, stale);
    if stale() {
        return;
    }
    let done = |stage, w, h, px, ms| Done { serial: job.serial, stage, w, h, full: (job.w, job.h), px, params: p, seen: seen.clone(), knobs: job.knobs, ms };
    out(done(0, q.width, q.height, quick, t.elapsed().as_secs_f32() * 1000.));
    let t = Instant::now();
    let first = draw(&p, stale);
    if stale() {
        return;
    }
    out(done(1, w, h, first.clone(), t.elapsed().as_secs_f32() * 1000.));
    let t = Instant::now();
    let stride = wallpaper::mask_stride(w);
    let mut mask = vec![0u8; stride * h];
    mask.par_chunks_mut(stride * BAND).enumerate().for_each(|(b, m)| {
        if !stale() {
            wallpaper::mark_rows(&p, &first, b * BAND..b * BAND + m.len() / stride, m);
        }
    });
    let mut refined = first;
    refined.par_chunks_mut(w * BAND).zip(mask.par_chunks(stride * BAND)).enumerate().for_each(|(b, (o, m))| {
        if !stale() {
            wallpaper::refine_rows(&p, b * BAND..b * BAND + o.len() / w, m, o);
        }
    });
    if stale() {
        return;
    }
    out(done(2, w, h, refined, t.elapsed().as_secs_f32() * 1000.));
}

/// The first pass of `p`, banded across the host's cores (a band's rows are its chunk's length over the width, so the last band is the frame's own remainder).
fn draw(p: &Params, stale: &(dyn Fn() -> bool + Sync)) -> Vec<u64> {
    let w = p.width;
    let mut px = vec![0u64; w * p.height];
    px.par_chunks_mut(w * BAND).enumerate().for_each(|(b, band)| {
        if !stale() {
            wallpaper::render_rows(p, b * BAND..b * BAND + band.len() / w, band);
        }
    });
    px
}

/// The window's 8-bit α + darkness pixel for each working value (√light × 16384): light = (v / 16384)², its gamma2tanh code at 8 bits, opaque; indexed by the value's 16 bits, so every value has an entry.
fn screen_lut() -> &'static [u32] {
    static LUT: OnceLock<Vec<u32>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..1u32 << 16)
            .map(|v| {
                let l = v as f32 / 16384.;
                255 - vsf::colour::convert::delinearize_gamma2tanh_bits_f32(l * l, 8)
            })
            .collect()
    })
}

/// A working value's three channels as the window's pixel.
#[inline]
fn screen(lut: &[u32], r: u64, g: u64, b: u64) -> u32 {
    0xFF00_0000 | lut[r as u16 as usize] << 16 | lut[g as u16 as usize] << 8 | lut[b as u16 as usize]
}

/// A picture made ready for the window: `w` × `h` pixels with its top left at (x0, y0), and how a frame pixel maps onto it (screen = origin + frame position × scale).
struct Fit {
    key: (u64, usize, usize, usize, usize, usize, bool, i64, i64),
    px: Vec<u32>,
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    origin: [f32; 2],
    scale: f32,
}

/// The picture `d` in the region (rx, ry, rw, rh): fitted (a box average of the working values under each window pixel, then the screen's code) or, `crop`, at one frame pixel a window pixel about the frame's centre moved by `pan`.
fn fit(d: &Done, region: (usize, usize, usize, usize), crop: bool, pan: [i64; 2]) -> Fit {
    let (rx, ry, rw, rh) = region;
    let (fw, fh) = d.full;
    let (pw, ph) = (d.w, d.h);
    let lut = screen_lut();
    let key = (d.serial, d.stage, rx, ry, rw, rh, crop, pan[0], pan[1]);
    if crop {
        // The frame pixel at the region's top left; a region pixel (x, y) shows frame pixel (cx + x, cy + y), the picture's pixel under it at its own scale (the quick pass is a quarter).
        let (cx, cy) = (fw as i64 / 2 + pan[0] - rw as i64 / 2, fh as i64 / 2 + pan[1] - rh as i64 / 2);
        let mut px = vec![0u32; rw * rh];
        // WHY/PROOF: the crop window and the frame are different rectangles (the window any size, the frame the device's, the pan anywhere); only their overlap has frame pixels, and a region pixel outside it would index outside the picture. These are the overlap's bounds, the region left empty (the ground shows) elsewhere.
        let (xa, xb) = ((-cx).max(0).min(rw as i64) as usize, (fw as i64 - cx).max(0).min(rw as i64) as usize);
        let (ya, yb) = ((-cy).max(0).min(rh as i64) as usize, (fh as i64 - cy).max(0).min(rh as i64) as usize);
        // WHY the max: chunks of 0 panic, and a window shrunk to nothing has a region 0 wide; its buffer is then empty, so chunks of 1 yield no row at all.
        px.par_chunks_mut(rw.max(1)).enumerate().for_each(|(y, row)| {
            if y < ya || y >= yb {
                return;
            }
            let sy = (cy + y as i64) as usize * ph / fh;
            for x in xa..xb {
                let sx = (cx + x as i64) as usize * pw / fw;
                let v = d.px[sy * pw + sx];
                row[x] = screen(lut, v, v >> 16, v >> 32);
            }
        });
        return Fit { key, px, w: rw, h: rh, x0: rx, y0: ry, origin: [(rx as i64 - cx) as f32, (ry as i64 - cy) as f32], scale: 1. };
    }
    // Fitted: the frame's scale to the region a smooth minimum of the two sides' (no corner as the window's shape crosses the frame's), centred.
    let s = smooth_min(rw as f32 / fw as f32, rh as f32 / fh as f32);
    let (dw, dh) = ((fw as f32 * s) as usize, (fh as f32 * s) as usize);
    let (x0, y0) = (rx + (rw - dw) / 2, ry + (rh - dh) / 2);
    let mut px = vec![0u32; dw * dh];
    // Each window pixel averages the picture's pixels in its footprint, [x·pw/dw, ⌈(x+1)·pw/dw⌉) in integers: the right edge is above the left (so a footprint is never empty, upscaling the quick pass included) and the last one is exactly pw, so every index is inside the picture.
    // WHY the max: as in the crop above, a fit 0 wide (a window shrunk to nothing) has an empty buffer, and chunks of 0 panic where chunks of 1 yield nothing.
    px.par_chunks_mut(dw.max(1)).enumerate().for_each(|(y, row)| {
        let (ya, yb) = (y * ph / dh, ((y + 1) * ph).div_ceil(dh));
        for (x, out) in row.iter_mut().enumerate() {
            let (xa, xb) = (x * pw / dw, ((x + 1) * pw).div_ceil(dw));
            let mut sum = [0u64; 3];
            for sy in ya..yb {
                for &v in &d.px[sy * pw + xa..sy * pw + xb] {
                    sum = [sum[0] + (v & 0xFFFF), sum[1] + (v >> 16 & 0xFFFF), sum[2] + (v >> 32 & 0xFFFF)];
                }
            }
            let n = ((yb - ya) * (xb - xa)) as u64;
            *out = screen(lut, sum[0] / n, sum[1] / n, sum[2] / n);
        }
    });
    Fit { key, px, w: dw, h: dh, x0, y0, origin: [x0 as f32, y0 as f32], scale: dw as f32 / fw as f32 }
}

/// Why a survey failed, by the knobs it ran under (empty: it passed).
fn failures(s: &Survey, k: &Knobs) -> Vec<&'static str> {
    let mut v = Vec::new();
    if s.near <= k.near[0] {
        v.push("sparse");
    }
    if s.near >= k.near[1] {
        v.push("crowded");
    }
    if s.near_min_third <= k.near_third {
        v.push("empty third");
    }
    if s.inside >= k.inside {
        v.push("blob");
    }
    v
}

/// An attempt's line: its number, its survey's numbers, and passed or why not.
fn attempt_line(a: u32, s: &Survey, k: &Knobs) -> String {
    let f = failures(s, k);
    let verdict = if f.is_empty() { "passes".to_string() } else { f.join(", ") };
    format!("A#{a}  near {:.1} %, third {:.1} %, in {:.2} %: {verdict}", s.near * 100., s.near_min_third * 100., s.inside * 100.)
}

/// The page.
pub struct Wall {
    pub sliders: Vec<Slider>,
    knobs: Knobs,
    dev: usize,
    seed: Seed,
    /// The counter N and P step (the last counter seed shown; a typed or random seed leaves it).
    counter: u64,
    /// The hex being typed (H), until Enter or Escape; and the last word about it.
    entry: Option<String>,
    note: String,
    crop: bool,
    pan: [i64; 2],
    marker: bool,
    tx: Option<mpsc::Sender<Job>>,
    latest: Arc<AtomicU64>,
    serial: u64,
    /// The newest picture back, its fit to the window, and each pass's time for the newest job.
    pic: Option<Done>,
    fit: Option<Fit>,
    times: [Option<f32>; 3],
    split: Coord,
    top: Coord,
    region: (usize, usize, usize, usize),
    info_x: Coord,
}

impl Wall {
    pub fn new(hit_counter: &mut HitId, seed: Option<Seed>) -> Self {
        let knobs = Knobs::DEFAULT;
        let sliders = (0..N_DIALS).map(|i| Slider::new(hit_counter, 0., 0., 1., 1., to_unit(&knobs, 0, i))).collect();
        let seed = seed.unwrap_or(Seed::counter(0));
        Self {
            sliders,
            knobs,
            dev: 0,
            seed,
            counter: seed.counter.unwrap_or(0),
            entry: None,
            note: String::new(),
            crop: false,
            pan: [0; 2],
            marker: true,
            tx: None,
            latest: Arc::new(AtomicU64::new(0)),
            serial: 0,
            pic: None,
            fit: None,
            times: [None; 3],
            split: 0.,
            top: 0.,
            region: (0, 0, 0, 0),
            info_x: 0.,
        }
    }

    /// The worker, started once the host hands over its wake-sender; the first job goes at once.
    pub fn start(&mut self, wake: Arc<dyn WakeSender<Done>>) {
        let (tx, rx) = mpsc::channel::<Job>();
        let latest = self.latest.clone();
        std::thread::spawn(move || {
            while let Ok(mut job) = rx.recv() {
                // Only the newest job waiting is worth drawing.
                while let Ok(j) = rx.try_recv() {
                    job = j;
                }
                let stale = || latest.load(Ordering::Relaxed) != job.serial;
                passes(&job, &stale, &mut |d| {
                    let _ = wake.send(d);
                });
            }
        });
        self.tx = Some(tx);
        self.submit();
    }

    /// A new job for the seed, frame and knobs as they are now.
    fn submit(&mut self) {
        self.serial += 1;
        self.latest.store(self.serial, Ordering::Relaxed);
        self.times = [None; 3];
        let (_, w, h) = DEVICES[self.dev];
        if let Some(tx) = &self.tx {
            let _ = tx.send(Job { serial: self.serial, seed: self.seed.bytes, w, h, knobs: self.knobs });
        }
    }

    /// A pass back from the worker: kept if it is newer than the picture shown (a later job, or a later pass of the same one). True if the window should redraw.
    pub fn accept(&mut self, d: Done) -> bool {
        if d.serial == self.serial {
            self.times[d.stage] = Some(d.ms);
        }
        let newer = match &self.pic {
            Some(p) => (d.serial, d.stage) > (p.serial, p.stage),
            None => true,
        };
        if newer {
            self.pic = Some(d);
        }
        true
    }

    pub fn typing(&self) -> bool {
        self.entry.is_some()
    }

    /// Lays the page out under the top strip: the picture's region at the left, the notes beside it, the slider column at the right.
    pub fn layout(&mut self, vp: Viewport, top: Coord) {
        let (w, h) = (vp.width_px as Coord, vp.height_px as Coord);
        let margin = vp.effective_span() / (1 << 6) as Coord;
        self.top = top;
        self.split = w * (1. - 3. / (1 << 3) as Coord);
        // The picture's region: the height under the strip, a portrait frame's width of it (the taller panel's, so both fit), never more than half the left side.
        let rh = h - top - 2. * margin;
        let rw = smooth_min(rh * DEVICES[0].1 as Coord / DEVICES[0].2 as Coord, self.split * 0.5);
        self.region = (margin as usize, (top + margin) as usize, rw as usize, rh as usize);
        self.info_x = margin * 2. + rw;
        // A narrower band than the orbs page's: these values are longer.
        crate::column(&mut self.sliders, self.split, w, top, h - margin, [0.34, 0.6]);
    }

    /// Slider moves into the knobs (or the frame); a change sends a new job.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for i in 0..N_DIALS {
            if self.sliders[i].take_change() {
                let v = from_unit(i, self.sliders[i].value());
                if get(&self.knobs, self.dev, i) != v {
                    set(&mut self.knobs, &mut self.dev, i, v);
                    changed = true;
                }
            }
        }
        if changed {
            self.submit();
        }
        changed
    }

    fn sync_sliders(&mut self) {
        for i in 0..N_DIALS {
            self.sliders[i].set_value(to_unit(&self.knobs, self.dev, i));
            self.sliders[i].take_change();
        }
    }

    fn new_seed(&mut self, s: Seed) {
        self.seed = s;
        if let Some(n) = s.counter {
            self.counter = n;
        }
        self.submit();
    }

    /// The page's keys; true if it took the key.
    pub fn key(&mut self, kev: &KeyEvent, shift: bool) -> bool {
        if kev.state != ElementState::Pressed {
            return false;
        }
        if let Some(e) = &mut self.entry {
            match &kev.logical_key {
                Key::Named(NamedKey::Enter) => {
                    let typed = e.clone();
                    self.entry = None;
                    match Seed::parse(&format!("G#{typed}")) {
                        Ok(s) => {
                            self.note.clear();
                            self.new_seed(s);
                        }
                        Err(m) => self.note = m,
                    }
                }
                Key::Named(NamedKey::Escape) => self.entry = None,
                Key::Named(NamedKey::Backspace) => {
                    e.pop();
                }
                Key::Character(c) if c.chars().all(|c| c.is_ascii_hexdigit() || c == '_') => e.push_str(&c.to_ascii_uppercase()),
                _ => {}
            }
            return true;
        }
        // The crop's pan: an eighth of the region a press, four times that with Shift.
        let step = (self.region.2.max(self.region.3) / (1 << 3)) as i64 * if shift { 4 } else { 1 };
        let pan = match &kev.logical_key {
            Key::Named(NamedKey::ArrowLeft) => Some([-step, 0]),
            Key::Named(NamedKey::ArrowRight) => Some([step, 0]),
            Key::Named(NamedKey::ArrowUp) => Some([0, -step]),
            Key::Named(NamedKey::ArrowDown) => Some([0, step]),
            _ => None,
        };
        if let Some(d) = pan {
            if !self.crop {
                return false;
            }
            self.pan = [self.pan[0] + d[0], self.pan[1] + d[1]];
            return true;
        }
        let Key::Character(c) = &kev.logical_key else { return false };
        match c.to_ascii_lowercase().as_str() {
            // WHY wrapping: the counter names 2^64 seeds and steps round them mod 2^64 (as SplitMix's own increment does), so before the first is the last.
            "n" => self.new_seed(Seed::counter(self.counter.wrapping_add(1))),
            "p" => self.new_seed(Seed::counter(self.counter.wrapping_sub(1))),
            "r" => self.new_seed(random_seed()),
            "h" => {
                self.entry = Some(String::new());
                self.note.clear();
            }
            "v" => {
                self.dev ^= 1;
                self.sliders[D_FRAME].set_value(to_unit(&self.knobs, self.dev, D_FRAME));
                self.sliders[D_FRAME].take_change();
                self.submit();
            }
            "c" => {
                self.crop = !self.crop;
                self.pan = [0; 2];
            }
            "a" => self.marker = !self.marker,
            "s" => print!("{}", rust_source(&self.knobs)),
            "x" => {
                self.knobs = Knobs::DEFAULT;
                self.sync_sliders();
                self.submit();
            }
            _ => return false,
        }
        true
    }

    /// The front of the page: the sliders and their values, the notes beside the picture, and the anchor's mark on it (the picture itself goes under the chrome, `render_back`).
    pub fn render_front(&mut self, target: &mut [u32], ctx: &mut Context, hit_map: &mut [HitId]) {
        let (w, h) = (ctx.viewport.width_px as usize, ctx.viewport.height_px as usize);
        let span = ctx.viewport.effective_span();
        let font = crate::font(span);
        let labels = DIALS.map(|d| d.label);
        let shown: Vec<String> = (0..N_DIALS).map(|i| show(&self.knobs, self.dev, i)).collect();
        crate::draw_column(&mut self.sliders, target, ctx, hit_map, self.split, &labels, &shown);
        self.refit();
        // The notes, a line each, down from the top of the picture's region.
        let mut lines: Vec<(String, u32)> = Vec::new();
        let [h0, h1] = self.seed.hex();
        lines.push((h0, LABEL));
        lines.push((h1, LABEL));
        lines.push((self.seed.counter.map_or("typed or random".to_string(), |n| format!("counter A#{n}")), DIM));
        if let Some(e) = &self.entry {
            lines.push((format!("hex: {e}_  (Enter takes it, Escape drops it)"), LABEL));
        } else if !self.note.is_empty() {
            lines.push((self.note.clone(), LABEL));
        }
        let ms = |t: Option<f32>| t.map_or("…".to_string(), |t| format!("{t:.0} ms"));
        lines.push((format!("quick {}, first {}, refined {}", ms(self.times[0]), ms(self.times[1]), ms(self.times[2])), DIM));
        if let Some(p) = &self.pic {
            let pp = &p.params;
            let pass = ["quick pass", "first pass", "refined"][p.stage];
            let current = if p.serial == self.serial { "" } else { ", drawing the new one" };
            lines.push((format!("showing the {pass}{current}"), DIM));
            lines.push((format!("c = {:+.5} {:+.5}i", pp.c[0], pp.c[1]), LABEL));
            lines.push((format!("anchor {:+.5} {:+.5}i, view {:.3} a span", pp.anchor[0], pp.anchor[1], (pp.step[0] * pp.step[0] + pp.step[1] * pp.step[1]).sqrt() * span_px(p.full)), LABEL));
            let passed = p.seen.iter().any(|(_, s)| s.passes_with(&p.knobs));
            lines.push((if passed { format!("kept attempt A#{}, the first to pass", pp.attempt) } else { format!("none of A#{} passed; kept the closest, A#{}", p.seen.len(), pp.attempt) }, LABEL));
            for (a, s) in &p.seen {
                lines.push((attempt_line(*a, s, &p.knobs), if *a == pp.attempt { LABEL } else { DIM }));
            }
        }
        let small = font * 0.8;
        // The notes stop at the slider column: a line too long for the window is cut there rather than written over the labels.
        let clip = Some(fluor::paint::Clip::new(self.info_x as usize, 0, self.split as usize, h));
        let mut y = self.region.1 as Coord + small;
        for (t, colour) in &lines {
            let mut canvas = Canvas::new(target, w, h, ctx.damage);
            ctx.text.draw_text_left(&mut canvas, t, self.info_x, y, &TextStyle::new(small, *colour), clip, None);
            y += small * 1.25;
        }
        let keys = ["N P next, previous seed · R random · H type hex · V frame", "C 1:1 (arrows pan) · A anchor mark · S print knobs · X defaults"];
        for (i, k) in keys.iter().enumerate() {
            let mut canvas = Canvas::new(target, w, h, ctx.damage);
            ctx.text.draw_text_left(&mut canvas, k, self.info_x, h as Coord - small * (2.5 - 1.25 * i as Coord), &TextStyle::new(small, DIM), clip, None);
        }
        // The anchor's mark: a ring about the point the view was built on, in front of the picture (a C∞ bump across the ring's width, so it has no hard edge).
        if let (true, Some(p), Some(f)) = (self.marker, &self.pic, &self.fit) {
            let pp = &p.params;
            let (d, s) = ([pp.anchor[0] - pp.centre[0], pp.anchor[1] - pp.centre[1]], pp.step);
            let s2 = s[0] * s[0] + s[1] * s[1];
            // The frame position under the anchor: `Frame::z_at` turned back (its step a rotation and scale), from the frame's centre.
            let fx = (d[0] * s[0] + d[1] * s[1]) / s2 + p.full.0 as f32 * 0.5;
            let fy = (d[1] * s[0] - d[0] * s[1]) / s2 + p.full.1 as f32 * 0.5;
            ring(target, w, h, f.origin[0] + fx * f.scale, f.origin[1] + fy * f.scale, span / (1 << 6) as Coord, span / (1 << 9) as Coord);
        }
    }

    /// The picture fitted again when it, the region, the crop or the pan changed.
    fn refit(&mut self) {
        let Some(p) = &self.pic else { return };
        let (rx, ry, rw, rh) = self.region;
        let key = (p.serial, p.stage, rx, ry, rw, rh, self.crop, self.pan[0], self.pan[1]);
        if self.fit.as_ref().is_some_and(|f| f.key == key) {
            return;
        }
        self.fit = Some(fit(p, self.region, self.crop, self.pan));
    }

    /// The picture, under everything in front of it; then the grounds.
    pub fn render_back(&mut self, target: &mut [u32], w: usize, _h: usize) {
        if let Some(f) = &self.fit {
            // WHY the max: a fit 0 wide has no pixels (chunks of 0 would panic; of 1, none).
            for (j, row) in f.px.chunks(f.w.max(1)).enumerate() {
                let line = &mut target[(f.y0 + j) * w + f.x0..(f.y0 + j) * w + f.x0 + f.w];
                for (d, &s) in line.iter_mut().zip(row) {
                    *d = d.under(s, BlendMode::Normal);
                }
            }
        }
        crate::grounds(target, w, self.split as usize);
    }
}

/// A frame's span in pixels (the harmonic mean of its sides), to read the view's size per span back.
fn span_px((w, h): (usize, usize)) -> f32 {
    2. * w as f32 * h as f32 / (w + h) as f32
}

/// A white ring of radius `r` about (cx, cy), `t` its half width: α = e^(1 − 1/(1 − u²)) for |u| < 1, u the distance from the ring over t (a bump, C∞ with every derivative zero where it meets 0), laid into `target` as the frontmost layer it is.
fn ring(target: &mut [u32], w: usize, h: usize, cx: f32, cy: f32, r: f32, t: f32) {
    let reach = r + t;
    // WHY/PROOF: the mark's box is the ring's reach about a point the view chose, which can sit outside the window (an anchor off the frame, or a crop panned away); the box is cut to the window so no row or column outside it is indexed.
    let (xa, xb) = ((cx - reach).floor().max(0.) as usize, ((cx + reach).ceil().max(0.) as usize).min(w));
    let (ya, yb) = ((cy - reach).floor().max(0.) as usize, ((cy + reach).ceil().max(0.) as usize).min(h));
    for y in ya..yb {
        for x in xa..xb {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let u = ((dx * dx + dy * dy).sqrt() - r) / t;
            if u * u < 1. {
                let a = (1. - 1. / (1. - u * u)).exp();
                target[y * w + x] = target[y * w + x].under(((a * 255.) as u32) << 24, BlendMode::Normal);
            }
        }
    }
}

/// `tune wallpaper --report [<seed>] [akita]`: one seed's passes on this host without a window, the attempts the survey looked at and the time each pass took, and the picture fitted to a window's region once (the page's whole path but the window).
pub fn report(args: &[String]) {
    let mut seed = Seed::counter(0);
    let mut dev = 0;
    for a in args {
        if a == "akita" {
            dev = 1;
        } else {
            seed = Seed::parse(a).unwrap_or_else(|e| fail(&e));
        }
    }
    let (name, w, h) = DEVICES[dev];
    let job = Job { serial: 1, seed: seed.bytes, w, h, knobs: Knobs::DEFAULT };
    let [h0, h1] = seed.hex();
    println!("{h0}{} on {name} {w} × {h}, {} threads", h1.trim(), rayon::current_num_threads());
    let mut last = None;
    passes(&job, &|| false, &mut |d| {
        if d.stage == 0 {
            for (a, s) in &d.seen {
                println!("  {}", attempt_line(*a, s, &d.knobs));
            }
            let p = &d.params;
            println!("kept A#{}: c = {:+.5} {:+.5}i, anchor {:+.5} {:+.5}i", p.attempt, p.c[0], p.c[1], p.anchor[0], p.anchor[1]);
        }
        println!("{} pass: {} × {} in {:.0} ms", ["quick", "first", "refined"][d.stage], d.w, d.h, d.ms);
        last = Some(d);
    });
    let d = last.unwrap_or_else(|| fail("no pass came back"));
    let t = Instant::now();
    let f = fit(&d, (0, 0, 600, 1000), false, [0; 2]);
    let mean = f.px.iter().map(|&p| 255 - (p >> 8 & 0xFF)).sum::<u32>() as f32 / f.px.len() as f32;
    println!("fitted to 600 × 1000: {} × {} in {:.0} ms, mean green code {mean:.1} of 255", f.w, f.h, t.elapsed().as_secs_f32() * 1000.);
    print!("{}", rust_source(&Knobs::DEFAULT));
}
