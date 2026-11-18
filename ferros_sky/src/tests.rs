//! The engine against published references, every number copied here with its source.
//! - Meeus, Astronomical Algorithms, 2nd ed. (Willmann-Bell 1998): the worked examples 12.a, 12.b, 15.a, 25.a, 28.a, 47.a, 48.a, 49.a, 49.b.
//! - The US Naval Observatory's Astronomical Applications API (aa.usno.navy.mil/api, US public domain), fetched 2026-11-17: rise, set, civil twilight and transit for a UTC day (`rstt/oneday`, tz=0), the geocentric almanac data for a moment (`celnav`), and the moon's phases for 2026 (`moon/phases/year`).

extern crate std;
use super::*;
use std::println;
use std::vec::Vec;

/// Eagle seconds at a civil UTC date and time (Hinnant's days_from_civil, then POSIX seconds less the Eagle epoch).
fn eagle(y: i64, m: i64, d: i64, hh: i64, mm: i64, ss: f64) -> f64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (days * 86_400 + hh * 3600 + mm * 60 - EAGLE_EPOCH_UNIX_SECS) as f64 + ss
}

/// Days from J2000 of a Julian Ephemeris Day.
fn jde(j: f64) -> f64 {
    j - 2_451_545.
}

fn deg(r: f64) -> f64 {
    r / D2R
}

/// The difference of two angles in degrees, reduced to (−180, 180].
fn ddeg(a: f64, b: f64) -> f64 {
    rev(a - b + 180.) - 180.
}

fn close(name: &str, got: f64, want: f64, tol: f64) -> f64 {
    let e = (got - want).abs();
    println!("{name}: got {got:.7} want {want:.7} error {e:.2e}");
    assert!(e <= tol, "{name}: got {got} want {want} (tolerance {tol})");
    e
}

#[test]
fn meeus_25a_sun() {
    // Meeus 25.a: 1992 October 13.0 TD = JDE 2448908.5; λ = 199.90895°, α = 198.38083°, δ = −7.78507°, R = 0.99766 AU.
    let (s, l0, _) = sun_tt(jde(2_448_908.5));
    close("25.a L0", rev(l0), 201.80720, 1e-5);
    close("25.a λ", deg(s.lon), 199.90895, 1e-5);
    close("25.a α", deg(s.ra), 198.38083, 1e-5);
    close("25.a δ", deg(s.dec), -7.78507, 1e-5);
    close("25.a R", s.au, 0.99766, 1e-5);
}

#[test]
fn meeus_28a_equation_of_time() {
    // Meeus 28.a: 1992 October 13.0 TD, E = +3.427351° = 13 min 42.6 s (Meeus from VSOP87; ours from chapter 25's 0.01°, which allows a second or two).
    let (s, _, _) = sun_tt(jde(2_448_908.5));
    close("28.a E (s)", s.eot, 13. * 60. + 42.6, 2.);
}

#[test]
fn meeus_12_sidereal() {
    // Meeus 12.a: 1987 April 10, 0h UT: mean θ0 = 13h10m46.3668s, apparent 13h10m46.1351s (Δψ = −3.788″, ε = 23°26′36.85″ from 22.a).
    let d = jde(2_446_895.5);
    let n = nutation(d);
    let mean = Nutation { dpsi: 0., eps: n.eps };
    let hms = |h: f64, m: f64, s: f64| (h + m / 60. + s / 3600.) * 15.;
    close("12.a mean θ0", deg(sidereal(d, &mean)), hms(13., 10., 46.3668), 1e-6);
    close("12.a apparent θ0", deg(sidereal(d, &n)), hms(13., 10., 46.1351), 3e-4);
    close("22.a Δψ (″)", deg(n.dpsi) * 3600., -3.788, 0.5);
    // Meeus 12.b: 1987 April 10, 19h21m00s UT: mean θ = 128.7378734°.
    let d = jde(2_446_895.5) + (19. + 21. / 60.) / 24.;
    close("12.b mean θ", deg(sidereal(d, &mean)), 128.7378734, 1e-6);
}

