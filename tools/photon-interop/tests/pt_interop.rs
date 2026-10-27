//! photon_core's Photon Transport against the stock app's PTManager, both directions, in one process: every datagram one side emits is fed to the other's own parser and state machine, so a frame the phone sends arrives in the app's buffer byte for byte and the app's transfer lands in the phone's — or this fails before any phone does.

use photon::network::fgtw::Keypair;
use photon::network::pt::{PTAck, PTComplete, PTData, PTManager, PTSpec};
use photon_core::pt::{self, Addr, Event, Pt};
use std::net::SocketAddr;
use vsf::file_format::VsfHeader;

const T: i64 = 1_420_000_000_000;

fn addr() -> (Addr, SocketAddr) {
    let mut ip = [0u8; 16];
    ip[..4].copy_from_slice(&[192, 168, 1, 2]);
    (Addr { fam: 4, ip, port: 4383 }, "192.168.1.2:4383".parse().unwrap())
}

/// A frame of `n` bytes that is a VSF document (the app's PT carries complete VSF files).
fn frame(n: usize) -> Vec<u8> {
    let mut f = vsf::VsfBuilder::new().creation_time_oscillations(T).provenance_hash([3u8; 32]).provenance_only().add_section("test", vec![("x".to_string(), vsf::VsfType::u(1, false))]).build().unwrap();
    while f.len() < n {
        f.push((f.len() % 253) as u8);
    }
    f
}

/// The app's receive loop, condensed: classify a datagram the phone sent and run it through PTManager; return what the app would send back.
fn app_takes(mgr: &mut PTManager, from: SocketAddr, d: &[u8], sender: [u8; 32]) -> Vec<Vec<u8>> {
    if photon::network::pt::is_pt_data(d) {
        let data = PTData::from_bytes(d).expect("the app parses our DATA packet");
        let sid = data.stream_id;
        let mut out = Vec::new();
        if let Some(ack) = mgr.handle_data(from, data) {
            out.push(ack);
        }
        if let Some(done) = mgr.check_inbound_complete(from, sid) {
            out.push(done);
        }
        return out;
    }
    let (header, end) = VsfHeader::decode(d).expect("the app decodes our VSF datagram");
    let prov: [u8; 32] = match &header.provenance_hash {
        vsf::VsfType::hp(b) => b.as_slice().try_into().unwrap(),
        _ => panic!("no provenance"),
    };
    if let Some(f) = header.fields.iter().find(|f| f.name.starts_with("pt_") && f.offset_bytes == 0 && f.size_bytes == 0) {
        return match f.name.as_str() {
            "pt_ack" => mgr.handle_ack(from, PTAck::from_vsf_header(prov, &f.inline_values).expect("the app parses our ack")),
            "pt_done" => {
                mgr.handle_complete(from, PTComplete::from_vsf_header(prov, &f.inline_values).expect("the app parses our done"));
                Vec::new()
            }
            other => panic!("unexpected control {other}"),
        };
    }
    let section = header.primary_section(d, end).unwrap();
    assert_eq!(section.name, "pt_spec", "a section datagram from the phone's PT is its SPEC");
    let fields: Vec<(String, vsf::VsfType)> = section.fields.iter().filter_map(|f| f.values.first().map(|v| (f.name.clone(), v.clone()))).collect();
    let spec = PTSpec::from_vsf_fields(&fields).expect("the app parses our SPEC");
    vec![mgr.handle_spec(from, spec, Some(sender))]
}

#[test]
fn phone_to_app_transfer() {
    let (pa, sa) = addr();
    let k = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
    let me = k.verifying_key().to_bytes();
    let mut ours = Pt::new(k);
    let mut theirs = PTManager::new(Keypair::from_seed(&[8u8; 32]));
    let f = frame(30_000);
    let mut wire: Vec<Vec<u8>> = ours.send(pa, f.clone(), 1_000, T).into_iter().map(|(_, d)| d).collect();
    let mut now = 1_000u64;
    for _ in 0..200 {
        if wire.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for d in wire.drain(..) {
            for back in app_takes(&mut theirs, sa, &d, me) {
                for (_, d2) in ours.receive(pa, &back, now, T) {
                    next.push(d2);
                }
            }
        }
        now += 1_000;
        wire = next;
    }
    let got = theirs.take_inbound_data(sa, b'a').expect("the app holds the reassembled transfer");
    assert_eq!(got, f, "the app reassembled our bytes exactly");
    assert_eq!(ours.take_events(), vec![Event::Sent { to: pa, hash: *blake3::hash(&f).as_bytes(), ok: true }]);
}

#[test]
fn app_to_phone_transfer() {
    let (pa, sa) = addr();
    let mut ours = Pt::new(ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]));
    let kp = Keypair::from_seed(&[8u8; 32]);
    let their_key = kp.public.to_bytes();
    let mut theirs = PTManager::new(kp);
    let f = frame(45_000);
    let mut wire: Vec<Vec<u8>> = vec![theirs.send_tagged(sa, None, f.clone(), Some([9u8; 32]), None)];
    let mut now = 1_000u64;
    for _ in 0..200 {
        if wire.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for d in wire.drain(..) {
            for (_, back) in ours.receive(pa, &d, now, T) {
                next.extend(app_takes(&mut theirs, sa, &back, [0u8; 32]));
            }
        }
        now += 1_000;
        wire = next;
    }
    assert_eq!(ours.take_events(), vec![Event::Frame { from: pa, bytes: f, signer: Some(their_key) }]);
    assert!(theirs.is_outbound_complete(&sa), "the app saw our DONE");
}

#[test]
fn small_frames_and_packet_acks_both_ways() {
    let (pa, sa) = addr();
    let mut ours = Pt::new(ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]));
    let mut theirs = PTManager::new(Keypair::from_seed(&[8u8; 32]));
    // The app's packet ack for a small frame we sent settles it.
    let f = frame(400);
    let out = ours.send(pa, f.clone(), 1_000, T);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1, f, "a small frame goes as itself");
    let ack = theirs.build_packet_ack(&f);
    assert!(ours.receive(pa, &ack, 2_000, T).is_empty());
    assert_eq!(ours.take_events(), vec![Event::Sent { to: pa, hash: *blake3::hash(&f).as_bytes(), ok: true }]);
    // Our packet ack for a small frame the app sent advances its stop-and-wait queue.
    let g = frame(500);
    let sent = theirs.send_tagged(sa, None, g.clone(), None, None);
    assert_eq!(sent, g);
    let ours_ack = pt::packet_ack(T, &g);
    let _ = app_takes(&mut theirs, sa, &ours_ack, [0u8; 32]);
    // Nothing further queued: the ack matched (a mismatched ack leaves the packet in flight, which a second send would queue behind).
    let h = frame(600);
    assert_eq!(theirs.send_tagged(sa, None, h.clone(), None, None), h, "the queue was free, so the next frame goes out at once");
}
