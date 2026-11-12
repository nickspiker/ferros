//! The quick row's gestures, its physics and its actions (docs/QUICK.md; owner, 2026-11-11: "gesture ideas, would be nice to have the torch, brightness and maybe a few other things", the proposal answered "Works", then "any tap in that area or drag down should bring up said second row of consoleness"). The display draws the row (its quick.rs) from the state sent here (op 56); the map owns the finger, the frames and what a tap does.
//!
//! - **The slide** (owner, 2026-11-11, on the Pro, after the first cut's grow-and-fan: "the top console pulldown is stupidly slow and jerky", then "track the finger down from top tho and slide it in so it doesn't need a re-render, just a move and composite, then do the iterate/2 each frame, like we do for the settings page until it snaps in or out of place"). The row's offset `off`, whole pixels: 0 hidden, `slide` open (the display's length, op 56's reply). A finger that lands in the doorway by the camera (op 41: 10) and goes down moves it one to one (owner, 2026-11-11: "the second console row should only show if drug down from the area in the invisible camera circle"); from open a push up brings it back the same way. The display draws the row once and only moves it.
//! - **The lift.** The row goes open when it was pulled past a third of the way (or, pushed from open, not back past a third), else hidden: each frame the panel showed, what is left to go halves (a right shift), so it lands in a few frames and holds still (`step`). No timer: the steps are the shown frames (owner, 2026-11-08: "we follow shown frames, not composited").
//! - **Two kinds of circle** (QUICK.md "Quick pages"). A switch acts on a tap (`tap`): radios off and back, sound round its modes, do not disturb, full screen, screencap. A level (the two rings, brightness and the torch) opens its page on a tap and is set by a drag down (owner, 2026-11-12: "tap on torch brings up the torch config screen, when dragging, once your finger goes below the torch circle it goes from 0% to 100% at the screen midpoint"; and "Same for brightness"). In the doorway a tap opens or shuts the row. Behind the row while it shows (op 41: 12): it shuts.
//! - **The drag down from a ring** (`pull_to`): nothing while the finger is still above the circle's bottom edge; once below it, the level is where the finger is, not how far it went: 0 at the circle's bottom edge (the torch off, brightness the dimmest the panel goes) to the whole level at the screen's vertical middle, back down as the finger goes back up, eased into both ends (`level_at`, the ring's own C∞ ease pinned to those two rows). Let go, the level is kept. No page opens from a drag (the span/12 open distance of 2026-11-11 went with this), and no long press anywhere (it would need a timer).
//! - **Full screen** (owner, 2026-11-11, on the Pro: "helm should be functional but not visible in full screen, whilist console should be gone gone"): the display neither draws the row nor gives it, the doorway or the console any pixel of the hit map, so nothing here starts; turned on by the map (the middle circle, home's swipe), the row shuts at once (`shut`).
//! - **The console's circles** (main.rs): every pixel of the dash is a circle's zone (the display's, nearest rim). Dragged toward the centre a circle opens its page; tapped still a radio switches and any other circle opens its page. No long press (it went 2026-11-11, with drag-to-centre back). Only the doorway pulls or taps the row open.

use super::*;
use core::sync::atomic::{AtomicU64, Ordering};

/// The map's corner sessions for the row (main.rs `corner`): a finger pulling it, behind it, or on its circle k (ITEM | k).
pub const PULL: u8 = 0x40;
pub const BEHIND: u8 = 0x60;
/// The top's ground no zone holds (op 41: 11): the system's, a touch there does nothing.
pub const NOTHING: u8 = 0x30;
pub const ITEM: u8 = 0x80;
/// The row's circles, left to right, as the display's quick::ITEMS has them.
pub const RADIOS_OFF: usize = 0;
pub const SOUND: usize = 1;
pub const BRIGHTNESS: usize = 2;
/// The middle one (owner, 2026-11-11: full screen moves from the deck swipe to "a quick-row circle"): the app in front full screen or not (display op 43); nothing with no app in front, where the display draws it dimmed.
pub const FULL_SCREEN: usize = 3;
pub const TORCH: usize = 4;
pub const DND: usize = 5;
pub const SCREENCAP: usize = 6;
/// How many circles the row has, and the hit map's id of the first (op 41: 32 + k).
pub const N: u8 = 7;

