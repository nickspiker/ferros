//! Mailboxes (docs/KERNEL.md "Mailboxes"; owner, 2026-11-19: "the driver for GPS should have a capability based public read only mailbox so you could have an unlimited number of items accessing it. same with accelerometer, mic, etc.").
//!
//! A mailbox is one frame of the kernel's pool (2 MiB) mapped twice: read-write at the writer's window, read-only at the readers' window.
//! One process writes it, the driver that holds its `<name>-mailbox` region.
//! Any number of processes read it, each holding a capability for its `<name>-view` region, granted at spawn or opened with a proof (`ferros_sys::mailbox`).
//! The writer never learns how many read: it publishes under a seqlock and wakes the sequence word's waiters with one call, and the kernel wakes every thread of every process waiting on it.
//! A reader that catches a publish half done sees the sequence move and reads again.
//!
//! The header, eight words:
//! - word 0, `SEQ`: the sequence, even while the words are whole and odd while a publish is under way, bit 63 (`DEAD`) set by the kernel when the writer's process has ended.
//!   Readers wait on this word (SYS_WAIT), the writer's wake (SYS_WAKE) ends every wait on it.
//! - word 1, `KIND`: the payload's layout, the writer's tag (`gps::KIND`, `motion::KIND`), so a reader never takes one layout for another.
//! - word 2, `LEN`: how many payload words the writer published.
//! - word 3, `AT`: the counter (CNTVCT) at the publish, so a reader measures how late it woke.
//! - words 4 to 7: not yet used, zero.
//! - the payload from word 8.
//!
//! Every word is touched only as an atomic, so a reader racing a publish reads stale or new words, never undefined behaviour, and the sequence says which (Boehm, "Can seqlocks get along with programming language memory models?", 2012: the writer's release fence after the odd store, the reader's acquire fence before its second load).
//! Readers only load: the readers' window is read-only, and a store or an atomic read-modify-write there faults.
//!
//! No VSF here: a mailbox is memory two processes share for the boot, never stored or sent, as the sensors' block is; a value that is kept goes thru the vault as a VSF document (ferros_kept).

#![no_std]

use core::sync::atomic::{fence, AtomicU64, Ordering};

/// A mailbox's bytes: one frame of the pool.
pub const BYTES: usize = 1 << 21;
/// A mailbox's words.
pub const WORDS: usize = BYTES >> 3;
/// The header's words.
pub const HEADER: usize = 1 << 3;
/// The most payload words a mailbox holds.
pub const PAYLOAD_MAX: usize = WORDS - HEADER;

/// The sequence word.
pub const SEQ: usize = 0;
/// The payload's layout.
pub const KIND: usize = 1;
/// The payload words published.
pub const LEN: usize = 2;
/// The counter at the publish.
pub const AT: usize = 3;
/// The sequence's bit set by the kernel when the writer's process has ended: no writer, and the payload zeroed.
pub const DEAD: u64 = 1 << 63;

/// The mailboxes, by number: the kernel's regions are `<name>-mailbox` (the writer's) and `<name>-view` (a reader's) for each, in this order.
pub const NAMES: [&str; 4] = ["gps", "motion", "mic", "test"];
/// The GPS driver's: its fix and its tracks, the predicted birds among them (`gps`).
pub const GPS: usize = 0;
/// The sensors process's: gravity, the magnetometers and the accelerometer (`motion`).
pub const MOTION: usize = 1;
/// The microphones', once the audio service captures (docs/KERNEL.md "Mailboxes": planned, no writer yet).
pub const MIC: usize = 2;
/// A test writer's (payloads/mbtest): the readers' measure without a radio.
pub const TEST: usize = 3;
/// The writers' regions, by number.
pub const BOX_NAMES: [&str; 4] = ["gps-mailbox", "motion-mailbox", "mic-mailbox", "test-mailbox"];
/// The readers' regions, by number.
pub const VIEW_NAMES: [&str; 4] = ["gps-view", "motion-view", "mic-view", "test-view"];

