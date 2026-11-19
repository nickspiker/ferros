//! What sealing a clip would cost, on the host (the device's own number is std/rawrec job 3): XChaCha20-Poly1305 in place over 2^8 MiB, a band's worth (about 2^17 bytes coded) a seal, on one core.
//! `cargo test --release --test seal_bench -- --ignored --nocapture`.

use chacha20poly1305::aead::{AeadInPlace, KeyInit};

#[test]
#[ignore]
fn seal_bench() {
    let c = chacha20poly1305::XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&[7u8; 32]));
    let mut buf = vec![0x5Au8; 1 << 17];
    let t = std::time::Instant::now();
    for k in 0..(1u64 << 11) {
        let mut nonce = [0u8; 24];
        nonce[..8].copy_from_slice(&k.to_le_bytes());
        c.encrypt_in_place_detached(chacha20poly1305::XNonce::from_slice(&nonce), b"", &mut buf).unwrap();
    }
    let us = t.elapsed().as_micros() as u64;
    eprintln!("seal bench (host, one core): A#256 MiB in A#{} ms, A#{} MB/s", us / 1000, (256u64 << 20) / us.max(1));
}
