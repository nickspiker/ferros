// Host tests: bit exact round trips on synthetic and real frames, and the bitstream's identity with the prototype (tools/raw-codec, Params::DEFAULT): for every band, prototype bytes == [la as u32 LE] ++ this crate's bytes.
// Numbers print as A# (decimal).

use ferros_rawcodec::*;
use std::time::Instant;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u16 {
        (self.next() % n) as u16
    }
}

/// Pack samples to RAW10 rows on a byte stride; the stride's spare bytes hold G#A5 so a read past a row's samples shows.
fn pack(px: &[u16], w: usize, h: usize, stride: usize) -> Vec<u8> {
    let mut raw = vec![0xA5u8; (h - 1) * stride + row_bytes(w)];
    for y in 0..h {
        for (g, s) in raw[y * stride..y * stride + row_bytes(w)].chunks_exact_mut(5).zip(px[y * w..(y + 1) * w].chunks_exact(4)) {
            g[4] = 0;
            for i in 0..4 {
                g[i] = (s[i] >> 2) as u8;
                g[4] |= ((s[i] & 3) as u8) << (2 * i);
            }
        }
    }
    raw
}

fn unpack(raw: &[u8], w: usize, h: usize, stride: usize) -> Vec<u16> {
    let mut px = vec![0u16; w * h];
    for y in 0..h {
        for (g, d) in raw[y * stride..y * stride + row_bytes(w)].chunks_exact(5).zip(px[y * w..(y + 1) * w].chunks_exact_mut(4)) {
            for i in 0..4 {
                d[i] = ((g[i] as u16) << 2) | ((g[4] as u16 >> (2 * i)) & 3);
            }
        }
    }
    px
}

const KINDS: usize = 11;

/// Synthetic frames: noise, flat, saturated, black, ramps, the worst escapes, k jumps, spikes, per-colour levels.
fn synth(kind: usize, w: usize, h: usize, seed: u64) -> Vec<u16> {
    let mut r = Rng(seed | 1);
    let mut px = vec![0u16; w * h];
    for y in 0..h {
        for x in 0..w {
            let v = match kind {
                0 => r.below(1 << 10),
                1 => 1 << 6,
                2 => 1023,
                3 => 0,
                4 => ((x + 3 * y) & 1023) as u16,
                5 => (x * 1023 / (w - 1)) as u16,
                6 => 200 + r.below(64) + r.below(64) + r.below(64) + r.below(64) - 128,
                // Neighbours at opposite ends: residuals near 512, every code an escape.
                7 => if (x ^ y) & 2 == 0 { 0 } else { 1023 },
                // Blocks of flat then noise: k steps by more than 1 and escapes in low-k blocks.
                8 => if (x >> 5) & 1 == 0 { 64 + r.below(4) } else { r.below(1 << 10) },
                9 => if r.below(16) == 0 { 1023 } else { 64 },
                _ => [64u16, 300, 700, 1000][(x & 1) | ((y & 1) << 1)] + r.below(16),
            };
            px[y * w + x] = v;
        }
    }
    px
}

struct Coders {
    work: Box<Work>,
    pwork: Box<raw_codec::Work>,
    out: Vec<u8>,
    pout: Vec<u8>,
    dst: Vec<u16>,
}

impl Coders {
    fn new() -> Coders {
        Coders { work: Box::new(Work::new()), pwork: Box::new(raw_codec::Work::new()), out: Vec::new(), pout: Vec::new(), dst: Vec::new() }
    }
    /// One band: encode, the prototype identity, decode, compare; returns la + lb.
    fn band(&mut self, src: &[u8], stride: usize, w: usize, rows: usize, cfa: Cfa, want: &[u16], what: &str) -> usize {
        self.out.resize(max_band_len(w, rows), 0);
        let (la, lb) = encode_band(src, stride, w, rows, cfa, &mut self.work, &mut self.out);
        let pcfa = if cfa == Cfa::Quad { raw_codec::Cfa::Quad } else { raw_codec::Cfa::Bayer };
        self.pout.resize(raw_codec::max_encoded_len(w, rows), 0);
        let pn = raw_codec::encode_band(src, stride, w, rows, pcfa, &raw_codec::Params::DEFAULT, None, &mut self.pwork, &mut self.pout);
        assert_eq!(pn, 4 + la + lb, "{what}: prototype length");
        assert_eq!(&self.pout[..4], &(la as u32).to_le_bytes(), "{what}: prototype's first-stream length");
        assert!(self.pout[4..pn] == self.out[..la + lb], "{what}: bytes differ from the prototype");
        self.dst.clear();
        self.dst.resize(w * rows, 0);
        let used = decode_band(&self.out[..la], &self.out[la..la + lb], w, rows, cfa, &mut self.work, &mut self.dst, w);
        assert_eq!(used, (la, lb), "{what}: decoder used other lengths");
        assert!(self.dst == want, "{what}: round trip differs");
        if w <= BLOCK {
            assert_eq!(lb, 0, "{what}: B should be empty");
        }
        la + lb
    }
}

