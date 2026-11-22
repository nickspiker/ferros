//! The frame kernel (src/lib.rs `frame_kernel`) in valhall's emulator: every pixel of a three-byte frame into fluor's G2t16 thru a code table, exactly as the CPU's conversion, opaque; the surface past the frame's width untouched, the invocations past its end drawing its last pixel again.

use lumis_gpu::{frame, frame_kernel};
use valhall::sim::{run, Memory};

#[test]
fn the_frame_exactly() {
    let (w, h, stride) = (37usize, 23usize, 40usize);
    let lut: Vec<u32> = (0..256u32).map(|v| 0xFFFF - v * v).collect();
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    let src: Vec<u8> = (0..w * h * 3).map(|_| { s ^= s << 13; s ^= s >> 7; s ^= s << 17; s as u8 }).collect();
    let base = 0x8000_0000u64;
    let (src_at, lut_at, dst_at) = (0usize, 4096usize, 8192usize);
    let mut mem = Memory { base, bytes: vec![0u8; dst_at + stride * h * 8] };
    mem.bytes[src_at..src_at + src.len()].copy_from_slice(&src);
    for (i, d) in lut.iter().enumerate() {
        mem.bytes[lut_at + 4 * i..lut_at + 4 * i + 4].copy_from_slice(&d.to_le_bytes());
    }
    let k = frame_kernel();
    let mut u = vec![0u64; frame::SLOT_COUNT as usize];
    u[frame::DST as usize] = base + dst_at as u64;
    u[frame::SRC as usize] = base + src_at as u64;
    u[frame::LUT as usize] = base + lut_at as u64;
    u[frame::SIZE as usize] = w as u64 | ((w * h) as u64) << 32;
    u[frame::STRIDE as usize] = stride as u64;
    u.extend_from_slice(&k.consts);
    for id in 0..(w * h + 15) as u32 {
        run(&k.words, &u, [id, 0], &mut mem);
    }
    for y in 0..h {
        for x in 0..stride {
            let i = y * stride + x;
            let got = u64::from_le_bytes(mem.bytes[dst_at + 8 * i..dst_at + 8 * i + 8].try_into().unwrap());
            let want = if x < w {
                let p = (y * w + x) * 3;
                (0xFFFFu64 << 48) | (lut[src[p] as usize] as u64) << 32 | (lut[src[p + 1] as usize] as u64) << 16 | lut[src[p + 2] as usize] as u64
            } else {
                0
            };
            assert_eq!(got, want, "({x}, {y})");
        }
    }
}
