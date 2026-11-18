//! The seal of an installed masked image (SANDBOX.md "Seal"), its two forms, and the move from the old one to the new: pure, the key handed in (the kernel's OS-only seal key, oracle.rs `seal_key`), host-tested here.
//!
//! - **By image** (the form until 2026-11-17): keyed BLAKE3 over the developer key, the provenance (the kist's option, or the capsule's hp), the masked flag, the modes, the instruction span, then every byte of the code. A launch hashed the whole image again to check it: 92 MB for Photon, on the kernel's core.
//! - **By address** (owner, 2026-11-17: "worth doing. do it."): the same fields, a tag first, and the image replaced by the capsule's address, its BLAKE3, which every launch computes and checks anyway (a kist's as the bytes land, ferros_kist::twin). The address names every byte of the capsule, code included, so the seal binds the same image; checking it costs no pass of its own.
//! - **Nothing runs on an address seal unless that address was computed from the bytes on that same launch**: the callers hand in only an address they computed or checked against the bytes they are about to run (`check`'s `address`).
//! - **The move**: a launch looks for the address seal first; only when it is absent does it compute the image seal (the old full pass) and, when that one is listed, the image runs and the pair is queued (`Verdict::ByImage`). The list is rewritten in two steps (`with_new`, then `without_old` once the new one has read back), so a crash anywhere leaves the old seal listed until the new one is, and the image runnable either way.

use alloc::vec::Vec;

/// What tells the two forms apart: the address form begins with it, the image form with the developer key.
const ADDRESS_TAG: &[u8] = b"ferros seal by address v0";

/// What the checker passed, as the `masked` section gives it.
pub struct Modes {
    pub text_len: usize,
    pub fixed: u32,
    pub bti: bool,
    pub exact: bool,
}

/// The fields both forms cover, in the order the image form always had them.
fn fields(h: &mut blake3::Hasher, developer: &[u8; 32], option: &[u8; 32], masked: bool, m: &Modes) {
    h.update(developer);
    h.update(option);
    h.update(&[masked as u8]);
    // Fixed mode's size is part of what was checked, so it is part of the seal (absent for ring, so ring seals are unchanged).
    if m.fixed != 0 {
        h.update(b"fixed");
        h.update(&[m.fixed as u8]);
    }
    if m.bti {
        h.update(b"bti");
    }
    if m.exact {
        h.update(b"exact");
    }
    h.update(&(m.text_len as u64).to_le_bytes());
}

/// The image form: the fields, then the code.
pub fn by_image(key: &[u8; 32], developer: &[u8; 32], option: &[u8; 32], masked: bool, m: &Modes, code: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(key);
    fields(&mut h, developer, option, masked, m);
    h.update(code);
    *h.finalize().as_bytes()
}

/// The address form: the tag, the fields, then the capsule's address (its BLAKE3, computed from the bytes on this launch).
pub fn by_address(key: &[u8; 32], developer: &[u8; 32], option: &[u8; 32], masked: bool, m: &Modes, address: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(key);
    h.update(ADDRESS_TAG);
    fields(&mut h, developer, option, masked, m);
    h.update(address);
    *h.finalize().as_bytes()
}

/// How an image is found sealed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Its address seal is listed.
    ByAddress,
    /// Only its image seal is listed: it runs, and (old, new) is to be moved (`with_new`, `without_old`).
    ByImage([u8; 32], [u8; 32]),
    /// Neither: not installed on this device.
    No,
}

fn listed(list: &[u8], s: &[u8; 32]) -> bool {
    list.chunks_exact(32).any(|e| e == s)
}

/// The image checked against the list of seals (32 bytes each): its address seal first; the image seal (`image`, the full pass) only when that is absent.
pub fn check(list: &[u8], key: &[u8; 32], developer: &[u8; 32], option: &[u8; 32], masked: bool, m: &Modes, address: &[u8; 32], image: impl FnOnce() -> [u8; 32]) -> Verdict {
    let new = by_address(key, developer, option, masked, m, address);
    if listed(list, &new) {
        return Verdict::ByAddress;
    }
    let old = image();
    if listed(list, &old) { Verdict::ByImage(old, new) } else { Verdict::No }
}

/// The move's first write: the list with `new` added (the old seal kept). None when `old` is no longer listed (a release or another move took it): nothing to move.
pub fn with_new(list: &[u8], old: &[u8; 32], new: &[u8; 32]) -> Option<Vec<u8>> {
    if !listed(list, old) {
        return None;
    }
    let mut l = list.to_vec();
    if !listed(list, new) {
        l.extend_from_slice(new);
    }
    Some(l)
}

