//! ahau: the attest screen (Claude memories/ferros_attest_os_level.md; the capsule the owner named, Māori "I, me"). The first thing on the glass at boot: init starts it before the map, at the system level, so it covers everything until the person proves who they are by typing their handle on the honeycomb keyboard. Then its window goes and the map is there.
//!
//! The derivation is ihi's, computed here without std (ahau_core): the identity seed is BLAKE3 over vsf's `x` encoding of the handle, byte-exact for every character the keyboard can type. Nothing of the identity is written to disk: the seed and proof are handed to the identity service (fgtw) in RAM (op 4), and the device is recognised as its identity's only once the client folds the public chain and finds this device a member (owner, 2026-10-28: "sand on the beach with no keys anywhere and it's all derived from the handle and the ira"). The one exception is opt-in: `device.unlock_at_boot` stores the seed so the device unlocks without the handle being typed — a stolen device then holds the identity. It is not switched here (owner, 2026-10-29: the attest screen carries no toggle); wherever it is switched on must say so.
//!
//! - Default: the handle is typed each boot. Its proof (ihi's memory-hard proof, 24.9 MB of scratch, 17 rounds) is derived, the seed and proof go to the identity service (fgtw) in RAM, and the client looks the identity up. A member unlocks; a fleet to join shows the words; a free handle is minted.
//! - Opt-in (unlock at boot): the seed is stored, the client loads it from the vault at boot, and the screen unlocks as soon as the client reports membership, with no handle typed.
//!
//! The handle shows as it is typed: it is a name the person proves, not a password (owner, 2026-10-27: "this is NOT a password"). A wrong one stays on screen to be corrected; the bar's left key clears it. Minting a new identity asks for the handle a second time, shown as typed like the first, the first cleared so a typo is not simply copied (owner, 2026-10-27: "you only do that once"); clear during the second goes back to the first. The buffer is wiped once attest succeeds.
//!
//! The scratch is this process's own .bss (25 MiB): the entry's header (`ferrosRT`, ferros_rt's) tells the kernel how much zeroed memory follows the code, so the arc holds it (payload.ld).
//!
//! The keyboard is the device's layout (`device.layout`, ferros_sys::PREF_LAYOUT: Dvorak unless set), which the last identity to attest leaves on the device (SETTINGS.md). There is no switch on this screen (owner, 2026-10-29: the room went to the mark); the layout is set from the identity's settings or `diag pref`, and the keyboard follows it.
//!
//! A first entry is not a claim (2026-10-28, after it let a person in under a handle that already had a fleet): the handle's roots go into the vault and the identity service (fgtw) looks the identity up, its fleet's devices nearby first and fgtw.org last (photon docs/pairing-v2.md, peers-are-fgtw.md). A fleet: this device joins it, its pairing words shown for one of the fleet's devices to add it. No fleet anywhere: the permanence warning, the handle again, and the client mints the genesis. Nothing answering: nothing is decided, and the person may go on while the client keeps looking. The screen does not hold the device hostage meanwhile (owner: "Doesn't have to stay locked").
//!
//! Grant slots: 0 console, 1 display (use), 2 touch (use), 3 its surface, 4 the vault scope `ahau.`, 5 the vault scope `fgtw.`, 6 the ira key, 7 settings, 8 the identity service, fgtw (use), 9 the identity unlock right, 10 a second surface (the keyboard's back buffer).
#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::arch::global_asm;
use ferros_sys::{call, cap, console, exit, freq, now, sleep_until, window};
use honeycomb::{Event, Key, Keyboard, Touch, Touches};

/// Typing at the blinkey: `c` into the handle bytes `t` (`n` of them, `chars` characters) at byte `at`, the bytes after it moved up. Nothing at MAX_CHARS. Returns where the blinkey goes: after `c`.
fn insert_at(t: &mut [u8; 4 * MAX_CHARS], n: &mut usize, chars: &mut usize, at: usize, c: char) -> usize {
    if *chars >= MAX_CHARS {
        return at;
    }
    let mut b = [0u8; 4];
    let e = c.encode_utf8(&mut b).as_bytes();
    t.copy_within(at..*n, at + e.len());
    t[at..at + e.len()].copy_from_slice(e);
    *n += e.len();
    *chars += 1;
    at + e.len()
}

/// Delete at the blinkey: the character before byte `at` out of the handle bytes, the bytes after it moved down and the freed tail wiped. Returns where the blinkey goes: where that character began.
fn delete_before(t: &mut [u8; 4 * MAX_CHARS], n: &mut usize, chars: &mut usize, at: usize) -> usize {
    if at == 0 {
        return 0;
    }
    // Back to the character's lead byte (the handle is UTF-8: one comes before any continuation byte).
    let mut s = at - 1;
    while t[s] & 0xC0 == 0x80 {
        s -= 1;
    }
    t.copy_within(at..*n, s);
    let gone = at - s;
    for b in t[*n - gone..*n].iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    *n -= gone;
    *chars -= 1;
    s
}

