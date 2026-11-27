//! The pure parts of the ip process's client TCP (payloads/ip; owner, 2026-11-11: "sounds like it needs re-engineered", after Mahere's 1.6 MB base cells crawled thru a 4 KiB window).
//! Nothing here touches the machine: ip keeps the connections and their buffers, sends and receives the segments, and asks these functions what the protocol says.
//! Each piece is tested on the host (`cargo test -p ferros_tcp`).
//!
//! - Sequence numbers compare modulo 2^32 (RFC 9293 3.4): [`lt`], [`le`].
//! - [`Ooo`]: the ranges taken in beyond the next expected byte, kept until the hole before them fills, and the SACK blocks that describe them (RFC 2018).
//! - [`Opts`]: a segment's options read (MSS, window scale, SACK-permitted, SACK blocks), every length checked; [`syn_options`] and [`sack_option`] write ours.
//! - [`window`]: the receive window to advertise, its right edge moved only by a useful amount (silly-window avoidance, RFC 9293 3.8.6.2.2), and [`window_update_due`].
//! - [`Rtt`]: the retransmission timeout from measured round trips (RFC 6298), with the doubling back-off.
//! - [`Cc`]: slow start, congestion avoidance, fast retransmit and NewReno recovery (RFC 5681, RFC 6582).
//! - [`mss_for`]: the segment size a link's MTU allows.

#![no_std]

/// `a` before `b` in sequence space.
pub fn lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

/// `a` before or at `b` in sequence space.
pub fn le(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) <= 0
}

/// The out-of-order ranges a connection holds at once: eight, more than a window's worth of separate losses in practice; a ninth that joins none is not kept (its bytes come again, in order or retransmitted).
pub const OOO_N: usize = 1 << 3;

/// The most SACK blocks an ACK carries: four, which with two NOPs fill the 36 bytes of a 40-byte option space no timestamps share (RFC 2018 3).
pub const SACK_N: usize = 1 << 2;

/// Ranges [start, end) of sequence space taken in past the next expected byte, ascending and apart (no two touch), and the range the latest segment landed in (the first SACK block, RFC 2018 4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ooo {
    pub r: [(u32, u32); OOO_N],
    pub n: usize,
    /// The start of a byte of the segment taken in last; its range goes first in a SACK option.
    pub latest: u32,
}

impl Ooo {
    pub const EMPTY: Ooo = Ooo { r: [(0, 0); OOO_N], n: 0, latest: 0 };

    /// Keep [s, e) (s before e, both past the next expected byte): joined with every range it touches or overlaps, else placed in order. False when it joins none and all `OOO_N` places are taken: nothing changed then.
    pub fn insert(&mut self, s: u32, e: u32) -> bool {
        let (mut s, mut e) = (s, e);
        let mut kept = [(0u32, 0u32); OOO_N];
        let mut k = 0;
        let mut joined = false;
        for &(a, b) in &self.r[..self.n] {
            // Apart: ends before s or starts after e.
            if lt(b, s) || lt(e, a) {
                kept[k] = (a, b);
                k += 1;
            } else {
                joined = true;
                if lt(a, s) {
                    s = a;
                }
                if lt(e, b) {
                    e = b;
                }
            }
        }
        if !joined && k == OOO_N {
            return false;
        }
        // In order: the first kept range that starts after e takes the place after the new one.
        let at = kept[..k].iter().position(|&(a, _)| lt(e, a)).unwrap_or(k);
        self.r[..at].copy_from_slice(&kept[..at]);
        self.r[at] = (s, e);
        self.r[at + 1..k + 1].copy_from_slice(&kept[at..k]);
        self.n = k + 1;
        true
    }

    /// The next expected byte once the ranges that start at or before `nxt` are taken: the new next byte (`nxt` or past it), those ranges dropped.
    pub fn advance(&mut self, mut nxt: u32) -> u32 {
        let mut gone = 0;
        while gone < self.n && le(self.r[gone].0, nxt) {
            if lt(nxt, self.r[gone].1) {
                nxt = self.r[gone].1;
            }
            gone += 1;
        }
        self.r.copy_within(gone..self.n, 0);
        self.n -= gone;
        nxt
    }

    /// The bytes held.
    pub fn bytes(&self) -> u32 {
        self.r[..self.n].iter().map(|&(a, b)| b.wrapping_sub(a)).sum()
    }

