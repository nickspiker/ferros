//! The map (SHELL.md): the phone is a map of your life. Every person, message, photo and app is a circle on one plane; the view is a camera over it, and the camera is the only thing that moves.
//!
//! The circles: the device itself first, at the centre (its front is the dashboard, its back the settings), then the apps — Photon, Mahere, Lumis, Opsin — placed once by a seeded random fit with generous air, so they land in the same place every time.
//!
//! The gestures (owner, 2026-10-22: "two finger scale/move for edits and back to a normal two finger vs one finger for the back"):
//! - One finger dragged from the air moves the plane; dragged from a circle, it moves that circle.
//! - Two fingers both inside one circle move and size that circle; otherwise they pinch to zoom and twist to turn the plane, about the point between them.
//! - Only circles smaller than three quarters of the screen's width can be taken hold of: zoomed in until a circle fills the view, it is a place, and the fingers move the plane again.
//! - A one-finger tap on a circle arrives at it (or launches it, for an app with a capsule): the next frame has the circle filling the screen, its front shown. A tap in the air around an arrived circle leaves it.
//! - Whatever was moved is kept in the vault when the fingers lift (owner, 2026-10-22: arranging is always on, no mode).
//! - Nothing is animated and nothing is timed (owner, 2026-10-22: "no animations ... instant, always, no timers"): a view is simply there on the next frame, and a tap is fingers down and up without moving, however long they rested.
//! - A two-finger tap turns a circle over: its back, the facts about it and its few choices. Another two-finger tap turns it back.
//!
//! All of it integer: the camera centre in world units ×256, the zoom in 16.16, pinches by integer square roots.
//!
//! What is still illustrative, and why: area is the share of the owner's time each thing takes, set from the owner's own ordering until time is measured. The rings (importance) and the one lit circle (changed since last seen) are set by hand here; importance becomes the owner's to declare and lit becomes Photon's to report. Positions are deterministic until the owner arranges by hand; then each moved circle is kept in the vault (`map.place.<name>`) and wins over the seeded place from then on.
//!
//! Started by init (payloads/init). Grant slots: 0 console, 1 display (use), 2 touch (use), 3 settings, 4 the vault scope `map.`, 5 the app surface (handed to the app in front), 6 the Photon client's endpoint (use: its state and pairing words for the Photon circle's front, PHOTON.md).

#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{call, cap, console, freq, now, sleep_until};
use tatau::Style;

const S_DISPLAY: u64 = 1;
const S_TOUCH: u64 = 2;
const S_SETTINGS: u64 = 3;
const S_STORE: u64 = 4;
/// 5: the `surfaces` region (WINDOWS.md); passed to an app it starts, the kernel gives that app one surface of it, its own.
const S_APPSURF: u64 = 5;
const S_PHOTON_EP: u64 = 6;

/// One circle: its label, its weight of ring (0 none, 1 thin, 2 heavy — importance), whether it is lit (changed since last seen), its share of the owner's time in thousandths (its area is that share of the map), its icon in the display server's bundle (tools/app-icons order), and what it waits for before it can be more than a circle.
struct Spec {
    label: &'static str,
    ring: u8,
    lit: u8,
    share: u32,
    icon: u8,
    needs: &'static str,
}

/// The device first: it is placed at the centre and never lights (lit means a person sent you something). The apps' shares are the owner's ordering until time is measured (2026-10-22: "Photon the most, then Mahere, then Lumis").
const CIRCLES: [Spec; 8] = [
    Spec { label: "ferros", ring: 1, lit: 0, share: 120, icon: 5, needs: "" },
    Spec { label: "Photon", ring: 2, lit: 1, share: 400, icon: 1, needs: "WiFi and your enrollment" },
    Spec { label: "Mahere", ring: 1, lit: 0, share: 250, icon: 2, needs: "a position (GNSS)" },
    Spec { label: "Lumis", ring: 1, lit: 0, share: 200, icon: 3, needs: "the camera as a process" },
    Spec { label: "Opsin", ring: 1, lit: 0, share: 150, icon: 4, needs: "Lumis's frames" },
    // The present stage's test screen, as an app: a tap launches its capsule (payloads/ramps).
    Spec { label: "Ramps", ring: 1, lit: 0, share: 60, icon: 6, needs: "" },
    // The clock, a widget the display draws itself (icon G#80, display WIDGET_CLOCK): three rings and the time, decimal or dozenal by the device's number base.
    Spec { label: "Clock", ring: 1, lit: 0, share: 150, icon: 0x80, needs: "" },
    // The light scope, a widget the display draws itself (icon G#82, display WIDGET_SCOPE): the last second of light on the back of the phone. (The spectral bars, G#81, went 2026-10-25: the scope's levels show the same channels.)
    Spec { label: "Scope", ring: 1, lit: 0, share: 150, icon: 0x82, needs: "" },
];
const N: usize = CIRCLES.len();
const DEVICE: usize = 0;
/// Photon: arrived at, its front shows the client's state and, until the phone is bound, the 23 pairing words for the sponsor's human (payloads/photon op 1).
const PHOTON: usize = 1;
const RAMPS: usize = 5;
/// The light scope: arrived at, it is in focus (`Map::scope_drag`).
const SCOPE: usize = 7;
/// The clock: arrived at, it is in focus, and the display shows it to two more places (owner, 2026-10-26).
const CLOCK: usize = 6;
/// The scope's channels a vertical swipe steps through, as device preference 7 numbers them (1 + the VD6282's channel: 1 red, 2 visible, 3 blue, 4 green, 5 IR, 6 clear), longest wavelength first and the composites last.
const SCOPE_STEPS: [u8; 6] = [5, 1, 4, 3, 2, 6];

/// The map's total circle area, as four circles of radius 110 world units: each circle gets its share of it, so area is time.
const BASE_RADIUS: i64 = 110;

// Colours, as the display server takes them (G#00RRGGBB): the map's own greys, white for what is current.
// The colours are the theme's (ferros_sys::theme, device preference 5), read as each frame is built.
fn white() -> u32 {
    ferros_sys::theme::current().text
}
fn grey() -> u32 {
    ferros_sys::theme::current().text2
}
fn dim() -> u32 {
    ferros_sys::theme::current().text3
}
fn fill() -> u32 {
    ferros_sys::theme::current().plane
}

/// Movement under this many panel pixels is a finger resting, not a drag: a tap may wobble this much.
const SLOP: i64 = 24;
/// One, in the rotation's Q14.
const ONE: i64 = 1 << 14;

/// A circle's radius in world units: area = share x the map's total, so r = 110 x sqrt(4 x share). Integer square root, no floats.
fn radius_of(share: u32) -> i64 {
    isqrt(BASE_RADIUS * BASE_RADIUS * 4 * share as i64 / 1000)
}

/// Zigzag decode (the inverse of `save_place`'s zig).
fn unzig(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

fn isqrt(v: i64) -> i64 {
    if v <= 0 {
        return 0;
    }
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

/// A small, fixed-seed generator: the same seed places the same circles in the same places on every boot, which is what "nothing ever moves" asks of a layout that is not yet stored.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in [-span, span).
    fn signed(&mut self, span: i64) -> i64 {
        ((self.next() >> 33) as i64 % (2 * span.max(1))) - span
    }
}

/// The device at the centre; then the rest one at a time, in table order (biggest first), each at the first random spot that keeps clear of every circle already placed by at least the larger of the two radii again: more air is better, and no two ever overlap.
fn place(rs: &[i64], xs: &mut [i64], ys: &mut [i64]) {
    let mut rng = Rng(0x6D61_7020_6C69_6665);
    for i in 1..rs.len() {
        let mut spread = rs[i] + rs[0];
        loop {
            let (x, y) = (rng.signed(spread), rng.signed(spread * 8 / 5));
            let clear = (0..i).all(|j| {
                let gap = rs[i] + rs[j] + rs[i].max(rs[j]);
                let (dx, dy) = (x - xs[j], y - ys[j]);
                dx * dx + dy * dy >= gap * gap
            });
            if clear {
                xs[i] = x;
                ys[i] = y;
                break;
            }
            // Widen the search slowly, so the circles stay as close as the air allows rather than scattering.
            spread += 4;
        }
    }
}

/// A line of text being written, for tatau's formatters.
struct Line {
    b: [u8; 120],
    n: usize,
}
impl Line {
    fn new() -> Line {
        Line { b: [0; 120], n: 0 }
    }
    fn s(&mut self, t: &str) -> &mut Line {
        let _ = self.write_str(t);
        self
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        let n = b.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + n].copy_from_slice(&b[..n]);
        self.n += n;
        Ok(())
    }
}

/// The ops of one frame, sent to the display server as one batch (op 15): one call per frame however much is on it.
struct Batch {
    b: [u8; 4096],
    n: usize,
}
impl Batch {
    fn new() -> Batch {
        let mut b = Batch { b: [0; 4096], n: 1 };
        b.b[0] = 15;
        b
    }
    fn op(&mut self, bytes: &[u8]) {
        if self.n + 2 + bytes.len() > self.b.len() {
            return;
        }
        self.b[self.n..self.n + 2].copy_from_slice(&(bytes.len() as u16).to_le_bytes());
        self.b[self.n + 2..self.n + 2 + bytes.len()].copy_from_slice(bytes);
        self.n += 2 + bytes.len();
    }
    /// Text with its baseline at y; align 0 left, 1 centred on x, 2 right edge at x.
    fn text(&mut self, x: i64, y: i64, px: u16, colour: u32, align: u8, t: &str) {
        let mut m = [0u8; 140];
        m[0] = 2;
        m[1..3].copy_from_slice(&(x.clamp(0, 65535) as u16).to_le_bytes());
        m[3..5].copy_from_slice(&(y.clamp(0, 65535) as u16).to_le_bytes());
        m[5..7].copy_from_slice(&px.to_le_bytes());
        m[7..11].copy_from_slice(&colour.to_le_bytes());
        m[11] = align;
        let t = t.as_bytes();
        let k = t.len().min(m.len() - 12);
        m[12..12 + k].copy_from_slice(&t[..k]);
        self.op(&m[..12 + k]);
    }
    fn disc(&mut self, x: i64, y: i64, r: i64, colour: u32) {
        let mut m = [0u8; 11];
        m[0] = 16;
        m[1..3].copy_from_slice(&(x.clamp(-32768, 32767) as i16).to_le_bytes());
        m[3..5].copy_from_slice(&(y.clamp(-32768, 32767) as i16).to_le_bytes());
        m[5..7].copy_from_slice(&(r.clamp(0, 65535) as u16).to_le_bytes());
        m[7..11].copy_from_slice(&colour.to_le_bytes());
        self.op(&m);
    }
    fn send(&self, display: u64) {
        let mut r = [0u8; 4];
        call(display, &self.b[..self.n], &mut r);
    }
}

