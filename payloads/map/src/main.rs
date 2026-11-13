//! The map (SHELL.md): the device is a map of your life. Every person, message, photo and app is a circle on one plane; the view is a camera over it, and the camera is the only thing that moves.
//!
//! The circles: one is built in, the system's own: the device's settings, one side only (owner, 2026-11-05: "that should only have one side, it's the OS settings page"). Nothing else is (owner, 2026-11-04: "For the baked in circles, there should be one. System settings, that's it. You want a clock? INSTALL IT YOURSELF!!!"): every other circle is a kist the person stows (docs/GLOSSARY.md: the signed capsule as this device checked, compiled and sealed it), a kist circle, and the map knows nothing about it until then. The placement below (a seeded random fit with generous air) is for when there are more.
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
//! Started by init (payloads/init), only once a person has attested (it reads their space). Grant slots: 0 console, 1 display (use), 2 touch (use), 3 settings, 4 the vault scope `map.`, 5 the identity service (use fgtw: the Fleet page, and the identity space: the arrangement and kists written from the person's fleet are read again when its generation moves, docs/IDENTITY-SPACE.md), 6 the vault scope `kists.`, 7 use launch (init), 8 use kists (the stower, payloads/kists: the Deck page stows, updates and unstows through it, stower.rs), 9 use nfc (attest with a tag: the NFC page adds a tag, AUTH.md), 10 use torch (the kernel's, not init's, the slot after the last: the quick row, quick.rs).

#![no_std]
#![no_main]

extern crate alloc;

mod stower;
mod quick;
/// Notes on the deck (docs/NOTIFY.md): the kernel's list, the glows, the resting view, the Deck button, the live takeover.
mod notes;
mod typed;

use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};
use ferros_sys::{call, cap, console, freq, now, sleep_until};
use ferros_lang::{Fleet as FleetMsg, Lang, Msg, Say};
use tatau::Style;

/// A message in the person's language (ferros_lang), read at draw time like every preference.
fn tr(m: Msg) -> &'static str {
    ferros_lang::t(m)
}

/// A message with holes in the person's language, its numbers in their base, as a line.
fn say(m: Say) -> Line {
    let mut l = Line::new();
    let _ = ferros_lang::s(m, &mut l);
    l
}

/// A fleet or identity message (fgtw's, shared with Photon) in the person's language, as a line.
fn fleet(m: FleetMsg) -> Line {
    let mut l = Line::new();
    let _ = ferros_lang::fleet(m, &mut l);
    l
}

/// `t` cut to at most `n` bytes at a character's edge: the display reads every text as UTF-8 and drops one cut inside a character (a translation is two bytes a letter in places).
fn fit(t: &str, n: usize) -> &str {
    let mut k = t.len().min(n);
    // WHY: an op's field is fixed and `t` is a translation or a name of any length; cutting inside a character makes the whole text invalid UTF-8, which the display drops. PROOF: index 0 is always a boundary, so the loop ends.
    while !t.is_char_boundary(k) {
        k -= 1;
    }
    &t[..k]
}

const S_DISPLAY: u64 = 1;
const S_TOUCH: u64 = 2;
const S_SETTINGS: u64 = 3;
const S_STORE: u64 = 4;
/// The identity service (payloads/fgtw): the Fleet page asks it the fleet's state and members, and starts this device's departure.
const S_FGTW: u64 = 5;
/// The vault scope `kists.`: the person's kists and their answers (ferros_kist), read for the kist circles and the settings, written with the person's answers.
const S_KIST_LIST: u64 = 6;
/// Init's `launch` endpoint: a kist circle tapped asks init to launch it, and a kist's card stops it (the map holds none of a kist's rights).
const S_LAUNCH: u64 = 7;
/// The stower's `kists` endpoint (payloads/kists): the Deck page stows, updates and unstows through it, and reads the pandok (the directory) and the jobs under way.
const S_STOWER: u64 = 8;
/// The NFC driver (payloads/nfc): its TAG (op 9) held while the NFC page adds a tag (attest with a tag, AUTH.md; owner, 2026-11-11: "ideally something in settings to add a tag").
const S_NFC: u64 = 9;
/// The torch (payloads/torch, docs/TORCH.md): its send right, which the kernel hands whoever it hands Settings, as the slot after the last (init's grants page is full); the quick row's torch circle calls it. An older kernel leaves the slot empty, a call on it fails, and the circle is drawn dimmed. 9 until init granted use nfc before it (2026-11-11).
const S_TORCH: u64 = 10;

/// The NFC page's "add a tag", as the main loop sets it: 0 not adding, else which add this is (counted from 1), so a tag answered for an add that has since ended is never kept. `tag_add_thread` holds the NFC driver's TAG while it is set and keeps the tag it is answered with (SYS_UNLOCK op 7, under Settings), then writes the outcome to TAG_DONE: the add's count << 2, then 1 kept, 2 kept already, 3 not kept. The main loop reads TAG_DONE each pass (a word, no call), as it reads the GPS copy.
static TAG_ADD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static TAG_DONE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The add's thread (a held call never waits on the main loop's thread): asleep on TAG_ADD while not adding; adding, the NFC driver's TAG held until a tag is read (the person's tap on the page is the consent, AUTH.md "An NFC tap is consent") or the call is let go (op 10: the page went, or the add was cancelled).
extern "C" fn tag_add_thread(grants: u64) -> ! {
    use core::sync::atomic::Ordering;
    let (nfc, settings) = (cap(grants, S_NFC), cap(grants, S_SETTINGS));
    let mut absent = freq() >> 6;
    loop {
        let w = TAG_ADD.load(Ordering::Acquire);
        if w == 0 || TAG_DONE.load(Ordering::Acquire) >> 2 == w {
            ferros_sys::wait_world(&TAG_ADD, w, 0);
            continue;
        }
        let mut uid = [0u8; 10];
        match call(nfc, &[9], &mut uid) {
            // WHY the lengths: the reply's length is the driver's word; only an unlock tag's UID (7 or 10 bytes) is one.
            n @ (7 | 10) => {
                absent = freq() >> 6;
                // The add ended while the call was held: this tag is not kept.
                if TAG_ADD.load(Ordering::Acquire) != w {
                    uid.fill(0);
                    continue;
                }
                let r = match ferros_sys::keep_tag(settings, &uid[..n as usize]) {
                    Some(true) => 1,
                    Some(false) => 2,
                    None => 3,
                };
                uid.fill(0);
                TAG_DONE.store(w << 2 | r, Ordering::Release);
            }
            // No driver behind the slot (NFC switched off and its driver gone, or an init without the slot): asked again after a doubling wait, as `fgtw_poller` does for a service not up, while the add stands.
            ferros_sys::ERR => {
                sleep_until(now() + absent);
                if absent < freq() {
                    absent <<= 1;
                }
            }
            // Let go (op 10): the loop looks at TAG_ADD again.
            _ => {}
        }
    }
}

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
/// The most circles the map holds: settings and the kists the person stowed (KISTS.md), each a kist circle after it.
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
/// The development look's orange (G#RRGGBB, VSF RGB codes at gamma 2): full red, green at half its light, no blue; the hairlines' on development builds and the Developer page's "release look" row while it is off.
const DEV_ORANGE: u32 = 0xFF_B4_00;
/// The circles' hairline colour, as the display draws the app circle's (display `hairline`): on release, grey (equal codes) at half the photopic light of the theme's ring (VSF RGB's weights on its codes, squared); on development builds, VSF RGB orange, full red and green at half its light.
fn hairline() -> u32 {
    if DEV_LOOK && LOOK.load(Ordering::Relaxed) {
        return DEV_ORANGE;
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
/// The helm's swipe inward acts past this fraction of the screen's harmonic span (a sixth: 248 px on the 8a's 1080x2400, 309 on the Pro's 1344x2992), and only mostly inward; a finger that stays within the roll's fraction (a twelfth) and never swiped lifts as a tap (owner, 2026-11-12, on the 8a).
const HELM_SWIPE: f64 = 6.;
const HELM_ROLL: f64 = 12.;
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
    /// The person's identity space (fgtw op 17): this device's deck sync, and whether the first pull from their fleet is done. None until the service answered it.
    deck: Option<(bool, bool)>,
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
    /// Adding a device (fgtw op 9, payloads/fgtw addview.rs): the flow's phase (0 closed, 1 open, 2 adding, 3 a device selected and Yes waiting, 4 handing over the key, 5 added, 6 failed or refused), the devices asking to join (the key's first 8 bytes, flags: bit 0 its beacon heard, bit 1 its request heard on this network, bit 2 the one being added, bit 3 offered as a pick now, bit 4 another identity's; its name), the service's note, and the device being added by name.
    add: u8,
    cand_key: [[u8; 8]; 8],
    cand_flags: [u8; 8],
    cand_name: [[u8; NAME_BYTES]; 8],
    cand_name_n: [u8; 8],
    cands: usize,
    add_note: [u8; 64],
    add_note_n: usize,
    /// The device selected (phase 3): its name and its masked words, the same it shows.
    sel_name: [u8; NAME_BYTES],
    sel_name_n: usize,
    sel_words: [u8; 512],
    sel_words_n: usize,
    /// The device being added (phases 2 and 4): its name.
    adding_name: [u8; NAME_BYTES],
    adding_name_n: usize,
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
        Fleet { state: None, words: [0; 64], words_n: 0, keys: [[0; 8]; 16], me: [false; 16], heard: [false; 16], names: [[0; NAME_BYTES]; 16], names_n: [0; 16], n: 0, confirm: false, leave_result: None, add: 0, cand_key: [[0; 8]; 8], cand_flags: [0; 8], cand_name: [[0; NAME_BYTES]; 8], cand_name_n: [0; 8], cands: 0, add_note: [0; 64], add_note_n: 0, sel_name: [0; NAME_BYTES], sel_name_n: 0, sel_words: [0; 512], sel_words_n: 0, adding_name: [0; NAME_BYTES], adding_name_n: 0, depart: None, depart_name: [0; NAME_BYTES], depart_name_n: 0, denied_by: [0; NAME_BYTES], denied_by_n: 0, deck: None, space_gen: None, sent: false, left: [0; NAME_BYTES], left_n: 0 }
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
        self.deck = ok.then(|| (s[4] == 1, s[5] == 1));
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
        let mut a = [0u8; 2 + 8 * (8 + 2 + NAME_BYTES) + 1 + 255 + 1 + NAME_BYTES + 2 + 512 + 1 + NAME_BYTES];
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
            // The selected device's name and words (fgtw op 9, addview.rs `reply`), as far as the reply and the fields hold them.
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
                at += 2 + len;
            }
            // The device being added (addview.rs `reply`, last), as far as the reply and the field hold it.
            self.adding_name_n = 0;
            if at < got {
                let len = a[at] as usize;
                if at + 1 + len <= got && len <= NAME_BYTES {
                    self.adding_name[..len].copy_from_slice(&a[at + 1..at + 1 + len]);
                    self.adding_name_n = len;
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

    /// An op to the identity service (8 open or close, 10 pick, 12 confirm, 14 approve or deny, 15 the keyboard for words, 18 deck sync). The page's view comes back by the doorbell.
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
    b: [u8; 1 << 8],
    n: usize,
}
impl Line {
    fn new() -> Line {
        Line { b: [0; 1 << 8], n: 0 }
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
        // Cut at a character's edge (`fit`): a line cut inside one would read back as nothing at all.
        let b = fit(s, self.b.len() - self.n).as_bytes();
        self.b[self.n..self.n + b.len()].copy_from_slice(b);
        self.n += b.len();
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
        let mut m = [0u8; 1 << 8];
        m[0] = 2;
        // Signed, as the display reads them (a page sliding off the left edge has its text at negative x); the clamp is the field's own range.
        m[1..3].copy_from_slice(&(x.clamp(-32768, 32767) as i16).to_le_bytes());
        m[3..5].copy_from_slice(&(y.clamp(-32768, 32767) as i16).to_le_bytes());
        m[5..7].copy_from_slice(&px.to_le_bytes());
        m[7..11].copy_from_slice(&colour.to_le_bytes());
        m[11] = align;
        let t = fit(t, m.len() - 12).as_bytes();
        let k = t.len();
        m[12..12 + k].copy_from_slice(&t[..k]);
        self.op(&m[..12 + k]);
    }
    /// Text centred on (x, y) and turned about that centre by (c, s) in Q14, at opacity `alpha` (255 opaque): op 19, the titles turning in.
    fn turned(&mut self, x: i64, y: i64, px: u16, colour: u32, alpha: u8, c: i16, s: i16, t: &str) {
        let mut m = [0u8; 1 << 8];
        m[0] = 19;
        // Signed, as the display reads them; the clamps are the fields' own ranges (a title riding off the screen can have its centre past the edge).
        m[1..3].copy_from_slice(&(x.clamp(-32768, 32767) as i16).to_le_bytes());
        m[3..5].copy_from_slice(&(y.clamp(-32768, 32767) as i16).to_le_bytes());
        m[5..7].copy_from_slice(&px.to_le_bytes());
        m[7..11].copy_from_slice(&colour.to_le_bytes());
        m[11] = alpha;
        m[12..14].copy_from_slice(&c.to_le_bytes());
        m[14..16].copy_from_slice(&s.to_le_bytes());
        let t = fit(t, m.len() - 16).as_bytes();
        let k = t.len();
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
        let mut m = [0u8; 1 << 8];
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
        let t = fit(t, m.len() - 27).as_bytes();
        let k = t.len();
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
        let mut m = [0u8; 1 << 8];
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
        let t = fit(t, m.len() - 28).as_bytes();
        let k = t.len();
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
/// The deck swipe's turn search (`Map::deck_turned`): turns sampled across a half turn, then golden-section steps between the best one's neighbours (each step keeps 0.618 of the bracket: 2⁵ steps take the two samples either side of the best, under six degrees, to about a millionth of one).
const TURN_SAMPLES: usize = 1 << 6;
const TURN_REFINE: usize = 1 << 5;
/// The zoom gain (natural logarithm) at which deck's fit turns half of the way to its best turn: an eighth, about 13% larger; under half of it (6%) the turn is a sixteenth of the way or less, past twice it nearly all (`Map::deck_turned`).
const TURN_GAIN: f64 = 1. / ((1 << 3) as f64);
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

/// The unstow sheet (docs/KISTS.md "The unstow sheet"; owner, 2026-11-12: "upon kist removal, I suspect it may be good to ask if the user wants to clear data? and if so, what categories?", then "Please." to the design): each kind of thing a kist leaves, kept or cleared, and where it goes from. Its defaults are the design's: what it kept for itself cleared (useless without it, and the bulk), what it made for you kept (yours), your answers kept (a stow again asks nothing), its notes always cleared, from all your devices.
#[derive(Clone, Copy)]
struct Sheet {
    capsule: [u8; 32],
    keep_data: bool,
    keep_made: bool,
    keep_answers: bool,
    here_only: bool,
}

/// The sheet's rows a tap turns over (`Choice::UnstowKeep`).
const SHEET_DATA: u8 = 0;
const SHEET_MADE: u8 = 1;
const SHEET_ANSWERS: u8 = 2;

/// What a kist made for the person: its items in the hold (docs/STORE.md). The hold is not built yet, so nothing is; the sheet shows its row only when this says there is something, kept by default (they are the person's; a stow again, or another kist, finds them).
fn hold_items(_developer: &[u8; 32], _option: &[u8; 32]) -> u64 {
    0
}

/// What a tap on the settings does: each row's own, kept with its rectangle as the last frame laid it out.
#[derive(Clone, Copy, PartialEq)]
enum Choice {
    Base(u8),
    Labels,
    Dither,
    /// The display's hit map in every fourth frame (Developer; display op 58).
    HitMap,
    /// One of the panel's channels on or off (Developer; display op 63): 0 red, 1 green, 2 blue.
    Channel(u8),
    /// The release look until a reboot (Developer; display op 64).
    ReleaseLook,
    /// One of the theme swatches.
    Theme(u8),
    /// The UI scale slider (preference 14), half to twice: the scale decided by x.
    Scale,
    /// A typed setting's slider (typed.rs, its index in `typed::ALL`; brightness one since 2026-11-12): its kept byte decided by x, live while the finger is on it and kept at the lift.
    Typed(u8),
    /// The torch page's on and off: the torch lit at its level (and strobe, and timeout) or dark (the quick circle's tap until 2026-11-12; its tap opens this page now).
    TorchOn,
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
    /// The Fleet page's "this device's deck": follows the person's fleet, or keeps its own (payloads/fgtw op 18; docs/IDENTITY-SPACE.md).
    DeckSync,
    /// A link on Connections: its own page (LINKS order).
    Link(u8),
    /// A radio's page: its switch, as its console circle's tap (preference 15).
    Switch(u8),
    /// A link's knob (docs/RADIO-CONTROLS.md): its link and slot in the control blocks (ferros_sys::control); a value knob goes round its choices, an action counts up.
    Knob(u8, u8),
    /// The clock's fixed offset (no zone), a quarter hour west (-1) or east (1).
    Zone(i8),
    /// The clock's zone (kernel zone.rs, docs/TIME.md): "follow my identity" turned over; the ruling choice automatic (true) or by hand; the picker opened, or one level back; a region in it (its index in the picker's regions); a zone (its index in the zones list); no zone, a fixed offset.
    ZoneFollow,
    ZoneAuto(bool),
    ZonePicker,
    ZoneBack,
    ZoneRegion(u16),
    ZoneCity(u16),
    ZoneNone,
    /// The language page (SETTINGS.md): "follow my identity" turned over; a language picked, by its index (fgtw::text::Lang::index).
    LangFollow,
    Lang(u8),
    /// The title: the page picker up or away.
    Picker,
    /// A page's title in the picker: that page.
    Pick(u8),
    /// A kist's want on its card or the Permissions page: (the kist in the person's list, the want), its answer turned over.
    Answer(u8, u8),
    /// The Deck page: a thing's unstow (its place in the person's list: a kist's opens its unstow sheet, a puck's "remove from the deck" is two taps), and the pandok's stow or update (its row in the directory).
    Unstow(u8),
    StowListed(u8),
    /// The unstow sheet (docs/KISTS.md "The unstow sheet"): a kind of thing kept or not (`SHEET_*`), where it goes from (true: this device only), and the sheet's Unstow or Cancel.
    UnstowKeep(u8),
    UnstowFrom(bool),
    UnstowGo,
    UnstowCancel,
    /// The Deck page: a row's card opened (its place in the person's list), and back to the list.
    Card(u8),
    CardBack,
    /// A kist's card: its running process stopped (init's `OP_STOP`).
    Stop(u8),
    /// A kist offered over USB, on the Deck page: Stow (true) or Cancel (false), the person's answer to the stower (docs/KISTS.md "Stowing").
    Stow(bool),
    /// A right's default for new kists (Permissions).
    Default(u8),
    /// The first-launch question: a want's switch (its index in the kist's wants).
    AskWant(u8),
    /// The first-launch question answered: the answers kept and the kist launched.
    AskGo,
    /// The NFC page's "add a tag" (or its cancel while it waits), and a kept tag's remove (its place in the list), attest with a tag (AUTH.md).
    TagAdd,
    TagRemove(u8),
    /// A live note's page (notes.rs `live_page`, docs/NOTIFY.md "The live takeover"): Answer opens its kist at the note, Decline lets it go.
    LiveAnswer,
    LiveDecline,
}

/// The settings pages, the ring a swipe goes round (Photon's split, photon docs/settings-panel.md; owner, 2026-11-05: "I like the page scopes and your organization"); a page for each link after Connections (owner, 2026-11-07: a radio's numbers and config, reached from its console circle by a drag toward the centre, or from Connections). Developer is a page only on development builds, or once the twelve taps made it one. The torch after the battery (owner, 2026-11-11: its brightness, strobe, lit share and timeout; QUICK.md "Quick pages"), opened by a tap on its quick circle as well (a drag down from it, since 2026-11-12, is its level), as Appearance, the look's one page, is by a tap on the brightness ring. The clock after them (docs/RADIO-CONTROLS.md): its time, zone and sources. The language after the clock, as the zone's twin (SETTINGS.md). The Deck, first named Items (owner, 2026-11-12: "make sure it says items in settings, not kists"): everything on the deck as a circle, by kind, each row opening its card. These are the pages' keys (the code finds a page by them); what a person reads is `page_title`.
const PAGES: [&str; 21] = ["appearance", "units", "keyboard", "security", "deck", "permissions", "fleet", "power", "connections", "cellular", "WiFi", "Bluetooth", "NFC", "GPS", "USB", "battery", "torch", "clock", "language", "about", "developer"];
/// Whether this is a development build (the display's own test, `FERROS_PRODUCTION` unset): Developer is always a page there.
const DEV: bool = option_env!("FERROS_PRODUCTION").is_none();
/// Whether it starts looking like one (the orange hairlines, as the display's): FERROS_RELEASE_LOOK set gives a development build the release look for screen captures (owner, 2026-11-10), its tools kept.
const DEV_LOOK: bool = DEV && option_env!("FERROS_RELEASE_LOOK").is_none();
/// The development look as it stands, the display's (its op 64): off for good once the Developer page's "release look" is tapped, until a reboot (owner, 2026-11-11: "upon tap in UI it goes black and stays until a reboot"); a map started again asks the display.
static LOOK: AtomicBool = AtomicBool::new(DEV_LOOK);
/// Taps in a row on About's version line that make Developer a page (owner, 2026-11-05: "twelve"), and the tap from which the hint shows and counts down ("don't show a toast hint until four taps"). No time window: any other tap starts the count again.
const DEV_TAPS: u8 = 12;
const DEV_HINT: u8 = 4;
/// The radios and USB in the kernel's facts order (facts 33 + 8 · link, ferros_sys::measure), as the Connections page names them.
const LINKS: [&str; 6] = ["cellular", "WiFi", "Bluetooth", "NFC", "GPS", "USB"];

/// A console circle's page by its zone's id (the display's dash zones, op 41): the radios and USB their link pages, 13 the clock, 14 the battery.
fn dash_page(id: u8) -> &'static str {
    match id {
        13 => "clock",
        14 => "battery",
        // 0-5 by the facts' order; the display gives no other id here.
        _ => LINKS[id as usize % LINKS.len()],
    }
}

/// A page's title in the person's language, by its key in `PAGES` (a link's own name is a name, and stays; cellular is a word, and is translated).
fn page_title(key: &'static str) -> &'static str {
    tr(match key {
        "appearance" => Msg::PageAppearance,
        "units" => Msg::PageUnits,
        "keyboard" => Msg::PageKeyboard,
        "security" => Msg::PageSecurity,
        "deck" => Msg::PageDeck,
        "permissions" => Msg::PagePermissions,
        "fleet" => Msg::PageFleet,
        "power" => Msg::PagePower,
        "connections" => Msg::PageConnections,
        "cellular" => Msg::PageCellular,
        "clock" => Msg::PageClock,
        "language" => Msg::PageLanguage,
        "about" => Msg::PageAbout,
        "battery" => Msg::Battery,
        "torch" => Msg::Torch,
        "developer" => Msg::PageDeveloper,
        _ => return key,
    })
}

/// A link's state in words: switched off by its circle (preference 15; USB, the sixth, has no switch) before anything its driver last said, which outlives it.
fn link_state(k: usize, state: Option<i64>) -> &'static str {
    if k < 5 && ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF) >> k & 1 == 1 {
        return tr(Msg::SwitchedOff);
    }
    // GPS links to nothing (owner, 2026-11-10, of its state row "on, no link": "for GPS say searching until a fix"): its driver's 1 is no fix yet and 2 a fix (payloads/gps `now_shown`).
    match (k, state) {
        (_, Some(0)) => tr(Msg::Off),
        (4, Some(1)) => tr(Msg::Searching),
        (4, Some(2)) => tr(Msg::HasFix),
        (_, Some(1)) => tr(Msg::OnNoLink),
        (_, Some(2)) => tr(Msg::Connected),
        _ => "-",
    }
}

/// What a right is, in the person's words and language, by its place in ferros_kist::RIGHTS (whose MEANS are the English).
fn right_name(r: usize) -> &'static str {
    const NAMES: [Msg; ferros_kist::RIGHTS.len()] = [Msg::RightDisplay, Msg::RightTouch, Msg::RightSurface, Msg::RightIra, Msg::RightDomain, Msg::RightEntropy, Msg::RightGps, Msg::RightUdp, Msg::RightAudio];
    tr(NAMES[r])
}

/// A theme's name in the person's language, by its place in ferros_sys::theme::THEMES (whose `name`s are the English).
fn theme_name(i: usize) -> &'static str {
    const NAMES: [Msg; ferros_sys::theme::THEMES.len()] = [Msg::ThemeGrey, Msg::ThemeSlate, Msg::ThemeEmber, Msg::ThemeNight];
    tr(NAMES[i])
}

/// The languages the language page offers, in Photon's picker order: those the display's face can draw (ferros_lang::renders; the others wait for a face).
fn languages() -> impl Iterator<Item = Lang> {
    Lang::ALL.into_iter().filter(|&l| ferros_lang::renders(l))
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
            tr(Msg::Technology)
        }
        m::RSSI => { num(l, v / 1000, " dBm"); tr(Msg::Signal) }
        m::NOISE => { num(l, v / 1000, " dBm"); tr(Msg::NoiseFloor) }
        m::SNR => { num(l, v / 1000, " dB"); tr(Msg::SignalToNoise) }
        m::RSRP => { num(l, v / 1000, " dBm"); tr(Msg::ReferencePower) }
        m::RSRQ => { num(l, v / 1000, " dB"); tr(Msg::ReferenceQuality) }
        m::SINR => { num(l, v / 1000, " dB"); tr(Msg::SignalToInterference) }
        m::VSWR => { num(l, v / 1000, ":1"); tr(Msg::AntennaMatch) }
        m::TX_POWER => { num(l, v / 1000, " dBm"); tr(Msg::TransmitPower) }
        m::FREQ if v >= 100_000 => { num(l, v / 1000, " MHz"); tr(Msg::Frequency) }
        m::FREQ => { num(l, v, " kHz"); tr(Msg::Frequency) }
        m::BANDWIDTH => { num(l, v / 1000, " MHz"); tr(Msg::ChannelWidth) }
        m::CHANNEL => { num(l, v, ""); tr(Msg::Channel) }
        m::RATE_DOWN | m::RATE_UP => {
            let _ = tatau::size_bits(l, f, v.unsigned_abs(), 1);
            l.s("/s");
            if kind == m::RATE_DOWN { tr(Msg::RateDown) } else { tr(Msg::RateUp) }
        }
        m::LATENCY => { let _ = tatau::duration_ns(l, f, v.unsigned_abs(), 1); tr(Msg::RoundTrip) }
        m::LOSS => { let _ = tatau::share(l, f, v.unsigned_abs(), 1_000_000, 2); tr(Msg::Loss) }
        m::MCS => { num(l, v, ""); "MCS" }
        m::STREAMS => { num(l, v, ""); tr(Msg::Streams) }
        m::SATS_SEEN => { num(l, v, ""); tr(Msg::SatellitesSeen) }
        m::SATS_USED => { num(l, v, ""); tr(Msg::SatellitesInFix) }
        m::CN0 => { num(l, v / 1000, " dB-Hz"); tr(Msg::BestCarrierToNoise) }
        m::ACCURACY => { num(l, v / 1000, " m"); tr(Msg::Accuracy) }
        m::REGISTRATION => {
            l.s(tr(match v {
                0 => Msg::NotRegistered,
                1 => Msg::RegHome,
                2 => Msg::Searching,
                3 => Msg::Denied,
                5 => Msg::Roaming,
                20 => Msg::EmergencyOnly,
                _ => Msg::Unknown,
            }));
            tr(Msg::Registration)
        }
        m::MTU => { let _ = tatau::size_bits(l, f, v.unsigned_abs() * 8, 1); tr(Msg::LargestPacket) }
        m::PACKETS_DOWN => { num(l, v, ""); tr(Msg::PacketsIn) }
        m::PACKETS_UP => { num(l, v, ""); tr(Msg::PacketsOut) }
        m::ERRORS_DOWN => { num(l, v, ""); tr(Msg::BadPacketsIn) }
        m::ERRORS_UP => { num(l, v, ""); tr(Msg::FailedPacketsOut) }
        m::DROPPED_DOWN => { num(l, v, ""); tr(Msg::DroppedIn) }
        m::DROPPED_UP => { num(l, v, ""); tr(Msg::RefusedOut) }
        m::SUSPENDS => { num(l, v, ""); tr(Msg::LinkNaps) }
        m::POWER_SAVE => {
            l.s(match v {
                0 => tr(Msg::Off),
                1 => tr(Msg::On),
                2 => tr(Msg::Fast),
                _ => "-",
            });
            tr(Msg::PowerSave)
        }
        m::LINK_POWER => {
            l.s(match v {
                0 => tr(Msg::Working),
                1 => tr(Msg::LightSleep),
                2 => tr(Msg::Sleep),
                3 => tr(Msg::Suspended),
                _ => "-",
            });
            tr(Msg::LinkPower)
        }
        m::ALTITUDE => { num(l, v / 1000, " m"); tr(Msg::Altitude) }
        m::SPEED => { num(l, v / 1000, " m/s"); tr(Msg::Speed) }
        m::HEADING => { num(l, v / 1000, "°"); tr(Msg::Heading) }
        _ => { num(l, v, ""); tr(Msg::Measure) }
    }
}

