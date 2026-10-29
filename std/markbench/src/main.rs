//! markbench: the attest mark's render cost (tapuwae), on the phone and the desktop alike. Spawned as any payload: `ferros-bridge spawn markbench.bin --name markbench --entropy`; everything it prints is the console's.
//! For each disc size and sampling scheme: the best of 2² renders, in milliseconds, on one core and on all of them, with the samples a pixel took on average.

use std::time::Instant;
use tapuwae::{Aa, Look, Params};

/// A stand-in device salt (the same stand-ins attest-mark steps through).
fn salt(dev: u32) -> [u8; 32] {
    *blake3::Hasher::new().update(b"ferros attest mark dev salt 0000").update(&dev.to_le_bytes()).finalize().as_bytes()
}

/// The best of 2² renders of a `size`-pixel disc, in milliseconds, and the mean samples per pixel.
fn time(size: usize, p: &Params, look: &Look) -> (f32, f32) {
    let mut buf = vec![0u32; size * size];
    let h = size as f32 * 0.5;
    let mut best = f32::MAX;
    let mut spp = 0.;
    for _ in 0..(1 << 2) {
        let t = Instant::now();
        let hist = tapuwae::render_disc_counted(&mut buf, size, h, h, h - 2., p, look);
        best = best.min(t.elapsed().as_secs_f32() * 1000.);
        let px: u64 = hist.iter().sum();
        spp = hist.iter().enumerate().map(|(m, &n)| m as u64 * n).sum::<u64>() as f32 / px as f32;
    }
    (best, spp)
}

fn main() {
    let all = rayon::current_num_threads();
    println!("markbench: {all} threads");
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let schemes: [(&str, Aa); 4] = [("linear (now)", Aa::Linear), ("fixed 4", Aa::Fixed(4)), ("fixed 16", Aa::Fixed(16)), ("fixed 64", Aa::Fixed(64))];
    // Device 9 sparse (long arcs), 6 a dense web.
    for dev in [9u32, 6] {
        let p = Params::new(&salt(dev), "ab");
        for size in [1usize << 8, 384, 1 << 10] {
            for (name, aa) in &schemes {
                let look = Look { aa: *aa, ..Look::attest() };
                // Fixed 64 at the full size takes long on one core: all cores only.
                let single = if size >= 1 << 10 && *aa == Aa::Fixed(64) { None } else { Some(one.install(|| time(size, &p, &look))) };
                let (ms_all, spp) = time(size, &p, &look);
                match single {
                    Some((ms1, _)) => println!("dev {dev} {size:>4}px {name:<13} spp {spp:>6.2}  1 core {ms1:>9.1} ms  {all} cores {ms_all:>8.1} ms  ({:.1}x)", ms1 / ms_all),
                    None => println!("dev {dev} {size:>4}px {name:<13} spp {spp:>6.2}  1 core         -     {all} cores {ms_all:>8.1} ms"),
                }
            }
        }
    }
    println!("markbench: done");
}
