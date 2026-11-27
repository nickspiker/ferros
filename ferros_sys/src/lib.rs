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
/// `spawn_kist`'s answer for a kist already running on a surface: this bit, the surface in bits 8-15 and its generation in bits 0-7, so its window can be brought to the front (display op 59, `window::front`).
pub const RUNNING_ON: u64 = 1 << 62;
/// OR'd into a `call`'s reply length: fail with ERR instead of waiting when no server is receiving on the endpoint (none alive, or one still bringing its device up and not yet receiving; now, or once its last server ends). Without it a call waits for a server, as across a restart (process.rs CALL_LIVE).
pub const CALL_LIVE: u64 = 1 << 63;
/// A message too long for the buffer it would land in (`recv_long`), or a reply too long for the caller's (`call_long`): this bit and the whole length (docs/NO-CEILINGS.md 1.0, owner decision 5: "too long", never a silent cut). Its top two bits are 01, so it is never ERR, NOTHING or a fault; `too_long` reads it.
pub const TOO_LONG: u64 = 1 << 62;
/// A call's reply length flag (x4 bit 61): tell this caller a reply longer than its buffer (`call_long`).
pub const CALL_LONG: u64 = 1 << 61;
/// A receive's flag (x4 bit 1): tell this server a message longer than its buffer (`recv_long`).
pub const RECV_LONG: u64 = 1 << 1;

/// The whole length when `r` (a `recv_long` or `call_long` answer) says "too long".
#[inline(always)]
pub const fn too_long(r: u64) -> Option<usize> {
    if r >> 62 == 1 { Some((r & !TOO_LONG) as usize) } else { None }
}

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
    pub const ZONE: u64 = 35;
    pub const LANG: u64 = 36;
    pub const CLAIM: u64 = 37;
    pub const BELL: u64 = 38;
    pub const NOT_STARTED: u64 = 39;
    pub const ARC: u64 = 40;
    pub const OFFER: u64 = 41;
    pub const CALLER: u64 = 42;
    pub const NOTIFY: u64 = 43;
    pub const THREAD_NAME: u64 = 44;
    pub const IRQ_ARM: u64 = 45;
    pub const PANIC: u64 = 46;
    pub const MAILBOX: u64 = 47;
    pub const BASE: u64 = 48;
    pub const SHARE: u64 = 49;
}

/// Notes: what a kist has waiting on the person (kernel SYS_NOTIFY, docs/NOTIFY.md; owner, 2026-11-12: "I think we try it with the in view, it can notify me, out of view, silent"). A note is a VSF document (ferros_kist::note). A kist posts and withdraws its own, with no capability (the kernel knows it by its capsule); the deck (a Settings holder that is not a kist) lists them, rings on them and clears them as the person looks.
pub mod notify {
    use super::{nr, off, svc, ERR, NOTHING};

    /// A record of `list`: [source 32][number u8][kind u8][asked u8][flags u8: bit 0 names a person, bit 1 has a target][count u32 LE][seq u64 LE: the notes' bell count it was posted at][deadline i64 LE: Eagle oscillations, 0 none].
    pub const REC: usize = 56;
    /// Every note of a source (`withdraw`, `seen`).
    pub const ALL: u8 = 0xFF;

    /// Why a post was refused.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Refused {
        /// Nobody present, or the kist has as many notes waiting as it may, or has posted as many as it may since the person last looked at it: post again once they have.
        Full,
        /// Not a note (ferros_kist::note), or the caller is neither a kist nor the system.
        NotANote,
    }

    fn answer(r: u64) -> Result<(u8, u8), Refused> {
        match r {
            ERR => Err(Refused::NotANote),
            NOTHING => Err(Refused::Full),
            r => Ok((r as u8, (r >> 8) as u8)),
        }
    }

    /// Post this kist's note `doc`: its number (0-15) and the kind the system gave it (ferros_kist::note STATUS, PERSONAL, LIVE: never above the one asked). A note whose `key` matches one of this kist's waiting replaces it under the same number.
    pub fn post(doc: &[u8]) -> Result<(u8, u8), Refused> {
        answer(svc(nr::NOTIFY, 0, off(doc.as_ptr()), doc.len() as u64, 0, 0))
    }
    /// The system's note on `source`'s behalf, given Settings (the deck's development notes): as `post`.
    pub fn post_for(settings: u64, source: &[u8; 32], doc: &[u8]) -> Result<(u8, u8), Refused> {
        answer(svc(nr::NOTIFY, 0, off(doc.as_ptr()), doc.len() as u64, settings, off(source.as_ptr())))
    }
    /// This kist's note `number` gone (`ALL`: every one of its): no longer true, or seen in the kist itself. How many went.
    pub fn withdraw(number: u8) -> u64 {
        svc(nr::NOTIFY, 1, number as u64, 0, 0, 0)
    }
    /// The note the person opened (the display's `EV_OPEN_AT` names its number) into `buf`: its document's length (written only when it fits), None when the note opened last is not that one.
    pub fn opened(number: u8, buf: &mut [u8]) -> Option<usize> {
        let r = svc(nr::NOTIFY, 2, number as u64, off(buf.as_mut_ptr()), buf.len() as u64, 0);
        (r != ERR && r != NOTHING).then_some(r as usize)
    }
    /// The notes waiting into `buf` (whole `REC` records, as many as fit), given Settings: how many there are.
    pub fn list(settings: u64, buf: &mut [u8]) -> Option<usize> {
        let r = svc(nr::NOTIFY, 3, settings, off(buf.as_mut_ptr()), buf.len() as u64, 0);
        (r != ERR).then_some(r as usize)
    }
    /// The notes' doorbell, given Settings: the kernel writes its change count into `word` now and whenever a note is posted, revised, withdrawn, seen or past its deadline, waking whatever waits on it (`wait_world`).
    pub fn doorbell(settings: u64, word: &core::sync::atomic::AtomicU64) -> bool {
        svc(nr::NOTIFY, 4, off(word as *const _ as *const u8), settings, 0, 0) == 0
    }
    /// The person looked at `source`: its note `number` (`ALL`: every one) cleared; `opened`, they tapped that note, kept for its kist to read where to land. How many went.
    pub fn seen(settings: u64, source: &[u8; 32], number: u8, opened: bool) -> Option<u64> {
        let r = svc(nr::NOTIFY, 5, settings, off(source.as_ptr()), number as u64, opened as u64);
        (r != ERR).then_some(r)
    }
    /// `source`'s note `number` as its document into `buf`, given Settings: its length (written only when it fits), None for no such note.
    pub fn read(settings: u64, source: &[u8; 32], number: u8, buf: &mut [u8]) -> Option<usize> {
        let mut req = [0u8; 33];
        req[..32].copy_from_slice(source);
        req[32] = number;
        let r = svc(nr::NOTIFY, 6, settings, off(req.as_ptr()), off(buf.as_mut_ptr()), buf.len() as u64);
        (r != ERR && r != NOTHING).then_some(r as usize)
    }
}

/// The caller's own arc, grown or shrunk on demand (kernel SYS_ARC, docs/ARCS.md step 2; owner, 2026-11-11: "why we cannot scale on demand, that was the whole point of ring memory"). No capability: a process reaches only its own arc, and only up to its ceiling (the heap its build declared, counted whole; fixed mode's size). A top is an arc offset: the arc's size, or exact mode's limit.
pub mod arc {
    use super::{nr, ERR};
    use core::arch::asm;
    use core::sync::atomic::{compiler_fence, Ordering};

    /// Why the kernel refused (process.rs `ARC_WHY_*`): past the ceiling, too few free frames, (`WHY_BUSY`, no longer answered: a masked arc whose other threads run elsewhere is held until they leave, kernel step 3), under what the process was spawned with, over a word the kernel holds, no such op.
    pub const WHY_CEILING: u64 = 1;
    pub const WHY_MEMORY: u64 = 2;
    pub const WHY_BUSY: u64 = 3;
    pub const WHY_FLOOR: u64 = 4;
    pub const WHY_HELD: u64 = 5;
    pub const WHY_OP: u64 = 6;

    /// The trap with both result registers: (x0, x1).
    fn call(op: u64, n: u64) -> (u64, u64) {
        let (a, b): (u64, u64);
        compiler_fence(Ordering::SeqCst);
        unsafe { asm!("svc #0", inlateout("x0") op => a, inlateout("x1") n => b, in("x8") nr::ARC, options(nostack)) };
        compiler_fence(Ordering::SeqCst);
        (a, b)
    }

    /// At least `bytes` more: Ok((the new top, the old top)); the bytes between are new, zeroed and the caller's. Err(the reason).
    pub fn grow(bytes: usize) -> Result<(usize, usize), u64> {
        match call(0, bytes as u64) {
            (ERR, why) => Err(why),
            (top, was) => Ok((top as usize, was as usize)),
        }
    }

    /// Keep the first `bytes` of the arc (rounded up as its mode needs, never under what it was spawned with) and give the rest back: Ok((the new top, the old top)). Nothing past the new top may be touched again.
    pub fn shrink(bytes: usize) -> Result<(usize, usize), u64> {
        match call(1, bytes as u64) {
            (ERR, why) => Err(why),
            (top, was) => Ok((top as usize, was as usize)),
        }
    }

    /// (the top now, the ceiling).
    pub fn size() -> (usize, usize) {
        let (a, b) = call(2, 0);
        (a as usize, b as usize)
    }

    /// (the bytes the device's frame pool has free now, the pool's bytes), kernel ARC_FREE (docs/NO-CEILINGS.md 0.5): read once when sizing a large ask or saying why one was refused, never on a timer. None from a kernel before it.
    pub fn free() -> Option<(usize, usize)> {
        match call(6, 0) {
            (ERR, _) => None,
            (a, b) => Some((a as usize, b as usize)),
        }
    }

    /// Give back the 2 MiB frames wholly inside `len` bytes from arc offset `off` while the arc keeps its size (kernel ARC_RELEASE, docs/ARCS.md step 4): what a process no longer needs in the middle of its arc (ahau's attest mark after attest). Those bytes must never be touched again (a fault ends the process) until a grow over them; no syscall buffer may lie there. Never the arc's first frame. Ok(the bytes given back), Err(the reason).
    pub fn release(off: u64, len: usize) -> Result<usize, u64> {
        let (a, b): (u64, u64);
        compiler_fence(Ordering::SeqCst);
        unsafe { asm!("svc #0", inlateout("x0") 3u64 => a, inlateout("x1") off => b, in("x2") len as u64, in("x8") nr::ARC, options(nostack)) };
        compiler_fence(Ordering::SeqCst);
        if a == ERR { Err(b) } else { Ok(a as usize) }
    }
}

/// The clock's zone (kernel zone.rs, SYS_ZONE; docs/TIME.md; owner, 2026-11-10): the identity's choice and this device's own, each automatic (the zone found from the cellular network, then GPS) or by hand (an IANA id), and the switch that says which rules.
pub mod zone {
    use super::{nr, off, svc, ERR};

    /// The longest id the kernel holds, and the state's length (kernel zone::ID_MAX, STATE_LEN).
    pub const ID_MAX: usize = 1 << 6;
    pub const STATE_LEN: usize = 12 + 4 * (1 + ID_MAX);
    /// Where the zone in effect comes from (`State::from`).
    pub const FROM_OFFSET: u8 = 0;
    pub const FROM_HAND: u8 = 1;
    pub const FROM_NETWORK: u8 = 2;
    pub const FROM_POSITION: u8 = 3;
    /// The identity's choice as written to this device (`State::identity`).
    pub const IDENTITY_NONE: u8 = 0;
    pub const IDENTITY_HAND: u8 = 1;
    pub const IDENTITY_AUTO: u8 = 2;

    /// An IANA id as the kernel holds it; empty for none.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Id {
        pub b: [u8; ID_MAX],
        pub n: u8,
    }

    impl Id {
        pub fn as_bytes(&self) -> &[u8] {
            &self.b[..self.n as usize]
        }
        pub fn as_str(&self) -> &str {
            core::str::from_utf8(self.as_bytes()).unwrap_or("")
        }
        pub fn is_none(&self) -> bool {
            self.n == 0
        }
    }

    /// The zone's state (SYS_ZONE op 5).
    #[derive(Clone, Copy)]
    pub struct State {
        /// This device follows the identity's choice.
        pub follow: bool,
        /// This device's own choice is automatic.
        pub device_auto: bool,
        /// The identity's choice as last written here: IDENTITY_NONE, _HAND or _AUTO.
        pub identity: u8,
        /// Where the zone in effect comes from (FROM_*).
        pub from: u8,
        /// The zones document has been read (no list, and no zone by id, before).
        pub table: bool,
        /// The cellular network's country (ISO 3166 alpha-2) and MCC, when heard.
        pub country: Option<[u8; 2]>,
        pub mcc: u16,
        /// The zone in effect, this device's by hand, the identity's, and the one found here.
        pub effective: Id,
        pub device: Id,
        pub identity_id: Id,
        pub found: Id,
    }

    impl State {
        /// The choice that rules this device: (automatic, its id by hand), the identity's while following it and it has one, else this device's.
        pub fn ruling(&self) -> (bool, Id) {
            if self.follow && self.identity != IDENTITY_NONE { (self.identity == IDENTITY_AUTO, self.identity_id) } else { (self.device_auto, self.device) }
        }
    }

    /// The zone's state now; None when the kernel has no zone call (an older kernel).
    pub fn state() -> Option<State> {
        let mut b = [0u8; STATE_LEN];
        if svc(nr::ZONE, 5, off(b.as_mut_ptr() as *const u8), b.len() as u64, 0, 0) != STATE_LEN as u64 {
            return None;
        }
        let id = |k: usize| {
            let at = 12 + k * (1 + ID_MAX);
            // The length byte is the kernel's, never past ID_MAX (zone::Id): a kernel that wrote more is broken, and the copy panics.
            let mut z = Id { b: [0; ID_MAX], n: b[at] };
            z.b[..z.n as usize].copy_from_slice(&b[at + 1..at + 1 + z.n as usize]);
            z
        };
        Some(State { follow: b[0] == 1, device_auto: b[1] == 1, identity: b[2], from: b[3], table: b[5] == 1, country: (b[6] != 0).then_some([b[6], b[7]]), mcc: u16::from_le_bytes([b[8], b[9]]), effective: id(0), device: id(1), identity_id: id(2), found: id(3) })
    }

    /// Every zone's id into `buf` (each its length, a byte, then its bytes; zone1970.tab's order): the list's whole length (0 before the zones document is read).
    pub fn list(buf: &mut [u8]) -> usize {
        let r = svc(nr::ZONE, 6, off(buf.as_mut_ptr()), buf.len() as u64, 0, 0);
        if r == ERR { 0 } else { r as usize }
    }

    /// This device's own choice, given the Settings capability: automatic, and its zone by hand (empty: none, the fixed offset). True when taken and kept.
    pub fn set_device(cap: u64, auto: bool, id: &[u8]) -> bool {
        svc(nr::ZONE, 2, cap, auto as u64, off(id.as_ptr()), id.len() as u64) == 0
    }

    /// Whether this device follows the identity's choice, given the Settings capability.
    pub fn set_follow(cap: u64, on: bool) -> bool {
        svc(nr::ZONE, 3, cap, on as u64, 0, 0) == 0
    }

    /// The identity's choice written to this device (the identity service only, payloads/fgtw): None forgets it, else (automatic, its id by hand).
    pub fn set_identity(choice: Option<(bool, &[u8])>) -> bool {
        match choice {
            None => svc(nr::ZONE, 4, 2, 0, 0, 0) == 0,
            Some((auto, id)) => svc(nr::ZONE, 4, auto as u64, off(id.as_ptr()), id.len() as u64, 0) == 0,
        }
    }

    /// The cellular network's word (the modem only): its MCC (0 none), and NITZ's legal offset (seconds east) with whether daylight time is in it, when it gave them.
    pub fn network(mcc: u16, offset: Option<(i32, Option<bool>)>) -> bool {
        let (o, flags) = match offset {
            None => (0, 0),
            Some((o, None)) => (o, 1),
            Some((o, Some(d))) => (o, 1 | 2 | (d as u64) << 2),
        };
        svc(nr::ZONE, 0, mcc as u64, o as i64 as u64, flags, 0) == 0
    }

    /// A new position (the GPS driver only), in 2^-23 degrees: the kernel finds the zone again once it has moved some 30 km.
    pub fn position(lat: i32, lon: i32) -> bool {
        svc(nr::ZONE, 1, lat as i64 as u64, lon as i64 as u64, 0, 0) == 0
    }

    /// The zone in effect's principal place (the GPS driver only; the zones document's city, never anyone's place): latitude and longitude in arc-seconds. None with no zone in effect (a fixed offset), no zones document, or an older kernel.
    pub fn place() -> Option<(i32, i32)> {
        let mut b = [0u8; 8];
        (svc(nr::ZONE, 7, off(b.as_mut_ptr()), 0, 0, 0) == 8).then(|| (i32::from_le_bytes([b[0], b[1], b[2], b[3]]), i32::from_le_bytes([b[4], b[5], b[6], b[7]])))
    }
}
/// Core masks for `pin` (bit = core index on husky): the A715 workers, where processes run by default, the X3, the kernel's core, for one real compute path, and the A510s, which run only threads pinned there (2026-11-18: work that should leave the A715s free, as Lumis's camera intake). A mask none of whose cores is up runs on any core that is.
pub const CORES_WORKERS: u64 = 0x00F0;
pub const CORES_BIG: u64 = 0x0100;
pub const CORES_LITTLE: u64 = 0x000F;

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

/// The second preferences block, 16 bytes under the first (kernel process.rs PREFS_HI): preference 32 + k at byte k. A kernel older than it leaves them zero, each one's default.
pub const PREFS_HI: u64 = 0xFB0;
/// Preferences 32-38, the language (`lang`): 32-33 the language in effect, 34-35 this device's own, 36-37 the identity's, each a packed ISO 639-3 code (0 none); 38 whether the device keeps its own (1) or follows the identity's (0).
pub const PREF_LANG: u64 = 32;
pub const PREF_LANG_DEVICE: u64 = 34;
pub const PREF_LANG_IDENTITY: u64 = 36;
pub const PREF_LANG_OWN: u64 = 38;
/// Preference 39: the sound mode, `SOUND`, `VIBRATE` or `SILENT` (the quick row's; owner, 2026-11-11). A device setting, set with `set_pref` thru the Settings capability.
pub const PREF_SOUND: u64 = 39;
pub const SOUND: u8 = 0;
pub const VIBRATE: u8 = 1;
pub const SILENT: u8 = 2;
/// Preference 40: do not disturb, the identity's (0 off, 1 on): written only by the identity service from the person's space (`dnd::set_identity`), read by everyone. To change it, a device asks the identity service (payloads/fgtw op 24), which puts it in the space and so on every device of the person's.
pub const PREF_DND: u64 = 40;
/// Preference 41: the radios that were off when "radios off" switched them all off (preference 15's bits), put back by its next tap.
pub const PREF_RADIOS_BEFORE: u64 = 41;
/// Preferences 42-45: the torch's settings (`torch`; docs/TORCH.md "Settings", QUICK.md "Quick pages"), device settings set thru the Settings capability: the level a tap lights it at, the strobe, the lit share of each flash, and when it turns itself off.
pub const PREF_TORCH_LEVEL: u64 = 42;
pub const PREF_TORCH_STROBE: u64 = 43;
pub const PREF_TORCH_DUTY: u64 = 44;
pub const PREF_TORCH_TIMEOUT: u64 = 45;

/// The torch's settings as the device keeps them (preferences 42-45; owner, 2026-11-11: "like flashlight we may want to control the brightness and strobe frequency and duty cycle and timeout"). One encoding for the map that sets them and the torch that obeys them, so the two cannot disagree. Every step is a power-of-two fraction of an octave, as brightness is 32nds of a stop: no decimal knob anywhere.
pub mod torch {
    use super::{pref, PREF_TORCH_DUTY, PREF_TORCH_LEVEL, PREF_TORCH_STROBE, PREF_TORCH_TIMEOUT};

    /// The strobe's steps an octave (32nds of an octave of frequency) and how many octaves it spans: code 1 a flash every 4 s, each further code a 32nd of an octave faster, the last (193) 16 a second.
    pub const STROBE_PER_OCTAVE: u8 = 1 << 5;
    pub const STROBE_OCTAVES: u8 = 6;
    /// The strobe's highest code: 0 is steady.
    pub const STROBE_STEPS: u8 = STROBE_OCTAVES * STROBE_PER_OCTAVE + 1;
    /// The slowest strobe's period: 4 s (a quarter of a hertz).
    const STROBE_SLOWEST_NS: u64 = 4_000_000_000;
    /// The lit share's reach either side of a half, in 256ths: 8/256 to 248/256 lit. WHY not the whole byte: at the fastest strobe (16 a second, 62.5 ms) the shortest flash is then 1.95 ms, past the LM3644's 1 ms torch ramp and the bus write that starts it (docs/TORCH.md "Strobe"); shorter would light dimmer than asked, or not at all.
    pub const DUTY_REACH: u8 = 120;
    /// The timeout's steps an octave (16ths of an octave of time) and how many octaves it spans: code 1 is 16 s, the last (161) 2^14 s, about 4 h 33 min.
    pub const TIMEOUT_PER_OCTAVE: u8 = 1 << 4;
    pub const TIMEOUT_OCTAVES: u8 = 10;
    /// The timeout's highest code: 0 is never.
    pub const TIMEOUT_STEPS: u8 = TIMEOUT_OCTAVES * TIMEOUT_PER_OCTAVE + 1;
    /// The shortest timeout: 16 s.
    const TIMEOUT_SHORTEST_NS: u64 = 16_000_000_000;

    /// 2^(i/32) for i in 0..32, Q30, computed at build time (the 32nd root of two by bisection, then its powers): no float at run time.
    const POW2_32: [u64; 32] = pow2_32();
    const fn pow2_32() -> [u64; 32] {
        // The root by 64 halvings of [1, 2] (past f64's precision): r^32 against 2.
        let (mut lo, mut hi) = (1.0f64, 2.0f64);
        let mut i = 0;
        while i < 64 {
            let mid = (lo + hi) / 2.;
            let mut p = 1.0f64;
            let mut k = 0;
            while k < 32 {
                p *= mid;
                k += 1;
            }
            if p < 2. {
                lo = mid;
            } else {
                hi = mid;
            }
            i += 1;
        }
        let mut t = [0u64; 32];
        let mut v = 1.0f64;
        let mut k = 0;
        while k < 32 {
            t[k] = (v * (1u64 << 30) as f64 + 0.5) as u64;
            v *= lo;
            k += 1;
        }
        t
    }

    /// The level a tap lights the torch at (preference 42): the last it was set to, 1-255 on its perceptual dimmer; the whole dimmer while it was never set (0).
    pub fn level() -> u8 {
        match pref(PREF_TORCH_LEVEL) {
            0 => 255,
            l => l,
        }
    }

