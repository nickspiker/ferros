//! The quick row's gestures, its physics and its actions (docs/QUICK.md; owner, 2026-11-11: "gesture ideas, would be nice to have the torch, brightness and maybe a few other things", the proposal answered "Works", then "any tap in that area or drag down should bring up said second row of consoleness"). The display draws the row (its quick.rs) from the state sent here (op 56); the map owns the finger, the frames and what a tap does.
//!
//! - **The pull** `p`: 0 shut, 1 open. A finger that lands in the doorway by the camera (op 41: 10), on the top's other ground (11) or on a radio circle and then goes down drags it: `p` is where it was when the finger landed plus the finger's travel over `pull_len` (an eighth of the screen's harmonic span), unbounded, so the row follows the finger one to one (the display draws it at `ease(p)`). From open a push up brings it back the same way.
//! - **The lift.** The row settles open when it was pulled past a third of the way (or, pushed from open, not back past a third), else shut, on a critically damped spring stepped once per frame the panel showed (owner, 2026-11-08: "we follow shown frames, not composited"), starting at the finger's own speed, so position and speed are both continuous at the lift and it never overshoots from rest. No timer: the spring's steps are the shown frames.
//! - **Taps.** In the doorway: the row opens, or shuts. On a quick circle: its action (`tap`). Behind the row while it shows (op 41: 12): it shuts.
//! - **The rings.** A finger on brightness or the torch turns them: the angle it sweeps about the circle's centre, a whole turn the whole range, eased at both ends (`ease`) so the value slows into each end rather than stopping dead.
//! - **Long press** on a radio circle: its page in settings once the finger has held still on it for `LONG_PRESS` shown frames, its ring filling meanwhile (owner, 2026-11-11, on the proposal: "Long press on a radio circle opens its Settings page; the quick row is one-tap things only"). A tap still switches the radio; a drag down pulls the row.

use super::*;
use core::sync::atomic::{AtomicU64, Ordering};

/// The map's corner sessions for the row (main.rs `corner`): a finger pulling it, behind it, or on its circle k (ITEM | k).
pub const PULL: u8 = 0x40;
pub const BEHIND: u8 = 0x60;
pub const ITEM: u8 = 0x80;
/// The row's circles, left to right, as the display's quick::ITEMS has them.
pub const RADIOS_OFF: usize = 0;
pub const SOUND: usize = 1;
pub const BRIGHTNESS: usize = 2;
pub const TORCH: usize = 3;
pub const DND: usize = 4;
pub const SCREENCAP: usize = 5;

/// Half the width of `ease`'s ends (the display's quick.rs EASE_HALF).
const EASE_HALF: f32 = 1. / (1 << 3) as f32;
/// The spring's angular frequency, radians a shown frame: critically damped, (1 + ωt)e^(−ωt) of the way is left after t frames, a thousand-and-twenty-fourth after about 37 (a third of a second at 120 Hz).
const OMEGA: f32 = 1. / (1 << 2) as f32;
/// Where the spring lands: within a thousand-and-twenty-fourth of its end, moving slower than that a frame (the drawn row is then within a twentieth of a pixel of where it lands: `ease` is near flat there).
const LAND: f32 = 1. / (1 << 10) as f32;
/// A long press: the shown frames a finger rests on a radio circle before its page opens (about half a second at 120 Hz).
const LONG_PRESS: u32 = 1 << 6;

/// The twin of the display's quick.rs `ease`: the pull as the row's level, the value itself thru the middle, exactly 0 below and 1 above, C∞. Here it eases a ring's share the same way.
pub fn ease(p: f32) -> f32 {
    let s = |t: f32| bump_step((t + EASE_HALF) * (1. / (2. * EASE_HALF)));
    let q = p * s(p);
    1. - (1. - q) * s(1. - q)
}

/// The C∞ step (the display's draw.rs `bump_step`): 0 at t ≤ 0, 1 at t ≥ 1, the bump exp(−1/t) against its mirror between (the two tests are its definition, not a clamp).
fn bump_step(t: f32) -> f32 {
    if t <= 0. {
        return 0.;
    }
    if t >= 1. {
        return 1.;
    }
    let (a, b) = (libm::expf(-1. / t), libm::expf(-1. / (1. - t)));
    a / (a + b)
}

