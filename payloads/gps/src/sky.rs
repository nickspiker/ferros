//! The satellites the chip's on-chip engine reports with each fix (GPS.md "The satellites"): one record per tracked signal at the fix message's end, decoded into a fixed table, with how each satellite moved since the previous fix that had it.
//!
//! Pure: no system calls, so the same file decodes the 2026-10-25 capture on the host for the match against gpsd.

use crate::{bits, sbits};

/// The table's size.
/// A fix message is at most 512 bytes (`fix_msg`), 4096 bits; the records start at bit 585 or 617 and take 56 bits each, so the length check in `take` admits at most (4096 − 585) / 56 = 62 records.
pub const SATS: usize = 64;

/// One reply record on the endpoint (op 2 SATS), in bytes.
pub const REC: usize = 25;

/// No rate yet: fewer than two one-degree steps seen on that axis.
pub const NO_RATE: i32 = i32::MIN;

/// One axis's one-degree steps (the chip reports whole degrees): the value and the chip's ms clock at the first step seen and at the latest.
#[derive(Clone, Copy, Default)]
pub struct Steps {
    n: u8,
    v0: i32,
    t0: u32,
    v1: i32,
    t1: u32,
}

impl Steps {
    fn step(&mut self, v: i32, t: u32) {
        if self.n == 0 {
            (self.v0, self.t0, self.n) = (v, t, 1);
        } else {
            (self.v1, self.t1, self.n) = (v, t, 2);
        }
    }
    /// Millidegrees per second from the first step to the latest, or `NO_RATE`.
    /// The two steps were seen at two different fixes (`take` records a step only where the chip's clock moved), so t1 − t0 is at least one fix interval and never 0.
    pub fn rate(&self) -> i32 {
        if self.n < 2 {
            return NO_RATE;
        }
        ((self.v1 - self.v0) as i64 * 1_000_000 / self.t1.wrapping_sub(self.t0) as i64) as i32
    }
}

/// One tracked signal.
#[derive(Clone, Copy, Default)]
pub struct Sat {
    /// The chip's system code: 0 GPS, 8 GLONASS (the only two in the capture).
    pub sys: u8,
    /// The chip's band byte: 0 L1 (GLONASS G1), 2 L5.
    pub band: u8,
    /// The GPS PRN, or the GLONASS orbital slot (NMEA numbers it 64 + slot).
    pub prn: u8,
    /// Bit 0: used in the fix (gpsd's `$PGLOR,SAT` flag G#02).
    /// Bit 1: the chip's flag gpsd prints as G#20 in `$PGLOR,SAT` (meaning unknown).
    pub flags: u8,
    /// Elevation and azimuth, whole degrees.
    pub el: i8,
    pub az: u16,
    /// Carrier to noise in quarter dB-Hz: the chip's byte + 16 (gpsd's integer dB-Hz is this >> 2).
    pub cn0: u16,
    /// The change since the previous fix that had this signal (the azimuth's the short way round north), and the chip's ms between the two; `d_ms` 0 for a signal new in this fix.
    pub d_el: i16,
    pub d_az: i16,
    pub d_ms: u32,
    /// The azimuth unwrapped across north, for the steps; the chip's ms clock at this fix.
    azu: i32,
    lms: u32,
    pub el_steps: Steps,
    pub az_steps: Steps,
}

/// The latest fix message's satellites: two tables, the latest and the one before, swapped at each message (no copy).
pub struct Sky {
    t: [[Sat; SATS]; 2],
    n: [usize; 2],
    cur: usize,
    /// Fix messages whose satellites were decoded, and those whose record count did not fit the message (the layout moved: read nothing rather than misplaced fields).
    pub msgs: u32,
    pub bad: u32,
}

impl Sky {
    pub fn new() -> Self {
        Sky { t: [[Sat::default(); SATS]; 2], n: [0; 2], cur: 0, msgs: 0, bad: 0 }
    }

    /// The latest fix message's signals.
    pub fn sats(&self) -> &[Sat] {
        &self.t[self.cur][..self.n[self.cur]]
    }

