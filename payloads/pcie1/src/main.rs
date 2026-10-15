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
    let Some((fw, nv)) = split_files(files) else {
        d.put(b"stage 7 wants --input-file firmware --input-file nvram\n");
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
    let mut m = Msgbuf { dma, h2d_w, d2h_w, d2h_r, w: 0, seq: 254, rd: 0, dseq: 254, trans: 0 };
    // Control submit ring: 8 ioctl response buffers and 8 event buffers first (the firmware DMAs ioctl results into a posted response buffer, named by its request id in the completion).
    for b in 0..8u32 {
        m.post(d, 0x0B, 1 + b, [8192, 0, dma + IOCTL_RESP + 0x2000 * b, 0, 0, 0, 0, 0]);
    }
    for b in 0..8u32 {
        m.post(d, 0x0D, 0x101 + b, [2048, 0, dma + EVENT_BUF + 0x800 * b, 0, 0, 0, 0, 0]);
    }
    m.ring(d);
    let t0 = d.now();
    match m.get_var(d, b"ver", dma + IOCTL_REQ, t0) {
        Some((0, b, len)) => {
            d.put(b"ver: ");
            d.text(dma + IOCTL_RESP + 0x2000 * b, len);
        }
        r => d.kv(b"ver failed, status", r.map(|r| r.0).unwrap_or(u32::MAX)),
    }
    d.flush();
    if stage >= 10 {
        // ---- stage 10: a second query the normal way, then the WiFi chip's S2MPU VID, measured: an ioctl whose request buffer sits outside the fence (G#8008_0000: the kernel image, inside the window the first fence allowed). The firmware's DMA read of it is denied (DECERR) and the S2MPU latches the fault with the VID, which `bridge diag` reports (S2MPU_FAULT_INFO bits 26:24). A denied read can't touch memory; the firmware may trap. ----
        match m.get_var(d, b"cur_etheraddr", dma + IOCTL_REQ, t0) {
            Some((0, _, len)) => d.kv(b"cur_etheraddr len", len),
            r => d.kv(b"cur_etheraddr failed, status", r.map(|r| r.0).unwrap_or(u32::MAX)),
        }
        d.put(b"probe: ioctl request buffer at G#80080000 (the kernel image, outside the fence)\n");
        d.flush();
        let r = m.get_var(d, b"ver", 0x8008_0000, t0);
        d.kv(b"probe result status", r.map(|r| r.0).unwrap_or(u32::MAX));
        let f = d.tcm_rd(sh);
        d.kv(b"sh.flags (G#400 = trapped)", f);
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
    /// WLC_GET_VAR (262): the iovar name and its NUL at `req_buf` (dhd passes the whole buffer length both ways). Waits for the completion (3 s); Some((status, response buffer index, length)). Every completion item is printed; each consumed response buffer is re-posted.
    fn get_var(&mut self, d: &mut Drv, name: &[u8], req_buf: u32, t0: u64) -> Option<(u32, u32, u32)> {
        if req_buf >= self.dma && req_buf < self.dma + 0x20_0000 {
            for (k, c) in name.iter().chain(&[0u8]).enumerate() {
                db(req_buf + k as u32, *c);
            }
        }
        self.trans += 1;
        self.post(d, 0x09, 0xFFFE, [262, self.trans | 256 << 16, 256, 0, req_buf, 0, 0, 0]);
        self.ring(d);
        let ts = d.now();
        while d.now().wrapping_sub(ts) < 3 * d.freq {
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
                d.hex(((d.now().wrapping_sub(t0) * 1_000_000) / d.freq) as u32);
                d.put(b"us d2h ctrl:");
                for v in words {
                    d.put(b" ");
                    d.hex(v);
                }
                d.put(b"\n");
                if words[0] & 0xFF == 0x0C {
                    let b = words[1].wrapping_sub(1);
                    if b < 8 {
                        self.post(d, 0x0B, 1 + b, [8192, 0, self.dma + 0x2_0000 + 0x2000 * b, 0, 0, 0, 0, 0]);
                        self.ring(d);
                    }
                    return Some((words[2] & 0xFFFF, b, words[3] & 0xFFFF));
                }
            }
            d.us(1000);
        }
        d.put(b"no ioctl completion\n");
        None
    }
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

/// [u32 LE len][bytes] twice.
fn split_files(b: &mut [u8]) -> Option<(&[u8], &mut [u8])> {
    let take = |b: &[u8]| -> Option<usize> { Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize) };
    let a = take(b)?;
    if b.len() < 4 + a + 4 {
        return None;
    }
    let (first, rest) = b[4..].split_at_mut(a);
    let n = take(rest)?;
    let second = rest.get_mut(4..4 + n)?;
    Some((first, second))
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
