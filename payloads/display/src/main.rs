//! The display server, first cut (DISPLAY-DESIGN.md §3, SHELL.md): an EL0 process that owns the framebuffer and the base layer, paints the shell's first-boot screen with fluor and a real typeface, and presents it on the panel's TE through DECON's trigger — the frame loop that was the kernel's `display::show` until now, as a process.
//!
//! What it owns: the `display-fb` DMA region (the surface the layer DMA reads), a heap region, DECON0's main/win/wincon blocks, layer 0's DMA / DPP / SRAMC / HDR-common blocks, DSIM0 with its PHY, and the GPIO bank with the TE pin. What it takes from the pipe as found: the raster (the layer DMA's image size) and the modeset ABL or the kernel left — the panel, DSIM's link and DECON's own geometry stay as they are. The layer it programs itself, every word (`Pipe::program_layer`), so nothing about window 5 is inherited.
//!
//! Spawn: ferros-bridge spawn display.bin 00 --name display --serve display --hw decon0-main --hw decon0-win --hw decon0-wincon --hw dpu-dma0 --hw dpp0 --hw gpio-gpp0 --dma display-fb --dma display-heap --hw dsim0 --hw dphy0 --hw sramc0 --hw hdrcomm0
//! Grant slots, in that order: 0 console, 1 the `display` endpoint (serve), 2 DECON main, 3 DECON win, 4 DECON wincon, 5 DMA0, 6 DPP0, 7 GPP0, 8 the framebuffer region, 9 the heap region (cacheable; the arc pool is too small for a typeface and the framebuffer region too slow), 10 DSIM0, 11 its PHY (the link rate for the TE timers), 12 SRAMC0, 13 HDRCOMM0 (the layer's geometry words); then 14-22 below (the phases, the app surfaces, the frame-start line, the sprites, the canvas, and the modeset's blocks).
//!
//! The `display` endpoint (SHELL.md: the shell draws lists, this process paints). One op per call, first byte the op, coordinates u16 little-endian in panel pixels, colours u32 little-endian G#00RRGGBB, one status byte back (0 = ok, 1 = bad message, 2 = too big):
//! - 1 `begin`: a new frame — the whole surface transparent. Until a client's first `begin`, this process paints its own first-boot screen.
//! - 2 `text [x][y baseline][px][colour][align: 0 left, 1 centre at x, 2 right edge at x][utf-8…]`.
//! - 3 `rect [x][y][w][h][colour]` (w or h = 0: a one-pixel rule).
//! - 4 `present`: the frame goes to the panel (what nothing painted shows black). Reply byte 1 = whether DECON advanced.
//! - 5 `info` → `[w u16][h u16][frames u32][uptime s u32]`.
//! - 6 `brightness [dbv u16]`: the panel's DBV (196 = 2 nits … 3307 = 1000 nits; DISPLAY-HK3.md §4), through DSIM from this process.
//! - 7 `refresh [hz u8]`: 60 or 120, the panel's own command set plus DSIM's command-TE timers.
//! - 8 `idle [floor u8: 30, 10, 1, 0 = off]`: the panel's auto-rate floor.
//! - 9 `dcs [bytes…]`: one raw DCS write (dev). 10 `read [reg u8][len u8]` → the bytes (dev).
//! - 11 `testcard`: the highlight shoulder on the glass (DISPLAY-CALIB.md, DISPLAY-DESIGN.md §6): a log ramp of scene light from −6 to +3.3 stops; the top half through the max-brightness curve (encoded gamma2tanh value as the gamma-2 signal: white at 57 % of peak, highlights rolled by tanh), the bottom half linear with a hard clip at the same white; ticks at 1×, 2×, 4×, 8× white; everything through the panel's measured 2.185. Replaces the frame; `begin` takes it back.
//! - 12 `scene clear`: empty the map's retained scene (SHELL.md: the device is a map of your life).
//! - 13 `scene add [count u8][count x circle]`: append circles, each 30 bytes — `x i32`, `y i32` (world units), `r u16`, `ring u8` (0 none, 1 thin, 2 heavy — importance), `lit u8` (1 = changed since last seen, the only colour on the map), `icon u8` (n = the n-th icon in the embedded bundle, 0 = none), `len u8`, `label [16]`.
//! - 14 `camera [cx i32][cy i32][zoom u32, 16.16]`, the centre in world units ×256 (24.8, so a pan at high zoom moves by a pixel, not a world unit): draw the scene through the camera and present. Reply byte 1 = whether DECON advanced.
//!   Optional flag byte 13, bit 0: keep the frame's overlay (see 15). Optional bytes 14-17: the turn, `[cos i16][sin i16]` in Q14 (world to screen is that rotation, then the zoom); absent = upright.
//! - 15 `batch [len u16][op…]…`: several ops in one call, in order — a frame with an overlay is `begin`, its text and rects and discs, then `camera` with the keep flag, which draws the scene under them and presents. One call per frame rather than one per op: each call is a round trip through the scheduler. The reply is the last op's, its status the first failure's.
//! - 16 `disc [x i16][y i16][r u16][colour]`: a filled circle in panel pixels (the shell's shapes are circles; a ring is a smaller disc of the ground colour sent first, since what is sent first is on top).
//! - 17 `scene move [index u8][x i32][y i32][r u16]` (optionally `[cos i16][sin i16]`, the circle's own turn in Q14): one circle of the scene moved, resized and turned in place (the owner arranging by hand).
//! - 22 `app [x u16][y u16][w u16][h u16]`: the app in front's frame, its 16-bit pixels in the `app-surface` region (R, G, B in bits 0-15, 16-31, 32-47: working values, gamma 2, white = Q14; bit 48 = no dither), published at full precision; 23 `app done`: the screen is the map's again. Op 5's reply carries whether an app is in front (byte 12).
//! - 21 `ramps [column u8]`: the present stage's test screen, or one column of it alone on black between two thin white lines (dithered bands: our 4×4 to 8 bits, 12 effective; undithered: the 8-bit truncation), straight from 16-bit working values (see `ramps`); status 3 when the pipe is not in 10 bits.
//! - 24 `frame wait`: returns once the panel's next frame has started, or after a hundred-and-twentieth of a second when no frame is in flight (a command-mode panel starts a frame only when one is armed); the reply is the frame-start count, u32. A client that draws once per return is locked to the panel's refresh: it reads its input right after a frame starts and its update is ready for the next one (the map, while a finger is down).
//! The scene is retained here so a frame costs the client three numbers rather than one call per circle: nothing on the map ever moves, so between scene changes only the camera does.
//! fluor's order holds inside a frame: what a client sends first is on top.
//!
//! Pixels are BGRX8888 as the layer reads them (G#xxRRGGBB); fluor draws into the same words with colours packed as G#FF_RRGGBB — its α + darkness convention is only a convention, every blend it does is linear, and the X byte is ignored by the DMA — so there is no conversion pass. What is not a convention is fluor's order: it composites **front to back**, a paint goes *under* what is already there and an opaque pixel is left alone, so a frame starts from transparent (α = 0), the text goes down first and the background last. (The first cut filled black over raw DRAM: every word that happened to carry α = FF survived, a random-colour matrix at 1 in 256 — the owner's "green speckle wallpaper", 2026-10-16.)

#![no_std]
#![no_main]

extern crate alloc;

mod draw;
mod glyphs;
mod modeset;
mod neon;
mod odt;
use odt::{odt_tables, pow, PANEL_GAMMA};

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::{cap, console, freq, mmio_read, mmio_write, now};
use draw::Px;

/// The 16-bit canvas as the text and rectangle calls see it (they once drew on fluor's 8-bit canvas; the shapes of those calls are kept).
struct Canvas<'a> {
    pixels: &'a mut [Px],
    width: usize,
    height: usize,
}
impl<'a> Canvas<'a> {
    fn new(pixels: &'a mut [Px], width: usize, height: usize, _damage: &mut Damage) -> Canvas<'a> {
        Canvas { pixels, width, height }
    }
}
/// Kept for the call shape; the scene tracks its damage itself.
struct Damage;
impl Damage {
    fn new() -> Damage {
        Damage
    }
}
mod paint {
    use super::{draw, Canvas};
    /// A rectangle of `colour` (G#AARRGGBB, 8-bit codes; alpha its coverage) under what is there; a zero width or height is one pixel (the rules).
    pub fn fill_rect(c: &mut Canvas, x: isize, y: isize, w: isize, h: isize, colour: u32, _: Option<()>, _: Option<()>) {
        let (w, h) = (w.max(1), h.max(1));
        let (bx0, by0, bx1, by1) = draw::bounds(c.width, c.height);
        let px = draw::at_coverage(colour & 0x00FF_FFFF, colour >> 24);
        for yy in (y as i64).max(by0)..((y + h) as i64).min(by1) {
            for xx in (x as i64).max(bx0)..((x + w) as i64).min(bx1) {
                let p = &mut c.pixels[yy as usize * c.width + xx as usize];
                *p = draw::under(*p, px);
            }
        }
    }
}

const S_CONSOLE: u64 = 0;
const S_DISPLAY: u64 = 1;
const S_DECON: u64 = 2;
const S_WIN: u64 = 3;
const S_WINCON: u64 = 4;
const S_DMA0: u64 = 5;
const S_DPP0: u64 = 6;
const S_GPP0: u64 = 7;
const S_FB: u64 = 8;
const S_HEAP: u64 = 9;
const S_DSIM: u64 = 10;
const S_DPHY: u64 = 11;
const S_SRAMC0: u64 = 12;
const S_HDRCOMM0: u64 = 13;
/// 14: the `display-phases` region (64 MiB, non-cacheable): the four temporal dither frames.
const S_PHASES: u64 = 14;
/// 15: the `surfaces` region (128 MiB, cacheable): the four apps' surfaces, each a full screen of 64-bit pixels (WINDOWS.md).
const S_APPSURF: u64 = 15;
/// DECON0's frame-start interrupt line (kernel `decon0-frame-start`): the `vsync` thread waits on it and moves FRAME_STARTS, which `settle` and `present` wait on instead of polling FRAME_COUNT.
const S_IRQ_FS: u64 = 16;
/// 17: the `display-sprites` region (32 MiB, cacheable): the sprite arena. Absent (a display spawned by hand with fewer slots), the arena comes from the heap at its old 6 MiB.
const S_SPRITES: u64 = 17;
/// 18: the `display-canvas` region (32 MiB, cacheable): the 16-bit back buffer. Without it there is no canvas: the server says so and leaves.
const S_CANVAS: u64 = 18;
/// 19-22: what the boot modeset (modeset.rs) needs beyond the blocks above: DECON0's sub block (the DSC encoders, the DSIM interface select), its DQE, the DQE's CGC, the DPU sysreg (the D-PHY's resets).
const S_DECON_SUB: u64 = 19;
const S_DQE: u64 = 20;
const S_DQE_CGC: u64 = 21;
const S_DPU_SYSREG: u64 = 22;
/// Grant slot 23: the `dpu` power switch (init's 101; dpu_power.rs): the DPU powered up from off.
const S_DPU_POWER: u64 = 23;
/// The panel's switches (kernel panel_power.rs, grants 102 and 103): VCI and the reset pulse; VDDD after the init set.
const S_PANEL_POWER: u64 = 24;
const S_PANEL_VDDD: u64 = 25;
const INT_EN: u64 = 0x0060;
const INT_EN_FRAME_START: u32 = 1 << 12;
static FRAME_STARTS: AtomicU64 = AtomicU64::new(0);
/// The frame-start count, as `present`, `arm` and `settle` compare it (truncated: they look only for a change). They read DECON's FRAME_COUNT register until 2026-11-01, which on this pipe stays at 1: `settle` never saw its frame start, left the TE trigger unmasked until a later pass, and the panel re-sent the same frame at up to 120 Hz meanwhile — 68 frame starts a second for the attest screen's 12 presents, which held the kernel's clock policy in its active profile (it counts 24 a second as animating) and the device warm at rest.
fn frame_no() -> u32 {
    FRAME_STARTS.load(Ordering::Acquire) as u32
}
/// A frame armed (`arm`, `present`): the frame-start thread masks the trigger again when it starts.
static ONE_FRAME: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static mut VSYNC_STACK: [u8; 16384] = [0; 16384];

/// The frame-start thread: each start of a frame at DECON clears its pending bit, counts it and wakes whoever waits for it.
extern "C" fn vsync(grants: u64) -> ! {
    let (irq, decon) = (cap(grants, S_IRQ_FS), cap(grants, S_DECON));
    let mut said = 0u32;
    loop {
        let r = ferros_sys::irq_wait(irq, 0);
        // The first returns, and any error, on the console: a wait that comes back at once forever is a spinning thread that costs everyone their slices (2026-10-23).
        if (r != 0 && said < 4) || said < 2 {
            said += 1;
            let mut l = Log { grants, buf: [0; 256], n: 0 };
            l.s(b"display: vsync irq_wait -> ");
            l.hex(r as u32);
            l.s(if r == ferros_sys::ERR { b" (ERR: no such capability)\n" } else { b"\n" });
            l.flush();
            if r == ferros_sys::ERR {
                ferros_sys::thread_exit(1);
            }
        }
        // Everything pending is cleared (write-1-to-clear), not only frame-start: a stale frame-done bit left the line asserted and the loop ran 162,000 times a second (2026-10-23).
        let pend = ferros_sys::mmio_read(decon, INT_PEND) as u32;
        ferros_sys::mmio_write(decon, INT_PEND, pend);
        let extra = ferros_sys::mmio_read(decon, INT_PEND_EXTRA) as u32;
        if extra != 0 {
            ferros_sys::mmio_write(decon, INT_PEND_EXTRA, extra);
        }
        if pend & INT_EN_FRAME_START != 0 {
            // An armed frame has started: the TE trigger masked again at once, so the panel gets that one frame. Left to `settle` on the main loop's next pass (up to a sixteenth of a second later), the 120 Hz TE re-sent it meanwhile: about 7 frame starts per 10 Hz dither flip, 70 a second at rest, which held the kernel's clock policy in its active profile (it counts 24 a second as animating) and the device warm (2026-11-01).
            if ONE_FRAME.swap(false, Ordering::AcqRel) {
                let t = ferros_sys::mmio_read(decon, TRIG_CON) as u32;
                ferros_sys::mmio_write(decon, TRIG_CON, (t & !HW_TRIG_EN) | HW_TRIG_MASK);
            }
            FRAME_STARTS.fetch_add(1, Ordering::Release);
            ferros_sys::wake(&FRAME_STARTS, 4);
        }
        ferros_sys::irq_ack(irq);
    }
}

// ---------------------------------------------------------------- the panel thread
//
// The third thread (KERNEL.md "Servers: receive and work are different threads"): everything that talks to the panel over DSI and takes milliseconds — the brightness setting (DBV/HBM) and the screen off/on sequences, whose sleep-out needs 120 ms — runs here, so the render thread never stalls a frame on it. The render thread posts a request (a word each, a sequence bump, a wake) and carries on; screen-on ends with a re-present it asks the render thread for. The DSI link is shared with the dev ops the render thread still runs (6-10): one futex mutex orders them.
static PANEL_SEQ: AtomicU64 = AtomicU64::new(0);
/// The brightness wanted: u64::MAX none, else hbm << 16 | dbv.
static PANEL_BRIGHT: AtomicU64 = AtomicU64::new(u64::MAX);
/// The screen wanted: 0 on, 1 off.
static PANEL_SCREEN: AtomicU64 = AtomicU64::new(0);
/// The panel thread's "screen on again, present the frame the layer holds" to the render thread.
static PANEL_REPRESENT: AtomicU64 = AtomicU64::new(0);
static DSI_LOCK: AtomicU64 = AtomicU64::new(0);
/// The panel powered up from off (state B): display on (`29`) is the panel thread's once a frame has gone out, so the panel never shows the memory it woke with.
static PANEL_ON_DUE: AtomicU64 = AtomicU64::new(0);
static mut PANEL_STACK: [u8; 16384] = [0; 16384];

fn dsi_lock() {
    while DSI_LOCK.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
        ferros_sys::wait(&DSI_LOCK, 1, 0);
    }
}
fn dsi_unlock() {
    DSI_LOCK.store(0, Ordering::Release);
    ferros_sys::wake(&DSI_LOCK, 1);
}

extern "C" fn panel(grants: u64) -> ! {
    let dsi = ferros_dsi::Dsim(DsiGrant { grants });
    let mut screen_off = false;
    let mut seen = 0u64;
    if PANEL_ON_DUE.load(Ordering::Acquire) == 1 {
        // The first frame out (the frame-start interrupt's count), or a quarter second: display on, then the brightness with TE2, as after the power key.
        let t0 = now();
        while FRAME_STARTS.load(Ordering::Acquire) == 0 && now().wrapping_sub(t0) < freq() / 4 {
            ferros_sys::sleep_until(now() + freq() / 256);
        }
        dsi_lock();
        let on = dsi.dcs(&[0x29]).is_ok();
        let (_, _, hbm, dbv) = brightness(ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS));
        set_panel(&dsi, hbm, dbv);
        // The panel's own word on it (DCS get_power_mode, G#0A: bit 7 booster, 4 sleep out, 3 normal mode, 2 display on; G#9C when all is well), and the frames seen.
        let mut pm = [0u8; 1];
        let read = dsi.read(0x0A, &mut pm).is_ok();
        dsi_unlock();
        PANEL_ON_DUE.store(0, Ordering::Release);
        let mut l = Log { grants, buf: [0; 256], n: 0 };
        l.s(if on { b"display: panel on after its first frame; power mode " } else { b"display: panel display-on stuck; power mode " });
        if read { l.hex(pm[0] as u32) } else { l.s(b"unread") }
        l.s(b", frame starts ");
        l.dec(FRAME_STARTS.load(Ordering::Acquire));
        l.s(b"\n");
        l.flush();
    }
    loop {
        let seq = PANEL_SEQ.load(Ordering::Acquire);
        if seq == seen {
            ferros_sys::wait(&PANEL_SEQ, seq, 0);
            continue;
        }
        seen = seq;
        let b = PANEL_BRIGHT.swap(u64::MAX, Ordering::AcqRel);
        if b != u64::MAX {
            dsi_lock();
            set_panel(&dsi, b >> 16 & 1 == 1, b as u16);
            dsi_unlock();
        }
        let off = PANEL_SCREEN.load(Ordering::Acquire) == 1;
        if off != screen_off && dsi.alive() {
            screen_off = off;
            dsi_lock();
            if off {
                let _ = dsi.dcs(&[0x28]);
                let _ = dsi.dcs(&[0x10]);
            } else {
                let _ = dsi.dcs(&[0x11]);
                // The panel's sleep-out settling time before display on (DCS, 120 ms): this thread sleeps, the frames go on.
                ferros_sys::sleep_until(now() + freq() * 120 / 1000);
                let _ = dsi.dcs(&[0x29]);
                // The brightness again (preference 3), with TE2: the panel woke very dark (owner, 2026-11-01), its DBV/HBM sent only when the preference changed.
                let (_, _, hbm, dbv) = brightness(ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS));
                set_panel(&dsi, hbm, dbv);
                PANEL_REPRESENT.store(1, Ordering::Release);
            }
            dsi_unlock();
            let mut l = Log { grants, buf: [0; 256], n: 0 };
            l.s(if off { b"display: screen off\n" } else { b"display: screen on\n" });
            l.flush();
        }
    }
}

/// Wait for a frame to start — the interrupt, or half a millisecond, whichever first (the count register stays the truth; the fallback keeps a missed interrupt from costing more than it used to).
fn frame_wait() {
    let seen = FRAME_STARTS.load(Ordering::Acquire);
    ferros_sys::wait(&FRAME_STARTS, seen, now() + freq() / 2000);
}

/// DSIM0 through its grant: one syscall per register access, the generic timer for the waits.
struct DsiGrant {
    grants: u64,
}

impl ferros_dsi::Link for DsiGrant {
    fn rd(&self, off: usize) -> u32 {
        mmio_read(cap(self.grants, S_DSIM), off as u64) as u32
    }
    fn wr(&self, off: usize, v: u32) {
        mmio_write(cap(self.grants, S_DSIM), off as u64, v);
    }
    fn now(&self) -> u64 {
        now()
    }
    fn freq(&self) -> u64 {
        freq()
    }
}

// DECON main (cal_9865 regs-decon.h).
const TRIG_CON: u64 = 0x0030;
const HW_TRIG_EN: u32 = 1 << 0;
const HW_TRIG_MASK: u32 = 1 << 4;
const SHD_REG_UP_REQ: u64 = 0x0050;
const SHD_GLOBAL: u32 = 1 << 31;
const SHD_CMP: u32 = 1 << 20;
const INT_PEND: u64 = 0x0070;
const INT_PEND_EXTRA: u64 = 0x0074;
// Layer DMA (regs-dpp.h).
const RDMA_IRQ: u64 = 0x0004;
const RDMA_IN_CTRL_0: u64 = 0x0008;
const RDMA_SRC_WIDTH: u64 = 0x0010;
const RDMA_SRC_OFFSET: u64 = 0x0018;
const RDMA_IMG_SIZE: u64 = 0x001C;
const RDMA_BASEADDR_P0: u64 = 0x0040;
const IDMA_FORMAT_MASK: u32 = 0x3F << 8;
const IDMA_BGRX8888: u32 = 4 << 8;
const IDMA_ROT_MASK: u32 = 0x7 << 4;
const IDMA_COMP_MASK: u32 = 0xF;
/// RDMA_IN_CTRL_0's frame alpha (bits 24-31), and opaque.
const IDMA_ALPHA_MASK: u32 = 0xFF << 24;
const IDMA_ALPHA_OPAQUE: u32 = 0xFF << 24;
/// DPP_COM_IO_CON bit 7: the DPP takes the DMA's frame alpha.
const DPP_ALPHA_SEL: u32 = 1 << 7;
const IDMA_IRQ_CLEAR: u32 = 0xDFB << 16;
/// The TE pin: GPP0 DAT bit 3.
const GPP0_DAT: u64 = 0x0004;

/// The heap region (cachemap DISPLAY_HEAP): 32 MiB, cacheable. Its top holds the back buffer (see `_entry`); the rest is the allocator's. The typeface parse alone allocates 14.7 MiB in total (3.7 MiB live at the peak) and this allocator frees nothing until the frame arena resets, so what is left under the back buffer has to hold the whole parse.
const HEAP_REGION: usize = 32 << 20;

/// A bump allocator over the heap in the framebuffer region, with a mark: everything allocated before `mark()` (the parsed typeface) lives for the process; everything after it is the frame's scratch (glyph bitmaps), thrown away by `reset()` after each frame.
struct Bump {
    base: UnsafeCell<usize>,
    next: UnsafeCell<usize>,
    mark: UnsafeCell<usize>,
    end: UnsafeCell<usize>,
}
unsafe impl Sync for Bump {}

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let base = *self.base.get();
        if base <= 1 {
            return core::ptr::null_mut();
        }
        let start = (*self.next.get() + l.align() - 1) & !(l.align() - 1);
        if start + l.size() > *self.end.get() {
            return core::ptr::null_mut();
        }
        *self.next.get() = start + l.size();
        start as *mut u8
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}

impl Bump {
    fn init(&self, base: usize) {
        unsafe {
            *self.base.get() = base;
            *self.next.get() = base;
            *self.mark.get() = base;
            *self.end.get() = base + HEAP_REGION;
        }
    }
    fn mark(&self) {
        unsafe { *self.mark.get() = *self.next.get() };
    }
    fn reset(&self) {
        unsafe { *self.next.get() = *self.mark.get() };
    }
    fn used(&self) -> usize {
        unsafe { *self.next.get() - *self.base.get() }
    }
}

/// Non-zero initialisers keep these in `.data`: the flat image carries no `.bss`, and whatever the kernel copies after the code (the input) would otherwise share their bytes. `init` sets the real values.
#[global_allocator]
static ALLOC: Bump = Bump { base: UnsafeCell::new(1), next: UnsafeCell::new(1), mark: UnsafeCell::new(1), end: UnsafeCell::new(1) };

/// Oxanium (OFL, assets/fonts), the owner's choice for ferros's face — the same family as the "Ready?" screen — in its +glyphs cut, which adds tatau's twelve dozenal digit glyphs at G#10–G#1B, so numbers draw in the device base as glyphs (SETTINGS.md).
static FONT: &[u8] = include_bytes!("../../../assets/fonts/Oxanium-Regular+glyphs.ttf");

/// An opaque colour in the layer's own byte order, which fluor blends linearly like any other word.
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// One circle of the map (SHELL.md), in world units.
#[derive(Clone, Copy)]
struct Circle {
    x: i32,
    y: i32,
    r: u16,
    ring: u8,
    lit: u8,
    icon: u8,
    len: u8,
    label: [u8; 16],
    /// The circle's own turn (its icon), (cos, sin) in Q14, on top of the camera's: set by `scene move`.
    c: i16,
    s: i16,
}
const NO_CIRCLE: Circle = Circle { x: 0, y: 0, r: 0, ring: 0, lit: 0, icon: 0, len: 0, label: [0; 16], c: 16384, s: 0 };

/// The bundle's codes to canvas values, 16 bits: ICN3 bundles hold VSF RGB gamma2tanh codes (tanh(√L), bucket bottom c/256), the canvas gamma 2 working values with white at Q14 — so atanh(c/256) × 16384, highlights above white kept (to about 3× white at code 255) now the canvas has the headroom. ICN2 (gamma 2 codes, white at 255): c × 16384 / 255.
fn icon_decode() -> [u16; 256] {
    let g2t = ICONS.len() >= 4 && &ICONS[..4] == b"ICN3";
    let mut t = [0u16; 256];
    for (c, e) in t.iter_mut().enumerate() {
        let v = if g2t { libm::atanhf(c as f32 / 256.0) * 16384.0 + 0.5 } else { c as f32 * 16384.0 / 255.0 + 0.5 };
        *e = if v.is_nan() || v < 0.0 { 0 } else if v > 65535.0 { 65535 } else { v as u16 };
    }
    t
}

/// The apps' icons, bundled by tools/app-icons: b"ICN3" (gamma2tanh codes; ICN2: gamma 2), count u8, size u16 LE, then per icon size x size RGBA, already cut to a circle, followed by its size x size grey (photopic luminance), all VSF RGB gamma 2 codes. Empty when the build had no bundle.
static ICONS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icons.bin"));

/// The n-th icon (from 1): its RGBA, its grey, and its side, if the bundle has it.
fn icon(n: u8) -> Option<(&'static [u8], &'static [u8], usize)> {
    if n == 0 || ICONS.len() < 7 || !(&ICONS[..4] == b"ICN2" || &ICONS[..4] == b"ICN3") || n > ICONS[4] {
        return None;
    }
    let size = u16::from_le_bytes([ICONS[5], ICONS[6]]) as usize;
    let at = 7 + (n as usize - 1) * size * size * 5;
    let px = ICONS.get(at..at + size * size * 4)?;
    let grey = ICONS.get(at + size * size * 4..at + size * size * 5)?;
    Some((px, grey, size))
}
/// Scene icon ids at and above this are widgets the display draws itself, not bundle icons: G#80 the clock.
const WIDGET_CLOCK: u8 = 0x80;
/// The light scope (the map's icon G#82): the 16 kHz trace of one channel over the last second, and the four colour channels' levels in stops beneath.
const WIDGET_SCOPE: u8 = 0x82;
/// The fast trace, as SYS_LIGHT last gave it: up to four seconds at 16 kHz, and the call's header.
static mut SCOPE_BUF: [i32; 65536 + 8192 + 4] = [0; 65536 + 8192 + 4];
/// The slow channels' history: (counter ticks, counts per unit gain for the six channels), the last 64 spectral samples (~13 s at the AoC's ~5 a second).
static mut SLOW: ([(u64, [f32; 6]); 64], usize) = ([(0, [0.0; 6]); 64], 0);
static mut SLOW_SEQ: u64 = 0;
/// Where the scope was drawn and whether a frame holds it (as CLOCK_SHOWN / CLOCK_RECT for the clock).
static mut SCOPE_SHOWN: bool = false;
static mut SCOPE_RECT: draw::Rect = draw::Rect::EMPTY;
/// The part of the scope that changes between frames (its plot and labels), as the last draw placed it: what the redraws between frames clear and draw again.
static mut SCOPE_LIVE: draw::Rect = draw::Rect::EMPTY;
/// The time the scope shows, µs (display op 25: the focused scope's drag; a second by default).
static mut SCOPE_SPAN_US: u32 = 1_000_000;
/// The scope folded on its dominant period (display op 25's mode byte, a tap on the focused scope): the average cycle, twice, in place of the trace.
static mut SCOPE_FOLD: bool = false;
/// The widget in focus (display op 25; 0 none): it is redrawn and flipped at the panel's rate, everything else stays at the 10 Hz flips (owner, 2026-10-25).
static mut FOCUS: u8 = 0;
/// The scope's redraws between frames: total µs and count, logged every 256.
static mut SCOPE_US: [u64; 2] = [0; 2];
static mut SCOPE_DBG: [u64; 4] = [0; 4];
/// The spectral sensor's newest sample as this pass read it (SYS_FACTS 20–26): counts, gains, sample number.
static mut SPECTRAL_NOW: Option<([f32; 6], [f32; 6], u64)> = None;
/// The 60 directions (sin, −cos) of k/60 of a turn, Q14, for the clock face (built once at start).
static mut CLOCK_DIRS: [(i64, i64); 60] = [(0, 0); 60];
/// A clock is on the screen (the last frame drew one): the render thread checks each second whether its digits changed.
static mut CLOCK_SHOWN: bool = false;
/// The unit the last drawn clock showed (seconds of the day, or dozenal 50 s units): a redraw happens only when it changes.
static mut CLOCK_KEY: i64 = -1;
/// The screen rectangle the clocks of the last frame covered: a clock-only redraw publishes just this.
static mut CLOCK_RECT: draw::Rect = draw::Rect::EMPTY;
/// The frame being drawn is a clock refresh: publish only CLOCK_RECT (the camera has not moved, so nothing else changed).
static mut CLOCK_ONLY: bool = false;
/// The clock between frames: Some(k) while the last camera op is replayed with `draw::CLIP` set to the clock's rectangle, to publish that region alone into phase k (the one the next flip shows) — no clear beyond the clip, no frame armed, no interaction counted, the frame's damage left as the last full op made it.
static mut CLIP_PHASE: Option<usize> = None;
/// A between-frames redraw for every phase at once (non-zero: the phases' mask), in place of CLIP_PHASE's one: what is not in focus while something is, redrawn at 10 Hz into all four so the fast flips never show an older copy of it.
static mut CLIP_MASK: u32 = 0;
/// The last frame a client sent — a camera op alone, or a batch holding one (the map's: `begin`, its overlay, the camera with the keep flag) — as it came: the clock's redraws replay it.
static mut LAST_FRAME: ([u8; SLOT], usize) = ([0; SLOT], 0);

/// Whether a batch (op 15) holds a camera op.
fn batch_has_camera(m: &[u8]) -> bool {
    let mut at = 1usize;
    while at + 2 <= m.len() {
        let l = u16::from_le_bytes([m[at], m[at + 1]]) as usize;
        if at + 2 + l > m.len() {
            return false;
        }
        if m.get(at + 2) == Some(&14) {
            return true;
        }
        at += 2 + l;
    }
    false
}

/// Where the clock was last drawn: centre x, y, radius (Q8 px), its turn (Q14), ring count.
static mut CLOCK_GEOM: (i64, i64, i64, i64, i64, usize) = (0, 0, 0, 16384, 0, 3);
/// The clock as last drawn into the back buffer: each ring's sweep and fill, the digits' key.
#[derive(Clone, Copy, PartialEq)]
struct ClockShown {
    frac: [u32; 6],
    lighter: [bool; 6],
    key: i64,
    /// The gravity turn it was drawn at (GRAV_TURN; zero without one).
    turn: (i64, i64),
}
static mut BACK_CLOCK: ClockShown = ClockShown { frac: [0; 6], lighter: [false; 6], key: -1, turn: (0, 0) };

/// The clock upright by gravity (owner's order, 2026-10-24): the face's turn, Q14 (cos, sin), that puts its 12:00 and its digits up against gravity on the glass, whatever the map's own turn. None until the AoC has sent a sample (then the face turns with the map, as before).
static mut GRAV_TURN: Option<(i64, i64)> = None;
/// How far behind the device the clock's turn runs: gravity is evaluated this long before now, between the last two samples the AoC sent (their own timestamps), so every frame sees the device's attitude at the same lag whatever the frame and sample rates. One 10 ms sample period, plus the ~1 ms the samples take to arrive, plus margin: 16 ms.
const GRAV_LAG_MS: u64 = 16;

