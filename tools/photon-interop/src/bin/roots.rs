//! The identity roots a ferros phone is paired with, derived on the dev box from the handle with Photon's own derivation (`Handle::to_identity_seed`, `Handle::username_to_handle_proof` — ihi over vsf's `x` text codec, whose 9 MB tables the phone does not carry). Reads the handle from a file (keys/, never a repo), writes `identity_seed` and `handle_proof` as 32-byte files beside it for `ferros-bridge store put photon.identity_seed @…` / `photon.handle_proof @…`. Prints only lengths and fingerprints — never the handle, never the roots.
//!
//! Usage: roots <handle file> <out dir>

use photon::types::Handle;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: roots <handle file> <out dir>");
        std::process::exit(2);
    }
    let handle = std::fs::read_to_string(&args[1]).expect("handle file");
    // The handle is BYTE-PRECISE in Photon; a file's trailing newline is the file's, not the handle's.
    let handle = handle.strip_suffix('\n').unwrap_or(&handle).to_string();
    let seed = Handle::to_identity_seed(&handle);
    let proof = Handle::username_to_handle_proof(&handle);
    let out = std::path::Path::new(&args[2]);
    std::fs::write(out.join("photon.identity_seed"), seed).expect("write seed");
    std::fs::write(out.join("photon.handle_proof"), proof).expect("write proof");
    let fp = |b: &[u8]| blake3::hash(b).to_hex()[..8].to_string();
    println!("identity_seed 32 B (fp {}), handle_proof 32 B (fp {}) written to {}", fp(&seed), fp(&proof), out.display());
}
