//! photon_core's BeamRx against Photon's, differentially: one hostile packet stream (loss, reordering, duplicates, corruption, stale frames, an orientation and an info change) into both, and after every packet they must agree on what they hand out and on every counter.

use photon::wave::beam as pb;
use photon::wave::keys as pk;
use photon_core::beam as cb;
use photon_core::wave as cw;

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

#[test]
fn receivers_agree_on_a_hostile_stream() {
    let secret = cw::derive_wave_secret(&[5; 32], &[6; 16], &[7; 32], &[8; 32]);
    let mut tx = cb::BeamTx::new(&secret, cw::Direction::OriginToAnswer);
    let mut ours = cb::BeamRx::new(&secret, cw::Direction::OriginToAnswer);
    let mut theirs = pb::BeamRx::new(&secret, pk::Direction::OriginToAnswer);
    let mut rng = Lcg(42);
    let mut stream: Vec<Vec<u8>> = Vec::new();
    stream.push(tx.info(&cb::Info { width: 320, height: 240, fps: 30, codec: cb::CODEC_H264, colour: vec![] }));
    for f in 0..48u32 {
        if f == 20 {
            stream.push(tx.orient(3));
            stream.push(tx.info(&cb::Info { width: 640, height: 480, fps: 15, codec: cb::CODEC_H264, colour: vec![1, 2, 3] }));
        }
        let size = 1 + (rng.next() as usize % 40_000);
        let bytes: Vec<u8> = (0..size).map(|_| rng.next() as u8).collect();
        let mut pkts = tx.frame(f, cb::Gains([4096, 4096 + f as u16, 4000, 4100]), &bytes);
        // Shuffle within the frame, drop some, duplicate some.
        for i in (1..pkts.len()).rev() {
            pkts.swap(i, rng.next() as usize % (i + 1));
        }
        for p in pkts {
            match rng.next() % 20 {
                0 | 1 => {}
                2 => {
                    stream.push(p.clone());
                    stream.push(p);
                }
                3 => {
                    let mut bad = p;
                    let i = rng.next() as usize % bad.len();
                    bad[i] ^= 0x40;
                    stream.push(bad);
                }
                _ => stream.push(p),
            }
        }
    }
    // Late packets from long-gone frames, and junk.
    let late = tx.frame(1, cb::Gains::NONE, &[9u8; 3000]);
    stream.extend(late);
    stream.push(vec![0xCA, 0, 0, 0, 0, 1, 2, 3, 4, 5]);
    stream.push(b"RX not a beam".to_vec());
    let mut handed = 0;
    for (i, p) in stream.iter().enumerate() {
        let (a, b) = (ours.push(p), theirs.push(p));
        assert_eq!(a.as_ref().map(|f| (f.frame_no, f.gains.0, &f.bytes)), b.as_ref().map(|f| (f.frame_no, f.gains.0, &f.bytes)), "packet {i}: frames handed out");
        handed += a.is_some() as u32;
        let (ia, ib) = (ours.take_info_change(), theirs.take_info_change());
        assert_eq!(ia.map(|x| (x.width, x.height, x.fps, x.colour)), ib.map(|x| (x.width, x.height, x.fps, x.colour)), "packet {i}: info change");
        assert_eq!((ours.rotation, ours.frames_done, ours.frames_lost, ours.bad_packets, ours.open_frames()), (theirs.rotation, theirs.frames_done, theirs.frames_lost, theirs.bad_packets, theirs.open_frames()), "packet {i}: counters");
    }
    println!("{} packets, {handed} frames handed out, {} lost, {} bad — identical in both receivers", stream.len(), ours.frames_lost, ours.bad_packets);
    // About 15% of packets are lost or corrupted against 12.5% repair, so many frames complete and many cannot.
    assert!(handed > 15 && ours.frames_lost > 10 && ours.bad_packets > 0, "the stream exercised every path");
}