/// Gravity at `now − GRAV_LAG_MS`, the clock turned to it: no dead zone, no flat hold (owner, 2026-10-24) — the turn is whatever the device's tilt says, however small; drawn when a frame goes out (the 10 Hz flips at rest, every frame under a finger). Up on the glass is the reading's own (x, y): the sensor reports the reaction to gravity, which points away from the ground. In screen pixels (y down) that is (x, −y), and the face's 12:00, (0, −1) turned by θ clockwise, is (sin θ, −cos θ): so θ = atan2(x, y). Returns whether the turn changed.
fn gravity_poll() -> bool {
    let f = ferros_sys::facts();
    unsafe { SPECTRAL_NOW = f.spectral() };
    if let Some((c, g, seq)) = unsafe { SPECTRAL_NOW } {
        if seq != unsafe { SLOW_SEQ } {
            unsafe {
                SLOW_SEQ = seq;
                let h = &mut *(&raw mut SLOW);
                h.0[h.1 % 64] = (now(), core::array::from_fn(|i| if g[i] > 0.0 { c[i] / g[i] } else { 0.0 }));
                h.1 += 1;
            }
        }
    }
    let Some(((l, tl), (p, tp))) = f.gravity_pair() else { return false };
    let t = now().wrapping_sub(freq() * GRAV_LAG_MS / 1000);
    // Where t falls between the two samples (held at either end: a late sample holds the last, never extrapolates).
    let s = if tl > tp { ((t as i64 - tp as i64) as f32 / (tl - tp) as f32).clamp(0.0, 1.0) } else { 1.0 };
    let (x, y) = (p[0] + (l[0] - p[0]) * s, p[1] + (l[1] - p[1]) * s);
    if x == 0.0 && y == 0.0 {
        return false;
    }
    // To the face's own resolution: its edge is antialiased in 1/256 of a pixel, so at radius R pixels a turn under 1/(256 R) radians changes no pixel — and R in the clock's Q8 units is 256 R, so the step is 1/R(Q8). Not a dead zone: every turn the glass can show is drawn, at once; one it cannot show is not a frame.
    let r_q8 = (unsafe { CLOCK_GEOM }.2).max(256) as f32;
    let a = libm::roundf(libm::atan2f(x, y) * r_q8) / r_q8;
    let turn = ((libm::cosf(a) * 16384.0) as i64, (libm::sinf(a) * 16384.0) as i64);
    let changed = unsafe { GRAV_TURN } != Some(turn);
    unsafe { GRAV_TURN = Some(turn) };
    changed
}

/// The clock each phase frame holds (None: unknown, after a publish that was not the clock's — the next redraw for it is whole).
static mut PHASE_CLOCK: [Option<ClockShown>; PHASES] = [None; PHASES];

/// The parts of the clock that differ between `was` and `now`, each its own rectangle (redrawn one at a time, so a wedge on one side and the centre do not become one box across the face): per ring, the wedge its sweep edge moved through (the bounding box of the edge at both ends, and any quarter-turn extreme between, a few pixels wide); the centre when the digits changed. None when a ring wrapped, changed its fill, or moved more than an eighth of a turn: then the whole clock.
fn clock_dirty(was: &ClockShown, now: &ClockShown) -> Option<([draw::Rect; 8], usize)> {
    let (cx, cy, r, tc, ts, n) = unsafe { CLOCK_GEOM };
    let dirs = unsafe { &*(&raw const CLOCK_DIRS) };
    if was.turn != now.turn {
        return None;
    }
    let mut parts = [draw::Rect::EMPTY; 8];
    let mut np = 0;
    let pt = |rad: i64, (ux, uy): (i64, i64)| {
        let (vx, vy) = ((ux * tc - uy * ts) >> 14, (ux * ts + uy * tc) >> 14);
        ((cx + (vx * rad >> 14)) >> 8, (cy + (vy * rad >> 14)) >> 8)
    };
    for i in 0..n {
        let (a, b) = (was.frac[i], now.frac[i]);
        if a == b && was.lighter[i] == now.lighter[i] {
            continue;
        }
        if b < a || b - a > 1 << 13 || was.lighter[i] != now.lighter[i] {
            return None;
        }
        let (ro, ri) = draw::clock_ring_radii(r, n, i);
        let mut x0 = i64::MAX;
        let (mut y0, mut x1, mut y1) = (i64::MAX, i64::MIN, i64::MIN);
        let mut add = |(x, y): (i64, i64)| {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        };
        for f in [a, b] {
            let d = draw::clock_dir(dirs, f);
            add(pt(ro, d));
            add(pt(ri, d));
        }
        // A quarter turn between the two (in the face's frame) bulges the arc past its ends.
        for q in 1..4u32 {
            if a < q << 14 && b > q << 14 {
                add(pt(ro, draw::clock_dir(dirs, q << 14)));
            }
        }
        parts[np] = draw::Rect { x0: x0 as i32 - 3, y0: y0 as i32 - 3, x1: x1 as i32 + 4, y1: y1 as i32 + 4 };
        np += 1;
    }
    if was.key != now.key {
        let rc = draw::clock_centre_radius(r, n) >> 8;
        let (x, y) = (cx >> 8, cy >> 8);
        parts[np] = draw::Rect { x0: (x - rc - 2) as i32, y0: (y - rc - 2) as i32, x1: (x + rc + 3) as i32, y1: (y + rc + 3) as i32 };
        np += 1;
    }
    Some((parts, np))
}

/// The clock between frames: time spent and how many, logged every 1024 (~100 s at rest).
static mut CLIP_US: [u64; 2] = [0; 2];
/// Inside those redraws: the digits (and am/pm), the face, µs summed since the last log.
static mut CLOCK_PART_US: [u64; 2] = [0; 2];

/// The last plain camera op (op 14 that started a frame from scratch, not over an overlay), replayed each second while a clock shows; None when the last frame had an overlay or an app.
static mut LAST_CAM: Option<([u8; 18], usize)> = None;

/// The clock's rings and the time written in its centre, from local wall time to the counter's tick, and the unit its digits count in (the key: the digits change when it does). Every ring is a continuous sweep of grey over the dark from 12:00 that clicks back to dark when it wraps (owner, 2026-10-23; continuous 2026-10-24, drawn at whatever rate frames go out). Decimal, three rings: seconds outer, minutes middle, the 12-hour hand inner (after noon its fill is a lighter grey: the AM/PM); digits h:m:s. Dozenal, four rings, the day in twelfths four times over: outermost the 50 s unit's twelfths (about 4.2 s each), then the 50 s units, the dozenal hour's twelfths, the dozenal hours over the whole day; digits from the hour down, without trailing zils (owner: noon is Lun, not Lun:Zil:Zil:Zil).
fn clock_state(text: &mut [u8; 32]) -> ([draw::Ring; 6], usize, usize, i64, Option<bool>) {
    // In focus (the map arrived at it, display op 25), the dozenal clock shows two more places (owner, 2026-10-26): six rings, and six glyphs in two rows of three, the last place about a 35th of a second.
    let focused = unsafe { FOCUS } == WIDGET_CLOCK;
    let (secs, sub) = ferros_sys::local_time().unwrap_or((0, 0));
    let hz = freq() as i64;
    // Ticks of the counter since local midnight.
    let t = secs.rem_euclid(86400) * hz + sub as i64;
    // Where t is in a period of `p` ticks, Q16 of a turn.
    let frac = |p: i64| (((t % p) << 16) / p.max(1)) as u32;
    let dozenal = ferros_sys::pref(ferros_sys::PREF_BASE) & 0x7F == 12;
    let mut n = 0;
    let mut put = |c: char, n: &mut usize| {
        let mut b = [0u8; 4];
        for &x in c.encode_utf8(&mut b).as_bytes() {
            if *n < text.len() {
                text[*n] = x;
                *n += 1;
            }
        }
    };
    let ring = |frac_q16: u32, lighter: bool, ticks: usize| draw::Ring { frac_q16, lighter, ticks };
    if dozenal && focused {
        // Six places: the day's share in dozenal, exact in 128-bit integers (a place's unit is not a whole number of counter ticks): place k counts day / 12^(k+1).
        let day = 86400 * hz as i128;
        let tt = t as i128;
        let places = 6u32;
        // Ring j (0 outermost) sweeps the unit of place (places - 1 - j): its fraction of that unit.
        let rings: [draw::Ring; 6] = core::array::from_fn(|j| {
            let m = places as i128 - 1 - j as i128;
            let scaled = tt * 12i128.pow(m as u32) % day;
            ring((scaled * 65536 / day) as u32, false, 12)
        });
        let u = (tt * 12i128.pow(places) / day) as i64;
        let d: [i64; 6] = core::array::from_fn(|k| (u / 12i64.pow(places - 1 - k as u32)) % 12);
        for (k, v) in d.iter().enumerate() {
            if k == 3 {
                put('\n', &mut n);
            } else if k > 0 {
                put(':', &mut n);
            }
            put(char::from_u32(0x10 + *v as u32).unwrap_or('?'), &mut n);
        }
        (rings, 6, n, u, None)
    } else if dozenal {
        // 50 s / 12 in ticks (the counter runs at 24.576 MHz: 102,400,000, exact).
        let q4 = 50 * hz / 12;
        let rings = [ring(frac(q4 * 12), false, 12), ring(frac(q4 * 144), false, 12), ring(frac(q4 * 1728), false, 12), ring(frac(q4 * 20736), false, 12), ring(0, false, 0), ring(0, false, 0)];
        let u = t / q4;
        let d = [u / 1728, (u / 144) % 12, (u / 12) % 12, u % 12];
        // The last place that is not zil; the hour always shows.
        let last = (1..4).rev().find(|&k| d[k] != 0).unwrap_or(0);
        for (k, v) in d.iter().enumerate().take(last + 1) {
            if k > 0 {
                put(':', &mut n);
            }
            put(char::from_u32(0x10 + *v as u32).unwrap_or('?'), &mut n);
        }
        (rings, 4, n, u, None)
    } else {
        let sod = t / hz;
        let (h, m, sec) = (sod / 3600, (sod / 60) % 60, sod % 60);
        let rings = [ring(frac(60 * hz), false, 60), ring(frac(3600 * hz), false, 60), ring(frac(43200 * hz), h >= 12, 12), ring(0, false, 0), ring(0, false, 0), ring(0, false, 0)];
        // Twelve-hour digits to match the twelve-hour hand ring, the hour without a leading zero (9:05:07, not 09:05:07), 12 at noon and midnight; am or pm written under them (owner, 2026-10-24).
        let h12 = if h % 12 == 0 { 12 } else { h % 12 };
        if h12 >= 10 {
            put((b'0' + (h12 / 10) as u8) as char, &mut n);
        }
        put((b'0' + (h12 % 10) as u8) as char, &mut n);
        for v in [m, sec] {
            put(':', &mut n);
            put((b'0' + (v / 10) as u8) as char, &mut n);
            put((b'0' + (v % 10) as u8) as char, &mut n);
        }
        (rings, 3, n, sod, Some(h >= 12))
    }
}

/// The light scope in a circle at (cx, cy), radius r (Q8 px), turned by (tc, ts) — the camera's turn and the circle's own, as icons are, so it lies however the fingers placed it — front to back over its black plate (owner, 2026-10-25). One line of text above the plot, words only: the channel and the mode (free, locked, fold). Top: the flicker engine's 16 kHz trace of one channel over the span (SCOPE_SPAN_US), AC (the AoC's PDM decimator high-passes it), drawn as a phosphor on a grid of half pixels both ways — every sample-to-sample segment lights the half-pixel rows it crosses in its half-pixel column, brighter where more of them cross, so every join shows and a dense waveform shades; when a column is narrower than a sample the samples are joined by straight lines. Bottom: the colour channels' levels over the same span in stops (log2 of counts per unit gain), one scale fitted to all four together, a faint line per stop, the AoC's ~5 readings a second joined by straight lines and the newest held to the right edge. Everything is laid out in the circle's own frame (u right, v down, pixels from its centre) and reaches the screen through `splat`, a point spread over the four pixels around it, so a turned plot stays smooth. Returns the circle's rectangle; SCOPE_LIVE gets the part that changes (the plot's turned box).
fn scope_widget(pixels: &mut [Px], width: usize, height: usize, cx: i64, cy: i64, r: i64, turn: (f32, f32), th: &ferros_sys::theme::Theme, font: &Font) -> draw::Rect {
    let rp = (r >> 8) as f32;
    let on = ferros_sys::pref(ferros_sys::PREF_SCOPE);
    let plate = BLACK & 0x00FF_FFFF;
    let (ox, oy) = ((cx >> 8) as f32 + 0.5, (cy >> 8) as f32 + 0.5);
    let (tc, ts) = turn;
    // The plot box in the circle's frame.
    let (u0, v0, bw, bh) = (-rp * 0.66, -rp * 0.55, rp * 1.32, rp * 1.1);
    let (top_h, gap) = (bh * 0.55, bh * 0.06);
    let bot_h = bh - top_h - gap;
    // The label's size and the line it sits on, above the box.
    let lpx = rp * 0.09;
    let lv = v0 - lpx * 0.9;
    // The turned box's screen bounds, with the label: what the redraws between frames clear and draw again.
    {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let vt = lv - lpx;
        for (u, v) in [(u0, vt), (u0 + bw, vt), (u0, v0 + bh), (u0 + bw, v0 + bh)] {
            let (x, y) = (ox + tc * u - ts * v, oy + ts * u + tc * v);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        unsafe { SCOPE_LIVE = draw::Rect { x0: x0 as i32 - 2, y0: y0 as i32 - 2, x1: x1 as i32 + 3, y1: y1 as i32 + 3 } };
    }
    if on == 0 {
        let mut cv = Canvas::new(pixels, width, height, &mut Damage);
        let px = rp * 0.11;
        let t = "scope off";
        let tw = text_width(font, t, px);
        text(&mut cv, font, t, ox - tw * 0.5, oy, px, 0xFF00_0000 | th.text2);
        return draw::disc(pixels, width, height, cx, cy, r, plate);
    }
    if bw < 2.0 || top_h < 2.0 {
        return draw::disc(pixels, width, height, cx, cy, r, plate);
    }
    let (bx0, by0, bx1, by1) = draw::bounds(width, height);
    // A point of the circle's frame at an opacity (0–1), spread over the four pixels around it by its distance to each (bilinear), under what is there.
    let mut splat = |pixels: &mut [Px], u: f32, v: f32, colour: u32, a: f32| {
        let (x, y) = (ox + tc * u - ts * v - 0.5, oy + ts * u + tc * v - 0.5);
        let (xf, yf) = (libm::floorf(x), libm::floorf(y));
        let (fx, fy) = (x - xf, y - yf);
        let (xi, yi) = (xf as i64, yf as i64);
        for (dx, dy, w) in [(0, 0, (1.0 - fx) * (1.0 - fy)), (1, 0, fx * (1.0 - fy)), (0, 1, (1.0 - fx) * fy), (1, 1, fx * fy)] {
            let (px_, py_) = (xi + dx, yi + dy);
            let c = (a * w * 255.0) as u32;
            if px_ < bx0 || py_ < by0 || px_ >= bx1 || py_ >= by1 || c == 0 {
                continue;
            }
            let p = &mut pixels[py_ as usize * width + px_ as usize];
            *p = draw::under(*p, draw::at_coverage(colour & 0x00FF_FFFF, c.min(255)));
        }
    };
    // A straight line in the circle's frame: points every half pixel, each carrying half the opacity, so a line's weight per pixel of length is `a` at any turn.
    let mut line = |pixels: &mut [Px], (ua, va): (f32, f32), (ub, vb): (f32, f32), colour: u32, a: f32| {
        let len = libm::sqrtf((ub - ua) * (ub - ua) + (vb - va) * (vb - va));
        let n = (len * 2.0) as usize + 1;
        let k = a * len / n as f32;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            splat(pixels, ua + (ub - ua) * t, va + (vb - va) * t, colour, if i == 0 || i == n { k * 0.5 } else { k });
        }
    };
    // Channel colours (register order, provisional): red, visible, blue, green, IR, clear.
    const COL: [u32; 6] = [0xFF4040, 0xC0C0C0, 0x4080FF, 0x40E060, 0xB040A0, 0xFFFFFF];
    let span_us = unsafe { SCOPE_SPAN_US } as u64;
    // ── the fast trace ──
    let buf = unsafe { &mut *(&raw mut SCOPE_BUF) };
    // The span's samples are asked for (four words more for the call's header), so a short span costs a short copy, and LOCK_REACH more for the lock, which ends the window on a rising crossing up to that far back.
    let (_, rate0, _, _) = ferros_sys::light(&mut buf[..4]);
    let want = ((span_us * rate0.max(1) as u64 / 1_000_000) as usize).clamp(2, buf.len() - 4 - LOCK_REACH);
    let (_end, _rate, mask, got) = ferros_sys::light(&mut buf[..want + LOCK_REACH + 4]);
    let all = &buf[..got];
    let n = got.min(want);
    let ch = (mask.trailing_zeros() as usize).min(5);
    // Half-pixel rows and columns.
    let (rows, cols) = ((top_h * 2.0) as usize, (bw * 2.0) as usize);
    // The dominant period and its newest rising crossing, from at least a quarter second (`scope_lock`), as an index into `all`.
    let from = got.saturating_sub(want.max(4096) + LOCK_REACH);
    let lock = scope_lock(&all[from..]).map(|(p, t)| (p, t + from as f32));
    let fold = unsafe { SCOPE_FOLD } && lock.is_some_and(|(p, _)| n as f32 >= p);
    // What is drawn: the samples themselves (a slice, the window ending on the lock's crossing when there is one), or a value per half-pixel column (narrower than a sample: the straight line between the samples around it; folded: the average cycle).
    let mut colv: alloc::vec::Vec<f32> = alloc::vec::Vec::new();
    let mut s: &[i32] = &[];
    if fold {
        // Folded (owner, 2026-10-25): every sample of the span placed by its phase in the dominant period, averaged into a bin per half-pixel column of one cycle, two cycles across the box, a rising crossing at the left edge and the middle. ~120 cycles a second of a 120 Hz source average the noise down ~11×, and as the period is no whole number of samples, successive cycles land at different phases and fill the cycle in finer than 16 kHz (a sampling scope's equivalent time).
        let (p, tc) = lock.unwrap_or((1.0, 0.0));
        let nb = (cols / 2).max(1);
        let mut sum = alloc::vec![0f32; nb];
        let mut cnt = alloc::vec![0u32; nb];
        let w = &all[got - n..];
        let base = (got - n) as f32 - tc;
        for (i, &v) in w.iter().enumerate() {
            let ph = (base + i as f32) / p;
            let k = (((ph - libm::floorf(ph)) * nb as f32) as usize).min(nb - 1);
            sum[k] += v as f32;
            cnt[k] += 1;
        }
        // Empty bins (a cycle wider than the span's samples can fill) take the straight line between their filled neighbours, round the cycle.
        let filled: alloc::vec::Vec<usize> = (0..nb).filter(|&k| cnt[k] > 0).collect();
        if !filled.is_empty() {
            let mut cyc = alloc::vec![0f32; nb];
            for (j, &k) in filled.iter().enumerate() {
                let k2 = filled[(j + 1) % filled.len()];
                let (va, vb) = (sum[k] / cnt[k] as f32, sum[k2] / cnt[k2] as f32);
                let gap = (k2 + nb - k - 1) % nb + 1;
                for d in 0..gap {
                    cyc[(k + d) % nb] = va + (vb - va) * d as f32 / gap as f32;
                }
            }
            colv = (0..cols).map(|c| cyc[c % nb]).collect();
        }
    } else if n >= 2 {
        // Locked: the window ends on the newest rising crossing (the fitted one, so it does not jitter), so every frame starts at the same phase and the waveform stands still; unlocked, on the newest sample.
        let (e, f) = match lock {
            Some((_, tc)) if libm::floorf(tc) as usize + 1 >= n && (libm::floorf(tc) as usize) < got => (libm::floorf(tc) as usize + 1, tc - libm::floorf(tc)),
            _ => (got, 1.0),
        };
        s = &all[e - n..e];
        let per = n as f32 / cols as f32;
        if per < 1.0 {
            // Narrower than a sample: the straight line between the samples around the column's centre; τ (the window's own time, 0 to n) is the slice's position τ + f − 1, the window ending exactly on the crossing.
            colv = (0..cols)
                .map(|col| {
                    let t = ((col as f32 + 0.5) * per + f - 1.0).max(0.0);
                    let i = (t as usize).min(n - 2);
                    let fr = (t - i as f32).min(1.0);
                    s[i] as f32 + (s[i + 1] - s[i]) as f32 * fr
                })
                .collect();
        }
    }
    // The label: the channel and the mode, words only (owner, 2026-10-25: no number that changes, so it is not rasterised anew as values move), turned with the plot.
    {
        const NAME: [&str; 6] = ["red", "visible", "blue", "green", "IR", "clear"];
        let mode = if fold { "fold" } else if lock.is_some() { "locked" } else { "free" };
        let mut t = alloc::string::String::new();
        let _ = core::fmt::Write::write_fmt(&mut t, format_args!("{} \u{2022} {}", NAME[ch], mode));
        text_turned_centre(pixels, width, height, &t, ox - ts * lv, oy + tc * lv, lpx, turn, 0xFF00_0000 | th.text2);
    }
    let drawn = !colv.is_empty() || s.len() >= 2;
    if drawn {
        let (mean, peak) = if !colv.is_empty() {
            let m = colv.iter().sum::<f32>() / colv.len() as f32;
            (m, colv.iter().map(|&v| (v - m).abs()).fold(1.0f32, f32::max))
        } else {
            let m = (s.iter().map(|&v| v as i64).sum::<i64>() / s.len() as i64) as f32;
            (m, s.iter().map(|&v| (v as f32 - m).abs()).fold(1.0f32, f32::max))
        };
        let half = rows as f32 * 0.5;
        let row = |v: f32| (half - (v - mean) / peak * (half - 1.0)).clamp(0.0, rows as f32 - 1.0);
        let mut cov = alloc::vec![0f32; rows];
        let per = if colv.is_empty() { s.len() as f32 / cols as f32 } else { 1.0 };
        let mut prev = row(if colv.is_empty() { s[0] as f32 } else { colv[0] });
        // A cell's opacity: a floor for being crossed at all, so every join shows (a fast edge lit only by its share of the column's segments came out at ~0.16 coverage, a few percent of light on the gamma-2 canvas: the edges vanished and the trace fell apart into strokes, 2026-10-25), plus the share of segments that cross it, square-rooted, so where the trace dwells is brighter; half, as two half-pixel columns share each pixel.
        let inv = 1.0 / per.max(1.0);
        for col in 0..cols {
            cov.iter_mut().for_each(|c| *c = 0.0);
            let mut seg = |cov: &mut [f32], y: f32, prev: &mut f32| {
                let (lo, hi) = (prev.min(y), prev.max(y));
                let (r0, r1) = (lo as usize, (hi as usize).min(rows - 1));
                for c in &mut cov[r0..=r1] {
                    *c += 1.0;
                }
                *prev = y;
            };
            if colv.is_empty() {
                let a = ((col as f32 * per) as usize).min(s.len() - 1);
                let b = (((col + 1) as f32 * per) as usize).clamp(a + 1, s.len());
                for &v in &s[a..b] {
                    seg(&mut cov, row(v as f32), &mut prev);
                }
            } else {
                seg(&mut cov, row(colv[col]), &mut prev);
            }
            let u = u0 + (col as f32 + 0.5) * 0.5;
            for (rr, &c) in cov.iter().enumerate() {
                if c > 0.0 {
                    splat(pixels, u, v0 + (rr as f32 + 0.5) * 0.5, COL[ch], 0.5 * (0.6 + 0.4 * libm::sqrtf((c * inv).min(1.0))));
                }
            }
        }
    }
    // ── the slow levels, in stops ──
    let h = unsafe { &*(&raw const SLOW) };
    let t_now = now();
    let span = freq_ticks() * span_us / 1_000_000;
    // Slots, as the spectral channels were labelled (provisional): r 0, g 1, b 2, ir 4.
    const SLOW_CH: [(usize, u32); 4] = [(0, 0xFF4040), (1, 0x40E060), (2, 0x4080FF), (4, 0xB040A0)];
    let count = h.1.min(64);
    let pts: alloc::vec::Vec<(u64, [f32; 6])> = (0..count).map(|i| h.0[(h.1 - count + i) % 64]).collect();
    unsafe {
        SCOPE_DBG[0] = count as u64;
        for k in 0..3 {
            SCOPE_DBG[1 + k] = pts.len().checked_sub(1 + k).map(|j| (t_now.wrapping_sub(pts[j].0) * 1000 / freq_ticks()).min(99_999)).unwrap_or(99_999);
        }
    }
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    // The scale's window: twice the span, and never less than half a second, so a short span still holds the reading it shows.
    let window = (2 * span).max(freq_ticks() / 2);
    for &(_, v) in pts.iter().filter(|(t, _)| t_now.wrapping_sub(*t) < window) {
        for &(slot, _) in &SLOW_CH {
            if v[slot] > 0.0 {
                let l = libm::log2f(v[slot]);
                lo = lo.min(l);
                hi = hi.max(l);
            }
        }
    }
    let vb = v0 + top_h + gap;
    if lo <= hi && bot_h > 4.0 {
        let (lo, hi) = (libm::floorf(lo), libm::ceilf(hi).max(libm::floorf(lo) + 1.0));
        let vrow = |l: f32| vb + (hi - l) / (hi - lo) * (bot_h - 1.0);
        // The time of a reading as a position across the box: now at the right edge, the span's start at the left.
        let ucol = |t: u64| u0 + bw - (t_now.wrapping_sub(t) as i64).max(0) as f32 / span.max(1) as f32 * bw;
        for &(slot, colour) in &SLOW_CH {
            let mut last: Option<(f32, f32)> = None;
            for &(t, v) in pts.iter() {
                if v[slot] <= 0.0 || t_now.wrapping_sub(t) > (1u64 << 62) {
                    continue;
                }
                let p = (ucol(t), vrow(libm::log2f(v[slot])));
                if let Some(q) = last {
                    // Only what lies inside the box: a segment that starts before the span is cut at its left edge.
                    if p.0 > u0 {
                        let a = if q.0 < u0 { (u0, q.1 + (p.1 - q.1) * (u0 - q.0) / (p.0 - q.0).max(1e-3)) } else { q };
                        line(pixels, a, p, colour, 0.9);
                    }
                }
                last = Some(p);
            }
            // The newest reading held to now.
            if let Some(q) = last {
                if q.0 < u0 + bw {
                    line(pixels, (q.0.max(u0), q.1), (u0 + bw, q.1), colour, 0.9);
                }
            }
        }
        // A faint line per stop.
        let mut st = lo;
        while st <= hi {
            let v = vrow(st);
            line(pixels, (u0, v), (u0 + bw, v), th.text2, 0.16);
            st += 1.0;
        }
    }
    draw::disc(pixels, width, height, cx, cy, r, plate)
}

/// How far back the scope's lock may end its window: half a second at 16 kHz, a period of 2 Hz and longer than any flicker worth locking to.
const LOCK_REACH: usize = 8192;

/// The dominant period of a stretch of the light trace and the time of its newest rising crossing, both in samples (fractional), or None when there is no steady one. Rising crossings of the stretch's mean with hysteresis (armed only once the trace has been a tenth of its swing below the mean, so noise at the mean cannot double-count), each placed between its two samples by the straight line through them; the median interval gives a first period, every crossing its cycle number by it (so a missed crossing does not break the count), and a least-squares line through (cycle, time) gives the period to a small fraction of a sample. No lock when there are fewer than three crossings, the period is under four samples, or the crossings scatter more than a tenth of a period from the line.
fn scope_lock(s: &[i32]) -> Option<(f32, f32)> {
    let n = s.len();
    if n < 64 {
        return None;
    }
    let mean = s.iter().map(|&v| v as i64).sum::<i64>() / n as i64;
    let h = s.iter().map(|&v| (v as i64 - mean).abs()).max().unwrap_or(0) / 10;
    if h == 0 {
        return None;
    }
    let mut cross: alloc::vec::Vec<f64> = alloc::vec::Vec::new();
    let mut armed = false;
    for i in 1..n {
        let (d0, d1) = (s[i - 1] as i64 - mean, s[i] as i64 - mean);
        armed |= d1 < -h;
        if armed && d0 < 0 && d1 >= 0 {
            cross.push((i - 1) as f64 + (-d0) as f64 / (d1 - d0) as f64);
            armed = false;
        }
    }
    let m = cross.len();
    if m < 3 {
        return None;
    }
    let mut iv: alloc::vec::Vec<f64> = cross.windows(2).map(|w| w[1] - w[0]).collect();
    iv.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let p0 = iv[iv.len() / 2];
    if p0 < 4.0 {
        return None;
    }
    let ks: alloc::vec::Vec<f64> = cross.iter().map(|&t| libm::round((t - cross[0]) / p0)).collect();
    let (sk, st) = (ks.iter().sum::<f64>(), cross.iter().sum::<f64>());
    let (skk, skt) = (ks.iter().map(|k| k * k).sum::<f64>(), ks.iter().zip(&cross).map(|(k, t)| k * t).sum::<f64>());
    let den = m as f64 * skk - sk * sk;
    if den <= 0.0 {
        return None;
    }
    let p = (m as f64 * skt - sk * st) / den;
    let a = (st - p * sk) / m as f64;
    let rms = libm::sqrt(ks.iter().zip(&cross).map(|(k, t)| (t - a - p * k) * (t - a - p * k)).sum::<f64>() / m as f64);
    if !(p >= 4.0) || rms > 0.1 * p {
        return None;
    }
    Some((p as f32, (a + p * ks[m - 1]) as f32))
}

/// The counter ticks in a second.
fn freq_ticks() -> u64 {
    freq()
}

/// Text centred on (cx, cy) and turned by (c, s), front to back: the clock's digits, which turn with its face.
fn text_turned_centre(pixels: &mut [Px], width: usize, height: usize, s: &str, cx: f32, cy: f32, px: f32, turn: (f32, f32), colour: u32) {
    let g = glyph_engine();
    let tw = g.width(s, px);
    // The baseline's start in the text's own frame, (−width/2, +0.36 em) from the centre, turned onto the screen.
    let (fx, fy) = (-tw * 0.5, px * 0.36);
    let (tc, ts) = turn;
    g.draw(pixels, width, height, s, cx + tc * fx - ts * fy, cy + ts * fx + tc * fy, px, turn, colour);
}

/// The map's retained scene. A static, not a heap vector: this process's heap is a bump allocator that every op resets to the mark after the typeface, so anything kept across calls on it would be overwritten by the next text op.
static mut SCENE: [Circle; 256] = [NO_CIRCLE; 256];
static mut SCENE_N: usize = 0;
/// Whether the layer is 10-bit (set once at start; `program_layer` reads it).
static mut TEN: bool = false;
/// What DECON said when a settle timed out: TRIG_CON, SHD_REG_UP_REQ, the frame-start count, the count it was waiting to move.
static mut SETTLE_TIMEOUT: [u32; 4] = [0; 4];
/// The first op 22 (a window shown), µs: [chrome, layout, settle, publish, whole], logged once by the slow-call line (2026-11-05: where the first attest frame's 177 ms goes).
static mut OP22_US: [u64; 5] = [0; 5];
/// The counter when the first window's frame was armed (raw, to line up against the kernel's press in `diag boot`).
static mut OP22_AT: u64 = 0;
/// Main loop passes, and those that could not sleep: a flip due, or a phase still owed (diagnostic, logged every ten seconds).
static mut PASSES: [u64; 3] = [0; 3];
/// The op being handled (G#EE outside the handler), and the first four times the layer was pointed at this server's buffers [counter, op] (2026-11-05: what shows ~80 ms of black before the attest screen).
static mut CUR_OP: u8 = 0xEE;
static mut FIRST_SHOWS: [(u64, u8); 4] = [(0, 0); 4];
static mut FIRST_SHOWS_N: usize = 0;
/// The first chrome draw, µs: [the discs, the whole] (the text is the rest), with op 22's line.
static mut CHROME_US: [u64; 2] = [0; 2];
/// The camera op's last phases, microseconds: the scene, the arena reset, the copy to the framebuffer, the present.
static mut PHASE_US: [u64; 4] = [1; 4];
/// Inside the scene: icons, discs, labels (µs, the last camera op).
static mut SCENE_US: [u64; 3] = [1; 3];
/// The map's colours, the glow's strength: the theme (ferros_sys::theme).
/// The glow that replaced the ring (owner, 2026-10-23: no stroke, so circles can touch; additive glow, 1/distance): its colour, how bright one pixel out by importance (ring 1, ring 2), the distance it holds that brightness (Q8 px), and the floor where it stops. Tunables, by eye.
const GLOW_SCALE_Q8: i64 = 256;
const GLOW_FLOOR: u32 = 2;
/// The log glow (`draw::glow_log`) in place of the 1/distance one: on; how far it reaches, 1/GLOW_FRACTION of the circle's radius past its edge at every size (owner, 2026-10-24: 1/12, no minimum, no maximum); and its fade, 2^GLOW_OCT_LOG2 octaves of squared distance from full light to none (2 = four). Tunables, by eye.
const GLOW_LOG: bool = true;
const GLOW_FRACTION: i64 = 12;
const GLOW_OCT_LOG2: u32 = 2;
const BLACK: u32 = rgb(0, 0, 0);

