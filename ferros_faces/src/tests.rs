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
    let face = ferros_tide::Face::at(&t, NOW).unwrap();
    let about = tide::About { scale: tide::scale(&t), place: Some(ferros_sky::Place::deg(47.6, -122.3)), station: "Seattle", distance_m: 3200. };
    let words = both("tide", osc(NOW), |w, focus| tide::face(w, &face, &about, focus));
    // Height now, the high and the low with their heights; in focus the station and its distance too.
    assert_eq!(words[0].len(), 5);
    assert_eq!(words[1].len(), 7);
    assert!(words[1].iter().any(|s| s == "Seattle") && words[1].iter().any(|s| s == "3km"));
    assert!(tide::deadline(&face) > NOW && tide::deadline(&face) <= face.until);
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
    let sats: Vec<gps::Signal> = (0..24).map(|k| gps::Signal { az: (k * 15) as u16, el: (k * 3) as i8, used: k % 3 != 0 }).collect();
    let s = gps::Sky { signals: &sats, fix: Some((8, NOW)), live: true };
    let words = both("gps", 0, |w, focus| gps::face(w, &s, focus));
    assert_eq!(words[0][0], "8m");
    assert_eq!((words[0][2].as_str(), words[0][3].as_str()), ("16", "24"));
}

#[test]
fn weather() {
    let f = ferros_weather::Forecast::parse(include_bytes!("../../ferros_weather/tests/fixtures/fgtw-reply.vsf")).unwrap();
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
