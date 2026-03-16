//! Storage backend trait — abstraction over physical storage.
//!
//! The Ledger doesn't assume block devices, sectors, or any particular
//! storage topology. This trait abstracts over anything that can store
//! and retrieve byte ranges: NVMe SSDs, eMMC flash, RAM disks, or
//! network-attached storage in a distributed mesh.
//!
//! ## Contrast
//! - BTRFS: Assumes block devices with fixed sector sizes (default 4K).
//!   The chunk tree maps logical addresses to physical (device, offset)
//!   pairs. Devices are added/removed via btrfs device commands.
//!   Minimum: one block device with known sector size.
//! - RedoxFS: Operates on a `Disk` trait with `read_at`/`write_at`
//!   taking block-aligned offsets. Fixed BLOCK_SIZE=4096. Supports
//!   a single disk with optional reserved bootloader area.
//! - Ledger: No block alignment. No sector size. The device trait
//!   accepts arbitrary byte ranges. The VSF layer handles its own
//!   encoding — the device just stores and retrieves bytes.

use alloc::vec::Vec;

/// Unique identifier for a storage device within a mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceId(pub [u8; 16]);

/// Errors from device operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceError {
    /// The requested range is beyond the device's capacity.
    OutOfBounds { offset: u64, len: u64, capacity: u64 },
    /// The device reported an I/O error.
    IoError(DeviceIoKind),
    /// The device is not ready (not initialized, powered down, etc.).
    NotReady,
    /// The device has been evicted from the mesh.
    Evicted,
}

/// Categories of I/O errors (no std::io dependency for no_std compat).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceIoKind {
    ReadFailed,
    WriteFailed,
    ReadError,
    WriteError,
    FlushFailed,
    Timeout,
    InvalidParam,
    PermissionDenied,
    HardwareFailure,
}

/// Information about a storage device's capabilities.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub id: DeviceId,
    /// Total capacity in bytes.
    pub capacity: u64,
    /// Vendor identifier (for dual-vendor mesh requirement).
    pub vendor: Vec<u8>,
    /// Model/serial for identification.
    pub model: Vec<u8>,
    /// Whether the device supports atomic writes of the given size.
    /// If None, no atomic write guarantee.
    pub atomic_write_size: Option<u64>,
    /// Whether the device has a volatile write cache that needs explicit flush.
    pub has_volatile_cache: bool,
}

/// The storage device trait — the lowest-level storage abstraction.
///
/// Implementations: raw NVMe, eMMC, RAM disk, network proxy, etc.
pub trait Device {
    /// Read `buf.len()` bytes starting at `offset`.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), DeviceError>;

    /// Write `data` starting at `offset`.
    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<(), DeviceError>;

    /// Ensure all previous writes are durable (survived power loss).
    fn flush(&mut self) -> Result<(), DeviceError>;

    /// Query device information.
    fn info(&self) -> &DeviceInfo;

    /// Total capacity in bytes.
    fn capacity(&self) -> u64 {
        self.info().capacity
    }
}