/// The pull a ring's share comes from: `ease`'s root, bisected over its whole span (it is 0 at −EASE_HALF and 1 at 1 + EASE_HALF).
fn unease(share: f32) -> f32 {
    let (mut lo, mut hi) = (-EASE_HALF, 1. + EASE_HALF);
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        if ease(mid) < share { lo = mid } else { hi = mid }
    }
    (lo + hi) * 0.5
}

/// A finger turning a ring: which circle, its centre, the finger's last angle (clockwise from twelve o'clock) and the pull the share comes from (`ease(acc)` is the share lit).
#[derive(Clone, Copy)]
struct Turn {
    k: usize,
    cx: f32,
    cy: f32,
    angle: f32,
    acc: f32,
}

pub struct Quick {
    display: u64,
    grants: u64,
    /// The finger's travel that pulls the row all the way, pixels: an eighth of the screen's harmonic span 2wh/(w + h).
    pull_len: f32,
    pub p: f32,
    v: f32,
    target: f32,
    pub settling: bool,
    /// The pull and the finger's row when the drag took hold, and whether the row was open then.
    grab: Option<(f32, i64)>,
    from_open: bool,
    /// The pull and the shown frame at the last pass, for the speed at the lift.
    last: (f32, u32),
    /// The quick circle a finger holds, and a ring it turns.
    held: Option<usize>,
    turn: Option<Turn>,
    /// The shares a turn shows (op 56), None while no finger turns that ring.
    bright_arc: Option<f32>,
    torch_arc: Option<f32>,
    /// The radio circle a finger presses and the shown frame it landed on; how far the press has come (0 to 1).
    press: Option<(u8, u32)>,
    press_share: f32,
    /// A screencap wanted once the row is shut.
    capture: bool,
    /// The torch's level to light at a tap (the last it was lit at, the whole dimmer at first).
    torch_lit: u8,
    /// The torch's level asked of it last, so a turn sends each new level once.
    torch_sent: u8,
    /// Whether the torch was asked its level since the row last started to show.
    torch_asked: bool,
    /// Op 56 as last sent: sent again only when something in it moved.
    sent: [u8; 15],
}

impl Quick {
    pub fn new(display: u64, grants: u64, w: i64, h: i64) -> Quick {
        let span = (2 * w * h) as f32 / (w + h) as f32;
        Quick { display, grants, pull_len: span * (1. / (1 << 3) as f32), p: 0., v: 0., target: 0., settling: false, grab: None, from_open: false, last: (0., 0), held: None, turn: None, bright_arc: None, torch_arc: None, press: None, press_share: 0., capture: false, torch_lit: 255, torch_sent: 0, torch_asked: false, sent: [0xEE; 15] }
    }

    /// A finger took hold of the pull at row `y` (a drag that began in the doorway, on the top's ground, on a radio circle or on a quick circle): the spring lets go of it, and it follows the finger from where it is.
    pub fn take(&mut self, y: i64, frame: u32) {
        self.settling = false;
        self.grab = Some((self.p, y));
        self.from_open = self.p >= 0.5;
        self.last = (self.p, frame);
        self.v = 0.;
    }

    /// The finger, now at row `y`, a shown frame `frame`.
    pub fn drag(&mut self, y: i64, frame: u32) {
        let Some((p0, y0)) = self.grab else { return };
        let p = p0 + (y - y0) as f32 / self.pull_len;
        // The speed in pull a shown frame, from the last pass that saw another frame (two passes can fall in one frame: op 24 comes back after a hundred-and-twentieth of a second without one).
        let frames = frame.wrapping_sub(self.last.1);
        if frames > 0 {
            self.v = (p - self.last.0) / frames as f32;
            self.last = (p, frame);
        }
        self.p = p;
    }

    /// The finger lifted from the pull: the row settles open past a third of the way, or (pushed from open) unless pushed back past a third, else shut, from its speed now.
    pub fn release(&mut self, frame: u32) {
        if self.grab.take().is_none() {
            return;
        }
        let third = 1. / 3.;
        self.target = if (self.from_open && self.p > 1. - third) || (!self.from_open && self.p >= third) { 1. } else { 0. };
        self.settling = true;
        self.last.1 = frame;
    }

    /// Open (from shut) or shut (from open) by a tap: the spring takes it from rest.
    pub fn toggle(&mut self, frame: u32) {
        let open = !(self.target == 1. && self.p > 0.);
        self.settle_to(if open { 1. } else { 0. }, frame);
    }

