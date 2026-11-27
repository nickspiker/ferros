//! A key worn by the person present as well as this device (SYS_IRA x4 bit 0; owner, 2026-11-09: "We mix the ira in with our identity. can't have same address devices on network"): the WiFi address per network (pcie1, radio_addresses_belong_to_identity), anything a system process asks for that must differ per person and per device and not exist while locked.
//! The kernel derives both halves where the keys are held: the device's half from the ira (`ira::derive`), the person's half from their domain key in registers under a context of its own (`identity_domain_keys`, never the base itself: keys_in_registers, b261a07b), and joins them here.

/// The context the two halves are joined under (unchanged since 2026-11-09).
pub const CONTEXT: &str = "ferros 2026-11-09 person ira";

/// The worn key: None while nobody is present (no person's half), else `derive_key(CONTEXT, person ‖ device)`.
pub fn worn(person: Option<&[u8; 32]>, device: &[u8; 32]) -> Option<[u8; 32]> {
    let p = person?;
    let mut h = blake3::Hasher::new_derive_key(CONTEXT);
    h.update(p);
    h.update(device);
    Some(*h.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_person_per_device_none_locked() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let (d0, d1) = ([7u8; 32], [8u8; 32]);
        assert_eq!(worn(None, &d0), None);
        let k = worn(Some(&a), &d0).unwrap();
        assert_ne!(k, worn(Some(&b), &d0).unwrap());
        assert_ne!(k, worn(Some(&a), &d1).unwrap());
        assert_eq!(k, worn(Some(&a), &d0).unwrap());
        assert_ne!(k, d0);
    }
}
