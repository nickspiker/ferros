//! Time zones (owner, 2026-11-10: "arabic needs to match wall clock with DST legal, dozenal stays"): a zone's current rule as tzdata writes it at the end of every zone file, a POSIX TZ string (`PST8PDT,M3.2.0,M11.1.0`), and from it, at any instant, the zone's standard offset (what dozenal and hex show, all year) and its legal one (what arabic shows: the standard offset, or the daylight one while it is in effect), and when that next changes.
//!
//! The grammar (POSIX.1-2017 8.3, with tzdata's extensions): `std offset [dst [offset] [,start[/time],end[/time]]]`. A name is three or more letters, or anything in angle brackets (`<+0530>`). An offset is `[+-]hh[:mm[:ss]]`, west of UTC positive (so `PST8` is eight hours behind). A daylight name with no offset is one hour ahead of standard. A date is `Mm.w.d` (month, week 1-5 where 5 is the last, weekday 0 Sunday), `Jn` (day 1-365, February 29 never counted) or `n` (day 0-365, counting it). A time defaults to 02:00 and may be negative or past 24 h (tzdata's extension). A rule with a daylight name but no dates is not one tzdata writes, and is refused.
//!
//! All integer, seconds and days, no heap: the kernel uses it.
#![no_std]

/// A zone's rule: its standard offset east of UTC in seconds, and, if it keeps daylight time, the daylight offset and the year's two changes (each in local time: the start in standard time, the end in daylight time, as POSIX says).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub std: i32,
    pub dst: Option<Daylight>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Daylight {
    pub offset: i32,
    pub start: (Date, i32),
    pub end: (Date, i32),
}

/// A day of the year as a rule names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Date {
    /// Month 1-12, week 1-5 (5: the last), weekday 0-6 (0 Sunday).
    Mwd(u8, u8, u8),
    /// Day 1-365, February 29 never counted.
    Julian1(u16),
    /// Day 0-365, February 29 counted.
    Julian0(u16),
}

/// The offsets in effect at an instant: standard and legal, east of UTC in seconds, and the next instant (Unix seconds, UTC) the legal one changes, None for a zone with no daylight time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    pub standard: i32,
    pub legal: i32,
    pub next: Option<i64>,
}

impl Rule {
    /// Parse a POSIX TZ string; None if it is not one this reads.
    pub fn parse(s: &str) -> Option<Rule> {
        let b = s.as_bytes();
        let mut i = 0;
        name(b, &mut i)?;
        let std = -offset(b, &mut i)?;
        if i == b.len() {
            return Some(Rule { std, dst: None });
        }
        name(b, &mut i)?;
        let off = if i < b.len() && b[i] != b',' { -offset(b, &mut i)? } else { std + 3600 };
        if i == b.len() || b[i] != b',' {
            return None;
        }
        i += 1;
        let start = when(b, &mut i)?;
        if i == b.len() || b[i] != b',' {
            return None;
        }
        i += 1;
        let end = when(b, &mut i)?;
        if i != b.len() {
            return None;
        }
        Some(Rule { std, dst: Some(Daylight { offset: off, start, end }) })
    }

    /// The offsets at `t` (Unix seconds, UTC), and the next change.
    pub fn at(&self, t: i64) -> Now {
        let Some(d) = self.dst else { return Now { standard: self.std, legal: self.std, next: None } };
        // The changes of the year t falls in (by standard time) and of the next, in UTC: the start counted from standard time, the end from daylight time.
        let year = civil_from_days((t + self.std as i64).div_euclid(86400)).0;
        let changes = |y: i64| -> (i64, i64) {
            let s = days_from_civil(y, 1, 1) + day_of_year(y, d.start.0) as i64;
            let e = days_from_civil(y, 1, 1) + day_of_year(y, d.end.0) as i64;
            (s * 86400 + d.start.1 as i64 - self.std as i64, e * 86400 + d.end.1 as i64 - d.offset as i64)
        };
        // Every change near t, in order: the year before, this year and the next, so a southern zone (daylight across New Year) and an instant near either end are both covered.
        let mut evs = [(0i64, false); 6];
        for (k, y) in [year - 1, year, year + 1].into_iter().enumerate() {
            let (s, e) = changes(y);
            evs[2 * k] = (s, true);
            evs[2 * k + 1] = (e, false);
        }
        evs.sort_unstable_by_key(|e| e.0);
        // In daylight time when the last change at or before t was a start.
        let last = evs.iter().rev().find(|e| e.0 <= t);
        let on = last.is_some_and(|e| e.1);
        let next = evs.iter().find(|e| e.0 > t).map(|e| e.0);
        Now { standard: self.std, legal: if on { d.offset } else { self.std }, next }
    }
}

