//! The SoC's heat (docs/THERMAL.md): every TMU zone read over ACPM once a second on the service loop, the kernel's own core's clock measured beside it, the hardware clock dividers' status, and the thermal policy that caps the clusters' and the GPU's rates.
//!
//! The policy works only thru caps: the clock policy (service.rs `clock_profile`, `gpu_rate`) asks for each domain's rate thru `capped`, so a profile switch never undoes a cap (until 2026-11-12 the throttle set the X3 by hand and the next profile switch put it back).
//! Two policies, `diag thermal policy step|smooth|off`; every boot starts at `step`:
//! - step (the default, as since 2026-10-23): the hottest CPU zone at `AT` (95 °C) caps the X3 at 1.4 GHz; below `AT` less 10 the cap goes.
//! - smooth (built 2026-11-12, off by default, untested on the device): each controlled domain's cap moves every second by an amount that grows smoothly with how far its own zone sits from its control temperature (Android's passive trips), down while above, back up while below, between the domain's floor in zuma-cpu.dtsi and the rate the clock policy wants.
//! - off: no cap at all (for measuring heat; the firmware's and the hardware's own protections stay).

use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering::Relaxed};

use crate::platform::{cntfrq, cntpct};

/// The TMU zones in ACPM's order (acpm::TMU_ZONES).
pub const ZONES: usize = 7;
/// Each zone's temperature as last read, whole °C; 0 = never read.
pub static TEMP: [AtomicU8; ZONES] = [const { AtomicU8::new(0) }; ZONES];
/// Each zone's hottest reading since boot (or since `diag thermal reset`).
static PEAK: [AtomicU8; ZONES] = [const { AtomicU8::new(0) }; ZONES];
/// Each zone's failed reads, and the last failure's code (acpm `E_*`, the firmware's own code in bits 8-15). A zone that fails keeps its last good reading in `TEMP`, so these say when that reading is stale.
static READ_ERRS: [AtomicU32; ZONES] = [const { AtomicU32::new(0) }; ZONES];
static READ_ERR_LAST: [AtomicU32; ZONES] = [const { AtomicU32::new(0) }; ZONES];

/// The hottest zone and its temperature, None until a zone has been read.
pub fn hottest() -> Option<(usize, u8)> {
    let mut best = None;
    for (i, t) in TEMP.iter().enumerate() {
        let c = t.load(Relaxed);
        if c != 0 && best.is_none_or(|(_, b)| c > b) {
            best = Some((i, c));
        }
    }
    best
}

/// One domain the policy controls: its ACPM DVFS domain (acpm::DVFS_DOMAINS), the TMU zone that heats with it, Android's control temperature for that zone (zuma-b0-ipop.dts, each zone's passive `control-temp` trip, the one its cooling map hangs on), and the domain's floor (zuma-cpu.dtsi min-freq; the GPU's lowest operating point in zuma-gpu.dtsi). Never below the floor: ACPM answers Ok to a rate under it and the device hard-resets (CPUCL0 at 300 MHz, 2026-10-25).
struct Ctl {
    domain: u32,
    zone: usize,
    trip: u8,
    floor: u32,
}
const CTL: [Ctl; 4] = [
    // The X3 (CPUCL2), zone BIG.
    Ctl { domain: 4, zone: 0, trip: 90, floor: 500_000 },
    // The A715s (CPUCL1), zone MID.
    Ctl { domain: 3, zone: 1, trip: 90, floor: 402_000 },
    // The A510s (CPUCL0), zone LITTLE.
    Ctl { domain: 2, zone: 2, trip: 95, floor: 324_000 },
    // The GPU (G3D), zone G3D.
    Ctl { domain: 7, zone: 3, trip: 90, floor: 150_000 },
];
const CTL_NAMES: [&str; 4] = ["X3", "A715", "A510", "GPU"];

/// Each controlled domain's cap, kHz; u32::MAX = none.
static CAP: [AtomicU32; 4] = [const { AtomicU32::new(u32::MAX) }; 4];
/// The rate last asked of each controlled domain (by the clock policy thru `capped`, or by the policy itself), kHz; 0 = never.
static ASKED: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];

