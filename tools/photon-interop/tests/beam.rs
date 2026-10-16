//! photon_core::beam's packets through Photon's own BeamRx: Info decodes, every frame reassembles bit for bit, and losing as many packets as there are repair symbols still reassembles.

use photon::wave::beam as pb;
use photon::wave::keys as pk;
use photon_core::beam as cb;
use photon_core::wave as cw;

fn secret() -> [u8; 32] {
    cw::derive_wave_secret(&[1; 32], &[2; 16], &[3; 32], &[4; 32])
}

fn frame_bytes(n: usize, salt: u32) -> Vec<u8> {
    (0..n).map(|i| (i as u32).wrapping_mul(2_654_435_761).wrapping_add(salt) as u8).collect()
}

#[test]
fn photon_receives_our_beam() {
    let mut tx = cb::BeamTx::new(&secret(), cw::Direction::OriginToAnswer);
    let mut rx = pb::BeamRx::new(&secret(), pk::Direction::OriginToAnswer);
    let info = cb::Info { width: 640, height: 480, fps: 30, codec: cb::CODEC_H264, colour: vec![] };
    assert!(rx.push(&tx.info(&info)).is_none());
    let got = rx.take_info_change().expect("Photon decodes our Info");
    assert_eq!((got.width, got.height, got.fps, got.codec, got.colour.clone()), (640, 480, 30, pb::Codec::H264, vec![]));
    assert!(rx.push(&tx.orient(1)).is_none());
    let gains = cb::Gains([4096, 4000, 4100, 5000]);
    for (frame_no, size) in [(0u32, 1usize), (1, 1160), (2, 1161), (3, 50_000), (4, 300_000)] {
        let bytes = frame_bytes(size, frame_no);
        let packets = tx.frame(frame_no, gains, &bytes);
        let source_n = size.div_ceil(cb::SYMBOL_BYTES as usize);
        assert_eq!(packets.len(), source_n + source_n.div_ceil(cb::REPAIR_PER_SOURCE), "frame {frame_no} packet count");
        assert!(packets.iter().all(|p| p.len() <= 1190), "every datagram under 1232");
        let mut done = None;
        for p in &packets {
            if let Some(f) = rx.push(p) {
                done = Some(f);
            }
        }
        let f = done.unwrap_or_else(|| panic!("Photon reassembles frame {frame_no} ({size} bytes)"));
        assert_eq!((f.frame_no, f.gains.0, f.bytes.len()), (frame_no, gains.0, size));
        assert!(f.bytes == bytes, "frame {frame_no} bytes");
    }
}

#[test]
fn photon_recovers_lost_packets() {
    let mut tx = cb::BeamTx::new(&secret(), cw::Direction::AnswerToOrigin);
    let mut rx = pb::BeamRx::new(&secret(), pk::Direction::AnswerToOrigin);
    for (frame_no, size) in [(0u32, 20_000usize), (1, 120_000)] {
        let bytes = frame_bytes(size, 99 + frame_no);
        let packets = tx.frame(frame_no, cb::Gains::NONE, &bytes);
        let source_n = size.div_ceil(cb::SYMBOL_BYTES as usize);
        let lose = source_n.div_ceil(cb::REPAIR_PER_SOURCE);
        // Drop `lose` source packets spread through the frame; the repair symbols must stand in for them.
        let step = source_n / lose;
        let kept: Vec<_> = packets.iter().enumerate().filter(|(i, _)| !(*i < source_n && i % step == 0 && i / step < lose)).map(|(_, p)| p).collect();
        assert_eq!(kept.len(), packets.len() - lose);
        let mut done = None;
        for p in kept {
            if let Some(f) = rx.push(p) {
                done = Some(f);
            }
        }
        let f = done.unwrap_or_else(|| panic!("frame {frame_no}: Photon recovers {lose} lost packets of {}", packets.len()));
        assert!(f.bytes == bytes);
    }
}
