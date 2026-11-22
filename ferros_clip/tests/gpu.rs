//! The band coder's GPU kernel (src/gpu_kernel.rs), run on valhall's emulator over a real Pro frame: every block's facts equal ferros_rawcodec `split_codes`'s, every block's body is its codes end to end, and the CPU's packing of them (`pack_band_bodies`) is `encode_band`'s bytes.

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
    check_at(w, 5120, bands, what)
}

/// As `check`, rows `stride` bytes apart (a real frame's bytes read at that stride: the same bytes go to the GPU and to the CPU).
fn check_at(w: usize, stride: usize, bands: usize, what: &str) {
    let _one = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
    let h = bands * rc::BAND_ROWS;
    let raw = frame(h, stride);
    let k = gpu::kernel(w, stride);
    // The words the device runs are build.rs's, compiled on the host: the same recording, so the same words.
    if let Some(p) = gpu::prebuilt(w, stride) {
        assert!(p.words == &k.words[..] && p.consts == &k.consts[..] && p.regs64 == k.regs64, "{what}: the prebuilt kernel differs from this recording");
    }
    eprintln!("{what}: the kernel is A#{} words, A#{} registers a thread (peak A#{} live), by kind {:?}", k.words.len(), if k.regs64 { 64 } else { 32 }, k.peak, k.kinds);
    // The GPU's memory: the frame with its slack, the bodies, the facts.
    let base = 0x8000_0000u64;
    let frame_at = 4096u64;
    let codes_at = frame_at + (h * stride) as u64 + 4096;
    let info_at = codes_at + ((h << gpu::BLOCKS_LOG2 << gpu::BODY_LOG2) * 4) as u64;
    let size = info_at + (h << gpu::BLOCKS_LOG2) as u64 * 4;
    let mut mem = Memory { base, bytes: vec![0u8; size as usize] };
    mem.bytes[frame_at as usize..frame_at as usize + raw.len()].copy_from_slice(&raw);
    let mut uniforms = vec![base + codes_at, base + frame_at, base + info_at];
    uniforms.extend_from_slice(&k.consts);
    for id in 0..(h << gpu::BLOCKS_LOG2) as u32 {
        run(&k.words, &uniforms, [id, 0], &mut mem);
    }
    let word = |at: u64, i: usize| u32::from_le_bytes(mem.bytes[at as usize + 4 * i..][..4].try_into().unwrap());
    let mut work = Box::new(rc::Work::new());
    let nb = rc::row_blocks(w);
    for b in 0..bands {
        let src = &raw[rc::band_src(b, h, w, stride)];
        let rows = rc::band_rows(h, b);
        let (mut info, mut codes) = (vec![0u32; rows * nb], vec![0u16; rows * w]);
        rc::split_codes(src, stride, w, rows, rc::Cfa::Bayer, &mut work, &mut info, &mut codes);
        let r0 = b * rc::BAND_ROWS;
        let gi: Vec<u32> = (0..rows << gpu::BLOCKS_LOG2).map(|i| word(info_at, (r0 << gpu::BLOCKS_LOG2) + i)).collect();
        let gb: Vec<u32> = (0..rows << gpu::BLOCKS_LOG2 << gpu::BODY_LOG2).map(|i| word(codes_at, (r0 << gpu::BLOCKS_LOG2 << gpu::BODY_LOG2) + i)).collect();
        for y in 0..rows {
            for j in 0..nb {
                let at = (y << gpu::BLOCKS_LOG2) + j;
                let v = gi[at];
                assert_eq!(v & 31, info[y * nb + j], "{what}: band {b} row {y} block {j}: facts");
                // The body: the CPU's codes end to end, most significant bit first.
                let x0 = j * rc::BLOCK;
                let mut bits: Vec<bool> = Vec::new();
                for &cd in &codes[y * w + x0..y * w + (x0 + rc::BLOCK).min(w)] {
                    let l = (cd >> rc::SPLIT_LEN_SHIFT) as u32;
                    bits.extend((0..l).rev().map(|s| ((cd & 0x3FF) as u32) >> s & 1 != 0));
                }
                assert_eq!(v >> rc::SPLIT_BITS_SHIFT, bits.len() as u32, "{what}: band {b} row {y} block {j}: body length");
                if (0..bits.len()).any(|q| (gb[(at << gpu::BODY_LOG2) + gpu::BODY_AT + q / 32] >> (31 - q % 32) & 1 != 0) != bits[q]) {
                    let want: Vec<String> = bits.chunks(32).map(|c| format!("{:08X}", c.iter().enumerate().fold(0u32, |a, (i, &b)| a | (b as u32) << (31 - i)))).collect();
                    let got: Vec<String> = (0..bits.len().div_ceil(32)).map(|q| format!("{:08X}", gb[(at << gpu::BODY_LOG2) + gpu::BODY_AT + q])).collect();
                    let lens: Vec<u16> = codes[y * w + x0..y * w + (x0 + rc::BLOCK).min(w)].iter().map(|c| c >> rc::SPLIT_LEN_SHIFT).collect();
                    eprintln!("want {want:?}\ngot  {got:?}\nlens {lens:?}");
                }
                for (q, &bit) in bits.iter().enumerate() {
                    assert_eq!(gb[(at << gpu::BODY_LOG2) + gpu::BODY_AT + q / 32] >> (31 - q % 32) & 1 != 0, bit, "{what}: band {b} row {y} block {j}: body bit {q}");
                }
                if bits.len() % 32 != 0 {
                    assert_eq!(gb[(at << gpu::BODY_LOG2) + gpu::BODY_AT + bits.len() / 32] & (u32::MAX >> (bits.len() % 32)), 0, "{what}: band {b} row {y} block {j}: the last word's spare bits");
                }
            }
        }
        // The CPU's half over the GPU's output: encode_band's bytes.
        let mut want = vec![0u8; rc::max_band_len(w, rows)];
        let (la, lb) = rc::encode_band(src, stride, w, rows, rc::Cfa::Bayer, &mut work, &mut want);
        let mut got = vec![0u8; rc::max_band_len(w, rows)];
        let (ga, gl) = rc::pack_band_bodies(&gi, 1 << gpu::BLOCKS_LOG2, &gb[gpu::BODY_AT..], gpu::BODY_LOG2, w, rows, rc::Cfa::Bayer, &mut got);
        assert_eq!((ga, gl), (la, lb), "{what}: band {b}: lengths");
        assert!(got[..ga + gl] == want[..la + lb], "{what}: band {b}: bytes");
    }
}

#[test]
fn the_gpu_kernel_is_the_band_coder_on_a_pro_frame() {
    // Two bands of the binned main's 4080 (127 whole blocks and one of 16): a band's first rows, its left and right edges, the band after.
    check(4080, 2, "4080 wide");
}

#[test]
fn rows_of_whole_blocks() {
    check(4096, 1, "4096 wide");
}

/// The other three Pro cameras' binned readouts as their jobs write them (ferros_cam cams), each kernel the one build.rs ships for its geometry: the ultrawide's 4000 (125 whole blocks), the front's 3840 (120), the tele's 4032 (126).
#[test]
fn the_other_cameras_geometries() {
    for (w, stride, what) in [(4000, 0x13C0, "the ultrawide"), (3840, 0x12C0, "the front"), (4032, 0x13C0, "the tele")] {
        assert!(gpu::prebuilt(w, stride).is_some(), "{what}: no kernel built");
        check_at(w, stride, 2, what);
    }
}
