//! One ledger entry (LEDGER.md "Entry format"): a complete VSF document, built and read with vsf's schema builders.
//!
//! ```text
//! RÅ< z y b l [e6 Eagle time] hp(provenance) n (ledger o b n) >
//! [ (seq:u) (boot:u) (prev:hp) (class:u) (key:hp) (nonce:mb) (tag:mp) (sealed:vX) ]
//! ```
//!
//! In the open: what the ring and the chain need without a key (the sequence, the boot, the previous entry's provenance hash, the class and the key's id), and the seal's nonce and tag.
//! Sealed (`sealed`, XChaCha20-Poly1305, wrap `X`): a `ledger_payload` section with the category, the writer and the words.
//! The Eagle time is the header's creation time, absent while the device has not read its clock (VSF's way for a device that cannot know the time); the AEAD binds it, with every open field, so none of them can be moved to another entry.
//!
//! The nonce is synthetic: BLAKE3 keyed by a subkey of the seal key over the bound fields and the plaintext, cut to 24 bytes (the SIV construction).
//! It needs no entropy, so an entry can be sealed before the wairua is drawn, and it repeats only for the same plaintext under the same open fields, which the sequence and the chain never repeat.
//!
//! Every integer is VSF's auto-sized unsigned (EWE): no fixed width on the flash.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{Key, Tag, XChaCha20Poly1305, XNonce};
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::{EtType, VsfBuilder, VsfType};

use crate::category::{Category, Class};

/// A 32-byte BLAKE3 hash.
pub type Hash = [u8; 32];

/// The open section's name, and the sealed one's.
pub const SECTION: &str = "ledger";
pub const PAYLOAD: &str = "ledger_payload";
/// The AEAD's associated data: the open fields and the time, as a section of their own (never stored; rebuilt to open).
const BIND: &str = "ledger_bind";

pub const SEQ: &str = "seq";
pub const BOOT: &str = "boot";
pub const PREV: &str = "prev";
pub const CLASS: &str = "class";
pub const KEY: &str = "key";
pub const NONCE: &str = "nonce";
pub const TAG: &str = "tag";
pub const SEALED: &str = "sealed";
pub const TIME: &str = "time";
pub const CATEGORY: &str = "category";
pub const WRITER: &str = "writer";
pub const WORDS: &str = "words";

/// The wrap byte of the sealed payload: XChaCha20-Poly1305 (vsf crypto_algorithms WRAP_XCHACHA20POLY1305), its nonce and tag in fields of their own.
pub const WRAP: u8 = b'X';

/// A key's public id (`key`): one-way, so an entry names the key it was sealed under and a reader knows whether it holds it.
const KEY_ID_CONTEXT: &str = "ferros 2026-11-17 ledger key id v0";
/// The subkey the synthetic nonce is keyed by.
const NONCE_CONTEXT: &str = "ferros 2026-11-17 ledger nonce v0";
/// The subkey XChaCha20-Poly1305 runs under.
const AEAD_CONTEXT: &str = "ferros 2026-11-17 ledger seal v0";

/// The previous-entry hash of the first entry ever (LEDGER.md "Day 0": genesis prev_hash = BLAKE3 of 32 zero bytes).
pub fn genesis() -> Hash {
    *blake3::hash(&[0u8; 32]).as_bytes()
}

/// Zero a value in place with volatile writes, so the compiler cannot drop them as dead stores.
pub(crate) fn wipe<T>(v: &mut T) {
    let size = core::mem::size_of::<T>();
    // Words where the value is aligned for them (a hasher's state is a couple of KiB: byte by byte it cost more than the hash).
    let words = if core::mem::align_of::<T>() >= 8 { size / 8 } else { 0 };
    let w = v as *mut T as *mut u64;
    for k in 0..words {
        // SAFETY: k < size / 8 and T is aligned to 8, so w + k is an aligned u64 inside `v`, which the caller holds mutably.
        unsafe { core::ptr::write_volatile(w.add(k), 0) };
    }
    let p = v as *mut T as *mut u8;
    for k in words * 8..size {
        // SAFETY: k < size_of::<T>(), so p + k is inside `v`.
        unsafe { core::ptr::write_volatile(p.add(k), 0) };
    }
}

/// A seal key in memory for one use: the 32 bytes the caller derived (the ira's ledger key, or the person's), and their public id.
/// Wiped when dropped.
/// The caller wipes its own copy.
pub struct SealKey {
    key: [u8; 32],
    id: Hash,
}

impl SealKey {
    pub fn new(key: &[u8; 32]) -> SealKey {
        SealKey { key: *key, id: *blake3::Hasher::new_derive_key(KEY_ID_CONTEXT).update(key).finalize().as_bytes() }
    }

