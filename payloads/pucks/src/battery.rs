//! The battery puck (owner, 2026-11-28: "rotary goodness around the temp readout, magnetic strength, stuff like that"; the battery first, so charging is seen): the charge round the rim and large in the middle, the current and the battery's temperature smaller under it.
//!
//! - **Data path**: the device's facts (ferros_sys `facts`: the share, the current, the temperature), which anyone reads, as the kernel samples the gauge (every 2 s while the screen is on); no right beyond its circle.
//! - **What it shows**: the rotary face (ferros_faces::rotary) over 0 to 100 per cent, clockwise; under the share the current in mA (negative draining; plain, as fmt has no mA unit) and the battery's temperature in °C. Charging (the current positive) fills the arc in `USED`, the GPS face's green; draining in `TEXT`.
//! - **Refresh**: only while its circle is shown. A worker thread sleeps on the facts' bell (SYS_BELL `BELL_FACTS`) and rings the puck's when the circle is shown; the kernel rings it when the share or the charging side changes. It rings nothing for the current's size or the temperature (kernel service.rs `bell_facts`: "a current that stays on its side of zero rings nothing"), so while shown the puck also looks at the facts 2^5 s after it last woke, and draws only when a shown number would move.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common.rs"]
mod common;

use core::sync::atomic::{AtomicU64, Ordering};
use ferros_faces::rotary::{face, Note, Rotary};
use ferros_faces::{TEXT, USED};
use ferros_puck::fmt::unit;

/// How long a shown puck goes without the facts' bell before it looks at the facts itself: the kernel's bell does not ring for the current's size or the battery's temperature, and these are the face's two small numbers.
/// A real deadline in the no-polling rule's sense, the only way those two reach the face; 2^5 s, so they lag by at most that, and only while the circle is on the glass.
const LOOK_S: i64 = 1 << 5;
/// The least move of the current (mA) or the temperature (°C) that draws a new face: 2^-4, under the last shown place in any base (one place in focus).
const STEP: f32 = 1. / (1 << 4) as f32;

/// The facts' bell word: the kernel writes the facts' change count here and wakes the worker.
static FACTS_WORD: AtomicU64 = AtomicU64::new(0);
static mut WORKER_STACK: [u8; 1 << 13] = [0; 1 << 13];

/// The worker: asleep on the facts' bell, ringing the puck's own when the circle is on the glass (a hidden puck draws nothing, and is drawn fresh when it is shown again).
extern "C" fn worker(_: u64) -> ! {
    loop {
        let seen = FACTS_WORD.load(Ordering::Acquire);
        ferros_sys::wait_world(&FACTS_WORD, seen, 0);
        if common::rt::view().shown {
            common::rt::ring();
        }
    }
}

/// What the face shows, from the facts: the share (per cent), the current (mA, positive charging) and the temperature (°C), each None while the kernel does not know it.
#[derive(Clone, Copy, PartialEq, Default)]
struct Battery {
    pct: Option<u64>,
    ma: Option<f32>,
    c: Option<f32>,
}

fn read() -> Battery {
    let f = ferros_sys::facts();
    Battery { pct: f.battery_pct(), ma: f.battery_ua().map(|ua| ua as f32 / 1000.), c: f.battery_temp_cc().map(|cc| cc as f32 / 100.) }
}

/// Whether two readings differ in a way the face shows: the share, the charging side, either small number by `STEP`, or in having one.
fn moved(a: &Battery, b: &Battery) -> bool {
    let d = |x: Option<f32>, y: Option<f32>| match (x, y) {
        (Some(x), Some(y)) => (x - y).abs() >= STEP,
        (x, y) => x.is_some() != y.is_some(),
    };
    let charging = |b: &Battery| b.ma.map(|m| m > 0.);
    a.pct != b.pct || charging(a) != charging(b) || d(a.ma, b.ma) || d(a.c, b.c)
}

fn draw(w: &mut ferros_puck::Writer, b: &Battery, focus: bool) -> Result<(), ferros_puck::Full> {
    let mut notes = [Note { value: 0., places: 0, unit: unit::NONE }; 2];
    let mut n = 0;
    if let Some(ma) = b.ma {
        notes[n] = Note { value: ma, places: 0, unit: unit::NONE };
        n += 1;
    }
    if let Some(c) = b.c {
        notes[n] = Note { value: c, places: 1, unit: unit::CELSIUS };
        n += 1;
    }
    let charging = b.ma.is_some_and(|m| m > 0.);
    let r = Rotary {
        value: b.pct.map(|p| p as f32),
        lo: 0.,
        hi: 100.,
        second: None,
        clockwise: true,
        unit: unit::PERCENT,
        places: 0,
        colour: if charging { USED } else { TEXT },
        notes: &notes[..n],
        label: None,
    };
    face(w, &r, focus)
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    let link = common::start(grants, input, input_len, false);
    if !ferros_sys::bell(ferros_sys::BELL_FACTS, &FACTS_WORD) {
        common::say(grants, b"battery: no facts bell (a kernel without bells); the face follows the view and its deadline alone\n");
    } else if ferros_sys::thread_spawn(worker, unsafe { &mut *(&raw mut WORKER_STACK) }, 0) != 0 {
        common::say(grants, b"battery: no worker thread; the face follows the view and its deadline alone\n");
    }
    let mut shown = Battery::default();
    let mut shown_view = None;
    loop {
        let seen = common::rt::bell();
        let v = common::rt::view();
        let mut deadline = None;
        if v.shown {
            let now = read();
            // A new size or turn, or shown again: drawn whatever the numbers did.
            let fresh_view = shown_view != Some((v.d, v.large, v.c, v.s));
            if fresh_view || moved(&now, &shown) {
                shown = now;
                shown_view = Some((v.d, v.large, v.c, v.s));
                common::show(link, |w, focus| draw(w, &now, focus));
            }
            deadline = common::rt::eagle_now().map(|t| t + LOOK_S * ferros_puck::OSC);
        } else {
            shown_view = None;
        }
        common::wait(seen, deadline);
    }
}

// The arc grows on demand to 2^26 (64 MiB of addresses): the scratch its square is drawn in before it goes thru the handle, as large as its circle asks (ferros_puck::rt `scratch`; at most two squares of `SIDE_MAX` fill the slot, one fills half of this).
ferros_sys::entry!(payload_main, grows ferros_sys::HEAP_DEVICE, starts common::HEAP_START_LOG2);
