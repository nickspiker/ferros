//! The one place a ferros process touches the machine (KERNEL.md, process ABI): the `svc` syscalls, the generic timer and the process's arc base.
//!
//! Everything else in a driver or stack is plain no_std Rust. Today processes are native aarch64 capsules; when the one backend exists (Rust → WASM → toka → native with compiled masking), this crate becomes WASM imports and the code above it is recompiled, not rewritten (KERNEL.md "Build order").
//!
//! Memory passed to the kernel is named by arc offset (pointer − arc base): every buffer handed to a syscall must live in the process's own arc — its stack or its image — which is all a process has.

#![no_std]

use core::arch::asm;
use core::sync::atomic::{compiler_fence, Ordering};

/// A syscall failed: unknown or wrong token, out-of-range offset, no such object.
pub const ERR: u64 = u64::MAX;
/// A polling `recv` found no message waiting.
pub const NOTHING: u64 = u64::MAX - 1;
/// OR'd into a `call`'s reply length: fail with ERR instead of waiting when no server is receiving on the endpoint (none alive, or one still bringing its device up and not yet receiving; now, or once its last server ends). Without it a call waits for a server, as across a restart (process.rs CALL_LIVE).
pub const CALL_LIVE: u64 = 1 << 63;

/// Syscall numbers (x8).
pub mod nr {
    pub const EXIT: u64 = 0;
    pub const CALL: u64 = 1;
    pub const RECV: u64 = 2;
    pub const REPLY: u64 = 3;
    pub const MMIO_READ: u64 = 4;
    pub const MMIO_WRITE: u64 = 5;
    pub const ENTROPY: u64 = 6;
    pub const DMA: u64 = 7;
    pub const INPUT: u64 = 8;
    pub const SLEEP: u64 = 9;
    pub const SPAWN: u64 = 10;
    pub const SET_PREF: u64 = 11;
    pub const FACTS: u64 = 12;
    pub const STORE: u64 = 13;
    pub const THREAD_SPAWN: u64 = 14;
    pub const THREAD_EXIT: u64 = 15;
    pub const WAIT: u64 = 16;
    pub const WAKE: u64 = 17;
    pub const IRQ_WAIT: u64 = 18;
    pub const IRQ_ACK: u64 = 19;
    pub const PIN: u64 = 20;
    pub const LIGHT: u64 = 21;
    pub const POWER: u64 = 22;
    pub const IRA: u64 = 23;
    pub const UNLOCK: u64 = 24;
    pub const PT: u64 = 25;
    pub const SENSORS: u64 = 26;
    pub const STATUS: u64 = 27;
    pub const PET: u64 = 28;
    pub const BLOCK: u64 = 29;
    pub const VAULT: u64 = 30;
    pub const TRACKS: u64 = 31;
    pub const CONTROL: u64 = 32;
    pub const CONTROLS: u64 = 33;
    pub const DETAIL: u64 = 34;
}
/// Core masks for `pin` (bit = core index on husky): the A715 workers, where processes run by default, and the X3, the kernel's core, for one real compute path.
pub const CORES_WORKERS: u64 = 0x00F0;
pub const CORES_BIG: u64 = 0x0100;

/// Where the kernel keeps this process's copy of the device preferences (SETTINGS.md): a 32-byte block in the grants page, rewritten by the kernel whenever a preference changes.
pub const PREFS: u64 = 0xFC0;
/// Preference 0: the number base, tatau's code.
pub const PREF_BASE: u64 = 0;
/// Preference 1: names under the map's circles (0 off, 1 on).
pub const PREF_LABELS: u64 = 1;
/// Preference 2: the screen, 0 on, 1 off (the power key toggles it).
pub const PREF_SCREEN_OFF: u64 = 2;
/// Preference 3: brightness in stops below the brightest white (0-8).
pub const PREF_BRIGHTNESS: u64 = 3;
/// Preference 4: the dither (0 spatial and temporal, 1 spatial only).
pub const PREF_DITHER: u64 = 4;
/// Preference 5: the theme, an index into `theme::THEMES` (0 = grey).
pub const PREF_THEME: u64 = 5;
/// Preference 7: the light scope, 0 off or 1–6 the VD6282 channel its fast trace follows (register order: 1 red, 2 visible, 3 blue, 4 green, 5 IR, 6 clear; provisional).
pub const PREF_SCOPE: u64 = 7;
/// Preference 6: the proximity wake, 0 off or 1 on (the kernel refuses a wake press while proximity reads near).
pub const PREF_PROX_WAKE: u64 = 6;
/// Preference 12: the honeycomb keyboard's layout, 0 Dvorak (the default) or 1 QWERTY with Q on the bottom row (honeycomb::LAYOUTS).
pub const PREF_LAYOUT: u64 = 12;
/// Preference 13: unlock at boot, 0 off (the default: the handle is typed each boot, nothing of the identity on disk) or 1 on (the seed is stored so the device unlocks without typing — a stolen device then holds the identity).
pub const PREF_UNLOCK_AT_BOOT: u64 = 13;
/// Preference 14: the UI scale an active app lays its page out by, a signed byte of 64ths of an octave: its layout unit times 2^(s/64), 0 one, -64 a half, 64 twice (a pinch inside an open circle sets it; owner, 2026-11-05).
pub const PREF_SCALE: u64 = 14;
/// Preference 15: the radios switched off, a bit each in the facts' order (fact 32: 0 cellular, 1 WiFi, 2 Bluetooth, 3 NFC, 4 GPS); the status circles toggle them (owner, 2026-11-07). The kernel stops a radio's driver while its bit is set and starts it again when it clears.
pub const PREF_RADIOS_OFF: u64 = 15;

/// The UI scale as set (preference 14): 64ths of an octave, signed; the factor is 2^(s/64).
pub fn ui_scale() -> i8 {
    pref(PREF_SCALE) as i8
}

/// A preference byte as this process's copy holds it now.
pub fn pref(index: u64) -> u8 {
    unsafe { core::ptr::read_volatile((arc_base() + PREFS + index.min(31)) as *const u8) }
}

/// The device's number base right now (tatau's code: 10 arabic, 12 DMS, 16 hex, G#8C DLS). Read it at draw time, not once: the person can change it while the app runs.
pub fn device_base() -> u8 {
    unsafe { core::ptr::read_volatile((arc_base() + PREFS + PREF_BASE) as *const u8) }
}

/// The bottom of the camera cutout, in panel rows: a fact of this panel, not a knob. husky's (Pixel 8 Pro, 1344 x 2992), measured on the glass (owner, 2026-11-08, tools/cutout-scan), under the white rule: a pixel is dead unless it shows pure white (the hole's edge shows the subpixels' fringe, cyan, magenta and the rest, and those count dead). The 5776 dead pixels, both sides marked row by row, rows 33 to 116, every row centred on x 671.5 (half a pixel left of the panel's middle), top and bottom mirrored about y 75.0 but for rows 34 to 37, a pixel wider on the left than their mirrors; checked by lighting the ring of pixels just outside them white on black (every one pure white). The knockout is the smallest circle covering every one of them whole, radius 43.56 about (671.5, 75.0) (the farthest dead centres 43.06 out, the ends of the flat sides, rows 65 and 84 at columns 629 and 713, plus half a pixel); its bottom edge, row 118.56, is what the app circle is tangent to. The dead area is flatter than a circle (one column for 20 rows at each side, 21 pixels across the top row), so the circle also covers 54 live pixels whole and 266 in part; the owner keeps the circle. The first map (pixels hidden completely, fringe counted live) gave radius 42.96; Android's DisplayCutout said (672, 75) radius 43.25, bottom 118.25.
pub const CUTOUT_BOTTOM: f32 = 118.5610;
/// akita's (Pixel 8a, AK3B at 1080 x 2400), measured on the glass (owner, 2026-11-08, tools/cutout-scan) under the white rule: a pixel is dead unless it shows pure white (the edge's fringe colours count dead). The 3276 dead pixels, rows 37 to 100, every row centred on x 539.5, top and bottom mirror images about y 69.0; checked by lighting the ring of pixels just outside them white on black (every one pure white). The knockout (the glass mask over the hole) is the smallest circle that covers every one of those pixels whole: radius 32.83 about (539.5, 69.0), the farthest dead centres 32.33 out (rows 48 and 89, columns 514 and 564) plus half a pixel, so its antialiased ramp lands only on live pixels (12 covered whole, 202 in part). Its bottom edge, row 101.83, is what the app circle is tangent to. The first map (pixels hidden completely, 3148) gave radius 32.30; Google's overlay (config_mainBuiltInDisplayCutout) says (539.5, 68.71) radius 33.72.
pub const CUTOUT_BOTTOM_AKITA: f32 = 101.8303;
/// The camera hole's knockout, the glass mask's circle over it: centre and radius in panel pixels (pixel edges: column x spans x..x + 1). both measured on their glass (`CUTOUT_BOTTOM`, `CUTOUT_BOTTOM_AKITA`).
pub const CUTOUT_HOLE: (f32, f32, f32) = (671.5, 75.0, 43.5610);
pub const CUTOUT_HOLE_AKITA: (f32, f32, f32) = (539.5, 69.0, 32.8303);

/// The board fact (SYS_FACTS 84: 1 husky, 2 akita) as first read, 0 until then: it does not change while a process runs.
static BOARD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Whether this device is akita (the Pixel 8a), for what differs from husky's panel and parts.
pub fn akita() -> bool {
    let mut b = BOARD.load(Ordering::Relaxed);
    if b == 0 {
        b = facts().0[84];
        BOARD.store(b, Ordering::Relaxed);
    }
    b == 2
}

/// This panel's cutout bottom (`CUTOUT_BOTTOM`, `CUTOUT_BOTTOM_AKITA`).
pub fn cutout_bottom() -> f32 {
    if akita() { CUTOUT_BOTTOM_AKITA } else { CUTOUT_BOTTOM }
}

/// This panel's camera hole (`CUTOUT_HOLE`, `CUTOUT_HOLE_AKITA`): centre x, centre y, radius.
pub fn cutout_hole() -> (f32, f32, f32) {
    if akita() { CUTOUT_HOLE_AKITA } else { CUTOUT_HOLE }
}

/// The circle an app lives in, on a `w` × `h` screen: centre and radius in pixels. Tangent to the bottom of the camera cutout and to the bottom of the screen, centred, so the corners outside it belong to the system (owner, 2026-11-02); the display draws its hairline, and every active app's page is inscribed in it (owner, 2026-11-05: the settings page fills it, the attest mark keeps its own smaller circle).
pub fn active_circle(w: usize, h: usize) -> (f32, f32, f32) {
    let c = cutout_bottom();
    let r = (h as f32 - c) * 0.5;
    (w as f32 * 0.5, c + r, r)
}

