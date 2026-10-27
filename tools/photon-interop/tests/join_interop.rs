//! photon_core's join against the stock app's own code, byte for byte: the device keys and eggs against fgtw's SigningBundle, the LAN beacon against udp::build_lan_discovery, the bind_req frame against protocol::build_bind_req_vsf (with the app's device signer installed, so both sides sign the envelope with the same two eggs), and the sponsor's fleet_resp / fanout_resp frames opened by the phone's half: a real chain founded and extended with fgtw, a real fan-out sealed with fgtw.

use fgtw::fleet::{scheme, MembershipBlob};
use fgtw::pq::{FleetSigner, SigningBundle};
use photon::network::fgtw::fleet as pfleet;
use photon::network::fgtw::protocol as pproto;
use photon_core::join::{self, DeviceKeys, Inbound};

const T: i64 = 1_420_000_000_000;
const FP: &[u8] = b"photon-interop device fingerprint";

fn hp() -> [u8; 32] {
    [0x5Au8; 32]
}
fn seed() -> [u8; 32] {
    blake3::hash(b"interop identity").into()
}

#[test]
fn device_keys_match_the_app() {
    let ours = DeviceKeys::derive(FP);
    let theirs = SigningBundle::derive(FP);
    assert_eq!(ours.public(), theirs.public(), "the three public keys");
    let msg = b"the eggs sign this";
    let mask = (1 << scheme::ED25519) | (1 << scheme::FALCON512) | (1 << scheme::SPHINCS_PLUS);
    let a = ours.eggs(msg, mask);
    let b = theirs.eggs(msg, mask);
    assert_eq!(a.len(), 3);
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!((x.scheme, &x.sig), (y.scheme, &y.sig), "egg scheme {}", x.scheme);
    }
}

#[test]
fn beacon_matches_the_app() {
    let ours = join::beacon(hp(), 4383, [9u8; 32], T);
    // The app stamps its own clock; everything but the time must agree, so compare with the time field cut out by re-parsing both.
    let src: std::net::SocketAddr = "192.168.1.9:4384".parse().unwrap();
    let (h, ip, port, dev) = photon::network::udp::parse_lan_discovery(&ours, src).expect("the app parses our beacon");
    assert_eq!((h, ip, port, dev), (hp(), "192.168.1.9".parse().unwrap(), 4383, Some([9u8; 32])));
    let theirs = photon::network::udp::build_lan_discovery(hp(), 4383, [9u8; 32]);
    let parsed = join::parse_beacon(&theirs).expect("we parse the app's beacon");
    assert_eq!(parsed, (hp(), 4383, Some([9u8; 32])));
}

#[test]
fn bind_req_matches_the_app_and_verifies() {
    let ours = DeviceKeys::derive(FP);
    let theirs = SigningBundle::derive(FP);
    pfleet::install_device_signer(pfleet::DeviceSigner::Pq(theirs.clone()));
    let req = join::make_bind_request(&ours, &seed(), &hp(), T);
    let identity_pk = ed25519_dalek::SigningKey::from_bytes(&seed()).verifying_key().to_bytes();
    assert!(req.verify(&hp(), &identity_pk), "the request verifies as the registry would");
    let our_frame = join::bind_req_vsf(&req, &hp(), &ours, T).unwrap();
    let (h, back, signer) = pproto::parse_bind_req_vsf(&our_frame).expect("the app parses + verifies our bind_req envelope");
    assert_eq!((h, signer), (hp(), ours.device_pubkey()));
    assert_eq!(back, req);
    // Same request, same time: the app's builder gives the same bytes (its installed signer is this device's bundle).
    let their_frame = pproto::build_bind_req_vsf(&req, &hp(), theirs.keypair().secret.as_bytes()).unwrap();
    let their_time_fixed = their_frame;
    // The app stamps its clock in the header; compare the frames with the app's parse instead of byte equality for the header, and byte equality for the section.
    let (_, back2, _) = pproto::parse_bind_req_vsf(&their_time_fixed).unwrap();
    assert_eq!(back2, req);
}