/// One publish under way: the sequence made odd at `begin`, the words written with `put`, the sequence even and two on at `finish`.
/// For a payload built in place (a ring's new samples); `publish` is the whole of it for a payload in hand.
pub struct Publish<'a> {
    words: &'a [AtomicU64],
    seq: u64,
}

impl<'a> Publish<'a> {
    /// Begin a publish into the writer's `words` (the whole mailbox, header first).
    /// The sequence the kernel left (`DEAD`, the last writer's end) is taken without its dead bit, and an odd one (a writer that never finished) rounded up, so a new writer counts on from it and never reuses a value a reader may be waiting on.
    pub fn begin(words: &'a [AtomicU64]) -> Publish<'a> {
        let s = words[SEQ].load(Ordering::Relaxed) & !DEAD;
        let seq = (s + 1) & !1;
        words[SEQ].store(seq + 1, Ordering::Relaxed);
        // The odd sequence is seen before any payload word changes (Boehm's writer).
        fence(Ordering::Release);
        Publish { words, seq }
    }
    /// Payload word `i`.
    pub fn put(&self, i: usize, v: u64) {
        self.words[HEADER + i].store(v, Ordering::Relaxed);
    }
    /// The header's layout, length and time, then the sequence even: the publish is whole. The new sequence, which the caller wakes the readers on.
    pub fn finish(self, kind: u64, len: usize, at: u64) -> u64 {
        self.words[KIND].store(kind, Ordering::Relaxed);
        self.words[LEN].store(len as u64, Ordering::Relaxed);
        self.words[AT].store(at, Ordering::Relaxed);
        self.words[SEQ].store(self.seq + 2, Ordering::Release);
        self.seq + 2
    }
}

/// Publish `payload` whole into the writer's `words`, tagged `kind`, stamped `at`: the new sequence.
pub fn publish(words: &[AtomicU64], kind: u64, at: u64, payload: &[u64]) -> u64 {
    let p = Publish::begin(words);
    for (i, &v) in payload.iter().enumerate() {
        p.put(i, v);
    }
    p.finish(kind, payload.len(), at)
}

/// What one look at a mailbox found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Got {
    /// A whole publish: its sequence, layout, the payload words it had (`len`, of which the reader's buffer took what fits) and its time.
    Fresh { seq: u64, kind: u64, len: usize, at: u64 },
    /// No writer: its process ended (`DEAD`) and the kernel zeroed the payload. A new writer's first publish moves the sequence on.
    Dead { seq: u64 },
    /// Nothing published yet since the mailbox was made.
    Never,
    /// A publish under way (the sequence odd: wait on it, the writer's wake ends the wait) or one that landed while the words were read (the sequence even and moved: read again at once).
    Busy { seq: u64 },
}

/// Read a mailbox's payload into `out` (as many words as fit) from the readers' `words` (the whole mailbox, header first), once.
/// `Fresh` only when the sequence was even and the same before and after the copy: the words are one publish's.
pub fn try_read(words: &[AtomicU64], out: &mut [u64]) -> Got {
    let s0 = words[SEQ].load(Ordering::Acquire);
    if s0 & 1 != 0 {
        return Got::Busy { seq: s0 };
    }
    if s0 & DEAD != 0 {
        return Got::Dead { seq: s0 };
    }
    if s0 == 0 {
        return Got::Never;
    }
    let kind = words[KIND].load(Ordering::Relaxed);
    let len = words[LEN].load(Ordering::Relaxed) as usize;
    let at = words[AT].load(Ordering::Relaxed);
    // WHY the min: `len` is the writer's word, read under no lock; the copy stays inside the reader's buffer and the mailbox whatever it says (a torn `len` is caught by the sequence below).
    let n = len.min(out.len()).min(PAYLOAD_MAX);
    for (i, o) in out[..n].iter_mut().enumerate() {
        *o = words[HEADER + i].load(Ordering::Relaxed);
    }
    // The copy's loads are done before the sequence is read again (Boehm's reader).
    fence(Ordering::Acquire);
    let s1 = words[SEQ].load(Ordering::Relaxed);
    if s1 != s0 {
        return Got::Busy { seq: s1 };
    }
    Got::Fresh { seq: s0, kind, len, at }
}

