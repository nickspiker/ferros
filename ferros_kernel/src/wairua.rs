//! *wairua* — the per-session secret, drawn fresh from hardware entropy at boot.
//!
//! Distinct from the *ira* (see [`crate::ira`] and VAULT-KEY.md / GLOSSARY.md): the ira is permanent and identity-bearing; the wairua is a volatile per-power-session secret that dies at power interruption and is never persisted. Session working keys derive from it, so when the session ends — or power drops — a wairua-derived key is unrecoverable by construction (freeze IS the destruct, see KEY_REGISTERS.md).
//!
//! This module only *draws and conditions* the raw entropy into a 32-byte secret. Holding it in the V-register file under the inverted-pair guard, and clearing it at session end, is [`crate::ira`]'s sibling concern in KEY_REGISTERS.md and lands separately; for now the caller receives the bytes and is responsible for not spilling them to durable storage.
//!
//! Two hardware sources, preferred in order:
//! 1. **SMCCC TRNG** (`TRNG_RND64`) — the proven husky path (`smccc_trng` backs Linux `/dev/hw_random`). SMC from EL2 traps to EL3 / TF-A, which returns conditioned entropy.
//! 2. **FEAT_RNG `RNDR`** — the ARMv9 architectural RNG system register, used only when `ID_AA64ISAR0_EL1.RNDR != 0` (else the read is UNDEFINED and traps).
//!
//! Whatever the source, the raw words are folded through BLAKE3 `derive_key` so the 32-byte output is decoupled from source width and whitened even if a source is mildly biased.

/// SMCCC `TRNG_RND64` function id (SMC64, Standard Secure Service). Returns up to 192 bits of entropy across x1:x2:x3, x0 = status (0 = success).
const SMCCC_TRNG_RND64: u64 = 0xC400_0053;

/// Max bits `TRNG_RND64` returns in one call (x1:x2:x3).
const TRNG_MAX_BITS: u64 = 192;

/// BLAKE3 KDF context that conditions raw entropy into the session secret. Domain-separated from every other ferros derivation.
const WAIRUA_CONTEXT: &str = "ferros.wairua.husky.v0";

/// Issue an SMC with three arguments; return `[x0, x1, x2, x3]`.
///
/// From EL2 this traps to EL3 (TF-A), the same path the shim's PSCI resets use.
#[inline(never)]
fn smc4(func: u64, a1: u64, a2: u64, a3: u64) -> [u64; 4] {
    let (mut r0, mut r1, mut r2, mut r3);
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") func => r0,
            inout("x1") a1 => r1,
            inout("x2") a2 => r2,
            inout("x3") a3 => r3,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
            options(nostack),
        );
    }
    [r0, r1, r2, r3]
}

/// Request `bits` (1..=192) of entropy from the firmware TRNG. `None` if the call is unsupported or errored.
fn trng_rnd64(bits: u64) -> Option<[u64; 3]> {
    let bits = bits.min(TRNG_MAX_BITS).max(1);
    let [status, x1, x2, x3] = smc4(SMCCC_TRNG_RND64, bits, 0, 0);
    // SMCCC success is 0; any negative (top-bit-set) value is an error / NOT_SUPPORTED.
    if status == 0 {
        Some([x1, x2, x3])
    } else {
        None
    }
}

/// Whether `FEAT_RNG` (the `RNDR`/`RNDRRS` system registers) is implemented — `ID_AA64ISAR0_EL1.RNDR` (bits [63:60]) nonzero. Reading `RNDR` when this is zero is UNDEFINED.
fn has_feat_rng() -> bool {
    let isar0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ID_AA64ISAR0_EL1", out(reg) isar0, options(nomem, nostack));
    }
    (isar0 >> 60) & 0xF != 0
}

/// Read the `RNDR` system register (encoded `S3_3_C2_C4_0` so it assembles without target-feature flags). Returns `None` on the spec'd failure case (register reads 0 with Z set); we detect it by retrying a bounded number of times and rejecting an all-zero result.
fn rndr() -> Option<u64> {
    for _ in 0..16 {
        let v: u64;
        unsafe {
            core::arch::asm!("mrs {}, S3_3_C2_C4_0", out(reg) v, options(nomem, nostack));
        }
        if v != 0 {
            return Some(v);
        }
    }
    None
}

/// Draw the session *wairua*: 32 bytes conditioned from at least 256 bits of fresh hardware entropy.
///
/// Returns `None` only if no hardware entropy source is available — in which case the caller must NOT proceed with a real session (there is no safe fallback for a session secret, and inventing one from a counter or timestamp would be a silent downgrade).
pub fn draw() -> Option<[u8; 32]> {
    // Collect raw entropy words (little-endian) into a scratch buffer; 6 u64 = 384 bits, comfortably over the 256 we condition down to.
    let mut raw = [0u8; 48];
    let mut filled = 0usize;

    let push = |w: u64, raw: &mut [u8; 48], filled: &mut usize| {
        if *filled + 8 <= raw.len() {
            raw[*filled..*filled + 8].copy_from_slice(&w.to_le_bytes());
            *filled += 8;
        }
    };

    // Preferred source: firmware TRNG. Two 192-bit calls fill the buffer.
    let mut got_trng = false;
    for _ in 0..2 {
        if let Some(words) = trng_rnd64(TRNG_MAX_BITS) {
            got_trng = true;
            for w in words {
                push(w, &mut raw, &mut filled);
            }
        } else {
            break;
        }
    }

    // Fallback: architectural RNDR, if firmware TRNG was unavailable or short.
    if filled < 32 && has_feat_rng() {
        while filled < raw.len() {
            match rndr() {
                Some(w) => push(w, &mut raw, &mut filled),
                None => break,
            }
        }
    }

    if filled < 32 {
        // Neither source yielded 256 bits.
        let _ = got_trng;
        return None;
    }

    Some(blake3::derive_key(WAIRUA_CONTEXT, &raw[..filled]))
}

/// A [`NonceSource`](ferros_vault::crypto::NonceSource) for the vault store.
///
/// Construction: `nonce_i = BLAKE3-keyed(seed, i)` where `seed` is a fresh per-boot wairua and `i` a monotonic counter. Each nonce is unique (distinct counter) and unpredictable (secret seed), and — crucially — this needs only ONE entropy draw at construction, not one per nonce, so a transient TRNG hiccup mid-session can't starve it. Because the seed is a fresh wairua every boot, nonces never repeat across boots either.
pub struct TrngNonce {
    seed: [u8; 32],
    counter: u64,
}

impl TrngNonce {
    /// Draw a fresh per-boot seed. On the (unexpected on husky) event that no hardware entropy is available, the seed is zero and nonces degrade to counter-only — a condition the `entry_vault` `wairua_ok` instrumentation flags separately.
    pub fn new() -> Self {
        Self {
            seed: draw().unwrap_or([0u8; 32]),
            counter: 0,
        }
    }
}

impl ferros_vault::crypto::NonceSource for TrngNonce {
    fn next_nonce(&mut self) -> [u8; 24] {
        let mut hasher = blake3::Hasher::new_keyed(&self.seed);
        hasher.update(&self.counter.to_le_bytes());
        self.counter = self.counter.wrapping_add(1);
        let mut n = [0u8; 24];
        n.copy_from_slice(&hasher.finalize().as_bytes()[..24]);
        n
    }
}
