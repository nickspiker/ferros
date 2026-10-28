//! ahau: the attest screen (Claude memories/ferros_attest_os_level.md; the capsule the owner named, Māori "I, me"). The first thing on the glass at boot: init starts it before the map, at the system level, so it covers everything until the person proves who they are by typing their handle on the honeycomb keyboard. Then its window goes and the map is there.
//!
//! The derivation is ihi's, computed here without std (ahau_core): the identity seed is BLAKE3 over vsf's `x` encoding of the handle, byte-exact for every character the keyboard can type. The device keeps a verifier, the seed under its ira's `ahau verifier` key, in the vault scope `ahau.`; matching it means the typed handle is this device's identity.
//!
//! - A device that knows its identity (a verifier, or the Photon roots pushed at pairing, `photon.identity_seed`): the handle is checked against it; the first match writes the verifier.
//! - A device with no identity yet: the handle is typed, then typed again (minting happens once, so a typo would be permanent), then its handle proof (ihi's memory-hard proof, 24.9 MB of scratch, 17 rounds) is derived, and the roots go where the Photon client reads them (`photon.identity_seed`, `photon.handle_proof`) beside the verifier.
//!
//! The handle shows as it is typed: it is a name the person proves, not a password (owner, 2026-10-27: "this is NOT a password"). A wrong one stays on screen to be corrected; the bar's left key clears it. Minting a new identity asks for the handle a second time, shown as typed like the first, the first cleared so a typo is not simply copied (owner, 2026-10-27: "you only do that once"); clear during the second goes back to the first. The buffer is wiped once attest succeeds.
//!
//! The scratch is this process's own .bss (25 MiB): the entry's header (`ferrosRT`, ferros_rt's) tells the kernel how much zeroed memory follows the code, so the arc holds it (payload.ld).
//!
//! Grant slots: 0 console, 1 display (use), 2 touch (use), 3 its surface, 4 the vault scope `ahau.`, 5 the vault scope `photon.`, 6 the ira key.
#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::arch::global_asm;
use ferros_sys::{call, cap, console, exit, freq, now, sleep_until, window};
use honeycomb::{Event, Key, Keyboard, Touch, Touches};

const S_DISPLAY: u64 = 1;
const S_TOUCH: u64 = 2;
const S_SURF: u64 = 3;
const S_AHAU: u64 = 4;
const S_PHOTON: u64 = 5;
const S_IRA: u64 = 6;
/// The longest handle the screen takes, in characters.
const MAX_CHARS: usize = 64;
/// Grey working values: the mark, the prompt, the status line (gamma 2, white = Q14).
const MARK: u64 = 9000;
const TEXT: u16 = 16384;
const QUIET: u16 = 7000;

global_asm!(
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
    main = sym ahau_main
);

/// The handle proof's scratch and nothing else: a bump allocator over a zeroed static, whose last allocation can be given back (the proof's one buffer, so a second derivation fits).
#[repr(C, align(64))]
struct Heap([u8; 25 << 20]);
static mut HEAP: Heap = Heap([0; 25 << 20]);
static mut HEAP_NEXT: usize = 0;

struct Bump;

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe {
            let base = (&raw mut HEAP) as usize;
            let start = (base + HEAP_NEXT + l.align() - 1) & !(l.align() - 1);
            if start + l.size() > base + (25 << 20) {
                return core::ptr::null_mut();
            }
            HEAP_NEXT = start + l.size() - base;
            start as *mut u8
        }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe {
            let base = (&raw mut HEAP) as usize;
            if p as usize + l.size() == base + HEAP_NEXT {
                HEAP_NEXT = p as usize - base;
            }
        }
    }
}

#[global_allocator]
static ALLOC: Bump = Bump;

/// What the screen is waiting for.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// The device knows its identity: check the handle against it.
    Known,
    /// No identity yet: the first entry.
    Fresh,
    /// No identity yet: the same handle again, whose seed is this.
    Confirm([u8; 32]),
}

