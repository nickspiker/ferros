//! Text on the glass when nothing else can put it there (owner, 2026-11-17: "Do we have any text dump panic hook code to show something on screen if shit goes south?"): the glass console (ferros_glass) writing straight into the buffer the DPU scans.
//!
//! - **Which buffer:** the DPU's own registers say (`display::scanned`): the full-screen window's layer, its DMA base, stride and format.
//!   Before the display server runs that is ABL's splash (the kernel's painter switched it to G#00RRGGBB); after, the phase of the four temporal dither frames on the layer, and then all four are written (the server flips between them) in the panel's baked codes.
//! - **Three uses:** a kernel panic (`panic_screen`, then `hold`, then the existing report and reset, platform.rs); a process that died by a fault or a panic while no display serves (`notice` from process.rs `end_process`); a boot whose vault, usb or display is not up 16 s after init started (`boot_watch`).
//! - **Notices** are kept, newest last, in a fixed 2 KiB buffer under the kernel lock and drawn whole each time, the latest lines kept when they run past the bottom (`Keep::Latest`).
//! - **The panic's** text keeps its earliest lines (the headline and the message) when it runs past the bottom (`Keep::Earliest`).

use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed};
use core::sync::atomic::{AtomicBool, AtomicU64};

use ferros_glass::{Buf, Glass, Keep, Paint, Surface};

use crate::display::{Pixels, Scanned};

/// Colours as 8-bit (r, g, b) in the buffer's own codes, by `Paint` (ground, ink, accent): on the phases these are the panel's baked codes, where 255 is the panel's full white at its brightness.
type Colours = [(u32, u32, u32); 3];
/// The panic's: white on black, its headline red.
const PANIC: Colours = [(0, 0, 0), (0xE0, 0xE0, 0xE0), (0xFF, 0x40, 0x20)];
/// The notices': amber on black, the kernel console's colour (display.rs `show`).
const NOTICE: Colours = [(0, 0, 0), (0xFF, 0xB0, 0), (0xFF, 0xB0, 0)];

/// One 8-bit colour as the layer's word.
fn word(p: Pixels, (r, g, b): (u32, u32, u32)) -> u32 {
    match p {
        Pixels::Bgrx8 => r << 16 | g << 8 | b,
        Pixels::Xrgb8 => b << 24 | g << 16 | r << 8 | 0xFF,
        // A 10-bit code with its two low bits zero, as the display's present leaves them for the DQE (payloads/display `dither8`); alpha opaque.
        Pixels::Bgra10 => b << 24 | g << 14 | r << 4 | 3,
    }
}

/// The four temporal phases are 16 MiB apart from the region's start (payloads/display `PHASE_BASE`).
const PHASE_STEP: u64 = 1 << 24;

/// Where the glass console's spans go: the scanned buffer, or all four phases when it is one of them.
struct Out {
    s: Scanned,
    bases: [u64; 4],
    n: usize,
    words: [u32; 3],
}

impl Out {
    fn new(s: Scanned, colours: &Colours) -> Out {
        let (p0, p1) = crate::cachemap::DISPLAY_PHASES;
        let (p0, p1) = (p0 as u64, p1 as u64);
        let words = [word(s.pixels, colours[0]), word(s.pixels, colours[1]), word(s.pixels, colours[2])];
        let frame = (s.stride * s.height * 4) as u64;
        // The layer on one of the display server's phases (the region is physical, so only with the SYSMMU's translation off): the same place in all four, which it flips between.
        if !s.translates() && s.iova >= p0 && s.iova < p1 && (s.iova - p0) % PHASE_STEP + frame <= PHASE_STEP && p0 + 4 * PHASE_STEP <= p1 {
            let off = (s.iova - p0) % PHASE_STEP;
            let bases = [p0 + off, p0 + PHASE_STEP + off, p0 + 2 * PHASE_STEP + off, p0 + 3 * PHASE_STEP + off];
            Out { s, bases, n: 4, words }
        } else {
            Out { s, bases: [s.iova, 0, 0, 0], n: 1, words }
        }
    }
}

