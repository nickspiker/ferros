// clip-check: a RAW clip pulled from a device (`ferros-bridge store extent <key> <bytes> <out>`) read on the host by vsf's clip reader.
// Every frame is decoded; a frame named in the hash list (std/rawrec's console: `rawrec: hash A#<frame> <BLAKE3 hex>`, the frame's packed rows as the camera wrote them) is packed back to RAW10 and its hash compared.
// Usage: clip-check <clip> [console log] [out: frame k's samples as RAW10 rows, for a look]
// Numbers print as A# (decimal) and G# (hex).

use std::time::Instant;

use vsf::visual::clip::Reader;

fn packed_hash(px: &[u16], w: usize, h: usize) -> [u8; 32] {
    let mut x = blake3::Hasher::new();
    let mut row = vec![0u8; w * 5 / 4];
    for y in 0..h {
        for g in 0..w / 4 {
            let v = &px[y * w + g * 4..y * w + g * 4 + 4];
            for i in 0..4 {
                row[g * 5 + i] = (v[i] >> 2) as u8;
            }
            row[g * 5 + 4] = (v[0] & 3) as u8 | ((v[1] & 3) << 2) as u8 | ((v[2] & 3) << 4) as u8 | ((v[3] & 3) << 6) as u8;
        }
        x.update(&row);
    }
    *x.finalize().as_bytes()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: clip-check <clip> [console log]");
        std::process::exit(1);
    }
    let bytes = std::fs::read(&args[1]).expect("the clip");
    let hashes: Vec<(usize, String)> = match args.get(2) {
        Some(p) => std::fs::read_to_string(p)
            .expect("the console log")
            .lines()
            .filter_map(|l| {
                let rest = l.split("rawrec: hash A#").nth(1)?;
                let mut it = rest.split_whitespace();
                Some((it.next()?.parse().ok()?, it.next()?.to_string()))
            })
            .collect(),
        None => Vec::new(),
    };
    let t = Instant::now();
    let r = Reader::open(bytes.as_slice()).expect("the clip opens");
    let sp = r.spec().clone();
    println!("clip-check: A#{} bytes, A#{} frames ({}), A#{} x A#{} {:?}, bands of A#{} rows, A#{}/A#{} a second, period A#{} ns, opened in A#{} ms", bytes.len(), r.frames().len(), if r.recovered() { "recovered by its chain" } else { "by its index" }, sp.width, sp.height, sp.mosaic, sp.band_rows, sp.rate_num, sp.rate_den, sp.period_ns, t.elapsed().as_millis());
    let (w, h) = (sp.width, sp.height);
    let raw = (w * h * 5 / 4) as u64;
    let (mut ok, mut bad, mut checked) = (0, 0, 0);
    let mut prev_at: Option<i64> = None;
    let (mut gap_most, mut holes) = (0i64, 0u64);
    let t = Instant::now();
    for k in 0..r.frames().len() {
        let e = &r.frames()[k];
        if k > 0 && e.meta.index != r.frames()[k - 1].meta.index + 1 {
            holes += e.meta.index - r.frames()[k - 1].meta.index - 1;
        }
        if let Some(p) = prev_at {
            gap_most = gap_most.max(e.meta.at - p);
        }
        prev_at = Some(e.meta.at);
        let px = match r.decode(k) {
            Ok(px) => px,
            Err(err) => {
                println!("clip-check: frame A#{k} did not decode: {err}");
                bad += 1;
                continue;
            }
        };
        if let Some((_, want)) = hashes.iter().find(|(f, _)| *f == k) {
            checked += 1;
            let got: String = packed_hash(&px, w, h).iter().map(|b| format!("{b:02X}")).collect();
            if got == *want {
                ok += 1;
            } else {
                bad += 1;
                println!("clip-check: frame A#{k} DIFFERS from the frame the device hashed");
            }
        }
        if args.get(3).is_some() && k == 0 {
            let mut out = Vec::with_capacity((raw) as usize);
            for y in 0..h {
                for g in 0..w / 4 {
                    let v = &px[y * w + g * 4..y * w + g * 4 + 4];
                    for i in 0..4 {
                        out.push((v[i] >> 2) as u8);
                    }
                    out.push((v[0] & 3) as u8 | ((v[1] & 3) << 2) as u8 | ((v[2] & 3) << 4) as u8 | ((v[3] & 3) << 6) as u8);
                }
            }
            std::fs::write(&args[3], &out).expect("the frame's rows");
        }
    }
    let osc = vsf::types::eagle_time::OSCILLATIONS_PER_SECOND as f64;
    let span = match (r.frames().first(), r.frames().last()) {
        (Some(a), Some(b)) => (b.meta.at - a.meta.at) as f64 / osc,
        _ => 0.,
    };
    println!("clip-check: decoded A#{} frames in A#{} ms; A#{checked} hashed frames checked, A#{ok} bit exact, A#{bad} not; A#{holes} declared holes; the frames span A#{:.3} s, the longest gap A#{:.2} ms; ratio A#{:.2} over the clip", r.frames().len(), t.elapsed().as_millis(), span, gap_most as f64 / osc * 1e3, raw as f64 * r.frames().len() as f64 / bytes.len() as f64);
    if bad != 0 {
        std::process::exit(2);
    }
}