struct Ahau {
    grants: u64,
    display: u64,
    base: u64,
    w: usize,
    h: usize,
    kb: Keyboard,
    mode: Mode,
    /// The verifier the device keeps, if any, and the Photon seed pushed at pairing, if any.
    verifier: Option<[u8; 32]>,
    photon_seed: Option<[u8; 32]>,
    device_key: [u8; 32],
    handle: [u8; 4 * MAX_CHARS],
    len: usize,
    chars: usize,
}

impl Ahau {
    fn surf(&self) -> &'static mut [u64] {
        unsafe { core::slice::from_raw_parts_mut(self.base as *mut u64, self.w * self.h) }
    }

    fn label(&self) -> impl FnMut(i64, i64, u16, u16, &str) + '_ {
        move |x, y, px, v, t| window::text(self.display, self.base, x as i16, y as i16, px, v, t.as_bytes())
    }

    /// Text centred across the screen on `baseline`, by the keyboard's estimate of the typeface's advance (0.56 em).
    fn centred(&self, baseline: usize, px: usize, value: u16, t: &str) {
        let x = self.w as i64 / 2 - t.chars().count() as i64 * px as i64 * 28 / 100;
        window::text(self.display, self.base, x as i16, baseline as i16, px as u16, value, t.as_bytes());
    }

    fn clear_rows(&self, y0: usize, y1: usize) {
        self.surf()[y0 * self.w..y1 * self.w].fill(0);
    }

    /// Where things go, from the screen's size: the mark's centre and radius, the title's and status line's baselines and sizes, the entry's baseline and size.
    fn mark_at(&self) -> (usize, usize, usize) {
        (self.w / 2, self.h / 5, self.w / 8)
    }
    fn title_at(&self) -> (usize, usize) {
        let (_, cy, r) = self.mark_at();
        (cy + r + self.w / 6, self.w / 14)
    }
    fn status_at(&self) -> (usize, usize) {
        let (t, ts) = self.title_at();
        (t + ts + self.w / 24, self.w / 34)
    }
    fn entry_at(&self) -> (usize, usize) {
        (self.kb.top - self.w / 8, self.w / 16)
    }

    fn status(&self, t: &str) {
        let (b, px) = self.status_at();
        self.clear_rows(b - px * 3 / 2, b + px / 2);
        self.centred(b, px, QUIET, t);
    }

    /// The entry: the handle as typed, a cursor, and a line under it.
    fn entry(&self) {
        let (b, px) = self.entry_at();
        self.clear_rows(b - px * 3 / 2, b + px / 2);
        let mut t = [0u8; 4 * MAX_CHARS + 4];
        let n = self.len;
        t[..n].copy_from_slice(&self.handle[..n]);
        t[n] = b'_';
        let shown = core::str::from_utf8(&t[..n + 1]).unwrap_or("_");
        self.centred(b, px, TEXT, shown);
        // The line: three quarters of the width, a 448th of it thick.
        let (x0, x1, y) = (self.w / 8, self.w * 7 / 8, b + px / 4);
        for row in y..y + (self.w / 448).max(1) {
            self.surf()[row * self.w + x0..row * self.w + x1].fill(grey(QUIET as u64));
        }
    }

    fn all(&self) {
        self.surf().fill(0);
        let (cx, cy, r) = self.mark_at();
        mark(self.surf(), self.w, cx, cy, r);
        let (tb, tpx) = self.title_at();
        self.centred(tb, tpx, TEXT, "attest");
        self.status(self.prompt());
        self.entry();
        self.kb.draw(self.surf(), &mut self.label());
    }

    fn prompt(&self) -> &'static str {
        match self.mode {
            Mode::Known => "This device knows one identity. Type its handle.",
            Mode::Fresh => "No identity on this device yet. Type your handle.",
            Mode::Confirm(_) => "Type it again: it becomes this device's identity.",
        }
    }

    fn show(&self) {
        window::show(self.display, self.base, 0, 0, self.w as u16, self.h as u16, window::SYSTEM);
    }

    /// The typed handle gone from memory.
    fn wipe(&mut self) {
        for b in self.handle.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        self.len = 0;
        self.chars = 0;
    }

    fn put(&self, scope: u64, key: &[u8], value: &[u8]) -> bool {
        ferros_sys::store_set(cap(self.grants, scope), key, value)
    }

    /// Enter: the handle against what the device knows, or the first identity's two entries.
    fn attest(&mut self) {
        if self.chars == 0 {
            return;
        }
        let seed = match core::str::from_utf8(&self.handle[..self.len]).ok().map(ahau_core::identity_seed) {
            Some(Ok(s)) => s,
            _ => {
                self.status("That handle has a character this keyboard cannot type.");
                return;
            }
        };
        let v = ahau_core::verifier(&self.device_key, &seed);
        match self.mode {
            Mode::Known => {
                let ok = match (self.verifier, self.photon_seed) {
                    (Some(stored), _) => ahau_core::same(&v, &stored),
                    (None, Some(ps)) => ahau_core::same(&seed, &ps),
                    (None, None) => false,
                };
                if !ok {
                    console(self.grants, b"ahau: a handle that is not this device's\n");
                    self.status("That is not this device's identity.");
                    return;
                }
                if self.verifier.is_none() && !self.put(S_AHAU, b"verifier", &v) {
                    console(self.grants, b"ahau: could not keep the verifier\n");
                }
                self.attested();
            }
            Mode::Fresh => {
                self.mode = Mode::Confirm(seed);
                self.wipe();
                self.status(self.prompt());
            }
            Mode::Confirm(first) => {
                if !ahau_core::same(&first, &seed) {
                    // The second stays on screen; corrected and entered, it is the first of a new pair.
                    self.mode = Mode::Fresh;
                    self.status("The two did not match. Fix it and enter, then type it again.");
                    return;
                }
                self.status("Deriving the handle's proof: about seven seconds.");
                self.entry();
                self.show();
                // Let the display put the status up before the proof holds this core.
                sleep_until(now() + freq() / 30);
                // On the X3: 6.5 s there, 22.6 s on a worker (measured 2026-10-27).
                ferros_sys::pin(ferros_sys::CORES_BIG);
                let t0 = now();
                let proof = ihi::handle_proof(&blake3::Hash::from(seed));
                let ms = ferros_sys::us_since(t0) / 1000;
                ferros_sys::pin(ferros_sys::CORES_WORKERS);
                let mut l = Line::new();
                let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: handle proof in {ms} ms\n"));
                console(self.grants, l.as_bytes());
                let kept = self.put(S_PHOTON, b"identity_seed", &seed) && self.put(S_PHOTON, b"handle_proof", proof.as_bytes()) && self.put(S_AHAU, b"verifier", &v);
                if !kept {
                    console(self.grants, b"ahau: could not keep the identity\n");
                    self.mode = Mode::Fresh;
                    self.status("The vault refused the identity. Type your handle.");
                    return;
                }
                console(self.grants, b"ahau: a new identity on this device\n");
                self.attested();
            }
        }
    }

    /// Back to the first entry of a new identity (clear during the second).
    fn unconfirm(&mut self) {
        if let Mode::Confirm(_) = self.mode {
            self.mode = Mode::Fresh;
            self.status(self.prompt());
        }
    }

    fn attested(&mut self) -> ! {
        self.wipe();
        console(self.grants, b"ahau: attested\n");
        window::done(self.display, self.base);
        exit(0)
    }

    fn tap(&mut self, key: Key) {
        match self.kb.tap(key) {
            Some(Event::Char(c)) => {
                let mut b = [0u8; 4];
                let e = c.encode_utf8(&mut b).as_bytes();
                if self.chars < MAX_CHARS {
                    self.handle[self.len..self.len + e.len()].copy_from_slice(e);
                    self.len += e.len();
                    self.chars += 1;
                }
                self.entry();
            }
            Some(Event::Back) => {
                while self.len > 0 {
                    self.len -= 1;
                    let lead = self.handle[self.len] & 0xC0 != 0x80;
                    self.handle[self.len] = 0;
                    if lead {
                        self.chars -= 1;
                        break;
                    }
                }
                self.entry();
            }
            Some(Event::Enter) => {
                self.attest();
                self.entry();
            }
            Some(Event::Leave) => {
                self.wipe();
                self.unconfirm();
                self.entry();
            }
            Some(Event::Redraw) => self.kb.draw(self.surf(), &mut self.label()),
            None => {}
        }
    }
}

