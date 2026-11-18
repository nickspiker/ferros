//! Host tests: the engine as the kist builds it (basecalc without its terminal), one expression per feature; each base's keypad; the keys' model; what the vault keeps.

use crate::calc::{Calc, Shown};
use crate::keys::{self, Key};
use crate::layout::{digit_grid, panel_circle, Layout, Rect};
use crate::store::Saved;
use basecalc::{coloured_vec_to_string, evaluate, BasecalcState};

/// The engine at a base, a precision and an angle unit, as the kist sets them.
fn engine(base: u8, digits: usize, radians: bool) -> BasecalcState {
    let mut s = BasecalcState::new();
    s.base = base;
    s.digits = digits;
    s.radians = radians;
    s.set_precision();
    s
}

/// An expression's result as the engine writes it (ASCII, as basecalc's own self test compares).
fn shown(base: u8, expr: &str) -> String {
    let (_, spans) = evaluate(expr, &mut engine(base, 12, true)).unwrap_or_else(|e| panic!("{expr}: {e}"));
    coloured_vec_to_string(&spans)
}

/// An expression's value, real and imaginary parts as doubles.
fn value(base: u8, radians: bool, expr: &str) -> (f64, f64) {
    let (v, _) = evaluate(expr, &mut engine(base, 12, radians)).unwrap_or_else(|e| panic!("{expr}: {e}"));
    (v.real().to_f64(), v.imag().to_f64())
}

fn near(got: (f64, f64), want: (f64, f64)) -> bool {
    (got.0 - want.0).abs() < 1e-12 && (got.1 - want.1).abs() < 1e-12
}

#[test]
fn engine_arithmetic_and_bases() {
    assert_eq!(shown(10, "2+3*4"), "  14.");
    assert_eq!(shown(10, "7%3"), "  1.");
    assert_eq!(shown(10, "2^10"), "  1 024.");
    assert_eq!(shown(2, "1+1"), "  10.");
    assert_eq!(shown(8, "7+1"), "  10.");
    assert_eq!(shown(12, "B*2"), "  1A.");
    assert_eq!(shown(16, "FF+1"), "  100.");
    assert_eq!(shown(36, "Z+1"), "  10.");
    assert_eq!(shown(7, "6+1"), "  10.");
}

#[test]
fn engine_complex_numbers() {
    assert!(near(value(10, true, "[3,4]*[1,-1]"), (7., 1.)));
    // i, as the key writes it.
    assert!(near(value(10, true, "[0,1]*[0,1]"), (-1., 0.)));
    assert!(near(value(10, true, "#sqrt(-4)"), (0., 2.)));
    assert!(near(value(10, true, "#re([3,4])"), (3., 0.)));
    assert!(near(value(10, true, "#im([3,4])"), (4., 0.)));
    assert!(near(value(10, true, "#abs([3,4])"), (5., 0.)));
    assert!(near(value(10, true, "#angle([0,1])"), (core::f64::consts::FRAC_PI_2, 0.)));
}

#[test]
fn engine_precision() {
    let at = |digits: usize| {
        let (_, spans) = evaluate("1/3", &mut engine(10, digits, true)).unwrap();
        coloured_vec_to_string(&spans)
    };
    let (four, many) = (at(4), at(1 << 5));
    assert!(four.starts_with("  0.333"), "{four}");
    assert!(many.starts_with("  0.333 333 333 333 333"), "{many}");
    assert!(many.len() > four.len() + 20, "{four} / {many}");
}

#[test]
fn engine_trig() {
    assert!(near(value(10, true, "#sin(@pi/2)"), (1., 0.)));
    assert!(near(value(10, true, "#cos(0)"), (1., 0.)));
    assert!(near(value(10, true, "#tan(@pi/4)"), (1., 0.)));
    assert!(near(value(10, true, "#asin(1)*2-@pi"), (0., 0.)));
    assert!(near(value(10, true, "#acos(1)"), (0., 0.)));
    assert!(near(value(10, true, "#atan(1)*4-@pi"), (0., 0.)));
    // Degrees.
    assert!(near(value(10, false, "#sin(90)"), (1., 0.)));
    assert!(near(value(10, false, "#atan(1)"), (45., 0.)));
}

#[test]
fn engine_logs_powers_roots() {
    assert!(near(value(10, true, "#ln(@e)"), (1., 0.)));
    // log is in the base in use.
    assert!(near(value(10, true, "#log(100)"), (2., 0.)));
    assert!(near(value(16, true, "#log(100)"), (2., 0.)));
    assert!(near(value(10, true, "8$2"), (3., 0.)));
    assert!(near(value(10, true, "#sqrt(81)"), (9., 0.)));
    assert!(near(value(10, true, "2^-1"), (0.5, 0.)));
}