/// A number formatter in the device base, right now: `tatau::count(&mut out, ferros_sys::numbers(Style::Glyphs), n, 1)`. Glyphs where the app draws with the Oxanium +glyphs face, words elsewhere.
pub fn numbers(style: tatau::Style) -> tatau::Fmt {
    tatau::Fmt::new(tatau::Base::from_code(device_base()).unwrap_or(tatau::Base::Dms), style)
}

/// Change a device preference, given the Settings capability: the kernel keeps it in the vault and every app sees it on its next frame. Returns 0, or ERR (no capability, or a value the preference does not take).
pub fn set_pref(cap: u64, index: u64, value: u8) -> u64 {
    svc(nr::SET_PREF, cap, index, value as u64, 0, 0)
}

/// `set_pref` applied to every process's preferences at once but not kept in the vault: a slider while the finger is down (owner, 2026-11-08: the brightness slider wrote the settings into the vault at every step and the settings ran at 20 fps). `set_pref` at the lift keeps it.
pub fn set_pref_live(cap: u64, index: u64, value: u8) -> u64 {
    svc(nr::SET_PREF, cap, index | 1 << 8, value as u64, 0, 0)
}

/// A radio's link, as its driver measures it, for the corners' circles (kernel SYS_STATUS, facts 33-80): up to eight measures a link (the five radios, then USB), each a tagged word, the kind in bits 48-63 and its value, signed, in bits 0-47, in the kind's own unit. A link is many measures, never one number and never bars (owner, 2026-11-03: "frequency, bandwidth, speed, latency, flavor, packet loss ... RSSI, VSWR"): a driver publishes what it measures and nothing else, and a new measure is a new kind, not a new layout. `MEASURE_NONE` (all ones) is an empty slot.
pub mod measure {
    /// 0 off, 1 on (up, no link), 2 connected.
    pub const STATE: u16 = 0;
    /// The technology, `TECH_*`.
    pub const TECH: u16 = 1;
    /// Received signal strength, milli-dBm.
    pub const RSSI: u16 = 2;
    /// The noise floor, milli-dBm.
    pub const NOISE: u16 = 3;
    /// Signal over noise, milli-dB.
    pub const SNR: u16 = 4;
    /// Cellular reference-signal received power, milli-dBm.
    pub const RSRP: u16 = 5;
    /// Cellular reference-signal received quality, milli-dB.
    pub const RSRQ: u16 = 6;
    /// Signal over interference and noise, milli-dB.
    pub const SINR: u16 = 7;
    /// The antenna's voltage standing wave ratio, thousandths (1500: 1.5 to 1).
    pub const VSWR: u16 = 8;
    /// Transmit power, milli-dBm.
    pub const TX_POWER: u16 = 9;
    /// The channel's centre frequency, kHz.
    pub const FREQ: u16 = 10;
    /// The channel's bandwidth, kHz.
    pub const BANDWIDTH: u16 = 11;
    /// The channel number (WiFi) or the band (cellular), as the standard numbers it.
    pub const CHANNEL: u16 = 12;
    /// The link's rate down and up, bit/s.
    pub const RATE_DOWN: u16 = 13;
    pub const RATE_UP: u16 = 14;
    /// Round trip, ns.
    pub const LATENCY: u16 = 15;
    /// Loss, parts per million.
    pub const LOSS: u16 = 16;
    /// The modulation and coding scheme index, and the spatial streams.
    pub const MCS: u16 = 17;
    pub const STREAMS: u16 = 18;
    /// GPS: satellites tracked, satellites in the fix, the best carrier to noise (milli-dB-Hz).
    pub const SATS_SEEN: u16 = 19;
    pub const SATS_USED: u16 = 20;
    pub const CN0: u16 = 21;
    /// GPS: the fix's horizontal error estimate, mm.
    pub const ACCURACY: u16 = 22;
    /// GPS: one tracked signal (a track word, `tracks`): `sat_word` packs it, `sat_of` unpacks it.
    pub const SAT: u16 = 23;
    /// Cellular: the data stack's registration state as the CP reports it (0 not registered, 1 home, 2 searching, 3 denied, 4 unknown, 5 roaming; the CP's own 20 an emergency attach).
    pub const REGISTRATION: u16 = 24;
    /// The largest packet the link takes, bytes.
    pub const MTU: u16 = 25;
    /// Packets received and sent since the driver started (counts).
    pub const PACKETS_DOWN: u16 = 26;
    pub const PACKETS_UP: u16 = 27;
    /// Packets received bad, and sent that failed, since the driver started (counts).
    pub const ERRORS_DOWN: u16 = 28;
    pub const ERRORS_UP: u16 = 29;
    /// Packets received and dropped by the driver (its queue full, or malformed), and packets refused going up (the ring full) (counts).
    pub const DROPPED_DOWN: u16 = 30;
    pub const DROPPED_UP: u16 = 31;
    /// The link's power-saving mode as the radio reports it (WiFi: 0 off, 1 on, 2 fast).
    pub const POWER_SAVE: u16 = 32;
    /// The link powered down to save power and back up, since the driver started (count; cellular: the PCIe link's suspends).
    pub const SUSPENDS: u16 = 33;
    /// GPS, with a fix: altitude (mm), ground speed (mm/s), heading (millidegrees from true north).
    pub const ALTITUDE: u16 = 34;
    pub const SPEED: u16 = 35;
    pub const HEADING: u16 = 36;
    /// USB: the link's power state (0 U0 working, 1 U1, 2 U2, 3 U3 suspended).
    pub const LINK_POWER: u16 = 37;
}

/// The links' control blocks (docs/RADIO-CONTROLS.md): which link, and what each slot of it means. Slot 0 of every block is its generation (counted up by every set); 1-15 the knobs, all ones (`UNSET`) for the driver's own default. A value knob is kept in the settings document; an action is a counter the page counts up and the driver acts on once.
pub mod control {
    pub const CELLULAR: u64 = 0;
    pub const WIFI: u64 = 1;
    pub const BLUETOOTH: u64 = 2;
    pub const NFC: u64 = 3;
    pub const GPS: u64 = 4;
    pub const USB: u64 = 5;
    /// The clock: the kernel is its driver.
    pub const CLOCK: u64 = 6;
    /// A slot never set: the driver's own default.
    pub const UNSET: u64 = u64::MAX;
    /// Words in a block.
    pub const WORDS: usize = 1 << 4;
    /// Cellular, an action: the data stack's radio off and on again (it registers afresh; the call comes back once it is home).
    pub const CELL_REREGISTER: u64 = 1;
    /// WiFi, kept: power save, 0 off (the default), 1 on (PM1), 2 fast (PM2). Applied at once.
    pub const WIFI_POWER_SAVE: u64 = 1;
    /// WiFi, kept: the band to join on, 0 any (5 GHz whenever the network has it, the default), 1 5 GHz only, 2 2.4 GHz only. Applied at the next join.
    pub const WIFI_BAND: u64 = 2;
    /// The clock: the zone, seconds east of UTC (signed, within a day). Kept as the settings document's `tz`.
    pub const CLOCK_ZONE: u64 = 1;
}

/// Set knob `slot` of link `link` (`control`) to `word`, given the Settings capability. Returns true when the kernel took it (and kept it, for a kept knob).
pub fn control(cap: u64, link: u64, slot: u64, word: u64) -> bool {
    svc(nr::CONTROL, cap, link, slot, word, 0) == 0
}

/// Link `link`'s control block now: the generation, then slots 1-15 (`control::UNSET` where unset). All zero for no such link.
pub fn controls(link: u64) -> [u64; control::WORDS] {
    let mut b = [0u64; control::WORDS];
    let r = svc(nr::CONTROLS, link, off(b.as_mut_ptr() as *const u8), b.len() as u64, 0, 0);
    if r == ERR {
        return [0; control::WORDS];
    }
    b
}

/// This driver's detail document (ferros_detail::build), kept by the kernel as its link's (at most 1 KiB). True when taken.
pub fn detail_set(doc: &[u8]) -> bool {
    svc(nr::DETAIL, 0, off(doc.as_ptr()), doc.len() as u64, 0, 0) == 0
}

/// Link `link`'s detail document into `buf`, given the Settings capability: the document's whole length (0: none yet), or None without the capability.
pub fn detail(cap: u64, link: u64, buf: &mut [u8]) -> Option<usize> {
    let r = svc(nr::DETAIL, 1, cap, link, off(buf.as_mut_ptr()), buf.len() as u64);
    (r != ERR).then_some(r as usize)
}

pub const MEASURE_NONE: u64 = u64::MAX;
pub const TECH_NONE: u8 = 0;
pub const TECH_WIFI4: u8 = 1;
pub const TECH_WIFI5: u8 = 2;
pub const TECH_WIFI6: u8 = 3;
pub const TECH_WIFI7: u8 = 4;
pub const TECH_LTE: u8 = 14;
pub const TECH_NR: u8 = 20;
pub const TECH_BT: u8 = 32;
pub const TECH_NFC: u8 = 33;
pub const TECH_GPS: u8 = 34;
pub const TECH_USB: u8 = 35;
/// One measure's word: the kind, and the value in its low 48 bits (two's complement; a driver's measures fit, the widest being a rate in bit/s).
pub fn measure_word(kind: u16, value: i64) -> u64 {
    (kind as u64) << 48 | value as u64 & ((1 << 48) - 1)
}
/// A word back: its kind and its value sign-extended, `None` for an empty slot.
pub fn measure_of(w: u64) -> Option<(u16, i64)> {
    if w == MEASURE_NONE { None } else { Some(((w >> 48) as u16, (w << 16) as i64 >> 16)) }
}
/// This driver's track `k` (0-63; `tracks`), as `status` sets a measure.
pub fn status_track(k: usize, word: u64) -> u64 {
    status(8 + k as u64, word)
}

/// Link `link`'s tracks (0 cellular … 5 USB, the facts' order) into `buf`, at most 64: the words written (each a tagged measure, all ones empty), or 0 for none.
pub fn tracks(link: u64, buf: &mut [u64]) -> usize {
    let r = svc(nr::TRACKS, link, off(buf.as_mut_ptr() as *const u8), buf.len() as u64, 0, 0);
    if r == ERR { 0 } else { r as usize }
}

