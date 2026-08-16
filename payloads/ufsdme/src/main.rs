//! DME-layer liveness probe. LINKSTARTUP fails identically with/without SW_RST, with/without recal, with/without device reset — so find out whether the host DME itself answers anything.
//!
//! 1. DME_GET of local attributes (PA_AvailTxDataLanes G#1520, PA_AvailRxDataLanes G#1540) via UICCMD — result 0 means the DME command path works.
//! 2. Direct UNIPRO-register linkstartup (UNIP_DME_LINKSTARTUP_REQ G#7850) — the LK/FW-visible mechanism, bypassing UICCMD.
//! 3. Dump the DME FSM/interrupt/error registers to see where the state machine sits.

#![no_std]
#![no_main]

use ferros_hal::ufs_cal::udelay;

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

/// UIC command via UICCMD; returns (result, arg3) — result G#FFFFFFFF = completion timeout.
fn uic(cmd: u32, a1: u32, a3: u32) -> (u32, u32) {
    for _ in 0..1_000_000u32 { if rd(STD + 0x30) & (1 << 3) != 0 { break; } }
    wr(STD + 0x20, 1 << 10);
    wr(STD + 0x94, a1);
    wr(STD + 0x98, 0);
    wr(STD + 0x9C, a3);
    wr(STD + 0x90, cmd);
    for _ in 0..2_000_000u32 {
        if rd(STD + 0x20) & (1 << 10) != 0 {
            wr(STD + 0x20, 1 << 10);
            return (rd(STD + 0x98) & 0xFF, rd(STD + 0x9C));
        }
    }
    (0xFFFF_FFFF, 0)
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_i: *const u8, _il: usize, out: *mut u8, cap: usize) -> u64 {
    unsafe extern "C" { static mut __bss_start: u8; static mut __bss_end: u8; }
    unsafe { let mut p = &raw mut __bss_start as *mut u8; let e = &raw mut __bss_end as *mut u8; while p < e { core::ptr::write_volatile(p, 0); p = p.add(1); } }
    let mut o = Out { ptr: out, cap, n: 0 };

    unlock();
    o.line(b"HCS", rd(STD + 0x30));
    o.line(b"HCE", rd(STD + 0x34));

    // 1. DME_GET local attributes.
    let (r1, v1) = uic(0x01, 0x1520 << 16, 0);
    o.line(b"GET_availtx_res", r1);
    o.line(b"GET_availtx_val", v1);
    let (r2, v2) = uic(0x01, 0x1540 << 16, 0);
    o.line(b"GET_availrx_res", r2);
    o.line(b"GET_availrx_val", v2);
    // DME_GET of PA_PWRMode shadow and VND debug: TX_HSGEAR? Also read what LINKSTARTUP thinks: VS attr DME state.
    let (r3, v3) = uic(0x01, 0x1571 << 16, 0);
    o.line(b"GET_pwrmode_res", r3);
    o.line(b"GET_pwrmode_val", v3);

    // 2. DME FSM / debug dump before direct linkstartup.
    o.line(b"FORCE_DME_ST", rd(UNIPRO + 0x150));
    o.line(b"AUTO_LS", rd(UNIPRO + 0x158));
    o.line(b"NEXT_DME_ST", rd(UNIPRO + 0x16C));
    o.line(b"ERR_LAYER", rd(UNIPRO + 0x0C0));
    o.line(b"ERR_CODE", rd(UNIPRO + 0x0C4));
    o.line(b"INTR_LSB", rd(UNIPRO + 0x7B00));
    o.line(b"INTR_MSB", rd(UNIPRO + 0x7B04));
    o.line(b"INTR_ERRCODE", rd(UNIPRO + 0x7B20));

    // 3. Direct UNIPRO linkstartup request, bypassing UICCMD.
    wr(UNIPRO + 0x7854, 0); // clear CNF
    wr(UNIPRO + 0x7850, 1); // LINKSTARTUP_REQ
    let mut cnf = 0u32;
    for _ in 0..2_000_000u32 {
        cnf = rd(UNIPRO + 0x7854);
        if cnf != 0 { break; }
    }
    udelay(10_000);
    o.line(b"DIRECT_LS_CNF", cnf);
    o.line(b"HCS_after", rd(STD + 0x30));
    o.line(b"UECPA", rd(STD + 0x38));
    o.line(b"PA_STATE", rd(UNIPRO + 0x15C));
    o.line(b"PA_TX_STATE", rd(UNIPRO + 0x160));
    o.line(b"CONN_RX", rd(UNIPRO + 0x3204));
    o.line(b"INTR_LSB2", rd(UNIPRO + 0x7B00));
    o.line(b"ERR_LAYER2", rd(UNIPRO + 0x0C0));
    o.line(b"ERR_CODE2", rd(UNIPRO + 0x0C4));

    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
