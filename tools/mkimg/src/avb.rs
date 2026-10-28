//! Android Verified Boot (AVB) for the locked-bootloader path (BOOT.md, "Locked-bootloader endgame"): the owner's RSA key, its public-key blob for `avb_custom_key`, the gate image with its own signed footer, and the slot's top-level `vbmeta` pointing at it. In Rust, not Google's avbtool (owner, 2026-10-28: no Python).
//!
//! The formats are AVB's own (libavb `avb_vbmeta_image.h`, `avb_descriptor.h`, `avb_footer.h`, `avb_crypto.h`), because ABL reads them; they exist only on the boot partitions. All integers are big-endian.
//!
//! How Google's chain is built (read from the factory `vbmeta` 2026-10-28, `vbmeta-info`): the top-level `vbmeta` carries hash descriptors for the bootloader images, `dtbo`, `vendor_boot` and `vendor_kernel_boot`, and *chain* descriptors for `boot`, `init_boot`, `vbmeta_system` and `vbmeta_vendor`. Each of those carries its own signed `vbmeta` (behind an AVB footer at the end of the partition for `boot`/`init_boot`), and the chain descriptor names the key that must have signed it and the rollback-index slot it uses. So ours changes exactly two things: the gate image gets its own footer `vbmeta` (a hash of the gate, with Google's boot properties and rollback index carried over), signed with the owner's key; and the top-level `vbmeta`'s `boot` chain descriptor names the owner's key instead of Google's. Every other descriptor is Google's, byte for byte, and the top level is signed with the owner's key too (it is the key in `avb_custom_key`).
//!
//! The key: RSA-4096, e = 65537, kept as one VSF document (section `avb_key`: `signing` = `kgr`, its primes p ‖ q; `verification` = `kvr`, the modulus), built and read through a schema.

use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{BigUint, Pkcs1v15Sign, RsaPrivateKey};
use sha2::{Digest, Sha256};
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

pub const HEADER: usize = 256;
const FOOTER: usize = 64;
const ALG_SHA256_RSA4096: u32 = 2;
const TAG_PROPERTY: u64 = 0;
const TAG_HASHTREE: u64 = 1;
const TAG_HASH: u64 = 2;
const TAG_CMDLINE: u64 = 3;
const TAG_CHAIN: u64 = 4;

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}
fn be64(b: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(b[at..at + 8].try_into().unwrap())
}
fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_be_bytes());
}
fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_be_bytes());
}
fn pad_to(n: usize, block: usize) -> usize {
    n.div_ceil(block) * block
}

// ==================== the key ====================

const KEY_SECTION: &str = "avb_key";

fn key_schema() -> SectionSchema {
    SectionSchema::new(KEY_SECTION).field("signing", TypeConstraint::RsaSigningKey).field("verification", TypeConstraint::RsaVerificationKey)
}

fn be_fixed(x: &BigUint, len: usize) -> Vec<u8> {
    let v = x.to_bytes_be();
    let mut b = vec![0u8; len - v.len()];
    b.extend_from_slice(&v);
    b
}

/// A fresh RSA-4096 key as a complete VSF document.
#[allow(deprecated)]
pub fn generate() -> Result<(RsaPrivateKey, Vec<u8>), String> {
    let key = RsaPrivateKey::new(&mut rand::rngs::OsRng, 4096).map_err(|e| e.to_string())?;
    let primes = key.primes();
    if primes.len() != 2 {
        return Err("expected two primes".into());
    }
    let mut pq = be_fixed(&primes[0], 256);
    pq.extend_from_slice(&be_fixed(&primes[1], 256));
    let section = key_schema()
        .build()
        .set("signing", VsfType::kgr(pq))
        .and_then(|b| b.set("verification", VsfType::kvr(be_fixed(key.n(), 512))))
        .and_then(|b| b.encode())
        .map_err(|e| format!("{e:?}"))?;
    let doc = vsf::VsfBuilder::new().creation_time_oscillations(vsf::eagle_time_oscillations()).provenance_only().add_unboxed(KEY_SECTION, section).build()?;
    Ok((key, doc))
}