fn grey(v: u64) -> u64 {
    v | v << 16 | v << 32
}

/// The ferros mark (the owner's icon idea for ahau): the iron ring, broken at the top, closed around a single centre point. Coverage from the squared distance, so only the one-pixel band at each edge takes a root (integer, Q8); the break is a vertical slot whose sides cover by fraction.
fn mark(s: &mut [u64], w: usize, cx: usize, cy: usize, r: usize) {
    let (ro, ri, rd) = ((r as i64) << 8, (r as i64 * 3 / 4) << 8, (r as i64 / 5) << 8);
    let half_gap = (r as i64 / 6) << 8;
    // Coverage of a disc of radius `rad` (Q8) at squared distance `d2` (Q16), 0 to 256.
    let disc = |rad: i64, d2: i64| -> i64 {
        let (lo, hi) = (rad - 128, rad + 128);
        if d2 <= lo * lo {
            256
        } else if d2 >= hi * hi {
            0
        } else {
            hi - (d2 as u64).isqrt() as i64
        }
    };
    let (ccx, ccy) = ((cx as i64) << 8, (cy as i64) << 8);
    for y in cy - r - 1..cy + r + 2 {
        for x in cx - r - 1..cx + r + 2 {
            let (dx, dy) = (((x as i64) << 8) + 128 - ccx, ((y as i64) << 8) + 128 - ccy);
            let d2 = dx * dx + dy * dy;
            let mut ring = disc(ro, d2) - disc(ri, d2);
            if dy < 0 && ring > 0 {
                // The break: how much of this pixel's width lies outside the slot.
                let (px0, px1) = (((x as i64) << 8) - ccx, ((x as i64 + 1) << 8) - ccx);
                let inside = (px1.min(half_gap) - px0.max(-half_gap)).max(0);
                ring = ring * (256 - inside) >> 8;
            }
            let cov = ring + disc(rd, d2);
            if cov > 0 {
                s[y * w + x] = grey(MARK * ((cov as u64) << 8).isqrt() >> 8);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ahau_main(grants: u64, _input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    // Spawned with any input: the handle proof's time on this phone over a fixed seed, on the core it lands on and on the X3, then exit (a development measurement; nothing is read or kept).
    if input_len > 0 && unsafe { *((ferros_sys::arc_base() + _input) as *const u8) } != b'c' {
        for (name, mask) in [("a worker", ferros_sys::CORES_WORKERS), ("the X3", ferros_sys::CORES_BIG)] {
            ferros_sys::pin(mask);
            sleep_until(now() + freq() / 1000);
            let t0 = now();
            let p = ihi::handle_proof(&blake3::Hash::from([7u8; 32]));
            let mut l = Line::new();
            let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: handle proof on {name} in {} ms (first byte {})\n", ferros_sys::us_since(t0) / 1000, p.as_bytes()[0]));
            console(grants, l.as_bytes());
        }
        exit(0);
    }
    let (display, touch) = (cap(grants, S_DISPLAY), cap(grants, S_TOUCH));
    let mut r = [0u8; 256];
    if call(display, &[5], &mut r) < 4 {
        console(grants, b"ahau: no display server\n");
        exit(1);
    }
    let (w, h) = (u16::from_le_bytes([r[0], r[1]]) as usize, u16::from_le_bytes([r[2], r[3]]) as usize);
    let base = ferros_sys::dma(cap(grants, S_SURF));
    if base == 0 || base == u64::MAX || w * h * 8 > 32 << 20 {
        console(grants, b"ahau: no surface\n");
        exit(2);
    }
    let Some(device_key) = ferros_sys::ira_key(cap(grants, S_IRA), "ahau verifier") else {
        console(grants, b"ahau: no ira this boot: nothing to keep an identity under\n");
        exit(3);
    };
    let read32 = |scope: u64, key: &[u8]| -> Option<[u8; 32]> {
        let mut b = [0u8; 32];
        (ferros_sys::store_get(cap(grants, scope), key, &mut b) == Some(32)).then_some(b)
    };
    let verifier = read32(S_AHAU, b"verifier");
    let photon_seed = read32(S_PHOTON, b"identity_seed");
    // Spawned with input "c": does the kept verifier belong to the paired seed under this device's key? Yes or no and nothing else (a development check; no handle involved).
    if input_len == 1 && unsafe { *((ferros_sys::arc_base() + _input) as *const u8) } == b'c' {
        let line: &[u8] = match (verifier, photon_seed) {
            (Some(v), Some(ps)) if ahau_core::same(&v, &ahau_core::verifier(&device_key, &ps)) => b"ahau check: the verifier belongs to the paired seed\n",
            (Some(_), Some(_)) => b"ahau check: the verifier does NOT belong to the paired seed\n",
            (None, Some(_)) => b"ahau check: no verifier yet; the paired seed is there\n",
            (Some(_), None) => b"ahau check: a verifier but no paired seed\n",
            (None, None) => b"ahau check: neither\n",
        };
        console(grants, line);
        exit(0);
    }
    let mode = if verifier.is_some() || photon_seed.is_some() { Mode::Known } else { Mode::Fresh };
    let mut kb = Keyboard::new(w, h);
    kb.left = "clear";
    let mut a = Ahau { grants, display, base, w, h, kb, mode, verifier, photon_seed, device_key, handle: [0; 4 * MAX_CHARS], len: 0, chars: 0 };
    a.all();
    a.show();
    console(grants, if mode == Mode::Known { b"ahau: waiting for this device's identity\n" } else { b"ahau: no identity on this device yet\n" });
    let mut touches = Touches::new(w);
    // Every key exact: a rolling finger still types where it landed.
    touches.trace_words = false;
    let mut held = [(0u8, 0i64, 0i64); 16];
    loop {
        let n = call(touch, &[1], &mut r) as usize;
        let count = if n >= 1 && n != ferros_sys::ERR as usize { (r[0] as usize).min(16) } else { 0 };
        let mut k = 0;
        while k < count && 1 + (k + 1) * 12 <= n {
            let rec = &r[1 + k * 12..1 + (k + 1) * 12];
            held[k] = (rec[0], u16::from_le_bytes([rec[4], rec[5]]) as i64, u16::from_le_bytes([rec[6], rec[7]]) as i64);
            k += 1;
        }
        let mut outs = [Touch::Scrolled; 48];
        let mut no = 0;
        touches.poll(&mut a.kb, &held[..k], &mut |x, y| window::mine(display, base, x as u16, y as u16), &mut |t| {
            outs[no] = t;
            no += 1;
        });
        for &t in &outs[..no] {
            match t {
                Touch::Light(key) => a.kb.draw_key(a.surf(), key, true, &mut a.label()),
                Touch::Unlight(key) => a.kb.draw_key(a.surf(), key, false, &mut a.label()),
                Touch::Scrolled => a.kb.draw(a.surf(), &mut a.label()),
                Touch::Tap(key) => a.tap(key),
                // A handle is exact bytes, typed key by key: no shape writing here.
                Touch::Trail(..) | Touch::Traced | Touch::TapAt(..) => {}
            }
        }
        if no > 0 {
            a.show();
        }
        sleep_until(now() + freq() / 240);
    }
}

/// A console line in a fixed buffer.
struct Line {
    b: [u8; 96],
    n: usize,
}

impl Line {
    fn new() -> Line {
        Line { b: [0; 96], n: 0 }
    }
    fn as_bytes(&self) -> &[u8] {
        &self.b[..self.n]
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
