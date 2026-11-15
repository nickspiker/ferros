//! The `camera` power switch (docs/CAMERA.md "The driver"; owner, 2026-11-14: "Can we get Lumis wired up?"): what the camera driver process (payloads/camera) may not do itself, each step code camera.rs has and has run, behind one switch in the pattern every move followed (KERNEL-OUT.md).
//!
//! On:
//! - both camera buses' pins, clock sources and controllers (hsi2c_14, the camera PMIC's, which the torch shares; hsi2c_1, the main sensor's, whose transfers are then the process's thru its `hsi2c1` grant);
//! - one camera module's power, the one SYS_POWER's argument names (`select`; 0 the main, 1 the ultrawide, 2 the front, 3 the tele, ferros_cam cams): the camera PMIC's supplies over ACPM channel 2, every rail's voltage programmed and read back behind the overvoltage guard while all are off, then the module's device-tree sequence and its sensor's reset released (the main's as the harness's `diag cam up` ran it, camera.rs `power_up`; the others' camera.rs `power_module`), then its sensor's bus (the ultrawide's hsi2c_3, the front's hsi2c_2, the tele's hsi2c_4, each the process's thru its grant);
//! - the ISP front end's power domain thru EL3 (the boot gated it, pd.rs; pd.rs `on` writes its configuration and makes the secure world's restore, the prepare call, the pair of the boot's save), never a PMU read from here;
//! - the front end's clocks (camera.rs `fe_clocks`: VDD_CAM, the ISPFE_NOC clock, CMU_ISPFE, SYSREG_ISPFE);
//! - the combo D-PHY's isolation bypass (PMU G#3EBC bit 0), once per boot;
//! - the front end's SYSMMUs in bypass and its core clock on;
//! - its three S2MPUs fenced to the frames the driver holds (s2mpu.rs `fence_cam`), nothing at all while it holds none.
//!
//! Off: the held camera's sensor stream stopped if it still runs (a holder that ended without closing), the PDMA engine stopped, that camera's receiver stopped and Linux's close replayed (camera_close.rs) when a job was given, the S2MPUs shut to everything, then the module powered down in its own order. The front end's domain stays on: switching it off and on again is not yet trusted (pd.rs), so a boot makes its pair of calls once.
//! A process that ends with the switch on has it turned off by the service loop's next pass (process.rs `power_offs_apply`); its frames went back to the pool already, behind a shut fence (process.rs `cam_free`).
//!
//! hsi2c_14 has one owner at a time: the torch switch refuses while this one is on, and this one while the torch's is (torch.rs).

use crate::camera::{PMIC_BUS, SENSOR_ADDR, SENSOR_BUS};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

/// The switch is on: the buses, the module and the front end are the camera driver's.
static HELD: AtomicBool = AtomicBool::new(false);
/// The module the next switch on powers (`select`, from SYS_POWER's argument), and the one the switch holds while on: the camera's id (ferros_cam cams: 0 the main, 1 the ultrawide, 2 the front, 3 the tele).
static NEXT: AtomicU8 = AtomicU8::new(0);
static MODULE: AtomicU8 = AtomicU8::new(0);
/// The front end's domain was brought on by this switch this boot (and is never gated again: the boot's gating skips it, pd.rs `gate_at_boot`).
static FE_ON: AtomicBool = AtomicBool::new(false);
/// The frames the camera driver holds: one run, its physical start and length in bytes (0 while none), set by process.rs `cam_ask` and `cam_free` under the kernel lock.
pub static REGION: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// The PMU's atomic set alias (exynos-pmu-if.c `exynos_pmu_set_bit_atomic`: offset | G#C000, the bit's index as the value), thru EL3, as Linux sets a bit of a PMU register below G#4000 without reading it.
const PMU: u64 = 0x1546_0000;
const PMU_SET_ALIAS: u64 = 0xC000;
/// The combo D-PHY's isolation register (phy-exynos-mipi `__set_phy_isolation`): bit 0 set bypasses the isolation, so the PHY's lanes take their settings (docs/oracle/CAMERA.md "The receive side, cold").
const PHY_ISO: u64 = 0x3EBC;
/// The PDMA engine (ferros_cam).
const PDMA: usize = ferros_cam::PDMA;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn spin_us(us: u64) {
    let t = crate::platform::cntpct() + crate::platform::cntfrq() / 1_000_000 * us;
    while crate::platform::cntpct() < t {}
}
fn ms_since(t0: u64) -> u64 {
    crate::platform::cntpct().wrapping_sub(t0) * 1000 / crate::platform::cntfrq()
}