    /// The SACK blocks for the next ACK: the range holding the latest segment first, then the others from the lowest, `SACK_N` at most; how many.
    pub fn sack_blocks(&self, out: &mut [(u32, u32); SACK_N]) -> usize {
        let first = self.r[..self.n].iter().position(|&(a, b)| le(a, self.latest) && lt(self.latest, b));
        let mut k = 0;
        if let Some(f) = first {
            out[0] = self.r[f];
            k = 1;
        }
        for (i, &x) in self.r[..self.n].iter().enumerate() {
            if k == SACK_N {
                break;
            }
            if Some(i) != first {
                out[k] = x;
                k += 1;
            }
        }
        k
    }
}

/// A segment's options as read: its MSS (0 when it gave none), its window scale shift (None when it gave none), whether it permits SACK, and the SACK blocks it carried.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opts {
    pub mss: u16,
    pub ws: Option<u8>,
    pub sack_ok: bool,
    pub sack: [(u32, u32); SACK_N],
    pub nsack: usize,
}

/// The largest window scale shift (RFC 7323 2.3): a larger one is taken as 14, as the RFC says to.
pub const WS_MAX: u8 = 14;

/// Read the options of a TCP header (the bytes from offset 20 to the data offset). The bytes are the peer's: every length is checked against what is there, and an option that runs past the end ends the walk with what was read so far.
pub fn parse(o: &[u8]) -> Opts {
    let mut p = Opts { mss: 0, ws: None, sack_ok: false, sack: [(0, 0); SACK_N], nsack: 0 };
    let mut i = 0;
    while i < o.len() {
        match o[i] {
            0 => break,
            1 => i += 1,
            kind => {
                // WHY the checks: the length byte is the peer's; one missing, under 2 (no progress) or running past the header would read outside the options.
                if i + 1 >= o.len() {
                    break;
                }
                let l = o[i + 1] as usize;
                if l < 2 || i + l > o.len() {
                    break;
                }
                let b = &o[i + 2..i + l];
                match (kind, l) {
                    (2, 4) => p.mss = u16::from_be_bytes([b[0], b[1]]),
                    // WHY the bound: RFC 7323 2.3, a shift over 14 is used as 14 (a window past 2^30 would be read as negative in sequence space).
                    (3, 3) => p.ws = Some(if b[0] > WS_MAX { WS_MAX } else { b[0] }),
                    (4, 2) => p.sack_ok = true,
                    (5, _) if (l - 2) % 8 == 0 => {
                        for blk in b.chunks_exact(8) {
                            // WHY: the option's length is the peer's; up to four blocks fit 40 bytes, more are not kept.
                            if p.nsack == SACK_N {
                                break;
                            }
                            p.sack[p.nsack] = (u32::from_be_bytes([blk[0], blk[1], blk[2], blk[3]]), u32::from_be_bytes([blk[4], blk[5], blk[6], blk[7]]));
                            p.nsack += 1;
                        }
                    }
                    _ => {}
                }
                i += l;
            }
        }
    }
    p
}

/// Our SYN's options: MSS `mss`, window scale `ws`, SACK permitted, each word aligned with NOPs (12 bytes).
pub fn syn_options(mss: u16, ws: u8) -> [u8; 12] {
    let m = mss.to_be_bytes();
    [2, 4, m[0], m[1], 1, 3, 3, ws, 1, 1, 4, 2]
}

/// A SACK option for `blocks` (`SACK_N` at most) into `out`, two NOPs ahead of it so it ends on a word: its length (0 for no blocks).
pub fn sack_option(blocks: &[(u32, u32)], out: &mut [u8; 4 + 8 * SACK_N]) -> usize {
    if blocks.is_empty() {
        return 0;
    }
    let n = blocks.len();
    out[0] = 1;
    out[1] = 1;
    out[2] = 5;
    out[3] = (2 + 8 * n) as u8;
    for (k, &(a, b)) in blocks.iter().enumerate() {
        out[4 + 8 * k..8 + 8 * k].copy_from_slice(&a.to_be_bytes());
        out[8 + 8 * k..12 + 8 * k].copy_from_slice(&b.to_be_bytes());
    }
    4 + 8 * n
}

