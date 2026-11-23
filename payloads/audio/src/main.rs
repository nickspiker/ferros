//! The audio service (docs/AUDIO.md): sound out of the speakers for every kist that holds the `audio` right. It owns the AoC's playback entry point 5 and the two CS35L41 amplifiers, mixes every open stream into one, and is clocked by the AoC: each period the AoC takes from its ring, it rings the AP on mailbox channel 20 (aoc_alsa.h:241 `PCM_CHANNEL`; husky's dmesg:7430 "audio_playback5 supports interrupt-driven"), and that interrupt is when this service mixes the next period and answers the writes that now fit. No timer anywhere on the sound's path.
//!
//! The kernel starts it at the first call on `audio` (process.rs `ON_DEMAND`, service.rs `audio_grants`), and it ends itself once every stream has closed and played out: silence holds no arc, and the amplifiers go back to reset (the `amp` switch is off when the process that held it ends). The next call starts another.
//!
//! Spawn by hand (development; the kernel's own start is the same grants): ferros-bridge spawn audio.bin --name audio --serve audio --dma aoc-ipc --hw aoc-mbox --irq aoc-mbox-f1 --hw usi1-spi --hw gpio-gpn0 --power amp --entropy --dma aoc-play --dma aoc-capture
//! Grant slots, in that order: 0 console, 1 the `audio` endpoint (serve), 2 the AoC's IPC block, 3 its mailboxes, 4 mailbox block f1's AoC-to-AP line, 5 the amplifiers' SPI port, 6 gpio-gpn0 (the right amplifier's select), 7 the `amp` switch, 8 entropy (the streams' handles), 9 the playback ring's window (`aoc-play`), 10 the capture ring's (`aoc-capture`).
//!
//! The endpoint's protocol is ferros_sys `audio` (its ops, the format, the ring). The AoC must be up (the sensors process asks the kernel to start it at every boot); without it an open is refused.
//!
//! The microphones are this service's too (mic.rs, since 2026-11-22): a capture thru the AoC's input queue and ring for the Developer page's level meter and its record-then-play-back, started by the first call that wants it and stopped once nobody asks.
//!
//! Two threads. This one receives and answers; the clock thread waits on the AoC's interrupt, mixes, writes the AoC's ring and answers held writes. Both work on one state under one lock (a futex word: `lock`), held for a copy or a period's mix, never across a call to the AoC.
#![no_std]
#![no_main]

mod amp;
mod aoc;
mod mic;
mod mix;

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::audio::{self, ERRORS, LEFT, PERIOD, RATE, REPORT, RIGHT, RING, TONE_MAX, WRITE_MAX};
use ferros_sys::{cap, entropy, irq_ack, irq_wait, recv, reply, ERR};

pub const S_AUDIO: u64 = 1;
pub const S_IPC: u64 = 2;
pub const S_MBOX: u64 = 3;
pub const S_IRQ: u64 = 4;
pub const S_SPI: u64 = 5;
pub const S_GPN: u64 = 6;
pub const S_POWER: u64 = 7;
pub const S_ENTROPY: u64 = 8;
/// The playback and capture rings' windows past `aoc-ipc` (kernel cachemap.rs AOC_PLAY, AOC_CAPTURE; since 2026-11-22).
pub const S_PLAY: u64 = 9;
pub const S_CAPTURE: u64 = 10;

static GRANTS: AtomicU64 = AtomicU64::new(0);
pub fn grants() -> u64 {
    GRANTS.load(Ordering::Relaxed)
}

/// A console line, formatted into a fixed buffer (this process has no heap).
pub struct Line {
    pub b: [u8; 256],
    pub n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // A line longer than the buffer is cut, a byte kept for its newline: the text is this process's own and the cut is what it should do.
        let k = s.len().min(self.b.len() - 1 - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}
#[macro_export]
macro_rules! say {
    ($($t:tt)*) => {{
        let mut l = $crate::Line { b: [0; 256], n: 0 };
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!($($t)*));
        l.b[l.n] = b'\n';
        ferros_sys::console($crate::grants(), &l.b[..l.n + 1]);
    }};
}

