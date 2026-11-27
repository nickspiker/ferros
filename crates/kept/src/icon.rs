//! Icons by their content identity (`hp`; owner, 2026-11-20): decoded once, kept in the vault, cached by the display.
//!
//! - **An icon's `hp`** is the BLAKE3 of the `icon` section its capsule carries (ferros_kist `icon_hp`): the same icon in two builds of a kist is one icon.
//! - **The index** (`kists.icons`, [`INDEX`], device data the stower writes at stow): a section `icon` per kist with an icon, `thing` (`ht`, the kist) and `icon` (`hp`, its icon now). The map reads it to know each circle's icon; the person's list does not carry it (KISTS.md "Icons").
//! - **A decoded icon** (in `kists.` under its `hp` as a 32-byte address): what the display draws, so the display decodes nothing (COMPOSITOR.md "Baked"): section `decoded`, `icon` (`hp`, the icon it is), `rgba` (`t_u3`, side × side × 4: the icon's codes as decoded, with the circle's coverage in alpha, [`texels`]) and `primaries` (what the codes are, [`Source`]; none in a document from before 2026-11-24, which is sRGB). Side is [`SIDE`], the icon's own size, never the size it is drawn at. Read back ([`read_decoded`]) as straight VSF RGB `g2t` codes: the source's sRGB numbers converted ([`from_srgb`]), VSF RGB as it is.
//! - **The display's cache** ([`Cache`]): a few icons built for drawing, each in a slot found by its `hp`; one missing is asked of the map, which sends its texels in pieces ([`Pieces`]).

use crate::document;
use alloc::vec::Vec;
use ferros_thing::{Hp, Ht};
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::types::Tensor;
use vsf::VsfType;

/// The icon decoder's endpoint (`icons`, std/icondec), the stower's alone: an icon's AV1 in, its texels out.
pub mod dec {
    /// `[OP_AV1][encoding][offset u32 LE][last u8][bytes…]`: a piece of the icon's AV1, in order from 0; the last decodes it. → [0] taken (on the last: decoded), [1] not an icon this decodes (refused, out of order, or more than memory holds: an icon is as large as its capsule carries, 2026-11-24).
    pub const OP_AV1: u8 = 0;
    /// `[OP_TEXELS][offset u32 LE]`: the decoded texels from `offset`, at most [`PIECE`] bytes. The decoder ends itself once it has handed over the last.
    pub const OP_TEXELS: u8 = 1;
    /// The icon's encoding: its codes `g2t`, or gamma 2 (untagged, vsf's rule), which the decoder takes to `g2t`.
    pub const G2T: u8 = 0;
    pub const GAMMA2: u8 = 1;
    /// The bytes one call carries either way: a call's message is at most 4 KiB, less the op and the header.
    pub const PIECE: usize = 4032;
}

/// The index's name in the `kists.` scope.
pub const INDEX: &[u8] = b"icons";
/// An icon's side in texels: a Photon avatar's (KISTS.md "Icons": one format, 256 × 256).
pub const SIDE: usize = 1 << 8;
/// A decoded icon's texels as the map hands them to the display: 256 KiB of straight `g2t` RGBA (the photopic grey is gone: nothing drew it).
pub const TEXELS: usize = SIDE * SIDE * 4;
/// The most kists the index holds: far past any deck, so an index past it is none this build reads.
pub const INDEX_MAX: usize = 1 << 10;

fn index_schema() -> SectionSchema {
    SectionSchema::new("icon").field("thing", TypeConstraint::Thing).field("icon", TypeConstraint::Blake3Provenance)
}

/// The index document: each kist's icon by its thing, stamped at `eagle`.
pub fn index(icons: &[(Ht, Hp)], eagle: i64) -> Option<Vec<u8>> {
    let mut b = vsf::VsfBuilder::new().creation_time_oscillations(eagle).provenance_only();
    for (t, h) in icons {
        let s = index_schema().build().set("thing", VsfType::from(*t)).ok()?.set("icon", VsfType::from(*h)).ok()?;
        b = b.add_unboxed("icon", s.encode().ok()?);
    }
    b.build().ok()
}

