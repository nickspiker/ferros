//! The engine against NOAA CO-OPS's published predictions (US public domain).
//! Each station's 37 harmonic constants (amplitude in metres, phase_GMT in degrees, NOAA's order), its MSL and MLLW datums, and its published highs and lows for 2026-11-01 to 2026-11-07 (UTC, MLLW, metric), all fetched from api.tidesandcurrents.noaa.gov on 2026-11-11 and copied here as plain Rust constants.

extern crate std;
use super::*;
use std::println;

/// POSIX seconds at a civil UTC date and time (Hinnant's days_from_civil).
fn unix(y: i64, m: i64, d: i64, hh: i64, mm: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + hh * 3600 + mm * 60
}

/// November 2026, day, hour, minute (UTC) in Eagle seconds.
fn nov(d: i64, hh: i64, mm: i64) -> f64 {
    eagle_from_unix(unix(2026, 11, d, hh, mm))
}

struct Published {
    name: &'static str,
    msl: f64,
    mllw: f64,
    /// (amplitude m, phase_GMT degrees) in NOAA's order.
    hc: [(f64, f64); N],
    /// (day of November, hour, minute, height m above MLLW, high).
    events: &'static [(i64, i64, i64, f64, bool)],
}

const H: bool = true;
const L: bool = false;

const SAN_FRANCISCO: Published = Published {
    name: "San Francisco 9414290",
    msl: 2.773,
    mllw: 1.822,
    hc: [
        (0.576, 208.2), (0.137, 216.2), (0.122, 183.2), (0.37, 225.4), (0.022, 136.8), (0.23, 208.4), (0.002, 7.3), (0.018, 124.1), (0.0, 0.0), (0.008, 112.0),
        (0.026, 189.9), (0.0, 0.0), (0.007, 100.2), (0.013, 153.3), (0.012, 260.0), (0.007, 214.3), (0.007, 282.6), (0.011, 237.5), (0.019, 244.3), (0.0, 0.0),
        (0.03, 272.3), (0.044, 200.2), (0.0, 0.0), (0.015, 153.1), (0.009, 200.0), (0.041, 202.4), (0.009, 204.6), (0.001, 126.8), (0.006, 208.6), (0.114, 222.1),
        (0.003, 37.3), (0.003, 37.6), (0.018, 229.6), (0.014, 92.2), (0.04, 206.0), (0.001, 108.5), (0.01, 149.0),
    ],
    events: &[
        (1, 5, 56, -0.132, L), (1, 13, 40, 1.46, H), (1, 18, 24, 0.989, L), (1, 23, 59, 1.646, H),
        (2, 7, 4, -0.044, L), (2, 14, 35, 1.534, H), (2, 19, 51, 0.837, L),
        (3, 1, 21, 1.537, H), (3, 8, 6, 0.044, L), (3, 15, 21, 1.626, H), (3, 21, 1, 0.631, L),
        (4, 2, 43, 1.471, H), (4, 9, 0, 0.143, L), (4, 16, 0, 1.72, H), (4, 21, 59, 0.412, L),
        (5, 3, 57, 1.442, H), (5, 9, 47, 0.263, L), (5, 16, 35, 1.802, H), (5, 22, 48, 0.212, L),
        (6, 5, 3, 1.434, H), (6, 10, 29, 0.399, L), (6, 17, 8, 1.862, H), (6, 23, 32, 0.051, L),
        (7, 6, 3, 1.436, H), (7, 11, 9, 0.545, L), (7, 17, 38, 1.895, H),
    ],
};

