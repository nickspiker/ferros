//! Surveys many seeds at the device's size and reports how each composed: whether it passed, after how many attempts, its coverage, and the time to choose it.
//! Usage: validate [count] [width height]   (default 256 seeds, 1344 2992)
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let count: u64 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(256);
    let w: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1344);
    let h: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(2992);
    let (mut pass, mut attempts, mut worst_ms, mut total_ms) = (0, [0u32; 33], 0f64, 0f64);
    let (mut near_lo, mut near_hi, mut third_lo) = (1f32, 0f32, 1f32);
    for n in 0..count {
        let t = std::time::Instant::now();
        let p = wallpaper::params(&wallpaper::seed_of(n), w, h);
        let ms = t.elapsed().as_secs_f64() * 1000.;
        let s = p.survey;
        if s.passes() {
            pass += 1;
        } else {
            println!("seed {n} kept its best without passing: {s:?}");
        }
        attempts[p.attempt as usize] += 1;
        near_lo = near_lo.min(s.near);
        near_hi = near_hi.max(s.near);
        third_lo = third_lo.min(s.near_min_third);
        worst_ms = worst_ms.max(ms);
        total_ms += ms;
    }
    println!("{pass}/{count} passed; near {near_lo:.3}..{near_hi:.3}, emptiest third at least {third_lo:.3}; params {:.1} ms mean, {worst_ms:.1} ms worst", total_ms / count as f64);
    for (k, &c) in attempts.iter().enumerate().filter(|(_, c)| **c > 0) {
        println!("  kept attempt {k}: {c} seeds");
    }
}
