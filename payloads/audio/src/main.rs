//! The audio service (docs/AUDIO.md): sound out of the speakers for every kist that holds the `audio` right. It owns the AoC's playback entry point 5 and the two CS35L41 amplifiers, mixes every open stream into one, and is clocked by the AoC: each period the AoC takes from its ring, it rings the AP on mailbox channel 20 (aoc_alsa.h:241 `PCM_CHANNEL`; husky's dmesg:7430 "audio_playback5 supports interrupt-driven"), and that interrupt is when this service mixes the next period and answers the writes that now fit. No timer anywhere on the sound's path.
//!
//! The kernel starts it at the first call on `audio` (process.rs `ON_DEMAND`, service.rs `audio_grants`), and it ends itself once every stream has closed and played out: silence holds no arc, and the amplifiers go back to reset (the `amp` switch is off when the process that held it ends). The next call starts another.
//!
//! Spawn by hand (development; the kernel's own start is the same grants): ferros-bridge spawn audio.bin --name audio --serve audio --dma aoc-ipc --hw aoc-mbox --irq aoc-mbox-f1 --hw usi1-spi --hw gpio-gpn0 --power amp --entropy --dma aoc-play-lo --dma aoc-capture --dma aoc-play-hi --dma aoc-heaps --serve mic --dma mic-mailbox
//! Grant slots, in that order: 0 console, 1 the `audio` endpoint (serve), 2 the AoC's IPC block, 3 its mailboxes, 4 mailbox block f1's AoC-to-AP line, 5 the amplifiers' SPI port, 6 gpio-gpn0 (the right amplifier's select), 7 the `amp` switch, 8 entropy (the streams' handles), 9 and 11 the playback ring's windows (`aoc-play-lo`, `aoc-play-hi`), 10 the capture ring's (`aoc-capture`), 12 the AAudio heaps (`aoc-heaps`), 13 the `mic` endpoint (serve: the capture ops, behind the mic right), 14 the microphone mailbox's writer region (`mic-mailbox`); a sweep's hand spawn adds 15, `--dma test-mailbox`.
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
/// The playback and capture rings' windows past `aoc-ipc` (kernel cachemap.rs AOC_PLAY_LO, AOC_CAPTURE, AOC_PLAY_HI; since 2026-11-22): the playback ring straddles G#9600_0000 and takes two.
pub const S_PLAY_LO: u64 = 9;
pub const S_CAPTURE: u64 = 10;
pub const S_PLAY_HI: u64 = 11;
/// The AoC's AAudio heaps (kernel cachemap.rs AOC_HEAPS, since 2026-11-24): the MMAP streams' buffers.
pub const S_HEAPS: u64 = 12;
/// The `mic` endpoint (serve, 2026-11-26): the microphones behind a right of their own (ferros_kist `mic`), its calls the only way to the capture ops; `audio`, every kist's, refuses them.
pub const S_MIC: u64 = 13;
/// The microphone mailbox's writer region (`mic-mailbox`, ferros_mailbox `MIC`), for the capture's publishing to the holders of its view.
pub const S_MIC_BOX: u64 = 14;
/// A hand spawn's dump for the bench's sweep (`--dma test-mailbox`, after `--serve mic --dma mic-mailbox`; slot 13 until 2026-11-26): the capture and its anchors, read by the host from the frame`diag arcs` names (development only; the kernel's own start grants nothing here).
pub const S_DUMP: u64 = 15;

/// The spawn input (a hand spawn's `--input-file`s, each a u32 length and its bytes: the bench's mono feed as i16, then the speaker's tuning, then its second part), its address and length in bytes; 0 for none.
static INPUT: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// The spawn input's file `k` (0 the feed, 1 and 2 the tuning's two parts): its bytes, None when absent.
pub fn input_file(k: usize) -> Option<&'static [u8]> {
    let (mut at, len) = (INPUT[0].load(Ordering::Relaxed), INPUT[1].load(Ordering::Relaxed));
    let end = at + len;
    for j in 0..=k {
        // WHY each check: the framing is the bridge's, in the spawn input's bytes; a short or torn input answers None.
        if at + 4 > end {
            return None;
        }
        // SAFETY: inside the spawn input, in this process's arc, by the check above.
        let n = (unsafe { core::ptr::read_unaligned(at as *const u32) }) as u64;
        if at + 4 + n > end {
            return None;
        }
        if j == k {
            // SAFETY: as above, `n` bytes inside the input.
            return Some(unsafe { core::slice::from_raw_parts((at + 4) as *const u8, n as usize) });
        }
        at += 4 + n;
    }
    None
}

static GRANTS: AtomicU64 = AtomicU64::new(0);
/// The cores the clock thread is pinned to, as `ferros_sys::pin` takes them (0: as spawned); set by a bench, taken by the clock thread at its next wake (a pin applies to the calling thread).
static PIN: AtomicU64 = AtomicU64::new(0);
/// The watch thread's cores, as `PIN` (a bench's byte 15): pinned to an idle core its multi-millisecond reads went (AUDIO.md "The amplifiers' watch, measured").
static WATCH_PIN: AtomicU64 = AtomicU64::new(0);
/// The clock thread's longest turn and its parts, in counter ticks (the microphones' turn, the playback's mix and writes, the amplifiers' watch): what keeps the thread from its next ring (measure before fixing: the clicks of 2026-11-24).
static TURN_MAX: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
fn turn_part(k: usize, t: u64) {
    TURN_MAX[k].fetch_max(t, Ordering::Relaxed);
}
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

/// How the sound is played: periods kept queued in the AoC's ring ahead of what it plays (`lead`), periods the AoC is told its buffer holds (`buf`, EP_SETUP2), and the period in frames (`per`, a power of two up to `PERIOD`).
/// The default is the shape built on 2026-11-11: two periods of 256 frames queued, 10.7 ms; with a stream's ring (`RING`, 21.3 ms at most) that is the latency's budget under the 40 ms a game wants (AUDIO.md "Latency"). The bench (`BENCH`, 2026-11-24) asks for others, to find the AoC's floor.
#[derive(Clone, Copy, PartialEq)]
pub struct Play {
    pub lead: u32,
    pub buf: u32,
    pub per: usize,
    /// The AoC's entry point the ring feeds (aoc.rs `ring_name`).
    pub ep: u8,
}

impl Play {
    pub const DEFAULT: Play = Play { lead: 2, buf: 2, per: PERIOD, ep: aoc::EP };
    /// The MMAP entry point: written ahead of the AoC's read place in a ring kept full (aoc.rs "The MMAP entry point").
    pub fn mmap(self) -> bool {
        self.ep == 0
    }
    /// Bytes a frame in the AoC's ring: two 32-bit floats for the MMAP entry point, two 16-bit samples otherwise.
    pub fn frame_bytes(self) -> usize {
        if self.mmap() { 8 } else { audio::FRAME_BYTES }
    }
    /// Bytes a period, as the AoC's ring counts.
    pub fn period_bytes(self) -> u32 {
        (self.per * self.frame_bytes()) as u32
    }
}
/// Periods of silence played after the last stream closed, before the source goes off and the amplifiers down: the ring and the AoC's pipeline emptied, the end of the sound not cut off.
const TAIL: u32 = 1 << 4;

