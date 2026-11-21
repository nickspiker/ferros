//! The flash log: the ledger's flash ring (LEDGER.md "The flash ring"), outside the vault, so it works when the vault is broken.
//!
//! Every line is a ledger entry (ferros_ledger): a complete VSF document with the boot, the Eagle time, the writer, a category and the words, sealed, and chained by BLAKE3 to the entry before.
//! The region is in the ferros partition from 1 GiB + 64 MiB in: two copies of 2^14 blocks (64 MiB each), a ring that wraps; the head is found by the chain, a torn last write is named and skipped (ferros_ledger::ring).
//! An entry is on the flash, in both copies and the first read back, before `line` returns; one said while the kernel lock is held, once that core lets the lock go (`deferred`).
//!
//! Sealing (LEDGER.md "Classes"): a system entry under a key derived from the ira (`system_key`, the one place it is made), a personal entry under the attested person's ledger key (`set_personal`, from kvault's unlock).
//! Nothing reaches the flash unsealed: what is said before the ira is held waits in the pre-boot buffer (and goes to the RAM log), and is written, sealed and in order, the moment the ira is up (`flush`, from vault boot).
//! A personal entry made while nobody is present is dropped and counted, never held: the next person to attest may be someone else, and an entry held in RAM would be sealed to them.
//!
//! The 2026-10-20 text log (1 GiB in, 2^14 blocks, a fixed-width header and plain lines) is never parsed here: `diag "log legacy"` hands its raw blocks to the bridge's one-shot converter, and `diag "log legacy erase"` zeroes it after.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_hal::ufs::UfsController;
use ferros_ledger::category::{self, Category, Class};
use ferros_ledger::death::Death;
use ferros_ledger::entry::{self, Payload, SealKey};
use ferros_ledger::preboot::{Pending, Preboot};
use ferros_ledger::ring::{Blocks, Finding, Item, Ring, Why, BLOCK};
use ferros_ledger::view;

const UFS_BASE: usize = 0x1320_0000;
/// The old text log's place in the ferros partition (1 GiB in) and its length in blocks: read raw for the converter, then erased.
const OLD_OFFSET: u32 = 1 << 18;
const OLD_BLOCKS: u32 = 1 << 14;
/// The ring's place, right after the old log, and the blocks in each of its two copies.
const OFFSET: u32 = OLD_OFFSET + OLD_BLOCKS;
const HALF: u32 = 1 << 14;
/// Entries the pre-boot buffer holds before it overwrites the oldest.
const PREBOOT: usize = 1 << 6;

/// The system class's seal key: derived from the ira under this context, here and nowhere else (LEDGER.md "Classes": readable by this device at any boot, unreadable off it, never the identity's keys).
/// The coordinator chose it pending the owner's word (2026-11-17): a change of root is a change to `system_key` alone.
const SYSTEM_SEAL_CONTEXT: &str = "ferros 2026-11-17 ledger system seal v0";

fn system_key() -> Option<SealKey> {
    let mut k = crate::ira::derive(SYSTEM_SEAL_CONTEXT)?;
    let s = SealKey::new(&k);
    crate::keys::wipe_to_dram(&mut k);
    Some(s)
}

/// The attested person's ledger key, held sealed under the wairua (keys.rs `Held`): set by kvault at unlock, cleared at lock.
static mut PERSONAL: Option<crate::keys::Held> = None;

/// The person's key, opened into memory for one seal or one answer (XChaCha20-Poly1305 does not run in registers yet; KEY_REGISTERS.md lists it with kvault's payload key).
fn personal_key() -> Option<SealKey> {
    let h = unsafe { (*(&raw const PERSONAL)).as_ref() }?;
    let mut k = crate::keys::held_open(h)?;
    let s = SealKey::new(&k);
    crate::keys::wipe_to_dram(&mut k);
    Some(s)
}

/// From kvault: the person's ledger key at unlock (Some), gone at lock (None).
pub fn set_personal(h: Option<crate::keys::Held>) {
    crate::process::LOG.with(|| {
        let p = unsafe { &mut *(&raw mut PERSONAL) };
        if let Some(old) = p.as_mut() {
            crate::keys::wipe_value_to_dram(old);
        }
        *p = h;
    })
}