const S_DISPLAY: u64 = 1;
const S_TOUCH: u64 = 2;
const S_SURF: u64 = 3;
/// The vault scope `ahau.` (identity class, AUTH.md): `proof`, the unlocked identity's handle proof once computed here.
const S_AHAU: u64 = 4;
/// The vault scope `fgtw.` (identity class): the identity service's, where the roots go when unlock-at-boot keeps them.
const S_FGTW: u64 = 5;
const S_SETTINGS: u64 = 7;
/// The ira key: per-purpose keys derived from the device's ira (here the attest mark's salt).
const S_IRA: u64 = 6;
/// The attest mark's pixels as tapuwae renders them (tapuwae::pack16: the surface's own 16-bit gamma-2 light, white = Q14, coverage in the top 16 bits), drawn by the mark's own thread and blitted at every redraw after; its side is FRESH_SIDE.
/// 2²⁰ pixels holds a mark of radius up to 510 (side 2r + 4): w/3 on panels up to 1530 wide.
static mut FRESH: [u64; 1 << 20] = [0; 1 << 20];
static mut FRESH_SIDE: usize = 0;
/// Where the mark stands: PENDING while its thread renders (the screen shows nothing there yet), READY once FRESH holds it, RING when there is none (no ira key, a panel too wide) and the drawn ring stands in.
static FRESH_STATE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(PENDING);
const PENDING: u64 = 0;
const READY: u64 = 1;
const RING: u64 = 2;
/// A board as drawn untinted, its rows from `kb.top` to the bar's foot (owner, 2026-10-29: a mask and an index to fill): each pixel's grey (one channel: it is all grey), its key (`Keyboard::tint_index`; 0 off the keys, where the weight is 0), and how much of its key's tint it takes (Q14): whole on the key and its edge (at or below the key's grey), none on the label (at its grey, LABEL) and off the keys, in between across the label's anti-aliased edge, so the labels stay white. 2²⁰ pixels hold the 1344-wide board's 621 rows.
struct Board {
    base: [u16; 1 << 20],
    weight: [u16; 1 << 20],
    index: [u8; 1 << 20],
}
const NO_BOARD: Board = Board { base: [0; 1 << 20], weight: [0; 1 << 20], index: [0; 1 << 20] };
/// Two boards kept (owner, 2026-11-01: copy and tint): the one on show and the one a held key will leave, or the other case, so a shift, a letter or a lift is one tint pass and a copy, with no label drawn. A board missing is drawn once into the slot not on show (`Ahau::board`); `Ahau::boards` says which board each slot holds.
static mut BOARDS: [Board; 2] = [NO_BOARD; 2];
/// What a board's untinted pixels follow: its layout, its view, upper case or not, the bar's left key's label. The shift key's own look (off, once, lock) is drawn over it at every paint.
type BoardKey = (usize, usize, bool, &'static str);

fn board_key(kb: &Keyboard) -> BoardKey {
    (kb.layout, kb.view, kb.shift != honeycomb::Shift::Off, kb.left)
}

/// The mark thread's work: the grants (for its console line), the radius and the salt, set before it starts; and its stack.
static mut FRESH_JOB: (u64, usize, [u8; 32]) = (0, 0, [0; 32]);
static mut FRESH_STACK: [u8; 1 << 16] = [0; 1 << 16];
/// Which pixels of FRESH hold any of a component (tapuwae::render_fresh's mask): what a new handle redraws.
static mut FRESH_MASK: [u64; (1 << 20) / 64] = [0; (1 << 20) / 64];
/// The mark's two maps (tapuwae::FreshCell, 8 bytes a sample): where the masked pixels' samples fall in the set, worked out once, so a new handle costs the balls' shading alone. Independent, so the coarse pass is a sixteenth of the fine one at boot too: the coarse map, a pixel's centre (2¹⁸ cells, 2 MiB, for the 900 px mark's 171 643 masked pixels); the fine map, 4 × 4 a pixel, filled by the first fine pass to reach each (3·2²⁰ cells, 24 MiB, for 2.75 M). Past either tapuwae works the rest out again.
static mut FRESH_COARSE: [tapuwae::FreshCell; 1 << 18] = [[0; 4]; 1 << 18];
static mut FRESH_MAP: [tapuwae::FreshCell; 3 << 20] = [[0; 4]; 3 << 20];
/// The handle as published for the mark (bytes, length), under FRESH_LOCK; FRESH_GEN counts publications, FRESH_DONE is one past the generation of the last finished frame (0 = none yet).
static mut FRESH_TEXT: ([u8; 4 * MAX_CHARS], usize) = ([0; 4 * MAX_CHARS], 0);
static FRESH_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static FRESH_GEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static FRESH_DONE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The generation the last finished frame drew (FRESH_DONE_GEN), and the generation of the handle as typed (SHOW_GEN): the screen puts up only a frame of the typed handle, so a preview drawn while a finger is down waits for the lift, and one the finger slid off never shows.
static FRESH_DONE_GEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static SHOW_GEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// fgtw's endpoint (payloads/fgtw, the identity service): op 1 its state and, while joining, its pairing words; op 2 mint.
const S_FGTW_EP: u64 = 8;
/// Grant slot 9: the identity unlock right (SYS_UNLOCK, AUTH.md): the person's space opens from the seed, before the proof.
const S_UNLOCK: u64 = 9;
/// Slot 10: a second surface, the keyboard's back buffer: a board that a key's tap will redraw (shift, a page, a layout) is drawn there whole while the finger is down and copied in at the lift (`prepare`), as the mark's frame is rendered ahead by its thread. Without it the lift redrew the board in place, 59 ms (2026-11-01: draw 14, the key map 37, the tint 8), and the shift showed a few frames late.
const S_BACK: u64 = 10;

/// The surfaces through their handles (SANDBOX.md, "Handles"): the screen draws into `BUF`, a frame-sized buffer in its own arc, and presents the rows it touched through the shown surface's handle, so the same binary runs native and masked and the drawing code (honeycomb, tapuwae, the pills) never holds a pointer outside the arc. Text is the display's to draw, into a surface it knows: the rows go to that surface first (`put_rows`), the text is asked for, and the rows come back (`get_rows`) before anything more is drawn over them. The second surface stays a display surface for that, reached through its own handle. Labels the keyboard draws are recorded while it draws and asked for after the rows are presented, or the presenting would wipe them.
type Front = ferros_sys::Handle<S_SURF>;
type BackS = ferros_sys::Handle<S_BACK>;
static mut BUF: [u64; (32 << 20) / 8] = [0; (32 << 20) / 8];
/// A recorded label: x, baseline, size, value, the text and its length.
static mut LABELS: [(i64, i64, u16, u16, [u8; 32], usize); 256] = [(0, 0, 0, 0, [0; 32], 0); 256];
static mut NLABELS: usize = 0;
/// The longest handle the screen takes, in characters.
const MAX_CHARS: usize = 64;
/// Grey working values: the mark, the prompt, the status line (gamma 2, white = Q14).
const MARK: u64 = 9000;
const TEXT: u16 = 16384;
const QUIET: u16 = 7000;
/// The entry's line: a quarter of white in gamma 2 (Q14).
const LINE: u64 = 1 << 12;
/// The button: Photon's attest button in grey (fluor's Button: the squircle of power 3, corners h/2 and h/4 by turns, a two-tone hairline of a 32nd of the label's size plus a pixel, shadow above the anti-diagonal and light below, so it reads raised). The fill as the back and attest buttons always were; the edges fluor's textbox edges as greys.
const PILL_SHADOW: u64 = 3100;
const PILL_LIGHT: u64 = 4096;

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
    /// No identity in hand: the entry.
    Fresh,
    /// The roots of this seed are in the vault; the identity service (fgtw) is looking the identity up.
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
    /// Whether the device is bound to an identity (one identity per device, AUTH.md): its handle unlocks, any other is denied.
    bound: bool,
    /// This session's last derived seed and proof (RAM only), so typing the same handle again (after `back`) skips the proof.
    session_seed: Option<[u8; 32]>,
    session_proof: Option<[u8; 32]>,
    /// The roots are set but the client has not taken them yet (`send_roots`).
    roots_pending: bool,
    handle: [u8; 4 * MAX_CHARS],
    len: usize,
    chars: usize,
    /// The blinkey's place in the handle, a byte offset on a character's boundary: typing inserts there, delete takes the character before it, a tap on the entry line moves it.
    caret: usize,
    /// The identity service's pairing words while this device joins its identity's fleet.
    word_buf: [u8; 512],
    word_len: usize,
    /// The back surface's base (0: none granted, the board is drawn in place), where boards are drawn and tinted before they show; which board each slot of BOARDS holds, and the slot on show.
    back: u64,
    boards: core::cell::Cell<[Option<BoardKey>; 2]>,
    shown_board: core::cell::Cell<usize>,
    /// Photon's blinkey: which end of its wave is bright, when it next flips (on the counter), and its xorshift32 state (fluor `BlinkTimer`'s seed).
    wave_top: core::cell::Cell<bool>,
    blink_next: core::cell::Cell<u64>,
    blink_rng: core::cell::Cell<u32>,
    /// The entry line's pan in pixels (TEXT.md: 0 while the handle fits), and the drag that moves it: where its finger landed and the pan then.
    pan: core::cell::Cell<i64>,
    grab: core::cell::Cell<Option<(i64, i64, i64)>>,
}

