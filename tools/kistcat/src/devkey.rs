//! A developer's key (owner, 2026-11-10): the secret kept as VSF in the keys directory, and the public record that names whose kists it signs.
//! The secret is one `kge`, the Ed25519 seed; the Falcon-512 and SPHINCS+ halves derive from it under the developer's label (fgtw `SigningBundle::developer`), so the label is part of the key and is never changed.
//! The record carries the three public keys, the developer's name, contact and web as `x` (the person's words; owner: "x is for text, ascii is for keys, values, labels, system stuff"), the kind, when it was made and its revision, and is signed by every egg of the key it describes: a newer revision of the same key replaces an older one, so a name or contact can change without the key changing.
//! Later, beside it and not in it: succession to a next key, revocation, a link to the developer's FGTW identity, and the automatic fields (their other kists, download and install counts).

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

/// A new key: 32 bytes from the operating system's random source, written to `secret` (refused if it exists, owner-only permissions) as section `developer_key` { label a, seed kge }.
pub fn new_secret(secret: &Path, label: &str, made: i64) -> Result<SigningBundle, String> {
    let mut seed = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut seed)).map_err(|e| format!("random: {e}"))?;
    let mut s = VsfSection::new("developer_key");
    s.add_field("label", VsfType::a(vsf::Ascii::new(label).map_err(|e| format!("label: {e}"))?));
    s.add_field("seed", VsfType::kge(seed.to_vec()));
    let doc = vsf::VsfBuilder::new().creation_time_oscillations(made).provenance_only().add_section_direct(s).build()?;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(secret).map_err(|e| format!("{}: {e} (an existing key is never overwritten)", secret.display()))?;
    f.write_all(&doc).map_err(|e| format!("{}: {e}", secret.display()))?;
    let bundle = SigningBundle::developer(&seed, label);
    seed.fill(0);
    Ok(bundle)
}

/// The key from its VSF secret: the seed and the label it was made under, the bundle derived from them.
pub fn read_secret(secret: &Path) -> Result<(String, SigningBundle), String> {
    let doc = std::fs::read(secret).map_err(|e| format!("{}: {e}", secret.display()))?;
    let s = section(&doc, "developer_key").map_err(|e| format!("{}: {e}", secret.display()))?;
    let label = s.get_field("label").and_then(|f| f.values.iter().find_map(|v| if let VsfType::a(t) = v { Some(String::from(t.as_str())) } else { None })).ok_or("no label")?;
    let mut seed: [u8; 32] = s.get_field("seed").and_then(|f| f.values.iter().find_map(|v| if let VsfType::kge(k) = v { k[..].try_into().ok() } else { None })).ok_or("no 32-byte kge seed")?;
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
