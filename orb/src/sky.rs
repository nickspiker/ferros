//! The GPS orb (owner, 2026-11-07): the sky seen from below, built from what the receiver knows.
//! Zenith at the centre, the horizon inside the vessel's body, by Lambert's azimuthal equal-area projection: ρ = ρ_h·√2·sin(z/2), z the zenith angle.
//! Equal-area, not stereographic: every patch of sky keeps its area on the dome, so a crowded sector looks crowded and the horizon is not swollen.
//! Each tracked satellite is a light at its azimuth and elevation: its light its C/N0, its colour the frequency it is heard on, its shape the spectrum of its code.
//! The fix is the knot where the used satellites' range arcs cross, its size the horizontal error estimate, its shape the geometry's error ellipse.
//! GPS time is a faint ripple through the dome; the whole dome is turned by a secret angle and optionally mirrored, and no satellite IDs are drawn.
//! In the tool the satellites are synthetic (the driver does not decode them yet): a seed places them, the knobs set how many, how strong, how many in the fix.
//! On the device there are none yet: the dome alone while searching, and while fixed a round knot the size of the driver's error estimate.

// The float maths without std (see `Fm`); unused where std is in the build.
#[allow(unused_imports)]
use crate::Fm;
use crate::look::{Look, RING_LIGHT, mix, unit};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::f32::consts::TAU;
use vsf::colour::convert::apply_matrix_3x3_f32;
use vsf::colour::spectral::constants::{LMS_2000_10DEG_1NM, LMS2VSF_RGB};

/// The carrier every colour is pinned to, MHz: GPS L1 C/A, Galileo E1 and QZSS L1 share it, so they share its hue, VSF green.
const L1_MHZ: f64 = 1575.42;
/// The L5 family's carrier (GPS L5, Galileo E5a, QZSS L5, BeiDou B2a, NavIC L5).
const L5_MHZ: f64 = 1176.45;
/// GLONASS L1 FDMA: 1602 MHz + k·0.5625 MHz, k = −7…+6 a satellite.
const GLO_L1_MHZ: f64 = 1602.;
const GLO_STEP_MHZ: f64 = 0.5625;
/// VSF's primaries, nm: green is where L1 lands, red and blue the ends the map runs toward.
const NM_R: f64 = 703.;
const NM_G: f64 = 523.;
const NM_B: f64 = 462.;
/// The C/A code's chip rate, Mchip/s: the unit of a light's width.
const CA_MCPS: f32 = 1.023;

/// Whether a system's satellites carry an L5-family signal the receiver tracks.
#[derive(Clone, Copy, PartialEq)]
enum L5 {
    Never,
    /// As many as the L5-capable fraction says.
    Some,
    Always,
}

/// One system: where its main signal sits, its code's chip rate (the main lobe's width), whether that code splits its spectrum into two lobes (BOC), and its L5 family signal.
struct System {
    name: &'static str,
    /// The main signal's carrier, MHz; GLONASS's is per satellite (0 here, worked out from k).
    mhz: f64,
    mcps: f32,
    boc: bool,
    l5: L5,
    l5_mcps: f32,
}

/// The six systems, in the knobs' order.
/// GPS L1 C/A is BPSK(1): one lobe, a single point.
/// Galileo E1 OS is CBOC(6,1,1/11), a BOC(1,1) to within a few percent of its power: the spectrum split into two lobes 1.023 MHz either side of the carrier, so a twin point.
/// GLONASS L1OF is BPSK(0.5), a lobe half as wide: a tighter point; its carrier per satellite, so GLONASS is a fan of hues.
/// BeiDou B1I is BPSK(2), a lobe twice as wide: a broader point, at its own carrier 1561.098 MHz.
/// QZSS L1 C/A is GPS's own signal from another orbit: a single point, GPS's hue.
/// NavIC SPS is BPSK(1) on both its carriers: S at 2492.028 MHz its main light, L5 its second, always there.
/// The L5 family is BPSK(10) or QPSK(10) at 10.23 Mchip/s (GPS L5, Galileo E5a, QZSS L5, BeiDou B2a): a lobe ten times as wide, a soft deep glow; GLONASS has none the receiver tracks.
const SYSTEMS: [System; 6] = [
    System { name: "GPS", mhz: L1_MHZ, mcps: CA_MCPS, boc: false, l5: L5::Some, l5_mcps: 10.23 },
    System { name: "Galileo", mhz: L1_MHZ, mcps: CA_MCPS, boc: true, l5: L5::Some, l5_mcps: 10.23 },
    System { name: "GLONASS", mhz: 0., mcps: 0.511, boc: false, l5: L5::Never, l5_mcps: 0. },
    System { name: "BeiDou", mhz: 1561.098, mcps: 2.046, boc: false, l5: L5::Some, l5_mcps: 10.23 },
    System { name: "QZSS", mhz: L1_MHZ, mcps: CA_MCPS, boc: false, l5: L5::Some, l5_mcps: 10.23 },
    System { name: "NavIC", mhz: 2492.028, mcps: CA_MCPS, boc: false, l5: L5::Always, l5_mcps: CA_MCPS },
];