/// Periods kept queued in the AoC's ring ahead of what it plays: two, 10.7 ms. With a stream's ring (`RING`, 21.3 ms at most) that is the latency's budget under the 40 ms a game wants (AUDIO.md "Latency"), the AoC's own pipeline on top, measured on the device.
const LEAD: usize = 2;
/// Bytes a period, as the AoC's ring counts.
const PERIOD_BYTES: u32 = (PERIOD * audio::FRAME_BYTES) as u32;
/// Periods of silence played after the last stream closed, before the source goes off and the amplifiers down: the ring and the AoC's pipeline emptied, the end of the sound not cut off.
const TAIL: u32 = 1 << 4;

pub struct State {
    /// The AoC playing entry point 5 and the amplifiers on.
    up: bool,
    /// The AoC's own generator playing (`SIGGEN`): the process stays until it is stopped.
    siggen: bool,
    /// Ending after an amplifier's fault: opens are refused from here, the process ends once the next call has been told.
    closing: bool,
    /// The sound being brought up (`ensure_up`) or down after its tail (`clock`): a second start waits for neither, it is refused.
    starting: bool,
    stopping: bool,
    services: Option<aoc::Services>,
    streams: [mix::Stream; mix::STREAMS],
    tone: mix::Tone,
    /// The microphone test's recording played back.
    clip: mix::Clip,
    /// The microphones (mic.rs).
    mic: mic::Mic,
    /// A handle's second byte: a stream slot's generation, so a freed slot's old handle stays unknown.
    generation: [u8; mix::STREAMS],
    periods: u64,
    underruns: u32,
    /// The AoC found its ring empty at an interrupt: this service late.
    starved: u32,
    irqs: u32,
    quiet: u32,
    /// Both amplifiers' (IRQ1_STATUS1, IRQ1_STATUS3) as last read: at GLOBAL_EN, then every period while sound plays (`clock`).
    amps: [(u32, u32); 2],
    /// An amplifier's error bit stopped everything (`fault`): the process stays, silent, until the next call has been told, then ends.
    fault: bool,
    /// The watch's reads failing (said once, at the first).
    watch_failed: u32,
}

static mut STATE: State = State { up: false, siggen: false, closing: false, starting: false, stopping: false, services: None, streams: [const { mix::Stream::EMPTY }; mix::STREAMS], tone: mix::Tone::NONE, clip: mix::Clip::NONE, mic: mic::Mic::NONE, generation: [0; mix::STREAMS], periods: 0, underruns: 0, starved: 0, irqs: 0, quiet: 0, amps: [(0, 0); 2], fault: false, watch_failed: 0 };

/// 0 free, 1 held, 2 held with a waiter.
static LOCK: AtomicU64 = AtomicU64::new(0);
pub fn lock() -> &'static mut State {
    if LOCK.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
        while LOCK.swap(2, Ordering::Acquire) != 0 {
            ferros_sys::wait(&LOCK, 2, 0);
        }
    }
    unsafe { &mut *(&raw mut STATE) }
}
pub fn unlock() {
    if LOCK.swap(0, Ordering::Release) == 2 {
        ferros_sys::wake(&LOCK, 1);
    }
}