/// The receive window to advertise: `free` the room past the next expected byte, `left` what the last advertisement still allows (its right edge less the next expected byte), `total` the most the window can be (the buffer, or 65535 without window scaling), `rmss` our segment size.
/// The right edge moves only when it can move by `min(total / 2, rmss)` (receiver silly-window avoidance, RFC 9293 3.8.6.2.2): a window opened a few bytes at a time invites segments of a few bytes.
pub fn window(free: u32, left: u32, total: u32, rmss: u32) -> u32 {
    let step = if total / 2 < rmss { total / 2 } else { rmss };
    if free >= left + step {
        free
    } else if left <= free {
        left
    } else {
        // WHY: never more than the room there is (the peer would send what has nowhere to go); `left` stays within `free` while the buffer only drains, so this is the defence against a caller's slip, not a case the stack makes.
        free
    }
}

/// Whether the room an app's read made deserves a window update of its own (ACKs carry the window anyway while data flows): the advertised window had shrunk under half of `total` and the room is now half or more, so a sender held back by a small window starts again without waiting for its persist timer.
pub fn window_update_due(free: u32, left: u32, total: u32) -> bool {
    left < total / 2 && free >= total / 2
}

/// The window field for a window of `w` bytes at shift `ws`, and the window that field says (what the peer may send): `w` rounded down to the scale's unit, but never under `left`, what the last advertisement still allows, rounded up (the right edge never moves back, RFC 9293 3.8.6.2.2; rounding down an edge kept where it was would pull it back up to a unit at every ACK, as Linux's `tcp_select_window` also avoids). The few bytes past the room that rounding up can say are trimmed on arrival like any overrun.
pub fn scaled(w: u32, left: u32, ws: u8) -> (u16, u32) {
    let mut f = w >> ws;
    let up = (left + (1 << ws) - 1) >> ws;
    if f < up {
        f = up;
    }
    // WHY: the caller sizes `total` as at most 65535 << ws, so f fits 16 bits; the bound holds the field to it should a caller pass more.
    let f = if f > 0xFFFF { 0xFFFF } else { f };
    (f as u16, f << ws)
}

/// The segment size a link of MTU `mtu` carries for family `fam` (4 or 6): the MTU less the IP and TCP headers. An MTU outside what the packet buffers hold (1500) or under each family's minimum (576 for IPv4, 1280 for IPv6) is not believed: 1200, which every path carries.
pub fn mss_for(mtu: u16, fam: u8) -> u16 {
    let (min, head) = if fam == 4 { (576, 40) } else { (1280, 60) };
    if mtu < min || mtu > 1500 { MSS_FALLBACK } else { mtu - head }
}

/// The segment size when the link says nothing believable: 1200, the size this stack used for everything until 2026-11-11.
pub const MSS_FALLBACK: u16 = 1200;

/// The peer's segment size when its SYN carries no MSS option: 536 for IPv4, 1220 for IPv6 (RFC 9293 3.7.1, RFC 8200 5).
pub fn mss_default(fam: u8) -> u16 {
    if fam == 4 { 536 } else { 1220 }
}

/// The retransmission timeout from measured round trips (RFC 6298), in timer ticks: the smoothed round trip, its variation, the timeout they give and the one in force (doubled by each expiry).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rtt {
    pub srtt: u64,
    pub var: u64,
    pub rto: u64,
    pub cur: u64,
    pub have: bool,
}

impl Rtt {
    /// Before any sample: the initial timeout `initial` (RFC 6298 2.1: one second).
    pub const fn new(initial: u64) -> Rtt {
        Rtt { srtt: 0, var: 0, rto: initial, cur: initial, have: false }
    }

    /// A round trip of `r` ticks measured (RFC 6298 2.2, 2.3; alpha 1/8, beta 1/4, K 4): the timeout is the smoothed round trip plus four variations (`g`, the clock's granularity, at least), held to [`min`, `max`]; the back-off ends.
    /// WHY the bounds: RFC 6298 2.4 and 2.5 (a floor so a quiet path's tiny variance never fires early, a ceiling of at least 60 s); the floor is ip's choice below the RFC's one second, as Linux's 200 ms is.
    pub fn sample(&mut self, r: u64, g: u64, min: u64, max: u64) {
        if !self.have {
            self.srtt = r;
            self.var = r / 2;
            self.have = true;
        } else {
            let d = self.srtt.abs_diff(r);
            self.var = self.var - self.var / 4 + d / 4;
            self.srtt = self.srtt - self.srtt / 8 + r / 8;
        }
        let k = if 4 * self.var > g { 4 * self.var } else { g };
        let t = self.srtt + k;
        self.rto = if t < min { min } else if t > max { max } else { t };
        self.cur = self.rto;
    }

