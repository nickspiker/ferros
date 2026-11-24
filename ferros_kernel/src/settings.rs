//! The device's settings (SETTINGS.md) as one spec VSF document in the vault, under the key `device`: a section `device` with a typed field per setting, stamped with its Eagle time and a provenance hash. It replaced the raw one-byte `device.*` keys on 2026-10-27 (owner: "All device settings should be proper spec VSF for aid in vault storage").
//!
//! The kernel loads the document right after the vault opens and puts each value into every process's preferences block; a change (SYS_SET_PREF from a process holding Settings, `diag pref`, or a `device` document stored from the bridge) writes the whole document again. A field that is absent, or out of range, keeps its default. The language is kept as a language (vsf `wl`, its ISO 639-3 code), this device's and the identity's, with the switch that says which rules (lang.rs), as the zone is.
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

use crate::process::{PREF_BRIGHTNESS, PREF_DITHER, PREF_DND, PREF_LABELS, PREF_LAYOUT, PREF_PROX_WAKE, PREF_RADIOS_BEFORE, PREF_RADIOS_OFF, PREF_SCALE, PREF_SCOPE, PREF_SOUND, PREF_THEME, PREF_TORCH_DUTY, PREF_TORCH_LEVEL, PREF_TORCH_STROBE, PREF_TORCH_TIMEOUT, PREF_UNLOCK_AT_BOOT, PREF_VOLUME, PREF_HELM};

/// The vault key and the section name.
pub const KEY: &str = "device";
const SECTION: &str = "device";

/// Each kept preference: its field name and its index in the preferences block. The screen's on/off (preference 2) is not kept: it is the power key's, every boot starts lit. Do not disturb is the identity's as its service last wrote it (`dnd_identity`, as `language_identity`), never read back into the identity.
/// The number base is not among them: preference 0 is the base in effect, and the document keeps this device's own and the identity's (`BASE`, base.rs).
pub const FIELDS: [(&str, usize); 19] = [
    ("labels", PREF_LABELS),
    ("brightness", PREF_BRIGHTNESS),
    ("dither", PREF_DITHER),
    ("theme", PREF_THEME),
    ("prox_wake", PREF_PROX_WAKE),
    ("scope", PREF_SCOPE),
    ("layout", PREF_LAYOUT),
    ("unlock_at_boot", PREF_UNLOCK_AT_BOOT),
    ("scale", PREF_SCALE),
    ("radios_off", PREF_RADIOS_OFF),
    ("radios_before", PREF_RADIOS_BEFORE),
    ("sound", PREF_SOUND),
    ("dnd_identity", PREF_DND),
    ("torch_level", PREF_TORCH_LEVEL),
    ("torch_strobe", PREF_TORCH_STROBE),
    ("torch_duty", PREF_TORCH_DUTY),
    ("torch_timeout", PREF_TORCH_TIMEOUT),
    ("volume", PREF_VOLUME),
    ("helm", PREF_HELM),
];
/// The number base (base.rs; 2026-11-22, as the language): this device's own (`base`, tatau's code; absent, DMS), whether the device follows the identity's (`base_follow`, 1 the default), and the identity's as its service last wrote it (`base_identity`; absent, none). Until 2026-11-22 `base` was preference 0 itself, the only choice there was, so a document from before reads as this device's own, followed by default.
const BASE: &str = "base";
const BASE_FOLLOW: &str = "base_follow";
const BASE_IDENTITY: &str = "base_identity";
/// The time zone: seconds east of UTC.
const TZ: &str = "tz";
/// What this device starts into (owner, 2026-11-21: "Start into: ferros / Android"; docs/BOOT.md "Android only by the identity's choice"): 0 ferros (the default), 1 Android, the attested identity's standing choice. Kept here and in the kernel alone, never in the preferences block: until the same day it was preference 8, which the kernel still writes as slot 0's surface generation for a display from before 2026-11-12 (process.rs `PREF_SURFACES`), so a window's generation became "Android" in the document and the 8a started into Android unasked (measured: b297-b299).
const START_INTO: &str = "start_into";
/// The setting in effect (`START_INTO`): set from the document at boot, by SYS_UNLOCK op 13 (the Settings page; Android only while a person is present on a device that allows it) and by `diag "start into ..."`; read by the service loop after the attest of a boot nobody answered (service.rs `START_INTO`).
pub static START_ANDROID: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The zone (owner, 2026-11-10: "Proper zone too"; zone.rs): this device's own choice, its IANA id by hand (`zone`; none: `tz` is the offset) and whether it is automatic (`zone_auto`, 1 the default); whether the device follows the identity's (`zone_follow`, 1 the default); the identity's as its service last wrote it (`zone_identity_auto` present once it has, `zone_identity` its id); the zone last found here and what found it (`zone_found`, `zone_found_by`: 1 the network, 2 a position).
const ZONE: &str = "zone";
const ZONE_AUTO: &str = "zone_auto";
const ZONE_FOLLOW: &str = "zone_follow";
const ZONE_IDENTITY: &str = "zone_identity";
const ZONE_IDENTITY_AUTO: &str = "zone_identity_auto";
const ZONE_FOUND: &str = "zone_found";
const ZONE_FOUND_BY: &str = "zone_found_by";
/// The language (lang.rs): this device's own (`language`, a vsf `wl`: its ISO 639-3 code, no script or region; absent, none: English), whether the device follows the identity's (`language_follow`, 1 the default), and the identity's as its service last wrote it (`language_identity`; absent, none).
const LANGUAGE: &str = "language";
const LANGUAGE_FOLLOW: &str = "language_follow";
const LANGUAGE_IDENTITY: &str = "language_identity";

