//! A developer's key (owner, 2026-11-10): the secret kept as VSF in the keys directory, and the public record that names whose kists it signs.
//! The secret is one `kge`, the Ed25519 seed, in ferros_kist::secret's form (section `developer_key` { label a, seed kge }; that module is its one reader and writer, shared with the bridge and ferros-mkimg); the Falcon-512 and SPHINCS+ halves derive from it under the developer's label (fgtw `SigningBundle::developer`), so the label is part of the key and is never changed.
//! The record carries the three public keys, the developer's name, contact and web as `x` (the person's words; owner: "x is for text, ascii is for keys, values, labels, system stuff"), the kind, when it was made and its revision, and is signed by every egg of the key it describes: a newer revision of the same key replaces an older one, so a name or contact can change without the key changing.
//! Later, beside it and not in it: succession to a next key, revocation, a link to the developer's FGTW identity, and the automatic fields (their other kists, download and stow counts).

use fgtw::fleet::scheme;
use fgtw::pq::SigningBundle;
use std::io::Write;
use std::path::Path;
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::VsfType;

/// The document's section `name`, its header decoded and its sections parsed.
fn section(doc: &[u8], name: &str) -> Result<VsfSection, String> {
    let (header, end) = VsfHeader::decode(doc).map_err(|e| format!("not a VSF file: {e:?}"))?;
    header.sections(doc, end).map_err(|e| format!("sections that do not parse: {e:?}"))?.into_iter().find(|s| s.name.as_str() == name).ok_or_else(|| format!("no {name} section"))
}

/// The public record, as `record` writes it and `read_record` reads it back.
pub struct Record {
    pub label: String,
    pub name: String,
    pub contact: Option<String>,
    pub web: Option<String>,
    pub kind: String,
    pub created: i64,
    pub revision: u64,
    pub ed25519: [u8; 32],
}

/// The key document for `seed` under `label` written to `secret`: refused if the file exists (a key is never overwritten), owner-only from its creation, then read back thru the one reader and checked to be the same key.
fn write_secret(secret: &Path, label: &str, seed: &[u8; 32], made: i64) -> Result<(), String> {
    let doc = ferros_kist::secret::document(label, seed, made)?;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(secret).map_err(|e| format!("{}: {e} (an existing key is never overwritten)", secret.display()))?;
    f.write_all(&doc).map_err(|e| format!("{}: {e}", secret.display()))?;
    drop(f);
    let (back, mut back_seed) = read_doc(secret)?;
    let same = back == label && back_seed == *seed;
    back_seed.fill(0);
    if !same {
        return Err(format!("{}: the key read back is not the one written: unusable, remove it", secret.display()));
    }
    Ok(())
}

/// A key file read thru the one reader (ferros_kist::secret, shared with the bridge and ferros-mkimg): its label and seed. The file's bytes are wiped once read; the caller wipes the seed.
fn read_doc(secret: &Path) -> Result<(String, [u8; 32]), String> {
    let mut doc = std::fs::read(secret).map_err(|e| format!("{}: {e}", secret.display()))?;
    let read = ferros_kist::secret::read(&doc).map_err(|e| format!("{}: {e}", secret.display()));
    doc.fill(0);
    read
}

/// A new key: 32 bytes from the operating system's random source, written to `secret` (refused if it exists, owner-only permissions) as section `developer_key` { label a, seed kge }.
pub fn new_secret(secret: &Path, label: &str, made: i64) -> Result<SigningBundle, String> {
    let mut seed = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut seed)).map_err(|e| format!("random: {e}"))?;
    let written = write_secret(secret, label, &seed, made);
    let bundle = written.map(|()| SigningBundle::developer(&seed, label));
    seed.fill(0);
    bundle
}

/// A raw key file (the form before 2026-11-10: the 32-byte seed, or ferros-mkimg's 64-byte seed then public key) written in the VSF form to the new file `secret` under `label`; the raw file is left as it was. Gives the Ed25519 public key, the only thing said of the key.
pub fn convert(raw: &Path, label: &str, secret: &Path, made: i64) -> Result<[u8; 32], String> {
    let mut old = std::fs::read(raw).map_err(|e| format!("{}: {e}", raw.display()))?;
    let converted = convert_bytes(&old, label, secret, made).map_err(|e| format!("{}: {e}", raw.display()));
    old.fill(0);
    converted
}

