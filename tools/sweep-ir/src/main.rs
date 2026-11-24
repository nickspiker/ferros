//! The audio bench's sweep, host side (docs/AUDIO.md "The sweep"; the owner, 2026-11-24: "Doesn't a sweep give you a better idea of device audio latency?").
//! `gen <out> [log2 frames]` writes an exponential sine sweep, 100 Hz to 20 kHz, as little-endian i16 at full scale, for the hand-spawned audio service's `--input-file` (it plays it at the bench's level, at most `TONE_MAX`).
//! `ir <sweep> <dump>` reads the dump the service left in the test mailbox's frame (`ferros-bridge read <frame> 200000 <dump>`), deconvolves each microphone's capture by the sweep (Y·X* / (|X|² + ε), by FFT), and says:
//! - the loopback latency: from the sweep's first frame written into the AoC's ring to its arrival at each microphone, the impulse's peak placed to a fraction of a frame and turned into counter time by the capture's read anchors;
//! - the frequency response in octave bands;
//! - the harmonics, which an exponential sweep puts ahead of the main impulse (Farina, "Simultaneous measurement of impulse response and distortion with a swept-sine technique", AES 2000).

use std::f64::consts::PI;

const RATE: f64 = 48_000.;
const F1: f64 = 100.;
const F2: f64 = 20_000.;
const MICS: usize = 3;
const MARK: u64 = 0x5357_4545_5044_554D;
const ANCHORS: usize = 1 << 13;
const SAMPLES_AT: usize = (8 + 2 * ANCHORS) * 8;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("gen") if a.len() >= 3 => {
            let log: u32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(17);
            let x = sweep(1 << log);
            let bytes: Vec<u8> = x.iter().flat_map(|v| ((v * 32767.).round() as i16).to_le_bytes()).collect();
            std::fs::write(&a[2], bytes).expect("write the sweep");
            println!("sweep: 2^{log} frames ({:.3} s), {F1} Hz to {F2} Hz, written to {}", (1u64 << log) as f64 / RATE, a[2]);
        }
        Some("ir") if a.len() >= 4 => ir(&a[2], &a[3]),
        _ => eprintln!("usage: sweep-ir gen <out> [log2 frames] | sweep-ir ir <sweep> <dump>"),
    }
}

/// Farina's exponential sweep of `n` frames: phase 2π·f1·T/ln(f2/f1)·(e^(t/T·ln(f2/f1)) − 1), with a 2^8-frame raised-cosine fade at each end so it starts and stops at rest.
fn sweep(n: usize) -> Vec<f64> {
    let t = n as f64 / RATE;
    let l = (F2 / F1).ln();
    let fade = 1 << 8;
    (0..n)
        .map(|k| {
            let s = k as f64 / RATE;
            let v = (2. * PI * F1 * t / l * ((s / t * l).exp() - 1.)).sin();
            let e = if k < fade { 0.5 - 0.5 * (PI * k as f64 / fade as f64).cos() } else if k >= n - fade { 0.5 - 0.5 * (PI * (n - 1 - k) as f64 / fade as f64).cos() } else { 1. };
            v * e
        })
        .collect()
}

#[derive(Clone, Copy)]
struct C(f64, f64);
impl C {
    fn mul(self, o: C) -> C {
        C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    fn conj(self) -> C {
        C(self.0, -self.1)
    }
    fn norm(self) -> f64 {
        self.0 * self.0 + self.1 * self.1
    }
}

/// An in-place radix-2 FFT (`inv`: the inverse, scaled by 1/n). `v.len()` a power of two.
fn fft(v: &mut [C], inv: bool) {
    let n = v.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            v.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let a = 2. * PI / len as f64 * if inv { 1. } else { -1. };
        let w = C(a.cos(), a.sin());
        for s in (0..n).step_by(len) {
            let mut u = C(1., 0.);
            for k in 0..len / 2 {
                let p = v[s + k];
                let q = v[s + k + len / 2].mul(u);
                v[s + k] = C(p.0 + q.0, p.1 + q.1);
                v[s + k + len / 2] = C(p.0 - q.0, p.1 - q.1);
                u = u.mul(w);
            }
        }
        len <<= 1;
    }
    if inv {
        for x in v.iter_mut() {
            *x = C(x.0 / n as f64, x.1 / n as f64);
        }
    }
}

fn u64_at(b: &[u8], k: usize) -> u64 {
    u64::from_le_bytes(b[8 * k..8 * k + 8].try_into().unwrap())
}

