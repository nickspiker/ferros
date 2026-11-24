//! The kernel vault's own documents (ferros_kernel kvault.rs; docs/VSF-AUDIT.md P11, P12, P13; owner, 2026-11-17: "yes, go on 0 and 1!"): every value the kernel's vault keeps for itself, and the envelope every sealed value is kept in, each a complete VSF document built with the schema builders.
//! Until 2026-11-17 the names were kept newline-joined, the address lists, the device-only secret and the kept seed as bare bytes, the bound mark as one byte, the genesis value as a text marker, and every sealed value as a bare nonce, ciphertext and tag.
//!
//! Every document's creation time is the caller's `now`: Eagle time in oscillations from the device's clock, 0 (the Eagle epoch) when the clock has not been read, the convention the settings document already keeps for "no clock" (ferros_kernel settings.rs `eagle_now`).

use alloc::string::String;
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

/// A sealed value as the vault stores it: section `vault_seal`, its one field `seal` the XChaCha20-Poly1305 envelope (the 24-byte nonce, the ciphertext, the 16-byte tag) wrapped `v'e'`, as the kernel's tag record keeps its seal.
const SEAL_SECTION: &str = "vault_seal";
const SEAL: &str = "seal";
/// The shortest envelope: a nonce and a tag around nothing.
pub const ENVELOPE_MIN: usize = 24 + 16;

/// The list of names a class keeps (`\0names`, `\0id.names`): section `names`, a `name` field per name, each a `d` (a vault name is a key, ASCII).
const NAMES_SECTION: &str = "names";
const NAME: &str = "name";
/// A scope's list of addresses (`\0addrs.<prefix>`): section `addresses`, an `address` field per entry, each the caller's 32-byte address as `hb`.
const ADDRS_SECTION: &str = "addresses";
/// The identity class's vault addresses (`\0id.addrs`): section `identity_entries`, an `address` field per entry, each a vault address (a keyed BLAKE3 of a name under the identity's address key) as `mb`.
const ENTRIES_SECTION: &str = "identity_entries";
const ADDRESS: &str = "address";
/// The device-only secret mixed into every identity key (`\0id.secret`): section `identity_secret`, field `secret`, 32 bytes of random material `hR`.
const SECRET_SECTION: &str = "identity_secret";
const SECRET: &str = "secret";
/// The device-only secret and the bound mark together (`\0id.secret` since 2026-11-24): section `device_secret`, field `secret` (32 bytes of random material, `hR`) and field `bound` (whether an identity's fleet bound this device under this secret). One document, so renewing the secret (one write) also unbinds: the owner's rule of 2026-11-24 that destroying is a key operation, never deletes ("Sand on a beach my friend"). A device whose secret is still the `identity_secret` document reads its bound mark from `\0id.bound` as before.
const DEVICE_SECRET_SECTION: &str = "device_secret";
/// The seed kept for unlock at boot, sealed under a key made from the device-only secret (`\0id.unlock` since 2026-11-24): section `kept_seed`, field `seal`, the XChaCha20-Poly1305 envelope wrapped `v'e'`. Renewing the secret leaves it unopenable, so nothing need be deleted.
const KEPT_SECTION: &str = "kept_seed";
/// The seed kept for unlock at boot (`\0id.unlock`, until 2026-11-24, read only while the secret is still its `identity_secret` document): section `identity_unlock`, field `seed`, the identity seed `hI`.
const UNLOCK_SECTION: &str = "identity_unlock";
const SEED: &str = "seed";
/// The mark that an identity's fleet bound this device (`\0id.bound`): section `identity_bound`, field `bound`, a boolean.
const BOUND_SECTION: &str = "identity_bound";
const BOUND: &str = "bound";
/// The vault's genesis value (`genesis`): section `genesis`, read back at every boot to show the vault opens under this ira; field `written`, when it was first written (Eagle time), present only when the clock was known then.
const GENESIS_SECTION: &str = "genesis";
const WRITTEN: &str = "written";

