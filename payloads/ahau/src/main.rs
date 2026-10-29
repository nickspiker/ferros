//! ahau: the attest screen (Claude memories/ferros_attest_os_level.md; the capsule the owner named, Māori "I, me"). The first thing on the glass at boot: init starts it before the map, at the system level, so it covers everything until the person proves who they are by typing their handle on the honeycomb keyboard. Then its window goes and the map is there.
//!
//! The derivation is ihi's, computed here without std (ahau_core): the identity seed is BLAKE3 over vsf's `x` encoding of the handle, byte-exact for every character the keyboard can type. Nothing of the identity is written to disk: the seed and proof are handed to the Photon client in RAM (op 4), and the device is recognised as its identity's only once the client folds the public chain and finds this device a member (owner, 2026-10-28: "sand on the beach with no keys anywhere and it's all derived from the handle and the ira"). The one exception is opt-in: `device.unlock_at_boot` stores the seed so the phone unlocks without the handle being typed — a stolen phone then holds the identity. It is not switched here (owner, 2026-10-29: the attest screen carries no toggle); wherever it is switched on must say so.
//!
//! - Default: the handle is typed each boot. Its proof (ihi's memory-hard proof, 24.9 MB of scratch, 17 rounds) is derived, the seed and proof go to the Photon client in RAM, and the client looks the identity up. A member unlocks; a fleet to join shows the words; a free handle is minted.
//! - Opt-in (unlock at boot): the seed is stored, the client loads it from the vault at boot, and the screen unlocks as soon as the client reports membership, with no handle typed.
//!
//! The handle shows as it is typed: it is a name the person proves, not a password (owner, 2026-10-27: "this is NOT a password"). A wrong one stays on screen to be corrected; the bar's left key clears it. Minting a new identity asks for the handle a second time, shown as typed like the first, the first cleared so a typo is not simply copied (owner, 2026-10-27: "you only do that once"); clear during the second goes back to the first. The buffer is wiped once attest succeeds.
//!
//! The scratch is this process's own .bss (25 MiB): the entry's header (`ferrosRT`, ferros_rt's) tells the kernel how much zeroed memory follows the code, so the arc holds it (payload.ld).
//!
//! The keyboard is the device's layout (`device.layout`, ferros_sys::PREF_LAYOUT: Dvorak unless set), which the last identity to attest leaves on the device (SETTINGS.md). There is no switch on this screen (owner, 2026-10-29: the room went to the mark); the layout is set from the identity's settings or `diag pref`, and the keyboard follows it.
//!
//! A first entry is not a claim (2026-10-28, after it let a person in under a handle that already had a fleet): the handle's roots go into the vault and the Photon client looks the identity up, its fleet's devices nearby first and fgtw.org last (photon docs/pairing-v2.md, peers-are-fgtw.md). A fleet: this device joins it, its pairing words shown for one of the fleet's devices to add it. No fleet anywhere: the permanence warning, the handle again, and the client mints the genesis. Nothing answering: nothing is decided, and the person may go on while the client keeps looking. The screen does not hold the device hostage meanwhile (owner: "Doesn't have to stay locked").
//!
//! Grant slots: 0 console, 1 display (use), 2 touch (use), 3 its surface, 4 the vault scope `ahau.`, 5 the vault scope `photon.`, 6 the ira key, 7 settings, 8 the Photon client (use).
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
const S_PHOTON: u64 = 5;
const S_SETTINGS: u64 = 7;
/// The ira key: per-purpose keys derived from the device's ira (here the attest mark's salt).
const S_IRA: u64 = 6;
/// The attest mark's pixels as tapuwae renders them (α + darkness, 8 bits a channel), drawn once by the mark's own thread and blitted at every redraw after; its side is FRESH_SIDE.
/// 2²⁰ pixels holds a mark of radius up to 510 (side 2r + 4): w/3 on panels up to 1530 wide.
static mut FRESH: [u32; 1 << 20] = [0; 1 << 20];
static mut FRESH_SIDE: usize = 0;
/// Where the mark stands: PENDING while its thread renders (the screen shows nothing there yet), READY once FRESH holds it, RING when there is none (no ira key, a panel too wide) and the drawn ring stands in.
static FRESH_STATE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(PENDING);
const PENDING: u64 = 0;
const READY: u64 = 1;
const RING: u64 = 2;
/// The mark thread's work: the grants (for its console line), the radius and the salt, set before it starts; and its stack.
static mut FRESH_JOB: (u64, usize, [u8; 32]) = (0, 0, [0; 32]);
static mut FRESH_STACK: [u8; 1 << 16] = [0; 1 << 16];
/// Which pixels of FRESH hold any of a component (tapuwae::render_fresh's mask): what a new handle redraws.
static mut FRESH_MASK: [u64; (1 << 20) / 64] = [0; (1 << 20) / 64];
/// The mark's map (tapuwae::FreshCell, 6 bytes a sample): where each sample of the masked pixels falls in the set, worked out once, so a new handle costs the balls' shading alone. 3·2²⁰ cells (18 MiB) hold the 900 px mark's 171 643 masked pixels at 4 × 4 (2.75 M) with room; past it tapuwae works the rest out again.
static mut FRESH_MAP: [tapuwae::FreshCell; 3 << 20] = [[0; 3]; 3 << 20];
/// The handle as published for the mark (bytes, length), under FRESH_LOCK; FRESH_GEN counts publications, FRESH_DONE is one past the generation of the last finished frame (0 = none yet).
static mut FRESH_TEXT: ([u8; 4 * MAX_CHARS], usize) = ([0; 4 * MAX_CHARS], 0);
static FRESH_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static FRESH_GEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static FRESH_DONE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The Photon client's endpoint (payloads/photon): op 1 its state and, while joining, its pairing words; op 2 mint.
const S_PHOTON_EP: u64 = 8;
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
    /// Unlock at boot: the client loaded the stored seed; waiting for it to report membership (then unlocked, no handle typed).
    Starting,
    /// No identity in hand: the entry.
    Fresh,
    /// The roots of this seed are in the vault; the Photon client is looking the identity up.
    Looking([u8; 32]),
    /// The identity has a fleet: one of its devices adds this one with the words shown; enter goes on.
    Join([u8; 32]),
    /// No fleet holds the handle: the permanence warning, and the handle again to mint.
    Mint([u8; 32]),
    /// No fleet device and no fgtw.org answer: nothing decided, the client keeps looking; enter goes on.
    Grey([u8; 32]),
    /// fgtw.org's chain for the handle is another identity's: enter starts over.
    Wrong,
}

