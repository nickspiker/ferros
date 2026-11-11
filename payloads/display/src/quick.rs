//! The quick row (docs/QUICK.md; owner, 2026-11-11: "gesture ideas, would be nice to have the torch, brightness and maybe a few other things", and on the proposal "Works"; then "any tap in that area or drag down should bring up said second row of consoleness, however we want to do that, some circle fitting goodness I suspect").
//!
//! - **Where.** Seven circles inside the app's circle (six until 2026-11-11, when full screen came in from the home swipe as the middle one), under the camera, below the top row of console circles that sit outside it: each tangent to the app's circle from inside, to a horizontal floor from above and to its neighbours, the outer pair to the glass's left and right edges, the floor as low as that takes (owner, 2026-11-11, on the Pro: "Let's make the circles always touch the left/right sides so move the bottom horizontal guide down until they scale up and touch"). The fit `corner_layout` makes in the corners, turned inside out (`layout`). Until then the floor was the console's last row and the row stopped short of the sides.
//! - **Collisions, by construction.** The console's corner circles are outside the app's circle and these inside it, both tangent to it, so they never overlap (their drawn rims, two and a half pixels, can meet, and the console's region redraw composes both). The camera hole is above the app's circle; the middle pair clear it by 56 pixels on husky, 41 on akita.
//! - **How it opens: a slide** (owner, 2026-11-11: "track the finger down from top tho and slide it in so it doesn't need a re-render, just a move and composite, then do the iterate/2 each frame, like we do for the settings page until it snaps in or out of place"). The map owns the finger and the stepping and sends the row's offset (op 56): 0 hidden, `SLIDE` open, whole pixels. The row is drawn once at its size into its own sprites; a frame of the slide is the sprites composed under the console's circles at the offset, clipped to the app's circle so the row comes out from behind its top. The grow-and-fan animation of the first cut (C∞ in the pull) is gone: the owner waived it ("Simply appearing and disappearing is fine too. no need for a scale in/out animation").
//! - **What draws it.** The console's worker: each circle into its sprite only when its look changes (`update`, `render`), and the composite into the bars' decoded rows (`compose`, from `bar_decode`).
//! - **Where a finger lands.** Op 41 answers which quick circle (32 + its index, with its centre and radius) while the row is all the way open, the doorway (10), the top's other system ground (11), or behind the row while any of it shows (12): one read of the hit map (hit.rs), whose bars' ids the console's worker writes as it composes the row (`open_circles`, `doorway_disc`).

use super::*;
use core::sync::atomic::AtomicBool;

/// The row's circles, left to right: the outer pair the smallest (the room under the arc thins toward the sides), the two rings either side of the middle (owner: "brightness as a ring you turn around a circle", "torch (tap on/off, ring for intensity)"), and full screen the middle one, the largest (owner, 2026-11-11: full screen moves from the home swipe "to a quick-row circle"; it switches the app in front, dimmed when there is none).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    RadiosOff,
    Sound,
    Brightness,
    FullScreen,
    Torch,
    Dnd,
    Screencap,
}
pub const N: usize = 7;
pub const ITEMS: [Item; N] = [Item::RadiosOff, Item::Sound, Item::Brightness, Item::FullScreen, Item::Torch, Item::Dnd, Item::Screencap];
/// Each side's chain, out from the middle circle.
const SIDE: usize = N / 2;

/// The row at rest, open: (x, y, r) a circle, left to right, solved once per screen size.
static mut LAYOUT: [(f32, f32, f32); N] = [(0., 0., 0.); N];
static mut LAYOUT_FOR: (usize, usize) = (0, 0);

/// Bisection steps: each halves a bracket, and the widest here (half the screen) comes down under a 64th of a pixel in 16.
const STEPS: u32 = 1 << 4;
/// The chain's own roots (`chain`) bisect brackets of at most half the width, to under a millionth of a pixel.
const FINE: u32 = 1 << 5;

/// The middle circle under a floor, centred on the split line x = `m`: tangent to the app's circle from inside (its rim taken off) and to the floor from above, on the lens's middle line as every circle of the row (`chain`).
fn middle(app: (f32, f32, f32), floor: f32, m: f32) -> (f32, f32, f32) {
    let (cx, cy, rr) = app;
    let (dx, cyd, k) = ((m - cx) as f64, cy as f64, (rr - CIRCLE_RIM - floor) as f64);
    let y = ((dx * dx + cyd * cyd - k * k) / (2. * (cyd + k))) as f32;
    (m, y, floor - y)
}

