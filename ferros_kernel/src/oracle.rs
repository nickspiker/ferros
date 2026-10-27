//! The gate and the oracle (ORACLE.md §5-6): a capsule only spawns after its signature proves this exact content, and only then does the oracle compute the process's *ihi*.
//!
//! The gate: the capsule is a VSF file (toka's capsule format) carrying the developer's Ed25519 key (`ke`) and a signature (`ge`) over the BLAKE3 of the whole file with `ge` zeroed — the code, its provenance hash `hp` and every header field. `vsf::verification::verify_file_signature` (the no_std `crypto-verify` path, the same code the host uses) must pass, or the oracle is never called and spawn never completes.
//!
//! The oracle: `ihi = spaghettify(system_persistent_secret || developer_pubkey || option)`. The developer_pubkey reaches it only from a capsule that survived the gate; `option` is the developer's semantic slot — here the capsule's `hp`, the identity that stays stable across versions (ORACLE.md §5).
//!
//! Not yet to spec: the system persistent secret should live in a write-only CSR the hardware never lets anything read back. On husky it is derived from the ira and held in RAM, a stand-in like the ira itself (PIPE / Glyph silicon closes it).
//!
//! Capsule sections: `toka` (bytecode, for the toka VM) or `aarch64` (field `code`, a binary blob of position-independent native code: dev payloads during bring-up, before the one backend compiles everything from toka).

use alloc::vec::Vec;
use core::cell::UnsafeCell;

struct Secret(UnsafeCell<Option<[u8; 32]>>);
// Single core, never touched from the IRQ handler.
unsafe impl Sync for Secret {}
static SECRET: Secret = Secret(UnsafeCell::new(None));

const SECRET_CONTEXT: &str = "ferros.system-persistent-secret.husky.v1";

/// Derive the system persistent secret from the ira (called once the ira exists).
pub fn set_system_secret(ira: &[u8; 32]) {
    unsafe { *SECRET.0.get() = Some(blake3::derive_key(SECRET_CONTEXT, ira)) };
}

/// A capsule that passed the gate.
pub struct Gated {
    pub code: Vec<u8>,
    pub developer_pubkey: [u8; 32],
    pub hp: [u8; 32],
    /// True if the code came from a `masked` section (sandboxed, SANDBOX.md): the kernel runs the checker on it. A plain `aarch64` section is native dev code, spawned unchecked (a production build refuses it).
    pub masked: bool,
    /// For masked code: how many bytes from the start are instructions (`.text`); the rest is read-only data. The checker covers exactly this span, and W^X makes everything after it non-executable.
    pub text_len: usize,
}

/// What the gate learned about a capsule whose code stays where it was staged (no copy): the developer's key and the provenance hash for the ihi.
pub struct Located {
    pub developer_pubkey: [u8; 32],
    pub hp: [u8; 32],
}

/// The gate over a capsule already in an arc, too large to copy (a std process: Photon is 100 MB of image): the signature verifies over the whole file as always; the code is the byte-vector value the caller names by offset and length, checked to be framed exactly so (`v` `b` and the EWE bit length right before it). Native capsules only — a masked one takes `gate`, whose checker needs the code on its own — and the signature covers every byte, so a wrong location runs the developer's own bytes, nothing else.
pub fn gate_located(capsule: &mut [u8], code_offset: usize, code_len: usize) -> Result<Located, &'static str> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use vsf::file_format::VsfHeader;
    use vsf::types::VsfType;
    // The signature, verified over the file with its own 64 bytes zeroed — done in place (zero, hash, verify, put them back) where vsf's verifier would copy the file first; a 100 MB copy is more than the kernel heap holds (the first Photon spawn died there, 2026-10-27).
    let (header, header_end) = VsfHeader::decode(capsule).map_err(|_| "gate: bad header")?;
    let sig: [u8; 64] = match &header.signature {
        Some(VsfType::ge(s)) if s.len() == 64 => s[..].try_into().unwrap(),
        Some(_) => return Err("gate: only an Ed25519-signed capsule streams; a multi-scheme one takes the staging slot"),
        None => return Err("gate: not a signed capsule"),
    };
    if sig.iter().all(|&b| b == 0) {
        return Err("gate: the capsule is not signed");
    }
    let pubkey: [u8; 32] = match &header.signer_pubkey {
        Some(VsfType::ke(k)) if k.len() == 32 => k[..].try_into().unwrap(),
        _ => return Err("gate: no Ed25519 developer key"),
    };
    let hdr_len = header_end.min(capsule.len());
    let at = (0..=hdr_len.saturating_sub(64)).find(|&i| capsule[i..i + 64] == sig).ok_or("gate: the signature is not in the header")?;
    capsule[at..at + 64].fill(0);
    let hash = blake3::hash(capsule);
    capsule[at..at + 64].copy_from_slice(&sig);
    let key = VerifyingKey::from_bytes(&pubkey).map_err(|_| "gate: bad developer key")?;
    key.verify(hash.as_bytes(), &Signature::from_bytes(&sig)).map_err(|_| "gate: signature does not verify")?;
    if code_len == 0 || code_offset < 3 || code_offset + code_len > capsule.len() {
        return Err("gate: the code is not inside the capsule");
    }
    let framed = (2..=12).any(|back| {
        let p = code_offset.wrapping_sub(back);
        if p + 2 > capsule.len() || capsule[p] != b'v' || capsule[p + 1] != b'b' {
            return false;
        }
        let mut q = p + 2;
        matches!(vsf::ewe::read_uint::<u64>(capsule, &mut q), Ok(bits) if bits == (code_len as u64) << 3 && q == code_offset)
    });
    if !framed {
        return Err("gate: the named code is not a byte-vector value of that length");
    }
    let hp: [u8; 32] = match &header.provenance_hash {
        VsfType::hp(h) if h.len() == 32 => h[..].try_into().unwrap(),
        _ => return Err("gate: no provenance hash"),
    };
    Ok(Located { developer_pubkey: pubkey, hp })
}

