//! The cellular modem (Shannon S5300 over PCIe channel 0): what the cold boot needs, step by step (docs/oracle/MODEM-BOOT-RECON.md).
//!
//! Step 1 here: the images. `modem.bin` in `modem_a` begins with a TOC of 32-byte entries {name[12], file offset, CP load address, size, CRC, id}; BOOT is the ROM stage, MAIN/VSS/APM go over the boot channel, and the three NV sections come from per-unit files (efs nv_normal.bin / nv_protected.bin, modem_userdata replay_region.bin).
//! Nothing here prints NV contents: they are per-unit.

use alloc::vec::Vec;

/// One TOC entry.
#[derive(Clone, Copy, Debug)]
pub struct Section {
    pub name: [u8; 12],
    pub offset: u32,
    pub load: u32,
    pub size: u32,
    pub crc: u32,
    pub id: u32,
}

impl Section {
    pub fn name(&self) -> &str {
        let n = self.name.iter().position(|&c| c == 0).unwrap_or(12);
        core::str::from_utf8(&self.name[..n]).unwrap_or("?")
    }
}

pub const MODEM_PART: &str = "modem_a";
pub const MODEM_BIN: &str = "/images/default/modem.bin";

/// Parse the TOC from the first block of modem.bin.
pub fn toc(first: &[u8]) -> Vec<Section> {
    let mut v = Vec::new();
    for e in first.chunks(32).take(32) {
        if e.len() < 32 || e[0] == 0 || !e[..12].iter().take_while(|&&c| c != 0).all(|&c| (32..127).contains(&c)) {
            break;
        }
        let w = |o: usize| u32::from_le_bytes([e[o], e[o + 1], e[o + 2], e[o + 3]]);
        let mut name = [0u8; 12];
        name.copy_from_slice(&e[..12]);
        v.push(Section { name, offset: w(12), load: w(16), size: w(20), crc: w(24), id: w(28) });
    }
    v
}

fn line(rep: &mut Vec<u8>, s: alloc::string::String) {
    rep.extend_from_slice(s.as_bytes());
}

/// `diag modem files`: find and list the modem's images on the flash, read only — modem.bin's TOC, and the sizes of the per-unit NV files.
pub fn files(rep: &mut Vec<u8>) {
    let t0 = crate::platform::cntpct();
    match crate::partfs::mount(MODEM_PART) {
        Err(e) => line(rep, alloc::format!("{e}\n")),
        Ok(mut m) => match m.open(MODEM_BIN) {
            Err(e) => line(rep, alloc::format!("{MODEM_PART}: {e}\n")),
            Ok(ino) => {
                line(rep, alloc::format!("{MODEM_PART}{MODEM_BIN}: A#{} bytes\n", ino.size));
                let mut first = alloc::vec![0u8; 4096];
                match m.read(&ino, 0, &mut first) {
                    Err(e) => line(rep, alloc::format!("{e}\n")),
                    Ok(_) => {
                        for s in toc(&first) {
                            line(rep, alloc::format!("  {:<8} offset G#{:X} load G#{:X} size G#{:X} crc G#{:X} id A#{}\n", s.name(), s.offset, s.load, s.size, s.crc, s.id));
                        }
                    }
                }
            }
        },
    }
    match crate::partfs::mount("efs") {
        Err(e) => line(rep, alloc::format!("{e}\n")),
        Ok(mut m) => {
            for f in ["/nv_normal.bin", "/nv_protected.bin"] {
                match m.open(f) {
                    Ok(i) => line(rep, alloc::format!("efs{f}: A#{} bytes\n", i.size)),
                    Err(e) => line(rep, alloc::format!("efs: {e}\n")),
                }
            }
        }
    }
    match crate::partfs::find("modem_userdata") {
        Some(p) => line(rep, alloc::format!("modem_userdata: LU A#{} LBA A#{}..A#{} (f2fs, not read: replay_region.bin)\n", p.lun, p.first, p.last)),
        None => line(rep, alloc::format!("modem_userdata not found\n")),
    }
    let ms = (crate::platform::cntpct() - t0) / (crate::platform::cntfrq() / 1000);
    line(rep, alloc::format!("A#{ms} ms\n"));
}