/// The ring's region on the UFS: blocks numbered from its start (copy A at 0, copy B at HALF).
struct Ufs {
    lun: u8,
    base: u32,
}

/// Block operations on the ring since the watchdogs were last fed (`feed`).
static OPS: AtomicU64 = AtomicU64::new(0);

/// Feed the watchdogs every 2^6 of the ring's block operations: opening the ring reads blocks with nothing else feeding them, so it feeds them itself, far inside their one-second window (platform.rs WDT_COUNT). 2026-11-18: an unfed open over a region holding old data is one suspect in the converter build's boot that never came up on the Pro.
fn feed() {
    if OPS.fetch_add(1, Ordering::Relaxed) & ((1 << 6) - 1) == 0 {
        crate::platform::pet_watchdogs();
    }
}

impl Blocks for Ufs {
    // Each block a command holding the UFS lock (process.rs `UFS`), its copy in or out with it: the controller has one data block, which a vault read on another core shares.
    fn read(&mut self, block: u32, out: &mut [u8; BLOCK]) -> bool {
        feed();
        crate::process::UFS.with(|| {
            let u = UfsController::new(UFS_BASE);
            if u.read_block_lun(self.lun, self.base + block) != 0 || u.last_response_status() != 0 {
                return false;
            }
            out.copy_from_slice(u.data_buffer());
            true
        })
    }
    fn write(&mut self, block: u32, data: &[u8; BLOCK]) -> bool {
        feed();
        crate::process::UFS.with(|| {
            let u = UfsController::new(UFS_BASE);
            u.data_buffer_mut().copy_from_slice(data);
            u.write_block_lun(self.lun, self.base + block) == 0 && u.last_response_status() == 0
        })
    }
}

struct State {
    ring: Ring,
    dev: Ufs,
    /// The ferros partition's first block, for the old log.
    part: u32,
    boot: u64,
}
static mut STATE: Option<State> = None;
static mut PRE: Preboot<PREBOOT> = Preboot::new();
/// Counters for `diag log`'s state: personal entries dropped while nobody was present, pre-boot entries overwritten (all boot), writes the flash refused.
static PERSONAL_DROPPED: AtomicU64 = AtomicU64::new(0);
static PRE_LOST: AtomicU64 = AtomicU64::new(0);
static WRITE_FAILS: AtomicU64 = AtomicU64::new(0);

fn state() -> Option<&'static mut State> {
    unsafe { (*(&raw mut STATE)).as_mut() }
}

fn pre() -> &'static mut Preboot<PREBOOT> {
    unsafe { &mut *(&raw mut PRE) }
}

/// Eagle time at `tick`, None before the clock has been read this boot (the entry then carries no time: VSF's way for a device that cannot know it).
fn time_at(tick: u64) -> Option<i64> {
    match crate::settings::eagle_at(tick) {
        0 => None,
        t => Some(t),
    }
}

/// Open the ring in the ferros partition, once `ufs_device::locate` found it; no partition (or one too small), no ring, and every line stays in the RAM log.
/// Finds the head by the chain; what the open found wrong (a torn last write) is written as the ledger's own entry once the ira is up.
pub fn init() -> bool {
    let Some((first, blocks)) = crate::ufs_device::part() else { return false };
    if blocks < OFFSET + 2 * HALF {
        return false;
    }
    crate::process::LOG.with(|| {
        let mut dev = Ufs { lun: 0, base: first + OFFSET };
        let (ring, findings) = Ring::open(&mut dev, HALF);
        let boot = match ring.last_boot() {
            Some(b) => b + 1,
            None => old_boot(first).map_or(1, |b| b + 1),
        };
        let t = crate::platform::cntpct();
        for f in findings {
            pre().push(Pending { tick: t, category: category::LEDGER, writer: "kernel".into(), words: alloc::format!("at open, block A#{} byte A#{}: {}", f.block, f.at, f.why), death: None });
        }
        unsafe { STATE = Some(State { ring, dev, part: first, boot }) };
    });
    true
}