/// The index read back, verified whole first; None for anything that is not one (and a kist twice).
pub fn read_index(doc: &[u8]) -> Option<Vec<(Ht, Hp)>> {
    let (header, _) = vsf::verification::read_verified(doc, None).ok()?;
    if header.fields.len() > INDEX_MAX {
        return None;
    }
    let mut out: Vec<(Ht, Hp)> = Vec::new();
    for f in &header.fields {
        if f.name != "icon" {
            return None;
        }
        // WHY checked and `get`: the offsets are the document's own; a section past its end is no index, not a panic.
        let bytes = doc.get(f.offset_bytes..f.offset_bytes.checked_add(f.size_bytes)?)?;
        let s = SectionBuilder::parse(index_schema(), bytes).ok()?;
        let first = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first()).cloned();
        let (t, h) = (Ht::from_vsf(&first("thing")?).ok()?, Hp::from_vsf(&first("icon")?).ok()?);
        if out.iter().any(|(x, _)| *x == t) {
            return None;
        }
        out.push((t, h));
    }
    Some(out)
}

/// `thing`'s icon set in an index (or dropped with None); whether it changed.
pub fn set(icons: &mut Vec<(Ht, Hp)>, thing: Ht, icon: Option<Hp>) -> bool {
    let at = icons.iter().position(|(t, _)| *t == thing);
    match (at, icon) {
        (Some(i), Some(h)) if icons[i].1 == h => false,
        (Some(i), Some(h)) => {
            icons[i].1 = h;
            true
        }
        (None, Some(h)) => {
            icons.push((thing, h));
            true
        }
        (Some(i), None) => {
            icons.remove(i);
            true
        }
        (None, None) => false,
    }
}

/// What an icon's codes are, as its capsule's icon section names them (owner, 2026-11-24: "Let's assume they are all source sRGB instead of VSF rgb but keep the g2t curve", then the method: "Convert FROM sRGB primaries TO VSF RGB, then assign g2t", VSF RGB being gamma 2 ([`from_srgb`]'s steps); Kūaha's kept: "I like that one the way it is").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// The section names no primaries, or `primaries: srgb`: the codes are the source picture's own sRGB numbers (tools/app-icons' assigned icons of 2026-11-20 are, tagged `g2t` though nothing was converted), converted as the owner said ([`from_srgb`]).
    Srgb,
    /// `primaries: vsf`: VSF RGB codes, kept as they are (Kūaha's, authored in VSF RGB; Aemulus's, made in VSF RGB gamma 2 from its picture by its own tool, those codes its icon's as they stand: the same steps an sRGB icon takes here).
    Vsf,
}

impl Source {
    /// The `primaries` field's value as vsf's image sections name it (vsf::visual `Primaries`).
    pub fn primaries(self) -> &'static str {
        match self {
            Source::Srgb => "srgb",
            Source::Vsf => "vsf",
        }
    }

    /// From the `primaries` field: none or `srgb` is the source's sRGB numbers, `vsf` VSF RGB; any other primaries are none an icon carries, refused.
    pub fn from_primaries(p: Option<&str>) -> Option<Source> {
        match p {
            None | Some("srgb") => Some(Source::Srgb),
            Some("vsf") => Some(Source::Vsf),
            Some(_) => None,
        }
    }
}

/// The owner's highlight extension (2026-11-24): sRGB's linear light x, 0 to 1, as unbounded light L = tan(x · π/2), so sRGB's white is light without end and g2t's tanh brings it back to the top code; nothing clips, and no white is balanced.
/// In f64: f32's π/2 rounds up past the true π/2, where tan turns a huge negative, while f64's rounds down, where tan(π/2) is 1.6 × 10^16, positive and finite (tested).
pub fn extend_tan(x: f64) -> f64 {
    libm::tan(x * core::f64::consts::FRAC_PI_2)
}

/// The comparison extension: g2t's exact inverse of x's gamma 2, L = atanh(√x)², so a grey keeps the gamma 2 code it had (tanh(atanh(√x)) = √x) and only highlights and hue differ; sRGB's white is infinite light (atanh(1)).
pub fn extend_g2t_inverse(x: f64) -> f64 {
    let a = libm::atanh(libm::sqrt(x));
    a * a
}