/// One GPS signal as a track word (measure::SAT), its 47 bits: system (0 GPS, 1 GLONASS, 2 Galileo, 3 BeiDou, 4 QZSS, 5 NavIC, 7 other) in 0-2, band (0 L1/E1/B1/G1, 1 L5/E5a/B2a) in 3, its number in 4-10, used in the fix in 11, elevation (signed degrees) in 12-19, azimuth (degrees) in 20-28, C/N0 (dB-Hz) in 29-34, then elevation's and azimuth's rates (in steps of two millidegrees a second, signed six bits each, -32 for not yet known) in 35-40 and 41-46.
pub struct Sat {
    pub system: u8,
    pub band: u8,
    pub number: u8,
    pub used: bool,
    pub elevation: i8,
    pub azimuth: u16,
    pub cn0: u8,
    pub el_rate: i8,
    pub az_rate: i8,
}

/// The rate a track word gives when it is not known yet.
pub const SAT_NO_RATE: i8 = -32;
/// The tracks a link has (the kernel's TRACKS).
pub const TRACKS_MAX: usize = 1 << 6;

pub fn sat_word(s: &Sat) -> u64 {
    let six = |v: i8| (v as u64) & 63;
    let v = (s.system as u64 & 7) | (s.band as u64 & 1) << 3 | (s.number as u64 & 127) << 4 | (s.used as u64) << 11 | ((s.elevation as u8) as u64) << 12 | (s.azimuth as u64 & 511) << 20 | (s.cn0 as u64 & 63) << 29 | six(s.el_rate) << 35 | six(s.az_rate) << 41;
    measure_word(measure::SAT, v as i64)
}

pub fn sat_of(w: u64) -> Option<Sat> {
    let (kind, v) = measure_of(w)?;
    if kind != measure::SAT {
        return None;
    }
    let v = v as u64;
    let six = |x: u64| ((x as u8) << 2) as i8 >> 2;
    Some(Sat { system: (v & 7) as u8, band: (v >> 3 & 1) as u8, number: (v >> 4 & 127) as u8, used: v >> 11 & 1 != 0, elevation: (v >> 12) as u8 as i8, azimuth: (v >> 20 & 511) as u16, cn0: (v >> 29 & 63) as u8, el_rate: six(v >> 35 & 63), az_rate: six(v >> 41 & 63) })
}

/// Set this driver's measure slot `slot` (0-7; 8 and up its tracks, `status_track`) to `word` (`measure_word`, or `MEASURE_NONE` to empty it). Only the modem, pcie1, bt, nfc, gps and usb drivers have slots; anyone else gets ERR.
pub fn status(slot: u64, word: u64) -> u64 {
    svc(nr::STATUS, slot, word, 0, 0, 0)
}

/// The process watchdog: this process is alive and going round its main loop. Call it once a pass, at the top (never only from inside a wait: a wait that never ends is the hang it is there to catch). The kernel records the longest stretch between two pets (`diag pets`), from the first pet on (the start is not counted) and less the time the petting thread spent in `wait_world` / `irq_wait_world` or blocked calling another process; nothing is enforced yet (owner, 2026-11-05; docs/RELEASE.md).
pub fn pet() {
    svc(nr::PET, 0, 0, 0, 0, 0);
}

/// The device's plain facts, for its dashboard (kernel SYS_FACTS); `None` where a fact is not known.
pub struct Facts(pub [u64; 88]);

/// Local wall time: (seconds since 1970 in local time — UTC plus the zone, `device.tz` — and the counter ticks into the current second), or None until the kernel has read the PMIC's clock. From the kernel's base (facts 8, 9) and this process's own counter.
/// The time zone as the kernel last handed it (preferences bytes 24-31): seconds east of UTC.
pub fn zone() -> i64 {
    unsafe { core::ptr::read_volatile((arc_base() + PREFS + 24) as *const u64) as i64 }
}

pub fn local_time() -> Option<(i64, u64)> {
    // From the preferences block (bytes 16-31, the kernel's PREF_WALL): no syscall — the first version asked SYS_FACTS on every frame, and facts samples the battery gauge over I2C under the kernel lock (it hung the kernel, 2026-10-23).
    let at = arc_base() + PREFS + 16;
    let rd = |o: u64| unsafe { core::ptr::read_volatile((at + o) as *const u64) };
    let (base, tz) = (rd(0), rd(8) as i64);
    if base == 0 {
        return None;
    }
    let hz = freq();
    let t = base.wrapping_add(now());
    Some(((t / hz) as i64 + tz, t % hz))
}

impl Facts {
    fn get(&self, i: usize) -> Option<u64> {
        Some(self.0[i]).filter(|&v| v != u64::MAX)
    }
    /// Which board (fact 84): 1 husky (Pixel 8 Pro), 2 akita (Pixel 8a). For a driver whose part differs between them.
    pub fn board(&self) -> Option<u64> {
        self.get(84)
    }
    /// From the press at "Ready?" to the shell starting, microseconds.
    pub fn boot_us(&self) -> Option<u64> {
        self.get(0)
    }
    /// The battery's state of charge, per cent.
    pub fn battery_pct(&self) -> Option<u64> {
        self.get(1)
    }
    /// The battery's temperature, hundredths of a degree C.
    pub fn battery_temp_cc(&self) -> Option<i64> {
        self.get(2).map(|v| v as i64)
    }
    /// The cell voltage, microvolts.
    pub fn cell_uv(&self) -> Option<u64> {
        self.get(3)
    }
    /// The battery current, microamps, positive = charging.
    pub fn battery_ua(&self) -> Option<i64> {
        self.get(4).map(|v| v as i64)
    }
    /// Whether a host has the USB link.
    pub fn usb_link(&self) -> Option<bool> {
        self.get(5).map(|v| v != 0)
    }
    /// Processes alive.
    pub fn processes(&self) -> Option<u64> {
        self.get(6)
    }
    /// Keys in the vault.
    pub fn vault_keys(&self) -> Option<u64> {
        self.get(7)
    }
    /// The hottest part of the SoC (any thermal zone: CPU clusters, GPU, ISP, TPU, AUR), whole degrees C.
    pub fn soc_temp_c(&self) -> Option<u64> {
        self.get(10)
    }
    /// Whether the kernel has the big core's clock down for heat.
    pub fn throttled(&self) -> Option<bool> {
        self.get(11).map(|v| v != 0)
    }
    /// The gravity vector the AoC last reported, m/s², in the panel's frame: x right, y up, z out of the glass (a device held upright reads y ≈ +9.8, one lying face up z ≈ +9.8). None until the sensors are up.
    pub fn gravity(&self) -> Option<[f32; 3]> {
        let (xy, z) = (self.get(12)?, self.get(13)?);
        Some([f32::from_bits(xy as u32), f32::from_bits((xy >> 32) as u32), f32::from_bits(z as u32)])
    }
    /// Proximity (the front TMD3719): Android's distance (0 near, 5 far by the AoC's own thresholds), the processed pdata0 and pdata1, and the raw photodiode counts pdata0 and pdata1; None before the first sample.
    pub fn proximity(&self) -> Option<[f32; 5]> {
        let (a, b, c) = (self.get(27)?, self.get(28)?, self.get(29)?);
        let f = |v: u64| f32::from_bits(v as u32);
        Some([f(a), f(a >> 32), f(b), f(b >> 32), f(c)])
    }
    /// The rear spectral sensor's (VD6282) newest sample: six channel counts, their six gains, and how many samples have come (a new one when it moves). The registry names the channels r, g, b, c1, c2, ir; which slot is which is not yet measured.
    pub fn spectral(&self) -> Option<([f32; 6], [f32; 6], u64)> {
        let seq = self.get(26)?;
        let w = |i: usize| -> Option<[f32; 2]> {
            let v = self.get(i)?;
            Some([f32::from_bits(v as u32), f32::from_bits((v >> 32) as u32)])
        };
        let (c0, c1, c2, g0, g1, g2) = (w(20)?, w(21)?, w(22)?, w(23)?, w(24)?, w(25)?);
        Some(([c0[0], c0[1], c1[0], c1[1], c2[0], c2[1]], [g0[0], g0[1], g1[0], g1[1], g2[0], g2[1]], seq))
    }
    /// The last two gravity samples, each (x, y, z m/s², its time in counter ticks — the AoC's own stamp), newest first: gravity at any moment between them by interpolation, so a reader can run at a fixed lag behind the device whatever its own rate.
    pub fn gravity_pair(&self) -> Option<(([f32; 3], u64), ([f32; 3], u64))> {
        let v = |i: usize| -> Option<([f32; 3], u64)> {
            let (xy, z, t) = (self.get(i)?, self.get(i + 1)?, self.get(i + 2)?);
            Some(([f32::from_bits(xy as u32), f32::from_bits((xy >> 32) as u32), f32::from_bits(z as u32)], t))
        };
        Some((v(14)?, v(17)?))
    }
}

/// The largest transfer one `SYS_STORE` call moves either way (the kernel's process.rs STORE_MAX): 256 KiB since 2026-11-08, four of the vault's 64 KiB pieces, which the vault process opens on four cores at once. 4 KiB before, and a kernel from before refuses a larger buffer (ERR).
pub const STORE_MAX: usize = 1 << 18;

/// Read `key` from the vault scope a Store capability names (the key is relative to the scope): Some(the value's whole length, of which at most `buf.len()` bytes were copied), or None if it is absent or the call was refused.
pub fn store_get(cap: u64, key: &[u8], buf: &mut [u8]) -> Option<usize> {
    let r = svc(nr::STORE, cap, (buf.len() as u64) << 8, off(key.as_ptr()), key.len() as u64, off(buf.as_mut_ptr()));
    if r == ERR || r == NOTHING { None } else { Some(r as usize) }
}

/// Blocks of a window on the ferros partition (a Blocks capability; the vault process's disk, VAULT-OUT.md): `buf.len() / 4096` blocks (1 to 32: one UFS command, 128 KiB; 16 on a kernel from before 2026-11-08, which refuses more) from block `at` of the window, read into `buf`. Returns 0, or ERR (not a window this process holds, out of it, or the UFS command failed).
pub fn block_read(cap: u64, at: u64, buf: &mut [u8]) -> u64 {
    svc(nr::BLOCK, cap, ((buf.len() as u64 >> 12) & 0xFF) << 8, at, off(buf.as_mut_ptr()), 0)
}