pub struct State {
    /// The AoC playing entry point 5 and the amplifiers on.
    up: bool,
    /// The AoC's own generator playing (`SIGGEN`): the process stays until it is stopped.
    siggen: bool,
    /// The MMAP entry point's writer: the running count it has written up to, and the AoC's rx at the last turn (what it played since is zeroed).
    cursor: u32,
    last_rx: u32,
    /// A sweep's dump waits to be read: the process stays (a mailbox's payload is zeroed by the kernel when its writer ends) until the next call, which the host sends once it has read the frame.
    dump_held: bool,
    /// Calls taken off an endpoint and not yet acted on (the `audio` and `mic` threads, 2026-11-26): while one is, the process is not idle, so neither thread ends it under the other's call.
    receiving: u8,
    /// Ending after an amplifier's fault: opens are refused from here, the process ends once the next call has been told.
    closing: bool,
    /// The sound being brought up (`ensure_up`) or down after its tail (`clock`): a second start waits for neither, it is refused.
    starting: bool,
    stopping: bool,
    services: Option<aoc::Services>,
    /// The shape the sound was brought up in (`Play`).
    play: Play,
    streams: [mix::Stream; mix::STREAMS],
    tone: mix::Tone,
    /// The bench's click train, its feed, and its measures.
    clicks: mix::Clicks,
    feed: mix::Feed,
    pub bench: Bench,
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
    /// Both amplifiers' (IRQ1_STATUS1, IRQ1_STATUS3) as last read: at GLOBAL_EN, then every `WATCH_FRAMES` written while sound plays (`watch`).
    amps: [(u32, u32); 2],
    /// An amplifier's error bit stopped everything (`fault`): the process stays, silent, until the next call has been told, then ends.
    fault: bool,
    /// The watch's reads failing (said once, at the first).
    watch_failed: u32,
}

static mut STATE: State = State { cursor: 0, last_rx: 0, dump_held: false, receiving: 0, up: false, siggen: false, closing: false, starting: false, stopping: false, services: None, play: Play::DEFAULT, clicks: mix::Clicks::NONE, feed: mix::Feed::NONE, bench: Bench::NONE, streams: [const { mix::Stream::EMPTY }; mix::STREAMS], tone: mix::Tone::NONE, clip: mix::Clip::NONE, mic: mic::Mic::NONE, generation: [0; mix::STREAMS], periods: 0, underruns: 0, starved: 0, irqs: 0, quiet: 0, amps: [(0, 0); 2], fault: false, watch_failed: 0 };

/// The amplifiers' port (spi_19): since the watch has a thread of its own (2026-11-24), it and the clock thread's bring-up and power-down both drive the port, one at a time under this lock (0 free, 1 held, 2 held with a waiter).
static AMP_LOCK: AtomicU64 = AtomicU64::new(0);
pub fn amp_lock() {
    if AMP_LOCK.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
        while AMP_LOCK.swap(2, Ordering::Acquire) != 0 {
            ferros_sys::wait(&AMP_LOCK, 2, 0);
        }
    }
}
pub fn amp_unlock() {
    if AMP_LOCK.swap(0, Ordering::Release) == 2 {
        ferros_sys::wake(&AMP_LOCK, 1);
    }
}

/// The watch's doorbell (the clock thread counts it up every `WATCH_FRAMES` written while sound plays) and the counter's time of its last completed read.
static WATCH_BELL: AtomicU64 = AtomicU64::new(0);
static WATCH_DONE_AT: AtomicU64 = AtomicU64::new(0);
/// WHY a bound on the watch's age (the coordinator for the owner, 2026-11-24: "treat a stale watch as unknown, not as safe"): the watch is the only protection the unprotected speaker has (amp.rs: bypass, no protection firmware), so a watch that has not completed a read for 2^11 frames (42.7 ms at 48 kHz) means the amplifiers' state is unknown, and the sound stops as it does on an error bit. 2^11 frames is about nine times the slowest read measured on the Pro (4.5 ms, the SPI stall that made the clicks) and about four of the deep-buffer path's 10 ms takes, so a healthy watch never trips it.
const WATCH_STALE_FRAMES: u64 = 1 << 11;

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

/// Everything from silence to playing: the AoC's services found, the amplifiers up, entry point 5 bound to the speaker and set up (the order Linux uses on husky, dmesg:7426-7437: bind, then the stream's prepare, then the amplifiers' power, then the source on at the trigger: hw.c:1322, pcm.c:533-622, cs35l41.c:2264-2299, hw.c:2685-2705), the lead's periods of silence queued, the f1 line listening.
fn start(p: Play) -> Result<aoc::Services, &'static str> {
    if !aoc::online() {
        return Err("the AoC is not up (the sensors process asks for its start at every boot)");
    }
    let ring = aoc::ring_name(p.ep).ok_or("no playback ring is known for that entry point")?;
    for name in [&b"audio_output_control"[..], ring] {
        aoc::dump(name);
    }
    let mut s = aoc::services_for(p.ep)?;
    aoc::describe("audio_output_control", s.control);
    aoc::describe(core::str::from_utf8(ring).unwrap_or("?"), s.playback);
    // The playback channel must be on f1, the block whose line this process holds.
    let ch = s.playback.irq();
    if !(16..32).contains(&ch) {
        return Err("the playback ring's channel is not on mailbox block f1");
    }
    // WHY: the periods ahead and the buffer the AoC is told of must fit in the AoC's ring, whose size is the AoC's.
    if s.ring_bytes() < p.lead * p.period_bytes() || s.ring_bytes() < p.buf * p.period_bytes() {
        return Err("the playback ring is shorter than the lead");
    }
    let amps = amp::Amps::new();
    amps.up()?;
    aoc::bind(&s, s.ep, aoc::SINK_SPEAKER, true)?;
    s.reset_write();
    if p.mmap() {
        // The playback heap mapped into the AoC first, as Android's HAL does at an MMAP stream's open (aoc.rs "The AAudio heaps"): the samples go there.
        if aoc::heaps() {
            aoc::map_heap(aoc::MAP_PLAYBACK, aoc::HEAP_PLAYBACK, true)?;
            s.heap = true;
            say!("audio: the playback heap mapped into the AoC: A#{} bytes at G#{:X}", aoc::HEAP_PLAYBACK.1, aoc::HEAP_PLAYBACK.0);
        }
        // The speaker's tuning first, as Android's HAL sends it before an MMAP stream's setup (aoc.rs `tune`), from the hand spawn's input for now.
        if let Some(t) = input_file(1) {
            aoc::tune(t)?;
            say!("audio: the speaker's tuning sent: A#{} bytes", t.len());
        }
        // The MMAP way: the buffer is the whole ring (ALSA's buffer is the ring the app maps, aoc_alsa_pcm.c:733-770), silent, marked full; the writer keeps the lead ahead of the AoC's read place (`play_turn`).
        // The period as Android's HAL gives it for this entry point: 1 ms of float frames, 384 bytes (measured 2026-11-24), whatever the service's own mixing period.
        aoc::setup_as(&s, aoc::MODE_PLAYBACK, s.ring_bytes(), (48 * p.frame_bytes()) as u32, 3, 0)?;
        s.zero_all();
        s.mark_full();
        // Android's second, shorter tuning came after the setup.
        if let Some(t) = input_file(2) {
            aoc::tune(t)?;
            say!("audio: the speaker's second tuning sent: A#{} bytes", t.len());
        }
    } else {
        aoc::setup(&s, aoc::MODE_PLAYBACK, p.buf * p.period_bytes(), p.period_bytes())?;
        let silence = [0u32; PERIOD];
        for _ in 0..p.lead {
            s.write(&silence[..p.per]);
        }
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
    aoc::source(&s, s.ep, true)?;
    say!("audio: playing: entry point A#{} to the speaker, A#{} periods of A#{} frames ahead, the AoC told of A#{} periods, its ring A#{} bytes", s.ep, p.lead, p.per, p.buf, s.ring_bytes());
    Ok(s)
}

