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
/// The per-unit NV files (f2fs): the boot channel's NV_NORM, NV_PROT and REPLAY sections.
pub const NV_FILES: [(&str, &[&str]); 2] = [("efs", &["/nv_normal.bin", "/nv_protected.bin"]), ("modem_userdata", &["/replay_region.bin"])];
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

/// Every modem line goes to the flash log as well as the report: a stage that hangs the bus loses its PT response to the watchdog reset, and the flash log is on the flash before the call returns (2026-10-20, the link cycle).
fn line(rep: &mut Vec<u8>, s: alloc::string::String) {
    rep.extend_from_slice(s.as_bytes());
    crate::flashlog::line(s.trim_end());
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
    // The per-unit NV files on f2fs: sizes, and a short BLAKE3 of each to compare with an image of the partition (the contents are per-unit and never printed).
    for (part, files) in NV_FILES {
        match crate::partfs::mount_f2fs(part) {
            Err(e) => line(rep, alloc::format!("{e}\n")),
            Ok(mut fs) => {
                line(rep, alloc::format!("{part}: f2fs, checkpoint version G#{:X}\n", fs.checkpoint_ver));
                if let Ok(root) = fs.root() {
                    let mut names = Vec::new();
                    let _ = fs.list(&root, &mut |n, ino| names.push((alloc::string::String::from_utf8_lossy(n).into_owned(), ino)));
                    line(rep, alloc::format!("{part}/: {}\n", names.iter().map(|(n, i)| alloc::format!("{n} (ino A#{i})")).collect::<Vec<_>>().join(", ")));
                }
                for f in files.iter() {
                    match crate::partfs::read_f2fs(part, f) {
                        Ok(v) => line(rep, alloc::format!("{part}{f}: A#{} bytes, BLAKE3 {}\n", v.len(), &blake3::hash(&v).to_hex()[..16])),
                        Err(e) => line(rep, alloc::format!("{e}\n")),
                    }
                }
            }
        }
    }
    let ms = (crate::platform::cntpct() - t0) / (crate::platform::cntfrq() / 1000);
    line(rep, alloc::format!("A#{ms} ms\n"));
}

// ---------------------------------------------------------------------------
// The handover block: `struct t_handover_block_info` (modem_prj.h 179-196), G#A1 bytes at ap2cp_handover_block_info (IPC+G#82C), which cbd fills before the boot (`update_handover_block_info`, decoded from cbd's own symbols 2026-10-29; both captures show its memcpy first of all). It tells the CP which board it is in, so it can pick its RF configuration. Layout and sources, for a normal boot of a user build:
//   G#00 version 1 · G#04 project_id = cdt[0] (platform) · G#08 revision = `hwinfo` from hardware_config.json · G#0C major_id = cdt[3] · G#10 minor_id = cdt[4] · G#14 modem_sku = cdt[6] · G#18 modem_hw = cdt[7] · G#1C cpinfo0 = 0 (set only for factory boot modes) · G#20 cpinfo1 = 0 (set only after an EFS clear) · G#24 cpinfo2 = /chosen/config/modem_flag · G#28 rf_sub = cdt[8] · G#2C rf_config = `rfid` from hardware_config.json, else /chosen/plat/rfid · G#30, G#34, G#38 reserved = cdt[1], cdt[2], cdt[5] · G#3C reserved = 0 (user build) · G#40, G#50 cpid = /chosen/config/imei1, imei2 (16 bytes each) · G#60 cpsig = /mnt/vendor/persist/modem/cpsha (64 bytes), then a NUL.
// cdt[] is the bootconfig's `androidboot.cdt_hwid`, read by cbd as "0x%04x%02x%02x%04x%02x%02x%02x%02x%04x%08x". husky's radio build carries no hardware_config.json (its /images/default holds modem.bin, uecap, pw_token_db and confpack), so cbd's lookup finds nothing: revision stays 0 and rf_config comes from the device tree, as here.
// The block is assembled in place at a real boot (`read_board`), while ABL's tree is still there: a RELOAD has no tree, and the IPC page keeps the block across reloads. cpsig is added by the ROM stage, which has the disk. The two IMEIs are this phone's identity: they go from ABL's device tree to the CP's own memory and are never printed, logged or stored.

const HANDOVER: usize = IPC + 0x82C;
const HANDOVER_LEN: usize = 0xA1;
const CPSHA: &str = "/modem/cpsha";

/// The bootconfig ABL appends to the initrd (slot.rs: the text, then its size and checksum, then "#BOOTCONFIG\n", ending at /chosen/linux,initrd-end).
fn bootconfig(dtb: &ferros_hal::dtb::Dtb) -> Option<&'static [u8]> {
    let prop = dtb.find_node_prop(b"chosen", b"linux,initrd-end")?;
    let end = if prop.data.len() >= 8 { prop.as_u64() } else { prop.as_u32().map(|v| v as u64) }? as usize;
    if end < 20 {
        return None;
    }
    let tail = unsafe { core::slice::from_raw_parts((end - 20) as *const u8, 20) };
    if &tail[8..] != b"#BOOTCONFIG\n" {
        return None;
    }
    let size = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as usize;
    // External data (ABL's): a size past the initrd's own end would read below it.
    if size == 0 || size > 64 * 1024 || size > end - 20 {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts((end - 20 - size) as *const u8, size) })
}

/// The last `key = value` in the bootconfig (`:=` overrides, last wins), quotes and spaces dropped.
fn bootconfig_value(text: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    let at = text.windows(key.len()).rposition(|w| w == key)?;
    let line = &text[at + key.len()..];
    let line = &line[..line.iter().position(|&c| c == b'\n').unwrap_or(line.len())];
    let eq = line.iter().position(|&c| c == b'=')?;
    Some(line[eq + 1..].iter().filter(|&&c| c != b' ' && c != b'"').copied().collect())
}

fn hex_digit(c: u8) -> Option<u32> {
    (c as char).to_digit(16)
}

/// Assemble the handover block at IPC+G#82C from ABL's device tree and bootconfig (a real boot only). Every field cbd leaves at 0 on a normal boot is 0; cpsig is zero until the ROM stage fills it.
pub fn read_board(dtb_addr: u64) {
    let Some(dtb) = (unsafe { ferros_hal::dtb::Dtb::from_ptr(dtb_addr as *const u8) }) else {
        return;
    };
    let mut b = [0u8; HANDOVER_LEN];
    let mut put = |off: usize, v: u32| b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    put(0x00, 1);
    if let Some(v) = bootconfig(&dtb).and_then(|t| bootconfig_value(t, b"androidboot.cdt_hwid")) {
        // "0x" then the ten fields at their fixed widths (cbd's sscanf format): 4 2 2 4 2 2 2 2 4 8 hex digits.
        let digits = v.strip_prefix(b"0x").unwrap_or(&v);
        let widths = [4usize, 2, 2, 4, 2, 2, 2, 2, 4, 8];
        if digits.len() == widths.iter().sum::<usize>() && digits.iter().all(|&c| hex_digit(c).is_some()) {
            let mut cdt = [0u32; 10];
            let mut at = 0;
            for (k, w) in widths.iter().enumerate() {
                cdt[k] = digits[at..at + w].iter().fold(0u32, |a, &c| a << 4 | hex_digit(c).unwrap());
                at += w;
            }
            for (off, k) in [(0x04, 0), (0x0C, 3), (0x10, 4), (0x14, 6), (0x18, 7), (0x28, 8), (0x30, 1), (0x34, 2), (0x38, 5)] {
                put(off, cdt[k]);
            }
        }
    }
    if let Some(v) = dtb.find_node_prop(b"plat", b"rfid").and_then(|p| p.as_u32()) {
        put(0x2C, v);
    }
    if let Some(p) = dtb.find_node_prop(b"config", b"modem_flag") {
        // fscanf "%x": optional spaces, an optional 0x, then hex digits up to the first other byte.
        let s = p.data.iter().skip_while(|&&c| c == b' ').copied().collect::<Vec<u8>>();
        let s = s.strip_prefix(b"0x").unwrap_or(&s);
        let n = s.iter().take_while(|&&c| hex_digit(c).is_some()).count();
        put(0x24, s[..n].iter().fold(0u32, |a, &c| a << 4 | hex_digit(c).unwrap()));
    }
    for (off, name) in [(0x40, b"imei1"), (0x50, b"imei2")] {
        if let Some(p) = dtb.find_node_prop(b"config", name) {
            // The property's length is ABL's; the block field is 16 bytes and cbd reads exactly 16, so a longer property is cut there and a shorter one zero-filled.
            let n = p.data.len().min(16);
            b[off..off + n].copy_from_slice(&p.data[..n]);
        }
    }
    for (i, &x) in b.iter().enumerate() {
        wr8(HANDOVER + i, x);
    }
    unsafe { core::arch::asm!("dsb sy") };
}

/// cpsig: the 64 bytes of persist's /modem/cpsha into the block (zero when the file is absent, as cbd leaves it), before the ROM boot.
fn handover_cpsig(rep: &mut Vec<u8>) {
    match crate::partfs::read_f2fs("persist", CPSHA) {
        Ok(v) if v.len() >= 64 => {
            for (i, &x) in v[..64].iter().enumerate() {
                wr8(HANDOVER + 0x60 + i, x);
            }
            wr8(HANDOVER + 0xA0, 0);
            line(rep, alloc::format!("handover block: version A#{}, cpsig from persist{CPSHA}\n", rd(HANDOVER)));
        }
        Ok(v) => line(rep, alloc::format!("handover block: persist{CPSHA} holds A#{} bytes, cpsig left zero\n", v.len())),
        Err(e) => line(rep, alloc::format!("handover block: {e}, cpsig left zero\n")),
    }
}

/// `diag modem hwcfg`: the handover block as it stands at IPC+G#82C — its board words in full, the IMEI and cpsig fields only as set or zero.
pub fn hwcfg(rep: &mut Vec<u8>) {
    let names = ["version", "project_id", "revision", "major_id", "minor_id", "modem_sku", "modem_hw", "cpinfo0", "cpinfo1", "cpinfo2", "rf_sub", "rf_config", "reserved0", "reserved1", "reserved2", "reserved3"];
    for (k, n) in names.iter().enumerate() {
        line(rep, alloc::format!("{n}: G#{:X}\n", rd(HANDOVER + 4 * k)));
    }
    let set = |off: usize, len: usize| (0..len).any(|i| rd8(HANDOVER + off + i) != 0);
    line(rep, alloc::format!("cpid 0, cpid 1, cpsig: {}, {}, {} (not shown)\n", ["zero", "set"][set(0x40, 16) as usize], ["zero", "set"][set(0x50, 16) as usize], ["zero", "set"][set(0x60, 64) as usize]));
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
    // The PMU power-domain status word is a plain read: EL3's priv-reg read refuses it (-22, measured here and on pd_ispfe / pd_hsi1 in amp.rs).
    let pd = rd(PD_HSI1_STATUS as usize);
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
    kv(rep, "pmu.pcie0", rd(PMU_PCIE0 as usize));
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
    line(rep, alloc::format!("after the sequence: gph0 CON G#{:X} DAT G#{:X} | gph1 CON G#{:X} DAT G#{:X} | gpa0 CON G#{:X} DAT G#{:X}\n", rd(GPH0), rd(GPH0 + 4), rd(GPH1), rd(GPH1 + 4), rd(GPA0), rd(GPA0 + 4)));
    line(rep, alloc::format!("AP2CP now: CP_PWR {} NRESET {} CP_WRST_N {} AP_ACTIVE {} PM_WRST_N {} WAKEUP {} DUMP_NOTI {}\n", CP_PWR.get() as u8, NRESET.get() as u8, CP_WRST_N.get() as u8, AP_ACTIVE.get() as u8, PM_WRST_N.get() as u8, AP2CP_WAKEUP.get() as u8, DUMP_NOTI.get() as u8));
    let t0 = now();
    let mut last = 0xFFu8;
    while since_us(t0) < 3_000_000 {
        crate::platform::pet_watchdogs();
        let s = PS_HOLD.get() as u8 | (CP2AP_WRST_N.get() as u8) << 1 | (CP2AP_WAKEUP.get() as u8) << 2 | (PHONE_ACTIVE.get() as u8) << 3;
        if s != last {
            line(rep, alloc::format!("+A#{} ms: PS_HOLD {} CP_WRST_N {} WAKEUP {} PHONE_ACTIVE {}\n", since_us(t0) / 1000, s & 1, (s >> 1) & 1, (s >> 2) & 1, (s >> 3) & 1));
            last = s;
        }
        us(1000);
    }
}