/// A carrier's wavelength of light, nm: logarithmic in the frequency, 1575.42 MHz exactly on VSF green.
/// x = ln(1575.42/f) is the frequency's octave-like distance from L1 (positive below it); the wavelength is 523·e^{A·tanh(γx/A)}, so near L1 a frequency ratio is a wavelength ratio raised to γ (the stretch), and far from it the map runs smoothly toward VSF red (below L1) or VSF blue (above), A = ln(703/523) or ln(523/462), never past either primary.
/// The two halves share their value, slope and curvature at L1 (C² in f); A is chosen once a satellite, not a pixel.
/// At γ = 4 the L1 cluster spans BeiDou B1I 542 nm (yellow-green) to GLONASS k = +6 at 489 nm (cyan), the L5 family lands at 703 nm (VSF red, the deeper light), NavIC S at 462 nm (VSF blue).
pub fn carrier_nm(mhz: f64, stretch: f64) -> f64 {
    let x = (L1_MHZ / mhz).ln();
    let a = if x > 0. { (NM_R / NM_G).ln() } else { (NM_G / NM_B).ln() };
    NM_G * (a * (stretch * x / a).tanh()).exp()
}

/// A wavelength as a colour, as payloads/display `battery_flavour` does it: its cone response (vsf LMS_2000_10DEG_1NM, linear between the 1 nm samples) in VSF RGB, moved toward white (E) by its most negative channel, so the hue is exact at the most saturation the primaries reach.
/// Then as linear light at the console ring's photopic light (VSF's weights R 0.2584, G 0.6346, B 0.1070), or with its brightest channel full where the colour cannot reach that.
/// Worked out once a satellite; the min and max pick a colour's channels, they shape no pixel.
pub fn wave_light(nm: f64) -> [f32; 3] {
    let s = &LMS_2000_10DEG_1NM;
    let f = nm as f32 - s.start_nm;
    let (i, w) = (f as usize, f - (f as usize) as f32);
    let lms = [0, 1, 2].map(|c| s.data[3 * i + c] * (1. - w) + s.data[3 * i + 3 + c] * w);
    let rgb = apply_matrix_3x3_f32(&LMS2VSF_RGB, &lms);
    let m = rgb[0].min(rgb[1]).min(rgb[2]);
    let lin = rgb.map(|v| v - m);
    let light = 0.2584 * lin[0] + 0.6346 * lin[1] + 0.1070 * lin[2];
    let k = RING_LIGHT / light;
    // WHY the cap (battery_flavour's): a colour whose photopic weight is low (deep red, deep blue) cannot reach the ring's light within the primaries; its brightest channel stops at full.
    let top = lin[0].max(lin[1]).max(lin[2]) * k;
    let k = if top > 1. { k / top } else { k };
    lin.map(|v| v * k)
}

/// One satellite as the dome draws it.
struct Sat {
    /// Its system (index into SYSTEMS) and its main light.s wavelength, nm.
    sys: usize,
    nm: f64,
    /// Its place on the dome (r units) and the unit vector of its azimuth there (after the secret turn and the mirror).
    x: f32,
    y: f32,
    ux: f32,
    uy: f32,
    cos_el: f32,
    sin_el: f32,
    cn0: f32,
    /// Its main light's colour (linear, at the ring's light), its width (r), and its twin's half-separation along the tangent (0 for a single point).
    col: [f32; 3],
    sigma: f32,
    twin: f32,
    /// Its L5-family light, if it has one: colour and width; drawn inward of the main light (toward the zenith, red innermost as on the rim's prism).
    l5: Option<([f32; 3], f32)>,
    /// Two flicker rates (turns a frame) and three phases (turns), its own.
    nu: [f32; 2],
    ph: [f32; 3],
    used: bool,
    /// This frame's light (C/N0, used or ember, flicker), worked out in `Sky::new`.
    light: f32,
    /// The range arc's radius this frame: |s| when fixed (through the centre), sweeping while searching.
    arc_r: f32,
}