/// `block_read`'s other half: `buf.len() / 4096` blocks written from `buf` at block `at` of the window.
pub fn block_write(cap: u64, at: u64, buf: &[u8]) -> u64 {
    svc(nr::BLOCK, cap, 1 | ((buf.len() as u64 >> 12) & 0xFF) << 8, at, off(buf.as_ptr()), 0)
}

/// The vault process's own call (a Vault capability; VAULT-OUT.md, migration): the `skip`-th entry of the kernel's vault still to move into `buf`, as [class: 1 the person's][the domain's address and sealing keys][the scope prefix's length][the full name]. Its length, NOTHING when none is left, ERR.
pub fn vault_next(cap: u64, skip: u64, buf: &mut [u8]) -> u64 {
    svc(nr::VAULT, cap, (buf.len() as u64) << 8, skip, 0, off(buf.as_mut_ptr()))
}

/// An old entry's value from `offset` into `buf`: its whole length, NOTHING if it is gone, ERR.
pub fn vault_read(cap: u64, name: &[u8], offset: u64, buf: &mut [u8]) -> u64 {
    svc(nr::VAULT, cap, 1 | (buf.len() as u64) << 8 | offset << 32, off(name.as_ptr()), name.len() as u64, off(buf.as_mut_ptr()))
}

/// An old entry written and committed in its domain: removed from the kernel's vault. 0, NOTHING if it was not there, ERR.
pub fn vault_done(cap: u64, name: &[u8]) -> u64 {
    svc(nr::VAULT, cap, 2, off(name.as_ptr()), name.len() as u64, 0)
}
/// Read part of a value larger than one transfer: `buf.len()` bytes (at most `STORE_MAX`) from `offset` of the value under `key`. Returns the value's whole length (the kernel keeps the value between such reads, so walking a large one costs one vault read).
pub fn store_get_at(cap: u64, key: &[u8], offset: usize, buf: &mut [u8]) -> Option<usize> {
    let r = svc(nr::STORE, cap, 2 | (buf.len() as u64) << 8 | (offset as u64) << 32, off(key.as_ptr()), key.len() as u64, off(buf.as_mut_ptr()));
    if r == ERR || r == NOTHING { None } else { Some(r as usize) }
}

/// `store_get_at` with the kernel's answer as it is: the value's whole length, NOTHING (absent) or ERR (refused: nobody present for a person's name, or no such grant), for a caller that must tell absent from refused (a list it would otherwise write over).
pub fn store_read_at(cap: u64, key: &[u8], offset: usize, buf: &mut [u8]) -> u64 {
    svc(nr::STORE, cap, 2 | (buf.len() as u64) << 8 | (offset as u64) << 32, off(key.as_ptr()), key.len() as u64, off(buf.as_mut_ptr()))
}

/// Write a value larger than one transfer in pieces: `piece` at `offset` of the value under `key`, `more` while pieces follow; the piece without `more` commits the whole (the kernel assembles it). True while the kernel takes them.
pub fn store_set_at(cap: u64, key: &[u8], offset: usize, piece: &[u8], more: bool) -> bool {
    svc(nr::STORE, cap, 1 | 2 | if more { 4 } else { 0 } | (piece.len() as u64) << 8 | (offset as u64) << 32, off(key.as_ptr()), key.len() as u64, off(piece.as_ptr())) == 0
}

/// Delete `key` from the scope: Some(whether it was there), None if refused.
pub fn store_delete(cap: u64, key: &[u8]) -> Option<bool> {
    match svc(nr::STORE, cap, 8, off(key.as_ptr()), key.len() as u64, 0) {
        0 => Some(true),
        NOTHING => Some(false),
        _ => None,
    }
}

/// The scope's keys, relative to it, one per line: read like a large value — `buf.len()` bytes (at most 4096) from `offset` of the listing; returns the listing's whole length.
pub fn store_keys(cap: u64, offset: usize, buf: &mut [u8]) -> Option<usize> {
    let r = svc(nr::STORE, cap, 16 | 2 | (buf.len() as u64) << 8 | (offset as u64) << 32, off(b"".as_ptr()), 0, off(buf.as_mut_ptr()));
    if r == ERR || r == NOTHING { None } else { Some(r as usize) }
}

/// An entry kept by its 32-byte address rather than a name (SYS_STORE bit 5; kete's vaults on ferros): read up to `buf.len()` bytes of it; Some(the whole length) or None when absent or refused. Larger values come in pieces with `store_get_addr_at`.
pub fn store_get_addr(cap: u64, addr: &[u8; 32], buf: &mut [u8]) -> Option<usize> {
    let r = svc(nr::STORE, cap, 32 | (buf.len() as u64) << 8, off(addr.as_ptr()), 32, off(buf.as_mut_ptr()));
    if r == ERR || r == NOTHING { None } else { Some(r as usize) }
}

pub fn store_get_addr_at(cap: u64, addr: &[u8; 32], offset: usize, buf: &mut [u8]) -> Option<usize> {
    let r = svc(nr::STORE, cap, 32 | 2 | (buf.len() as u64) << 8 | (offset as u64) << 32, off(addr.as_ptr()), 32, off(buf.as_mut_ptr()));
    if r == ERR || r == NOTHING { None } else { Some(r as usize) }
}

/// Write a whole value under an address, in 4 KiB pieces when it is larger than one transfer. True when the kernel took it.
pub fn store_set_addr(cap: u64, addr: &[u8; 32], value: &[u8]) -> bool {
    if value.len() <= 4096 {
        return svc(nr::STORE, cap, 32 | 1 | (value.len() as u64) << 8, off(addr.as_ptr()), 32, off(value.as_ptr())) == 0;
    }
    let pieces = value.len().div_ceil(4096);
    value.chunks(4096).enumerate().all(|(i, piece)| {
        let more = if i + 1 < pieces { 4 } else { 0 };
        svc(nr::STORE, cap, 32 | 1 | 2 | more | (piece.len() as u64) << 8 | ((i * 4096) as u64) << 32, off(addr.as_ptr()), 32, off(piece.as_ptr())) == 0
    })
}

/// Delete the entry under an address: Some(whether it was there), None if refused.
pub fn store_delete_addr(cap: u64, addr: &[u8; 32]) -> Option<bool> {
    match svc(nr::STORE, cap, 32 | 8, off(addr.as_ptr()), 32, 0) {
        0 => Some(true),
        NOTHING => Some(false),
        _ => None,
    }
}

/// The scope's addresses, 32 bytes each, concatenated: read like a large value from `offset`; returns the listing's whole length.
pub fn store_addrs(cap: u64, offset: usize, buf: &mut [u8]) -> Option<usize> {
    let r = svc(nr::STORE, cap, 32 | 16 | 2 | (buf.len() as u64) << 8 | (offset as u64) << 32, off(b"".as_ptr()), 0, off(buf.as_mut_ptr()));
    if r == ERR || r == NOTHING { None } else { Some(r as usize) }
}

/// Bit 63 of a buffer argument: the buffer is named by its address inside one of the caller's DMA regions instead of its offset in the arc (SYS_PT).
pub const DMA_ADDR: u64 = 1 << 63;

/// Hand the kernel one assembled PT command (`[cap:32][op][params]`) through a Pt capability (the USB driver process, ptlink.rs). `act` resubmits a command whose reply said it waits to act (reload, reboot, `diag android`), once that reply has gone out. False when one is already in, or the command is too large.
pub fn pt_submit(cap: u64, cmd: &[u8], act: bool) -> bool {
    pt_submit_at(cap, off(cmd), cmd.len(), act)
}

/// `pt_submit` for a command at `at`: an arc offset, or `DMA_ADDR | address` in a DMA region the caller holds.
pub fn pt_submit_at(cap: u64, at: u64, len: usize, act: bool) -> bool {
    svc(nr::PT, cap, if act { 3 } else { 0 }, at, len as u64, 0) == 0
}

/// Say a transfer is moving: the kernel counts it as activity and runs the cores at their active clocks for the next second (the USB process calls it at each SPEC and each reply run).
pub fn pt_moving(cap: u64) {
    svc(nr::PT, cap, 2, 0, 0, 0);
}

/// The kernel's reply to the command handed over.
pub enum PtReply {
    /// Still running.
    Running,
    /// Done: the reply's full length (`buf` holds its bytes from the offset asked), whether there is a reply at all (the kernel's own USB sends none after a reload or reboot, nor for a reload with nothing staged), and whether the command waits to act.
    Done(usize, bool, bool),
    /// Nothing was handed over.
    None,
}

/// Take the reply from `offset` into `buf`; the kernel drops it once a take reaches its end.
pub fn pt_take(cap: u64, offset: usize, buf: &mut [u8]) -> PtReply {
    pt_take_at(cap, offset, off(buf as &[u8]), buf.len())
}

/// `pt_take` into `len` bytes at `at`: an arc offset, or `DMA_ADDR | address` in a DMA region the caller holds.
pub fn pt_take_at(cap: u64, offset: usize, at: u64, len: usize) -> PtReply {
    match svc(nr::PT, cap, 1, at, len as u64, offset as u64) {
        NOTHING => PtReply::Running,
        ERR => PtReply::None,
        r => PtReply::Done((r & 0xFFFF_FFFF) as usize, r >> 33 & 1 == 0, r >> 32 & 1 == 1),
    }
}

/// The block the sensors process publishes (payloads/sensors; the kernel's sensorlink.rs repeats these): u64 words, the magic first and written last, then the samples the kernel answers SYS_FACTS and SYS_LIGHT with and reads proximity from for the power key's wake rule, and from word `SCOPE` the light scope's ring of i32 samples.
pub mod sensor_block {
    pub const MAGIC: u64 = 0x5345_4E53_4F52_5301;
    /// x | y << 32 (f32 bits, m/s²), then z | sample count << 32.
    pub const GRAVITY_XY: usize = 1;
    pub const GRAVITY_Z: usize = 2;
    /// The last two gravity samples: [x | y << 32, z, time in counter ticks], newest first (6 words).
    pub const PAIR: usize = 3;
    /// The spectral sensor's channel counts and gains as f32 pairs (6 words), then its sample count.
    pub const SPECTRAL: usize = 9;
    /// Proximity: distance | processed pdata0, processed pdata1 | raw pdata0, raw pdata1 | sample count << 32.
    pub const PROX: usize = 16;
    /// The light scope: rate | bytes << 32 | ignored bits << 40 | little-endian << 48; the next sample's number; the channel mask.
    pub const SCOPE_FMT: usize = 19;
    pub const SCOPE_N: usize = 20;
    pub const SCOPE_MASK: usize = 21;
    pub const SCOPE: usize = 32;
    pub const SCOPE_LEN: usize = 65536;
    /// The block's size in u64 words.
    pub const WORDS: usize = SCOPE + SCOPE_LEN / 2;
}