/// The policy: 0 step, 1 smooth, 2 off.
static POLICY: AtomicU8 = AtomicU8::new(0);
pub const STEP: u8 = 0;
pub const SMOOTH: u8 = 1;
pub const OFF: u8 = 2;
/// The step policy's trip (°C; release below it less 10), and a ceiling on every smooth trip, so `diag thermal at <C>` tests either policy on a cool device. Every boot starts at 95: Android's control temperature for the little cluster, its big and mid ones being 90.
pub static AT: AtomicU8 = AtomicU8::new(95);
/// The X3's cap under the step policy: half its top rate (measured 2026-10-23, the core then counts ~1297 M dependent adds a second).
const STEP_KHZ: u32 = 1_400_000;

/// The kernel's own core's clock as measured on each read (`measure_mhz`), the lowest since boot, and the rate the firmware holds for the X3 (acpm::dvfs_get), all MHz.
static CORE_MHZ: AtomicU32 = AtomicU32::new(0);
static CORE_MHZ_MIN: AtomicU32 = AtomicU32::new(u32::MAX);
static X3_HELD_MHZ: AtomicU32 = AtomicU32::new(0);
/// Seconds (reads) in which the kernel's core ran more than an eighth under the rate the firmware holds for it: no DVFS rate does that (the X3's floor is 500 MHz), a clock divider does (2026-11-12: 364 MHz = 2914 / 8).
static SLOW_S: AtomicU64 = AtomicU64::new(0);
/// Seconds in which the firmware held the X3 more than an eighth under what ferros last asked: the ACPM firmware's own cap (its thermal governor, or a power limit).
static FW_CAP_S: AtomicU64 = AtomicU64::new(0);
/// Seconds in which any controlled domain was capped by ferros's policy.
static CAPPED_S: AtomicU64 = AtomicU64::new(0);

/// The hardware clock dividers' status (CLKDIVSTEP_STAT; google_bcl, zuma-bcl.h): the clusters' and the GPU's, as last read and every bit seen since boot. The divider steps a domain's clock down in hardware when the PMIC warns (its buck's over-current OCP_WARN / SOFT_OCP_WARN, the battery's SMPL_WARN, VDROOP); ACPM's power-management firmware arms it at every boot (flexpmu_cal_system_zuma.h `pmucal_lpm_init`: CPUCL1/2 CLKDIVSTEP = G#210407, OCP/VDROOP/SMPL filters), and ferros never touches it. Which bit means which source is not documented in the tree: the seen bits after a slow second are the evidence.
const DIV_NAMES: [&str; 4] = ["CPUCL0", "CPUCL1", "CPUCL2", "G3D"];
/// Each divider block's base and its status word's offset.
const DIV: [(usize, usize); 4] = [(0x29C0_0000, 0x83C), (0x29D0_0000, 0x848), (0x29D8_0000, 0x848), (0x1EE0_0000, 0x854)];
static DIV_LAST: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static DIV_SEEN: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
/// pd_g3d's status in the PMU, bit 0 on (a plain read, GPU.md): the GPU's divider block answers only while it is on.
const PD_G3D_STATUS: usize = 0x1546_1A84;

fn rd(a: usize) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}

/// Whether the kernel runs on the X3 (MPIDR Aff1 8, CPU8): a FERROS_X3=0 build keeps CPU0 and leaves the X3 parked.
fn on_x3() -> bool {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
    (mpidr >> 8) & 0xFF == 8
}

/// Whether a divider block can be read now. CPUCL0 always (Linux reads it unconditionally). CPUCL1 while any A715 worker is up (the dark profile stops them, and their cluster may then be down). CPUCL2 while the kernel runs on the X3. G3D while its power domain is on.
fn div_readable(k: usize) -> bool {
    match k {
        0 => true,
        1 => crate::smp::UP.load(Relaxed) != 0,
        2 => on_x3(),
        _ => rd(PD_G3D_STATUS) & 1 == 1,
    }
}