    /// The key's public id, as an entry sealed under it carries it.
    pub fn id(&self) -> &Hash {
        &self.id
    }

    fn sub(&self, context: &str) -> [u8; 32] {
        blake3::derive_key(context, &self.key)
    }
}

impl Drop for SealKey {
    fn drop(&mut self) {
        wipe(&mut self.key);
    }
}

/// Why an entry could not be built, read or opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The bytes do not start a VSF document.
    NotADocument,
    /// The header does not decode, or its length does not fit what is there.
    Header(String),
    /// The document does not verify against its provenance hash.
    Provenance(String),
    /// The `ledger` section is missing, out of the document, or not to the schema.
    Schema(String),
    /// Sealed under another key than the one offered.
    WrongKey,
    /// The seal does not open: the ciphertext, its tag or a bound field changed.
    Seal,
    /// A value VSF would not take (a category name outside VSF's rules).
    Build(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NotADocument => write!(f, "not a VSF document"),
            Error::Header(e) => write!(f, "its header does not decode ({e})"),
            Error::Provenance(e) => write!(f, "it does not verify ({e})"),
            Error::Schema(e) => write!(f, "its ledger section is not to the schema ({e})"),
            Error::WrongKey => write!(f, "sealed under another key"),
            Error::Seal => write!(f, "its seal does not open"),
            Error::Build(e) => write!(f, "not built ({e})"),
        }
    }
}

fn custom(name: &'static str, description: &'static str, f: fn(&VsfType) -> bool) -> TypeConstraint {
    TypeConstraint::Custom { name, description, validator: Arc::new(f) }
}

/// The open section: everything the ring and the chain need, and the seal.
pub fn schema() -> SectionSchema {
    SectionSchema::new(SECTION)
        .field(SEQ, TypeConstraint::AnyUnsigned)
        .field(BOOT, TypeConstraint::AnyUnsigned)
        .field(PREV, TypeConstraint::Blake3Provenance)
        .field(CLASS, TypeConstraint::AnyUnsigned)
        .field(KEY, TypeConstraint::Blake3Provenance)
        .field(NONCE, custom("mb24", "the synthetic nonce: 24 bytes of keyed BLAKE3 (mb)", |v| matches!(v, VsfType::mb(b) if b.len() == 24)))
        .field(TAG, custom("mp16", "the Poly1305 tag (mp, 16 bytes)", |v| matches!(v, VsfType::mp(b) if b.len() == 16)))
        .field(SEALED, TypeConstraint::Wrapped(WRAP))
}

/// The sealed section: the category, the writer, the words.
pub fn payload_schema() -> SectionSchema {
    SectionSchema::new(PAYLOAD).field(CATEGORY, TypeConstraint::DictKey).field(WRITER, TypeConstraint::Utf8Text).field(WORDS, TypeConstraint::Utf8Text)
}

fn bind_schema() -> SectionSchema {
    SectionSchema::new(BIND)
        .field(SEQ, TypeConstraint::AnyUnsigned)
        .field(BOOT, TypeConstraint::AnyUnsigned)
        .field(PREV, TypeConstraint::Blake3Provenance)
        .field(CLASS, TypeConstraint::AnyUnsigned)
        .field(KEY, TypeConstraint::Blake3Provenance)
        .field(TIME, TypeConstraint::AnyEagleTime)
}

/// An unsigned integer as the flash holds every one: VSF's auto-sized `u` (EWE), the smallest width that holds it.
pub fn uint(n: u64) -> VsfType {
    VsfType::u(n as usize, false)
}

fn eagle(t: i64) -> VsfType {
    VsfType::e(EtType::e6(t))
}

fn schema_err(e: vsf::schema::ValidationError) -> Error {
    Error::Schema(e.to_string())
}

/// Where an entry sits in the chain, and when it was made: the open fields that are not the seal's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub seq: u64,
    pub boot: u64,
    pub prev: Hash,
    /// Eagle time in oscillations; None before the device has read its clock this boot.
    pub time: Option<i64>,
}

/// What is sealed: the category's name, who wrote it, the words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    pub category: String,
    pub writer: String,
    pub words: String,
}

impl Payload {
    pub fn new(category: Category, writer: &str, words: &str) -> Payload {
        Payload { category: category.name.to_string(), writer: writer.to_string(), words: words.to_string() }
    }
}

/// An entry as the flash holds it, verified against its provenance hash but not opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub place: Place,
    /// The class number as stored (`Class::from_number` names it; an unknown number is a newer writer's).
    pub class: u64,
    pub key: Hash,
    pub nonce: [u8; 24],
    pub tag: [u8; 16],
    pub sealed: Vec<u8>,
    /// The document's provenance hash: what the next entry's `prev` names.
    pub hash: Hash,
}

