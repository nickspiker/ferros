//! The post-quantum signatures a Photon fleet join needs, on the phone: derives a device's Ed25519, Falcon-512 and SLH-DSA-SHA2-128s keys from a fingerprint and signs one message with each, exactly as the stock app's signer does (fgtw src/pq.rs `SigningBundle::derive` and `eggs`: BLAKE3 domain seeds into rand 0.8's StdRng, which is ChaCha12; Falcon and SPHINCS+ signing seeded from the secret and the message), then prints a BLAKE3 prefix of each public key and signature and the times. A host run of fgtw's own code on the same test fingerprint gives the same lines (STATUS.md, "Enrollment / pairing").
//!
//! `ferros-bridge spawn pqprobe.bin --name pqprobe`. Grant slots: 0 console. The work runs on a second thread with a 512 KiB stack (Falcon's signing key is 116 KiB by value and its key generator 48 KiB; an arc's own stack is 64 KiB), pinned to the X3.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;

use ed25519_dalek::{Signer, SigningKey};
use fips205::slh_dsa_sha2_128s;
use fips205::traits::{SerDes, Signer as _};
use fn_dsa::{sign_key_size, signature_size, vrfy_key_size, KeyPairGenerator, KeyPairGeneratorStandard, SigningKey as _, SigningKeyStandard, DOMAIN_NONE, FN_DSA_LOGN_512, HASH_ID_RAW};
use rand_chacha::ChaCha12Rng;
use rand_core::SeedableRng;

/// The heap lives in `.data`, not `.bss` (the kernel sizes an arc from the flat image, which carries no `.bss`): for whatever a dependency allocates (one links `alloc`).
const HEAP_BYTES: usize = 64 * 1024;

#[repr(C, align(16))]
struct Heap(UnsafeCell<[u8; HEAP_BYTES]>);
unsafe impl Sync for Heap {}
#[unsafe(link_section = ".data.heap")]
static HEAP: Heap = Heap(UnsafeCell::new([1; HEAP_BYTES]));

/// A bump allocator that starts over whenever every allocation has been freed . Single-threaded, so plain cells suffice.
struct Bump {
    next: UnsafeCell<usize>,
    live: UnsafeCell<usize>,
}
unsafe impl Sync for Bump {}

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let base = HEAP.0.get() as usize;
        let start = (base + *self.next.get() + l.align() - 1) & !(l.align() - 1);
        if start + l.size() > base + HEAP_BYTES {
            return core::ptr::null_mut();
        }
        *self.next.get() = start + l.size() - base;
        *self.live.get() += 1;
        start as *mut u8
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {
        *self.live.get() -= 1;
        if *self.live.get() == 0 {
            *self.next.get() = 0;
        }
    }
}

#[global_allocator]
static ALLOC: Bump = Bump { next: UnsafeCell::new(0), live: UnsafeCell::new(0) };

const FINGERPRINT: &[u8] = b"ferros pq probe fingerprint";
const MESSAGE: &[u8] = b"ferros pq probe message";
const STACK_BYTES: usize = 512 * 1024;

#[repr(C, align(16))]
struct Stack([u8; STACK_BYTES]);
/// In `.data`, not `.bss`: the kernel sizes an arc from the flat image, which carries no `.bss`.
#[unsafe(link_section = ".data.stack")]
static mut STACK: Stack = Stack([1; STACK_BYTES]);

struct Log {
    grants: u64,
    buf: [u8; 1024],
    n: usize,
}

