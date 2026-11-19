//! The identity's 32-byte values the processes keep, each a document of one section named for the value, with one field of the same name holding it as its honest VSF type:
//! - the handle proof (`ahau.proof`, `fgtw.handle_proof`): `hP`, Photon's handle proof (ihi's memory-hard proof over the identity seed);
//! - the identity seed (`fgtw.identity_seed`): `hI`, Photon's identity seed (BLAKE3 of the handle);
//! - the fleet key (`fgtw.fleet_key`): `kc`, a ChaCha20-Poly1305 key (fgtw's fan-out hands it to each member; it keys the fleet state's ChaCha20-Poly1305 in Photon's `kete` and the identity space's XChaCha20-Poly1305 thru a derived key);
//! - the wallpaper seed (`ahau.wallpaper`): `hb`, a BLAKE3 output (ahau_core::wallpaper_seed, BLAKE3's key derivation from the identity seed);
//! - the made wallpaper's hash (`map.wallpaper.made.key`): `hb`, BLAKE3 of the wallpaper seed and the screen's width and height (payloads/wallgen), what the kept `wallpaper.made` was made from.
//!
//! The documents are plaintext to the process, as the raw 32 bytes were: the vault seals them (identity class).

use crate::{document, first, only};
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

/// Which value a document holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    HandleProof,
    IdentitySeed,
    FleetKey,
    WallpaperSeed,
    WallpaperMade,
}

impl Kind {
    /// The section's name, and its one field's.
    pub fn name(self) -> &'static str {
        match self {
            Kind::HandleProof => "handle_proof",
            Kind::IdentitySeed => "identity_seed",
            Kind::FleetKey => "fleet_key",
            Kind::WallpaperSeed => "wallpaper_seed",
            Kind::WallpaperMade => "wallpaper_made",
        }
    }

    fn field(self) -> &'static str {
        match self {
            Kind::WallpaperMade => "made_hash",
            k => k.name(),
        }
    }

    fn typed(self, b: &[u8; 32]) -> VsfType {
        match self {
            Kind::HandleProof => VsfType::hP(b.to_vec()),
            Kind::IdentitySeed => VsfType::hI(b.to_vec()),
            Kind::FleetKey => VsfType::kc(b.to_vec()),
            Kind::WallpaperSeed | Kind::WallpaperMade => VsfType::hb(b.to_vec()),
        }
    }

    /// The value's bytes when `v` is this kind's type marker and 32 bytes long.
    fn untyped(self, v: &VsfType) -> Option<[u8; 32]> {
        let b = match (self, v) {
            (Kind::HandleProof, VsfType::hP(b)) | (Kind::IdentitySeed, VsfType::hI(b)) | (Kind::FleetKey, VsfType::kc(b)) | (Kind::WallpaperSeed | Kind::WallpaperMade, VsfType::hb(b)) => b,
            _ => return None,
        };
        <[u8; 32]>::try_from(b.as_slice()).ok()
    }

    fn constraint(self) -> TypeConstraint {
        match self {
            Kind::HandleProof => only("hP", |v| matches!(v, VsfType::hP(_))),
            Kind::IdentitySeed => only("hI", |v| matches!(v, VsfType::hI(_))),
            Kind::FleetKey => TypeConstraint::Curve25519Key,
            Kind::WallpaperSeed | Kind::WallpaperMade => TypeConstraint::Blake3Rolling,
        }
    }

    fn schema(self) -> SectionSchema {
        SectionSchema::new(self.name()).field(self.field(), self.constraint())
    }
}

/// The document for `value` as `kind`, stamped at `eagle`.
pub fn build(kind: Kind, value: &[u8; 32], eagle: i64) -> Option<Vec<u8>> {
    let b = kind.schema().build().set(kind.field(), kind.typed(value)).ok()?;
    document(kind.name(), b.encode().ok()?, eagle)
}

/// The value a `kind` document holds. None for anything that is not one (another kind's document among them).
pub fn parse(kind: Kind, doc: &[u8]) -> Option<[u8; 32]> {
    let s = SectionBuilder::parse_document(kind.schema(), doc, None).ok()?;
    kind.untyped(&first(&s, kind.field())?)
}


/// What the identity service does with the roots it keeps when it reads unlock at boot again (fgtw `follow_persist`, each pass; AUTH.md "Unlock at boot").
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Follow {
    /// The setting is as the service last followed it: nothing to do.
    Unchanged,
    /// Turned on: the roots, the fleet key and the chain in hand are kept now, so the next boot that unlocks finds them.
    Keep,
    /// Turned off: everything of the identity goes from the disk now (off is always safe).
    Forget,
}

/// `on` is the setting now, `keeping` whether the service keeps the roots already.
/// Until 2026-11-18 the service read it only at its start and when roots were handed in, so turning it on after left the roots unkept, while the kernel keeps its own seed at the next unlock with it on: the two keeps could part (the Pro, b970).
pub fn follow(on: bool, keeping: bool) -> Follow {
    match (on, keeping) {
        (true, false) => Follow::Keep,
        (false, true) => Follow::Forget,
        _ => Follow::Unchanged,
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    const KINDS: [Kind; 5] = [Kind::HandleProof, Kind::IdentitySeed, Kind::FleetKey, Kind::WallpaperSeed, Kind::WallpaperMade];

    #[test]
    fn each_secret_round_trips_and_no_kind_reads_as_another() {
        for (k, kind) in KINDS.iter().enumerate() {
            let v = [0x40 + k as u8; 32];
            let doc = build(*kind, &v, 1 << 44).unwrap();
            assert_eq!(parse(*kind, &doc), Some(v));
            assert_eq!(crate::created(&doc), Some(1 << 44));
            for other in KINDS.iter().filter(|o| *o != kind) {
                assert_eq!(parse(*other, &doc), None, "{kind:?} read as {other:?}");
            }
        }
        std::println!("a secret: {} bytes", build(Kind::FleetKey, &[0; 32], 0).unwrap().len());
    }

    #[test]
    fn raw_bytes_are_not_a_secret_document() {
        assert_eq!(parse(Kind::HandleProof, &[7; 32]), None);
    }

    /// The disk follows the setting both ways and does nothing while it stands: turned on mid-session keeps, turned off forgets.
    #[test]
    fn the_kept_roots_follow_unlock_at_boot() {
        assert_eq!(follow(true, false), Follow::Keep);
        assert_eq!(follow(false, true), Follow::Forget);
        assert_eq!(follow(true, true), Follow::Unchanged);
        assert_eq!(follow(false, false), Follow::Unchanged);
        // A pass after each change finds nothing more to do.
        let mut keeping = false;
        for on in [true, true, false, false, true] {
            match follow(on, keeping) {
                Follow::Keep => keeping = true,
                Follow::Forget => keeping = false,
                Follow::Unchanged => {}
            }
            assert_eq!(keeping, on);
            assert_eq!(follow(on, keeping), Follow::Unchanged);
        }
    }
}