/// The knot: the horizontal error's covariance (r², screen axes), worked out from the used satellites' geometry and scaled so its RMS radius is the error estimate.
struct Knot {
    cxx: f32,
    cxy: f32,
    cyy: f32,
}

/// Everything the dome needs for one frame.
pub struct Sky {
    sats: Vec<Sat>,
    /// The horizon's radius, in r.
    rho_h: f32,
    knot: Option<Knot>,
    /// Each used satellite's range error on the dome, r (the knot's σ over HDOP).
    sigma_r: f32,
    /// The flavour's light (linear, VSF green at the ring's light).
    flavour: [f32; 3],
    /// The ripple's phase this frame (radians) and its wavenumber (radians a unit of r).
    ripple_ph: f32,
    ripple_k: f32,
    knee: f32,
    pub hdop: f32,
    pub n_tracked: usize,
    pub n_used: usize,
}

/// The base width of a light (a C/A code's lobe), in r: 2⁻⁶, a pixel at the console's true size, six on the large circle.
const SIGMA0: f32 = 1. / (1 << 6) as f32;
/// The lowest elevation drawn: sin(el) from 2⁻⁴ (3.6°) up, an elevation mask.
const SIN_EL_MIN: f32 = 1. / (1 << 4) as f32;
/// The horizon is 2⁷ m from the centre in the knot's scale: a 4 m error is a knot 2⁻⁵ of the dome's radius.
const DOME_M: f32 = (1 << 7) as f32;