/// `diag modem off`: the CP powered down, the first half of `power_on` (the WAKEUP pulse, NRESET, CP_WRST_N, CP_PWR and PM_WRST_N low). The PCIe side is left as it is: only the modem diags touch it, and each starts from the survey.
pub fn off(rep: &mut Vec<u8>) {
    AP2CP_WAKEUP.out(true);
    ms(10);
    AP2CP_WAKEUP.out(false);
    NRESET.out(false);
    CP_WRST_N.out(false);
    CP_PWR.out(false);
    ms(30);
    PM_WRST_N.out(false);
    AP_ACTIVE.out(false);
    line(rep, alloc::format!("CP off: CP_PWR {} NRESET {} CP_WRST_N {} PM_WRST_N {}; PS_HOLD {}\n", CP_PWR.get() as u8, NRESET.get() as u8, CP_WRST_N.get() as u8, PM_WRST_N.get() as u8, PS_HOLD.get() as u8));
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
    link_at(rep, 1, 1)
}

/// `speed` = 1 or 3 (LNKCAP/LNKCTL2 target speed), `lanes` = 1 or 2. The ROM stage trains at Gen1 x1; after the CP jumps to the image it pulled, its endpoint comes back as Gen3 x2 and will not train at Gen1 x1 (measured 2026-10-20: eight attempts bounce POLL_ACTIVE <-> POLL_CONFIG, DL Link Active never sets).
fn link_at(rep: &mut Vec<u8>, speed: u32, lanes: u32) -> bool {
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
    wr(DBI + 0xA0, (v & !0xF) | speed);
    let v = rd(DBI + 0x7C);
    wr(DBI + 0x7C, (v & !0xF) | speed);
    // PORT_LINK_CONTROL's link mode is a mask, not a count (dw_pcie_setup): 1 lane = 1, 2 lanes = 3.
    let mode = if lanes >= 2 { 3 } else { 1 };
    let v = rd(DBI + 0x710);
    wr(DBI + 0x710, (v & !((1 << 7) | (0x3F << 16))) | (1 << 5) | (mode << 16));
    let v = rd(DBI + 0x80C);
    wr(DBI + 0x80C, (v & !(0x1F << 8)) | (lanes << 8));
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
    wr(DBI + 0x7C, (v & !((7 << 15) | 0xF)) | 0x3_8000 | speed);
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
    // The CP's endpoint may not answer configuration at once after the link trains: all ones until it does. Up to 256 ms (2026-10-24 night: G#FFFFFFFF 113 µs after L0, twice).
    let t0 = now();
    let mut id = cfg_rd(0);
    while id == 0xFFFF_FFFF && since_us(t0) < 256_000 {
        ms(1);
        id = cfg_rd(0);
    }
    line(rep, alloc::format!("config answered after A#{} us\n", since_us(t0)));
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
    // ap2cp_united_status: zeroed, then the dual-SIM detect field (bits [15:14], sbi_ds_det_pos 14) set to `ds_detect - 1` = 1, Linux's default of two SIMs (modem_ctrl_s5100.c get_ds_detect) — both captures show the pair, G#0 then G#4000 (an earlier reading of them missed the second write).
    wr(IPC + 0x808, 0);
    wr(IPC + 0x80C, 0);
    wr(IPC + 0x808, 1 << 14);
    // ap2cp_kerneltime (G#824): on a cold boot whatever the DRAM held (measured G#FF with flipped bits); zero. The handover block after it was assembled at boot (`read_board`); its cpsig comes from the disk now.
    wr(IPC + 0x824, 0);
    handover_cpsig(rep);
    // `init_legacy_link` (link_device_memory_legacy.c:228): magic and mem_access to 0, every ring's head and tail to 0, then the magic and mem_access = 1, both read back. On a cold boot the cursors are whatever the DRAM held (measured G#FFFF_FFFF), and the first frame has nowhere to go.
    wr(IPC, 0);
    wr(IPC + 0x4, 0);
    for off in [0x8, 0xC, 0x10, 0x14, 0x18, 0x1C, 0x20, 0x24] {
        wr(IPC + off, 0);
    }
    wr(IPC, 0xBDBD);
    wr(IPC + 0x4, 1);
    unsafe { core::arch::asm!("dsb sy") };
    line(rep, alloc::format!("magic G#{:X} mem_access G#{:X}; rings zeroed\n", rd(IPC), rd(IPC + 0x4)));
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
    if n < 6 || rd(BOOT_STAGE) != 0x3FFF || !link_cycle(rep) || n < 7 {
        return;
    }
    let mut seq = None;
    download(rep, &mut seq);
    match (n, seq) {
        (8, Some(s)) => start_and_watch(rep, s),
        (9, Some(s)) => start_and_handshake(rep, s, false, 0),
        (10, Some(s)) => start_and_handshake(rep, s, true, 0),
        (11, Some(s)) => start_and_handshake(rep, s, true, 1),
        (12, Some(s)) => start_and_handshake(rep, s, true, 2),
        (13, Some(s)) => start_and_handshake(rep, s, true, 3),
        (14, Some(s)) => start_and_handshake(rep, s, true, 4),
        (n, Some(s)) if n >= 15 => start_and_handshake(rep, s, true, 5),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Stages 6-7: the link cycle the CP's jump to its downloaded bootloader forces, then the boot-channel download (MODEM-BOOT-RECON.md "Boot channel G#F1").

/// RAW ring cursors in the IPC region: the AP writes at the tx head and reads at the rx tail.
const TXQ_HEAD: usize = IPC + 0x18;
const TXQ_TAIL: usize = IPC + 0x1C;
const RXQ_HEAD: usize = IPC + 0x20;
const RXQ_TAIL: usize = IPC + 0x24;
const TX_RING: usize = IPC + 0x3000;
const TX_RING_LEN: u32 = 0x1F_D000;
const RX_RING: usize = IPC + 0x20_0000;
const RX_RING_LEN: u32 = 0x20_0000;
/// The FMT ring pair, the other half of the legacy link: the CP answers our boot-channel frames on RAW but announces itself on FMT (its `cp2ap_msg` reads G#83 = INT_VALID | SEND_FMT | SEND_RAW).
const FMT_RXQ_HEAD: usize = IPC + 0x10;
const FMT_RXQ_TAIL: usize = IPC + 0x14;
const FMT_RX_RING: usize = IPC + 0x2000;
const FMT_RX_RING_LEN: u32 = 0x1000;
const AP2CP_MSG: usize = IPC + 0x800;
/// `mask2int(MASK_SEND_DATA)` (link_device_memory.h): data waiting in a ring.
const INT_SEND_DATA: u32 = 0x0081;
/// Doorbell values: `int_ap2cp_* | G#1_0000` — message 0, PCIe link ack G#E.
const DB_MSG: u32 = 0x1_0000;
const DB_LINK_ACK: u32 = 0x1_000E;
/// The boot channel.
const CH_BOOT: u8 = 0xF1;
/// Largest data bytes in one frame, as the capture shows the daemon using.
const CHUNK: usize = 0xC000;

fn wr8(a: usize, v: u8) {
    unsafe { core::ptr::write_volatile(a as *mut u8, v) }
}
fn rd8(a: usize) -> u8 {
    unsafe { core::ptr::read_volatile(a as *const u8) }
}

/// The ch0 PHY power-down (`exynos_pcie_rc_phy_all_pwrdn`, ch0 branch): what the RC does before PERST drops, so the PHY comes back cleanly on the next `phy()`.
fn phy_pwrdn() {
    clr(PHY + 0x204, 3 << 2);
    wr(PCS + 0x150, 0x3_00D9);
    us(5);
    for (off, v) in [(0x1044, 0x2A), (0x1048, 0xAA), (0x104C, 0xA8), (0x1050, 0x80), (0x185C, 0x0A)] {
        wr(PHY + off, v);
    }
    us(1);
    for (off, v) in [(0x2044, 0x2A), (0x2048, 0xAA), (0x204C, 0xA8), (0x2050, 0x80), (0x285C, 0x0A)] {
        wr(PHY + off, v);
    }
    us(1);
    wr(PHY + 0x208, 0xFF);
    wr(PHY + 0x228, 0xFF);
    us(1);
    wr(PHY + 0x580, 0x0A);
    wr(PHY + 0x928, 0xAA);
    wr(PHY + 0x00C, 0x0A);
    let v = rd(PHY + 0x600);
    wr(PHY + 0x600, v | 2 << 4);
    wr(PCS + 0x150, 0x3_00DE);
    us(5);
    for _ in 0..100 {
        if rd(PHY + 0x260) & 4 == 0 {
            break;
        }
        us(1);
    }
    set(UDBG + 0xC700, 1 << 1);
    us(10);
}

/// Drop the link the way `exynos_pcie_rc_poweroff` does (pcie-exynos-rc.c 4005-4050), which is more than PERST and the LTSSM: the PHY input clock goes back to the oscillator first, and a SOFT_PWR_RESET pulse (ELBI G#3A4 bit 1, 20 us low) runs before the PHY powers down. Without those two the CP's endpoint never retrains (2026-10-20).
fn link_down() {
    PERST.out(false);
    clr(SOC + 0x4000, 0x3);
    wr(ELBI + 0x054, 0);
    let v = rd(ELBI + 0x3A4);
    wr(ELBI + 0x3A4, v & !2);
    us(20);
    wr(ELBI + 0x3A4, v | 2);
    phy_pwrdn();
}

/// The PHY and the link, up to `tries` attempts: Gen3 x2 as the driver asks for after the ROM stage, then Gen1 x1, alternating. Each attempt is a full PHY configuration with the link dropped in between, and a run that fails them all leaves the PHY up so the next ELBI read does not stall the core.
fn link_tries(rep: &mut Vec<u8>, tries: u32) -> bool {
    for attempt in 1..=tries {
        let (speed, lanes) = if attempt % 2 == 1 { (3, 2) } else { (1, 1) };
        phy(rep);
        if link_at(rep, speed, lanes) {
            line(rep, alloc::format!("link up on attempt A#{attempt} (Gen{speed} x{lanes}); status G#{:X}\n", rd(DBI + 0x80)));
            return true;
        }
        link_down();
        ms(10);
    }
    phy(rep);
    PERST.out(true);
    line(rep, alloc::format!("no link in A#{tries} attempts; PHY left up so the registers stay readable\n"));
    false
}

/// Stage 6: the link between the ROM stage and the download (`start_normal_boot` steps f-h).
///
/// `s5100_poweroff_pcie` waits 30 ms and then refuses to drop the link if CP2AP_WAKEUP already reads 1 or a transmit is pending (`modem_ctrl_s5100.c:1661-1675`); otherwise it drops it, step (g) waits for the CP to raise CP2AP_WAKEUP (20 ms x 200), and step (h) brings it back at Gen3 x2 and rings the link acknowledgement.
fn link_cycle(rep: &mut Vec<u8>) -> bool {
    ms(30);
    if CP2AP_WAKEUP.get() {
        line(rep, alloc::format!("cycle: CP2AP_WAKEUP already high, link kept\n"));
    } else {
        link_down();
        line(rep, alloc::format!("cycle: link dropped, waiting for the CP\n"));
        let t0 = now();
        let mut up = false;
        while since_us(t0) < 4_000_000 {
            crate::platform::pet_watchdogs();
            if CP2AP_WAKEUP.get() {
                up = true;
                break;
            }
            ms(20);
        }
        line(rep, alloc::format!("cycle: CP2AP_WAKEUP {} after A#{} ms\n", if up { "high" } else { "still low" }, since_us(t0) / 1000));
        if !up {
            return false;
        }
        AP2CP_WAKEUP.out(true);
        ms(5);
        if !link_tries(rep, 6) || !endpoint(rep) {
            return false;
        }
    }
    wr(DOORBELL.load(core::sync::atomic::Ordering::Relaxed), DB_LINK_ACK);
    line(rep, alloc::format!("link ack rung (G#{DB_LINK_ACK:X})\n"));
    true
}

/// Put one boot-channel frame in the TX ring: the 12-byte SIT header, the command word, and `body` (the rest of the payload). Then the head, the message mask and the doorbell.
fn send_frame(seq: &mut u16, cmd: u16, body: &[u8], data: Option<&[u8]>) -> Result<(), alloc::string::String> {
    let payload = 2 + body.len() + data.map(|d| d.len()).unwrap_or(0);
    let total = 12 + payload;
    // Every frame takes a multiple of 8 bytes in the ring, zero filled (cpif `exynos_calc_padding_size`: the capture's G#A321 is G#14 long and moves the head G#18).
    let padded = (total + 7) & !7;
    let head = rd(TXQ_HEAD);
    let tail = rd(TXQ_TAIL);
    let free = (tail + TX_RING_LEN - head - 1) % TX_RING_LEN;
    if head >= TX_RING_LEN || tail >= TX_RING_LEN || (free as usize) < padded {
        return Err(alloc::format!("tx ring full or bad: head {head} tail {tail} need {padded}"));
    }
    // The frame counter is a full u16 in bytes 2–3 (the capture runs past G#FF); the channel's own sequence is its low byte.
    *seq = seq.wrapping_add(1);
    let hdr = [0xCD, 0xAB, *seq as u8, (*seq >> 8) as u8, 0x00, 0xC0, total as u8, (total >> 8) as u8, CH_BOOT, *seq as u8, 0, 0];
    let mut pos = head;
    let mut put = |b: &[u8]| {
        for &x in b {
            wr8(TX_RING + pos as usize, x);
            pos = (pos + 1) % TX_RING_LEN;
        }
    };
    put(&hdr);
    put(&cmd.to_le_bytes());
    put(body);
    if let Some(d) = data {
        put(d);
    }
    put(&[0u8; 8][..padded - total]);
    unsafe { core::arch::asm!("dsb sy") };
    wr(TXQ_HEAD, pos);
    unsafe { core::arch::asm!("dsb sy") };
    // No mask and no doorbell: on the boot channel the CP polls the ring. In the whole Android capture the AP writes `ap2cp_msg` exactly once (zero, at `init_legacy_link`) and never again, and rings no doorbell per frame. Writing G#81 and ringing per frame got the first frame answered and every later one consumed in silence (2026-10-20).
    Ok(())
}

/// Wait for a frame on the boot channel and return its command word. Frames are consumed from the RX ring whatever they are.
fn recv_frame(ms_wait: u64) -> Result<u16, alloc::string::String> {
    let t0 = now();
    loop {
        crate::platform::pet_watchdogs();
        for (name, base, len_max, headr, tailr) in [("raw", RX_RING, RX_RING_LEN, RXQ_HEAD, RXQ_TAIL), ("fmt", FMT_RX_RING, FMT_RX_RING_LEN, FMT_RXQ_HEAD, FMT_RXQ_TAIL)] {
            let head = rd(headr);
            let mut tail = rd(tailr);
            if head >= len_max || tail >= len_max {
                return Err(alloc::format!("{name} rx ring bad: head {head} tail {tail}"));
            }
            if head == tail {
                continue;
            }
            let at = |o: u32| rd8(base + ((tail + o) % len_max) as usize);
            if at(0) != 0xCD || at(1) != 0xAB {
                return Err(alloc::format!("no frame header at {name} rx tail {tail}: G#{:02X}{:02X}", at(1), at(0)));
            }
            let flen = at(6) as u32 | (at(7) as u32) << 8;
            let cmd = at(12) as u16 | (at(13) as u16) << 8;
            tail = (tail + ((flen.max(12) + 7) & !7)) % len_max;
            wr(tailr, tail);
            // The root complex's MSI status is write-1-to-clear (DBI G#830): left set, the CP's next interrupt is coalesced into the one already pending.
            let msi = rd(DBI + 0x830);
            if msi != 0 {
                wr(DBI + 0x830, msi);
            }
            crate::flashlog::line(&alloc::format!("modem rx {name}: G#{cmd:04X} len {flen}, msi G#{msi:X}"));
            return Ok(cmd);
        }
        if since_us(t0) > ms_wait * 1000 {
            return Err(alloc::format!("no reply in A#{ms_wait} ms (raw head {} tail {}, fmt head {} tail {}, cp2ap_msg G#{:X})", rd(RXQ_HEAD), rd(RXQ_TAIL), rd(FMT_RXQ_HEAD), rd(FMT_RXQ_TAIL), rd(IPC + 0x804)));
        }
        us(200);
    }
}

/// One command and its answer: the CP answers G#Axxx with the same number in G#Cxxx.
fn command(seq: &mut u16, cmd: u16, body: &[u8], ms_wait: u64) -> Result<(), alloc::string::String> {
    send_frame(seq, cmd, body, None)?;
    // Whether the CP reads what we put in the ring at all: it consumes at the tx tail.
    let t0 = now();
    while since_us(t0) < 500_000 && rd(TXQ_TAIL) != rd(TXQ_HEAD) {
        us(500);
    }
    crate::flashlog::line(&alloc::format!("modem tx G#{cmd:04X}: tx head {} tail {} after A#{} us, cp2ap_msg G#{:X}", rd(TXQ_HEAD), rd(TXQ_TAIL), since_us(t0), rd(IPC + 0x804)));
    let got = recv_frame(ms_wait)?;
    let want = (cmd & 0x0FFF) | 0xC000;
    if got != want {
        return Err(alloc::format!("sent G#{cmd:04X}, expected G#{want:04X}, got G#{got:04X}"));
    }
    Ok(())
}

/// Send one section over the boot channel: begin (G#A1n0), the data frames (G#A1nB, each acked), end (G#A1nD). `read` fills the chunk at an offset.
fn send_section<F: FnMut(u64, &mut [u8]) -> Result<(), alloc::string::String>>(rep: &mut Vec<u8>, seq: &mut u16, id: u32, size: u32, mut read: F) -> Result<(), alloc::string::String> {
    let n = ((id & 0xF) << 4) as u16;
    let t0 = now();
    command(seq, 0xA100 | n, &[0, 0], 2000)?;
    let frames = send_data(seq, id, size, &mut read)?;
    command(seq, 0xA10D | n, &[0, 0], 2000)?;
    line(rep, alloc::format!("section A#{id}: G#{size:X} bytes in A#{frames} frames, A#{} ms\n", since_us(t0) / 1000));
    Ok(())
}

/// A stage's data frames (G#A1nB {u16 chunk + 8, u32 total, u32 offset, data}), each acked by G#C1nB; how many.
fn send_data<F: FnMut(u64, &mut [u8]) -> Result<(), alloc::string::String>>(seq: &mut u16, id: u32, size: u32, read: &mut F) -> Result<u32, alloc::string::String> {
    let n = ((id & 0xF) << 4) as u16;
    let mut buf = alloc::vec![0u8; CHUNK];
    let mut off = 0u32;
    let mut frames = 0u32;
    while off < size {
        crate::platform::pet_watchdogs();
        let want = (size - off).min(CHUNK as u32) as usize;
        read(off as u64, &mut buf[..want])?;
        let mut body = alloc::vec::Vec::with_capacity(10);
        body.extend_from_slice(&((want + 8) as u16).to_le_bytes());
        body.extend_from_slice(&size.to_le_bytes());
        body.extend_from_slice(&off.to_le_bytes());
        send_frame(seq, 0xA10B | n, &body, Some(&buf[..want]))?;
        let got = recv_frame(4000)?;
        if got != (0xC10B | n) {
            return Err(alloc::format!("section A#{id} at G#{off:X}: expected G#{:04X}, got G#{got:04X}", 0xC10B | n));
        }
        off += want as u32;
        frames += 1;
    }
    Ok(frames)
}

/// Stage 7: the boot-channel download. MAIN, VSS and APM come out of `modem.bin`, the three NV sections out of the per-unit files on f2fs (efs, modem_userdata); START is not sent.
fn download(rep: &mut Vec<u8>, seq_out: &mut Option<u16>) {
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
    let toc = toc(&first);
    // The per-unit NV files, read before anything is sent so a missing file stops the download before it starts (cbd would stand in all-G#FF; ferros does not).
    let mut nv = Vec::new();
    for (part, path, id) in [("efs", "/nv_normal.bin", 5u32), ("efs", "/nv_protected.bin", 6), ("modem_userdata", "/replay_region.bin", 7)] {
        match crate::partfs::read_f2fs(part, path) {
            Ok(v) if v.len() == 0x8_0000 => nv.push((path, id, v)),
            Ok(v) => return line(rep, alloc::format!("{part}{path}: A#{} bytes, expected G#8_0000\n", v.len())),
            Err(e) => return line(rep, alloc::format!("{e}\n")),
        }
    }
    // The daemon's first boot-channel write comes well after the link acknowledgement; give the CP's bootloader the same breathing room.
    ms(500);
    let mut seq = 0u16;
    let mut step = |rep: &mut Vec<u8>, what: &str, r: Result<(), alloc::string::String>| -> bool {
        match r {
            Ok(()) => true,
            Err(e) => {
                line(rep, alloc::format!("{what}: {e}\n"));
                false
            }
        }
    };
    // Stage 1 is the TOC itself (cbd `build_std_dload_control`, decoded 2026-10-24): start G#A110 → G#C110, one data frame G#A11B carrying modem.bin[0..G#410] (the table of contents with its signature) → its ack G#C11B, then G#A11D → G#C11D. The C11B the capture showed "unprompted" is that ack: the capture had missed the one A11B frame, ferros never sent it, and the CP waited for it and dropped everything after.
    if !step(rep, "TOC start", command(&mut seq, 0xA110, &[0, 0], 2000)) {
        return;
    }
    let toc_len = 0x410usize.min(first.len());
    let toc_bytes = first[..toc_len].to_vec();
    match send_data(&mut seq, 1, toc_len as u32, &mut |off: u64, out: &mut [u8]| {
        out.copy_from_slice(&toc_bytes[off as usize..off as usize + out.len()]);
        Ok(())
    }) {
        Ok(f) => line(rep, alloc::format!("TOC sent in A#{f} frame, acked (G#C11B)\n")),
        Err(e) => return line(rep, alloc::format!("TOC data: {e}\n")),
    }
    if !step(rep, "TOC done", command(&mut seq, 0xA11D, &[0, 0], 2000)) {
        return;
    }
    line(rep, alloc::format!("stage 1 (TOC) complete (G#C11D)\n"));
    for name in ["MAIN", "VSS", "APM"] {
        let Some(s) = toc.iter().find(|s| s.name() == name) else {
            line(rep, alloc::format!("{name}: not in the TOC\n"));
            return;
        };
        let base = s.offset as u64;
        let r = send_section(rep, &mut seq, s.id, s.size, |off, out| {
            // The section starts mid-block: read the whole blocks around the wanted range and copy the middle out.
            let want = out.len();
            let start = base + off;
            let lo = (start & !4095) as u64;
            let span = ((start - lo) as usize + want + 4095) & !4095;
            let mut tmp = alloc::vec![0u8; span];
            m.read(&ino, lo, &mut tmp).map_err(|e| alloc::format!("{e}"))?;
            out.copy_from_slice(&tmp[(start - lo) as usize..(start - lo) as usize + want]);
            Ok(())
        });
        if !step(rep, name, r) {
            return;
        }
        if name == "MAIN" {
            // The TOC's CRC for MAIN, forwarded as the daemon does (G#A321).
            let mut b = alloc::vec::Vec::new();
            b.extend_from_slice(&0u16.to_le_bytes());
            b.extend_from_slice(&s.crc.to_le_bytes());
            // Its answer is G#C3n0, not the G#C3n1 the generic Axxx → Cxxx rule gives (cbd waits for C320 after A321; 2026-10-24 night it came).
            send_frame(&mut seq, 0xA321, &b, None).ok();
            match recv_frame(4000) {
                Ok(0xC320) => line(rep, alloc::format!("MAIN CRC accepted (G#C320)\n")),
                Ok(c) => return line(rep, alloc::format!("MAIN CRC: got G#{c:04X}, expected G#C320\n")),
                Err(e) => return line(rep, alloc::format!("MAIN CRC: {e}\n")),
            }
        }
    }
    // The three NV sections in the captured order (NV_NORM 5, NV_PROT 6, REPLAY sent as section 7), straight from the f2fs files; their contents are never logged.
    for (path, id, v) in &nv {
        let r = send_section(rep, &mut seq, *id, v.len() as u32, |off, out| {
            out.copy_from_slice(&v[off as usize..off as usize + out.len()]);
            Ok(())
        });
        if !step(rep, path, r) {
            return;
        }
    }
    // Stage 7 stops here; stage 8 sends START (G#A400, the owner's go 2026-10-26: data only) from the frame counter left here.
    *seq_out = Some(seq);
    line(rep, alloc::format!("MAIN, VSS, APM and the three NV sections loaded\n"));
    line(rep, alloc::format!("cp2ap_msg G#{:X}, cp2ap_status G#{:X}, boot_stage G#{:X}\n", rd(IPC + 0x804), rd(IPC + 0x80C), rd(BOOT_STAGE)));
}

/// Stage 8: START, then ten seconds of watching what the CP's main firmware does, answering nothing (MODEM-BOOT-RECON.md: after G#C400 the CP raises INIT_START, then PHONE_START, then opens channel G#29, the remote-file service). The owner cleared START and data bring-up on 2026-10-26; voice, SMS, MMS and emergency calls stay untouched. This is the first look, to measure before answering: every change of the CP's message and status words, its two GPIO lines and the boot stage, and every frame on the RAW and FMT rings (channel, length, first bytes; the paths the CP asks for are not per-unit data), each with its time since START.
fn start_and_watch(rep: &mut Vec<u8>, mut seq: u16) {
    match command(&mut seq, 0xA400, &[0, 0], 4000) {
        Ok(()) => line(rep, alloc::format!("START acked (G#C400): the main firmware runs\n")),
        Err(e) => return line(rep, alloc::format!("START: {e}\n")),
    }
    let t0 = now();
    let sample = || [rd(IPC + 0x804), rd(IPC + 0x80C), rd(BOOT_STAGE), PHONE_ACTIVE.get() as u32, CP2AP_WAKEUP.get() as u32];
    let names = ["cp2ap_msg", "cp2ap_status", "boot_stage", "PHONE_ACTIVE", "CP2AP_WAKEUP"];
    let mut last = sample();
    line(rep, alloc::format!("t=0: cp2ap_msg G#{:X} cp2ap_status G#{:X} boot_stage G#{:X} PHONE_ACTIVE {} CP2AP_WAKEUP {}\n", last[0], last[1], last[2], last[3], last[4]));
    let (mut changes, mut frames) = (0u32, 0u32);
    while since_us(t0) < 10_000_000 {
        crate::platform::pet_watchdogs();
        let now_s = sample();
        for k in 0..5 {
            if now_s[k] != last[k] && changes < 64 {
                line(rep, alloc::format!("t={} ms: {} G#{:X} -> G#{:X}\n", since_us(t0) / 1000, names[k], last[k], now_s[k]));
                changes += 1;
            }
        }
        last = now_s;
        for (name, base, len_max, headr, tailr) in [("raw", RX_RING, RX_RING_LEN, RXQ_HEAD, RXQ_TAIL), ("fmt", FMT_RX_RING, FMT_RX_RING_LEN, FMT_RXQ_HEAD, FMT_RXQ_TAIL)] {
            let head = rd(headr);
            let tail = rd(tailr);
            // The CP resets the rings at START; a cursor outside its ring is not ours to walk, only to report once.
            if head >= len_max || tail >= len_max || head == tail {
                continue;
            }
            let at = |o: u32| rd8(base + ((tail + o) % len_max) as usize);
            if at(0) != 0xCD || at(1) != 0xAB {
                if frames < 64 {
                    line(rep, alloc::format!("t={} ms: {name} head {head} tail {tail}, no frame header (G#{:02X}{:02X}); left as is\n", since_us(t0) / 1000, at(1), at(0)));
                    frames += 1;
                }
                continue;
            }
            let flen = (at(6) as u32 | (at(7) as u32) << 8).max(12);
            let ch = at(8);
            if frames < 64 {
                let n = (flen - 12).min(32);
                let mut hex = alloc::string::String::new();
                let mut txt = alloc::string::String::new();
                for i in 0..n {
                    let b = at(12 + i);
                    hex.push_str(&alloc::format!("{b:02X}"));
                    txt.push(if (0x20..0x7F).contains(&b) { b as char } else { '.' });
                }
                line(rep, alloc::format!("t={} ms: {name} frame ch G#{ch:02X} len A#{flen}: {hex} |{txt}|\n", since_us(t0) / 1000));
                frames += 1;
            }
            wr(tailr, (tail + ((flen + 7) & !7)) % len_max);
            let msi = rd(DBI + 0x830);
            if msi != 0 {
                wr(DBI + 0x830, msi);
            }
        }
        us(500);
    }
    line(rep, alloc::format!("watched 10 s: A#{changes} changes, A#{frames} frames logged; nothing answered (diag modem off powers the CP down)\n"));
}

// ---------------------------------------------------------------------------
// Stage 9: the post-START IPC handshake (MODEM-BOOT-RECON.md step 5-7; link_device.c cmd_init_start_handler 457-520, cmd_phone_start_handler 526-640).
//
// After START the CP's main firmware raises INIT_START on cp2ap_msg (observed G#83 -> G#C1 ~370 ms in). On husky the link has no LINK_ATTR_SBD_IPC, so the INIT_START handler skips the whole SBD link and is just: set the AP capabilities, answer PIF_INIT_DONE. The CP then raises PHONE_START (G#C8); the AP checks capabilities (AP caps that the CP lacks crash it — so claim nothing, 0, which passes), re-inits the legacy link and answers INIT_END (G#C2). rild_ready gates INIT_END on Linux (the RIL daemon must have the channels open); ferros is the RIL, so it answers unconditionally. This first cut does NOT set up pktproc (the DL direct ring in G#E800_0000) — it is a measure-first step to see whether the CP reaches ONLINE on the handshake alone or needs the ring before it will proceed; pktproc and the data path come next.

const CP2AP_MSG: usize = IPC + 0x804;
/// The capability words (link_device.c 3922-3928): per part, the AP's then the CP's, 4 bytes each — AP part 0 at G#A0, CP part 0 at G#A4, AP part 1 at G#A8, CP part 1 at G#AC.
const CAP_AP: usize = IPC + 0xA0;
const CAP_CP: usize = IPC + 0xA4;

/// cmd2int (link_device_memory.h): INT_VALID (G#80) | CMD_VALID (G#40) | cmd — matches the observed INIT_START G#C1.
const fn cmd2int(cmd: u16) -> u16 {
    0xC0 | (cmd & 0x3F)
}
const INIT_START: u16 = cmd2int(0x01);
const INIT_END: u16 = cmd2int(0x02);
const PHONE_START: u16 = cmd2int(0x08);
const CRASH_EXIT: u16 = cmd2int(0x09);
const PIF_INIT_DONE: u16 = cmd2int(0x0D);

/// Send an IPC command to the CP (pcie_send_ap2cp_irq, link_device.c:2686): the command word into ap2cp_msg, then the message doorbell. Unlike the boot channel, the IPC handshake is driven entirely by this msg + doorbell, not the RAW ring.
fn ap2cp_cmd(cmd: u16) {
    wr(AP2CP_MSG, cmd as u32);
    unsafe { core::arch::asm!("dsb sy") };
    wr(DOORBELL.load(core::sync::atomic::Ordering::Relaxed), DB_MSG);
    unsafe { core::arch::asm!("dsb sy") };
}

/// A CP->AP value on cp2ap_msg carries a command when CMD_VALID (G#40) is set (G#83 data-signals do not); the command byte is the low six bits.
fn is_cmd(v: u16) -> bool {
    v & 0x40 != 0
}

/// Stage 9 answers the handshake bare; stage 10 (`full`) does what Linux does around it — the pktproc DL rings at INIT_START, the legacy-link reset at PHONE_START — and then watches.
fn start_and_handshake(rep: &mut Vec<u8>, mut seq: u16, full: bool, probe: u8) {
    match command(&mut seq, 0xA400, &[0, 0], 4000) {
        Ok(()) => line(rep, alloc::format!("START acked (G#C400): the main firmware runs\n")),
        Err(e) => return line(rep, alloc::format!("START: {e}\n")),
    }
    let t0 = now();
    let mut answered_init_start = false;
    let mut phone_started = false;
    let mut last_cmd = 0u16;
    // Stage 10 stops answering at INIT_END and hands over to the watch; stage 9 keeps polling its 12 s.
    while since_us(t0) < 12_000_000 && !(full && phone_started) {
        crate::platform::pet_watchdogs();
        let v = rd(CP2AP_MSG) as u16;
        let msi = rd(DBI + 0x830);
        if msi != 0 {
            wr(DBI + 0x830, msi);
        }
        if is_cmd(v) && v != last_cmd {
            last_cmd = v;
            line(rep, alloc::format!("t={} ms: cp2ap_msg command G#{v:X}\n", since_us(t0) / 1000));
            match v {
                INIT_START if !answered_init_start => {
                    if full {
                        pktproc_init();
                        line(rep, alloc::format!("  pktproc: A#{PP_QUEUES} DL queues of A#{PP_NUM_DESC} descriptors, A#{} buffers posted each\n", PP_NUM_DESC - 1));
                    }
                    // Claim no capabilities, as husky's own DT does (ap_capability_0/1 = 0): (ap ^ cp) & ap == 0 passes the CP's check whatever it supports (link_device.c:585).
                    wr(CAP_AP, 0);
                    wr(CAP_AP + 8, 0);
                    unsafe { core::arch::asm!("dsb sy") };
                    ap2cp_cmd(PIF_INIT_DONE);
                    answered_init_start = true;
                    line(rep, alloc::format!("  -> PIF_INIT_DONE (G#{PIF_INIT_DONE:X}) sent, caps 0\n"));
                }
                PHONE_START if !phone_started => {
                    if full {
                        line(rep, alloc::format!("  CP capabilities: part 0 G#{:X}, part 1 G#{:X}\n", rd(CAP_CP), rd(CAP_CP + 8)));
                        legacy_init();
                    }
                    ap2cp_cmd(INIT_END);
                    phone_started = true;
                    line(rep, alloc::format!("  -> INIT_END (G#{INIT_END:X}) sent; PHONE_ACTIVE {}\n", PHONE_ACTIVE.get() as u32));
                }
                CRASH_EXIT => {
                    line(rep, alloc::format!("  CP raised CRASH_EXIT (G#{CRASH_EXIT:X}): the handshake was refused; see the FMT ring and cp2ap_status\n"));
                    break;
                }
                _ => {}
            }
        }
        us(500);
    }
    line(rep, alloc::format!("handshake over: INIT_START {}, PHONE_START {}, cp2ap_msg G#{:X} cp2ap_status G#{:X} PHONE_ACTIVE {} CP2AP_WAKEUP {}\n",
        if answered_init_start { "answered" } else { "never seen" },
        if phone_started { "answered with INIT_END" } else { "never seen" },
        rd(CP2AP_MSG), rd(IPC + 0x80C), PHONE_ACTIVE.get() as u32, CP2AP_WAKEUP.get() as u32));
    if full && phone_started {
        watch_online(rep, now(), probe);
    }
}

// ---------------------------------------------------------------------------
// Stage 10: the DL packet rings (pktproc), the legacy link, and the first look at what an online CP says.
//
// pktproc (link_rx_pktproc.c pktproc_create / pktproc_init; DT zuma-cp-s5300-sit.dtsi 139-183): G#E800_0000 holds a G#1000 info region, then the sktbuf descriptors, then from +G#10_0000 the G#1B0_0000 of packet buffers, four queues a quarter each. The CP sees the region at G#2000_0000. Linux maps each queue's buffers through the HSI1 SysMMU at IOVA = the buffer's own place in this region (cpif_pcie_iommu_map_va: src_pa = q_buff_pbase + stride × index); with the SysMMU in bypass that IOVA is the physical address, so ferros posts the region's own buffers in place. The region is Device memory in the cache map (cachemap.rs: every block of that GiB outside the surfaces), so the CP's writes need no maintenance.

const PKTPROC: usize = 0xE800_0000;
const PP_CP_BASE: u32 = 0x2000_0000;
const PP_DESC: u32 = 0x1000;
const PP_BUFF: u32 = 0x10_0000;
const PP_BUFF_LEN: u32 = 0x1B0_0000;
const PP_QUEUES: u32 = 4;
const PP_MAX_PACKET: u32 = 1584;
/// One buffer per descriptor, 2 KiB apart: Linux's stride is 1584 plus the skb front padding and shared info, rounded up to a power of two (pktproc_adjust_size), which is this; on bare metal it is ours to pick, and a power of two keeps the index a shift.
const PP_STRIDE: u32 = 1 << 11;
const PP_Q_BUFF: u32 = PP_BUFF_LEN / PP_QUEUES;
const PP_NUM_DESC: u32 = PP_Q_BUFF / PP_STRIDE;
const PP_DESC_LEN: u32 = 16;
/// Sktbuf descriptor control bits (byte 5 of the first u64) and status (byte 6).
const PP_HEAD: u64 = 0x80 << 40;
const PP_RINGEND: u64 = 0x08 << 40;
const PP_DONE: u64 = 0x01 << 48;

fn wr64(a: usize, v: u64) {
    unsafe { core::ptr::write_volatile(a as *mut u64, v) }
}
fn rd64(a: usize) -> u64 {
    unsafe { core::ptr::read_volatile(a as *const u64) }
}
/// `pktproc_q_info` i: 20 bytes from +4 — cp_desc_pbase, num_desc, cp_buff_pbase, fore_ptr, rear_ptr.
const fn pp_qinfo(q: u32) -> usize {
    PKTPROC + 4 + 20 * q as usize
}
const fn pp_desc(q: u32, i: u32) -> usize {
    PKTPROC + (PP_DESC + (q * PP_NUM_DESC + i) * PP_DESC_LEN) as usize
}
/// The CP's address of queue q's buffer i (the descriptor's cp_data_paddr) and where the AP reads it.
const fn pp_buf_cp(q: u32, i: u32) -> u32 {
    PP_CP_BASE + PP_BUFF + q * PP_Q_BUFF + i * PP_STRIDE
}
const fn pp_buf_ap(q: u32, i: u32) -> usize {
    PKTPROC + (PP_BUFF + q * PP_Q_BUFF + i * PP_STRIDE) as usize
}

/// Post queue q's buffer i: its address, HEAD on the first, RINGEND on the last; status and length cleared.
fn pp_post(q: u32, i: u32) {
    let mut d = pp_buf_cp(q, i) as u64;
    if i == 0 {
        d |= PP_HEAD;
    }
    if i == PP_NUM_DESC - 1 {
        d |= PP_RINGEND;
    }
    wr64(pp_desc(q, i) + 8, 0);
    wr64(pp_desc(q, i), d);
}

/// pktproc_create's info header plus pktproc_init: every descriptor cleared, each queue's entry written, every buffer but one posted (a ring of n holds n - 1, `circ_get_space`), fore_ptr after them. No doorbell — the CP reads the header when it starts the rings.
fn pktproc_init() {
    for a in (PKTPROC..PKTPROC + (PP_DESC + PP_QUEUES * PP_NUM_DESC * PP_DESC_LEN) as usize).step_by(8) {
        wr64(a, 0);
    }
    // num_queues[3:0], desc_mode[5:4] 1 = sktbuf, irq_mode[7:6] 1 = a vector per queue (pktproc_dl_use_exclusive_irq), max_packet_size[23:8].
    wr(PKTPROC, PP_QUEUES | 1 << 4 | 1 << 6 | PP_MAX_PACKET << 8);
    for q in 0..PP_QUEUES {
        wr(pp_qinfo(q), PP_CP_BASE + PP_DESC + q * PP_NUM_DESC * PP_DESC_LEN);
        wr(pp_qinfo(q) + 4, PP_NUM_DESC);
        wr(pp_qinfo(q) + 8, pp_buf_cp(q, 0));
        wr(pp_qinfo(q) + 16, 0);
        for i in 0..PP_NUM_DESC - 1 {
            pp_post(q, i);
        }
        unsafe { core::arch::asm!("dsb sy") };
        wr(pp_qinfo(q) + 12, PP_NUM_DESC - 1);
    }
    unsafe { core::arch::asm!("dsb sy") };
}

/// init_legacy_link (link_device_memory_legacy.c 228-268): magic and mem_access dropped, the FMT and RAW cursors (IPC+G#8..G#24) zeroed, then magic G#AA (MEM_IPC_MAGIC) and mem_access 1.
fn legacy_init() {
    wr(IPC, 0);
    wr(IPC + 4, 0);
    for a in (IPC + 0x8..IPC + 0x28).step_by(4) {
        wr(a, 0);
    }
    unsafe { core::arch::asm!("dsb sy") };
    wr(IPC, 0xAA);
    wr(IPC + 4, 1);
    unsafe { core::arch::asm!("dsb sy") };
}

// ---------------------------------------------------------------------------
// The RIL's wire (decoded 2026-10-29 from the vendor image's vendor.radio.protocol.sit.base.so and .stream.so, whose builders are exported): each RIL message is one SIT link frame on FMT channel 245 (`umts_ipc0`; 246 is the second), its payload an RCM message — u8 type (0 request, 1 response, 2 indication), u8 0, u16 id, u16 total length, then for a request a u32 token and two zero bytes (a response puts its error byte after the token), an indication only two zero bytes: a 12-byte header, 8 for an indication. Then the parameters. Tokens count up from 0 (TokenGen). Ids from `rcmMsgToString`: SIT_GET_RADIO_POWER G#801, SIT_SET_RADIO_POWER G#800 (u32 state 2 on, 1 off, then two option bytes), SIT_IND_RADIO_STATE_CHANGED G#802, SIT_IND_RADIO_READY G#803, SIT_SET_INITIAL_ATTACH_APN G#603, SIT_SETUP_DATA_CALL G#600, SIT_IND_DATA_CALL_LIST_CHANGED G#604.

const FMT_TXQ_HEAD: usize = IPC + 0x8;
const FMT_TXQ_TAIL: usize = IPC + 0xC;
const FMT_TX_RING: usize = IPC + 0x1000;
const FMT_TX_RING_LEN: u32 = 0x1000;
const CH_FMT0: u8 = 245;
/// `mask2int(MASK_SEND_FMT)` (link_device_memory.h: SEND_FMT G#2, SEND_RAW G#1, INT_VALID G#80): what the AP writes to ap2cp_msg after putting frames in the FMT ring.
const INT_SEND_FMT: u32 = 0x82;
const SIT_GET_RADIO_POWER: u16 = 0x0801;
const SIT_SET_RADIO_POWER: u16 = 0x0800;
const SIT_GET_PS_REG_STATE: u16 = 0x0701;
const SIT_GET_SIM_STATUS: u16 = 0x0200;
const SIT_SET_PS_SERVICE: u16 = 0x0710;
const SIT_SET_INITIAL_ATTACH_APN: u16 = 0x0603;
/// The APN the data SIM attaches with. Verizon's public internet APN, the one its data-only devices attach on (stack 1's confpack is us_vzw); not IMS, so nothing voice comes up. A per-carrier choice ferros will take from settings later.
const ATTACH_APN: &[u8] = b"VZWINTERNET";

/// SIT_SET_INITIAL_ATTACH_APN's 239 parameter bytes (ProtocolPsBuilder::BuildSetInitialAttachApn, 251 bytes with the header), as offsets into the whole message: G#C profile id 0, G#D 14 (22 when a PDP type is 5), G#E 0, G#F APN type 0 (default; 1 IMS, 2 emergency), G#10 the APN [101], G#75 user [50], G#A7 password [50], G#D9 auth 0 (none), G#DA PDP type 3 (IPV4V6; 1 IP, 2 IPV6), G#DB P-CSCF request 0, G#F8 the roaming PDP type 3, G#F9 the roaming P-CSCF request 0, the rest 0.
fn initial_attach_params(apn: &[u8]) -> Vec<u8> {
    let mut m = alloc::vec![0u8; 0xFB];
    m[0xD] = 14;
    // The APN field is 101 bytes with its NUL; the name is our own constant, so one too long is a bug: fail loud.
    assert!(apn.len() <= 100);
    m[0x10..0x10 + apn.len()].copy_from_slice(apn);
    m[0xDA] = 3;
    m[0xF8] = 3;
    m[12..].to_vec()
}
const SIT_SET_PREFERRED_DATA_MODEM: u16 = 0x0740;

/// Names for the ids the data path meets (rcmMsgToString); others print as their number.
fn sit_name(id: u16) -> &'static str {
    match id {
        0x0200 => "GET_SIM_STATUS",
        0x0210 => "IND_SIM_STATUS_CHANGED",
        0x024E => "IND_SIM_SLOT_STATUS_CHANGED",
        0x0600 => "SETUP_DATA_CALL",
        0x0603 => "SET_INITIAL_ATTACH_APN",
        0x0604 => "IND_DATA_CALL_LIST_CHANGED",
        0x0609 => "IND_DATA_STATE_CHANGE",
        0x070E => "IND_NTW_STATE_CHANGED",
        0x070F => "IND_CELL_INFO_LIST",
        0x0717 => "IND_NET_CURRENT_LTE_MODE",
        0x072C => "IND_TOTAL_OOS",
        0x072D => "IND_MCC",
        0x0737 => "IND_RADIO_CAPABILITY",
        0x0742 => "IND_PHYSICAL_CHANNEL_CONFIG",
        0x074A => "IND_REG_FAILED",
        0x0700 => "GET_CS_REG_STATE",
        0x0701 => "GET_PS_REG_STATE",
        0x0710 => "SET_PS_SERVICE",
        0x0740 => "SET_PREFERRED_DATA_MODEM",
        0x0800 => "SET_RADIO_POWER",
        0x0801 => "GET_RADIO_POWER",
        0x0802 => "IND_RADIO_STATE_CHANGED",
        0x0803 => "IND_RADIO_READY",
        0x0804 => "IND_PHONE_RESET",
        0x0805 => "IND_MODEM_RESTART",
        0x0905 => "IND_NITZ_TIME_RECEIVED",
        0x0906 => "IND_SIGNAL_STRENGTH",
        0x0D05 => "IND_AIMS_REGISTRATION",
        _ => "",
    }
}

/// Whether a message's parameters may be shown: radio power and state, registration and coverage, signal, SIM card state and data-call state carry no identity. Anything else — SIM files, IMSI, ICCID, IMEI, numbers, SMS, calls, cell ids, location, time — is shown by its header alone.
fn sit_shown(id: u16) -> bool {
    // Network state (G#70E) is left out: after the PLMN it carries the tracking-area code.
    matches!(id, 0x0210 | 0x0603 | 0x0604 | 0x0609 | 0x0710 | 0x0717 | 0x072C | 0x0740 | 0x074A | 0x0800..=0x0805 | 0x0906)
}

/// The AP's side of the SIT link: each io-device (one per channel here) keeps its own frame counter and its own channel counter, both starting at 1 (`exynos_build_header`); the RCM token counts RIL requests.
struct SitLink {
    frame: [u16; 256],
    ch: [u8; 256],
    token: u32,
}

/// One single-frame message into the FMT TX ring (cfg G#C000, 8-byte padding as on RAW), then the SEND_FMT message and the doorbell (link_device.c 860-880).
fn fmt_send(link: &mut SitLink, ch: u8, payload: &[u8]) -> Result<(), alloc::string::String> {
    ring_send(link, false, ch, payload)
}

/// One single-frame message into the FMT or the RAW TX ring, then that ring's SEND message and the doorbell. The RAW ring is the boot channel's and, after START, the remote-file service's.
fn ring_send(link: &mut SitLink, raw: bool, ch: u8, payload: &[u8]) -> Result<(), alloc::string::String> {
    let (ring, len, headr, tailr, int) = if raw { (TX_RING, TX_RING_LEN, TXQ_HEAD, TXQ_TAIL, INT_SEND_DATA) } else { (FMT_TX_RING, FMT_TX_RING_LEN, FMT_TXQ_HEAD, FMT_TXQ_TAIL, INT_SEND_FMT) };
    let total = 12 + payload.len();
    let padded = (total + 7) & !7;
    let head = rd(headr);
    let tail = rd(tailr);
    // The cursors live in shared memory the CP also writes: out of range, or too little room, and nothing is written.
    let free = (tail + len - head - 1) % len;
    if head >= len || tail >= len || (free as usize) < padded {
        return Err(alloc::format!("tx ring full or bad: head {head} tail {tail} need {padded}"));
    }
    link.frame[ch as usize] = link.frame[ch as usize].wrapping_add(1);
    link.ch[ch as usize] = link.ch[ch as usize].wrapping_add(1);
    let (frame, chseq) = (link.frame[ch as usize], link.ch[ch as usize]);
    let hdr = [0xCD, 0xAB, frame as u8, (frame >> 8) as u8, 0x00, 0xC0, total as u8, (total >> 8) as u8, ch, chseq, 0, 0];
    let mut pos = head;
    let mut put = |b: &[u8]| {
        for &x in b {
            wr8(ring + pos as usize, x);
            pos = (pos + 1) % len;
        }
    };
    put(&hdr);
    put(payload);
    put(&[0u8; 8][..padded - total]);
    unsafe { core::arch::asm!("dsb sy") };
    wr(headr, pos);
    unsafe { core::arch::asm!("dsb sy") };
    wr(AP2CP_MSG, int);
    unsafe { core::arch::asm!("dsb sy") };
    wr(DOORBELL.load(core::sync::atomic::Ordering::Relaxed), DB_MSG);
    Ok(())
}

/// A RIL request with the next token on RIL channel `ch` (245 or 246, one per stack); returns the token.
fn sit_request(link: &mut SitLink, ch: u8, id: u16, params: &[u8]) -> Result<u32, alloc::string::String> {
    let tok = link.token;
    link.token = link.token.wrapping_add(1);
    let len = 12 + params.len();
    let mut m = alloc::vec![0u8, 0, id as u8, (id >> 8) as u8, len as u8, (len >> 8) as u8];
    m.extend_from_slice(&tok.to_le_bytes());
    m.extend_from_slice(&[0, 0]);
    m.extend_from_slice(params);
    fmt_send(link, ch, &m)?;
    Ok(tok)
}

// ---------------------------------------------------------------------------
// The modem service on oem_ipc1 (channel 130), Android's /vendor/bin/shared_modem_platform. Its messages are protobuf `modem_svc_message.ModemSvcMessage` (decoded 2026-10-29 from the descriptors embedded in the vendor image's libmodem_svc_proto_legacy_soong.so): {1 type (1 request, 2 response, 3 indication), 2 message_id fixed32, 3 token fixed32, 4 result (0 success), then one payload: 5 modemstat, 6 extendedlog, 7 property, 8 userspacelog, 9 file, 10 temperature, 11 test, 12 uecap_file, 13 push_events}. property_message = {1 read_request {1 property}, 2 read_response {1 value}, 3 write_request {1 property, 2 value}, 4 write_response {}}. A response repeats the request's message_id and token.
// Protobuf is the modem's wire, not a ferros format: read strictly (a malformed message is dropped and logged), written by hand.

const CH_MODEM_SVC: u8 = 130;
/// UE-capability file blocks: 1 KiB of data a message keeps each one a single SIT frame (under 2 KiB with its headers) and leaves room in the 4 KiB FMT ring.
const UECAP_BLOCK: usize = 1 << 10;

/// One protobuf field: number, wire type, the varint/fixed value, and for a length-delimited field its bytes.
struct PbField<'a> {
    num: u32,
    wire: u8,
    val: u64,
    bytes: &'a [u8],
}