    pub fn settle_to(&mut self, target: f32, frame: u32) {
        self.grab = None;
        self.target = target;
        self.settling = true;
        self.last.1 = frame;
    }

    /// The spring stepped once for each frame the panel showed since the last step: the remaining offset pulled back by ω² and the speed damped by 2ω (critically damped, semi-implicit), landing exactly on its end once within `LAND` and nearly still. Returns whether it still moves.
    pub fn step(&mut self, frame: u32) -> bool {
        if !self.settling {
            return false;
        }
        let steps = frame.wrapping_sub(self.last.1);
        self.last.1 = frame;
        for _ in 0..steps {
            let a = -OMEGA * OMEGA * (self.p - self.target) - 2. * OMEGA * self.v;
            self.v += a;
            self.p += self.v;
            if libm::fabsf(self.p - self.target) < LAND && libm::fabsf(self.v) < LAND {
                self.p = self.target;
                self.v = 0.;
                self.settling = false;
                break;
            }
        }
        self.settling
    }

    /// A finger landed on quick circle `k` at (x, y), the circle's centre (cx, cy): held (lighter), and on a ring the turn begins at the share it shows now.
    pub fn hold(&mut self, k: usize, x: i64, y: i64, cx: f32, cy: f32) {
        self.held = Some(k);
        let share = match k {
            BRIGHTNESS => 1. - ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS) as f32 * (1. / 255.),
            TORCH => torch_now().map_or(0., |l| l as f32 * (1. / 255.)),
            _ => return,
        };
        // A torch that does not answer has no ring to turn.
        if k == TORCH && torch_now().is_none() {
            return;
        }
        self.turn = Some(Turn { k, cx, cy, angle: angle(x, y, cx, cy), acc: unease(share) });
    }

    /// The finger on a ring moved to (x, y): the angle swept since the last pass turns the share, a whole turn the whole range; brightness is applied live (not kept until the lift), the torch asked for each new level.
    pub fn turn_to(&mut self, x: i64, y: i64) {
        let Some(mut t) = self.turn else { return };
        let a = angle(x, y, t.cx, t.cy);
        let tau = core::f32::consts::TAU;
        // The sweep the short way round: atan2 jumps by a whole turn at six o'clock, and a finger moves far less than half a turn about the centre in one pass.
        let mut d = a - t.angle;
        if d > tau * 0.5 {
            d -= tau;
        } else if d < -tau * 0.5 {
            d += tau;
        }
        t.angle = a;
        t.acc += d / tau;
        let share = ease(t.acc);
        self.turn = Some(t);
        // The share as a byte: the cast holds a share the ease puts a hair past either end at 0 or 255.
        match t.k {
            BRIGHTNESS => {
                self.bright_arc = Some(share);
                let v = (255. * (1. - share) + 0.5) as u8;
                if v != ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS) {
                    ferros_sys::set_pref_live(cap(self.grants, S_SETTINGS), ferros_sys::PREF_BRIGHTNESS, v);
                }
            }
            _ => {
                self.torch_arc = Some(share);
                let v = (255. * share + 0.5) as u8;
                if v != self.torch_sent {
                    self.torch_sent = v;
                    if v > 0 {
                        self.torch_lit = v;
                    }
                    torch_ask(Some(v));
                }
            }
        }
    }

    /// The finger left a quick circle it held: a turn ends (brightness kept in the vault now, once), or a still finger acts (`tap`).
    pub fn let_go(&mut self, moved: bool, frame: u32) {
        if let Some(t) = self.turn.take() {
            if t.k == BRIGHTNESS && self.bright_arc.is_some() {
                ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_BRIGHTNESS, ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS));
            }
        }
        let held = self.held.take();
        let turned = self.bright_arc.take().is_some() | self.torch_arc.take().is_some();
        if let (Some(k), false, false) = (held, moved, turned) {
            self.tap(k, frame);
        }
    }

    /// A tap on quick circle k.
    fn tap(&mut self, k: usize, frame: u32) {
        let settings = cap(self.grants, S_SETTINGS);
        let mut l = Line::new();
        match k {
            RADIOS_OFF => {
                // Every radio off, the set that was off kept (preference 41); from all off, that set back (all on when it was all of them).
                let off = ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF) & 0x1F;
                if off == 0x1F {
                    let before = ferros_sys::pref(ferros_sys::PREF_RADIOS_BEFORE) & 0x1F;
                    let back = if before == 0x1F { 0 } else { before };
                    ferros_sys::set_pref(settings, ferros_sys::PREF_RADIOS_OFF, back);
                    let _ = write!(l, "map: quick: radios back, off G#{back:02X}\n");
                } else {
                    ferros_sys::set_pref(settings, ferros_sys::PREF_RADIOS_BEFORE, off);
                    ferros_sys::set_pref(settings, ferros_sys::PREF_RADIOS_OFF, 0x1F);
                    let _ = write!(l, "map: quick: every radio off (G#{off:02X} were)\n");
                }
            }
            SOUND => {
                // Sound, vibrate, silent, round again.
                let next = (ferros_sys::pref(ferros_sys::PREF_SOUND) + 1) % 3;
                ferros_sys::set_pref(settings, ferros_sys::PREF_SOUND, next);
                let _ = write!(l, "map: quick: sound mode {next}\n");
            }
            BRIGHTNESS => {}
            TORCH => match torch_now() {
                Some(0) => {
                    self.torch_sent = self.torch_lit;
                    torch_ask(Some(self.torch_lit));
                }
                Some(_) => {
                    self.torch_sent = 0;
                    torch_ask(Some(0));
                }
                // No torch answers: nothing (the circle is drawn dimmed).
                None => {}
            },
            DND => {
                let on = !ferros_sys::dnd::on();
                dnd_ask(on);
                let _ = write!(l, "map: quick: do not disturb {}\n", if on { "on" } else { "off" });
            }
            SCREENCAP => {
                // The row shuts first; the display takes the capture once no circle of it is on the screen (op 56's flag), so it is not in the picture.
                self.capture = true;
                self.settle_to(0., frame);
                let _ = write!(l, "map: quick: screencap once the row is shut\n");
            }
            _ => {}
        }
        if !l.as_str().is_empty() {
            console(self.grants, l.as_str().as_bytes());
        }
    }

    /// A finger landed on radio circle `radio` (its bit) at shown frame `frame`: the press starts.
    pub fn press_start(&mut self, radio: u8, frame: u32) {
        self.press = Some((radio, frame));
        self.press_share = 0.;
    }

    /// The press goes (the finger moved or lifted).
    pub fn press_end(&mut self) {
        self.press = None;
        self.press_share = 0.;
    }

    /// The press at shown frame `frame`: its ring's share; Some(radio) the pass it has held long enough (its page opens; the press is over).
    pub fn press_at(&mut self, frame: u32) -> Option<u8> {
        let (radio, from) = self.press?;
        let n = frame.wrapping_sub(from);
        self.press_share = n as f32 * (1. / LONG_PRESS as f32);
        if n >= LONG_PRESS {
            self.press_end();
            return Some(radio);
        }
        None
    }

    /// Op 56 to the display when anything in it moved: the pull, the torch, the held circle, the press, the rings' shares, and a screencap once the row has landed shut.
    pub fn send(&mut self) {
        // The torch asked its level once each time the row starts to show (past the ease's first stretch, so a finger resting on the top's ground starts nothing), until it has answered: its first call starts it (docs/TORCH.md), and it ends itself again while dark.
        if self.p > EASE_HALF && !self.torch_asked {
            self.torch_asked = true;
            if torch_now().is_none() {
                torch_ask(None);
            }
        }
        if self.p == 0. && !self.settling {
            self.torch_asked = false;
        }
        let mut m = [0u8; 15];
        m[0] = 56;
        m[1..5].copy_from_slice(&((self.p * 65536.) as i32).to_le_bytes());
        let torch = torch_now();
        let landed_shut = self.p == 0. && !self.settling && self.grab.is_none();
        m[5] = torch.is_some() as u8 | ((self.capture && landed_shut) as u8) << 1;
        m[6] = torch.unwrap_or(0);
        m[7] = self.held.map_or(0xFF, |k| k as u8);
        m[8] = self.press.filter(|_| self.press_share > 0.).map_or(0xFF, |p| p.0);
        // A share as Q16, G#FFFF for none. WHY the min: a full share (1, or the hair past it `ease` allows) would read as none on the wire; G#FFFE is a 65535th short of full, under a hundredth of a pixel of arc.
        let q16 = |v: Option<f32>| v.map_or(0xFFFF, |s| ((s * 65535.) as u16).min(0xFFFE)).to_le_bytes();
        let (b, t, s) = (q16(self.bright_arc), q16(self.torch_arc), q16(Some(self.press_share)));
        m[9..11].copy_from_slice(&b);
        m[11..13].copy_from_slice(&t);
        m[13..15].copy_from_slice(&s);
        if m == self.sent {
            return;
        }
        let mut r = [0u8; 4];
        if call(self.display, &m, &mut r) != ferros_sys::ERR {
            self.sent = m;
            // The capture is the display's now.
            if m[5] & 2 != 0 {
                self.capture = false;
            }
        }
    }
}

