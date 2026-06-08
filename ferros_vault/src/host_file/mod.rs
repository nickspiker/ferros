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

pub mod capability;
pub mod device;
pub mod mesh;

pub use capability::{PermissiveCapabilityEngine, ROOT_CAPABILITY_TOKEN};
pub use device::FileDevice;
pub use mesh::SingleDeviceMeshEngine;