    /// An expiry: the timeout doubles (RFC 6298 5.5), up to `max`.
    pub fn backoff(&mut self, max: u64) {
        self.cur = if self.cur > max / 2 { max } else { self.cur * 2 };
    }

    /// New data acknowledged: the back-off ends (RFC 6298 5.7 recomputes on the next sample; the timeout the samples gave is the one to use until then).
    pub fn reset(&mut self) {
        self.cur = self.rto;
    }
}

/// Congestion control (RFC 5681 with NewReno's recovery, RFC 6582), in bytes: the congestion window, the slow-start threshold, duplicate ACKs in a row, and the recovery point while recovering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cc {
    pub cwnd: u32,
    pub ssthresh: u32,
    pub dup: u32,
    pub recover: u32,
    pub recovering: bool,
}

/// What the sender does after an ACK.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    /// Send as the windows allow.
    Go,
    /// Resend the first unacknowledged segment first (fast retransmit, or a partial ACK in recovery).
    Resend,
}

impl Cc {
    /// A new connection whose first sequence number is `iss`: the initial window of RFC 5681 3.1 (four segments of a small MSS, three of an Ethernet-sized one: min(4·SMSS, max(2·SMSS, 4380))), no threshold yet, the recovery point at `iss` (RFC 6582 3.2 step 1).
    pub fn new(smss: u32, iss: u32) -> Cc {
        let two = 2 * smss;
        let iw = if 4 * smss < 4380 { 4 * smss } else if two > 4380 { two } else { 4380 };
        Cc { cwnd: iw, ssthresh: u32::MAX >> 1, dup: 0, recover: iss, recovering: false }
    }

    /// The threshold after a loss: half of what was in flight, two segments at least (RFC 5681 equation 4).
    fn half(flight: u32, smss: u32) -> u32 {
        if flight / 2 > 2 * smss { flight / 2 } else { 2 * smss }
    }

    /// An ACK of `acked` new bytes, `ack` the new first unacknowledged byte, `flight` what is outstanding after it, `limited` whether the sender was using its congestion window before it (in flight within a segment of it).
    /// The window grows only while it limits the sender (RFC 5681 3.1's note, RFC 7661): it can then never pass what the send buffer holds plus two segments, so it cannot run away.
    pub fn on_ack(&mut self, ack: u32, acked: u32, flight: u32, limited: bool, smss: u32) -> Act {
        self.dup = 0;
        if self.recovering {
            if le(self.recover, ack) {
                // A full ACK: recovery ends, the window deflated to the threshold (RFC 6582 3.2 step 3, its first option, held to what is in flight plus one segment so no burst follows).
                self.recovering = false;
                let f = flight + smss;
                self.cwnd = if f < self.ssthresh { f } else { self.ssthresh };
                return Act::Go;
            }
            // A partial ACK: the next hole resent, the window deflated by what was acknowledged and one segment added back when that was a segment or more (RFC 6582 3.2 step 4).
            // WHY the test: half the flight is the threshold, so a partial ACK can cover more than the window holds; the subtraction would wrap to a huge window and a burst.
            self.cwnd = if self.cwnd > acked { self.cwnd - acked } else { 0 };
            if acked >= smss {
                self.cwnd += smss;
            }
            return Act::Resend;
        }
        if limited {
            if self.cwnd < self.ssthresh {
                // Slow start: one segment at most per ACK (RFC 5681 3.1, appropriate byte counting with L = 1, RFC 3465).
                self.cwnd += if acked < smss { acked } else { smss };
            } else {
                // Congestion avoidance: about a segment per round trip.
                let inc = (smss as u64 * smss as u64 / self.cwnd as u64) as u32;
                self.cwnd += if inc == 0 { 1 } else { inc };
            }
        }
        Act::Go
    }