    /// The strobe as kept (preference 43): 0 steady, else its code.
    pub fn strobe() -> u8 {
        pref(PREF_TORCH_STROBE)
    }
    /// The lit share as kept (preference 44): a signed byte, 256ths either side of a half (0, the default, a half).
    pub fn duty() -> u8 {
        pref(PREF_TORCH_DUTY)
    }
    /// The timeout as kept (preference 45): 0 never, else its code.
    pub fn timeout() -> u8 {
        pref(PREF_TORCH_TIMEOUT)
    }

    /// A strobe code's period in nanoseconds: None for 0 (steady). The kernel keeps the preference within `STROBE_STEPS` (it refuses anything past it), and any byte would still index the table in range (the fraction is below 32).
    pub fn strobe_period_ns(code: u8) -> Option<u64> {
        if code == 0 {
            return None;
        }
        let k = (code - 1) as u32;
        let (whole, frac) = (k / STROBE_PER_OCTAVE as u32, k % STROBE_PER_OCTAVE as u32);
        Some((((STROBE_SLOWEST_NS as u128) << 30) / POW2_32[frac as usize] as u128 >> whole) as u64)
    }

    /// The lit share of a flash, in 256ths, from the kept signed byte (`duty`): a half and the byte. The kernel keeps the preference within `DUTY_REACH` either side (it refuses anything past it).
    pub fn duty_256(code: u8) -> u32 {
        (128 + code as i8 as i32) as u32
    }

    /// A timeout code's length in nanoseconds: None for 0 (never). The kernel keeps the preference within `TIMEOUT_STEPS`; even the byte's top (15 octaves) is far inside a u128 here.
    pub fn timeout_ns(code: u8) -> Option<u64> {
        if code == 0 {
            return None;
        }
        let k = (code - 1) as u32;
        let (whole, frac) = (k / TIMEOUT_PER_OCTAVE as u32, (k % TIMEOUT_PER_OCTAVE as u32) * (32 / TIMEOUT_PER_OCTAVE as u32));
        Some((((TIMEOUT_SHORTEST_NS as u128) << whole) * POW2_32[frac as usize] as u128 >> 30) as u64)
    }
}

/// Do not disturb (preference 40; SETTINGS.md): the person's, carried by their identity space.
pub mod dnd {
    use super::{nr, pref, svc, PREF_DND};

    /// Whether the person has do not disturb on, as their identity service last wrote it here.
    pub fn on() -> bool {
        pref(PREF_DND) == 1
    }

    /// The identity service writes the person's choice to this device (the kernel takes it from the process named fgtw alone, as SYS_LANG op 2). True when taken and kept.
    pub fn set_identity(on: bool) -> bool {
        svc(nr::SET_PREF, 0, PREF_DND, on as u64, 0, 0) == 0
    }
}

/// Where a kist finds its grants (docs/KISTS.md "The implicit set"; ferros_kist `slots`, the one layout, which the kernel follows), for a kist whose capsule declares only the rights it is asked for, as every capsule does from 2026-11-12: the implicit set in fixed slots, never declared, never asked (owner, 2026-11-12: "Yes on the implicit set"), its asked wants after the first six, then the GPU's three (`gpu::kist_slots(LEADING + asked)`), then sound out. A right the person said no to is an empty slot, its token zeros. A capsule that still declares implicit rights gets each where it declares it, as before.
pub mod kist {
    /// The display endpoint (`use display`).
    pub const DISPLAY: u64 = 1;
    /// The touch endpoint (`use touch`).
    pub const TOUCH: u64 = 2;
    /// A surface of its own: its window.
    pub const SURFACE: u64 = 3;
    /// Its key sealed to this device (`ira_key`).
    pub const IRA: u64 = 4;
    /// Its own vault domain (`--store`'s grant).
    pub const DOMAIN: u64 = 5;
    /// Randomness.
    pub const ENTROPY: u64 = 6;
    /// The slots before the asked wants.
    pub const LEADING: u64 = 6;
    /// The slot of its asked want `i` (from 0, in the order it declares them).
    pub const fn asked(i: u64) -> u64 {
        LEADING + 1 + i
    }
    /// Sound out (`use audio`), after the GPU's three slots, for a kist declaring `asked` asked wants.
    pub const fn audio(asked: u64) -> u64 {
        LEADING + asked + 4
    }
}

/// The identity service's `facets` endpoint (docs/FACETS.md "To build" 2, 2026-11-13): a kist granted be you or people holds a send right on it; the identity service (payloads/fgtw) serves it, as the slot after its last.
pub mod facets {
    /// Op 0, be you: [0][the alias's public bundle's length u16 LE][the bundle (fgtw::pq::KeyBundle bytes)][the alias's possession eggs (fgtw::binding::possession)] → [`OK`][the handle proof 32][the pinned identity key 32][the binding, a complete VSF file (fgtw::binding::Binding)], or [a refusal below]. Held until the binding is signed (once a launch: the service keeps it).
    pub const BE_YOU: u8 = 0;
    /// Op 1, the people: [1] → [`OK`][the people, a complete VSF file (fgtw::people::doc)], all of them or the chosen ones, or [a refusal below].
    pub const PEOPLE: u8 = 1;
    /// Op 2, wait for be you (2026-11-14): [2][the count last answered u32 LE] → [`OK`][count u32 LE][ready u8], at once when the count moved on since the caller's, else held and answered the moment it does: when the identity service comes to hold what be you needs (a fleet it is a member of, the person key handed to the kernel, so the caller's alias exists) or loses it (a lock). Ready 1 means op 0 would be answered now. Refused [`NOT_GRANTED`] at once when the person did not grant be you (the grant is the launch's and never changes under a running kist). A kist that started before attest asks this instead of asking again on a clock (owner, 2026-11-10: "No polling!").
    pub const WAIT: u8 = 2;
    /// The answer's first byte: done.
    pub const OK: u8 = 0;
    /// Refused: the person did not grant this facet to the caller (or the caller is no kist).
    pub const NOT_GRANTED: u8 = 1;
    /// Refused: nobody is present, or the identity service has no identity or no fleet in hand yet.
    pub const NOT_PRESENT: u8 = 2;
    /// Refused: no alias (the person key not handed to the kernel yet), or the bundle is not the alias the kernel stamped, or its eggs do not hold it.
    pub const NOT_ALIAS: u8 = 3;
    /// Refused: a malformed request.
    pub const MALFORMED: u8 = 4;
    /// The kernel's stamp's length (process.rs `facets_stamp`).
    pub const STAMP: usize = 97;

    /// The kernel's stamp ahead of every call on `facets`: [flags: bit 0 a kist, bit 1 be you granted, bit 2 people granted, bit 3 the person present, bit 4 its alias held][developer 32][option 32][the alias's Ed25519 key 32]. Written by the kernel for every call, so the service trusts it and nothing after it.
    #[derive(Clone, Copy)]
    pub struct Stamp {
        pub kist: bool,
        pub be_you: bool,
        pub people: bool,
        pub present: bool,
        pub alias: Option<[u8; 32]>,
        pub developer: [u8; 32],
        pub option: [u8; 32],
    }

    impl Stamp {
        /// The stamp and the caller's own bytes after it; None for a message shorter than a stamp (never one the kernel wrote).
        pub fn read(msg: &[u8]) -> Option<(Stamp, &[u8])> {
            if msg.len() < STAMP {
                return None;
            }
            let f = msg[0];
            let k = |at: usize| -> [u8; 32] {
                let mut o = [0u8; 32];
                o.copy_from_slice(&msg[at..at + 32]);
                o
            };
            let s = Stamp { kist: f & 1 != 0, be_you: f & 2 != 0, people: f & 4 != 0, present: f & 8 != 0, alias: (f & 16 != 0).then(|| k(65)), developer: k(1), option: k(33) };
            Some((s, &msg[STAMP..]))
        }
    }
}

/// The GPU as every client reaches it (docs/GPU-KISTS.md plan step 1, 2026-11-12): what the kernel puts ahead of each call on the `gpu` endpoint, and where a kist finds its GPU grants.
pub mod gpu {
    /// The GPU service's view of a client's own block of the layer arena: offset 0 of the block (payloads/gpu `ARENA_VA`). A client names its memory to its kernels as `ARENA_VA` plus an offset into its own block; nothing else of the arena is mapped for it.
    pub const ARENA_VA: u64 = 0x8000_0000;
    /// The apps' surfaces in a client's GPU view: a surface it holds at `SURFACES_VA` plus its address's distance from `SURFACES_PA`; a surface it does not hold is not mapped. `SURFACES_PA` was the four surfaces' physical base until 2026-11-12, and stays the point the view is reckoned from (Mahere's build computes it so): a surface's address is now in the surfaces' window (`super::SURFACES_WINDOW`), its frames anywhere in the pool, so slot k's view is G#50_0000_0000 plus k times 32 MiB.
    pub const SURFACES_VA: u64 = 0xC000_0000;
    pub const SURFACES_PA: u64 = 0xCA00_0000;
    /// The reply's status bytes past [status][µs u32] when the job reached outside the client's memory (status `OUTSIDE`): [the GPU address u64][the GPU MMU's fault status u32].
    pub const OUTSIDE: u8 = 4;
    /// Status 5: a kist's job on a production build (its GPU code is unchecked: development builds only, until the checked kernel form).
    pub const UNCHECKED: u8 = 5;

    /// A kist's GPU grants by slot (the kernel's `kist_gpu`; ferros_kist `slots`): with `wants` slots before them (1 to `wants`), the GPU service at `wants + 1`, its block of the layer arena at `wants + 2`, a second surface for a mailbox at `wants + 3`. For a kist that declares only asked rights (`super::kist`), `wants` is `kist::LEADING` plus how many it declares; for a capsule that declares implicit rights too (as every capsule did before 2026-11-12), how many it declares. Each may be an empty slot (none free, or a production build): its token reads as zeros.
    pub const fn kist_slots(wants: u64) -> (u64, u64, u64) {
        (wants + 1, wants + 2, wants + 3)
    }

    /// The most regions a stamp names that the service maps (a client holds a block and a surface or two).
    pub const REGIONS: usize = 8;
    /// The most frames a stamp carries: four surfaces of 16 frames and a block at the ceiling (`super::GPU_BLOCK_MOST`).
    pub const FRAMES: usize = 64 + super::GPU_BLOCK_MOST / FRAME as usize + CAMERA_VIEW_BYTES as usize / FRAME as usize;
    /// A frame: what a surface's and a GPU block's physical memory comes in (the kernel's frames.rs FRAME), 16 to a surface.
    pub const FRAME: u64 = 1 << 21;

    /// The kernel's stamp ahead of a request on `gpu` (process.rs `gpu_stamp`): [the caller's epoch u64][flags u8: bit 0 a kist, bit 1 a production build, bit 2 frames follow][n u8][n × (base u64, size u64)], every GPU-reachable region the caller holds, then (bit 2, since 2026-11-12) for each region in a window of frames (`in_window`: a surface, or since 2026-11-13 a GPU block), in order, its frames' physical addresses (u64 each, its size over `FRAME` of them), then (bit 3, since 2026-11-21, the display alone) its view (`View`). The kernel writes it for every call, so the service trusts it and nothing after it.
    /// The frames stay in the message's bytes (`frames_of` reads them): a block at the ceiling carries 256, more than a fixed array beside each client is worth.
    #[derive(Clone, Copy)]
    pub struct Stamp<'a> {
        pub epoch: u64,
        pub kist: bool,
        pub production: bool,
        pub n: usize,
        pub regions: [(u64, u64); REGIONS],
        /// The frames' bytes, 8 a frame, in the order of their regions (`frames_of`).
        frames: &'a [u8],
        /// The display's view (flags bit 3, GPU-COMPOSE.md 2.1, 2026-11-21): only for the caller that holds the display's phases (the display), whose stamp the kernel writes with them and with the windows it composes, read-only.
        pub view: Option<View<'a>>,
    }

    /// The display's GPU view as its stamp names it (flags bit 3): [0 u8, so an older service takes the section for a malformed request and refuses it][seq u64][the phases' base u64][their bytes u64][m u8][m × (surface slot u8, generation u8)][m × 16 frames u64, each surface's in order].
    /// The phases are mapped read-write at `PHASES_VA` (fixed memory: the base is their physical address, side by side); each named surface read-only at its place from `SURFACES_VA`, as its owner's space has it. Nothing else of any window.
    /// `seq` numbers the display's stamps (the kernel's count): the service tells the kernel which stamp its installed tables came from (SYS_DMA x1 = 10 on `arena`), and only a surface no stamp since names and no table maps goes back to the pool (GPU-COMPOSE.md 2.2).
    #[derive(Clone, Copy)]
    pub struct View<'a> {
        pub seq: u64,
        pub phases: (u64, u64),
        /// Bit k: surface slot k named read-only.
        pub ro: u32,
        /// Slot k's generation in bits 4k to 4k + 3.
        pub ro_gen: u64,
        /// The named surfaces' frames, 16 a surface, in slot order, 8 bytes a frame.
        frames: &'a [u8],
    }

    impl<'a> View<'a> {
        /// The frames of read-only surface slot `k` (a slot `ro` names): its 16 physical addresses in order; none for any other slot.
        pub fn frames_of(&self, k: usize) -> impl Iterator<Item = u64> + 'a {
            let named = k < super::SURFACE_SLOTS && self.ro >> k & 1 == 1;
            let at = if named { (self.ro & ((1u32 << k) - 1)).count_ones() as usize * SURFACE_FRAMES } else { 0 };
            let bytes: &'a [u8] = if named { &self.frames[8 * at..8 * (at + SURFACE_FRAMES)] } else { &[] };
            bytes.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        }
    }

    /// A surface's frames.
    pub const SURFACE_FRAMES: usize = super::SURFACE_BYTES / FRAME as usize;
    /// Where the display's GPU space maps the four phases, read-write (GPU-COMPOSE.md 1.2): phase k at `PHASES_VA` plus k times its bytes, as the CPU sees them from the region's base.
    pub const PHASES_VA: u64 = 0x2_0000_0000;
    /// The most bytes of phases the service maps.
    pub const PHASES_MOST: u64 = 64 << 20;

    /// What names a client's GPU memory (`Stamp::key`): its epoch and its regions, and for the display the windows it views read-only (their slots and generations). A surface's frames never change while it is held, and a block's only as it grows (the kernel appends frames, its size in its region), so the same key is the same frames.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Key {
        pub epoch: u64,
        pub kist: bool,
        pub n: usize,
        pub regions: [(u64, u64); REGIONS],
        pub display: bool,
        pub ro: u32,
        pub ro_gen: u64,
    }

    /// Whether a region lies in a window of frames: a surface (the surfaces' window) or a GPU block (the blocks' window, since 2026-11-13), whose frames the stamp carries.
    pub const fn in_window(b: u64, size: u64) -> bool {
        (b >= super::SURFACES_WINDOW && b + size <= super::SURFACES_WINDOW + (super::SURFACE_SLOTS * super::SURFACE_BYTES) as u64) || (b >= super::GPU_BLOCKS_WINDOW && b + size <= super::GPU_BLOCKS_WINDOW + (super::GPU_BLOCK_SLOTS * super::GPU_BLOCK_MOST) as u64) || is_camera(b, size)
    }

    /// The camera's view window (the kernel's cachemap CAM_VIEW_VA): a stamp names the camera view there for its holder alone, with its frames (2026-11-20, docs/GPU-KISTS.md "The camera ring on the GPU"); the service maps them read-only at `CAMERA_VA`.
    pub const CAMERA_VIEW_WINDOW: u64 = 0x51_2000_0000;
    pub const CAMERA_VIEW_BYTES: u64 = 512 << 20;
    /// Where a client's GPU space maps its camera view, read-only: the view's first frame at `CAMERA_VA`, the rest after it as the CPU sees them from the view's base.
    pub const CAMERA_VA: u64 = 0x1_0000_0000;

    /// Whether a region is the camera's view.
    pub const fn is_camera(b: u64, size: u64) -> bool {
        b >= CAMERA_VIEW_WINDOW && b + size <= CAMERA_VIEW_WINDOW + CAMERA_VIEW_BYTES
    }

    /// Whether a region is a GPU block: in the blocks' window.
    pub const fn is_block(b: u64, size: u64) -> bool {
        b >= super::GPU_BLOCKS_WINDOW && b + size <= super::GPU_BLOCKS_WINDOW + (super::GPU_BLOCK_SLOTS * super::GPU_BLOCK_MOST) as u64
    }

    impl<'a> Stamp<'a> {
        /// The stamp and the caller's own bytes after it; None when the message is shorter than its stamp (only a kernel from before the stamp sends one so), names more than `REGIONS`, or carries more than `FRAMES`: the service then refuses the request. A kernel from before the frames (no bit 2) names its surfaces at their physical addresses, which lie outside the window, so none is a surface here.
        pub fn read(m: &'a [u8]) -> Option<(Stamp<'a>, &'a [u8])> {
            if m.len() < 10 {
                return None;
            }
            let n = m[9] as usize;
            let end = 10 + 16 * n;
            // WHY: a kernel from before the stamp delivers the client's own bytes here, which name any count; and a caller holding more GPU-reachable regions than the service maps is refused whole, never mapped in part.
            if m.len() < end || n > REGIONS {
                return None;
            }
            let mut s = Stamp { epoch: u64::from_le_bytes(m[0..8].try_into().unwrap()), kist: m[8] & 1 != 0, production: m[8] & 2 != 0, n, regions: [(0, 0); REGIONS], frames: &[], view: None };
            for k in 0..s.n {
                let at = 10 + 16 * k;
                s.regions[k] = (u64::from_le_bytes(m[at..at + 8].try_into().unwrap()), u64::from_le_bytes(m[at + 8..at + 16].try_into().unwrap()));
            }
            if m[8] & 4 == 0 {
                return Some((s, &m[end..]));
            }
            let nf: usize = s.regions().iter().filter(|r| in_window(r.0, r.1)).map(|r| (r.1 / FRAME) as usize).sum();
            // WHY: the frames' count follows from the regions the kernel wrote; past what the service maps, or past what arrived (a message cut at the service's buffer), the request is refused whole, as for regions.
            if nf > FRAMES || m.len() < end + 8 * nf {
                return None;
            }
            s.frames = &m[end..end + 8 * nf];
            let end = end + 8 * nf;
            if m[8] & 8 == 0 {
                return Some((s, &m[end..]));
            }
            // The display's view (bit 3): its fixed part, then its surfaces' slots and frames.
            // WHY each check: as above, the counts are the kernel's, and a message cut short or naming past the surface slots is refused whole, never mapped in part.
            const FIXED: usize = 1 + 8 * 3 + 1;
            if m.len() < end + FIXED || m[end] != 0 {
                return None;
            }
            let word = |i: usize| u64::from_le_bytes(m[end + i..end + i + 8].try_into().unwrap());
            let (seq, phases) = (word(1), (word(9), word(17)));
            let n = m[end + 25] as usize;
            let after = end + FIXED + 2 * n + 8 * SURFACE_FRAMES * n;
            if n > super::SURFACE_SLOTS || m.len() < after || phases.1 > PHASES_MOST {
                return None;
            }
            let (mut ro, mut ro_gen) = (0u32, 0u64);
            for j in 0..n {
                let (k, g) = (m[end + FIXED + 2 * j] as usize, m[end + FIXED + 2 * j + 1] as u64);
                // In slot order, each once (the kernel's own order): the frames are read by slot (`View::frames_of`).
                if k >= super::SURFACE_SLOTS || ro >> k != 0 {
                    return None;
                }
                ro |= 1 << k;
                ro_gen |= (g & 15) << (4 * k);
            }
            s.view = Some(View { seq, phases, ro, ro_gen, frames: &m[end + FIXED + 2 * n..after] });
            Some((s, &m[after..]))
        }

        /// The regions it names.
        pub fn regions(&self) -> &[(u64, u64)] {
            &self.regions[..self.n]
        }

        /// Its client's memory as a key: the same key, the same memory.
        pub fn key(&self) -> Key {
            let (ro, ro_gen) = self.view.map_or((0, 0), |v| (v.ro, v.ro_gen));
            Key { epoch: self.epoch, kist: self.kist, n: self.n, regions: self.regions, display: self.view.is_some(), ro, ro_gen }
        }

        /// The frames of region `i` when it is in a window (a surface, a GPU block), in order: its physical memory, 2 MiB a frame; none for any other region.
        pub fn frames_of(&self, i: usize) -> impl Iterator<Item = u64> + 'a {
            let mut at = 0;
            let mut count = 0;
            for (k, r) in self.regions().iter().enumerate() {
                if !in_window(r.0, r.1) {
                    continue;
                }
                let n = (r.1 / FRAME) as usize;
                if k == i {
                    count = n;
                    break;
                }
                at += n;
            }
            // A stamp without frames (bit 2 clear) carries none for it: nothing to map.
            let bytes: &'a [u8] = if 8 * (at + count) <= self.frames.len() { &self.frames[8 * at..8 * (at + count)] } else { &[] };
            bytes.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        }
    }

    /// A kernel's preload word (the run's and the load's): bit 12 r60 = the global id x, bit 13 r61 = the global id y, bit 15 (the service's own) 64 registers a thread.
    pub const PRELOAD_X: u16 = 1 << 12;
    pub const PRELOAD_Y: u16 = 1 << 13;
    pub const PRELOAD_REGS64: u16 = 1 << 15;

    /// The GPU queue (docs/GPU-KISTS.md "The queue", 2026-11-20): jobs submitted from the caller's own thread into a ring in its own GPU block, ended by the GPU service with their GPU time and any fault, waited on by a doorbell; no call a job (ferros_gpuq has the layout).
    ///
    /// ```ignore
    /// let (service, block) = (wants + 1, wants + 2); // a kist's grants (ferros_sys::gpu::kist_slots).
    /// let mut q = Queue::open(cap(g, service), cap(g, block), 0, 2)?;     // the ring at the block's start, 4 deep
    /// // the kernel's words written into the block at `code_at` first (valhall's recorder, compiled where the kist chooses)
    /// let k = q.kernel(code_at, words, PRELOAD_X | PRELOAD_Y)?;
    /// let n = q.submit(&Job { kernel: k, groups: (w / 16, h), uniforms: &[ARENA_VA + out_at, ...] }).unwrap();
    /// let e = q.wait(n);                                                   // e.status, e.gpu_ticks(), e.fault_address
    /// ```
    pub mod queue {
        pub use ferros_gpuq::{Ended, Job, DEPTH_MOST_LOG2, E_CAMERA, E_CLOSED, E_FAULT, E_OVERRUN, E_SERVICE, S_DONE, S_FAILED, S_LOST, S_OUTSIDE, S_REFUSED, UNIFORMS_MOST};
        use core::sync::atomic::AtomicU64;

        /// A queue open on the GPU service.
        /// `open` is a plain call: it waits for a service that is still booting the GPU, and the kernel answers it ERR at once when no GPU service runs or is coming (2026-11-22, after a jump; process.rs `gpu_absent`). Every call after it is live (`call_live`): the service it opened on has received, so a service gone since, whose new one would not know this queue, answers ERR at once instead of a wait.
        pub struct Queue {
            ring: ferros_gpuq::Ring<'static>,
            service: u64,
            at: usize,
            next: u64,
        }

        impl Queue {
            /// The queue at byte `at` (a multiple of 4 KiB) of the GPU block granted at `block`, 2^`depth_log2` jobs deep (at most `DEPTH_MOST_LOG2`), on the GPU service granted at `service`: the block must hold it whole, and should already be the size the caller wants (a block grown after the open is another client to the service, `Stamp::key`, whose kernels and queue are not this one's).
            /// Err: the GPU service's status (1: no room, no slot, a kernel or service from before the queue; 5 a kist's on a production build), or ERR for a grant that is no GPU block.
            pub fn open(service: u64, block: u64, at: usize, depth_log2: u32) -> Result<Queue, u64> {
                let base = super::super::dma(block);
                let size = super::super::dma_size(block, super::super::GPU_BLOCK_MOST as u64).unwrap_or(0) as usize;
                // WHY: the caller's offset and depth: the ring must lie in its own block, or its words would be another region's.
                if base == super::super::ERR || depth_log2 > DEPTH_MOST_LOG2 || at % ferros_gpuq::PAGE != 0 || at + ferros_gpuq::bytes(depth_log2) > size {
                    return Err(super::super::ERR);
                }
                // SAFETY: the block is the caller's for its life (the kernel never takes a held block back), mapped at `base`, `size` bytes; the ring's words lie inside it (checked above), 8-aligned; every word an atomic, as the service and the GPU share them.
                let words = unsafe { core::slice::from_raw_parts((base as usize + at) as *const AtomicU64, ferros_gpuq::bytes(depth_log2) / 8) };
                let ring = ferros_gpuq::Ring::new(words, depth_log2).ok_or(super::super::ERR)?;
                let mut msg = [3u8, 0, 0, 0, 0, depth_log2 as u8];
                msg[1..5].copy_from_slice(&(at as u32).to_le_bytes());
                let mut reply = [1u8; 1];
                let r = super::super::call(service, &msg, &mut reply);
                if r == super::super::ERR || r == 0 || reply[0] != 0 {
                    return Err(if r == super::super::ERR || r == 0 { super::super::ERR } else { reply[0] as u64 });
                }
                Ok(Queue { ring, service, at, next: ring.submitted() })
            }

            /// A kernel for this queue's jobs: its `words` Valhall words, written by the caller at byte `code_at` of its block, copied by the service into its own executable pages; `preload` as `PRELOAD_X` and the others. Its handle, or None (the service's room: 16 kernels, 2^14 words a client).
            pub fn kernel(&self, code_at: usize, words: usize, preload: u16) -> Option<u8> {
                let mut msg = [2u8, 0, 0, 0, 0, 0, 0, 0, 0];
                msg[1..3].copy_from_slice(&preload.to_le_bytes());
                msg[3..7].copy_from_slice(&(code_at as u32).to_le_bytes());
                msg[7..9].copy_from_slice(&(words as u16).to_le_bytes());
                let mut reply = [1u8; 3];
                let r = super::super::call_live(self.service, &msg, &mut reply);
                (r >= 2 && reply[0] == 0).then_some(reply[1])
            }

            /// As `kernel`, for a kernel recorded with a workgroup (valhall::ssa `workgroup`, 2026-11-21): 2^`wg_log2` invocations along x (4 to 8; a job's grid then counts these workgroups), and, when `shared_log2` is not 0, 2^`shared_log2` bytes of workgroup storage each, which the GPU keeps in the caller's own block at byte `shared_at` (a multiple of 4 KiB) in the 2^`room_log2` bytes set aside there.
            /// The room: the storage of as many workgroups as a core's threads hold, for every core id (2^max(`shared_log2`, 7) bytes × THREAD_MAX_THREADS / 2^`wg_log2` × 25 on the Pro's Mali-G715, whose last core id is 24); with less the service gives fewer a core and the GPU holds workgroups back until one's storage is free. Its handle and the workgroups a core may hold, log 2, or None (the service's room, a field outside its range, or a room too small for one workgroup a core).
            pub fn kernel_group(&self, code_at: usize, words: usize, preload: u16, wg_log2: u8, shared_log2: u8, shared_at: usize, room_log2: u8) -> Option<(u8, u8)> {
                let mut msg = [2u8, 0, 0, 0, 0, 0, 0, 0, 0, wg_log2, shared_log2, 0, 0, 0, 0, room_log2];
                msg[1..3].copy_from_slice(&preload.to_le_bytes());
                msg[3..7].copy_from_slice(&(code_at as u32).to_le_bytes());
                msg[7..9].copy_from_slice(&(words as u16).to_le_bytes());
                msg[11..15].copy_from_slice(&(shared_at as u32).to_le_bytes());
                let mut reply = [1u8; 3];
                let r = super::super::call_live(self.service, &msg, &mut reply);
                // An older service answers 2 bytes and ignores the workgroup: refused here, since its jobs would run 16 wide with no storage.
                (r == 3 && reply[0] == 0).then_some((reply[1], reply[2]))
            }

            /// `job` submitted, and the service's doorbell rung (SYS_WAKE on `SUBMIT`): its number, which `wait` and `result` take. None when 2^depth jobs are not yet ended (wait for the oldest first), it passes more than `UNIFORMS_MOST` uniforms, or the queue has ended (`ended`).
            /// The uniforms are read by the GPU from the job's record, so a job's own buffers' addresses go here as `ARENA_VA` plus their offsets in the block.
            pub fn submit(&mut self, job: &Job) -> Option<u64> {
                let n = self.next;
                if !self.ring.submit(n, job) {
                    return None;
                }
                self.next = n + 1;
                super::super::wake(self.ring.submit_word(), 1);
                Some(n)
            }

            /// Asleep until job `n` has ended (or the queue has: every job then ends, lost), and its result. Read it before job n + 2^depth is submitted: it lies in that job's record.
            pub fn wait(&self, n: u64) -> Ended {
                loop {
                    if let Some(e) = self.ring.result(n) {
                        return e;
                    }
                    let d = self.ring.done_raw();
                    // The service gone (the kernel's mark): no result will come.
                    if self.ring.ended() == E_SERVICE {
                        return Ended { status: S_LOST, ..Ended::default() };
                    }
                    if self.ring.result(n).is_none() {
                        super::super::wait(self.ring.done_word(), d, 0);
                    }
                }
            }

            /// Job `n`'s result if it has ended, at once.
            pub fn result(&self, n: u64) -> Option<Ended> {
                self.ring.result(n)
            }

            /// Jobs ended so far (every job numbered under it), and why the queue ended (0 while it runs; `E_FAULT` after a job of it failed: `open` it again to go on).
            pub fn done(&self) -> u64 {
                self.ring.done()
            }
            pub fn ended(&self) -> u64 {
                self.ring.ended()
            }
            /// The number the next `submit` takes.
            pub fn next(&self) -> u64 {
                self.next
            }

            /// The queue opened again after it ended (a fault, a close): the jobs from `next` on are served.
            pub fn reopen(&mut self) -> bool {
                let mut msg = [3u8, 0, 0, 0, 0, self.ring.depth_log2() as u8];
                msg[1..5].copy_from_slice(&(self.at as u32).to_le_bytes());
                let mut reply = [1u8; 1];
                let r = super::super::call_live(self.service, &msg, &mut reply);
                r == 1 && reply[0] == 0
            }

            /// The service told this process's memory now (op 5): after a camera OPEN, its view is mapped read-only in this process's GPU space at `CAMERA_VA` (a buffer at its place in the view, the MAP header's +48, is at `CAMERA_VA` plus that place); after a CLOSE it is gone. Whether the camera is mapped, or None (refused).
            /// Wait for the queue's jobs that read the camera before its CLOSE: one still in flight is stopped when the view goes, and the queue ends (`E_CAMERA`).
            pub fn refresh(&self) -> Option<bool> {
                self.refresh_mapped(true)
            }

            /// `refresh`, the camera mapped cacheable on the GPU when `cached` (the default since 2026-11-20: each job of this process then invalidates the GPU's caches before it runs, so a frame landed since is read from DRAM), non-cacheable when not (a measure; docs/GPU-KISTS.md "The camera ring on the GPU").
            pub fn refresh_mapped(&self, cached: bool) -> Option<bool> {
                let mut reply = [1u8; 2];
                let r = super::super::call_live(self.service, &[5, cached as u8], &mut reply);
                (r == 2 && reply[0] == 0).then_some(reply[1] != 0)
            }

            /// The display's view named (GPU-COMPOSE.md 2.1, 2026-11-21; op 5 with a list): the windows it composes, each surface slot with the generation its window was shown under, at most `SURFACE_SLOTS`. The kernel stamps each one its owner still holds under that generation, read-only, and the service makes the display's GPU space from it: those surfaces at their places from `SURFACES_VA`, the phases at `PHASES_VA`. A slot left out is unmapped, and its frames go back to the pool only once no table of the display's maps them. Called when the windows change, never a frame; for a caller that is not the display the list is ignored. Whether the service took it.
            pub fn view(&self, surfaces: &[(u8, u8)]) -> bool {
                let mut msg = [0u8; 3 + 2 * super::super::SURFACE_SLOTS];
                // WHY the min: the list is the caller's; past the slots there is nothing to name.
                let n = surfaces.len().min(super::super::SURFACE_SLOTS);
                msg[..3].copy_from_slice(&[5, 1, n as u8]);
                for (j, &(k, g)) in surfaces[..n].iter().enumerate() {
                    msg[3 + 2 * j] = k;
                    msg[4 + 2 * j] = g;
                }
                let mut reply = [1u8; 2];
                let r = super::super::call_live(self.service, &msg[..3 + 2 * n], &mut reply);
                r == 2 && reply[0] == 0
            }

            /// This client's GPU work run as a kist's from now (op 7, a measure: priority 8 and the kists' turns): a client may only give up its standing. Whether the service took it (an older one refuses it).
            pub fn as_kist(&self) -> bool {
                let mut reply = [1u8; 1];
                let r = super::super::call_live(self.service, &[7], &mut reply);
                r == 1 && reply[0] == 0
            }

            /// The queue closed: a job still running stopped, the rest lost.
            pub fn close(&mut self) {
                let mut reply = [1u8; 1];
                super::super::call_live(self.service, &[4], &mut reply);
            }
        }
    }
}

