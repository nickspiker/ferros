//! The Sound page's speaker, microphone and buzz tests (owner, 2026-11-22: "Just throw a mic and speaker test in the ferros settings and I can test it"; earlier, "sound and buzz, why not"; 2026-11-24, moved from Developer: "yes, add a sound page"; docs/AUDIO.md "The Sound page's tests").
//!
//! - **Speaker:** a 1 kHz tone from the audio service (payloads/audio `TONE`) on the left, right or both amplifiers while a button is held, at a level the page's slider sets, never past the service's ceiling (ferros_sys audio `TONE_MAX`: there is no speaker protection yet). The finger's lift stops it; so does any amplifier error bit, and the service's own bound (`TONE_HOLD`, 5.5 s) if this map never sends the stop. The amplifiers' state comes back with every answer and is shown as numbers.
//! - **Microphones:** a live level per microphone while "listen" is checked (`MIC_LEVELS`, a held call answered as the microphones' samples come in), and a recording of the chosen microphone played back on the speaker (`RECORD`), scaled to the same slider's level. The recording lives in the audio service's memory only and is gone once played.
//! - **Buzz:** one ROM waveform of the vibration motor thru the NFC driver (its op 8, docs/NFC.md), waveform 1 by default, the one the notes tap with.
//!
//! Nothing here waits on the main loop's thread: the calls go on a worker of this module's own, the meter on a second, and each answer is published in atomics the main loop reads once a pass (`pass`), as the WiFi page's picker does. Nothing is stored: every setting here is off, or its default, at each start.

use super::*;
use core::sync::atomic::AtomicU64;
use ferros_sys::audio;

/// Grant slot 12: send on `audio`, the kernel's, the slot after the torch's (process.rs, since 2026-11-22). A kernel from before leaves it empty, and the page says the kernel needs updating.
pub const S_AUDIO: u64 = 12;

/// Whether this map was handed `audio`: its grants page counts 13 slots or more (the console, init's ten, the kernel's torch and audio after them).
pub fn granted(grants: u64) -> bool {
    // SAFETY: the grants page is this process's, its slot count at G#200 (as wifi::granted reads it).
    unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + grants + 0x200) as *const u32) > 12 }
}

/// The level the tests start at: 111, nine octaves (54 dB) under full scale, the first step of docs/AUDIO.md's "A sound you can hear, quietly"; the slider runs from 1 to `audio::TONE_MAX`.
pub const LEVEL: u8 = 111;
/// The motor's ROM waveform the buzz starts at (notes.rs `TAP`, the one played in its bring-up), and the ROM's last (the driver's limit, payloads/nfc `M_ROM_LAST`).
pub const WAVE: u8 = 1;
pub const WAVE_LAST: u8 = 0x26;

/// The level slider's value at `t` (0 to 1 across its track): 1 to `audio::TONE_MAX`, a 16th of an octave a step, dimmest on the left; and where a level sits on it.
pub fn level_of(t: f64) -> u8 {
    1 + libm::round(t * (audio::TONE_MAX - 1) as f64) as u8
}
pub fn level_place(level: u8) -> f64 {
    (level - 1) as f64 / (audio::TONE_MAX - 1) as f64
}
/// The waveform slider's: 0 to `WAVE_LAST`.
pub fn wave_of(t: f64) -> u8 {
    libm::round(t * WAVE_LAST as f64) as u8
}
pub fn wave_place(wave: u8) -> f64 {
    wave as f64 / WAVE_LAST as f64
}

/// What the main loop keeps of the tests, never stored.
pub struct Tests {
    pub level: u8,
    pub wave: u8,
    /// The microphone a recording takes.
    pub mic: u8,
    pub listening: bool,
    /// Why the meter last stopped by itself: 0 it did not, 1 no audio service answered, 2 the microphones would not start, 3 no audio right. Cleared when "listen" is checked again.
    pub listen_why: u8,
    /// The speaker button held: its channels.
    holding: Option<u8>,
    seen: u64,
    levels_seen: u64,
    shown: bool,
}

impl Tests {
    pub fn new() -> Tests {
        Tests { level: LEVEL, wave: WAVE, mic: 0, listening: false, listen_why: 0, holding: None, seen: 0, levels_seen: 0, shown: false }
    }
}

