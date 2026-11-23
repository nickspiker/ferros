//! The microphones (docs/AUDIO.md "Microphones"): husky's three built-in microphones captured thru the AoC (aoc.rs, the input control queue and the ring `audio_capture0`), for the Developer page's microphone test: a live level per microphone (`MIC_LEVELS`) and a recording of one played back on the speaker (`RECORD`).
//!
//! The capture runs only while someone asks: a meter's call held, or a recording under way. It is clocked by what the microphones send, the AoC ringing the capture ring's channel when that channel is on mailbox block f1 (the line this process holds); counted in the capture's own 10 ms periods, never a timer. A capture nobody asks for again within `IDLE_BLOCKS` periods is stopped, and the process ends once nothing else holds it.
//!
//! The recording stays in this process's memory (mix.rs `REC`), is played back once, and is gone with the process: nothing is written anywhere (AGENT.md rule -1, no new location; owner, 2026-11-22: RAM only).

use crate::{aoc, lock, mix, say, unlock, State};
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::reply;
use ferros_sys::audio::{MICS, RECORD_FRAMES, REPORT};

/// Capture periods (10 ms each) a meter's answer covers: 8, 80 ms, so a live meter is answered about twelve times a second.
const METER_BLOCKS: u32 = 1 << 3;
/// Capture periods with no meter held and no recording before the capture is stopped: 64, 640 ms, long past the gap between a meter's answer and its next call.
const IDLE_BLOCKS: u32 = 1 << 6;

// The capture's shape is aoc.rs's (three microphones interleaved, 16-bit, 480 frames a period); the protocol's count of microphones must be the same.
const _: () = assert!(MICS == aoc::MICS);

/// A recording under way: which microphone, the level to play it back at, frames taken, the largest magnitude seen, and the call to answer once it has played.
pub struct Rec {
    mic: u8,
    level: u8,
    n: usize,
    peak: u16,
    token: [u8; 32],
}

pub struct Mic {
    /// The capture running: its services.
    pub cap: Option<aoc::Capture>,
    /// Being started or stopped (its commands under way on the input queue, which one thread at a time speaks).
    pub starting: bool,
    pub stopping: bool,
    /// The f1 channel bit the capture ring rings, or 0 when its channel is on a block whose line this process does not hold (then polled: `clock`).
    pub bit: u16,
    /// The next sample's microphone (the samples come interleaved), and frames into the current period.
    phase: usize,
    frames: usize,
    pub blocks: u32,
    idle: u32,
    /// Bytes the AoC wrote over before they were read (this process late), so far.
    pub skipped: u32,
    peaks: [u16; MICS],
    window: u32,
    /// The meter's held call.
    pub meter: Option<[u8; 32]>,
    pub rec: Option<Rec>,
}

impl Mic {
    pub const NONE: Mic = Mic { cap: None, starting: false, stopping: false, bit: 0, phase: 0, frames: 0, blocks: 0, idle: 0, skipped: 0, peaks: [0; MICS], window: 0, meter: None, rec: None };
    /// Anything that keeps the process: a capture, its start or stop under way, a meter waiting, a recording.
    pub fn busy(&self) -> bool {
        self.cap.is_some() || self.starting || self.stopping || self.meter.is_some() || self.rec.is_some()
    }
}

/// The capture running, started if it is not (the receiving thread, as `ensure_up`): true when capturing. Refused while it is being stopped or the process is ending.
pub fn ensure() -> bool {
    {
        let st = lock();
        if st.mic.cap.is_some() {
            unlock();
            return true;
        }
        if st.mic.starting || st.mic.stopping || st.closing {
            unlock();
            return false;
        }
        st.mic.starting = true;
        unlock();
    }
    let r = start();
    let st = lock();
    st.mic.starting = false;
    let why = match r {
        Ok((c, bit)) => {
            st.mic.cap = Some(c);
            st.mic.bit = bit;
            st.mic.phase = 0;
            st.mic.frames = 0;
            st.mic.idle = 0;
            st.mic.window = 0;
            st.mic.peaks = [0; MICS];
            None
        }
        Err(e) => Some(e),
    };
    unlock();
    match why {
        Some(e) => say!("audio: the microphones would not start: {e}"),
        // A capture the AoC's line does not clock: its reader woken.
        None => {
            POLL.fetch_add(1, Ordering::AcqRel);
            ferros_sys::wake(&POLL, 1);
        }
    }
    why.is_none()
}

