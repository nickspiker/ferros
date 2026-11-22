//! The screenshot's degauss (owner, 2026-11-22: "a degauss effect, first frame, values *2, then /1.5 then *1.25 then /1.125, etc."): the sign that the helm's two-finger slide took a screenshot.
//! Shown frame k after the capture shows the captured frame with every pixel's light times 1 + 2^-k on even k and over it on odd k, each relative to the true image (never cumulative): ×2, ÷1.5, ×1.25, ÷1.125, ×1.0625, ÷1.03125 …
//! The light is the working value's (VSF RGB, gamma 2, so a light factor f is √f on the value), thru the bake's own roll-off: light past white goes into the panel's headroom as every pixel's does, tanh in root space, never a clamp.
//! It ends at the first step whose largest move of any baked code is under one 8-bit code step (`end`); that frame and every one after is the true image.
//! Pure maths, no NEON: tools/degauss tests it on the host. main.rs `degauss_step` runs it, one step per shown frame.

/// The light factor at step k: 1 + 2^-k, multiplied on even k, divided on odd k.
pub fn factor(k: u32) -> f32 {
    let f = 1. + 1. / (1u64 << k) as f32;
    if k & 1 == 0 { f } else { 1. / f }
}

/// The scale on a working value (gamma 2) that makes its light `factor(k)`: the root of the factor.
pub fn scale(k: u32) -> f32 {
    libm::sqrtf(factor(k))
}

/// The top of the working field: 4.0 (65535 in Q14, a step under it).
const FIELD_TOP: u32 = 1 << 2;
/// Samples of the working field a unit (`largest_move`): 1/256 of white apart.
const SAMPLES_PER_UNIT: u32 = 1 << 8;
/// One 8-bit panel code in the bake's Q16 (`b >> 8` the code the link takes).
pub const CODE_STEP: f32 = (1 << 8) as f32;
/// Steps past this one are not looked at: by then a step moves nothing on any panel (2^-24 of a light).
const STEPS_MAX: u32 = 1 << 5;

/// A grey working value `v` (white 1) baked, as bake.rs does it with the E gain at 1 (the largest a channel has): the roll-off tanh²(kb·v) as a share of the peak, the panel's 1/γ, as a code of `top` (bake.rs `TOP`, Q16).
pub fn code(kb: f32, gamma: f32, top: f32, v: f32) -> f32 {
    let t = libm::tanhf(kb * v);
    top * libm::powf(t * t, 1. / gamma)
}

/// The largest move of a baked code at step k, over the whole working field (0 to 4.0, every 256th of white): |code(v · scale) − code(v)| at its most, in Q16.
pub fn largest_move(kb: f32, gamma: f32, top: f32, k: u32) -> f32 {
    let s = scale(k);
    let mut worst = 0f32;
    for i in 0..=FIELD_TOP * SAMPLES_PER_UNIT {
        let v = i as f32 * (1. / SAMPLES_PER_UNIT as f32);
        let d = (code(kb, gamma, top, v * s) - code(kb, gamma, top, v)).abs();
        if d > worst {
            worst = d;
        }
    }
    worst
}

/// The step the effect ends at: the first whose largest move is under one 8-bit code step (`CODE_STEP`), for the bake's roll-off gain `kb` (bake.rs `Bake::k`) and the panel's γ. Frames 0 to end − 1 carry the factors; frame `end` is the true image.
pub fn end(kb: f32, gamma: f32, top: f32) -> u32 {
    (0..STEPS_MAX).find(|&k| largest_move(kb, gamma, top, k) < CODE_STEP).unwrap_or(STEPS_MAX)
}
