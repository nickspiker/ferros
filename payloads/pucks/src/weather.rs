//! The weather puck, small (owner, 2026-11-17, on weather: "your call, don't go nuts").
//!
//! - **Data path**: one POST of a complete VSF document to fgtw.org over plain HTTP (the `ip` process, the `udp` right), section `weather_ask` carrying only the ~10 km cell of the kept place (ferros_weather's grid, rounded on the device before anything leaves it) and the local day's start; fgtw.org (fgtw-bootstrap branch `pucks-weather`) fetches MET Norway's forecast server side, caches it per cell, and answers a small VSF document (now, the next hours, today's high and low, its next update), every value read here by name and type marker, refused whole on any fault. No JSON reaches the device.
//! - **Refresh**: at the forecast's own deadline (`Forecast::next_deadline`: the next hour it shows, or the upstream's next update, whichever is first), and only while its circle is shown: a hidden weather puck asks nothing; shown again past its deadline it asks at once.
//! - **Not yet**: the answer is not signed by fgtw.org's key, so on plain HTTP someone on the path could forge the weather (they could only withhold or alter a forecast, never learn more than the cell, which the ask names anyway).

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common.rs"]
mod common;
mod net;

use ferros_faces::weather::face;
use ferros_weather::{Ask, Forecast};

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    let link = common::start(grants, false);
    common::start_place(grants);
    let udp = ferros_sys::cap(grants, common::S_UDP);
    let mut forecast: Option<Forecast> = None;
    let mut asked_cell = None;
    // When the forecast held goes stale (its next_deadline as it was fetched), and when a failed ask is tried again.
    let mut due: i64 = 0;
    let mut retry: Option<i64> = None;
    loop {
        let seen = common::rt::bell();
        let v = common::rt::view();
        let now = ferros_puck::rt::eagle_now();
        let cell = ferros_puck::rt::place::current().map(|(c, _)| c);
        let stale = match (&forecast, now) {
            (Some(_), Some(t)) => t >= due || asked_cell != cell,
            _ => true,
        };
        if v.shown && stale {
            if let (Some(c), Some(t)) = (cell, now) {
                // The local day's start in the person's zone (the high and low are the rest of that day).
                let zone = ferros_sys::zone() * ferros_puck::OSC;
                let day = 86_400 * ferros_puck::OSC;
                let day_start = (t + zone).div_euclid(day) * day - zone;
                let ask = Ask { cell: c, day_start };
                if let Ok(doc) = ask.build(t) {
                    match net::post(udp, &doc).map(|b| Forecast::parse(&b)) {
                        // The answer must be for the cell asked: another cell's weather is not this place's.
                        Some(Ok(f)) if f.cell == c => {
                            due = f.next_deadline(t);
                            forecast = Some(f);
                            asked_cell = cell;
                            retry = None;
                        }
                        _ => {
                            common::say(grants, b"weather: no forecast this time\n");
                            // WHY a retry deadline: nothing rings when the network comes back; 2^8 s, a retransmit's kind of deadline, only while shown.
                            retry = Some(t + (1 << 8) * ferros_puck::OSC);
                        }
                    }
                }
            }
        }
        match &forecast {
            Some(f) => common::show(link, |w, focus| face(w, f, focus)),
            None => common::show(link, |w, _| ferros_faces::sky(w).map(|_| ())),
        }
        // Hidden: no deadline at all (the view's bell wakes it when shown). Shown: the forecast's own.
        let deadline = match (&forecast, now, v.shown) {
            (_, _, true) if retry.is_some() => retry,
            (Some(_), Some(_), true) => Some(due),
            _ => None,
        };
        common::wait(seen, deadline);
    }
}

// The arc grows on demand to 2^26 (64 MiB of addresses): the scratch its square is drawn in before it goes thru the handle, as large as its circle asks (ferros_puck::rt `scratch`; at most two squares of `SIDE_MAX` fill the slot, one fills half of this).
ferros_sys::entry!(payload_main, grows 1 << 26);