/// FERROS_MAP_START=crowd's extra circles (`Map::crowd`): x, y, r in world units, and how many.
static mut CROWD: ([i64; 256], [i64; 256], [i64; 256], usize) = ([0; 256], [0; 256], [0; 256], 0);

/// The camera: the centre in world units ×256, the zoom in panel pixels per world unit, 16.16, and the turn as a unit vector (cos, sin) in Q14 — world to screen is that rotation, then the zoom. A vector rather than an angle: a twist composes by one complex multiply and nothing needs a sine.
#[derive(Clone, Copy, PartialEq)]
struct Cam {
    x: i64,
    y: i64,
    z: i64,
    c: i64,
    s: i64,
}
/// The zoom's one fixed point: on release the world is rescaled so the camera's zoom is exactly this (one world unit = one pixel) and every position and radius absorbs the factor — the integers never walk toward an extreme (owner, 2026-10-23: "rescale upon release so we never accidentally hit infinities"). The limits are all relative (`Map::zoom_limits`, `Map::radius_limits`).
const Z_ONE: i64 = 1 << 16;
/// The circles together may not shrink below this fraction of the screen's area (1/256), nor the smallest circle grow past the screen; the largest and smallest circle may not differ by more than this in area (256×: 16× in radius).
const AREA_FRACTION: i64 = 256;
const RADIUS_RATIO: i64 = 16;

impl Cam {
    /// A panel offset from the centre, in pixels, to world units ×256: the rotation undone, then the zoom.
    fn delta(&self, dx: i64, dy: i64) -> (i64, i64) {
        (((self.c * dx + self.s * dy) << 10) / self.z, ((self.c * dy - self.s * dx) << 10) / self.z)
    }
    /// A panel pixel to world units ×256.
    fn world(&self, w: i64, h: i64, sx: i64, sy: i64) -> (i64, i64) {
        let (dx, dy) = self.delta(sx - w / 2, sy - h / 2);
        (self.x + dx, self.y + dy)
    }
    /// A world point (×256) to its panel pixel: `world` undone.
    fn screen(&self, w: i64, h: i64, wx: i64, wy: i64) -> (i64, i64) {
        let (dx, dy) = (wx - self.x, wy - self.y);
        (w / 2 + (((self.c * dx - self.s * dy) / ONE * self.z) >> 24), h / 2 + (((self.s * dx + self.c * dy) / ONE * self.z) >> 24))
    }
    fn op(&self, keep: bool) -> [u8; 18] {
        let mut m = [0u8; 18];
        m[0] = 14;
        m[1..5].copy_from_slice(&(self.x as i32).to_le_bytes());
        m[5..9].copy_from_slice(&(self.y as i32).to_le_bytes());
        m[9..13].copy_from_slice(&(self.z as u32).to_le_bytes());
        m[13] = keep as u8;
        m[14..16].copy_from_slice(&(self.c as i16).to_le_bytes());
        m[16..18].copy_from_slice(&(self.s as i16).to_le_bytes());
        m
    }
}

/// What the screen is showing: the plane, or one circle's front or back, arrived at.
#[derive(Clone, Copy, PartialEq)]
enum View {
    Map,
    Front(usize),
    Back(usize),
}

/// The device's back: what each row does when tapped.
#[derive(Clone, Copy)]
enum Choice {
    Base(u8),
    Labels,
    Dither,
    Theme,
    /// The brightness dots: which one is decided by x.
    Brightness,
}

/// The four bases in the order the chooser shows them: the house base first.
const BASES: [u8; 4] = [12, 0x8C, 10, 16];

fn base_name(code: u8) -> &'static str {
    match code {
        10 => "arabic",
        16 => "hexadecimal",
        0x8C => "dozenal linear (DLS)",
        _ => "dozenal magnitude (DMS)",
    }
}

struct Map {
    grants: u64,
    display: u64,
    w: i64,
    h: i64,
    /// Circles in world units ×256, and radii in world units.
    xs: [i64; N],
    ys: [i64; N],
    rs: [i64; N],
    cam: Cam,
    view: View,
    /// The device back's rows, as laid out by the last frame: (top, bottom, what).
    rows: [(i64, i64, Choice); 8],
    nrows: usize,
    /// Each circle's own turn (its icon), (cos, sin) in Q14; two fingers in a circle twist it.
    rc: [i64; N],
    rsn: [i64; N],
    /// Circles moved or sized by hand since arranging began, to keep when it ends.
    arranged: [bool; N],
    /// The one ramps column shown alone, if any (the display-side ramps of op 21, kept for comparison).
    ramps_only: Option<usize>,
    /// The app in front has shown itself, and when it was launched.
    app_seen: bool,
    app_since: u64,
    /// The display server's ramps are on the panel (the present stage's test screen); any tap comes back.
    ramps: bool,
    /// The time the focused scope shows, µs, and the gesture under way on it: its axis (0 not yet known, 1 across: the time, 2 up and down: the channel) and the vertical travel not yet spent on a step.
    scope_span_us: u32,
    scope_axis: u8,
    scope_acc: i64,
    /// The focused scope folded on its dominant period (a one-finger tap toggles it).
    scope_fold: bool,
    /// The focus, span and mode the display last heard (display op 25).
    focus_sent: (u8, u32, bool),
}

impl Map {
    /// The arrived circle's radius on the panel.
    fn arrived_r(&self) -> i64 {
        self.w * 15 / 32
    }

    /// A circle's vault key, relative to the map's scope: `place.<label>`, so a circle keeps its place however the table grows.
    fn place_key(i: usize, key: &mut [u8; 32]) -> usize {
        let l = CIRCLES[i].label.as_bytes();
        key[..6].copy_from_slice(b"place.");
        let n = l.len().min(26);
        key[6..6 + n].copy_from_slice(&l[..n]);
        6 + n
    }

    /// The arrangement made by hand, from the vault: each circle's place as EWE integers (CLAUDE.md: no fixed widths on storage) — x and y in world units ×256, zigzag-mapped (they can be negative), the radius, and then (since the circles turn) its turn as cos and sin in Q14, zigzag; a place kept before turns existed has three and stays upright.
    fn load_places(&mut self) -> usize {
        let mut loaded = 0;
        for i in 0..N {
            let mut key = [0u8; 32];
            let kl = Map::place_key(i, &mut key);
            let mut v = [0u8; 64];
            let Some(n) = ferros_sys::store_get(cap(self.grants, S_STORE), &key[..kl], &mut v) else { continue };
            let v = &v[..n.min(64)];
            let mut at = 0;
            let mut vals = [0u64; 5];
            let mut got = 0;
            for slot in vals.iter_mut() {
                match ferros_ledger::ewe::decode_u64(&v[at.min(v.len())..]) {
                    Some((x, k)) => {
                        *slot = x;
                        at += k;
                        got += 1;
                    }
                    None => break,
                }
            }
            let ok = got >= 3;
            if ok && vals[2] > 0 && vals[2] < 65536 {
                let unzig = |u: u64| ((u >> 1) as i64) ^ -((u & 1) as i64);
                self.xs[i] = unzig(vals[0]);
                self.ys[i] = unzig(vals[1]);
                self.rs[i] = vals[2] as i64;
                if got == 5 {
                    let (c, sn) = (unzig(vals[3]), unzig(vals[4]));
                    if c.abs() <= ONE && sn.abs() <= ONE && c * c + sn * sn > ONE * ONE / 2 {
                        self.rc[i] = c;
                        self.rsn[i] = sn;
                    }
                }
                loaded += 1;
            }
        }
        loaded
    }

    /// Keep circle i's place in the vault.
    fn save_place(&self, i: usize) -> bool {
        let mut key = [0u8; 32];
        let kl = Map::place_key(i, &mut key);
        let zig = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
        let mut b = [0u8; 32];
        let mut n = 0;
        for v in [zig(self.xs[i]), zig(self.ys[i]), self.rs[i] as u64, zig(self.rc[i]), zig(self.rsn[i])] {
            n += ferros_ledger::ewe::encode_u64(&mut b[n..], v);
        }
        ferros_sys::store_set(cap(self.grants, S_STORE), &key[..kl], &b[..n])
    }