/// Publish the sensors' block through a Sensors capability: the kernel reads it in place from now on, until this process ends. False if refused (too small, or not in the arc).
pub fn sensors_publish(cap: u64, block: &[u64]) -> bool {
    svc(nr::SENSORS, cap, 0, off(block), (block.len() * 8) as u64, 0) == 0
}

/// The board's CDT and MLB number from the device tree, for the AoC's registry scripts.
pub fn sensors_board(cap: u64) -> (u32, u32) {
    let r = svc(nr::SENSORS, cap, 1, 0, 0, 0);
    if r == ERR { (0, 0) } else { (r as u32, (r >> 32) as u32) }
}

/// A device's power through a Power capability (1 on, 0 off). The kernel runs it only on its own core: this pins the calling thread to the X3 for the call (a millisecond's sleep moves it there) and back to the workers after. True when the sequence succeeded.
pub fn power(cap: u64, on: bool) -> bool {
    pin(CORES_BIG);
    sleep_until(now() + freq() / 1000);
    let r = svc(nr::POWER, cap, on as u64, 0, 0, 0);
    pin(0);
    r == 0
}

/// A per-purpose 32-byte key rooted in the device's ira, through an Ira capability: `blake3::derive_key("ferros process ira v1 " ‖ domain, ira)` — the same bytes on every boot and after a factory reset, never the ira itself. None when the kernel has no ira this boot.
pub fn ira_key(cap: u64, domain: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    let r = svc(nr::IRA, cap, off(domain.as_ptr()), domain.len() as u64, off(out.as_mut_ptr()), 0);
    (r == 32).then_some(out)
}

/// UTC wall time: (seconds since 1970, the counter ticks into the current second, the counter's rate), or None until the kernel has read the PMIC's clock. `local_time` with the zone taken back out — for stamps that cross the network (Photon's eagle time).
pub fn utc_time() -> Option<(i64, u64, u64)> {
    let at = arc_base() + PREFS + 16;
    let rd = |o: u64| unsafe { core::ptr::read_volatile((at + o) as *const u64) };
    let base = rd(0);
    if base == 0 {
        return None;
    }
    let hz = freq();
    let t = base.wrapping_add(now());
    Some(((t / hz) as i64, t % hz, hz))
}

/// Write `value` under `key` in a Store capability's vault scope; true when the vault took it.
pub fn store_set(cap: u64, key: &[u8], value: &[u8]) -> bool {
    svc(nr::STORE, cap, 1 | (value.len() as u64) << 8, off(key.as_ptr()), key.len() as u64, off(value.as_ptr())) == 0
}

/// The light scope's latest samples into `buf` (SYS_LIGHT): (the stream's next sample number, rate Hz, channel mask, how many samples were written at the front of `buf`, oldest first).
pub fn light(buf: &mut [i32]) -> (u64, u32, u32, usize) {
    let bytes = unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * 4) };
    let mut hdr = [0u8; 16];
    // The kernel writes its 16-byte header then the samples into the one buffer: read it off and move the samples to the front.
    let n = svc(nr::LIGHT, off(bytes.as_ptr()), bytes.len() as u64, 0, 0, 0) as usize;
    if n < 16 {
        return (0, 0, 0, 0);
    }
    hdr.copy_from_slice(&bytes[..16]);
    let count = (n - 16) / 4;
    bytes.copy_within(16..16 + count * 4, 0);
    let end = u64::from_le_bytes(hdr[0..8].try_into().unwrap());
    let rate = u32::from_le_bytes(hdr[8..12].try_into().unwrap());
    let mask = u32::from_le_bytes(hdr[12..16].try_into().unwrap());
    (end, rate, mask, count)
}

/// Read the facts now.
pub fn facts() -> Facts {
    let mut b = [0u8; 704];
    svc(nr::FACTS, off(b.as_ptr()), b.len() as u64, 0, 0, 0);
    Facts(core::array::from_fn(|i| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap())))
}

/// The raw trap: number in x8, arguments in x0..x4, result in x0. The kernel may write into any buffer named by the arguments, so nothing is assumed cached across it.
#[inline(always)]
pub fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let r: u64;
    compiler_fence(Ordering::SeqCst);
    unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
    compiler_fence(Ordering::SeqCst);
    r
}

/// Apply the image's own relocations, for a payload linked `-pie` (see payloads/display/payload.ld + build.rs): native payloads run unrelocated at their arc base (KERNEL.md), so any absolute address the linker baked into data — a `dyn` vtable, a function-pointer table, a `&'static str` table — points into nowhere. With `-pie` the linker leaves those words as R_AARCH64_RELATIVE entries in `.rela.dyn` instead, and this adds the load address to each. Call it first in `_entry`, before anything that could touch such data; the loop itself uses only PC-relative addressing. No-op for an image linked without `.rela.dyn`.
///
/// # Safety
/// Must run exactly once, before any other code of the image, from the image this crate was linked into.
#[inline(never)]
pub unsafe fn relocate() {
    unsafe extern "C" {
        static __image_start: u8;
        static __rela_start: u8;
        static __rela_end: u8;
    }
    const R_AARCH64_RELATIVE: u64 = 1027;
    unsafe {
        let base = &raw const __image_start as u64;
        let mut p = &raw const __rela_start as *const u64;
        let end = &raw const __rela_end as *const u64;
        while p < end {
            let (offset, info, addend) = (p.read(), p.add(1).read(), p.add(2).read());
            if info & 0xFFFF_FFFF == R_AARCH64_RELATIVE {
                ((base + offset) as *mut u64).write(base.wrapping_add(addend));
            }
            p = p.add(3);
        }
        asm!("dsb ish", "isb", options(nostack, preserves_flags));
    }
}

/// The process's arc base (x28, set by the kernel at entry and never written by compiled code).
#[inline(always)]
pub fn arc_base() -> u64 {
    let b: u64;
    unsafe { asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
    b
}

/// The generic timer count (EL0 may read it: CNTHCTL_EL2.EL0VCTEN).
#[inline(always)]
pub fn now() -> u64 {
    let t: u64;
    unsafe { asm!("isb", "mrs {}, cntvct_el0", out(reg) t, options(nomem, nostack, preserves_flags)) };
    t
}

/// Timer ticks per second.
#[inline(always)]
pub fn freq() -> u64 {
    let f: u64;
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) f, options(nomem, nostack, preserves_flags)) };
    f
}

/// Busy-wait `us` microseconds on the timer.
pub fn delay_us(us: u64) {
    let (t0, f) = (now(), freq());
    while now().wrapping_sub(t0) < us * f / 1_000_000 {}
}

/// Microseconds since `t0` (a `now()` value).
pub fn us_since(t0: u64) -> u64 {
    now().wrapping_sub(t0) * 1_000_000 / freq()
}

