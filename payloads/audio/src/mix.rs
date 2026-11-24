//! The mixer: every stream's ring, a period at a time, into the one the AoC plays (docs/AUDIO.md "Mixing").
//!
//! A frame is one word, the left sample in its low half and the right in its high half (S16_LE interleaved, as kists write and the AoC reads), so a frame moves as one load and one store.

use ferros_sys::audio::{gain_q15, ALERT, LEFT, PERIOD, RECORD_FRAMES, RIGHT, RING, WRITE_MAX};
use ferros_sys::{dnd, pref, PREF_SOUND, SOUND};

/// Streams at once: a power of two, more than a device plays together (a game, a call, a notification).
pub const STREAMS: usize = 1 << 3;
/// A stream whose ring has been empty this many frames with no write waiting (2^16, 1.4 s) is closed: its kist paused, or ended without closing (an ending process says nothing to its servers). Counted in frames played, the hardware's clock, never a timer, and in frames rather than periods since the period became the service's choice (2026-11-24). A write to it later answers "unknown handle", and the kist opens again.
pub const STALE: u32 = 1 << 16;

pub struct Stream {
    pub live: bool,
    pub handle: [u8; 8],
    pub class: u8,
    /// The stream's own gain, 32768 unity, at most 65535 (+6 dB).
    pub gain: u16,
    ring: [u32; RING],
    head: usize,
    pub fill: usize,
    /// A write that did not fit yet: its reply token and its frames, answered when a period played makes room (the doorbell).
    pub held: bool,
    pub token: [u8; 32],
    pending: [u32; WRITE_MAX],
    pending_n: usize,
    /// Frames in a row its ring has been empty.
    dry: u32,
    /// Closed by its kist: freed once its ring has played out.
    pub closing: bool,
}

impl Stream {
    pub const EMPTY: Stream = Stream { live: false, handle: [0; 8], class: 0, gain: 1 << 15, ring: [0; RING], head: 0, fill: 0, held: false, token: [0; 32], pending: [0; WRITE_MAX], pending_n: 0, dry: 0, closing: false };

    pub fn open(&mut self, handle: [u8; 8], class: u8) {
        *self = Stream::EMPTY;
        self.live = true;
        self.handle = handle;
        self.class = class;
    }

    /// The frames the ring can take now.
    pub fn room(&self) -> usize {
        RING - self.fill
    }

    /// `frames` (little-endian bytes, four a frame) into the ring if they all fit: true when taken.
    pub fn put(&mut self, frames: &[u8]) -> bool {
        let n = frames.len() >> 2;
        if n > self.room() {
            return false;
        }
        let mut at = (self.head + self.fill) & (RING - 1);
        for f in frames.chunks_exact(4) {
            self.ring[at] = u32::from_le_bytes([f[0], f[1], f[2], f[3]]);
            at = (at + 1) & (RING - 1);
        }
        self.fill += n;
        self.dry = 0;
        true
    }

    /// A write held for room: its frames kept here until `release`.
    pub fn hold(&mut self, token: [u8; 32], frames: &[u8]) {
        let n = frames.len() >> 2;
        for (k, f) in frames.chunks_exact(4).enumerate() {
            self.pending[k] = u32::from_le_bytes([f[0], f[1], f[2], f[3]]);
        }
        self.pending_n = n;
        self.token = token;
        self.held = true;
        self.dry = 0;
    }

    /// The held write's frames into the ring once they fit: its token to answer, and the room left after them.
    pub fn release(&mut self) -> Option<([u8; 32], usize)> {
        if !self.held || self.pending_n > self.room() {
            return None;
        }
        let mut at = (self.head + self.fill) & (RING - 1);
        for k in 0..self.pending_n {
            self.ring[at] = self.pending[k];
            at = (at + 1) & (RING - 1);
        }
        self.fill += self.pending_n;
        self.held = false;
        Some((self.token, self.room()))
    }
}

