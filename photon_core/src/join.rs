//! The phone's half of joining a Photon fleet over the mesh (photon docs/pairing-v2.md "The ceremony over the mesh"; photon src/network/fgtw/protocol.rs build_bind_req_vsf / parse_fleet_resp_vsf / parse_fanout_resp_vsf; src/network/udp.rs build_lan_discovery): the device's keys across the three signature families, derived from an ira-rooted secret exactly as the app derives its from the machine fingerprint (fgtw src/pq.rs `SigningBundle::derive`, `eggs`); the frames themselves are fgtw's (`fgtw::protocol`, re-exported here); what is the phone's own is what to do with the `fleet_resp` and `fanout_resp` a sponsor hands back (fold the chain against our own roots, open the fan-out with our own ira). The chain fold, the egg verification and the fan-out open are `fgtw`'s own code, so what the phone accepts is what the app and the worker accept.
//!
//! Time is the caller's (eagle oscillations), as everywhere in this crate; byte compatibility with the app is pinned by tools/photon-interop.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use fgtw::fleet::{scheme, Egg, MembershipBlob};
use fgtw::keys::Keypair;
use fgtw::pq::{FleetSigner, KeyBundle};
use rand_chacha::ChaCha12Rng;
use rand_core::SeedableRng;

/// This device's keys across the three families the fleet signs with: Ed25519 (the device key itself), Falcon-512 and SLH-DSA-SHA2-128s — derived from one secret exactly as the app's `SigningBundle::derive` derives from the machine fingerprint (BLAKE3 domain seeds into rand 0.8's StdRng, which is ChaCha12), so the same secret gives the same keys here and there. On ferros the secret is the kernel's ira-derived key for the domain `photon device` (ferros_sys::ira_key): the same on every boot and after a factory reset.
pub struct DeviceKeys {
    ed: Keypair,
    falcon_sk: Vec<u8>,
    falcon_vk: Vec<u8>,
    sphincs_sk: [u8; fips205::slh_dsa_sha2_128s::SK_LEN],
    sphincs_pk: [u8; fips205::slh_dsa_sha2_128s::PK_LEN],
}

impl DeviceKeys {
    pub fn derive(fingerprint: &[u8]) -> Self {
        use fips205::slh_dsa_sha2_128s;
        use fips205::traits::SerDes;
        use fn_dsa::{sign_key_size, vrfy_key_size, KeyPairGenerator, KeyPairGenerator512, FN_DSA_LOGN_512};
        let ed = fgtw::keys::derive_device_keypair(fingerprint);
        let seed_for = |domain: &str| -> [u8; 32] { blake3::derive_key(domain, fingerprint) };
        let mut rng = ChaCha12Rng::from_seed(seed_for("PHOTON_DEVICE_FALCON512_v1"));
        let mut falcon_sk = vec![0u8; sign_key_size(FN_DSA_LOGN_512)];
        let mut falcon_vk = vec![0u8; vrfy_key_size(FN_DSA_LOGN_512)];
        // The degree-512-only generator and signing key: the same bytes as the app's Standard ones for this degree, a third of the stack (a phone thread's stack is small).
        KeyPairGenerator512::default().keygen(FN_DSA_LOGN_512, &mut rng, &mut falcon_sk, &mut falcon_vk);
        let mut rng = ChaCha12Rng::from_seed(seed_for("PHOTON_DEVICE_SPHINCS_PLUS_v1"));
        // Keygen from a seeded CSPRNG is deterministic; the only failure is an internal invariant, which is a bug, not a condition.
        let (pk, sk) = slh_dsa_sha2_128s::try_keygen_with_rng(&mut rng).expect("slh-dsa keygen from a seeded rng");
        DeviceKeys { ed, falcon_sk, falcon_vk, sphincs_sk: sk.into_bytes(), sphincs_pk: pk.into_bytes() }
    }

    pub fn device_pubkey(&self) -> [u8; 32] {
        self.ed.public.to_bytes()
    }

