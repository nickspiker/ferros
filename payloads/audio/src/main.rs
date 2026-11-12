//! The audio service (docs/AUDIO.md): sound out of the speakers for every kist that holds the `audio` right. It owns the AoC's playback entry point 5 and the two CS35L41 amplifiers, mixes every open stream into one, and is clocked by the AoC: each period the AoC takes from its ring, it rings the AP on mailbox channel 20 (aoc_alsa.h:241 `PCM_CHANNEL`; husky's dmesg:7430 "audio_playback5 supports interrupt-driven"), and that interrupt is when this service mixes the next period and answers the writes that now fit. No timer anywhere on the sound's path.
//!
//! The kernel starts it at the first call on `audio` (process.rs `ON_DEMAND`, service.rs `audio_grants`), and it ends itself once every stream has closed and played out: silence holds no arc, and the amplifiers go back to reset (the `amp` switch is off when the process that held it ends). The next call starts another.
//!
//! Spawn by hand (development; the kernel's own start is the same grants): ferros-bridge spawn audio.bin --name audio --serve audio --dma aoc-ipc --hw aoc-mbox --irq aoc-mbox-f1 --hw usi1-spi --hw gpio-gpn0 --power amp --entropy
//! Grant slots, in that order: 0 console, 1 the `audio` endpoint (serve), 2 the AoC's IPC block, 3 its mailboxes, 4 mailbox block f1's AoC-to-AP line, 5 the amplifiers' SPI port, 6 gpio-gpn0 (the right amplifier's select), 7 the `amp` switch, 8 entropy (the streams' handles).
//!
//! The endpoint's protocol is ferros_sys `audio` (its ops, the format, the ring). The AoC must be up (the sensors process asks the kernel to start it at every boot); without it an open is refused.
//!
//! Two threads. This one receives and answers; the clock thread waits on the AoC's interrupt, mixes, writes the AoC's ring and answers held writes. Both work on one state under one lock (a futex word: `lock`), held for a copy or a period's mix, never across a call to the AoC.
#![no_std]
#![no_main]

mod amp;
mod aoc;
mod mix;

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::audio::{self, PERIOD, RATE, RING, WRITE_MAX};
use ferros_sys::{cap, entropy, irq_ack, irq_wait, recv, reply, ERR};

pub const S_AUDIO: u64 = 1;
pub const S_IPC: u64 = 2;
pub const S_MBOX: u64 = 3;
pub const S_IRQ: u64 = 4;
pub const S_SPI: u64 = 5;
pub const S_GPN: u64 = 6;
pub const S_POWER: u64 = 7;
pub const S_ENTROPY: u64 = 8;

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

struct State {
    /// The AoC playing entry point 5 and the amplifiers on.
    up: bool,
    /// The AoC's own generator playing (`SIGGEN`): the process stays until it is stopped.
    siggen: bool,
    /// Bringing them down: opens are refused from here, the process ends.
    closing: bool,
    services: Option<aoc::Services>,
    streams: [mix::Stream; mix::STREAMS],
    tone: mix::Tone,
    /// A handle's second byte: a stream slot's generation, so a freed slot's old handle stays unknown.
    generation: [u8; mix::STREAMS],
    periods: u64,
    underruns: u32,
    /// The AoC found its ring empty at an interrupt: this service late.
    starved: u32,
    irqs: u32,
    quiet: u32,
}

static mut STATE: State = State { up: false, siggen: false, closing: false, services: None, streams: [const { mix::Stream::EMPTY }; mix::STREAMS], tone: mix::Tone::NONE, generation: [0; mix::STREAMS], periods: 0, underruns: 0, starved: 0, irqs: 0, quiet: 0 };

/// 0 free, 1 held, 2 held with a waiter.
static LOCK: AtomicU64 = AtomicU64::new(0);
fn lock() -> &'static mut State {
    if LOCK.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
        while LOCK.swap(2, Ordering::Acquire) != 0 {
            ferros_sys::wait(&LOCK, 2, 0);
        }
    }
    unsafe { &mut *(&raw mut STATE) }
}
fn unlock() {
    if LOCK.swap(0, Ordering::Release) == 2 {
        ferros_sys::wake(&LOCK, 1);
    }
}