// ---- requests: counted up at each (the worker waits on ASK) ----
static ASK: AtomicU64 = AtomicU64::new(0);
/// The tone wanted: bit 16 a request, bit 17 start (else stop), bits 0-7 the level, 8-9 the channels.
static TONE_WANT: AtomicU64 = AtomicU64::new(0);
/// A buzz wanted: bit 8 a request, bits 0-7 the ROM waveform.
static BUZZ_WANT: AtomicU64 = AtomicU64::new(0);
/// A recording wanted: bit 16 a request, bits 0-7 the level, 8-9 the microphone.
static REC_WANT: AtomicU64 = AtomicU64::new(0);

// ---- answers: each outcome with the count of answers so far in its high bits; SEQ counts every answer, for the main loop ----
static SEQ: AtomicU64 = AtomicU64::new(0);
/// The speaker's outcome (`Spk`), and the amplifiers' report: per side IRQ1_STATUS1 << 32 | IRQ1_STATUS3.
static SPK: AtomicU64 = AtomicU64::new(0);
static AMPS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
/// The buzz's outcome: 0 none yet, 1 played, 2 not.
static BUZZ: AtomicU64 = AtomicU64::new(0);
/// The recording's outcome (`Rec`) in bits 0-7, its peak in 8-23.
static REC: AtomicU64 = AtomicU64::new(0);
/// The meter: wanted (0 not, else the showing it is for), its peaks (three u16), the count of answers, and why its last call failed (`Tests::listen_why`'s codes, 0 it did not).
static LISTEN: AtomicU64 = AtomicU64::new(0);
static LISTENS: AtomicU64 = AtomicU64::new(0);
static LEVELS: AtomicU64 = AtomicU64::new(0);
static LEVELS_SEQ: AtomicU64 = AtomicU64::new(0);
static LEVELS_FAILED: AtomicU64 = AtomicU64::new(0);

/// The speaker's outcomes.
#[derive(Clone, Copy, PartialEq)]
pub enum Spk {
    None,
    Playing,
    Stopped,
    Refused,
    NoAnswer,
    Fault,
    NoRight,
}
/// The recording's outcomes.
#[derive(Clone, Copy, PartialEq)]
pub enum Rec {
    None,
    Recording,
    Played,
    Silent,
    MicsWouldNotStart,
    NoAnswer,
    SpeakerWouldNotStart,
    Refused,
    NoRight,
}

fn ask() {
    ASK.fetch_add(1, Ordering::AcqRel);
    ferros_sys::wake(&ASK, 1);
}

fn answered() {
    SEQ.fetch_add(1, Ordering::AcqRel);
}

/// The buzz asked for (the page's button).
pub fn buzz(wave: u8) {
    BUZZ_WANT.store(1 << 8 | wave as u64, Ordering::Release);
    ask();
}

/// A recording asked for (the page's button).
pub fn record(mic: u8, level: u8) {
    REC_WANT.store(1 << 16 | (mic as u64) << 8 | level as u64, Ordering::Release);
    REC.store(Rec::Recording as u64, Ordering::Release);
    answered();
    ask();
}

/// The speaker as it last answered, and the amplifiers' words per side (IRQ1_STATUS1, IRQ1_STATUS3).
pub fn speaker() -> (Spk, [(u32, u32); 2]) {
    let o = match SPK.load(Ordering::Acquire) {
        1 => Spk::Playing,
        2 => Spk::Stopped,
        3 => Spk::Refused,
        4 => Spk::NoAnswer,
        5 => Spk::Fault,
        6 => Spk::NoRight,
        _ => Spk::None,
    };
    let w = |k: usize| {
        let v = AMPS[k].load(Ordering::Acquire);
        ((v >> 32) as u32, v as u32)
    };
    (o, [w(0), w(1)])
}