    /// No two circles overlap: after circle `held` moved or grew, every overlapping pair is pushed apart along the line between their centres until they touch (with a hair of air), the held one never moving — so a push carries on through the neighbours (owner, 2026-10-22). A few rounds settle it; every circle that moved is redrawn and kept when the fingers lift.
    fn push_apart(&mut self, held: usize) {
        const AIR: i64 = 4;
        for _ in 0..8 {
            let mut any = false;
            for a in 0..N {
                for b in a + 1..N {
                    let (dx, dy) = (self.xs[b] - self.xs[a], self.ys[b] - self.ys[a]);
                    let want = (self.rs[a] + self.rs[b] + AIR) << 8;
                    let d2 = dx * dx + dy * dy;
                    if d2 >= want * want {
                        continue;
                    }
                    any = true;
                    let d = isqrt(d2);
                    let (ux, uy) = if d > 0 { ((dx << 14) / d, (dy << 14) / d) } else { (ONE, 0) };
                    let gap = want - d;
                    // The held circle stays where the fingers are; between two others, each gives half.
                    let (ka, kb) = if a == held { (0, gap) } else if b == held { (gap, 0) } else { (gap / 2, gap - gap / 2) };
                    self.xs[a] -= ux * ka >> 14;
                    self.ys[a] -= uy * ka >> 14;
                    self.xs[b] += ux * kb >> 14;
                    self.ys[b] += uy * kb >> 14;
                    for i in [a, b] {
                        if i != held {
                            self.arranged[i] = true;
                            self.send_move(i);
                        }
                    }
                }
            }
            if !any {
                break;
            }
        }
    }

    /// The circle a finger at this panel pixel takes hold of: the one under it, if it is small enough on screen to be an object rather than the place the view is in, and only on the plane (an arrived circle's front or back moves the view).
    fn grasp(&self, sx: i64, sy: i64) -> Option<usize> {
        if self.view != View::Map {
            return None;
        }
        // A circle is the plane only when it covers the whole screen — all four corners inside it, nothing else to see: then fingers on it pan and zoom, the way out of the zoom-in limit. Any other circle under the fingers is held, however big and wherever its centre is (owner, 2026-10-23: a big circle with its centre off screen only moved the camera).
        self.hit(sx, sy).filter(|&i| !self.covers_screen(i))
    }

    /// Whether circle `i` covers all four corners of the screen (distance squared, world units).
    fn covers_screen(&self, i: usize) -> bool {
        [(0, 0), (self.w, 0), (0, self.h), (self.w, self.h)].iter().all(|&(sx, sy)| {
            let (wx, wy) = self.cam.world(self.w, self.h, sx, sy);
            let (dx, dy) = ((wx - self.xs[i]) >> 8, (wy - self.ys[i]) >> 8);
            dx * dx + dy * dy <= self.rs[i] * self.rs[i]
        })
    }

    /// The zoom's bounds, relative to the circles as they are: out, until together they cover 1/AREA_FRACTION of the screen; in, until the smallest alone fills it. (16.16, like `cam.z`.)
    fn zoom_limits(&self) -> (i64, i64) {
        let mut sum_r2 = 0i64;
        let mut r_min = i64::MAX;
        for i in 0..N {
            let r = self.rs[i].max(1);
            sum_r2 += r * r;
            r_min = r_min.min(r);
        }
        // Σ π r² z² ≥ w h / F  →  z ≥ √(w h / (F π Σ r²)); 804 ≈ 256 π, scaled for F.
        let den = (sum_r2 * 804 * AREA_FRACTION / 256).max(1);
        let z_out = isqrt((self.w * self.h / den) << 32).max(1);
        let z_in = (self.w.min(self.h) << 16) / (2 * r_min);
        (z_out.min(z_in), z_in.max(z_out))
    }

    /// The radius circle `i` may take, given the others: within RADIUS_RATIO of the smallest and the largest of them.
    fn radius_limits(&self, i: usize) -> (i64, i64) {
        let (mut lo, mut hi) = (i64::MAX, 1i64);
        for j in 0..N {
            if j != i {
                lo = lo.min(self.rs[j].max(1));
                hi = hi.max(self.rs[j]);
            }
        }
        ((hi / RADIUS_RATIO).max(1), (lo * RADIUS_RATIO).max(1))
    }

    /// The circles may not leave the screen entirely: their joint bounding box on the panel must still touch it (owner, 2026-10-23: "at least one edge has to be touching so we don't lose the circles"). Called after every camera change; the camera is pulled back by the overshoot.
    fn keep_on_screen(&mut self) {
        let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for i in 0..N {
            let (sx, sy) = self.cam.screen(self.w, self.h, self.xs[i], self.ys[i]);
            let sr = (self.rs[i] * self.cam.z >> 16).max(1);
            x0 = x0.min(sx - sr);
            y0 = y0.min(sy - sr);
            x1 = x1.max(sx + sr);
            y1 = y1.max(sy + sr);
        }
        let shift_x = if x1 < 0 { -x1 } else if x0 > self.w { self.w - x0 } else { 0 };
        let shift_y = if y1 < 0 { -y1 } else if y0 > self.h { self.h - y0 } else { 0 };
        if shift_x != 0 || shift_y != 0 {
            // Content must move by the shift on the panel: the camera moves the other way, in world units.
            let (dx, dy) = self.cam.delta(shift_x, shift_y);
            self.cam.x -= dx;
            self.cam.y -= dy;
        }
    }

    /// The release: the camera's zoom back to exactly one, the world rescaled to match (positions ×256 and radii alike), so nothing accumulates. Every circle is then kept, since its stored place changed.
    fn renormalise(&mut self) {
        let z = self.cam.z;
        if z == Z_ONE {
            return;
        }
        for i in 0..N {
            self.xs[i] = self.xs[i] * z / Z_ONE;
            self.ys[i] = self.ys[i] * z / Z_ONE;
            self.rs[i] = (self.rs[i] * z / Z_ONE).max(1);
            self.arranged[i] = true;
        }
        self.cam.x = self.cam.x * z / Z_ONE;
        self.cam.y = self.cam.y * z / Z_ONE;
        self.cam.z = Z_ONE;
        self.send_scene();
    }

    /// The view — where the camera is and how the plane is turned — into the vault (`map.view`: zigzag x, y, c, s), so a restart shows the plane as it was left. The zoom is always one at a release (`renormalise`), so the scale is already in the radii.
    fn save_view(&self) -> bool {
        let zig = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
        let mut b = [0u8; 64];
        let mut n = 0;
        for v in [zig(self.cam.x), zig(self.cam.y), zig(self.cam.c), zig(self.cam.s)] {
            n += ferros_ledger::ewe::encode_u64(&mut b[n..], v);
        }
        ferros_sys::store_set(cap(self.grants, S_STORE), b"view", &b[..n])
    }
    fn load_view(&mut self) -> bool {
        let mut v = [0u8; 64];
        let Some(n) = ferros_sys::store_get(cap(self.grants, S_STORE), b"view", &mut v) else { return false };
        let n = (n as usize).min(v.len());
        let mut vals = [0i64; 4];
        let mut at = 0;
        for slot in vals.iter_mut() {
            match ferros_ledger::ewe::decode_u64(&v[at.min(n)..n]) {
                Some((x, used)) => {
                    *slot = unzig(x);
                    at += used;
                }
                None => return false,
            }
        }
        // A sane turn only: (c, s) near the unit circle in Q14.
        let len2 = vals[2] * vals[2] + vals[3] * vals[3];
        if !(ONE * ONE * 9 / 10..=ONE * ONE * 11 / 10).contains(&len2) {
            return false;
        }
        self.cam = Cam { x: vals[0], y: vals[1], z: Z_ONE, c: vals[2], s: vals[3] };
        self.keep_on_screen();
        true
    }

    /// Whether, with circle `i` moved by (dx, dy), every circle still has a neighbour within one screen diagonal of it (edge to edge) at the zoom-out limit — distance, not a bounding box, so no orientation and no rectangles (owner, 2026-10-23: "so it never exceeds the screen diagonal; don't want weird rectangles floating around"). Zooming all the way out then always shows a connected set.
    fn fits_when_out(&self, i: usize, dx: i64, dy: i64) -> bool {
        let (z_out, _) = self.zoom_limits();
        // Positions ×256 and radii in world units: the gap in world units ×256, against the diagonal brought to world ×256 at the zoom-out limit.
        let diag = isqrt(self.w * self.w + self.h * self.h);
        let limit = ((diag << 16) / z_out.max(1)) << 8;
        let pos = |j: usize| if j == i { (self.xs[j] + dx, self.ys[j] + dy) } else { (self.xs[j], self.ys[j]) };
        for a in 0..N {
            let (ax, ay) = pos(a);
            let mut nearest = i64::MAX;
            for b in 0..N {
                if b == a {
                    continue;
                }
                let (bx, by) = pos(b);
                let d = isqrt((ax - bx) * (ax - bx) + (ay - by) * (ay - by)) - ((self.rs[a] + self.rs[b]) << 8);
                nearest = nearest.min(d);
            }
            if nearest > limit {
                return false;
            }
        }
        true
    }

    /// Keep every circle moved since the last time in the vault.
    fn save_arranged(&mut self) {
        for i in 0..N {
            if self.arranged[i] {
                self.arranged[i] = false;
                if !self.save_place(i) {
                    console(self.grants, b"map: the vault did not take a place\n");
                }
            }
        }
    }

    /// The circle under a panel pixel, smallest first when they overlap (the smaller is drawn on top).
    fn hit(&self, sx: i64, sy: i64) -> Option<usize> {
        let (wx, wy) = self.cam.world(self.w, self.h, sx, sy);
        let mut best: Option<usize> = None;
        for i in 0..N {
            let (dx, dy) = ((wx - self.xs[i]) >> 8, (wy - self.ys[i]) >> 8);
            if dx * dx + dy * dy <= self.rs[i] * self.rs[i] && best.is_none_or(|b| self.rs[i] < self.rs[b]) {
                best = Some(i);
            }
        }
        best
    }