fn seal_schema() -> SectionSchema {
    SectionSchema::new(SEAL_SECTION).field(SEAL, TypeConstraint::Wrapped(b'e'))
}
fn names_schema() -> SectionSchema {
    SectionSchema::new(NAMES_SECTION).field(NAME, TypeConstraint::DictKey)
}
fn addrs_schema() -> SectionSchema {
    SectionSchema::new(ADDRS_SECTION).field(ADDRESS, TypeConstraint::Blake3Rolling)
}
fn entries_schema() -> SectionSchema {
    // `mb` has no constraint of its own in vsf's schema; every value is held to it on the way out (`entries_of`).
    SectionSchema::new(ENTRIES_SECTION).field(ADDRESS, TypeConstraint::Any)
}
fn secret_schema() -> SectionSchema {
    SectionSchema::new(SECRET_SECTION).field(SECRET, TypeConstraint::AnyHash)
}
fn device_secret_schema() -> SectionSchema {
    SectionSchema::new(DEVICE_SECRET_SECTION).field(SECRET, TypeConstraint::AnyHash).field(BOUND, TypeConstraint::AnyUnsigned)
}
fn kept_schema() -> SectionSchema {
    SectionSchema::new(KEPT_SECTION).field(SEAL, TypeConstraint::Wrapped(b'e'))
}
fn unlock_schema() -> SectionSchema {
    SectionSchema::new(UNLOCK_SECTION).field(SEED, TypeConstraint::AnyHash)
}
fn bound_schema() -> SectionSchema {
    SectionSchema::new(BOUND_SECTION).field(BOUND, TypeConstraint::AnyUnsigned)
}
fn genesis_schema() -> SectionSchema {
    SectionSchema::new(GENESIS_SECTION).field(WRITTEN, TypeConstraint::AnyEagleTime)
}

/// One section's document.
fn document(name: &str, section: Vec<u8>, now: i64) -> Option<Vec<u8>> {
    vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_unboxed(name, section).build().ok()
}

/// A one-field document.
fn single(schema: SectionSchema, field: &str, v: VsfType, now: i64) -> Option<Vec<u8>> {
    let name = schema.name.clone();
    let section = schema.build().set(field, v).ok()?.encode().ok()?;
    document(&name, section, now)
}

/// A document of one repeated field, its values in order.
fn many(schema: SectionSchema, field: &str, values: impl Iterator<Item = VsfType>, now: i64) -> Option<Vec<u8>> {
    let name = schema.name.clone();
    let mut b = schema.build();
    for v in values {
        b = b.append_multi(field, alloc::vec![v]).ok()?;
    }
    document(&name, b.encode().ok()?, now)
}

/// A field's values, every occurrence's, in order; None for a document that is not of this schema.
fn values(schema: SectionSchema, field: &str, doc: &[u8]) -> Option<Vec<VsfType>> {
    let s = SectionBuilder::parse_document(schema, doc, None).ok()?;
    Some(s.get_fields(field).iter().flat_map(|f| f.values.iter().cloned()).collect())
}

/// A one-field document's value: Some(None) when the document is of this schema but leaves the field out.
fn first(schema: SectionSchema, field: &str, doc: &[u8]) -> Option<Option<VsfType>> {
    values(schema, field, doc).map(|v| v.into_iter().next())
}

/// A 32-byte value out of a hash-like field's bytes.
fn k32(b: &[u8]) -> Option<[u8; 32]> {
    b.try_into().ok()
}

// ── The envelope ──

/// The document a sealed value is stored as, around its envelope (`[nonce][ciphertext][tag]`, moved in, never copied by the caller).
pub fn sealed_document(envelope: Vec<u8>, now: i64) -> Option<Vec<u8>> {
    single(seal_schema(), SEAL, VsfType::v(b'e', envelope), now)
}

/// The envelope out of a stored document; None for anything that is not one (or one whose envelope cannot hold a nonce and a tag).
pub fn envelope_of(doc: &[u8]) -> Option<Vec<u8>> {
    match first(seal_schema(), SEAL, doc)? {
        Some(VsfType::v(b'e', e)) if e.len() >= ENVELOPE_MIN => Some(e),
        _ => None,
    }
}

