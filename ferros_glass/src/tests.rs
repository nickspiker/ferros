//! Host tests: the wrap (long words, manual returns, exactly full lines, overruns kept either way) on a grid of characters, and the rasteriser on a pixel buffer.

extern crate std;
use super::*;
use std::string::String;
use std::vec;
use std::vec::Vec;

/// The text laid out as the rows a reader sees, trailing spaces trimmed.
fn rows_of(text: &str, cols: usize, rows: usize, keep: Keep) -> Vec<String> {
    let mut g = vec![vec![b' '; cols]; rows];
    let mut heads = vec![vec![false; cols]; rows];
    let used = lay(text.as_bytes(), cols, rows, keep, |r, c, b, h| {
        assert!(r < rows && c < cols, "cell ({r}, {c}) outside a {cols}x{rows} grid");
        g[r][c] = b;
        heads[r][c] = h;
    });
    assert_eq!(used, rows_used(text.as_bytes(), cols, rows));
    g.truncate(used);
    g.into_iter().map(|r| String::from_utf8(r).unwrap().trim_end().into()).collect()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| String::from(*x)).collect()
}

#[test]
fn words_wrap_at_the_width() {
    assert_eq!(rows_of("the quick brown fox jumps", 10, 8, Keep::Earliest), s(&["the quick", "brown fox", "jumps"]));
}

#[test]
fn a_long_word_starts_a_line_and_is_cut_at_each_end() {
    assert_eq!(rows_of("at ferros_kernel/src/process.rs:1234", 10, 8, Keep::Earliest), s(&["at", "ferros_ker", "nel/src/pr", "ocess.rs:1", "234"]));
    // A word longer than a line at the start of a line: cut where it stands.
    assert_eq!(rows_of("abcdefghijk", 4, 8, Keep::Earliest), s(&["abcd", "efgh", "ijk"]));
}

#[test]
fn manual_returns_end_lines_and_keep_indentation() {
    assert_eq!(rows_of("one\ntwo\n\n  four", 10, 8, Keep::Earliest), s(&["one", "two", "", "  four"]));
}

#[test]
fn an_exactly_full_line_then_a_return_is_one_line() {
    assert_eq!(rows_of("abcd\nefgh\n", 4, 8, Keep::Earliest), s(&["abcd", "efgh"]));
    // An exactly full line followed by a word: the word on the next line, the space between dropped.
    assert_eq!(rows_of("abcd efgh", 4, 8, Keep::Earliest), s(&["abcd", "efgh"]));
    // A word ending exactly at the edge followed by spaces: no blank line, no indent.
    assert_eq!(rows_of("ab cd    ef", 5, 8, Keep::Earliest), s(&["ab cd", "ef"]));
    // A final return does not count a line.
    assert_eq!(rows_used(b"abcd\n", 4, 8), 1);
    assert_eq!(rows_used(b"", 4, 8), 0);
    assert_eq!(rows_used(b"\n", 4, 8), 1);
}

#[test]
fn overrun_keeps_the_earliest_lines() {
    let text = "l0\nl1\nl2\nl3\nl4\nl5";
    assert_eq!(rows_of(text, 6, 4, Keep::Earliest), s(&["l0", "l1", "l2", "..."]));
}

#[test]
fn overrun_keeps_the_latest_lines() {
    let text = "l0\nl1\nl2\nl3\nl4\nl5";
    assert_eq!(rows_of(text, 6, 4, Keep::Latest), s(&["...", "l3", "l4", "l5"]));
}

#[test]
fn overrun_by_wrapping_alone() {
    // One long paragraph that wraps past the grid.
    let text = "aaaa bbbb cccc dddd eeee";
    assert_eq!(rows_of(text, 4, 3, Keep::Earliest), s(&["aaaa", "bbbb", "..."]));
    assert_eq!(rows_of(text, 4, 3, Keep::Latest), s(&["...", "dddd", "eeee"]));
}