/// Whether quick circle k is a level (the two rings): a tap opens its page, a drag down sets it. Every other circle is a switch: a tap acts, a drag moves the row.
pub fn level(k: usize) -> bool {
    k == BRIGHTNESS || k == TORCH
}

/// A level circle's page, the settings page its tap opens (QUICK.md "Quick pages": a quick circle's page is the settings page that holds what it sets, opened a second way): brightness Appearance, the one page for the look (owner, 2026-11-12: "tap brings you to the more granular screen settings page, which I wanted to combine a few things, brightness, theme, other look related items"); the torch its own (owner, same day: "tap on torch brings up the torch config screen"). The switches have none here: a tap is their action.
pub fn page(k: usize) -> Option<&'static str> {
    match k {
        BRIGHTNESS => Some("appearance"),
        TORCH => Some("torch"),
        _ => None,
    }
}

/// Half the width of `ease`'s ends (a ring's share eased at both ends).
const EASE_HALF: f32 = 1. / (1 << 3) as f32;

/// A ring's share eased at its ends: the value itself thru the middle, exactly 0 from −EASE_HALF down and exactly 1 from 1 + EASE_HALF up, C∞, so the level slows into each end rather than stopping dead. Each corner of the ramp is smoothed by the bump step integrated (`bump_ramp`): its slope is the bump step itself, rising from 0 to 1 across the corner and never past, so the share never leaves [0, 1] and never turns back. Until 2026-11-12 a corner was p times the bump step, whose slope rose to about twice the middle's and which dipped 0.65 % below 0 and rose 0.6 % past 1 (a pull counted from where it took hold never showed it; a level at the finger's place would have, as a dead tenth of the run at the circle's edge where the byte held at 0).
pub fn ease(p: f32) -> f32 {
    let corner = |x: f32| 2. * EASE_HALF * bump_ramp(x * (1. / (2. * EASE_HALF)) + 0.5);
    1. - corner(1. - corner(p))
}

/// The bump step integrated from 0 to t (`bump_step`'s ramp): 0 at t ≤ 0, t − ½ at t ≥ 1 (the step is antisymmetric about ½, so its whole integral is a half), and between them by Simpson's rule over SLICES slices (C∞ in t, the integrand is; its slope in t is a sum of the step and its rise, both never negative, so it never turns back). The tests are its definition, as the step's are.
fn bump_ramp(t: f32) -> f32 {
    if t <= 0. {
        return 0.;
    }
    if t >= 1. {
        return t - 0.5;
    }
    let h = t * (1. / SLICES as f32);
    // Simpson's weights, the rule's own: 1 at the ends (the step is 0 at t = 0), 4 at the odd slices, 2 at the even.
    let mut sum = bump_step(t);
    for k in 1..SLICES {
        sum += bump_step(k as f32 * h) * if k & 1 == 1 { 4. } else { 2. };
    }
    sum * h * (1. / 3.)
}

/// Simpson's slices for `bump_ramp`: the ramp at t = 1 lands on ½ to float precision and its slope on 1 to under a thousandth (measured on the host), far under a step of a byte level.
const SLICES: u32 = 1 << 4;

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

/// A ring's share with the finger at row `y` (owner, 2026-11-12: "once your finger goes below the torch circle it goes from 0% to 100% at the screen midpoint"): exactly 0 at and above `bottom` (the circle's bottom edge), exactly 1 at and below `mid` (the screen's vertical middle), and between them `ease` stretched so its exact ends land on those two rows: its C∞ corners inside the run (each a fifth of it), so the level leaves 0 and reaches 1 with no kink, and the three fifths between are linear in the finger's place. Absolute: where the finger is, never how far it went.
pub fn level_at(y: f32, bottom: f32, mid: f32) -> f32 {
    ease(-EASE_HALF + (y - bottom) / (mid - bottom) * (1. + 2. * EASE_HALF))
}