/// The groups a link's page shows its measures in (docs/RADIO-CONTROLS.md), in order: each kind's group.
const GROUPS: [Msg; 5] = [Msg::Signal, Msg::Channel, Msg::GroupLink, Msg::GroupCounters, Msg::GroupMotion];
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

/// A kist the display draws itself in its circle (a widget: its scene icon at G#80 and up, display WIDGET_*), by the kist's name: the clock (owner, 2026-11-10: "my clock widget kist on the homescreen again"). Stowed like any app (payloads/clock, tools/clock-kist.sh), never launched.
fn widget(name: &str) -> Option<u8> {
    name.eq_ignore_ascii_case("Clock").then_some(CLOCK_FACE)
}

/// The display's clock face as a scene icon (display `WIDGET_CLOCK`, G#80).
const CLOCK_FACE: u8 = 1 << 7;

/// What a puck's circle shows, by the display's icon for it (`widget`), for its card on the Deck.
fn puck_shows(icon: u8) -> Msg {
    match icon {
        CLOCK_FACE => Msg::TheTime,
        _ => Msg::Unknown,
    }
}

/// Why a kist did not start, or stopped after it started (ferros_kist::launch::why), in the person's words: under its open circle (`ask_page`) and on its card.
fn why_words(why: u8) -> Msg {
    use ferros_kist::launch::why as w;
    match why {
        w::ROOM => Msg::NotEnoughMemory,
        w::NOT_HERE => Msg::NotStowedHere,
        w::CHECKS => Msg::FailedItsChecks,
        w::READ => Msg::CouldNotBeRead,
        // Started, then stopped (a fault or a panic: the kind is on the console; a kill from outside).
        w::FAULT | w::PANIC => Msg::ItCrashed,
        w::KILLED => Msg::ClosedFromOutside,
        _ => Msg::SomethingWentWrong,
    }
}

/// A key's first four bytes, as a card shortens a developer's key or a kist's option.
fn short_key(l: &mut Line, k: &[u8; 32]) {
    let _ = write!(l, "G#{:02X}{:02X}{:02X}{:02X}", k[0], k[1], k[2], k[3]);
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


/// Whether the Fleet page is on screen, as the main loop last saw it (bit 0), counted in the bits above so each change is a new word: `fleet_page_thread` opens fgtw's add flow while it shows and closes it when it goes (owner, 2026-11-10: adding a device, by a touch on its row or by NFC, needs "the fleet page open").
static FLEET_PAGE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The add flow following the Fleet page (fgtw op 8 [1] or [0]), on a thread of its own: a call waits as long as fgtw's pass, and the main loop never waits on fgtw (`fgtw_poller`). Asleep on FLEET_PAGE between changes; a refused call (fgtw not serving yet) is made again after the doubling wait `fgtw_poller` uses.
extern "C" fn fleet_page_thread(grants: u64) -> ! {
    use core::sync::atomic::Ordering;
    let svc = cap(grants, S_FGTW);
    let mut done = 0u64;
    let mut absent = freq() >> 6;
    loop {
        let v = FLEET_PAGE.load(Ordering::Acquire);
        if v == done {
            ferros_sys::wait_world(&FLEET_PAGE, v, 0);
            continue;
        }
        let mut r = [0u8; 4];
        if ferros_sys::call_live(svc, &[8, (v & 1) as u8], &mut r) == ferros_sys::ERR {
            sleep_until(now() + absent);
            if absent < freq() {
                absent <<= 1;
            }
            continue;
        }
        absent = freq() >> 6;
        done = v;
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
    tr(match field {
        "carrier" => Msg::Carrier,
        "apn" => Msg::DataApn,
        "attach_apn" => Msg::AttachApn,
        "ipv4" => Msg::Address,
        "ipv6" => Msg::AddressIpv6,
        "dns4" | "dns" => return "DNS",
        "dns6" => Msg::DnsIpv6,
        "ssid" => Msg::Network,
        "bssid" => Msg::AccessPoint,
        "mac" => Msg::ThisDeviceOnIt,
        "router" => Msg::Router,
        "lease" => Msg::Lease,
        _ => Msg::Detail,
    })
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
        _ => tr(Msg::OtherSystem),
    }
}

/// The four bases in the order the chooser shows them: the house base first.
const BASES: [u8; 4] = [12, 0x8C, 10, 16];

fn base_name(code: u8) -> &'static str {
    tr(match code {
        10 => Msg::BaseArabic,
        16 => Msg::BaseHex,
        0x8C => Msg::BaseDls,
        _ => Msg::BaseDms,
    })
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
    /// A flight's trace (development builds): each step's zoom and turn left, as per mille of where they began, `|` between the frames drawn; said on the console as it lands.
    flight_trace: alloc::string::String,
    /// Taps in a row on About's version line; the unlock-at-boot row armed by its first tap.
    taps: u8,
    armed: bool,
    /// Developer made a page by the twelve taps (the vault, `map.dev`).
    dev: bool,
    /// Whether the display shows its hit map in every fourth frame (display op 58; development builds): the Developer page's checkbox and the two-finger swipe up from back and deck together switch it, and the display's answer is kept here.
    hit_view: bool,
    /// The panel's channels as the display has them (display op 63; development builds): bit 0 red, 1 green, 2 blue, set = on; the Developer page's three checkboxes.
    channels: u8,
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
    /// An app is in front (a kist, or one the map did not start: a bridge spawn), and it has the touches.
    app_front: bool,
    /// The kists brought to the front, most recent first, by capsule (owner, 2026-11-11: back's swipe "flip to the previous kist"): a kist launched or raised goes to the head; each once.
    recent: alloc::vec::Vec<[u8; 32]>,
    /// The kist this map brought to the front last, while it is still in front (None once the map has the glass again).
    front_kist: Option<[u8; 32]>,
    /// How many circles there are: settings, then a kist circle for each kist in the person's list (at most `MAX`).
    n: usize,
    /// The person's kists and their answers (`kists.list`, ferros_kist), as last read: kist circle i is `kists.kists[i - 1]`.
    kists: ferros_kist::Kists,
    /// The first-launch question on an open kist circle: each unanswered want's switch as the person has it now, a bit per want (started from their defaults).
    ask: u64,
    /// The clock page's zone picker while it is open (`ZonePick`), and the last word on a choice that did not take (the clock's or the language's), shown until the next tap on the page, in the language in effect when it draws.
    zone_pick: Option<ZonePick>,
    zone_note: Option<Msg>,
    /// The stower as last read (stower.rs): the directory's kists and each job under way.
    stower: stower::Stower,
    /// The Deck page's unstow armed on a kist or a puck, by its place in the person's list (the first tap of two, as "remove this device").
    unstow_armed: Option<u8>,
    /// The Deck page's card open, by the capsule of the kist or puck it is for (owner, 2026-11-12: a row opens its card, the one way to see a kist's info); None on the list. Gone with it when that thing is unstowed.
    card: Option<[u8; 32]>,
    /// The unstow sheet open on a kist's card, in the card's place (docs/KISTS.md "The unstow sheet"): what is kept and from where, as the person has it so far. Gone with the card.
    sheet: Option<Sheet>,
    /// What the sheet's kist kept for itself, as the stower last told it (stower.rs `size_take`): its capsule, and its bytes and values, None when the vault did not say.
    sheet_size: Option<([u8; 32], Option<(u64, u64)>)>,
    /// The GPS page's claim (owner, 2026-11-10: "open the GPS screen (which should turn it on whilist GPS screen is open)"; ferros_sys::claim): held while the page shows, the screen is on and GPS is switched on.
    gps_claimed: bool,
    /// The driver as `gps_watch` last read it while the page holds its claim, at each ring of its doorbell: None not answering yet (starting, its session replaying), Some(None) no fix yet, Some(Some(fix)) the fix. On the page only, never logged, dropped as the page goes.
    gps_fix: Option<Option<GpsFix>>,
    /// Attest with a tag (the NFC page, AUTH.md): the kept tags as the kernel last listed them (SYS_UNLOCK op 9, read as the page comes and after each change; never the UIDs, which the kernel never gives), how many, and what the last add said (0 nothing, else `TAG_DONE`'s outcome).
    tags: [ferros_sys::KeptTag; 1 << 4],
    ntags: usize,
    tag_said: u64,
    /// How many adds this map has started (`TAG_ADD`'s values).
    tag_adds: u64,
    /// The kist this map last asked init to launch and was told "started" (ferros_kist::launch::STARTED), with the kists' bell count as it stood then (KISTS_WORD): a launch the kernel did not start after all comes back with a later count (ferros_sys::not_started), and only that one is said. Dropped once it is said.
    asked: Option<([u8; 32], u64)>,
    /// Why that kist did not start, or stopped after it started (ferros_kist::launch::why), for its circle (owner, 2026-11-11: Mahere tapped, "nothing opened", it looked stuck; then Mahere faulting at once, "no toast or anything"): the capsule, the reason, and whether its circle is open on it yet. Not yet: it waits for the plane at rest with nothing held (`show_fault`); then shown on the open circle until the circle closes.
    fault: Option<([u8; 32], u8, bool)>,
    /// Kists that started and then stopped abnormally (ferros_kist::launch::why::stopped: a fault, a panic, a kill from outside) while not the one asked last (owner, 2026-11-11: Mahere faulted at once, "no toast or anything"): each said once, as its circle is next tapped, before any launch (`tap_inner`). Dropped when the kernel no longer keeps it (that kist started again since).
    stopped: alloc::vec::Vec<([u8; 32], u8)>,
    /// The kists' bell count of the newest record already taken (`kists_rang`): a record at or under it was said or passed over before.
    kists_read: u64,
    /// The kists alive as the kernel last said (ferros_sys::not_started::live, at each ring of the kists' bell), by capsule; None until it has (a kernel without op 2), when kist circles show no state.
    live: Option<alloc::vec::Vec<[u8; 32]>>,
    /// The kists the kernel keeps a record for (ferros_sys::not_started::list: a start that did not happen, or a stop by a fault, a panic or a kill from outside), by capsule: stopped on their circles until they start again, each with its reason (ferros_kist::launch::why) for its card on the Deck.
    ended: alloc::vec::Vec<([u8; 32], u8)>,
    /// Things waiting on the person, by kist capsule, with how many: a for-you glow, its reach growing with the count. Nothing adds to it yet: ferros has no notifications (owner, 2026-11-11: for you is "anything waiting on the person"), and whatever brings them sets the count here and calls `send_states`.
    waiting: alloc::vec::Vec<([u8; 32], u16)>,
    /// Each circle's state as last sent to the display (op 57: the glow's byte and the count), so a ring of the kists' bell sends only what changed.
    sent: [(u8, u16); MAX],
    /// The notes waiting on the person as the kernel last listed them (notes.rs, docs/NOTIFY.md), the newest one taken (its bell count: `notes_rang` acts on each once), and when the earliest deadline among them passes (counter ticks; the main loop's rest ends there and the list is read again).
    notes: Vec<notes::Rec>,
    notes_read: u64,
    notes_due: Option<u64>,
    /// The deck's resting view (owner, 2026-11-12: the reach is the view): the camera as the person last settled the deck; a circle whose disc meets it can notify, out of it is silent. A pan that does not settle changes nothing, and an open kist leaves it as it was.
    rest: Cam,
    /// The kists that tapped the motor since the person last looked at them: one tap a kist until then.
    tapped: Vec<[u8; 32]>,
    /// The Deck button's note as last sent (display op 66): the icon, the count in view (or the questions waiting), and its glow (ferros_glow: for you, or a question).
    helm_sent: (u8, u16, u8),
    /// The system's questions waiting on the person, oldest first, each with its note's number when the kernel took it (notes.rs `questions_sync`, docs/NOTIFY.md "System questions").
    questions: Vec<(notes::Question, Option<u8>)>,
    /// A live note on the screen (its source and number) and its words; the note a tap opens its kist at (`notes_front`), and one waiting for its kist's window to show.
    live_note: Option<([u8; 32], u8)>,
    live_words: Option<alloc::string::String>,
    opening: Option<u8>,
    open_pending: Option<([u8; 32], u8)>,
}

/// A fix as the GPS driver gives it (payloads/gps op 0 FIX): latitude and longitude in 2^-23 degrees. Its error estimate is the page's accuracy measure already (the driver's slot 1).
#[derive(Clone, Copy)]
struct GpsFix {
    lat: i32,
    lon: i32,
}

/// The radix of the device's base as the coordinates write it (`degrees_linear`, `degrees_dms`): ten, sixteen, or twelve for both dozenal scales (a coordinate is a place, so DMS writes it linear too, as DLS does).
fn coord_radix(f: tatau::Fmt) -> u64 {
    match f.base {
        tatau::Base::Arabic => 10,
        tatau::Base::Hex => 1 << 4,
        tatau::Base::Dms | tatau::Base::Dls => 12,
    }
}

/// `v` in the device's base at `width` digits or more: arabic and hex as Rust writes them, dozenal digit by digit in the style's glyphs or words (tatau's: words a space apart, none across the radix point, which the caller writes).
fn coord_digits(l: &mut Line, f: tatau::Fmt, v: u64, width: usize) {
    match f.base {
        tatau::Base::Arabic => {
            let _ = write!(l, "{v:0width$}");
        }
        tatau::Base::Hex => {
            let _ = write!(l, "{v:0width$X}");
        }
        tatau::Base::Dms | tatau::Base::Dls => {
            // Least significant first. Twenty-four places hold any u64 in base twelve (eighteen digits) and every width asked here (two at most).
            let mut d = [0u8; 24];
            let (mut n, mut x) = (0, v);
            loop {
                d[n] = (x % 12) as u8;
                n += 1;
                x /= 12;
                if x == 0 && n >= width {
                    break;
                }
            }
            for i in (0..n).rev() {
                match f.style {
                    tatau::Style::Glyphs => {
                        let _ = l.write_char(char::from_u32(tatau::GLYPH_ZERO + d[i] as u32).unwrap_or('?'));
                    }
                    tatau::Style::Words => {
                        if i + 1 != n {
                            l.s(" ");
                        }
                        l.s(tatau::NAMES[d[i] as usize]);
                    }
                }
            }
        }
    }
}

/// The radix point in the device's base: a full stop in arabic and hex, tatau's `·` in dozenal.
fn coord_point(l: &mut Line, f: tatau::Fmt) {
    let _ = l.write_char(if f.base.dozenal() { tatau::POINT } else { '.' });
}

/// One of a fix's coordinates in degrees and a fraction of one (owner, 2026-11-10: "the dymaxion words, lat/lon decimal and dms all for display", and of the digits: "follow the base, yes"), in the device's base: arabic six places, hex five (16^5 near 10^6), dozenal six (12^6 near 3·10^6), each about a tenth of a metre or finer (the fix is in 2^-23 degrees, about a centimetre). The sign a minus in arabic and hex, tatau's arrows in dozenal (always shown, as DLS shows them).
fn degrees_linear(l: &mut Line, f: tatau::Fmt, v: i32) {
    let a = v.unsigned_abs() as u64;
    let radix = coord_radix(f);
    let places = if radix == 1 << 4 { 5 } else { 6 };
    let scale = radix.pow(places as u32);
    // The fraction's last digit, rounded at the half: 2^-23 degrees times the scale, plus half of 2^23, over 2^23. A coordinate is at most 180 degrees (below 2^31 in 2^-23 degrees) and the scale below 2^22, so the product stays below 2^53.
    let r = (a * scale + (1 << 22)) >> 23;
    if f.base.dozenal() {
        let _ = l.write_char(if v < 0 { tatau::DOWN } else { tatau::UP });
    } else if v < 0 {
        l.s("-");
    }
    coord_digits(l, f, r / scale, 1);
    coord_point(l, f);
    coord_digits(l, f, r % scale, places);
}

/// One of a fix's coordinates in degrees, minutes and seconds with its hemisphere (N S for a latitude, E W for a longitude), each number in the device's base (the sixty and sixty stay: they are the unit), seconds to one digit after the point (a tenth, a sixteenth or a twelfth of a second of arc: three metres or less).
fn degrees_dms(l: &mut Line, f: tatau::Fmt, v: i32, pos: char, neg: char) {
    let a = v.unsigned_abs() as u64;
    let radix = coord_radix(f);
    // Seconds of arc in units of one digit after the point, rounded: 2^-23 degrees times 3600 times the radix, plus half of 2^23, over 2^23 (below 2^31 · 2^16, no overflow).
    let t = (a * 3600 * radix + (1 << 22)) >> 23;
    coord_digits(l, f, t / (3600 * radix), 1);
    l.s("\u{b0} ");
    coord_digits(l, f, t / (60 * radix) % 60, 2);
    l.s("\u{2032} ");
    coord_digits(l, f, t / radix % 60, 2);
    coord_point(l, f);
    coord_digits(l, f, t % radix, 1);
    let _ = write!(l, "\u{2033} {}", if v < 0 { neg } else { pos });
}

impl Map {
    /// The kept tags read again from the kernel (SYS_UNLOCK op 9, under Settings): as the NFC page comes, and after an add or a remove. None read (refused) lists none.
    fn tags_read(&mut self) {
        self.ntags = ferros_sys::kept_tags(cap(self.grants, S_SETTINGS), &mut self.tags).map_or(0, |(_, took)| took);
    }

    /// An add that is waiting ended (cancelled, or its page gone): the NFC driver lets the held call go (op 10; a call to it from this thread is answered on the driver's next pass, a millisecond apart).
    fn tag_stop(&mut self) {
        TAG_ADD.store(0, core::sync::atomic::Ordering::Release);
        ferros_sys::wake(&TAG_ADD, 1);
        ferros_sys::nfc::tag_let_go(cap(self.grants, S_NFC));
    }

    /// Who is using right `right` now (ferros_sys::claim::users; owner, 2026-11-10: "we need to be able to see what kists are using each permission"): each claim's kist capsule, None for a system process (this page's own). At most sixteen: more than the page lists. Read at each draw of the pages that show it, one call; the kernel keeps nothing for the map to ask about later.
    fn users(&self, right: u64) -> Vec<Option<[u8; 32]>> {
        use ferros_sys::claim::REC;
        let mut buf = [0u8; REC << 4];
        let n = ferros_sys::claim::users(cap(self.grants, S_SETTINGS), right, &mut buf).unwrap_or(0);
        // WHY the min: the count is every claim, the buffer holds sixteen records.
        buf.chunks_exact(REC).take(n.min(1 << 4)).map(|r| (r[0] == 1).then(|| r[1..REC].try_into().unwrap())).collect()
    }

    /// A claimant as a person reads it: a kist by its name in their list, a system process as "settings" (the only one that claims today), a kist the list no longer names (a capsule updated while it ran) by its address's first bytes.
    fn user_name(&self, u: &Option<[u8; 32]>, l: &mut Line) {
        match u {
            None => {
                l.s(tr(Msg::Settings));
            }
            Some(c) => match self.kists.kists.iter().find(|k| &k.capsule == c) {
                Some(k) => {
                    l.s(&k.name);
                }
                None => {
                    let _ = write!(l, "G#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3]);
                }
            },
        }
    }

    /// A kist's want as its switch's label on the permissions and GPS pages: its name, "not asked yet" while unanswered, "using now" while it holds a claim on the right (`users`).
    fn want_label(&self, k: &ferros_kist::Kist, j: usize, using: &[Option<[u8; 32]>]) -> Line {
        if k.wants[j].answer.is_none() {
            say(Say::NotAskedYet(&k.name))
        } else if using.iter().any(|u| u.as_ref() == Some(&k.capsule)) {
            say(Say::UsingNow(&k.name))
        } else {
            let mut l = Line::new();
            l.s(&k.name);
            l
        }
    }

}

/// Whether the GPS page holds its claim, as the main loop last saw it (bit 0), counted in the bits above so each change is a new word (as `FLEET_PAGE`): `gps_watch` follows it.
static GPS_PAGE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// What `gps_watch` last read from the driver (`Map::gps_fix`'s meaning), under a sequence count (odd while it is written; it moves only when the driver's doorbell rang).
static GPS_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static mut GPS_SNAP: Option<Option<GpsFix>> = None;
/// What the chip has sent, as `gps_watch` last read it with the fix (payloads/gps FIX bytes 33-37): its frames << 8 | their kinds; 0 before a driver answered, or from one that does not say. Written before the sequence moves, read as the page draws.
static GPS_HEARD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// GPS's claims as the kernel writes them here at every change (SYS_CLAIM op 4, ferros_sys::claim::doorbell; fact 85's word): the permissions and GPS pages draw "using now" again when it moves, nothing asked on a timer.
static CLAIMS_WORD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The kists' bell as the kernel writes it here (SYS_NOT_STARTED op 1, ferros_sys::not_started::doorbell): its count moves when a kist launch init answered "started" did not start after all, or a kist that had not starts; the main loop then looks for the launch it asked (`kists_rang`), nothing asked on a timer.
static KISTS_WORD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The kernel's bells (ferros_sys::bell, SYS_BELL): each link's change count in LINKS' order (its measures, tracks and detail), then the facts' (`ferros_sys::BELL_FACTS`: the battery, the processes, the vault's keys, the chip's heat), written here by the kernel at every change. The main loop draws the page that shows one again when it moves, nothing asked on a timer (owner, 2026-11-10: "1s poll can desync too").
static BELLS: [core::sync::atomic::AtomicU64; ferros_sys::BELL_FACTS as usize + 1] = [const { core::sync::atomic::AtomicU64::new(0) }; ferros_sys::BELL_FACTS as usize + 1];

/// `gps_watch`'s copy published under `GPS_SEQ`.
fn gps_publish(v: Option<Option<GpsFix>>) {
    use core::sync::atomic::Ordering;
    GPS_SEQ.fetch_add(1, Ordering::AcqRel);
    unsafe { *(&raw mut GPS_SNAP) = v };
    GPS_SEQ.fetch_add(1, Ordering::AcqRel);
}

/// The GPS page's doorbell on a thread of its own (owner, 2026-11-10: "Needs a doorbell on the driver's fix counter, for sure. 1s poll can desync too"): while the page holds its claim, the driver's WATCH (thru the kernel, ferros_sys::claim::gps_watch) is held until its fixes or fix messages move, then its fix is read (FIX) and published. A driver still replaying, or not started yet, takes the watch when it first receives, so "starting" turns into the fix with nothing asked again. Refused, or the driver gone while it held the watch (it is not started again while the claim stands), the page shows "starting" and the thread sleeps until the page changes. Until then the main loop asked the fix once a second.
extern "C" fn gps_watch(grants: u64) -> ! {
    use core::sync::atomic::Ordering;
    let settings = cap(grants, S_SETTINGS);
    // The driver's change count last seen; u32::MAX at each new claim, so the first watch is answered as soon as a driver receives it.
    let mut count = u32::MAX;
    loop {
        let v = GPS_PAGE.load(Ordering::Acquire);
        if v & 1 == 0 {
            count = u32::MAX;
            ferros_sys::wait_world(&GPS_PAGE, v, 0);
            continue;
        }
        match ferros_sys::claim::gps_watch(settings, count) {
            Some(c) => {
                count = c;
                // The fix, then the chip's frames and their kinds (38 bytes; a driver older than 2026-11-11 sends the 33 of the fix alone).
                let mut r = [0u8; 38];
                let n = ferros_sys::claim::gps_ask(settings, 0, &mut r);
                let at = |k: usize| i32::from_le_bytes([r[k], r[k + 1], r[k + 2], r[k + 3]]);
                GPS_HEARD.store(if n != ferros_sys::ERR && n >= 38 { (at(33) as u32 as u64) << 8 | r[37] as u64 } else { 0 }, Ordering::Release);
                gps_publish(if n == ferros_sys::ERR || n < 33 {
                    None
                } else if r[0] == 0 {
                    Some(None)
                } else {
                    Some(Some(GpsFix { lat: at(9), lon: at(13) }))
                });
            }
            None => {
                gps_publish(None);
                count = u32::MAX;
                ferros_sys::wait_world(&GPS_PAGE, v, 0);
            }
        }
    }
}

/// The copy `gps_watch` last published and its sequence count, read whole (again if it was being written); None when it has not moved from `seen`.
fn gps_take(seen: u32) -> Option<(u32, Option<Option<GpsFix>>)> {
    use core::sync::atomic::Ordering;
    loop {
        let s0 = GPS_SEQ.load(Ordering::Acquire);
        if s0 == seen {
            return None;
        }
        let v = unsafe { *(&raw const GPS_SNAP) };
        if s0 & 1 == 0 && GPS_SEQ.load(Ordering::Acquire) == s0 {
            return Some((s0, v));
        }
    }
}

/// The clock page's zone picker (owner, 2026-11-10: "auto/manual zone, picker, all that"): every zone's id as the kernel lists them (SYS_ZONE op 6, the zones document's order), browsed by region (the IANA id's first part: America, Europe, …) then by place. No typing: the map has no keyboard of its own (the attest screen hosts the one there is), so a list it is.
struct ZonePick {
    /// The list as the kernel gave it: each id its length, a byte, then its bytes.
    raw: Vec<u8>,
    /// Each id's place in `raw`: (start, length), in the list's order.
    ids: Vec<(usize, usize)>,
    /// The regions, in alphabetical order, each once.
    regions: Vec<(usize, usize)>,
    /// The region open, an index into `regions`; None: the regions are shown.
    region: Option<usize>,
}

impl ZonePick {
    /// The picker from the kernel's list; None when there is none (the zones document not on this device, or not read yet).
    fn load() -> Option<ZonePick> {
        let mut raw = alloc::vec![0u8; 1 << 13];
        let mut n = ferros_sys::zone::list(&mut raw);
        if n > raw.len() {
            raw = alloc::vec![0u8; n];
            n = ferros_sys::zone::list(&mut raw);
        }
        // WHY the second bound: the kernel answers the list's whole length, which can pass the buffer; read whole or not at all (a list cut short would end inside an id).
        if n == 0 || n > raw.len() {
            return None;
        }
        raw.truncate(n);
        let mut ids = Vec::new();
        let mut at = 0;
        while at < raw.len() {
            let len = raw[at] as usize;
            ids.push((at + 1, len));
            at += 1 + len;
        }
        let mut p = ZonePick { raw, ids, regions: Vec::new(), region: None };
        let mut regions: Vec<(usize, usize)> = Vec::new();
        for k in 0..p.ids.len() {
            let r = p.region_of(k);
            if !regions.iter().any(|&(s, l)| &p.raw[s..s + l] == r.as_bytes()) {
                let (s, _) = p.ids[k];
                regions.push((s, r.len()));
            }
        }
        regions.sort_by(|a, b| p.raw[a.0..a.0 + a.1].cmp(&p.raw[b.0..b.0 + b.1]));
        p.regions = regions;
        Some(p)
    }