/// Preference 46: the sound's volume (docs/AUDIO.md "Volume"), a device setting set thru the Settings capability: 0 never set (`audio::DEFAULT_LEVEL`), 1-255 dimmest to loudest, each step a 16th of an octave of amplitude (255 is full scale, 239 half, 1 almost 16 octaves down).
pub const PREF_VOLUME: u64 = 46;

/// Preference 47: what the helm's two buttons do (owner, 2026-11-18: "a settings addition to choose what back and home do"; docs/WINDOWS.md "The helm"), a choice per bit, 0 each default: bit 0 Back's tap goes out to the deck at a kist's top (else it stays in the kist), bit 1 Back's swipe in a kist goes forward, the Again key to the kist, a redo of its last step back (else to the most recent kist), bit 2 Back's swipe on the deck goes to the most recent kist (else it packs the circles into the screen), bit 3 Deck's swipe in a kist goes to the deck (else it hides or shows the controls), bit 4 Deck's swipe on the deck packs the circles into the screen (else it fits every circle in view).
pub const PREF_HELM: u64 = 47;
/// PREF_HELM's bits.
pub mod helm {
    pub const BACK_OUT: u8 = 1 << 0;
    pub const BACK_SWIPE_FORWARD: u8 = 1 << 1;
    pub const BACK_SWIPE_DECK_RECENT: u8 = 1 << 2;
    pub const DECK_SWIPE_OUT: u8 = 1 << 3;
    pub const DECK_SWIPE_DECK_PACK: u8 = 1 << 4;
}

/// Sound out (docs/AUDIO.md): the system's audio service, which owns the AoC's playback entry point and the speaker amplifiers and mixes every stream into one. A kist reaches it thru the `audio` right (ferros_kist::RIGHTS), a send right on the `audio` endpoint; the service is started by the kernel at the first call (process.rs `ON_DEMAND`) and ends once every stream has closed and played out, so silence holds no arc and the amplifiers sit in reset.
///
/// One format for every stream, the one the AoC's entry point is set up for: 48 kHz, two channels interleaved (left, then right), signed 16-bit little-endian. A stream's ring holds `RING` frames; a write is answered once all its frames are in the ring, so a writer that has more to say waits there, clocked by the hardware (the AoC's playback interrupt frees a period at a time): the reply is the doorbell, no poll and no timer on either side.
///
/// The endpoint's calls, first byte the op, integers little-endian:
/// - `OPEN` + class (`MEDIA` or `ALERT`) → [0][handle, 8 bytes][rate u32][period frames u16][ring frames u32], or [1] refused (no AoC yet, every stream taken, or the amplifiers would not start).
/// - `WRITE` + handle + frames (at most `WRITE_MAX`, 4 bytes each) → [0][room u32, the frames the ring can take now] once every frame is in; [1] for a handle the service does not know (closed, or a service started since).
/// - `CLOSE` + handle → [0]; what the ring holds still plays.
/// - `GAIN` + handle + gain (u16, 32768 unity) → [0]: the stream's own level, under the device's volume.
/// - `STATE` → [AoC online u8][playing u8][streams u8][0][periods played u64][underruns u32][interrupts u32][starved u32, the AoC's ring found empty]: for the console and the verification (docs/AUDIO.md).
/// - `TONE` + level (u8, the volume's scale, at most `TONE_MAX`) + periods (u16) [+ channels u8, `LEFT` | `RIGHT`, both when absent]: a 1 kHz sine of the service's own for that many periods, mixed like any stream, faded in and out over a period: the first sound's safe test, and the Developer page's speaker test (held while the finger is down). Periods 0 stops it (faded over the next period) and starts nothing. → [0][the amplifiers' `REPORT`], or [1] refused (a level over `TONE_MAX`, no channel, the sound would not start).
/// - `SIGGEN` + wave type (u8) + period (u16) + amplitude (u32): the AoC's own signal generator on the speaker's entry point, no PCM at all (AUDIO.md "The AoC side, shortest"), for the bring-up; [0] sent, [1] refused.
/// - `PROBE`: the bring-up's silent check (docs/AUDIO.md "Verification" step 0): the AoC's two services found and described, one control command answered (the entry point's sample count), both amplifiers brought from reset to ready (their trims, the port, the PLL set; never GLOBAL_EN, so nothing can sound), every line on the console. → [0][left DEVID u32][right DEVID u32][the control command answered u8], or [1] with the console saying where it stopped. The amplifiers go back to reset as the process ends.
///
/// - `MIC_LEVELS`: held until the next eighth of the capture's 10 ms periods has come in (80 ms, counted in what the microphones sent, no timer), then → [0][each microphone's peak, u16 × `MICS`, the largest magnitude of its samples since the last answer][bytes the service fell behind and lost, u32 so far][periods captured u32]; the first call starts the capture (the AoC's microphones, no sound out). Call it again at once for a live meter; a capture nobody asks for again within 64 periods (640 ms) is stopped. A second caller replaces the first, which is answered [1]. [2] the capture would not start (the console says why).
/// - `RECORD` + microphone (0 to `MICS` - 1) + level (as `TONE`'s, at most `TONE_MAX`): held while `RECORD_FRAMES` of that microphone are captured, then the recording played back once on both speakers, scaled so its loudest sample comes out at the level (whatever the microphone picked up, never louder than the speaker test can be) → [0][its peak u16][played u8, 0 when the sound would not start][the amplifiers' `REPORT`]. [1] refused (a recording under way, a level or microphone out of range), [2] the capture would not start, [3] the microphone gave silence (peak 0: nothing to play).
///
/// `REPORT`, the amplifiers as the service last read them (it reads both each period while it plays, and stops everything at the first error bit): [left IRQ1_STATUS1 u32][left IRQ1_STATUS3 u32][right IRQ1_STATUS1 u32][right IRQ1_STATUS3 u32][stopped by an error u8]; `PUP_DONE`, `ERRORS` and `PLL_LOCK` pick the bits out. All zero before the amplifiers were first on.
pub mod audio {
    use super::{call, pref, ERR, PREF_VOLUME};

    pub const OPEN: u8 = 0;
    pub const WRITE: u8 = 1;
    pub const CLOSE: u8 = 2;
    pub const GAIN: u8 = 3;
    pub const STATE: u8 = 4;
    pub const TONE: u8 = 5;
    pub const SIGGEN: u8 = 6;
    pub const PROBE: u8 = 7;
    pub const MIC_LEVELS: u8 = 8;
    pub const RECORD: u8 = 9;
    /// The bench (2026-11-24, docs/AUDIO.md "The bench"): the sound in a shape of the caller's, a quiet click train or tone, every interrupt measured and, with the capture on, each click found in the microphones. [BENCH, lead, buffer, period's log2, level, count (u16), spacing's log2, flags (bit 0 clicks, bit 1 capture), capture ms, capture width]; held until it ends, the measures on the console.
    pub const BENCH: u8 = 10;
    /// [MIC_STREAM] on `mic` (2026-11-26): the capture published into the microphone mailbox (ferros_mailbox `mic`, its 1 ms periods at 32 bits) for the holders of its view → [0] streaming, [1] refused (no view held, the microphones would not start, or they run in another shape for the meter). Ask it after opening the view; it stops by itself once no process holds a view (the kernel's mark, fact 90), so giving the view back is the stop, and a reader that ends takes its share with it.
    pub const MIC_STREAM: u8 = 11;

    /// The built-in microphones the service captures (husky's three, ids 0-2), and the recording's length: 2^17 frames, 2.7 s, one microphone, kept in the service's memory only and gone once played.
    pub const MICS: usize = 3;
    pub const RECORD_FRAMES: usize = 1 << 17;

    /// `TONE`'s channels: the left amplifier (cs35l41@0, the frame's first sample), the right (cs35l41@1, the second).
    pub const LEFT: u8 = 1 << 0;
    pub const RIGHT: u8 = 1 << 1;
    /// WHY a ceiling on the test tone (AGENT.md rule 0): the amplifiers run in bypass, without their speaker protection firmware (docs/AUDIO.md "Speaker protection"), so nothing between a sample and the cone limits its excursion or its coil's heat but the level. The first session's limit is four octaves (24 dB) under full scale, 191 (AUDIO.md "Verification" step 2: "Never above 191 in the first session"); a full scale sine held on an unprotected micro speaker is the one way this test could damage the device. The service refuses a level past it (never cuts it down), so a caller's mistake is said, not played.
    pub const TONE_MAX: u8 = 191;
    /// The tone's own bound in periods when a caller holds it (the Developer page: while the finger is down): 1024 periods, 5.5 s, counted by the hardware. WHY a bound: a caller that dies between its start and its stop never sends the stop, and an unprotected speaker must not play on for it.
    pub const TONE_HOLD: u16 = 1 << 10;
    /// `REPORT`'s length, and its bits (cs35l41.h:667-698): IRQ1_STATUS1's power-up done and its errors (the amplifier or the boost shorted, over temperature, the boost's over and under voltage), IRQ1_STATUS3's PLL lock.
    pub const REPORT: usize = 17;
    pub const PUP_DONE: u32 = 1 << 24;
    pub const ERRORS: u32 = 1 << 31 | 1 << 8 | 1 << 17 | 1 << 6 | 1 << 7;
    pub const PLL_LOCK: u32 = 1 << 1;

    /// Music, games, a call's voice: plays in every sound mode, at the device's volume.
    pub const MEDIA: u8 = 0;
    /// A notification or a ring: plays only in the sound mode `SOUND`, and not while do not disturb is on (preferences 39 and 40).
    pub const ALERT: u8 = 1;

    /// Frames a second, channels, and bytes a frame.
    pub const RATE: u32 = 48_000;
    pub const CHANNELS: usize = 2;
    pub const FRAME_BYTES: usize = 4;
    /// The service's period, the unit the hardware frees: 256 frames, 5.3 ms.
    pub const PERIOD: usize = 1 << 8;
    /// A stream's ring: four periods, 21.3 ms.
    pub const RING: usize = PERIOD << 2;
    /// The most frames one write carries: a call's message is at most 4096 bytes (the kernel's copy), less the op and the handle.
    pub const WRITE_MAX: usize = (4096 - 9) / FRAME_BYTES;

    /// The volume's level when the preference was never set: two octaves under full scale.
    pub const DEFAULT_LEVEL: u8 = 255 - (2 << 4);

    /// round(2^15 · 2^(-k/16)), k = 0..15: the fraction of an octave below full scale, for `gain_q15`.
    const OCTAVE_16THS: [u32; 16] = [32768, 31379, 30048, 28774, 27554, 26386, 25268, 24196, 23170, 22188, 21247, 20347, 19484, 18658, 17867, 17109];

    /// A level (1-255; 0 stands for `DEFAULT_LEVEL`) as a gain, 2^15 for full scale: (255 - level) 16ths of an octave down, the whole octaves a shift and the rest from the table.
    pub fn gain_q15(level: u8) -> u32 {
        let l = if level == 0 { DEFAULT_LEVEL } else { level };
        let down = (255 - l) as u32;
        OCTAVE_16THS[(down & 15) as usize] >> (down >> 4)
    }

    /// The device's volume as set (preference 46).
    pub fn level() -> u8 {
        match pref(PREF_VOLUME) {
            0 => DEFAULT_LEVEL,
            l => l,
        }
    }

    /// An open stream: its handle, and the shape the service gave it.
    pub struct Stream {
        pub handle: [u8; 8],
        pub rate: u32,
        pub period: u16,
        pub ring: u32,
    }

    /// Open a stream of `class` on the `audio` endpoint `cap`; None when refused.
    pub fn open(cap: u64, class: u8) -> Option<Stream> {
        let mut r = [0u8; 19];
        let n = call(cap, &[OPEN, class], &mut r);
        if n != 19 || r[0] != 0 {
            return None;
        }
        let mut handle = [0u8; 8];
        handle.copy_from_slice(&r[1..9]);
        Some(Stream { handle, rate: u32::from_le_bytes([r[9], r[10], r[11], r[12]]), period: u16::from_le_bytes([r[13], r[14]]), ring: u32::from_le_bytes([r[15], r[16], r[17], r[18]]) })
    }

