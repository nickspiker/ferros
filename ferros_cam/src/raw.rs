//! The front end's RAW10 frame and its binning for the `camera` endpoint (docs/CAMERA.md "The endpoint": FRAME, `bin` x `bin` same-colour samples averaged, 16 bits a sample, row by row).
//! MIPI RAW10 packed: four pixels in five bytes, the four most significant bytes then one byte of the four pairs of low bits, a line at a 256-byte stride (G#1400 for 4080 pixels; docs/oracle/CAMERA.md "First frame").

/// One RAW10 line unpacked to 16-bit samples, `px.len()` of them (a multiple of four).
pub fn unpack(line: &[u8], px: &mut [u16]) {
    for (q, p) in px.chunks_exact_mut(4).enumerate() {
        let b = &line[q * 5..q * 5 + 5];
        let lsb = b[4] as u16;
        p[0] = (b[0] as u16) << 2 | lsb & 3;
        p[1] = (b[1] as u16) << 2 | lsb >> 2 & 3;
        p[2] = (b[2] as u16) << 2 | lsb >> 4 & 3;
        p[3] = (b[3] as u16) << 2 | lsb >> 6 & 3;
    }
}

/// One binned output row, `out.len() / 2` samples, from the 2 x `bin` unpacked lines of its row pair (`pix`, `w` samples a line, line r at `r * w`): output sample ox is the mean of the `bin` x `bin` same-colour samples of its block, so the mosaic keeps its phase (`phase` 0 the pair's even output row, 1 its odd). `bin` a power of two, at most 8; `acc` a line of `w` samples to work in.
/// The phase's `bin` lines are summed first, sample by sample (straight loops the compiler vectorises), then each output adds its `bin` same-colour samples of that sum. Sixteen bits hold every sum: at most 8 x 8 samples of 1023, 65472.
pub fn bin_row(pix: &[u16], w: usize, bin: usize, phase: usize, acc: &mut [u16], out: &mut [u8]) {
    let shift = 2 * bin.trailing_zeros();
    let acc = &mut acc[..w];
    acc.copy_from_slice(&pix[phase * w..(phase + 1) * w]);
    for j in 1..bin {
        let line = &pix[(phase + 2 * j) * w..(phase + 2 * j + 1) * w];
        for (a, &p) in acc.iter_mut().zip(line) {
            *a += p;
        }
    }
    // Output pair k (its two colours) covers 2 x bin samples of the summed line from 2 x bin x k: colour c at offsets c, c + 2, ..
    for (k, o) in out.chunks_exact_mut(4).enumerate() {
        let block = &acc[2 * bin * k..2 * bin * (k + 1)];
        let (mut s0, mut s1) = (0u16, 0u16);
        for pair in block.chunks_exact(2) {
            s0 += pair[0];
            s1 += pair[1];
        }
        o[..2].copy_from_slice(&(s0 >> shift).to_le_bytes());
        o[2..].copy_from_slice(&(s1 >> shift).to_le_bytes());
    }
}

// ---------------------------------------------------------------- the accumulate (owner, 2026-11-15: "straight add/accumulate and see what numbers we can hit")

/// Samples a block: eight RAW10 groups, 40 bytes, unpacked and added 32 at a time.
pub const BLOCK_SAMPLES: usize = 32;
pub const BLOCK_BYTES: usize = 40;
/// Bytes a line must hold past its last block: each block is read as three 16-byte loads (48 bytes) and uses the first 40.
pub const SLACK: usize = 8;

/// The blocks a line of `w` samples takes (the accumulator's row pitch is `blocks * 32` samples: the main's 4080 make 128 blocks, 4096 samples, the last 16 its stride's padding).
pub const fn blocks(w: usize) -> usize {
    (w + BLOCK_SAMPLES - 1) / BLOCK_SAMPLES
}

