//! Ledger categories — typed namespaces for event chains.
//!
//! Each category maintains its own independent BLAKE3 hash chain, sequence counter, and capability scope.

/// Ledger category identifier.
///
/// Categories form a tree. Each leaf has an independent chain. The `as_bytes()` representation is the canonical category path used in entry encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Category {
    // -- Kernel --
    KernelBoot = 0x01,
    KernelIpc = 0x02,
    KernelMemory = 0x03,
    KernelCapability = 0x04,
    KernelKill = 0x05,

    // -- USB --
    UsbPhy = 0x10,
    UsbDwc3 = 0x11,
    UsbEnumeration = 0x12,
    UsbPt = 0x13,

    // -- Photon --
    PhotonTransport = 0x20,
    PhotonToken = 0x21,
    PhotonMessages = 0x22,

    // -- TOKEN --
    TokenAttestation = 0x30,
    TokenAuth = 0x31,

    // -- Vault (Ring FS) --
    VaultBoot = 0x40,
    VaultWrite = 0x41,
    VaultRepair = 0x42,

    // -- VSF --
    Vsf = 0x50,
}

impl Category {
    /// Canonical path bytes for this category (used in entry identity section).
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::KernelBoot => b"Kernel.Boot",
            Self::KernelIpc => b"Kernel.IPC",
            Self::KernelMemory => b"Kernel.Memory",
            Self::KernelCapability => b"Kernel.Capability",
            Self::KernelKill => b"Kernel.Kill",
            Self::UsbPhy => b"USB.PHY",
            Self::UsbDwc3 => b"USB.DWC3",
            Self::UsbEnumeration => b"USB.Enumeration",
            Self::UsbPt => b"USB.PT",
            Self::PhotonTransport => b"Photon.Transport",
            Self::PhotonToken => b"Photon.TOKEN",
            Self::PhotonMessages => b"Photon.Messages",
            Self::TokenAttestation => b"TOKEN.Attestation",
            Self::TokenAuth => b"TOKEN.Auth",
            Self::VaultBoot => b"Vault.Boot",
            Self::VaultWrite => b"Vault.Write",
            Self::VaultRepair => b"Vault.Repair",
            Self::Vsf => b"VSF",
        }
    }
}
