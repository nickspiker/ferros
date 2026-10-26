//! The PMICs' on-device power meters (ODPM): the power drawn from every supply rail, for finding where the phone's power goes (POWER.md, 2026-10-25). Each PMIC — the S2MPG14 (main, ACPM speedy channel 0) and the S2MPG15 (sub, channel 1) — meters 12 channels at once, each muxed to any of its bucks or LDOs; its meter block answers at I2C address G#0A (ACPM type G#0A). Google's driver: drivers/regulator/s2mpg14-powermeter.c, s2mpg15-powermeter.c, include/linux/mfd/samsung/s2mpg1415-meter.h; the rail names are zuma-pmic.dtsi's schematic names.
//!
//! A sweep (`diag odpm start`, then `diag odpm`) takes every rail in batches of 12 per PMIC, both PMICs at once, from the service loop's own passes — never busy-waiting on the kernel's core, which would load the CPU rails it measures. Per batch: the channel selectors set, a quarter second to settle, the accumulators latched (CTRL2 ASYNC_RD) and read (41-bit energy per channel, a shared 20-bit sample count), a second, latched and read again; the average power over that second is the energy difference over the count difference times the rail's resolution (mW per count, by regulator type, from s2mpg1415-register.h).

use crate::acpm;

const METER: u32 = 0x0A;
const CTRL1: u32 = 0x08;
const CTRL2: u32 = 0x09;
const CTRL4: u32 = 0x0B;
const CTRL5: u32 = 0x0C;
const BUCKEN1: u32 = 0x0F;
const MUXSEL0: u32 = 0x11;
const ACC_DATA0: u32 = 0x63;
const ACC_COUNT: u32 = 0xAB;
const CHANNELS: usize = 12;

// Resolutions, mW per count (s2mpg1415-register.h).
const CMS: f32 = 0.006868131868131;
const CMD: f32 = 0.013736263736263;
const CMT: f32 = 0.020604395604395;
const VM: f32 = 0.013736263736263;
const BB: f32 = 0.010989011;
const NLDO150: f32 = 0.000457875457875;
const PLDO150: f32 = 0.000915750915750;
const NLDO300: f32 = 0.000915750915750;
const PLDO300: f32 = 0.001831501831501;
const DVS450: f32 = 0.000686813;
const NLDO450: f32 = 0.001373626373626;
const PLDO600: f32 = 0.003663003663003;
const DVS800OFF: f32 = 0.002442002442002;
const DVS800ON: f32 = 0.001221001221001;
const NLDO800: f32 = 0.002442002442002;
const PLDO800: f32 = 0.004884004884004;
const DVS1200ON: f32 = 0.001831501831501;
const NLDO1200: f32 = 0.001831501831501;