/// The key from its VSF document: provenance verified, the primes rebuilt into a key, and the stored modulus checked against it.
#[allow(deprecated)]
pub fn load(doc: &[u8]) -> Result<RsaPrivateKey, String> {
    let s = SectionBuilder::parse_document(key_schema(), doc, None).map_err(|e| format!("{e:?}"))?;
    let first = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first()).cloned();
    let (Some(VsfType::kgr(pq)), Some(VsfType::kvr(n))) = (first("signing"), first("verification")) else {
        return Err("no signing (kgr) and verification (kvr) key in the document".into());
    };
    if pq.len() != 512 {
        return Err(format!("signing key is {} bytes, not 512 (p ‖ q of RSA-4096)", pq.len()));
    }
    let (p, q) = (BigUint::from_bytes_be(&pq[..256]), BigUint::from_bytes_be(&pq[256..]));
    let key = RsaPrivateKey::from_p_q(p, q, BigUint::from(65537u32)).map_err(|e| e.to_string())?;
    if key.n().bits() != 4096 || be_fixed(key.n(), 512) != n {
        return Err("the primes do not make the stored modulus".into());
    }
    Ok(key)
}

/// AVB's public-key blob (`AvbRSAPublicKeyHeader`: bit count, n0inv = -1/n mod 2^32, then n and R² mod n with R = 2^bits, big-endian): what `avb_custom_key` holds and what a vbmeta or chain descriptor carries.
pub fn public_key_blob(key: &RsaPrivateKey) -> Vec<u8> {
    blob_of(key.n())
}

/// The blob for a modulus (the public half alone).
pub fn blob_of(n: &BigUint) -> Vec<u8> {
    let bits = n.bits();
    let le = n.to_bytes_le();
    let n0 = u32::from_le_bytes([le[0], le[1], le[2], le[3]]);
    // Newton's iteration for the inverse of n's low word mod 2^32 (n is odd): five steps from 1, each doubling the correct bits; then negated.
    let mut inv: u32 = 1;
    for _ in 0..5 {
        inv = inv.wrapping_mul(2u32.wrapping_sub(n0.wrapping_mul(inv)));
    }
    let rr = (BigUint::from(1u8) << (2 * bits)) % n;
    let mut out = vec![0u8; 8];
    put32(&mut out, 0, bits as u32);
    put32(&mut out, 4, inv.wrapping_neg());
    out.extend_from_slice(&be_fixed(n, bits / 8));
    out.extend_from_slice(&be_fixed(&rr, bits / 8));
    out
}

// ==================== reading ====================

/// A parsed `vbmeta` image: the header fields we use and each descriptor whole (tag, length, body).
pub struct Vbmeta {
    pub header: [u8; HEADER],
    pub algorithm: u32,
    pub rollback_index: u64,
    pub flags: u32,
    pub rollback_index_location: u32,
    pub release: String,
    pub descriptors: Vec<Vec<u8>>,
    pub public_key: Vec<u8>,
}

/// The `vbmeta` of an image: at its start (a vbmeta partition), or behind the AVB footer at its end (a partition that carries its own; the footer's recorded image size comes back with it).
pub fn find(img: &[u8]) -> Result<(&[u8], Option<u64>), String> {
    if img.len() >= 4 && &img[0..4] == b"AVB0" {
        return Ok((img, None));
    }
    if img.len() >= FOOTER && &img[img.len() - FOOTER..img.len() - FOOTER + 4] == b"AVBf" {
        let f = &img[img.len() - FOOTER..];
        let (orig, off, size) = (be64(f, 12), be64(f, 20) as usize, be64(f, 28) as usize);
        if off + size > img.len() {
            return Err("footer points past the image".into());
        }
        return Ok((&img[off..off + size], Some(orig)));
    }
    Err("neither a vbmeta image (AVB0) nor an image with an AVB footer (AVBf)".into())
}

