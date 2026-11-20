//! The band coder's first half as a GPU kernel (src/gpu.rs), run on valhall's emulator over a real Pro frame: every block's facts and every residual's code equal ferros_rawcodec `split_codes`, and the CPU's packing of them is `encode_band`'s bytes.

use ferros_clip::gpu;
use ferros_rawcodec as rc;
use valhall::sim::{run, Memory};

/// The first rows of a real frame (target/cam, as ferros_rawcodec's real_frames test reads), or a synthetic one when it is missing.
fn frame(h: usize, stride: usize) -> Vec<u8> {
    if let Ok(raw) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/shot1.raw10") {
        return raw[..h * stride].to_vec();
    }
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    (0..h * stride)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s as u8
        })
        .collect()
}

/// valhall's SSA recorder is one global (begin to compile), so the tests record one at a time.
static RECORDING: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn check(w: usize, bands: usize, what: &str) {
    let _one = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
    let (stride, h) = (5120usize, bands * rc::BAND_ROWS);
    let raw = frame(h, stride);
    let k = gpu::kernel(w, stride);
    eprintln!("{what}: the kernel is A#{} words, A#{} registers a thread", k.words.len(), if k.regs64 { 64 } else { 32 });
    // The GPU's memory: the frame with its slack, the codes, the facts.
    let base = 0x8000_0000u64;
    let frame_at = 4096u64;
    let codes_at = frame_at + (h * stride) as u64 + 4096;
    let info_at = codes_at + (h * gpu::CODES_ROW * 2) as u64;
    let size = info_at + (h << gpu::BLOCKS_LOG2) as u64 * 4;
    let mut mem = Memory { base, bytes: vec![0u8; size as usize] };
    mem.bytes[frame_at as usize..frame_at as usize + raw.len()].copy_from_slice(&raw);
    let mut uniforms = vec![base + codes_at, base + frame_at, base + info_at];
    uniforms.extend_from_slice(&k.consts);
    for id in 0..(h << gpu::BLOCKS_LOG2) as u32 {
        run(&k.words, &uniforms, [id, 0], &mut mem);
    }
    let mut work = Box::new(rc::Work::new());
    let nb = rc::row_blocks(w);
    for b in 0..bands {
        let src = &raw[rc::band_src(b, h, w, stride)];
        let rows = rc::band_rows(h, b);
        let (mut info, mut codes) = (vec![0u32; rows * nb], vec![0u16; rows * w]);
        rc::split_codes(src, stride, w, rows, rc::Cfa::Bayer, &mut work, &mut info, &mut codes);
        let r0 = b * rc::BAND_ROWS;
        let gi: Vec<u32> = (0..rows << gpu::BLOCKS_LOG2).map(|i| u32::from_le_bytes(mem.bytes[(info_at as usize) + 4 * ((r0 << gpu::BLOCKS_LOG2) + i)..][..4].try_into().unwrap())).collect();
        let gc: Vec<u16> = (0..rows * gpu::CODES_ROW).map(|i| u16::from_le_bytes(mem.bytes[(codes_at as usize) + 2 * (r0 * gpu::CODES_ROW + i)..][..2].try_into().unwrap())).collect();
        for y in 0..rows {
            for j in 0..nb {
                assert_eq!(gi[(y << gpu::BLOCKS_LOG2) + j], info[y * nb + j], "{what}: band {b} row {y} block {j}: facts");
            }
            for x in 0..w {
                assert_eq!(gc[y * gpu::CODES_ROW + x], codes[y * w + x], "{what}: band {b} row {y} sample {x}: code");
            }
        }
        // The CPU's half over the GPU's output: encode_band's bytes.
        let mut want = vec![0u8; rc::max_band_len(w, rows)];
        let (la, lb) = rc::encode_band(src, stride, w, rows, rc::Cfa::Bayer, &mut work, &mut want);
        let mut got = vec![0u8; rc::max_band_len(w, rows)];
        let (ga, gb) = rc::pack_band_strided(&gi, 1 << gpu::BLOCKS_LOG2, &gc, gpu::CODES_ROW, w, rows, rc::Cfa::Bayer, &mut work, &mut got);
        assert_eq!((ga, gb), (la, lb), "{what}: band {b}: lengths");
        assert!(got[..ga + gb] == want[..la + lb], "{what}: band {b}: bytes");
    }
}

#[test]
fn the_gpu_first_half_is_split_codes_on_a_pro_frame() {
    // Two bands of the binned main's 4080 (127 whole blocks and one of 16): a band's first rows, its left and right edges, the band after.
    check(4080, 2, "4080 wide");
}

#[test]
fn rows_of_whole_blocks() {
    check(4096, 1, "4096 wide");
}