struct Ahau {
    grants: u64,
    display: u64,
    base: u64,
    w: usize,
    h: usize,
    kb: Keyboard,
    mode: Mode,
    /// This session's last derived seed and proof (RAM only), so typing the same handle again (after `back`) skips the proof.
    session_seed: Option<[u8; 32]>,
    session_proof: Option<[u8; 32]>,
    /// Unlock at boot (device.unlock_at_boot): the seed is stored so the phone unlocks without typing. Opt-in, warned.
    unlock_at_boot: bool,
    handle: [u8; 4 * MAX_CHARS],
    len: usize,
    chars: usize,
    /// When the screen came up (the Starting wait's 10 s).
    started: u64,
    /// The Photon client's pairing words while this device joins its identity's fleet.
    word_buf: [u8; 512],
    word_len: usize,
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
    /// The mark: two thirds of the width across (owner, 2026-10-29), its top just under the front camera. The camera's hole is centred 75 px down a 1344-wide panel with a 43.25 px radius (husky's display cutout, Android's device overlay config_mainBuiltInDisplayCutout), so its lower edge is 118.25/1344 = 473/5376 of the width down; the mark starts 2⁻⁶ of the width below that.
    fn mark_at(&self) -> (usize, usize, usize) {
        let r = self.w / 3;
        let top = self.w * 473 / 5376 + self.w / 64;
        (self.w / 2, top + r, r)
    }
    fn status_at(&self) -> (usize, usize) {
        let (_, cy, r) = self.mark_at();
        (cy + r + self.w / 16, self.w / 34)
    }


