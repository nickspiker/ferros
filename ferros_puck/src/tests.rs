//! Host tests: the wire form round trips and refuses what it must, the rasteriser covers what each shape is and keeps the front in front, the circle is the clip, the typed values read as the clock writes them.

extern crate std;
use super::*;
use crate::draw::{draw, next_change, Ctx, Geom, Target};
use std::string::String;
use std::vec;
use std::vec::Vec;

/// A target that keeps each pixel's accumulated opacity (front to back: what is already there hides what goes under it) and which colour was put first at each.
struct Grid {
    w: i32,
    h: i32,
    a: Vec<f32>,
    first: Vec<Option<Colour>>,
    texts: Vec<(f32, f32, f32, String)>,
}

impl Grid {
    fn new(w: i32, h: i32) -> Grid {
        Grid { w, h, a: vec![0.; (w * h) as usize], first: vec![None; (w * h) as usize], texts: Vec::new() }
    }
    fn area(&self) -> f32 {
        self.a.iter().sum()
    }
    fn at(&self, x: i32, y: i32) -> (f32, Option<Colour>) {
        let i = (y * self.w + x) as usize;
        (self.a[i], self.first[i])
    }
}

impl Target for Grid {
    fn clip(&self) -> (i32, i32, i32, i32) {
        (0, 0, self.w, self.h)
    }
    fn under(&mut self, x: i32, y: i32, cov: f32, c: Colour) {
        let i = (y * self.w + x) as usize;
        if self.first[i].is_none() && cov > 0.5 {
            self.first[i] = Some(c);
        }
        self.a[i] += cov * (1. - self.a[i]);
    }
    fn text(&mut self, cx: f32, cy: f32, px: f32, _turn: (f32, f32), _c: Colour, s: &str) {
        self.texts.push((cx, cy, px, String::from(s)));
    }
}

const RED: Colour = Colour::rgb(200, 30, 30);
const BLUE: Colour = Colour::rgb(30, 30, 200);
const G: Geom = Geom { cx: 128., cy: 128., r: 100., cos: 1., sin: 0. };
const CTX: Ctx = Ctx { now: 0, zone: 0, base: 10 };

fn list(f: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut buf = [0u8; LIST_MAX];
    let mut w = Writer::new(&mut buf);
    f(&mut w);
    w.bytes().to_vec()
}

#[test]
fn a_disc_covers_its_area_and_the_circle_clips_it() {
    let l = list(|w| {
        w.disc(0, 0, (ONE / 2) as u16, RED).unwrap();
    });
    check(&l).unwrap();
    let mut g = Grid::new(256, 256);
    draw(&l, &G, &CTX, &mut g);
    let want = core::f32::consts::PI * 50. * 50.;
    assert!((g.area() - want).abs() < want / 256., "disc area {} against {want}", g.area());
    // A disc twice the circle is clipped to the circle: its area is the circle's.
    let l = list(|w| {
        w.disc(0, 0, (ONE * 2) as u16, RED).unwrap();
    });
    let mut g = Grid::new(256, 256);
    draw(&l, &G, &CTX, &mut g);
    let want = core::f32::consts::PI * 100. * 100.;
    assert!((g.area() - want).abs() < want / 256., "clipped area {} against {want}", g.area());
    assert_eq!(g.at(2, 2).0, 0.);
}

#[test]
fn the_first_op_is_in_front() {
    let l = list(|w| {
        w.disc(0, 0, (ONE / 4) as u16, RED).unwrap();
        w.disc(0, 0, (ONE / 2) as u16, BLUE).unwrap();
    });
    let mut g = Grid::new(256, 256);
    draw(&l, &G, &CTX, &mut g);
    assert_eq!(g.at(128, 128).1, Some(RED));
    assert_eq!(g.at(128 + 37, 128).1, Some(BLUE));
    // Opaque in front: the blue went under red's centre and added nothing.
    assert!((g.at(128, 128).0 - 1.).abs() < 1e-6);
}