fn start() -> Result<(aoc::Capture, u16), &'static str> {
    if !aoc::online() {
        return Err("the AoC is not up (the sensors process asks for its start at every boot)");
    }
    let c = aoc::capture_services()?;
    aoc::describe("audio_input_control", c.control);
    aoc::describe_up("audio_capture0", c.ring);
    aoc::capture_start(&c)?;
    let ch = c.irq();
    let bit = if (16..32).contains(&ch) { 1u16 << (ch - 16) } else { 0 };
    if bit != 0 {
        aoc::listen(1, bit, true);
        say!("audio: capturing microphones A#0 to A#{}: the AoC rings channel A#{ch} (block f1) as it writes", MICS - 1);
    } else {
        say!("audio: capturing microphones A#0 to A#{}: the ring's channel A#{ch} is not on block f1, so it is read every 128th of a second", MICS - 1);
    }
    Ok((c, bit))
}

/// Rung when a capture starts that the AoC's line cannot clock (`poller`).
static POLL: AtomicU64 = AtomicU64::new(0);

/// The capture's reader when its ring's channel is not on block f1 (the AoC rings a line this process does not hold): asleep while no such capture runs; while one does, the ring read every 128th of a second. WHY a period here, the one timer in this process: nothing rings for that ring, and Linux reads a ring that is not interrupt-driven the same way, every 10 ms (aoc_alsa.h:69, pcm.c:258-278); 1/128 s is under the AoC's 10 ms period, so each read finds at most about one. Which case the AoC's channel is, the console says at the first capture; with f1's channel this thread never wakes.
pub extern "C" fn poller(_: u64) -> ! {
    loop {
        let v = POLL.load(Ordering::Acquire);
        let polled = {
            let st = lock();
            let p = st.mic.cap.is_some() && st.mic.bit == 0;
            unlock();
            p
        };
        if !polled {
            ferros_sys::wait(&POLL, v, 0);
            continue;
        }
        ferros_sys::sleep_until(ferros_sys::now() + (ferros_sys::freq() >> 7));
        turn(0);
    }
}

/// The clock thread's turn for the microphones (main.rs `clock`): what the AoC wrote since the last turn read, each sample into its microphone's peak and, while recording, the chosen microphone's into the recording; a meter answered at the end of its window; a finished recording played back; an unwanted capture stopped.
pub fn turn(rang: u16) {
    let st = lock();
    let Some(c) = st.mic.cap else {
        unlock();
        return;
    };
    // With an f1 channel, a turn reads when that channel rang; without one (the poller's turns), every turn reads.
    if st.mic.bit != 0 && rang & st.mic.bit == 0 {
        unlock();
        return;
    }
    let m = &mut st.mic;
    let skipped = c.skip_overrun();
    if skipped != 0 {
        m.skipped = m.skipped.wrapping_add(skipped);
        // The skipped bytes are whole samples of the interleaved stream: the next one read belongs that many microphones on.
        m.phase = (m.phase + (skipped as usize >> 1)) % MICS;
    }
    // SAFETY: REC is this thread's alone while a recording is under way (mix.rs reads it only once the recording has ended).
    let rec = unsafe { &mut *(&raw mut mix::REC) };
    let (mut phase, mut frames, mut peaks) = (m.phase, 0usize, m.peaks);
    let mut r = m.rec.as_ref().map(|r| (r.mic as usize, r.n, r.peak));
    c.read(|x| {
        let a = x.unsigned_abs();
        // A running maximum, the meter's peak: the larger of two magnitudes, nothing held to a range.
        if a > peaks[phase] {
            peaks[phase] = a;
        }
        if let Some((mic, n, peak)) = r.as_mut() {
            // The recording's length is the buffer's: frames past it are the meter's alone.
            if phase == *mic && *n < RECORD_FRAMES {
                rec[*n] = x;
                *n += 1;
                if a > *peak {
                    *peak = a;
                }
            }
        }
        phase += 1;
        if phase == MICS {
            phase = 0;
            frames += 1;
        }
    });
    m.phase = phase;
    m.peaks = peaks;
    if let (Some(rr), Some((_, n, peak))) = (m.rec.as_mut(), r) {
        rr.n = n;
        rr.peak = peak;
    }
    m.frames += frames;
    let blocks = (m.frames / aoc::CAPTURE_PERIOD) as u32;
    m.frames %= aoc::CAPTURE_PERIOD;
    m.blocks = m.blocks.wrapping_add(blocks);
    m.window += blocks;
    if m.window >= METER_BLOCKS {
        m.window = 0;
        if let Some(t) = m.meter.take() {
            let mut out = [0u8; 1 + 2 * MICS + 8];
            for (k, p) in m.peaks.iter().enumerate() {
                out[1 + 2 * k..3 + 2 * k].copy_from_slice(&p.to_le_bytes());
            }
            out[1 + 2 * MICS..5 + 2 * MICS].copy_from_slice(&m.skipped.to_le_bytes());
            out[5 + 2 * MICS..].copy_from_slice(&m.blocks.to_le_bytes());
            reply(&t, &out);
        }
        m.peaks = [0; MICS];
    }
    if m.meter.is_none() && m.rec.is_none() {
        m.idle += blocks;
    } else {
        m.idle = 0;
    }
    let done = if m.rec.as_ref().is_some_and(|r| r.n == RECORD_FRAMES) { m.rec.take() } else { None };
    let stop = m.idle >= IDLE_BLOCKS;
    let bit = m.bit;
    if stop {
        m.cap = None;
        m.stopping = true;
    }
    unlock();
    if let Some(r) = done {
        recorded(r);
    }
    if stop {
        if bit != 0 {
            aoc::listen(1, bit, false);
        }
        aoc::capture_stop(&c);
        say!("audio: microphones stopped (nobody listening)");
        let st = lock();
        st.mic.stopping = false;
        unlock();
        crate::end_if_idle();
    }
}