/// The table lookups that turn a block's 40 bytes into its 32 samples: for output sample i, its high byte at 5 (i / 4) + i % 4 and its low-bits byte at 5 (i / 4) + 4; the low bits then shifted right by 2 (i % 4) and masked (`LOW_SHIFT`), the high byte shifted left by 2, or-ed.
const HIGH_IDX: [[u8; 16]; 2] = {
    let mut t = [[0u8; 16]; 2];
    let mut i = 0;
    while i < 32 {
        t[i / 16][i % 16] = (5 * (i / 4) + i % 4) as u8;
        i += 1;
    }
    t
};
const LOW_IDX: [[u8; 16]; 2] = {
    let mut t = [[0u8; 16]; 2];
    let mut i = 0;
    while i < 32 {
        t[i / 16][i % 16] = (5 * (i / 4) + 4) as u8;
        i += 1;
    }
    t
};
/// Per 16-bit lane, the right shift (as NEON's left shift by a negative count) that brings sample i % 4's two low bits down.
const LOW_SHIFT: [i16; 8] = [0, -2, -4, -6, 0, -2, -4, -6];

/// One block's 32 samples, in four lanes of eight: the same table lookups and shifts the NEON path makes, lane by lane, for the host's tests (and any build that is not aarch64).
#[cfg(any(test, not(target_arch = "aarch64")))]
fn block_model(b: &[u8]) -> [[u16; 8]; 4] {
    let mut out = [[0u16; 8]; 4];
    for q in 0..4 {
        for l in 0..8 {
            let i = q * 8 + l;
            let hi = b[HIGH_IDX[i / 16][i % 16] as usize] as u16;
            let lo = b[LOW_IDX[i / 16][i % 16] as usize] as u16;
            out[q][l] = hi << 2 | (lo >> -LOW_SHIFT[l]) & 3;
        }
    }
    out
}