/// Everything from silence to playing: the AoC's services found, the amplifiers up, entry point 5 bound to the speaker and set up (the order Linux uses on husky, dmesg:7426-7437: bind, then the stream's prepare, then the amplifiers' power, then the source on at the trigger: hw.c:1322, pcm.c:533-622, cs35l41.c:2264-2299, hw.c:2685-2705), `LEAD` periods of silence queued, the f1 line listening.
fn start() -> Result<aoc::Services, &'static str> {
    if !aoc::online() {
        return Err("the AoC is not up (the sensors process asks for its start at every boot)");
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
    amps.on()?;
    aoc::listen(1 << (ch - 16));
    aoc::source(&s, aoc::EP, true)?;
    say!("audio: playing: entry point A#{} to the speaker, A#{} periods of A#{} frames ahead, the AoC's ring A#{} bytes", aoc::EP, LEAD, PERIOD, s.ring_bytes());
    Ok(s)
}

/// Back to silence and out: the source off (hw.c:2711-2729), the amplifiers down, the speaker unbound (hw.c:1662-1687), and the process ended (its `amp` switch off with it: both amplifiers in reset).
fn stop(s: &aoc::Services) -> ! {
    let _ = aoc::source(s, aoc::EP, false);
    amp::Amps::new().off();
    let _ = aoc::bind(s, aoc::EP, aoc::SINK_SPEAKER, false);
    say!("audio: silent, ending");
    ferros_sys::exit(0)
}

/// The clock: the AoC rang (a period taken), so the ring is topped up to `LEAD` periods, each a period mixed, and held writes that now fit are answered.
extern "C" fn clock(_: u64) -> ! {
    let irq = cap(grants(), S_IRQ);
    loop {
        irq_wait(irq, 0);
        let rang = aoc::take();
        irq_ack(irq);
        ferros_sys::pet();
        let st = lock();
        st.irqs = st.irqs.wrapping_add(1);
        if !st.up || rang == 0 {
            unlock();
            continue;
        }
        let Some(s) = st.services.as_ref() else {
            unlock();
            continue;
        };
        if s.queued() == 0 {
            st.starved = st.starved.wrapping_add(1);
        }
        let mut out = [0u32; PERIOD];
        while s.queued() + PERIOD_BYTES <= LEAD as u32 * PERIOD_BYTES {
            let m = mix::period(&mut st.streams, &mut st.tone, &mut out);
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
            st.closing = true;
            st.up = false;
            let s = st.services.take();
            unlock();
            if let Some(s) = s {
                stop(&s);
            }
            ferros_sys::exit(0);
        }
        unlock();
    }
}

static mut CLOCK_STACK: [u8; 1 << 14] = [0; 1 << 14];
static mut MSG: [u8; 4096] = [0; 4096];

/// Bring the sound up if it is not: true when playing. Called without the lock (the AoC and the amplifiers take tens of milliseconds); the receiving thread is the only one that starts it.
fn ensure_up() -> bool {
    {
        let st = lock();
        let (up, closing) = (st.up, st.closing);
        unlock();
        if closing {
            return false;
        }
        if up {
            return true;
        }
    }
    match start() {
        Ok(s) => {
            let st = lock();
            st.services = Some(s);
            st.up = true;
            st.quiet = 0;
            unlock();
            true
        }
        Err(e) => {
            say!("audio: could not start: {e}");
            false
        }
    }
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
                let mut r = [0u8; 24];
                r[0] = aoc::online() as u8;
                r[1] = st.up as u8;
                r[2] = st.streams.iter().filter(|x| x.live).count() as u8;
                r[4..12].copy_from_slice(&st.periods.to_le_bytes());
                r[12..16].copy_from_slice(&st.underruns.to_le_bytes());
                r[16..20].copy_from_slice(&st.irqs.to_le_bytes());
                r[20..24].copy_from_slice(&st.starved.to_le_bytes());
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
            audio::TONE if n == 4 => {
                if !ensure_up() {
                    reply(&token, &[1]);
                    continue;
                }
                let st = lock();
                st.tone.start(m[1], u16::from_le_bytes([m[2], m[3]]) as u32);
                unlock();
                say!("audio: tone, 1 kHz at level A#{} for A#{} periods", m[1], u16::from_le_bytes([m[2], m[3]]));
                reply(&token, &[0]);
            }
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
    let idle = !st.up && !st.siggen && st.streams.iter().all(|x| !x.live);
    unlock();
    if idle {
        ferros_sys::exit(1);
    }
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
        stop(&s);
    }
    let amps = amp::Amps::new();
    let r = amps
        .up()
        .and_then(|_| aoc::siggen(&s, wave, period, amplitude))
        .and_then(|_| aoc::setup(&s, aoc::MODE_SIGGEN, 0, 0))
        .and_then(|_| aoc::bind(&s, aoc::EP, aoc::SINK_SPEAKER, true))
        .and_then(|_| amps.on())
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
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

ferros_sys::entry!(audio_main);