impl Sky {
    pub fn new(k: &Look) -> Sky {
        let frame = k.frame as f32;
        let rho_h = 1. - k.depth - 1. / (1 << 4) as f32;
        let fixed_state = k.state == 2;
        let stretch = k.stretch as f64;
        // The privacy transform: the dome mirrored (x → −x) if asked, then turned by the secret angle.
        let (ts, tc) = (TAU * k.secret_turn).sin_cos();
        let mirror = if k.mirror { -1. } else { 1. };
        // The open sky's centre: a canyon's opening points somewhere the seed picks.
        let open_c = TAU * unit(mix(k.seed as u64 ^ 0x6F70_656E)) as f32;
        let mut sats = Vec::new();
        for (si, sys) in SYSTEMS.iter().enumerate() {
            for i in 0..k.sats[si] {
                let h = mix((k.seed as u64) << 24 ^ (si as u64) << 8 ^ i as u64);
                let u = |j: u64| unit(mix(h ^ j)) as f32;
                // Azimuth inside the open sector, uniform; sin(el) uniform above the mask: uniform over the sky's area, so uniform on the equal-area dome.
                let az = open_c + (u(0) - 0.5) * k.sector * TAU;
                let sin_el = SIN_EL_MIN + (1. - SIN_EL_MIN) * u(1);
                let cos_el = (1. - sin_el * sin_el).sqrt();
                // The zenith angle's half: sin(z/2) = √((1 − cos z)/2), cos z = sin(el).
                let rho = rho_h * 2f32.sqrt() * ((1. - sin_el) * 0.5).sqrt();
                // Seen from below with north up, east is on the left: azimuth (from north, toward east) points (−sin az, −cos az) on the screen (y down).
                let (sa, ca) = az.sin_cos();
                let (dx, dy) = (-sa * mirror, -ca);
                let (ux, uy) = (dx * tc - dy * ts, dx * ts + dy * tc);
                // C/N0: stronger high in the sky (30 dB-Hz at the horizon, 46 at the zenith), plus a spread of near-Gaussian noise (three uniforms, unit variance).
                let g = (u(2) + u(3) + u(4) - 1.5) * 2.;
                let cn0 = 30. + 16. * sin_el + k.cn0_spread * g;
                // GLONASS's channel k, −7…+6 from the seed.
                let mhz = if sys.mhz == 0. { GLO_L1_MHZ + ((u(5) * 14.) as i32 - 7) as f64 * GLO_STEP_MHZ } else { sys.mhz };
                let nm = carrier_nm(mhz, stretch);
                let col = wave_light(nm);
                // A light's width follows its code's main lobe, softened to the square root so the L5 glow stays beside its satellite.
                let sigma = SIGMA0 * (sys.mcps / CA_MCPS).sqrt();
                let has_l5 = match sys.l5 {
                    L5::Never => false,
                    L5::Some => u(6) < k.l5_frac,
                    L5::Always => true,
                };
                let l5 = has_l5.then(|| (wave_light(carrier_nm(L5_MHZ, stretch)), SIGMA0 * (sys.l5_mcps / CA_MCPS).sqrt()));
                sats.push(Sat {
                    sys: si,
                    nm,
                    x: rho * ux,
                    y: rho * uy,
                    ux,
                    uy,
                    cos_el,
                    sin_el,
                    cn0,
                    col,
                    sigma,
                    twin: if sys.boc { 1.5 * sigma } else { 0. },
                    l5,
                    nu: [1. / (1 << 3) as f32 + u(7) / (1 << 4) as f32, 1. / (1 << 2) as f32 + u(8) / (1 << 3) as f32],
                    ph: [u(9), u(10), u(11)],
                    used: false,
                    light: 0.,
                    arc_r: rho,
                });
            }
        }
        let n_tracked = sats.len();
        // The fix uses the strongest: the receiver weights by C/N0. WHY the min: the used count is a count of satellites, and cannot exceed the tracked.
        let mut order: Vec<usize> = (0..n_tracked).collect();
        order.sort_by(|&a, &b| sats[b].cn0.total_cmp(&sats[a].cn0));
        let n_want = (k.used as usize).min(n_tracked);
        for &i in &order[..n_want] {
            sats[i].used = true;
        }
        // The geometry: G's rows (cos el·u, sin el, 1) of the used satellites, Q = (GᵀG)⁻¹, its horizontal block the error ellipse's shape; HDOP = √(Q_xx + Q_yy). Four satellites at least (three position axes and the clock).
        let used: Vec<&Sat> = sats.iter().filter(|s| s.used).collect();
        let q = (used.len() >= 4).then(|| {
            let mut n = [[0f64; 4]; 4];
            for s in &used {
                let g = [(s.cos_el * s.ux) as f64, (s.cos_el * s.uy) as f64, s.sin_el as f64, 1.];
                for a in 0..4 {
                    for b in 0..4 {
                        n[a][b] += g[a] * g[b];
                    }
                }
            }
            invert4(n)
        });
        let hdop = q.map_or(f32::INFINITY, |q| (q[0][0] + q[1][1]).sqrt() as f32);
        // The knot's shape from the horizontal geometry alone, Q₂ = (Σ cos²el·u uᵀ)⁻¹: what the arcs on the dome show (each arc constrains the fix across itself, along u), so the knot is the arcs' own overlap, a lens where they cross shallowly.
        // The full Q above couples the clock and the height into the horizontal (its HDOP is the receiver's, shown on the slider); its ellipse can lie across the arcs, which the picture could not explain.
        let (mut a, mut b, mut c) = (0f64, 0f64, 0f64);
        for s in &used {
            let (gx, gy) = ((s.cos_el * s.ux) as f64, (s.cos_el * s.uy) as f64);
            (a, b, c) = (a + gx * gx, b + gx * gy, c + gy * gy);
        }
        let det2 = a * c - b * b;
        let (qxx, qxy, qyy) = (c / det2, -b / det2, a / det2);
        let hdop2 = (qxx + qyy).sqrt() as f32;
        // The knot's RMS radius is the driver's horizontal error estimate; each arc's range error the share that makes their overlap that size.
        let herr_r = k.herr_m / DOME_M * rho_h;
        let sigma_r = herr_r / hdop2;
        let n_fit = used.len();
        // Fixed with no satellites to draw (the device, until the driver decodes them): the receiver's word for the fix, and no geometry to shape the knot, so a round one whose RMS radius is the error estimate (half its square on each axis).
        let blind = sats.is_empty();
        let fixed = fixed_state && (q.is_some() || blind);
        let knot = fixed.then(|| match q {
            Some(_) => {
                let s2 = (sigma_r * sigma_r) as f64;
                Knot { cxx: (qxx * s2) as f32, cxy: (qxy * s2) as f32, cyy: (qyy * s2) as f32 }
            }
            None => Knot { cxx: herr_r * herr_r * 0.5, cxy: 0., cyy: herr_r * herr_r * 0.5 },
        });
        let n_used = if fixed { n_fit } else { 0 };
        // This frame's lights.
        for s in &mut sats {
            if !fixed {
                s.used = false;
            }
            // Searching, every satellite is still being acquired: 3 dB weaker, an ember.
            let cn0 = if fixed { s.cn0 } else { s.cn0 - 3. };
            // C/N0 as light: the ring's light at 44 dB-Hz, a stop every 4 dB (52 dB-Hz burns at 4, 24 dB-Hz is 2⁻⁵ at the edge of seeing).
            let base = ((cn0 - 44.) / 4.).exp2();
            // Weak ones flicker: a depth from 0 (strong) to 1 (weak) by a logistic in C/N0 about 28 dB-Hz, two incommensurate rates, never below dark.
            let depth = 1. / (1. + ((cn0 - 28.) / 2.).exp());
            let wob = ((TAU * (frame * s.nu[0] + s.ph[0])).sin() + (TAU * (frame * s.nu[1] + s.ph[1])).sin()) * 0.5;
            let flicker = 1. + depth * wob;
            // Used in the fix: solid. Tracked only: an ember, 2⁻³ the light, breathing once every 2⁶ frames.
            let ember = if s.used { 1. } else { (1. + 0.5 * (TAU * (frame / (1 << 6) as f32 + s.ph[2])).sin()) / (1 << 3) as f32 };
            s.light = base * flicker * ember;
            // Searching, each range arc sweeps a quarter of its length either way at its own rate (one sweep every 2⁶ to 2⁷ frames): the arcs overlap and part, looking for their crossing.
            let rho = (s.x * s.x + s.y * s.y).sqrt();
            s.arc_r = if fixed { rho } else { rho * (1. + 0.25 * (TAU * (frame * (1. + s.ph[0]) / (1 << 7) as f32 + s.ph[1])).sin()) };
        }
        let fl = k.flavour.map(|g| g * g);
        Sky {
            sats,
            rho_h,
            knot,
            sigma_r,
            flavour: fl,
            // GPS time: the ripple runs outward one wavelength every 2⁶ frames; 2⁴ wavelengths from the zenith to the horizon.
            ripple_ph: TAU * frame / (1 << 6) as f32,
            ripple_k: TAU * (1 << 4) as f32 / rho_h,
            knee: k.knee_stops.exp2(),
            hdop,
            n_tracked,
            n_used,
        }
    }