    /// The Ed25519 half as a dalek signing key — what pings, pongs and PT SPECs are signed with (photon_core::wire, pt).
    pub fn signing_key(&self) -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&self.ed.secret.to_bytes())
    }

    /// The public bundle this device declares.
    pub fn public(&self) -> KeyBundle {
        KeyBundle::new(vec![(scheme::ED25519, self.ed.public.to_bytes().to_vec()), (scheme::FALCON512, self.falcon_vk.clone()), (scheme::SPHINCS_PLUS, self.sphincs_pk.to_vec())]).expect("a derived bundle is well-formed by construction")
    }
}

impl FleetSigner for DeviceKeys {
    fn keypair(&self) -> &Keypair {
        &self.ed
    }

    /// One egg per scheme in `schemes` that this device holds, Ed25519 first. The PQ signatures are seeded from the secret and the message (the app's rule), so a given (key, message) always yields the same egg.
    fn eggs(&self, msg: &[u8], schemes: scheme::Mask) -> Vec<Egg> {
        let mut out = self.ed.eggs(msg, schemes);
        if scheme::covers(schemes, 1 << scheme::FALCON512) {
            use fn_dsa::{signature_size, SigningKey, SigningKey512, DOMAIN_NONE, HASH_ID_RAW};
            let mut h = blake3::Hasher::new_derive_key("PHOTON_FALCON512_SIGN_NONCE_v1");
            h.update(&self.falcon_sk);
            h.update(msg);
            let mut rng = ChaCha12Rng::from_seed(*h.finalize().as_bytes());
            let mut sk = SigningKey512::decode(&self.falcon_sk).expect("stored falcon key decodes");
            let mut sig = vec![0u8; signature_size(sk.get_logn())];
            sk.sign(&mut rng, &DOMAIN_NONE, &HASH_ID_RAW, msg, &mut sig);
            out.push(Egg { scheme: scheme::FALCON512, sig });
        }
        if scheme::covers(schemes, 1 << scheme::SPHINCS_PLUS) {
            use fips205::slh_dsa_sha2_128s::PrivateKey;
            use fips205::traits::{SerDes, Signer};
            let mut h = blake3::Hasher::new_derive_key("PHOTON_SPHINCS_PLUS_SIGN_NONCE_v1");
            h.update(&self.sphincs_sk);
            h.update(msg);
            let mut rng = ChaCha12Rng::from_seed(*h.finalize().as_bytes());
            let sk = PrivateKey::try_from_bytes(&self.sphincs_sk).expect("stored slh-dsa key decodes");
            let sig = sk.try_sign_with_rng(&mut rng, msg, &[], true).expect("slh-dsa sign");
            out.push(Egg { scheme: scheme::SPHINCS_PLUS, sig: sig.to_vec() });
        }
        out
    }

    fn bundle(&self) -> Option<KeyBundle> {
        Some(self.public())
    }
}

// The frames between fleet devices are fgtw's (`fgtw::protocol`, fgtw MIGRATION.md step 5, 2026-10-31): the beacon, the binding request and its `bind_req` envelope, the departure request, `fleet_req` / `fleet_resp` / `fanout_resp` — one codec with the app and every other fleet client, not a copy pinned against it. Builders take this device's keys (`DeviceKeys` is a `FleetSigner`) and the caller's eagle time.
pub use fgtw::protocol::{beacon, bind_req, bind_request, depart_consent, depart_req, depart_words, depart_words_commit, parse_beacon, DepartReq, BEACON_PORT, BEACON_V4, BEACON_V6, DEPART_DESK, DEPART_NEW_OWNER, PORT};

/// A device-signed frame from a peer, by its section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    /// A membership chain for handle `hp` (`MembershipBlob::to_vsf_bytes` inside).
    FleetResp { hp: [u8; 32], chain: Vec<u8> },
    /// A fan-out blob for handle `hp` (`fanout_to_bytes` inside).
    FanoutResp { hp: [u8; 32], blob: Vec<u8> },
    /// Some other envelope frame, by section name (a sibling's control traffic the phone does not handle yet).
    Other(String),
}

