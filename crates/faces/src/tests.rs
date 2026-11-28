//! Every face, at rest and in focus, from its engine's real output: the list is one the display accepts (`check`), within its bounds, and draws (something covers the circle, every word reaches the target); each list's size printed (`cargo test -- --nocapture`).

extern crate std;
use crate::*;
use ferros_puck::draw::{draw, Ctx, Geom, Target};
use ferros_puck::{check, Colour, LIST_MAX, OPS_MAX};
use std::println;
use std::string::String;
use std::vec;
use std::vec::Vec;

struct Grid {
    a: Vec<f32>,
    words: Vec<String>,
}

impl Target for Grid {
    fn clip(&self) -> (i32, i32, i32, i32) {
        (0, 0, 256, 256)
    }
    fn under(&mut self, x: i32, y: i32, cov: f32, _c: Colour) {
        let i = (y * 256 + x) as usize;
        self.a[i] += cov * (1. - self.a[i]);
    }
    fn text(&mut self, _cx: f32, _cy: f32, _px: f32, _turn: (f32, f32), _c: Colour, s: &str) {
        self.words.push(String::from(s));
    }
}

/// Write a face both ways, check and draw each, and say its size.
fn both(name: &str, now_osc: i64, mut f: impl FnMut(&mut Writer, bool) -> Result<(), Full>) -> [Vec<String>; 2] {
    let mut out: [Vec<String>; 2] = [Vec::new(), Vec::new()];
    for focus in [false, true] {
        let mut buf = [0u8; LIST_MAX];
        let mut w = Writer::new(&mut buf);
        f(&mut w, focus).unwrap_or_else(|_| panic!("{name}: the list is full"));
        let l = w.bytes();
        let c = check(l).unwrap_or_else(|e| panic!("{name}: refused: {e:?}"));
        assert!(l.len() <= LIST_MAX && (c.count as usize) <= OPS_MAX);
        println!("{name} {}: {} ops, {} bytes{}", if focus { "in focus" } else { "at rest" }, c.count, l.len(), if c.anchored { ", anchored" } else { "" });
        let mut g = Grid { a: vec![0.; 256 * 256], words: Vec::new() };
        draw(l, &Geom { cx: 128., cy: 128., r: 120., cos: 1., sin: 0. }, &Ctx { now: now_osc, zone: -8 * 3600, base: 10 }, &mut g);
        // The sky covers the whole circle: its area is the circle's.
        let area: f32 = g.a.iter().sum();
        let want = core::f32::consts::PI * 120. * 120.;
        assert!((area - want).abs() < want / 64., "{name}: covered {area} of {want}");
        out[focus as usize] = g.words;
    }
    out
}

/// 2026-11-17 16:00 UTC in Eagle seconds.
const NOW: f64 = 1_795_017_600. + 14_182_940.;

#[test]
fn tide() {
    // A mixed tide like Seattle's in its two largest constituents (M2 1.07 m, K1 0.84 m) on a 2 m mean sea level.
    let mut t = ferros_tide::Tide::flat(2.);
    t.set(ferros_tide::index_of("M2").unwrap(), 1.07, 10.);
    t.set(ferros_tide::index_of("K1").unwrap(), 0.84, 260.);
    let mut glyphs = ferros_glyphs::Glyphs::with_arena(ferros_glyphs::FONT, 1 << 15).unwrap();
    let mut cache = tide::Cache::new();
    let zone = -8 * 3600;
    for (base, focus, turn) in [(12u8, false, (1f32, 0f32)), (10, true, (0.6, 0.8)), (16, false, (1., 0.))] {
        let chart = tide::Chart { tide: &t, scale: tide::scale(&t), place: ferros_sky::Place::deg(47.6, -122.3), now: NOW, zone, base, station: focus.then_some(("Seattle", 3200.)) };
        let d = 256;
        let mut px = vec![0u64; d * d];
        tide::paint(&mut px, d, turn, &chart, &mut cache, &mut glyphs);
        // Every pixel inside the circle is opaque: the sky is drawn under everything.
        let r = d as f32 * 0.5;
        let mut inside = 0;
        for y in 0..d {
            for x in 0..d {
                let (dx, dy) = (x as f32 + 0.5 - r, y as f32 + 0.5 - r);
                if dx * dx + dy * dy < (r - 1.) * (r - 1.) {
                    inside += 1;
                    assert_eq!(px[y * d + x] >> 48, 0xFFFF, "base {base}: pixel ({x}, {y}) not opaque");
                }
            }
        }
        assert!(inside > 0);
        // The next square is due within a turn of now's words, and after now.
        let next = tide::deadline(&chart, d);
        assert!(next > NOW && next <= NOW + tide::word_step(base), "base {base}: deadline {}", next - NOW);
        println!("tide base {base}: next square in {} s", next - NOW);
    }
}