/// The boot count the old text log's header kept, read once to seed the ring's first boot so the numbers go on (b930, b931, ...): only while the ring holds no entry, never again after.
/// Its header was {"FLOG", u32 next block, u32 bytes used, u32 boot count}, little-endian: the one fixed-width read left, of a structure that is erased once converted.
fn old_boot(part: u32) -> Option<u64> {
    let mut b = [0u8; BLOCK];
    let mut dev = Ufs { lun: 0, base: part + OLD_OFFSET };
    if !dev.read(0, &mut b) || &b[..4] != b"FLOG" {
        return None;
    }
    Some(u32::from_le_bytes([b[12], b[13], b[14], b[15]]) as u64)
}

/// This boot's number (the `b<boot>` every entry carries): 0 before `init` or with no ring.
pub fn boot() -> u64 {
    unsafe { (*(&raw const STATE)).as_ref() }.map_or(0, |s| s.boot)
}

/// One line of the kernel's words, category `text`.
/// Under the flash log's own lock (process.rs `LOG`), each block a command under the UFS lock: the ring's blocks share the UFS with the vault, and a line written from the service loop while a syscall on another core read the vault broke that read (2026-10-24: init's spawn of touch failed at boot with "unsealed block").
/// Until 2026-11-18 under the kernel lock, so every core waited out each line (to 0.66 ms, `diag lock`); a line said holding the kernel lock (a syscall's) is written once its core lets the lock go (`DEFERRED`).
pub fn line(text: &str) {
    put(category::TEXT, "kernel", text, None)
}

/// A line in a process's words (its console text the process asked to keep), its name the writer.
pub fn line_from(writer: &str, text: &str) {
    put(category::TEXT, writer, text, None)
}

/// A line about the person (category `person.text`): sealed under their key, dropped while nobody is present.
pub fn personal(text: &str) {
    put(category::PERSON_TEXT, "kernel", text, None)
}

/// The lines that repeat on a retry loop, kept once and then counted (coordinator, 2026-11-20: b1062's USB retries, three lines every 4 s with nothing plugged in, pushed a crash's lead-up out of `diag log`'s window): per slot, the hash of the last line kept and how many times it came again since.
pub mod once {
    /// The USB link's tcpc state, the link readied, the USB process's end after its tries.
    pub const USB_TCPC: usize = 0;
    pub const USB_LINK: usize = 1;
    pub const USB_END: usize = 2;
    pub const SLOTS: usize = 1 << 2;
}
static ONCE_HASH: [AtomicU64; once::SLOTS] = [const { AtomicU64::new(0) }; once::SLOTS];
static ONCE_AGAIN: [AtomicU64; once::SLOTS] = [const { AtomicU64::new(0) }; once::SLOTS];

/// Whether a line in `slot` is to be kept: the first time, and whenever it differs from the last one kept (then the count of the last one's repeats goes first, as a line of its own). Counted and dropped otherwise.
fn once_keep(slot: usize, writer: &str, words: &str) -> bool {
    // FNV-1a over the writer, a zero, the words: a line told from the last one kept, no more (a collision only drops a line the count then names).
    let h = writer.bytes().chain(core::iter::once(0)).chain(words.bytes()).fold(0xCBF2_9CE4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01B3));
    if ONCE_HASH[slot].swap(h, Ordering::Relaxed) == h {
        ONCE_AGAIN[slot].fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let again = ONCE_AGAIN[slot].swap(0, Ordering::Relaxed);
    if again != 0 {
        put(category::TEXT, "kernel", &alloc::format!("the line before from {writer}'s retry said A#{again} more times"), None);
    }
    true
}

/// A kernel line that repeats on a retry loop: kept once, then counted (`once`).
pub fn line_once(slot: usize, text: &str) {
    if once_keep(slot, "kernel", text) {
        line(text)
    }
}

/// The repeats counted and not yet said, for the heartbeat's line: (slot, count).
pub fn once_pending() -> Vec<(usize, u64)> {
    (0..once::SLOTS).filter_map(|s| match ONCE_AGAIN[s].load(Ordering::Relaxed) { 0 => None, n => Some((s, n)) }).collect()
}