    fn id(&self, k: usize) -> &str {
        let (s, l) = self.ids[k];
        core::str::from_utf8(&self.raw[s..s + l]).unwrap_or("")
    }

    /// An id's region: its part before the first `/` (the whole id when it has none).
    fn region_of(&self, k: usize) -> &str {
        let id = self.id(k);
        id.split('/').next().unwrap_or(id)
    }

    fn region(&self, r: usize) -> &str {
        let (s, l) = self.regions[r];
        core::str::from_utf8(&self.raw[s..s + l]).unwrap_or("")
    }

    /// The zones of region `r`, in alphabetical order of their places.
    fn places(&self, r: usize) -> Vec<usize> {
        let name = self.region(r);
        let mut v: Vec<usize> = (0..self.ids.len()).filter(|&k| self.region_of(k) == name).collect();
        v.sort_by(|&a, &b| self.id(a).cmp(self.id(b)));
        v
    }
}

/// A zone's id as a person reads it: the place without its region, its parts joined by commas and its underscores spaces ("America/Argentina/Buenos_Aires" → "Argentina, Buenos Aires"). The page's lines are short: written into a Line.
fn zone_words(l: &mut Line, id: &str, with_region: bool) {
    let mut parts = id.split('/');
    if !with_region && id.contains('/') {
        parts.next();
    }
    for (k, part) in parts.enumerate() {
        if k > 0 {
            l.s(", ");
        }
        for (j, piece) in part.split('_').enumerate() {
            if j > 0 {
                l.s(" ");
            }
            l.s(piece);
        }
    }
}

impl Map {
    /// Circle `i` as the scene draws it: settings first, then each stowed kist, its name its label and its icon the display's bundled one for that app (tools/app-icons order), or none.
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

    /// The person's list read again (another device of theirs may have answered, or a stow added a kist), less what this device leaves off its deck (an unstow "this device only", `ferros_kist::Hidden`); false when it could not be read (nobody present), the copy kept as it was.
    fn reload_kists(&mut self) -> bool {
        match ferros_kist::store::read_here(cap(self.grants, S_KIST_LIST)) {
            Some(k) => {
                self.kists = k;
                true
            }
            None => false,
        }
    }

    /// The person's list read again, and the kist circles made to match it when a kist came or went (a stow over the bridge, one on another of their devices): placed afresh by the seeded fit, each kept place winning, then the scene sent again.
    fn refresh_circles(&mut self) {
        let was = self.placed();
        if !self.reload_kists() {
            return;
        }
        // WHY: the arrays hold MAX circles (as at start).
        let n = (1 + self.kists.kists.len()).min(MAX);
        if n == self.n {
            // As many circles, but a circle may hold another kist now (one unstowed, another stowed): its state follows, drawn with the next frame as a new scene is.
            let _ = self.send_states(false);
            return;
        }
        self.place_circles(n, &was);
    }

    /// The person's kists and arrangement read again after the identity service wrote them from their fleet (payloads/fgtw space.rs: a reset device getting its deck back, a move made on another of their devices): every circle placed afresh, each kept place winning, the view as kept, then the scene sent again.
    fn reload_arrangement(&mut self) {
        let was = self.placed();
        if !self.reload_kists() {
            return;
        }
        // WHY: the arrays hold MAX circles (as at start).
        let n = (1 + self.kists.kists.len()).min(MAX);
        self.place_circles(n, &was);
        if self.load_view() {
            self.send_scene();
        }
    }

    /// Each circle on the deck now, by its label, and its place (x, y, radius, turn): taken before the person's list is read again, which may change which kist each index is.
    fn placed(&self) -> Vec<(alloc::string::String, [i64; 5])> {
        (0..self.n).map(|i| (alloc::string::String::from(self.label(i)), [self.xs[i], self.ys[i], self.rs[i], self.rc[i], self.rsn[i]])).collect()
    }

    /// `n` circles placed by the seeded fit, a circle already on the deck (`was`, by its label) where it stands, each kept place winning (`load_places_near`), pushed apart, and the scene sent. Until 2026-11-12 every circle went back to its seed first, so one the vault kept no place for jumped (owner, on the 8a: "sometimes that ferros logo just BOOP!": the settings circle, its place taken out of the vault by the fleet's sync, sent off the deck at its last size whenever the fleet wrote).
    fn place_circles(&mut self, n: usize, was: &[(alloc::string::String, [i64; 5])]) {
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
        // The vault's places win; a circle it keeps none for that is already on the deck stays where it stands; a new one (a kist just stowed) goes with the settings circle, about which the seeds are laid.
        let mut held = self.load_places();
        for i in 0..n {
            if held >> i & 1 == 0 {
                if let Some((_, p)) = was.iter().find(|(l, _)| l.as_str() == self.label(i)) {
                    [self.xs[i], self.ys[i], self.rs[i], self.rc[i], self.rsn[i]] = *p;
                    held |= 1 << i;
                }
            }
        }
        self.with_settings(held);
        self.push_apart(MAX);
        self.send_scene();
    }

    /// The places kept by hand (`load_places`), then every circle still at its seeded place moved with the settings circle: the seeded fit is about the settings circle at the origin, and a kist circle stowed after the person moved settings would otherwise land far from it, off the view they keep.
    fn load_places_near(&mut self) -> usize {
        let kept = self.load_places();
        self.with_settings(kept);
        kept.count_ones() as usize
    }

    /// Every circle not in `held` (a bit a circle) still at its seed, which is laid about the settings circle at the origin: moved by where settings stands now.
    fn with_settings(&mut self, held: u32) {
        let (dx, dy) = (self.xs[0], self.ys[0]);
        for i in 1..self.n {
            if held >> i & 1 == 0 {
                self.xs[i] += dx;
                self.ys[i] += dy;
            }
        }
    }

    /// The person's list written whole, after a change of theirs (an answer, a default); the console says when the vault did not take it. The map's copy lacks what this device leaves off its deck, so the change goes into the list as the vault holds it: the defaults, and each answer of every kist the map shows; a kist left off here keeps its own.
    fn save_kists(&self) -> bool {
        let c = cap(self.grants, S_KIST_LIST);
        let ok = match ferros_kist::store::read(c) {
            Some(mut whole) => {
                whole.defaults = self.kists.defaults;
                for k in &self.kists.kists {
                    if let Some(w) = whole.kists.iter_mut().find(|w| w.developer == k.developer && w.option == k.option) {
                        w.wants = k.wants.clone();
                    }
                }
                ferros_kist::store::write(c, &whole, ferros_detail::eagle_now())
            }
            None => false,
        };
        if !ok {
            console(self.grants, b"map: the vault did not take the person's kists\n");
        }
        ok
    }

    /// Ask init to launch kist circle `i`'s kist (KISTS.md): the map holds none of its rights. Init's answer (ferros_kist::launch). A widget's kist is not launched: the display draws it, in its circle, from the scene (`widget`). Answered "started", the kist is remembered with the kists' bell as it stands (`asked`): the kernel may yet not start it, and then its circle says why.
    fn launch(&mut self, i: usize) -> u8 {
        let Some(k) = self.kist(i) else { return ferros_kist::launch::NOT_STOWED };
        if widget(&k.name).is_some() {
            return ferros_kist::launch::RUNNING;
        }
        let mut m = [0u8; 33];
        m[0] = ferros_kist::launch::OP_LAUNCH;
        m[1..].copy_from_slice(&k.capsule);
        let mut r = [ferros_kist::launch::REFUSED, 0, 0, 0];
        // The bell as it stands before the ask: the kernel's job may fail before init's answer is back here.
        let bell = KISTS_WORD.load(core::sync::atomic::Ordering::Acquire);
        let n = call(cap(self.grants, S_LAUNCH), &m, &mut r);
        // Running on a surface (init's RUNNING with the surface and its generation): its window to the front instead of a second copy (owner, 2026-11-11: a running kist's circle tapped "brings it to the front").
        let raised = r[0] == ferros_kist::launch::RUNNING && n == 3 && ferros_sys::window::front(self.display, r[1], r[2]);
        let mut l = Line::new();
        let _ = write!(l, "map: launch {}: {}\n", k.name, match r[0] { 0 => "starting", 1 if raised => "running, brought to the front", 1 => "running", 2 => "asking first", 3 => "not stowed", 5 => "the list is not readable", _ => "refused" });
        console(self.grants, l.as_str().as_bytes());
        let capsule = k.capsule;
        if r[0] == ferros_kist::launch::STARTED {
            self.asked = Some((capsule, bell));
            self.fault = None;
        }
        // Started or running, it is the kist in front (or about to be, once its window shows): the head of the recent kists.
        if r[0] == ferros_kist::launch::STARTED || r[0] == ferros_kist::launch::RUNNING {
            self.recent.retain(|c| *c != capsule);
            self.recent.insert(0, capsule);
            self.front_kist = Some(capsule);
        }
        if raised {
            self.app_front = true;
        }
        // In front, the person is looking at it: its notes cleared, and told which one a tap opened it at (notes.rs `notes_front`; owner, 2026-11-12: "when tapping on Photon it clears it and resumes regular 500nm").
        if r[0] == ferros_kist::launch::STARTED || raised {
            self.notes_front(capsule, raised);
        } else {
            self.opening = None;
        }
        r[0]
    }

    /// The kist circle holding `capsule`, if it is still stowed.
    fn circle_of(&self, capsule: &[u8; 32]) -> Option<usize> {
        (1..self.n).find(|&i| self.kist(i).is_some_and(|k| k.capsule == *capsule))
    }

    /// Deck (WINDOWS.md "The helm"; owner, 2026-11-11: "Home backgrounds the kist so depending on how it was designed, like Photon, it doesn't die but it stops rendering everything and stops playing anything except for notifications"): the app in front behind (display op 60, the app told so); the map is on the glass as it was left once it draws (`draw`, or the caller's own frame). Whether an app went behind.
    fn send_behind(&mut self, draw: bool) -> bool {
        let Some((k, g)) = ferros_sys::window::behind(self.display) else { return false };
        let mut l = Line::new();
        let _ = write!(l, "map: deck: the window on surface {k} (generation {g}) behind\n");
        console(self.grants, l.as_str().as_bytes());
        self.app_front = false;
        self.front_kist = None;
        if draw {
            self.draw();
        }
        true
    }

    /// A console circle's page by its zone's id (`dash_page`), dragged toward the centre or tapped still (but a radio's, whose tap switches it), whatever is in front (owner, 2026-11-12: "The console slide to center should work with anything open..."): an app in front goes behind first, as deck's tap sends it, and the settings circle opens on the page over the map as it was left.
    /// Until then a kist in front took the gesture to nothing (the pages live on the plane, and the map stood aside), altho the touch was the console's from its landing (op 41 gave its zone, op 27 told the app the point was not its own).
    fn dash_open(&mut self, id: u8) {
        if self.app_front && !self.send_behind(false) {
            // None went behind: the app ended since the map last looked, and the map has the glass (as back's tap finds it).
            self.app_front = false;
            self.front_kist = None;
        }
        self.show_page(dash_page(id));
    }

    /// Back's swipe (owner, 2026-11-11: "flip to the previous kist (this one to the background)"): the kist before the one in front, or with the map on the glass the last kist in front, launched as a tap on its circle would (init's RUNNING brings its window to the front; one that ended since starts again). The one in front goes behind first. A kist unstowed since is passed over.
    fn flip(&mut self) -> &'static str {
        let skip = if self.app_front { self.front_kist } else { None };
        loop {
            let Some(c) = self.recent.iter().copied().find(|c| Some(*c) != skip) else { return "map: back: no other kist to flip to" };
            let Some(i) = self.circle_of(&c) else {
                self.recent.retain(|x| *x != c);
                continue;
            };
            // The one in front behind first, the map not drawn: the next window is raised in the same breath, so the glass goes from one kist to the other. Not raised (starting again, or refused), the map draws until the kist's window shows.
            if self.app_front {
                self.send_behind(false);
            }
            let said = match self.launch(i) {
                ferros_kist::launch::RUNNING => "map: back: flipped to the kist before",
                ferros_kist::launch::STARTED => "map: back: the kist before starting again",
                _ => "map: back: the kist before could not come to the front",
            };
            if !self.app_front {
                self.draw();
            }
            return said;
        }
    }

    /// The kists' bell rang (KISTS_WORD): the records the kernel keeps (ferros_sys::not_started) read, each one newer than the last taken (`kists_read`) taken once. The launch asked last (`asked`), rung after the ask, whether it did not start or started and then stopped: kept for its circle (`fault`) and shown once the plane is at rest (`show_fault`), the person having just tapped it or being in it. Any other kist that stopped abnormally (a fault, a panic, a kill from outside): said as its circle is next tapped, once (`stopped`). A start that failed for a launch this map did not ask (a relaunch) says nothing here. A stop the kernel no longer keeps (that kist started again since) is dropped unsaid. Then the kists alive read, and the circles' states sent where they changed (`state`, `send_states`).
    fn kists_rang(&mut self) {
        // WHY the size and the `take`: one record a kist the person has, and the circles hold at most MAX of them; a list longer than the buffer is cut to whole records by the kernel, and a shorter one leaves the rest of the buffer unwritten, so only the first n records are read.
        let mut buf = [0u8; ferros_sys::not_started::REC * MAX];
        let Some(n) = ferros_sys::not_started::list(cap(self.grants, S_SETTINGS), &mut buf) else { return };
        let records = || buf.chunks_exact(ferros_sys::not_started::REC).take(n);
        // Every kist the kernel keeps a record for is stopped on its circle (`state`), whatever the reason, until it starts again.
        self.ended = records().map(|r| (r[1..33].try_into().unwrap(), r[0])).collect();
        self.stopped.retain(|s| records().any(|r| r[1..33] == s.0 && ferros_kist::launch::why::stopped(r[0])));
        let read = self.kists_read;
        for r in records() {
            let at = u64::from_le_bytes(r[33..41].try_into().unwrap());
            if at <= read {
                continue;
            }
            // The newest record taken, so the next ring takes only what came after it.
            if at > self.kists_read {
                self.kists_read = at;
            }
            let capsule: [u8; 32] = r[1..33].try_into().unwrap();
            let why = r[0];
            let mut l = Line::new();
            if self.asked.is_some_and(|(c, bell)| c == capsule && at > bell) {
                self.asked = None;
                self.fault = Some((capsule, why, false));
                let _ = write!(l, "map: the kist asked last {} (reason {why})\n", if ferros_kist::launch::why::stopped(why) { "stopped" } else { "did not start" });
            } else if ferros_kist::launch::why::stopped(why) {
                self.stopped.retain(|s| s.0 != capsule);
                self.stopped.push((capsule, why));
                let _ = write!(l, "map: a kist stopped (reason {why}), said at its circle's next tap\n");
            } else {
                continue;
            }
            console(self.grants, l.as_str().as_bytes());
        }
        // The kists alive (op 2), and every circle whose state that changed sent again (`state`): the bell rings at each kist's start and end, so the glows follow them with nothing asked on a timer. A kernel without op 2 leaves them with no state.
        // WHY the size: one capsule a kist circle, at most MAX of them; a longer list is cut to whole addresses by the kernel, and only the first `k` are read.
        let mut live = [0u8; 32 * MAX];
        if let Some(k) = ferros_sys::not_started::live(cap(self.grants, S_SETTINGS), &mut live) {
            self.live = Some(live.chunks_exact(32).take(k).map(|c| c.try_into().unwrap()).collect());
        }
        if self.send_states(false) && !self.app_front {
            self.draw();
        }
    }