/// Back to silence: the source off (hw.c:2711-2729), the amplifiers down, the speaker unbound (hw.c:1662-1687), the playback's channel no longer listened to. The process ends after it unless the microphones still run (`end_if_idle`); its `amp` switch goes off with it, both amplifiers in reset.
fn down(s: &aoc::Services) {
    aoc::listen(0, 0xFFFF, false);
    let _ = aoc::source(s, s.ep, false);
    amp_lock();
    amp::Amps::new().off();
    amp_unlock();
    let _ = aoc::bind(s, s.ep, aoc::SINK_SPEAKER, false);
    if s.heap {
        let _ = aoc::map_heap(aoc::MAP_PLAYBACK, aoc::HEAP_PLAYBACK, false);
    }
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
    st.receiving == 0 && !st.dump_held && !st.bench.on && !st.bench.holding && !st.up && !st.siggen && !st.starting && !st.stopping && !st.mic.busy() && st.streams.iter().all(|x| !x.live)
}

/// The clock: the AoC rang (a period taken, or the microphones' ring written), so the microphones' samples are taken (mic.rs `turn`), the playback ring topped up to the lead's periods (`Play`), each a period mixed, and held writes that now fit answered.
extern "C" fn clock(_: u64) -> ! {
    let name = b"clock";
    ferros_sys::svc(ferros_sys::nr::THREAD_NAME, ferros_sys::off(name.as_ptr()), name.len() as u64, 0, 0, 0);
    let irq = cap(grants(), S_IRQ);
    let mut pinned = 0;
    loop {
        irq_wait(irq, 0);
        let want = PIN.load(Ordering::Relaxed);
        if want != pinned {
            ferros_sys::pin(want);
            pinned = want;
        }
        let rang = aoc::take();
        irq_ack(irq);
        ferros_sys::pet();
        let t0 = ferros_sys::now();
        mic::turn(rang);
        let t1 = ferros_sys::now();
        play_turn(rang);
        let t2 = ferros_sys::now();
        turn_part(0, t2 - t0);
        turn_part(1, t1 - t0);
        turn_part(2, t2 - t1);
    }
}

/// The playback's turn at an interrupt.
fn play_turn(rang: u16) {
    let mut ended: Option<(Bench, [u8; 32], aoc::Services)> = None;
    {
        let st = lock();
        st.irqs = st.irqs.wrapping_add((rang != 0) as u32);
        if !st.up || rang == 0 {
            unlock();
            return;
        }
        let Some(s) = st.services else {
            unlock();
            return;
        };
        if watch_stale(st) {
            return;
        }
        let p = st.play;
        let pb = p.period_bytes();
        if p.mmap() {
            mmap_turn(st, &s, p);
            if st.quiet as usize * p.per >= (TAIL as usize) << 8 && !st.bench.on {
                stop_after_tail(st);
                return;
            }
            unlock();
            ring_watch();
            return;
        }
        let q = s.queued();
        if q == 0 {
            st.starved = st.starved.wrapping_add(1);
        }
        st.bench.interrupt(ferros_sys::now(), q, s.taken());
        let mut out = [0u32; PERIOD];
        // Topped up to the lead in frames, a period at most at a time: the AoC takes what it takes (on entry point 5, up to 10 ms between two rings, whatever period it was told), and a top-up in whole periods left it short (2026-11-24: 5.84 s taken in 7.29 s with two periods of 256 frames).
        let target = p.lead * pb;
        loop {
            let ahead = s.queued();
            if ahead >= target {
                break;
            }
            // The frames this turn: the room to the lead or a period, the smaller of two counts.
            let room = ((target - ahead) as usize) >> 2;
            let o = &mut out[..if room < p.per { room } else { p.per }];
            let m = mix::period(&mut st.streams, &mut st.tone, &mut st.clip, &mut st.clicks, &mut st.feed, o, p.per);
            s.write(o);
            WATCH_DUE.fetch_add(o.len() as u64, Ordering::Relaxed);
            st.periods += 1;
            st.underruns = st.underruns.wrapping_add(m.underruns);
            st.quiet = if m.sound || st.streams.iter().any(|x| x.live) { 0 } else { st.quiet + 1 };
            st.bench.period(&st.clicks, &st.tone, &st.feed, m.click.map(|k| (ferros_sys::now(), ahead + (k * audio::FRAME_BYTES) as u32)), o.len());
        }
        for x in st.streams.iter_mut().filter(|x| x.live) {
            if let Some((token, room)) = x.release() {
                let mut r = [0u8; 5];
                r[1..5].copy_from_slice(&(room as u32).to_le_bytes());
                reply(&token, &r);
            }
        }
        if st.bench.on && st.bench.done {
            st.bench.on = false;
            st.dump_held = st.bench.dump != 0;
            let mut b = st.bench;
            b.mic = (st.mic.reads, st.mic.read_min, st.mic.read_max, st.mic.skipped, st.mic.total);
            b.counts = (st.periods, st.underruns, st.starved);
            ended = Some((b, st.bench.token, s));
        }
        // The tail in frames (`TAIL` periods of the default's 256), so a short period does not cut the sound's end short.
        if st.quiet as usize * p.per >= (TAIL as usize) << 8 && !st.bench.on && ended.is_none() {
            stop_after_tail(st);
            return;
        }
        unlock();
    }
    if let Some((b, token, s)) = ended {
        b.report(&token, &s);
    }
    ring_watch();
}

/// The tail played: the sound brought down and the process ended if nothing else holds it. Called with the lock held; returns with it released.
fn stop_after_tail(st: &mut State) {
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
}