/// A process's end (category `kernel.process`, process.rs `death_line`), its writer the process: the sentence in the words, and the exit value, the last line, and a panic's or a fault's report (message, place, thread, frames) as typed fields (ferros_ledger death).
pub fn death(writer: &str, words: &str, d: Death) {
    put(category::PROCESS, writer, words, Some(d))
}

/// A process's end on a retry loop (the USB process giving up after its tries, started again by the kernel): kept once, then counted (`once`).
pub fn death_once(slot: usize, writer: &str, words: &str, d: Death) {
    if once_keep(slot, writer, words) {
        death(writer, words, d)
    }
}

/// The kernel's own end in the boot before, found in its record at this boot (category `kernel.panic`, service.rs).
pub fn kernel_death(words: &str, d: Death) {
    put(category::PANIC, "kernel", words, Some(d))
}

/// Per core: the lines said while it held the kernel lock (a syscall's, a process's end), with the counter at each, written once it lets the lock go (`deferred`, from process.rs `unlock`). Touched by its own core only, which holds the kernel lock while it adds one.
/// Why (the Pro, 2026-11-18, `diag lock`): each line is a UFS write and its read back, 0.7 to 1.3 ms, and written under the kernel lock every core waited it out: a process's end held the lock 1.0 to 1.56 ms, a driver's kept console line 1 ms. Now only the core that said it waits, and the line keeps the time it was said at.
static mut DEFERRED: [Vec<(Category, String, String, Option<Death>, u64)>; 16] = [const { Vec::new() }; 16];

fn put(c: Category, writer: &str, words: &str, death: Option<Death>) {
    let tick = crate::platform::cntpct();
    let me = crate::process::core();
    if crate::process::holder() == me + 1 {
        unsafe { (*(&raw mut DEFERRED))[me].push((c, writer.into(), words.into(), death, tick)) };
        return;
    }
    crate::process::LOG.with(|| put_locked(c, writer, words, death, tick))
}

/// The lines this core said under the kernel lock (`DEFERRED`), in the order said, now that it let the lock go (process.rs `unlock`, after the lock is free). Not while it holds the flash log's lock or a storage lock above it (a ring append under way, or the UFS): they wait for its next unlock.
pub fn deferred() {
    let me = crate::process::core();
    let q = unsafe { &mut (*(&raw mut DEFERRED))[me] };
    if q.is_empty() || crate::process::ranks_from(crate::process::LOG.rank()) {
        return;
    }
    let lines = core::mem::take(q);
    crate::process::LOG.with(|| {
        for (c, writer, words, death, tick) in lines {
            put_locked(c, &writer, &words, death, tick);
        }
    })
}

fn put_locked(c: Category, writer: &str, words: &str, death: Option<Death>, tick: u64) {
    match c.class {
        Class::Personal => {
            // Never the RAM log, never the pre-boot buffer: the person's words go sealed to the flash or nowhere.
            let (Some(s), Some(key)) = (state(), personal_key()) else {
                PERSONAL_DROPPED.fetch_add(1, Ordering::Relaxed);
                return;
            };
            write(s, &key, c, writer, words, death, time_at(tick));
        }
        Class::System => {
            // The RAM log keeps every system line as it always has when the flash cannot take it yet.
            // The key is derived only when there is a ring to write to.
            let live = state().and_then(|s| system_key().map(|k| (s, k)));
            let Some((s, key)) = live else {
                crate::platform::pstore(alloc::format!("flash log: {writer}: {words}\n").as_bytes());
                pre().push(Pending { tick, category: c, writer: writer.into(), words: words.into(), death });
                return;
            };
            drain(s, &key);
            write(s, &key, c, writer, words, death, time_at(tick));
        }
    }
}

