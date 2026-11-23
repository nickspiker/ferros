//! The thermometer puck (owner, 2026-11-20: the Pro's IR thermometer into a `thermo` mailbox and a puck).
//!
//! - **Data path**: the thermo mailbox (ferros_mailbox::thermo; docs/SENSORS.md "The thermo mailbox"), which the sensors process publishes from the MLX90632's samples, opened with no proof (owner, 2026-11-20: open, no permission necessary) and read thru its handle (ferros_sys::mailbox `View`: the puck runs masked).
//! - **What it shows**: the object's temperature (what the back of the device points at) large, the sensor's own temperature smaller under it, as the AoC reports them; uncalibrated (the owner's blackbodies come later).
//!   The normal range while the object is inside it, the extended range past it (`NORMAL_MAX_C`).
//! - **Refresh**: only while its circle is shown, at each publish (about two a second: the AoC sends the samples in batches of half a second), and only when a number moved by `STEP_C` or the view changed. A worker thread waits on the mailbox's sequence while the circle is shown and rings the puck's bell; hidden, it waits on the bell instead, so a hidden puck wakes for nothing the mailbox does. No timer.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common.rs"]
mod common;

use ferros_faces::thermo::{face, Reading};
use ferros_sys::mailbox::{thermo, Got, View, THERMO};

/// Where the kernel puts the view's handle: the first pair past the slots the puck's build wrote, its grant count (0 the console, 1 to 6 the leading implicit rights, 7 `puck`, 8 to 10 the GPU's, 11 audio: ferros_kist `slots` for a capsule that declares `puck` alone).
const S_VIEW: u64 = 12;
/// The normal range's top as the AoC lists it (its sensor info's maximum, 42 °C; the extended range's is 200 °C): past it the extended range's object is shown.
const NORMAL_MAX_C: f32 = 42.;
/// The least move of either number that draws a new face: 2^-4 °C, under the shown place.
const STEP_C: f32 = 1. / (1 << 4) as f32;

/// The view, once opened, for the worker thread.
static mut VIEW: Option<View<S_VIEW>> = None;
static mut WORKER_STACK: [u8; 1 << 13] = [0; 1 << 13];

/// The worker: while the circle is shown, asleep on the mailbox's sequence, ringing the bell at each publish (and at the writer's end); hidden, asleep on the bell until the view changes.
extern "C" fn worker(_: u64) -> ! {
    let Some(v) = (unsafe { *(&raw const VIEW) }) else {
        loop {
            ferros_sys::sleep_until(u64::MAX);
        }
    };
    let mut seq = v.seq();
    loop {
        let b = common::rt::bell();
        if !common::rt::view().shown {
            common::rt::wait(b, None);
            continue;
        }
        seq = v.wait(seq, 0);
        common::rt::ring();
    }
}

/// The reading the face shows from both ranges: the normal range's while its object is inside it, else the extended range's when it has a sample.
fn reading(r: &[thermo::Range; thermo::RANGES]) -> Reading {
    let n = &r[thermo::NORMAL];
    let e = &r[thermo::EXTENDED];
    let use_extended = e.samples != 0 && (n.samples == 0 || n.object() >= NORMAL_MAX_C);
    let s = if use_extended { e } else { n };
    if s.samples == 0 {
        return Reading::default();
    }
    Reading { object: Some(s.object()), sensor: Some(s.sensor()) }
}

/// Whether two readings differ by `STEP_C` in either number, or in having one.
fn moved(a: &Reading, b: &Reading) -> bool {
    let d = |x: Option<f32>, y: Option<f32>| match (x, y) {
        (Some(x), Some(y)) => (x - y).abs() >= STEP_C,
        (x, y) => x.is_some() != y.is_some(),
    };
    d(a.object, b.object) || d(a.sensor, b.sensor)
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    let link = common::start(grants, input, input_len, false);
    let mut token = [0u8; 32];
    let view = View::<S_VIEW>::open(THERMO, 0, &mut token);
    match view {
        Some(_) => {
            unsafe { VIEW = view };
            if ferros_sys::thread_spawn(worker, unsafe { &mut *(&raw mut WORKER_STACK) }, 0) != 0 {
                common::say(grants, b"thermometer: no worker thread; the face follows the view alone\n");
            }
        }
        None => common::say(grants, b"thermometer: no thermo mailbox (a kernel from before 2026-11-20, or no handle at slot 12): the face stays empty\n"),
    }
    let mut words = [0u64; thermo::WORDS];
    let mut shown = Reading::default();
    let mut shown_view = None;
    loop {
        let seen = common::rt::bell();
        let v = common::rt::view();
        let now = match &view {
            Some(m) => match m.read(&mut words) {
                Got::Fresh { kind: thermo::KIND, .. } => thermo::decode(&words).map(|r| reading(&r)).unwrap_or_default(),
                // Another layout, no writer, or nothing yet: nothing to show.
                _ => Reading::default(),
            },
            None => Reading::default(),
        };
        // A new size or turn, or shown again: drawn whatever the numbers did.
        let fresh_view = shown_view != Some((v.shown, v.d, v.large, v.c, v.s));
        if v.shown && (fresh_view || moved(&now, &shown)) {
            shown = now;
            shown_view = Some((v.shown, v.d, v.large, v.c, v.s));
            common::show(link, |w, focus| face(w, &now, focus));
        } else if !v.shown {
            shown_view = None;
        }
        common::wait(seen, None);
    }
}

// The arc grows on demand to 2^26 (64 MiB of addresses): the scratch its square is drawn in before it goes thru the handle, as large as its circle asks (ferros_puck::rt `scratch`; at most two squares of `SIDE_MAX` fill the slot, one fills half of this).
ferros_sys::entry!(payload_main, grows 1 << 26);