fn pb_varint(b: &[u8], p: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for s in 0..10 {
        // The CP's bytes: a varint running off the end of the message is malformed.
        let x = *b.get(*p)?;
        *p += 1;
        v |= ((x & 0x7F) as u64) << (7 * s);
        if x & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// Every top-level field of a message, or None when it is malformed. Lengths are the CP's, so each is checked against what is left before it is sliced.
fn pb_fields(b: &[u8]) -> Option<Vec<PbField<'_>>> {
    let mut p = 0;
    let mut out = Vec::new();
    while p < b.len() {
        let k = pb_varint(b, &mut p)?;
        let (num, wire) = ((k >> 3) as u32, (k & 7) as u8);
        let f = match wire {
            0 => PbField { num, wire, val: pb_varint(b, &mut p)?, bytes: &[] },
            2 => {
                let n = pb_varint(b, &mut p)? as usize;
                if n > b.len() - p {
                    return None;
                }
                p += n;
                PbField { num, wire, val: 0, bytes: &b[p - n..p] }
            }
            1 | 5 => {
                let n = if wire == 1 { 8 } else { 4 };
                if n > b.len() - p {
                    return None;
                }
                let mut v = [0u8; 8];
                v[..n].copy_from_slice(&b[p..p + n]);
                p += n;
                PbField { num, wire, val: u64::from_le_bytes(v), bytes: &[] }
            }
            _ => return None,
        };
        out.push(f);
    }
    Some(out)
}

fn pb_put_varint(v: &mut Vec<u8>, mut x: u64) {
    while x >= 0x80 {
        v.push(x as u8 | 0x80);
        x >>= 7;
    }
    v.push(x as u8);
}

fn pb_put_bytes(v: &mut Vec<u8>, num: u32, b: &[u8]) {
    pb_put_varint(v, (num as u64) << 3 | 2);
    pb_put_varint(v, b.len() as u64);
    v.extend_from_slice(b);
}

/// Answer one ModemSvcMessage: property writes are acknowledged and kept, property reads answered from what was written (empty otherwise, as an unset Android property reads), anything else logged and left unanswered. Returns the reply's bytes.
fn modem_svc(rep: &mut Vec<u8>, t_ms: u64, body: &[u8], props: &mut Vec<(Vec<u8>, Vec<u8>)>) -> Option<Vec<Vec<u8>>> {
    let Some(top) = pb_fields(body) else {
        line(rep, alloc::format!("t={t_ms} ms: modem_svc: malformed message, dropped\n"));
        return None;
    };
    let get = |n: u32| top.iter().find(|f| f.num == n);
    let (ty, id, token) = (get(1).map(|f| f.val), get(2).map(|f| f.val as u32), get(3).map(|f| f.val as u32).unwrap_or(0));
    let txt = |b: &[u8]| alloc::string::String::from_utf8_lossy(b).into_owned();
    // A userspace log line (payload 8, `UserspaceLogMessage`): shown, not answered.
    if let Some(log) = get(8) {
        line(rep, alloc::format!("t={t_ms} ms: modem_svc log (id A#{}): {}\n", id.unwrap_or(0), txt(log.bytes).trim_matches(|c: char| !c.is_ascii_graphic())));
        return None;
    }
    // A UE-capability file read (payload 12, `UecapFileMessage` {1 read_request {1 size, 2 block_size, 3 file_dir, 4 file_name}, 2 read_response {1 offset, 2 data, 3 result, 4 last_block}}). The directory is UECAP_PATH (3), the modem image's /images/default/uecap; the file is served in blocks of UECAP_BLOCK bytes, each a RESPONSE with the request's id and token and its own offset, the last one flagged. A file that is not there gets one final block with result 1 (UFM_INTERNAL_ERROR).
    if let (Some(1), Some(id), Some(uc)) = (ty, id, get(12)) {
        let req = pb_fields(uc.bytes)?;
        let rr = req.iter().find(|f| f.num == 1)?;
        let rf = pb_fields(rr.bytes)?;
        let g = |n: u32| rf.iter().find(|f| f.num == n);
        let name = txt(g(4).map(|f| f.bytes).unwrap_or(&[]));
        let file = if name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.') && !name.is_empty() {
            crate::partfs::mount(MODEM_PART).and_then(|mut m| {
                let ino = m.open(&alloc::format!("/images/default/uecap/{name}"))?;
                let mut v = alloc::vec![0u8; (ino.size as usize + 4095) & !4095];
                m.read(&ino, 0, &mut v)?;
                v.truncate(ino.size as usize);
                Ok(v)
            })
        } else {
            Err(alloc::format!("not a plain file name"))
        };
        let block = |off: u32, data: &[u8], result: u64, last: bool| {
            let mut resp = Vec::new();
            pb_put_varint(&mut resp, 1 << 3);
            pb_put_varint(&mut resp, off as u64);
            pb_put_bytes(&mut resp, 2, data);
            pb_put_varint(&mut resp, 3 << 3);
            pb_put_varint(&mut resp, result);
            pb_put_varint(&mut resp, 4 << 3);
            pb_put_varint(&mut resp, last as u64);
            let mut inner = Vec::new();
            pb_put_bytes(&mut inner, 2, &resp);
            let mut out = alloc::vec![0x08, 0x02, 0x15];
            out.extend_from_slice(&id.to_le_bytes());
            out.push(0x1D);
            out.extend_from_slice(&token.to_le_bytes());
            pb_put_bytes(&mut out, 12, &inner);
            out
        };
        return Some(match file {
            Ok(v) => {
                line(rep, alloc::format!("t={t_ms} ms: modem_svc uecap read {name} (token A#{token}) -> A#{} bytes from {MODEM_PART}\n", v.len()));
                let n = v.len().div_ceil(UECAP_BLOCK).max(1);
                (0..n).map(|k| block((k * UECAP_BLOCK) as u32, &v[k * UECAP_BLOCK..((k + 1) * UECAP_BLOCK).min(v.len())], 0, k + 1 == n)).collect()
            }
            Err(e) => {
                line(rep, alloc::format!("t={t_ms} ms: modem_svc uecap read {name} (token A#{token}) -> not available ({e})\n"));
                alloc::vec![block(0, &[], 1, true)]
            }
        });
    }
    let (Some(1), Some(id), Some(prop)) = (ty, id, get(7)) else {
        line(rep, alloc::format!("t={t_ms} ms: modem_svc: type {:?} id {:?}, payload {:?}, not answered\n", ty, id, top.iter().filter(|f| f.num >= 5).map(|f| f.num).collect::<Vec<_>>()));
        return None;
    };
    let op = pb_fields(prop.bytes)?;
    let mut inner = Vec::new();
    for f in op.iter() {
        let kv = pb_fields(f.bytes)?;
        let key = kv.iter().find(|x| x.num == 1).map(|x| x.bytes).unwrap_or(&[]);
        match f.num {
            3 => {
                let val = kv.iter().find(|x| x.num == 2).map(|x| x.bytes).unwrap_or(&[]);
                line(rep, alloc::format!("t={t_ms} ms: modem_svc property write {} = {} (token A#{token})\n", txt(key), txt(val)));
                match props.iter_mut().find(|(k, _)| k.as_slice() == key) {
                    Some(e) => e.1 = val.to_vec(),
                    None => props.push((key.to_vec(), val.to_vec())),
                }
                pb_put_bytes(&mut inner, 4, &[]);
            }
            1 => {
                let val = props.iter().find(|(k, _)| k.as_slice() == key).map(|(_, v)| v.clone()).unwrap_or_default();
                line(rep, alloc::format!("t={t_ms} ms: modem_svc property read {} -> \"{}\" (token A#{token})\n", txt(key), txt(&val)));
                let mut r = Vec::new();
                pb_put_bytes(&mut r, 1, &val);
                pb_put_bytes(&mut inner, 2, &r);
            }
            _ => {}
        }
    }
    if inner.is_empty() {
        return None;
    }
    let mut out = alloc::vec![0x08, 0x02, 0x15];
    out.extend_from_slice(&id.to_le_bytes());
    out.push(0x1D);
    out.extend_from_slice(&token.to_le_bytes());
    pb_put_bytes(&mut out, 7, &inner);
    Some(alloc::vec![out])
}