/// Everything from silence to playing: the AoC's services found, the amplifiers up, entry point 5 bound to the speaker and set up (the order Linux uses on husky, dmesg:7426-7437: bind, then the stream's prepare, then the amplifiers' power, then the source on at the trigger: hw.c:1322, pcm.c:533-622, cs35l41.c:2264-2299, hw.c:2685-2705), `LEAD` periods of silence queued, the f1 line listening.
fn start() -> Result<aoc::Services, &'static str> {
    if !aoc::online() {
        return Err("the AoC is not up (the sensors process asks for its start at every boot)");
    }
    for name in [&b"audio_output_control"[..], b"audio_playback5", b"audio_input_control", b"audio_capture0"] {
        aoc::dump(name);
    }
    let s = aoc::services()?;
    aoc::describe("audio_output_control", s.control);
    aoc::describe("audio_playback5", s.playback);
    // The playback channel must be on f1, the block whose line this process holds.
    let ch = s.playback.irq();
    if !(16..32).contains(&ch) {
        return Err("audio_playback5's channel is not on mailbox block f1");
    }
    // WHY: `LEAD` periods ahead must fit in the AoC's ring, whose size is the AoC's.
    if s.ring_bytes() < LEAD as u32 * PERIOD_BYTES {
        return Err("audio_playback5's ring is shorter than the lead");
    }
    let amps = amp::Amps::new();
    amps.up()?;
    aoc::bind(&s, aoc::EP, aoc::SINK_SPEAKER, true)?;
    s.reset_write();
    aoc::setup(&s, aoc::MODE_PLAYBACK, LEAD as u32 * PERIOD_BYTES, PERIOD_BYTES)?;
    let silence = [0u32; PERIOD];
    for _ in 0..LEAD {
        s.write(&silence);
    }
    let w = amps.on()?;
    {
        let st = lock();
        st.amps = w;
        st.fault = (w[0].0 | w[1].0) & ERRORS != 0;
        unlock();
    }
    // An error bit at GLOBAL_EN: no source, nothing sent; the process ends at its next turn and the switch puts both in reset.
    if (w[0].0 | w[1].0) & ERRORS != 0 {
        return Err("an amplifier reports an error at power-up (the report has its bits)");
    }
    aoc::listen(0, 1 << (ch - 16), true);
    aoc::source(&s, aoc::EP, true)?;
    say!("audio: playing: entry point A#{} to the speaker, A#{} periods of A#{} frames ahead, the AoC's ring A#{} bytes", aoc::EP, LEAD, PERIOD, s.ring_bytes());
    Ok(s)
}

/// Back to silence: the source off (hw.c:2711-2729), the amplifiers down, the speaker unbound (hw.c:1662-1687), the playback's channel no longer listened to. The process ends after it unless the microphones still run (`end_if_idle`); its `amp` switch goes off with it, both amplifiers in reset.
fn down(s: &aoc::Services) {
    aoc::listen(0, 0xFFFF, false);
    let _ = aoc::source(s, aoc::EP, false);
    amp::Amps::new().off();
    let _ = aoc::bind(s, aoc::EP, aoc::SINK_SPEAKER, false);
    say!("audio: speaker silent");
}

/// Nothing left to do (no sound, no stream, no capture, nothing starting or stopping, no fault waiting to be told): the process ends, from the clock thread (the receiving thread's own check is `exit_if_idle`).
pub fn end_if_idle() {
    let st = lock();
    let idle = idle(st) && !st.fault;
    unlock();
    if idle {
        say!("audio: silent, ending");
        ferros_sys::exit(0);
    }
}

fn idle(st: &State) -> bool {
    !st.up && !st.siggen && !st.starting && !st.stopping && !st.mic.busy() && st.streams.iter().all(|x| !x.live)
}

/// The clock: the AoC rang (a period taken, or the microphones' ring written), so the microphones' samples are taken (mic.rs `turn`), the playback ring topped up to `LEAD` periods, each a period mixed, and held writes that now fit answered.
extern "C" fn clock(_: u64) -> ! {
    let irq = cap(grants(), S_IRQ);
    loop {
        irq_wait(irq, 0);
        let rang = aoc::take();
        irq_ack(irq);
        ferros_sys::pet();
        mic::turn(rang);
        play_turn(rang);
    }
}

/// The playback's turn at an interrupt.
fn play_turn(rang: u16) {
    {
        let st = lock();
        st.irqs = st.irqs.wrapping_add((rang != 0) as u32);
        if !st.up || rang == 0 {
            unlock();
            return;
        }
        let Some(s) = st.services.as_ref() else {
            unlock();
            return;
        };
        if s.queued() == 0 {
            st.starved = st.starved.wrapping_add(1);
        }
        let mut out = [0u32; PERIOD];
        while s.queued() + PERIOD_BYTES <= LEAD as u32 * PERIOD_BYTES {
            let m = mix::period(&mut st.streams, &mut st.tone, &mut st.clip, &mut out);
            s.write(&out);
            st.periods += 1;
            st.underruns = st.underruns.wrapping_add(m.underruns);
            st.quiet = if m.sound || st.streams.iter().any(|x| x.live) { 0 } else { st.quiet + 1 };
        }
        for x in st.streams.iter_mut().filter(|x| x.live) {
            if let Some((token, room)) = x.release() {
                let mut r = [0u8; 5];
                r[1..5].copy_from_slice(&(room as u32).to_le_bytes());
                reply(&token, &r);
            }
        }
        if st.quiet >= TAIL {
            st.stopping = true;
            st.up = false;
            let s = st.services.take();
            unlock();
            if let Some(s) = s {
                down(&s);
            }
            let st = lock();
            st.stopping = false;
            unlock();
            end_if_idle();
            return;
        }
        unlock();
    }
    watch();
}

