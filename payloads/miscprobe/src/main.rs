//! misc-partition boot-control probe — READ ONLY.
//!
//! Question: does Tensor ABL keep A/B slot metadata in the AOSP-standard `bootloader_control` block (misc partition, offset 2048)?
//! If yes, ferros can mark its own slot successful over UFS and stop ABL's ~7-boot rollback to Android (see REPEATER.md "Operational").
//!
//! Sequence: scan the LUN 0 GPT for the `misc` partition, read its first 4KB block, dump bytes 2048..2080, parse the struct, verify magic (G#42414342 "BCAB") and CRC32 over the first 28 bytes.
//! No writes anywhere. The write payload comes only after this confirms format + location.

#![no_std]
#![no_main]

use ferros_hal::ufs::UfsController;

// A dependency in the hal graph links against alloc; this payload never allocates, so satisfy the linker with a null allocator.
struct NoAlloc;
unsafe impl core::alloc::GlobalAlloc for NoAlloc {
    unsafe fn alloc(&self, _: core::alloc::Layout) -> *mut u8 {
        core::ptr::null_mut()
    }
    unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {}
}
#[global_allocator]
static NO_ALLOC: NoAlloc = NoAlloc;

const UFS_BASE: usize = 0x1320_0000;

struct Out {
    ptr: *mut u8,
    cap: usize,
    n: usize,
}

impl Out {
    fn put(&mut self, b: u8) {
        if self.n < self.cap {
            unsafe { *self.ptr.add(self.n) = b; }
            self.n += 1;
        }
    }
    fn s(&mut self, s: &[u8]) {
        for &b in s {
            self.put(b);
        }
    }
    fn hex2(&mut self, v: u8) {
        let h = b"0123456789ABCDEF";
        self.put(h[(v >> 4) as usize]);
        self.put(h[(v & 0xF) as usize]);
    }
    fn hex8(&mut self, v: u32) {
        for i in (0..4).rev() {
            self.hex2((v >> (i * 8)) as u8);
        }
    }
    fn dec(&mut self, mut v: u64) {
        let mut tmp = [0u8; 20];
        let mut i = 0;
        loop {
            tmp[i] = b'0' + (v % 10) as u8;
            v /= 10;
            i += 1;
            if v == 0 {
                break;
            }
        }
        while i > 0 {
            i -= 1;
            self.put(tmp[i]);
        }
    }
}