// ---------------------------------------------------------------------------
// The remote-file service on RAW channel 41 (`umts_rfs0`, Android's /vendor/bin/rfsd; decoded 2026-10-29 from rfsd's handlers and the Android capture's own channel-41 conversation). Each message: u16 cmd, u16 transaction, u32 body length, then the body as u32 words. The CP opens a file into a slot (OPEN 4: slot, path length, path; slots 1-2 are the NV files, 3 the protected NV, 4 up files by path), asks for I/O (IO_REQUEST 6: slot, offset, size, direction 1 read / 2 write), queries (OP_STATUS 7: slot) and closes (CLOSE 5: slot). The AP answers each with STATUS (3: status, slot) and the CP acks every STATUS with its own. A read streams DATA (1: slot, offset, size, bytes) chunks the CP acks; a write is pulled by the AP, a cmd 2 {slot, offset, size} per chunk answered by the CP's cmd 2 {0, slot, size, bytes}, then STATUS. Chunks are G#7DC bytes, as rfsd's.
// ferros never writes the phone's NV or files: a write is pulled to the end and acknowledged, and its bytes are dropped (the CP keeps its NV in RAM; the next boot loads the NV on flash, as before). Opens of files ferros cannot serve yet are logged and left unanswered.

const CH_RFS: u8 = 41;
const RFS_CHUNK: u32 = 0x7DC;