/// Whether the switch is on (the torch's switch refuses while it is).
pub fn held() -> bool {
    HELD.load(Ordering::Acquire)
}

/// Whether the front end's domain is on by this switch's doing (pd.rs leaves it on; the fence and its report may touch the S2MPUs inside it).
pub fn fe_on() -> bool {
    FE_ON.load(Ordering::Acquire)
}

/// The `camera` power switch (process.rs POWERS).
pub fn power(on: bool) -> bool {
    if on { up() } else { down() }
}

/// Which module the next switch on powers: SYS_POWER's argument bits 8-15 for this switch (ferros_sys `power_arg`; 0, the main, for a plain `power`), set just before `power` under the kernel lock.
pub fn select(module: u8) {
    NEXT.store(module, Ordering::Release);
}

/// The module the switch holds (meaningful while `held`).
pub fn module() -> u8 {
    MODULE.load(Ordering::Acquire)
}

/// The last lines of a report into the flash log, so a refusal says why.
fn log_tail(what: &str, rep: &[u8]) {
    let text = core::str::from_utf8(rep).unwrap_or("");
    let lines: alloc::vec::Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    // WHY saturating: a report of fewer than four lines would underflow the start index and panic the kernel on its core; it is the whole report then.
    for l in &lines[lines.len().saturating_sub(4)..] {
        crate::flashlog::line(&alloc::format!("camera {what}: {l}"));
    }
}