/// The move's second write, made only from a list read back from the vault that holds `new`: that list without `old`. None while `new` is not in it (the first write did not land: the old seal stays).
pub fn without_old(list: &[u8], old: &[u8; 32], new: &[u8; 32]) -> Option<Vec<u8>> {
    if !listed(list, new) {
        return None;
    }
    Some(list.chunks_exact(32).filter(|e| *e != old).flatten().copied().collect())
}

/// The list as the kernel's vault keeps it (`sealed`; docs/VSF-AUDIT.md P12, 2026-11-17): a VSF document, section `seals`, a `seal` field per installed image, each its 32-byte seal as `mb` (a keyed BLAKE3, under the OS-only seal key).
/// Until 2026-11-17 the vault kept the seals as bare bytes, 32 each, concatenated, which is still how the checks here take the list in memory.
const SECTION: &str = "seals";
const SEAL: &str = "seal";

fn list_schema() -> vsf::schema::SectionSchema {
    // `mb` has no constraint of its own in vsf's schema; every value is held to it on the way out (`list_of`).
    vsf::schema::SectionSchema::new(SECTION).field(SEAL, vsf::schema::TypeConstraint::Any)
}

/// The list's document (`now`: Eagle time in oscillations, 0 when the clock is not known). None when the bytes are not whole seals.
pub fn list_document(list: &[u8], now: i64) -> Option<Vec<u8>> {
    if list.len() % 32 != 0 {
        return None;
    }
    let mut b = list_schema().build();
    for s in list.chunks_exact(32) {
        b = b.append_multi(SEAL, alloc::vec![vsf::VsfType::mb(s.to_vec())]).ok()?;
    }
    let section = b.encode().ok()?;
    vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_unboxed(SECTION, section).build().ok()
}

