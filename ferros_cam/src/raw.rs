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

/// One binned output row, `out.len() / 2` samples, from the 2 x `bin` unpacked lines of its row pair (`pix`, `w` samples a line, line r at `r * w`): output sample ox is the mean of the `bin` x `bin` same-colour samples of its block, so the mosaic keeps its phase (`phase` 0 the pair's even output row, 1 its odd). `bin` a power of two.
pub fn bin_row(pix: &[u16], w: usize, bin: usize, phase: usize, out: &mut [u8]) {
    let shift = 2 * bin.trailing_zeros();
    for (ox, o) in out.chunks_exact_mut(2).enumerate() {
        let x0 = 2 * bin * (ox / 2) + (ox & 1);
        let mut s = 0u32;
        for j in 0..bin {
            let line = &pix[(phase + 2 * j) * w..];
            for i in 0..bin {
                s += line[x0 + 2 * i] as u32;
            }
        }
        o.copy_from_slice(&((s >> shift) as u16).to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (bw, bh) = (w / bin, h / bin);
        let mut pix = vec![0u16; 2 * bin * w];
        let mut row = vec![0u8; bw * 2];
        let mut sum = 0u64;
        for pair in 0..bh / 2 {
            for r in 0..2 * bin {
                let y = pair * 2 * bin + r;
                unpack(&raw[y * stride..y * stride + stride], &mut pix[r * w..(r + 1) * w]);
            }
            for phase in 0..2 {
                bin_row(&pix, w, bin, phase, &mut row);
                let oy = pair * 2 + phase;
                for ox in 0..bw {
                    let (x0, y0) = (4 * (ox / 2) + (ox & 1), 4 * (oy / 2) + (oy & 1));
                    let want = (px(x0, y0) + px(x0 + 2, y0) + px(x0, y0 + 2) + px(x0 + 2, y0 + 2)) / 4;
                    let got = u16::from_le_bytes([row[ox * 2], row[ox * 2 + 1]]) as u32;
                    assert_eq!(got, want, "sample ({ox}, {oy})");
                    sum += got as u64;
                }
            }
        }
        println!("bin 2: {bw} x {bh}, mean sample {}", sum / (bw * bh) as u64);
    }
}