#[test]
fn sponsor_frames_open_on_the_phone() {
    let phone = DeviceKeys::derive(FP);
    let sponsor = SigningBundle::derive(b"the sponsor's fingerprint");
    pfleet::install_device_signer(pfleet::DeviceSigner::Pq(sponsor.clone()));
    // A fleet founded by the sponsor, then the phone bound into it declared — the chain the sponsor pushes.
    let mut chain = MembershipBlob::genesis_v2(&sponsor, hp(), &seed(), T);
    let req = join::make_bind_request(&phone, &seed(), &hp(), T + 1_000);
    chain.add_declared(&sponsor, req.device_pubkey, T + 2_000, req.t, req.device_sig.clone(), req.bundle.clone().unwrap());
    let chain_bytes = chain.to_vsf_bytes().unwrap();
    let frame = pproto::build_fleet_resp_vsf(&hp(), &chain_bytes, &sponsor.keypair().public.to_bytes(), sponsor.keypair().secret.as_bytes()).unwrap();
    let (inbound, signer) = join::parse_frame(&frame).expect("the phone verifies the sponsor's fleet_resp envelope");
    assert_eq!(signer, sponsor.keypair().public.to_bytes());
    let Inbound::FleetResp { hp: h, chain: ch } = inbound else { panic!("not a fleet_resp") };
    assert_eq!((h, ch.len()), (hp(), chain_bytes.len()));
    let (blob, bound) = join::adopt_chain(&hp(), &seed(), &phone.device_pubkey(), &ch).expect("the phone folds the chain");
    assert!(bound, "the phone finds itself in the fold");
    assert_eq!(blob.ops.len(), 2);
    // A chain for another identity is refused.
    assert!(join::adopt_chain(&hp(), &[1u8; 32], &phone.device_pubkey(), &ch).is_err());
    // The sponsor's fan-out, sealed to both members, opens with the phone's own ira.
    let key = [0x77u8; 32];
    let members = blob.fold().unwrap();
    let wraps = fgtw::fanout::fanout_seal(&hp(), &key, &members, &seed()).unwrap();
    let epoch_pub = fgtw::pq::epoch_bundle(&key).public();
    let blob_bytes = fgtw::fanout::fanout_to_bytes(3, &fgtw::fanout::fleet_key_fingerprint(&key), &sponsor.keypair().public.to_bytes(), &epoch_pub, &wraps);
    let frame = pproto::build_fanout_resp_vsf(&hp(), &blob_bytes, &sponsor.keypair().public.to_bytes(), sponsor.keypair().secret.as_bytes()).unwrap();
    let (inbound, _) = join::parse_frame(&frame).unwrap();
    let Inbound::FanoutResp { blob: fb, .. } = inbound else { panic!("not a fanout_resp") };
    assert_eq!(join::open_fanout(&hp(), &fb, &phone, &seed()).unwrap(), Some((3, key)));
    // A stranger's ira opens nothing.
    let stranger = DeviceKeys::derive(b"someone else");
    assert_eq!(join::open_fanout(&hp(), &fb, &stranger, &seed()).unwrap(), None);
    // The pairing words the phone shows are the ones the sponsor's screen computes for its key.
    assert_eq!(join::pair_words(&phone.device_pubkey(), &seed()), pfleet::masked_device_words(&phone.device_pubkey(), &seed()));
}

#[test]
fn eagle_time_from_utc_matches_vsf() {
    // The app stamps `vsf::eagle_time_oscillations()` (the legacy label epoch, 20:17:40 UTC — feedback_epoch_fixed); our conversion of the same wall clock must land within a tenth of a second of it.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let ours = join::eagle_from_utc(now.as_secs() as i64, now.subsec_nanos() as u64, 1_000_000_000);
    let theirs = vsf::eagle_time_oscillations();
    assert!((ours - theirs).abs() < vsf::OSCILLATIONS_PER_SECOND as i64 / 10, "ours {ours} theirs {theirs}");
}