/// A zone name: letters, three or more, or anything between `<` and `>`.
fn name(b: &[u8], i: &mut usize) -> Option<()> {
    if *i < b.len() && b[*i] == b'<' {
        let close = b[*i..].iter().position(|&c| c == b'>')?;
        *i += close + 1;
        return Some(());
    }
    let start = *i;
    while *i < b.len() && b[*i].is_ascii_alphabetic() {
        *i += 1;
    }
    (*i - start >= 3).then_some(())
}

/// `[+-]h[h][:mm[:ss]]` in seconds, as written (west positive for an offset).
fn offset(b: &[u8], i: &mut usize) -> Option<i32> {
    let neg = match b.get(*i) {
        Some(b'-') => {
            *i += 1;
            true
        }
        Some(b'+') => {
            *i += 1;
            false
        }
        _ => false,
    };
    let mut secs = number(b, i)? * 3600;
    for scale in [60, 1] {
        if *i < b.len() && b[*i] == b':' {
            *i += 1;
            secs += number(b, i)? * scale;
        } else {
            break;
        }
    }
    Some(if neg { -secs } else { secs })
}

fn number(b: &[u8], i: &mut usize) -> Option<i32> {
    let start = *i;
    let mut n = 0i32;
    while *i < b.len() && b[*i].is_ascii_digit() {
        n = n * 10 + (b[*i] - b'0') as i32;
        *i += 1;
    }
    (*i > start).then_some(n)
}

/// A date and its time of day (default 02:00).
fn when(b: &[u8], i: &mut usize) -> Option<(Date, i32)> {
    let date = match b.get(*i) {
        Some(b'M') => {
            *i += 1;
            let m = number(b, i)?;
            if b.get(*i) != Some(&b'.') {
                return None;
            }
            *i += 1;
            let w = number(b, i)?;
            if b.get(*i) != Some(&b'.') {
                return None;
            }
            *i += 1;
            let d = number(b, i)?;
            if !(1..=12).contains(&m) || !(1..=5).contains(&w) || !(0..=6).contains(&d) {
                return None;
            }
            Date::Mwd(m as u8, w as u8, d as u8)
        }
        Some(b'J') => {
            *i += 1;
            let n = number(b, i)?;
            if !(1..=365).contains(&n) {
                return None;
            }
            Date::Julian1(n as u16)
        }
        _ => {
            let n = number(b, i)?;
            if !(0..=365).contains(&n) {
                return None;
            }
            Date::Julian0(n as u16)
        }
    };
    let time = if b.get(*i) == Some(&b'/') {
        *i += 1;
        offset(b, i)?
    } else {
        7200
    };
    Some((date, time))
}

fn leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// The day of year (0 = January 1) a rule's date falls on in year `y`.
fn day_of_year(y: i64, d: Date) -> u32 {
    match d {
        Date::Julian0(n) => n as u32,
        // February 29 is never counted: a day past February in a leap year is one later.
        Date::Julian1(n) => n as u32 - 1 + (leap(y) && n > 59) as u32,
        Date::Mwd(m, w, wd) => {
            let first = days_from_civil(y, m as u32, 1);
            // 1970-01-01 was a Thursday (4).
            let first_wd = (first + 4).rem_euclid(7) as u32;
            let mut day = (wd as u32 + 7 - first_wd) % 7 + (w as u32 - 1) * 7;
            let len = month_len(y, m as u32);
            while day >= len {
                day -= 7;
            }
            (first - days_from_civil(y, 1, 1)) as u32 + day
        }
    }
}