/// The pre-boot buffer into the ring, oldest first, then the count of what it overwrote.
fn drain(s: &mut State, key: &SealKey) {
    if pre().is_empty() {
        return;
    }
    let lost = pre().drain(|p| write(s, key, p.category, &p.writer, &p.words, p.death, time_at(p.tick)));
    if lost > 0 {
        PRE_LOST.fetch_add(lost, Ordering::Relaxed);
        write(s, key, category::LEDGER, "kernel", &alloc::format!("the pre-boot buffer overwrote A#{lost} entries before the ira was up: they are in the RAM log only"), None, time_at(crate::platform::cntpct()));
    }
}

fn write(s: &mut State, key: &SealKey, c: Category, writer: &str, words: &str, death: Option<Death>, time: Option<i64>) {
    if let Err(e) = s.ring.append(&mut s.dev, s.boot, time, c.class, key, &Payload { death, ..Payload::new(c, writer, words) }) {
        WRITE_FAILS.fetch_add(1, Ordering::Relaxed);
        // A system line the flash refused goes to the RAM log; a personal one never does.
        if c.class == Class::System {
            crate::platform::pstore(alloc::format!("flash log: not written ({e:?}): {writer}: {words}\n").as_bytes());
        }
    }
}

/// From vault boot, once the ira is held: what waited in the pre-boot buffer goes to the flash now, sealed.
pub fn flush() {
    crate::process::LOG.with(|| {
        if let Some((s, key)) = state().and_then(|s| system_key().map(|k| (s, k))) {
            drain(s, &key);
        }
    })
}

/// `diag log [n]`: the newest `n` entries as the answer's VSF documents (ferros_ledger::view), at most `room` bytes, the ring's state first.
/// Each entry as stored, then opened when this device holds its key now (system: the ira; personal: the person present); a system entry that does not open is named.
/// Until signed bridge commands exist (BRIDGE.md B2) whoever holds the USB cable reads this opened: the seal covers a chip taken off the board, not the cable (LEDGER.md).
pub fn answer(n: usize, room: usize) -> Vec<u8> {
    crate::process::LOG.with(|| answer_locked(n, room))
}

fn answer_locked(n: usize, room: usize) -> Vec<u8> {
    let now = time_at(crate::platform::cntpct());
    let person = unsafe { (*(&raw const PERSONAL)).is_some() };
    let Some(s) = state() else {
        let st = view::State { boot: 0, next_seq: 0, block: 0, half: 0, preboot_held: pre().len() as u64, preboot_lost: PRE_LOST.load(Ordering::Relaxed), personal_dropped: PERSONAL_DROPPED.load(Ordering::Relaxed), write_fails: 0, person };
        return view::state(&st, now).unwrap_or_default();
    };
    let st = view::State {
        boot: s.boot,
        next_seq: s.ring.next_seq(),
        block: s.ring.head_block(),
        half: HALF,
        preboot_held: pre().len() as u64,
        preboot_lost: PRE_LOST.load(Ordering::Relaxed),
        personal_dropped: PERSONAL_DROPPED.load(Ordering::Relaxed),
        write_fails: WRITE_FAILS.load(Ordering::Relaxed),
        person,
    };
    let head = view::state(&st, now).unwrap_or_default();
    let (sys, per) = (system_key(), personal_key());
    // Each item's documents, newest kept first when the room runs out.
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    for item in s.ring.newest(&mut s.dev, n) {
        let mut c = Vec::new();
        match item {
            Item::Entry { block, at, doc, stored } => {
                c.extend_from_slice(&doc);
                let key = match Class::from_number(stored.class) {
                    Some(Class::System) => sys.as_ref(),
                    Some(Class::Personal) => per.as_ref(),
                    None => None,
                };
                if let Some(key) = key {
                    match entry::open(&stored, key) {
                        Ok(p) => c.extend_from_slice(&view::opened(&stored, &p).unwrap_or_default()),
                        // A personal entry of another person stays shut without a word; a system entry that does not open is this device's own and is named.
                        Err(_) if stored.class == Class::Personal.number() => {}
                        Err(e) => c.extend_from_slice(&view::finding(&Finding { block, at, why: Why::Shut(e) }, Some(stored.place.seq), now).unwrap_or_default()),
                    }
                }
            }
            Item::Finding(f) => c.extend_from_slice(&view::finding(&f, None, now).unwrap_or_default()),
        }
        chunks.push(c);
    }
    let mut total = head.len();
    let mut keep = 0usize;
    for c in chunks.iter().rev() {
        if total + c.len() > room {
            break;
        }
        total += c.len();
        keep += 1;
    }
    let mut out = head;
    for c in &chunks[chunks.len() - keep..] {
        out.extend_from_slice(c);
    }
    out
}

