//! `PermissiveCapabilityEngine` — no-op capability verifier for single-user single-device deployments (Photon-on-Linux today; any future single-user app).
//!
//! Why this exists: the [`crate::Ledger`] type signature carries a `C: CapabilityEngine` generic so the same code can run on multi-user ferros hardware (where capabilities are real). For Photon's "one user owns the entire device" reality, capabilities collapse to "if you have the master key, you have everything." This stub satisfies the trait without doing any actual gating.
//!
//! When we eventually port to multi-user contexts, swap this engine for a real [`CapabilityEngine`] impl; the rest of the code is unchanged.

use alloc::vec::Vec;

use crate::capability::{
    CapabilityConstraints, CapabilityEngine, CapabilityError, CapabilityLevel, CapabilityToken,
};
use crate::hash::{DerivedSeed, ObjectHash, Salt};

/// A pre-built [`CapabilityToken`] that the `PermissiveCapabilityEngine` always accepts. Single-user code passes this to every `Ledger::{get, put}` call; the verifier always returns `Ok`. Equivalent to having root in a Unix world — but contained to single-user deployments where there's no other user to protect anything from.
///
/// Fields are set to zero/sentinel values because they don't matter — the permissive engine ignores them entirely. Constructed via `const fn` so it's a true compile-time constant.
pub const ROOT_CAPABILITY_TOKEN: CapabilityToken = CapabilityToken {
    target: ObjectHash([0u8; 32]),
    level: CapabilityLevel::Write,
    credential: ObjectHash([0u8; 32]),
    delegated_from: None,
    constraints: CapabilityConstraints {
        max_delegation_depth: None,
        current_depth: 0,
        expires_at_generation: None,
        domain_restriction: None,
    },
    generation: 0,
};

/// Stub capability engine — accepts every token, supports no delegation, supports no revocation.
#[derive(Clone, Debug, Default)]
pub struct PermissiveCapabilityEngine;

impl PermissiveCapabilityEngine {
    pub const fn new() -> Self {
        Self
    }
}

impl CapabilityEngine for PermissiveCapabilityEngine {
    /// Always Ok. Single-user means "you have the key, you have access" — there's no second user to gate against.
    fn verify(
        &self,
        _token: &CapabilityToken,
        _current_generation: u64,
    ) -> Result<(), CapabilityError> {
        Ok(())
    }

    /// Delegation is meaningless when there's no other user. Errors out so callers can't accidentally rely on delegation semantics that aren't real.
    fn delegate(
        &self,
        _parent: &CapabilityToken,
        _new_level: CapabilityLevel,
        _new_constraints: CapabilityConstraints,
    ) -> Result<CapabilityToken, CapabilityError> {
        Err(CapabilityError::InvalidCredential)
    }

    /// Revocation likewise has no meaning in single-user; no-op success so callers that defensively revoke don't break.
    fn revoke(&mut self, _target: &ObjectHash, _new_salt: Salt) -> Result<(), CapabilityError> {
        Ok(())
    }

    /// No salt table in the permissive engine; returns None so any code that depends on salt rotation falls through to its no-salt path.
    fn current_salt(&self, _target: &ObjectHash) -> Option<Salt> {
        None
    }

    /// No derived-seed table either — same rationale as `current_salt`.
    fn derived_seeds(&self, _target: &ObjectHash) -> Option<(DerivedSeed, DerivedSeed)> {
        None
    }
}

// Suppress dead-code warnings during scaffold phase — these'll be used once the rest of the host_file module lands.
#[allow(dead_code)]
fn _vec_capacity_check() -> Vec<u8> {
    Vec::new()
}