/// A 1 kHz sine at 48 kHz, one cycle: round(32767 · sin(2πk/48)). The test tone's (`TONE`): a sine puts its energy at one frequency, where a square's harmonics would reach the tweeter-less speaker's top end.
pub const SINE: [i16; 48] = [
    0, 4277, 8481, 12539, 16383, 19947, 23170, 25996, 28377, 30273, 31650, 32487, 32767, 32487, 31650, 30273, 28377, 25996, 23170, 19947, 16383, 12539, 8481, 4277, 0, -4277, -8481, -12539, -16383, -19947, -23170, -25996, -28377, -30273, -31650, -32487, -32767, -32487, -31650, -30273, -28377, -25996, -23170, -19947, -16383, -12539, -8481, -4277,
];

/// The service's own tone: frames left to play, its gain, where in the cycle it is, its channels, and frames played since it started (its fade in runs over the first period). Counted in frames since 2026-11-24, when the period became a choice and a turn's mix a part of one.
pub struct Tone {
    pub left: u32,
    pub gain: u32,
    phase: usize,
    /// `audio::LEFT` | `audio::RIGHT`.
    pub mask: u8,
    played: u32,
}

/// The longest fade, in frames: the default period, 256. A shorter period fades over itself.
const FADE: u32 = 1 << 8;

impl Tone {
    pub const NONE: Tone = Tone { left: 0, gain: 0, phase: 0, mask: 0, played: FADE };
    /// Started for `frames`, or changed while it plays (its level or channels; a playing tone is not faded in again).
    pub fn start(&mut self, level: u8, frames: u32, mask: u8) {
        if self.left == 0 {
            self.played = 0;
        }
        self.left = frames;
        self.gain = gain_q15(level);
        self.mask = mask;
    }
    /// Stopped: faded out over its next `FADE` frames.
    pub fn stop(&mut self) {
        if self.left > FADE {
            self.left = FADE;
        }
    }
}
/// The microphone test's recording (mic.rs): one microphone's samples, kept in this process's memory only (never written anywhere: AGENT.md rule -1), played back once and gone with the process. `audio::RECORD_FRAMES` of them, 2.7 s.
pub static mut REC: [i16; RECORD_FRAMES] = [0; RECORD_FRAMES];

/// The recording played back: frames left, where it is, and its gain; it fades in over its first frames and out over its last (`period`).
pub struct Clip {
    pub left: usize,
    at: usize,
    /// The gain in Q15 that takes the recording's own peak to the level asked for: `start`.
    k: i64,
}

impl Clip {
    pub const NONE: Clip = Clip { left: 0, at: 0, k: 0 };
    /// `n` frames of `REC` played from the start, scaled so their peak (`peak`, at least 1) comes out at the level's amplitude (`gain_q15(level)`, in sample units): the loudest sample played is the level asked for and no louder, whatever the microphone picked up. With the level at most `TONE_MAX` that is 2048, 24 dB under full scale, the speaker test's own ceiling (ferros_sys audio `TONE_MAX`).
    pub fn start(&mut self, n: usize, peak: u16, level: u8) {
        self.left = n;
        self.at = 0;
        self.k = ((gain_q15(level) as i64) << 15) / peak as i64;
    }
    /// Stopped: faded out over at most `per` more frames, one period (the smaller of two counts: what is left, or a period).
    pub fn stop(&mut self, per: usize) {
        if self.left > per {
            self.left = per;
        }
    }
}

/// The bench's click train (main.rs `Bench`): every `spacing` frames a burst of the 1 kHz sine, eight cycles (384 frames, 8 ms), rising over its first cycle and falling over its last, on both channels; `count` of them after one `spacing` of silence. Quiet (its level at most `TONE_MAX`, the unprotected speaker's ceiling), and long enough for the loopback's 1 kHz detector to find under the room's noise (2026-11-24: a 2 ms burst at -24 dBFS reached the microphones at 6 to 29 counts against a noise of 10 to 17).
pub struct Clicks {
    pub left: u32,
    pub gain: u32,
    pub spacing: u32,
    /// Frames until the next click starts.
    until: u32,
    /// Frames into the click under way (`BURST` when none).
    at: u32,
}

/// A click's length in frames: eight cycles of `SINE`; its attack and release one cycle each.
const BURST: u32 = 384;
const EDGE: u32 = 48;

impl Clicks {
    pub const NONE: Clicks = Clicks { left: 0, gain: 0, spacing: 0, until: 0, at: BURST };
    pub fn start(&mut self, level: u8, count: u32, spacing: u32) {
        *self = Clicks { left: count, gain: gain_q15(level), spacing, until: spacing, at: BURST };
    }
}

