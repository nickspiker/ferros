//! Linux single-file vault backend — implements the Ledger trait set against `std::fs::File`.
//!
//! Used by Photon (and any other userspace consumer) as a proving ground for the vault design before it ships on the real ferros OS. Everything that lands here has a direct equivalent on the real-hardware side; the only difference is which trait impls plug into [`crate::Ledger`].
//!
//! Module layout:
//! - [`device`] — `FileDevice`, the [`crate::device::Device`] impl over a single regular file.
//! - [`vsf_wrapper`] — outer VSF envelope (magic + version + provenance hash + one `vault` section) that wraps the opaque vault payload.
//! - [`store`] — `FileStore`, the [`crate::store::ObjectStore`] impl that lives inside that VSF wrapper.
//! - [`anchor_key_store`] — derives the 32-byte anchor key from photon's `(identity_seed, device_secret)` roots.
//! - [`root_commit`] — the `logical_key → content_hash` dictionary object that bridges photon's logical-key API onto content-addressed storage.
//! - [`capability`] — `PermissiveCapabilityEngine`, no-op verifier (single-user single-device assumption).
//! - [`mesh`] — `SingleDeviceMeshEngine`, no-op consensus (single-device assumption).
//!
//! Type alias [`PhotonLedger`] collapses the three-generic [`crate::Ledger`] into the concrete combo Photon uses.

pub mod anchor_key_store;
pub mod capability;
pub mod device;
pub mod inspect;
pub mod mesh;
pub mod root_commit;
pub mod store;
pub mod vault_anchor;
pub mod vsf_wrapper;

pub use anchor_key_store::derive_anchor_key;
pub use inspect::{inspect_vault, InspectError};
pub use capability::{PermissiveCapabilityEngine, ROOT_CAPABILITY_TOKEN};
pub use device::FileDevice;
pub use mesh::SingleDeviceMeshEngine;
pub use root_commit::{RootCommit, RootCommitError};
pub use store::{FileStore, FileStoreError, DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE};
pub use vault_anchor::{
    build as build_anchor, compute_hmac as compute_anchor_hmac, decode as decode_anchor,
    derive_slot_offset, derive_slot_offset_with_probe, encode as encode_anchor, AnchorError,
    VaultAnchor, SLOT_STRIDE, SLOT_ZERO_OFFSET,
};
