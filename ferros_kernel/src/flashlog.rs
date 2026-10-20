//! An append-only plain-text log on the flash, outside the vault, for nailing down vault behaviour across reboots (2026-10-20).
//!
//! Region: the ferros partition (LU 0) from 1 GiB in: block 0 is a header {"FLOG", u32 next data block, u32 bytes used in it, u32 boot count}, then 16383 data blocks of text. Each line is appended into the current block and the block and the header are both written back, so a line is on the flash before `line` returns. When the data blocks run out the log stops (no wrap: nothing is ever silently lost).
//! Lines start with the boot count and the counter time in ms: "b<boot> t<ms> <text>".

use alloc::vec::Vec;
use ferros_hal::ufs::UfsController;

const UFS_BASE: usize = 0x1320_0000;
const BASE: u32 = crate::ufs_device::FERROS_BASE_LBA + 262_144;
const BLOCKS: u32 = 16_384;
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
    if u.read_block_lun(0, lba) != 0 || u.last_response_status() != 0 {
        return false;
    }
    out.copy_from_slice(u.data_buffer());
    true
}

fn write(lba: u32, data: &[u8; 4096]) -> bool {
    let u = ufs();
    u.data_buffer_mut().copy_from_slice(data);
    u.write_block_lun(0, lba) == 0 && u.last_response_status() == 0
}

fn write_header(s: &State) -> bool {
    let mut h = [0u8; 4096];
    h[..4].copy_from_slice(MAGIC);
    h[4..8].copy_from_slice(&s.block.to_le_bytes());
    h[8..12].copy_from_slice(&s.used.to_le_bytes());
    h[12..16].copy_from_slice(&s.boot.to_le_bytes());
    write(BASE, &h)
}

/// Open the log (creating it if the header is missing) and count this boot. Call once, after the UFS link is up.
pub fn init() -> bool {
    let mut h = [0u8; 4096];
    if !read(BASE, &mut h) {
        return false;
    }
    let mut s = if &h[..4] == MAGIC {
        let w = |o: usize| u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]);
        State { block: w(4).clamp(1, BLOCKS), used: w(8).min(4096), boot: w(12) + 1, buf: [0; 4096] }
    } else {
        State { block: 1, used: 0, boot: 1, buf: [0; 4096] }
    };
    if s.used > 0 && !read(BASE + s.block, &mut s.buf) {
        return false;
    }
    let ok = write_header(&s);
    unsafe { STATE = Some(s) };
    ok
}

/// Append one line. Silently does nothing before `init` or once the region is full.
pub fn line(text: &str) {
    let Some(s) = (unsafe { (*(&raw mut STATE)).as_mut() }) else { return };
    let ms = crate::platform::cntpct() / (crate::platform::cntfrq() / 1000);
    let full = alloc::format!("b{} t{} {}\n", s.boot, ms, text);
    for &c in full.as_bytes() {
        if s.used as usize == 4096 {
            if s.block + 1 >= BLOCKS {
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
            write(BASE + s.block, &s.buf);
        }
    }
    write(BASE + s.block, &s.buf);
    write_header(s);
}

/// The last `max` bytes of the log, for `diag log`.
pub fn tail(max: usize) -> Vec<u8> {
    let Some(s) = (unsafe { (*(&raw const STATE)).as_ref() }) else { return b"flash log not open\n".to_vec() };
    let mut out = Vec::new();
    let blocks_back = (max / 4096 + 1) as u32;
    let first = s.block.saturating_sub(blocks_back).max(1);
    let mut b = [0u8; 4096];
    for blk in first..=s.block {
        if blk == s.block {
            out.extend_from_slice(&s.buf[..s.used as usize]);
        } else if read(BASE + blk, &mut b) {
            out.extend_from_slice(&b);
        }
    }
    let skip = out.len().saturating_sub(max);
    out.drain(..skip);
    out
}