impl Surface for Out {
    fn span(&mut self, y: usize, x0: usize, x1: usize, paint: Paint) {
        let w = self.words[paint as usize];
        let len = ((x1 - x0) * 4) as u64;
        for &base in &self.bases[..self.n] {
            let row = base + ((y * self.s.stride + x0) * 4) as u64;
            let mut off = 0u64;
            while off < len {
                // A page at a time through the SYSMMU when it translates (its pages are 4 KiB at the least); the whole run at once when it does not.
                let a = row + off;
                let left = len - off;
                let n = if self.s.translates() {
                    let page = 0x1000 - (a & 0xFFF);
                    // WHY the smaller: the run ends at its own end or the page's, whichever comes first; the next page may map elsewhere.
                    if page < left { page } else { left }
                } else {
                    left
                };
                if let Some(p) = self.s.phys(a) {
                    for i in 0..n / 4 {
                        // SAFETY: `p` is the scanned buffer's own memory, by the DPU's base, stride and size (`display::scanned`) and the SYSMMU's walk: inside the panel's frame, DRAM the kernel maps one to one (Device for ABL's splash, non-cacheable for the phases).
                        unsafe { core::ptr::write_volatile((p + i * 4) as *mut u32, w) };
                    }
                }
                off += n;
            }
        }
    }
}

/// `text` on the glass in `colours`, from the top, `keep` deciding which end stays when it runs past the bottom; the frame sent at the panel's next TE. False when no buffer is on the glass to write (the DPU off, or a layer the console cannot write).
fn show(text: &[u8], keep: Keep, colours: &Colours) -> bool {
    let Some(s) = crate::display::scanned() else { return false };
    let Some(g) = Glass::new(s.width, s.height) else { return false };
    let mut out = Out::new(s, colours);
    ferros_glass::draw(&g, text, keep, &mut out);
    unsafe { core::arch::asm!("dsb sy") };
    s.arm();
    true
}

/// How long the panic screen holds before the reset when nobody presses power: 2^5 s.
/// WHY a deadline (a real one: AGENT.md, no timers otherwise): until the reset the device does nothing else, so the screen cannot wait forever for a hand that may never come (a device in a pocket must come back by itself); 32 s is long enough to pick the device up, read six lines twice and photograph them, and the report is kept for the next boot's DIAG whatever happens.
pub const HOLD_SHIFT: u32 = 5;

/// The kernel's panic on the glass: the headline, the message, where, the core and the last crumb, the kernel heap's last refusal if there was one, and how to go on.
/// From the panic handler only (platform.rs `report_panic`), after the report is in the ramoops block: no allocation, no lock, formatting into a fixed buffer.
/// False when nothing could be drawn.
pub fn panic_screen(info: &core::panic::PanicInfo, core: u64, crumb: u64) -> bool {
    use core::fmt::Write;
    let mut t = Buf::<{ 1 << 11 }>::new();
    let _ = write!(t, "ferros panic\n{}\n", info.message());
    if let Some(l) = info.location() {
        let _ = write!(t, "at {}:A#{}:A#{}\n", l.file(), l.line(), l.column());
    }
    let _ = write!(t, "core A#{core}, last crumb G#{crumb:X}\n");
    let fail = ferros_heap::kernel::LockedTlsf::last_fail();
    if fail.size != 0 {
        let _ = write!(t, "kernel heap refused A#{} bytes at alignment A#{} for G#{:X}; then A#{} in use, A#{} free in A#{} blocks, the largest A#{}\n", fail.size, fail.align, fail.caller, fail.in_use, fail.free, fail.free_blocks, fail.largest);
    }
    let _ = write!(t, "\nThe report is kept for the next boot's diag. Press power to restart now; it restarts by itself in A#{} s.\n", 1u32 << HOLD_SHIFT);
    show(t.bytes(), Keep::Earliest, &PANIC)
}

/// Keep the panic screen up: until a press of the power key (a key already down when the panic came must be let go first) or `HOLD_SHIFT`'s deadline, petting the watchdogs meanwhile (every other pet is stopped, so a hang anywhere still resets within the watchdog's second).
/// The key is read as its pin's level (gpa4-0, active low, on the always-on bank; killswitch.rs `power_held`) 64 times a second: no interrupt is needed, so it works whatever the panic left masked.
pub fn hold() {
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    let mut was = crate::killswitch::power_held();
    loop {
        crate::platform::pet_for_panic();
        let t = crate::platform::cntpct();
        while crate::platform::cntpct().wrapping_sub(t) < f >> 6 {
            core::hint::spin_loop();
        }
        let held = crate::killswitch::power_held();
        if (held && !was) || crate::platform::cntpct().wrapping_sub(t0) >= f << HOLD_SHIFT {
            return;
        }
        was = held;
    }
}

