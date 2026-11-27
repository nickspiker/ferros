//! The microphones (docs/AUDIO.md "Microphones"): husky's three built-in microphones captured thru the AoC (aoc.rs, the input control queue and the ring `audio_capture0`), for the Developer page's microphone test: a live level per microphone (`MIC_LEVELS`) and a recording of one played back on the speaker (`RECORD`).
//!
//! The capture runs only while someone asks: a meter's call held, or a recording under way. It is clocked by what the microphones send, the AoC ringing the capture ring's channel when that channel is on mailbox block f1 (the line this process holds); counted in the capture's own 10 ms periods, never a timer. A capture nobody asks for again within `IDLE_BLOCKS` periods is stopped, and the process ends once nothing else holds it.
//!
//! The recording stays in this process's memory (mix.rs `REC`), is played back once, and is gone with the process: nothing is written anywhere (AGENT.md rule -1, no new location; owner, 2026-11-22: RAM only).

use crate::{aoc, lock, mix, say, unlock, State};
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::reply;
use ferros_sys::audio::{MICS, RECORD_FRAMES, REPORT};

/// Frames a meter's answer covers: 2^12, 85 ms, so a live meter is answered about twelve times a second whatever the capture's period. Counted in frames since 2026-11-24 (the period is a capture's own: aoc.rs `Shape`).
const METER_FRAMES: usize = 1 << 12;
/// Frames with no meter held, no recording and no bench before the capture is stopped: 2^15, 683 ms, long past the gap between a meter's answer and its next call.
const IDLE_FRAMES: usize = 1 << 15;

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
    /// The capture's period and width, asked at its start.
    pub shape: aoc::Shape,
    /// The f1 channel bit the capture ring rings, or 0 when its channel is on a block whose line this process does not hold (then polled: `clock`).
    pub bit: u16,
    /// The next sample's microphone (the samples come interleaved), and frames into the current meter's window.
    phase: usize,
    frames: usize,
    /// Frames since the capture started, read or skipped: the capture's own clock, stamped on every measurement.
    pub total: u64,
    pub blocks: u32,
    idle: usize,
    /// The capture's turns that found bytes, and the bytes each found (least and most): how the AoC hands the samples over.
    pub reads: u32,
    pub read_min: u32,
    pub read_max: u32,
    /// The first words of a wide capture, as the AoC packs them (said once).
    raw_said: bool,
    /// Bytes the AoC wrote over before they were read (this process late), so far.
    pub skipped: u32,
    peaks: [u16; MICS],
    window: u32,
    /// The meter's held call.
    pub meter: Option<[u8; 32]>,
    pub rec: Option<Rec>,
    /// Publishing into the microphone mailbox for the holders of its view (`MIC_STREAM`, 2026-11-26), and the publishes since the kernel was last asked whether any process still holds one (`VIEW_CHECK`).
    pub stream: bool,
    since_check: u32,
    /// Frames the AoC wrote over before they were read, since the stream began: the mailbox's `OVERRUN`.
    overrun: u64,
}

/// The stream's shape (`MIC_STREAM`): 1 ms periods, 32-bit samples, the normal record path's three microphones (measured on the Pro 2026-11-24: whole milliseconds only, 1 ms and 32 bits both work).
const STREAM_SHAPE: aoc::Shape = aoc::Shape { ms: 1, width: aoc::WIDTH_32, gain: None, kind: aoc::REC_NORMAL };
/// Publishes between two looks at whether any process still holds the microphone mailbox's view (fact 90, the kernel's mark): 2^6, about 64 ms of 1 ms periods, counted on the capture's own clock, never a timer. Once none does, the stream stops: giving the view back is the stop, and a reader that ended takes its share with it.
const VIEW_CHECK: u32 = 1 << 6;

/// The microphone mailbox's writer, taken at the first stream (the `mic-mailbox` grant, slot 14). The clock thread's alone once taken.
static mut MIC_BOX: Option<ferros_sys::mailbox::Writer> = None;

/// Whether any process holds the microphone mailbox's view now: the kernel's own mark, fact 90's microphone bit (ferros_sys::uses).
fn view_held() -> bool {
    let word = ferros_sys::facts().0[ferros_sys::uses::FACT];
    ferros_sys::uses::live(word) >> ferros_sys::uses::MIC & 1 == 1
}

impl Mic {
    pub const NONE: Mic = Mic { cap: None, starting: false, stopping: false, shape: aoc::Shape::LINUX, bit: 0, phase: 0, frames: 0, total: 0, blocks: 0, idle: 0, reads: 0, read_min: u32::MAX, read_max: 0, raw_said: false, skipped: 0, peaks: [0; MICS], window: 0, meter: None, rec: None, stream: false, since_check: 0, overrun: 0 };
    /// Anything that keeps the process: a capture, its start or stop under way, a meter waiting, a recording.
    pub fn busy(&self) -> bool {
        // A bench holds the capture by its own flag (main.rs `Bench`), read with the rest of the state where this is called.
        self.cap.is_some() || self.starting || self.stopping || self.meter.is_some() || self.rec.is_some()
    }
}