/// One side's chain under a floor (owner, 2026-11-11: "move the bottom horizontal guide down until they scale up and touch"): three circles out from the middle circle `mid` (`dir` +1 right, −1 left), each tangent to the app's circle from inside (`inner`, its rim taken off as the corners take it) and to the floor `floor` from above, the first also to the middle circle (its twin on the other side touches it too), each next to the one before. A circle's centre is on the lens's middle line, where the arc's room and the floor's are equal (`medial`), and the chain steps out along it. Until the seventh circle (2026-11-11) the first touched the split line itself.
fn chain(app: (f32, f32, f32), floor: f32, mid: (f32, f32, f32), dir: f32) -> [(f32, f32, f32); SIDE] {
    let (cx, cy, rr) = app;
    let inner = |x: f32, y: f32| rr - CIRCLE_RIM - libm::sqrtf((x - cx) * (x - cx) + (y - cy) * (y - cy));
    let clear = |x: f32, y: f32| {
        let (a, b) = (inner(x, y), floor - y);
        if a < b { a } else { b }
    };
    // The point at column s as far from the arc as from the floor, in closed form: R' − √(dx² + (y − cy)²) = floor − y with R' the arc's radius less the rim, so with k = R' − floor, √(dx² + (y − cy)²) = y + k, and squared y = (dx² + cy² − k²) / 2(cy + k) (cy + k is the circle's bottom less the floor, positive). In f64: cy² is millions of square pixels, and f32 would keep a quarter of one.
    let medial = |s: f32| -> (f32, f32) {
        let (dx, cy, k) = ((s - cx) as f64, cy as f64, (rr - CIRCLE_RIM - floor) as f64);
        (s, ((dx * dx + cy * cy - k * k) / (2. * (cy + k))) as f32)
    };
    let mut out = [(0f32, 0f32, 0f32); SIDE];
    let mut p = mid;
    for c in out.iter_mut() {
        let u = root(&|u: f32| { let (x, y) = medial(p.0 + dir * u); libm::sqrtf((x - p.0) * (x - p.0) + (y - p.1) * (y - p.1)) - p.2 - clear(x, y) }, 0., (1 << 2) as f32 * p.2, FINE);
        let (x, y) = medial(p.0 + dir * u);
        *c = (x, y, clear(x, y));
        p = *c;
    }
    out
}

/// The root of an increasing `g` between `lo` (below it) and `hi` (above it), `steps` bisections.
fn root(g: &dyn Fn(f32) -> f32, mut lo: f32, mut hi: f32, steps: u32) -> f32 {
    for _ in 0..steps {
        let mid = (lo + hi) * 0.5;
        if g(mid) < 0. { lo = mid } else { hi = mid }
    }
    hi
}

/// The row's circles at rest, open (`LAYOUT`), solved (owner, 2026-11-11, on the Pro: "Let's make the circles always touch the left/right sides so move the bottom horizontal guide down until they scale up and touch"):
/// - The middle circle (full screen) is centred on the split, tangent to the arc and the floor (`middle`); each side is a chain out from it under the floor (`chain`): tangent to the app's arc from inside, to the floor, to its neighbour. Centred under the camera, the middle circle's top meets the app circle's top, where the camera hole touches the circle from outside; the compose clips the row to the app's circle (`compose`), so none of its rim reaches the hole.
/// - The outer pair are tangent to the glass's own left and right edges, exactly as `corner_layout` measures the glass (`GlassOutline`, its hairline and the circle's rim taken off): the corners are rounded and not alike (the big one top left), so the walls are not mirror images.
/// - Two unknowns, two equations, solved by bisection, never stepped on a timer: for a floor, the split (the middle circle's centre) moves until both outer circles are as near their walls (moving it right brings the right wall nearer and takes the left away); then the floor moves down until that nearness is nought (a lower floor grows every circle, so the chain reaches further out). On husky the split lands a pixel right of centre, on akita six (its fat corner is rounder).
/// - Lower floor, bigger circles: on husky the middle pair reach row 413, past the console's old last row (288): the console's rows reach the row's floor since 2026-11-11 (`rows`).
pub fn layout(width: usize, height: usize) -> [(f32, f32, f32); N] {
    unsafe {
        if LAYOUT_FOR == (width, height) {
            return LAYOUT;
        }
    }
    let t0 = ferros_sys::now();
    let app = circle(width, height);
    let (w, cx) = (width as f32, app.0);
    let outline = GlassOutline::new(width, height);
    // A circle's room to the glass, as the corners have it (`corner_layout`'s `glass`).
    let wall = |c: (f32, f32, f32)| outline.dist(c.0, c.1) - EDGE_HAIRLINE - CIRCLE_RIM - c.2;
    let reach = w * 0.5;
    let ends = |floor: f32, m: f32| {
        let mid = middle(app, floor, m);
        (wall(chain(app, floor, mid, -1.)[SIDE - 1]), wall(chain(app, floor, mid, 1.)[SIDE - 1]))
    };
    // For a floor: the split where both outer circles are as near their walls, and how near.
    let split = |floor: f32| -> (f32, f32) {
        let m = root(&|m: f32| { let (l, r) = ends(floor, m); l - r }, cx - w * 0.125, cx + w * 0.125, STEPS);
        (m, ends(floor, m).1)
    };
    // The floor between a 32nd of the width under the app circle's top (circles far short of the walls) and half the width under it (far past touching on any glass like these).
    let top = app.1 - app.2;
    let floor = root(&|f: f32| -split(f).1, top + w * (1. / (1 << 5) as f32), top + reach, STEPS);
    let m = split(floor).0;
    let mid = middle(app, floor, m);
    let (left, right) = (chain(app, floor, mid, -1.), chain(app, floor, mid, 1.));
    let mut out = [(0f32, 0f32, 0f32); N];
    out[SIDE] = mid;
    for k in 0..SIDE {
        out[SIDE + 1 + k] = right[k];
        out[SIDE - 1 - k] = left[k];
    }
    // The solved row, once per size, to the console: (x, y, r) in pixels, the floor, and the time the solve took.
    let mut l = Log { grants: unsafe { PANIC_GRANTS }, buf: [0; 256], n: 0 };
    l.s(b"display: quick row");
    for c in &out {
        l.s(b" (");
        l.dec(c.0 as u64);
        l.s(b",");
        l.dec(c.1 as u64);
        l.s(b",");
        l.dec(c.2 as u64);
        l.s(b")");
    }
    l.s(b", floor ");
    l.dec(floor as u64);
    l.s(b", walls ");
    l.dec((wall(out[0]) * 100.) as u64);
    l.s(b"/");
    l.dec((wall(out[N - 1]) * 100.) as u64);
    l.s(b" hundredths, solved in ");
    l.dec(ferros_sys::us_since(t0));
    l.s(b" us\n");
    l.flush();
    unsafe {
        LAYOUT = out;
        LAYOUT_FOR = (width, height);
    }
    out
}