fn bind(place: &Place, class: u64, key: &Hash) -> Result<Vec<u8>, Error> {
    let mut b = bind_schema()
        .build()
        .set(SEQ, uint(place.seq))
        .and_then(|b| b.set(BOOT, uint(place.boot)))
        .and_then(|b| b.set(PREV, VsfType::hp(place.prev.to_vec())))
        .and_then(|b| b.set(CLASS, uint(class)))
        .and_then(|b| b.set(KEY, VsfType::hp(key.to_vec())))
        .map_err(schema_err)?;
    if let Some(t) = place.time {
        b = b.set(TIME, eagle(t)).map_err(schema_err)?;
    }
    b.encode().map_err(schema_err)
}

fn synthetic_nonce(key: &SealKey, ad: &[u8], plain: &[u8]) -> [u8; 24] {
    let mut nk = key.sub(NONCE_CONTEXT);
    let mut h = blake3::Hasher::new_keyed(&nk);
    h.update(ad);
    h.update(plain);
    let mut n = [0u8; 24];
    n.copy_from_slice(&h.finalize().as_bytes()[..24]);
    wipe(&mut nk);
    wipe(&mut h);
    n
}

fn cipher(key: &SealKey) -> XChaCha20Poly1305 {
    let mut k = key.sub(AEAD_CONTEXT);
    let c = XChaCha20Poly1305::new(Key::from_slice(&k));
    wipe(&mut k);
    c
}

/// Build one entry: the payload sealed under `key`, the document chained to `place.prev`.
pub fn build(place: &Place, class: Class, key: &SealKey, payload: &Payload) -> Result<Vec<u8>, Error> {
    let category = vsf::types::Ascii::new(payload.category.as_str()).map_err(|_| Error::Build("a category name that is not ASCII".to_string()))?;
    vsf::validate_name(category.as_str()).map_err(Error::Build)?;
    let mut plain = payload_schema()
        .build()
        .set(CATEGORY, VsfType::d(category))
        .and_then(|b| b.set(WRITER, VsfType::x(payload.writer.clone())))
        .and_then(|b| b.set(WORDS, VsfType::x(payload.words.clone())))
        .and_then(|b| b.encode())
        .map_err(schema_err)?;
    let ad = bind(place, class.number(), key.id())?;
    let nonce = synthetic_nonce(key, &ad, &plain);
    let mut c = cipher(key);
    let tag = c.encrypt_in_place_detached(XNonce::from_slice(&nonce), &ad, &mut plain);
    wipe(&mut c);
    let tag = tag.map_err(|_| Error::Seal)?;
    let section = schema()
        .build()
        .set(SEQ, uint(place.seq))
        .and_then(|b| b.set(BOOT, uint(place.boot)))
        .and_then(|b| b.set(PREV, VsfType::hp(place.prev.to_vec())))
        .and_then(|b| b.set(CLASS, uint(class.number())))
        .and_then(|b| b.set(KEY, VsfType::hp(key.id().to_vec())))
        .and_then(|b| b.set(NONCE, VsfType::mb(nonce.to_vec())))
        .and_then(|b| b.set(TAG, VsfType::mp(tag.to_vec())))
        .and_then(|b| b.set(SEALED, VsfType::v(WRAP, plain)))
        .and_then(|b| b.encode())
        .map_err(schema_err)?;
    let mut doc = VsfBuilder::new();
    if let Some(t) = place.time {
        doc = doc.creation_time_oscillations(t);
    }
    doc.provenance_only().add_unboxed(SECTION, section).build().map_err(Error::Build)
}

/// The length of the VSF document that starts at `bytes[0]`, from its header's `l`: None if no document starts there.
/// A length past the end of `bytes` is an error: the document was cut.
pub fn doc_len(bytes: &[u8]) -> Result<usize, Error> {
    if bytes.len() < 4 || &bytes[..3] != "RÅ".as_bytes() || bytes[3] != b'<' {
        return Err(Error::NotADocument);
    }
    let (h, end) = vsf::VsfHeader::decode(bytes).map_err(Error::Header)?;
    if h.file_length < end || h.file_length > bytes.len() {
        return Err(Error::Header(alloc::format!("a length of A#{} where A#{} bytes are left", h.file_length, bytes.len())));
    }
    Ok(h.file_length)
}

