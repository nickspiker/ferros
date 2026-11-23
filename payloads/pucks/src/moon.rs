//! The moon puck (docs/PUCKS.md): the moon drawn lit as it is, its illuminated share, the next principal phase, its rise and set, from ferros_sky (Meeus ch. 47, 48, 49 and 15; quarters against USNO's 2026 list within 0.78 min) at the puck's kept place.
//! Refresh: the earliest of ferros_sky `next_deadline` (moonrise, moonset, the next quarter, the day's end) and the moment the shown share turns over to the next whole percent, found from its rate now: the content's own deadline, a few hours apart at the quarters and longer near new and full.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common.rs"]
mod common;

use ferros_faces::sky;
use ferros_sky::{Illumination, MoonDay, Place};

/// When the share shown as a whole percent next changes, after `now`: half a percent's distance to the next rounding boundary over the share's rate now (its change over 2^10 s), at least a minute so a share standing still at full or new does not ask at once.
fn percent_turns(now: f64, p: Place) -> f64 {
    let f0 = Illumination::at(now, p).fraction * 100.;
    let dt = (1 << 10) as f64;
    let rate = (Illumination::at(now + dt, p).fraction * 100. - f0) / dt;
    let next = if rate >= 0. { libm::floor(f0 + 0.5) + 0.5 } else { libm::ceil(f0 - 0.5) - 0.5 };
    // WHY the floor of a minute: at full and new the rate passes thru zero, and the time to the next boundary grows without bound or divides by nothing; a minute is finer than the share can change there.
    now + ((next - f0) / rate).abs().max(60.)
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    let link = common::start(grants, input, input_len, false);
    common::start_place(grants);
    loop {
        let seen = common::rt::bell();
        let mut deadline = None;
        match (common::now_secs(), common::place()) {
            (Some(now), Some(p)) => {
                let d = MoonDay::around(now, p, ferros_sys::zone() as f64);
                common::show(link, |w, focus| sky::moon(w, &d, focus));
                deadline = Some(ferros_faces::osc(ferros_sky::next_deadline(now, p, d.end).min(percent_turns(now, p))));
            }
            _ => common::show(link, |w, _| ferros_faces::sky(w).map(|_| ())),
        }
        common::wait(seen, deadline);
    }
}

// The arc grows on demand to 2^26 (64 MiB of addresses): the scratch its square is drawn in before it goes thru the handle, as large as its circle asks (ferros_puck::rt `scratch`; at most two squares of `SIDE_MAX` fill the slot, one fills half of this).
ferros_sys::entry!(payload_main, grows ferros_sys::HEAP_DEVICE, starts common::HEAP_START_LOG2);