impl Ahau {
    fn surf(&self) -> &'static mut [u64] {
        unsafe { core::slice::from_raw_parts_mut(&raw mut BUF as *mut u64, self.w * self.h) }
    }

    /// Rows y0..y1 of the buffer onto the shown surface.
    fn present(&self, y0: usize, y1: usize) {
        Front::write_words(window::PORCH + (y0 * self.w * 8) as u64, &self.surf()[y0 * self.w..y1 * self.w]);
    }

    /// Rows y0..y1 of the buffer onto surface `tb` (the second surface or the shown one), for the display to draw text into.
    fn put_rows(&self, tb: u64, y0: usize, y1: usize) {
        let rows = &self.surf()[y0 * self.w..y1 * self.w];
        let off = window::PORCH + (y0 * self.w * 8) as u64;
        if tb == self.back && self.back != 0 { BackS::write_words(off, rows) } else { Front::write_words(off, rows) }
    }

    /// Rows y0..y1 back from surface `tb` into the buffer, with what the display drew there.
    fn get_rows(&self, tb: u64, y0: usize, y1: usize) {
        let rows = &mut self.surf()[y0 * self.w..y1 * self.w];
        let off = window::PORCH + (y0 * self.w * 8) as u64;
        if tb == self.back && self.back != 0 { BackS::read_words(off, rows) } else { Front::read_words(off, rows) }
    }

    /// The labels recorded by the last keyboard draw, drawn by the display onto surface `tb`.
    fn replay_labels(&self, tb: u64) {
        let n = unsafe { NLABELS };
        for i in 0..n {
            let (x, y, px, v, text, len) = unsafe { (*(&raw const LABELS))[i] };
            window::text_centred(self.display, tb, x as i16, y as i16, px, v, &text[..len]);
        }
        unsafe { NLABELS = 0 };
    }

    fn label(&self) -> impl FnMut(i64, i64, u16, u16, &str) + '_ {
        move |x, y, px, v, t| unsafe {
            let n = NLABELS;
            if n < 256 {
                let b = t.as_bytes();
                let len = if b.len() > 32 { 32 } else { b.len() };
                let mut text = [0u8; 32];
                text[..len].copy_from_slice(&b[..len]);
                (*(&raw mut LABELS))[n] = (x, y, px, v, text, len);
                NLABELS = n + 1;
            }
        }
    }

    /// Text centred across the screen on `baseline`, by the keyboard's estimate of the typeface's advance (0.56 em).
    fn centred(&self, baseline: usize, px: usize, value: u16, t: &str) {
        let x = self.w as i64 / 2 - t.chars().count() as i64 * px as i64 * 28 / 100;
        window::text(self.display, self.base, x as i16, baseline as i16, px as u16, value, t.as_bytes());
    }

    fn clear_rows(&self, y0: usize, y1: usize) {
        self.surf()[y0 * self.w..y1 * self.w].fill(0);
        self.present(y0, y1);
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
        // On the entry line: the blinkey to the nearest character boundary.
        let (b, px) = self.entry_at();
        if ((b - px * 3 / 2) as i64..(b + px / 2) as i64).contains(&y) {
            self.place_caret(x);
            return;
        }
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
        self.show_board();
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
            Mode::Fresh => Some(if self.bound { "unlock" } else { "bind" }),
            Mode::Join(_) | Mode::Grey(_) => Some("go on"),
            Mode::Mint(_) => Some("mint"),
            Mode::Wrong => Some("start over"),
            Mode::Looking(_) => None,
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
            pill(s, self.w, bx0, by0, bx1, by1, 1800, self.w / 24);
        }
        let (x0, y0, x1, y1) = self.button_at();
        if self.button_label().is_some() {
            pill(s, self.w, x0, y0, x1, y1, 2600, self.w / 24);
        }
        // The row presented before the display writes its words on it.
        self.present(ry0, ry1);
        if self.can_go_back() {
            let (bx0, by0, bx1, by1) = self.back_at();
            let px = self.w / 24;
            let x = (bx0 + bx1) as i64 / 2 - 4 * px as i64 * 28 / 100;
            window::text(self.display, self.base, x as i16, ((by0 + by1) / 2 + px * 35 / 100) as i16, px as u16, TEXT, b"back");
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

    /// The identity service's state (op 1): its state byte, with the pairing words kept while it joins.
    fn client_state(&mut self) -> Option<u8> {
        let mut out = [0u8; 600];
        // CALL_LIVE: a client that is not answering yet (starting, or between passes of its loop) is a None this poll, never a frozen screen.
        let n = ferros_sys::call_live(cap(self.grants, S_FGTW_EP), &[1], &mut out);
        if n == ferros_sys::ERR || n == ferros_sys::NOTHING || n < 2 {
            return None;
        }
        let n = (n as usize).min(out.len());
        let k = (n - 2).min(self.word_buf.len());
        self.word_buf[..k].copy_from_slice(&out[2..2 + k]);
        self.word_len = k;
        Some(out[0])
    }

    /// While the identity is being looked up or joined: follow the client. Returns whether the screen changed.
    fn follow_client(&mut self) -> bool {
        let seed = match self.mode {
            Mode::Looking(s) | Mode::Join(s) | Mode::Grey(s) => s,
            _ => return false,
        };
        if self.roots_pending {
            self.roots_pending = !self.hand_roots();
            if self.roots_pending {
                return false;
            }
        }
        let Some(state) = self.client_state() else { return false };
        let next = match state {
            1 => Mode::Join(seed),
            2 | 3 => {
                console(self.grants, b"ahau: this device is a member of the identity's fleet\n");
                self.mark_member();
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

    /// Hand the roots to the identity service (fgtw) in RAM (op 4): it takes them, keeps them on disk only if unlock-at-boot is on, and looks the identity up. Nothing is written here. The call does not wait for a client that is not answering yet (CALL_LIVE): the hand-over is then marked pending and made again at the next poll (`follow_client`), so the screen never freezes on it.
    fn send_roots(&mut self, seed: &[u8; 32], proof: &[u8; 32]) {
        self.session_seed = Some(*seed);
        self.session_proof = Some(*proof);
        self.roots_pending = !self.hand_roots();
    }

    /// One attempt at the op-4 hand-over of the session's roots: true when the client took them.
    fn hand_roots(&self) -> bool {
        let (Some(seed), Some(proof)) = (self.session_seed, self.session_proof) else { return false };
        let mut m = [0u8; 65];
        m[0] = 4;
        m[1..33].copy_from_slice(&seed);
        m[33..65].copy_from_slice(&proof);
        let mut out = [0u8; 16];
        ferros_sys::call_live(cap(self.grants, S_FGTW_EP), &m, &mut out) != ferros_sys::ERR
    }

    /// The entry: the handle as typed, a cursor, and a line under it.
    fn entry(&self) {
        publish_fresh_text(&self.handle[..self.len], true);
        // A keystroke restarts the blinkey: bright at the top, its next tick a fresh interval away (Photon: the caret stays solid through typing).
        self.wave_top.set(true);
        self.blink_next.set(self.next_blink());
        self.entry_line(true);
    }

    /// The blinkey's next tick: a geiger on the display's 10 Hz grid (owner, 2026-11-01: "quantize the blinkey to 10hz, or geiger it"): one to three whole tenths of a second away at random (xorshift32, as Photon's fluor `BlinkTimer` draws its 0–300 ms), landing on a tenth of a second of the counter, so a tick never asks for a frame between the dither's own flips. The irregularity still reads as alive.
    fn next_blink(&self) -> u64 {
        let mut r = self.blink_rng.get();
        r ^= r << 13;
        r ^= r >> 17;
        r ^= r << 5;
        self.blink_rng.set(r);
        let tenth = freq() / 10;
        (now() / tenth + 1 + (r % 3) as u64) * tenth
    }
    /// A blinkey tick, when due: the wave flips end and the entry line is drawn again. True when it drew.
    fn blink(&self) -> bool {
        if now() < self.blink_next.get() {
            return false;
        }
        self.wave_top.set(!self.wave_top.get());
        self.blink_next.set(self.next_blink());
        self.entry_line(false);
        true
    }

    /// Photon's blinkey (fluor `paint::draw_blinkey`, owner 2026-11-01: "steal the vertical blinkey"): a caret `h` high from `top` at column `x`, its brightness a wave down its height peaking toward the top or the bottom (`wave_top`; each tick flips it), smeared sideways at half the brightness per step, added onto what is there. The wave peaks at (8/9)(16/9) × 100 ≈ 158 of 255, Photon's CURSOR_BRIGHTNESS, here in the working values (white Q14). The smear scales with the text: fluor halves it every pixel over 7 at Photon's 16-20 px text, a hairline beside the entry's 84 px letters, so here a step is a thirty-second of the text's size `px` (a sixteenth read as a bloom, 2026-11-01), over 7 steps either side, added in linear light.
    fn blinkey(&self, s: &mut [u64], x: usize, top: usize, h: usize, px: usize) {
        let step = px as f32 / (1 << 5) as f32;
        let reach = (7. * step) as usize;
        // WHY/PROOF: the caret follows the typed text, and a long handle pushes it (and its smear) past the screen's edge — the smear would index into the next row. Photon's textbox guards the same.
        if x < reach || x + reach >= self.w || h < 2 {
            return;
        }
        let half = h / 2;
        let top_bright = self.wave_top.get();
        for y in top..top + h {
            let t = (y as f32 - top as f32 - half as f32) / half as f32;
            let wave = if top_bright { (1. - t * t) * (1. - t) * (1. - t) } else { (1. - t * t) * (1. + t) * (1. + t) } * 100.;
            // A byte's worth (up to ~158) in white's Q14: × 64.
            let w_base = wave * (1 << 6) as f32;
            for dx in -(reach as i64)..=reach as i64 {
                let w = (w_base * libm::exp2f(-(dx.unsigned_abs() as f32) / step)) as u64;
                if w == 0 {
                    continue;
                }
                let i = y * self.w + (x as i64 + dx) as usize;
                let p = s[i];
                // Light adds in linear light: the working values are gamma 2, so the caret's brightness w joins a pixel v as √(v² + w²) — over black the same as adding w, over the letters and the line no brighter than their light and the caret's together. WHY/PROOF the min: not a guard — brightening stops at white, as Photon's caret stops at no darkness; past it the value would carry into the next channel.
                let add = |c: u64| (c * c + w * w).isqrt().min(honeycomb::WHITE);
                s[i] = add(p & 0xFFFF) | add(p >> 16 & 0xFFFF) << 16 | add(p >> 32 & 0xFFFF) << 32;
            }
        }
    }

    /// The handle as the entry line shows it — a newline as ¶ (the typeface has no return arrow), so the handle stays on one line as typed — its length, and where the blinkey falls in it.
    fn shown(&self) -> ([u8; 4 * MAX_CHARS + 4 * MAX_CHARS], usize, usize) {
        let mut t = [0u8; 4 * MAX_CHARS + 4 * MAX_CHARS];
        let (mut n, mut at) = (0, 0);
        for (i, &c) in self.handle[..self.len].iter().enumerate() {
            if i == self.caret {
                at = n;
            }
            if c == b'\n' {
                t[n..n + 2].copy_from_slice("¶".as_bytes());
                n += 2;
            } else {
                t[n] = c;
                n += 1;
            }
        }
        if self.caret >= self.len {
            at = n;
        }
        (t, n, at)
    }

    /// The width of `t` at `px` in the display's typeface, quarter pixels (display op 28); an older display server that cannot say gets the old estimate (0.56 em a character).
    fn measure(&self, px: usize, t: &[u8]) -> i64 {
        match window::width(self.display, px as u16, t) {
            Some(q) => q as i64,
            None => core::str::from_utf8(t).map_or(0, |s| s.chars().count() as i64) * px as i64 * 224 / 100,
        }
    }

    /// The entry line's window (TEXT.md, "The attest line"): the underline's span, from an eighth of the width to seven eighths; the fade's width, half an em of the text size `px`; and inside the fades, where text that fits sits whole.
    fn window_of(&self, px: usize) -> (i64, i64, i64, i64) {
        let (w0, w1, f) = ((self.w / 8) as i64, (self.w * 7 / 8) as i64, (px / 2) as i64);
        (w0, w1, w0 + f, w1 - f)
    }

    /// Where text `tw` pixels wide starts on the line: centred while it fits inside the fades, else centred and moved by the pan.
    fn text_left(&self, px: usize, tw: i64) -> i64 {
        let (_, _, in0, in1) = self.window_of(px);
        let xc = self.w as i64 / 2 - tw / 2;
        if tw <= in1 - in0 { xc } else { xc + self.pan.get() }
    }

    /// A pan held to the text's ends: its left end no further right than the window's inner left edge, its right end no further left than the inner right edge. WHY/PROOF: past either the text would leave the window with blank line beside it, the ends the owner asked to stop at (TEXT.md, Open 2: a hard stop, a bound with a velocity step, the owner's to accept or replace); text that fits has no pan.
    fn bound_pan(&self, px: usize, tw: i64, p: i64) -> i64 {
        let (_, _, in0, in1) = self.window_of(px);
        if tw <= in1 - in0 {
            return 0;
        }
        let xc = self.w as i64 / 2 - tw / 2;
        let (lo, hi) = (in1 - tw - xc, in0 - xc);
        if p < lo { lo } else if p > hi { hi } else { p }
    }

    /// A one-finger drag that landed on the entry line at (x0, y0) is at x now: the text pans with it, the character it grabbed staying under the finger (TEXT.md). The pan at the landing is kept for the drag, keyed by where it landed.
    fn drag_text(&self, x0: i64, y0: i64, x: i64) -> bool {
        let (b, px) = self.entry_at();
        if !((b - px * 3 / 2) as i64..(b + px / 2) as i64).contains(&y0) {
            return false;
        }
        let grab = match self.grab.get() {
            Some(g) if g.0 == x0 && g.1 == y0 => g.2,
            _ => {
                self.grab.set(Some((x0, y0, self.pan.get())));
                self.pan.get()
            }
        };
        let (t, n, _) = self.shown();
        let tw = self.measure(px, &t[..n]) / 4;
        let p = self.bound_pan(px, tw, grab + x - x0);
        if p == self.pan.get() {
            return false;
        }
        self.pan.set(p);
        self.entry_line(false);
        true
    }

    /// A tap on the entry line: the blinkey to the character boundary nearest `x` (each boundary measured by the display), solid again.
    fn place_caret(&mut self, x: i64) {
        let (_, px) = self.entry_at();
        let (t, n, _) = self.shown();
        let left = self.text_left(px, self.measure(px, &t[..n]) / 4);
        // Walk the handle's character boundaries and the shown text's together (a newline is one byte here, two there).
        let (mut best, mut best_d) = (0usize, i64::MAX);
        let (mut i, mut j) = (0usize, 0usize);
        loop {
            let d = (left + self.measure(px, &t[..j]) / 4 - x).abs();
            if d < best_d {
                (best, best_d) = (i, d);
            }
            if i >= self.len {
                break;
            }
            let c = self.handle[i];
            let step = if c < 0x80 { 1 } else if c >= 0xF0 { 4 } else if c >= 0xE0 { 3 } else { 2 };
            j += if c == b'\n' { 2 } else { step };
            i += step;
        }
        self.caret = best;
        self.wave_top.set(true);
        self.blink_next.set(self.next_blink());
        self.entry_line(false);
    }

    /// The typed handle on the entry line, the blinkey after it, and the line under them. Centred while it fits; wider, inside the underline's window, panned, its ends fading out over half an em (TEXT.md, "The attest line"). `chase`: after a keystroke the pan first follows the caret, landing it exactly on the window's inner edge it crossed (fluor `update_scroll`), so typing at the end keeps the newest characters in view.
    fn entry_line(&self, chase: bool) {
        let (b, px) = self.entry_at();
        let (r0, r1) = ((b - px * 3 / 2) * self.w, (b + px / 2) * self.w);
        // Built whole in the back surface's rows and copied in, so a frame the display composes meanwhile never catches them cleared: drawn in place, the line flickered black at the blinkey's ticks (2026-11-01). Without a back surface, in place. The keyboard's boards use the back surface's rows below these.
        let tb = if self.back != 0 { self.back } else { self.base };
        let s = self.surf();
        s[r0..r1].fill(0);
        let (t, n, at) = self.shown();
        let shown = core::str::from_utf8(&t[..n]).unwrap_or("");
        // Measured by the typeface's own advances (display op 28), the blinkey where the text before it ends.
        let (tw, caret_px) = (self.measure(px, &t[..n]) / 4, self.measure(px, &t[..at]) / 4);
        let (w0, w1, in0, in1) = self.window_of(px);
        if chase {
            let xc = self.w as i64 / 2 - tw / 2;
            let cx = xc + self.pan.get() + caret_px;
            let p = if cx < in0 { in0 - xc - caret_px } else if cx > in1 { in1 - xc - caret_px } else { self.pan.get() };
            self.pan.set(self.bound_pan(px, tw, p));
        }
        let x = self.text_left(px, tw);
        self.put_rows(tb, r0 / self.w, r1 / self.w);
        window::text(self.display, tb, x as i16, b as i16, px as u16, TEXT, shown.as_bytes());
        self.get_rows(tb, r0 / self.w, r1 / self.w);
        // The window: nothing outside it, and over the last half em at each end the text fades by a C∞ smooth step (exp(−1/t) against its mirror: exactly 0 at the window's edge and 1 at the fade's inner edge, every derivative matching there), in linear light: the gamma-2 value times √f. Fixed in place, so text that fits (inside the fades) is never dimmed and nothing jumps as the handle grows past the edge.
        let fade = |t: f32| {
            let e = |u: f32| if u > 0. { libm::expf(-1. / u) } else { 0. };
            e(t) / (e(t) + e(1. - t))
        };
        let f = (in0 - w0) as f32;
        let (y0, y1) = (r0 / self.w, r1 / self.w);
        for col in (0..in0).chain(in1..self.w as i64) {
            let t = (if col < in0 { (col - w0) as f32 + 0.5 } else { (w1 - col) as f32 - 0.5 }) / f;
            let k = (libm::sqrtf(fade(t)) * (1u32 << 14) as f32) as u64;
            for y in y0..y1 {
                let p = &mut s[y * self.w + col as usize];
                let ch = |v: u64| (v & 0xFFFF) * k >> 14;
                *p = ch(*p) | ch(*p >> 16) << 16 | ch(*p >> 32) << 32;
            }
        }
        // The blinkey where the caret is, while it is in the window: a drag can carry it out (typing brings it back, `chase`).
        if (w0..w1).contains(&(x + caret_px)) {
            self.blinkey(s, (x + caret_px) as usize, b - px * 7 / 8, px * 17 / 16, px);
        }
        // The line: three quarters of the width, a single pixel, grey at a quarter in gamma 2 (owner, 2026-10-29).
        let (x0, x1, y) = (self.w / 8, self.w * 7 / 8, b + px / 4);
        s[y * self.w + x0..y * self.w + x1].fill(grey(LINE));
        self.present(r0 / self.w, r1 / self.w);
    }

    fn all(&self) {
        self.surf().fill(0);
        Front::fill64(window::PORCH, (self.w * self.h) as u64, 0);
        let (cx, cy, r) = self.mark_at();
        draw_mark(self.surf(), self.w, cx, cy, r);
        self.present(cy - r - 2, cy + r + 2);
        self.status(self.prompt());
        self.entry();
        self.button();
        self.show_board();
    }

    /// The keyboard's rows: from its top to the bar's foot.
    fn kb_rows(&self) -> (usize, usize) {
        (self.kb.top, self.h - honeycomb::corner_rows(self.w, self.h))
    }

    /// Where a board is drawn and painted before it shows: the back surface (its base and pixels), or, without one, the screen itself.
    fn target(&self) -> (u64, &'static mut [u64]) {
        (if self.back != 0 { self.back } else { self.base }, self.surf())
    }

    /// The board for `kb` (its keys, labels and case, untinted), as a slot of BOARDS: kept, or drawn now into the slot not on show — every key and label through the display (some 50 ms), its greys read back, each pixel's key and tint weight worked out. The key map is the board on show's when the layout and view are the same (a shift, the left key), else hit-tested afresh (some 37 ms).
    fn board(&self, kb: &Keyboard) -> usize {
        let key = board_key(kb);
        let mut keys = self.boards.get();
        if let Some(slot) = (0..2).find(|&i| keys[i] == Some(key)) {
            return slot;
        }
        let shown = self.shown_board.get();
        let slot = 1 - shown;
        let (top, bottom) = self.kb_rows();
        let w = self.w;
        let (tb, t) = self.target();
        let mut plain = kb.clone();
        plain.tint = None;
        plain.draw(t, &mut self.label());
        self.put_rows(tb, top, bottom);
        self.replay_labels(tb);
        self.get_rows(tb, top, bottom);
        let boards = unsafe { &mut *(&raw mut BOARDS) };
        let (b, other) = if slot == 0 { let (x, y) = boards.split_at_mut(1); (&mut x[0], &y[0]) } else { let (x, y) = boards.split_at_mut(1); (&mut y[0], &x[0]) };
        let n = (bottom - top) * w;
        let same = keys[shown].is_some_and(|k| k.0 == key.0 && k.1 == key.1);
        if same {
            b.index[..n].copy_from_slice(&other.index[..n]);
        }
        let label = honeycomb::LABEL as u64;
        for y in top..bottom {
            for x in 0..w {
                let i = (y - top) * w + x;
                let p = t[y * w + x] & 0xFFFF;
                b.base[i] = p as u16;
                let k = if same {
                    b.index[i] as usize
                } else {
                    let k = match kb.hit(x as i64, y as i64) {
                        Key::None => 64,
                        k => Keyboard::tint_index(k),
                    };
                    b.index[i] = if k == 64 { 0 } else { k as u8 };
                    k
                };
                let f = if k < 48 { honeycomb::KEY } else { honeycomb::BAR_KEY };
                // Off the keys (k 64, kept as key 0) no tint at all: the pixel stays the grey it was drawn.
                b.weight[i] = if k == 64 || p >= label { 0 } else if p <= f { 1 << 14 } else { (((label - p) << 14) / (label - f)) as u16 };
            }
        }
        keys[slot] = Some(key);
        self.boards.set(keys);
        slot
    }

    /// The keyboard on the glass as `self.kb` stands: its board (`board`), the rainbow laid on from the base in one pass (three multiplies a channel), the shift key drawn over it as it stands (off, once or lock; it has no label), then copied in whole, so the display never composes it half done. Its rows: the keyboard's, from its top to the bar's foot.
    fn show_board(&self) {
        let (top, bottom) = self.kb_rows();
        let w = self.w;
        let n = (bottom - top) * w;
        // WHY: the board's size follows the panel's; one too big for BOARDS is drawn tinted the slow way (every key and label again) instead of indexing past it.
        if n > 1 << 20 {
            self.kb.draw(self.surf(), &mut self.label());
            self.present(top, self.h);
            self.replay_labels(self.base);
            return;
        }
        let slot = self.board(&self.kb);
        let b = unsafe { &(*(&raw const BOARDS))[slot] };
        let (_, t) = self.target();
        let one = 1i64 << 14;
        let tint = self.kb.tint.unwrap_or([[1 << 14; 3]; 64]);
        let rows = &mut t[top * w..top * w + n];
        for i in 0..n {
            let (p, wt, c) = (b.base[i] as i64, b.weight[i] as i64, &tint[b.index[i] as usize]);
            let ch = |c: u16| (p * (one + (wt * (c as i64 - one) >> 14)) >> 14) as u64;
            rows[i] = ch(c[0]) | ch(c[1]) << 16 | ch(c[2]) << 32;
        }
        self.kb.draw_key(t, Key::Bar(honeycomb::B_SHIFT), false, &mut |_, _, _, _, _| {});
        self.present(top, bottom);
        self.shown_board.set(slot);
    }

    /// A finger is down on `key`: if its tap will change the board (a shift, a page, a layout, the letter ending a one-letter shift), that board is drawn now (`prepare_board`), the way the mark's next frame is rendered ahead by its thread, so the lift is a tint and a copy.
    fn prepare(&self, key: Key) {
        let mut next = self.kb.clone();
        next.tap(key);
        if board_key(&next) != board_key(&self.kb) {
            self.prepare_board(&next);
        }
    }

    /// Board `kb` drawn ahead into BOARDS. Only into the back surface: drawn on the screen it would show untinted. And only one that fits (`show_board`'s check).
    fn prepare_board(&self, kb: &Keyboard) {
        if self.back != 0 && (self.kb_rows().1 - self.kb_rows().0) * self.w <= 1 << 20 {
            self.board(kb);
        }
    }

    fn prompt(&self) -> &'static str {
        match self.mode {
            // The screen's words (owner, 2026-10-31: simple and direct; AUTH.md "What the attest screen says").
            Mode::Fresh if self.bound => "Type your handle.",
            Mode::Fresh => "This device is unbound.\nType your handle to bind.",
            Mode::Looking(_) => "Finding this identity.",
            Mode::Join(_) => "Use an existing device to bind this device to your fleet.",
            Mode::Mint(_) => "This handle is unclaimed.\nMinting is permanent.\nType your handle again to mint your identity.",
            Mode::Grey(_) => "No answer yet. Still looking.",
            Mode::Wrong => "This handle's record doesn't match.\nStart over with another.",
        }
    }

    fn show(&self) {
        window::show(self.display, self.base, 0, 0, self.w as u16, self.h as u16, window::SYSTEM);
    }

    /// The entry line's rows alone presented (`window::show_changed`): a blinkey tick changes nothing else, and a whole show republishes the screen in every dither phase (four full-screen publishes, some 33 ms each on the X3, 2026-11-01).
    fn show_entry(&self) {
        let (b, px) = self.entry_at();
        let (y0, y1) = (b - px * 3 / 2, b + px / 2);
        window::show_changed(self.display, self.base, 0, 0, self.w as u16, self.h as u16, window::SYSTEM, window::WORKING, (0, y0 as u16, self.w as u16, (y1 - y0) as u16));
    }

    /// The typed handle gone from memory.
    fn wipe(&mut self) {
        for b in self.handle.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        self.len = 0;
        self.chars = 0;
        self.caret = 0;
        publish_fresh_text(&[], true);
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
            Mode::Looking(_) => return,
            _ => {}
        }
        if self.chars == 0 {
            return;
        }
        let seed = match core::str::from_utf8(&self.handle[..self.len]).ok().map(ahau_core::identity_seed) {
            Some(Ok(s)) => s,
            // Unreachable by construction: the encoder's table is generated from the keyboard's own characters (ahau_core build.rs), so a key without a code fails the build. Said on the console, not the screen.
            _ => {
                console(self.grants, b"ahau: the handle has a character the encoder has no code for (the keyboard and ahau_core out of step?)\n");
                return;
            }
        };
        // The person's space (AUTH.md): unlocked from the seed at once; the memory-hard proof is the identity service's business (the public registry), not the unlock's. One identity per device: on a device its fleet has bound, the kernel opens only that identity's space, so an open space there is the device's own person and the screen goes now, the proof made behind it; another handle is refused. On a device not bound yet the look-up and the join run as before (closing on "the space is not empty" closed Brittany's screen before her join, 2026-10-31).
        if self.mode == Mode::Fresh {
            match ferros_sys::unlock_identity(cap(self.grants, S_UNLOCK), &seed) {
                // Opened on a bound device: this is the identity the device belongs to.
                ferros_sys::Unlock::Opened(_) if ferros_sys::identity_bound(cap(self.grants, S_UNLOCK)) == Some(true) => self.unlocked_known(seed),
                ferros_sys::Unlock::Opened(_) => console(self.grants, b"ahau: unlocked; this device is not bound to a fleet yet: looking the identity up\n"),
                ferros_sys::Unlock::Belongs => {
                    console(self.grants, b"ahau: refused: this device belongs to another identity\n");
                    self.wipe();
                    self.status("Denied. This device is bound to another identity.");
                    self.entry();
                    return;
                }
                ferros_sys::Unlock::Failed => console(self.grants, b"ahau: the identity could not be unlocked (no Unlock right, or no ira)\n"),
            }
        }
        match self.mode {
            Mode::Fresh => {
                // The same handle again this session (after `back`): its proof is cached in RAM, no second derivation.
                let proof = if self.session_seed == Some(seed) { self.session_proof.unwrap() } else { self.proof_for(&seed, true) };
                // The roots to the client in RAM; it keeps them on disk only if unlock-at-boot is on.
                self.send_roots(&seed, &proof);
                console(self.grants, b"ahau: roots handed to fgtw; it looks the identity up\n");
                self.wipe();
                self.mode = Mode::Looking(seed);
                self.status(self.prompt());
            }
            Mode::Mint(first) => {
                if !ahau_core::same(&first, &seed) {
                    self.status("They don't match. Try again.");
                    return;
                }
                self.status("Minting.");
                self.entry();
                self.show();
                let mut out = [0u8; 600];
                let n = ferros_sys::call_live(cap(self.grants, S_FGTW_EP), &[2], &mut out);
                let r = if n == ferros_sys::ERR || n == 0 { 255 } else { out[0] };
                match r {
                    0 => {
                        console(self.grants, b"ahau: minted; this device is the identity's first member\n");
                        self.mark_member();
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
                        self.status("Minting failed. Enter to retry.");
                    }
                }
            }
            _ => {}
        }
    }

    /// Clear during the mint's second entry only clears it: the verdict stands.
    fn unconfirm(&mut self) {}

    /// The identity's fleet has this device: the kernel binds it to the unlocked identity, so from the next boot only that identity's handle opens it (until a departure releases it).
    fn mark_member(&self) {
        if !ferros_sys::bind_identity(cap(self.grants, S_UNLOCK)) {
            console(self.grants, b"ahau: could not bind this device to the identity\n");
        }
    }

    /// The handle proof for the unlocked identity: kept in its own sealed space after the first time (`ahau.proof`; the proof is public, it is what beacons and the registry carry), so a handle this device recognizes never computes it again. Otherwise computed on the X3 (about 5 s; 9.5 s on a worker core, 2026-10-31), the status saying so when `say`, and kept.
    fn proof_for(&mut self, seed: &[u8; 32], say: bool) -> [u8; 32] {
        let mut p = [0u8; 32];
        if ferros_sys::store_get(cap(self.grants, S_AHAU), b"proof", &mut p) == Some(32) {
            console(self.grants, b"ahau: handle proof from this identity's space (recognized)\n");
            return p;
        }
        if say {
            self.status("Computing identity keys.");
            self.entry();
            self.show();
            // Let the display put the status up before the proof holds the core.
            sleep_until(now() + freq() / 30);
        }
        ferros_sys::pin(ferros_sys::CORES_BIG);
        sleep_until(now() + freq() / 1000);
        let t0 = now();
        let proof = *ihi::handle_proof(&blake3::Hash::from(*seed)).as_bytes();
        let ms = ferros_sys::us_since(t0) / 1000;
        ferros_sys::pin(0);
        let mut l = Line::new();
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: handle proof in {ms} ms on the X3\n"));
        console(self.grants, l.as_bytes());
        if !self.put(S_AHAU, b"proof", &proof) {
            console(self.grants, b"ahau: could not keep the handle proof in the identity's space\n");
        }
        proof
    }

    /// A person this device knows: the screen closes now, then the handle proof is made behind it and the roots go to the identity service (fgtw) (made again until it takes them: it starts once the space is open), then this process ends.
    fn unlocked_known(&mut self, seed: [u8; 32]) -> ! {
        self.wipe();
        console(self.grants, b"ahau: unlocked (this device knows the identity); the screen goes, the proof follows\n");
        window::done(self.display, self.base);
        let proof = self.proof_for(&seed, false);
        self.send_roots(&seed, &proof);
        while self.roots_pending {
            sleep_until(now() + freq() / 16);
            self.roots_pending = !self.hand_roots();
        }
        console(self.grants, b"ahau: roots handed to fgtw\n");
        exit(0)
    }

    fn attested(&mut self) -> ! {
        self.wipe();
        console(self.grants, b"ahau: attested\n");
        window::done(self.display, self.base);
        exit(0)
    }

    /// A finger is down on `key`: the handle as that key would make it goes to the mark's thread now, so its frame is ready by the lift; the text itself changes only at the lift (`tap`). The key's effect is found on a copy of the keyboard (its shift and page are the real one's), so nothing here changes until the tap.
    fn preview(&self, key: Key) {
        let mut kb = self.kb.clone();
        // The handle as the key will leave it, at the blinkey, as `tap` will make it.
        let (mut t, mut n, mut ch) = (self.handle, self.len, self.chars);
        match kb.tap(key) {
            Some(Event::Char(c)) => {
                insert_at(&mut t, &mut n, &mut ch, self.caret, c);
            }
            Some(Event::Enter) => {
                insert_at(&mut t, &mut n, &mut ch, self.caret, '\n');
            }
            Some(Event::Back) => {
                delete_before(&mut t, &mut n, &mut ch, self.caret);
            }
            Some(Event::Leave) => n = 0,
            _ => return,
        }
        publish_fresh_text(&t[..n], false);
        for b in t.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
    }

    fn type_char(&mut self, c: char) {
        self.caret = insert_at(&mut self.handle, &mut self.len, &mut self.chars, self.caret, c);
        self.entry();
    }

    fn tap(&mut self, key: Key) {
        match self.kb.tap(key) {
            Some(Event::Char(c)) => self.type_char(c),
            Some(Event::Back) => {
                self.caret = delete_before(&mut self.handle, &mut self.len, &mut self.chars, self.caret);
                self.entry();
            }
            // Enter is a character of the handle (owner, 2026-10-28: "enter is a valid char"); the button under the entry line attests.
            Some(Event::Enter) => self.type_char('\n'),
            Some(Event::Leave) => {
                self.wipe();
                self.unconfirm();
                self.entry();
            }
            // A shift, a page or a layout: the board, drawn at the press (`prepare`), tinted and copied in.
            Some(Event::Redraw) => {
                self.show_board();
                return;
            }
            None => return,
        }
        // Every stroke: the keys take the new handle's rainbow, laid on from the board in one pass; a letter that ended a one-letter shift brings the lower-case board, drawn at its press.
        self.retint();
        self.show_board();
    }


    /// The keys' rainbow for the handle as typed (owner, 2026-10-29): every key its own hue, the hues spread evenly round the wheel and dealt to the keys by a hash of the handle keyed by the mark's salt (the device's), so the board is always a whole rainbow and every stroke deals it afresh. Each tint is a quarter saturation at the key's own brightness: in linear VSF RGB the grey mixed a quarter of the way to a hue of the same photopic luminance (through LMS, vsf::colour::convert::vsf_rgb_to_photopic_f32; Rec. 709 weights until 2026-11-01, an sRGB assumption the display never shared), back to gamma 2 as a multiplier per channel.
    fn retint(&mut self) {
        self.kb.tint = Some(self.tint_for(&self.handle[..self.len]));
    }

    /// The rainbow for `handle` (`retint`'s).
    fn tint_for(&self, handle: &[u8]) -> [[u16; 3]; 64] {
        let salt = unsafe { FRESH_JOB.2 };
        let mut deal = [0u8; 64];
        blake3::Hasher::new_keyed(&salt).update(b"attest keys").update(handle).finalize_xof().fill(&mut deal);
        let hexes = self.kb.hexes();
        let n = hexes + honeycomb::BAR_KEYS.len();
        // A shuffle of the slots 0..n (Fisher-Yates on the hash's bytes; the small modulo bias does not show in a hue).
        let mut order = [0u8; 64];
        for (i, o) in order[..n].iter_mut().enumerate() {
            *o = i as u8;
        }
        for i in (1..n).rev() {
            order.swap(i, deal[i] as usize % (i + 1));
        }
        let turn = deal[0] as f32 * (1. / (1 << 8) as f32);
        let quarter = 1. / (1 << 2) as f32;
        let mut tint = [[1u16 << 14; 3]; 64];
        for k in 0..n {
            let hue = (order[k] as f32 + turn) / n as f32;
            // A smooth rainbow in linear VSF RGB: each primary a raised cosine a third of a turn from the next, a point in its triangle.
            let lin = [0, 1, 2].map(|c| {
                let v = 0.5 + 0.5 * libm::cosf(core::f32::consts::TAU * (hue - c as f32 / 3.));
                v * v
            });
            let luma = vsf::colour::convert::vsf_rgb_to_photopic_f32(lin[0], lin[1], lin[2]);
            let slot = if k < hexes { k } else { 48 + k - hexes };
            tint[slot] = lin.map(|l| (libm::sqrtf(1. - quarter + quarter * l / luma) * (1u32 << 14) as f32) as u16);
        }
        tint
    }
}

/// Photon's attest button in grey (`PILL_*`), filling (x0, y0)–(x1, y1) on the surface: the outer squircle rounded rectangle in the two edge greys, the inner one (inset by the hairline) in `fill`, each corner a superellipse of power 3 with fluor's asymmetric radii (h/2 at the top left and bottom right, h/4 at the others), every edge anti-aliased by its distance across the curve. `label_px` sizes the hairline as fluor's Button does: a 32nd of the label's size, plus a pixel.
fn pill(s: &mut [u64], w: usize, x0: usize, y0: usize, x1: usize, y1: usize, fill: u64, label_px: usize) {
    let (bw, bh) = ((x1 - x0) as f32, (y1 - y0) as f32);
    let stroke = (label_px / (1 << 5) + 1) as f32;
    let big = bh * 0.5;
    // Coverage of the rounded rectangle inset by `inset` at (fx, fy) from the box's corner: inside its straight edges whole; in a corner square, the superellipse |u/r|³ + |v/r|³ = 1, by the distance across it along the ray from the corner's centre (its own length scaled by f^(−1/3), f the superellipse's value there), half a pixel either side.
    let cover = |fx: f32, fy: f32, inset: f32| -> f32 {
        let (x, y, w, h) = (fx - inset, fy - inset, bw - 2. * inset, bh - 2. * inset);
        if x <= 0. || y <= 0. || x >= w || y >= h {
            return 0.;
        }
        let r_big = big - inset;
        let radii = [r_big, r_big * 0.5, r_big, r_big * 0.5];
        // The corner this point is in, if any: its radius and the point's distances in from the corner square's inner edges.
        let corner = [(x, y, radii[0]), (w - x, y, radii[1]), (w - x, h - y, radii[2]), (x, h - y, radii[3])];
        for (dx, dy, r) in corner {
            if dx < r && dy < r {
                let (u, v) = (r - dx, r - dy);
                let rho = libm::sqrtf(u * u + v * v);
                let f = (u * u * u + v * v * v) / (r * r * r);
                let to_edge = rho / libm::cbrtf(f) - rho;
                // WHY: a coverage is a fraction of the pixel, 0 to 1 by what it means; the edge's one-pixel ramp is held to that.
                return (to_edge + 0.5).max(0.).min(1.);
            }
        }
        1.
    };
    for y in y0..y1 {
        for x in x0..x1 {
            let (fx, fy) = ((x - x0) as f32 + 0.5, (y - y0) as f32 + 0.5);
            let outer = cover(fx, fy, 0.);
            if outer <= 0. {
                continue;
            }
            let inner = cover(fx, fy, stroke);
            // The hairline's tone by the box's anti-diagonal: shadow above it, light below (fluor's raised bevel).
            let edge = if fx / bw + fy / bh <= 1. { PILL_SHADOW } else { PILL_LIGHT };
            let v = fill as f32 * inner + edge as f32 * outer * (1. - inner);
            s[y * w + x] = grey(v as u64);
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

/// The attest mark for a device no identity has attested on yet (tapuwae::render_fresh): the Mandelbrot set in the device's own colour, from a key the ira derives for this purpose alone ("attest mark"), so it is this device's and no other's, with the device's attest ball inside every component, following the handle as it is typed.
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
    // On the worker cores, not the X3: the display server lives on the X3, and the mark's renders (200 ms a keystroke, seconds of refining) shared it in 10 ms slices with every label, measure and frame the display drew — a board drawn at a shift's press took 300 ms, not 59, and everything on the glass lagged (measured 2026-11-01). Typing holds the workers at their active clocks.
    ferros_sys::pin(ferros_sys::CORES_WORKERS);
    sleep_until(now() + freq() / 1000);
    let mut full = true;
    let mut frames = 0u64;
    loop {
        let g = FRESH_GEN.load(core::sync::atomic::Ordering::Acquire);
        for fine in [false, true] {
            if render_fresh_mark(full, fine, g) {
                frames += 1;
                FRESH_DONE_GEN.store(g, core::sync::atomic::Ordering::Release);
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
    let coarse = unsafe { &mut *(&raw mut FRESH_COARSE) };
    let map = unsafe { &mut *(&raw mut FRESH_MAP) };
    let moved_on = || fine && FRESH_GEN.load(core::sync::atomic::Ordering::Acquire) != g;
    let done = tapuwae::render_fresh(buf, side, mid, mid, r as f32, &salt, core::str::from_utf8(&text[..n]).unwrap_or(""), &tapuwae::Look::fresh(), 4, mask, coarse, map, full, fine, &moved_on);
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
    let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: attest mark {what} ({side} px, {}) in {} ms on the workers\n", if fine { "4x4" } else { "1" }, ferros_sys::us_since(t0) / 1000));
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

/// Hand the mark's thread a handle, when it changed: copied under the lock, the generation moved on, the thread woken. `typed`: this is the handle as typed (the screen will show its frames); otherwise a preview, the handle as a key that is still down would make it (owner, 2026-10-29: the render starts at the touch, the text and its mark appear together at the lift).
fn publish_fresh_text(handle: &[u8], typed: bool) {
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
    if typed {
        SHOW_GEN.store(FRESH_GEN.load(core::sync::atomic::Ordering::Acquire), core::sync::atomic::Ordering::Release);
    }
}

/// The mark centred at (cx, cy) on the surface, as it stands: the rendered mark when READY (already the surface's 16-bit light, copied), the ring when RING, nothing while PENDING.
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
            if d >> 48 == 0 {
                continue;
            }
            // The colour channels as they are (light over black, the surface's own scale); the surface keeps its own alpha.
            s[(y0 + y) * w + x0 + x] = d & ((1 << 48) - 1);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ahau_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
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
    // Unlocked and bound already (unlock at boot, `device.unlock_at_boot`, AUTH.md: the kernel unlocked the device's own identity from the seed it kept, before init started anything): no attest screen; the identity service (fgtw) loads its own from that space. Unlocked but not bound is a join in progress (this screen restarted mid-way): the screen comes back, and the handle typed again is recognized at once.
    if ferros_sys::identity_unlocked(cap(grants, S_UNLOCK)) == Some(true) && ferros_sys::identity_bound(cap(grants, S_UNLOCK)) == Some(true) {
        console(grants, b"ahau: the identity was unlocked at boot; no attest screen\n");
        exit(0);
    }
    let mode = Mode::Fresh;
    let mut kb = Keyboard::new(w, h);
    kb.left = "clear";
    // The device's layout; a value from a newer build than this one falls back to the default.
    let l = ferros_sys::pref(ferros_sys::PREF_LAYOUT) as usize;
    kb.layout = if l < honeycomb::LAYOUTS { l } else { 0 };
    let bound = ferros_sys::identity_bound(cap(grants, S_UNLOCK)) == Some(true);
    // The keyboard's back buffer, when init granted a second surface (an older init does not: the board then redraws in place).
    let back = match ferros_sys::dma(cap(grants, S_BACK)) {
        b if b == 0 || b == u64::MAX || b == base => 0,
        b => b,
    };
    let mut a = Ahau { grants, display, base, w, h, kb, mode, bound, session_seed: None, session_proof: None, roots_pending: false, handle: [0; 4 * MAX_CHARS], len: 0, chars: 0, caret: 0, word_buf: [0; 512], word_len: 0, back, boards: core::cell::Cell::new([None; 2]), shown_board: core::cell::Cell::new(0), wave_top: core::cell::Cell::new(true), blink_next: core::cell::Cell::new(0), blink_rng: core::cell::Cell::new(0xDEAD_BEEF), pan: core::cell::Cell::new(0), grab: core::cell::Cell::new(None) };
    console(grants, if back != 0 { b"ahau: the keyboard's back buffer is a second surface\n" } else { b"ahau: no second surface: the keyboard redraws in place\n" });
    start_fresh_mark(grants, w / 3);
    a.retint();
    a.all();
    a.show();
    // The upper-case board drawn ahead too, so the first shift is a tint and a copy.
    if back != 0 {
        let mut up = a.kb.clone();
        up.shift = honeycomb::Shift::Once;
        a.prepare_board(&up);
    }
    console(grants, b"ahau: no identity in hand; type the handle\n");
    let mut polls = 0u32;
    let mut touches = Touches::new(w);
    // Every key exact: a rolling finger still types where it landed.
    touches.trace_words = false;
    let mut held = [(0u8, 0i64, 0i64); 16];
    // The last frame of the mark put on the glass (FRESH_DONE's count).
    let mut mark_shown = 0u64;
    loop {
        let done = FRESH_DONE.load(core::sync::atomic::Ordering::Acquire);
        let typed = FRESH_DONE_GEN.load(core::sync::atomic::Ordering::Acquire) == SHOW_GEN.load(core::sync::atomic::Ordering::Acquire);
        if done != mark_shown && typed {
            let (cx, cy, r) = a.mark_at();
            draw_mark(a.surf(), a.w, cx, cy, r);
            a.present(cy - r - 2, cy + r + 2);
            a.show();
            mark_shown = done;
        }
        // The blinkey's tick, when due: the entry line drawn again with the wave flipped, and only its rows presented.
        if a.blink() {
            a.show_entry();
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
        // A key let go without its tap (the finger slid off) leaves the typed handle as it was: its preview gives way to the typed handle again, decided after the whole poll, since a lift reports the key's Unlight just before its Tap.
        let mut slid = false;
        for &t in &outs[..no] {
            match t {
                Touch::Light(key) => {
                    a.kb.draw_key(a.surf(), key, true, &mut a.label());
                    // The lit key on the glass first: preparing a whole board takes some 50 ms.
                    a.show();
                    a.preview(key);
                    a.prepare(key);
                }
                Touch::Unlight(key) => {
                    a.kb.draw_key(a.surf(), key, false, &mut a.label());
                    slid = true;
                }
                Touch::Scrolled => {
                    a.show_board();
                }
                Touch::Tap(key) => {
                    a.tap(key);
                    slid = false;
                }
                Touch::TapAt(x, y) => a.tap_at(x, y),
                // A drag on the entry line pans it (TEXT.md); elsewhere nothing yet.
                Touch::Drag(x0, y0, x, _) => {
                    a.drag_text(x0, y0, x);
                }
                // A handle is exact bytes, typed key by key: no shape writing here.
                Touch::Trail(..) | Touch::Traced => {}
            }
        }
        if slid {
            publish_fresh_text(&a.handle[..a.len], true);
        }
        // Following the identity service's verdict while the identity is looked up or joined: about four times a second.
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
