//! GLONASS broadcast ephemerides turned into the GPS chip's Kepler form (docs/GPS.md "The LTO windows are broadcast ephemerides": gpsd hands the BCM4776 GLONASS windows in IS-GPS-200's layout too).
//! A GLONASS satellite broadcasts a state vector (PZ-90 earth-fixed position, velocity and the moon's and sun's acceleration) every 30 minutes; GLONASS's ICD (Appendix 3.1.2) carries it with the earth's J2 and rotation by a 4th-order Runge-Kutta.
//! Here: the vectors read from a RINEX 3 navigation file ([`each_glo`]), the orbit carried over a span by the nearest vector for each instant ([`orbit_at`]), and the IS-GPS-200 ephemeris fitted to it by least squares ([`fit`]), its equations being `eph::gps_ecef`'s, so the chip's own Kepler model reproduces the orbit.
//! PZ-90.11 and WGS-84 agree to centimetres; the frames are taken as one.

use crate::eph::{gps_ecef, GpsEph};
use libm::{acos, atan2, cos, sin, sqrt};

/// One GLONASS broadcast record: the slot, the frequency channel, the epoch tb as GPS seconds of the GPS week (RINEX gives it in UTC; GPS = UTC + 18 s), the clock (-τn, γn), the state in metres, metres a second and metres a second squared.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GloEph {
    pub slot: u8,
    pub k: i8,
    pub week: u16,
    pub tb: f64,
    pub tau: f64,
    pub gamma: f64,
    pub health: u8,
    pub r: [f64; 3],
    pub v: [f64; 3],
    pub a: [f64; 3],
}

/// The GPS-UTC leap seconds since 2017.
const LEAP: f64 = 18.;

fn num(s: &[u8]) -> Option<f64> {
    let s = core::str::from_utf8(s).ok()?.trim();
    if s.is_empty() {
        return Some(0.);
    }
    let mut b = [0u8; 32];
    if s.len() > b.len() {
        return None;
    }
    for (i, c) in s.bytes().enumerate() {
        b[i] = if c == b'D' || c == b'd' { b'E' } else { c };
    }
    core::str::from_utf8(&b[..s.len()]).ok()?.parse().ok()
}

fn four(l: &[u8]) -> Option<[f64; 4]> {
    let mut v = [0.; 4];
    for (k, x) in v.iter_mut().enumerate() {
        let a = 4 + 19 * k;
        if a >= l.len() {
            break;
        }
        *x = num(&l[a..(a + 19).min(l.len())])?;
    }
    Some(v)
}

/// Every GLONASS record of a RINEX 3 navigation file (the epoch line and three orbit lines), in file order. Returns (taken, refused).
pub fn each_glo(text: &[u8], mut take: impl FnMut(GloEph)) -> (u32, u32) {
    let mut lines = text.split(|&c| c == b'\n').map(|l| if l.last() == Some(&b'\r') { &l[..l.len() - 1] } else { l });
    let (mut ok, mut bad) = (0u32, 0u32);
    let mut header = true;
    while let Some(l) = lines.next() {
        if header {
            header = !l.windows(13).any(|w| w == b"END OF HEADER");
            continue;
        }
        if l.first() != Some(&b'R') || l.len() < 23 {
            continue;
        }
        let (Some(a), Some(b), Some(c)) = (lines.next(), lines.next(), lines.next()) else {
            bad += 1;
            break;
        };
        match record(l, a, b, c) {
            Some(e) => {
                ok += 1;
                take(e)
            }
            None => bad += 1,
        }
    }
    (ok, bad)
}

fn record(l: &[u8], a: &[u8], b: &[u8], c: &[u8]) -> Option<GloEph> {
    let int = |x: usize, y: usize| -> Option<i64> { core::str::from_utf8(l.get(x..y)?).ok()?.trim().parse().ok() };
    let slot = int(1, 3)? as u8;
    let (y, mo, d, h, mi, s) = (int(4, 8)?, int(9, 11)?, int(12, 14)?, int(15, 17)?, int(18, 20)?, int(21, 23)?);
    let days = |y: i64, m: i64, d: i64| {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
    };
    let dd = days(y, mo, d) - days(1980, 1, 6);
    let sow = (dd.rem_euclid(7) * 86400 + h * 3600 + mi * 60 + s) as f64 + LEAP;
    let c0 = |k: usize| num(&l[(23 + 19 * k).min(l.len())..(23 + 19 * (k + 1)).min(l.len())]);
    let (fa, fb, fc) = (four(a)?, four(b)?, four(c)?);
    Some(GloEph {
        slot,
        k: fb[3] as i8,
        week: dd.div_euclid(7) as u16,
        tb: sow,
        tau: c0(0)?,
        gamma: c0(1)?,
        health: fa[3] as u8,
        r: [fa[0] * 1e3, fb[0] * 1e3, fc[0] * 1e3],
        v: [fa[1] * 1e3, fb[1] * 1e3, fc[1] * 1e3],
        a: [fa[2] * 1e3, fb[2] * 1e3, fc[2] * 1e3],
    })
}

