//! The host half of the UDP latency bench (payloads/udplat on the device; docs/WIFI.md "Datagrams at speed").
//!
//! udplat <device IPv4> <spacing µs> <count> [--size N] [--bind A] [--bulk burst,size,period_us] [--sink port] [--class 0|1|2]
//!
//! Sends `count` echo datagrams to the device's port 7778, one every `spacing` µs, and reads the echoes back.
//! Each echo carries the device's counter when its receive returned and just before its send, so the round trip splits into the two one-way paths: host to the device process (wire, chip, pcie1, ip, the process's receive) and the device process to the host.
//! The two clocks are not joined; each direction's delay is told as what it added over that direction's least, after the clocks' drift is taken out (the least delays of the first and last eighth of the run), so the numbers are the queueing and waking a path adds, not the wire's own time.
//! With --bulk the device sends a beam's load alongside (a burst of `burst` datagrams of `size` bytes every `period` µs, to the sink port, counted here).

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PORT: u16 = 7778;

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    v[i]
}

fn line(name: &str, v: &[f64]) {
    let mut w = v.to_vec();
    let mean = w.iter().sum::<f64>() / w.len().max(1) as f64;
    println!(
        "  {name:<28} p50 {:>8.1}  p90 {:>8.1}  p99 {:>8.1}  max {:>8.1}  mean {:>8.1} us",
        pct(&mut w, 0.5),
        pct(&mut w, 0.9),
        pct(&mut w, 0.99),
        pct(&mut w, 1.),
        mean
    );
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: udplat <device IPv4> <spacing us> <count> [--size N] [--bind A] [--bulk burst,size,period_us] [--sink port]");
        std::process::exit(1);
    }
    let dev: std::net::Ipv4Addr = a[1].parse().expect("device IPv4");
    let spacing = Duration::from_micros(a[2].parse().expect("spacing"));
    let count: u32 = a[3].parse().expect("count");
    let mut size = 120usize;
    let mut bind = "0.0.0.0".to_string();
    let mut bulk: Option<(u16, u16, u32)> = None;
    let mut sink_port = 7780u16;
    let mut class = 0u8;
    let mut i = 4;
    while i < a.len() {
        match a[i].as_str() {
            "--size" => size = a[i + 1].parse().unwrap(),
            "--bind" => bind = a[i + 1].clone(),
            "--sink" => sink_port = a[i + 1].parse().unwrap(),
            "--class" => class = a[i + 1].parse().unwrap(),
            "--bulk" => {
                let p: Vec<u32> = a[i + 1].split(',').map(|x| x.parse().unwrap()).collect();
                bulk = Some((p[0] as u16, p[1] as u16, p[2]));
            }
            x => panic!("unknown {x}"),
        }
        i += 2;
    }
    let size = size.max(48);
    let to = SocketAddr::from((dev, PORT));
    let sock = Arc::new(UdpSocket::bind((bind.as_str(), 7779)).expect("bind 7779"));
    let t0 = Instant::now();
    let ns = move || t0.elapsed().as_nanos() as u64;
    let done = Arc::new(AtomicBool::new(false));

    // The bulk sink: opened toward the device first (the box's firewall keeps only flows it saw go out), then counted.
    let bulk_n = Arc::new(AtomicU64::new(0));
    let bulk_bytes = Arc::new(AtomicU64::new(0));
    let mut sink_thread = None;
    if let Some((burst, bsize, period)) = bulk {
        let sink = UdpSocket::bind((bind.as_str(), sink_port)).expect("bind sink");
        sink.send_to(b"H", to).unwrap();
        sink.set_read_timeout(Some(Duration::from_millis(250))).unwrap();
        let (n, b, d) = (bulk_n.clone(), bulk_bytes.clone(), done.clone());
        sink_thread = Some(std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while !d.load(Ordering::Relaxed) {
                if let Ok(k) = sink.recv(&mut buf) {
                    n.fetch_add(1, Ordering::Relaxed);
                    b.fetch_add(k as u64, Ordering::Relaxed);
                }
            }
        }));
        let bursts = ((spacing.as_micros() as u64 * count as u64) / period as u64 + 64) as u32;
        let mut m = vec![b'B'];
        m.extend_from_slice(&burst.to_le_bytes());
        m.extend_from_slice(&bsize.to_le_bytes());
        m.extend_from_slice(&period.to_le_bytes());
        m.extend_from_slice(&bursts.to_le_bytes());
        m.extend_from_slice(&sink_port.to_le_bytes());
        sock.send_to(&m, to).unwrap();
        std::thread::sleep(Duration::from_millis(256));
    }

    // Each echo: (host send ns, host arrival ns, device receive ticks, device send ticks, freq, the previous send call's ticks).
    let rec: Arc<Mutex<Vec<Option<(u64, u64, u64, u64, u64, u64)>>>> = Arc::new(Mutex::new(vec![None; count as usize]));
    let sent_at: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(vec![0; count as usize]));
    sock.set_read_timeout(Some(Duration::from_millis(250))).unwrap();
    let rx = {
        let (s, r, d) = (sock.clone(), rec.clone(), done.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while !d.load(Ordering::Relaxed) {
                let Ok((k, _)) = s.recv_from(&mut buf) else { continue };
                let t = ns();
                if k < 48 || buf[0] != b'E' {
                    continue;
                }
                let u = |o: usize| u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
                let seq = u32::from_le_bytes(buf[1..5].try_into().unwrap()) as usize;
                if seq < r.lock().unwrap().len() {
                    r.lock().unwrap()[seq] = Some((u(5), t, u(16), u(24), u(32), u(40)));
                }
            }
        })
    };
    let mut m = vec![0u8; size];
    m[0] = b'E';
    m[13] = class;
    let start = Instant::now();
    for seq in 0..count {
        let due = start + spacing * seq;
        while Instant::now() < due {
            let left = due - Instant::now();
            if left > Duration::from_micros(200) {
                std::thread::sleep(left - Duration::from_micros(150));
            }
        }
        m[1..5].copy_from_slice(&seq.to_le_bytes());
        let t = ns();
        m[5..13].copy_from_slice(&t.to_le_bytes());
        sent_at.lock().unwrap()[seq as usize] = t;
        sock.send_to(&m, to).unwrap();
    }
    std::thread::sleep(Duration::from_millis(512));
    done.store(true, Ordering::Relaxed);
    rx.join().unwrap();
    if let Some(t) = sink_thread {
        t.join().unwrap();
    }

    let rec = rec.lock().unwrap();
    let got: Vec<_> = rec.iter().flatten().copied().collect();
    let lost = count as usize - got.len();
    if got.is_empty() {
        println!("no echoes ({count} sent)");
        return;
    }
    let f = got[0].4 as f64;
    let us = |ticks: u64| ticks as f64 * 1e6 / f;
    // Raw one-way delays in µs with an unknown constant offset each: forward = device receive - host send, back = host arrival - device send.
    let fwd: Vec<(f64, f64)> = got.iter().map(|g| (g.0 as f64 / 1e3, us(g.2) - g.0 as f64 / 1e3)).collect();
    let back: Vec<(f64, f64)> = got.iter().map(|g| (g.1 as f64 / 1e3, g.1 as f64 / 1e3 - us(g.3))).collect();
    // The drift between the clocks, from the least delays of the first and last eighth.
    let slope = |v: &[(f64, f64)]| {
        let e = (v.len() / 8).max(1);
        let lo = |s: &[(f64, f64)]| s.iter().cloned().fold((0., f64::MAX), |m, x| if x.1 < m.1 { x } else { m });
        let (a, b) = (lo(&v[..e]), lo(&v[v.len() - e..]));
        if b.0 > a.0 { (b.1 - a.1) / (b.0 - a.0) } else { 0. }
    };
    // Forward drifts by +s, back by -s: one estimate from both.
    let s = (slope(&fwd) - slope(&back)) / 2.;
    let flat = |v: &[(f64, f64)], s: f64| {
        let w: Vec<f64> = v.iter().map(|x| x.1 - s * x.0).collect();
        let m = w.iter().cloned().fold(f64::MAX, f64::min);
        w.iter().map(|x| x - m).collect::<Vec<f64>>()
    };
    let fwd_add = flat(&fwd, s);
    let back_add = flat(&back, -s);
    let rtt: Vec<f64> = got.iter().map(|g| (g.1 - g.0) as f64 / 1e3).collect();
    let turn: Vec<f64> = got.iter().map(|g| us(g.3 - g.2)).collect();
    let call: Vec<f64> = got.iter().skip(1).map(|g| us(g.5)).collect();
    let net: Vec<f64> = got.iter().map(|g| (g.1 - g.0) as f64 / 1e3 - us(g.3 - g.2)).collect();
    // The device's send pacing as the host saw it: the gaps between echoes landing, against the spacing.
    let mut gaps = Vec::new();
    for w in got.windows(2) {
        gaps.push(((w[1].1 - w[0].1) as f64 / 1e3 - (w[1].0 - w[0].0) as f64 / 1e3).abs());
    }
    let secs = spacing.as_secs_f64() * count as f64;
    println!(
        "{} echoes of {size} bytes at {} us spacing ({:.0}/s each way), {} lost ({:.2}%), clocks drift {:.1} ppm",
        count,
        spacing.as_micros(),
        1. / spacing.as_secs_f64(),
        lost,
        100. * lost as f64 / count as f64,
        s * 1e6
    );
    line("round trip", &rtt);
    line("round trip less the device", &net);
    line("device turnaround", &turn);
    line("device send call", &call);
    line("added, host to process", &fwd_add);
    line("added, process to host", &back_add);
    line("landing gap off spacing", &gaps);
    if bulk.is_some() {
        let n = bulk_n.load(Ordering::Relaxed);
        let b = bulk_bytes.load(Ordering::Relaxed);
        println!("  bulk alongside: {n} datagrams, {:.2} Mbit/s over the run ({:.0}/s)", b as f64 * 8. / secs / 1e6, n as f64 / secs);
    }
}
