//! The map (SHELL.md): the device is a map of your life. Every person, message, photo and app is a circle on one plane; the view is a camera over it, and the camera is the only thing that moves.
//!
//! The circles: one is built in, the system's own: the device's settings, one side only (owner, 2026-11-05: "that should only have one side, it's the OS settings page"). Nothing else is (owner, 2026-11-04: "For the baked in circles, there should be one. System settings, that's it. You want a clock? INSTALL IT YOURSELF!!!"): every other circle is a kist the person installs (docs/GLOSSARY.md: the signed capsule as this device checked, compiled and sealed it), a kist circle, and the map knows nothing about it until then. The placement below (a seeded random fit with generous air) is for when there are more.
//!
//! The gestures (owner, 2026-10-22: "two finger scale/move for edits and back to a normal two finger vs one finger for the back"):
//! - One finger dragged from the air moves the plane; dragged from a circle, it moves that circle.
//! - Two fingers both inside one circle move and size that circle; otherwise they pinch to zoom and twist to turn the plane, about the point between them.
//! - Only circles smaller than three quarters of the screen's width can be taken hold of: zoomed in until a circle fills the view, it is a place, and the fingers move the plane again.
//! - A one-finger tap on a circle arrives at it: the next frame has its page inscribed in the circle an app lives in (ferros_sys::active_circle, the display's hairline; owner 2026-11-05: "each active app has the same theme, circle inscribed"). A tap in the air around an arrived circle leaves it.
//! - Whatever was moved is kept in the vault when the fingers lift (owner, 2026-10-22: arranging is always on, no mode).
//! - Nothing is animated and nothing is timed (owner, 2026-10-22: "no animations ... instant, always, no timers"): a view is simply there on the next frame, and a tap is fingers down and up without moving, however long they rested.
//!
//! All of it integer: the camera centre in world units ×256, the zoom in 16.16, pinches by integer square roots.
//!
//! Positions are deterministic until the person arranges by hand; then each moved circle is kept in the vault (`map.place.<name>`) and wins over the seeded place from then on.
//!
//! Started by init (payloads/init), only once a person has attested (it reads their space). Grant slots: 0 console, 1 display (use), 2 touch (use), 3 settings, 4 the vault scope `map.`, 5 the identity service (use fgtw: the Fleet page, and the identity space: the arrangement and kists written from the person's fleet are read again when its generation moves, docs/IDENTITY-SPACE.md).

#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::Write;
use ferros_sys::{call, cap, console, freq, now, sleep_until};
use tatau::Style;

const S_DISPLAY: u64 = 1;
const S_TOUCH: u64 = 2;
const S_SETTINGS: u64 = 3;
const S_STORE: u64 = 4;
/// The identity service (payloads/fgtw): the Fleet page asks it the fleet's state and members, and starts this device's departure.
const S_FGTW: u64 = 5;
/// The vault scope `kists.`: the person's kists and their answers (ferros_kist), read for the kist circles and the settings, written with the person's answers.
const S_KIST_LIST: u64 = 6;
/// Init's `launch` endpoint: a kist circle tapped asks init to launch it (the map holds none of a kist's rights).
const S_LAUNCH: u64 = 7;

/// The heap a link's detail document is verified and read in (ferros_detail): a few kilobytes while a link's page shows, freed after each frame.
const HEAP_SIZE: usize = 1 << 18;
#[repr(C, align(64))]
struct Heap([u8; HEAP_SIZE]);
static mut HEAP: Heap = Heap([0; HEAP_SIZE]);
#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

/// One circle: its label, its weight of ring (0 none, 1 thin, 2 heavy — importance), whether it is lit (changed since last seen), its share of the map's area in thousandths, and its icon in the display server's bundle (tools/app-icons order).
struct Spec<'a> {
    label: &'a str,
    ring: u8,
    lit: u8,
    share: u32,
    icon: u8,
}