#[test]
fn engine_rounding_and_parts() {
    assert!(near(value(10, true, "#ceil(2.1)"), (3., 0.)));
    assert!(near(value(10, true, "#floor(2.9)"), (2., 0.)));
    assert!(near(value(10, true, "#round(2.6)"), (3., 0.)));
    // basecalc's integer part is the floor, and the fraction what is left above it.
    assert!(near(value(10, true, "#int(-2.5)"), (-3., 0.)));
    assert!(near(value(10, true, "#frac(-2.25)"), (0.75, 0.)));
    assert!(near(value(10, true, "#frac(2.25)"), (0.25, 0.)));
    assert!(near(value(10, true, "#sign(-7)"), (-1., 0.)));
    assert!(near(value(10, true, "#erf(0)"), (0., 0.)));
}

#[test]
fn engine_constants_and_previous() {
    assert!(near(value(10, true, "@pi"), (core::f64::consts::PI, 0.)));
    assert!(near(value(10, true, "@e"), (core::f64::consts::E, 0.)));
    assert!((value(10, true, "@phi").0 - 1.618_033_988_749_895).abs() < 1e-12);
    assert!((value(10, true, "@gamma").0 - 0.577_215_664_901_532_9).abs() < 1e-12);
    assert!((value(10, true, "@catalan").0 - 0.915_965_594_177_219).abs() < 1e-12);
    let r = value(10, true, "@rand").0;
    assert!((0. ..1.).contains(&r));
    value(10, true, "@grand");
    let mut s = engine(10, 12, true);
    evaluate("6*7", &mut s).unwrap();
    let (v, _) = evaluate("&+1", &mut s).unwrap();
    assert_eq!(v.real().to_f64(), 43.);
}

/// Each base's digit block: the grid holds every digit with no empty row, digits in reading order, and every base of a family puts each digit where the family's largest base does.
#[test]
fn every_base_has_its_grid() {
    for base in 2..=36u8 {
        let (cols, rows) = digit_grid(base);
        let cells = cols as usize * rows as usize;
        assert!(cells >= base as usize && cells - (base as usize) < cols as usize, "base {base}: {cols} x {rows}");
    }
    let named = [(2, (2, 1)), (4, (2, 2)), (8, (4, 2)), (10, (3, 4)), (12, (3, 4)), (16, (4, 4)), (20, (6, 4)), (36, (6, 6))];
    for (base, grid) in named {
        assert_eq!(digit_grid(base), grid, "base {base}");
    }
    for family in [&[2u8, 4][..], &[3, 5, 6, 9, 10, 11, 12], &[7, 8, 13, 14, 15, 16], &(17..=36).collect::<Vec<u8>>()] {
        let widest = *family.last().unwrap();
        for &base in family {
            assert_eq!(digit_grid(base).0, digit_grid(widest).0, "base {base} keeps base {widest}'s places");
        }
    }
}