fn month_len(y: i64, m: u32) -> u32 {
    match m {
        2 => if leap(y) { 29 } else { 28 },
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a civil date (month 1-12), Howard Hinnant's days-from-civil.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = (m + 9) % 12;
    let doy = (153 * mp as u64 + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// The civil date (year, month 1-12, day) of days since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe as i64 + era * 400 + (m <= 2) as i64, m, d)
}

/// A zone as the choice sees it (one `zone` section of the zones document, tools/tzvsf): its rule, its countries (ISO 3166 alpha-2; the first is the country of its principal place, as zone1970.tab lists them) and its principal place (latitude, longitude in arc-seconds).
#[derive(Clone, Copy, Debug)]
pub struct Zone<'a> {
    pub rule: Rule,
    pub countries: &'a [[u8; 2]],
    pub at: (i32, i32),
}

/// What is known of where the device is (owner, 2026-11-10: "cellular, then GPS"): the country of the network it is on (from its MCC), the offset the network says is legal there now and, when it said, whether daylight time is in it (NITZ), and a position (arc-seconds).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clue {
    pub country: Option<[u8; 2]>,
    pub offset: Option<(i32, Option<bool>)>,
    pub at: Option<(i32, i32)>,
}

/// What named the zone: the network (its country, its offset, a position only choosing among the zones those leave), or a position alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Network,
    Position,
}

/// The zone a clue names at `t` (Unix seconds, UTC), as Android picks one from NITZ and the MCC, then the position: the index into `zones`, and what named it.
///
/// - The pool: the zones of the network's country whose rule gives the network's offset now, and its daylight flag too when it gave one; failing that the offset alone; failing that (an offset none of the country's zones keeps) the country alone. With no country, every zone.
/// - With a position: the pool's zone whose principal place is nearest. Until zone regions exist (timezone-boundary-builder's licence is the owner's call) this is the fallback: a place near a border can be nearer another zone's principal place than its own.
/// - Without one: the pool's first zone whose country is listed first (zone1970.tab puts the most populous first), else its first; but only when the network gave an offset or every zone in the pool keeps the same rule. A country of several zones and no offset names none, and an offset with no country names nothing.
pub fn find(zones: &[Zone], clue: &Clue, t: i64) -> Option<(usize, Source)> {
    let in_country = |z: &Zone| clue.country.is_none_or(|c| z.countries.contains(&c));
    let fits = |z: &Zone, strict: bool| match clue.offset {
        None => true,
        Some((o, dst)) => {
            let n = z.rule.at(t);
            n.legal == o && (!strict || dst.is_none_or(|d| (n.legal != n.standard) == d))
        }
    };
    let level = [Some(true), Some(false), None].into_iter().find(|&s| zones.iter().any(|z| in_country(z) && s.is_none_or(|s| fits(z, s))))?;
    let pool = |z: &Zone| in_country(z) && level.is_none_or(|s| fits(z, s));
    if let Some(p) = clue.at {
        let i = nearest(zones, p, pool)?;
        return Some((i, if clue.country.is_some() { Source::Network } else { Source::Position }));
    }
    let country = clue.country?;
    let first = zones.iter().position(|z| pool(z) && z.countries.first() == Some(&country)).or_else(|| zones.iter().position(pool))?;
    let offset_used = clue.offset.is_some() && level.is_some();
    if !offset_used && zones.iter().any(|z| pool(z) && z.rule != zones[first].rule) {
        return None;
    }
    Some((first, Source::Network))
}