const THE_BATTERY: Published = Published {
    name: "The Battery 8518750",
    msl: 1.785,
    mllw: 1.002,
    hc: [
        (0.671, 18.2), (0.128, 42.9), (0.156, 0.5), (0.103, 178.5), (0.024, 264.1), (0.051, 176.3), (0.023, 150.4), (0.008, 69.5), (0.011, 20.8), (0.011, 265.9),
        (0.026, 359.7), (0.001, 272.0), (0.023, 10.0), (0.022, 347.7), (0.003, 209.5), (0.008, 10.5), (0.013, 132.1), (0.004, 247.4), (0.004, 196.6), (0.0, 0.0),
        (0.032, 55.3), (0.094, 144.6), (0.0, 0.0), (0.0, 0.0), (0.002, 143.9), (0.011, 196.6), (0.012, 32.4), (0.002, 299.9), (0.004, 234.6), (0.033, 178.8),
        (0.002, 357.9), (0.01, 66.1), (0.023, 21.0), (0.006, 44.7), (0.036, 44.5), (0.002, 332.2), (0.015, 258.9),
    ],
    events: &[
        (1, 5, 55, 1.362, H), (1, 11, 53, 0.233, L), (1, 18, 10, 1.561, H),
        (2, 0, 58, 0.132, L), (2, 6, 57, 1.385, H), (2, 13, 9, 0.255, L), (2, 19, 13, 1.513, H),
        (3, 2, 1, 0.103, L), (3, 7, 58, 1.428, H), (3, 14, 18, 0.226, L), (3, 20, 16, 1.478, H),
        (4, 2, 57, 0.057, L), (4, 8, 59, 1.49, H), (4, 15, 19, 0.172, L), (4, 21, 18, 1.464, H),
        (5, 3, 47, 0.017, L), (5, 9, 56, 1.566, H), (5, 16, 12, 0.113, L), (5, 22, 14, 1.466, H),
        (6, 4, 33, -0.005, L), (6, 10, 45, 1.638, H), (6, 17, 2, 0.064, L), (6, 23, 2, 1.468, H),
        (7, 5, 17, -0.004, L), (7, 11, 27, 1.689, H), (7, 17, 49, 0.029, L), (7, 23, 46, 1.459, H),
    ],
};

const SEATTLE: Published = Published {
    name: "Seattle 9447130",
    msl: 4.443,
    mllw: 2.419,
    hc: [
        (1.063, 10.8), (0.268, 36.8), (0.214, 341.1), (0.834, 276.8), (0.021, 200.7), (0.459, 254.6), (0.009, 312.8), (0.036, 79.3), (0.002, 254.3), (0.009, 172.7),
        (0.044, 355.5), (0.0, 0.0), (0.034, 238.9), (0.023, 313.1), (0.031, 330.2), (0.02, 49.9), (0.021, 45.0), (0.024, 304.1), (0.043, 313.4), (0.0, 0.0),
        (0.024, 217.0), (0.07, 283.2), (0.0, 0.0), (0.015, 157.0), (0.015, 245.0), (0.073, 248.9), (0.016, 38.0), (0.003, 11.2), (0.01, 265.5), (0.257, 276.2),
        (0.008, 284.4), (0.004, 178.0), (0.049, 58.7), (0.035, 48.5), (0.079, 37.7), (0.001, 204.4), (0.012, 229.3),
    ],
    events: &[
        (1, 3, 37, 2.861, H), (1, 10, 58, -0.294, L), (1, 18, 55, 3.502, H),
        (2, 1, 3, 2.205, L), (2, 5, 3, 2.6, H), (2, 12, 3, 0.051, L), (2, 19, 55, 3.506, H),
        (3, 2, 26, 1.825, L), (3, 6, 47, 2.442, H), (3, 13, 12, 0.397, L), (3, 20, 44, 3.533, H),
        (4, 3, 22, 1.364, L), (4, 8, 30, 2.47, H), (4, 14, 21, 0.725, L), (4, 21, 24, 3.557, H),
        (5, 4, 6, 0.89, L), (5, 9, 57, 2.649, H), (5, 15, 26, 1.037, L), (5, 21, 56, 3.56, H),
        (6, 4, 43, 0.45, L), (6, 11, 7, 2.89, H), (6, 16, 24, 1.34, L), (6, 22, 24, 3.533, H),
        (7, 5, 16, 0.079, L), (7, 12, 5, 3.13, H), (7, 17, 17, 1.631, L), (7, 22, 50, 3.476, H),
    ],
};

const HONOLULU: Published = Published {
    name: "Honolulu 1612340",
    msl: 1.412,
    mllw: 1.161,
    hc: [
        (0.171, 59.4), (0.056, 54.7), (0.033, 48.8), (0.149, 226.8), (0.001, 195.3), (0.081, 215.9), (0.001, 229.7), (0.001, 342.6), (0.0, 0.0), (0.001, 92.9),
        (0.007, 56.0), (0.0, 0.0), (0.004, 7.8), (0.004, 39.9), (0.005, 251.7), (0.001, 76.2), (0.002, 210.0), (0.005, 232.5), (0.01, 238.3), (0.005, 43.4),
        (0.0, 0.0), (0.048, 177.7), (0.0, 0.0), (0.006, 44.3), (0.002, 195.4), (0.014, 209.6), (0.004, 43.6), (0.001, 357.3), (0.001, 231.0), (0.045, 224.9),
        (0.001, 199.8), (0.001, 164.1), (0.004, 49.3), (0.0, 0.0), (0.016, 45.7), (0.0, 0.0), (0.001, 104.0),
    ],
    events: &[
        (1, 2, 55, 0.082, L), (1, 7, 52, 0.184, H), (1, 11, 47, 0.129, L), (1, 19, 51, 0.635, H),
        (2, 3, 42, 0.06, L), (2, 9, 24, 0.254, H), (2, 13, 39, 0.171, L), (2, 20, 53, 0.6, H),
        (3, 4, 16, 0.041, L), (3, 10, 24, 0.343, H), (3, 15, 16, 0.186, L), (3, 21, 47, 0.56, H),
        (4, 4, 45, 0.025, L), (4, 11, 9, 0.436, H), (4, 16, 35, 0.183, L), (4, 22, 34, 0.512, H),
        (5, 5, 10, 0.012, L), (5, 11, 49, 0.523, H), (5, 17, 42, 0.173, L), (5, 23, 15, 0.457, H),
        (6, 5, 33, 0.001, L), (6, 12, 25, 0.596, H), (6, 18, 40, 0.161, L), (6, 23, 52, 0.401, H),
        (7, 5, 55, -0.007, L), (7, 13, 0, 0.651, H), (7, 19, 32, 0.149, L),
    ],
};

