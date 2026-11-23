//! The mixer: every stream's ring, a period at a time, into the one the AoC plays (docs/AUDIO.md "Mixing").
//!
//! A frame is one word, the left sample in its low half and the right in its high half (S16_LE interleaved, as kists write and the AoC reads), so a frame moves as one load and one store.

use ferros_sys::audio::{gain_q15, ALERT, LEFT, PERIOD, RECORD_FRAMES, RIGHT, RING, WRITE_MAX};
use ferros_sys::{dnd, pref, PREF_SOUND, SOUND};

/// Streams at once: a power of two, more than a device plays together (a game, a call, a notification).
pub const STREAMS: usize = 1 << 3;
/// A stream whose ring has been empty this many periods with no write waiting (about 1.4 s) is closed: its kist paused, or ended without closing (an ending process says nothing to its servers). Counted in periods played, the hardware's clock, never a timer. A write to it later answers "unknown handle", and the kist opens again.
pub const STALE: u32 = 1 << 8;

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
    /// Periods in a row its ring has been empty.
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
const SINE: [i16; 48] = [
    0, 4277, 8481, 12539, 16383, 19947, 23170, 25996, 28377, 30273, 31650, 32487, 32767, 32487, 31650, 30273, 28377, 25996, 23170, 19947, 16383, 12539, 8481, 4277, 0, -4277, -8481, -12539, -16383, -19947, -23170, -25996, -28377, -30273, -31650, -32487, -32767, -32487, -31650, -30273, -28377, -25996, -23170, -19947, -16383, -12539, -8481, -4277,
];

/// The service's own tone: periods left to play, its gain, where in the cycle it is, its channels, and whether its next period is its first (faded in).
pub struct Tone {
    pub left: u32,
    pub gain: u32,
    phase: usize,
    /// `audio::LEFT` | `audio::RIGHT`.
    pub mask: u8,
    first: bool,
}

impl Tone {
    pub const NONE: Tone = Tone { left: 0, gain: 0, phase: 0, mask: 0, first: false };
    /// Started, or changed while it plays (its level or channels; a playing tone is not faded in again).
    pub fn start(&mut self, level: u8, periods: u32, mask: u8) {
        self.first = self.left == 0;
        self.left = periods;
        self.gain = gain_q15(level);
        self.mask = mask;
    }
    /// Stopped: its next period is its last, faded out.
    pub fn stop(&mut self) {
        if self.left > 1 {
            self.left = 1;
        }
    }
}
/// The microphone test's recording (mic.rs): one microphone's samples, kept in this process's memory only (never written anywhere: AGENT.md rule -1), played back once and gone with the process. `audio::RECORD_FRAMES` of them, 2.7 s.
pub static mut REC: [i16; RECORD_FRAMES] = [0; RECORD_FRAMES];

/// The recording played back: frames left, where it is, its gain, and whether its next period is its first (faded in).
pub struct Clip {
    pub left: usize,
    at: usize,
    /// The gain in Q15 that takes the recording's own peak to the level asked for: `start`.
    k: i64,
    first: bool,
}

impl Clip {
    pub const NONE: Clip = Clip { left: 0, at: 0, k: 0, first: false };
    /// `n` frames of `REC` played from the start, scaled so their peak (`peak`, at least 1) comes out at the level's amplitude (`gain_q15(level)`, in sample units): the loudest sample played is the level asked for and no louder, whatever the microphone picked up. With the level at most `TONE_MAX` that is 2048, 24 dB under full scale, the speaker test's own ceiling (ferros_sys audio `TONE_MAX`).
    pub fn start(&mut self, n: usize, peak: u16, level: u8) {
        self.left = n;
        self.at = 0;
        self.k = ((gain_q15(level) as i64) << 15) / peak as i64;
        self.first = true;
    }
    /// Stopped: faded out over at most one more period (the smaller of two counts: what is left, or a period).
    pub fn stop(&mut self) {
        if self.left > PERIOD {
            self.left = PERIOD;
        }
    }
}

