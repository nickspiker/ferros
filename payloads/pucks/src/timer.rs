//! The timer puck (owner, 2026-11-28): a timer and a stopwatch in one circle, the ring draining counterclockwise as the time runs out (the stopwatch's filling counterclockwise with each minute), the time large in the middle (ferros_faces::timer, on the rotary face).
//!
//! - **Setting it** is a turn of the finger about the circle's centre while the puck is the centred one (display op 90, ferros_puck::rt `drags`), on a log dial: T = 1 s · 60^θ, θ in turns, so a turn is a minute, two an hour, three 60 hours. Relative: each step of the finger's angle (its smallest signed change, unwrapped across the seam) multiplies the time by 60^Δθ from wherever it is, clockwise up and counterclockwise down; kept in f64 seconds, nothing snapped and nothing bounded (the digits round only for reading).
//! - **Taps**: in the middle, start or pause; two taps on the ring (off the middle and the marks) near each other with no drag between, clear: judged by the taps' sequence and places alone, never by a time window. In focus (the circle at least half the panel) the presets 1, 3, 5, 10, 15 and 30 minutes and the mode marks (timer, stopwatch) are tapped where they are drawn.
//! - **What a drag turns**: the time left of a running or paused timer (and the time set with it, so the ring is full again), the time set of one waiting or done. The stopwatch is not set by hand: a drag on it does nothing.
//! - **It survives everything**: the mode, the time set, a running timer's end (Eagle time), a paused one's time left, a running stopwatch's start (Eagle time) and a stopped one's count are kept in the puck's own vault domain as a VSF document (`timer`) the moment they change, read back at start, so a kill, a relaunch or a reboot carries on; a timer that came due while the device was off reads zero at once. A drag's steps are kept at its lift: each step written would be some sixty vault commits a second, each flushed.
//! - **Done**: the ring empty and the digits zero. No alarm yet (the owner has not decided how a finished timer announces itself).
//! - **Refresh**: only while shown, at the next moment the fill's end moves half a pixel or the digits change, whichever is first (a deadline its content sets, never an interval); idle, paused, done or hidden, no deadline at all (hidden, the end time runs on by itself and the face is drawn fresh when shown).

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common.rs"]
mod common;

use ferros_faces::timer::{self, Hit, Timer};
use ferros_puck::{call, fmt, OSC};
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::{EtType, VsfType};

/// The domain key of the kept state.
const KEY: &[u8] = b"timer";
/// The time a timer holds before anyone set one: one turn of the dial, a minute.
const FIRST: f64 = 60.;
/// How near the second of two taps on the ring lands to the first to clear, Q14 of the radius: an eighth of it.
const NEAR: i64 = 1 << 11;
/// How far past a moment a deadline is set: the counter's tick that `ticks_at` floors to is 41 ns at 24.576 MHz, and a wake that lands before the digits' boundary only draws the same face again; 2^-20 s is about twenty ticks.
const PAST: i64 = OSC >> 20;

/// The timer: waiting at its time set, running to an end (Eagle oscillations; done once now has passed it), or paused with its time left.
#[derive(Clone, Copy, PartialEq)]
enum Run {
    Idle,
    /// i128: an end past 2^63 oscillations (205 years ahead) is a time the dial can set; f64 to i128 saturates past 1.2 · 10^29 s, 10^19 times the universe's age.
    Running { end: i128 },
    Paused { left: f64 },
}

/// The stopwatch: stopped at a count (seconds), or running since a moment (Eagle oscillations: the count is now less it).
#[derive(Clone, Copy, PartialEq)]
enum Watch {
    Stopped { elapsed: f64 },
    Running { since: i64 },
}

#[derive(Clone, Copy, PartialEq)]
struct State {
    /// The stopwatch is shown (else the timer).
    stopwatch: bool,
    /// The timer's time set, seconds.
    set: f64,
    run: Run,
    watch: Watch,
}

/// Seconds as Eagle oscillations.
fn osc(secs: f64) -> i128 {
    (secs * OSC as f64) as i128
}

