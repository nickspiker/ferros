//! The ira and the wairua, held in CPU registers instead of DRAM (KEY_REGISTERS.md, "Off DRAM").
//!
//! The code that touches them is keyreg/keys.rs, written in safe Rust and lifted by tools/keyreg-lift.sh to keyreg/keys.s, which is what links here: the lift refuses any output that uses the stack or calls out, so no secret is ever spilled. Each secret lives as two shares (pointer-authentication keys XOR dead EL1 registers), and every use joins them inside that code and gives back only what was asked for: a derived key, a stream block, a sealed blob. Calls run with interrupts masked and the caller-saved registers scrubbed before they unmask, so no trap frame catches a leftover.
//!
//! Every core has its own registers. A secret set on one core reaches the others sealed: XOR a keystream from a key the fuses give (re-derivable in microseconds by any core, no disk), with a MAC, in one blob that lives only until every core it was meant for has taken it. The same blob carries the secrets through a core's power-down or deep sleep (the wairua stays the same: owner, 2026-10-28) and from CPU0 to the X3 at the hop. What the seal guards against: anything that reads DRAM but cannot run code on this phone. What it does not: someone who resets the phone into their own code while a blob is in DRAM, which lasts only as long as a handoff or a sleep (no always-on storage that dies on reset exists, KEY_REGISTERS.md).

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// raw: the table lookups write their register lists in braces, which a template would read as placeholders.
core::arch::global_asm!(include_str!("../../keyreg/keys.s"), options(raw));

#[repr(C, align(16))]
pub struct Out4(pub [u64; 4]);
#[repr(C, align(16))]
pub struct Out8(pub [u64; 8]);

unsafe extern "C" {
    fn keyreg_derive(slot: u32, context_key: &[u64; 4], out: &mut Out4);
    fn keyreg_stream(context_key: &[u64; 4], msg: &[u64; 8], len: u32, block: u64, out: &mut Out8);
    fn keyreg_set(slot: u32, mask: &[u64; 4], a: &[u64; 4], b: &[u64; 4]);
    fn keyreg_seal(keystream: &[u64; 8], out: &mut Out8);
    fn keyreg_remask(slot: u32, mask: &[u64; 4]);
    fn keyreg_clear();
    fn keyreg_scrub();
}

pub const IRA: u32 = 0;
pub const WAIRUA: u32 = 1;

/// Run one key function: interrupts masked from before it to after the scrub of what it left in the caller-saved registers.
fn sealed<R>(f: impl FnOnce() -> R) -> R {
    let daif: u64;
    unsafe { core::arch::asm!("mrs {}, daif", "msr daifset, #0xF", out(reg) daif, options(nomem, nostack)) };
    let r = f();
    unsafe {
        keyreg_scrub();
        core::arch::asm!("msr daif, {}", in(reg) daif, options(nomem, nostack));
    }
    r
}

fn words(b: &[u8; 32]) -> [u64; 4] {
    let mut w = [0u64; 4];
    for (k, c) in b.chunks_exact(8).enumerate() {
        w[k] = u64::from_le_bytes(c.try_into().unwrap());
    }
    w
}

/// Words to bytes without the heap (the hot paths: derive and stream).
fn le<const N: usize, const B: usize>(w: &[u64; N]) -> [u8; B] {
    let mut b = [0u8; B];
    for (k, x) in w.iter().enumerate() {
        b[k * 8..k * 8 + 8].copy_from_slice(&x.to_le_bytes());
    }
    b
}

