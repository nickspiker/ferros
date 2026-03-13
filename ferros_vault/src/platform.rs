//! Platform-specific boot and storage configurations.
//!
//! This module captures the concrete hardware reality for each target
//! platform. No hand-waving — every path maps to a real device, a real
//! partition, a real driver.
//!
//! # Fairphone 5 Development Layout
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │ Internal UFS (256GB)                                    │
//! │ Qualcomm QCM6490, GPT, A/B slots                       │
//! │                                                         │
//! │ boot_a ──── Android kernel (safety net)                 │
//! │ boot_b ──── ferros kernel                               │
//! │ super_a ─── Android system images                       │
//! │ super_b ─── (unused or minimal ferros initrd)           │
//! │ userdata ── Android's ~200GB (don't touch)              │
//! │                                                         │
//! │ Anchor key: embedded in boot_b image (Phase 1)          │
//! │             or ferros_anchor partition (if GPT modified) │
//! └─────────────────────────────────────────────────────────┘
//!
//! ┌─────────────────────────────────────────────────────────┐
//! │ microSD card (64–256GB, different vendor)                │
//! │                                                         │
//! │ THE LEDGER LIVES HERE (Phase 1)                         │
//! │                                                         │
//! │ Raw block access — no filesystem on the card.           │
//! │ Layout:                                                 │
//! │   [anchor ring: slots at BLAKE3-derived offsets]         │
//! │   [object store: append-only content-addressed objects]  │
//! │   [commit chain: generation-linked commit records]       │
//! │                                                         │
//! │ Removable: pull card, inspect on PC, dd dump, swap.     │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Dev Workflow
//!
//! ```text
//! # Iterative development (no flash, boots once):
//! $ fastboot boot ferros.img
//!
//! # Persistent dual-boot:
//! $ fastboot flash boot_b ferros.img
//! $ fastboot set_active b       # boot ferros
//! $ fastboot set_active a       # back to android
//!
//! # Inspect ledger on PC:
//! $ dd if=/dev/mmcblk0 bs=1M | ferros_inspect --dump
//! ```
//!
//! ## Phase Progression
//!
//! ```text
//! Phase 1 (now):
//!   Mesh = 1 device (microSD, degraded mode)
//!   Anchor key = embedded in boot image or small partition
//!   Object store = raw microSD
//!   Switch slots via fastboot
//!
//! Phase 1.5 (soon):
//!   Mesh = 2 devices (microSD + internal UFS GPT partition)
//!   Shrink userdata, add two GPT partitions:
//!     ferros_anchor (4KB, type 66657272-6f73-416e-...)
//!     ferros_vault (50GB, type 66657272-6f73-4c65-...)
//!   Real mesh: dual device, dual vendor
//!   Anchor key = ferros_anchor partition on internal UFS
//!
//! Phase 2 (custom ABL):
//!   Anchor key = RPMB via ABL→DTB handoff
//!   Mesh = 2 devices with hardware-isolated key store
//!
//! Phase 3 (Glyph):
//!   Own silicon, own secure world, own everything
//! ```

use alloc::vec;
use alloc::vec::Vec;

use crate::anchor::KeyStoreBackend;

// ---------------------------------------------------------------------------
// GPT Partition Type GUIDs for Ferros
// ---------------------------------------------------------------------------

/// GPT partition type GUID for the ferros ledger data partition.
///
/// We register a proper type GUID so that:
/// - Android's vold/sgdisk won't try to mount or format our partition
/// - GPT-aware tools can identify it as "ferros ledger"
/// - We're a polite neighbor in a shared partition table
///
/// Generated as a v4 UUID. This is the ferros project's GUID —
/// any tool that sees this type knows it's a raw ledger partition,
/// not a filesystem.
///
/// `66657272-6f73-4c65-6467-657200000001`
///  f  e  r  r  o  s  Le  dg  e  r  ...  01
pub const GPT_TYPE_FERROS_LEDGER: [u8; 16] = [
    0x66, 0x65, 0x72, 0x72, // "ferr"
    0x6f, 0x73,             // "os"
    0x4c, 0x65,             // "Le"
    0x64, 0x67,             // "dg"
    0x65, 0x72,             // "er"
    0x00, 0x00, 0x00, 0x01, // version 01
];

