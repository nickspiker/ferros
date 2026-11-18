//! The sun puck (docs/PUCKS.md): the local day's sunrise and sunset, golden and blue hours, the sun's height over the day, from ferros_sky (Meeus ch. 25 and 15; against USNO within 0.46 min) at the puck's kept place.
//! Refresh: at the next moment any of its words change (ferros_sky `next_deadline`: a crossing of any of the sun's levels, solar noon, the day's end); between, the display moves the sun along its curve.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common.rs"]
mod common;

use ferros_faces::sky;
use ferros_sky::SunDay;

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    let link = common::start(grants, false);
    common::start_place(grants);
    loop {
        let seen = common::rt::bell();
        let mut deadline = None;
        match (common::now_secs(), common::place()) {
            (Some(now), Some(p)) => {
                // The person's local day, in the offset their base shows (legal for decimal, standard for dozenal and hex: TIME.md).
                let d = SunDay::around(now, p, ferros_sys::zone() as f64);
                common::show(link, |w, focus| sky::sun(w, &d, focus));
                deadline = Some(ferros_faces::osc(ferros_sky::next_deadline(now, p, d.end)));
            }
            _ => common::show(link, |w, _| ferros_faces::sky(w).map(|_| ())),
        }
        common::wait(seen, deadline);
    }
}

ferros_sys::entry!(payload_main);