/// `diag modem readmain`: read all of MAIN out of modem.bin through ext4 and hash it (BLAKE3), timing it: the read path the boot download will use. Compare the hash with the host's over the same slice of modem.bin (the TOC's CRC is not plain CRC-32, and the boot channel only forwards it, so it is not recomputed).
pub fn read_main(rep: &mut Vec<u8>) {
    let mut m = match crate::partfs::mount(MODEM_PART) {
        Ok(m) => m,
        Err(e) => return line(rep, alloc::format!("{e}\n")),
    };
    let ino = match m.open(MODEM_BIN) {
        Ok(i) => i,
        Err(e) => return line(rep, alloc::format!("{e}\n")),
    };
    let mut first = alloc::vec![0u8; 4096];
    if let Err(e) = m.read(&ino, 0, &mut first) {
        return line(rep, alloc::format!("{e}\n"));
    }
    let Some(main) = toc(&first).into_iter().find(|s| s.name() == "MAIN") else {
        return line(rep, alloc::format!("no MAIN in the TOC\n"));
    };
    // MAIN starts mid-block: read whole blocks from the block holding its start and hash the slice that is MAIN.
    let t0 = crate::platform::cntpct();
    let start = main.offset as u64;
    let end = start + main.size as u64;
    let mut h = blake3::Hasher::new();
    let mut buf = alloc::vec![0u8; 256 * 1024];
    let mut pos = start & !4095;
    while pos < end {
        crate::platform::pet_watchdogs();
        let n = match m.read(&ino, pos, &mut buf) {
            Ok(n) if n > 0 => n,
            Ok(_) => break,
            Err(e) => return line(rep, alloc::format!("at G#{pos:X}: {e}\n")),
        };
        let lo = start.saturating_sub(pos) as usize;
        let hi = ((end - pos) as usize).min(n);
        if lo < hi {
            h.update(&buf[lo..hi]);
        }
        pos += n as u64;
    }
    let ms = (crate::platform::cntpct() - t0) / (crate::platform::cntfrq() / 1000);
    let d = h.finalize();
    let mut hx = alloc::string::String::new();
    for b in &d.as_bytes()[..16] {
        hx.push_str(&alloc::format!("{b:02x}"));
    }
    line(rep, alloc::format!("MAIN G#{:X} bytes from file offset G#{:X} read in A#{ms} ms, BLAKE3 {hx}...\n", main.size, main.offset));
}

// ---------------------------------------------------------------------------
// Staged cold bring-up of the modem over PCIe channel 0 (MODEM-BOOT-RECON.md sections 1-3), ported from the working ch1 WiFi driver (payloads/pcie1, WIFI.md) with ch0's addresses and its own PHY branch (pcie-exynos-zuma-rc-cal.c 533-714).
// `diag modem sN` runs stages 0..=N in order, so a hang points at the stage that caused it:
// s0 read-only survey; s1 modem power-on by GPIO; s2 PHY; s3 root complex + link (Gen1 x1); s4 iATU, endpoint config, BAR0; s5 ROM boot (BOOT section from the flash, doorbell, boot_stage G#3FFF).

const PMU: u64 = 0x1546_0000;
const PD_HSI1_STATUS: u64 = 0x1546_2B04;
const PMU_PCIE0: u64 = PMU + 0x3EC4;
const CMU_HSI1: usize = 0x1200_0000;
const SYSREG_HSI1: usize = 0x1202_0000;
const GPIO_HSI1: usize = 0x1204_0000;
const SYSMMU_HSI1: usize = 0x1206_0000;
const S2MPU_HSI1: usize = 0x1209_0000;
const ELBI: usize = 0x1210_0000;
const SOC: usize = 0x1211_0000;
const PCS: usize = 0x1212_0000;
const UDBG: usize = 0x1213_0000;
const PHY: usize = 0x1214_0000;
const DBI: usize = 0x1240_0000;
const ATU: usize = DBI + 0x30_0000;
const CFG: usize = 0x40FF_E000;
/// The memory window: CPU G#4000_0000 = bus G#14E0_0000; the CP's doorbell (EP BAR0) is bus G#14E6_0000 = CPU G#4006_0000.
const MEM_CPU: u32 = 0x4000_0000;
const MEM_BUS: u32 = 0x14E0_0000;
const DOORBELL_BUS: u32 = 0x14E6_0000;
/// The doorbell's CPU address, as s51xx_pcie_probe derives it: window start + (G#14E6_0000 - BAR0's readback). Linux ends up at G#4006_0000 (the census of 2026-10-19 saw the G#1_0000 write there). Set by stage 4.
static DOORBELL: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0x4006_0000);
const GPIO_ALIVE: usize = 0x154D_0000;
/// Shared memory (zuma-rmem.dtsi): the IPC region and the MSI / boot-register page.
pub const IPC: usize = 0xEA40_0000;
const ROM_IMG: usize = IPC + 0x1_0000;
const MSI_PAGE: usize = 0xF620_0000;
const BOOT_STAGE: usize = MSI_PAGE + 0x10;

