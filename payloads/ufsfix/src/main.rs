//! UFS clean NOP probe — set NEXUS tag-0 bit, then the simplest transfer, NO other poking.
//!
//! Run on a FRESHLY REBOOTED controller (clean ABL init) to decide the core question without my earlier hibernate-exit experiments (which pushed HCS into PWR_FATAL_ERROR): does a NOP OUT complete on a clean controller once NEXUS_TYPE bit0 is set? If yes -> the mechanism works and read_block's remaining failure is SCSI/CDB-specific. If no -> the transfer mechanism needs full re-init (HCE reset + DME_LINKSTARTUP + M-PHY cal).

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

const UFS_BASE: usize = 0x1320_0000;

struct Out { ptr: *mut u8, cap: usize, n: usize }
impl Out {
    fn put(&mut self, b: u8) { if self.n < self.cap { unsafe { *self.ptr.add(self.n) = b; } self.n += 1; } }
    fn s(&mut self, s: &[u8]) { for &b in s { self.put(b); } }
    fn hex(&mut self, v: u32) { let h=b"0123456789ABCDEF"; for i in (0..8).rev() { self.put(h[((v>>(i*4))&0xF) as usize]); } }
    fn line(&mut self, l: &[u8], v: u32) { self.s(l); self.put(b'='); self.hex(v); self.put(b'\n'); }
}
fn rd(a: usize) -> u32 { unsafe { core::ptr::read_volatile(a as *const u32) } }
fn wr(a: usize, v: u32) { unsafe { core::ptr::write_volatile(a as *mut u32, v); core::arch::asm!("dsb sy"); } }

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_i: *const u8, _il: usize, out: *mut u8, cap: usize) -> u64 {
    unsafe extern "C" { static mut __bss_start: u8; static mut __bss_end: u8; }
    unsafe { let mut p = &raw mut __bss_start as *mut u8; let e = &raw mut __bss_end as *mut u8; while p < e { core::ptr::write_volatile(p, 0); p = p.add(1); } }
    let mut o = Out { ptr: out, cap, n: 0 };

    o.line(b"HCS_fresh", rd(UFS_BASE + 0x30));      // expect 010F on a clean controller (UPMCRS=PWR_LOCAL)
    o.line(b"NEXUS_fresh", rd(0x1320_1140));
    wr(0x1320_1140, 0xFFFF_FFFF);                    // set tag-0 nexus bit (the only necessary write)

    let ufs = UfsController::new(UFS_BASE);
    let p = ufs.probe();
    o.line(b"HCE", p.hce);
    o.line(b"nop_ocs", p.nop_ocs as u32);
    o.line(b"nop_rsp", p.nop_rsp as u32);
    o.line(b"nop_ok", p.nop_ok as u32);
    o.line(b"geo_ok", p.geo_ok as u32);
    o.line(b"cap_sectors", p.total_raw_capacity_sectors as u32);
    o.s(if p.nop_ok { b"*** NOP OK on clean controller ***\n" } else { b"NOP fails on clean controller -> needs full re-init\n" });
    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