/// A packed code (lang.rs) as the document keeps it, or none for 0.
fn language_of(code: u16) -> Option<vsf::Language> {
    if code == 0 {
        return None;
    }
    // The letters base 26, as vsf packs them (lang.rs keeps only codes below 26³, so each is a letter).
    let l = [b'a' + (code / 676) as u8, b'a' + (code / 26 % 26) as u8, b'a' + (code % 26) as u8];
    Some(vsf::Language::new(l))
}

/// The knobs kept as device settings (docs/RADIO-CONTROLS.md): field name, link (the control blocks' order: 0 cellular, 1 WiFi, …, 7 the battery), slot. Value knobs only: actions are counters and are never kept.
/// `charge_cv` is the battery's charge voltage cap in mV (docs/CHARGING.md, owner 2026-11-24): this device's battery, so a device setting, never the identity's.
pub const KNOBS: [(&str, usize, usize); 3] = [("wifi_power_save", 1, 1), ("wifi_band", 1, 2), ("charge_cv", crate::process::CONTROL_BATTERY, crate::process::CONTROL_CHARGE_CV)];

fn schema() -> SectionSchema {
    let mut s = SectionSchema::new(SECTION);
    for (name, _) in FIELDS {
        s = s.field(name, TypeConstraint::AnyUnsigned);
    }
    for (name, _, _) in KNOBS {
        s = s.field(name, TypeConstraint::AnyUnsigned);
    }
    s = s.field(TZ, TypeConstraint::AnySigned).field(START_INTO, TypeConstraint::AnyUnsigned);
    for name in [ZONE, ZONE_IDENTITY, ZONE_FOUND] {
        s = s.field(name, TypeConstraint::AsciiText);
    }
    for name in [ZONE_AUTO, ZONE_FOLLOW, ZONE_IDENTITY_AUTO, ZONE_FOUND_BY] {
        s = s.field(name, TypeConstraint::AnyUnsigned);
    }
    s.field(LANGUAGE, TypeConstraint::Language).field(LANGUAGE_IDENTITY, TypeConstraint::Language).field(LANGUAGE_FOLLOW, TypeConstraint::AnyUnsigned).field(BASE, TypeConstraint::AnyUnsigned).field(BASE_FOLLOW, TypeConstraint::AnyUnsigned).field(BASE_IDENTITY, TypeConstraint::AnyUnsigned)
}

/// Eagle time now, in oscillations, from the RTC's base (0 until the RTC has been read: the document then says it was made at the epoch).
pub fn eagle_now() -> i64 {
    eagle_at(crate::platform::cntpct())
}