/// CMU_HSI1 gates on the ch0 path (cmucal-sfr.c): read before anything in HSI1 is touched (an access to a gated block hangs the core).
const HSI1_GATES: [(&str, usize); 14] = [
    ("phy_apb", 0x2094), ("dbi_aclk", 0x2068), ("mstr_aclk", 0x206C), ("subctrl_apb", 0x2074), ("slv_aclk", 0x2070), ("ia_clk", 0x209C),
    ("phy_refclk", 0x208C), ("gpio_pclk", 0x205C), ("uasc_dbi", 0x20D8), ("uasc_slv", 0x20E0), ("udbg_apb", 0x2090), ("sysmmu", 0x204C), ("ssmt_aclk", 0x20C8), ("ssmt_pclk", 0x20CC),
];

fn rd(a: usize) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { core::ptr::write_volatile(a as *mut u32, v) }
}
fn set(a: usize, bits: u32) {
    wr(a, rd(a) | bits)
}
fn clr(a: usize, bits: u32) {
    wr(a, rd(a) & !bits)
}
fn now() -> u64 {
    crate::platform::cntpct()
}
fn us(n: u64) {
    let t = now() + crate::platform::cntfrq() / 1_000_000 * n;
    while now() < t {}
}
fn ms(n: u64) {
    for _ in 0..n {
        crate::platform::pet_watchdogs();
        us(1000);
    }
}
fn since_us(t0: u64) -> u64 {
    (now() - t0) * 1_000_000 / crate::platform::cntfrq()
}
/// Poll `(rd(a) & mask) == want` every 10 µs for at most `max_us`; the microseconds it took.
fn poll(a: usize, mask: u32, want: u32, max_us: u64) -> Option<u64> {
    let t0 = now();
    loop {
        if rd(a) & mask == want {
            return Some(since_us(t0));
        }
        if since_us(t0) > max_us {
            return None;
        }
        us(10);
    }
}
fn kv(rep: &mut Vec<u8>, k: &str, v: u32) {
    line(rep, alloc::format!("{k} G#{v:X}\n"));
}
fn gate_open(v: u32) -> bool {
    !(v & (1 << 20) != 0 && v & (1 << 21) == 0)
}

/// A GPIO pad: bank base (CON at +0, DAT +4, PUD +8, DRV +C) and pin.
#[derive(Clone, Copy)]
struct Pad(usize, u32);
impl Pad {
    fn func(self, f: u32) {
        let c = rd(self.0);
        wr(self.0, (c & !(0xF << (self.1 * 4))) | (f & 0xF) << (self.1 * 4));
    }
    fn pud(self, p: u32) {
        let c = rd(self.0 + 8);
        wr(self.0 + 8, (c & !(0xF << (self.1 * 4))) | (p & 0xF) << (self.1 * 4));
    }
    fn level(self, high: bool) {
        if high { set(self.0 + 4, 1 << self.1) } else { clr(self.0 + 4, 1 << self.1) }
    }
    /// Output at `high`: level first, then direction, so it never glitches.
    fn out(self, high: bool) {
        self.level(high);
        self.func(1);
    }
    fn get(self) -> bool {
        rd(self.0 + 4) & (1 << self.1) != 0
    }
}
// MODEM-BOOT-RECON.md section 1 table.
const GPH0: usize = GPIO_HSI1;
const GPH1: usize = GPIO_HSI1 + 0x20;
const GPA0: usize = GPIO_ALIVE;
const GPA6: usize = GPIO_ALIVE + 0xA0;
const GPA7: usize = GPIO_ALIVE + 0xC0;
const CP_PWR: Pad = Pad(GPH1, 2);
const NRESET: Pad = Pad(GPH1, 7);
const CP_WRST_N: Pad = Pad(GPH1, 1);
const AP_ACTIVE: Pad = Pad(GPH1, 0);
const AP2CP_WAKEUP: Pad = Pad(GPH0, 2);
const DUMP_NOTI: Pad = Pad(GPH0, 3);
const PM_WRST_N: Pad = Pad(GPA0, 2);
const CP2AP_WRST_N: Pad = Pad(GPA0, 1);
const PS_HOLD: Pad = Pad(GPA6, 4);
const CP2AP_WAKEUP: Pad = Pad(GPA6, 5);
const PHONE_ACTIVE: Pad = Pad(GPA7, 0);
const PERST: Pad = Pad(GPH0, 0);
const CLKREQ: Pad = Pad(GPH0, 1);

