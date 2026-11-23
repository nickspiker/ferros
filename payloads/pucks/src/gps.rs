//! The GPS puck (docs/PUCKS.md; owner, 2026-11-17: "GPS, compass, shit that's useful!").
//!
//! - **The API it uses** is the one kists already have (GPS.md "On demand"): the `gps` right, a send right on the GPS driver's endpoint; SYS_CLAIM op 0 with that right as proof starts the driver, op 1 releases it; the driver's FIX (op 0: valid, fixes, age, latitude, longitude, altitude, error, speed, track) and WATCH (op 3: held until its fixes, its messages or its predicted sky move); the sky from the kernel's TRACKS for GPS, what the console's GPS circle reads (since 2026-11-18: the signals heard and the birds predicted from the orbits). No new API was needed. Behind attest: the kernel refuses the claim while nobody is present, and starts no GPS before a person attests (memory radios_at_attest_gps_locked).
//! - **When the radio is on**: only while its circle is in focus (a tap on it; the map's "the GPS screen turns it on while it is open", owner, 2026-11-10). At rest it shows what the last session found, kept in memory only; nothing of a fix is stored.
//! - **Refresh**: in focus, each time the driver's WATCH rings (its own messages, about one a second while it tracks); at rest, never (no deadline at all).
//! - **What it never shows**: the position. A puck shows how well the device knows where it is, never where that is.

#![no_std]
#![no_main]

extern crate alloc;

#[path = "common.rs"]
mod common;

use ferros_faces::gps::{face, Signal, Sky, SIGNALS};
use ferros_sys::{call_live, cap, claim};


#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    let link = common::start(grants, input, input_len, false);
    let gps = cap(grants, common::S_GPS);
    let mut signals = [Signal::default(); SIGNALS];
    let mut n = 0usize;
    let mut fix: Option<(u32, f64)> = None;
    loop {
        let seen = common::rt::bell();
        let v = common::rt::view();
        if v.large && claim::claim(claim::GPS, gps) {
            let mut count = 0u32;
            while common::rt::view().large {
                let mut f = [0u8; 33];
                if call_live(gps, &[0], &mut f) >= 33 && f[0] == 1 {
                    let age_ms = u32::from_le_bytes(f[5..9].try_into().unwrap()) as f64;
                    let err = u32::from_le_bytes(f[21..25].try_into().unwrap());
                    fix = common::now_secs().map(|t| (err, t - age_ms / 1000.));
                }
                // The sky as the driver publishes it (the kernel's TRACKS for GPS, as the console's GPS circle reads it; 2026-11-18): the signals the chip hears with their C/N0 and whether used, then the birds only predicted from the orbits (C/N0 0, not used).
                let mut b = [0u64; ferros_sys::TRACKS_MAX];
                // WHY the min: the count is the kernel's answer; only the words the buffer holds are read.
                let got = ferros_sys::tracks(ferros_sys::control::GPS, &mut b).min(b.len());
                n = 0;
                for s in b[..got].iter().filter_map(|&w| ferros_sys::sat_of(w)) {
                    // WHY the bound: the face draws at most SIGNALS; the rest are left out.
                    if n == SIGNALS {
                        break;
                    }
                    signals[n] = Signal { used: s.used, el: s.elevation, az: s.azimuth, cn0: s.cn0, predicted: s.cn0 == 0 && !s.used };
                    n += 1;
                }
                let sky = Sky { signals: &signals[..n], fix, live: true };
                common::show(link, |w, focus| face(w, &sky, focus));
                // The driver's doorbell: held until its fixes or messages move (about a second while it tracks). A driver still replaying its session answers ERR at once: an eighth of a second, then ask again.
                let mut wq = [3u8, 0, 0, 0, 0];
                wq[1..5].copy_from_slice(&count.to_le_bytes());
                let mut r = [0u8; 4];
                if call_live(gps, &wq, &mut r) == 4 {
                    count = u32::from_le_bytes(r);
                } else {
                    ferros_sys::sleep_until(ferros_sys::now() + (ferros_sys::freq() >> 3));
                }
            }
            claim::release(claim::GPS);
        }
        let sky = Sky { signals: &signals[..n], fix, live: false };
        common::show(link, |w, focus| face(w, &sky, focus));
        // At rest nothing changes until the view does: no deadline.
        common::wait(seen, None);
    }
}

// The arc grows on demand to 2^26 (64 MiB of addresses): the scratch its square is drawn in before it goes thru the handle, as large as its circle asks (ferros_puck::rt `scratch`; at most two squares of `SIDE_MAX` fill the slot, one fills half of this).
ferros_sys::entry!(payload_main, grows ferros_sys::HEAP_DEVICE, starts common::HEAP_START_LOG2);