/// GPT partition type GUID for the ferros anchor key partition.
///
/// Small partition (~4KB–64KB). Holds only the 32-byte anchor key
/// and ring config. Separate from the ledger data partition so that
/// the key store can be on a different LUN or device.
///
/// `66657272-6f73-416e-6368-6f7200000001`
///  f  e  r  r  o  s  An  ch  o  r  ...  01
pub const GPT_TYPE_FERROS_ANCHOR: [u8; 16] = [
    0x66, 0x65, 0x72, 0x72, // "ferr"
    0x6f, 0x73,             // "os"
    0x41, 0x6e,             // "An"
    0x63, 0x68,             // "ch"
    0x6f, 0x72,             // "or"
    0x00, 0x00, 0x00, 0x01, // version 01
];

/// Recommended GPT partition names (UTF-16LE in GPT, but we store as bytes).
pub const GPT_NAME_LEDGER: &str = "ferros_vault";
pub const GPT_NAME_ANCHOR: &str = "ferros_anchor";

/// Minimum sizes for GPT partitions.
pub const ANCHOR_PARTITION_MIN_BYTES: u64 = 4096;        // 4KB — holds the 32-byte key + config
pub const LEDGER_PARTITION_MIN_BYTES: u64 = 8 * 1024;    // 8KB — pico minimum
pub const LEDGER_PARTITION_RECOMMENDED_BYTES: u64 = 50 * 1024 * 1024 * 1024; // 50GB for FP5

/// The anchor ring lives INSIDE the ledger partition (first 1MB).
/// This constant documents the reservation — the object store starts
/// after the ring. Both mesh devices need this same layout.
pub const ANCHOR_RING_RESERVED_BYTES: u64 = crate::anchor::ANCHOR_RING_DEFAULT_BYTES;

// ---------------------------------------------------------------------------
// Platform definitions
// ---------------------------------------------------------------------------

/// Platform identifier — which hardware are we running on?
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Platform {
    /// Fairphone 5, QCM6490, development dual-boot.
    Fp5Dev,
    /// Fairphone 5, QCM6490, with custom ABL for RPMB access.
    Fp5CustomAbl,
    /// Glyph RISC-V custom hardware.
    Glyph,
    /// Minimal embedded device (8KB+ flash).
    Pico,
    /// QEMU or host-native testing.
    Test,
}

/// A storage device that the platform knows about.
#[derive(Clone, Debug)]
pub struct PlatformDevice {
    /// What this device is (human-readable for debugging).
    pub description: &'static str,
    /// How the kernel accesses this device.
    pub access: DeviceAccess,
    /// Vendor string (for mesh dual-vendor checks).
    pub vendor: &'static str,
    /// Capacity in bytes (0 = detect at runtime).
    pub capacity: u64,
}

/// How the kernel reaches a storage device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceAccess {
    /// Linux-style block device path (for early dev on mainline kernel).
    /// e.g., "/dev/mmcblk1" for microSD, "/dev/sda" for UFS LUN.
    BlockDevice { path: &'static str },
    /// Raw UFS LUN access (for ferros kernel, no Linux).
    UfsLun { lun: u8 },
    /// Raw SD/MMC controller access (for ferros kernel, no Linux).
    SdMmc { controller: u8 },
    /// RAM-backed (testing).
    Memory { size: u64 },
}

/// Complete platform configuration — everything the boot code needs
/// to find devices, read anchor keys, and mount the ledger.
#[derive(Clone, Debug)]
pub struct PlatformConfig {
    pub platform: Platform,
    pub key_store_backend: KeyStoreBackend,
    pub devices: Vec<PlatformDevice>,
    pub anchor_ring_size: u64,
}

