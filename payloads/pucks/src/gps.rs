//! The GPS puck (docs/PUCKS.md; owner, 2026-11-17: "GPS, compass, shit that's useful!").
//!
//! - **The API it uses** is the one kists already have (GPS.md "On demand"): the `gps` right, a send right on the GPS driver's endpoint; SYS_CLAIM op 0 with that right as proof starts the driver, op 1 releases it; the driver's FIX (op 0: valid, fixes, age, latitude, longitude, altitude, error, speed, track), SATS (op 2: each signal's system, band, PRN, used, elevation, azimuth, C/N0) and WATCH (op 3: held until its fixes or messages move). No new API was needed. Behind attest: the kernel refuses the claim while nobody is present, and starts no GPS before a person attests (memory radios_at_attest_gps_locked).
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

/// The driver's SATS reply: n, msgs u32, age u32, then 25-byte records (payloads/gps sky.rs `REC`).
const REC: usize = 25;
const SATS_MAX: usize = 1 << 6;

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    let link = common::start(grants, false);
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
                let mut s = [0u8; 9 + REC * SATS_MAX];
                let got = call_live(gps, &[2], &mut s);
                if got != ferros_sys::ERR && got >= 9 {
                    // WHY the bounds: the reply is the driver's (at most its 64 records) and the face draws at most SIGNALS; whatever arrived past either is left out, never read past the reply.
                    let k = (s[0] as usize).min(SATS_MAX).min((got as usize - 9) / REC).min(SIGNALS);
                    for (i, g) in signals.iter_mut().take(k).enumerate() {
                        let r = &s[9 + i * REC..9 + (i + 1) * REC];
                        *g = Signal { used: r[3] & 1 == 1, el: r[4] as i8, az: u16::from_le_bytes([r[5], r[6]]) };
                    }
                    n = k;
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

ferros_sys::entry!(payload_main);