fn tide_of(s: &Published) -> Tide {
    let mut t = Tide::flat(s.msl - s.mllw);
    for (k, &(a, g)) in s.hc.iter().enumerate() {
        t.set(k, a, g);
    }
    t
}

/// Every published event against the engine's nearest one of its kind: (the worst |Δt| in minutes, the worst |Δh| in metres, the mean |Δt|, the mean |Δh|), and the counts (published, found).
/// `printed`: the constituents as the tables take them ([`Frame::printed`]), else at each day's middle ([`Frame::at`], as the puck draws).
fn compare(s: &Published, printed: bool) -> (f64, f64, f64, f64, usize, usize) {
    compare_with(s, tide_of(s), printed)
}

fn compare_with(s: &Published, tide: Tide, printed: bool) -> (f64, f64, f64, f64, usize, usize) {
    let mut found = [Extreme { t: 0., h: 0., high: false }; 1 << 6];
    // A day at a time, as the puck asks.
    let mut n = 0;
    for d in 1..8 {
        let (from, to) = (nov(d, 0, 0), nov(d + 1, 0, 0));
        let fr = if printed { Frame::printed(from) } else { Frame::at(0.5 * (from + to)) };
        n += tide.extremes_in(&fr, from, to, &mut found[n..]);
    }
    let (mut wt, mut wh, mut st, mut sh) = (0f64, 0f64, 0., 0.);
    for &(d, hh, mm, h, high) in s.events {
        let t = nov(d, hh, mm);
        let e = found[..n].iter().filter(|e| e.high == high).min_by(|a, b| (a.t - t).abs().total_cmp(&(b.t - t).abs())).unwrap();
        let dt = (e.t - t).abs() / 60.;
        let dh = (e.h - h).abs();

        wt = wt.max(dt);
        wh = wh.max(dh);
        st += dt;
        sh += dh;
    }
    let m = s.events.len() as f64;
    (wt, wh, st / m, sh / m, s.events.len(), n)
}

/// The tolerances, as the printed tables take the constituents: published times are rounded to the minute and heights to the millimetre, and amplitudes and phases to a millimetre and a tenth of a degree.
const PRINTED_MINUTES: f64 = 4.;
const PRINTED_METRES: f64 = 0.015;
/// And as the puck takes them, at each moment: the nodal f and u then differ from the year's middle by a few per cent of the diurnal tide.
const LIVE_MINUTES: f64 = 4.;
const LIVE_METRES: f64 = 0.035;

fn check(s: &Published) {
    for (printed, minutes, metres) in [(true, PRINTED_MINUTES, PRINTED_METRES), (false, LIVE_MINUTES, LIVE_METRES)] {
        let (wt, wh, mt, mh, published, found) = compare(s, printed);
        println!("{} ({}): {} published, {} found; time worst {:.1} min, mean {:.2} min; height worst {:.1} cm, mean {:.2} cm", s.name, if printed { "printed" } else { "live" }, published, found, wt, mt, wh * 100., mh * 100.);
        assert_eq!(published, found, "{}: as many highs and lows as published", s.name);
        assert!(wt <= minutes, "{}: worst time {wt} min", s.name);
        assert!(wh <= metres, "{}: worst height {wh} m", s.name);
    }
}

#[test]
fn san_francisco() {
    check(&SAN_FRANCISCO);
}

#[test]
fn the_battery() {
    check(&THE_BATTERY);
}

#[test]
fn seattle() {
    check(&SEATTLE);
}