    impl Stream {
        /// Frames out (left, right, left, right …; at most `WRITE_MAX` frames, so `2 * WRITE_MAX` samples): returns once all are in the ring, with the room left; None for a handle the service no longer knows (open again).
        pub fn write(&self, cap: u64, samples: &[i16]) -> Option<u32> {
            let mut m = [0u8; 4096];
            m[0] = WRITE;
            m[1..9].copy_from_slice(&self.handle);
            let n = samples.len() & !1;
            for (k, s) in samples[..n].iter().enumerate() {
                m[9 + 2 * k..11 + 2 * k].copy_from_slice(&s.to_le_bytes());
            }
            let mut r = [0u8; 5];
            let got = call(cap, &m[..9 + 2 * n], &mut r);
            (got == 5 && r[0] == 0).then(|| u32::from_le_bytes([r[1], r[2], r[3], r[4]]))
        }

        /// The stream's own gain (32768 unity).
        pub fn gain(&self, cap: u64, gain: u16) -> bool {
            let mut m = [0u8; 11];
            m[0] = GAIN;
            m[1..9].copy_from_slice(&self.handle);
            m[9..11].copy_from_slice(&gain.to_le_bytes());
            let mut r = [0u8; 1];
            call(cap, &m, &mut r) == 1 && r[0] == 0
        }

        /// Closed: what the ring holds still plays.
        pub fn close(self, cap: u64) -> bool {
            let mut m = [0u8; 9];
            m[0] = CLOSE;
            m[1..9].copy_from_slice(&self.handle);
            let mut r = [0u8; 1];
            call(cap, &m, &mut r) != ERR && r[0] == 0
        }
    }
}

/// A preference byte as this process's copy holds it now.
pub fn pref(index: u64) -> u8 {
    // The two blocks: 0-31 at PREFS, 32-47 at PREFS_HI. WHY the min and the mask: the index is the caller's; one past 31 stays inside the first block (as it always has) and one past 47 inside the second, never the tokens or the ihi.
    let at = if index < 32 { PREFS + index } else { PREFS_HI + (index & 15) };
    unsafe { core::ptr::read_volatile((arc_base() + at) as *const u8) }
}

/// The language the shell speaks (kernel lang.rs, SYS_LANG; SETTINGS.md): this device's own and the identity's, each a language by its ISO 639-3 code, and the switch that says which rules. The state is read from the preferences block; the calls change it.
pub mod lang {
    use super::{nr, pref, svc, PREF_LANG, PREF_LANG_DEVICE, PREF_LANG_IDENTITY, PREF_LANG_OWN};

    /// A code as the kernel keeps it: the three letters base 26 (vsf `wl`'s packing); 0 none.
    pub fn pack(code: [u8; 3]) -> u16 {
        code.iter().fold(0u16, |p, &c| p * 26 + (c - b'a') as u16)
    }

    /// The letters of a packed code; None for none.
    pub fn unpack(p: u16) -> Option<[u8; 3]> {
        // WHY the bound: the kernel keeps only codes below 26³ (lang::valid), so this holds for any kernel that has the call; a zeroed block is none.
        (p != 0 && p < 26 * 26 * 26).then(|| [b'a' + (p / 676) as u8, b'a' + (p / 26 % 26) as u8, b'a' + (p % 26) as u8])
    }

    fn read(at: u64) -> u16 {
        u16::from_le_bytes([pref(at), pref(at + 1)])
    }

    /// The language in effect (the identity's while the device follows it and it has one, else this device's); None for none.
    pub fn effective() -> Option<[u8; 3]> {
        unpack(read(PREF_LANG))
    }

    /// This device's own language; None for none.
    pub fn device() -> Option<[u8; 3]> {
        unpack(read(PREF_LANG_DEVICE))
    }

    /// The identity's, as its service last wrote it; None for none.
    pub fn identity() -> Option<[u8; 3]> {
        unpack(read(PREF_LANG_IDENTITY))
    }

    /// Whether this device follows the identity's language (the default).
    pub fn follows() -> bool {
        pref(PREF_LANG_OWN) == 0
    }

    /// This device's own language, given the Settings capability (None: none). True when taken and kept.
    pub fn set_device(cap: u64, code: Option<[u8; 3]>) -> bool {
        svc(nr::LANG, 0, cap, code.map_or(0, pack) as u64, 0, 0) == 0
    }

    /// Whether this device follows the identity's language, given the Settings capability.
    pub fn set_follow(cap: u64, on: bool) -> bool {
        svc(nr::LANG, 1, cap, !on as u64, 0, 0) == 0
    }

    /// The identity's language written to this device (the identity service only, payloads/fgtw): None, it has none.
    pub fn set_identity(code: Option<[u8; 3]>) -> bool {
        svc(nr::LANG, 2, code.map_or(0, pack) as u64, 0, 0, 0) == 0
    }
}

/// The number base and whose choice it is (kernel base.rs, SETTINGS.md "The number base"; 2026-11-22, as `lang`): the identity's while the device follows it and it has one, else this device's own. The base in effect is `device_base` (preference 0); the rest is read with `state`, for the settings page.
pub mod base {
    use super::{nr, svc};

    /// The choices as the kernel holds them.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct State {
        /// The base in effect, tatau's code.
        pub effective: u8,
        /// This device's own.
        pub device: u8,
        /// The identity's, as its service last wrote it; None for none.
        pub identity: Option<u8>,
        /// Whether this device follows the identity's (the default).
        pub follows: bool,
    }

    /// The kernel's choices (SYS_BASE op 3); None from a kernel from before 2026-11-22, which knows only preference 0.
    pub fn state() -> Option<State> {
        let w = svc(nr::BASE, 3, 0, 0, 0, 0);
        if w == super::ERR {
            return None;
        }
        let identity = (w >> 16) as u8;
        Some(State { effective: w as u8, device: (w >> 8) as u8, identity: (identity != 0).then_some(identity), follows: (w >> 24) & 1 == 0 })
    }

    /// This device's own base, given the Settings capability. True when taken and kept.
    pub fn set_device(cap: u64, code: u8) -> bool {
        svc(nr::BASE, 0, cap, code as u64, 0, 0) == 0
    }

    /// Whether this device follows the identity's base, given the Settings capability.
    pub fn set_follow(cap: u64, on: bool) -> bool {
        svc(nr::BASE, 1, cap, !on as u64, 0, 0) == 0
    }

    /// The identity's base written to this device (the identity service only, payloads/fgtw): None, it has none.
    pub fn set_identity(code: Option<u8>) -> bool {
        svc(nr::BASE, 2, code.unwrap_or(0) as u64, 0, 0, 0) == 0
    }
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

/// The circle an app lives in, on a `w` × `h` screen: centre and radius in pixels. Tangent to the bottom of the camera cutout and to the bottom of the screen, centred, so the corners outside it belong to the system (owner, 2026-11-02); every active app's page is inscribed in it (owner, 2026-11-05: the settings page fills it, the attest mark keeps its own smaller circle).
/// Since 2026-11-12 a kist's window is no longer cut to it: the window fills the glass and the corners' controls float in front of it, so the circle is the kist's safe area, told to its window as `window::EV_SAFE` (WINDOWS.md "Drawn under the controls").
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
    /// WiFi: a join the network page handed in (pcie1 JOIN): 0 running, then its outcome if it failed: 1 no WPA2-PSK network of that name in range, 2 refused (a wrong passphrase among the reasons), 3 timed out. A join that succeeds shows as STATE 2 instead. The page reads it from the facts, so it never calls a driver that is busy joining.
    pub const JOIN: u16 = 38;
    /// NFC: how many clients hold it open with the screen off (nfc op 12; owner, 2026-11-24), for the console's mark. Absent when none does.
    pub const HOLDS: u16 = 39;
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
    /// WiFi, kept: power save, 0 off, 1 on (PM1), 2 fast (PM2), 3 with the screen (PM2 while it is off, off while it is on; the default, as unset). Applied at once.
    pub const WIFI_POWER_SAVE: u64 = 1;
    /// WiFi, kept: the band to join on, 0 any (5 GHz whenever the network has it, the default), 1 5 GHz only, 2 2.4 GHz only. Applied at the next join.
    pub const WIFI_BAND: u64 = 2;
    /// The clock: the zone, seconds east of UTC (signed, within a day). Kept as the settings document's `tz`.
    pub const CLOCK_ZONE: u64 = 1;
    /// The battery: the kernel is its driver (the MAX77759 charger on hsi2c_11, docs/CHARGING.md).
    pub const BATTERY: u64 = 7;
    /// The battery, kept: the charge voltage cap in mV, the charger's CV (CHG_CNFG_04, 10 mV steps), from `CHARGE_CV_FLOOR` to the battery's own ceiling; unset, `CHARGE_CV_DEFAULT` (owner, 2026-11-24: "I'd set it to 4.2 v by default and allow the user to set it above").
    /// Applied at once.
    pub const CHARGE_CV: u64 = 1;
    /// The charger's lowest CV in its 10 mV range, mV.
    pub const CHARGE_CV_FLOOR: u64 = 4000;
    /// The battery's own ceiling, mV: the last step of Google's table for the cell (google,chg-cv-limits ends at 4480000 on husky and on akita alike), never the chip's 4.50 V.
    pub const CHARGE_CV_CEILING: u64 = 4480;
    /// The cap with nothing kept, mV.
    pub const CHARGE_CV_DEFAULT: u64 = 4200;
    /// The CV's step, mV.
    pub const CHARGE_CV_STEP: u64 = 10;
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

/// The rights in use (kernel process.rs CLAIMS, SYS_CLAIM; owner, 2026-11-10): a process says it is using a right now, and the kernel keeps that until it releases it or ends. GPS is the first: its driver runs only while something claims it (the map's GPS page while it is open; a kist granted GPS while it wants a position), the switch on meaning allowed, not receiving. A kist calls `claim(GPS, its gps slot)` and then its `gps` endpoint (payloads/gps: FIX, SATS) with `call_live`, which answers ERR while the driver is still starting; it releases when done, or simply ends.
pub mod claim {
    use super::{nr, off, svc, CALL_LIVE, ERR};
    /// GPS's code in ferros_kist::RIGHTS.
    pub const GPS: u64 = 6;
    /// A record of `users`: the kind (0 a system process, 1 a kist), then the kist's capsule address (zeros for a system process).
    pub const REC: usize = 33;
    /// GPS's claims in the facts (fact 85): bits 0-15 how many, bits 16 and up a count of every change.
    pub const FACT: usize = 85;

    /// Claim right `right`, proved by the token at `proof` (for GPS: Settings, the `gps` endpoint's send right, or a GPS mailbox view). False when refused: no such proof, nobody present, GPS switched off. Claiming again is no second claim.
    pub fn claim(right: u64, proof: u64) -> bool {
        svc(nr::CLAIM, 0, right, proof, 0, 0) == 0
    }
    /// Release right `right` (nothing when not held).
    pub fn release(right: u64) {
        svc(nr::CLAIM, 1, right, 0, 0, 0);
    }
    /// Who claims `right` now, given Settings: records of `REC` bytes into `buf` (whole ones, as many as fit), and how many there are; None without the capability.
    pub fn users(settings: u64, right: u64, buf: &mut [u8]) -> Option<usize> {
        let r = svc(nr::CLAIM, 2, right, settings, off(buf.as_mut_ptr()), buf.len() as u64);
        (r != ERR).then_some(r as usize)
    }
    /// The GPS driver's op `op` (0 FIX, 2 SATS) asked thru the kernel for a Settings holder that holds a GPS claim (the map: no `use gps` of its own), its reply into `reply`: the reply's length, or ERR (no claim, no capability, or no driver receiving yet).
    pub fn gps_ask(settings: u64, op: u8, reply: &mut [u8]) -> u64 {
        svc(nr::CLAIM, 3, op as u64, settings, off(reply.as_mut_ptr()), reply.len() as u64 | CALL_LIVE)
    }
    /// The GPS driver's doorbell (payloads/gps op 3 WATCH) for a Settings holder that holds a GPS claim: blocks until the driver's change count (its fixes and fix messages) is other than `seen`, then the count now. A driver not receiving yet (starting, or not started for the claim yet) takes the call when it first receives. None: refused (no claim, no capability), or the driver ended while it held the call. Call it from a thread of its own: it waits as long as nothing changes.
    pub fn gps_watch(settings: u64, seen: u32) -> Option<u32> {
        let mut r = [0u8; 4];
        let n = svc(nr::CLAIM, 3, 3 | (seen as u64) << 8, settings, off(r.as_mut_ptr()), r.len() as u64);
        (n == 4).then(|| u32::from_le_bytes(r))
    }
    /// Bluetooth's code in ferros_kist::RIGHTS: the kernel marks it in use while the holder scans (its OP_SCAN to its OP_SCAN_STOP or OP_CLOSE), the nearby indicator.
    pub const BLUETOOTH: u64 = 14;
    /// The cameras' code: in use from the holder's OPEN on `camera` to its CLOSE, or the camera driver's end.
    pub const CAMERA: u64 = 9;
    /// The microphones' code: in use while the holder holds the microphone mailbox's view.
    pub const MIC: u64 = 15;
    /// A record of `live` (kernel op 5): the right's code, the kind (0 a system process, 1 a kist), the kist's capsule address (zeros for a system process), and the Eagle time (oscillations) it began, i64 LE.
    pub const LIVE_REC: usize = 42;
    /// A record of `log` (kernel op 6): as `LIVE_REC`, then the Eagle time it ended, i64 LE.
    pub const LOG_REC: usize = 50;

    /// Every right in use now and who holds it, given Settings: records of `LIVE_REC` bytes into `buf` (whole ones, as many as fit), and how many there are; None without the capability. The kernel's own marks (the cameras, the microphones, Bluetooth's scans) and the claims (GPS) alike.
    pub fn live(settings: u64, buf: &mut [u8]) -> Option<usize> {
        let r = svc(nr::CLAIM, 5, 0, settings, off(buf.as_mut_ptr()), buf.len() as u64);
        (r != ERR).then_some(r as usize)
    }
    /// The uses that ended, newest first, given Settings: records of `LOG_REC` bytes into `buf` (whole ones, as many as fit), and how many the kernel keeps; None without the capability. A GPS fix read by a kist holding no claim of its own is logged too, its end its start.
    pub fn log(settings: u64, buf: &mut [u8]) -> Option<usize> {
        let r = svc(nr::CLAIM, 6, 0, settings, off(buf.as_mut_ptr()), buf.len() as u64);
        (r != ERR).then_some(r as usize)
    }
    /// The person has seen the uses (the permissions page shown), given Settings: the "used since you last looked" bits of fact 90 clear, never on a timer. False without the capability.
    pub fn seen(settings: u64) -> bool {
        svc(nr::CLAIM, 7, 0, settings, 0, 0) == 0
    }
    /// The claims' doorbell (kernel op 4), given Settings: the kernel writes GPS's claims (`FACT`'s word: the count, and a change count above bit 16) into `word` now and whenever they change, waking whatever waitson it (`wait_world`). False without the capability.
    pub fn doorbell(settings: u64, word: &core::sync::atomic::AtomicU64) -> bool {
        svc(nr::CLAIM, 4, off(word as *const _ as *const u8), settings, 0, 0) == 0
    }
}

/// The rights in use as the privacy indicators show them (fact 90, the kernel's uses table, docs/KERNEL.md "Rights in use"; the owner, 2026-11-26): bits 0-7 an indicator lit now, bits 8-15 one used since the person last looked and not lit now, bits 16 and up a count of every change.
/// Written by the kernel alone, from what it sees: a kist never says what it uses.
pub mod uses {
    /// The fact's number.
    pub const FACT: usize = 90;
    /// The indicators, by bit, the numbers of ferros_kist::indicator (which maps a right to its bit for the kernel): a camera open (the disc behind the hole), the microphones, fine location (GPS), nearby scanning (Bluetooth), motion and screen (each kept for its right, none marked yet).
    pub const CAMERA: u32 = 0;
    pub const MIC: u32 = 1;
    pub const LOCATION: u32 = 2;
    pub const NEARBY: u32 = 3;
    pub const MOTION: u32 = 4;
    pub const SCREEN: u32 = 5;
    /// How many indicators there are.
    pub const COUNT: u32 = 6;
    /// The indicators lit now, a bit each.
    pub fn live(word: u64) -> u8 {
        word as u8
    }
    /// The indicators used since the person last looked and not lit now, a bit each.
    pub fn missed(word: u64) -> u8 {
        (word >> 8) as u8
    }
    /// A count of every change, so a reader knows the set moved without comparing it.
    pub fn changes(word: u64) -> u64 {
        word >> 16
    }
}

/// Shared regions (docs/KERNEL.md "Shared regions", 2026-11-24): a server's pool of memory and the runs of it it gives to the processes it answers, so a stream (a sound ring, a socket's datagrams) passes between them thru memory both map, with no call per item.
/// - The server: `pool` makes or grows its pool (one a process, in place, its address never moving), names the handle slot a masked server reaches it thru and its bell (a word of its own every holder's wake counts up and rings); `give` hands a run of it to the caller of the request it is answering (its reply token), answering the view's id, which the server puts in its reply; `take` takes a run back; `alive` says whether its holder still holds it.
/// - The holder: `view` with the id from the server's reply gives the run's address, its bytes and the handle slot the kernel wrote for it.
/// - The doorbell: `wait` / `wait_world` on a word of the run, and `wake` on it, by either side, as on a word of the arc: waiters of every process are kept under the word's address, the same in both, so one wake reaches whoever waits there. A wait on a run the caller no longer holds (taken back, its server ended) answers ERR: the run is gone.
/// - A run ends at `take`, at its holder's end or at its server's; every thread waiting on one of its words is woken then.
pub mod share {
    use super::{nr, off, svc, ERR};

    /// The pool's handle slot names "the first free pair past the build's" for a holder; a server names its own.
    pub const NO_SLOT: u64 = u64::MAX;
    /// The least a run is: a page.
    pub const RUN_LEAST_LOG: u32 = 12;

    /// The caller's pool, made when it has none, grown in place to 2^`log` bytes (whole 2 MiB frames; asking for less than it holds leaves it), its handle at `slot` of the caller's handle table (a free pair past the ones the build wrote), its bell `bell` (a word of the caller's own, which every holder's wake counts up and rings: the one word a server waits on for all its holders; None leaves it as it was). Its address, the same in every process; None, said on the console: past a GiB, the slot taken, the bell not in the arc, the frame pool short, or a kernel from before.
    pub fn pool(log: u32, slot: u64, bell: Option<&core::sync::atomic::AtomicU64>) -> Option<u64> {
        let b = bell.map_or(0, |w| off(w as *const _ as *const u8));
        let r = svc(nr::SHARE, 0, log as u64, slot, b, 0);
        (r != ERR).then_some(r)
    }

    /// The run of 2^`log` bytes at `at` in the caller's pool (a power of two from a page, `at` aligned to it, inside what the pool holds) given read-write to the process waiting on the reply token `reply` (the caller of the request being answered). The view's id, never 0, for the reply to carry; None, said on the console: no pool, a run outside it or misaligned, a token answering no other process.
    pub fn give(reply: &[u8; 32], at: u64, log: u32) -> Option<u64> {
        let r = svc(nr::SHARE, 1, off(reply.as_ptr()), at, log as u64, 0);
        (r != ERR).then_some(r)
    }

    /// The caller's view `id` taken back: its holder's handle cleared, its waiters woken. False when it is not one of the caller's (ended already).
    pub fn take(id: u64) -> bool {
        svc(nr::SHARE, 2, id, 0, 0, 0) == 0
    }

    /// A view the caller holds: (its address, its bytes, the handle slot the kernel wrote for it, or None when no pair was free). None: not a view the caller holds (taken back, its server ended), or a kernel from before.
    pub fn view(id: u64) -> Option<(u64, u64, Option<u64>)> {
        let mut b = [0u8; 16];
        let a = svc(nr::SHARE, 3, id, off(b.as_mut_ptr()), 0, 0);
        if a == ERR {
            return None;
        }
        let bytes = u64::from_le_bytes(b[..8].try_into().unwrap());
        let slot = u64::from_le_bytes(b[8..].try_into().unwrap());
        Some((a, bytes, (slot != NO_SLOT).then_some(slot)))
    }

    /// Whether the caller's view `id` is still held: Some(false) once its holder ended or it was taken back; None without a pool.
    pub fn alive(id: u64) -> Option<bool> {
        match svc(nr::SHARE, 4, id, 0, 0, 0) {
            ERR => None,
            r => Some(r == 1),
        }
    }
}

/// Mailboxes (ferros_mailbox, docs/KERNEL.md "Mailboxes"; owner, 2026-11-19: "a capability based public read only mailbox so you could have an unlimited number of items accessing it"): a driver publishes into a frame of words, and any number of readers map it read-only and read it without calling the driver.
/// - The writer holds the `<name>-mailbox` region (granted at its spawn) and takes it with `Writer::take`; it publishes with `publish` (or `begin` and `finish` for a payload built in place), which wakes every reader waiting, in one call, however many there are.
/// - A reader holds the `<name>-view` region, granted at its spawn (`Reader::map`) or opened with a proof (`Reader::open`: the GPS right, Settings); it waits for the next publish (`wait`) and reads a whole one (`read`), never calling the writer. Masked code reads thru the view's handle instead (`View`).
/// - The kernel marks the mailbox dead when the writer's process ends and zeroes its payload (`Got::Dead`); a new writer's first publish brings it back.
pub mod mailbox {
    pub use ferros_mailbox::{frame, gps, mic, motion, thermo, Got, Publish, AT, BOX_NAMES, DEAD, FRAME, GPS, HEADER, KIND, LEN, MIC, MOTION, NAMES, PAYLOAD_MAX, SEQ, TEST, THERMO, VIEW_NAMES, WORDS};
    use super::{dma, now, nr, off, svc, wait, wait_world, wake, ERR};
    use core::sync::atomic::AtomicU64;

    /// SYS_MAILBOX's op: open a view with a proof.
    const OPEN: u64 = 0;
    const CLOSE: u64 = 1;

    /// Give back mailbox `id`'s view opened with `open` (2026-11-26): the capability and its handle gone, so a masked reader reads it no more, and for the microphones the kernel's in-use mark with it (the privacy indicator goes dark when the view does, never on the reader's word). Nothing when this process holds no view of it.
    pub fn close(id: usize) {
        svc(nr::MAILBOX, CLOSE, id as u64, 0, 0, 0);
    }

