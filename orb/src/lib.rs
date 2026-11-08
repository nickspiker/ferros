//! The console's radio circles, one maths for the desktop tool (tools/orb-tune) and the device (payloads/display's console).
//! A circle is the vessel (Lumis's camera-button profile made radial: three channel rings and a dark body) and, while its radio is live, the orb inside it: a Julia set read as the link's I/Q plane (look.rs), or for GPS the sky dome (sky.rs).
//! Every layer is linear VSF RGB light encoded to `g2t` codes (tanh √L) at 16 bits, a fluor `G2t16`, composed front to back with fluor's `under` and nothing else.
//! No I/O and no std: libm for the transcendentals, alloc for the satellites and sparks.

#![no_std]

extern crate alloc;

pub mod look;
pub mod sky;

pub use look::{C, GPS, Look, Prep, RADIOS, Radio, Vessel};

/// The float maths the circles use, by libm: core's f32 and f64 have no transcendentals without std.
/// Named as std's methods, so the maths reads the same in both builds.
/// Where std is in the build (tools/orb-tune, whose fluor brings it) its inherent methods outrank these and the trait goes unused: the desktop keeps std's maths, the device has libm's.
#[allow(dead_code)]
pub(crate) trait Fm: Sized {
    fn exp(self) -> Self;
    fn exp2(self) -> Self;
    fn ln(self) -> Self;
    fn log2(self) -> Self;
    fn sqrt(self) -> Self;
    fn sin(self) -> Self;
    fn cos(self) -> Self;
    fn sin_cos(self) -> (Self, Self);
    fn tanh(self) -> Self;
    fn atan2(self, x: Self) -> Self;
    fn fract(self) -> Self;
    fn ceil(self) -> Self;
}

impl Fm for f32 {
    fn exp(self) -> f32 {
        libm::expf(self)
    }
    fn exp2(self) -> f32 {
        libm::exp2f(self)
    }
    fn ln(self) -> f32 {
        libm::logf(self)
    }
    fn log2(self) -> f32 {
        libm::log2f(self)
    }
    fn sqrt(self) -> f32 {
        libm::sqrtf(self)
    }
    fn sin(self) -> f32 {
        libm::sinf(self)
    }
    fn cos(self) -> f32 {
        libm::cosf(self)
    }
    fn sin_cos(self) -> (f32, f32) {
        libm::sincosf(self)
    }
    fn tanh(self) -> f32 {
        libm::tanhf(self)
    }
    fn atan2(self, x: f32) -> f32 {
        libm::atan2f(self, x)
    }
    fn fract(self) -> f32 {
        self - libm::truncf(self)
    }
    fn ceil(self) -> f32 {
        libm::ceilf(self)
    }
}

impl Fm for f64 {
    fn exp(self) -> f64 {
        libm::exp(self)
    }
    fn exp2(self) -> f64 {
        libm::exp2(self)
    }
    fn ln(self) -> f64 {
        libm::log(self)
    }
    fn log2(self) -> f64 {
        libm::log2(self)
    }
    fn sqrt(self) -> f64 {
        libm::sqrt(self)
    }
    fn sin(self) -> f64 {
        libm::sin(self)
    }
    fn cos(self) -> f64 {
        libm::cos(self)
    }
    fn sin_cos(self) -> (f64, f64) {
        libm::sincos(self)
    }
    fn tanh(self) -> f64 {
        libm::tanh(self)
    }
    fn atan2(self, x: f64) -> f64 {
        libm::atan2(self, x)
    }
    fn fract(self) -> f64 {
        self - libm::trunc(self)
    }
    fn ceil(self) -> f64 {
        libm::ceil(self)
    }
}
