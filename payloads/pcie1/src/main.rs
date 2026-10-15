//! The WiFi PCIe driver, as a process: brings up PCIe channel 1 to the BCM4398 following WIFI.md, touching hardware only through the hardware grants it was spawned with (ferros_kernel process.rs, syscalls 4/5), and reports every step through its console.
//!
//! Spawn (grant order matters — slots 1..12):
//!   ferros-bridge spawn pcie1.bin <stage> --name pcie1 --hw pcie1-elbi --hw pcie1-soc --hw pcie1-pcs --hw pcie1-udbg --hw pcie1-phy --hw pcie1-dbi --hw pcie1-cfg --hw pcie1-mem --hw gpio-hsi2 --hw gpio-far-alive --hw cmu-hsi2 --hw pmu
//! Stage (input byte, hex): 00 read-only survey (CMU gates, PMU PHY control, GPIO, ELBI/LTSSM, DBI ID) · 01 + PHY bring-up (WIFI.md §5-8) · 02 + link training (§9) · 03 + config space and the chip id (§10-11) · 04 + dump the chip's NCI enumeration ROM (read-only).
//! Each stage only goes further than the last, so a hang (bus stall → watchdog reset) points at the stage that caused it.

#![no_std]
#![no_main]

use core::arch::asm;

const ELBI: u64 = 1;
const SOC: u64 = 2;
const PCS: u64 = 3;
const UDBG: u64 = 4;
const PHY: u64 = 5;
const DBI: u64 = 6;
const CFG: u64 = 7;
const MEM: u64 = 8;
const GPIO: u64 = 9;
const CMU: u64 = 11;
const PMU: u64 = 12;
/// The entropy right (`--entropy`, after the hardware grants).
const ENTROPY: u64 = 13;
/// The DMA region (`--dma pcie1-dma`, after the entropy right).
const DMA: u64 = 14;
const ERR: u64 = u64::MAX;
/// BAR1's offset inside the pcie1-mem window (stage 6 places it above BAR0's 64 KiB), and its size: the 4 MiB slice of the dongle backplane that cfg G#84 selects.
const BAR1: u64 = 0x40_0000;
const BAR1_SIZE: u32 = 0x40_0000;

struct Drv {
    grants: u64,
    base: u64,
    freq: u64,
    log: [u8; 3000],
    n: usize,
    failed: bool,
    /// The backplane window BAR1 shows now (cfg G#84), so a run of accesses in one window costs one config write.
    win: u32,
}

fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let r: u64;
    unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
    r
}