/// The zone among those `keep` passes whose principal place is nearest `at` (arc-seconds) along the earth's surface: the largest cosine of the angle between the two places, so no square root and no arc cosine.
pub fn nearest(zones: &[Zone], at: (i32, i32), keep: impl Fn(&Zone) -> bool) -> Option<usize> {
    let (s0, c0) = (sin_q30(at.0 as i64), cos_q30(at.0 as i64));
    let mut best: Option<(usize, i128)> = None;
    for (i, z) in zones.iter().enumerate().filter(|(_, z)| keep(z)) {
        // The longitudes' difference brought within half a turn: two places either side of the antimeridian are close, not a whole turn apart.
        let mut d = z.at.1 as i64 - at.1 as i64;
        if d > HALF_TURN {
            d -= 2 * HALF_TURN;
        } else if d < -HALF_TURN {
            d += 2 * HALF_TURN;
        }
        let (s, c) = (sin_q30(z.at.0 as i64), cos_q30(z.at.0 as i64));
        // cos(angle) = sin φ0 sin φ1 + cos φ0 cos φ1 cos Δλ, in Q60.
        let dot = s0 as i128 * s as i128 + (c0 as i128 * c as i128 >> 30) * cos_q30(d) as i128;
        if best.is_none_or(|b| dot > b.1) {
            best = Some((i, dot));
        }
    }
    best.map(|b| b.0)
}

/// Half a turn in arc-seconds.
const HALF_TURN: i64 = 648_000;
const Q30: i128 = 1 << 30;
/// π in Q30.
const PI_Q30: i128 = 3_373_259_426;

/// The cosine of an angle in arc-seconds within half a turn either way, in Q30: integer, as the rest of this crate (the kernel uses it).
pub fn cos_q30(a: i64) -> i64 {
    let a = a.abs();
    // Past a right angle cos(a) = -cos(180° - a), so the series is taken at most a quarter turn out, where its first eight terms are good to 2^-33.
    let (a, sign) = if a > HALF_TURN / 2 { (HALF_TURN - a, -1) } else { (a, 1) };
    let x = a as i128 * PI_Q30 / HALF_TURN as i128;
    let x2 = x * x >> 30;
    // Taylor's series by Horner: 1 - x²/(1·2) (1 - x²/(3·4) (1 - …)), to x¹⁴/14!.
    let mut c = Q30;
    for k in [182, 132, 90, 56, 30, 12, 2] {
        c = Q30 - (x2 * c >> 30) / k;
    }
    sign * c as i64
}

/// The sine of a latitude in arc-seconds (a quarter turn either way), in Q30: the cosine of its distance from the pole.
pub fn sin_q30(lat: i64) -> i64 {
    cos_q30(HALF_TURN / 2 - lat)
}

#[cfg(test)]
mod tests {
    use super::*;
    const H: i32 = 3600;
    fn utc(y: i64, m: u32, d: u32, h: i64) -> i64 {
        days_from_civil(y, m, d) * 86400 + h * 3600
    }

    #[test]
    fn los_angeles_2026() {
        let r = Rule::parse("PST8PDT,M3.2.0,M11.1.0").unwrap();
        assert_eq!(r.std, -8 * H);
        // Daylight starts 2026-03-08 02:00 PST = 10:00 UTC, ends 2026-11-01 02:00 PDT = 09:00 UTC.
        assert_eq!(r.at(utc(2026, 3, 8, 9)).legal, -8 * H);
        assert_eq!(r.at(utc(2026, 3, 8, 9)).next, Some(utc(2026, 3, 8, 10)));
        assert_eq!(r.at(utc(2026, 3, 8, 10)).legal, -7 * H);
        assert_eq!(r.at(utc(2026, 11, 1, 8)).legal, -7 * H);
        assert_eq!(r.at(utc(2026, 11, 1, 9)).legal, -8 * H);
        assert_eq!(r.at(utc(2026, 11, 10, 20)), Now { standard: -8 * H, legal: -8 * H, next: Some(utc(2027, 3, 14, 10)) });
    }

    #[test]
    fn fixed_and_bracketed() {
        assert_eq!(Rule::parse("<+0530>-5:30").unwrap().at(0).legal, 5 * H + 30 * 60);
        assert_eq!(Rule::parse("UTC0").unwrap().at(0), Now { standard: 0, legal: 0, next: None });
        assert_eq!(Rule::parse("<-03>3").unwrap().std, -3 * H);
    }

