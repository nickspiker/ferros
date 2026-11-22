//! The screenshot's degauss on the host: payloads/display/src/degauss.rs itself, and the light a step gets from the display's bake (neon.rs `light16`'s Lambert tanh with `Bake::gained`'s k), tested.
//! `cargo test --manifest-path tools/degauss/Cargo.toml -- --nocapture` (the table of each step's light at white is printed).

#[path = "../../../payloads/display/src/degauss.rs"]
pub mod degauss;

/// bake.rs `TOP`: the baked code at the panel's peak.
#[cfg(test)]
const TOP: f32 = 65472.;
/// The two panels' γ (odt.rs `panel_gamma`): husky's HK3, akita's AK3B.
#[cfg(test)]
const GAMMAS: [f32; 2] = [2.19, 2.23];

/// The roll-off's gain on a working value for white at `wp` of the E peak (bake.rs `set`): atanh(√(white / peak)).
#[cfg(test)]
fn kb(wp: f32) -> f32 {
    libm::atanhf(libm::sqrtf(wp))
}

/// Every white the brightness reaches (main.rs `brightness`): half the peak at the top, down to a sixteenth (the panel's floor at an eighth of its peak, white a sixteenth of the panel's 16x headroom), each 2^-1/4 of a stop.
#[cfg(test)]
fn whites() -> impl Iterator<Item = f32> {
    (0..=12).map(|i| 0.5 * libm::exp2f(-(i as f32) * 0.25))
}

/// neon.rs `light16`'s maths, in f64 so the test sees the maths and not f32's rounding (the device runs it in f16, `light16h`, which its start-up `light_check` holds to the f32 form): tanh(x) by Lambert's continued fraction to 7th order, x = k·s·v (the gained k lane, `Bake::gained`), squared: the light as a share of the E peak.
#[cfg(test)]
fn light_lambert(k: f32, s: f32, v: f32) -> f32 {
    let x = k as f64 * s as f64 * v as f64;
    let y = x * x;
    let num = 135135. + y * (17325. + y * (378. + y));
    let den = 135135. + y * (62370. + y * (3150. + y * 28.));
    let t = x * num / den;
    (t * t) as f32
}

#[cfg(test)]
mod tests {
    use super::degauss::*;
    use super::*;

    #[test]
    fn the_owners_sequence() {
        // "first frame, values *2, then /1.5 then *1.25 then /1.125, etc.": each relative to the true image.
        let want = [2., 1. / 1.5, 1.25, 1. / 1.125, 1.0625, 1. / 1.03125, 1.015625, 1. / 1.0078125];
        for (k, &w) in want.iter().enumerate() {
            let f = factor(k as u32);
            assert!((f - w).abs() <= w * f32::EPSILON, "step {k}: {f} against {w}");
        }
        // Alternating about 1, each step's distance from 1 (as a light factor, 1 + 2^-k) half the one before.
        for k in 0..20u32 {
            let f = factor(k);
            let up = if k & 1 == 0 { f } else { 1. / f };
            assert!((up - 1. - libm::exp2f(-(k as f32))).abs() <= f32::EPSILON * 4., "step {k}");
            assert_eq!(f > 1., k & 1 == 0, "step {k} on the wrong side of 1");
        }
        // The working value's scale is the light factor's root (gamma 2).
        for k in 0..20u32 {
            let s = scale(k);
            assert!((s * s - factor(k)).abs() <= factor(k) * f32::EPSILON * 4.);
        }
    }

    #[test]
    fn ends_under_one_code_step() {
        for g in GAMMAS {
            for wp in whites() {
                let k = kb(wp);
                let e = end(k, g, TOP);
                // Every step before the end moves some code by a whole 8-bit step or more; the end's moves none.
                for j in 0..e {
                    assert!(largest_move(k, g, TOP, j) >= CODE_STEP, "γ {g} white {wp}: step {j} moves under a code");
                }
                assert!(largest_move(k, g, TOP, e) < CODE_STEP);
                // Six steps on both panels at every brightness: ×2, ÷1.5, ×1.25, ÷1.125, ×1.0625, ÷1.03125, then the true image.
                assert_eq!(e, 6, "γ {g} white {wp}");
            }
        }
        // The moves, in 8-bit codes, at the Pro's brightest.
        let k = kb(0.5);
        let moves: Vec<String> = (0..8).map(|j| format!("{:.2}", largest_move(k, GAMMAS[0], TOP, j) / CODE_STEP)).collect();
        println!("largest code move per step (8-bit codes), husky, white half the peak: {}", moves.join(", "));
    }

    #[test]
    fn the_gained_light() {
        for wp in whites() {
            let k = kb(wp);
            let e = end(k, GAMMAS[0], TOP);
            for j in 0..e {
                let (s, f) = (scale(j), factor(j));
                let mut last = -1f32;
                // Every 16-bit working value (white 2^14, the field to 4.0).
                for q in 0..=u16::MAX as u32 {
                    let v = q as f32 * (1. / (1 << 14) as f32);
                    let l = light_lambert(k, s, v);
                    let t = libm::tanhf(k * s * v);
                    // Lambert's form against tanh where the gain takes it (x to 5 at the brightest, past the 3.5 the field reached before): within 2^-12 of the peak's light.
                    assert!((l - t * t).abs() < 1. / (1 << 12) as f32, "white {wp} step {j} v {v}: {l} against {}", t * t);
                    // No clamp anywhere: the light only approaches the peak (its code within the u16 field before any saturating shift), and the order of values is kept.
                    assert!(l < 1. + 1. / (1 << 15) as f32, "white {wp} step {j} v {v}: {l} past the peak");
                    assert!(TOP * libm::powf(l, 1. / GAMMAS[0]) < u16::MAX as f32);
                    assert!(l >= last, "white {wp} step {j}: not monotonic at v {v}");
                    last = l;
                }
                // In the dark the roll-off is linear: the light is the factor's, within 2^-8.
                let v = 1. / (1 << 6) as f32;
                let ratio = light_lambert(k, s, v) / light_lambert(k, 1., v);
                assert!((ratio / f - 1.).abs() < 1. / (1 << 8) as f32, "white {wp} step {j}: dark ratio {ratio} against {f}");
                // At white and above the roll-off bends it toward 1: a brightening step brightens less than its factor (into the headroom, never past the peak), a dimming one dims less.
                for v in [1f32, 2., 4.] {
                    let ratio = light_lambert(k, s, v) / light_lambert(k, 1., v);
                    if j & 1 == 0 {
                        assert!(ratio <= f && ratio >= 1., "white {wp} step {j} v {v}: {ratio}");
                    } else {
                        assert!(ratio >= f && ratio <= 1., "white {wp} step {j} v {v}: {ratio}");
                    }
                }
            }
        }
        // The light at white each step, for the report: the Pro's brightest (white half the peak) and the HK3's typical (203 of 1178 cd/m²).
        for wp in [0.5f32, 203. / 1178.] {
            let k = kb(wp);
            let row: Vec<String> = (0..6).map(|j| format!("{:.3}", light_lambert(k, scale(j), 1.) / light_lambert(k, 1., 1.))).collect();
            println!("white at {wp:.3} of the peak: its light per step {}", row.join(", "));
        }
    }
}