    /// The whole scene to the display server: clear, then every circle in one call.
    fn send_scene(&self) {
        let labels = ferros_sys::pref(ferros_sys::PREF_LABELS) == 1;
        let mut r = [0u8; 4];
        call(self.display, &[12], &mut r);
        let mut m = [0u8; 2 + 30 * N];
        m[0] = 13;
        m[1] = N as u8;
        for (i, a) in CIRCLES.iter().enumerate() {
            let at = 2 + i * 30;
            m[at..at + 4].copy_from_slice(&((self.xs[i] >> 8) as i32).to_le_bytes());
            m[at + 4..at + 8].copy_from_slice(&((self.ys[i] >> 8) as i32).to_le_bytes());
            m[at + 8..at + 10].copy_from_slice(&(self.rs[i] as u16).to_le_bytes());
            m[at + 10] = a.ring;
            m[at + 11] = a.lit;
            m[at + 12] = a.icon;
            let label = a.label.as_bytes();
            let len = if labels { label.len().min(16) } else { 0 };
            m[at + 13] = len as u8;
            m[at + 14..at + 14 + len].copy_from_slice(&label[..len]);
        }
        call(self.display, &m, &mut r);
        self.send_crowd();
        // The circles' own turns ride on `scene move`: a fresh scene starts every circle upright.
        for i in 0..N {
            if self.rc[i] != ONE || self.rsn[i] != 0 {
                self.send_move(i);
            }
        }
    }

    /// The crowd's circles (world units: x, y, r), kept so every `send_scene` appends them again after the map's own.
    fn send_crowd(&self) {
        let (xs, ys, rs, k) = unsafe { &*(&raw const CROWD) };
        let mut r = [0u8; 4];
        let mut done = 0;
        while done < *k {
            let count = (*k - done).min(120);
            let mut m = [0u8; 2 + 30 * 120];
            m[0] = 13;
            m[1] = count as u8;
            for c in 0..count {
                let i = done + c;
                let at = 2 + c * 30;
                m[at..at + 4].copy_from_slice(&(xs[i] as i32).to_le_bytes());
                m[at + 4..at + 8].copy_from_slice(&(ys[i] as i32).to_le_bytes());
                m[at + 8..at + 10].copy_from_slice(&(rs[i].clamp(1, 65535) as u16).to_le_bytes());
                m[at + 10] = (i % 3) as u8;
                m[at + 11] = (i % 17 == 0) as u8;
                m[at + 12] = [1u8, 2, 3, 4, 5, 6][i % 6];
            }
            call(self.display, &m[..2 + 30 * count], &mut r);
            done += count;
        }
    }

    /// A stress scene (FERROS_MAP_START=crowd): `n` circles of random size, 20-120 px across at the current zoom, around the view (1.2 screens), none overlapping each other or the map's own, appended to the display's scene only. Random importance (so the glows vary), a few lit, icons from the bundle in turn. Returns how many fit.
    fn crowd(&self, n: usize) -> usize {
        let mut rng = Rng(0x6372_6F77_6400_0001);
        let z = self.cam.z.max(1);
        let px = |p: i64| p * 65536 / z;
        let (cx, cy) = (self.cam.x >> 8, self.cam.y >> 8);
        let (hw, hh) = (px(self.w) * 6 / 10, px(self.h) * 6 / 10);
        let mut xs = [0i64; 256];
        let mut ys = [0i64; 256];
        let mut rs = [0i64; 256];
        let mut k = 0;
        for i in 0..N {
            xs[k] = self.xs[i] >> 8;
            ys[k] = self.ys[i] >> 8;
            rs[k] = self.rs[i];
            k += 1;
        }
        let own = k;
        let mut tries = 0;
        while k < own + n && tries < 200_000 {
            tries += 1;
            let r = px(20 + (rng.next() >> 40) as i64 % 101);
            let (x, y) = (cx + rng.signed(hw), cy + rng.signed(hh));
            if (0..k).all(|j| {
                let (dx, dy, g) = (x - xs[j], y - ys[j], r + rs[j]);
                dx * dx + dy * dy >= g * g
            }) {
                xs[k] = x;
                ys[k] = y;
                rs[k] = r;
                k += 1;
            }
        }
        let mut c = (xs, ys, rs, 0usize);
        for i in own..k {
            c.0[i - own] = xs[i];
            c.1[i - own] = ys[i];
            c.2[i - own] = rs[i];
        }
        c.3 = k - own;
        unsafe { CROWD = c };
        self.send_crowd();
        k - own
    }

    /// One circle moved or resized in the display server's scene (op 17).
    fn send_move(&self, i: usize) {
        let mut m = [0u8; 16];
        m[0] = 17;
        m[1] = i as u8;
        m[2..6].copy_from_slice(&((self.xs[i] >> 8) as i32).to_le_bytes());
        m[6..10].copy_from_slice(&((self.ys[i] >> 8) as i32).to_le_bytes());
        m[10..12].copy_from_slice(&(self.rs[i] as u16).to_le_bytes());
        m[12..14].copy_from_slice(&(self.rc[i] as i16).to_le_bytes());
        m[14..16].copy_from_slice(&(self.rsn[i] as i16).to_le_bytes());
        let mut r = [0u8; 4];
        call(self.display, &m, &mut r);
    }

    /// The camera that shows every circle with air around them, never closer than one world unit to a pixel.
    fn home(&self) -> Cam {
        let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for i in 0..N {
            let r = self.rs[i] << 8;
            x0 = x0.min(self.xs[i] - r);
            y0 = y0.min(self.ys[i] - r);
            x1 = x1.max(self.xs[i] + r);
            y1 = y1.max(self.ys[i] + r);
        }
        // Zoom so the bounds take seven eighths of the panel either way.
        let (bw, bh) = (((x1 - x0) >> 8).max(1), ((y1 - y0) >> 8).max(1));
        let z = ((self.w * 7 / 8) << 16) / bw;
        let (lo, hi) = self.zoom_limits();
        let z = z.min(((self.h * 7 / 8) << 16) / bh).clamp(lo, hi);
        Cam { x: (x0 + x1) / 2, y: (y0 + y1) / 2, z, c: self.cam.c, s: self.cam.s }
    }

    /// The camera that has arrived at circle i: it fills the panel's width, less a margin.
    fn at(&self, i: usize) -> Cam {
        let (lo, hi) = self.zoom_limits();
        Cam { x: self.xs[i], y: self.ys[i], z: ((self.arrived_r() << 16) / self.rs[i].max(1)).clamp(lo, hi), c: self.cam.c, s: self.cam.s }
    }

    /// Go to `to`: the camera is simply there (no animation).
    fn fly(&mut self, to: Cam) {
        self.cam = to;
    }

    /// Draw the frame: the overlay for the view first (what is sent first is on top), then the scene under it through the camera.
    fn draw(&mut self) {
        let mut b = Batch::new();
        b.op(&[1]);
        self.nrows = 0;
        match self.view {
            View::Map => {}
            View::Front(DEVICE) => self.dashboard(&mut b),
            View::Back(DEVICE) => self.settings(&mut b),
            View::Front(SCOPE) => {
                let (cx, cy, r) = (self.w / 2, self.h / 2, self.arrived_r());
                b.text(cx, cy + r + 64, 32, grey(), 1, "tap: fold \u{2022} drag across: time \u{2022} swipe up or down: channel \u{2022} two fingers: back");
            }
            View::Front(PHOTON) => self.photon_front(&mut b),
            View::Front(i) => {
                let (cx, cy, r) = (self.w / 2, self.h / 2, self.arrived_r());
                b.text(cx, cy + r + 96, 52, white(), 1, CIRCLES[i].label);
                b.text(cx, cy + r + 160, 32, grey(), 1, "not here yet \u{2022} two fingers for its back");
            }
            View::Back(i) => self.app_back(&mut b, i),
        }
        b.op(&self.cam.op(true));
        b.send(self.display);
        self.send_focus();
    }

    /// The Photon circle's front: the client's state line and, while unbound, the 23 pairing words in rows (op 1 of the `photon` endpoint → [state][fleet devices heard][words]). The client not running reads as "not running".
    fn photon_front(&mut self, b: &mut Batch) {
        let (cx, cy, r) = (self.w / 2, self.h / 2, self.arrived_r());
        b.text(cx, cy + r + 96, 52, white(), 1, CIRCLES[PHOTON].label);
        let mut out = [0u8; 512];
        let n = ferros_sys::call(cap(self.grants, S_PHOTON_EP), &[1], &mut out);
        if n == ferros_sys::ERR || n == ferros_sys::NOTHING || n < 2 {
            b.text(cx, cy + r + 160, 32, grey(), 1, "the Photon client is not running \u{2022} two fingers for its back");
            return;
        }
        let n = (n as usize).min(out.len());
        let (state, heard) = (out[0], out[1]);
        let line = match state {
            0 => "no identity on this device yet: attest at boot",
            1 => "this identity has a fleet \u{2022} add this device from one of its devices",
            2 => "bound \u{2022} waiting for the fleet key",
            3 => "a device of your fleet",
            4 => "looking the identity up",
            5 => "no fleet holds this handle \u{2022} it can be minted from the attest screen",
            6 => "fgtw.org's chain for this handle is another identity's",
            _ => "no fleet device and no fgtw.org answer \u{2022} nothing decided yet",
        };
        b.text(cx, cy + r + 160, 32, grey(), 1, line);
        let mut heard_line = [0u8; 48];
        let hl = {
            let s = b"fleet devices heard on this network: ";
            heard_line[..s.len()].copy_from_slice(s);
            heard_line[s.len()] = b'0' + heard.min(9);
            s.len() + 1
        };
        if let Ok(t) = core::str::from_utf8(&heard_line[..hl]) {
            b.text(cx, cy + r + 208, 32, grey(), 1, t);
        }
        if state == 1 {
            if let Ok(words) = core::str::from_utf8(&out[2..n]) {
                b.text(cx, cy + r + 280, 32, grey(), 1, "type these on your other device (Settings \u{2022} Fleet \u{2022} Add device):");
                // The words come camelCased in one run; six to a row, split at the capitals.
                let mut rows: [[u8; 128]; 5] = [[0; 128]; 5];
                let mut lens = [0usize; 5];
                let (mut row, mut count) = (0usize, 0usize);
                for (i, c) in words.bytes().enumerate() {
                    if c.is_ascii_uppercase() && i > 0 {
                        count += 1;
                        if count % 6 == 0 && row + 1 < rows.len() {
                            row += 1;
                        } else if lens[row] < 127 {
                            rows[row][lens[row]] = b' ';
                            lens[row] += 1;
                        }
                    }
                    if lens[row] < 128 {
                        rows[row][lens[row]] = c;
                        lens[row] += 1;
                    }
                }
                for k in 0..=row {
                    if let Ok(t) = core::str::from_utf8(&rows[k][..lens[k]]) {
                        b.text(cx, cy + r + 340 + 56 * k as i64, 44, white(), 1, t);
                    }
                }
            }
        }
    }

