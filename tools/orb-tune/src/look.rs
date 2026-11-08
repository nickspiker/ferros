//! The look of one console circle comes from the `orb` crate (the maths the device draws with); this is the desktop's side of it: the rows spread over rayon's threads, and the radio's name, which needs fluor's text.

pub use orb::look::*;

use fluor::pixel::G2t16;
use rayon::prelude::*;

/// One circle of radius `r` pixels in a square of side 2⌈r⌉ + 4, `ss` × `ss` samples a pixel (ss a power of two), with the name's layer `text` (a composite of the same size, or empty).
pub fn render(p: &Prep, r: f32, ss: u32, text: &[G2t16]) -> (Vec<G2t16>, usize) {
    let side = side(r);
    let mut out = vec![0u64; side * side];
    out.par_chunks_mut(side).enumerate().for_each(|(j, row)| {
        let t = if text.is_empty() { &[][..] } else { &text[j * side..(j + 1) * side] };
        render_row(p, r, ss, j, row, t);
    });
    (out, side)
}

/// The radio's name, when it is off: its flavour's colour a stop over the ring's, centred in a square of side `side`, as a composite layer.
pub fn name_layer(text: &mut fluor::text::TextRenderer, k: &Look, r: f32, side: usize) -> Vec<G2t16> {
    let mut buf = vec![0u64; side * side];
    // GPS shows its symbol, the pin (drawn in the crate's `orb_under`), not its name.
    if k.state != 0 || !k.orb || k.radio == GPS {
        return buf;
    }
    let name = RADIOS[k.radio].name;
    let fl = k.flavour.map(|g| g * g * 2.);
    // An 8-bit α + darkness colour for the text style: the name's codes, opaque.
    let code = fl.map(|v| (g2t(v) * 256.) as u32);
    let colour = 0xFF00_0000 | (255 - code[0]) << 16 | (255 - code[1]) << 8 | (255 - code[2]);
    let mut damage = fluor::canvas::Damage::new();
    let mut canvas = fluor::canvas::Canvas::new(&mut buf[..], side, side, &mut damage);
    // The size: the name's width a little over the circle's radius, whatever its length.
    let probe = fluor::text::TextStyle::new(r, colour);
    let wide = text.measure_text(name, &probe);
    let size = r * r * 1.25 / wide;
    let style = fluor::text::TextStyle::new(size, colour);
    let mid = side as f32 * 0.5;
    text.draw_text_center(&mut canvas, name, mid, mid, &style, None, None);
    buf
}