#[test]
fn meeus_47a_moon() {
    // Meeus 47.a: 1992 April 12, 0h TD = JDE 2448724.5: λ = 133.162655°, β = −3.229126°, Δ = 368409.7 km, π = 0.991990°; apparent λ = 133.167265°, α = 134.688470°, δ = +13.768368°.
    let d = jde(2_448_724.5);
    let (l, b, km) = moon_geo(d);
    close("47.a λ", l, 133.162655, 2e-6);
    close("47.a β", b, -3.229126, 2e-6);
    close("47.a Δ (km)", km, 368_409.7, 0.1);
    let (m, _) = moon_tt(d);
    close("47.a π", deg(m.parallax), 0.991990, 2e-6);
    // The apparent values take Meeus's full nutation; ours is chapter 22's short series (0.5″ = 1.4e-4°).
    close("47.a apparent λ", deg(m.lon), 133.167265, 2e-4);
    close("47.a α", deg(m.ra), 134.688470, 2e-4);
    close("47.a δ", deg(m.dec), 13.768368, 2e-4);
}

#[test]
fn meeus_48a_illumination() {
    // Meeus 48.a: 1992 April 12, 0h TD: i = 69.0756°, k = 0.6786, χ = 285.0° (from the high-accuracy sun; ours is chapter 25's).
    let d = jde(2_448_724.5);
    let (s, _, _) = sun_tt(d);
    let (m, _) = moon_tt(d);
    let t = J2000_EAGLE + d * DAY;
    let il = illumination(&s, &m, Place::deg(0., 0.), t);
    close("48.a i", deg(il.phase_angle), 69.0756, 0.01);
    close("48.a k", il.fraction, 0.6786, 1e-4);
    close("48.a χ", deg(il.bright_limb), 285.0, 0.1);
    assert!(il.waxing, "48.a: 1992 April 12 is between first quarter and full");
}

#[test]
fn meeus_49_phases() {
    // Meeus 49.a: the new moon of 1977 February, k = −283: JDE 2443192.65118.
    close("49.a new moon JDE", phase_tt(-283, Quarter::New) + 2_451_545., 2_443_192.651_18, 1e-5);
    // Meeus 49.b: the first last quarter of 2044, k = 544.75: JDE 2467636.49186.
    close("49.b last quarter JDE", phase_tt(544, Quarter::Last) + 2_451_545., 2_467_636.491_86, 1e-5);
}

#[test]
fn meeus_15a_method() {
    // Meeus 15.a: Venus at Boston (φ = +42.3333°, L = 71.0833° west) on 1988 March 20, h0 = −0.5667°, ΔT = 56 s, θ0 = 177.74208° at 0h UT.
    // Venus's α and δ at 0h TD on March 19, 20, 21: (40.68021°, 18.04761°), (41.73129°, 18.44092°), (42.78204°, 18.82742°).
    // Meeus's result: transit m0 = 0.81980, rising m1 = 0.51766, setting m2 = 0.12130 (fractions of the UT day).
    // Our sampling and bisection on the same interpolated altitude must land on his roots.
    let interp = |y: [f64; 3], n: f64| {
        let (a, b) = (y[1] - y[0], y[2] - y[1]);
        y[1] + n / 2. * (a + b + n * (b - a))
    };
    let ra = [40.68021, 41.73129, 42.78204];
    let dec = [18.04761, 18.44092, 18.82742];
    let (lat, west) = (42.3333 * D2R, 71.0833);
    let h = |x: f64| -> (f64, f64) {
        let m = x / DAY;
        let n = m + 56. / DAY;
        let theta = 177.74208 + 360.985647 * m;
        let hour = (theta - west - interp(ra, n)) * D2R;
        (hour, interp(dec, n) * D2R)
    };
    let alt = |x: f64| {
        let (hour, d) = h(x);
        horizontal(hour, d, lat).alt - (-0.5667 * D2R)
    };
    let mut c = [Crossing { t: 0., rising: false }; 4];
    let n = crossings_of(alt, 0., DAY, &mut c);
    assert_eq!(n, 2);
    let rise = c[..n].iter().find(|x| x.rising).unwrap().t / DAY;
    let set = c[..n].iter().find(|x| !x.rising).unwrap().t / DAY;
    close("15.a rising m1", rise, 0.51766, 1e-4);
    close("15.a setting m2", set, 0.12130, 1e-4);
    let n = crossings_of(|x| sin(h(x).0), 0., DAY, &mut c);
    let transit = c[..n].iter().find(|x| x.rising).unwrap().t / DAY;
    close("15.a transit m0", transit, 0.81980, 1e-4);
}

/// What USNO publishes for a UTC day at a place: (hour, minute) of each event, None where it lists none.
struct UsnoDay {
    name: &'static str,
    lat: f64,
    lon: f64,
    date: (i64, i64, i64),
    rise: Option<(i64, i64)>,
    set: Option<(i64, i64)>,
    civil_begin: Option<(i64, i64)>,
    civil_end: Option<(i64, i64)>,
    transit: Option<(i64, i64)>,
    moonrise: Option<(i64, i64)>,
    moonset: Option<(i64, i64)>,
}