fn convert_bytes(old: &[u8], label: &str, secret: &Path, made: i64) -> Result<[u8; 32], String> {
    if old.starts_with("RÅ<".as_bytes()) {
        return Err("already a VSF document: nothing to convert".into());
    }
    // WHY: the file is outside input; only the two raw forms hold a seed in their first 32 bytes, and the slices below are in bounds for exactly those lengths.
    if old.len() != 32 && old.len() != 64 {
        return Err(format!("G#{:X} bytes: a raw key is a 32-byte seed or a 64-byte seed and public key", old.len()));
    }
    let mut seed: [u8; 32] = old[..32].try_into().unwrap();
    let public = fgtw::keys::Keypair::from_seed(&seed).public.to_bytes();
    // The 64-byte form's second half is its seed's public key (ed25519-compact's layout); anything else is no keypair, and nothing is written.
    if old.len() == 64 && old[32..] != public {
        seed.fill(0);
        return Err("its second half is not its seed's public key: not a keypair, nothing written".into());
    }
    let written = write_secret(secret, label, &seed, made);
    seed.fill(0);
    written.map(|()| public)
}

/// The key from its VSF secret: the seed and the label it was made under, the bundle derived from them.
pub fn read_secret(secret: &Path) -> Result<(String, SigningBundle), String> {
    let (label, mut seed) = read_doc(secret)?;
    let bundle = SigningBundle::developer(&seed, &label);
    seed.fill(0);
    Ok((label, bundle))
}

/// The public record for `r`, signed by every egg of `key`.
pub fn record(r: &Record, key: &SigningBundle) -> Result<Vec<u8>, String> {
    let public = key.public();
    let mut s = VsfSection::new("developer");
    s.add_field("label", VsfType::a(vsf::Ascii::new(&r.label).map_err(|e| format!("label: {e}"))?));
    s.add_field("name", VsfType::x(r.name.clone()));
    if let Some(c) = &r.contact {
        s.add_field("contact", VsfType::x(c.clone()));
    }
    if let Some(w) = &r.web {
        s.add_field("web", VsfType::x(w.clone()));
    }
    s.add_field("kind", VsfType::a(vsf::Ascii::new(&r.kind).map_err(|e| format!("kind: {e}"))?));
    s.add_field("created", VsfType::e(vsf::types::EtType::e6(r.created)));
    s.add_field("revision", VsfType::u(r.revision as usize, false));
    s.add_field("ed25519", VsfType::ke(public.ed25519().to_vec()));
    s.add_field("falcon512", VsfType::kvf(public.pubkey(scheme::FALCON512).ok_or("no Falcon-512 key")?.to_vec()));
    s.add_field("sphincs", VsfType::kvs(public.pubkey(scheme::SPHINCS_PLUS).ok_or("no SPHINCS+ key")?.to_vec()));
    let unsigned = vsf::VsfBuilder::new().creation_time_oscillations(r.created).signed_only_eggs(VsfType::ke(public.ed25519().to_vec()), &fgtw::release::release_slots()).add_section_direct(s).build()?;
    fgtw::release::sign_release(unsigned, key)
}