struct Log {
    grants: u64,
    buf: [u8; 256],
    n: usize,
}

impl Log {
    fn s(&mut self, t: &[u8]) {
        for &b in t {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
    }
    /// G# hex: eight digits for a 32-bit value, sixteen when it needs them (a counter).
    fn hex(&mut self, v: impl Into<u64>) {
        let v: u64 = v.into();
        self.s(b"G#");
        for i in (0..if v >> 32 == 0 { 8 } else { 16 }).rev() {
            self.s(&[b"0123456789ABCDEF"[((v >> (i * 4)) & 0xF) as usize]]);
        }
    }
    /// A counter stamp line, to line the server's start up against the kernel's press (diag boot prints its counter).
    fn at(&mut self, what: &[u8]) {
        self.s(b"display: at ");
        self.hex(now());
        self.s(b" ");
        self.s(what);
        self.s(b"\n");
        self.flush();
    }
    fn dec(&mut self, mut v: u64) {
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let t = d;
        self.s(&t[i..]);
    }
    fn flush(&mut self) {
        let n = self.n;
        let buf = self.buf;
        console(self.grants, &buf[..n]);
        self.n = 0;
    }
}

struct Pipe {
    grants: u64,
}

impl Pipe {
    fn rd(&self, slot: u64, off: u64) -> u32 {
        mmio_read(cap(self.grants, slot), off) as u32
    }
    fn wr(&self, slot: u64, off: u64, v: u32) {
        mmio_write(cap(self.grants, slot), off, v);
    }
    /// The whole layer, every word, as the kernel's modeset programs it from the oracle (dpu::decon_init) — nothing inherited: window `win` non-secure, ABL's blend function for an opaque base layer (FUNC_CON_0 G#102: function 1, alpha-multiply source 2; proven on this pipe), the full raster from (0,0), no colour map; WINCON enable on channel 0 (DMA0/DPP0); the DMA linear BGRX8888 at `fb` (translation is off, so the DMA address is the physical one), stride = width, no offset, image = raster; DPP0, SRAMC0 and HDRCOMM0 told the same size.
    fn program_layer(&self, win: u32, fb: usize, width: u32, height: u32) {
        unsafe {
            if FIRST_SHOWS_N < 4 {
                FIRST_SHOWS[FIRST_SHOWS_N] = (now(), CUR_OP);
                FIRST_SHOWS_N += 1;
            }
        }
        let w = (win * 0x1000) as u64;
        self.wr(S_WIN, w + 0x00, 1);
        self.wr(S_WIN, w + 0x04, 0x0102);
        self.wr(S_WIN, w + 0x08, 0);
        self.wr(S_WIN, w + 0x0C, 0);
        self.wr(S_WIN, w + 0x10, (height - 1) << 16 | (width - 1));
        self.wr(S_WIN, w + 0x14, 0);
        self.wr(S_WIN, w + 0x18, 0);
        self.wr(S_WIN, w + 0x1C, 0);
        self.wr(S_WINCON, w, 1);
        let ctrl = self.rd(S_DMA0, RDMA_IN_CTRL_0);
        let ten = unsafe { TEN };
        // The layer's frame alpha opaque (IN_CTRL_0 bits 24-31, IDMA_ALPHA) and the DPP taking it (DPP_COM_IO_CON bit 7, DPP_ALPHA_SEL), as ABL leaves them (G#FF, 1; regs-dpp.h, cal_9865). Kept from whatever was there until 2026-11-04, when a jump from a screen-off Android left both 0 and every frame went out fully transparent: black.
        self.wr(S_DMA0, RDMA_IN_CTRL_0, (ctrl & !(IDMA_FORMAT_MASK | IDMA_ROT_MASK | IDMA_COMP_MASK | IDMA_ALPHA_MASK)) | IDMA_ALPHA_OPAQUE | if ten { 19 << 8 } else { IDMA_BGRX8888 });
        // A 10-bit layer keeps its two low bits only if the DPP and HDR-common blocks are told the format too (bits [2:0] = 1, ARGB8101010) with their 10 bpc bit (6); measured 2026-10-22.
        let fmt = if ten { 1 | 1 << 6 } else { 0 };
        self.wr(S_DPP0, 0x0038, (self.rd(S_DPP0, 0x0038) & !(7 | 1 << 6)) | fmt | DPP_ALPHA_SEL);
        self.wr(S_HDRCOMM0, 0x000C, (self.rd(S_HDRCOMM0, 0x000C) & !(7 | 1 << 6)) | fmt);
        self.wr(S_DMA0, RDMA_SRC_WIDTH, width);
        self.wr(S_DMA0, 0x0014, height);
        self.wr(S_DMA0, RDMA_SRC_OFFSET, 0);
        self.wr(S_DMA0, RDMA_IMG_SIZE, height << 16 | width);
        self.wr(S_DMA0, RDMA_BASEADDR_P0, fb as u32);
        self.wr(S_DPP0, 0x003C, height << 16 | width);
        // The scaler off at 1:1 (DPP_COM_SCL_* in regs-dpp.h): Android leaves it on at its last layer's ratio, which drew ferros's full-raster layer blown up (the thirty-fifth jump).
        self.wr(S_DPP0, 0x0080, 0);
        self.wr(S_DPP0, 0x0084, height << 16 | width);
        self.wr(S_DPP0, 0x0088, 1 << 20);
        self.wr(S_DPP0, 0x008C, 1 << 20);
        self.wr(S_DPP0, 0x0090, 0);
        self.wr(S_DPP0, 0x0094, 0);
        self.wr(S_SRAMC0, 0x0014, (height - 1) << 16);
        self.wr(S_HDRCOMM0, 0x0020, height << 16 | width);
    }
    /// One frame: the window's and the global shadow update, the TE trigger unmasked until a frame starts (the frame-start interrupt's count, `frame_no`) (or a quarter second), then masked again. Returns (frames advanced, RDMA_IRQ).
    fn present(&self, win: u32) -> (bool, u32) {
        let f0 = frame_no();
        ONE_FRAME.store(true, Ordering::Release);
        self.wr(S_DECON, SHD_REG_UP_REQ, self.rd(S_DECON, SHD_REG_UP_REQ) | SHD_GLOBAL | SHD_CMP | (1 << win));
        let t = self.rd(S_DECON, TRIG_CON);
        self.wr(S_DECON, TRIG_CON, (t & !HW_TRIG_MASK) | HW_TRIG_EN);
        let t0 = now();
        let limit = freq() / 4;
        let mut moved = false;
        while now().wrapping_sub(t0) < limit {
            if frame_no() != f0 {
                moved = true;
                break;
            }
            // Asleep between looks, not spinning: on one core a spin here starved the touch server (its reads took two and three time slices, 2026-10-23).
            frame_wait();
        }
        let t = self.rd(S_DECON, TRIG_CON);
        self.wr(S_DECON, TRIG_CON, (t & !HW_TRIG_EN) | HW_TRIG_MASK);
        let irq = self.rd(S_DMA0, RDMA_IRQ);
        self.wr(S_DMA0, RDMA_IRQ, irq & IDMA_IRQ_CLEAR);
        (moved, irq)
    }
    /// Start a frame and return at once: the shadow update and the TE trigger unmasked; `settle` masks it again once the frame has started. Returns the frame-start count as it was, for `settle`.
    fn arm(&self, win: u32) -> u32 {
        let f0 = frame_no();
        ONE_FRAME.store(true, Ordering::Release);
        self.wr(S_DECON, SHD_REG_UP_REQ, self.rd(S_DECON, SHD_REG_UP_REQ) | SHD_GLOBAL | SHD_CMP | (1 << win));
        let t = self.rd(S_DECON, TRIG_CON);
        self.wr(S_DECON, TRIG_CON, (t & !HW_TRIG_MASK) | HW_TRIG_EN);
        f0
    }
    /// An armed frame that has started: mask the trigger (so the TE does not keep re-sending the same frame) and forget it. With `wait`, spin until it starts (a quarter second at most); otherwise only look.
    fn settle(&self, armed: &mut Option<u32>, wait: bool) {
        let Some(f0) = *armed else { return };
        let t0 = now();
        loop {
            if frame_no() != f0 {
                break;
            }
            if wait {
                frame_wait();
            }
            if !wait || now().wrapping_sub(t0) > freq() / 4 {
                if !wait {
                    return;
                }
                unsafe {
                    SETTLE_TIMEOUT = [self.rd(S_DECON, TRIG_CON), self.rd(S_DECON, SHD_REG_UP_REQ), frame_no(), f0];
                }
                break;
            }
        }
        let t = self.rd(S_DECON, TRIG_CON);
        self.wr(S_DECON, TRIG_CON, (t & !HW_TRIG_EN) | HW_TRIG_MASK);
        *armed = None;
    }
    /// The first enabled window on channel 0 (the base layer ABL draws on; 5 on a cold boot).
    fn base_window(&self) -> u32 {
        for w in 0..8u32 {
            let con = self.rd(S_WINCON, (w * 0x1000) as u64);
            if con & 1 == 1 && (con >> 4) & 0xF == 0 {
                return w;
            }
        }
        5
    }
    fn te(&self) -> bool {
        self.rd(S_GPP0, GPP0_DAT) >> 3 & 1 == 1
    }
}

/// The typeface's glyph engine (glyphs.rs: swash, outlines turned before rasterising, cached), made once at start.
static mut GLYPHS: Option<glyphs::Glyphs> = None;
fn glyph_engine() -> &'static mut glyphs::Glyphs {
    unsafe { (*(&raw mut GLYPHS)).as_mut().expect("glyphs") }
}
/// A handle standing for the typeface at the call sites (the engine itself is the static above).
struct Font;

/// Text into the canvas, front to back (under what is there), upright, left-aligned at (x, baseline). Returns the advance.
fn text(c: &mut Canvas, _font: &Font, s: &str, x: f32, baseline: f32, px: f32, colour: u32) -> f32 {
    let (w, h) = (c.width, c.height);
    glyph_engine().draw(c.pixels, w, h, s, x, baseline, px, (1.0, 0.0), colour)
}

fn text_width(_font: &Font, s: &str, px: f32) -> f32 {
    glyph_engine().width(s, px)
}

/// Square root by Newton on plain multiplies and divides. On husky, 2026-10-17, both `libm::sqrtf` and an inline `fsqrt` returned 0 for every input in this process while `libm::tanhf`/`exp2f`/`log2f` and fontdue's arithmetic were right — unexplained; `fsqrt_probe` records it per boot.
fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut r = if x > 1.0 { x } else { 1.0 };
    for _ in 0..24 {
        r = 0.5 * (r + x / r);
    }
    r
}

fn fsqrt_probe(x: f32) -> f32 {
    let r: f32;
    unsafe { core::arch::asm!("fsqrt {0:s}, {1:s}", out(vreg) r, in(vreg) x, options(pure, nomem, nostack)) };
    r
}

/// tanh from the exp2 that works: tanh(x) = (e^{2x} − 1) / (e^{2x} + 1).
fn tanh(x: f32) -> f32 {
    if x > 10.0 {
        return 1.0;
    }
    let e = libm::exp2f(2.0 * x * core::f32::consts::LOG2_E);
    (e - 1.0) / (e + 1.0)
}


/// The panel's measured transfer (DISPLAY-CALIB.md): relative luminance → 8-bit code, γ 2.185.
fn panel_code(y: f32) -> u32 {
    let y = if y.is_nan() || y < 0.0 { 0.0 } else if y > 1.0 { 1.0 } else { y };
    let c = pow(y, 1.0 / 2.185) * 255.0 + 0.5;
    if c.is_nan() { 0 } else if c > 255.0 { 255 } else { c as u32 }
}

/// The display curve with a knee (DISPLAY-DESIGN.md §6): linear up to white, the tanh shoulder only above it, reaching the peak at gamma2tanh's top (9.6× white). `white` = white's share of the peak (1/H); `k` sets how gently the shoulder begins (0.35: half-way to the peak near 4× white). Below white this is exactly the linear curve — same black, same slope — so SDR is untouched; the encoding's own tanh(√L)² curve was tried first and lifts the midtones by up to 1.74× (the owner saw it on the card, 2026-10-17).
fn knee(l: f32, white: f32, k: f32) -> f32 {
    if l <= 1.0 {
        return white * l;
    }
    let h = 1.0 / white;
    let top = tanh(k * (9.6 - 1.0));
    white * (1.0 + (h - 1.0) * tanh(k * (l - 1.0)) / top)
}

/// The highlight test card. Scene light L across the width on a log scale, −6 stops to +3.26 (9.6× white, gamma2tanh's top). Top half: the display curve, Y/peak = tanh²(√(L/H)) with H = 1.75 — slope 1 at black, white at 41 % of the peak, the stops above rolled smoothly, nothing clipped. Bottom half: linear at the same white, clipped at the peak. A 12-pixel line of code 255 between them is the peak itself. Ticks (dark lines) at L = 1, 2, 4, 8.
fn testcard(pixels: &mut [Px], width: usize, height: usize) {
    let stops_lo = -6.0f32;
    let stops_hi = libm::log2f(9.6);
    for x in 0..width {
        let stops = stops_lo + (stops_hi - stops_lo) * x as f32 / (width - 1) as f32;
        let l = libm::exp2f(stops);
        // The owner's curve (2026-10-17): Y/peak = tanh(L/H), H = peak ÷ white — smooth everywhere, never clips, white at tanh(1/H) of the peak (52 % at H = 1.75 against 57 % linear).
        // Top: the display curve (DISPLAY-DESIGN.md §6), tanh in gamma-2 space, Y/peak = tanh²(√(L/H)). Bottom: linear at the same white, clipped at the peak — the reference the roll-off is judged against.
        let b = tanh(sqrt(l / 1.75));
        let white = tanh(sqrt(1.0 / 1.75));
        let top = panel_code(b * b);
        let bottom = panel_code(l * white * white);
        let tick = [0.0f32, 1.0, 2.0, 3.0].iter().any(|t| libm::fabsf(stops - t) < 0.004);
        for y in 0..height {
            let band = y * 8 / height;
            let c = if y < height / 2 { top } else { bottom };
            // A hairline tick where L is a power of two from white, in the two middle bands only.
            let c = if tick && (band == 3 || band == 4) { 0 } else { c };
            // The panel's peak, code 255, as a 12-pixel line between the halves: the reference both ramps' tops are judged against.
            let c = if y >= height / 2 - 6 && y < height / 2 + 6 { 255 } else { c };
            pixels[y * width + x] = draw::opaque(c << 16 | c << 8 | c);
        }
    }
}

/// The present stage's policy (DISPLAY-CALIB.md, "Decided", 2026-10-22): the panel driven at its measured peak (HBM: 1467 cd/m² of native white on this unit) and white at 133 cd/m².
const PEAK_NITS: f32 = 1467.0;
const WHITE_NITS: f32 = 133.0;
/// White is equal-energy E (owner): solved from the panel's measured spectra under VSF's observer (Stockman & Sharpe 10°, `tools/panel-white-e.py`, the DBV 3307 readings), E needs R 1.0, G 0.7394, B 0.8172 of each channel's light — the ODT's row sums below; E keeps 80.1 % of native white's light, which is the peak the roll-off lands on. On the code they are g^(1/2.19), Q15: the ramps (op 21) apply them to the one table directly.
const E_CODE_Q15: [u32; 3] = [32768, 28548, 29883];
const E_LIGHT: f32 = 0.801;

/// The present stage's one table: a 16-bit working value (gamma 2, white = 1.0 = Q14 16384, up to 4.0 = 16× white in light) to a 10-bit panel code with 4 fraction bits (Q4, 0..16368), before the white point. One entry folds the whole display transform: the roll-off, Y/peak = tanh²(a·v), always on, with infinity on the panel's peak and `a` solved so white lands exactly on its share of it, tanh²(a) = white ÷ peak, a = atanh(√(white/peak)) (owner, 2026-10-22: at the brightest setting white is half the peak); then the panel's 1/2.19. Shared by the three channels (their curves agree within the instrument's noise); E's per-channel gains come after it. Built once, here, with f32; everything per pixel is integer.
fn build_lut(lut: &mut [u16], white: f32, peak_e: f32) {
    let k = libm::atanhf(sqrt((white / peak_e).min(0.999)));
    for (i, e) in lut.iter_mut().enumerate() {
        let y = tanh(i as f32 / 16384.0 * k);
        let q = pow(y, 2.0 / PANEL_GAMMA) * 1023.0 * 16.0 + 0.5;
        *e = if q.is_nan() || q < 0.0 { 0 } else if q > 16368.0 { 16368 } else { q as u16 };
    }
}

/// The panel's brightness: high brightness mode with DBV G#FFF, or normal mode at `dbv`. TE2 with it.
fn set_panel(dsi: &ferros_dsi::Dsim<DsiGrant>, hbm: bool, dbv: u16) -> bool {
    let r = dsi.dcs(&[0xF0, 0x5A, 0x5A]).and_then(|_| dsi.dcs(&[0x53, if hbm { 0xE0 } else { 0x20 }])).and_then(|_| dsi.dcs(&[0x51, (dbv >> 8) as u8, dbv as u8])).and_then(|_| dsi.dcs(&[0xF0, 0xA5, 0xA5]));
    te2(dsi);
    r.is_ok()
}

/// The panel's TE2 output, the beat the under-display light and proximity sensor (TMD3719) times its bursts to: on, changeable, rising edge G#10 and falling G#30 lines into the frame (panel-google-hk3.c `hk3_update_te2_internal`; ABL leaves it off, and without it the sensor never measured, 2026-10-24). Sent with the brightness and after every display on.
fn te2(dsi: &ferros_dsi::Dsim<DsiGrant>) {
    let _ = dsi.dcs(&[0xF0, 0x5A, 0x5A]);
    let _ = dsi.dcs(&[0xB0, 0x00, 0x42, 0xF2]);
    let _ = dsi.dcs(&[0xF2, 0x0D]);
    let _ = dsi.dcs(&[0xB0, 0x00, 0x01, 0xB9]);
    let _ = dsi.dcs(&[0xB9, 0x04]);
    let _ = dsi.dcs(&[0xB0, 0x00, 0x1E, 0xB9]);
    let _ = dsi.dcs(&[0xB9, 0x00, 0x10, 0x00, 0x30]);
    let _ = dsi.dcs(&[0xF0, 0xA5, 0xA5]);
}

/// The panel's brightness (DBV) against its light, measured in the 1 cm window (DISPLAY-CALIB.md, 2026-10-22): normal mode to DBV 3307; above that, high brightness mode at its full 1472 cd/m² (native white).
const DBV_NITS: [(f32, f32); 7] = [(196.0, 1.9), (400.0, 9.2), (853.0, 49.9), (1353.0, 136.2), (2000.0, 324.6), (2600.0, 588.5), (3307.0, 984.2)];
const HBM_NITS: f32 = 1471.9;

/// The panel setting that gives a native peak as close to `nits` as it has: (high brightness mode, DBV, the native peak it gives). Between measured points, log-linear.
fn panel_for(nits: f32) -> (bool, u16, f32) {
    if nits > DBV_NITS[6].1 {
        return (true, 4095, HBM_NITS);
    }
    if nits <= DBV_NITS[0].1 {
        return (false, DBV_NITS[0].0 as u16, DBV_NITS[0].1);
    }
    for w in DBV_NITS.windows(2) {
        let ((d0, n0), (d1, n1)) = (w[0], w[1]);
        if nits <= n1 {
            let t = libm::logf(nits / n0) / libm::logf(n1 / n0);
            return (false, (d0 + (d1 - d0) * t) as u16, nits);
        }
    }
    (false, 3307, DBV_NITS[6].1)
}

/// Brightness step (stops below the brightest white) to (white, the E peak the table lands on, the panel setting): the brightest white is half the E peak at the panel's top (owner: max white one stop under max); each step halves it; the panel's own brightness comes down once its peak would be more than 16× white (the working format's headroom), so the codes are never wasted on light no value can reach.
fn brightness(step: u8) -> (f32, f32, bool, u16) {
    let top_e = HBM_NITS * E_LIGHT;
    let white = top_e * 0.5 / (1u32 << step.min(8)) as f32;
    // The panel never below the normal-mode top (owner, 2026-11-03): below about DBV 1400 the DDIC dims by lengthening each frame's dark window (95 % emission duty at DBV 1490 and above, about 85 % at 1236, under half at 983, measured by eye in Android), so every step under it is the pixel values' (the table and the 12-bit dither), the light's duty staying at the panel's 95 %. High brightness mode only for the steps that need more than DBV 3307 gives.
    let want = (white * 16.0).min(top_e) / E_LIGHT;
    let (hbm, dbv, native) = if want > DBV_NITS[6].1 { panel_for(want) } else { (false, DBV_NITS[6].0 as u16, DBV_NITS[6].1) };
    (white, native * E_LIGHT, hbm, dbv)
}