    /// A kist that did not start, its circle opened on why, as a tap on it opens it (`fault_page`): once the plane is at rest, nothing is held and no app is in front, so it never pulls the view from under a finger or a flight. Its circle gone meanwhile (the kist unstowed), nothing is shown.
    fn show_fault(&mut self) {
        let Some((capsule, _, false)) = self.fault else { return };
        if self.view != View::Map || self.settling || self.touching || self.app_front {
            return;
        }
        let Some(i) = (1..self.n).find(|&i| self.kist(i).is_some_and(|k| k.capsule == capsule)) else {
            self.fault = None;
            return;
        };
        if let Some(f) = self.fault.as_mut() {
            f.2 = true;
        }
        let to = self.at(i);
        self.back = Some(self.cam);
        self.view = View::Open(i);
        self.leaving = false;
        self.fly(to);
        self.draw();
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

    /// The arrangement made by hand, from the vault: each circle's place as EWE integers (CLAUDE.md: no fixed widths on storage) — x and y in world units ×256, zigzag-mapped (they can be negative), the radius, and then (since the circles turn) its turn as cos and sin in Q14, zigzag; a place kept before turns existed has three and stays upright. Which circles it placed, a bit each (MAX is 16).
    fn load_places(&mut self) -> u32 {
        let mut loaded = 0u32;
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
                loaded |= 1 << i;
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

    /// The whole scene to the display server: clear, then every circle in one call, then every circle's state (a fresh scene has none: `send_states`).
    fn send_scene(&mut self) {
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
            // The settings circle's name in the person's language (its `label` stays the English key its place is kept under); a kist's is its own. The op's field holds 16 bytes, cut at a character's edge (`fit`).
            let label = fit(if i == 0 { tr(Msg::Settings) } else { a.label }, 16).as_bytes();
            let len = if labels || a.icon == 0 { label.len() } else { 0 };
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
        let _ = self.send_states(true);
    }

    /// Circle `i`'s state as its glow (ferros_glow, display op 57; owner, 2026-11-11, on the Pro: "glow is good! you can nix the test circles and use those colours for realzies"), with the count of things waiting on the person. The settings circle is the system's own, 703 nm (owner: "For the ferros settings specifically, I think 703nm"). A kist circle, from what the kernel last said (`kists_rang`), first that matches:
    /// - for you (400 nm): something waits on the person for it (`waiting`; nothing adds to it yet, there are no notifications).
    /// - running (500 nm): a live process of it, in front or behind (deck leaves it alive behind). A widget the display draws itself (the clock) is running too: it is never a process, and it is live on the glass.
    /// - stopped (600 nm): the kernel keeps a record for it (ferros_sys::not_started): its last process ended by a fault, a panic or a kill from outside, or its last launch did not start; gone once it starts again.
    /// - not running (a ring of darkness): stowed and none of those (not launched since boot, or ended by its own exit or the person's relaunch).
    /// Before the kernel has said (a kernel without op 2), a kist circle has no state: the theme's glow as before.
    fn state(&self, i: usize) -> (ferros_glow::Glow, u16) {
        use ferros_glow::Glow;
        let Some(k) = self.kist(i) else { return (Glow::System, 0) };
        if let Some(w) = self.waiting.iter().find(|w| w.0 == k.capsule).filter(|w| w.1 > 0) {
            return (Glow::ForYou, w.1);
        }
        if widget(&k.name).is_some() {
            return (Glow::Running, 0);
        }
        let Some(live) = self.live.as_ref() else { return (Glow::None, 0) };
        if live.contains(&k.capsule) {
            (Glow::Running, 0)
        } else if self.ended.iter().any(|e| e.0 == k.capsule) {
            (Glow::Stopped, 0)
        } else {
            (Glow::NotRunning, 0)
        }
    }

    /// A kist's state in words, for its row and its card on the Deck, as its glow says it (`state`): running, stopped (with the kernel's reason), or not running; None before the kernel has said which kists are alive (a kernel without op 2). Not for a puck: never a process, it has no state to say.
    fn kist_state(&self, capsule: &[u8; 32]) -> Option<(Msg, Option<u8>)> {
        let live = self.live.as_ref()?;
        Some(if live.contains(capsule) {
            (Msg::Running, None)
        } else if let Some(e) = self.ended.iter().find(|e| &e.0 == capsule) {
            (Msg::Stopped, Some(e.1))
        } else {
            (Msg::NotRunning, None)
        })
    }

    /// The circles' states to the display (op 57): every circle's after a fresh scene (`all`), else only those changed since last sent. Whether any was sent.
    fn send_states(&mut self, all: bool) -> bool {
        let mut m = [0u8; 2 + 4 * MAX];
        m[0] = 57;
        let mut count = 0;
        for i in 0..self.n {
            let (g, n) = self.state(i);
            let s = (g as u8, n);
            if !all && self.sent[i] == s {
                continue;
            }
            self.sent[i] = s;
            let at = 2 + count * 4;
            m[at] = i as u8;
            m[at + 1] = s.0;
            m[at + 2..at + 4].copy_from_slice(&n.to_le_bytes());
            count += 1;
        }
        if count == 0 {
            return false;
        }
        m[1] = count as u8;
        let mut r = [0u8; 4];
        call(self.display, &m[..2 + 4 * count], &mut r);
        true
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
    fn deck_fit(&self) -> Cam {
        self.deck_fit_at(self.cam.c, self.cam.s).0
    }

    /// The deck swipe's camera (owner, 2026-11-11: deck's swipe flies out "to fit every circle" and "let's rotate it to find the most zoom we can too so it spins into place"): every circle in view as `deck_fit` fits them, at the turn that lets them be largest. The fit's zoom is a function of the turn with period a half turn (turning by π mirrors the circles through the fit's middle, which fits alike), continuous and piecewise smooth (the fit is the least of each circle's two room ratios). A bounded search, no timer: `TURN_SAMPLES` turns across the half turn centred on the camera's own (so the answer is the nearer of each pair a half turn apart, and the flight spins the short way), then golden-section steps inside the best sample's neighbours. Where the zoom limit caps every turn alike (a few small circles), the camera keeps its own turn: nothing to gain by spinning; and where the best turn gains little, it turns little (owner, 2026-11-12), the part of the way smooth in the gain (`TURN_GAIN`). Also the gain (the natural logarithm of the zoom gained) and the turn taken, in radians, for the console.
    fn deck_turned(&self) -> (Cam, f64, f64) {
        let a0 = libm::atan2(self.cam.s as f64, self.cam.c as f64);
        let half = core::f64::consts::PI;
        let turn = |a: f64| (libm::round(libm::cos(a) * ONE as f64) as i64, libm::round(libm::sin(a) * ONE as f64) as i64);
        let fit = |a: f64| { let (c, s) = turn(a); self.deck_fit_at(c, s).1 };
        let step = half / TURN_SAMPLES as f64;
        let (mut best, mut best_z) = (a0, fit(a0));
        for j in 0..TURN_SAMPLES {
            let a = a0 - half * 0.5 + step * (j as f64 + 0.5);
            let z = fit(a);
            if z > best_z {
                (best, best_z) = (a, z);
            }
        }
        // Golden-section search on [best − step, best + step]: the fit is unimodal at that scale (each sample is a 64th of a half turn, under three degrees).
        let g = (libm::sqrt(5.) - 1.) * 0.5;
        let (mut lo, mut hi) = (best - step, best + step);
        let (mut p, mut q) = (hi - g * (hi - lo), lo + g * (hi - lo));
        let (mut zp, mut zq) = (fit(p), fit(q));
        for _ in 0..TURN_REFINE {
            if zp < zq {
                lo = p;
                p = q;
                zp = zq;
                q = lo + g * (hi - lo);
                zq = fit(q);
            } else {
                hi = q;
                q = p;
                zq = zp;
                p = hi - g * (hi - lo);
                zp = fit(p);
            }
        }
        let (a, z) = if zp > zq { (p, zp) } else { (q, zq) };
        let (best, best_z) = if z > best_z { (a, z) } else { (best, best_z) };
        // What the turn gains: the zoom the camera may reach at the best turn against at its own, as a natural logarithm (a turn that only gains past the zoom limit gains nothing). The limit as a soft minimum, z / (1 + (z / limit)⁸)^(1/8): the fit's zoom below the limit, the limit above it, C∞ between, so the gain is smooth in the circles' places as the turn it sets is drawn. Never below zero: the camera's own turn is the search's first candidate.
        let (_, hi_z) = self.zoom_limits();
        let hi = hi_z as f64;
        let reach = |z: f64| { let a = z / hi; let a2 = a * a; let a4 = a2 * a2; z / libm::pow(1. + a4 * a4, 1. / ((1 << 3) as f64)) };
        let gain = libm::log(reach(best_z) / reach(fit(a0)));
        // How much of the way the camera turns (owner, 2026-11-12, on the 8a: a 32 degree spin for a fit that gained little; "it changes orientation instantly on occasion"): x⁴ / (1 + x⁴) of the gain against `TURN_GAIN`, C∞, nothing for a gain far under it, half at it, nearly all of the way past twice it. A partial turn still fits every circle (`deck_fit_at` at that turn), only not as large.
        let x = gain / TURN_GAIN;
        let x4 = x * x * x * x;
        let part = x4 / (1. + x4);
        let a = a0 + part * (best - a0);
        let (c, s) = turn(a);
        (self.deck_fit_at(c, s).0, gain, a - a0)
    }

    /// `deck_fit` at the turn (c, s), Q14: the camera, and the fit's own zoom before the zoom limits hold it (what the turn is chosen by).
    fn deck_fit_at(&self, c: i64, s: i64) -> (Cam, f64) {
        let (cc, ss) = (c, s);
        let (c, s, one) = (c as f64, s as f64, ONE as f64);
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
        let mut cam = Cam { x: wx as i64, y: wy as i64, z: (z as i64).clamp(lo, hi), c: cc, s: ss };
        // The camera's centre is the panel's: it stands off the fit's middle by the big circle's offset from the panel's centre, as `at` does.
        let (dx, dy) = cam.delta(0, self.h / 2 - ay);
        cam.x += dx;
        cam.y += dy;
        (cam, z)
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
        if DEV {
            let left = |i: usize| if f.off[i] == 0. { 0 } else { libm::round(p[i].0 / f.off[i] * 1000.) as i64 };
            let _ = core::fmt::Write::write_fmt(&mut self.flight_trace, format_args!(" {}:z{}t{}", f.t, left(2), left(3)));
        }
        if !still && DEV && !self.flight_trace.is_empty() {
            let l = alloc::format!("map: flight landed (zoom G#{:X}, turn {} mrad at the start): frame:zoom left, turn left, per mille;{}\n", t.z, libm::round(f.off[3] * 1000.) as i64, self.flight_trace);
            console(self.grants, l.as_bytes());
            self.flight_trace.clear();
        }
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
                // A circle that said why its kist did not start has been read and closed.
                if self.fault.is_some_and(|f| f.2) {
                    self.fault = None;
                }
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
        // The page itself, not the page picker over it.
        self.picker = false;
        // The settings circle open and staying: the page is enough. Otherwise it opens: from the map (the way back kept), from a kist's circle (how its launch stopped, or its first-launch question; the way back to the map it kept stays), or from its own flight out (`leaving`). Until 2026-11-12 the page was set and a kist's circle stayed open, drawing its own page: Mahere's "it crashed" circle swallowed the first kist offered over USB, which then counted as shown and never opened again.
        if self.view != View::Open(0) || self.leaving {
            if !matches!(self.view, View::Open(_)) {
                self.back = Some(self.cam);
            }
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
            b.turned(libm::round(x) as i64, libm::round(y) as i64, size as u16, white(), alpha as u8, libm::round(c * 16384.) as i16, libm::round(s * 16384.) as i16, page_title(PAGES[p]));
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

    /// The first-launch question (owner, 2026-11-08), inscribed in the kist's open circle: its name, then each asked want not yet answered (never the implicit set, ferros_kist::IMPLICIT: owner, 2026-11-12, "Yes on the implicit set"), its switch as the person has it (`ask`, started from their defaults: out of the box every one off) and the developer's reason under it, then "start". Asked once for this person and this kist, on every device of theirs; a kist itself can never ask.
    fn ask_page(&mut self, b: &mut Batch, i: usize, region: bool) {
        let (cx, cy, r, uq) = self.page_frame();
        let uu = |n: i64, d: i64| n * uq / (d << 8);
        let half = self.w / 2;
        let (title_y, top, bottom) = self.bands();
        let Some(k) = self.kist(i).cloned() else { return };
        // A live note's page first (notes.rs): the takeover is the system's, over any question.
        if self.live_page(b, i, region) {
            return;
        }
        b.text(cx, title_y, uu(2, 1) as u16, white(), 1, &k.name);
        // Opened on why its launch did not start (`show_fault`): that, in the person's language, and nothing to tap; a tap outside closes it, and a tap on the circle again asks again.
        if let Some((_, why, true)) = self.fault.filter(|f| f.0 == k.capsule) {
            let reason = why_words(why);
            let y = (top + bottom) / 2;
            let head = if ferros_kist::launch::why::stopped(why) { say(Say::Stopped(&k.name)) } else { say(Say::CouldNotStart(&k.name)) };
            b.text(cx, y - uu(1, 1), uu(5, 4) as u16, white(), 1, head.as_str());
            b.text(cx, y + uu(1, 1), uu(1, 1) as u16, grey(), 1, tr(reason));
            if !region {
                b.disc(cx, cy, r, fill());
            }
            return;
        }
        let open: alloc::vec::Vec<usize> = (0..k.wants.len()).filter(|&j| k.wants[j].asked() && k.wants[j].answer.is_none()).collect();
        // The rows' height, centred in the band: a line saying what this is, each want two lines, then start.
        let pitch = uu(7, 2);
        let h = uu(3, 1) + open.len() as i64 * pitch + uu(4, 1);
        let mut y = (top + bottom - h) / 2 + uu(2, 1);
        b.text(cx, y, uu(1, 1) as u16, grey(), 1, tr(Msg::AsksForOnce));
        y += uu(3, 1);
        let live = self.target.is_none();
        for &j in &open {
            let w = &k.wants[j];
            let on = self.ask >> j & 1 == 1;
            // Its checkbox (COMPOSITOR.md "Controls"), the label white when on.
            let (sw, colours, tick) = mark_look(uu(5, 4));
            b.mark(50, cx, y - uu(1, 2), uu(5, 4), uu(1, 1), sw, on as u8 | ((on && tick) as u8) << 1, [colours[0], colours[1], if on { white() } else { grey() }, colours[3]], right_name(w.right as usize));
            b.text(cx, y + uu(3, 2), uu(7, 8) as u16, dim(), 1, &w.reason);
            if live {
                self.push_row(cx - half, cx + half, y - uu(2, 1), y + uu(3, 2), Choice::AskWant(j as u8));
            }
            y += pitch;
        }
        y += uu(1, 1);
        // The button that starts it: fluor's, held while a finger is on it.
        let [bf, bh, tl, br] = BUTTON_LOOK[ferros_sys::theme::index()];
        b.button(cx, y - uu(1, 2), uu(16, 1), uu(2, 1), uu(3, 2), [if self.pressed == Some(Choice::AskGo) { bh } else { bf }, tl, br, white()], tr(Msg::Start));
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
            b.text(cx, base, size as u16, if p == self.page { white() } else { grey() }, 1, page_title(PAGES[p]));
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
        // A row of text, centred, wrapped at spaces to the page's width (owner, 2026-11-10: "this device is asking your fleet for its rela" ran off the screen with room below): as many words as fit a line by the typeface's average advance (five eighths of its size, as `fact_wrap` judges it), the lines a row and a half apart, the whole of it one tap target.
        let row = |m: &mut Map, b: &mut Batch, y: &mut i64, px: i64, colour: u32, t: &str, what: Option<Choice>| {
            let fits = ((2 * half - uu(2, 1)) * 8 / (px * 5)) as usize;
            let y0 = *y;
            // A `\n` is a forced line break (the shared fleet text's contract, fgtw text.rs); each line between is wrapped at spaces.
            for (k, para) in t.split('\n').enumerate() {
                if k > 0 {
                    *y += uu(3, 2);
                }
                let mut rest = para;
                loop {
                    let cut = if rest.len() <= fits {
                        rest.len()
                    } else {
                        let mut f = fits;
                        while !rest.is_char_boundary(f) {
                            f -= 1;
                        }
                        // A word longer than the line is cut where the line ends.
                        rest[..f].rfind(' ').map_or(f, |k| k + 1)
                    };
                    // WHY: a line that takes nothing (fewer than one character fits: a page narrower than a glyph, or a first character wider in bytes than the line) would leave `rest` as it was and never end the loop. PROOF: every other case cuts at least one byte (rfind's k + 1, or f past a boundary). A line takes at least its first character.
                    let cut = if cut == 0 { rest.chars().next().map_or(rest.len(), char::len_utf8) } else { cut };
                    b.text(cx, *y, px as u16, colour, 1, rest[..cut].trim_end());
                    rest = rest[cut..].trim_start();
                    if rest.is_empty() {
                        break;
                    }
                    *y += uu(3, 2);
                }
            }
            if let (true, Some(c)) = (hit, what) {
                m.push_row(cx - half, cx + half, y0 - uu(2, 1), *y + uu(1, 1), c);
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
        // A note under the control above it, small and dim: a message's lines ('\n' between them), the first a unit over `y` and each next a unit and a half lower, `y` then past them all (where the two-line notes always left it).
        let notes = |b: &mut Batch, y: &mut i64, t: &str| {
            let mut n = 0;
            for (k, line) in t.split('\n').enumerate() {
                b.text(cx, *y - uu(1, 1) + k as i64 * uu(3, 2), note as u16, dim(), 1, line);
                n = k as i64 + 1;
            }
            *y += n * uu(3, 2);
        };
        // Rows for a message of several lines ('\n' a forced break; each line still wrapped at its spaces by `row`).
        let rows = |m: &mut Map, b: &mut Batch, y: &mut i64, px: i64, colour: u32, t: &str| {
            for line in t.split('\n') {
                row(m, b, y, px, colour, line, None);
            }
        };
        // A typed setting (typed.rs, its index in `typed::ALL`): its name with its value in words (a level's name alone, its slider says it), its slider under it, the value under a finger as it moves.
        let typed_row = |m: &mut Map, b: &mut Batch, y: &mut i64, i: usize| {
            let s = typed::ALL[i];
            let what = Choice::Typed(i as u8);
            let v = match m.slide {
                Some((c, t)) if c == what => s.value(t),
                _ => s.now(),
            };
            let mut words = Line::new();
            s.words(v, &mut words);
            if words.as_str().is_empty() {
                b.text(cx, *y, small as u16, grey(), 1, tr(s.name));
                *y += uu(2, 1);
            } else {
                fact(b, y, tr(s.name), words.as_str());
            }
            slider(m, b, y, what);
        };
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
                    b.text(x, y + uu(3, 1) + uu(1, 2), note as u16, if i == now_t { white() } else { grey() }, 1, theme_name(i));
                    if hit {
                        self.push_row(x - uu(3, 1), x + uu(3, 1), y - uu(2, 1) - uu(1, 2), y + uu(4, 1), Choice::Theme(i as u8));
                    }
                }
                y += uu(7, 1);
                // Brightness: a typed setting (typed.rs), continuous, dimmest on the left, dragged or tapped (owner, 2026-11-08: "sliders (for brightness)", "why's brightness not continuous?"); the brightness ring's tap opens this page, and its drag down sets the same preference (QUICK.md "Quick pages").
                typed_row(self, b, &mut y, typed::BRIGHTNESS);
                // The UI scale, half to twice, as a pinch on the page sets it.
                b.text(cx, y, small as u16, grey(), 1, tr(Msg::Scale));
                y += uu(2, 1);
                slider(self, b, &mut y, Choice::Scale);
                let labels = ferros_sys::pref(ferros_sys::PREF_LABELS) == 1;
                mark(self, b, &mut y, 50, tr(Msg::NamesUnderCircles), labels, Some(Choice::Labels));
                // The temporal dither, from Developer (owner, 2026-11-12: the brightness page is where "brightness, theme, other look related items" go): checked, the four phases flip at rest; unchecked, spatial alone and the panel rests (DISPLAY-POWER.md: the flips cost nothing measurable with the 10 Hz floor).
                let spatial = ferros_sys::pref(ferros_sys::PREF_DITHER) == 1;
                mark(self, b, &mut y, 50, tr(Msg::TemporalDither), !spatial, Some(Choice::Dither));
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
                b.text(cx, y, note as u16, dim(), 1, tr(Msg::Layout));
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
                mark(self, b, &mut y, 50, tr(if armed { Msg::AttestAtBootArmed } else { Msg::AttestAtBoot }), on, Some(Choice::UnlockAtBoot));
                notes(b, &mut y, tr(Msg::AttestAtBootNote));
            }
            "fleet" => {
                // The fleet this identity's devices make (payloads/fgtw): where this device stands, the members, and removing this device. Removing wipes the identity's things from it (AUTH.md: the identity class) and it restarts at the attest screen once a device of the fleet approves. Its words are fgtw's (fgtw::text), the same Photon shows, but for the deck's and what removing wipes here.
                let f = self.fleet;
                let named = |b: &[u8], n: usize, or: FleetMsg<'static>| -> Line {
                    match core::str::from_utf8(&b[..n]) {
                        Ok(s) if !s.is_empty() => {
                            let mut l = Line::new();
                            l.s(s);
                            l
                        }
                        _ => fleet(or),
                    }
                };
                'fleet: {
                // A sibling asking to leave: the request alone, nothing else on the page until it is answered (owner, 2026-11-10: "curious why there's so many buttons on that screen", and "it's a bit vague").
                if let (Some(3), Some((intent, result))) = (f.state, f.depart) {
                    let name = named(&f.depart_name, f.depart_name_n, FleetMsg::OneOfYourDevices);
                    rows(self, b, &mut y, small, white(), fleet(FleetMsg::DepartRequestLine(name.as_str())).as_str());
                    match result {
                        0 | 3 => {
                            // What approving does (it asked to have its hardware freed for a new owner, intent 1, or to keep it with this fleet), a sentence wrapped to the page, and what denying does.
                            rows(self, b, &mut y, note, dim(), fleet(if intent == 1 { FleetMsg::DepartFreesHardware } else { FleetMsg::DepartKeepsHardware }).as_str());
                            notes(b, &mut y, fleet(FleetMsg::DepartDenyKeeps).as_str());
                            button(self, b, &mut y, fleet(if result == 3 { FleetMsg::DepartApproveAgainPill } else { FleetMsg::DepartApprovePill }).as_str(), false, Some(Choice::FleetApprove));
                            button(self, b, &mut y, fleet(FleetMsg::DepartDenyPill).as_str(), false, Some(Choice::FleetDeny));
                        }
                        1 => row(self, b, &mut y, small, grey(), fleet(FleetMsg::DepartApproving).as_str(), None),
                        2 => row(self, b, &mut y, small, grey(), fleet(FleetMsg::DepartApproved).as_str(), None),
                        _ => rows(self, b, &mut y, small, grey(), fleet(FleetMsg::DepartDeniedHere(name.as_str())).as_str()),
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
                    row(self, b, &mut y, small, white(), fleet(if f.state == Some(8) { FleetMsg::DepartAsking } else { FleetMsg::DepartSending }).as_str(), None);
                    notes(b, &mut y, fleet(FleetMsg::DepartTapHint).as_str());
                    notes(b, &mut y, fleet(FleetMsg::DepartThen).as_str());
                    break 'fleet;
                }
                // A departure approved here has gone through: said once, above the page.
                if f.left_n > 0 {
                    let name = named(&f.left, f.left_n, FleetMsg::ADevice);
                    rows(self, b, &mut y, small, white(), fleet(FleetMsg::FleetSignedOut(name.as_str())).as_str());
                }
                let state = match f.state {
                    None => {
                        let mut l = Line::new();
                        l.s(tr(Msg::FleetServiceNotRunning));
                        l
                    }
                    Some(0) => fleet(FleetMsg::NoIdentityHere),
                    Some(1) => fleet(FleetMsg::JoiningConfirm),
                    Some(2) => fleet(FleetMsg::InFleetWaitingKey),
                    Some(3) => fleet(FleetMsg::InFleet),
                    Some(8) => fleet(FleetMsg::LeavingFleet),
                    // fgtw's verdicts while not yet bound (payloads/fgtw V_*), each its own words: until 2026-11-08 all four read "looking the identity up", forever when offline.
                    Some(5) => fleet(FleetMsg::NewIdentityNoFleet),
                    Some(6) => fleet(FleetMsg::HandleHasOtherFleet),
                    Some(7) => fleet(FleetMsg::NoVerdict),
                    Some(_) => fleet(FleetMsg::LookingUp),
                };
                row(self, b, &mut y, small, white(), state.as_str(), None);
                for i in 0..f.n {
                    // Each member by the two-word name its fleet knows it by (Photon's Fleet page shows the same one); the key's first four bytes only when the service gave no name.
                    let mut name = Line::new();
                    match core::str::from_utf8(&f.names[i][..f.names_n[i] as usize]) {
                        Ok(s) if !s.is_empty() => {
                            name.s(s);
                        }
                        _ => {
                            for x in &f.keys[i][..4] {
                                let _ = write!(name, "{x:02x}");
                            }
                        }
                    }
                    let l = if f.me[i] {
                        fleet(FleetMsg::MemberThisDevice(name.as_str()))
                    } else if f.heard[i] {
                        fleet(FleetMsg::MemberNearby(name.as_str()))
                    } else {
                        name
                    };
                    row(self, b, &mut y, small, if f.me[i] { white() } else { grey() }, l.as_str(), None);
                }
                // Adding a device, and approving a sibling's departure (fgtw::sponsor, through payloads/fgtw ops 8-15): a member of the fleet only.
                if f.state == Some(3) {
                    // The deck across the person's devices (owner, 2026-11-08: "on all my ferros devices the homescreen layout should sync, unless of course I go into settings and uncheck that"): its kists and answers always follow them; its arrangement does unless switched off here.
                    if let Some((on, settled)) = f.deck {
                        // A checkbox once the fleet has answered: checked, the same on all your devices; unchecked, this device's own.
                        if settled {
                            mark(self, b, &mut y, 50, tr(Msg::DeckSame), on, Some(Choice::DeckSync));
                        } else {
                            row(self, b, &mut y, small, grey(), tr(Msg::DeckFetching), None);
                        }
                    }
                    if f.denied_by_n > 0 {
                        let name = named(&f.denied_by, f.denied_by_n, FleetMsg::OneOfYourDevices);
                        rows(self, b, &mut y, small, white(), fleet(FleetMsg::DepartDeniedOn(name.as_str())).as_str());
                    }
                    if f.add == 0 {
                        button(self, b, &mut y, fleet(FleetMsg::AddDevicePill).as_str(), false, Some(Choice::FleetAdd));
                        // Only a device heard here (its beacon or its request on this network, flags bits 0 and 1): one known only from fgtw.org's registry is no one nearby, and a join asked from anywhere on the internet is said nowhere (owner, 2026-11-10: "LAN, fine. WAN, nope").
                        if f.cand_flags[..f.cands].iter().any(|&fl| fl & 3 != 0) {
                            notes(b, &mut y, fleet(FleetMsg::DeviceAsksToJoin).as_str());
                        }
                    } else {
                        let phase = match f.add {
                            1 => FleetMsg::AddAsking,
                            2 => FleetMsg::AddingToFleet,
                            3 => FleetMsg::AddSelected,
                            4 => FleetMsg::HandingKey,
                            5 => FleetMsg::Added,
                            _ => FleetMsg::AddingFailed,
                        };
                        // Binding or handing over the key (phases 2 and 4): the device being added by name, over what is being done with it (op 9's last field).
                        if matches!(f.add, 2 | 4) && f.adding_name_n > 0 {
                            let name = named(&f.adding_name, f.adding_name_n, FleetMsg::TheDevice);
                            row(self, b, &mut y, small, white(), fleet(FleetMsg::AddingDevice(name.as_str())).as_str(), None);
                        }
                        row(self, b, &mut y, small, white(), fleet(phase).as_str(), None);
                        // The identity service's own note on the flow, in its words (payloads/fgtw add.rs, op 9).
                        if f.add_note_n > 0 {
                            b.text(cx, y - uu(1, 1), note as u16, dim(), 1, core::str::from_utf8(&f.add_note[..f.add_note_n]).unwrap_or(""));
                            y += uu(2, 1);
                        }
                        for i in 0..f.cands {
                            // A device the add machine offers as a pick (flag bit 3: heard nearby, and over Bluetooth only while more than one device asks) is a tap to pick; one only the registry knows is matched by its words.
                            let nearby = f.cand_flags[i] & 3 != 0;
                            let name = named(&f.cand_name[i], f.cand_name_n[i] as usize, FleetMsg::ADevice);
                            let l = fleet(if f.cand_flags[i] & 4 != 0 {
                                FleetMsg::CandAdded(name.as_str())
                            } else if nearby {
                                FleetMsg::CandNearby(name.as_str())
                            } else {
                                FleetMsg::CandNotNearby(name.as_str())
                            });
                            let pick = f.cand_flags[i] & 8 != 0 && f.add == 1;
                            row(self, b, &mut y, small, if pick { white() } else { grey() }, l.as_str(), pick.then_some(Choice::FleetPick(i as u8)));
                        }
                        if f.add == 1 {
                            if f.cands == 0 {
                                notes(b, &mut y, fleet(FleetMsg::OpenPhotonJoin).as_str());
                            }
                            button(self, b, &mut y, fleet(FleetMsg::TypeItsWordsPill).as_str(), false, Some(Choice::FleetWords));
                            notes(b, &mut y, fleet(FleetMsg::HoldTogether).as_str());
                        }
                        // Selected (op 9 phase 3, owner 2026-11-10: "I need a distinct green add, and note to press back otherwise"): the device and its words, the same it shows, Add in VSF RGB primary green, and Back with nothing to undo.
                        if f.add == 3 {
                            let name = named(&f.sel_name, f.sel_name_n, FleetMsg::TheDevice);
                            row(self, b, &mut y, small, white(), fleet(FleetMsg::SelectedDevice(name.as_str())).as_str(), None);
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
                            // The question and its two answers side by side (owner, 2026-11-10: "Is your new device bright green? and that should have a yes and no, no does the same as back"): Yes in VSF RGB green adds it; No goes back to the list, nothing to undo.
                            rows(self, b, &mut y, small, white(), fleet(FleetMsg::IsNewDeviceGreen).as_str());
                            let (bw, off) = (uu(7, 1), uu(4, 1));
                            let yes_down = self.pressed == Some(Choice::FleetConfirm);
                            b.button(cx - off, y - uu(1, 2), bw, uu(2, 1), uu(5, 4), [if yes_down { GREEN_HELD } else { GREEN }, GREEN_EDGE, GREEN_EDGE, 0x000000], fleet(FleetMsg::YesPill).as_str());
                            let [fill, held, tl, br] = BUTTON_LOOK[ferros_sys::theme::index()];
                            let no_down = self.pressed == Some(Choice::FleetAdd);
                            b.button(cx + off, y - uu(1, 2), bw, uu(2, 1), uu(5, 4), [if no_down { held } else { fill }, tl, br, white()], fleet(FleetMsg::NoPill).as_str());
                            if hit {
                                self.push_row(cx - half, cx, y - uu(2, 1), y + uu(1, 1), Choice::FleetConfirm);
                                self.push_row(cx, cx + half, y - uu(2, 1), y + uu(1, 1), Choice::FleetAdd);
                            }
                            y += uu(3, 1);
                        }
                        button(self, b, &mut y, fleet(if f.add >= 5 { FleetMsg::DonePill } else { FleetMsg::ClosePill }).as_str(), false, Some(Choice::FleetClose));
                    }
                }
                if matches!(f.state, Some(2) | Some(3)) {
                    button(self, b, &mut y, fleet(FleetMsg::RemovePill { armed: f.confirm }).as_str(), f.confirm, Some(Choice::FleetRemove));
                    if f.confirm {
                        notes(b, &mut y, tr(Msg::RemoveNote));
                    }
                    // 4: the identity's last device cannot leave (fgtw's departure machine refuses it); any other refusal, the departure did not start.
                    match f.leave_result {
                        Some(4) => rows(self, b, &mut y, small, white(), fleet(FleetMsg::LastDevice).as_str()),
                        Some(r) if r != 0 => row(self, b, &mut y, small, white(), fleet(FleetMsg::DepartNotStarted).as_str(), None),
                        _ => {}
                    }
                }
                }
            }
            "power" => {
                let on = ferros_sys::pref(ferros_sys::PREF_PROX_WAKE) == 1;
                mark(self, b, &mut y, 50, tr(Msg::ProximityWake), on, Some(Choice::ProxWake));
                notes(b, &mut y, tr(Msg::ProximityWakeNote));
            }
            "connections" => {
                // Each link as its driver says it (measure STATE), each row its own page's door (owner, 2026-11-07).
                let facts = ferros_sys::facts();
                for (k, name) in LINKS.iter().enumerate() {
                    let state = facts.0[33 + 8 * k..41 + 8 * k].iter().filter_map(|&m| ferros_sys::measure_of(m)).find(|m| m.0 == ferros_sys::measure::STATE).map(|m| m.1);
                    let y_row = y;
                    // Switched off by its status circle (preference 15; USB, the sixth, has no switch): the driver's last word outlives it, so the switch is read first.
                    fact(b, &mut y, page_title(name), link_state(k, state));
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
                    mark(self, b, &mut y, 50, tr(Msg::On), !off, Some(Choice::Switch(k as u8)));
                }
                fact(b, &mut y, tr(Msg::State), link_state(k, state));
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
                // GPS: where you are, once the driver this page started (its claim) has a fix (owner, 2026-11-10: "bring up GPS and have the dymaxion words, lat/lon decimal and dms all for display"); then who is using GPS now, this page among them. On the screen only: none of it is logged or kept.
                if k == 4 {
                    header(b, &mut y, tr(Msg::RightGps));
                    match self.gps_fix {
                        Some(Some(fx)) => {
                            let deg = |v: i32| v as f64 / (1u64 << 23) as f64;
                            // The seven Dymaxion words (vsf WorldCoord): the point to a few millimetres, as words to say aloud.
                            let words = ferros_kist::vsf::types::WorldCoord::from_lat_lon(deg(fx.lat), deg(fx.lon)).to_words();
                            fact_wrap(b, &mut y, tr(Msg::PositionWords), &words);
                            let mut l = Line::new();
                            degrees_linear(&mut l, f, fx.lat);
                            l.s(", ");
                            degrees_linear(&mut l, f, fx.lon);
                            fact_wrap(b, &mut y, tr(Msg::LatLon), l.as_str());
                            let mut l = Line::new();
                            degrees_dms(&mut l, f, fx.lat, 'N', 'S');
                            l.s(", ");
                            degrees_dms(&mut l, f, fx.lon, 'E', 'W');
                            fact_wrap(b, &mut y, tr(Msg::DegMinSec), l.as_str());
                        }
                        // Under its heading alone, "where you are" said once (owner, 2026-11-10: it showed as the heading and again as the row's name); "using it now" the same.
                        // No fix yet, and what the chip is doing meanwhile (owner, 2026-11-11: "there's zero satellites showing on this page is the thing ... I'd like to see what's actually going on in this thing"): its frames so far and their kinds, moving as it talks (the driver's doorbell moves every 64 frames). Its satellites come only with a fix message (GPS.md "What the chip sends before a fix").
                        Some(None) => {
                            row(self, b, &mut y, small, white(), tr(Msg::NoFixYet), None);
                            let h = GPS_HEARD.load(core::sync::atomic::Ordering::Acquire);
                            if h != 0 {
                                let l = say(Say::ChipHeard { messages: h >> 8, kinds: h & 0xFF });
                                row(self, b, &mut y, small, white(), l.as_str(), None);
                            }
                        }
                        None if self.gps_claimed => row(self, b, &mut y, small, white(), tr(Msg::StartingUp), None),
                        None => row(self, b, &mut y, small, white(), tr(Msg::SwitchedOff), None),
                    }
                    header(b, &mut y, tr(Msg::UsingNow));
                    let users = self.users(ferros_sys::claim::GPS);
                    if users.is_empty() {
                        row(self, b, &mut y, small, white(), tr(Msg::NoneYet), None);
                    }
                    for u in &users {
                        let mut l = Line::new();
                        self.user_name(u, &mut l);
                        row(self, b, &mut y, small, white(), l.as_str(), None);
                    }
                    // Who may use it (owner, 2026-11-08: "who has GPS"): each kist that wants GPS, its switch the person's answer as on the permissions page, "using now" by its name while it holds a claim.
                    let ks = self.kists.kists.clone();
                    let gps = ferros_sys::claim::GPS as u8;
                    if ks.iter().any(|kk| kk.wants.iter().any(|w| w.right == gps)) {
                        header(b, &mut y, tr(Msg::PagePermissions));
                    }
                    for (ki, kk) in ks.iter().enumerate() {
                        if let Some(j) = kk.wants.iter().position(|w| w.right == gps) {
                            let l = self.want_label(kk, j, &users);
                            mark(self, b, &mut y, 50, l.as_str(), kk.wants[j].answer.is_some_and(|a| a.0), Some(Choice::Answer(ki as u8, j as u8)));
                        }
                    }
                }
                let mut any = false;
                for (g, gname) in GROUPS.iter().enumerate() {
                    let mut first = true;
                    for &w in all {
                        let Some((kind, v)) = ferros_sys::measure_of(w) else { continue };
                        if kind == ferros_sys::measure::STATE || kind == ferros_sys::measure::SAT || group_of(kind) != g {
                            continue;
                        }
                        if first {
                            header(b, &mut y, tr(*gname));
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
                        header(b, &mut y, tr(Msg::Satellites));
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
                    // Where it stands and how strong, its numbers written in the device's base and handed to the person's language whole.
                    let (mut up, mut round, mut cn0) = (Line::new(), Line::new(), Line::new());
                    cnt(&mut up, s.elevation as i64);
                    cnt(&mut round, s.azimuth as i64);
                    cnt(&mut cn0, s.cn0 as i64);
                    let l = say(Say::SatPlace { up: up.as_str(), round: round.as_str(), cn0: cn0.as_str(), used: s.used });
                    fact(b, &mut y, name.as_str(), l.as_str());
                }
                if !any && nsat == 0 {
                    fact(b, &mut y, tr(Msg::Measures), tr(Msg::NoneYet));
                }
                // The names and addresses: shown only here, to whoever holds the settings.
                let mut doc = [0u8; 1 << 10];
                if let Some(n) = ferros_sys::detail(cap(self.grants, S_SETTINGS), k as u64, &mut doc) {
                    // WHY the bound: the kernel returns the document's whole length, which the call allows to exceed the buffer; a document this page cannot hold whole is not read.
                    if n > 0 && n <= doc.len() {
                        if let Some(fields) = ferros_detail::parse(k as u64, &doc[..n]) {
                            if !fields.is_empty() {
                                header(b, &mut y, tr(Msg::NamesAndAddresses));
                            }
                            for (name, _, v) in fields.iter() {
                                let mut l = Line::new();
                                detail_words(&mut l, f, v);
                                fact_wrap(b, &mut y, detail_label(name), l.as_str());
                            }
                        }
                    }
                }
                // Attest with a tag (AUTH.md; owner, 2026-11-11: "ideally something in settings to add a tag"): the tags kept to attest this device with, each by the start of its id (a keyed hash of its UID: the UID never comes back from the kernel) and when it was kept, each with its remove; "add a tag", which waits for the next tag held to the device while this page shows (the tap on the page is the consent, AUTH.md "An NFC tap is consent"); and what a kept tag means, warned as attest at boot is.
                if k == 3 {
                    header(b, &mut y, tr(Msg::TagAttest));
                    let tags = self.tags;
                    for (i, t) in tags[..self.ntags].iter().enumerate() {
                        let mut name = Line::new();
                        name.s("G#");
                        for byte in &t.id[..4] {
                            let _ = write!(name, "{byte:02X}");
                        }
                        let mut when = Line::new();
                        if t.added == 0 {
                            when.s("-");
                        } else {
                            // Eagle time to the wall clock in the zone, for the eye only (tatau writes it in the device's base; hex shows the Eagle stamp itself).
                            const EPOCH_UNIX: i64 = -14_182_940;
                            let ops = ferros_kist::vsf::OSCILLATIONS_PER_SECOND as i64;
                            let secs = t.added.div_euclid(ops) + EPOCH_UNIX + ferros_sys::zone();
                            let ns = (t.added.rem_euclid(ops) as i128 * 1_000_000_000 / ops as i128) as u32;
                            let _ = tatau::date_time(&mut when, f, secs, ns, t.added);
                        }
                        fact(b, &mut y, name.as_str(), when.as_str());
                        button(self, b, &mut y, tr(Msg::TagRemove), false, Some(Choice::TagRemove(i as u8)));
                    }
                    if self.ntags == 0 {
                        row(self, b, &mut y, small, white(), tr(Msg::NoneYet), None);
                    }
                    let adding = TAG_ADD.load(core::sync::atomic::Ordering::Acquire) != 0;
                    button(self, b, &mut y, tr(if adding { Msg::Cancel } else { Msg::TagAdd }), adding, Some(Choice::TagAdd));
                    let said = match self.tag_said {
                        _ if adding => Some(Msg::TagHold),
                        1 => Some(Msg::TagKeptNow),
                        2 => Some(Msg::TagKeptAlready),
                        3 => Some(Msg::TagNotKept),
                        _ => None,
                    };
                    if let Some(m) = said {
                        row(self, b, &mut y, small, white(), tr(m), None);
                    }
                    notes(b, &mut y, tr(Msg::TagNote));
                }
                // The knobs (ferros_sys::control): a value goes round its choices at each tap, an action happens at its tap.
                header(b, &mut y, tr(Msg::Settings));
                let c = ferros_sys::controls(k as u64);
                use ferros_sys::control as ctl;
                match k as u64 {
                    ctl::CELLULAR => {
                        button(self, b, &mut y, tr(Msg::ReRegisterNow), false, Some(Choice::Knob(k as u8, ctl::CELL_REREGISTER as u8)));
                        b.text(cx, y - uu(1, 1), note as u16, dim(), 1, tr(Msg::ReRegisterNote));
                        y += uu(2, 1);
                    }
                    ctl::WIFI => {
                        // Each value knob a radio group, its choices in its own order (unset reads as `unset`'s: power save follows the screen, the band is any).
                        let save = [tr(Msg::Off), tr(Msg::On), tr(Msg::Fast), tr(Msg::WithTheScreen)];
                        let band = [tr(Msg::BandAny), tr(Msg::Band5Only), tr(Msg::Band24Only)];
                        for (slot, title, names, unset) in [(ctl::WIFI_POWER_SAVE, tr(Msg::PowerSave), &save[..], 3), (ctl::WIFI_BAND, tr(Msg::Band), &band[..], 0)] {
                            let now = if c[slot as usize] == ctl::UNSET { unset } else { c[slot as usize] };
                            b.text(cx, y, note as u16, dim(), 1, title);
                            y += uu(2, 1);
                            radios(self, b, &mut y, names, now as usize, &|v| Choice::KnobSet(k as u8, slot as u8, v as u8));
                        }
                        notes(b, &mut y, tr(Msg::BandNote));
                    }
                    _ => {
                        b.text(cx, y, note as u16, dim(), 1, tr(Msg::NothingToSetYet));
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
                        fact(b, &mut y, tr(Msg::AtUtc), l.as_str());
                    }
                    None => {
                        fact(b, &mut y, tr(Msg::Time), tr(Msg::NotReadYet));
                    }
                }
                // An offset as the page writes it: UTC, then how far east (+) or west (-), in the device's base.
                let utc = |l: &mut Line, s: i64| {
                    l.s("UTC");
                    if s != 0 {
                        l.s(if s < 0 { " - " } else { " + " });
                        let _ = tatau::duration_ns(l, f, s.unsigned_abs() * 1_000_000_000, 1);
                    }
                };
                let st = ferros_sys::zone::state();
                // The zone picker while it is open (Choice::ZonePicker): it alone under the time, the regions then a region's places (tzdata's names, which stay). Taken out of the map while the page is laid out, put back after.
                if let Some(pk) = self.zone_pick.take() {
                    let ruling = st.map(|s| s.ruling().1);
                    row(self, b, &mut y, small, white(), match pk.region {
                        None => tr(Msg::ChooseAZone),
                        Some(r) => pk.region(r),
                    }, None);
                    button(self, b, &mut y, tr(Msg::Back), false, Some(Choice::ZoneBack));
                    // A row of the list: drawn and tappable only on the page's band (a region holds over a hundred places, past the batch's room and the page's 64 tappable rows), its place on the page kept either way.
                    let item = |m: &mut Map, b: &mut Batch, y: &mut i64, t: &str, on: bool, what: Choice| {
                        if *y + uu(2, 1) > top && *y - uu(2, 1) < bottom {
                            b.text(cx, *y, small as u16, if on { white() } else { grey() }, 1, t);
                            if hit {
                                m.push_row(cx - half, cx + half, *y - uu(2, 1), *y + uu(1, 1), what);
                            }
                        }
                        *y += uu(3, 1);
                    };
                    match pk.region {
                        None => {
                            item(self, b, &mut y, tr(Msg::NoneFixedOffset), ruling.is_some_and(|r| r.is_none()), Choice::ZoneNone);
                            for r in 0..pk.regions.len() {
                                let on = ruling.is_some_and(|z| z.as_str().split('/').next() == Some(pk.region(r)));
                                item(self, b, &mut y, pk.region(r), on, Choice::ZoneRegion(r as u16));
                            }
                        }
                        Some(r) => {
                            for k in pk.places(r) {
                                let mut l = Line::new();
                                zone_words(&mut l, pk.id(k), false);
                                item(self, b, &mut y, l.as_str(), ruling.is_some_and(|z| z.as_str() == pk.id(k)), Choice::ZoneCity(k as u16));
                            }
                        }
                    }
                    self.zone_pick = Some(pk);
                } else {
                    let (standard, legal) = ferros_sys::zone_offsets();
                    match st {
                        None => fact(b, &mut y, tr(Msg::Zone), tr(Msg::NoZonesInKernel)),
                        Some(st) => {
                            // The zone in effect, its two offsets (arabic shows the legal one, dozenal and hex the standard one all year), and where it comes from.
                            let mut l = Line::new();
                            if st.effective.is_none() {
                                utc(&mut l, standard);
                            } else {
                                zone_words(&mut l, st.effective.as_str(), true);
                            }
                            fact(b, &mut y, tr(Msg::Zone), l.as_str());
                            if !st.effective.is_none() {
                                let mut l = Line::new();
                                utc(&mut l, standard);
                                fact(b, &mut y, tr(Msg::Standard), l.as_str());
                                let mut l = Line::new();
                                utc(&mut l, legal);
                                let l = if legal != standard { say(Say::DaylightTime(l.as_str())) } else { l };
                                fact(b, &mut y, tr(Msg::Legal), l.as_str());
                            }
                            fact(b, &mut y, tr(Msg::From), tr(match st.from {
                                ferros_sys::zone::FROM_NETWORK => Msg::FromNetwork,
                                ferros_sys::zone::FROM_POSITION => Msg::FromPosition,
                                ferros_sys::zone::FROM_HAND => Msg::FromHand,
                                _ => Msg::FromOffset,
                            }));
                            if st.from == ferros_sys::zone::FROM_POSITION {
                                notes(b, &mut y, tr(Msg::NearestPlaceNote));
                            }
                            y += uu(1, 2);
                            // Whose choice rules (owner, 2026-11-10: "it's per identity, you can uncheck and set per device tho"): the identity's, the same on all the person's devices that follow it, or this device's own.
                            mark(self, b, &mut y, 50, tr(Msg::FollowMyIdentity), st.follow, Some(Choice::ZoneFollow));
                            let says = match (st.follow, st.identity) {
                                (false, _) => Msg::ZoneOwn,
                                (true, ferros_sys::zone::IDENTITY_NONE) => Msg::ZoneIdentityNone,
                                (true, _) => Msg::SetHereAllFollow,
                            };
                            b.text(cx, y - uu(1, 1), note as u16, dim(), 1, tr(says));
                            y += uu(2, 1);
                            let (auto, id) = st.ruling();
                            radios(self, b, &mut y, &[tr(Msg::Automatic), tr(Msg::ByHand)], if auto { 0 } else { 1 }, &|v| Choice::ZoneAuto(v == 0));
                            if auto {
                                notes(b, &mut y, tr(if matches!(st.from, ferros_sys::zone::FROM_HAND | ferros_sys::zone::FROM_OFFSET) { Msg::ZoneAutoNothingFound } else { Msg::ZoneAutoStandsIn }));
                            }
                            let mut l = Line::new();
                            if id.is_none() {
                                l.s(tr(Msg::NoneFixedOffset));
                            } else {
                                zone_words(&mut l, id.as_str(), true);
                            }
                            row(self, b, &mut y, small, white(), l.as_str(), None);
                            button(self, b, &mut y, tr(Msg::ChooseAZone), false, Some(Choice::ZonePicker));
                            if id.is_none() {
                                button(self, b, &mut y, tr(Msg::QuarterHourEast), false, Some(Choice::Zone(1)));
                                button(self, b, &mut y, tr(Msg::QuarterHourWest), false, Some(Choice::Zone(-1)));
                            }
                        }
                    }
                    if let Some(t) = self.zone_note {
                        row(self, b, &mut y, note, white(), tr(t), None);
                    }
                    y += uu(1, 2);
                    b.text(cx, y, note as u16, dim(), 1, tr(Msg::Sources));
                    y += uu(2, 1);
                    fact(b, &mut y, tr(Msg::From), tr(Msg::ThePmicClock));
                    fact(b, &mut y, tr(Msg::Read), tr(Msg::AtStartThenHourly));
                    // The network's word on the zone (NITZ and its country), as the kernel heard it; its time is not used yet.
                    let l = match st.filter(|s| s.mcc != 0) {
                        Some(s) => {
                            let mut mcc = Line::new();
                            let _ = tatau::count(&mut mcc, f, s.mcc as u64, 1);
                            match s.country {
                                Some(c) => say(Say::HeardMccIn { mcc: mcc.as_str(), country: core::str::from_utf8(&c).unwrap_or("?") }),
                                None => say(Say::HeardMcc(mcc.as_str())),
                            }
                        }
                        None => {
                            let mut l = Line::new();
                            l.s(tr(Msg::NotHeardYet));
                            l
                        }
                    };
                    fact(b, &mut y, tr(Msg::NetworkZone), l.as_str());
                    fact(b, &mut y, tr(Msg::NetworkTime), tr(Msg::NotUsedYet));
                    fact(b, &mut y, tr(Msg::GpsTime), tr(Msg::NotUsedYet));
                    fact(b, &mut y, tr(Msg::PageUnits), base_name(ferros_sys::device_base()));
                }
            }
            "language" => {
                // The language (SETTINGS.md), as the zone: whose choice rules (the identity's, the same on all the person's devices that follow it, or this device's own), then every language the display can draw, each named in itself (Photon's picker: autonyms, in its order), the one in effect chosen. A pick is the identity's while following it (payloads/fgtw op 23), else this device's (SYS_LANG op 0).
                let follows = ferros_sys::lang::follows();
                mark(self, b, &mut y, 50, tr(Msg::FollowMyIdentity), follows, Some(Choice::LangFollow));
                let says = match (follows, ferros_sys::lang::identity()) {
                    (false, _) => Msg::LangOwn,
                    (true, None) => Msg::LangIdentityNone,
                    (true, Some(_)) => Msg::SetHereAllFollow,
                };
                b.text(cx, y - uu(1, 1), note as u16, dim(), 1, tr(says));
                y += uu(2, 1);
                // The one shown chosen is the language in effect (English while none is, or one the display cannot draw is).
                let now = ferros_lang::current();
                let langs: Vec<Lang> = languages().collect();
                let names: Vec<&str> = langs.iter().map(|l| l.autonym()).collect();
                let at = langs.iter().position(|&l| l == now).unwrap_or(usize::MAX);
                radios(self, b, &mut y, &names, at, &|v| Choice::Lang(langs[v].index()));
                notes(b, &mut y, tr(Msg::MoreLanguagesNeedFace));
                if let Some(t) = self.zone_note {
                    row(self, b, &mut y, note, white(), tr(t), None);
                }
            }
            "battery" => {
                // The battery (owner, 2026-11-11: "might have to build a battery page"), the console's battery circle's page: what the kernel's gauge reads every two seconds (facts 1-4, the MAX77759 fuel gauge), each in the person's base, drawn again at the facts' bell. The charge, then the current's way, the cell's voltage, the current, their product and the temperature. The gauge's health (full capacity against design, cycles) is not read by the kernel yet.
                let f = ferros_sys::numbers(Style::Glyphs);
                let facts = ferros_sys::facts();
                // A whole count in the base with its unit, signed.
                let num = |n: i64, unit: &str| {
                    let mut l = Line::new();
                    if n < 0 {
                        l.s("-");
                    }
                    if let Ok(tatau::Out::Nothing) = tatau::count(&mut l, f, n.unsigned_abs(), 1) {
                        l.s("0");
                    }
                    l.s(unit);
                    l
                };
                let not_read = || {
                    let mut l = Line::new();
                    l.s(tr(Msg::NotRead));
                    l
                };
                let charge = match facts.battery_pct() {
                    Some(p) => {
                        let mut l = Line::new();
                        let _ = tatau::share(&mut l, f, p, 100, 2);
                        l
                    }
                    None => not_read(),
                };
                b.text(cx, y, (uu(3, 1)) as u16, white(), 1, charge.as_str());
                y += uu(3, 1);
                let (uv, ua) = (facts.cell_uv(), facts.battery_ua());
                if let Some(ua) = ua {
                    row(self, b, &mut y, small, grey(), tr(if ua > 0 { Msg::Charging } else { Msg::Discharging }), None);
                }
                fact(b, &mut y, tr(Msg::Charge), charge.as_str());
                fact(b, &mut y, tr(Msg::Voltage), uv.map_or_else(not_read, |v| num((v / 1000) as i64, " mV")).as_str());
                fact(b, &mut y, tr(Msg::Current), ua.map_or_else(not_read, |a| num(a / 1000, " mA")).as_str());
                let power = match (uv, ua) {
                    (Some(v), Some(a)) => num((v as i64 / 1000) * (a / 1000) / 1000, " mW"),
                    _ => not_read(),
                };
                fact(b, &mut y, tr(Msg::PowerDraw), power.as_str());
                fact(b, &mut y, tr(Msg::Temperature), facts.battery_temp_cc().map_or_else(not_read, |c| num(c / 100, " °C")).as_str());
            }
            "torch" => {
                // The torch (docs/TORCH.md "Settings"; owner, 2026-11-11: "like flashlight we may want to control the brightness and strobe frequency and duty cycle and timeout"), its quick circle's page (QUICK.md "Quick pages"): on and off as the circle's tap, then each of its typed settings (typed.rs), its name with its value in words, its slider under it, the value under a finger as it moves. Set with the torch dark, they wait for its next light.
                let lit = quick::torch_now().is_some_and(|l| l > 0);
                mark(self, b, &mut y, 50, tr(Msg::On), lit, Some(Choice::TorchOn));
                for i in typed::TORCH {
                    typed_row(self, b, &mut y, i);
                }
            }
            "about" => {
                let f = ferros_sys::numbers(Style::Glyphs);
                let facts = ferros_sys::facts();
                b.text(cx, y, (uu(2, 1)) as u16, white(), 1, "ferros");
                y += uu(3, 1);
                row(self, b, &mut y, small, grey(), tr(if DEV { Msg::VersionDev } else { Msg::Version }), Some(Choice::Version));
                // The hint from the fourth tap on, counting down to Developer; it stays until the next tap changes it (no timers).
                if self.taps >= DEV_HINT {
                    let l = if DEV || self.dev {
                        let mut l = Line::new();
                        l.s(tr(Msg::DeveloperAlready));
                        l
                    } else {
                        say(Say::TapsToDeveloper((DEV_TAPS - self.taps) as u64))
                    };
                    b.text(cx, y - uu(1, 1), note as u16, white(), 1, l.as_str());
                    y += uu(2, 1);
                }
                y += uu(1, 1);
                let v = match facts.battery_pct() {
                    Some(p) => {
                        let mut pct = Line::new();
                        let _ = tatau::share(&mut pct, f, p, 100, 2);
                        match facts.battery_ua() {
                            Some(ua) if ua > 0 => say(Say::Charging(pct.as_str())),
                            Some(_) => say(Say::OnBattery(pct.as_str())),
                            None => pct,
                        }
                    }
                    None => {
                        let mut v = Line::new();
                        v.s(tr(Msg::NotRead));
                        v
                    }
                };
                fact(b, &mut y, tr(Msg::Battery), v.as_str());
                let mut v = Line::new();
                match facts.boot_us() {
                    Some(us) => {
                        let _ = tatau::duration_ns(&mut v, f, us * 1000, 1);
                    }
                    None => {
                        v.s("-");
                    }
                }
                fact(b, &mut y, tr(Msg::PressToReady), v.as_str());
                let mut v = Line::new();
                // The counter runs from the SoC's reset: power on, near enough (the bootloader's seconds included).
                let (t, hz) = (now(), freq());
                let _ = tatau::duration_ns(&mut v, f, t / hz * 1_000_000_000 + t % hz * 1_000_000_000 / hz, 1);
                fact(b, &mut y, tr(Msg::SincePowerOn), v.as_str());
                let mut v = Line::new();
                if let Some(n) = facts.processes() {
                    let _ = tatau::count(&mut v, f, n, 1);
                }
                fact(b, &mut y, tr(Msg::Processes), v.as_str());
                let mut v = Line::new();
                if let Some(n) = facts.vault_keys() {
                    let _ = tatau::count(&mut v, f, n, 1);
                }
                fact(b, &mut y, tr(Msg::InTheVault), v.as_str());
                // The hottest part of the SoC as the kernel last read it, and whether the kernel has the big core's clock down for it.
                let mut v = Line::new();
                match facts.soc_temp_c() {
                    Some(c) => {
                        let _ = tatau::count(&mut v, f, c, 1);
                        v.s(" °C");
                        if facts.throttled() == Some(true) {
                            v = say(Say::Throttled(v.as_str()));
                        }
                    }
                    None => {
                        v.s("-");
                    }
                }
                fact(b, &mut y, tr(Msg::Chip), v.as_str());
            }
            "deck" => {
                // Everything on the deck, each row opening its card (`card`): what it is, who made it, its rights as switches (owner, 2026-11-08: per kist, its rights; a tap changes the answer for every device of the person's, and a kist running is killed and started again with the answers as they now stand), stop, update and unstow.
                let ks = self.kists.kists.clone();
                // The stower's (stower.rs): the directory and the jobs under way. Sizes in the person's number base.
                let inst = self.stower.clone();
                let f = ferros_sys::numbers(Style::Glyphs);
                // A job's bar (fluor's slider track, op 48, its knob no wider than the track) and its stage in words under it; a finished job's words alone.
                let progress = |m: &mut Map, b: &mut Batch, y: &mut i64, j: &stower::Job| {
                    if j.stage < stower::DONE {
                        let (a, z) = m.track(cx);
                        let tr = uq >> 3;
                        b.slider(a, z, *y, tr, tr, stower::share(j), white(), dim(), white());
                        *y += uu(3, 2);
                    }
                    let l = stower::words(j, f);
                    row(m, b, y, note, if j.stage == stower::FAILED { white() } else { grey() }, l.as_str(), None);
                };
                // A kist offered over USB (docs/KISTS.md "Stowing"; owner, 2026-11-11: "the proper term for installing a kist is stow"), above everything: while the stower checks it, its bar; once checked, what it is, who made it, its version and size, plainly whether its code is unchecked (a development build's native code), what it asks for and why, then Stow and Cancel; then how it ended, until the job gives way. Nothing is in the person's list before Stow. No icon yet: a capsule carries none (the circles' icons are the display's own).
                for j in inst.jobs.iter().filter(|j| j.kind == stower::KIND_OFFER) {
                    match (j.stage, inst.offer.as_ref().filter(|o| o.capsule == j.capsule)) {
                        (stower::ASKING, Some(o)) => {
                            row(self, b, &mut y, uu(3, 2), white(), tr(Msg::OfferedOverUsb), None);
                            let mut l = Line::new();
                            let _ = write!(l, "{} ", o.name);
                            let _ = ferros_kist::write_version(&mut l, f, &o.version.0);
                            row(self, b, &mut y, small, white(), l.as_str(), None);
                            // Its developer by the name the capsule gives (the person's pin keeps the same), else its key's first bytes.
                            let mut d = Line::new();
                            if o.developer_name.is_empty() {
                                let _ = write!(d, "G#{:02X}{:02X}{:02X}{:02X}", o.developer[0], o.developer[1], o.developer[2], o.developer[3]);
                            } else {
                                d.s(&o.developer_name);
                            }
                            let by = say(Say::ByDeveloper(d.as_str()));
                            row(self, b, &mut y, note, grey(), by.as_str(), None);
                            let mut s = Line::new();
                            let _ = tatau::size_bits(&mut s, f, o.size << 3, 1);
                            row(self, b, &mut y, note, grey(), s.as_str(), None);
                            if o.native {
                                row(self, b, &mut y, small, white(), tr(Msg::UncheckedDevOnly), None);
                            }
                            // Only what its first start will ask: the implicit set (ferros_kist::IMPLICIT) is every kist's, never asked, so an older capsule's declaration of it is not shown.
                            if !o.wants.iter().any(|w| !ferros_kist::implicit(w.0)) {
                                row(self, b, &mut y, note, dim(), tr(Msg::AsksForNothing), None);
                            } else {
                                row(self, b, &mut y, note, grey(), tr(Msg::AsksAtFirstStart), None);
                                for (right, reason) in o.wants.iter().filter(|w| !ferros_kist::implicit(w.0)) {
                                    row(self, b, &mut y, small, white(), right_name(*right as usize), None);
                                    y -= uu(3, 4);
                                    row(self, b, &mut y, note, dim(), reason, None);
                                }
                            }
                            button(self, b, &mut y, tr(Msg::Stow), false, Some(Choice::Stow(true)));
                            button(self, b, &mut y, tr(Msg::Cancel), false, Some(Choice::Stow(false)));
                        }
                        // Under way (its bar), or ended: its name when the check found it, the words under.
                        _ => {
                            row(self, b, &mut y, small, white(), if j.name.is_empty() { tr(Msg::OfferedOverUsb) } else { &j.name }, None);
                            progress(self, b, &mut y, j);
                        }
                    }
                    y += uu(1, 1);
                }
                // A thing's card (owner, 2026-11-12, of four ways to see a kist's info: "just 4", the Deck's row opening its card): the only place its info shows, in place of the list while it is open, Back above it. One gone meanwhile (unstowed, here or on another device of theirs) closes it.
                let open = self.card.and_then(|c| ks.iter().position(|k| k.capsule == c));
                if open.is_none() {
                    self.card = None;
                }
                if self.sheet.is_some_and(|s| Some(s.capsule) != self.card) {
                    self.sheet = None;
                }
                // The unstow sheet, in the card's place (docs/KISTS.md "The unstow sheet"; owner, 2026-11-12: "ask if the user wants to clear data? and if so, what categories?"): each kind of thing the kist leaves, a tick to keep it, what it comes to; where it goes from; then Unstow, the second of the two taps. What is not kept is erased at once: no bin.
                if let (Some(ki), Some(sh)) = (open, self.sheet) {
                    let k = &ks[ki];
                    button(self, b, &mut y, tr(Msg::Back), false, Some(Choice::UnstowCancel));
                    row(self, b, &mut y, uu(3, 2), white(), &k.name, None);
                    y -= uu(3, 4);
                    row(self, b, &mut y, note, grey(), tr(Msg::UnstowWhatToKeep), None);
                    // What it kept for itself: its own domain (caches, downloaded areas, its settings), with its size as the vault counts it; cleared by default (useless without it, and the bulk).
                    mark(self, b, &mut y, 50, tr(Msg::KeptForItself), sh.keep_data, Some(Choice::UnstowKeep(SHEET_DATA)));
                    y -= uu(3, 4);
                    let l = match self.sheet_size.filter(|s| s.0 == sh.capsule).and_then(|s| s.1) {
                        // Seen on the 8a, 2026-11-12: "nothing yet, of no use without it" says more than there is.
                        Some((_, 0)) => {
                            let mut l = Line::new();
                            l.s(tr(Msg::NothingYet));
                            l
                        }
                        Some((bytes, _)) => {
                            let mut s = Line::new();
                            let _ = tatau::size_bits(&mut s, f, bytes << 3, 1);
                            say(Say::KeptForItself(s.as_str()))
                        }
                        // Not counted yet, or the vault did not say: its size is left out.
                        None => {
                            let mut l = Line::new();
                            l.s(tr(Msg::UselessWithoutIt));
                            l
                        }
                    };
                    row(self, b, &mut y, note, dim(), l.as_str(), None);
                    // What it made for the person (items in the hold, docs/STORE.md): kept by default, theirs. The hold is not built, so the row shows only when there is something (`hold_items`).
                    if hold_items(&k.developer, &k.option) > 0 {
                        mark(self, b, &mut y, 50, tr(Msg::ThingsMadeForYou), sh.keep_made, Some(Choice::UnstowKeep(SHEET_MADE)));
                        y -= uu(3, 4);
                        row(self, b, &mut y, note, dim(), tr(Msg::ThingsMadeYours), None);
                    }
                    // The person's answers: kept by default, so a stow again asks nothing. They are the person's on every device, so from this device only they stay, ticked and fixed.
                    if sh.here_only {
                        mark(self, b, &mut y, 50, tr(Msg::YourAnswers), true, None);
                        y -= uu(3, 4);
                        row(self, b, &mut y, note, dim(), tr(Msg::AnswersOtherDevices), None);
                    } else {
                        mark(self, b, &mut y, 50, tr(Msg::YourAnswers), sh.keep_answers, Some(Choice::UnstowKeep(SHEET_ANSWERS)));
                        y -= uu(3, 4);
                        row(self, b, &mut y, note, dim(), tr(if sh.keep_answers { Msg::AnswersAskNothing } else { Msg::AnswersAskAgain }), None);
                    }
                    // Its notes, always: nothing would be left to open them.
                    row(self, b, &mut y, note, dim(), tr(Msg::NotesGoWithIt), None);
                    y += uu(1, 2);
                    row(self, b, &mut y, note, grey(), tr(Msg::UnstowFrom), None);
                    radios(self, b, &mut y, &[tr(Msg::AllYourDevices), tr(Msg::ThisDeviceOnly)], sh.here_only as usize, &|v| Choice::UnstowFrom(v == 1));
                    row(self, b, &mut y, note, dim(), tr(Msg::GoneMeansGone), None);
                    button(self, b, &mut y, tr(Msg::Unstow), false, Some(Choice::UnstowGo));
                    button(self, b, &mut y, tr(Msg::Cancel), false, Some(Choice::UnstowCancel));
                    self.page_h[p] = y - y0;
                    return;
                }
                if let Some(ki) = open {
                    let k = &ks[ki];
                    let puck = widget(&k.name);
                    button(self, b, &mut y, tr(Msg::Back), false, Some(Choice::CardBack));
                    row(self, b, &mut y, uu(3, 2), white(), &k.name, None);
                    y -= uu(3, 4);
                    row(self, b, &mut y, note, grey(), tr(if puck.is_some() { Msg::KindPuck } else { Msg::KindKist }), None);
                    let st = self.kist_state(&k.capsule);
                    if let Some(icon) = puck {
                        // A puck: what its circle shows (the clock: the time), then its rights and its remove. It is never opened, so it has no state to stop.
                        fact(b, &mut y, tr(Msg::Shows), tr(puck_shows(icon)));
                    } else {
                        // Who made it: the developer's name as their first kist here gave it (the person's pin), and always their key's first bytes, which is who they are.
                        let mut d = Line::new();
                        if let Some(n) = self.kists.pins.iter().find(|q| q.developer == k.developer).and_then(|q| q.name.as_ref()) {
                            let _ = write!(d, "{n} ");
                        }
                        short_key(&mut d, &k.developer);
                        fact_wrap(b, &mut y, tr(Msg::MadeBy), d.as_str());
                        // Its version as its signed declaration writes it, in the person's base, leading zero parts left out.
                        let mut v = Line::new();
                        match k.version.as_ref() {
                            Some(ver) => {
                                let _ = ferros_kist::write_version(&mut v, f, &ver.0);
                            }
                            None => {
                                v.s("-");
                            }
                        }
                        fact(b, &mut y, tr(Msg::FactVersion), v.as_str());
                        // Its capsule's size where it is known: the pandok listing this very capsule, else the job that brought it while the stower still holds it. The person's list keeps no size, and the map holds no right to `capsules.`.
                        let size = inst.dir.iter().find(|e| e.capsule == k.capsule).map(|e| e.size).or_else(|| inst.job_for(&k.capsule).map(|j| j.total).filter(|&t| t != 0));
                        let mut s = Line::new();
                        match size {
                            Some(z) => {
                                let _ = tatau::size_bits(&mut s, f, z << 3, 1);
                            }
                            None => {
                                s.s("-");
                            }
                        }
                        fact(b, &mut y, tr(Msg::Size), s.as_str());
                        // Its identity (KISTS.md: its developer and option, what its answers and its domain are kept by, unchanged by an update), each shortened to its first bytes.
                        let mut id = Line::new();
                        short_key(&mut id, &k.developer);
                        id.s(" ");
                        short_key(&mut id, &k.option);
                        fact_wrap(b, &mut y, tr(Msg::Identity), id.as_str());
                        // Its state as its glow says it, a stop's reason under it; "-" before the kernel has said which kists are alive.
                        fact(b, &mut y, tr(Msg::State), st.map_or("-", |s| tr(s.0)));
                        if let Some((_, Some(why))) = st {
                            row(self, b, &mut y, note, dim(), tr(why_words(why)), None);
                        }
                        // What it keeps: its own domain (the vault under the person's key and its identity). Its size is not told to the map yet; the hold (STORE.md) is later.
                        fact(b, &mut y, tr(Msg::Keeps), tr(Msg::OwnDomain));
                    }
                    // Its rights, each a switch (asked once, ever, per person, per kist, per right: the person's answer, kept for all their devices; switched off, the right is taken back, the kist killed and started again without it), the developer's reason under each, "using now" while it holds a claim on it (only GPS is claimed today). Only the asked ones: the implicit set (ferros_kist::IMPLICIT) is every kist's and has no switch (owner, 2026-11-12, "Yes on the implicit set").
                    y += uu(1, 2);
                    b.text(cx, y, note as u16, dim(), 1, tr(Msg::PagePermissions));
                    y += uu(2, 1);
                    if !k.wants.iter().any(|w| w.asked()) {
                        row(self, b, &mut y, note, dim(), tr(Msg::AsksForNothing), None);
                    }
                    let gps = ferros_sys::claim::GPS;
                    let using = if k.wants.iter().any(|w| w.right as u64 == gps) { self.users(gps) } else { Vec::new() };
                    for (j, w) in k.wants.iter().enumerate().filter(|(_, w)| w.asked()) {
                        let right = right_name(w.right as usize);
                        let l = if w.answer.is_none() {
                            say(Say::NotAskedYet(right))
                        } else if w.right as u64 == gps && using.iter().any(|u| u.as_ref() == Some(&k.capsule)) {
                            say(Say::UsingNow(right))
                        } else {
                            let mut l = Line::new();
                            l.s(right);
                            l
                        };
                        mark(self, b, &mut y, 50, l.as_str(), w.answer.is_some_and(|a| a.0), Some(Choice::Answer(ki as u8, j as u8)));
                        y -= uu(3, 4);
                        row(self, b, &mut y, note, dim(), &w.reason, None);
                    }
                    y += uu(1, 2);
                    // Its actions. A restore or an unstow of it under way, or one that failed, first; while one is under way, no unstow.
                    let job = inst.job_for(&k.capsule).filter(|j| j.stage != stower::DONE);
                    if let Some(j) = job {
                        progress(self, b, &mut y, j);
                    }
                    let busy = job.is_some_and(|j| j.stage < stower::DONE);
                    // Stop, while it runs: one shot dead (init's OP_STOP), no record kept (the person's own act), so it goes to not running.
                    if st.is_some_and(|s| s.0 == Msg::Running) && puck.is_none() {
                        button(self, b, &mut y, tr(Msg::Stop), false, Some(Choice::Stop(ki as u8)));
                    }
                    // Update, when the pandok lists a newer version of the same kist (its identity; the pandok's own rule, `Version`'s order; one stowed before versions takes any listed one), its update's bar while it runs.
                    let newer = inst.dir.iter().position(|e| e.developer == k.developer && e.option == k.option && e.capsule != k.capsule && k.version.as_ref().is_none_or(|v| e.version > *v));
                    if let Some(di) = newer {
                        let e = &inst.dir[di];
                        let up = inst.job_for(&e.capsule).filter(|j| j.stage != stower::DONE);
                        if let Some(j) = up {
                            progress(self, b, &mut y, j);
                        }
                        if !up.is_some_and(|j| j.stage < stower::DONE) {
                            let mut l = Line::new();
                            let _ = write!(l, "{}, ", tr(Msg::Update));
                            let _ = tatau::size_bits(&mut l, f, e.size << 3, 1);
                            button(self, b, &mut y, l.as_str(), false, Some(Choice::StowListed(di as u8)));
                        }
                    }
                    // Unstow in two taps: a kist's opens its unstow sheet (what is kept, from where), whose Unstow is the second; a puck's says what a person sees, off the deck, armed by the first tap (it keeps nothing of its own).
                    if inst.here && !busy {
                        let armed = puck.is_some() && self.unstow_armed == Some(ki as u8);
                        let words = match (puck.is_some(), armed) {
                            (true, false) => Msg::RemoveFromDeck,
                            (true, true) => Msg::TapAgainRemove,
                            (false, _) => Msg::Unstow,
                        };
                        button(self, b, &mut y, tr(words), armed, Some(Choice::Unstow(ki as u8)));
                        if armed {
                            row(self, b, &mut y, note, dim(), tr(Msg::AnswersKept), None);
                        }
                    }
                    // The rest of the page is the list's; the card is all of it while open.
                    self.page_h[p] = y - y0;
                    return;
                }
                // Everything on the deck as a circle, by kind (owner, 2026-11-12: "make sure the settings/items page lists kists and pucks. we may add more stuff to the home screen like contacts"): the kists first, then the pucks (a puck: a live circle never opened, docs/PUCKS.md), each kind a group, room for more kinds (contacts) as more groups. A row is its name, and under it its kind and a kist's state (as its glow says it); the whole of it opens its card.
                if ks.is_empty() {
                    row(self, b, &mut y, small, grey(), tr(Msg::NothingStowed), None);
                }
                for puck in [false, true] {
                    for (ki, k) in ks.iter().enumerate().filter(|(_, k)| widget(&k.name).is_some() == puck) {
                        let what = Some(Choice::Card(ki as u8));
                        row(self, b, &mut y, uu(3, 2), white(), &k.name, what);
                        y -= uu(3, 4);
                        let mut l = Line::new();
                        l.s(tr(if puck { Msg::KindPuck } else { Msg::KindKist }));
                        if let Some((s, _)) = self.kist_state(&k.capsule).filter(|_| !puck) {
                            l.s(", ");
                            l.s(tr(s));
                        }
                        row(self, b, &mut y, note, grey(), l.as_str(), what);
                        // A restore or an unstow of it under way, or one that failed.
                        if let Some(j) = inst.job_for(&k.capsule).filter(|j| j.stage != stower::DONE) {
                            progress(self, b, &mut y, j);
                        }
                        y += uu(1, 2);
                    }
                }
                // The pandok (the directory, kists.fgtw.org; owner, 2026-11-11: "pandok is the place with the things"), under the list as it was on the Kists page before the Deck: every kist listed, to stow, or to update when a newer one is listed than the one stowed.
                if inst.here {
                    y += uu(1, 2);
                    row(self, b, &mut y, uu(3, 2), white(), tr(Msg::Pandok), None);
                    match inst.dir_state {
                        0 | 1 => row(self, b, &mut y, small, grey(), "reading the list of kists", None),
                        3 => row(self, b, &mut y, small, grey(), "the list of kists could not be reached", None),
                        _ if inst.dir.is_empty() => row(self, b, &mut y, small, grey(), "no kists listed yet", None),
                        _ => {}
                    }
                    if inst.dir_state == 2 && !inst.dir_checked {
                        row(self, b, &mut y, note, dim(), "the list's signature is not checked yet", None);
                    }
                    for (di, e) in inst.dir.iter().enumerate() {
                        let mut l = Line::new();
                        // Its version in the person's base, leading zero parts left out: what is shown is what the update below compares.
                        let _ = write!(l, "{} ", e.name);
                        let _ = ferros_kist::write_version(&mut l, f, &e.version.0);
                        row(self, b, &mut y, small, white(), l.as_str(), None);
                        y -= uu(3, 4);
                        row(self, b, &mut y, note, dim(), &e.about, None);
                        let mine = ks.iter().find(|k| k.developer == e.developer && k.option == e.option);
                        let job = inst.job_for(&e.capsule).filter(|j| j.stage != stower::DONE);
                        if let Some(j) = job {
                            progress(self, b, &mut y, j);
                        }
                        // Under way: no button until it is done or has failed.
                        let busy = job.is_some_and(|j| j.stage < stower::DONE);
                        if busy {
                        } else if mine.is_some_and(|k| k.capsule == e.capsule) {
                            row(self, b, &mut y, note, grey(), tr(Msg::Stowed), None);
                        } else if let Some(v) = mine.and_then(|k| k.version.as_ref()).filter(|v| e.version <= **v) {
                            // An update only when the listed version is newer than the one stowed (ferros_kist `Version`'s order: more parts is a later stage, leading zeros mean nothing); one stowed before versions (None) takes any listed one.
                            let mut l = Line::new();
                            let _ = write!(l, "{} ", tr(Msg::Stowed));
                            let _ = ferros_kist::write_version(&mut l, f, &v.0);
                            row(self, b, &mut y, note, grey(), l.as_str(), None);
                        } else {
                            let mut l = Line::new();
                            let _ = write!(l, "{}, ", tr(if mine.is_some() { Msg::Update } else { Msg::Stow }));
                            let _ = tatau::size_bits(&mut l, f, e.size << 3, 1);
                            button(self, b, &mut y, l.as_str(), false, Some(Choice::StowListed(di as u8)));
                        }
                        y += uu(1, 2);
                    }
                }
            }
            "permissions" => {
                // Each right the other way round (owner, 2026-11-08: "who has GPS", "who has the network"), with the person's default for every new kist first: what its switch starts at in a first-launch question (out of the box, off).
                let ks = self.kists.kists.clone();
                // And who is using each right now (owner, 2026-11-10: "we need to be able to see what kists are using each permission"): a kist holding a claim on it says so by its name (`users`). Only GPS is claimed today; a right nothing marks the use of lists no one as using it.
                // Only the asked rights: the implicit set (ferros_kist::IMPLICIT) is every kist's, with no default and no answer to turn (owner, 2026-11-12, "Yes on the implicit set").
                for r in (0..ferros_kist::RIGHTS.len()).filter(|&r| !ferros_kist::implicit(r as u8)) {
                    row(self, b, &mut y, uu(3, 2), white(), right_name(r), None);
                    let on = self.kists.defaults >> r & 1 == 1;
                    mark(self, b, &mut y, 50, tr(Msg::NewKistsStartOn), on, Some(Choice::Default(r as u8)));
                    let using = if r as u64 == ferros_sys::claim::GPS { self.users(r as u64) } else { Vec::new() };
                    for (ki, k) in ks.iter().enumerate() {
                        if let Some(j) = k.wants.iter().position(|w| w.right as usize == r) {
                            let l = self.want_label(k, j, &using);
                            mark(self, b, &mut y, 50, l.as_str(), k.wants[j].answer.is_some_and(|a| a.0), Some(Choice::Answer(ki as u8, j as u8)));
                        }
                    }
                    y += uu(1, 1);
                }
            }
            _ => {
                // Developer: the panel's and the sensors' knobs (the temporal dither moved to Appearance, 2026-11-12); the light scope's channel a radio group.
                // The display's hit map in every fourth frame (owner, 2026-11-11: "a debug/devel checkbox for toggling on/off the hitmap on the fourth frame"); development builds only, as the display's view is.
                if DEV {
                    let on = self.hit_view;
                    mark(self, b, &mut y, 50, tr(Msg::HitMap), on, Some(Choice::HitMap));
                    // The panel's channels (owner, 2026-11-11: "a channel picker in debug that enables/disables red/green/blue of the panel output so I can look at channels"): a checkbox each, all on until a reboot unless changed here; an off channel is code 0 on every pixel the panel gets (display op 63).
                    b.text(cx, y, note as u16, dim(), 1, tr(Msg::PanelChannels));
                    y += uu(2, 1);
                    for (c, name) in [Msg::ScopeRed, Msg::ScopeGreen, Msg::ScopeBlue].into_iter().enumerate() {
                        let on = self.channels >> c & 1 == 1;
                        mark(self, b, &mut y, 50, tr(name), on, Some(Choice::Channel(c as u8)));
                    }
                    // The release look (owner, same day: "a good spot for the release look, we can have it always be off and show orange, upon tap in UI it goes black and stays until a reboot"): off in the development look, its box and its label in the development orange; one tap and the display (and this map's hairlines) take the release look until a reboot (display op 64), the box checked from then on and taking no more taps. Never stored.
                    if DEV_LOOK && LOOK.load(Ordering::Relaxed) {
                        let (sw, colours, _) = mark_look(uu(5, 4));
                        b.mark(50, cx, y - uu(1, 2), uu(5, 4), uu(1, 1), sw, 0, [DEV_ORANGE, colours[1], DEV_ORANGE, colours[3]], tr(Msg::ReleaseLook));
                        if hit {
                            self.push_row(cx - half, cx + half, y - uu(2, 1), y + uu(1, 1), Choice::ReleaseLook);
                        }
                        y += uu(3, 1);
                    } else {
                        mark(self, b, &mut y, 50, tr(Msg::ReleaseLook), true, None);
                    }
                    row(self, b, &mut y, note, grey(), tr(Msg::UntilReboot), None);
                }
                let sc = ferros_sys::pref(ferros_sys::PREF_SCOPE);
                b.text(cx, y, note as u16, dim(), 1, tr(Msg::LightScope));
                y += uu(2, 1);
                radios(self, b, &mut y, &[tr(Msg::Off), tr(Msg::ScopeRed), tr(Msg::ScopeVisible), tr(Msg::ScopeBlue), tr(Msg::ScopeGreen), "IR", tr(Msg::ScopeClear)], sc as usize, &|v| Choice::ScopeSet(v as u8));
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
    /// Where a slider's value puts its knob (0 to 1): a typed setting's by its kind (brightness's stops dimmest on the left), the scale from a half on the left to twice on the right (the pinch's or the drag's while one goes on).
    fn slider_rest(&self, what: Choice) -> f64 {
        match what {
            Choice::Scale => (self.scale_live.unwrap_or(ferros_sys::ui_scale() as f32) as f64 + 64.) / 128.,
            Choice::Typed(i) => {
                let s = typed::ALL[i as usize];
                s.place(s.now())
            }
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
    /// A slider set from the finger at x (a drag across it, or a tap): the knob there, and the value written as it changes. A typed setting (brightness's 32nds of a stop among them) is written live thru the Settings capability each time its value changes; the scale follows the finger live (`scale_live`) and is kept when the finger lifts, as the pinch's is (every process lays itself out again on a scale change, so once, not every frame).
    fn slide_to(&mut self, what: Choice, x: i64) {
        let (a, b) = self.track(self.w / 2);
        // WHY the bounds: the finger runs past either end of the track, and the knob stays at the end it passed.
        let t = ((x - a) as f64 / (b - a) as f64).clamp(0., 1.);
        self.slide = Some((what, t));
        self.knob = None;
        match what {
            Choice::Scale => self.scale_live = Some((t * 128. - 64.) as f32),
            // A typed setting: its byte applied live as it changes (`set_pref_live`), kept at the lift (`slide_done`; owner, 2026-11-08, of brightness: written into the vault at every 32nd of a stop, a commit under the kernel lock each, the settings ran at 20 fps). A kernel from before refuses the live call: then kept at once. The torch's: its level asked of it while it is lit (dark, the level waits for the next light), the rest read again by it (`torch_apply`, only while lit).
            Choice::Typed(i) => {
                let s = typed::ALL[i as usize];
                let v = s.value(t);
                if v != ferros_sys::pref(s.pref) {
                    if ferros_sys::set_pref_live(cap(self.grants, S_SETTINGS), s.pref, v) == ferros_sys::ERR {
                        ferros_sys::set_pref(cap(self.grants, S_SETTINGS), s.pref, v);
                    }
                    if i as usize == typed::TORCH_LEVEL {
                        if quick::torch_now().is_some_and(|l| l > 0) {
                            quick::torch_ask(Some(v));
                        }
                    } else if typed::TORCH.contains(&(i as usize)) {
                        quick::torch_apply();
                    }
                }
            }
            _ => {}
        }
    }
    /// The finger left a slider: its knob goes from where the finger left it to its value's place on the spring, stepped in display frames.
    fn slide_done(&mut self) {
        let Some((what, t)) = self.slide.take() else { return };
        self.typed_kept(what);
        let rest = match what {
            // The scale is kept rounded to its 64ths (the lift writes it).
            Choice::Scale => (libm::round(t * 128. - 64.) + 64.) / 128.,
            _ => self.slider_rest(what),
        };
        let (a, b) = self.track(self.w / 2);
        let off = (t - rest) * (b - a) as f64;
        self.knob = if off.abs() >= 0.5 { Some((what, off, 0., 0)) } else { None };
    }
    /// A zone choice made on the clock page (automatic, and the id by hand; empty: none, a fixed offset): the identity's while this device follows it, thru its identity service (payloads/fgtw op 22: into the person's space, then to every device that follows it, this one first); else this device's own, straight to the kernel. What did not take is said on the page.
    fn zone_choose(&mut self, auto: bool, id: &[u8]) {
        let Some(st) = ferros_sys::zone::state() else { return };
        if st.follow {
            let mut m = Vec::with_capacity(2 + id.len());
            m.push(22);
            m.push(auto as u8);
            m.extend_from_slice(id);
            let mut r = [0u8; 8];
            let n = ferros_sys::call_live(cap(self.grants, S_FGTW), &m, &mut r);
            self.zone_note = match (n, r[0]) {
                (ferros_sys::ERR | ferros_sys::NOTHING | 0, _) => Some(Msg::ServiceDidNotAnswer),
                (_, 0) => None,
                (_, 1) => Some(Msg::FleetNotAnsweredYet),
                _ => Some(Msg::NotAZone),
            };
        } else if !ferros_sys::zone::set_device(cap(self.grants, S_SETTINGS), auto, id) {
            self.zone_note = Some(Msg::KernelDidNotTake);
        }
    }

    /// A language chosen on the language page: the identity's while this device follows it, thru its identity service (payloads/fgtw op 23: into the person's space, then to every device that follows it, this one first); else this device's own, straight to the kernel (SYS_LANG op 0). What did not take is said on the page, as the zone's is.
    fn lang_choose(&mut self, l: Lang) {
        if ferros_sys::lang::follows() {
            let iso = l.iso();
            let m = [23, iso[0], iso[1], iso[2]];
            let mut r = [0u8; 8];
            let n = ferros_sys::call_live(cap(self.grants, S_FGTW), &m, &mut r);
            self.zone_note = match (n, r[0]) {
                (ferros_sys::ERR | ferros_sys::NOTHING | 0, _) => Some(Msg::ServiceDidNotAnswer),
                (_, 0) => None,
                (_, 1) => Some(Msg::FleetNotAnsweredYet),
                _ => Some(Msg::NotALanguage),
            };
        } else if !ferros_sys::lang::set_device(cap(self.grants, S_SETTINGS), Some(l.iso())) {
            self.zone_note = Some(Msg::KernelDidNotTake);
        }
    }

    /// A typed setting as its slider left it, kept in the vault (the drag applied it live); anything else, nothing.
    fn typed_kept(&self, what: Choice) {
        if let Choice::Typed(i) = what {
            let p = typed::ALL[i as usize].pref;
            ferros_sys::set_pref(cap(self.grants, S_SETTINGS), p, ferros_sys::pref(p));
        }
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
            if self.swipe == 1 && matches!(self.row_at(start.1, start.2), Some(Choice::Scale | Choice::Typed(_))) {
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

    /// The display's hit map in every fourth frame switched (display op 58, toggle; development builds), from the Developer page's checkbox or the two-finger swipe up from back and deck: the display's answer kept for the checkbox, and said on the console.
    fn hit_view_toggle(&mut self) {
        let mut r = [0u8; 4];
        if call(self.display, &[58, 2], &mut r) >= 2 && r[0] == 0 {
            self.hit_view = r[1] == 1;
            console(self.grants, if self.hit_view { b"map: hit map on (every fourth frame)\n" } else { b"map: hit map off\n" });
        }
    }

    /// One of the panel's channels switched (display op 63; development builds), from the Developer page's checkboxes: `c` 0 red, 1 green, 2 blue; the display's answer kept for the checkboxes.
    fn channel_toggle(&mut self, c: u8) {
        let mut r = [0u8; 4];
        if call(self.display, &[63, self.channels ^ 1 << c], &mut r) >= 2 && r[0] == 0 {
            self.channels = r[1];
        }
    }

    /// The release look until a reboot (display op 64; development builds), from the Developer page's row: one way, so the row stays checked and takes no more taps.
    fn release_look(&mut self) {
        let mut r = [0u8; 4];
        if call(self.display, &[64, 1], &mut r) >= 2 && r[0] == 0 && r[1] == 1 {
            LOOK.store(false, Ordering::Relaxed);
            console(self.grants, b"map: the release look until a reboot\n");
        }
    }

    /// A helm button's light (display op 54): `corner` 8 deck or 9 back, `level` 0 to 1.
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
        let to = self.deck_fit();
        self.fly(to);
        self.draw();
        "map: back: zoomed out to every circle"
    }

    /// Deck's swipe with an app in front (owner, 2026-11-11, on the Pro: "the swipe in on home whilist in a kist needs to show/hide controls"): the app in front full screen or not (display op 43, as the quick row's middle circle switches it), the kist staying in front. Full screen hides ferros's controls (the console, the helm, the app's circle); the helm's wedges stay the system's on the hit map, undrawn, so this same swipe brings them back, and the console's rows are the app's (owner, 2026-11-11, on the Pro: "helm should be functional but not visible in full screen, whilist console should be gone gone"; display hit.rs `row`), the quick row shut with them. The new state, or None with no app in front (the display's refusal, status 3: its word on what is in front, not the map's).
    fn full_switch(&mut self) -> Option<bool> {
        let mut r = [0u8; 4];
        let n = call(self.display, &[43], &mut r);
        (n >= 2 && n != ferros_sys::ERR && r[0] == 0).then_some(r[1] == 1)
    }

    /// Deck's swipe on the deck (owner, 2026-11-11: "on the home screen it auto-fits"): the plane flies out to every circle at the turn that fits them largest (`deck_turned`), an open circle closing on the way as a tap outside it does.
    fn deck_fly(&mut self) {
        if matches!(self.view, View::Open(_)) {
            self.back = None;
            self.leaving = true;
        }
        let (to, gain, turned) = self.deck_turned();
        let mut l = Line::new();
        let _ = write!(l, "map: deck: the best turn gains A#{} per mille of zoom; turned A#{} mrad\n", libm::round((libm::exp(gain) - 1.) * 1000.) as i64, libm::round(turned * 1000.) as i64);
        console(self.grants, l.as_str().as_bytes());
        self.fly(to);
        self.draw();
    }

    /// A still tap on the helm (WINDOWS.md "The helm"; owner, 2026-11-11). Deck (8): the app in front behind, the map as it was left; with the map on the glass, an open circle closes. Back (9): Escape to the app in front ("back should signal escape to the app and go up one page"; at its main page it does nothing more, never leaving the kist); with the map on the glass, a level back (`back_step`). Never a tap on what lies under the corner.
    fn helm_tap(&mut self, corner: u8) -> &'static str {
        if corner == 8 {
            // A system question waiting (its ring red): straight to it (docs/NOTIFY.md "System questions"), unless it is what the glass shows already.
            if self.question_open() {
                return "map: deck: the question waiting opened";
            }
            if self.send_behind(true) {
                return "map: deck: the app in front behind";
            }
            if matches!(self.view, View::Open(_)) {
                let to = self.back.take().unwrap_or_else(|| self.deck_fit());
                self.leaving = true;
                self.fly(to);
                self.draw();
                return "map: deck: the open circle closed";
            }
            return "map: deck: the map already";
        }
        if self.app_front {
            if ferros_sys::window::key(self.display, ferros_sys::window::KEY_ESCAPE).is_some() {
                return "map: back: Escape to the app in front";
            }
            // The app ended since the map last looked: the map has the glass.
            self.app_front = false;
            self.front_kist = None;
        }
        self.back_step()
    }

    /// A tap: one finger or two, at a panel point. Logged with what it did, so a session can be followed from `diag console`.
    fn tap(&mut self, fingers: usize, sx: i64, sy: i64) {
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
                // A kist stowed or removed since: its circle comes or goes before the tap is placed.
                self.refresh_circles();
                if let Some(i) = self.hit(sx, sy) {
                    // A kist circle: launched by init with the person's answers; a want never answered first opens the circle on the question (KISTS.md). The list read again first: another of the person's devices may have answered meanwhile.
                    if i > 0 {
                        self.reload_kists();
                        let Some(k) = self.kist(i) else { return };
                        let capsule = k.capsule;
                        // A stop it has not said yet (it crashed while the person was elsewhere, `stopped`): this tap opens the circle on that, once, and the next tap launches.
                        if let Some(s) = self.stopped.iter().position(|s| s.0 == capsule) {
                            let (c, why) = self.stopped.remove(s);
                            self.fault = Some((c, why, true));
                        } else {
                            let Some(k) = self.kist(i) else { return };
                            if !k.unanswered() {
                                // A circle glowing for you opens its kist at its latest note (docs/NOTIFY.md "A tap").
                                self.opening = self.latest_note(&capsule);
                                self.launch(i);
                                return;
                            }
                            // Each unanswered switch starts at the person's default for its right.
                            self.ask = k.wants.iter().enumerate().fold(0u64, |a, (j, w)| a | ((self.kists.defaults >> w.right & 1) as u64) << j);
                        }
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
                // The live takeover is modal: a tap outside does not close it (Answer, Decline, the caller's end, or Deck do).
                if !inside && self.live_showing() {
                    return;
                }
                if !inside {
                    // Back to the plane: the page shrinks with the circle as the camera flies back to the view it was opened from (a circle open since the start has none: every circle in view), and the view is the plane once it lands (`step_cam`).
                    let to = self.back.take().unwrap_or_else(|| self.deck_fit());
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
                if !matches!(row, Some(Choice::Unstow(_))) {
                    self.unstow_armed = None;
                }
                // The clock's (or the language's) word on a choice that did not take stays until the next tap.
                self.zone_note = None;
                let set = |m: &Map, pref: u64, v: u8| {
                    ferros_sys::set_pref(cap(m.grants, S_SETTINGS), pref, v);
                };
                match row {
                    Some(Choice::Base(code)) => set(self, ferros_sys::PREF_BASE, code),
                    // A slider tapped: its knob and its value there at once, as a drag would leave them; the scale is kept now (the lift that wrote a drag's has passed).
                    Some(c @ (Choice::Scale | Choice::Typed(_))) => {
                        self.slide_to(c, sx);
                        self.slide = None;
                        self.typed_kept(c);
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
                    // The torch on at its level (and its strobe and timeout, which it reads itself) or off (its quick circle's tap until 2026-11-12, which opens this page now); one not heard from yet is asked to light (its first call starts it).
                    Some(Choice::LiveAnswer) => {
                        self.live_answer(true);
                        return;
                    }
                    Some(Choice::LiveDecline) => {
                        self.live_answer(false);
                        return;
                    }
                    Some(Choice::TorchOn) => quick::torch_ask(Some(if quick::torch_now().is_some_and(|l| l > 0) { 0 } else { ferros_sys::torch::level() })),
                    Some(Choice::Theme(i)) => set(self, ferros_sys::PREF_THEME, i),
                    Some(Choice::Dither) => set(self, ferros_sys::PREF_DITHER, (ferros_sys::pref(ferros_sys::PREF_DITHER) != 1) as u8),
                    Some(Choice::HitMap) => self.hit_view_toggle(),
                    Some(Choice::Channel(c)) => self.channel_toggle(c),
                    Some(Choice::ReleaseLook) => self.release_look(),
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
                    Some(Choice::DeckSync) => {
                        let on = self.fleet.deck.is_some_and(|h| h.0);
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
                    // Attest with a tag: an add started (the next tag held to the device is kept, `tag_add_thread`), or the one waiting cancelled.
                    Some(Choice::TagAdd) => {
                        if TAG_ADD.load(core::sync::atomic::Ordering::Acquire) != 0 {
                            self.tag_stop();
                        } else {
                            self.tag_adds += 1;
                            self.tag_said = 0;
                            TAG_ADD.store(self.tag_adds, core::sync::atomic::Ordering::Release);
                            ferros_sys::wake(&TAG_ADD, 1);
                        }
                    }
                    // A kept tag forgotten (one tap: nothing of the identity goes with it, and the tag can be kept again).
                    Some(Choice::TagRemove(i)) => {
                        if ferros_sys::forget_tag(cap(self.grants, S_SETTINGS), Some(&self.tags[i as usize].id)).is_none() {
                            console(self.grants, b"map: the kernel did not forget the tag\n");
                        }
                        self.tag_said = 0;
                        self.tags_read();
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
                    // The zone (docs/TIME.md): who rules, automatic or by hand, and the picker's levels; a zone picked or none closes it.
                    Some(Choice::ZoneFollow) => {
                        if let Some(st) = ferros_sys::zone::state() {
                            if !ferros_sys::zone::set_follow(cap(self.grants, S_SETTINGS), !st.follow) {
                                self.zone_note = Some(Msg::KernelDidNotTake);
                            }
                        }
                    }
                    // The language (SETTINGS.md): whose choice rules, and a language picked (the identity's while following it, else this device's).
                    Some(Choice::LangFollow) => {
                        if !ferros_sys::lang::set_follow(cap(self.grants, S_SETTINGS), !ferros_sys::lang::follows()) {
                            self.zone_note = Some(Msg::KernelDidNotTake);
                        }
                    }
                    Some(Choice::Lang(i)) => self.lang_choose(Lang::from_index(i)),
                    Some(Choice::ZoneAuto(auto)) => {
                        if let Some(st) = ferros_sys::zone::state() {
                            let id = st.ruling().1;
                            self.zone_choose(auto, id.as_bytes());
                        }
                    }
                    Some(Choice::ZonePicker) => {
                        self.zone_pick = ZonePick::load();
                        if self.zone_pick.is_none() {
                            self.zone_note = Some(Msg::NoZonesYet);
                        }
                        self.scroll = 0;
                    }
                    Some(Choice::ZoneBack) => {
                        match self.zone_pick.as_mut() {
                            Some(p) if p.region.is_some() => p.region = None,
                            _ => self.zone_pick = None,
                        }
                        self.scroll = 0;
                    }
                    Some(Choice::ZoneRegion(r)) => {
                        if let Some(p) = self.zone_pick.as_mut() {
                            p.region = Some(r as usize);
                        }
                        self.scroll = 0;
                    }
                    Some(c @ (Choice::ZoneCity(_) | Choice::ZoneNone)) => {
                        let id: Vec<u8> = match (c, self.zone_pick.as_ref()) {
                            (Choice::ZoneCity(k), Some(p)) => p.id(k as usize).as_bytes().to_vec(),
                            _ => Vec::new(),
                        };
                        // Picked by hand: the choice keeps whether it is automatic (the zone picked then stands in until one is found).
                        let auto = ferros_sys::zone::state().is_some_and(|s| s.ruling().0);
                        self.zone_choose(auto, &id);
                        self.zone_pick = None;
                        self.scroll = 0;
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
                    // An answer turned over on a kist's card or the Permissions page (never answered reads as off: one tap turns it on): kept in the person's list, and the kist, if running, killed and launched again with it.
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
                    // Unstow in two taps (as "remove this device"; a puck's "remove from the deck" the same): the first arms it, the second asks the stower (op 4), which takes it out of the person's list, keeps its answers and drops its capsule; its circle goes when the job is done, and the card with it.
                    // A kist's Unstow: its sheet, in the card's place, at its top, at the design's defaults; what it kept for itself counted by the stower meanwhile (stower.rs `size_ask`, drawn when it comes).
                    Some(Choice::Unstow(k)) if self.kists.kists.get(k as usize).is_some_and(|kist| widget(&kist.name).is_none()) => {
                        let kist = &self.kists.kists[k as usize];
                        self.sheet = Some(Sheet { capsule: kist.capsule, keep_data: false, keep_made: true, keep_answers: true, here_only: false });
                        if self.sheet_size.is_none_or(|s| s.0 != kist.capsule) {
                            self.sheet_size = None;
                        }
                        stower::size_ask(&kist.capsule, &kist.developer, &kist.option);
                        self.scroll = 0;
                    }
                    Some(Choice::UnstowKeep(r)) => {
                        if let Some(s) = self.sheet.as_mut() {
                            match r {
                                SHEET_DATA => s.keep_data = !s.keep_data,
                                SHEET_MADE => s.keep_made = !s.keep_made,
                                _ => s.keep_answers = !s.keep_answers,
                            }
                        }
                    }
                    Some(Choice::UnstowFrom(here)) => {
                        if let Some(s) = self.sheet.as_mut() {
                            s.here_only = here;
                        }
                    }
                    Some(Choice::UnstowCancel) => {
                        self.sheet = None;
                        self.scroll = 0;
                    }
                    // The sheet's Unstow, the second tap: a running kist stopped first (init's OP_STOP, one shot dead), so nothing writes its domain while it goes; then the stower's op 4 with the person's choices; its notes cleared here (docs/NOTIFY.md "Seen": nothing is left to open them). The card goes when the job is done (here only: when the list read again leaves it off this device), and the sheet with it.
                    Some(Choice::UnstowGo) => {
                        let sh = self.sheet.take();
                        if let Some((sh, kist)) = sh.and_then(|sh| self.kists.kists.iter().find(|k| k.capsule == sh.capsule).cloned().map(|k| (sh, k))) {
                            if self.kist_state(&kist.capsule).is_some_and(|s| s.0 == Msg::Running) {
                                let mut m = [0u8; 33];
                                m[0] = ferros_kist::launch::OP_STOP;
                                m[1..].copy_from_slice(&kist.capsule);
                                let mut r = [0u8; 1];
                                let n = call(cap(self.grants, S_LAUNCH), &m, &mut r);
                                if n == ferros_sys::ERR || n < 1 || (r[0] != ferros_kist::launch::STOPPED && r[0] != ferros_kist::launch::NOT_RUNNING) {
                                    console(self.grants, b"map: init did not stop the kist before its unstow\n");
                                }
                            }
                            let mut m = [0u8; 66];
                            m[0] = 4;
                            m[1..33].copy_from_slice(&kist.developer);
                            m[33..65].copy_from_slice(&kist.option);
                            m[65] = !sh.keep_data as u8 | ((!sh.keep_answers && !sh.here_only) as u8) << 1 | (sh.here_only as u8) << 2;
                            if stower::ask(self.grants, &m) != Some(0) {
                                console(self.grants, b"map: the stower did not take the unstow\n");
                            } else {
                                ferros_sys::notify::seen(cap(self.grants, S_SETTINGS), &kist.capsule, ferros_sys::notify::ALL, false);
                            }
                        }
                        self.scroll = 0;
                    }
                    Some(Choice::Unstow(k)) => {
                        if self.unstow_armed == Some(k) {
                            self.unstow_armed = None;
                            if let Some(kist) = self.kists.kists.get(k as usize) {
                                let mut m = [0u8; 65];
                                m[0] = 4;
                                m[1..33].copy_from_slice(&kist.developer);
                                m[33..].copy_from_slice(&kist.option);
                                if stower::ask(self.grants, &m) != Some(0) {
                                    console(self.grants, b"map: the stower did not take the unstow\n");
                                } else {
                                    // Its notes go with it (docs/NOTIFY.md "Seen"): nothing of a kist the person put away waits on them.
                                    ferros_sys::notify::seen(cap(self.grants, S_SETTINGS), &kist.capsule, ferros_sys::notify::ALL, false);
                                }
                            }
                        } else {
                            self.unstow_armed = Some(k);
                        }
                    }
                    // A row on the Deck: its card, at its top; Back: the list, at its top.
                    Some(Choice::Card(k)) => {
                        self.card = self.kists.kists.get(k as usize).map(|kist| kist.capsule);
                        self.scroll = 0;
                    }
                    Some(Choice::CardBack) => {
                        self.card = None;
                        self.scroll = 0;
                    }
                    // A kist's Stop: init ends its running process (OP_STOP), one shot dead; the kists' bell rings as it ends, and its state follows on the card and its circle (`kists_rang`).
                    Some(Choice::Stop(k)) => {
                        if let Some(kist) = self.kists.kists.get(k as usize) {
                            let mut m = [0u8; 33];
                            m[0] = ferros_kist::launch::OP_STOP;
                            m[1..].copy_from_slice(&kist.capsule);
                            let mut r = [0u8; 1];
                            let n = call(cap(self.grants, S_LAUNCH), &m, &mut r);
                            if n == ferros_sys::ERR || n < 1 || (r[0] != ferros_kist::launch::STOPPED && r[0] != ferros_kist::launch::NOT_RUNNING) {
                                console(self.grants, b"map: init did not take the stop\n");
                            }
                        }
                    }
                    // Stow or update the directory's kist: the stower fetches, checks and seals it, and the page shows each stage as its doorbell rings.
                    Some(Choice::StowListed(d)) => {
                        if let Some(row) = self.stower.dir.get(d as usize) {
                            let mut m = [0u8; 33];
                            m[0] = 3;
                            m[1..].copy_from_slice(&row.capsule);
                            if stower::ask(self.grants, &m) != Some(0) {
                                console(self.grants, b"map: the stower did not take the stow\n");
                            }
                        }
                    }
                    // The person's answer to a kist offered over USB: the stower stows it in their list or drops it, and the host that offered it hears which (docs/KISTS.md "Stowing").
                    Some(Choice::Stow(stow)) => {
                        if let Some(o) = self.stower.offer.as_ref() {
                            let mut m = [0u8; 34];
                            m[0] = 5;
                            m[1..33].copy_from_slice(&o.capsule);
                            m[33] = stow as u8;
                            if stower::ask(self.grants, &m) != Some(0) {
                                console(self.grants, b"map: the stower did not take the answer to the offer\n");
                            }
                        }
                    }
                    Some(Choice::Default(r)) => {
                        self.reload_kists();
                        self.kists.defaults ^= 1 << r;
                        self.save_kists();
                    }
                    Some(Choice::AskWant(j)) => self.ask ^= 1 << j,
                    // The question answered: every asked want it showed gets the switch's answer, kept for the person (asked once, ever); then the circle closes and init launches the kist.
                    Some(Choice::AskGo) => {
                        if let View::Open(i) = self.view {
                            let ask = self.ask;
                            let now = ferros_detail::eagle_now();
                            if let Some(k) = i.checked_sub(1).and_then(|k| self.kists.kists.get_mut(k)) {
                                for (j, w) in k.wants.iter_mut().enumerate() {
                                    if w.asked() && w.answer.is_none() {
                                        w.answer = Some((ask >> j & 1 == 1, now));
                                    }
                                }
                            }
                            if self.save_kists() {
                                let to = self.back.take().unwrap_or_else(|| self.deck_fit());
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
    // The deck's start by part, to the console with "deck drawn" (2026-11-12, measuring attest to the deck): [the display's size, the person's kists read, the places kept, the scene sent, the view kept, the first draw], counter ticks at each end.
    let mut t_start = [now(); 7];
    let display = cap(grants, S_DISPLAY);
    let touch = cap(grants, S_TOUCH);
    let mut r = [0u8; 256];

    // The panel's size, from the display server.
    if call(display, &[5], &mut r) < 4 {
        console(grants, b"map: no display server\n");
        ferros_sys::exit(1);
    }
    let (w, h) = (u16::from_le_bytes([r[0], r[1]]) as i64, u16::from_le_bytes([r[2], r[3]]) as i64);
    t_start[1] = now();

    // The circles: settings, then a kist circle for each kist the person stowed (their list, `kists.list`; none readable, none shown), sized by share and placed once.
    let kists = ferros_kist::store::read_here(cap(grants, S_KIST_LIST)).unwrap_or_default();
    t_start[2] = now();
    // WHY: the scene and the arrays hold MAX circles; kists past it get no circle until there is room (the settings' Deck page still lists them).
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
        flight_trace: alloc::string::String::new(),
        taps: 0,
        armed: false,
        dev: false,
        hit_view: false,
        channels: 7,
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
        recent: alloc::vec::Vec::new(),
        front_kist: None,
        n,
        kists,
        ask: 0,
        zone_pick: None,
        zone_note: None,
        stower: stower::Stower::default(),
        gps_claimed: false,
        gps_fix: None,
        tags: [ferros_sys::KeptTag::default(); 1 << 4],
        ntags: 0,
        tag_said: 0,
        tag_adds: 0,
        unstow_armed: None,
        card: None,
        sheet: None,
        sheet_size: None,
        asked: None,
        fault: None,
        stopped: alloc::vec::Vec::new(),
        kists_read: 0,
        live: None,
        ended: alloc::vec::Vec::new(),
        waiting: alloc::vec::Vec::new(),
        sent: [(0, 0); MAX],
        notes: Vec::new(),
        notes_read: 0,
        notes_due: None,
        rest: Cam { x: 0, y: 0, z: Z_ONE, c: ONE, s: 0 },
        tapped: Vec::new(),
        helm_sent: (0, 0, 0),
        questions: Vec::new(),
        live_note: None,
        live_words: None,
        opening: None,
        open_pending: None,
    };
    // The display's hit map view as it stands (op 58, ask), for the Developer page's checkbox: a map started again finds it as it was left.
    if DEV {
        let mut r = [0u8; 4];
        map.hit_view = call(map.display, &[58, 3], &mut r) >= 2 && r[0] == 0 && r[1] == 1;
        // The panel's channels and the look as the display has them (ops 59 and 60, ask): a map started again finds them as they were left.
        if call(map.display, &[63, 0x80], &mut r) >= 2 && r[0] == 0 {
            map.channels = r[1];
        }
        if call(map.display, &[64, 0], &mut r) >= 2 && r[0] == 0 && r[1] == 1 {
            LOOK.store(false, Ordering::Relaxed);
        }
    }
    let kept = map.load_places_near();
    t_start[3] = now();
    // Whatever the vault held, no two circles start overlapping (places kept before circles pushed each other could).
    map.push_apart(MAX);
    if kept > 0 {
        let mut l = Line::new();
        let _ = write!(l, "map: {kept} circle(s) where the owner put them\n");
        console(grants, l.as_str().as_bytes());
    }
    map.send_scene();
    map.cam = map.deck_fit();
    t_start[4] = now();
    if map.load_view() {
        console(grants, b"map: the view as it was left\n");
    }
    t_start[5] = now();
    // Developer, once the twelve taps made it a page (release builds; always one on development builds).
    let mut dv = [0u8; 4];
    map.dev = matches!(ferros_sys::store_get(cap(grants, S_STORE), b"dev", &mut dv), Some(n) if n >= 1 && dv[0] == 1);
    let mut saved_view = map.cam;
    // The deck's resting view starts as the view it was left at.
    map.rest = map.cam;
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
    t_start[6] = now();
    // Nothing is in focus (display op 25, icon 0): a display that outlived an older map, one with widgets that took focus, would otherwise keep redrawing one at the panel's rate.
    call(display, &[25, 0, 0, 0, 0, 0, 0], &mut r);
    // A development build can time the frame loop without a finger (FERROS_MAP_START=bench): 32 frames of a pinch with a twist, then 32 of a pan, each the whole round trip to the display server; the averages to the console, then the camera back at rest.
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
    // FERROS_MAP_START=zoom (development, measuring the scene while it grows): two seconds after start, 128 display frames (held four seconds at the largest, for a capture) of a pinch with a twist from the deck's view, the settings circle and its mark growing past the screen, each frame drawn moving and paced by the panel (op 24), then the view back at rest; the frames the panel showed to the console.
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
    {
        let us = |a: u64, b: u64| (b - a) * 1_000_000 / freq();
        let mut l = Line::new();
        let _ = write!(l, "map: deck drawn {} us after start (display {}, kists list {}, places {}, scene {}, view {}, draw {} us); a tap on a circle opens it\n", us(t_start[0], t_start[6]), us(t_start[0], t_start[1]), us(t_start[1], t_start[2]), us(t_start[2], t_start[3]), us(t_start[3], t_start[4]), us(t_start[4], t_start[5]), us(t_start[5], t_start[6]));
        console(grants, l.as_str().as_bytes());
    }
    // FERROS_MAP_START=crowd: 200 more circles in the display's scene only (never the vault), for timing a busy deck.
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
    // A session that began on one of the helm's corner circles (the display's op 41: 8 deck, 9 back; 0 none), and where its first finger is now: the system's gesture, never a pan (owner, 2026-11-07: from the deck circle to the centre and let go, full screen on or off).
    let mut corner = 0u8;
    let mut corner_at = (0i64, 0i64);
    // Where that finger landed, and whether the corner has acted this touch.
    let mut corner_from = (0i64, 0i64);
    let mut corner_done = false;
    // Whether the helm's finger went further than a roll from where it landed (`HELM_ROLL`): then its lift is no tap.
    let mut corner_far = false;
    // Two fingers on back and deck together (development builds): 0 not looked at yet this touch, 1 they are, 2 they are not; and whether their swipe up has switched the hit map this touch.
    let mut pair = 0u8;
    let mut pair_done = false;
    // The quick row (quick.rs): its slide, physics, the rings' drags and the taps; and whether the session's finger landed in its doorway by the camera (a still tap there opens or shuts it).
    let mut qr = quick::Quick::new(display, grants, w, h);
    let mut quick_door = false;
    let mut start = [(false, 0i64, 0i64); 16];
    // The circle the fingers hold, decided again whenever the number of moving fingers changes (one finger: the circle it started on; two: the circle both are inside), and for how many fingers it was decided.
    let mut grab: Option<usize> = None;
    let mut grab_n = 0usize;
    let mut last_app_check = 0u64;
    // The bells as the pages were last drawn with them (BELLS), the radios' switches (preference 15) likewise, and the second the clock or About last drew (`tick`).
    let mut bells_seen = [0u64; ferros_sys::BELL_FACTS as usize + 1];
    // The display's frame count as last read on a facts bell: a smaller one means the display restarted.
    let mut display_frames_seen = 0u32;
    let mut radios_seen = ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF);
    // The torch as last drawn: its answer and its four settings (preferences 42-45).
    const TORCH_PREFS: [u64; 4] = [ferros_sys::PREF_TORCH_LEVEL, ferros_sys::PREF_TORCH_STROBE, ferros_sys::PREF_TORCH_DUTY, ferros_sys::PREF_TORCH_TIMEOUT];
    let mut torch_seen = (quick::torch_now(), TORCH_PREFS.map(ferros_sys::pref));
    let mut tick_seen = 0i64;
    // The identity space's generation last read (payloads/fgtw op 17, through `fgtw_poller`).
    let mut space_seen = 0u32;
    // The departure waiting here last seen (its intent and the asking device's name), so the Fleet page opens once for each: a request answered clears it, so the same device asking again opens the page again.
    let mut ask_seen: Option<(u8, [u8; NAME_BYTES], usize)> = None;
    // The kist offered over USB last opened the Deck page on (docs/KISTS.md "Stowing").
    let mut offer_seen: Option<[u8; 32]> = None;
    // Whether the console was told that offer waits on an app in front (once an offer).
    let mut offer_held_said = false;
    // The copy of the identity service last taken (`fgtw_poller`'s sequence count).
    let mut fleet_seen = 0u32;
    // Whether the Fleet page showed at the last pass (`FLEET_PAGE`).
    let mut fleet_shown = false;
    // The stower's copy last taken, the finished jobs in it (a kist stowed or unstowed: the circles follow), and whether the Deck page showed at the last pass (opening it reads the directory again).
    let (mut stower_seen, mut stower_done, mut kists_shown) = (0u32, 0u64, false);
    // The sizer's answer last taken (stower.rs `size_take`).
    let mut size_seen = 0u32;
    // GPS's claims as the permissions and GPS pages were last drawn with them (CLAIMS_WORD, the kernel's doorbell): a change draws them again.
    let mut claims_seen = 0u64;
    // `gps_watch`'s copy last taken (its sequence count).
    let mut gps_seen = 0u32;
    // Whether the GPS page showed (with GPS on and the screen on) at the last pass: its claim follows it.
    let mut gps_shown = false;
    // Whether the NFC page showed last pass (its kept tags read as it comes, an add ended as it goes), and TAG_DONE as last taken.
    let mut nfc_shown = false;
    let mut tag_done_seen = 0u64;
    let mut last_pass = now();
    let mut draw_us = 0u64;
    let mut move_us = 0u64;
    let mut last_ignored = 0u16;
    let mut push_us = 0u64;
    // The identity service's answers come from a thread of their own (`fgtw_poller`): the main loop never waits on fgtw.
    static mut POLL_STACK: [u8; 1 << 14] = [0; 1 << 14];
    ferros_sys::thread_spawn(fgtw_poller, unsafe { &mut *(&raw mut POLL_STACK) }, grants);
    // The add flow following the Fleet page (`fleet_page_thread`).
    static mut PAGE_STACK: [u8; 1 << 14] = [0; 1 << 14];
    ferros_sys::thread_spawn(fleet_page_thread, unsafe { &mut *(&raw mut PAGE_STACK) }, grants);
    // The stower's doorbell (payloads/kists), for the Deck page: on a thread of its own, never on a timer.
    static mut STOWER_STACK: [u8; 1 << 14] = [0; 1 << 14];
    ferros_sys::thread_spawn(stower::watch, unsafe { &mut *(&raw mut STOWER_STACK) }, grants);
    // The unstow sheet's size of what a kist kept for itself (stower.rs `sizer`): asleep until a sheet opens.
    static mut SIZER_STACK: [u8; 1 << 14] = [0; 1 << 14];
    ferros_sys::thread_spawn(stower::sizer, unsafe { &mut *(&raw mut SIZER_STACK) }, grants);
    // The GPS driver's doorbell, for the GPS page (`gps_watch`): on a thread of its own, asleep while the page is not open.
    static mut GPS_STACK: [u8; 1 << 14] = [0; 1 << 14];
    ferros_sys::thread_spawn(gps_watch, unsafe { &mut *(&raw mut GPS_STACK) }, grants);
    // The NFC page's "add a tag" (`tag_add_thread`): asleep until an add starts.
    static mut TAG_STACK: [u8; 1 << 14] = [0; 1 << 14];
    ferros_sys::thread_spawn(tag_add_thread, unsafe { &mut *(&raw mut TAG_STACK) }, grants);
    // The quick row's slow calls (the torch, the identity service's do not disturb), on a thread of their own (quick.rs `worker`).
    static mut QUICK_STACK: [u8; 1 << 14] = [0; 1 << 14];
    ferros_sys::thread_spawn(quick::worker, unsafe { &mut *(&raw mut QUICK_STACK) }, grants);
    // The claims' doorbell (SYS_CLAIM op 4): the kernel writes GPS's claims into CLAIMS_WORD at every change. A kernel from before refuses it, and "using now" is then read only as a page draws.
    if !ferros_sys::claim::doorbell(cap(grants, S_SETTINGS), &CLAIMS_WORD) {
        console(grants, b"map: the kernel has no claims doorbell\n");
    }
    // The kists' bell (SYS_NOT_STARTED op 1): a launch init answered "started" that the kernel did not start after all. A kernel from before refuses it, and such a tap shows nothing, as before.
    if !ferros_sys::not_started::doorbell(cap(grants, S_SETTINGS), &KISTS_WORD) {
        console(grants, b"map: the kernel has no kists' bell\n");
    }
    // The notes' bell (SYS_NOTIFY op 4, docs/NOTIFY.md): a note posted, withdrawn, seen or past its deadline. A kernel from before refuses it, and nothing glows for you.
    if !ferros_sys::notify::doorbell(cap(grants, S_SETTINGS), &notes::WORD) {
        console(grants, b"map: the kernel has no notes' bell\n");
    }
    // The system's questions are this map's (notes.rs `questions_sync`): any a map before this one posted (it ended, or restarted) go, and whatever still waits is posted again from what this map holds.
    ferros_sys::notify::seen(cap(grants, S_SETTINGS), &ferros_kist::note::SYSTEM, ferros_sys::notify::ALL, false);
    let mut notes_seen = notes::WORD.load(core::sync::atomic::Ordering::Acquire);
    // What waits already (posted before this map started, or kept from before the boot) glows at once; it taps and takes over nothing: every note's bell count is at most the bell's now, where `notes_read` starts.
    map.notes_read = notes_seen;
    map.notes_rang();
    let mut kists_seen = KISTS_WORD.load(core::sync::atomic::Ordering::Acquire);
    // What the kernel kept before this map started is not this map's to say.
    map.kists_read = kists_seen;
    // The kist circles' states as they stand (`kists_rang` says nothing kept before this map started): a map started again finds a running kist running and a stopped one stopped.
    map.kists_rang();
    // The links' and the facts' bells (SYS_BELL): the kernel writes each source's change count into BELLS at every change. A kernel from before refuses them, and those pages then draw only as they open or are touched.
    for (s, w) in BELLS.iter().enumerate() {
        if !ferros_sys::bell(s as u64, w) {
            console(grants, b"map: the kernel has no bells\n");
            break;
        }
    }
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
        // The Fleet page shown or gone: the add flow follows it (`fleet_page_thread`), so a device can be added by a touch on its row or by NFC whenever the page is open.
        let shown = matches!(map.view, View::Open(_)) && PAGES[map.page] == "fleet";
        if shown != fleet_shown {
            fleet_shown = shown;
            let v = FLEET_PAGE.load(core::sync::atomic::Ordering::Acquire);
            FLEET_PAGE.store(((v >> 1) + 1) << 1 | shown as u64, core::sync::atomic::Ordering::Release);
            ferros_sys::wake(&FLEET_PAGE, 1);
        }
        // The stower's copy, when its doorbell has rung (stower.rs): taken in, the Deck page drawn again while it shows, and the person's list and circles read again when a job has finished (a kist stowed, updated, unstowed or restored).
        if let Some((seq, s)) = stower::take(stower_seen) {
            stower_seen = seq;
            let done = s.finished();
            map.stower = s;
            if done != stower_done {
                stower_done = done;
                map.refresh_circles();
            }
            if matches!(map.view, View::Open(_)) && PAGES[map.page] == "deck" && !map.settling {
                map.draw();
            }
        }
        // What the sheet's kist kept for itself, once the stower has counted it: drawn while the sheet shows.
        if let Some((seq, capsule, size)) = stower::size_take(size_seen) {
            size_seen = seq;
            map.sheet_size = Some((capsule, size));
            if map.sheet.is_some_and(|s| s.capsule == capsule) && matches!(map.view, View::Open(_)) && PAGES[map.page] == "deck" && !map.settling {
                map.draw();
            }
        }
        // The Deck page opened: the stower reads the pandok (the directory) again (owner: on opening the page, never on a timer).
        let shown = matches!(map.view, View::Open(_)) && PAGES[map.page] == "deck";
        if shown != kists_shown {
            kists_shown = shown;
            if shown {
                let _ = stower::ask(grants, &[1]);
            }
        }
        // The GPS page holds a GPS claim while it shows (owner, 2026-11-10: "open the GPS screen (which should turn it on whilist GPS screen is open)"), GPS switched on and the screen on: the kernel starts the driver for the first claim and stops it after the last (SYS_CLAIM). Claimed as the page comes, released as it goes; the switch turned off drops the claim in the kernel, and turned on again the page claims anew.
        let shown = matches!(map.view, View::Open(_)) && PAGES[map.page] == "GPS" && ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF) >> 4 & 1 == 0 && ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) != 1;
        if shown != gps_shown {
            gps_shown = shown;
            if shown {
                map.gps_claimed = ferros_sys::claim::claim(ferros_sys::claim::GPS, cap(grants, S_SETTINGS));
                // Refused (no person present, which the map never runs without): not asked again until the page shows again.
                if !map.gps_claimed {
                    console(grants, b"map: the GPS page's claim was refused\n");
                }
            } else {
                ferros_sys::claim::release(ferros_sys::claim::GPS);
                map.gps_claimed = false;
            }
            // The doorbell thread follows the claim (`gps_watch`); the page starts at "starting" until the driver rings.
            map.gps_fix = None;
            let v = GPS_PAGE.load(core::sync::atomic::Ordering::Acquire);
            GPS_PAGE.store(((v >> 1) + 1) << 1 | map.gps_claimed as u64, core::sync::atomic::Ordering::Release);
            ferros_sys::wake(&GPS_PAGE, 1);
            if !map.settling {
                map.draw();
            }
        }
        // Attest with a tag: the NFC page's kept tags read as it comes; an add waiting ends as it goes (reader mode only while the page shows, AUTH.md).
        let shown = matches!(map.view, View::Open(_)) && PAGES[map.page] == "NFC";
        if shown != nfc_shown {
            nfc_shown = shown;
            map.tag_said = 0;
            if shown {
                map.tags_read();
            } else if TAG_ADD.load(core::sync::atomic::Ordering::Acquire) != 0 {
                map.tag_stop();
            }
            if !map.settling {
                map.draw();
            }
        }
        // An add's tag was kept, or not (`tag_add_thread` wrote TAG_DONE): one tag an add, its outcome said, the list read again.
        let done = TAG_DONE.load(core::sync::atomic::Ordering::Acquire);
        if done != tag_done_seen {
            tag_done_seen = done;
            if done >> 2 == TAG_ADD.load(core::sync::atomic::Ordering::Acquire) {
                map.tag_said = done & 3;
                TAG_ADD.store(0, core::sync::atomic::Ordering::Release);
                map.tags_read();
                if !map.settling {
                    map.draw();
                }
            }
        }
        // What the open page shows moved: drawn again once this pass, whichever of its doorbells rang (the GPS driver's and its link's together draw once). While the page settles it is drawn every frame anyway.
        let open = matches!(map.view, View::Open(_));
        let page = PAGES[map.page];
        let mut page_dirty = false;
        // The driver rang (`gps_watch` published what it read): taken in, and the GPS page drawn again while it holds its claim.
        if let Some((seq, v)) = gps_take(gps_seen) {
            gps_seen = seq;
            if map.gps_claimed {
                map.gps_fix = v;
                page_dirty |= open && page == "GPS";
            }
        }
        // GPS's claims moved (the kernel wrote CLAIMS_WORD): the pages that say who is using it drawn again while they show.
        let c = CLAIMS_WORD.load(core::sync::atomic::Ordering::Acquire);
        if c != claims_seen {
            claims_seen = c;
            page_dirty |= open && matches!(page, "permissions" | "GPS");
        }
        // A link's bell (its measures, tracks or detail moved; SYS_BELL): its page and Connections drawn again while they show, the GPS page among them, since its driver publishes the satellites and the accuracy after the fix messages its own doorbell answers to. The facts' bell (the battery, the processes, the vault's keys, the chip's heat): About.
        for (s, w) in BELLS.iter().enumerate() {
            let v = w.load(core::sync::atomic::Ordering::Acquire);
            if v != bells_seen[s] {
                bells_seen[s] = v;
                page_dirty |= open && if s < LINKS.len() { page == "connections" || page == LINKS[s] } else { matches!(page, "about" | "battery") };
                // The facts' bell also rings when the process table changes: a display that restarted (its frame count, op 5 bytes 4-7, starts again from zero) has no scene, and showed black until a touch woke the map (owner, 2026-11-11: "it showed black until I touched it"); so the scene goes again at once.
                if s == ferros_sys::BELL_FACTS as usize {
                    let mut q = [0u8; 16];
                    if call(map.display, &[5], &mut q) >= 8 {
                        let frames = u32::from_le_bytes([q[4], q[5], q[6], q[7]]);
                        if frames < display_frames_seen {
                            console(grants, b"map: the display started again; its scene sent anew\n");
                            map.send_scene();
                            map.draw();
                        }
                        display_frames_seen = frames;
                    }
                }
            }
        }
        // A radio switched on or off (preference 15, from here or from a status circle): its switch and state on Connections and on its page.
        let radios = ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF);
        if radios != radios_seen {
            radios_seen = radios;
            page_dirty |= open && (page == "connections" || LINKS.contains(&page));
        }
        // The torch answered (lit, dark, or put out by its timeout and asked again) or its settings moved (here, or from the bridge): its page drawn again while it shows. Its answer comes from the quick row's worker thread, read here as every pass reads it.
        let torch = (quick::torch_now(), TORCH_PREFS.map(ferros_sys::pref));
        if torch != torch_seen {
            torch_seen = torch;
            page_dirty |= open && page == "torch";
        }
        // The clock and About's time since power on change with the second: each drawn as its second turns (`tick_at` ends the rest there), not on a timer of its own that drifts against the second. The clock's second is the wall clock's in the zone (a new zone, or the clock read again, turns it too), About's the counter's, which it counts from; a clock not read yet ticks on the counter until it is.
        let hz = freq();
        let t = now();
        let into = match page {
            "clock" if open => Some(ferros_sys::local_time().unwrap_or(((t / hz) as i64, t % hz))),
            "about" if open => Some(((t / hz) as i64, t % hz)),
            _ => None,
        };
        // The counter read after the second's own reading, so the turn it gives is never early (a rest that ended before the turn would draw the same second again), only as late as the two reads are apart.
        let t = now();
        // When the second the clock or About shows ends (None while neither shows): the rest ends there, so the page's draw lands on the second, not up to a rest later.
        let tick_at = into.map(|(s, past)| {
            if s != tick_seen {
                tick_seen = s;
                page_dirty = true;
            }
            t + hz - past
        });
        if page_dirty && !map.settling {
            map.draw();
        }
        // The kists' bell rang (KISTS_WORD): the launch asked last looked for among those that did not start (`kists_rang`); one found opens its circle on why once the plane is at rest and no finger is down (`show_fault`), a question already in hand, not a poll.
        let kw = KISTS_WORD.load(core::sync::atomic::Ordering::Acquire);
        if kw != kists_seen {
            kists_seen = kw;
            map.kists_rang();
        }
        if !down {
            map.show_fault();
        }
        // The system's questions (docs/NOTIFY.md "System questions"): one that started waiting on the person posted as a note, one answered or ended cleared, from what this pass holds (the stower's offer, the sheet, the fleet, the open circle); the kernel's bell then lights the Deck button below.
        map.questions_sync();
        // The notes' bell rang, or the earliest deadline passed (the list read again drops it in the kernel): the glows, the Deck button, a tap or a takeover (notes.rs).
        let nw = notes::WORD.load(core::sync::atomic::Ordering::Acquire);
        if nw != notes_seen || map.notes_due.is_some_and(|d| now() >= d) {
            notes_seen = nw;
            map.notes_rang();
        }
        // The live page left without an answer (Deck closed it): the note stays waiting, glowing; a tap on its circle opens its kist at it.
        if map.live_note.is_some() && !map.live_showing() && map.flight.is_none() {
            map.live_note = None;
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
        // A kist offered over USB, checked and waiting on this person: a new one opens the Deck page on it, Stow and Cancel ready (above the list, or above a card left open), as a departure opens the Fleet page. Not over an app in front, nor under a finger or while the map settles: it opens once the fingers lift, or once the person is back on the map.
        if !map.app_front && !down && !map.settling {
            let offer = map.stower.offer.as_ref().map(|o| o.capsule);
            if offer.is_some() && offer != offer_seen {
                map.show_page("deck");
                map.draw();
                console(grants, b"map: the Kists page opened on the kist offered over USB, Stow and Cancel ready\n");
            }
            offer_seen = offer;
        } else if map.app_front && !offer_held_said && map.stower.offer.as_ref().is_some_and(|o| Some(o.capsule) != offer_seen) {
            // Said once an offer: the page waits for the app in front to go behind (Home), as KISTS.md "Stowing" has it.
            offer_held_said = true;
            console(grants, b"map: a kist offered over USB waits for the app in front to go behind; the Kists page opens then\n");
        }
        if map.stower.offer.is_none() {
            offer_held_said = false;
        }

        // The person's identity space (fgtw op 17's generation, in the copy), on the plane with nothing held: a new generation means the identity service wrote this device's kists or deck from the person's fleet (a reset device getting its arrangement back, a move made on another of their devices), and the circles are read again. Generation 0 is nothing written yet; a map started after a write reads it once more, which changes nothing.
        if map.view == View::Map && !down && !map.settling {
            if let Some(g) = map.fleet.space_gen.filter(|&g| g != space_seen) {
                space_seen = g;
                map.reload_arrangement();
            }
        }

        // An app in front (a bridge spawn: Hive, for one), asked of the display eight times a second.
        if !map.app_front && now().wrapping_sub(last_app_check) > freq() / 8 {
            last_app_check = now();
            let mut q = [0u8; 16];
            if call(map.display, &[5], &mut q) >= 13 && q[12] == 1 {
                map.app_front = true;
            }
        }
        // A kist started from a tap on a note: told which note once its window is in front (display op 67).
        if map.app_front {
            if let Some((c, n)) = map.open_pending.take() {
                map.open_at(c, n);
            }
        }
        // An app in front: it has the touches; the map only watches for it to hand the screen back.
        if map.app_front {
            if map.app_done() {
                map.app_front = false;
                map.front_kist = None;
                map.draw();
            }
            // Still in front: the touches are its, but the corners are the system's (WINDOWS.md), so they are read on below for a corner gesture, and nothing else is done with them.
        }
        // FERROS_MAP_START=open (development, measuring the flight with no one at the panel): three seconds after start and every two seconds after, while nothing moves, a tap on the settings circle opens it and the next tap, low at the left outside the open circle, closes it; eight flights.
        // FERROS_MAP_START=outhold: one flight in, then the flight out held at its eighth frame (the camera and the fade where they are, nothing stepped), for a capture of the screen mid-flight.
        // FERROS_MAP_START=kist (development, the first launch with no one at the panel): three seconds after start a tap on the last kist circle (the newest stowed); two seconds on, if the question is up, a tap on its "start".
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
            // Where the first finger landed: on the deck or back circle, the session is the corner's.
            let f = cur[ids[0]];
            let mut q = [0u8; 8];
            let (xb, yb) = ((f.1 as u16).to_le_bytes(), (f.2 as u16).to_le_bytes());
            let got = call(map.display, &[41, xb[0], xb[1], yb[0], yb[1]], &mut q);
            let hit = if got != ferros_sys::ERR && got >= 1 { q[0] } else { 0xFF };
            // A quick circle's disc on the glass (x, y, r), which the display answers after its id: a ring's drag counts from its bottom edge.
            let disc = (got != ferros_sys::ERR && got >= 7).then(|| {
                let at = |i: usize| u16::from_le_bytes([q[i], q[i + 1]]) as f32;
                (at(1), at(3), at(5))
            });
            // A console circle's zone (owner, 2026-11-11: "each zone in the console always map to a circle ... closest to circle wins if it's in the dash area"; the display's dash zones: 0-4 the radios, 5 USB, 13 the clock, 14 the battery, as 16 + its id): dragged toward the centre it opens that circle's page (owner: "dragging a console circle to center no longer works", restored), tapped still a radio switches and any other opens its page.
            // The quick row (QUICK.md): only the doorway by the camera (10) pulls or taps it open (owner: "the second console row should only show if drug down from the area in the invisible camera circle"); a quick circle (32 + k, its disc after) is held, a ring readied for a drag down; behind the row while it shows (12) shuts it, and the touch goes no further. The top's ground with no zone (11, none on these screens) is the system's and does nothing.
            corner = match hit {
                8 | 9 => hit,
                0..=5 | 13 | 14 => 16 | hit,
                10 => quick::PULL,
                11 => quick::NOTHING,
                12 => quick::BEHIND,
                c if (32..32 + quick::N).contains(&c) => quick::ITEM | (c - 32),
                _ => 0,
            };
            quick_door = hit == 10;
            // The panel's frame count now (op 24: the next frame's start): at rest the loop sleeps between passes and the count it holds is old, and a press, a pull or the spring counted from it would take every frame since as its own.
            if corner != 0 {
                let mut fr = [0u8; 4];
                call(map.display, &[24], &mut fr);
                frame_count = u32::from_le_bytes(fr);
            }
            match corner {
                quick::PULL => qr.take(f.2, frame_count),
                quick::BEHIND => qr.settle_to(0, frame_count),
                c if c & quick::ITEM != 0 => qr.hold((c & 7) as usize, disc),
                _ => {}
            }
            // A helm button under the finger: lit at once (display op 54).
            if corner == 8 || corner == 9 {
                map.helm_light(corner, 0.5);
            }
            corner_at = (f.1, f.2);
            corner_from = (f.1, f.2);
            corner_done = false;
            corner_far = false;
            pair = 0;
            pair_done = false;
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
            // The quick row's sessions (QUICK.md): the pull follows the finger from where it landed in the doorway; a ring dragged down is its level; any other drag up or down on a quick circle moves the open row (pushed back up, it shuts).
            if held >= 1 && corner != 0 && corner != 8 && corner != 9 {
                let (fx, fy) = (cur[ids[0]].1, cur[ids[0]].2);
                let (dx, dy) = (fx - corner_from.0, fy - corner_from.1);
                if corner & 16 != 0 && corner < 32 && !corner_done {
                    // A console circle's swipe inward, as the helm's was until 2026-11-12 (below): the finger's travel toward the screen's centre past a twelfth of the harmonic span opens its page, once a touch, whatever is in front (`dash_open`: an app in front goes behind first).
                    let (ux, uy) = ((w / 2 - corner_from.0) as f64, (h / 2 - corner_from.1) as f64);
                    let inward = (dx as f64 * ux + dy as f64 * uy) / libm::sqrt(ux * ux + uy * uy);
                    let span = (2 * w * h) as f64 / (w + h) as f64;
                    if inward > span / 12. {
                        corner_done = true;
                        map.dash_open(corner & 15);
                        map.draw();
                    }
                } else if corner & quick::ITEM != 0 && moved {
                    // A ring dragged down is its level (QUICK.md "Quick pages"; owner, 2026-11-12: "when dragging, once your finger goes below the torch circle it goes from 0% to 100% at the screen midpoint"): nothing while the finger is above the circle's bottom edge, then where the finger is, down to the screen's middle (`quick::level_at`). Once the drag took hold it keeps the finger whichever way it goes. Up from a ring, or up or down from a switch, moves the row as before (pushed back up, it shuts). No drag opens a page (a tap on a ring does).
                    let k = (corner & 7) as usize;
                    if qr.pulling() || quick::level(k) && dy > 0 && dy.abs() > dx.abs() {
                        qr.pull_to(fy);
                    } else if dy.abs() > dx.abs() {
                        qr.let_go(true, frame_count);
                        corner = quick::PULL;
                        qr.take(fy, frame_count);
                    }
                }
                if corner == quick::PULL {
                    qr.drag(fy, frame_count);
                }
            }
            // Two fingers swiped up from back and deck together (owner, 2026-11-11: "Quick shortcut in dev only is swipe up from both back and home to toggle on/off"): the display's hit map in every fourth frame, once a touch. The second finger's landing is asked of the display as the first's was (op 41); while the two are a pair the corner's own swipe does nothing.
            if DEV && (corner == 8 || corner == 9) && held >= 2 && pair == 0 {
                pair = 2;
                if let Some(&id) = ids[..held].iter().find(|&&id| (start[id].1, start[id].2) != corner_from) {
                    let (xb, yb) = ((start[id].1 as u16).to_le_bytes(), (start[id].2 as u16).to_le_bytes());
                    let mut q = [0u8; 8];
                    if call(map.display, &[41, xb[0], xb[1], yb[0], yb[1]], &mut q) >= 1 && q[0] == 17 - corner {
                        pair = 1;
                    }
                }
            }
            if pair == 1 && !pair_done && held >= 2 {
                // Up past a twelfth of the screen's harmonic span (the corner's own threshold until 2026-11-12), both of them.
                let span = (2 * w * h) as f64 / (w + h) as f64;
                if ids[..held].iter().filter(|&&id| (start[id].2 - cur[id].2) as f64 > span / 12.).count() >= 2 {
                    pair_done = true;
                    map.hit_view_toggle();
                    // The Developer page, if it is on show, draws its checkbox anew.
                    if !map.app_front && matches!(map.view, View::Open(_)) {
                        map.draw();
                    }
                }
            }
            if (corner == 8 || corner == 9) && held >= 1 && pair != 1 {
                corner_at = (cur[ids[0]].1, cur[ids[0]].2);
                // The swipe inward: the finger's travel along the line from where it landed toward the screen's centre, and across it. Past a sixth of the screen's harmonic span (2wh/(w+h)) and mostly inward (within 45 degrees of that line: more along it than across), the corner acts, once a touch (WINDOWS.md "The helm"): deck, the app in front full screen or not, or with none out to every circle turned to fit; back, the kist before. Until 2026-11-12 a twelfth and any direction (owner, 2026-11-07), and on the 8a a tap with a little roll crossed it (owner: "it changes orientation instantly on occasion"): deck's fit and turn fired on a tap.
                let (ux, uy) = ((w / 2 - corner_from.0) as f64, (h / 2 - corner_from.1) as f64);
                let (dx, dy) = ((corner_at.0 - corner_from.0) as f64, (corner_at.1 - corner_from.1) as f64);
                let ul = libm::sqrt(ux * ux + uy * uy);
                let inward = (dx * ux + dy * uy) / ul;
                let across = (dx * uy - dy * ux) / ul;
                let span = (2 * w * h) as f64 / (w + h) as f64;
                let reach = span / HELM_SWIPE;
                // Gone further than a roll (a twelfth of the span) any way: no longer a tap, whatever it does next.
                if dx * dx + dy * dy > (span / HELM_ROLL) * (span / HELM_ROLL) {
                    corner_far = true;
                }
                // The helm button's light follows the swipe: half at the landing, toward full as the finger comes in (½(1 + tanh(inward / threshold)), C∞, lower again if it pulls away), full once it acted.
                let lvl = if corner_done { 1.0 } else { 0.5 * (1.0 + libm::tanh(inward / reach)) };
                map.helm_light(corner, lvl as f32);
                if !corner_done && inward > reach && inward > across.abs() {
                    corner_done = true;
                    // The helm's swipes (owner, 2026-11-11, on the Pro: "the swipe in on home whilist in a kist needs to show/hide controls. on the home screen it auto-fits"; "Can't hide controls on home screen"). Deck's: with an app in front, its full screen on or off (`full_switch`; the kist stays in front, where until then this swipe sent it behind first); with none, the plane flies out to every circle at the turn that fits them largest ("so it spins into place"), the controls always shown there. The display's answer decides which, not `app_front`. Back's: a flip to the kist before (this one behind). The quick row's middle circle switches full screen too.
                    let said = if corner == 8 {
                        match map.full_switch() {
                            Some(true) => {
                                // The quick row goes with the console (`quick::Quick::shut`).
                                qr.shut();
                                "map: deck: full screen, the controls hidden"
                            }
                            Some(false) => "map: deck: full screen off, the controls shown",
                            None => {
                                map.deck_fly();
                                "map: deck: every circle in view, turned to fit them largest"
                            }
                        }
                    } else {
                        map.flip()
                    };
                    console(grants, said.as_bytes());
                    console(grants, b"\n");
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
            // A corner's session let go: it acted while moving (above), if it went far enough inward. A still tap is each place's own (below): the helm's deck and back, a console circle's, the quick row's.
            let mut tap_thru = !moved;
            // The quick row's sessions end here (QUICK.md): a still tap in the doorway opens or shuts the row, any other pull settles by halving; a quick circle acts (or its turn is kept); behind the row the touch already shut it and goes no further. A console circle tapped still: a radio switches (preference 15), any other opens its page; dragged in it already opened its page; dragged elsewhere nothing.
            match corner {
                quick::PULL => {
                    if !moved && quick_door {
                        qr.toggle(frame_count);
                        tap_thru = false;
                    } else {
                        qr.release(frame_count);
                    }
                }
                quick::BEHIND => tap_thru = false,
                c if c & quick::ITEM != 0 => {
                    // A level circle tapped (brightness, the torch) opens its page, the row shutting as it does.
                    if let Some(page) = qr.let_go(moved, frame_count) {
                        if !map.app_front {
                            qr.settle_to(0, frame_count);
                            map.show_page(page);
                            map.draw();
                        }
                    }
                    tap_thru = false;
                }
                c if c & 16 != 0 && c < 32 => {
                    tap_thru = false;
                    let id = c & 15;
                    if !moved && id < 5 {
                        let off = ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF) ^ (1 << id);
                        ferros_sys::set_pref(cap(grants, S_SETTINGS), ferros_sys::PREF_RADIOS_OFF, off);
                        let mut l = Line::new();
                        let _ = write!(l, "map: status circle {id} tapped: radios off G#{off:02X}\n");
                        console(grants, l.as_str().as_bytes());
                    } else if !moved {
                        map.dash_open(id);
                        map.draw();
                    }
                }
                quick::NOTHING => tap_thru = false,
                // The helm tapped still: deck or back (`helm_tap`), with an app in front as on the map. Two fingers on back and deck together are the hit map's pair (development builds), never a tap.
                8 | 9 => {
                    tap_thru = false;
                    // Still, or rolled less than a twelfth of the span and never swiped (owner, 2026-11-12: a tap with a roll is a tap): the general slop is a few pixels, a fingertip's roll on the glass more.
                    if !corner_done && !corner_far && pair != 1 {
                        let said = map.helm_tap(corner);
                        console(grants, said.as_bytes());
                        console(grants, b"\n");
                    }
                }
                _ => {}
            }
            if corner != 0 && tap_thru {
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
                // Settled on the deck: its resting view, and what is in it (notes.rs).
                if map.view == View::Map {
                    map.rest = map.cam;
                    map.notes_view();
                }
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

        // The quick row this pass (QUICK.md): stepped by the frames the panel showed since its last step; its state to the display when anything in it moved.
        if !down {
            qr.step(frame_count);
        }
        qr.send();
        // A corner's session moves nothing on the map; with an app in front the map does nothing else at all, reading touches only for the corners. While a corner session or the quick row is under way, each pass starts as the panel's next frame does (op 24, as a drag's), so the row and a press move by shown frames; otherwise a sixty-fourth of a second between looks while an app is in front.
        if corner != 0 || map.app_front {
            prev = cur;
            if corner != 0 || qr.settling {
                let mut fr = [0u8; 4];
                call(display, &[24], &mut fr);
                frame_count = u32::from_le_bytes(fr);
            } else {
                sleep_until(now() + freq() / (1 << 6));
            }
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
        if held == 0 && !map.settling && !qr.settling {
            // WHY the min: two deadlines, the rest's and the second's turn on the clock or About (`tick_at`); the earlier ends the rest, so the second is drawn as it turns rather than up to a rest after.
            let rest = now() + freq() / 30;
            // A note's deadline ends the rest too, as an alarm would (docs/NOTIFY.md "Deadlines").
            let rest = map.notes_due.map_or(rest, |d| rest.min(d));
            sleep_until(tick_at.map_or(rest, |at| rest.min(at)));
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
            if DEV && map.flight.is_some() {
                map.flight_trace.push_str(" |");
            }
            map.settling = moving;
            if option_env!("FERROS_MAP_START") == Some("outhold") && map.leaving && map.flight.is_some_and(|f| f.t >= 8) {
                map.settling = false;
            }
            if steps > 0 {
                map.draw();
            }
            // Come to rest after the lift (a fling, a flight): the view into the vault if it moved, as the lift does for what the fingers left.
            if !moving && map.cam != saved_view {
                // Settled on the deck: its resting view, and what is in it (notes.rs).
                if map.view == View::Map {
                    map.rest = map.cam;
                    map.notes_view();
                }
                if map.save_view() {
                    saved_view = map.cam;
                }
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