/// The console's rows (the bars' top rows): the app circle's chrome rows (`chrome_rows`), or down to the quick row's floor and the reach its circles draw past their radius (the console's REACH, two and a half pixels, and a row of slack), whichever is lower. Each row is three rows of the bars' buffers (`BAR`, `BAR_W`, `BAR_W_BACK`), 8 bytes a pixel: on husky the floor at 413 costs 129 rows, 4.2 MiB of the display heap.
pub fn rows(width: usize, height: usize, chrome_top: usize) -> usize {
    let floor = layout(width, height).iter().map(|c| c.1 + c.2).fold(0., f32::max);
    let need = floor as usize + (1 << 2);
    if need > chrome_top { need } else { chrome_top }
}


// ---- the slide (owner, 2026-11-11, on the Pro: "the top console pulldown is stupidly slow and jerky"; then "track the finger down from top tho and slide it in so it doesn't need a re-render, just a move and composite, then do the iterate/2 each frame, like we do for the settings page until it snaps in or out of place") ----
//
// Until then the row grew out of the camera point, every circle drawn again at a new size each frame (vessel, rings, words: up to 9.5 ms a console draw, over a 120 Hz frame). Now each circle is drawn once at its size into its own sprite (`render`), and the row only moves: its offset `off` (0 hidden, `slide` open) shifts every sprite down by whole pixels, and the console's worker composes the shifted sprites under the console's own circles into the bars' decoded rows (`compose`, from `bar_decode`), clipped to the app's circle, so the row comes out from behind the top of it. Per frame: no drawing, one band of rows composed and published.

/// The row's offset as the map last sent it (op 56), whole pixels: 0 hidden, `slide` open. The map follows the finger and halves the rest each frame after the lift.
pub static OFFSET: AtomicU64 = AtomicU64::new(0);
/// The slide's length (`slide`), for op 56's reply: the map's travel.
pub static SLIDE: AtomicU64 = AtomicU64::new(0);

/// The row as the map last set it (op 56), in two words the receive thread stores and the console's worker reads without a lock (a pass that reads one new and the other old is put right by the next):
/// - `STATE_A`: flags in bits 32-39 (bit 0 the torch answers, bit 1 a screencap once the row is hidden); the torch's level in 40-47; the quick circle a finger holds in 48-55 (G#FF none); bits 56-63 unused (the radio press ring, drawn until 2026-11-11, when the long press went).
/// - `STATE_B`: the brightness ring's lit share (u16 Q16, G#FFFF: from preference 3) in bits 0-15, the torch ring's (G#FFFF: from its level) in 16-31.
pub static STATE_A: AtomicU64 = AtomicU64::new(0xFFFF << 48);
pub static STATE_B: AtomicU64 = AtomicU64::new(0xFFFF_FFFF);
/// A screencap asked for once the row is hidden (op 56 flag bit 1): the render thread takes it as soon as the bars on the screen hold none of the row (`EMPTY_FRONT`).
pub static CAPTURE: AtomicBool = AtomicBool::new(false);
/// Whether the bars the worker last decoded hold none of the row (offset 0), and whether those on the screen do (the render thread's, at the swap).
pub static EMPTY_BACK: AtomicBool = AtomicBool::new(true);
pub static mut EMPTY_FRONT: bool = true;