/// PZ-90's constants (GLONASS ICD 2008, 3.1.2): GM, the equatorial radius, J2 (C20 = -J2), the earth's rotation.
const GM: f64 = 398_600.4418e9;
const AE: f64 = 6_378_136.;
const J2: f64 = 1_082_625.75e-9;
const WE: f64 = 7.292_115e-5;

fn deriv(s: &[f64; 6], acc: &[f64; 3]) -> [f64; 6] {
    let (x, y, z) = (s[0], s[1], s[2]);
    let r2 = x * x + y * y + z * z;
    let r = sqrt(r2);
    let mu = GM / (r2 * r);
    let k = 1.5 * J2 * GM * AE * AE / (r2 * r2 * r);
    let zz = 5. * z * z / r2;
    [
        s[3],
        s[4],
        s[5],
        -mu * x - k * x * (1. - zz) + WE * WE * x + 2. * WE * s[4] + acc[0],
        -mu * y - k * y * (1. - zz) + WE * WE * y - 2. * WE * s[3] + acc[1],
        -mu * z - k * z * (3. - zz) + acc[2],
    ]
}

/// The satellite's earth-fixed position at GPS second of week `t`, carried from record `e` by the ICD's Runge-Kutta in steps of at most 2^5 s.
pub fn carry(e: &GloEph, t: f64) -> [f64; 3] {
    let mut s = [e.r[0], e.r[1], e.r[2], e.v[0], e.v[1], e.v[2]];
    let span = t - e.tb;
    let n = libm::ceil(libm::fabs(span) / 32.).max(1.) as usize;
    let h = span / n as f64;
    for _ in 0..n {
        let k1 = deriv(&s, &e.a);
        let s2: [f64; 6] = core::array::from_fn(|i| s[i] + h / 2. * k1[i]);
        let k2 = deriv(&s2, &e.a);
        let s3: [f64; 6] = core::array::from_fn(|i| s[i] + h / 2. * k2[i]);
        let k3 = deriv(&s3, &e.a);
        let s4: [f64; 6] = core::array::from_fn(|i| s[i] + h * k3[i]);
        let k4 = deriv(&s4, &e.a);
        s = core::array::from_fn(|i| s[i] + h / 6. * (k1[i] + 2. * k2[i] + 2. * k3[i] + k4[i]));
    }
    [s[0], s[1], s[2]]
}

/// The orbit at `t` from the satellite's records: carried from the one whose tb is nearest (each is meant for its own quarter hour either side).
pub fn orbit_at(recs: &[GloEph], t: f64) -> Option<[f64; 3]> {
    let e = recs.iter().min_by(|a, b| libm::fabs(a.tb - t).partial_cmp(&libm::fabs(b.tb - t)).unwrap())?;
    Some(carry(e, t))
}

