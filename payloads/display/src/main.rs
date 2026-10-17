//! The display server, first cut (DISPLAY-DESIGN.md §3, SHELL.md): an EL0 process that owns the framebuffer and the base layer, paints the shell's first-boot screen with fluor and a real typeface, and presents it on the panel's TE through DECON's trigger — the frame loop that was the kernel's `display::show` until now, as a process.
//!
//! What it owns: the `display-fb` DMA region (the surface the layer DMA reads, plus this process's heap above it), DECON0's main/win/wincon blocks, layer 0's DMA and DPP blocks, and the GPIO bank with the TE pin. What it takes from the pipe as found: the raster (DPP0's image size) and the modeset ABL or the kernel left — the panel, DSIM and DECON's geometry stay as they are; this process only re-points the layer and pushes frames.
//!
//! Spawn: ferros-bridge spawn display.bin 00 --name display --serve display --hw decon0-main --hw decon0-win --hw decon0-wincon --hw dpu-dma0 --hw dpp0 --hw gpio-gpp0 --dma display-fb --dma display-heap --hw dsim0 --hw dphy0
//! Grant slots, in that order: 0 console, 1 the `display` endpoint (serve), 2 DECON main, 3 DECON win, 4 DECON wincon, 5 DMA0, 6 DPP0, 7 GPP0, 8 the framebuffer region, 9 the heap region (cacheable; the arc pool is too small for a typeface and the framebuffer region too slow), 10 DSIM0, 11 its PHY (the link rate for the TE timers).
//!
//! The `display` endpoint (SHELL.md: the shell draws lists, this process paints). One op per call, first byte the op, coordinates u16 little-endian in panel pixels, colours u32 little-endian G#00RRGGBB, one status byte back (0 = ok, 1 = bad message, 2 = too big):
//! - 1 `begin`: a new frame — the whole surface transparent. Until a client's first `begin`, this process paints its own first-boot screen.
//! - 2 `text [x][y baseline][px][colour][align: 0 left, 1 centre at x, 2 right edge at x][utf-8…]`.
//! - 3 `rect [x][y][w][h][colour]` (w or h = 0: a one-pixel rule).
//! - 4 `present`: black under everything, then the frame goes to the panel. Reply byte 1 = whether DECON advanced.
//! - 5 `info` → `[w u16][h u16][frames u32][uptime s u32]`.
//! - 6 `brightness [dbv u16]`: the panel's DBV (196 = 2 nits … 3307 = 1000 nits; DISPLAY-HK3.md §4), through DSIM from this process.
//! - 7 `refresh [hz u8]`: 60 or 120, the panel's own command set plus DSIM's command-TE timers.
//! - 8 `idle [floor u8: 30, 10, 1, 0 = off]`: the panel's auto-rate floor.
//! - 9 `dcs [bytes…]`: one raw DCS write (dev). 10 `read [reg u8][len u8]` → the bytes (dev).
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

/// The heap region (cachemap DISPLAY_HEAP): 16 MiB, cacheable. The typeface parse alone allocates 14.7 MiB in total (3.7 MiB live at the peak) and this allocator frees nothing until the frame arena resets, so the region has to hold the whole parse.
const HEAP_BYTES: usize = 16 << 20;