    /// The dome's light at (x, y) (r units), `fs` the sample's footprint (r), as four layers front to back: the satellites, the knot, the range arcs, the sky (the horizon and time's ripple).
    /// Each layer's light goes through the soft knee W·L/(W + L) (C∞, below W however hot), so nothing reaches a ceiling.
    /// Last, the night: the dome's opacity (3/4 inside the horizon, by the same logistic), a black layer under the light that dims the vessel's body behind the sky so the satellites stand out; the body still shows through a quarter.
    pub fn light(&self, x: f32, y: f32, fs: f32) -> ([[f32; 3]; 4], f32) {
        let f2 = fs * fs / 12.;
        let mut sat_l = [0f32; 3];
        let mut arc_l = [0f32; 3];
        for s in &self.sats {
            // A Gaussian point of width σ widened by the footprint (variance fs²/12), its energy kept.
            let point = |cx: f32, cy: f32, sig: f32| {
                let v = sig * sig + f2;
                let (dx, dy) = (x - cx, y - cy);
                sig * sig / v * (-(dx * dx + dy * dy) / (2. * v)).exp()
            };
            let main = if s.twin == 0. {
                point(s.x, s.y, s.sigma)
            } else {
                // BOC's two lobes: twin points along the tangent (−u_y, u_x), half the light each.
                let (tx, ty) = (-s.uy * s.twin, s.ux * s.twin);
                0.5 * (point(s.x + tx, s.y + ty, s.sigma) + point(s.x - tx, s.y - ty, s.sigma))
            };
            for c in 0..3 {
                sat_l[c] += s.light * main * s.col[c];
            }
            if let Some((col, sig)) = s.l5 {
                // The L5 light inward of the main one (toward the zenith) by the two widths together, so the pair stands apart even when both codes are narrow (NavIC), its energy a quarter of the main light's: a wider lobe a dimmer peak, a deep glow beside the point.
                let off = s.sigma + sig;
                let b = point(s.x - s.ux * off, s.y - s.uy * off, sig) * (s.sigma * s.sigma) / (4. * sig * sig);
                for c in 0..3 {
                    sat_l[c] += s.light * b * col[c];
                }
            }
            // The range arc: the circle about the satellite through the fix, lit on the side facing the centre (a von Mises fade of the angle about the satellite, κ = 2²).
            // Two parts: the arc itself, a fine line 2⁻⁸ r wide at 2⁻³ of the ring's light, so the arcs read as arcs and cross where the fix is; and its range error, a band as wide as the satellite's horizontal range error (σ/cos el: a high satellite says little about the horizontal) at 2⁻⁵ by cos el, whose overlaps are the knot's haze.
            // Both widened by the footprint with their energy kept; searching, the band is 2⁻⁵ r, no error known yet.
            if s.used || self.knot.is_none() {
                let (dx, dy) = (x - s.x, y - s.y);
                // Softened by 2⁻¹⁰ r so the angle is smooth through the satellite's own point.
                let dist = (dx * dx + dy * dy + 1. / (1 << 20) as f32).sqrt();
                let d = dist - s.arc_r;
                let cos_t = -(dx * s.ux + dy * s.uy) / dist;
                let along = ((1 << 2) as f32 * (cos_t - 1.)).exp();
                let gauss = |w0: f32| {
                    let w2 = w0 * w0 + f2;
                    (w0 * w0 / w2).sqrt() * (-d * d / (2. * w2)).exp()
                };
                let (bw, ba) = if self.knot.is_some() { (self.sigma_r / s.cos_el, s.cos_el / (1 << 5) as f32) } else { (1. / (1 << 5) as f32, 1. / (1 << 5) as f32) };
                let arc = (gauss(1. / (1 << 8) as f32) / (1 << 3) as f32 + ba * gauss(bw)) * along;
                for c in 0..3 {
                    arc_l[c] += arc * s.col[c];
                }
            }
        }
        // The knot: the error ellipse as a Gaussian about the centre (the fix, where the observer stands under the zenith), widened by the footprint.
        // Its peak 2× the ring's light at a round knot of width 2⁻⁵ r, falling as the error grows (1/σ²) and as the ellipse stretches (2√det/trace, 1 round, toward 0 for a lens): a tight round knot burns, a long one is a haze.
        let mut knot_l = [0f32; 3];
        if let Some(kn) = &self.knot {
            let (a, b, c) = (kn.cxx + f2, kn.cxy, kn.cyy + f2);
            let det = a * c - b * b;
            let q = (c * x * x - 2. * b * x * y + a * y * y) / det;
            let tr = a + c;
            let k0 = 1. / (1 << 5) as f32;
            let peak = 2. * k0 * k0 * 4. * det.sqrt() / (tr * tr);
            let g = peak * (-q / 2.).exp();
            knot_l = self.flavour.map(|v| g * v);
        }
        // The sky: a dim dome (2⁻¹⁰ of the flavour) cut at the horizon by a logistic a footprint and 2⁻⁷ r wide, a horizon line (2⁻⁶, a Gaussian 2⁻⁷ r), and GPS time's ripple (2⁻⁹ at full depth), its contrast falling with the footprint (a box's attenuation of a wave, e^{−k²fs²/24}) so it never aliases.
        let rho = (x * x + y * y).sqrt();
        let we = (fs * fs + 1. / (1 << 14) as f32).sqrt();
        let dome = 1. / (1. + ((rho - self.rho_h) / we).exp());
        let hw2 = 1. / (1 << 14) as f32 + f2;
        let horizon = (-(rho - self.rho_h) * (rho - self.rho_h) / (2. * hw2)).exp() / (1 << 6) as f32;
        let att = (-self.ripple_k * self.ripple_k * fs * fs / 24.).exp();
        let ripple = 0.5 * (1. + att * (self.ripple_k * rho - self.ripple_ph).cos()) / (1 << 9) as f32;
        let sky = dome * (1. / (1 << 10) as f32 + ripple) + horizon;
        let sky_l = self.flavour.map(|v| v * sky);
        // The arcs are of the sky: cut at the horizon with it (a searching arc swings past the rim).
        let arc_l = arc_l.map(|v| v * dome);
        let w = self.knee;
        ([sat_l, knot_l, arc_l, sky_l].map(|l| l.map(|v| w * v / (w + v))), 0.75 * dome)
    }
}