/// aa.usno.navy.mil/api/rstt/oneday?date=…&coords=…&tz=0, fetched 2026-11-17 (times UTC, rounded to the minute).
const USNO: [UsnoDay; 8] = [
    UsnoDay { name: "New York 2026-06-21", lat: 40.7128, lon: -74.0060, date: (2026, 6, 21), rise: Some((9, 25)), set: Some((0, 31)), civil_begin: Some((8, 52)), civil_end: Some((1, 4)), transit: Some((16, 58)), moonrise: Some((16, 50)), moonset: Some((4, 29)) },
    UsnoDay { name: "New York 2026-12-21", lat: 40.7128, lon: -74.0060, date: (2026, 12, 21), rise: Some((12, 17)), set: Some((21, 32)), civil_begin: Some((11, 46)), civil_end: Some((22, 3)), transit: Some((16, 54)), moonrise: Some((19, 0)), moonset: Some((9, 35)) },
    UsnoDay { name: "Sydney 2026-06-21", lat: -33.8688, lon: 151.2093, date: (2026, 6, 21), rise: Some((21, 0)), set: Some((6, 54)), civil_begin: Some((20, 32)), civil_end: Some((7, 22)), transit: Some((1, 57)), moonrise: Some((1, 25)), moonset: Some((13, 39)) },
    UsnoDay { name: "Sydney 2026-12-21", lat: -33.8688, lon: 151.2093, date: (2026, 12, 21), rise: Some((18, 41)), set: Some((9, 5)), civil_begin: Some((18, 12)), civil_end: Some((9, 35)), transit: Some((1, 53)), moonrise: Some((6, 6)), moonset: Some((16, 13)) },
    UsnoDay { name: "Greenwich 2026-03-20", lat: 51.4769, lon: -0.0005, date: (2026, 3, 20), rise: Some((6, 3)), set: Some((18, 13)), civil_begin: Some((5, 30)), civil_end: Some((18, 46)), transit: Some((12, 7)), moonrise: Some((6, 16)), moonset: Some((20, 35)) },
    UsnoDay { name: "Greenwich 2026-11-17", lat: 51.4769, lon: -0.0005, date: (2026, 11, 17), rise: Some((7, 21)), set: Some((16, 8)), civil_begin: Some((6, 44)), civil_end: Some((16, 45)), transit: Some((11, 45)), moonrise: Some((13, 13)), moonset: Some((23, 29)) },
    // Tromsø, midsummer: "continuously above the horizon" and "continuously above the twilight limit".
    UsnoDay { name: "Tromso 2026-06-21", lat: 69.6492, lon: 18.9553, date: (2026, 6, 21), rise: None, set: None, civil_begin: None, civil_end: None, transit: Some((10, 46)), moonrise: Some((10, 18)), moonset: Some((22, 14)) },
    // Tromsø, midwinter: the sun "continuously below the horizon" with civil twilight 08:31 to 12:53; the moon "continuously above the horizon".
    UsnoDay { name: "Tromso 2026-12-21", lat: 69.6492, lon: 18.9553, date: (2026, 12, 21), rise: None, set: None, civil_begin: Some((8, 31)), civil_end: Some((12, 53)), transit: None, moonrise: None, moonset: None },
];

/// Minutes between a computed moment and a published (hour, minute) on the day starting at `day`.
fn minutes(got: f64, day: f64, want: (i64, i64)) -> f64 {
    (got - (day + (want.0 * 3600 + want.1 * 60) as f64)) / 60.
}

