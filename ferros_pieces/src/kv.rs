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
/// The seed kept for unlock at boot (`\0id.unlock`): section `identity_unlock`, field `seed`, the identity seed `hI`.
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

/// The one-shot move from the old forms (2026-11-17), run at the first boot after the change and deleted the same day once it has run on every device.
pub mod convert {
    use super::*;

    /// The genesis value as it was kept: this text.
    pub const OLD_GENESIS: &[u8] = b"FERROS-GENESIS-0";

    /// Values by name in one class of the kernel's vault, opened and sealed by the vault.
    pub trait Values {
        type Error;
        fn get(&mut self, name: &str) -> Result<Option<Vec<u8>>, Self::Error>;
        /// `value` under `name`, durable when this returns; a refused write leaves what was there.
        fn put(&mut self, name: &str, value: &[u8]) -> Result<(), Self::Error>;
        /// A buffer that held a value, overwritten (the kernel pushes it out to DRAM: keys.rs `wipe_to_dram`).
        fn wipe(&mut self, b: &mut [u8]) {
            b.fill(0);
        }
    }

    /// What became of one value.
    #[derive(Debug, PartialEq, Eq)]
    pub enum Moved {
        /// Nothing under its name.
        Absent,
        /// Already its document.
        Already,
        /// Converted: its document written over it and read back as written.
        Converted,
        /// Left as it was, and why.
        Left(&'static str),
    }

    /// One value moved: read; left alone when it is absent or already `is_new`; otherwise `from_old` makes its document from the old bytes (None: not the old form either, left), written over the old, read back and compared.
    /// A refused write leaves the old value (manifestus commits a put whole or not at all); a write that does not read back as written is undone by writing the old value back.
    /// Every buffer that held the value is wiped before this returns (some are secrets).
    pub fn one<V: Values>(v: &mut V, name: &str, is_new: impl Fn(&[u8]) -> bool, from_old: impl Fn(&[u8]) -> Option<Vec<u8>>) -> Moved {
        let mut old = match v.get(name) {
            Ok(Some(o)) => o,
            Ok(None) => return Moved::Absent,
            Err(_) => return Moved::Left("it could not be read"),
        };
        if is_new(&old) {
            v.wipe(&mut old);
            return Moved::Already;
        }
        let Some(mut doc) = from_old(&old) else {
            v.wipe(&mut old);
            return Moved::Left("it is in neither the old form nor the new");
        };
        let r = if v.put(name, &doc).is_err() {
            Moved::Left("the write was refused; the old value stands")
        } else {
            match v.get(name) {
                Ok(Some(mut back)) => {
                    let same = back == doc;
                    v.wipe(&mut back);
                    if same {
                        Moved::Converted
                    } else if v.put(name, &old).is_ok() {
                        Moved::Left("it did not read back as written; the old value written back")
                    } else {
                        Moved::Left("it did not read back as written, and the old value could not be written back")
                    }
                }
                _ if v.put(name, &old).is_ok() => Moved::Left("it did not read back; the old value written back"),
                _ => Moved::Left("it did not read back, and the old value could not be written back"),
            }
        };
        v.wipe(&mut old);
        v.wipe(&mut doc);
        r
    }

    /// The old name list: newline-joined text, its reserved and empty entries dropped as its reader dropped them.
    pub fn names(old: &[u8], now: i64) -> Option<Vec<u8>> {
        let s = core::str::from_utf8(old).ok()?;
        let names: Vec<String> = s.split('\n').filter(|n| !n.is_empty() && !n.starts_with('\0')).map(String::from).collect();
        names_document(&names, now)
    }

    /// An old address list: whole 32-byte addresses, concatenated.
    pub fn addresses(old: &[u8], now: i64) -> Option<Vec<u8>> {
        addresses_document(old, now)
    }

    /// The old identity address list: whole 32-byte vault addresses, concatenated.
    pub fn entries(old: &[u8], now: i64) -> Option<Vec<u8>> {
        entries_document(old, now)
    }