impl State {
    /// The timer's time left at `now`: its time set while waiting, its time left while paused, zero once done.
    fn left(&self, now: i64) -> f64 {
        match self.run {
            Run::Idle => self.set,
            Run::Paused { left } => left,
            Run::Running { end } if end > now as i128 => (end - now as i128) as f64 / OSC as f64,
            Run::Running { .. } => 0.,
        }
    }
    fn done(&self, now: i64) -> bool {
        matches!(self.run, Run::Running { end } if end <= now as i128)
    }
    /// The stopwatch's count at `now`, seconds.
    fn elapsed(&self, now: i64) -> f64 {
        match self.watch {
            Watch::Stopped { elapsed } => elapsed,
            Watch::Running { since } => (now - since) as f64 / OSC as f64,
        }
    }
    /// A tap in the middle: start, pause, start again; a timer that is done starts again for its time set.
    fn toggle(&mut self, now: i64) {
        if self.stopwatch {
            self.watch = match self.watch {
                Watch::Stopped { elapsed } => Watch::Running { since: now - (elapsed * OSC as f64) as i64 },
                Watch::Running { since } => Watch::Stopped { elapsed: (now - since) as f64 / OSC as f64 },
            };
            return;
        }
        let now = now as i128;
        self.run = match self.run {
            Run::Idle => Run::Running { end: now + osc(self.set) },
            Run::Paused { left } => Run::Running { end: now + osc(left) },
            Run::Running { end } if end > now => Run::Paused { left: (end - now) as f64 / OSC as f64 },
            Run::Running { .. } => Run::Running { end: now + osc(self.set) },
        };
    }
    /// Two taps on the ring: the timer back to its time set, waiting; the stopwatch to zero, stopped.
    fn clear(&mut self) {
        if self.stopwatch {
            self.watch = Watch::Stopped { elapsed: 0. };
        } else {
            self.run = Run::Idle;
        }
    }
    /// The finger turned the dial by `d` turns: the time left of a running or paused timer, and the time set with it (the ring full again), or the time set of one waiting or done, multiplied by 60^d.
    fn turn(&mut self, d: f64, now: i64) {
        match self.run {
            Run::Running { end } if end > now as i128 => {
                let v = timer::turned(self.left(now), d);
                self.set = v;
                self.run = Run::Running { end: now as i128 + osc(v) };
            }
            Run::Paused { left } => {
                let v = timer::turned(left, d);
                self.set = v;
                self.run = Run::Paused { left: v };
            }
            Run::Idle | Run::Running { .. } => {
                self.set = timer::turned(self.set, d);
                self.run = Run::Idle;
            }
        }
    }
}

/// An end or a start as Eagle time: e6, the standard form, while it fits 64 bits, else e7.
fn eagle(t: i128) -> VsfType {
    VsfType::e(i64::try_from(t).map_or(EtType::e7(t), EtType::e6))
}

fn keep(domain: u64, st: &State, now: i64) -> bool {
    let mut s = VsfSection::new("timer");
    s.add_field("stopwatch", VsfType::u0(st.stopwatch));
    s.add_field("set", VsfType::f6(st.set));
    match st.run {
        Run::Idle => {}
        Run::Running { end } => s.add_field("end", eagle(end)),
        Run::Paused { left } => s.add_field("left", VsfType::f6(left)),
    }
    match st.watch {
        Watch::Running { since } => s.add_field("since", eagle(since as i128)),
        Watch::Stopped { elapsed } => s.add_field("elapsed", VsfType::f6(elapsed)),
    }
    vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_section_direct(s).build().is_ok_and(|d| ferros_sys::store_set(domain, KEY, &d))
}