/// The bench's feed (main.rs `Bench`, flag bit 3): a mono signal of the caller's, the hand spawn's input (a sweep the host made), played once on both channels at a level, its first frame marked like a click's.
pub struct Feed {
    /// Where the samples lie (the spawn input's address) and how many.
    pub base: u64,
    pub len: usize,
    pub at: usize,
    pub gain: u32,
    pub on: bool,
    /// `audio::LEFT` | `audio::RIGHT`: the speakers it plays on (one at a time tells which microphone is nearest which).
    pub mask: u8,
}

impl Feed {
    pub const NONE: Feed = Feed { base: 0, len: 0, at: 0, gain: 0, on: false, mask: 0 };
}

/// What a period's mixing found.
pub struct Mixed {
    /// A stream that had been playing ran short (its kist late).
    pub underruns: u32,
    /// Anything sounded: a live stream with frames, or the tone.
    pub sound: bool,
    /// The frame in this period where a click began, if one did.
    pub click: Option<usize>,
}

/// WHY the clip (AGENT.md rule 0: stated, proved, what it prevents): the sum of the streams is the only place a sample can leave i16's range, and it does whenever two loud streams (or one with its gain over unity) line up: k streams at full scale sum to k times it. Cast to i16 as it stands, 40000 becomes -25536: the waveform jumps a full scale the wrong way, a crack at the loudest the speaker gives, which without the amplifiers' protection firmware (amp.rs) is the one thing this service must never send. Held to the rails instead, it flattens: audible as distortion, never as a jump. A smooth knee (a sigmoid near the rails) would distort less, the owner's call (docs/AUDIO.md "Open").
fn rail(v: i32) -> u32 {
    let s = if v > i16::MAX as i32 {
        i16::MAX
    } else if v < i16::MIN as i32 {
        i16::MIN
    } else {
        v as i16
    };
    s as u16 as u32
}

