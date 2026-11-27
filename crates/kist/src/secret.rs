//! A signing key kept as VSF (owner, 2026-11-10: secrets move to VSF): the one form of every Ed25519 secret the host tools sign with, and its one reader and writer.
//!
//! - **The form**: a provenance-only VSF document, section `developer_key` { `label` a, `seed` kge }. The seed is RFC 8032's 32-byte private key; the label is the name the Falcon-512 and SPHINCS+ halves derive under (fgtw `SigningBundle::developer`), so it is part of the key and never changed. A developer's key (tools/kistcat `devkey new`) and the ferros key (`ferros.key.vsf`, label `FERROS_PROJECT`, the name kistcat derives the project bundle under) are both this form.
//! - **Readers**: the bridge (`capsule`, $FERROS_DEV_KEY), ferros-mkimg (`sign-kernel`, `seed`) and kistcat (`build`, `verify`, `kist`) all read thru [`read`]; nothing else parses a key file.
//! - **The old form** (a raw file: the 32-byte seed, or ferros-mkimg's 64-byte seed and public key) is refused, never read: [`read`] names the conversion (`kistcat devkey convert`), which writes the VSF form to a new file and leaves the old one as it was.

use alloc::string::String;
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

/// The key's section.
pub const SECTION: &str = "developer_key";
/// The label the ferros key derives its post-quantum halves under (kistcat's project bundle). Part of the key: a different label is a different bundle.
pub const FERROS_LABEL: &str = "FERROS_PROJECT";

/// The refusal for a raw key file, naming the conversion.
pub const RAW: &str = "a raw key file (the form before 2026-11-10): convert it once with `cargo run --release --manifest-path tools/kistcat/Cargo.toml -- devkey convert --raw <this file> --label <LABEL> --secret <new .key.vsf>` (the ferros key: --label FERROS_PROJECT, to keys/ferros.key.vsf) and give the new file";

/// The section's schema: the label as ASCII text, the seed as an Ed25519 signing seed.
pub fn schema() -> SectionSchema {
    SectionSchema::new(SECTION).field("label", TypeConstraint::AsciiText).field("seed", TypeConstraint::Ed25519SigningKey)
}

/// The key document for `seed` under `label`, made at `made` (Eagle time oscillations). The caller writes it to a new file, owner-only, and wipes the seed.
pub fn document(label: &str, seed: &[u8; 32], made: i64) -> Result<Vec<u8>, String> {
    let label = vsf::Ascii::new(label).map_err(|e| alloc::format!("label: {e}"))?;
    let section = schema().build().set("label", VsfType::a(label)).and_then(|b| b.set("seed", VsfType::kge(seed.to_vec()))).and_then(|b| b.encode()).map_err(|e| alloc::format!("{e}"))?;
    vsf::VsfBuilder::new().creation_time_oscillations(made).provenance_only().add_unboxed(SECTION, section).build()
}

/// The key from its document: its label and its 32-byte seed, the document's provenance checked. The caller derives what it signs with (`ed25519` from the seed, or `fgtw::pq::SigningBundle::developer(&seed, &label)`) and wipes the seed.
/// A file that is not VSF and is exactly a raw key's length is refused with [`RAW`].
pub fn read(doc: &[u8]) -> Result<(String, [u8; 32]), String> {
    if !doc.starts_with("RÅ<".as_bytes()) && (doc.len() == 32 || doc.len() == 64) {
        return Err(String::from(RAW));
    }
    let s = SectionBuilder::parse_document(schema(), doc, None).map_err(|e| alloc::format!("not a key document (VSF, section {SECTION}): {e}"))?;
    let first = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first());
    let Some(VsfType::a(label)) = first("label") else { return Err(String::from("no label (a)")) };
    // WHY: the length is the file's; `try_from` refuses a seed of any other length.
    let Some(seed) = (match first("seed") {
        Some(VsfType::kge(k)) => <[u8; 32]>::try_from(k.as_slice()).ok(),
        _ => None,
    }) else {
        return Err(String::from("no 32-byte kge seed"));
    };
    Ok((String::from(label.as_str()), seed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vsf::file_format::VsfSection;

    #[test]
    fn a_key_round_trips() {
        let doc = document("TEST_DEVELOPER", &[3; 32], 1).unwrap();
        assert_eq!(read(&doc).unwrap(), (String::from("TEST_DEVELOPER"), [3; 32]));
    }

    /// A key as kistcat's `devkey new` wrote it before this reader (the section added whole): it still reads.
    #[test]
    fn a_key_written_as_a_section_reads() {
        let mut s = VsfSection::new(SECTION);
        s.add_field("label", VsfType::a(vsf::Ascii::lit("TEST_DEVELOPER")));
        s.add_field("seed", VsfType::kge(alloc::vec![3; 32]));
        let doc = vsf::VsfBuilder::new().creation_time_oscillations(1).provenance_only().add_section_direct(s).build().unwrap();
        assert_eq!(read(&doc).unwrap(), (String::from("TEST_DEVELOPER"), [3; 32]));
    }

    #[test]
    fn a_raw_key_is_refused_with_the_conversion() {
        assert_eq!(read(&[5; 32]).unwrap_err(), RAW);
        assert_eq!(read(&[5; 64]).unwrap_err(), RAW);
    }

    #[test]
    fn a_tampered_or_short_key_is_refused() {
        let mut doc = document("TEST_DEVELOPER", &[3; 32], 1).unwrap();
        let at = doc.len() - 8;
        doc[at] ^= 1;
        assert!(read(&doc).is_err());
        let mut s = VsfSection::new(SECTION);
        s.add_field("label", VsfType::a(vsf::Ascii::lit("TEST_DEVELOPER")));
        s.add_field("seed", VsfType::kge(alloc::vec![3; 31]));
        let short = vsf::VsfBuilder::new().creation_time_oscillations(1).provenance_only().add_section_direct(s).build().unwrap();
        assert!(read(&short).is_err());
    }
}