fn load(domain: u64) -> Option<State> {
    let mut b = [0u8; 1 << 10];
    let n = ferros_sys::store_get(domain, KEY, &mut b)?;
    // WHY: the document is under 2^9 bytes; a longer value is none of this puck's.
    if n > b.len() {
        return None;
    }
    let doc = &b[..n];
    vsf::verification::is_original(doc).ok()?;
    let (h, end) = VsfHeader::decode(doc).ok()?;
    let sections = h.sections(doc, end).ok()?;
    let s = sections.iter().find(|s| s.name == "timer")?;
    let float = |name: &str| s.get_field(name)?.values.iter().find_map(|v| if let VsfType::f6(x) = v { Some(*x) } else { None });
    let when = |name: &str| {
        s.get_field(name)?.values.iter().find_map(|v| match v {
            VsfType::e(EtType::e5(t)) => Some(*t as i128),
            VsfType::e(EtType::e6(t)) => Some(*t as i128),
            VsfType::e(EtType::e7(t)) => Some(*t),
            _ => None,
        })
    };
    // Read off the device's own storage, but checked as any outside input: a time set must be a time (finite, more than zero) for the dial to turn it, a time left likewise, a count not negative.
    let set = float("set").filter(|t| t.is_finite() && *t > 0.)?;
    let stopwatch = s.get_field("stopwatch")?.values.iter().find_map(|v| v.as_u64())? != 0;
    let run = match (when("end"), float("left").filter(|t| t.is_finite() && *t > 0.)) {
        (Some(end), _) => Run::Running { end },
        (None, Some(left)) => Run::Paused { left },
        (None, None) => Run::Idle,
    };
    let watch = match (when("since").and_then(|t| i64::try_from(t).ok()), float("elapsed").filter(|t| t.is_finite() && *t >= 0.)) {
        (Some(since), _) => Watch::Running { since },
        (None, Some(elapsed)) => Watch::Stopped { elapsed },
        (None, None) => Watch::Stopped { elapsed: 0. },
    };
    Some(State { stopwatch, set, run, watch })
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    let link = common::start(grants, input, input_len, false);
    let domain = ferros_sys::cap(grants, common::S_DOMAIN);
    let mut st = load(domain).unwrap_or(State { stopwatch: false, set: FIRST, run: Run::Idle, watch: Watch::Stopped { elapsed: 0. } });
    let mut kept = st;
    // The finger's last angle in a drag (turns), None between drags; the first of two taps on the ring, None once anything else came between.
    let mut angle: Option<f64> = None;
    let mut armed: Option<(i16, i16)> = None;
    let mut refused = false;
    loop {
        let seen = common::rt::bell();
        let v = common::rt::view();
        // Before the kernel has read the clock there is nothing to time: the next view change asks again.
        let Some(now) = common::rt::eagle_now() else {
            common::wait(seen, None);
            continue;
        };
        let mut drags = [(0i16, 0i16, 0u8); call::DRAGS];
        let nd = common::rt::drags(&mut drags);
        for &(x, y, phase) in &drags[..nd] {
            armed = None;
            let a = timer::angle(x, y);
            if let Some(from) = angle.filter(|_| phase != call::DRAG_DOWN && !st.stopwatch) {
                st.turn(timer::unwrap(from, a), now);
            }
            angle = (phase != call::DRAG_UP).then_some(a);
        }
        let mut taps = [(0i16, 0i16); call::TAPS];
        let nt = common::rt::taps(&mut taps);
        for &(x, y) in &taps[..nt] {
            match timer::hit(x, y, v.large) {
                Hit::Centre => {
                    armed = None;
                    st.toggle(now);
                }
                Hit::Preset(minutes) => {
                    armed = None;
                    st.stopwatch = false;
                    st.set = (minutes * 60) as f64;
                    st.run = Run::Idle;
                }
                Hit::Mode(stopwatch) => {
                    armed = None;
                    st.stopwatch = stopwatch;
                }
                Hit::Rim => match armed {
                    Some((ax, ay)) if (x as i64 - ax as i64).pow(2) + (y as i64 - ay as i64).pow(2) < NEAR * NEAR => {
                        st.clear();
                        armed = None;
                    }
                    _ => armed = Some((x, y)),
                },
            }
        }
        // Kept the moment it changes, but for a drag under way, which is kept at its lift.
        if st != kept && angle.is_none() {
            if !keep(domain, &st, now) && !refused {
                refused = true;
                common::say(grants, b"timer: the vault refused its state; it runs on, and a restart starts from what was kept before\n");
            }
            kept = st;
        }
        let mut deadline = None;
        if v.shown {
            let base = ferros_sys::device_base() & 0x7F;
            let (secs, up, share, running, theta, rate) = if st.stopwatch {
                let e = st.elapsed(now);
                // The fill is the current minute's share, sweeping and starting again each minute as the clock's seconds ring does.
                (e, false, (e % 60. / 60.) as f32, matches!(st.watch, Watch::Running { .. }), None, 1. / 60.)
            } else {
                let l = st.left(now);
                (l, true, timer::drain(l, st.set), matches!(st.run, Run::Running { .. }) && !st.done(now), Some(timer::theta(st.set)), 1. / st.set)
            };
            let mut b = [0u8; fmt::OUT];
            let label = fmt::duration(secs, base, up, &mut b);
            let t = Timer { stopwatch: st.stopwatch, share, running, label, theta };
            common::show(link, |w, focus| timer::face(w, &t, focus));
            if running {
                // The earlier of the two moments the face changes: the fill's end half a pixel on, the digits' next reading (not a bound: both are deadlines, and the first to come is the one to wake for).
                let dt = timer::half_pixel(rate, v.d).min(fmt::duration_next(secs, base, up));
                deadline = Some(now + (dt * OSC as f64) as i64 + PAST);
            }
        }
        common::wait(seen, deadline);
    }
}

// The arc grows on demand to 2^26 (64 MiB of addresses): the scratch its square is drawn in before it goes thru the handle, as large as its circle asks (ferros_puck::rt `scratch`; at most two squares of `SIDE_MAX` fill the slot, one fills half of this).
ferros_sys::entry!(payload_main, grows ferros_sys::HEAP_DEVICE, starts common::HEAP_START_LOG2);
