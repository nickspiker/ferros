//! The orbits against an independent source, and the place's combination.
//! - CelesTrak's GNSS GP elements (`gp.php?GROUP=gnss&FORMAT=tle`) and its GPS almanac (`GPS/almanac/Yuma/almanac.yuma.txt`, week 397 mod 1024), both fetched over plain HTTP 2026-11-18 (testdata/): the almanac carried by IS-GPS-200's own equations (table 20-IV), an orbit source and a method that share nothing with the elements' path, must put every GPS satellite in the same place in the sky.

extern crate std;
use super::place::*;
use super::*;
use std::println;
use std::string::String;
use std::vec::Vec;

const TLE: &str = include_str!("testdata/gnss-2026-11-18.tle");
const YUMA: &str = include_str!("testdata/yuma-2026-11-18.txt");

/// Eagle seconds at a civil UTC date and time (ferros_sky's tests use the same).
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

fn all() -> Vec<Elements> {
    let mut v = Vec::new();
    let (taken, refused) = each_tle(TLE.as_bytes(), |e| v.push(e));
    assert_eq!(taken as usize, v.len());
    assert_eq!(refused, 0);
    v
}

#[test]
fn the_four_systems_are_read_and_nothing_else() {
    let v = all();
    let count = |s: u8| v.iter().filter(|e| e.system == s).count();
    println!("GPS {} GLONASS {} Galileo {} BeiDou {}", count(GPS), count(GLONASS), count(GALILEO), count(BEIDOU));
    assert_eq!(count(GPS), 32);
    assert!(count(GLONASS) >= 22, "GLONASS {}", count(GLONASS));
    assert!(count(GALILEO) >= 24, "Galileo {}", count(GALILEO));
    assert!(count(BEIDOU) >= 40, "BeiDou {}", count(BEIDOU));
    // Every GPS PRN 1-32 once.
    let mut prns: Vec<u8> = v.iter().filter(|e| e.system == GPS).map(|e| e.number).collect();
    prns.sort();
    assert_eq!(prns, (1..=32).collect::<Vec<u8>>());
    assert_eq!(identify("QZS-2 (QZSS/PRN 194)"), None);
    assert_eq!(identify("SES-15 (WAAS/PRN 133)"), None);
    assert_eq!(identify("GSAT-8 (GAGAN/PRN 127)"), None);
    assert_eq!(identify("IRNSS-1B"), None);
    assert_eq!(identify("0 BEIDOU-3 M1 (C19)"), Some((BEIDOU, 19)));
    assert_eq!(identify("GPS BIIR-5  (PRN 22)    "), Some((GPS, 22)));
    assert_eq!(identify("GSAT0201 (GALILEO 5)"), Some((GALILEO, 0)));
}

#[test]
fn a_set_that_fails_its_checksum_is_refused() {
    let l0 = "GPS BIIR-5  (PRN 22)";
    let mut lines = TLE.lines().skip_while(|l| !l.starts_with(l0));
    lines.next();
    let (l1, l2) = (lines.next().unwrap(), lines.next().unwrap());
    let e = tle(l0, l1, l2).unwrap();
    println!("PRN 22: epoch {:.3} days from J2000, i {:.4} deg, n {:.8} rev/day", days_j2000(e.epoch), e.inclination / D2R, e.motion);
    // One digit of the inclination changed.
    let bad = l2.replacen("54.8244", "54.8245", 1);
    assert_eq!(tle(l0, l1, &bad), None);
    // The two lines of different satellites.
    let other = TLE.lines().skip_while(|l| !l.starts_with("GPS BIIR-8")).nth(2).unwrap();
    assert_eq!(tle(l0, l1, other), None);
    // The epoch: 26322.58878740 is 2026, day 322 (November 18) at 0.5888 of the day.
    let want = eagle(2026, 11, 18, 0, 0, 0.) + 0.588_787_40 * DAY;
    assert!((e.epoch - want).abs() < 1e-3, "{} {}", e.epoch, want);
}

/// A GPS almanac entry (Yuma): what IS-GPS-200 needs.
struct Alm {
    prn: u8,
    e: f64,
    toa: f64,
    i: f64,
    omega_dot: f64,
    sqrt_a: f64,
    omega0: f64,
    w: f64,
    m0: f64,
    week: i64,
}

fn yuma() -> Vec<Alm> {
    let mut out = Vec::new();
    let mut cur: Vec<(String, f64)> = Vec::new();
    for l in YUMA.lines().chain(core::iter::once("****")) {
        if l.starts_with("****") {
            if !cur.is_empty() {
                let g = |k: &str| cur.iter().find(|(n, _)| n.starts_with(k)).unwrap().1;
                out.push(Alm { prn: g("ID") as u8, e: g("Eccentricity"), toa: g("Time of Applicability"), i: g("Orbital Inclination"), omega_dot: g("Rate of Right Ascen"), sqrt_a: g("SQRT(A)"), omega0: g("Right Ascen at Week"), w: g("Argument of Perigee"), m0: g("Mean Anom"), week: g("week") as i64 });
                cur.clear();
            }
            continue;
        }
        if let Some((k, v)) = l.split_once(':') {
            if let Ok(x) = v.trim().parse::<f64>() {
                cur.push((k.trim().into(), x));
            }
        }
    }
    out
}