/// `diag "log deaths [n] [blocks]"`: the newest `n` deaths (entries of `kernel.process` or `kernel.panic` that carry a death's fields: ferros_ledger death) found in the newest `blocks` blocks of the ring, each as stored and opened, oldest first, the ring's state first, at most `room` bytes: what `ferros-bridge deaths` reads.
/// A search, not a read of the chain: `diag log` checks the chain. Every system entry it passes is opened to see its category (the category is sealed), about ten entries a block.
/// Under the flash log's lock for the whole walk, so the lines said meanwhile wait (`deferred`, or the caller): a diag, never the hot path.
/// `exits`: plain exits too; without it only panics, faults and the kernel's own ends.
pub fn deaths(n: usize, blocks: u32, room: usize, exits: bool) -> Vec<u8> {
    crate::process::LOG.with(|| deaths_locked(n, blocks, room, exits))
}

fn deaths_locked(n: usize, blocks: u32, room: usize, exits: bool) -> Vec<u8> {
    let now = time_at(crate::platform::cntpct());
    let (Some(s), Some(key)) = (state(), system_key()) else { return Vec::new() };
    let st = view::State {
        boot: s.boot,
        next_seq: s.ring.next_seq(),
        block: s.ring.head_block(),
        half: HALF,
        preboot_held: pre().len() as u64,
        preboot_lost: PRE_LOST.load(Ordering::Relaxed),
        personal_dropped: PERSONAL_DROPPED.load(Ordering::Relaxed),
        write_fails: WRITE_FAILS.load(Ordering::Relaxed),
        person: unsafe { (*(&raw const PERSONAL)).is_some() },
    };
    let mut out = view::state(&st, now).unwrap_or_default();
    // Newest first while walking; each death's two documents, put oldest first after.
    let mut found: Vec<Vec<u8>> = Vec::new();
    let mut total = out.len();
    s.ring.back(&mut s.dev, blocks, |doc, stored| {
        if stored.class != Class::System.number() {
            return true;
        }
        let Ok(p) = entry::open(stored, &key) else { return true };
        match &p.death {
            None => return true,
            Some(d) if d.end == ferros_ledger::death::END_EXIT && !exits => return true,
            Some(_) => {}
        }
        let mut c = doc.to_vec();
        c.extend_from_slice(&view::opened(stored, &p).unwrap_or_default());
        total += c.len();
        if total > room {
            return false;
        }
        found.push(c);
        found.len() < n
    });
    for c in found.iter().rev() {
        out.extend_from_slice(c);
    }
    out
}

/// The old log's blocks a host asks for, cut to the old log.
/// WHY the cut: `first` and `count` come from the host (diag); a range past the old log would read or erase the ring or whatever follows it.
/// PROOF of no overflow: `first` is below OLD_BLOCKS (2^14) when the range is not empty and the diag site holds `count` to at most 2^6, so `first + count` stays far below 2^32.
pub const LEGACY_STEP: u32 = 1 << 6;
fn legacy_range(first: u32, count: u32) -> core::ops::Range<u32> {
    if first >= OLD_BLOCKS {
        return 0..0;
    }
    first..(first + count).min(OLD_BLOCKS)
}

/// `diag "log legacy <first> <count>"`: blocks of the old text log as they are on the flash, for the bridge's converter (never parsed here).
pub fn legacy_blocks(first: u32, count: u32) -> Vec<u8> {
    crate::process::LOG.with(|| {
        let Some(s) = state() else { return Vec::new() };
        let mut dev = Ufs { lun: 0, base: s.part + OLD_OFFSET };
        let mut out = Vec::new();
        let mut b = [0u8; BLOCK];
        for k in legacy_range(first, count) {
            if !dev.read(k, &mut b) {
                break;
            }
            out.extend_from_slice(&b);
        }
        out
    })
}

