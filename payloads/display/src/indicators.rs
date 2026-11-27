//! The privacy indicators beside the camera hole (docs/QUICK.md "The privacy indicators"; owner, 2026-11-26: "be classy about it").
//!
//! - **What.** The camera a ring round the lens (a solid disc behind the hole, the knockout in front making it a stroke), and the other rights as beads threaded round it (beads.rs, the geometry): left toward GPS motion low and fine location level with the lens, the spare above them never drawn; right toward Bluetooth nearby scanning low, the microphones level, screen above.
//! - **Lit, missed, nothing.** An indicator lit now is solid in its colour; one used since the person last looked and not lit now is the same shape at a quarter of the opacity; any other draws nothing, the lens bare.
//! - **The source.** Kernel fact 90 (ferros_sys `uses`, docs/KERNEL.md "Rights in use"), written by the kernel from what it sees, never by a kist: read with the console's facts each pass of the console's worker (`console_draw`). An unknown word (all ones: a kernel from before) draws nothing.
//! - **How it draws.** A layer of its own over the hole's rows, drawn once in fluor's α + darkness and baked in place, as a quick circle's sprite is (quick.rs), again only when what is lit changes (or the bake); composed into the bars' decoded rows under the console's pixels and in front of the quick row (`bar_decode`), its rows owed a compose as the quick row's are, so the render publishes just them.
//! - **Full screen.** The bars are hidden in full screen, but not this: a full-screen app must not be able to hide a live microphone. The columns each row holds of the layer go with the bars' buffers (`SPAN`, swapped with them), and in full screen `bar_line` gives those alone, so the CPU's publish and the GPU's front layer (gpu.rs `front_row`, made from `bar_line`) keep them in front of the app. The hit map is not changed: the beads are no touch target.

use super::*;
use ferros_sys::uses;

/// The indicators' wavelengths in nm (owner, 2026-11-26), each a console colour by `spectral_flavour`: one place to tune them.
pub const MIC_NM: f32 = 590.;
pub const LOCATION_NM: f32 = 550.;
pub const MOTION_NM: f32 = 470.;
pub const SCREEN_NM: f32 = 640.;
/// The camera's colour: white, equal codes at the lit grey's light (the console's and the quick row's `LIT_GREY`), a lens lit like a flash.
const CAMERA_WHITE: f32 = 9000.;
/// A missed indicator (used since the person last looked, not lit now): the lit shape at its opacity shifted down by this, a quarter.
const MISSED_LOG2: u32 = 2;
/// How far a disc's drawing reaches past its radius: its antialiased edge, the C∞ step across a pixel from half a pixel inside to half outside, and half a pixel of slack.
const REACH: f32 = 1.;

/// Each slot's indicator (ferros_sys `uses` bit), `[side][slot]` as beads.rs numbers them (side 0 left, slot 0 the lowest); None the spare, never drawn.
pub const SLOT_BITS: [[Option<u32>; beads::SLOTS]; 2] = [[Some(uses::MOTION), Some(uses::LOCATION), None], [Some(uses::NEARBY), Some(uses::MIC), Some(uses::SCREEN)]];

/// Fact 90's word as the console's worker last read it (`console_draw`); all ones, unknown, until then.
pub static WORD: AtomicU64 = AtomicU64::new(u64::MAX);

/// The indicators now: (lit, missed), a bit each; none for an unknown word. Missed is only what is not lit too, so a shape is drawn once.
pub fn bits() -> (u8, u8) {
    let w = WORD.load(Ordering::Acquire);
    if w == u64::MAX { (0, 0) } else { (uses::live(w), uses::missed(w) & !uses::live(w)) }
}

/// An indicator's colour, working values (gamma 2, white at Q14): the camera white, the microphones, fine location (GPS's own 550 nm), motion and screen by their wavelengths, nearby scanning Bluetooth's flavour.
pub fn colour(bit: u32) -> (f32, f32, f32) {
    match bit {
        uses::CAMERA => (CAMERA_WHITE, CAMERA_WHITE, CAMERA_WHITE),
        uses::MIC => spectral_flavour(MIC_NM),
        uses::LOCATION => spectral_flavour(LOCATION_NM),
        uses::NEARBY => flavour(Corner::Bluetooth).0,
        uses::MOTION => spectral_flavour(MOTION_NM),
        _ => spectral_flavour(SCREEN_NM),
    }
}