impl Log {
    fn put(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn hash(&mut self, b: &[u8]) {
        let h = blake3::hash(b);
        let x = b"0123456789abcdef";
        for &c in &h.as_bytes()[..8] {
            self.put(&[x[(c >> 4) as usize], x[(c & 15) as usize]]);
        }
    }
    fn dec(&mut self, mut v: u64) {
        let mut b = [0u8; 20];
        let mut i = b.len();
        loop {
            i -= 1;
            b[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let d = b;
        self.put(&d[i..]);
    }
    fn flush(&mut self) {
        let mut r = [0u8; 16];
        ferros_sys::call(ferros_sys::cap(self.grants, 0), &self.buf[..self.n], &mut r);
        self.n = 0;
    }
}

static mut GRANTS: u64 = 0;

extern "C" fn work(_: u64) -> ! {
    let mut l = Log { grants: unsafe { GRANTS }, buf: [0; 1024], n: 0 };
    ferros_sys::pin(ferros_sys::CORES_BIG);
    ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 1000);
    let seed_for = |domain: &str| -> [u8; 32] { blake3::derive_key(domain, FINGERPRINT) };

    let t = ferros_sys::now();
    let ed = SigningKey::from_bytes(blake3::hash(FINGERPRINT).as_bytes());
    let mut falcon_sk = [0u8; sign_key_size(FN_DSA_LOGN_512)];
    let mut falcon_vk = [0u8; vrfy_key_size(FN_DSA_LOGN_512)];
    {
        let mut rng = ChaCha12Rng::from_seed(seed_for("PHOTON_DEVICE_FALCON512_v1"));
        KeyPairGeneratorStandard::default().keygen(FN_DSA_LOGN_512, &mut rng, &mut falcon_sk, &mut falcon_vk);
    }
    let t_falcon_kg = ferros_sys::us_since(t);
    let t = ferros_sys::now();
    let mut rng = ChaCha12Rng::from_seed(seed_for("PHOTON_DEVICE_SPHINCS_PLUS_v1"));
    let Ok((sphincs_pk, sphincs_sk)) = slh_dsa_sha2_128s::try_keygen_with_rng(&mut rng) else { ferros_sys::exit(1) };
    let t_sphincs_kg = ferros_sys::us_since(t);
    let (sphincs_pk, sphincs_sk) = (sphincs_pk.into_bytes(), sphincs_sk.into_bytes());

    l.put(b"pqprobe: ed25519 pk ");
    l.hash(ed.verifying_key().as_bytes());
    l.put(b" falcon vk ");
    l.hash(&falcon_vk);
    l.put(b" (");
    l.dec(falcon_vk.len() as u64);
    l.put(b" B) sphincs pk ");
    l.hash(&sphincs_pk);
    l.put(b" (");
    l.dec(sphincs_pk.len() as u64);
    l.put(b" B)\n");
    l.flush();

    let ed_sig = ed.sign(MESSAGE).to_bytes();
    l.put(b"pqprobe: egg scheme 0 sig ");
    l.hash(&ed_sig);
    l.put(b" (64 B)\n");

    let t = ferros_sys::now();
    let mut h = blake3::Hasher::new_derive_key("PHOTON_FALCON512_SIGN_NONCE_v1");
    h.update(&falcon_sk);
    h.update(MESSAGE);
    let mut rng = ChaCha12Rng::from_seed(*h.finalize().as_bytes());
    let Some(mut sk) = SigningKeyStandard::decode(&falcon_sk) else { ferros_sys::exit(2) };
    let mut fsig = [0u8; signature_size(FN_DSA_LOGN_512)];
    sk.sign(&mut rng, &DOMAIN_NONE, &HASH_ID_RAW, MESSAGE, &mut fsig);
    let t_falcon_sign = ferros_sys::us_since(t);
    l.put(b"pqprobe: egg scheme 1 sig ");
    l.hash(&fsig);
    l.put(b" (");
    l.dec(fsig.len() as u64);
    l.put(b" B)\n");

    let t = ferros_sys::now();
    let mut h = blake3::Hasher::new_derive_key("PHOTON_SPHINCS_PLUS_SIGN_NONCE_v1");
    h.update(&sphincs_sk);
    h.update(MESSAGE);
    let mut rng = ChaCha12Rng::from_seed(*h.finalize().as_bytes());
    let Ok(sk) = slh_dsa_sha2_128s::PrivateKey::try_from_bytes(&sphincs_sk) else { ferros_sys::exit(3) };
    let Ok(ssig) = sk.try_sign_with_rng(&mut rng, MESSAGE, &[], true) else { ferros_sys::exit(4) };
    let t_sphincs_sign = ferros_sys::us_since(t);
    l.put(b"pqprobe: egg scheme 2 sig ");
    l.hash(&ssig);
    l.put(b" (");
    l.dec(ssig.len() as u64);
    l.put(b" B)\n");
    l.put(b"pqprobe: on the X3, falcon keygen ");
    l.dec(t_falcon_kg / 1000);
    l.put(b" ms, sphincs keygen ");
    l.dec(t_sphincs_kg / 1000);
    l.put(b" ms, falcon sign ");
    l.dec(t_falcon_sign / 1000);
    l.put(b" ms, sphincs sign ");
    l.dec(t_sphincs_sign / 1000);
    l.put(b" ms\n");
    l.flush();
    ferros_sys::exit(0)
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { GRANTS = grants };
    let stack = unsafe { &mut (*(&raw mut STACK)).0 };
    ferros_sys::thread_spawn(work, stack, 0);
    ferros_sys::thread_exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