#[test]
fn a_ring_and_an_arc_cover_what_they_are() {
    let l = list(|w| {
        w.ring(0, 0, (ONE / 2) as u16, (ONE / 4) as u16, RED).unwrap();
    });
    let mut g = Grid::new(256, 256);
    draw(&l, &G, &CTX, &mut g);
    let want = core::f32::consts::PI * (50. * 50. - 25. * 25.);
    assert!((g.area() - want).abs() < want / 128., "ring area {} against {want}", g.area());
    // A quarter of that ring as an arc, round ended: a quarter of the band plus one cap's disc (two half discs).
    let l = list(|w| {
        w.arc(0, 0, (3 * ONE / 8) as u16, (ONE / 4) as u16, 0, 1 << 14, RED).unwrap();
    });
    let mut g = Grid::new(256, 256);
    draw(&l, &G, &CTX, &mut g);
    let want = want / 4. + core::f32::consts::PI * 12.5 * 12.5;
    assert!((g.area() - want).abs() < want / 64., "arc area {} against {want}", g.area());
    // Clockwise from up: the quarter from 12 to 3 o'clock covers the upper right, not the upper left.
    assert!(g.at(128 + 26, 128 - 26).0 > 0.99);
    assert_eq!(g.at(128 - 26, 128 - 26).0, 0.);
}

#[test]
fn a_curve_passes_thru_its_samples_and_fills_to_its_base() {
    // A straight line from (-1/2, 0) to (1/2, 0) as values 0..100 over [0, 200], slope 0: at height 1/2 of its box.
    let samples = [(100, 0), (100, 0), (100, 0)];
    let l = list(|w| {
        w.curve(-(ONE / 2) as i16, (ONE / 2) as i16, (ONE / 2) as i16, -(ONE / 2) as i16, 0, 200, (ONE / 32) as u16, RED, Some(BLUE), 0, &samples).unwrap();
    });
    check(&l).unwrap();
    let mut g = Grid::new(256, 256);
    draw(&l, &G, &CTX, &mut g);
    // The line at y = 128 (the middle of its box), the fill below it to the box's base at y = 178.
    assert_eq!(g.at(128, 128).1, Some(RED));
    assert_eq!(g.at(128, 160).1, Some(BLUE));
    assert_eq!(g.at(128, 100).0, 0.);
    assert_eq!(g.at(128, 185).0, 0.);
    // A rising Hermite piece: values 0 then 200 with the slopes of the straight line between them (200 a span) is that straight line.
    let cv = Curve { x0: 0, x1: 1, yb: 0, yt: 1, vlo: 0, vhi: 1, width: 0, line: RED, fill: None, turn: 0, samples: &[0, 0, 0, 0, 200, 0, 0, 0, 200, 0, 0, 0, 200, 0, 0, 0] };
    for k in 0..=16 {
        let s = k as f32 / 16.;
        let (v, dv) = cv.at(s);
        assert!((v - 200. * s).abs() < 1e-3 && (dv - 200.).abs() < 1e-3, "at {s}: {v} {dv}");
    }
}

#[test]
fn a_marker_rides_its_curve_at_now() {
    let samples = [(0, 0), (0, 0)];
    let day = 86400 * OSC;
    let l = list(|w| {
        let c = w.curve(-(ONE / 2) as i16, (ONE / 2) as i16, 0, -(ONE / 2) as i16, -1, 1, 0, RED, None, 0, &samples).unwrap();
        w.marker(c, 0, day, (ONE / 16) as u16, BLUE).unwrap();
    });
    assert!(check(&l).unwrap().anchored);
    let mut g = Grid::new(256, 256);
    draw(&l, &G, &Ctx { now: day / 4, ..CTX }, &mut g);
    // A quarter of the way from x = 78 to 178, on the line's y (its middle value, 0, at the box's middle).
    assert!(g.at(103, 103).0 > 0.99, "marker at {:?}", g.at(103, 103));
    // Its next half pixel: 86400 s over 100 px, half a pixel is 432 s.
    let next = next_change(&l, 100., day / 4).unwrap();
    assert_eq!((next - day / 4) / OSC, 432);
    // Before its span the next change is the span's start; after it nothing moves.
    assert_eq!(next_change(&l, 100., -5), Some(0));
    assert_eq!(next_change(&l, 100., day + 1), None);
}