/// One period mixed into `out`, whose length is the period (a power of two, at most `PERIOD`: main.rs checks it where it is chosen): every live stream at its gain times the device's volume (alerts silent unless the sound mode is `SOUND` and do not disturb is off: preferences 39 and 40), the tone, the clip and the clicks; the streams' rings advanced, the stale and the played-out freed.
pub fn period(streams: &mut [Stream; STREAMS], tone: &mut Tone, clip: &mut Clip, clicks: &mut Clicks, feed: &mut Feed, out: &mut [u32], period: usize) -> Mixed {
    // `out` is this turn's frames, at most a period: the service tops the AoC's ring up to its lead in frames (main.rs `play_turn`).
    let per = out.len();
    let mut acc = [[0i32; 2]; PERIOD];
    let acc = &mut acc[..per];
    let volume = gain_q15(ferros_sys::audio::level());
    let alerts = pref(PREF_SOUND) == SOUND && !dnd::on();
    let mut m = Mixed { underruns: 0, sound: false, click: None };
    for s in streams.iter_mut().filter(|s| s.live) {
        // The two gains are each at most 2^16 and 2^15, so their product fits in u32 before the shift; the result at most 65535, and a sample times it fits in i32 (32768 · 65535 < 2^31).
        let g = if s.class == ALERT && !alerts { 0 } else { (s.gain as u32 * volume) >> 15 } as i32;
        // A stream short of a period plays what it has: the count of frames to take, the smaller of two counts, not a value held to a range.
        let take = if s.fill < per { s.fill } else { per };
        for (k, a) in acc[..take].iter_mut().enumerate() {
            let f = s.ring[(s.head + k) & (RING - 1)];
            a[0] += (f as u16 as i16 as i32 * g) >> 15;
            a[1] += ((f >> 16) as u16 as i16 as i32 * g) >> 15;
        }
        s.head = (s.head + take) & (RING - 1);
        s.fill -= take;
        m.sound |= take != 0;
        if take < per {
            // Short after playing: an underrun. Empty and quiet: a dry period, its frames counted to `STALE`.
            if take != 0 && !s.closing {
                m.underruns += 1;
            }
            if take == 0 && !s.held {
                s.dry += per as u32;
            }
        }
        // Never with a write held: its writer waits for that reply (`release` answers it first, and the frames then play out).
        if !s.held && ((s.closing && s.fill == 0) || s.dry >= STALE) {
            s.live = false;
        }
    }
    if tone.left != 0 {
        let g = tone.gain as i32;
        let (l, r) = ((tone.mask & LEFT != 0) as i32, (tone.mask & RIGHT != 0) as i32);
        // The fade's length: the period, at most `FADE`.
        let fade = if (period as u32) < FADE { period as u32 } else { FADE };
        let fsh = fade.trailing_zeros();
        for a in acc.iter_mut() {
            if tone.left == 0 {
                break;
            }
            // The envelope in 256ths: rising over the first fade, falling over the last, so the tone starts and stops without the step a cut sine makes (a click).
            let env = (if tone.played < fade { tone.played << 8 >> fsh } else if tone.left <= fade { (tone.left - 1) << 8 >> fsh } else { 1 << 8 }) as i32;
            let v = ((SINE[tone.phase] as i32 * g) >> 15) * env >> 8;
            a[0] += v * l;
            a[1] += v * r;
            tone.phase += 1;
            if tone.phase == SINE.len() {
                tone.phase = 0;
            }
            tone.played += 1;
            tone.left -= 1;
        }
        m.sound = true;
    }
    if clip.left != 0 {
        // SAFETY: REC is written only by the clock thread while recording (mic.rs) and read only here, on the same thread, after the recording has ended.
        let rec = unsafe { &*(&raw const REC) };
        // A clip shorter than a period plays what it has: the count of frames to take, the smaller of two counts.
        let take = if clip.left < per { clip.left } else { per };
        let fade = if period < FADE as usize { period } else { FADE as usize };
        let fsh = fade.trailing_zeros();
        for (j, a) in acc[..take].iter_mut().enumerate() {
            // The same envelope as the tone's: rising over the first fade, falling over the last, counted in the clip's own frames so a short turn does not cut it.
            let done = clip.at + j;
            let left = clip.left - j;
            let env = if done < fade { ((done << 8) >> fsh) as i64 } else if left <= fade { (((left - 1) << 8) >> fsh) as i64 } else { 1 << 8 };
            let v = ((rec[done] as i64 * clip.k) >> 15) * env >> 8;
            a[0] += v as i32;
            a[1] += v as i32;
        }
        clip.at += take;
        clip.left -= take;
        m.sound = true;
    }
    if clicks.left != 0 || clicks.at < BURST {
        let g = clicks.gain as i32;
        for (k, a) in acc.iter_mut().enumerate() {
            if clicks.at == BURST && clicks.left != 0 {
                if clicks.until == 0 {
                    clicks.at = 0;
                    clicks.left -= 1;
                    clicks.until = clicks.spacing;
                    m.click = Some(k);
                } else {
                    clicks.until -= 1;
                }
            }
            if clicks.at < BURST {
                // Rising over the first cycle and falling over the last, so the burst starts and ends at zero.
                let env = if clicks.at < EDGE { clicks.at } else if clicks.at >= BURST - EDGE { BURST - clicks.at } else { EDGE } as i32;
                let v = ((SINE[(clicks.at % SINE.len() as u32) as usize] as i32 * g) >> 15) * env / EDGE as i32;
                a[0] += v;
                a[1] += v;
                clicks.at += 1;
            }
        }
        m.sound = true;
    }
    if feed.on {
        if feed.at == 0 {
            m.click = Some(0);
        }
        let g = feed.gain as i32;
        let (l, r) = ((feed.mask & LEFT != 0) as i32, (feed.mask & RIGHT != 0) as i32);
        for a in acc.iter_mut() {
            if feed.at == feed.len {
                feed.on = false;
                break;
            }
            // SAFETY: `base` is this process's spawn input, `len` samples of it checked against its length where the feed was set (main.rs `bench`), and nothing writes it.
            let x = (unsafe { core::ptr::read_unaligned((feed.base as *const i16).add(feed.at)) }) as i32;
            let v = (x * g) >> 15;
            a[0] += v * l;
            a[1] += v * r;
            feed.at += 1;
        }
        m.sound = true;
    }
    for (o, a) in out.iter_mut().zip(acc.iter()) {
        *o = rail(a[0]) | rail(a[1]) << 16;
    }
    m
}
