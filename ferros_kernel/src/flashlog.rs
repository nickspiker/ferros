//! An append-only plain-text log on the flash, outside the vault, for nailing down vault behaviour across reboots (2026-10-20).
//!
//! Region: the ferros partition (LU 0) from 1 GiB in: block 0 is a header {"FLOG", u32 next data block, u32 bytes used in it, u32 boot count}, then 16383 data blocks of text. Each line is appended into the current block and the block and the header are both written back, so a line is on the flash before `line` returns. When the data blocks run out the log stops (no wrap: nothing is ever silently lost).
//! Lines start with the boot count and the counter time in ms: "b<boot> t<ms> <text>".

use alloc::vec::Vec;
use ferros_hal::ufs::UfsController;

const UFS_BASE: usize = 0x1320_0000;
/// The log's place in the ferros partition (from 1 GiB in, `ufs_device::part`), and its length in blocks.
const OFFSET: u32 = 262_144;
const BLOCKS: u32 = 16_384;
/// Where the log is on this boot: (LU, first block, blocks), set by `init` (the ferros partition) or `init_in` (another place: never a partition the bootloader owns, such as akita's `klog`, main.rs).
static mut REGION: (u8, u32, u32) = (0, 0, 0);
fn lun() -> u8 {
    unsafe { REGION.0 }
}
fn base() -> u32 {
    unsafe { REGION.1 }
}
fn blocks() -> u32 {
    unsafe { REGION.2 }
}
const MAGIC: &[u8; 4] = b"FLOG";

struct State {
    block: u32,
    used: u32,
    boot: u32,
    buf: [u8; 4096],
}
static mut STATE: Option<State> = None;

fn ufs() -> UfsController {
    UfsController::new(UFS_BASE)
}

fn read(lba: u32, out: &mut [u8; 4096]) -> bool {
    let u = ufs();
    if u.read_block_lun(lun(), lba) != 0 || u.last_response_status() != 0 {
        return false;
    }
    out.copy_from_slice(u.data_buffer());
    true
}

fn write(lba: u32, data: &[u8; 4096]) -> bool {
    let u = ufs();
    u.data_buffer_mut().copy_from_slice(data);
    u.write_block_lun(lun(), lba) == 0 && u.last_response_status() == 0
}

fn write_header(s: &State) -> bool {
    let mut h = [0u8; 4096];
    h[..4].copy_from_slice(MAGIC);
    h[4..8].copy_from_slice(&s.block.to_le_bytes());
    h[8..12].copy_from_slice(&s.used.to_le_bytes());
    h[12..16].copy_from_slice(&s.boot.to_le_bytes());
    write(base(), &h)
}

/// Open the log at (LU, first block, blocks): the ferros partition's place (`init`) or another the caller names. Call once, after the UFS link is up.
pub fn init_in(lun: u8, first: u32, blocks: u32) -> bool {
    unsafe { REGION = (lun, first, blocks) };
    open()
}

/// Open the log in the ferros partition, once `ufs_device::locate` found it; no partition (or one too small for it), no log.
pub fn init() -> bool {
    match crate::ufs_device::part() {
        Some((first, blocks)) if blocks >= OFFSET + BLOCKS => init_in(0, first + OFFSET, BLOCKS),
        _ => false,
    }
}

/// Open the log where `REGION` says (creating it if the header is missing) and count this boot.
fn open() -> bool {
    let mut h = [0u8; 4096];
    if !read(base(), &mut h) {
        return false;
    }
    let mut s = if &h[..4] == MAGIC {
        let w = |o: usize| u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]);
        State { block: w(4).clamp(1, blocks()), used: w(8).min(4096), boot: w(12) + 1, buf: [0; 4096] }
    } else {
        State { block: 1, used: 0, boot: 1, buf: [0; 4096] }
    };
    if s.used > 0 && !read(base() + s.block, &mut s.buf) {
        return false;
    }
    let ok = write_header(&s);
    unsafe { STATE = Some(s) };
    ok
}

/// Append one line. Silently does nothing before `init` or once the region is full. Under the kernel lock: the log's blocks share the UFS with the vault, and a line written from the service loop while a syscall on another core read the vault broke that read (2026-10-24: init's spawn of touch failed at boot with "unsealed block", the touch server never started).
pub fn line(text: &str) {
    crate::process::with_lock_held_or(|| line_locked(text))
}

fn line_locked(text: &str) {
    // No flash log (akita, storage-off: board.rs): the line goes to the RAM log instead, which a warm reset keeps and Android shows as the previous boot's console.
    let Some(s) = (unsafe { (*(&raw mut STATE)).as_mut() }) else {
        crate::platform::pstore(alloc::format!("flash log: {text}\n").as_bytes());
        return;
    };
    let ms = crate::platform::cntpct() / (crate::platform::cntfrq() / 1000);
    let full = alloc::format!("b{} t{} {}\n", s.boot, ms, text);
    for &c in full.as_bytes() {
        if s.used as usize == 4096 {
            if s.block + 1 >= blocks() {
                return;
            }
            s.block += 1;
            s.used = 0;
            s.buf = [0; 4096];
        }
        s.buf[s.used as usize] = c;
        s.used += 1;
        // A block is written when it fills, and the tail block after every line (below).
        if s.used as usize == 4096 {
            write(base() + s.block, &s.buf);
        }
    }
    write(base() + s.block, &s.buf);
    write_header(s);
}

/// The last `max` bytes of the log, for `diag log`; under the kernel lock, as `line`.
pub fn tail(max: usize) -> Vec<u8> {
    crate::process::with_lock_held_or(|| tail_locked(max))
}

fn tail_locked(max: usize) -> Vec<u8> {
    let Some(s) = (unsafe { (*(&raw const STATE)).as_ref() }) else { return b"flash log not open\n".to_vec() };
    let mut out = Vec::new();
    let blocks_back = (max / 4096 + 1) as u32;
    let first = s.block.saturating_sub(blocks_back).max(1);
    let mut b = [0u8; 4096];
    for blk in first..=s.block {
        if blk == s.block {
            out.extend_from_slice(&s.buf[..s.used as usize]);
        } else if read(base() + blk, &mut b) {
            out.extend_from_slice(&b);
        }
    }
    let skip = out.len().saturating_sub(max);
    out.drain(..skip);
    out
}