/// The capture running, started in `shape` if it is not (the receiving thread, as `ensure_up`): true when capturing. Refused while it is being stopped or the process is ending, and when it runs in another shape.
pub fn ensure(shape: aoc::Shape) -> bool {
    {
        let st = lock();
        if st.mic.cap.is_some() {
            let same = st.mic.shape == shape;
            unlock();
            return same;
        }
        if st.mic.starting || st.mic.stopping || st.closing {
            unlock();
            return false;
        }
        st.mic.starting = true;
        unlock();
    }
    let r = start(shape);
    let st = lock();
    st.mic.starting = false;
    let why = match r {
        Ok((c, bit)) => {
            st.mic.cap = Some(c);
            st.mic.shape = shape;
            st.mic.bit = bit;
            st.mic.phase = 0;
            st.mic.frames = 0;
            st.mic.total = 0;
            st.mic.idle = 0;
            st.mic.window = 0;
            st.mic.reads = 0;
            st.mic.read_min = u32::MAX;
            st.mic.read_max = 0;
            st.mic.raw_said = false;
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

fn start(shape: aoc::Shape) -> Result<(aoc::Capture, u16), &'static str> {
    if !aoc::online() {
        return Err("the AoC is not up (the sensors process asks for its start at every boot)");
    }
    let mut c = aoc::capture_services_for(shape)?;
    // An MMAP record's samples go to the capture heap, mapped first as Android's HAL does (aoc.rs "The AAudio heaps").
    if shape.kind == aoc::REC_MMAP && aoc::heaps() {
        aoc::map_heap(aoc::MAP_CAPTURE, aoc::HEAP_CAPTURE, true)?;
        c.heap = true;
        say!("audio: the capture heap mapped into the AoC: A#{} bytes at G#{:X}", aoc::HEAP_CAPTURE.1, aoc::HEAP_CAPTURE.0);
    }
    aoc::describe("audio_input_control", c.control);
    aoc::describe_up("audio_capture0", c.ring);
    aoc::capture_start(&c, shape)?;
    say!("audio: capture asked for A#{} ms periods, width code A#{} (A#{} bytes a sample), A#{} bytes a period", shape.ms, shape.width, shape.sample_bytes(), shape.period_bytes());
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
    let t = ferros_sys::now();
    let bench = &mut st.bench;
    let m = &mut st.mic;
    let sb = m.shape.sample_bytes();
    let chans = m.shape.chans();
    // The stream's publish begun before anything goes into the ring (the sequence odd until `finish_mic`). MIC_BOX under the state lock, which both the clock thread's and the poller's turns hold.
    let writer = if m.stream { unsafe { (*(&raw const MIC_BOX)).as_ref() } } else { None };
    let publish = writer.map(|w| w.begin());
    let skipped = c.skip_overrun();
    if skipped != 0 {
        m.skipped = m.skipped.wrapping_add(skipped);
        // The skipped bytes are whole samples of the interleaved stream: the next one read belongs that many microphones on, and the clock moves by the frames they held.
        let samples = skipped as usize / sb;
        let was = m.total;
        m.total += ((m.phase + samples) / chans) as u64;
        m.phase = (m.phase + samples) % chans;
        // The frames written over are silence in the ring, never the samples a ring ago left in their slots; a reader sees how many in `OVERRUN`.
        if let Some(p) = publish.as_ref() {
            m.overrun += m.total - was;
            let ring = ferros_sys::mailbox::mic::RING_FRAMES as u64;
            // WHY the max: more frames skipped than the ring holds need only the ring's worth zeroed, every slot being one of them.
            // WHY the saturating subtract: in a capture's first ring of frames `m.total - ring` underflows a u64; PROOF: it would name a frame beyond every frame written, and the max would then zero nothing.
            for f in was.max(m.total.saturating_sub(ring))..m.total {
                for ch in 0..chans {
                    ferros_sys::mailbox::mic::put(p, f, ch, 0);
                }
            }
        }
    }
    // SAFETY: REC is this thread's alone while a recording is under way (mix.rs reads it only once the recording has ended).
    let rec = unsafe { &mut *(&raw mut mix::REC) };
    let (mut phase, mut frames, mut peaks) = (m.phase, 0usize, m.peaks);
    let mut r = m.rec.as_ref().map(|r| (r.mic as usize, r.n, r.peak));
    let total0 = m.total;
    let mut raw = [0u32; 6];
    let mut raws = 0usize;
    let said = m.raw_said;
    let n = c.read(sb as u64, |x, w| {
        if !said && raws < raw.len() {
            raw[raws] = w;
            raws += 1;
        }
        bench.sample(phase, x, total0 + frames as u64);
        // The stream: every sample at its frame's position, full scale as an i32 whatever the width (a 32-bit capture's word as it came, a 16-bit one's sample in the top half).
        if let Some(p) = publish.as_ref() {
            ferros_sys::mailbox::mic::put(p, total0 + frames as u64, phase, if sb == 4 { w as i32 } else { (x as i32) << 16 });
        }
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
        if phase == chans {
            phase = 0;
            frames += 1;
        }
    });
    m.phase = phase;
    m.peaks = peaks;
    m.total += frames as u64;
    // The stream's publish whole: the frames before the position in, the readers woken; the anchor is the position not yet arrived at the turn's start, its counter `t`.
    if let (Some(w), Some(p)) = (writer, publish) {
        let h = ferros_sys::mailbox::mic::Head { pos: m.total, rate: ferros_sys::audio::RATE, channels: chans as u8, anchor_pos: m.total, anchor_at: t, overrun: m.overrun };
        w.finish_mic(p, &h);
        m.since_check += 1;
    }
    let check = m.stream && m.since_check >= VIEW_CHECK;
    if check {
        m.since_check = 0;
    }
    if n != 0 {
        m.reads = m.reads.wrapping_add(1);
        m.read_min = m.read_min.min(n);
        m.read_max = m.read_max.max(n);
        // The capture's frames now in: what a bench's detection is measured against (main.rs `Bench::read`).
        bench.read(t, m.total);
    }
    let wide = if !m.raw_said && raws != 0 && sb == 4 {
        m.raw_said = true;
        Some(raw)
    } else {
        None
    };
    if let (Some(rr), Some((_, n, peak))) = (m.rec.as_mut(), r) {
        rr.n = n;
        rr.peak = peak;
    }
    m.frames += frames;
    let pf = m.shape.period_frames();
    let blocks = (m.frames / pf) as u32;
    m.frames %= pf;
    m.blocks = m.blocks.wrapping_add(blocks);
    m.window += frames as u32;
    if m.window as usize >= METER_FRAMES {
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
    if m.meter.is_none() && m.rec.is_none() && !bench.capturing() && !m.stream {
        m.idle += frames;
    } else {
        m.idle = 0;
    }
    let done = if m.rec.as_ref().is_some_and(|r| r.n == RECORD_FRAMES) { m.rec.take() } else { None };
    let stop = m.idle >= IDLE_FRAMES;
    let bit = m.bit;
    if stop {
        m.cap = None;
        m.stopping = true;
    }
    unlock();
    // No process holds the microphone mailbox's view any more (the kernel's mark): the stream stops, and the capture with it once nothing else wants it (`IDLE_FRAMES`). Asked with the lock let go.
    if check && !view_held() {
        let st = lock();
        st.mic.stream = false;
        unlock();
        say!("audio: the microphone stream stopped: no process holds its view");
    }
    if let Some(w) = wide {
        say!("audio: the capture's first words: G#{:X} G#{:X} G#{:X} G#{:X} G#{:X} G#{:X}", w[0], w[1], w[2], w[3], w[4], w[5]);
    }
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
    let played = crate::ensure_up(crate::Play::DEFAULT);
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
    if !ensure(aoc::Shape::LINUX) {
        reply(&token, &[2]);
        return;
    }
    let st = lock();
    st.mic.rec = Some(Rec { mic, level, n: 0, peak: 0, token });
    unlock();
    say!("audio: recording microphone A#{mic}: A#{RECORD_FRAMES} frames");
}

/// `MIC_STREAM` (the `mic` thread, 2026-11-26): the capture published into the microphone mailbox for the holders of its view, started in the stream's shape if it is not running: [0] streaming, [1] refused (no process holds the view, the mailbox not ours to write, the microphones would not start, or they run in another shape for the meter).
pub fn stream(token: [u8; 32]) {
    if !view_held() {
        reply(&token, &[1]);
        return;
    }
    // The writer taken once, at the first stream (slot 14); the one writer of the mailbox from then on.
    let taken = {
        lock();
        let t = unsafe { (*(&raw const MIC_BOX)).is_some() };
        unlock();
        t
    };
    if !taken {
        let Some(w) = ferros_sys::mailbox::Writer::take(ferros_sys::cap(crate::grants(), crate::S_MIC_BOX)) else {
            say!("audio: the microphone mailbox could not be taken (its grant, slot 14, or another writer): no stream");
            reply(&token, &[1]);
            return;
        };
        lock();
        unsafe { *(&raw mut MIC_BOX) = Some(w) };
        unlock();
    }
    if !ensure(STREAM_SHAPE) {
        reply(&token, &[1]);
        return;
    }
    let st = lock();
    if !st.mic.stream {
        st.mic.stream = true;
        st.mic.since_check = 0;
        st.mic.overrun = 0;
        say!("audio: the microphones stream into their mailbox, 1 ms periods at 32 bits");
    }
    unlock();
    reply(&token, &[0]);
}

/// `MIC_LEVELS` (the receiving thread): the capture running in `shape`, the call held until the meter's window ends (`turn`); a meter already held is answered [1], replaced.
pub fn levels(token: [u8; 32], shape: aoc::Shape) {
    if !ensure(shape) {
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
