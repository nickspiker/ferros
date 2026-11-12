//! What modem.rs reaches the hardware thru, as a process: the modem's shared memory directly (EL0 windows, kernel cachemap.rs `MODEM_WINDOWS`: Device, but for the RAW TX ring, Normal non-cacheable, `MODEM_TX_NC`), every register thru its hardware grant (a syscall per access), the PMU thru the kernel's EL3 path, time from the generic timer.

use crate::{S_DMA, S_HW};

static mut GRANTS: u64 = 0;

/// The hardware grants in slot order from `S_HW` (main.rs): each block's address and size, as the kernel's registry has them (process.rs `HARDWARE`).
const BLOCKS: [(usize, usize); 15] = [
    (0x1210_0000, 0x2000),     // pcie0-elbi
    (0x1211_0000, 0x5000),     // pcie0-soc
    (0x1212_0000, 0x5000),     // pcie0-pcs
    (0x1213_0000, 0xD000),     // pcie0-udbg
    (0x1214_0000, 0x3000),     // pcie0-phy
    (0x1240_0000, 0x38_1000),  // pcie0-dbi
    (0x40FF_E000, 0x2000),     // pcie0-cfg
    (0x4000_0000, 0xFF_0000),  // pcie0-mem
    (0x1200_0000, 0x8000),     // cmu-hsi1
    (0x1202_0000, 0x2000),     // hsi1-sysreg
    (0x1204_0000, 0x1000),     // gpio-hsi1
    (0x1206_0000, 0x1_0000),   // sysmmu-hsi1
    (0x1209_0000, 0x1_0000),   // s2mpu-hsi1
    (0x154D_0000, 0x1000),     // gpio-alive
    (0x1546_0000, 0x1_0000),   // pmu (through EL3)
];

/// The shared-memory windows in slot order from `S_DMA`: modem-pktproc, modem-ipc, modem-msi.
pub const WINDOWS: [(usize, usize); 3] = [(0xE800_0000, 0xEA00_0000), (0xEA40_0000, 0xEA80_0000), (0xF620_0000, 0xF640_0000)];

/// Take the grants: every DMA window must be the one the kernel granted, at the address this code uses (a mismatch is a build of the two out of step: fail loud).
pub fn init(grants: u64) {
    unsafe { GRANTS = grants };
    for (k, w) in WINDOWS.iter().enumerate() {
        assert_eq!(ferros_sys::dma(cap(S_DMA + k as u64)) as usize, w.0);
    }
}

pub fn cap(slot: u64) -> u64 {
    unsafe { GRANTS + 0x204 + 32 * slot }
}

/// The grant slot and offset of a register. An address in no granted block is a bug in this driver: fail loud.
fn block(a: usize) -> (u64, u64) {
    let k = BLOCKS.iter().position(|b| a >= b.0 && a < b.0 + b.1).unwrap_or_else(|| panic!("no grant holds register G#{a:X}"));
    (S_HW + k as u64, (a - BLOCKS[k].0) as u64)
}

fn in_window(a: usize) -> bool {
    WINDOWS.iter().any(|w| a >= w.0 && a < w.1)
}