/// The one built-in circle: the system's own, the device's settings. It never lights (lit means a person sent you something).
const CIRCLES: [Spec<'static>; 1] = [Spec { label: "settings", ring: 1, lit: 0, share: 250, icon: 6 }];
/// The most circles the map holds: settings and the kists the person installed (KISTS.md), each a kist circle after it.
const MAX: usize = 1 << 4;
/// A kist circle's share of the map's area until it is sized by hand (thousandths, as `Spec::share`).
const KIST_SHARE: u32 = 125;

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

/// The controls (owner, 2026-11-08: "add in the fluor elements like sliders (for brightness) and buttons", then "radio buttons and checkboxes ... very against the switch design"): fluor's button and slider, a radio's ring and a checkbox's squircle, drawn by the display (ops 47-50, COMPOSITOR.md "Controls").
/// A button's colours by theme, G#RRGGBB VSF RGB codes at gamma 2 as every op's: its fill, its fill while held (or armed, the first tap of two), and its edge on the top-left and the bottom-right of the anti-diagonal, light then shadow, lit from the top left as the keys are (owner, 2026-11-09; until then shadow then light, fluor's old raised bevel, which read sunken) (fluor's BUTTON_FILL, BUTTON_HELD, TEXTBOX_SHADOW_EDGE and TEXTBOX_LIGHT_EDGE on grey, whose dark codes are the same at gamma 2; the other themes' in their own hues).
/// The Fleet page's Add on a selected device: VSF RGB primary green (0, 1, 0), its held fill and its edge darker greens, the label black (owner, 2026-11-10: "a distinct green add").
const GREEN: u32 = 0x00FF00;
const GREEN_HELD: u32 = 0x00A000;
const GREEN_EDGE: u32 = 0x006000;
const BUTTON_LOOK: [[u32; 4]; 4] = [
    [0x1A214C, 0x3C4CA8, 0x423F36, 0x2A3336],
    [0x162238, 0x2E4C78, 0x48525E, 0x1A2028],
    [0x3A2414, 0x6E4424, 0x5A4A3A, 0x241A12],
    [0x380A06, 0x6A160C, 0x4A120C, 0x180404],
];
/// The radio's and the checkbox's look, one of three for the owner to pick from (2026-11-08): 0 a pixel's stroke and the fill alike in the theme's mark colour (white in the default theme, owner 2026-11-08: "no amber"); 1 that hairline round a fill of the theme's text colour, the checkbox ticked in the ground's; 2 fluor's stroke (a 32nd of the label's size, plus a pixel) in the secondary text colour, filled with the theme's lit colour and ticked.
const MARK_LOOK: u8 = 0;
/// The circles' hairline colour, as the display draws the app circle's (display `hairline`): on release, grey (equal codes) at half the photopic light of the theme's ring (VSF RGB's weights on its codes, squared); on development builds, VSF RGB orange, full red and green at half its light.
fn hairline() -> u32 {
    if DEV_LOOK {
        return 0xFF_B4_00;
    }
    let ring = ferros_sys::theme::current().ring;
    let lin = |sh: u32| {
        let c = (ring >> sh & 0xFF) as f32 * (1. / 255.);
        c * c
    };
    let light = 0.2584 * lin(16) + 0.6346 * lin(8) + 0.1070 * lin(0);
    let code = (libm::sqrtf(light * 0.5) * 255.) as u32;
    code << 16 | code << 8 | code
}
/// A mark's stroke width (Q8 pixels) and colours (stroke, fill, ink unused here, tick) and whether a checked box carries its tick, for the label size `px`, by `MARK_LOOK`.
fn mark_look(px: i64) -> (i64, [u32; 4], bool) {
    // The stroke the buttons carry (an em over 32, plus a pixel: fluor's widgets), never under 2 pixels (owner, 2026-11-09: "2px ... min, follow the same button stroke so if they get big we want to have more than 2px"; a pixel's stroke was too thin and its anti-aliasing jittered).
    let sw = (((px >> 5) + 1).max(2)) << 8;
    match MARK_LOOK {
        0 => {
            let m = ferros_sys::theme::current().mark;
            (sw, [m, m, 0, fill()], false)
        }
        1 => (sw, [hairline(), white(), 0, fill()], true),
        _ => (sw, [grey(), ferros_sys::theme::current().lit, 0, fill()], true),
    }
}

/// Movement under this many panel pixels is a finger resting, not a drag: a tap may wobble this much.
const SLOP: i64 = 24;
/// One, in the rotation's Q14.
const ONE: i64 = 1 << 14;

/// The rate of every motion after a finger lifts or a tap: the halving's, ln 2 a display frame (owner, 2026-11-05: halve what is left each frame), as a critically damped spring (owner, 2026-11-06: "just needs some ease").
const SPRING: f64 = core::f64::consts::LN_2;

/// A critically damped spring to 0, `t` display frames after it started at offset `x0` with speed `v0` (a frame): its offset and speed then. x(t) = (x₀ + (v₀ + ωx₀)t)·e^(−ωt): it leaves at v₀ (from rest, easing in) and comes to 0 with every derivative going to 0, passing it at most once.
fn spring(x0: f64, v0: f64, t: i32) -> (f64, f64) {
    spring_at(x0, v0, t, SPRING)
}

/// The critically damped spring at rate `w`: x(t) = (x0 + (v0 + w·x0)·t)·e^(−w·t), and its speed.
fn spring_at(x0: f64, v0: f64, t: i32, w: f64) -> (f64, f64) {
    let t = t as f64;
    let e = libm::exp(-w * t);
    let a = v0 + w * x0;
    ((x0 + a * t) * e, (v0 - w * a * t) * e)
}

/// The friction on a fling, the plane's pan and a page's scroll alike: true, the owner's pixel a frame per frame that fades out as the fling stops (f = −v/√(1 + v²): 1 at any real speed, so the same reach, v²/2, and easing to nothing at the stop, the algebraic sigmoid, so the stop has no kink); false, the pixel a frame per frame exactly (owner, 2026-11-05), which stops dead (owner, 2026-11-06: "let's try b, but I'ma think we'll go for a at the end"; tried, b was "too slippery" — its friction fades with the speed, so the last pixels creep on — "let's go with a").
const FRICTION_FADES: bool = false;

/// One display frame of a fling at speed `v` (pixels a frame, signed): how far it goes and its speed after. Fading, the frame integrated in sixteen steps; at rest once under a sixteenth of a pixel a frame, when all it has left to go is under a sixteenth of a pixel.
fn coast(v: f64) -> (f64, f64) {
    if FRICTION_FADES {
        const SUB: i32 = 1 << 4;
        let dt = 1. / SUB as f64;
        let (mut v, mut d) = (v, 0.);
        for _ in 0..SUB {
            d += v * dt;
            v -= dt * v / libm::sqrt(1. + v * v);
        }
        (d, if v.abs() < 1. / 16. { 0. } else { v })
    } else {
        (v, if v.abs() <= 1. { 0. } else { v - v.signum() })
    }
}

/// A camera flight in progress (`Map::fly`): where each part of the camera stood from the target when it began, in the space it looks even in — the centre's world offset (×256), the zoom's natural logarithm against the target's, the turn's angle from the target's in radians, and the fade between the plane and the open page (COMPOSITOR.md "The flight, prerendered") against where it goes — each part's speed then (a display frame), the fade's end, and the frames since.
#[derive(Clone, Copy)]
struct Flight {
    off: [f64; 5],
    vel: [f64; 5],
    fade_to: f64,
    t: i32,
}

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

/// The longest device name the Fleet page keeps: two voca words (fgtw::pair::device_name_default) are at most 36 bytes.
const NAME_BYTES: usize = 1 << 6;

/// The identity service as the map knows it (payloads/fgtw ops 1, 6, 9, 13 and 17), read by `fgtw_poller` each time the service's doorbell (op 21) rings, never on a timer; the main loop takes a copy (`fleet_take`). The page's own state (`confirm`, `leave_result`, `sent`, `left`) is the main loop's and survives each copy.
#[derive(Clone, Copy)]
struct Fleet {
    /// None until the service answered at all (it starts only once a person has attested).
    state: Option<u8>,
    /// The person's identity space (fgtw op 17): this device's home-screen sync, and whether the first pull from their fleet is done. None until the service answered it.
    home: Option<(bool, bool)>,
    /// The space's generation (op 17): a new one means the circles are read again.
    space_gen: Option<u32>,
    /// This device's departure asked for and not yet showing as under way (the second tap on remove, before the service says leaving): the page says so at once.
    sent: bool,
    /// A sibling's departure approved here and gone from the service since: its name, said on the page until the next tap.
    left: [u8; NAME_BYTES],
    left_n: usize,
    words: [u8; 64],
    words_n: usize,
    keys: [[u8; 8]; 16],
    me: [bool; 16],
    /// Each member heard on this network (fgtw op 6, flag bit 1).
    heard: [bool; 16],
    /// Each member's two-word device name (fgtw op 6's names), `names_n` bytes of it; 0 when the service gave none, and the row shows the key prefix instead.
    names: [[u8; NAME_BYTES]; 16],
    names_n: [u8; 16],
    n: usize,
    /// The remove row's first tap: what goes with the device is shown, and the next tap on it starts the departure.
    confirm: bool,
    /// What the last departure request answered (fgtw `leave`: 0 started, 1 not a member, 2 a bad intent, 3 the request did not build).
    leave_result: Option<u8>,
    /// Adding a device (fgtw op 9): the flow's phase (payloads/fgtw ADD_*: 0 closed, 1 open, 2 adding, 3 a device selected and Add waiting, 4 handing over the key, 5 added, 6 failed), the devices asking to join (the key's first 8 bytes, flags: bit 0 its beacon heard, bit 1 its request heard on this network, bit 2 the one just bound; its name), and the service's note.
    add: u8,
    cand_key: [[u8; 8]; 8],
    cand_flags: [u8; 8],
    cand_name: [[u8; NAME_BYTES]; 8],
    cand_name_n: [u8; 8],
    cands: usize,
    add_note: [u8; 64],
    add_note_n: usize,
    /// The device selected (phase 3, fgtw ADD_SELECTED): its name and its masked words, the same it shows.
    sel_name: [u8; NAME_BYTES],
    sel_name_n: usize,
    sel_words: [u8; 512],
    sel_words_n: usize,
    /// A sibling's departure waiting for approval here (fgtw op 13): its intent and how the approval stands (0 waiting, 1 approving, 2 approved, 3 failed), and its name.
    depart: Option<(u8, u8)>,
    depart_name: [u8; NAME_BYTES],
    depart_name_n: usize,
    /// This device's own departure denied (fgtw op 13 [2]): the name of the device that denied it.
    denied_by: [u8; NAME_BYTES],
    denied_by_n: usize,
}

impl Fleet {
    const fn new() -> Self {
        Fleet { state: None, words: [0; 64], words_n: 0, keys: [[0; 8]; 16], me: [false; 16], heard: [false; 16], names: [[0; NAME_BYTES]; 16], names_n: [0; 16], n: 0, confirm: false, leave_result: None, add: 0, cand_key: [[0; 8]; 8], cand_flags: [0; 8], cand_name: [[0; NAME_BYTES]; 8], cand_name_n: [0; 8], cands: 0, add_note: [0; 64], add_note_n: 0, sel_name: [0; NAME_BYTES], sel_name_n: 0, sel_words: [0; 512], sel_words_n: 0, depart: None, depart_name: [0; NAME_BYTES], depart_name_n: 0, denied_by: [0; NAME_BYTES], denied_by_n: 0, home: None, space_gen: None, sent: false, left: [0; NAME_BYTES], left_n: 0 }
    }

    /// A fresh copy from `fgtw_poller` taken in, the page's own state kept: the remove row's arming, the last request's answer, a departure sent and not yet under way, and a departure approved here that has now gone (said as left).
    fn take(&mut self, s: Fleet) {
        let (confirm, leave_result, sent, mut left, mut left_n) = (self.confirm, self.leave_result, self.sent, self.left, self.left_n);
        // Gone after this device approved it (1 approving, 2 approved): it left. Gone after a deny (4) is no departure.
        if let (Some((_, 1 | 2)), None) = (self.depart, s.depart) {
            left = self.depart_name;
            left_n = self.depart_name_n;
        }
        // A decision made here (1 approving, 4 denying) stands over a copy read before the service took it (still 0, waiting).
        let decided = self.depart.filter(|d| d.1 == 1 || d.1 == 4);
        *self = s;
        if let (Some(d), Some((_, 0))) = (decided, s.depart) {
            self.depart = Some(d);
        }
        self.confirm = confirm;
        self.leave_result = leave_result;
        self.sent = sent && s.state != Some(8);
        self.left = left;
        self.left_n = left_n;
    }

    /// The service's state and words (`[state][siblings heard][words]`), then its members (`[count][count × (8 key bytes, flags)]`). CALL_LIVE: a service that is not running answers at once instead of holding the page.
    fn poll(&mut self, svc: u64) {
        let mut r = [0u8; 80];
        let n = ferros_sys::call_live(svc, &[1], &mut r);
        if n == ferros_sys::ERR || n < 2 {
            self.state = None;
            return;
        }
        self.state = Some(r[0]);
        let w = (n as usize - 2).min(self.words.len());
        self.words[..w].copy_from_slice(&r[2..2 + w]);
        self.words_n = w;

        let mut s = [0u8; 8];
        let ok = ferros_sys::call_live(svc, &[17], &mut s) == 8;
        self.home = ok.then(|| (s[4] == 1, s[5] == 1));
        self.space_gen = ok.then(|| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
        let mut m = [0u8; 1 + 16 * 9 + 16 * (1 + NAME_BYTES)];
        let k = ferros_sys::call_live(svc, &[6], &mut m);
        if k != ferros_sys::ERR && k >= 1 {
            // WHY: the count is the service's word; the rows read are those that both fit the reply and the table. The length is the reply as the service sent it, counted only as far as the buffer took it.
            let got = (k as usize).min(m.len());
            let cnt = (m[0] as usize).min(16).min((got - 1) / 9);
            for i in 0..cnt {
                self.keys[i].copy_from_slice(&m[1 + i * 9..9 + i * 9]);
                self.me[i] = m[9 + i * 9] & 1 == 1;
                self.heard[i] = m[9 + i * 9] & 2 == 2;
                self.names_n[i] = 0;
            }
            // The names after the rows (`[length][UTF-8]` each, in row order). WHY the checks: the lengths are another process's words; a name that would run past what the reply holds or what a slot keeps ends the reading, and the rows from there show their key prefix.
            let mut at = 1 + cnt * 9;
            for i in 0..cnt {
                if at >= got {
                    break;
                }
                let len = m[at] as usize;
                if len > NAME_BYTES || at + 1 + len > got {
                    break;
                }
                self.names[i][..len].copy_from_slice(&m[at + 1..at + 1 + len]);
                self.names_n[i] = len as u8;
                at += 1 + len;
            }
            self.n = cnt;
        }
        self.poll_add(svc);
    }

    /// The add flow and a departure waiting (fgtw ops 9 and 13). WHY the checks: the counts and lengths are another process's words; a row or a name that would run past what the reply holds or a slot keeps ends the reading there.
    fn poll_add(&mut self, svc: u64) {
        let mut a = [0u8; 2 + 8 * (8 + 2 + NAME_BYTES) + 1 + 255 + 1 + NAME_BYTES + 2 + 512];
        let k = ferros_sys::call_live(svc, &[9], &mut a);
        self.cands = 0;
        self.add_note_n = 0;
        if k != ferros_sys::ERR && k >= 2 {
            let got = (k as usize).min(a.len());
            self.add = a[0];
            let mut at = 2;
            for i in 0..(a[1] as usize).min(8) {
                if at + 10 > got {
                    break;
                }
                let len = a[at + 9] as usize;
                if len > NAME_BYTES || at + 10 + len > got {
                    break;
                }
                self.cand_key[i].copy_from_slice(&a[at..at + 8]);
                self.cand_flags[i] = a[at + 8];
                self.cand_name[i][..len].copy_from_slice(&a[at + 10..at + 10 + len]);
                self.cand_name_n[i] = len as u8;
                self.cands = i + 1;
                at += 10 + len;
            }
            if at < got {
                let len = (a[at] as usize).min(self.add_note.len());
                if at + 1 + len <= got {
                    self.add_note[..len].copy_from_slice(&a[at + 1..at + 1 + len]);
                    self.add_note_n = len;
                }
                at += 1 + a[at] as usize;
            }
            // The selected device's name and words (fgtw `adding_reply`), as far as the reply and the fields hold them.
            self.sel_name_n = 0;
            self.sel_words_n = 0;
            if at < got {
                let len = a[at] as usize;
                if at + 1 + len <= got && len <= NAME_BYTES {
                    self.sel_name[..len].copy_from_slice(&a[at + 1..at + 1 + len]);
                    self.sel_name_n = len;
                }
                at += 1 + len;
            }
            if at + 2 <= got {
                let len = u16::from_le_bytes([a[at], a[at + 1]]) as usize;
                if at + 2 + len <= got && len <= self.sel_words.len() {
                    self.sel_words[..len].copy_from_slice(&a[at + 2..at + 2 + len]);
                    self.sel_words_n = len;
                }
            }
        } else {
            self.add = 0;
        }
        let mut d = [0u8; 4 + NAME_BYTES];
        let k = ferros_sys::call_live(svc, &[13], &mut d);
        self.depart = None;
        self.denied_by_n = 0;
        // WHY the mins: the name's length is the service's word; what is copied is what both the reply and the field hold.
        if k != ferros_sys::ERR && k >= 4 && (d[0] == 1 || d[0] == 2) {
            let len = (d[3] as usize).min(NAME_BYTES).min(k as usize - 4);
            if d[0] == 1 {
                self.depart_name[..len].copy_from_slice(&d[4..4 + len]);
                self.depart_name_n = len;
                self.depart = Some((d[1], d[2]));
            } else {
                self.denied_by[..len].copy_from_slice(&d[4..4 + len]);
                self.denied_by_n = len;
            }
        }
    }

    /// An op to the identity service (8 open or close, 10 pick, 12 confirm, 14 approve or deny, 15 the keyboard for words, 18 home sync). The page's view comes back by the doorbell.
    fn add_op(&mut self, svc: u64, msg: &[u8]) {
        let mut r = [0u8; 8];
        let _ = ferros_sys::call_live(svc, msg, &mut r);
    }

    /// Start this device's departure (fgtw op 5, intent 1: to a new owner, so the approver also frees the device for another fleet).
    fn leave(&mut self, svc: u64) {
        let mut r = [0u8; 80];
        let n = ferros_sys::call_live(svc, &[5, 1], &mut r);
        self.leave_result = if n == ferros_sys::ERR || n < 1 { None } else { Some(r[0]) };
        self.confirm = false;
        self.sent = self.leave_result == Some(0);
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
        // Signed, as the display reads them (a page sliding off the left edge has its text at negative x); the clamp is the field's own range.
        m[1..3].copy_from_slice(&(x.clamp(-32768, 32767) as i16).to_le_bytes());
        m[3..5].copy_from_slice(&(y.clamp(-32768, 32767) as i16).to_le_bytes());
        m[5..7].copy_from_slice(&px.to_le_bytes());
        m[7..11].copy_from_slice(&colour.to_le_bytes());
        m[11] = align;
        let t = t.as_bytes();
        let k = t.len().min(m.len() - 12);
        m[12..12 + k].copy_from_slice(&t[..k]);
        self.op(&m[..12 + k]);
    }
    /// Text centred on (x, y) and turned about that centre by (c, s) in Q14, at opacity `alpha` (255 opaque): op 19, the titles turning in.
    fn turned(&mut self, x: i64, y: i64, px: u16, colour: u32, alpha: u8, c: i16, s: i16, t: &str) {
        let mut m = [0u8; 140];
        m[0] = 19;
        // Signed, as the display reads them; the clamps are the fields' own ranges (a title riding off the screen can have its centre past the edge).
        m[1..3].copy_from_slice(&(x.clamp(-32768, 32767) as i16).to_le_bytes());
        m[3..5].copy_from_slice(&(y.clamp(-32768, 32767) as i16).to_le_bytes());
        m[5..7].copy_from_slice(&px.to_le_bytes());
        m[7..11].copy_from_slice(&colour.to_le_bytes());
        m[11] = alpha;
        m[12..14].copy_from_slice(&c.to_le_bytes());
        m[14..16].copy_from_slice(&s.to_le_bytes());
        let t = t.as_bytes();
        let k = t.len().min(m.len() - 16);
        m[16..16 + k].copy_from_slice(&t[..k]);
        self.op(&m[..16 + k]);
    }
    /// The open circle, landed, as an app's region (display op 18, in place of `begin`): its centre, radius and ground; what follows is its page, in front of a clean window the display makes once.
    fn region(&mut self, x: i64, y: i64, r: i64, ground: u32) {
        let mut m = [0u8; 11];
        m[0] = 18;
        // The clamps are the fields' own ranges, as for every op the display reads (the circle is the screen's, well inside them).
        m[1..3].copy_from_slice(&(x.clamp(-32768, 32767) as i16).to_le_bytes());
        m[3..5].copy_from_slice(&(y.clamp(-32768, 32767) as i16).to_le_bytes());
        m[5..7].copy_from_slice(&(r.clamp(0, 65535) as u16).to_le_bytes());
        m[7..11].copy_from_slice(&ground.to_le_bytes());
        self.op(&m);
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
    /// fluor's button (display op 47): its label at `px` in `ink` centred on (cx, cy), in front of a pill `w` wide (wider for a long label) and `h` high, filled `fill` inside its two-tone edge (`tl` on the top-left side of the anti-diagonal, `br` on the other).
    #[allow(clippy::too_many_arguments)]
    fn button(&mut self, cx: i64, cy: i64, w: i64, h: i64, px: i64, look: [u32; 4], t: &str) {
        let mut m = [0u8; 140];
        m[0] = 47;
        // The clamps are the fields' own ranges, as for every op the display reads (a page sliding off an edge has its buttons past it).
        m[1..3].copy_from_slice(&(cx.clamp(-32768, 32767) as i16).to_le_bytes());
        m[3..5].copy_from_slice(&(cy.clamp(-32768, 32767) as i16).to_le_bytes());
        m[5..7].copy_from_slice(&(w.clamp(0, 65535) as u16).to_le_bytes());
        m[7..9].copy_from_slice(&(h.clamp(0, 65535) as u16).to_le_bytes());
        m[9..11].copy_from_slice(&(px.clamp(0, 65535) as u16).to_le_bytes());
        for (k, c) in look.iter().enumerate() {
            m[11 + 4 * k..15 + 4 * k].copy_from_slice(&c.to_le_bytes());
        }
        let t = t.as_bytes();
        let k = t.len().min(m.len() - 27);
        m[27..27 + k].copy_from_slice(&t[..k]);
        self.op(&m[..27 + k]);
    }
    /// fluor's slider (display op 48): the track from x0 to x1 at y, `tr` its half-thickness and `kr` the knob's radius (Q8 pixels), the knob at `t` of the way (0 to 1), `left` up to it, `right` past it.
    #[allow(clippy::too_many_arguments)]
    fn slider(&mut self, x0: i64, x1: i64, y: i64, tr: i64, kr: i64, t: f64, left: u32, right: u32, knob: u32) {
        let mut m = [0u8; 25];
        m[0] = 48;
        for (k, &v) in [x0, x1, y].iter().enumerate() {
            m[1 + 2 * k..3 + 2 * k].copy_from_slice(&(v.clamp(-32768, 32767) as i16).to_le_bytes());
        }
        m[7..9].copy_from_slice(&(tr.clamp(0, 65535) as u16).to_le_bytes());
        m[9..11].copy_from_slice(&(kr.clamp(0, 65535) as u16).to_le_bytes());
        // The knob's place in 65535ths; the slider keeps t within 0 to 1, and the cast saturates.
        m[11..13].copy_from_slice(&(libm::round(t * 65535.) as u16).to_le_bytes());
        for (k, c) in [left, right, knob].iter().enumerate() {
            m[13 + 4 * k..17 + 4 * k].copy_from_slice(&c.to_le_bytes());
        }
        self.op(&m);
    }
    /// A radio button (display op 49) or a checkbox (op 50) and its label, centred together on (cx, cy): the mark `mark` pixels across, its stroke `sw` wide (Q8 pixels), `state` bit 0 chosen or checked (bit 1 with it: the checkbox's tick); colours stroke, fill, the label's ink, the tick.
    #[allow(clippy::too_many_arguments)]
    fn mark(&mut self, op: u8, cx: i64, cy: i64, px: i64, mark: i64, sw: i64, state: u8, colours: [u32; 4], t: &str) {
        let mut m = [0u8; 140];
        m[0] = op;
        m[1..3].copy_from_slice(&(cx.clamp(-32768, 32767) as i16).to_le_bytes());
        m[3..5].copy_from_slice(&(cy.clamp(-32768, 32767) as i16).to_le_bytes());
        m[5..7].copy_from_slice(&(px.clamp(0, 65535) as u16).to_le_bytes());
        m[7..9].copy_from_slice(&(mark.clamp(0, 65535) as u16).to_le_bytes());
        m[9..11].copy_from_slice(&(sw.clamp(0, 65535) as u16).to_le_bytes());
        m[11] = state;
        for (k, c) in colours.iter().enumerate() {
            m[12 + 4 * k..16 + 4 * k].copy_from_slice(&c.to_le_bytes());
        }
        let t = t.as_bytes();
        let k = t.len().min(m.len() - 28);
        m[28..28 + k].copy_from_slice(&t[..k]);
        self.op(&m[..28 + k]);
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

/// What the screen is showing: the plane, or one circle arrived at, its page open.
#[derive(Clone, Copy, PartialEq)]
enum View {
    Map,
    Open(usize),
}

/// What a tap on the settings does: each row's own, kept with its rectangle as the last frame laid it out.
#[derive(Clone, Copy, PartialEq)]
enum Choice {
    Base(u8),
    Labels,
    Dither,
    /// One of the theme swatches.
    Theme(u8),
    /// The brightness slider, continuous (preference 3, 32nds of a stop): the value is decided by x, dragged or tapped.
    Brightness,
    /// The UI scale slider (preference 14), half to twice: the scale decided by x.
    Scale,
    /// A link's value knob set to one of its choices (its radio group): link, slot, value.
    KnobSet(u8, u8, u8),
    /// The light scope's channel (preference 7) set to one of its radio group, 0 off to 6.
    ScopeSet(u8),
    /// The keyboard's layout (honeycomb::LAYOUTS: 0 Dvorak, 1 QWERTY).
    Layout(u8),
    UnlockAtBoot,
    ProxWake,
    /// About's version line: twelve taps in a row and Developer is a page.
    Version,
    /// A neighbour page's title: one page that way.
    Page(i8),
    /// The Fleet page's "remove this device": the first tap shows what goes with it, the second starts the departure.
    FleetRemove,
    /// The Fleet page's add flow (fgtw::sponsor through payloads/fgtw ops 8-15): open it, pick a device heard nearby (its row), type the newcomer's words (on the attest screen's keyboard, ahau), confirm a nearby pick turned green, close it.
    FleetAdd,
    FleetPick(u8),
    FleetWords,
    FleetConfirm,
    FleetClose,
    /// A sibling's departure waiting here: Approve (fgtw op 14 [1]) or Deny ([0]).
    FleetApprove,
    FleetDeny,
    /// The Fleet page's "this device's home screen": follows the person's fleet, or keeps its own (payloads/fgtw op 18; docs/IDENTITY-SPACE.md).
    HomeSync,
    /// A link on Connections: its own page (LINKS order).
    Link(u8),
    /// A radio's page: its switch, as its console circle's tap (preference 15).
    Switch(u8),
    /// A link's knob (docs/RADIO-CONTROLS.md): its link and slot in the control blocks (ferros_sys::control); a value knob goes round its choices, an action counts up.
    Knob(u8, u8),
    /// The clock's zone, a quarter hour west (-1) or east (1).
    Zone(i8),
    /// The title: the page picker up or away.
    Picker,
    /// A page's title in the picker: that page.
    Pick(u8),
    /// A kist's want on the Kists or Permissions page: (the kist in the person's list, the want), its answer turned over.
    Answer(u8, u8),
    /// A right's default for new kists (Permissions).
    Default(u8),
    /// The first-launch question: a want's switch (its index in the kist's wants).
    AskWant(u8),
    /// The first-launch question answered: the answers kept and the kist launched.
    AskGo,
}

/// The settings pages, the ring a swipe goes round (Photon's split, photon docs/settings-panel.md; owner, 2026-11-05: "I like the page scopes and your organization"); a page for each link after Connections (owner, 2026-11-07: a radio's numbers and config, reached from its console circle by a drag toward the centre, or from Connections). Developer is a page only on development builds, or once the twelve taps made it one. The clock after the links (docs/RADIO-CONTROLS.md): its time, zone and sources.
const PAGES: [&str; 18] = ["appearance", "units", "keyboard", "security", "kists", "permissions", "fleet", "power", "connections", "cellular", "WiFi", "Bluetooth", "NFC", "GPS", "USB", "clock", "about", "developer"];
/// Whether this is a development build (the display's own test, `FERROS_PRODUCTION` unset): Developer is always a page there.
const DEV: bool = option_env!("FERROS_PRODUCTION").is_none();
/// Whether it looks like one (the orange hairlines, as the display's): FERROS_RELEASE_LOOK set gives a development build the release look for screen captures (owner, 2026-11-10), its tools kept.
const DEV_LOOK: bool = DEV && option_env!("FERROS_RELEASE_LOOK").is_none();
/// Taps in a row on About's version line that make Developer a page (owner, 2026-11-05: "twelve"), and the tap from which the hint shows and counts down ("don't show a toast hint until four taps"). No time window: any other tap starts the count again.
const DEV_TAPS: u8 = 12;
const DEV_HINT: u8 = 4;
/// The radios and USB in the kernel's facts order (facts 33 + 8 · link, ferros_sys::measure), as the Connections page names them.
const LINKS: [&str; 6] = ["cellular", "WiFi", "Bluetooth", "NFC", "GPS", "USB"];

/// A link's state in words: switched off by its circle (preference 15; USB, the sixth, has no switch) before anything its driver last said, which outlives it.
fn link_state(k: usize, state: Option<i64>) -> &'static str {
    if k < 5 && ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF) >> k & 1 == 1 {
        return "switched off";
    }
    match state {
        Some(0) => "off",
        Some(1) => "on, no link",
        Some(2) => "connected",
        _ => "-",
    }
}

/// A measure (ferros_sys::measure) for a link's page: its name returned, its value written into `l` in its unit and the device's base (tatau). Milli units shown whole: a link's numbers to the unit are all a person reads.
fn measure_words(l: &mut Line, f: tatau::Fmt, kind: u16, v: i64) -> &'static str {
    use ferros_sys::measure as m;
    let num = |l: &mut Line, n: i64, unit: &str| {
        if n < 0 {
            l.s("-");
        }
        if let Ok(tatau::Out::Nothing) = tatau::count(l, f, n.unsigned_abs(), 1) {
            l.s("0");
        }
        l.s(unit);
    };
    match kind {
        m::TECH => {
            l.s(match v {
                1 => "WiFi 4",
                2 => "WiFi 5",
                3 => "WiFi 6",
                4 => "WiFi 7",
                14 => "LTE",
                20 => "NR",
                32 => "Bluetooth",
                33 => "NFC",
                34 => "GPS",
                35 => "USB",
                _ => "-",
            });
            "technology"
        }
        m::RSSI => { num(l, v / 1000, " dBm"); "signal" }
        m::NOISE => { num(l, v / 1000, " dBm"); "noise floor" }
        m::SNR => { num(l, v / 1000, " dB"); "signal to noise" }
        m::RSRP => { num(l, v / 1000, " dBm"); "reference power" }
        m::RSRQ => { num(l, v / 1000, " dB"); "reference quality" }
        m::SINR => { num(l, v / 1000, " dB"); "signal to interference" }
        m::VSWR => { num(l, v / 1000, ":1"); "antenna match (VSWR)" }
        m::TX_POWER => { num(l, v / 1000, " dBm"); "transmit power" }
        m::FREQ if v >= 100_000 => { num(l, v / 1000, " MHz"); "frequency" }
        m::FREQ => { num(l, v, " kHz"); "frequency" }
        m::BANDWIDTH => { num(l, v / 1000, " MHz"); "channel width" }
        m::CHANNEL => { num(l, v, ""); "channel" }
        m::RATE_DOWN | m::RATE_UP => {
            let _ = tatau::size_bits(l, f, v.unsigned_abs(), 1);
            l.s("/s");
            if kind == m::RATE_DOWN { "rate down" } else { "rate up" }
        }
        m::LATENCY => { let _ = tatau::duration_ns(l, f, v.unsigned_abs(), 1); "round trip" }
        m::LOSS => { let _ = tatau::share(l, f, v.unsigned_abs(), 1_000_000, 2); "loss" }
        m::MCS => { num(l, v, ""); "MCS" }
        m::STREAMS => { num(l, v, ""); "streams" }
        m::SATS_SEEN => { num(l, v, ""); "satellites seen" }
        m::SATS_USED => { num(l, v, ""); "satellites in the fix" }
        m::CN0 => { num(l, v / 1000, " dB-Hz"); "best carrier to noise" }
        m::ACCURACY => { num(l, v / 1000, " m"); "accuracy (±)" }
        m::REGISTRATION => {
            l.s(match v {
                0 => "not registered",
                1 => "home",
                2 => "searching",
                3 => "denied",
                5 => "roaming",
                20 => "emergency only",
                _ => "unknown",
            });
            "registration"
        }
        m::MTU => { let _ = tatau::size_bits(l, f, v.unsigned_abs() * 8, 1); "largest packet" }
        m::PACKETS_DOWN => { num(l, v, ""); "packets in" }
        m::PACKETS_UP => { num(l, v, ""); "packets out" }
        m::ERRORS_DOWN => { num(l, v, ""); "bad packets in" }
        m::ERRORS_UP => { num(l, v, ""); "failed packets out" }
        m::DROPPED_DOWN => { num(l, v, ""); "dropped in" }
        m::DROPPED_UP => { num(l, v, ""); "refused out" }
        m::SUSPENDS => { num(l, v, ""); "link naps" }
        m::POWER_SAVE => {
            l.s(match v {
                0 => "off",
                1 => "on",
                2 => "fast",
                _ => "-",
            });
            "power save"
        }
        m::LINK_POWER => {
            l.s(match v {
                0 => "working",
                1 => "light sleep",
                2 => "sleep",
                3 => "suspended",
                _ => "-",
            });
            "link power"
        }
        m::ALTITUDE => { num(l, v / 1000, " m"); "altitude" }
        m::SPEED => { num(l, v / 1000, " m/s"); "speed" }
        m::HEADING => { num(l, v / 1000, "°"); "heading" }
        _ => { num(l, v, ""); "measure" }
    }
}

/// The groups a link's page shows its measures in (docs/RADIO-CONTROLS.md), in order: each kind's group.
const GROUPS: [&str; 5] = ["signal", "channel", "link", "counters", "motion"];
fn group_of(kind: u16) -> usize {
    use ferros_sys::measure as m;
    match kind {
        m::FREQ | m::BANDWIDTH | m::CHANNEL => 1,
        m::RATE_DOWN | m::RATE_UP | m::LATENCY | m::LOSS | m::MCS | m::STREAMS | m::MTU | m::POWER_SAVE | m::LINK_POWER | m::REGISTRATION | m::TECH => 2,
        m::PACKETS_DOWN | m::PACKETS_UP | m::ERRORS_DOWN | m::ERRORS_UP | m::DROPPED_DOWN | m::DROPPED_UP | m::SUSPENDS => 3,
        m::ALTITUDE | m::SPEED | m::HEADING => 4,
        _ => 0,
    }
}

/// A kist the display draws itself in its circle (a widget: its scene icon at G#80 and up, display WIDGET_*), by the kist's name: the clock (owner, 2026-11-10: "my clock widget kist on the homescreen again"). Installed like any app (payloads/clock, tools/clock-kist.sh), never launched.
fn widget(name: &str) -> Option<u8> {
    name.eq_ignore_ascii_case("Clock").then_some(0x80)
}

/// The identity service as `fgtw_poller` last read it, under a sequence count (odd while it is written; it moves only when the service rang).
static FLEET_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static mut FLEET_SNAP: Fleet = Fleet::new();

/// The identity service's doorbell, held on a thread of its own (owner, 2026-11-10: "No polling! No timers unless PROVEN necessary! It should be push or doorbell"). It calls fgtw's op 21 with the change count it last saw; fgtw answers when anything the map reads has changed (a departure asking, the fleet, the add flow, the space), and only then is the service read (ops 1, 6, 9, 13, 17) and the copy published. Until 2026-11-10 the map asked ops 13 and 17 every second from its main loop, each answer 2 s behind fgtw's work, and gestures and the Fleet page froze with it.
extern "C" fn fgtw_poller(grants: u64) -> ! {
    use core::sync::atomic::Ordering;
    let svc = cap(grants, S_FGTW);
    let mut snap = Fleet::new();
    // u32::MAX is never a count fgtw has given, so the first call is answered at once.
    let mut count = u32::MAX;
    // Only while fgtw is not serving: it starts once a person has attested and may restart, and no doorbell exists for a server coming up, so it is asked again after a wait that doubles from a sixty-fourth of a second to a second (the one timer here, and why).
    let mut absent = freq() >> 6;
    loop {
        let mut m = [21u8, 0, 0, 0, 0];
        m[1..].copy_from_slice(&count.to_le_bytes());
        let mut r = [0u8; 4];
        // An fgtw without the doorbell answers op 21 as its state (a byte or two), never four bytes: treated as absent.
        if ferros_sys::call_live(svc, &m, &mut r) != 4 {
            sleep_until(now() + absent);
            if absent < freq() {
                absent <<= 1;
            }
            continue;
        }
        absent = freq() >> 6;
        count = u32::from_le_bytes(r);
        snap.poll(svc);
        FLEET_SEQ.fetch_add(1, Ordering::AcqRel);
        unsafe { *(&raw mut FLEET_SNAP) = snap };
        FLEET_SEQ.fetch_add(1, Ordering::AcqRel);
    }
}

/// The copy `fgtw_poller` last published and its sequence count, read whole (again if it was being written); None when it has not moved from `seen`.
fn fleet_take(seen: u32) -> Option<(u32, Fleet)> {
    use core::sync::atomic::Ordering;
    loop {
        let s0 = FLEET_SEQ.load(Ordering::Acquire);
        if s0 == seen {
            return None;
        }
        let v = unsafe { *(&raw const FLEET_SNAP) };
        if s0 & 1 == 0 && FLEET_SEQ.load(Ordering::Acquire) == s0 {
            return Some((s0, v));
        }
    }
}

/// A detail field's name for the page (ferros_detail's schemas).
fn detail_label(field: &str) -> &'static str {
    match field {
        "carrier" => "carrier",
        "apn" => "data APN",
        "attach_apn" => "attach APN",
        "ipv4" => "address",
        "ipv6" => "address (IPv6)",
        "dns4" => "DNS",
        "dns6" => "DNS (IPv6)",
        "ssid" => "network",
        "bssid" => "access point",
        "mac" => "this device on it",
        "router" => "router",
        "dns" => "DNS",
        "lease" => "lease",
        _ => "detail",
    }
}

/// A detail value written for the page: names as they are; addresses, being identifiers and not quantities, in hexadecimal grouped as their standards group them (no base's magnitude reading makes sense of them); a lease as a duration in the device's base.
fn detail_words(l: &mut Line, f: tatau::Fmt, v: &ferros_detail::vsf::VsfType) {
    use ferros_detail::vsf::VsfType;
    let hex = |l: &mut Line, bytes: &[u8], group: usize, sep: &str| {
        for (i, c) in bytes.chunks(group).enumerate() {
            if i > 0 {
                l.s(sep);
            }
            for b in c {
                let _ = write!(l, "{b:02X}");
            }
        }
    };
    match v {
        VsfType::a(s) => {
            l.s(s);
        }
        // A network's name: its octets as UTF-8 text when they are, else as hex (an SSID is octets, and the hex is the honest form of one that is not text).
        VsfType::nw(s) => match core::str::from_utf8(s.as_bytes()) {
            Ok(t) if !t.chars().any(char::is_control) => {
                l.s(t);
            }
            _ => hex(l, s.as_bytes(), 1, ""),
        },
        // IPv4 in the standard dotted decimal (owner, 2026-11-09: "Standard convention for units is fine for now"); IPv6 and hardware addresses keep their standard hex.
        VsfType::ni(a) => {
            for (k, b) in a.iter().enumerate() {
                if k > 0 {
                    l.s(".");
                }
                let _ = write!(l, "{b}");
            }
        }
        VsfType::nj(a) => hex(l, a, 2, ":"),
        VsfType::nm(a) => hex(l, a, 1, ":"),
        other => match other.as_u64() {
            Some(s) => {
                let _ = tatau::duration_ns(l, f, s * 1_000_000_000, 1);
            }
            None => {
                l.s("-");
            }
        },
    }
}

/// A satellite system's name (ferros_sys::Sat).
fn system_name(s: u8) -> &'static str {
    match s {
        0 => "GPS",
        1 => "GLONASS",
        2 => "Galileo",
        3 => "BeiDou",
        4 => "QZSS",
        5 => "NavIC",
        _ => "other",
    }
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
    xs: [i64; MAX],
    ys: [i64; MAX],
    rs: [i64; MAX],
    cam: Cam,
    view: View,
    /// The settings' tappable places, as laid out by the last frame: (left, right, top, bottom, what).
    rows: [(i64, i64, i64, i64, Choice); 64],
    /// The settings page on show; the strip's offset from it in pixels (positive: dragged right, the page before it coming in), kept within half a screen by moving `page` as it passes; how far the page is scrolled up; and the fling still carrying it (pixels a frame, under `coast`'s friction), with the fraction of a pixel it has gone past its last whole one.
    page: usize,
    page_dx: i64,
    scroll: i64,
    fling: f64,
    fling_res: f64,
    /// The plane under one finger: the last three frames' movements on the panel, whether this touch panned it, and its fling once the finger lifts: (direction x, direction y, speed in pixels a frame, the sub-pixel left over x, y).
    pans: [(i64, i64); 3],
    panned: bool,
    pan: Option<(f64, f64, f64, f64, f64)>,
    /// Each page's content height as last laid out, for how far it scrolls.
    page_h: [i64; PAGES.len()],
    /// The drag on the settings: which way it is locked (0 not yet, 1 across, 2 up and down), where the strip and the scroll stood at the touch, and the last three frames' movements up and down.
    swipe: u8,
    drag_base: (i64, i64),
    dys: [i64; 3],
    /// Across: the last three frames' movements, and the strip's spring to its page once the finger lifts: (offset at the start, speed at the start in pixels a frame in `page_dx`'s sense, frames gone); None at rest or under a finger.
    dxs: [i64; 3],
    spring: Option<(f64, f64, i32, f64)>,
    /// Moving after the finger lifted, and the display's frame count the motion last stepped to (None: the first step is one frame).
    settling: bool,
    frame_seen: Option<u32>,
    /// Taps in a row on About's version line; the unlock-at-boot row armed by its first tap.
    taps: u8,
    armed: bool,
    /// Developer made a page by the twelve taps (the vault, `map.dev`).
    dev: bool,
    /// The Fleet page as the identity service last answered: its state (payloads/fgtw: 0 no identity, 1 joining, 2 bound waiting for the key, 3 a member with the key, 4-7 looking up, 8 leaving), the approval words while leaving, the members (their device names, or key prefixes where there is none; which one is this device), and the remove row armed by its first tap.
    fleet: Fleet,
    /// Where the camera is flying (`fly`), and whether the flight leaves the open circle for the plane (the page drawn, shrinking with the circle, until it lands).
    target: Option<Cam>,
    flight: Option<Flight>,
    /// The view a circle was opened from, flown back to when it closes (owner, 2026-11-06: "it should revert to the last view when exiting").
    back: Option<Cam>,
    leaving: bool,
    /// The fade from the plane (0) to the open page (1), on the flight's spring; and whether the display flies this flight from its two prerendered images (op 45 took it), and the page image's camera (the open circle landed).
    fade: f64,
    fast: bool,
    page_cam: Option<Cam>,
    /// A finger is down (the touch session), for the camera op's moving flag.
    touching: bool,
    /// The page picker is up (a tap on the title), and how far its list is scrolled, pixels.
    picker: bool,
    pick_scroll: i64,
    /// A pinch on the open page: the spread it began at (sixteenths of a pixel) and the scale then; and the scale it is at now, shown live and kept as the preference when the fingers lift (64ths of an octave).
    pinch: Option<(i64, f32)>,
    scale_live: Option<f32>,
    /// The controls under a finger (COMPOSITOR.md "Controls"): the row a finger is down on, which a button shows held until the finger lifts or moves; a slider dragged and the knob's place on it now (0 to 1); and a slider's knob let go off its value's place (a brightness between stops, a scale between 64ths), on a spring to it: (the slider, the offset in pixels and the speed it started at, frames gone).
    pressed: Option<Choice>,
    slide: Option<(Choice, f64)>,
    knob: Option<(Choice, f64, f64, i32)>,
    nrows: usize,
    /// Each circle's own turn (its icon), (cos, sin) in Q14; two fingers in a circle twist it.
    rc: [i64; MAX],
    rsn: [i64; MAX],
    /// Circles moved or sized by hand since arranging began, to keep when it ends.
    arranged: [bool; MAX],
    /// An app is in front (one the map did not start: a bridge spawn), and it has the touches.
    app_front: bool,
    /// How many circles there are: settings, then a kist circle for each kist in the person's list (at most `MAX`).
    n: usize,
    /// The person's kists and their answers (`kists.list`, ferros_kist), as last read: kist circle i is `kists.kists[i - 1]`.
    kists: ferros_kist::Kists,
    /// The first-launch question on an open kist circle: each unanswered want's switch as the person has it now, a bit per want (started from their defaults).
    ask: u64,
}

