//! The handle compared with the person present's (docs/AUTH.md "Remove this device"): the kernel compares the seed of a handle typed just now with the seed it holds for the person present.
//!
//! Until 2026-11-24 this was also the retype gate's (attest at boot turned on by the handle typed again on a second entry screen, SYS_UNLOCK op 12, with a reply document); that screen is gone, replaced by the approve on the attest screen itself (approve.rs).
//! What stays is the compare, which the removal's erase still makes (SYS_UNLOCK op 15): the seed the identity service hands the kernel must be the person present's.
//!
//! The compare is over one-way tags, never the seeds: the kernel derives its side from the seed it holds sealed under the wairua, in registers (`keys::held_derive`, the same `derive_key` as `tag`), so the held seed is never opened into memory for it.
//! Equal tags mean equal seeds (BLAKE3 is collision resistant), and the compare runs in time that does not depend on where they differ (`crate::same`).

/// The context both sides derive their tag under.
pub const CONTEXT: &str = "ferros 2026-11-17 retype gate v0";

/// The tag of a seed: `derive_key(CONTEXT, seed)`, which the kernel makes in registers from the seed it holds.
pub fn tag(seed: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key(CONTEXT, seed)
}

/// Whether the seed typed now is the one `held_tag` was made from, in time that does not depend on where they differ.
pub fn matches(held_tag: &[u8; 32], typed_seed: &[u8; 32]) -> bool {
    let mut t = tag(typed_seed);
    let same = crate::same(held_tag, &t);
    for b in t.iter_mut() {
        // SAFETY: a valid, aligned byte of a local array.
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    same
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// The kernel's side is `derive_key(CONTEXT, held seed ‖ nothing)` (keys::held_derive with an empty message); the blake3 crate's derive_key over the seed alone is the same bytes.
    #[test]
    fn tag_is_derive_key() {
        let seed = [7u8; 32];
        let mut h = blake3::Hasher::new_derive_key(CONTEXT);
        h.update(&seed);
        h.update(&[]);
        assert_eq!(tag(&seed), *h.finalize().as_bytes());
    }

    /// The same handle matches; another handle, and a seed one bit off anywhere, does not.
    #[test]
    fn compare() {
        let mine = crate::identity_seed("Correct Horse").unwrap();
        let held = tag(&mine);
        assert!(matches(&held, &crate::identity_seed("Correct Horse").unwrap()));
        assert!(!matches(&held, &crate::identity_seed("correct horse").unwrap()));
        assert!(!matches(&held, &crate::identity_seed("Correct Horse ").unwrap()));
        for byte in 0..32 {
            for bit in 0..8 {
                let mut s = mine;
                s[byte] ^= 1 << bit;
                assert!(!matches(&held, &s), "byte {byte} bit {bit}");
            }
        }
    }

    /// `same` looks at every byte: a difference at the first or the last is found alike.
    #[test]
    fn same_every_byte() {
        let a = [0x5Au8; 32];
        for at in 0..32 {
            let mut b = a;
            b[at] ^= 0x80;
            assert!(!crate::same(&a, &b));
        }
        assert!(crate::same(&a, &a));
    }
}