fn one(c: &mut Coders, kind: usize, w: usize, rows: usize, cfa: Cfa, seed: u64) {
    let px = synth(kind, w, rows, seed);
    // A stride past the row with spare bytes, as the camera's 256-byte stride has.
    let stride = row_bytes(w) + 3;
    let raw = pack(&px, w, rows, stride);
    c.band(&raw, stride, w, rows, cfa, &px, &format!("kind {kind} w {w} rows {rows} {cfa:?}"));
}

#[test]
fn small_widths_every_band_height() {
    let mut c = Coders::new();
    let widths = [8, 12, 16, 20, 28, 32, 36, 60, 64, 68, 100, 124, 252];
    for cfa in [Cfa::Bayer, Cfa::Quad] {
        for &w in &widths {
            for rows in 1..=BAND_ROWS {
                for kind in 0..KINDS {
                    one(&mut c, kind, w, rows, cfa, (w * 131 + rows * 7 + kind) as u64);
                }
            }
        }
    }
}

#[test]
fn wide_rows() {
    let mut c = Coders::new();
    for cfa in [Cfa::Bayer, Cfa::Quad] {
        for &w in &[1000, 4000, 4080, 4624, 8160, MAX_W] {
            for &rows in &[1, 2, 3, 4, 5, 17, BAND_ROWS] {
                for kind in 0..KINDS {
                    one(&mut c, kind, w, rows, cfa, (w + rows + kind) as u64);
                }
            }
        }
    }
}

/// A frame cut by the frame helpers, the last band short.
#[test]
fn frame_by_bands() {
    let mut c = Coders::new();
    for cfa in [Cfa::Bayer, Cfa::Quad] {
        let (w, h, stride) = (4080, 200, 5120);
        assert_eq!(bands(h), 4);
        assert_eq!(band_rows(h, 3), 8);
        assert_eq!(band_rows(h, 2), BAND_ROWS);
        for kind in [0, 6, 8, 10] {
            let px = synth(kind, w, h, 77);
            let raw = pack(&px, w, h, stride);
            for k in 0..bands(h) {
                let y0 = band_first_row(k);
                let rows = band_rows(h, k);
                let r = band_src(k, h, w, stride);
                assert_eq!(r.end, (y0 + rows - 1) * stride + row_bytes(w));
                c.band(&raw[r], stride, w, rows, cfa, &px[y0 * w..(y0 + rows) * w], &format!("frame kind {kind} band {k} {cfa:?}"));
            }
        }
    }
}

/// The worst case (every code an escape) codes inside max_band_len.
#[test]
fn worst_case_fits() {
    let mut c = Coders::new();
    for &w in &[8, 36, 4080] {
        let rows = BAND_ROWS;
        let px = synth(7, w, rows, 1);
        let raw = pack(&px, w, rows, row_bytes(w));
        let n = c.band(&raw, row_bytes(w), w, rows, Cfa::Bayer, &px, "worst");
        assert!(n <= max_band_len(w, rows));
    }
}

/// The real frames in target/agent-rawvideo/frames.txt: identity, round trip, ratio and host encode time (one core).
#[test]
#[ignore]
fn real_frames() {
    let list = "/mnt/Harbor/Code/ferros/target/agent-rawvideo/frames.txt";
    let text = std::fs::read_to_string(list).expect("frames list");
    let mut c = Coders::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.is_empty() || t[0].starts_with('#') {
            continue;
        }
        let (w, h, stride, period): (usize, usize, usize, usize) = (t[2].parse().unwrap(), t[3].parse().unwrap(), t[4].parse().unwrap(), t[5].parse().unwrap());
        let Ok(raw) = std::fs::read(t[1]) else {
            println!("{}: skipped, {} missing", t[0], t[1]);
            continue;
        };
        assert!(raw.len() >= (h - 1) * stride + row_bytes(w), "{}: short file", t[0]);
        let cfa = if period == 4 { Cfa::Quad } else { Cfa::Bayer };
        let px = unpack(&raw, w, h, stride);
        // Identity and round trip, band by band.
        let mut coded = 0;
        for k in 0..bands(h) {
            let y0 = band_first_row(k);
            let rows = band_rows(h, k);
            coded += c.band(&raw[band_src(k, h, w, stride)], stride, w, rows, cfa, &px[y0 * w..(y0 + rows) * w], &format!("{} band {k}", t[0]));
        }
        // Encode time alone, the best of four runs.
        let mut out = vec![0u8; max_band_len(w, BAND_ROWS)];
        let mut best = f64::MAX;
        for _ in 0..4 {
            let t0 = Instant::now();
            let mut n = 0;
            for k in 0..bands(h) {
                let (la, lb) = encode_band(&raw[band_src(k, h, w, stride)], stride, w, band_rows(h, k), cfa, &mut c.work, &mut out);
                n += la + lb;
            }
            best = best.min(t0.elapsed().as_secs_f64());
            assert_eq!(n, coded);
        }
        let packed = w * h * 10 / 8;
        println!("{}: A#{} x A#{}, coded A#{} bytes, ratio A#{:.3}, A#{:.3} bits a pixel, encode A#{:.1} ms (one core), identity and round trip exact", t[0], w, h, coded, packed as f64 / coded as f64, coded as f64 * 8. / (w * h) as f64, best * 1e3);
    }
}