pub fn parse(img: &[u8]) -> Result<Vbmeta, String> {
    if img.len() < HEADER || &img[0..4] != b"AVB0" {
        return Err("not a vbmeta image (no AVB0)".into());
    }
    let auth = be64(img, 12) as usize;
    let aux = be64(img, 20) as usize;
    if HEADER + auth + aux > img.len() {
        return Err("blocks run past the image".into());
    }
    let a = HEADER + auth;
    let (pk_off, pk_size) = (be64(img, 64) as usize, be64(img, 72) as usize);
    let (d_off, d_size) = (be64(img, 96) as usize, be64(img, 104) as usize);
    if d_off + d_size > aux || pk_off + pk_size > aux {
        return Err("descriptors or key run past the auxiliary block".into());
    }
    let mut descriptors = Vec::new();
    let mut at = a + d_off;
    let end = a + d_off + d_size;
    while at + 16 <= end {
        let len = 16 + be64(img, at + 8) as usize;
        if at + len > end {
            return Err("descriptor runs past the block".into());
        }
        descriptors.push(img[at..at + len].to_vec());
        at += len;
    }
    Ok(Vbmeta {
        header: img[..HEADER].try_into().unwrap(),
        algorithm: be32(img, 28),
        rollback_index: be64(img, 112),
        flags: be32(img, 120),
        rollback_index_location: be32(img, 124),
        release: String::from_utf8_lossy(&img[128..176]).trim_end_matches('\0').to_string(),
        descriptors,
        public_key: img[a + pk_off..a + pk_off + pk_size].to_vec(),
    })
}

fn signed_bytes(img: &[u8]) -> Sha256 {
    let (auth, aux) = (be64(img, 12) as usize, be64(img, 20) as usize);
    let mut h = Sha256::new();
    h.update(&img[..HEADER]);
    h.update(&img[HEADER + auth..HEADER + auth + aux]);
    h
}

/// Whether the stored hash matches the header and auxiliary block (a flag patched after signing breaks it: ABL's HASH_MISMATCH). SHA-256 images only.
pub fn hash_ok(img: &[u8]) -> Option<bool> {
    if !(1..=3).contains(&be32(img, 28)) {
        return None;
    }
    let (h_off, h_size) = (be64(img, 32) as usize, be64(img, 40) as usize);
    Some(signed_bytes(img).finalize().as_slice() == &img[HEADER + h_off..HEADER + h_off + h_size])
}

/// Whether the signature verifies under the key the image carries, as ABL checks it before comparing that key with the one it trusts.
pub fn signature_ok(img: &[u8]) -> Result<bool, String> {
    let v = parse(img)?;
    if v.algorithm == 0 || v.public_key.len() < 8 {
        return Err("unsigned (algorithm NONE)".into());
    }
    let bits = be32(&v.public_key, 0) as usize;
    let n = BigUint::from_bytes_be(&v.public_key[8..8 + bits / 8]);
    let pk = rsa::RsaPublicKey::new(n, BigUint::from(65537u32)).map_err(|e| e.to_string())?;
    let (s_off, s_size) = (be64(img, 48) as usize, be64(img, 56) as usize);
    Ok(pk.verify(Pkcs1v15Sign::new::<Sha256>(), &signed_bytes(img).finalize(), &img[HEADER + s_off..HEADER + s_off + s_size]).is_ok())
}

fn name_of(d: &[u8]) -> Option<String> {
    let b = &d[16..];
    match be64(d, 0) {
        TAG_HASH => Some(String::from_utf8_lossy(&b[116..116 + be32(b, 40) as usize]).to_string()),
        TAG_CHAIN => Some(String::from_utf8_lossy(&b[76..76 + be32(b, 4) as usize]).to_string()),
        _ => None,
    }
}

/// A hash descriptor's partition, image size, salt and digest.
pub fn hash_of(d: &[u8]) -> Option<(String, u64, Vec<u8>, Vec<u8>)> {
    if be64(d, 0) != TAG_HASH {
        return None;
    }
    let b = &d[16..];
    let (nl, sl, dl) = (be32(b, 40) as usize, be32(b, 44) as usize, be32(b, 48) as usize);
    Some((name_of(d)?, be64(b, 0), b[116 + nl..116 + nl + sl].to_vec(), b[116 + nl + sl..116 + nl + sl + dl].to_vec()))
}

/// A chain descriptor's partition and the key it names.
pub fn chain_of(d: &[u8]) -> Option<(String, Vec<u8>)> {
    if be64(d, 0) != TAG_CHAIN {
        return None;
    }
    let b = &d[16..];
    let (nl, kl) = (be32(b, 4) as usize, be32(b, 8) as usize);
    Some((name_of(d)?, b[76 + nl..76 + nl + kl].to_vec()))
}