/// A write the CP asked for, being pulled chunk by chunk.
struct RfsPull {
    tid: u16,
    slot: u32,
    off: u32,
    end: u32,
}

fn rfs_status(link: &mut SitLink, tid: u16, slot: u32) -> Result<(), alloc::string::String> {
    let mut m = alloc::vec![3u8, 0, tid as u8, (tid >> 8) as u8, 8, 0, 0, 0];
    for w in [0u32, slot, 0] {
        m.extend_from_slice(&w.to_le_bytes());
    }
    ring_send(link, true, CH_RFS, &m)
}

fn rfs_pull(link: &mut SitLink, p: &RfsPull) -> Result<(), alloc::string::String> {
    let mut m = alloc::vec![2u8, 0, p.tid as u8, (p.tid >> 8) as u8, 0xC, 0, 0, 0];
    for w in [p.slot, p.off, (p.end - p.off).min(RFS_CHUNK)] {
        m.extend_from_slice(&w.to_le_bytes());
    }
    ring_send(link, true, CH_RFS, &m)
}

/// One remote-file message from the CP: answered as rfsd answers it, writes pulled and dropped.
fn rfs(rep: &mut Vec<u8>, t_ms: u64, b: &[u8], link: &mut SitLink, pull: &mut Option<RfsPull>) {
    // The CP's bytes: every word read is checked to lie inside the message.
    let w = |i: usize| if b.len() >= i + 4 { Some(u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])) } else { None };
    let (Some(head), Some(_len)) = (w(0), w(4)) else {
        return line(rep, alloc::format!("t={t_ms} ms: rfs: short message (A#{} bytes)\n", b.len()));
    };
    let (cmd, tid) = (head as u16, (head >> 16) as u16);
    let r = match (cmd, w(8)) {
        (3, _) => Ok(()),
        (7, Some(slot)) => {
            line(rep, alloc::format!("t={t_ms} ms: rfs OP_STATUS slot A#{slot} (tid A#{tid}) -> status 0\n"));
            rfs_status(link, tid, slot)
        }
        (5, Some(slot)) => {
            line(rep, alloc::format!("t={t_ms} ms: rfs CLOSE slot A#{slot} (tid A#{tid}) -> status 0\n"));
            rfs_status(link, tid, slot)
        }
        (4, Some(slot)) => {
            let n = w(12).unwrap_or(0) as usize;
            let path = if b.len() >= 16 + n { alloc::string::String::from_utf8_lossy(&b[16..16 + n]).into_owned() } else { alloc::string::String::from("?") };
            line(rep, alloc::format!("t={t_ms} ms: rfs OPEN slot A#{slot} {} (tid A#{tid}): not served yet\n", path.trim_end_matches('\0')));
            Ok(())
        }
        (6, Some(slot)) => match (w(12), w(16), w(20)) {
            (Some(off), Some(size), Some(2)) => {
                line(rep, alloc::format!("t={t_ms} ms: rfs write slot A#{slot} G#{size:X} bytes at G#{off:X} (tid A#{tid}): pulled, not stored\n"));
                // Offset and size are the CP's: wrapping keeps a hostile pair from panicking the kernel; the pull then simply ends early.
                let p = RfsPull { tid, slot, off, end: off.wrapping_add(size) };
                let r = if size == 0 { rfs_status(link, tid, slot) } else { rfs_pull(link, &p) };
                if size != 0 {
                    *pull = Some(p);
                }
                r
            }
            (off, size, dir) => {
                line(rep, alloc::format!("t={t_ms} ms: rfs IO_REQUEST slot A#{slot} offset {off:?} size {size:?} direction {dir:?} (tid A#{tid}): not served yet\n"));
                Ok(())
            }
        },
        (2, _) => match (pull.as_mut(), w(16)) {
            (Some(p), Some(n)) if p.tid == tid => {
                p.off = p.off.wrapping_add(n);
                if n == 0 || p.off >= p.end {
                    let (tid, slot) = (p.tid, p.slot);
                    *pull = None;
                    line(rep, alloc::format!("t={t_ms} ms: rfs write slot A#{slot} pulled to the end -> status 0\n"));
                    rfs_status(link, tid, slot)
                } else {
                    rfs_pull(link, p)
                }
            }
            _ => {
                line(rep, alloc::format!("t={t_ms} ms: rfs data (tid A#{tid}) with no write pending, dropped\n"));
                Ok(())
            }
        },
        _ => {
            line(rep, alloc::format!("t={t_ms} ms: rfs cmd A#{cmd} (tid A#{tid}): not handled\n"));
            Ok(())
        }
    };
    if let Err(e) = r {
        line(rep, alloc::format!("  rfs reply not sent: {e}\n"));
    }
}