/// The angle of (x, y) about (cx, cy), clockwise from twelve o'clock (y grows down the screen).
fn angle(x: i64, y: i64, cx: f32, cy: f32) -> f32 {
    libm::atan2f(x as f32 + 0.5 - cx, cy - (y as f32 + 0.5))
}

// ---- the slow calls, on a thread of their own: the torch (started by the kernel on demand: its first call waits while it starts) and the identity service (busy with the network); the main loop never waits on either (`fgtw_poller`'s rule) ----

/// Requests: counted up at each (the worker waits on it).
static ASK: AtomicU64 = AtomicU64::new(0);
/// The torch level wanted (bit 8 set: a request; bit 9: only its level asked), taken by the worker.
static TORCH_WANT: AtomicU64 = AtomicU64::new(0);
/// Do not disturb wanted (bit 8 set: a request; bit 0 on), taken by the worker.
static DND_WANT: AtomicU64 = AtomicU64::new(0);
/// The torch as it last answered: bit 8 it answered, bit 9 a call failed (no torch: the circle dims), bits 0-7 its level.
static TORCH_NOW: AtomicU64 = AtomicU64::new(0);

/// The torch's level as it last answered; None when it has not or a call failed. Unknown until the row first opens, and dark then (`torch_ask(None)`).
pub fn torch_now() -> Option<u8> {
    let t = TORCH_NOW.load(Ordering::Acquire);
    (t & 0x300 == 0x100).then_some(t as u8)
}

