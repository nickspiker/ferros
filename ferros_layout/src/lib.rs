//! Partition layout and shared constants for ferros.
//!
//! All block addresses are 4KB-aligned (UFS native block size).
//! Same layout on UFS and SD — the kernel mirrors everything.
//!
//! Edit these constants to resize rings or move regions.
//! All crates import from here — one edit, one rebuild.

#![no_std]

// ---------------------------------------------------------------------------
// Block geometry
// ---------------------------------------------------------------------------

/// Block size in bytes (UFS native, SD uses 8×512-byte sectors per block).
pub const BLOCK_SIZE: usize = 4096;

/// Convert a block number to a byte offset.
pub const fn block_to_bytes(block: u32) -> u64 {
    block as u64 * BLOCK_SIZE as u64
}

// ---------------------------------------------------------------------------
// Seed (loaded by ABL from boot_a / boot_b partitions, not from these blocks)
// These are for writing seed copies to UFS data partition for recovery.
// ---------------------------------------------------------------------------

/// Seed copy A — block offset on UFS data partition.
pub const SEED_A_BLOCK: u32 = 0x400;

/// Seed copy B — >1MB from A for redundancy.
pub const SEED_B_BLOCK: u32 = 0x800;

// ---------------------------------------------------------------------------
// Kernel ring — scanned by the seed to find the current kernel
// ---------------------------------------------------------------------------

/// Base block for the kernel ring (UFS and SD, same address).
pub const KERNEL_RING_BASE: u32 = 0xC00;

/// Number of entries in the kernel ring. Must be a power of 2.
pub const KERNEL_RING_SIZE: u32 = 256;

/// Binary search depth for the kernel ring (log2(KERNEL_RING_SIZE)).
pub const KERNEL_RING_DEPTH: u32 = 8;

// ---------------------------------------------------------------------------
// Vault root ring — scanned by the kernel to find system state
// ---------------------------------------------------------------------------

/// Base block for the vault root ring.
pub const VAULT_ROOT_RING_BASE: u32 = 0x2000;

/// Number of entries in the vault root ring. Must be a power of 2.
pub const VAULT_ROOT_RING_SIZE: u32 = 1 << 16; // 65536

/// Binary search depth for the vault root ring.
pub const VAULT_ROOT_RING_DEPTH: u32 = 16;

// ---------------------------------------------------------------------------
// State ring — process state, capabilities, display state
// ---------------------------------------------------------------------------

/// Base block for the state ring.
pub const STATE_RING_BASE: u32 = 0x4_0000;

/// Number of entries in the state ring. Must be a power of 2.
pub const STATE_RING_SIZE: u32 = 1 << 18; // 262144 = 1GB

// ---------------------------------------------------------------------------
// Ledger ring — categorized event chain
// ---------------------------------------------------------------------------

/// Base block for the ledger ring.
pub const LEDGER_RING_BASE: u32 = 0x8_0000;

/// Number of entries in the ledger ring. Must be a power of 2.
pub const LEDGER_RING_SIZE: u32 = 1 << 18; // 262144 = 1GB

// ---------------------------------------------------------------------------
// HAMT region — kernel binaries, vault objects, everything else
// ---------------------------------------------------------------------------

/// First block of the HAMT region. Everything before this is ring/reserved.
pub const HAMT_BASE: u32 = 0xC_0000;

// ---------------------------------------------------------------------------
// DRAM addresses
// ---------------------------------------------------------------------------

/// Kernel load address in DRAM. The seed loads the kernel here and jumps.
/// Must match the kernel's linker script (_start address).
pub const KERNEL_DRAM_BASE: usize = 0x8008_0000;

/// Staging area for hot-reload (kernel loads new image here before jumping).
pub const RELOAD_STAGE_BASE: usize = 0x9000_0000;

/// Splash framebuffer base (ABL-configured, DPU scans from here).
pub const SPLASH_FB_BASE: usize = 0xE100_0000;

// ---------------------------------------------------------------------------
// Hardware base addresses (QCM6490 / Fairphone 5)
// ---------------------------------------------------------------------------

/// UFSHCI v3.0 controller base address.
pub const UFS_BASE: usize = 0x01D8_4000;

/// SDHCI controller base for SD card (SDC2).
pub const SDHCI_BASE: usize = 0x08804000;