#[test]
fn tide_words() {
    // A minute past 2026-11-18 16:00 UTC is 08:01 at -8 h (a minute past, so float rounding at the turn of the minute cannot floor it back): the day's 48th of 144 in dozenal (Ter, Zil: the zil dropped), 08:01 in decimal, G#70BC s in hex (to G#100 s, G#70: 7, the bar, 0).
    let words = |base| {
        let w = tide::time_words(NOW + 60., -8 * 3600, base);
        (String::from(w.a.as_str()), String::from(w.b.as_str()), w.split == tide::Split::Lone)
    };
    assert_eq!(words(12), (String::from("\u{14}"), String::new(), true));
    assert_eq!(words(10), (String::from("08"), String::from("01"), false));
    assert_eq!(words(16), (String::from("7"), String::from("0"), false));
}

#[test]
fn sun_and_moon() {
    let p = ferros_sky::Place::deg(47.6, -122.3);
    let d = ferros_sky::SunDay::around(NOW, p, -8. * 3600.);
    let words = both("sun", osc(NOW), |w, focus| sky::sun(w, &d, focus));
    // Noon's altitude, sunrise and sunset; civil dawn and dusk in focus.
    assert_eq!((words[0].len(), words[1].len()), (3, 5));
    let m = ferros_sky::MoonDay::around(NOW, p, -8. * 3600.);
    let words = both("moon", osc(NOW), |w, focus| sky::moon(w, &m, focus));
    assert!(words[0].iter().any(|s| s.ends_with('%')));
    // Polar night at Tromso: no sunrise or sunset to show, the face still whole.
    let polar = ferros_sky::SunDay::around(NOW + 30. * 86400., ferros_sky::Place::deg(69.65, 18.96), 3600.);
    let words = both("sun, polar night", osc(NOW), |w, focus| sky::sun(w, &polar, focus));
    assert_eq!(words[0].len(), 1);
}

#[test]
fn compass() {
    let c = compass::Compass { heading: Some(1.2), declination: 15f64.to_radians(), coverage: [1., 0.5, 0.25] };
    let words = both("compass", 0, |w, focus| compass::face(w, &c, focus));
    assert_eq!(words[0], ["69\u{B0}"]);
    let none = compass::Compass { heading: None, declination: 0., coverage: [0.; 3] };
    both("compass, no heading", 0, |w, focus| compass::face(w, &none, focus));
}

#[test]
fn gps() {
    let sats: Vec<gps::Signal> = (0..24).map(|k| gps::Signal { az: (k * 15) as u16, el: (k * 3) as i8, used: k % 3 == 1, cn0: if k % 3 == 2 { 0 } else { 20 + k as u8 }, predicted: k % 3 == 2 }).collect();
    let s = gps::Sky { signals: &sats, fix: Some((8, NOW)), live: true };
    let words = both("gps", 0, |w, focus| gps::face(w, &s, focus));
    assert_eq!(words[0][0], "8m");
    assert_eq!((words[0][2].as_str(), words[0][3].as_str()), ("8", "16"));
    // The right-hand count is the signals heard: a bird only predicted is not one.
}

#[test]
fn weather() {
    let f = ferros_weather::Forecast::parse(include_bytes!("../../weather/tests/fixtures/fgtw-reply.vsf")).unwrap();
    let words = both("weather", f.now.time, |w, focus| weather::face(w, &f, focus));
    // 6.3 C now (the fixture's Seattle morning) and the day's low and high.
    assert_eq!(words[0][0], "6\u{B0}C");
    assert!(weather::deadline(&f, f.now.time) > f.now.time);
}