/// The amplifiers read once a period while sound plays (the AoC's interrupt is the clock, no timer), outside the lock: only this thread drives their port while `up` (`start` runs before it is set, `down` and `fault` after this thread clears it). Any error bit (AMP_SHORT, BST_SHORT, TEMP, BST_OVP, BST_DCM_UVP) stops everything at once (`fault`): there is no speaker protection in bypass (amp.rs), so a fault is never played thru.
fn watch() {
    match amp::Amps::new().status() {
        Ok(w) => {
            let st = lock();
            st.amps = w;
            if (w[0].0 | w[1].0) & ERRORS != 0 {
                fault(st);
                return;
            }
            unlock();
        }
        Err(e) => {
            let st = lock();
            st.watch_failed = st.watch_failed.wrapping_add(1);
            let first = st.watch_failed == 1;
            unlock();
            if first {
                say!("audio: the amplifiers' watch could not read them: {e}");
            }
        }
    }
}

/// An amplifier's error bit while sound plays: the tone and every stream stopped (a held write answered "unknown"), the source off, both amplifiers down, the speaker unbound. The process stays, silent, so the next call is told why (its report says `stopped by an error`), then ends (`exit_if_idle`); the switch puts both amplifiers in reset with it. Called with the lock held; returns with it released.
fn fault(st: &mut State) {
    st.fault = true;
    st.closing = true;
    st.up = false;
    st.tone = mix::Tone::NONE;
    st.clip = mix::Clip::NONE;
    for x in st.streams.iter_mut().filter(|x| x.live) {
        if x.held {
            reply(&x.token, &[1]);
            x.held = false;
        }
        x.live = false;
    }
    let w = st.amps;
    let s = st.services.take();
    unlock();
    for (k, name) in amp::NAMES.iter().enumerate() {
        say!("audio: {name} amplifier: IRQ1_STATUS1 G#{:X}, errors G#{:X} (AMP_SHORT G#80000000, TEMP G#20000, BST_SHORT G#100, BST_DCM_UVP G#80, BST_OVP G#40)", w[k].0, w[k].0 & ERRORS);
    }
    say!("audio: stopped: an amplifier reported an error");
    if let Some(s) = s {
        down(&s);
    }
}

/// The amplifiers' report (ferros_sys audio `REPORT`), after a status byte.
pub fn report(st: &State, status: u8) -> [u8; 1 + REPORT] {
    let mut r = [0u8; 1 + REPORT];
    r[0] = status;
    for (k, &(s1, s3)) in st.amps.iter().enumerate() {
        r[1 + 8 * k..5 + 8 * k].copy_from_slice(&s1.to_le_bytes());
        r[5 + 8 * k..9 + 8 * k].copy_from_slice(&s3.to_le_bytes());
    }
    r[REPORT] = st.fault as u8;
    r
}

static mut CLOCK_STACK: [u8; 1 << 14] = [0; 1 << 14];
static mut POLL_STACK: [u8; 1 << 13] = [0; 1 << 13];
static mut MSG: [u8; 4096] = [0; 4096];

/// Bring the sound up if it is not: true when playing. Called without the lock (the AoC and the amplifiers take tens of milliseconds); one start at a time (`starting`), from the receiving thread, or from the clock thread for a recording's playback (mic.rs `recorded`): a second caller meanwhile is refused, as one is while the sound is brought down.
pub fn ensure_up() -> bool {
    {
        let st = lock();
        if st.up {
            unlock();
            return true;
        }
        if st.closing || st.starting || st.stopping {
            unlock();
            return false;
        }
        st.starting = true;
        unlock();
    }
    let r = start();
    let st = lock();
    st.starting = false;
    let why = match r {
        Ok(s) => {
            st.services = Some(s);
            st.up = true;
            st.quiet = 0;
            None
        }
        Err(e) => Some(e),
    };
    unlock();
    if let Some(e) = why {
        say!("audio: could not start: {e}");
    }
    why.is_none()
}