/// The row's state now (`STATE_A`, `STATE_B`, `OFFSET`).
#[derive(Clone, Copy)]
pub struct Now {
    pub off: i64,
    /// The torch's level, None when the map found no torch to call.
    pub torch: Option<u8>,
    /// The rings' lit shares while a finger turns them (else from preference 3 and the torch's level).
    pub bright_arc: Option<f32>,
    pub torch_arc: Option<f32>,
    /// The quick circle a finger holds (shown lighter).
    pub held: Option<usize>,
}

pub fn state() -> Now {
    let (a, b) = (STATE_A.load(Ordering::Acquire), STATE_B.load(Ordering::Acquire));
    let q16 = |v: u64| (v & 0xFFFF) as f32 * (1. / 65535.);
    let idx = |v: u64| if v & 0xFF == 0xFF { None } else { Some((v & 0xFF) as usize) };
    Now {
        off: OFFSET.load(Ordering::Acquire) as i64,
        torch: (a >> 32 & 1 == 1).then_some((a >> 40) as u8),
        bright_arc: (b & 0xFFFF != 0xFFFF).then(|| q16(b)),
        torch_arc: (b >> 16 & 0xFFFF != 0xFFFF).then(|| q16(b >> 16)),
        held: idx(a >> 48),
    }
}

/// Op 56 `quick [offset i32][flags u8][torch level u8][held u8][unused u8][brightness ring u16 Q16][torch ring u16 Q16][unused u16]` (15 bytes): the map's state of the row, stored for the console's worker, which is nudged. Answered at once by the receive thread with [0][the slide's length u16], so the map knows how far the row travels.
pub fn set(m: &[u8]) -> [u8; 3] {
    let w16 = |i: usize| u16::from_le_bytes([m[i], m[i + 1]]) as u64;
    let off = i32::from_le_bytes([m[1], m[2], m[3], m[4]]);
    // WHY the bounds: the offset is the map's, and the bars hold the row's rows only between hidden and open (the row's band and the bars' rows end at its floor); past either end there is nothing to compose.
    let slide = SLIDE.load(Ordering::Acquire) as i32;
    let off = if off < 0 { 0 } else if off > slide { slide } else { off };
    OFFSET.store(off as u64, Ordering::Release);
    STATE_A.store((m[5] as u64) << 32 | (m[6] as u64) << 40 | (m[7] as u64) << 48 | (m[8] as u64) << 56, Ordering::Release);
    STATE_B.store(w16(9) | w16(11) << 16, Ordering::Release);
    if m[5] & 2 != 0 {
        CAPTURE.store(true, Ordering::Release);
    }
    CON_NUDGE.fetch_add(1, Ordering::Release);
    ferros_sys::wake(&CON_NUDGE, 1);
    let s = (slide as u16).to_le_bytes();
    [0, s[0], s[1]]
}

/// The row's circles as the bars hold them (`compose`, at the offset the worker took) when it stands all the way open; None while it is hidden or slides (a finger on it is then the slide's). The hit map's quick circles (hit.rs `bars_row`), at the worker's compose step.
pub fn open_circles(width: usize, height: usize) -> Option<[(f32, f32, f32); N]> {
    let slide = SLIDE.load(Ordering::Acquire) as i64;
    (slide > 0 && unsafe { COMPOSED } == slide).then(|| layout(width, height))
}

/// One circle's sprite: every pixel the circle draws (out to its reach past the radius), row by row from `y0`, each row a span of columns [x0, x1) at `at` in `data`. Drawn in fluor's α + darkness, then decoded in place to working values (`working`), as the bars are, so a frame of the slide only copies and folds.
struct Sprite {
    y0: i64,
    span: &'static [(u16, u16, u32)],
    data: &'static mut [u64],
    /// The look it was drawn with (`look`); 0 not drawn.
    key: u64,
}

static mut SPRITES: [Option<Sprite>; N] = [const { None }; N];
/// The rows the seven sprites cover at rest, open, [top, bottom): the band the slide moves.
static mut BAND: (i64, i64) = (0, 0);
/// The offset the bars were last composed at.
static mut COMPOSED: i64 = -1;

/// A circle's drawing reaches past its radius: the vessel's table runs to r + 2, its rim antialiased half a pixel more (the console's REACH), and a pixel of slack.
const SPRITE_REACH: f32 = 3.;

