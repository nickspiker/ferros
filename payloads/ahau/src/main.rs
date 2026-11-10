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
//! A first entry is not a claim (2026-10-28, after it let a person in under a handle that already had a fleet): the handle's roots go into the vault and the identity service (fgtw) looks the identity up, its fleet's devices nearby first and fgtw.org last (photon docs/pairing-v2.md, peers-are-fgtw.md). A fleet: this device joins it, its pairing words shown for one of the fleet's devices to add it. No fleet anywhere: the permanence warning, the handle again, and the client mints the genesis. Nothing answering: nothing is decided, and the screen waits while the client keeps looking. There is no way past the screen but a bind or a mint (owner, 2026-11-04, of the "go on" that let a person in mid-join: "I do not see a point. seems like another security nightmare"); `back` returns to the entry.
//!
//! Grant slots: 0 console, 1 display (use), 2 touch (use), 3 its surface, 4 the vault scope `ahau.`, 5 the vault scope `fgtw.`, 6 the ira key, 7 settings, 8 the identity service, fgtw (use), 9 the identity unlock right, 10 a second surface (the keyboard's back buffer).
#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::arch::global_asm;
use ferros_sys::{call, cap, console, exit, freq, now, sleep_until, window};
use honeycomb::{Event, Key, Keyboard, Touch, Touches};

/// Typing at the blinkey: `c` into the text bytes `t` (`n` of them, `chars` characters) at byte `at`, the bytes after it moved up. Nothing at `limit` characters (MAX_CHARS for a handle, WORD_CHARS for words), nor when the bytes would not fit the buffer. Returns where the blinkey goes: after `c`.
fn insert_at(t: &mut [u8; TEXT_BYTES], n: &mut usize, chars: &mut usize, at: usize, c: char, limit: usize) -> usize {
    let mut b = [0u8; 4];
    let e = c.encode_utf8(&mut b).as_bytes();
    // WHY the byte check: words allow WORD_CHARS characters and a character is up to four bytes, more than the buffer holds; past its end the copy below would panic. A handle (MAX_CHARS × 4 bytes) always fits.
    if *chars >= limit || *n + e.len() > TEXT_BYTES {
        return at;
    }
    t.copy_within(at..*n, at + e.len());
    t[at..at + e.len()].copy_from_slice(e);
    *n += e.len();
    *chars += 1;
    at + e.len()
}

