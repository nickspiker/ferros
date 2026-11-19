// clip-check: a RAW clip pulled from a device (`ferros-bridge store extent <key> <bytes> <out>`) read on the host by vsf's clip reader.
// Every frame is decoded (on every host core) and its samples packed back to RAW10 band by band and hashed (ferros_clip `samples_source_hash`), then compared with the source hash the recorder's workers took of the frame as the camera wrote it (std/rawrec's `rawrec.hashes`, a VSF document: `index`, `hashes` [n, 32]).
// Usage: clip-check <clip> [hashes.vsf]
// Numbers print as A# (decimal) and G# (hex).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use vsf::types::VsfType;
use vsf::visual::clip::Reader;

fn hashes(path: &str) -> Vec<(u64, [u8; 32])> {
    let bytes = std::fs::read(path).expect("the hashes");
    // A console log instead (Lumis's `RAW video: frame <k> source <hex>` lines, k the clip's frame).
    if !bytes.starts_with("RÅ<".as_bytes()) {
        let hex = |s: &str| -> Option<[u8; 32]> { (s.len() == 64).then(|| core::array::from_fn(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap_or(0))) };
        return String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|l| {
                let mut w = l.split("RAW video: frame ").nth(1)?.split_whitespace();
                let k = w.next()?.parse().ok()?;
                (w.next()? == "source").then_some(())?;
                Some((k, hex(w.next()?)?))
            })
            .collect();
    }
    let doc = vsf::container::Document::open(bytes.as_slice()).expect("the hashes document");
    let s = doc.section(doc.entry("rawrec_hashes").expect("section rawrec_hashes")).expect("the section");
    let first = |n: &str| s.get_field(n).and_then(|f| f.values.first()).cloned();
    let (Some(VsfType::t_u6(ix)), Some(VsfType::t_u3(hs))) = (first("index"), first("hashes")) else { panic!("index and hashes") };
    ix.data.iter().zip(hs.data.chunks(32)).map(|(&i, h)| (i, h.try_into().unwrap())).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: clip-check <clip> [hashes.vsf]");
        std::process::exit(1);
    }
    let file = std::fs::File::open(&args[1]).expect("the clip");
    let size = file.metadata().unwrap().len();
    let want = args.get(2).map(|p| hashes(p)).unwrap_or_default();
    let t = Instant::now();
    let r = Reader::open(file).expect("the clip opens");
    let sp = r.spec().clone();
    let n = r.frames().len();
    println!("clip-check: A#{size} bytes, A#{n} frames ({}), A#{} x A#{} {:?}, bands of A#{} rows, A#{}/A#{} a second, period A#{} ns, opened in A#{} ms", if r.recovered() { "recovered by its chain" } else { "by its index" }, sp.width, sp.height, sp.mosaic, sp.band_rows, sp.rate_num, sp.rate_den, sp.period_ns, t.elapsed().as_millis());
    let (w, h) = (sp.width, sp.height);
    // The frames' timing and holes, from the index.
    let osc = vsf::types::eagle_time::OSCILLATIONS_PER_SECOND as f64;
    let (mut holes, mut gap_most, mut gap_least) = (0u64, 0i64, i64::MAX);
    for k in 1..n {
        let (a, b) = (&r.frames()[k - 1], &r.frames()[k]);
        holes += b.meta.index - a.meta.index - 1;
        gap_most = gap_most.max(b.meta.at - a.meta.at);
        gap_least = gap_least.min(b.meta.at - a.meta.at);
    }
    let span = if n > 1 { (r.frames()[n - 1].meta.at - r.frames()[0].meta.at) as f64 / osc } else { 0. };
    println!("clip-check: the frames span A#{span:.3} s; A#{holes} declared holes; between frames A#{:.3} to A#{:.3} ms", gap_least as f64 / osc * 1e3, gap_most as f64 / osc * 1e3);
    let t = Instant::now();
    let next = AtomicUsize::new(0);
    let (ok, bad, unchecked) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    let threads = std::thread::available_parallelism().map(|p| p.get()).unwrap_or(4);
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                if k >= n {
                    break;
                }
                match r.decode(k) {
                    Ok(px) => match want.iter().find(|(i, _)| *i == k as u64) {
                        Some((_, hh)) if ferros_clip::samples_source_hash(&px, w, h) == *hh => {
                            ok.fetch_add(1, Ordering::Relaxed);
                        }
                        Some(_) => {
                            bad.fetch_add(1, Ordering::Relaxed);
                            println!("clip-check: frame A#{k} DIFFERS from the frame the camera wrote");
                        }
                        None => {
                            unchecked.fetch_add(1, Ordering::Relaxed);
                        }
                    },
                    Err(e) => {
                        bad.fetch_add(1, Ordering::Relaxed);
                        println!("clip-check: frame A#{k} did not decode: {e}");
                    }
                }
            });
        }
    });
    let raw = (w * h * 5 / 4) as f64 * n as f64;
    let (ok, bad, unchecked) = (ok.into_inner(), bad.into_inner(), unchecked.into_inner());
    println!("clip-check: decoded A#{n} frames in A#{} ms on A#{threads} threads: A#{ok} bit exact against the camera's frames, A#{bad} not, A#{unchecked} with no hash; ratio A#{:.2} over the clip", t.elapsed().as_millis(), raw / size as f64);
    if bad != 0 {
        std::process::exit(2);
    }
}