fn bytes(w: &[u64]) -> alloc::vec::Vec<u8> {
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Overwrite, in a way the compiler keeps.
pub fn wipe(b: &mut [u8]) {
    for x in b.iter_mut() {
        unsafe { core::ptr::write_volatile(x, 0) };
    }
}
fn wipe_words(w: &mut [u64]) {
    for x in w.iter_mut() {
        unsafe { core::ptr::write_volatile(x, 0) };
    }
}
/// A whole value overwritten (a blake3 hasher holds its key in its state).
fn wipe_value<T>(v: &mut T) {
    let p = v as *mut T as *mut u8;
    for k in 0..core::mem::size_of::<T>() {
        unsafe { core::ptr::write_volatile(p.add(k), 0) };
    }
}

/// The dead stack below this frame zeroed (4 KiB, interrupts masked; a worker's stack is 128 KiB and this runs shallow in it): blake3's own frames held the seal key while it hashed, and a blob in DRAM beside a stale copy of its key would be no seal at all.
fn scrub_stack() {
    unsafe {
        core::arch::asm!(
            "mrs {d}, daif",
            "msr daifset, #0xF",
            "mov {p}, sp",
            "sub {e}, {p}, #4096",
            "2:",
            "stp xzr, xzr, [{e}], #16",
            "cmp {e}, {p}",
            "b.lo 2b",
            "msr daif, {d}",
            d = out(reg) _,
            p = out(reg) _,
            e = out(reg) _,
            options(nostack),
        );
    }
}

/// The cores that run ferros now: this one and the workers that are up. Before the hop (CPU0, no workers yet) the X3 and every worker, since they will.
fn running() -> u32 {
    let up = crate::smp::UP.load(Ordering::Acquire);
    if me() == 0 && up == 0 && option_env!("FERROS_X3") != Some("0") { AFTER_HOP } else { (1 << me()) | up << 4 }
}

/// This core's index: MPIDR Aff1 (CPU0 0, the A715 workers 4-7, the X3 8).
fn me() -> usize {
    let m: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) m, options(nomem, nostack)) };
    ((m >> 8) & 0xF) as usize
}
const CORES: usize = 9;
/// The cores that run ferros after the hop: the X3 and the four workers.
pub const AFTER_HOP: u32 = (1 << 8) | (0xF << 4);

/// Bumped whenever a secret is set; HELD[core] is the generation that core's registers hold (0 = nothing). PRESENT: bit 0 ira, bit 1 wairua.
static GEN: AtomicU32 = AtomicU32::new(0);
static HELD: [AtomicU32; CORES] = [const { AtomicU32::new(0) }; CORES];
static PRESENT: AtomicU32 = AtomicU32::new(0);
/// Self-test (first set of the boot): 0 not run, 1 passed, else the failing check's number + 1.
static TEST: AtomicU32 = AtomicU32::new(0);
/// Diagnostics: masks drawn without the TRNG, blobs that failed their MAC.
pub static WEAK_MASKS: AtomicU32 = AtomicU32::new(0);
pub static BAD_BLOBS: AtomicU32 = AtomicU32::new(0);

/// The check (KEY_REGISTERS.md §2, across cores): per secret, a one-way tag, the first 64 bits of derive_key(CHECK, secret), kept in RAM like the wairua's fingerprint (it reveals nothing about the secret). Every core checks its copy against the tags about every quarter second and before every derive (about 400 cycles a secret on the X3). A core whose copy disagrees asks the others: if the cores that checked lately agree with it, the tag is what flipped and is replaced; otherwise its copy is cleared and it takes a fresh sealed one from a core that passes. Five register files flipping alike is not a failure mode; a bit flipped in one core's registers is caught and repaired. A random flip escapes the check with odds 2^-64.
const CHECK: &str = "ferros keyreg check v0";
static TAG: [core::sync::atomic::AtomicU64; 2] = [const { core::sync::atomic::AtomicU64::new(0) }; 2];
static CORE_TAG: [[core::sync::atomic::AtomicU64; 2]; CORES] = [const { [const { core::sync::atomic::AtomicU64::new(0) }; 2] }; CORES];
static LAST_CHECK: [core::sync::atomic::AtomicU64; CORES] = [const { core::sync::atomic::AtomicU64::new(0) }; CORES];
pub static CHECK_FAILS: AtomicU32 = AtomicU32::new(0);
pub static TAG_FIXES: AtomicU32 = AtomicU32::new(0);

/// This core's tag for the secret in `slot`, from its registers as they are.
fn tag_here(slot: u32) -> u64 {
    let ck = context_key(CHECK);
    let mut o = Out4([0; 4]);
    sealed(|| unsafe { keyreg_derive(slot, &ck, &mut o) });
    let t = o.0[0];
    wipe_words(&mut o.0);
    t
}