/// The kernel's end of a writer: the sequence odd and dead, the first `len` payload words zeroed, then even and dead, so no reader ever reads a dead writer's words as a publish and none keeps reading a place or a voice after its driver has gone.
/// Called with every thread of the writer's process gone (its arc is freed next), so nothing of it writes after this.
/// The new sequence.
pub fn kill(words: &[AtomicU64]) -> u64 {
    let s = words[SEQ].load(Ordering::Relaxed) & !DEAD;
    let seq = (s + 1) & !1;
    words[SEQ].store((seq + 1) | DEAD, Ordering::Relaxed);
    fence(Ordering::Release);
    // WHY the min: `len` is the dead writer's word; the zeroing stays inside the mailbox whatever it says.
    let n = (words[LEN].load(Ordering::Relaxed) as usize).min(PAYLOAD_MAX);
    for w in &words[HEADER..HEADER + n] {
        w.store(0, Ordering::Relaxed);
    }
    words[LEN].store(0, Ordering::Relaxed);
    words[SEQ].store((seq + 2) | DEAD, Ordering::Release);
    (seq + 2) | DEAD
}

/// The GPS driver's mailbox (payloads/gps; docs/GPS.md "The mailbox"): the latest fix, what the chip has sent, and the tracks (every signal of the latest fix message, then each predicted bird no heard signal stands for), published whenever any of them moves.
/// The position is the person's: a reader holds `gps-view` only thru the GPS right (the kernel's proof), and the kernel zeroes it when the driver ends.
pub mod gps {
    /// The layout's tag: "GPSMBOX" and its version, 0.
    pub const KIND: u64 = 0x4750_534D_424F_5800;
    /// The most tracks: the kernel's TRACKS for a link (ferros_sys `TRACKS_MAX`).
    pub const TRACKS: usize = 1 << 6;
    /// Word 0: bit 0 a fix, bit 1 the fix carried a track (heading), bits 8 to 15 the kinds of frame the chip has sent, bits 32 to 63 the fixes so far.
    pub const STATE: usize = 0;
    /// Word 1: the counter at the latest fix.
    pub const FIX_AT: usize = 1;
    /// Word 2: latitude | longitude << 32, each i32 in 2^-23 degrees.
    pub const LAT_LON: usize = 2;
    /// Word 3: altitude (i32, m) | the error estimate << 32 (u32, m).
    pub const ALT_ERR: usize = 3;
    /// Word 4: ground speed | track << 32, each i32 in 1/64 m/s and 1/64 degree.
    pub const SPEED_TRACK: usize = 4;
    /// Word 5: every frame from the chip so far | the fix messages decoded so far << 32.
    pub const FRAMES_MSGS: usize = 5;
    /// Word 6: the counter at the latest fix message (the tracks' age).
    pub const SKY_AT: usize = 6;
    /// Word 7: the tracks in use (bits 0 to 7) | the predicted sky's generation << 32.
    pub const TRACKS_N: usize = 7;
    /// Words 8 on: the tracks, a word each (ferros_sys `sat_word`).
    pub const TRACK0: usize = 8;
    /// The payload's words.
    pub const WORDS: usize = TRACK0 + TRACKS;