fn cp2ap(rep: &mut Vec<u8>, at: &str) {
    line(rep, alloc::format!("{at}: PS_HOLD {} CP_WRST_N {} WAKEUP {} PHONE_ACTIVE {}\n", PS_HOLD.get() as u8, CP2AP_WRST_N.get() as u8, CP2AP_WAKEUP.get() as u8, PHONE_ACTIVE.get() as u8));
}

/// Stage 0: what the cold boot left, read only. Returns false when HSI1 is not powered or its gates are closed (nothing further is safe).
fn survey(rep: &mut Vec<u8>) -> bool {
    let pd = crate::platform::priv_reg(PD_HSI1_STATUS, false, 0) as u32;
    kv(rep, "pd_hsi1.status", pd);
    if pd & 1 == 0 {
        line(rep, alloc::format!("HSI1 not powered: stopping\n"));
        return false;
    }
    let mut all = true;
    let mut s = alloc::string::String::from("cmu_hsi1:");
    for (n, off) in HSI1_GATES {
        let v = rd(CMU_HSI1 + off);
        all &= gate_open(v);
        s.push_str(&alloc::format!(" {n} G#{v:X}"));
    }
    s.push('\n');
    line(rep, s);
    kv(rep, "pmu.pcie0", crate::platform::priv_reg(PMU_PCIE0, false, 0) as u32);
    if !all {
        line(rep, alloc::format!("a gate on the ch0 path is closed: stopping before any HSI1 register is read\n"));
        return false;
    }
    line(rep, alloc::format!("gph0 CON G#{:X} DAT G#{:X} | gph1 CON G#{:X} DAT G#{:X}\n", rd(GPH0), rd(GPH0 + 4), rd(GPH1), rd(GPH1 + 4)));
    line(rep, alloc::format!("gpa0 CON G#{:X} DAT G#{:X} | gpa6 CON G#{:X} DAT G#{:X} | gpa7 CON G#{:X} DAT G#{:X}\n", rd(GPA0), rd(GPA0 + 4), rd(GPA6), rd(GPA6 + 4), rd(GPA7), rd(GPA7 + 4)));
    cp2ap(rep, "cp2ap");
    kv(rep, "s2mpu_hsi1.version", rd(S2MPU_HSI1 + 0x60));
    kv(rep, "s2mpu_hsi1.prot_en", rd(S2MPU_HSI1 + 0x0));
    kv(rep, "sysmmu_hsi1.ctrl", rd(SYSMMU_HSI1));
    kv(rep, "elbi.ltssm", rd(ELBI + 0x2C8));
    kv(rep, "dbi.id", rd(DBI));
    kv(rep, "boot_stage", rd(BOOT_STAGE));
    true
}

/// Stage 1: power the CP from off (modem_ctrl_s5100.c power_on_cp: clear boot_stage, AP_ACTIVE 1, DUMP_NOTI 0, zero the message words, then gpio_power_off_cp and gpio_power_offon_cp with their delays), then watch the CP's pins for a second.
fn power_on(rep: &mut Vec<u8>) {
    wr(BOOT_STAGE, 0);
    for p in [CP2AP_WRST_N, PS_HOLD, CP2AP_WAKEUP, PHONE_ACTIVE] {
        p.func(0);
        p.pud(0);
    }
    PM_WRST_N.func(1);
    AP_ACTIVE.out(true);
    DUMP_NOTI.out(false);
    wr(IPC + 0x800, 0);
    wr(IPC + 0x804, 0);
    // Off: WAKEUP pulse, then everything low.
    AP2CP_WAKEUP.out(true);
    ms(10);
    AP2CP_WAKEUP.out(false);
    NRESET.out(false);
    CP_WRST_N.out(false);
    CP_PWR.out(false);
    ms(30);
    PM_WRST_N.out(false);
    ms(50);
    // On.
    PM_WRST_N.out(true);
    ms(10);
    CP_PWR.out(true);
    ms(10);
    NRESET.out(true);
    ms(10);
    CP_WRST_N.out(true);
    let t0 = now();
    let mut last = 0xFFu8;
    while since_us(t0) < 1_000_000 {
        crate::platform::pet_watchdogs();
        let s = PS_HOLD.get() as u8 | (CP2AP_WRST_N.get() as u8) << 1 | (CP2AP_WAKEUP.get() as u8) << 2 | (PHONE_ACTIVE.get() as u8) << 3;
        if s != last {
            line(rep, alloc::format!("+A#{} ms: PS_HOLD {} CP_WRST_N {} WAKEUP {} PHONE_ACTIVE {}\n", since_us(t0) / 1000, s & 1, (s >> 1) & 1, (s >> 2) & 1, (s >> 3) & 1));
            last = s;
        }
        us(1000);
    }
}