/// The IS-GPS-200 ephemeris (`GpsEph`, week and toe given) whose positions best match `pos(t)` over `toe ± half` seconds (least squares over samples every 2^6 s, Gauss-Newton with a numerical Jacobian, from the osculating elements at toe), and the fit's RMS error in metres.
/// The clock is left to the caller.
pub fn fit(pos: &dyn Fn(f64) -> [f64; 3], week: u16, toe: f64, half: f64) -> (GpsEph, f64) {
    // The osculating elements at toe: the state in the inertial frame that coincides with the earth-fixed one at toe.
    let dt = 1.;
    let (p0, pm, pp) = (pos(toe), pos(toe - dt), pos(toe + dt));
    let vfix: [f64; 3] = core::array::from_fn(|i| (pp[i] - pm[i]) / (2. * dt));
    let v = [vfix[0] - WE * p0[1], vfix[1] + WE * p0[0], vfix[2]];
    let r = sqrt(p0[0] * p0[0] + p0[1] * p0[1] + p0[2] * p0[2]);
    let vv = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    let mu = 3.986005e14;
    let h = [p0[1] * v[2] - p0[2] * v[1], p0[2] * v[0] - p0[0] * v[2], p0[0] * v[1] - p0[1] * v[0]];
    let hn = sqrt(h[0] * h[0] + h[1] * h[1] + h[2] * h[2]);
    let a = 1. / (2. / r - vv / mu);
    let ev: [f64; 3] = core::array::from_fn(|i| ((vv - mu / r) * p0[i] - (p0[0] * v[0] + p0[1] * v[1] + p0[2] * v[2]) * v[i]) / mu);
    let e = sqrt(ev[0] * ev[0] + ev[1] * ev[1] + ev[2] * ev[2]);
    let inc = acos(h[2] / hn);
    let node = atan2(h[0], -h[1]);
    let nv = [cos(node), sin(node), 0.];
    let u = atan2(p0[2] / sin(inc), p0[0] * nv[0] + p0[1] * nv[1]);
    let (w, m0) = if e > 1e-6 {
        let w = atan2(ev[2] / sin(inc), ev[0] * nv[0] + ev[1] * nv[1]);
        let nu = u - w;
        let ea = 2. * libm::atan(sqrt((1. - e) / (1. + e)) * libm::tan(nu / 2.));
        (w, ea - e * sin(ea))
    } else {
        (0., u)
    };
    let mut x = [sqrt(a), e, inc, node + WE * toe, w, m0, 0., -5.5e-9, 0., 0., 0., 0., 0., 0., 0.];
    let model = |x: &[f64; 15]| GpsEph { prn: 0, week, toe, toc: toe, sqrt_a: x[0], e: x[1], i0: x[2], omega0: x[3], omega: x[4], m0: x[5], dn: x[6], omega_dot: x[7], idot: x[8], cuc: x[9], cus: x[10], crc: x[11], crs: x[12], cic: x[13], cis: x[14], ..Default::default() };
    let n = ((2. * half) / 64.) as usize + 1;
    let mut ts = [0f64; 1 << 8];
    let mut obs = [[0f64; 3]; 1 << 8];
    let n = n.min(ts.len());
    for i in 0..n {
        ts[i] = toe - half + 2. * half * i as f64 / (n - 1) as f64;
        obs[i] = pos(ts[i]);
    }
    let resid = |x: &[f64; 15], out: &mut [f64]| {
        let m = model(x);
        for i in 0..n {
            let p = gps_ecef(&m, ts[i]);
            for c in 0..3 {
                out[3 * i + c] = obs[i][c] - p[c];
            }
        }
    };
    let steps = [1e-3, 1e-9, 1e-9, 1e-9, 1e-9, 1e-9, 1e-13, 1e-13, 1e-13, 1e-9, 1e-9, 1e-3, 1e-3, 1e-9, 1e-9];
    let mut r0 = [0f64; 3 << 8];
    let mut rj = [0f64; 3 << 8];
    let mut jac = [[0f64; 15]; 3 << 8];
    let mut lambda = 1e-3;
    let rms = |r: &[f64]| sqrt(r.iter().map(|v| v * v).sum::<f64>() / r.len() as f64);
    resid(&x, &mut r0[..3 * n]);
    let mut cur = rms(&r0[..3 * n]);
    for _ in 0..1 << 6 {
        for j in 0..15 {
            let mut xj = x;
            xj[j] += steps[j];
            resid(&xj, &mut rj[..3 * n]);
            for i in 0..3 * n {
                jac[i][j] = (r0[i] - rj[i]) / steps[j];
            }
        }
        // Normal equations (J^T J + λ diag) dx = J^T r, scaled per column.
        let mut ata = [[0f64; 15]; 15];
        let mut atb = [0f64; 15];
        for i in 0..3 * n {
            for p in 0..15 {
                atb[p] += jac[i][p] * r0[i];
                for q in 0..15 {
                    ata[p][q] += jac[i][p] * jac[i][q];
                }
            }
        }
        for p in 0..15 {
            ata[p][p] *= 1. + lambda;
        }
        let Some(dx) = solve(ata, atb) else { break };
        let mut xn = x;
        for p in 0..15 {
            xn[p] += dx[p];
        }
        resid(&xn, &mut rj[..3 * n]);
        let nr = rms(&rj[..3 * n]);
        if nr < cur {
            x = xn;
            r0[..3 * n].copy_from_slice(&rj[..3 * n]);
            let gain = cur - nr;
            cur = nr;
            lambda *= 0.25;
            if gain < 1e-4 {
                break;
            }
        } else {
            lambda *= 16.;
        }
    }
    (model(&x), cur)
}

/// Gaussian elimination with partial pivoting; None for a singular system.
fn solve(mut a: [[f64; 15]; 15], mut b: [f64; 15]) -> Option<[f64; 15]> {
    for c in 0..15 {
        let p = (c..15).max_by(|&i, &j| libm::fabs(a[i][c]).partial_cmp(&libm::fabs(a[j][c])).unwrap())?;
        if a[p][c] == 0. {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in c + 1..15 {
            let f = a[r][c] / a[c][c];
            for k in c..15 {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = [0f64; 15];
    for c in (0..15).rev() {
        let s: f64 = (c + 1..15).map(|k| a[c][k] * x[k]).sum();
        x[c] = (b[c] - s) / a[c][c];
    }
    Some(x)
}
