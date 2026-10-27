//! The phone's half of joining a Photon fleet over the mesh (photon docs/pairing-v2.md "The ceremony over the mesh"; photon src/network/fgtw/protocol.rs build_bind_req_vsf / parse_fleet_resp_vsf / parse_fanout_resp_vsf; src/network/udp.rs build_lan_discovery): the device's keys across the three signature families, derived from an ira-rooted secret exactly as the app derives its from the machine fingerprint (fgtw src/pq.rs `SigningBundle::derive`, `eggs`); the LAN beacon; the binding request and its signed `bind_req` frame; the `fleet_resp` and `fanout_resp` frames a sponsor hands back, and what to do with them (fold the chain against our own roots, open the fan-out with our own ira). The chain fold, the egg verification and the fan-out open are `fgtw`'s own code, so what the phone accepts is what the app and the worker accept.
//!
//! Time is the caller's (eagle oscillations), as everywhere in this crate; byte compatibility with the app is pinned by tools/photon-interop.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use fgtw::fleet::{bindreq_signing_bytes, scheme, BindRequest, Egg, MembershipBlob};
use fgtw::keys::Keypair;
use fgtw::pq::{envelope_mask, reserve_eggs, sign_envelope, FleetSigner, KeyBundle};
use rand_chacha::ChaCha12Rng;
use rand_core::SeedableRng;
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::{VsfBuilder, VsfType};

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
        use fn_dsa::{sign_key_size, vrfy_key_size, KeyPairGenerator, KeyPairGeneratorStandard, FN_DSA_LOGN_512};
        let ed = fgtw::keys::derive_device_keypair(fingerprint);
        let seed_for = |domain: &str| -> [u8; 32] { blake3::derive_key(domain, fingerprint) };
        let mut rng = ChaCha12Rng::from_seed(seed_for("PHOTON_DEVICE_FALCON512_v1"));
        let mut falcon_sk = vec![0u8; sign_key_size(FN_DSA_LOGN_512)];
        let mut falcon_vk = vec![0u8; vrfy_key_size(FN_DSA_LOGN_512)];
        KeyPairGeneratorStandard::default().keygen(FN_DSA_LOGN_512, &mut rng, &mut falcon_sk, &mut falcon_vk);
        let mut rng = ChaCha12Rng::from_seed(seed_for("PHOTON_DEVICE_SPHINCS_PLUS_v1"));
        // Keygen from a seeded CSPRNG is deterministic; the only failure is an internal invariant, which is a bug, not a condition.
        let (pk, sk) = slh_dsa_sha2_128s::try_keygen_with_rng(&mut rng).expect("slh-dsa keygen from a seeded rng");
        DeviceKeys { ed, falcon_sk, falcon_vk, sphincs_sk: sk.into_bytes(), sphincs_pk: pk.into_bytes() }
    }

    pub fn device_pubkey(&self) -> [u8; 32] {
        self.ed.public.to_bytes()
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
            use fn_dsa::{signature_size, SigningKey, SigningKeyStandard, DOMAIN_NONE, HASH_ID_RAW};
            let mut h = blake3::Hasher::new_derive_key("PHOTON_FALCON512_SIGN_NONCE_v1");
            h.update(&self.falcon_sk);
            h.update(msg);
            let mut rng = ChaCha12Rng::from_seed(*h.finalize().as_bytes());
            let mut sk = SigningKeyStandard::decode(&self.falcon_sk).expect("stored falcon key decodes");
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

/// The ports the app listens on: frames, and the LAN beacons.
pub const PORT: u16 = 4383;
pub const BEACON_PORT: u16 = 4384;
/// The app's LAN beacon groups: IPv4 multicast 239.104.199.144 and IPv6 ff02::68c7:9014 (photon src/network/status.rs).
pub const BEACON_V4: [u8; 4] = [239, 104, 199, 144];
pub const BEACON_V6: [u8; 16] = [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x68, 0xc7, 0x90, 0x14];

/// The LAN discovery beacon (`pt_disc`): the handle proof as the header's provenance, the device key and the frame port in the section; unsigned, provenance only. Byte-identical to the app's `udp::build_lan_discovery` for the same time.
pub fn beacon(handle_proof: [u8; 32], port: u16, device_pubkey: [u8; 32], time: i64) -> Vec<u8> {
    VsfBuilder::new()
        .creation_time_oscillations(time)
        .provenance_hash(handle_proof)
        .provenance_only()
        .add_section("pt_disc", vec![("port".to_string(), VsfType::u(port as usize, false)), ("ke".to_string(), VsfType::ke(device_pubkey.to_vec()))])
        .build()
        .unwrap_or_default()
}

/// A beacon's (handle proof, frame port, device key); None for anything else.
pub fn parse_beacon(bytes: &[u8]) -> Option<([u8; 32], u16, Option<[u8; 32]>)> {
    let (header, end) = VsfHeader::decode(bytes).ok()?;
    let hp: [u8; 32] = match &header.provenance_hash {
        VsfType::hp(b) if b.len() == 32 => b.as_slice().try_into().ok()?,
        _ => return None,
    };
    let section = header.primary_section(bytes, end).ok()?;
    if section.name != "pt_disc" {
        return None;
    }
    let port = section.get_field("port").and_then(|f| f.values.first()).and_then(|v| v.as_u64()).and_then(|n| u16::try_from(n).ok())?;
    let ke = section.get_field("ke").and_then(|f| f.values.first()).and_then(|v| match v {
        VsfType::ke(b) if b.len() == 32 => b.as_slice().try_into().ok(),
        _ => None,
    });
    Some((hp, port, ke))
}

/// This device's binding request for fleet `handle_proof` at time `t`: "I consent to join", one egg per scheme it holds, co-signed by `Ed25519(identity_seed)`; no NFC commitment. The same bytes the app's `make_bind_request` mints.
pub fn make_bind_request(keys: &DeviceKeys, identity_seed: &[u8; 32], handle_proof: &[u8; 32], t: i64) -> BindRequest {
    use ed25519_dalek::Signer;
    let me = keys.device_pubkey();
    let msg = bindreq_signing_bytes(handle_proof, &me, t);
    let bundle = keys.bundle();
    let mask = bundle.as_ref().map(|b| b.mask()).unwrap_or(scheme::MASK_BASE);
    let identity_key = ed25519_dalek::SigningKey::from_bytes(identity_seed);
    BindRequest { device_pubkey: me, t, device_sig: keys.eggs(&msg, mask), bundle, identity_sig: identity_key.sign(&msg).to_bytes().to_vec(), nfc_hash: [0u8; 32] }
}

/// A device-signed envelope around `section`: the header reserves one egg slot per scheme at the envelope tier (Ed25519 + Falcon for a full bundle) and `sign_envelope` fills them over the file hash — the app's `signed_only_eggs(envelope_slots)` + `sign_device_envelope`.
pub fn signed_frame(section: VsfSection, keys: &DeviceKeys, time: i64) -> Result<Vec<u8>, String> {
    let unsigned = VsfBuilder::new()
        .creation_time_oscillations(time)
        .signed_only_eggs(VsfType::ke(keys.device_pubkey().to_vec()), &reserve_eggs(envelope_mask(keys)))
        .add_section_direct(section)
        .build()
        .map_err(|e| alloc::format!("frame build: {e}"))?;
    sign_envelope(unsigned, keys)
}

/// The `bind_req` frame: the request's fields (hp, dk, t, ds, is, kb, nh) in an envelope signed by the device — the app's `build_bind_req_vsf`.
pub fn bind_req_vsf(req: &BindRequest, handle_proof: &[u8; 32], keys: &DeviceKeys, time: i64) -> Result<Vec<u8>, String> {
    let mut section = VsfSection::new("bind_req");
    section.add_field("hp", VsfType::hP(handle_proof.to_vec()));
    section.add_field("dk", VsfType::ke(req.device_pubkey.to_vec()));
    section.add_field("t", VsfType::e(vsf::types::EtType::e6(req.t)));
    section.add_field("ds", VsfType::ge(fgtw::pq::eggs_to_bytes(&req.device_sig)));
    section.add_field("is", VsfType::ge(req.identity_sig.clone()));
    if let Some(b) = &req.bundle {
        section.add_field("kb", VsfType::ge(b.to_bytes()));
    }
    section.add_field("nh", VsfType::hb(req.nfc_hash.to_vec()));
    signed_frame(section, keys, time)
}

/// The `fleet_req` frame: ask a device for handle `hp`'s membership chain.
pub fn fleet_req_vsf(handle_proof: &[u8; 32], keys: &DeviceKeys, time: i64) -> Result<Vec<u8>, String> {
    let mut section = VsfSection::new("fleet_req");
    section.add_field("hp", VsfType::hP(handle_proof.to_vec()));
    signed_frame(section, keys, time)
}

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

fn field_hp(section: &VsfSection, name: &str) -> Option<[u8; 32]> {
    section.get_field(name).and_then(|f| f.values.first()).and_then(|v| match v {
        VsfType::hP(b) | VsfType::hp(b) | VsfType::hb(b) if b.len() == 32 => b.as_slice().try_into().ok(),
        _ => None,
    })
}

fn field_ge(section: &VsfSection, name: &str) -> Option<Vec<u8>> {
    section.get_field(name).and_then(|f| f.values.first()).and_then(|v| match v {
        VsfType::ge(b) => Some(b.clone()),
        _ => None,
    })
}

/// Verify a device-signed envelope frame (the file signature under its own `ke`) and read its section. Returns the frame and the signing device. Pings, pongs and photons are not envelopes (their signature covers the provenance alone) — `wire::parse` reads those.
pub fn parse_frame(bytes: &[u8]) -> Result<(Inbound, [u8; 32]), String> {
    let (header, end) = vsf::verification::read_verified(bytes, None).map_err(|e| alloc::format!("frame verification failed: {e}"))?;
    let signer = vsf::verification::extract_signer_pubkey(bytes)?;
    let section = header.primary_section(bytes, end).map_err(|e| alloc::format!("frame section: {e}"))?;
    let frame = match section.name.as_str() {
        "fleet_resp" => Inbound::FleetResp { hp: field_hp(&section, "hp").ok_or("fleet_resp missing hp")?, chain: field_ge(&section, "ch").ok_or("fleet_resp missing ch")? },
        "fanout_resp" => Inbound::FanoutResp { hp: field_hp(&section, "hp").ok_or("fanout_resp missing hp")?, blob: field_ge(&section, "bl").ok_or("fanout_resp missing bl")? },
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