#[test]
fn honolulu() {
    check(&HONOLULU);
}

/// The speeds the Doodson numbers give against NOAA's published ones (degrees an hour), to 10^-5.
#[test]
fn speeds_match_noaa() {
    const SPEEDS: [f64; N] = [
        28.984104, 30.0, 28.43973, 15.041069, 57.96821, 13.943035, 86.95232, 44.025173, 60.0, 57.423832, 28.512583, 90.0, 27.968208, 27.895355, 16.139101, 29.455626, 15.0, 14.496694, 15.5854435,
        0.5443747, 0.0821373, 0.0410686, 1.0158958, 1.0980331, 13.471515, 13.398661, 29.958933, 30.041067, 12.854286, 14.958931, 31.015896, 43.47616, 29.528479, 42.92714, 30.082138, 115.93642,
        58.984104,
    ];
    let fr = Frame::at(nov(1, 0, 0));
    for k in 0..N {
        let w = fr.speed_deg_per_hour(k);
        assert!((w - SPEEDS[k]).abs() < 1e-5, "{}: {w} against {}", CONSTITUENTS[k].name, SPEEDS[k]);
    }
}

/// The nodal factors stay in their ranges and every f and u moves smoothly across a whole node cycle (18.61 years, sampled every 2 days): a nodal angle that jumped half a turn (a bare arctangent) shows here.
#[test]
fn nodal_corrections_are_smooth() {
    let mut last: Option<Frame> = None;
    let mut t = eagle_from_unix(unix(2020, 1, 1, 0, 0));
    for _ in 0..(1 << 12) {
        let fr = Frame::at(t);
        // Schureman's Table 14 ranges: M2 0.963 to 1.038; M1 the widest, its factor up to about 2.3 (its Qa near 2 times f(O1) near 1.19).
        assert!(fr.f[0] > 0.96 && fr.f[0] < 1.04, "f(M2) {}", fr.f[0]);
        for k in 0..N {
            assert!(fr.f[k] > 0.3 && fr.f[k] < 2.4, "f({}) {}", CONSTITUENTS[k].name, fr.f[k]);
        }
        if let Some(p) = last {
            for k in 0..N {
                let du = (fr.u[k] - p.u[k] + core::f64::consts::PI).rem_euclid(core::f64::consts::TAU) - core::f64::consts::PI;
                assert!((fr.f[k] - p.f[k]).abs() < 0.02, "f({}) jumped", CONSTITUENTS[k].name);
                assert!(du.abs() < 2. * D2R, "u({}) jumped {} degrees", CONSTITUENTS[k].name, du / D2R);
            }
        }
        last = Some(fr);
        t += 2. * 86_400.;
    }
}

/// Each constituent's argument convention (its multiple of 90°, which the sources state differently for the diurnal ones, S1 and M1) is the one the published predictions use: no quarter or half turn added to any one constituent brings the printed comparison closer over all four stations.
#[test]
fn conventions_are_the_published_ones() {
    let all = [&SAN_FRANCISCO, &THE_BATTERY, &SEATTLE, &HONOLULU];
    // Mean |Δh| (cm) and a third of the mean |Δt| (min), summed over the stations.
    let score = |turn: Option<(usize, f64)>| -> f64 {
        all.iter()
            .map(|s| {
                let mut t = tide_of(s);
                if let Some((k, d)) = turn {
                    t.phase[k] += d * D2R;
                }
                let r = compare_with(s, t, true);
                r.3 * 100. + r.2 / 3.
            })
            .sum()
    };
    let base = score(None);
    for k in 0..N {
        for d in [90., 180., 270.] {
            let s = score(Some((k, d)));
            assert!(s >= base - 1e-9, "{} turned {d} degrees fits better: {s} against {base}", CONSTITUENTS[k].name);
        }
    }
}

#[test]
fn eagle_round_trip() {
    let t = nov(11, 12, 34) + 0.25;
    assert!((eagle_secs(eagle_oscillations(t)) - t).abs() < 1e-6);
    // J2000.0 is day 0.
    assert_eq!(days_j2000(eagle_from_unix(unix(2000, 1, 1, 12, 0))), 0.);
    // The year's bounds.
    assert_eq!(year_bounds(nov(11, 12, 0)), (eagle_from_unix(unix(2026, 1, 1, 0, 0)), eagle_from_unix(unix(2027, 1, 1, 0, 0))));
    assert_eq!(year_bounds(eagle_from_unix(unix(2024, 12, 31, 23, 59))).0, eagle_from_unix(unix(2024, 1, 1, 0, 0)));
}