/// The gate. Ok only if the capsule's Ed25519 signature verifies over this exact file.
pub fn gate(capsule: &[u8]) -> Result<Gated, &'static str> {
    use vsf::file_format::VsfHeader;
    use vsf::types::VsfType;
    match vsf::verification::verify_file_signature(capsule) {
        Ok(true) => {}
        Ok(false) => return Err("gate: signature does not verify"),
        Err(_) => return Err("gate: not a signed capsule"),
    }
    let (header, header_end) = VsfHeader::decode(capsule).map_err(|_| "gate: bad header")?;
    let developer_pubkey: [u8; 32] = match &header.signer_pubkey {
        Some(VsfType::ke(k)) if k.len() == 32 => k[..].try_into().unwrap(),
        _ => return Err("gate: no Ed25519 developer key"),
    };
    let hp: [u8; 32] = match &header.provenance_hash {
        VsfType::hp(h) if h.len() == 32 => h[..].try_into().unwrap(),
        _ => return Err("gate: no provenance hash"),
    };
    let sections = header.sections(capsule, header_end).map_err(|_| "gate: bad sections")?;
    let find = |name: &str| {
        sections
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.get_field("code"))
            .and_then(|f| f.values.first())
            .and_then(|v| match v {
                VsfType::v(b'b', bytes) => Some(bytes.clone()),
                _ => None,
            })
    };
    let (code, masked) = match find("masked") {
        Some(c) => (c, true),
        None => (find("aarch64").ok_or("gate: no code section")?, false),
    };
    let text_len = if masked {
        let t = sections
            .iter()
            .find(|s| s.name == "masked")
            .and_then(|s| s.get_field("text"))
            .and_then(|f| f.values.first())
            .and_then(|v| match v {
                // VSF integers are auto-sized: whatever width the encoder picked comes back.
                VsfType::u(n, _) => Some(*n),
                VsfType::u3(n) => Some(*n as usize),
                VsfType::u4(n) => Some(*n as usize),
                VsfType::u5(n) => Some(*n as usize),
                VsfType::u6(n) => Some(*n as usize),
                _ => None,
            })
            .ok_or("gate: masked code without a text length")?;
        if t == 0 || t > code.len() || t % 4 != 0 {
            return Err("gate: bad text length");
        }
        t
    } else {
        code.len()
    };
    Ok(Gated { code, developer_pubkey, hp, masked, text_len })
}

/// The OS-only key that seals installed images (SANDBOX.md): derived from the system persistent secret, so nothing outside this OS on this device can produce a seal.
pub fn seal_key() -> Option<[u8; 32]> {
    let secret = unsafe { (*SECRET.0.get())? };
    Some(blake3::derive_key("ferros.seal.v1", &secret))
}

/// The seal of a gated capsule: keyed BLAKE3 over who signed it, its provenance, whether it is masked, its instruction span and the exact image.
pub fn seal(g: &Gated) -> Option<[u8; 32]> {
    let mut h = blake3::Hasher::new_keyed(&seal_key()?);
    h.update(&g.developer_pubkey);
    h.update(&g.hp);
    h.update(&[g.masked as u8]);
    h.update(&(g.text_len as u64).to_le_bytes());
    h.update(&g.code);
    Some(*h.finalize().as_bytes())
}

/// The oracle. None until the system persistent secret exists (no ira, no ihi).
pub fn ihi(developer_pubkey: &[u8; 32], option: &[u8]) -> Option<[u8; 32]> {
    let secret = unsafe { (*SECRET.0.get())? };
    let mut input = Vec::with_capacity(64 + option.len());
    input.extend_from_slice(&secret);
    input.extend_from_slice(developer_pubkey);
    input.extend_from_slice(option);
    Some(ihi::spaghettify(&input))
}