    /// The mailbox's words at `base` (its region's address, from SYS_DMA or the open).
    fn words(base: u64) -> &'static [AtomicU64] {
        // SAFETY: `base` is the kernel's answer for a region this process holds: a frame of the pool mapped for the boot at that address (the writer's window read-write, the readers' read-only), 8-aligned, `WORDS` words long; atomics read it whatever another process writes there.
        unsafe { core::slice::from_raw_parts(base as *const AtomicU64, WORDS) }
    }

    /// The writer's side of a mailbox.
    pub struct Writer {
        words: &'static [AtomicU64],
    }

    impl Writer {
        /// The mailbox whose writer's region is granted at `token` (`<name>-mailbox`), mapped, this process its one writer. None: no such grant, a kernel without mailboxes, or another live process writes it (a writer still ending: take it again at the next publish).
        pub fn take(token: u64) -> Option<Writer> {
            let base = dma(token);
            (base != ERR && base != 0).then(|| Writer { words: words(base) })
        }
        /// Publish `payload` whole, tagged `kind`, and wake every reader waiting: the new sequence and how many threads woke.
        pub fn publish(&self, kind: u64, payload: &[u64]) -> (u64, u64) {
            let s = ferros_mailbox::publish(self.words, kind, now(), payload);
            (s, self.ring())
        }
        /// Begin a publish built in place (`Publish::put` each word), then `finish`.
        pub fn begin(&self) -> Publish<'static> {
            Publish::begin(self.words)
        }
        /// The publish begun whole (`kind`, `len` words), and every reader waiting woken: the new sequence and how many threads woke.
        pub fn finish(&self, p: Publish<'static>, kind: u64, len: usize) -> (u64, u64) {
            let s = p.finish(kind, len, now());
            (s, self.ring())
        }
        /// The microphones' ring publish ended with its header (ferros_mailbox `mic::finish`: the frames before `h.pos` whole), and every reader waiting woken: the new sequence and how many threads woke.
        pub fn finish_mic(&self, p: Publish<'static>, h: &mic::Head) -> (u64, u64) {
            let s = mic::finish(p, h, now());
            (s, self.ring())
        }
        /// The sequence word's waiters woken, in every process: the kernel's one scan of its threads (SYS_WAKE on a mailbox's sequence word by its writer), never a call per reader.
        fn ring(&self) -> u64 {
            wake(&self.words[SEQ], u64::MAX)
        }
    }

    /// A reader's side of a mailbox.
    pub struct Reader {
        words: &'static [AtomicU64],
    }

    impl Reader {
        /// The mailbox whose view is granted at `token` (`<name>-view`, a spawn's grant, or the token `open` wrote).
        pub fn map(token: u64) -> Option<Reader> {
            let base = dma(token);
            (base != ERR && base != 0).then(|| Reader { words: words(base) })
        }
        /// Mailbox `id` (`GPS`, `MOTION`, ...) opened with the token at `proof` (0: none), the view's own token written into `token`, which the caller keeps (a proof for a claim, `map` again).
        /// The proof the kernel takes: for GPS the Settings capability, the `gps` right (its endpoint's send right) or a GPS view; the motion mailbox none yet (gravity and the field are in the facts, which anyone reads); the thermometer none (owner, 2026-11-20: open, no permission necessary). None: refused, or a kernel without mailboxes.
        pub fn open(id: usize, proof: u64, token: &mut [u8; 32]) -> Option<Reader> {
            let base = svc(nr::MAILBOX, OPEN, id as u64, proof, off(token.as_mut_ptr()), 0);
            (base != ERR && base != 0).then(|| Reader { words: words(base) })
        }
        /// The sequence now (even: whole; odd: a publish under way; `DEAD`: no writer).
        pub fn seq(&self) -> u64 {
            self.words[SEQ].load(core::sync::atomic::Ordering::Acquire)
        }
        /// Asleep until the sequence is other than `seen` (a publish, or the writer's end), or the counter reaches `deadline` (0: none): the sequence then. A wait on the world (`wait_world`): what the writer publishes comes when it comes, so the process watchdog's clock is paused.
        /// Every wait in the system is a loop that looks again (the kernel wakes all word waiters when a preference changes), so a wake that finds the sequence unmoved waits again.
        pub fn wait(&self, seen: u64, deadline: u64) -> u64 {
            loop {
                let r = wait_world(&self.words[SEQ], seen, deadline);
                let s = self.seq();
                // 2: the deadline; ERR: a kernel that refuses the word (none from before 2026-11-19 takes it).
                if s != seen || r == 2 || r == ERR {
                    return s;
                }
            }
        }
        /// One look (ferros_mailbox `try_read`): `Busy` when a publish was under way or landed in it. For a reader that counts its retries (payloads/mbtest); `read` is the rest's.
        pub fn try_read(&self, out: &mut [u64]) -> Got {
            ferros_mailbox::try_read(self.words, out)
        }
        /// One whole publish into `out` (as many words as fit): a read that a publish landed in is read again at once, one under way is waited out on the sequence word (the writer's wake ends it). Never `Busy`.
        pub fn read(&self, out: &mut [u64]) -> Got {
            loop {
                match ferros_mailbox::try_read(self.words, out) {
                    Got::Busy { seq } if seq & 1 != 0 => {
                        wait(&self.words[SEQ], seq, 0);
                    }
                    Got::Busy { .. } => {}
                    g => return g,
                }
            }
        }
    }

    /// A reader's side of a mailbox reached thru its handle at grants slot `SLOT` (SANDBOX.md "Handles"): masked code's reader, which `Reader` cannot be (a masked load of the view's address lands in the arc).
    /// The kernel writes an opened view's handle at the first free pair past the slots its build wrote (2026-11-20, process.rs `mbox_open`), so a process that opens one view finds it at its own grant count, which it knows from its layout; `open` checks the pair is that view's before it answers.
    /// The same seqlock as `Reader` (ferros_mailbox `try_read_by`), each word an aligned 64-bit load thru the handle (single-copy atomic), the fences between.
    #[derive(Clone, Copy)]
    pub struct View<const SLOT: u64> {
        base: u64,
    }

    impl<const SLOT: u64> View<SLOT> {
        /// Mailbox `id` opened with the token at `proof` (0: none), as `Reader::open`; None when refused, a kernel without mailboxes, or a kernel that put no handle at `SLOT` for it (one from before 2026-11-20, or the pair taken).
        pub fn open(id: usize, proof: u64, token: &mut [u8; 32]) -> Option<View<SLOT>> {
            let base = svc(nr::MAILBOX, OPEN, id as u64, proof, off(token.as_mut_ptr()), 0);
            (base != ERR && base != 0 && super::Handle::<SLOT>::pair().0 == base).then_some(View { base })
        }
        fn load(i: usize) -> u64 {
            super::Handle::<SLOT>::read64(8 * i as u64)
        }
        /// The sequence now (even: whole; odd: a publish under way; `DEAD`: no writer).
        pub fn seq(&self) -> u64 {
            let s = Self::load(SEQ);
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
            s
        }
        /// SYS_WAIT on the sequence word by its address (the kernel resolves it to the view this process holds, `mbox_wait_word`): no reference to the word is made, since masked code cannot load thru one.
        fn wait_word(&self, seen: u64, deadline: u64, world: bool) -> u64 {
            svc(nr::WAIT, off((self.base + 8 * SEQ as u64) as *const u8), seen, deadline, world as u64, 0)
        }
        /// As `Reader::wait`: asleep until the sequence is other than `seen` or the counter reaches `deadline` (0: none), a wait on the world; the sequence then.
        pub fn wait(&self, seen: u64, deadline: u64) -> u64 {
            loop {
                let r = self.wait_word(seen, deadline, true);
                let s = self.seq();
                // 2: the deadline; ERR: a kernel that refuses the word.
                if s != seen || r == 2 || r == ERR {
                    return s;
                }
            }
        }
        /// One look (ferros_mailbox `try_read_by`).
        pub fn try_read(&self, out: &mut [u64]) -> Got {
            ferros_mailbox::try_read_by(Self::load, out)
        }
        /// The microphones' ring from position `from` up to the newest into `out` (ferros_mailbox `mic::read_by`), once: on `Busy`, wait on the sequence it names (`wait`) and look again.
        pub fn read_mic(&self, from: u64, out: &mut [i32]) -> mic::Read {
            mic::read_by(Self::load, from, out)
        }
        /// One whole publish into `out`, as `Reader::read`: never `Busy`.
        pub fn read(&self, out: &mut [u64]) -> Got {
            loop {
                match self.try_read(out) {
                    Got::Busy { seq } if seq & 1 != 0 => {
                        self.wait_word(seq, 0, false);
                    }
                    Got::Busy { .. } => {}
                    g => return g,
                }
            }
        }
    }
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

/// The facts' bell source (`bell`): the facts a settings page shows (the battery's share and whether it charges, the processes alive, the vault's keys, the hottest part of the chip and its throttle). Sources 0-5 are the links in the facts' order.
pub const BELL_FACTS: u64 = 6;
/// The windows' owners' bell source (`bell`) in its old form, for the holder of the `surfaces` region alone (the display server): byte k slot k's live generation for slots 0 to 3, and a change count above bit 32. Kept for a display from before 2026-11-12; `BELL_SLOTS` carries every slot.
pub const BELL_SURFACES: u64 = 9;
/// The surface slots' bell source (`bell`, since 2026-11-12), for the holder of the `surfaces` region alone (the display server): the word is not a count but every slot's live generation, bits 4k to 4k + 3 slot k's (1 to 15 while the process it was given to is alive, 0 once it has ended, however it ended, or while the slot is free; `surface_generation`). A window shown on a surface under a generation its slot no longer holds belongs to a process that has ended.
pub const BELL_SLOTS: u64 = 12;
/// The identity's lock bell source (`bell`, since 2026-11-17): the word is the kernel's lock epoch, which moves at every lock of the identity (the lock op, an unlock that locks what was open first, a release).
/// The identity service wipes what it holds of the identity when it moves.
/// Any process may register a word for it: whether a person is present is fact 83 already.
pub const BELL_IDENTITY: u64 = 13;
/// The pucks' squares (2026-11-18, docs/PUCKS.md), for the holder of the `surfaces` region (the display): bits 4j to 4j + 3 square slot j's generation (1 to 15) while its puck lives, 0 otherwise ([`surface_generation`] reads it the same way).
pub const BELL_PUCKS: u64 = 14;
/// The camera on the GPU (2026-11-20, docs/GPU-KISTS.md "The camera ring on the GPU"), for the GPU service alone (the holder of `arena`): counted each time a camera view its holder's GPU calls carried is taken back, so the service unmaps it from that client's space and says so (SYS_DMA x1 = 9 on `arena`).
pub const BELL_GPU_CAMERA: u64 = 15;
// Bell 16 is the identity's state, registered by SYS_UNLOCK op 11 (`identity_doorbell`), never by SYS_BELL.

/// The app in front (2026-11-18, docs/WINDOWS.md "Stop"), for the holder of the `surfaces` region (the display): not a bell the kernel rings but the display's own word, which the kernel reads at power + volume down to end the app in front without asking the display anything (`bell_front`, `front_word`).
pub const BELL_FRONT: u64 = 17;

/// The display's word for the app window in front (`BELL_FRONT`): its surface slot in bits 0-7, its generation in 8-15, bit 16 set; 0 with none.
pub const fn front_word(slot: usize, generation: u8) -> u64 {
    1 << 16 | (generation as u64) << 8 | slot as u64
}

/// `word` registered as the display's word for the app in front (`BELL_FRONT`): the kernel reads it from now, never writes it. False for a caller without the `surfaces` region, or a kernel from before.
pub fn bell_front(word: &core::sync::atomic::AtomicU64) -> bool {
    svc(nr::BELL, BELL_FRONT, off(word as *const _ as *const u8), 0, 0, 0) == 0
}

/// How many surfaces the kernel can give out at once (2026-11-12, on demand from the frame pool; four fixed ones before): slots of the surfaces' window, slot k at `SURFACES_WINDOW + k * SURFACE_BYTES`.
pub const SURFACE_SLOTS: usize = 16;
/// One surface slot's bytes: a whole panel at 64 bits a pixel and its porch, rounded up to a power of two (the kernel's SURFACE_SIZE).
pub const SURFACE_BYTES: usize = 32 << 20;
/// The surfaces' window, the kernel's cachemap SURF_VA: what `dma` answers for the whole `surfaces` region.
pub const SURFACES_WINDOW: u64 = 0x50_0A00_0000;

/// How many pucks may hold a square at once (2026-11-18, docs/PUCKS.md; the kernel's cachemap PUCK_SLOTS): slot j at `PUCKS_WINDOW + j * PUCK_BYTES`.
pub const PUCK_SLOTS: usize = 16;
/// One square slot's addresses (the kernel's PUCK_SIZE): its frames are only what its puck asked for (`puck_size`).
pub const PUCK_BYTES: usize = 64 << 20;
/// The pucks' window, the kernel's cachemap PUCK_VA: where the display reads the squares (it holds the surfaces' window and runs native).
pub const PUCKS_WINDOW: u64 = 0x50_4000_0000;

/// The most GPU memory one client may hold (the kernel's cachemap GPU_BLOCK_MOST, 2026-11-13): its GPU block, frames of the pool on demand, asked for by `gpu_block_ask`.
pub const GPU_BLOCK_MOST: usize = 512 << 20;
/// How many GPU blocks the kernel can give out at once: slots of the blocks' window, slot k at `GPU_BLOCKS_WINDOW + k * GPU_BLOCK_MOST`.
pub const GPU_BLOCK_SLOTS: usize = 16;
/// The GPU blocks' window, the kernel's cachemap GPUB_VA: what `dma` answers for the whole `arena` region (the GPU service's), a block at the start of its slot.
pub const GPU_BLOCKS_WINDOW: u64 = 0x58_0000_0000;

/// Slot k's generation in the slots' bell word (`BELL_SLOTS`): 0 when nobody alive holds it.
pub const fn surface_generation(word: u64, k: usize) -> u8 {
    (word >> (4 * k)) as u8 & 15
}

/// A doorbell on what the settings pages show (kernel SYS_BELL; owner, 2026-11-10: "Needs a doorbell"): the kernel writes `source`'s change count into `word` now and whenever it changes, waking whatever waits on it (`wait_world`). Source 0-5 a link (its measures, tracks and detail, as its driver sets them, and emptied as it ends), `BELL_FACTS` the facts. A reader that compares the word to the count it last drew with knows to draw again, and asks nothing on a timer. False for no such source, or a kernel without bells.
pub fn bell(source: u64, word: &core::sync::atomic::AtomicU64) -> bool {
    svc(nr::BELL, source, off(word as *const _ as *const u8), 0, 0, 0) == 0
}

/// The kist launches that did not start (kernel SYS_NOT_STARTED; owner, 2026-11-11: Mahere's circle tapped, nothing opened, "it looked stuck"). Init answers a launch "started" as soon as it is queued (ferros_kist::launch::STARTED); the kernel then reads the capsule from the vault, checks it and sizes its arc, and a launch that fails there is kept here, one record a capsule, until that kist next starts; a kist that started and then ended by a fault, a panic or a kill from outside is kept the same way; and the kists alive now (`live`). For the Settings holder alone (the map), as the claims' users list.
pub mod not_started {
    use super::{nr, off, svc, ERR};
    /// A record of `list`: the reason (ferros_kist::launch::why), the capsule's address (32 bytes), and the kists' bell count it rang (u64 LE), so a reader tells a failure that came after its own ask from one kept before.
    pub const REC: usize = 41;

    /// The records into `buf` (whole ones, as many as fit), given Settings: how many there are, or None without the capability (or a kernel without them).
    pub fn list(settings: u64, buf: &mut [u8]) -> Option<usize> {
        let r = svc(nr::NOT_STARTED, 0, settings, off(buf.as_mut_ptr()), buf.len() as u64, 0);
        (r != ERR).then_some(r as usize)
    }
    /// The kists' doorbell, given Settings: the kernel writes its change count into `word` now and whenever a launch does not start, a kist starts, or a kist ends any way, waking whatever waits on it (`wait_world`). False without the capability, or a kernel without it.
    pub fn doorbell(settings: u64, word: &core::sync::atomic::AtomicU64) -> bool {
        svc(nr::NOT_STARTED, 1, off(word as *const _ as *const u8), settings, 0, 0) == 0
    }
    /// The kists alive now into `buf`, given Settings (op 2; owner, 2026-11-11: their circles glow running, stopped or not running): each a capsule's address, 32 bytes, as many whole ones as fit; how many there are, or None without the capability (or a kernel from before, which refuses the op).
    pub fn live(settings: u64, buf: &mut [u8]) -> Option<usize> {
        let r = svc(nr::NOT_STARTED, 2, settings, off(buf.as_mut_ptr()), buf.len() as u64, 0);
        (r != ERR).then_some(r as usize)
    }
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
pub struct Facts(pub [u64; 91]);

/// The bootloader's lock state (`Facts::boot_lock`; FLASHER.md step 7: re-locking ends setup, leaving it unlocked is the person's known choice).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootLock {
    /// Anyone holding the device can boot their own system and derive its ira, so read the device class (settings, stowed kists, cached wallpaper).
    Unlocked,
    /// Locked to the custom key (verified boot yellow): the key this ferros is signed with.
    LockedOwnKey,
    /// Locked to the maker's key (verified boot green).
    LockedMakersKey,
}

/// Local wall time: (seconds since 1970 in local time — UTC plus the zone, `device.tz` — and the counter ticks into the current second), or None until the kernel has read the PMIC's clock. From the kernel's base (facts 8, 9) and this process's own counter.
/// The zone's two offsets as the kernel last handed them (preferences bytes 24-31, kernel rtc.rs `packed`): (standard, legal), seconds east of UTC. Legal is daylight time while it is in effect; without a zone's rule (a manual offset) the two are the same.
pub fn zone_offsets() -> (i64, i64) {
    let v = unsafe { core::ptr::read_volatile((arc_base() + PREFS + 24) as *const u64) };
    (v as u32 as i32 as i64, (v >> 32) as u32 as i32 as i64)
}

/// The offset the device's base shows (owner, 2026-11-10): arabic the legal one, matching the wall clocks, daylight time and all; dozenal and hex the zone's standard offset all year, never daylight time, so their date can differ from arabic's for an hour of the day in daylight season.
pub fn zone() -> i64 {
    let (standard, legal) = zone_offsets();
    if device_base() == 10 { legal } else { standard }
}