#[test]
fn a_sweep_follows_now() {
    let l = list(|w| {
        w.sweep(0, 0, (ONE / 2) as u16, (ONE / 8) as u16, 0, 0, 60, RED).unwrap();
    });
    let area = |now: i64| {
        let mut g = Grid::new(256, 256);
        draw(&l, &G, &Ctx { now, ..CTX }, &mut g);
        g.area()
    };
    let (a, b) = (area(15 * OSC), area(45 * OSC));
    // Three times the band, the same two caps.
    let band = 2. * core::f32::consts::PI * 50. * 12.5;
    assert!(((b - a) - band / 2.).abs() < band / 64., "{a} {b}");
}

#[test]
fn the_display_refuses_bad_lists_whole() {
    let good = list(|w| {
        w.disc(0, 0, 100, RED).unwrap();
        w.text(0, 0, 1000, Colour::theme(theme::TEXT), "4.2 m").unwrap();
    });
    assert_eq!(check(&good), Ok(Checked { count: 2, anchored: false }));
    let mut bad = good.clone();
    bad[0] = 1;
    assert_eq!(check(&bad), Err(Refused::Header));
    let mut bad = good.clone();
    bad[2] = kind::END;
    assert_eq!(check(&bad), Err(Refused::Kind(0)));
    let mut bad = good.clone();
    bad.push(0);
    assert_eq!(check(&bad), Err(Refused::Trailing));
    let mut bad = good.clone();
    bad.truncate(bad.len() - 1);
    assert!(check(&bad).is_err());
    // A colour's tag past the last, a theme slot past the last.
    let mut bad = good.clone();
    bad[3 + 6 + 2] = 7;
    assert_eq!(check(&bad), Err(Refused::Value(0)));
    let mut bad = good.clone();
    bad[3 + 6 + 2 + 2] = theme::END;
    bad[3 + 6 + 2] = THEME;
    assert_eq!(check(&bad), Err(Refused::Value(0)));
    // A marker that names no earlier curve.
    let l = list(|w| {
        w.disc(0, 0, 100, RED).unwrap();
        w.marker(0, 0, 1, 10, RED).unwrap();
    });
    assert_eq!(check(&l), Err(Refused::Value(1)));
    // Text that is not UTF-8.
    let mut bad = good.clone();
    let at = bad.len() - 1;
    bad[at] = 0xFF;
    assert_eq!(check(&bad), Err(Refused::Value(1)));
    // The bounds: OPS_MAX ops fit, one more does not.
    let mut buf = [0u8; LIST_MAX];
    let mut w = Writer::new(&mut buf);
    for _ in 0..OPS_MAX {
        w.disc(0, 0, 1, RED).unwrap();
    }
    assert_eq!(w.disc(0, 0, 1, RED), Err(Full));
    assert!(check(w.bytes()).is_ok());
}

