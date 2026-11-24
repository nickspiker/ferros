//! The battery's charge voltage cap (owner, 2026-11-24; docs/CHARGING.md "Decided: the charge voltage cap"): a slider on the battery page from the charger's 4.00 V floor to the battery's own ceiling, 4.20 V while nothing is kept.
//! The kernel drives the charger (ferros_sys::control `BATTERY`, slot `CHARGE_CV`, mV): it checks the cap, keeps it in the device's settings document and writes the charger's CV at once.
//! This device's battery, so a device setting: never the identity's.

use ferros_sys::control::{BATTERY, CHARGE_CV, CHARGE_CV_CEILING, CHARGE_CV_DEFAULT, CHARGE_CV_FLOOR, CHARGE_CV_STEP};

/// The cap's steps across the slider: 4.00 to 4.48 V in 10 mV is A#48.
const STEPS: u64 = (CHARGE_CV_CEILING - CHARGE_CV_FLOOR) / CHARGE_CV_STEP;

/// How sharply the slider's ends round off, in track lengths: 2^7, so the end's last 2^-7 of the track bends and the rest runs straight.
const SHARP: f64 = (1 << 7) as f64;

/// The cap the kernel holds, mV; the default while nothing is kept, or for a kernel without the battery's block (its controls read all zero).
pub fn now_mv() -> u64 {
    let v = ferros_sys::controls(BATTERY)[CHARGE_CV as usize];
    if (CHARGE_CV_FLOOR..=CHARGE_CV_CEILING).contains(&v) {
        v
    } else {
        CHARGE_CV_DEFAULT
    }
}

/// Ask the kernel for cap `mv`; true when it took and kept it.
pub fn set(cap: u64, mv: u64) -> bool {
    ferros_sys::control(cap, BATTERY, CHARGE_CV, mv)
}

/// Where cap `mv` puts the knob, 0 to 1.
pub fn place(mv: u64) -> f64 {
    (mv - CHARGE_CV_FLOOR) as f64 / (CHARGE_CV_CEILING - CHARGE_CV_FLOOR) as f64
}

/// The knob's place from the finger's place `u` along the track (0 at its left end, 1 at its right; past either when the finger runs off).
/// The owner's C¹ rule for this slider: no clamp, no kink.
/// softplus(k u) - softplus(k (u - 1)), over k: C∞ everywhere, within 2^-7 of u along the track, and leaning smoothly onto 0 and 1 past its ends.
/// At either end it is ln 2 / k off the bound, about a 185th: under half a step of A#48, so the ends still reach 4.00 and 4.48 V.
pub fn of_finger(u: f64) -> f64 {
    // ln(1 + e^z): the two branches are the same function, split only so exp never overflows; no kink at 0.
    let softplus = |z: f64| if z > 0. { z + libm::log1p(libm::exp(-z)) } else { libm::log1p(libm::exp(z)) };
    (softplus(SHARP * u) - softplus(SHARP * (u - 1.))) / SHARP
}

/// The cap at the knob's place `t` (0 to 1), mV: the nearest of the charger's 10 mV steps.
pub fn mv_at(t: f64) -> u64 {
    CHARGE_CV_FLOOR + CHARGE_CV_STEP * libm::round(t * STEPS as f64) as u64
}

/// The rough share of a full charge (to the battery's 4.48 V ceiling) that cap `mv` holds, in whole per cent.
/// An estimate for a 4.48 V lithium-cobalt cell, not a measurement of this one: 4.00 V about 64 %, 4.20 V about 82 %, 4.48 V all of it, on a smooth rising curve (docs/CHARGING.md says where it comes from and what will replace it: the gauge's own reading after a charge to each cap).
pub fn share_pct(mv: u64) -> u64 {
    let x = (mv - CHARGE_CV_FLOOR) as f64 / 1000.;
    libm::round(64. + 100.71 * x - 53.57 * x * x) as u64
}

/// Above the default: the share of the default's charge cycles cap `mv` leaves, in 2^10ths (`CYCLES_WHOLE`), by Battery University's rule (BU-808, Table 4: each 0.1 V less doubles a cell's cycles, measured on cells built for 4.20 V).
/// 2^(-(mv - 4200) / 100): C∞ in the cap.
pub fn cycles_share(mv: u64) -> u64 {
    let over = (mv - CHARGE_CV_DEFAULT) as f64 / 100.;
    libm::round(CYCLES_WHOLE as f64 * libm::exp2(-over)) as u64
}

/// The whole `cycles_share` is a share of.
pub const CYCLES_WHOLE: u64 = 1 << 10;