#[test]
fn usno_sun_and_moon_days() {
    let (mut sun_worst, mut moon_worst) = (0f64, 0f64);
    for u in USNO.iter() {
        let place = Place::deg(u.lat, u.lon);
        let day = eagle(u.date.0, u.date.1, u.date.2, 0, 0, 0.);
        let sd = SunDay::new(day, day + DAY, place);
        let md = MoonDay::new(day, day + DAY, place, day + DAY / 2.);
        let check = |what: &str, got: Option<f64>, want: Option<(i64, i64)>, tol: f64, worst: &mut f64| match (got, want) {
            (Some(g), Some(w)) => {
                let e = minutes(g, day, w);
                println!("{} {what}: {e:+.2} min", u.name);
                assert!(e.abs() <= tol, "{} {what}: off by {e} minutes", u.name);
                *worst = worst.max(e.abs());
            }
            (None, None) => {}
            _ => panic!("{} {what}: got {got:?}, USNO {want:?}", u.name),
        };
        check("sunrise", sd.sun.rise(), u.rise, 1., &mut sun_worst);
        check("sunset", sd.sun.set(), u.set, 1., &mut sun_worst);
        check("civil dawn", sd.civil.rise(), u.civil_begin, 1., &mut sun_worst);
        check("civil dusk", sd.civil.set(), u.civil_end, 1., &mut sun_worst);
        // USNO lists no transit at Tromsø in midwinter (the sun stays below the horizon), so that day's noon goes unchecked.
        if u.transit.is_some() {
            check("solar noon", Some(sd.noon), u.transit, 1., &mut sun_worst);
        }
        check("moonrise", md.moon.rise(), u.moonrise, 2., &mut moon_worst);
        check("moonset", md.moon.set(), u.moonset, 2., &mut moon_worst);
    }
    // The polar days said outright.
    let tromso = Place::deg(69.6492, 18.9553);
    let jun = eagle(2026, 6, 21, 0, 0, 0.);
    let dec = eagle(2026, 12, 21, 0, 0, 0.);
    let s = SunDay::new(jun, jun + DAY, tromso);
    assert_eq!(s.sun, Pass::Up);
    assert_eq!(s.civil, Pass::Up);
    let s = SunDay::new(dec, dec + DAY, tromso);
    assert_eq!(s.sun, Pass::Down);
    assert!(matches!(s.astronomical, Pass::Crosses { rise: Some(_), set: Some(_) }));
    assert_eq!(MoonDay::new(dec, dec + DAY, tromso, dec).moon, Pass::Up);
    println!("worst: sun events {sun_worst:.2} min, moon rise/set {moon_worst:.2} min");
}

#[test]
fn polar_bands() {
    let tromso = Place::deg(69.6492, 18.9553);
    // Midsummer: the sun's lowest is about +3° at the lower transit (22:46 UTC), inside golden hour's −4° to +6°: one band cut in two by the day's ends.
    let jun = eagle(2026, 6, 21, 0, 0, 0.);
    let s = SunDay::new(jun, jun + DAY, tromso);
    let g = s.golden.spans();
    println!("Tromso June golden: {:?}", g.iter().map(|x| ((x.from - jun) / 3600., (x.to - jun) / 3600.)).collect::<Vec<_>>());
    assert_eq!(g.len(), 2);
    assert_eq!(g[0].from, jun);
    assert_eq!(g[1].to, jun + DAY);
    assert_eq!(s.blue.n, 0);
    // Midwinter: the noon sun peaks near −3°, so golden hour holds noon and blue hour comes before and after.
    let dec = eagle(2026, 12, 21, 0, 0, 0.);
    let s = SunDay::new(dec, dec + DAY, tromso);
    println!("Tromso December noon altitude {:.3}°", deg(s.noon_altitude));
    let g = s.golden.spans();
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].side(s.noon), Side::Thru);
    let b = s.blue.spans();
    assert_eq!(b.len(), 2);
    assert_eq!(b[0].side(s.noon), Side::Morning);
    assert_eq!(b[1].side(s.noon), Side::Evening);
    // A middle latitude has the four photographers' spans, two each side of noon.
    let gw = Place::deg(51.4769, -0.0005);
    let nov = eagle(2026, 11, 17, 0, 0, 0.);
    let s = SunDay::new(nov, nov + DAY, gw);
    assert_eq!(s.golden.n, 2);
    assert_eq!(s.blue.n, 2);
    assert_eq!(s.golden.spans[0].side(s.noon), Side::Morning);
    assert_eq!(s.golden.spans[1].side(s.noon), Side::Evening);
    // The blue hour ends where the golden begins.
    assert!((s.blue.spans[0].to - s.golden.spans[0].from).abs() < 0.1);
}

