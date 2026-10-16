//! beam-recv: receive the phone's test beam (payloads/beamtest) with nothing but Photon's own library — its BeamRx, its media packet layer for the audio, its OpenH264 decoder — under the same fixed TEST wave secret.
//!
//! Usage: beam-recv [port = 4393] [out dir = .] [idle seconds = 20] [phone address]. Writes frame-NNNN.ppm for every 30th decoded picture and the last, and prints stats once a second.
//!
//! With a phone address it sends a small datagram there from the listening port once a second until the beam arrives: a stateful firewall then sees the phone's stream (same ports, reversed) as replies on an established flow, not as unsolicited inbound UDP.

use photon::wave::{beam as pb, h264 as ph, keys as pk, packet as pp};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

fn ppm(path: &str, p: &ph::I420) {
    let mut out = format!("P6\n{} {}\n255\n", p.w, p.h).into_bytes();
    for y in 0..p.h {
        for x in 0..p.w {
            let (l, cb, cr) = (p.y[y * p.w + x] as f32, p.u[(y / 2) * (p.w / 2) + x / 2] as f32 - 128.0, p.v[(y / 2) * (p.w / 2) + x / 2] as f32 - 128.0);
            for c in [l + 1.402 * cr, l - 0.344136 * cb - 0.714136 * cr, l + 1.772 * cb] {
                out.push(c.clamp(0.0, 255.0) as u8);
            }
        }
    }
    std::fs::write(path, out).expect("write ppm");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(4393);
    let dir = args.get(2).cloned().unwrap_or_else(|| ".".into());
    let idle = Duration::from_secs(args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20));
    let phone: Option<std::net::SocketAddr> = args.get(4).map(|a| format!("{a}:{port}").parse().expect("phone address"));
    let mut punched = Instant::now() - Duration::from_secs(2);
    let secret = pk::derive_wave_secret(&[0xFE; 32], &[0xBE; 16], &[0xA7; 32], &[0xC0; 32]);
    let sock = UdpSocket::bind(("0.0.0.0", port)).expect("bind");
    sock.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    let mut rx = pb::BeamRx::new(&secret, pk::Direction::OriginToAnswer);
    let mut audio = pk::StepChain::new(&secret, pk::Direction::OriginToAnswer);
    let mut dec = ph::Decoder::new().expect("decoder");
    let (mut packets, mut decoded, mut dec_err, mut audio_ok, mut audio_bad, mut exact_card) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut from = None;
    let mut last_pkt = Instant::now();
    let mut tick = Instant::now();
    let mut last_pic: Option<ph::I420> = None;
    println!("beam-recv: listening on UDP {port}");
    let mut buf = [0u8; 2048];
    loop {
        if let Some(p) = phone {
            if packets == 0 && punched.elapsed() >= Duration::from_secs(1) {
                punched = Instant::now();
                let _ = sock.send_to(b"beam-recv", p);
            }
        }
        if let Ok((n, src)) = sock.recv_from(&mut buf) {
            packets += 1;
            last_pkt = Instant::now();
            from.get_or_insert(src);
            let wire = &buf[..n];
            match wire.first() {
                Some(&pp::BEAM_MAGIC) => {
                    if let Some(info) = rx.take_info_change().or(None) {
                        println!("info: {}x{} at {} fps", info.width, info.height, info.fps);
                    }
                    if let Some(frame) = rx.push(wire) {
                        if let Some(info) = rx.take_info_change() {
                            println!("info: {}x{} at {} fps", info.width, info.height, info.fps);
                        }
                        match dec.decode(&frame.bytes) {
                            Ok(Some(pic)) => {
                                decoded += 1;
                                if frame.frame_no % 30 == 0 {
                                    ppm(&format!("{dir}/frame-{:04}.ppm", frame.frame_no), &pic);
                                }
                                // The card's moving block is pure 235 luma at row band 4: a quick per-frame sanity check that real pixels arrived.
                                let bx = (frame.frame_no as usize % (pic.w / 16)) * 16 + 8;
                                if pic.y[(4 * 16 + 8) * pic.w + bx] == 235 {
                                    exact_card += 1;
                                }
                                last_pic = Some(pic);
                            }
                            Ok(None) => {}
                            Err(e) => {
                                dec_err += 1;
                                eprintln!("decode: {e}");
                            }
                        }
                    }
                }
                Some(&pp::MEDIA_MAGIC) => match pp::parse_header(wire).and_then(|(h, s)| pp::open(&mut audio, &h, s)) {
                    Some(p) if p.first() == Some(&4) && (p.len() == 493 || p.len() == 504) => audio_ok += 1,
                    _ => audio_bad += 1,
                },
                _ => {}
            }
        }
        if tick.elapsed() >= Duration::from_secs(1) && packets > 0 {
            tick = Instant::now();
            println!("from {:?}: {packets} datagrams, {} frames done, {} lost, {} bad, {decoded} decoded ({exact_card} card-checked), {dec_err} decode errors, audio {audio_ok} ok / {audio_bad} bad", from.unwrap(), rx.frames_done, rx.frames_lost, rx.bad_packets);
        }
        if packets > 0 && last_pkt.elapsed() > idle || packets == 0 && last_pkt.elapsed() > idle * 6 {
            break;
        }
    }
    if let Some(p) = last_pic {
        ppm(&format!("{dir}/frame-last.ppm"), &p);
    }
    println!("FINAL: {packets} datagrams, {} frames done, {} lost, {} bad, {decoded} decoded ({exact_card} card-checked), {dec_err} decode errors, audio {audio_ok} ok / {audio_bad} bad", rx.frames_done, rx.frames_lost, rx.bad_packets);
}
