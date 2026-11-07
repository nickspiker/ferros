//! The look of one console circle: the vessel (Lumis's camera-button profile, made radial) and the orb inside it.
//! Every layer is built in linear VSF RGB light, encoded to `g2t` codes (tanh √L) at 16 bits, and composed front to back with fluor's `under`: the rim's rings (glass, in front), the radio's name (only while it is off), the orb, the dark body, then black.
//! Positions are in units of the circle's radius r, so the same look holds at every size; the rings are blurred analytically by the sample's footprint (a Gaussian under a box is near a wider Gaussian of the same area), so a ring under a pixel wide still antialiases.

use fluor::pixel::{Blend, BlendMode, G2t16, Pixel};
use rayon::prelude::*;
use std::f64::consts::TAU;
use vsf::colour::convert::delinearize_gamma2tanh_f32;

/// A complex number, for the Julia set and the bulbs' Newton solves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct C {
    pub re: f64,
    pub im: f64,
}

impl C {
    pub const fn new(re: f64, im: f64) -> C {
        C { re, im }
    }
    /// e^{iθ}.
    pub fn cis(t: f64) -> C {
        C::new(t.cos(), t.sin())
    }
    pub fn norm2(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
    pub fn abs(self) -> f64 {
        self.norm2().sqrt()
    }
    pub fn conj(self) -> C {
        C::new(self.re, -self.im)
    }
    pub fn scale(self, k: f64) -> C {
        C::new(self.re * k, self.im * k)
    }
    pub fn sq(self) -> C {
        self * self
    }
}

impl std::ops::Add for C {
    type Output = C;
    fn add(self, o: C) -> C {
        C::new(self.re + o.re, self.im + o.im)
    }
}
impl std::ops::Sub for C {
    type Output = C;
    fn sub(self, o: C) -> C {
        C::new(self.re - o.re, self.im - o.im)
    }
}
impl std::ops::Mul for C {
    type Output = C;
    fn mul(self, o: C) -> C {
        C::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
}
impl std::ops::Div for C {
    type Output = C;
    fn div(self, o: C) -> C {
        let n = o.norm2();
        C::new((self.re * o.re + self.im * o.im) / n, (self.im * o.re - self.re * o.im) / n)
    }
}

/// One radio's family: its console circle's true radius on husky, its flavour, and the base choices its orb starts from.
pub struct Radio {
    pub name: &'static str,
    /// The circle's radius in panel pixels at 1344 × 2992, from the display's own `corner_layout` log ("display: corners").
    pub console_r: f32,
    /// The flavour as gamma-2 values (√L of VSF RGB, white 1), from `flavour()` in payloads/display (the ring's colour, working values over 2¹⁴).
    pub flavour: [f32; 3],
    /// The picture's orientation, in turns.
    pub turn: f64,
    /// Whether c is mirrored across the real axis (the Julia set mirrored).
    pub conj: bool,
    /// Where on its bulb c sits (the attracting cycle's multiplier's argument), in turns.
    pub phase: f64,
    /// The three bands' names.
    pub bands: [&'static str; 3],
}

const fn q14(r: f32, g: f32, b: f32) -> [f32; 3] {
    [r / (1 << 14) as f32, g / (1 << 14) as f32, b / (1 << 14) as f32]
}

/// The seven radios, keys 0 to 6.
pub const RADIOS: [Radio; 7] = [
    Radio { name: "WiFi", console_r: 81., flavour: q14(0., 10918., 7075.), turn: 0., conj: false, phase: 0.5, bands: ["2.4 GHz", "5 GHz", "6 GHz"] },
    Radio { name: "LTE", console_r: 109., flavour: q14(13932., 6972., 0.), turn: 0.25, conj: false, phase: 0.25, bands: ["low", "mid", "high"] },
    Radio { name: "NR", console_r: 109., flavour: q14(13932., 6972., 0.), turn: 0.75, conj: true, phase: 0.75, bands: ["FR1 low", "FR1 mid", "FR2 mmWave"] },
    Radio { name: "Bluetooth", console_r: 67., flavour: q14(0., 0., 16384.), turn: 0.125, conj: true, phase: 0.125, bands: ["LE 1M", "LE 2M", "LE coded"] },
    Radio { name: "NFC", console_r: 33., flavour: q14(8160., 0., 16384.), turn: 0., conj: false, phase: 0., bands: ["A", "B", "F"] },
    Radio { name: "GPS", console_r: 65., flavour: q14(0., 11298., 0.), turn: 0.375, conj: false, phase: 0.625, bands: ["L1", "L5", "L1 + L5"] },
    Radio { name: "USB", console_r: 49., flavour: q14(16384., 0., 0.), turn: 0.5, conj: true, phase: 0.375, bands: ["USB 2", "USB 3", "USB 3 x2"] },
];

/// Every knob, in its own unit.
#[derive(Clone, Debug)]
pub struct Look {
    pub radio: usize,
    /// The green ring's depth from the circle's edge, in r.
    pub depth: f32,
    /// Each ring's Gaussian width σ, in r.
    pub width: f32,
    /// Channel separation: blue sits this much further out than green, red half of it further in, in r.
    pub sep: f32,
    /// The rings' peak light against the flavour's, in stops.
    pub rim_stops: f32,
    /// The body's tint, this many stops under the flavour.
    pub body_stops: f32,
    /// The flavour, gamma-2 values (√L) of VSF RGB.
    pub flavour: [f32; 3],
    pub orb: bool,
    /// 0 off, 1 searching, 2 connected.
    pub state: u32,
    /// The bulb's period: the Julia set's arms.
    pub period: u32,
    pub snr_db: f32,
    pub rssi_dbm: f32,
    pub noise_dbm: f32,
    pub band: u32,
    pub loss_pct: f32,
    pub frame: u32,
    /// Where the orb has faded to e⁻¹, in r.
    pub orb_r: f32,
    /// The Julia plane's half-width at that radius.
    pub plane: f32,
}

impl Look {
    pub fn new(radio: usize) -> Look {
        Look {
            radio,
            // Lumis's rings for a box of height h sit at 0.052h (red), 0.048h (green), 0.039h (blue): the diameter 2r for h, so green at 0.096r, blue 0.018r further out, red 0.008r further in (here half the separation, 0.009r).
            depth: 0.096,
            // Lumis's tent is about 0.009h = 0.018r across at half height: σ = FWHM / 1.665.
            width: 0.011,
            sep: 0.018,
            rim_stops: 0.,
            // Lumis's body is c²/1024 a channel against the rim's c: about two stops of code, four of light.
            body_stops: 4.,
            flavour: RADIOS[radio].flavour,
            orb: false,
            state: 2,
            period: 2,
            snr_db: 30.,
            rssi_dbm: -55.,
            noise_dbm: -92.,
            band: 1,
            loss_pct: 0.,
            frame: 0,
            orb_r: 0.75,
            plane: 1.5,
        }
    }
}

/// The light of a VSF RGB colour as the eye sees it (photopic, white 1): the display's own weights (vsf_rgb_to_photopic_f32).
fn photopic(l: [f32; 3]) -> f32 {
    0.2584 * l[0] + 0.6346 * l[1] + 0.1070 * l[2]
}

/// A gamma-2 colour turned about the grey axis by `a` radians (Rodrigues), then squared to light: the square keeps every channel's light non-negative, and the whole map is C∞ in `a`.
fn turn_hue(g: [f32; 3], a: f32) -> [f32; 3] {
    let k = 1. / 3f32.sqrt();
    let (s, c) = a.sin_cos();
    let dot = k * (g[0] + g[1] + g[2]);
    // k × g, with k = (1, 1, 1)/√3.
    let cross = [k * (g[2] - g[1]), k * (g[0] - g[2]), k * (g[1] - g[0])];
    let mut out = [0f32; 3];
    for i in 0..3 {
        let v = g[i] * c + cross[i] * s + k * dot * (1. - c);
        out[i] = v * v;
    }
    out
}

/// The nucleus of the period-p bulb on the main cardioid at internal angle 1/p: Newton on f_c^p(0) = 0, from the bulb's root e^{iθ}/2 − e^{2iθ}/4 (θ = 2π/p) pushed out along the cardioid's normal by the bulb's approximate radius sin(π/p)/p².
/// Period 1 is the cardioid itself, whose centre is 0.
pub fn nucleus(p: u32) -> C {
    if p == 1 {
        return C::new(0., 0.);
    }
    let th = TAU / p as f64;
    let root = C::cis(th).scale(0.5) - C::cis(2. * th).scale(0.25);
    // dc/dθ, then the outward normal −i·t/|t|.
    let t = C::new(0., 1.) * (C::cis(th).scale(0.5) - C::cis(2. * th).scale(0.5));
    let n = C::new(0., -1.) * t.scale(1. / t.abs());
    let rb = (std::f64::consts::PI / p as f64).sin() / (p * p) as f64;
    let mut c = root + n.scale(rb);
    for _ in 0..1 << 6 {
        let (mut z, mut dz) = (C::new(0., 0.), C::new(0., 0.));
        for _ in 0..p {
            dz = z * dz.scale(2.) + C::new(1., 0.);
            z = z.sq() + c;
        }
        let step = z / dz;
        c = c - step;
        if step.norm2() < 1e-30 {
            break;
        }
    }
    c
}

/// The c whose period-p cycle has multiplier λ, in the bulb whose nucleus is `nuc` (the bulb's interior coordinate: λ = 0 the nucleus, |λ| = 1 its boundary, |λ| > 1 just past it).
/// Newton in (z, c) on f_c^p(z) = z and (f_c^p)'(z) = λ, continued from the nucleus (where z = 0 is on the cycle) along λ in 2⁶ steps so it stays on this bulb's branch.
pub fn interior(p: u32, lam: C, nuc: C) -> C {
    let (mut z, mut c) = (C::new(0., 0.), nuc);
    let steps = 1 << 6;
    for j in 1..=steps {
        let l = lam.scale(j as f64 / steps as f64);
        for _ in 0..1 << 5 {
            // z_k, A = ∂z_k/∂z (also the cycle's multiplier at k = p), B = ∂z_k/∂c, D = ∂A/∂z, E = ∂A/∂c.
            let (mut zk, mut a, mut b, mut d, mut e) = (z, C::new(1., 0.), C::new(0., 0.), C::new(0., 0.), C::new(0., 0.));
            for _ in 0..p {
                let two_z = zk.scale(2.);
                let (na, nb) = (two_z * a, two_z * b + C::new(1., 0.));
                let (nd, ne) = ((a * a + zk * d).scale(2.), (b * a + zk * e).scale(2.));
                (a, b, d, e) = (na, nb, nd, ne);
                zk = zk.sq() + c;
            }
            let (f1, f2) = (zk - z, a - l);
            let a1 = a - C::new(1., 0.);
            let det = a1 * e - b * d;
            let dz = (f1 * e - b * f2) / det;
            let dc = (a1 * f2 - d * f1) / det;
            z = z - dz;
            c = c - dc;
            if dz.norm2() + dc.norm2() < 1e-30 {
                break;
            }
        }
    }
    c
}

/// A spark: where it is on its way from the core to the rim, and its light (a bump in its progress).
struct Spark {
    x: f32,
    y: f32,
    light: f32,
}

/// Everything one render needs, worked out once from the knobs.
pub struct Prep {
    /// Each channel's ring centre depth, in r (R, G, B).
    ring_d: [f32; 3],
    sigma: f32,
    /// Each channel's ring peak light (its own channel only).
    ring_l: [f32; 3],
    body_l: [f32; 3],
    body_edge: f32,
    orb: bool,
    c: C,
    /// Plane coordinates from r units: rotation and scale.
    rot: C,
    warp_a: f64,
    warp_k: f64,
    warp_ph: f64,
    gain: f32,
    glow_w: f32,
    /// z units to r units, for the distance estimate.
    z_to_r: f32,
    env_inv: f32,
    /// The orb's flavour as gamma-2 values with the band's turn applied, and the light normaliser that puts its photopic light at 1.
    hue_g: [f32; 3],
    hue_norm: f32,
    plasma: f32,
    sparks: Vec<Spark>,
    spark_s: f32,
}

/// Stepping a hash: SplitMix64.
fn mix(k: u64) -> u64 {
    let mut z = k.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A uniform in [0, 1) from a hash.
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

impl Prep {
    pub fn new(k: &Look) -> Prep {
        let radio = &RADIOS[k.radio];
        let fl = k.flavour.map(|g| g * g);
        // The band widens the rim's prism: half a stop of separation a band.
        let sep = k.sep * ((k.band as f32) * 0.5).exp2();
        let ring_d = [k.depth + sep * 0.5, k.depth, k.depth - sep];
        let rim = k.rim_stops.exp2();
        let body = (-k.body_stops).exp2();
        let connected = k.state == 2;
        // Searching: c walks its circle four times as fast, with a quarter of the light, no plasma and no sparks.
        let speed = if connected { 1. } else { 4. };
        let m = (1. + 1. / (1 << 4) as f64) * (-(k.snr_db as f64) / 8.).exp2();
        let phase = radio.phase + k.frame as f64 * speed / (1 << 8) as f64;
        let lam = C::cis(TAU * phase).scale(m);
        let mut c = interior(k.period, lam, nucleus(k.period));
        if radio.conj {
            c = c.conj();
        }
        let r = k.orb_r as f64;
        let rot = C::cis(TAU * radio.turn).scale(k.plane as f64 / r);
        let warp_a = ((k.noise_dbm as f64 + 90.) / 8.).exp2() / (1 << 6) as f64;
        let gain = ((k.rssi_dbm + 60.) / 10. - 2.).exp2() * if connected { 1. } else { 1. / (1 << 2) as f32 };
        let glow_w = ((k.rssi_dbm + 60.) / 16.).exp2() / (1 << 6) as f32;
        let hue_g = {
            let l = turn_hue(k.flavour, (k.band as f32 - 1.) * std::f32::consts::TAU / (1 << 4) as f32);
            l.map(|v| v.sqrt())
        };
        let hue_norm = 1. / photopic(turn_hue(hue_g, 0.));
        let mut sparks = Vec::new();
        let spark_s = 1. / (1 << 6) as f32;
        if connected {
            let n = (k.loss_pct / 100. * (1 << 7) as f32) as u64;
            let (r0, r1) = (1. / (1 << 3) as f32, 1. - k.depth - 4. * spark_s);
            for i in 0..n {
                let h = mix(i ^ 0x5A5A);
                let a = TAU * unit(h);
                let v = (1. + unit(mix(h))) / (1 << 6) as f64;
                let t = (unit(mix(h ^ 1)) + k.frame as f64 * v).fract();
                // The spark's light: a C∞ bump in t, zero at birth and at the rim, 1 halfway (e⁴ · e^{−1/(t(1−t))}); t = 0 gives e^{−∞} = 0 by IEEE, no guard needed.
                let b = (4. - 1. / (t * (1. - t))).exp() as f32;
                let rho = r0 + t as f32 * (r1 - r0);
                sparks.push(Spark { x: rho * a.cos() as f32, y: rho * a.sin() as f32, light: b });
            }
        }
        Prep {
            ring_d,
            sigma: k.width,
            ring_l: [fl[0] * rim, fl[1] * rim, fl[2] * rim],
            body_l: fl.map(|v| v * body),
            body_edge: k.depth,
            orb: k.orb && k.state != 0,
            c,
            rot,
            warp_a,
            warp_k: (1 << 3) as f64,
            warp_ph: TAU * k.frame as f64 / (1 << 7) as f64,
            gain,
            glow_w,
            z_to_r: (r / k.plane as f64) as f32,
            env_inv: 1. / (k.orb_r * k.orb_r),
            hue_g,
            hue_norm,
            plasma: if connected { (1 << 3) as f32 } else { 0. },
            sparks,
            spark_s,
        }
    }

    pub fn c(&self) -> C {
        self.c
    }
}

/// The 16-bit g2t code of a light, as a fraction (tanh √L).
fn g2t(l: f32) -> f32 {
    delinearize_gamma2tanh_f32(l)
}

/// A layer of light: codes t = g2t(L) a channel, at α = 1 − Π(1 − t), each channel's colour t/α, so over black it shows exactly t and over anything else it hides that in proportion to its own light.
/// With no light α is 0 and t/α is NaN, which the cast takes to 0: an empty pixel.
/// α is summed as t₀ + (1 − t₀)t₁ + (1 − t₀)(1 − t₁)t₂, every term non-negative: the product form 1 − Π(1 − t) cancels in f32 where the light is faint (a ring's far tail), giving α = 0 beside t > 0, and t/α = ∞ then overflowed the 16-bit field (a dark circle inside the body, seen in the first dump).
fn light_layer(l: [f32; 3]) -> G2t16 {
    let t = l.map(g2t);
    let a = t[0] + (1. - t[0]) * (t[1] + (1. - t[1]) * t[2]);
    let one = G2t16::ONE as f32;
    G2t16::from_channels((a * one) as u32, t.map(|v| G2t16::ONE - (v / a * one) as u32))
}

/// A layer of one colour at opacity α.
fn tint_layer(l: [f32; 3], a: f32) -> G2t16 {
    let one = G2t16::ONE as f32;
    G2t16::from_channels((a * one) as u32, l.map(|v| G2t16::ONE - (g2t(v) * one) as u32))
}

/// Opaque black, the background.
const BLACK: G2t16 = u64::MAX;

/// One sample at (x, y) in r units from the centre, `fs` the sample's footprint in r, `text` the name's layer (a composite) at this pixel: every layer front to back from empty.
fn sample(p: &Prep, x: f32, y: f32, fs: f32, text: G2t16) -> G2t16 {
    let rho2 = x * x + y * y;
    let d = 1. - rho2.sqrt();
    // The rings: each channel a Gaussian at its own depth, widened by the footprint's box (variance fs²/12) with its area kept.
    let se = (p.sigma * p.sigma + fs * fs / 12.).sqrt();
    // The circle's own edge: a logistic (C∞) about a footprint wide at d = 0 (plus an eighth of σ, so it never becomes a step), so a ring pushed out by a wide separation is cut there and nothing is drawn outside.
    let ew = (fs * fs + p.sigma * p.sigma / (1 << 6) as f32).sqrt();
    let edge = 1. / (1. + (-d * 2. / ew).exp());
    let amp = p.sigma / se * edge;
    let ring = [0, 1, 2].map(|c| {
        let u = (d - p.ring_d[c]) / se;
        p.ring_l[c] * amp * (-u * u).exp()
    });
    let mut acc: G2t16 = 0;
    acc = acc.under(light_layer(ring), BlendMode::Normal);
    acc = acc.under_premult(text, BlendMode::Normal);
    if p.orb {
        acc = acc.under(light_layer(orb(p, x, y, rho2, fs)), BlendMode::Normal);
    }
    // The body: its tint, opaque but for 2⁻⁴ of the background, rising by a logistic (C∞) under the green ring, so nothing is drawn outside.
    let body_a = (1. - 1. / (1 << 4) as f32) / (1. + (-(d - p.body_edge) * 2. / se).exp());
    acc = acc.under(tint_layer(p.body_l, body_a), BlendMode::Normal);
    acc.under(BLACK, BlendMode::Normal)
}

/// The orb's light at (x, y): the Julia set z ← z² + c of the prepared c, its filaments by the distance estimate, its interior a plasma lit at the critical point's preimages (an orbit trap at 0, summed with falling weights), sparks, all under a C∞ envelope that ends before the rim.
fn orb(p: &Prep, x: f32, y: f32, rho2: f32, fs: f32) -> [f32; 3] {
    let n_max = 1 << 8;
    let bail = (1 << 8) as f64;
    let w = C::new(x as f64, y as f64) * p.rot;
    // Turbulence: a small smooth warp of z₀, its phase stepped per frame.
    let w = C::new(w.re + p.warp_a * (p.warp_k * w.im + p.warp_ph).sin(), w.im + p.warp_a * (p.warp_k * w.re + p.warp_ph).cos());
    let (mut z, mut dz) = (w, C::new(1., 0.));
    let (mut trap, mut wk) = (0f64, 1f64);
    let decay = (-1f64 / (1 << 3) as f64).exp2();
    let trap_inv = (1 << 6) as f64;
    let mut escaped = None;
    for n in 0..n_max {
        let r2 = z.norm2();
        // Past 2⁴ the term is under e⁻¹⁶, below a 16-bit code's step; skipping it saves the exp, it shapes nothing.
        let u = r2 * trap_inv;
        if u < 16. {
            trap += wk * (-u).exp();
        }
        if r2 > bail {
            escaped = Some(n);
            break;
        }
        dz = (z * dz).scale(2.);
        z = z.sq() + p.c;
        wk *= decay;
    }
    let trap = (trap * (1. - decay)) as f32 * p.plasma;
    let (glow, mu) = match escaped {
        Some(n) => {
            let az = z.abs();
            let de = (az * az.ln() / dz.abs()) as f32 * p.z_to_r;
            let mu = n as f64 + 1. - az.ln().ln() / std::f64::consts::LN_2;
            let u = de / p.glow_w;
            let g = 1. / (1. + u * u);
            (g * g, mu as f32)
        }
        // The bounded interior sits a stop under the filaments, so the set's boundary reads as the bright edge; the step there is the Julia set's own boundary (a fractal, with no smooth version), softened only by the samples.
        None => (1. / (1 << 1) as f32, n_max as f32),
    };
    // The filaments' hue drifts with the smooth escape count, a sixteenth of a turn either way.
    let col = turn_hue(p.hue_g, (mu / (1 << 3) as f32).sin() * std::f32::consts::TAU / (1 << 4) as f32).map(|v| v * p.hue_norm);
    let e = rho2 * p.env_inv;
    let env = (-(e * e) * (e * e)).exp() * p.gain;
    let white = glow * glow * glow * glow / (1 << 2) as f32 + trap * trap * trap;
    let mut l = col.map(|v| env * (v * (glow + trap) + white));
    // Sparks: Gaussian points of white-flavoured light, widened by the footprint like the rings.
    let s2 = p.spark_s * p.spark_s + fs * fs / 12.;
    let k = p.spark_s * p.spark_s / s2 * p.gain;
    for s in &p.sparks {
        let (dx, dy) = (x - s.x, y - s.y);
        let b = s.light * k * (-(dx * dx + dy * dy) / s2).exp();
        for i in 0..3 {
            l[i] += b * (col[i] + 1.);
        }
    }
    l
}

/// One circle of radius `r` pixels in a square of side 2⌈r⌉ + 4, `ss` × `ss` samples a pixel (ss a power of two), with the name's layer `text` (a composite of the same size, or empty).
pub fn render(p: &Prep, r: f32, ss: u32, text: &[G2t16]) -> (Vec<G2t16>, usize) {
    let side = 2 * r.ceil() as usize + 4;
    let mid = side as f32 * 0.5;
    let fs = 1. / (r * ss as f32);
    let shift = 2 * ss.trailing_zeros();
    let mut out = vec![0u64; side * side];
    out.par_chunks_mut(side).enumerate().for_each(|(j, row)| {
        for (i, px) in row.iter_mut().enumerate() {
            let t = if text.is_empty() { 0 } else { text[j * side + i] };
            let mut sum = [0u64; 3];
            for b in 0..ss {
                for a in 0..ss {
                    let x = (i as f32 + (a as f32 + 0.5) / ss as f32 - mid) / r;
                    let y = (j as f32 + (b as f32 + 0.5) / ss as f32 - mid) / r;
                    let s = sample(p, x, y, fs, t).channels();
                    for c in 0..3 {
                        sum[c] += s[c] as u64;
                    }
                }
            }
            *px = G2t16::from_channels(G2t16::ONE, sum.map(|v| (v >> shift) as u32));
        }
    });
    (out, side)
}

/// The radio's name, when it is off: its flavour's colour a stop over the ring's, centred in a square of side `side`, as a composite layer.
pub fn name_layer(text: &mut fluor::text::TextRenderer, k: &Look, r: f32, side: usize) -> Vec<G2t16> {
    let mut buf = vec![0u64; side * side];
    if k.state != 0 || !k.orb {
        return buf;
    }
    let name = RADIOS[k.radio].name;
    let fl = k.flavour.map(|g| g * g * 2.);
    // An 8-bit α + darkness colour for the text style: the name's codes, opaque.
    let code = fl.map(|v| (g2t(v) * 256.) as u32);
    let colour = 0xFF00_0000 | (255 - code[0]) << 16 | (255 - code[1]) << 8 | (255 - code[2]);
    let mut damage = fluor::canvas::Damage::new();
    let mut canvas = fluor::canvas::Canvas::new(&mut buf[..], side, side, &mut damage);
    // The size: the name's width a little over the circle's radius, whatever its length.
    let probe = fluor::text::TextStyle::new(r, colour);
    let wide = text.measure_text(name, &probe);
    let size = r * r * 1.25 / wide;
    let style = fluor::text::TextStyle::new(size, colour);
    let mid = side as f32 * 0.5;
    text.draw_text_center(&mut canvas, name, mid, mid, &style, None, None);
    buf
}