/// The inverse of a symmetric 4 × 4 (GᵀG), by Gauss-Jordan with partial pivoting.
/// With four or more satellites spread over the sky it is well conditioned; all in one azimuth it is near singular and HDOP grows large, which is the point.
fn invert4(m: [[f64; 4]; 4]) -> [[f64; 4]; 4] {
    let mut a = m;
    let mut inv = [[0f64; 4]; 4];
    for i in 0..4 {
        inv[i][i] = 1.;
    }
    for col in 0..4 {
        let piv = (col..4).max_by(|&p, &q| a[p][col].abs().total_cmp(&a[q][col].abs())).unwrap();
        a.swap(col, piv);
        inv.swap(col, piv);
        let d = a[col][col];
        for j in 0..4 {
            a[col][j] /= d;
            inv[col][j] /= d;
        }
        for row in 0..4 {
            if row != col {
                let f = a[row][col];
                for j in 0..4 {
                    a[row][j] -= f * a[col][j];
                    inv[row][j] -= f * inv[col][j];
                }
            }
        }
    }
    inv
}

/// The map pin GPS shows when switched off (payloads/display, "the classic teardrop with a hole punch"): the convex hull of a round head (radius 0.3 r) and the tip 0.6 r below its centre, its sides the tangents from the tip, a hole of 0.12 r through the head, the pin centred from the head's top to the tip.
/// Its signed distance in r (negative inside), the display's own; its coverage is the C∞ step across two pixels of it.
pub fn pin_sd(x: f32, y: f32) -> f32 {
    let (a, l, hole) = (0.3f32, 0.6f32, 0.12f32);
    let (qx, qy) = (x.abs(), y + (l - a) * 0.5);
    let (ca, sa) = (a / l, (1. - (a / l) * (a / l)).sqrt());
    let (kx, ky) = (a * sa, a * ca);
    let (vx, vy) = (-kx, l - ky);
    let tlen = (vx * vx + vy * vy).sqrt();
    let (dirx, diry) = (vx / tlen, vy / tlen);
    let s = (qx - kx) * dirx + (qy - ky) * diry;
    // The hull's distance in its three parts (head, tip, the straight side), which meet with matching slope: the display's construction, reproduced exactly.
    let body = if s <= 0. {
        (qx * qx + qy * qy).sqrt() - a
    } else if s >= tlen {
        (qx * qx + (qy - l) * (qy - l)).sqrt()
    } else {
        (qx - kx) * sa + (qy - ky) * ca
    };
    body.max(hole - (qx * qx + qy * qy).sqrt())
}