/// (selector, schematic name, resolution) per rail.
type Rail = (u8, &'static str, f32);

static MAIN_RAILS: [Rail; 34] = [
    (0x01, "S1M_VDD_MIF", CMS),
    (0x02, "S2M_VDD_CPUCL2", CMD),
    (0x03, "S3M_VDD_CPUCL1", CMT),
    (0x04, "S4M_VDD_CPUCL0", CMD),
    (0x05, "S5M_VDD_INT", CMT),
    (0x06, "S6M_LLDO1", CMS),
    (0x07, "S7M_VDD_TPU", CMT),
    (0x08, "S8M_LLDO2", CMS),
    (0x09, "S9M_VDD_CPUCL0_M", CMS),
    (0x21, "L1M_ALIVE", DVS800OFF),
    (0x22, "L2M_ALIVE", PLDO800),
    (0x23, "L3M_VDD_AOC_RET", DVS450),
    (0x24, "L4M_TS1_AVDD", PLDO300),
    (0x25, "L5M_S5910_VDD18VD", PLDO150),
    (0x26, "L6M_HSI", NLDO800),
    (0x27, "L7M_DPAUX", PLDO150),
    (0x28, "L8M_EUSB", DVS450),
    (0x29, "L9M_EUSB", NLDO300),
    (0x2A, "L10M_AOC", NLDO300),
    (0x2B, "L11M_VDD_CPUCL1_M", DVS1200ON),
    (0x2C, "L12M", DVS1200ON),
    (0x2D, "L13M_VDD_TPU_M", DVS800ON),
    (0x2E, "L14M_DISP_VCI", PLDO150),
    (0x2F, "L15M", DVS1200ON),
    (0x30, "L16M_PCIE0", NLDO450),
    (0x31, "L17M_VDD_CPUCL2_M", DVS800ON),
    (0x32, "L18M_PCIE0", NLDO150),
    (0x33, "L19M_EUSB", PLDO150),
    (0x34, "L20M_DMIC1", PLDO150),
    (0x35, "L21M_GSC", PLDO300),
    (0x36, "L22M_DISP_VDDD", NLDO450),
    (0x37, "L23M", PLDO150),
    (0x38, "L24M", PLDO600),
    (0x39, "L25M_TS1_DVDD", PLDO300),
];

static SUB_RAILS: [Rail; 45] = [
    (0x01, "S1S_VDD_CAM", CMD),
    (0x02, "S2S_VDD_G3D", CMD),
    (0x03, "S3S_LLDO1", CMS),
    (0x04, "S4S_VDD2H_MEM", CMS),
    (0x05, "S5S_VDDQ_MEM", CMS),
    (0x06, "S6S_LLDO2", CMS),
    (0x07, "S7S_MLDO", VM),
    (0x08, "S8S_VDD_G3D_L2", CMS),
    (0x09, "S9S_VDD_AOC", CMS),
    (0x0A, "S10S_VDD_SLC_M", CMS),
    (0x0B, "S11S_G3D_GLB", CMS),
    (0x0C, "S12S_VDD_AUR", CMD),
    (0x0D, "SD", VM),
    (0x0E, "SA", VM),
    (0x0F, "SC", CMS),
    (0x10, "BB_HLDO", BB),
    (0x21, "L1S_VDD_G3D_M", DVS1200ON),
    (0x22, "L2S_PLL_MIPI_UFS", NLDO1200),
    (0x23, "L3S_PCIE1", NLDO450),
    (0x24, "L4S", PLDO150),
    (0x25, "L5S_PROX", PLDO150),
    (0x26, "L6S", PLDO150),
    (0x27, "L7S_SENSORS_FPS", PLDO150),
    (0x28, "L8S", PLDO150),
    (0x29, "L9S_GNSS_CORE", NLDO150),
    (0x2A, "L10S_GNSS_RF", PLDO150),
    (0x2B, "L11S_GNSS_AUX", PLDO150),
    (0x2C, "L12S_TS2_DVDD", PLDO300),
    (0x2D, "L13S", PLDO300),
    (0x2E, "L14S", PLDO300),
    (0x2F, "L15S_UDFPS", PLDO150),
    (0x30, "L16S", PLDO150),
    (0x31, "L17S", PLDO150),
    (0x32, "L18S", PLDO150),
    (0x33, "L19S_DMIC3", PLDO150),
    (0x34, "L20S_DMIC45", PLDO150),
    (0x35, "L21S_VDD2L_MEM", NLDO800),
    (0x36, "L22S", PLDO150),
    (0x37, "L23S", DVS800OFF),
    (0x38, "L24S_HSI", NLDO800),
    (0x39, "L25S", PLDO150),
    (0x3A, "L26S_TS2_AVDD", PLDO150),
    (0x3B, "L27S", PLDO150),
    (0x3C, "L28S", PLDO150),
    (0x3D, "L29S", PLDO150),
];

fn rd(pmic: u32, reg: u32) -> u8 {
    acpm::read(pmic, METER, reg).unwrap_or(0)
}
fn wr(pmic: u32, reg: u32, v: u8) {
    let _ = acpm::write(pmic, METER, reg, v);
}

/// The meter on, sampling at 125 Hz (Google's rate), every buck enabled for metering, accumulating power.
fn enable(pmic: u32) {
    // CTRL1: METER_EN (bit 0), INT_SAMP_RATE (bits 4:2) = 4, 125 Hz.
    let c1 = rd(pmic, CTRL1);
    wr(pmic, CTRL1, (c1 & !0x1C) | 4 << 2 | 1);
    wr(pmic, BUCKEN1, 0xFF);
    wr(pmic, BUCKEN1 + 1, 0xFF);
    // Accumulator mode: power on every channel (CTRL4 = 0, CTRL5's low nibble = 0).
    wr(pmic, CTRL4, 0);
    let c5 = rd(pmic, CTRL5);
    wr(pmic, CTRL5, c5 & !0x0F);
}

/// Latch the accumulators (ASYNC_RD, self-clearing) and read them: the 12 channels' 41-bit energies and the shared sample count.
fn latch(pmic: u32) -> ([u64; CHANNELS], u32) {
    let c2 = rd(pmic, CTRL2);
    wr(pmic, CTRL2, c2 | 0x80);
    for _ in 0..64 {
        if rd(pmic, CTRL2) & 0x80 == 0 {
            break;
        }
    }
    let mut acc = [0u64; CHANNELS];
    for (ch, a) in acc.iter_mut().enumerate() {
        let base = ACC_DATA0 + (ch as u32) * 6;
        let mut v = 0u64;
        for k in 0..6u32 {
            v |= (rd(pmic, base + k) as u64) << (8 * k);
        }
        *a = v & ((1u64 << 41) - 1);
    }
    let count = rd(pmic, ACC_COUNT) as u32 | (rd(pmic, ACC_COUNT + 1) as u32) << 8 | ((rd(pmic, ACC_COUNT + 2) & 0x0F) as u32) << 16;
    (acc, count)
}

/// One sweep in progress: the batch, its phase (0 selectors set and settling, 1 first latch taken), when the phase ends, the first latch of each PMIC, and every rail's average power so far (mW; main rails first, then sub).
struct Sweep {
    batch: usize,
    tries: u8,
    phase: u8,
    until: u64,
    first: [([u64; CHANNELS], u32); 2],
    mw: [f32; 34 + 45],
}
static mut SWEEP: Option<Sweep> = None;
static mut RESULT: Option<alloc::string::String> = None;

fn rails(pmic: u32) -> &'static [Rail] {
    if pmic == acpm::MAIN { &MAIN_RAILS } else { &SUB_RAILS }
}