/// A finger dragging down from a ring (`pull_to`): which ring; its bottom edge on the glass and the screen's middle (`level_at`'s two rows); whether the drag took hold (from then the finger's every move is the ring's), and whether the finger has gone below the bottom edge yet (until then nothing changes).
#[derive(Clone, Copy)]
struct Pull {
    k: usize,
    bottom: f32,
    mid: f32,
    started: bool,
    below: bool,
}

pub struct Quick {
    display: u64,
    grants: u64,
    /// The row's offset, whole pixels: 0 hidden, `slide` open (the display's op 56 reply says how far; until it has, a fallback of an eighth of the screen's harmonic span).
    pub off: i64,
    slide: i64,
    /// Where the row is headed after the lift (0 or `slide`), and whether it is on its way.
    target: i64,
    pub settling: bool,
    /// The offset and the finger's row when the drag took hold, and whether the row was open then.
    grab: Option<(i64, i64)>,
    from_open: bool,
    /// The shown frame the stepping last took.
    last: u32,
    /// The quick circle a finger holds, and a ring's drag down.
    held: Option<usize>,
    pull: Option<Pull>,
    /// The screen's vertical middle, where a ring dragged down reaches its whole level.
    mid: f32,
    /// The shares a drag shows (op 56), None while no finger drags that ring below its bottom edge.
    bright_arc: Option<f32>,
    torch_arc: Option<f32>,
    /// A screencap wanted once the row is hidden.
    capture: bool,
    /// The torch's level asked of it last, so a drag sends each new level once.
    torch_sent: u8,
    /// Whether the torch was asked its level since the row last started to show.
    torch_asked: bool,
    /// Op 56 as last sent: sent again only when something in it moved.
    sent: [u8; 15],
}

impl Quick {
    pub fn new(display: u64, grants: u64, w: i64, h: i64) -> Quick {
        let span = 2 * w * h / (w + h);
        Quick { display, grants, off: 0, slide: span >> 3, target: 0, settling: false, grab: None, from_open: false, last: 0, held: None, pull: None, mid: h as f32 * 0.5, bright_arc: None, torch_arc: None, capture: false, torch_sent: 0, torch_asked: false, sent: [0xEE; 15] }
    }

    /// A finger took hold of the row at screen row `y` (a drag that began in the doorway, on the top's ground, on a radio circle or on a quick circle): the stepping lets go of it, and it follows the finger from where it is.
    pub fn take(&mut self, y: i64, _frame: u32) {
        self.settling = false;
        self.grab = Some((self.off, y));
        self.from_open = self.off * 2 >= self.slide;
    }

    /// The finger, now at row `y`: the row moves with it one to one (owner, 2026-11-11: "track the finger down from top tho and slide it in"). WHY the bounds: the row slides between hidden and open, and has nowhere to be past either (the display holds its rows only between).
    pub fn drag(&mut self, y: i64, _frame: u32) {
        let Some((o0, y0)) = self.grab else { return };
        self.off = (o0 + y - y0).clamp(0, self.slide);
    }

    /// The finger lifted: the row goes open when it was pulled past a third of the way (or, pushed from open, unless pushed back past a third), else hidden, halving what is left each shown frame (`step`).
    pub fn release(&mut self, frame: u32) {
        if self.grab.take().is_none() {
            return;
        }
        let third = self.slide / 3;
        let open = if self.from_open { self.off > self.slide - third } else { self.off >= third };
        self.settle_to(if open { 1 } else { 0 }, frame);
    }

    /// Open (from hidden) or hidden (from open) by a tap.
    pub fn toggle(&mut self, frame: u32) {
        let open = !(self.target == self.slide && self.off > 0);
        self.settle_to(open as i64, frame);
    }