/// `diag "log legacy erase <first> <count>"`: zero blocks of the old text log, each read back.
/// Returns how many were zeroed and verified; the bridge goes on in steps so the flash log's lock is never held for long.
pub fn legacy_erase(first: u32, count: u32) -> u32 {
    let mut done = 0u32;
    for k in legacy_range(first, count) {
        let ok = crate::process::LOG.with(|| {
            let Some(s) = state() else { return false };
            let mut dev = Ufs { lun: 0, base: s.part + OLD_OFFSET };
            let zero = [0u8; BLOCK];
            let mut back = [1u8; BLOCK];
            dev.write(k, &zero) && dev.read(k, &mut back) && back == zero
        });
        if !ok {
            break;
        }
        done += 1;
    }
    done
}

/// `diag "log bench"`: what an entry costs on this device against the old line, in microseconds a piece (A#).
/// The old line's work (format and copy) and its two block writes; the entry's key, seal and document, and its write (A, read back, B) as 2^3 real `bench` entries.
pub fn bench() -> String {
    crate::process::LOG.with(bench_locked)
}

fn bench_locked() -> String {
    let f = crate::platform::cntfrq();
    let us = |t: u64, n: u64| t * 1_000_000 / f / n;
    let now = crate::platform::cntpct;
    const N: u64 = 1 << 6;
    const W: u64 = 1 << 3;
    let words = "camera: on in A#412 ms, module A#2, S2MPUs G#1F (1 fenced)";
    let mut tail = [0u8; BLOCK];
    let t = now();
    for k in 0..N {
        let line = alloc::format!("b{} t{} {}\n", 931, k * 7, words);
        tail[..line.len()].copy_from_slice(line.as_bytes());
        core::hint::black_box(&tail);
    }
    let old = us(now() - t, N);
    let Some(s) = state() else { return String::from("bench: no ring\n") };
    let t = now();
    let mut keys = Vec::new();
    for _ in 0..N {
        keys.push(system_key());
    }
    let key_us = us(now() - t, N);
    let Some(Some(key)) = keys.pop() else { return String::from("bench: no ira, nothing to seal with\n") };
    drop(keys);
    let place = entry::Place { seq: s.ring.next_seq(), boot: s.boot, prev: s.ring.head_hash(), time: time_at(now()) };
    let p = Payload::new(category::BENCH, "kernel", words);
    let t = now();
    let mut len = 0usize;
    for _ in 0..N {
        len = entry::build(&place, Class::System, &key, &p).map(|d| d.len()).unwrap_or(0);
    }
    let build = us(now() - t, N);
    // The old line's I/O: two block writes (its tail block and its header), here the head block's primary copy written twice with what it already holds.
    let mut blk = [0u8; BLOCK];
    let head = s.ring.head_block();
    let t = now();
    let mut io_ok = s.dev.read(head, &mut blk);
    let read_us = us(now() - t, 1);
    let t = now();
    for _ in 0..W {
        io_ok &= s.dev.write(head, &blk) && s.dev.write(head, &blk);
    }
    let old_io = us(now() - t, W);
    let t = now();
    for _ in 0..W {
        if s.ring.append(&mut s.dev, s.boot, time_at(now()), Class::System, &key, &p).is_err() {
            io_ok = false;
        }
    }
    let append = us(now() - t, W);
    alloc::format!(
        "old line: A#{old} us to format and copy, A#{old_io} us its two block writes\nentry: A#{key_us} us the seal key from the ira, A#{build} us to seal and build (A#{} bytes), A#{append} us an append with the key in hand (seal, build, write A, read back, write B; A#{W} bench entries written)\none block read: A#{read_us} us{}\n",
        len,
        if io_ok { "" } else { "; a write or read FAILED" }
    )
}

/// The ring's region in the ferros partition (first block, blocks: both copies), for the owner's fresh start (service.rs `vault fresh`), which zeroes it: a zeroed region is a ring never written.
pub fn ring_region() -> (u64, u64) {
    (OFFSET as u64, 2 * HALF as u64)
}
