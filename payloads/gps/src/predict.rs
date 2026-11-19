//! The sky before a lock (GPS.md "Predicted birds"; owner, 2026-11-11: "can't we figure that out from the orbital data anyway?"; 2026-11-18: liked showing predicted birds before a lock, from a rough fix): every GPS, GLONASS, Galileo and BeiDou satellite above the horizon, from the orbits (`gps.orbits`, ferros_kept::gnss), the clock and the rough place (or the fix once there is one), published as tracks beside the ones the chip hears.
//!
//! - **The orbits** are CelesTrak's GNSS elements, fetched over plain HTTP thru the `ip` process when the kept ones are older than `FRESH_S` (or there are none), read by ferros_gnss's checked parser and kept as a VSF document stamped with when they were fetched. A failed fetch keeps the old ones, whatever their age (a few km a week of drift: still far under a degree of sky).
//! - **A predicted bird** is a track with C/N0 0, not used (no new bit in `ferros_sys::Sat`): the display and the GPS puck draw it faint. One the chip hears replaces it: the same system (or one the chip names by a code not known yet) within `MATCH_DEG` of it, or the same GPS or BeiDou number.
//! - **Refreshed** every `EVERY_S` while there are birds to show: a satellite crosses half a degree of sky a minute, so this is the drawn sky's own deadline (the loop's wait), not a poll of anything.

use crate::net;
use alloc::vec::Vec;
use ferros_gnss::{apart, sky, Bird, Elements, Orbit};
use ferros_sky::Place;
use ferros_sys::{cap, store_get_at, store_set, EAGLE_OSCILLATIONS_PER_SECOND as OPS};

/// Kept orbits older than this are fetched again: 2^16 s, some 18 hours (CelesTrak refreshes GNSS elements daily).
pub const FRESH_S: i64 = 1 << 16;
/// How often the predicted sky is worked out again while it shows: 2^6 s, some half a degree of a satellite's travel.
pub const EVERY_S: u64 = 1 << 6;
/// A heard signal within this many degrees of a predicted bird of its system is that bird.
const MATCH_DEG: f32 = (1 << 2) as f32;
/// The most birds kept: some 55 are above the horizon at once with the four systems (the host tests: 51 at Wellington, 54 at Tromsø above 5 degrees).
pub const BIRDS: usize = 1 << 7;

const HOST: &str = "celestrak.org";
const PATH: &str = "/NORAD/elements/gp.php?GROUP=gnss&FORMAT=tle";
const KEY: &[u8] = b"orbits";
/// The grant slot of `udp` (the kernel's list since 2026-11-18; a hand spawn adds `--use udp` last).
const S_UDP: u64 = 9;

pub struct Predict {
    pub orbits: Vec<Orbit>,
    /// When the orbits were fetched, Eagle oscillations; 0 with none.
    pub fetched: i64,
    pub birds: [Bird; BIRDS],
    pub n: usize,
    /// Each new sky moves it (the WATCH count and the tracks' publish).
    pub generation: u32,
    /// The counter at the last sky.
    pub at: u64,
}

impl Predict {
    pub fn new() -> Predict {
        Predict { orbits: Vec::new(), fetched: 0, birds: [Bird::default(); BIRDS], n: 0, generation: 0, at: 0 }
    }

    /// The kept orbits, whole (a value larger than one transfer read in pieces); false with none or one that does not read.
    pub fn load(&mut self, store: u64) -> bool {
        let mut head = [0u8; 1 << 4];
        let Some(total) = store_get_at(store, KEY, 0, &mut head) else { return false };
        let mut doc = alloc::vec![0u8; total];
        let mut got = 0;
        while got < total {
            // WHY the min: a transfer carries at most STORE_MAX.
            let k = (total - got).min(ferros_sys::STORE_MAX);
            if store_get_at(store, KEY, got, &mut doc[got..got + k]).is_none() {
                return false;
            }
            got += k;
        }
        let Some((fetched, els)) = ferros_kept::gnss::parse(&doc) else { return false };
        self.take(&els, fetched);
        true
    }

    fn take(&mut self, els: &[Elements], fetched: i64) {
        self.orbits = els.iter().map(Orbit::new).collect();
        self.fetched = fetched;
    }

    /// Whether the kept orbits want fetching again (none, or older than `FRESH_S` on the real clock).
    pub fn stale(&self) -> bool {
        let now = ferros_sys::eagle_now();
        self.orbits.is_empty() || now == 0 || (now - self.fetched) / OPS as i64 > FRESH_S
    }

    /// Whether this driver was given the network (`udp` at slot 9).
    pub fn can_fetch(grants: u64) -> bool {
        unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + grants + 0x200) as *const u32) as u64 > S_UDP }
    }

    /// Fetch, read, keep: Ok((satellites taken, sets refused)) or why not.
    pub fn fetch(&mut self, grants: u64, store: u64) -> Result<(u32, u32), &'static str> {
        let body = net::get(cap(grants, S_UDP), HOST, PATH)?;
        let mut els = Vec::new();
        let (taken, refused) = ferros_gnss::each_tle(&body, |e| els.push(e));
        if taken == 0 {
            return Err("the answer held no GNSS elements");
        }
        let fetched = ferros_sys::eagle_now();
        let doc = ferros_kept::gnss::build(&els, fetched, fetched).ok_or("the orbits' document did not build")?;
        let kept = store_set(store, KEY, &doc);
        self.take(&els, fetched);
        if !kept {
            return Err("fetched, but the vault refused gps.orbits");
        }
        Ok((taken, refused))
    }

    /// The sky now from `place` (radians); the counter `at` it was worked out at. Nothing without orbits or the real clock.
    pub fn sky(&mut self, place: Place, at: u64) {
        let now = ferros_sys::eagle_now();
        if self.orbits.is_empty() || now == 0 {
            return;
        }
        let t = now as f64 / OPS as f64;
        self.n = sky(&self.orbits, t, place, 0., &mut self.birds);
        self.at = at;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Whether a predicted bird is heard: `heard` as (system as `ferros_sys::Sat` numbers it, number, elevation, azimuth).
    pub fn is_heard(b: &Bird, heard: &[(u8, u8, i8, u16)]) -> bool {
        heard.iter().any(|&(sys, num, el, az)| {
            let same = sys == b.system || sys == 7;
            same && ((b.number != 0 && num == b.number && sys == b.system) || apart((el as f32, az as f32), (b.el, b.az)) < MATCH_DEG)
        })
    }
}