impl Drv {
    fn cap(&self, slot: u64) -> u64 {
        self.grants + 4 + 32 * slot
    }
    fn rd(&mut self, g: u64, off: u64) -> u32 {
        let v = svc(4, self.cap(g), off, 0, 0, 0);
        if v == ERR {
            self.failed = true;
            self.put(b"!rd ");
            self.hex(g as u32);
            self.put(b"+");
            self.hex(off as u32);
            self.put(b"\n");
        }
        v as u32
    }
    fn wr(&mut self, g: u64, off: u64, v: u32) {
        if svc(5, self.cap(g), off, v as u64, 0, 0) == ERR {
            self.failed = true;
            self.put(b"!wr ");
            self.hex(g as u32);
            self.put(b"+");
            self.hex(off as u32);
            self.put(b"\n");
        }
    }
    fn set(&mut self, g: u64, off: u64, bits: u32) {
        let v = self.rd(g, off);
        self.wr(g, off, v | bits);
    }
    fn clr(&mut self, g: u64, off: u64, bits: u32) {
        let v = self.rd(g, off);
        self.wr(g, off, v & !bits);
    }
    fn now(&self) -> u64 {
        let t: u64;
        unsafe { asm!("isb", "mrs {}, cntvct_el0", out(reg) t) };
        t
    }
    fn us(&self, n: u64) {
        let t0 = self.now();
        while self.now().wrapping_sub(t0) < n * self.freq / 1_000_000 {}
    }
    /// Poll until (reg & mask) == want or `max_us` passes; returns the elapsed microseconds, or None on timeout (logged, not fatal: WIFI.md §13.10).
    fn poll(&mut self, g: u64, off: u64, mask: u32, want: u32, max_us: u64) -> Option<u64> {
        let t0 = self.now();
        loop {
            if self.rd(g, off) & mask == want {
                return Some(self.now().wrapping_sub(t0) * 1_000_000 / self.freq);
            }
            if self.now().wrapping_sub(t0) > max_us * self.freq / 1_000_000 {
                return None;
            }
            self.us(10);
        }
    }
    fn put(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.log.len() {
                self.log[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn hex(&mut self, v: u32) {
        let h = b"0123456789ABCDEF";
        self.put(b"G#");
        for i in (0..8).rev() {
            let c = h[((v >> (i * 4)) & 15) as usize];
            self.put(&[c]);
        }
    }
    fn dec(&mut self, mut v: u32) {
        let mut b = [0u8; 10];
        let mut i = b.len();
        loop {
            i -= 1;
            b[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(b"A#");
        let digits = b;
        self.put(&digits[i..]);
    }
    fn kv(&mut self, k: &[u8], v: u32) {
        self.put(k);
        self.put(b"=");
        self.hex(v);
        self.put(b"\n");
    }
    fn show(&mut self, k: &[u8], g: u64, off: u64) {
        let v = self.rd(g, off);
        self.kv(k, v);
    }
    /// One word of dongle memory (backplane address) through BAR1, moving the window first (dhdpcie_bus_membytes with bar1 switching).
    fn window(&mut self, addr: u32) {
        let w = addr & !(BAR1_SIZE - 1);
        if w != self.win {
            self.wr(CFG, 0x84, w);
            self.win = w;
        }
    }
    fn tcm_rd(&mut self, addr: u32) -> u32 {
        self.window(addr);
        self.rd(MEM, BAR1 + (addr & (BAR1_SIZE - 1)) as u64)
    }
    fn tcm_wr(&mut self, addr: u32, v: u32) {
        self.window(addr);
        self.wr(MEM, BAR1 + (addr & (BAR1_SIZE - 1)) as u64, v);
    }
    /// Write bytes to dongle memory as little-endian words, the last one zero-padded; returns how many words read back different.
    fn tcm_write_verify(&mut self, addr: u32, bytes: &[u8]) -> u32 {
        let words = bytes.len().div_ceil(4);
        let word = |i: usize| {
            let mut w = [0u8; 4];
            for (j, b) in w.iter_mut().enumerate() {
                *b = bytes.get(i * 4 + j).copied().unwrap_or(0);
            }
            u32::from_le_bytes(w)
        };
        for i in 0..words {
            self.tcm_wr(addr + 4 * i as u32, word(i));
        }
        let mut bad = 0;
        for i in 0..words {
            if self.tcm_rd(addr + 4 * i as u32) != word(i) {
                bad += 1;
            }
        }
        bad
    }
    /// Printable bytes of a NUL-terminated string in the DMA region (at most `len`, 512).
    fn text(&mut self, a: u32, len: u32) {
        for k in 0..len.min(512) {
            let c = dbr(a + k);
            if c == 0 {
                break;
            }
            if (0x20..0x7F).contains(&c) {
                self.put(&[c]);
            }
        }
        self.put(b"\n");
    }
    fn flush(&mut self) {
        if self.n > 0 {
            svc(1, self.grants + 4, self.log.as_ptr() as u64 - self.base, self.n as u64, 0xA00, 16);
            self.n = 0;
        }
    }
    fn pmu_rmw(&mut self, off: u64, mask: u32, val: u32) {
        let v = self.rd(PMU, off);
        self.wr(PMU, off, (v & !mask) | (val & mask));
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    let base: u64;
    let freq: u64;
    unsafe {
        asm!("mov {}, x28", out(reg) base);
        asm!("mrs {}, cntfrq_el0", out(reg) freq);
    }
    let stage = if input_len > 0 { unsafe { core::ptr::read_volatile((base + input) as *const u8) } } else { 0 };
    let mut d = Drv { grants, base, freq, log: [0; 3000], n: 0, failed: false, win: u32::MAX };
    let rest: &mut [u8] = if input_len > 1 { unsafe { core::slice::from_raw_parts_mut((base + input + 1) as *mut u8, input_len as usize - 1) } } else { &mut [] };
    run(&mut d, stage, rest);
    d.flush();
    unsafe { asm!("svc #0", in("x0") d.failed as u64, in("x8") 0u64, options(noreturn, nostack)) }
}

fn run(d: &mut Drv, stage: u8, files: &mut [u8]) {
    // ---- stage 0: survey (WIFI.md §2, §3, §9 status registers) ----
    d.put(b"pcie1 stage ");
    d.hex(stage as u32);
    d.put(b"\n");
    for &(name, off) in &[
        (&b"cmu.phy_refclk"[..], 0x201Cu64),
        (b"cmu.dbi_aclk", 0x2088),
        (b"cmu.mstr_aclk", 0x208C),
        (b"cmu.slv_aclk", 0x2090),
        (b"cmu.subctrl_apb", 0x2094),
        (b"cmu.phy_udbg_apb", 0x2098),
        (b"cmu.uasc_dbi_aclk", 0x20EC),
        (b"cmu.uasc_slv_aclk", 0x20F4),
        (b"cmu.qch_apb", 0x3058),
        (b"cmu.qch_axi", 0x3060),
        (b"cmu.pcie_user_mux", 0x0620),
    ] {
        let v = d.rd(CMU, off);
        d.kv(name, v);
    }
    let pmu = d.rd(PMU, 0x3EC8);
    d.kv(b"pmu.phy_ch1", pmu);
    let gcon = d.rd(GPIO, 0x0);
    let gdat = d.rd(GPIO, 0x4);
    d.kv(b"gph3.con", gcon);
    d.kv(b"gph3.dat", gdat);
    let lt = d.rd(ELBI, 0x2C8);
    d.kv(b"elbi.ltssm", lt);
    let id = d.rd(DBI, 0x0);
    d.kv(b"dbi.id", id);
    d.flush();
    if stage < 1 {
        return;
    }

    // ---- stage 1: PHY into a known state, WL_REG_ON, PHY calibration (WIFI.md §5-8) ----
    for off in [0x10u64, 0x14, 0x18] {
        d.wr(ELBI, off, 0); // A1
    }
    d.pmu_rmw(0x3EC8, 1, 1); // A2: PHY isolation release
    d.clr(PHY, 0x204, 0xC); // A3
    for (off, v) in [(0x1044u64, 0x2A), (0x1048, 0xAA), (0x104C, 0xA8), (0x1050, 0x80), (0x185C, 0x0A)] {
        d.wr(PHY, off, v); // A4
    }
    d.us(1);
    d.wr(PHY, 0x208, 0xFF); // A5
    d.us(1);
    d.wr(PHY, 0x580, 0x0A); // A6
    d.wr(PHY, 0x928, 0xAA);
    d.wr(PHY, 0x00C, 0x0A); // A7
    d.us(50);
    d.clr(UDBG, 0xC700, 1); // A8
    d.us(10);
    // B: pins. PERST (gph3-0) output low, WL_REG_ON (gph3-4) output low then high, CLKREQ (gph3-1) function 2 pull-up, dev_wake (gph3-5) input pull-down.
    let mut con = d.rd(GPIO, 0x0);
    con = (con & !0xF) | 0x1;
    con = (con & !(0xF << 4)) | (0x2 << 4);
    con = (con & !(0xF << 16)) | (0x1 << 16);
    con &= !(0xF << 20);
    d.clr(GPIO, 0x4, (1 << 0) | (1 << 4)); // B1: both low before switching to outputs
    d.wr(GPIO, 0x0, con);
    let pud = d.rd(GPIO, 0x8);
    d.wr(GPIO, 0x8, (pud & !((0xF << 4) | (0xF << 16) | (0xF << 20))) | (3 << 4) | (1 << 20));
    let drv = d.rd(GPIO, 0xC);
    d.wr(GPIO, 0xC, (drv & !(0xF | (0xF << 4) | (0xF << 16))) | 3 | (3 << 4));
    d.us(20_000); // WL_REG_ON low ≥ 20 ms (§4)
    d.set(GPIO, 0x4, 1 << 4); // B2: WL_REG_ON high
    d.us(200_000); // B3
    d.wr(SOC, 0x4000, 0x15); // B5
    // C: PHY power-down clear.
    d.set(UDBG, 0xC700, 1);
    d.us(100);
    d.wr(PHY, 0x00C, 0);
    d.wr(PHY, 0x928, 0x55);
    d.wr(PHY, 0x580, 0x02);
    d.wr(PHY, 0x208, 0);
    for off in [0x1044u64, 0x1048, 0x104C, 0x1050, 0x185C] {
        d.wr(PHY, off, 0);
    }
    d.us(10);
    // D: zuma PHY configuration, ch1.
    d.wr(PHY, 0x32C, 0x3); // D1
    d.set(UDBG, 0xC700, 1 << 0); // D2
    d.clr(UDBG, 0xC700, 1 << 1); // D3
    match d.poll(UDBG, 0xC734, 1 << 2, 1 << 2, 100_000) {
        Some(t) => d.kv(b"ext_pll_lock_us", t as u32),
        None => d.put(b"ext_pll_lock TIMEOUT\n"),
    }
    d.clr(SOC, 0x4004, 1 << 2); // D5
    d.set(SOC, 0x4004, 0x30); // D6
    d.wr(ELBI, 0x080, 0x4); // D7
    d.wr(ELBI, 0x3A4, 0xF); // D8
    d.wr(ELBI, 0x3A4, 0xD);
    d.us(10);
    d.wr(ELBI, 0x3A4, 0xF); // D9
    d.us(10);
    for v in [1u32, 0] {
        d.wr(ELBI, 0x1404, v); // D10
        d.wr(ELBI, 0x1408, v);
        d.wr(ELBI, 0x1400, v);
    }
    d.wr(ELBI, 0x3D8, 1); // D11
    const PHYCFG: &[(u64, u32)] = &[
        // D12
        (0x0000, 0x88), (0x001C, 0x66), (0x01F4, 0x00), (0x0514, 0x59), (0x051C, 0x11), (0x062C, 0x0E), (0x0644, 0x22), (0x0688, 0x03), (0x06D4, 0x28), (0x0788, 0x64), (0x078C, 0x64), (0x0790, 0x50), (0x0794, 0x50), (0x0944, 0x05), (0x0948, 0x05), (0x094C, 0x05), (0x0950, 0x05),
        // D13-D16
        (0x0590, 0x02), (0x07F8, 0xB0), (0x0730, 0x08), (0x0344, 0xC0), (0x0040, 0x04), (0x0204, 0x03), (0x02D4, 0x1F), (0x0358, 0x10), (0x0018, 0x01),
        // D17-D18
        (0x0514, 0x5B), (0x0608, 0x0C), (0x060C, 0x0F), (0x0610, 0x0F), (0x0614, 0x0F), (0x0618, 0x0F), (0x0510, 0x80), (0x0688, 0x03), (0x0644, 0x23), (0x0624, 0x11), (0x0630, 0x0F), (0x06D0, 0x53),
        // D19 (lane)
        (0x1140, 0x04), (0x1144, 0x04), (0x1148, 0x04), (0x114C, 0x02), (0x1150, 0), (0x1154, 0), (0x1158, 0), (0x115C, 0), (0x12CC, 0x1C), (0x12DC, 0x6C), (0x130C, 0x29), (0x13B4, 0x2F), (0x1A64, 0x05), (0x1A68, 0x05), (0x1A84, 0x05), (0x1A88, 0x05), (0x1A98, 0), (0x1A9C, 0), (0x1AA8, 0x07), (0x1AB8, 0), (0x1ABC, 0), (0x1AF8, 0x90), (0x1B34, 0x03), (0x1BB0, 0x03), (0x1BB4, 0x03), (0x1BC0, 0x06), (0x1BC4, 0x06), (0x1BE8, 0x01), (0x1BF8, 0x04), (0x1C98, 0), (0x1CA4, 0x81),
        // D20 (BCM)
        (0x1B20, 0x02), (0x1340, 0x23), (0x002C, 0x3C),
    ];
    for &(off, v) in PHYCFG {
        d.wr(PHY, off, v);
    }
    d.set(PHY, 0x1350, 1); // D21
    const PCSCFG: &[(u64, u32)] = &[(0x190, 0x100B_0604), (0x154, 0x0007_00D5), (0x100, 0x1640_0000), (0x104, 0x0860_0000), (0x114, 0x1850_0000), (0x124, 0x6070_0000), (0x174, 0x7), (0x178, 0x100), (0x17C, 0x700), (0x110, 0x0160_0202)];
    for &(off, v) in PCSCFG {
        d.wr(PCS, off, v); // D22-D24
    }
    d.wr(ELBI, 0x1404, 1); // D25
    d.us(10);
    d.wr(ELBI, 0x1408, 1); // D26
    d.wr(ELBI, 0x1400, 1);
    for &(name, off, bit) in &[(&b"pll_lock_us"[..], 0xA80u64, 0u32), (b"cdr_lock_us", 0x15C0, 4), (b"ofs_cal_us", 0x140C, 7)] {
        match d.poll(PHY, off, 1 << bit, 1 << bit, 100_000) {
            Some(t) => d.kv(name, t as u32),
            None => {
                d.put(name);
                d.put(b" TIMEOUT\n");
            }
        }
    }
    d.wr(PHY, 0x32C, 0); // D30
    d.wr(ELBI, 0x1078, 1); // D31
    d.flush();
    if stage < 2 {
        return;
    }

    // ---- stage 2: RC setup, PERST, LTSSM (WIFI.md §9), Gen1 forced (no Gen3 insistence) ----
    d.pmu_rmw(0x3EC8, 0x400, 0x400); // E1
    d.wr(UDBG, 0xC800, 0x421); // E2
    d.wr(PHY, 0x32C, 0); // E3
    d.wr(DBI, 0x890, 0x1_2000); // E4
    d.set(GPIO, 0x4, 1 << 0); // E5: PERST# high
    d.us(16_000); // E6
    d.set(ELBI, 0x3BC, 0x11); // E7
    d.wr(ELBI, 0x3A0, 0x2); // E8
    d.wr(ELBI, 0x074, 1); // E9
    d.clr(ELBI, 0x3A8, 0xFFF); // E10
    d.wr(ELBI, 0x474, 1); // E11
    d.wr(ELBI, 0x1078, 1); // E12
    // E13 dw_pcie_setup_rc, with link speed capped at Gen1.
    d.set(DBI, 0x8BC, 1);
    let v = d.rd(DBI, 0xA0);
    d.wr(DBI, 0xA0, (v & !0xF) | 1);
    let v = d.rd(DBI, 0x7C);
    d.wr(DBI, 0x7C, (v & !0xF) | 1);
    let v = d.rd(DBI, 0x710);
    d.wr(DBI, 0x710, (v & !((1 << 7) | (0x3F << 16))) | (1 << 5) | (1 << 16));
    let v = d.rd(DBI, 0x80C);
    d.wr(DBI, 0x80C, (v & !(0x1F << 8)) | (1 << 8));
    d.wr(DBI, 0x10, 4);
    d.wr(DBI, 0x14, 0);
    let v = d.rd(DBI, 0x3C);
    d.wr(DBI, 0x3C, (v & !(0xFF << 8)) | (1 << 8));
    let v = d.rd(DBI, 0x18);
    d.wr(DBI, 0x18, (v & 0xFF00_0000) | 0x00FF_0100);
    let v = d.rd(DBI, 0x4);
    d.wr(DBI, 0x4, (v & !0xFFFF) | 0x107);
    d.wr(DBI, 0x10, 0);
    let v = d.rd(DBI, 0x8);
    d.wr(DBI, 0x8, (v & 0xFFFF) | (0x0604 << 16)); // class code (16-bit at G#0A)
    d.clr(DBI, 0x8BC, 1);
    // E14 exynos_pcie_setup_rc (Gen1 cap kept).
    d.wr(DBI, 0x8BC, 1);
    d.wr(DBI, 0x0, 0xECED_144D);
    let v = d.rd(DBI, 0x7C);
    d.wr(DBI, 0x7C, (v & !((7 << 15) | 0xF)) | 0x3_8000 | 1);
    d.wr(DBI, 0xB40, 0x1A);
    d.wr(DBI, 0xB44, 0xEA);
    d.wr(DBI, 0x44, 0);
    let v = d.rd(DBI, 0x98);
    d.wr(DBI, 0x98, (v & !0xF) | 5);
    // E15 history buffer.
    d.clr(ELBI, 0xC00, 1 << 2);
    d.wr(ELBI, 0xC04, 0xFFFF_FFFF);
    d.wr(ELBI, 0xC08, 0xFFFF_FFFF);
    d.set(ELBI, 0xC00, 3);
    let id = d.rd(DBI, 0x0);
    d.kv(b"dbi.id", id);
    d.wr(ELBI, 0x054, 1); // E17: LTSSM enable
    let t0 = d.now();
    let mut last = 0xFFu32;
    let mut l0 = false;
    for _ in 0..6000 {
        let s = d.rd(ELBI, 0x2C8) & 0x3F;
        if s != last {
            d.put(b"ltssm ");
            d.hex(s);
            d.put(b"\n");
            last = s;
        }
        if s == 0x11 {
            l0 = true;
            break;
        }
        d.us(10);
    }
    let dt = (d.now().wrapping_sub(t0) * 1_000_000 / d.freq) as u32;
    d.kv(b"ltssm_us", dt);
    let ls = d.rd(DBI, 0x80);
    d.kv(b"dbi.lnkctl_sta", ls);
    d.put(if l0 { b"LINK UP (L0)\n" } else { b"no L0\n" });
    d.flush();
    if !l0 || stage < 3 {
        return;
    }

    // ---- stage 3: iATU, config space, BAR0, chip id (WIFI.md §10-11) ----
    let atu = 0x30_0000u64;
    for (off, v) in [(0x08u64, 0x60FF_E000u32), (0x0C, 0), (0x10, 0x60FF_FFFF), (0x14, 0x0100_0000), (0x18, 0), (0x00, 0x4), (0x04, 0x8000_0000)] {
        d.wr(DBI, atu + off, v);
    }
    for (off, v) in [(0x200u64, 0u32), (0x208, 0x6000_0000), (0x20C, 0), (0x210, 0x60FE_FFFF), (0x214, 0x6000_0000), (0x218, 0), (0x204, 0x8000_0000)] {
        d.wr(DBI, atu + off, v);
    }
    d.wr(ELBI, 0x074, 1);
    let vid = d.rd(CFG, 0x0);
    d.wr(ELBI, 0x074, 0);
    d.kv(b"ep.vendor_device", vid);
    if vid & 0xFFFF != 0x14E4 {
        d.put(b"not Broadcom\n");
        return;
    }
    // BAR0 at the bottom of the window (64-bit), the bridge window around it, memory + bus master on.
    d.wr(CFG, 0x10, 0x6000_0000);
    d.wr(CFG, 0x14, 0);
    d.wr(DBI, 0x20, 0x60F0_6000);
    d.set(CFG, 0x04, 0x6);
    d.wr(CFG, 0x80, 0x1800_0000); // PCI_BAR0_WIN = SI_ENUM_BASE: chipcommon
    let chip = d.rd(MEM, 0x0);
    d.kv(b"chipcommon.chipid", chip);
    d.kv(b"chip", chip & 0xFFFF);
    d.kv(b"rev", (chip >> 16) & 0xF);
    d.flush();
    if stage < 4 {
        return;
    }

    // ---- stage 4: dump the NCI enumeration ROM (read-only) — the chip describes its own cores there (nciutils.c nci_init: ChipCommon EromPtrOffset G#FC → EROM2 base; EROM1/OOBR sit below it). The host parses the dump before any core reset is attempted. ----
    let erom2 = d.rd(MEM, 0xFC);
    d.kv(b"cc.eromptr", erom2);
    let erom1 = erom2 & !0xFFF;
    let oobr = erom2 & !0x1FFF;
    d.kv(b"erom1_base", erom1);
    d.kv(b"oobr_base", oobr);
    d.flush();
    for &(name, base) in &[(&b"oobr"[..], oobr), (b"erom", erom1)] {
        d.wr(CFG, 0x80, base);
        d.put(b"dump ");
        d.put(name);
        d.put(b" ");
        d.hex(base);
        d.put(b"\n");
        // Measure first: time the first reads (a window the chip doesn't decode costs a PCIe completion timeout per read and starves everything else); dump further only if they come back fast.
        let t0 = d.now();
        let probe = d.rd(MEM, 0);
        let us = (d.now().wrapping_sub(t0) * 1_000_000 / d.freq) as u32;
        d.kv(b"first_read", probe);
        d.kv(b"first_read_us", us);
        d.flush();
        let words = if us > 100 || probe == 0xFFFF_FFFF { 0u64 } else { 512 };
        for i in 0..words {
            if i % 8 == 0 {
                d.flush();
                d.hex((i * 4) as u32);
                d.put(b":");
            }
            let v = d.rd(MEM, i * 4);
            d.put(b" ");
            d.hex(v);
            if i % 8 == 7 {
                d.put(b"\n");
            }
        }
        d.flush();
    }
    d.wr(CFG, 0x80, 0x1800_0000);
    if stage < 5 {
        return;
    }

    // ---- stage 5: measure before the firmware download touches anything (read-only on the chip). BAR sizes; the DAR security status (pcie_core.h: secure boot means a signature file is required); the CA7's NCI state: core info and DMP control in the OOBR (nci_core_cflags: OOBR + core offset + 8, bit 5 CPUHALT), reset control in its AMNI wrapper (nci_iscoreup: wrapper + G#140 bit 0 = in reset), the wrapper reached through BAR0's second 4 KiB window (cfg G#70). ----
    d.clr(CFG, 0x04, 0x2);
    for (name, off, restore) in [(&b"bar0"[..], 0x10u64, 0x6000_0000u32), (b"bar1", 0x18, 0)] {
        d.wr(CFG, off, 0xFFFF_FFFF);
        let raw = d.rd(CFG, off);
        d.wr(CFG, off, restore);
        d.put(name);
        d.kv(b".raw", raw);
        d.put(name);
        d.kv(b".size", (!(raw & !0xF)).wrapping_add(1));
    }
    d.set(CFG, 0x04, 0x2);
    d.show(b"cfg.bar1win", CFG, 0x84);
    d.show(b"cfg.bar0win2", CFG, 0x70);
    d.wr(CFG, 0x80, 0x1800_1000);
    d.show(b"pcie.dar_security_status", MEM, 0xA74);
    d.wr(CFG, 0x80, oobr);
    d.show(b"ca7.coreinfo", MEM, 0x240);
    d.show(b"ca7.dmpctrl", MEM, 0x248);
    d.show(b"ca7.dmp+c", MEM, 0x24C);
    d.wr(CFG, 0x80, 0x1800_0000);
    d.flush();
    d.wr(CFG, 0x70, 0x1850_4000);
    let t0 = d.now();
    let id = d.rd(MEM, 0x1100);
    d.kv(b"ca7.amni.first_read_us", (d.now().wrapping_sub(t0) * 1_000_000 / d.freq) as u32);
    d.kv(b"ca7.amni.idm_device_id", id);
    d.flush();
    if id != 0xFFFF_FFFF {
        d.show(b"ca7.amni.reset_control", MEM, 0x1140);
        d.show(b"ca7.amni.reset_status", MEM, 0x1144);
    }
    d.flush();
    // The module's CIS tuples in OTP (dhd_custom_cis.c: 4398 range G#1801_115C..G#1801_11B3; tag G#80 start, G#81 vendor → the VID that names the module NVRAM, vid_naming_table_4398). Read-only, first read timed like the ROM dump.
    d.wr(CFG, 0x80, 0x1801_1000);
    let t0 = d.now();
    let first = d.rd(MEM, 0x15C);
    d.kv(b"otp.cis.first_read_us", (d.now().wrapping_sub(t0) * 1_000_000 / d.freq) as u32);
    d.flush();
    if first != 0xFFFF_FFFF {
        d.put(b"otp.cis:");
        for off in (0x15Cu64..0x1B4).step_by(4) {
            let v = d.rd(MEM, off);
            d.put(b" ");
            d.hex(v);
        }
        d.put(b"\n");
    }
    d.wr(CFG, 0x80, 0x1800_0000);
    d.flush();
    if stage < 6 {
        return;
    }

    // ---- stage 6: the dongle's RAM through BAR1, ARM still halted. RAM size the way si_sysmem_size counts it (sysmem rev 14 ≥ 12: coreinfo ROM banks [31:24], SRAM banks [23:16]; per bank, bankidx G#10 ← index, bankinfo G#40 size = 8 KiB × ([6:0] + 1)); the RAM base is the sysmem's second slave port (G#2A_0000, dhd_pcie.c NCI default). BAR1 (4 MiB) sits above BAR0 (64 KiB) at G#6040_0000; cfg G#84 picks which 4 MiB of the backplane it shows (dhdpcie_setbar1win). Then a pattern round trip in both windows, and the shared-area word at the top of RAM zeroed (dhdpcie_init_shared_addr). ----
    d.wr(CFG, 0x80, 0x1808_8000);
    let ci = d.rd(MEM, 0x0);
    d.kv(b"sysmem.coreinfo", ci);
    let (nrb, nb) = ((ci >> 24) & 0xFF, (ci >> 16) & 0xFF);
    let mut ramsize = 0u32;
    for i in nrb..nrb + nb {
        d.wr(MEM, 0x10, i);
        let bi = d.rd(MEM, 0x40);
        ramsize += 8192 * ((bi & 0x7F) + 1);
    }
    d.wr(CFG, 0x80, 0x1800_0000);
    d.kv(b"ramsize", ramsize);
    const RAM_BASE: u32 = 0x2A_0000;
    d.wr(CFG, 0x18, 0x6040_0000);
    d.wr(CFG, 0x1C, 0);
    d.flush();
    let t0 = d.now();
    let w0 = d.tcm_rd(RAM_BASE);
    d.kv(b"bar1.first_read_us", (d.now().wrapping_sub(t0) * 1_000_000 / d.freq) as u32);
    d.kv(b"ram[base]", w0);
    d.flush();
    if w0 == 0xFFFF_FFFF {
        d.put(b"BAR1 does not decode\n");
        return;
    }
    let top = RAM_BASE + ramsize - 4;
    let v = d.tcm_rd(top);
    d.kv(b"ram[top]", v);
    for (addr, pat) in [(RAM_BASE, 0xA5C3_3C5Au32), (top - 4, 0x5A3C_C3A5)] {
        d.tcm_wr(addr, pat);
        let back = d.tcm_rd(addr);
        d.put(if back == pat { b"roundtrip ok " } else { b"roundtrip BAD " });
        d.hex(addr);
        d.put(b" ");
        d.hex(back);
        d.put(b"\n");
    }
    d.tcm_wr(top, 0);
    let v = d.tcm_rd(top);
    d.kv(b"ram[top] zeroed", v);
    d.show(b"cfg.bar1win", CFG, 0x84);
    d.flush();
    if stage < 7 {
        return;
    }

    // ---- stage 7: download and start the firmware (dhd_pcie.c _dhdpcie_download_firmware with the CA7 path of __dhdpcie_bus_download_state). Input after the stage byte: [u32 LE len][firmware .bin][u32 LE len][NVRAM text]. ----
    let Some((fw, nv, clm, cred)) = split_files(files) else {
        d.put(b"stage 7 wants --input-file firmware --input-file nvram [--input-file clm_blob [--input-file wifi_credential]]\n");
        d.failed = true;
        return;
    };
    d.kv(b"fw.len", fw.len() as u32);
    d.kv(b"nvram.raw_len", nv.len() as u32);
    if fw.len() < 4 || fw.len() as u32 > ramsize {
        d.put(b"firmware does not fit\n");
        d.failed = true;
        return;
    }
    // MSI before the firmware runs: it signals boot-complete with an MSI, and with none programmed the write goes to address 0 and the firmware traps (stage 08's first console). The RC's DesignWare MSI receiver (DBI port logic: MSI_ADDR_LO/HI G#820/G#824, vector enable G#828, mask G#82C, status G#830) terminates writes to MSI_ADDR inside the controller; the EP's MSI capability points at the same address. MSI_TARGET is in the boot ROM region, never DRAM, so a write that ever escaped interception would land nowhere.
    const MSI_TARGET: u32 = 0xF000;
    d.wr(DBI, 0x820, MSI_TARGET);
    d.wr(DBI, 0x824, 0);
    d.wr(DBI, 0x828, 0xFFFF_FFFF);
    d.wr(DBI, 0x82C, 0);
    d.wr(DBI, 0x830, 0xFFFF_FFFF);
    let msi = find_cap(d, 0x05);
    d.kv(b"ep.msi_cap", msi.unwrap_or(0) as u32);
    if let Some(c) = msi {
        let ctl = d.rd(CFG, c) >> 16;
        let is64 = ctl & 0x80 != 0;
        d.wr(CFG, c + 4, MSI_TARGET);
        if is64 {
            d.wr(CFG, c + 8, 0);
        }
        let data_off = if is64 { c + 0xC } else { c + 8 };
        let dv = d.rd(CFG, data_off & !3);
        d.wr(CFG, data_off & !3, dv & 0xFFFF_0000);
        // Message control: enable, one vector (multiple-message enable [6:4] = 0).
        let w = d.rd(CFG, c);
        d.wr(CFG, c, (w & !(0x71 << 16)) | (1 << 16));
        d.show(b"ep.msi_ctl", CFG, c);
    }
    d.flush();
    // Enter: the CA7 must be out of reset and halted (stage 5 found it so after WL_REG_ON; dhd resets it into that state only when it isn't, which this driver doesn't do yet).
    d.wr(CFG, 0x80, oobr);
    let dmp = d.rd(MEM, 0x248);
    d.wr(CFG, 0x80, 0x1800_0000);
    d.wr(CFG, 0x70, 0x1850_4000);
    let in_reset = d.rd(MEM, 0x1140) & 1;
    if dmp & 0x20 == 0 || in_reset != 0 {
        d.kv(b"ca7 not halted+up, dmpctrl", dmp);
        d.failed = true;
        return;
    }
    let ramtop = RAM_BASE + ramsize;
    d.tcm_wr(ramtop - 4, 0);
    // The image at the RAM base; its first word is the reset instruction, written to address 0 last.
    let resetinstr = u32::from_le_bytes([fw[0], fw[1], fw[2], fw[3]]);
    let t0 = d.now();
    let bad = d.tcm_write_verify(RAM_BASE, fw);
    d.kv(b"fw.write_verify_ms", ((d.now().wrapping_sub(t0) * 1000) / d.freq) as u32);
    d.kv(b"fw.bad_words", bad);
    d.flush();
    if bad != 0 {
        d.failed = true;
        return;
    }
    // NVRAM: text → NUL-separated vars (process_nvram_vars), zero-padded to 4 bytes, plus the terminating NUL dhd counts (varsz = len + 1, then ROUNDUP 4) — at the top of RAM below the length token.
    let n = process_nvram_vars(nv);
    let vlen = n.div_ceil(4) * 4;
    let varsize = (vlen + 1).div_ceil(4) * 4;
    let varaddr = ramtop - 4 - varsize as u32;
    let bad = d.tcm_write_verify(varaddr, &nv[..vlen.min(nv.len())]);
    for a in ((varaddr + vlen as u32)..(ramtop - 4)).step_by(4) {
        d.tcm_wr(a, 0);
    }
    let w = (varsize / 4) as u32;
    let token = (!w << 16) | (w & 0xFFFF);
    d.tcm_wr(ramtop - 4, token);
    d.kv(b"nvram.vars_len", n as u32);
    d.kv(b"nvram.addr", varaddr);
    d.kv(b"nvram.bad_words", bad);
    d.kv(b"nvram.token", token);
    // Random seed for the firmware's RNG (dhdpcie_wrt_rnd → a reverse TLV under the vars: 128 bytes, then length (no checksum on this TLV, the firmware crashes on one), then type G#FEEDC0DE). From the session's wairua through the entropy right — never from the host.
    let mut seed = [0u8; 128];
    let off = seed.as_mut_ptr() as u64 - d.base;
    if svc(6, d.cap(ENTROPY), off, 128, 0, 0) != 128 {
        d.put(b"no entropy right or no wairua: refusing to start the firmware without a seed\n");
        d.failed = true;
        return;
    }
    let rnd = varaddr - 8 - 128;
    let bad = d.tcm_write_verify(rnd, &seed);
    d.tcm_wr(rnd + 128, 128);
    d.tcm_wr(rnd + 132, 0xFEED_C0DE);
    d.kv(b"rnd.addr", rnd);
    d.kv(b"rnd.bad_words", bad);
    // Exit: reset instruction at 0, then clear CPUHALT in the CA7's DMP control ("for ARM CA7 it is enough if we clear bit5 in IO DMP ctrl") and wait for it to read back.
    d.tcm_wr(0, resetinstr);
    d.kv(b"resetinstr", resetinstr);
    let v = d.tcm_rd(0);
    d.kv(b"ram[0]", v);
    d.flush();
    d.wr(CFG, 0x80, oobr);
    let v = d.rd(MEM, 0x248) & !0x20;
    d.wr(MEM, 0x248, v);
    let rel = d.poll(MEM, 0x248, 0xFFFF_FFFF, v, 5000);
    d.kv(b"ca7.released_us", rel.map(|u| u as u32).unwrap_or(u32::MAX));
    d.wr(CFG, 0x80, 0x1800_0000);
    d.flush();
    // The firmware boots and replaces the length token in the last RAM word with the address of its shared area (dhdpcie_readshared). Watch it for up to 4 s.
    let t0 = d.now();
    let mut last = token;
    let shared = loop {
        let v = d.tcm_rd(ramtop - 4);
        if v != last {
            d.kv(b"ramtop changed", v);
            d.kv(b"  at_ms", ((d.now().wrapping_sub(t0) * 1000) / d.freq) as u32);
            last = v;
        }
        if v != token && v != 0 && v >= RAM_BASE && v < ramtop {
            break Some(v);
        }
        if d.now().wrapping_sub(t0) > 4 * d.freq {
            break None;
        }
        d.us(1000);
    };
    d.flush();
    let Some(sh) = shared else {
        d.kv(b"no shared area; ramtop word", last);
        d.wr(CFG, 0x80, oobr);
        d.show(b"ca7.dmpctrl", MEM, 0x248);
        d.wr(CFG, 0x80, 0x1800_0000);
        d.flush();
        return;
    };
    d.kv(b"FIRMWARE UP, shared", sh);
    d.show(b"rc.msi_status", DBI, 0x830);
    d.put(b"shared:");
    for i in 0..24 {
        let v = d.tcm_rd(sh + 4 * i);
        d.put(b" ");
        d.hex(v);
    }
    d.put(b"\n");
    d.flush();
    if stage < 8 {
        return;
    }

    // ---- stage 8: what the running firmware says about itself. The settled shared area (bcmpcie.h pciedev_shared_t: flags, trap_addr, assert_exp/file, assert_line, console_addr at +G#14, fwid, ..., rings_info_ptr at +G#30) once its pointers are filled in; trap_addr non-zero means the firmware trapped. Then its console: hnd_cons_t at console_addr, log ring at +8 (buf, buf_size, idx) — the firmware's own boot log, printed as text. ----
    // Timeline first (stage 8's first run saw the shared flags and fwid go to zero within 2 s, console never set): every 5 ms for 3 s, log any change in shared flags / console_addr, the ramtop word and the CA7's DMP control. Not backplane G#0: once booted the firmware disables that region, and a host read there through BAR1 is an AXI slave error the firmware traps on (seen: "AXI Slave Err on s_pcie ... addr 0", then a FIQ trap).
    let t0 = d.now();
    let mut prev = [u32::MAX; 4];
    while d.now().wrapping_sub(t0) < 3 * d.freq {
        let mut cur = [d.tcm_rd(sh), d.tcm_rd(sh + 0x14), d.tcm_rd(ramtop - 4), 0];
        d.wr(CFG, 0x80, oobr);
        cur[3] = d.rd(MEM, 0x248);
        d.wr(CFG, 0x80, 0x1800_0000);
        if cur != prev {
            d.hex(((d.now().wrapping_sub(t0) * 1_000_000) / d.freq) as u32);
            d.put(b"us flags/console/ramtop/dmp:");
            for v in cur {
                d.put(b" ");
                d.hex(v);
            }
            d.put(b"\n");
            prev = cur;
            if d.n > 2400 {
                d.flush();
            }
        }
        d.us(5000);
    }
    // The first ramtop value is an early, temporary one: around 430 ms in, the firmware publishes the real shared area there (dhd never sees the first — it reads after the firmware-boot interrupt, dhdpcie_wait_readshared_area_addr).
    let sh = d.tcm_rd(ramtop - 4);
    d.kv(b"shared (settled)", sh);
    d.show(b"rc.msi_status", DBI, 0x830);
    if sh >= RAM_BASE && sh < ramtop {
        // bcmpcie.h: PCIE_SHARED_ASSERT G#200, PCIE_SHARED_TRAP G#400.
        let f = d.tcm_rd(sh);
        d.put(if f & 0x600 != 0 { b"FIRMWARE TRAPPED/ASSERTED\n" } else { b"firmware running, no trap\n" });
    }
    if sh < RAM_BASE || sh >= ramtop {
        d.flush();
        return;
    }
    for (k, off) in [(&b"sh.flags"[..], 0u32), (b"sh.trap_addr", 4), (b"sh.assert_line", 0x10), (b"sh.console_addr", 0x14), (b"sh.fwid", 0x1C), (b"sh.h2d_mb", 0x28), (b"sh.d2h_mb", 0x2C), (b"sh.rings_info", 0x30)] {
        let v = d.tcm_rd(sh + off);
        d.kv(k, v);
    }
    d.flush();
    let cons = d.tcm_rd(sh + 0x14);
    if cons < RAM_BASE || cons >= ramtop {
        return;
    }
    let (buf, size, idx) = (d.tcm_rd(cons + 8), d.tcm_rd(cons + 12), d.tcm_rd(cons + 16));
    d.kv(b"cons.buf", buf);
    d.kv(b"cons.size", size);
    d.kv(b"cons.idx", idx);
    d.flush();
    if buf < RAM_BASE || size == 0 || size > 0x1_0000 || buf + size > ramtop || idx > size {
        return;
    }
    // Unwrapped start (idx is where the next byte goes): oldest first when the ring has wrapped, i.e. the byte after idx is non-zero.
    let wrapped = d.tcm_rd(buf + (idx & !3) + 4) != 0 && idx + 8 < size;
    let (start, len) = if wrapped { (idx, size) } else { (0, idx) };
    console_dump(d, buf, size, start, len);
    if stage < 9 {
        return;
    }

    // ---- stage 9: the msgbuf rings and the first ioctl (dhd_msgbuf.c dhd_prot_init order; WIFI.md §14). Read/write indices stay in TCM (no DMA'd index arrays), so every completion item is checked by epoch and XOR (D2H_SYNC_XORCSUM). Everything the chip may DMA lives in the pcie1-dma region, inside the S2MPU fence. ----
    let dma = svc(7, d.cap(DMA), 0, 0, 0, 0);
    if dma == ERR {
        d.put(b"no DMA region (--dma pcie1-dma)\n");
        d.failed = true;
        return;
    }
    let dma = dma as u32;
    d.kv(b"dma.base", dma);
    // DMA region layout: the five common rings, the 8-byte scratch, the ioctl request buffer, 8 ioctl response buffers (8 KiB), 8 event buffers (2 KiB).
    const RINGS: [(u32, u16, u16); 5] = [(0x0_0000, 64, 40), (0x0_1000, 512, 32), (0x0_5000, 64, 24), (0x0_6000, 1024, 24), (0x0_C000, 512, 40)];
    const SCRATCH: u32 = 0x1_1000;
    const IOCTL_REQ: u32 = 0x1_3000;
    const IOCTL_RESP: u32 = 0x2_0000;
    const EVENT_BUF: u32 = 0x3_0000;
    for off in (0..0x4_0000u32).step_by(4) {
        dw(dma + off, 0);
    }
    let ri = d.tcm_rd(sh + 48);
    let ringmem = d.tcm_rd(ri);
    let (h2d_w, h2d_r, d2h_w, d2h_r) = (d.tcm_rd(ri + 4), d.tcm_rd(ri + 8), d.tcm_rd(ri + 12), d.tcm_rd(ri + 16));
    d.kv(b"ri.ringmem", ringmem);
    d.kv(b"ri.h2d_w", h2d_w);
    d.kv(b"ri.d2h_w", d2h_w);
    let v = d.tcm_rd(ri + 52);
    d.kv(b"ri.max_flowrings|max_subq", v);
    let v = d.tcm_rd(ri + 56);
    d.kv(b"ri.max_cplrings|max_vdevs", v);
    // host_cap: IPC 9, HOSTRDY, deep sleep off (NO_OOB_DW without INBAND_DW), no trap on a PCIe Unsupported Request.
    d.tcm_wr(sh + 84, 0x0080_1409);
    d.tcm_wr(sh + 112, 0);
    for (i, &(off, items, len)) in RINGS.iter().enumerate() {
        let m = ringmem + 16 * i as u32;
        let id = d.tcm_rd(m);
        d.put(b"ring ");
        d.hex(i as u32);
        d.kv(b" idx|type", id);
        d.tcm_wr(m + 4, items as u32 | (len as u32) << 16);
        d.tcm_wr(m + 8, dma + off);
        d.tcm_wr(m + 12, 0);
    }
    for i in 0..2u32 {
        d.tcm_wr(h2d_w + 4 * i, 0);
        d.tcm_wr(h2d_r + 4 * i, 0);
    }
    for i in 0..3u32 {
        d.tcm_wr(d2h_w + 4 * i, 0);
        d.tcm_wr(d2h_r + 4 * i, 0);
    }
    d.tcm_wr(sh + 56, dma + SCRATCH);
    d.tcm_wr(sh + 60, 0);
    d.tcm_wr(sh + 52, 8);
    d.flush();
    // Hostready: H2D doorbell 1 (PCIe core registers at BAR0 + G#2000; DB0 G#140, DB1 G#144).
    unsafe { core::arch::asm!("dsb sy") };
    d.wr(MEM, 0x2144, 0x1234_5678);
    let cons_before = d.tcm_rd(cons + 16);
    let t0 = d.now();
    let rx_off = d.tcm_rd(sh + 36);
    let mut m = Msgbuf { dma, h2d_w, d2h_w, d2h_r, w: 0, seq: 254, rd: 0, dseq: 254, trans: 0, verbose: stage < 11, t0, rx_off, scan_done: false, seen: [[0; 6]; 96], nseen: 0, want: [0; 32], want_len: 0, target: None, linked: false, keyed: false, join_failed: false, rxp_w: 0, rxp_seq: 254, rx_rd: 0, rx_dseq: 254, rx_frames: 0, flow_status: None, tx_w: 0, tx_rd: 0, tx_dseq: 254, tx_done: 0, tx_status: 0, mac: [0; 6], xid: 0, offer: None, acked: false, nak: false, lease: [[0; 4]; 3], lease_s: 0 };
    // Control submit ring: 8 ioctl response buffers and 8 event buffers first (the firmware DMAs ioctl results into a posted response buffer, named by its request id in the completion).
    for b in 0..8u32 {
        m.post(d, 0x0B, 1 + b, [8192, 0, dma + IOCTL_RESP + 0x2000 * b, 0, 0, 0, 0, 0]);
    }
    for b in 0..8u32 {
        m.post(d, 0x0D, 0x101 + b, [2048, 0, dma + EVENT_BUF + 0x800 * b, 0, 0, 0, 0, 0]);
    }
    m.ring(d);
    match m.get_var(d, b"ver", dma + IOCTL_REQ) {
        Some((0, b, len)) => {
            d.put(b"ver: ");
            d.text(dma + IOCTL_RESP + 0x2000 * b, len);
        }
        r => d.kv(b"ver failed, status", r.map(|r| r.0).unwrap_or(u32::MAX)),
    }
    d.flush();
    if stage >= 10 {
        // ---- stage 10: a second query the normal way, then the WiFi chip's S2MPU VID, measured: an ioctl whose request buffer sits outside the fence (G#8008_0000: the kernel image, inside the window the first fence allowed). The firmware's DMA read of it is denied (DECERR) and the S2MPU latches the fault with the VID, which `bridge diag` reports (S2MPU_FAULT_INFO bits 26:24). A denied read can't touch memory; the firmware may trap. ----
        match m.get_var(d, b"cur_etheraddr", dma + IOCTL_REQ) {
            Some((0, _, len)) => d.kv(b"cur_etheraddr len", len),
            r => d.kv(b"cur_etheraddr failed, status", r.map(|r| r.0).unwrap_or(u32::MAX)),
        }
    }
    if stage == 10 {
        d.put(b"probe: ioctl request buffer at G#80080000 (the kernel image, outside the fence)\n");
        d.flush();
        let r = m.get_var(d, b"ver", 0x8008_0000);
        d.kv(b"probe result status", r.map(|r| r.0).unwrap_or(u32::MAX));
        let f = d.tcm_rd(sh);
        d.kv(b"sh.flags (G#400 = trapped)", f);
    }
    if stage >= 11 {
        if let Some((ssid, _)) = credential(cred) {
            m.want[..ssid.len()].copy_from_slice(ssid);
            m.want_len = ssid.len();
        }
        scan(d, &mut m, clm);
        if stage >= 12 {
            join(d, &mut m, cred);
        }
        if stage >= 13 && m.keyed {
            dhcp(d, &mut m);
        }
    }
    let f = d.tcm_rd(sh);
    d.kv(b"sh.flags", f);
    let idx = d.tcm_rd(cons + 16);
    console_dump(d, buf, size, cons_before, (idx + size - cons_before) % size);
}

/// The control path of msgbuf (dhd_msgbuf.c): H2D control submit (ring 0, 64 × 40 B) and D2H control complete (ring 2, 64 × 24 B) in the DMA region, indices in TCM.
struct Msgbuf {
    dma: u32,
    h2d_w: u32,
    d2h_w: u32,
    d2h_r: u32,
    /// H2D control submit write index and sequence (epoch = seq % 253, seq from 254; flags = phase G#80 on the first lap).
    w: u32,
    seq: u32,
    /// D2H control complete read index and expected sequence.
    rd: u32,
    dseq: u32,
    trans: u32,
    /// Print every completion item (bring-up stages) or only what matters (the scan).
    verbose: bool,
    t0: u64,
    /// pcie_shared dma_rxoffset: where a packet starts inside a posted buffer.
    rx_off: u32,
    scan_done: bool,
    seen: [[u8; 6]; 96],
    nseen: usize,
    /// The network to join (stage 12): its SSID from the credential file, and the best matching BSS the scan found (WPA2-PSK, not 6 GHz).
    want: [u8; 32],
    want_len: usize,
    target: Option<Target>,
    /// Join progress from events: link up (16), keyed (46 with status 6 or 10, reason 0), or a failure.
    linked: bool,
    keyed: bool,
    join_failed: bool,
    /// H2D rxpost (ring 1, 512 × 32 B at +G#1000, index slot 1) write index and sequence; D2H rx complete (ring 4, 512 × 40 B at +G#C000, index slot 2) read index and sequence; frames seen.
    rxp_w: u32,
    rxp_seq: u32,
    rx_rd: u32,
    rx_dseq: u32,
    rx_frames: u32,
    /// Transmit (stage 13): flowring create status (MSG_TYPE_FLOW_RING_CREATE_CMPLT G#04), flowring write index and packets, D2H tx complete (ring 3, 1024 × 24 B at +G#6000, index slot 1) read index, sequence, count and last status.
    flow_status: Option<u32>,
    tx_w: u32,
    tx_rd: u32,
    tx_dseq: u32,
    tx_done: u32,
    tx_status: u32,
    mac: [u8; 6],
    /// DHCP: transaction id, the offer (address, server), the ack and what it carried (subnet, router, DNS, lease seconds).
    xid: u32,
    offer: Option<([u8; 4], [u8; 4])>,
    acked: bool,
    nak: bool,
    lease: [[u8; 4]; 3],
    lease_s: u32,
}

#[derive(Clone, Copy)]
struct Target {
    bssid: [u8; 6],
    chanspec: u32,
    rssi: i16,
    /// The AP's RSN group cipher suite (OUI + type) and whether it can do management frame protection (RSN capabilities bit 7).
    group: [u8; 4],
    mfpc: bool,
    /// Control channel (bss_info +88) and whether it is 5 GHz.
    ctl: u8,
    five: bool,
}

impl Msgbuf {
    /// Write one control submit item (cmn_msg_hdr + 32 bytes of body); `ring` publishes.
    fn post(&mut self, _d: &mut Drv, ty: u32, req: u32, body: [u32; 8]) {
        let phase = if (self.w / 64) % 2 == 0 { 0x80 } else { 0 };
        let a = self.dma + 40 * (self.w % 64);
        dw(a, ty | phase << 16 | (self.seq % 253) << 24);
        dw(a + 4, req);
        for (k, v) in body.iter().enumerate() {
            dw(a + 8 + 4 * k as u32, *v);
        }
        self.seq += 1;
        self.w += 1;
    }
    /// Publish posted items: the TCM write index, then doorbell 0 (BAR0 + G#2140).
    fn ring(&mut self, d: &mut Drv) {
        unsafe { core::arch::asm!("dsb sy") };
        d.tcm_wr(self.h2d_w, self.w % 64);
        d.wr(MEM, 0x2140, 0xFF00_0000 | (self.w % 64));
    }
    /// Post receive buffer `i` (2 KiB at +G#4_0000 + i × G#800) on the rxpost ring: host_rxbuf_post_t — header (type G#11, request id = G#1000 + i), u16 metadata length 0, u16 data length, rsvd, metadata address 0, data address.
    fn post_rx(&mut self, i: u32) {
        let phase = if (self.rxp_w / 512) % 2 == 0 { 0x80 } else { 0 };
        let a = self.dma + 0x1000 + 32 * (self.rxp_w % 512);
        let buf = self.dma + 0x4_0000 + 0x800 * i;
        let words = [0x11 | phase << 16 | (self.rxp_seq % 253) << 24, 0x1000 + i, 2048 << 16, 0, 0, 0, buf, 0];
        for (k, v) in words.iter().enumerate() {
            dw(a + 4 * k as u32, *v);
        }
        self.rxp_seq += 1;
        self.rxp_w += 1;
    }
    /// Publish posted rx buffers: rxpost write index (H2D slot 1), doorbell 0 for ring 1.
    fn ring_rx(&mut self, d: &mut Drv) {
        unsafe { core::arch::asm!("dsb sy") };
        d.tcm_wr(self.h2d_w + 4, self.rxp_w % 512);
        d.wr(MEM, 0x2140, 0xFF00_0000 | 1 << 16 | (self.rxp_w % 512));
    }
    /// Consume rx completions (host_rxbuf_cmpl_t, type G#12: request id = buffer, data_len +14, data_offset +16): log length and EtherType, re-post the buffer.
    fn pump_rx(&mut self, d: &mut Drv) {
        let wi = d.tcm_rd(self.d2h_w + 8);
        while self.rx_rd != wi {
            let a = self.dma + 0xC000 + 40 * self.rx_rd;
            let words: [u32; 10] = core::array::from_fn(|k| dr(a + 4 * k as u32));
            if words[0] >> 24 != self.rx_dseq % 253 || words.iter().fold(0, |x, v| x ^ v) != 0 {
                break;
            }
            self.rx_dseq += 1;
            self.rx_rd = (self.rx_rd + 1) % 512;
            d.tcm_wr(self.d2h_r + 8, self.rx_rd);
            let i = words[1].wrapping_sub(0x1000);
            let len = words[3] >> 16;
            // dhd: the frame starts at data_offset if non-zero, otherwise at pcie_shared dma_rxoffset (8 here), measured from the posted buffer; it is a plain Ethernet frame.
            let off = if words[4] & 0xFFFF != 0 { words[4] & 0xFFFF } else { self.rx_off };
            self.rx_frames += 1;
            if i < 64 {
                let p = self.dma + 0x4_0000 + 0x800 * i + off;
                if be16(p + 12) == 0x0800 {
                    self.ipv4(d, p + 14, len.saturating_sub(off).saturating_sub(14));
                }
                d.put(b"rx frame len ");
                d.dec(len);
                d.put(b" ethertype ");
                d.hex(be16(p + 12));
                d.put(b"\n");
                if false {
                    d.put(b"  cmpl:");
                    for v in words {
                        d.put(b" ");
                        d.hex(v);
                    }
                    d.put(b"\n  buf:");
                    let b0 = self.dma + 0x4_0000 + 0x800 * i;
                    for k in 0..8 {
                        d.put(b" ");
                        d.hex(dr(b0 + 4 * k));
                    }
                    d.put(b"\n");
                }
                self.post_rx(i);
                self.ring_rx(d);
            }
        }
    }
    /// An IPv4 packet: if it is UDP to port 68 carrying a BOOTP reply with our transaction id, take the DHCP message (options after the 240-byte BOOTP header + magic cookie): 53 type (2 OFFER, 5 ACK, 6 NAK), 54 server, 1 subnet, 3 router, 6 DNS, 51 lease.
    fn ipv4(&mut self, d: &mut Drv, ip: u32, len: u32) {
        let ihl = (dbr(ip) & 0xF) as u32 * 4;
        if dbr(ip) >> 4 != 4 || dbr(ip + 9) != 17 || len < ihl + 8 + 240 {
            return;
        }
        let udp = ip + ihl;
        if be16(udp + 2) != 68 {
            return;
        }
        let b = udp + 8;
        if dbr(b) != 2 || be32(b + 4) != self.xid {
            return;
        }
        let yiaddr: [u8; 4] = core::array::from_fn(|k| dbr(b + 16 + k as u32));
        let (mut ty, mut server) = (0u8, [0u8; 4]);
        let mut o = b + 240;
        // Bounded by the IP header's total length (the rx completion's length doesn't cover the whole packet the way it was read here: the options walk stopped early).
        let end = ip + (be16(ip + 2)).min(len.max(be16(ip + 2)));
        while o < end {
            let code = dbr(o);
            if code == 255 {
                break;
            }
            if code == 0 {
                o += 1;
                continue;
            }
            let l = dbr(o + 1) as u32;
            let v4: [u8; 4] = core::array::from_fn(|k| dbr(o + 2 + k as u32));
            match code {
                53 => ty = dbr(o + 2),
                54 => server = v4,
                1 => self.lease[0] = v4,
                3 => self.lease[1] = v4,
                6 => self.lease[2] = v4,
                51 => self.lease_s = u32::from_be_bytes(v4),
                _ => {}
            }
            o += 2 + l;
        }
        // One put per arm: a `match` yielding string slices compiles to a table of absolute pointers in .rodata, which a payload loaded at its arc base (no relocation) can't follow (seen: data abort in pump_rx, FAR G#…9927).
        d.put(b"dhcp ");
        if ty == 2 {
            d.put(b"OFFER");
        } else if ty == 5 {
            d.put(b"ACK");
        } else if ty == 6 {
            d.put(b"NAK");
        } else {
            d.put(b"other");
        }
        d.put(b" ");
        ip_put(d, yiaddr);
        d.put(b" from ");
        ip_put(d, server);
        d.put(b"\n");
        match ty {
            2 if self.offer.is_none() => self.offer = Some((yiaddr, server)),
            5 => self.acked = true,
            6 => self.nak = true,
            _ => {}
        }
    }
    /// Consume tx completions (host_txbuf_cmpl_t, type G#10, 24 B: request id = packet, status & G#F 0 = sent).
    fn pump_tx(&mut self, d: &mut Drv) {
        let wi = d.tcm_rd(self.d2h_w + 4);
        while self.tx_rd != wi {
            let a = self.dma + 0x6000 + 24 * self.tx_rd;
            let words: [u32; 6] = core::array::from_fn(|k| dr(a + 4 * k as u32));
            if words[0] >> 24 != self.tx_dseq % 253 || words.iter().fold(0, |x, v| x ^ v) != 0 {
                break;
            }
            self.tx_dseq += 1;
            self.tx_rd = (self.tx_rd + 1) % 1024;
            d.tcm_wr(self.d2h_r + 4, self.tx_rd);
            self.tx_done += 1;
            self.tx_status = words[2] & 0xF;
        }
    }
    /// Send one Ethernet frame on flowring 2 (msgbuf ring 5, H2D index slot 2): copy it to a tx buffer (2 KiB at +G#7_0000), then host_txbuf_post_t v1 (48 B): header (type G#0F, request id), the 14-byte Ethernet header, flags G#01 (802.3), seg_cnt 1, data address = the payload after the header, data_len = length − 14.
    fn send(&mut self, d: &mut Drv, frame: &[u8]) {
        let k = self.tx_w % 8;
        let buf = self.dma + 0x7_0000 + 0x800 * k;
        for (j, b) in frame.iter().enumerate() {
            db(buf + j as u32, *b);
        }
        let phase = if (self.tx_w / 512) % 2 == 0 { 0x80 } else { 0 };
        let a = self.dma + 0x6_0000 + 48 * (self.tx_w % 512);
        let mut item = [0u8; 48];
        item[0] = 0x0F;
        item[2] = phase as u8;
        item[4..8].copy_from_slice(&(0x2000 + self.tx_w).to_le_bytes());
        item[8..22].copy_from_slice(&frame[..14]);
        item[22] = 0x01;
        item[23] = 1;
        item[32..36].copy_from_slice(&(buf + 14).to_le_bytes());
        item[42..44].copy_from_slice(&((frame.len() - 14) as u16).to_le_bytes());
        for j in 0..12 {
            dw(a + 4 * j, u32::from_le_bytes(item[4 * j as usize..4 * j as usize + 4].try_into().unwrap()));
        }
        self.tx_w += 1;
        unsafe { core::arch::asm!("dsb sy") };
        d.tcm_wr(self.h2d_w + 8, self.tx_w % 512);
        d.wr(MEM, 0x2140, 0xFF00_0000 | 5 << 16 | (self.tx_w % 512));
    }
    /// Consume every valid D2H control completion: events are handled (and their buffers re-posted) here; an ioctl completion is returned as (status, response buffer index, length), its buffer re-posted.
    fn pump(&mut self, d: &mut Drv) -> Option<(u32, u32, u32)> {
        let wi = d.tcm_rd(self.d2h_w);
        while self.rd != wi {
            let a = self.dma + 0x5000 + 24 * self.rd;
            let words: [u32; 6] = core::array::from_fn(|k| dr(a + 4 * k as u32));
            if words[0] >> 24 != self.dseq % 253 || words.iter().fold(0, |x, v| x ^ v) != 0 {
                break; // the index ran ahead of the item's DMA: look again
            }
            self.dseq += 1;
            self.rd = (self.rd + 1) % 64;
            d.tcm_wr(self.d2h_r, self.rd);
            if self.verbose {
                d.hex(((d.now().wrapping_sub(self.t0) * 1_000_000) / d.freq) as u32);
                d.put(b"us d2h ctrl:");
                for v in words {
                    d.put(b" ");
                    d.hex(v);
                }
                d.put(b"\n");
            }
            match words[0] & 0xFF {
                0x0C => {
                    let b = words[1].wrapping_sub(1);
                    if b < 8 {
                        self.post(d, 0x0B, 1 + b, [8192, 0, self.dma + 0x2_0000 + 0x2000 * b, 0, 0, 0, 0, 0]);
                        self.ring(d);
                    }
                    return Some((words[2] & 0xFFFF, b, words[3] & 0xFFFF));
                }
                // MSG_TYPE_FLOW_RING_CREATE_CMPLT: status +8, flow ring id +10.
                0x04 => self.flow_status = Some(words[2] & 0xFFFF),
                // MSG_TYPE_WL_EVENT: request id = the event buffer, event_data_len at +12; the packet starts rx_dataoffset into the buffer.
                0x0E => {
                    let b = words[1].wrapping_sub(0x101);
                    if b < 8 {
                        let at = self.dma + 0x3_0000 + 0x800 * b;
                        self.event(d, at + self.rx_off, words[3] & 0xFFFF);
                        self.post(d, 0x0D, 0x101 + b, [2048, 0, at, 0, 0, 0, 0, 0]);
                        self.ring(d);
                    }
                }
                _ => {}
            }
        }
        None
    }
    /// A firmware event (bcm_event_t: Ethernet header, ether type G#886C, BRCM header, then wl_event_msg_t with big-endian fields: type +28, status +32, datalen +44, data at +72).
    fn event(&mut self, d: &mut Drv, p: u32, len: u32) {
        if len < 72 || be16(p + 12) != 0x886C {
            return;
        }
        let (ty, status, dlen) = (be32(p + 28), be32(p + 32), be32(p + 44));
        if ty != 69 {
            // Join progress (stage 12): SET_SSID 0, AUTH 3, DEAUTH 5/6, ASSOC 7, DISASSOC 11/12, LINK 16 (flags bit 0 = up), PSK_SUP 46 (status 6 KEYED / 10 WAIT_G1 with reason 0 = authorized), AUTHORIZED 136.
            let (reason, flags) = (be32(p + 36), be16(p + 26));
            d.put(b"event ");
            d.dec(ty);
            d.put(b" status ");
            d.dec(status);
            d.put(b" reason ");
            d.dec(reason);
            d.put(b" flags ");
            d.hex(flags);
            d.put(b"\n");
            match ty {
                16 => self.linked = flags & 1 != 0,
                46 if (status == 6 || status == 10) && reason == 0 => self.keyed = true,
                46 if status < 10 && reason != 0 && reason != 21 => self.join_failed = true,
                0 | 3 | 7 if status != 0 => self.join_failed = true,
                5 | 6 | 11 | 12 => self.join_failed = true,
                _ => {}
            }
            return;
        }
        // WLC_E_ESCAN_RESULT: status 8 = a partial result carrying one BSS, 0 = the scan finished, anything else = it ended otherwise.
        if status != 8 {
            self.scan_done = true;
            d.kv(b"escan finished, status", status);
            return;
        }
        let r = p + 72;
        if dlen < 12 || dr32u(r + 10) & 0xFFFF == 0 {
            return;
        }
        self.bss(d, r + 12);
    }
    /// One wl_bss_info (the version-independent prefix): BSSID +8, capability +16, SSID +18/+19, chanspec +72, RSSI +78, ctl_ch +88, IEs at ie_offset +116 / ie_length +120. Printed once per BSSID.
    fn bss(&mut self, d: &mut Drv, bi: u32) {
        let bssid: [u8; 6] = core::array::from_fn(|k| dbr(bi + 8 + k as u32));
        if self.seen[..self.nseen].contains(&bssid) {
            return;
        }
        if self.nseen < self.seen.len() {
            self.seen[self.nseen] = bssid;
            self.nseen += 1;
        }
        let cs = dr32u(bi + 72) & 0xFFFF;
        let band: &[u8] = if cs & 0xC000 == 0 {
            b"2G"
        } else if cs & 0xC000 == 0x4000 {
            b"6G"
        } else {
            b"5G"
        };
        let ch = if cs & 0x3800 == 0x1000 { cs & 0xFF } else { dbr(bi + 88) as u32 };
        let rssi = (dr32u(bi + 78 - 2) >> 16) as u16 as i16;
        let (ie_off, ie_len) = (dr32u(bi + 116) & 0xFFFF, dr32u(bi + 120));
        let mut sec: &[u8] = if dr32u(bi + 16) & 0x10 != 0 { b"WEP" } else { b"open" };
        let mut q = bi + ie_off;
        let end = q + ie_len.min(2048);
        while q + 2 <= end {
            let (id, l) = (dbr(q), dbr(q + 1) as u32);
            if id == 48 {
                sec = b"RSN";
                self.consider(bi, q + 2, l, cs);
            } else if id == 221 && l >= 4 && sec != b"RSN" && dbr(q + 2) == 0x00 && dbr(q + 3) == 0x50 && dbr(q + 4) == 0xF2 && dbr(q + 5) == 1 {
                sec = b"WPA";
            }
            q += 2 + l;
        }
        d.put(band);
        d.put(b" ch ");
        d.dec(ch);
        d.put(b" rssi -");
        d.dec((-(rssi as i32)).max(0) as u32);
        d.put(b" ");
        d.put(sec);
        d.put(b" \"");
        let n = (dbr(bi + 18) as u32).min(32);
        for k in 0..n {
            let c = dbr(bi + 19 + k);
            let c = if (0x20..0x7F).contains(&c) { c } else { b'?' };
            d.put(&[c]);
        }
        d.put(b"\"\n");
        if d.n > 2400 {
            d.flush();
        }
    }
    /// A BSS whose SSID is the one to join and whose RSN IE (version 2, group 4, pairwise count + list, AKM count + list, capabilities) offers PSK (00-0F-AC:2), outside 6 GHz (which needs WPA3-SAE): keep the strongest.
    fn consider(&mut self, bi: u32, rsn: u32, l: u32, cs: u32) {
        let n = dbr(bi + 18) as usize;
        if self.want_len == 0 || n != self.want_len || cs & 0xC000 == 0x4000 || l < 8 {
            return;
        }
        if (0..n).any(|k| dbr(bi + 19 + k as u32) != self.want[k]) {
            return;
        }
        let group: [u8; 4] = core::array::from_fn(|k| dbr(rsn + 2 + k as u32));
        let np = (dbr(rsn + 6) as u32) | (dbr(rsn + 7) as u32) << 8;
        let a = rsn + 8 + 4 * np;
        if a + 2 > rsn + l {
            return;
        }
        let na = (dbr(a) as u32) | (dbr(a + 1) as u32) << 8;
        let psk = (0..na).any(|k| {
            let s = a + 2 + 4 * k;
            dbr(s) == 0x00 && dbr(s + 1) == 0x0F && dbr(s + 2) == 0xAC && dbr(s + 3) == 2
        });
        let caps = a + 2 + 4 * na;
        let mfpc = caps + 1 < rsn + l && dbr(caps) & 0x80 != 0;
        if !psk {
            return;
        }
        let rssi = (dr32u(bi + 76) >> 16) as u16 as i16;
        let five = cs & 0xC000 == 0xC000;
        // Prefer 5 GHz (a first 2.4 GHz join associated, then lost the link waiting for the first beacon — WLC_E_LINK reason 8; 2.4 GHz shares the antenna with the chip's still-uninitialised Bluetooth), then signal: 5 GHz wins unless more than 15 dB weaker.
        let score = |five: bool, rssi: i16| rssi as i32 + if five { 15 } else { 0 };
        if self.target.map(|t| score(t.five, t.rssi) >= score(five, rssi)).unwrap_or(false) {
            return;
        }
        let bssid: [u8; 6] = core::array::from_fn(|k| dbr(bi + 8 + k as u32));
        self.target = Some(Target { bssid, chanspec: cs, rssi, group, mfpc, ctl: dbr(bi + 88), five });
    }
    /// One ioctl: `in_len` bytes at the request buffer go to the firmware, up to `out_len` come back in a posted response buffer. Waits for the completion (3 s), handling events meanwhile.
    fn ioctl(&mut self, d: &mut Drv, cmd: u32, req_buf: u32, in_len: u32, out_len: u32) -> Option<(u32, u32, u32)> {
        self.trans += 1;
        self.post(d, 0x09, 0xFFFE, [cmd, self.trans | in_len << 16, out_len, 0, req_buf, 0, 0, 0]);
        self.ring(d);
        let ts = d.now();
        while d.now().wrapping_sub(ts) < 3 * d.freq {
            if let Some(r) = self.pump(d) {
                return Some(r);
            }
            d.us(1000);
        }
        d.put(b"no ioctl completion\n");
        None
    }
    /// WLC_GET_VAR (262): the iovar name and its NUL (then `arg`) at the request buffer; dhd passes the whole buffer length both ways. The value comes back at offset 0 of the response buffer.
    fn get_var(&mut self, d: &mut Drv, name: &[u8], req_buf: u32) -> Option<(u32, u32, u32)> {
        self.get_var_arg(d, name, &[], req_buf)
    }
    fn get_var_arg(&mut self, d: &mut Drv, name: &[u8], arg: &[u8], req_buf: u32) -> Option<(u32, u32, u32)> {
        if req_buf >= self.dma && req_buf < self.dma + 0x20_0000 {
            for (k, c) in name.iter().chain(&[0u8]).chain(arg).enumerate() {
                db(req_buf + k as u32, *c);
            }
        }
        self.ioctl(d, 262, req_buf, 256, 256)
    }
    /// WLC_SET_VAR (263): name, NUL, value, no padding (bcm_mkiovar); the length is exactly that.
    fn set_var(&mut self, d: &mut Drv, name: &[u8], value: &[u8]) -> u32 {
        let req = self.dma + 0x1_3000;
        let mut k = 0;
        for c in name.iter().chain(&[0u8]).chain(value) {
            db(req + k, *c);
            k += 1;
        }
        self.ioctl(d, 263, req, k, k).map(|r| r.0).unwrap_or(u32::MAX)
    }
    fn set_int(&mut self, d: &mut Drv, name: &[u8], v: u32) -> u32 {
        self.set_var(d, name, &v.to_le_bytes())
    }
    /// A plain ioctl carrying one int (WLC_SET_INFRA, WLC_SET_PM, ...).
    fn ioctl_int(&mut self, d: &mut Drv, cmd: u32, v: u32) -> u32 {
        let req = self.dma + 0x1_3000;
        for (k, b) in v.to_le_bytes().iter().enumerate() {
            db(req + k as u32, *b);
        }
        self.ioctl(d, cmd, req, 4, 4).map(|r| r.0).unwrap_or(u32::MAX)
    }
    /// The response buffer holding an ioctl's answer.
    fn resp(&self, b: u32) -> u32 {
        self.dma + 0x2_0000 + 0x2000 * b
    }
}

fn be16(a: u32) -> u32 {
    (dbr(a) as u32) << 8 | dbr(a + 1) as u32
}
fn be32(a: u32) -> u32 {
    be16(a) << 16 | be16(a + 2)
}
/// A little-endian u32 at any byte address.
fn dr32u(a: u32) -> u32 {
    u32::from_le_bytes([dbr(a), dbr(a + 1), dbr(a + 2), dbr(a + 3)])
}

/// Physical (= identity-mapped) access to the DMA region: Normal non-cacheable, EL0 read/write.
fn dw(a: u32, v: u32) {
    unsafe { core::ptr::write_volatile(a as u64 as *mut u32, v) }
}
fn dr(a: u32) -> u32 {
    unsafe { core::ptr::read_volatile(a as u64 as *const u32) }
}
fn db(a: u32, v: u8) {
    unsafe { core::ptr::write_volatile(a as u64 as *mut u8, v) }
}
fn dbr(a: u32) -> u8 {
    unsafe { core::ptr::read_volatile(a as u64 as *const u8) }
}

/// The firmware's console ring (hnd_cons log) from `start`, `len` bytes, printable text only.
fn console_dump(d: &mut Drv, buf: u32, size: u32, start: u32, len: u32) {
    d.put(b"---- firmware console\n");
    let mut i = 0;
    while i < len {
        let pos = (start + i) % size;
        let w = d.tcm_rd(buf + (pos & !3));
        let c = (w >> (8 * (pos & 3))) as u8;
        if c == b'\n' || (0x20..0x7F).contains(&c) {
            d.put(&[c]);
        }
        if d.n > 2400 && c == b'\n' {
            d.flush();
        }
        i += 1;
    }
    d.put(b"\n---- end\n");
    d.flush();
}

/// Walk the endpoint's capability list (cfg G#34) for `id`; returns the capability's config offset.
fn find_cap(d: &mut Drv, id: u32) -> Option<u64> {
    let mut p = (d.rd(CFG, 0x34) & 0xFC) as u64;
    for _ in 0..48 {
        if p == 0 {
            return None;
        }
        let h = d.rd(CFG, p);
        if h & 0xFF == id {
            return Some(p);
        }
        p = ((h >> 8) & 0xFC) as u64;
    }
    None
}

/// A#192.168.1.1 — one base prefix for the dotted quad.
fn ip_put(d: &mut Drv, a: [u8; 4]) {
    d.put(b"A#");
    for (k, b) in a.iter().enumerate() {
        if k > 0 {
            d.put(b".");
        }
        let mut v = *b as u32;
        let mut digits = [0u8; 3];
        let mut n = 0;
        loop {
            digits[2 - n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        d.put(&digits[3 - n..]);
    }
}

/// Stage 13: one transmit flowring, then DHCP — DISCOVER (broadcast), OFFER, REQUEST, ACK — from the EL0 driver. Flowring 2 (dhd's first flow id) is created on the control ring with its DMA ring (512 × 48 B at +G#6_0000), DA broadcast and SA our MAC, as dhd does for a STA's first packet; a STA has one flowring per priority, whatever the destination.
fn dhcp(d: &mut Drv, m: &mut Msgbuf) {
    let req = m.dma + 0x1_3000;
    if let Some((0, b, _)) = m.get_var(d, b"cur_etheraddr", req) {
        m.mac = core::array::from_fn(|k| dbr(m.resp(b) + k as u32));
    }
    d.tcm_wr(m.h2d_w + 8, 0);
    let mut c = [0u8; 32];
    c[0..6].copy_from_slice(&[0xFF; 6]);
    c[6..12].copy_from_slice(&m.mac);
    c[14..16].copy_from_slice(&2u16.to_le_bytes());
    c[20..22].copy_from_slice(&512u16.to_le_bytes());
    c[22..24].copy_from_slice(&48u16.to_le_bytes());
    c[24..28].copy_from_slice(&(m.dma + 0x6_0000).to_le_bytes());
    let body: [u32; 8] = core::array::from_fn(|k| u32::from_le_bytes(c[4 * k..4 * k + 4].try_into().unwrap()));
    m.flow_status = None;
    m.post(d, 0x03, 0, body);
    m.ring(d);
    let ts = d.now();
    while m.flow_status.is_none() && d.now().wrapping_sub(ts) < 2 * d.freq {
        let _ = m.pump(d);
        d.us(1000);
    }
    d.kv(b"flowring create status", m.flow_status.unwrap_or(u32::MAX));
    if m.flow_status != Some(0) {
        d.flush();
        return;
    }
    m.xid = 0xF3A0_0000 | (d.now() as u32 & 0xFFFF);
    for attempt in 0..3u32 {
        let frame = dhcp_frame(&m.mac, m.xid, 1, None);
        m.send(d, &frame[..342]);
        d.kv(b"DISCOVER sent, attempt", attempt);
        d.flush();
        let ts = d.now();
        while m.offer.is_none() && d.now().wrapping_sub(ts) < 3 * d.freq {
            let _ = m.pump(d);
            m.pump_rx(d);
            m.pump_tx(d);
            d.us(1000);
        }
        if m.offer.is_some() {
            break;
        }
    }
    d.kv(b"tx completions", m.tx_done);
    d.kv(b"last tx status", m.tx_status);
    let Some((ip, server)) = m.offer else {
        d.put(b"no DHCP offer\n");
        d.flush();
        return;
    };
    let frame = dhcp_frame(&m.mac, m.xid, 3, Some((ip, server)));
    m.send(d, &frame[..342]);
    d.flush();
    let ts = d.now();
    while !m.acked && !m.nak && d.now().wrapping_sub(ts) < 4 * d.freq {
        let _ = m.pump(d);
        m.pump_rx(d);
        m.pump_tx(d);
        d.us(1000);
    }
    if m.acked {
        d.put(b"LEASED ");
        ip_put(d, ip);
        d.put(b" mask ");
        ip_put(d, m.lease[0]);
        d.put(b" router ");
        ip_put(d, m.lease[1]);
        d.put(b" dns ");
        ip_put(d, m.lease[2]);
        d.put(b" lease ");
        d.dec(m.lease_s);
        d.put(b" s\n");
        d.kv(b"rx frames", m.rx_frames);
    } else {
        d.put(if m.nak { b"DHCP NAK\n" } else { b"no DHCP ACK\n" });
    }
    d.flush();
}

/// A broadcast DHCP message from 0.0.0.0:68 to 255.255.255.255:67 (342 bytes): Ethernet, IPv4 (header checksum, UDP checksum 0 as IPv4 allows), BOOTP (op 1, broadcast flag so replies come broadcast, our MAC as chaddr), the magic cookie, options 53 (1 DISCOVER / 3 REQUEST), 50 + 54 on a REQUEST, 55 (subnet, router, DNS, lease), end.
fn dhcp_frame(mac: &[u8; 6], xid: u32, ty: u8, req: Option<([u8; 4], [u8; 4])>) -> [u8; 342] {
    let mut f = [0u8; 342];
    f[0..6].copy_from_slice(&[0xFF; 6]);
    f[6..12].copy_from_slice(mac);
    f[12..14].copy_from_slice(&[0x08, 0x00]);
    let ip = 14;
    let ip_len = (342 - 14) as u16;
    f[ip..ip + 12].copy_from_slice(&[0x45, 0, (ip_len >> 8) as u8, ip_len as u8, 0, 0, 0, 0, 64, 17, 0, 0]);
    f[ip + 16..ip + 20].copy_from_slice(&[255; 4]);
    let mut sum = 0u32;
    for k in (0..20).step_by(2) {
        sum += u16::from_be_bytes([f[ip + k], f[ip + k + 1]]) as u32;
    }
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    f[ip + 10..ip + 12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
    let udp = ip + 20;
    let udp_len = ip_len - 20;
    f[udp..udp + 8].copy_from_slice(&[0, 68, 0, 67, (udp_len >> 8) as u8, udp_len as u8, 0, 0]);
    let b = udp + 8;
    f[b..b + 4].copy_from_slice(&[1, 1, 6, 0]);
    f[b + 4..b + 8].copy_from_slice(&xid.to_be_bytes());
    f[b + 10] = 0x80;
    f[b + 28..b + 34].copy_from_slice(mac);
    f[b + 236..b + 240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
    let mut o = b + 240;
    for chunk in [&[53u8, 1, ty][..]] {
        f[o..o + chunk.len()].copy_from_slice(chunk);
        o += chunk.len();
    }
    if let Some((ip_req, server)) = req {
        f[o..o + 2].copy_from_slice(&[50, 4]);
        f[o + 2..o + 6].copy_from_slice(&ip_req);
        f[o + 6..o + 8].copy_from_slice(&[54, 4]);
        f[o + 8..o + 12].copy_from_slice(&server);
        o += 12;
    }
    f[o..o + 6].copy_from_slice(&[55, 4, 1, 3, 6, 51]);
    f[o + 6] = 255;
    f
}

/// [u32 LE len][bytes] twice (firmware, NVRAM), then optionally the CLM blob and the WiFi credential (empty if absent).
fn split_files(b: &mut [u8]) -> Option<(&[u8], &mut [u8], &[u8], &[u8])> {
    let take = |b: &[u8]| -> Option<usize> { Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize) };
    let a = take(b)?;
    if b.len() < 4 + a + 4 {
        return None;
    }
    let (first, rest) = b[4..].split_at_mut(a);
    let n = take(rest)?;
    if rest.len() < 4 + n {
        return None;
    }
    let (second, rest) = rest[4..].split_at_mut(n);
    let (third, rest): (&[u8], &[u8]) = match take(rest) {
        Some(c) if rest.len() >= 4 + c => (&rest[4..4 + c], &rest[4 + c..]),
        _ => (&[], &[]),
    };
    let fourth: &[u8] = match take(rest) {
        Some(c) if rest.len() >= 4 + c => &rest[4..4 + c],
        _ => &[],
    };
    Some((first, second, third, fourth))
}

/// The WiFi credential file (keys/wifi.txt): the SSID on the first line, the passphrase on the second. Never printed.
fn credential(c: &[u8]) -> Option<(&[u8], &[u8])> {
    let trim = |s: &[u8]| -> usize { s.iter().rposition(|&b| b != b'\r' && b != b' ').map(|i| i + 1).unwrap_or(0) };
    let nl = c.iter().position(|&b| b == b'\n')?;
    let ssid = &c[..trim(&c[..nl])];
    let rest = &c[nl + 1..];
    let end = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
    let pass = &rest[..trim(&rest[..end])];
    if ssid.is_empty() || ssid.len() > 32 || pass.len() < 8 || pass.len() > 63 {
        return None;
    }
    Some((ssid, pass))
}

/// Stage 12: join the best scanned BSS of the credential's network with WPA2-PSK, the firmware doing the 4-way handshake (dhd's wl_cfg80211_connect order: sup_wpa, STA defaults, infra, security cleared, wpa_auth provisional → auth → wsec → mfp → wpa_auth final, the passphrase as WLC_SET_WSEC_PMK, our RSN IE as "wpaie", then "join" with the BSSID and chanspec). Done when the firmware's supplicant reports keyed.
fn join(d: &mut Drv, m: &mut Msgbuf, cred: &[u8]) {
    let Some((ssid, pass)) = credential(cred) else {
        d.put(b"stage 12 wants --input-file keys/wifi.txt (SSID line, passphrase line, 8..63 chars)\n");
        d.flush();
        return;
    };
    let Some(t) = m.target else {
        d.put(b"no WPA2-PSK BSS of that SSID in the scan (outside 6 GHz)\n");
        d.flush();
        return;
    };
    d.put(if t.five { b"joining: 5G ch " } else { b"joining: 2G ch " });
    d.dec(t.ctl as u32);
    d.put(b" rssi -");
    d.dec((-(t.rssi as i32)).max(0) as u32);
    d.put(if t.mfpc { b" (AP can do MFP)\n" } else { b"\n" });
    // Receive buffers first: the firmware runs split RX (SPLITRX_MODE 4), so data frames — the AP's EAPOL handshake messages among them — land in host buffers from the rxpost ring; with none posted the handshake never starts (seen: associated, no PSK_SUP, beacon loss after 15 s).
    for i in 0..64 {
        m.post_rx(i);
    }
    m.ring_rx(d);
    let r = m.set_int(d, b"sup_wpa", 1);
    d.kv(b"sup_wpa", r);
    let r = m.set_int(d, b"roam_off", 1);
    d.kv(b"roam_off", r);
    let r = m.set_int(d, b"buf_key_b4_m4", 1);
    d.kv(b"buf_key_b4_m4", r);
    let r = m.ioctl_int(d, 86, 0);
    d.kv(b"PM off", r);
    let r = m.ioctl_int(d, 20, 1);
    d.kv(b"infra", r);
    for name in [&b"wsec"[..], b"auth", b"wpa_auth"] {
        let _ = m.set_int(d, name, 0);
    }
    let r = [m.set_int(d, b"wpa_auth", 0xC0), m.set_int(d, b"auth", 0), m.set_int(d, b"wsec", 4), m.set_int(d, b"mfp", t.mfpc as u32), m.set_int(d, b"wpa_auth", 0x80)];
    d.put(b"security wpa_auth/auth/wsec/mfp/wpa_auth:");
    for v in r {
        d.put(b" ");
        d.hex(v);
    }
    d.put(b"\n");
    // wsec_pmk_t: u16 key_len, u16 flags (1 = passphrase), u8 key[256], u16 opt_len — 263 bytes as dhd sends it.
    let req = m.dma + 0x1_3000;
    for k in 0..263 {
        db(req + k, 0);
    }
    for (k, b) in (pass.len() as u16).to_le_bytes().iter().chain(&1u16.to_le_bytes()).chain(pass).enumerate() {
        db(req + k as u32, *b);
    }
    let r = m.ioctl(d, 268, req, 263, 263).map(|r| r.0).unwrap_or(u32::MAX);
    d.kv(b"WSEC_PMK", r);
    for k in 0..263 {
        db(req + k, 0);
    }
    // Our RSN IE: version 1, the AP's group cipher, pairwise CCMP, AKM PSK, capabilities (MFPC if we set mfp 1).
    let mut ie = [0u8; 22];
    ie[..4].copy_from_slice(&[48, 20, 1, 0]);
    ie[4..8].copy_from_slice(&t.group);
    ie[8..20].copy_from_slice(&[1, 0, 0x00, 0x0F, 0xAC, 4, 1, 0, 0x00, 0x0F, 0xAC, 2]);
    ie[20] = if t.mfpc { 0x80 } else { 0 };
    let r = m.set_var(d, b"wpaie", &ie);
    d.kv(b"wpaie", r);
    // join: v0 (wl_extjoin_params_t) unless join_ver says otherwise; ssid (u32 len + 32), scan params (scan_type u8, pad, nprobes, active, passive, home = -1), then the assoc params: BSSID, bssid_cnt 0, one chanspec.
    let jv = match m.get_var(d, b"join_ver", req) {
        Some((0, b, _)) => dr32u(m.resp(b) + 4) & 0xFFFF,
        _ => 0,
    };
    d.kv(b"join_ver", jv);
    let mut j = [0u8; 84];
    let (o_ssid, o_scan, o_bssid, o_cnt, o_num, o_list) = match jv {
        0 => (0usize, 36usize, 56usize, 62usize, 64usize, 68usize),
        1 => (4, 40, 64, 70, 72, 76),
        _ => (4, 40, 64, 70, 76, 80),
    };
    j[o_ssid..o_ssid + 4].copy_from_slice(&(ssid.len() as u32).to_le_bytes());
    j[o_ssid + 4..o_ssid + 4 + ssid.len()].copy_from_slice(ssid);
    for k in 0..4 {
        let at = o_scan + 4 + 4 * k;
        j[at..at + 4].copy_from_slice(&(-1i32).to_le_bytes());
    }
    j[o_bssid..o_bssid + 6].copy_from_slice(&t.bssid);
    j[o_cnt..o_cnt + 2].copy_from_slice(&0u16.to_le_bytes());
    j[o_num..o_num + 4].copy_from_slice(&1u32.to_le_bytes());
    j[o_list..o_list + 2].copy_from_slice(&(t.chanspec as u16).to_le_bytes());
    m.linked = false;
    m.keyed = false;
    m.join_failed = false;
    let r = m.set_var(d, b"join", &j[..o_list + 2]);
    d.kv(b"join", r);
    d.flush();
    let ts = d.now();
    while !m.keyed && !m.join_failed && d.now().wrapping_sub(ts) < 15 * d.freq {
        let _ = m.pump(d);
        m.pump_rx(d);
        if d.n > 2000 {
            d.flush();
        }
        d.us(2000);
    }
    d.kv(b"join_ms", ((d.now().wrapping_sub(ts) * 1000) / d.freq) as u32);
    d.kv(b"rx frames", m.rx_frames);
    if m.keyed {
        d.put(b"ASSOCIATED AND KEYED (WPA2-PSK, firmware supplicant)\n");
        // scb_val_t: i32 val, u8 ea[6], pad — WLC_GET_RSSI (127).
        for k in 0..12 {
            db(req + k, 0);
        }
        if let Some((0, b, _)) = m.ioctl(d, 127, req, 12, 12) {
            let v = dr32u(m.resp(b)) as i32;
            d.put(b"link rssi -");
            d.dec((-v).max(0) as u32);
            d.put(b"\n");
        }
    } else {
        d.put(if m.join_failed { b"join failed (see events)\n" } else { b"join timed out\n" });
    }
    d.flush();
}

/// Stage 11: a scan, the way dhd's up path and wl_run_escan do it. The regulatory table first (`country` reads "#n" when the firmware has no CLM: then the CLM blob goes in through `clmload` in 1408-byte chunks), the escan-result event (69) enabled in `event_msgs_ext`, WLC_UP (2), `scan_ver` to pick the request layout, then `escan` over every channel; results arrive as events, one BSS each, until a final status.
fn scan(d: &mut Drv, m: &mut Msgbuf, clm: &[u8]) {
    let req = m.dma + 0x1_3000;
    if let Some((0, b, _)) = m.get_var(d, b"country", req) {
        let cc: [u8; 4] = core::array::from_fn(|k| dbr(m.resp(b) + k as u32));
        d.put(b"country before CLM: ");
        d.put(&cc[..2]);
        d.put(b"\n");
        if &cc[..2] == b"#n" {
            if clm.is_empty() {
                d.put(b"no CLM in firmware and none given (--input-file clm_blob)\n");
                return;
            }
            // wl_dload_data_t per chunk: u16 flag (handler version 1 << 12 | DL_BEGIN 2 on the first | DL_END 4 on the last), u16 type 2 (CLM), u32 len, u32 crc 0, data; the chunk padded to 8.
            let mut off = 0;
            let mut status = 0;
            while off < clm.len() && status == 0 {
                let n = (clm.len() - off).min(1408);
                let mut flag = 1u16 << 12;
                if off == 0 {
                    flag |= 2;
                }
                if off + n == clm.len() {
                    flag |= 4;
                }
                let mut chunk = [0u8; 1424];
                chunk[0..2].copy_from_slice(&flag.to_le_bytes());
                chunk[2..4].copy_from_slice(&2u16.to_le_bytes());
                chunk[4..8].copy_from_slice(&(n as u32).to_le_bytes());
                chunk[12..12 + n].copy_from_slice(&clm[off..off + n]);
                let total = (12 + n).div_ceil(8) * 8;
                status = m.set_var(d, b"clmload", &chunk[..total]);
                off += n;
            }
            d.kv(b"clmload status", status);
            if let Some((0, b, _)) = m.get_var(d, b"country", req) {
                let cc: [u8; 4] = core::array::from_fn(|k| dbr(m.resp(b) + k as u32));
                d.put(b"country after CLM: ");
                d.put(&cc[..2]);
                d.put(b"\n");
            }
        }
    }
    // event_msgs_ext: GET the mask (ver 1, command 0, len 27, maxgetsize 27), set bit 69, SET it back (command 3).
    let mut mask = [0u8; 27];
    if let Some((0, b, _)) = m.get_var_arg(d, b"event_msgs_ext", &[1, 0, 27, 27], req) {
        for (k, v) in mask.iter_mut().enumerate() {
            *v = dbr(m.resp(b) + 4 + k as u32);
        }
    }
    for e in [69u32, 0, 3, 5, 6, 7, 11, 12, 16, 46, 136] {
        mask[(e >> 3) as usize] |= 1 << (e & 7);
    }
    let mut ev = [0u8; 31];
    ev[..4].copy_from_slice(&[1, 3, 27, 0]);
    ev[4..].copy_from_slice(&mask);
    let r = m.set_var(d, b"event_msgs_ext", &ev);
    d.kv(b"event_msgs_ext set", r);
    for (k, v) in 0u32.to_le_bytes().iter().enumerate() {
        db(req + k as u32, *v);
    }
    let r = m.ioctl(d, 2, req, 4, 4).map(|r| r.0).unwrap_or(u32::MAX);
    d.kv(b"WLC_UP", r);
    let ver = match m.get_var(d, b"scan_ver", req) {
        Some((0, b, _)) => dr32u(m.resp(b) + 4) & 0xFFFF,
        _ => 1,
    };
    d.kv(b"scan_ver", ver);
    // escan: u32 version, u16 action 1 (start), u16 sync id, then the scan params — v1 without the version/length prefix; v2/v3 72 bytes, v4 76 (scan_type_ext). Broadcast SSID, BSSID ff…ff, any BSS type, active, firmware-default times, every channel.
    let mut p = [0u8; 84];
    let (hdr_ver, plen): (u32, usize) = match ver {
        2 | 3 => (ver, 72),
        4 => (4, 76),
        _ => (1, 64),
    };
    p[0..4].copy_from_slice(&hdr_ver.to_le_bytes());
    p[4..6].copy_from_slice(&1u16.to_le_bytes());
    p[6..8].copy_from_slice(&0x4F31u16.to_le_bytes());
    let s = 8;
    let o = if hdr_ver == 1 { 0 } else { 4 };
    if hdr_ver != 1 {
        p[s..s + 2].copy_from_slice(&(hdr_ver as u16).to_le_bytes());
        p[s + 2..s + 4].copy_from_slice(&(plen as u16).to_le_bytes());
    }
    p[s + o + 36..s + o + 42].copy_from_slice(&[0xFF; 6]);
    p[s + o + 42] = 2;
    let t = s + if hdr_ver == 1 { 44 } else if hdr_ver == 4 { 56 } else { 52 };
    for k in 0..4 {
        p[t + 4 * k..t + 4 * k + 4].copy_from_slice(&(-1i32).to_le_bytes());
    }
    m.scan_done = false;
    let r = m.set_var(d, b"escan", &p[..8 + plen]);
    d.kv(b"escan", r);
    d.flush();
    let ts = d.now();
    while !m.scan_done && d.now().wrapping_sub(ts) < 12 * d.freq {
        let _ = m.pump(d);
        d.us(2000);
    }
    d.kv(b"BSSes", m.nseen as u32);
    d.kv(b"scan_ms", ((d.now().wrapping_sub(ts) * 1000) / d.freq) as u32);
    d.flush();
}

/// bcmutils.c process_nvram_vars, in place: drop '\r', '#' comments to end of line and empty lines; each line ends in a NUL. Returns the length used (the rest is zeroed).
fn process_nvram_vars(v: &mut [u8]) -> usize {
    let (mut dp, mut find_newline, mut column) = (0usize, false, 0usize);
    for n in 0..v.len() {
        let c = v[n];
        if c == b'\r' || (find_newline && c != b'\n') {
            continue;
        }
        find_newline = false;
        if c == b'#' {
            find_newline = true;
            continue;
        }
        if c == b'\n' {
            if column == 0 {
                continue;
            }
            v[dp] = 0;
            dp += 1;
            column = 0;
            continue;
        }
        v[dp] = c;
        dp += 1;
        column += 1;
    }
    for b in &mut v[dp..] {
        *b = 0;
    }
    dp
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { asm!("svc #0", in("x0") 0xDEADu64, in("x8") 0u64, options(noreturn, nostack)) }
}