/// An sRGB code triple as the `g2t` codes the display draws, extended by `extend`, the owner's steps (2026-11-24): "1. Read the pixel, sRGB code. 2. Decurve to sRGB linear x, in 0..1, float. 3. Extend the highlights: L = tan(x · π/2), so x = 1 → ∞ (white), all in float. 4. Convert sRGB primaries to VSF RGB linear with the matrix, in float. 5. Encode g2t properly: code = tanh(sqrt(L))".
/// Step 4 is vsf's `SRGB2VSF_RGB` (built thru 1931 XYZ, colorimetric): every entry positive and none zero, so a channel of any light, even infinite, times its entry is light of the same sign and no infinity ever meets a zero.
/// Step 5 is vsf's g2t encode (`delinearize_gamma2tanh_u8_f32`, truncation settled against its decoder): g2t's top code is the bucket of every light from atanh(255/256)², about 9.6 times diffuse white, to infinity, so a white of any of its channels' lights, however large, is code 255 in all three, neutral, and nothing is clipped.
#[allow(deprecated)]
pub fn from_srgb_extended(c: [u8; 3], extend: impl Fn(f64) -> f64) -> [u8; 3] {
    // sRGB is only the space an icon's codes arrive in (the owner's exception for decoding foreign pictures at ingest): decoded once, here, never encoded to.
    let x = c.map(vsf::colour::legacy::convert::linearize_srgb_u8);
    let l = x.map(|v| extend(v as f64) as f32);
    let v = vsf::colour::convert::apply_matrix_3x3_f32(&vsf::colour::legacy::constants::SRGB2VSF_RGB, &l);
    v.map(vsf::colour::convert::delinearize_gamma2tanh_u8_f32)
}

/// An sRGB code triple as the codes the display draws, the owner's pipeline ([`from_srgb_extended`] with [`extend_tan`]).
pub fn from_srgb(c: [u8; 3]) -> [u8; 3] {
    from_srgb_extended(c, extend_tan)
}

/// Straight RGBA texels from codes of `source` to the codes the display draws, in place: sRGB numbers converted ([`from_srgb`]), VSF RGB left as it is; alpha untouched.
pub fn to_g2t(rgba: &mut [u8], source: Source) {
    if source == Source::Vsf {
        return;
    }
    for p in rgba.chunks_exact_mut(4) {
        let c = from_srgb([p[0], p[1], p[2]]);
        p[..3].copy_from_slice(&c);
    }
}

fn decoded_schema() -> SectionSchema {
    let rgba = TypeConstraint::Custom {
        name: "rgba",
        description: "side x side x 4 bytes",
        validator: alloc::sync::Arc::new(|v: &VsfType| matches!(v, VsfType::t_u3(t) if t.data.len() == TEXELS && t.shape.iter().product::<usize>() == TEXELS)),
    };
    SectionSchema::new("decoded").field("icon", TypeConstraint::Blake3Provenance).field("rgba", rgba).field("primaries", TypeConstraint::AsciiText)
}

/// A decoded icon's document: the icon it is, its RGBA ([`TEXELS`]) as decoded, and what its codes are (`primaries`, [`Source`]), stamped at `eagle`. None for texels of another size.
pub fn decoded(icon: &Hp, rgba: &[u8], source: Source, eagle: i64) -> Option<Vec<u8>> {
    if rgba.len() != TEXELS {
        return None;
    }
    let primaries = VsfType::a(vsf::Ascii::checked("primaries", alloc::string::String::from(source.primaries())));
    let b = decoded_schema().build().set("icon", VsfType::from(*icon)).ok()?.set("rgba", VsfType::t_u3(Tensor::new(alloc::vec![SIDE, SIDE, 4], rgba.to_vec()))).ok()?.set("primaries", primaries).ok()?;
    document("decoded", b.encode().ok()?, eagle)
}