/// Order this process's writes to device-visible memory (its DMA region) before what follows — typically a doorbell write telling the device to look (`dsb sy`).
#[inline(always)]
pub fn dma_barrier() {
    unsafe { asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Arc offset of something in this process's arc.
#[inline(always)]
pub fn off<T: ?Sized>(p: *const T) -> u64 {
    p as *const u8 as u64 - arc_base()
}

/// The token for grant `slot`, given the grants offset the kernel passed at entry (x0): the handle table in the page's first 512 bytes, then at 0x200 a u32 count, then 32-byte tokens; slot 0 is the console.
#[inline(always)]
pub fn cap(grants: u64, slot: u64) -> u64 {
    grants + 0x204 + 32 * slot
}

/// Start the signed capsule stored in the vault under `key` as a new process called `name`, handing it copies of this process's own capabilities in grant `slots` (they become the child's slots 1.. in that order; slot 0 is always its console). The child is queued; this process carries on. Returns 0, or ERR (the reason is on the console). At most 24 slots.
pub fn spawn(grants: u64, name: &[u8], key: &[u8], slots: &[u64]) -> u64 {
    spawn_with(grants, name, key, slots, 0)
}

/// `spawn`, but only if no live process has this name: 0 started, NOTHING already running (nothing started), ERR refused. How init's keepers start a driver again without ever starting a second beside a live one.
pub fn spawn_unique(grants: u64, name: &[u8], key: &[u8], slots: &[u64]) -> u64 {
    spawn_with(grants, name, key, slots, 1)
}

/// The most slots a child can be handed in one spawn: the request, at 64 tokens of 32 bytes, stays inside the 4096 bytes the kernel reads of it.
pub const SPAWN_SLOTS: usize = 64;

fn spawn_with(grants: u64, name: &[u8], key: &[u8], slots: &[u64], flags: u64) -> u64 {
    let mut req = [0u8; 2 + 64 + 64 + 1 + SPAWN_SLOTS * 32];
    // More slots than fit is the caller's bug, so it panics rather than dropping the rest: until 2026-11-04 this took the first 24 without a word, and the display server (25) lost its last, `panel-vddd`, whenever init started it; after a jump from a screen-off Android the panel then came up without VDDD and stayed black.
    assert!(slots.len() <= SPAWN_SLOTS, "spawn: more slots than a request holds");
    let (nl, kl, n) = (name.len().min(64), key.len().min(64), slots.len());
    let mut at = 0;
    req[at] = nl as u8;
    req[at + 1..at + 1 + nl].copy_from_slice(&name[..nl]);
    at += 1 + nl;
    req[at] = kl as u8;
    req[at + 1..at + 1 + kl].copy_from_slice(&key[..kl]);
    at += 1 + kl;
    req[at] = n as u8;
    at += 1;
    for &slot in &slots[..n] {
        let tok = (arc_base() + cap(grants, slot)) as *const u8;
        for i in 0..32 {
            req[at + i] = unsafe { core::ptr::read_volatile(tok.add(i)) };
        }
        at += 32;
    }
    svc(nr::SPAWN, off(req.as_ptr()), at as u64, flags, 0, 0)
}

/// Launch a kist (KISTS.md; `SYS_SPAWN` kist mode): the capsule stored in the vault under its 32-byte address `capsule` (the `capsules.` scope) as a process called `name`, with one slot per want the capsule declares, in its order: the right's code (ferros_kist::RIGHTS) where the person granted it, G#FF (an empty slot) where they did not. The kernel makes every right's object itself from the Kists capability at grant `kists`, and refuses any right the capsule does not declare in that slot once it has read it. 0 queued (the kernel streams the capsule from the vault and starts it; the outcome goes to the console), NOTHING already running or starting, ERR refused (the reason on the console). At most 64 slots. `replace`: a live process of that name is killed first (one shot dead) and the kist launched again; with none alive, NOTHING and nothing launched.
pub fn spawn_kist(grants: u64, kists: u64, name: &[u8], capsule: &[u8; 32], codes: &[u8], replace: bool) -> u64 {
    let mut req = [0u8; 1 + 64 + 32 + 32 + 1 + 64];
    assert!(name.len() <= 64 && codes.len() <= 64, "spawn_kist: a name or a list of slots longer than a request holds");
    let mut at = 0;
    req[at] = name.len() as u8;
    req[at + 1..at + 1 + name.len()].copy_from_slice(name);
    at += 1 + name.len();
    req[at..at + 32].copy_from_slice(capsule);
    at += 32;
    let tok = (arc_base() + cap(grants, kists)) as *const u8;
    for i in 0..32 {
        req[at + i] = unsafe { core::ptr::read_volatile(tok.add(i)) };
    }
    at += 32;
    req[at] = codes.len() as u8;
    req[at + 1..at + 1 + codes.len()].copy_from_slice(codes);
    at += 1 + codes.len();
    svc(nr::SPAWN, off(req.as_ptr()), at as u64, 2 | (replace as u64) << 2, 0, 0)
}

/// End this process; its arc and capabilities cease to exist.
pub fn exit(code: u64) -> ! {
    svc(nr::EXIT, code, 0, 0, 0, 0);
    loop {}
}

/// Call an endpoint (or the console): `msg` goes to the server, its reply lands in `reply`; returns the reply length (ERR on a bad token).
pub fn call(cap: u64, msg: &[u8], reply: &mut [u8]) -> u64 {
    svc(nr::CALL, cap, off(msg.as_ptr()), msg.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64)
}

/// `call` that fails with ERR at once when no server is receiving on the endpoint (`CALL_LIVE`), instead of waiting for one: for a caller that must not hang on a driver that is down or still bringing its device up.
pub fn call_live(cap: u64, msg: &[u8], reply: &mut [u8]) -> u64 {
    svc(nr::CALL, cap, off(msg.as_ptr()), msg.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64 | CALL_LIVE)
}

/// Receive on an endpoint: the message lands in `buf`, a single-use reply token in `token`; returns the message length. With `poll`, returns NOTHING at once when no message waits.
pub fn recv(cap: u64, buf: &mut [u8], token: &mut [u8; 32], poll: bool) -> u64 {
    svc(nr::RECV, cap, off(buf.as_mut_ptr()), buf.len() as u64, off(token.as_mut_ptr()), poll as u64)
}

/// A new thread of this process: `entry(arg)` on `stack` (its top is the thread's SP; give it 16 KiB or more), runnable at once while the caller carries on. Threads share the arc and every capability; they are not addressable (KERNEL.md "Threads, interrupts and cores"). Returns 0 or ERR.
pub fn thread_spawn(entry: extern "C" fn(u64) -> !, stack: &mut [u8], arg: u64) -> u64 {
    let top = unsafe { stack.as_mut_ptr().add(stack.len()) };
    svc(nr::THREAD_SPAWN, off(entry as usize as *const u8), off(top), arg, 0, 0)
}

/// This thread ends with `value`; the last thread of a process ends the process.
pub fn thread_exit(value: u64) -> ! {
    svc(nr::THREAD_EXIT, value, 0, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

/// Block while `word` still holds `expected`, until another thread of this process calls `wake` on it (0), or the counter reaches `deadline` (2; 0 = none); 1 at once if the word had already changed. A futex with a timeout: the word is the state, this is the wait, and a server loop waits for a request or its next deadline in one call.
pub fn wait(word: &core::sync::atomic::AtomicU64, expected: u64, deadline: u64) -> u64 {
    svc(nr::WAIT, off(word as *const _ as *const u8), expected, deadline, 0, 0)
}

/// `wait` for something the world brings (traffic, credentials, a person): the process watchdog's clock is paused while it waits (`pet`). A wait on this process's own work or its own hardware is a plain `wait`, and counts.
pub fn wait_world(word: &core::sync::atomic::AtomicU64, expected: u64, deadline: u64) -> u64 {
    svc(nr::WAIT, off(word as *const _ as *const u8), expected, deadline, 1, 0)
}

/// Make up to `count` threads waiting on `word` runnable; returns how many.
pub fn wake(word: &core::sync::atomic::AtomicU64, count: u64) -> u64 {
    svc(nr::WAKE, off(word as *const _ as *const u8), count, 0, 0, 0)
}

/// Block until the interrupt line the token grants fires (0), or the counter reaches `deadline` (2; 0 = none). A line that fired while no one waited answers at once. The line stays masked until `irq_ack`; clear the device's own pending bit before acking.
pub fn irq_wait(cap: u64, deadline: u64) -> u64 {
    svc(nr::IRQ_WAIT, cap, deadline, 0, 0, 0)
}

/// `irq_wait` for a line driven by the world (a person, the network, a peer): the process watchdog's clock is paused while it waits (`pet`). A line from hardware this driver itself asked for (a transfer done, a frame) is a plain `irq_wait`, and counts.
pub fn irq_wait_world(cap: u64, deadline: u64) -> u64 {
    svc(nr::IRQ_WAIT, cap, deadline, 1, 0, 0)
}

/// The device is handled: unmask its line.
pub fn irq_ack(cap: u64) -> u64 {
    svc(nr::IRQ_ACK, cap, 0, 0, 0, 0)
}

/// The cores this thread may run on from now (`CORES_*`; 0 = the workers). Threads it spawns later inherit it.
pub fn pin(mask: u64) -> u64 {
    svc(nr::PIN, mask, 0, 0, 0, 0)
}

/// Answer a call; the token stops existing.
pub fn reply(token: &[u8; 32], msg: &[u8]) -> u64 {
    svc(nr::REPLY, off(token.as_ptr()), off(msg.as_ptr()), msg.len() as u64, 0, 0)
}

/// Read a 32-bit register of a hardware grant (ERR if the offset is unaligned or outside it).
pub fn mmio_read(cap: u64, offset: u64) -> u64 {
    svc(nr::MMIO_READ, cap, offset, 0, 0, 0)
}

/// Write a 32-bit register of a hardware grant.
pub fn mmio_write(cap: u64, offset: u64, value: u32) -> u64 {
    svc(nr::MMIO_WRITE, cap, offset, value as u64, 0, 0)
}

/// Fill `buf` (≤ 4096 bytes) with randomness from the session's wairua; returns its length, or ERR without the entropy right or a wairua.
pub fn entropy(cap: u64, buf: &mut [u8]) -> u64 {
    svc(nr::ENTROPY, cap, off(buf.as_mut_ptr()), buf.len() as u64, 0, 0)
}

/// The physical base of a DMA region grant (non-cacheable memory the process reads and writes directly, its device's only DMA window).
pub fn dma(cap: u64) -> u64 {
    svc(nr::DMA, cap, 0, 0, 0, 0)
}

/// Copy spawn input into `buf`, starting at byte `off`; returns how many bytes (0 at the end). Large one-shot input (firmware) is streamed this way so the arc stays small.
pub fn input(start: u64, buf: &mut [u8]) -> usize {
    svc(nr::INPUT, start, off(buf.as_mut_ptr() as *const u8), buf.len() as u64, 0, 0) as usize
}

/// Sleep until the generic timer count reaches `deadline` (see `now`/`freq`): the process is parked, not scheduled, until then.
pub fn sleep_until(deadline: u64) {
    svc(nr::SLEEP, deadline, 0, 0, 0, 0);
}

/// Say something on the console (grant slot 0).
pub fn console(grants: u64, text: &[u8]) {
    let mut r = [0u8; 16];
    call(cap(grants, 0), text, &mut r);
}

/// What a payload's panic handler calls: exit with G#DEAD (the kernel prints `name: exit G#…DEAD`).
pub fn panic_exit() -> ! {
    exit(0xDEAD)
}

/// The performance monitors, which the kernel opens to EL0 on every core (ferros_kernel/src/pmu.rs). Core-wide: counts include whatever else ran on the core, so a measurement wants the process pinned and the loop long against a slice. Event counters are 32 bits.
pub mod pmu {
    /// Common ARMv8 events (PMEVTYPER evtCount).
    pub const SW_INCR: u32 = 0x00;
    pub const L1D_CACHE_REFILL: u32 = 0x03;
    pub const INST_RETIRED: u32 = 0x08;
    pub const BR_MIS_PRED: u32 = 0x10;
    pub const CPU_CYCLES: u32 = 0x11;
    pub const STALL_FRONTEND: u32 = 0x23;
    pub const STALL_BACKEND: u32 = 0x24;
    pub const ASE_SPEC: u32 = 0x74;

    /// The cycle counter (64 bits; counts EL0, EL1 and EL2).
    #[inline(always)]
    pub fn cycles() -> u64 {
        let v: u64;
        unsafe { core::arch::asm!("isb", "mrs {}, pmccntr_el0", out(reg) v, options(nomem, nostack)) };
        v
    }

    /// How many event counters this core has (PMCR_EL0.N).
    pub fn counters() -> u32 {
        let v: u64;
        unsafe { core::arch::asm!("mrs {}, pmcr_el0", out(reg) v, options(nomem, nostack)) };
        ((v >> 11) & 0x1F) as u32
    }

    /// Event counter `n` counts `event` at EL0 only (PMEVTYPER: P set, U and NSH clear), from zero.
    pub fn program(n: u32, event: u32) {
        unsafe {
            core::arch::asm!(
                "msr pmselr_el0, {n}",
                "isb",
                "msr pmxevtyper_el0, {t}",
                "msr pmxevcntr_el0, xzr",
                "isb",
                n = in(reg) n as u64,
                t = in(reg) (1u64 << 31) | event as u64,
                options(nostack),
            )
        };
    }

    /// Event counter `n`'s count.
    #[inline(always)]
    pub fn read(n: u32) -> u64 {
        let v: u64;
        unsafe { core::arch::asm!("msr pmselr_el0, {n}", "isb", "mrs {v}, pmxevcntr_el0", n = in(reg) n as u64, v = out(reg) v, options(nostack)) };
        v
    }
}

/// The look the shell draws in: every colour the display server and the map choose (as VSF RGB gamma 2 codes, G#RRGGBB — the panel's own correction, the ODT, is not a theme's business) and the glow's strength. Device preference 5 picks one (`device.theme`; settings flow from the identity to the device, SETTINGS.md). The first is the grey the shell was built in.
pub mod theme {
    #[derive(Clone, Copy)]
    pub struct Theme {
        pub name: &'static str,
        /// A circle without an icon, and the clock's face.
        pub plane: u32,
        /// The glow of an ordinary circle.
        pub ring: u32,
        /// What changed since last seen: a lit circle's fill and glow (the map's only colour).
        pub lit: u32,
        /// A radio's and a checkbox's outline and fill: white in the default theme (owner, 2026-11-08: "white for the checkbox/radial too, no amber, unless of course the theme wants something else but that's for the non default"); the others their own text colour (night stays red).
        pub mark: u32,
        /// Text: primary, secondary, faint.
        pub text: u32,
        pub text2: u32,
        pub text3: u32,
        /// The clock: a ring's swept fill, the hour ring's fill after noon (decimal's AM/PM), the strokes between rings, a tick still ahead of the fill, a tick the fill has passed.
        pub clock_fill: u32,
        pub clock_fill_pm: u32,
        pub clock_stroke: u32,
        pub clock_tick: u32,
        pub clock_tick_passed: u32,
        /// The glow's brightness one pixel out, by importance (none, ordinary, heavy).
        pub glow_peak: [u32; 3],
    }

    pub const THEMES: [Theme; 4] = [
        Theme { name: "grey", plane: 0x2A2A2A, ring: 0x8A8A8A, lit: 0xFFA000, mark: 0xFFFFFF, text: 0xF0F0F0, text2: 0x9A9A9A, text3: 0x5C5C5C, clock_fill: 0x707070, clock_fill_pm: 0x9C9C9C, clock_stroke: 0, clock_tick: 0xFFFFFF, clock_tick_passed: 0, glow_peak: [0, 0x77, 0xFF] },
        Theme { name: "slate", plane: 0x222830, ring: 0x7A8CA0, lit: 0x3CC8FF, mark: 0xE8F0F8, text: 0xE8F0F8, text2: 0x8EA0B4, text3: 0x506070, clock_fill: 0x5A6C80, clock_fill_pm: 0x8AA0B8, clock_stroke: 0, clock_tick: 0xE8F0FF, clock_tick_passed: 0, glow_peak: [0, 0x77, 0xFF] },
        Theme { name: "ember", plane: 0x2C241E, ring: 0xA08060, lit: 0xFF7A30, mark: 0xF4E8DC, text: 0xF4E8DC, text2: 0xB09880, text3: 0x6A5848, clock_fill: 0x7C6450, clock_fill_pm: 0xA88C70, clock_stroke: 0, clock_tick: 0xFFF0E0, clock_tick_passed: 0, glow_peak: [0, 0x77, 0xFF] },
        Theme { name: "night", plane: 0x1E0806, ring: 0x802018, lit: 0xFF2810, mark: 0xD04030, text: 0xD04030, text2: 0x902818, text3: 0x501008, clock_fill: 0x601810, clock_fill_pm: 0x902418, clock_stroke: 0, clock_tick: 0xC03020, clock_tick_passed: 0, glow_peak: [0, 0x50, 0xB0] },
    ];

    /// The theme the device preference names now.
    pub fn current() -> &'static Theme {
        &THEMES[(super::pref(super::PREF_THEME) as usize).min(THEMES.len() - 1)]
    }

    /// Its index (part of anything cached that a theme colours).
    pub fn index() -> usize {
        (super::pref(super::PREF_THEME) as usize).min(THEMES.len() - 1)
    }
}

/// A window on the display server (WINDOWS.md): an app draws its own full-screen surface (the `surface` it was given at spawn; 64-bit pixels: R, G, B working values in bits 0-15, 16-31, 32-47, bit 48 no dither, bit 63 see-through) and the display composes the windows frontmost first, inside the circle tangent to the screen's top and bottom.
pub mod window {
    use super::call;

    /// Window levels: an app, the keyboard, the system (attest). Higher is in front.
    pub const APP: u8 = 0;
    pub const INPUT: u8 = 1;
    pub const SYSTEM: u8 = 2;
    /// A surface pixel that shows what is behind it.
    pub const CLEAR: u64 = 1 << 63;
    /// The bytes before a surface's pixel 0 (the kernel's SURFACE_PORCH): `dma` answers a surface past them, and a process that reaches its surface through its handle adds them to every offset.
    pub const PORCH: u64 = 64;

    /// Show (or redraw) the window: the part of the screen it covers, its surface's base, its level. True when the display took it.
    pub fn show(display: u64, base: u64, x: u16, y: u16, w: u16, h: u16, level: u8) -> bool {
        show_as(display, base, x, y, w, h, level, WORKING)
    }

    /// A surface of the display's own working values (gamma 2, white = Q14, bit 63 see-through).
    pub const WORKING: u8 = 0;
    /// A surface of fluor's 16-bit pixels (fluor `pixel::G2t16`: α and darkness of VSF `g2t` codes), which the display decodes itself.
    pub const G2T16: u8 = 1;

    /// As `show`, saying what the surface's pixels are (`WORKING` or `G2T16`).
    pub fn show_as(display: u64, base: u64, x: u16, y: u16, w: u16, h: u16, level: u8, format: u8) -> bool {
        let mut m = [0u8; 19];
        m[0] = 22;
        m[1..3].copy_from_slice(&x.to_le_bytes());
        m[3..5].copy_from_slice(&y.to_le_bytes());
        m[5..7].copy_from_slice(&w.to_le_bytes());
        m[7..9].copy_from_slice(&h.to_le_bytes());
        m[9..17].copy_from_slice(&base.to_le_bytes());
        m[17] = level;
        m[18] = format;
        let mut r = [0u8; 4];
        call(display, &m, &mut r);
        r[0] == 0
    }

    /// As `show_as`, saying that only (cx, cy, cw, ch) of the surface changed since the window's last show: the display publishes that part alone, where a plain show republishes the whole screen in every dither phase (2026-11-01: the attest screen's blinkey cost four full-screen publishes a tick).
    #[allow(clippy::too_many_arguments)]
    pub fn show_changed(display: u64, base: u64, x: u16, y: u16, w: u16, h: u16, level: u8, format: u8, changed: (u16, u16, u16, u16)) -> bool {
        let mut m = [0u8; 27];
        m[0] = 22;
        m[1..3].copy_from_slice(&x.to_le_bytes());
        m[3..5].copy_from_slice(&y.to_le_bytes());
        m[5..7].copy_from_slice(&w.to_le_bytes());
        m[7..9].copy_from_slice(&h.to_le_bytes());
        m[9..17].copy_from_slice(&base.to_le_bytes());
        m[17] = level;
        m[18] = format;
        m[19..21].copy_from_slice(&changed.0.to_le_bytes());
        m[21..23].copy_from_slice(&changed.1.to_le_bytes());
        m[23..25].copy_from_slice(&changed.2.to_le_bytes());
        m[25..27].copy_from_slice(&changed.3.to_le_bytes());
        let mut r = [0u8; 4];
        call(display, &m, &mut r);
        r[0] == 0
    }

    /// A window's mode byte: bits 0-2 the blend mode, how the window sits on what is beneath it, in fluor's order; bit 3 `OPAQUE`, a promise that no pixel of the surface is see-through, so the display never reads what the window covers.
    pub const NORMAL: u8 = 0;
    pub const MULTIPLY: u8 = 1;
    pub const SCREEN: u8 = 2;
    pub const ADD: u8 = 3;
    pub const SUBTRACT: u8 = 4;
    pub const OVERLAY: u8 = 5;
    pub const DARKEN: u8 = 6;
    pub const LIGHTEN: u8 = 7;
    pub const OPAQUE: u8 = 8;

    /// As `show_changed`, with the window's mode byte (a blend mode, `OPAQUE`, or both); `changed` is the whole window when nothing narrower is known.
    #[allow(clippy::too_many_arguments)]
    pub fn show_full(display: u64, base: u64, x: u16, y: u16, w: u16, h: u16, level: u8, format: u8, changed: (u16, u16, u16, u16), mode: u8) -> bool {
        let mut m = [0u8; 28];
        m[0] = 22;
        m[1..3].copy_from_slice(&x.to_le_bytes());
        m[3..5].copy_from_slice(&y.to_le_bytes());
        m[5..7].copy_from_slice(&w.to_le_bytes());
        m[7..9].copy_from_slice(&h.to_le_bytes());
        m[9..17].copy_from_slice(&base.to_le_bytes());
        m[17] = level;
        m[18] = format;
        m[19..21].copy_from_slice(&changed.0.to_le_bytes());
        m[21..23].copy_from_slice(&changed.1.to_le_bytes());
        m[23..25].copy_from_slice(&changed.2.to_le_bytes());
        m[25..27].copy_from_slice(&changed.3.to_le_bytes());
        m[27] = mode;
        let mut r = [0u8; 4];
        call(display, &m, &mut r);
        r[0] == 0
    }

    /// The window is done: what is behind it shows.
    pub fn done(display: u64, base: u64) {
        let mut m = [0u8; 9];
        m[0] = 23;
        m[1..9].copy_from_slice(&base.to_le_bytes());
        let mut r = [0u8; 4];
        call(display, &m, &mut r);
    }

    /// A line of text into the surface (the display's typeface): screen coordinates, size in pixels, grey value Q14 (white 16384).
    pub fn text(display: u64, base: u64, x: i16, baseline: i16, px: u16, value: u16, s: &[u8]) {
        text_op(26, display, base, x, baseline, px, value, s);
    }

    /// As `text`, centred across `cx` by its ink: the middle of the pixels the glyphs draw, left to right, not of their advances, so a narrow letter (i, l) or one with uneven side bearings sits in the middle of its key (owner, 2026-11-01: left and right by the ink box, not up and down). Display op 29.
    pub fn text_centred(display: u64, base: u64, cx: i16, baseline: i16, px: u16, value: u16, s: &[u8]) {
        text_op(29, display, base, cx, baseline, px, value, s);
    }

    fn text_op(op: u8, display: u64, base: u64, x: i16, baseline: i16, px: u16, value: u16, s: &[u8]) {
        let mut m = [0u8; 256];
        m[0] = op;
        m[1..9].copy_from_slice(&base.to_le_bytes());
        m[9..11].copy_from_slice(&x.to_le_bytes());
        m[11..13].copy_from_slice(&baseline.to_le_bytes());
        m[13..15].copy_from_slice(&px.to_le_bytes());
        m[15..17].copy_from_slice(&value.to_le_bytes());
        let n = s.len().min(m.len() - 17);
        m[17..17 + n].copy_from_slice(&s[..n]);
        let mut r = [0u8; 4];
        call(display, &m[..17 + n], &mut r);
    }

    /// The advance of `s` at `px` pixels in the display's typeface, in quarter pixels: where text drawn by `text` ends, so a caret sits on it and a tap finds its character. None when the display does not answer (an older display server).
    pub fn width(display: u64, px: u16, s: &[u8]) -> Option<u32> {
        let mut m = [0u8; 256];
        m[0] = 28;
        m[1..3].copy_from_slice(&px.to_le_bytes());
        let n = s.len().min(m.len() - 3);
        m[3..3 + n].copy_from_slice(&s[..n]);
        let mut r = [0u8; 8];
        let got = call(display, &m[..3 + n], &mut r);
        (got >= 5 && got != super::ERR && r[0] == 0).then(|| u32::from_le_bytes([r[1], r[2], r[3], r[4]]))
    }

    /// Whether the screen point belongs to this window: the frontmost window covering it, inside the circle. Ask on each finger down; a finger that lands on another window (or in a corner) is not this app's.
    pub fn mine(display: u64, base: u64, x: u16, y: u16) -> bool {
        let mut m = [0u8; 13];
        m[0] = 27;
        m[1..9].copy_from_slice(&base.to_le_bytes());
        m[9..11].copy_from_slice(&x.to_le_bytes());
        m[11..13].copy_from_slice(&y.to_le_bytes());
        let mut r = [0u8; 4];
        call(display, &m, &mut r) >= 1 && r[0] == 1
    }

    /// The circle an app lives in: its centre and radius in pixels, and how many rows at the top and bottom reach the corners outside it (the circle is tangent to the screen's top and bottom).
    pub fn circle(w: usize, h: usize) -> (usize, usize, usize, usize) {
        let (cx, r) = (w / 2, h / 2);
        let chord = (r * r - cx * cx).isqrt();
        (cx, h / 2, r, r - chord + 1)
    }
}

/// Whether an identity is unlocked (AUTH.md), through an Unlock capability. None without the capability.
pub fn identity_unlocked(cap: u64) -> Option<bool> {
    let r = svc(nr::UNLOCK, cap, 0, 0, 0, 0);
    (r != ERR).then_some(r & 1 == 1)
}

/// Whether an identity's fleet has bound this device (only that identity unlocks it then). None without the capability.
pub fn identity_bound(cap: u64) -> Option<bool> {
    let r = svc(nr::UNLOCK, cap, 0, 0, 0, 0);
    (r != ERR).then_some(r & 2 == 2)
}

/// What an unlock did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unlock {
    /// The identity's space is open; how many entries it has on this device.
    Opened(u64),
    /// The device is bound to another identity: this one does not open here.
    Belongs,
    /// No Unlock right, no vault or no ira.
    Failed,
}

/// Unlock the person's identity class of the vault from their 32-byte seed (AUTH.md): the kernel derives the key from the ira, the device-only secret and the seed, and keeps it until power goes or the device switches OS. A device bound to another identity refuses.
pub fn unlock_identity(cap: u64, seed: &[u8; 32]) -> Unlock {
    match svc(nr::UNLOCK, cap, 1, off(seed.as_ptr()), 0, 0) {
        ERR => Unlock::Failed,
        NOTHING => Unlock::Belongs,
        n => Unlock::Opened(n),
    }
}

/// The unlocked identity's fleet has bound this device: from now on only it unlocks here. True when done.
pub fn bind_identity(cap: u64) -> bool {
    svc(nr::UNLOCK, cap, 3, 0, 0, 0) == 0
}

/// Release the device (a completed departure): every identity entry deleted, the binding gone. True when done.
pub fn release_identity(cap: u64) -> bool {
    svc(nr::UNLOCK, cap, 4, 0, 0, 0) == 0
}

/// The unlocked identity's own entries on this device as one VSF document (section `identity_export`, an `entry` field per vault name: the name and its stored bytes), written into `buf`, for saving them to the fleet before a departure. The document's length, or None while locked or when it does not fit. Read a transfer at a time from offsets (op 5's x4): the document is built at offset 0 and kept until read to its end, and each call answers one transfer (4096 bytes) at most.
pub fn export_identity(cap: u64, buf: &mut [u8]) -> Option<usize> {
    let mut at = 0;
    loop {
        // One transfer a call: what the kernel copies back, so `at` knows how far each call wrote.
        let piece = (buf.len() - at).min(1 << 12);
        let whole = match svc(nr::UNLOCK, cap, 5, off(buf[at..].as_ptr()), piece as u64, at as u64) {
            ERR => return None,
            n => n as usize,
        };
        if whole > buf.len() {
            return None;
        }
        at += piece.min(whole - at);
        if at >= whole {
            return Some(whole);
        }
    }
}

/// A granted region reached through its handle (SANDBOX.md, "Handles"): slot `SLOT`'s pair in the handle table at the arc's start (its base and its mask, written by the kernel, read-only to the process), every access `base + (offset & mask)` in exactly the instruction forms the sandbox checker admits: the pair load `ldp x23, x22, [x28, #16·SLOT]`, the masked offset `and x21, off, x22`, the access `[x23, x21]`. Native and masked code run the same instructions; a masked process can reach a granted region no other way. An offset past the region wraps inside it. A grant with no region (an endpoint, a right) has a zero pair: every access lands on address 0 and faults, which is the right answer.
pub struct Handle<const SLOT: u64>;

impl<const SLOT: u64> Handle<SLOT> {
    /// The region's base and mask as the table holds them.
    #[inline(always)]
    pub fn pair() -> (u64, u64) {
        let (b, m): (u64, u64);
        // Through the pair registers, the only destination the checker allows the table's load.
        unsafe { asm!("ldp x23, x22, [x28, #{t}]", "mov {b}, x23", "mov {m}, x22", b = out(reg) b, m = out(reg) m, t = const 16 * SLOT, options(nostack, readonly, preserves_flags)) };
        (b, m)
    }
    /// The region's size in bytes (0 for a grant with no region).
    #[inline(always)]
    pub fn len() -> u64 {
        let (b, m) = Self::pair();
        if b == 0 { 0 } else { m + 1 }
    }
    #[inline(always)]
    pub fn read8(off: u64) -> u8 {
        let v: u32;
        unsafe { asm!("ldp x23, x22, [x28, #{t}]", "and x21, {o}, x22", "ldrb {v:w}, [x23, x21]", o = in(reg) off, v = out(reg) v, t = const 16 * SLOT, options(nostack, readonly, preserves_flags)) };
        v as u8
    }
    #[inline(always)]
    pub fn read32(off: u64) -> u32 {
        let v: u32;
        unsafe { asm!("ldp x23, x22, [x28, #{t}]", "and x21, {o}, x22", "ldr {v:w}, [x23, x21]", o = in(reg) off, v = out(reg) v, t = const 16 * SLOT, options(nostack, readonly, preserves_flags)) };
        v
    }
    #[inline(always)]
    pub fn read64(off: u64) -> u64 {
        let v: u64;
        unsafe { asm!("ldp x23, x22, [x28, #{t}]", "and x21, {o}, x22", "ldr {v}, [x23, x21]", o = in(reg) off, v = out(reg) v, t = const 16 * SLOT, options(nostack, readonly, preserves_flags)) };
        v
    }
    #[inline(always)]
    pub fn write8(off: u64, v: u8) {
        unsafe { asm!("ldp x23, x22, [x28, #{t}]", "and x21, {o}, x22", "strb {v:w}, [x23, x21]", o = in(reg) off, v = in(reg) v as u32, t = const 16 * SLOT, options(nostack, preserves_flags)) };
    }
    #[inline(always)]
    pub fn write32(off: u64, v: u32) {
        unsafe { asm!("ldp x23, x22, [x28, #{t}]", "and x21, {o}, x22", "str {v:w}, [x23, x21]", o = in(reg) off, v = in(reg) v, t = const 16 * SLOT, options(nostack, preserves_flags)) };
    }
    #[inline(always)]
    pub fn write64(off: u64, v: u64) {
        unsafe { asm!("ldp x23, x22, [x28, #{t}]", "and x21, {o}, x22", "str {v}, [x23, x21]", o = in(reg) off, v = in(reg) v, t = const 16 * SLOT, options(nostack, preserves_flags)) };
    }
    /// `n` words of `v` from `off`, the pair selected once.
    #[inline(always)]
    pub fn fill64(off: u64, n: u64, v: u64) {
        unsafe {
            asm!(
                "ldp x23, x22, [x28, #{t}]",
                "cbz {n}, 2f",
                "1:",
                "and x21, {o}, x22",
                "str {v}, [x23, x21]",
                "add {o}, {o}, #8",
                "subs {n}, {n}, #1",
                "b.ne 1b",
                "2:",
                o = inout(reg) off => _, n = inout(reg) n => _, v = in(reg) v, t = const 16 * SLOT, options(nostack)
            )
        };
    }
}

impl<const SLOT: u64> Handle<SLOT> {
    /// `words` into the region from `off`: a frame's rows from a buffer in the arc to a surface, the pair selected once per word (the load from the arc is ordinary masked code).
    #[inline(always)]
    pub fn write_words(off: u64, words: &[u64]) {
        let mut o = off;
        for &w in words {
            Self::write64(o, w);
            o += 8;
        }
    }
    /// `words` out of the region from `off`.
    #[inline(always)]
    pub fn read_words(off: u64, words: &mut [u64]) {
        let mut o = off;
        for w in words.iter_mut() {
            *w = Self::read64(o);
            o += 8;
        }
    }
}

/// A payload's entry with ferros_rt's image header (process.rs `image_tail`): `b main`, a pad word, "ferrosRT" at offset 8, the zeroed tail (.bss + 16, payload.ld's `__ferros_tail`), three unused words and a heap of 0. The kernel sizes the process's arc from the flat image plus that tail and refuses an image without the header (2026-11-09: the display's and bt's stacks sat on their own statics because the kernel never counted their .bss). `main` is `extern "C" fn(grants: u64, input: u64, input_len: u64) -> !`.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        core::arch::global_asm!(
            r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 0
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad 0
    .size _entry, . - _entry
"#,
            main = sym $main
        );
    };
}