pub fn local_time() -> Option<(i64, u64)> {
    // From the preferences block (bytes 16-31, the kernel's PREF_WALL): no syscall — the first version asked SYS_FACTS on every frame, and facts samples the battery gauge over I2C under the kernel lock (it hung the kernel, 2026-10-23).
    let base = unsafe { core::ptr::read_volatile((arc_base() + PREFS + 16) as *const u64) };
    if base == 0 {
        return None;
    }
    let tz = zone();
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
    /// The bootloader's lock state as ABL told this boot (fact 87, kernel bootlock.rs), None when not known (a reload, or ABL said neither). Unlocked when either of ABL's two words says so (the device state `unlocked`, or verified boot `orange`), so a disagreement shows the exposure rather than hiding it.
    pub fn boot_lock(&self) -> Option<BootLock> {
        let v = self.get(87)?;
        let (device, vbs) = (v & 0xFF, v >> 8 & 0xFF);
        match (device, vbs) {
            (2, _) | (_, 3) => Some(BootLock::Unlocked),
            (1, 2) => Some(BootLock::LockedOwnKey),
            (1, 1) => Some(BootLock::LockedMakersKey),
            _ => None,
        }
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
    /// Whether the kernel's thermal policy holds a clock (the X3's, the A715s', the A510s' or the GPU's) under the rate the clock policy wants.
    pub fn throttled(&self) -> Option<bool> {
        self.get(11).map(|v| v != 0)
    }
    /// Which thermal zone is the hottest (fact 86, the one `soc_temp_c` reads): 0 the X3, 1 the A715s, 2 the A510s, 3 the GPU, 4 the camera ISP, 5 the TPU, 6 the AUR (the AoC's audio core).
    pub fn soc_hottest_zone(&self) -> Option<u64> {
        self.get(86)
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
    /// The magnetic field as the AoC last reported it (facts 88 and 89, 2026-11-17: the fused magnetometers, µT, the device's axes, uncalibrated for hard iron: ferros_compass calibrates), and how many samples have come (a new one when it moves).
    pub fn magnetic(&self) -> Option<([f32; 3], u32)> {
        let (xy, z) = (self.get(88)?, self.get(89)?);
        let n = (z >> 32) as u32;
        (n != 0).then(|| ([f32::from_bits(xy as u32), f32::from_bits((xy >> 32) as u32), f32::from_bits(z as u32)], n))
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
/// The largest a get (`store_get_at`, `store_read_at`) reads in one call on a kernel from 2026-11-11 on (the kernel's process.rs STORE_GET_MAX): 1 MiB, sixteen of the vault's pieces opened as one batch, a quarter of the round trips. A kernel from before refuses a get over STORE_MAX (ERR): a reader asks again at STORE_MAX, as the display does.
pub const STORE_GET_MAX: usize = 1 << 20;

/// Read `key` from the vault scope a Store capability names (the key is relative to the scope): Some(the value's whole length, of which at most `buf.len()` bytes were copied), or None if it is absent or the call was refused.
pub fn store_get(cap: u64, key: &[u8], buf: &mut [u8]) -> Option<usize> {
    let r = svc(nr::STORE, cap, (buf.len() as u64) << 8, off(key.as_ptr()), key.len() as u64, off(buf.as_mut_ptr()));
    if r == ERR || r == NOTHING { None } else { Some(r as usize) }
}

/// Blocks of a window on the ferros partition (a Blocks capability; the vault process's disk, VAULT-OUT.md): `buf.len() / 4096` blocks (1 to 256: one UFS command, 1 MiB; 32 on a kernel from 2026-11-08 and 16 on one from before, which refuse more) from block `at` of the window, read into `buf`. Returns 0, or ERR (not a window this process holds, out of it, or the UFS command failed).
pub fn block_read(cap: u64, at: u64, buf: &mut [u8]) -> u64 {
    svc(nr::BLOCK, cap, ((buf.len() as u64 >> 12) & 0xFFFF) << 8, at, off(buf.as_mut_ptr()), 0)
}

/// `block_read`, timed by the kernel (SYS_BLOCK bit 1, 2026-11-11): (0 or ERR, and on success the timer ticks of the UFS command and of the kernel's copy into `buf`, or None for those from a kernel that does not time).
pub fn block_read_timed(cap: u64, at: u64, buf: &mut [u8]) -> (u64, Option<(u64, u64)>) {
    let op = 2 | ((buf.len() as u64 >> 12) & 0xFFFF) << 8;
    let (r, cmd, copy): (u64, u64, u64);
    compiler_fence(Ordering::SeqCst);
    unsafe { asm!("svc #0", inlateout("x0") cap => r, inlateout("x1") op => cmd, inlateout("x2") at => copy, in("x3") off(buf.as_mut_ptr()), in("x4") 0u64, in("x8") nr::BLOCK, options(nostack)) };
    compiler_fence(Ordering::SeqCst);
    // A kernel that does not time leaves x1 and x2 as they went in.
    (r, (r == 0 && (cmd, copy) != (op, at)).then_some((cmd, copy)))
}

/// The device's write cache to the medium (SYS_BLOCK op bit 6, 2026-11-22: SYNCHRONIZE CACHE of the unit the window lies on): Some((0 or ERR, the ticks it took)), or None from a kernel that does not know the op (it leaves x1 as it went in).
pub fn block_flush(cap: u64) -> Option<(u64, u64)> {
    let op = 64u64;
    let (r, ticks): (u64, u64);
    compiler_fence(Ordering::SeqCst);
    unsafe { asm!("svc #0", inlateout("x0") cap => r, inlateout("x1") op => ticks, in("x2") 0u64, in("x3") 0u64, in("x4") 0u64, in("x8") nr::BLOCK, options(nostack)) };
    compiler_fence(Ordering::SeqCst);
    (ticks != op).then_some((r, ticks))
}

/// `block_read`'s other half: `buf.len() / 4096` blocks written from `buf` at block `at` of the window.
pub fn block_write(cap: u64, at: u64, buf: &[u8]) -> u64 {
    svc(nr::BLOCK, cap, 1 | ((buf.len() as u64 >> 12) & 0xFFFF) << 8, at, off(buf.as_ptr()), 0)
}

/// The vault process's own call (a Vault capability; VAULT-OUT.md, migration): the `skip`-th entry of the kernel's vault still to move into `buf`, as [class: 1 the person's][the domain's address and sealing keys][the scope prefix's length][the full name]. Its length, NOTHING when none is left, ERR.
pub fn vault_next(cap: u64, skip: u64, buf: &mut [u8]) -> u64 {
    svc(nr::VAULT, cap, (buf.len() as u64) << 8, skip, 0, off(buf.as_mut_ptr()))
}

/// An old entry's value from `offset` into `buf`: its whole length, NOTHING if it is gone, ERR.
pub fn vault_read(cap: u64, name: &[u8], offset: u64, buf: &mut [u8]) -> u64 {
    svc(nr::VAULT, cap, 1 | (buf.len() as u64) << 8 | offset << 32, off(name.as_ptr()), name.len() as u64, off(buf.as_mut_ptr()))
}

/// The vault process answers gets of up to `max` bytes (its reply buffer less the value word; the kernel holds it to its own STORE_GET_MAX): until a vault process says so the kernel forwards no get larger than STORE_MAX (2026-11-11). 0, or ERR (no Vault capability, or a kernel from before).
pub fn vault_gets(cap: u64, max: usize) -> u64 {
    svc(nr::VAULT, cap, 3 | ((max as u64) & 0xFF_FFFF) << 8, 0, 0, 0)
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

/// Write a value under an address in pieces, as `store_set_at` does by name: `piece` at `offset`, `more` while pieces follow; the piece without `more` commits the whole (the vault process writes each piece as it comes, so a value of any size passes through a buffer of one transfer). A piece is at most `STORE_MAX`. True while the vault takes them.
pub fn store_set_addr_at(cap: u64, addr: &[u8; 32], offset: usize, piece: &[u8], more: bool) -> bool {
    svc(nr::STORE, cap, 32 | 1 | 2 | if more { 4 } else { 0 } | (piece.len() as u64) << 8 | (offset as u64) << 32, off(addr.as_ptr()), 32, off(piece.as_ptr())) == 0
}

/// Delete the entry under an address: Some(whether it was there), None if refused.
pub fn store_delete_addr(cap: u64, addr: &[u8; 32]) -> Option<bool> {
    match svc(nr::STORE, cap, 32 | 8, off(addr.as_ptr()), 32, 0) {
        0 => Some(true),
        NOTHING => Some(false),
        _ => None,
    }
}

/// SYS_STORE x1 bit 7: a call on the hold (docs/STORE.md, docs/HOLD-API.md), on an `items` grant (process.rs `Object::Items`).
pub const STORE_ITEMS: u64 = 1 << 7;

/// One call on the hold: `call` as ferros_items `wire::call` makes it (at most a transfer and its head), the answer's bytes into `reply`; the value (ERR refused, NOTHING gone). ferros_items `kist::Hold` makes the calls; a payload hands it `kist::Sys { cap }` with this grant's token, or calls this.
pub fn items_call(cap: u64, call: &[u8], reply: &mut [u8]) -> u64 {
    svc(nr::STORE, cap, STORE_ITEMS | (reply.len() as u64) << 8, off(call.as_ptr()), call.len() as u64, off(reply.as_mut_ptr()))
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
    /// The magnetic field (the compass puck): x | y << 32 (f32 bits, µT), then z | sample count << 32.
    pub const MAG_XY: usize = 22;
    pub const MAG_Z: usize = 23;
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
    power_arg(cap, on, 0)
}

/// `power` with a byte for the switch in bits 8-15 of its word: the `camera` switch's module (0 the main, 1 the ultrawide, 2 the front, 3 the tele; kernel camera_power.rs `select`). Every other switch takes 0.
pub fn power_arg(cap: u64, on: bool, arg: u8) -> bool {
    pin(CORES_BIG);
    sleep_until(now() + freq() / 1000);
    let r = svc(nr::POWER, cap, on as u64 | (arg as u64) << 8, 0, 0, 0);
    pin(0);
    r == 0
}

/// A per-purpose 32-byte key rooted in the device's ira, through an Ira capability: `blake3::derive_key("ferros process ira v1 " ‖ domain, ira)` — the same bytes on every boot and after a factory reset, never the ira itself. None when the kernel has no ira this boot.
pub fn ira_key(cap: u64, domain: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    let r = svc(nr::IRA, cap, off(domain.as_ptr()), domain.len() as u64, off(out.as_mut_ptr()), 0);
    (r == 32).then_some(out)
}

/// `ira_key` bound to the person present as well as this device (the kernel mixes the unlocked identity's domain key in): None while nobody's identity is unlocked. A kernel from before 2026-11-09 ignores the flag and gives the device's key alone.
pub fn ira_person_key(cap: u64, domain: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    let r = svc(nr::IRA, cap, off(domain.as_ptr()), domain.len() as u64, off(out.as_mut_ptr()), 1);
    (r == 32).then_some(out)
}

/// This power session's 32-byte key for the calling system process, through its Ira capability (SYS_IRA x4 bit 2, 2026-11-14): the same for every call in one boot, unlinkable to any other boot (the kernel derives it from the wairua and the process's name). None for a kist, with no wairua, or from a kernel before 2026-11-14 (it refuses the call: no domain is passed), so never a lasting key in its place.
pub fn session_key(cap: u64) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    let r = svc(nr::IRA, cap, 0, 0, off(out.as_mut_ptr()), 4);
    (r == 32).then_some(out)
}

/// A kist's alias (docs/FACETS.md "The alias"), through its Ira capability (`kist::IRA`): a 32-byte seed for this kist alone, the same on every device of the person's, derived by the kernel from the person key and this kist's gate identity. The kist makes its keys from it (fgtw::alias::bundle, agreement_secret) and never shows it. None for a process not launched as a kist, before attest, or before the identity service has handed the kernel the person key; a kernel before 2026-11-12 answers as `ira_key` with no domain, which is refused (ERR), so None too.
pub fn alias(cap: u64) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    let r = svc(nr::IRA, cap, 0, 0, off(out.as_mut_ptr()), 2);
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

/// Eagle time's oscillations in one second: the hydrogen-1 hyperfine periods (vsf's `OSCILLATIONS_PER_SECOND`, the definition, kept here so this crate needs no vsf; a payload that has both checks they agree at build).
pub const EAGLE_OSCILLATIONS_PER_SECOND: u64 = 1_420_407_826;

/// The Eagle epoch (1969-07-20 20:17:40 UTC) in the wall clock's seconds since 1970 (vsf's `EAGLE_EPOCH_UNIX_SECS`).
const EAGLE_EPOCH_UNIX: i128 = -14_182_940;

/// Eagle time now, in oscillations, from the wall clock the kernel publishes (`utc_time`): the one conversion every process stamps its documents with (the kernel's settings.rs `eagle_now` is the same).
/// 0, the Eagle epoch, until the kernel has read the PMIC's clock: the convention settings.rs uses for "no clock" (a document then says it was made at the epoch); never a counter.
pub fn eagle_now() -> i64 {
    let Some((secs, ticks, hz)) = utc_time() else { return 0 };
    let ops = EAGLE_OSCILLATIONS_PER_SECOND as i128;
    ((secs as i128 - EAGLE_EPOCH_UNIX) * ops + ticks as i128 * ops / hz as i128) as i64
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
    // Unknown (all ones) where a kernel from before writes fewer facts than this build reads (88 before 2026-11-17, 90 before 2026-11-26).
    let mut b = [0xFFu8; 728];
    // Thru a mutable pointer: the kernel writes the buffer, and an immutable one may be read as the constant it was made.
    svc(nr::FACTS, off(b.as_mut_ptr()), b.len() as u64, 0, 0, 0);
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
/// A process with more grants than the first page holds (108 and the console) has its count and tokens on the pages after it, and the kernel passes the grants offset 0x200 short of them (docs/NO-CEILINGS.md step 5), so this is the same sum.
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

/// The slots a spawn copies into its request (each slot's 32-byte token, as every kernel reads it); past this many the kernel reads the slot numbers where the caller holds them (x2 bit 6) and finds the tokens in the caller's own grants, so a spawn hands any number (docs/NO-CEILINGS.md step 5: the request was cut at 4096 bytes until then, which held 64).
const SPAWN_COPIED: usize = 64;

fn spawn_with(grants: u64, name: &[u8], key: &[u8], slots: &[u64], flags: u64) -> u64 {
    let (nl, kl, n) = (name.len().min(64), key.len().min(64), slots.len());
    if n > SPAWN_COPIED {
        // [name length][name][key length][key][the count, two bytes little-endian]; the slots themselves (u64 each) at x3, their count at x4.
        let mut req = [0u8; 2 + 64 + 64 + 2];
        let mut at = 0;
        req[at] = nl as u8;
        req[at + 1..at + 1 + nl].copy_from_slice(&name[..nl]);
        at += 1 + nl;
        req[at] = kl as u8;
        req[at + 1..at + 1 + kl].copy_from_slice(&key[..kl]);
        at += 1 + kl;
        req[at..at + 2].copy_from_slice(&(n as u16).to_le_bytes());
        at += 2;
        // A count past two bytes is more grants than any page set the kernel builds: the caller's bug, loud.
        assert!(n <= u16::MAX as usize, "spawn: more slots than a request counts");
        return svc(nr::SPAWN, off(req.as_ptr()), at as u64, flags | 64, off(slots.as_ptr()), n as u64);
    }
    let mut req = [0u8; 2 + 64 + 64 + 1 + SPAWN_COPIED * 32];
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

/// Launch a kist (KISTS.md; `SYS_SPAWN` kist mode): the capsule stored in the vault under its 32-byte address `capsule` (the `capsules.` scope) as a process called `name`, with one code per want the capsule declares, in its order: the right's code (ferros_kist::RIGHTS) where the person granted it, G#FF (an empty slot) where they did not, and an implicit right's code always (ferros_kist `launch_codes`). The kernel lays the kist's slots out itself (ferros_kist `slots`, the implicit set handed whatever is sent for it), makes every right's object itself from the Kists capability at grant `kists`, and refuses any right the capsule does not declare in that slot once it has read it. 0 queued (the kernel streams the capsule from the vault and starts it; the outcome goes to the console), `RUNNING_ON` | surface << 8 | generation already running on that surface (its window, display op 59), NOTHING already running with no surface or still starting, ERR refused (the reason on the console). At most 64 slots. `replace`: a live process of that name is killed first (one shot dead) and the kist launched again; with none alive, NOTHING and nothing launched.
pub fn spawn_kist(grants: u64, kists: u64, name: &[u8], capsule: &[u8; 32], codes: &[u8], replace: bool) -> u64 {
    kist_call(grants, kists, name, capsule, codes, 2 | (replace as u64) << 2)
}

/// Stop a running kist (`SYS_SPAWN` kist mode, x2 bit 5; the person's Stop on its card, docs/KISTS.md "The deck and the card"): its live process of that name killed, one shot dead, and nothing launched; no record kept, as for a relaunch's kill (the person's own act). 0 stopped, NOTHING not running (nothing done), ERR refused (no Kists right at grant `kists`).
pub fn stop_kist(grants: u64, kists: u64, name: &[u8], capsule: &[u8; 32]) -> u64 {
    kist_call(grants, kists, name, capsule, &[], 2 | 1 << 5)
}

fn kist_call(grants: u64, kists: u64, name: &[u8], capsule: &[u8; 32], codes: &[u8], flags: u64) -> u64 {
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
    svc(nr::SPAWN, off(req.as_ptr()), at as u64, flags, 0, 0)
}

/// A stow's stages as the kernel tells them on the caller's word (process.rs `stow_tell`): the stage in bits 56-63, below it the bytes read so far, or the reason it was refused.
pub mod stow {
    /// The kernel reading the stored capsule from the vault.
    pub const READING: u64 = 0;
    /// The sandbox checker over its masked code.
    pub const CHECKING: u64 = 1;
    /// Its seal, keyed to this device.
    pub const SEALING: u64 = 2;
    /// Checked and sealed, the seal kept.
    pub const DONE: u64 = 3;
    /// Refused; the low byte says why (`WHY_*`).
    pub const REFUSED: u64 = 4;
    pub const WHY_NOT_NAMED: u64 = 1;
    pub const WHY_SECTIONS: u64 = 2;
    pub const WHY_NATIVE: u64 = 3;
    pub const WHY_SIGNATURE: u64 = 4;
    pub const WHY_MODES: u64 = 5;
    pub const WHY_CHECKER: u64 = 6;
    pub const WHY_SECRET: u64 = 7;
    pub const WHY_KEPT: u64 = 8;
    pub const WHY_READ: u64 = 9;
    pub const WHY_ROOM: u64 = 10;
    /// DONE's value for a stow's check (`stow_check`) that found native code on a development build: taken unchecked and unsealed, as the bridge's development door took it (a launch runs native code on a development build only). A production build refuses it (`WHY_NATIVE`).
    pub const NATIVE: u64 = 1;
    /// A word's stage and its value.
    pub fn split(word: u64) -> (u64, u64) {
        (word >> 56, word & ((1 << 56) - 1))
    }
}

/// Ask the kernel to stow a kist this process stored in `capsules.` under `capsule` (SANDBOX.md "Lifecycle": read from the vault, gated, its masked code checked once, sealed to this device): `stow` the slot of the Stow right, `word` where each stage is told (`stow::*`; wait on it with `wait_world`, never ask on a timer). 0 queued, ERR refused (the reason on the console; one stow at a time).
pub fn stow_kist(grants: u64, stow: u64, capsule: &[u8; 32], word: &core::sync::atomic::AtomicU64) -> u64 {
    let mut req = [0u8; 72];
    req[..32].copy_from_slice(capsule);
    let tok = (arc_base() + cap(grants, stow)) as *const u8;
    for i in 0..32 {
        req[32 + i] = unsafe { core::ptr::read_volatile(tok.add(i)) };
    }
    req[64..].copy_from_slice(&off(word as *const _ as *const u8).to_le_bytes());
    svc(nr::SPAWN, off(req.as_ptr()), req.len() as u64, 8, 0, 0)
}

/// `stow_kist` for a kist offered to stow (docs/KISTS.md "Stowing"): the same check and seal, except that native code on a development build is taken unchecked (told DONE with `stow::NATIVE`), as the bridge's development door took it; a production build refuses it as the directory's path does (`stow::WHY_NATIVE`). The kernel sees native code in the capsule's first bytes and reads no further.
pub fn stow_check(grants: u64, stow: u64, capsule: &[u8; 32], word: &core::sync::atomic::AtomicU64) -> u64 {
    let mut req = [0u8; 72];
    req[..32].copy_from_slice(capsule);
    let tok = (arc_base() + cap(grants, stow)) as *const u8;
    for i in 0..32 {
        req[32 + i] = unsafe { core::ptr::read_volatile(tok.add(i)) };
    }
    req[64..].copy_from_slice(&off(word as *const _ as *const u8).to_le_bytes());
    svc(nr::SPAWN, off(req.as_ptr()), req.len() as u64, 8 | 16, 0, 0)
}

/// A kist's own vault domain sized, for the stower (`stow` the Stow right's token, as `cap` gives it; docs/KISTS.md "The unstow sheet"): the kist named by its identity, the domain the kernel derives for a process of it (the oracle over the developer and the option), so the stower never holds its keys. `SYS_STORE` with the Stow token, x1 bit 4, x2/x3 the identity. Some((how many values, their bytes in all)), None refused (nobody present, no vault process, no Stow right, a kernel from before).
pub fn kist_domain_size(stow: u64, developer: &[u8; 32], option: &[u8; 32]) -> Option<(u64, u64)> {
    let mut id = [0u8; 64];
    id[..32].copy_from_slice(developer);
    id[32..].copy_from_slice(option);
    let mut n = [0u8; 8];
    let r = svc(nr::STORE, stow, 16, off(id.as_ptr()), 64, off(n.as_mut_ptr()));
    (r != ERR).then(|| (u64::from_le_bytes(n), r))
}

/// A kist's own vault domain dropped whole, for the stower (as `kist_domain_size`, x1 bit 3): every value it kept for itself, erased at once (the person cleared it on the unstow sheet; nothing comes back). Some(how many values went), None refused.
pub fn kist_domain_drop(stow: u64, developer: &[u8; 32], option: &[u8; 32]) -> Option<u64> {
    let mut id = [0u8; 64];
    id[..32].copy_from_slice(developer);
    id[32..].copy_from_slice(option);
    let r = svc(nr::STORE, stow, 8, off(id.as_ptr()), 64, 0);
    (r != ERR).then_some(r)
}

/// The kist offered over USB to stow (kernel SYS_OFFER; docs/KISTS.md "Stowing", ferros_kist::offer), for the stower alone: it holds the Stow right (`stow`, the token's offset in the grants page, as `cap` gives it).
pub mod offer {
    use super::{nr, off, svc, ERR, NOTHING};
    /// The offers' doorbell: the kernel writes its change count into `word` now and whenever a host offers a kist, waking whatever waits on it (`wait_world`). False without the Stow right, or a kernel without it.
    pub fn doorbell(stow: u64, word: &core::sync::atomic::AtomicU64) -> bool {
        svc(nr::OFFER, 0, stow, off(word as *const _ as *const u8), 0, 0) == 0
    }
    /// The offer not yet ended, if one is: its capsule's address and length. The stower checks it from there (it is in `capsules.`); taking it again (a restarted stower) gives the same offer.
    pub fn take(stow: u64) -> Option<([u8; 32], u64)> {
        let mut b = [0u8; 40];
        let r = svc(nr::OFFER, 1, stow, off(b.as_mut_ptr()), b.len() as u64, 0);
        if r == ERR || r == NOTHING {
            return None;
        }
        Some((b[..32].try_into().unwrap(), u64::from_le_bytes(b[32..].try_into().unwrap())))
    }
    /// The offer's state as the stower has it (ferros_kist::offer: CHECKING, ASKING, STOWED, REFUSED with its why, CANCELLED; flags bit 0 native), which the kernel answers the host's held read with. False when `capsule` is not the offer (another replaced it).
    pub fn tell(stow: u64, capsule: &[u8; 32], state: u8, why: u8, flags: u8) -> bool {
        let mut b = [0u8; 35];
        b[..32].copy_from_slice(capsule);
        b[32] = state;
        b[33] = why;
        b[34] = flags;
        svc(nr::OFFER, 2, stow, off(b.as_ptr()), b.len() as u64, 0) == 0
    }
}

/// End this process; its arc and capabilities cease to exist.
pub fn exit(code: u64) -> ! {
    svc(nr::EXIT, code, 0, 0, 0, 0);
    loop {}
}

/// Call an endpoint (or the console): `msg` goes to the server whole, at any length (a kernel since docs/NO-CEILINGS.md step 5; before, its first 4096 bytes), its reply lands in `reply`; returns the reply length, ERR on a bad token, when the server's buffer is shorter than `msg` (the server never sees it cut; the console says so) or when the kernel cannot hold `msg`. A reply longer than `reply` is cut to it, as always: `call_long` says when.
pub fn call(cap: u64, msg: &[u8], reply: &mut [u8]) -> u64 {
    svc(nr::CALL, cap, off(msg.as_ptr()), msg.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64)
}

/// `call` that says when the reply was longer than `reply`: `TOO_LONG` | its whole length (`too_long`), its first `reply.len()` bytes in `reply`. A kernel before docs/NO-CEILINGS.md step 5 ignores the flag and cuts it.
pub fn call_long(cap: u64, msg: &[u8], reply: &mut [u8]) -> u64 {
    svc(nr::CALL, cap, off(msg.as_ptr()), msg.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64 | CALL_LONG)
}

/// `call` that fails with ERR at once when no server is receiving on the endpoint (`CALL_LIVE`), instead of waiting for one: for a caller that must not hang on a driver that is down or still bringing its device up.
pub fn call_live(cap: u64, msg: &[u8], reply: &mut [u8]) -> u64 {
    svc(nr::CALL, cap, off(msg.as_ptr()), msg.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64 | CALL_LIVE)
}

/// Receive on an endpoint: the message lands in `buf`, a single-use reply token in `token`; returns the message length. With `poll`, returns NOTHING at once when no message waits.
/// A message longer than `buf` is refused by the kernel, never cut (docs/NO-CEILINGS.md step 5): its caller answered ERR, the console saying who sent how many bytes. For a server whose requests are all shorter than its buffer; one that takes longer ones receives with `recv_long`.
pub fn recv(cap: u64, buf: &mut [u8], token: &mut [u8; 32], poll: bool) -> u64 {
    svc(nr::RECV, cap, off(buf.as_mut_ptr()), buf.len() as u64, off(token.as_mut_ptr()), poll as u64)
}

/// `recv` that is told a message longer than `buf`: `TOO_LONG` | its length (`too_long`), nothing received and no token; the message stays first in the queue for a receive with a buffer that holds it (a kernel since docs/NO-CEILINGS.md step 5; one before cut it to `buf`).
pub fn recv_long(cap: u64, buf: &mut [u8], token: &mut [u8; 32], poll: bool) -> u64 {
    svc(nr::RECV, cap, off(buf.as_mut_ptr()), buf.len() as u64, off(token.as_mut_ptr()), poll as u64 | RECV_LONG)
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

/// `wait` that a call to an endpoint this thread polls (a `recv` with the poll flag) also ends, with 3; a call already waiting returns 3 at once. For a server that waits on its own work (a frame its device is making for a call it holds) and on its callers in one wait, the watchdog's clock running as for any plain wait (kernel ≥ 2026-11-11; an older kernel ignores the flag).
pub fn wait_or_call(word: &core::sync::atomic::AtomicU64, expected: u64, deadline: u64) -> u64 {
    svc(nr::WAIT, off(word as *const _ as *const u8), expected, deadline, 2, 0)
}

/// `wait_world` that a call to an endpoint this thread polls (a `recv` with the poll flag) also ends, with 3; a call already waiting returns 3 at once. A server that waits on a doorbell of its own (a word another of its threads rings) and on its callers in one wait: the IP stack's idle wait (kernel ≥ 2026-11-11; an older kernel ignores the flag and the call waits for the deadline).
pub fn wait_world_or_call(word: &core::sync::atomic::AtomicU64, expected: u64, deadline: u64) -> u64 {
    svc(nr::WAIT, off(word as *const _ as *const u8), expected, deadline, 3, 0)
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

/// Arm the line `line` grants with register operations inside the hardware grant `hw` (offsets from its start), encoded by ferros_layout irqarm `encode`, run by the kernel each time the line fires, before the line is masked and this driver woken, without the kernel lock (docs/KERNEL.md "Armed actions"); empty `program` disarms. 0, or ERR when the kernel refuses it (a register outside the grant, an unknown operation, a bad mask, too many, a grant no armed action may touch). Disarmed by the kernel when this process ends or turns a power switch off.
pub fn irq_arm(line: u64, hw: u64, program: &[u8]) -> u64 {
    svc(nr::IRQ_ARM, line, hw, off(program.as_ptr()), program.len() as u64, 0)
}

/// The cores this thread may run on from now (`CORES_*`; 0 = the workers). Threads it spawns later inherit it.
pub fn pin(mask: u64) -> u64 {
    svc(nr::PIN, mask, 0, 0, 0, 0)
}

/// Answer a call; the token stops existing.
pub fn reply(token: &[u8; 32], msg: &[u8]) -> u64 {
    svc(nr::REPLY, off(token.as_ptr()), off(msg.as_ptr()), msg.len() as u64, 0, 0)
}

/// `caller`'s answer: the caller holds the Settings capability (the map, the attest screen: the system's window management).
pub const CALLER_SETTINGS: u64 = 1 << 8;
/// `caller`'s answer: the caller is a kist.
pub const CALLER_KIST: u64 = 1 << 9;
/// `caller`'s answer: bit `CALLER_SURFACE0 + k` for surface slot k (bits 32 to 47 since 2026-11-12, sixteen slots; bits 0 to 3 for four surfaces before, which the kernel still sets for slots 0 to 3).
pub const CALLER_SURFACE0: u32 = 32;

/// Whether `caller`'s answer `h` holds surface slot k.
#[inline]
pub const fn caller_holds_surface(h: u64, k: usize) -> bool {
    h >> (CALLER_SURFACE0 as usize + k) & 1 == 1
}

/// What the process behind a call holds that its server decides by (kernel SYS_CALLER), asked with the call's reply token before replying: bits 32 to 47 the surface slots it holds (`caller_holds_surface`: slot k its own, or inside a region holding it whole), `CALLER_SETTINGS`, `CALLER_KIST`. None for a token that is no call of this process's, or on a kernel from before (no such syscall): a server refuses then.
pub fn caller(token: &[u8; 32]) -> Option<u64> {
    match svc(nr::CALLER, off(token.as_ptr()), 0, 0, 0, 0) {
        ERR | NOTHING => None,
        h => Some(h),
    }
}

/// `caller_watch`'s answer when the kernel now watches the caller's end (kernel SYS_CALLER x1, 2026-11-18).
pub const CALLER_WATCHED: u64 = 1 << 62;
/// The bit the kernel sets in a `caller_watch` word when the process behind the call ends.
pub const CALLER_ENDED: u64 = 1 << 63;

/// What `caller_watch` found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Watch {
    /// The caller's end is watched: the word gets `CALLER_ENDED` when it ends. Its holds as `caller` answers them.
    Watched(u64),
    /// A kernel from before 2026-11-18: the caller is alive and holds these, but nothing watches its end.
    Unwatched(u64),
    /// The process behind the call has ended already (its reply token went with it).
    Gone,
    /// A kernel without SYS_CALLER, or a word the kernel refused: nothing known.
    Unknown,
}

/// `caller`, and the end of the process behind the call watched from now (kernel SYS_CALLER x1; docs/CAMERA.md "Serving while a frame waits"): when that process ends, any way, the kernel sets `CALLER_ENDED` in `word` (an atomic or, so a count kept in its low bits stays whole) and wakes this process's threads waiting on it. Asked with the call's reply token before replying. One watch a process: a second moves it. The bit is cleared here once the watch is the kernel's (a watch moved away can no longer ring), so an end between the two shows only as the reply to that call failing: the caller checks that reply.
pub fn caller_watch(token: &[u8; 32], word: &core::sync::atomic::AtomicU64) -> Watch {
    match svc(nr::CALLER, off(token.as_ptr()), off(word as *const _ as *const u8), 0, 0, 0) {
        NOTHING => Watch::Gone,
        ERR => Watch::Unknown,
        h if h & CALLER_WATCHED != 0 => {
            word.fetch_and(!CALLER_ENDED, core::sync::atomic::Ordering::SeqCst);
            Watch::Watched(h & !CALLER_WATCHED)
        }
        h => Watch::Unwatched(h),
    }
}

/// Whether the process behind a call this process holds is still alive (its reply token still held): false once it has ended, None on a kernel without SYS_CALLER. A server checks it at the moment it needs the answer (another wants what that caller holds), never on a clock.
pub fn caller_alive(token: &[u8; 32]) -> Option<bool> {
    match svc(nr::CALLER, off(token.as_ptr()), 0, 0, 0, 0) {
        NOTHING => Some(false),
        ERR => None,
        _ => Some(true),
    }
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

/// A DMA region grant's size in bytes (since 2026-11-11), or None from a kernel before it, which answers the region's base instead (told by its being past `most`, the largest size the caller could expect).
pub fn dma_size(cap: u64, most: u64) -> Option<u64> {
    let n = svc(nr::DMA, cap, 1, 0, 0, 0);
    (n != ERR && n <= most).then_some(n)
}

/// Ask the kernel to grow this process's GPU block (the grant at `cap`: a kist's `gpu::kist_slots` block, a spawn's `layers`) to 2^`log` bytes, in place: its base and everything in it stay. Its size after (at least what it held; asking for less leaves it as it is), or None: not a GPU block, past `GPU_BLOCK_MOST`, more than the frame pool can give (the console says which), or a kernel from before 2026-11-13 (which answers the region's base, told by its being past any block's size).
pub fn gpu_block_ask(cap: u64, log: u32) -> Option<u64> {
    let n = svc(nr::DMA, cap, 2, log as u64, 0, 0);
    (n != ERR && n <= GPU_BLOCK_MOST as u64).then_some(n)
}

/// Another surface of this process's own (kernel SYS_DMA x1 = 10 on a surface it holds, at `held`; 2026-11-20, docs/GPU-KISTS.md "Layers"): its token written into `out` (the caller keeps it: `dma(off(out))` answers the surface's base past its porch again), its base past the porch now. A window on it is a layer the display composes front to back with the caller's others, and two make a layer's mailbox (`window::show_buffer`, op 22's 36-byte form). At most four a process; None past that, with no slot free or the pool short (the console says which), or on a kernel from before.
pub fn surface_ask(held: u64, out: &mut [u8; 32]) -> Option<u64> {
    let n = svc(nr::DMA, held, 10, off(out.as_mut_ptr()), 0, 0);
    (n != ERR && n != 0).then_some(n)
}

/// Ask the kernel to resize this puck's square (the grant at `cap`: a puck's surface slot, docs/PUCKS.md) to hold `bytes` past its porch, larger or smaller, in whole frames of the pool; its base (`dma`) never moves. The bytes it holds after, or None: not the caller's square, past `PUCK_BYTES`, more than the pool can give (the console says which), or a kernel from before 2026-11-18.
pub fn puck_size(cap: u64, bytes: u64) -> Option<u64> {
    let n = svc(nr::DMA, cap, 6, bytes, 0, 0);
    (n != ERR && n < PUCK_BYTES as u64).then_some(n)
}

/// Copy spawn input into `buf`, starting at byte `off`; returns how many bytes (0 at the end). Large one-shot input (firmware) is streamed this way so the arc stays small.
pub fn input(start: u64, buf: &mut [u8]) -> usize {
    svc(nr::INPUT, start, off(buf.as_mut_ptr() as *const u8), buf.len() as u64, 0, 0) as usize
}

/// Sleep until the generic timer count reaches `deadline` (see `now`/`freq`): the process is parked, not scheduled, until then. A call to an endpoint this thread polls ends it early (`sleep_until_or_call` says which).
pub fn sleep_until(deadline: u64) {
    svc(nr::SLEEP, deadline, 0, 0, 0, 0);
}

/// `sleep_until`, saying why it ended: true when a call to an endpoint this thread polls ended it (or was already waiting), false at the deadline (kernel ≥ 2026-11-11; an older one always says false).
pub fn sleep_until_or_call(deadline: u64) -> bool {
    svc(nr::SLEEP, deadline, 0, 0, 0, 0) == 1
}

/// Say something on the console (grant slot 0).
pub fn console(grants: u64, text: &[u8]) {
    let mut r = [0u8; 16];
    call(cap(grants, 0), text, &mut r);
}

/// Bit 62 of a console write's reply length: the kernel writes the line to the flash log as well (process.rs CONSOLE_FLASH).
pub const CONSOLE_FLASH: u64 = 1 << 62;

/// `console`, and the same line into the flash log, which survives a reset that loses the RAM (a driver's steps toward a device that may hang the SoC). A few lines a session: each is a flash write under the kernel lock.
pub fn console_flash(grants: u64, text: &[u8]) {
    let mut r = [0u8; 16];
    svc(nr::CALL, cap(grants, 0), off(text.as_ptr()), text.len() as u64, off(r.as_mut_ptr()), r.len() as u64 | CONSOLE_FLASH);
}

/// Exit with G#DEAD and nothing said: a panic handler that has no `PanicInfo` to hand (`panic_report` is the one to call).
pub fn panic_exit() -> ! {
    exit(0xDEAD)
}

/// The grants a payload's panic is said thru (`panic_report`: slot 0, the console), set by the payload at its start; before that (`u64::MAX`, never a grants address) nothing is said on the console (the kernel still says it, from `panic_tell`). Not 0: a masked process's grants are at its arc's offset 0 (2026-11-18: the pucks' first panic report said nothing for that).
static PANIC_GRANTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);

/// Where `panic_report` says a panic: the payload's grants, as its entry got them.
pub fn panic_grants(grants: u64) {
    PANIC_GRANTS.store(grants, Ordering::Relaxed);
}

/// What a payload's panic handler calls (`ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)`): the panic's message, file, line and column to the kernel (`panic_tell`), then, when the payload gave its grants (`panic_grants`), `name: panicked at file:line:col: message` on the console, then exit G#DEAD.
/// The kernel keeps the report with the call chain it walks from here, and the process's end goes to the flash log, the console and (with no display) the glass whole: the message, `file:line:column`, the thread, and the frames as offsets into the arc for the host to name (`ferros-bridge deaths`, KERNEL.md "A process that dies").
/// The console line is for a kernel from before SYS_PANIC, whose death line takes the last console line (2026-11-18: GPS and Compass panicked on the 8a with an empty last line, their handler having said nothing).
/// No allocation: the message is formatted once, into a buffer on the stack.
pub fn panic_report(name: &str, info: &core::panic::PanicInfo) -> ! {
    // WHY the count: formatting the message runs the panicking code's own Display impls, which can panic again, and a second panic comes back here; formatting again would recurse until the stack ran out.
    // The second tells the kernel where it is with its message only when that needs no formatting (else a fixed one), so a panic inside a panic's message still says where (2026-11-18, panictest 03: it said nothing); a third (or another thread's at the same time) goes straight to the exit.
    static TELLING: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
    let nth = TELLING.fetch_add(1, Ordering::AcqRel);
    if nth == 1 {
        let (file, line, column) = info.location().map_or(("", 0, 0), |l| (l.file(), l.line(), l.column()));
        let m = info.message().as_str().unwrap_or("a panic while formatting a panic's message");
        panic_tell(m.as_bytes(), file.as_bytes(), line, column);
    }
    if nth == 0 {
        let mut buf = PanicText { b: [0; PANIC_TEXT], n: 0 };
        let msg = info.message();
        let text: &[u8] = match msg.as_str() {
            Some(s) => s.as_bytes(),
            None => {
                let _ = core::fmt::write(&mut buf, format_args!("{msg}"));
                &buf.b[..buf.n]
            }
        };
        let (file, line, column) = info.location().map_or(("", 0, 0), |l| (l.file(), l.line(), l.column()));
        panic_tell(text, file.as_bytes(), line, column);
        let grants = PANIC_GRANTS.load(Ordering::Relaxed);
        if grants != u64::MAX {
            let mut l = PanicLine { b: [0; PANIC_TEXT << 1], n: 0 };
            // The message as already formatted (cut at a character boundary by `PanicText`, so still UTF-8), never formatted a second time.
            let m = core::str::from_utf8(text).unwrap_or("");
            let _ = core::fmt::write(&mut l, format_args!("{name}: panicked at {file}:{line}:{column}: {m}"));
            l.b[l.n] = b'\n';
            console(grants, &l.b[..l.n + 1]);
        }
    }
    exit(0xDEAD)
}

/// The console's line: the name, the place and the message, the last byte kept for the newline.
struct PanicLine {
    b: [u8; PANIC_TEXT << 1],
    n: usize,
}

impl core::fmt::Write for PanicLine {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &c in s.as_bytes() {
            // WHY the bound: a file name and a message are whatever the panic gave; past the line's room they are cut, never written past it, and the last byte stays for the newline.
            if self.n < self.b.len() - 1 {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        Ok(())
    }
}

/// The most of a panic's message `panic_report` formats: the kernel keeps 2^9 bytes of it (ferros_ledger death MESSAGE_MAX).
const PANIC_TEXT: usize = 1 << 9;

/// Text into a fixed buffer, whole characters only, the rest dropped.
struct PanicText {
    b: [u8; PANIC_TEXT],
    n: usize,
}

impl core::fmt::Write for PanicText {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // WHY the cut: the message is any length and the buffer is fixed; it is cut at a character boundary so what the kernel gets is still UTF-8.
        let mut k = s.len().min(PANIC_TEXT - self.n);
        while !s.is_char_boundary(k) {
            k -= 1;
        }
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}

/// SYS_PANIC: tell the kernel this process is panicking, with its message (UTF-8), the source file as the compiler named it, and the line and column.
/// The kernel keeps the first 2^9 bytes of the message and 2^8 of the file, the calling thread's name, and the call chain it walks from this call's frame (x29) with the program counter (a frame-pointer walk, each record checked to lie in the arc and above the last).
/// It does not end the process: the exit that follows does, and its end carries the report. A std program's hook calls it thru ferros_rt, then aborts.
/// Not inlined, so the walk starts at a frame of its own.
#[inline(never)]
pub fn panic_tell(message: &[u8], file: &[u8], line: u32, column: u32) {
    svc(nr::PANIC, off(message.as_ptr()), message.len() as u64, off(file.as_ptr()), file.len() as u64, line as u64 | (column as u64) << 32);
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
    pub use ferros_theme::{Theme, THEMES};

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
    /// Above `APP` is the system's alone (the Settings holder, never a kist): the display refuses another caller's show there with `NOT_YOURS` (WINDOWS.md "Whose level").
    pub const APP: u8 = 0;
    pub const INPUT: u8 = 1;
    pub const SYSTEM: u8 = 2;
    /// A surface pixel that shows what is behind it.
    pub const CLEAR: u64 = 1 << 63;
    /// A reply's status for an op naming a surface the caller does not hold, or acting on a window that is neither the caller's nor the caller the system's (WINDOWS.md "Whose surface").
    pub const NOT_YOURS: u8 = 4;
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
    /// A surface already in the panel's codes (the display's COMPOSITOR.md "Baked"; owner, 2026-11-12): R, G, B in bits 0-15, 16-31, 32-47, each the panel's own code (its primaries, the roll-off and its gamma applied by the producer, `bake`) in Q16 of its top, premultiplied; bit 63 see-through, bits 49-62 the transparency (Q14), bit 48 no dither, as `WORKING`'s. The display composes and presents them as they are.
    pub const BAKED: u8 = 2;

    /// The bake a `BAKED` surface is made with (display op 65), its generation first: a working colour (VSF RGB, gamma 2, white at 1) v becomes, per channel, l = tanh²(k·v); the panel's light p = m·l (rows the panel's red, green, blue; columns VSF's); its code p^g, × 65472 (the panel's top code in Q16 less its last quarter step), held at the field's ends. A brightness change moves the generation and is told to every window as `EV_BAKE`.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Bake {
        pub generation: u32,
        pub k: f32,
        pub m: [[f32; 3]; 3],
        pub g: f32,
    }

    /// The display's bake now (op 65 → `[0][generation u32][k f32][m 9 × f32, row by row][g f32]`, little-endian), None from a display without it.
    pub fn bake(display: u64) -> Option<Bake> {
        let mut r = [0u8; 49];
        if super::call(display, &[65], &mut r) != 49 || r[0] != 0 {
            return None;
        }
        let f = |i: usize| f32::from_le_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]);
        Some(Bake { generation: u32::from_le_bytes([r[1], r[2], r[3], r[4]]), k: f(5), m: core::array::from_fn(|i| core::array::from_fn(|j| f(9 + 4 * (3 * i + j)))), g: f(45) })
    }

    /// As `show`, saying what the surface's pixels are (`WORKING`, `G2T16` or `BAKED`).
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

    /// A layer's mailbox show (op 22's 36-byte form, GPU-KISTS.md 1.4 and "Layers"): the window on surface `base` shows the pixels of surface `buffer` (the caller's own, `super::surface_ask`) from this show on, `show_full`'s other fields as there. Answered once the display has taken it, so the buffer shown before is no longer read and is the caller's to draw into again: two buffers a layer, swapped show by show.
    /// A window keeps its place in the stack across shows that leave its place, level, format and mode as they were: a kist's layers are stacked by their first shows, the one shown first behind.
    #[allow(clippy::too_many_arguments)]
    pub fn show_buffer(display: u64, base: u64, buffer: u64, x: u16, y: u16, w: u16, h: u16, level: u8, format: u8, changed: (u16, u16, u16, u16), mode: u8) -> bool {
        let mut m = [0u8; 36];
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
        m[28..36].copy_from_slice(&buffer.to_le_bytes());
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

    // ---- front, behind and keys (WINDOWS.md "The helm", owner 2026-11-11: "Home backgrounds the kist so depending on how it was designed, like Photon, it doesn't die but it stops rendering everything and stops playing anything except for notifications", "back should signal escape to the app and go up one page") ----

    /// A window's event (display op 62): its kind in the high byte, then for a key its USB HID usage (keyboard page) in the low one.
    /// Behind: the window left the glass (deck, or a flip to another kist); the process lives on, nothing it shows reaches the glass until it is in front again. Draw nothing; play nothing but notifications.
    pub const EV_BEHIND: u8 = 1;
    /// In front again (its circle tapped, a flip back): the display composes it from its surface as it stands.
    pub const EV_FRONT: u8 = 2;
    /// A key pressed for this window (back's tap: `KEY_ESCAPE`).
    pub const EV_KEY: u8 = 3;
    /// The person opened a note of this kist's (docs/NOTIFY.md "A tap"): the low byte its number; the note's document, with where to land (`target`), from `notify::opened`.
    pub const EV_OPEN_AT: u8 = 5;
    /// Text the system keyboard typed for this window (display op 70; WINDOWS.md "The keyboard"): UTF-8, its length in the low byte, the bytes in the reply's payloads (`PAYLOAD_KINDS`).
    pub const EV_TEXT: u8 = 6;
    /// The keyboard's place changed: its payload two bytes, little-endian, the screen rows it covers from the bottom up (0: gone). The window keeps what it is typing into above them.
    pub const EV_INSET: u8 = 7;
    /// The window's safe area (WINDOWS.md "Drawn under the controls", owner 2026-11-12: "Should we draw under the controls?", then "Agreed. Build it!"): a window fills the glass, and the system's controls float in front of its corners, so it keeps its own buttons and text where nothing covers them.
    /// The payload is `SAFE_BYTES` long: the app's circle (`active_circle`) in the window's own pixels, its top-left corner the origin, centre x and centre y as i32 and the radius as u32, each in 256ths of a pixel, little-endian; then a byte of `SAFE_TOP` and `SAFE_BOTTOM`, which of the glass outside the circle the system holds.
    /// A point is the window's to use when it is inside the circle, or outside it on a half (above the circle's centre, or below it) whose bit is clear.
    /// Told when the window first shows, and again whenever it changes (full screen on or off); the keyboard's place is `EV_INSET`'s, apart from it.
    pub const EV_SAFE: u8 = 8;
    /// `EV_SAFE`'s payload length.
    pub const SAFE_BYTES: usize = 13;
    /// `EV_SAFE`'s byte: the glass outside the circle above its centre is the system's (the console's circles, drawn and touched there).
    pub const SAFE_TOP: u8 = 1;
    /// `EV_SAFE`'s byte: the glass outside the circle below its centre is the system's (the helm's wedges, Back and Deck: touched there even in full screen, where they are not drawn).
    pub const SAFE_BOTTOM: u8 = 2;
    /// Kinds from this one up carry a payload: the low byte is its length, and the payloads follow the reply's events, in the events' order. A kind a reader does not know is stepped over by its length.
    pub const PAYLOAD_KINDS: u8 = 6;
    /// The most bytes one event's payload holds (a typed word, a key's text); longer text is sent in pieces.
    pub const PIECE: usize = 1 << 6;
    /// The display's bake changed (a brightness change): a `BAKED` surface is made again with the new one (`bake`), every pixel of it.
    pub const EV_BAKE: u8 = 4;
    /// Escape, by its USB HID usage: what back's tap sends the app in front (up one page; nothing at its main page).
    pub const KEY_ESCAPE: u8 = 0x29;
    /// Again (HID G#79): Back's swipe set to go forward, the redo of the last step back (fluor `NamedKey::Forward`; PREF_HELM `BACK_SWIPE_FORWARD`).
    pub const KEY_AGAIN: u8 = 0x79;
    /// Enter and backspace, by their USB HID usages: the keyboard's enter and delete keys.
    pub const KEY_ENTER: u8 = 0x28;
    pub const KEY_BACKSPACE: u8 = 0x2A;
    /// How many events op 62 holds a window: the newest, a ring (a window that missed more still learns whether it is behind, the reply's state byte).
    pub const EVENTS: usize = 1 << 4;
    /// The longest op 62 reply: the head, the events, and every event's payload at its longest.
    pub const EVENTS_REPLY: usize = 7 + 2 * EVENTS + EVENTS * PIECE;

    // ---- the keyboard (WINDOWS.md "The keyboard", 2026-11-12): an app asks for it, the system keyboard (the attest screen's process after attest) shows it at the input level and types into the app in front, the display carrying both ways ----

    /// This window wants the keyboard (`on`) or no longer does (display op 68), as a text field gains or loses focus. True when the display took it; false with the window not the app in front (the keyboard is the front app's alone) or a display without the op.
    pub fn text_wanted(display: u64, base: u64, on: bool) -> bool {
        let mut m = [0u8; 10];
        m[0] = 68;
        m[1..9].copy_from_slice(&base.to_le_bytes());
        m[9] = on as u8;
        let mut r = [0u8; 4];
        call(display, &m, &mut r) >= 1 && r[0] == 0
    }

    /// The keyboard's doorbell (display op 69, the system's alone): held by the display until whether an app wants the keyboard changed from what `seen` counted. (the count now, wanted), None from a display without the op or a caller that is not the system's. Call it from a thread of its own: the call waits.
    pub fn keyboard_watch(display: u64, seen: u32) -> Option<(u32, bool)> {
        let mut m = [0u8; 5];
        m[0] = 69;
        m[1..5].copy_from_slice(&seen.to_le_bytes());
        let mut r = [0u8; 8];
        (call(display, &m, &mut r) >= 6 && r[0] == 0).then(|| (u32::from_le_bytes([r[1], r[2], r[3], r[4]]), r[5] == 1))
    }

    /// Whether an app window is in front (display op 5's byte 12) as a doorbell (display op 75, anyone's): held by the display until that changed from what `seen` counted.
    /// (the count now, in front), None from a display without the op. Call it with a `seen` no answer gave (u32::MAX) for the state at once; with the count it answered, it waits.
    pub fn front_watch(display: u64, seen: u32) -> Option<(u32, bool)> {
        let mut m = [0u8; 5];
        m[0] = 75;
        m[1..5].copy_from_slice(&seen.to_le_bytes());
        let mut r = [0u8; 8];
        (call(display, &m, &mut r) >= 6 && r[0] == 0).then(|| (u32::from_le_bytes([r[1], r[2], r[3], r[4]]), r[5] == 1))
    }

    /// Text typed into the window that wants the keyboard while it is the app in front (display op 70, the system's alone; `EV_TEXT`): at most `PIECE` bytes (more is the caller's bug, and panics). True when it went.
    pub fn type_text(display: u64, text: &[u8]) -> bool {
        let mut m = [0u8; 2 + PIECE];
        m[0] = 70;
        m[2..2 + text.len()].copy_from_slice(text);
        let mut r = [0u8; 4];
        call(display, &m[..2 + text.len()], &mut r) >= 1 && r[0] == 0
    }

    /// A key, by its USB HID usage, to the window that wants the keyboard while it is the app in front (display op 70, `EV_KEY`): enter, backspace. True when it went.
    pub fn type_key(display: u64, usage: u8) -> bool {
        let mut r = [0u8; 4];
        call(display, &[70, usage], &mut r) >= 1 && r[0] == 0
    }

    /// The keyboard put away (display op 71, the system's alone): no app wants it until one asks again, and the one that did is told the keyboard's place is gone (`EV_INSET` 0).
    pub fn keyboard_away(display: u64) {
        let mut r = [0u8; 4];
        call(display, &[71], &mut r);
    }

    /// The window shown on `surface` under `generation` to the front of its level (display op 59), and told so (`EV_FRONT`). True when there was such a window.
    pub fn front(display: u64, surface: u8, generation: u8) -> bool {
        let mut r = [0u8; 4];
        call(display, &[59, surface, generation], &mut r) >= 1 && r[0] == 0
    }

    /// The app window in front to the background (display op 60), told so (`EV_BEHIND`): its surface and generation, None when no app window was in front.
    pub fn behind(display: u64) -> Option<(u8, u8)> {
        let mut r = [0u8; 4];
        (call(display, &[60], &mut r) >= 3 && r[0] == 0).then_some((r[1], r[2]))
    }

    /// A key, by its USB HID usage, to the app window in front (display op 61, `EV_KEY`): its surface and generation, None when no app window was in front.
    pub fn key(display: u64, usage: u8) -> Option<(u8, u8)> {
        let mut r = [0u8; 4];
        (call(display, &[61, usage], &mut r) >= 3 && r[0] == 0).then_some((r[1], r[2]))
    }

    /// The person opened note `number` of the kist in front (display op 67, `EV_OPEN_AT`; the deck alone, docs/NOTIFY.md "A tap"): its surface and generation, None when no app window was in front (or the caller is not the system's window management).
    pub fn open_at(display: u64, number: u8) -> Option<(u8, u8)> {
        let mut r = [0u8; 4];
        (call(display, &[67, number], &mut r) >= 3 && r[0] == 0).then_some((r[1], r[2]))
    }

    /// The helm's Deck button as the deck's summary (display op 66; owner, 2026-11-12: "if anything on the deck has changed state, specifically 400nm notifications, we change the deck circle in the helm", then "So ideally the deck circle shows the latest item icon preview and shows the violet ring?"): `icon` the latest note's source's icon in the display's bundle (0: none, the ring alone), `count` how many wait in the deck's resting view (0: the plain Deck mark), `glow` its ring and glow as ferros_glow's state byte: for you (4, 400 nm), or a question (6, 703 nm: the system waits on the person's answer; owner, 2026-11-12: "only difference is it would be 700nm red, rather than the regular 400nm notification glow"). True when the display took it.
    pub fn helm_note(display: u64, icon: u8, count: u16, glow: u8) -> bool {
        let c = count.to_le_bytes();
        let mut r = [0u8; 4];
        call(display, &[66, icon, c[0], c[1], glow], &mut r) >= 1 && r[0] == 0
    }

    /// This window's events after `seen` (display op 62), held by the display until there is one: (the count now, behind, the events since `seen`, oldest first, at most `EVENTS`). None when the display has no such op. Call it from a thread of its own: the call waits.
    pub fn events(display: u64, base: u64, seen: u32, out: &mut [u16; EVENTS]) -> Option<(u32, bool, usize)> {
        let mut m = [0u8; 13];
        m[0] = 62;
        m[1..9].copy_from_slice(&base.to_le_bytes());
        m[9..13].copy_from_slice(&seen.to_le_bytes());
        let mut r = [0u8; 7 + 2 * EVENTS];
        let n = call(display, &m, &mut r);
        if n == super::ERR || n < 7 || r[0] != 0 {
            return None;
        }
        // WHY the bounds: the count byte and the length are the display's (another process's word), and a reply longer than the buffer is cut at it; only whole events that landed in the buffer are read, at most `out`'s.
        let k = (r[6] as usize).min(EVENTS).min(((n as usize).min(r.len()) - 7) / 2);
        for (i, e) in out[..k].iter_mut().enumerate() {
            *e = u16::from_le_bytes([r[7 + 2 * i], r[8 + 2 * i]]);
        }
        Some((u32::from_le_bytes([r[1], r[2], r[3], r[4]]), r[5] & 1 == 1, k))
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

/// The identity doorbell's word (`identity_doorbell`): an identity is unlocked.
pub const ID_UNLOCKED: u64 = 1;
/// The identity doorbell's word: an identity's fleet has bound this device.
pub const ID_BOUND: u64 = 2;
/// The identity doorbell's word: the kernel holds the person key (FACETS.md D1).
pub const ID_PERSON: u64 = 4;

/// The identity's doorbell (SYS_UNLOCK op 11, kernel from 2026-11-16), given the Unlock right: the kernel writes the state into `word` now and at every change (`ID_UNLOCKED`, `ID_BOUND`, `ID_PERSON`; a change count above bit 8), waking whatever waits on it (`wait_world`).
/// The word must live as long as the process (a static): the kernel keeps writing it.
/// False without the capability, or from a kernel before.
pub fn identity_doorbell(cap: u64, word: &core::sync::atomic::AtomicU64) -> bool {
    svc(nr::UNLOCK, cap, 11, off(word as *const _ as *const u8), 0, 0) == 0
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

/// Whether the kernel holds the person key (docs/FACETS.md D1): handed in by the identity service, wiped at lock. None without the capability; false from a kernel before 2026-11-12.
pub fn person_key_held(cap: u64) -> Option<bool> {
    let r = svc(nr::UNLOCK, cap, 0, 0, 0, 0);
    (r != ERR).then_some(r & 4 == 4)
}

/// Hand the person key to the kernel (SYS_UNLOCK op 10, the Unlock right; docs/FACETS.md D1): it derives each kist's alias from it and wipes it at lock. True when held; false while nobody is present, without the right, or from a kernel before 2026-11-12.
pub fn give_person_key(cap: u64, key: &[u8; 32]) -> bool {
    svc(nr::UNLOCK, cap, 10, off(key.as_ptr()), 0, 0) == 0
}

/// Android on this device (SYS_UNLOCK op 13, the Settings right; docs/BOOT.md "Android only by the identity's choice"): what the Settings page asks of the kernel. Each but `State` only while a person is present.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Android {
    /// Start Android now: ferros leaves the gate its word and resets (the device must allow Android).
    StartNow = 0,
    /// Android off on this device; what it starts into goes back to ferros.
    /// (2 was Android on again until 2026-11-24: Android on is an approve on the attest screen now, `approve::ask` with ahau_core::approve::ANDROID_ON, and the kernel refuses 2.)
    Off = 1,
    /// The state: bit 0 this device allows Android, bit 1 the person present accepted the disclaimer, bit 2 it starts into Android.
    State = 3,
    /// What this device starts into (the person's standing choice, applied after the attest of a boot nobody answered): ferros, always taken.
    StartIntoFerros = 4,
    /// Android: only while a person is present on a device that allows Android.
    StartIntoAndroid = 5,
    /// Android's recovery request waiting on the person (docs/BOOT.md "Recovery requests from Android"): one of the `RECOVERY_*` classes, `RECOVERY_NONE` while there is none or nobody is present.
    RecoveryAsk = 6,
    /// Yes to it, once: the device restarts into Android's recovery with what Android asked (only `RECOVERY_WIPE` and `RECOVERY_RESCUE`, the person's alone to allow).
    RecoveryYes = 7,
    /// No to it: the request is cleared, and ferros stays.
    RecoveryNo = 8,
}

/// Android's recovery request classes (`Android::RecoveryAsk`), the kernel's (ferros_hal::bcb `ASK_*`), by the proof each needs (owner, 2026-11-02).
/// None waiting.
pub const RECOVERY_NONE: u64 = 0;
/// A factory reset: erase Android's data. The attest allows it.
pub const RECOVERY_WIPE: u64 = 1;
/// Android could not start: its rescue screen, which offers to erase its data. The attest allows it.
pub const RECOVERY_RESCUE: u64 = 2;
/// A wipe asked from afar (a device admin, Find My Device): refused, only no.
pub const RECOVERY_REMOTE: u64 = 3;
/// Something that can change what boots (an update, a sideload, fastbootd, the recovery menu): needs a second fleet device as well, not built, so only no.
pub const RECOVERY_CHANGES: u64 = 4;

/// Ask the kernel `what` (`Android`) with the Settings right `cap`: 1 done (or the state's bits), 0 refused, None without the right or a vault.
pub fn android(cap: u64, what: Android) -> Option<u64> {
    match svc(nr::UNLOCK, cap, 13, what as u64, 0, 0) {
        ERR => None,
        n => Some(n),
    }
}

/// Remove this device from its identity (SYS_UNLOCK op 15, the identity service alone; docs/AUTH.md "Remove this device"): the first `len` bytes of `buf` hold [flags: bit 0 a dry run, bit 1 a shred (fgtw::security: the same erase with no departure, under the SHRED approval, ending in a restart rather than the power off)][the typed handle's seed 32][each kist of the person's, (developer 32, option 32)].
/// A removal → Some(1) removed (the device powers off once the kernel's drops are sent), Some(0) the seed is not the person present's (nothing changed).
/// A removal needs the person's approve on the attest screen held in the kernel (ahau_core::approve::REMOVE_DEVICE, since 2026-11-24; a shred, SHRED), which it takes: without it, None and nothing changed.
/// A dry run → Some(the account's whole length), its text written over `buf`; nothing changed.
/// None: refused (nobody present, no right, a buffer of another shape, a kernel before 2026-11-22).
pub fn remove_device(cap: u64, buf: &mut [u8], len: usize) -> Option<u64> {
    match svc(nr::UNLOCK, cap, 15, off(buf.as_mut_ptr()), len as u64, buf.len() as u64) {
        ERR => None,
        n => Some(n),
    }
}

/// Attest to approve (owner, 2026-11-24; docs/AUTH.md "Approve on the attest screen"): a protective change, or a change to a setting under a "require attest" guard, approved on the attest screen itself.
/// A request is ahau_core::approve's word (its kind in bits 0-7, its number above): never text, so the screen says what it approves in the person's own words.
pub mod approve {
    use super::{nr, svc, ERR, NOTHING};

    /// Ask for a change to be approved (SYS_UNLOCK op 16, the Settings right, never a kist): the kernel keeps the request, locks the identity at once (its keys dropped, as any lock), and starts the attest screen again over everything, which shows what is asked.
    /// The person's handle typed there unlocks and the kernel applies exactly that change; a failed attest leaves the device locked with nothing applied. There is no Decline (owner, 2026-11-24: "You can toggle off again if you screw up.").
    /// True when asked; false when refused (nobody present, the device not bound, a request the kernel does not take, no right).
    pub fn ask(settings: u64, word: u64) -> bool {
        svc(nr::UNLOCK, settings, 16, word, 0, 0) == 0
    }

    /// Lock now (SYS_UNLOCK op 21, the Settings right, never a kist; fgtw::security, the Security page): the identity locked at once and the attest screen started again, as an ask does, with nothing asked. It only tightens, so it needs no approve.
    /// True when locked; false when nobody is present or the right is missing (a kernel before 2026-11-24 refuses it too).
    pub fn lock(settings: u64) -> bool {
        svc(nr::UNLOCK, settings, 21, 0, 0, 0) == 0
    }

    /// The request waiting on the attest screen (SYS_UNLOCK op 17, the Unlock or the Settings right): its word, None when there is none.
    pub fn pending(cap: u64) -> Option<u64> {
        match svc(nr::UNLOCK, cap, 17, 0, 0, 0) {
            ERR | 0 => None,
            w => Some(w),
        }
    }

    /// The "require attest" guards on (SYS_UNLOCK op 19 [0], the Settings or the Unlock right): their bits (ahau_core::approve::guard), None without the right.
    pub fn guards(cap: u64) -> Option<u64> {
        match svc(nr::UNLOCK, cap, 19, 0, 0, 0) {
            ERR => None,
            g => Some(g),
        }
    }

    /// Guard `g` on (SYS_UNLOCK op 19 [1], the Settings right): no attest, since tightening is always safe; kept in the device's settings document at once. The guards' bits after, None when refused.
    /// Off is an approve (`ask` with ahau_core::approve::GUARD_OFF).
    pub fn guard_on(settings: u64, g: u8) -> Option<u64> {
        match svc(nr::UNLOCK, settings, 19, 1, g as u64, 0) {
            ERR => None,
            b => Some(b),
        }
    }

    /// What became of an approval asked for (`take`).
    pub enum Taken {
        /// Approved: the word, now this caller's to carry out.
        Approved(u64),
        /// Asked and waiting on the attest screen.
        Waiting,
        /// Neither: failed, never asked, or gone with a lock.
        No,
    }

    /// `take`, only looking: an approval of `kind` approved and held is left for the process that takes it (the map looks, the identity service takes as it opens the add flow).
    pub fn held(cap: u64, kind: u8) -> Taken {
        match svc(nr::UNLOCK, cap, 20, kind as u64, 1, 0) {
            NOTHING => Taken::Waiting,
            ERR | 0 => Taken::No,
            w => Taken::Approved(w),
        }
    }

    /// What became of an approval of `kind` that a process carries out itself (SYS_UNLOCK op 20, the Settings or the Unlock right; ahau_core::approve `kernel` false): approved, its word taken (the kernel holds it no longer); still waiting; or neither.
    pub fn take(cap: u64, kind: u8) -> Taken {
        match svc(nr::UNLOCK, cap, 20, kind as u64, 0, 0) {
            NOTHING => Taken::Waiting,
            ERR | 0 => Taken::No,
            w => Taken::Approved(w),
        }
    }
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

/// Attest with a tag (AUTH.md "Attest with a tag"): how many bytes of a kept tag's id the list gives and a forget names.
pub const TAG_SHOWN: usize = 8;

/// A kept tag as the kernel lists it: the start of its id (a keyed hash of its UID, never the UID) and when it was kept (Eagle time, oscillations; 0 when the record does not say).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct KeptTag {
    pub id: [u8; TAG_SHOWN],
    pub added: i64,
}

/// Unlock by a tag (SYS_UNLOCK op 6, the Unlock right): the kernel opens the record it keeps for the tag whose UID this is and unlocks its identity as `unlock_identity` would; the identity's seed lands in `seed`, for the identity service. `Failed` for a tag kept for nothing here, one whose record does not open, a UID that is not 4, 7 or 10 bytes, or no right: one refusal, no detail.
pub fn unlock_by_tag(cap: u64, uid: &[u8], seed: &mut [u8; 32]) -> Unlock {
    match svc(nr::UNLOCK, cap, 6, off(uid.as_ptr()), uid.len() as u64, off(seed.as_mut_ptr())) {
        ERR => Unlock::Failed,
        NOTHING => Unlock::Belongs,
        n => Unlock::Opened(n),
    }
}

/// Keep the tag whose UID this is for the identity present (SYS_UNLOCK op 7, the Unlock right or Settings): Some(true) kept, Some(false) kept already, None refused (nobody present, or it could not be kept).
pub fn keep_tag(cap: u64, uid: &[u8]) -> Option<bool> {
    match svc(nr::UNLOCK, cap, 7, off(uid.as_ptr()), uid.len() as u64, 0) {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

/// Forget the kept tag whose id starts so, or every kept tag (None) (SYS_UNLOCK op 8): how many went, None refused.
pub fn forget_tag(cap: u64, which: Option<&[u8; TAG_SHOWN]>) -> Option<u64> {
    let r = match which {
        Some(w) => svc(nr::UNLOCK, cap, 8, off(w.as_ptr()), TAG_SHOWN as u64, 0),
        None => svc(nr::UNLOCK, cap, 8, 0, 0, 0),
    };
    (r != ERR).then_some(r)
}

/// The kept tags (SYS_UNLOCK op 9) into `out`: how many are kept and how many of them `out` took, None refused (nobody present).
pub fn kept_tags(cap: u64, out: &mut [KeptTag]) -> Option<(usize, usize)> {
    const ENTRY: usize = TAG_SHOWN + 8;
    let mut b = [0u8; ENTRY << 4];
    let n = svc(nr::UNLOCK, cap, 9, off(b.as_mut_ptr()), b.len() as u64, 0);
    if n == ERR {
        return None;
    }
    let took = (n as usize).min(out.len()).min(b.len() / ENTRY);
    for (k, t) in out[..took].iter_mut().enumerate() {
        let e = &b[k * ENTRY..(k + 1) * ENTRY];
        t.id.copy_from_slice(&e[..TAG_SHOWN]);
        t.added = i64::from_le_bytes([e[8], e[9], e[10], e[11], e[12], e[13], e[14], e[15]]);
    }
    Some((n as usize, took))
}

/// The NFC driver's unlock tags (payloads/nfc ops 9 and 10, attest with a tag).
pub mod nfc {
    /// Held until the NFC driver reads an unlock tag (NFC-A, Type 2, a 7- or 10-byte UID): its UID into `uid` and its length; None when the driver let the call go (op 10, or more callers held than it keeps) or is not there. Call it from a thread of its own: it waits as long as no tag comes.
    pub fn tag_wait(nfc: u64, uid: &mut [u8; 10]) -> Option<usize> {
        let n = super::call(nfc, &[9], uid);
        // WHY the check: the length is the driver's word; only a UID length it can send (7 or 10) is one.
        matches!(n, 7 | 10).then_some(n as usize)
    }

    /// Every held `tag_wait` let go (answered with nothing). True when the driver answered.
    pub fn tag_let_go(nfc: u64) -> bool {
        let mut r = [0u8; 1];
        super::call_live(nfc, &[10], &mut r) == 1
    }

    /// The driver makes sure its chip still reads tags (op 11): discovery found stopped is started again, a chip that does not answer is brought up again from a reset pulse. A doorbell, rung at a person's touch, never on a timer. Answered before the check, so it costs the caller a call; true when the driver answered (a driver from before 2026-11-17 answers nothing).
    pub fn probe(nfc: u64) -> bool {
        let mut r = [0u8; 1];
        super::call_live(nfc, &[11], &mut r) == 1
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

/// The heap a growing image declares when nothing but the device bounds it (docs/NO-CEILINGS.md step 3; owner, 2026-11-23: "if I open notepad and it requests 1mb of memory and it's happy, then I go and open a video ... and it malloc's 4GB of RAM, it better work"): 2^47, the largest block of the arcs' 48-bit space, more than any device holds. The kernel takes the least of it and its pool (process.rs `image_tail_ceiling`), and every grow past the free frames is refused as it comes (ARC_WHY_MEMORY).
/// Not the largest word: the bridge and the kernel add the image's other words to it, and 2^47 leaves those sums far from wrapping.
pub const HEAP_DEVICE: usize = 1 << 47;

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
    // A payload whose heap grows on demand (docs/ARCS.md step 2; the display's since 2026-11-11, when its fixed 32 MiB heap region ran out at start): bit 0 of the pad word set (process.rs `image_grows`), and the heap's ceiling at offset 48. The kernel maps the first 4 MiB of that heap after .bss at spawn (`HEAP_START`) and sizes the arc's window for the whole ceiling; the rest is the process's to ask for (SYS_ARC, `arc::grow`), whole frames above its first stack.
    ($main:path, grows $ceiling:expr) => {
        core::arch::global_asm!(
            r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 1
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad {ceiling}
    .size _entry, . - _entry
"#,
            main = sym $main,
            ceiling = const $ceiling
        );
    };
    // A growing heap that starts smaller than the kernel's 4 MiB (docs/NO-CEILINGS.md step 2, the start-size word): bits 8 to 13 of the pad word the log2 of the heap the kernel maps at spawn (process.rs `image_start`), the rest asked for as it runs out. A kernel from before reads bit 0 alone and maps its 4 MiB.
    ($main:path, grows $ceiling:expr, starts $log2:expr) => {
        core::arch::global_asm!(
            r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte {flags}
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad {ceiling}
    .size _entry, . - _entry
"#,
            main = sym $main,
            flags = const 1 | (($log2 & 63) << 8),
            ceiling = const $ceiling
        );
    };
}

/// Whether a data link is up: cellular or WiFi in state 2, connected (facts 33-48, `measure::STATE`; links 0 cellular, 1 WiFi). For a screen that waits on the network to say so (ahau, owner 2026-11-09: "should say something like 'waiting for connectivity'").
pub fn data_link_up(f: &Facts) -> bool {
    (0..2).any(|k| f.0[33 + 8 * k..41 + 8 * k].iter().filter_map(|&m| measure_of(m)).any(|m| m.0 == measure::STATE && m.1 == 2))
}
