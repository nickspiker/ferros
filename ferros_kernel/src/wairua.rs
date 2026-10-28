//! *wairua* — the per-session secret, drawn fresh from physical entropy once per power session.
//!
//! Distinct from the *ira* (see [`crate::ira`] and VAULT-KEY.md / GLOSSARY.md): the ira is permanent and identity-bearing; the wairua is a volatile per-power-session secret that dies at power interruption and is never persisted. Session working keys derive from it, so when the session ends — or power drops — a wairua-derived key is unrecoverable by construction (freeze IS the destruct, see KEY_REGISTERS.md).
//!
//! Lazy: nothing at boot needs it (opening and reading the vault don't; only encrypting does), so KOHI runs the first time something asks — by then more entropy sources can be up to feed it. The checklist's wairua line turns yellow at that moment.
//!
//! It comes from one KOHI per session ([`crate::kohi`]: PIPE's health stack and byzantine threshold over the firmware TRNG, RNDR where present, and CPU timing jitter). No healthy majority, no wairua: callers must refuse to proceed (NGARO), never fall back to a counter or timestamp.
//!
//! Held in CPU registers, not RAM, since 2026-10-28 (keys.rs, KEY_REGISTERS.md): KOHI hands it over the moment it is drawn and its copy is wiped; everything that needs it asks keys.rs for a key derived from it. What this module keeps is public: whether KOHI succeeded, its health, and the one-way fingerprint.

use core::cell::UnsafeCell;

/// BLAKE3 KDF context for the session secret. v1: KOHI (multi-source, health-tested) replaced v0's single TRNG draw.
const WAIRUA_CONTEXT: &str = "ferros.wairua.husky.v1";
/// Context for the vault nonce seed derived from the wairua.
const NONCE_SEED_CONTEXT: &str = "ferros.wairua.nonce-seed.v1";

/// KOHI's outcome (the wairua drawn and held, or not) and its health, once it has run.
struct Session(UnsafeCell<Option<(bool, [crate::kohi::Health; 3])>>);
// Single core, never touched from the IRQ handler.
unsafe impl Sync for Session {}
static SESSION: Session = Session(UnsafeCell::new(None));

/// How long this session's KOHI took (ms); 0 until it has run.
static KOHI_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn session_state() -> &'static (bool, [crate::kohi::Health; 3]) {
    unsafe {
        let s = &mut *SESSION.0.get();
        if s.is_none() {
            let t0 = crate::platform::cntpct();
            let (mut w, health) = crate::kohi::gather(WAIRUA_CONTEXT);
            KOHI_MS.store(((crate::platform::cntpct() - t0) / (crate::platform::cntfrq() / 1000)) as u32, core::sync::atomic::Ordering::Relaxed);
            // Into the registers; `keys::set` wipes this copy.
            let held = w.as_mut().is_some_and(|w| crate::keys::set(crate::keys::WAIRUA, w));
            *s = Some((held, health));
            if held {
                // Yellow: up, but a stand-in until PIPE (held in CPU registers, not on-die ones). The first two bytes of its one-way fingerprint follow the label.
                let fp = fingerprint().unwrap_or_default();
                let hex = b"0123456789ABCDEF";
                let s4 = [hex[(fp[0] >> 4) as usize], hex[(fp[0] & 15) as usize], hex[(fp[1] >> 4) as usize], hex[(fp[1] & 15) as usize]];
                crate::checklist::mark_interim(crate::checklist::Item::Wairua);
                crate::checklist::suffix(crate::checklist::Item::Wairua, &s4);
            }
        }
        s.as_ref().unwrap()
    }
}

/// One-way display fingerprint of the wairua (BLAKE3 derive_key under its own context, first 64 bits). None until it is drawn.
pub fn fingerprint() -> Option<[u8; 8]> {
    let k = crate::keys::derive(crate::keys::WAIRUA, "ferros husky 2026-10-14 wairua display fingerprint v1")?;
    let mut fp = [0u8; 8];
    fp.copy_from_slice(&k[..8]);
    Some(fp)
}

/// This session's KOHI result if it has run, without running it: whether the wairua is held, and each source's health.
pub fn peek() -> Option<&'static (bool, [crate::kohi::Health; 3])> {
    unsafe { (*SESSION.0.get()).as_ref() }
}

pub fn kohi_ms() -> u32 {
    KOHI_MS.load(core::sync::atomic::Ordering::Relaxed)
}

/// Draw this session's wairua if it has not been (the first call runs KOHI). False = KOHI failed (NGARO): there is none.
pub fn draw() -> bool {
    session_state().0
}

/// `blake3::derive_key(context, wairua)`, drawing it first if need be. None = no wairua.
pub fn derive(context: &str) -> Option<[u8; 32]> {
    if !draw() {
        return None;
    }
    crate::keys::derive(crate::keys::WAIRUA, context)
}

/// A [`NonceSource`](ferros_vault::crypto::NonceSource) for the vault store.
///
/// `nonce_i = BLAKE3-keyed(seed, i)`, seed = a key derived from this session's wairua, i a monotonic counter: unique (distinct counter) and unpredictable (secret seed), and never repeating across boots because the wairua is fresh each power session.
/// Without a wairua there is no safe seed — a zero seed would make nonces a bare counter that restarts every boot, repeating nonces across boots under the same key — so the source refuses: reads still work, the first encryption panics (reported through the fault sink) instead of writing with a reused nonce.
pub struct TrngNonce {
    /// Derived from the wairua at the first encryption (that is what runs KOHI if nothing has yet).
    seed: Option<Option<[u8; 32]>>,
    counter: u64,
}

impl TrngNonce {
    pub fn new() -> Self {
        Self { seed: None, counter: 0 }
    }
}

impl ferros_vault::crypto::NonceSource for TrngNonce {
    fn next_nonce(&mut self) -> [u8; 24] {
        let seed = *self.seed.get_or_insert_with(|| derive(NONCE_SEED_CONTEXT));
        let Some(seed) = seed else {
            panic!("no wairua (KOHI failed): refusing to encrypt with a reused nonce");
        };
        let mut hasher = blake3::Hasher::new_keyed(&seed);
        hasher.update(&self.counter.to_le_bytes());
        self.counter = self.counter.wrapping_add(1);
        let mut n = [0u8; 24];
        n.copy_from_slice(&hasher.finalize().as_bytes()[..24]);
        n
    }
}