// ── The lists ──

/// The list of names; None when a name is not ASCII (a vault name is a key, `d`).
pub fn names_document(names: &[String], now: i64) -> Option<Vec<u8>> {
    let mut v = Vec::with_capacity(names.len());
    for n in names {
        v.push(VsfType::d(vsf::Ascii::from_bytes(n.as_bytes())?));
    }
    many(names_schema(), NAME, v.into_iter(), now)
}

/// The names in a list document; None for anything that is not one.
pub fn names_of(doc: &[u8]) -> Option<Vec<String>> {
    values(names_schema(), NAME, doc)?
        .into_iter()
        .map(|v| match v {
            VsfType::d(a) => Some(String::from(a.as_str())),
            _ => None,
        })
        .collect()
}

/// A scope's addresses (32 bytes each, concatenated, as the vault hands them out) as their document; None when the bytes are not whole addresses.
pub fn addresses_document(list: &[u8], now: i64) -> Option<Vec<u8>> {
    if list.len() % 32 != 0 {
        return None;
    }
    many(addrs_schema(), ADDRESS, list.chunks_exact(32).map(|c| VsfType::hb(c.to_vec())), now)
}

/// A scope's addresses out of their document, concatenated; None for anything that is not one.
pub fn addresses_of(doc: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for v in values(addrs_schema(), ADDRESS, doc)? {
        match v {
            VsfType::hb(b) if b.len() == 32 => out.extend_from_slice(&b),
            _ => return None,
        }
    }
    Some(out)
}

/// The identity class's vault addresses (concatenated) as their document; None when the bytes are not whole addresses.
pub fn entries_document(list: &[u8], now: i64) -> Option<Vec<u8>> {
    if list.len() % 32 != 0 {
        return None;
    }
    many(entries_schema(), ADDRESS, list.chunks_exact(32).map(|c| VsfType::mb(c.to_vec())), now)
}

/// The identity class's vault addresses out of their document, concatenated; None for anything that is not one.
pub fn entries_of(doc: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for v in values(entries_schema(), ADDRESS, doc)? {
        match v {
            VsfType::mb(b) if b.len() == 32 => out.extend_from_slice(&b),
            _ => return None,
        }
    }
    Some(out)
}

// ── The single values ──

/// The device-only secret's document.
pub fn secret_document(secret: &[u8; 32], now: i64) -> Option<Vec<u8>> {
    single(secret_schema(), SECRET, VsfType::hR(secret.to_vec()), now)
}

/// The device-only secret out of its document.
/// The caller wipes it once used, and the document it came in.
pub fn secret_of(doc: &[u8]) -> Option<[u8; 32]> {
    match first(secret_schema(), SECRET, doc)?? {
        VsfType::hR(b) => k32(&b),
        _ => None,
    }
}

/// The device-only secret with the bound mark (`device_secret`).
pub fn device_secret_document(secret: &[u8; 32], bound: bool, now: i64) -> Option<Vec<u8>> {
    let name = String::from(DEVICE_SECRET_SECTION);
    let section = device_secret_schema().build().set(SECRET, VsfType::hR(secret.to_vec())).ok()?.set(BOUND, VsfType::u0(bound)).ok()?.encode().ok()?;
    document(&name, section, now)
}

/// The device-only secret and the bound mark out of a `device_secret` document; None for anything else (an `identity_secret` document is read by `secret_of`).
/// The caller wipes the secret once used, and the document it came in.
pub fn device_secret_of(doc: &[u8]) -> Option<([u8; 32], bool)> {
    let s = SectionBuilder::parse_document(device_secret_schema(), doc, None).ok()?;
    let first = |f: &str| s.get_fields(f).first().and_then(|x| x.values.first()).cloned();
    let secret = match first(SECRET)? {
        VsfType::hR(b) => k32(&b)?,
        _ => return None,
    };
    let bound = match first(BOUND)? {
        VsfType::u0(b) => b,
        _ => return None,
    };
    Some((secret, bound))
}