/// The kernel's notices since boot, newest last, one a line (2 KiB; the oldest lines dropped when a new one needs the room).
/// Under the kernel lock.
static mut NOTES: Buf<{ 1 << 11 }> = Buf::new();

/// A line for the glass: kept with the others, and all of them drawn when `glass` (the display not serving, or a boot stall), the latest kept when they run past the bottom.
/// Never over a panic's screen.
/// Under the kernel lock (process.rs `end_process`, `boot_watch`).
pub fn notice(line: &[u8], glass: bool) {
    let n = unsafe { &mut *(&raw mut NOTES) };
    let cap = n.buf.len();
    let need = n.n + line.len() + 1;
    if need > cap {
        // The oldest whole lines out: up to the first line end at or past the bytes needed.
        // WHY the smaller: a line longer than the whole buffer needs more than is kept; then everything kept goes (and `push` drops the line's tail past the end).
        let over = if need - cap < n.n { need - cap } else { n.n };
        let cut = n.bytes()[over..].iter().position(|&b| b == b'\n').map(|i| over + i + 1).unwrap_or(n.n);
        n.buf.copy_within(cut..n.n, 0);
        n.n -= cut;
    }
    n.push(line);
    n.push(b"\n");
    if glass && crate::GLASS_CORE.load(Acquire) == 0 {
        show(n.bytes(), Keep::Latest, &NOTICE);
    }
}

/// When init was started (the counter), 0 until it has: `boot_watch`'s start (service.rs `start_home`).
pub static INIT_AT: AtomicU64 = AtomicU64::new(0);
static WATCHED: AtomicBool = AtomicBool::new(false);
/// How long after init started the vault, usb and display must be up: 2^4 s.
/// WHY 16 s: press to init is 0.4 s and press to the first IPC 0.5 s (BOOT.md "Boot time"); the vault opens in tens of milliseconds, the display server's typeface in 30 ms and its first screen within the second, the host enumerates the usb process in 0.35 s.
/// Sixteen seconds is eight times a generous two, and still short of the time a person stares at a black screen before giving up (2026-11-17: a boot circle, then black, no USB, and nothing to say why).
pub const BOOT_DEADLINE_SHIFT: u32 = 4;

/// Once a service loop pass (it passes at least four times a second): when init has run `BOOT_DEADLINE_SHIFT` seconds, whether the vault, usb and display are up (each serving its endpoint, process.rs `up_locked`); those that are not are named on the console, in the flash log and on the glass, even over a serving display (whose own screen then is not working: the stall is the thing to see, and the display redraws over it as soon as what it shows changes).
/// Two loads and a compare until then, nothing after.
pub fn boot_watch() {
    let at = INIT_AT.load(Relaxed);
    if at == 0 || WATCHED.load(Relaxed) || crate::platform::cntpct().wrapping_sub(at) < crate::platform::cntfrq() << BOOT_DEADLINE_SHIFT {
        return;
    }
    WATCHED.store(true, Relaxed);
    let names: [&[u8]; 3] = [b"vault", b"usb", b"display"];
    let up = crate::process::with_lock(|| names.map(crate::process::up_locked));
    if up.iter().all(|&u| u) {
        return;
    }
    let mut line = alloc::string::String::from("boot stalled:");
    for (name, _) in names.iter().zip(up).filter(|(_, u)| !u) {
        line.push(' ');
        line.push_str(core::str::from_utf8(name).unwrap_or("?"));
    }
    line.push_str(&alloc::format!(" not up A#{} s after init started", 1u32 << BOOT_DEADLINE_SHIFT));
    crate::process::push_console(alloc::format!("{line}\n").as_bytes());
    crate::flashlog::line(&line);
    crate::process::with_lock(|| notice(line.as_bytes(), true));
}

/// Which core is drawing a terminal screen (a panic's), plus one; 0 none. `report_panic` claims it; the fault vector (main.rs `.Lexc_recover`) resets at once while it is set.
pub fn claim(core_plus_one: u64) -> Result<(), u64> {
    crate::GLASS_CORE.compare_exchange(0, core_plus_one, AcqRel, Acquire).map(|_| ())
}