/// The stream a handle names, if it is live.
fn find(st: &State, h: &[u8]) -> Option<usize> {
    st.streams.iter().position(|x| x.live && x.handle == h[..8])
}

#[unsafe(no_mangle)]
pub extern "C" fn audio_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    GRANTS.store(grants, Ordering::Relaxed);
    ferros_sys::thread_spawn(clock, unsafe { &mut *(&raw mut CLOCK_STACK) }, 0);
    ferros_sys::thread_spawn(mic::poller, unsafe { &mut *(&raw mut POLL_STACK) }, 0);
    let ep = cap(grants, S_AUDIO);
    let msg = unsafe { &mut *(&raw mut MSG) };
    let mut token = [0u8; 32];
    // From the second message on: a service with nothing to play (a STATE, a refused start, a stale handle) ends before it waits again, so an arc is held only while there is sound. The first is the call that started it.
    let mut served = false;
    loop {
        if served {
            exit_if_idle();
        }
        served = true;
        let n = recv(ep, msg, &mut token, false) as usize;
        if n as u64 == ERR || n == 0 {
            continue;
        }
        let m = &msg[..n];
        match m[0] {
            audio::OPEN if n == 2 => {
                if !ensure_up() {
                    reply(&token, &[1]);
                    continue;
                }
                let st = lock();
                let Some(k) = st.streams.iter().position(|x| !x.live) else {
                    unlock();
                    reply(&token, &[1]);
                    continue;
                };
                let mut h = [0u8; 8];
                h[0] = k as u8;
                st.generation[k] = st.generation[k].wrapping_add(1);
                h[1] = st.generation[k];
                entropy(cap(grants, S_ENTROPY), &mut h[2..]);
                st.streams[k].open(h, m[1]);
                unlock();
                let mut r = [0u8; 19];
                r[1..9].copy_from_slice(&h);
                r[9..13].copy_from_slice(&RATE.to_le_bytes());
                r[13..15].copy_from_slice(&(PERIOD as u16).to_le_bytes());
                r[15..19].copy_from_slice(&(RING as u32).to_le_bytes());
                reply(&token, &r);
            }
            // WHY the length checks here and below: a message is the caller's, any length up to 4096; a frame is four bytes, and a write past `WRITE_MAX` frames would not fit the held buffer.
            audio::WRITE if n >= 9 && (n - 9) & 3 == 0 && (n - 9) >> 2 <= WRITE_MAX => {
                let st = lock();
                let Some(k) = find(st, &m[1..9]).filter(|&k| !st.streams[k].held && !st.streams[k].closing) else {
                    unlock();
                    reply(&token, &[1]);
                    continue;
                };
                let s = &mut st.streams[k];
                if s.put(&m[9..]) {
                    let room = s.room() as u32;
                    unlock();
                    let mut r = [0u8; 5];
                    r[1..5].copy_from_slice(&room.to_le_bytes());
                    reply(&token, &r);
                } else {
                    // Held: answered by the clock thread when a period played makes room.
                    s.hold(token, &m[9..]);
                    unlock();
                }
            }
            audio::CLOSE if n == 9 => {
                let st = lock();
                if let Some(k) = find(st, &m[1..9]) {
                    st.streams[k].closing = true;
                }
                unlock();
                reply(&token, &[0]);
            }
            audio::GAIN if n == 11 => {
                let st = lock();
                let found = find(st, &m[1..9]).map(|k| st.streams[k].gain = u16::from_le_bytes([m[9], m[10]])).is_some();
                unlock();
                reply(&token, &[!found as u8]);
            }
            audio::STATE => {
                let st = lock();
                let mut r = [0u8; 24 + REPORT];
                r[0] = aoc::online() as u8;
                r[1] = st.up as u8;
                r[2] = st.streams.iter().filter(|x| x.live).count() as u8;
                r[4..12].copy_from_slice(&st.periods.to_le_bytes());
                r[12..16].copy_from_slice(&st.underruns.to_le_bytes());
                r[16..20].copy_from_slice(&st.irqs.to_le_bytes());
                r[20..24].copy_from_slice(&st.starved.to_le_bytes());
                r[24..].copy_from_slice(&report(st, 0)[1..]);
                let (periods, underruns, irqs, starved) = (st.periods, st.underruns, st.irqs, st.starved);
                let svc = st.services.as_ref().map(|s| (s.control.irq(), s.playback.irq(), s.queued()));
                unlock();
                say!("audio: AoC {}, playing {}, A#{} streams, A#{periods} periods, A#{underruns} underruns, A#{irqs} interrupts, A#{starved} starved", if r[0] == 1 { "up" } else { "down" }, r[1], r[2]);
                if let Some((c, p, q)) = svc {
                    say!("audio: control on channel A#{c}, playback on channel A#{p}, A#{q} bytes queued at the AoC");
                    // The AoC's own count of what entry point 5 has taken: moving means it plays, whatever the ears say.
                    if let Ok(s) = aoc::services() {
                        match aoc::samples(&s) {
                            Ok(n) => say!("audio: the AoC says entry point A#{} has taken A#{n} samples", aoc::EP),
                            Err(e) => say!("audio: GET_EP_TOT_SAMPLES: {e}"),
                        }
                    }
                }
                reply(&token, &r);
            }
            // The test tone (`TONE`): four bytes as before (both channels), or five with its channels.
            audio::TONE if n == 4 || n == 5 => {
                let level = m[1];
                let periods = u16::from_le_bytes([m[2], m[3]]) as u32;
                let mask = if n == 5 { m[4] } else { LEFT | RIGHT };
                // Stopped: faded over the next period, then the tail's silence and the end; nothing started for it.
                if periods == 0 {
                    let st = lock();
                    let playing = st.tone.left != 0 || st.clip.left != 0;
                    st.tone.stop();
                    st.clip.stop();
                    let r = report(st, 0);
                    unlock();
                    if playing {
                        say!("audio: tone stopped");
                    }
                    reply(&token, &r);
                    continue;
                }
                // WHY (AGENT.md rule 0): `TONE_MAX`'s, the speaker unprotected in bypass; level 0 stands for the device's default volume (gain_q15), two octaves down, far past the ceiling, so it is refused too. A channel mask must name a channel and nothing else.
                if level == 0 || level > TONE_MAX || mask == 0 || mask & !(LEFT | RIGHT) != 0 {
                    say!("audio: tone refused: level A#{level} (A#1 to A#{TONE_MAX}), channels G#{mask:X}");
                    let st = lock();
                    let r = report(st, 1);
                    unlock();
                    reply(&token, &r);
                    continue;
                }
                if !ensure_up() {
                    let st = lock();
                    let r = report(st, 1);
                    unlock();
                    reply(&token, &r);
                    continue;
                }
                let st = lock();
                st.tone.start(level, periods, mask);
                let r = report(st, 0);
                unlock();
                say!("audio: tone, 1 kHz at level A#{level} for A#{periods} periods, {}", match mask { LEFT => "left", RIGHT => "right", _ => "both" });
                reply(&token, &r);
            }
            audio::PROBE => {
                let st = lock();
                let busy = st.up || st.siggen || st.closing || st.starting || st.stopping;
                unlock();
                if busy {
                    say!("audio: probe refused: sound is playing");
                    reply(&token, &[1]);
                    continue;
                }
                match probe() {
                    Ok((ids, answered)) => {
                        let mut r = [0u8; 10];
                        r[1..5].copy_from_slice(&ids[0].to_le_bytes());
                        r[5..9].copy_from_slice(&ids[1].to_le_bytes());
                        r[9] = answered as u8;
                        reply(&token, &r);
                    }
                    Err(e) => {
                        say!("audio: probe stopped: {e}");
                        reply(&token, &[1]);
                    }
                }
            }
            audio::MIC_LEVELS => mic::levels(token),
            audio::RECORD if n == 3 => mic::record(token, m[1], m[2]),
            audio::SIGGEN if n == 8 => {
                let amplitude = u32::from_le_bytes([m[4], m[5], m[6], m[7]]);
                reply(&token, &[siggen(m[1], u16::from_le_bytes([m[2], m[3]]), amplitude) as u8 ^ 1]);
            }
            _ => {
                reply(&token, &[1]);
            }
        }
    }
}

