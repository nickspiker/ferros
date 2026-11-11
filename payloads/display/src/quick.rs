//! The quick row (docs/QUICK.md; owner, 2026-11-11: "gesture ideas, would be nice to have the torch, brightness and maybe a few other things", and on the proposal "Works"; then "any tap in that area or drag down should bring up said second row of consoleness, however we want to do that, some circle fitting goodness I suspect").
//!
//! - **Where.** Six circles inside the app's circle, under the camera, below the top row of console circles that sit outside it: each tangent to the app's circle from inside, to a horizontal floor from above and to its neighbours, the outer pair to the glass's left and right edges, the floor as low as that takes (owner, 2026-11-11, on the Pro: "Let's make the circles always touch the left/right sides so move the bottom horizontal guide down until they scale up and touch"). The fit `corner_layout` makes in the corners, turned inside out (`layout`). Until then the floor was the console's last row and the row stopped short of the sides.
//! - **Collisions, by construction.** The console's corner circles are outside the app's circle and these inside it, both tangent to it, so they never overlap (their drawn rims, two and a half pixels, can meet, and the console's region redraw composes both). The camera hole is above the app's circle; the middle pair clear it by 56 pixels on husky, 41 on akita.
//! - **How it opens.** The map owns the finger and the physics and sends the pull `p` (op 56): 0 shut, 1 open, any value between or past. The circles are drawn at `ease(p)`, C∞ in `p` and exactly 0 or 1 past either end, so the finger is followed one to one thru the middle and the row never kinks at its ends. At level `o` every circle has `o` of its radius and `o` of its angle about the app circle's centre, still tangent to that circle from inside: they grow out of the point under the camera and fan out along the arc (`at`).
//! - **What draws it.** The console's worker, in the console's own buffer and its region machinery (`console_draw`): the quick circles are console circles that move.
//! - **Where a finger lands.** Op 41 answers which quick circle (32 + its index, with its centre and radius), the doorway (10), the top's other system ground (11), or behind the row while it shows (12).

use super::*;
use core::sync::atomic::AtomicBool;

/// The row's circles, left to right: the outer pair the smallest (the room under the arc thins toward the sides), the middle pair the two rings (owner: "brightness as a ring you turn around a circle", "torch (tap on/off, ring for intensity)").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    RadiosOff,
    Sound,
    Brightness,
    Torch,
    Dnd,
    Screencap,
}
pub const N: usize = 6;
pub const ITEMS: [Item; N] = [Item::RadiosOff, Item::Sound, Item::Brightness, Item::Torch, Item::Dnd, Item::Screencap];

/// Half the width of the ends of `ease`, in the pull's own unit: past `1 + EASE_HALF` the row is exactly open, short of `-EASE_HALF` exactly shut.
const EASE_HALF: f32 = 1. / (1 << 3) as f32;

/// The pull `p` as the row's level: `p` itself thru the middle, held to exactly 0 below and 1 above, C∞ everywhere (no clamp: GUI continuity). A C∞ step s (the bump against its mirror, `draw::bump_step`) of half-width EASE_HALF about 0 turns `p` into q = p·s(p), the ramp with its corner smoothed; the same at 1 from the other side. Its overshoot past either end is under a sixteenth of EASE_HALF (a 128th of the pull: under a pixel of the largest radius, inside the floor's slack), and only while it moves. The map's twin is payloads/map quick.rs `ease`.
pub fn ease(p: f32) -> f32 {
    let s = |t: f32| draw::bump_step((t + EASE_HALF) * (1. / (2. * EASE_HALF)));
    let q = p * s(p);
    1. - (1. - q) * s(1. - q)
}

/// The row at rest, open: (x, y, r) a circle, left to right, solved once per screen size; and the doorway's radius about the camera hole, its own cache (it needs the corners, which the row does not).
static mut LAYOUT: [(f32, f32, f32); N] = [(0., 0., 0.); N];
static mut LAYOUT_FOR: (usize, usize) = (0, 0);
static mut DOOR: f32 = 0.;
static mut DOOR_FOR: (usize, usize) = (0, 0);