/// A decoded icon read back as the display takes it (its RGBA, [`TEXELS`], as `g2t` codes: [`to_g2t`] by what its codes are), only when it is the icon asked for: a value under one `hp` that says it is another is none.
/// A document that names no primaries (every icon decoded before 2026-11-24) is the source's sRGB numbers, so every icon already in a vault follows the conversion with nothing decoded again.
pub fn read_decoded(doc: &[u8], want: &Hp) -> Option<Vec<u8>> {
    let s = SectionBuilder::parse_document(decoded_schema(), doc, None).ok()?;
    let first = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first()).cloned();
    if Hp::from_vsf(&first("icon")?).ok()? != *want {
        return None;
    }
    let source = match first("primaries") {
        None => Source::Srgb,
        Some(VsfType::a(t)) => Source::from_primaries(Some(t.as_str()))?,
        Some(_) => return None,
    };
    match first("rgba") {
        Some(VsfType::t_u3(rgba)) => {
            let mut rgba = rgba.data;
            to_g2t(&mut rgba, source);
            Some(rgba)
        }
        _ => None,
    }
}

/// The share of each texel inside the icon's inscribed circle, in 255ths, measured on a 16 x 16 grid per texel (tools/app-icons made the bundle's alpha the same way): the circle is cut where it is drawn, and its edge's coverage rides in alpha.
fn coverage(x: usize, y: usize) -> u8 {
    // In 32nds of a texel, from the centre: the grid's points at x + (i + ½)/16 texels, the centre and the radius at SIDE/2.
    let r = (SIDE as i64) << 4;
    let mut inside = 0u32;
    for j in 0..16i64 {
        for i in 0..16i64 {
            let (dx, dy) = (((x as i64) << 5) + 2 * i + 1 - r, ((y as i64) << 5) + 2 * j + 1 - r);
            inside += (dx * dx + dy * dy <= r * r) as u32;
        }
    }
    // 0 to 256 points to 0 to 255, rounded.
    ((inside * 255 + 128) >> 8) as u8
}

/// What the display draws from a decoded icon picture (vsf::visual::av1 `decode` of its AV1, three planes of side x side 8-bit codes, R then G then B): straight `g2t` RGBA, the inscribed circle's coverage in alpha (KISTS.md "Icons"). `gamma2`: the codes are gamma 2 (an untagged icon), each taken to its `g2t` code thru its light. None for planes of another size.
pub fn texels(planes: &[u16], gamma2: bool) -> Option<Vec<u8>> {
    let n = SIDE * SIDE;
    if planes.len() != 3 * n {
        return None;
    }
    let g2t = |c: u16| -> u8 {
        if gamma2 {
            let v = c as f32 / 256.;
            vsf::colour::convert::delinearize_gamma2tanh_u8_f32(v * v)
        } else {
            c as u8
        }
    };
    let mut out = Vec::with_capacity(TEXELS);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let i = y * SIDE + x;
            out.extend_from_slice(&[g2t(planes[i]), g2t(planes[n + i]), g2t(planes[2 * n + i]), coverage(x, y)]);
        }
    }
    Some(out)
}

/// An icon's texels arriving in pieces (the map's, one call each): in order from the start, for one icon at a time. A piece for another icon, or out of order, starts again from it.
#[derive(Default)]
pub struct Pieces {
    icon: Option<Hp>,
    buf: Vec<u8>,
}

impl Pieces {
    /// One piece at `offset`: the whole texels when this one completes them, None while more are due or the piece was not the next (dropped).
    pub fn take(&mut self, icon: &Hp, offset: usize, piece: &[u8]) -> Option<Vec<u8>> {
        if self.icon != Some(*icon) || offset != self.buf.len() {
            self.icon = None;
            self.buf.clear();
            if offset != 0 {
                return None;
            }
            self.icon = Some(*icon);
        }
        // WHY: the length is the map's; texels past the icon's size are no icon, and the buffer would grow without end.
        if self.buf.len() + piece.len() > TEXELS {
            self.icon = None;
            self.buf.clear();
            return None;
        }
        self.buf.extend_from_slice(piece);
        if self.buf.len() < TEXELS {
            return None;
        }
        self.icon = None;
        Some(core::mem::take(&mut self.buf))
    }
}

/// The display's icons built for drawing, by `hp`, in a fixed number of slots: a slot's number is what the scene names the icon by.
pub struct Cache<T> {
    slots: Vec<Option<(Hp, T, u64)>>,
    tick: u64,
}