/// Ask the torch for a level (Some) or only its level now (None), on the worker.
pub fn torch_ask(level: Option<u8>) {
    TORCH_WANT.store(1 << 8 | level.map_or(1 << 9, |l| l as u64), Ordering::Release);
    ASK.fetch_add(1, Ordering::AcqRel);
    ferros_sys::wake(&ASK, 1);
}

/// Ask the identity service to switch the person's do not disturb (fgtw op 24), on the worker.
fn dnd_ask(on: bool) {
    DND_WANT.store(1 << 8 | on as u64, Ordering::Release);
    ASK.fetch_add(1, Ordering::AcqRel);
    ferros_sys::wake(&ASK, 1);
}

/// The worker: asleep on `ASK` until the main loop wants something, then each call made waiting (a torch started on demand answers once it runs, docs/TORCH.md "Who starts it"; a live call would be refused while it starts). The torch's answer is published in `TORCH_NOW` for the main loop's next pass.
pub extern "C" fn worker(grants: u64) -> ! {
    let mut seen = 0u64;
    loop {
        let n = ASK.load(Ordering::Acquire);
        if n == seen {
            ferros_sys::wait_world(&ASK, n, 0);
            continue;
        }
        seen = n;
        let t = TORCH_WANT.swap(0, Ordering::AcqRel);
        if t & 1 << 8 != 0 {
            let msg = if t & 1 << 9 != 0 { [1u8, 0] } else { [0u8, t as u8] };
            let mut r = [0u8; 4];
            let got = call(cap(grants, S_TORCH), &msg[..if msg[0] == 1 { 1 } else { 2 }], &mut r);
            if got == ferros_sys::ERR || got == 0 {
                TORCH_NOW.store(1 << 9, Ordering::Release);
                console(grants, b"map: quick: no torch answered\n");
            } else {
                TORCH_NOW.store(1 << 8 | r[0] as u64, Ordering::Release);
            }
        }
        let d = DND_WANT.swap(0, Ordering::AcqRel);
        if d & 1 << 8 != 0 {
            let mut r = [0u8; 4];
            let got = call(cap(grants, S_FGTW), &[24, d as u8 & 1], &mut r);
            if got == ferros_sys::ERR || got == 0 || r[0] != 0 {
                console(grants, b"map: quick: the identity service did not take do not disturb (not running, or before its first pull)\n");
            }
        }
    }
}