/// Nothing playing (no streams, no generator) and nothing being brought up: the process ends, so the next call starts a fresh one (the kernel's on-demand start) rather than meeting a half-started one, and silence holds no arc.
fn exit_if_idle() {
    let st = lock();
    let idle = idle(st);
    unlock();
    if idle {
        ferros_sys::exit(1);
    }
}

/// The silent check (`PROBE`, docs/AUDIO.md "Verification" step 0): the AoC's services found and described, one control command sent and answered, both amplifiers from reset to ready (`Amps::up`: trims, port, PLL, AMP_EN; GLOBAL_EN never, so neither can sound, and the AoC's speaker port is never started). Their DEVIDs and whether the control queue answered; the switch puts both back in reset as the process ends after it (`exit_if_idle`).
fn probe() -> Result<([u32; 2], bool), &'static str> {
    if !aoc::online() {
        return Err("the AoC is not up (the sensors process asks for its start at every boot)");
    }
    for name in [&b"audio_output_control"[..], b"audio_playback5", b"audio_input_control", b"audio_capture0"] {
        aoc::dump(name);
    }
    // The rings are checked and said, and the probe goes on past a refusal: the control queue and the amplifiers are worth knowing about whatever the rings' places.
    match aoc::services() {
        Ok(s) => say!("audio: the playback ring A#{} bytes, A#{} queued", s.ring_bytes(), s.queued()),
        Err(e) => say!("audio: {e}"),
    }
    if let Err(e) = aoc::capture_services() {
        say!("audio: {e}");
    }
    // A get, changing nothing: the control queue's round trip, and the channel its answer comes on.
    let answered = match aoc::control_samples() {
        Ok(n) => {
            say!("audio: control answered: entry point A#{} has taken A#{n} samples", aoc::EP);
            true
        }
        Err(e) => {
            say!("audio: control: {e}");
            e != aoc::NO_ANSWER
        }
    };
    let amps = amp::Amps::new();
    amps.up()?;
    let w = amps.status()?;
    for (k, name) in amp::NAMES.iter().enumerate() {
        say!("audio: {name} amplifier ready, off: IRQ1_STATUS1 G#{:X} (errors G#{:X}), IRQ1_STATUS3 G#{:X}", w[k].0, w[k].0 & ERRORS, w[k].1);
    }
    say!("audio: probe done, nothing sounded (GLOBAL_EN never set); both amplifiers back in reset as this process ends");
    Ok((amps.devids(), answered))
}