/// The ordered 4×4 dither (Bayer), in sixteenths. The table's Q4 10-bit code is a Q6 8-bit code; the threshold (×4, in 64ths) is added before the six fraction bits are dropped, so each 4×4 cell (0.2 mm at 489 ppi) carries four bits below the 8-bit code the link takes: 12 bits effective. The DQE's own dither is in shift mode (kernel `start_home`), passing these 8 bits through untouched — two ordered dithers stacked partly cancelled into visible steps (owner's eyes, 2026-10-22).
const BAYER: [[u16; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// A Q4 10-bit code (= Q6 8-bit) to the 10-bit word's code: dithered to 8 bits by the threshold `th` (0-63, in 64ths of an 8-bit step), the two low bits zero for the DQE's truncation.
#[inline(always)]
fn dither8(q4: u32, th: u32) -> u32 {
    ((q4 + th) >> 6).min(255) << 2
}

/// The temporal dither (owner, 2026-10-22): four frames, flipped every 100 ms. Each pixel's threshold is its 4×4 Bayer level (4 bits, ×4) plus a temporal rank (2 bits) that differs from frame to frame, so over the four frames the pixel shows all 64 levels of the six fraction bits: 8-bit codes on the link, 14 bits effective. The rank's order is offset per 4×4 cell, so no frame is darker or lighter than another as a whole.
const PHASES: usize = 4;
const TEMPORAL_RANK: [u32; 4] = [0, 2, 1, 3];

#[inline(always)]
fn threshold(x: usize, y: usize, k: usize) -> u32 {
    BAYER[y & 3][x & 3] as u32 * 4 + TEMPORAL_RANK[(k + (x >> 2) + (y >> 2)) & 3]
}

/// The four phase frames (`display-phases`), each 16 MiB apart; set at start when the region is granted.
static mut PHASE_BASE: [usize; PHASES] = [1; PHASES];
static mut TEMPORAL: bool = false;
/// A client has drawn (the op handler's `client_owned`, readable outside its closure): until then nothing flips, so the boot ring stays up.
static mut CLIENT_DREW: bool = false;
/// The phase on the layer now.
static mut CUR_PHASE: usize = 0;

/// The buffer the layer should show: the current phase when the temporal dither is on, else the one framebuffer.
fn layer_fb(fb: usize) -> usize {
    unsafe { if TEMPORAL { PHASE_BASE[CUR_PHASE] } else { fb } }
}

fn phase_slice(k: usize, n: usize) -> &'static mut [u32] {
    unsafe { core::slice::from_raw_parts_mut(PHASE_BASE[k] as *mut u32, n) }
}

/// FERROS_FPS_DOT=1 (dev builds): the centre pixel of each phase frame stamped red, green, blue, black for phases 0-3 after every publish into it, so the dot cycles R G B K at whatever rate the layer flips (10 Hz at rest, the frame rate under a finger) and a stall shows as a stuck colour. Absent from other builds.
const FPS_DOT: bool = option_env!("FERROS_FPS_DOT").is_some();
/// The four colours as BGRA1010102 words (B[31:22] G[21:12] R[11:2] A[1:0]).
const FPS_DOT_WORDS: [u32; PHASES] = [0x3FF << 2 | 3, 0x3FF << 12 | 3, 0x3FF << 22 | 3, 3];

/// The dev dot into the phases in `mask` (bit k = phase k), when built in.
#[inline(always)]
fn fps_dot(mask: u32, width: usize, height: usize) {
    if FPS_DOT {
        for k in 0..PHASES {
            if mask >> k & 1 == 1 {
                phase_slice(k, width * height)[height / 2 * width + width / 2] = FPS_DOT_WORDS[k];
            }
        }
    }
}

/// What each phase frame has not yet been given of the current content, as one rectangle (empty = current): a present writes only the phase on the layer (all four took 71 ms a frame, 2026-10-22) and adds what it changed to the others' rectangles; the idle loop brings the others up to date one at a time; the flip only moves to a phase that is current.
static mut PHASE_DIRTY: [draw::Rect; PHASES] = [draw::Rect { x0: 0, y0: 0, x1: 4096, y1: 4096 }; PHASES];

/// The 8-bit canvas into the phase frames in `mask` (bit k = phase k), over `r` only (widened to 16-pixel steps): NEON, 16 pixels per step (neon.rs), a row at a time through a cached buffer, then wide copies out. The scalar `publish10` below is the reference.
// ---------------------------------------------------------------- the publish workers
//
// A tall publish is cut into stripes of rows that the render thread and the worker cores take until none are left (KERNEL.md "Cores": tiled rendering across the workers — the publish is the first pass split this way; the pattern for the effects to come). Taken, not dealt: a core that is busy or slow (the A715s run the map and touch, and at 1.07 GHz) simply takes fewer, and nothing waits on a share it was given. The render thread waits until every stripe taken has been finished — never gives up on one, so a phase is never left half written (a 20 ms deadline did that under a pan: the late worker then wrote its stale band over newer content, 2026-10-24). A stripe is claimed by a compare-and-swap on (job, next stripe), so a worker that wakes late for a finished job finds the job number changed and claims nothing. Phase frames are non-cacheable and the canvas is inner-shareable: no maintenance between cores.
const PUB_WORKERS: usize = 3;
/// Rows below which one thread does the whole rect (the wake and the wait cost more than a short rect saves).
const PUB_SPLIT_ROWS: i32 = 64;
/// Rows a stripe: small enough that the cores' different rates even out, large enough that claiming is noise.
const PUB_STRIPE: i32 = 32;
static PUB_JOB: AtomicU64 = AtomicU64::new(0);
static PUB_UP: AtomicU64 = AtomicU64::new(0);
/// The current job and its next stripe: job << 32 | stripe.
static PUB_CLAIM: AtomicU64 = AtomicU64::new(0);
/// Stripes of the current job finished (job << 32 | count).
static PUB_FINISHED: AtomicU64 = AtomicU64::new(0);
/// The job: canvas base, width, height, phase mask, the rect, the stripe count.
static mut PUB_PARAMS: (usize, usize, usize, u32, draw::Rect, usize) = (0, 0, 0, 0, draw::Rect::EMPTY, 1);
static mut PUB_TABS: neon::Odt = unsafe { core::mem::zeroed() };
static mut PUB_STACKS: [[u8; 16384]; PUB_WORKERS] = [[0; 16384]; PUB_WORKERS];

/// Take stripes of job `job` until none are left; each one finished is counted. Returns when the job has no stripe left to take (others may still be finishing theirs).
fn pub_take(job: u64) {
    loop {
        let c = PUB_CLAIM.load(Ordering::Acquire);
        if c >> 32 != job & 0xFFFF_FFFF {
            return;
        }
        let (base, w, h, mask, r, n) = unsafe { PUB_PARAMS };
        let i = (c & 0xFFFF_FFFF) as usize;
        if i >= n {
            return;
        }
        if PUB_CLAIM.compare_exchange(c, c + 1, Ordering::AcqRel, Ordering::Relaxed).is_err() {
            continue;
        }
        let y0 = r.y0 + i as i32 * PUB_STRIPE;
        let stripe = draw::Rect { x0: r.x0, y0, x1: r.x1, y1: (y0 + PUB_STRIPE).min(r.y1) };
        let back = unsafe { core::slice::from_raw_parts(base as *const Px, w * h) };
        publish_band(back, w, h, unsafe { &*(&raw const PUB_TABS) }, mask, stripe);
        PUB_FINISHED.fetch_add(1, Ordering::AcqRel);
        ferros_sys::wake(&PUB_FINISHED, 1);
    }
}

extern "C" fn pub_worker(_idx: u64) -> ! {
    PUB_UP.fetch_add(1, Ordering::AcqRel);
    let mut seen = 0u64;
    loop {
        let job = PUB_JOB.load(Ordering::Acquire);
        if job == seen {
            ferros_sys::wait(&PUB_JOB, job, 0);
            continue;
        }
        seen = job;
        pub_take(job);
    }
}

/// The canvas through the present stage into the phases `mask` names, over `r`: in stripes taken by the workers and this thread when tall, else here.
fn publish10x4(back: &[Px], width: usize, height: usize, tabs: &neon::Odt, mask: u32, r: draw::Rect) {
    let r = r.clip(width, height);
    if r.is_empty() {
        return;
    }
    // A publish that is not the clock's own may carry an older clock (the back buffer's, whenever it was drawn): its next redraw is whole.
    if unsafe { CLIP_PHASE }.is_none() {
        for k in 0..PHASES {
            if mask >> k & 1 == 1 {
                unsafe {
                    PHASE_CLOCK[k] = None;
                }
            }
        }
    }
    let workers = PUB_UP.load(Ordering::Acquire) as usize;
    if r.y1 - r.y0 < PUB_SPLIT_ROWS || workers == 0 {
        publish_band(back, width, height, tabs, mask, r);
        fps_dot(mask, width, height);
        return;
    }
    let n = ((r.y1 - r.y0 + PUB_STRIPE - 1) / PUB_STRIPE) as usize;
    let job = PUB_JOB.load(Ordering::Acquire) + 1;
    unsafe {
        PUB_TABS = *tabs;
        PUB_PARAMS = (back.as_ptr() as usize, width, height, mask, r, n);
    }
    PUB_FINISHED.store((job & 0xFFFF_FFFF) << 32, Ordering::Release);
    PUB_CLAIM.store((job & 0xFFFF_FFFF) << 32, Ordering::Release);
    PUB_JOB.store(job, Ordering::Release);
    ferros_sys::wake(&PUB_JOB, PUB_WORKERS as u64);
    pub_take(job);
    // Every stripe taken, finished: no deadline — a stripe in a worker's hands is being written, and leaving now would flip to a half-written phase.
    loop {
        let f = PUB_FINISHED.load(Ordering::Acquire);
        if (f & 0xFFFF_FFFF) as usize >= n {
            break;
        }
        ferros_sys::wait(&PUB_FINISHED, f, now() + freq() / 1000);
    }
    fps_dot(mask, width, height);
}

fn publish_band(back: &[Px], width: usize, height: usize, tabs: &neon::Odt, mask: u32, r: draw::Rect) {
    if r.is_empty() {
        return;
    }
    let (x0, x1) = ((r.x0 as usize) & !15, ((r.x1 as usize + 15) & !15).min(width & !15));
    let mut row = [0u32; 1344];
    for k in 0..PHASES {
        if mask >> k & 1 == 0 {
            continue;
        }
        let dst = phase_slice(k, width * height);
        for y in r.y0 as usize..r.y1 as usize {
            let th = neon::thresholds(y, k, &BAYER, &TEMPORAL_RANK);
            let t_n = now();
            unsafe {
                let thv = core::arch::aarch64::vld1q_u8(th.as_ptr());
                let src = back.as_ptr().add(y * width);
                let mut x = x0;
                while x < x1 {
                    neon::sixteen_canvas(tabs, src.add(x), row.as_mut_ptr().add(x), thv);
                    x += 16;
                }
            }
            let t_c = now();
            dst[y * width + x0..y * width + x1].copy_from_slice(&row[x0..x1]);
            unsafe {
                PUB_TICKS[0] += t_c - t_n;
                PUB_TICKS[1] += now() - t_c;
                PUB_TICKS[2] += ((x1 - x0) as u64) << 0;
            }
        }
    }
}
/// Per second (the frames/s line): the camera ops' scene, -, publish and wait times summed, and their count.
/// [scene, glow, publish, wait, ops, icons drawn, blits, clear].
static mut SEC_US: [u64; 8] = [0; 8];
/// The last camera op's clear of the previous frame's rectangle, µs.
static mut CLEAR_US: u64 = 0;
/// The publish's own split, in counter ticks since the last camera op: the NEON loop, the copy out, and pixels.
static mut PUB_TICKS: [u64; 3] = [1; 3];

/// Content changed inside `r`: every phase owes it; the one on the layer is paid now (with whatever it already owed).
/// A frame into the ring: the next phase in turn, never the one on the layer (which the panel may still be reading), and it becomes the current one. Every frame goes this way — a camera op, an app's frame, the status line — so the frame after can be armed at once (the buffer it writes was on screen three frames before) and consecutive frames carry consecutive phases' thresholds: the temporal dither at whatever rate frames come, 120 Hz under a finger, 10 Hz at rest (the flip below, which publishes nothing once the ring holds the image).
fn publish_next(back: &[Px], width: usize, height: usize, tabs: &neon::Odt, r: draw::Rect) {
    // The glass is the app's while one is in front (op 22 to op 23). The map's frames still draw into the back buffer but reach no phase: one that did put the map in one phase of four and the app in the rest, and the 10 Hz flips showed both (2026-10-27). Op 23 marks every phase dirty, so the map's next frame repaints all of it.
    if unsafe { (*(&raw const APP)).is_some() } {
        return;
    }
    unsafe {
        for k in 0..PHASES {
            PHASE_DIRTY[k] = PHASE_DIRTY[k].union(r);
        }
        let k = (CUR_PHASE + 1) % PHASES;
        publish10x4(back, width, height, tabs, 1 << k, PHASE_DIRTY[k]);
        PHASE_DIRTY[k] = draw::Rect::EMPTY;
        CUR_PHASE = k;
    }
}

/// Each scene circle's rendered pixels (icon, fill, ring — premultiplied, from transparent), kept while its size, turn, colour and icon stay the same, so a pan blits instead of drawing again. Circles over 1100 pixels across are drawn directly. In a 10 MiB arena of the heap; when it fills, everything is drawn again into it from the start.
#[derive(Clone, Copy)]
struct Sprite {
    /// Word offset into the arena, words reserved, side in pixels.
    off: usize,
    cap: usize,
    d: usize,
    key: [i64; 6],
    valid: bool,
}
const NO_SPRITE: Sprite = Sprite { off: 0, cap: 0, d: 0, key: [0; 6], valid: false };
static mut SPRITES: [Sprite; 256] = [NO_SPRITE; 256];
/// Each circle's key in the last frame: a circle whose key just changed (a pinch, a twist) is drawn directly — drawing it into its sprite and blitting would cost more than drawing it — and only cached once it holds still for a frame.
static mut LAST_KEY: [[i64; 6]; 256] = [[0; 6]; 256];
static mut ARENA: usize = 1;
static mut ARENA_NEXT: usize = 1;
/// The arena's size in words: the `display-sprites` region's 32 MiB when granted (a screen packed with circles needs ~20 MB of square sprites; 6 MiB thrashed at 200, 2026-10-24), else 6 MiB of the heap (the 16 MiB below the back buffer, where the 256-px icon mips, 4.4 MB, and the glyph cache, 2 MiB, left no room for 10, 2026-10-23).
static mut ARENA_WORDS: usize = (6 << 20) / 8;
const SPRITE_MAX: usize = 1100;

/// Circle i's sprite for this key: reused if it is current, else drawn (into its own space if that is big enough, otherwise new space, the arena started over when full). The sprite is d × d pixels with the circle at its centre pixel, followed by d words of row extents (first | last << 16, both inclusive; first > last for an empty row). Returns the sprite's words and extents.
fn sprite(i: usize, key: [i64; 6], d: usize, draw_into: impl FnOnce(&mut [Px], usize)) -> (&'static [Px], &'static [Px]) {
    unsafe {
        let words = d * d + d;
        let sp = &mut *(&raw mut SPRITES[i]);
        if !(sp.valid && sp.key == key && sp.d == d) {
            if sp.cap < words {
                if ARENA_NEXT + words > ARENA_WORDS {
                    // Full: every sprite is drawn again from the start of the arena.
                    for k in 0..256 {
                        SPRITES[k].valid = false;
                        SPRITES[k].cap = 0;
                    }
                    ARENA_NEXT = 0;
                }
                sp.off = ARENA_NEXT;
                sp.cap = words;
                ARENA_NEXT += words;
            }
            let buf = core::slice::from_raw_parts_mut((ARENA as *mut Px).add(sp.off), words);
            let (px, ext) = buf.split_at_mut(d * d);
            px.fill(0);
            // A sprite is drawn whole, in its own coordinates: the screen's clip (a widget's redraw) does not apply. Built under it, a sprite came out mostly empty and was cached that way (2026-10-24: black corners where the clock's redraw met its neighbours).
            let clip = draw::CLIP;
            draw::CLIP = draw::Rect { x0: 0, y0: 0, x1: i32::MAX, y1: i32::MAX };
            draw_into(px, d);
            draw::CLIP = clip;
            for (r, e) in ext.iter_mut().enumerate() {
                let row = &px[r * d..(r + 1) * d];
                match (row.iter().position(|&p| p != 0), row.iter().rposition(|&p| p != 0)) {
                    (Some(a), Some(b)) => *e = a as u64 | (b as u64) << 16,
                    _ => *e = 1,
                }
            }
            sp.key = key;
            sp.d = d;
            sp.valid = true;
        }
        let buf = core::slice::from_raw_parts((ARENA as *const Px).add(sp.off), words);
        (&buf[..d * d], &buf[d * d..])
    }
}

/// A sprite under what is there, its top-left at (ox, oy).
fn blit(px: &mut [Px], width: usize, height: usize, spr: &[Px], ext: &[Px], d: usize, ox: i64, oy: i64) {
    let (bx0, by0, bx1, by1) = draw::bounds(width, height);
    for r in 0..d {
        let y = oy + r as i64;
        if y < by0 || y >= by1 {
            continue;
        }
        let (a, b) = ((ext[r] & 0xFFFF) as i64, (ext[r] >> 16) as i64);
        if a > b {
            continue;
        }
        let (x0, x1) = ((ox + a).max(bx0), (ox + b).min(bx1 - 1));
        if x0 > x1 {
            continue;
        }
        let row = &mut px[y as usize * width + x0 as usize..=y as usize * width + x1 as usize];
        let src = &spr[r * d + (x0 - ox) as usize..=r * d + (x1 - ox) as usize];
        // Two at a time (neon.rs `under2`, branch-free), the row's last one alone.
        let n2 = row.len() & !1;
        let mut i = 0;
        while i < n2 {
            unsafe { neon::under2(row.as_mut_ptr().add(i), src.as_ptr().add(i)) };
            i += 2;
        }
        for (p, &q) in row[n2..].iter_mut().zip(src[n2..].iter()) {
            *p = draw::under(*p, q);
        }
    }
}

/// Whether any window is up (WINDOWS.md): the whole screen while one is, `None` while the map has it. The widgets and the map's frames check it.
static mut APP: Option<draw::Rect> = None;
/// The four surfaces' base (the `surfaces` region): surface k is the `SURFACE_BYTES` from `SURFS + k * SURFACE_BYTES`, a full screen of 64-bit pixels laid out at the screen's width.
static mut SURFS: usize = 1;
const SURFACES: usize = 4;
const SURFACE_BYTES: usize = 32 << 20;
/// The porch before each surface's pixel 0 (the kernel's SURFACE_PORCH): clients name their surface by its address past it.
const PORCH: usize = 64;
/// A window (WINDOWS.md), one per surface: the surface's generation when it was shown (the kernel's preference bytes 8-11; a change or a 0 means its process ended), its level (0 an app, 1 input, 2 the system: higher is in front), when it was last shown (later is in front within a level), and the part of the screen it covers. Outside its rectangle, and wherever a pixel has `CLEAR` set, what is behind it shows.
/// Its surface's `format` says what its pixels are (op 22's optional last byte): 0 the display's own working values (gamma 2, white = Q14, bit 63 see-through); 1 fluor's 16-bit `g2t` (`G2T16`), which the display decodes itself so a fluor app hands its buffer over as it drew it (owner, 2026-10-29: fluor the one compositor, passed straight through).
#[derive(Clone, Copy)]
struct Win {
    generation: u8,
    level: u8,
    order: u32,
    rect: draw::Rect,
    format: u8,
    /// Its mode byte (`MODE_MASK`, `OPAQUE`).
    mode: u8,
}
/// A surface of fluor's 16-bit pixels (fluor `pixel::G2t16`): α in bits 48-63, then each channel's darkness (the top code less the channel's VSF `g2t` code, scaled by α) in bits 32-47 (R), 16-31 (G) and 0-15 (B); α 0 is see-through.
const G2T16: u8 = 1;
/// A 16-bit `g2t` code's working value, `16384 × atanh(code / 2¹⁶)` (VSF: light = atanh(code / 2ⁿ)², and gamma 2 is its root), every code, built once at start; the top 79 codes decode past the working value's 4.0 and stop there. Fluor's own present for the display (fluor host/panel.rs `q14_from_g2t16`) is this table.
static mut G2T16_Q14: *const u16 = core::ptr::null();

/// A surface pixel as the display's working value, by its window's format: its own unchanged; a G2T16 one composited over black as fluor's present does (each channel's visible code, the top code less its darkness, scaled by α), thru `G2T16_Q14`, and α 0 see-through.
#[inline]
fn working(p: u64, format: u8) -> u64 {
    if format != G2T16 {
        // A see-through working pixel is clear whatever its channels hold (a client clears by the bit alone and may leave old values under it); the fold adds channels, so they must be nothing.
        return if p >> 63 != 0 { CLEAR } else { p };
    }
    let a = p >> 48;
    if a == 0 {
        return CLEAR;
    }
    let t = unsafe { core::slice::from_raw_parts(G2T16_Q14, 1 << 16) };
    // (65535 − d) × α + 32767 stays under 2³², as fluor's present computes it.
    let ch = |sh: u32| t[(((0xFFFF - (p >> sh & 0xFFFF)) * a + 0x7FFF) / 0xFFFF) as usize] as u64;
    // Its transparency, Q14 in bits 49-62 (`trans`), so a chain can fold what is behind it by what its alpha left; a working surface's own pixels carry only the bit.
    ch(32) | ch(16) << 16 | ch(0) << 32 | (16384 - (a >> 2)) << 49
}
static mut WINS: [Option<Win>; SURFACES] = [None; SURFACES];
static mut RAISES: u32 = 0;
/// Bit 63 of a surface pixel: see-through.
const CLEAR: u64 = 1 << 63;
/// The corners outside the circle (the system's: the clock now, signal and navigation later): the top `CHROME_TOP` rows then the bottom `CHROME_BOT` rows, 64-bit pixels like a surface. Drawn by `chrome_draw`.
static mut CHROME: usize = 1;
static mut CHROME_TOP: usize = 0;
static mut CHROME_BOT: usize = 0;
/// The minute the corners' clock last drew, so it redraws once a minute.
static mut CHROME_KEY: i64 = -1;

/// Surface k's pixels.
fn surface(k: usize, width: usize, height: usize) -> &'static mut [u64] {
    unsafe { core::slice::from_raw_parts_mut((SURFS + k * SURFACE_BYTES + PORCH) as *mut u64, width * height) }
}

/// The surface a client names by its base (its own `dma` of the surface it was given). A message from another process: the base is checked, not trusted.
fn surface_of(base: u64) -> Option<usize> {
    let off = base.wrapping_sub(unsafe { SURFS } as u64) as usize;
    if off % SURFACE_BYTES == PORCH && off / SURFACE_BYTES < SURFACES { Some(off / SURFACE_BYTES) } else { None }
}

/// The windows up, frontmost first: (surface, window), at most four.
fn front_order() -> ([(usize, Win); SURFACES], usize) {
    let mut v = [(0usize, Win { generation: 0, level: 0, order: 0, rect: draw::Rect::EMPTY, format: 0, mode: 0 }); SURFACES];
    let mut n = 0;
    for k in 0..SURFACES {
        if let Some(w) = unsafe { WINS[k] } {
            // Insertion by (level, order), highest first.
            let mut at = n;
            while at > 0 && (v[at - 1].1.level, v[at - 1].1.order) < (w.level, w.order) {
                v[at] = v[at - 1];
                at -= 1;
            }
            v[at] = (k, w);
            n += 1;
        }
    }
    (v, n)
}

/// The layer stack as bands and spans (WINDOWS.md, "The layer stack", 2026-11-02; the geometry is fluor's `layout`, shared with Photon's stacking), rebuilt only when the windows change (`layout_build`), never per frame.
/// A span's chain is the windows covering it, frontmost first, cut after the first opaque one: nothing behind an opaque window is ever read. Publishing a row walks its band's spans: a span with one opaque working window is a copy, a chain of working windows in Normal is the branchless select, anything else (a 16-bit fluor surface with its alpha, a blend mode) is the general fold.
/// At most four windows, so at most nine bands and nine spans a band; the chains index `ORDER`.
type Band = fluor::layout::Band<SURFACES, { 2 * SURFACES + 1 }>;
static mut BANDS: [Band; 2 * SURFACES + 1] = [Band::NONE; 2 * SURFACES + 1];
static mut NBANDS: usize = 0;
/// The layout's layers, frontmost first, as its chains index them: (surface, format, mode).
static mut ORDER: [(u8, u8, u8); SURFACES] = [(0, 0, 0); SURFACES];
/// A window's mode byte (op 22, `ferros_sys::window`): bits 0-2 the blend mode in fluor's order (0 Normal, 1 Multiply, 2 Screen, 3 Add, 4 Subtract, 5 Overlay, 6 Darken, 7 Lighten), bit 3 opaque (no see-through pixel anywhere: the chain stops at it).
const MODE_MASK: u8 = 7;
const OPAQUE: u8 = 8;
fn layout_build(width: usize, height: usize) {
    let (ord, n) = front_order();
    let mut layers = [fluor::layout::Layer { x0: 0, y0: 0, x1: 0, y1: 0, opaque: false }; SURFACES];
    for (i, &(k, w)) in ord[..n].iter().enumerate() {
        layers[i] = fluor::layout::Layer { x0: w.rect.x0, y0: w.rect.y0, x1: w.rect.x1, y1: w.rect.y1, opaque: w.mode & OPAQUE != 0 };
        unsafe { ORDER[i] = (k as u8, w.format, w.mode) };
    }
    unsafe { NBANDS = fluor::layout::layout(&layers[..n], width, height, core::slice::from_raw_parts_mut(&raw mut BANDS as *mut Band, 2 * SURFACES + 1)) };
}

/// A working pixel's transparency, Q14: 0 opaque, 16384 clear (bit 63, the see-through bit every working surface may set), between from bits 49-62 (a decoded fluor surface's alpha; a working surface declares only the bit).
#[inline]
fn trans(p: u64) -> u64 {
    if p >> 63 != 0 { 16384 } else { p >> 49 & 0x3FFF }
}

/// One layer folded under the composite so far (`acc` with transparency `t`), as fluor's `Blend::under` does it front to back, on the working values: each channel gains the mode's value of (composite, layer) by what the layer's alpha fills of what was still transparent, and the transparency is the product. The mode is the one of the layer above (how that layer sits on what is beneath it). Modes are defined against white (Q14 16384) as fluor's are against the top code; brighter working values (the canvas goes to four times white) are taken as the math gives, and a channel saturates at the field: 16 bits is what a working value holds, and Add past the field is the mode's own definition, as fluor's Add saturates at its one.
#[inline]
fn fold(acc: u64, t: u64, p: u64, mode: u8) -> (u64, u64) {
    let tp = trans(p);
    let consumed = (t * (16384 - tp)) >> 14;
    let ch = |sh: u32| -> u64 {
        let (a, b) = ((acc >> sh & 0xFFFF) as i64, (p >> sh & 0xFFFF) as i64);
        // The mode's value on the visible operands: fluor's one definition of the eight modes (BlendMode::visible), white at Q14.
        let m = fluor::pixel::BlendMode::from_byte(mode).visible(a, b, 16384);
        let v = a + (m * consumed as i64 >> 14);
        (if v < 0 { 0 } else if v > 0xFFFF { 0xFFFF } else { v } as u64) << sh
    };
    let t2 = (t * tp) >> 14;
    ((acc & 1 << 48) | ch(0) | ch(16) | ch(32) | t2 << 49 | if t2 == 16384 { 1 << 63 } else { 0 }, t2)
}

/// The glass's own corner radius, in panel pixels: a fact of this panel to be tuned on the device (owner, 2026-11-02: "we'll fine tune the radius on this device to squeeze as much out of it"). Android reports 68 at its 1008-wide frame, 91 at the full raster.
const GLASS_RADIUS: f32 = 80.0;
/// Whether this is a development build (the hairlines, the orange).
const DEV: bool = option_env!("FERROS_PRODUCTION").is_none();
/// Outside the screen's edge: black on release; on development builds the hairline's orange one stop darker (each channel's code over the root of two: half the light at gamma 2), so a dev image is told at a glance and the mask is still a mask.
const EDGE_MASK: u64 = if DEV { 1 << 48 | 11585 | 8192 << 16 } else { 1 << 48 };

/// The glass's shape, planned once per screen size (owner, 2026-11-02: "maintain a global compressed mask since most will be opaque for the OS in general"): the circle an app lives in (the corners outside it are the system's chrome) and the screen's own edge, as Photon draws its window on Android glass (fluor paint/edge.rs `glass_corner_params`, Nick 2026-09-17: a superellipse corner inside the glass's circular arc, cutting one and a half glass insets deeper at the diagonal, exponent 6 or the first lower one whose corners fit, nine tenths of the fitted size; the top-left and bottom-right corners twice the top-right and bottom-left, the 2:1 asymmetry).
/// Per row: the columns of chrome from the left and from the right (the circle's outside), the columns masked from the left and from the right (the edge's outside, over the chrome), then the row's antialiased pixels in the order they apply (column, shape, coverage Q16, hairline alpha Q16): the circle's first, in column order, then the edge's. `GLASS_ROWS` has height + 1 entries; a row's pixels run from its `first` to the next row's. Everything else on a row is glass, untouched: the publish path does no geometry at all, it copies runs and blends the listed pixels.
/// The edge comes from fluor's walk of the corners' crossings (`walk_window_edges`, the one walk Photon's chrome draws with: a four-corner mirror and a single-pixel alias per axis), the circle from its distance field, both at plan time only.
struct GlassRow {
    first: u32,
    chrome_l: u16,
    chrome_r: u16,
    mask_l: u16,
    mask_r: u16,
}
/// A listed pixel's shape: the circle (its base is the chrome under it, its hairline the ring's) or the edge (its base the mask colour, its hairline the edge's).
const SHAPE_CIRCLE: u16 = 0;
const SHAPE_EDGE: u16 = 1;
/// Non-zero initialisers (see `ALLOC`): `plan_glass` sets them.
static mut GLASS_ROWS: usize = 1;
static mut GLASS_PX: usize = 1;
fn plan_glass(width: usize, height: usize) {
    use fluor::paint::edge::{compute_squircle_crossings, glass_corner_params, walk_window_edges};
    let (rs, rb, n) = glass_corner_params(GLASS_RADIUS, width as f32, height as f32);
    let (ss, cs) = compute_squircle_crossings(rs, n);
    let (sb, cb) = compute_squircle_crossings(rb, n);
    // The rows the edge's corners reach, at the top and at the bottom: the big cap and the inner pixel past it. Those rows are kept whole while the walk runs, in the sprite arena (empty at init, 64 MiB; the walk's writes land in any order and a pixel can be written more than once, so the rows are compacted afterwards, not as they come); the rows between get only the straight edges' pixels, which arrive one per row per edge.
    let band = sb + cb.len() + 2;
    let dense = band * width;
    assert!(2 * 2 * 2 * dense <= unsafe { ARENA_WORDS } * 8, "glass plan: the arena is smaller than two bands of clip and hairline");
    let clip = unsafe { core::slice::from_raw_parts_mut(ARENA as *mut u16, 2 * dense) };
    let hair = unsafe { core::slice::from_raw_parts_mut((ARENA as *mut u16).add(2 * dense), 2 * dense) };
    clip.fill(0xFFFF);
    hair.fill(0);
    let mut mid: alloc::vec::Vec<(u32, u16, u16, u16)> = alloc::vec::Vec::new();
    let at = |idx: usize| -> Result<usize, (u32, u16)> {
        let (y, x) = (idx / width, idx % width);
        if y < band {
            Ok(idx)
        } else if y >= height - band {
            Ok((band + y - (height - band)) * width + x)
        } else {
            Err((y as u32, x as u16))
        }
    };
    let mut on_clip = |idx: usize, cov: u32| match at(idx) {
        Ok(d) => clip[d] = cov as u16,
        Err((y, x)) => mid.push((y, x, cov as u16, 0)),
    };
    let mut mid_h: alloc::vec::Vec<(u32, u16, u16, u16)> = alloc::vec::Vec::new();
    let mut on_hair = |idx: usize, _side: fluor::paint::edge::EdgeSide, alpha: u32| match at(idx) {
        // A pixel the walk reports twice (where a corner's row walk and column walk meet) carries both hairline alphas composed one under the other, as the chrome composes them: a + b - ab, the product rounded up so full under full stays full.
        Ok(d) => {
            let u = hair[d] as u32;
            hair[d] = (u + alpha - ((u * alpha + 0xFFFF) >> 16)) as u16;
        }
        Err((y, x)) => mid_h.push((y, x, 0xFFFF, alpha as u16)),
    };
    walk_window_edges(width as u32, height as u32, sb, &cb, ss, &cs, &mut on_clip, &mut on_hair);
    mid.extend_from_slice(&mid_h);
    mid.sort_unstable_by_key(|e| (e.0, e.1));
    // The circle: its centre and radius, the band its edge and hairline touch (a pixel and a half either side of the radius), and the rows that reach outside it (the chrome rows, the only rows the circle's runs and pixels may land on: the chrome buffer holds those rows alone, and a plan that put the circle elsewhere would index past it, loudly).
    let (cx, cy, rad) = circle(width, height);
    let (rin, rout) = ((rad - 1.5) * (rad - 1.5), (rad + 1.5) * (rad + 1.5));
    let (top, bot) = unsafe { (CHROME_TOP, CHROME_BOT) };
    let mut rows: alloc::vec::Vec<GlassRow> = alloc::vec::Vec::with_capacity(height + 1);
    let mut px: alloc::vec::Vec<(u16, u16, u16, u16)> = alloc::vec::Vec::new();
    let mut mi = 0;
    for y in 0..height {
        let first = px.len() as u32;
        let (mut cl, mut cr) = (0usize, width);
        if y < top || y >= height - bot {
            let dy = y as f32 + 0.5 - cy;
            let dy2 = dy * dy;
            // A pixel's coverage by the disc and the hairline's weight at it, from its centre's distance to the edge: coverage 1 with the centre half a pixel or more inside the edge, 0 half a pixel or more outside, the ramp between (by the definition of coverage; the band is wider than that pixel, it holds the hairline too, so the limits are the geometry's, not a guard); the hairline one pixel wide, centred on the radius, its profile (1 - t²)² across it, C¹ at its edges and its centre.
            let pixel = |x: usize| -> (u32, u32) {
                let dx = x as f32 + 0.5 - cx;
                let d2 = dx * dx + dy2;
                if d2 >= rout {
                    (0, 0)
                } else if d2 > rin {
                    let d = libm::sqrtf(d2);
                    let cov = rad + 0.5 - d;
                    let cov = if cov > 1.0 { 1.0 } else if cov < 0.0 { 0.0 } else { cov };
                    let t = d - rad;
                    let hl = if t * t < 1.0 { let u = 1.0 - t * t; u * u } else { 0.0 };
                    ((cov * 65536.0) as u32, (hl * 65536.0) as u32)
                } else {
                    (65536, 0)
                }
            };
            // One crossing per row (owner, 2026-11-02: "count the crossings vertically"): the circle's chord at this row, where its edge band begins and ends; only the band's pixels are looked at. Past the outer crossing is chrome, inside the inner one is glass; where the inner crossing does not exist (the rows at the circle's top and bottom) the band is the whole chord.
            if dy2 >= rout {
                cl = width;
                cr = width;
            } else {
                let half_out = libm::sqrtf(rout - dy2);
                // WHY the screen's bounds: the circle is wider than the screen (its radius exceeds half the width), so on most rows its crossings lie off the glass; the band is cut to the row.
                let (x0, x1) = ((cx - half_out) as i64 - 2, (cx + half_out) as i64 + 2);
                cl = if x0 < 0 { 0 } else { x0 as usize };
                cr = if x1 > width as i64 { width } else { x1 as usize };
                while cl < width && pixel(cl) == (0, 0) {
                    cl += 1;
                }
                while cr > cl && pixel(cr - 1) == (0, 0) {
                    cr -= 1;
                }
                let (li, ri) = if dy2 < rin {
                    let half_in = libm::sqrtf(rin - dy2);
                    let (li, ri) = ((cx - half_in) as i64 + 2, (cx + half_in) as i64 - 2);
                    (if li < cl as i64 { cl } else { li as usize }, if ri > cr as i64 { cr } else { ri as usize })
                } else {
                    (cr, cr)
                };
                let mut band = |x: usize| {
                    let (c, h) = pixel(x);
                    if c != 65536 || h != 0 {
                        px.push((x as u16, SHAPE_CIRCLE, c.min(0xFFFF) as u16, h.min(0xFFFF) as u16));
                    }
                };
                for x in cl..li.min(cr) {
                    band(x);
                }
                for x in li.max(ri)..cr {
                    band(x);
                }
            }
        } else if y == top {
            // The first glass row: the circle's chord is wider than the screen here, by `chrome_rows`'s construction, so no chrome and no circle pixel lies on it or on any row until the bottom's.
            let dx = 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            assert!(dx * dx + dy * dy <= rin, "glass plan: the circle reaches a row the chrome does not hold");
        }
        let (mut ml, mut mr) = (0usize, width);
        let d = if y < band { Some(y) } else if y >= height - band { Some(band + y - (height - band)) } else { None };
        if let Some(d) = d {
            let (c, h) = (&clip[d * width..(d + 1) * width], &hair[d * width..(d + 1) * width]);
            while ml < width && c[ml] == 0 {
                ml += 1;
            }
            while mr > ml && c[mr - 1] == 0 {
                mr -= 1;
            }
            for x in ml..mr {
                if c[x] != 0xFFFF || h[x] != 0 {
                    px.push((x as u16, SHAPE_EDGE, c[x], h[x]));
                }
            }
        } else {
            while mi < mid.len() && mid[mi].0 == y as u32 {
                let e = mid[mi];
                // The same pixel from both lists (never, today: the middle rows carry one pixel per edge): the last clip, the hairlines composed.
                let same = px.len() > first as usize && px[px.len() - 1].0 == e.1 && px[px.len() - 1].1 == SHAPE_EDGE;
                match px.last_mut() {
                    Some(l) if same => {
                        l.2 = e.2.min(l.2);
                        let (u, a) = (l.3 as u32, e.3 as u32);
                        l.3 = (u + a - ((u * a + 0xFFFF) >> 16)) as u16;
                    }
                    _ => px.push((e.1, SHAPE_EDGE, e.2, e.3)),
                }
                mi += 1;
            }
        }
        rows.push(GlassRow { first, chrome_l: cl as u16, chrome_r: cr as u16, mask_l: ml as u16, mask_r: mr as u16 });
    }
    rows.push(GlassRow { first: px.len() as u32, chrome_l: 0, chrome_r: width as u16, mask_l: 0, mask_r: width as u16 });
    unsafe {
        GLASS_ROWS = rows.leak().as_ptr() as usize;
        GLASS_PX = px.leak().as_ptr() as usize;
    }
}

/// The circle an app lives in (ferros_sys::active_circle, shared with the apps that inscribe their pages in it): tangent to the bottom of the camera cutout and to the bottom of the screen, centred, so the corners outside it belong to the system (owner, 2026-11-02). Its centre and radius, in pixels. (Until 2026-11-02 it was tangent to the top of the screen, through the cutout.)
fn circle(width: usize, height: usize) -> (f32, f32, f32) {
    ferros_sys::active_circle(width, height)
}

/// How many rows at the top and at the bottom reach outside the circle: where the circle's chord is narrower than the screen, plus the hairline's own width and a row of slack.
fn chrome_rows(width: usize, height: usize) -> (usize, usize) {
    let (cx, cy, r) = circle(width, height);
    let half = libm::sqrtf(r * r - cx * cx);
    ((cy - half) as usize + 3, (height as f32 - (cy + half)) as usize + 3)
}

/// Whether the screen point is inside the circle.
fn in_circle(x: i64, y: i64, width: usize, height: usize) -> bool {
    let (cx, cy, r) = circle(width, height);
    let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
    dx * dx + dy * dy <= r * r
}

/// The window that owns a screen point: the frontmost whose rectangle holds it and whose pixel there is not see-through, inside the circle (the corners are the system's).
fn owner_at(x: i64, y: i64, width: usize, height: usize) -> Option<usize> {
    if x < 0 || y < 0 || x >= width as i64 || y >= height as i64 || !in_circle(x, y, width, height) {
        return None;
    }
    let (ord, n) = front_order();
    for &(k, w) in &ord[..n] {
        let r = w.rect;
        if (x as i32) >= r.x0 && (x as i32) < r.x1 && (y as i32) >= r.y0 && (y as i32) < r.y1 && working(surface(k, width, height)[y as usize * width + x as usize], w.format) & CLEAR == 0 {
            return Some(k);
        }
    }
    None
}