/// Check this core's copies of the secrets in `which` (bit 0 ira, bit 1 wairua) against the tags. False = this core's copy was bad: it is cleared and the core has asked for a fresh one. Caller holds LOCK and this core holds the current generation.
fn check_here(which: u32) -> bool {
    let k = me();
    let now = crate::platform::cntpct();
    let fresh = crate::platform::cntfrq();
    let g = GEN.load(Ordering::Acquire);
    let present = PRESENT.load(Ordering::Acquire) & which;
    let mut ok = true;
    for slot in 0..2u32 {
        if present & (1 << slot) == 0 {
            continue;
        }
        let t = tag_here(slot);
        CORE_TAG[k][slot as usize].store(t, Ordering::Release);
        let stored = TAG[slot as usize].load(Ordering::Acquire);
        if t == stored {
            continue;
        }
        // Disagreement: the cores that hold this generation and checked within the last second decide.
        let (mut with_me, mut with_tag) = (0, 0);
        for j in 0..CORES {
            if j == k || HELD[j].load(Ordering::Acquire) != g || now.wrapping_sub(LAST_CHECK[j].load(Ordering::Acquire)) > fresh {
                continue;
            }
            let o = CORE_TAG[j][slot as usize].load(Ordering::Acquire);
            if o == t {
                with_me += 1;
            } else if o == stored {
                with_tag += 1;
            }
        }
        if with_me > with_tag {
            TAG[slot as usize].store(t, Ordering::Release);
            TAG_FIXES.fetch_add(1, Ordering::Relaxed);
        } else {
            ok = false;
        }
    }
    LAST_CHECK[k].store(now, Ordering::Release);
    if !ok {
        // Fail closed: this core's copy never derives anything again; another core's replaces it.
        CHECK_FAILS.fetch_add(1, Ordering::Relaxed);
        sealed(|| unsafe { keyreg_clear() });
        HELD[k].store(0, Ordering::Release);
        ASKING.fetch_or(1 << k, Ordering::AcqRel);
        crate::process::kick(0xFFFF);
    }
    ok
}

struct Blob {
    nonce: [u8; 32],
    ct: Out8,
    mac: [u8; 32],
    generation: u32,
    present: u32,
}
struct BlobCell(UnsafeCell<Blob>);
// SAFETY: every access is under LOCK.
unsafe impl Sync for BlobCell {}
static BLOB: BlobCell = BlobCell(UnsafeCell::new(Blob { nonce: [0; 32], ct: Out8([0; 8]), mac: [0; 32], generation: 0, present: 0 }));
static LOCK: AtomicBool = AtomicBool::new(false);
/// Cores the live blob is still waiting for (0 = no blob).
static WANT: AtomicU32 = AtomicU32::new(0);
/// Cores that came up after the blob was gone and need it again.
static ASKING: AtomicU32 = AtomicU32::new(0);

fn lock() {
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
}
fn unlock() {
    LOCK.store(false, Ordering::Release);
}

/// The public context key for a derive_key context string, from a small cache: hashing the context cost about 1,100 of a derive's 1,500 cycles on the X3 (2026-10-28). Sixteen entries searched in order and replaced round-robin; a context longer than an entry holds (none today: SYS_IRA's are at most 86 bytes) is hashed every time.
const CTX_LEN: usize = 96;
struct CtxCache {
    text: [[u8; CTX_LEN]; 16],
    len: [usize; 16],
    key: [[u64; 4]; 16],
    next: usize,
}
struct CtxCell(UnsafeCell<CtxCache>);
// SAFETY: every access is under CTX_LOCK.
unsafe impl Sync for CtxCell {}
static CTX: CtxCell = CtxCell(UnsafeCell::new(CtxCache { text: [[0; CTX_LEN]; 16], len: [usize::MAX; 16], key: [[0; 4]; 16], next: 0 }));
static CTX_LOCK: AtomicBool = AtomicBool::new(false);

fn context_key(context: &str) -> [u64; 4] {
    let c = context.as_bytes();
    if c.len() > CTX_LEN {
        return words(&blake3::hazmat::hash_derive_key_context(context));
    }
    while CTX_LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    let cache = unsafe { &mut *CTX.0.get() };
    let hit = (0..16).find(|&k| cache.len[k] == c.len() && cache.text[k][..c.len()] == *c);
    let key = match hit {
        Some(k) => cache.key[k],
        None => {
            let key = words(&blake3::hazmat::hash_derive_key_context(context));
            let k = cache.next;
            cache.next = (k + 1) % 16;
            cache.text[k][..c.len()].copy_from_slice(c);
            cache.len[k] = c.len();
            cache.key[k] = key;
            key
        }
    };
    CTX_LOCK.store(false, Ordering::Release);
    key
}

