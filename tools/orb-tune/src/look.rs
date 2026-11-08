//! The look of one console circle: the vessel (Lumis's camera-button profile, made radial) and the orb inside it.
//! Every layer is built in linear VSF RGB light, encoded to `g2t` codes (tanh √L) at 16 bits, and composed front to back with fluor's `under`: the rim's rings (glass, in front), the radio's name (only while it is off), the orb, the dark body, then black.
//! GPS draws the sky dome (sky.rs) in the orb's place, and its map pin in the name's.
//! Positions are in units of the circle's radius r, so the same look holds at every size; the rings are blurred analytically by the sample's footprint (a Gaussian under a box is near a wider Gaussian of the same area), so a ring under a pixel wide still antialiases.

use fluor::pixel::{Blend, BlendMode, G2t16, Pixel};
use rayon::prelude::*;
use std::f64::consts::TAU;
use vsf::colour::convert::delinearize_gamma2tanh_f32;
use crate::sky::{self, Sky};

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
    /// A typical channel width, MHz, and constellation, bits a symbol (the knobs' starting points for this radio).
    pub bw_mhz: f32,
    pub qam_bits: u32,
}

const fn q14(r: f32, g: f32, b: f32) -> [f32; 3] {
    [r / (1 << 14) as f32, g / (1 << 14) as f32, b / (1 << 14) as f32]
}

/// The seven radios, keys 0 to 6.
/// The GPS radio's index, whose orb is the sky dome.
pub const GPS: usize = 5;

pub const RADIOS: [Radio; 7] = [
    Radio { name: "WiFi", console_r: 81., flavour: q14(0., 10918., 7075.), turn: 0., conj: false, phase: 0.5, bands: ["2.4 GHz", "5 GHz", "6 GHz"], bw_mhz: 80., qam_bits: 8 },
    Radio { name: "LTE", console_r: 109., flavour: q14(13932., 6972., 0.), turn: 0.25, conj: false, phase: 0.25, bands: ["low", "mid", "high"], bw_mhz: 20., qam_bits: 6 },
    Radio { name: "NR", console_r: 109., flavour: q14(13932., 6972., 0.), turn: 0.75, conj: true, phase: 0.75, bands: ["FR1 low", "FR1 mid", "FR2 mmWave"], bw_mhz: 100., qam_bits: 8 },
    Radio { name: "Bluetooth", console_r: 67., flavour: q14(0., 0., 16384.), turn: 0.125, conj: true, phase: 0.125, bands: ["LE 1M", "LE 2M", "LE coded"], bw_mhz: 2., qam_bits: 2 },
    Radio { name: "NFC", console_r: 33., flavour: q14(8160., 0., 16384.), turn: 0., conj: false, phase: 0., bands: ["A", "B", "F"], bw_mhz: 1., qam_bits: 2 },
    Radio { name: "GPS", console_r: 65., flavour: q14(0., 11298., 0.), turn: 0.375, conj: false, phase: 0.625, bands: ["L1", "L5", "L1 + L5"], bw_mhz: 2., qam_bits: 2 },
    Radio { name: "USB", console_r: 49., flavour: q14(16384., 0., 0.), turn: 0.5, conj: true, phase: 0.375, bands: ["USB 2", "USB 3", "USB 3 x2"], bw_mhz: 512., qam_bits: 2 },
];