#[test]
fn thermo() {
    let r = thermo::Reading { object: Some(21.25), sensor: Some(28.07) };
    let words = both("thermo", 0, |w, focus| thermo::face(w, &r, focus));
    assert_eq!(words[0], ["21.3\u{B0}C", "28.1\u{B0}C"]);
    assert_eq!(words[1], ["21.25\u{B0}C", "28.07\u{B0}C"]);
    let none = thermo::Reading::default();
    let words = both("thermo, no sample yet", 0, |w, focus| thermo::face(w, &none, focus));
    assert!(words[0].is_empty());
}

/// The fill's sweep (Q16 of a turn) in a written rotary face: the arc op at the track's radius whose colour is the face's.
fn fill_sweep(l: &[u8], c: Colour) -> u16 {
    ferros_puck::ops(l)
        .find_map(|op| match op {
            ferros_puck::Op::Arc { r, sweep, c: oc, .. } if oc == c && r == u(0.8) => Some(sweep),
            _ => None,
        })
        .expect("a rotary face with a value has its fill")
}

#[test]
fn rotary() {
    use ferros_puck::fmt::unit;
    use ferros_puck::LIST_MAX;
    let notes = [rotary::Note { value: -412.5, places: 0, unit: unit::NONE }, rotary::Note { value: 31.25, places: 1, unit: unit::CELSIUS }];
    let at = |v: Option<f32>, clockwise: bool, second: Option<f32>| rotary::Rotary { value: v, lo: 0., hi: 100., second, clockwise, unit: unit::PERCENT, places: 0, colour: TEXT, notes: &notes };
    // The face fits a list and draws whole at the range's ends, inside, past both ends, and before any reading, either way round, with and without a second value.
    for v in [None, Some(-40.), Some(0.), Some(0.5), Some(37.), Some(100.), Some(160.), Some(1e9)] {
        for clockwise in [true, false] {
            let r = at(v, clockwise, Some(80.));
            let words = both("rotary", 0, |w, focus| rotary::face(w, &r, focus));
            assert_eq!(words[0].len(), 2 + v.is_some() as usize);
        }
    }
    let words = both("rotary, plain", 0, |w, focus| rotary::face(w, &at(Some(87.), true, None), focus));
    // -412.5 mA rounds half away from zero, as `fmt::number` rounds.
    assert_eq!(words[0], ["87%", "-413", "31.3\u{B0}C"]);
    // The fill's sweep grows with the value across the range and half a range past its top (further out the tanh nears its bound by less than Q16's step), and stays under a turn however far past.
    let mut last = 0u16;
    for k in 0..=1 << 8 {
        let v = k as f32 * (150. / (1 << 8) as f32);
        let mut buf = [0u8; LIST_MAX];
        let mut w = Writer::new(&mut buf);
        rotary::face(&mut w, &at(Some(v), true, None), false).unwrap();
        let s = fill_sweep(w.bytes(), TEXT);
        assert!(k == 0 || s > last, "sweep {s} at {v} after {last}");
        last = s;
    }
    for v in [200., 1e3, 1e9, f32::MAX] {
        let mut buf = [0u8; LIST_MAX];
        let mut w = Writer::new(&mut buf);
        rotary::face(&mut w, &at(Some(v), true, None), false).unwrap();
        let s = fill_sweep(w.bytes(), TEXT);
        assert!(s >= last && (s as f32) <= (rotary::TRACK + rotary::GAP * 0.5) * 65536., "sweep {s} at {v}");
        last = s;
    }
    // The angle is smooth at the range's ends: the slope just inside and just outside each agree, and it rises everywhere.
    let h = 1. / (1 << 10) as f32;
    for x in [0f32, 1.] {
        let below = (rotary::turns(x) - rotary::turns(x - h)) / h;
        let above = (rotary::turns(x + h) - rotary::turns(x)) / h;
        assert!((below - above).abs() < 1. / (1 << 6) as f32, "slope at {x}: {below} below, {above} above");
    }
    // It rises from half a range below the bottom to half a range past the top (further out f32's tanh is flat), and stays inside its bounds however far out.
    let mut prev = rotary::turns(-0.5);
    for k in 1..=1 << 12 {
        let t = rotary::turns(-0.5 + k as f32 * (2. / (1 << 12) as f32));
        assert!(t > prev, "turns falls at {k}");
        prev = t;
    }
    for x in [-1e30f32, -64., -8., 9., 64., 1e30] {
        let t = rotary::turns(x);
        assert!(t >= -rotary::GAP * 0.5 && t <= rotary::TRACK + rotary::GAP * 0.5, "turns({x}) = {t}");
    }
}