    /// Head open (1) or hidden (0), from wherever it is.
    pub fn settle_to(&mut self, open: i64, frame: u32) {
        self.grab = None;
        self.target = open * self.slide;
        self.settling = self.off != self.target;
        self.last = frame;
    }

    /// Shut at once, no slide (owner, 2026-11-11, on the Pro: in full screen the console is "gone gone"): full screen turned on by the map's own hand (the middle circle, deck's swipe). The display draws none of the row in full screen and gives none of it to the hit map, so a slide back would move nothing anyone sees; landed hidden here, the row is not found open when full screen goes off. A finger's grab or a held circle ends with it.
    pub fn shut(&mut self) {
        self.grab = None;
        self.held = None;
        self.pull = None;
        self.off = 0;
        self.target = 0;
        self.settling = false;
    }

    /// One step for each frame the panel showed since the last: what is left to go halves, the half of a pixel dropped (a right shift), so it lands in a few frames and then holds still (owner, 2026-11-11: "then do the iterate/2 each frame, like we do for the settings page until it snaps in or out of place"). Counted in shown frames, never a clock: a missed frame steps twice. Returns whether it still moves.
    pub fn step(&mut self, frame: u32) -> bool {
        if !self.settling {
            return false;
        }
        let steps = frame.wrapping_sub(self.last);
        self.last = frame;
        for _ in 0..steps {
            let left = self.target - self.off;
            self.off = self.target - left.signum() * (left.abs() >> 1);
            if self.off == self.target {
                self.settling = false;
                break;
            }
        }
        self.settling
    }

    /// A finger landed on quick circle `k`, whose disc on the glass is (x, y, r) as op 41 gave it: held (lighter), and on a ring its drag readied, its bottom edge y + r. A torch that does not answer has no level to set (the circle is drawn dimmed).
    pub fn hold(&mut self, k: usize, disc: Option<(f32, f32, f32)>) {
        self.held = Some(k);
        let Some((_, y, r)) = disc else { return };
        if !level(k) || k == TORCH && torch_now().is_none() {
            return;
        }
        self.pull = Some(Pull { k, bottom: y + r, mid: self.mid, started: false, below: false });
    }

    /// Whether a ring's drag took hold this touch: from then on the finger's every move is the ring's, whichever way it goes.
    pub fn pulling(&self) -> bool {
        self.pull.is_some_and(|p| p.started)
    }

