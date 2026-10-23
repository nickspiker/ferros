//! The display server, first cut (DISPLAY-DESIGN.md §3, SHELL.md): an EL0 process that owns the framebuffer and the base layer, paints the shell's first-boot screen with fluor and a real typeface, and presents it on the panel's TE through DECON's trigger — the frame loop that was the kernel's `display::show` until now, as a process.
//!
//! What it owns: the `display-fb` DMA region (the surface the layer DMA reads), a heap region, DECON0's main/win/wincon blocks, layer 0's DMA / DPP / SRAMC / HDR-common blocks, DSIM0 with its PHY, and the GPIO bank with the TE pin. What it takes from the pipe as found: the raster (the layer DMA's image size) and the modeset ABL or the kernel left — the panel, DSIM's link and DECON's own geometry stay as they are. The layer it programs itself, every word (`Pipe::program_layer`), so nothing about window 5 is inherited.
//!
//! Spawn: ferros-bridge spawn display.bin 00 --name display --serve display --hw decon0-main --hw decon0-win --hw decon0-wincon --hw dpu-dma0 --hw dpp0 --hw gpio-gpp0 --dma display-fb --dma display-heap --hw dsim0 --hw dphy0 --hw sramc0 --hw hdrcomm0
//! Grant slots, in that order: 0 console, 1 the `display` endpoint (serve), 2 DECON main, 3 DECON win, 4 DECON wincon, 5 DMA0, 6 DPP0, 7 GPP0, 8 the framebuffer region, 9 the heap region (cacheable; the arc pool is too small for a typeface and the framebuffer region too slow), 10 DSIM0, 11 its PHY (the link rate for the TE timers), 12 SRAMC0, 13 HDRCOMM0 (the layer's geometry words).
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
//! - 12 `scene clear`: empty the map's retained scene (SHELL.md: the phone is a map of your life).
//! - 13 `scene add [count u8][count x circle]`: append circles, each 30 bytes — `x i32`, `y i32` (world units), `r u16`, `ring u8` (0 none, 1 thin, 2 heavy — importance), `lit u8` (1 = changed since last seen, the only colour on the map), `icon u8` (n = the n-th icon in the embedded bundle, 0 = none), `len u8`, `label [16]`.
//! - 14 `camera [cx i32][cy i32][zoom u32, 16.16]`, the centre in world units ×256 (24.8, so a pan at high zoom moves by a pixel, not a world unit): draw the scene through the camera and present. Reply byte 1 = whether DECON advanced.
//!   Optional flag byte 13, bit 0: keep the frame's overlay (see 15). Optional bytes 14-17: the turn, `[cos i16][sin i16]` in Q14 (world to screen is that rotation, then the zoom); absent = upright.
//! - 15 `batch [len u16][op…]…`: several ops in one call, in order — a frame with an overlay is `begin`, its text and rects and discs, then `camera` with the keep flag, which draws the scene under them and presents. One call per frame rather than one per op: each call is a round trip through the scheduler. The reply is the last op's, its status the first failure's.
//! - 16 `disc [x i16][y i16][r u16][colour]`: a filled circle in panel pixels (the shell's shapes are circles; a ring is a smaller disc of the ground colour sent first, since what is sent first is on top).
//! - 17 `scene move [index u8][x i32][y i32][r u16]` (optionally `[cos i16][sin i16]`, the circle's own turn in Q14): one circle of the scene moved, resized and turned in place (the owner arranging by hand).
//! - 21 `ramps [column u8]`: the present stage's test screen, or one column of it alone on black between two thin white lines (dithered bands: our 4×4 to 8 bits, 12 effective; undithered: the 8-bit truncation), straight from 16-bit working values (see `ramps`); status 3 when the pipe is not in 10 bits.
//! The scene is retained here so a frame costs the client three numbers rather than one call per circle: nothing on the map ever moves, so between scene changes only the camera does.
//! fluor's order holds inside a frame: what a client sends first is on top.
//!
//! Pixels are BGRX8888 as the layer reads them (G#xxRRGGBB); fluor draws into the same words with colours packed as G#FF_RRGGBB — its α + darkness convention is only a convention, every blend it does is linear, and the X byte is ignored by the DMA — so there is no conversion pass. What is not a convention is fluor's order: it composites **front to back**, a paint goes *under* what is already there and an opaque pixel is left alone, so a frame starts from transparent (α = 0), the text goes down first and the background last. (The first cut filled black over raw DRAM: every word that happened to carry α = FF survived, a random-colour matrix at 1 in 256 — the owner's "green speckle wallpaper", 2026-10-16.)

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use ferros_sys::{cap, console, freq, mmio_read, mmio_write, now};
use fluor::canvas::{Canvas, Damage};
use fluor::paint;
use fluor::pixel::{Blend, BlendMode};

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
const FRAME_COUNT: u64 = 0x0004;
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
    /// Stop allocating at `end` (the back buffer starts there).
    fn limit(&self, end: usize) {
        unsafe { *self.end.get() = end };
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
const WHITE: u32 = rgb(0xF0, 0xF0, 0xF0);
const GREY: u32 = rgb(0x9A, 0x9A, 0x9A);
const DIM: u32 = rgb(0x5C, 0x5C, 0x5C);
const RULE: u32 = rgb(0x28, 0x28, 0x28);
const AMBER: u32 = rgb(0xFF, 0xA0, 0x00);

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

/// The apps' icons, bundled by tools/app-icons: b"ICN2", count u8, size u16 LE, then per icon size x size RGBA, already cut to a circle, followed by its size x size grey (photopic luminance), all VSF RGB gamma 2 codes. Empty when the build had no bundle.
static ICONS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icons.bin"));

/// The n-th icon (from 1): its RGBA, its grey, and its side, if the bundle has it.
fn icon(n: u8) -> Option<(&'static [u8], &'static [u8], usize)> {
    if n == 0 || ICONS.len() < 7 || &ICONS[..4] != b"ICN2" || n > ICONS[4] {
        return None;
    }
    let size = u16::from_le_bytes([ICONS[5], ICONS[6]]) as usize;
    let at = 7 + (n as usize - 1) * size * size * 5;
    let px = ICONS.get(at..at + size * size * 4)?;
    let grey = ICONS.get(at + size * size * 4..at + size * size * 5)?;
    Some((px, grey, size))
}
/// The map's retained scene. A static, not a heap vector: this process's heap is a bump allocator that every op resets to the mark after the typeface, so anything kept across calls on it would be overwritten by the next text op.
static mut SCENE: [Circle; 256] = [NO_CIRCLE; 256];
static mut SCENE_N: usize = 0;
/// Whether the layer is 10-bit (set once at start; `program_layer` reads it).
static mut TEN: bool = false;
/// The camera op's last phases, microseconds: the scene, the arena reset, the copy to the framebuffer, the present.
static mut PHASE_US: [u64; 4] = [1; 4];
/// The map's colours: a quiet grey plane, the ring a lighter grey, and amber — the only colour — for what changed since it was last seen.
const MAP_FILL: u32 = rgb(0x2A, 0x2A, 0x2A);
const MAP_RING: u32 = rgb(0x8A, 0x8A, 0x8A);
const MAP_LIT: u32 = AMBER;
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
    fn hex(&mut self, v: u32) {
        self.s(b"G#");
        for i in (0..8).rev() {
            self.s(&[b"0123456789ABCDEF"[((v >> (i * 4)) & 0xF) as usize]]);
        }
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
        self.wr(S_DMA0, RDMA_IN_CTRL_0, (ctrl & !(IDMA_FORMAT_MASK | IDMA_ROT_MASK | IDMA_COMP_MASK)) | if ten { 19 << 8 } else { IDMA_BGRX8888 });
        // A 10-bit layer keeps its two low bits only if the DPP and HDR-common blocks are told the format too (bits [2:0] = 1, ARGB8101010) with their 10 bpc bit (6); measured 2026-10-22.
        let fmt = if ten { 1 | 1 << 6 } else { 0 };
        self.wr(S_DPP0, 0x0038, (self.rd(S_DPP0, 0x0038) & !(7 | 1 << 6)) | fmt);
        self.wr(S_HDRCOMM0, 0x000C, (self.rd(S_HDRCOMM0, 0x000C) & !(7 | 1 << 6)) | fmt);
        self.wr(S_DMA0, RDMA_SRC_WIDTH, width);
        self.wr(S_DMA0, 0x0014, height);
        self.wr(S_DMA0, RDMA_SRC_OFFSET, 0);
        self.wr(S_DMA0, RDMA_IMG_SIZE, height << 16 | width);
        self.wr(S_DMA0, RDMA_BASEADDR_P0, fb as u32);
        self.wr(S_DPP0, 0x003C, height << 16 | width);
        self.wr(S_SRAMC0, 0x0014, (height - 1) << 16);
        self.wr(S_HDRCOMM0, 0x0020, height << 16 | width);
    }
    /// One frame: the window's and the global shadow update, the TE trigger unmasked until FRAME_COUNT moves (or a quarter second), then masked again. Returns (frames advanced, RDMA_IRQ).
    fn present(&self, win: u32) -> (bool, u32) {
        let f0 = self.rd(S_DECON, FRAME_COUNT);
        self.wr(S_DECON, SHD_REG_UP_REQ, self.rd(S_DECON, SHD_REG_UP_REQ) | SHD_GLOBAL | SHD_CMP | (1 << win));
        let t = self.rd(S_DECON, TRIG_CON);
        self.wr(S_DECON, TRIG_CON, (t & !HW_TRIG_MASK) | HW_TRIG_EN);
        let t0 = now();
        let limit = freq() / 4;
        let mut moved = false;
        while now().wrapping_sub(t0) < limit {
            if self.rd(S_DECON, FRAME_COUNT) != f0 {
                moved = true;
                break;
            }
        }
        let t = self.rd(S_DECON, TRIG_CON);
        self.wr(S_DECON, TRIG_CON, (t & !HW_TRIG_EN) | HW_TRIG_MASK);
        let irq = self.rd(S_DMA0, RDMA_IRQ);
        self.wr(S_DMA0, RDMA_IRQ, irq & IDMA_IRQ_CLEAR);
        (moved, irq)
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

/// Text through fontdue: glyph coverage blended into the canvas, left-aligned at (x, baseline). Returns the advance.
fn text(c: &mut Canvas, font: &fontdue::Font, s: &str, x: f32, baseline: f32, px: f32, colour: u32) -> f32 {
    let mut pen = x;
    let (w, h) = (c.width as isize, c.height as isize);
    for ch in s.chars() {
        let (m, cov) = font.rasterize(ch, px);
        let gx = pen as isize + m.xmin as isize;
        let gy = baseline as isize - m.ymin as isize - m.height as isize;
        for row in 0..m.height as isize {
            let y = gy + row;
            if y < 0 || y >= h {
                continue;
            }
            for col in 0..m.width as isize {
                let xx = gx + col;
                if xx < 0 || xx >= w {
                    continue;
                }
                let a = cov[(row * m.width as isize + col) as usize] as u32;
                if a == 0 {
                    continue;
                }
                let i = (y * w + xx) as usize;
                let d = c.pixels[i];
                let mix = |sh: u32| {
                    let dc = (d >> sh) & 0xFF;
                    let sc = (colour >> sh) & 0xFF;
                    ((dc * (255 - a) + sc * a) / 255) << sh
                };
                c.pixels[i] = 0xFF00_0000 | mix(16) | mix(8) | mix(0);
            }
        }
        pen += m.advance_width;
    }
    pen - x
}

fn text_width(font: &fontdue::Font, s: &str, px: f32) -> f32 {
    s.chars().map(|ch| font.metrics(ch, px).advance_width).sum()
}

fn text_center(c: &mut Canvas, font: &fontdue::Font, s: &str, baseline: f32, px: f32, colour: u32) {
    let w = text_width(font, s, px);
    text(c, font, s, (c.width as f32 - w) * 0.5, baseline, px, colour);
}

fn text_right(c: &mut Canvas, font: &fontdue::Font, s: &str, right: f32, baseline: f32, px: f32, colour: u32) {
    let w = text_width(font, s, px);
    text(c, font, s, right - w, baseline, px, colour);
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

/// x^p for x > 0, through exp2/log2.
fn pow(x: f32, p: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    libm::exp2f(p * libm::log2f(x))
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
fn testcard(pixels: &mut [u32], width: usize, height: usize) {
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
            pixels[y * width + x] = 0xFF00_0000 | c << 16 | c << 8 | c;
        }
    }
}

/// The present stage's policy (DISPLAY-CALIB.md, "Decided", 2026-10-22): the panel driven at its measured peak (HBM: 1467 cd/m² of native white on this unit) and white at 133 cd/m².
const PEAK_NITS: f32 = 1467.0;
const WHITE_NITS: f32 = 133.0;
/// White is equal-energy E, applied at the tail end (owner): per-channel gains in linear light after the roll-off, so E at full is the brightest the panel shows and some native codes are never reached. Solved from the panel's measured spectra under VSF's observer (Stockman & Sharpe 10°, `tools/panel-white-e.py`, the DBV 3307 readings): R 1.0, G 0.7394, B 0.8172 in light; E keeps 80.1 % of native white's light, which is the peak the roll-off lands on. On the code they are g^(1/2.19), Q15.
const E_CODE_Q15: [u32; 3] = [32768, 28548, 29883];
const E_LIGHT: f32 = 0.801;
/// The panel's measured power law (γ 2.19 in 8- and 10-bit codes, to 10-bit code 8; DISPLAY-CALIB.md).
const PANEL_GAMMA: f32 = 2.19;

/// The present stage's one table: a 16-bit working value (gamma 2, white = 1.0 = Q14 16384, up to 4.0 = 16× white in light) to a 10-bit panel code with 4 fraction bits (Q4, 0..16368), before the white point. One entry folds the whole display transform: the roll-off, Y/peak = tanh²(a·v), always on, with infinity on the panel's peak and `a` solved so white lands exactly on its share of it, tanh²(a) = white ÷ peak, a = atanh(√(white/peak)) (owner, 2026-10-22: at the brightest setting white is half the peak); then the panel's 1/2.19. Shared by the three channels (their curves agree within the instrument's noise); E's per-channel gains come after it. Built once, here, with f32; everything per pixel is integer.
fn build_lut(lut: &mut [u16]) {
    let k = libm::atanhf(sqrt(WHITE_NITS / (PEAK_NITS * E_LIGHT)));
    for (i, e) in lut.iter_mut().enumerate() {
        let y = tanh(i as f32 / 16384.0 * k);
        let q = pow(y, 2.0 / PANEL_GAMMA) * 1023.0 * 16.0 + 0.5;
        *e = if q.is_nan() || q < 0.0 { 0 } else if q > 16368.0 { 16368 } else { q as u16 };
    }
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
/// The phase on the layer now.
static mut CUR_PHASE: usize = 0;

/// The buffer the layer should show: the current phase when the temporal dither is on, else the one framebuffer.
fn layer_fb(fb: usize) -> usize {
    unsafe { if TEMPORAL { PHASE_BASE[CUR_PHASE] } else { fb } }
}

fn phase_slice(k: usize, n: usize) -> &'static mut [u32] {
    unsafe { core::slice::from_raw_parts_mut(PHASE_BASE[k] as *mut u32, n) }
}

/// Phase frames not yet written for the current content: a present writes only the phase on the layer (all four took 71 ms a frame, 2026-10-22), and the idle loop fills the rest one at a time from the back buffer; the flip only moves to a phase that is current.
static mut PHASE_DIRTY: [bool; PHASES] = [true; PHASES];

/// The 8-bit canvas into the phase frames in `mask` (bit k = phase k) in one pass: the table once per pixel, then each phase's threshold and word; a row at a time through cached buffers, then wide copies out.
fn publish10x4(back: &[u32], width: usize, height: usize, lut8: &[[u16; 256]; 3], mask: u32) {
    let mut rows = [[0u32; 1344]; PHASES];
    let w = width.min(1344);
    for y in 0..height {
        let src = &back[y * width..y * width + w];
        // Black first, in one fill per phase row; then only painted pixels are worked (most of the map is empty: the per-pixel loop over all four rows cost 55 ms a frame, 2026-10-22).
        for (k, row) in rows.iter_mut().enumerate() {
            if mask >> k & 1 == 1 {
                row[..w].fill(3);
            }
        }
        let mut x = 0usize;
        while x < w {
            // Four empty pixels at a time skip together.
            if x & 3 == 0 && x + 4 <= w && (src[x] | src[x + 1] | src[x + 2] | src[x + 3]) & 0x00FF_FFFF == 0 {
                x += 4;
                continue;
            }
            let p = src[x] & 0x00FF_FFFF;
            if p == 0 {
                x += 1;
                continue;
            }
            let (r, g, b) = (lut8[0][(p >> 16 & 0xFF) as usize] as u32, lut8[1][(p >> 8 & 0xFF) as usize] as u32, lut8[2][(p & 0xFF) as usize] as u32);
            for (k, row) in rows.iter_mut().enumerate() {
                if mask >> k & 1 == 1 {
                    let th = threshold(x, y, k);
                    row[x] = pack10(dither8(r, th), dither8(g, th), dither8(b, th));
                }
            }
            x += 1;
        }
        for (k, row) in rows.iter().enumerate() {
            if mask >> k & 1 == 1 {
                phase_slice(k, width * height)[y * width..y * width + w].copy_from_slice(&row[..w]);
            }
        }
    }
    unsafe {
        for k in 0..PHASES {
            if mask >> k & 1 == 1 {
                PHASE_DIRTY[k] = false;
            }
        }
    }
}

/// Content changed: write the phase on the layer now, mark the others stale.
fn publish_current(back: &[u32], width: usize, height: usize, lut8: &[[u16; 256]; 3]) {
    unsafe {
        PHASE_DIRTY = [true; PHASES];
        publish10x4(back, width, height, lut8, 1 << CUR_PHASE);
    }
}

/// A 10-bit pixel as the hardware takes it once DMA, DPP and HDR-common agree (kernel `Panel::set_10bit`): B[31:22] G[21:12] R[11:2] A[1:0].
#[inline(always)]
fn pack10(r: u32, g: u32, b: u32) -> u32 {
    b << 22 | g << 12 | r << 2 | 3
}

/// The 8-bit canvas through the present stage: each channel value read as gamma 2 with white at 255, its Q4 code from the table (`lut8`), the dither, the BGRA1010102 word. Empty pixels (most of the map) skip all of it. A row at a time through a cached buffer, then one wide copy into the non-cacheable framebuffer.
fn publish10(back: &[u32], screen: &mut [u32], width: usize, lut8: &[[u16; 256]; 3]) {
    let mut row = [0u32; 4096];
    for (y, (src, dst)) in back.chunks(width).zip(screen.chunks_mut(width)).enumerate() {
        let bay = &BAYER[y & 3];
        let out = &mut row[..src.len()];
        for (x, (s, d)) in src.iter().zip(out.iter_mut()).enumerate() {
            let p = *s & 0x00FF_FFFF;
            if p == 0 {
                *d = 3;
                continue;
            }
            let t = bay[x & 3] as u32 * 4;
            let q = |ch: usize, c: u32| dither8(lut8[ch][c as usize] as u32, t);
            *d = pack10(q(0, p >> 16 & 0xFF), q(1, p >> 8 & 0xFF), q(2, p & 0xFF));
        }
        dst.copy_from_slice(out);
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
fn publish(back: &[u32], screen: &mut [u32]) {
    screen.copy_from_slice(back);
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

/// A filled circle under what is already there (fluor's order), antialiased at its rim, in integers: each row's whole span inside the circle is filled with plain writes and tests, and only the pixel or two at each end of a row gets a coverage from its distance to the centre. fluor's `circle_filled` blends every pixel: 18 ms for a screen-sized disc (2026-10-22).
fn disc(px: &mut [u32], width: usize, height: usize, cx: i64, cy: i64, r: i64, colour: u32) {
    if r <= 0 {
        return;
    }
    let (w, h) = (width as i64, height as i64);
    for y in (cy - r - 1).max(0)..=(cy + r + 1).min(h - 1) {
        let dy = y - cy;
        let outer = (r + 1) * (r + 1) - dy * dy;
        if outer < 0 {
            continue;
        }
        let inner = (r - 1) * (r - 1) - dy * dy;
        let hi = if inner >= 0 { isqrt(inner) } else { -1 };
        let ho = isqrt(outer);
        let row = &mut px[y as usize * width..(y as usize + 1) * width];
        for x in (cx - ho).max(0)..=(cx + ho).min(w - 1) {
            let dx = x - cx;
            let p = &mut row[x as usize];
            if *p >= 0xFF00_0000 {
                continue;
            }
            if dx.abs() <= hi {
                *p = if *p == 0 { colour } else { (*p).under(colour, BlendMode::Normal) };
            } else {
                // Coverage = r − d + ½, in 256ths.
                let d = isqrt((dx * dx + dy * dy) << 16);
                let cov = ((r << 8) + 128 - d).clamp(0, 255);
                if cov > 0 {
                    *p = (*p).under((cov as u32) << 24 | colour & 0x00FF_FFFF, BlendMode::Normal);
                }
            }
        }
    }
}

/// hh:mm:ss since boot — the only clock this process has until a time source is granted.
fn uptime(buf: &mut [u8; 8], secs: u64) -> &str {
    let (h, m, s) = ((secs / 3600) % 100, (secs / 60) % 60, secs % 60);
    let two = |v: u64, o: usize, b: &mut [u8; 8]| {
        b[o] = b'0' + (v / 10) as u8;
        b[o + 1] = b'0' + (v % 10) as u8;
    };
    two(h, 0, buf);
    buf[2] = b':';
    two(m, 3, buf);
    buf[5] = b':';
    two(s, 6, buf);
    core::str::from_utf8(buf).unwrap_or("--:--:--")
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
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

    // The raster as the pipe runs it.
    let img = pipe.rd(S_DMA0, RDMA_IMG_SIZE);
    let (width, height) = ((img & 0xFFFF) as usize, (img >> 16) as usize);
    let win = pipe.base_window();
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
    let back = (heap + HEAP_REGION - width * height * 4) & !0xFFF;
    ALLOC.limit(back);
    // Input byte 0 = 1: a deliberate panic, to prove the panic handler's console line.
    let mut sel = [0u8; 1];
    let _ = input;
    if input_len > 0 && ferros_sys::input(0, &mut sel) == 1 && sel[0] == 1 {
        log.s(b"display: about to panic on purpose\n");
        log.flush();
        panic!("deliberate, at frame zero");
    }

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
    let font = match fontdue::Font::from_bytes(FONT, fontdue::FontSettings::default()) {
        Ok(f) => f,
        Err(_) => {
            log.s(b"display: typeface failed to parse\n");
            log.flush();
            ferros_sys::exit(3);
        }
    };
    // The present stage's table and its 8-bit slice, built before the mark so they live for the process.
    let t_lut = now();
    let lut: &'static mut [u16] = alloc::vec![0u16; 65536].leak();
    build_lut(lut);
    // The 8-bit canvas's tables, one per channel with E's gain on the code already in.
    let mut lut8 = [[0u16; 256]; 3];
    for (ch, t) in lut8.iter_mut().enumerate() {
        for (c, e) in t.iter_mut().enumerate() {
            *e = (lut[c * 16384 / 255] as u32 * E_CODE_Q15[ch] >> 15) as u16;
        }
    }
    let lut_ms = ferros_sys::us_since(t_lut) / 1000;
    ALLOC.mark();
    log.s(b"display: present table (E peak ");
    log.dec((PEAK_NITS * E_LIGHT) as u64);
    log.s(b" cd/m2, white ");
    log.dec(WHITE_NITS as u64);
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

    let pixels: &mut [u32] = unsafe { core::slice::from_raw_parts_mut(back as *mut u32, width * height) };
    let screen: &mut [u32] = unsafe { core::slice::from_raw_parts_mut(fb as *mut u32, width * height) };
    let span = width as f32;
    let (w, h) = (width as f32, height as f32);
    let margin = span * 0.04;
    let status_y = span * 0.05;
    let status_px = span * 0.021;

    pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);

    // The first-boot screen (SHELL.md 3), fluor's way: transparent, then front to back — the text, the marks, the rules; what is left empty shows black (the panel ignores alpha).
    let mut damage = Damage::new();
    pixels.fill(0);
    {
        let mut c = Canvas::new(pixels, width, height, &mut damage);
        text_center(&mut c, &font, "ferros", h * 0.40, span * 0.11, WHITE);
        text_center(&mut c, &font, "No one yet.", h * 0.47, span * 0.032, GREY);
        text_center(&mut c, &font, "Pair with a device you trust.", h * 0.51, span * 0.032, GREY);
        text(&mut c, &font, "apps", margin, h - span * 0.05, span * 0.019, DIM);
        text_right(&mut c, &font, "slot b \u{2022} ferros", w - margin, h - span * 0.05, span * 0.019, GREY);
        paint::fill_rect(&mut c, (w * 0.5 - span * 0.004) as isize, (h * 0.56) as isize, (span * 0.008) as isize, (span * 0.008) as isize, AMBER, None, None);
        paint::fill_rect(&mut c, margin as isize, (status_y + span * 0.03) as isize, (w - 2.0 * margin) as isize, 0, RULE, None, None);
        paint::fill_rect(&mut c, margin as isize, (h - span * 0.09) as isize, (w - 2.0 * margin) as isize, 0, RULE, None, None);
    }
    ALLOC.reset();
    let strip = (status_y + span * 0.02) as usize;

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
        let (_, f1) = dsi.sequence(ferros_dsi::hk3::IDLE_10);
        let (_, f2) = dsi.sequence(ferros_dsi::hk3::TE_FIXED);
        log.s(if f1.is_none() && f2.is_none() { b"display: panel idle floor 10 Hz, TE fixed 120\n" } else { b"display: idle floor / TE command stuck\n" });
        log.flush();
    }

    // 10 bits when the kernel has put DECON in the 10 bpc blend (GLOBAL_CON bit 20, set by the modeset before the shell starts): the layer becomes 10-bit and every frame goes through the present stage, with the panel at its peak (HBM) so the table's headroom is real. Otherwise the 8-bit copy, as before.
    let ten = pipe.rd(S_DECON, 0x0020) & (1 << 20) != 0;
    if ten && dsi.alive() {
        let r = dsi.dcs(&[0xF0, 0x5A, 0x5A]).and_then(|_| dsi.dcs(&[0x53, 0xE0])).and_then(|_| dsi.dcs(&[0x51, 0x0F, 0xFF])).and_then(|_| dsi.dcs(&[0xF0, 0xA5, 0xA5]));
        log.s(if r.is_ok() { b"display: 10 bits, panel at its peak (HBM), white 133 cd/m2\n" } else { b"display: 10 bits, but the HBM command stuck\n" });
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
    let mut next_flip = now() + freq() / 10;

    let mut frames = 0u64;
    let mut clock = [0u8; 8];
    // A client owns the surface from its first `begin` on; until then this process paints its own screen and clock.
    let mut client_owned = false;
    // When the map last moved its camera: while someone is touching, poll the endpoint fast enough to follow a finger; at rest, slowly, so the core sleeps.
    let mut last_interaction = 0u64;
    let mut next_tick = (now() / freq() + 1) * freq();
    let mut msg = [0u8; 4096];
    let mut tok = [0u8; 32];
    let mut last_slow_log = 0u64;
    // A second console writer: `log` is lent to the op handler for the whole pass.
    let mut tlog = Log { grants, buf: [0; 256], n: 0 };
    loop {
        // One op, as the endpoint's header lists them: the reply's first byte is the status, the return the reply's length.
        let mut handle = |m: &[u8], out: &mut [u8; 16]| -> usize {
            let mut len = 1usize;
            out[0] = 0;
            match m.first() {
                Some(1) => {
                    client_owned = true;
                    pixels.fill(0);
                }
                Some(2) if m.len() >= 12 => {
                    let x = u16::from_le_bytes([m[1], m[2]]) as f32;
                    let y = u16::from_le_bytes([m[3], m[4]]) as f32;
                    let px = u16::from_le_bytes([m[5], m[6]]) as f32;
                    let colour = 0xFF00_0000 | u32::from_le_bytes([m[7], m[8], m[9], m[10]]) & 0x00FF_FFFF;
                    let align = m[11];
                    match core::str::from_utf8(&m[12..]) {
                        Ok(t) if px >= 4.0 && px <= 512.0 => {
                            let mut c = Canvas::new(pixels, width, height, &mut damage);
                            match align {
                                1 => {
                                    let tw = text_width(&font, t, px);
                                    text(&mut c, &font, t, x - tw * 0.5, y, px, colour);
                                }
                                2 => text_right(&mut c, &font, t, x, y, px, colour),
                                _ => {
                                    text(&mut c, &font, t, x, y, px, colour);
                                }
                            }
                            ALLOC.reset();
                        }
                        _ => out[0] = 1,
                    }
                }
                Some(3) if m.len() >= 13 => {
                    let v = |i: usize| u16::from_le_bytes([m[i], m[i + 1]]) as isize;
                    let colour = 0xFF00_0000 | u32::from_le_bytes([m[9], m[10], m[11], m[12]]) & 0x00FF_FFFF;
                    let mut c = Canvas::new(pixels, width, height, &mut damage);
                    paint::fill_rect(&mut c, v(1), v(3), v(5), v(7), colour, None, None);
                }
                Some(4) => {
                    // The finished frame to the framebuffer: reads from cache, writes combined.
                    if ten && unsafe { TEMPORAL } { publish_current(pixels, width, height, &lut8) } else if ten { publish10(pixels, screen, width, &lut8) } else { publish(pixels, screen) }
                    ferros_sys::dma_barrier();
                    pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                    let (moved, _) = pipe.present(win);
                    frames += 1;
                    out[1] = moved as u8;
                    len = 2;
                }
                Some(12) => {
                    client_owned = true;
                    unsafe { SCENE_N = 0 };
                }
                Some(13) if m.len() >= 2 => {
                    client_owned = true;
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
                    last_interaction = now();
                    let cx = i32::from_le_bytes([m[1], m[2], m[3], m[4]]) as f32 / 256.0;
                    let cy = i32::from_le_bytes([m[5], m[6], m[7], m[8]]) as f32 / 256.0;
                    let z = u32::from_le_bytes([m[9], m[10], m[11], m[12]]) as f32 / 65536.0;
                    // Flag bit 0: keep what the frame already holds (a `begin` and the client's text and rects, sent first in the same batch), so the scene goes under an overlay; without it the frame starts empty.
                    if m.get(13).copied().unwrap_or(0) & 1 == 0 {
                        pixels.fill(0);
                    }
                    // The turn: (cos, sin), Q14 on the wire.
                    let (rc, rs) = if m.len() >= 18 {
                        (i16::from_le_bytes([m[14], m[15]]) as f32 / 16384.0, i16::from_le_bytes([m[16], m[17]]) as f32 / 16384.0)
                    } else {
                        (1.0, 0.0)
                    };
                    let mut t_phase = now();
                    {
                        let mut c = Canvas::new(pixels, width, height, &mut damage);
                        for i in 0..unsafe { SCENE_N } {
                            let cir = unsafe { SCENE[i] };
                            let (dx, dy) = (cir.x as f32 - cx, cir.y as f32 - cy);
                            let sx = (rc * dx - rs * dy) * z + w * 0.5;
                            let sy = (rs * dx + rc * dy) * z + h * 0.5;
                            let sr = cir.r as f32 * z;
                            if sr < 1.0 || sx + sr < 0.0 || sx - sr > w || sy + sr < 0.0 || sy - sr > h {
                                continue;
                            }
                            // Importance is the ring's weight, in screen pixels so it stays legible at every zoom.
                            let stroke = match cir.ring { 0 => 0.0, 1 => libm::fmaxf(1.0, sr * 0.04), _ => libm::fmaxf(2.0, sr * 0.12) };
                            let lit = cir.lit != 0;
                            // fluor composites front to back: the label first, then the fill, then the ring's larger disc, which lands only on the band the fill left open.
                            // Zoom is semantic: a name appears only once the circle is big enough to read it.
                            if sr >= 36.0 && cir.len > 0 {
                                if let Ok(t) = core::str::from_utf8(&cir.label[..cir.len as usize]) {
                                    let px = libm::fminf(libm::fmaxf(sr * 0.32, 12.0), 72.0);
                                    let tw = text_width(&font, t, px);
                                    text(&mut c, &font, t, sx - tw * 0.5, sy + px * 0.35, px, if lit { BLACK } else { WHITE });
                                }
                            }
                            // The app's icon, inside the ring: grey at rest, identifying by shape, and in its own colours only while lit — so colour on the map still means exactly one thing, that this changed.
                            if let Some((px, grey, size)) = icon(cir.icon) {
                                let ri = sr - stroke;
                                let (x0, y0) = ((sx - ri).max(0.0) as usize, (sy - ri).max(0.0) as usize);
                                let (x1, y1) = (((sx + ri) as usize).min(width), ((sy + ri) as usize).min(height));
                                let step = size as f32 / (2.0 * ri);
                                let half = size as f32 * 0.5;
                                // The icon's turn on screen: the camera's, then the circle's own.
                                let (cc, cs) = (cir.c as f32 / 16384.0, cir.s as f32 / 16384.0);
                                let (rc, rs) = (rc * cc - rs * cs, rs * cc + rc * cs);
                                for yy in y0..y1 {
                                    let oy = yy as f32 - sy;
                                    for xx in x0..x1 {
                                        // Front to back: only where nothing is painted yet (tested first: under an arrived circle's overlay that is every pixel), and only the icon's solid part — its rim goes to the fill underneath, which carries the antialiased edge.
                                        let dst = &mut c.pixels[yy * width + xx];
                                        if *dst >> 24 != 0 {
                                            continue;
                                        }
                                        // The icon turns with the map: each panel pixel taken back through the rotation into the icon.
                                        let ox = xx as f32 - sx;
                                        let (fu, fv) = ((rc * ox + rs * oy) * step + half, (rc * oy - rs * ox) * step + half);
                                        if fu < 0.0 || fv < 0.0 {
                                            continue;
                                        }
                                        let (u, v) = ((fu as usize).min(size - 1), (fv as usize).min(size - 1));
                                        let k = (v * size + u) * 4;
                                        if px[k + 3] < 128 {
                                            continue;
                                        }
                                        let rgb = if lit {
                                            (px[k] as u32) << 16 | (px[k + 1] as u32) << 8 | px[k + 2] as u32
                                        } else {
                                            let y = grey[k / 4] as u32;
                                            y << 16 | y << 8 | y
                                        };
                                        *dst = 0xFF00_0000 | rgb;
                                    }
                                }
                            }
                            disc(c.pixels, width, height, sx as i64, sy as i64, (sr - stroke) as i64, if lit { MAP_LIT } else { MAP_FILL });
                            if stroke > 0.0 {
                                disc(c.pixels, width, height, sx as i64, sy as i64, sr as i64, MAP_RING);
                            }
                        }
                        unsafe { PHASE_US[0] = ferros_sys::us_since(t_phase) };
                        t_phase = now();
                    }
                    ALLOC.reset();
                    unsafe { PHASE_US[1] = ferros_sys::us_since(t_phase) };
                    t_phase = now();
                    // The finished frame to the framebuffer: reads from cache, writes combined.
                    if ten && unsafe { TEMPORAL } { publish_current(pixels, width, height, &lut8) } else if ten { publish10(pixels, screen, width, &lut8) } else { publish(pixels, screen) }
                    ferros_sys::dma_barrier();
                    unsafe { PHASE_US[2] = ferros_sys::us_since(t_phase) };
                    t_phase = now();
                    pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                    let (moved, _) = pipe.present(win);
                    unsafe { PHASE_US[3] = ferros_sys::us_since(t_phase) };
                    frames += 1;
                    out[1] = moved as u8;
                    len = 2;
                }
                Some(21) => {
                    client_owned = true;
                    // `[column u8]`: that column alone (0-8); absent or past 8: all of them.
                    let only = m.get(1).map(|&c| c as usize).filter(|&c| c < 9);
                    if ten {
                        if unsafe { TEMPORAL } {
                            let (a, b, c, d) = (phase_slice(0, width * height), phase_slice(1, width * height), phase_slice(2, width * height), phase_slice(3, width * height));
                            ramps(&mut [a, b, c, d], width, height, lut, only);
                            unsafe { PHASE_DIRTY = [false; PHASES] };
                        } else {
                            ramps(&mut [&mut *screen], width, height, lut, only);
                        }
                        ferros_sys::dma_barrier();
                        pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                        let (moved, _) = pipe.present(win);
                        frames += 1;
                        out[1] = moved as u8;
                        len = 2;
                    } else {
                        out[0] = 3;
                    }
                }
                Some(16) if m.len() >= 11 => {
                    let v = |i: usize| i16::from_le_bytes([m[i], m[i + 1]]) as isize;
                    let r = u16::from_le_bytes([m[5], m[6]]) as isize;
                    let colour = 0xFF00_0000 | u32::from_le_bytes([m[7], m[8], m[9], m[10]]) & 0x00FF_FFFF;
                    let mut c = Canvas::new(pixels, width, height, &mut damage);
                    disc(c.pixels, width, height, v(1) as i64, v(3) as i64, r as i64, colour);
                }
                Some(17) if m.len() >= 12 => {
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
                Some(5) => {
                    out[0..2].copy_from_slice(&(width as u16).to_le_bytes());
                    out[2..4].copy_from_slice(&(height as u16).to_le_bytes());
                    out[4..8].copy_from_slice(&(frames as u32).to_le_bytes());
                    out[8..12].copy_from_slice(&((now() / freq()) as u32).to_le_bytes());
                    len = 12;
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
                    if ten && unsafe { TEMPORAL } { publish_current(pixels, width, height, &lut8) } else if ten { publish10(pixels, screen, width, &lut8) } else { publish(pixels, screen) }
                    ferros_sys::dma_barrier();
                    pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                    let (moved, _) = pipe.present(win);
                    frames += 1;
                    client_owned = true;
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
        // Clients first: every call waiting on the endpoint.
        loop {
            let n = ferros_sys::recv(cap(grants, S_DISPLAY), &mut msg, &mut tok, true);
            if n == ferros_sys::NOTHING || n == ferros_sys::ERR {
                break;
            }
            let m = &msg[..(n as usize).min(msg.len())];
            let mut out = [0u8; 16];
            let t_call = now();
            let mut op_us = [0u64; 32];
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
            // How long a call took, by its first op (a batch by its last), logged when slow and at most every two seconds: the frame budget is the thing to watch.
            let took = ferros_sys::us_since(t_call);
            if took > 20_000 && now().wrapping_sub(last_slow_log) > 2 * freq() {
                last_slow_log = now();
                tlog.s(b"display: call op ");
                tlog.dec(m.first().copied().unwrap_or(0) as u64);
                tlog.s(b" (");
                tlog.dec(n);
                tlog.s(b" bytes) took ");
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
                tlog.s(b"; camera: scene ");
                for (k, name) in [b" ms, arena " as &[u8], b" ms, copy ", b" ms, present ", b" ms\n"].iter().enumerate() {
                    tlog.dec(unsafe { PHASE_US[k] } / 1000);
                    tlog.s(name);
                }
                tlog.flush();
            }
            ferros_sys::reply(&tok, &out[..len]);
        }

        let t = now();
        if t >= next_tick {
            let secs = t / freq();
            next_tick = (secs + 1) * freq();
            if !client_owned {
                // The status line: uptime left, the link right; repainted every second — the strip cleared to transparent, text, then black under it.
                pixels[..strip * width].fill(0);
                {
                    let mut c = Canvas::new(pixels, width, height, &mut damage);
                    let tt = uptime(&mut clock, secs);
                    text(&mut c, &font, tt, margin, status_y, status_px, GREY);
                    text_right(&mut c, &font, "USB \u{2022} slot b", w - margin, status_y, status_px, DIM);
                    paint::fill_rect(&mut c, 0, 0, width as isize, strip as isize, BLACK, None, None);
                }
                ALLOC.reset();
                // The finished frame to the framebuffer: reads from cache, writes combined.
                if ten && unsafe { TEMPORAL } { publish_current(pixels, width, height, &lut8) } else if ten { publish10(pixels, screen, width, &lut8) } else { publish(pixels, screen) }
                ferros_sys::dma_barrier();
                // The layer is ours every frame: the kernel's bench ops (bridge display …) still re-point it when they run.
                pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
                let (moved, irq) = pipe.present(win);
                frames += 1;
                if frames == 1 || frames % 30 == 0 || !moved || irq & !(1 << 16) != 0 {
                    log.s(b"display: frame ");
                    log.dec(frames);
                    log.s(if moved { b" ok, RDMA_IRQ " } else { b" NOT SHOWN, RDMA_IRQ " });
                    log.hex(irq);
                    log.s(b" INT_PEND ");
                    log.hex(pipe.rd(S_DECON, INT_PEND));
                    log.s(b"/");
                    log.hex(pipe.rd(S_DECON, INT_PEND_EXTRA));
                    log.s(if pipe.te() { b" TE high\n" } else { b" TE low\n" });
                    log.flush();
                }
            }
        }
        // Asleep until the next poll or the next tick, whichever is first; the service loop has the CPU meanwhile. A sixteenth of a second at rest — a client waits at most that long — but a hundred-and-twentieth for two seconds after the map moves its camera, since a 62 ms wait per call caps a pinch at sixteen frames a second.
        // The temporal dither: the next phase frame on the layer every 100 ms, once a client owns the surface (the frames are only written by its presents). The panel then never rests at its idle floor; the owner chose the bits over the ~0.2 W (2026-10-22).
        // A stale phase first, from the back buffer, one per pass so a call waiting on the endpoint is never held up by more than one.
        if unsafe { TEMPORAL } && client_owned {
            if let Some(k) = (0..PHASES).find(|&k| unsafe { PHASE_DIRTY[k] }) {
                publish10x4(pixels, width, height, &lut8, 1 << k);
            }
        }
        let next_clean = unsafe { !PHASE_DIRTY[(CUR_PHASE + 1) % PHASES] };
        if unsafe { TEMPORAL } && client_owned && next_clean && now() >= next_flip {
            next_flip = now() + freq() / 10;
            unsafe { CUR_PHASE = (CUR_PHASE + 1) % PHASES };
            pipe.program_layer(win, layer_fb(fb), width as u32, height as u32);
            let _ = pipe.present(win);
            frames += 1;
        }
        let poll = if now().wrapping_sub(last_interaction) < 2 * freq() { freq() / 120 } else { freq() / 16 };
        let wake = if unsafe { TEMPORAL } && client_owned { next_tick.min(next_flip) } else { next_tick };
        ferros_sys::sleep_until(wake.min(now() + poll));
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