/// A record read and checked: the three keys rebuilt into a bundle, and every egg verified against them (it signs itself, so a record proves its holder had the key).
pub fn read_record(doc: &[u8]) -> Result<Record, String> {
    let s = section(doc, "developer")?;
    let first = |name: &str| s.get_field(name).and_then(|f| f.values.first().cloned());
    let text = |name: &str| match first(name) {
        Some(VsfType::x(t)) => Some(t),
        _ => None,
    };
    let ascii = |name: &str| match first(name) {
        Some(VsfType::a(t)) => Some(String::from(t.as_str())),
        _ => None,
    };
    let ed: [u8; 32] = match first("ed25519") {
        Some(VsfType::ke(k)) => k[..].try_into().map_err(|_| "ed25519 is not 32 bytes")?,
        _ => return Err("no ed25519 key".into()),
    };
    let falcon = match first("falcon512") {
        Some(VsfType::kvf(k)) => k,
        _ => return Err("no Falcon-512 key".into()),
    };
    let sphincs = match first("sphincs") {
        Some(VsfType::kvs(k)) => k,
        _ => return Err("no SPHINCS+ key".into()),
    };
    let bundle = fgtw::pq::KeyBundle::new(vec![(scheme::ED25519, ed.to_vec()), (scheme::FALCON512, falcon), (scheme::SPHINCS_PLUS, sphincs)])?;
    fgtw::release::verify_signed_by(doc, &ed, &bundle)?;
    let created = match first("created") {
        Some(VsfType::e(vsf::types::EtType::e6(t))) => t,
        _ => return Err("no created".into()),
    };
    let revision = match first("revision") {
        Some(v) => v.as_u64().ok_or("revision is not a number")?,
        None => return Err("no revision".into()),
    };
    Ok(Record { label: ascii("label").ok_or("no label")?, name: text("name").ok_or("no name")?, contact: text("contact"), web: text("web"), kind: ascii("kind").ok_or("no kind")?, created, revision, ed25519: ed })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own under the system's temporary directory for one test's key files (test seeds only).
    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("kistcat-devkey-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// ferros-mkimg's old 64-byte form (seed then public key) converts to the VSF form, owner-only, the same key under the label; the raw file is untouched, a second conversion to the same file is refused, and the reader refuses the raw file naming the conversion.
    #[test]
    fn a_raw_keypair_converts_to_the_same_key() {
        let d = dir("pair");
        let seed = [7u8; 32];
        let public = fgtw::keys::Keypair::from_seed(&seed).public.to_bytes();
        let raw = d.join("ferros.secret");
        std::fs::write(&raw, [&seed[..], &public[..]].concat()).unwrap();
        let out = d.join("ferros.key.vsf");
        assert_eq!(convert(&raw, ferros_kist::secret::FERROS_LABEL, &out, 1).unwrap(), public);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&out).unwrap().permissions().mode() & 0o777, 0o600);
        let (label, bundle) = read_secret(&out).unwrap();
        assert_eq!(label, ferros_kist::secret::FERROS_LABEL);
        assert_eq!(bundle.public().ed25519(), public);
        assert_eq!(bundle.public().commit(), SigningBundle::developer(&seed, ferros_kist::secret::FERROS_LABEL).public().commit());
        assert_eq!(std::fs::read(&raw).unwrap().len(), 64);
        assert!(convert(&raw, ferros_kist::secret::FERROS_LABEL, &out, 1).is_err(), "an existing key is never overwritten");
        assert!(read_secret(&raw).err().unwrap().contains("devkey convert"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A 32-byte seed converts; a 64-byte file whose halves are not a keypair, a VSF file and any other length are refused, and nothing is written.
    #[test]
    fn a_raw_seed_converts_and_the_rest_is_refused() {
        let d = dir("seed");
        let raw = d.join("seed");
        std::fs::write(&raw, [9u8; 32]).unwrap();
        let out = d.join("seed.key.vsf");
        convert(&raw, "TEST_RELEASE", &out, 1).unwrap();
        assert_eq!(read_secret(&out).unwrap().0, "TEST_RELEASE");
        let not_pair = d.join("not-pair");
        std::fs::write(&not_pair, [3u8; 64]).unwrap();
        assert!(convert(&not_pair, "X", &d.join("a.key.vsf"), 1).is_err());
        assert!(!d.join("a.key.vsf").exists());
        assert!(convert(&out, "X", &d.join("b.key.vsf"), 1).unwrap_err().contains("already"));
        let odd = d.join("odd");
        std::fs::write(&odd, [3u8; 48]).unwrap();
        assert!(convert(&odd, "X", &d.join("c.key.vsf"), 1).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