/// The slide's length: from hidden (every pixel of the row above the app circle's top, so clipped away) to open, whole pixels.
fn slide_len(width: usize, height: usize) -> i64 {
    let (_, cy, rr) = circle(width, height);
    unsafe { BAND.1 - libm::floorf(cy - rr) as i64 }
}

/// The sprites, made once at start (before the heap's mark, kept for the process): each circle's rows and spans from the layout, its data zeroed (drawn at the first `update`). On husky about 265 thousand pixels, 2.1 MiB.
pub fn init(width: usize, height: usize) {
    let lay = layout(width, height);
    let (mut top, mut bot) = (i64::MAX, i64::MIN);
    for (k, &(x, y, r)) in lay.iter().enumerate() {
        let re = r + SPRITE_REACH;
        let (y0, y1) = (libm::floorf(y - re) as i64, libm::ceilf(y + re) as i64);
        let mut span = alloc::vec::Vec::with_capacity((y1 - y0) as usize);
        let mut at = 0u32;
        for row in y0..y1 {
            let dy = row as f32 + 0.5 - y;
            let h2 = re * re - dy * dy;
            // WHY the bounds: a row past the disc's reach has no chord; and the outer circles touch the glass's sides, so their reach runs a few pixels past the screen's edges, which hold no columns.
            let half = if h2 > 0. { libm::sqrtf(h2) } else { 0. };
            let (a, b) = (libm::floorf(x - half), libm::ceilf(x + half));
            let x0 = if a < 0. { 0 } else { a as u16 };
            let x1 = if b > width as f32 { width as u16 } else { b as u16 };
            let x1 = if x1 < x0 { x0 } else { x1 };
            span.push((x0, x1, at));
            at += (x1 - x0) as u32;
        }
        top = top.min(y0);
        bot = bot.max(y1);
        let data = alloc::vec![0u64; at as usize].leak();
        unsafe { (*(&raw mut SPRITES))[k] = Some(Sprite { y0, span: span.leak(), data, key: 0 }) };
    }
    unsafe { BAND = (top, bot) };
    SLIDE.store(slide_len(width, height) as u64, Ordering::Release);
}

/// Working values (gamma 2, white at Q14), as the console's: the grey of a lit circle, of one that is off, and the inks.
const LIT_GREY: f32 = 9000.;
const OFF_GREY: f32 = 3200.;
const LIVE_INK: f32 = 11000.;
const LIGHT_INK: f32 = 7000.;