/// The AoC's own generator on entry point 5 (AUDIO.md "The AoC side, shortest": CFG_SIGGEN, EP_SETUP in mode 5, BIND2, SOURCE), for the bring-up: no PCM, so it says whether the AoC to amplifier path works before the ring does. Amplitude 0 stops it and ends the process. Refused while streams play.
fn siggen(wave: u8, period: u16, amplitude: u32) -> bool {
    {
        let st = lock();
        let busy = st.up;
        unlock();
        if busy {
            say!("audio: siggen refused: streams are playing");
            return false;
        }
    }
    let s = match aoc::services() {
        Ok(s) => s,
        Err(e) => {
            say!("audio: siggen: {e}");
            return false;
        }
    };
    if amplitude == 0 {
        down(&s);
        ferros_sys::exit(0);
    }
    let amps = amp::Amps::new();
    let r = amps
        .up()
        .and_then(|_| aoc::siggen(&s, wave, period, amplitude))
        .and_then(|_| aoc::setup(&s, aoc::MODE_SIGGEN, 0, 0))
        .and_then(|_| aoc::bind(&s, aoc::EP, aoc::SINK_SPEAKER, true))
        .and_then(|_| amps.on().and_then(|w| if (w[0].0 | w[1].0) & ERRORS == 0 { Ok(()) } else { Err("an amplifier reports an error") }))
        .and_then(|_| aoc::source(&s, aoc::EP, true));
    match r {
        Ok(()) => {
            say!("audio: siggen: wave A#{wave}, period A#{period}, amplitude A#{amplitude}");
            let st = lock();
            st.siggen = true;
            unlock();
            true
        }
        Err(e) => {
            say!("audio: siggen: {e}");
            false
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

ferros_sys::entry!(audio_main);