/// An indicator's opacity now: whole while lit, a quarter while missed, None (nothing drawn) otherwise.
pub fn opacity(bit: u32, (lit, missed): (u8, u8)) -> Option<f32> {
    if lit >> bit & 1 != 0 {
        Some(1.)
    } else if missed >> bit & 1 != 0 {
        Some(1. / (1 << MISSED_LOG2) as f32)
    } else {
        None
    }
}

/// Every disc the indicators can draw, with its bit: the camera's first, then the beads but the spare.
pub fn discs(b: &beads::Beads) -> impl Iterator<Item = ((f32, f32, f32), u32)> + '_ {
    let beads = (0..2).flat_map(move |side| (0..beads::SLOTS).filter_map(move |j| SLOT_BITS[side][j].map(|bit| ((b.at[side][j].0, b.at[side][j].1, b.bead), bit))));
    core::iter::once((b.cam, uses::CAMERA)).chain(beads)
}

/// The solved geometry, once per screen size.
static mut GEOMETRY: Option<((usize, usize), beads::Beads)> = None;

/// The indicators' geometry on this screen (beads.rs `solve` about `ferros_sys::cutout_hole`, beside the app's circle), solved once per size and logged with its checks: the top bead's room under the glass's edge hairline and the least room to the GPS and Bluetooth circles (the console's drawn reach included).
pub fn geometry(width: usize, height: usize) -> beads::Beads {
    if let Some((size, b)) = unsafe { *(&raw const GEOMETRY) } {
        if size == (width, height) {
            return b;
        }
    }
    let b = beads::solve(ferros_sys::cutout_hole(), circle(width, height));
    let outline = GlassOutline::new(width, height);
    let glass = b.at.iter().flatten().map(|c| outline.dist(c.0, c.1) - b.bead - EDGE_HAIRLINE).fold(f32::MAX, f32::min);
    let (corners, n) = corner_layout(width, height);
    let room = |what: Corner| corners[..n].iter().find(|c| c.3 == what).map(|c| b.gap((c.0, c.1, c.2)) - CONSOLE_REACH);
    let mut l = Log { grants: unsafe { PANIC_GRANTS }, buf: [0; 256], n: 0 };
    let v = |l: &mut Log, x: f32| {
        // Hundredths of a pixel, A# decimal; a negative room said with its sign.
        if x < 0. {
            l.s(b"-");
        }
        let h = (libm::fabsf(x) * 100.) as u64;
        l.s(b"A#");
        l.dec(h / 100);
        l.s(b".");
        l.s(&[b'0' + (h / 10 % 10) as u8, b'0' + (h % 10) as u8]);
    };
    l.s(b"display: indicators: camera disc r ");
    v(&mut l, b.cam.2);
    l.s(b", beads r ");
    v(&mut l, b.bead);
    l.s(b" on a ring of ");
    v(&mut l, b.ring);
    l.s(b", lowest at ");
    v(&mut l, b.low[0] * (180. / core::f32::consts::PI));
    l.s(b"/");
    v(&mut l, b.low[1] * (180. / core::f32::consts::PI));
    l.s(b" degrees, step ");
    v(&mut l, b.step * (180. / core::f32::consts::PI));
    l.s(b"; top at row ");
    v(&mut l, b.top());
    l.s(b"; room: glass ");
    v(&mut l, glass);
    l.s(b", GPS ");
    v(&mut l, room(Corner::Gps).unwrap_or(f32::MAX));
    l.s(b", Bluetooth ");
    v(&mut l, room(Corner::Bluetooth).unwrap_or(f32::MAX));
    l.s(b"\n");
    l.flush();
    unsafe { GEOMETRY = Some(((width, height), b)) };
    b
}

