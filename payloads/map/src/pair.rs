//! The screenshot's pair (owner, 2026-11-18: "screenshot is the two finger slide"; docs/WINDOWS.md "Screenshot"): two fingers that landed on back and deck together, both slid up. Pure, so tools/chords tests it on the host.

/// Whether at least two of the held fingers have slid up past a twelfth of the screen's harmonic span (2wh/(w+h), the helm's old swipe threshold): `ups` is each held finger's rise, its landing row less its row now (positive up), and `w`, `h` the screen's size. Movement and finger count only, never time.
pub fn slid(ups: impl Iterator<Item = i64>, w: i64, h: i64) -> bool {
    let span = (2 * w * h) as f64 / (w + h) as f64;
    ups.filter(|&up| up as f64 > span / 12.).count() >= 2
}