    /// The old device-only secret: exactly 32 bytes.
    pub fn secret(old: &[u8], now: i64) -> Option<Vec<u8>> {
        secret_document(&k32(old)?, now)
    }

    /// The old kept seed: exactly 32 bytes.
    pub fn unlock(old: &[u8], now: i64) -> Option<Vec<u8>> {
        unlock_document(&k32(old)?, now)
    }

    /// The old bound mark: the one byte 1, the only value ever written.
    pub fn bound(old: &[u8], now: i64) -> Option<Vec<u8>> {
        (old == [1]).then(|| bound_document(true, now)).flatten()
    }

    /// The old genesis marker: when it was written is not known, so `written` is left out (`genesis_document` with no clock).
    pub fn genesis(old: &[u8], now: i64) -> Option<Vec<u8>> {
        // The document's own creation time is now, the conversion's; the value's first writing is not, and is not made up.
        (old == OLD_GENESIS).then(|| {
            let name = String::from(GENESIS_SECTION);
            let section = genesis_schema().build().encode().ok()?;
            document(&name, section, now)
        })?
    }

    /// The raw store underneath the vault's seal: every live address, and its stored bytes as they lie.
    pub trait Raw {
        type Error;
        fn keys(&mut self) -> Result<Vec<[u8; 32]>, Self::Error>;
        fn get(&mut self, key: &[u8; 32]) -> Result<Option<Vec<u8>>, Self::Error>;
        fn put(&mut self, key: &[u8; 32], value: &[u8]) -> Result<(), Self::Error>;
    }

    /// What the envelope move did: how many were already documents, how many it wrapped, and each one it left and why.
    #[derive(Debug, Default)]
    pub struct Wrapped {
        pub already: usize,
        pub wrapped: usize,
        pub left: Vec<([u8; 32], &'static str)>,
    }