/// A recording full: played back once at its level, scaled from its own peak (mix.rs `Clip::start`), and its call answered with the peak, whether it played, and the amplifiers' report. The sound brought up from this thread (`ensure_up`: a recording ends on the capture's clock, with nobody's call to start it from).
fn recorded(r: Rec) {
    say!("audio: recorded A#{RECORD_FRAMES} frames of microphone A#{}, its peak A#{}", r.mic, r.peak);
    // WHY: the playback's gain is the level over the peak; a microphone that gave nothing but zeros has no peak to scale from, and nothing to hear.
    if r.peak == 0 {
        say!("audio: microphone A#{} gave silence: nothing played", r.mic);
        reply(&r.token, &[3]);
        return;
    }
    let played = crate::ensure_up();
    let st: &mut State = lock();
    if played {
        st.clip.start(RECORD_FRAMES, r.peak, r.level);
    }
    let rep = crate::report(st, 0);
    unlock();
    if played {
        say!("audio: playing the recording back at level A#{}", r.level);
    }
    let mut out = [0u8; 4 + REPORT];
    out[1..3].copy_from_slice(&r.peak.to_le_bytes());
    out[3] = played as u8;
    out[4..].copy_from_slice(&rep[1..]);
    reply(&r.token, &out);
}

/// `RECORD`'s start (the receiving thread): checked, the capture running, the recording set; its call held until the recording has played (`recorded`).
pub fn record(token: [u8; 32], mic: u8, level: u8) {
    // WHY: a microphone past husky's three has no channel in the capture; the level is the playback's, held to the speaker test's ceiling (ferros_sys audio `TONE_MAX`, the unprotected speaker), level 0 being the device's default volume, far past it.
    if mic as usize >= MICS || level == 0 || level > ferros_sys::audio::TONE_MAX {
        say!("audio: recording refused: microphone A#{mic}, level A#{level}");
        reply(&token, &[1]);
        return;
    }
    {
        let st = lock();
        let busy = st.mic.rec.is_some();
        unlock();
        if busy {
            reply(&token, &[1]);
            return;
        }
    }
    if !ensure() {
        reply(&token, &[2]);
        return;
    }
    let st = lock();
    st.mic.rec = Some(Rec { mic, level, n: 0, peak: 0, token });
    unlock();
    say!("audio: recording microphone A#{mic}: A#{RECORD_FRAMES} frames");
}

/// `MIC_LEVELS` (the receiving thread): the capture running, the call held until the meter's window ends (`turn`); a meter already held is answered [1], replaced.
pub fn levels(token: [u8; 32]) {
    if !ensure() {
        reply(&token, &[2]);
        return;
    }
    let st = lock();
    let old = st.mic.meter.replace(token);
    unlock();
    if let Some(o) = old {
        reply(&o, &[1]);
    }
}