/// Verify a device-signed envelope frame (fgtw's `open_frame`: the file signature under its own `ke`) and sort it by section. Returns the frame and the signing device. Pings, pongs and photons are not envelopes (their signature covers the provenance alone) — `wire::parse` reads those.
pub fn parse_frame(bytes: &[u8]) -> Result<(Inbound, [u8; 32]), String> {
    use fgtw::protocol::{field_ge, field_handle_proof, open_frame};
    let (section, signer) = open_frame(bytes)?;
    let frame = match section.name.as_str() {
        "fleet_resp" => Inbound::FleetResp { hp: field_handle_proof(&section, "hp").ok_or("fleet_resp missing hp")?, chain: field_ge(&section, "ch").ok_or("fleet_resp missing ch")? },
        "fanout_resp" => Inbound::FanoutResp { hp: field_handle_proof(&section, "hp").ok_or("fanout_resp missing hp")?, blob: field_ge(&section, "bl").ok_or("fanout_resp missing bl")? },
        other => Inbound::Other(other.to_string()),
    };
    Ok((frame, signer))
}

/// A chain handed to this device: it must claim slot `handle_proof`, its genesis must be this identity's (the canonical key from `identity_seed`), and it must fold. Returns the chain and whether `me` is a current member — green when true.
pub fn adopt_chain(handle_proof: &[u8; 32], identity_seed: &[u8; 32], me: &[u8; 32], chain: &[u8]) -> Result<(MembershipBlob, bool), String> {
    let blob = MembershipBlob::from_vsf_bytes(chain)?;
    if blob.genesis_handle_proof() != Some(*handle_proof) {
        return Err("chain claims a different handle".into());
    }
    if !blob.genesis_identity_matches(identity_seed) {
        return Err("chain is not this identity's fleet".into());
    }
    let members = blob.fold().map_err(|e| alloc::format!("chain does not fold: {e:?}"))?;
    let bound = members.contains(me);
    Ok((blob, bound))
}

/// A fan-out blob handed to this device: `Some((revision, fleet key))` when it holds a wrap for our ira and its header's epoch bundle is the one the key derives (a header we cannot reproduce is refused — the app's `open_checked`); None when no wrap is ours.
pub fn open_fanout(handle_proof: &[u8; 32], blob: &[u8], keys: &DeviceKeys, identity_seed: &[u8; 32]) -> Result<Option<(u64, [u8; 32])>, String> {
    let f = fgtw::fanout::fanout_from_bytes_full(blob)?;
    let Some(k) = fgtw::fanout::fanout_open(handle_proof, &f.kfp, &f.wraps, &keys.ed, identity_seed) else { return Ok(None) };
    // The epoch signing bundle derives from the fleet key under its own domain (fgtw pq.rs `epoch_bundle`).
    let epoch = DeviceKeys::derive(&blake3::derive_key("PHOTON_FLEET_EPOCH_SIGNING_v1", &k));
    if epoch.public() != f.epoch_pub {
        return Err("fanout: header epoch bundle does not match the fleet key — tampered header".into());
    }
    Ok(Some((f.revision, k)))
}

/// The 23 words this device shows for the sponsor to type: its device key masked to the fleet (fgtw pair.rs).
pub fn pair_words(device_pubkey: &[u8; 32], identity_seed: &[u8; 32]) -> String {
    fgtw::pair::masked_device_words(device_pubkey, identity_seed)
}

/// Eagle oscillations from a UTC wall clock (seconds since 1970 and the fraction as counter ticks over its rate): the legacy epoch label 1969-07-20T20:17:40 UTC the app's stamps count from (vsf eagle_time.rs `EAGLE_EPOCH_UNIX_SECS`).
pub fn eagle_from_utc(secs: i64, ticks: u64, hz: u64) -> i64 {
    const EPOCH: i64 = -14_182_940;
    let ops = vsf::OSCILLATIONS_PER_SECOND as i128;
    let whole = (secs as i128 - EPOCH as i128) * ops;
    let frac = if hz == 0 { 0 } else { (ticks as i128 * ops) / hz as i128 };
    (whole + frac) as i64
}