fn overlap(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// Every key of every base, layer and picker, on the Pro's and the 8a's glass: inside the frame and inside the app's circle (corners included), no two overlapping, each digit of the base once and in its grid's reading order.
#[test]
fn every_key_stays_inside_the_circle() {
    for (w, h) in [(1344., 2992.), (1080., 2400.)] {
        let circle = panel_circle(w, h);
        let layout = Layout::new(w, h, circle);
        for base in 2..=36u8 {
            for second in [false, true] {
                for picking in [false, true] {
                    let placed = layout.place(base, second, picking);
                    for p in &placed {
                        let r = p.rect;
                        assert!(r.w > 0. && r.h > 0., "{:?}", p);
                        for (x, y) in [(r.x, r.y), (r.x + r.w, r.y), (r.x, r.y + r.h), (r.x + r.w, r.y + r.h)] {
                            assert!(x >= 0. && x <= w && y >= 0. && y <= h, "{w}x{h} base {base}: {:?} leaves the frame", p.key);
                            assert!(circle.contains(x, y), "{w}x{h} base {base}: {:?} leaves the circle at ({x}, {y})", p.key);
                        }
                    }
                    for (i, a) in placed.iter().enumerate() {
                        for b in &placed[i + 1..] {
                            assert!(!overlap(&a.rect, &b.rect), "{w}x{h} base {base}: {:?} overlaps {:?}", a.key, b.key);
                        }
                    }
                    if !picking {
                        let digits: Vec<_> = placed.iter().filter_map(|p| if let Key::Digit(d) = p.key { Some((d, p.rect)) } else { None }).collect();
                        assert_eq!(digits.len(), base as usize);
                        let cols = digit_grid(base).0 as usize;
                        for (d, r) in &digits {
                            let (row, col) = (*d as usize / cols, *d as usize % cols);
                            let first = digits[0].1;
                            let left = digits[row * cols].1;
                            assert!((r.y - (first.y + row as f32 * (r.h + layout.gap))).abs() < 0.01, "base {base} digit {d} row");
                            assert!((r.x - (left.x + col as f32 * (r.w + layout.gap))).abs() < 0.01, "base {base} digit {d} column");
                        }
                    } else {
                        let bases = placed.iter().filter(|p| matches!(p.key, Key::Base(_))).count();
                        // The picker's 35 and the quick strip's six.
                        assert_eq!(bases, 35 + 6);
                    }
                }
            }
        }
    }
}

fn press(c: &mut Calc, keys: &[Key]) {
    for k in keys {
        c.press(*k);
    }
}

#[test]
fn keys_write_the_engines_syntax() {
    let mut c = Calc::new();
    press(&mut c, &[Key::Digit(2), Key::Write(keys::PI)]);
    assert_eq!(c.expr, "2*@pi");
    press(&mut c, &[Key::Clear, Key::Write(keys::SIN), Key::Digit(0)]);
    assert_eq!(c.expr, "#sin(0");
    assert!(matches!(c.shown, Shown::Preview(_)));
    press(&mut c, &[Key::Equals]);
    assert_eq!(c.expr, "#sin(0)");
    assert_eq!(c.value.as_ref().unwrap().real().to_f64(), 0.);
    // An operator after a result goes on from it.
    press(&mut c, &[Key::Write(keys::ADD), Key::Digit(5), Key::Equals]);
    assert_eq!(c.history.last().unwrap().line.expr, "&+5");
    assert_eq!(c.value.as_ref().unwrap().real().to_f64(), 5.);
    // The delete key takes a unit back whole.
    press(&mut c, &[Key::Write(keys::ASIN), Key::Back]);
    assert_eq!(c.expr, "");
    // i next to a number is a product.
    press(&mut c, &[Key::Digit(3), Key::Write(keys::I), Key::Equals]);
    assert_eq!(c.history.last().unwrap().line.expr, "3*[0,1]");
    assert_eq!(c.value.as_ref().unwrap().imag().to_f64(), 3.);
    // Half an expression is an error only at `=`.
    press(&mut c, &[Key::Digit(1), Key::Write(keys::ADD)]);
    assert_eq!(c.shown, Shown::Nothing);
    press(&mut c, &[Key::Equals]);
    assert_eq!(c.shown, Shown::Error("Incomplete expression!".into()));
}

#[test]
fn the_square_writes_two_in_the_base() {
    let mut c = Calc::new();
    press(&mut c, &[Key::Base(2), Key::Digit(1), Key::Digit(1), Key::Square]);
    assert_eq!(c.expr, "11^10");
    press(&mut c, &[Key::Equals]);
    assert_eq!(c.value.as_ref().unwrap().real().to_f64(), 9.);
}

#[test]
fn a_base_finishes_the_expression_first() {
    let mut c = Calc::new();
    press(&mut c, &[Key::Base(16), Key::Digit(15), Key::Digit(15)]);
    let change = c.press(Key::Base(10));
    assert!(change.relayout && change.save);
    assert_eq!(c.history.last().unwrap().line.base, 16);
    match &c.shown {
        Shown::Value(spans) => assert_eq!(coloured_vec_to_string(spans), "  255."),
        other => panic!("{other:?}"),
    }
    // The picker opens, and a base from it closes it.
    press(&mut c, &[Key::Bases]);
    assert!(c.picking);
    press(&mut c, &[Key::Base(7)]);
    assert!(!c.picking);
    assert_eq!(c.state.base, 7);
}

#[test]
fn the_precision_steps_in_half_stops() {
    let mut c = Calc::new();
    assert_eq!(c.state.digits, 12);
    press(&mut c, &[Key::More]);
    assert_eq!(c.state.digits, 16);
    press(&mut c, &[Key::Fewer, Key::Fewer]);
    assert_eq!(c.state.digits, 8);
    for _ in 0..64 {
        c.press(Key::More);
    }
    assert_eq!(c.state.digits, 1 << 12);
    for _ in 0..64 {
        c.press(Key::Fewer);
    }
    assert_eq!(c.state.digits, 1);
}

#[test]
fn the_vault_document_round_trips() {
    let mut c = Calc::new();
    press(&mut c, &[Key::Digit(6), Key::Write(keys::MUL), Key::Digit(7), Key::Equals]);
    press(&mut c, &[Key::Base(12), Key::Write(keys::ADD), Key::Write(keys::RAND), Key::Equals]);
    press(&mut c, &[Key::Angle, Key::More, Key::Write(keys::SIN), Key::Digit(7), Key::Digit(6), Key::Equals]);
    let saved = c.saved();
    let doc = saved.build(vsf::types::eagle_time::eagle_time_oscillations()).unwrap();
    let back = Saved::parse(&doc).unwrap();
    assert_eq!(back, saved);
    assert_eq!(back.lines.len(), 3);
    // Run again, every line gives what it gave, the random draw included.
    let again = Calc::restore(&back);
    assert_eq!(again.history.len(), 3);
    for (a, b) in again.history.iter().zip(&c.history) {
        assert_eq!(a.value, b.value, "{}", a.line.expr);
        assert_eq!(a.spans, b.spans);
    }
    assert_eq!((again.state.base, again.state.digits, again.state.radians), (12, 16, false));
    assert_eq!(again.shown, c.shown);
}