/// aa.usno.navy.mil/api/moon/phases/year?year=2026, fetched 2026-11-17: (month, day, hour, minute, phase), UTC.
const PHASES_2026: [(i64, i64, i64, i64, Quarter); 50] = {
    use Quarter::*;
    [
        (1, 3, 10, 3, Full), (1, 10, 15, 48, Last), (1, 18, 19, 52, New), (1, 26, 4, 47, First),
        (2, 1, 22, 9, Full), (2, 9, 12, 43, Last), (2, 17, 12, 1, New), (2, 24, 12, 27, First),
        (3, 3, 11, 38, Full), (3, 11, 9, 38, Last), (3, 19, 1, 23, New), (3, 25, 19, 18, First),
        (4, 2, 2, 12, Full), (4, 10, 4, 51, Last), (4, 17, 11, 52, New), (4, 24, 2, 32, First),
        (5, 1, 17, 23, Full), (5, 9, 21, 10, Last), (5, 16, 20, 1, New), (5, 23, 11, 11, First),
        (5, 31, 8, 45, Full), (6, 8, 10, 0, Last), (6, 15, 2, 54, New), (6, 21, 21, 55, First),
        (6, 29, 23, 56, Full), (7, 7, 19, 29, Last), (7, 14, 9, 43, New), (7, 21, 11, 5, First),
        (7, 29, 14, 36, Full), (8, 6, 2, 21, Last), (8, 12, 17, 37, New), (8, 20, 2, 46, First),
        (8, 28, 4, 18, Full), (9, 4, 7, 51, Last), (9, 11, 3, 27, New), (9, 18, 20, 44, First),
        (9, 26, 16, 49, Full), (10, 3, 13, 25, Last), (10, 10, 15, 50, New), (10, 18, 16, 12, First),
        (10, 26, 4, 12, Full), (11, 1, 20, 28, Last), (11, 9, 7, 2, New), (11, 17, 11, 48, First),
        (11, 24, 14, 53, Full), (12, 1, 6, 8, Last), (12, 9, 0, 52, New), (12, 17, 5, 42, First),
        (12, 24, 1, 28, Full), (12, 30, 18, 59, Last),
    ]
};

#[test]
fn usno_phases_2026() {
    let mut t = eagle(2026, 1, 1, 0, 0, 0.);
    let (mut worst, mut sum) = (0f64, 0.);
    for &(mo, d, hh, mm, q) in PHASES_2026.iter() {
        let (got_q, got) = next_phase(t);
        assert_eq!(got_q, q, "phase after {mo}-{d}");
        let e = (got - eagle(2026, mo, d, hh, mm, 0.)) / 60.;
        worst = worst.max(e.abs());
        sum += e.abs();
        assert!(e.abs() <= 1., "{q:?} 2026-{mo}-{d}: off by {e} minutes");
        t = got;
    }
    println!("2026 phases vs USNO: worst {worst:.2} min, mean {:.2} min", sum / 50.);
}

/// aa.usno.navy.mil/api/celnav, fetched 2026-11-17: (place, moment, body, declination, GHA, computed altitude hc, azimuth Zn, parallax in altitude pa), degrees; hc is geocentric (pa is the correction to it).
/// (latitude, longitude, (year, month, day, hour), body, δ, GHA, hc, Zn, pa).
type Celnav = (f64, f64, (i64, i64, i64, i64), Body, f64, f64, f64, f64, f64);
const CELNAV: [Celnav; 4] = [
    (51.4769, -0.0005, (2026, 11, 17, 12), Body::Sun, -19.051301, 3.759323, 19.394854, 183.766701, 0.002323),
    (51.4769, -0.0005, (2026, 11, 17, 18), Body::Moon, -11.802696, 356.445206, 26.645164, 176.105875, 0.826974),
    (-33.8688, 151.2093, (2026, 6, 21, 3), Body::Sun, 23.437765, 224.566044, 30.76049, 343.125706, 0.00206),
    (-33.8688, 151.2093, (2026, 6, 21, 3), Body::Moon, 2.345985, 143.843726, 19.177525, 73.40345, 0.902844),
];

#[test]
fn usno_celnav_positions() {
    let mut worst = [0f64; 2];
    for &(lat, lon, (y, mo, d, hh), body, dec, gha, hc, zn, pa) in CELNAV.iter() {
        let place = Place::deg(lat, lon);
        let t = eagle(y, mo, d, hh, 0, 0.);
        let (got_dec, got_gha, geo, topo) = match body {
            Body::Sun => {
                let s = Sun::at(t);
                let h = s.horizontal(place);
                (s.dec, s.gast - s.ra, h, h)
            }
            Body::Moon => {
                let m = Moon::at(t);
                (m.dec, m.gast - m.ra, m.geocentric(place), m.topocentric(place))
            }
        };
        let k = body as usize;
        let errs = [
            deg(got_dec) - dec,
            ddeg(deg(got_gha), gha),
            deg(geo.alt) - hc,
            ddeg(deg(geo.az), zn),
            deg(topo.alt) - (hc - pa),
        ];
        println!("{body:?} {y}-{mo}-{d} {hh}h at {lat},{lon}: errors (degrees) δ {:+.4} GHA {:+.4} alt {:+.4} az {:+.4} topocentric alt {:+.4}", errs[0], errs[1], errs[2], errs[3], errs[4]);
        for e in errs {
            worst[k] = worst[k].max(e.abs());
            assert!(e.abs() < 0.01, "{body:?}: {errs:?}");
        }
    }
    println!("worst: sun {:.4}°, moon {:.4}°", worst[0], worst[1]);
}