    /// The finger dragging down from a ring is at row `y` (QUICK.md "Quick pages"; owner, 2026-11-12: "once your finger goes below the torch circle it goes from 0% to 100% at the screen midpoint"). Nothing until the finger first goes below the circle's bottom edge; from then the level is where the finger is (`level_at`): 0 at the bottom edge (the torch off, brightness the dimmest), the whole level at the screen's middle, following it back up too. Brightness applied live (kept at the lift, `let_go`), the torch asked for each new level (it lights with its own strobe, lit share and timeout, which it reads for itself).
    pub fn pull_to(&mut self, y: i64) {
        let Some(mut p) = self.pull else { return };
        p.started = true;
        p.below |= y as f32 > p.bottom;
        self.pull = Some(p);
        if !p.below {
            return;
        }
        let share = level_at(y as f32, p.bottom, p.mid);
        // The share as a byte, rounded: `level_at` is exactly 0 and 1 at its ends, so the byte runs 0 to 255.
        if p.k == BRIGHTNESS {
            self.bright_arc = Some(share);
            let v = (255. * (1. - share) + 0.5) as u8;
            if v != ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS) {
                ferros_sys::set_pref_live(cap(self.grants, S_SETTINGS), ferros_sys::PREF_BRIGHTNESS, v);
            }
        } else {
            self.torch_arc = Some(share);
            let v = (255. * share + 0.5) as u8;
            if v != self.torch_sent {
                self.torch_sent = v;
                torch_ask(Some(v));
            }
        }
    }

    /// The finger left a quick circle it held: a ring's drag ends (its level kept in the vault now, once: brightness as preference 3, the torch's as the level its page's on lights it at, preference 42), or a still finger taps (`tap`). Returns the page to open, if the tap opens one.
    pub fn let_go(&mut self, moved: bool, frame: u32) -> Option<&'static str> {
        let settings = cap(self.grants, S_SETTINGS);
        if self.pull.take().is_some_and(|p| p.below) {
            if self.bright_arc.is_some() {
                ferros_sys::set_pref(settings, ferros_sys::PREF_BRIGHTNESS, ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS));
            }
            // Dragged to off, the level its page lights it at stays what it was.
            if self.torch_arc.is_some() && self.torch_sent > 0 {
                ferros_sys::set_pref(settings, ferros_sys::PREF_TORCH_LEVEL, self.torch_sent);
            }
        }
        let held = self.held.take();
        let pulled = self.bright_arc.take().is_some() | self.torch_arc.take().is_some();
        match (held, moved, pulled) {
            (Some(k), false, false) => self.tap(k, frame),
            _ => None,
        }
    }

    /// A tap on quick circle k: a switch acts; a level returns its page to open.
    fn tap(&mut self, k: usize, frame: u32) -> Option<&'static str> {
        let settings = cap(self.grants, S_SETTINGS);
        let mut l = Line::new();
        let mut opens = None;
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
            // A level's tap opens its page (owner, 2026-11-12: "tap on torch brings up the torch config screen", and of brightness "tap brings you to the more granular screen settings page"): Appearance, the torch its own, whose on lights it at what was set last (preference 42, and the strobe, lit share and timeout it reads for itself; the tap did that until 2026-11-12). A torch that does not answer: nothing (the circle is drawn dimmed, and its page would set nothing).
            BRIGHTNESS => opens = page(k),
            TORCH => {
                if torch_now().is_some() {
                    opens = page(k);
                }
            }
            FULL_SCREEN => {
                // The display switches the app in front's full screen and says how it stands; with no app in front it refuses (status 3) and nothing changes. Turned on, the row is gone with the console (`shut`).
                let mut r = [0u8; 4];
                let n = call(self.display, &[43], &mut r);
                if n >= 2 && n != ferros_sys::ERR && r[0] == 0 && r[1] == 1 {
                    self.shut();
                }
                let _ = write!(l, "map: quick: {}\n", match (n >= 2 && n != ferros_sys::ERR, r[0], r[1]) {
                    (true, 0, 1) => "full screen",
                    (true, 0, _) => "full screen off",
                    _ => "full screen: no app in front",
                });
            }
            DND => {
                let on = !ferros_sys::dnd::on();
                dnd_ask(on);
                let _ = write!(l, "map: quick: do not disturb {}\n", if on { "on" } else { "off" });
            }
            SCREENCAP => {
                // The row shuts first; the display takes the capture once no circle of it is on the screen (op 56's flag), so it is not in the picture.
                self.capture = true;
                self.settle_to(0, frame);
                let _ = write!(l, "map: quick: screencap once the row is shut\n");
            }
            _ => {}
        }
        if !l.as_str().is_empty() {
            console(self.grants, l.as_str().as_bytes());
        }
        opens
    }

    /// Op 56 to the display when anything in it moved: the offset, the torch, the held circle, the rings' shares, and a screencap once the row has landed hidden. The reply says the slide's length, kept for the next drag.
    pub fn send(&mut self) {
        // The torch asked its level once each time the row starts to show: until it has answered (its first call starts it, docs/TORCH.md, and it ends itself again while dark), and while it was last lit (its timeout may have put it out since; a lit torch is running, so the ask starts nothing).
        if self.off > 0 && !self.torch_asked {
            self.torch_asked = true;
            if torch_now().is_none_or(|l| l > 0) {
                torch_ask(None);
            }
        }
        if self.off == 0 && !self.settling {
            self.torch_asked = false;
        }
        let mut m = [0u8; 15];
        m[0] = 56;
        m[1..5].copy_from_slice(&(self.off as i32).to_le_bytes());
        let torch = torch_now();
        let landed_hidden = self.off == 0 && !self.settling && self.grab.is_none();
        m[5] = torch.is_some() as u8 | ((self.capture && landed_hidden) as u8) << 1;
        m[6] = torch.unwrap_or(0);
        m[7] = self.held.map_or(0xFF, |k| k as u8);
        m[8] = 0xFF;
        // A share as Q16, G#FFFF for none. WHY the min: a full share (1, or the hair past it `ease` allows) would read as none on the wire; G#FFFE is a 65535th short of full, under a hundredth of a pixel of arc.
        let q16 = |v: Option<f32>| v.map_or(0xFFFF, |s| ((s * 65535.) as u16).min(0xFFFE)).to_le_bytes();
        let (b, t) = (q16(self.bright_arc), q16(self.torch_arc));
        m[9..11].copy_from_slice(&b);
        m[11..13].copy_from_slice(&t);
        if m == self.sent {
            return;
        }
        let mut r = [0u8; 4];
        let n = call(self.display, &m, &mut r);
        if n != ferros_sys::ERR {
            self.sent = m;
            // The slide's length as the display has it (a display from before the slide answers one byte, and the fallback stays).
            if n >= 3 {
                let s = u16::from_le_bytes([r[1], r[2]]) as i64;
                if s > 0 && s != self.slide {
                    self.slide = s;
                }
            }
            // The capture is the display's now.
            if m[5] & 2 != 0 {
                self.capture = false;
            }
        }
    }
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