fn ir(sweep_path: &str, dump_path: &str) {
    let sb = std::fs::read(sweep_path).expect("read the sweep");
    let x: Vec<f64> = sb.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f64 / 32768.).collect();
    let d = std::fs::read(dump_path).expect("read the dump");
    assert_eq!(u64_at(&d, 0), MARK, "the dump's mark: not a bench dump");
    let freq = u64_at(&d, 1) as f64;
    let (tw, ahead, from, frames, anchors, shape) = (u64_at(&d, 2), u64_at(&d, 3), u64_at(&d, 4), u64_at(&d, 5) as usize, u64_at(&d, 6) as usize, u64_at(&d, 7));
    let (ep, lead, per, ms) = (shape & 0xFF, (shape >> 8) & 0xFF, (shape >> 16) & 0xFFFF, shape >> 32);
    println!("dump: entry point {ep}, lead {lead} x {per} frames, capture {ms} ms; {frames} frames from capture frame {from}, {anchors} anchors; the sweep's first frame written with {} frames ahead in the AoC's ring", ahead / 4);
    let anc: Vec<(u64, u64)> = (0..anchors).map(|k| (u64_at(&d, 8 + 2 * k), u64_at(&d, 9 + 2 * k))).collect();
    let n = (frames + x.len()).next_power_of_two();
    let mut xf: Vec<C> = (0..n).map(|k| C(*x.get(k).unwrap_or(&0.), 0.)).collect();
    fft(&mut xf, false);
    let xmax = xf.iter().map(|c| c.norm()).fold(0., f64::max);
    // The regularisation: 60 dB under the sweep's strongest bin, so bins the sweep never excited (under 100 Hz, over 20 kHz) are not divided by nothing.
    let eps = xmax * 1e-6;
    for m in 0..MICS {
        let mut y: Vec<C> = (0..n)
            .map(|k| {
                if k < frames {
                    let at = SAMPLES_AT + 2 * (k * MICS + m);
                    C(i16::from_le_bytes([d[at], d[at + 1]]) as f64 / 32768., 0.)
                } else {
                    C(0., 0.)
                }
            })
            .collect();
        fft(&mut y, false);
        let mut h: Vec<C> = y.iter().zip(xf.iter()).map(|(yy, xx)| { let c = yy.mul(xx.conj()); let k = xx.norm() + eps; C(c.0 / k, c.1 / k) }).collect();
        let resp = h.clone();
        fft(&mut h, true);
        let hm: Vec<f64> = h.iter().map(|c| c.0).collect();
        // The linear impulse: the largest magnitude in the causal half.
        let (p, pk) = hm[..n / 2].iter().enumerate().fold((0, 0f64), |b, (k, v)| if v.abs() > b.1 { (k, v.abs()) } else { b });
        // A parabola thru the peak and its neighbours places it to a fraction of a frame.
        let (a0, a1, a2) = (hm[(p + n - 1) % n].abs(), hm[p].abs(), hm[(p + 1) % n].abs());
        let frac = 0.5 * (a0 - a2) / (a0 - 2. * a1 + a2);
        let pf = p as f64 + if frac.is_finite() { frac } else { 0. };
        // The noise: the median magnitude of the impulse's tail, past 0.1 s after the peak.
        // WHY the min (AGENT.md rule 0): a peak within 0.1 s of the causal half's end leaves no tail past it; the range then starts at the end and is empty (noise 0, said as an infinite margin), never past it.
        let mut tail: Vec<f64> = hm[(p + 4800).min(n / 2)..n / 2].iter().map(|v| v.abs()).collect();
        tail.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let noise = tail.get(tail.len() / 2).copied().unwrap_or(0.);
        // The arrival in the capture's frames, and the read that held it: arrival time = that read's time less the frames that came in after the arrival.
        let arrive = from as f64 + pf;
        let lat = anc.iter().find(|a| a.0 as f64 > arrive).map(|&(total, t)| {
            let ta = t as f64 - (total as f64 - arrive) / RATE * freq;
            (ta - tw as f64) / freq * 1e6
        });
        let ahead_us = (ahead / 4) as f64 / RATE * 1e6;
        print!("mic {m}: impulse at {pf:.2} frames, peak {:.1} dBFS-ish, {:.1} dB over the tail's median", 20. * pk.log10(), 20. * (pk / noise).log10());
        match lat {
            Some(l) => println!("; write to arrival {l:.0} us, {:.0} us past the frames ahead of it ({ahead_us:.0} us)", l - ahead_us),
            None => println!("; no anchor after the arrival"),
        }
        // The harmonics: an exponential sweep puts harmonic k's impulse T·ln(k)/ln(f2/f1) ahead of the linear one.
        let t = x.len() as f64 / RATE;
        let mut hl = String::new();
        for k in 2..=5u32 {
            let dt = t * (k as f64).ln() / (F2 / F1).ln() * RATE;
            let c = (p as f64 - dt).rem_euclid(n as f64) as usize;
            let w = 64;
            // WHY the saturating_sub and the min (AGENT.md rule 0): the window round a harmonic's place is cut at the buffer's ends rather than indexing past them; a harmonic that close to an end is outside the sweep's span anyway.
            let hk = (c.saturating_sub(w)..(c + w).min(n)).map(|i| hm[i].abs()).fold(0., f64::max);
            hl += &format!(" h{k} {:.1} dB", 20. * (hk / pk).log10());
        }
        // The response in octave bands, each band's mean power, relative to 1 kHz's.
        let band = |f: f64| {
            let (lo, hi) = ((f / 2f64.sqrt() / RATE * n as f64) as usize, (f * 2f64.sqrt() / RATE * n as f64) as usize);
            let s: f64 = resp[lo..hi].iter().map(|c| c.norm()).sum::<f64>() / (hi - lo).max(1) as f64;
            10. * s.log10()
        };
        let r1k = band(1000.);
        let mut bl = String::new();
        for f in [125., 250., 500., 1000., 2000., 4000., 8000., 16000.] {
            bl += &format!(" {}:{:+.1}", f as u32, band(f) - r1k);
        }
        println!("  harmonics vs the linear peak:{hl}; response by octave (dB vs 1 kHz):{bl}");
    }
}