/// The kernel's own core's clock now, MHz: 2^16 dependent adds (one a cycle on every core of the SoC) with IRQs held off, so nothing but the clock stretches them, timed on the system counter. 22 us at 2.9 GHz, 180 us at 364 MHz; the counter's 24.576 MHz gives at least 540 ticks, so the time is never zero and the reading is good to 0.2 %.
fn measure_mhz() -> u32 {
    let daif: u64;
    let mut v = 0u64;
    unsafe { core::arch::asm!("mrs {}, daif", "msr daifset, #2", out(reg) daif) };
    let t0 = cntpct();
    for _ in 0..1 << 14 {
        unsafe { core::arch::asm!("add {v}, {v}, #1", "add {v}, {v}, #1", "add {v}, {v}, #1", "add {v}, {v}, #1", v = inout(reg) v, options(nomem, nostack)) };
    }
    let dt = cntpct() - t0;
    unsafe { core::arch::asm!("msr daif, {}", in(reg) daif) };
    (v * cntfrq() / dt / 1_000_000) as u32
}

/// The rate to ask of a DVFS domain: the clock policy's `khz` under the domain's cap, if the policy controls it; recorded as asked. Called by the clock policy for every request it makes.
pub fn capped(domain: u32, khz: u32) -> u32 {
    match CTL.iter().position(|c| c.domain == domain) {
        Some(i) => {
            // The cap is the policy's whole action: the lower of what the clock policy wants and what the heat allows.
            let r = khz.min(CAP[i].load(Relaxed));
            ASKED[i].store(r, Relaxed);
            r
        }
        None => khz,
    }
}

/// A rate asked by hand (`diag dvfs set`), past the caps: recorded as asked, so the firmware-capped count and the policy's next request start from it.
pub fn note_asked(domain: u32, khz: u32) {
    if let Some(i) = CTL.iter().position(|c| c.domain == domain) {
        ASKED[i].store(khz, Relaxed);
    }
}

/// Whether ferros's policy holds any domain under the rate the clock policy wants (fact 11).
pub fn throttled() -> bool {
    CAP.iter().enumerate().any(|(i, c)| crate::service::profile_khz(CTL[i].domain).is_some_and(|top| c.load(Relaxed) < top))
}

/// The X3's rate the clock policy wants while the policy caps it under that, kHz; 0 when not capped (DIAG THROTTLED_MHZ, in MHz).
pub fn x3_capped_from() -> u32 {
    match crate::service::profile_khz(CTL[0].domain) {
        Some(top) if CAP[0].load(Relaxed) < top => top,
        _ => 0,
    }
}

/// Ask controlled domain `i` for its capped rate if that differs from what it was last asked: a request, then a rate read, which the firmware answers only once the request is done (service.rs `clock_profile`, why). Nothing while the clock policy has no rate for it (before its first switch, `diag dvfs policy off`, or the GPU with no `gpu` process).
fn apply(i: usize) {
    let Some(top) = crate::service::profile_khz(CTL[i].domain) else { return };
    let r = top.min(CAP[i].load(Relaxed));
    if ASKED[i].load(Relaxed) != r {
        ASKED[i].store(r, Relaxed);
        let _ = crate::acpm::dvfs_set(CTL[i].domain, r);
        let _ = crate::acpm::dvfs_get(CTL[i].domain);
    }
}

/// Every cap gone, each domain back at the clock policy's rate.
fn uncap_all() {
    for (i, c) in CAP.iter().enumerate() {
        if c.swap(u32::MAX, Relaxed) != u32::MAX {
            apply(i);
        }
    }
}

/// `diag thermal policy step|smooth|off`.
pub fn set_policy(p: u8) {
    if POLICY.swap(p, Relaxed) != p {
        uncap_all();
    }
}