/// After INIT_END: 90 s of everything the CP does, answering nothing — the message and status words, the RAW and FMT frames (channel, length, and bytes: up to 160 with text for the remote-file channel 41 and the oem_ipc channels 129-136, whose requests name files and properties; only the first 32 as hex for the RIL channels, whose payloads may carry SIM identity), and every DL packet (queue, channel, length, first bytes), each buffer reposted as it is read.
fn watch_online(rep: &mut Vec<u8>, t0: u64, probe: u8) {
    let mut done = [0u32; PP_QUEUES as usize];
    let mut link = SitLink { frame: [0; 256], ch: [0; 256], token: 0 };
    let (mut probed, mut powered) = (false, false);
    let mut next_reg = 5_000_000u64;
    let (mut sim_asked, mut data_on, mut ia_set) = (false, false, false);
    let mut props: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut pull: Option<RfsPull> = None;
    let (mut frames, mut packets, mut changes) = (0u32, 0u32, 0u32);
    let mut per_ch = [0u32; 256];
    let mut last = (rd(CP2AP_MSG), rd(IPC + 0x80C), PHONE_ACTIVE.get());
    line(rep, alloc::format!("online: magic G#{:X} mem_access {}, watching 90 s\n", rd(IPC), rd(IPC + 4)));
    let hex = |at: &dyn Fn(u32) -> u8, n: u32| {
        let mut h = alloc::string::String::new();
        for i in 0..n {
            h.push_str(&alloc::format!("{:02X}", at(i)));
        }
        h
    };
    while since_us(t0) < 90_000_000 {
        crate::platform::pet_watchdogs();
        let now_s = (rd(CP2AP_MSG), rd(IPC + 0x80C), PHONE_ACTIVE.get());
        if now_s != last && changes < 32 {
            line(rep, alloc::format!("t={} ms: cp2ap_msg G#{:X} cp2ap_status G#{:X} PHONE_ACTIVE {}\n", since_us(t0) / 1000, now_s.0, now_s.1, now_s.2 as u32));
            changes += 1;
            if now_s.0 as u16 == CRASH_EXIT {
                line(rep, alloc::format!("  CRASH_EXIT: the CP gave up\n"));
                break;
            }
        }
        last = now_s;
        if probe >= 1 && !probed && since_us(t0) > 2_000_000 {
            probed = true;
            match sit_request(&mut link, CH_FMT0, SIT_GET_RADIO_POWER, &[]) {
                Ok(tok) => line(rep, alloc::format!("t={} ms: -> SIT_GET_RADIO_POWER (G#{SIT_GET_RADIO_POWER:X}) token A#{tok} on ch A#{CH_FMT0}\n", since_us(t0) / 1000)),
                Err(e) => line(rep, alloc::format!("t={} ms: SIT_GET_RADIO_POWER not sent: {e}\n", since_us(t0) / 1000)),
            }
        }
        // Stage e: what Android's telephony does for its data SIM once the SIM is ready — the preferred data modem (one byte, the stack's index: ProtocolPsBuilder::BuildSetPreferredDataModem), then allow data (SIT_SET_PS_SERVICE, one byte 1: ProtocolNetworkBuilder::BuildAllowData), both on that stack's channel. The SIM sits on stack 1 here (its confpack is us_vzw).
        // Stage f: the initial-attach APN on the data SIM's stack before data is allowed, as Android's telephony sets it once the SIM is ready.
        if probe >= 5 && !ia_set && since_us(t0) > 6_500_000 {
            ia_set = true;
            let ch = CH_FMT0 + 1;
            match sit_request(&mut link, ch, SIT_SET_INITIAL_ATTACH_APN, &initial_attach_params(ATTACH_APN)) {
                Ok(tok) => line(rep, alloc::format!("t={} ms: -> SIT_SET_INITIAL_ATTACH_APN {} IPV4V6 ch A#{ch} token A#{tok}\n", since_us(t0) / 1000, alloc::string::String::from_utf8_lossy(ATTACH_APN))),
                Err(e) => line(rep, alloc::format!("t={} ms: SIT_SET_INITIAL_ATTACH_APN not sent: {e}\n", since_us(t0) / 1000)),
            }
        }
        if probe >= 4 && !data_on && since_us(t0) > 7_000_000 {
            data_on = true;
            let ch = CH_FMT0 + 1;
            for (id, v, name) in [(SIT_SET_PREFERRED_DATA_MODEM, 1u8, "SIT_SET_PREFERRED_DATA_MODEM 1"), (SIT_SET_PS_SERVICE, 1, "SIT_SET_PS_SERVICE on")] {
                match sit_request(&mut link, ch, id, &[v]) {
                    Ok(tok) => line(rep, alloc::format!("t={} ms: -> {name} ch A#{ch} token A#{tok}\n", since_us(t0) / 1000)),
                    Err(e) => line(rep, alloc::format!("t={} ms: {name} not sent: {e}\n", since_us(t0) / 1000)),
                }
            }
        }
        if probe >= 3 && !sim_asked && since_us(t0) > 6_000_000 {
            sim_asked = true;
            for ch in [CH_FMT0, CH_FMT0 + 1] {
                if let Err(e) = sit_request(&mut link, ch, SIT_GET_SIM_STATUS, &[]) {
                    line(rep, alloc::format!("t={} ms: SIT_GET_SIM_STATUS not sent on ch A#{ch}: {e}\n", since_us(t0) / 1000));
                }
            }
        }
        if probe >= 3 && since_us(t0) > next_reg {
            next_reg += 5_000_000;
            for ch in [CH_FMT0, CH_FMT0 + 1] {
                if let Err(e) = sit_request(&mut link, ch, SIT_GET_PS_REG_STATE, &[]) {
                    line(rep, alloc::format!("t={} ms: SIT_GET_PS_REG_STATE not sent on ch A#{ch}: {e}\n", since_us(t0) / 1000));
                }
            }
        }
        if probe >= 2 && !powered && since_us(t0) > 3_000_000 {
            powered = true;
            // state 2 = on; the two option bytes are Android's forEmergencyCall and preferredForEmergencyCall (NetworkService::DoRadioPower → ProtocolNetworkBuilder::BuildRadioPower): both 0, a plain power-on for data. Each stack has its own RIL channel (245 the first SIM slot, 246 the second) and is powered on its own, as each Android RIL instance does.
            for ch in [CH_FMT0, CH_FMT0 + 1] {
                match sit_request(&mut link, ch, SIT_SET_RADIO_POWER, &[2, 0, 0, 0, 0, 0]) {
                    Ok(tok) => line(rep, alloc::format!("t={} ms: -> SIT_SET_RADIO_POWER on (G#{SIT_SET_RADIO_POWER:X}) ch A#{ch} token A#{tok}\n", since_us(t0) / 1000)),
                    Err(e) => line(rep, alloc::format!("t={} ms: SIT_SET_RADIO_POWER not sent on ch A#{ch}: {e}\n", since_us(t0) / 1000)),
                }
            }
        }
        for (name, base, len_max, headr, tailr) in [("raw", RX_RING, RX_RING_LEN, RXQ_HEAD, RXQ_TAIL), ("fmt", FMT_RX_RING, FMT_RX_RING_LEN, FMT_RXQ_HEAD, FMT_RXQ_TAIL)] {
            let head = rd(headr);
            let tail = rd(tailr);
            if head >= len_max || tail >= len_max || head == tail {
                continue;
            }
            let at = |o: u32| rd8(base + ((tail + o) % len_max) as usize);
            if at(0) != 0xCD || at(1) != 0xAB {
                line(rep, alloc::format!("t={} ms: {name} head {head} tail {tail}: no frame header (G#{:02X}{:02X}), ring left\n", since_us(t0) / 1000, at(1), at(0)));
                return;
            }
            let flen = (at(6) as u32 | (at(7) as u32) << 8).max(12);
            let ch = at(8);
            per_ch[ch as usize] += 1;
            if probe >= 1 && name == "fmt" && ch == CH_MODEM_SVC {
                let body: Vec<u8> = (0..flen - 12).map(|i| at(12 + i)).collect();
                if let Some(replies) = modem_svc(rep, since_us(t0) / 1000, &body, &mut props) {
                    for reply in replies {
                        // The FMT ring is 4 KiB: a burst of replies waits for the CP to drain it, up to 200 ms a frame.
                        let t1 = now();
                        let mut r = fmt_send(&mut link, CH_MODEM_SVC, &reply);
                        while r.is_err() && since_us(t1) < 200_000 {
                            us(100);
                            r = fmt_send(&mut link, CH_MODEM_SVC, &reply);
                        }
                        if let Err(e) = r {
                            line(rep, alloc::format!("  modem_svc reply not sent: {e}\n"));
                            break;
                        }
                    }
                }
            }
            // The remote-file service answers for itself (rfs logs each request, never the data); its frames are not logged raw.
            let served_rfs = probe >= 1 && name == "raw" && ch == CH_RFS;
            if served_rfs {
                let body: Vec<u8> = (0..flen - 12).map(|i| at(12 + i)).collect();
                rfs(rep, since_us(t0) / 1000, &body, &mut link, &mut pull);
            }
            // A RIL message: the RCM header (type, id, length, then token and error for a response) decoded; the parameters as their first 16 bytes at most. The frame length is the CP's: a frame too short for the header it claims is logged raw below instead of read past its end.
            let ril = ch == CH_FMT0 || ch == CH_FMT0 + 1;
            let rcm_hdr = if at(12) == 2 { 8 } else { 12 };
            // The SIM's file-data and status chatter (G#210, G#246, G#248, G#24F) is counted, not logged: it is long, frequent and identity.
            let quiet = served_rfs || ril && flen >= 16 && matches!(at(14) as u16 | (at(15) as u16) << 8, 0x0210 | 0x0246 | 0x0248 | 0x024F);
            if quiet {
                // Counted in per_ch only.
            } else if ril && flen >= 12 + rcm_hdr && frames < 256 {
                let b = |i: u32| at(12 + i);
                let (ty, id, len) = (b(0), b(2) as u16 | (b(3) as u16) << 8, b(4) as u32 | (b(5) as u32) << 8);
                let kind = match ty {
                    0 => "request",
                    1 => "response",
                    2 => "indication",
                    _ => "type?",
                };
                let tok = b(6) as u32 | (b(7) as u32) << 8 | (b(8) as u32) << 16 | (b(9) as u32) << 24;
                let params = |i: u32| b(rcm_hdr + i);
                // Parameters only for messages that carry no identity (sit_shown); the rest by header alone.
                // Registration replies: the state, reject cause and radio technology (the first 4 bytes); the area and cell ids after them are location and stay hidden.
                let np = if id == 0x0700 || id == 0x0701 { (flen - 12 - rcm_hdr).min(4) } else if sit_shown(id) { (flen - 12 - rcm_hdr).min(16) } else { 0 };
                // SIM status (G#200): the card state, the application count, and each application's first 5 bytes (type, state, personalisation, PIN states); the AIDs and labels after them stay hidden.
                if id == 0x0200 && ty == 1 && flen - 12 >= rcm_hdr + 3 {
                    let apps = params(2) as u32;
                    let mut s = alloc::format!("card G#{:X} apps A#{apps}", params(0));
                    for k in 0..apps.min(4) {
                        if flen - 12 >= rcm_hdr + 3 + 63 * k + 5 {
                            s.push_str(&alloc::format!(" [{}]", hex(&|i| params(3 + 63 * k + i), 5)));
                        }
                    }
                    line(rep, alloc::format!("t={} ms: fmt ch A#{ch} SIM status: {s}\n", since_us(t0) / 1000));
                }
                let resp = if ty == 1 { alloc::format!(" token A#{tok} error G#{:X}", b(10)) } else { alloc::string::String::new() };
                line(rep, alloc::format!("t={} ms: fmt ch A#{ch} SIT {kind} G#{id:04X} {} len A#{len}{resp}: {}\n", since_us(t0) / 1000, sit_name(id), hex(&params, np)));
                frames += 1;
            } else if frames < 256 {
                let named = ch == 41 || (129..=136).contains(&ch);
                let n = (flen - 12).min(if named { 160 } else { 32 });
                let body = |i: u32| at(12 + i);
                let mut txt = alloc::string::String::new();
                if named {
                    for i in 0..n {
                        let b = body(i);
                        txt.push(if (0x20..0x7F).contains(&b) { b as char } else { '.' });
                    }
                }
                line(rep, alloc::format!("t={} ms: {name} ch A#{ch} cfg G#{:02X}{:02X} len A#{flen}: {} |{txt}|\n", since_us(t0) / 1000, at(5), at(4), hex(&body, n)));
                frames += 1;
            }
            wr(tailr, (tail + ((flen + 7) & !7)) % len_max);
        }
        for q in 0..PP_QUEUES {
            let rear = rd(pp_qinfo(q) + 16);
            if rear >= PP_NUM_DESC {
                line(rep, alloc::format!("t={} ms: pktproc q{q} rear_ptr A#{rear} out of range, queue left\n", since_us(t0) / 1000));
                return;
            }
            while done[q as usize] != rear {
                let i = done[q as usize];
                let d = rd64(pp_desc(q, i));
                let w = rd64(pp_desc(q, i) + 8);
                let (len, ch) = (w as u16 as u32, (w >> 48) as u8);
                if packets < 32 {
                    let b = pp_buf_ap(q, i);
                    let body = |o: u32| rd8(b + o as usize);
                    line(rep, alloc::format!("t={} ms: pktproc q{q} desc A#{i} ch A#{ch} len A#{len} done {} addr G#{:X}: {}\n", since_us(t0) / 1000, (d & PP_DONE != 0) as u32, d & 0xF_FFFF_FFFF, hex(&body, len.min(40))));
                }
                packets += 1;
                // Repost at fore (Linux's refill): the slot one behind the rear is free now that its packet is read.
                let fore = rd(pp_qinfo(q) + 12);
                pp_post(q, fore);
                unsafe { core::arch::asm!("dsb sy") };
                wr(pp_qinfo(q) + 12, (fore + 1) % PP_NUM_DESC);
                done[q as usize] = (i + 1) % PP_NUM_DESC;
            }
        }
        let msi = rd(DBI + 0x830);
        if msi != 0 {
            wr(DBI + 0x830, msi);
        }
        us(500);
    }
    let mut chs = alloc::string::String::new();
    for (ch, &n) in per_ch.iter().enumerate() {
        if n != 0 {
            chs.push_str(&alloc::format!(" ch A#{ch}: A#{n}"));
        }
    }
    line(rep, alloc::format!("watched: A#{frames} frames logged ({}), A#{packets} DL packets; cp2ap_msg G#{:X} PHONE_ACTIVE {}\n", if chs.is_empty() { " none" } else { &chs }, rd(CP2AP_MSG), PHONE_ACTIVE.get() as u32));
}