/// The MMAP entry point's turn (the lock held): the frames the AoC played since the last turn zeroed, the mix written from the writer's count up to the lead past the AoC's rx (a writer found behind rx starts at rx, an underrun), the ring marked full again. The bench's measures as the plain ring's, with "queued" the frames written ahead of the AoC.
fn mmap_turn(st: &mut State, s: &aoc::Services, p: Play) {
    let pb = p.period_bytes();
    let rx = s.taken();
    if st.periods == 0 {
        st.cursor = rx;
        st.last_rx = rx;
    }
    // Signed distances between running counts: they wrap together at 2^32.
    let ahead = st.cursor.wrapping_sub(rx) as i32;
    if ahead <= 0 {
        st.starved = st.starved.wrapping_add(1);
        st.cursor = rx;
    }
    st.bench.interrupt(ferros_sys::now(), st.cursor.wrapping_sub(rx), rx);
    s.zero_at(st.last_rx, rx.wrapping_sub(st.last_rx));
    st.last_rx = rx;
    let target = rx.wrapping_add(p.lead * pb);
    let mut out = [0u32; PERIOD];
    while (target.wrapping_sub(st.cursor) as i32) > 0 {
        // The frames this turn: the room to the lead or a period, the smaller of two counts.
        let room = target.wrapping_sub(st.cursor) as usize / p.frame_bytes();
        let o = &mut out[..if room < p.per { room } else { p.per }];
        let ahead = st.cursor.wrapping_sub(rx);
        let m = mix::period(&mut st.streams, &mut st.tone, &mut st.clip, &mut st.clicks, &mut st.feed, o, p.per);
        s.put_at_f32(st.cursor, o);
        st.cursor = st.cursor.wrapping_add((o.len() * p.frame_bytes()) as u32);
        WATCH_DUE.fetch_add(o.len() as u64, Ordering::Relaxed);
        st.periods += 1;
        st.underruns = st.underruns.wrapping_add(m.underruns);
        st.quiet = if m.sound || st.streams.iter().any(|x| x.live) { 0 } else { st.quiet + 1 };
        // The bytes ahead as frames of the plain ring's four bytes, so the bench's report reads alike for both.
        st.bench.period(&st.clicks, &st.tone, &st.feed, m.click.map(|k| (ferros_sys::now(), (ahead / p.frame_bytes() as u32 + k as u32) * audio::FRAME_BYTES as u32)), o.len());
    }
    s.mark_full();
    for x in st.streams.iter_mut().filter(|x| x.live) {
        if let Some((token, room)) = x.release() {
            let mut r = [0u8; 5];
            r[1..5].copy_from_slice(&(room as u32).to_le_bytes());
            reply(&token, &r);
        }
    }
    if st.bench.on && st.bench.done {
        st.bench.on = false;
        st.dump_held = st.bench.dump != 0;
        let mut b = st.bench;
        b.mic = (st.mic.reads, st.mic.read_min, st.mic.read_max, st.mic.skipped, st.mic.total);
        b.counts = (st.periods, st.underruns, st.starved);
        let token = st.bench.token;
        let s2 = *s;
        unlock();
        b.report(&token, &s2);
        let _ = lock();
    }
}