/// IS-GPS-200 table 20-IV for an almanac: the satellite in the earth's frame at Eagle second `t`, km.
fn alm_ecef(a: &Alm, t: f64) -> [f64; 3] {
    const MU: f64 = 3.986_005e14;
    const OMEGA_E: f64 = 7.292_115_146_7e-5;
    // GPS time: the POSIX seconds since 1980-01-06, plus the 18 leap seconds since.
    let gps = t + EAGLE_EPOCH_UNIX_SECS as f64 - 315_964_800. + 18.;
    // The almanac's week is modulo 1024: the third rollover (2019-04-07) is the one this file is past.
    let toe = (a.week + 2048) as f64 * 604_800. + a.toa;
    let tk = gps - toe;
    let big_a = a.sqrt_a * a.sqrt_a;
    let n = sqrt(MU / (big_a * big_a * big_a));
    let m = a.m0 + n * tk;
    let mut ea = m;
    for _ in 0..16 {
        ea -= (ea - a.e * sin(ea) - m) / (1. - a.e * cos(ea));
    }
    let nu = atan2(sqrt(1. - a.e * a.e) * sin(ea), cos(ea) - a.e);
    let phi = nu + a.w;
    let r = big_a * (1. - a.e * cos(ea));
    let (xp, yp) = (r * cos(phi), r * sin(phi));
    let om = a.omega0 + (a.omega_dot - OMEGA_E) * tk - OMEGA_E * a.toa;
    [(xp * cos(om) - yp * cos(a.i) * sin(om)) / 1000., (xp * sin(om) + yp * cos(a.i) * cos(om)) / 1000., yp * sin(a.i) / 1000.]
}

#[test]
fn gps_from_the_elements_agrees_with_the_almanac() {
    let v = all();
    let alm = yuma();
    assert_eq!(alm.len(), 32, "the almanac's satellites");
    let places = [Place::deg(37.8, -122.4), Place::deg(-33.9, 151.2), Place::deg(64.1, -21.9), Place::deg(0.3, 32.6), Place::deg(-54.8, -68.3)];
    let (mut worst, mut worst_km, mut n) = (0f64, 0f64, 0u32);
    // Six hours either side of the elements' day, every three hours.
    for h in 0..5 {
        let t = eagle(2026, 11, 18, 12, 0, 0.) + (h as f64 - 2.) * 3. * 3600.;
        for a in &alm {
            let Some(e) = v.iter().find(|e| e.system == GPS && e.number == a.prn) else { continue };
            let o = Orbit::new(e);
            let (p, q) = (o.ecef(t), alm_ecef(a, t));
            let km = sqrt((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2));
            worst_km = worst_km.max(km);
            for &pl in &places {
                let (ea, aa) = look(p, pl);
                let (eb, ab) = look(q, pl);
                if eb < 5. * D2R {
                    continue;
                }
                let d = apart(((ea / D2R) as f32, (aa / D2R) as f32), ((eb / D2R) as f32, (ab / D2R) as f32)) as f64;
                worst = worst.max(d);
                n += 1;
            }
        }
    }
    println!("{n} looks above 5 degrees: the elements and the almanac within {worst:.4} degree of sky; the satellites within {worst_km:.1} km");
    // Measured 2026-11-18: 0.095 degree at worst, the satellites 20 to 38 km apart whether the elements were hours or a day old (SDP4's lunar, solar and resonance terms and the short-period ones, which `Orbit` leaves out, and the almanac's own error days from its time of applicability). The bound is twice the worst seen: still a fifth of the whole degree the chip reports.
    assert!(worst < 0.2, "worst {worst} degree");
    assert!(n > 100);
}

#[test]
fn the_sky_from_a_place_holds_the_four_systems() {
    let v = all();
    let orbits: Vec<Orbit> = v.iter().map(Orbit::new).collect();
    let t = eagle(2026, 11, 19, 3, 0, 0.);
    for (name, p) in [("Wellington", Place::deg(-41.29, 174.78)), ("Denver", Place::deg(39.74, -104.99)), ("Tromso", Place::deg(69.65, 18.96))] {
        let mut out = [Bird::default(); 1 << 7];
        let n = sky(&orbits, t, p, 5. * D2R, &mut out);
        let by = |s: u8| out[..n].iter().filter(|b| b.system == s).count();
        println!("{name}: {n} above 5 degrees: GPS {} GLONASS {} Galileo {} BeiDou {}", by(GPS), by(GLONASS), by(GALILEO), by(BEIDOU));
        assert!((24..=64).contains(&n), "{name}: {n}");
        assert!(by(GPS) >= 6 && by(GLONASS) >= 4 && by(GALILEO) >= 4, "{name}");
        for b in &out[..n] {
            assert!(b.el >= 5. && b.el <= 90. && b.az >= 0. && b.az < 360.);
        }
    }
}