/// The provenance hash of a document this build just made (its header read, nothing recomputed): what the next entry chains to.
pub fn hash_of_built(doc: &[u8]) -> Result<Hash, Error> {
    let (h, _) = vsf::VsfHeader::decode(doc).map_err(Error::Header)?;
    match &h.provenance_hash {
        VsfType::hp(b) if b.len() == 32 => Ok(b.as_slice().try_into().unwrap()),
        _ => Err(Error::Provenance("no 32-byte provenance hash".to_string())),
    }
}

/// Read one entry (exactly one document's bytes): verified against its provenance hash, its open section to the schema.
/// Nothing is opened.
pub fn parse(doc: &[u8]) -> Result<Stored, Error> {
    let (header, _) = vsf::verification::read_verified(doc, None).map_err(Error::Provenance)?;
    let hash: Hash = match &header.provenance_hash {
        VsfType::hp(h) if h.len() == 32 => h.as_slice().try_into().unwrap(),
        _ => return Err(Error::Provenance("no 32-byte provenance hash".to_string())),
    };
    let time = match &header.creation_time {
        Some(VsfType::e(EtType::e6(t))) => Some(*t),
        None => None,
        Some(_) => return Err(Error::Header("a creation time that is not e6".to_string())),
    };
    let field = header.fields.iter().find(|f| f.name == SECTION).ok_or_else(|| Error::Schema("no ledger section".to_string()))?;
    // WHY checked: the offset and size are the flash's, read from the document's own table; their sum can wrap and pass a bound check unchecked.
    let end = field.offset_bytes.checked_add(field.size_bytes).ok_or_else(|| Error::Schema("a section range that overflows".to_string()))?;
    if end > doc.len() {
        return Err(Error::Schema(alloc::format!("a section ending at A#{end} in a document of A#{}", doc.len())));
    }
    let s = SectionBuilder::parse(schema(), &doc[field.offset_bytes..end]).map_err(schema_err)?;
    let one = |name: &str| -> Result<VsfType, Error> {
        match s.get(name).map_err(schema_err)?.as_slice() {
            [v] => Ok(v.clone()),
            _ => Err(Error::Schema(alloc::format!("`{name}` is not one value"))),
        }
    };
    let n = |name: &str| s.get_value::<u64>(name).map_err(schema_err);
    let h32 = |name: &str| s.get_value::<[u8; 32]>(name).map_err(schema_err);
    let nonce = match one(NONCE)? {
        VsfType::mb(b) => b.as_slice().try_into().map_err(|_| Error::Schema("a nonce that is not 24 bytes".to_string()))?,
        _ => return Err(Error::Schema("a nonce that is not mb".to_string())),
    };
    let tag = match one(TAG)? {
        VsfType::mp(b) => b.as_slice().try_into().map_err(|_| Error::Schema("a tag that is not 16 bytes".to_string()))?,
        _ => return Err(Error::Schema("a tag that is not mp".to_string())),
    };
    let sealed = match one(SEALED)? {
        VsfType::v(WRAP, b) => b,
        _ => return Err(Error::Schema("a payload that is not wrapped X".to_string())),
    };
    Ok(Stored { place: Place { seq: n(SEQ)?, boot: n(BOOT)?, prev: h32(PREV)?, time }, class: n(CLASS)?, key: h32(KEY)?, nonce, tag, sealed, hash })
}

/// Open a stored entry with `key`: its seal checked (the tag over the bound open fields and the time, and the synthetic nonce over the plaintext), its payload to the schema.
pub fn open(s: &Stored, key: &SealKey) -> Result<Payload, Error> {
    if s.key != *key.id() {
        return Err(Error::WrongKey);
    }
    let ad = bind(&s.place, s.class, &s.key)?;
    let mut buf = s.sealed.clone();
    let mut c = cipher(key);
    let r = c.decrypt_in_place_detached(XNonce::from_slice(&s.nonce), &ad, &mut buf, Tag::from_slice(&s.tag));
    wipe(&mut c);
    let out = (|| {
        r.map_err(|_| Error::Seal)?;
        if synthetic_nonce(key, &ad, &buf) != s.nonce {
            return Err(Error::Seal);
        }
        let p = SectionBuilder::parse(payload_schema(), &buf).map_err(schema_err)?;
        let text = |name: &str| p.get_value::<String>(name).map_err(schema_err);
        Ok(Payload { category: text(CATEGORY)?, writer: text(WRITER)?, words: text(WORDS)? })
    })();
    // The plaintext wiped on every path, opened or not.
    wipe_vec(&mut buf);
    out
}

/// Zero a buffer of plaintext before it is freed.
pub(crate) fn wipe_vec(b: &mut Vec<u8>) {
    for x in b.iter_mut() {
        // SAFETY: x is a valid &mut u8 into the buffer.
        unsafe { core::ptr::write_volatile(x, 0) };
    }
}