fn up() -> bool {
    let want = NEXT.load(Ordering::Acquire);
    if held() {
        // One module at a time: they share the camera PMIC's rails and the front end.
        if want != module() {
            crate::flashlog::line(&alloc::format!("camera: refused module A#{want}, the switch holds A#{}", module()));
            return false;
        }
        return true;
    }
    // WHY the check: the module is the caller's byte (SYS_POWER's argument); one with no sequence is refused before anything is powered.
    let other = match want {
        0 => None,
        k => match crate::camera::module(k) {
            Some(m) => Some(m),
            None => {
                crate::flashlog::line(&alloc::format!("camera: refused, no module A#{want}"));
                return false;
            }
        },
    };
    if crate::torch::held() {
        crate::flashlog::line("camera: refused, the torch holds hsi2c_14 (the camera PMIC's bus)");
        return false;
    }
    let t0 = crate::platform::cntpct();
    let mut rep = alloc::vec::Vec::new();
    crate::flashlog::line("camera: on, the buses");
    crate::camera::buses_up(&mut rep);
    // Only the PMIC's bus can be ready now: the sensor's lines are held low until the module's rails are on, so its master reads busy (the Pro, 2026-11-14: gpp2 DAT G#0, "USI1 (sensor): ready false"), and the harness's `diag cam up` brought it up again after the power, as below.
    if !crate::hsi2c::ready(PMIC_BUS.base, PMIC_BUS.usi_mode) {
        log_tail("buses", &rep);
        return false;
    }
    match other {
        None => {
            crate::flashlog::line("camera: on, the main module");
            if !crate::camera::power_up(&mut rep) {
                log_tail("module", &rep);
                crate::camera::power_down(&mut rep);
                return false;
            }
            // The sensor's bus again now its pads are powered (as the harness found it, 2026-10-19), and only then asked whether it is ready.
            crate::hsi2c::bring_up(SENSOR_BUS.base, SENSOR_BUS.usi_mode, crate::camera::FS_DIV, 0x2_0000);
            if !crate::hsi2c::ready(SENSOR_BUS.base, SENSOR_BUS.usi_mode) {
                let b = SENSOR_BUS.base;
                crate::flashlog::line(&alloc::format!("camera: the sensor's bus not ready with the module powered (TRANS_STATUS G#{:X}, gpp2 DAT G#{:X})", rd(b + 0x50), rd(0x1084_0044)));
                crate::camera::power_down(&mut rep);
                return false;
            }
        }
        Some(m) => {
            crate::flashlog::line(&alloc::format!("camera: on, the {} module", m.name));
            crate::camera::module_bus_up(m);
            let ok = crate::camera::power_module(m, &mut rep);
            // Every step of the sequence into the flash log, the sub-PMIC GPIO readings among them: the front camera's and the tele's power-up is a first try (docs/CAMERA.md "The other cameras").
            for l in core::str::from_utf8(&rep).unwrap_or("").lines().filter(|l| l.contains(m.name) || l.contains("FAILED") || l.contains("refusing")) {
                crate::flashlog::line(&alloc::format!("camera: {l}"));
            }
            if !ok {
                return false;
            }
            crate::camera::module_bus_up(m);
            if !crate::hsi2c::ready(m.bus, m.usi_mode) {
                crate::flashlog::line(&alloc::format!("camera: the {} sensor's bus not ready with the module powered (TRANS_STATUS G#{:X}, pins DAT G#{:X})", m.name, rd(m.bus + 0x50), rd(0x1084_0004 + m.bus_bank)));
                crate::camera::power_down_module(m, &mut rep);
                return false;
            }
        }
    }
    // The front end's domain thru EL3 only: CONFIGURATION, then the secure world's restore (pd.rs `on`), the pair of the save the boot's gating made. Never gated (a switch on before the gating at 16 s), the prepare call alone, once a boot, as the harness's `fe_up` made it on October's cold boots.
    // The module that was powered, down again (a refusal past this point).
    let module_down = |rep: &mut alloc::vec::Vec<u8>| match other {
        None => crate::camera::power_down(rep),
        Some(m) => crate::camera::power_down_module(m, rep),
    };
    crate::flashlog::line("camera: on, the front end's domain");
    if !crate::pd::is_on("ispfe") {
        crate::pd::on("ispfe", &mut rep);
        if !crate::pd::is_on("ispfe") {
            log_tail("domain", &rep);
            module_down(&mut rep);
            return false;
        }
    } else if let Some(r) = crate::camera::prepare_once() {
        crate::flashlog::line(&alloc::format!("camera: the domain was never gated; prepare call made, G#{r:X}"));
    }
    // CONFIGURATION is only the request: the domain's STATUS (bit 0, a plain read, as pd.rs `off` reads it before its own EL3 call every boot, and the harness's `fe_up` before its prepare call in October) says it is up, before any of its clocks is touched (a CMU access in a domain still down poisons EL3). Bounded at 2^13 µs: `fe_up` gave it 10 ms.
    let until = crate::platform::cntpct() + crate::platform::cntfrq() / 1_000_000 * (1 << 13);
    while !crate::camera::fe_powered() && crate::platform::cntpct() < until {}
    if !crate::camera::fe_powered() {
        crate::flashlog::line("camera: the front end's domain was asked on and its status never said so; nothing of it touched");
        module_down(&mut rep);
        return false;
    }
    FE_ON.store(true, Ordering::Release);
    crate::flashlog::line("camera: on, the front end's clocks");
    crate::camera::fe_clocks(&mut rep);
    phy_isolation_bypass();
    crate::camera::sysmmu_bypass(&mut rep);
    // The engine's ring base cleared while it is idle: `down` reads it to know whether a job was ever given.
    wr(PDMA + ferros_cam::PDMA_BASE_LO, 0);
    let fenced = fence_now();
    MODULE.store(want, Ordering::Release);
    HELD.store(true, Ordering::Release);
    crate::flashlog::line(&alloc::format!("camera: on in A#{} ms, module A#{want}, S2MPUs G#{fenced:X} (1 fenced)", ms_since(t0)));
    true
}