/// Eagle time at counter tick `tick` (this boot's counter), from the RTC's base; 0 until the RTC has been read.
/// The flash log works out an entry's time from the tick it was made at when it waited in the pre-boot buffer (flashlog.rs).
pub fn eagle_at(tick: u64) -> i64 {
    use core::sync::atomic::Ordering;
    const EPOCH_UNIX: i128 = -14_182_940;
    let base = crate::rtc::BASE.load(Ordering::Relaxed);
    if base == 0 {
        return 0;
    }
    let hz = crate::platform::cntfrq() as i128;
    let t = (base as i128) + tick as i128;
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
    b = b.set(START_INTO, VsfType::u(START_ANDROID.load(core::sync::atomic::Ordering::Relaxed) as usize, false)).ok()?;
    let tz = crate::rtc::TZ.load(core::sync::atomic::Ordering::Relaxed);
    b = b.set(TZ, VsfType::i(tz as isize)).ok()?;
    let z = crate::zone::state();
    let unsigned = |v: u8| VsfType::u(v as usize, false);
    for (name, id) in [(ZONE, z.device.id), (ZONE_IDENTITY, z.identity.map_or(crate::zone::Id::NONE, |c| c.id)), (ZONE_FOUND, z.found)] {
        if !id.is_none() {
            b = b.set(name, VsfType::a(vsf::Ascii::from_bytes(id.as_bytes())?)).ok()?;
        }
    }
    b = b.set(ZONE_AUTO, unsigned(z.device.auto as u8)).ok()?.set(ZONE_FOLLOW, unsigned(z.follow as u8)).ok()?;
    if let Some(c) = z.identity {
        b = b.set(ZONE_IDENTITY_AUTO, unsigned(c.auto as u8)).ok()?;
    }
    if !z.found.is_none() {
        b = b.set(ZONE_FOUND_BY, unsigned(z.found_by)).ok()?;
    }
    for (name, code) in [(LANGUAGE, crate::lang::device()), (LANGUAGE_IDENTITY, crate::lang::identity())] {
        if let Some(l) = language_of(code) {
            b = b.set(name, VsfType::wl(l)).ok()?;
        }
    }
    b = b.set(LANGUAGE_FOLLOW, unsigned(!crate::lang::own() as u8)).ok()?;
    b = b.set(BASE, unsigned(crate::base::device())).ok()?.set(BASE_FOLLOW, unsigned(!crate::base::own() as u8)).ok()?;
    if crate::base::identity() != 0 {
        b = b.set(BASE_IDENTITY, unsigned(crate::base::identity())).ok()?;
    }
    // A knob still unset (all ones) is left out: absent means the driver's own default.
    for (name, link, slot) in KNOBS {
        let v = crate::process::CONTROL[link][slot].load(core::sync::atomic::Ordering::Relaxed);
        if v != u64::MAX {
            b = b.set(name, VsfType::u(v as usize, false)).ok()?;
        }
    }
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
    // What the device starts into: Android only for exactly 1.
    if let Some(v) = first(START_INTO).and_then(|v| v.as_u64()) {
        START_ANDROID.store(v == 1, core::sync::atomic::Ordering::Relaxed);
        n += 1;
    }
    // The knobs as kept; a driver ignores a value it cannot use, so none is range-checked here.
    for (name, link, slot) in KNOBS {
        if let Some(v) = first(name).and_then(|v| v.as_u64()) {
            crate::process::CONTROL[link][slot].store(v, core::sync::atomic::Ordering::Relaxed);
            crate::process::CONTROL_GEN[link].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            n += 1;
        }
    }
    // The zone's choices, each absent one at its default (automatic, following the identity, nothing from the identity or found yet); an id that is not one is none.
    let id = |name: &str| match first(name) {
        Some(VsfType::a(z)) => crate::zone::Id::new(z.as_bytes()).unwrap_or(crate::zone::Id::NONE),
        _ => crate::zone::Id::NONE,
    };
    let flag = |name: &str| first(name).and_then(|v| v.as_u64()).map(|v| v == 1);
    let device = crate::zone::Choice { auto: flag(ZONE_AUTO).unwrap_or(true), id: id(ZONE) };
    let identity = flag(ZONE_IDENTITY_AUTO).map(|auto| crate::zone::Choice { auto, id: id(ZONE_IDENTITY) });
    // WHY the filter: the document can be stored from the bridge; a finder past 2 (the position) is none zone.rs knows, read as none.
    let found_by = first(ZONE_FOUND_BY).and_then(|v| v.as_u64()).filter(|&b| b <= 2).unwrap_or(0) as u8;
    crate::zone::load(device, identity, flag(ZONE_FOLLOW).unwrap_or(true), id(ZONE_FOUND), found_by);
    n += 1;
    // The language's choices, each absent one at its default (none, following the identity, nothing from it yet); a field that is not a language is none.
    // WHY `valid`: the document can be stored from the bridge, and a packed code past three letters would be handed to every process as one; read as none.
    let language = |name: &str| match first(name) {
        Some(VsfType::wl(l)) if crate::lang::valid(l.packed() as u64) => l.packed(),
        _ => 0,
    };
    crate::lang::load(language(LANGUAGE), language(LANGUAGE_IDENTITY), flag(LANGUAGE_FOLLOW) == Some(false), held);
    n += 1;
    // The base's choices, each absent one at its default (this device's as it stands, DMS at boot; following the identity; nothing from it yet).
    // WHY `valid`: the document can be stored from the bridge, and a code tatau does not know would be handed to every process; read as absent.
    let base = |name: &str| first(name).and_then(|v| v.as_u64()).filter(|&c| crate::base::valid(c)).map(|c| c as u8);
    crate::base::load(base(BASE).unwrap_or(crate::base::device()), base(BASE_IDENTITY).unwrap_or(0), flag(BASE_FOLLOW) == Some(false), held);
    n += 1;
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