/// Its settings changed (the torch page: its strobe, lit share or timeout, preferences 43-45): the torch reads them again and puts them in effect (its op 3), on the worker, after any level asked with it. Only while it is lit: a dark torch reads them at its next light, and asking a torch that ended would start one for nothing.
static TORCH_APPLY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
pub fn torch_apply() {
    if torch_now().is_some_and(|l| l > 0) {
        TORCH_APPLY.store(true, Ordering::Release);
        ASK.fetch_add(1, Ordering::AcqRel);
        ferros_sys::wake(&ASK, 1);
    }
}

/// Ask the identity service to switch the person's do not disturb (fgtw op 24), on the worker.
fn dnd_ask(on: bool) {
    DND_WANT.store(1 << 8 | on as u64, Ordering::Release);
    ASK.fetch_add(1, Ordering::AcqRel);
    ferros_sys::wake(&ASK, 1);
}

/// A haptic tap wanted (bit 8 set: a request; bits 0-7 the motor's ROM waveform), taken by the worker: a note arrived in view (notes.rs, docs/NOTIFY.md "Personal"). The NFC driver serves the motor (its op 8, docs/NFC.md), a call of some 25 ms the main loop never waits on.
static BUZZ_WANT: AtomicU64 = AtomicU64::new(0);

/// One tap of the motor's ROM waveform `w`, on the worker.
pub fn buzz(w: u8) {
    BUZZ_WANT.store(1 << 8 | w as u64, Ordering::Release);
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
        // A torch from before op 3 answers it with nothing: its settings then take at its next light.
        if TORCH_APPLY.swap(false, Ordering::AcqRel) {
            let mut r = [0u8; 4];
            let got = call(cap(grants, S_TORCH), &[3], &mut r);
            if got != ferros_sys::ERR && got >= 1 {
                TORCH_NOW.store(1 << 8 | r[0] as u64, Ordering::Release);
            }
        }
        let b = BUZZ_WANT.swap(0, Ordering::AcqRel);
        if b & 1 << 8 != 0 {
            let mut r = [0xFFu8; 1];
            let got = call(cap(grants, S_NFC), &[8, b as u8], &mut r);
            if got == ferros_sys::ERR || got == 0 || r[0] != 0 {
                console(grants, b"map: notes: the motor did not play (NFC switched off, or its driver not up)\n");
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
