//! `SingleDeviceMeshEngine` — no-op mesh consensus for single-device deployments.
//!
//! Same shape as [`PermissiveCapabilityEngine`]: the [`crate::Ledger`] generic over `M: MeshEngine` is what makes single-vs-multi-device a swap-in-place at the engine layer. For Photon-on-Linux there's one device; "consensus" is trivially "this device agrees with itself."
//!
//! The mesh member list has exactly one member — the local device. Generation is incremented per commit. Every `propose_commit` returns `Committed`.

use alloc::vec::Vec;

use crate::commit::CommitRecord;
use crate::device::DeviceId;
use crate::mesh::{ConsensusResult, DeviceState, MeshEngine, MeshError, MeshMember};

/// No-op mesh engine for single-device single-user use. Holds one fixed `MeshMember` and an internal `generation` counter that advances on each `propose_commit`.
#[derive(Clone, Debug)]
pub struct SingleDeviceMeshEngine {
    members: Vec<MeshMember>,
    generation: u64,
}

impl SingleDeviceMeshEngine {
    /// Build with a single member identified by `device_id` (typically derived from the host's machine fingerprint) and a vendor string (informational; the real `is_healthy` dual-vendor check is no-op here).
    pub fn new(device_id: DeviceId, vendor: Vec<u8>) -> Self {
        let member = MeshMember {
            device_id,
            state: DeviceState::Active,
            confirmed_generation: 0,
            vendor,
        };
        Self {
            members: alloc::vec![member],
            generation: 0,
        }
    }
}

impl MeshEngine for SingleDeviceMeshEngine {
    /// Always returns `Committed`. Generation auto-advances. Single-device means no consensus to fail.
    fn propose_commit(&mut self, _commit: &CommitRecord) -> ConsensusResult {
        self.generation = self.generation.saturating_add(1);
        if let Some(m) = self.members.first_mut() {
            m.confirmed_generation = self.generation;
        }
        ConsensusResult::Committed {
            generation: self.generation,
        }
    }

    fn members(&self) -> &[MeshMember] {
        &self.members
    }

    fn current_generation(&self) -> u64 {
        self.generation
    }

    /// Adding members is a no-op success — the single-device engine doesn't track a real member list. Returning Ok rather than erroring so any code that defensively adds members works.
    fn add_member(
        &mut self,
        _device_id: DeviceId,
        _vendor: Vec<u8>,
    ) -> Result<(), MeshError> {
        Ok(())
    }

    fn evict_member(&mut self, _device_id: DeviceId) -> Result<(), MeshError> {
        Ok(())
    }

    /// No-op resolve_conflict — returns an unresolvable error since single-device can't have a conflict in the first place. Callers shouldn't be calling this.
    fn resolve_conflict(
        &self,
        _a: &DeviceId,
        _b: &DeviceId,
        _generation: u64,
    ) -> Result<crate::hash::ObjectHash, MeshError> {
        Err(MeshError::UnresolvableConflict)
    }

    /// Single-device is "healthy" by definition — there's no dual-vendor requirement to violate, and "the local device is reachable" is implicit in the engine existing.
    fn is_healthy(&self) -> bool {
        true
    }
}