/// Stage 2: PHY. Pins (PERST low, CLKREQ function 2 pull-up), SOC+G#4000 = G#15, PMU PHY isolation released, the ch0 power-down clear, then the ch0 PHY configuration with its lock polls (logged, not fatal, as Linux does).
fn phy(rep: &mut Vec<u8>) {
    for off in [0x10, 0x14, 0x18] {
        wr(ELBI + off, 0);
    }
    PERST.out(false);
    CLKREQ.func(2);
    CLKREQ.pud(3);
    let r = crate::platform::priv_rmw(PMU_PCIE0, 1, 1);
    kv(rep, "pmu.pcie0 isolation release ret", r as u32);
    wr(SOC + 0x4000, 0x15);
    // phy_all_pwrdn_clear, ch0 branch.
    clr(UDBG + 0xC700, 1 << 1);
    us(100);
    clr(PHY + 0x600, 3 << 4);
    wr(PHY + 0x00C, 0);
    wr(PHY + 0x928, 0x55);
    wr(PHY + 0x580, 0x02);
    wr(PHY + 0x208, 0);
    wr(PHY + 0x228, 0);
    for off in [0x1044, 0x1048, 0x104C, 0x1050, 0x185C, 0x2044, 0x2048, 0x204C, 0x2050, 0x285C] {
        wr(PHY + off, 0);
    }
    us(10);
    // exynos_pcie_rc_pcie_phy_config, ch0 branch.
    wr(PHY + 0x32C, 0x3);
    clr(UDBG + 0xC710, 1 << 1);
    clr(UDBG + 0xC700, 1 << 1);
    match poll(UDBG + 0xC704, 1 << 3, 1 << 3, 100_000) {
        Some(t) => line(rep, alloc::format!("ext_pll_lock after A#{t} us\n")),
        None => line(rep, alloc::format!("ext_pll_lock TIMEOUT (G#{:X})\n", rd(UDBG + 0xC704))),
    }
    clr(SOC + 0x4004, 1 << 2);
    set(SOC + 0x4004, 0x3 << 4);
    wr(ELBI + 0x080, 0x4);
    wr(ELBI + 0x3A4, 0xF);
    wr(ELBI + 0x3A4, 0xD);
    us(10);
    wr(ELBI + 0x3A4, 0xF);
    us(10);
    for v in [1u32, 0] {
        wr(ELBI + 0x1404, v);
        wr(ELBI + 0x1408, v);
        wr(ELBI + 0x1400, v);
    }
    wr(ELBI + 0x3D8, 1);
    const COMMON: &[(usize, u32)] = &[
        (0x0000, 0x88), (0x001C, 0x66), (0x01F4, 0x00), (0x0510, 0x81), (0x0514, 0x59), (0x051C, 0x11), (0x062C, 0x0E), (0x0644, 0x22), (0x0688, 0x03), (0x06D4, 0x28), (0x0788, 0x64), (0x078C, 0x64), (0x0790, 0x50), (0x0794, 0x50), (0x0944, 0x05), (0x0948, 0x05), (0x094C, 0x05), (0x0950, 0x05),
        (0x0590, 0x02), (0x07F8, 0xB0), (0x0730, 0x08), (0x0344, 0xC0), (0x0040, 0x04), (0x0204, 0x03), (0x02D4, 0x1F), (0x0358, 0x10), (0x0018, 0x01),
        (0x0514, 0x5B), (0x0608, 0x0C), (0x060C, 0x0F), (0x0610, 0x0F), (0x0614, 0x0F), (0x0618, 0x0F), (0x0510, 0x80), (0x0688, 0x03), (0x0644, 0x23), (0x0624, 0x11), (0x0630, 0x0F), (0x06D0, 0x53),
    ];
    for &(off, v) in COMMON {
        wr(PHY + off, v);
    }
    const LANE: &[(usize, u32)] = &[
        (0x1140, 0x04), (0x1144, 0x04), (0x1148, 0x04), (0x114C, 0x02), (0x1150, 0), (0x1154, 0), (0x1158, 0), (0x115C, 0), (0x12CC, 0x1C), (0x12DC, 0x6C), (0x130C, 0x29), (0x13B4, 0x2F), (0x1A64, 0x05), (0x1A68, 0x05), (0x1A84, 0x05), (0x1A88, 0x05), (0x1A98, 0), (0x1A9C, 0), (0x1AA8, 0x07), (0x1AB8, 0), (0x1ABC, 0), (0x1AF8, 0x90), (0x1B34, 0x03), (0x1BB0, 0x03), (0x1BB4, 0x03), (0x1BC0, 0x06), (0x1BC4, 0x06), (0x1BE8, 0x01), (0x1BF8, 0x04), (0x1C98, 0), (0x1CA4, 0x81),
    ];
    // Two lanes, lane n at +n·G#1000.
    for lane in 0..2 {
        for &(off, v) in LANE {
            wr(PHY + lane * 0x1000 + off, v);
        }
    }
    const PCSCFG: &[(usize, u32)] = &[(0x190, 0x100B_0604), (0x154, 0x0007_00D5), (0x100, 0x1640_0000), (0x104, 0x0860_0000), (0x114, 0x1850_0000), (0x124, 0x6070_0000), (0x174, 0x7), (0x178, 0x100), (0x17C, 0x10)];
    for &(off, v) in PCSCFG {
        wr(PCS + off, v);
    }
    wr(ELBI + 0x1404, 1);
    us(10);
    wr(ELBI + 0x1408, 1);
    wr(ELBI + 0x1400, 1);
    for (name, off, bit) in [("pll_lock", 0xA80usize, 0u32), ("cdr_lock", 0x15C0, 4), ("ofs_cal", 0x140C, 7)] {
        match poll(PHY + off, 1 << bit, 1 << bit, 100_000) {
            Some(t) => line(rep, alloc::format!("{name} after A#{t} us\n")),
            None => line(rep, alloc::format!("{name} TIMEOUT (G#{:X})\n", rd(PHY + off))),
        }
    }
    let v = rd(UDBG + 0xC710);
    wr(UDBG + 0xC710, (v & !(3 << 8)) | 3 << 8);
    wr(PHY + 0x32C, 0);
    wr(ELBI + 0x1078, 1);
}