/// From the service loop on the kernel's core; does its work once a second and says whether it did (the facts' bell rings after it). Every zone over ACPM (28 us each, measured 2026-10-23), the kernel's core's clock, the firmware's rate for the X3, the dividers' status, then the policy.
/// A read on the kernel's existing once-a-second pass and not an interrupt: the TMU's threshold interrupts go to the ACPM firmware (it owns the TMU, CAP_APM_IRQ), and the die's heat moves over seconds, so a second's latency costs a degree or two at most; the hardware divider and the firmware's own trips act within microseconds and milliseconds behind it.
pub fn poll() -> bool {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let (f, now) = (cntfrq(), cntpct());
    let last = LAST.load(Relaxed);
    if last != 0 && now.wrapping_sub(last) < f {
        return false;
    }
    LAST.store(now, Relaxed);
    for i in 0..ZONES {
        match crate::acpm::tmu_read(i as u32) {
            Ok((c, _, _)) => {
                TEMP[i].store(c, Relaxed);
                PEAK[i].fetch_max(c, Relaxed);
            }
            Err(e) => {
                READ_ERRS[i].fetch_add(1, Relaxed);
                READ_ERR_LAST[i].store(e, Relaxed);
            }
        }
    }
    let mhz = measure_mhz();
    CORE_MHZ.store(mhz, Relaxed);
    CORE_MHZ_MIN.fetch_min(mhz, Relaxed);
    if let Ok(k) = crate::acpm::dvfs_get(CTL[0].domain) {
        let held = k / 1000;
        X3_HELD_MHZ.store(held, Relaxed);
        if on_x3() && mhz < held - (held >> 3) {
            SLOW_S.fetch_add(1, Relaxed);
        }
        let asked = ASKED[0].load(Relaxed) / 1000;
        if held < asked - (asked >> 3) {
            FW_CAP_S.fetch_add(1, Relaxed);
        }
    }
    for k in 0..DIV.len() {
        if div_readable(k) {
            let s = rd(DIV[k].0 + DIV[k].1);
            DIV_LAST[k].store(s, Relaxed);
            DIV_SEEN[k].fetch_or(s, Relaxed);
        }
    }
    match POLICY.load(Relaxed) {
        STEP => step(),
        SMOOTH => smooth(),
        _ => {}
    }
    if CAP.iter().any(|c| c.load(Relaxed) != u32::MAX) {
        CAPPED_S.fetch_add(1, Relaxed);
    }
    true
}

/// The step policy: the hottest CPU zone (big, mid, little) at `AT` caps the X3 at STEP_KHZ, below `AT` less 10 the cap goes. `AT` is at least 20 (`diag thermal at` keeps it there), so the release point is never below 10.
fn step() {
    let cpu = TEMP[..3].iter().map(|t| t.load(Relaxed)).max().unwrap_or(0);
    let at = AT.load(Relaxed);
    let capped = CAP[0].load(Relaxed) != u32::MAX;
    if !capped && cpu >= at {
        CAP[0].store(STEP_KHZ, Relaxed);
        apply(0);
    } else if capped && cpu < at - 10 {
        CAP[0].store(u32::MAX, Relaxed);
        apply(0);
    }
}

/// The smooth policy: each controlled domain's cap moves once a second by s·e/(|e| + 4), e its zone's temperature less its control temperature (°C) and s a sixteenth of the rate the clock policy wants for it: a degree over takes a fifth of s off, four degrees half of it, far over nearly all of it; under the trip it climbs back the same way. The step's slope is continuous in e (1/4 of s a degree at e = 0), so the cap has no kink as the zone crosses its trip, and a second's change is at most a sixteenth of the rate: a frame's time stretches by at most about 6 % from one second to the next, never a cliff.
/// The cap stays between the domain's floor (the device resets under it) and the clock policy's rate (above it the cap would only wind up, to be unwound slowly when heat comes); at the top it is no cap at all.
fn smooth() {
    let at = AT.load(Relaxed);
    for (i, c) in CTL.iter().enumerate() {
        let Some(top) = crate::service::profile_khz(c.domain) else { continue };
        let t = TEMP[c.zone].load(Relaxed);
        if t == 0 {
            continue;
        }
        let e = t as i64 - c.trip.min(at) as i64;
        let s = (top >> 4) as i64;
        let was = CAP[i].load(Relaxed).min(top) as i64;
        let want = (was - s * e / (e.abs() + 4)).max(c.floor as i64);
        CAP[i].store(if want >= top as i64 { u32::MAX } else { want as u32 }, Relaxed);
        apply(i);
    }
}