/// What a period's mixing found.
pub struct Mixed {
    /// A stream that had been playing ran short (its kist late).
    pub underruns: u32,
    /// Anything sounded: a live stream with frames, or the tone.
    pub sound: bool,
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

/// One period mixed into `out`: every live stream at its gain times the device's volume (alerts silent unless the sound mode is `SOUND` and do not disturb is off: preferences 39 and 40), and the tone; the streams' rings advanced, the stale and the played-out freed.
pub fn period(streams: &mut [Stream; STREAMS], tone: &mut Tone, clip: &mut Clip, out: &mut [u32; PERIOD]) -> Mixed {
    let mut acc = [[0i32; 2]; PERIOD];
    let volume = gain_q15(ferros_sys::audio::level());
    let alerts = pref(PREF_SOUND) == SOUND && !dnd::on();
    let mut m = Mixed { underruns: 0, sound: false };
    for s in streams.iter_mut().filter(|s| s.live) {
        // The two gains are each at most 2^16 and 2^15, so their product fits in u32 before the shift; the result at most 65535, and a sample times it fits in i32 (32768 · 65535 < 2^31).
        let g = if s.class == ALERT && !alerts { 0 } else { (s.gain as u32 * volume) >> 15 } as i32;
        // A stream short of a period plays what it has: the count of frames to take, the smaller of two counts, not a value held to a range.
        let take = if s.fill < PERIOD { s.fill } else { PERIOD };
        for (k, a) in acc[..take].iter_mut().enumerate() {
            let f = s.ring[(s.head + k) & (RING - 1)];
            a[0] += (f as u16 as i16 as i32 * g) >> 15;
            a[1] += ((f >> 16) as u16 as i16 as i32 * g) >> 15;
        }
        s.head = (s.head + take) & (RING - 1);
        s.fill -= take;
        m.sound |= take != 0;
        if take < PERIOD {
            // Short after playing: an underrun. Empty and quiet: a dry period, counted to `STALE`.
            if take != 0 && !s.closing {
                m.underruns += 1;
            }
            if take == 0 && !s.held {
                s.dry += 1;
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
        for (k, a) in acc.iter_mut().enumerate() {
            // The envelope in 256ths (a period is 256 frames): rising over the first period, falling over the last, so the tone starts and stops without the step a cut sine makes (a click). A one-period tone only rises.
            let env = if tone.first { k } else if tone.left == 1 { PERIOD - 1 - k } else { PERIOD } as i32;
            let v = ((SINE[tone.phase] as i32 * g) >> 15) * env >> 8;
            a[0] += v * l;
            a[1] += v * r;
            tone.phase += 1;
            if tone.phase == SINE.len() {
                tone.phase = 0;
            }
        }
        tone.first = false;
        tone.left -= 1;
        m.sound = true;
    }
    if clip.left != 0 {
        // SAFETY: REC is written only by the clock thread while recording (mic.rs) and read only here, on the same thread, after the recording has ended.
        let rec = unsafe { &*(&raw const REC) };
        // A clip shorter than a period plays what it has: the count of frames to take, the smaller of two counts.
        let take = if clip.left < PERIOD { clip.left } else { PERIOD };
        for (j, a) in acc[..take].iter_mut().enumerate() {
            // The same envelope as the tone's: rising over the first period, falling over the frames of the last.
            let env = if clip.first { j as i64 } else if clip.left <= PERIOD { (((take - j) as i64) << 8) / take as i64 } else { PERIOD as i64 };
            let v = ((rec[clip.at + j] as i64 * clip.k) >> 15) * env >> 8;
            a[0] += v as i32;
            a[1] += v as i32;
        }
        clip.at += take;
        clip.left -= take;
        clip.first = false;
        m.sound = true;
    }
    for (o, a) in out.iter_mut().zip(acc.iter()) {
        *o = rail(a[0]) | rail(a[1]) << 16;
    }
    m
}