#[test]
fn text_that_exactly_fills_the_grid_has_no_mark() {
    assert_eq!(rows_of("a\nb\nc", 4, 3, Keep::Earliest), s(&["a", "b", "c"]));
    assert_eq!(rows_of("a\nb\nc", 4, 3, Keep::Latest), s(&["a", "b", "c"]));
}

#[test]
fn other_characters_are_one_question_mark_each() {
    // "é" is two UTF-8 bytes, "→" three: one cell each.
    // A tab is a space; a carriage return nothing.
    assert_eq!(rows_of("caf\u{e9} \u{2192}\tx\r\n\u{1}", 20, 4, Keep::Earliest), s(&["caf? ? x", "?"]));
}

/// The cells laid out as ids, one row a vector, for text with tatau's cells (not ASCII, so not a `String`).
fn cells_of(text: &str, cols: usize, rows: usize) -> Vec<Vec<u8>> {
    let mut g = vec![vec![b' '; cols]; rows];
    let used = lay(text.as_bytes(), cols, rows, Keep::Earliest, |r, c, b, _| g[r][c] = b);
    g.truncate(used);
    g
}

#[test]
fn a_dozenal_digit_takes_two_cells_and_the_point_and_arrows_one() {
    // ↑ 1 · 6 B : the up arrow, digit one, the point, digit six, digit eleven.
    let d = |k: u8| [DOZENAL + 2 * k, DOZENAL + 2 * k + 1];
    let g = cells_of("\u{2191}\u{11}\u{B7}\u{16}\u{1B} s", 12, 2);
    let mut want = vec![UP];
    want.extend(d(1));
    want.push(POINT);
    want.extend(d(6));
    want.extend(d(11));
    want.extend(b" s  ");
    assert_eq!(g, vec![want]);
    assert_eq!(cells_of("\u{2193}", 4, 1), vec![vec![DOWN, b' ', b' ', b' ']]);
}

#[test]
fn a_dozenal_digit_is_never_split_across_lines() {
    // Five cells a line: "ab" and two digits (2 + 4 cells) is a word too long for a line, cut so the second digit starts the next line whole.
    let g = cells_of("ab\u{10}\u{11}", 5, 4);
    assert_eq!(g, vec![vec![b'a', b'b', DOZENAL, DOZENAL + 1, b' '], vec![DOZENAL + 2, DOZENAL + 3, b' ', b' ', b' ']]);
    // One column: the left half alone, never a cell past the grid.
    assert_eq!(cells_of("\u{10}", 1, 2), vec![vec![DOZENAL]]);
}

#[test]
fn the_dozenal_cells_are_drawn_and_sit_on_the_baseline() {
    // Every digit (its two halves together), the point and the arrows light something; none reaches the bitmap font's bottom row (the glyphs end above it, as the ASCII does).
    for id in DOZENAL..=DOWN {
        assert_eq!(glyph(id)[GLYPH_H - 1], 0, "cell G#{id:X} reaches the last row");
    }
    for id in [POINT, UP, DOWN] {
        assert!(glyph(id).iter().any(|&r| r != 0), "cell G#{id:X} is empty");
    }
    for k in 0..12 {
        let (l, r) = (glyph(DOZENAL + 2 * k), glyph(DOZENAL + 2 * k + 1));
        assert!(l.iter().zip(r.iter()).any(|(&a, &b)| a | b != 0), "digit {k} is empty");
    }
}