/// A descriptor in words, for `vbmeta-info`.
pub fn describe(d: &[u8]) -> String {
    let b = &d[16..];
    match be64(d, 0) {
        TAG_HASH => {
            let (name, size, salt, digest) = hash_of(d).unwrap();
            format!("hash      {name:<20} {size} bytes {} digest {} salt {} B flags {}", String::from_utf8_lossy(&b[8..40]).trim_end_matches('\0'), hex(&digest[..8]), salt.len(), be32(b, 52))
        }
        TAG_CHAIN => {
            let (name, key) = chain_of(d).unwrap();
            format!("chain     {name:<20} rollback location {} key sha256 {} flags {}", be32(b, 0), hex(&Sha256::digest(&key)[..8]), be32(b, 12))
        }
        TAG_PROPERTY => {
            let (kl, vl) = (be64(b, 0) as usize, be64(b, 8) as usize);
            format!("property  {} = {}", String::from_utf8_lossy(&b[16..16 + kl]), String::from_utf8_lossy(&b[16 + kl + 1..16 + kl + 1 + vl]))
        }
        TAG_CMDLINE => format!("cmdline   flags {}: {}", be32(b, 0), String::from_utf8_lossy(&b[8..8 + be32(b, 4) as usize])),
        TAG_HASHTREE => "hashtree".to_string(),
        t => format!("tag {t} ({} bytes)", d.len()),
    }
}

// ==================== writing ====================

fn descriptor(tag: u64, mut body: Vec<u8>) -> Vec<u8> {
    while body.len() % 8 != 0 {
        body.push(0);
    }
    let mut d = vec![0u8; 16];
    put64(&mut d, 0, tag);
    put64(&mut d, 8, body.len() as u64);
    d.extend_from_slice(&body);
    d
}

/// A hash descriptor for `name`: SHA-256 over salt ‖ image, as ABL computes it over the first `image.len()` bytes of the partition.
fn hash_descriptor(name: &str, image: &[u8], salt: &[u8; 32]) -> Vec<u8> {
    let digest = {
        let mut h = Sha256::new();
        h.update(salt);
        h.update(image);
        h.finalize()
    };
    let mut body = vec![0u8; 116];
    put64(&mut body, 0, image.len() as u64);
    body[8..14].copy_from_slice(b"sha256");
    put32(&mut body, 40, name.len() as u32);
    put32(&mut body, 44, 32);
    put32(&mut body, 48, 32);
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(salt);
    body.extend_from_slice(&digest);
    descriptor(TAG_HASH, body)
}

/// A chain descriptor's copy naming another key: rollback location and flags kept.
fn rechain(d: &[u8], key_blob: &[u8]) -> Vec<u8> {
    let b = &d[16..];
    let (loc, nl, flags) = (be32(b, 0), be32(b, 4) as usize, be32(b, 12));
    let mut body = vec![0u8; 76];
    put32(&mut body, 0, loc);
    put32(&mut body, 4, nl as u32);
    put32(&mut body, 8, key_blob.len() as u32);
    put32(&mut body, 12, flags);
    body.extend_from_slice(&b[76..76 + nl]);
    body.extend_from_slice(key_blob);
    descriptor(TAG_CHAIN, body)
}