    /// One publish's contents.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Gps {
        pub valid: bool,
        pub has_track: bool,
        pub kinds: u8,
        pub fixes: u32,
        pub fix_at: u64,
        pub lat: i32,
        pub lon: i32,
        pub alt: i32,
        pub err: u32,
        pub speed: i32,
        pub track: i32,
        pub frames: u32,
        pub msgs: u32,
        pub sky_at: u64,
        pub generation: u32,
        pub n: usize,
        pub tracks: [u64; TRACKS],
    }

    impl Default for Gps {
        fn default() -> Gps {
            Gps { valid: false, has_track: false, kinds: 0, fixes: 0, fix_at: 0, lat: 0, lon: 0, alt: 0, err: 0, speed: 0, track: 0, frames: 0, msgs: 0, sky_at: 0, generation: 0, n: 0, tracks: [0; TRACKS] }
        }
    }

    impl Gps {
        /// The payload's words.
        pub fn encode(&self, w: &mut [u64; WORDS]) {
            let pair = |a: i32, b: i32| a as u32 as u64 | (b as u32 as u64) << 32;
            w[STATE] = self.valid as u64 | (self.has_track as u64) << 1 | (self.kinds as u64) << 8 | (self.fixes as u64) << 32;
            w[FIX_AT] = self.fix_at;
            w[LAT_LON] = pair(self.lat, self.lon);
            w[ALT_ERR] = pair(self.alt, self.err as i32);
            w[SPEED_TRACK] = pair(self.speed, self.track);
            w[FRAMES_MSGS] = self.frames as u64 | (self.msgs as u64) << 32;
            w[SKY_AT] = self.sky_at;
            // WHY the min: `n` is the writer's count; the words hold `TRACKS`.
            let n = self.n.min(TRACKS);
            w[TRACKS_N] = n as u64 | (self.generation as u64) << 32;
            w[TRACK0..TRACK0 + TRACKS].copy_from_slice(&self.tracks);
            w[TRACK0 + n..].fill(0);
        }
        /// A payload's contents; None for one shorter than the layout (a reader's buffer cut short, or another layout).
        pub fn decode(w: &[u64]) -> Option<Gps> {
            if w.len() < WORDS {
                return None;
            }
            let lo = |v: u64| v as u32 as i32;
            let hi = |v: u64| (v >> 32) as u32 as i32;
            let mut tracks = [0u64; TRACKS];
            // WHY the min: the count is the writer's word.
            let n = (w[TRACKS_N] as u8 as usize).min(TRACKS);
            tracks[..n].copy_from_slice(&w[TRACK0..TRACK0 + n]);
            Some(Gps {
                valid: w[STATE] & 1 != 0,
                has_track: w[STATE] >> 1 & 1 != 0,
                kinds: (w[STATE] >> 8) as u8,
                fixes: (w[STATE] >> 32) as u32,
                fix_at: w[FIX_AT],
                lat: lo(w[LAT_LON]),
                lon: hi(w[LAT_LON]),
                alt: lo(w[ALT_ERR]),
                err: hi(w[ALT_ERR]) as u32,
                speed: lo(w[SPEED_TRACK]),
                track: hi(w[SPEED_TRACK]),
                frames: w[FRAMES_MSGS] as u32,
                msgs: (w[FRAMES_MSGS] >> 32) as u32,
                sky_at: w[SKY_AT],
                generation: (w[TRACKS_N] >> 32) as u32,
                n,
                tracks,
            })
        }
    }
}

/// The sensors process's motion mailbox (payloads/sensors; docs/SENSORS.md "The motion mailbox"): gravity and its latest two samples, the magnetometers' field and the accelerometer, published with every sample the AoC sends of any of them.
/// The first ten words are the sensors' block's (ferros_sys `sensor_block`), so a reader decodes them the same way.
pub mod motion {
    /// The layout's tag: "MOTION" and its version, 0.
    pub const KIND: u64 = 0x4D4F_5449_4F4E_0000;
    /// Word 0: gravity x | y << 32 (f32 bits, m/s², the panel's frame); word 1: z | the samples so far << 32.
    pub const GRAVITY_XY: usize = 0;
    pub const GRAVITY_Z: usize = 1;
    /// Words 2 to 7: the last two gravity samples, [x | y << 32, z, time in counter ticks], newest first.
    pub const PAIR: usize = 2;
    /// Word 8: the magnetic field x | y << 32 (f32 bits, µT); word 9: z | the samples so far << 32.
    pub const MAG_XY: usize = 8;
    pub const MAG_Z: usize = 9;
    /// Word 10: the accelerometer x | y << 32 (f32 bits, m/s², the panel's frame, gravity included); word 11: z | the samples so far << 32; word 12: the newest sample's time in counter ticks.
    pub const ACCEL_XY: usize = 10;
    pub const ACCEL_Z: usize = 11;
    pub const ACCEL_AT: usize = 12;
    /// The payload's words.
    pub const WORDS: usize = 13;
}

#[cfg(test)]
mod tests;