#[test]
fn numbers_read_in_the_base_for_the_person_and_a_hash_for_the_developer() {
    use tatau::{Base, Fmt, Style};
    let dev = |n: Num| {
        let mut t = String::new();
        n.dev(&mut t).unwrap();
        t
    };
    let person = |n: Num, b: Base| {
        let mut t = String::new();
        n.person(&mut t, Fmt::new(b, Style::Words)).unwrap();
        t
    };
    assert_eq!(dev(Num::Count(12)), "A#12");
    assert_eq!(dev(Num::Bits(0x96000045)), "G#96000045");
    assert_eq!(dev(Num::Word(0xDEAD)), "G#000000000000DEAD");
    assert_eq!(dev(Num::Line(42)), "42");
    assert_eq!(dev(Num::Ns(16_000_000_000)), "A#16 s");
    assert_eq!(dev(Num::Bytes(4096)), "A#4096 bytes");
    // A position is its digits in the base, never doublings: 144 is one gross.
    assert_eq!(person(Num::Index(144), Base::Dms), "Zila Zil Zil");
    assert_eq!(person(Num::Bits(0x1F), Base::Hex), "1F");
    assert_eq!(person(Num::Line(144), Base::Arabic), "144");
    // An amount in DMS is its doublings: 4096 is twelve (Zila Zil) with a zero digit after the point; zero has none and reads 0.
    assert_eq!(person(Num::Count(4096), Base::Dms), "Zila Zil\u{B7}Zil");
    assert_eq!(person(Num::Count(0), Base::Dms), "0");
    assert_eq!(person(Num::Count(4096), Base::Arabic), "4096");
    // The same line said to both.
    let line = |out: &mut dyn Say| {
        out.s("core ");
        out.n(Num::Index(3));
        out.s(", crumb ");
        out.n(Num::Bits(0xEE12));
    };
    let (mut d, mut p) = (String::new(), String::new());
    line(&mut Dev(&mut d));
    line(&mut Person(&mut p, Fmt::new(Base::Dms, Style::Glyphs)));
    assert_eq!(d, "core A#3, crumb G#EE12");
    // G#EE12 is 60946, dozenal 2 11 3 2 10: glyphs U+0012 U+001B U+0013 U+0012 U+001A.
    assert_eq!(p, "core \u{13}, crumb \u{12}\u{1B}\u{13}\u{12}\u{1A}");
}

#[test]
fn the_headline_is_up_to_the_first_return() {
    let mut heads = Vec::new();
    lay(b"ferros panic wraps\nmessage", 8, 8, Keep::Earliest, |r, _, _, h| heads.push((r, h)));
    assert!(heads.iter().all(|&(r, h)| h == (r < 3)), "{heads:?}");
}

#[test]
fn degenerate_grids_draw_nothing() {
    let mut n = 0;
    assert_eq!(lay(b"abc", 0, 4, Keep::Earliest, |_, _, _, _| n += 1), 0);
    assert_eq!(lay(b"abc", 4, 0, Keep::Latest, |_, _, _, _| n += 1), 0);
    assert_eq!(n, 0);
    // One row and an overrun: the row is the mark.
    assert_eq!(rows_of("a\nb", 4, 1, Keep::Earliest), s(&["..."]));
    // Narrower than the mark.
    assert_eq!(rows_of("a\nb\nc", 2, 2, Keep::Latest), s(&["..", "c"]));
}

#[test]
fn every_byte_lays_out_inside_the_grid() {
    // Every byte value, in runs, at several widths: nothing outside the grid, nothing panics.
    let all: Vec<u8> = (0..=255u8).chain(b"   \n\n word ".iter().copied()).cycle().take(4096).collect();
    for cols in 1..9 {
        for rows in 1..9 {
            for keep in [Keep::Earliest, Keep::Latest] {
                lay(&all, cols, rows, keep, |r, c, b, _| assert!(r < rows && c < cols && ((0x20..=0x7E).contains(&b) || (DOZENAL..=DOWN).contains(&b))));
            }
        }
    }
}

#[test]
fn the_grid_follows_the_span() {
    // The Pro (husky, 1344 x 2992): span 1854, scale 3.
    let pro = Glass::new(1344, 2992).unwrap();
    assert_eq!((pro.scale, pro.cols, pro.rows, pro.y0), (3, 51, 55, 231));
    // The 8a (akita, 1080 x 2400): span 1489, scale 2.
    let a = Glass::new(1080, 2400).unwrap();
    assert_eq!((a.scale, a.cols, a.rows, a.y0), (2, 61, 66, 186));
    // The grid is centred and inside the panel on both.
    for g in [pro, a] {
        assert!(g.x0 + g.cols * g.cell_w() <= g.width && g.x0 * 2 + g.cols * g.cell_w() >= g.width - 1);
        assert!(g.y0 + g.rows * g.cell_h() + g.pad() <= g.height);
    }
    // Registers that are no panel.
    assert_eq!(Glass::new(0, 2992), None);
    assert_eq!(Glass::new(1344, 0), None);
    assert_eq!(Glass::new(1 << 15, 2992), None);
    assert_eq!(Glass::new(300, 300), None);
}