/// A quick circle's look now: its grey (lit while what it switches is on; a stop darker for vibrate; the off grey; a stop under that for a torch that does not answer, and for full screen with no app in front; the live ink's light under a finger), whether it is on (its ink), its ring's lit share (brightness by preference 3, 0 the brightest; the torch by its level; while a finger turns one, the share the map sent), and as one key with its word's language and the sound mode.
fn look(k: usize, s: &Now) -> (f32, bool, Option<f32>, u64) {
    let stop = core::f32::consts::FRAC_1_SQRT_2;
    let off = ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF);
    let sound = ferros_sys::pref(ferros_sys::PREF_SOUND);
    let (on, grey) = match ITEMS[k] {
        Item::Brightness => (true, LIT_GREY),
        Item::Torch => match s.torch {
            None => (false, OFF_GREY * stop),
            Some(0) => (false, OFF_GREY),
            Some(_) => (true, LIT_GREY),
        },
        Item::RadiosOff => if off & 0x1F == 0x1F { (true, LIT_GREY) } else { (false, OFF_GREY) },
        Item::Sound => match sound {
            ferros_sys::SOUND => (true, LIT_GREY),
            ferros_sys::VIBRATE => (true, LIT_GREY * stop),
            _ => (false, OFF_GREY),
        },
        Item::Dnd => if ferros_sys::dnd::on() { (true, LIT_GREY) } else { (false, OFF_GREY) },
        Item::Screencap => (false, OFF_GREY),
        // Lit while the app in front is full screen (`FULL`); dimmed with no app in front, where it switches nothing.
        Item::FullScreen => match (app_live(), unsafe { FULL }) {
            (false, _) => (false, OFF_GREY * stop),
            (true, true) => (true, LIT_GREY),
            (true, false) => (false, OFF_GREY),
        },
    };
    let grey = if s.held == Some(k) { LIVE_INK } else { grey };
    let ring = match ITEMS[k] {
        Item::Brightness => Some(s.bright_arc.unwrap_or(1. - ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS) as f32 * (1. / 255.))),
        Item::Torch => s.torch.map(|l| s.torch_arc.unwrap_or(l as f32 * (1. / 255.))),
        _ => None,
    };
    let mut key = 0x9E37_79B9_7F4A_7C15u64 ^ k as u64;
    for v in [grey.to_bits() as u64 | (on as u64) << 32, ring.map_or(u64::MAX, |f| f.to_bits() as u64), sound as u64 | (ferros_sys::pref(ferros_sys::PREF_LANG) as u64) << 8 | (ferros_sys::pref(ferros_sys::PREF_LANG + 1) as u64) << 16] {
        key = (key.rotate_left(7) ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    (grey, on, ring, key | 1)
}

/// Circle k drawn into its sprite at its size, front to back from empty: its ring, its word, then its vessel under them (the console's vessel, `orb::Vessel`, by a table over the radius); then decoded in place to working values.
/// The sprites drawn since the slide's last measurement line (main.rs `SLIDE_US`).
pub static RENDERS: AtomicU64 = AtomicU64::new(0);

fn render(k: usize, grey: f32, on: bool, ring_share: Option<f32>, width: usize, height: usize) {
    use fluor::pixel::{Blend, BlendMode};
    RENDERS.fetch_add(1, Ordering::Relaxed);
    let (x, y, r) = layout(width, height)[k];
    let Some(sp) = (unsafe { (*(&raw mut SPRITES))[k].as_mut() }) else { return };
    let (y0, span) = (sp.y0, sp.span);
    let data = &mut *sp.data;
    data.fill(0);
    // A pixel's place in the sprite. WHY the tests: the ring's box, the word's glyphs and the vessel's square reach a little past the disc the sprite holds; there is nothing of the circle there.
    let at = |px: i64, py: i64| -> Option<usize> {
        let j = py - y0;
        if j < 0 || j >= span.len() as i64 {
            return None;
        }
        let (x0, x1, o) = span[j as usize];
        if px < x0 as i64 || px >= x1 as i64 {
            return None;
        }
        Some(o as usize + (px - x0 as i64) as usize)
    };
    let ink = g2t_colour(if on { (LIVE_INK, LIVE_INK, LIVE_INK) } else { (LIGHT_INK, LIGHT_INK, LIGHT_INK) });
    if let Some(share) = ring_share {
        ring(x, y, r * (1.0 - 2.0 * VESSEL_DEPTH), share, ink, data, &at, &|b| b);
    }
    let eng = unsafe { (*(&raw mut CON_GLYPHS)).as_mut().expect("console glyphs") };
    word(k, x, y, r * if ring_share.is_some() { RING_INNER } else { 1.0 }, ink, data, &at, eng);
    // The vessel: as the console draws its circles (`console_draw`), a table over the radius by sixteenths of a pixel, so each pixel is a root and a lookup.
    let fl = [grey; 3].map(|v| { let c = v * (1.0 / 16384.0); c * c });
    let vessel = orb::Vessel::new(fl, VESSEL_DEPTH, VESSEL_LAMBDA, VESSEL_SHARP, VESSEL_SEP, 0.0, 4.0);
    let fs = 1.0 / r;
    const SUB: f32 = (1 << 4) as f32;
    let lut: alloc::vec::Vec<(u64, u64)> = (0..((r + 2.0) * SUB) as usize).map(|j| vessel.at(1.0 - (j as f32 + 0.5) * (1.0 / SUB) * fs, fs)).collect();
    for (j, &(x0, x1, o)) in span.iter().enumerate() {
        let dy = (y0 + j as i64) as f32 + 0.5 - y;
        for px in x0..x1 {
            let dx = px as f32 + 0.5 - x;
            // Past the table's end (outside r + 2) the vessel is nothing.
            let Some(&(rg, body)) = lut.get((libm::sqrtf(dx * dx + dy * dy) * SUB) as usize) else { continue };
            let i = o as usize + (px - x0) as usize;
            data[i] = data[i].under(rg, BlendMode::Normal).under(body, BlendMode::Normal);
        }
    }
    for p in data.iter_mut() {
        *p = working(*p, G2T16);
    }
}

/// The worker's pass over the row (`console_worker`): any circle whose look changed drawn again into its sprite (a tap, a ring turned, a language), and the offset taken. Returns the bars' rows owed a compose: those a changed sprite covers where it stands now, and the band where the row was and is when it moved. What it took is the next `compose`'s.
pub fn update(width: usize, height: usize) -> Option<(usize, usize)> {
    let s = state();
    let slide = SLIDE.load(Ordering::Acquire) as i64;
    let (top, bot) = unsafe { BAND };
    let mut rows: Option<(i64, i64)> = None;
    let mut owe = |a: i64, b: i64| rows = Some(rows.map_or((a, b), |(p, q)| (p.min(a), q.max(b))));
    for k in 0..N {
        let (grey, on, ring_share, key) = look(k, &s);
        let Some(sp) = (unsafe { (*(&raw mut SPRITES))[k].as_mut() }) else { continue };
        if sp.key == key {
            continue;
        }
        sp.key = key;
        let (y0, n) = (sp.y0, sp.span.len() as i64);
        render(k, grey, on, ring_share, width, height);
        if s.off > 0 {
            let d = s.off - slide;
            owe(y0 + d, y0 + n + d);
        }
    }
    let was = unsafe { COMPOSED };
    if s.off != was {
        for o in [s.off, was] {
            if o > 0 {
                owe(top + o - slide, bot + o - slide);
            }
        }
        unsafe { COMPOSED = s.off };
    }
    EMPTY_BACK.store(s.off == 0, Ordering::Release);
    // WHY the bounds: the band moved up by the slide reaches above the screen's top; only the bars' top rows exist.
    let lim = unsafe { BAR_TOP } as i64;
    rows.map(|(a, b)| ((if a < 0 { 0 } else { a }) as usize, (if b > lim { lim } else { b }) as usize)).filter(|r| r.0 < r.1)
}

/// The row composed into one decoded row of the bars, screen row `y` (`bar_decode`), under the console's circles already there: each sprite's row at the offset taken (`COMPOSED`), clipped to the app's circle (exact area coverage across its two edge columns), copied where nothing of the console is and folded under it (`under_pm`) where something is. Returns the columns written, [from, to), (0, 0) for none.
pub fn compose(y: usize, out: &mut [u64], width: usize, height: usize) -> (usize, usize) {
    let off = unsafe { COMPOSED };
    if off <= 0 {
        return (0, 0);
    }
    let d = off - SLIDE.load(Ordering::Acquire) as i64;
    let (cx, cy, rr) = circle(width, height);
    let dy = y as f32 + 0.5 - cy;
    let h2 = rr * rr - dy * dy;
    if h2 <= 0. {
        return (0, 0);
    }
    let half = libm::sqrtf(h2);
    let (cl, cr) = (cx - half, cx + half);
    let (mut lo, mut hi) = (width, 0usize);
    for sp in unsafe { (*(&raw const SPRITES)).iter().flatten() } {
        let j = y as i64 - d - sp.y0;
        if j < 0 || j >= sp.span.len() as i64 {
            continue;
        }
        let (x0, x1, o) = sp.span[j as usize];
        // The chord's whole pixels [wa, wb), and its two part ones at either end (`a` and `b - 1` when the arc crosses this sprite's span): the coverage worked out only there, never in the inner loop (2026-11-11: it was worked out at every pixel, a slide step's compose 2.8 ms on the Pro).
        let (a, b) = (if (x0 as f32) < cl { libm::floorf(cl) as usize } else { x0 as usize }, if (x1 as f32) > cr { libm::ceilf(cr) as usize } else { x1 as usize });
        if a >= b {
            continue;
        }
        let (wa, wb) = (if (a as f32) < cl { a + 1 } else { a }, if (b as f32) > cr { b - 1 } else { b });
        // This row of the sprite, indexed by screen column from x0.
        let data = &sp.data[o as usize..];
        let x0 = x0 as usize;
        let mut put = |x: usize, s: u64| {
            let f = out[x];
            out[x] = if f >> 63 != 0 { s } else { let (ch, t) = under_pm(f & ((1 << 48) - 1), trans(f), s & ((1 << 48) - 1), trans(s)); ch | t << 49 };
        };
        for x in wa..wb.max(wa) {
            let s = data[x - x0];
            if s >> 63 == 0 {
                put(x, s);
            }
        }
        // A column the arc crosses: the layer scaled by the share of the pixel inside it. WHY the bounds: coverage is the overlap of [x, x + 1] with the chord, a share by definition.
        let edges = if b - 1 == a { [a, a] } else { [a, b - 1] };
        for (k, &x) in edges.iter().enumerate() {
            if (x >= wa && x < wb) || (k == 1 && edges[1] == edges[0]) {
                continue;
            }
            let s = data[x - x0];
            let cov = { let (p, q) = (if (x as f32) > cl { x as f32 } else { cl }, if ((x + 1) as f32) < cr { (x + 1) as f32 } else { cr }); q - p };
            if s >> 63 != 0 || cov <= 0. {
                continue;
            }
            let q = (cov * 65536.) as u64;
            put(x, scale_q16(s, q) | (16384 - ((16384 - trans(s)) * q >> 16)) << 49);
        }
        lo = lo.min(a);
        hi = hi.max(b);
    }
    if lo < hi { (lo, hi) } else { (0, 0) }
}

/// A ring's band, as shares of the circle's inner radius (inside the vessel's rim): outer and inner edge. The word of a circle with a ring stays inside `RING_INNER` of its radius.
const RING_BAND: (f32, f32) = (0.97, 0.84);
pub const RING_INNER: f32 = 0.78;
/// The ring's light, as the ink's opacity: the lit share and the rest (the calendar's, `CAL_SWEPT`, `CAL_REST`).
const RING_LIT: f32 = CAL_SWEPT;
const RING_REST: f32 = CAL_REST;

/// A ring about (x, y) inside inner radius `ri`, `share` of it lit clockwise from twelve o'clock (a quick circle's value): the band's edges and the lit arc's two ends each the C∞ step across a pixel, as the clock's calendar rings are drawn, so the lit end moves smoothly with the share, never a pixel at a time. Front to back by `under` into the console.
pub fn ring(x: f32, y: f32, ri: f32, share: f32, ink: u64, console: &mut [u64], at: &dyn Fn(i64, i64) -> Option<usize>, cut: &dyn Fn((i64, i64, i64, i64)) -> (i64, i64, i64, i64)) {
    use fluor::pixel::{Blend, BlendMode};
    let tau = core::f32::consts::TAU;
    let (r1, r0) = (ri * RING_BAND.0, ri * RING_BAND.1);
    let (x0, x1, y0, y1) = cut(((x - r1 - 1.) as i64, (x + r1) as i64 + 1, (y - r1 - 1.) as i64, (y + r1) as i64 + 1));
    for py in y0..=y1 {
        for px in x0..=x1 {
            let Some(i) = at(px, py) else { continue };
            let (dx, dy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
            let d = libm::sqrtf(dx * dx + dy * dy);
            let band = draw::bump_step(r1 - d + 0.5) * draw::bump_step(d - r0 + 0.5);
            if band <= 0. {
                continue;
            }
            // The turn's share clockwise from twelve o'clock, 0 to 1, and the arc lengths in pixels at this radius past the lit part's start and short of its end.
            let a = libm::atan2f(dx, -dy);
            let f = if a < 0. { a + tau } else { a } * (1. / tau);
            let lit = draw::bump_step(f * tau * d + 0.5) * draw::bump_step((share - f) * tau * d + 0.5);
            let level = RING_LIT * lit + RING_REST * (1. - lit);
            console[i] = console[i].under(ink | ((band * level * 65535.) as u64) << 48, BlendMode::Normal);
        }
    }
}

/// A quick circle's word, in the person's language (ferros_lang), a word a line stacked about its centre, each line as large as fits seven tenths of its chord within `r`: the sound mode's word is the mode now.
pub fn word(k: usize, x: f32, y: f32, r: f32, ink: u64, console: &mut [u64], at: &dyn Fn(i64, i64) -> Option<usize>, eng: &mut glyphs::Glyphs) {
    use ferros_lang::Msg;
    use fluor::pixel::{Blend, BlendMode};
    let m = match ITEMS[k] {
        Item::Brightness => Msg::Brightness,
        Item::Torch => Msg::Torch,
        Item::RadiosOff => Msg::RadiosOff,
        Item::Sound => match ferros_sys::pref(ferros_sys::PREF_SOUND) {
            ferros_sys::SOUND => Msg::SoundOn,
            ferros_sys::VIBRATE => Msg::Vibrate,
            _ => Msg::Silent,
        },
        Item::Dnd => Msg::DoNotDisturb,
        Item::Screencap => Msg::Screencap,
        Item::FullScreen => Msg::FullScreen,
    };
    let text = ferros_lang::t(m);
    // At most four lines: every language's words for these are three words or fewer.
    let mut lines = [""; 4];
    let mut nl = 0;
    for w in text.split(' ').filter(|w| !w.is_empty()) {
        if nl < lines.len() {
            lines[nl] = w;
            nl += 1;
        }
    }
    // The lines span 1.2 r about the centre, a pitch each, so a line's far edge is at most 0.6 r out and the chord's root stays positive.
    let pitch = r * if nl == 1 { 0.5 } else { 1.2 / nl as f32 };
    let mut top = y - pitch * nl as f32 * 0.5;
    for line in &lines[..nl] {
        let far = libm::fabsf(top - y).max(libm::fabsf(top + pitch - y));
        let chord = 2. * libm::sqrtf(r * r - far * far) * 0.7;
        let mut px = pitch * 0.9;
        let tw0 = eng.width(line, px);
        if tw0 > chord {
            px *= chord / tw0;
        }
        let tw = eng.width(line, px);
        let base = top + pitch * 0.5 + px * 0.36;
        eng.cover(line, x - tw * 0.5, base, px, (1., 0.), |gx, gy, a| {
            if let Some(i) = at(gx, gy) {
                console[i] = console[i].under(ink | (a as u64) << 48, BlendMode::Normal);
            }
        });
        top += pitch;
    }
}