impl<T> Cache<T> {
    /// `n` slots, none full.
    pub fn new(n: usize) -> Cache<T> {
        Cache { slots: (0..n).map(|_| None).collect(), tick: 0 }
    }

    /// The slot holding `icon`, marked used now; None when it is not built here.
    pub fn slot(&mut self, icon: &Hp) -> Option<usize> {
        self.tick += 1;
        let t = self.tick;
        let i = self.slots.iter().position(|s| s.as_ref().is_some_and(|(h, _, _)| h == icon))?;
        if let Some(s) = self.slots[i].as_mut() {
            s.2 = t;
        }
        Some(i)
    }

    /// What slot `i` holds, for drawing.
    pub fn get(&self, i: usize) -> Option<&T> {
        self.slots.get(i).and_then(|s| s.as_ref()).map(|(_, t, _)| t)
    }

    /// `icon` built and kept: in its slot if it has one, else an empty slot, else the one used longest ago that `keep` does not hold (the icons the scene shows now). None when every slot is one `keep` holds.
    pub fn put(&mut self, icon: Hp, built: T, keep: &[usize]) -> Option<usize> {
        self.tick += 1;
        let t = self.tick;
        let i = match self.slots.iter().position(|s| s.as_ref().is_some_and(|(h, _, _)| *h == icon)) {
            Some(i) => i,
            None => match self.slots.iter().position(|s| s.is_none()) {
                Some(i) => i,
                None => (0..self.slots.len()).filter(|i| !keep.contains(i)).min_by_key(|&i| self.slots[i].as_ref().map_or(0, |s| s.2))?,
            },
        };
        self.slots[i] = Some((icon, built, t));
        Some(i)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec;

    fn hp(n: u8) -> Hp {
        Hp::from_bytes([n; 32])
    }

    #[test]
    fn the_index_round_trips_and_follows_a_change() {
        let k = |n: u8| ferros_thing::kist(&[n; 32], &[n; 32]);
        let mut icons = Vec::new();
        assert!(set(&mut icons, k(1), Some(hp(1))));
        assert!(set(&mut icons, k(2), Some(hp(2))));
        assert!(!set(&mut icons, k(1), Some(hp(1))));
        assert!(set(&mut icons, k(1), Some(hp(3))), "an update with another icon");
        let doc = index(&icons, 1 << 45).unwrap();
        assert_eq!(read_index(&doc), Some(vec![(k(1), hp(3)), (k(2), hp(2))]));
        assert!(set(&mut icons, k(2), None));
        assert_eq!(read_index(&index(&icons, 1).unwrap()), Some(vec![(k(1), hp(3))]));
        assert_eq!(read_index(&index(&[], 1).unwrap()), Some(vec![]));
        assert_eq!(read_index(&index(&[(k(1), hp(1)), (k(1), hp(2))], 1).unwrap()), None, "a kist twice");
        assert_eq!(read_index(&crate::map::view(&[1, 2, 3, 4], 1).unwrap()), None);
    }

    #[test]
    fn a_decoded_icon_round_trips_and_is_only_itself() {
        let rgba: Vec<u8> = (0..TEXELS).map(|i| (i * 7) as u8).collect();
        let doc = decoded(&hp(9), &rgba, Source::Vsf, 1 << 45).unwrap();
        assert_eq!(read_decoded(&doc, &hp(9)), Some(rgba.clone()));
        assert_eq!(read_decoded(&doc, &hp(8)), None, "a value that says it is another icon");
        assert_eq!(decoded(&hp(9), &rgba[1..], Source::Vsf, 1), None);
        assert!(doc.len() < TEXELS + (1 << 10), "the icon at its own size: {} bytes", doc.len());
        std::println!("a decoded icon: {} bytes", doc.len());
    }

    /// A decoded picture made what the display draws: `g2t` codes kept, gamma 2 codes taken to `g2t` thru their light, the inscribed circle's coverage in alpha (opaque at the centre, none in the corners, partial on the edge, and the same at the four quarters).
    #[test]
    fn texels_are_the_codes_inside_the_circle() {
        let n = SIDE * SIDE;
        let mut planes = alloc::vec![0u16; 3 * n];
        for i in 0..n {
            planes[i] = 200;
            planes[n + i] = 128;
            planes[2 * n + i] = 10;
        }
        let t = texels(&planes, false).unwrap();
        assert_eq!(t.len(), TEXELS);
        let at = |x: usize, y: usize| &t[(y * SIDE + x) * 4..(y * SIDE + x) * 4 + 4];
        assert_eq!(at(128, 128), &[200, 128, 10, 255]);
        assert_eq!(at(0, 0)[3], 0);
        assert_eq!(at(SIDE - 1, SIDE - 1)[3], 0);
        let edge = at(37, 37)[3];
        assert!(edge > 0 && edge < 255, "a texel on the circle is partly inside: {edge}");
        assert_eq!((at(SIDE - 38, 37)[3], at(37, SIDE - 38)[3], at(SIDE - 38, SIDE - 38)[3]), (edge, edge, edge), "the circle is the same in its four quarters");
        // Gamma 2: white stays white's code under tanh's headroom, black stays black.
        let g = texels(&planes.iter().map(|_| 0).collect::<Vec<u16>>(), true).unwrap();
        assert_eq!(&g[(128 * SIDE + 128) * 4..(128 * SIDE + 128) * 4 + 3], &[0, 0, 0]);
        let w = texels(&alloc::vec![255u16; 3 * n], true).unwrap();
        let c = w[(128 * SIDE + 128) * 4];
        assert_eq!(c, vsf::colour::convert::delinearize_gamma2tanh_u8_f32((255. / 256.) * (255. / 256.)));
        assert!(c < 255 && c > 180, "gamma 2's white lands on g2t's diffuse white, below the headroom: {c}");
        assert_eq!(texels(&planes[1..], false), None);
    }

    /// The owner's pipeline (2026-11-24): white of any channel's light is code 255 in all three, black is 0, f64's tan(π/2) positive and finite, a grey D65's light rising in every channel; the codes asked for printed under both extensions.
    #[test]
    fn srgb_highlights_extend_to_the_top_code() {
        let tan_white = extend_tan(1.);
        assert!(tan_white.is_finite() && tan_white > 1e15, "f64's tan(π/2): {tan_white}");
        assert_eq!(from_srgb([255; 3]), [255; 3], "white, neutral at the top code");
        assert_eq!(from_srgb_extended([255; 3], extend_g2t_inverse), [255; 3]);
        assert_eq!(from_srgb([0; 3]), [0; 3]);
        assert_eq!(from_srgb_extended([0; 3], extend_g2t_inverse), [0; 3]);
        // No white is balanced: an sRGB grey is D65 light, which in VSF RGB is (0.91, 1.03, 1.09) times it, so its codes lean that way below white; each channel rises with the grey and none goes back.
        let mut last = [0u8; 3];
        for c in 0..=255u8 {
            let g = from_srgb([c; 3]);
            assert!(g[0] <= g[1] && g[1] <= g[2], "a grey is D65's light, red the least, blue the most: {c} -> {g:?}");
            assert!((0..3).all(|k| g[k] >= last[k]), "codes rise with the grey: {c} -> {g:?} after {last:?}");
            last = g;
        }
        // The codes the coordinator asked for: x (sRGB linear) 0.214, 0.5, 0.9 and 1, by the sRGB code that decurves nearest each, and the three primaries.
        #[allow(deprecated)]
        let code_of = |x: f32| (0..=255u8).min_by(|&a, &b| (vsf::colour::legacy::convert::linearize_srgb_u8(a) - x).abs().total_cmp(&(vsf::colour::legacy::convert::linearize_srgb_u8(b) - x).abs())).unwrap();
        for x in [0.214f32, 0.5, 0.9, 1.] {
            let c = code_of(x);
            std::println!("x {x} (sRGB code {c}): tan {:?}, atanh {:?}", from_srgb([c; 3]), from_srgb_extended([c; 3], extend_g2t_inverse));
        }
        for (n, c) in [("red", [255, 0, 0]), ("green", [0, 255, 0]), ("blue", [0, 0, 255])] {
            std::println!("{n}: tan {:?}, atanh {:?}", from_srgb(c), from_srgb_extended(c, extend_g2t_inverse));
        }
        let mut v = alloc::vec![200u8, 100, 50, 255];
        to_g2t(&mut v, Source::Vsf);
        assert_eq!(v, [200, 100, 50, 255], "VSF RGB is kept");
        to_g2t(&mut v, Source::Srgb);
        let c = from_srgb([200, 100, 50]);
        assert_eq!(v, [c[0], c[1], c[2], 255], "alpha untouched");
        assert_eq!(Source::from_primaries(None), Some(Source::Srgb));
        assert_eq!(Source::from_primaries(Some("srgb")), Some(Source::Srgb));
        assert_eq!(Source::from_primaries(Some("vsf")), Some(Source::Vsf));
        assert_eq!(Source::from_primaries(Some("rec2020")), None);
    }

    /// A decoded document from before (no `primaries`) reads as sRGB numbers, converted; one that names `vsf` reads as it is.
    #[test]
    fn decoded_icons_read_by_what_their_codes_are() {
        let rgba: Vec<u8> = (0..TEXELS).map(|i| (i * 7) as u8).collect();
        let old = document("decoded", decoded_schema().build().set("icon", VsfType::from(hp(3))).unwrap().set("rgba", VsfType::t_u3(Tensor::new(alloc::vec![SIDE, SIDE, 4], rgba.clone()))).unwrap().encode().unwrap(), 1).unwrap();
        let mut want = rgba.clone();
        to_g2t(&mut want, Source::Srgb);
        assert_ne!(want, rgba);
        assert_eq!(read_decoded(&old, &hp(3)), Some(want.clone()));
        assert_eq!(read_decoded(&decoded(&hp(3), &rgba, Source::Srgb, 1).unwrap(), &hp(3)), Some(want));
        assert_eq!(read_decoded(&decoded(&hp(3), &rgba, Source::Vsf, 1).unwrap(), &hp(3)), Some(rgba));
    }

    #[test]
    fn pieces_make_one_icon_in_order() {
        let all: Vec<u8> = (0..TEXELS).map(|i| (i % 251) as u8).collect();
        let mut p = Pieces::default();
        let mut got = None;
        for (k, c) in all.chunks(4032).enumerate() {
            assert!(got.is_none());
            got = p.take(&hp(1), k * 4032, c);
        }
        assert_eq!(got, Some(all.clone()));
        // Out of order: dropped, and the next start begins again.
        assert_eq!(p.take(&hp(1), 4032, &all[4032..8064]), None);
        assert_eq!(p.take(&hp(1), 0, &all[..4032]), None);
        // Another icon mid-way starts again from it.
        assert_eq!(p.take(&hp(2), 0, &all[..4032]), None);
        let mut got = None;
        for (k, c) in all.chunks(1 << 12).enumerate() {
            got = p.take(&hp(2), k << 12, c);
        }
        assert_eq!(got, Some(all.clone()));
        // Past the size: no icon.
        assert_eq!(p.take(&hp(3), 0, &vec![0; TEXELS + 1]), None);
    }

    #[test]
    fn the_cache_keeps_what_the_scene_shows() {
        let mut c: Cache<u8> = Cache::new(2);
        assert_eq!(c.slot(&hp(1)), None);
        assert_eq!(c.put(hp(1), 10, &[]), Some(0));
        assert_eq!(c.put(hp(2), 20, &[]), Some(1));
        assert_eq!(c.slot(&hp(1)), Some(0), "used now");
        // Full: the one used longest ago goes (hp 2), unless the scene shows it.
        assert_eq!(c.put(hp(3), 30, &[0]), Some(1));
        assert_eq!(c.slot(&hp(2)), None);
        assert_eq!(c.get(1), Some(&30));
        assert_eq!(c.put(hp(4), 40, &[0, 1]), None, "every slot shown");
        // The same icon again takes its own slot.
        assert_eq!(c.put(hp(3), 31, &[0, 1]), Some(1));
        assert_eq!(c.get(1), Some(&31));
    }
}