    /// The display told what is in focus (op 25): the scope or the clock when arrived at, so it is redrawn every frame while everything else keeps the 10 Hz flips; the scope's span and whether it is folded. A call of its own, never in a frame's batch: the display replays its last frame between flips, and a replayed focus would undo a drag's span.
    fn send_focus(&mut self) {
        let f = (
            match self.view {
                View::Front(SCOPE) => CIRCLES[SCOPE].icon,
                View::Front(CLOCK) => CIRCLES[CLOCK].icon,
                _ => 0,
            },
            self.scope_span_us,
            self.scope_fold,
        );
        if f != self.focus_sent {
            self.focus_sent = f;
            let mut m = [0u8; 7];
            m[0] = 25;
            m[1] = f.0;
            m[2..6].copy_from_slice(&f.1.to_le_bytes());
            m[6] = f.2 as u8;
            let mut r = [0u8; 4];
            call(self.display, &m, &mut r);
        }
    }

    /// One finger on the focused scope (owner, 2026-10-25), along the plot's own axes however it is turned: across, the time it shows — to the right stretches the trace (a shorter span), to the left compresses it, a factor of two per half the screen's width, from 2^-6 s to 4 s; up or down, the next or the previous channel, a step per eighth of the screen's height. The gesture's axis is whichever its travel (tx, ty) favoured when it passed the slop, and holds until the fingers lift.
    fn scope_drag(&mut self, dx: i64, dy: i64, tx: i64, ty: i64) {
        // Into the plot's own axes: its turn on the screen is the camera's and the circle's (as the display draws it), undone.
        let (cc, cs) = (self.rc[SCOPE], self.rsn[SCOPE]);
        let (tc, ts) = ((self.cam.c * cc - self.cam.s * cs) >> 14, (self.cam.s * cc + self.cam.c * cs) >> 14);
        let local = |x: i64, y: i64| ((tc * x + ts * y) >> 14, (tc * y - ts * x) >> 14);
        let ((dx, dy), (tx, ty)) = (local(dx, dy), local(tx, ty));
        if self.scope_axis == 0 {
            self.scope_axis = if tx.abs() >= ty.abs() { 1 } else { 2 };
        }
        if self.scope_axis == 1 {
            let f = libm::exp2f(-(dx as f32) / (self.w as f32 * 0.5));
            self.scope_span_us = ((self.scope_span_us as f32 * f) as u32).clamp(15_625, 4_000_000);
            self.send_focus();
            return;
        }
        self.scope_acc += dy;
        let step = (self.h / 8).max(1);
        while self.scope_acc.abs() >= step {
            let up = self.scope_acc < 0;
            self.scope_acc += if up { step } else { -step };
            let now_ch = ferros_sys::pref(ferros_sys::PREF_SCOPE);
            let at = SCOPE_STEPS.iter().position(|&c| c == now_ch);
            let k = match (at, up) {
                (None, _) => 0,
                (Some(i), true) => (i + 1) % SCOPE_STEPS.len(),
                (Some(i), false) => (i + SCOPE_STEPS.len() - 1) % SCOPE_STEPS.len(),
            };
            ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_SCOPE, SCOPE_STEPS[k]);
        }
    }

    /// Label and value on a row: the label right of centre's left, the value left of centre's right.
    fn row(b: &mut Batch, cx: i64, y: i64, label: &str, value: &str) {
        b.text(cx - 20, y, 36, grey(), 2, label);
        b.text(cx + 20, y, 36, white(), 0, value);
    }

    /// The device's front: the state of the device in plain facts, every number in the device base.
    fn dashboard(&self, b: &mut Batch) {
        let (cx, cy, r) = (self.w / 2, self.h / 2, self.arrived_r());
        let f = ferros_sys::numbers(Style::Glyphs);
        let facts = ferros_sys::facts();
        let mut y = cy - r * 9 / 16;
        b.text(cx, y, 72, white(), 1, "ferros");
        y += 120;
        let mut v = Line::new();
        match facts.battery_pct() {
            Some(p) => {
                let _ = tatau::share(&mut v, f, p, 100, 2);
                if let Some(ua) = facts.battery_ua() {
                    v.s(if ua > 0 { "  charging" } else { "  on battery" });
                }
            }
            None => {
                v.s("not read");
            }
        }
        Map::row(b, cx, y, "battery", v.as_str());
        y += 64;
        let mut v = Line::new();
        match facts.boot_us() {
            Some(us) => {
                let _ = tatau::duration_ns(&mut v, f, us * 1000, 1);
            }
            None => {
                v.s("-");
            }
        }
        Map::row(b, cx, y, "press to ready", v.as_str());
        y += 64;
        let mut v = Line::new();
        // The counter runs from the SoC's reset: power on, near enough (the bootloader's seconds included).
        let (t, hz) = (now(), freq());
        let _ = tatau::duration_ns(&mut v, f, t / hz * 1_000_000_000 + t % hz * 1_000_000_000 / hz, 1);
        Map::row(b, cx, y, "since power on", v.as_str());
        y += 64;
        Map::row(b, cx, y, "USB", match facts.usb_link() { Some(true) => "linked", Some(false) => "no link", None => "-" });
        y += 64;
        let mut v = Line::new();
        if let Some(n) = facts.processes() {
            let _ = tatau::count(&mut v, f, n, 1);
        }
        Map::row(b, cx, y, "processes", v.as_str());
        y += 64;
        let mut v = Line::new();
        if let Some(n) = facts.vault_keys() {
            let _ = tatau::count(&mut v, f, n, 1);
        }
        Map::row(b, cx, y, "in the vault", v.as_str());
        y += 64;
        // The hottest part of the SoC as the kernel last read it (once a second, over ACPM), and whether the kernel has the big core's clock down for it.
        let mut v = Line::new();
        match facts.soc_temp_c() {
            Some(c) => {
                let _ = tatau::count(&mut v, f, c, 1);
                v.s(" °C");
                if facts.throttled() == Some(true) {
                    v.s("  throttled");
                }
            }
            None => {
                v.s("-");
            }
        }
        Map::row(b, cx, y, "chip", v.as_str());
        y += 64;
        Map::row(b, cx, y, "numbers", base_name(ferros_sys::device_base()));
        b.text(cx, cy + r * 3 / 4, 28, dim(), 1, "two fingers: settings");
        b.disc(cx, cy, r - r / 25 - 1, fill());
    }

    /// The device's back: its few choices. The number base is chosen by seeing — each row is two seconds and a mebibyte (2^23 bits, a whole number of doublings) written in that base — with its name small beneath.
    fn settings(&mut self, b: &mut Batch) {
        let (cx, cy, r) = (self.w / 2, self.h / 2, self.arrived_r());
        let mut y = cy - r * 5 / 8;
        b.text(cx, y, 56, white(), 1, "settings");
        y += 96;
        let now_base = ferros_sys::device_base();
        for code in BASES {
            let f = tatau::Fmt::new(tatau::Base::from_code(code).unwrap_or(tatau::Base::Dms), Style::Glyphs);
            let mut v = Line::new();
            let _ = tatau::duration_ns(&mut v, f, 2_000_000_000, 1);
            v.s("   ");
            let _ = tatau::size_bits(&mut v, f, 1 << 23, 1);
            let current = code == now_base;
            b.text(cx, y, 44, if current { white() } else { grey() }, 1, v.as_str());
            b.text(cx, y + 38, 24, if current { grey() } else { dim() }, 1, base_name(code));
            if current {
                b.disc(cx - r * 5 / 8, y - 14, 10, white());
            }
            self.push_row(y - 56, y + 52, Choice::Base(code));
            y += 112;
        }
        y += 24;
        let labels = ferros_sys::pref(ferros_sys::PREF_LABELS) == 1;
        b.text(cx, y, 36, grey(), 1, if labels { "names under circles: on" } else { "names under circles: off" });
        self.push_row(y - 44, y + 24, Choice::Labels);
        y += 80;
        let spatial = ferros_sys::pref(ferros_sys::PREF_DITHER) == 1;
        b.text(cx, y, 36, grey(), 1, if spatial { "dither: spatial (the panel rests)" } else { "dither: spatial and temporal" });
        self.push_row(y - 44, y + 24, Choice::Dither);
        y += 80;
        let mut tl = Line::new();
        let _ = write!(tl, "theme: {}", ferros_sys::theme::current().name);
        b.text(cx, y, 36, grey(), 1, tl.as_str());
        self.push_row(y - 44, y + 24, Choice::Theme);
        // Brightness: nine dots, a stop apart, dimmest on the left; the current one larger and white.
        y += 80;
        b.text(cx, y, 36, grey(), 1, "brightness");
        y += 60;
        let now = 8 - ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS).min(8) as i64;
        let (x0, step) = Map::dots(cx, r);
        for k in 0..9 {
            let x = x0 + k * step;
            if k == now {
                b.disc(x, y, 14, white());
            } else {
                b.disc(x, y, 8, dim());
            }
        }
        self.push_row(y - 40, y + 40, Choice::Brightness);


        b.disc(cx, cy, r - r / 25 - 1, fill());
    }

    /// The brightness dots' first x and spacing on the device's back.
    fn dots(cx: i64, r: i64) -> (i64, i64) {
        let step = r * 3 / 2 / 8;
        (cx - 4 * step, step)
    }

    fn push_row(&mut self, top: i64, bottom: i64, what: Choice) {
        if self.nrows < self.rows.len() {
            self.rows[self.nrows] = (top, bottom, what);
            self.nrows += 1;
        }
    }

    /// An app's back: the facts about it, the same for every circle.
    fn app_back(&self, b: &mut Batch, i: usize) {
        let (cx, cy, r) = (self.w / 2, self.h / 2, self.arrived_r());
        let f = ferros_sys::numbers(Style::Glyphs);
        let a = &CIRCLES[i];
        let mut y = cy - r / 2;
        b.text(cx, y, 64, white(), 1, a.label);
        y += 112;
        let mut v = Line::new();
        let _ = tatau::share(&mut v, f, a.share as u64, 1000, 2);
        Map::row(b, cx, y, "your time", v.as_str());
        y += 64;
        let here = a.needs.is_empty();
        Map::row(b, cx, y, "installed", if here { "yes" } else { "not yet" });
        y += 64;
        Map::row(b, cx, y, "waits for", if here { "nothing" } else { a.needs });
        y += 64;
        Map::row(b, cx, y, "capabilities", "none");
        b.text(cx, cy + r * 3 / 4, 28, dim(), 1, "time is set by hand until it is measured");
        b.disc(cx, cy, r - r / 25 - 1, fill());
    }

    /// A tap: one finger or two, at a panel point. Logged with what it did, so a session can be followed from `diag console`.
    fn tap(&mut self, fingers: usize, sx: i64, sy: i64) {
        let before = self.view;
        self.tap_inner(fingers, sx, sy);
        let mut l = Line::new();
        let name = |v: View| match v {
            View::Map => ("map", ""),
            View::Front(i) => ("front of ", CIRCLES[i].label),
            View::Back(i) => ("back of ", CIRCLES[i].label),
        };
        let (a, b) = name(self.view);
        let _ = write!(l, "map: {fingers}-finger tap at {sx},{sy} -> {a}{b}{}\n", if before == self.view { " (no change)" } else { "" });
        console(self.grants, l.as_str().as_bytes());
    }

    /// Launch the Ramps app: its capsule from the vault, given the display, touch and the app surface (SYS_SPAWN: a child gets only what its parent holds). While it is in front the map takes no touches; it comes back when the display says no app is in front any more (after it has seen this one arrive).
    fn show_ramps(&mut self) {
        if ferros_sys::spawn(self.grants, b"ramps", b"ramps", &[S_DISPLAY, S_TOUCH, S_APPSURF]) == 0 {
            self.ramps = true;
            self.app_seen = false;
            self.app_since = now();
        } else {
            console(self.grants, b"map: could not launch ramps\n");
        }
    }

    /// While an app is in front: has it handed the screen back (or never arrived)?
    fn app_done(&mut self) -> bool {
        let mut r = [0u8; 16];
        let n = call(self.display, &[5], &mut r);
        let active = n >= 13 && r[12] == 1;
        if active {
            self.app_seen = true;
        }
        (self.app_seen && !active) || (!self.app_seen && now().wrapping_sub(self.app_since) > 2 * freq())
    }

    fn tap_inner(&mut self, fingers: usize, sx: i64, sy: i64) {
        if self.ramps {
            // On the ramps: one finger on a column shows it alone, one finger again shows them all; two fingers go back to the map.
            if fingers == 2 {
                self.ramps = false;
                self.ramps_only = None;
                self.draw();
                return;
            }
            self.ramps_only = match self.ramps_only {
                Some(_) => None,
                None => {
                    // The display server's column layout: nine columns between 4-pixel lines.
                    let col = (self.w - 4 * 10) / 9;
                    Some(((sx - 4).max(0) / (col + 4)).clamp(0, 8) as usize)
                }
            };
            let mut r = [0u8; 4];
            match self.ramps_only {
                Some(k) => call(self.display, &[21, k as u8], &mut r),
                None => call(self.display, &[21], &mut r),
            };
            return;
        }
        let inside = {
            let (dx, dy) = (sx - self.w / 2, sy - self.h / 2);
            dx * dx + dy * dy <= self.arrived_r() * self.arrived_r()
        };
        match (fingers, self.view) {
            (1, View::Map) => {
                if self.hit(sx, sy) == Some(RAMPS) {
                    self.show_ramps();
                } else if let Some(i) = self.hit(sx, sy) {
                    let to = self.at(i);
                    self.fly(to);
                    self.view = View::Front(i);
                    self.draw();
                }
            }
            (1, View::Front(SCOPE)) if inside => {
                // The focused scope: a tap folds it on its dominant period, or unfolds it (owner, 2026-10-25).
                self.scope_fold = !self.scope_fold;
                self.send_focus();
            }
            (1, View::Front(_)) | (1, View::Back(_)) => {
                if !inside {
                    self.view = View::Map;
                    let to = self.home();
                    self.fly(to);
                    self.draw();
                } else if self.view == View::Back(DEVICE) {
                    let row = self.rows[..self.nrows].iter().find(|r| sy >= r.0 && sy < r.1).map(|r| r.2);
                    match row {
                        Some(Choice::Base(code)) => {
                            ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_BASE, code);
                            self.draw();
                        }
                        Some(Choice::Brightness) => {
                            let (x0, step) = Map::dots(self.w / 2, self.arrived_r());
                            let k = ((sx - x0 + step / 2) / step).clamp(0, 8);
                            ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_BRIGHTNESS, (8 - k) as u8);
                            self.draw();
                        }
                        Some(Choice::Theme) => {
                            let next = (ferros_sys::theme::index() + 1) % ferros_sys::theme::THEMES.len();
                            ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_THEME, next as u8);
                            self.draw();
                        }
                        Some(Choice::Dither) => {
                            let spatial = ferros_sys::pref(ferros_sys::PREF_DITHER) == 1;
                            ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_DITHER, (!spatial) as u8);
                            self.draw();
                        }
                        Some(Choice::Labels) => {
                            let on = ferros_sys::pref(ferros_sys::PREF_LABELS) == 1;
                            ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_LABELS, (!on) as u8);
                            self.send_scene();
                            self.draw();
                        }
                        None => {}
                    }
                }
            }
            (2, View::Map) => {
                if let Some(i) = self.hit(sx, sy) {
                    let to = self.at(i);
                    self.fly(to);
                    self.view = View::Back(i);
                    self.draw();
                }
            }
            (2, View::Front(SCOPE)) => {
                // The focused scope: two fingers leave it (owner, 2026-10-25), back to the map.
                self.view = View::Map;
                let to = self.home();
                self.fly(to);
                self.draw();
            }
            (2, View::Front(i)) => {
                self.view = View::Back(i);
                self.draw();
            }
            (2, View::Back(i)) => {
                self.view = View::Front(i);
                self.draw();
            }
            _ => {}
        }
    }
}