    /// A duplicate ACK (RFC 5681 2: nothing new acknowledged, no data, the window unchanged, data outstanding): the third starts fast retransmit and recovery, unless it is for data sent before the last recovery ended (RFC 6582 4.1); in recovery each one inflates the window by a segment, as a segment has left the network.
    /// `una` the first unacknowledged byte, `max` the most ever sent, `flight` what is outstanding.
    /// WHY the inflation's bound: duplicate ACKs are the peer's to send; no more segments can have left the network than are in flight, so a flood of them inflates the window no further than that.
    pub fn on_dup(&mut self, una: u32, max: u32, flight: u32, smss: u32) -> Act {
        self.dup += 1;
        if self.recovering {
            if self.dup as u64 * smss as u64 <= flight as u64 {
                self.cwnd += smss;
            }
            return Act::Go;
        }
        if self.dup == 3 && lt(self.recover, una) {
            self.ssthresh = Self::half(flight, smss);
            self.recover = max;
            self.recovering = true;
            self.cwnd = self.ssthresh + 3 * smss;
            return Act::Resend;
        }
        Act::Go
    }

    /// The retransmission timer expired (RFC 5681 3.1 equation 4, then the loss window of one segment; RFC 6582 recovery point at the most sent).
    pub fn on_timeout(&mut self, flight: u32, max: u32, smss: u32) {
        self.ssthresh = Self::half(flight, smss);
        self.cwnd = smss;
        self.dup = 0;
        self.recovering = false;
        self.recover = max;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_wraps() {
        assert!(lt(u32::MAX, 0));
        assert!(lt(u32::MAX - 5, 3));
        assert!(!lt(3, u32::MAX - 5));
        assert!(le(7, 7));
        assert!(!lt(7, 7));
    }

    #[test]
    fn ooo_merges_and_orders() {
        let mut o = Ooo::EMPTY;
        assert!(o.insert(300, 400));
        assert!(o.insert(100, 200));
        assert!(o.insert(500, 600));
        assert_eq!(&o.r[..o.n], &[(100, 200), (300, 400), (500, 600)]);
        // Touching joins.
        assert!(o.insert(200, 300));
        assert_eq!(&o.r[..o.n], &[(100, 400), (500, 600)]);
        // Overlapping two joins both.
        assert!(o.insert(350, 550));
        assert_eq!(&o.r[..o.n], &[(100, 600)]);
        assert_eq!(o.bytes(), 500);
    }

    #[test]
    fn ooo_across_the_wrap() {
        let mut o = Ooo::EMPTY;
        let base = u32::MAX - 1000;
        assert!(o.insert(base.wrapping_add(1500), base.wrapping_add(2000)));
        assert!(o.insert(base.wrapping_add(500), base.wrapping_add(900)));
        assert_eq!(o.r[0], (base.wrapping_add(500), base.wrapping_add(900)));
        assert_eq!(o.advance(base.wrapping_add(500)), base.wrapping_add(900));
        assert_eq!(o.n, 1);
        assert_eq!(o.advance(base.wrapping_add(1600)), base.wrapping_add(2000));
        assert_eq!(o.n, 0);
    }

    #[test]
    fn ooo_full_refuses_only_what_joins_nothing() {
        let mut o = Ooo::EMPTY;
        for k in 0..OOO_N as u32 {
            assert!(o.insert(1000 + 100 * k, 1050 + 100 * k));
        }
        assert!(!o.insert(5000, 5100));
        assert_eq!(o.n, OOO_N);
        // Joining one still works when full.
        assert!(o.insert(1050, 1060));
        assert_eq!(o.r[0], (1000, 1060));
    }

    #[test]
    fn ooo_advance_fills_holes() {
        let mut o = Ooo::EMPTY;
        o.insert(200, 300);
        o.insert(400, 500);
        // The hole before 200 not filled yet.
        assert_eq!(o.advance(150), 150);
        assert_eq!(o.n, 2);
        // In-order data reaches 200: the first range joins.
        assert_eq!(o.advance(200), 300);
        assert_eq!(o.n, 1);
        // A range wholly behind the next byte is dropped without moving it back.
        assert_eq!(o.advance(600), 600);
        assert_eq!(o.n, 0);
    }

    #[test]
    fn sack_latest_first() {
        let mut o = Ooo::EMPTY;
        o.insert(100, 200);
        o.insert(300, 400);
        o.insert(500, 600);
        o.latest = 300;
        let mut b = [(0, 0); SACK_N];
        let n = o.sack_blocks(&mut b);
        assert_eq!(&b[..n], &[(300, 400), (100, 200), (500, 600)]);
        let mut opt = [0u8; 4 + 8 * SACK_N];
        let l = sack_option(&b[..n], &mut opt);
        assert_eq!(l, 28);
        assert_eq!(&opt[..4], &[1, 1, 5, 26]);
        let p = parse(&opt[..l]);
        assert_eq!(p.nsack, 3);
        assert_eq!(&p.sack[..3], &b[..3]);
    }

    #[test]
    fn options_read() {
        let p = parse(&syn_options(1440, 5));
        assert_eq!(p.mss, 1440);
        assert_eq!(p.ws, Some(5));
        assert!(p.sack_ok);
        // A shift past 14 is 14.
        assert_eq!(parse(&[3, 3, 20]).ws, Some(14));
        // Lengths that lie end the walk without reading past the bytes.
        assert_eq!(parse(&[2, 4, 5]).mss, 0);
        assert_eq!(parse(&[2, 40, 5, 180]).mss, 0);
        assert_eq!(parse(&[8, 0, 2, 4, 5, 180]).mss, 0);
        assert_eq!(parse(&[8]).mss, 0);
        // End of list stops; NOPs pass.
        assert_eq!(parse(&[0, 2, 4, 5, 180]).mss, 0);
        assert_eq!(parse(&[1, 1, 2, 4, 5, 180]).mss, 1460);
        // Unknown options are stepped over by their length.
        assert_eq!(parse(&[8, 10, 0, 0, 0, 0, 0, 0, 0, 0, 2, 4, 5, 180]).mss, 1460);
        // A SACK option whose length is no whole number of blocks is passed over.
        assert_eq!(parse(&[5, 7, 0, 0, 0, 0, 0]).nsack, 0);
    }

    #[test]
    fn window_moves_by_useful_steps() {
        let total = 1 << 20;
        // Plenty of room, nothing advertised: all of it.
        assert_eq!(window(total, 0, total, 1440), total);
        // A few bytes more room than advertised: the edge stays.
        assert_eq!(window(5000, 4000, total, 1440), 4000);
        // A segment's worth more: it moves.
        assert_eq!(window(5440, 4000, total, 1440), 5440);
        // A small buffer: half of it is the step.
        assert_eq!(window(600, 0, 1000, 1440), 600);
        assert_eq!(window(400, 0, 1000, 1440), 0);
        // Never more than the room.
        assert_eq!(window(100, 4000, total, 1440), 100);
        assert!(window_update_due(total / 2, 1000, total));
        assert!(!window_update_due(total / 2 - 1, 1000, total));
        assert!(!window_update_due(total, total / 2, total));
    }

    #[test]
    fn scaling() {
        assert_eq!(scaled(1 << 20, 0, 5), (1 << 15, 1 << 20));
        assert_eq!(scaled((1 << 20) - 1, 0, 5), ((1 << 15) - 1, (1 << 20) - 32));
        assert_eq!(scaled(65535, 0, 0), (65535, 65535));
        assert_eq!(scaled(70000, 0, 0), (65535, 65535));
        // An edge kept where it was is rounded up, never back.
        assert_eq!(scaled(1000, 1000, 5), (32, 1024));
        assert_eq!(scaled(992, 992, 5), (31, 992));
        // A window that moves on rounds down as before.
        assert_eq!(scaled(5000, 1000, 5), (156, 4992));
    }

    #[test]
    fn mss_from_mtu() {
        assert_eq!(mss_for(1500, 4), 1460);
        assert_eq!(mss_for(1500, 6), 1440);
        assert_eq!(mss_for(1428, 6), 1368);
        assert_eq!(mss_for(0, 4), MSS_FALLBACK);
        assert_eq!(mss_for(1200, 6), MSS_FALLBACK);
        assert_eq!(mss_for(9000, 4), MSS_FALLBACK);
        assert_eq!(mss_default(4), 536);
        assert_eq!(mss_default(6), 1220);
    }

    #[test]
    fn rtt_estimator() {
        let (g, min, max) = (1, 250, 60_000);
        let mut r = Rtt::new(1000);
        assert_eq!(r.cur, 1000);
        r.sample(100, g, min, max);
        // First sample: srtt 100, var 50, rto 100 + 200 = 300.
        assert_eq!((r.srtt, r.var, r.rto), (100, 50, 300));
        r.sample(100, g, min, max);
        // var 3/4·50 + 0 = 38 (integer), srtt 100: rto 100 + 152 = 252.
        assert_eq!((r.srtt, r.var, r.rto), (100, 38, 252));
        for _ in 0..64 {
            r.sample(100, g, min, max);
        }
        // A steady path: the floor.
        assert_eq!(r.rto, min);
        r.backoff(max);
        assert_eq!(r.cur, 2 * min);
        for _ in 0..32 {
            r.backoff(max);
        }
        assert_eq!(r.cur, max);
        r.reset();
        assert_eq!(r.cur, min);
        // A slow path rises past the floor.
        r.sample(2000, g, min, max);
        assert!(r.rto > 2000 / 8 + 100);
    }

    #[test]
    fn congestion_slow_start_then_avoidance() {
        let s = 1460;
        let mut c = Cc::new(s, 0);
        assert_eq!(c.cwnd, 4380);
        assert_eq!(Cc::new(536, 0).cwnd, 2144);
        c.ssthresh = 8000;
        // Slow start: a segment per ACK while limited.
        assert_eq!(c.on_ack(0, s, 0, true, s), Act::Go);
        assert_eq!(c.cwnd, 4380 + s);
        // Not limited: no growth.
        c.on_ack(0, s, 0, false, s);
        assert_eq!(c.cwnd, 4380 + s);
        c.on_ack(0, s, 0, true, s);
        c.on_ack(0, s, 0, true, s);
        assert!(c.cwnd >= c.ssthresh);
        let w = c.cwnd;
        c.on_ack(0, s, 0, true, s);
        // Avoidance: about s·s/cwnd.
        assert_eq!(c.cwnd, w + s * s / w);
    }

    #[test]
    fn congestion_fast_retransmit_and_newreno() {
        let s = 1000;
        let mut c = Cc::new(s, 0);
        c.cwnd = 20_000;
        let (una, max, flight) = (10_000u32, 30_000u32, 20_000u32);
        assert_eq!(c.on_dup(una, max, flight, s), Act::Go);
        assert_eq!(c.on_dup(una, max, flight, s), Act::Go);
        assert_eq!(c.on_dup(una, max, flight, s), Act::Resend);
        assert!(c.recovering);
        assert_eq!(c.ssthresh, 10_000);
        assert_eq!(c.cwnd, 13_000);
        // Inflation per further duplicate.
        c.on_dup(una, max, flight, s);
        assert_eq!(c.cwnd, 14_000);
        // A partial ACK: resend the next hole, deflate.
        assert_eq!(c.on_ack(15_000, 5_000, 15_000, true, s), Act::Resend);
        assert_eq!(c.cwnd, 14_000 - 5_000 + s);
        assert!(c.recovering);
        // A full ACK ends recovery at the threshold or less.
        assert_eq!(c.on_ack(30_000, 15_000, 0, true, s), Act::Go);
        assert!(!c.recovering);
        assert_eq!(c.cwnd, s);
        // Duplicates for data sent before the recovery point do not start another.
        c.dup = 0;
        assert_eq!(c.on_dup(25_000, 40_000, 15_000, s), Act::Go);
        assert_eq!(c.on_dup(25_000, 40_000, 15_000, s), Act::Go);
        assert_eq!(c.on_dup(25_000, 40_000, 15_000, s), Act::Go);
        assert!(!c.recovering);
    }

    #[test]
    fn congestion_inflation_bounded_by_flight() {
        let s = 1000;
        let mut c = Cc::new(s, 0);
        c.cwnd = 10_000;
        for _ in 0..3 {
            c.on_dup(1, 10_001, 5_000, s);
        }
        let w = c.cwnd;
        // A flood of forged duplicates: at most the segments in flight inflate it.
        for _ in 0..1 << 16 {
            c.on_dup(1, 10_001, 5_000, s);
        }
        assert_eq!(c.cwnd, w + 2 * s);
    }

    #[test]
    fn congestion_timeout() {
        let s = 1000;
        let mut c = Cc::new(s, 0);
        c.cwnd = 50_000;
        c.on_timeout(40_000, 90_000, s);
        assert_eq!((c.cwnd, c.ssthresh, c.recover), (s, 20_000, 90_000));
    }
}