/// The amplifiers read every `WATCH_FRAMES` written while sound plays (the AoC's interrupt is the clock, no timer), outside the state's lock, under the port's (`amp_lock`): the bring-up and the power-down hold it for their whole sequence. Any error bit (AMP_SHORT, BST_SHORT, TEMP, BST_OVP, BST_DCM_UVP) stops everything at once (`fault`): there is no speaker protection in bypass (amp.rs), so a fault is never played thru.
fn watch() {
    let t = ferros_sys::now();
    amp_lock();
    // Only while sound plays: the bring-up and the power-down hold the port's lock for their whole sequence, and a read after `up` cleared is not wanted.
    let up = {
        let st = lock();
        let up = st.up;
        unlock();
        up
    };
    if !up {
        amp_unlock();
        return;
    }
    let r = amp::Amps::new().status();
    amp_unlock();
    turn_part(3, ferros_sys::now() - t);
    WATCH_DONE_AT.store(ferros_sys::now(), Ordering::Release);
    match r {
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

/// The amplifiers' watch on a thread of its own (2026-11-24): woken by the clock thread every `WATCH_FRAMES` written (`ring_watch`), it reads both amplifiers' status, so a slow SPI read (measured to 4.5 ms) never holds the next period's mix past the AoC's read.
/// A read finding an error bit stops everything (`fault`); a watch that falls behind is caught by the clock thread (`WATCH_STALE_FRAMES`).
extern "C" fn watcher(_: u64) -> ! {
    let name = b"watch";
    ferros_sys::svc(ferros_sys::nr::THREAD_NAME, ferros_sys::off(name.as_ptr()), name.len() as u64, 0, 0, 0);
    let mut seen = WATCH_BELL.load(Ordering::Acquire);
    let mut pinned = 0;
    loop {
        ferros_sys::wait(&WATCH_BELL, seen, 0);
        let want = WATCH_PIN.load(Ordering::Relaxed);
        if want != pinned {
            ferros_sys::pin(want);
            pinned = want;
        }
        seen = WATCH_BELL.load(Ordering::Acquire);
        watch();
    }
}

/// Frames written to the AoC since the watch was last rung (the clock thread's alone).
static WATCH_DUE: AtomicU64 = AtomicU64::new(0);
/// The watch's cadence in frames written: 2^8 (5.3 ms), the period the watch was designed for (AUDIO.md "Speaker protection").
/// Rung every interrupt instead, the watch read both amplifiers about once a millisecond on the MMAP path and on any path with the microphones on (their ring shares the line): about 500 us of an A715 each, half a core (2026-11-24, the Pro, `diag sched`).
const WATCH_FRAMES: u64 = 1 << 8;

/// The clock thread's ring of the watch, once `WATCH_FRAMES` have been written while sound plays.
fn ring_watch() {
    if WATCH_DUE.load(Ordering::Relaxed) < WATCH_FRAMES {
        return;
    }
    WATCH_DUE.store(0, Ordering::Relaxed);
    WATCH_BELL.fetch_add(1, Ordering::AcqRel);
    ferros_sys::wake(&WATCH_BELL, 1);
}

/// The watch's age checked at the clock thread's turn (the lock held): past `WATCH_STALE_FRAMES` the amplifiers are unknown, said, and everything stopped as for an error bit (`fault`, which returns with the lock released). True when it stopped.
fn watch_stale(st: &mut State) -> bool {
    let last = WATCH_DONE_AT.load(Ordering::Acquire);
    let age = ferros_sys::now().wrapping_sub(last);
    if last == 0 || age <= ferros_sys::freq() * WATCH_STALE_FRAMES / audio::RATE as u64 {
        return false;
    }
    say!("audio: the amplifiers' watch is stale: A#{} us since its last read, more than A#{} frames: their state is unknown, so everything is stopped", age * 1_000_000 / ferros_sys::freq(), WATCH_STALE_FRAMES);
    fault(st);
    true
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
static mut WATCH_STACK: [u8; 1 << 14] = [0; 1 << 14];
static mut MSG: [u8; 4096] = [0; 4096];
/// The `mic` endpoint's thread (`mic_server`): its stack and its message; its ops are a few bytes.
static mut MIC_STACK: [u8; 1 << 13] = [0; 1 << 13];
static mut MIC_MSG: [u8; 1 << 6] = [0; 1 << 6];

/// Bring the sound up if it is not: true when playing. Called without the lock (the AoC and the amplifiers take tens of milliseconds); one start at a time (`starting`), from the receiving thread, or from the clock thread for a recording's playback (mic.rs `recorded`): a second caller meanwhile is refused, as one is while the sound is brought down.
pub fn ensure_up(p: Play) -> bool {
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
    amp_lock();
    let r = start(p);
    amp_unlock();
    let st = lock();
    st.starting = false;
    let why = match r {
        Ok(s) => {
            st.services = Some(s);
            st.play = p;
            // The watch's age counts from here: the amplifiers were read at their power-up (`start`).
            WATCH_DONE_AT.store(ferros_sys::now(), Ordering::Release);
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
    INPUT[0].store(ferros_sys::arc_base() + _input, Ordering::Relaxed);
    INPUT[1].store(_input_len, Ordering::Relaxed);
    ferros_sys::thread_spawn(clock, unsafe { &mut *(&raw mut CLOCK_STACK) }, 0);
    ferros_sys::thread_spawn(mic::poller, unsafe { &mut *(&raw mut POLL_STACK) }, 0);
    ferros_sys::thread_spawn(watcher, unsafe { &mut *(&raw mut WATCH_STACK) }, 0);
    ferros_sys::thread_spawn(mic_server, unsafe { &mut *(&raw mut MIC_STACK) }, 0);
    let ep = cap(grants, S_AUDIO);
    let msg = unsafe { &mut *(&raw mut MSG) };
    let mut token = [0u8; 32];
    // From the second message on: a service with nothing to play (a STATE, a refused start, a stale handle) ends before it waits again, so an arc is held only while there is sound. The first is the call that started it.
    let mut served = false;
    let mut taken = false;
    loop {
        // The call before acted on: no longer counted against the idle check (`receiving`).
        if taken {
            lock().receiving -= 1;
            unlock();
            taken = false;
        }
        if served {
            exit_if_idle();
        }
        served = true;
        let n = recv(ep, msg, &mut token, false) as usize;
        if n as u64 == ERR || n == 0 {
            continue;
        }
        let m = &msg[..n];
        // Any call releases a sweep's dump: the host has read it.
        {
            let st = lock();
            st.dump_held = false;
            st.receiving += 1;
            unlock();
            taken = true;
        }
        match m[0] {
            audio::OPEN if n == 2 => {
                if !ensure_up(Play::DEFAULT) {
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
                let per = st.play.per;
                unlock();
                let mut r = [0u8; 19];
                r[1..9].copy_from_slice(&h);
                r[9..13].copy_from_slice(&RATE.to_le_bytes());
                r[13..15].copy_from_slice(&(per as u16).to_le_bytes());
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
                            Ok(n) => say!("audio: the AoC says entry point A#{} has taken A#{n} samples", s.ep),
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
                    st.clip.stop(st.play.per);
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
                if !ensure_up(Play::DEFAULT) {
                    let st = lock();
                    let r = report(st, 1);
                    unlock();
                    reply(&token, &r);
                    continue;
                }
                let st = lock();
                // The protocol's periods are the default's 256 frames.
                st.tone.start(level, periods << 8, mask);
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
            // The capture ops are the microphones', on `mic` alone (`mic_server`, 2026-11-26): every kist holds `audio`, so here they fall to the refusal below.
            audio::BENCH if (14..=16).contains(&n) => bench(token, m),
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

/// The `mic` endpoint's receiving thread (2026-11-26): the capture ops for a caller the person granted the microphones (the kernel hands `mic`'s send right only with the mic right, and to Settings), every other op refused.
/// The kernel marks a microphone mailbox view in use for the privacy indicator; these held calls are Settings' meter and test recording.
extern "C" fn mic_server(_: u64) -> ! {
    let ep = cap(grants(), S_MIC);
    let msg = unsafe { &mut *(&raw mut MIC_MSG) };
    let mut token = [0u8; 32];
    let mut taken = false;
    loop {
        // As the `audio` thread: the call before acted on, then an idle service ends (a capture started from here ends with its call's answer, and a service started by a call on `mic` ends once nothing is left).
        // A call the other thread takes in the instant before it counts it is answered as any call whose server ended, and its next call starts a fresh service.
        if taken {
            lock().receiving -= 1;
            unlock();
            taken = false;
            exit_if_idle();
        }
        let n = recv(ep, msg, &mut token, false) as usize;
        if n as u64 == ERR || n == 0 {
            continue;
        }
        lock().receiving += 1;
        unlock();
        taken = true;
        let m = &msg[..n];
        match m[0] {
            audio::MIC_LEVELS if n == 1 => mic::levels(token, aoc::Shape::LINUX),
            // The capture's shape asked with it (2026-11-24): its period in whole milliseconds and its width's code.
            // WHY the bounds: the caller's bytes; the AoC's field is a u8 of milliseconds (a period of 0 is none), its widths 1 to 3 (aoc.rs `WIDTH_16`..`WIDTH_32`); a period past 10 ms is longer than Linux's own and buys nothing here.
            audio::MIC_LEVELS if n == 3 && (1..=10).contains(&m[1]) && (aoc::WIDTH_16..=aoc::WIDTH_32).contains(&m[2]) => mic::levels(token, aoc::Shape { ms: m[1], width: m[2], gain: None, kind: aoc::REC_NORMAL }),
            audio::RECORD if n == 3 => mic::record(token, m[1], m[2]),
            audio::MIC_STREAM if n == 1 => mic::stream(token),
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
    // Every audio ring the AoC lists (aoc_alsa_drv.c:34-74): where each lies, so which ones the granted windows reach is known (2026-11-24, the low-latency paths).
    for name in [&b"audio_output_control"[..], b"audio_input_control", b"audio_playback0", b"audio_playback1", b"audio_playback5", b"audio_raw", b"audio_capture0", b"audio_capture1", b"audio_capture2", b"audio_capture3", b"audio_hifiin", b"audio_hifiout", b"audio_android_aec", b"audio_voip_rx", b"audio_voip_tx", b"audio_bulk_tx", b"audio_bulk_rx", b"audio_output_tuning", b"audio_input_tuning"] {
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

/// The bench (`BENCH`, 2026-11-24; docs/AUDIO.md "The bench"): the sound brought up in a shape of the caller's (the lead, the buffer the AoC is told of, the period), a click train or a held tone played at a quiet level, and while it plays, every interrupt and period measured; with the capture on, every click looked for in the microphones (the loopback). Its call is held and answered at the end, the measures on the console. Everything is stamped with the AoC's own counts: its count of samples taken (GET_EP_TOT_SAMPLES) at the start and the end, the playback ring's byte counts, the capture's frames.
#[derive(Clone, Copy)]
pub struct Bench {
    pub on: bool,
    pub done: bool,
    /// The capture held while the sound comes up, before the bench starts (`on` is still false, so no period counts toward it).
    holding: bool,
    capture: bool,
    pub token: [u8; 32],
    play: Play,
    spacing: u32,
    t0: u64,
    samples0: u64,
    /// The interrupts the playback saw, and the gaps between them in counter ticks: least, most, sum, and by 2^k microseconds.
    irqs: u32,
    last_t: u64,
    gap_min: u64,
    gap_max: u64,
    gap_sum: u64,
    gaps: [u32; 64],
    /// Bytes queued at the AoC when it rang (least, most), and bytes it took between two rings (least, most).
    q_min: u32,
    q_max: u32,
    last_rx: u32,
    take_min: u32,
    take_max: u32,
    /// Frames played since the last click, or since the tone ended.
    after: u32,
    /// The clicks written: when, and the bytes ahead of each in the AoC's ring.
    written: [(u64, u32); 64],
    nw: usize,
    /// The clicks found: when the read that held each was taken, the frames of the capture after its first loud sample, the microphone it crossed its threshold on first, and the loudest sample any microphone gave while it was looked for. A click never found is a zero time, with its loudest.
    found: [(u64, u64, u8, u64); 64],
    /// The loudest sample while the click under way is looked for.
    loudest: u64,
    nf: usize,
    /// The 1 kHz detector per microphone: the last cycle's products with the sine and the cosine (a ring of `SINE`'s length) and their sums, so the energy at 1 kHz over the last millisecond is I² + Q² for the cost of two adds a sample.
    iq: [[(i32, i32); 48]; audio::MICS],
    sum: [(i64, i64); audio::MICS],
    /// The microphones' 1 kHz energy before the first click (each one's most), the threshold from it, a click armed (written, not yet found), and the frame it was found at.
    noise: [u64; audio::MICS],
    thr: [u64; audio::MICS],
    armed: bool,
    hit: Option<(u64, u8)>,
    /// The sweep's dump (flag bit 3): its address (0 for none), the capture frame it starts at, the frames stored, the read anchors stored.
    dump: u64,
    dump_from: u64,
    dump_frames: u64,
    anchors: u64,
    /// Copied at the end for the report: the capture's (reads, least and most bytes a read, bytes skipped, frames), the service's (periods, underruns, starved).
    shape_ms: u8,
    pub mic: (u32, u32, u32, u32, u64),
    pub counts: (u64, u32, u32),
}

impl Bench {
    pub const NONE: Bench = Bench { on: false, done: false, holding: false, capture: false, token: [0; 32], play: Play::DEFAULT, spacing: 0, t0: 0, samples0: 0, irqs: 0, last_t: 0, gap_min: u64::MAX, gap_max: 0, gap_sum: 0, gaps: [0; 64], q_min: u32::MAX, q_max: 0, last_rx: 0, take_min: u32::MAX, take_max: 0, after: 0, written: [(0, 0); 64], nw: 0, found: [(0, 0, 0, 0); 64], nf: 0, loudest: 0, iq: [[(0, 0); 48]; audio::MICS], sum: [(0, 0); audio::MICS], noise: [0; audio::MICS], thr: [0; audio::MICS], armed: false, hit: None, shape_ms: 0, dump: 0, dump_from: 0, dump_frames: 0, anchors: 0, mic: (0, 0, 0, 0, 0), counts: (0, 0, 0) };

    /// The capture is held for the bench.
    pub fn capturing(&self) -> bool {
        (self.on || self.holding) && self.capture
    }

    /// The AoC rang the playback: the gap since its last ring, what it left queued, what it took since.
    fn interrupt(&mut self, t: u64, queued: u32, rx: u32) {
        if !self.on {
            return;
        }
        if self.irqs != 0 {
            let g = t - self.last_t;
            self.gap_min = self.gap_min.min(g);
            self.gap_max = self.gap_max.max(g);
            self.gap_sum += g;
            let us = g * 1_000_000 / ferros_sys::freq();
            self.gaps[63 - (us | 1).leading_zeros() as usize] += 1;
            let took = rx.wrapping_sub(self.last_rx);
            self.take_min = self.take_min.min(took);
            self.take_max = self.take_max.max(took);
        }
        self.q_min = self.q_min.min(queued);
        self.q_max = self.q_max.max(queued);
        self.irqs += 1;
        self.last_t = t;
        self.last_rx = rx;
    }

    /// A period written: a click that began in it recorded (when, and the bytes ahead of its first frame), the frames since the last click counted, and the bench done once the train (or the tone) has ended and a spacing of silence followed it.
    fn period(&mut self, clicks: &mix::Clicks, tone: &mix::Tone, feed: &mix::Feed, click: Option<(u64, u32)>, per: usize) {
        if !self.on {
            return;
        }
        if let Some((t, ahead)) = click {
            // The first click sets each microphone's threshold from the 1 kHz energy before it: 2^4 times its most (12 dB), so the room does not trigger it.
            if self.nw == 0 {
                for k in 0..audio::MICS {
                    self.thr[k] = (self.noise[k] << 4) + (1 << 4);
                }
            }
            // A click still armed was never heard: kept as a miss.
            if self.armed && self.nf < self.found.len() {
                self.found[self.nf] = (0, 0, 0, self.loudest);
                self.nf += 1;
            }
            self.loudest = 0;
            if self.nw < self.written.len() {
                self.written[self.nw] = (t, ahead);
                self.nw += 1;
            }
            self.armed = self.capture;
            self.hit = None;
            self.after = 0;
        } else {
            self.after += per as u32;
        }
        let playing = clicks.left != 0 || tone.left != 0 || feed.on;
        self.done = !playing && self.after >= self.spacing;
        // The last click, never heard by the end: kept as a miss.
        if self.done && self.armed && self.nf < self.found.len() {
            self.found[self.nf] = (0, 0, 0, self.loudest);
            self.nf += 1;
            self.armed = false;
        }
    }

    /// A capture sample (mic.rs `turn`): before the first click, each microphone's noise peak; after a click, its first sample over the threshold.
    pub fn sample(&mut self, mic: usize, x: i16, frame: u64) {
        if !self.capturing() {
            return;
        }
        if self.dump != 0 && self.on && frame >= self.dump_from {
            let n = frame - self.dump_from;
            if n < DUMP_FRAMES {
                // SAFETY: the test mailbox's frame, 2 MiB mapped read-write for this process (SYS_DMA); `DUMP_SAMPLES` + n * 3 + mic is inside it by `DUMP_FRAMES`'s bound.
                unsafe { core::ptr::write_volatile((self.dump as *mut i16).add(DUMP_SAMPLES + (n as usize) * audio::MICS + mic), x) };
                // Counted at any microphone: a two-channel record path never reaches the third.
                if n + 1 > self.dump_frames {
                    self.dump_frames = n + 1;
                }
            }
        }
        // The sample's products with the sine and the cosine at its place in the cycle (the cosine a quarter cycle on), the oldest of the cycle taken out of the sums: a sliding one-cycle correlation with 1 kHz, whatever the burst's phase.
        let k = (frame % 48) as usize;
        let s = (x as i32 * mix::SINE[k] as i32) >> 15;
        let c = (x as i32 * mix::SINE[(k + 12) % 48] as i32) >> 15;
        let old = self.iq[mic][k];
        self.iq[mic][k] = (s, c);
        let (i, q) = &mut self.sum[mic];
        *i += (s - old.0) as i64;
        *q += (c - old.1) as i64;
        let e = (*i * *i + *q * *q) as u64;
        if self.nw == 0 {
            if e > self.noise[mic] {
                self.noise[mic] = e;
            }
            return;
        }
        if self.armed && e > self.loudest {
            self.loudest = e;
        }
        if self.armed && self.hit.is_none() && e > self.thr[mic] {
            self.hit = Some((frame, mic as u8));
        }
    }

    /// A capture read ended at time `t` with `total` frames in: a click found in it recorded.
    pub fn read(&mut self, t: u64, total: u64) {
        if self.dump != 0 && self.on && self.anchors < DUMP_ANCHORS {
            // SAFETY: as `sample`: anchor k at words 8 + 2k and 9 + 2k, inside the frame by `DUMP_ANCHORS`.
            unsafe {
                let w = self.dump as *mut u64;
                core::ptr::write_volatile(w.add(8 + 2 * self.anchors as usize), total);
                core::ptr::write_volatile(w.add(9 + 2 * self.anchors as usize), t);
            }
            self.anchors += 1;
        }
        if let Some((frame, mic)) = self.hit.take() {
            if self.nf < self.found.len() {
                self.found[self.nf] = (t, total - frame, mic, self.loudest);
                self.nf += 1;
            }
            self.armed = false;
        }
    }

    /// The bench's end (the clock thread, its lock let go): the AoC's sample count read again, every measure said, the call answered.
    fn report(&self, token: &[u8; 32], s: &aoc::Services) {
        if self.dump != 0 {
            // The dump's header, eight words: a mark, the counter's frequency, the feed's first frame's write time and the bytes ahead of it, the capture frame the samples start at, the frames stored, the anchors stored, and the shape (entry point, lead, period, capture ms).
            let (tw, ahead) = self.written[0];
            let p = self.play;
            let head = [DUMP_MARK, ferros_sys::freq(), tw, ahead as u64, self.dump_from, self.dump_frames, self.anchors, p.ep as u64 | (p.lead as u64) << 8 | (p.per as u64) << 16 | (self.shape_ms as u64) << 32];
            for (k, v) in head.into_iter().enumerate() {
                // SAFETY: as `sample`: the first eight words of the frame.
                unsafe { core::ptr::write_volatile((self.dump as *mut u64).add(k), v) };
            }
            say!("audio: bench: dump in the test mailbox: A#{} frames of three microphones from capture frame A#{}, A#{} anchors; the feed written at G#{:X}, A#{} bytes ahead", self.dump_frames, self.dump_from, self.anchors, tw, ahead);
        }
        let f = ferros_sys::freq();
        let us = |ticks: u64| ticks * 1_000_000 / f;
        let elapsed = ferros_sys::now() - self.t0;
        let samples1 = aoc::samples(s).unwrap_or(0);
        let p = self.play;
        say!("audio: bench: entry point A#{}, lead A#{}, the AoC told of A#{} periods, period A#{} frames (A#{} us); A#{} us, A#{} periods, A#{} underruns, A#{} starved", p.ep, p.lead, p.buf, p.per, p.per as u64 * 1_000_000 / 48_000, us(elapsed), self.counts.0, self.counts.1, self.counts.2);
        say!("audio: bench: the AoC's samples A#{} to A#{}: A#{} taken, A#{} expected at 48 kHz over the bench", self.samples0, samples1, samples1.wrapping_sub(self.samples0), us(elapsed) * 48 / 1000);
        // The mean gap over the gaps there were: one fewer than the interrupts (none for one interrupt, said as 0).
        let mean = if self.irqs > 1 { us(self.gap_sum / (self.irqs as u64 - 1)) } else { 0 };
        say!("audio: bench: the clock thread's longest turn A#{} us: the microphones' A#{}, the playback's A#{} (of it the amplifiers' watch A#{})", us(TURN_MAX[0].load(Ordering::Relaxed)), us(TURN_MAX[1].load(Ordering::Relaxed)), us(TURN_MAX[2].load(Ordering::Relaxed)), us(TURN_MAX[3].load(Ordering::Relaxed)));
        {
            let sl: [u64; 6] = core::array::from_fn(|k| amp::SLOW[k].load(Ordering::Relaxed));
            say!("audio: bench: the slowest status read A#{} us: its longest access A#{} us (at G#{:X}), its longest gap between accesses A#{} us, its longest STATUS wait A#{} us, A#{} accesses", us(sl[0]), us(sl[1]), sl[2], us(sl[3]), us(sl[4]), sl[5]);
            let mut l = Line { b: [0; 256], n: 0 };
            for (k, c) in amp::BY_US.iter().enumerate() {
                let c = c.load(Ordering::Relaxed);
                if c != 0 {
                    let _ = write!(l, " >=A#{}:A#{}", 1u64 << k, c);
                }
            }
            say!("audio: bench: status reads by us from{}", core::str::from_utf8(&l.b[..l.n]).unwrap_or(""));
        }
        say!("audio: bench: A#{} interrupts, gaps A#{} to A#{} us, mean A#{}; queued at a ring A#{} to A#{} bytes; taken between rings A#{} to A#{} bytes", self.irqs, us(self.gap_min), us(self.gap_max), mean, self.q_min, self.q_max, self.take_min, self.take_max);
        let mut l = Line { b: [0; 256], n: 0 };
        for (k, &c) in self.gaps.iter().enumerate().filter(|(_, c)| **c != 0) {
            let _ = write!(l, " >=A#{}:A#{}", 1u64 << k, c);
        }
        say!("audio: bench: gaps by us from{}", core::str::from_utf8(&l.b[..l.n]).unwrap_or(""));
        if self.capture {
            let (reads, rmin, rmax, skipped, frames) = self.mic;
            say!("audio: bench: microphone gains (G#80000000 not answered): hardware A#{} cB, record soft gain A#{} dB", aoc::GAINS[0].load(Ordering::Relaxed), aoc::GAINS[1].load(Ordering::Relaxed));
            // Energies as amplitudes (their square roots), the scale of a sample.
            say!("audio: bench: capture A#{} reads of A#{} to A#{} bytes, A#{} skipped, A#{} frames; the 1 kHz noise before the first click A#{} A#{} A#{}, thresholds A#{} A#{} A#{} (amplitudes over a cycle)", reads, rmin, rmax, skipped, frames, self.noise[0].isqrt(), self.noise[1].isqrt(), self.noise[2].isqrt(), self.thr[0].isqrt(), self.thr[1].isqrt(), self.thr[2].isqrt());
            let (mut n, mut lo, mut hi, mut sum, mut wlo, mut whi, mut wsum) = (0i64, i64::MAX, i64::MIN, 0i64, u64::MAX, 0u64, 0u64);
            let mut l = Line { b: [0; 256], n: 0 };
            for (k, &(tr, lag, mic, loud)) in self.found[..self.nf].iter().enumerate() {
                let Some(&(tw, ahead)) = self.written.get(k) else { break };
                if tr == 0 {
                    let _ = write!(l, " miss(A#{})", loud.isqrt());
                    continue;
                }
                // Write to read: the click put in the ring to the read that held its start. Refined: back by the frames that came in after its start, so the capture's period is not counted (what is left is the playback's path, the air, and the capture's own delivery).
                let wtr = us(tr - tw);
                // Signed: a read late after the click's start can hold more frames after it than the time since its write.
                let refined = wtr as i64 - (lag * 1000 / 48) as i64;
                n += 1;
                lo = lo.min(refined);
                hi = hi.max(refined);
                sum += refined;
                wlo = wlo.min(wtr);
                whi = whi.max(wtr);
                wsum += wtr;
                let _ = write!(l, " A#{refined}/A#{wtr}(m{mic},A#{},q A#{})", loud.isqrt(), ahead / audio::FRAME_BYTES as u32);
            }
            say!("audio: bench: loopback us (refined/write to read, mic, frames queued ahead):{}", core::str::from_utf8(&l.b[..l.n]).unwrap_or(""));
            if n != 0 {
                say!("audio: bench: loopback A#{} of A#{} clicks heard: refined A#{} to A#{} us, mean A#{}; write to read A#{} to A#{} us, mean A#{}", n, self.nw, lo, hi, sum / n, wlo, whi, wsum / n as u64);
            }
        }
        let mut r = [0u8; 1 + 4 * 6];
        for (k, v) in [self.irqs, us(self.gap_max) as u32, self.q_min, self.take_max, self.counts.2, self.nf as u32].into_iter().enumerate() {
            r[1 + 4 * k..5 + 4 * k].copy_from_slice(&v.to_le_bytes());
        }
        reply(token, &r);
    }
}

/// The dump's layout in the test mailbox's frame (2 MiB): eight header words, then `DUMP_ANCHORS` anchors (capture frames in, the counter at that read: two words each), then from byte `DUMP_SAMPLES` * 2 the samples, three microphones interleaved, `DUMP_FRAMES` frames at most (2^18, 5.5 s).
const DUMP_MARK: u64 = 0x5357_4545_5044_554D;
const DUMP_ANCHORS: u64 = 1 << 13;
const DUMP_SAMPLES: usize = (8 + 2 * DUMP_ANCHORS as usize) * 4;
const DUMP_FRAMES: u64 = 1 << 18;
const _: () = assert!(DUMP_SAMPLES * 2 + DUMP_FRAMES as usize * audio::MICS * 2 <= 1 << 21, "the dump fits the mailbox's frame");

/// `BENCH` (the receiving thread): [op, lead, buf, period's log2, level, count (u16), spacing's log2, flags, capture ms, capture width, entry point, record soft gain dB (i8), hardware gain dB (i8)]. Flags: bit 0 clicks (else a held tone as long as the train), bit 1 the capture on (the loopback), bit 2 the two gains set at the capture's start (else the AoC's own), bit 3 the spawn input played once (a sweep) with the capture dumped into the test mailbox (a hand spawn's `--dma test-mailbox` and `--input-file`), bits 4 and 5 the feed on the left or the right speaker alone.
fn bench(token: [u8; 32], m: &[u8]) {
    let (lead, buf, psh, level) = (m[1] as u32, m[2] as u32, m[3] as u32, m[4]);
    let count = u16::from_le_bytes([m[5], m[6]]) as u32;
    let ssh = m[7] as u32;
    let (clicks, capture) = (m[8] & 1 != 0, m[8] & 2 != 0);
    // Flags bits 6 and 7: the record path (aoc.rs `REC_NORMAL`, `REC_MMAP`, `REC_RAW`).
    let shape = aoc::Shape { ms: m[9], width: m[10], gain: (m[8] & 4 != 0).then_some((m[12] as i8, m[13] as i8)), kind: (m[8] >> 6) & 3 };
    let ep = m[11];
    // Byte 14, when sent: the clock thread's cores (bit k core k: the A510s 0 to 3, the A715s 4 to 7), to measure a pinned thread's doorbell to run.
    if m.len() >= 15 && m[14] != 0 {
        PIN.store(m[14] as u64, Ordering::Relaxed);
    }
    if m.len() >= 16 && m[15] != 0 {
        WATCH_PIN.store(m[15] as u64, Ordering::Relaxed);
    }
    let feed = m[8] & 8 != 0;
    // WHY each bound (the caller's bytes): a lead of 0 plays nothing and a buffer under the lead is a lie to the AoC; a period from 2^4 frames (ALSA's least, 16 bytes, is four frames; 2^4 is the least the AoC is asked for here) to `PERIOD` (the mixer's arrays); the level at most `TONE_MAX`, the unprotected speaker's ceiling, never 0 (the default volume, far louder); a spacing of 2^10 to 2^16 frames (21 ms, longer than any loopback, to 1.4 s); the capture's shape as `MIC_LEVELS` takes it.
    let ok = aoc::ring_name(ep).is_some() && lead != 0 && buf >= lead && (4..=8).contains(&psh) && level != 0 && level <= TONE_MAX && count != 0 && (10..=16).contains(&ssh) && shape.kind <= aoc::REC_RAW && (!capture || ((1..=10).contains(&shape.ms) && (aoc::WIDTH_16..=aoc::WIDTH_32).contains(&shape.width)));
    if !ok {
        say!("audio: bench refused: lead A#{lead}, buffer A#{buf}, period 2^A#{psh}, level A#{level}, count A#{count}, spacing 2^A#{ssh}, capture A#{} ms width A#{}", shape.ms, shape.width);
        reply(&token, &[1]);
        return;
    }
    {
        let st = lock();
        let busy = st.up || st.siggen || st.closing || st.starting || st.stopping || st.bench.on || st.bench.holding || st.mic.busy();
        unlock();
        if busy {
            say!("audio: bench refused: sound or the microphones are in use");
            reply(&token, &[1]);
            return;
        }
    }
    let play = Play { lead, buf, per: 1 << psh, ep };
    let spacing = 1u32 << ssh;
    // The feed: the spawn input's samples after its u32 length (the bridge's `--input-file` framing), and the dump's frame. WHY the checks: both are the hand spawn's, absent on the kernel's own start.
    let (mut fbase, mut flen, mut dump) = (0u64, 0usize, 0u64);
    if feed {
        let (b, l) = (INPUT[0].load(Ordering::Relaxed), INPUT[1].load(Ordering::Relaxed));
        // SAFETY: the spawn input lies in this process's arc at `b`, `l` bytes; its first four are the file's length.
        let n = if l >= 4 { (unsafe { core::ptr::read_unaligned(b as *const u32) }) as u64 } else { 0 };
        let d = ferros_sys::dma(ferros_sys::cap(grants(), S_DUMP));
        if n == 0 || n + 4 > l || !capture || d == ferros_sys::ERR || d == 0 {
            say!("audio: bench refused: a sweep needs the spawn input (--input-file), the capture, and the test mailbox (--dma test-mailbox)");
            reply(&token, &[1]);
            return;
        }
        (fbase, flen, dump) = (b + 4, (n >> 1) as usize, d);
    }
    // The capture first, so its noise is measured before the first click; it is held for the bench from here (`Bench::capturing`).
    if capture {
        {
            let st = lock();
            st.bench = Bench { holding: true, capture: true, ..Bench::NONE };
            unlock();
        }
        if !mic::ensure(shape) {
            let st = lock();
            st.bench = Bench::NONE;
            unlock();
            reply(&token, &[2]);
            return;
        }
    }
    if !ensure_up(play) {
        let st = lock();
        st.bench = Bench::NONE;
        unlock();
        reply(&token, &[1]);
        return;
    }
    let st = lock();
    let s = st.services;
    unlock();
    let Some(s) = s else {
        reply(&token, &[1]);
        return;
    };
    let samples0 = aoc::samples(&s).unwrap_or(0);
    let st = lock();
    // The noise is measured from here, the amplifiers up: their start's pop is not the room.
    st.bench = Bench { on: true, capture, token, play, spacing, t0: ferros_sys::now(), samples0, dump, dump_from: st.mic.total, shape_ms: shape.ms, ..Bench::NONE };
    if feed {
        // Flags bits 4 and 5: the left and the right speaker alone; neither, both.
        let mask = match (m[8] >> 4) & 3 { 0 => LEFT | RIGHT, k => k };
        st.feed = mix::Feed { base: fbase, len: flen, at: 0, gain: audio::gain_q15(level), on: true, mask };
    } else if clicks {
        st.clicks.start(level, count, spacing);
    } else {
        st.tone.start(level, count * spacing, LEFT | RIGHT);
    }
    unlock();
    say!("audio: bench: lead A#{lead}, buffer A#{buf}, period A#{} frames, level A#{level}, A#{count} {} A#{spacing} frames apart, capture {}", 1 << psh, if clicks { "clicks" } else { "spacings of tone," }, if capture { "on" } else { "off" });
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

ferros_sys::entry!(audio_main);
