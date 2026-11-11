//! The quick row (docs/QUICK.md; owner, 2026-11-11: "gesture ideas, would be nice to have the torch, brightness and maybe a few other things", and on the proposal "Works"; then "any tap in that area or drag down should bring up said second row of consoleness, however we want to do that, some circle fitting goodness I suspect").
//!
//! - **Where.** Six circles in the lens between the top of the app's circle and the first row where its chord is the screen's whole width (the console's last row): inside the app's circle, under the camera, below the top row of console circles that sit outside it. Each is tangent to the app's circle from inside and to that row from above, and to its neighbours: the fit `corner_layout` makes in the corners, turned inside out (`layout`).
//! - **How it opens.** The map owns the finger and the physics and sends the pull `p` (op 56): 0 shut, 1 open, any value between or past. The circles are drawn at `ease(p)`, C∞ in `p` and exactly 0 or 1 past either end, so the finger is followed one to one thru the middle and the row never kinks at its ends. At level `o` every circle has `o` of its radius and `o` of its angle about the app circle's centre, still tangent to that circle from inside: they grow out of the point under the camera and fan out along the arc (`at`).
//! - **What draws it.** The console's worker, in the console's own buffer and its region machinery (`console_draw`): the quick circles are console circles that move.
//! - **Where a finger lands.** Op 41 answers which quick circle (32 + its index, with its centre and radius), the doorway (10), the top's other system ground (11), or behind the row while it shows (12).

use super::*;
use core::sync::atomic::AtomicBool;

/// The row's circles, left to right: the outer pair the smallest (the lens thins toward the sides), the middle pair the two rings (owner: "brightness as a ring you turn around a circle", "torch (tap on/off, ring for intensity)").
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

/// The row at rest, open: (x, y, r) a circle, left to right, and the doorway's radius about the camera hole; solved once per screen size.
static mut LAYOUT: [(f32, f32, f32); N] = [(0., 0., 0.); N];
static mut DOOR: f32 = 0.;
static mut LAYOUT_FOR: (usize, usize) = (0, 0);

/// The row's circles at rest, open (`LAYOUT`).
pub fn layout(width: usize, height: usize) -> [(f32, f32, f32); N] {
    unsafe {
        if LAYOUT_FOR == (width, height) {
            return LAYOUT;
        }
    }
    let (cx, cy, rr) = circle(width, height);
    // The floor: a circle's drawing reaches two and a half pixels past its radius (the console's REACH), and the console holds rows up to CHROME_TOP alone, so every circle stays four rows above that.
    let floor = unsafe { CHROME_TOP } as f32 - (1 << 2) as f32;
    // A point's room inside the app's circle, the rim a console circle draws past its radius taken off as the corners take it (CIRCLE_RIM).
    let inner = |x: f32, y: f32| rr - CIRCLE_RIM - libm::sqrtf((x - cx) * (x - cx) + (y - cy) * (y - cy));
    let clear = |x: f32, y: f32| {
        let (a, b) = (inner(x, y), floor - y);
        if a < b { a } else { b }
    };
    // The point at column s as far from the arc as from the floor (the lens's middle line there): bisected between the arc and the floor, where the arc's room grows and the floor's shrinks.
    let medial = |s: f32| -> (f32, f32) {
        let (mut lo, mut hi) = (cy - libm::sqrtf(rr * rr - (s - cx) * (s - cx)), floor);
        for _ in 0..40 {
            let mid = (lo + hi) * 0.5;
            if inner(s, mid) < floor - mid { lo = mid } else { hi = mid }
        }
        (s, (lo + hi) * 0.5)
    };
    // The root of an increasing `g` between `lo` (below it) and `hi` (above it).
    let root = |g: &dyn Fn(f32) -> f32, mut lo: f32, mut hi: f32| {
        for _ in 0..40 {
            let mid = (lo + hi) * 0.5;
            if g(mid) < 0. { lo = mid } else { hi = mid }
        }
        hi
    };
    // Right of the centre: the first circle tangent to the arc, the floor and the centre line (its twin on the left touches it there); then each tangent to the arc, the floor and the one before it.
    let mut right = [(0f32, 0f32, 0f32); N / 2];
    let s = root(&|s: f32| { let (x, y) = medial(s); (s - cx) - clear(x, y) }, cx, cx + (width >> 2) as f32);
    let (x, y) = medial(s);
    right[0] = (x, y, clear(x, y));
    for k in 1..N / 2 {
        let p = right[k - 1];
        let s = root(&|s: f32| { let (x, y) = medial(s); libm::sqrtf((x - p.0) * (x - p.0) + (y - p.1) * (y - p.1)) - p.2 - clear(x, y) }, p.0, p.0 + (1 << 2) as f32 * p.2);
        let (x, y) = medial(s);
        right[k] = (x, y, clear(x, y));
    }
    let mut out = [(0f32, 0f32, 0f32); N];
    for k in 0..N / 2 {
        out[N / 2 + k] = right[k];
        out[N / 2 - 1 - k] = (2. * cx - right[k].0, right[k].1, right[k].2);
    }
    // The doorway: the disc about the camera hole out to the nearest of the top's console circles (outside the app's circle, the open spot NFC left).
    let (hx, hy, _) = ferros_sys::cutout_hole();
    let (corners, n) = corner_layout(width, height);
    let door = corners[..n].iter().filter(|c| c.1 < height as f32 * 0.5).map(|c| libm::sqrtf((c.0 - hx) * (c.0 - hx) + (c.1 - hy) * (c.1 - hy)) - c.2).fold(f32::MAX, f32::min);
    // The solved row, once per size, to the console: (x, y, r) in pixels, and the doorway's radius.
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
    l.s(b", doorway ");
    l.dec(door as u64);
    l.s(b"\n");
    l.flush();
    unsafe {
        LAYOUT = out;
        DOOR = door;
        LAYOUT_FOR = (width, height);
    }
    out
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
    layout(width, height);
    let (hx, hy, _) = ferros_sys::cutout_hole();
    let door = unsafe { DOOR };
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
