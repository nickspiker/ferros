//! Slot-1 NOP test — decides "enable-time NEXUS latching" vs "full re-init needed".
//!
//! NEXUS_TYPE reads G#FFFFFFFE: ABL enabled HCE with tag-0's nexus bit CLEAR but tags 1-31 SET. If NEXUS is latched at HCE-enable, our runtime write to set bit 0 is ignored, and slot 0 can never work — but slot 1 (bit already set) can. This submits a NOP OUT in transfer-list SLOT 1 (ring UTRLDBR bit 1) with a hand-built 2-slot UTRD list, bypassing the single-slot HAL.
//!
//! NOP completes in slot 1 -> latching confirmed; the fix is "use a pre-enabled slot", no controller re-init.
//! NOP also fails -> mechanism is broken for another reason; full HCE reset + link startup + M-PHY cal needed.

#![no_std]
#![no_main]

const UFS: usize = 0x1320_0000;
const IS: usize = 0x20;
const UTRLBA: usize = 0x50;
const UTRLBAU: usize = 0x54;
const UTRLDBR: usize = 0x58;
const UTRLCLR: usize = 0x5C;
const UTRLRSR: usize = 0x60;

#[repr(C, align(1024))]
struct UtrdList { slot: [[u32; 8]; 2] }
#[repr(C, align(128))]
struct Ucd { cmd: [u8; 512], rsp: [u8; 512], prdt: [u32; 4] }

static mut LIST: UtrdList = UtrdList { slot: [[0; 8]; 2] };
static mut UCD: Ucd = Ucd { cmd: [0; 512], rsp: [0; 512], prdt: [0; 4] };

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
    o.line(b"NEXUS", rd(0x1320_1140));

    let ucd_addr = &raw const UCD as usize as u64;
    unsafe {
        // NOP OUT UPIU in UCD, task tag = 1 (matches slot 1).
        UCD.cmd[0] = 0x00; // NOP OUT
        UCD.cmd[3] = 1;    // task tag 1
        // Slot 1 UTRD: native-UFS cmd + interrupt, OCS pre-arm, UCD ptr, response/PRDT offsets.
        LIST.slot[1][0] = (1 << 24) | (1 << 28);
        LIST.slot[1][2] = 0x0000_000F;
        LIST.slot[1][4] = ucd_addr as u32;
        LIST.slot[1][5] = (ucd_addr >> 32) as u32;
        LIST.slot[1][6] = (0x0080 << 16) | 0x0080;
        LIST.slot[1][7] = (0x0100 << 16) | 0x0000;
        core::arch::asm!("dsb sy");
    }
    let list_addr = &raw const LIST as usize as u64;

    // Point the transfer list at our 2-slot list, clear doorbells, run.
    wr(UFS + UTRLRSR, 0);
    wr(UFS + UTRLBA, list_addr as u32);
    wr(UFS + UTRLBAU, (list_addr >> 32) as u32);
    wr(UFS + UTRLCLR, 0x0000_0000); // clear all slot doorbells
    wr(UFS + IS, 0xFFFF_FFFF);
    wr(UFS + UTRLRSR, 1);

    // Ring SLOT 1.
    wr(UFS + UTRLDBR, 1 << 1);
    let mut s = 0u32;
    while rd(UFS + IS) & 1 == 0 && s < 4_000_000 { s += 1; }
    wr(UFS + IS, 1);

    let ocs = unsafe { LIST.slot[1][2] & 0xFF };
    let rsp0 = unsafe { UCD.rsp[0] };
    o.line(b"IS", rd(UFS + IS));
    o.line(b"DBR", rd(UFS + UTRLDBR));
    o.line(b"slot1_OCS", ocs);
    o.line(b"rsp[0]", rsp0 as u32);
    if ocs == 0 && rsp0 == 0x20 {
        o.s(b"*** SLOT-1 NOP OK - NEXUS latching confirmed, use a pre-set slot ***\n");
    } else if ocs == 0 {
        o.s(b"OCS=0 completed (rsp not NOP_IN - check)\n");
    } else {
        o.s(b"slot-1 NOP also fails -> full re-init needed\n");
    }
    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
