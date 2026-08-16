//! Minimal UFS re-enable: HCE cycle + vendor config + DME_LINKSTARTUP, with NO SW_RST, NO device reset, NO recalibration.
//!
//! Tests whether the surviving PHY/PCS calibration (ABL's on a fresh boot, or whatever the kernel's full_init left) plus a bare HCE cycle with NEXUS_TYPE set at enable time is enough for a working link — isolating SW_RST/our-cal as the linkstartup killer.
//! If the link comes up, runs a NOP via the HAL probe path to test the transfer engine.

#![no_std]
#![no_main]

use ferros_hal::ufs::UfsController;

struct NoAlloc;
unsafe impl core::alloc::GlobalAlloc for NoAlloc {
    unsafe fn alloc(&self, _: core::alloc::Layout) -> *mut u8 { core::ptr::null_mut() }
    unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {}
}
#[global_allocator]
static NO_ALLOC: NoAlloc = NoAlloc;

const STD: usize = 0x1320_0000;
const HCI: usize = 0x1320_1100;
const UNIPRO: usize = 0x1328_0000;

struct Out { ptr: *mut u8, cap: usize, n: usize }
impl Out {
    fn put(&mut self, b: u8) { if self.n < self.cap { unsafe { *self.ptr.add(self.n) = b; } self.n += 1; } }
    fn s(&mut self, s: &[u8]) { for &b in s { self.put(b); } }
    fn hex(&mut self, v: u32) { let h = b"0123456789ABCDEF"; for i in (0..8).rev() { self.put(h[((v >> (i * 4)) & 0xF) as usize]); } }
    fn line(&mut self, l: &[u8], v: u32) { self.s(l); self.put(b'='); self.hex(v); self.put(b'\n'); }
}
fn rd(a: usize) -> u32 { unsafe { core::ptr::read_volatile(a as *const u32) } }
fn wr(a: usize, v: u32) { unsafe { core::ptr::write_volatile(a as *mut u32, v); core::arch::asm!("dsb sy"); } }

fn unlock() {
    wr(HCI + 0xB4, rd(HCI + 0xB4) & !0xFF0);
    wr(HCI + 0xB0, rd(HCI + 0xB0) & !0x1F);
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_i: *const u8, _il: usize, out: *mut u8, cap: usize) -> u64 {
    unsafe extern "C" { static mut __bss_start: u8; static mut __bss_end: u8; }
    unsafe { let mut p = &raw mut __bss_start as *mut u8; let e = &raw mut __bss_end as *mut u8; while p < e { core::ptr::write_volatile(p, 0); p = p.add(1); } }
    let mut o = Out { ptr: out, cap, n: 0 };

    o.line(b"HCS_before", rd(STD + 0x30));
    unlock();
    o.line(b"PA_STATE_before", rd(UNIPRO + 0x15C));

    // HCE off.
    wr(STD + 0x34, 0);
    for _ in 0..1_000_000u32 { if rd(STD + 0x34) & 1 == 0 { break; } }
    o.line(b"HCE_off", rd(STD + 0x34));

    // Vendor config (ABL's values), NO SW_RST.
    unlock();
    wr(HCI + 0x60, 0xA);                  // DATA_REORDER
    wr(HCI + 0x00, (1 << 31) | 12);       // TXPRDT prefetch|12
    wr(HCI + 0x04, 12);                   // RXPRDT
    wr(HCI + 0x40, 0xFFFF_FFFF);          // UTRL_NEXUS_TYPE — before HCE 0->1
    wr(HCI + 0x44, 0xFFFF_FFFF);          // UTMRL_NEXUS_TYPE
    wr(HCI + 0x6C, (3 << 27) | 3);        // AXIDMA burst, ABL value

    // HCE on.
    wr(STD + 0x34, 1);
    let mut hce_ok = false;
    for _ in 0..1_000_000u32 { if rd(STD + 0x34) & 1 == 1 { hce_ok = true; break; } }
    o.line(b"HCE_on", hce_ok as u32);
    unlock();

    // Clear stale error state, then DME_LINKSTARTUP.
    let _ = rd(STD + 0x38); // UECPA clear-on-read
    wr(STD + 0x20, 0xFFFF_FFFF);
    let mut ls_res = 0xFFFF_FFFFu32;
    for _ in 0..1_000_000u32 { if rd(STD + 0x30) & (1 << 3) != 0 { break; } } // UCRDY
    wr(STD + 0x94, 0);
    wr(STD + 0x98, 0);
    wr(STD + 0x9C, 0);
    wr(STD + 0x90, 0x16);
    for _ in 0..2_000_000u32 {
        if rd(STD + 0x20) & (1 << 10) != 0 { // UCCS
            wr(STD + 0x20, 1 << 10);
            ls_res = rd(STD + 0x98) & 0xFF;
            break;
        }
    }
    o.line(b"LS_RES", ls_res);

    // Wait for Device Present.
    let mut dp = false;
    for _ in 0..2_000_000u32 { if rd(STD + 0x30) & 1 != 0 { dp = true; break; } }
    o.line(b"HCS_after", rd(STD + 0x30));
    o.line(b"UECPA", rd(STD + 0x38));
    o.line(b"PA_STATE", rd(UNIPRO + 0x15C));
    o.line(b"PA_TX_STATE", rd(UNIPRO + 0x160));
    o.line(b"LS_CNF", rd(UNIPRO + 0x7854));
    o.line(b"CONN_RX", rd(UNIPRO + 0x3204));

    if dp {
        // Transfer engine test: HAL probe = list init + NOP (+ descriptors if NOP works).
        let ufs = UfsController::new(STD);
        let p = ufs.probe();
        o.line(b"nop_ocs", p.nop_ocs as u32);
        o.line(b"nop_ok", p.nop_ok as u32);
        o.line(b"geo_ok", p.geo_ok as u32);
        o.line(b"cap_sectors", p.total_raw_capacity_sectors as u32);
        o.s(if p.nop_ok { b"*** LINK UP + NOP OK (no SW_RST, no recal) ***\n" } else { b"link up but NOP fails\n" });
    } else {
        o.s(b"no device present - linkstartup failed\n");
    }

    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
