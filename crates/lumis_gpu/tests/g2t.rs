//! The conversion kernel (src/lib.rs `to_g2t16_kernel`) in valhall's emulator against the same conversion on the CPU: every pixel of a rectangle of 8-bit codes into fluor's G2t16 thru a code table, exact; the pixels outside the rectangle untouched.

use lumis_gpu::{g2t, to_g2t16_kernel};
use valhall::sim::{run, Memory};

#[test]
fn codes_to_g2t16_exactly() {
    let (w, h) = (37usize, 23usize);
    let (x0, y0, run_, rise) = (5usize, 3usize, 29usize, 17usize);
    // A darkness table as std/lumis builds it, any 16-bit values: here a falling curve.
    let lut: Vec<u32> = (0..256u32).map(|v| 0xFFFF - (v * v)).collect();
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    let src: Vec<u32> = (0..w * h).map(|_| { s ^= s << 13; s ^= s >> 7; s ^= s << 17; (s as u32) & 0xFF_FFFF }).collect();
    let base = 0x8000_0000u64;
    let (src_at, lut_at, dst_at) = (0usize, w * h * 4, w * h * 4 + 1024);
    let mut mem = Memory { base, bytes: vec![0u8; dst_at + w * h * 8] };
    for (i, p) in src.iter().enumerate() {
        mem.bytes[src_at + 4 * i..src_at + 4 * i + 4].copy_from_slice(&p.to_le_bytes());
    }
    for (i, d) in lut.iter().enumerate() {
        mem.bytes[lut_at + 4 * i..lut_at + 4 * i + 4].copy_from_slice(&d.to_le_bytes());
    }
    let k = to_g2t16_kernel();
    let wu = |a: u64, b: u64| a | b << 32;
    let mut uniforms = vec![0u64; g2t::SLOTS as usize];
    uniforms[g2t::SRC as usize] = base + src_at as u64;
    uniforms[g2t::DST as usize] = base + dst_at as u64;
    uniforms[g2t::LUT as usize] = base + lut_at as u64;
    uniforms[g2t::ORIGIN as usize] = wu(x0 as u64, y0 as u64);
    uniforms[g2t::SIZE as usize] = wu(run_ as u64, rise as u64);
    uniforms[g2t::STRIDES as usize] = wu(w as u64, w as u64);
    uniforms.extend_from_slice(&k.consts);
    for id in 0..(run_ * rise) as u32 {
        run(&k.words, &uniforms, [id, 0], &mut mem);
    }
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let got = u64::from_le_bytes(mem.bytes[dst_at + 8 * i..dst_at + 8 * i + 8].try_into().unwrap());
            let inside = (x0..x0 + run_).contains(&x) && (y0..y0 + rise).contains(&y);
            let p = src[i];
            let want = if inside { (0xFFFFu64 << 48) | (lut[(p & 0xFF) as usize] as u64) << 32 | (lut[(p >> 8 & 0xFF) as usize] as u64) << 16 | lut[(p >> 16 & 0xFF) as usize] as u64 } else { 0 };
            assert_eq!(got, want, "pixel ({x}, {y})");
        }
    }
}