    /// A tap on no key: the button under the entry line acts (or `back` beside it).
    fn tap_at(&mut self, x: i64, y: i64) {
        if self.can_go_back() {
            let (bx0, by0, bx1, by1) = self.back_at();
            if (bx0 as i64..bx1 as i64).contains(&x) && (by0 as i64..by1 as i64).contains(&y) {
                self.go_back();
                return;
            }
        }
        let (x0, y0, x1, y1) = self.button_at();
        if (x0 as i64..x1 as i64).contains(&x) && (y0 as i64..y1 as i64).contains(&y) {
            if self.button_label().is_some() {
                self.attest();
                self.entry();
                self.button();
                self.words();
            }
            return;
        }
    }

    /// The device's layout changed (from anywhere: the identity's settings, `diag pref`): the keyboard follows.
    fn follow_layout(&mut self) -> bool {
        let l = ferros_sys::pref(ferros_sys::PREF_LAYOUT) as usize;
        if l == self.kb.layout || l >= honeycomb::LAYOUTS {
            return false;
        }
        self.kb.layout = l;
        self.kb.draw(self.surf(), &mut self.label());
        true
    }

    /// Whether a step back to the entry exists: from any step after a first entry (owner, 2026-10-28: "we need a way to go back to the previous screen").
    fn can_go_back(&self) -> bool {
        matches!(self.mode, Mode::Looking(_) | Mode::Join(_) | Mode::Mint(_) | Mode::Grey(_) | Mode::Wrong)
    }

    /// The button under the entry line: its rectangle (x0, y0, x1, y1). With a step back, it moves right and `back` takes the left.
    fn button_at(&self) -> (usize, usize, usize, usize) {
        let (b, px) = self.entry_at();
        let y0 = b + px;
        if self.can_go_back() {
            (self.w * 9 / 16, y0, self.w * 7 / 8, y0 + self.w / 10)
        } else {
            (self.w * 5 / 16, y0, self.w * 11 / 16, y0 + self.w / 10)
        }
    }

    /// The `back` button, left of the main one.
    fn back_at(&self) -> (usize, usize, usize, usize) {
        let (_, y0, _, y1) = self.button_at();
        (self.w / 8, y0, self.w * 7 / 16, y1)
    }

    /// Back to the entry: the pending roots stay (the client keeps at them); the same handle resumes, another replaces them.
    fn go_back(&mut self) {
        self.mode = Mode::Fresh;
        self.status(self.prompt());
        self.words();
        self.entry();
        self.button();
    }