/// The kept seed's sealed document (`kept_seed`): the envelope as the caller sealed it.
pub fn kept_document(envelope: Vec<u8>, now: i64) -> Option<Vec<u8>> {
    single(kept_schema(), SEAL, VsfType::v(b'e', envelope), now)
}

/// The envelope out of a `kept_seed` document.
pub fn kept_of(doc: &[u8]) -> Option<Vec<u8>> {
    match first(kept_schema(), SEAL, doc)?? {
        VsfType::v(b'e', b) if b.len() >= ENVELOPE_MIN => Some(b),
        _ => None,
    }
}

/// The kept seed's document.
pub fn unlock_document(seed: &[u8; 32], now: i64) -> Option<Vec<u8>> {
    single(unlock_schema(), SEED, VsfType::hI(seed.to_vec()), now)
}

/// The kept seed out of its document.
/// The caller wipes it once used, and the document it came in.
pub fn unlock_of(doc: &[u8]) -> Option<[u8; 32]> {
    match first(unlock_schema(), SEED, doc)?? {
        VsfType::hI(b) => k32(&b),
        _ => None,
    }
}

/// The bound mark's document.
pub fn bound_document(bound: bool, now: i64) -> Option<Vec<u8>> {
    single(bound_schema(), BOUND, VsfType::u0(bound), now)
}

/// Whether the bound mark says bound; None for anything that is not the mark.
pub fn bound_of(doc: &[u8]) -> Option<bool> {
    match first(bound_schema(), BOUND, doc)?? {
        VsfType::u0(b) => Some(b),
        _ => None,
    }
}

/// The genesis value's document: `written` when the clock is known (`now` past the epoch), left out when it is not, so no time is made up.
pub fn genesis_document(now: i64) -> Option<Vec<u8>> {
    let name = String::from(GENESIS_SECTION);
    let mut b = genesis_schema().build();
    if now > 0 {
        b = b.set(WRITTEN, VsfType::e(vsf::EtType::e6(now))).ok()?;
    }
    document(&name, b.encode().ok()?, now)
}

