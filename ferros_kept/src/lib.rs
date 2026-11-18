//! What the payload processes keep in the vault, each value one complete VSF document (docs/VSF-AUDIT.md, wave 0, owner 2026-11-17): the magic, a creation time in Eagle oscillations, a provenance hash, and one section of named, typed fields, built with the schema builders and read back thru `SectionBuilder::parse_document`, which verifies the whole document first.
//!
//! | Value | Section | Fields |
//! |---|---|---|
//! | `gps.last` | `last_fix` | `at` `w` (the place), `when` `e` (when the fix came, on the real clock; absent when the clock was not read) |
//! | `map.place.<label>` | `place` | `x`, `y` `i` (world ×256), `radius` `u`, `cos`, `sin` `i` (Q14) |
//! | `map.view` | `view` | `x`, `y`, `cos`, `sin` `i` |
//! | `map.dev` | `developer` | `unlocked` `u0` |
//! | `ahau.proof`, `fgtw.handle_proof` | `handle_proof` | `handle_proof` `hP` |
//! | `fgtw.identity_seed` | `identity_seed` | `identity_seed` `hI` |
//! | `fgtw.fleet_key` | `fleet_key` | `fleet_key` `kc` |
//! | `ahau.wallpaper` | `wallpaper_seed` | `wallpaper_seed` `hb` |
//! | `map.wallpaper.made.key` | `wallpaper_made` | `made_hash` `hb` |
//! | `fgtw.sponsor` | `sponsor` | `address` `ni` or `nj`, `port` `np` |
//!
//! Every maker takes the Eagle time to stamp (`ferros_sys::eagle_now` on the device: 0 before the kernel has read the clock), so all of it runs on the host's tests.
//! Only these documents are read: the old forms (raw bytes, EWE runs, fixed-width structs) were converted once by the build before this one, and a value still in one reads as no value.

#![no_std]

extern crate alloc;

pub mod gps;
pub mod map;
pub mod secret;
pub mod sponsor;

pub use vsf;

use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, TypeConstraint};
use vsf::VsfType;

/// One section's document, stamped at `eagle` (oscillations): None if the builder refuses it.
fn document(section: &str, body: Vec<u8>, eagle: i64) -> Option<Vec<u8>> {
    vsf::VsfBuilder::new().creation_time_oscillations(eagle).provenance_only().add_unboxed(section, body).build().ok()
}

/// A field's first value, by name.
fn first(s: &SectionBuilder, field: &str) -> Option<VsfType> {
    s.get_fields(field).first().and_then(|f| f.values.first()).cloned()
}

/// A constraint for one type marker that vsf's schema has no named constraint for (`w`, `u0`, `hP`, `hI`, `np`): the schema refuses any other on set and on parse.
fn only(name: &'static str, accepts: fn(&VsfType) -> bool) -> TypeConstraint {
    TypeConstraint::Custom { name, description: name, validator: alloc::sync::Arc::new(accepts) }
}

/// The document's creation time, for a reader that wants it (the tests check every maker stamps the time it is given).
pub fn created(doc: &[u8]) -> Option<i64> {
    let (header, _) = vsf::VsfHeader::decode(doc).ok()?;
    match header.creation_time {
        Some(vsf::VsfType::e(vsf::EtType::e6(t))) => Some(t),
        _ => None,
    }
}