/// The buzz as it last answered: None not yet, else whether it played.
pub fn buzzed() -> Option<bool> {
    match BUZZ.load(Ordering::Acquire) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// The recording as it last answered, and its peak.
pub fn recorded() -> (Rec, u16) {
    let v = REC.load(Ordering::Acquire);
    let o = match v & 0xFF {
        1 => Rec::Recording,
        2 => Rec::Played,
        3 => Rec::Silent,
        4 => Rec::MicsWouldNotStart,
        5 => Rec::NoAnswer,
        6 => Rec::SpeakerWouldNotStart,
        7 => Rec::Refused,
        8 => Rec::NoRight,
        _ => Rec::None,
    };
    (o, (v >> 8) as u16)
}

/// The microphones' peaks as the meter last had them; None before its first answer in this showing.
pub fn levels() -> Option<[u16; audio::MICS]> {
    let v = LEVELS.load(Ordering::Acquire);
    (v >> 63 == 1).then(|| [v as u16, (v >> 16) as u16, (v >> 32) as u16])
}

/// A peak as decibels under full scale (32768), rounded: 0 for full scale, about 90 for a peak of 1. None for silence (a peak of 0 has no level in decibels).
pub fn under(peak: u16) -> Option<u64> {
    (peak != 0).then(|| libm::roundf(20. * libm::log10f(32768. / peak as f32)) as u64)
}

/// A level (the volume's scale: 16ths of an octave under full scale) as decibels under full scale, rounded: an octave of amplitude is 20 log10 2 dB.
pub fn level_under(level: u8) -> u64 {
    libm::roundf((255 - level) as f32 * (20. * libm::log10f(2.)) / ((1 << 4) as f32)) as u64
}

/// The main loop's turn, once a pass: the speaker's button followed (held: the tone on; let go, or the finger moved off: off), the meter wanted while the page shows with "listen" checked, and true when an answer came that the page shows.
pub fn pass(map: &mut Map, page_shown: bool) -> bool {
    let t = &mut map.tests;
    let want = match map.pressed {
        Some(Choice::SpeakerHold(m)) if page_shown => Some(m),
        _ => None,
    };
    if want != t.holding {
        t.holding = want;
        let w = match want {
            Some(m) => 1 << 16 | 1 << 17 | (m as u64) << 8 | t.level as u64,
            None => 1 << 16,
        };
        TONE_WANT.store(w, Ordering::Release);
        ask();
    }
    let listen = page_shown && t.listening;
    if listen != t.shown {
        t.shown = listen;
        let v = if listen { LISTENS.fetch_add(1, Ordering::AcqRel) + 1 } else { 0 };
        if listen {
            LEVELS.store(0, Ordering::Release);
        }
        LISTEN.store(v, Ordering::Release);
        ferros_sys::wake(&LISTEN, 1);
    }
    // A meter whose call failed (no audio service, or the microphones would not start) is switched off: the page says why, and a check starts it again. No retry on a timer.
    let why = LEVELS_FAILED.swap(0, Ordering::AcqRel);
    if t.listening && why != 0 {
        t.listening = false;
        t.listen_why = why as u8;
    }
    let mut dirty = false;
    let s = SEQ.load(Ordering::Acquire);
    if s != t.seen {
        t.seen = s;
        dirty = true;
    }
    let l = LEVELS_SEQ.load(Ordering::Acquire);
    if l != t.levels_seen {
        t.levels_seen = l;
        dirty = true;
    }
    dirty && page_shown
}

/// The worker: asleep on ASK until the page wants something, then each call made in turn (a held RECORD keeps it about three seconds; a tone pressed meanwhile is taken after it, and a press already let go by then sends nothing).
pub extern "C" fn worker(grants: u64) -> ! {
    let audio_cap = cap(grants, S_AUDIO);
    let mut seen = 0u64;
    // A tone start sent and not yet stopped: a stop is sent only after one.
    let mut started = false;
    loop {
        let n = ASK.load(Ordering::Acquire);
        if n == seen {
            ferros_sys::wait_world(&ASK, n, 0);
            continue;
        }
        seen = n;
        let t = TONE_WANT.swap(0, Ordering::AcqRel);
        if t & 1 << 16 != 0 && (t & 1 << 17 != 0 || started) {
            let start = t & 1 << 17 != 0;
            if !granted(grants) {
                SPK.store(Spk::NoRight as u64, Ordering::Release);
            } else {
                let hold = audio::TONE_HOLD.to_le_bytes();
                // A stop is periods 0: the service fades it out and starts nothing for it.
                let m = if start { [audio::TONE, t as u8, hold[0], hold[1], (t >> 8) as u8 & 3] } else { [audio::TONE, 1, 0, 0, audio::LEFT | audio::RIGHT] };
                let mut r = [0u8; 1 + audio::REPORT];
                let got = call(audio_cap, &m, &mut r);
                let o = if got == ferros_sys::ERR || got == 0 {
                    Spk::NoAnswer
                } else {
                    if got as usize == r.len() {
                        for k in 0..2 {
                            let s1 = u32::from_le_bytes([r[1 + 8 * k], r[2 + 8 * k], r[3 + 8 * k], r[4 + 8 * k]]);
                            let s3 = u32::from_le_bytes([r[5 + 8 * k], r[6 + 8 * k], r[7 + 8 * k], r[8 + 8 * k]]);
                            AMPS[k].store((s1 as u64) << 32 | s3 as u64, Ordering::Release);
                        }
                    }
                    let fault = got as usize == r.len() && r[audio::REPORT] == 1;
                    match (r[0], fault, start) {
                        (_, true, _) => Spk::Fault,
                        (0, _, true) => Spk::Playing,
                        (0, _, false) => Spk::Stopped,
                        _ => Spk::Refused,
                    }
                };
                SPK.store(o as u64, Ordering::Release);
            }
            started = start;
            answered();
        }
        let b = BUZZ_WANT.swap(0, Ordering::AcqRel);
        if b & 1 << 8 != 0 {
            let mut r = [0xFFu8; 1];
            let got = call(cap(grants, S_NFC), &[8, b as u8], &mut r);
            let ok = got != ferros_sys::ERR && got != 0 && r[0] == 0;
            BUZZ.store(if ok { 1 } else { 2 }, Ordering::Release);
            if !ok {
                console(grants, b"map: buzz test: the motor did not play (NFC switched off, or its driver not up)\n");
            }
            answered();
        }
        let w = REC_WANT.swap(0, Ordering::AcqRel);
        if w & 1 << 16 != 0 {
            let o = if !granted(grants) {
                (Rec::NoRight, 0)
            } else {
                let mut r = [0u8; 4 + audio::REPORT];
                let got = call(audio_cap, &[audio::RECORD, (w >> 8) as u8 & 3, w as u8], &mut r);
                if got == ferros_sys::ERR || got == 0 {
                    (Rec::NoAnswer, 0)
                } else {
                    match r[0] {
                        0 if got as usize == r.len() => (if r[3] == 1 { Rec::Played } else { Rec::SpeakerWouldNotStart }, u16::from_le_bytes([r[1], r[2]])),
                        2 => (Rec::MicsWouldNotStart, 0),
                        3 => (Rec::Silent, 0),
                        _ => (Rec::Refused, 0),
                    }
                }
            };
            REC.store(o.0 as u64 | (o.1 as u64) << 8, Ordering::Release);
            answered();
        }
    }
}

/// The meter, on a thread of its own: asleep while nobody listens; listening, `MIC_LEVELS` called again as soon as it answers (the service holds each call until the microphones have sent the next 80 ms), the peaks published. A call that fails ends the listening (`pass` unchecks it); nothing is asked again on a timer.
pub extern "C" fn meter(grants: u64) -> ! {
    let audio_cap = cap(grants, S_AUDIO);
    loop {
        let w = LISTEN.load(Ordering::Acquire);
        if w == 0 || !granted(grants) {
            if w != 0 {
                LEVELS_FAILED.store(3, Ordering::Release);
                LEVELS_SEQ.fetch_add(1, Ordering::AcqRel);
                LISTEN.store(0, Ordering::Release);
                continue;
            }
            ferros_sys::wait_world(&LISTEN, w, 0);
            continue;
        }
        let mut r = [0u8; 1 + 2 * audio::MICS + 8];
        let got = call(audio_cap, &[audio::MIC_LEVELS], &mut r);
        // The listening stopped (or started again) while the call was held: this answer is not that showing's.
        if LISTEN.load(Ordering::Acquire) != w {
            continue;
        }
        if got as usize == r.len() && r[0] == 0 {
            let p = |k: usize| u16::from_le_bytes([r[1 + 2 * k], r[2 + 2 * k]]) as u64;
            LEVELS.store(1 << 63 | p(0) | p(1) << 16 | p(2) << 32, Ordering::Release);
            LEVELS_SEQ.fetch_add(1, Ordering::AcqRel);
        } else if got == 1 && r[0] == 1 {
            // Replaced by another caller's meter: asked again.
        } else {
            LEVELS_FAILED.store(1 + (got == 1 && r[0] == 2) as u64, Ordering::Release);
            LEVELS_SEQ.fetch_add(1, Ordering::AcqRel);
            console(grants, b"map: microphone test: the audio service did not give levels (the console says why)\n");
            LISTEN.store(0, Ordering::Release);
        }
    }
}

/// A message as a line, beside the ones `say` writes (the page's outcome rows are either).
pub fn line(t: &str) -> Line {
    let mut l = Line::new();
    let _ = l.write_str(t);
    l
}