/// `diag thermal reset`: the peaks, the error counts, the slow and capped seconds, the lowest clock and the dividers' seen bits start again.
pub fn reset() {
    for i in 0..ZONES {
        PEAK[i].store(TEMP[i].load(Relaxed), Relaxed);
        READ_ERRS[i].store(0, Relaxed);
        READ_ERR_LAST[i].store(0, Relaxed);
    }
    for s in [&SLOW_S, &FW_CAP_S, &CAPPED_S] {
        s.store(0, Relaxed);
    }
    CORE_MHZ_MIN.store(u32::MAX, Relaxed);
    for (k, s) in DIV_SEEN.iter().enumerate() {
        s.store(DIV_LAST[k].load(Relaxed), Relaxed);
    }
}

/// `diag thermal`: every zone (now, peak, status, probes, failed reads), the hottest named, the kernel's core's clock against the firmware's rate, the slow and capped seconds, the dividers, the policy and its caps.
pub fn report(out: &mut alloc::vec::Vec<u8>) {
    use alloc::format;
    let t0 = cntpct();
    let _ = crate::acpm::tmu_read(0);
    let us = cntpct().wrapping_sub(t0) * 1_000_000 / cntfrq();
    let p = POLICY.load(Relaxed);
    let pname = match p {
        STEP => "step",
        SMOOTH => "smooth",
        _ => "off",
    };
    out.extend_from_slice(format!("policy {pname}, at A#{} C; one zone read A#{us} us\n", AT.load(Relaxed)).as_bytes());
    match hottest() {
        Some((i, c)) => out.extend_from_slice(format!("hottest: {} A#{c} C\n", crate::acpm::TMU_ZONES[i]).as_bytes()),
        None => out.extend_from_slice(b"hottest: no zone read yet\n"),
    }
    for (i, name) in crate::acpm::TMU_ZONES.iter().enumerate() {
        let (errs, last) = (READ_ERRS[i].load(Relaxed), READ_ERR_LAST[i].load(Relaxed));
        match crate::acpm::tmu_read(i as u32) {
            Ok((t, st, pr)) => out.extend_from_slice(format!("{name:>6}: A#{t} C, peak A#{} C, stat G#{st:X}, probes {:?}, failed reads A#{errs} (last G#{last:X})\n", PEAK[i].load(Relaxed), pr).as_bytes()),
            Err(e) => out.extend_from_slice(format!("{name:>6}: error G#{e:X} now, last good A#{} C, peak A#{} C, failed reads A#{errs}\n", TEMP[i].load(Relaxed), PEAK[i].load(Relaxed)).as_bytes()),
        }
    }
    let min = CORE_MHZ_MIN.load(Relaxed);
    out.extend_from_slice(format!("kernel's core: A#{} MHz measured now (lowest A#{} MHz), the firmware holds the X3 at A#{} MHz, ferros asked A#{} MHz\n", measure_mhz(), if min == u32::MAX { 0 } else { min }, X3_HELD_MHZ.load(Relaxed), ASKED[0].load(Relaxed) / 1000).as_bytes());
    out.extend_from_slice(format!("seconds: slow A#{} (the X3 more than 1/8 under the firmware's rate: a clock divider), firmware-capped A#{} (the firmware more than 1/8 under ferros's ask), capped by ferros A#{}\n", SLOW_S.load(Relaxed), FW_CAP_S.load(Relaxed), CAPPED_S.load(Relaxed)).as_bytes());
    out.extend_from_slice(b"dividers (CLKDIVSTEP_STAT, last / every bit seen):");
    for (k, n) in DIV_NAMES.iter().enumerate() {
        out.extend_from_slice(format!(" {n} G#{:X} / G#{:X};", DIV_LAST[k].load(Relaxed), DIV_SEEN[k].load(Relaxed)).as_bytes());
    }
    out.push(b'\n');
    out.extend_from_slice(b"caps:");
    for (i, n) in CTL_NAMES.iter().enumerate() {
        let c = CAP[i].load(Relaxed);
        let trip = CTL[i].trip.min(AT.load(Relaxed));
        if c == u32::MAX {
            out.extend_from_slice(format!(" {n} none (trip A#{trip} C);").as_bytes());
        } else {
            out.extend_from_slice(format!(" {n} A#{} MHz (trip A#{trip} C);", c / 1000).as_bytes());
        }
    }
    out.push(b'\n');
}