#[test]
fn moments_read_as_the_clock_writes_them() {
    let mut b = [0u8; fmt::OUT];
    // 2026-11-17 16:07:30 UTC as Eagle oscillations: POSIX 1795018050.
    let unix = 1_795_018_050i64;
    let t = (unix - EAGLE_EPOCH_UNIX_SECS) * OSC;
    assert_eq!(fmt::second_of_day(t, 0), 16 * 3600 + 7 * 60 + 30);
    assert_eq!(fmt::moment(t, 0, 10, 1, &mut b), "4:07pm");
    assert_eq!(fmt::moment(t, -8 * 3600, 10, 1, &mut b), "8:07am");
    assert_eq!(fmt::moment(t, 0, 10, 2, &mut b), "4:07:30pm");
    // Dozenal: 16:07:30 is 0.67187 of the day: 8 twelfths (Luna), then 0.0625 of one, 0 (Zil), 9 (Lunor), so two places show Luna and the zil is dropped.
    assert_eq!(fmt::moment(t, 0, 12, 1, &mut b), "\u{18}");
    assert_eq!(fmt::moment(t, 0, 12, 2, &mut b), "\u{18}:\u{10}:\u{19}");
    // Hex: the seconds since midnight, 58050 = G#E2C2.
    assert_eq!(fmt::moment(t, 0, 16, 1, &mut b), "E2C2");
    // Noon is twelve, midnight too.
    assert_eq!(fmt::moment((43200 - EAGLE_EPOCH_UNIX_SECS) * OSC, 0, 10, 1, &mut b), "12:00pm");
    assert_eq!(fmt::moment((0 - EAGLE_EPOCH_UNIX_SECS) * OSC, 0, 10, 1, &mut b), "12:00am");
}

#[test]
fn numbers_read_in_the_persons_base() {
    let mut b = [0u8; fmt::OUT];
    let q = |v: f64| (v * 65536.) as i64;
    assert_eq!(fmt::number(q(4.25), 2, fmt::unit::METRE, 10, &mut b), "4.25m");
    assert_eq!(fmt::number(q(-0.3), 1, fmt::unit::METRE, 10, &mut b), "-0.3m");
    assert_eq!(fmt::number(q(18.), 0, fmt::unit::CELSIUS, 10, &mut b), "18\u{B0}C");
    // 4.25 in dozenal is 4;3 (a quarter is three twelfths).
    assert_eq!(fmt::number(q(4.25), 1, fmt::unit::METRE, 12, &mut b), "\u{14}.\u{13}m");
    // 255.5 in hex is FF.8.
    assert_eq!(fmt::number(q(255.5), 1, fmt::unit::NONE, 16, &mut b), "FF.8");
    // Rounded to the last place, and no minus on a value that rounds to zero.
    assert_eq!(fmt::number(q(-0.01), 1, fmt::unit::NONE, 10, &mut b), "0.0");
}

#[test]
fn values_reach_the_target_as_words_at_their_place() {
    let l = list(|w| {
        w.moment(0, (ONE / 2) as i16, (ONE / 8) as u16, Colour::theme(theme::TEXT), (-EAGLE_EPOCH_UNIX_SECS + 3600) * OSC, 1).unwrap();
        w.number(0, 0, (ONE / 4) as u16, Colour::theme(theme::TEXT), 3 << 16, 0, fmt::unit::METRE).unwrap();
    });
    let mut g = Grid::new(256, 256);
    draw(&l, &Geom { sin: 1., cos: 0., ..G }, &CTX, &mut g);
    assert_eq!(g.texts[0].3, "1:00am");
    // Turned a quarter: the circle's down is the screen's left.
    assert!((g.texts[0].0 - 78.).abs() < 1e-3 && (g.texts[0].1 - 128.).abs() < 1e-3);
    assert!((g.texts[0].2 - 12.5).abs() < 1e-3);
    assert_eq!(g.texts[1].3, "3m");
}

#[test]
fn a_turned_curve_turns_about_the_centre() {
    // The flat line of the first curve test at y = 0, turned a quarter clockwise: it runs down the middle, its fill (which was below it) now to its left.
    let samples = [(100, 0), (100, 0)];
    let l = list(|w| {
        w.curve(-(ONE / 2) as i16, (ONE / 2) as i16, (ONE / 2) as i16, -(ONE / 2) as i16, 0, 200, (ONE / 32) as u16, RED, Some(BLUE), 1 << 14, &samples).unwrap();
    });
    let mut g = Grid::new(256, 256);
    draw(&l, &G, &CTX, &mut g);
    assert_eq!(g.at(128, 100).1, Some(RED));
    assert_eq!(g.at(110, 128).1, Some(BLUE));
    assert_eq!(g.at(146, 128).0, 0.);
}