/// The D-PHY's isolation bypass, once per boot (the bit stays set until reset; repeated in a second `cam pdma` its EL3 write never returned, 2026-10-21). Set thru the PMU's atomic alias, as Linux sets it, with no read of the PMU: camera.rs `phy_isolation_bypass` read the register directly first, the pattern that left the next EL3 call hanging (its viewfinder's `display::adopt`, 2026-10-21).
fn phy_isolation_bypass() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::Relaxed) {
        crate::flashlog::line("camera: the D-PHY's isolation bypass thru EL3");
        let r = crate::platform::priv_reg(PMU + (PHY_ISO | PMU_SET_ALIAS), true, 0);
        crate::flashlog::line(&alloc::format!("camera: isolation bypass returned G#{r:X}"));
    }
}

/// The front end's S2MPUs to the frames the driver holds now (`REGION`), or to nothing. Only while the domain is on (an access with it down poisons EL3).
fn fence_now() -> u32 {
    let (pa, len) = (REGION[0].load(Ordering::Acquire) as usize, REGION[1].load(Ordering::Acquire) as usize);
    crate::s2mpu::fence_cam(pa, len)
}

/// The driver's frames changed (taken or given back, process.rs, under the kernel lock): the fence follows them while the domain is on. A region taken before the switch is on is fenced when it comes on.
pub fn region_changed() -> u32 {
    if !fe_on() {
        return 0;
    }
    fence_now()
}

fn down() -> bool {
    let t0 = crate::platform::cntpct();
    let was = HELD.swap(false, Ordering::AcqRel);
    let m = module();
    // The held camera's sensor and receiver (ferros_cam cams): `MODULE` is only ever a module `up` accepted (0 to 3), or the main's 0 before any.
    let cam = ferros_cam::cams::CAMS[m as usize];
    let (bus, addr) = if m == 0 { (SENSOR_BUS.base, SENSOR_ADDR) } else { (cam.bus, cam.addr) };
    let mut rep = alloc::vec::Vec::new();
    let mut steps = alloc::string::String::new();
    if fe_on() {
        // A holder that ended without closing leaves the sensor streaming: mode_select (G#0100) bit 0. Stopped with its own write (16-bit values on the Samsung parts, 8-bit on the ultrawide's Sony), then a frame's time (67.8 ms measured on the main; 2^16 µs) before the engine, as `cam pdma` waited 50 ms.
        let mut s = [0u8; 1];
        if crate::hsi2c::write_read(bus, addr, &[0x01, 0x00], &mut s).is_ok() && s[0] & 1 == 1 {
            let _ = crate::hsi2c::write_bytes(bus, addr, cam.stream_off);
            spin_us(1 << 16);
            steps.push_str(" stream stopped;");
        }
        if rd(PDMA + ferros_cam::PDMA_BASE_LO) != 0 {
            // The engine stopped as `cam pdma` stops it after a run, the held camera's receiver stopped, then Linux's close (camera_close.rs, 223 writes, captured after the main camera; the harness replayed it after its ultrawide runs too): without it the next open's front end takes no frames (2026-10-21).
            wr(PDMA + ferros_cam::PDMA_CTRL, 0);
            wr(PDMA + 0x2C, 0);
            wr(PDMA + 0x40, 0);
            for &(a, v) in cam.rx_stop.iter() {
                wr(a, v);
            }
            for &(a, v) in crate::camera_close::CLOSE.iter() {
                wr(a, v);
            }
            wr(PDMA + ferros_cam::PDMA_BASE_LO, 0);
            steps.push_str(" engine stopped, close replayed;");
        }
        let f = crate::s2mpu::fence_cam(0, 0);
        steps.push_str(&alloc::format!(" S2MPUs shut G#{f:X};"));
    }
    match crate::camera::module(m) {
        Some(md) if was => crate::camera::power_down_module(md, &mut rep),
        _ => crate::camera::power_down(&mut rep),
    }
    crate::flashlog::line(&alloc::format!("camera: off{} module A#{m} down in A#{} ms{}", steps, ms_since(t0), if was { "" } else { " (it was not on)" }));
    true
}