/// The display's C∞ step (draw::bump_step): 0 below 0, 1 above 1, e^{−1/t} / (e^{−1/t} + e^{−1/(1−t)}) between, every derivative 0 at both ends.
pub fn bump_step(t: f32) -> f32 {
    if t <= 0. {
        return 0.;
    }
    if t >= 1. {
        return 1.;
    }
    let (a, b) = ((-1. / t).exp(), (-1. / (1. - t)).exp());
    a / (a + b)
}

/// The pin's ink as linear light: the display's LIGHT_INK, 7000 of 2¹⁴ in gamma 2.
pub const PIN_INK: f32 = (7000. / (1 << 14) as f32) * (7000. / (1 << 14) as f32);

impl Sky {
    /// One line a system for --dump and S: how many tracked and used, their wavelengths' span, their C/N0 span; then HDOP.
    pub fn report(&self) -> String {
        let mut out = String::new();
        for (si, sys) in SYSTEMS.iter().enumerate() {
            let ss: Vec<&Sat> = self.sats.iter().filter(|s| s.sys == si).collect();
            if ss.is_empty() {
                continue;
            }
            let nm = ss.iter().map(|s| s.nm).fold((f64::INFINITY, 0f64), |a, v| (a.0.min(v), a.1.max(v)));
            let cn = ss.iter().map(|s| s.cn0).fold((f32::INFINITY, 0f32), |a, v| (a.0.min(v), a.1.max(v)));
            let used = ss.iter().filter(|s| s.used).count();
            let l5 = ss.iter().filter(|s| s.l5.is_some()).count();
            out += &format!("  {:8} {:2} tracked, {:2} used, {:2} with L5; {:.1}-{:.1} nm; C/N0 {:.1}-{:.1} dB-Hz\n", sys.name, ss.len(), used, l5, nm.0, nm.1, cn.0, cn.1);
        }
        out += &format!("  HDOP {:.2}, range σ {:.4} r", self.hdop, self.sigma_r);
        out
    }
}