impl Map {
    /// Circle `i` as the scene draws it: settings first, then each installed kist, its name its label and its icon the display's bundled one for that app (tools/app-icons order), or none.
    fn spec(&self, i: usize) -> Spec<'_> {
        if i == 0 {
            let c = &CIRCLES[0];
            return Spec { label: c.label, ring: c.ring, lit: c.lit, share: c.share, icon: c.icon };
        }
        let name = self.kists.kists[i - 1].name.as_str();
        // The display's bundle (tools/app-icons APPS, 1-based): the kist's icon by its app's name until a kist carries its own (KISTS.md: the list's `icon`).
        const BUNDLED: [&str; 5] = ["Photon", "Mahere", "Lumis", "Opsin", "Ramps"];
        let icon = match widget(name) {
            Some(w) => w,
            None => BUNDLED.iter().position(|b| b.eq_ignore_ascii_case(name)).map_or(0, |k| k as u8 + 1),
        };
        Spec { label: name, ring: 1, lit: 0, share: KIST_SHARE, icon }
    }

    fn label(&self, i: usize) -> &str {
        self.spec(i).label
    }

    /// The kist behind circle `i` (none for settings).
    fn kist(&self, i: usize) -> Option<&ferros_kist::Kist> {
        i.checked_sub(1).and_then(|k| self.kists.kists.get(k))
    }

    /// The person's list read again (another device of theirs may have answered, or an install added a kist); false when it could not be read (nobody present), the copy kept as it was.
    fn reload_kists(&mut self) -> bool {
        match ferros_kist::store::read(cap(self.grants, S_KIST_LIST)) {
            Some(k) => {
                self.kists = k;
                true
            }
            None => false,
        }
    }

    /// The person's list read again, and the kist circles made to match it when a kist came or went (an install over the bridge, one on another of their devices): placed afresh by the seeded fit, each kept place winning, then the scene sent again.
    fn refresh_circles(&mut self) {
        if !self.reload_kists() {
            return;
        }
        // WHY: the arrays hold MAX circles (as at start).
        let n = (1 + self.kists.kists.len()).min(MAX);
        if n == self.n {
            return;
        }
        self.place_circles(n);
    }

    /// The person's kists and arrangement read again after the identity service wrote them from their fleet (payloads/fgtw space.rs: a reset device getting its home screen back, a move made on another of their devices): every circle placed afresh, each kept place winning, the view as kept, then the scene sent again.
    fn reload_arrangement(&mut self) {
        if !self.reload_kists() {
            return;
        }
        // WHY: the arrays hold MAX circles (as at start).
        let n = (1 + self.kists.kists.len()).min(MAX);
        self.place_circles(n);
        if self.load_view() {
            self.send_scene();
        }
    }

    /// `n` circles placed by the seeded fit, each kept place winning (`load_places_near`), pushed apart, and the scene sent.
    fn place_circles(&mut self, n: usize) {
        self.n = n;
        for i in 1..MAX {
            self.rs[i] = if i < n { radius_of(KIST_SHARE) } else { 0 };
            self.rc[i] = ONE;
            self.rsn[i] = 0;
        }
        let (mut xs, mut ys) = ([0i64; MAX], [0i64; MAX]);
        place(&self.rs[..n], &mut xs[..n], &mut ys[..n]);
        for i in 0..n {
            self.xs[i] = xs[i] << 8;
            self.ys[i] = ys[i] << 8;
        }
        self.load_places_near();
        self.push_apart(MAX);
        self.send_scene();
    }

    /// The places kept by hand (`load_places`), then every circle still at its seeded place moved with the settings circle: the seeded fit is about the settings circle at the origin, and a kist circle installed after the person moved settings would otherwise land far from it, off the view they keep.
    fn load_places_near(&mut self) -> usize {
        let seeded: [(i64, i64); MAX] = core::array::from_fn(|i| (self.xs[i], self.ys[i]));
        let kept = self.load_places();
        let (dx, dy) = (self.xs[0] - seeded[0].0, self.ys[0] - seeded[0].1);
        for i in 1..self.n {
            if (self.xs[i], self.ys[i]) == seeded[i] {
                self.xs[i] += dx;
                self.ys[i] += dy;
            }
        }
        kept
    }

    /// The person's list written whole, after a change of theirs (an answer, a default); the console says when the vault did not take it.
    fn save_kists(&self) -> bool {
        let ok = ferros_kist::store::write(cap(self.grants, S_KIST_LIST), &self.kists, ferros_detail::eagle_now());
        if !ok {
            console(self.grants, b"map: the vault did not take the person's kists\n");
        }
        ok
    }

    /// Ask init to launch kist circle `i`'s kist (KISTS.md): the map holds none of its rights. Init's answer (ferros_kist::launch). A widget's kist is not launched: the display draws it, in its circle, from the scene (`widget`).
    fn launch(&self, i: usize) -> u8 {
        let Some(k) = self.kist(i) else { return ferros_kist::launch::NOT_INSTALLED };
        if widget(&k.name).is_some() {
            return ferros_kist::launch::RUNNING;
        }
        let mut m = [0u8; 33];
        m[0] = ferros_kist::launch::OP_LAUNCH;
        m[1..].copy_from_slice(&k.capsule);
        let mut r = [ferros_kist::launch::REFUSED; 1];
        call(cap(self.grants, S_LAUNCH), &m, &mut r);
        let mut l = Line::new();
        let _ = write!(l, "map: launch {}: {}\n", k.name, match r[0] { 0 => "starting", 1 => "running", 2 => "asking first", 3 => "not installed", 5 => "the list is not readable", _ => "refused" });
        console(self.grants, l.as_str().as_bytes());
        r[0]
    }

    /// A running kist relaunched with its answers as they now stand (owner, 2026-11-08: a revoke is a kill and a respawn without that right, and the kill is one shot dead): init's `OP_RELAUNCH`; nothing happens to a kist not running.
    fn relaunch(&self, k: usize) {
        let Some(kist) = self.kists.kists.get(k) else { return };
        let mut m = [0u8; 33];
        m[0] = ferros_kist::launch::OP_RELAUNCH;
        m[1..].copy_from_slice(&kist.capsule);
        let mut r = [0u8; 1];
        call(cap(self.grants, S_LAUNCH), &m, &mut r);
    }

    /// Where an arrived circle stands on the panel, centre and radius: the circle an app lives in (ferros_sys::active_circle, the display's, its hairline), so every active app's page is inscribed in the same circle (owner, 2026-11-05). Until then fifteen thirty-seconds of the width across, at the panel's centre. The zoom keeps the map's own circle within the screen's width, so the page's fill, at this radius, is what shows.
    fn arrived(&self) -> (i64, i64, i64) {
        let (cx, cy, r) = ferros_sys::active_circle(self.w as usize, self.h as usize);
        (cx as i64, cy as i64, r as i64)
    }

    /// A circle's vault key, relative to the map's scope: `place.<label>`, so a circle keeps its place however the table grows.
    fn place_key(&self, i: usize, key: &mut [u8; 32]) -> usize {
        let l = self.label(i).as_bytes();
        key[..6].copy_from_slice(b"place.");
        // WHY: a vault name is at most 32 bytes here; a kist's name past 26 is cut, and a space (a vault name holds none) becomes an underscore.
        let n = l.len().min(26);
        for (d, &s) in key[6..6 + n].iter_mut().zip(l) {
            *d = if s == b' ' { b'_' } else { s };
        }
        6 + n
    }

    /// The arrangement made by hand, from the vault: each circle's place as EWE integers (CLAUDE.md: no fixed widths on storage) — x and y in world units ×256, zigzag-mapped (they can be negative), the radius, and then (since the circles turn) its turn as cos and sin in Q14, zigzag; a place kept before turns existed has three and stays upright.
    fn load_places(&mut self) -> usize {
        let mut loaded = 0;
        for i in 0..self.n {
            let mut key = [0u8; 32];
            let kl = self.place_key(i, &mut key);
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
        let kl = self.place_key(i, &mut key);
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
            for a in 0..self.n {
                for b in a + 1..self.n {
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
        for i in 0..self.n {
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
        // Alone, a circle has nothing to be within a ratio of (the search below would leave lo at i64::MAX and lo × RADIUS_RATIO would overflow), so the screen bounds it instead, at the zoom it is held at: from a 64th of the screen's width across to the width itself (past three quarters of the width it is a place, and the fingers move the plane anyway). Two fingers on a kist circle size it, the only one too (owner, 2026-11-07: until then a lone circle kept its size and only turned).
        if self.n == 1 {
            return (((self.w >> 7) << 16) / self.cam.z, ((self.w >> 1) << 16) / self.cam.z);
        }
        let (mut lo, mut hi) = (i64::MAX, 1i64);
        for j in 0..self.n {
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
        for i in 0..self.n {
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
        for i in 0..self.n {
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
        // Alone, a circle is its own connected set: with no neighbour the nearest distance below stays i64::MAX and every move would be refused.
        if self.n == 1 {
            return true;
        }
        let (z_out, _) = self.zoom_limits();
        // Positions ×256 and radii in world units: the gap in world units ×256, against the diagonal brought to world ×256 at the zoom-out limit.
        let diag = isqrt(self.w * self.w + self.h * self.h);
        let limit = ((diag << 16) / z_out.max(1)) << 8;
        let pos = |j: usize| if j == i { (self.xs[j] + dx, self.ys[j] + dy) } else { (self.xs[j], self.ys[j]) };
        for a in 0..self.n {
            let (ax, ay) = pos(a);
            let mut nearest = i64::MAX;
            for b in 0..self.n {
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
        for i in 0..self.n {
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
        for i in 0..self.n {
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
        let mut m = [0u8; 2 + 30 * MAX];
        m[0] = 13;
        m[1] = self.n as u8;
        for i in 0..self.n {
            let a = self.spec(i);
            let at = 2 + i * 30;
            m[at..at + 4].copy_from_slice(&((self.xs[i] >> 8) as i32).to_le_bytes());
            m[at + 4..at + 8].copy_from_slice(&((self.ys[i] >> 8) as i32).to_le_bytes());
            m[at + 8..at + 10].copy_from_slice(&(self.rs[i] as u16).to_le_bytes());
            m[at + 10] = a.ring;
            m[at + 11] = a.lit;
            m[at + 12] = a.icon;
            let label = a.label.as_bytes();
            let len = if labels || a.icon == 0 { label.len().min(16) } else { 0 };
            m[at + 13] = len as u8;
            m[at + 14..at + 14 + len].copy_from_slice(&label[..len]);
        }
        call(self.display, &m, &mut r);
        self.send_crowd();
        // The circles' own turns ride on `scene move`: a fresh scene starts every circle upright.
        for i in 0..self.n {
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
        for i in 0..self.n {
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

    /// The camera that shows every circle with air around them, never closer than one world unit to a pixel: every circle inside the screen's width and the big circle (`active_circle`) together, so none lands past an edge or under the console in a top corner (owner, 2026-11-08: "the fit doesn't fit to screen extents anded with the big circle"). The turn is kept; the circles' bounds are taken as the screen will turn them, their middle put on the big circle's centre, and the zoom is the largest at which each circle passes both tests, with no margin.
    fn home(&self) -> Cam {
        let (c, s, one) = (self.cam.c as f64, self.cam.s as f64, ONE as f64);
        // A world offset (×256) turned as `Cam::screen` turns it.
        let turn = |x: f64, y: f64| ((c * x - s * y) / one, (s * x + c * y) / one);
        let (mut u0, mut v0, mut u1, mut v1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for i in 0..self.n {
            let (u, v) = turn(self.xs[i] as f64, self.ys[i] as f64);
            let r = (self.rs[i] << 8) as f64;
            u0 = u0.min(u - r);
            v0 = v0.min(v - r);
            u1 = u1.max(u + r);
            v1 = v1.max(v + r);
        }
        let (um, vm) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
        // The middle back into the world: the turn undone (its transpose).
        let (wx, wy) = ((c * um + s * vm) / one, (c * vm - s * um) / one);
        let (_, ay, ar) = self.arrived();
        let (half_w, ar) = (self.w as f64 * 0.5, ar as f64);
        // The zoom (Q16 pixels a world unit; a world offset ×256 is (d / 256) · (z / 2¹⁶) pixels): each circle's farthest reach across, |du| + r, within the half width, and its farthest reach from the big circle's centre, √(du² + dv²) + r, within its radius.
        // No margin (owner, 2026-11-08: "No margins."): the outermost circle touches the edge it meets.
        let room = (1u64 << 24) as f64;
        let mut z = f64::MAX;
        for i in 0..self.n {
            let (u, v) = turn(self.xs[i] as f64, self.ys[i] as f64);
            let (du, dv, r) = (u - um, v - vm, (self.rs[i] << 8) as f64);
            z = z.min(room * half_w / (du.abs() + r)).min(room * ar / (libm::sqrt(du * du + dv * dv) + r));
        }
        let (lo, hi) = self.zoom_limits();
        let mut cam = Cam { x: wx as i64, y: wy as i64, z: (z as i64).clamp(lo, hi), c: self.cam.c, s: self.cam.s };
        // The camera's centre is the panel's: it stands off the fit's middle by the big circle's offset from the panel's centre, as `at` does.
        let (dx, dy) = cam.delta(0, self.h / 2 - ay);
        cam.x += dx;
        cam.y += dy;
        cam
    }

    /// The camera that has arrived at circle i: the circle on the active circle (`arrived`), the circle an app lives in, however much larger than the screen that is (no zoom limit here: arriving, the map's own circle becomes the app's). The camera's centre is the panel's, so it stands off the circle's centre by the active circle's offset from the panel's centre, in world units at the arrived zoom.
    fn at(&self, i: usize) -> Cam {
        let (_, ay, ar) = self.arrived();
        let mut cam = Cam { x: self.xs[i], y: self.ys[i], z: (ar << 16) / self.rs[i].max(1), c: self.cam.c, s: self.cam.s };
        let (dx, dy) = cam.delta(0, self.h / 2 - ay);
        cam.x += dx;
        cam.y += dy;
        cam
    }

    /// Go to `to`: the camera flies there a display frame at a time (`step_cam`), each part of it on the critically damped spring (`spring`) in the space it looks even in (owner, 2026-11-05: the tap's flight follows the settings' snap; 2026-11-06: eased, as the pages are). From rest it eases in; a new target mid-flight keeps the speed the camera has, so a second tap turns the flight without a jolt.
    fn fly(&mut self, to: Cam) {
        let mut vel: [f64; 5] = match (self.target, self.flight) {
            (Some(_), Some(f)) => core::array::from_fn(|i| spring(f.off[i], f.vel[i], f.t).1),
            _ => [0.; 5],
        };
        let (c, s) = (self.cam.c as f64, self.cam.s as f64);
        let (tc, ts) = (to.c as f64, to.s as f64);
        // The fade's end: the open page when flying to it, the plane when leaving it; on the plane there is no page, the fade at rest at 0.
        let fade_to = match (self.view, self.leaving) {
            (View::Open(_), false) => 1.,
            (View::Open(_), true) => 0.,
            _ => {
                self.fade = 0.;
                vel[4] = 0.;
                0.
            }
        };
        let off = [(self.cam.x - to.x) as f64, (self.cam.y - to.y) as f64, libm::log(self.cam.z as f64 / to.z as f64), libm::atan2(tc * s - ts * c, tc * c + ts * s), self.fade - fade_to];
        self.flight = Some(Flight { off, vel, fade_to, t: 0 });
        self.target = Some(to);
        self.settling = true;
        self.frame_seen = None;
        self.prerender();
    }

    /// A flight to or from the open circle starts (COMPOSITOR.md "The flight, prerendered"): the display is asked to make what it lacks of the flight's two images (op 45), the plane at its camera and the page landed, and the flight then sends only the camera and the fade a frame (op 46). Refused (no free surface, an app in front), the flight draws every frame as before.
    fn prerender(&mut self) {
        let (View::Open(i), Some(to)) = (self.view, self.target) else {
            self.fast = false;
            return;
        };
        let opening = !self.leaving;
        // Opening: the plane is where the camera is now, the page where it lands. Closing: the plane where it lands, the page as it was landed.
        let (plane, page) = if opening { (self.cam, to) } else { (to, self.page_cam.unwrap_or_else(|| self.at(i))) };
        let mut b = [0u8; 4096];
        b[0] = 45;
        b[1] = opening as u8;
        b[2..20].copy_from_slice(&plane.op(true));
        b[20..38].copy_from_slice(&page.op(true));
        let mut n = 38;
        let mut put = |b: &mut [u8; 4096], op: &[u8]| {
            // WHY the test: a page of more than the message holds is cut, as `Batch::op` cuts one.
            if n + 2 + op.len() <= b.len() {
                b[n..n + 2].copy_from_slice(&(op.len() as u16).to_le_bytes());
                b[n + 2..n + 2 + op.len()].copy_from_slice(op);
                n += 2 + op.len();
            }
        };
        if opening {
            // The page as landing shows it: laid out with no flight (`page_frame` landed), its region op, its ops, the landed camera.
            let target = self.target.take();
            let mut pb = Batch::new();
            let (cx, cy, r, _) = self.page_frame();
            pb.region(cx, cy, r, fill());
            match self.view {
                View::Open(i) if i > 0 => self.ask_page(&mut pb, i, true),
                _ => self.settings(&mut pb, true),
            }
            self.nrows = 0;
            self.target = target;
            let mut at = 1usize;
            while at + 2 <= pb.n {
                let l = u16::from_le_bytes([pb.b[at], pb.b[at + 1]]) as usize;
                put(&mut b, &pb.b[at + 2..at + 2 + l]);
                at += 2 + l;
            }
            put(&mut b, &page.op(true));
            self.page_cam = Some(page);
        } else {
            put(&mut b, &plane.op(true));
        }
        let mut r = [0u8; 4];
        let got = call(self.display, &b[..n], &mut r);
        self.fast = got >= 1 && got != ferros_sys::ERR && r[0] == 0;
        let mut l = Line::new();
        let _ = write!(l, "map: flight {}: {}\n", if opening { "in" } else { "out" }, if self.fast { "prerendered" } else { "drawn whole (the display has no room)" });
        console(self.grants, l.as_str().as_bytes());
    }

    /// One display frame of the camera's flight to `target`: each part where its spring has it now — the centre at its world offset, the zoom at the target's times e to the logarithm's offset (a circle grows by smooth factors), the turn at the target's direction turned by the angle's. Landed, exactly on the target, once every part and its speed are under half a pixel on the screen; then, leaving, the view is the plane again. Returns whether it is still flying.
    fn step_cam(&mut self) -> bool {
        let (Some(t), Some(mut f)) = (self.target, self.flight) else { return false };
        f.t += 1;
        let p: [(f64, f64); 5] = core::array::from_fn(|i| spring(f.off[i], f.vel[i], f.t));
        // The fade: from rest the spring never passes its end, and a flight turned round starts it with a speed under ω times its offset (each step keeps that so), so it stays between the plane and the page.
        self.fade = f.fade_to + p[4].0;
        let z = t.z as f64 * libm::exp(p[2].0);
        let (cs, sn) = (libm::cos(p[3].0), libm::sin(p[3].0));
        let (tc, ts) = (t.c as f64, t.s as f64);
        self.cam = Cam {
            x: t.x + libm::round(p[0].0) as i64,
            y: t.y + libm::round(p[1].0) as i64,
            z: libm::round(z) as i64,
            c: libm::round(tc * cs - ts * sn) as i64,
            s: libm::round(ts * cs + tc * sn) as i64,
        };
        self.flight = Some(f);
        // On the screen, in pixels: the centre's offset at this zoom (world ×256 at 16.16 pixels a unit), the zoom's and the turn's across the screen's width; each part and its speed.
        let w = self.w as f64;
        // The fade in 1024ths: it lands within a thousandth of its end, so landing on the real page shows no step.
        let px = |i: usize, v: f64| if i < 2 { v.abs() * z / (1u64 << 24) as f64 } else if i < 4 { v.abs() * w } else { v.abs() * (1 << 10) as f64 };
        let still = (0..5).any(|i| px(i, p[i].0) >= 0.5 || px(i, p[i].1) >= 0.5);
        if !still {
            self.cam = t;
            self.target = None;
            self.flight = None;
            self.fade = f.fade_to;
            self.fast = false;
            if self.leaving {
                self.leaving = false;
                self.view = View::Map;
                self.picker = false;
            }
        }
        still
    }

    /// Draw the frame: the overlay for the view first (what is sent first is on top), then the scene under it through the camera. The one circle is the system's: the device's settings.
    /// Landed on an open circle, the frame is its region (COMPOSITOR.md "The open circle is an app's region"): the ground and the scene made once by the display, the page in front of them, only what changed drawn and published. Flying, or on the plane, every frame is drawn whole.
    fn draw(&mut self) {
        let mut b = Batch::new();
        // Flying to or from the open circle on the display's prerendered images: the camera now and the fade, nothing drawn (op 46).
        if self.fast && self.target.is_some() && matches!(self.view, View::Open(_)) {
            self.nrows = 0;
            let mut m = [0u8; 21];
            m[0] = 46;
            m[1..19].copy_from_slice(&self.cam.op(true));
            // The fade as 16-bit coverage; it stays within 0 to 1 (`step_cam`), and the cast saturates.
            m[19..21].copy_from_slice(&(libm::round(self.fade * 65535.) as u16).to_le_bytes());
            b.op(&m);
            b.send(self.display);
            return;
        }
        let region = matches!(self.view, View::Open(_)) && self.target.is_none();
        if region {
            let (cx, cy, r, _) = self.page_frame();
            self.page_cam = Some(self.cam);
            b.region(cx, cy, r, fill());
        } else {
            b.op(&[1]);
        }
        self.nrows = 0;
        match self.view {
            View::Map => {}
            View::Open(0) => self.settings(&mut b, region),
            View::Open(i) => self.ask_page(&mut b, i, region),
        }
        // Flag bit 1: something moves (a finger is down, or a flight, fling or spring goes on): the display may pan by shifting what it drew (COMPOSITOR.md "Pans from the last frame"); a frame without it is drawn whole, at rest.
        let mut cam = self.cam.op(true);
        cam[13] |= ((self.touching || self.settling) as u8) << 1;
        b.op(&cam);
        b.send(self.display);
    }

    /// How many settings pages there are now: Developer the last of them on development builds, or once the twelve taps made it one.
    fn pages(&self) -> usize {
        if DEV || self.dev { PAGES.len() } else { PAGES.len() - 1 }
    }
    /// Page `p`, round the ring either way.
    /// Settings at the page named `name` (a link's page, from Connections or from its console circle dragged toward the centre): from the plane the settings circle flies open on it, as a tap on it does; already open, it is on that page at once.
    fn show_page(&mut self, name: &str) {
        let Some(p) = PAGES.iter().position(|q| *q == name) else { return };
        self.page = p;
        self.page_dx = 0;
        self.scroll = 0;
        self.fling = 0.;
        self.spring = None;
        if !matches!(self.view, View::Open(_)) {
            self.back = Some(self.cam);
            let to = self.at(0);
            self.view = View::Open(0);
            self.leaving = false;
            self.fly(to);
        }
        self.settling = true;
        self.frame_seen = None;
        self.draw();
    }

    fn wrap(&self, p: isize) -> usize {
        p.rem_euclid(self.pages() as isize) as usize
    }
    /// Where the open page stands on the panel now: centre, radius and its layout unit in 256ths of a pixel. While the camera flies, on the circle as the camera shows it, so the page grows and shrinks with it and nothing pops in on arrival; landed, exactly the arrived circle. The unit is a 48th of the width at the circle's arrived size (28 px on husky: the page as it stood at 1344 px), times the UI scale, 2^(s/64): the device's (preference 14), or the pinch's while one goes on.
    fn page_frame(&self) -> (i64, i64, i64, i64) {
        let (ax, ay, ar) = self.arrived();
        let s = self.scale_live.unwrap_or(ferros_sys::ui_scale() as f32);
        let k = libm::exp2f(s / 64.);
        let unit = |r: i64| ((self.w << 8) * r / (48 * ar)) as f32 * k;
        match (self.view, self.target) {
            (View::Open(i), Some(_)) => {
                let (sx, sy) = self.cam.screen(self.w, self.h, self.xs[i], self.ys[i]);
                let r = (self.rs[i] * self.cam.z) >> 16;
                (sx, sy, r, unit(r) as i64)
            }
            _ => (ax, ay, ar, unit(ar) as i64),
        }
    }
    /// The settings' layout unit in 256ths of a pixel, as the page stands now (`page_frame`).
    fn unit(&self) -> i64 {
        self.page_frame().3
    }
    /// The settings' bands, from the circle as the page stands now: the titles' baseline just under its top (under the camera, landed), and the band the page lives in (top, bottom).
    fn bands(&self) -> (i64, i64, i64) {
        let (_, cy, r, uq) = self.page_frame();
        let title = cy - r + (4 * uq >> 8);
        (title, title + (3 * uq >> 8), cy + r - (4 * uq >> 8))
    }
    /// How far page `p` scrolls: what of it does not fit its band.
    fn scroll_max(&self, p: usize) -> i64 {
        let (_, top, bottom) = self.bands();
        (self.page_h[p] - (bottom - top)).max(0)
    }

    /// The settings (owner, 2026-11-05): the pages a ring, the page on show inscribed in the circle an app lives in, its title large at the top with its neighbours' small and dim either side; a swipe across goes round the ring, up and down scrolls a page longer than its band. Laid out in 48ths of the width (`unit`).
    fn settings(&mut self, b: &mut Batch, region: bool) {
        let (cx, cy, r, uq) = self.page_frame();
        // Lengths in the unit, from its 256ths (uq): k units over d, rounded once, so the page scales smoothly through a flight or a pinch.
        let uu = |n: i64, d: i64| n * uq / (d << 8);
        let (title_y, _, _) = self.bands();
        // The titles ride the circle (owner, 2026-11-07: "rotate around with the circle", not slid left and right and clipped, not spun about themselves, no vertical skew): each page's title stands on the arc about the circle's centre through the title's place, a page apart by the angle whose arc is the screen's width, so at the top a title moves with the finger. Turning from one page to the next the outgoing title rides the arc away and the incoming one rides in from the other side, each turned to its place on the arc (its baseline the arc's tangent), at one size. Each is drawn at α = cos²(π · its offset in pages), gone at half a page with α's slope 0 there, so the hand-over is C¹. The offset is the finger's or its spring's, counted in display frames: the motion has no clock of its own.
        let span = uu(18, 1);
        let size = uu(2, 1);
        // The ink's centre about 0.36 of an em over the baseline (as the display centres turned text), and its distance from the circle's centre: the arc's radius.
        let (ink_y, cyf) = ((title_y - size * 23 / 64) as f64, cy as f64);
        let arc = cyf - ink_y;
        let per_page = self.w as f64 / arc;
        let t = self.page_dx as f64 / self.w as f64;
        for k in -1isize..=1 {
            let d = k as f64 + t;
            let v = libm::cos(core::f64::consts::PI * d);
            let alpha = libm::round(v * v * 255.) as i64;
            // Past half a page (or so near it that nothing would show) the title is gone.
            if d.abs() >= 0.5 || alpha == 0 {
                continue;
            }
            // Its angle round the circle from straight up, clockwise as the strip moves right.
            let phi = d * per_page;
            let (c, s) = (libm::cos(phi), libm::sin(phi));
            let (x, y) = (cx as f64 + arc * s, cyf - arc * c);
            let p = self.wrap(self.page as isize + k);
            b.turned(libm::round(x) as i64, libm::round(y) as i64, size as u16, white(), alpha as u8, libm::round(c * 16384.) as i16, libm::round(s * 16384.) as i16, PAGES[p]);
        }
        // The neighbours are still a tap away, either side of the title where their titles stood.
        for k in [-1i8, 1] {
            let x = cx + k as i64 * span;
            self.push_row(x - span / 2, x + span / 2, title_y - uu(2, 1), title_y + uu(1, 1), Choice::Page(k));
        }
        // The title itself: a tap brings up the page picker (owner, 2026-11-08: "a vertical scrolling settings picker that comes up with just titles when you click on the title"), a tap again puts it away.
        self.push_row(cx - span / 2, cx + span / 2, title_y - uu(2, 1), title_y + uu(1, 1), Choice::Picker);
        if self.picker {
            self.draw_picker(b, cx);
        } else {
            let cur = self.page;
            // Each page's height at the unit it is drawn at, laid out first into a batch that is not sent (not live, so no rows): its centring needs it, and the unit changes every frame of a flight (a page never laid out was centred as empty, 2026-11-05).
            for k in -1isize..=1 {
                let p = self.wrap(cur as isize + k);
                self.draw_page(&mut Batch::new(), p, cx, false);
            }
            self.draw_page(b, cur, cx + self.page_dx, true);
            if self.page_dx > 0 {
                let p = self.wrap(cur as isize - 1);
                self.draw_page(b, p, cx + self.page_dx - self.w, false);
            } else if self.page_dx < 0 {
                let p = self.wrap(cur as isize + 1);
                self.draw_page(b, p, cx + self.page_dx + self.w, false);
            }
        }
        // The page out to the circle itself, which the display's hairline draws over; a region's ground is its region op's, made once.
        if !region {
            b.disc(cx, cy, r, fill());
        }
    }

    /// The first-launch question (owner, 2026-11-08), inscribed in the kist's open circle: its name, then each want not yet answered, its switch as the person has it (`ask`, started from their defaults: out of the box every one off) and the developer's reason under it, then "start". Asked once for this person and this kist, on every device of theirs; a kist itself can never ask.
    fn ask_page(&mut self, b: &mut Batch, i: usize, region: bool) {
        let (cx, cy, r, uq) = self.page_frame();
        let uu = |n: i64, d: i64| n * uq / (d << 8);
        let half = self.w / 2;
        let (title_y, top, bottom) = self.bands();
        let Some(k) = self.kist(i).cloned() else { return };
        b.text(cx, title_y, uu(2, 1) as u16, white(), 1, &k.name);
        let open: alloc::vec::Vec<usize> = (0..k.wants.len()).filter(|&j| k.wants[j].answer.is_none()).collect();
        // The rows' height, centred in the band: a line saying what this is, each want two lines, then start.
        let pitch = uu(7, 2);
        let h = uu(3, 1) + open.len() as i64 * pitch + uu(4, 1);
        let mut y = (top + bottom - h) / 2 + uu(2, 1);
        b.text(cx, y, uu(1, 1) as u16, grey(), 1, "asks for, once; change it later in settings");
        y += uu(3, 1);
        let live = self.target.is_none();
        for &j in &open {
            let w = &k.wants[j];
            let on = self.ask >> j & 1 == 1;
            // Its checkbox (COMPOSITOR.md "Controls"), the label white when on.
            let (sw, colours, tick) = mark_look(uu(5, 4));
            b.mark(50, cx, y - uu(1, 2), uu(5, 4), uu(1, 1), sw, on as u8 | ((on && tick) as u8) << 1, [colours[0], colours[1], if on { white() } else { grey() }, colours[3]], ferros_kist::MEANS[w.right as usize]);
            b.text(cx, y + uu(3, 2), uu(7, 8) as u16, dim(), 1, &w.reason);
            if live {
                self.push_row(cx - half, cx + half, y - uu(2, 1), y + uu(3, 2), Choice::AskWant(j as u8));
            }
            y += pitch;
        }
        y += uu(1, 1);
        // The button that starts it: fluor's, held while a finger is on it.
        let [bf, bh, tl, br] = BUTTON_LOOK[ferros_sys::theme::index()];
        b.button(cx, y - uu(1, 2), uu(16, 1), uu(2, 1), uu(3, 2), [if self.pressed == Some(Choice::AskGo) { bh } else { bf }, tl, br, white()], "start");
        if live {
            self.push_row(cx - half, cx + half, y - uu(2, 1), y + uu(1, 1), Choice::AskGo);
        }
        if !region {
            b.disc(cx, cy, r, fill());
        }
    }

    /// The page picker's rows: their pitch and the size of their titles, in the unit (`unit`), and how far the list scrolls (what of it does not fit the page's band).
    fn picker_metrics(&self) -> (i64, i64, i64) {
        let uq = self.unit();
        let (_, top, bottom) = self.bands();
        let pitch = 3 * uq >> 8;
        let size = 7 * uq / (4 << 8);
        // WHY the floor: a list shorter than the band does not scroll at all.
        (pitch, size, (self.pages() as i64 * pitch - (bottom - top)).max(0))
    }

    /// The page picker (owner, 2026-11-08): every page's title, one a row down the page's band, the open page's white and the rest grey, the list scrolled by `pick_scroll`; a tap on a title opens that page. Only the rows inside the band are sent; the text moves, nothing is laid out again.
    fn draw_picker(&mut self, b: &mut Batch, cx: i64) {
        let (_, top, bottom) = self.bands();
        let (pitch, size, _) = self.picker_metrics();
        let half = self.w / 2;
        for p in 0..self.pages() {
            // The row's band and its baseline, about two thirds down it.
            let row_top = top + p as i64 * pitch - self.pick_scroll;
            let base = row_top + pitch * 2 / 3;
            // Wholly inside the band only: a title cut by the band's edge would be clipped by nothing (the circle's edge is further out).
            if row_top < top || row_top + pitch > bottom {
                continue;
            }
            b.text(cx, base, size as u16, if p == self.page { white() } else { grey() }, 1, PAGES[p]);
            if self.page_dx == 0 {
                self.push_row(cx - half, cx + half, row_top, row_top + pitch, Choice::Pick(p as u8));
            }
        }
    }

    /// What a vertical drag and its fling move: the open page's scroll, or the page picker's while it is up; and how far it goes.
    fn vscroll(&self) -> i64 {
        if self.picker { self.pick_scroll } else { self.scroll }
    }
    fn vscroll_max(&self) -> i64 {
        if self.picker { self.picker_metrics().2 } else { self.scroll_max(self.page) }
    }
    fn set_vscroll(&mut self, v: i64) {
        if self.picker {
            self.pick_scroll = v;
        } else {
            self.scroll = v;
        }
    }

    /// Page `p` centred on x = `cx`: centred in its band when it fits, else from the band's top less the scroll. `live`: the page on show, whose rows a tap reaches (only once the strip has settled on it).
    fn draw_page(&mut self, b: &mut Batch, p: usize, cx: i64, live: bool) {
        let uq = self.unit();
        let uu = |n: i64, d: i64| n * uq / (d << 8);
        let (_, top, bottom) = self.bands();
        let fits = self.page_h[p] <= bottom - top;
        let y0 = if fits { (top + bottom - self.page_h[p]) / 2 } else { top - if live { self.scroll } else { 0 } };
        let hit = live && self.page_dx == 0;
        let half = self.w / 2;
        let mut y = y0 + uu(2, 1);
        // One row of text, centred, tappable when `what` is given.
        let row = |m: &mut Map, b: &mut Batch, y: &mut i64, px: i64, colour: u32, t: &str, what: Option<Choice>| {
            b.text(cx, *y, px as u16, colour, 1, t);
            if let (true, Some(c)) = (hit, what) {
                m.push_row(cx - half, cx + half, *y - uu(2, 1), *y + uu(1, 1), c);
            }
            *y += uu(3, 1);
        };
        // The controls (COMPOSITOR.md "Controls"), each in a row's place and pitch, tappable across the row as a row is. A button: fluor's pill, sixteen units wide (a longer label grows it), two high, centred on the text's middle, held while a finger is down on it, and in its held fill while `armed` (the first tap of two).
        let button = |m: &mut Map, b: &mut Batch, y: &mut i64, t: &str, armed: bool, what: Option<Choice>| {
            let [fill, held, tl, br] = BUTTON_LOOK[ferros_sys::theme::index()];
            let down = armed || what.is_some() && m.pressed == what;
            b.button(cx, *y - uu(1, 2), uu(16, 1), uu(2, 1), uu(5, 4), [if down { held } else { fill }, tl, br, white()], t);
            if let (true, Some(c)) = (hit, what) {
                m.push_row(cx - half, cx + half, *y - uu(2, 1), *y + uu(1, 1), c);
            }
            *y += uu(3, 1);
        };
        // A checkbox (op 50) or a radio button (op 49) with its label, the mark a unit across: filled when on (`MARK_LOOK`), the label white when on and grey when off.
        let mark = |m: &mut Map, b: &mut Batch, y: &mut i64, op: u8, t: &str, on: bool, what: Option<Choice>| {
            let (sw, colours, tick) = mark_look(uu(5, 4));
            let state = on as u8 | ((on && tick) as u8) << 1;
            b.mark(op, cx, *y - uu(1, 2), uu(5, 4), uu(1, 1), sw, state, [colours[0], colours[1], if on { white() } else { grey() }, colours[3]], t);
            if let (true, Some(c)) = (hit, what) {
                m.push_row(cx - half, cx + half, *y - uu(2, 1), *y + uu(1, 1), c);
            }
            *y += uu(3, 1);
        };
        // A radio group (op 49, state bit 2): one choice of several, its marks in one column, the group centred on the page by its widest label (the display measures it, op 28); `now` the one chosen, `what` each one's choice.
        let radios = |m: &mut Map, b: &mut Batch, y: &mut i64, names: &[&str], now: usize, what: &dyn Fn(usize) -> Choice| {
            let (px, mk) = (uu(5, 4), uu(1, 1));
            let wide = names.iter().filter_map(|n| ferros_sys::window::width(m.display, px as u16, n.as_bytes())).max().unwrap_or(0) as i64 / 4;
            let left = cx - (mk + mk / 2 + wide) / 2;
            let (sw, colours, _) = mark_look(px);
            for (v, n) in names.iter().enumerate() {
                let on = v == now;
                b.mark(49, left, *y - uu(1, 2), px, mk, sw, on as u8 | 4, [colours[0], colours[1], if on { white() } else { grey() }, colours[3]], n);
                if hit {
                    m.push_row(cx - half, cx + half, *y - uu(2, 1), *y + uu(1, 1), what(v));
                }
                *y += uu(3, 1);
            }
        };
        // A slider (op 48): fluor's, the theme's text colour up to the knob and on it, black past it; its knob a unit across and its track a quarter of that thick (fluor's proportions); the row it is in reaches a unit and a half either side.
        let slider = |m: &mut Map, b: &mut Batch, y: &mut i64, what: Choice| {
            let (a, z) = m.track(cx);
            let kr = uq >> 1;
            b.slider(a, z, *y, kr >> 2, kr, m.slider_t(what, cx), white(), 0, white());
            if hit {
                m.push_row(cx - half, cx + half, *y - uu(3, 2), *y + uu(3, 2), what);
            }
            *y += uu(4, 1);
        };
        // A fact: its name right of the centre's left, its value left of the centre's right.
        let fact = |b: &mut Batch, y: &mut i64, name: &str, value: &str| {
            b.text(cx - uu(1, 1), *y, (uu(5, 4)) as u16, grey(), 2, name);
            b.text(cx + uu(1, 1), *y, (uu(5, 4)) as u16, white(), 0, value);
            *y += uu(2, 1) + uu(1, 4);
        };
        // A fact whose value can run past the page's edge (an IPv6 address; owner, 2026-11-10: "should wrap when it hits the edge of the screen"): broken after its own separators (a colon, a space, a dot) onto further lines under it, each as long as fits. The map has no font metrics (the display has them): a character is taken as five eighths of the size, wider than the face's digits, so a line errs short. Half the page wide less a unit either side is always tens of characters, so a line is never empty.
        let fact_wrap = |b: &mut Batch, y: &mut i64, name: &str, value: &str| {
            let px = uu(5, 4);
            let fits = ((half - uu(2, 1)) * 8 / (px * 5)) as usize;
            b.text(cx - uu(1, 1), *y, px as u16, grey(), 2, name);
            let mut rest = value;
            loop {
                let cut = if rest.len() <= fits {
                    rest.len()
                } else {
                    let mut f = fits;
                    while !rest.is_char_boundary(f) {
                        f -= 1;
                    }
                    rest[..f].rfind([':', ' ', '.']).map_or(f, |k| k + 1)
                };
                b.text(cx + uu(1, 1), *y, px as u16, white(), 0, &rest[..cut]);
                rest = &rest[cut..];
                if rest.is_empty() {
                    break;
                }
                *y += uu(3, 2);
            }
            *y += uu(2, 1) + uu(1, 4);
        };
        let small = uu(5, 4);
        let note = uu(7, 8);
        match PAGES[p] {
            "appearance" => {
                // The themes as swatches: each its ground with its ring around it, the one in use with its mark colour at the centre, its name beneath.
                let now_t = ferros_sys::theme::index();
                for (i, t) in ferros_sys::theme::THEMES.iter().enumerate() {
                    let x = cx + (2 * i as i64 - 3) * uu(3, 1);
                    if i == now_t {
                        // The chosen one's dot in its theme's mark colour, as a radio's (white in the default theme; owner, 2026-11-08: "no amber").
                        b.disc(x, y, uu(1, 2), t.mark);
                    }
                    b.disc(x, y, uu(2, 1) - uu(1, 4), t.plane);
                    b.disc(x, y, uu(2, 1), t.ring);
                    b.text(x, y + uu(3, 1) + uu(1, 2), note as u16, if i == now_t { white() } else { grey() }, 1, t.name);
                    if hit {
                        self.push_row(x - uu(3, 1), x + uu(3, 1), y - uu(2, 1) - uu(1, 2), y + uu(4, 1), Choice::Theme(i as u8));
                    }
                }
                y += uu(7, 1);
                b.text(cx, y, small as u16, grey(), 1, "brightness");
                y += uu(2, 1);
                // Brightness: a continuous slider, dimmest on the left, dragged or tapped (owner, 2026-11-08: "sliders (for brightness)", "why's brightness not continuous?").
                slider(self, b, &mut y, Choice::Brightness);
                // The UI scale, half to twice, as a pinch on the page sets it.
                b.text(cx, y, small as u16, grey(), 1, "scale");
                y += uu(2, 1);
                slider(self, b, &mut y, Choice::Scale);
                let labels = ferros_sys::pref(ferros_sys::PREF_LABELS) == 1;
                mark(self, b, &mut y, 50, "names under circles", labels, Some(Choice::Labels));
            }
            "units" => {
                // The number base is chosen by seeing: each row is two seconds and a mebibyte (2^23 bits, a whole number of doublings) written in that base, its name small beneath.
                let now_base = ferros_sys::device_base();
                for code in BASES {
                    let f = tatau::Fmt::new(tatau::Base::from_code(code).unwrap_or(tatau::Base::Dms), Style::Glyphs);
                    let mut v = Line::new();
                    let _ = tatau::duration_ns(&mut v, f, 2_000_000_000, 1);
                    v.s("   ");
                    let _ = tatau::size_bits(&mut v, f, 1 << 23, 1);
                    let current = code == now_base;
                    b.text(cx, y, (uu(3, 2)) as u16, if current { white() } else { grey() }, 1, v.as_str());
                    b.text(cx, y + uu(11, 8), note as u16, if current { grey() } else { dim() }, 1, base_name(code));
                    // Its radio button left of it, chosen for the base in use (one of a group: a choice, not a switch).
                    let (sw, colours, _) = mark_look(small);
                    b.mark(49, cx - uu(14, 1), y - uu(1, 2), small, uu(1, 1), sw, current as u8, [colours[0], colours[1], 0, 0], "");
                    if hit {
                        self.push_row(cx - half, cx + half, y - uu(2, 1), y + uu(15, 8), Choice::Base(code));
                    }
                    y += uu(4, 1);
                }
            }
            "keyboard" => {
                b.text(cx, y, note as u16, dim(), 1, "layout");
                y += uu(2, 1);
                let now_l = ferros_sys::pref(ferros_sys::PREF_LAYOUT);
                radios(self, b, &mut y, &["Dvorak", "QWERTY"], now_l as usize, &|i| Choice::Layout(i as u8));
            }
            "security" => {
                // Unlock at boot: off by default. Turning it on takes two taps, the first arming it with the reason it is off (AUTH.md: a stolen device then holds the identity).
                let on = ferros_sys::pref(ferros_sys::PREF_UNLOCK_AT_BOOT) == 1;
                // A checkbox; armed, its label asks for the second tap.
                let armed = self.armed;
                // Its words say attest, as the attest screen does (owner, 2026-11-10: "always attest").
                mark(self, b, &mut y, 50, if armed { "tap again: attest at boot" } else { "attest at boot" }, on, Some(Choice::UnlockAtBoot));
                b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "on, the device opens without your handle:");
                b.text(cx, y + uu(1, 2), note as u16, dim(), 1, "whoever holds it holds your identity");
                y += uu(3, 1);
            }
            "fleet" => {
                // The fleet this identity's devices make (payloads/fgtw): where this device stands, the members, and removing this device. Removing wipes the identity's things from it (AUTH.md: the identity class) and it restarts at the attest screen once a device of the fleet approves.
                let f = self.fleet;
                'fleet: {
                // A sibling asking to leave: the request alone, nothing else on the page until it is answered (owner, 2026-11-10: "curious why there's so many buttons on that screen", and "it's a bit vague").
                if let (Some(3), Some((intent, result))) = (f.state, f.depart) {
                    let name = core::str::from_utf8(&f.depart_name[..f.depart_name_n]).unwrap_or("");
                    b.text(cx, y, (uu(3, 2)) as u16, white(), 1, if name.is_empty() { "one of your devices" } else { name });
                    y += uu(2, 1);
                    row(self, b, &mut y, small, white(), if intent == 1 { "asks to leave your fleet, for a new owner" } else { "asks to leave your fleet" }, None);
                    match result {
                        0 | 3 => {
                            b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "approve: your things are wiped from it and it restarts");
                            b.text(cx, y + uu(1, 2), note as u16, dim(), 1, "deny: it stays in your fleet");
                            y += uu(3, 1);
                            button(self, b, &mut y, if result == 3 { "approve again: the last one did not go out" } else { "approve" }, false, Some(Choice::FleetApprove));
                            button(self, b, &mut y, "deny", false, Some(Choice::FleetDeny));
                        }
                        1 => row(self, b, &mut y, small, grey(), "approving", None),
                        2 => row(self, b, &mut y, small, grey(), "approved: it is leaving your fleet", None),
                        _ => row(self, b, &mut y, small, grey(), "denied: it stays in your fleet", None),
                    }
                    break 'fleet;
                }
                // This device leaving, or its request on the way: that alone (owner, 2026-11-10: the second tap on remove showed nothing).
                if f.state == Some(8) || f.sent {
                    let me = (0..f.n).find(|&i| f.me[i]).and_then(|i| core::str::from_utf8(&f.names[i][..f.names_n[i] as usize]).ok()).unwrap_or("");
                    if !me.is_empty() {
                        b.text(cx, y, (uu(3, 2)) as u16, white(), 1, me);
                        y += uu(2, 1);
                    }
                    row(self, b, &mut y, small, white(), if f.state == Some(8) { "this device asks to leave your fleet" } else { "sending the request to your devices" }, None);
                    b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "approve or deny on any of your other devices,");
                    b.text(cx, y + uu(1, 2), note as u16, dim(), 1, "or tap one of them to this one");
                    y += uu(3, 1);
                    b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "once approved, your things are wiped from it");
                    b.text(cx, y + uu(1, 2), note as u16, dim(), 1, "and it restarts; back gives up leaving");
                    y += uu(3, 1);
                    break 'fleet;
                }
                // A departure approved here has gone through: said once, above the page.
                if f.left_n > 0 {
                    let mut l = Line::new();
                    l.s(core::str::from_utf8(&f.left[..f.left_n]).unwrap_or("a device"));
                    l.s(" left your fleet");
                    row(self, b, &mut y, small, white(), l.as_str(), None);
                }
                let state = match f.state {
                    None => "the identity service is not running",
                    Some(0) => "no identity here",
                    Some(1) => "joining: confirm on one of your devices",
                    Some(2) => "in the fleet, waiting for its key",
                    Some(3) => "in the fleet",
                    Some(8) => "leaving the fleet",
                    // fgtw's verdicts while not yet bound (payloads/fgtw V_*), each its own words: until 2026-11-08 all four read "looking the identity up", forever when offline.
                    Some(5) => "a new identity: no fleet holds it yet",
                    Some(6) => "this handle has another identity's fleet",
                    Some(7) => "no verdict: nothing answers",
                    Some(_) => "looking the identity up",
                };
                row(self, b, &mut y, small, white(), state, None);
                for i in 0..f.n {
                    // Each member by the two-word name its fleet knows it by (Photon's Fleet page shows the same one); the key's first four bytes only when the service gave no name.
                    let mut l = Line::new();
                    match core::str::from_utf8(&f.names[i][..f.names_n[i] as usize]) {
                        Ok(name) if !name.is_empty() => {
                            l.s(name);
                        }
                        _ => {
                            for x in &f.keys[i][..4] {
                                let _ = write!(l, "{x:02x}");
                            }
                        }
                    }
                    if f.me[i] {
                        l.s("  this device");
                    } else if f.heard[i] {
                        l.s("  nearby");
                    }
                    row(self, b, &mut y, small, if f.me[i] { white() } else { grey() }, l.as_str(), None);
                }
                // Adding a device, and approving a sibling's departure (fgtw::sponsor, through payloads/fgtw ops 8-15): a member of the fleet only.
                if f.state == Some(3) {
                    // The home screen across the person's devices (owner, 2026-11-08: "on all my ferros devices the homescreen layout should sync, unless of course I go into settings and uncheck that"): its kists and answers always follow them; its arrangement does unless switched off here.
                    if let Some((on, settled)) = f.home {
                        let label = match (on, settled) {
                            (_, false) => "home screen: fetching yours from your fleet",
                            (true, true) => "home screen: the same on all your devices",
                            (false, true) => "home screen: this device's own",
                        };
                        // A checkbox once the fleet has answered: checked, the same on all your devices; unchecked, this device's own.
                        if settled {
                            mark(self, b, &mut y, 50, "home screen: the same on all your devices", on, Some(Choice::HomeSync));
                        } else {
                            row(self, b, &mut y, small, grey(), label, None);
                        }
                    }
                    if f.denied_by_n > 0 {
                        let mut l = Line::new();
                        l.s("leaving was denied on ");
                        l.s(core::str::from_utf8(&f.denied_by[..f.denied_by_n]).unwrap_or("one of your devices"));
                        row(self, b, &mut y, small, white(), l.as_str(), None);
                    }
                    if f.add == 0 {
                        button(self, b, &mut y, "add a device", false, Some(Choice::FleetAdd));
                        // Only a device heard here (its beacon or its request on this network, flags bits 0 and 1): one known only from fgtw.org's registry is no one nearby, and a join asked from anywhere on the internet is said nowhere (owner, 2026-11-10: "LAN, fine. WAN, nope").
                        if f.cand_flags[..f.cands].iter().any(|&fl| fl & 3 != 0) {
                            b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "a device on this network asks to join");
                            y += uu(2, 1);
                        }
                    } else {
                        let phase = match f.add {
                            1 => "add a device: devices asking to join",
                            2 => "adding it to the fleet",
                            3 => "add a device: selected",
                            4 => "handing it the fleet's key",
                            5 => "added",
                            _ => "adding it failed",
                        };
                        row(self, b, &mut y, small, white(), phase, None);
                        if f.add_note_n > 0 {
                            b.text(cx, y - uu(1, 1), note as u16, dim(), 1, core::str::from_utf8(&f.add_note[..f.add_note_n]).unwrap_or(""));
                            y += uu(2, 1);
                        }
                        for i in 0..f.cands {
                            // A device heard nearby (its request on this network, or its beacon) is a tap to pick; one only the registry knows is matched by its words.
                            let nearby = f.cand_flags[i] & 3 != 0;
                            let mut l = Line::new();
                            l.s(core::str::from_utf8(&f.cand_name[i][..f.cand_name_n[i] as usize]).unwrap_or("a device"));
                            l.s(if f.cand_flags[i] & 4 != 0 { "  added" } else if nearby { "  nearby: tap to select" } else { "  not nearby: type its words" });
                            let pick = nearby && f.add == 1;
                            row(self, b, &mut y, small, if pick { white() } else { grey() }, l.as_str(), pick.then_some(Choice::FleetPick(i as u8)));
                        }
                        if f.add == 1 {
                            if f.cands == 0 {
                                b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "open Photon's join screen on the device");
                                y += uu(2, 1);
                            }
                            button(self, b, &mut y, "type its words", false, Some(Choice::FleetWords));
                            b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "or tap the two devices together");
                            y += uu(2, 1);
                        }
                        // Selected (fgtw ADD_SELECTED, owner 2026-11-10: "I need a distinct green add, and note to press back otherwise"): the device and its words, the same it shows, Add in VSF RGB primary green, and Back with nothing to undo.
                        if f.add == 3 {
                            let name = core::str::from_utf8(&f.sel_name[..f.sel_name_n]).unwrap_or("the device");
                            let mut l = Line::new();
                            l.s("Selected: ");
                            l.s(name);
                            row(self, b, &mut y, small, white(), l.as_str(), None);
                            // The words four to a line, as the device shows them.
                            let words = core::str::from_utf8(&f.sel_words[..f.sel_words_n]).unwrap_or("");
                            let mut line = Line::new();
                            let mut k = 0;
                            for w in words.split(' ').filter(|w| !w.is_empty()) {
                                if k == 4 {
                                    b.text(cx, y - uu(1, 1), note as u16, grey(), 1, line.as_str());
                                    y += uu(3, 2);
                                    line = Line::new();
                                    k = 0;
                                }
                                if k > 0 {
                                    line.s(" ");
                                }
                                line.s(w);
                                k += 1;
                            }
                            if k > 0 {
                                b.text(cx, y - uu(1, 1), note as u16, grey(), 1, line.as_str());
                                y += uu(2, 1);
                            }
                            let mut l = Line::new();
                            l.s("Add ");
                            l.s(name);
                            let down = self.pressed == Some(Choice::FleetConfirm);
                            b.button(cx, y - uu(1, 2), uu(16, 1), uu(2, 1), uu(5, 4), [if down { GREEN_HELD } else { GREEN }, GREEN_EDGE, GREEN_EDGE, 0x000000], l.as_str());
                            if hit {
                                self.push_row(cx - half, cx + half, y - uu(2, 1), y + uu(1, 1), Choice::FleetConfirm);
                            }
                            y += uu(3, 1);
                            b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "Didn't turn green? Press Back.");
                            y += uu(2, 1);
                            button(self, b, &mut y, "back", false, Some(Choice::FleetAdd));
                        }
                        button(self, b, &mut y, if f.add >= 5 { "done" } else { "close" }, false, Some(Choice::FleetClose));
                    }
                }
                if matches!(f.state, Some(2) | Some(3)) {
                    button(self, b, &mut y, if f.confirm { "tap again: remove this device" } else { "remove this device" }, f.confirm, Some(Choice::FleetRemove));
                    if f.confirm {
                        b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "wipes from this device: the map's arrangement,");
                        b.text(cx, y + uu(1, 2), note as u16, dim(), 1, "WiFi networks, the last location; not saved");
                        y += uu(3, 1);
                    }
                    if f.leave_result.is_some_and(|r| r != 0) {
                        row(self, b, &mut y, small, white(), "the departure did not start", None);
                    }
                }
                }
            }
            "power" => {
                let on = ferros_sys::pref(ferros_sys::PREF_PROX_WAKE) == 1;
                mark(self, b, &mut y, 50, "proximity wake", on, Some(Choice::ProxWake));
                b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "on, the power key lights the screen");
                b.text(cx, y + uu(1, 2), note as u16, dim(), 1, "only when nothing covers the sensor");
                y += uu(3, 1);
            }
            "connections" => {
                // Each link as its driver says it (measure STATE), each row its own page's door (owner, 2026-11-07).
                let facts = ferros_sys::facts();
                for (k, name) in LINKS.iter().enumerate() {
                    let state = facts.0[33 + 8 * k..41 + 8 * k].iter().filter_map(|&m| ferros_sys::measure_of(m)).find(|m| m.0 == ferros_sys::measure::STATE).map(|m| m.1);
                    let y_row = y;
                    // Switched off by its status circle (preference 15; USB, the sixth, has no switch): the driver's last word outlives it, so the switch is read first.
                    fact(b, &mut y, name, link_state(k, state));
                    if hit {
                        self.push_row(cx - half, cx + half, y_row - uu(2, 1), y, Choice::Link(k as u8));
                    }
                }
            }
            name @ ("cellular" | "WiFi" | "Bluetooth" | "NFC" | "GPS" | "USB") => {
                // A link's own page (owner, 2026-11-07: "the radio details that actually show numbers and config"; the console's circles carry none): its switch, its state, and every measure its driver publishes, each by name in its unit, in the device's base.
                let k = LINKS.iter().position(|l| *l == name).unwrap_or(0);
                let facts = ferros_sys::facts();
                let slots = &facts.0[33 + 8 * k..41 + 8 * k];
                let state = slots.iter().filter_map(|&m| ferros_sys::measure_of(m)).find(|m| m.0 == ferros_sys::measure::STATE).map(|m| m.1);
                if k < 5 {
                    let off = ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF) >> k & 1 == 1;
                    mark(self, b, &mut y, 50, "on", !off, Some(Choice::Switch(k as u8)));
                }
                fact(b, &mut y, "state", link_state(k, state));
                let f = ferros_sys::numbers(Style::Glyphs);
                // Everything the driver publishes (docs/RADIO-CONTROLS.md): its eight slots, then its tracks, grouped (signal, channel, link, counters, motion); tracked satellites listed on their own; then its names and addresses (the detail, read with the Settings capability), then its knobs.
                let mut words = [ferros_sys::MEASURE_NONE; 8 + ferros_sys::TRACKS_MAX];
                words[..8].copy_from_slice(slots);
                let nt = ferros_sys::tracks(k as u64, &mut words[8..]);
                let all = &words[..8 + nt];
                let header = |b: &mut Batch, y: &mut i64, t: &str| {
                    *y += uu(1, 2);
                    b.text(cx, *y, note as u16, dim(), 1, t);
                    *y += uu(2, 1);
                };
                let mut any = false;
                for (g, gname) in GROUPS.iter().enumerate() {
                    let mut first = true;
                    for &w in all {
                        let Some((kind, v)) = ferros_sys::measure_of(w) else { continue };
                        if kind == ferros_sys::measure::STATE || kind == ferros_sys::measure::SAT || group_of(kind) != g {
                            continue;
                        }
                        if first {
                            header(b, &mut y, gname);
                            first = false;
                        }
                        let mut l = Line::new();
                        let what = measure_words(&mut l, f, kind, v);
                        fact(b, &mut y, what, l.as_str());
                        any = true;
                    }
                }
                // Each tracked signal: its system, number and band, where it stands overhead, its carrier to noise, and whether the fix uses it.
                let mut nsat = 0;
                for &w in all {
                    let Some(s) = ferros_sys::sat_of(w) else { continue };
                    if nsat == 0 {
                        header(b, &mut y, "satellites");
                    }
                    nsat += 1;
                    let cnt = |l: &mut Line, n: i64| {
                        if n < 0 {
                            l.s("-");
                        }
                        if let Ok(tatau::Out::Nothing) = tatau::count(l, f, n.unsigned_abs(), 1) {
                            l.s("0");
                        }
                    };
                    let mut name = Line::new();
                    name.s(system_name(s.system));
                    name.s(" ");
                    cnt(&mut name, s.number as i64);
                    name.s(if s.band == 1 { " L5" } else { " L1" });
                    let mut l = Line::new();
                    cnt(&mut l, s.elevation as i64);
                    l.s("° up, ");
                    cnt(&mut l, s.azimuth as i64);
                    l.s("° round, ");
                    cnt(&mut l, s.cn0 as i64);
                    l.s(if s.used { " dB-Hz, in the fix" } else { " dB-Hz" });
                    fact(b, &mut y, name.as_str(), l.as_str());
                }
                if !any && nsat == 0 {
                    fact(b, &mut y, "measures", "none yet");
                }
                // The names and addresses: shown only here, to whoever holds the settings.
                let mut doc = [0u8; 1 << 10];
                if let Some(n) = ferros_sys::detail(cap(self.grants, S_SETTINGS), k as u64, &mut doc) {
                    // WHY the bound: the kernel returns the document's whole length, which the call allows to exceed the buffer; a document this page cannot hold whole is not read.
                    if n > 0 && n <= doc.len() {
                        if let Some(fields) = ferros_detail::parse(k as u64, &doc[..n]) {
                            if !fields.is_empty() {
                                header(b, &mut y, "names and addresses");
                            }
                            for (name, _, v) in fields.iter() {
                                let mut l = Line::new();
                                detail_words(&mut l, f, v);
                                fact_wrap(b, &mut y, detail_label(name), l.as_str());
                            }
                        }
                    }
                }
                // The knobs (ferros_sys::control): a value goes round its choices at each tap, an action happens at its tap.
                header(b, &mut y, "settings");
                let c = ferros_sys::controls(k as u64);
                use ferros_sys::control as ctl;
                match k as u64 {
                    ctl::CELLULAR => {
                        button(self, b, &mut y, "re-register now", false, Some(Choice::Knob(k as u8, ctl::CELL_REREGISTER as u8)));
                        b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "the radio off and on: it registers afresh");
                        y += uu(2, 1);
                    }
                    ctl::WIFI => {
                        // Each value knob a radio group, its choices in its own order (unset reads as `unset`'s: power save follows the screen, the band is any).
                        for (slot, title, names, unset) in [(ctl::WIFI_POWER_SAVE, "power save", &["off", "on", "fast", "with the screen"][..], 3), (ctl::WIFI_BAND, "band", &["any (5 GHz first)", "5 GHz only", "2.4 GHz only"][..], 0)] {
                            let now = if c[slot as usize] == ctl::UNSET { unset } else { c[slot as usize] };
                            b.text(cx, y, note as u16, dim(), 1, title);
                            y += uu(2, 1);
                            radios(self, b, &mut y, &names, now as usize, &|v| Choice::KnobSet(k as u8, slot as u8, v as u8));
                        }
                        b.text(cx, y - uu(1, 1), note as u16, dim(), 1, "the band takes at the next join:");
                        b.text(cx, y + uu(1, 2), note as u16, dim(), 1, "switch WiFi off and on");
                        y += uu(3, 1);
                    }
                    _ => {
                        b.text(cx, y, note as u16, dim(), 1, "nothing to set here yet");
                        y += uu(2, 1);
                    }
                }
            }
            "clock" => {
                // The clock (docs/RADIO-CONTROLS.md): the time here and at UTC in the device's base, the zone and its knob, and where the time comes from.
                let f = ferros_sys::numbers(Style::Glyphs);
                let zone = ferros_sys::zone();
                match ferros_sys::local_time() {
                    Some((secs, ticks)) => {
                        let ns = (ticks * 1_000_000_000 / freq()) as u32;
                        let eagle = ferros_detail::eagle_now();
                        let mut l = Line::new();
                        let _ = tatau::date_time(&mut l, f, secs, ns, eagle);
                        b.text(cx, y, (uu(3, 2)) as u16, white(), 1, l.as_str());
                        y += uu(3, 1);
                        let mut l = Line::new();
                        let _ = tatau::date_time(&mut l, f, secs - zone, ns, eagle);
                        fact(b, &mut y, "at UTC", l.as_str());
                    }
                    None => {
                        fact(b, &mut y, "time", "not read yet");
                    }
                }
                let mut l = Line::new();
                l.s("UTC");
                if zone != 0 {
                    l.s(if zone < 0 { " - " } else { " + " });
                    let _ = tatau::duration_ns(&mut l, f, zone.unsigned_abs() * 1_000_000_000, 1);
                }
                fact(b, &mut y, "zone", l.as_str());
                button(self, b, &mut y, "a quarter hour east", false, Some(Choice::Zone(1)));
                button(self, b, &mut y, "a quarter hour west", false, Some(Choice::Zone(-1)));
                y += uu(1, 2);
                b.text(cx, y, note as u16, dim(), 1, "sources");
                y += uu(2, 1);
                fact(b, &mut y, "from", "the PMIC's clock");
                fact(b, &mut y, "read", "at start, then hourly");
                fact(b, &mut y, "network time", "not decoded yet");
                fact(b, &mut y, "GPS time", "not used yet");
                fact(b, &mut y, "units", base_name(ferros_sys::device_base()));
            }
            "about" => {
                let f = ferros_sys::numbers(Style::Glyphs);
                let facts = ferros_sys::facts();
                b.text(cx, y, (uu(2, 1)) as u16, white(), 1, "ferros");
                y += uu(3, 1);
                row(self, b, &mut y, small, grey(), if DEV { "version Zil, development build" } else { "version Zil" }, Some(Choice::Version));
                // The hint from the fourth tap on, counting down to Developer; it stays until the next tap changes it (no timers).
                if self.taps >= DEV_HINT {
                    let mut l = Line::new();
                    if DEV || self.dev {
                        l.s("Developer is a page already");
                    } else {
                        let _ = tatau::count(&mut l, f, (DEV_TAPS - self.taps) as u64, 1);
                        l.s(" taps to Developer");
                    }
                    b.text(cx, y - uu(1, 1), note as u16, white(), 1, l.as_str());
                    y += uu(2, 1);
                }
                y += uu(1, 1);
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
                fact(b, &mut y, "battery", v.as_str());
                let mut v = Line::new();
                match facts.boot_us() {
                    Some(us) => {
                        let _ = tatau::duration_ns(&mut v, f, us * 1000, 1);
                    }
                    None => {
                        v.s("-");
                    }
                }
                fact(b, &mut y, "press to ready", v.as_str());
                let mut v = Line::new();
                // The counter runs from the SoC's reset: power on, near enough (the bootloader's seconds included).
                let (t, hz) = (now(), freq());
                let _ = tatau::duration_ns(&mut v, f, t / hz * 1_000_000_000 + t % hz * 1_000_000_000 / hz, 1);
                fact(b, &mut y, "since power on", v.as_str());
                let mut v = Line::new();
                if let Some(n) = facts.processes() {
                    let _ = tatau::count(&mut v, f, n, 1);
                }
                fact(b, &mut y, "processes", v.as_str());
                let mut v = Line::new();
                if let Some(n) = facts.vault_keys() {
                    let _ = tatau::count(&mut v, f, n, 1);
                }
                fact(b, &mut y, "in the vault", v.as_str());
                // The hottest part of the SoC as the kernel last read it, and whether the kernel has the big core's clock down for it.
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
                fact(b, &mut y, "chip", v.as_str());
            }
            "kists" => {
                // Each installed kist and what it asked for (owner, 2026-11-08: per kist, its rights, switches), the developer's reason under each; a tap changes the answer for every device of the person's (the list is theirs), and a kist running is killed and started again with the answers as they now stand.
                let ks = self.kists.kists.clone();
                if ks.is_empty() {
                    row(self, b, &mut y, small, grey(), "no kists installed", None);
                }
                for (ki, k) in ks.iter().enumerate() {
                    row(self, b, &mut y, uu(3, 2), white(), &k.name, None);
                    if k.wants.is_empty() {
                        row(self, b, &mut y, note, dim(), "asks for nothing", None);
                    }
                    for (j, w) in k.wants.iter().enumerate() {
                        // A checkbox: on or off, a want never asked saying so.
                        let mut l = Line::new();
                        l.s(ferros_kist::MEANS[w.right as usize]);
                        if w.answer.is_none() {
                            l.s(" (not asked yet)");
                        }
                        mark(self, b, &mut y, 50, l.as_str(), w.answer.is_some_and(|a| a.0), Some(Choice::Answer(ki as u8, j as u8)));
                        y -= uu(3, 4);
                        row(self, b, &mut y, note, dim(), &w.reason, None);
                    }
                    y += uu(1, 1);
                }
            }
            "permissions" => {
                // Each right the other way round (owner, 2026-11-08: "who has GPS", "who has the network"), with the person's default for every new kist first: what its switch starts at in a first-launch question (out of the box, off).
                let ks = self.kists.kists.clone();
                for r in 0..ferros_kist::RIGHTS.len() {
                    row(self, b, &mut y, uu(3, 2), white(), ferros_kist::MEANS[r], None);
                    let on = self.kists.defaults >> r & 1 == 1;
                    mark(self, b, &mut y, 50, "new kists start on", on, Some(Choice::Default(r as u8)));
                    for (ki, k) in ks.iter().enumerate() {
                        if let Some(j) = k.wants.iter().position(|w| w.right as usize == r) {
                            let mut l = Line::new();
                            l.s(&k.name);
                            if k.wants[j].answer.is_none() {
                                l.s(" (not asked yet)");
                            }
                            mark(self, b, &mut y, 50, l.as_str(), k.wants[j].answer.is_some_and(|a| a.0), Some(Choice::Answer(ki as u8, j as u8)));
                        }
                    }
                    y += uu(1, 1);
                }
            }
            _ => {
                // Developer: the panel's and the sensors' knobs.
                let spatial = ferros_sys::pref(ferros_sys::PREF_DITHER) == 1;
                // The temporal dither a checkbox (unchecked, spatial alone: the panel rests); the light scope's channel a radio group.
                mark(self, b, &mut y, 50, "temporal dither", !spatial, Some(Choice::Dither));
                let sc = ferros_sys::pref(ferros_sys::PREF_SCOPE);
                b.text(cx, y, note as u16, dim(), 1, "light scope");
                y += uu(2, 1);
                radios(self, b, &mut y, &["off", "red", "visible", "blue", "green", "IR", "clear"], sc as usize, &|v| Choice::ScopeSet(v as u8));
            }
        }
        self.page_h[p] = y - y0;
    }

    /// A slider's track on a page centred on x = `cx`: its ends. Three quarters of the screen's width at the circle's arrived size, growing and shrinking with it through a flight, and not with the UI scale: the scale's own slider keeps its ends under the finger while the page grows round it.
    fn track(&self, cx: i64) -> (i64, i64) {
        let (_, _, r, _) = self.page_frame();
        let (_, _, ar) = self.arrived();
        let half = self.w * r / ar * 3 >> 3;
        (cx - half, cx + half)
    }
    /// Where a slider's value puts its knob (0 to 1): brightness's stops dimmest on the left (preference 3 counts stops below the brightest), the scale from a half on the left to twice on the right (the pinch's or the drag's while one goes on).
    fn slider_rest(&self, what: Choice) -> f64 {
        match what {
            Choice::Brightness => (255 - ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS)) as f64 / 255.,
            Choice::Scale => (self.scale_live.unwrap_or(ferros_sys::ui_scale() as f32) as f64 + 64.) / 128.,
            _ => 0.,
        }
    }
    /// Where a slider's knob is drawn now: under the finger dragging it, else at its value, off it by its spring's offset once let go between two values.
    fn slider_t(&self, what: Choice, cx: i64) -> f64 {
        if let Some((c, t)) = self.slide {
            if c == what {
                return t;
            }
        }
        let rest = self.slider_rest(what);
        match self.knob {
            Some((c, x0, v0, n)) if c == what => {
                let (a, b) = self.track(cx);
                rest + spring(x0, v0, n).0 / (b - a) as f64
            }
            _ => rest,
        }
    }
    /// A slider set from the finger at x (a drag across it, or a tap): the knob there, and the value written as it changes. Brightness writes its 32nds of a stop through the Settings capability each time the value changes; the scale follows the finger live (`scale_live`) and is kept when the finger lifts, as the pinch's is (every process lays itself out again on a scale change, so once, not every frame).
    fn slide_to(&mut self, what: Choice, x: i64) {
        let (a, b) = self.track(self.w / 2);
        // WHY the bounds: the finger runs past either end of the track, and the knob stays at the end it passed.
        let t = ((x - a) as f64 / (b - a) as f64).clamp(0., 1.);
        self.slide = Some((what, t));
        self.knob = None;
        match what {
            Choice::Brightness => {
                // Continuous (owner, 2026-11-08): 32nds of a stop, the knob's place across the track's 255 of them.
                let v = 255 - libm::round(t * 255.) as u8;
                // Applied live, kept at the lift (`brightness_kept`; owner, 2026-11-08: written into the vault at every 32nd of a stop, a commit under the kernel lock each, the settings ran at 20 fps). A kernel from before refuses the live call: then kept at once, as before.
                if v != ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS) && ferros_sys::set_pref_live(cap(self.grants, S_SETTINGS), ferros_sys::PREF_BRIGHTNESS, v) == ferros_sys::ERR {
                    ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_BRIGHTNESS, v);
                }
            }
            Choice::Scale => self.scale_live = Some((t * 128. - 64.) as f32),
            _ => {}
        }
    }
    /// The finger left a slider: its knob goes from where the finger left it to its value's place on the spring, stepped in display frames.
    fn slide_done(&mut self) {
        let Some((what, t)) = self.slide.take() else { return };
        if what == Choice::Brightness {
            self.brightness_kept();
        }
        let rest = match what {
            // The scale is kept rounded to its 64ths (the lift writes it).
            Choice::Scale => (libm::round(t * 128. - 64.) + 64.) / 128.,
            _ => self.slider_rest(what),
        };
        let (a, b) = self.track(self.w / 2);
        let off = (t - rest) * (b - a) as f64;
        self.knob = if off.abs() >= 0.5 { Some((what, off, 0., 0)) } else { None };
    }
    /// The brightness as the slider left it, kept in the vault (the drag applied it live, `set_pref_live`).
    fn brightness_kept(&self) {
        ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_BRIGHTNESS, ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS));
    }
    /// The settings' row under a panel point, as the last frame laid them out.
    fn row_at(&self, sx: i64, sy: i64) -> Option<Choice> {
        self.rows[..self.nrows].iter().find(|r| sx >= r.0 && sx < r.1 && sy >= r.2 && sy < r.3).map(|r| r.4)
    }

    fn push_row(&mut self, x0: i64, x1: i64, top: i64, bottom: i64, what: Choice) {
        if self.nrows < self.rows.len() {
            self.rows[self.nrows] = (x0, x1, top, bottom, what);
            self.nrows += 1;
        }
    }

    /// A finger dragging on the open settings, at `cur` having been at `prev` last frame and touched down at `start`: locked across or up and down by its first movement past the slop. Across, the strip follows it, the page changing as the strip passes half a screen; up and down, the page scrolls with it, its last three frames' movements kept for the fling.
    fn drag_settings(&mut self, start: (bool, i64, i64), prev: (bool, i64, i64), cur: (bool, i64, i64)) {
        let (dx, dy) = (cur.1 - start.1, cur.2 - start.2);
        if self.swipe == 0 {
            self.swipe = if dx.abs() >= dy.abs() { 1 } else { 2 };
            // Across, from a slider: the drag is the slider's (3), not the strip's; up and down still scrolls the page.
            if self.swipe == 1 && matches!(self.row_at(start.1, start.2), Some(Choice::Brightness | Choice::Scale)) {
                self.swipe = 3;
            }
        }
        if self.swipe == 3 {
            // The slider held since the lock (a scale change lays the page out again, and its row may move from under where the finger landed).
            if let Some(what) = self.slide.map(|s| s.0).or(self.row_at(start.1, start.2)) {
                self.slide_to(what, cur.1);
            }
            return;
        }
        if self.swipe == 1 {
            let moved = if prev.0 { cur.1 - prev.1 } else { 0 };
            self.dxs = [self.dxs[1], self.dxs[2], moved];
            self.page_dx = self.drag_base.0 + dx;
            while self.page_dx > self.w / 2 {
                self.page = self.wrap(self.page as isize - 1);
                self.page_dx -= self.w;
                self.drag_base.0 -= self.w;
                self.scroll = 0;
            }
            while self.page_dx < -self.w / 2 {
                self.page = self.wrap(self.page as isize + 1);
                self.page_dx += self.w;
                self.drag_base.0 += self.w;
                self.scroll = 0;
            }
        } else {
            // WHY the bounds: a page scrolls only as far as it is longer than its band; past either end there is nothing to show.
            let v = (self.drag_base.1 - dy).clamp(0, self.vscroll_max());
            self.set_vscroll(v);
            let moved = if prev.0 { cur.2 - prev.2 } else { 0 };
            self.dys = [self.dys[1], self.dys[2], moved];
        }
    }

    /// One frame of the strip on its way to its page (`page_dx` to 0; `release_settings` picked the page). The page is a detent and the strip a mass on a critically damped spring to it (`spring`; owner, 2026-11-06: "it's clunky ... just needs some ease"), from the offset and the speed it started with, t in display frames. One curve for every case and smooth in every derivative: it leaves at the finger's speed (from rest, easing in), slides into the page with every derivative going to 0, and momentum heading away coasts out and comes back on the same curve; a flick that would pass the page swings past it once, a little, as a detent does. ω is the halving's rate, ln 2 a frame. Computed from the closed form at each frame count, so a missed frame skips a sample, never the motion. A strip left off its page with no spring (a title tapped, a snap caught and let go) starts one from rest.
    fn step_strip(&mut self) {
        let (x0, v0, t, w) = match self.spring {
            Some(s) => s,
            None if self.page_dx != 0 => (self.page_dx as f64, 0., 0, SPRING),
            None => return,
        };
        let t = t + 1;
        let (x, v) = spring_at(x0, v0, t, w);
        // At rest on the page once both the offset and the speed are under half a pixel: nothing more would show.
        if x.abs() < 0.5 && v.abs() < 0.5 {
            self.page_dx = 0;
            self.spring = None;
        } else {
            self.page_dx = libm::round(x) as i64;
            self.spring = Some((x0, v0, t, w));
        }
    }

    /// The finger lifted from a drag on the settings. Up and down, the fling is the largest of the last three frames' movements, the page going on the way the finger last went (owner, 2026-11-05: "maintain the max of the last three distance of finger movements vertically and always subtract one pixel per frame").
    /// Across, the same momentum decides the page (owner, 2026-11-06: a quick swipe from 0 to 49 % must roll on to the next page; "strict first principles physics"). Losing a pixel a frame, speed v goes v + (v − 1) + … + 1 = v(v + 1)/2 pixels before it rests; the page is the one nearest that resting point, as many pages on as the fling reaches. `page` moves there now and `page_dx` becomes the strip's offset from it; the spring (`step_strip`) takes it there, starting at the finger's speed.
    fn release_settings(&mut self) {
        if self.swipe == 1 {
            let m = self.dxs.iter().map(|d| d.abs()).max().unwrap_or(0);
            let last = self.dxs.iter().rev().find(|&&d| d != 0).copied().unwrap_or(0);
            let vx = last.signum() * m;
            let rest = self.page_dx + vx.signum() * m * (m + 1) / 2;
            // The page nearest the resting point, k pages back (positive `page_dx` brings the page before in), and never more than one: left, right or this one (owner, 2026-11-08: "clamp it to one page, no doubles, otherwise it starts to get weird"; a hard fling reached several).
            let k = ((rest + rest.signum() * self.w / 2) / self.w).signum();
            if k != 0 {
                self.page = self.wrap(self.page as isize - k as isize);
                self.page_dx -= k * self.w;
                self.scroll = 0;
            }
            // The spring's rate: the halving's, or, when the strip is already heading for its page faster than that rate would bring it (v0 toward the page above w·|x0|), the rate at which it arrives exactly at the finger's speed: x(t) = (x0 + (v0 + w·x0)·t)·e^(−w·t) crosses the page only where v0 + w·x0 has the opposite sign to x0, so w = −v0/x0 makes the approach a plain decay, never past the page, its speed at release the finger's (owner, 2026-11-08: "it could overshoot when you combine the two and settle").
            let (x0, v0) = (self.page_dx as f64, vx as f64);
            let w = if x0 * v0 < 0. { SPRING.max(-v0 / x0) } else { SPRING };
            self.spring = if self.page_dx != 0 || vx != 0 { Some((x0, v0, 0, w)) } else { None };
        }
        if self.swipe == 2 {
            let m = self.dys.iter().map(|d| d.abs()).max().unwrap_or(0);
            let last = self.dys.iter().rev().find(|&&d| d != 0).copied().unwrap_or(0);
            self.fling = -(last.signum() * m) as f64;
        }
        if self.swipe == 3 {
            self.slide_done();
        }
        self.swipe = 0;
        self.settling = self.page_dx != 0 || self.fling != 0. || self.spring.is_some() || self.knob.is_some();
    }

    /// One display frame of the motion after a finger lifted or a tap (owner, 2026-11-05: frame-counted): the camera's flight (`step_cam`), the settings' strip on its spring (`step_strip`), and the flings — a page's scroll and the plane's pan — carried by their speed under `coast`'s friction, whole pixels moved and the fraction kept for the next frame. Returns whether anything is still moving.
    fn step(&mut self) -> bool {
        self.step_strip();
        // A slider's knob let go between two values: to its value's place on the same spring as the strip, at rest once both its offset and its speed are under half a pixel.
        if let Some((c, x0, v0, t)) = self.knob {
            let (x, v) = spring(x0, v0, t + 1);
            self.knob = if x.abs() < 0.5 && v.abs() < 0.5 { None } else { Some((c, x0, v0, t + 1)) };
        }
        if self.fling != 0. {
            let max = self.vscroll_max();
            let (d, v) = coast(self.fling);
            self.fling_res += d;
            let whole = libm::trunc(self.fling_res);
            self.fling_res -= whole;
            let mut s = self.vscroll() + whole as i64;
            self.fling = v;
            // WHY: the page's (or the list's) ends stop the fling; past them there is nothing to show.
            if s < 0 || s > max {
                s = s.clamp(0, max);
                self.fling = 0.;
            }
            self.set_vscroll(s);
        }
        if let Some((ux, uy, v, rx, ry)) = self.pan {
            let (d, v) = coast(v);
            let (rx, ry) = (rx + ux * d, ry + uy * d);
            let (ix, iy) = (libm::trunc(rx), libm::trunc(ry));
            if ix != 0. || iy != 0. {
                let (wx, wy) = self.cam.delta(ix as i64, iy as i64);
                self.cam.x -= wx;
                self.cam.y -= wy;
                self.keep_on_screen();
            }
            self.pan = if v == 0. { None } else { Some((ux, uy, v, rx - ix, ry - iy)) };
        }
        let flying = self.step_cam();
        self.page_dx != 0 || self.fling != 0. || self.spring.is_some() || self.pan.is_some() || self.knob.is_some() || flying
    }

    /// A helm button's light (display op 54): `corner` 8 home or 9 back, `level` 0 to 1.
    fn helm_light(&self, corner: u8, level: f32) {
        let q = ((level * 65535.0) as u16).to_le_bytes();
        let mut r = [0u8; 4];
        let _ = call(self.display, &[54, corner, q[0], q[1]], &mut r);
    }

    /// The back corner, one level at a time (owner, 2026-11-09: "pressing back ... should stop me trying to leave and go to the regular fleet page, then back again goes to the list of all the categories, then back again should go to the home screen"): on the Fleet page a departure under way is given up (fgtw op 20), the remove row's armed confirm disarmed, an add flow closed; then a settings page goes to the page picker; then the picker, or a kist's page, closes to every circle. What it did, for the console.
    fn back_step(&mut self) -> &'static str {
        if self.view == View::Open(0) && PAGES[self.page] == "fleet" {
            let svc = cap(self.grants, S_FGTW);
            if self.fleet.state == Some(8) {
                let mut r = [0u8; 4];
                let _ = ferros_sys::call_live(svc, &[20], &mut r);
                self.fleet.confirm = false;
                self.fleet.leave_result = None;
                self.fleet.sent = false;
                self.draw();
                return "map: back: the departure given up";
            }
            if self.fleet.confirm {
                self.fleet.confirm = false;
                self.draw();
                return "map: back: the remove row disarmed";
            }
            if self.fleet.add != 0 {
                self.fleet.add_op(svc, &[8, 0]);
                self.draw();
                return "map: back: the add flow closed";
            }
        }
        if self.view == View::Open(0) && !self.picker {
            self.picker = true;
            self.draw();
            return "map: back: the page picker";
        }
        // An open circle closes on the way, as a tap outside it does: the view is the plane once the camera lands (`step_cam`).
        if matches!(self.view, View::Open(_)) {
            self.back = None;
            self.leaving = true;
        }
        let to = self.home();
        self.fly(to);
        self.draw();
        "map: back: zoomed out to every circle"
    }

    /// A tap: one finger or two, at a panel point. Logged with what it did, so a session can be followed from `diag console`.
    fn tap(&mut self, fingers: usize, sx: i64, sy: i64) {
        // The back corner (display op 41: 9), lower left, tapped still: a level back (`back_step`), never a tap on what lies under it.
        if fingers == 1 && sx >= 0 && sy > self.h / 2 {
            let mut m = [41u8, 0, 0, 0, 0];
            m[1..3].copy_from_slice(&(sx as u16).to_le_bytes());
            m[3..5].copy_from_slice(&(sy as u16).to_le_bytes());
            let mut r = [0xFFu8; 1];
            if call(self.display, &m, &mut r) == 1 && r[0] == 9 {
                let said = self.back_step();
                console(self.grants, said.as_bytes());
                console(self.grants, b"\n");
                return;
            }
        }
        let before = self.view;
        // A status circle first (owner, 2026-11-07): they sit over everything at the top, so a one-finger tap on a radio's switches that radio (preference 15) and goes no further. The display knows where they are (op 41).
        if fingers == 1 && sx >= 0 && sy >= 0 && sy < self.h / 4 {
            let mut m = [41u8, 0, 0, 0, 0];
            m[1..3].copy_from_slice(&(sx as u16).to_le_bytes());
            m[3..5].copy_from_slice(&(sy as u16).to_le_bytes());
            let mut r = [0xFFu8; 1];
            if call(self.display, &m, &mut r) == 1 && r[0] < 5 {
                let off = ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF) ^ (1 << r[0]);
                ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_RADIOS_OFF, off);
                let mut l = Line::new();
                let _ = write!(l, "map: status circle {} tapped: radios off G#{off:02X}\n", r[0]);
                console(self.grants, l.as_str().as_bytes());
                return;
            }
        }
        self.tap_inner(fingers, sx, sy);
        let mut l = Line::new();
        let name = |v: View| match v {
            View::Map => ("map", ""),
            View::Open(i) => ("open: ", self.label(i)),
        };
        let (a, b) = name(self.view);
        let _ = write!(l, "map: {fingers}-finger tap at {sx},{sy} -> {a}{b}{}\n", if before == self.view { " (no change)" } else { "" });
        console(self.grants, l.as_str().as_bytes());
    }

    /// While an app is in front: has it handed the screen back?
    fn app_done(&mut self) -> bool {
        let mut r = [0u8; 16];
        let n = call(self.display, &[5], &mut r);
        !(n >= 13 && r[12] == 1)
    }

    fn tap_inner(&mut self, fingers: usize, sx: i64, sy: i64) {
        let inside = {
            let (cx, cy, r) = self.arrived();
            let (dx, dy) = (sx - cx, sy - cy);
            dx * dx + dy * dy <= r * r
        };
        match (fingers, self.view) {
            (1, View::Map) => {
                // A kist installed or removed since: its circle comes or goes before the tap is placed.
                self.refresh_circles();
                if let Some(i) = self.hit(sx, sy) {
                    // A kist circle: launched by init with the person's answers; a want never answered first opens the circle on the question (KISTS.md). The list read again first: another of the person's devices may have answered meanwhile.
                    if i > 0 {
                        self.reload_kists();
                        let Some(k) = self.kist(i) else { return };
                        if !k.unanswered() {
                            self.launch(i);
                            return;
                        }
                        // Each unanswered switch starts at the person's default for its right.
                        self.ask = k.wants.iter().enumerate().fold(0u64, |a, (j, w)| a | ((self.kists.defaults >> w.right & 1) as u64) << j);
                    }
                    let to = self.at(i);
                    // The view to come back to when it closes.
                    self.back = Some(self.cam);
                    self.view = View::Open(i);
                    self.leaving = false;
                    self.fly(to);
                    self.draw();
                }
            }
            (1, View::Open(_)) => {
                if !inside {
                    // Back to the plane: the page shrinks with the circle as the camera flies back to the view it was opened from (a circle open since the start has none: every circle in view), and the view is the plane once it lands (`step_cam`).
                    let to = self.back.take().unwrap_or_else(|| self.home());
                    self.leaving = true;
                    self.fly(to);
                    self.draw();
                    return;
                }
                let row = self.rows[..self.nrows].iter().find(|r| sx >= r.0 && sx < r.1 && sy >= r.2 && sy < r.3).map(|r| r.4);
                // Any other tap starts the version count again, and disarms unlock at boot.
                if row != Some(Choice::Version) {
                    self.taps = 0;
                }
                if row != Some(Choice::UnlockAtBoot) {
                    self.armed = false;
                }
                if row != Some(Choice::FleetRemove) {
                    self.fleet.confirm = false;
                }
                let set = |m: &Map, pref: u64, v: u8| {
                    ferros_sys::set_pref(cap(m.grants, S_SETTINGS), pref, v);
                };
                match row {
                    Some(Choice::Base(code)) => set(self, ferros_sys::PREF_BASE, code),
                    // A slider tapped: its knob and its value there at once, as a drag would leave them; the scale is kept now (the lift that wrote a drag's has passed).
                    Some(c @ (Choice::Brightness | Choice::Scale)) => {
                        self.slide_to(c, sx);
                        self.slide = None;
                        if c == Choice::Brightness {
                            self.brightness_kept();
                        }
                        if let Some(s) = self.scale_live.take() {
                            set(self, ferros_sys::PREF_SCALE, libm::roundf(s) as i8 as u8);
                        }
                    }
                    // A link's value knob set to the choice tapped in its radio group.
                    Some(Choice::KnobSet(link, slot, v)) => {
                        if !ferros_sys::control(cap(self.grants, S_SETTINGS), link as u64, slot as u64, v as u64) {
                            console(self.grants, b"map: the kernel did not take the knob\n");
                        }
                    }
                    Some(Choice::ScopeSet(v)) => set(self, ferros_sys::PREF_SCOPE, v),
                    Some(Choice::Theme(i)) => set(self, ferros_sys::PREF_THEME, i),
                    Some(Choice::Dither) => set(self, ferros_sys::PREF_DITHER, (ferros_sys::pref(ferros_sys::PREF_DITHER) != 1) as u8),
                    Some(Choice::Labels) => {
                        set(self, ferros_sys::PREF_LABELS, (ferros_sys::pref(ferros_sys::PREF_LABELS) != 1) as u8);
                        self.send_scene();
                    }
                    Some(Choice::Layout(i)) => set(self, ferros_sys::PREF_LAYOUT, i),
                    Some(Choice::ProxWake) => set(self, ferros_sys::PREF_PROX_WAKE, (ferros_sys::pref(ferros_sys::PREF_PROX_WAKE) != 1) as u8),
                    // Off in one tap; on in two, the first arming it with the warning.
                    Some(Choice::UnlockAtBoot) => {
                        if ferros_sys::pref(ferros_sys::PREF_UNLOCK_AT_BOOT) == 1 {
                            set(self, ferros_sys::PREF_UNLOCK_AT_BOOT, 0);
                        } else if self.armed {
                            set(self, ferros_sys::PREF_UNLOCK_AT_BOOT, 1);
                            self.armed = false;
                        } else {
                            self.armed = true;
                        }
                    }
                    // Remove this device: the first tap shows what goes with it, the second starts the departure (the words then show here).
                    // Adding a device (payloads/fgtw ops 8, 10, 12, 15): the flow opened and closed, a nearby device picked, a pick confirmed, the keyboard asked for the newcomer's words (1) or a leaver's (2), which the attest screen (ahau) hosts.
                    Some(Choice::FleetAdd) => self.fleet.add_op(cap(self.grants, S_FGTW), &[8, 1]),
                    Some(Choice::FleetClose) => self.fleet.add_op(cap(self.grants, S_FGTW), &[8, 0]),
                    Some(Choice::FleetPick(i)) => {
                        let mut m = [0u8; 9];
                        m[0] = 10;
                        m[1..].copy_from_slice(&self.fleet.cand_key[i as usize]);
                        self.fleet.add_op(cap(self.grants, S_FGTW), &m);
                    }
                    Some(Choice::FleetConfirm) => self.fleet.add_op(cap(self.grants, S_FGTW), &[12]),
                    Some(Choice::FleetWords) => self.fleet.add_op(cap(self.grants, S_FGTW), &[15, 1]),
                    // Approve or Deny: the page says which at once (1 approving, 4 denying, the latter the page's own), then the service is asked; what it did comes back by the doorbell (owner, 2026-11-10: "tapping approve on the 8a doesn't seem to do much").
                    Some(Choice::FleetApprove) => {
                        self.fleet.depart = self.fleet.depart.map(|d| (d.0, 1));
                        self.fleet.left_n = 0;
                        self.draw();
                        self.fleet.add_op(cap(self.grants, S_FGTW), &[14, 1]);
                    }
                    Some(Choice::FleetDeny) => {
                        self.fleet.depart = self.fleet.depart.map(|d| (d.0, 4));
                        self.fleet.left_n = 0;
                        self.draw();
                        self.fleet.add_op(cap(self.grants, S_FGTW), &[14, 0]);
                    }
                    Some(Choice::HomeSync) => {
                        let on = self.fleet.home.is_some_and(|h| h.0);
                        self.fleet.add_op(cap(self.grants, S_FGTW), &[18, !on as u8]);
                    }
                    Some(Choice::FleetRemove) => {
                        if self.fleet.confirm {
                            // The page says the request is going out at once (owner, 2026-11-10: "Second tap on remove device gives zero visual confirmation it took"), then the service is asked.
                            self.fleet.confirm = false;
                            self.fleet.sent = true;
                            self.draw();
                            self.fleet.leave(cap(self.grants, S_FGTW));
                        } else {
                            self.fleet.confirm = true;
                        }
                    }
                    Some(Choice::Version) => {
                        // WHY the stop at twelve: the count means nothing past it, and held there an endless run of taps cannot wrap the u8 back under the hint.
                        if self.taps < DEV_TAPS {
                            self.taps += 1;
                        }
                        // The twelfth tap on a release build: Developer is a page from now on, kept in the vault.
                        if self.taps == DEV_TAPS && !DEV && !self.dev {
                            self.dev = true;
                            if !ferros_sys::store_set(cap(self.grants, S_STORE), b"dev", &[1]) {
                                console(self.grants, b"map: the vault did not take Developer\n");
                            }
                        }
                    }
                    // A neighbour's title: that page, coming in from its side and settling as a swipe does.
                    Some(Choice::Link(k)) => {
                        // Connections' row: that link's own page, straight there.
                        self.show_page(LINKS[k as usize]);
                    }
                    Some(Choice::Switch(k)) => {
                        let off = ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF) ^ (1 << k);
                        set(self, ferros_sys::PREF_RADIOS_OFF, off);
                    }
                    // A link's knob (docs/RADIO-CONTROLS.md): an action counts up, a value goes round its three choices (unset reads as the first).
                    Some(Choice::Knob(link, slot)) => {
                        use ferros_sys::control as ctl;
                        let cur = ferros_sys::controls(link as u64)[slot as usize];
                        let next = match (link as u64, slot as u64) {
                            // WHY the wrap: the driver acts on any change of the count, and from unset (all ones) it wraps to 0, which is a change too.
                            (ctl::CELLULAR, ctl::CELL_REREGISTER) => cur.wrapping_add(1),
                            _ => (if cur == ctl::UNSET { 0 } else { cur } + 1) % 3,
                        };
                        if !ferros_sys::control(cap(self.grants, S_SETTINGS), link as u64, slot as u64, next) {
                            console(self.grants, b"map: the kernel did not take the knob\n");
                        }
                    }
                    // The zone a quarter hour either way (zones are set in quarter hours).
                    Some(Choice::Zone(d)) => {
                        use ferros_sys::control as ctl;
                        let z = ferros_sys::zone() + d as i64 * 900;
                        // WHY the range: no zone lies west of UTC - 12 h or east of UTC + 14 h; a tap past either end does nothing.
                        if (-12 * 3600..=14 * 3600).contains(&z) && !ferros_sys::control(cap(self.grants, S_SETTINGS), ctl::CLOCK, ctl::CLOCK_ZONE, z as u64) {
                            console(self.grants, b"map: the kernel did not take the zone\n");
                        }
                    }
                    Some(Choice::Picker) => {
                        // Up: scrolled so the open page's title stands in the middle of the band, as near as the list's ends allow. Away: the page as it was.
                        self.picker = !self.picker;
                        self.fling = 0.;
                        if self.picker {
                            let (pitch, _, max) = self.picker_metrics();
                            let (_, top, bottom) = self.bands();
                            // WHY the bounds: the list scrolls only from its first title to its last; past them there is nothing to show.
                            self.pick_scroll = (self.page as i64 * pitch - (bottom - top - pitch) / 2).clamp(0, max);
                        }
                    }
                    // An answer turned over on the Kists or Permissions page (never answered reads as off: one tap turns it on): kept in the person's list, and the kist, if running, killed and launched again with it.
                    Some(Choice::Answer(ki, j)) => {
                        self.reload_kists();
                        if let Some(w) = self.kists.kists.get_mut(ki as usize).and_then(|k| k.wants.get_mut(j as usize)) {
                            let on = !w.answer.is_some_and(|a| a.0);
                            w.answer = Some((on, ferros_detail::eagle_now()));
                            if self.save_kists() {
                                self.relaunch(ki as usize);
                            }
                        }
                    }
                    Some(Choice::Default(r)) => {
                        self.reload_kists();
                        self.kists.defaults ^= 1 << r;
                        self.save_kists();
                    }
                    Some(Choice::AskWant(j)) => self.ask ^= 1 << j,
                    // The question answered: every want it showed gets the switch's answer, kept for the person (asked once, ever); then the circle closes and init launches the kist.
                    Some(Choice::AskGo) => {
                        if let View::Open(i) = self.view {
                            let ask = self.ask;
                            let now = ferros_detail::eagle_now();
                            if let Some(k) = i.checked_sub(1).and_then(|k| self.kists.kists.get_mut(k)) {
                                for (j, w) in k.wants.iter_mut().enumerate() {
                                    if w.answer.is_none() {
                                        w.answer = Some((ask >> j & 1 == 1, now));
                                    }
                                }
                            }
                            if self.save_kists() {
                                let to = self.back.take().unwrap_or_else(|| self.home());
                                self.leaving = true;
                                self.fly(to);
                                self.launch(i);
                            }
                        }
                    }
                    Some(Choice::Pick(p)) => {
                        self.page = p as usize;
                        self.page_dx = 0;
                        self.scroll = 0;
                        self.fling = 0.;
                        self.spring = None;
                        self.picker = false;
                    }
                    Some(Choice::Page(k)) => {
                        self.page = self.wrap(self.page as isize + k as isize);
                        self.page_dx += k as i64 * self.w;
                        self.scroll = 0;
                        self.fling = 0.;
                        self.spring = None;
                        self.settling = true;
                        self.frame_seen = None;
                    }
                    None => {}
                }
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
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    // The circles' labels are `&'static str`, absolute addresses fixed at link time; linked position-independent, they are relocations this applies before anything reads them. Without it the first label read faulted at its link-time address, G#D9B (2026-10-21).
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init((&raw mut HEAP) as usize, HEAP_SIZE) };
    let display = cap(grants, S_DISPLAY);
    let touch = cap(grants, S_TOUCH);
    let mut r = [0u8; 256];

    // The panel's size, from the display server.
    if call(display, &[5], &mut r) < 4 {
        console(grants, b"map: no display server\n");
        ferros_sys::exit(1);
    }
    let (w, h) = (u16::from_le_bytes([r[0], r[1]]) as i64, u16::from_le_bytes([r[2], r[3]]) as i64);

    // The circles: settings, then a kist circle for each kist the person installed (their list, `kists.list`; none readable, none shown), sized by share and placed once.
    let kists = ferros_kist::store::read(cap(grants, S_KIST_LIST)).unwrap_or_default();
    // WHY: the scene and the arrays hold MAX circles; kists past it get no circle until there is room (the settings' Kists page still lists them).
    let n = (1 + kists.kists.len()).min(MAX);
    let rs: [i64; MAX] = core::array::from_fn(|i| if i == 0 { radius_of(CIRCLES[0].share) } else if i < n { radius_of(KIST_SHARE) } else { 0 });
    let mut xs = [0i64; MAX];
    let mut ys = [0i64; MAX];
    place(&rs[..n], &mut xs[..n], &mut ys[..n]);
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
        rows: [(0, 0, 0, 0, Choice::Labels); 64],
        page: 0,
        page_dx: 0,
        scroll: 0,
        fling: 0.,
        fling_res: 0.,
        pans: [(0, 0); 3],
        panned: false,
        pan: None,
        page_h: [0; PAGES.len()],
        swipe: 0,
        drag_base: (0, 0),
        dys: [0; 3],
        dxs: [0; 3],
        spring: None,
        settling: false,
        frame_seen: None,
        taps: 0,
        armed: false,
        dev: false,
        fleet: Fleet::new(),
        target: None,
        flight: None,
        back: None,
        leaving: false,
        fade: 0.,
        fast: false,
        page_cam: None,
        touching: false,
        picker: false,
        pick_scroll: 0,
        pinch: None,
        scale_live: None,
        pressed: None,
        slide: None,
        knob: None,
        nrows: 0,
        rc: [ONE; MAX],
        rsn: [0; MAX],
        arranged: [false; MAX],
        app_front: false,
        n,
        kists,
        ask: 0,
    };
    let kept = map.load_places_near();
    // Whatever the vault held, no two circles start overlapping (places kept before circles pushed each other could).
    map.push_apart(MAX);
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
    // Developer, once the twelve taps made it a page (release builds; always one on development builds).
    let mut dv = [0u8; 4];
    map.dev = matches!(ferros_sys::store_get(cap(grants, S_STORE), b"dev", &mut dv), Some(n) if n >= 1 && dv[0] == 1);
    let mut saved_view = map.cam;
    // A development build can start arrived at a circle, its page open, without a finger: FERROS_MAP_START=o<n> (circle n).
    if let Some(v) = option_env!("FERROS_MAP_START").map(str::as_bytes) {
        if v.len() == 2 && (v[1] as usize).wrapping_sub(b'0' as usize) < map.n {
            let i = (v[1] - b'0') as usize;
            map.cam = map.at(i);
            map.view = View::Open(i);
            map.fade = 1.;
        }
        // FERROS_MAP_START=swipe: the settings open, and the synthetic finger (below) swipes across them, for timing the open circle's frames with no one at the panel.
        // FERROS_MAP_START=picker: the same with the page picker up, the synthetic finger scrolling its list.
        if v == b"swipe" || v == b"picker" {
            map.picker = v == b"picker";
            map.cam = map.at(0);
            map.view = View::Open(0);
            map.fade = 1.;
        }
    }
    map.draw();
    // Nothing is in focus (display op 25, icon 0): a display that outlived an older map, one with widgets that took focus, would otherwise keep redrawing one at the panel's rate.
    call(display, &[25, 0, 0, 0, 0, 0, 0], &mut r);
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
    // FERROS_MAP_START=zoom (development, measuring the scene while it grows): two seconds after start, 128 display frames (held four seconds at the largest, for a capture) of a pinch with a twist from the home view, the settings circle and its mark growing past the screen, each frame drawn moving and paced by the panel (op 24), then the view back home at rest; the frames the panel showed to the console.
    if matches!(option_env!("FERROS_MAP_START"), Some("zoom") | Some("zoomcrowd")) {
        // zoomcrowd: with the crowd's 200 circles too (a busy screen drawn whole every frame).
        if option_env!("FERROS_MAP_START") == Some("zoomcrowd") {
            map.crowd(200);
        }
        sleep_until(now() + 2 * freq());
        let home = map.cam;
        map.touching = true;
        let mut fr = [0u8; 4];
        call(display, &[24], &mut fr);
        let (t0, f0) = (now(), u32::from_le_bytes(fr));
        for k in 0..128i64 {
            // At the largest, held four seconds (the frame drawn moving stays on the screen), for a capture.
            if k == 64 {
                sleep_until(now() + 4 * freq());
            }
            let grow = if k < 64 { 1016 } else { 1000_000 / 1016 };
            map.cam.z = map.cam.z * grow / 1000;
            let (c, sn) = (map.cam.c * 16380 / ONE - map.cam.s * 286 / ONE, map.cam.s * 16380 / ONE + map.cam.c * 286 / ONE);
            map.cam.c = c;
            map.cam.s = sn;
            map.draw();
            call(display, &[24], &mut fr);
        }
        let ms = ferros_sys::us_since(t0) / 1000;
        let frames = u32::from_le_bytes(fr).wrapping_sub(f0);
        let mut l = Line::new();
        let _ = write!(l, "map: zoom 128 draws in {ms} ms, {frames} frames on the panel\n");
        console(grants, l.as_str().as_bytes());
        map.touching = false;
        map.cam = home;
        map.draw();
    }
    console(grants, b"map: home drawn; a tap on a circle opens it\n");
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
    let mut scale_seen = ferros_sys::pref(ferros_sys::PREF_SCALE);

    // The previous frame's held contacts, by contact id: present, x, y.
    let mut prev = [(false, 0i64, 0i64); 16];
    // The touch session: from the first finger down to the last one up.
    let mut down = false;
    let mut most = 0usize;
    let mut moved = false;
    // A session that began on one of the helm's corner circles (the display's op 41: 8 home, 9 back; 0 none), and where its first finger is now: the system's gesture, never a pan (owner, 2026-11-07: from the home circle to the centre and let go, full screen on or off).
    let mut corner = 0u8;
    let mut corner_at = (0i64, 0i64);
    // Where that finger landed, and whether the corner has acted this touch.
    let mut corner_from = (0i64, 0i64);
    let mut corner_done = false;
    let mut start = [(false, 0i64, 0i64); 16];
    // The circle the fingers hold, decided again whenever the number of moving fingers changes (one finger: the circle it started on; two: the circle both are inside), and for how many fingers it was decided.
    let mut grab: Option<usize> = None;
    let mut grab_n = 0usize;
    let mut last_app_check = 0u64;
    let mut last_facts = 0u64;
    // The identity space's generation last read (payloads/fgtw op 17, through `fgtw_poller`).
    let mut space_seen = 0u32;
    // The departure waiting here last seen (its intent and the asking device's name), so the Fleet page opens once for each: a request answered clears it, so the same device asking again opens the page again.
    let mut ask_seen: Option<(u8, [u8; NAME_BYTES], usize)> = None;
    // The copy of the identity service last taken (`fgtw_poller`'s sequence count).
    let mut fleet_seen = 0u32;
    let mut last_pass = now();
    let mut draw_us = 0u64;
    let mut move_us = 0u64;
    let mut last_ignored = 0u16;
    let mut push_us = 0u64;
    // The identity service's answers come from a thread of their own (`fgtw_poller`): the main loop never waits on fgtw.
    static mut POLL_STACK: [u8; 1 << 14] = [0; 1 << 14];
    ferros_sys::thread_spawn(fgtw_poller, unsafe { &mut *(&raw mut POLL_STACK) }, grants);
    // FERROS_MAP_START=drag: a synthetic finger instead of the touch server for the first 192 passes — two seconds after start (past the display's first full frame), a pan through the air and back, so the frame pacing can be measured with no one at the panel (it logs passes, draws and the rate).
    let mut synth = 0u32;
    // FERROS_MAP_START=drag: the drag's own rate, for measuring the frame pacing with no one at the panel.
    let (mut synth_t0, mut synth_draws, mut synth_at) = (0u64, 0u32, (0i64, 0i64));
    let map_start = now();
    // The display's frame-start count, from the last frame wait (op 24).
    let (mut frame_count, mut synth_f0) = (0u32, 0u32);
    // FERROS_MAP_START=open: the flights tapped so far, and when the last was.
    let (mut synth_open, mut synth_open_at) = (0u32, 0u64);
    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
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
        // A new UI scale (preference 14, from a pinch here or from the bridge): the page laid out again at it.
        let sc = ferros_sys::pref(ferros_sys::PREF_SCALE);
        if sc != scale_seen {
            scale_seen = sc;
            map.draw();
        }

        // The identity service's copy, when its doorbell has rung (`fgtw_poller`): taken in, and the Fleet page drawn again while it shows. Nothing here asks fgtw anything.
        if let Some((seq, s)) = fleet_take(fleet_seen) {
            fleet_seen = seq;
            map.fleet.take(s);
            if matches!(map.view, View::Open(_)) && PAGES[map.page] == "fleet" && !map.settling {
                map.draw();
            }
        }
        // A fleet device's departure waiting on this person: a new one opens the Fleet page on it, Approve or Deny ready (owner, 2026-11-10: "go right to the fleet page and have the unbind request 'Approve' or 'Deny' screen ready to go"). Not over an app in front, and not while a finger is down or the map is settling: a request waiting opens the page once the fingers lift.
        if !map.app_front && !down && !map.settling {
            let ask = map.fleet.depart.filter(|d| d.1 == 0).map(|d| (d.0, map.fleet.depart_name, map.fleet.depart_name_n));
            if ask.is_some() && ask != ask_seen {
                map.show_page("fleet");
                map.draw();
            }
            ask_seen = ask;
        }

        // The person's identity space (fgtw op 17's generation, in the copy), on the plane with nothing held: a new generation means the identity service wrote this device's kists or home screen from the person's fleet (a reset device getting its arrangement back, a move made on another of their devices), and the circles are read again. Generation 0 is nothing written yet; a map started after a write reads it once more, which changes nothing.
        if map.view == View::Map && !down && !map.settling {
            if let Some(g) = map.fleet.space_gen.filter(|&g| g != space_seen) {
                space_seen = g;
                map.reload_arrangement();
            }
        }

        // The open settings' live facts (About, Connections, each link's page, the clock): drawn again once a second while they show (live data, not motion).
        if matches!(map.view, View::Open(_)) && matches!(PAGES[map.page], "about" | "connections" | "cellular" | "WiFi" | "Bluetooth" | "NFC" | "GPS" | "USB" | "clock") && !map.settling && now().wrapping_sub(last_facts) > freq() {
            last_facts = now();
            map.draw();
        }

        // An app in front (a bridge spawn: Hive, for one), asked of the display eight times a second.
        if !map.app_front && now().wrapping_sub(last_app_check) > freq() / 8 {
            last_app_check = now();
            let mut q = [0u8; 16];
            if call(map.display, &[5], &mut q) >= 13 && q[12] == 1 {
                map.app_front = true;
            }
        }
        // An app in front: it has the touches; the map only watches for it to hand the screen back.
        if map.app_front {
            if map.app_done() {
                map.app_front = false;
                map.draw();
            }
            // Still in front: the touches are its, but the corners are the system's (WINDOWS.md), so they are read on below for a corner gesture, and nothing else is done with them.
        }
        // FERROS_MAP_START=open (development, measuring the flight with no one at the panel): three seconds after start and every two seconds after, while nothing moves, a tap on the settings circle opens it and the next tap, low at the left outside the open circle, closes it; eight flights.
        // FERROS_MAP_START=outhold: one flight in, then the flight out held at its eighth frame (the camera and the fade where they are, nothing stepped), for a capture of the screen mid-flight.
        // FERROS_MAP_START=kist (development, the first launch with no one at the panel): three seconds after start a tap on the last kist circle (the newest installed); two seconds on, if the question is up, a tap on its "start".
        if option_env!("FERROS_MAP_START") == Some("kist") && map.n > 1 && synth_open < 2 && !map.settling && !down && now().wrapping_sub(map_start) > 3 * freq() && now().wrapping_sub(synth_open_at) > 2 * freq() {
            synth_open += 1;
            synth_open_at = now();
            if map.view == View::Map {
                let (sx, sy) = map.cam.screen(w, h, map.xs[map.n - 1], map.ys[map.n - 1]);
                map.tap(1, sx, sy);
            } else if let Some(r) = map.rows[..map.nrows].iter().find(|r| r.4 == Choice::AskGo).copied() {
                map.tap(1, (r.0 + r.1) / 2, (r.2 + r.3) / 2);
            }
        }
        if (option_env!("FERROS_MAP_START") == Some("open") && synth_open < 8 || option_env!("FERROS_MAP_START") == Some("outhold") && synth_open < 2) && !map.settling && !down && now().wrapping_sub(map_start) > 3 * freq() && now().wrapping_sub(synth_open_at) > 2 * freq() {
            synth_open += 1;
            synth_open_at = now();
            if map.view == View::Map {
                let (sx, sy) = map.cam.screen(w, h, map.xs[0], map.ys[0]);
                map.tap(1, sx, sy);
            } else {
                map.tap(1, 4, h - 4);
            }
        }
        // The screen off (the power key): no touches reach the map.
        let t_loop = now();
        let swipe = option_env!("FERROS_MAP_START") == Some("swipe");
        let pick = option_env!("FERROS_MAP_START") == Some("picker");
        let n = if matches!(option_env!("FERROS_MAP_START"), Some("drag") | Some("crowd") | Some("swipe") | Some("picker")) && synth < 192 && now().wrapping_sub(map_start) > 2 * freq() {
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
                // Swiping the settings: from right of the centre, leftward.
                if pick {
                    synth_at = (w / 2, h / 2 + h / 4);
                }
                if swipe {
                    synth_at = (w / 2 + 360, h / 2);
                }
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
                // Swiping, 8 pixels a pass across: 720 out, past half the screen, so the page turns over and back.
                // The picker's list: up and down, 8 pixels a pass, up from below the middle and back.
                let (x, y) = if swipe { (synth_at.0 - off * 8 / 6, synth_at.1) } else if pick { (synth_at.0, synth_at.1 - off * 8 / 6) } else { (synth_at.0, synth_at.1 - off) };
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
                // Only fingers, gloves and styluses count (FTM5 touch types 1-3): a palm (4), a hover (0) or the grip of the hand holding the device at the edge (7) is not a finger — counted as one, it turned a one-finger drag into a two-finger twist around the palm (owner, 2026-10-23: "it's spinning around when I only have one finger down").
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
            map.touching = true;
            most = 0;
            moved = false;
            start = [(false, 0, 0); 16];
            grab = None;
            grab_n = 0;
            // A finger on the open settings catches the strip and the page where they are: the motion stops under it.
            map.swipe = 0;
            map.drag_base = (map.page_dx, map.vscroll());
            map.dys = [0; 3];
            map.dxs = [0; 3];
            map.spring = None;
            map.fling = 0.;
            map.pans = [(0, 0); 3];
            map.panned = false;
            map.pan = None;
            map.settling = map.target.is_some();
            // Where the first finger landed: on the home or back circle, the session is the corner's.
            let f = cur[ids[0]];
            let mut q = [0u8; 4];
            let (xb, yb) = ((f.1 as u16).to_le_bytes(), (f.2 as u16).to_le_bytes());
            let hit = if call(map.display, &[41, xb[0], xb[1], yb[0], yb[1]], &mut q) >= 1 { q[0] } else { 0xFF };
            // A radio's console circle too (0-4, as 16 + its bit): dragged toward the centre it opens that radio's page; tapped still, it switches as before.
            corner = if hit == 8 || hit == 9 { hit } else if hit < 5 { 16 | hit } else { 0 };
            // A helm button under the finger: lit at once (display op 54).
            if corner == 8 || corner == 9 {
                map.helm_light(corner, 0.5);
            }
            corner_at = (f.1, f.2);
            corner_from = (f.1, f.2);
            corner_done = false;
            // Measuring (the first cut did not fire for the owner): a touch in the bottom sixth, what the display said is under it.
            if f.2 > h - h / 6 {
                let mut l = Line::new();
                let _ = write!(l, "map: touch down at {},{}: corner {}\n", f.1, f.2, hit);
                console(grants, l.as_str().as_bytes());
            }
            // A finger on the open settings' controls: the row it is on, which a button shows held (no corner gesture, no app in front).
            map.knob = None;
            map.pressed = if corner == 0 && !map.app_front && matches!(map.view, View::Open(_)) { map.row_at(f.1, f.2) } else { None };
            if map.pressed.is_some() {
                map.draw();
            }
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
                    // A finger that moves is a drag, not a press: the button it was on is let go.
                    map.pressed = None;
                }
            }
            if corner != 0 && held >= 1 {
                corner_at = (cur[ids[0]].1, cur[ids[0]].2);
                // The swipe inward: the finger's travel along the line from where it landed toward the screen's centre. Past a twelfth of the screen's harmonic span (2wh/(w+h); owner, 2026-11-07), the corner acts, once a touch: home, full screen on or off (op 43); back, the map zoomed out to every circle (`home`), the map being recents (WINDOWS.md).
                let (ux, uy) = ((w / 2 - corner_from.0) as f64, (h / 2 - corner_from.1) as f64);
                let inward = ((corner_at.0 - corner_from.0) as f64 * ux + (corner_at.1 - corner_from.1) as f64 * uy) / libm::sqrt(ux * ux + uy * uy);
                let span = (2 * w * h) as f64 / (w + h) as f64;
                // The helm button's light follows the swipe: half at the landing, toward full as the finger comes in (½(1 + tanh(inward / threshold)), C∞, lower again if it pulls away), full once it acted.
                if corner == 8 || corner == 9 {
                    let lvl = if corner_done { 1.0 } else { 0.5 * (1.0 + libm::tanh(inward / (span / 12.))) };
                    map.helm_light(corner, lvl as f32);
                }
                if !corner_done && inward > span / 12. {
                    corner_done = true;
                    if corner & 16 != 0 {
                        // A radio's circle dragged in: its page in settings (with an app in front, the settings circle is not on the screen: nothing).
                        if !map.app_front {
                            map.show_page(LINKS[(corner & 15) as usize]);
                        }
                    } else if corner == 8 {
                        let mut q = [0u8; 4];
                        let n = call(map.display, &[43], &mut q);
                        console(grants, if n >= 2 && q[1] == 1 { b"map: full screen\n" } else { b"map: full screen off\n" });
                    } else {
                        let said = map.back_step();
                        console(grants, said.as_bytes());
                        console(grants, b"\n");
                    }
                }
            }
        }
        if held == 0 && down {
            down = false;
            map.touching = false;
            // The helm button let go: its light out (the display fades it).
            if corner == 8 || corner == 9 {
                map.helm_light(corner, 0.0);
            }
            // A corner's session let go: it acted while moving (above), if it went far enough inward. A still tap on a corner circle is a tap as before (on the home circle it closes the open settings).
            if corner != 0 && !moved {
                corner = 0;
            }
            if corner != 0 {
                corner = 0;
                continue;
            }
            // An app in front: its own touches, nothing for the map.
            if map.app_front {
                continue;
            }
            // A drag on the open settings lifted: the strip snaps, the fling carries on, stepped from the last frame the drag saw.
            if matches!(map.view, View::Open(_)) && map.swipe != 0 {
                map.release_settings();
                map.frame_seen = Some(frame_count);
            } else if matches!(map.view, View::Open(_)) && map.page_dx != 0 {
                // Caught mid-snap and let go without a drag: it goes on settling.
                map.settling = true;
                map.frame_seen = Some(frame_count);
            }
            // The plane panned by one finger lifted: it goes on the way the finger last went, at the largest of its last three frames' movements, as a page's scroll does (owner, 2026-11-06). A finger that had stopped before lifting leaves no fling.
            if map.view == View::Map && map.panned && map.target.is_none() {
                let len = |p: (i64, i64)| libm::sqrt((p.0 * p.0 + p.1 * p.1) as f64);
                let m = map.pans.iter().map(|&p| len(p)).fold(0., f64::max);
                if let Some(&last) = map.pans.iter().rev().find(|p| **p != (0, 0)) {
                    let l = len(last);
                    map.pan = Some((last.0 as f64 / l, last.1 as f64 / l, m, 0., 0.));
                    map.settling = true;
                    map.frame_seen = Some(frame_count);
                }
            }
            // A pinch on the open page lifted: its scale is the device's from now (preference 14, kept in the vault by the kernel), once, not every frame of the pinch. The scale slider's drag ends the same way.
            map.pressed = None;
            map.pinch = None;
            if let Some(s) = map.scale_live.take() {
                ferros_sys::set_pref(cap(grants, S_SETTINGS), ferros_sys::PREF_SCALE, libm::roundf(s) as i8 as u8);
                let mut l = Line::new();
                let _ = write!(l, "map: scale kept: {} 64ths of an octave\n", libm::roundf(s) as i8);
                console(grants, l.as_str().as_bytes());
                map.draw();
            }
            // A flight a touch paused goes on from this frame.
            if map.target.is_some() {
                map.settling = true;
                map.frame_seen = Some(frame_count);
            }
            // Fingers up: the world back to zoom one, then whatever moved (or everything, after a rescale) into the vault, and the view if it changed. The same picture is drawn once more in the rescaled world, so the display holds a frame that matches its scene (its clock redraws replay it).
            // Not while a circle is open or the camera flies: the rescale would move the world under the page and the flight's target.
            if map.view == View::Map && map.target.is_none() {
                map.renormalise();
            }
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

        // A corner's session moves nothing on the map; with an app in front the map does nothing else at all, reading touches only for the corners, a hundred and twenty-eighth of a second apart while one is held, a sixty-fourth between.
        if corner != 0 || map.app_front {
            prev = cur;
            sleep_until(now() + freq() / if down { 1 << 7 } else { 1 << 6 });
            continue;
        }
        // Drags and pinches, once the fingers have moved past the slop: the contacts present both now and last frame are the ones that moved.
        // One finger on the open settings: across goes round the pages, up and down scrolls the page (owner, 2026-11-05); two fingers still leave for the plane.
        if moved && held == 1 && matches!(map.view, View::Open(_)) {
            let id = ids[0];
            map.drag_settings(start[id], prev[id], cur[id]);
            map.draw();
            synth_draws += 1;
        } else if moved && held == 2 && matches!(map.view, View::Open(_)) {
            // Two fingers on the open page: the UI scale follows their spread, 64ths of an octave by log₂ of how far they have opened since they began (owner, 2026-11-05: "makes it so you can easily scale any app any time"), shown live and kept when they lift.
            let (a, b) = (ids[0], ids[1]);
            // WHY the floor of 1: two fingers on one spot have no spread, and log₂ of 0 (or a ratio over 0) is infinite.
            let d = dist16((cur[a].1, cur[a].2), (cur[b].1, cur[b].2)).max(1);
            let (d0, s0) = *map.pinch.get_or_insert((d, map.scale_live.unwrap_or(ferros_sys::ui_scale() as f32)));
            // WHY the bounds: the preference's own range, half to twice (the kernel refuses anything past it; SCALE_REACH).
            map.scale_live = Some((s0 + 64. * libm::log2f(d as f32 / d0 as f32)).clamp(-64., 64.));
            map.draw();
        } else if moved {
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
            if nb >= 1 && map.view != View::Map {
                // Moving away from an arrived circle leaves it: the overlay goes, the plane is free again, and the view it was opened from is not flown back to.
                map.view = View::Map;
                map.back = None;
            }
            if nb == 1 {
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
                    // One finger from the air: the plane follows it, its last three frames' movements kept for the fling.
                    map.cam.x -= dx;
                    map.cam.y -= dy;
                    map.keep_on_screen();
                    map.pans = [map.pans[1], map.pans[2], (cur[id].1 - prev[id].1, cur[id].2 - prev[id].2)];
                    map.panned = true;
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
        // The screen off: no frame starts, so a settle waiting on them never steps and `frame wait` came back each refresh period with nothing done, 120 passes a second in the dark, the X3 near full (2026-11-10). Dark, the map rests an eighth of a second a pass and a settle carries on when the frames do.
        let dark = ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) == 1;
        if dark {
            sleep_until(now() + freq() / (1 << 3));
            continue;
        }
        if held == 0 && !map.settling {
            sleep_until(now() + freq() / 30);
        } else {
            let mut fr = [0u8; 4];
            call(display, &[24], &mut fr);
            frame_count = u32::from_le_bytes(fr);
        }
        // The settings moving after the lift, stepped once per frame the panel started since the last step (owner, 2026-11-05: "count frames and not rendered frames so the animation stays smooth if we miss a frame"): a pass that missed frames steps them all, then draws once. Each draw arms the next frame, so the count moves while anything does. Settled, the loop goes back to resting.
        if held == 0 && map.settling {
            let steps = match map.frame_seen {
                Some(f) => frame_count.wrapping_sub(f),
                None => 1,
            };
            map.frame_seen = Some(frame_count);
            let mut moving = true;
            for _ in 0..steps {
                moving = map.step();
                if !moving {
                    break;
                }
            }
            map.settling = moving;
            if option_env!("FERROS_MAP_START") == Some("outhold") && map.leaving && map.flight.is_some_and(|f| f.t >= 8) {
                map.settling = false;
            }
            if steps > 0 {
                map.draw();
            }
            // Come to rest after the lift (a fling, a flight): the view into the vault if it moved, as the lift does for what the fingers left.
            if !moving && map.cam != saved_view && map.save_view() {
                saved_view = map.cam;
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