/// FP5 development config: microSD as sole ledger device,
/// anchor key embedded (or in small partition).
pub fn fp5_dev_config() -> PlatformConfig {
    PlatformConfig {
        platform: Platform::Fp5Dev,
        key_store_backend: KeyStoreBackend::UfsPartition {
            partition_name: b"ferros_anchor".to_vec(),
            offset: 0,
        },
        devices: vec![
            PlatformDevice {
                description: "microSD card (ledger primary)",
                access: DeviceAccess::SdMmc { controller: 0 },
                vendor: "unknown", // detected at runtime from CID register
                capacity: 0,       // detected at runtime
            },
        ],
        anchor_ring_size: crate::anchor::ANCHOR_RING_DEFAULT_SLOTS,
    }
}

/// FP5 Phase 1.5: microSD + internal UFS GPT partition = real mesh.
///
/// Requires repartitioning the internal UFS:
/// ```text
/// # From Android recovery or fastboot:
/// # 1. Shrink userdata by ~50GB
/// # 2. Add ferros_anchor partition (4KB, type GPT_TYPE_FERROS_ANCHOR)
/// # 3. Add ferros_vault partition (50GB, type GPT_TYPE_FERROS_LEDGER)
/// ```
pub fn fp5_dual_mesh_config() -> PlatformConfig {
    PlatformConfig {
        platform: Platform::Fp5Dev,
        key_store_backend: KeyStoreBackend::UfsPartition {
            partition_name: GPT_NAME_ANCHOR.as_bytes().to_vec(),
            offset: 0,
        },
        devices: vec![
            PlatformDevice {
                description: "microSD card (mesh member 0)",
                access: DeviceAccess::SdMmc { controller: 0 },
                vendor: "unknown",
                capacity: 0,
            },
            PlatformDevice {
                description: "Internal UFS ferros_vault partition (mesh member 1)",
                access: DeviceAccess::BlockDevice { path: "/dev/disk/by-partlabel/ferros_vault" },
                vendor: "unknown",
                capacity: 0,
            },
        ],
        anchor_ring_size: crate::anchor::ANCHOR_RING_DEFAULT_SLOTS,
    }
}

/// FP5 Phase 2: custom ABL, RPMB-backed anchor key.
pub fn fp5_custom_abl_config() -> PlatformConfig {
    PlatformConfig {
        platform: Platform::Fp5CustomAbl,
        key_store_backend: KeyStoreBackend::AblDtbHandoff {
            dtb_node: b"/reserved-memory/ferros-anchor-key".to_vec(),
        },
        devices: vec![
            PlatformDevice {
                description: "microSD card (mesh member 0)",
                access: DeviceAccess::SdMmc { controller: 0 },
                vendor: "unknown",
                capacity: 0,
            },
            PlatformDevice {
                description: "Internal UFS ledger LUN (mesh member 1)",
                access: DeviceAccess::UfsLun { lun: 4 },
                vendor: "unknown",
                capacity: 0,
            },
        ],
        anchor_ring_size: crate::anchor::ANCHOR_RING_DEFAULT_SLOTS,
    }
}

/// QEMU / host-native test config.
pub fn test_config(device_size: u64) -> PlatformConfig {
    PlatformConfig {
        platform: Platform::Test,
        key_store_backend: KeyStoreBackend::Memory,
        devices: vec![
            PlatformDevice {
                description: "RAM device 0",
                access: DeviceAccess::Memory { size: device_size },
                vendor: "test-vendor-a",
                capacity: device_size,
            },
            PlatformDevice {
                description: "RAM device 1",
                access: DeviceAccess::Memory { size: device_size },
                vendor: "test-vendor-b",
                capacity: device_size,
            },
        ],
        anchor_ring_size: 4, // small ring for fast tests
    }
}
