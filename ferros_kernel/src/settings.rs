//! The device's settings (SETTINGS.md) as one spec VSF document in the vault, under the key `device`: a section `device` with a typed field per setting, stamped with its Eagle time and a provenance hash. It replaced the raw one-byte `device.*` keys on 2026-10-27 (owner: "All device settings should be proper spec VSF for aid in vault storage").
//!
//! The kernel loads the document right after the vault opens and puts each value into every process's preferences block; a change (SYS_SET_PREF from a process holding Settings, `diag pref`, or a `device` document stored from the bridge) writes the whole document again. A field that is absent, or out of range, keeps its default. The language joins as its own VSF type once vsf has one (asked of the VSF session, 2026-10-27); it will follow the last attested identity, as every setting here does (SETTINGS.md).
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

use crate::process::{PREF_BASE, PREF_BRIGHTNESS, PREF_DITHER, PREF_LABELS, PREF_LAYOUT, PREF_PROX_WAKE, PREF_SCOPE, PREF_THEME};

/// The vault key and the section name.
pub const KEY: &str = "device";
const SECTION: &str = "device";

/// Each kept preference: its field name and its index in the preferences block. The screen's on/off (preference 2) is not kept: it is the power key's, every boot starts lit.
pub const FIELDS: [(&str, usize); 8] = [
    ("base", PREF_BASE),
    ("labels", PREF_LABELS),
    ("brightness", PREF_BRIGHTNESS),
    ("dither", PREF_DITHER),
    ("theme", PREF_THEME),
    ("prox_wake", PREF_PROX_WAKE),
    ("scope", PREF_SCOPE),
    ("layout", PREF_LAYOUT),
];
/// The time zone: seconds east of UTC.
const TZ: &str = "tz";

fn schema() -> SectionSchema {
    let mut s = SectionSchema::new(SECTION);
    for (name, _) in FIELDS {
        s = s.field(name, TypeConstraint::AnyUnsigned);
    }
    s.field(TZ, TypeConstraint::AnySigned)
}

/// Eagle time now, in oscillations, from the RTC's base (0 until the RTC has been read: the document then says it was made at the epoch).
fn eagle_now() -> i64 {
    use core::sync::atomic::Ordering;
    const EPOCH_UNIX: i128 = -14_182_940;
    let base = crate::rtc::BASE.load(Ordering::Relaxed);
    if base == 0 {
        return 0;
    }
    let hz = crate::platform::cntfrq() as i128;
    let t = (base as i128) + crate::platform::cntpct() as i128;
    let ops = vsf::OSCILLATIONS_PER_SECOND as i128;
    let (secs, ticks) = (t / hz, t % hz);
    ((secs - EPOCH_UNIX) * ops + ticks * ops / hz) as i64
}

/// The document for the settings as they are now, with preference `index` at `value` (the change being made; None for none).
pub fn document(change: Option<(usize, u8)>) -> Option<Vec<u8>> {
    let mut b = schema().build();
    for (name, index) in FIELDS {
        let v = match change {
            Some((i, v)) if i == index => v,
            _ => crate::process::pref(index),
        };
        b = b.set(name, VsfType::u(v as usize, false)).ok()?;
    }
    let tz = crate::rtc::TZ.load(core::sync::atomic::Ordering::Relaxed);
    b = b.set(TZ, VsfType::i(tz as isize)).ok()?;
    let section = b.encode().ok()?;
    vsf::VsfBuilder::new().creation_time_oscillations(eagle_now()).provenance_only().add_unboxed(SECTION, section).build().ok()
}

/// Read a document and put every valid value into the preferences block (`held`: the kernel lock is already held). Returns how many settings it carried, or None if it is not a settings document.
pub fn apply(doc: &[u8], held: bool) -> Option<usize> {
    let section = SectionBuilder::parse_document(schema(), doc, None).ok()?;
    let first = |name: &str| section.get_fields(name).first().and_then(|f| f.values.first()).cloned();
    let mut n = 0;
    for (name, index) in FIELDS {
        let Some(v) = first(name).and_then(|v| v.as_u64()) else { continue };
        if v <= u8::MAX as u64 && crate::process::pref_ok(index, v as u8) {
            if held {
                crate::process::set_pref_held(index, v as u8);
            } else {
                crate::process::set_pref(index, v as u8);
            }
            n += 1;
        }
    }
    if let Some(z) = first(TZ).and_then(|v| v.as_i64()) {
        // A zone is within a day of UTC; anything else is not a zone.
        if z.abs() < 86_400 {
            if held {
                crate::rtc::set_tz_held(z);
            } else {
                crate::rtc::set_tz(z);
            }
            n += 1;
        }
    }
    Some(n)
}