/// The seals out of the list's document, concatenated; None for anything that is not one, which a caller must never take for an empty list (a seal added over it would drop every other).
pub fn list_of(doc: &[u8]) -> Option<Vec<u8>> {
    let s = vsf::schema::SectionBuilder::parse_document(list_schema(), doc, None).ok()?;
    let mut out = Vec::new();
    for v in s.get_fields(SEAL).iter().flat_map(|f| f.values.iter()) {
        match v {
            vsf::VsfType::mb(b) if b.len() == 32 => out.extend_from_slice(b),
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const KEY: [u8; 32] = [7; 32];
    const DEV: [u8; 32] = [1; 32];
    const OPT: [u8; 32] = [2; 32];
    const M: Modes = Modes { text_len: 4096, fixed: 0, bti: true, exact: true };

    fn image() -> Vec<u8> {
        (0..10_000u32).map(|i| (i * 31) as u8).collect()
    }

    #[test]
    fn the_image_form_is_the_kernels_old_one_byte_for_byte() {
        // oracle.rs `seal_parts` before 2026-11-17, written out.
        let code = image();
        let mut h = blake3::Hasher::new_keyed(&KEY);
        h.update(&DEV);
        h.update(&OPT);
        h.update(&[1]);
        h.update(b"bti");
        h.update(b"exact");
        h.update(&4096u64.to_le_bytes());
        h.update(&code);
        assert_eq!(by_image(&KEY, &DEV, &OPT, true, &M, &code), *h.finalize().as_bytes());
    }

    #[test]
    fn the_address_form_binds_every_field_and_the_address() {
        let addr = *blake3::hash(&image()).as_bytes();
        let s = by_address(&KEY, &DEV, &OPT, true, &M, &addr);
        assert_ne!(s, by_image(&KEY, &DEV, &OPT, true, &M, &image()));
        let mut other = addr;
        other[0] ^= 1;
        assert_ne!(s, by_address(&KEY, &DEV, &OPT, true, &M, &other));
        assert_ne!(s, by_address(&[8; 32], &DEV, &OPT, true, &M, &addr));
        assert_ne!(s, by_address(&KEY, &OPT, &DEV, true, &M, &addr));
        assert_ne!(s, by_address(&KEY, &DEV, &OPT, false, &M, &addr));
        assert_ne!(s, by_address(&KEY, &DEV, &OPT, true, &Modes { text_len: 4092, ..M }, &addr));
        assert_ne!(s, by_address(&KEY, &DEV, &OPT, true, &Modes { bti: false, exact: false, ..M }, &addr));
        assert_ne!(s, by_address(&KEY, &DEV, &OPT, true, &Modes { fixed: 20, bti: true, exact: false, text_len: 4096 }, &addr));
    }

    #[test]
    fn an_old_seal_runs_once_by_image_then_moves_and_the_old_goes_only_after_the_new_reads_back() {
        let code = image();
        let addr = *blake3::hash(&code).as_bytes();
        let old = by_image(&KEY, &DEV, &OPT, true, &M, &code);
        let other = [9u8; 32];
        let list: Vec<u8> = [other, old].concat();
        // Not moved yet: the image pass runs, the image is sealed by it.
        let mut passes = 0;
        let v = check(&list, &KEY, &DEV, &OPT, true, &M, &addr, || {
            passes += 1;
            old
        });
        let Verdict::ByImage(o, n) = v else { panic!("{v:?}") };
        assert_eq!((o, passes), (old, 1));
        // A crash before the first write: the old seal still runs it.
        assert!(matches!(check(&list, &KEY, &DEV, &OPT, true, &M, &addr, || old), Verdict::ByImage(..)));
        // The first write keeps the old; a crash now leaves both, and the address seal answers without the image pass.
        let both = with_new(&list, &o, &n).unwrap();
        assert!(both.chunks_exact(32).any(|e| e == old) && both.chunks_exact(32).any(|e| e == n));
        let mut passes = 0;
        assert_eq!(
            check(&both, &KEY, &DEV, &OPT, true, &M, &addr, || {
                passes += 1;
                old
            }),
            Verdict::ByAddress
        );
        assert_eq!(passes, 0, "an address seal needs no image pass");
        // The old goes only from a list that holds the new.
        assert_eq!(without_old(&list, &o, &n), None);
        let after = without_old(&both, &o, &n).unwrap();
        assert_eq!(after, [other, n].concat());
        assert_eq!(check(&after, &KEY, &DEV, &OPT, true, &M, &addr, || old), Verdict::ByAddress);
        // Moving again finds no old seal: nothing to do.
        assert_eq!(with_new(&after, &o, &n), None);
    }

    #[test]
    fn a_different_image_under_the_same_address_claim_is_not_sealed() {
        let code = image();
        let addr = *blake3::hash(&code).as_bytes();
        let list = by_address(&KEY, &DEV, &OPT, true, &M, &addr).to_vec();
        let mut changed = code.clone();
        changed[5000] ^= 1;
        // The caller computes the address from the bytes it runs: a changed image has another address, and neither form is listed for it.
        let addr2 = *blake3::hash(&changed).as_bytes();
        assert_eq!(check(&list, &KEY, &DEV, &OPT, true, &M, &addr2, || by_image(&KEY, &DEV, &OPT, true, &M, &changed)), Verdict::No);
        let _ = vec![0u8];
    }

    const NOW: i64 = 1 << 50;

    #[test]
    fn the_list_document_reads_back() {
        let list: Vec<u8> = [[1u8; 32], [2; 32], [3; 32]].concat();
        assert_eq!(list_of(&list_document(&list, NOW).unwrap()).unwrap(), list);
        assert_eq!(list_of(&list_document(&[], NOW).unwrap()).unwrap(), Vec::<u8>::new());
        assert!(list_document(&[1; 33], NOW).is_none());
    }

    #[test]
    fn every_image_on_a_kept_list_still_runs() {
        // An address seal, an image seal not moved yet, another kist's address seal.
        let code = image();
        let addr = *blake3::hash(&code).as_bytes();
        let moved = by_address(&KEY, &DEV, &OPT, true, &M, &addr);
        let other_code: Vec<u8> = (0..4096u32).map(|i| (i * 13) as u8).collect();
        let unmoved = by_image(&KEY, &OPT, &DEV, true, &M, &other_code);
        let third = by_address(&KEY, &[5; 32], &[6; 32], false, &Modes { text_len: 64, fixed: 20, bti: false, exact: false }, &[7; 32]);
        let old: Vec<u8> = [moved, unmoved, third].concat();
        let doc = list_document(&old, NOW).unwrap();
        let list = list_of(&doc).unwrap();
        assert_eq!(list, old, "the seals keep their order and bytes");
        assert_eq!(check(&list, &KEY, &DEV, &OPT, true, &M, &addr, || unreachable!()), Verdict::ByAddress);
        let other_addr = *blake3::hash(&other_code).as_bytes();
        assert!(matches!(check(&list, &KEY, &OPT, &DEV, true, &M, &other_addr, || unmoved), Verdict::ByImage(..)));
        assert_eq!(check(&list, &KEY, &[5; 32], &[6; 32], false, &Modes { text_len: 64, fixed: 20, bti: false, exact: false }, &[7; 32], || [0; 32]), Verdict::ByAddress);
        // Bare seals are not a list document: the reader never takes them for one, empty or not.
        assert_eq!(list_of(&old), None);
        assert_eq!(list_of(&[]), None);
        assert!(list_document(&old[..95], NOW).is_none());
    }
}