#[test]
fn deadlines_walk_the_day() {
    // Greenwich, 2026-11-17 (UTC day): the wakes in order are the sun's twelve level crossings, noon, the first quarter (11:48), moonrise and moonset, then the day's end.
    let place = Place::deg(51.4769, -0.0005);
    let (start, end) = local_day(eagle(2026, 11, 17, 9, 0, 0.), 0.);
    assert_eq!(start, eagle(2026, 11, 17, 0, 0, 0.));
    let mut t = start;
    let mut wakes = Vec::new();
    while t < end {
        t = next_deadline(t, place, end);
        wakes.push(t);
    }
    println!("{:?}", wakes.iter().map(|w| (w - start) / 3600.).collect::<Vec<_>>());
    assert_eq!(wakes.len(), 17);
    assert_eq!(*wakes.last().unwrap(), end);
    for w in wakes.windows(2) {
        assert!(w[1] > w[0]);
    }
    // Every published moment of the day is among the wakes (USNO's times, rounded to the minute; the first quarter from the 2026 phase list).
    for want in [(6, 44), (7, 21), (11, 45), (11, 48), (13, 13), (16, 8), (16, 45), (23, 29)] {
        let near = wakes.iter().map(|&w| minutes(w, start, want).abs()).fold(f64::MAX, f64::min);
        assert!(near <= 1., "no wake near {want:?}: nearest {near} min");
    }
    // The sun's day agrees with the walk.
    let sd = SunDay::new(start, end, place);
    assert_eq!(next_crossing(Body::Sun, start, end, place, SUNRISE, true), sd.sun.rise());
}

#[test]
fn curves_are_hermite_ready() {
    // The curve's rates are the true slopes: against the samples' own central difference, a smooth day's curve agrees to the step's third-order term.
    let place = Place::deg(51.4769, -0.0005);
    let day = eagle(2026, 11, 17, 0, 0, 0.);
    let sd = SunDay::new(day, day + DAY, place);
    let md = MoonDay::new(day, day + DAY, place, day + DAY / 2.);
    for c in [&sd.curve, &md.curve] {
        assert_eq!(c.start, day);
        assert!((c.start + (CURVE - 1) as f64 * c.step - (day + DAY)).abs() < 1e-6);
        for k in 1..CURVE - 1 {
            let fd = 0.5 * (c.alt[k + 1] - c.alt[k - 1]);
            assert!((fd - c.rate[k]).abs() < 2e-3, "sample {k}: {fd} vs {}", c.rate[k]);
        }
    }
    // The sun's curve crosses −0.8333° between the samples that hold the sunrise.
    let rise = sd.sun.rise().unwrap();
    let k = ((rise - day) / sd.curve.step) as usize;
    assert!((sd.curve.alt[k] as f64) < SUNRISE && (sd.curve.alt[k + 1] as f64) > SUNRISE);
    println!("noon {:.3}° at {:.3} h; horizon for the moon {:.4}°; light {:?}", deg(sd.noon_altitude), (sd.noon - day) / 3600., deg(md.horizon), md.light);
    // USNO for 2026-11-17 at Greenwich: first quarter (11:48) so near 50% lit and waxing at noon.
    assert!(md.light.waxing);
    assert!((md.light.fraction - 0.5).abs() < 0.01);
    assert_eq!(md.next_phase, Quarter::Full);
}

#[test]
fn delta_t_matches_its_table() {
    // The C¹ join passes thru every knot; 2026.0 is 69.11 s in IERS/USNO's table (one year past the last knot).
    for (k, &v) in DELTA_T.iter().enumerate() {
        let y = DELTA_T_FIRST + k as f64 * DELTA_T_STEP;
        let t = J2000_EAGLE + (y - 2000.) * 365.25 * DAY;
        close("ΔT knot", delta_t(t), v, 1e-9);
    }
    close("ΔT 2026.0", delta_t(eagle(2026, 1, 1, 0, 0, 0.)), 69.1099, 0.1);
}