    /// What the button does now, or nothing while the client is asked.
    fn button_label(&self) -> Option<&'static str> {
        match self.mode {
            Mode::Fresh => Some("attest"),
            Mode::Join(_) | Mode::Grey(_) => Some("go on"),
            Mode::Mint(_) => Some("mint"),
            Mode::Wrong => Some("start over"),
            Mode::Starting | Mode::Looking(_) => None,
        }
    }

    fn button(&self) {
        // The whole row first, so a button that moved or went leaves nothing behind.
        let (_, ry0, _, ry1) = self.button_at();
        let s = self.surf();
        for y in ry0..ry1 {
            s[y * self.w..(y + 1) * self.w].fill(0);
        }
        if self.can_go_back() {
            let (bx0, by0, bx1, by1) = self.back_at();
            for y in by0..by1 {
                s[y * self.w + bx0..y * self.w + bx1].fill(grey(1800));
            }
            let px = self.w / 24;
            let x = (bx0 + bx1) as i64 / 2 - 4 * px as i64 * 28 / 100;
            window::text(self.display, self.base, x as i16, ((by0 + by1) / 2 + px * 35 / 100) as i16, px as u16, TEXT, b"back");
        }
        let (x0, y0, x1, y1) = self.button_at();
        for y in y0..y1 {
            s[y * self.w + x0..y * self.w + x1].fill(if self.button_label().is_some() { grey(2600) } else { 0 });
        }
        if let Some(t) = self.button_label() {
            let px = self.w / 24;
            let x = (x0 + x1) as i64 / 2 - t.len() as i64 * px as i64 * 28 / 100;
            window::text(self.display, self.base, x as i16, ((y0 + y1) / 2 + px * 35 / 100) as i16, px as u16, TEXT, t.as_bytes());
        }
    }

    fn entry_at(&self) -> (usize, usize) {
        (self.kb.top - self.w * 5 / 16, self.w / 16)
    }

    /// The status: a line, or two split at a newline.
    fn status(&self, t: &str) {
        let (b, px) = self.status_at();
        self.clear_rows(b - px * 3 / 2, b + px * 2);
        for (k, line) in t.split('\n').enumerate() {
            self.centred(b + k * px * 13 / 10, px, QUIET, line);
        }
    }

    /// The pairing words while joining, six to a row under the status line, split at the capitals they come camelCased with; cleared otherwise.
    fn words(&self) {
        let px = self.w / 30;
        let top = self.status_at().0 + self.w / 8;
        self.clear_rows(top, top + px * 9);
        if !matches!(self.mode, Mode::Join(_)) {
            return;
        }
        let w = &self.word_buf[..self.word_len];
        let mut row = [0u8; 160];
        let (mut n, mut count, mut line) = (0usize, 0usize, 0usize);
        let flush = |row: &[u8], line: usize| {
            if let Ok(t) = core::str::from_utf8(row) {
                self.centred(top + px * 3 / 2 + line * px * 3 / 2, px, TEXT, t);
            }
        };
        for (i, &c) in w.iter().enumerate() {
            if c.is_ascii_uppercase() && i > 0 {
                count += 1;
                if count % 6 == 0 {
                    flush(&row[..n], line);
                    line += 1;
                    n = 0;
                } else if n < row.len() {
                    row[n] = b' ';
                    n += 1;
                }
            }
            if n < row.len() {
                row[n] = c;
                n += 1;
            }
        }
        flush(&row[..n], line);
    }

    /// The Photon client's state (op 1): its state byte, with the pairing words kept while it joins.
    fn client_state(&mut self) -> Option<u8> {
        let mut out = [0u8; 600];
        let n = call(cap(self.grants, S_PHOTON_EP), &[1], &mut out);
        if n == ferros_sys::ERR || n == ferros_sys::NOTHING || n < 2 {
            return None;
        }
        let n = (n as usize).min(out.len());
        let k = (n - 2).min(self.word_buf.len());
        self.word_buf[..k].copy_from_slice(&out[2..2 + k]);
        self.word_len = k;
        Some(out[0])
    }

    /// Unlock at boot: the client loaded the stored seed. A member (state 2 or 3) unlocks with no handle typed; anything after 15 s means the stored seed is not a member (removed from its fleet, say), so fall to the entry.
    fn follow_start(&mut self) -> bool {
        match self.client_state() {
            Some(2) | Some(3) => {
                console(self.grants, b"ahau: unlocked at boot (this device is a member)\n");
                self.attested();
            }
            _ if now().wrapping_sub(self.started) > 15 * freq() => {
                console(self.grants, b"ahau: the stored seed is not a member; asking for the handle\n");
                self.mode = Mode::Fresh;
                self.status(self.prompt());
                self.button();
                true
            }
            _ => false,
        }
    }

    /// While the identity is being looked up or joined: follow the client. Returns whether the screen changed.
    fn follow_client(&mut self) -> bool {
        if self.mode == Mode::Starting {
            return self.follow_start();
        }
        let seed = match self.mode {
            Mode::Looking(s) | Mode::Join(s) | Mode::Grey(s) => s,
            _ => return false,
        };
        let Some(state) = self.client_state() else { return false };
        let next = match state {
            1 => Mode::Join(seed),
            2 | 3 => {
                console(self.grants, b"ahau: this device is a member of the identity's fleet\n");
                self.attested();
            }
            5 => Mode::Mint(seed),
            6 => Mode::Wrong,
            7 => Mode::Grey(seed),
            _ => return false,
        };
        if next == self.mode && !matches!(next, Mode::Join(_)) {
            return false;
        }
        let changed = next != self.mode;
        self.mode = next;
        if changed {
            self.status(self.prompt());
            self.button();
        }
        self.words();
        true
    }

    /// Hand the roots to the Photon client in RAM (op 4): it takes them, keeps them on disk only if unlock-at-boot is on, and looks the identity up. Nothing is written here.
    fn send_roots(&mut self, seed: &[u8; 32], proof: &[u8; 32]) {
        let mut m = [0u8; 65];
        m[0] = 4;
        m[1..33].copy_from_slice(seed);
        m[33..65].copy_from_slice(proof);
        let mut out = [0u8; 16];
        call(cap(self.grants, S_PHOTON_EP), &m, &mut out);
        self.session_seed = Some(*seed);
        self.session_proof = Some(*proof);
    }

    /// The entry: the handle as typed, a cursor, and a line under it.
    fn entry(&self) {
        publish_fresh_text(&self.handle[..self.len]);
        let (b, px) = self.entry_at();
        self.clear_rows(b - px * 3 / 2, b + px / 2);
        // A newline shows as ¶ (the typeface has no return arrow): the handle stays on one line as typed.
        let mut t = [0u8; 4 * MAX_CHARS + 4];
        let mut n = 0;
        for &c in &self.handle[..self.len] {
            if c == b'\n' {
                t[n..n + 2].copy_from_slice("¶".as_bytes());
                n += 2;
            } else {
                t[n] = c;
                n += 1;
            }
        }
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
        draw_mark(self.surf(), self.w, cx, cy, r);
        self.status(self.prompt());
        self.entry();
        self.button();
        self.kb.draw(self.surf(), &mut self.label());
    }

    fn prompt(&self) -> &'static str {
        match self.mode {
            Mode::Starting => "Unlocking: asking this device's Photon client to confirm membership.",
            Mode::Fresh => "No identity on this device yet. Type your handle.",
            Mode::Looking(_) => "Looking this identity up: its devices nearby, then fgtw.org.",
            Mode::Join(_) => "This identity has a fleet. On one of its devices: Fleet, Add device,\nand type the words below. Go on meanwhile if you like.",
            Mode::Mint(_) => "No fleet holds this handle. Minting makes it yours for good:\nno reset, no recovery. Type it again, then mint.",
            Mode::Grey(_) => "No device of this identity and no fgtw.org answers: nothing decided.\nStill looking. Go on meanwhile if you like.",
            Mode::Wrong => "fgtw.org's chain for this handle claims another slot.\nStart over to type another.",
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
        publish_fresh_text(&[]);
    }

    fn put(&self, scope: u64, key: &[u8], value: &[u8]) -> bool {
        ferros_sys::store_set(cap(self.grants, scope), key, value)
    }

    /// The attest button (or a mint's second enter): a first entry derives the proof and hands the roots to the client; a join or grey goes on; a mint confirms; wrong starts over.
    fn attest(&mut self) {
        match self.mode {
            Mode::Join(_) | Mode::Grey(_) => {
                // Going on: the client keeps joining (or looking). Nothing on disk.
                self.attested();
            }
            Mode::Wrong => {
                self.mode = Mode::Fresh;
                self.status(self.prompt());
                self.words();
                return;
            }
            Mode::Looking(_) | Mode::Starting => return,
            _ => {}
        }
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
        match self.mode {
            Mode::Fresh => {
                // The same handle again this session (after `back`): its proof is cached in RAM, no second derivation.
                let proof = if self.session_seed == Some(seed) {
                    self.session_proof.unwrap()
                } else {
                    self.status("Deriving the handle's proof: about five seconds.");
                    self.entry();
                    self.show();
                    // Let the display put the status up before the proof holds this core.
                    sleep_until(now() + freq() / 30);
                    // On the X3 (where the kernel runs): about five seconds (measured 2026-10-27).
                    let t0 = now();
                    let proof = *ihi::handle_proof(&blake3::Hash::from(seed)).as_bytes();
                    let ms = ferros_sys::us_since(t0) / 1000;
                    let mut l = Line::new();
                    let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: handle proof in {ms} ms\n"));
                    console(self.grants, l.as_bytes());
                    proof
                };
                // The roots to the client in RAM; it keeps them on disk only if unlock-at-boot is on.
                self.send_roots(&seed, &proof);
                console(self.grants, b"ahau: roots handed to the Photon client; it looks the identity up\n");
                self.wipe();
                self.mode = Mode::Looking(seed);
                self.status(self.prompt());
            }
            Mode::Mint(first) => {
                if !ahau_core::same(&first, &seed) {
                    self.status("The two did not match. Fix it and enter.");
                    return;
                }
                self.status("Minting: the genesis goes to fgtw.org.");
                self.entry();
                self.show();
                let mut out = [0u8; 600];
                let n = call(cap(self.grants, S_PHOTON_EP), &[2], &mut out);
                let r = if n == ferros_sys::ERR || n == 0 { 255 } else { out[0] };
                match r {
                    0 => {
                        console(self.grants, b"ahau: minted; this device is the identity's first member\n");
                        self.attested();
                    }
                    4 => {
                        // Another device's genesis won the handle first: it has a fleet now.
                        self.mode = Mode::Join(seed);
                        self.status(self.prompt());
                        let _ = self.client_state();
                        self.words();
                    }
                    _ => {
                        let mut l = Line::new();
                        let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: mint refused, client says {r}\n"));
                        console(self.grants, l.as_bytes());
                        self.status("fgtw.org did not take the genesis.\nEnter tries again.");
                    }
                }
            }
            _ => {}
        }
    }

    /// Clear during the mint's second entry only clears it: the verdict stands.
    fn unconfirm(&mut self) {}

    fn attested(&mut self) -> ! {
        self.wipe();
        console(self.grants, b"ahau: attested\n");
        window::done(self.display, self.base);
        exit(0)
    }

    fn type_char(&mut self, c: char) {
        let mut b = [0u8; 4];
        let e = c.encode_utf8(&mut b).as_bytes();
        if self.chars < MAX_CHARS {
            self.handle[self.len..self.len + e.len()].copy_from_slice(e);
            self.len += e.len();
            self.chars += 1;
        }
        self.entry();
    }

    fn tap(&mut self, key: Key) {
        match self.kb.tap(key) {
            Some(Event::Char(c)) => self.type_char(c),
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
            // Enter is a character of the handle (owner, 2026-10-28: "enter is a valid char"); the button under the entry line attests.
            Some(Event::Enter) => self.type_char('\n'),
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

/// The attest mark for a device no identity has attested on yet (tapuwae::render_fresh): the Mandelbrot set in the device's own colour, from a key the ira derives for this purpose alone ("attest mark"), so it is this phone's and no other's, with the device's attest ball inside every component, following the handle as it is typed.
/// Rendered on a thread of its own pinned to the X3 (the one big core), so the screen and the keyboard are never held: the whole mark once, then on every change to the handle only the components again (the outline never changes); the main loop blits each finished frame (FRESH_DONE).
/// Without the ira grant, or on a panel too wide for the buffer, the state is RING and the drawn ring stands in.
fn start_fresh_mark(grants: u64, r: usize) {
    let side = 2 * r + 4;
    // WHY: r comes from the display server's panel width; a wider panel than this buffer was sized for falls back to the drawn ring instead of rendering past FRESH's end.
    if side * side > unsafe { (&*(&raw const FRESH)).len() } {
        console(grants, b"ahau: the panel is too wide for the mark's buffer; the ring stands in\n");
        FRESH_STATE.store(RING, core::sync::atomic::Ordering::Release);
        return;
    }
    let Some(salt) = ferros_sys::ira_key(cap(grants, S_IRA), "attest mark") else {
        console(grants, b"ahau: no ira key; the ring stands in\n");
        FRESH_STATE.store(RING, core::sync::atomic::Ordering::Release);
        return;
    };
    unsafe { FRESH_JOB = (grants, r, salt) };
    let stack = unsafe { &mut *(&raw mut FRESH_STACK) };
    if ferros_sys::thread_spawn(fresh_thread, stack, 0) == ferros_sys::ERR {
        // No thread: the whole mark here, once, before the screen comes up; it then does not follow the handle.
        console(grants, b"ahau: no thread for the mark; rendering it once, first\n");
        render_fresh_mark(true, false, 0);
        FRESH_DONE.store(1, core::sync::atomic::Ordering::Release);
    }
}

/// The mark's thread: the whole mark, then each new handle's components, waiting on FRESH_GEN between. Each time two passes (owner, 2026-10-29): a coarse one, a sample a pixel, on the glass at once; then a fine one, 4 × 4 a pixel, a moment later, which a newer handle stops at the next row. FRESH_DONE counts the frames finished, for the main loop to put up.
extern "C" fn fresh_thread(_: u64) -> ! {
    ferros_sys::pin(ferros_sys::CORES_BIG);
    sleep_until(now() + freq() / 1000);
    let mut full = true;
    let mut frames = 0u64;
    loop {
        let g = FRESH_GEN.load(core::sync::atomic::Ordering::Acquire);
        for fine in [false, true] {
            if render_fresh_mark(full, fine, g) {
                frames += 1;
                FRESH_DONE.store(frames, core::sync::atomic::Ordering::Release);
            }
            full = false;
        }
        while FRESH_GEN.load(core::sync::atomic::Ordering::Acquire) == g {
            ferros_sys::wait(&FRESH_GEN, g, 0);
        }
    }
}

/// One pass for the handle as published at generation `g` (the whole mark when `full`, else the components; coarse or `fine`), with its time on the console (never the handle); then READY. A fine pass stops when the handle has moved on, and returns false. The thread's copy of the handle is wiped when it is done with it.
fn render_fresh_mark(full: bool, fine: bool, g: u64) -> bool {
    let (grants, r, salt) = unsafe { FRESH_JOB };
    let side = 2 * r + 4;
    let mut text = [0u8; 4 * MAX_CHARS];
    let n = with_fresh_text(|t, n| {
        text[..n].copy_from_slice(&t[..n]);
        n
    });
    let t0 = now();
    let buf = unsafe { &mut (&mut *(&raw mut FRESH))[..side * side] };
    let mask = unsafe { &mut (&mut *(&raw mut FRESH_MASK))[..(side * side).div_ceil(64)] };
    let mid = side as f32 * 0.5;
    let map = unsafe { &mut *(&raw mut FRESH_MAP) };
    let moved_on = || fine && FRESH_GEN.load(core::sync::atomic::Ordering::Acquire) != g;
    let done = tapuwae::render_fresh(buf, side, mid, mid, r as f32, &salt, core::str::from_utf8(&text[..n]).unwrap_or(""), 4, mask, map, full, fine, &moved_on);
    for b in text.iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    let mut l = Line::new();
    let what = match (full, fine, done) {
        (true, _, _) => "drawn",
        (_, false, _) => "follows the handle",
        (_, true, true) => "cleared up",
        (_, true, false) => "refine stopped for a newer handle",
    };
    let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: attest mark {what} ({side} px, {}) in {} ms on the X3\n", if fine { "4x4" } else { "1" }, ferros_sys::us_since(t0) / 1000));
    console(grants, l.as_bytes());
    unsafe { FRESH_SIDE = side };
    FRESH_STATE.store(READY, core::sync::atomic::Ordering::Release);
    done
}

/// The handle as the mark's thread is to draw it, under FRESH_LOCK (held only for a copy of at most 4·MAX_CHARS bytes, so a spin).
fn with_fresh_text<R>(f: impl FnOnce(&mut [u8; 4 * MAX_CHARS], usize) -> R) -> R {
    while FRESH_LOCK.swap(true, core::sync::atomic::Ordering::Acquire) {
        core::hint::spin_loop();
    }
    let r = unsafe {
        let (t, n) = &mut *(&raw mut FRESH_TEXT);
        f(t, *n)
    };
    FRESH_LOCK.store(false, core::sync::atomic::Ordering::Release);
    r
}

/// Hand the mark's thread the handle as it now stands, when it changed: copied under the lock, the generation moved on, the thread woken.
fn publish_fresh_text(handle: &[u8]) {
    let changed = with_fresh_text(|t, n| {
        if &t[..n] == handle {
            return false;
        }
        t[..handle.len()].copy_from_slice(handle);
        for b in t[handle.len()..].iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        unsafe { (*(&raw mut FRESH_TEXT)).1 = handle.len() };
        true
    });
    if changed {
        FRESH_GEN.fetch_add(1, core::sync::atomic::Ordering::Release);
        ferros_sys::wake(&FRESH_GEN, 1);
    }
}

/// The mark centred at (cx, cy) on the surface, as it stands: the rendered mark when READY (tapuwae's α + darkness over black is α − darkness a channel, 8 bits, widened to the surface's gamma-2 values, white = Q14, red in the low 16 bits as the display server reads them), the ring when RING, nothing while PENDING.
fn draw_mark(s: &mut [u64], w: usize, cx: usize, cy: usize, r: usize) {
    match FRESH_STATE.load(core::sync::atomic::Ordering::Acquire) {
        READY => {}
        RING => {
            mark(s, w, cx, cy, r);
            return;
        }
        _ => return,
    }
    let side = unsafe { FRESH_SIDE };
    let buf = unsafe { &(&*(&raw const FRESH))[..side * side] };
    let (x0, y0) = (cx - side / 2, cy - side / 2);
    for y in 0..side {
        for x in 0..side {
            let d = buf[y * side + x];
            let a = (d >> 24) as u64;
            if a == 0 {
                continue;
            }
            let ch = |sh: u32| (a - (d >> sh & 255) as u64) * 16384 / 255;
            s[(y0 + y) * w + x0 + x] = ch(16) | ch(8) << 16 | ch(0) << 32;
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
    // Unlock at boot: when it is on, the Photon client loads the stored seed and the screen only waits for membership (Starting). Otherwise the handle is typed (Fresh).
    let unlock_at_boot = ferros_sys::pref(ferros_sys::PREF_UNLOCK_AT_BOOT) == 1;
    let mode = if unlock_at_boot { Mode::Starting } else { Mode::Fresh };
    let mut kb = Keyboard::new(w, h);
    kb.left = "clear";
    // The device's layout; a value from a newer build than this one falls back to the default.
    let l = ferros_sys::pref(ferros_sys::PREF_LAYOUT) as usize;
    kb.layout = if l < honeycomb::LAYOUTS { l } else { 0 };
    let mut a = Ahau { grants, display, base, w, h, kb, mode, session_seed: None, session_proof: None, unlock_at_boot, handle: [0; 4 * MAX_CHARS], len: 0, chars: 0, word_buf: [0; 512], word_len: 0, started: now() };
    start_fresh_mark(grants, w / 3);
    a.all();
    a.show();
    console(grants, if mode == Mode::Starting { b"ahau: unlock at boot: the client loaded the stored seed; waiting for membership\n" } else { b"ahau: no identity in hand; type the handle\n" });
    let mut polls = 0u32;
    let mut touches = Touches::new(w);
    // Every key exact: a rolling finger still types where it landed.
    touches.trace_words = false;
    let mut held = [(0u8, 0i64, 0i64); 16];
    // The last frame of the mark put on the glass (FRESH_DONE's count).
    let mut mark_shown = 0u64;
    loop {
        let done = FRESH_DONE.load(core::sync::atomic::Ordering::Acquire);
        if done != mark_shown {
            let (cx, cy, r) = a.mark_at();
            draw_mark(a.surf(), a.w, cx, cy, r);
            a.show();
            mark_shown = done;
        }
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
                Touch::TapAt(x, y) => a.tap_at(x, y),
                // A handle is exact bytes, typed key by key: no shape writing here.
                Touch::Trail(..) | Touch::Traced => {}
            }
        }
        // Following the Photon client's verdict while the identity is looked up or joined: about four times a second.
        polls += 1;
        let followed = polls % 64 == 0 && a.follow_client();
        if a.follow_layout() || no > 0 || followed {
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