/// Whether a document is the genesis value: Some(when it was written, if the clock was known then), None for anything else.
pub fn genesis_of(doc: &[u8]) -> Option<Option<i64>> {
    match first(genesis_schema(), WRITTEN, doc)? {
        None => Some(None),
        Some(VsfType::e(vsf::EtType::e6(t))) => Some(Some(t)),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1 << 50;

    #[test]
    fn every_document_reads_back() {
        let env: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
        assert_eq!(envelope_of(&sealed_document(env.clone(), NOW).unwrap()).unwrap(), env);
        let names: Vec<String> = ["device", "genesis", "sealed", "aoc.fw.0"].iter().map(|s| String::from(*s)).collect();
        assert_eq!(names_of(&names_document(&names, NOW).unwrap()).unwrap(), names);
        let list: Vec<u8> = (0..96u32).map(|i| i as u8).collect();
        assert_eq!(addresses_of(&addresses_document(&list, NOW).unwrap()).unwrap(), list);
        assert_eq!(entries_of(&entries_document(&list, NOW).unwrap()).unwrap(), list);
        assert_eq!(secret_of(&secret_document(&[9; 32], NOW).unwrap()), Some([9; 32]));
        assert_eq!(unlock_of(&unlock_document(&[3; 32], NOW).unwrap()), Some([3; 32]));
        assert_eq!(bound_of(&bound_document(true, NOW).unwrap()), Some(true));
        assert_eq!(genesis_of(&genesis_document(NOW).unwrap()), Some(Some(NOW)));
        assert_eq!(genesis_of(&genesis_document(0).unwrap()), Some(None));
        assert_eq!(device_secret_of(&device_secret_document(&[5; 32], true, NOW).unwrap()), Some(([5; 32], true)));
        assert_eq!(device_secret_of(&device_secret_document(&[6; 32], false, NOW).unwrap()), Some(([6; 32], false)));
        assert_eq!(kept_of(&kept_document(env.clone(), NOW).unwrap()), Some(env));
    }

    /// The two secret documents are never read for each other, and a kept seed is never read as the old plain one.
    #[test]
    fn the_secret_formats_stay_apart() {
        let new = device_secret_document(&[1; 32], true, NOW).unwrap();
        let old = secret_document(&[1; 32], NOW).unwrap();
        assert_eq!(secret_of(&new), None);
        assert_eq!(device_secret_of(&old), None);
        let kept = kept_document(alloc::vec![2; 72], NOW).unwrap();
        assert_eq!(unlock_of(&kept), None);
        assert_eq!(kept_of(&unlock_document(&[2; 32], NOW).unwrap()), None);
    }

    #[test]
    fn empty_lists_are_documents_too() {
        // The identity's empty name list is what recognises it after a bind (kvault `unlock`).
        let d = names_document(&[], NOW).unwrap();
        assert_eq!(names_of(&d), Some(Vec::new()));
        assert_eq!(addresses_of(&addresses_document(&[], NOW).unwrap()), Some(Vec::new()));
        assert_eq!(entries_of(&entries_document(&[], NOW).unwrap()), Some(Vec::new()));
    }

    #[test]
    fn no_document_is_taken_for_another() {
        let docs = [
            sealed_document(alloc::vec![1; 64], NOW).unwrap(),
            names_document(&[String::from("a")], NOW).unwrap(),
            addresses_document(&[1; 32], NOW).unwrap(),
            entries_document(&[1; 32], NOW).unwrap(),
            secret_document(&[1; 32], NOW).unwrap(),
            unlock_document(&[1; 32], NOW).unwrap(),
            bound_document(true, NOW).unwrap(),
            genesis_document(NOW).unwrap(),
        ];
        let readers: [fn(&[u8]) -> bool; 8] = [
            |d| envelope_of(d).is_some(),
            |d| names_of(d).is_some(),
            |d| addresses_of(d).is_some(),
            |d| entries_of(d).is_some(),
            |d| secret_of(d).is_some(),
            |d| unlock_of(d).is_some(),
            |d| bound_of(d).is_some(),
            |d| genesis_of(d).is_some(),
        ];
        for (i, d) in docs.iter().enumerate() {
            for (j, r) in readers.iter().enumerate() {
                assert_eq!(r(d), i == j, "document {i} read by reader {j}");
            }
        }
    }

    #[test]
    fn a_name_that_is_not_ascii_is_refused() {
        assert!(names_document(&[String::from("caf\u{e9}")], NOW).is_none());
    }

    #[test]
    fn a_tampered_document_does_not_read() {
        let mut d = secret_document(&[5; 32], NOW).unwrap();
        let n = d.len();
        d[n - 3] ^= 1;
        assert_eq!(secret_of(&d), None);
    }

    #[test]
    fn a_real_envelope_reads_back_out_of_its_document_and_opens() {
        use chacha20poly1305::aead::{AeadInPlace, KeyInit};
        use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&[0x42; 32]));
        // As kvault `seal` makes it: the nonce, the plaintext encrypted in place, the tag; a piece's address as associated data.
        let (plain, at, n) = (b"settings document", [1u8; 32], [7u8; 24]);
        let mut out = n.to_vec();
        out.extend_from_slice(plain);
        let tag = cipher.encrypt_in_place_detached(XNonce::from_slice(&n), &at, &mut out[24..]).unwrap();
        out.extend_from_slice(&tag);
        let mut e = envelope_of(&sealed_document(out, NOW).unwrap()).unwrap();
        e.drain(..24);
        cipher.decrypt_in_place(XNonce::from_slice(&n), &at, &mut e).unwrap();
        assert_eq!(e, plain);
        // An envelope too short to hold a nonce and a tag is not one.
        assert_eq!(envelope_of(&sealed_document(alloc::vec![1; ENVELOPE_MIN - 1], NOW).unwrap()), None);
    }
}