/// Standard CRC-32 (IEEE 802.3, reflected, init/xorout G#FFFFFFFF) — what AOSP libboot_control uses.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
fn le64(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Entry — pinned at blob offset 0 by payload.ld.
/// FIRST zeroes this blob's own .bss: objcopy strips it from the flat binary, so statics (the UFS DMA buffers!) otherwise start as DRAM garbage — that was the OCS=G#F failure mode.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_inp: *const u8, _in_len: usize, out: *mut u8, out_cap: usize) -> u64 {
    unsafe extern "C" {
        static mut __bss_start: u8;
        static mut __bss_end: u8;
    }
    unsafe {
        let mut p = &raw mut __bss_start as *mut u8;
        let end = &raw mut __bss_end as *mut u8;
        while p < end {
            core::ptr::write_volatile(p, 0);
            p = p.add(1);
        }
    }

    let mut o = Out { ptr: out, cap: out_cap, n: 0 };
    let ufs = UfsController::new(UFS_BASE);
    if !ufs.link_is_up() {
        o.s(b"UFS link down\n");
        return o.n as u64;
    }

    // Slot recovery: a previous bad submission can leave the slot-0 doorbell stuck (ringing an already-set doorbell is a no-op, so every later command silently never starts). Stop the list, clear slot 0 via UTRLCLR (active-low per-slot), wait for the doorbell to drop, then rebase and restart.
    let rd = |off: usize| unsafe { core::ptr::read_volatile((UFS_BASE + off) as *const u32) };
    let wr = |off: usize, v: u32| unsafe {
        core::ptr::write_volatile((UFS_BASE + off) as *mut u32, v);
    };
    o.s(b"pre: UTRLBA=");
    o.hex8(rd(0x50));
    o.s(b" DBR=");
    o.hex8(rd(0x58));
    o.s(b" IS=");
    o.hex8(rd(0x20));
    o.put(b'\n');

    // DO NOT read the Exynos vendor block (G#1320_1100) from a payload: the access hangs the AP outright (clock/protection gated — froze the device 2026-08-16, physical power-cycle required).
    // The UTRL_NEXUS_TYPE hypothesis for the never-completing commands must be tested from kernel boot code where a hang self-recovers via ABL A/B fallback. See UFS.md.
    if rd(0x58) & 1 != 0 {
        wr(0x60, 0); // UTRLRSR: stop
        wr(0x54, !1u32); // UTRLCLR: clear slot 0 (write 0 in the slot's bit position)
        let mut spins = 0u32;
        while rd(0x58) & 1 != 0 && spins < 4_000_000 {
            spins += 1;
        }
        o.s(b"doorbell recovery: DBR=");
        o.hex8(rd(0x58));
        o.put(b'\n');
    }
    ufs.init_transfer_list();

    // GPT header at LBA 1 (4KB logical blocks on this UFS LUN).
    let ocs = ufs.read_block(1);
    if ocs != 0 {
        o.s(b"GPT header read OCS=");
        o.hex2(ocs);
        o.s(b" IS=");
        o.hex8(rd(0x20));
        o.s(b" DBR=");
        o.hex8(rd(0x58));
        o.s(b" HCS=");
        o.hex8(rd(0x30));
        o.put(b'\n');
        return o.n as u64;
    }
    let hdr = ufs.data_buffer();
    if &hdr[0..8] != b"EFI PART" {
        o.s(b"no EFI PART signature\n");
        return o.n as u64;
    }
    let entries_lba = le64(&hdr[72..80]);
    let num_entries = le32(&hdr[80..84]) as usize;
    let entry_size = le32(&hdr[84..88]) as usize;
    o.s(b"GPT ok entries_lba=");
    o.dec(entries_lba);
    o.s(b" n=");
    o.dec(num_entries as u64);
    o.s(b" esz=");
    o.dec(entry_size as u64);
    o.put(b'\n');
    if entry_size == 0 || entry_size > 4096 {
        o.s(b"bad entry size\n");
        return o.n as u64;
    }

    // Scan entries for the partition named "misc" (UTF-16LE name at entry offset 56). List every name seen so a miss is still informative.
    let per_block = 4096 / entry_size;
    let mut misc_lba: u64 = 0;
    o.s(b"parts:");
    'scan: for blk in 0..(num_entries.div_ceil(per_block)) {
        if ufs.read_block(entries_lba as u32 + blk as u32) != 0 {
            o.s(b" [entry block read fail]");
            break;
        }
        let buf = ufs.data_buffer();
        for i in 0..per_block {
            let e = &buf[i * entry_size..(i + 1) * entry_size];
            if e[0..16].iter().all(|&b| b == 0) {
                break 'scan; // unused entry = end of table
            }
            // Narrow the UTF-16LE name to ASCII (names here are all ASCII).
            let mut name = [0u8; 36];
            let mut nl = 0;
            for c in 0..36 {
                let lo = e[56 + c * 2];
                let hi = e[56 + c * 2 + 1];
                if lo == 0 && hi == 0 {
                    break;
                }
                name[nl] = if hi == 0 { lo } else { b'?' };
                nl += 1;
            }
            o.put(b' ');
            o.s(&name[..nl]);
            if &name[..nl] == b"misc" {
                misc_lba = le64(&e[32..40]);
            }
        }
    }
    o.put(b'\n');

    if misc_lba == 0 {
        o.s(b"misc partition NOT FOUND on LUN0\n");
        return o.n as u64;
    }
    o.s(b"misc first_lba=");
    o.dec(misc_lba);
    o.put(b'\n');

    // bootloader_control lives at misc offset 2048 — inside the first 4KB block.
    if ufs.read_block(misc_lba as u32) != 0 {
        o.s(b"misc block read fail\n");
        return o.n as u64;
    }
    let blk = ufs.data_buffer();
    let bc = &blk[2048..2080];
    o.s(b"raw:");
    for &b in bc {
        o.put(b' ');
        o.hex2(b);
    }
    o.put(b'\n');

    // Parse: slot_suffix[4], magic u32, version u8, packed counts u8, reserved u8[1]... then slot_info.
    o.s(b"suffix=");
    for &b in &bc[0..4] {
        o.put(if (0x20..0x7F).contains(&b) { b } else { b'.' });
    }
    let magic = le32(&bc[4..8]);
    o.s(b" magic=");
    o.hex8(magic);
    o.s(b" version=");
    o.dec(bc[8] as u64);
    let packed = bc[9];
    o.s(b" nb_slot=");
    o.dec((packed & 0x7) as u64);
    o.put(b'\n');
    for slot in 0..2 {
        // slot_metadata is 2 bytes: byte0 = priority:4 | tries_remaining:3 | successful_boot:1 (LSB-first bitfields), byte1 = verity_corrupted:1 | reserved:7.
        let b0 = bc[12 + slot * 2];
        let b1 = bc[12 + slot * 2 + 1];
        o.s(b"slot_");
        o.put(b'a' + slot as u8);
        o.s(b": pri=");
        o.dec((b0 & 0xF) as u64);
        o.s(b" tries=");
        o.dec(((b0 >> 4) & 0x7) as u64);
        o.s(b" successful=");
        o.dec(((b0 >> 7) & 1) as u64);
        o.s(b" verity_corrupt=");
        o.dec((b1 & 1) as u64);
        o.put(b'\n');
    }
    let crc_stored = le32(&bc[28..32]);
    let crc_calc = crc32(&bc[0..28]);
    o.s(b"crc stored=");
    o.hex8(crc_stored);
    o.s(b" calc=");
    o.hex8(crc_calc);
    if magic == 0x4241_4342 && crc_stored == crc_calc {
        o.s(b" -> BOOTLOADER_CONTROL CONFIRMED\n");
    } else {
        o.s(b" -> mismatch: NOT the AOSP block (or different layout)\n");
    }
    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