fn batches() -> usize {
    MAIN_RAILS.len().max(SUB_RAILS.len()).div_ceil(CHANNELS)
}

fn select(batch: usize) {
    for pmic in [acpm::MAIN, acpm::SUB] {
        let r = rails(pmic);
        for ch in 0..CHANNELS {
            let sel = r.get(batch * CHANNELS + ch).map(|x| x.0).unwrap_or(0);
            wr(pmic, MUXSEL0 + ch as u32, sel);
        }
    }
}

/// `diag odpm start`: both meters on and a sweep begun.
pub fn start() -> &'static str {
    if unsafe { (*(&raw const SWEEP)).is_some() } {
        return "a sweep is already running\n";
    }
    enable(acpm::MAIN);
    enable(acpm::SUB);
    select(0);
    let f = crate::platform::cntfrq();
    unsafe {
        RESULT = None;
        // The first batch two seconds out: the command that starts a sweep holds the clock policy active for a second, and its switch back would reset the meters under the first batch.
        SWEEP = Some(Sweep { batch: 0, tries: 0, phase: 0, until: crate::platform::cntpct() + 2 * f, first: [([0; CHANNELS], 0); 2], mw: [f32::NAN; 34 + 45] });
    }
    "odpm sweep started: every rail of both PMICs, 12 at a time, a second each; read it with `diag odpm`\n"
}