/// The windows into phase `k` over `r` (screen coordinates): each row composed frontmost first (a pixel comes from the first window that covers it; nothing behind the windows but black), cut to the circle with its edge antialiased, the corners from the chrome; then each pixel's R, G, B (bits 0-15, 16-31, 32-47, working values: gamma 2, white = Q14) through the one table, E's gains and the dither, bit 48 set = no dither for that pixel.
fn publish_app(r: draw::Rect, tabs: &neon::Odt, k: usize, width: usize, height: usize) {
    let r = r.clip(width, height);
    if r.is_empty() {
        return;
    }
    let dst = phase_slice(k, width * height);
    unsafe {
        PHASE_CLOCK[k] = None;
    }
    let (top, bot) = unsafe { (CHROME_TOP, CHROME_BOT) };
    let chrome = unsafe { core::slice::from_raw_parts(CHROME as *const u64, (top + bot) * width) };
    // The hairline's colour (owner, 2026-11-02). Release builds: standard-illuminant-E grey (equal VSF RGB codes) one stop darker than the theme's ring: the ring's photopic light (VSF RGB's weights, as vsf::colour::convert::vsf_rgb_to_photopic_f32 has them: R 0.2584, G 0.6346, B 0.1070, on gamma-2 codes) halved, back to a code. Development builds: bright orange in VSF RGB, full red, half-light green, no blue, so a dev image is told at a glance.
    let line = if option_env!("FERROS_PRODUCTION").is_some() {
        let ring = draw::opaque(ferros_sys::theme::current().ring);
        let lin = |sh: u32| { let c = (ring >> sh & 0xFFFF) as f32 * (1.0 / 16384.0); c * c };
        let light = 0.2584 * lin(0) + 0.6346 * lin(16) + 0.1070 * lin(32);
        let code = (libm::sqrtf(light * 0.5) * 16384.0) as u64;
        1 << 63 | code | code << 16 | code << 32
    } else {
        let g = (libm::sqrtf(0.5) * 16384.0) as u64;
        1 << 63 | 16384 | g << 16
    };
    let mut row = [3u32; 1344];
    let mut px = [0u64; 1344];
    let (xa, xb) = (r.x0 as usize, r.x1 as usize);
    for y in r.y0 as usize..r.y1 as usize {
        // The row's spans (`layout_build`): each one's chain, frontmost first, into the row.
        let bands = unsafe { core::slice::from_raw_parts(&raw const BANDS as *const Band, NBANDS) };
        let order = unsafe { core::slice::from_raw_parts(&raw const ORDER as *const (u8, u8, u8), SURFACES) };
        let band = bands.iter().find(|b| (y as u16) >= b.y0 && (y as u16) < b.y1).unwrap();
        for sp in band.spans() {
            let (a, b) = ((sp.x0 as usize).max(xa), (sp.x1 as usize).min(xb));
            if a >= b {
                continue;
            }
            let chain = sp.chain();
            if chain.is_empty() {
                // Nothing here: black.
                px[a..b].fill(0);
                continue;
            }
            let layer = |i: u8| order[i as usize];
            let plain = chain.iter().all(|&i| layer(i).1 != G2T16 && layer(i).2 & MODE_MASK == 0);
            let (k0, f0, m0) = layer(chain[0]);
            let front = &surface(k0 as usize, width, height)[y * width..(y + 1) * width];
            if m0 & OPAQUE != 0 && f0 != G2T16 {
                px[a..b].copy_from_slice(&front[a..b]);
            } else if plain {
                // Working windows in Normal: see-through is the bit alone, so the fold is a select. Branchless: an uncovered pixel takes this window's, a covered one keeps its own; black where every window is see-through.
                px[a..b].copy_from_slice(&front[a..b]);
                for &i in &chain[1..] {
                    let src = &surface(layer(i).0 as usize, width, height)[y * width..(y + 1) * width];
                    for x in a..b {
                        let p = px[x];
                        let m = (p >> 63).wrapping_neg();
                        px[x] = (p & !m) | (src[x] & m);
                    }
                }
                for p in &mut px[a..b] {
                    *p &= ((*p >> 63) ^ 1).wrapping_neg();
                }
            } else {
                for x in a..b {
                    let mut acc = working(front[x], f0);
                    let mut t = trans(acc);
                    let mut mode = m0;
                    for &i in &chain[1..] {
                        if t == 0 {
                            break;
                        }
                        let (k, f, m) = layer(i);
                        let p = working(surface(k as usize, width, height)[y * width + x], f);
                        (acc, t) = fold(acc, t, p, mode);
                        mode = m;
                    }
                    // What is still transparent at the end is over black: the premultiplied channels are the light, the transparency is spent.
                    px[x] = acc & !(0x7FFF << 49);
                }
            }
        }
        // The glass's shape, from the plan (`plan_glass`): the circle's outside copies the chrome row, the edge's outside takes the mask colour (black on release builds; the hairline's orange one stop darker on development builds), and the listed pixels take their hairline at its alpha, then their base by their coverage's complement. The circle's hairline is `line`; the edge's is the orange on development builds and on release builds the mask colour itself, as Photon's release build on Android draws its hairline black so the glass's corner is black to the curve. A chrome row is read only where the plan put chrome; elsewhere it is empty and a wrong plan would say so.
        {
            let rows = unsafe { core::slice::from_raw_parts(GLASS_ROWS as *const GlassRow, height + 1) };
            let g = &rows[y];
            let (first, next) = (g.first as usize, rows[y + 1].first as usize);
            let (cl, cr, ml, mr) = (g.chrome_l as usize, g.chrome_r as usize, g.mask_l as usize, g.mask_r as usize);
            if cl > 0 || cr < width || ml > 0 || mr < width || first != next {
                let crow = if y < top { &chrome[y * width..(y + 1) * width] } else if y >= height - bot { &chrome[(top + y - (height - bot)) * width..(top + y - (height - bot) + 1) * width] } else { &chrome[..0] };
                let edge_line = if DEV { line } else { EDGE_MASK };
                let mix = |a: u64, b: u64, wb: u64| {
                    let ch = |sh: u32| (((a >> sh & 0xFFFF) * (65536 - wb) + (b >> sh & 0xFFFF) * wb) >> 16) << sh;
                    (b & 1 << 48) | ch(0) | ch(16) | ch(32)
                };
                let q = |v: u16| if v == 0xFFFF { 65536 } else { v as u64 };
                // Coverage is an area, so it weighs light, and a working value is a gamma-2 code: the covered mix is on the squares, back to a code by the root (what the circle did before the plan: the value by the root of its coverage, over black). The hairline's weight stays on the codes, as it was drawn and judged (owner, 2026-11-02).
                let mixl = |a: u64, b: u64, wb: u64| {
                    let ch = |sh: u32| {
                        let (la, lb) = ((a >> sh & 0xFFFF) * (a >> sh & 0xFFFF), (b >> sh & 0xFFFF) * (b >> sh & 0xFFFF));
                        (libm::sqrt(((la * (65536 - wb) + lb * wb) >> 16) as f64) as u64) << sh
                    };
                    (b & 1 << 48) | ch(0) | ch(16) | ch(32)
                };
                px[xa..xb.min(cl)].copy_from_slice(&crow[xa..xb.min(cl)]);
                if xa.max(cr) < xb {
                    px[xa.max(cr)..xb].copy_from_slice(&crow[xa.max(cr)..xb]);
                }
                // In the order the shapes stack: the circle over the windows and its chrome, the edge over everything. The row's circle pixels come first in the list, so the mask runs go in between.
                let plan = unsafe { core::slice::from_raw_parts(GLASS_PX as *const (u16, u16, u16, u16), next) };
                let row_px = &plan[first..next];
                let split = row_px.iter().position(|e| e.1 == SHAPE_EDGE).unwrap_or(row_px.len());
                for &(x, _, c, h) in &row_px[..split] {
                    let x = x as usize;
                    if x < xa || x >= xb {
                        continue;
                    }
                    let p = px[x];
                    // The circle: coverage first (the content over the chrome, in light), the hairline over both sides after (on the codes, as before the plan: a pixel centred on the radius carries the whole line, a straddling pair its profile on each side).
                    px[x] = mix(mixl(crow[x], p, q(c)), line, q(h));
                }
                for x in xa..xb.min(ml) {
                    px[x] = EDGE_MASK;
                }
                for x in xa.max(mr)..xb {
                    px[x] = EDGE_MASK;
                }
                for &(x, _, c, h) in &row_px[split..] {
                    let x = x as usize;
                    if x < xa || x >= xb {
                        continue;
                    }
                    let p = px[x];
                    // The edge: both mixes in light (owner, 2026-11-02: "as long as the edge renders gamma proper-ish and trims it to black on production"); on a production build the hairline is the mask's black, so the two pixels are one coverage ramp of the content toward black.
                    px[x] = mixl(EDGE_MASK, mixl(p, edge_line, q(h)), q(c));
                }
            }
        }
        // A screencap (op 30): the finished row, in working values, into the capture surface instead of through the panel's table.
        if unsafe { CAPTURE } != 0 {
            let cap = unsafe { core::slice::from_raw_parts_mut(CAPTURE as *mut u64, width * height) };
            cap[y * width + xa..y * width + xb].copy_from_slice(&px[xa..xb]);
            continue;
        }
        let th = neon::thresholds(y, k, &BAYER, &TEMPORAL_RANK);
        unsafe {
            let base = core::arch::aarch64::vld1q_u8(th.as_ptr());
            let mut x = xa;
            while x + 16 <= xb {
                neon::sixteen_app(tabs, px.as_ptr().add(x), row.as_mut_ptr().add(x), neon::thresholds_at(base, x));
                x += 16;
            }
            if x < xb {
                let n = xb - x;
                let mut pin = [0u64; 16];
                let mut pout = [0u32; 16];
                pin[..n].copy_from_slice(&px[x..xb]);
                neon::sixteen_app(tabs, pin.as_ptr(), pout.as_mut_ptr(), neon::thresholds_at(base, x));
                row[x..xb].copy_from_slice(&pout[..n]);
            }
        }
        dst[y * width + xa..y * width + xb].copy_from_slice(&row[xa..xb]);
    }
    if unsafe { CAPTURE } != 0 {
        return;
    }
    fps_dot(1 << k, width, height);
}

/// Where op 30 has the finished frame written instead of a phase (a free surface's pixels), 0 when not capturing.
static mut CAPTURE: usize = 0;

/// After a window left: every phase owes the whole screen; with none left, the map has the screen again and its next frame repaints all of it.
fn windows_changed(width: usize, height: usize) {
    let full = draw::Rect::full(width, height);
    layout_build(width, height);
    unsafe {
        PHASE_DIRTY = [full; PHASES];
        if (*(&raw const WINS)).iter().all(|w| w.is_none()) {
            APP = None;
            PREV_DMG = full;
        }
    }
}

/// The corners' circles (WINDOWS.md, "The corners", owner 2026-11-03: "tangent on all sides, three for each"): in each corner a middle circle, the largest that fits between the glass's own edge (its fat corner, exactly as `plan_glass` drew it) and the app's circle, so tangent to both; and on each side of it one more, tangent to the glass, the app's circle and the middle one. Planned once per screen size from the glass plan, then cached.
/// The fit: `clear(p)` is a point's distance to the nearer of the two (the glass boundary, from each row's masked columns and the screen's own edges; the app's circle, |p − C| − R). The middle circle maximises it (a coarse grid, then a fine one around the best). A side circle's centre lies where the two distances are equal (between the glass and the app's circle), found by bisection across the corner's strip at each step along the edge, and is the point there whose clearance also equals its distance to the middle circle's edge: the root of that difference, stepped out from the middle circle and bisected.
#[derive(Clone, Copy, PartialEq)]
enum Corner {
    Clock,
    Cellular,
    Wifi,
    Bluetooth,
    Nfc,
    Gnss,
    Usb,
    Battery,
    Home,
    Spare,
}

/// The glass boundary's distance from a point: the masked columns of each row near it (left and right), and the screen's top and bottom edges.
fn glass_dist(rows: &[GlassRow], width: usize, height: usize, x: f32, y: f32, reach: f32) -> f32 {
    // Outside the glass on its own row (in a masked corner, or off the screen): no clearance at all. Unsigned, a point beyond the corner's curve read as roomy and the bottom-right circle was solved out in the mask (2026-11-03).
    if y < 0.0 || y >= height as f32 || x < 0.0 || x >= width as f32 {
        return -1.0;
    }
    let own = &rows[y as usize];
    if x < own.mask_l as f32 || x > own.mask_r as f32 {
        return -1.0;
    }
    let mut d = y.min(height as f32 - y).min(x).min(width as f32 - x);
    let y0 = (y - reach).max(0.0) as usize;
    let y1 = ((y + reach) as usize).min(height - 1);
    for yy in y0..=y1 {
        let dy = y - (yy as f32 + 0.5);
        // The row's glass runs from column mask_l to column mask_r (both from the left; mask_r is the width where the row has no right mask).
        let (l, r) = (rows[yy].mask_l as f32, rows[yy].mask_r as f32);
        let dl = x - l;
        let dr = r - x;
        let dl2 = libm::sqrtf(dl * dl + dy * dy);
        let dr2 = libm::sqrtf(dr * dr + dy * dy);
        if dl2 < d { d = dl2; }
        if dr2 < d { d = dr2; }
    }
    d
}

/// The screen's corner circles: (x, y, radius, what), up to sixteen.
static mut CORNERS: [(f32, f32, f32, Corner); 16] = [(0.0, 0.0, 0.0, Corner::Spare); 16];
static mut CORNERS_N: usize = 0;
static mut CORNERS_FOR: (usize, usize) = (0, 0);

fn corner_layout(width: usize, height: usize) -> ([(f32, f32, f32, Corner); 16], usize) {
    unsafe {
        if CORNERS_FOR == (width, height) {
            return (CORNERS, CORNERS_N);
        }
    }
    let rows = unsafe { core::slice::from_raw_parts(GLASS_ROWS as *const GlassRow, height + 1) };
    let (cx, cy, rr) = circle(width, height);
    let (w, h) = (width as f32, height as f32);
    let reach = h / 8.0;
    let app = |x: f32, y: f32| libm::sqrtf((x - cx) * (x - cx) + (y - cy) * (y - cy)) - rr;
    let glass = |x: f32, y: f32| glass_dist(rows, width, height, x, y, reach);
    let clear = |x: f32, y: f32| { let (a, g) = (app(x, y), glass(x, y)); if a < g { a } else { g } };
    let mut out = [(0.0f32, 0.0f32, 0.0f32, Corner::Spare); 16];
    let mut n = 0;
    // Each corner: (left?, top?, middle, along the horizontal edge, along the vertical edge, the next along the horizontal edge). The indicators up top, navigation at the bottom (owner, 2026-11-03); IP is no radio, its measures are the links'. Bluetooth has more to show than NFC, so it takes the larger place, the next along the top (owner, 2026-11-04).
    let jobs = [
        (true, true, Corner::Clock, Corner::Battery, Corner::Usb, Some(Corner::Gnss)),
        (false, true, Corner::Cellular, Corner::Wifi, Corner::Nfc, Some(Corner::Bluetooth)),
        (true, false, Corner::Spare, Corner::Spare, Corner::Spare, None),
        (false, false, Corner::Home, Corner::Spare, Corner::Spare, None),
    ];
    for &(left, top, mid_what, hor_what, ver_what, next_what) in &jobs {
        // The corner's quarter of the screen, as far as the app's circle leaves room: the strip above (below) the circle's chord at the screen's edge.
        let strip = if top { cy - libm::sqrtf(rr * rr - cx * cx) } else { h - (cy + libm::sqrtf(rr * rr - cx * cx)) };
        let (xa, xb) = if left { (0.0, w * 0.5) } else { (w * 0.5, w) };
        let (ya, yb) = if top { (0.0, strip) } else { (h - strip, h) };
        // The middle circle: coarse, then fine.
        let (mut bx, mut by, mut br) = (0.0f32, 0.0f32, -1.0f32);
        let step = 4.0;
        let mut y = ya + step * 0.5;
        while y < yb {
            let mut x = xa + step * 0.5;
            while x < xb {
                let c = clear(x, y);
                if c > br { (bx, by, br) = (x, y, c); }
                x += step;
            }
            y += step;
        }
        let (cx0, cy0) = (bx, by);
        for j in 0..33 {
            for i in 0..33 {
                let (x, y) = (cx0 + (i as f32 - 16.0) * 0.25, cy0 + (j as f32 - 16.0) * 0.25);
                let c = clear(x, y);
                if c > br { (bx, by, br) = (x, y, c); }
            }
        }
        out[n] = (bx, by, br, mid_what);
        n += 1;
        // A side circle: along `horizontal` (x moves toward the screen's middle) or the vertical edge (y moves toward the app's circle), the centre on the equal-distance line (so tangent to the glass and the app's circle), from `s0` on until its clearance meets the edge of the circle `prev`, the one it chains from.
        let side = |horizontal: bool, prev: (f32, f32, f32), s0: f32| -> Option<(f32, f32, f32)> {
            let dir = if horizontal { if left { 1.0 } else { -1.0 } } else if top { 1.0 } else { -1.0 };
            // The equal-distance point at a coordinate `s` along the edge: bisection across the strip, between the edge and the middle circle's centre line (the strip only narrows away from the middle circle).
            let medial = |s: f32| -> (f32, f32) {
                let (mut lo, mut hi) = if horizontal { (if top { 0.5 } else { h - 0.5 }, by) } else { (if left { 0.5 } else { w - 0.5 }, bx) };
                for _ in 0..40 {
                    let mid = (lo + hi) * 0.5;
                    let (x, y) = if horizontal { (s, mid) } else { (mid, s) };
                    if glass(x, y) < app(x, y) { lo = mid } else { hi = mid }
                }
                let m = (lo + hi) * 0.5;
                if horizontal { (s, m) } else { (m, s) }
            };
            let gap = |s: f32| -> f32 {
                let (x, y) = medial(s);
                let c = glass(x, y);
                libm::sqrtf((x - prev.0) * (x - prev.0) + (y - prev.1) * (y - prev.1)) - prev.2 - c
            };
            let (mut a, mut found) = (s0, false);
            let mut s = s0 + dir;
            for _ in 0..2048 {
                if gap(s) >= 0.0 {
                    found = true;
                    break;
                }
                a = s;
                s += dir;
            }
            if !found {
                return None;
            }
            let (mut lo, mut hi) = (a, s);
            for _ in 0..30 {
                let mid = (lo + hi) * 0.5;
                if gap(mid) >= 0.0 { hi = mid } else { lo = mid }
            }
            let (x, y) = medial(hi);
            Some((x, y, glass(x, y)))
        };
        let mid = (bx, by, br);
        let hor = side(true, mid, bx);
        if let Some(c) = hor {
            out[n] = (c.0, c.1, c.2, hor_what);
            n += 1;
        }
        if let Some(c) = side(false, mid, by) {
            out[n] = (c.0, c.1, c.2, ver_what);
            n += 1;
        }
        // The next along the top: tangent to the glass, the app's circle and the side circle before it.
        if let (Some(what), Some(c)) = (next_what, hor) {
            if let Some(d) = side(true, c, c.0) {
                out[n] = (d.0, d.1, d.2, what);
                n += 1;
            }
        }
    }
    // The solved circles, once per size, to the console: (x, y, r) in pixels.
    let mut b = [0u8; 512];
    let mut k = 0;
    let mut put = |t: &[u8]| { for &c in t { if k < b.len() { b[k] = c; k += 1; } } };
    put(b"display: corners");
    for c in &out[..n] {
        put(b" (");
        for (j, v) in [c.0, c.1, c.2].iter().enumerate() {
            let mut d = [0u8; 8];
            let (mut m, mut i) = (if *v > 0.0 { *v as u32 } else { 0 }, 8);
            loop { i -= 1; d[i] = b'0' + (m % 10) as u8; m /= 10; if m == 0 { break; } }
            if *v < 0.0 { put(b"-"); }
            put(&d[i..]);
            if j < 2 { put(b","); }
        }
        put(b")");
    }
    put(b"\n");
    ferros_sys::console(unsafe { PANIC_GRANTS }, &b[..k]);
    unsafe {
        CORNERS = out;
        CORNERS_N = n;
        CORNERS_FOR = (width, height);
    }
    (out, n)
}

/// Each radio's flavour (owner, 2026-11-03: "throw some of that in the design"), picked in VSF RGB and scaled to the light of the grey they replace by VSF's photopic weights (R 0.2584, G 0.6346, B 0.1070, vsf_rgb_to_photopic_f32): the ring at RING_ON's light (0.3017 linear), the name at INK's (0.4507; periwinkle tops out at 0.388, its blue full). As working values, gamma 2 white at Q14. Cellular amber, a warm thing that carries far; WiFi sea teal, the near water; Bluetooth periwinkle; NFC lime, a touch; GNSS dusk magenta, the sky it listens to. The rest grey, equal codes (illuminant E).
fn flavour(what: Corner) -> ((f32, f32, f32), (f32, f32, f32)) {
    const RING_ON: f32 = 9000.0;
    const INK: f32 = 11000.0;
    match what {
        Corner::Cellular => ((12107.0, 8121.0, 3424.0), (14797.0, 9926.0, 4185.0)),
        Corner::Wifi => ((2706.0, 10480.0, 9373.0), (3307.0, 12809.0, 11456.0)),
        Corner::Bluetooth => ((8549.0, 7915.0, 14451.0), (9693.0, 8974.0, 16384.0)),
        Corner::Nfc => ((6893.0, 10276.0, 3980.0), (8425.0, 12560.0, 4864.0)),
        Corner::Gnss => ((12831.0, 6415.0, 10735.0), (15682.0, 7841.0, 13121.0)),
        _ => ((RING_ON, RING_ON, RING_ON), (INK, INK, INK)),
    }
}

/// A line of a corner's text, on the stack (the chrome draws without the heap's growth). 32 bytes hold the longest line `measure_text` makes ("kind 65535: " and a 48-bit value's 15 digits with its sign, 28 bytes), so a write past it is a bug there and panics.
#[derive(Clone, Copy)]
struct Txt {
    b: [u8; 32],
    n: usize,
}
impl Txt {
    const fn new() -> Txt {
        Txt { b: [0; 32], n: 0 }
    }
    fn s(&mut self, t: &str) -> &mut Txt {
        for &c in t.as_bytes() {
            self.b[self.n] = c;
            self.n += 1;
        }
        self
    }
    /// `v / unit` with `places` digits after the point, rounded half away from zero.
    fn num(&mut self, v: i64, unit: i64, places: u32) -> &mut Txt {
        let p = 10i64.pow(places);
        let scaled = (v.unsigned_abs() as i128 * p as i128 * 2 + unit as i128) / (unit as i128 * 2);
        if v < 0 && scaled != 0 {
            self.s("-");
        }
        let (whole, frac) = ((scaled / p as i128) as u64, (scaled % p as i128) as u64);
        let mut d = [0u8; 20];
        let (mut m, mut i) = (whole, 20);
        loop {
            i -= 1;
            d[i] = b'0' + (m % 10) as u8;
            m /= 10;
            if m == 0 {
                break;
            }
        }
        self.s(core::str::from_utf8(&d[i..]).unwrap_or(""));
        if places > 0 {
            self.s(".");
            let mut f = [0u8; 8];
            let mut m = frac;
            for k in (0..places as usize).rev() {
                f[k] = b'0' + (m % 10) as u8;
                m /= 10;
            }
            self.s(core::str::from_utf8(&f[..places as usize]).unwrap_or(""));
        }
        self
    }
    fn str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
}

/// One measure as a person reads it: the value in its unit, the kind named where the unit alone does not say it.
fn measure_text(t: &mut Txt, kind: u16, v: i64) {
    use ferros_sys::measure as k;
    let _ = match kind {
        k::RSSI => t.num(v, 1000, 0).s(" dBm"),
        k::NOISE => t.s("noise ").num(v, 1000, 0),
        k::SNR => t.s("SNR ").num(v, 1000, 0).s(" dB"),
        k::RSRP => t.s("RSRP ").num(v, 1000, 0),
        k::RSRQ => t.s("RSRQ ").num(v, 1000, 1),
        k::SINR => t.s("SINR ").num(v, 1000, 0).s(" dB"),
        k::VSWR => t.s("VSWR ").num(v, 1000, 2),
        k::TX_POWER => t.s("tx ").num(v, 1000, 0).s(" dBm"),
        k::FREQ if v >= 1_000_000 => t.num(v, 1_000_000, 3).s(" GHz"),
        k::FREQ if v >= 100_000 => t.num(v, 1000, 0).s(" MHz"),
        k::FREQ => t.num(v, 1000, 2).s(" MHz"),
        k::BANDWIDTH => t.num(v, 1000, 0).s(" MHz wide"),
        k::CHANNEL => t.s("ch ").num(v, 1, 0),
        k::RATE_DOWN | k::RATE_UP => {
            if kind == k::RATE_UP {
                t.s("up ");
            }
            if v >= 1_000_000_000 { t.num(v, 1_000_000_000, 2).s(" Gb/s") } else if v >= 1_000_000 { t.num(v, 1_000_000, 0).s(" Mb/s") } else { t.num(v, 1000, 0).s(" kb/s") }
        }
        k::LATENCY => t.num(v, 1_000_000, 1).s(" ms"),
        k::LOSS => t.num(v, 10_000, 2).s(" % loss"),
        k::MCS => t.s("MCS ").num(v, 1, 0),
        k::STREAMS => t.num(v, 1, 0).s(" streams"),
        k::SATS_SEEN => t.num(v, 1, 0).s(" seen"),
        k::SATS_USED => t.num(v, 1, 0).s(" in fix"),
        k::CN0 => t.num(v, 1000, 0).s(" dB-Hz"),
        _ => t.s("kind ").num(kind as i64, 1, 0).s(": ").num(v, 1, 0),
    };
}

/// A disc's coverage at signed distance `d` (pixels inside the edge, negative outside): the algebraic sigmoid x/√(1+x²) over about a pixel, C∞ (no clamp, GUI continuity).
#[inline]
fn soft(d: f32) -> f32 {
    let x = 2.0 * d;
    0.5 + 0.5 * x / libm::sqrtf(1.0 + x * x)
}

