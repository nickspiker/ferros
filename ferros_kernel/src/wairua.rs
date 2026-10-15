//! *wairua* — the per-session secret, drawn fresh from physical entropy once per power session.
//!
//! Distinct from the *ira* (see [`crate::ira`] and VAULT-KEY.md / GLOSSARY.md): the ira is permanent and identity-bearing; the wairua is a volatile per-power-session secret that dies at power interruption and is never persisted. Session working keys derive from it, so when the session ends — or power drops — a wairua-derived key is unrecoverable by construction (freeze IS the destruct, see KEY_REGISTERS.md).
//!
//! It comes from one KOHI per session ([`crate::kohi`]: PIPE's health stack and byzantine threshold over the firmware TRNG, RNDR where present, and CPU timing jitter). No healthy majority, no wairua: callers must refuse to proceed (NGARO), never fall back to a counter or timestamp.
//!
//! Not yet to spec: the value lives in RAM. Holding it in the V-register file under the inverted-pair guard, and clearing it at session end, is KEY_REGISTERS.md's concern and lands separately.

use core::cell::UnsafeCell;

/// BLAKE3 KDF context for the session secret. v1: KOHI (multi-source, health-tested) replaced v0's single TRNG draw.
const WAIRUA_CONTEXT: &str = "ferros.wairua.husky.v1";
/// Context for the vault nonce seed derived from the wairua.
const NONCE_SEED_CONTEXT: &str = "ferros.wairua.nonce-seed.v1";

struct Session(UnsafeCell<Option<(Option<[u8; 32]>, [crate::kohi::Health; 3])>>);
// Single core, never touched from the IRQ handler.
unsafe impl Sync for Session {}
static SESSION: Session = Session(UnsafeCell::new(None));

fn session_state() -> &'static (Option<[u8; 32]>, [crate::kohi::Health; 3]) {
    unsafe {
        let s = &mut *SESSION.0.get();
        if s.is_none() {
            *s = Some(crate::kohi::gather(WAIRUA_CONTEXT));
        }
        s.as_ref().unwrap()
    }
}

/// This session's wairua: the first call runs KOHI, later calls return the same value. `None` = KOHI failed (NGARO).
pub fn draw() -> Option<[u8; 32]> {
    session_state().0
}

/// Each KOHI source's health from this session's gather (order: [`crate::kohi::SOURCES`]).
pub fn source_health() -> [crate::kohi::Health; 3] {
    session_state().1
}

/// A [`NonceSource`](ferros_vault::crypto::NonceSource) for the vault store.
///
/// `nonce_i = BLAKE3-keyed(seed, i)`, seed = a key derived from this session's wairua, i a monotonic counter: unique (distinct counter) and unpredictable (secret seed), and never repeating across boots because the wairua is fresh each power session.
/// Without a wairua there is no safe seed — a zero seed would make nonces a bare counter that restarts every boot, repeating nonces across boots under the same key — so the source refuses: reads still work, the first encryption panics (reported through the fault sink) instead of writing with a reused nonce.
pub struct TrngNonce {
    seed: Option<[u8; 32]>,
    counter: u64,
}

impl TrngNonce {
    pub fn new() -> Self {
        Self { seed: draw().map(|w| blake3::derive_key(NONCE_SEED_CONTEXT, &w)), counter: 0 }
    }
}

impl ferros_vault::crypto::NonceSource for TrngNonce {
    fn next_nonce(&mut self) -> [u8; 24] {
        let Some(seed) = self.seed else {
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
