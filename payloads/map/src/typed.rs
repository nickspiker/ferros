//! The typed settings kit (docs/QUICK.md "Quick pages"; owner, 2026-11-11, of the torch and brightness: "trying to figure out a good way to unify all those settings", and to the proposal "Please."). A control declares its settings as typed values (a level, a share, steps of an octave with an off at the left end), each kept in a preference, and its page is drawn from that list with the settings pages' one kit (COMPOSITOR.md "Controls": a slider for each, its name and its value in words above it, a checkbox for an on and off). A quick circle's page is a settings page opened a second way (a pull down from the circle), so this is the settings pages' kit, not a second one.
//!
//! The torch is the first control built this way (`TORCH`); Appearance's brightness and scale sliders came before it and are still drawn by hand there.

use super::*;

/// What a setting is, which says how its slider's place maps to its kept byte and back, and what words its value reads as.
#[derive(Clone, Copy)]
pub enum Kind {
    /// A level 1-255 on a dimmer of its own (the torch's is perceptual, docs/TORCH.md): the slider's left end the dimmest (1), its right the brightest (255). The slider is its value; no words.
    Level,
    /// A share either side of a half: a signed byte of 256ths, `reach` each way. Its words: the share, in the person's base.
    Share { reach: u8 },
    /// Off at the slider's left end, then `steps` steps, each a power-of-two fraction of an octave of a time (`ns`, the step's length; ferros_sys::torch says which fraction). Its words: `off`, or the time in the person's base.
    Octaves { steps: u8, off: Msg, ns: fn(u8) -> Option<u64> },
}

/// A setting: the preference it is kept in (the device document's field, SETTINGS.md), its name, and what it is.
#[derive(Clone, Copy)]
pub struct Typed {
    pub pref: u64,
    pub name: Msg,
    pub kind: Kind,
}

/// Every typed setting, the index a page's slider row carries (`Choice::Typed`).
pub const ALL: [Typed; 4] = [
    // The torch (docs/TORCH.md "Settings"; owner, 2026-11-11: "brightness and strobe frequency and duty cycle and timeout"): the level a tap lights it at, the strobe (off, or a flash every 4 s to 16 a second, read as its period), the share of each flash lit, and when it turns itself off (never, or 16 s to about 4 h 33 min).
    Typed { pref: ferros_sys::PREF_TORCH_LEVEL, name: Msg::Brightness, kind: Kind::Level },
    Typed { pref: ferros_sys::PREF_TORCH_STROBE, name: Msg::Strobe, kind: Kind::Octaves { steps: ferros_sys::torch::STROBE_STEPS, off: Msg::Off, ns: ferros_sys::torch::strobe_period_ns } },
    Typed { pref: ferros_sys::PREF_TORCH_DUTY, name: Msg::LitPerFlash, kind: Kind::Share { reach: ferros_sys::torch::DUTY_REACH } },
    Typed { pref: ferros_sys::PREF_TORCH_TIMEOUT, name: Msg::TurnsOffAfter, kind: Kind::Octaves { steps: ferros_sys::torch::TIMEOUT_STEPS, off: Msg::Never, ns: ferros_sys::torch::timeout_ns } },
];
/// The torch page's settings, in `ALL`.
pub const TORCH: core::ops::Range<usize> = 0..4;
/// The torch's level, in `ALL`.
pub const TORCH_LEVEL: usize = 0;

impl Typed {
    /// The kept byte now. The torch's level reads as the level a tap lights it at (ferros_sys::torch::level: the whole dimmer while never set).
    pub fn now(&self) -> u8 {
        match self.kind {
            Kind::Level if self.pref == ferros_sys::PREF_TORCH_LEVEL => ferros_sys::torch::level(),
            _ => ferros_sys::pref(self.pref),
        }
    }

    /// Where byte `v` puts the slider's knob, 0 to 1.
    pub fn place(&self, v: u8) -> f64 {
        match self.kind {
            Kind::Level => (v as f64 - 1.) / 254.,
            Kind::Share { reach } => (v as i8 as f64 + reach as f64) / (2 * reach as u32) as f64,
            Kind::Octaves { steps, .. } => v as f64 / steps as f64,
        }
    }

    /// The byte at the knob's place `t` (0 to 1, the slider holds it there), the nearest step.
    pub fn value(&self, t: f64) -> u8 {
        match self.kind {
            Kind::Level => 1 + libm::round(t * 254.) as u8,
            Kind::Share { reach } => (libm::round(t * (2 * reach as u32) as f64) as i32 - reach as i32) as i8 as u8,
            Kind::Octaves { steps, .. } => libm::round(t * steps as f64) as u8,
        }
    }

    /// Byte `v` in the person's words and base: empty for a level (its slider says it).
    pub fn words(&self, v: u8, l: &mut Line) {
        let f = ferros_sys::numbers(Style::Glyphs);
        match self.kind {
            Kind::Level => {}
            Kind::Share { .. } => {
                let _ = tatau::share(l, f, (128 + v as i8 as i32) as u64, 256, 2);
            }
            Kind::Octaves { off, ns, .. } => match ns(v) {
                Some(ns) => {
                    let _ = tatau::duration_ns(l, f, ns, 1);
                }
                None => {
                    l.s(tr(off));
                }
            },
        }
    }
}