    /// The records of a fix message `m` (from the class on, unescaped): a 32-bit block more ahead of them when bit 213 (the track) is set, the count u7 17 bits before the first, then 56 bits each — PRN u6, used u1, a bit always 1, the unnamed flag u1, a bit always 1, system u6, elevation s8, azimuth u16, band u8, C/N0 u8 — and the message ends 16 bits after its body, the records' end less one bit rounded up to a whole 32-bit word (all 213 messages of the capture; a 32-bit window, so it admits exactly one count).
    pub fn take(&mut self, m: &[u8]) {
        let s = 585 + 32 * bits(m, 213, 1) as usize;
        let n = bits(m, s - 17, 7) as usize;
        let next = self.cur ^ 1;
        // WHY: n is the chip's, and the start depends on flag bits only one of which varied in the capture.
        // All 213 fix messages in it end exactly here; a message that does not is laid out some other way, and reading it would put fields from the wrong place into the table (and an n past 64 would index past it).
        // This also bounds n ≤ 62 < SATS: m is at most 4096 bits.
        if m.len() * 8 != (s + 56 * n + 30) / 32 * 32 + 16 {
            self.bad += 1;
            self.n[next] = 0;
            self.cur = next;
            return;
        }
        let lms = bits(m, 231, 32);
        let [a, b] = &mut self.t;
        let (old, new) = if self.cur == 0 { (&*a, b) } else { (&*b, a) };
        let old = &old[..self.n[self.cur]];
        for r in 0..n {
            let o = s + 56 * r;
            let mut x = Sat {
                prn: bits(m, o, 6) as u8,
                flags: (bits(m, o + 6, 1) | bits(m, o + 8, 1) << 1) as u8,
                sys: bits(m, o + 10, 6) as u8,
                el: sbits(m, o + 16, 8) as i8,
                az: bits(m, o + 24, 16) as u16,
                band: bits(m, o + 40, 8) as u8,
                cn0: bits(m, o + 48, 8) as u16 + 16,
                ..Sat::default()
            };
            x.azu = x.az as i32;
            x.lms = lms;
            if let Some(p) = old.iter().find(|p| (p.sys, p.band, p.prn) == (x.sys, x.band, x.prn)) {
                x.d_ms = lms.wrapping_sub(p.lms);
                x.d_el = x.el as i16 - p.el as i16;
                // The chip reports 0..=360: the change the short way round north.
                let da = (x.az as i32 - p.az as i32 + 540).rem_euclid(360) - 180;
                x.d_az = da as i16;
                x.azu = p.azu + da;
                (x.el_steps, x.az_steps) = (p.el_steps, p.az_steps);
                if x.d_ms != 0 {
                    if x.d_el != 0 {
                        x.el_steps.step(x.el as i32, lms);
                    }
                    if da != 0 {
                        x.az_steps.step(x.azu, lms);
                    }
                }
            }
            new[r] = x;
        }
        self.n[next] = n;
        self.cur = next;
        self.msgs += 1;
    }

    /// Satellites tracked (a satellite seen on L1 and L5 counts once), those used in the fix, and the best C/N0 in quarter dB-Hz (0 with none).
    pub fn counts(&self) -> (u32, u32, u16) {
        let t = self.sats();
        let first = |k: usize, used: bool| !t[..k].iter().any(|p| (p.sys, p.prn) == (t[k].sys, t[k].prn) && (!used || p.flags & 1 == 1));
        let seen = (0..t.len()).filter(|&k| first(k, false)).count() as u32;
        let used = (0..t.len()).filter(|&k| t[k].flags & 1 == 1 && first(k, true)).count() as u32;
        (seen, used, t.iter().fold(0, |b, p| b.max(p.cn0)))
    }

    /// The endpoint's record for signal `k` of the latest fix (op 2 SATS, main.rs's header), `REC` bytes.
    pub fn record(&self, k: usize, out: &mut [u8]) {
        let x = &self.sats()[k];
        out[0] = x.sys;
        out[1] = x.band;
        out[2] = x.prn;
        out[3] = x.flags;
        out[4] = x.el as u8;
        out[5..7].copy_from_slice(&x.az.to_le_bytes());
        out[7..9].copy_from_slice(&x.d_el.to_le_bytes());
        out[9..11].copy_from_slice(&x.d_az.to_le_bytes());
        out[11..13].copy_from_slice(&x.cn0.to_le_bytes());
        out[13..17].copy_from_slice(&x.d_ms.to_le_bytes());
        out[17..21].copy_from_slice(&x.el_steps.rate().to_le_bytes());
        out[21..25].copy_from_slice(&x.az_steps.rate().to_le_bytes());
    }
}