/// A panel in memory: one byte a pixel, 0 untouched, 1 ground, 2 ink, 3 accent.
struct Mem {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Surface for Mem {
    fn span(&mut self, y: usize, x0: usize, x1: usize, paint: Paint) {
        assert!(y < self.h && x0 < x1 && x1 <= self.w, "span y {y} x {x0}..{x1} outside {}x{}", self.w, self.h);
        let v = match paint {
            Paint::Ground => 1,
            Paint::Ink => 2,
            Paint::Accent => 3,
        };
        self.px[y * self.w + x0..y * self.w + x1].fill(v);
    }
}

#[test]
fn the_rasteriser_draws_the_glyphs_at_scale_on_the_ground() {
    let g = Glass::new(1344, 2992).unwrap();
    let mut m = Mem { w: g.width, h: g.height, px: vec![0; g.width * g.height] };
    let used = draw(&g, b"ferros panic\nA", Keep::Earliest, &mut m);
    assert_eq!(used, 2);
    let (cw, ch, sc) = (g.cell_w(), g.cell_h(), g.scale);
    // The ground: every pixel of the band's rows is painted, nothing above or below it.
    let (b0, b1) = (g.y0 - g.pad(), g.y0 + used * ch + g.pad());
    for y in 0..g.height {
        let row = &m.px[y * g.width..(y + 1) * g.width];
        if (b0..b1).contains(&y) {
            assert!(row.iter().all(|&p| p != 0), "row {y} has unpainted pixels");
        } else {
            assert!(row.iter().all(|&p| p == 0), "row {y} painted outside the band");
        }
    }
    // Each cell holds its glyph exactly: a lit font pixel is a scale x scale square of ink (accent on the headline), an unlit one ground.
    let check = |row: usize, col: usize, c: u8, lit: u8| {
        for (gy, &bits) in glyph(c).iter().enumerate() {
            for gx in 0..GLYPH_W {
                let want = if bits >> (7 - gx) & 1 != 0 { lit } else { 1 };
                for sy in 0..sc {
                    for sx in 0..sc {
                        let (x, y) = (g.x0 + col * cw + gx * sc + sx, g.y0 + row * ch + gy * sc + sy);
                        assert_eq!(m.px[y * g.width + x], want, "cell ({row}, {col}) {:?} font pixel ({gx}, {gy})", c as char);
                    }
                }
            }
        }
    };
    for (i, &c) in b"ferros panic".iter().enumerate() {
        check(0, i, c, 3);
    }
    check(1, 0, b'A', 2);
}

#[test]
fn the_rasteriser_fills_to_the_bottom_and_stays_inside() {
    // Far more text than fits: the last row is used, every span inside the panel (Mem asserts it).
    let mut text = String::new();
    for i in 0..400 {
        text.push_str(&std::format!("line A#{i} with some words to wrap across the width of the glass\n"));
    }
    for (w, h) in [(1344, 2992), (1080, 2400), (2992, 1344)] {
        let g = Glass::new(w, h).unwrap();
        let mut m = Mem { w, h, px: vec![0; w * h] };
        assert_eq!(draw(&g, text.as_bytes(), Keep::Latest, &mut m), g.rows);
        // Ink on the last row of cells.
        let last = g.y0 + (g.rows - 1) * g.cell_h();
        assert!((last..last + g.cell_h()).any(|y| m.px[y * w..(y + 1) * w].contains(&2)));
    }
}

#[test]
fn buf_drops_what_does_not_fit() {
    use core::fmt::Write;
    let mut b = Buf::<8>::new();
    write!(b, "A#{} and more", 1234).unwrap();
    assert_eq!(b.bytes(), b"A#1234 a");
}