/// From the service loop: the running sweep's next step when one is due.
pub fn poll() {
    let Some(s) = (unsafe { (*(&raw mut SWEEP)).as_mut() }) else { return };
    let (f, now) = (crate::platform::cntfrq(), crate::platform::cntpct());
    if now < s.until {
        return;
    }
    if s.phase == 0 {
        s.first = [latch(acpm::MAIN), latch(acpm::SUB)];
        s.phase = 1;
        s.until = now + f;
        return;
    }
    // A batch whose accumulators went backwards, or whose sample count did not move, was reset under it (a clock switch does it: the first screen-off sweep read 14, 28 and 43 W, exact multiples of a wrapped difference, 2026-10-25): measured again, up to three tries; after that its rails read unknown, never a number.
    let latches = [latch(acpm::MAIN), latch(acpm::SUB)];
    let sane = latches.iter().zip(s.first.iter()).all(|((acc, count), (acc0, count0))| *count > *count0 && acc.iter().zip(acc0.iter()).all(|(a, a0)| a >= a0));
    if !sane && s.tries < 3 {
        s.tries += 1;
        s.phase = 0;
        s.until = now + f / 4;
        return;
    }
    for (k, pmic) in [acpm::MAIN, acpm::SUB].into_iter().enumerate() {
        let (acc, count) = latches[k];
        let (acc0, count0) = s.first[k];
        let n = count.wrapping_sub(count0) as f32;
        let r = rails(pmic);
        for ch in 0..CHANNELS {
            let i = s.batch * CHANNELS + ch;
            if let Some(&(_, _, res)) = r.get(i) {
                let slot = if k == 0 { i } else { MAIN_RAILS.len() + i };
                s.mw[slot] = if sane && n > 0.0 { (acc[ch] - acc0[ch]) as f32 / n * res } else { f32::NAN };
            }
        }
    }
    s.tries = 0;
    s.batch += 1;
    if s.batch < batches() {
        select(s.batch);
        s.phase = 0;
        s.until = now + f / 4;
        return;
    }
    // Done: every rail, largest first, and the sums.
    let mut all: alloc::vec::Vec<(&str, f32)> = MAIN_RAILS.iter().chain(SUB_RAILS.iter()).zip(s.mw.iter()).map(|(r, &m)| (r.1, m)).collect();
    let (main_sum, sub_sum): (f32, f32) = (s.mw[..34].iter().filter(|m| m.is_finite()).sum(), s.mw[34..].iter().filter(|m| m.is_finite()).sum());
    all.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
    let unknown = s.mw.iter().filter(|m| !m.is_finite()).count();
    let mut out = alloc::format!("odpm: {unknown} rails unknown (reset under every try); main PMIC rails {:.0} mW, sub PMIC rails {:.0} mW, total {:.0} mW (rail outputs, before conversion losses; the panel's VSYS feed and the radios' are not among them)\n", main_sum, sub_sum, main_sum + sub_sum);
    for (name, m) in all {
        if m.is_finite() && m >= 0.5 {
            out += &alloc::format!("{m:8.1} mW  {name}\n");
        }
    }
    unsafe {
        RESULT = Some(out);
        SWEEP = None;
    }
}

/// `diag odpm`: the last sweep's result, or its progress.
pub fn report(out: &mut alloc::vec::Vec<u8>) {
    match unsafe { (&*(&raw const SWEEP), &*(&raw const RESULT)) } {
        (Some(s), _) => out.extend_from_slice(alloc::format!("sweep running: batch {} of {}\n", s.batch + 1, batches()).as_bytes()),
        (None, Some(r)) => out.extend_from_slice(r.as_bytes()),
        (None, None) => out.extend_from_slice(b"no sweep yet: diag odpm start\n"),
    }
}