/// Every knob, in its own unit.
#[derive(Clone, Debug)]
pub struct Look {
    pub radio: usize,
    /// The green ring's depth from the circle's edge, in r.
    pub depth: f32,
    /// Each ring's inner tail, in r: inward of its peak it fades e-fold every λ (Lumis's v = k³w⁹ moves e-fold every 0.0063 of the button's height, 0.0126 r).
    pub width: f32,
    /// How hard each ring drops on its outer side, toward the edge: the profile exp(−s(e^{t/s} − 1 − t/s)), t the depth above the peak in λ; small s a cliff (at s = ¼ it is down to 0.5 % where Lumis's reaches zero, 0.81 λ out), s = 1 as soft out as in.
    pub sharp: f32,
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
    /// The channel's width, log₂ of MHz: how far the phase swings the hue.
    pub bw_log2: f32,
    /// The constellation's bits a symbol (QAM 2^bits points on a square lattice).
    pub qam_bits: u32,
    pub loss_pct: f32,
    /// Multipath delay spread, ns: the interference fringes' spatial frequency.
    pub delay_ns: f32,
    pub frame: u32,
    /// Where the orb has faded to e⁻¹, in r.
    pub orb_r: f32,
    /// The Julia plane's half-width at that radius.
    pub plane: f32,
    /// The soft knee's light, stops from white: the orb's light L becomes W·L/(W + L), so nothing reaches a ceiling.
    pub knee_stops: f32,
    /// GPS: tracked satellites a system (GPS, Galileo, GLONASS, BeiDou, QZSS, NavIC), from the seed's sky.
    pub sats: [u32; 6],
    /// The seed that places the satellites (azimuth, elevation, C/N0 noise, GLONASS channel, L5).
    pub seed: u32,
    /// C/N0's spread about its elevation trend, dB.
    pub cn0_spread: f32,
    /// How many of the strongest are used in the fix.
    pub used: u32,
    /// The fraction of the satellites whose system has an L5 signal that carry one.
    pub l5_frac: f32,
    /// The horizontal error estimate, m (the driver's ACCURACY, in mm there).
    pub herr_m: f32,
    /// The open sky's sector, turns: 1 the whole sky, small an urban canyon (the satellites' azimuths squeezed into it, so the geometry, and HDOP, worsens).
    pub sector: f32,
    /// Privacy: the dome turned by this secret angle (turns), and mirrored.
    pub secret_turn: f32,
    pub mirror: bool,
    /// The colour map's stretch about L1 (γ in `sky::carrier_nm`).
    pub stretch: f32,
}