/// The corners' circles into the chrome when the minute or a radio's state changed (or `force`). Returns whether it drew.
fn chrome_draw(width: usize, height: usize, force: bool) -> bool {
    let t_all = now();
    let mut tb = [0u8; 32];
    let (_, _, tn, key, _) = clock_state(&mut tb);
    let facts = ferros_sys::facts().0;
    let radios = facts[32];
    // The battery's (facts 1-4) and the links' measures (facts 33-80) folded into the key too: the chrome redraws when one changes, at most every two seconds (the drivers' pace).
    let mut mh = 0u64;
    for &m in facts[1..5].iter().chain(&facts[33..81]) {
        mh = (mh.rotate_left(7) ^ m).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    let k = ((key / 60) << 8 | (radios & 0x7F) as i64) ^ (mh >> 1) as i64;
    if !force && k == unsafe { CHROME_KEY } {
        return false;
    }
    unsafe { CHROME_KEY = k };
    let (rows, bot) = unsafe { (CHROME_TOP, CHROME_BOT) };
    let chrome = unsafe { core::slice::from_raw_parts_mut(CHROME as *mut u64, (rows + bot) * width) };
    chrome.fill(0);
    // A screen pixel's place in the chrome: the top rows, then the bottom rows.
    let at = |x: i64, y: i64| -> Option<usize> {
        if x < 0 || x >= width as i64 || y < 0 {
            return None;
        }
        let y = y as usize;
        if y < rows {
            Some(y * width + x as usize)
        } else if y >= height - bot && y < height {
            Some((rows + y - (height - bot)) * width + x as usize)
        } else {
            None
        }
    };
    // Working values (gamma 2, white at Q14): the face, its ring when the thing is live and when it is not, the label.
    const FACE: f32 = 1800.0;
    const RING_ON: f32 = 9000.0;
    const RING_OFF: f32 = 3200.0;
    const INK: f32 = 11000.0;
    let line = tb[..tn].split(|&c| c == b'\n').next().unwrap_or(&[]);
    let mut colons = 0;
    let end = line.iter().position(|&c| { colons += (c == b':') as u32; colons == 2 }).unwrap_or(line.len());
    let time = core::str::from_utf8(&line[..end]).unwrap_or("");
    let (circles, n) = corner_layout(width, height);
    for &(x, y, r, what) in &circles[..n] {
        let live = match what {
            Corner::Clock | Corner::Home => true,
            Corner::Cellular => radios & 1 != 0,
            Corner::Wifi => radios & 2 != 0,
            Corner::Bluetooth => radios & 4 != 0,
            Corner::Nfc => radios & 8 != 0,
            Corner::Gnss => radios & 16 != 0,
            // The usb driver's own word (its measure STATE): connected once the host configured it.
            Corner::Usb => ferros_sys::measure_of(facts[33 + 8 * 5]) == Some((ferros_sys::measure::STATE, 2)),
            // The gauge answered (facts 1-4, the kernel's two-second poll of the MAX77759 on hsi2c_11).
            Corner::Battery => facts[1] != u64::MAX,
            Corner::Spare => false,
        };
        let ring_w = r / 16.0;
        // The radio's state as its driver says it (measure STATE): connected the flavour's full ring, up and searching one stop darker; a driver that says nothing, its ring by whether it runs.
        let state = match what {
            Corner::Cellular => Some(0),
            Corner::Wifi => Some(1),
            Corner::Bluetooth => Some(2),
            Corner::Nfc => Some(3),
            Corner::Gnss => Some(4),
            Corner::Usb => Some(5),
            _ => None,
        }
        .and_then(|ri: usize| facts[33 + 8 * ri..41 + 8 * ri].iter().filter_map(|&m| ferros_sys::measure_of(m)).find(|m| m.0 == ferros_sys::measure::STATE).map(|m| m.1));
        let (ring_c, ink_c) = flavour(what);
        let ring = if !live {
            (RING_OFF, RING_OFF, RING_OFF)
        } else if state == Some(1) {
            (ring_c.0 * core::f32::consts::FRAC_1_SQRT_2, ring_c.1 * core::f32::consts::FRAC_1_SQRT_2, ring_c.2 * core::f32::consts::FRAC_1_SQRT_2)
        } else {
            ring_c
        };
        let t_disc = now();
        let (x0, x1, y0, y1) = ((x - r - 2.0) as i64, (x + r + 2.0) as i64, (y - r - 2.0) as i64, (y + r + 2.0) as i64);
        for py in y0..=y1 {
            for px in x0..=x1 {
                let Some(i) = at(px, py) else { continue };
                let (dx, dy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
                let d = r - libm::sqrtf(dx * dx + dy * dy);
                // The disc, then the ring over it: the ring is the band within ring_w of the edge.
                let disc = soft(d);
                let band = disc - soft(d - ring_w);
                // Over what is there already, by the disc's own coverage: a neighbour's pixels outside this disc stay as they were.
                let ch = |sh: u32, ring: f32| ((((chrome[i] >> sh) & 0xFFFF) as f32 * (1.0 - disc) + FACE * (disc - band) + ring * band) as u64) << sh;
                chrome[i] = ch(0, ring.0) | ch(16, ring.1) | ch(32, ring.2);
            }
        }
        unsafe { CHROME_US[0] += ferros_sys::us_since(t_disc) };
        // The radio's measures, as its driver published them (facts 33 + 8 · radio): every one shown as itself, in its unit, a line each; none folded into a level, no bars (owner, 2026-11-03).
        let radio = match what {
            Corner::Cellular => Some(0),
            Corner::Wifi => Some(1),
            Corner::Bluetooth => Some(2),
            Corner::Nfc => Some(3),
            Corner::Gnss => Some(4),
            Corner::Usb => Some(5),
            _ => None,
        };
        let mut lines = [Txt::new(); 9];
        let mut nl = 1;
        // The battery: its charge as the name, then the cell's voltage, the current (+ charging), their product and the temperature, as the gauge measures them (facts 1-4: per cent, hundredths of a degree, µV, µA).
        let mut charge = Txt::new();
        if what == Corner::Battery && facts[1] != u64::MAX {
            charge.num(facts[1] as i64, 1, 0).s(" %");
            let (cc, uv, ua) = (facts[2] as i64, facts[3] as i64, facts[4] as i64);
            lines[nl].num(uv, 1_000_000, 3).s(" V");
            nl += 1;
            if ua >= 0 {
                lines[nl].s("+");
            }
            lines[nl].num(ua, 1000, 0).s(" mA");
            nl += 1;
            lines[nl].num(uv * ua / 1_000_000, 1_000_000, 2).s(" W");
            nl += 1;
            lines[nl].num(cc, 100, 1).s(" \u{B0}C");
            nl += 1;
        }
        let mut tech = None;
        if let Some(ri) = radio {
            for &m in &facts[33 + 8 * ri..41 + 8 * ri] {
                let Some((kind, v)) = ferros_sys::measure_of(m) else { continue };
                match kind {
                    ferros_sys::measure::STATE => {}
                    ferros_sys::measure::TECH => tech = Some(v),
                    _ => {
                        measure_text(&mut lines[nl], kind, v);
                        nl += 1;
                    }
                }
            }
        }
        let name: &str = match what {
            Corner::Clock => time,
            Corner::Cellular => match tech {
                Some(14) => "LTE",
                Some(20) => "NR",
                _ => "cell",
            },
            Corner::Wifi => "wifi",
            Corner::Bluetooth => "bt",
            Corner::Nfc => "nfc",
            Corner::Gnss => "gnss",
            Corner::Usb => "usb",
            Corner::Battery if charge.n > 0 => charge.str(),
            Corner::Battery => "batt",
            Corner::Home | Corner::Spare => "",
        };
        if name.is_empty() {
            continue;
        }
        lines[0].s(name);
        let eng = glyph_engine();
        let dim = RING_OFF * 1.4;
        // The name in the radio's flavour, its measures in plain ink; all dim when it does not run.
        let ink_of = |j: usize| if !live { (dim, dim, dim) } else if j == 0 { ink_c } else { (INK, INK, INK) };
        // The lines stacked about the centre, a pitch each; each line as large as fits its own chord (seven tenths of the circle's width at the line's far edge), the name half again as large as a measure.
        let weight = |j: usize| if j == 0 && nl > 1 { 1.5 } else { 1.0 };
        let total: f32 = (0..nl).map(weight).sum();
        let pitch = if nl == 1 { r * 0.66 } else { r * 1.5 / total };
        let mut top_y = y - pitch * total * 0.5;
        for j in 0..nl {
            let text = lines[j].str();
            let lh = pitch * weight(j);
            let far = libm::fabsf(top_y - y).max(libm::fabsf(top_y + lh - y));
            // The lines span 1.5 r about the centre, so a line's far edge is at most 0.75 r out and the root's argument stays positive.
            let chord = 2.0 * libm::sqrtf(r * r - far * far) * 0.7;
            let mut px = lh * 0.9;
            let tw0 = eng.width(text, px);
            if tw0 > chord {
                px *= chord / tw0;
            }
            let tw = eng.width(text, px);
            let base = top_y + lh * 0.5 + px * 0.36;
            let ink = ink_of(j);
            eng.cover(text, x - tw * 0.5, base, px, (1.0, 0.0), |gx, gy, a| {
                if let Some(i) = at(gx, gy) {
                    let a = a as f32 / 255.0;
                    let ch = |sh: u32, ink: f32| { let p = ((chrome[i] >> sh) & 0xFFFF) as f32; ((p + (ink - p) * a) as u64) << sh };
                    chrome[i] = ch(0, ink.0) | ch(16, ink.1) | ch(32, ink.2);
                }
            });
            top_y += lh;
        }
    }
    ALLOC.reset();
    unsafe {
        if CHROME_US[1] == 0 {
            CHROME_US[1] = ferros_sys::us_since(t_all);
        }
    }
    true
}
/// The rectangle the last frame drew (cleared before the next one draws), and the one this frame is drawing.
static mut PREV_DMG: draw::Rect = draw::Rect { x0: 0, y0: 0, x1: 4096, y1: 4096 };
static mut CUR_DMG: draw::Rect = draw::Rect::EMPTY;

/// A 10-bit pixel as the hardware takes it once DMA, DPP and HDR-common agree (kernel `Panel::set_10bit`): B[31:22] G[21:12] R[11:2] A[1:0].
#[inline(always)]
fn pack10(r: u32, g: u32, b: u32) -> u32 {
    b << 22 | g << 12 | r << 2 | 3
}

/// The 8-bit canvas through the present stage with the spatial dither only (temporal off): the ODT 16 pixels a step (neon.rs `sixteen_odt`) with this row's Bayer thresholds, a row at a time through a cached buffer, then one wide copy into the non-cacheable framebuffer.
fn publish10(back: &[Px], screen: &mut [u32], width: usize, tabs: &neon::Odt) {
    let mut row = [0u32; 4096];
    let w16 = width & !15;
    for (y, (src, dst)) in back.chunks(width).zip(screen.chunks_mut(width)).enumerate() {
        let mut th = [0u8; 16];
        for (x, t) in th.iter_mut().enumerate() {
            *t = (BAYER[y & 3][x & 3] * 4) as u8;
        }
        unsafe {
            let thv = core::arch::aarch64::vld1q_u8(th.as_ptr());
            let mut x = 0;
            while x < w16 {
                neon::sixteen_canvas(tabs, src.as_ptr().add(x), row.as_mut_ptr().add(x), thv);
                x += 16;
            }
        }
        dst[..w16].copy_from_slice(&row[..w16]);
    }
}

/// The ramps (op 21): straight from 16-bit working values to the panel, to see the present stage do its whole job. Columns, left to right, each a ramp from the top (black) to the bottom (the owner, 2026-10-22: vertical, for the longer run between steps), with 4-pixel white lines between them: grey to white dithered, and undithered (the fraction dropped); near-black (v to 1/8, light to 1/64 of white) dithered, undithered, and as an 8-bit canvas would hand it over (v in 1/255 steps); the headroom (v to 4.0, light to 16× white) through the roll-off; then red, green and blue to white. Written into every phase frame, each with its own thresholds (or the one framebuffer, spatial only).
fn ramps(outs: &mut [&mut [u32]], width: usize, height: usize, lut: &[u16], only: Option<usize>) {
    const COLS: usize = 9;
    const LINE: usize = 4;
    let col = (width - LINE * (COLS + 1)) / COLS;
    // One column alone (the owner, 2026-10-22: the dim ones against black, without their bright neighbours): a strip three eighths of the width down the middle, a thin white line each side, black elsewhere.
    let strip = width * 3 / 8;
    let (s0, s1) = ((width - strip) / 2, (width + strip) / 2);
    let n = outs.len();
    let mut rows = [[0u32; 1344]; PHASES];
    let w = width.min(1344);
    for y in 0..height {
        // The ramp's position, 0..=65535 down the height.
        let f = (y as u32 * 65535 / (height as u32 - 1)) as u32;
        for x in 0..w {
            let (k, inside, black) = match only {
                Some(k) => (k, x >= s0 && x < s1, x + LINE < s0 || x >= s1 + LINE),
                None => {
                    let k = x.saturating_sub(LINE) / (col + LINE);
                    (k, x >= LINE && (x - LINE) % (col + LINE) < col && k < COLS, false)
                }
            };
            let (v, dither, chan) = if black {
                (0u32, false, 3usize)
            } else if !inside {
                (16384u32, true, 3usize)
            } else {
                let (top, dither, eight) = match k {
                    0 | 1 => (16384u32, k == 0, false),
                    2 | 3 | 4 => (2048, k != 3, k == 4),
                    5 => (65535, true, false),
                    _ => (16384, true, false),
                };
                let mut v = f * top / 65535;
                if eight {
                    v = (v * 255 + 8192) / 16384 * 16384 / 255;
                }
                (v, dither, match k { 6 => 0, 7 => 1, 8 => 2, _ => 3 })
            };
            let base = lut[v.min(65535) as usize] as u32;
            let q4 = [base * E_CODE_Q15[0] >> 15, base * E_CODE_Q15[1] >> 15, base * E_CODE_Q15[2] >> 15];
            for (p, row) in rows.iter_mut().enumerate().take(n) {
                let th = if !dither { 0 } else if n == PHASES { threshold(x, y, p) } else { BAYER[y & 3][x & 3] as u32 * 4 };
                let c = |ch: usize| dither8(q4[ch], th);
                row[x] = match chan {
                    0 => pack10(c(0), 0, 0),
                    1 => pack10(0, c(1), 0),
                    2 => pack10(0, 0, c(2)),
                    _ => pack10(c(0), c(1), c(2)),
                };
            }
        }
        for (p, out) in outs.iter_mut().enumerate() {
            out[y * width..y * width + w].copy_from_slice(&rows[p][..w]);
        }
    }
}

/// The finished frame out to the framebuffer. There is no black pass: the layer is BGRX, so the panel never sees the alpha byte, an empty pixel (0) already shows black, and black blended under an antialiased edge adds nothing to its colour — only to the alpha the panel ignores. The pass that did it cost 15-25 ms a frame and changed no visible pixel (2026-10-22).
fn publish(back: &[Px], screen: &mut [u32]) {
    let c8 = |v: u64| ((v & 0xFFFF) * 255 / 16384).min(255) as u32;
    for (d, &p) in screen.iter_mut().zip(back.iter()) {
        *d = 0xFF00_0000 | c8(p) << 16 | c8(p >> 16) << 8 | c8(p >> 32);
    }
}

fn isqrt(v: i64) -> i64 {
    if v <= 0 {
        return 0;
    }
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}



// ---------------------------------------------------------------- the receive thread and the request ring
//
// Two threads (KERNEL.md "Threads, interrupts and cores": receive and work are different threads). The receive thread blocks in `recv` and never touches hardware or the canvas: it answers `5` (the query) from statics, and puts every other message into this ring for the render thread, replying at once — so a client's call costs microseconds whatever the panel is doing. A frame (op 14, or a batch, 15) that arrives while the previous frame still waits in the ring replaces it: the client outran the renderer, and only the newest scene matters. The dev and hardware ops that answer with data (a DSI read, "moved") wait for the render thread to run them and reply with its result.
const RING: usize = 16;
const SLOT: usize = 4096;
static mut RING_MSG: [[u8; SLOT]; RING] = [[0; SLOT]; RING];
static mut RING_LEN: [u16; RING] = [0; RING];
static mut RING_REPLY: [[u8; 16]; RING] = [[0; 16]; RING];
static mut RING_RLEN: [u8; RING] = [0; RING];
/// Slots consumed (the render thread advances it) and filled (the receive thread advances it); the difference is the queue.
static RING_HEAD: AtomicU64 = AtomicU64::new(0);
static RING_TAIL: AtomicU64 = AtomicU64::new(0);
/// Per slot, how many requests the render thread has completed in it: a waiting receive thread sees it move.
static RING_DONE: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static FRAMES: AtomicU64 = AtomicU64::new(0);
static mut SCREEN_WH: (u16, u16) = (0, 0);
static mut RECV_STACK: [u8; 32768] = [0; 32768];

/// Ops whose reply carries the render thread's result (a DSI read, whether a frame moved): the receive thread waits for them.
fn op_is_sync(op: u8) -> bool {
    matches!(op, 4 | 6 | 7 | 8 | 9 | 10 | 11 | 21 | 26 | 28 | 29 | 30 | 31)
}

extern "C" fn receive(grants: u64) -> ! {
    let ep = cap(grants, S_DISPLAY);
    let mut msg = [0u8; SLOT];
    let mut tok = [0u8; 32];
    loop {
        let n = ferros_sys::recv(ep, &mut msg, &mut tok, false);
        if n == ferros_sys::ERR || n == 0 || n > SLOT as u64 {
            continue;
        }
        let n = n as usize;
        let op = msg[0];
        if op == 5 {
            let (w, h) = unsafe { SCREEN_WH };
            let mut out = [0u8; 13];
            out[0..2].copy_from_slice(&w.to_le_bytes());
            out[2..4].copy_from_slice(&h.to_le_bytes());
            out[4..8].copy_from_slice(&(FRAMES.load(Ordering::Relaxed) as u32).to_le_bytes());
            out[8..12].copy_from_slice(&((now() / freq()) as u32).to_le_bytes());
            out[12] = unsafe { APP }.is_some() as u8;
            ferros_sys::reply(&tok, &out);
            continue;
        }
        if op == 27 && n >= 13 {
            // Whose is this point (WINDOWS.md): [base u64][x u16][y u16] → 1 if the caller's window is the frontmost that covers it (inside the circle, not see-through there), else 0. Answered here: a client asks it on every finger down.
            let (w, h) = unsafe { SCREEN_WH };
            let mine = surface_of(u64::from_le_bytes(msg[1..9].try_into().unwrap()));
            let (x, y) = (u16::from_le_bytes([msg[9], msg[10]]) as i64, u16::from_le_bytes([msg[11], msg[12]]) as i64);
            let yes = mine.is_some() && owner_at(x, y, w as usize, h as usize) == mine;
            ferros_sys::reply(&tok, &[yes as u8]);
            continue;
        }
        if op == 24 {
            // Answered here, never queued: the wait is the reply (op 24 above). Another client's call waits behind it at most a refresh.
            let seen = FRAME_STARTS.load(Ordering::Acquire);
            let deadline = now() + freq() / 120;
            while FRAME_STARTS.load(Ordering::Acquire) == seen && now() < deadline {
                ferros_sys::wait(&FRAME_STARTS, seen, deadline);
            }
            ferros_sys::reply(&tok, &(FRAME_STARTS.load(Ordering::Acquire) as u32).to_le_bytes());
            continue;
        }
        // Room: wait for the render thread to consume a slot.
        let mut head = RING_HEAD.load(Ordering::Acquire);
        let mut tail = RING_TAIL.load(Ordering::Relaxed);
        while tail - head >= RING as u64 {
            ferros_sys::wait(&RING_HEAD, head, 0);
            head = RING_HEAD.load(Ordering::Acquire);
        }
        let frame = matches!(op, 14 | 15);
        // A frame behind another unconsumed frame replaces it (the slot being overwritten is never the one the render thread is copying: at least two are queued, and it works on the oldest).
        let (slot, append) = if frame && tail - head >= 2 && matches!(unsafe { RING_MSG[((tail - 1) % RING as u64) as usize][0] }, 14 | 15) { (((tail - 1) % RING as u64) as usize, false) } else { ((tail % RING as u64) as usize, true) };
        unsafe {
            RING_MSG[slot][..n].copy_from_slice(&msg[..n]);
            RING_LEN[slot] = n as u16;
        }
        let done = RING_DONE[slot].load(Ordering::Acquire);
        if append {
            tail += 1;
            RING_TAIL.store(tail, Ordering::Release);
        }
        ferros_sys::wake(&RING_TAIL, 1);
        if op_is_sync(op) {
            while RING_DONE[slot].load(Ordering::Acquire) == done {
                ferros_sys::wait(&RING_DONE[slot], done, 0);
            }
            let len = unsafe { RING_RLEN[slot] } as usize;
            let reply = unsafe { RING_REPLY[slot] };
            ferros_sys::reply(&tok, &reply[..len.min(16)]);
        } else {
            // Replied before it is drawn: status 0, and "moved" for a frame as before.
            ferros_sys::reply(&tok, if frame { &[0, 1] } else { &[0] });
        }
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    // First: the image's own relocations (fontdue's `dyn OutlineBuilder` vtable is an absolute address in .rodata).
    unsafe { ferros_sys::relocate() };
    unsafe { PANIC_GRANTS = grants };
    let mut log = Log { grants, buf: [0; 256], n: 0 };
    let pipe = Pipe { grants };
    let fb = ferros_sys::dma(cap(grants, S_FB)) as usize;
    if fb == 0 || fb == u64::MAX as usize {
        log.s(b"display: no framebuffer region\n");
        log.flush();
        ferros_sys::exit(1);
    }
    let heap = ferros_sys::dma(cap(grants, S_HEAP)) as usize;
    if heap == 0 || heap == u64::MAX as usize {
        log.s(b"display: no heap region\n");
        log.flush();
        ferros_sys::exit(1);
    }
    ALLOC.init(heap);

    // The DPU powered down (SYS_FACTS 30 = 1: Android's DECON hibernation at a jump, or ferros's own `dpu off`): no DECON or DSIM register may be touched before the kernel's `dpu` switch has powered it (the EL3 poison rule, the kernel's display.rs); then the whole pipe from reset (modeset::from_off, DISPLAY-DESIGN.md §8).
    // On the X3 for the power switches and the modeset: the kernel takes SYS_POWER only from its own core, and a thread nobody pinned runs on the workers. Unpinned, this thread started on the X3 (where init spawned it), took the `dpu` and `panel` switches there, and was on a worker by the modeset's `panel-vddd`, refused without the switch ever running: the panel came up without VDDD after two jumps, ran a few frames and stopped (2026-11-04). Unpinned again before any thread is spawned: they inherit the mask.
    ferros_sys::pin(ferros_sys::CORES_BIG);
    ferros_sys::sleep_until(now() + freq() / 1000);
    let from_off = ferros_sys::facts().0[30] == 1;
    if from_off {
        if !ferros_sys::power(cap(grants, S_DPU_POWER), true) || ferros_sys::facts().0[30] != 2 {
            log.s(b"display: the DPU is powered down and its switch did not bring it up; its registers are not touched\n");
            log.flush();
            ferros_sys::exit(3);
        }
        log.s(b"display: the DPU was powered down: switched on, the pipe from reset\n");
        log.flush();
    }

    // ABL's 8 bpc pipe to the 10 bpc blend, once (modeset.rs; the kernel did this before init until 2026-11-01): before the raster is read and before any frame or panel thread.
    let t_ms = now();
    // After the jump from a running Android (SYS_FACTS 31 = 6) the pipe runs Android's raster and panel mode (already 10 bpc). The raster below is then DECON's own output (the blender's background size), not the layer DMA's, which holds whatever layer Android drew last (1008x2023 on the sixth jump, 2026-11-01). The modeset is not forced: forcing it under Android's panel state was followed by the APM watchdog on jumps seven to nine.
    let jumped = ferros_sys::facts().0[31] == 6;
    // Android switches the panel between 1008x2244 (its default "high resolution") and 1344x2992. A jump from 1008x2244 is drawn at that raster (touch then lands off: the controller reports in 1344x2992). Forcing the WQHD modeset there switched DECON and the DSC encoders but not the panel, which kept decoding in its scaled mode (the picture three-up, garbage below; the thirteenth jump, 2026-11-01): the stock driver changes the panel's resolution inside a full re-initialisation (reset, init set, PPS, C3/F2), not yet ported. Android set to "Full resolution" jumps clean.
    // After the jump the whole modeset runs again (force): with DECON left as Android configured it, ferros's frames never completed (the frame counter stood still, "settle timeout", the panel holding Android's last frame: the thirty-fifth jump). From Android's scaled mode as well: the modeset's PPS and resolution commands take the panel to its full raster (jump 37, 2026-11-02; the thirteenth jump's three-up had other causes, the X3 never launched among them).
    // The panel off (SYS_FACTS 81 = 1: its VCI off, a jump from a screen Android had timed out, or ferros's own `panel off`): its rails and reset pulse through the `panel` switch now, its init set inside the modeset after the link starts, display on after the first frame (DISPLAY-DESIGN.md §8 state B). The modeset is forced then: a running pipe needs the panel's PPS and init all the same.
    let panel_cold = ferros_sys::facts().0[81] == 1;
    if panel_cold {
        if !ferros_sys::power(cap(grants, S_PANEL_POWER), true) {
            log.s(b"display: the panel is off and its switch did not bring it up\n");
            log.flush();
            ferros_sys::exit(3);
        }
        PANEL_ON_DUE.store(1, Ordering::Release);
        log.s(b"display: the panel was off: rails on, reset, its init set next\n");
        log.flush();
    }
    // VDDD, asked again on a refusal: after three jumps it was refused at the kernel's door while `dpu` and `panel` just before it were taken, and a panel without VDDD stays black and stops its TE (2026-11-04). Each refusal is said; the kernel's flash log says which check refused it.
    let vddd = || {
        for _ in 0..4 {
            if ferros_sys::power(cap(grants, S_PANEL_VDDD), true) {
                return true;
            }
            console(grants, b"display: VDDD refused; asking again in 16 ms\n");
            ferros_sys::sleep_until(now() + freq() / 64);
        }
        false
    };
    let cold: Option<&dyn Fn() -> bool> = if panel_cold { Some(&vddd) } else { None };
    // On its rails with the DPU off, or after any jump: the modeset asks the panel whether it is showing, and wakes it through this switch if not (Android's screen off left it dozing with the DPU on as well as off).
    let reset = || ferros_sys::power(cap(grants, S_PANEL_POWER), true);
    let awake: Option<(&dyn Fn() -> bool, &dyn Fn() -> bool)> = if (from_off || jumped) && !panel_cold { Some((&reset, &vddd)) } else { None };
    log.at(b"modeset begins");
    let rep = if from_off { modeset::from_off(&pipe, &ferros_dsi::Dsim(DsiGrant { grants }), cold, awake) } else { modeset::ten_bpc(&pipe, &ferros_dsi::Dsim(DsiGrant { grants }), jumped || panel_cold, jumped, cold, awake) };
    log.at(b"modeset done");
    if !rep.is_empty() {
        log.s(b"display: ");
        log.s(&rep);
        log.s(b"display: start to 10 bpc took ");
        log.dec(ferros_sys::us_since(t_ms) / 1000);
        log.s(b" ms\n");
        log.flush();
    }
    ferros_sys::pin(0);

    // The raster as the pipe runs it.
    let img = if jumped { pipe.rd(S_DECON, 0x0220) } else { pipe.rd(S_DMA0, RDMA_IMG_SIZE) };
    let (width, height) = ((img & 0xFFFF) as usize, (img >> 16) as usize);
    // After the jump, window 5 (the base layer ferros owns on a cold boot) and no other: Android leaves its own layers enabled (window 0 on the thirteenth jump, its blend word G#00FF0D02 still in place), and the first enabled one carried ferros's pixels too bright and posterized. Every other window off, from here.
    let win = if jumped {
        for w in 0..8u64 {
            if w != 5 {
                pipe.wr(S_WINCON, w * 0x1000, 0);
            }
        }
        // Their shadow copies latch at the next frame only with their own update bits.
        pipe.wr(S_DECON, SHD_REG_UP_REQ, pipe.rd(S_DECON, SHD_REG_UP_REQ) | SHD_GLOBAL | 0xFF);
        5
    } else {
        pipe.base_window()
    };
    log.s(b"display: ");
    log.dec(width as u64);
    log.s(b"x");
    log.dec(height as u64);
    log.s(b" window ");
    log.dec(win as u64);
    log.s(b" fb ");
    log.hex(fb as u32);
    log.s(b"\n");
    log.flush();
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        ferros_sys::exit(2);
    }
    // The back buffer: every frame is composited here, in cacheable memory, and copied to the framebuffer only at present. fluor reads every pixel it blends onto, and a read from the non-cacheable framebuffer costs about 300 ns: drawing straight into it took 2.7 s for one dashboard frame (2026-10-22: a screen-sized disc 394 ms, the scene 2.3 s, while clearing all 16 MB took 1 ms — the writes are combined, the reads are not).
    // The back buffer: the 16-bit canvas in its own region (8 bytes a pixel); the heap keeps its whole region.
    let back = ferros_sys::dma(cap(grants, S_CANVAS));
    if back == 0 || back == ferros_sys::ERR || (width * height * 8) > (32 << 20) {
        log.s(b"display: no canvas region (display-canvas): cannot draw\n");
        log.flush();
        ferros_sys::exit(3);
    }
    let back = back as usize;

    // The core's clock, measured: 2^20 dependent adds take 2^20 cycles (1.8 ms at the 576 MHz measured 2026-10-22), so cycles per microsecond is the MHz this process runs at.
    let t_clk = now();
    let mut acc = 0u64;
    for _ in 0..(1u64 << 20) {
        unsafe { core::arch::asm!("add {0}, {0}, #1", inout(reg) acc) };
    }
    let us = ferros_sys::us_since(t_clk).max(1);
    log.s(b"display: core clock ~");
    log.dec((1u64 << 20) / us + (acc & 0));
    log.s(b" MHz\n");
    log.flush();
    let t_font = now();
    match glyphs::Glyphs::new(FONT) {
        Some(g) => unsafe { GLYPHS = Some(g) },
        None => {
            log.s(b"display: typeface failed to parse\n");
            log.flush();
            ferros_sys::exit(3);
        }
    }
    let font = Font;
    // The present stage's table and its 8-bit slice, built before the mark so they live for the process.
    let t_lut = now();
    let lut: &'static mut [u16] = alloc::vec![0u16; 65536].leak();
    // fluor's 16-bit g2t codes to working values (`G2T16_Q14`).
    {
        let g2t: &'static mut [u16] = alloc::vec![0u16; 1 << 16].leak();
        for (c, v) in g2t.iter_mut().enumerate() {
            // WHY/PROOF: the working value is 16 bits (4.0); the top 79 codes decode past it (atanh(65535/65536) × 2¹⁴ ≈ 90 850), and `as u16` alone would also stop them there — the min names the ceiling.
            *v = (16384. * libm::atanh(c as f64 / 65536.) + 0.5).min(65535.) as u16;
        }
        unsafe { G2T16_Q14 = g2t.as_ptr() };
    }
    let mut bright = ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS);
    let (white0, peak0, hbm0, dbv0) = brightness(bright);
    build_lut(lut, white0, peak0);
    // The ODT's tables for this white (neon.rs `Odt`): every pixel's path.
    let mut tabs = odt_tables(white0, peak0);
    let lut_ms = ferros_sys::us_since(t_lut) / 1000;
    // Each app icon's mip chain (draw::Mips), premultiplied, built once and kept.
    let mips: &'static [draw::Mips] = {
        let decode = icon_decode();
        let mut v = alloc::vec::Vec::new();
        let mut n = 1u8;
        while let Some((px, grey, size)) = icon(n) {
            v.push(draw::Mips::build(px, grey, size, &decode));
            n += 1;
        }
        v.leak()
    };
    unsafe {
        let sprites = ferros_sys::dma(cap(grants, S_SPRITES));
        if sprites != 0 && sprites != ferros_sys::ERR {
            ARENA = sprites as usize;
            ARENA_WORDS = (64 << 20) / 8;
        } else {
            ARENA = alloc::vec![0u64; ARENA_WORDS].leak().as_mut_ptr() as usize;
        }
        ARENA_NEXT = 0;
        // The corners' chrome, kept across calls (before the mark).
        let (top, bot) = chrome_rows(width, height);
        CHROME_TOP = top;
        CHROME_BOT = bot;
        plan_glass(width, height);
        CHROME = alloc::vec![0u64; (top + bot) * width].leak().as_mut_ptr() as usize;
    }
    ALLOC.mark();
    log.s(b"display: present table (E peak ");
    log.dec(peak0 as u64);
    log.s(b" cd/m2, white ");
    log.dec(white0 as u64);
    log.s(b", white at 10-bit code ");
    log.dec((lut[16384] >> 4) as u64);
    log.s(b") in ");
    log.dec(lut_ms);
    log.s(b" ms\n");
    log.flush();
    log.s(b"display: typeface up in ");
    log.dec(ferros_sys::us_since(t_font) / 1000);
    log.s(b" ms, heap ");
    log.dec(ALLOC.used() as u64 >> 10);
    log.s(b" KiB\n");
    log.flush();

    let pixels: &mut [Px] = unsafe { core::slice::from_raw_parts_mut(back as *mut Px, width * height) };
    let screen: &mut [u32] = unsafe { core::slice::from_raw_parts_mut(fb as *mut u32, width * height) };

    // The layer is left on the boot ring's surface until a client's first frame: every present points it here first, so the ring stays up until the attest screen replaces it, with no black between (owner, 2026-11-05). The back buffer starts black.
    let mut damage = Damage::new();
    pixels.fill(0);

    // The panel's command path, and the link rate its TE timers are figured from (the DPHY PLL at +G#100 of the dphy0 grant).
    let dsi = ferros_dsi::Dsim(DsiGrant { grants });
    let hs_mbps = ferros_dsi::pll_hs_clk(|off| mmio_read(cap(grants, S_DPHY), 0x100 + off as u64) as u32);
    log.s(b"display: dsim ");
    log.s(if dsi.alive() { b"alive, link " } else { b"NOT alive, link " });
    log.dec(hs_mbps as u64);
    log.s(b" Mbps\n");
    log.flush();
    // Nothing moves between presents, so the panel may rest: the 10 Hz auto-rate floor (its own engine drops the refresh when frames stop and comes back for the next one; DISPLAY-POWER.md: 150–190 mW under 120 Hz) — with the TE kept fixed at 120 Hz, as Android runs it: DECON triggers on TE, so a changeable TE at the floor would make every present wait for the next 100 ms tick (measured 10 Hz on the pin, 2026-10-16 night). Fixed, the panel inserts the frame at once and the saving stays. A client can change the floor (op 8).
    if dsi.alive() {
        log.at(b"panel idle + TE commands");
        let (_, f1) = dsi.sequence(ferros_dsi::hk3::IDLE_10);
        let (_, f2) = dsi.sequence(ferros_dsi::hk3::TE_FIXED);
        log.s(if f1.is_none() && f2.is_none() { b"display: panel idle floor 10 Hz, TE fixed 120\n" } else { b"display: idle floor / TE command stuck\n" });
        log.flush();
    }

    // 10 bits when the kernel has put DECON in the 10 bpc blend (GLOBAL_CON bit 20, set by the modeset before the shell starts): the layer becomes 10-bit and every frame goes through the present stage, with the panel at its peak (HBM) so the table's headroom is real. Otherwise the 8-bit copy, as before.
    let ten = pipe.rd(S_DECON, 0x0020) & (1 << 20) != 0;
    if ten && dsi.alive() {
        log.at(b"panel brightness");
        let r = set_panel(&dsi, hbm0, dbv0);
        log.at(b"panel brightness done");
        log.s(if r { b"display: 10 bits, panel brightness set for the white and its headroom\n" } else { b"display: 10 bits, but the brightness command stuck\n" });
    } else {
        log.s(b"display: 8 bits (DECON not in the 10 bpc blend)\n");
    }
    log.flush();
    unsafe { TEN = ten };
    // The temporal dither's four frames, when the region is granted and the pipe is in 10 bits.
    let phases = ferros_sys::dma(cap(grants, S_PHASES)) as usize;
    if ten && phases != 0 && phases != u64::MAX as usize && width * height * 4 <= 16 << 20 {
        unsafe {
            for k in 0..PHASES {
                PHASE_BASE[k] = phases + k * (16 << 20);
                phase_slice(k, width * height).fill(3);
            }
            TEMPORAL = true;
        }
        log.s(b"display: temporal dither on: four frames, flipped every 100 ms\n");
    } else {
        log.s(b"display: no temporal dither (spatial only)\n");
    }
    log.flush();
    let surf = ferros_sys::dma(cap(grants, S_APPSURF)) as usize;
    if surf != 0 && surf != u64::MAX as usize {
        // The four surfaces' start: from init's spawn the grant arrives as one surface, surface 0, answered past its porch; from a restart (the kernel's shell grants) as the whole region at its base. Rounded down to the surface size, both are the region's start (the native display reaches all four from there; masked, it will need the whole region as one grant either way).
        unsafe { SURFS = surf & !(SURFACE_BYTES - 1) };
    }
    let mut next_flip = now() + freq() / 10;
    // In focus, when the clock is next drawn (10 Hz).
    let mut clock_slow_next = 0u64;
    let mut screen_off = false;
    // A frame armed and not yet seen to start (Pipe::arm / settle).
    // The modeset re-sent the panel its PPS, raster and rate, and the panel drops what it was showing: black from here to the first client frame (464 to 1033 ms after the press, measured 2026-11-05). The layer is still on the boot ring's surface, so one frame now puts the ring back until the attest screen replaces it. Armed, not waited for (the frame-start count is not running yet; waiting cost a quarter second): the main loop's settle masks the trigger once the frame has started. A cold or jumped panel has shown nothing yet: the same frame is its first.
    let mut armed: Option<u32> = Some(pipe.arm(win));

    let mut frames = 0u64;
    // A client owns the surface from its first `begin` on; until then this process paints its own screen and clock.
    let mut client_owned = false;
    // When the map last moved its camera: while someone is touching, poll the endpoint fast enough to follow a finger; at rest, slowly, so the core sleeps.
    let mut last_interaction = 0u64;
    let mut next_tick = (now() / freq() + 1) * freq();
    let mut msg = [0u8; 4096];
    let mut last_slow_log = 0u64;
    let mut last_reply = 0u64;
    // A second console writer: `log` is lent to the op handler for the whole pass.
    let mut tlog = Log { grants, buf: [0; 256], n: 0 };
    unsafe { SCREEN_WH = (width as u16, height as u16) };
    if ferros_sys::thread_spawn(receive, unsafe { &mut *(&raw mut RECV_STACK) }, grants) != 0 {
        log.s(b"display: no receive thread\n");
        log.flush();
    }
    // Frame-start interrupts, and only those, on at DECON (the bootloader leaves every source enabled, INT_EN G#303011, with frame-done pending and never cleared); the pending bits cleared first; then the thread that takes them.
    pipe.wr(S_DECON, INT_EN, 0);
    pipe.wr(S_DECON, INT_PEND, !0);
    pipe.wr(S_DECON, INT_PEND_EXTRA, !0);
    pipe.wr(S_DECON, INT_EN, 1 | INT_EN_FRAME_START);
    if ferros_sys::thread_spawn(vsync, unsafe { &mut *(&raw mut VSYNC_STACK) }, grants) != 0 {
        log.s(b"display: no vsync thread\n");
        log.flush();
    }
    if ferros_sys::thread_spawn(panel, unsafe { &mut *(&raw mut PANEL_STACK) }, grants) != 0 {
        log.s(b"display: no panel thread\n");
        log.flush();
    }
    for i in 0..PUB_WORKERS {
        if ferros_sys::thread_spawn(pub_worker, unsafe { &mut (*(&raw mut PUB_STACKS))[i] }, i as u64) != 0 {
            log.s(b"display: no publish worker\n");
            log.flush();
        }
    }
    // The render thread on the big core (KERNEL.md "Cores": pinning by latency class — this is the one real compute path; receive and vsync stay on the workers, where the map and the touch server run, so a call hands off on the caller's own core). After the other threads are spawned: they inherit the mask.
    ferros_sys::pin(ferros_sys::CORES_BIG);
    // What the ring's tail was when this pass last looked: the wait at the end of a pass returns as soon as it moves.
    let mut seen_tail = 0u64;
    let (mut frames_last_tick, mut starts_last_tick) = (0u64, 0u64);
    let mut clock_due = false;
    // The clock face's 60 directions, once.
    unsafe {
        for k in 0..60 {
            let a = k as f32 * core::f32::consts::TAU / 60.0;
            CLOCK_DIRS[k] = ((libm::sinf(a) * 16384.0) as i64, (-libm::cosf(a) * 16384.0) as i64);
        }
    }
    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
        // One op, as the endpoint's header lists them: the reply's first byte is the status, the return the reply's length.
        let mut handle = |m: &[u8], out: &mut [u8; 16]| -> usize {
            let mut len = 1usize;
            out[0] = 0;
            unsafe { CUR_OP = m.first().copied().unwrap_or(0) };
            match m.first() {
                Some(1) => {
                    client_owned = true;
                    unsafe { CLIENT_DREW = true };
                    // Only what the last frame drew needs clearing; the rest of the back buffer is still empty. Between frames (the clock's redraw), only the clip.
                    unsafe {
                        if { CLIP_PHASE }.is_some() {
                            draw::clear(pixels, width, draw::CLIP.clip(width, height));
                        } else {
                            draw::clear(pixels, width, PREV_DMG.clip(width, height));
                            CUR_DMG = draw::Rect::EMPTY;
                        }
                    }
                }
                Some(2) if m.len() >= 12 => {
                    let x = u16::from_le_bytes([m[1], m[2]]) as f32;
                    let y = u16::from_le_bytes([m[3], m[4]]) as f32;
                    let px = u16::from_le_bytes([m[5], m[6]]) as f32;
                    let colour = 0xFF00_0000 | u32::from_le_bytes([m[7], m[8], m[9], m[10]]) & 0x00FF_FFFF;
                    let align = m[11];
                    match core::str::from_utf8(&m[12..]) {
                        Ok(t) if px >= 4.0 && px <= 512.0 => {
                            let tw = text_width(&font, t, px);
                            let left = match align { 1 => x - tw * 0.5, 2 => x - tw, _ => x };
                            let mut c = Canvas::new(pixels, width, height, &mut damage);
                            text(&mut c, &font, t, left, y, px, colour);
                            ALLOC.reset();
                            let r = draw::Rect { x0: left as i32 - 2, y0: (y - px) as i32 - 2, x1: (left + tw) as i32 + 3, y1: (y + px * 0.4) as i32 + 3 };
                            unsafe { CUR_DMG = CUR_DMG.union(r.clip(width, height)) };
                        }
                        _ => out[0] = 1,
                    }
                }
                Some(3) if m.len() >= 13 => {
                    let v = |i: usize| u16::from_le_bytes([m[i], m[i + 1]]) as isize;
                    let colour = 0xFF00_0000 | u32::from_le_bytes([m[9], m[10], m[11], m[12]]) & 0x00FF_FFFF;
                    let mut c = Canvas::new(pixels, width, height, &mut damage);
                    paint::fill_rect(&mut c, v(1), v(3), v(5), v(7), colour, None, None);
                    let r = draw::Rect { x0: v(1) as i32, y0: v(3) as i32, x1: (v(1) + v(5).max(1)) as i32, y1: (v(3) + v(7).max(1)) as i32 };
                    unsafe { CUR_DMG = CUR_DMG.union(r.clip(width, height)) };
                }
                Some(4) => {
                    // The finished frame to the framebuffer: reads from cache, writes combined.
                    if ten && unsafe { TEMPORAL } { publish_next(pixels, width, height, &tabs, draw::Rect::full(width, height)) } else if ten { publish10(pixels, screen, width, &tabs) } else { publish(pixels, screen) }
                    ferros_sys::dma_barrier();
                    pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                    let (moved, _) = pipe.present(win);
                    frames += 1;
                        FRAMES.store(frames, Ordering::Relaxed);
                    out[1] = moved as u8;
                    len = 2;
                }
                Some(12) => {
                    // The scene changed: the last frame no longer matches it, so the clock is not redrawn from it until the client sends the frame that does (a pinch's renormalise rescaled the scene under the old camera, and the replay drew a clock and scenery at the wrong scale into the clock's square, 2026-10-24).
                    unsafe { LAST_FRAME.1 = 0 };
                    client_owned = true;
                    unsafe { CLIENT_DREW = true };
                    unsafe { SCENE_N = 0 };
                }
                Some(13) if m.len() >= 2 => {
                    // The scene changed: the last frame no longer matches it, so the clock is not redrawn from it until the client sends the frame that does (a pinch's renormalise rescaled the scene under the old camera, and the replay drew a clock and scenery at the wrong scale into the clock's square, 2026-10-24).
                    unsafe { LAST_FRAME.1 = 0 };
                    client_owned = true;
                    unsafe { CLIENT_DREW = true };
                    let count = m[1] as usize;
                    let mut at = 2usize;
                    for _ in 0..count {
                        if at + 30 > m.len() || unsafe { SCENE_N } >= 256 {
                            out[0] = 2;
                            break;
                        }
                        let i32le = |i: usize| i32::from_le_bytes([m[i], m[i + 1], m[i + 2], m[i + 3]]);
                        let mut label = [0u8; 16];
                        label.copy_from_slice(&m[at + 14..at + 30]);
                        let c = Circle { x: i32le(at), y: i32le(at + 4), r: u16::from_le_bytes([m[at + 8], m[at + 9]]), ring: m[at + 10], lit: m[at + 11], icon: m[at + 12], len: m[at + 13].min(16), label, c: 16384, s: 0 };
                        unsafe {
                            SCENE[SCENE_N] = c;
                            SCENE_N += 1;
                        }
                        at += 30;
                    }
                }
                Some(14) if m.len() >= 13 => {
                    client_owned = true;
                    unsafe { CLIENT_DREW = true };
                    let clip_k = unsafe { CLIP_PHASE };
                    if clip_k.is_none() {
                        last_interaction = now();
                        // Gravity for this frame: under a finger the map's frames come back to back and the loop's top (where gravity is read at rest) is not reached between them, so the clock kept the turn it had when the touch began (owner, 2026-10-24).
                        gravity_poll();
                    }
                    let saved_dmg = unsafe { (PREV_DMG, CUR_DMG) };
                    // The camera, in integers: the centre in world units ×256, the zoom 16.16, the turn (cos, sin) Q14.
                    let cx = i32::from_le_bytes([m[1], m[2], m[3], m[4]]) as i64;
                    let cy = i32::from_le_bytes([m[5], m[6], m[7], m[8]]) as i64;
                    let z = u32::from_le_bytes([m[9], m[10], m[11], m[12]]) as i64;
                    let (rc, rs) = if m.len() >= 18 { (i16::from_le_bytes([m[14], m[15]]) as i64, i16::from_le_bytes([m[16], m[17]]) as i64) } else { (16384, 0) };
                    // Flag bit 0: keep what the frame already holds (a `begin` and the client's overlay, sent first in the same batch), so the scene goes under it; without it the frame starts from the last frame's rectangle cleared.
                    unsafe {
                        // A full frame says afresh which widgets are on the glass; a redraw between frames (a clip) draws only what reaches its clip and must not forget the others (the clock's clip turned the scope's redraws off, and the scope's the clock's, 2026-10-25).
                        if clip_k.is_none() {
                            CLOCK_SHOWN = false;
                            CLOCK_RECT = draw::Rect::EMPTY;
                            SCOPE_SHOWN = false;
                            SCOPE_RECT = draw::Rect::EMPTY;
                        }
                        LAST_CAM = if m.get(13).copied().unwrap_or(0) & 1 == 0 && m.len() <= 18 {
                            let mut b = [0u8; 18];
                            b[..m.len()].copy_from_slice(m);
                            Some((b, m.len()))
                        } else {
                            None
                        };
                    }
                    if m.get(13).copied().unwrap_or(0) & 1 == 0 {
                        let t_c = now();
                        unsafe {
                            draw::clear(pixels, width, if clip_k.is_some() { draw::CLIP.clip(width, height) } else { PREV_DMG.clip(width, height) });
                            CUR_DMG = draw::Rect::EMPTY;
                            CLEAR_US = ferros_sys::us_since(t_c);
                        }
                    }
                    let mut t_phase = now();
                    unsafe { SCENE_US = [0; 3]; PUB_TICKS = [0; 3] };
                    let (wq, hq) = ((width as i64) << 7, (height as i64) << 7);
                    // The glows, collected as the circles go and drawn after all of them, so they land behind every circle (front to back: what is drawn later goes under). Each: centre, radius (Q8), colour, peak.
                    let mut glows = [(0i64, 0i64, 0i64, 0u32, 0u32); 256];
                    let mut nglow = 0usize;
                    // The theme (ferros_sys::theme, device preference 5): every colour the scene chooses.
                    let th = ferros_sys::theme::current();
                    for i in 0..unsafe { SCENE_N } {
                        let cir = unsafe { SCENE[i] };
                        // World to screen, Q8 pixels: offset, turn, zoom, centre.
                        let (dx, dy) = (((cir.x as i64) << 8) - cx, ((cir.y as i64) << 8) - cy);
                        // Centres on whole pixels, so a cached sprite and a fresh drawing always line up.
                        let sx = (((rc * dx - rs * dy) >> 14) * z / 65536 + wq) & !255 | 128;
                        let sy = (((rs * dx + rc * dy) >> 14) * z / 65536 + hq) & !255 | 128;
                        let sr = cir.r as i64 * z >> 8;
                        // Between frames, only what reaches the clip (the circle and its glow, 1/12 of the radius out) is drawn.
                        if clip_k.is_some() {
                            let reach = sr + sr / GLOW_FRACTION + 512;
                            let c = unsafe { draw::CLIP };
                            if (sx + reach) >> 8 < c.x0 as i64 || (sx - reach) >> 8 >= c.x1 as i64 || (sy + reach) >> 8 < c.y0 as i64 || (sy - reach) >> 8 >= c.y1 as i64 {
                                continue;
                            }
                        }
                        // Its glow, before the visibility cut: a circle just off the screen still lights its edge.
                        if sr >= 256 && cir.ring > 0 && nglow < glows.len() {
                            let colour = if cir.lit != 0 { th.lit & 0x00FF_FFFF } else { th.ring & 0x00FF_FFFF };
                            glows[nglow] = (sx, sy, sr, colour, th.glow_peak[(cir.ring as usize).min(2)]);
                            nglow += 1;
                        }
                        if sr < 256 || sx + sr < 0 || sx - sr > (width as i64) << 8 || sy + sr < 0 || sy - sr > (height as i64) << 8 {
                            continue;
                        }
                        // Importance is the ring's weight, in screen pixels so it stays legible at every zoom: thin 4 % (at least a pixel), heavy 12 % (at least two).
                        // No stroke: the circle's image reaches its radius, so circles touch; importance is the glow, drawn after every circle (below).
                        // No stroke any more; the slot in the sprite's key holds the theme, which colours a plain circle's fill.
                        let stroke = ferros_sys::theme::index() as i64;
                        let lit = cir.lit != 0;
                        let ri = sr;
                        // A widget the display draws itself (not a bundle icon): the clock — its time in the centre first, then the face under it; never cached, it changes every second.
                        if cir.icon == WIDGET_SCOPE {
                            // Turned as an icon is: the camera's turn, then the circle's own.
                            let (cc, cs) = (cir.c as i64, cir.s as i64);
                            let turn = (((rc * cc - rs * cs) >> 14) as f32 / 16384.0, ((rs * cc + rc * cs) >> 14) as f32 / 16384.0);
                            let r = scope_widget(pixels, width, height, sx, sy, sr, turn, th, &font);
                            ALLOC.reset();
                            unsafe {
                                CUR_DMG = CUR_DMG.union(r);
                                SCOPE_SHOWN = true;
                                SCOPE_RECT = SCOPE_RECT.union(r);
                            }
                            continue;
                        }
                        if cir.icon == WIDGET_CLOCK {
                            let mut tb = [0u8; 32];
                            let (rings, nrings, tn, key, pm) = clock_state(&mut tb);
                            unsafe { CLOCK_KEY = key };
                            if let Ok(t) = core::str::from_utf8(&tb[..tn]) {
                                // The centre's width (124/256 of the radius, both sides), less a margin, at the text's natural width.
                                let room = (sr >> 8) as f32 * 2.0 * 124.0 / 256.0 * 0.82;
                                // Sized by the widest the time can be (12:00:00, four dozenal places, or a row of three in focus), so the digits keep one size as the time's own width changes.
                                let widest = match nrings {
                                    6 => "\u{10}:\u{10}:\u{10}",
                                    4 => "\u{10}:\u{10}:\u{10}:\u{10}",
                                    _ => "00:00:00",
                                };
                                let unit = text_width(&font, widest, 100.0).max(1.0);
                                // As big as the face makes room for, at every zoom (owner, 2026-10-24: no cap — at 256 px the digits stopped growing with the clock).
                                let px = room / unit * 100.0;
                                // The digits turn with the face: upright by gravity once the AoC reports it, else with the map's and the circle's own turn.
                                let (cc, cs) = (cir.c as i64, cir.s as i64);
                                let (fc, fs) = unsafe { GRAV_TURN }.unwrap_or(((rc * cc - rs * cs) >> 14, (rs * cc + rc * cs) >> 14));
                                let (tcf, tsf) = (fc as f32 / 16384.0, fs as f32 / 16384.0);
                                let (cxf, cyf) = ((sx >> 8) as f32 + 0.5, (sy >> 8) as f32 + 0.5);
                                let t_digits = now();
                                // Two rows in focus (the upper three places over the lower three), one otherwise; rows stack along the face's own down.
                                let rows = t.split('\n').count() as f32;
                                for (k, line) in t.split('\n').enumerate() {
                                    let d = (k as f32 - (rows - 1.0) * 0.5) * px * 1.15;
                                    text_turned_centre(pixels, width, height, line, cxf - tsf * d, cyf + tcf * d, px, (tcf, tsf), 0xFF00_0000 | th.text);
                                    ALLOC.reset();
                                }
                                // am / pm under the digits, half their size, turned with the face.
                                if let Some(pm) = pm {
                                    let d = px * 0.8;
                                    text_turned_centre(pixels, width, height, if pm { "pm" } else { "am" }, cxf - tsf * d, cyf + tcf * d, px * 0.45, (tcf, tsf), 0xFF00_0000 | th.text2);
                                    ALLOC.reset();
                                }
                                unsafe { CLOCK_PART_US[0] += ferros_sys::us_since(t_digits) };
                            }
                            let t_face = now();
                            // The face's turn: gravity's when known, else the camera's and the circle's own, as the icons get it.
                            let (cc, cs) = (cir.c as i64, cir.s as i64);
                            let (fc, fs) = unsafe { GRAV_TURN }.unwrap_or(((rc * cc - rs * cs) >> 14, (rs * cc + rc * cs) >> 14));
                            let r = draw::clock(pixels, width, height, sx, sy, sr, &rings[..nrings], unsafe { &*(&raw const CLOCK_DIRS) }, fc, fs);
                            unsafe { CLOCK_PART_US[1] += ferros_sys::us_since(t_face) };
                            unsafe {
                                CLOCK_GEOM = (sx, sy, sr, fc, fs, nrings);
                                BACK_CLOCK = ClockShown { frac: core::array::from_fn(|k| rings[k].frac_q16), lighter: core::array::from_fn(|k| rings[k].lighter), key, turn: GRAV_TURN.unwrap_or((0, 0)) };
                            }
                            unsafe {
                                CUR_DMG = CUR_DMG.union(r);
                                CLOCK_SHOWN = true;
                                CLOCK_RECT = CLOCK_RECT.union(r);
                            }
                            continue;
                        }
                        // Front to back: the label, the icon, the fill, the ring's larger disc beneath them all.
                        // Zoom is semantic: a name appears only once the circle is big enough to read it.
                        if sr >= 36 << 8 && cir.len > 0 {
                            if let Ok(t) = core::str::from_utf8(&cir.label[..cir.len as usize]) {
                                // A third of the radius, at every zoom (no cap: it stopped at 72 px; the 36-px threshold above is when a name appears at all).
                                let px = (sr >> 8) as f32 * 0.32;
                                let tw = text_width(&font, t, px);
                                let (fx, fy) = ((sx >> 8) as f32, (sy >> 8) as f32);
                                let mut c = Canvas::new(pixels, width, height, &mut damage);
                                text(&mut c, &font, t, fx - tw * 0.5, fy + px * 0.35, px, if lit { BLACK } else { 0xFF00_0000 | th.text });
                                ALLOC.reset();
                            }
                        }
                        let t_i = now();
                        let (cc, cs) = (cir.c as i64, cir.s as i64);
                        let (tc, ts) = ((rc * cc - rs * cs) >> 14, (rs * cc + rc * cs) >> 14);
                        let fill = if lit { th.lit & 0x00FF_FFFF } else { th.plane & 0x00FF_FFFF };
                        let mp = mips.get((cir.icon as usize).wrapping_sub(1));
                        let d = (2 * ((sr >> 8) + 3)) as usize;
                        let mut r = draw::Rect::EMPTY;
                        let still = unsafe { LAST_KEY[i] == [sr, stroke, tc, ts, lit as i64, cir.icon as i64] };
                        unsafe { LAST_KEY[i] = [sr, stroke, tc, ts, lit as i64, cir.icon as i64] };
                        if d <= SPRITE_MAX && still {
                            // Cached: drawn at the sprite's centre pixel, then blitted.
                            let key = [sr, stroke, tc, ts, lit as i64, cir.icon as i64];
                            let (spr, ext) = sprite(i, key, d, |px, d| {
                                let c = ((d as i64 / 2) << 8) | 128;
                                // The fill only where there is no art: under an icon it showed as a dark rim along the edge.
                                match mp {
                                    Some(mp) => {
                                        draw::icon(px, d, d, c, c, ri, tc, ts, mp, lit);
                                    }
                                    None => {
                                        draw::disc(px, d, d, c, c, ri, fill);
                                    }
                                }

                            });
                            unsafe { SCENE_US[0] += ferros_sys::us_since(t_i) };
                            let t_b = now();
                            blit(pixels, width, height, spr, ext, d, (sx >> 8) - d as i64 / 2, (sy >> 8) - d as i64 / 2);
                            unsafe { SCENE_US[1] += ferros_sys::us_since(t_b) };
                        } else {
                            if let Some(mp) = mp {
                                draw::icon(pixels, width, height, sx, sy, ri, tc, ts, mp, lit);
                            }
                            let t_d = now();
                            unsafe { SCENE_US[0] += ferros_sys::us_since(t_i) };
                            if mp.is_none() {
                                draw::disc(pixels, width, height, sx, sy, ri, fill);
                            }
                            r = draw::Rect::EMPTY;
                            unsafe { SCENE_US[1] += ferros_sys::us_since(t_d) };
                        }
                        let whole = draw::Rect { x0: ((sx - sr) >> 8) as i32 - 3, y0: ((sy - sr) >> 8) as i32 - 3, x1: ((sx + sr) >> 8) as i32 + 4, y1: ((sy + sr) >> 8) as i32 + 4 }.clip(width, height);
                        unsafe { CUR_DMG = CUR_DMG.union(whole).union(r) };
                    }
                    let t_g = now();
                    for &(gx, gy, gr, colour, peak) in &glows[..nglow] {
                        let gr_rect = if GLOW_LOG {
                            draw::glow_log(pixels, width, height, gx, gy, gr, colour, peak, GLOW_FRACTION, GLOW_OCT_LOG2)
                        } else {
                            draw::glow(pixels, width, height, gx, gy, gr, colour, peak, GLOW_SCALE_Q8, GLOW_FLOOR)
                        };
                        unsafe { CUR_DMG = CUR_DMG.union(gr_rect) };
                    }
                    unsafe { SCENE_US[2] = ferros_sys::us_since(t_g) };
                    unsafe { PHASE_US[0] = ferros_sys::us_since(t_phase) };
                    t_phase = now();
                    unsafe { PHASE_US[1] = 0 };
                    // What changed on screen: what the last frame drew and what this one drew.
                    // A clock refresh (the camera has not moved): only the clock's own rectangle changed.
                    if let Some(k) = clip_k {
                        unsafe {
                            publish10x4(pixels, width, height, &tabs, if CLIP_MASK != 0 { CLIP_MASK } else { 1 << k }, draw::CLIP.clip(width, height));
                            (PREV_DMG, CUR_DMG) = saved_dmg;
                        }
                        ferros_sys::dma_barrier();
                        return len;
                    }
                    let changed = unsafe { if CLOCK_ONLY { CLOCK_RECT.clip(width, height) } else { PREV_DMG.union(CUR_DMG).clip(width, height) } };
                    unsafe { PREV_DMG = CUR_DMG };
                    // The finished frame out. Temporal: into the next phase, armed without waiting for it to go out (only for the previous frame to have started); the layer flips to it at the next TE.
                    if ten && unsafe { TEMPORAL } {
                        pipe.settle(&mut armed, true);
                        unsafe { PHASE_US[3] = ferros_sys::us_since(t_phase) };
                        t_phase = now();
                        publish_next(pixels, width, height, &tabs, changed);
                        ferros_sys::dma_barrier();
                        unsafe { PHASE_US[2] = ferros_sys::us_since(t_phase) };
                        pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                        armed = Some(pipe.arm(win));
                        frames += 1;
                        FRAMES.store(frames, Ordering::Relaxed);
                        unsafe {
                            for k in 0..4 {
                                SEC_US[k] += PHASE_US[k];
                            }
                            SEC_US[1] += SCENE_US[2];
                            SEC_US[4] += 1;
                            SEC_US[5] += SCENE_US[0];
                            SEC_US[6] += SCENE_US[1];
                            SEC_US[7] += CLEAR_US;
                        }
                        out[1] = 1;
                        len = 2;
                    } else {
                        if ten { publish10(pixels, screen, width, &tabs) } else { publish(pixels, screen) }
                        ferros_sys::dma_barrier();
                        unsafe { PHASE_US[2] = ferros_sys::us_since(t_phase) };
                        t_phase = now();
                        pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                        let (moved, _) = pipe.present(win);
                        unsafe { PHASE_US[3] = ferros_sys::us_since(t_phase) };
                        frames += 1;
                        FRAMES.store(frames, Ordering::Relaxed);
                        out[1] = moved as u8;
                        len = 2;
                    }
                }
                Some(21) => {
                    client_owned = true;
                    unsafe { CLIENT_DREW = true };
                    // `[column u8]`: that column alone (0-8); absent or past 8: all of them.
                    let only = m.get(1).map(|&c| c as usize).filter(|&c| c < 9);
                    if ten {
                        if unsafe { TEMPORAL } {
                            let (a, b, c, d) = (phase_slice(0, width * height), phase_slice(1, width * height), phase_slice(2, width * height), phase_slice(3, width * height));
                            ramps(&mut [a, b, c, d], width, height, lut, only);
                            // Every phase now holds the ramps; the next frame of the map starts from a clear screen (its first publish covers everything).
                            unsafe {
                                PHASE_DIRTY = [draw::Rect::EMPTY; PHASES];
                                PREV_DMG = draw::Rect::full(width, height);
                            }
                        } else {
                            ramps(&mut [&mut *screen], width, height, lut, only);
                        }
                        ferros_sys::dma_barrier();
                        pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                        let (moved, _) = pipe.present(win);
                        frames += 1;
                        FRAMES.store(frames, Ordering::Relaxed);
                        out[1] = moved as u8;
                        len = 2;
                    } else {
                        out[0] = 3;
                    }
                }
                Some(22) if m.len() >= 18 => {
                    // A window shown or redrawn (WINDOWS.md): [x u16][y u16][w u16][h u16] the part of the screen it covers, [base u64] its surface (the client's own `dma` of it), [level u8] 0 an app, 1 input, 2 the system, and optionally [format u8] its pixels (`Win::format`: 0, the default, the display's working values; 1 fluor's 16-bit g2t). It goes in front of its level; every phase owes the whole screen; the next one is published now, the others while idle.
                    client_owned = true;
                    unsafe { CLIENT_DREW = true };
                    last_interaction = now();
                    let v = |i: usize| u16::from_le_bytes([m[i], m[i + 1]]) as i32;
                    let rect = draw::Rect { x0: v(1), y0: v(3), x1: v(1) + v(5), y1: v(3) + v(7) };
                    let base = u64::from_le_bytes(m[9..17].try_into().unwrap());
                    match surface_of(base) {
                        Some(sk) if ten && unsafe { TEMPORAL } && ferros_sys::pref(8 + sk as u64) != 0 => {
                            let full = draw::Rect::full(width, height);
                            let generation = ferros_sys::pref(8 + sk as u64);
                            // What changed (ferros_sys `show_changed`: [x u16][y u16][w u16][h u16] after the format byte): only that part is published and owed by the other phases — when the window is already up as it was (same generation, place and level) and the screen composed. Otherwise, and for a plain show, the whole screen.
                            let mode = if m.len() > 27 { m[27] } else { 0 };
                            let same = unsafe { WINS[sk] }.is_some_and(|w| w.generation == generation && w.rect == rect && w.level == m[17] && w.mode == mode) && unsafe { APP }.is_some();
                            let changed = if m.len() >= 27 && same { draw::Rect { x0: v(19), y0: v(21), x1: v(19) + v(23), y1: v(21) + v(25) }.intersect(rect) } else { full };
                            let t22 = now();
                            let mut t_chrome = t22;
                            unsafe {
                                if { APP }.is_none() {
                                    chrome_draw(width, height, true);
                                }
                                t_chrome = now();
                                if !same || m.len() < 27 {
                                    RAISES += 1;
                                    WINS[sk] = Some(Win { generation, level: m[17], order: RAISES, rect, format: if m.len() > 18 { m[18] } else { 0 }, mode });
                                    layout_build(width, height);
                                }
                                if OP22_US[4] == 0 {
                                    OP22_US[0] = t_chrome - t22;
                                }
                                APP = Some(full);
                                for k in 0..PHASES {
                                    PHASE_DIRTY[k] = PHASE_DIRTY[k].union(changed);
                                }
                            }
                            let t_settle = now();
                            pipe.settle(&mut armed, true);
                            let k = unsafe { (CUR_PHASE + 1) % PHASES };
                            let t_pub = now();
                            // Everything phase k owes, which now holds `changed`: it is about to be shown, and its dirty region is cleared below. Publishing `changed` alone dropped what it still owed from earlier shows, and stale strips of older frames flashed as the phases rotated (2026-11-01).
                            publish_app(unsafe { PHASE_DIRTY[k] }, &tabs, k, width, height);
                            unsafe {
                                PHASE_DIRTY[k] = draw::Rect::EMPTY;
                                CUR_PHASE = k;
                                // When the windows are gone, the map's next frame starts from a clear screen.
                                PREV_DMG = full;
                            }
                            ferros_sys::dma_barrier();
                            pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                            armed = Some(pipe.arm(win));
                            unsafe {
                                if OP22_US[4] == 0 {
                                    let (te, us) = (now(), |t: u64| t * 1_000_000 / freq());
                                    OP22_US[0] = us(OP22_US[0]);
                                    OP22_US[1] = us(t_settle - t_chrome);
                                    OP22_US[2] = us(t_pub - t_settle);
                                    OP22_US[3] = us(te - t_pub);
                                    OP22_US[4] = us(te - t22);
                                    OP22_AT = te;
                                }
                            }
                            frames += 1;
                            FRAMES.store(frames, Ordering::Relaxed);
                        }
                        _ => out[0] = 3,
                    }
                }
                // Measure: [size u16 px][utf8…] → [0][width u32, quarter pixels]: the advance of the text at that size in the one typeface (the pen moves by each glyph's advance, as op 26 draws it), so an app puts its caret where the letters end and finds the character under a tap (the attest screen, 2026-11-01: placed by an estimate of the advance, its caret wandered off the letters). Synchronous (`op_is_sync`): the reply is the width. Was op 27 for its first day, which was already "whose point" (answered by the receive thread for any call of 13 bytes or more): with that and no wait for its reply, every measure fell back to the estimate.
                Some(28) if m.len() >= 3 => {
                    let px = u16::from_le_bytes([m[1], m[2]]) as f32;
                    let t = core::str::from_utf8(&m[3..]).unwrap_or("");
                    let w = (glyph_engine().width(t, px) * 4.0) as u32;
                    out[1..5].copy_from_slice(&w.to_le_bytes());
                    len = 5;
                }
                // Text into a window's surface, in its 16-bit working values: [base u64] the surface, [x i16][baseline i16][size u16 px][value u16 Q14, white = 16384][utf8…] in screen coordinates (a surface is the whole screen). Glyph coverage times the value, grey, taking the brighter of itself and what is there (so labels sit on black), and clearing the see-through bit where it draws. The app shows the window (op 22) when it is done drawing. Apps have no font of their own yet; the display's is the one typeface (SHELL.md). Synchronous, so the text is in the surface before the app writes it again. Was op 24 until 2026-10-27: the frame wait took that number on 2026-10-24 (answered by the receive thread, never queued), and every text call since had been a frame wait that drew nothing.
                // Op 29, the same with x the centre of the text's ink (window::text_centred, the keyboard's labels, 2026-11-01): the glyphs' coverage found once at a pen of 0 (the cache keeps them for the draw), its leftmost and rightmost pixels' middle put on x. Left and right only: the baseline stays where it is asked.
                Some(op @ (26 | 29)) if m.len() >= 17 => {
                    let i16le = |k: usize| i16::from_le_bytes([m[k], m[k + 1]]) as i32;
                    let (x0, base, px, val) = (i16le(9), i16le(11), u16::from_le_bytes([m[13], m[14]]) as f32, u16::from_le_bytes([m[15], m[16]]) as u32);
                    match surface_of(u64::from_le_bytes(m[1..9].try_into().unwrap())) {
                        Some(sk) if px >= 4.0 && px <= 512.0 => {
                            let surf = surface(sk, width, height);
                            let t = core::str::from_utf8(&m[17..]).unwrap_or("");
                            let x0 = if *op == 29 {
                                let (mut lo, mut hi) = (i64::MAX, i64::MIN);
                                glyph_engine().cover(t, 0.0, 0.0, px, (1.0, 0.0), |x, _, a| {
                                    if a != 0 {
                                        lo = lo.min(x);
                                        hi = hi.max(x);
                                    }
                                });
                                // No ink (a space): centred on its advance instead.
                                if lo > hi { x0 - (glyph_engine().width(t, px) / 2.0) as i32 } else { x0 - ((lo + hi + 1) / 2) as i32 }
                            } else {
                                x0
                            };
                            glyph_engine().cover(t, x0 as f32, base as f32, px, (1.0, 0.0), |x, y, a| {
                                if x < 0 || y < 0 || x >= width as i64 || y >= height as i64 {
                                    return;
                                }
                                let v = (val * a / 255).min(0xFFFF) as u64;
                                let d = &mut surf[y as usize * width + x as usize];
                                let have = if *d & CLEAR != 0 { 0 } else { *d & 0xFFFF };
                                if v > have {
                                    *d = (*d & 1 << 48) | v | v << 16 | v << 32;
                                }
                            });
                            ALLOC.reset();
                        }
                        _ => out[0] = 3,
                    }
                }
                // Focus: [icon u8, 0 none][span µs u32][mode u8: bit 0 fold]. The focused widget is redrawn and flipped every frame; the span is the time the scope shows (2^-6 s to 4 s).
                Some(25) if m.len() >= 6 => {
                    unsafe {
                        FOCUS = m[1];
                        SCOPE_SPAN_US = u32::from_le_bytes([m[2], m[3], m[4], m[5]]).clamp(15_625, 4_000_000);
                        SCOPE_FOLD = m.get(6).copied().unwrap_or(0) & 1 == 1;
                    }
                }
                Some(23) if m.len() >= 9 => {
                    // A window is done: [base u64] its surface. The windows behind it show (the idle fill republishes every phase), or with none left the screen is the map's again (its next frame repaints everything).
                    match surface_of(u64::from_le_bytes(m[1..9].try_into().unwrap())) {
                        Some(sk) => unsafe {
                            WINS[sk] = None;
                            windows_changed(width, height);
                            last_interaction = 0;
                        },
                        None => out[0] = 3,
                    }
                }
                Some(16) if m.len() >= 11 => {
                    let v = |i: usize| i16::from_le_bytes([m[i], m[i + 1]]) as isize;
                    let r = u16::from_le_bytes([m[5], m[6]]) as isize;
                    let colour = 0xFF00_0000 | u32::from_le_bytes([m[7], m[8], m[9], m[10]]) & 0x00FF_FFFF;
                    let r = draw::disc(pixels, width, height, ((v(1) as i64) << 8) + 128, ((v(3) as i64) << 8) + 128, (r as i64) << 8, colour & 0x00FF_FFFF);
                    unsafe { CUR_DMG = CUR_DMG.union(r) };
                }
                Some(17) if m.len() >= 12 => {
                    unsafe { LAST_FRAME.1 = 0 };
                    let i = m[1] as usize;
                    if i < unsafe { SCENE_N } {
                        let i32le = |k: usize| i32::from_le_bytes([m[k], m[k + 1], m[k + 2], m[k + 3]]);
                        unsafe {
                            SCENE[i].x = i32le(2);
                            SCENE[i].y = i32le(6);
                            SCENE[i].r = u16::from_le_bytes([m[10], m[11]]);
                            if m.len() >= 16 {
                                SCENE[i].c = i16::from_le_bytes([m[12], m[13]]);
                                SCENE[i].s = i16::from_le_bytes([m[14], m[15]]);
                            }
                        }
                    } else {
                        out[0] = 1;
                    }
                }
                // 30 `capture`: the frame as it is composed for the panel (windows, the corners, the glass) but in working values, before the panel's table and dither, into a surface no window holds; replies its physical address as text, `G#…` (tools/screencap reads it; the reply holds 16 bytes). With no window up the canvas is the screen: replies `canvas`.
                Some(30) => {
                    let free = (0..SURFACES).find(|&k| unsafe { (*(&raw const WINS))[k] }.is_none());
                    let text: alloc::string::String = match (unsafe { APP }.is_some(), free) {
                        (false, _) => "canvas\n".into(),
                        (true, None) => "no free surface\n".into(),
                        (true, Some(k)) => {
                            let s = surface(k, width, height);
                            unsafe { CAPTURE = s.as_mut_ptr() as usize };
                            publish_app(draw::Rect::full(width, height), &tabs, 0, width, height);
                            unsafe { CAPTURE = 0 };
                            alloc::format!("G#{:X}\n", s.as_ptr() as usize)
                        }
                    };
                    let b = text.as_bytes();
                    let n = b.len().min(out.len());
                    out[..n].copy_from_slice(&b[..n]);
                    len = n;
                }
                // 31 `panel off` (development: the bench for state B, DISPLAY-DESIGN.md §8): display off, sleep in, the 100 ms the panel needs, then its rails and reset through the `panel` switch; the screen stays dark until a display server starts and powers it up again (`diag "restart display"`). Replies 0, or 1 if the switch refused.
                Some(31) if option_env!("FERROS_PRODUCTION").is_none() => {
                    dsi_lock();
                    let _ = dsi.dcs(&[0x28]);
                    let _ = dsi.dcs(&[0x10]);
                    dsi_unlock();
                    ferros_sys::sleep_until(now() + freq() / 10);
                    out[0] = !ferros_sys::power(cap(grants, S_PANEL_POWER), false) as u8;
                    len = 1;
                }
                Some(5) => {
                    out[0..2].copy_from_slice(&(width as u16).to_le_bytes());
                    out[2..4].copy_from_slice(&(height as u16).to_le_bytes());
                    out[4..8].copy_from_slice(&(frames as u32).to_le_bytes());
                    out[8..12].copy_from_slice(&((now() / freq()) as u32).to_le_bytes());
                    out[12] = unsafe { APP }.is_some() as u8;
                    len = 13;
                }
                // The panel, through DSIM: every write bracketed by the hk3 level-2 unlock/lock as the stock driver does.
                Some(6) if m.len() >= 3 => {
                    let dbv = u16::from_le_bytes([m[1], m[2]]).min(4095);
                    let r = dsi.dcs(&[0xF0, 0x5A, 0x5A]).and_then(|_| dsi.dcs(&[0x51, (dbv >> 8) as u8, dbv as u8])).and_then(|_| dsi.dcs(&[0xF0, 0xA5, 0xA5]));
                    out[0] = r.is_err() as u8 * 3;
                }
                Some(7) if m.len() >= 2 => {
                    let (script, fps) = match m[1] {
                        60 => (ferros_dsi::hk3::REFRESH_60, 60),
                        120 => (ferros_dsi::hk3::REFRESH_120, 120),
                        _ => (&[][..], 0),
                    };
                    if fps == 0 {
                        out[0] = 1;
                    } else {
                        let (_, fail) = dsi.sequence(script);
                        dsi.set_cmd_te(fps, hs_mbps, width as u32);
                        out[0] = fail.is_some() as u8 * 3;
                    }
                }
                Some(8) if m.len() >= 2 => {
                    let script = match m[1] {
                        30 => Some(ferros_dsi::hk3::IDLE_30),
                        10 => Some(ferros_dsi::hk3::IDLE_10),
                        1 => Some(ferros_dsi::hk3::IDLE_1),
                        0 => Some(ferros_dsi::hk3::IDLE_EXIT),
                        _ => None,
                    };
                    match script {
                        Some(sc) => out[0] = dsi.sequence(sc).1.is_some() as u8 * 3,
                        None => out[0] = 1,
                    }
                }
                Some(9) if m.len() >= 2 => {
                    out[0] = dsi.dcs(&m[1..]).is_err() as u8 * 3;
                }
                Some(11) => {
                    // The arithmetic the card is built from, logged (2026-10-17: a card came out all code 0 once; a NaN cast is 0).
                    let probe = |l: f32| {
                        let v = tanh(sqrt(l));
                        (v, panel_code(v * v))
                    };
                    let (v0, c0) = probe(1.0 / 64.0);
                    let (v1, c1) = probe(1.0);
                    let (v2, c2) = probe(9.6);
                    log.s(b"display: testcard tanh(sqrt L) at L=1/64,1,9.6: ");
                    for v in [v0, v1, v2] {
                        log.dec((v * 1000.0) as u64);
                        log.s(b"/1000 ");
                    }
                    log.s(b"codes ");
                    for c in [c0, c1, c2] {
                        log.dec(c as u64);
                        log.s(b" ");
                    }
                    log.s(b"libm sqrtf(4)*1000=");
                    log.dec((libm::sqrtf(4.0) * 1000.0) as u64);
                    log.s(b" fsqrt(4)*1000=");
                    log.dec((fsqrt_probe(4.0) * 1000.0) as u64);
                    log.s(b" newton sqrt(4)*1000=");
                    log.dec((sqrt(4.0) * 1000.0) as u64);
                    log.s(b" libm tanhf(1)*1000=");
                    log.dec((libm::tanhf(1.0) * 1000.0) as u64);
                    log.s(b"\n");
                    log.flush();
                    testcard(pixels, width, height);
                    // The finished frame to the framebuffer: reads from cache, writes combined.
                    if ten && unsafe { TEMPORAL } { publish_next(pixels, width, height, &tabs, draw::Rect::full(width, height)) } else if ten { publish10(pixels, screen, width, &tabs) } else { publish(pixels, screen) }
                    ferros_sys::dma_barrier();
                    pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                    let (moved, _) = pipe.present(win);
                    frames += 1;
                        FRAMES.store(frames, Ordering::Relaxed);
                    client_owned = true;
                    unsafe { CLIENT_DREW = true };
                    out[1] = moved as u8;
                    len = 2;
                }
                Some(10) if m.len() >= 3 => {
                    let n = (m[2] as usize).clamp(1, 15);
                    let _ = dsi.dcs(&[0xF0, 0x5A, 0x5A]);
                    match dsi.read(m[1], &mut out[1..1 + n]) {
                        Ok(k) => len = 1 + k,
                        Err(_) => out[0] = 3,
                    }
                    let _ = dsi.dcs(&[0xF0, 0xA5, 0xA5]);
                }
                _ => out[0] = 1,
            }
            len
        };
        // A pass of this loop that runs long holds every client up (touch reads wait for the core): timed, and logged when over 30 ms with its parts.
        let t_pass = now();
        let mut calls_us = 0u64;
        let mut ncalls = 0u32;
        // A clock on the screen, spatial dither only (no flips): a new second, the last plain frame drawn again, as if the map had sent it (the clock reads the time as it draws).
        // The last frame again: a camera op, or each op of a batch in order.
        let mut replay = |handle: &mut dyn FnMut(&[u8], &mut [u8; 16]) -> usize| {
            let (b, n) = unsafe { *(&raw const LAST_FRAME) };
            let m = &b[..n];
            let mut out = [0u8; 16];
            if m.first() == Some(&15) {
                let mut at = 1usize;
                while at + 2 <= m.len() {
                    let l = u16::from_le_bytes([m[at], m[at + 1]]) as usize;
                    if at + 2 + l > m.len() {
                        break;
                    }
                    let _ = handle(&m[at + 2..at + 2 + l], &mut out);
                    at += 2 + l;
                }
            } else if !m.is_empty() {
                let _ = handle(m, &mut out);
            }
        };
        // Gravity at its fixed lag, each pass: drawn at rest on the 10 Hz flips, under a finger on every frame (owner, 2026-10-24: "10 Hz unless touched" — a frame per turn cost the X3 4.4 ms each, 24 % of it on a desk, 53 % in a hand, and 79 °C). The fixed lag keeps the 10 Hz steps even: each shows where the device was 16 ms before it, not whichever sample came last.
        gravity_poll();
        if clock_due {
            clock_due = false;
            unsafe { CLOCK_ONLY = true };
            replay(&mut handle);
            unsafe { CLOCK_ONLY = false };
        }
        // The clock between frames (owner, 2026-10-24: its rings move at the frame rate): when this pass will flip, the clock's region alone is drawn again (`draw::CLIP`) and published into the phase the flip shows, so every phase carries the current clock while it is on the glass. Under a finger the camera ops draw it with everything else; at rest the 10 Hz flips carry it, no frame added.
        let flip_due = unsafe { CLIENT_DREW && TEMPORAL } && !screen_off && ferros_sys::pref(ferros_sys::PREF_DITHER) == 0 && unsafe { PHASE_DIRTY[(CUR_PHASE + 1) % PHASES].is_empty() } && now() >= next_flip;
        // With a widget in focus the flips run at the panel's rate; the clock keeps 10 Hz, drawn into every phase at once (owner, 2026-10-25: the focused widget at full rate, everything else at 10).
        let focus = unsafe { FOCUS } != 0;
        // The clock itself in focus runs at the panel's rate too (its last place turns over ~35 times a second).
        let clock_focus = unsafe { FOCUS } == WIDGET_CLOCK;
        let clock_now = !focus || clock_focus || now() >= clock_slow_next;
        if flip_due && clock_now && unsafe { CLOCK_SHOWN && !{ CLOCK_RECT }.is_empty() && (*(&raw const APP)).is_none() && LAST_FRAME.1 > 0 } {
            if focus && !clock_focus {
                clock_slow_next = now() + freq() / 10;
                unsafe { CLIP_MASK = (1 << PHASES) - 1 };
            }
            let k = unsafe { (CUR_PHASE + 1) % PHASES };
            // What phase k's clock lacks: the wedges the sweeps moved through since it was last drawn there, and the centre if the digits changed; the whole clock when that is not known.
            let mut tb = [0u8; 32];
            let (rings, _, _, key, _) = clock_state(&mut tb);
            let now_c = ClockShown { frac: core::array::from_fn(|i| rings[i].frac_q16), lighter: core::array::from_fn(|i| rings[i].lighter), key, turn: unsafe { GRAV_TURN }.unwrap_or((0, 0)) };
            let whole = ({
                let mut w = [draw::Rect::EMPTY; 8];
                w[0] = unsafe { CLOCK_RECT };
                w
            }, 1);
            let (parts, np) = match unsafe { PHASE_CLOCK[k] } {
                Some(was) => clock_dirty(&was, &now_c).unwrap_or(whole),
                None => whole,
            };
            let t_clip = now();
            let mut px_n = 0u64;
            let mut drew = false;
            for clip in parts[..np].iter().map(|c| c.clip(width, height)).filter(|c| !c.is_empty()) {
                px_n += ((clip.x1 - clip.x0) as u64) * ((clip.y1 - clip.y0) as u64);
                unsafe {
                    draw::CLIP = clip;
                    CLIP_PHASE = Some(k);
                }
                replay(&mut handle);
                drew = true;
                unsafe {
                    CLIP_PHASE = None;
                    draw::CLIP = draw::Rect { x0: 0, y0: 0, x1: i32::MAX, y1: i32::MAX };
                }
            }
            {
                unsafe {
                    // What phase k now holds: the state just drawn, if anything was; unchanged otherwise.
                    if drew {
                        PHASE_CLOCK[k] = Some(BACK_CLOCK);
                        if CLIP_MASK != 0 {
                            PHASE_CLOCK = [Some(BACK_CLOCK); PHASES];
                        }
                    }
                    CLIP_MASK = 0;
                    CLIP_US[0] += ferros_sys::us_since(t_clip);
                    CLIP_US[1] += 1;
                    if CLIP_US[1] == 256 {
                        let mut l = Log { grants, buf: [0; 256], n: 0 };
                        l.s(b"display: clock between frames: ");
                        l.dec(CLIP_US[0] / 256);
                        l.s(b" us each (digits ");
                        l.dec(CLOCK_PART_US[0] / 256);
                        l.s(b" us, face ");
                        l.dec(CLOCK_PART_US[1] / 256);
                        l.s(b" us, the rest the clear and the publish), the last ");
                        l.dec(px_n);
                        l.s(b" px\n");
                        l.flush();
                        CLIP_US = [0; 2];
                        CLOCK_PART_US = [0; 2];
                    }
                }
            }
        }
        // The scope between frames: on every flip while it is on, its plot and labels (SCOPE_LIVE, inside its circle) drawn again into the phase the flip shows (its trace moves every flip: a tenth of its width at 10 Hz, every frame when it is in focus).
        if flip_due && ferros_sys::pref(ferros_sys::PREF_SCOPE) != 0 && unsafe { SCOPE_SHOWN && !{ SCOPE_RECT }.is_empty() && (*(&raw const APP)).is_none() && LAST_FRAME.1 > 0 } {
            let k = unsafe { (CUR_PHASE + 1) % PHASES };
            let clip = unsafe { SCOPE_LIVE.intersect(SCOPE_RECT) }.clip(width, height);
            if !clip.is_empty() {
                let t_s = now();
                unsafe {
                    draw::CLIP = clip;
                    CLIP_PHASE = Some(k);
                }
                replay(&mut handle);
                unsafe {
                    CLIP_PHASE = None;
                    draw::CLIP = draw::Rect { x0: 0, y0: 0, x1: i32::MAX, y1: i32::MAX };
                    SCOPE_US[0] += ferros_sys::us_since(t_s);
                    SCOPE_US[1] += 1;
                    if SCOPE_US[1] == 256 {
                        let mut l = Log { grants, buf: [0; 256], n: 0 };
                        l.s(b"display: scope: ");
                        l.dec(SCOPE_US[0] / 256);
                        l.s(b" us each, ");
                        l.dec(((clip.x1 - clip.x0) as u64) * ((clip.y1 - clip.y0) as u64));
                        l.s(if FOCUS == WIDGET_SCOPE { b" px, in focus; " } else { b" px; " });
                        l.s(b"levels ");
                        l.dec(SCOPE_DBG[0]);
                        l.s(b", newest ages ms ");
                        for k in 1..4 {
                            l.dec(SCOPE_DBG[k]);
                            l.s(b" ");
                        }
                        l.s(b"\n");
                        l.flush();
                        SCOPE_US = [0; 2];
                    }
                }
            }
        }
        // Windows whose process ended (the kernel cleared or renewed its surface's generation byte): gone, and what was behind them shows. Then the corners' clock, once a minute while windows are up, into every phase over its corner.
        unsafe {
            let mut gone = false;
            for sk in 0..SURFACES {
                if let Some(w) = WINS[sk] {
                    if ferros_sys::pref(8 + sk as u64) != w.generation {
                        WINS[sk] = None;
                        gone = true;
                    }
                }
            }
            if gone {
                // The idle fill republishes every phase within an eighth of a second of the last window op.
                windows_changed(width, height);
            }
            if { APP }.is_some() && chrome_draw(width, height, false) {
                let top = draw::Rect { x0: 0, y0: 0, x1: width as i32, y1: CHROME_TOP as i32 };
                let bottom = draw::Rect { x0: 0, y0: (height - CHROME_BOT) as i32, x1: width as i32, y1: height as i32 };
                for k in 0..PHASES {
                    publish_app(top, &tabs, k, width, height);
                    publish_app(bottom, &tabs, k, width, height);
                }
            }
        }
        // Clients first: every request the receive thread has queued, oldest first. The slot is copied out and released before it is run, so the receive thread can refill it meanwhile.
        loop {
            let head = RING_HEAD.load(Ordering::Relaxed);
            if head == RING_TAIL.load(Ordering::Acquire) {
                break;
            }
            let slot = (head % RING as u64) as usize;
            let n = unsafe { RING_LEN[slot] } as usize;
            msg[..n].copy_from_slice(unsafe { &RING_MSG[slot][..n] });
            RING_HEAD.store(head + 1, Ordering::Release);
            ferros_sys::wake(&RING_HEAD, 1);
            let m = &msg[..n];
            let mut out = [0u8; 16];
            let t_call = now();
            ncalls += 1;
            // The time since the last reply: what the client spent between calls (its own work, the scheduler, this loop's own sleep).
            let gap_us = (t_call.wrapping_sub(last_reply) * 1_000_000 / freq()).min(999_999);
            let mut op_us = [0u64; 32];
            let dsi_op = m.first().is_some_and(|&op| (6..=10).contains(&op));
            if dsi_op {
                dsi_lock();
            }
            let len = if m.first() == Some(&15) {
                // A batch: [len u16][op…] after [len u16][op…], run in order; the reply is the last op's, with the first failure's status if any failed.
                let (mut at, mut fail, mut len) = (1usize, 0u8, 1usize);
                while at + 2 <= m.len() {
                    let l = u16::from_le_bytes([m[at], m[at + 1]]) as usize;
                    if at + 2 + l > m.len() || m.get(at + 2) == Some(&15) {
                        fail = 1;
                        break;
                    }
                    let t_op = now();
                    len = handle(&m[at + 2..at + 2 + l], &mut out);
                    let op = m.get(at + 2).copied().unwrap_or(0) as usize;
                    op_us[op.min(31)] += ferros_sys::us_since(t_op);
                    if fail == 0 {
                        fail = out[0];
                    }
                    at += 2 + l;
                }
                out[0] = fail;
                len
            } else {
                handle(m, &mut out)
            };
            unsafe { CUR_OP = 0xEE };
            if m.first() == Some(&14) || (m.first() == Some(&15) && batch_has_camera(m)) {
                unsafe {
                    let lf = &mut *(&raw mut LAST_FRAME);
                    lf.0[..m.len()].copy_from_slice(m);
                    lf.1 = m.len();
                }
            }
            if dsi_op {
                dsi_unlock();
            }
            // How long a call took, by its first op (a batch by its last), logged when slow and at most every two seconds: the frame budget is the thing to watch.
            let took = ferros_sys::us_since(t_call);
            if took > 20_000 && now().wrapping_sub(last_slow_log) > 2 * freq() {
                last_slow_log = now();
                tlog.s(b"display: call op ");
                tlog.dec(m.first().copied().unwrap_or(0) as u64);
                tlog.s(b" (");
                tlog.dec(n as u64);
                tlog.s(b" bytes, ");
                tlog.dec(gap_us / 1000);
                tlog.s(b" ms after the last reply) took ");
                tlog.dec(took / 1000);
                tlog.s(b" ms;");
                for (op, &us) in op_us.iter().enumerate() {
                    if us >= 1000 {
                        tlog.s(b" op ");
                        tlog.dec(op as u64);
                        tlog.s(b" ");
                        tlog.dec(us / 1000);
                        tlog.s(b" ms");
                    }
                }
                // Only when a settle has timed out (until 2026-11-05 the record started at ones and was printed every time, so every slow call read "settle timeout TRIG G#1 SHD G#1 FC G#1 was G#1" when none had).
                if unsafe { SETTLE_TIMEOUT[3] } != 0 {
                    tlog.s(b"; settle timeout TRIG ");
                    tlog.hex(unsafe { SETTLE_TIMEOUT[0] });
                    tlog.s(b" SHD ");
                    tlog.hex(unsafe { SETTLE_TIMEOUT[1] });
                    tlog.s(b" FC ");
                    tlog.hex(unsafe { SETTLE_TIMEOUT[2] });
                    tlog.s(b" was ");
                    tlog.hex(unsafe { SETTLE_TIMEOUT[3] });
                }
                // The first window shown, by part, once.
                if unsafe { OP22_US[4] } != 0 && unsafe { OP22_US[4] } != u64::MAX {
                    for (name, &us) in [b"; first op 22: chrome ".as_slice(), b" ms, layout ", b" ms, settle ", b" ms, publish ", b" ms, whole "].iter().zip(unsafe { OP22_US }.iter()) {
                        tlog.s(name);
                        tlog.dec(us / 1000);
                    }
                    tlog.s(b" ms (chrome discs ");
                    tlog.dec(unsafe { CHROME_US[0] } / 1000);
                    tlog.s(b" ms of ");
                    tlog.dec(unsafe { CHROME_US[1] } / 1000);
                    tlog.s(b" ms, the rest text");
                    tlog.s(b", armed at counter ");
                    tlog.hex(unsafe { OP22_AT });
                    for k in 0..unsafe { FIRST_SHOWS_N } {
                        let (t, op) = unsafe { FIRST_SHOWS[k] };
                        tlog.s(b"; shown at ");
                        tlog.hex(t);
                        tlog.s(b" op ");
                        tlog.hex(op as u32);
                    }
                    tlog.s(b")");
                    unsafe { OP22_US[4] = u64::MAX };
                }
                tlog.s(b"; icons ");
                tlog.dec(unsafe { SCENE_US[0] } / 1000);
                tlog.s(b" ms (drawn; cached: sprites), discs or blits ");
                tlog.dec(unsafe { SCENE_US[1] } / 1000);
                tlog.s(b" ms; publish neon ");
                tlog.dec(unsafe { PUB_TICKS[0] } * 1000 / freq());
                tlog.s(b" ms, copy ");
                tlog.dec(unsafe { PUB_TICKS[1] } * 1000 / freq());
                tlog.s(b" ms, kpx ");
                tlog.dec(unsafe { PUB_TICKS[2] } / 1000);
                tlog.s(b"; camera: scene ");
                for (k, name) in [b" ms, - " as &[u8], b" ms, publish ", b" ms, waited for the last frame ", b" ms\n"].iter().enumerate() {
                    tlog.dec(unsafe { PHASE_US[k] } / 1000);
                    tlog.s(name);
                }
                tlog.flush();
            }
            if m.first().is_some_and(|&op| op_is_sync(op)) {
                unsafe {
                    RING_REPLY[slot] = out;
                    RING_RLEN[slot] = len.min(16) as u8;
                }
                RING_DONE[slot].fetch_add(1, Ordering::Release);
                ferros_sys::wake(&RING_DONE[slot], 1);
            }
            last_reply = now();
            calls_us += ferros_sys::us_since(t_call);
        }

        let t = now();
        if t >= next_tick {
            let secs = t / freq();
            next_tick = (secs + 1) * freq();
            if client_owned && (!unsafe { TEMPORAL } || ferros_sys::pref(ferros_sys::PREF_DITHER) != 0) && unsafe { CLOCK_SHOWN && LAST_FRAME.1 > 0 && (*(&raw const APP)).is_none() } {
                let mut tb = [0u8; 32];
                let (_, _, _, key, _) = clock_state(&mut tb);
                if key != unsafe { CLOCK_KEY } {
                    clock_due = true;
                }
            }
            // The frame rate, once a second while frames are coming (presents and flips both count), with the frame starts DECON reported through its interrupt: the two agree when every frame goes out.
            let fs = FRAME_STARTS.load(Ordering::Relaxed);
            if secs % 10 == 0 {
                tlog.s(b"display: passes ");
                tlog.dec(unsafe { PASSES[0] });
                tlog.s(b", no sleep for a flip ");
                tlog.dec(unsafe { PASSES[1] });
                tlog.s(b", for an owed phase ");
                tlog.dec(unsafe { PASSES[2] });
                tlog.s(b"\n");
                tlog.flush();
                unsafe { PASSES = [0; 3] };
            }
            // The stats line every ten seconds rather than every one: at one a second it alone filled the kernel's console ring and hid every other process's lines (2026-10-26).
            if frames != frames_last_tick && client_owned && secs % 10 == 0 {
                tlog.s(b"display: ");
                tlog.dec(frames - frames_last_tick);
                tlog.s(b" frames/s, ");
                tlog.dec(fs - starts_last_tick);
                tlog.s(b" frame starts/s; camera frames: scene ");
                tlog.dec(unsafe { SEC_US[0] } / 1000);
                tlog.s(b" ms (glow ");
                tlog.dec(unsafe { SEC_US[1] } / 1000);
                tlog.s(b" ms, icons ");
                tlog.dec(unsafe { SEC_US[5] } / 1000);
                tlog.s(b" ms, blits ");
                tlog.dec(unsafe { SEC_US[6] } / 1000);
                tlog.s(b" ms, clear ");
                tlog.dec(unsafe { SEC_US[7] } / 1000);
                tlog.s(b" ms)");
                tlog.s(b" ms, publish ");
                tlog.dec(unsafe { SEC_US[2] } / 1000);
                tlog.s(b" ms, waited for the last frame ");
                tlog.dec(unsafe { SEC_US[3] } / 1000);
                tlog.s(b" ms, in ");
                tlog.dec(unsafe { SEC_US[4] });
                tlog.s(b" camera ops\n");
                tlog.flush();
            }
            unsafe { SEC_US = [0; 8] };
            frames_last_tick = frames;
            starts_last_tick = fs;
            // No client yet: nothing is presented, so the boot ring stays on the glass until a client's first frame replaces it (until 2026-11-05 a black frame went up here each second, and with the layer pointed at it at the start and the dither's black phases it blanked the screen between the ring and the attest screen).
        }
        // Asleep until the next poll or the next tick, whichever is first; the service loop has the CPU meanwhile. A sixteenth of a second at rest — a client waits at most that long — but a hundred-and-twentieth for two seconds after the map moves its camera, since a 62 ms wait per call caps a pinch at sixteen frames a second.
        // The temporal dither: the next phase frame on the layer every 100 ms, once a client owns the surface (the frames are only written by its presents). Measured 2026-10-24 with the screen on (DISPLAY-POWER.md): with the 10 Hz floor set the flips cost nothing (3.03 W against 3.05 without them) — the panel idles at its floor between them; held at 120 Hz they would cost 0.37 W.
        // A frame armed by the last camera op: masked again once it has started.
        pipe.settle(&mut armed, false);
        // Brightness (preference 3): a new white, so a new table and panel setting, and every pixel published again from the back buffer as it stands.
        let b = ferros_sys::pref(ferros_sys::PREF_BRIGHTNESS);
        if b != bright && ten {
            bright = b;
            let (white, peak_e, hbm, dbv) = brightness(b);
            build_lut(lut, white, peak_e);
            tabs = odt_tables(white, peak_e);
            // The panel's own setting, by the panel thread.
            PANEL_BRIGHT.store((hbm as u64) << 16 | dbv as u64, Ordering::Release);
            PANEL_SEQ.fetch_add(1, Ordering::AcqRel);
            ferros_sys::wake(&PANEL_SEQ, 1);
            let full = draw::Rect::full(width, height);
            unsafe {
                PHASE_DIRTY = [full; PHASES];
            }
            let mut pub_us = 0u64;
            if unsafe { TEMPORAL } && client_owned {
                pipe.settle(&mut armed, true);
                let t_p = now();
                publish_next(pixels, width, height, &tabs, full);
                pub_us = ferros_sys::us_since(t_p);
                ferros_sys::dma_barrier();
                pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                armed = Some(pipe.arm(win));
            }
            log.s(b"display: full publish ");
            log.dec(pub_us);
            log.s(b" us; brightness ");
            log.dec(b as u64);
            log.s(b": white ");
            log.dec(white as u64);
            log.s(b" cd/m2, E peak ");
            log.dec(peak_e as u64);
            log.s(if hbm { b" (HBM)\n" } else { b" (DBV " });
            if !hbm {
                log.dec(dbv as u64);
                log.s(b")\n");
            }
            log.flush();
        }
        // The power key (kernel preference 2): the panel's display off and sleep in while the screen is off, sleep out and display on when it comes back, with the frame the layer already holds. No flips while dark.
        // The power key (kernel preference 2): the panel thread does the DSI work; the frame the layer holds is presented again once it says the screen is back.
        let off = ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) == 1;
        if off != screen_off {
            screen_off = off;
            PANEL_SCREEN.store(off as u64, Ordering::Release);
            PANEL_SEQ.fetch_add(1, Ordering::AcqRel);
            ferros_sys::wake(&PANEL_SEQ, 1);
        }
        if PANEL_REPRESENT.swap(0, Ordering::AcqRel) == 1 {
            pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
            let _ = pipe.present(win);
        }
        // A stale phase first, from the back buffer, one per pass so a call waiting on the endpoint is never held up by more than one.
        let t_idle = now();
        // Only once the frames have stopped coming (an eighth of a second without a camera op): while a finger moves, the phases wait and the frames go out spatial only, rather than every frame also paying for a stale phase.
        if unsafe { TEMPORAL } && client_owned && now().wrapping_sub(last_interaction) > freq() / 8 {
            let k = unsafe { (CUR_PHASE + 1) % PHASES };
            if unsafe { !PHASE_DIRTY[k].is_empty() } {
                match unsafe { APP } {
                    Some(_) => publish_app(unsafe { PHASE_DIRTY[k] }, &tabs, k, width, height),
                    None => publish10x4(pixels, width, height, &tabs, 1 << k, unsafe { PHASE_DIRTY[k] }),
                }
                unsafe { PHASE_DIRTY[k] = draw::Rect::EMPTY };
            }
        }
        let idle_us = ferros_sys::us_since(t_idle);
        let t_flip = now();
        let next_clean = unsafe { PHASE_DIRTY[(CUR_PHASE + 1) % PHASES].is_empty() };
        // Spatial only (preference 4): no flips, so the panel can rest at its idle floor; the phase on the layer already carries a complete spatial dither.
        let temporal_on = ferros_sys::pref(ferros_sys::PREF_DITHER) == 0;
        // The flip this pass decided at its top, when the clock was drawn into the phase it shows: re-reading the time here flipped, now and then, to a phase whose clock had not been drawn (its edge 400 ms behind, 2026-10-24).
        if unsafe { TEMPORAL } && client_owned && !screen_off && temporal_on && next_clean && flip_due {
            // The focused widget at the panel's rate, everything else on the 10 Hz flips. In focus the timer is half a frame and `settle` (which holds a flip until the last one has started) does the pacing: a full frame's timer started after each flip's own work slipped a refresh now and then (100 a second, 2026-10-25).
            next_flip = now() + if unsafe { FOCUS } != 0 { freq() / 240 } else { freq() / 10 };
            pipe.settle(&mut armed, true);
            unsafe { CUR_PHASE = (CUR_PHASE + 1) % PHASES };
            pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
            let _ = pipe.present(win);
            frames += 1;
                        FRAMES.store(frames, Ordering::Relaxed);
        }
        let flip_us = ferros_sys::us_since(t_flip);
        let pass_us = ferros_sys::us_since(t_pass);
        if pass_us > 30_000 {
            tlog.s(b"display: long pass ");
            tlog.dec(pass_us / 1000);
            tlog.s(b" ms: ");
            tlog.dec(ncalls as u64);
            tlog.s(b" calls ");
            tlog.dec(calls_us / 1000);
            tlog.s(b" ms, idle fill ");
            tlog.dec(idle_us / 1000);
            tlog.s(b" ms, flip ");
            tlog.dec(flip_us / 1000);
            tlog.s(b" ms\n");
            tlog.flush();
        }
        // Until a request arrives (the receive thread moves the ring's tail) or the next deadline: the second's tick; the flip, only while a flip can happen (while a finger moves the next phase stays dirty and `next_flip` falls into the past — a wake set from it made the old sleep return at once and this loop spin its slice, 2026-10-23); and a stale phase to refill once the frames have stopped coming.
        let flip_possible = unsafe { TEMPORAL } && client_owned && !screen_off && temporal_on && next_clean;
        let mut wake = next_tick;
        if flip_possible {
            wake = wake.min(next_flip.max(now()));
        }
        // Only for what the idle fill above can do this pass: the next phase owed. Woken for any owed phase, the loop never slept while one it does not fill was owed and nothing flipped (~15,700 passes a second at rest, 2026-11-05).
        if unsafe { TEMPORAL } && client_owned && unsafe { !PHASE_DIRTY[(CUR_PHASE + 1) % PHASES].is_empty() } {
            wake = wake.min((last_interaction + freq() / 8).max(now()));
        }
        // Diagnostic (2026-11-05: ~15,700 passes a second at rest): which deadline left nothing to sleep.
        unsafe {
            PASSES[0] += 1;
            if wake <= now() {
                if flip_possible && next_flip <= now() {
                    PASSES[1] += 1;
                } else {
                    PASSES[2] += 1;
                }
            }
        }
        let tail = RING_TAIL.load(Ordering::Acquire);
        if tail == seen_tail {
            ferros_sys::wait(&RING_TAIL, tail, wake);
        }
        seen_tail = RING_TAIL.load(Ordering::Acquire);
    }
}

/// A panic says where and why on the console before the process ends with G#DEAD.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    struct Buf([u8; 240], usize);
    impl core::fmt::Write for Buf {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for &b in s.as_bytes() {
                if self.1 < self.0.len() {
                    self.0[self.1] = b;
                    self.1 += 1;
                }
            }
            Ok(())
        }
    }
    let grants = unsafe { PANIC_GRANTS };
    console(grants, b"display: panic handler entered\n");
    let mut b = Buf([0; 240], 0);
    let _ = core::fmt::write(&mut b, format_args!("display: PANIC {info}\n"));
    console(grants, &b.0[..b.1]);
    ferros_sys::panic_exit()
}
/// The console grant for the panic handler (set first thing in `_entry`; `.data`, see ALLOC).
static mut PANIC_GRANTS: u64 = 1;
