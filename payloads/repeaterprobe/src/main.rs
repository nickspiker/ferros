//! eUSB2 repeater probe — bring up HSI2C, read the repeater's REV_ID (G#B0).
//!
//! First bring-up milestone for [REPEATER.md]. A successful REV_ID read proves the HSI2C
//! driver works and the repeater bus is ours; everything after is tuning writes.
//!
//! Runs over the RUN channel against the already-enumerated device — no kernel reflash, no
//! re-rolling enumeration per iteration. Read-only: it reads ABL's HSI2C timing and reads the
//! repeater; it does NOT write the bus config or the repeater until the read path is proven.
//!
//! !!! TWO CONSTANTS BELOW ARE PLACEHOLDERS !!! Fill from the live DT (Android + Magisk root):
//!   HSI2C_BASE  = parent i2c-bus node 'reg' (Exynos HSI2C / USI controller base)
//!   REPEATER_ADDR = eusb-repeater node 'reg' (7-bit I2C address)
//! See REPEATER.md "The two unknowns" for the exact adb commands.

#![no_std]
#![no_main]

// ---- FILL THESE FROM DT (REPEATER.md) ----
const HSI2C_BASE: usize = 0xDEAD_0000; // TODO: parent i2c bus 'reg' base
const REPEATER_ADDR: u8 = 0x00; // TODO: eusb-repeater 'reg' (7-bit)
// ------------------------------------------

const REV_ID: u8 = 0xB0;

// Exynos HSI2C (USI-I2C) register offsets — same layout as i2c-exynos5.
const HSI2C_CTL: usize = 0x00;
const HSI2C_FIFO_CTL: usize = 0x04;
const HSI2C_INT_STATUS: usize = 0x24;
const HSI2C_FIFO_STATUS: usize = 0x30;
const HSI2C_TXDATA: usize = 0x34;
const HSI2C_RXDATA: usize = 0x38;
const HSI2C_CONF: usize = 0x40;
const HSI2C_AUTO_CONF: usize = 0x44;
const HSI2C_TRANS_STATUS: usize = 0x50;
const HSI2C_ADDR: usize = 0x70;

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
    fn hex(&mut self, val: u32) {
        let h = b"0123456789ABCDEF";
        for i in (0..8).rev() {
            self.put(h[((val >> (i * 4)) & 0xF) as usize]);
        }
    }
    fn line(&mut self, label: &[u8], val: u32) {
        self.s(label);
        self.put(b'=');
        self.hex(val);
        self.put(b'\n');
    }
}

fn rd(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((HSI2C_BASE + off) as *const u32) }
}
fn wr(off: usize, v: u32) {
    unsafe {
        core::ptr::write_volatile((HSI2C_BASE + off) as *mut u32, v);
        core::arch::asm!("dsb sy");
    }
}

/// Entry — pinned at blob offset 0 by payload.ld.
///
/// NOTE: the master-transfer sequence below is a SKELETON. It reads back ABL's live HSI2C
/// register state (CTL/CONF/timing) first — that dump alone is useful even before the transfer
/// logic is finalized, since it reveals the controller's calibrated config to reuse. The actual
/// AUTO_CONF read transaction is left as the next iteration once the base/addr are known and the
/// register dump confirms we're talking to the right controller.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_inp: *const u8, _in_len: usize, out: *mut u8, out_cap: usize) -> u64 {
    let mut o = Out { ptr: out, cap: out_cap, n: 0 };

    if HSI2C_BASE == 0xDEAD_0000 || REPEATER_ADDR == 0 {
        o.s(b"HSI2C_BASE / REPEATER_ADDR are placeholders - fill from DT (REPEATER.md)\n");
        return o.n as u64;
    }

    // Step 1 (this iteration): dump ABL's live HSI2C config. Confirms we're on the right
    // controller and gives the calibrated CTL/CONF/timing to reuse instead of recomputing.
    o.line(b"CTL", rd(HSI2C_CTL));
    o.line(b"FIFO_CTL", rd(HSI2C_FIFO_CTL));
    o.line(b"CONF", rd(HSI2C_CONF));
    o.line(b"TRANS_STATUS", rd(HSI2C_TRANS_STATUS));
    o.line(b"INT_STATUS", rd(HSI2C_INT_STATUS));

    // Step 2 (next iteration): master read of REV_ID over AUTO_CONF, then:
    //   o.line(b"REV_ID", rev as u32);
    // Skeleton transfer scaffold (do NOT trust until the config dump above is validated):
    let _ = (HSI2C_ADDR, HSI2C_AUTO_CONF, HSI2C_TXDATA, HSI2C_RXDATA, HSI2C_FIFO_STATUS, REPEATER_ADDR, REV_ID, wr);
    o.s(b"config dumped; REV_ID transfer is next iteration\n");

    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