/// Delete at the blinkey: the character before byte `at` out of the handle bytes, the bytes after it moved down and the freed tail wiped. Returns where the blinkey goes: where that character began.
fn delete_before(t: &mut [u8; TEXT_BYTES], n: &mut usize, chars: &mut usize, at: usize) -> usize {
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
/// A board as drawn untinted, its rows from `kb.top` to `kb.bottom`, the bar's foot at its lowest (owner, 2026-10-29: a mask and an index to fill): each pixel's grey (one channel: it is all grey), its key (`Keyboard::tint_index`; 0 off the keys, where the weight is 0), and how much of its key's tint it takes (Q14): whole on the key and its edge (at or below the key's grey), none on the label (at its grey, LABEL) and off the keys, in between across the label's anti-aliased edge, so the labels stay white.
/// 2²⁰ + 2¹⁸ pixels: the 1344-wide board is 788 rows since its foot follows the app's circle down to the screen's bottom (owner, 2026-11-07; 1059072 pixels, past 2²⁰), and this holds 975 of them.
const BOARD_PIXELS: usize = (1 << 20) + (1 << 18);
struct Board {
    base: [u16; BOARD_PIXELS],
    weight: [u16; BOARD_PIXELS],
    index: [u8; BOARD_PIXELS],
    /// Each pixel's see-through part, 0 opaque to 255 clear (the display's working transparency, Q14, in eighths of a byte): the keys' edges and the gaps between them, through which the words under the keyboard show (`words`).
    clear: [u8; BOARD_PIXELS],
}
const NO_BOARD: Board = Board { base: [0; BOARD_PIXELS], weight: [0; BOARD_PIXELS], index: [0; BOARD_PIXELS], clear: [0; BOARD_PIXELS] };
/// Two boards kept (owner, 2026-11-01: copy and tint): the one on show and the one a held key will leave, or the other case, so a shift, a letter or a lift is one tint pass and a copy, with no label drawn. A board missing is drawn once into the slot not on show (`Ahau::board`); `Ahau::boards` says which board each slot holds.
static mut BOARDS: [Board; 2] = [NO_BOARD; 2];
/// What a board's untinted pixels follow: its layout, its view, upper case or not, the bar's left key's label. The shift key's own look (off, once, lock) is drawn over it at every paint.
type BoardKey = (usize, usize, bool, &'static str);

fn board_key(kb: &Keyboard) -> BoardKey {
    (kb.layout, kb.view, kb.shift != honeycomb::Shift::Off, kb.left)
}

/// The mark thread's work: the grants (for its console line), the radius and the salt, set before it starts; and its stack.
static mut FRESH_JOB: (u64, usize, [u8; 32]) = (0, 0, [0; 32]);
/// The device is bound to an identity: the mark is the attest button (tapuwae `Look::attest()`, `render_button_pack16`) of the handle as typed, not the free device's Mandelbrot (owner, 2026-11-04: the third of tapuwae's marks, designed in tools/attest-mark as "the device bonded to an identity" and never wired here). Drawn from the device's sealed salt and the typed text alone: the owner sees their own picture form as they type their handle, a swapped device or a fake screen cannot draw it, and nothing on the locked device says whose it is.
static mut BONDED: bool = false;
/// The last lift's stages in microseconds (retint, board, tint pass, present), for its console line while the lift's lag is looked at.
static mut LIFT_US: [u64; 4] = [0; 4];
/// Keys type at the press (owner, 2026-11-05: "try 2 and then maybe go to 1"): the handle, the keys' rainbow and the mark move at the press, and a finger sliding off types the key all the same. False: the mark renders a preview at the press and the lift types it (a slide-off then leaves the handle as it was).
const COMMIT_AT_PRESS: bool = true;

static mut FRESH_STACK: [u8; 1 << 16] = [0; 1 << 16];
/// The bonded button's helpers: three more threads on the worker cores, each drawing every fourth row of a pass (tapuwae `render_button_pack16`'s row share), the mark's thread the remaining share. The fresh mark redraws only its components, a quarter of the disc (159k of 630k pixels at 900 px), where the ball is 85 % of it (538k) at the same cost a sample, so one core that kept up with the fresh mark is about four short of the ball (1.0 to 1.4 s a pass alone, 2026-11-04).
const HELPERS: usize = 3;
static mut HELP_STACKS: [[u8; 1 << 16]; HELPERS] = [[0; 1 << 16]; HELPERS];
/// How many helpers run: spawned in order at the mark's start, stopping at the first refused, so their shares (1..=HELPERS_UP) are always contiguous.
static HELPERS_UP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// A pass for the helpers: HELP_JOB moves on once HELP_PASS holds its arguments; HELP_DONE counts the helpers through with it, HELP_STOPPED that one of them gave it up for a newer handle.
static HELP_JOB: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static HELP_DONE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static HELP_STOPPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The pass: the handle's placement (on the mark thread's stack, which outlives the pass, since that thread waits for every helper before it returns), fine (4 × 4) or not, its generation, the mark's side.
static mut HELP_PASS: (*const tapuwae::Params, bool, u64, usize) = (core::ptr::null(), false, 0, 0);
/// What the helpers' job is: 0 a pass (HELP_PASS), 1 J(c)'s address points for the handle's placement (HELP_EXTENT).
static HELP_KIND: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// The placement's c and its points (on the mark thread's stack, which waits for every helper before it goes on): each thread fills every (helpers + 1)-th point from its share.
static mut HELP_EXTENT: ((f64, f64), *mut [(f64, f64); tapuwae::EXTENT_POINTS]) = ((0., 0.), core::ptr::null_mut());
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

/// The identity service is asked from two threads of its own, never from the screen's (owner, 2026-11-10: "a blinking attest button and a frozen cursor when I hit attest"): a call waits as long as fgtw's pass does, and a pass took 3 s while it minted binding requests, so the screen froze with it. The map had the same fault the same day (its `fgtw_poller`).
/// The session's roots for op 4 (seed, proof), written by the screen before it moves ROOTS_GEN on; `roots_thread` hands each generation over and sets ROOTS_TAKEN to it once fgtw took it.
static mut ROOTS: [u8; 64] = [0; 64];
static ROOTS_GEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static ROOTS_TAKEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static mut ROOTS_STACK: [u8; 1 << 15] = [0; 1 << 15];
static mut CLIENT_STACK: [u8; 1 << 15] = [0; 1 << 15];

/// The identity service as `client_thread` last read it: its state byte (op 1), its pairing words while joining, this device's name in the fleet (op 19, while joining), and the roots generation fgtw held when it was read (a copy read before the latest roots says nothing about them).
#[derive(Clone, Copy)]
struct ClientCopy {
    roots: u64,
    state: u8,
    words: [u8; 512],
    words_len: usize,
    name: [u8; 48],
    name_len: usize,
}
/// The copy under a sequence count (odd while it is written; it moves only when the service rang).
static CLIENT_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static mut CLIENT_COPY: ClientCopy = ClientCopy { roots: 0, state: 0, words: [0; 512], words_len: 0, name: [0; 48], name_len: 0 };

/// Waits for fgtw to answer: it starts once the space is open and may restart, and no doorbell exists for a server coming up, so a refused call is made again after a wait that doubles from a sixty-fourth of a second to a second (the one timer on these threads, and why).
fn absent_wait(absent: &mut u64) {
    sleep_until(now() + *absent);
    if *absent < freq() {
        *absent <<= 1;
    }
}

/// Hands each new generation of the session's roots to fgtw (op 4), sleeping on ROOTS_GEN between, so the screen never waits on the hand-over.
extern "C" fn roots_thread(grants: u64) -> ! {
    use core::sync::atomic::Ordering;
    let svc = cap(grants, S_FGTW_EP);
    let mut absent = freq() >> 6;
    loop {
        let g = ROOTS_GEN.load(Ordering::Acquire);
        if g == ROOTS_TAKEN.load(Ordering::Acquire) {
            ferros_sys::wait_world(&ROOTS_GEN, g, 0);
            continue;
        }
        let mut m = [0u8; 65];
        m[0] = 4;
        m[1..].copy_from_slice(unsafe { &*(&raw const ROOTS) });
        // Written again while copied: the newer roots are the ones to hand.
        if ROOTS_GEN.load(Ordering::Acquire) != g {
            continue;
        }
        let mut out = [0u8; 16];
        let n = ferros_sys::call_live(svc, &m, &mut out);
        // Taken when the reply says so (its state byte first, 0 while it holds no identity). Any reply at all is not enough: from 2026-10-28 to 2026-11-04 fgtw ignored the message (it wanted 69 bytes), still replied, and the roots were counted as handed.
        if n == ferros_sys::ERR || n == ferros_sys::NOTHING || n < 1 || out[0] == 0 {
            absent_wait(&mut absent);
            continue;
        }
        absent = freq() >> 6;
        ROOTS_TAKEN.store(g, Ordering::Release);
        ferros_sys::wake(&ROOTS_TAKEN, 1);
    }
}

/// fgtw's doorbell (op 21), held from the first roots on: answered when what it watches changes (its state, the fleet, the space), and only then is op 1 (and op 19 while joining) read and the copy published. Nothing on a timer but `absent_wait`.
extern "C" fn client_thread(grants: u64) -> ! {
    use core::sync::atomic::Ordering;
    let svc = cap(grants, S_FGTW_EP);
    // Nothing to follow before a person typed a handle.
    while ROOTS_GEN.load(Ordering::Acquire) == 0 {
        ferros_sys::wait_world(&ROOTS_GEN, 0, 0);
    }
    let mut c = ClientCopy { roots: 0, state: 0, words: [0; 512], words_len: 0, name: [0; 48], name_len: 0 };
    // u32::MAX is never a count fgtw has given, so the first call is answered at once.
    let mut count = u32::MAX;
    let mut absent = freq() >> 6;
    loop {
        let mut m = [21u8, 0, 0, 0, 0];
        m[1..].copy_from_slice(&count.to_le_bytes());
        let mut r = [0u8; 4];
        // An fgtw without the doorbell answers op 21 as its state (a byte or two), never four bytes: treated as absent.
        if ferros_sys::call_live(svc, &m, &mut r) != 4 {
            absent_wait(&mut absent);
            continue;
        }
        absent = freq() >> 6;
        count = u32::from_le_bytes(r);
        c.roots = ROOTS_TAKEN.load(Ordering::Acquire);
        let mut out = [0u8; 600];
        let n = ferros_sys::call_live(svc, &[1], &mut out);
        if n == ferros_sys::ERR || n == ferros_sys::NOTHING || n < 2 {
            continue;
        }
        // WHY the mins: the reply's length is the service's word; the words are kept only as far as both buffers reach.
        let n = (n as usize).min(out.len());
        let k = (n - 2).min(c.words.len());
        c.state = out[0];
        c.words[..k].copy_from_slice(&out[2..2 + k]);
        c.words_len = k;
        c.name_len = 0;
        if c.state == 1 {
            let n = ferros_sys::call_live(svc, &[19], &mut c.name);
            if n != ferros_sys::ERR && (1..=48).contains(&(n as usize)) {
                c.name_len = n as usize;
            }
        }
        CLIENT_SEQ.fetch_add(1, Ordering::AcqRel);
        unsafe { *(&raw mut CLIENT_COPY) = c };
        CLIENT_SEQ.fetch_add(1, Ordering::AcqRel);
    }
}

/// The copy `client_thread` last published and its sequence count, read whole (again if it was being written); None when it has not moved from `seen`.
fn client_take(seen: u32) -> Option<(u32, ClientCopy)> {
    use core::sync::atomic::Ordering;
    loop {
        let s0 = CLIENT_SEQ.load(Ordering::Acquire);
        if s0 == seen {
            return None;
        }
        let v = unsafe { *(&raw const CLIENT_COPY) };
        if s0 & 1 == 0 && CLIENT_SEQ.load(Ordering::Acquire) == s0 {
            return Some((s0, v));
        }
    }
}
/// Grant slot 9: the identity unlock right (SYS_UNLOCK, AUTH.md): the person's space opens from the seed, before the proof.
const S_UNLOCK: u64 = 9;
/// Slot 10: a second surface, the keyboard's back buffer: a board that a key's tap will redraw (shift, a page, a layout) is drawn there whole while the finger is down and copied in at the lift (`prepare`), as the mark's frame is rendered ahead by its thread. Without it the lift redrew the board in place, 59 ms (2026-11-01: draw 14, the key map 37, the tint 8), and the shift showed a few frames late.
const S_BACK: u64 = 10;
/// Slots 11 and 12: the GPU service (`use gpu`) and the layer arena, where the mark's quick pass is drawn on the GPU (GPU.md, "The resident service, and the attest mark's path"). Without them (after a jump, no GPU service) the quick pass stays on the CPU.
const S_GPU: u64 = 11;
const S_LAYERS: u64 = 12;
/// The WiFi driver's `wlan` (init's slot 44): the network page's SCAN, JOIN and STATUS (pcie1, ATTEST-UI.md "WiFi connect").
const S_WLAN: u64 = 13;
/// The GPU's view of the layer arena (payloads/gpu ARENA_VA), and ahau's own region of it: the kernel's words at 32 MiB, the quick pass's pixels from 33 MiB (gputest uses the first 32).
const ARENA_VA: u64 = 0x8000_0000;
const GPU_CODE: usize = 32 << 20;
const GPU_OUT: usize = 33 << 20;
/// The GPU's second output, past the fine passes' sums (43 MiB, 16 bytes a staged pixel): the passes alternate between the two, so the screen shows one while the GPU writes the other.
const GPU_OUT2: usize = 54 << 20;
/// Where the mark is drawn from (`draw_mark`): 0 for FRESH (the CPU drew it), else the address of the GPU output the last pass landed in. No copy out of the arena (1.3 ms a pass for a 900 px mark, 2026-11-05), and no frame showing a pass half written.
static MARK_FRONT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// The GPU output the next pass writes: whichever of the two the screen is not showing.
fn gpu_back(arena: usize) -> usize {
    if MARK_FRONT.load(core::sync::atomic::Ordering::Acquire) == arena + GPU_OUT { GPU_OUT2 } else { GPU_OUT }
}

/// The surfaces through their handles (SANDBOX.md, "Handles"): the screen draws into `BUF`, a frame-sized buffer in its own arc, and presents the rows it touched through the shown surface's handle, so the same binary runs native and masked and the drawing code (honeycomb, tapuwae, the pills) never holds a pointer outside the arc. Text is the display's to draw, into a surface it knows: the rows go to that surface first (`put_rows`), the text is asked for, and the rows come back (`get_rows`) before anything more is drawn over them. The second surface stays a display surface for that, reached through its own handle. Labels the keyboard draws are recorded while it draws and asked for after the rows are presented, or the presenting would wipe them.
type Front = ferros_sys::Handle<S_SURF>;
type BackS = ferros_sys::Handle<S_BACK>;
static mut BUF: [u64; (32 << 20) / 8] = [0; (32 << 20) / 8];
/// A recorded label: x, baseline, size, value, the text and its length.
static mut LABELS: [(i64, i64, u16, u16, [u8; 32], usize); 256] = [(0, 0, 0, 0, [0; 32], 0); 256];
static mut NLABELS: usize = 0;
/// The longest handle the screen takes, in characters.
const MAX_CHARS: usize = 64;
/// The entry's buffer, in bytes: a handle (MAX_CHARS characters of up to four bytes) or a newcomer's 23 pairing words (WORD_CHARS).
const TEXT_BYTES: usize = 1 << 9;
/// The most characters of words typed for the identity service (its pairing words: 23 voca words of at most 18 letters with spaces between).
const WORD_CHARS: usize = (1 << 9) - (1 << 6);
/// Grey working values: the mark, the prompt, the status line (gamma 2, white = Q14).
const MARK: u64 = 9000;
const TEXT: u16 = 16384;
const QUIET: u16 = 7000;
/// A pill under a finger (the network page's rows and buttons): about twice the brightest pill at rest.
const PRESSED: u64 = 1 << 13;
/// The flat dark grey behind the network picked on the network page.
const SELECTED: u64 = 1 << 11;

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

/// The heap as it stands, and back to it: everything allocated between is dropped by then (the GPU compile, a keystroke's graph and words; a bump heap that never went back would run out after a dozen handles).
/// Who has the heap: 0 no one, 1 the mark's GPU compile (gpu_quick), 2 the handle proof. A mark and reset around the compile must not span the proof's buffer, and the proof's 24.9 MB must not find the compile's pieces in the way.
static HEAP_USE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// Whether the last pass was drawn on the GPU, for its console line.
static GPU_DREW: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn heap_mark() -> usize {
    unsafe { HEAP_NEXT }
}
fn heap_reset(m: usize) {
    unsafe { HEAP_NEXT = m };
}

/// ahau's region of the layer arena, past the quick pass's pixels (GPU_OUT, 6.5 MB): the circle's staged coordinates and the fine passes' sums.
const GPU_STAGE: usize = 40 << 20;
const GPU_SUMS: usize = 43 << 20;
/// How many pixels the stage holds for the size it was staged at (0: not staged yet), and that size.
static mut STAGED: (usize, usize) = (0, 0);

/// One kernel through the GPU service: compiled by `make` with the heap held (HEAP_USE) and given back after, its words into ahau's region of the arena, its uniforms `addrs` then the kernel's constants. False when there is no service receiving (after a jump, or still booting the GPU), the proof holds the heap, or the job did not finish.
/// The last GPU run's compile and job times (µs), for the mark's console line.
static GPU_COMPILE_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static GPU_JOB_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The last quick pass's other parts (µs): the handle's parameters, the stage (0 when it was already made), the whole GPU call.
static GPU_PARTS_US: [core::sync::atomic::AtomicU64; 3] = [const { core::sync::atomic::AtomicU64::new(0) }; 3];

fn gpu_run(grants: u64, arena: usize, addrs: &[u64], groups: u32, make: impl FnOnce() -> tapuwae::gpu::Compiled) -> bool {
    if HEAP_USE.compare_exchange(0, 1, core::sync::atomic::Ordering::Acquire, core::sync::atomic::Ordering::Relaxed).is_err() {
        return false;
    }
    let mark = heap_mark();
    let t0 = now();
    let k = make();
    GPU_COMPILE_US.store(ferros_sys::us_since(t0), core::sync::atomic::Ordering::Relaxed);
    for (i, &w) in k.words.iter().enumerate() {
        unsafe { core::ptr::write_volatile((arena + GPU_CODE + 8 * i) as *mut u64, w) };
    }
    // [1][preload u16, bit 15: 64 registers][code offset u32][words u16][groups x u32][groups y u32][n u8][n × u64].
    let mut msg = [0u8; 18 + 8 * 16];
    msg[0] = 1;
    msg[1..3].copy_from_slice(&(0x1000u16 | if k.regs64 { 0x8000 } else { 0 }).to_le_bytes());
    msg[3..7].copy_from_slice(&(GPU_CODE as u32).to_le_bytes());
    msg[7..9].copy_from_slice(&(k.words.len() as u16).to_le_bytes());
    msg[9..13].copy_from_slice(&groups.to_le_bytes());
    msg[13..17].copy_from_slice(&1u32.to_le_bytes());
    let nu = addrs.len() + k.consts.len();
    msg[17] = nu as u8;
    for (i, a) in addrs.iter().chain(k.consts.iter()).enumerate() {
        msg[18 + 8 * i..26 + 8 * i].copy_from_slice(&a.to_le_bytes());
    }
    drop(k);
    heap_reset(mark);
    HEAP_USE.store(0, core::sync::atomic::Ordering::Release);
    let mut reply = [1u8, 0, 0, 0, 0];
    let n = ferros_sys::call_live(cap(grants, S_GPU), &msg[..18 + 8 * nu], &mut reply);
    GPU_JOB_US.store(u32::from_le_bytes([reply[1], reply[2], reply[3], reply[4]]) as u64, core::sync::atomic::Ordering::Relaxed);
    n != ferros_sys::ERR && reply[0] == 0
}

/// The circle's stage for radius `r` in ahau's region of the arena, made once per size: every pixel the passes draw (tapuwae::gpu::button_stage); the frame zeroed with it, since no pass writes outside the circle. Returns how many pixels it holds.
fn ensure_stage(arena: usize, r: usize) -> usize {
    if unsafe { STAGED.1 } != r {
        let side = 2 * r + 4;
        // Both outputs zeroed (no pass writes outside the circle); the screen back on FRESH until a pass lands at this size.
        MARK_FRONT.store(0, core::sync::atomic::Ordering::Release);
        for out in [GPU_OUT, GPU_OUT2] {
            for i in 0..side * side {
                unsafe { core::ptr::write_volatile((arena + out + 8 * i) as *mut u64, 0) };
            }
        }
        let mut n = 0usize;
        tapuwae::gpu::button_stage(r as f32, |i| {
            unsafe { core::ptr::write_volatile((arena + GPU_STAGE + 4 * n) as *mut u32, i) };
            n += 1;
        });
        unsafe { STAGED = (n, r) };
    }
    unsafe { STAGED.0 }
}

/// The layer arena as ahau sees it, if granted.
fn arena(grants: u64) -> Option<usize> {
    let a = ferros_sys::dma(cap(grants, S_LAYERS)) as usize;
    if a == 0 || a == usize::MAX { None } else { Some(a) }
}

/// The bonded button's quick pass on the GPU: tapuwae's own shading recorded through valhall (tapuwae::gpu), run by the GPU service into the output of ahau's region of the layer arena the screen is not showing (side x side pack16 words, as render_button_pack16 writes them), which it shows from then (MARK_FRONT). False when the GPU did not draw it: the CPU draws it then.
fn gpu_quick(grants: u64, r: usize, p: &tapuwae::Params) -> bool {
    let Some(arena) = arena(grants) else { return false };
    // Over the circle's stage only: nothing outside it is computed (~22 % of the square).
    let t = now();
    let staged = ensure_stage(arena, r);
    GPU_PARTS_US[1].store(ferros_sys::us_since(t), core::sync::atomic::Ordering::Relaxed);
    let t = now();
    let back = gpu_back(arena);
    if !gpu_run(grants, arena, &[ARENA_VA + back as u64, ARENA_VA + GPU_STAGE as u64], staged.div_ceil(16) as u32, || tapuwae::gpu::button_quick_staged_kernel(r as f32, p, &tapuwae::Look::attest())) {
        return false;
    }
    GPU_PARTS_US[2].store(ferros_sys::us_since(t), core::sync::atomic::Ordering::Relaxed);
    MARK_FRONT.store(arena + back, core::sync::atomic::Ordering::Release);
    true
}

/// The handle's placement (tapuwae `Params::new_placed`), J(c)'s address points (half its work) shared with the helpers when they run: each thread fills every (helpers + 1)-th point, this one share 0, then it waits for them all. The same points in the same slots, so the same placement.
fn placed(salt: &[u8; 32], text: &str) -> tapuwae::Params {
    let up = HELPERS_UP.load(core::sync::atomic::Ordering::Acquire);
    if up == 0 {
        return tapuwae::Params::new_placed(salt, text, true);
    }
    tapuwae::Params::new_placed_with(salt, text, true, &mut |c, pts| {
        let at: *mut [(f64, f64); tapuwae::EXTENT_POINTS] = pts;
        HELP_DONE.store(0, core::sync::atomic::Ordering::Relaxed);
        unsafe { HELP_EXTENT = (c, at) };
        HELP_KIND.store(1, core::sync::atomic::Ordering::Release);
        HELP_JOB.fetch_add(1, core::sync::atomic::Ordering::Release);
        ferros_sys::wake(&HELP_JOB, up);
        for i in (0..tapuwae::EXTENT_POINTS).step_by(1 + up as usize) {
            unsafe { (*at)[i] = tapuwae::extent_point(c, i as u32) };
        }
        loop {
            let d = HELP_DONE.load(core::sync::atomic::Ordering::Acquire);
            if d >= up {
                break;
            }
            ferros_sys::wait(&HELP_DONE, d, 0);
        }
    })
}

/// The bonded button's 4 x 4 pass on the GPU as 16 distinct passes over the circle's staged coordinates (owner, 2026-11-05; tapuwae::gpu::button_fine_kernel): each pass one more sample a pixel, its frame published as it stands, so the mark sharpens pass by pass; a newer handle leaves between passes (each ~16 ms). After pass 15 the pixels are the CPU's 4 x 4. False when it stopped for a newer handle or the GPU could not run a pass.
fn gpu_fine_passes(g: u64, frames: &mut u64) -> bool {
    let (grants, r, salt) = unsafe { FRESH_JOB };
    let side = 2 * r + 4;
    let Some(arena) = arena(grants) else { return false };
    let staged = ensure_stage(arena, r);
    let mut text = [0u8; 4 * MAX_CHARS];
    let n = with_fresh_text(|t, n| {
        text[..n].copy_from_slice(&t[..n]);
        n
    });
    let p = placed(&salt, core::str::from_utf8(&text[..n]).unwrap_or(""));
    for b in text.iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    let t0 = now();
    for m in 0..16u32 {
        if FRESH_GEN.load(core::sync::atomic::Ordering::Acquire) != g {
            return false;
        }
        let back = gpu_back(arena);
        let addrs = [ARENA_VA + back as u64, ARENA_VA + GPU_STAGE as u64, ARENA_VA + GPU_SUMS as u64];
        if !gpu_run(grants, arena, &addrs, staged.div_ceil(16) as u32, || tapuwae::gpu::button_fine_kernel(r as f32, &p, &tapuwae::Look::attest(), m)) {
            return false;
        }
        MARK_FRONT.store(arena + back, core::sync::atomic::Ordering::Release);
        *frames += 1;
        FRESH_DONE_GEN.store(g, core::sync::atomic::Ordering::Release);
        FRESH_DONE.store(*frames, core::sync::atomic::Ordering::Release);
    }
    let mut l = Line::new();
    let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: attest mark cleared up ({side} px, 16 passes) in {} ms on the GPU\n", ferros_sys::us_since(t0) / 1000));
    console(grants, l.as_bytes());
    true
}

/// What the screen is waiting for.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// No identity in hand: the entry.
    Fresh,
    /// The roots of this seed are in the vault; the identity service (fgtw) is looking the identity up.
    Looking([u8; 32]),
    /// The identity has a fleet: one of its devices adds this one with the words shown (or the tap); the screen closes when the fleet binds it.
    Join([u8; 32]),
    /// No fleet holds the handle: the permanence warning, and the handle again to mint.
    Mint([u8; 32]),
    /// No fleet device and no fgtw.org answer: nothing decided, the client keeps looking.
    Grey([u8; 32]),
    /// fgtw.org's chain for the handle is another identity's: enter starts over.
    Wrong,
    /// fgtw.org refused the binding request: this device is in another identity's fleet (fgtw verdict 9). Enter starts over.
    Owned,
    /// After attest, the keyboard for words the identity service wants typed (the Fleet page, fgtw ops 15 and 16): 1 a newcomer's pairing words, 2 a leaver's approval words. Nothing of the handle is in it.
    Words(u8),
    /// After attest, waiting for words to be wanted, the window gone.
    Hosting,
    /// The network page over the attest screen (ATTEST-UI.md "First use", 2: pulled down from the mark): the networks the WiFi chip sees; the mode under it waits in `under`.
    Networks,
    /// The network page: the passphrase for the network picked, typed on the keyboard (it never feeds the mark or the keys' rainbow).
    Pass,
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
    /// The roots are set but the client has not taken them yet (`send_roots`): the generation published to `roots_thread` (ROOTS_GEN), which ROOTS_TAKEN reaches once fgtw took them.
    roots_pending: bool,
    roots_gen: u64,
    /// `client_thread`'s copy last taken (its sequence count).
    client_seen: u32,
    handle: [u8; TEXT_BYTES],
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
    /// The UI scale the text and the buttons follow, 64ths of an octave (preference 14, as the map's pages; owner, 2026-11-09: "make it so the two finger zoom changes the button and text size like Photon"): set live by a pinch above the keyboard, kept as the device's at the lift.
    scale: core::cell::Cell<f32>,
    /// The last status line, drawn again at a new scale.
    status_text: core::cell::Cell<([u8; 256], usize)>,
    /// The surface text goes into: the shown one, or the back one while a band is composed there (`compose`).
    text_to: core::cell::Cell<u64>,
    /// The network page: the mode under it, the handle as it stood (put back when the page closes), the last SCAN reply, the network picked (name, length), and a JOIN sent whose link has not come.
    under: Mode,
    stash: ([u8; TEXT_BYTES], usize, usize, usize),
    nets: [u8; 1 << 11],
    nets_len: usize,
    pick: ([u8; 32], usize),
    /// The filter as typed when a network was picked (bytes, length, characters), back in the text box on `back`.
    filter: ([u8; 128], usize, usize),
    joining: bool,
    /// The network page's element under a finger, lit while it is held (0 the button, 1 `back`, 16 + k the list's row k): it acts when the finger lifts still on it (`net_release`), however it moved meanwhile, so a tap that wandered is not taken for a drag.
    pressed: core::cell::Cell<Option<u8>>,
    /// The status's line count as last drawn (`status`), the join step's band below it (`words_band`): whether it holds the words, its scroll in pixels, the drag that moves it (landing row, scroll then), its last three frames' movements, the fling, its fraction of a pixel, the scroll's end, the panel frame last stepped; and this device's name in the fleet (fgtw op 19).
    status_lines: core::cell::Cell<usize>,
    words_shown: core::cell::Cell<bool>,
    words_scroll: core::cell::Cell<i64>,
    words_grab: core::cell::Cell<Option<(i64, i64)>>,
    words_dys: core::cell::Cell<[i64; 3]>,
    words_fling: core::cell::Cell<f64>,
    words_fling_res: core::cell::Cell<f64>,
    words_max: core::cell::Cell<i64>,
    words_frame: core::cell::Cell<Option<u32>>,
    dev_name: ([u8; 48], usize),
}