/// The shared-memory windows through their handles (SANDBOX.md, "Handles"): every access `base + (offset & mask)`, the same instructions native and masked. modem.rs computes absolute addresses inside a window; the window is found by address and the handle takes the offset. An address in no window is a bug in this driver: fail loud.
type Pktproc = ferros_sys::Handle<{ S_DMA }>;
type Ipc = ferros_sys::Handle<{ S_DMA + 1 }>;
type Msi = ferros_sys::Handle<{ S_DMA + 2 }>;
fn window(a: usize) -> (usize, u64) {
    let k = WINDOWS.iter().position(|w| a >= w.0 && a < w.1).unwrap_or_else(|| panic!("no window holds G#{a:X}"));
    (k, (a - WINDOWS[k].0) as u64)
}
pub fn win_rd8(a: usize) -> u8 {
    match window(a) { (0, o) => Pktproc::read8(o), (1, o) => Ipc::read8(o), (_, o) => Msi::read8(o) }
}
pub fn win_wr8(a: usize, v: u8) {
    match window(a) { (0, o) => Pktproc::write8(o, v), (1, o) => Ipc::write8(o, v), (_, o) => Msi::write8(o, v) }
}
pub fn win_rd32(a: usize) -> u32 {
    match window(a) { (0, o) => Pktproc::read32(o), (1, o) => Ipc::read32(o), (_, o) => Msi::read32(o) }
}
pub fn win_wr32(a: usize, v: u32) {
    match window(a) { (0, o) => Pktproc::write32(o, v), (1, o) => Ipc::write32(o, v), (_, o) => Msi::write32(o, v) }
}
/// `words` into a window from `a`, the window found once and one 8-byte store a word (`Handle::write_words`).
/// The RAW TX ring is Normal non-cacheable (kernel cachemap.rs `MODEM_TX_NC`, since 2026-11-12): a run of word stores is gathered into whole bursts, and the caller orders them before the head's write with `dsb sy`. The rest of the windows are Device-nGnRnE, where no store is gathered with another and each waits for its write response (the boot channel's 98 MB went a byte a store until 2026-11-11, then a word a store at 26 MB/s for the whole section).
pub fn win_write_words(a: usize, words: &[u64]) {
    match window(a) { (0, o) => Pktproc::write_words(o, words), (1, o) => Ipc::write_words(o, words), (_, o) => Msi::write_words(o, words) }
}
pub fn win_rd64(a: usize) -> u64 {
    match window(a) { (0, o) => Pktproc::read64(o), (1, o) => Ipc::read64(o), (_, o) => Msi::read64(o) }
}
pub fn win_wr64(a: usize, v: u64) {
    match window(a) { (0, o) => Pktproc::write64(o, v), (1, o) => Ipc::write64(o, v), (_, o) => Msi::write64(o, v) }
}

pub fn rd(a: usize) -> u32 {
    if in_window(a) {
        return win_rd32(a);
    }
    let (slot, off) = block(a);
    ferros_sys::mmio_read(cap(slot), off) as u32
}

pub fn wr(a: usize, v: u32) {
    if in_window(a) {
        return win_wr32(a, v);
    }
    let (slot, off) = block(a);
    ferros_sys::mmio_write(cap(slot), off, v);
}

/// The PMU word at `a`: its `mask` bits set to `val`'s, the rest kept (the kernel's EL3 path does the write). Read back: 0 when the bits hold, as the EL3 call answered on success, else the word as it reads (a write EL3 refused shows here).
pub fn priv_rmw(a: u64, mask: u32, val: u32) -> u64 {
    let v = rd(a as usize);
    wr(a as usize, v & !mask | val & mask);
    let now = rd(a as usize);
    if now & mask == val & mask { 0 } else { now as u64 }
}

pub fn now() -> u64 {
    ferros_sys::now()
}

pub fn freq() -> u64 {
    ferros_sys::freq()
}

/// The driver's one lock (the kernel's, in the kernel's copy of this code): the service's state is the main thread's and the `rmnet` thread's (main.rs), each pass and each request under it. Not re-entrant: nothing that holds it calls it again.
static LOCK: ferros_heap::SpinLock<()> = ferros_heap::SpinLock::new(());
pub fn locked<R>(f: impl FnOnce() -> R) -> R {
    let _g = LOCK.lock();
    f()
}

/// A line to the console (the kernel keeps a process's console in its flash log).
pub fn log(s: &str) {
    say(s.as_bytes());
    say(b"\n");
}

/// Bytes to the console, a line at a time within the kernel's 4 KiB a call.
pub fn say(b: &[u8]) {
    for c in b.chunks(1 << 11) {
        ferros_sys::console(unsafe { GRANTS }, c);
    }
}