    /// Every stored value's bare envelope wrapped in its document, needing no key (the envelope is moved as it lies, sealed): read, wrapped, written over itself, read back and compared, the old bytes written back if it does not read back as written.
    /// Err only when the live addresses cannot be listed.
    pub fn envelopes<R: Raw>(r: &mut R, now: i64) -> Result<Wrapped, R::Error> {
        let mut out = Wrapped::default();
        for k in r.keys()? {
            let old = match r.get(&k) {
                Ok(Some(o)) => o,
                Ok(None) => continue,
                Err(_) => {
                    out.left.push((k, "it could not be read"));
                    continue;
                }
            };
            if envelope_of(&old).is_some() {
                out.already += 1;
                continue;
            }
            if old.len() < ENVELOPE_MIN {
                out.left.push((k, "too short to be a sealed value"));
                continue;
            }
            let Some(doc) = sealed_document(old.clone(), now) else {
                out.left.push((k, "its document did not build"));
                continue;
            };
            if r.put(&k, &doc).is_err() {
                out.left.push((k, "the write was refused; the old value stands"));
                continue;
            }
            match r.get(&k) {
                Ok(Some(back)) if back == doc => out.wrapped += 1,
                _ => out.left.push((k, if r.put(&k, &old).is_ok() { "it did not read back as written; the old value written back" } else { "it did not read back as written, and the old value could not be written back" })),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::convert::*;
    use super::*;
    use std::collections::BTreeMap;

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

    /// One class of the kernel's vault in memory; `refuse` makes every put fail, `garble` makes the next read of a written value differ.
    #[derive(Default)]
    struct Mem {
        map: BTreeMap<String, Vec<u8>>,
        refuse: bool,
        garble: bool,
    }
    impl Values for Mem {
        type Error = ();
        fn get(&mut self, name: &str) -> Result<Option<Vec<u8>>, ()> {
            let mut v = self.map.get(name).cloned();
            if self.garble {
                self.garble = false;
                if let Some(b) = v.as_mut() {
                    b[0] ^= 1;
                }
            }
            Ok(v)
        }
        fn put(&mut self, name: &str, value: &[u8]) -> Result<(), ()> {
            if self.refuse {
                return Err(());
            }
            self.map.insert(String::from(name), value.to_vec());
            Ok(())
        }
    }

    /// Each old value exactly as the kernel wrote it before 2026-11-17 (kvault.rs: `names.join("\n")`, the chunks concatenated, the 32 bytes as they were, `[1]`, vault_boot's MSG).
    fn old_vault() -> Mem {
        let mut m = Mem::default();
        let names = ["device", "genesis", "sealed", "zones"].join("\n");
        m.map.insert(String::from("\0names"), names.into_bytes());
        m.map.insert(String::from("\0addrs.\0tag."), [[0xA0u8; 32], [0xA1; 32]].concat());
        m.map.insert(String::from("\0id.addrs"), [[0xB0u8; 32], [0xB1; 32], [0xB2; 32]].concat());
        m.map.insert(String::from("\0id.secret"), alloc::vec![0x5E; 32]);
        m.map.insert(String::from("\0id.unlock"), alloc::vec![0x5D; 32]);
        m.map.insert(String::from("\0id.bound"), alloc::vec![1]);
        m.map.insert(String::from("genesis"), OLD_GENESIS.to_vec());
        m
    }

    fn move_all(m: &mut Mem) -> Vec<Moved> {
        alloc::vec![
            one(m, "\0names", |d| names_of(d).is_some(), |o| names(o, NOW)),
            one(m, "\0addrs.\0tag.", |d| addresses_of(d).is_some(), |o| addresses(o, NOW)),
            one(m, "\0id.addrs", |d| entries_of(d).is_some(), |o| entries(o, NOW)),
            one(m, "\0id.secret", |d| secret_of(d).is_some(), |o| secret(o, NOW)),
            one(m, "\0id.unlock", |d| unlock_of(d).is_some(), |o| unlock(o, NOW)),
            one(m, "\0id.bound", |d| bound_of(d).is_some(), |o| bound(o, NOW)),
            one(m, "genesis", |d| genesis_of(d).is_some(), |o| genesis(o, NOW)),
        ]
    }

    #[test]
    fn every_old_value_converts_and_reads_as_it_meant() {
        let mut m = old_vault();
        assert!(move_all(&mut m).iter().all(|r| *r == Moved::Converted));
        assert_eq!(names_of(&m.map["\0names"]).unwrap(), ["device", "genesis", "sealed", "zones"]);
        assert_eq!(addresses_of(&m.map["\0addrs.\0tag."]).unwrap(), [[0xA0u8; 32], [0xA1; 32]].concat());
        assert_eq!(entries_of(&m.map["\0id.addrs"]).unwrap(), [[0xB0u8; 32], [0xB1; 32], [0xB2; 32]].concat());
        assert_eq!(secret_of(&m.map["\0id.secret"]), Some([0x5E; 32]));
        assert_eq!(unlock_of(&m.map["\0id.unlock"]), Some([0x5D; 32]));
        assert_eq!(bound_of(&m.map["\0id.bound"]), Some(true));
        assert_eq!(genesis_of(&m.map["genesis"]), Some(None));
        // A second run finds them all done.
        assert!(move_all(&mut m).iter().all(|r| *r == Moved::Already));
    }

    #[test]
    fn the_old_name_list_drops_what_its_reader_dropped() {
        let d = names(b"\nb\n\0selftest-marker\na\n", NOW).unwrap();
        assert_eq!(names_of(&d).unwrap(), ["b", "a"]);
        assert_eq!(names_of(&names(b"", NOW).unwrap()).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn what_is_not_the_old_form_stays_as_it_is() {
        let mut m = Mem::default();
        m.map.insert(String::from("\0id.secret"), alloc::vec![1; 31]);
        m.map.insert(String::from("\0id.bound"), alloc::vec![2]);
        m.map.insert(String::from("\0id.addrs"), alloc::vec![1; 33]);
        m.map.insert(String::from("genesis"), b"FERROS-GENESIS-1".to_vec());
        m.map.insert(String::from("\0names"), alloc::vec![0xFF, 0xFE]);
        let before = m.map.clone();
        for r in move_all(&mut m) {
            assert!(matches!(r, Moved::Left(_) | Moved::Absent), "{r:?}");
        }
        assert_eq!(m.map, before);
    }

    #[test]
    fn a_refused_write_leaves_the_old_value() {
        let mut m = old_vault();
        m.refuse = true;
        let before = m.map.clone();
        assert!(move_all(&mut m).iter().all(|r| matches!(r, Moved::Left(_))));
        assert_eq!(m.map, before);
    }

    #[test]
    fn a_write_that_reads_back_wrong_is_undone() {
        let mut m = old_vault();
        let before = m.map["\0id.secret"].clone();
        // The read of the old value is clean; the garble hits the read-back.
        struct Garbled(Mem, usize);
        impl Values for Garbled {
            type Error = ();
            fn get(&mut self, name: &str) -> Result<Option<Vec<u8>>, ()> {
                self.1 += 1;
                // The second read is the read-back.
                self.0.garble = self.1 == 2;
                self.0.get(name)
            }
            fn put(&mut self, name: &str, value: &[u8]) -> Result<(), ()> {
                self.0.put(name, value)
            }
        }
        let mut g = Garbled(core::mem::take(&mut m), 0);
        let r = one(&mut g, "\0id.secret", |d| secret_of(d).is_some(), |o| secret(o, NOW));
        assert!(matches!(r, Moved::Left(_)));
        assert_eq!(g.0.map["\0id.secret"], before);
    }

    /// The raw store under the seal, in memory.
    #[derive(Default)]
    struct RawMem(BTreeMap<[u8; 32], Vec<u8>>);
    impl Raw for RawMem {
        type Error = ();
        fn keys(&mut self) -> Result<Vec<[u8; 32]>, ()> {
            Ok(self.0.keys().copied().collect())
        }
        fn get(&mut self, k: &[u8; 32]) -> Result<Option<Vec<u8>>, ()> {
            Ok(self.0.get(k).cloned())
        }
        fn put(&mut self, k: &[u8; 32], v: &[u8]) -> Result<(), ()> {
            self.0.insert(*k, v.to_vec());
            Ok(())
        }
    }

    #[test]
    fn real_envelopes_are_wrapped_and_still_open() {
        use chacha20poly1305::aead::{AeadInPlace, KeyInit};
        use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&[0x42; 32]));
        let mut r = RawMem::default();
        // Each as kvault `seal` made it: the nonce, the plaintext encrypted in place, the tag; a piece's address as associated data, none for a name's value.
        let values: [(&[u8], [u8; 32]); 3] = [(b"settings document", [1; 32]), (&[0u8; 0], [2; 32]), (&[7u8; 5000], [3; 32])];
        for (plain, at) in values {
            let n = [at[0]; 24];
            let mut out = n.to_vec();
            out.extend_from_slice(plain);
            let tag = cipher.encrypt_in_place_detached(XNonce::from_slice(&n), &at, &mut out[24..]).unwrap();
            out.extend_from_slice(&tag);
            r.0.insert(at, out);
        }
        // One stored already as a document, and one too short to be an envelope.
        r.0.insert([4; 32], sealed_document(alloc::vec![9; 40], NOW).unwrap());
        r.0.insert([5; 32], alloc::vec![1; 39]);
        let w = envelopes(&mut r, NOW).unwrap();
        assert_eq!((w.already, w.wrapped, w.left.len()), (1, 3, 1));
        assert_eq!(w.left[0].0, [5; 32]);
        assert_eq!(r.0[&[5; 32]], alloc::vec![1; 39]);
        for (plain, at) in values {
            let mut e = envelope_of(&r.0[&at]).unwrap();
            let n = e[..24].to_vec();
            e.drain(..24);
            cipher.decrypt_in_place(XNonce::from_slice(&n), &at, &mut e).unwrap();
            assert_eq!(e, plain);
        }
        // A second run wraps nothing.
        let w = envelopes(&mut r, NOW).unwrap();
        assert_eq!((w.already, w.wrapped, w.left.len()), (4, 0, 1));
    }
}