/// A console circle's drawn reach past its radius (`console_draw`'s REACH), and a bead's own (`REACH`): the room the geometry's check leaves between them.
const CONSOLE_REACH: f32 = 2.5 + REACH;

/// The layer: every pixel the indicators can draw, row by row from `y0`, each row a span of columns [x0, x1) at `at` in `data`; drawn in fluor's α + darkness, then baked in place (bake.rs), as the bars are. `lit` each row's columns holding anything as last drawn, (0, 0) for none.
struct Layer {
    y0: i64,
    span: &'static [(u16, u16, u32)],
    lit: &'static mut [(u16, u16)],
    data: &'static mut [u64],
    /// The look it was drawn with (`key`); 0 not drawn.
    key: u64,
}

static mut LAYER: Option<Layer> = None;

/// The columns of each bars' top row the layer's compose wrote (`compose`), as the bars' decoded rows hold them: the back buffers the console's worker writes, swapped with the front at the render thread's swap (`swap`), as `BAR_SPAN` is. `bar_line` reads the front in full screen.
static mut SPAN: usize = 1;
static mut SPAN_BACK: usize = 1;

/// The layer, made once at start (before the heap's mark, kept for the process): its rows and spans from the geometry, its data zeroed (drawn at the first `update`); and the spans the bars carry, every top row of them.
pub fn init(width: usize, height: usize) {
    let b = geometry(width, height);
    let (y0, y1) = b.rows(REACH);
    let top = unsafe { BAR_TOP };
    // The bars hold the layer's rows: the beads stand above the app circle's top, and the bars' top rows reach past it (the chrome rows, the quick row's floor).
    assert!(y0 >= 0 && y1 as usize <= top, "indicators: their rows outrun the bars' top rows");
    let mut span = alloc::vec::Vec::with_capacity((y1 - y0) as usize);
    let mut at = 0u32;
    for row in y0..y1 {
        let fy = row as f32 + 0.5;
        let (mut x0, mut x1) = (width as f32, 0f32);
        for ((x, y, r), _) in discs(&b) {
            let re = r + REACH;
            let h2 = re * re - (fy - y) * (fy - y);
            if h2 > 0. {
                let half = libm::sqrtf(h2);
                // WHY the min and max: the row's columns are the union of every disc's chord on it, an extent, never a clamp.
                x0 = x0.min(libm::floorf(x - half));
                x1 = x1.max(libm::ceilf(x + half));
            }
        }
        // WHY the test: a row of the band that no disc's reach crosses (none on these screens, the camera's disc spanning them) holds no columns; the discs lie inside the glass by the geometry's check, so the columns of the rest are on the row.
        let (x0, x1) = if x0 < x1 { (x0 as u16, x1 as u16) } else { (0, 0) };
        span.push((x0, x1, at));
        at += (x1 - x0) as u32;
    }
    let n = span.len();
    let data = alloc::vec![0u64; at as usize].leak();
    let lit = alloc::vec![(0u16, 0u16); n].leak();
    unsafe {
        LAYER = Some(Layer { y0, span: span.leak(), lit, data, key: 0 });
        SPAN = alloc::vec![(0u16, 0u16); top].leak().as_mut_ptr() as usize;
        SPAN_BACK = alloc::vec![(0u16, 0u16); top].leak().as_mut_ptr() as usize;
    }
}