    #[test]
    fn southern_hemisphere() {
        // Sydney: daylight from the first Sunday of October 02:00 to the first Sunday of April 03:00 (daylight time).
        let r = Rule::parse("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        assert_eq!(r.at(utc(2026, 1, 15, 0)).legal, 11 * H);
        assert_eq!(r.at(utc(2026, 7, 1, 0)).legal, 10 * H);
        // 2026-04-05 03:00 AEDT = 2026-04-04 16:00 UTC.
        assert_eq!(r.at(utc(2026, 4, 4, 15)).legal, 11 * H);
        assert_eq!(r.at(utc(2026, 4, 4, 16)).legal, 10 * H);
    }

    #[test]
    fn europe_and_extended_times() {
        let r = Rule::parse("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        // The last Sunday of March 2026 is the 29th: 02:00 CET = 01:00 UTC.
        assert_eq!(r.at(utc(2026, 3, 29, 0)).legal, H);
        assert_eq!(r.at(utc(2026, 3, 29, 1)).legal, 2 * H);
        // Greenland: changes at -1:00 and 0:00 local (tzdata's extension).
        assert!(Rule::parse("<-02>2<-01>,M3.5.0/-1,M10.5.0/0").is_some());
    }

    #[test]
    fn refuses_what_it_does_not_read() {
        assert!(Rule::parse("PST8PDT").is_none());
        assert!(Rule::parse("XX5").is_none());
        assert!(Rule::parse("EST5EDT,M13.1.0,M11.1.0").is_none());
    }

    #[test]
    fn cosine_in_q30() {
        let q = 1i64 << 30;
        assert_eq!(cos_q30(0), q);
        assert!((cos_q30(60 * 3600) - q / 2).abs() < 4);
        assert!(cos_q30(90 * 3600).abs() < 4);
        assert!((cos_q30(180 * 3600) + q).abs() < 4);
        assert!((sin_q30(30 * 3600) - q / 2).abs() < 4);
        assert!((sin_q30(-90 * 3600) + q).abs() < 4);
    }

    /// Arc-seconds from degrees and minutes.
    fn dm(d: i32, m: i32) -> i32 {
        d * 3600 + d.signum() * m * 60
    }

    /// A few of zone1970.tab's rows, Phoenix after Denver as there, Zurich before Berlin (it is not: to show the first-listed country wins over the order).
    const TABLE: [(&str, &str, &[[u8; 2]], (i32, i32)); 9] = [
        ("America/Edmonton", "MST7MDT,M3.2.0,M11.1.0", &[[b'C', b'A']], (53 * 3600 + 33 * 60, -113 * 3600 - 28 * 60)),
        ("Europe/Zurich", "CET-1CEST,M3.5.0,M10.5.0/3", &[[b'C', b'H'], [b'D', b'E'], [b'L', b'I']], (47 * 3600 + 23 * 60, 8 * 3600 + 32 * 60)),
        ("Europe/Berlin", "CET-1CEST,M3.5.0,M10.5.0/3", &[[b'D', b'E'], [b'D', b'K'], [b'N', b'O'], [b'S', b'E'], [b'S', b'J']], (52 * 3600 + 30 * 60, 13 * 3600 + 22 * 60)),
        ("Pacific/Fiji", "<+12>-12", &[[b'F', b'J']], (-18 * 3600 - 8 * 60, 178 * 3600 + 25 * 60)),
        ("America/New_York", "EST5EDT,M3.2.0,M11.1.0", &[[b'U', b'S']], (40 * 3600 + 43 * 60, -74 * 3600)),
        ("America/Denver", "MST7MDT,M3.2.0,M11.1.0", &[[b'U', b'S']], (39 * 3600 + 44 * 60, -104 * 3600 - 59 * 60)),
        ("America/Phoenix", "MST7", &[[b'U', b'S'], [b'C', b'A']], (33 * 3600 + 27 * 60, -112 * 3600 - 4 * 60)),
        ("America/Los_Angeles", "PST8PDT,M3.2.0,M11.1.0", &[[b'U', b'S']], (34 * 3600 + 3 * 60, -118 * 3600 - 14 * 60)),
        ("Pacific/Honolulu", "HST10", &[[b'U', b'S']], (21 * 3600 + 18 * 60, -157 * 3600 - 51 * 60)),
    ];

    fn pick(clue: Clue, t: i64) -> Option<(&'static str, Source)> {
        let zones: [Zone; 9] = core::array::from_fn(|i| Zone { rule: Rule::parse(TABLE[i].1).unwrap(), countries: TABLE[i].2, at: TABLE[i].3 });
        find(&zones, &clue, t).map(|(i, s)| (TABLE[i].0, s))
    }

    #[test]
    fn network_country_and_offset() {
        let (winter, summer) = (utc(2026, 11, 10, 20), utc(2026, 7, 1, 20));
        let us = Some(*b"US");
        let net = |o: i32, dst: Option<bool>| Clue { country: us, offset: Some((o * H, dst)), at: None };
        assert_eq!(pick(net(-8, Some(false)), winter), Some(("America/Los_Angeles", Source::Network)));
        // UTC - 7 in summer: Pacific daylight time, or Arizona's standard time all year, told apart by the daylight flag.
        assert_eq!(pick(net(-7, Some(true)), summer).unwrap().0, "America/Los_Angeles");
        assert_eq!(pick(net(-7, Some(false)), summer).unwrap().0, "America/Phoenix");
        // Without the flag, the first of the two in the table.
        assert_eq!(pick(net(-7, None), summer).unwrap().0, "America/Phoenix");
        // UTC - 7 in winter: Denver and Phoenix both keep it; the first in the table, as Android takes its country's default.
        assert_eq!(pick(net(-7, Some(false)), winter).unwrap().0, "America/Denver");
        // An offset none of the country's zones keeps: the country alone, which for the United States names none.
        assert_eq!(pick(net(5, None), winter), None);
        // Germany: Berlin, its own, over Zurich (Büsingen), listed first in the table but Switzerland's.
        assert_eq!(pick(Clue { country: Some(*b"DE"), offset: Some((H, Some(false))), at: None }, winter).unwrap().0, "Europe/Berlin");
        // The country alone: one rule across its zones names it; several do not.
        assert_eq!(pick(Clue { country: Some(*b"DE"), offset: None, at: None }, winter).unwrap().0, "Europe/Berlin");
        assert_eq!(pick(Clue { country: us, offset: None, at: None }, winter), None);
        // An offset with no country names nothing.
        assert_eq!(pick(Clue { country: None, offset: Some((-8 * H, Some(false))), at: None }, winter), None);
    }

    #[test]
    fn position_chooses() {
        let winter = utc(2026, 11, 10, 20);
        // Tucson, in winter at UTC - 7 on a US network: Denver and Phoenix both fit; the nearer principal place is Phoenix.
        let tucson = (dm(32, 13), dm(-110, 58));
        assert_eq!(pick(Clue { country: Some(*b"US"), offset: Some((-7 * H, Some(false))), at: Some(tucson) }, winter), Some(("America/Phoenix", Source::Network)));
        // The position alone: the nearest principal place anywhere.
        assert_eq!(pick(Clue { at: Some(tucson), ..Clue::default() }, winter), Some(("America/Phoenix", Source::Position)));
        // Across the antimeridian: Taveuni at 179° 58' W is Fiji's, not Honolulu's.
        assert_eq!(pick(Clue { at: Some((dm(-16, 50), dm(-179, 58))), ..Clue::default() }, winter).unwrap().0, "Pacific/Fiji");
        // A US network and a place in Canada (Calgary): the nearest of the US zones, never Edmonton.
        assert_eq!(pick(Clue { country: Some(*b"US"), offset: None, at: Some((dm(51, 3), dm(-114, 4))) }, winter).unwrap().0, "America/Denver");
        // Nothing known: nothing named.
        assert_eq!(pick(Clue::default(), winter), None);
    }
}
