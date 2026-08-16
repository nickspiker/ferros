//! UFS region dump — capture ABL's programmed vendor/UNIPRO/PMA state before we re-init.
//!
//! Stage byte (hex input, default G#1): bit0 = vendor HCI, bit1 = UNIPRO, bit2 = PMA.
//! Before touching UNIPRO/PMA it clears the HCI_FORCE_HCS auto clock-stop enables (MPHY_APBCLK_STOP_EN etc) — the suspected cause of every past vendor-region hang — and restores them after.
//! Run staged (1, then 3, then 7) so a hang localizes to a region; the cluster watchdog recovers a hang in ~87s.

#![no_std]
#![no_main]

struct NoAlloc;
unsafe impl core::alloc::GlobalAlloc for NoAlloc {
    unsafe fn alloc(&self, _: core::alloc::Layout) -> *mut u8 { core::ptr::null_mut() }
    unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {}
}
#[global_allocator]
static NO_ALLOC: NoAlloc = NoAlloc;

const HCI: usize = 0x1320_1100;    // vendor-specified block
const UNIPRO: usize = 0x1328_0000;
const PMA: usize = 0x1320_4000;    // M-PHY (APB behind FORCE_HCS auto-gate)

const FORCE_HCS: usize = HCI + 0xB4;

struct Out { ptr: *mut u8, cap: usize, n: usize }
impl Out {
    fn put(&mut self, b: u8) { if self.n < self.cap { unsafe { *self.ptr.add(self.n) = b; } self.n += 1; } }
    fn s(&mut self, s: &[u8]) { for &b in s { self.put(b); } }
    fn hex(&mut self, v: u32) { let h = b"0123456789ABCDEF"; for i in (0..8).rev() { self.put(h[((v >> (i * 4)) & 0xF) as usize]); } }
    fn line(&mut self, l: &[u8], v: u32) { self.s(l); self.put(b'='); self.hex(v); self.put(b'\n'); }
}
fn rd(a: usize) -> u32 { unsafe { core::ptr::read_volatile(a as *const u32) } }
fn wr(a: usize, v: u32) { unsafe { core::ptr::write_volatile(a as *mut u32, v); core::arch::asm!("dsb sy"); } }

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(inp: *const u8, in_len: usize, out: *mut u8, cap: usize) -> u64 {
    unsafe extern "C" { static mut __bss_start: u8; static mut __bss_end: u8; }
    unsafe { let mut p = &raw mut __bss_start as *mut u8; let e = &raw mut __bss_end as *mut u8; while p < e { core::ptr::write_volatile(p, 0); p = p.add(1); } }
    let mut o = Out { ptr: out, cap, n: 0 };

    let stage = if in_len > 0 { unsafe { *inp } } else { 1 };
    o.line(b"stage", stage as u32);

    if stage & 1 != 0 {
        o.line(b"FORCE_HCS", rd(HCI + 0xB4));
        o.line(b"CLKSTOP_CTRL", rd(HCI + 0xB0));
        o.line(b"US_TO_CNT", rd(HCI + 0x0C));
        o.line(b"TXPRDT", rd(HCI + 0x00));
        o.line(b"RXPRDT", rd(HCI + 0x04));
        o.line(b"DATA_REORDER", rd(HCI + 0x60));
        o.line(b"NEXUS", rd(HCI + 0x40));
        o.line(b"UTMRL_NEXUS", rd(HCI + 0x44));
        o.line(b"AXIDMA_BURST", rd(HCI + 0x6C));
        o.line(b"IOP_ACG", rd(HCI + 0x100));
        o.line(b"GPIO_OUT", rd(HCI + 0x70));
        o.line(b"REFCLK_SEL", rd(HCI + 0x108));
        o.line(b"SW_RST", rd(HCI + 0x50));
        o.line(b"V2P1_CTRL", rd(HCI + 0x8C));
    }

    // Keep every UFS clock running while we look at UNIPRO/PMA: clear all auto stop enables.
    let saved_fhcs = rd(FORCE_HCS);
    if stage & 6 != 0 {
        wr(FORCE_HCS, saved_fhcs & !0xFF0);
        // Also clear any latched actual stops.
        let cs = rd(HCI + 0xB0);
        wr(HCI + 0xB0, cs & !0x1F);
        o.line(b"FHCS_now", rd(FORCE_HCS));
        o.line(b"CLKSTOP_now", rd(HCI + 0xB0));
    }

    if stage & 2 != 0 {
        o.line(b"UNIP_VER", rd(UNIPRO + 0x000));
        o.line(b"UNIP_INFO", rd(UNIPRO + 0x004));
        o.line(b"DBG_PRD", rd(UNIPRO + 0x44));
        o.line(b"AVAIL_TX", rd(UNIPRO + 0x3080));
        o.line(b"AVAIL_RX", rd(UNIPRO + 0x3100));
        o.line(b"ACTIVE_TX", rd(UNIPRO + 0x3180));
        o.line(b"ACTIVE_RX", rd(UNIPRO + 0x3200));
        o.line(b"CONN_TX", rd(UNIPRO + 0x3184));
        o.line(b"CONN_RX", rd(UNIPRO + 0x3204));
        o.line(b"MAXRXHSGEAR", rd(UNIPRO + 0x321C));
        o.line(b"PA_CTRLSTATE", rd(UNIPRO + 0x15C));
        o.line(b"SUITE1", rd(UNIPRO + 0x39A8));
        o.line(b"SUITE2", rd(UNIPRO + 0x39B4));
        o.line(b"LINKSTARTUP_CNF", rd(UNIPRO + 0x7854));
    }

    if stage & 4 != 0 {
        o.line(b"PMA_000", rd(PMA + 0x000));
        o.line(b"PMA_9F4", rd(PMA + 0x9F4));
        o.line(b"PMA_C74", rd(PMA + 0xC74));
        o.line(b"PMA_888", rd(PMA + 0x888));
    }

    if stage & 6 != 0 {
        wr(FORCE_HCS, saved_fhcs);
        o.line(b"FHCS_restored", rd(FORCE_HCS));
    }

    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