/// The layer's look now as one key: what is lit and missed, and the bake (a brightness change bakes it again). Never 0, which is "not drawn yet".
fn key(bits: (u8, u8)) -> u64 {
    let mut k = 0x9E37_79B9_7F4A_7C15u64;
    for v in [bits.0 as u64 | (bits.1 as u64) << 8, bake::generation() as u64] {
        k = (k.rotate_left(7) ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    k | 1
}

/// The layer drawn from empty: each disc lit or missed solid in its colour at its opacity, its edge the C∞ step across a pixel (`draw::bump_step`), front to back by `under` (the discs only touch, so the order is only at their antialiased edges); each row's columns holding anything found; then baked in place.
fn render(b: &beads::Beads, bits: (u8, u8)) {
    use fluor::pixel::{Blend, BlendMode};
    let Some(ly) = (unsafe { (*(&raw mut LAYER)).as_mut() }) else { return };
    let (y0, span) = (ly.y0, ly.span);
    let data = &mut *ly.data;
    data.fill(0);
    for ((x, y, r), bit) in discs(b) {
        let Some(op) = opacity(bit, bits) else { continue };
        let ink = g2t_colour(colour(bit));
        let re = r + REACH;
        // The disc's rows inside the layer: the layer's rows are the discs' with their reach (`init`), so every one of them is.
        let (ra, rb) = ((libm::floorf(y - re) as i64 - y0) as usize, (libm::ceilf(y + re) as i64 - y0) as usize);
        for j in ra..rb {
            let (x0, _, o) = span[j];
            let dy = (y0 + j as i64) as f32 + 0.5 - y;
            // The disc's columns on this row, inside the row's span by its construction (the span is the union of the discs' chords with their reach).
            let h2 = re * re - dy * dy;
            if h2 <= 0. {
                continue;
            }
            let half = libm::sqrtf(h2);
            let (ca, cb) = (libm::floorf(x - half) as usize, libm::ceilf(x + half) as usize);
            for px in ca..cb {
                let dx = px as f32 + 0.5 - x;
                let cov = draw::bump_step(r - libm::sqrtf(dx * dx + dy * dy) + 0.5) * op;
                if cov <= 0. {
                    continue;
                }
                let i = o as usize + (px - x0 as usize);
                data[i] = data[i].under(ink | ((cov * 65535.) as u64) << 48, BlendMode::Normal);
            }
        }
    }
    for (j, &(x0, x1, o)) in span.iter().enumerate() {
        let row = &data[o as usize..o as usize + (x1 - x0) as usize];
        let first = row.iter().position(|&p| p >> 48 != 0);
        ly.lit[j] = match first {
            Some(a) => (x0 + a as u16, x0 + row.iter().rposition(|&p| p >> 48 != 0).map_or(a, |z| z) as u16 + 1),
            None => (0, 0),
        };
    }
    bake::now().g2t_in_place(data);
}

/// The console worker's pass over the indicators (`console_worker`): the layer drawn again when what is lit changed (or the bake). Returns the bars' rows owed a compose: the layer's, when it was drawn.
pub fn update(width: usize, height: usize) -> Option<(usize, usize)> {
    let bits = bits();
    let k = key(bits);
    let (y0, n) = {
        let Some(ly) = (unsafe { (*(&raw mut LAYER)).as_mut() }) else { return None };
        if ly.key == k {
            return None;
        }
        ly.key = k;
        (ly.y0, ly.span.len())
    };
    render(&geometry(width, height), bits);
    Some((y0 as usize, y0 as usize + n))
}

/// The layer composed into one decoded row of the bars, screen row `y` of the top band (`bar_decode`), under what is already there (the console's circles): copied where nothing is, folded under it (`under_pm`) where something is. Its columns, [from, to), (0, 0) for none, kept for the bars' swap (`SPAN_BACK`) and returned.
pub fn compose(y: usize, out: &mut [u64]) -> (usize, usize) {
    let s = row(y, out);
    unsafe { *(SPAN_BACK as *mut (u16, u16)).add(y) = (s.0 as u16, s.1 as u16) };
    s
}

fn row(y: usize, out: &mut [u64]) -> (usize, usize) {
    let Some(ly) = (unsafe { (*(&raw const LAYER)).as_ref() }) else { return (0, 0) };
    let j = y as i64 - ly.y0;
    // WHY the test: the bars' compose asks for every row of the top band, and the layer holds only the hole's rows; there is nothing of it on the others.
    if j < 0 || j >= ly.span.len() as i64 {
        return (0, 0);
    }
    let (x0, _, o) = ly.span[j as usize];
    let (a, z) = (ly.lit[j as usize].0 as usize, ly.lit[j as usize].1 as usize);
    for x in a..z {
        let s = ly.data[o as usize + x - x0 as usize];
        if s >> 63 != 0 {
            continue;
        }
        let f = out[x];
        out[x] = if f >> 63 != 0 { s } else { let (ch, t) = under_pm(f & ((1 << 48) - 1), trans(f), s & ((1 << 48) - 1), trans(s)); ch | t << 49 };
    }
    (a, z)
}

/// The bars swapped (the render thread, at a pass's top): the layer's columns go with them.
pub fn swap() {
    unsafe { core::mem::swap(&mut *(&raw mut SPAN), &mut *(&raw mut SPAN_BACK)) };
}

/// The columns of the bars' top row `b` (a screen row) holding the layer, as the bars on the screen hold them, (0, 0) for none: what `bar_line` gives in full screen.
#[inline]
pub fn shown(b: usize) -> (usize, usize) {
    let s = unsafe { *(SPAN as *const (u16, u16)).add(b) };
    (s.0 as usize, s.1 as usize)
}

// ---- the quick row's permissions circle (QUICK.md) ----

/// The pattern's reach in its circle: the beads' outer edge at this share of the circle's inner radius (inside the vessel's rim), three quarters.
const PATTERN_SHARE: f32 = 1. - 1. / (1 << 2) as f32;
/// An unlit slot's outline in the pattern: its width a quarter of the bead's radius, at an eighth of the ink's opacity.
const OUTLINE_LOG2: u32 = 2;
const FAINT_LOG2: u32 = 3;

/// The indicators drawn small into a quick circle about (x, y) inside inner radius `ri`, front to back by `under` into the circle's sprite: the lens a black disc in the middle; the camera's disc under it (its colour lit or missed, else the ink faint, so the lens wears its trim either way); and the beads where the real ones stand, at the same angles and relative sizes, each lit or missed in its colour, else a faint outline of the ink; nothing for the spare.
pub fn pattern(x: f32, y: f32, ri: f32, ink: u64, width: usize, height: usize, sprite: &mut [u64], at: &dyn Fn(i64, i64) -> Option<usize>) {
    use fluor::pixel::{Blend, BlendMode};
    let b = geometry(width, height);
    let bits = bits();
    let (hx, hy, hr) = ferros_sys::cutout_hole();
    let k = ri * PATTERN_SHARE / (b.ring + b.bead);
    let faint = 1. / (1 << FAINT_LOG2) as f32;
    let black = g2t_colour((0., 0., 0.));
    // One disc (or an outline `w` wide) of radius `r` at (cx, cy), at opacity `op`.
    let mut disc = |cx: f32, cy: f32, r: f32, w: Option<f32>, c: u64, op: f32| {
        let re = r + REACH;
        for py in libm::floorf(cy - re) as i64..libm::ceilf(cy + re) as i64 {
            for px in libm::floorf(cx - re) as i64..libm::ceilf(cx + re) as i64 {
                let Some(i) = at(px, py) else { continue };
                let (dx, dy) = (px as f32 + 0.5 - cx, py as f32 + 0.5 - cy);
                let d = libm::sqrtf(dx * dx + dy * dy);
                let inner = w.map_or(1., |w| draw::bump_step(d - (r - w) + 0.5));
                let cov = draw::bump_step(r - d + 0.5) * inner * op;
                if cov > 0. {
                    sprite[i] = sprite[i].under(c | ((cov * 65535.) as u64) << 48, BlendMode::Normal);
                }
            }
        }
    };
    disc(x, y, hr * k, None, black, 1.);
    for ((cx, cy, r), bit) in discs(&b) {
        let (px, py, pr) = (x + (cx - hx) * k, y + (cy - hy) * k, r * k);
        match opacity(bit, bits) {
            Some(op) => disc(px, py, pr, None, g2t_colour(colour(bit)), op),
            None if bit == uses::CAMERA => disc(px, py, pr, None, ink, faint),
            None => disc(px, py, pr, Some(pr * (1. / (1 << OUTLINE_LOG2) as f32)), ink, faint),
        }
    }
}
