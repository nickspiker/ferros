//! eUSB2 repeater TUNE payload — apply the husky DT `repeater_tune*` table and verify by readback.
//!
//! Why: the DT tune table is applied by the Android kernel driver at probe, NOT by ABL.
//! On ferros boots the repeater runs on ABL/POR defaults — 7 of the 8 tuned registers differ from Google's calibrated values (measured 2026-08-16, table in REPEATER.md).
//! An untuned analog eye is the leading suspect for the enumeration coin-flip.
//!
//! Sequence: dump the 8 target registers BEFORE, write each DT value, read each back, dump AFTER with a per-register verify verdict.
//! Writes go only to the positively-identified repeater (REV_ID gate first) — the bus is shared with the PMIC chain, so a failed REV_ID aborts before any write.
//!
//! Uses the hardware-proven `ferros_hal::hsi2c` engine — this payload doubles as the integration test for the extracted HAL module in a relocated-blob context.

#![no_std]
#![no_main]

use ferros_hal::hsi2c::{Hsi2c, PIXEL8_EUSB_REPEATER_ADDR, PIXEL8_HSI2C11_BASE};

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

const REV_ID: u8 = 0xB0;
const EXPECTED_REV: u8 = 0x03;

/// The husky DT tune table (hs_tune_eusb node): (register, value). Mask is G#FF, shift 0 for all — whole-register writes.
const TUNE: [(u8, u8); 8] = [
    (0x50, 0x0A), // eusb_mode_control
    (0x70, 0x3C), // u_tx_adjust_port1
    (0x71, 0x2C), // u_hs_tx_pre_emphasis_p1
    (0x72, 0x90), // u_rx_adjust_port1
    (0x73, 0x83), // u_disconnect_squelch_port1
    (0x77, 0x00), // e_hs_tx_pre_emphasis_p1
    (0x78, 0x0B), // e_tx_adjust_port1
    (0x79, 0x40), // e_rx_adjust_port1
];

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
    fn hex2(&mut self, val: u8) {
        let h = b"0123456789ABCDEF";
        self.put(h[(val >> 4) as usize]);
        self.put(h[(val & 0xF) as usize]);
    }
    fn hex8(&mut self, val: u32) {
        let h = b"0123456789ABCDEF";
        for i in (0..8).rev() {
            self.put(h[((val >> (i * 4)) & 0xF) as usize]);
        }
    }
}

/// Entry — pinned at blob offset 0 by payload.ld.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_inp: *const u8, _in_len: usize, out: *mut u8, out_cap: usize) -> u64 {
    let mut o = Out { ptr: out, cap: out_cap, n: 0 };
    let i2c = Hsi2c::new(PIXEL8_HSI2C11_BASE);
    i2c.init();

    // Safety gate: positively identify the repeater before any write on the shared bus.
    match i2c.read_reg(PIXEL8_EUSB_REPEATER_ADDR, REV_ID) {
        Ok(rev) if rev == EXPECTED_REV => {
            o.s(b"REV_ID ok\n");
        }
        Ok(rev) => {
            o.s(b"REV_ID unexpected: ");
            o.hex2(rev);
            o.s(b" - ABORT, no writes\n");
            return o.n as u64;
        }
        Err(trans) => {
            o.s(b"REV_ID read failed trans=");
            o.hex8(trans);
            o.s(b" - ABORT, no writes\n");
            return o.n as u64;
        }
    }

    // reg: before -> write -> after [verdict]
    let mut all_ok = true;
    for &(reg, val) in TUNE.iter() {
        o.s(b"reg ");
        o.hex2(reg);
        o.s(b": ");
        match i2c.read_reg(PIXEL8_EUSB_REPEATER_ADDR, reg) {
            Ok(before) => {
                o.hex2(before);
            }
            Err(_) => {
                o.s(b"??");
            }
        }
        o.s(b" -> ");
        o.hex2(val);
        match i2c.write_reg(PIXEL8_EUSB_REPEATER_ADDR, reg, val) {
            Ok(()) => {}
            Err(trans) => {
                o.s(b" WRITE FAIL trans=");
                o.hex8(trans);
                o.put(b'\n');
                all_ok = false;
                continue;
            }
        }
        match i2c.read_reg(PIXEL8_EUSB_REPEATER_ADDR, reg) {
            Ok(after) => {
                o.s(b" readback ");
                o.hex2(after);
                if after == val {
                    o.s(b" OK\n");
                } else {
                    o.s(b" MISMATCH\n");
                    all_ok = false;
                }
            }
            Err(trans) => {
                o.s(b" READBACK FAIL trans=");
                o.hex8(trans);
                o.put(b'\n');
                all_ok = false;
            }
        }
    }

    if all_ok {
        o.s(b"TUNE APPLIED + VERIFIED\n");
    } else {
        o.s(b"TUNE INCOMPLETE - see lines above\n");
    }
    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