/// `diag thermal hw`: what arms the hardware's own protections, read only. The dividers' configuration words (CLKDIVSTEP, its filters and its two step sizes), the PMIC's warning thresholds that drive them (S2MPG14 B2M the X3, B3M the A715s, B7M the TPU; S2MPG15 B2S the GPU; their OCP_WARN and SOFT_OCP_WARN: bit 7 on, bits 0-4 the level, Linux's level 0 = 8 A on a two-phase buck, 13.5 A on a three-phase one, each step 0.166 A or 0.25 A lower; the main PMIC's SMPL_WARN_CTRL, bits 5-7 its level), and the ACPM firmware's thermal governor as it reports itself (TMU_IPC_GET_TARGET_FREQ, and each zone's eight trip counters, TMU_IPC_GET_TRIP_CNT: Linux reads both from sysfs).
pub fn report_hw(out: &mut alloc::vec::Vec<u8>) {
    use alloc::format;
    for (k, n) in DIV_NAMES.iter().enumerate().take(3) {
        if !div_readable(k) {
            out.extend_from_slice(format!("{n}: not readable now\n").as_bytes());
            continue;
        }
        let b = DIV[k].0;
        out.extend_from_slice(format!("{n} G#{b:X} + G#830..G#848:").as_bytes());
        for o in (0x830..=0x848).step_by(4) {
            out.extend_from_slice(format!(" G#{:X}", rd(b + o)).as_bytes());
        }
        out.push(b'\n');
    }
    let pm = |s: u32, r: u32| crate::acpm::read(s, crate::acpm::T_PMIC, r).map(|v| format!("G#{v:X}")).unwrap_or_else(|e| format!("error G#{e:X}"));
    use crate::acpm::{MAIN, SUB};
    out.extend_from_slice(format!("S2MPG14 OCP_WARN B2M (X3) {} B3M (A715) {} B7M (TPU) {}; SOFT B2M {} B3M {} B7M {}; debounce {} {}; SMPL_WARN_CTRL {}\n", pm(MAIN, 0xBB), pm(MAIN, 0xBC), pm(MAIN, 0xBD), pm(MAIN, 0xBE), pm(MAIN, 0xBF), pm(MAIN, 0xC0), pm(MAIN, 0xC1), pm(MAIN, 0xC2), pm(MAIN, 0x16)).as_bytes());
    out.extend_from_slice(format!("S2MPG15 OCP_WARN B2S (GPU) {}; SOFT B2S {}; debounce {}\n", pm(SUB, 0xC7), pm(SUB, 0xC8), pm(SUB, 0xC9)).as_bytes());
    for (i, name) in crate::acpm::TMU_ZONES.iter().enumerate() {
        let tf = crate::acpm::tmu_target_freq(i as u32).map(|k| format!("A#{k}")).unwrap_or_else(|e| format!("error G#{e:X}"));
        out.extend_from_slice(format!("{name:>6}: firmware target {tf}; trips").as_bytes());
        for trip in 0..8 {
            match crate::acpm::tmu_trip_count(i as u32, trip) {
                Ok(n) => out.extend_from_slice(format!(" A#{n}").as_bytes()),
                Err(e) => out.extend_from_slice(format!(" G#{e:X}").as_bytes()),
            }
        }
        out.push(b'\n');
    }
}

/// DIAG's heat lines.
pub fn diag(out: &mut alloc::vec::Vec<u8>) {
    use crate::platform::append_hex;
    append_hex(out, b"SOC_C=", hottest().map_or(0, |h| h.1 as u32));
    append_hex(out, b"SOC_ZONE=", hottest().map_or(u32::MAX, |h| h.0 as u32));
    append_hex(out, b"THROTTLED_MHZ=", x3_capped_from() / 1000);
    append_hex(out, b"CORE_MHZ=", CORE_MHZ.load(Relaxed));
    append_hex(out, b"X3_HELD_MHZ=", X3_HELD_MHZ.load(Relaxed));
    append_hex(out, b"X3_SLOW_S=", SLOW_S.load(Relaxed) as u32);
    append_hex(out, b"X3_FW_CAP_S=", FW_CAP_S.load(Relaxed) as u32);
    append_hex(out, b"CLKDIV_CL2_SEEN=", DIV_SEEN[2].load(Relaxed));
}