/// Stage 3: root complex setup, PERST, LTSSM to L0 at Gen1 x1 (exynos_pcie_rc_establish_link; the ch1 port in payloads/pcie1 stage 2 with ch0's PMU and UDBG differences).
fn link(rep: &mut Vec<u8>) -> bool {
    let r = crate::platform::priv_rmw(PMU_PCIE0, 0x400, 0x400);
    kv(rep, "pmu.pcie0 bit10 ret", r as u32);
    clr(UDBG + 0xC800, 3 << 5);
    wr(PHY + 0x32C, 0);
    wr(DBI + 0x890, 0x1_2000);
    PERST.out(true);
    ms(18);
    set(ELBI + 0x3BC, 0x11);
    wr(ELBI + 0x3A0, 0x2);
    wr(ELBI + 0x074, 1);
    clr(ELBI + 0x3A8, 0xFFF);
    wr(ELBI + 0x474, 1);
    wr(ELBI + 0x1078, 1);
    // dw_pcie_setup_rc, Gen1 x1.
    set(DBI + 0x8BC, 1);
    let v = rd(DBI + 0xA0);
    wr(DBI + 0xA0, (v & !0xF) | 1);
    let v = rd(DBI + 0x7C);
    wr(DBI + 0x7C, (v & !0xF) | 1);
    let v = rd(DBI + 0x710);
    wr(DBI + 0x710, (v & !((1 << 7) | (0x3F << 16))) | (1 << 5) | (1 << 16));
    let v = rd(DBI + 0x80C);
    wr(DBI + 0x80C, (v & !(0x1F << 8)) | (1 << 8));
    wr(DBI + 0x10, 4);
    wr(DBI + 0x14, 0);
    let v = rd(DBI + 0x3C);
    wr(DBI + 0x3C, (v & !(0xFF << 8)) | (1 << 8));
    let v = rd(DBI + 0x18);
    wr(DBI + 0x18, (v & 0xFF00_0000) | 0x00FF_0100);
    let v = rd(DBI + 0x4);
    wr(DBI + 0x4, (v & !0xFFFF) | 0x107);
    wr(DBI + 0x10, 0);
    let v = rd(DBI + 0x8);
    wr(DBI + 0x8, (v & 0xFFFF) | (0x0604 << 16));
    clr(DBI + 0x8BC, 1);
    // exynos_pcie_setup_rc.
    wr(DBI + 0x8BC, 1);
    wr(DBI, 0xECED_144D);
    let v = rd(DBI + 0x7C);
    wr(DBI + 0x7C, (v & !((7 << 15) | 0xF)) | 0x3_8000 | 1);
    wr(DBI + 0xB40, 0x1A);
    wr(DBI + 0xB44, 0xEA);
    wr(DBI + 0x44, 0);
    let v = rd(DBI + 0x98);
    wr(DBI + 0x98, (v & !0xF) | 5);
    // IO coherency as Linux sets it (exynos_pcie_rc_set_iocc).
    wr(DBI + 0x8E8, 0x1010_1010);
    let v = rd(SYSREG_HSI1 + 0x704);
    wr(SYSREG_HSI1 + 0x704, (v & !3) | 3);
    clr(ELBI + 0xC00, 1 << 2);
    wr(ELBI + 0xC04, 0xFFFF_FFFF);
    wr(ELBI + 0xC08, 0xFFFF_FFFF);
    set(ELBI + 0xC00, 3);
    kv(rep, "dbi.id", rd(DBI));
    wr(ELBI + 0x054, 1);
    let t0 = now();
    let mut last = 0xFFu32;
    let mut l0 = false;
    let mut trace = alloc::string::String::from("ltssm:");
    while since_us(t0) < 100_000 {
        let s = rd(ELBI + 0x2C8) & 0x3F;
        if s != last {
            trace.push_str(&alloc::format!(" G#{s:X}"));
            last = s;
        }
        if s == 0x11 {
            l0 = true;
            break;
        }
        us(10);
    }
    trace.push('\n');
    line(rep, trace);
    line(rep, alloc::format!("{} after A#{} us; link status G#{:X}\n", if l0 { "LINK UP (L0)" } else { "no L0" }, since_us(t0), rd(DBI + 0x80)));
    l0
}

