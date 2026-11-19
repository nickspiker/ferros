//! The broadcast ephemerides the chip gets in place of the replay's LTO windows (docs/GPS.md "Built: the driver's windows from gps.eph"): `gps.eph` (ferros_kept `eph`, device class), and the replay's windows rebuilt from it (ferros_gnss `windows::rebuild`).
//! The replay's own windows (Broadcom's LTO, from the capture) stay the fallback: with no set within reach of now, the replay goes as it was stored.
//! How the device gets fresh sets (owner, 2026-11-19): from the public HTTPS sources (BKG, ESA) once ferros has TLS, fleet-first, never a mirror on fgtw.org; staged after RAW video. Until then the host stores them (`gps-trace eph`, `ferros-bridge store put gps.eph`).

use alloc::vec::Vec;
use ferros_gnss::windows::Set;
use ferros_sys::store_get_at;

const KEY: &[u8] = b"eph";
/// A set is used while its toe is within this of now: a broadcast set is fitted for 4 hours around its toe, so 2^13 s (2.3 hours) is its reach and a little.
pub const REACH_S: f64 = (1u64 << 13) as f64;

/// The kept document's sets and when it was fetched; None when there is none or it does not read.
pub fn load(store: u64) -> Option<(i64, Vec<Set>)> {
    let mut head = [0u8; 1 << 4];
    let total = store_get_at(store, KEY, 0, &mut head)?;
    let mut doc = alloc::vec![0u8; total];
    let mut got = 0;
    while got < total {
        // WHY the min: a transfer carries at most STORE_MAX.
        let k = (total - got).min(ferros_sys::STORE_MAX);
        store_get_at(store, KEY, got, &mut doc[got..got + k])?;
        got += k;
    }
    ferros_kept::eph::parse(&doc)
}

/// The sets within reach of GPS second `now` (the continuous week times 604800 plus the second of week).
pub fn fresh(sets: &[Set], now: f64) -> Vec<Set> {
    sets.iter()
        .copied()
        .filter(|s| {
            let d = now - (s.eph.week as f64 * 604_800. + s.eph.toe);
            d < REACH_S && d > -REACH_S
        })
        .collect()
}