/// Fresh random words. With the wairua held: its keyed stream over a counter and the clock (microseconds). Before that: the firmware TRNG (about 1.3 ms a call on husky, 2026-10-28), else (counted in WEAK_MASKS) the counter. Masks and nonces only have to be unknown to someone who sees one share or the blob.
static RANDOM_N: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
fn random(out: &mut [u64]) {
    if held() & 2 != 0 {
        let mut i = 0;
        while i < out.len() {
            let n = RANDOM_N.fetch_add(1, Ordering::Relaxed);
            let mut b = stream_block(&context_key("ferros keyreg random v0"), &[n, crate::platform::cntpct(), me() as u64, 0, 0, 0, 0, 0], 24, 0);
            for &w in b.0.iter() {
                if i < out.len() {
                    out[i] = w;
                    i += 1;
                }
            }
            wipe_words(&mut b.0);
        }
        return;
    }
    if crate::kohi::trng_words(out) {
        return;
    }
    WEAK_MASKS.fetch_add(1, Ordering::Relaxed);
    for (k, x) in out.iter_mut().enumerate() {
        *x = crate::platform::cntpct().rotate_left(k as u32 * 7) ^ (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// Which secrets this core holds now (bit 0 ira, bit 1 wairua): all present ones, if its registers are at the current generation.
fn held() -> u32 {
    let g = GEN.load(Ordering::Acquire);
    if g != 0 && HELD[me()].load(Ordering::Acquire) == g { PRESENT.load(Ordering::Acquire) } else { 0 }
}

/// The seal: a key from the fuses (chip ID, trim, ASV tables, speed bin: the ira's hardware sources less the disk ones, so any core re-derives it in microseconds), expanded under the blob's nonce into the 64-byte keystream and a MAC key.
fn seal_keys(nonce: &[u8; 32]) -> ([u64; 8], [u8; 32], [u64; 8]) {
    let mut h = blake3::Hasher::new_derive_key("ferros keyreg seal v0");
    h.update(&crate::ira::read_hw_identity());
    h.update(&crate::ira::read_ap_hw_tune());
    h.update(&crate::ira::read_asv_tbl());
    h.update(&crate::ira::read_hpm_asv());
    h.update(&[crate::ira::read_dvfs_version()]);
    let mut key = *h.finalize().as_bytes();
    wipe_value(&mut h);
    let mut x = blake3::Hasher::new_keyed(&key);
    x.update(nonce);
    let mut out = [0u8; 160];
    let mut xof = x.finalize_xof();
    xof.fill(&mut out);
    wipe_value(&mut xof);
    wipe_value(&mut x);
    let mut ks = [0u64; 8];
    for (k, c) in out[..64].chunks_exact(8).enumerate() {
        ks[k] = u64::from_le_bytes(c.try_into().unwrap());
    }
    let mut mac_key = [0u8; 32];
    mac_key.copy_from_slice(&out[64..96]);
    let mut masks = [0u64; 8];
    for (k, c) in out[96..].chunks_exact(8).enumerate() {
        masks[k] = u64::from_le_bytes(c.try_into().unwrap());
    }
    wipe(&mut key);
    wipe(&mut out);
    (ks, mac_key, masks)
}

fn mac(mac_key: &[u8; 32], nonce: &[u8; 32], ct: &Out8, generation: u32, present: u32) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(mac_key);
    h.update(nonce);
    h.update(&bytes(&ct.0));
    h.update(&generation.to_le_bytes());
    h.update(&present.to_le_bytes());
    let r = *h.finalize().as_bytes();
    wipe_value(&mut h);
    r
}

/// Seal what this core holds into the blob for the cores in `want`. Caller holds LOCK and the current generation.
fn publish(want: u32) {
    let b = unsafe { &mut *BLOB.0.get() };
    let mut n = [0u64; 4];
    random(&mut n);
    b.nonce.copy_from_slice(&bytes(&n));
    let (mut ks, mut mk, mut unused) = seal_keys(&b.nonce);
    wipe_words(&mut unused);
    sealed(|| unsafe { keyreg_seal(&ks, &mut b.ct) });
    b.generation = GEN.load(Ordering::Acquire);
    b.present = PRESENT.load(Ordering::Acquire);
    b.mac = mac(&mk, &b.nonce, &b.ct, b.generation, b.present);
    wipe_words(&mut ks);
    wipe(&mut mk);
    scrub_stack();
    WANT.store(want, Ordering::Release);
    if WANT.load(Ordering::Acquire) == 0 {
        drop_blob();
    }
}

/// The blob gone: every core it was for has it.
fn drop_blob() {
    let b = unsafe { &mut *BLOB.0.get() };
    wipe_words(&mut b.ct.0);
    wipe(&mut b.nonce);
    wipe(&mut b.mac);
    b.generation = 0;
    WANT.store(0, Ordering::Release);
}

/// Take the live blob into this core's registers, if it is meant for this core. Caller holds LOCK.
fn adopt() -> bool {
    let bit = 1 << me();
    if WANT.load(Ordering::Acquire) & bit == 0 {
        return false;
    }
    let b = unsafe { &*BLOB.0.get() };
    let (mut ks, mut mk, mut m) = seal_keys(&b.nonce);
    let ok = mac(&mk, &b.nonce, &b.ct, b.generation, b.present) == b.mac;
    wipe(&mut mk);
    scrub_stack();
    if ok {
        // First split from the seal's own masks (no TRNG on the wake path), then a fresh one from the wairua's stream, which the blob cannot predict.
        sealed(|| unsafe {
            keyreg_set(IRA, &[m[0], m[1], m[2], m[3]], &[b.ct.0[0], b.ct.0[1], b.ct.0[2], b.ct.0[3]], &[ks[0], ks[1], ks[2], ks[3]]);
            keyreg_set(WAIRUA, &[m[4], m[5], m[6], m[7]], &[b.ct.0[4], b.ct.0[5], b.ct.0[6], b.ct.0[7]], &[ks[4], ks[5], ks[6], ks[7]]);
        });
        HELD[me()].store(b.generation, Ordering::Release);
        random(&mut m);
        sealed(|| unsafe {
            keyreg_remask(IRA, &[m[0], m[1], m[2], m[3]]);
            keyreg_remask(WAIRUA, &[m[4], m[5], m[6], m[7]]);
        });
        // A blob sealed from a bad copy would verify (its MAC covers the sealing, not the secret): the tags catch that too.
        check_here(3);
    } else {
        // Fail closed: a blob that does not verify is never loaded (a flipped bit would be a wrong key, silently).
        BAD_BLOBS.fetch_add(1, Ordering::Relaxed);
    }
    wipe_words(&mut ks);
    wipe_words(&mut m);
    if WANT.fetch_and(!bit, Ordering::AcqRel) & !bit == 0 {
        drop_blob();
    }
    ok
}

/// Make sure this core holds the current secrets, from the live blob if it has to. False when it cannot (no blob for it: it asks a holder, see `serve`).
fn ensure() -> bool {
    let g = GEN.load(Ordering::Acquire);
    if g == 0 || HELD[me()].load(Ordering::Acquire) == g {
        return true;
    }
    lock();
    let ok = adopt() || HELD[me()].load(Ordering::Acquire) == GEN.load(Ordering::Acquire);
    if !ok {
        ASKING.fetch_or(1 << me(), Ordering::AcqRel);
    }
    unlock();
    ok
}

/// From each core's loop (the X3's service loop, each worker's idle loop): take a blob meant for this core, or seal one for cores that asked.
pub fn serve() {
    // The quarter-second check of this core's copies.
    if held() != 0 && crate::platform::cntpct().wrapping_sub(LAST_CHECK[me()].load(Ordering::Acquire)) > crate::platform::cntfrq() / 4 {
        lock();
        if held() != 0 {
            check_here(3);
        }
        unlock();
    }
    let want = WANT.load(Ordering::Acquire);
    let asking = ASKING.load(Ordering::Acquire);
    if want & (1 << me()) == 0 && asking == 0 {
        return;
    }
    lock();
    if WANT.load(Ordering::Acquire) & (1 << me()) != 0 {
        adopt();
    }
    // Seal for cores that asked, only from a copy that passes its check now.
    let asking = ASKING.load(Ordering::Acquire);
    if asking != 0 && WANT.load(Ordering::Acquire) == 0 && held() != 0 && check_here(3) {
        ASKING.store(0, Ordering::Release);
        publish(asking);
    }
    unlock();
}

/// Hold `value` as this boot's ira (slot 0) or wairua (slot 1), on this core and, sealed, on every other; `value` is wiped. False if the self-test failed (the secret is then not held anywhere: callers refuse, as for no secret at all).
pub fn set(slot: u32, value: &mut [u8; 32]) -> bool {
    lock();
    if TEST.load(Ordering::Acquire) == 0 {
        TEST.store(self_test(), Ordering::Release);
    }
    if TEST.load(Ordering::Acquire) != 1 {
        wipe(value);
        unlock();
        return false;
    }
    // This core must hold the other secret before the new generation, or the blob would carry a stale one.
    let _ = adopt();
    let mut m = [0u64; 4];
    random(&mut m);
    let mut v = words(value);
    sealed(|| unsafe { keyreg_set(slot, &m, &v, &[0; 4]) });
    wipe_words(&mut v);
    wipe_words(&mut m);
    wipe(value);
    PRESENT.fetch_or(1 << slot, Ordering::AcqRel);
    TAG[slot as usize].store(tag_here(slot), Ordering::Release);
    let g = GEN.fetch_add(1, Ordering::AcqRel) + 1;
    HELD[me()].store(g, Ordering::Release);
    publish(running() & !(1 << me()));
    unlock();
    true
}

/// `blake3::derive_key(context, secret)` for the secret in `slot`. None if it is not held.
pub fn derive(slot: u32, context: &str) -> Option<[u8; 32]> {
    if !ensure() || held() & (1 << slot) == 0 {
        return None;
    }
    // Checked before every derive (they are rare: process ira keys, the capability key, the nonce seed, the fingerprint); a bad copy gives nothing, never a wrong key.
    lock();
    let ok = held() & (1 << slot) != 0 && check_here(1 << slot);
    unlock();
    if !ok {
        // Repaired from another core in microseconds when one is awake; a sixty-fourth of a second at most, then nothing.
        let until = crate::platform::cntpct() + crate::platform::cntfrq() / 64;
        while held() & (1 << slot) == 0 && crate::platform::cntpct() < until {
            serve();
            core::hint::spin_loop();
        }
        if held() & (1 << slot) == 0 {
            return None;
        }
    }
    let ck = context_key(context);
    let mut out = Out4([0; 4]);
    sealed(|| unsafe { keyreg_derive(slot, &ck, &mut out) });
    let k: [u8; 32] = le(&out.0);
    wipe_words(&mut out.0);
    Some(k)
}

fn stream_block(ck: &[u64; 4], msg: &[u64; 8], len: u32, block: u64) -> Out8 {
    let mut out = Out8([0; 8]);
    sealed(|| unsafe { keyreg_stream(ck, msg, len, block, &mut out) });
    out
}

/// `blake3::Hasher::new_keyed(&derive_key(context, wairua))` over `msg` (≤ 64 bytes), its XOF into `out`. False if the wairua is not held.
pub fn stream(context: &str, msg: &[u8], out: &mut [u8]) -> bool {
    if !ensure() || held() & 2 == 0 {
        return false;
    }
    let ck = context_key(context);
    let mut m = [0u8; 64];
    m[..msg.len()].copy_from_slice(msg);
    let mut mw = [0u64; 8];
    for (k, c) in m.chunks_exact(8).enumerate() {
        mw[k] = u64::from_le_bytes(c.try_into().unwrap());
    }
    for (block, chunk) in out.chunks_mut(64).enumerate() {
        let b = stream_block(&ck, &mw, msg.len() as u32, block as u64);
        let bb: [u8; 64] = le(&b.0);
        chunk.copy_from_slice(&bb[..chunk.len()]);
    }
    true
}

/// Before this core stops running ferros (CPU0 at the hop; a core about to power down): leave the secrets sealed for the cores that will, and clear this core's registers.
pub fn leave(want: u32) {
    lock();
    if held() != 0 && WANT.load(Ordering::Acquire) == 0 {
        publish(want);
    }
    sealed(|| unsafe { keyreg_clear() });
    HELD[me()].store(0, Ordering::Release);
    unlock();
}

/// The cores the blob still waits for, for the X3 to give up on ones that never came up (a worker whose CPU_ON failed would keep the blob in DRAM for good).
pub fn forget_missing() {
    lock();
    let w = WANT.load(Ordering::Acquire) & running();
    WANT.store(w, Ordering::Release);
    if w == 0 {
        drop_blob();
    }
    unlock();
}

/// Known-answer test of the lifted code against the blake3 crate, with test values in the registers (never a real secret: it runs before the first one is set), then a seal and unseal of them. 1 = passed.
fn self_test() -> u32 {
    let tv = |s: u8| -> [u8; 32] { core::array::from_fn(|k| s.wrapping_mul(31).wrapping_add(k as u8 * 7)) };
    let (ti, tw) = (tv(1), tv(2));
    let mut m = [0u64; 8];
    random(&mut m);
    sealed(|| unsafe {
        keyreg_set(IRA, &[m[0], m[1], m[2], m[3]], &words(&ti), &[0; 4]);
        keyreg_set(WAIRUA, &[m[4], m[5], m[6], m[7]], &words(&tw), &[0; 4]);
    });
    let ctx = "ferros keyreg self-test v0";
    let ck = context_key(ctx);
    let mut fail = 0;
    for (slot, v) in [(IRA, &ti), (WAIRUA, &tw)] {
        let mut out = Out4([0; 4]);
        sealed(|| unsafe { keyreg_derive(slot, &ck, &mut out) });
        if bytes(&out.0) != blake3::derive_key(ctx, v) {
            fail = 2 + slot;
        }
    }
    let n = 0x0123_4567_89AB_CDEFu64;
    let mut want = [0u8; 128];
    let mut h = blake3::Hasher::new_keyed(&blake3::derive_key(ctx, &tw));
    h.update(&n.to_le_bytes());
    h.finalize_xof().fill(&mut want);
    for block in 0..2u64 {
        let b = stream_block(&ck, &[n, 0, 0, 0, 0, 0, 0, 0], 8, block);
        if bytes(&b.0) != want[block as usize * 64..][..64] {
            fail = 4 + block as u32;
        }
    }
    // Seal, clear, unseal with fresh masks: the derive must still match.
    let nonce = [7u8; 32];
    let (ks, _, _) = seal_keys(&nonce);
    let mut ct = Out8([0; 8]);
    sealed(|| unsafe { keyreg_seal(&ks, &mut ct) });
    sealed(|| unsafe { keyreg_clear() });
    random(&mut m);
    sealed(|| unsafe {
        keyreg_set(IRA, &[m[0], m[1], m[2], m[3]], &[ct.0[0], ct.0[1], ct.0[2], ct.0[3]], &[ks[0], ks[1], ks[2], ks[3]]);
        keyreg_set(WAIRUA, &[m[4], m[5], m[6], m[7]], &[ct.0[4], ct.0[5], ct.0[6], ct.0[7]], &[ks[4], ks[5], ks[6], ks[7]]);
    });
    let mut out = Out4([0; 4]);
    sealed(|| unsafe { keyreg_derive(WAIRUA, &ck, &mut out) });
    if bytes(&out.0) != blake3::derive_key(ctx, &tw) {
        fail = 6;
    }
    sealed(|| unsafe { keyreg_clear() });
    if fail == 0 { 1 } else { fail }
}

/// `diag keys`: what this core and the others hold, the self-test, and (`diag keys sleep`) a power-down rehearsal on this core: seal, clear the registers as power-off would, take the blob back, and compare the wairua's fingerprint before and after.
pub fn diag(params: &[u8], out: &mut alloc::vec::Vec<u8>) {
    const FP: &str = "ferros husky 2026-10-14 wairua display fingerprint v1";
    if params == b"sleep" {
        let before = derive(WAIRUA, FP);
        let t0 = crate::platform::cntpct();
        leave(1 << me());
        let t1 = crate::platform::cntpct();
        lock();
        let ok = adopt();
        unlock();
        let t2 = crate::platform::cntpct();
        let after = derive(WAIRUA, FP);
        let us = |t: u64| t * 1_000_000 / crate::platform::cntfrq();
        out.extend_from_slice(alloc::format!("sleep rehearsal on core {}: seal+clear A#{} us, unseal A#{} us, adopted {}, wairua {}\n", me(), us(t1 - t0), us(t2 - t1), ok, match (before, after) { (Some(a), Some(b)) if a[..8] == b[..8] => "SAME", (None, _) => "not held before", _ => "CHANGED" }).as_bytes());
        // Where the unseal's time goes.
        let mut w = [0u64; 8];
        let a = crate::platform::cntpct();
        random(&mut w[..4]);
        let b = crate::platform::cntpct();
        random(&mut w);
        let c = crate::platform::cntpct();
        let _ = seal_keys(&[1; 32]);
        let d = crate::platform::cntpct();
        scrub_stack();
        let e = crate::platform::cntpct();
        out.extend_from_slice(alloc::format!("  random 4 words A#{} us, 8 words A#{} us, seal keys A#{} us, stack scrub A#{} us\n", us(b - a), us(c - b), us(d - c), us(e - d)).as_bytes());
    }
    if params == b"flip" && held() & 2 != 0 {
        // One bit of this core's wairua share 0 flipped, as a particle would: the next derive must notice, take a good copy from another core, and give the same key.
        let before = derive(WAIRUA, FP);
        let fails = CHECK_FAILS.load(Ordering::Relaxed);
        unsafe { core::arch::asm!("mrs {t}, S3_0_C2_C2_0", "eor {t}, {t}, #1", "msr S3_0_C2_C2_0, {t}", t = out(reg) _, options(nomem, nostack)) };
        let t0 = crate::platform::cntpct();
        let after = derive(WAIRUA, FP);
        let t1 = crate::platform::cntpct();
        out.extend_from_slice(alloc::format!("flip on core {}: caught {}, repaired in A#{} us, wairua {}\n", me(), CHECK_FAILS.load(Ordering::Relaxed) > fails, (t1 - t0) * 1_000_000 / crate::platform::cntfrq(), match (before, after) { (Some(a), Some(b)) if a[..8] == b[..8] => "SAME", (_, None) => "NOT HELD", _ => "CHANGED" }).as_bytes());
    }
    if params == b"time" && held() & 2 != 0 {
        // The cost of a use, in this core's cycles: the lifted derive alone (shares joined + one compression), and the whole `derive` (interrupts masked, scrub, context key).
        let cyc = || -> u64 {
            let c: u64;
            unsafe { core::arch::asm!("isb", "mrs {}, pmccntr_el0", out(reg) c, options(nomem, nostack)) };
            c
        };
        let ck = context_key("ferros keyreg timing v0");
        let mut o = Out4([0; 4]);
        const N: u64 = 1 << 12;
        let t0 = cyc();
        for _ in 0..N {
            sealed(|| unsafe { keyreg_derive(WAIRUA, &ck, &mut o) });
        }
        let t1 = cyc();
        for _ in 0..N {
            let _ = derive(WAIRUA, "ferros keyreg timing v0");
        }
        let t2 = cyc();
        let mut s = Out8([0; 8]);
        for _ in 0..N {
            sealed(|| unsafe { keyreg_stream(&ck, &[1, 0, 0, 0, 0, 0, 0, 0], 8, 0, &mut s) });
        }
        let t3 = cyc();
        wipe_words(&mut o.0);
        wipe_words(&mut s.0);
        out.extend_from_slice(alloc::format!("core {}: cycles per call: lifted derive A#{}, whole derive A#{}, stream block A#{}\n", me(), (t1 - t0) / N, (t2 - t1) / N, (t3 - t2) / N).as_bytes());
    }
    let test = TEST.load(Ordering::Acquire);
    out.extend_from_slice(alloc::format!("self-test {}  generation {}  present {}{}  blob waits for G#{:X}  asking G#{:X}  weak masks {}  bad blobs {}  check fails {}  tag fixes {}\n", match test { 0 => "not run".into(), 1 => "passed".into(), f => alloc::format!("FAILED at {}", f - 1) }, GEN.load(Ordering::Acquire), if PRESENT.load(Ordering::Acquire) & 1 != 0 { "ira " } else { "" }, if PRESENT.load(Ordering::Acquire) & 2 != 0 { "wairua" } else { "" }, WANT.load(Ordering::Acquire), ASKING.load(Ordering::Acquire), WEAK_MASKS.load(Ordering::Relaxed), BAD_BLOBS.load(Ordering::Relaxed), CHECK_FAILS.load(Ordering::Relaxed), TAG_FIXES.load(Ordering::Relaxed)).as_bytes());
    for (k, h) in HELD.iter().enumerate() {
        let g = h.load(Ordering::Acquire);
        if g != 0 {
            out.extend_from_slice(alloc::format!("  core {k} holds generation {g}{}\n", if k == me() { " (this core)" } else { "" }).as_bytes());
        }
    }
    if let Some(fp) = derive(WAIRUA, FP) {
        out.extend_from_slice(alloc::format!("  wairua fingerprint {:02X}{:02X}\n", fp[0], fp[1]).as_bytes());
    }
}

/// Whether these registers hold anything yet (the register probe, keyreg.rs, writes patterns into them and must not run once they do).
pub fn in_use() -> bool {
    GEN.load(Ordering::Acquire) != 0
}