impl Ahau {
    /// The scale's factor on a size: 2^(s/64).
    fn k(&self, v: usize) -> usize {
        (v as f32 * libm::exp2f(self.scale.get() * (1. / (1 << 6) as f32))) as usize
    }

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
        let x = self.centre_x(self.w as i64 / 2, px, t.as_bytes());
        window::text(self.display, self.text_to.get(), x as i16, baseline as i16, px as u16, value, t.as_bytes());
    }

    /// Where text starts so it is centred on `cx`: its width as the display's typeface sets it (op 28, `measure`, in quarter pixels). A width guessed from the character count (0.56 em each) put proportional lines off centre (owner, 2026-11-04).
    fn centre_x(&self, cx: i64, px: usize, t: &[u8]) -> i64 {
        cx - self.measure(px, t) / 8
    }

    /// Rows y0..y1 composed whole before they show (owner, 2026-11-09: "Flickers to black a lot"): the buffer's rows (cleared or with their shapes) put on the back surface, `text` draws there (the display lays each glyph in front, what is under by the room left), the rows taken back and presented once. Drawn in place, a frame caught them cleared or a button bare.
    fn compose(&self, y0: usize, y1: usize, text: impl FnOnce()) {
        let tb = if self.back != 0 { self.back } else { self.base };
        self.put_rows(tb, y0, y1);
        self.text_to.set(tb);
        text();
        self.text_to.set(self.base);
        self.get_rows(tb, y0, y1);
        self.present(y0, y1);
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
        // On the network page the status sits just over the buttons (its three lines' rows and a sixty-fourth of the width above them), and the list runs down to it.
        if self.net_open() {
            let px = self.k(self.w / 34);
            return (self.button_top() - px * 31 / 10 - self.w / 64, px);
        }
        let (_, cy, r) = self.mark_at();
        (cy + r + self.w / 16, self.k(self.w / 34))
    }


    /// A tap on no key: on the entry line, the blinkey moves there. (The buttons act from their own press, `net_press`.)
    fn tap_at(&mut self, x: i64, y: i64) {
        // On the entry line: the blinkey to the nearest character boundary.
        let (b, px) = self.entry_at();
        if self.typing() && ((b - px * 3 / 2) as i64..(b + px / 2) as i64).contains(&y) {
            self.place_caret(x);
            return;
        }
        // The buttons and the network page's rows act from their own press (`net_press`, `net_release`), not from a tap.
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

    /// Whether the screen takes a handle now: the first entry, and the mint's second. Elsewhere (looking, the words, no answer, wrong) there is no text box and keys type nothing.
    fn typing(&self) -> bool {
        matches!(self.mode, Mode::Fresh | Mode::Mint(_) | Mode::Words(_) | Mode::Networks | Mode::Pass)
    }

    /// Whether a step back to the entry exists: from any step after a first entry (owner, 2026-10-28: "we need a way to go back to the previous screen").
    fn can_go_back(&self) -> bool {
        matches!(self.mode, Mode::Looking(_) | Mode::Join(_) | Mode::Mint(_) | Mode::Grey(_) | Mode::Wrong | Mode::Owned | Mode::Networks | Mode::Pass)
    }

    /// The button under the entry line: its rectangle (x0, y0, x1, y1). With a step back, it moves right and `back` takes the left.
    fn button_at(&self) -> (usize, usize, usize, usize) {
        let y0 = self.button_top();
        // The height, the label and the width follow the scale, about the centre (at twice, w/8 to 7w/8). There is no `back` button: the bottom-left corner is back (owner, 2026-11-09: "Never show a back button").
        let hgt = self.k(self.w / 10);
        let half = self.k(self.w * 3 / 16);
        (self.w / 2 - half, y0, self.w / 2 + half, y0 + hgt)
    }

    /// Back to the entry: the pending roots stay (the client keeps at them); the same handle resumes, another replaces them.
    fn go_back(&mut self) {
        // On the network page: from the passphrase back to the list, from the list back to the screen under it.
        match self.mode {
            Mode::Pass => return self.net_list(),
            Mode::Networks => return self.net_close(),
            _ => {}
        }
        self.mode = Mode::Fresh;
        self.status(self.prompt());
        self.words();
        self.entry();
        self.button();
    }

    /// What the button does now, or nothing while the client is asked.
    fn button_label(&self) -> Option<&'static str> {
        match self.mode {
            // Always "attest", bound or not (owner, 2026-11-10: "needs to say attest, not unlock. always attest"): the prompt above says whether the handle also binds this device.
            Mode::Fresh => Some("attest"),
            Mode::Mint(_) => Some("mint"),
            Mode::Wrong | Mode::Owned => Some("start over"),
            Mode::Words(_) => Some("cancel"),
            Mode::Networks if !self.joining => Some("scan"),
            Mode::Networks if self.joining => Some("joining"),
            // Only a network this device can join (WPA2-PSK outside 6 GHz) gets a join.
            Mode::Pass if self.net_found().is_some_and(|f| f.1 & 1 != 0) => Some("join"),
            Mode::Pass => None,
            Mode::Looking(_) | Mode::Join(_) | Mode::Grey(_) | Mode::Hosting | Mode::Networks => None,
        }
    }

    fn button(&self) {
        // While joining there is no button, and its row is the words' band's (`words`): cleared here it cut a black band through them.
        if matches!(self.mode, Mode::Join(_)) {
            return;
        }
        // The whole row first, so a button that moved or went leaves nothing behind.
        let (_, ry0, _, ry1) = self.button_at();
        let s = self.surf();
        for y in ry0..ry1 {
            s[y * self.w..(y + 1) * self.w].fill(0);
        }
        let (x0, y0, x1, y1) = self.button_at();
        if self.button_label().is_some() {
            pill(s, self.w, x0, y0, x1, y1, if self.pressed.get() == Some(0) { PRESSED } else if self.joining { 1800 } else { 2600 }, self.k(self.w / 24));
        }
        // The labels in front of the pills, the row composed whole before it shows (`compose`).
        self.compose(ry0, ry1, || {
        if let Some(t) = self.button_label() {
            let px = self.k(self.w / 24);
            let x = self.centre_x((x0 + x1) as i64 / 2, px, t.as_bytes());
            window::text(self.display, self.text_to.get(), x as i16, ((y0 + y1) / 2 + px * 35 / 100) as i16, px as u16, TEXT, t.as_bytes());
        }
        });
    }

    /// The entry's baseline and size: above the button, an entry size over its top, so a larger scale grows them upward, never into the keyboard (2026-11-09: grown downward, a scaled-up button's foot ran into the keyboard's rows and stayed there as a line).
    fn entry_at(&self) -> (usize, usize) {
        let px = self.k(self.w / 16);
        // On the network page the text box is at the top: the filter, then the passphrase (owner, 2026-11-09).
        if self.net_open() {
            return (self.net_top() + px * 3 / 2, px);
        }
        (self.button_top() - px, px)
    }

    /// The button row's top: its foot a fixed 0.15 of the width above the keyboard (where it stood at scale one), its height the scale's.
    fn button_top(&self) -> usize {
        self.kb.top - self.w * 3 / 20 - self.k(self.w / 10)
    }

    /// `t` broken into lines no wider than `width` pixels at `px`: at each newline, and between words where the next would not fit (a word wider than the whole width keeps a line to itself). The lines as byte ranges into `out`; their count.
    fn wrap(&self, px: usize, t: &[u8], width: i64, out: &mut [(usize, usize)]) -> usize {
        let mut n = 0;
        let mut para = 0;
        while para <= t.len() {
            let pend = t[para..].iter().position(|&c| c == b'\n').map_or(t.len(), |k| para + k);
            let mut start = para;
            loop {
                // The longest run of whole words from `start` that fits; the first word always goes in.
                let (mut end, mut j) = (start, start);
                while j < pend {
                    let next = t[j..pend].iter().position(|&c| c == b' ').map_or(pend, |k| j + k);
                    if end > start && self.measure(px, &t[start..next]) / 4 > width {
                        break;
                    }
                    end = next;
                    j = next + 1;
                }
                // WHY the test: `out` is a fixed array the caller sizes for its text; lines past it are not drawn rather than written past its end.
                if n < out.len() {
                    out[n] = (start, end);
                    n += 1;
                }
                if end >= pend {
                    break;
                }
                start = end + 1;
            }
            para = pend + 1;
        }
        n
    }

    /// The status: its lines (a newline breaks one) wrapped to the screen's width, centred (owner, 2026-11-09: "it should word wrap ... not get a nasty black band and disappear"). The rows the last status took are cleared too, so a shorter one leaves nothing of a longer.
    fn status(&self, t: &str) {
        let mut keep = ([0u8; 256], t.len().min(256));
        keep.0[..keep.1].copy_from_slice(&t.as_bytes()[..keep.1]);
        self.status_text.set(keep);
        let (b, px) = self.status_at();
        let mut lines = [(0usize, 0usize); 32];
        let n = self.wrap(px, t.as_bytes(), (self.w * 13 / 16) as i64, &mut lines);
        let rows = n.max(self.status_lines.get()).max(1);
        self.status_lines.set(n);
        let (y0, y1) = (b - px * 3 / 2, b + (rows - 1) * px * 13 / 10 + px / 2);
        self.surf()[y0 * self.w..y1 * self.w].fill(0);
        self.compose(y0, y1, || {
            for (k, &(s, e)) in lines[..n].iter().enumerate() {
                self.centred(b + k * px * 13 / 10, px, QUIET, core::str::from_utf8(&t.as_bytes()[s..e]).unwrap_or(""));
            }
        });
    }

    /// The join step's band: from under the status's last line down to the button row (the entry line, hidden while joining, is inside it).
    fn words_band(&self) -> (usize, usize) {
        let (b, px) = self.status_at();
        let y0 = b + self.status_lines.get().max(1).saturating_sub(1) * px * 13 / 10 + px / 2 + self.w / 32;
        // Down to the button row when there is a button; the join step has none, and its words run on under the keyboard to its foot (owner, 2026-11-09: "literally let you scroll it under the keyboard"), when its board is kept (`show_board`: one too big is drawn straight onto the glass, with no transparency kept to put them under).
        let (top, bottom) = self.kb_rows();
        let y1 = if self.button_label().is_some() {
            self.button_top() - self.w / 64
        } else if (bottom - top) * self.w <= BOARD_PIXELS {
            bottom
        } else {
            top - self.w / 64
        };
        // WHY: at a large scale the status can reach the buttons; the band is then empty, not inverted.
        (y0, y1.max(y0))
    }

    /// While joining: this device's name (the one the approving device shows for its request), then the pairing words, split at the capitals they come camelCased with, wrapped to the width. The band scrolls by the pixel with the finger and flings on as the settings pages do (`scroll_words`, `release_words`, `step_words`); its top and bottom edges fade out the text cut there, the fade's depth growing smoothly with what is cut. Otherwise the band is cleared once, when it held them.
    fn words(&self) {
        let px = self.k(self.w / 30);
        let (y0, y1) = self.words_band();
        if !matches!(self.mode, Mode::Join(_)) {
            if self.words_shown.replace(false) {
                self.surf()[y0 * self.w..y1 * self.w].fill(0);
                self.present(y0, y1);
                // The words ran under the keyboard: it is laid again without them.
                self.show_board();
            }
            return;
        }
        self.words_shown.set(true);
        self.surf()[y0 * self.w..y1 * self.w].fill(0);
        let mut t = [0u8; 640];
        let mut n = 0;
        let mut push = |b: &[u8], n: &mut usize| {
            for &c in b {
                // WHY: `t` holds the name (48) and the words (512) with their spaces; anything past it is dropped, not written past its end.
                if *n < t.len() {
                    t[*n] = c;
                    *n += 1;
                }
            }
        };
        if self.dev_name.1 > 0 {
            push(b"This device: ", &mut n);
            // The name comes camelCased (two voca words, as the words do): a space before each capital after the first.
            for (i, &c) in self.dev_name.0[..self.dev_name.1].iter().enumerate() {
                if c.is_ascii_uppercase() && i > 0 {
                    push(b" ", &mut n);
                }
                push(&[c], &mut n);
            }
            push(b"\n", &mut n);
        }
        let head = n;
        for (i, &c) in self.word_buf[..self.word_len].iter().enumerate() {
            if c.is_ascii_uppercase() && i > 0 {
                push(b" ", &mut n);
            }
            push(&[c], &mut n);
        }
        let mut lines = [(0usize, 0usize); 64];
        let nl = self.wrap(px, &t[..n], (self.w * 13 / 16) as i64, &mut lines);
        let pitch = px * 3 / 2;
        // WHY the bounds: the scroll reaches from the first line to where the last sits at the band's foot; past either end the band would show nothing, and text shorter than the band does not scroll.
        // What can be read is above the keyboard: the scroll ends where the last line clears its top.
        let seen = if y1 > self.kb.top { self.kb.top } else { y1 };
        let max = ((nl * pitch + px / 2) as i64 - (seen - y0) as i64).max(0);
        self.words_max.set(max);
        let scroll = self.words_scroll.get().min(max).max(0);
        self.words_scroll.set(scroll);
        // As `compose`, with the edges faded between the text and the glass.
        let tb = if self.back != 0 { self.back } else { self.base };
        self.put_rows(tb, y0, y1);
        for (k, &(s, e)) in lines[..nl].iter().enumerate() {
            let base = (y0 + px * 5 / 4 + k * pitch) as i64 - scroll;
            if base + (px / 2) as i64 <= y0 as i64 || base - (px * 3 / 2) as i64 >= y1 as i64 {
                continue;
            }
            let v = if s < head { QUIET } else { TEXT };
            let x = self.centre_x(self.w as i64 / 2, px, &t[s..e]);
            window::text(self.display, tb, x as i16, base as i16, px as u16, v, &t[s..e]);
        }
        self.get_rows(tb, y0, y1);
        // The keyboard's rows front to back: each key pixel first, the words under it by what it leaves see-through (the display's `under_pm`: the words' channels by that transparency added), so a key's inside hides them, its edge lets part through, a gap shows them whole. The result is opaque.
        if y1 > self.kb.top {
            let (top, w) = (self.kb.top, self.w);
            let b = unsafe { &(*(&raw const BOARDS))[self.shown_board.get()] };
            let tint = self.kb.tint.unwrap_or([[1 << 14; 3]; 64]);
            let s = self.surf();
            for i in 0..(y1 - top) * w {
                let k = board_px(b, &tint, i);
                let tr = b.clear[i] as u64;
                let p = &mut s[top * w + i];
                *p = if tr == 0 {
                    k
                } else {
                    let ch = |sh: u32| ((k >> sh & 0xFFFF) + ((*p >> sh & 0xFFFF) * tr >> 8)) << sh;
                    ch(0) | ch(16) | ch(32)
                };
            }
            // The shift key as it stands (off, once, lock), as `lay_board` draws it.
            self.kb.draw_key(s, Key::Bar(honeycomb::B_SHIFT), false, &mut |_, _, _, _, _| {});
        }
        // Each edge fades over up to an em: its depth em·tanh(cut/em), where `cut` is the text out of sight past that edge (0 at rest at the top: no fade there), so the fade comes and goes with every derivative continuous. Across it, the entry line's C∞ step, exp(−1/u) against its mirror.
        let fade = |u: f32| {
            let e = |u: f32| if u > 0. { libm::expf(-1. / u) } else { 0. };
            e(u) / (e(u) + e(1. - u))
        };
        let em = px as f32;
        let depth = |cut: i64| em * libm::tanhf(cut as f32 / em);
        let s = self.surf();
        // Only the top: below, the words go under the keyboard.
        for (d, top) in [(depth(scroll), true)] {
            if d < 1. {
                continue;
            }
            for r in 0..(d as usize).min(y1 - y0) {
                let y = if top { y0 + r } else { y1 - 1 - r };
                let k = (libm::sqrtf(fade((r as f32 + 0.5) / d)) * (1u32 << 14) as f32) as u64;
                for p in &mut s[y * self.w..(y + 1) * self.w] {
                    let ch = |v: u64| (v & 0xFFFF) * k >> 14;
                    *p = ch(*p) | ch(*p >> 16) << 16 | ch(*p >> 32) << 32;
                }
            }
        }
        self.present(y0, y1);
    }

    /// A drag on the join step's band: the text follows the finger by the pixel (the scroll at the drag's landing plus how far the finger has come up since), each frame's movement kept for the fling.
    fn scroll_words(&self, y0: i64, y: i64) {
        let start = match self.words_grab.get() {
            Some((g, s)) if g == y0 => s,
            _ => {
                let s = self.words_scroll.get();
                self.words_grab.set(Some((y0, s)));
                self.words_dys.set([0; 3]);
                self.words_fling.set(0.);
                s
            }
        };
        let before = self.words_scroll.get();
        self.words_scroll.set(start + (y0 - y));
        self.words();
        let d = self.words_scroll.get() - before;
        let dys = self.words_dys.get();
        self.words_dys.set([dys[1], dys[2], d]);
    }

    /// The finger lifted from the band: it goes on the way it last went at the largest of its last three frames' movements (the settings pages' `release_settings`).
    fn release_words(&self) {
        if self.words_grab.take().is_none() {
            return;
        }
        let dys = self.words_dys.get();
        let m = dys.iter().map(|d| d.abs()).max().unwrap_or(0);
        let last = dys.iter().rev().find(|&&d| d != 0).copied().unwrap_or(0);
        self.words_fling.set((last.signum() * m) as f64);
        self.words_frame.set(None);
    }

    /// One display frame of the fling: the settings pages' friction (`coast`: a pixel a frame per frame, at rest under one), stopped by the band's ends. Whether it still moves.
    fn step_words(&self) -> bool {
        let v = self.words_fling.get();
        if v == 0. {
            return false;
        }
        let (d, v) = (v, if v.abs() <= 1. { 0. } else { v - v.signum() });
        let res = self.words_fling_res.get() + d;
        let whole = libm::trunc(res);
        self.words_fling_res.set(res - whole);
        let s = self.words_scroll.get() + whole as i64;
        let max = self.words_max.get();
        // WHY: the band's ends stop the fling; past them there is nothing to show.
        self.words_fling.set(if s < 0 || s > max { 0. } else { v });
        self.words_scroll.set(s);
        self.words();
        self.words_fling.get() != 0.
    }

    /// While the identity is being looked up or joined: follow the client, from `client_thread`'s copy (nothing here calls fgtw). Returns whether the screen changed.
    fn follow_client(&mut self) -> bool {
        let seed = match self.mode {
            Mode::Looking(s) | Mode::Join(s) | Mode::Grey(s) => s,
            _ => return false,
        };
        if self.roots_pending {
            if ROOTS_TAKEN.load(core::sync::atomic::Ordering::Acquire) != self.roots_gen {
                return false;
            }
            self.roots_pending = false;
            console(self.grants, b"ahau: roots handed to fgtw; it looks the identity up\n");
        }
        let Some((seq, c)) = client_take(self.client_seen) else { return false };
        self.client_seen = seq;
        // A copy read before fgtw took these roots is about the last ones.
        if c.roots != self.roots_gen {
            return false;
        }
        // The words redrawn only when the identity service has new ones (a fresh binding request): redrawn every poll, the words screen flickered four times a second (owner, 2026-11-04).
        let same_words = self.word_buf[..self.word_len] == c.words[..c.words_len];
        self.word_buf = c.words;
        self.word_len = c.words_len;
        let next = match c.state {
            1 => Mode::Join(seed),
            2 | 3 => {
                console(self.grants, b"ahau: this device is a member of the identity's fleet\n");
                self.mark_member();
                self.attested();
            }
            5 => Mode::Mint(seed),
            6 => Mode::Wrong,
            7 => Mode::Grey(seed),
            9 => Mode::Owned,
            _ => return false,
        };
        if next == self.mode && (same_words || !matches!(next, Mode::Join(_))) {
            return false;
        }
        let changed = next != self.mode;
        self.mode = next;
        // This device's name in the fleet, for the join step (fgtw op 19: empty until the roots are in).
        if matches!(next, Mode::Join(_)) && self.dev_name.1 == 0 && c.name_len > 0 {
            self.dev_name = (c.name, c.name_len);
        }
        if changed {
            self.status(self.prompt());
            self.entry_line(false);
            self.button();
        }
        self.words();
        true
    }

    /// Hand the roots to the identity service (fgtw) in RAM (op 4): it takes them, keeps them on disk only if unlock-at-boot is on, and looks the identity up. Nothing is written here. The hand-over is `roots_thread`'s, which waits for fgtw however long it takes; here the roots are only published to it, so the screen never freezes on it.
    fn send_roots(&mut self, seed: &[u8; 32], proof: &[u8; 32]) {
        use core::sync::atomic::Ordering;
        self.session_seed = Some(*seed);
        self.session_proof = Some(*proof);
        let roots = unsafe { &mut *(&raw mut ROOTS) };
        roots[..32].copy_from_slice(seed);
        roots[32..].copy_from_slice(proof);
        self.roots_gen = ROOTS_GEN.fetch_add(1, Ordering::AcqRel) + 1;
        self.roots_pending = true;
        // Both threads: the roots' to hand them, the client's to start following.
        ferros_sys::wake(&ROOTS_GEN, 2);
    }

    /// The entry: the handle as typed and a cursor.
    fn entry(&self) {
        // The mark is the handle's: words typed for the identity service draw none, nor does a passphrase.
        if !matches!(self.mode, Mode::Words(_) | Mode::Networks | Mode::Pass) {
            publish_fresh_text(&self.handle[..self.len], true);
        }
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

    /// Photon's blinkey (fluor `paint::draw_blinkey`, owner 2026-11-01: "steal the vertical blinkey"): a caret `h` high from `top` at column `x`, its brightness a wave down its height peaking toward the top or the bottom (`wave_top`; each tick flips it), smeared sideways at half the brightness per step, in front of what is there (the letters and the line under it). The wave peaks at (8/9)(16/9) × 100 ≈ 158 of 255, Photon's CURSOR_BRIGHTNESS, here in the working values (white Q14). The smear scales with the text: fluor halves it every pixel over 7 at Photon's 16-20 px text, a hairline beside the entry's 84 px letters, so here a step is a thirty-second of the text's size `px` (a sixteenth read as a bloom, 2026-11-01), over 7 steps either side, composed in linear light.
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
                // Front to back: the caret is the front layer, white light at α = w²/WHITE² in linear light (the working values are gamma 2, so light is the square), into the empty accumulator first; the letters and the line behind it go under through what it leaves, WHITE² − w² (WHITE² is 2²⁸, a shift). Over black it is w; over white, white. Nothing comes out brighter than white or than what was there, so no clamp: the wave peaks at 27/16 × 100 (at t = −1/2 or 1/2), so w ≤ 10800 < WHITE and the term left is never negative.
                let (front, left) = (w * w, honeycomb::WHITE * honeycomb::WHITE - w * w);
                let under = |c: u64| (front + (c * c * left >> 28)).isqrt();
                s[i] = under(p & 0xFFFF) | under(p >> 16 & 0xFFFF) << 16 | under(p >> 32 & 0xFFFF) << 32;
            }
        }
    }

    /// The handle as the entry line shows it — a newline as ¶ (the typeface has no return arrow), so the handle stays on one line as typed — its length, and where the blinkey falls in it.
    fn shown(&self) -> ([u8; 2 * TEXT_BYTES], usize, usize) {
        let mut t = [0u8; 2 * TEXT_BYTES];
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
        // No entry while nothing is typed: the words screen and the others between entries have no text box (owner, 2026-11-04).
        if !self.typing() {
            // While joining, these rows belong to the words' band (`words`): cleared here they cut a black band through the words (owner, 2026-11-09).
            if !matches!(self.mode, Mode::Join(_)) {
                self.surf()[r0..r1].fill(0);
                self.present(r0 / self.w, r1 / self.w);
            }
            return;
        }
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
        // The blinkey where the caret is, while it is in the window: a drag can carry it out (typing brings it back, `chase`). In front of the letters, already in the rows it composes over. (No line under the handle: the owner, 2026-11-09, "nix the underline for the handle".)
        if (w0..w1).contains(&(x + caret_px)) {
            self.blinkey(s, (x + caret_px) as usize, b - px * 7 / 8, px * 17 / 16, px);
        }
        self.present(r0 / self.w, r1 / self.w);
    }

    /// Everything between the mark and the keyboard drawn again at the scale now (the status, the pairing words, the entry, the buttons), the rows cleared first so a smaller size leaves nothing of the larger.
    fn rescale(&self) {
        let (_, cy, r) = self.mark_at();
        let (y0, y1) = (cy + r + 2, self.kb.top);
        // The network page is laid out from top to buttons by the scale: drawn whole again.
        if self.net_open() {
            self.all();
            self.show();
            return;
        }
        // Cleared in the buffer only: each piece composes its own band and shows it whole, then the region is shown once more from the buffer, which now holds their text too, so what a smaller size no longer covers goes black with no frame of it blank in between.
        self.surf()[y0 * self.w..y1 * self.w].fill(0);
        let (t, n) = self.status_text.get();
        self.status(core::str::from_utf8(&t[..n]).unwrap_or(""));
        self.words();
        self.entry_line(true);
        self.button();
        self.present(y0, y1);
        self.show();
    }

    fn all(&self) {
        self.surf().fill(0);
        Front::fill64(window::PORCH, (self.w * self.h) as u64, 0);
        if self.net_open() {
            // The network page: its list where the mark was, its own status line.
            self.net_draw();
            self.net_status();
        } else {
            if !matches!(self.mode, Mode::Words(_)) {
                let (cx, cy, r) = self.mark_at();
                draw_mark(self.surf(), self.w, cx, cy, r);
                self.present(cy - r - 2, cy + r + 2);
            }
            self.status(self.prompt());
        }
        self.entry();
        self.button();
        self.show_board();
    }

    /// Whether the network page is up.
    fn net_open(&self) -> bool {
        matches!(self.mode, Mode::Networks | Mode::Pass)
    }

    /// The network page pulled down over the screen: the mode under it and the handle as typed are kept for its closing; one scan is asked for.
    fn net_pull(&mut self) {
        if self.net_open() || matches!(self.mode, Mode::Words(_) | Mode::Hosting) {
            return;
        }
        self.under = self.mode;
        self.stash = (self.handle, self.len, self.chars, self.caret);
        self.zero_text();
        self.mode = Mode::Networks;
        self.joining = false;
        self.nets_len = 0;
        self.net_scan();
        self.all();
    }

    /// The network page closed: the screen under it back as it was, the handle as typed.
    fn net_close(&mut self) {
        self.zero_text();
        (self.handle, self.len, self.chars, self.caret) = self.stash;
        for b in self.stash.0.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        self.mode = self.under;
        self.joining = false;
        self.all();
    }

    /// From the passphrase back to the list.
    fn net_list(&mut self) {
        self.zero_text();
        // The filter as it was when the network was picked.
        let (f, n, chars) = self.filter;
        self.handle[..n].copy_from_slice(&f[..n]);
        (self.len, self.chars, self.caret) = (n, chars, n);
        self.mode = Mode::Networks;
        self.all();
    }

    /// The text box emptied, its bytes overwritten (a passphrase never lingers in memory).
    fn zero_text(&mut self) {
        for b in self.handle.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        self.len = 0;
        self.chars = 0;
        self.caret = 0;
        self.pan.set(0);
    }

    /// The list's `k`th network from the last SCAN reply (pcie1: [scanning, count], then name length, name, dBm, flags): its name, signal and flags.
    fn net_entry(&self, k: usize) -> Option<(&[u8], i8, u8)> {
        // WHY `get`: the reply is another process's bytes (the driver's), its lengths unchecked here; a short or malformed one ends the list rather than indexing past the reply.
        let b = &self.nets[..self.nets_len];
        if b.len() < 2 || k >= b[1] as usize {
            return None;
        }
        let mut at = 2;
        for j in 0..=k {
            let n = *b.get(at)? as usize;
            let e = b.get(at + 1..at + 3 + n)?;
            if j == k {
                return Some((&e[..n], e[n] as i8, e[n + 1]));
            }
            at += 3 + n;
        }
        None
    }

    /// The page's top: below the console's circles, which run along the top of the glass over the mark's top three eighths. The text box sits here (`entry_at`), the list or the network picked under it (owner, 2026-11-09: "textbox needs to go up top to filter the networks").
    fn net_top(&self) -> usize {
        let (_, cy, r) = self.mark_at();
        cy - r * 5 / 8
    }

    /// The list's rows: their size, pitch, the first row's baseline and the list's foot (the band's), under the text box.
    fn net_rows(&self) -> (usize, usize, usize, usize) {
        let (b, epx) = self.entry_at();
        let px = self.k(self.w / 24);
        let pitch = px * 2;
        (px, pitch, b + epx / 2 + px * 11 / 8, self.net_band().1)
    }

    /// The rows' band: from under the text box down to the status line's rows.
    fn net_band(&self) -> (usize, usize) {
        let (b, epx) = self.entry_at();
        let (sb, spx) = self.status_at();
        (b + epx / 2, sb - spx * 3 / 2)
    }

    /// Row `k`'s pill (x0, y0, x1, y1): three quarters of the pitch tall, centred on its text's middle (a third of an em above the baseline), across all but a sixteenth of the width each side.
    fn net_row_rect(&self, k: usize) -> (usize, usize, usize, usize) {
        let (px, pitch, first, _) = self.net_rows();
        let mid = first + k * pitch - px / 3;
        (self.w / 16, mid - pitch * 3 / 8, self.w * 15 / 16, mid + pitch * 3 / 8)
    }

    /// Whether `name` holds the filter as typed, letters matched either case (an empty filter holds every name).
    fn net_matches(&self, name: &[u8]) -> bool {
        let f = &self.handle[..self.len];
        f.is_empty() || name.windows(f.len()).any(|w| w.eq_ignore_ascii_case(f))
    }

    /// The `k`th network the filter lets through, as `net_entry` gives it.
    fn net_visible(&self, k: usize) -> Option<(&[u8], i8, u8)> {
        let mut seen = 0;
        let mut j = 0;
        while let Some(e) = self.net_entry(j) {
            if self.net_matches(e.0) {
                if seen == k {
                    return Some(e);
                }
                seen += 1;
            }
            j += 1;
        }
        None
    }

    /// The network picked, as the last scan has it (signal and flags), if it was seen.
    fn net_found(&self) -> Option<(i8, u8)> {
        let pick = &self.pick.0[..self.pick.1];
        let mut j = 0;
        while let Some((name, dbm, flags)) = self.net_entry(j) {
            if name == pick {
                return Some((dbm, flags));
            }
            j += 1;
        }
        None
    }

    /// How many rows are shown: the networks the filter lets through, as many as fit above the list's foot.
    fn net_shown(&self) -> usize {
        let (px, pitch, first, foot) = self.net_rows();
        let mut k = 0;
        while self.net_visible(k).is_some() && first + k * pitch + px / 2 <= foot {
            k += 1;
        }
        k
    }

    /// The row whose pill holds `y`, among the rows shown.
    fn net_row_at(&self, y: i64) -> Option<usize> {
        (0..self.net_shown()).find(|&k| {
            let (_, y0, _, y1) = self.net_row_rect(k);
            (y0 as i64..y1 as i64).contains(&y)
        })
    }

    /// The screen's element under (x, y), as `pressed` names them: the button, `back`, or on the network page a row (rows only while no join runs).
    fn net_hit(&self, x: i64, y: i64) -> Option<u8> {
        let inside = |(x0, y0, x1, y1): (usize, usize, usize, usize)| (x0 as i64..x1 as i64).contains(&x) && (y0 as i64..y1 as i64).contains(&y);
        if self.button_label().is_some() && inside(self.button_at()) {
            return Some(0);
        }
        if self.mode == Mode::Networks && !self.joining {
            return self.net_row_at(y).map(|k| 16 + k as u8);
        }
        None
    }

    /// The pressed element drawn again (lit, or at rest once `pressed` is cleared).
    fn net_redraw(&self, what: u8) {
        if what >= 16 {
            self.net_draw();
        } else {
            self.button();
        }
    }

    /// A finger down: the element under it lit at once.
    fn net_press(&self, x: i64, y: i64) -> bool {
        let Some(e) = self.net_hit(x, y) else { return false };
        self.pressed.set(Some(e));
        self.net_redraw(e);
        true
    }

    /// The finger moved: off its element, the element goes back to rest and will not act.
    fn net_follow(&self, x: i64, y: i64) {
        if let Some(e) = self.pressed.get() {
            if self.net_hit(x, y) != Some(e) {
                self.pressed.set(None);
                self.net_redraw(e);
            }
        }
    }

    /// The finger lifted: the element it was still on acts (the button scans, joins or attests, `back` steps back, a row is picked).
    fn net_release(&mut self) {
        let Some(e) = self.pressed.take() else { return };
        self.net_redraw(e);
        match e {
            0 if self.mode == Mode::Pass => self.net_join(),
            0 if self.mode == Mode::Networks => {
                if !self.joining {
                    self.net_scan();
                }
            }
            // The attest screen's own button (bind, unlock, mint, start over, cancel).
            0 => {
                self.attest();
                self.entry();
                self.button();
                self.words();
            }
            k if k >= 16 => self.net_pick((k - 16) as usize),
            _ => {}
        }
    }

    /// Under the text box: the networks the filter lets through (each a pill: its name, bright when this device can join it, quiet otherwise, and at the right its signal in dBm, or `kept` for the one the device keeps), or, once one is picked, that network alone, lit, with what the scan knows of it.
    fn net_draw(&self) {
        let (y0, y1) = self.net_band();
        self.surf()[y0 * self.w..y1 * self.w].fill(0);
        let (px, pitch, first, foot) = self.net_rows();
        let picked = self.mode == Mode::Pass || self.joining;
        // The rows are text, not buttons (owner, 2026-11-09: "No buttons for the network, just a solid dark grey fill behind selected with bold text"): one flat dark grey behind the network picked, or behind the row under a finger while it is held.
        let lit = if picked { Some(0) } else { self.pressed.get().filter(|&e| e >= 16).map(|e| (e - 16) as usize) };
        if let Some(k) = lit {
            let (x0, ry0, x1, ry1) = self.net_row_rect(k);
            for y in ry0..ry1 {
                self.surf()[y * self.w + x0..y * self.w + x1].fill(grey(SELECTED));
            }
        }
        self.compose(y0, y1, || {
            if picked {
                let found = self.net_found();
                self.net_row_text(0, &self.pick.0[..self.pick.1], found.map(|f| f.0), found.map_or(0, |f| f.1), true);
                // What the scan knows of it, a line each under its pill.
                let mut lines: [Line; 4] = core::array::from_fn(|_| Line::new());
                let mut n = 0;
                match found {
                    None => {
                        let _ = core::fmt::Write::write_str(&mut lines[0], "Not seen in the last scan.");
                        n = 1;
                    }
                    Some((dbm, flags)) => {
                        let _ = core::fmt::Write::write_fmt(&mut lines[n], format_args!("Signal {dbm} dBm"));
                        n += 1;
                        let bands = [(5, "2.4 GHz"), (3, "5 GHz"), (6, "6 GHz")];
                        let mut first_band = true;
                        for (bit, name) in bands {
                            if flags >> bit & 1 != 0 {
                                let _ = core::fmt::Write::write_str(&mut lines[n], if first_band { "" } else { ", " });
                                let _ = core::fmt::Write::write_str(&mut lines[n], name);
                                first_band = false;
                            }
                        }
                        if !first_band {
                            n += 1;
                        }
                        let _ = core::fmt::Write::write_str(&mut lines[n], if flags & 1 != 0 { "WPA2 personal" } else if flags & 2 != 0 { "Open: not joined by this device yet" } else { "WPA3, enterprise or WEP: not joined by this device yet" });
                        n += 1;
                        if flags & (1 << 4) != 0 {
                            let _ = core::fmt::Write::write_str(&mut lines[n], "Kept on this device");
                            n += 1;
                        }
                    }
                }
                for (j, l) in lines[..n].iter().enumerate() {
                    let b = first + pitch + j * px * 3 / 2;
                    if b + px / 2 > foot {
                        break;
                    }
                    window::text(self.display, self.text_to.get(), (self.w / 8) as i16, b as i16, px as u16, QUIET, l.as_bytes());
                }
            } else {
                for k in 0..self.net_shown() {
                    let Some((name, dbm, flags)) = self.net_visible(k) else { break };
                    self.net_row_text(k, name, Some(dbm), flags, lit == Some(k));
                }
            }
        });
    }

    /// Row `k`'s text: the name at the left (bright when joinable or picked), the signal (or `kept`) at the right; a name too long for the row at the scale ends in "..." short of the signal.
    fn net_row_text(&self, k: usize, name: &[u8], dbm: Option<i8>, flags: u8, lit: bool) {
        let (px, pitch, first, _) = self.net_rows();
        let b = first + k * pitch;
        let mut l = Line::new();
        if flags & (1 << 4) != 0 {
            let _ = core::fmt::Write::write_str(&mut l, if wifi_joined() { "connected" } else { "kept" });
        } else if let Some(d) = dbm {
            let _ = core::fmt::Write::write_fmt(&mut l, format_args!("{d} dBm"));
        }
        let x = (self.w * 7 / 8) as i64 - self.measure(px, l.as_bytes()) / 4;
        window::text(self.display, self.text_to.get(), x as i16, b as i16, px as u16, QUIET, l.as_bytes());
        let (mut shown, mut n) = printable(name);
        let room = x - (self.w / 8) as i64 - (px / 2) as i64;
        if self.measure(px, &shown[..n]) / 4 > room {
            while n > 0 {
                n -= 1;
                shown[n..n + 3].copy_from_slice(b"...");
                if self.measure(px, &shown[..n + 3]) / 4 <= room {
                    break;
                }
            }
            n += 3;
        }
        let v = if lit || flags & 1 != 0 { TEXT } else { QUIET };
        window::text(self.display, self.text_to.get(), (self.w / 8) as i16, b as i16, px as u16, v, &shown[..n]);
        // Bold, as the typeface has no heavier weight: the name drawn again a twenty-fourth of an em to the right.
        if lit {
            window::text(self.display, self.text_to.get(), (self.w / 8 + px.div_ceil(24)) as i16, b as i16, px as u16, v, &shown[..n]);
        }
    }

    /// The page's status line: what it is doing, or what to do.
    fn net_status(&self) {
        let name = printable(&self.pick.0[..self.pick.1]);
        let name = core::str::from_utf8(&name.0[..name.1]).unwrap_or("");
        let mut l = Line::new();
        let scanning = self.nets_len >= 1 && self.nets[0] == 1;
        let count = if self.nets_len >= 2 { self.nets[1] } else { 0 };
        let _ = match self.mode {
            _ if self.joining => core::fmt::Write::write_fmt(&mut l, format_args!("Joining {name}.")),
            Mode::Pass if self.button_label().is_some() => core::fmt::Write::write_fmt(&mut l, format_args!("The passphrase for {name}.")),
            Mode::Pass if self.net_found().is_none() => core::fmt::Write::write_fmt(&mut l, format_args!("{name} is out of reach.")),
            Mode::Pass => core::fmt::Write::write_fmt(&mut l, format_args!("{name} needs a kind of security this device cannot join yet.")),
            _ if self.nets_len == 0 => core::fmt::Write::write_str(&mut l, "The WiFi is starting."),
            _ if count == 0 && scanning => core::fmt::Write::write_str(&mut l, "Looking for networks."),
            _ if count == 0 => core::fmt::Write::write_str(&mut l, "No networks found."),
            _ if self.net_visible(0).is_none() => core::fmt::Write::write_str(&mut l, "No network matches."),
            _ => core::fmt::Write::write_str(&mut l, "Type to narrow the list, tap a network to join it."),
        };
        self.status(core::str::from_utf8(l.as_bytes()).unwrap_or(""));
    }

    /// A scan asked of the WiFi driver ([4, 1]), and the list as it stands.
    fn net_scan(&mut self) {
        self.net_ask(&[4, 1]);
    }

    /// SCAN asked (`q`): the reply kept and drawn when it changed. A driver not answering yet (starting, or busy joining) leaves the list as it was.
    fn net_ask(&mut self, q: &[u8]) -> bool {
        let mut out = [0u8; 1 << 11];
        let n = ferros_sys::call_live(cap(self.grants, S_WLAN), q, &mut out);
        if n == ferros_sys::ERR || n < 2 {
            return false;
        }
        let n = n as usize;
        if n == self.nets_len && out[..n] == self.nets[..n] {
            return false;
        }
        self.nets[..n].copy_from_slice(&out[..n]);
        self.nets_len = n;
        self.net_draw();
        self.net_status();
        true
    }

    /// A network picked: its passphrase is typed next.
    fn net_pick(&mut self, k: usize) {
        let Some((name, _, _)) = self.net_visible(k) else { return };
        let n = name.len();
        let mut pick = [0u8; 32];
        pick[..n].copy_from_slice(name);
        self.pick = (pick, n);
        // The filter kept for `back`; the text box takes the passphrase.
        // 32 characters of up to four bytes each (`type_char`'s limit for the filter).
        let mut f = [0u8; 128];
        f[..self.len].copy_from_slice(&self.handle[..self.len]);
        self.filter = (f, self.len, self.chars);
        self.zero_text();
        self.mode = Mode::Pass;
        self.all();
    }

    /// JOIN: the network picked and the passphrase typed, handed to the WiFi driver, which keeps them and joins (the page then waits for the link: `net_poll`). The request's bytes and the text box overwritten after.
    fn net_join(&mut self) {
        let (name, n) = self.pick;
        let pass = &self.handle[..self.len];
        if !(8..=63).contains(&pass.len()) || !pass.is_ascii() {
            self.status("A passphrase is 8 to 63 plain characters.");
            return;
        }
        let mut q = [0u8; 2 + 32 + 63];
        q[0] = 5;
        q[1] = n as u8;
        q[2..2 + n].copy_from_slice(&name[..n]);
        q[2 + n..2 + n + pass.len()].copy_from_slice(pass);
        let len = 2 + n + pass.len();
        let mut out = [0u8; 8];
        let r = ferros_sys::call_live(cap(self.grants, S_WLAN), &q[..len], &mut out);
        for b in q.iter_mut() {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
        match (r, out[0]) {
            (r, _) if r == ferros_sys::ERR || r == 0 => self.status("The WiFi is not answering yet. Try again."),
            (_, 0) => {
                self.zero_text();
                self.joining = true;
                self.mode = Mode::Networks;
                self.entry();
                self.button();
                self.net_status();
            }
            (_, 1) => self.status("A passphrase is 8 to 63 plain characters."),
            (_, 3) => self.status("This device is on a network already."),
            _ => self.status("The device could not keep this network."),
        }
    }

    /// The page's poll (about four times a second): while a join runs, its outcome (the data link up closes the page; the driver answering STATUS again with no link means it failed); otherwise the list, while a scan fills it.
    fn net_poll(&mut self) -> bool {
        if !self.joining {
            // No answer yet (owner, 2026-11-09: "I waited there for a minute, then I pressed scan and it immediately populated"): the driver was not serving `wlan` yet when the page asked (it serves once its chip is up and any kept network joined), so the scan is asked for again each poll until it answers; then the list is read while its scan runs.
            if self.nets_len == 0 {
                return self.net_ask(&[4, 1]);
            }
            return self.nets[0] == 1 && self.net_ask(&[4]);
        }
        // While it joins, the driver answers nothing (a call would hold this screen until the join ended): its WiFi link's measures in the facts say how it goes. STATE 2: joined (the page closes; the address follows). JOIN 1-3: it failed, and why.
        let f = ferros_sys::facts();
        let wifi = |kind: u16| f.0[33 + 8..41 + 8].iter().filter_map(|&m| ferros_sys::measure_of(m)).find(|m| m.0 == kind).map(|m| m.1);
        if wifi(ferros_sys::measure::STATE) == Some(2) {
            self.net_close();
            return true;
        }
        let outcome = match wifi(ferros_sys::measure::JOIN) {
            Some(o @ 1..=3) => o,
            _ => return false,
        };
        self.joining = false;
        self.mode = Mode::Pass;
        let shown = printable(&self.pick.0[..self.pick.1]);
        let name = core::str::from_utf8(&shown.0[..shown.1]).unwrap_or("");
        let mut l = Line::new();
        let _ = match outcome {
            1 => core::fmt::Write::write_fmt(&mut l, format_args!("{name} is out of reach.")),
            3 => core::fmt::Write::write_fmt(&mut l, format_args!("{name} did not answer. Try again.")),
            _ => core::fmt::Write::write_fmt(&mut l, format_args!("{name} refused the passphrase.")),
        };
        self.status(core::str::from_utf8(l.as_bytes()).unwrap_or(""));
        self.entry();
        self.button();
        true
    }

    /// The keyboard's rows: from its top to the bar's foot at its lowest (on the app's circle, its bottom).
    fn kb_rows(&self) -> (usize, usize) {
        (self.kb.top, self.kb.bottom)
    }

    /// One key drawn lit or at rest, onto the glass: into the frame buffer, its label drawn in by the display (on the board's target, then read back, as `board` bakes its labels, so a later present of these rows keeps it), then the keyboard's rows presented and shown.
    /// Since the sandbox (730cb8a, 2026-11-03) the frame buffer is ahau's own, not the shown surface: a key drawn into it and shown without a present never reached the glass, so a press stopped brightening its key.
    /// Only the key's own rows: laid first from the board on show, since the frame buffer's keyboard rows are also where a board is drawn ahead (`board`, at start and at a press), so they can hold a board not on show (the upper-case one flashed at the first press, 2026-11-04); the whole keyboard's rows took 20-53 ms a press.
    fn light(&self, key: Key, lit: bool) {
        let (y0, y1) = self.kb.key_rows(key);
        let (top, bottom) = self.kb_rows();
        if (bottom - top) * self.w <= BOARD_PIXELS {
            self.lay_board(self.shown_board.get(), y0, y1);
        }
        unsafe { NLABELS = 0 };
        self.kb.draw_key(self.surf(), key, lit, &mut self.label());
        let (tb, _) = self.target();
        self.put_rows(tb, y0, y1);
        self.replay_labels(tb);
        self.get_rows(tb, y0, y1);
        self.present(y0, y1);
        self.show();
        if self.words_shown.get() {
            self.words();
            self.show();
        }
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
                let tr = if t[y * w + x] >> 63 != 0 { 16384 } else { t[y * w + x] >> 49 & 0x3FFF };
                b.clear[i] = (tr * 255 >> 14) as u8;
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
        let n = (bottom - top) * self.w;
        // WHY: the board's size follows the panel's; one too big for BOARDS is drawn tinted the slow way (every key and label again) instead of indexing past it.
        if n > BOARD_PIXELS {
            self.kb.draw(self.surf(), &mut self.label());
            self.present(top, self.h);
            self.replay_labels(self.base);
            return;
        }
        let t0 = now();
        let slot = self.board(&self.kb);
        let t1 = now();
        self.lay_board(slot, top, bottom);
        let t2 = now();
        self.present(top, bottom);
        let t3 = now();
        self.shown_board.set(slot);
        // While joining, the words go under the board just laid (`words` composes the keyboard's rows with them).
        if self.words_shown.get() {
            self.words();
        }
        // The lift's stages, for its console line (`Touch::Tap`): the board (drawn or kept), the tint pass, the present.
        let us = |a: u64, b: u64| b.wrapping_sub(a) * 1_000_000 / freq();
        unsafe { LIFT_US[1..4].copy_from_slice(&[us(t0, t1), us(t1, t2), us(t2, t3)]) };
    }

    /// Rows y0..y1 (inside the keyboard's) of board `slot` laid into the frame buffer in the handle's tints, the shift key's ▲ on top: what `show_board` presents.
    fn lay_board(&self, slot: usize, y0: usize, y1: usize) {
        let (top, w) = (self.kb_rows().0, self.w);
        let b = unsafe { &(*(&raw const BOARDS))[slot] };
        let t = self.surf();
        let tint = self.kb.tint.unwrap_or([[1 << 14; 3]; 64]);
        for i in (y0 - top) * w..(y1 - top) * w {
            t[top * w + i] = board_px(b, &tint, i) | clear_bits(b.clear[i]);
        }
        self.kb.draw_key(t, Key::Bar(honeycomb::B_SHIFT), false, &mut |_, _, _, _, _| {});
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
        if self.back != 0 && (self.kb_rows().1 - self.kb_rows().0) * self.w <= BOARD_PIXELS {
            self.board(kb);
        }
    }

    fn prompt(&self) -> &'static str {
        match self.mode {
            // The screen's words (owner, 2026-10-31: simple and direct; AUTH.md "What the attest screen says").
            Mode::Fresh if self.bound => "Type your handle.",
            Mode::Fresh => "This device is unbound.\nType your handle to bind.",
            // No data link, nothing can be found: said so rather than looking (owner, 2026-11-09). The loop redraws it when a link comes or goes.
            Mode::Looking(_) if !ferros_sys::data_link_up(&ferros_sys::facts()) => "Waiting for connectivity.",
            Mode::Looking(_) => "Finding this identity.",
            Mode::Join(_) => "Use an existing device to bind this device to your fleet.",
            Mode::Mint(_) => "This handle is unclaimed.\nMinting is permanent.\nType your handle again to mint your identity.",
            Mode::Grey(_) if !ferros_sys::data_link_up(&ferros_sys::facts()) => "Waiting for connectivity.",
            Mode::Grey(_) => "No answer yet. Still looking.",
            Mode::Wrong => "This handle's record doesn't match.\nStart over with another.",
            Mode::Owned => "This device belongs to another identity.\nOnly its fleet can let it go.",
            Mode::Words(_) => "Type the words the new device shows.",
            Mode::Hosting => "",
            // The network page writes its own lines (`net_status`).
            Mode::Networks | Mode::Pass => "",
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

    /// The attest button (or a mint's second enter): a first entry derives the proof and hands the roots to the client; a mint confirms; wrong starts over. A join or no answer has no button: the screen closes only when the fleet binds the device.
    fn attest(&mut self) {
        match self.mode {
            Mode::Words(_) => {
                self.end_words(true);
                return;
            }
            Mode::Hosting => return,
            Mode::Join(_) | Mode::Grey(_) => return,
            Mode::Wrong | Mode::Owned => {
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
                console(self.grants, b"ahau: roots set; their thread hands them to fgtw\n");
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
                        // Its words come with `client_thread`'s next copy (the mint rang the doorbell).
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
        // The heap is the proof's while it runs: a GPU compile in progress (a keystroke's, half a millisecond) finishes first (HEAP_USE).
        while HEAP_USE.compare_exchange(0, 2, core::sync::atomic::Ordering::Acquire, core::sync::atomic::Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
        let proof = *ihi::handle_proof(&blake3::Hash::from(*seed)).as_bytes();
        HEAP_USE.store(0, core::sync::atomic::Ordering::Release);
        let ms = ferros_sys::us_since(t0) / 1000;
        let mut l = Line::new();
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: handle proof in {ms} ms on the X3\n"));
        console(self.grants, l.as_bytes());
        if !self.put(S_AHAU, b"proof", &proof) {
            console(self.grants, b"ahau: could not keep the handle proof in the identity's space\n");
        }
        proof
    }

    /// The person's wallpaper (owner, 2026-11-07: handle entry unlocks it): their wallpaper seed to the display (op 42), which makes their wallpaper from it unless they set their own. The seed is one-way from their identity seed (BLAKE3's key derivation under a context of its own), so it shows nothing of the handle, and it is the same on every device of theirs and theirs alone. It is kept in their space (`ahau.wallpaper`) for a boot that unlocks without the handle being typed (`send_wallpaper`).
    fn wallpaper(&self, seed: &[u8; 32]) {
        let ws = ahau_core::wallpaper_seed(seed);
        // Kept only when it is not already (every attest of a device's person but the first): a write is a vault commit, and this is on the way to the home screen (owner, 2026-11-08: "should be milliseconds").
        let mut kept = [0u8; 32];
        let same = ferros_sys::store_get(cap(self.grants, S_AHAU), b"wallpaper", &mut kept) == Some(32) && kept == ws;
        if !same && !self.put(S_AHAU, b"wallpaper", &ws) {
            console(self.grants, b"ahau: the wallpaper seed was not kept\n");
        }
        send_wallpaper(self.display, &ws);
    }

    /// A person this device knows: the screen closes now, then the handle proof is made behind it and the roots go to the identity service (fgtw) (made again until it takes them: it starts once the space is open), then this process ends.
    fn unlocked_known(&mut self, seed: [u8; 32]) -> ! {
        self.wipe();
        console(self.grants, b"ahau: unlocked (this device knows the identity); the screen goes, the proof follows\n");
        // The screen first, then the wallpaper's seed (the display loads it on a thread of its own and answers at once): the home screen waits for neither (owner, 2026-11-08: "4 seconds from attest to showing homescreen ... should be milliseconds").
        window::done(self.display, self.base);
        self.wallpaper(&seed);
        let proof = self.proof_for(&seed, false);
        self.send_roots(&seed, &proof);
        // `roots_thread` hands them over; this waits on its word, no timer.
        loop {
            let t = ROOTS_TAKEN.load(core::sync::atomic::Ordering::Acquire);
            if t == self.roots_gen {
                break;
            }
            ferros_sys::wait_world(&ROOTS_TAKEN, t, 0);
        }
        self.roots_pending = false;
        console(self.grants, b"ahau: roots handed to fgtw\n");
        self.host_words()
    }

    fn attested(&mut self) -> ! {
        if let Some(seed) = self.session_seed {
            self.wallpaper(&seed);
        }
        console(self.grants, b"ahau: attested\n");
        self.host_words()
    }

    /// After attest this process stays, its window gone, as the keyboard for words the identity service wants typed (the Fleet page's add flow and departure approvals, fgtw ops 15 and 16, the sponsor's side of photon docs/pairing-v2.md): asked four times a second, and while words are wanted the screen shows its keyboard and an entry line, each stroke sent to the identity service and its answer on the status line, until they match, the person cancels, or the service no longer wants them. The handle was wiped at attest; nothing of it is here.
    fn host_words(&mut self) -> ! {
        self.wipe();
        self.mode = Mode::Hosting;
        window::done(self.display, self.base);
        console(self.grants, b"ahau: staying as the keyboard for words the identity service asks for (the Fleet page)\n");
        let touch = cap(self.grants, S_TOUCH);
        let mut touches = Touches::new(self.w);
        touches.trace_words = false;
        let mut held = [(0u8, 0i64, 0i64); 16];
        let mut r = [0u8; 256];
        let mut pressed: Option<Key> = None;
        loop {
            ferros_sys::pet();
            let kind = self.words_wanted();
            if kind == 0 {
                sleep_until(now() + freq() / 4);
                continue;
            }
            self.mode = Mode::Words(kind);
            self.wipe();
            self.retint();
            self.all();
            self.show();
            console(self.grants, b"ahau: words wanted: a new device's\n");
            let mut polls = 0u32;
            while matches!(self.mode, Mode::Words(_)) {
                ferros_sys::pet();
                if self.blink() {
                    self.show_entry();
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
                let (display, base) = (self.display, self.base);
                touches.poll(&mut self.kb, &held[..k], &mut |x, y| window::mine(display, base, x as u16, y as u16), &mut |t| {
                    outs[no] = t;
                    no += 1;
                });
                for &t in &outs[..no] {
                    if !matches!(self.mode, Mode::Words(_)) {
                        break;
                    }
                    match t {
                        // Typed at the press, as the handle is (COMMIT_AT_PRESS): the lift only lets the key go.
                        Touch::Light(key) => {
                            self.light(key, true);
                            self.prepare(key);
                            self.tap(key);
                            if matches!(self.mode, Mode::Words(_)) {
                                self.light(key, true);
                            }
                            pressed = Some(key);
                        }
                        Touch::Unlight(key) => {
                            if pressed == Some(key) {
                                pressed = None;
                            }
                            self.light(key, false);
                        }
                        Touch::Scrolled => self.show_board(),
                        Touch::Tap(key) => {
                            if pressed == Some(key) {
                                pressed = None;
                                self.light(key, false);
                            } else {
                                self.tap(key);
                            }
                        }
                        Touch::TapAt(x, y) => self.tap_at(x, y),
                        Touch::Drag(x0, y0, x, _) => {
                            self.drag_text(x0, y0, x);
                        }
                        Touch::Trail(..) | Touch::Traced => {}
                    }
                }
                // The identity service no longer wants them (matched on its side, the Fleet page closed the flow, the departure gone): the screen goes.
                polls += 1;
                if polls % 64 == 0 && matches!(self.mode, Mode::Words(_)) && self.words_wanted() != kind {
                    self.end_words(false);
                }
                if matches!(self.mode, Mode::Words(_)) && (no > 0 || self.follow_layout()) {
                    self.show();
                }
                sleep_until(now() + freq() / 240);
            }
        }
    }

    /// The words the identity service wants typed now (fgtw op 16): 0 none, 1 a newcomer's, 2 a leaver's.
    fn words_wanted(&self) -> u8 {
        let mut o = [0u8; 4];
        let n = ferros_sys::call_live(cap(self.grants, S_FGTW_EP), &[16], &mut o);
        // Only a newcomer's words (1): a departure takes no words since 2026-11-10 (owner: Approve or Deny).
        if n == ferros_sys::ERR || n == ferros_sys::NOTHING || n < 1 || o[0] > 1 { 0 } else { o[0] }
    }

    /// The words as typed to the identity service: a newcomer's, matched at every stroke (fgtw op 11, the live matcher). The answer on the status line; a match ends the words.
    fn send_words(&mut self) {
        let Mode::Words(_) = self.mode else { return };
        let mut m = [0u8; 1 + TEXT_BYTES];
        m[0] = 11;
        m[1..1 + self.len].copy_from_slice(&self.handle[..self.len]);
        let mut o = [0u8; 128];
        let n = ferros_sys::call_live(cap(self.grants, S_FGTW_EP), &m[..1 + self.len], &mut o);
        if n == ferros_sys::ERR || n == ferros_sys::NOTHING || n < 1 {
            self.status("The identity service is not answering.");
            return;
        }
        // WHY the min: the reply's length is the service's word; what follows the kind is read only as far as this buffer took it.
        let n = (n as usize).min(o.len());
        let text = |from: usize| core::str::from_utf8(o.get(from..n).unwrap_or(&[])).unwrap_or("");
        let mut l = Line::new();
        match o[0] {
            // Matched: the identity service adds it now; the Fleet page shows how it goes.
            2 => {
                self.end_words(false);
                return;
            }
            0 if n > 2 => {
                let _ = core::fmt::Write::write_fmt(&mut l, format_args!("\"{}\" is not one of the words.", text(2)));
            }
            0 => {
                let _ = core::fmt::Write::write_str(&mut l, "Waiting for the new device to ask.");
            }
            1 => {
                let _ = core::fmt::Write::write_str(&mut l, self.prompt());
            }
            3 => {
                let _ = core::fmt::Write::write_fmt(&mut l, format_args!("No device asking has \"{}\".", text(2)));
            }
            4 => {
                let _ = core::fmt::Write::write_fmt(&mut l, format_args!("Matches {}.", text(2)));
            }
            5 => {
                let _ = core::fmt::Write::write_fmt(&mut l, format_args!("Matches {} devices.", o.get(1).copied().unwrap_or(0)));
            }
            _ => {
                self.end_words(false);
                return;
            }
        }
        self.status(core::str::from_utf8(l.as_bytes()).unwrap_or(""));
    }

    /// The words done: the screen goes, the text wiped; `cancel` tells the identity service they are no longer wanted (op 15 [0]).
    fn end_words(&mut self, cancel: bool) {
        if cancel {
            let mut o = [0u8; 4];
            let _ = ferros_sys::call_live(cap(self.grants, S_FGTW_EP), &[15, 0], &mut o);
        }
        self.wipe();
        self.mode = Mode::Hosting;
        window::done(self.display, self.base);
    }

    /// A finger is down on `key`: the handle as that key would make it goes to the mark's thread now, so its frame is ready by the lift; the text itself changes only at the lift (`tap`). The key's effect is found on a copy of the keyboard (its shift and page are the real one's), so nothing here changes until the tap.
    fn preview(&self, key: Key) {
        if !self.typing() || matches!(self.mode, Mode::Words(_) | Mode::Networks | Mode::Pass) {
            return;
        }
        let mut kb = self.kb.clone();
        // The handle as the key will leave it, at the blinkey, as `tap` will make it.
        let (mut t, mut n, mut ch) = (self.handle, self.len, self.chars);
        match kb.tap(key) {
            Some(Event::Char(c)) => {
                insert_at(&mut t, &mut n, &mut ch, self.caret, c, MAX_CHARS);
            }
            Some(Event::Enter) => {
                insert_at(&mut t, &mut n, &mut ch, self.caret, '\n', MAX_CHARS);
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
        let limit = match self.mode {
            Mode::Words(_) => WORD_CHARS,
            // WPA2's longest passphrase.
            Mode::Pass => 63,
            // A filter on names, which are 32 bytes at most.
            Mode::Networks => 32,
            _ => MAX_CHARS,
        };
        self.caret = insert_at(&mut self.handle, &mut self.len, &mut self.chars, self.caret, c, limit);
        self.entry();
    }

    fn tap(&mut self, key: Key) {
        let typing = self.typing();
        match self.kb.tap(key) {
            Some(Event::Char(_) | Event::Back | Event::Enter | Event::Leave) if !typing => return,
            Some(Event::Char(c)) => self.type_char(c),
            Some(Event::Back) => {
                self.caret = delete_before(&mut self.handle, &mut self.len, &mut self.chars, self.caret);
                self.entry();
            }
            // The passphrase: enter joins, clear empties it (the mark is not the passphrase's, so nothing is published).
            Some(Event::Enter) if self.mode == Mode::Pass => {
                self.net_join();
                return;
            }
            Some(Event::Leave) if self.mode == Mode::Pass => {
                self.zero_text();
                self.entry();
            }
            // The filter: enter picks the one network it leaves, clear empties it.
            Some(Event::Enter) if self.mode == Mode::Networks => {
                if self.net_visible(0).is_some() && self.net_visible(1).is_none() {
                    self.net_pick(0);
                }
                return;
            }
            Some(Event::Leave) if self.mode == Mode::Networks => {
                self.zero_text();
                self.entry();
            }
            // Words: enter sends them as typed (they hold no newline).
            Some(Event::Enter) if matches!(self.mode, Mode::Words(_)) => {
                self.send_words();
                return;
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
        // Words: matched as they are typed (the newcomer's), the result on the status line.
        if matches!(self.mode, Mode::Words(_)) {
            self.send_words();
            if !matches!(self.mode, Mode::Words(_)) {
                return;
            }
        }
        // Every stroke: the keys take the new handle's rainbow, laid on from the board in one pass; a letter that ended a one-letter shift brings the lower-case board, drawn at its press. A passphrase leaves the handle's rainbow as it was: the keys would otherwise show a hash of it.
        let t = now();
        if !self.net_open() {
            self.retint();
        } else if self.mode == Mode::Networks {
            // The filter changed: the list follows.
            self.net_draw();
            self.net_status();
        }
        unsafe { LIFT_US[0] = now().wrapping_sub(t) * 1_000_000 / freq() };
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
    // The edge greys from the fill as the honeycomb's keys have theirs: light √(fill² + BEVEL²) on the top left, shadow fill² / light on the bottom right.
    let light = (fill * fill + honeycomb::BEVEL * honeycomb::BEVEL).isqrt();
    let shadow = fill * fill / light;
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
            // The hairline's tone by the box's anti-diagonal: light above it, shadow below, lit from the top left as the keys are (owner, 2026-11-09: "TL should be brightish like the keymap and the bottom right should be darker"): the honeycomb's rule on this fill.
            let edge = if fx / bw + fy / bh <= 1. { light } else { shadow };
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
    // The bonded button's helpers, before the mark's thread starts its first pass.
    if unsafe { BONDED } {
        for t in 0..HELPERS {
            let stack = unsafe { &mut (*(&raw mut HELP_STACKS))[t] };
            if ferros_sys::thread_spawn(help_thread, stack, t as u64 + 1) == ferros_sys::ERR {
                break;
            }
            HELPERS_UP.store(t as u64 + 1, core::sync::atomic::Ordering::Release);
        }
    }
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
            // After a quick pass the GPU drew, the 4 x 4 as 16 GPU passes, each published as it lands; the CPU's pass only when the GPU could not finish them for this handle.
            if fine && GPU_DREW.load(core::sync::atomic::Ordering::Relaxed) && (gpu_fine_passes(g, &mut frames) || FRESH_GEN.load(core::sync::atomic::Ordering::Acquire) != g) {
                full = false;
                continue;
            }
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

/// A helper of the bonded button's passes (HELPERS): share `t` of every pass's rows, `t` from 1 (the mark's thread draws share 0). Each draws only its own rows of FRESH, so the threads never write the same pixel.
extern "C" fn help_thread(t: u64) -> ! {
    ferros_sys::pin(ferros_sys::CORES_WORKERS);
    let mut seen = 0u64;
    loop {
        let j = HELP_JOB.load(core::sync::atomic::Ordering::Acquire);
        if j == seen {
            ferros_sys::wait(&HELP_JOB, j, 0);
            continue;
        }
        seen = j;
        if HELP_KIND.load(core::sync::atomic::Ordering::Acquire) == 1 {
            let (c, pts) = unsafe { HELP_EXTENT };
            let step = 1 + HELPERS_UP.load(core::sync::atomic::Ordering::Acquire) as usize;
            for i in (t as usize..tapuwae::EXTENT_POINTS).step_by(step) {
                unsafe { (*pts)[i] = tapuwae::extent_point(c, i as u32) };
            }
            HELP_DONE.fetch_add(1, core::sync::atomic::Ordering::Release);
            ferros_sys::wake(&HELP_DONE, 1);
            continue;
        }
        let ((p, fine, g, side), r) = unsafe { (HELP_PASS, FRESH_JOB.1) };
        let buf = unsafe { &mut (&mut *(&raw mut FRESH))[..side * side] };
        let mid = side as f32 * 0.5;
        let moved_on = || fine && FRESH_GEN.load(core::sync::atomic::Ordering::Acquire) != g;
        let step = 1 + HELPERS_UP.load(core::sync::atomic::Ordering::Acquire) as usize;
        if !tapuwae::render_button_pack16(buf, side, mid, mid, r as f32, unsafe { &*p }, &tapuwae::Look::attest(), if fine { 4 } else { 1 }, (t as usize, step), &moved_on) {
            HELP_STOPPED.store(true, core::sync::atomic::Ordering::Release);
        }
        HELP_DONE.fetch_add(1, core::sync::atomic::Ordering::Release);
        ferros_sys::wake(&HELP_DONE, 1);
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
    let typed = core::str::from_utf8(&text[..n]).unwrap_or("");
    // Bound: the attest button, the whole disc at every change (it has no components to redraw alone), in the surface's own pixels (`pack16`, as `render_fresh` writes them: the G2t16 encoding first used here came out nearly white). Two passes, as the fresh mark's, through the same machinery (tapuwae `render_button_pack16`: the ball four samples to a lane call, as the fresh mark shades its components): one sample a pixel at once, then a 4 × 4 grid to clear it up, given up for a newer handle. The adaptive `Look::attest()` (up to 64 samples a pixel, one at a time) took 22 s for the first draw and 55 s a keystroke on the workers (2026-11-04).
    let done = if unsafe { BONDED } {
        let t = now();
        let p = placed(&salt, typed);
        GPU_PARTS_US[0].store(ferros_sys::us_since(t), core::sync::atomic::Ordering::Relaxed);
        // The quick pass on the GPU when its service is up (GPU.md), before any helper is woken: when it is drawn there the helpers have nothing to do this pass (and must not write the rows it fills).
        let gpu_drawn = !fine && gpu_quick(grants, r, &p);
        GPU_DREW.store(gpu_drawn, core::sync::atomic::Ordering::Relaxed);
        // The rows shared with the helpers (HELPERS): theirs first, then this thread's own, then every helper waited for, so `p` and the pass outlive them all.
        let up = if gpu_drawn { 0 } else { HELPERS_UP.load(core::sync::atomic::Ordering::Acquire) };
        if up > 0 {
            HELP_DONE.store(0, core::sync::atomic::Ordering::Relaxed);
            HELP_STOPPED.store(false, core::sync::atomic::Ordering::Relaxed);
            unsafe { HELP_PASS = (&p, fine, g, side) };
            HELP_KIND.store(0, core::sync::atomic::Ordering::Release);
            HELP_JOB.fetch_add(1, core::sync::atomic::Ordering::Release);
            ferros_sys::wake(&HELP_JOB, up);
        }
        let mine = gpu_drawn || tapuwae::render_button_pack16(buf, side, mid, mid, r as f32, &p, &tapuwae::Look::attest(), if fine { 4 } else { 1 }, (0, 1 + up as usize), &moved_on);
        loop {
            let d = HELP_DONE.load(core::sync::atomic::Ordering::Acquire);
            if d >= up {
                break;
            }
            ferros_sys::wait(&HELP_DONE, d, 0);
        }
        mine && !HELP_STOPPED.load(core::sync::atomic::Ordering::Acquire)
    } else {
        tapuwae::render_fresh(buf, side, mid, mid, r as f32, &salt, typed, &tapuwae::Look::fresh(), 4, mask, coarse, map, full, fine, &moved_on)
    };
    // A pass the CPU drew is in FRESH: the screen shows it from here (a GPU pass already moved MARK_FRONT to its output).
    if !GPU_DREW.load(core::sync::atomic::Ordering::Relaxed) {
        MARK_FRONT.store(0, core::sync::atomic::Ordering::Release);
    }
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
    let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: attest mark {what} ({side} px, {}) in {} ms {}{}\n", if fine { "4x4" } else { "1" }, ferros_sys::us_since(t0) / 1000, if GPU_DREW.load(core::sync::atomic::Ordering::Relaxed) { "on the GPU" } else { "on the workers" }, if GPU_DREW.load(core::sync::atomic::Ordering::Relaxed) { { let q = |i: usize| GPU_PARTS_US[i].load(core::sync::atomic::Ordering::Relaxed); alloc::format!(" (params {} us, stage {} us, GPU call {} us: compile {} us, job {} us)", q(0), q(1), q(2), GPU_COMPILE_US.load(core::sync::atomic::Ordering::Relaxed), GPU_JOB_US.load(core::sync::atomic::Ordering::Relaxed)) } } else { alloc::string::String::new() }));
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
    let front = MARK_FRONT.load(core::sync::atomic::Ordering::Acquire);
    let buf = if front == 0 { unsafe { &(&*(&raw const FRESH))[..side * side] } } else { unsafe { core::slice::from_raw_parts(front as *const u64, side * side) } };
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
    // The screen's own thread on the X3, beside the display server: the mark's passes hold all four workers (HELPERS) while a handle is typed, and a press or a lift drawn behind them waited for their slices. The mark's threads pin themselves to the workers.
    ferros_sys::pin(ferros_sys::CORES_BIG);
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
        // The person's wallpaper from the seed kept when they last typed their handle (`wallpaper`).
        let mut ws = [0u8; 32];
        if ferros_sys::store_get(cap(grants, S_AHAU), b"wallpaper", &mut ws) == Some(32) {
            send_wallpaper(cap(grants, S_DISPLAY), &ws);
        }
        exit(0);
    }
    let mode = Mode::Fresh;
    let mut kb = Keyboard::new(w, ferros_sys::active_circle(w, h));
    fit_bar(cap(grants, S_DISPLAY), &mut kb);
    kb.left = "clear";
    // The device's layout; a value from a newer build than this one falls back to the default.
    let l = ferros_sys::pref(ferros_sys::PREF_LAYOUT) as usize;
    kb.layout = if l < honeycomb::LAYOUTS { l } else { 0 };
    let bound = ferros_sys::identity_bound(cap(grants, S_UNLOCK)) == Some(true);
    unsafe { BONDED = bound };
    // The keyboard's back buffer, when init granted a second surface (an older init does not: the board then redraws in place).
    let back = match ferros_sys::dma(cap(grants, S_BACK)) {
        b if b == 0 || b == u64::MAX || b == base => 0,
        b => b,
    };
    let mut a = Ahau { grants, display, base, w, h, kb, mode, bound, session_seed: None, session_proof: None, roots_pending: false, roots_gen: 0, client_seen: 0, handle: [0; TEXT_BYTES], len: 0, chars: 0, caret: 0, scale: core::cell::Cell::new(ferros_sys::ui_scale() as f32), text_to: core::cell::Cell::new(base), status_text: core::cell::Cell::new(([0; 256], 0)), word_buf: [0; 512], word_len: 0, back, boards: core::cell::Cell::new([None; 2]), shown_board: core::cell::Cell::new(0), wave_top: core::cell::Cell::new(true), blink_next: core::cell::Cell::new(0), blink_rng: core::cell::Cell::new(0xDEAD_BEEF), pan: core::cell::Cell::new(0), grab: core::cell::Cell::new(None), under: Mode::Fresh, stash: ([0; TEXT_BYTES], 0, 0, 0), nets: [0; 1 << 11], nets_len: 0, pick: ([0; 32], 0), filter: ([0; 128], 0, 0), joining: false, pressed: core::cell::Cell::new(None), status_lines: core::cell::Cell::new(0), words_shown: core::cell::Cell::new(false), words_scroll: core::cell::Cell::new(0), words_grab: core::cell::Cell::new(None), words_dys: core::cell::Cell::new([0; 3]), words_fling: core::cell::Cell::new(0.), words_fling_res: core::cell::Cell::new(0.), words_max: core::cell::Cell::new(0), words_frame: core::cell::Cell::new(None), dev_name: ([0; 48], 0) };
    console(grants, if back != 0 { b"ahau: the keyboard's back buffer is a second surface\n" } else { b"ahau: no second surface: the keyboard redraws in place\n" });
    // The identity service's two threads (`roots_thread`, `client_thread`): both sleep until a handle is typed.
    ferros_sys::thread_spawn(roots_thread, unsafe { &mut *(&raw mut ROOTS_STACK) }, grants);
    ferros_sys::thread_spawn(client_thread, unsafe { &mut *(&raw mut CLIENT_STACK) }, grants);
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
    // The pinch above the keyboard: (spread at its start, the scale then), and whether the touch now down is one (`rescale`).
    let mut pinch: Option<(f32, f32)> = None;
    // The data link's state last drawn into the status while looking (None outside it).
    let mut link_up: Option<bool> = None;
    let mut pinched = false;
    // A pull (the network page down, or up again) acts once per touch: set when it acts, cleared when every finger lifts.
    let mut pulled = false;
    // A finger on one of the system's places (op 41): which (bit 7 once it moved: no longer a tap), where it landed, whether it acted; whether this touch's landing was asked about yet.
    let mut orb: Option<(u8, i64, i64, bool)> = None;
    let mut orb_asked = false;
    // Whether this touch's landing was looked at for the network page's rows and buttons.
    let mut press_asked = false;
    // The key typed at its press (COMMIT_AT_PRESS), whose tap at the lift is then only its letting go.
    let mut pressed: Option<Key> = None;
    // The last frame of the mark put on the glass (FRESH_DONE's count).
    let mut mark_shown = 0u64;
    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
        let done = FRESH_DONE.load(core::sync::atomic::Ordering::Acquire);
        let typed = FRESH_DONE_GEN.load(core::sync::atomic::Ordering::Acquire) == SHOW_GEN.load(core::sync::atomic::Ordering::Acquire);
        // Not while the network page covers the mark: it is drawn whole when the page closes.
        if done != mark_shown && typed && !a.net_open() {
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
        // While it looks for the identity, the status follows the network: "Waiting for connectivity." with no data link, the looking words once one is up (and back), redrawn only when that changes.
        if matches!(a.mode, Mode::Looking(_) | Mode::Grey(_)) {
            let up = ferros_sys::data_link_up(&ferros_sys::facts());
            if link_up != Some(up) {
                if link_up.is_some() {
                    a.status(a.prompt());
                    a.show();
                }
                link_up = Some(up);
            }
        } else {
            link_up = None;
        }
        let n = call(touch, &[1], &mut r) as usize;
        let count = if n >= 1 && n != ferros_sys::ERR as usize { (r[0] as usize).min(16) } else { 0 };
        let mut k = 0;
        while k < count && 1 + (k + 1) * 12 <= n {
            let rec = &r[1 + k * 12..1 + (k + 1) * 12];
            held[k] = (rec[0], u16::from_le_bytes([rec[4], rec[5]]) as i64, u16::from_le_bytes([rec[6], rec[7]]) as i64);
            k += 1;
        }
        // Two fingers above the keyboard: a pinch, the scale following their spread, 64ths of an octave by log₂ of how far they opened since they began (as the map's pages), kept as the device's when they lift. Until every finger lifts, the touches above the keyboard are the pinch's, not taps or drags.
        // It starts with both above the keyboard and holds wherever they wander after (owner, 2026-11-09: "the two finger zoom gets cancelled when you touch the keyboard"); one finger lifted, the other is still the pinch's, until both are up.
        if k == 2 && (pinch.is_some() || (held[0].2 < a.kb.top as i64 && held[1].2 < a.kb.top as i64)) {
            let (dx, dy) = (held[0].1 - held[1].1, held[0].2 - held[1].2);
            // WHY the floor of 1: two fingers on one spot have no spread, and log₂ of a ratio over 0 is infinite.
            let d = (((dx * dx + dy * dy) as u64).isqrt() as f32).max(1.);
            let (d0, s0) = *pinch.get_or_insert((d, a.scale.get()));
            // WHY the bounds: the preference's own range, half to twice (the kernel refuses anything past it).
            let s = (s0 + 64. * libm::log2f(d / d0)).clamp(-64., 64.);
            if (s - a.scale.get()).abs() >= 1. {
                a.scale.set(s);
                a.rescale();
            }
            pinched = true;
        }
        // The buttons (and the network page's rows) under a lone finger: lit at its landing, back at rest if it leaves them, acting at the lift while still on them (`net_release`; owner, 2026-11-09: "any movement just ... cancels"). A second finger (a pinch) lets the element go.
        {
            if k == 1 {
                let (x, y) = (held[0].1, held[0].2);
                if !press_asked {
                    press_asked = true;
                    if y < a.kb.top as i64 && a.net_press(x, y) {
                        a.show();
                    }
                } else if a.pressed.get().is_some() {
                    a.net_follow(x, y);
                    if a.pressed.get().is_none() {
                        a.show();
                    }
                }
            } else if k >= 2 {
                if let Some(e) = a.pressed.take() {
                    a.net_redraw(e);
                    a.show();
                }
            }
        }
        if k == 0 {
            if a.pressed.get().is_some() {
                a.net_release();
                a.show();
            }
            press_asked = false;
        }
        // The system's places around the glass, which are the display's, so a finger on one never reaches this screen's own touches: asked of the display where a lone finger lands (op 41), as the map does. A radio's circle (0-4) tapped still switches that radio; the WiFi circle (1) swiped toward the centre opens the network page (owner, 2026-11-07: tap a circle, slide toward the centre = its page); the back corner (9) swiped inward, or tapped, steps back. Inward is the finger's travel along the line from where it landed to the screen's centre, past a twelfth of the span (the map's measure, 2wh/(w+h)).
        if k == 1 {
            let (x, y) = (held[0].1, held[0].2);
            match orb {
                // Only where the glass is not this screen's own (`window::mine`) and no button took the press: op 41 calls the whole lower half outside the app's circle the back region, which takes in this screen's buttons and keys, and a press on them is theirs.
                None if !orb_asked && (a.pressed.get().is_some() || window::mine(display, base, x as u16, y as u16)) => orb_asked = true,
                None if !orb_asked => {
                    orb_asked = true;
                    let mut m = [41u8, 0, 0, 0, 0];
                    m[1..3].copy_from_slice(&(x as u16).to_le_bytes());
                    m[3..5].copy_from_slice(&(y as u16).to_le_bytes());
                    let mut q = [0xFFu8; 1];
                    if call(display, &m, &mut q) == 1 && (q[0] < 5 || q[0] == 9) {
                        orb = Some((q[0], x, y, false));
                        // The back button of the helm lit at once (display op 54).
                        if q[0] == 9 {
                            helm_light(display, 9, 0.5);
                        }
                    }
                }
                Some((what, x0, y0, false)) => {
                    let (ux, uy) = ((a.w as i64 / 2 - x0) as f32, (a.h as i64 / 2 - y0) as f32);
                    let inward = ((x - x0) as f32 * ux + (y - y0) as f32 * uy) / libm::sqrtf(ux * ux + uy * uy);
                    let span = (2 * a.w * a.h) as f32 / (a.w + a.h) as f32;
                    // The helm's back button follows the swipe, as the map's does: ½(1 + tanh(inward / threshold)).
                    if what & 0x7F == 9 {
                        helm_light(display, 9, 0.5 * (1.0 + libm::tanhf(inward / (span * (1. / 12.)))));
                    }
                    if inward > span * (1. / 12.) {
                        // Acted: the lift is not a tap as well (a swipe first seen already far from its landing never set the moved bit).
                        orb = Some((what | 0x80, x0, y0, true));
                        if what & 0x7F == 9 {
                            helm_light(display, 9, 1.0);
                        }
                        match what & 0x7F {
                            1 => a.net_pull(),
                            9 if a.can_go_back() => a.go_back(),
                            // The other radios have no page before attest.
                            _ => {}
                        }
                        a.show();
                    } else if (x - x0).abs() + (y - y0).abs() > (a.w / 64) as i64 {
                        // Moved, not yet far: no longer a tap.
                        orb = Some((what | 0x80, x0, y0, false));
                    }
                }
                _ => {}
            }
        }
        // The helm's back button let go: its light out (the display fades it).
        if k == 0 && orb.is_some_and(|o| o.0 & 0x7F == 9) {
            helm_light(display, 9, 0.0);
        }
        if k == 0 {
            match orb {
                // The back corner tapped still: back.
                Some((9, _, _, false)) if a.can_go_back() => {
                    a.go_back();
                    a.show();
                }
                // A radio's circle tapped still: that radio off or on (preference 15, as the map does after attest; owner, 2026-11-09: the radios switch on the attest screen too). The display redraws the circle from the preference.
                Some((r, _, _, false)) if r < 5 => {
                    let off = ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF) ^ (1 << r);
                    ferros_sys::set_pref(cap(grants, S_SETTINGS), ferros_sys::PREF_RADIOS_OFF, off);
                    let mut l = Line::new();
                    let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: status circle {r} tapped: radios off G#{off:02X}\n"));
                    console(grants, l.as_bytes());
                }
                _ => {}
            }
            orb = None;
            orb_asked = false;
            pulled = false;
            a.release_words();
        }
        if k == 0 {
            if pinch.take().is_some() {
                ferros_sys::set_pref(cap(grants, S_SETTINGS), ferros_sys::PREF_SCALE, libm::roundf(a.scale.get()) as i8 as u8);
            }
            pinched = false;
        }
        let mut keys = [(0u8, 0i64, 0i64); 16];
        let mut nk = 0;
        for &f in &held[..k] {
            if !pinched {
                keys[nk] = f;
                nk += 1;
            }
        }
        let mut outs = [Touch::Scrolled; 48];
        let mut no = 0;
        touches.poll(&mut a.kb, &keys[..nk], &mut |x, y| window::mine(display, base, x as u16, y as u16), &mut |t| {
            outs[no] = t;
            no += 1;
        });
        // A key let go without its tap (the finger slid off) is drawn at rest again (a tap redraws the whole board, so a lift does not) and leaves the typed handle as it was: its preview gives way to the typed handle again, decided after the whole poll, since a lift reports the key's Unlight just before its Tap.
        let mut slid = None;
        for &t in &outs[..no] {
            match t {
                Touch::Light(key) => {
                    // The lit key on the glass first: preparing a whole board takes some 50 ms.
                    let t0 = now();
                    a.light(key, true);
                    let mut l = Line::new();
                    let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: key lit in {} us\n", ferros_sys::us_since(t0)));
                    console(a.grants, l.as_bytes());
                    a.prepare(key);
                    if COMMIT_AT_PRESS {
                        // The stroke at the press: the handle, the keys' rainbow and the mark all move now; the lift only lets the key go.
                        let t0 = now();
                        a.tap(key);
                        a.light(key, true);
                        pressed = Some(key);
                        let mut l = Line::new();
                        let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: typed at the press in {} us (retint {}, board {}, tint {}, present {})\n", ferros_sys::us_since(t0), unsafe { LIFT_US[0] }, unsafe { LIFT_US[1] }, unsafe { LIFT_US[2] }, unsafe { LIFT_US[3] }));
                        console(a.grants, l.as_bytes());
                    } else {
                        // The mark previews the key under the finger, shown once the lift makes it the typed handle.
                        a.preview(key);
                    }
                }
                Touch::Unlight(key) => {
                    slid = Some(key);
                }
                Touch::Scrolled => {
                    a.show_board();
                }
                // Already typed at its press: the lift lets the key go.
                Touch::Tap(key) if pressed == Some(key) => {
                    pressed = None;
                    a.light(key, false);
                    slid = None;
                }
                Touch::Tap(key) => {
                    let t0 = now();
                    a.tap(key);
                    let mut l = Line::new();
                    let _ = core::fmt::Write::write_fmt(&mut l, format_args!("ahau: lift drawn in {} us (retint {}, board {}, tint {}, present {})\n", ferros_sys::us_since(t0), unsafe { LIFT_US[0] }, unsafe { LIFT_US[1] }, unsafe { LIFT_US[2] }, unsafe { LIFT_US[3] }));
                    console(a.grants, l.as_bytes());
                    slid = None;
                }
                Touch::TapAt(x, y) if !pinched => a.tap_at(x, y),
                Touch::TapAt(..) => {}
                // A pull down from the mark's upper half opens the network page, a pull up on the page closes it (an eighth of the width, once per touch); a drag on the entry line pans it (TEXT.md).
                Touch::Drag(x0, y0, x, y) if !pinched => {
                    let (_, cy, _) = a.mark_at();
                    let reach = (a.w / 8) as i64;
                    if !pulled && !a.net_open() && y0 < cy as i64 && y - y0 > reach {
                        pulled = true;
                        a.net_pull();
                    } else if !pulled && a.mode == Mode::Networks && y0 < a.kb.top as i64 && y0 - y > reach {
                        pulled = true;
                        a.net_close();
                    } else if matches!(a.mode, Mode::Join(_)) && { let (b0, b1) = a.words_band(); (b0 as i64..b1 as i64).contains(&y0) } {
                        a.scroll_words(y0, y);
                    } else {
                        a.drag_text(x0, y0, x);
                    }
                }
                Touch::Drag(..) => {}
                // A handle is exact bytes, typed key by key: no shape writing here.
                Touch::Trail(..) | Touch::Traced => {}
            }
        }
        if let Some(key) = slid {
            if pressed == Some(key) {
                pressed = None;
            }
            a.light(key, false);
            publish_fresh_text(&a.handle[..a.len], true);
        }
        // Following the identity service's verdict while the identity is looked up or joined: `client_thread`'s copy, read when it moved (no call here).
        polls += 1;
        let followed = !a.net_open() && a.follow_client();
        if polls % 64 == 0 && a.net_open() && a.net_poll() {
            a.show();
        }
        if a.follow_layout() || no > 0 || followed {
            a.show();
        }
        // The words flinging after a lift: stepped once per frame the panel started since the last step (display op 24 waits for the next and gives its count), as the settings pages are.
        if a.words_fling.get() != 0. {
            let mut fr = [0u8; 4];
            call(display, &[24], &mut fr);
            let f = u32::from_le_bytes(fr);
            let steps = a.words_frame.get().map_or(1, |p| f.wrapping_sub(p));
            a.words_frame.set(Some(f));
            for _ in 0..steps {
                if !a.step_words() {
                    break;
                }
            }
            a.show();
            continue;
        }
        sleep_until(now() + freq() / 240);
    }
}

/// A console line in a fixed buffer.
struct Line {
    b: [u8; 256],
    n: usize,
}

impl Line {
    fn new() -> Line {
        Line { b: [0; 256], n: 0 }
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

/// The wallpaper seed to the display server (its op 42, `wallpaper`): it makes the person's wallpaper from it, or shows their own.
fn send_wallpaper(display: u64, ws: &[u8; 32]) {
    let mut m = [0u8; 33];
    m[0] = 42;
    m[1..].copy_from_slice(ws);
    let mut r = [0u8; 16];
    let _ = ferros_sys::call(display, &m, &mut r);
}
/// The keyboard's bar divided over the glass the eye sees (display op 53: the glass's columns on a row), averaged over the bar's top and foot rows (owner, 2026-11-08: "go off center intersect of squircle for divisions"). A display without the op leaves the screen's width.
fn fit_bar(display: u64, kb: &mut honeycomb::Keyboard) {
    let (top, foot) = kb.bar_rows();
    let mut span = [0f32; 2];
    for y in [top, foot] {
        let mut r = [0u8; 8];
        if ferros_sys::call(display, &[53, y as u8, (y >> 8) as u8], &mut r) < 4 {
            return;
        }
        span[0] += u16::from_le_bytes([r[0], r[1]]) as f32 * 0.5;
        span[1] += u16::from_le_bytes([r[2], r[3]]) as f32 * 0.5;
    }
    kb.fit_bar(span[0], span[1]);
}

/// A network's name as the page shows it: printable ASCII kept, anything else a `?` (the driver joins ASCII names only; the bytes of others are not trusted to be text).
/// Whether WiFi is joined: its link's STATE measure 2 (the driver's, facts 41-48), so the page says the network kept is connected, not merely kept (owner, 2026-11-09: "it just says pangolin kept, nothing about connected").
fn wifi_joined() -> bool {
    let f = ferros_sys::facts();
    f.0[33 + 8..41 + 8].iter().filter_map(|&m| ferros_sys::measure_of(m)).any(|m| m.0 == ferros_sys::measure::STATE && m.1 == 2)
}

fn printable(name: &[u8]) -> ([u8; 35], usize) {
    // Three bytes past the longest name: room for the "..." of one cut short (`net_draw`).
    let mut t = [0u8; 35];
    for (o, &c) in t.iter_mut().zip(name) {
        *o = if (0x20..0x7F).contains(&c) { c } else { b'?' };
    }
    (t, name.len())
}

/// Board pixel `i` in the handle's tints: its grey, each channel moved toward the key's tint by the pixel's weight (a label keeps its own grey), as working values, premultiplied (the key's edge already holds only its share).
fn board_px(b: &Board, tint: &[[u16; 3]; 64], i: usize) -> u64 {
    let one = 1i64 << 14;
    let (p, wt, c) = (b.base[i] as i64, b.weight[i] as i64, &tint[b.index[i] as usize]);
    let ch = |c: u16| (p * (one + (wt * (c as i64 - one) >> 14)) >> 14) as u64;
    ch(c[0]) | ch(c[1]) << 16 | ch(c[2]) << 32
}

/// A board pixel's see-through part (`Board::clear`, 0 to 255) as the display's working transparency: Q14 in bits 49-62, or bit 63 when clear through.
fn clear_bits(c: u8) -> u64 {
    if c == 255 { 1 << 63 } else { ((c as u64) << 6) << 49 }
}

/// A helm button's light (display op 54): `corner` 8 home or 9 back, `level` 0 to 1.
fn helm_light(display: u64, corner: u8, level: f32) {
    let q = ((level * 65535.0) as u16).to_le_bytes();
    let mut r = [0u8; 4];
    let _ = call(display, &[54, corner, q[0], q[1]], &mut r);
}