#[test]
fn the_sidereal_time_matches_meeus() {
    // Meeus 12.a: 1987 April 10, 0h UT: mean sidereal time 13h 10m 46.3668s.
    let t = eagle(1987, 4, 10, 0, 0, 0.);
    let want = (13. + 10. / 60. + 46.3668 / 3600.) * 15. * D2R;
    let got = gmst(t);
    println!("GMST {got:.9} want {want:.9}");
    assert!((got - want).abs() < 1e-6);
}

fn c(lat: f64, lon: f64, km: f64) -> Circle {
    Circle { lat: lat * D2R, lon: lon * D2R, radius: km * 1000. }
}

#[test]
fn a_tight_place_agreed_with_stands() {
    // A last fix 10 km from a zone's city, both inside each other's reach.
    let v = combine(&[c(39.74, -104.99, 2000.), c(39.83, -104.99, 5.)]).unwrap();
    assert_eq!(v.tightest, 1);
    assert_eq!((v.agreed, v.contradicted), (1, 0));
    assert_eq!(v.circle, c(39.83, -104.99, 5.));
}

#[test]
fn a_tight_place_contradicted_is_widened_to_hold_both() {
    // A last fix in Denver, the zone in effect now Tokyo's: neither silently wins.
    let fix = c(39.74, -104.99, 50.);
    let zone = c(35.69, 139.69, 2000.);
    let v = combine(&[fix, zone]).unwrap();
    let d = distance(&fix, &zone);
    println!("{:.0} km apart: the circle {:.0} km", d / 1000., v.circle.radius / 1000.);
    assert_eq!((v.agreed, v.contradicted), (0, 1));
    assert!((v.circle.radius - (d + fix.radius + zone.radius) / 2.).abs() < 1.);
    // It holds both circles whole.
    assert!(distance(&v.circle, &fix) + fix.radius <= v.circle.radius + 1.);
    assert!(distance(&v.circle, &zone) + zone.radius <= v.circle.radius + 1.);
}

#[test]
fn a_last_fix_widens_with_its_age() {
    let a = last_fix(0., 0., Some(8.), Some(0.));
    let b = last_fix(0., 0., Some(8.), Some(3600.));
    assert_eq!(a.radius, FIX_FLOOR_M + 8.);
    assert_eq!(b.radius, FIX_FLOOR_M + 8. + DRIFT_M_S * 3600.);
    assert_eq!(last_fix(0., 0., None, None).radius, UNDATED_M);
    assert_eq!(combine(&[]), None);
}

const BRDC: &str = include_str!("testdata/brdc-gps-2026-11-19.rnx");

#[test]
fn broadcast_ephemerides_agree_with_the_almanac() {
    // BKG's BRDC for 2026-11-19 (day 323, the first ephemeris of each GPS satellite) by IS-GPS-200's ephemeris equations, against the almanac of 2026-11-18 by its own: two sources, the same satellites to an almanac's few km (1.1 to 3.6 km measured).
    let mut v = Vec::new();
    let (taken, refused) = eph::each_gps(BRDC.as_bytes(), |e| v.push(e));
    assert_eq!(refused, 0);
    assert!(taken >= 30, "A#{taken}");
    let alm = yuma();
    // 2026-11-19 03:00 UTC as Eagle seconds, and as GPS seconds of the ephemeris's week.
    let t = eagle(2026, 11, 19, 3, 0, 0.);
    let gps = t + EAGLE_EPOCH_UNIX_SECS as f64 - 315_964_800. + 18.;
    let mut worst: f64 = 0.;
    for e in &v {
        let Some(a) = alm.iter().find(|a| a.prn == e.prn) else { continue };
        let sow = gps - e.week as f64 * 604_800.;
        let p = eph::gps_ecef(e, sow);
        let q = alm_ecef(a, t);
        let d = sqrt((p[0] / 1000. - q[0]).powi(2) + (p[1] / 1000. - q[1]).powi(2) + (p[2] / 1000. - q[2]).powi(2));
        let r = sqrt(p[0] * p[0] + p[1] * p[1] + p[2] * p[2]) / 1000.;
        println!("G{:02} week A#{} toe A#{} radius {r:.0} km, from the almanac {d:.2} km", e.prn, e.week, e.toe);
        assert!((26_000. ..27_000.).contains(&r));
        worst = worst.max(d);
    }
    assert!(worst < 8., "the worst A#{worst} km");
}
