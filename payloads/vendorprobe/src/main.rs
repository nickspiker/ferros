//! UFS vendor-region probe (READ ONLY, stage 1) — the decisive test for the storage blocker.
//!
//! Everything else about UFS is proven fine (UFS.md); the controller ignores the doorbell because the Samsung HCI_UTRL_NEXUS_TYPE bit likely isn't set. That register lives in the "reg_hci" vendor region at G#1320_1100, which froze the phone once from a payload that was doing several things at once — never isolated to a single read.
//!
//! This reads ONE vendor register (NEXUS_TYPE @ +0x40 = G#1320_1140) and returns it. With the recoverable watchdog live, if the read hangs the phone auto-resets in ~87s instead of freezing. Outcome:
//! - returns a value -> the vendor region is accessible; the whole "VS hangs" story was a payload misfire, and setting NEXUS_TYPE=FFFFFFFF is likely the entire UFS fix.
//! - hangs (device drops, watchdog recovers) -> the vendor region genuinely stalls on CPU access; chase the APB/UNIPRO gate next.
//!
//! Deliberately reads NOTHING else risky first, so a hang is unambiguously the vendor read.

#![no_std]
#![no_main]

const NEXUS_TYPE: usize = 0x1320_1140;

struct Out { ptr: *mut u8, cap: usize, n: usize }
impl Out {
    fn put(&mut self, b: u8) { if self.n < self.cap { unsafe { *self.ptr.add(self.n) = b; } self.n += 1; } }
    fn s(&mut self, s: &[u8]) { for &b in s { self.put(b); } }
    fn hex(&mut self, v: u32) { let h=b"0123456789ABCDEF"; for i in (0..8).rev() { self.put(h[((v>>(i*4))&0xF) as usize]); } }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_i: *const u8, _il: usize, out: *mut u8, cap: usize) -> u64 {
    let mut o = Out { ptr: out, cap, n: 0 };
    o.s(b"reading NEXUS_TYPE @ G#13201140 ...\n");
    let v = unsafe { core::ptr::read_volatile(NEXUS_TYPE as *const u32) };
    o.s(b"NEXUS_TYPE=");
    o.hex(v);
    o.s(b"\nvendor region READABLE\n");
    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
