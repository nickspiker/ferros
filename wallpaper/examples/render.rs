//! Renders a seed's wallpaper to a VSF visual to look at: one Display plane, VSF RGB, gamma2tanh at 16 bits, written exactly as tools/screencap writes the screen.
//! Usage: render <seed> <out.vsf> [width height]   (seed: 64 hex digits, or a number spread by `seed_of`; default 1344 2992)
use vsf::colour::convert::delinearize_gamma2tanh_bits_f32;
use vsf::types::BitPackedTensor;
use vsf::visual::{Encoding, Plane, Primaries, Visual};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        eprintln!("usage: render <seed: 64 hex digits or a number> <out.vsf> [width height]");
        std::process::exit(2);
    }
    let seed = if a[1].len() == 64 {
        let mut s = [0u8; 32];
        for (k, b) in s.iter_mut().enumerate() {
            *b = u8::from_str_radix(&a[1][k * 2..k * 2 + 2], 16).expect("hex seed");
        }
        s
    } else {
        wallpaper::seed_of(a[1].parse().expect("seed number"))
    };
    let w: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1344);
    let h: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(2992);
    let t0 = std::time::Instant::now();
    let p = wallpaper::params(&seed, w, h);
    let t_params = t0.elapsed();
    let mut px = vec![0u64; w * h];
    let t1 = std::time::Instant::now();
    wallpaper::render_rows(&p, 0..h, &mut px);
    let t_render = t1.elapsed();
    // Planar [3, h, w]: R plane, G plane, B plane.
    let mut samples = vec![0u16; 3 * w * h];
    for i in 0..w * h {
        let p = px[i];
        for c in 0..3 {
            let v = (p >> (16 * c)) as u16 as f32 / 16384.;
            samples[c * w * h + i] = delinearize_gamma2tanh_bits_f32(v * v, 16) as u16;
        }
    }
    let mut plane = Plane::encoded("display", "screen", w, h, Primaries::Vsf, Encoding::Gamma2Tanh, BitPackedTensor::pack(16, vec![3, h, w], &samples));
    plane.name = "wallpaper".into();
    let v = Visual { title: "ferros wallpaper".into(), text: "A seed's default wallpaper, the display's 16-bit working values.".into(), planes: vec![plane], ..Default::default() };
    let file = v.write().expect("visual");
    std::fs::write(&a[2], &file).expect("write");
    println!("{}: {w}x{h}, c = {:.5} {:+.5}i, attempt {}, {:?}, params {} ms, render {} ms (one thread)", a[2], p.c[0], p.c[1], p.attempt, p.survey, t_params.as_millis(), t_render.as_millis());
}
