//! The stack kernel (src/lib.rs `stack_kernel`) in valhall's emulator against the CPU's stacking: a RAW10 frame with its line's padding unpacked as std/lumis's rows are and written into a plane, then two more added, every sample exact, nothing past the plane and its slack touched (the frame here is 21 chunks, so 11 invocations past it).

use lumis_gpu::{stack, stack_kernel};
use valhall::sim::{run, Memory};

fn unpack(line: &[u8], out: &mut [u16]) {
    for (px, b) in out.chunks_exact_mut(4).zip(line.chunks_exact(5)) {
        for i in 0..4 {
            px[i] = (b[i] as u16) << 2 | ((b[4] >> (2 * i)) & 3) as u16;
        }
    }
}

#[test]
fn frames_stack_exactly() {
    // A 48 x 7 frame: 3 chunks of 16 samples a row (60 bytes), each line 64 bytes (4 of padding).
    let (w, h, line) = (48usize, 7usize, 64usize);
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    let mut frame = || -> Vec<u8> { (0..line * h).map(|_| { s ^= s << 13; s ^= s >> 7; s ^= s << 17; s as u8 }).collect() };
    let frames = [frame(), frame(), frame()];
    let base = 0x8000_0000u64;
    let (src_at, plane_at) = (0usize, 4096usize);
    let mut mem = Memory { base, bytes: vec![0u8; plane_at + w * h * 2 + stack::SLACK + 64] };
    mem.bytes[plane_at + w * h * 2 + stack::SLACK..].fill(0xA5);
    let mut want = vec![0u32; w * h];
    let mut row = vec![0u16; w];
    let chunks = w / stack::CHUNK * h;
    for (k, f) in frames.iter().enumerate() {
        mem.bytes[src_at..src_at + f.len()].copy_from_slice(f);
        let kern = stack_kernel(k > 0);
        let mut u = vec![0u64; stack::SLOT_COUNT as usize];
        u[stack::SRC as usize] = base + src_at as u64;
        u[stack::PLANE as usize] = base + plane_at as u64;
        u[stack::SIZE as usize] = (w / stack::CHUNK) as u64 | (chunks as u64) << 32;
        u[stack::LINES as usize] = (line / 4) as u64 | (w as u64) << 32;
        u.extend_from_slice(&kern.consts);
        for id in 0..(chunks.div_ceil(16) * 16) as u32 {
            run(&kern.words, &u, [id, 0], &mut mem);
        }
        for y in 0..h {
            unpack(&f[y * line..y * line + w / 4 * 5], &mut row);
            for x in 0..w {
                want[y * w + x] = if k == 0 { row[x] as u32 } else { want[y * w + x] + row[x] as u32 };
            }
        }
    }
    for i in 0..w * h {
        let got = u16::from_le_bytes([mem.bytes[plane_at + 2 * i], mem.bytes[plane_at + 2 * i + 1]]);
        assert_eq!(got as u32, want[i], "sample {i}");
    }
    assert!(mem.bytes[plane_at + w * h * 2 + stack::SLACK..].iter().all(|&b| b == 0xA5), "past the plane and its slack untouched");
}
