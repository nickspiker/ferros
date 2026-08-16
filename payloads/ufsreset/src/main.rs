//! UFS bring-up with a REAL device reset via the gph5-1 pin (pinctrl G#1306_0000).
//!
//! ufsinit2 proved DME_LINKSTARTUP fails even on pristine ABL cal after a bare HCE cycle: the UFS device still thinks its link is up and ignores link startup — it must be hardware-reset first.
//! HCI_GPIO_OUT only reaches the pin when gph5-1's pinmux routes function 2 (UFS); ABL's leftover state is unknown, so this payload drives the pin directly as a GPIO output (CON=1, DAT toggle), then restores CON.
//! Sequence: dump pin state -> GPIO device reset -> settle -> HCE cycle + vendor config -> DME_LINKSTARTUP retries -> NOP probe.

#![no_std]
#![no_main]

use ferros_hal::ufs::UfsController;
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
const GPH5CON: usize = 0x1306_0000;
const GPH5DAT: usize = 0x1306_0004;
/// PMU (G#1546_0000) + ufs-phy-iso offset (zuma-ufs.dtsi): bit0=1 = isolation BYPASSED (PHY powered).
const PMU_UFS_PHY_ISO: usize = 0x1546_3EC0;
/// GPIO_PERIC0 gpp0 bank — gpp0-1 is the ufs_fixed_vcc regulator enable (active high).
const GPP0CON: usize = 0x1084_0000;
const GPP0DAT: usize = 0x1084_0004;

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

    let con0 = rd(GPH5CON);
    let dat0 = rd(GPH5DAT);
    o.line(b"GPH5CON", con0);
    o.line(b"GPH5DAT", dat0);
    o.line(b"HCS_before", rd(STD + 0x30));

    unlock();

    // Device hardware reset with the pin under OUR control the whole time: gph5-1 -> GPIO output driving HIGH (out of reset, no HCI_GPIO_OUT ambiguity), then a real low pulse, then HIGH again — and LEAVE it GPIO-high so the device's reset line state is definitively known.
    wr(GPH5CON, (con0 & !0xF0) | 0x10);
    wr(GPH5DAT, dat0 | 0x2);
    udelay(1_000);
    wr(GPH5DAT, dat0 & !0x2);
    udelay(100);
    wr(GPH5DAT, dat0 | 0x2);
    udelay(10_000);
    o.line(b"GPH5CON_now", rd(GPH5CON));
    o.line(b"GPH5DAT_now", rd(GPH5DAT));
    o.s(b"dev reset done, pin held GPIO-high\n");

    // THE suspected fix: gph5-0 is ufs_refclk_out, parked by ABL as GPIO-low = device refclk DEAD. A running link tolerates a gated refclk, but the device PHY cannot participate in link startup without it. Route the pad to function 2 so REFCLKOUT actually reaches the device.
    let con1 = rd(GPH5CON);
    wr(GPH5CON, (con1 & !0xF) | 0x2);
    udelay(1_000);
    o.line(b"GPH5CON_refclk", rd(GPH5CON));

    // PHY isolation: if ABL isolated the M-PHY at handoff the analog lanes are unpowered — no cal, reset, or refclk matters. Bit0=1 = bypass (powered), per the kernel's exynos_ufs_ctrl_phy_pwr.
    let iso = rd(PMU_UFS_PHY_ISO);
    o.line(b"PHY_ISO_before", iso);
    if iso & 1 == 0 {
        wr(PMU_UFS_PHY_ISO, iso | 1);
        udelay(1_000);
        o.line(b"PHY_ISO_after", rd(PMU_UFS_PHY_ISO));
    }

    // Device VCC power cycle — the only guaranteed device reset if RST_n is tied inactive on this board (every RST_n pin experiment changed nothing; a UniPro device still in LinkUp ignores link startup entirely). VCCQ stays up (PMIC rail); pulling VCC is the JEDEC power-cycle path Linux itself uses in deep suspend. Device is idle, no writes in flight.
    let pcon = rd(GPP0CON);
    let pdat = rd(GPP0DAT);
    o.line(b"GPP0CON", pcon);
    o.line(b"GPP0DAT", pdat);
    if (pcon >> 4) & 0xF == 1 {
        wr(GPP0DAT, pdat & !0x2);
        udelay(100_000);
        wr(GPP0DAT, pdat | 0x2);
        udelay(200_000);
        o.s(b"VCC power-cycled\n");
    } else {
        o.s(b"gpp0-1 not a GPIO output - VCC untouched\n");
    }

    let mut ls_res = 0xFFFF_FFFFu32;
    let mut dp = false;
    for attempt in 1..=4u32 {
        // Device boot settle — grows per attempt (2ms, 52ms, 102ms, 152ms).
        udelay(2_000 + (attempt as u64 - 1) * 50_000);

        // HCE off.
        wr(STD + 0x34, 0);
        for _ in 0..1_000_000u32 { if rd(STD + 0x34) & 1 == 0 { break; } }

        // Vendor config (ABL's values), no SW_RST — ABL's PCS/PMA cal is intact and proven.
        unlock();
        wr(HCI + 0x60, 0xA);
        wr(HCI + 0x00, (1 << 31) | 12);
        wr(HCI + 0x04, 12);
        wr(HCI + 0x40, 0xFFFF_FFFF);
        wr(HCI + 0x44, 0xFFFF_FFFF);
        wr(HCI + 0x6C, (3 << 27) | 3);

        // HCE on.
        wr(STD + 0x34, 1);
        for _ in 0..1_000_000u32 { if rd(STD + 0x34) & 1 == 1 { break; } }
        unlock();

        // DME_LINKSTARTUP.
        let _ = rd(STD + 0x38);
        wr(STD + 0x20, 0xFFFF_FFFF);
        for _ in 0..1_000_000u32 { if rd(STD + 0x30) & (1 << 3) != 0 { break; } }
        wr(STD + 0x94, 0);
        wr(STD + 0x98, 0);
        wr(STD + 0x9C, 0);
        wr(STD + 0x90, 0x16);
        ls_res = 0xFFFF_FFFF;
        for _ in 0..2_000_000u32 {
            if rd(STD + 0x20) & (1 << 10) != 0 {
                wr(STD + 0x20, 1 << 10);
                ls_res = rd(STD + 0x98) & 0xFF;
                break;
            }
        }
        if ls_res == 0 {
            for _ in 0..2_000_000u32 { if rd(STD + 0x30) & 1 != 0 { dp = true; break; } }
        }
        o.line(b"try", attempt);
        o.line(b"LS_RES", ls_res);
        o.line(b"HCS", rd(STD + 0x30));
        if dp { break; }
    }

    o.line(b"UECPA", rd(STD + 0x38));
    o.line(b"PA_STATE", rd(UNIPRO + 0x15C));
    o.line(b"CONN_RX", rd(UNIPRO + 0x3204));

    if dp {
        let ufs = UfsController::new(STD);
        let p = ufs.probe();
        o.line(b"nop_ocs", p.nop_ocs as u32);
        o.line(b"nop_ok", p.nop_ok as u32);
        o.line(b"geo_ok", p.geo_ok as u32);
        o.line(b"cap_sectors", p.total_raw_capacity_sectors as u32);
        o.s(if p.nop_ok { b"*** DEVICE RESET + LINK UP + NOP OK ***\n" } else { b"link up but NOP fails\n" });
    } else {
        o.s(b"linkstartup still failing after real device reset\n");
    }

    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