impl Look {
    pub fn new(radio: usize) -> Look {
        Look {
            radio,
            // Lumis's rings for a box of height h sit at 0.052h (red), 0.048h (green), 0.039h (blue): the diameter 2r for h, so green at 0.096r, blue 0.018r further out, red 0.008r further in (here half the separation, 0.009r).
            depth: 0.096,
            width: 0.0126,
            sharp: 0.25,
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
            bw_log2: RADIOS[radio].bw_mhz.log2(),
            qam_bits: RADIOS[radio].qam_bits,
            loss_pct: 0.,
            delay_ns: (1 << 5) as f32,
            frame: 0,
            orb_r: 0.75,
            plane: 1.5,
            knee_stops: -1.,
            sats: [8, 6, 5, 6, 2, 0],
            seed: 0,
            cn0_spread: 6.,
            used: 12,
            l5_frac: 0.5,
            herr_m: 4.,
            sector: 1.,
            secret_turn: 0.,
            mirror: false,
            stretch: 4.,
        }
    }
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
/// Returns c and the cycle's point z it converged on.
/// Newton in (z, c) on f_c^p(z) = z and (f_c^p)'(z) = λ, continued from the nucleus (where z = 0 is on the cycle) along λ in 2⁶ steps so it stays on this bulb's branch.
pub fn interior(p: u32, lam: C, nuc: C) -> (C, C) {
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
    (c, z)
}

/// A spark: where it is on its way from the core to the rim, and its light (a bump in its progress).
struct Spark {
    x: f32,
    y: f32,
    light: f32,
}

/// A plane wave across the orb in r units: wavenumber (kx, ky) and phase.
#[derive(Clone, Copy)]
struct Wave {
    kx: f32,
    ky: f32,
    ph: f32,
}

impl Wave {
    fn at(&self, x: f32, y: f32) -> f32 {
        self.kx * x + self.ky * y + self.ph
    }
}

/// The orbit is iterated at most 2¹⁰ times, and its sums (nuclei, I/Q phasor, constellation) run over its first 2⁵ points, the same count for every pixel so the sums never step where the escape or settle count does.
const N_MAX: u32 = 1 << 10;
const N_SUM: u32 = 1 << 5;

/// The light of VSF grey at the console ring's working value 9000 (Q14): the flavours are at this photopic light (or their brightest channel full), so the orb takes it as its unit.
pub(crate) const RING_LIGHT: f32 = (9000. / (1 << 14) as f32) * (9000. / (1 << 14) as f32);

/// e^{−κ}I₀(κ), the mean of e^{κ(cos θ − 1)} over a turn (von Mises), by I₀'s series: normalises a constellation point so a sharp one is bright and a smeared one a dim haze of the same symbols.
fn vm_mean(kappa: f64) -> f64 {
    let (mut term, mut sum) = (1f64, 1f64);
    let q = kappa * kappa / 4.;
    for j in 1..1 << 8 {
        term *= q / (j * j) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    (-kappa).exp() * sum
}

/// Everything one render needs, worked out once from the knobs.
pub struct Prep {
    /// Each channel's ring centre depth, in r (R, G, B).
    ring_d: [f32; 3],
    lambda: f32,
    sharp: f32,
    /// Each channel's ring peak light (its own channel only).
    ring_l: [f32; 3],
    body_l: [f32; 3],
    body_edge: f32,
    orb: bool,
    c: C,
    /// The attracting cycle's point the orbits are tested against, the cycle's multiplier's argument, and its period.
    z_star: C,
    lam_arg: f64,
    /// ln(1/|λ|): how many e-folds the cycle pulls an orbit in a period (positive while it attracts).
    lam_ln: f64,
    period: f64,
    /// Plane coordinates from r units: rotation and scale.
    rot: C,
    warp_a: f64,
    warp_k: f64,
    warp_ph: f64,
    gain: f32,
    glow_w: f32,
    /// z units to r units, for the distance estimates.
    z_to_r: f32,
    env_inv: f32,
    /// The orb's flavour as gamma-2 values with the band's turn applied, and the light that puts the flavour's own light at the unit.
    hue_g: [f32; 3],
    hue_norm: f32,
    /// The I/Q phase's hue swing (radians at full phase), and the carrier's rotation this frame.
    swing: f32,
    carrier: C,
    /// Noise grain in the hue: amplitude (radians) and three fine waves.
    grain_a: f32,
    grain: [Wave; 3],
    /// Multipath: two echoes' amplitudes and waves, and the mean power that normalises the fringes.
    echo_a: [f32; 2],
    echo: [Wave; 2],
    echo_norm: f32,
    /// The constellation: lattice spacing and half-extent (z units), sharpness κ, and the gain that normalises a point by its mean.
    qam_d: f64,
    qam_half: f64,
    qam_k: f64,
    qam_gain: f32,
    plasma: f32,
    knee: f32,
    sparks: Vec<Spark>,
    spark_s: f32,
    /// GPS: the sky dome in place of the Julia orb while the radio is on, and the map pin while it is off.
    sky: Option<Sky>,
    pin: bool,
}

/// Stepping a hash: SplitMix64.
pub(crate) fn mix(k: u64) -> u64 {
    let mut z = k.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A uniform in [0, 1) from a hash.
pub(crate) fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

impl Prep {
    pub fn new(k: &Look) -> Prep {
        let radio = &RADIOS[k.radio];
        let fl = k.flavour.map(|g| g * g);
        let tau = std::f32::consts::TAU;
        let frame = k.frame as f32;
        // The band widens the rim's prism: half a stop of separation a band.
        let sep = k.sep * ((k.band as f32) * 0.5).exp2();
        let ring_d = [k.depth + sep * 0.5, k.depth, k.depth - sep];
        let rim = k.rim_stops.exp2();
        let body = (-k.body_stops).exp2();
        let connected = k.state == 2;
        // Searching: c walks its circle four times as fast, with a quarter of the light, no plasma, no constellation and no sparks.
        let speed = if connected { 1. } else { 4. };
        let m = (1. + 1. / (1 << 4) as f64) * (-(k.snr_db as f64) / 8.).exp2();
        let phase = radio.phase + k.frame as f64 * speed / (1 << 8) as f64;
        let mut lam = C::cis(TAU * phase).scale(m);
        let (mut c, mut z_star) = interior(k.period, lam, nucleus(k.period));
        if radio.conj {
            (c, z_star, lam) = (c.conj(), z_star.conj(), lam.conj());
        }
        let r = k.orb_r as f64;
        let rot = C::cis(TAU * radio.turn).scale(k.plane as f64 / r);
        let warp_a = ((k.noise_dbm as f64 + 90.) / 8.).exp2() / (1 << 6) as f64;
        let gain = ((k.rssi_dbm + 60.) / 10. - 1.).exp2() * if connected { 1. } else { 1. / (1 << 2) as f32 };
        let glow_w = ((k.rssi_dbm + 60.) / 16.).exp2() / (1 << 6) as f32;
        let hue_g = {
            let l = turn_hue(k.flavour, (k.band as f32 - 1.) * tau / (1 << 4) as f32);
            l.map(|v| v.sqrt())
        };
        // Bandwidth: a 20 MHz channel swings the hue 2⁻⁵ of a turn either way, nearly one colour; 160 MHz a quarter turn, 320 MHz half.
        let swing = tau * k.bw_log2.exp2() / 20. / (1 << 5) as f32;
        // The carrier turns the I/Q plane 2⁻⁵ of a turn a frame.
        let carrier = C::cis(TAU * k.frame as f64 / (1 << 5) as f64);
        // Noise: 2⁻⁶ of a turn of grain at −90 dBm, a stop more every 8 dB; three fine waves (2⁵ to 2⁵ + 2⁴ cycles a radius), drifting a frame at a time.
        let grain_a = tau * ((k.noise_dbm + 90.) / 8.).exp2() / (1 << 6) as f32;
        let grain = [(0., 0.), (1. / 3., 1. / 8.), (2. / 3., 1. / 4.)].map(|(turn, extra): (f32, f32)| {
            let kk = tau * (1 << 5) as f32 * (1. + extra);
            Wave { kx: kk * (tau * turn).cos(), ky: kk * (tau * turn).sin(), ph: frame * tau * (1. / (1 << 3) as f32 + extra) }
        });
        // Multipath: two echoes at half and a quarter amplitude, their fringes one cycle a radius per 2⁶ ns of delay spread, Doppler-shifted a frame at a time (2⁻⁶ and 3·2⁻⁷ of a turn).
        let echo_a = [0.5, 0.25];
        let kd = tau * k.delay_ns / (1 << 6) as f32;
        let t0 = radio.turn as f32;
        let echo = [(t0 + 1. / 8., 1., 1. / (1 << 6) as f32), (t0 + 5. / 8., 1.5, 3. / (1 << 7) as f32)].map(|(turn, scale, dop): (f32, f32, f32)| Wave {
            kx: kd * scale * (tau * turn).cos(),
            ky: kd * scale * (tau * turn).sin(),
            ph: tau * dop * frame,
        });
        let echo_norm = 1. / (1. + echo_a[0] * echo_a[0] + echo_a[1] * echo_a[1]);
        // The constellation: 2^bits points, √M a side, on [−1, 1]² of the plane; each point's spread σ is the noise's amplitude over the signal's, 2^(−SNR/6 dB) of the spacing (10^(−SNR/20)), so κ = 1/(2πσ)².
        let side = (1u32 << (k.qam_bits / 2)) as f64;
        let qam_half = 1.;
        let qam_d = 2. * qam_half / side;
        let sig = (-(k.snr_db as f64) / 6.).exp2();
        let qam_k = 1. / (TAU * sig * TAU * sig);
        let qam_gain = if connected { (1. / ((1 << 3) as f64 * vm_mean(qam_k) * vm_mean(qam_k))) as f32 } else { 0. };
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
            lambda: k.width,
            sharp: k.sharp,
            ring_l: [fl[0] * rim, fl[1] * rim, fl[2] * rim],
            body_l: fl.map(|v| v * body),
            body_edge: k.depth,
            orb: k.orb && k.state != 0,
            c,
            z_star,
            lam_arg: lam.im.atan2(lam.re),
            lam_ln: -lam.abs().ln(),
            period: k.period as f64,
            rot,
            warp_a,
            warp_k: (1 << 3) as f64,
            warp_ph: TAU * k.frame as f64 / (1 << 7) as f64,
            gain,
            glow_w,
            z_to_r: (r / k.plane as f64) as f32,
            env_inv: 1. / (k.orb_r * k.orb_r),
            hue_g,
            hue_norm: 1. / RING_LIGHT,
            swing,
            carrier,
            grain_a,
            grain,
            echo_a,
            echo,
            echo_norm,
            qam_d,
            qam_half,
            qam_k,
            qam_gain,
            plasma: if connected { (1 << 3) as f32 } else { 0. },
            knee: k.knee_stops.exp2(),
            sparks,
            spark_s,
            sky: (k.radio == GPS && k.orb && k.state != 0).then(|| Sky::new(k)),
            pin: k.radio == GPS && k.orb && k.state == 0,
        }
    }

    pub fn c(&self) -> C {
        self.c
    }

    /// The constellation's trap at z: a von Mises bump at every lattice point (C∞ and periodic), the lattice's square extent a super-Gaussian (x⁸), all ≤ 1.
    fn qam(&self, z: C) -> f64 {
        let (u, v) = ((z.re + self.qam_half) / self.qam_d - 0.5, (z.im + self.qam_half) / self.qam_d - 0.5);
        let (ex, ey) = (z.re / self.qam_half, z.im / self.qam_half);
        let (ex2, ey2) = (ex * ex, ey * ey);
        let edge = (ex2 * ex2) * (ex2 * ex2) + (ey2 * ey2) * (ey2 * ey2);
        (self.qam_k * ((TAU * u).cos() + (TAU * v).cos() - 2.) - edge).exp()
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
fn sample(p: &Prep, x: f32, y: f32, fs: f32, px: f32, text: G2t16) -> G2t16 {
    let rho2 = x * x + y * y;
    let d = 1. - rho2.sqrt();
    // The rings, Lumis's shape made smooth: each channel peaks at its own depth, drops off a cliff on its outer side and fades e-fold every λ on its inner, exp(−s(e^{t/s} − 1 − t/s)) with t the depth above the peak in λ (C∞, its peak rounded where Lumis's is a point). The footprint widens λ by its box (variance fs²/12, the ring's light kept), and s so the cliff is never under a footprint.
    let le = (p.lambda * p.lambda + fs * fs / 12.).sqrt();
    let s = (p.sharp * p.sharp + fs * fs / (le * le * 12.)).sqrt();
    // The circle's own edge: a logistic (C∞) about a footprint wide at d = 0 (plus an eighth of λ, so it never becomes a step), so a ring pushed out by a wide separation is cut there and nothing is drawn outside.
    let ew = (fs * fs + p.lambda * p.lambda / (1 << 6) as f32).sqrt();
    let edge = 1. / (1. + (-d * 2. / ew).exp());
    let amp = p.lambda / le * edge;
    let ring = [0, 1, 2].map(|c| {
        let t = (p.ring_d[c] - d) / le;
        p.ring_l[c] * amp * (-s * ((t / s).exp() - 1. - t / s)).exp()
    });
    let mut acc: G2t16 = 0;
    acc = acc.under(light_layer(ring), BlendMode::Normal);
    acc = acc.under_premult(text, BlendMode::Normal);
    if p.pin {
        // GPS off: the map pin in the ink, its coverage the display's C∞ step across two pixels of its signed distance.
        let cov = sky::bump_step((1. - sky::pin_sd(x, y) / px) * 0.5);
        acc = acc.under(tint_layer([sky::PIN_INK; 3], cov), BlendMode::Normal);
    }
    if let Some(sky) = &p.sky {
        // GPS on: the dome's four layers front to back, the satellites, the knot, the range arcs, the sky; then the night, black at the dome's opacity, before the body.
        let (lights, night) = sky.light(x, y, fs);
        for l in lights {
            acc = acc.under(light_layer(l), BlendMode::Normal);
        }
        acc = acc.under(tint_layer([0.; 3], night), BlendMode::Normal);
    } else if p.orb {
        acc = acc.under(light_layer(orb(p, x, y, rho2, fs)), BlendMode::Normal);
    }
    // The body: its tint, opaque but for 2⁻⁴ of the background, rising by a logistic (C∞) under the green ring, so nothing is drawn outside.
    let body_a = (1. - 1. / (1 << 4) as f32) / (1. + (-(d - p.body_edge) * 2. / le).exp());
    acc = acc.under(tint_layer(p.body_l, body_a), BlendMode::Normal);
    acc.under(BLACK, BlendMode::Normal)
}

/// The orb's light at (x, y), with the Julia plane read as the link's I/Q plane.
/// The orbit z ← z² + c from the warped z₀ either escapes, settles onto the attracting cycle (tested at one of its points, z*), or neither within 2¹⁰ steps.
/// Each side of the set's boundary has its own measure that runs to the boundary smoothly: outside the distance estimate |z| ln|z| / |dz|, falling to 0 at the boundary; inside the smooth settle time s = n − p·ln(ε/|w|)/ln(1/|λ|) (w = z − z*), which the cycle's linear part keeps continuous where the settle count steps by a period, and which grows without bound toward the boundary.
/// The edge glow is a C∞ function of each, 1 at the boundary from both sides (the undecided count as boundary), so the set has no hard edge.
/// Inside, away from the glow: a dim plasma with veins along the Koenigs phase (arg w less the cycle's turning, continuous the same way), and white-hot nuclei where orbits settle fastest (2^(−s/2), the preimages of the cycle).
/// Over all of it: the orbit's I/Q phasor turns the hue around the flavour by the bandwidth's swing, noise adds grain to that hue, the constellation trap lights where orbits pass the QAM lattice, multipath fringes modulate it, and the summed light goes through a soft knee.
fn orb(p: &Prep, x: f32, y: f32, rho2: f32, fs: f32) -> [f32; 3] {
    let bail = (1 << 8) as f64;
    let eps = 1. / (1u64 << 20) as f64;
    let w0 = C::new(x as f64, y as f64) * p.rot;
    // Turbulence: a small smooth warp of z₀, its phase stepped per frame.
    let w0 = C::new(w0.re + p.warp_a * (p.warp_k * w0.im + p.warp_ph).sin(), w0.im + p.warp_a * (p.warp_k * w0.re + p.warp_ph).cos());
    let (mut z, mut dz) = (w0, C::new(1., 0.));
    let (mut iq, mut qam, mut wk) = (C::new(0., 0.), 0f64, 1f64);
    let decay = (-1f64 / (1 << 3) as f64).exp2();
    // Outside: the distance estimate (z units). Inside: the settle time and the Koenigs phase.
    enum Fate {
        Out(f64),
        In(f64, f64),
    }
    let mut fate = None;
    for n in 0..N_MAX {
        let r2 = z.norm2();
        if n < N_SUM {
            iq = iq + z.scale(wk * (-r2 / 4.).exp());
            // Past |z| = 2 the lattice's extent term is under e⁻²⁵⁶: skipping it shapes nothing.
            if r2 < 4. {
                qam += wk * p.qam(z);
            }
            wk *= decay;
        }
        if fate.is_none() {
            if r2 > bail {
                let az = r2.sqrt();
                fate = Some(Fate::Out(az * az.ln() / dz.abs()));
                // Escaped: every sum's later terms are under e⁻⁶⁴ (|z| > 2⁴), so stopping here changes none of them.
                break;
            }
            let w = z - p.z_star;
            let aw = w.abs();
            if aw < eps {
                let s = n as f64 - p.period * (eps / aw).ln() / p.lam_ln;
                let koenigs = w.im.atan2(w.re) - n as f64 / p.period * p.lam_arg;
                fate = Some(Fate::In(s, koenigs));
            }
        } else if n >= N_SUM {
            break;
        }
        dz = (z * dz).scale(2.);
        z = z.sq() + p.c;
    }
    let on = p.plasma / (1 << 3) as f32;
    let (glow, body, nuc) = match fate {
        Some(Fate::Out(d)) => {
            let u = d as f32 * p.z_to_r / p.glow_w;
            let g = 1. / (1. + u * u);
            (g * g, 0., 0.)
        }
        Some(Fate::In(s, ph)) => {
            // The inside's glow: q⁴/(1 + q⁴) of the settle time in units of 2⁴ periods, 0 at the nuclei and 1 toward the boundary.
            let q = (s / (p.period * (1 << 4) as f64)) as f32;
            let q4 = (q * q) * (q * q);
            let g = q4 / (1. + q4);
            // Between: a plasma 2⁻⁴, veins along the Koenigs phase (p of them around each nucleus) to 2⁻³ over it, fading into the glow.
            let vein = ((1 << 3) as f64 * ((ph * p.period).cos() - 1.)).exp() as f32;
            let body = (1. - g) * (1. / (1 << 4) as f32 + vein / (1 << 3) as f32) * on;
            (g, body, (-(s as f32) / 2.).exp2() * on)
        }
        None => (1., 0., 0.),
    };
    let qam = (qam * (1. - decay)) as f32 * p.qam_gain;
    // The phase: the orbit's I/Q phasor turned by the carrier, its quadrature over its length (softened by 2⁻⁶ so it is smooth through zero), times the bandwidth's swing; plus the noise's grain.
    let iq = iq * p.carrier;
    let quad = (iq.im / (iq.norm2() + 1. / (1 << 12) as f64).sqrt()) as f32;
    let grain = p.grain.iter().map(|w| w.at(x, y).sin()).sum::<f32>() / 3.;
    // Multipath: |1 + Σ aⱼ e^{iθⱼ}|² over its mean, and its phase nudging the hue a little.
    let (mut fr, mut fi) = (1f32, 0f32);
    for (a, w) in p.echo_a.iter().zip(&p.echo) {
        let (s, c) = w.at(x, y).sin_cos();
        fr += a * c;
        fi += a * s;
    }
    let fade = (fr * fr + fi * fi) * p.echo_norm;
    let hue = p.swing * quad + p.grain_a * grain + fi / (1 << 3) as f32;
    let col = turn_hue(p.hue_g, hue).map(|v| v * p.hue_norm);
    let e = rho2 * p.env_inv;
    let env = (-(e * e) * (e * e)).exp() * p.gain * fade;
    let white = glow * glow * glow * glow / (1 << 3) as f32 + nuc * nuc * nuc;
    let mut l = col.map(|v| env * (v * (glow + body + nuc + qam) + white));
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
    // The soft knee: W·L/(W + L), C∞ for L ≥ 0, below W however hot the core.
    l.map(|v| p.knee * v / (p.knee + v))
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
                    let s = sample(p, x, y, fs, 1. / r, t).channels();
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
    // GPS shows its symbol, the pin (drawn in `sample`), not its name.
    if k.state != 0 || !k.orb || k.radio == GPS {
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