/// "map: numbers in <base>", with two seconds written in it, in words (the console is a terminal surface).
fn say_base(grants: u64, code: u8) {
    let mut out = Line::new();
    let f = ferros_sys::numbers(Style::Words);
    let _ = write!(out, "map: numbers in {:?} (G#{:02X}); two seconds reads ", f.base, code);
    let _ = tatau::duration_ns(&mut out, f, 2_000_000_000, 1);
    out.s("\n");
    console(grants, out.as_str().as_bytes());
}

/// The distance between two panel points in sixteenths of a pixel, for the pinch.
fn dist16(a: (i64, i64), b: (i64, i64)) -> i64 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    isqrt((dx * dx + dy * dy) << 8)
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    // The circles' labels are `&'static str`, absolute addresses fixed at link time; linked position-independent, they are relocations this applies before anything reads them. Without it the first label read faulted at its link-time address, G#D9B (2026-10-21).
    unsafe { ferros_sys::relocate() };
    let display = cap(grants, S_DISPLAY);
    let touch = cap(grants, S_TOUCH);
    let mut r = [0u8; 256];

    // The panel's size, from the display server.
    if call(display, &[5], &mut r) < 4 {
        console(grants, b"map: no display server\n");
        ferros_sys::exit(1);
    }
    let (w, h) = (u16::from_le_bytes([r[0], r[1]]) as i64, u16::from_le_bytes([r[2], r[3]]) as i64);

    // The circles, sized by share and placed once.
    let rs: [i64; N] = core::array::from_fn(|i| radius_of(CIRCLES[i].share));
    let mut xs = [0i64; N];
    let mut ys = [0i64; N];
    place(&rs, &mut xs, &mut ys);
    let mut map = Map {
        grants,
        display,
        w,
        h,
        xs: xs.map(|x| x << 8),
        ys: ys.map(|y| y << 8),
        rs,
        cam: Cam { x: 0, y: 0, z: 1 << 16, c: ONE, s: 0 },
        view: View::Map,
        rows: [(0, 0, Choice::Labels); 8],
        nrows: 0,
        rc: [ONE; N],
        rsn: [0; N],
        arranged: [false; N],
        ramps_only: None,
        ramps: false,
        scope_span_us: 1_000_000,
        scope_axis: 0,
        scope_acc: 0,
        scope_fold: false,
        // Nothing sent yet, whatever the display holds: a display server that outlived a map restarted while the scope was in focus kept flipping at the panel's rate, and the clock policy saw a display animating and held the active clocks (2.9 W at rest, 2026-10-25). The first frame always says.
        focus_sent: (0xFF, 0, false),
        app_seen: false,
        app_since: 0,
    };
    let kept = map.load_places();
    // Whatever the vault held, no two circles start overlapping (places kept before circles pushed each other could).
    map.push_apart(N);
    if kept > 0 {
        let mut l = Line::new();
        let _ = write!(l, "map: {kept} circle(s) where the owner put them\n");
        console(grants, l.as_str().as_bytes());
    }
    map.send_scene();
    map.cam = map.home();
    if map.load_view() {
        console(grants, b"map: the view as it was left\n");
    }
    let mut saved_view = map.cam;
    // A development build can start arrived somewhere, to see a front or a back without a finger: FERROS_MAP_START=f<n> or b<n> (circle n).
    if let Some(v) = option_env!("FERROS_MAP_START").map(str::as_bytes) {
        if v.len() == 2 && (v[1] as usize).wrapping_sub(b'0' as usize) < N {
            let i = (v[1] - b'0') as usize;
            map.cam = map.at(i);
            map.view = if v[0] == b'b' { View::Back(i) } else { View::Front(i) };
        }
    }
    map.draw();
    if option_env!("FERROS_MAP_START") == Some("rr") {
        map.show_ramps();
    }
    // A development build can time the frame loop without a finger (FERROS_MAP_START=bench): 32 frames of a pinch with a twist, then 32 of a pan, each the whole round trip to the display server; the averages to the console, then the camera back home.
    // FERROS_MAP_START=ipc: the bare cost of a round trip to each server (64 trivial calls each: display info, touch info).
    if option_env!("FERROS_MAP_START") == Some("ipc") {
        for (name, ep, op) in [("display", display, 5u8), ("touch", touch, 2u8)] {
            let t0 = now();
            let mut worst = 0u64;
            for _ in 0..64 {
                let t = now();
                call(ep, &[op], &mut r);
                worst = worst.max(ferros_sys::us_since(t));
            }
            let mut l = Line::new();
            let _ = write!(l, "map: ipc {name}: {} us a round trip, worst {} us\n", ferros_sys::us_since(t0) / 64, worst);
            console(grants, l.as_str().as_bytes());
        }
    }
    if option_env!("FERROS_MAP_START") == Some("bench") {
        let home = map.cam;
        for (name, pinch) in [("pinch+twist", true), ("pan", false)] {
            let t0 = now();
            for k in 0..32i64 {
                if pinch {
                    map.cam.z = map.cam.z * 1030 / 1000;
                    let (c, sn) = (map.cam.c * 16364 / ONE - map.cam.s * 572 / ONE, map.cam.s * 16364 / ONE + map.cam.c * 572 / ONE);
                    map.cam.c = c;
                    map.cam.s = sn;
                } else {
                    map.cam.x += 7 << 8;
                    map.cam.y += (k & 3) << 8;
                }
                map.draw();
            }
            let us = ferros_sys::us_since(t0) / 32;
            let mut l = Line::new();
            let _ = write!(l, "map: bench {name}: {} us a frame ({} fps)\n", us, 1_000_000 / us.max(1));
            console(grants, l.as_str().as_bytes());
            map.cam = home;
        }
        map.draw();
    }
    console(grants, b"map: home drawn; tap arrives, two fingers turn a circle over\n");
    // FERROS_MAP_START=crowd: 200 more circles in the display's scene only (never the vault), for timing a busy home screen.
    if option_env!("FERROS_MAP_START") == Some("crowd") {
        let placed = map.crowd(200);
        let mut l = Line::new();
        let _ = write!(l, "map: crowd of {} circles added\n", placed);
        console(grants, l.as_str().as_bytes());
        map.draw();
    }
    if let Some(mode) = option_env!("FERROS_MAP_START") {
        let mut l = Line::new();
        let _ = write!(l, "map: dev start mode {mode}\n");
        console(grants, l.as_str().as_bytes());
    }
    // The device's number base, as every app reads it (SETTINGS.md): logged now and whenever it changes, which is the proof the preference reaches a running process.
    let mut base = ferros_sys::device_base();
    say_base(grants, base);
    let mut labels = ferros_sys::pref(ferros_sys::PREF_LABELS);
    let mut theme_seen = ferros_sys::pref(ferros_sys::PREF_THEME);

    // The previous frame's held contacts, by contact id: present, x, y.
    let mut prev = [(false, 0i64, 0i64); 16];
    // The touch session: from the first finger down to the last one up.
    let mut down = false;
    let mut most = 0usize;
    let mut moved = false;
    let mut start = [(false, 0i64, 0i64); 16];
    // The circle the fingers hold, decided again whenever the number of moving fingers changes (one finger: the circle it started on; two: the circle both are inside), and for how many fingers it was decided.
    let mut grab: Option<usize> = None;
    let mut grab_n = 0usize;
    let mut last_app_check = 0u64;
    let mut last_dashboard = now();
    let mut last_pass = now();
    let mut draw_us = 0u64;
    let mut move_us = 0u64;
    let mut last_ignored = 0u16;
    let mut push_us = 0u64;
    // FERROS_MAP_START=drag: a synthetic finger instead of the touch server for the first 192 passes — two seconds after start (past the display's first full frame), a pan through the air and back, so the frame pacing can be measured with no one at the panel (it logs passes, draws and the rate).
    let mut synth = 0u32;
    // FERROS_MAP_START=drag: the drag's own rate, for measuring the frame pacing with no one at the panel.
    let (mut synth_t0, mut synth_draws, mut synth_at) = (0u64, 0u32, (0i64, 0i64));
    let map_start = now();
    // The display's frame-start count, from the last frame wait (op 24).
    let (mut frame_count, mut synth_f0) = (0u32, 0u32);
    loop {
        // A preference changed (here or from the bridge): redraw in it.
        let b = ferros_sys::device_base();
        if b != base {
            base = b;
            say_base(grants, base);
            map.draw();
        }
        let l = ferros_sys::pref(ferros_sys::PREF_LABELS);
        if l != labels {
            labels = l;
            map.send_scene();
            map.draw();
        }
        // A new theme: everything drawn again in it.
        let tm = ferros_sys::pref(ferros_sys::PREF_THEME);
        if tm != theme_seen {
            theme_seen = tm;
            map.draw();
        }
        // The dashboard is live: once a second while it is showing.
        if map.view == View::Front(DEVICE) && now().wrapping_sub(last_dashboard) > freq() {
            last_dashboard = now();
            map.draw();
        }

        // An app the map did not launch (a bridge spawn: Hive, for one) stands in front the same way: asked of the display eight times a second.
        if !map.ramps && now().wrapping_sub(last_app_check) > freq() / 8 {
            last_app_check = now();
            let mut q = [0u8; 16];
            if call(map.display, &[5], &mut q) >= 13 && q[12] == 1 {
                map.ramps = true;
                map.app_seen = true;
            }
        }
        // An app in front: it has the touches; the map only watches for it to hand the screen back.
        if map.ramps {
            if map.app_done() {
                map.ramps = false;
                map.draw();
            } else {
                sleep_until(now() + freq() / 10);
                continue;
            }
        }
        // The screen off (the power key): no touches reach the map.
        let t_loop = now();
        let n = if matches!(option_env!("FERROS_MAP_START"), Some("drag") | Some("crowd")) && synth < 192 && now().wrapping_sub(map_start) > 2 * freq() {
            synth += 1;
            if synth == 2 {
                // From the second pass: the first frame wait's reply is in.
                synth_t0 = now();
                synth_draws = 0;
                synth_f0 = frame_count;
            }
            if synth == 1 {
                // An empty spot to put the finger on: up the middle from the bottom until no circle is under it.
                synth_at = (w / 2, h - 160);
                let mut y = h - 160;
                while y > 160 && map.hit(w / 2, y).is_some() {
                    y -= 40;
                }
                synth_at.1 = y;
            }
            if synth == 181 {
                let mut l = Line::new();
                let ms = ferros_sys::us_since(synth_t0) / 1000;
                let frames = frame_count.wrapping_sub(synth_f0);
                let _ = write!(l, "map: drag 179 passes in {} ms, {} draws, {} frames on the panel: {} frames/s\n", ms, synth_draws, frames, frames as u64 * 1000 / ms.max(1));
                console(grants, l.as_str().as_bytes());
            }
            if synth > 180 {
                r[0] = 0;
                1
            } else {
                // A pan through the air, 90 passes up and 90 back, so the plane ends where it was and the lift is no tap.
                let k = synth as i64 - 1;
                let off = if k < 90 { k * 6 } else { (180 - k) * 6 };
                let (x, y) = (synth_at.0, synth_at.1 - off);
                r[0] = 1;
                r[1..13].fill(0);
                r[3] = 1;
                r[5..7].copy_from_slice(&(x.clamp(0, w - 1) as u16).to_le_bytes());
                r[7..9].copy_from_slice(&(y.clamp(0, h - 1) as u16).to_le_bytes());
                13
            }
        } else if ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) == 1 { 0 } else { call(touch, &[1], &mut r) as usize };
        let touch_us = ferros_sys::us_since(t_loop);
        let mut cur = [(false, 0i64, 0i64); 16];
        let mut ids = [0usize; 16];
        let mut held = 0usize;
        let mut ignored = 0u16;
        if n >= 1 && n != ferros_sys::ERR as usize {
            let count = (r[0] as usize).min(16);
            for k in 0..count {
                if 1 + (k + 1) * 12 > n {
                    break;
                }
                let rec = &r[1 + k * 12..1 + (k + 1) * 12];
                let id = (rec[0] as usize) & 15;
                // Only fingers, gloves and styluses count (FTM5 touch types 1-3): a palm (4), a hover (0) or the grip of the hand holding the phone at the edge (7) is not a finger — counted as one, it turned a one-finger drag into a two-finger twist around the palm (owner, 2026-10-23: "it's spinning around when I only have one finger down").
                if !(1..=3).contains(&rec[2]) {
                    ignored |= 1 << rec[2].min(15);
                    continue;
                }
                cur[id] = (true, u16::from_le_bytes([rec[4], rec[5]]) as i64, u16::from_le_bytes([rec[6], rec[7]]) as i64);
                ids[held] = id;
                held += 1;
            }
        }

        // The session: when it began, how many fingers it ever held, where each landed, and whether any went further than a resting finger does.
        if held > 0 && !down {
            down = true;
            most = 0;
            moved = false;
            start = [(false, 0, 0); 16];
            grab = None;
            grab_n = 0;
            map.scope_axis = 0;
            map.scope_acc = 0;
        }
        if down {
            most = most.max(held);
            for &id in ids[..held].iter() {
                if !start[id].0 {
                    start[id] = cur[id];
                }
                let (dx, dy) = (cur[id].1 - start[id].1, cur[id].2 - start[id].2);
                if dx * dx + dy * dy > SLOP * SLOP {
                    moved = true;
                }
            }
        }
        if held == 0 && down {
            down = false;
            // Fingers up: the world back to zoom one, then whatever moved (or everything, after a rescale) into the vault, and the view if it changed. The same picture is drawn once more in the rescaled world, so the display holds a frame that matches its scene (its clock redraws replay it).
            map.renormalise();
            map.draw();
            map.save_arranged();
            if map.cam != saved_view {
                if map.save_view() {
                    saved_view = map.cam;
                } else {
                    console(grants, b"map: the vault did not take the view\n");
                }
            }
            if !moved && most <= 2 {
                // Where the fingers landed, on average.
                let (mut sx, mut sy, mut k) = (0i64, 0i64, 0i64);
                for s in start.iter().filter(|s| s.0) {
                    sx += s.1;
                    sy += s.2;
                    k += 1;
                }
                if k > 0 {
                    map.tap(most, sx / k, sy / k);
                }
            }
        }

        // Drags and pinches, once the fingers have moved past the slop: the contacts present both now and last frame are the ones that moved.
        if moved {
            let mut both = [0usize; 2];
            let mut nb = 0usize;
            for &id in ids[..held].iter() {
                if prev[id].0 && nb < 2 {
                    both[nb] = id;
                    nb += 1;
                }
            }
            let before = map.cam;
            let mut scene_moved = false;
            if nb != grab_n && nb > 0 {
                grab_n = nb;
                grab = if nb == 1 {
                    map.grasp(start[both[0]].1, start[both[0]].2)
                } else {
                    let (ga, gb) = (map.grasp(prev[both[0]].1, prev[both[0]].2), map.grasp(prev[both[1]].1, prev[both[1]].2));
                    if ga == gb { ga } else { None }
                };
            }
            // One finger on the focused scope works the scope, not the plane.
            let on_scope = nb == 1 && map.view == View::Front(SCOPE);
            if on_scope {
                let id = both[0];
                map.scope_drag(cur[id].1 - prev[id].1, cur[id].2 - prev[id].2, cur[id].1 - start[id].1, cur[id].2 - start[id].2);
            }
            if !on_scope && nb >= 1 && map.view != View::Map {
                // Moving away from an arrived circle leaves it: the overlay goes, the plane is free again.
                map.view = View::Map;
            }
            if nb == 1 && !on_scope {
                let id = both[0];
                let (dx, dy) = map.cam.delta(cur[id].1 - prev[id].1, cur[id].2 - prev[id].2);
                if let Some(i) = grab {
                    // One finger from a circle: the circle follows it, pushing the others aside — as far as everything still fits at the zoom-out limit.
                    let t_m = now();
                    let (dx, dy) = if map.fits_when_out(i, dx, dy) { (dx, dy) } else { (0, 0) };
                    map.xs[i] += dx;
                    map.ys[i] += dy;
                    map.send_move(i);
                    map.arranged[i] = true;
                    move_us += ferros_sys::us_since(t_m);
                    let t_p = now();
                    map.push_apart(i);
                    push_us += ferros_sys::us_since(t_p);
                    scene_moved = true;
                } else {
                    // One finger from the air: the plane follows it.
                    map.cam.x -= dx;
                    map.cam.y -= dy;
                    map.keep_on_screen();
                }
            } else if nb == 2 {
                let (a, b) = (both[0], both[1]);
                let (pa, pb, ca, cb) = ((prev[a].1, prev[a].2), (prev[b].1, prev[b].2), (cur[a].1, cur[a].2), (cur[b].1, cur[b].2));
                let (d0, d1) = (dist16(pa, pb), dist16(ca, cb));
                let (m0x, m0y) = ((pa.0 + pb.0) / 2, (pa.1 + pb.1) / 2);
                let (m1x, m1y) = ((ca.0 + cb.0) / 2, (ca.1 + cb.1) / 2);
                if let Some(i) = grab {
                    // Both fingers in one circle: it follows the point between them and grows or shrinks with their spread.
                    let (dx, dy) = map.cam.delta(m1x - m0x, m1y - m0y);
                    let (dx, dy) = if map.fits_when_out(i, dx, dy) { (dx, dy) } else { (0, 0) };
                    map.xs[i] += dx;
                    map.ys[i] += dy;
                    if d0 > 8 * 16 && d1 > 8 * 16 {
                        let (lo, hi) = map.radius_limits(i);
                        map.rs[i] = (map.rs[i] * d1 / d0).clamp(lo, hi.max(lo));
                        // And turns with their twist, as the plane does.
                        let (ax, ay, bx, by) = (pb.0 - pa.0, pb.1 - pa.1, cb.0 - ca.0, cb.1 - ca.1);
                        let (dot, cross) = (ax * bx + ay * by, ax * by - ay * bx);
                        let ab = (d0 * d1 >> 8).max(1);
                        let (c, sn) = ((map.rc[i] * dot - map.rsn[i] * cross) / ab, (map.rsn[i] * dot + map.rc[i] * cross) / ab);
                        let len = isqrt(c * c + sn * sn).max(1);
                        map.rc[i] = c * ONE / len;
                        map.rsn[i] = sn * ONE / len;
                    }
                    map.send_move(i);
                    map.arranged[i] = true;
                    map.push_apart(i);
                    scene_moved = true;
                } else if d0 > 8 * 16 && d1 > 8 * 16 {
                    // Two fingers: zoom by how much they spread and turn by how much they twist, about the point between them, so what is under your fingers stays under your fingers.
                    let (wx, wy) = map.cam.world(w, h, m0x, m0y);
                    let (lo, hi) = map.zoom_limits();
                    map.cam.z = (map.cam.z * d1 / d0).clamp(lo, hi);
                    // The twist between the two finger vectors, as (cos, sin) = (dot, cross) / (|a| |b|), composed onto the camera's own turn and brought back to unit length.
                    let (ax, ay, bx, by) = (pb.0 - pa.0, pb.1 - pa.1, cb.0 - ca.0, cb.1 - ca.1);
                    let (dot, cross) = (ax * bx + ay * by, ax * by - ay * bx);
                    // |a| |b| from the two spreads (sixteenths of a pixel each), so the product comes back near Q14 before the exact renormalising square root.
                    let ab = (d0 * d1 >> 8).max(1);
                    let (c, s) = ((map.cam.c * dot - map.cam.s * cross) / ab, (map.cam.s * dot + map.cam.c * cross) / ab);
                    let len = isqrt(c * c + s * s).max(1);
                    map.cam.c = c * ONE / len;
                    map.cam.s = s * ONE / len;
                    let (dx, dy) = map.cam.delta(m1x - w / 2, m1y - h / 2);
                    map.cam.x = wx - dx;
                    map.cam.y = wy - dy;
                    map.keep_on_screen();
                }
            }
            let t_draw = now();
            if map.cam != before || scene_moved {
                map.draw();
                synth_draws += 1;
            }
            draw_us = ferros_sys::us_since(t_draw);
        }
        // A hitch, measured (owner, 2026-10-23: "smooth for a quarter second, then it hangs"): a pass with a finger down that took over 50 ms from the last one, split into the touch read and the frame.
        let pass_us = ferros_sys::us_since(last_pass);
        if held > 0 && pass_us > 50_000 {
            let mut l = Line::new();
            let _ = write!(l, "map: stall {} ms (touch read {} ms, move call {} ms, push {} ms, frame {} ms, {} fingers, other contact kinds G#{:X})\n", pass_us / 1000, touch_us / 1000, move_us / 1000, push_us / 1000, draw_us / 1000, held, ignored);
            console(grants, l.as_str().as_bytes());
        }
        if ignored != 0 && ignored != last_ignored {
            let mut l = Line::new();
            let _ = write!(l, "map: ignoring contacts that are not fingers (kinds G#{:X})\n", ignored);
            console(grants, l.as_str().as_bytes());
        }
        last_ignored = ignored;
        last_pass = now();
        draw_us = 0;
        move_us = 0;
        push_us = 0;
        prev = cur;
        // Locked to the panel while a finger is down: each pass starts when the display's next frame has started (op 24, `frame wait`), so the touch is read fresh right after a refresh begins and the update is ready for the next one — one update a refresh while it fits, every other refresh (steady, no beat) when it does not. Was a fixed 1/120 s after the work: a period of work + 8.3 ms, about 60 a second (2026-10-24); the display replies to a frame before drawing it, so the frame call itself paces nothing. A thirtieth at rest.
        if held == 0 {
            sleep_until(now() + freq() / 30);
        } else {
            let mut fr = [0u8; 4];
            call(display, &[24], &mut fr);
            frame_count = u32::from_le_bytes(fr);
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