/// A signed `vbmeta` over `descriptors` (each whole): the libavb version, rollback index and location as given; flags 0 (verification on).
fn sign(descriptors: &[u8], version: &[u8], rollback_index: u64, location: u32, key: &RsaPrivateKey) -> Result<Vec<u8>, String> {
    let pk = public_key_blob(key);
    let mut aux = descriptors.to_vec();
    let pk_off = aux.len();
    aux.extend_from_slice(&pk);
    aux.resize(pad_to(aux.len(), 64), 0);
    let (hash_size, sig_size) = (32usize, 512usize);
    let auth_size = pad_to(hash_size + sig_size, 64);
    let mut h = [0u8; HEADER];
    h[0..4].copy_from_slice(b"AVB0");
    h[4..12].copy_from_slice(version);
    put64(&mut h, 12, auth_size as u64);
    put64(&mut h, 20, aux.len() as u64);
    put32(&mut h, 28, ALG_SHA256_RSA4096);
    put64(&mut h, 32, 0);
    put64(&mut h, 40, hash_size as u64);
    put64(&mut h, 48, hash_size as u64);
    put64(&mut h, 56, sig_size as u64);
    put64(&mut h, 64, pk_off as u64);
    put64(&mut h, 72, pk.len() as u64);
    put64(&mut h, 80, (pk_off + pk.len()) as u64);
    put64(&mut h, 88, 0);
    put64(&mut h, 96, 0);
    put64(&mut h, 104, descriptors.len() as u64);
    put64(&mut h, 112, rollback_index);
    put32(&mut h, 120, 0);
    put32(&mut h, 124, location);
    let release = b"ferros-mkimg 0";
    h[128..128 + release.len()].copy_from_slice(release);
    let digest = {
        let mut s = Sha256::new();
        s.update(h);
        s.update(&aux);
        s.finalize()
    };
    let sig = key.sign(Pkcs1v15Sign::new::<Sha256>(), &digest).map_err(|e| e.to_string())?;
    if sig.len() != sig_size {
        return Err(format!("signature is {} bytes", sig.len()));
    }
    let mut auth = vec![0u8; auth_size];
    auth[..hash_size].copy_from_slice(&digest);
    auth[hash_size..hash_size + sig_size].copy_from_slice(&sig);
    let mut out = h.to_vec();
    out.extend_from_slice(&auth);
    out.extend_from_slice(&aux);
    Ok(out)
}

/// The gate image as a whole boot partition: the image, its signed `vbmeta` at the next 4 KiB boundary (a hash of the image; Google's boot properties, rollback index and location from `google_boot`), zeros, and the AVB footer in the last 64 bytes.
pub fn boot_partition(image: &[u8], partition_size: usize, google_boot: &Vbmeta, key: &RsaPrivateKey, salt: &[u8; 32]) -> Result<Vec<u8>, String> {
    let mut ds = hash_descriptor("boot", image, salt);
    for d in &google_boot.descriptors {
        if be64(d, 0) == TAG_PROPERTY {
            ds.extend_from_slice(d);
        }
    }
    let vb = sign(&ds, &google_boot.header[4..12], google_boot.rollback_index, google_boot.rollback_index_location, key)?;
    let off = pad_to(image.len(), 4096);
    if off + vb.len() + FOOTER > partition_size {
        return Err(format!("image ({} bytes) and vbmeta ({} bytes) do not fit the {partition_size}-byte partition", image.len(), vb.len()));
    }
    let mut out = vec![0u8; partition_size];
    out[..image.len()].copy_from_slice(image);
    out[off..off + vb.len()].copy_from_slice(&vb);
    let f = &mut out[partition_size - FOOTER..];
    f[0..4].copy_from_slice(b"AVBf");
    put32(f, 4, 1);
    put32(f, 8, 0);
    put64(f, 12, image.len() as u64);
    put64(f, 20, off as u64);
    put64(f, 28, vb.len() as u64);
    Ok(out)
}

/// Google's top-level `vbmeta` with its `boot` chain descriptor naming the owner's key, signed with the owner's key: every other descriptor kept byte for byte, the rollback index and its location kept.
pub fn top_vbmeta(google: &Vbmeta, key: &RsaPrivateKey) -> Result<Vec<u8>, String> {
    let blob = public_key_blob(key);
    let mut ds = Vec::new();
    let mut rechained = false;
    for d in &google.descriptors {
        if chain_of(d).is_some_and(|(n, _)| n == "boot") {
            ds.extend_from_slice(&rechain(d, &blob));
            rechained = true;
        } else {
            ds.extend_from_slice(d);
        }
    }
    if !rechained {
        return Err("Google's vbmeta has no chain descriptor for boot".into());
    }
    sign(&ds, &google.header[4..12], google.rollback_index, google.rollback_index_location, key)
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blob is rebuilt from Google's own modulus, byte for byte as Google's avbtool wrote it: n0inv and R² mod n checked against a key ABL already trusts. Needs the factory image (skipped without it).
    #[test]
    fn blob_matches_googles() {
        let Ok(img) = std::fs::read(std::env::var("AVB_GOOGLE_VBMETA").unwrap_or_default()) else {
            return;
        };
        let v = parse(&img).unwrap();
        let bits = be32(&v.public_key, 0) as usize;
        let n = BigUint::from_bytes_be(&v.public_key[8..8 + bits / 8]);
        assert_eq!(blob_of(&n), v.public_key);
    }
}