fn cfg_rd(off: usize) -> u32 {
    wr(ELBI + 0x074, 1);
    let v = rd(CFG + off);
    wr(ELBI + 0x074, 0);
    v
}
fn cfg_wr(off: usize, v: u32) {
    wr(ELBI + 0x074, 1);
    wr(CFG + off, v);
    wr(ELBI + 0x074, 0);
}

/// Stage 4: iATU (CFG0 to bus 1, MEM CPU G#4000_0000 -> bus G#14E0_0000), the CP's config space (expect vendor G#144D), BAR0 = the doorbell's bus address, the bridge window, memory + bus master, and the CP's MSI pointed at the MSI page (s51xx_pcie_probe; MSI capability at G#50).
fn endpoint(rep: &mut Vec<u8>) -> bool {
    for (off, v) in [(0x08usize, CFG as u32), (0x0C, 0), (0x10, CFG as u32 + 0x1FFF), (0x14, 0x0100_0000), (0x18, 0), (0x00, 0x4), (0x04, 0x8000_0000)] {
        wr(ATU + off, v);
    }
    for (off, v) in [(0x200usize, 0u32), (0x208, MEM_CPU), (0x20C, 0), (0x210, MEM_CPU + 0x00FE_FFFF), (0x214, MEM_BUS), (0x218, 0), (0x204, 0x8000_0000)] {
        wr(ATU + off, v);
    }
    let id = cfg_rd(0);
    kv(rep, "cp.vendor_device", id);
    if id & 0xFFFF != 0x144D {
        line(rep, alloc::format!("not the Samsung CP: stopping\n"));
        return false;
    }
    cfg_wr(0x10, DOORBELL_BUS);
    let bar = cfg_rd(0x10);
    kv(rep, "cp.bar0 readback", bar);
    let db = MEM_CPU as usize + (DOORBELL_BUS - (bar & !0xF)) as usize;
    DOORBELL.store(db, core::sync::atomic::Ordering::Relaxed);
    kv(rep, "doorbell at", db as u32);
    // Bridge memory window around bus G#14E0_0000..G#14EF_FFFF.
    wr(DBI + 0x20, (MEM_BUS + 0x000F_0000) | (MEM_BUS >> 16));
    let cmd = cfg_rd(0x4);
    cfg_wr(0x4, cmd | 0x6);
    // MSI capability (s51xx: control G#50, address G#54, data G#58): the MSI page, 4 vectors, enabled; the RC's receiver at the same address.
    let msi = cfg_rd(0x50);
    kv(rep, "cp.msi_cap", msi);
    if msi & 0xFF == 0x05 {
        cfg_wr(0x54, MSI_PAGE as u32);
        cfg_wr(0x58, 0);
        cfg_wr(0x50, (msi & !(7 << 20)) | (2 << 20) | (1 << 16));
    }
    wr(DBI + 0x820, MSI_PAGE as u32);
    wr(DBI + 0x824, 0);
    wr(DBI + 0x828, 0xFFFF_FFFF);
    wr(DBI + 0x82C, 0);
    kv(rep, "cp.command", cfg_rd(0x4));
    true
}