/// Bisection steps: each halves a bracket, and the widest here (half the screen) comes down under a 64th of a pixel in 16.
const STEPS: u32 = 1 << 4;
/// The chain's own roots (`chain`) bisect brackets of at most half the width, to under a millionth of a pixel.
const FINE: u32 = 1 << 5;

/// One side's chain under a floor (owner, 2026-11-11: "move the bottom horizontal guide down until they scale up and touch"): three circles from the split line x = `m` outward (`dir` +1 right, −1 left), each tangent to the app's circle from inside (`inner`, its rim taken off as the corners take it) and to the floor `floor` from above, the first also to the split line (its twin on the other side touches it there), each next to the one before. A circle's centre is on the lens's middle line, where the arc's room and the floor's are equal (`medial`), and the chain steps out along it.
fn chain(app: (f32, f32, f32), floor: f32, m: f32, dir: f32, reach: f32) -> [(f32, f32, f32); N / 2] {
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
    let mut out = [(0f32, 0f32, 0f32); N / 2];
    let u = root(&|u: f32| { let (x, y) = medial(m + dir * u); u - clear(x, y) }, 0., reach, FINE);
    let (x, y) = medial(m + dir * u);
    out[0] = (x, y, clear(x, y));
    for k in 1..N / 2 {
        let p = out[k - 1];
        let u = root(&|u: f32| { let (x, y) = medial(p.0 + dir * u); libm::sqrtf((x - p.0) * (x - p.0) + (y - p.1) * (y - p.1)) - p.2 - clear(x, y) }, 0., (1 << 2) as f32 * p.2, FINE);
        let (x, y) = medial(p.0 + dir * u);
        out[k] = (x, y, clear(x, y));
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
/// - Each side is a chain under the floor (`chain`): tangent to the app's arc from inside, to the floor, to its neighbour.
/// - The outer pair are tangent to the glass's own left and right edges, exactly as `corner_layout` measures the glass (`GlassOutline`, its hairline and the circle's rim taken off): the corners are rounded and not alike (the big one top left), so the walls are not mirror images.
/// - Two unknowns, two equations, solved by bisection, never stepped on a timer: for a floor, the split between the middle pair moves until both outer circles are as near their walls (moving it right brings the right wall nearer and takes the left away); then the floor moves down until that nearness is nought (a lower floor grows every circle, so the chain reaches further out). On husky the split lands a pixel right of centre, on akita six (its fat corner is rounder).
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
    let ends = |floor: f32, m: f32| (wall(chain(app, floor, m, -1., reach)[N / 2 - 1]), wall(chain(app, floor, m, 1., reach)[N / 2 - 1]));
    // For a floor: the split where both outer circles are as near their walls, and how near.
    let split = |floor: f32| -> (f32, f32) {
        let m = root(&|m: f32| { let (l, r) = ends(floor, m); l - r }, cx - w * 0.125, cx + w * 0.125, STEPS);
        (m, ends(floor, m).1)
    };
    // The floor between a 32nd of the width under the app circle's top (circles far short of the walls) and half the width under it (far past touching on any glass like these).
    let top = app.1 - app.2;
    let floor = root(&|f: f32| -split(f).1, top + w * (1. / (1 << 5) as f32), top + reach, STEPS);
    let m = split(floor).0;
    let (left, right) = (chain(app, floor, m, -1., reach), chain(app, floor, m, 1., reach));
    let mut out = [(0f32, 0f32, 0f32); N];
    for k in 0..N / 2 {
        out[N / 2 + k] = right[k];
        out[N / 2 - 1 - k] = left[k];
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

/// The doorway's radius: the disc about the camera hole out to the nearest of the top's console circles (outside the app's circle, the open spot NFC left).
fn door(width: usize, height: usize) -> f32 {
    unsafe {
        if DOOR_FOR == (width, height) {
            return DOOR;
        }
    }
    let (hx, hy, _) = ferros_sys::cutout_hole();
    let (corners, n) = corner_layout(width, height);
    let d = corners[..n].iter().filter(|c| c.1 < height as f32 * 0.5).map(|c| libm::sqrtf((c.0 - hx) * (c.0 - hx) + (c.1 - hy) * (c.1 - hy)) - c.2).fold(f32::MAX, f32::min);
    let mut l = Log { grants: unsafe { PANIC_GRANTS }, buf: [0; 256], n: 0 };
    l.s(b"display: quick row doorway ");
    l.dec(d as u64);
    l.s(b"\n");
    l.flush();
    unsafe {
        DOOR = d;
        DOOR_FOR = (width, height);
    }
    d
}


/// The row at level `o` (0 shut, 1 open): each circle `o` of its radius, its centre `o` of its angle about the app's circle's centre and still tangent to that circle from inside, so the row grows out of the point under the camera and fans out along the arc; at 1 exactly `layout`. Neighbours never overlap on the way: their centres are further apart than at rest for the radii they have.
pub fn at(width: usize, height: usize, o: f32) -> [(f32, f32, f32); N] {
    let (cx, cy, rr) = circle(width, height);
    layout(width, height).map(|(x, y, r)| {
        let th = libm::atan2f(x - cx, cy - y) * o;
        let rad = r * o;
        let d = rr - CIRCLE_RIM - rad;
        (cx + d * libm::sinf(th), cy - d * libm::cosf(th), rad)
    })
}

/// The row as the map last set it (op 56), in two words so the receive thread stores it and the console's worker reads it without a lock (a draw that reads one word new and the other old is put right by the next):
/// - `STATE_A`: the pull (i32, Q16) in bits 0-31; flags in 32-39 (bit 0 the torch answers, bit 1 a screencap once the row is shut); the torch's level in 40-47; the quick circle a finger holds in 48-55 (G#FF none); the radio circle a finger presses in 56-63 (G#FF none).
/// - `STATE_B`: the brightness ring's lit share (u16 Q16, G#FFFF: from preference 3) in bits 0-15, the torch ring's (G#FFFF: from its level) in 16-31, the press's share in 32-47.
pub static STATE_A: AtomicU64 = AtomicU64::new(0xFFFF << 48);
pub static STATE_B: AtomicU64 = AtomicU64::new(0xFFFF_FFFF);
/// A screencap asked for once the row is shut (op 56 flag bit 1): the render thread takes it as soon as the bars on the screen hold no quick circle (`EMPTY_FRONT`).
pub static CAPTURE: AtomicBool = AtomicBool::new(false);
/// Whether the console's last draw had no quick circle in it (the worker sets it with its result), and whether the bars on the screen have none (the render thread's, at the swap).
pub static EMPTY_BACK: AtomicBool = AtomicBool::new(true);
pub static mut EMPTY_FRONT: bool = true;

/// The row's state now (`STATE_A`, `STATE_B`).
#[derive(Clone, Copy)]
pub struct Now {
    /// The level, `ease` of the pull.
    pub o: f32,
    /// The torch's level, None when the map found no torch to call.
    pub torch: Option<u8>,
    /// The rings' lit shares while a finger turns them (else from preference 3 and the torch's level).
    pub bright_arc: Option<f32>,
    pub torch_arc: Option<f32>,
    /// The quick circle a finger holds (shown lighter), and the radio circle a finger presses with how far the press has come (its ring).
    pub held: Option<usize>,
    pub press: Option<(usize, f32)>,
}

pub fn state() -> Now {
    let (a, b) = (STATE_A.load(Ordering::Acquire), STATE_B.load(Ordering::Acquire));
    let q16 = |v: u64| (v & 0xFFFF) as f32 * (1. / 65535.);
    let idx = |v: u64| if v & 0xFF == 0xFF { None } else { Some((v & 0xFF) as usize) };
    Now {
        o: ease((a as u32 as i32) as f32 * (1. / 65536.)),
        torch: (a >> 32 & 1 == 1).then_some((a >> 40) as u8),
        bright_arc: (b & 0xFFFF != 0xFFFF).then(|| q16(b)),
        torch_arc: (b >> 16 & 0xFFFF != 0xFFFF).then(|| q16(b >> 16)),
        held: idx(a >> 48),
        press: idx(a >> 56).map(|r| (r, q16(b >> 32))),
    }
}

/// Op 56 `quick [pull i32 Q16][flags u8][torch level u8][held u8][press radio u8][brightness ring u16 Q16][torch ring u16 Q16][press u16 Q16]` (15 bytes): the map's state of the row, stored for the console's worker, which is nudged to draw it. Answered at once by the receive thread.
pub fn set(m: &[u8]) {
    let w16 = |i: usize| u16::from_le_bytes([m[i], m[i + 1]]) as u64;
    let pull = u32::from_le_bytes([m[1], m[2], m[3], m[4]]) as u64;
    STATE_A.store(pull | (m[5] as u64) << 32 | (m[6] as u64) << 40 | (m[7] as u64) << 48 | (m[8] as u64) << 56, Ordering::Release);
    STATE_B.store(w16(9) | w16(11) << 16 | w16(13) << 32, Ordering::Release);
    if m[5] & 2 != 0 {
        CAPTURE.store(true, Ordering::Release);
    }
    CON_NUDGE.fetch_add(1, Ordering::Release);
    ferros_sys::wake(&CON_NUDGE, 1);
}

/// The quick circle under a point at the row's level now: its index and (x, y, r). None while the row is shut.
pub fn hit(x: f32, y: f32, width: usize, height: usize) -> Option<(usize, (f32, f32, f32))> {
    let o = state().o;
    if o <= 0. {
        return None;
    }
    at(width, height, o).iter().enumerate().find(|(_, c)| (x - c.0) * (x - c.0) + (y - c.1) * (y - c.1) <= c.2 * c.2).map(|(k, &c)| (k, c))
}

/// Whether a point is in the doorway: the disc about the camera hole out to the nearest top console circle, outside the app's circle.
pub fn doorway(x: f32, y: f32, width: usize, height: usize) -> bool {
    let (hx, hy, _) = ferros_sys::cutout_hole();
    let door = door(width, height);
    (x - hx) * (x - hx) + (y - hy) * (y - hy) < door * door && !in_circle(x as i64, y as i64, width, height)
}

/// Whether the row shows now (op 41's "behind"): its largest circle at least half a pixel, as the console draws it. A pull that never left the ease's first stretch (a finger resting on the top's ground) shows nothing, and a touch behind it is the app's.
pub fn showing(width: usize, height: usize) -> bool {
    state().o * layout(width, height)[N / 2].2 >= 0.5
}

/// A ring's band, as shares of the circle's inner radius (inside the vessel's rim): outer and inner edge. The word of a circle with a ring stays inside `RING_INNER` of its radius.
const RING_BAND: (f32, f32) = (0.97, 0.84);
pub const RING_INNER: f32 = 0.78;
/// The ring's light, as the ink's opacity: the lit share and the rest (the calendar's, `CAL_SWEPT`, `CAL_REST`).
const RING_LIT: f32 = CAL_SWEPT;
const RING_REST: f32 = CAL_REST;

/// A ring about (x, y) inside inner radius `ri`, `share` of it lit clockwise from twelve o'clock (a quick circle's value, or a radio's press toward its page): the band's edges and the lit arc's two ends each the C∞ step across a pixel, as the clock's calendar rings are drawn, so the lit end moves smoothly with the share, never a pixel at a time. Front to back by `under` into the console.
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
