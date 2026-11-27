//! Encrypt → decrypt returns the plaintext, and advance moves the key (forward secrecy). Proves internal correctness; byte-compatibility with the stock app is verified on device.

use photon_core::chain::*;

fn test_chain() -> Chain {
    // A deterministic 8 KiB active portion.
    let mut active = [0u8; ACTIVE_LINKS * LINK_SIZE];
    for (i, b) in active.iter_mut().enumerate() {
        *b = (i * 7 + 3) as u8;
    }
    Chain::from_bytes(&active).expect("8 KiB active portion")
}

#[test]
fn encrypt_decrypt_round_trip() {
    let chain = test_chain();
    let t = 1_420_000_000i64;
    let plaintext = b"the first photon: a VSF message body of arbitrary length \x00\x01\x02";
    let salt = derive_salt(b"", &chain);
    let scratch = generate_scratch(&chain, &salt);
    let ct = encrypt_layers(plaintext, &chain, &scratch, t);
    assert_ne!(&ct[..], &plaintext[..], "ciphertext differs from plaintext");
    // Receiver at the same key (index 511), same salt/scratch, same time.
    let pt = decrypt_layers(&ct, &chain, CURRENT_KEY_INDEX, &scratch, t);
    assert_eq!(&pt[..], &plaintext[..], "decrypt recovers the plaintext");
}

#[test]
fn advance_changes_key_and_history() {
    let mut chain = test_chain();
    let before = *chain.current_key();
    let link510 = *chain.link(510).unwrap();
    chain.advance(1_420_000_001, b"our first", &[]);
    assert_ne!(*chain.current_key(), before, "advance derives a new current key");
    // Left shift: old [511] is gone, old [510] moved to [509].
    assert_eq!(*chain.link(509).unwrap(), link510, "links shift left by one");
    // A different plaintext gives a different fresh link (entropy flows in).
    let mut other = test_chain();
    other.advance(1_420_000_001, b"our SECOND", &[]);
    assert_ne!(*chain.current_key(), *other.current_key());
}

#[test]
fn salt_chains_to_previous_plaintext() {
    let chain = test_chain();
    let s0 = derive_salt(b"", &chain);
    let s1 = derive_salt(b"previous message", &chain);
    assert_ne!(s0, s1, "the salt binds to the previous plaintext (message ordering)");
}

#[test]
fn full_bytes_round_trip() {
    let chain = test_chain();
    let snap = chain.to_bytes();
    assert_eq!(snap.len(), CHAIN_LINKS * LINK_SIZE);
    let back = Chain::from_full_bytes(&snap).unwrap();
    assert_eq!(back.current_key(), chain.current_key());
}