#[test]
fn next_finds_both() {
    let tide = tide_of(&SEATTLE);
    let (h, l) = tide.next(nov(2, 0, 0));
    let (h, l) = (h.unwrap(), l.unwrap());
    // The 2nd's first events: the low at 01:03 and the high at 05:03.
    assert!((l.t - nov(2, 1, 3)).abs() < 4. * 60.);
    assert!((h.t - nov(2, 5, 3)).abs() < 4. * 60.);
    assert!(!l.high && h.high);
}

/// The live curve is smooth: the height from one frame and from a frame made a day later agree where both are used (the puck redraws a day's curve from a fresh frame).
#[test]
fn frames_agree() {
    let tide = tide_of(&SEATTLE);
    let (a, b) = (Frame::at(nov(3, 0, 0)), Frame::at(nov(4, 0, 0)));
    let t = nov(3, 12, 0);
    assert!((tide.height_in(&a, t) - tide.height_in(&b, t)).abs() < 1e-3);
    assert!((tide.height_in(&a, t) - tide.height(t)).abs() < 1e-3);
}

/// A face: its words are the next extremes, its deadline the earlier of them, and its curve joined by cubic Hermite pieces (the drawer's C¹ curve) stays within a centimetre of the true tide between samples at every station.
#[test]
fn a_face_carries_its_day() {
    for s in [&SAN_FRANCISCO, &THE_BATTERY, &SEATTLE, &HONOLULU] {
        let tide = tide_of(s);
        let now = nov(4, 9, 30);
        let f = Face::at(&tide, now).unwrap();
        assert!(f.next_high.high && !f.next_low.high && f.next_high.t > now && f.next_low.t > now);
        assert_eq!(f.until, f.next_high.t.min(f.next_low.t));
        assert_eq!(f.rate > 0., f.next_high.t < f.next_low.t, "{}: rising means the high comes first", s.name);
        let step = 86_400. / CURVE as f64;
        let mut worst = 0f64;
        for k in 0..CURVE - 1 {
            for j in 1..(1 << 3) {
                let x = j as f64 / (1 << 3) as f64;
                let (h00, h10, h01, h11) = (2. * x * x * x - 3. * x * x + 1., x * x * x - 2. * x * x + x, -2. * x * x * x + 3. * x * x, x * x * x - x * x);
                let h = h00 * f.heights[k] as f64 + h10 * f.rates[k] as f64 + h01 * f.heights[k + 1] as f64 + h11 * f.rates[k + 1] as f64;
                worst = worst.max((h - tide.height(f.start + (k as f64 + x) * step)).abs());
            }
        }
        println!("{}: the face's curve within {:.2} mm", s.name, worst * 1e3);
        assert!(worst < 0.01, "{}: {worst}", s.name);
        assert!(f.lo <= f.height as f32 + 1e-3 && f.height as f32 <= f.hi + 1e-3);
    }
}

/// The station documents tools/tide-station wrote (stations/, from the same published values) read back to the same model, and the catalog finds each station from a point near it.
#[cfg(feature = "vsf")]
#[test]
fn the_station_fixtures_are_the_published_constants() {
    use crate::station::{Catalog, Station, unit};
    let docs: [(&[u8], &Published, (f64, f64)); 4] = [
        (include_bytes!("../stations/9414290.tide.vsf"), &SAN_FRANCISCO, (37.7, -122.6)),
        (include_bytes!("../stations/8518750.tide.vsf"), &THE_BATTERY, (40.6, -73.9)),
        (include_bytes!("../stations/9447130.tide.vsf"), &SEATTLE, (47.5, -122.5)),
        (include_bytes!("../stations/1612340.tide.vsf"), &HONOLULU, (21.4, -157.7)),
    ];
    let catalog = Catalog::parse(include_bytes!("../stations/catalog.vsf")).unwrap();
    for (doc, s, near) in docs {
        let (station, _) = Station::parse(doc).unwrap();
        let (tide, unknown) = station.tide();
        assert!(unknown.is_empty());
        let want = tide_of(s);
        assert!((tide.z0 - want.z0).abs() < 1e-6, "{}", s.name);
        for k in 0..N {
            // Amplitudes to the micrometre, phases to 2^-32 of a turn.
            assert!((tide.amp[k] - want.amp[k]).abs() < 1e-6 && (tide.phase[k] - want.phase[k]).abs() < 1e-8, "{} {}", s.name, CONSTITUENTS[k].name);
        }
        let (k, _) = catalog.nearest(unit(near.0, near.1)).unwrap();
        assert_eq!(catalog.stations[k].id, station.id);
        assert!(s.name.ends_with(station.id.as_str()));
    }
}