/// A bump allocator over the heap in the framebuffer region, with a mark: everything allocated before `mark()` (the parsed typeface) lives for the process; everything after it is the frame's scratch (glyph bitmaps), thrown away by `reset()` after each frame.
struct Bump {
    base: UnsafeCell<usize>,
    next: UnsafeCell<usize>,
    mark: UnsafeCell<usize>,
}
unsafe impl Sync for Bump {}

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let base = *self.base.get();
        if base <= 1 {
            return core::ptr::null_mut();
        }
        let start = (*self.next.get() + l.align() - 1) & !(l.align() - 1);
        if start + l.size() > base + HEAP_BYTES {
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
static ALLOC: Bump = Bump { base: UnsafeCell::new(1), next: UnsafeCell::new(1), mark: UnsafeCell::new(1) };

/// Oxanium (OFL, assets/fonts), the owner's choice for ferros's face — the same family as the "Ready?" screen.
static FONT: &[u8] = include_bytes!("../../../assets/fonts/Oxanium-Regular.ttf");

/// An opaque colour in the layer's own byte order, which fluor blends linearly like any other word.
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}
const WHITE: u32 = rgb(0xF0, 0xF0, 0xF0);
const GREY: u32 = rgb(0x9A, 0x9A, 0x9A);
const DIM: u32 = rgb(0x5C, 0x5C, 0x5C);
const RULE: u32 = rgb(0x28, 0x28, 0x28);
const AMBER: u32 = rgb(0xFF, 0xA0, 0x00);
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
    /// Point the base layer at `fb` (translation is off, so the DMA address is the physical one), linear BGRX8888, stride = width.
    fn take_layer(&self, fb: usize, width: u32) {
        let ctrl = self.rd(S_DMA0, RDMA_IN_CTRL_0);
        self.wr(S_DMA0, RDMA_IN_CTRL_0, (ctrl & !(IDMA_FORMAT_MASK | IDMA_ROT_MASK | IDMA_COMP_MASK)) | IDMA_BGRX8888);
        self.wr(S_DMA0, RDMA_BASEADDR_P0, fb as u32);
        self.wr(S_DMA0, RDMA_SRC_WIDTH, width);
        self.wr(S_DMA0, RDMA_SRC_OFFSET, 0);
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
    // Input byte 0 = 1: a deliberate panic, to prove the panic handler's console line.
    let mut sel = [0u8; 1];
    let _ = input;
    if input_len > 0 && ferros_sys::input(0, &mut sel) == 1 && sel[0] == 1 {
        log.s(b"display: about to panic on purpose\n");
        log.flush();
        panic!("deliberate, at frame zero");
    }

    let t_font = now();
    let font = match fontdue::Font::from_bytes(FONT, fontdue::FontSettings::default()) {
        Ok(f) => f,
        Err(_) => {
            log.s(b"display: typeface failed to parse\n");
            log.flush();
            ferros_sys::exit(3);
        }
    };
    ALLOC.mark();
    log.s(b"display: typeface up in ");
    log.dec(ferros_sys::us_since(t_font) / 1000);
    log.s(b" ms, heap ");
    log.dec(ALLOC.used() as u64 >> 10);
    log.s(b" KiB\n");
    log.flush();

    let pixels: &mut [u32] = unsafe { core::slice::from_raw_parts_mut(fb as *mut u32, width * height) };
    let span = width as f32;
    let (w, h) = (width as f32, height as f32);
    let margin = span * 0.04;
    let status_y = span * 0.05;
    let status_px = span * 0.021;

    pipe.take_layer(fb, width as u32);

    // The first-boot screen (SHELL.md 3), fluor's way: transparent, then front to back — the text, the marks, the rules, and the black last, under everything.
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
        paint::fill_rect(&mut c, 0, 0, width as isize, height as isize, BLACK, None, None);
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
    // Nothing moves between presents, so the panel may rest: the 10 Hz auto-rate floor (its own engine drops the refresh when frames stop and comes back for the next one; DISPLAY-POWER.md: 150–190 mW under 120 Hz). A client can change it (op 8).
    if dsi.alive() {
        let (_, fail) = dsi.sequence(ferros_dsi::hk3::IDLE_10);
        log.s(if fail.is_none() { b"display: panel idle floor 10 Hz\n" } else { b"display: idle floor command stuck\n" });
        log.flush();
    }

    let mut frames = 0u64;
    let mut clock = [0u8; 8];
    // A client owns the surface from its first `begin` on; until then this process paints its own screen and clock.
    let mut client_owned = false;
    let mut next_tick = (now() / freq() + 1) * freq();
    let mut msg = [0u8; 4096];
    let mut tok = [0u8; 32];
    loop {
        // Clients first: every call waiting on the endpoint.
        loop {
            let n = ferros_sys::recv(cap(grants, S_DISPLAY), &mut msg, &mut tok, true);
            if n == ferros_sys::NOTHING || n == ferros_sys::ERR {
                break;
            }
            let m = &msg[..(n as usize).min(msg.len())];
            let mut out = [0u8; 16];
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
                    {
                        let mut c = Canvas::new(pixels, width, height, &mut damage);
                        paint::fill_rect(&mut c, 0, 0, width as isize, height as isize, BLACK, None, None);
                    }
                    ferros_sys::dma_barrier();
                    pipe.take_layer(fb, width as u32);
                    let (moved, _) = pipe.present(win);
                    frames += 1;
                    out[1] = moved as u8;
                    len = 2;
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
                ferros_sys::dma_barrier();
                // The layer is ours every frame: the kernel's bench ops (bridge display …) still re-point it when they run.
                pipe.take_layer(fb, width as u32);
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
        // Asleep until the next poll (a sixteenth of a second: a client waits at most that long) or the next tick, whichever is first; the service loop has the CPU meanwhile.
        ferros_sys::sleep_until(next_tick.min(now() + freq() / 16));
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