/// Stage 5: the ROM boot (start_normal_boot + set_cp_rom_boot_img): HSI1 S2MPU protection off so the CP can reach AP memory, BOOT section from modem.bin into G#EA41_0000, the control words and magic G#BDBD, the image address/size into the MSI page, doorbell G#1_0000, poll boot_stage for G#3FFF.
fn rom_boot(rep: &mut Vec<u8>) {
    // S2MPU v9: CTRL_PROT_EN_PER_VID_CLR (G#54) = all VIDs unprotected (cold_ufs did this for HSI2 before the fence).
    if rd(S2MPU_HSI1 + 0x60) & 0xFFFF_0000 == 0x9000_0000 {
        wr(S2MPU_HSI1 + 0x54, 0xFF);
        kv(rep, "s2mpu_hsi1.prot_en after clear", rd(S2MPU_HSI1));
    } else {
        kv(rep, "s2mpu_hsi1 not v9, version", rd(S2MPU_HSI1 + 0x60));
    }
    let mut m = match crate::partfs::mount(MODEM_PART) {
        Ok(m) => m,
        Err(e) => return line(rep, alloc::format!("{e}\n")),
    };
    let ino = match m.open(MODEM_BIN) {
        Ok(i) => i,
        Err(e) => return line(rep, alloc::format!("{e}\n")),
    };
    let mut head = alloc::vec![0u8; 0x1_8000];
    if let Err(e) = m.read(&ino, 0, &mut head) {
        return line(rep, alloc::format!("{e}\n"));
    }
    let Some(boot) = toc(&head[..4096]).into_iter().find(|s| s.name() == "BOOT") else {
        return line(rep, alloc::format!("no BOOT in the TOC\n"));
    };
    let (o, n) = (boot.offset as usize, boot.size as usize);
    if o + n > head.len() {
        return line(rep, alloc::format!("BOOT outside the first G#{:X} bytes\n", head.len()));
    }
    for (i, w) in head[o..o + n].chunks(4).enumerate() {
        let mut b = [0u8; 4];
        b[..w.len()].copy_from_slice(w);
        wr(ROM_IMG + i * 4, u32::from_le_bytes(b));
    }
    line(rep, alloc::format!("BOOT G#{n:X} bytes at G#{ROM_IMG:X}\n"));
    // init_control_messages, as captured: srinfo offset, capability offset, capability words, united status (DS detect G#4000).
    wr(IPC + 0x64, 0x40_0000);
    wr(IPC + 0x70, 0xA0);
    for off in [0xA0, 0xA4, 0xA8, 0xAC] {
        wr(IPC + off, 0);
    }
    wr(IPC + 0x808, 0x4000);
    wr(IPC + 0x80C, 0);
    wr(IPC, 0xBDBD);
    kv(rep, "magic readback", rd(IPC));
    wr(MSI_PAGE + 0x14, ROM_IMG as u32);
    wr(MSI_PAGE + 0x18, 0);
    wr(MSI_PAGE + 0x1C, n as u32);
    wr(BOOT_STAGE, 0);
    unsafe { core::arch::asm!("dsb sy") };
    wr(DOORBELL.load(core::sync::atomic::Ordering::Relaxed), 0x1_0000);
    let t0 = now();
    let mut last = 0xFFFF_FFFFu32;
    let mut trace = alloc::string::String::from("boot_stage:");
    while since_us(t0) < 4_000_000 {
        crate::platform::pet_watchdogs();
        let s = rd(BOOT_STAGE);
        if s != last {
            trace.push_str(&alloc::format!(" G#{s:X}@A#{}ms", since_us(t0) / 1000));
            last = s;
        }
        if s == 0x3FFF {
            break;
        }
        us(1000);
    }
    trace.push('\n');
    line(rep, trace);
    line(rep, alloc::format!("{}; msi status G#{:X}, cp2ap_msg G#{:X}, err_report G#{:X}\n", if last == 0x3FFF { "CP ROM DONE (G#3FFF)" } else { "boot_stage did not reach G#3FFF" }, rd(DBI + 0x830), rd(IPC + 0x804), rd(MSI_PAGE + 0x8)));
    cp2ap(rep, "cp2ap");
}

/// `diag modem sN`: stages 0..=N.
pub fn stages(rep: &mut Vec<u8>, n: u8) {
    if !survey(rep) || n < 1 {
        return;
    }
    power_on(rep);
    if n < 2 {
        return;
    }
    ms(200);
    phy(rep);
    if n < 3 || !link(rep) || n < 4 || !endpoint(rep) || n < 5 {
        return;
    }
    rom_boot(rep);
}