/// One RAW10 line's first `blocks` blocks added into a row of 16-bit sums (`acc`, `blocks * 32` samples): 64 frames of 1023 fit 16 bits (65472), so a u16 row takes up to 2^6 frames before it is folded into a wider total (`fold`). `line` holds `blocks * 40 + SLACK` bytes. One path, no branch per sample.
pub fn add10_u16(line: &[u8], acc: &mut [u16], blocks: usize) {
    assert!(line.len() >= blocks * BLOCK_BYTES + SLACK && acc.len() >= blocks * BLOCK_SAMPLES);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        use core::arch::aarch64::*;
        let hi0 = vld1q_u8(HIGH_IDX[0].as_ptr());
        let hi1 = vld1q_u8(HIGH_IDX[1].as_ptr());
        let lo0 = vld1q_u8(LOW_IDX[0].as_ptr());
        let lo1 = vld1q_u8(LOW_IDX[1].as_ptr());
        let sh = vld1q_s16(LOW_SHIFT.as_ptr());
        let three = vdupq_n_u16(3);
        for k in 0..blocks {
            // SAFETY: block k's 48 bytes lie inside `line` (40 k + 48 <= blocks 40 + SLACK) and its 32 sums inside `acc`, by the assert above.
            let t = vld1q_u8_x3(line.as_ptr().add(k * BLOCK_BYTES));
            let (h0, h1) = (vqtbl3q_u8(t, hi0), vqtbl3q_u8(t, hi1));
            let (l0, l1) = (vqtbl3q_u8(t, lo0), vqtbl3q_u8(t, lo1));
            let s = |h: uint8x8_t, l: uint8x8_t| vorrq_u16(vshll_n_u8::<2>(h), vandq_u16(vshlq_u16(vmovl_u8(l), sh), three));
            let a = acc.as_mut_ptr().add(k * BLOCK_SAMPLES);
            vst1q_u16(a, vaddq_u16(vld1q_u16(a), s(vget_low_u8(h0), vget_low_u8(l0))));
            vst1q_u16(a.add(8), vaddq_u16(vld1q_u16(a.add(8)), s(vget_high_u8(h0), vget_high_u8(l0))));
            vst1q_u16(a.add(16), vaddq_u16(vld1q_u16(a.add(16)), s(vget_low_u8(h1), vget_low_u8(l1))));
            vst1q_u16(a.add(24), vaddq_u16(vld1q_u16(a.add(24)), s(vget_high_u8(h1), vget_high_u8(l1))));
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    for k in 0..blocks {
        let q = block_model(&line[k * BLOCK_BYTES..k * BLOCK_BYTES + 48]);
        for (j, lane) in q.iter().enumerate() {
            for (l, &v) in lane.iter().enumerate() {
                let a = &mut acc[k * BLOCK_SAMPLES + j * 8 + l];
                *a = a.wrapping_add(v);
            }
        }
    }
}

/// `add10_u16` into 32-bit sums: no fold, twice the bytes the add reads and writes.
pub fn add10_u32(line: &[u8], acc: &mut [u32], blocks: usize) {
    assert!(line.len() >= blocks * BLOCK_BYTES + SLACK && acc.len() >= blocks * BLOCK_SAMPLES);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        use core::arch::aarch64::*;
        let hi0 = vld1q_u8(HIGH_IDX[0].as_ptr());
        let hi1 = vld1q_u8(HIGH_IDX[1].as_ptr());
        let lo0 = vld1q_u8(LOW_IDX[0].as_ptr());
        let lo1 = vld1q_u8(LOW_IDX[1].as_ptr());
        let sh = vld1q_s16(LOW_SHIFT.as_ptr());
        let three = vdupq_n_u16(3);
        for k in 0..blocks {
            // SAFETY: as `add10_u16`'s.
            let t = vld1q_u8_x3(line.as_ptr().add(k * BLOCK_BYTES));
            let (h0, h1) = (vqtbl3q_u8(t, hi0), vqtbl3q_u8(t, hi1));
            let (l0, l1) = (vqtbl3q_u8(t, lo0), vqtbl3q_u8(t, lo1));
            let s = |h: uint8x8_t, l: uint8x8_t| vorrq_u16(vshll_n_u8::<2>(h), vandq_u16(vshlq_u16(vmovl_u8(l), sh), three));
            let a = acc.as_mut_ptr().add(k * BLOCK_SAMPLES);
            let add = |a: *mut u32, s: uint16x8_t| {
                vst1q_u32(a, vaddw_u16(vld1q_u32(a), vget_low_u16(s)));
                vst1q_u32(a.add(4), vaddw_high_u16(vld1q_u32(a.add(4)), s));
            };
            add(a, s(vget_low_u8(h0), vget_low_u8(l0)));
            add(a.add(8), s(vget_high_u8(h0), vget_high_u8(l0)));
            add(a.add(16), s(vget_low_u8(h1), vget_low_u8(l1)));
            add(a.add(24), s(vget_high_u8(h1), vget_high_u8(l1)));
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    for k in 0..blocks {
        let q = block_model(&line[k * BLOCK_BYTES..k * BLOCK_BYTES + 48]);
        for (j, lane) in q.iter().enumerate() {
            for (l, &v) in lane.iter().enumerate() {
                let a = &mut acc[k * BLOCK_SAMPLES + j * 8 + l];
                *a = a.wrapping_add(v as u32);
            }
        }
    }
}

/// One RAW10 line's blocks unpacked (no add): the slit's ring takes its samples this way.
pub fn unpack10(line: &[u8], out: &mut [u16], blocks: usize) {
    out[..blocks * BLOCK_SAMPLES].fill(0);
    add10_u16(line, out, blocks);
}

/// A row of 16-bit sums folded into the 32-bit total and cleared: every 2^6 frames of `add10_u16`.
pub fn fold(acc: &mut [u16], total: &mut [u32]) {
    for (t, a) in total.iter_mut().zip(acc.iter_mut()) {
        *t += *a as u32;
        *a = 0;
    }
}

/// One RAW12 line (two samples in three bytes: the two high bytes, then the low nibbles, the first sample's in bits 3 to 0) added into 32-bit sums, `w` samples (even). For the RAW12 attempt only: plain loops the compiler vectorises as it can.
pub fn add12_u32(line: &[u8], acc: &mut [u32], w: usize) {
    for (p, b) in acc[..w].chunks_exact_mut(2).zip(line.chunks_exact(3)) {
        p[0] += (b[0] as u32) << 4 | (b[2] & 0xF) as u32;
        p[1] += (b[1] as u32) << 4 | (b[2] >> 4) as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The block path (its lookups and shifts, as the NEON path makes them) against the plain unpack, over every low-bits pattern and a sweep of high bytes; and the u16 and u32 adds agree.
    #[test]
    fn blocks_unpack_as_the_plain_path() {
        let w = 4080;
        let n = blocks(w);
        let stride = 0x1400;
        let mut line = vec![0u8; stride + SLACK];
        for (i, b) in line.iter_mut().enumerate() {
            *b = (i * 131 + (i >> 3) * 7) as u8;
        }
        let mut plain = vec![0u16; n * BLOCK_SAMPLES];
        unpack(&line[..n * BLOCK_BYTES], &mut plain);
        let mut a16 = vec![0u16; n * BLOCK_SAMPLES];
        let mut a32 = vec![0u32; n * BLOCK_SAMPLES];
        for _ in 0..3 {
            add10_u16(&line, &mut a16, n);
            add10_u32(&line, &mut a32, n);
        }
        for i in 0..w {
            assert_eq!(a16[i], 3 * plain[i], "sample {i}");
            assert_eq!(a32[i], 3 * plain[i] as u32);
        }
        let mut total = vec![0u32; n * BLOCK_SAMPLES];
        fold(&mut a16, &mut total);
        assert_eq!(&total[..w], &a32[..w]);
        assert!(a16.iter().all(|&a| a == 0));
        assert_eq!(n, 128);
        assert_eq!(blocks(4000) * BLOCK_BYTES + SLACK, 5008);
        assert!(blocks(4000) * BLOCK_BYTES + SLACK <= 0x13C0 + 64);
        // 64 frames of the brightest sample fit 16 bits.
        assert!(64 * 1023 <= u16::MAX as u32);
    }

    #[test]
    fn raw12_adds() {
        let mut acc = [0u32; 2];
        add12_u32(&[0xAB, 0xCD, 0x21], &mut acc, 2);
        assert_eq!(acc, [0xAB1, 0xCD2]);
    }

    #[test]
    fn a_quad_unpacks() {
        // Pixels 0x3FF, 0x000, 0x155, 0x2AA: MSBs FF 00 55 AA, LSBs 11 00 01 10 = 0b10_01_00_11.
        let mut px = [0u16; 4];
        unpack(&[0xFF, 0x00, 0x55, 0xAA, 0b10_01_00_11], &mut px);
        assert_eq!(px, [0x3FF, 0x000, 0x155, 0x2AA]);
    }

    /// The recorded frame from the Pro (target/cam/img.bin, local), binned 2 x 2 here and as std/camreplay's build bins it: the same samples.
    #[test]
    fn the_local_frame_bins_as_camreplay() {
        let Ok(raw) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/img.bin") else { return };
        let (w, h, stride, bin) = (4080usize, 3072usize, 0x1400usize, 2usize);
        let px = |x: usize, y: usize| -> u32 {
            let g = y * stride + (x / 4) * 5;
            (raw[g + (x & 3)] as u32) << 2 | (raw[g + 4] as u32 >> (2 * (x & 3))) & 3
        };
        // Bin 2 as std/camreplay's build bins it (its formula, written out); 4 and 8 as the same mean over their blocks.
        for bin in [bin, 4, 8] {
            let (bw, bh) = (w / bin, h / bin);
            let mut pix = vec![0u16; 2 * bin * w];
            let mut row = vec![0u8; bw * 2];
            let mut acc = vec![0u16; w];
            let mut sum = 0u64;
            for pair in 0..bh / 2 {
                for r in 0..2 * bin {
                    let y = pair * 2 * bin + r;
                    unpack(&raw[y * stride..y * stride + stride], &mut pix[r * w..(r + 1) * w]);
                }
                for phase in 0..2 {
                    bin_row(&pix, w, bin, phase, &mut acc, &mut row);
                    let oy = pair * 2 + phase;
                    for ox in 0..bw {
                        let (x0, y0) = (2 * bin * (ox / 2) + (ox & 1), 2 * bin * (oy / 2) + (oy & 1));
                        let want = if bin == 2 {
                            (px(x0, y0) + px(x0 + 2, y0) + px(x0, y0 + 2) + px(x0 + 2, y0 + 2)) / 4
                        } else {
                            (0..bin).flat_map(|j| (0..bin).map(move |i| (i, j))).map(|(i, j)| px(x0 + 2 * i, y0 + 2 * j)).sum::<u32>() / (bin * bin) as u32
                        };
                        let got = u16::from_le_bytes([row[ox * 2], row[ox * 2 + 1]]) as u32;
                        assert_eq!(got, want, "bin {bin} sample ({ox}, {oy})");
                        sum += got as u64;
                    }
                }
            }
            println!("bin {bin}: {bw} x {bh}, mean sample {}", sum / (bw * bh) as u64);
        }
    }
}
