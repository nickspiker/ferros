//! rawrec: the RAW video recorder's test harness (docs/RAW-VIDEO.md "The recorder").
//! Spawned with the camera and a vault scope, in that order: `ferros-bridge spawn rawrec.bin <hex> --name rawrec --use camera --store test`; with `--use gpu --dma layers` after them a Bayer clip is coded on the GPU (ferros_clip `gpu`, +20 bit 3 turns it off).
//!
//! The input (hex bytes):
//! - +0 the job: 0 record, 1 the clip's record (info), 2 delete it, 6 the per-core profile, 7 the GPU bench, 8 the camera alone (the power baseline), 5 read it back as sealed (a recovered one too) by its index, every 2^(+1)-th frame decoded against its hash, 3 the seal bench (XChaCha20-Poly1305 on one and four cores of each cluster: what sealing a clip would cost), 4 the NEON check (+1 frames a gain, 0: 4): this build's band coder against the scalar, byte for byte, on the binned main at gains 1x to 64x, each band decoded against the frame, both timed on four A715s; build with `--features neon` for it to mean anything.
//! - record (+17 the writer's cores: 0 the A510s, 1 the A715s, 2 the X3): +1 frames a second (1 to 60, 0: 30); +2 frames to record, u16 LE (0: by seconds); +4 seconds, u16 LE (0: 64 frames); +6 A715 workers (0 to 4, G#FF none; 0: 4); +7 A510 workers (0 to 4); +8 the shutter in microseconds, u32 LE (0: the driver's); +12 the gain as 2^(n-1) times unity (0: the driver's); +13 every 2^n-th frame hashed as captured (0: 3, every 8th); +14 coded frames waiting for the flash (0: 4); +15 the hottest zone that ends the run, C (0: 85); +16 1: decode the clip on the device after, every hashed frame against its hash (0: yes, 2: no); +18 1: the main loop on the A510s; +19 the clip's name, rawrec<n>.clip (0: rawrec.clip).
//!
//! Every second it says the frames, the dropped ones, the bytes written and their rate, the longest frame's coding and the hottest zone; at the end the clip's length and seal, its first block in the clips area (for a pull), and each hashed frame's index and BLAKE3 (of its packed rows as the camera wrote them, each row's 5/4 bytes a sample, the stride's tail left out), which a host decode of the pulled clip repacks and compares.

use std::time::Instant;

use ferros_clip::ferros::{Extent, ExtentSink, ExtentSource};
use ferros_clip::{cfa_of_byte, Camera, FrameIn, Mode, Pacing, Recorder, Taken};

const SYS_CALL: u64 = 1;
const SYS_DMA: u64 = 7;
const SYS_INPUT: u64 = 8;
const SYS_FACTS: u64 = 12;
const ERR: u64 = u64::MAX;
/// The camera endpoint's slot (`--use camera` first).
const S_CAMERA: u64 = 1;
/// The clip's name in the scope.
const NAME: &str = "rawrec.clip";

fn arc_base() -> u64 {
    let b: u64;
    unsafe { core::arch::asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
    b
}

fn off<T: ?Sized>(p: *const T) -> u64 {
    (p as *const u8 as u64).wrapping_sub(arc_base())
}

fn cap(slot: u64) -> u64 {
    0x204 + 32 * slot
}

fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let r: u64;
    unsafe { core::arch::asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
    r
}

fn call(slot: u64, msg: &[u8], reply: &mut [u8]) -> u64 {
    svc(SYS_CALL, cap(slot), off(msg.as_ptr()), msg.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64)
}

fn input() -> Vec<u8> {
    let mut all = Vec::new();
    let mut b = vec![0u8; 1 << 16];
    loop {
        let n = svc(SYS_INPUT, all.len() as u64, off(b.as_mut_ptr()), b.len() as u64, 0, 0) as usize;
        all.extend_from_slice(&b[..n]);
        if n < b.len() {
            return all;
        }
    }
}

/// The hottest thermal zone, C (fact 10).
fn hottest() -> u64 {
    let mut b = [0xFFu8; 11 * 8];
    svc(SYS_FACTS, off(b.as_mut_ptr()), b.len() as u64, 0, 0, 0);
    u64::from_le_bytes(b[80..88].try_into().unwrap())
}

fn set(what: u8, v: u64) -> u8 {
    let mut m = [3u8, what, 0, 0, 0, 0, 0, 0, 0, 0];
    m[2..10].copy_from_slice(&v.to_le_bytes());
    let mut a = [0xEEu8; 1];
    if call(S_CAMERA, &m, &mut a) == ERR { 0xEE } else { a[0] }
}

/// A value into the vault scope (`SYS_STORE` set, 256 KiB a piece): the hashes for the host.
fn store_put(slot: u64, name: &str, value: &[u8]) -> bool {
    const SET: u64 = 1;
    const AT: u64 = 2;
    const MORE: u64 = 4;
    let piece = 1usize << 18;
    let mut at = 0;
    while at < value.len() {
        let n = (value.len() - at).min(piece);
        let more = if at + n < value.len() { MORE } else { 0 };
        if svc(13, cap(slot), SET | AT | more | (n as u64) << 8 | (at as u64) << 32, off(name.as_ptr()), name.len() as u64, off(value[at..].as_ptr())) != 0 {
            return false;
        }
        at += n;
    }
    true
}

/// The frames' source hashes as one VSF document (section `rawrec_hashes`: `index` the clip's frame numbers, `hashes` [n, 32]).
fn hashes_doc(hashes: &[(u64, [u8; 32])]) -> Vec<u8> {
    use vsf::types::{Tensor, VsfType};
    let n = hashes.len();
    vsf::vsf_builder::VsfBuilder::new()
        .add_section("rawrec_hashes", vec![("index".to_string(), VsfType::t_u6(Tensor::new(vec![n], hashes.iter().map(|h| h.0).collect()))), ("hashes".to_string(), VsfType::t_u3(Tensor::new(vec![n, 32], hashes.iter().flat_map(|h| h.1).collect())))])
        .build()
        .unwrap_or_default()
}

fn hex(h: &[u8; 32]) -> String {
    h.iter().map(|b| format!("{b:02X}")).collect()
}

/// Job 3: what sealing a clip would cost (docs/RAW-VIDEO.md, the next step): XChaCha20-Poly1305 over 2^6 MiB in place, a MiB a seal, on one core and on four of each cluster, at the clocks the recording holds (the A715s at 1572 MHz while a clip is open: hold them with `diag dvfs` around this job to match).
fn seal_bench() {
    use chacha20poly1305::aead::{AeadInPlace, KeyInit};
    let key = chacha20poly1305::Key::from_slice(&[7u8; 32]).to_owned();
    for (what, mask, threads) in [("one A715", 0x00F0u64, 1usize), ("four A715s", 0x00F0, 4), ("one A510", 0x000F, 1), ("four A510s", 0x000F, 4)] {
        let t = Instant::now();
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| {
                    ferros_clip::ferros::pin(mask);
                    let c = chacha20poly1305::XChaCha20Poly1305::new(&key);
                    let mut buf = vec![0x5Au8; 1 << 20];
                    for k in 0..(1u64 << 6) {
                        let mut nonce = [0u8; 24];
                        nonce[..8].copy_from_slice(&k.to_le_bytes());
                        let _ = c.encrypt_in_place_detached(chacha20poly1305::XNonce::from_slice(&nonce), b"", &mut buf);
                    }
                });
            }
        });
        let us = t.elapsed().as_micros() as u64;
        println!("rawrec: seal bench, {what}: A#{} MiB in A#{} ms, A#{} MB/s", threads << 6, us / 1000, ((threads as u64) << 26) / us.max(1));
    }
}

/// One frame's bands coded by `encode_band` (NEON in a `--features neon` build) and by `encode_band_scalar`, compared byte for byte, and both timed on four A715 threads: (bands that differ, scalar us, this build's us, coded bytes).
fn neon_frame(rows: &[u8], w: usize, h: usize, stride: usize) -> (usize, u64, u64, usize) {
    use ferros_rawcodec as rc;
    let nb = rc::bands(h);
    let mut work = Box::new(rc::Work::new());
    let mut a = vec![0u8; rc::max_band_len(w, rc::BAND_ROWS)];
    let mut b = vec![0u8; rc::max_band_len(w, rc::BAND_ROWS)];
    let (mut differ, mut coded) = (0, 0);
    for k in 0..nb {
        let n = rc::band_rows(h, k);
        let src = &rows[rc::band_src(k, h, w, stride)];
        let (la, lb) = rc::encode_band(src, stride, w, n, rc::Cfa::Bayer, &mut work, &mut a);
        let (sa, sb) = rc::encode_band_scalar(src, stride, w, n, rc::Cfa::Bayer, &mut work, &mut b);
        coded += la + lb;
        if (la, lb) != (sa, sb) || a[..la + lb] != b[..sa + sb] {
            differ += 1;
            if differ == 1 {
                let at = a[..la + lb].iter().zip(b[..sa + sb].iter()).position(|(x, y)| x != y).unwrap_or(la.min(sa));
                println!("rawrec: neon check: band A#{k} differs: lengths A#{la}+A#{lb} against the scalar's A#{sa}+A#{sb}, first at byte A#{at}");
            }
        }
    }
    let time = |neon: bool| -> u64 {
        let t = Instant::now();
        std::thread::scope(|sc| {
            for q in 0..4usize {
                sc.spawn(move || {
                    ferros_clip::ferros::pin(0x00F0);
                    let mut work = Box::new(rc::Work::new());
                    let mut o = vec![0u8; rc::max_band_len(w, rc::BAND_ROWS)];
                    for k in (q..nb).step_by(4) {
                        let n = rc::band_rows(h, k);
                        let src = &rows[rc::band_src(k, h, w, stride)];
                        if neon {
                            rc::encode_band(src, stride, w, n, rc::Cfa::Bayer, &mut work, &mut o);
                        } else {
                            rc::encode_band_scalar(src, stride, w, n, rc::Cfa::Bayer, &mut work, &mut o);
                        }
                    }
                });
            }
        });
        t.elapsed().as_micros() as u64
    };
    let (ts, tn) = (time(false), time(true));
    (differ, ts, tn, coded)
}

/// Job 6: where a frame's coding goes, per core kind (the 60/s work, 2026-11-19): one frame from the view, its 48 bands on one core of each kind in turn, the band coder, the coded bands' BLAKE3 (`band_digest`), the source rows' BLAKE3 (`band_source_digest`) and the copy of the coded bytes timed apart; then the coder alone on four A715s, four A510s and all eight. Run under the recording's clocks: a one-block extent is held open meanwhile (the A715s at 1572 MHz, the A510s at their top).
fn profile(slot: u64) {
    use ferros_rawcodec as rc;
    let mut ext = Extent::new(slot, "rawrec.prof");
    let held = ext.open(1).is_ok();
    let mut r = [0u8; 64];
    let n = call(S_CAMERA, &Pacing::of(30).open_msg(0, 16_000_000), &mut r);
    if n == ERR || r[0] != 0 || n < 48 {
        println!("rawrec: profile: OPEN refused");
        return;
    }
    let mut tok = [0u8; 32];
    tok.copy_from_slice(&r[1..33]);
    let base = svc(SYS_DMA, off(tok.as_ptr()), 0, 0, 0, 0);
    let (w, h) = (u16::from_le_bytes([r[36], r[37]]) as usize, u16::from_le_bytes([r[38], r[39]]) as usize);
    let stride = u32::from_le_bytes(r[40..44].try_into().unwrap()) as usize;
    let mut hd = [0u8; 64];
    let mut mapped = 0xFFu8;
    for _ in 0..8 {
        if call(S_CAMERA, &[5, mapped], &mut hd) == ERR {
            println!("rawrec: profile: MAP failed");
            let _ = call(S_CAMERA, &[4], &mut r);
            return;
        }
        mapped = hd[40];
    }
    // SAFETY: the view's buffer this MAP names, held until the next MAP.
    let view = unsafe { core::slice::from_raw_parts((base + u64::from_le_bytes(hd[48..56].try_into().unwrap())) as *const u8, h * stride) };
    // The same frame copied into the process's own memory: the copy's rate is the view's read rate, and every step below runs on both.
    let mut copy = vec![0u8; view.len()];
    let t = Instant::now();
    copy.copy_from_slice(view);
    let cu = t.elapsed().as_micros() as u64;
    let t = Instant::now();
    let mut again = vec![0u8; view.len()];
    again.copy_from_slice(&copy);
    let cc = t.elapsed().as_micros() as u64;
    println!("rawrec: profile: the frame (A#{} bytes) copied out of the view in A#{cu} us (A#{} MB/s); from memory to memory A#{cc} us (A#{} MB/s)", view.len(), view.len() as u64 / cu.max(1), view.len() as u64 / cc.max(1));
    for (from, rows) in [("the view", view), ("a copy", &copy[..])] {
    let nb = rc::bands(h);
    println!("rawrec: profile: from {from}: NEON A#{}, recording clocks A#{held}, A#{nb} bands of A#{w} x A#{} rows", rc::NEON as u8, rc::BAND_ROWS);
    for (what, mask) in [("an A715", 0x0010u64), ("an A510", 0x0001), ("the X3", 0x0100)] {
        let (te, tb, ts, tc) = std::thread::scope(|sc| {
            sc.spawn(|| {
                ferros_clip::ferros::pin(mask);
                let mut work = Box::new(rc::Work::new());
                let mut o = vec![0u8; rc::max_band_len(w, rc::BAND_ROWS)];
                let mut keep = vec![0u8; o.len()];
                let (mut te, mut tb, mut ts, mut tc) = (0u64, 0u64, 0u64, 0u64);
                for pass in 0..2 {
                    for k in 0..nb {
                        let n = rc::band_rows(h, k);
                        let src = &rows[rc::band_src(k, h, w, stride)];
                        let t0 = Instant::now();
                        let (la, lb) = rc::encode_band(src, stride, w, n, rc::Cfa::Bayer, &mut work, &mut o);
                        let t1 = Instant::now();
                        let _ = vsf::visual::clip::band_digest(&o[..la], &o[la..la + lb]);
                        let t2 = Instant::now();
                        let _ = ferros_clip::band_source_digest(src, stride, w, n);
                        let t3 = Instant::now();
                        keep[..la + lb].copy_from_slice(&o[..la + lb]);
                        let t4 = Instant::now();
                        // The second pass counts: the first warms the caches and the code.
                        if pass == 1 {
                            te += (t1 - t0).as_micros() as u64;
                            tb += (t2 - t1).as_micros() as u64;
                            ts += (t3 - t2).as_micros() as u64;
                            tc += (t4 - t3).as_micros() as u64;
                        }
                    }
                }
                (te, tb, ts, tc)
            })
            .join()
            .unwrap()
        });
        println!("rawrec: profile: on {what}, a frame: the coder A#{} us, the coded bands' BLAKE3 A#{} us, the source's BLAKE3 A#{} us, the copy A#{} us", te, tb, ts, tc);
    }
    for (what, masks) in [("four A715s", vec![0x00F0u64; 4]), ("four A715s, each its own", vec![0x0010u64, 0x0020, 0x0040, 0x0080]), ("two A715s, each its own", vec![0x0010u64, 0x0020]), ("four A510s", vec![0x000F; 4]), ("four A715s and four A510s", [vec![0x00F0u64; 4], vec![0x000F; 4]].concat())] {
        let next = std::sync::atomic::AtomicUsize::new(0);
        let t = Instant::now();
        let per: Vec<(u64, usize)> = std::thread::scope(|sc| {
            let hs: Vec<_> = masks
                .iter()
                .map(|&m| {
                    let next = &next;
                    sc.spawn(move || {
                        ferros_clip::ferros::pin(m);
                        let mut work = Box::new(rc::Work::new());
                        let mut o = vec![0u8; rc::max_band_len(w, rc::BAND_ROWS)];
                        let (t0, mut done) = (Instant::now(), 0);
                        loop {
                            let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if k >= nb {
                                break;
                            }
                            let src = &rows[rc::band_src(k, h, w, stride)];
                            rc::encode_band(src, stride, w, rc::band_rows(h, k), rc::Cfa::Bayer, &mut work, &mut o);
                            done += 1;
                        }
                        (t0.elapsed().as_micros() as u64, done)
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        println!("rawrec: profile: the coder on {what}, bands claimed as each frees: A#{} us a frame; bands each A#{:?}", t.elapsed().as_micros(), per.iter().map(|p| p.1).collect::<Vec<_>>());
    }
    }
    let _ = call(S_CAMERA, &[6, mapped], &mut r);
    let _ = call(S_CAMERA, &[4], &mut r);
    if held {
        ext.delete();
    }
}

/// Job 7: the band coder's first half on the GPU (ferros_clip gpu, the 60/s work): spawned with `--use camera --store test --use gpu --dma layers` (+1 the GPU service's grant, 0: 3; +2 the GPU block's, 0: 4; +3 runs, 0: 8).
/// Each run: a frame of the view copied into the block, the kernel run over it, then every band packed (`gpu::pack_band`) by coders claiming bands (+4: 0 four A715s and four A510s, one to a core; 1 four A715s, one to a core; 2 four A715s on the cluster's mask); the first run's bands compared with `encode_band`'s byte for byte.
fn gpu_bench(slot: u64, inp: &[u8]) {
    use ferros_rawcodec as rc;
    let arg = |k: usize| inp.get(k).copied().unwrap_or(0) as u64;
    let (gs, bs) = (if arg(1) == 0 { 3 } else { arg(1) }, if arg(2) == 0 { 4 } else { arg(2) });
    let runs = if arg(3) == 0 { 8 } else { arg(3) as usize };
    let mut ext = Extent::new(slot, "rawrec.prof");
    let held = ext.open(1).is_ok();
    let mut r = [0u8; 64];
    let n = call(S_CAMERA, &Pacing::of(30).open_msg(0, 16_000_000), &mut r);
    if n == ERR || r[0] != 0 || n < 48 {
        println!("rawrec: gpu: OPEN refused");
        return;
    }
    let mut tok = [0u8; 32];
    tok.copy_from_slice(&r[1..33]);
    let base = svc(SYS_DMA, off(tok.as_ptr()), 0, 0, 0, 0);
    let (w, h) = (u16::from_le_bytes([r[36], r[37]]) as usize, u16::from_le_bytes([r[38], r[39]]) as usize);
    let stride = u32::from_le_bytes(r[40..44].try_into().unwrap()) as usize;
    let mut hd = [0u8; 64];
    let mut mapped = 0xFFu8;
    let close = |mapped: u8, r: &mut [u8; 64]| {
        let _ = call(S_CAMERA, &[6, mapped], r);
        let _ = call(S_CAMERA, &[4], r);
    };
    for _ in 0..8 {
        if call(S_CAMERA, &[5, mapped], &mut hd) == ERR {
            println!("rawrec: gpu: MAP failed");
            close(mapped, &mut r);
            return;
        }
        mapped = hd[40];
    }
    // SAFETY: the view's buffer the MAP header names, held until the next MAP.
    let view = |hd: &[u8; 64]| unsafe { core::slice::from_raw_parts((base + u64::from_le_bytes(hd[48..56].try_into().unwrap())) as *const u8, h * stride) };
    let t = Instant::now();
    let mut dev = match ferros_clip::gpu::Device::open(gs, bs, w, h, stride) {
        Ok(d) => d,
        Err(e) => {
            println!("rawrec: gpu: {e}");
            close(mapped, &mut r);
            return;
        }
    };
    println!("rawrec: gpu: the block grown and the kernel written in A#{} us; A#{w} x A#{h}, A#{stride} bytes a line, recording clocks A#{held}", t.elapsed().as_micros());
    let nb = rc::bands(h);
    let masks: Vec<u64> = match arg(4) { 1 => vec![0x0010, 0x0020, 0x0040, 0x0080], 2 => vec![0x00F0; 4], _ => vec![0x0010, 0x0020, 0x0040, 0x0080, 0x0001, 0x0002, 0x0004, 0x0008] };
    for run in 0..runs {
        let t0 = Instant::now();
        // The copy and the run together (`run` is both); the service's own count is the GPU's part.
        let gpu_us = dev.run(0, view(&hd));
        let t_copy = t0.elapsed().as_micros() as u64;
        let gpu_us = match gpu_us {
            Ok(us) => us as u64,
            Err(e) => {
                println!("rawrec: gpu: run A#{run}: {e}");
                break;
            }
        };
        let t1 = Instant::now();
        let next = std::sync::atomic::AtomicUsize::new(0);
        let (info, bodies) = (dev.info(0), dev.bodies(0));
        let outs: Vec<Vec<(usize, usize, usize, Vec<u8>)>> = std::thread::scope(|sc| {
            let hs: Vec<_> = masks
                .iter()
                .map(|&m| {
                    let next = &next;
                    sc.spawn(move || {
                        ferros_clip::ferros::pin(m);
                        let mut mine = Vec::new();
                        let mut o = vec![0u8; rc::max_band_len(w, rc::BAND_ROWS)];
                        loop {
                            let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if k >= nb {
                                break;
                            }
                            let (la, lb) = ferros_clip::gpu::pack_band(info, bodies, k, w, h, &mut o);
                            mine.push((k, la, lb, if run == 0 { o[..la + lb].to_vec() } else { Vec::new() }));
                        }
                        mine
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let pack_us = t1.elapsed().as_micros() as u64;
        let total = t0.elapsed().as_micros() as u64;
        let coded: usize = outs.iter().flatten().map(|b| b.1 + b.2).sum();
        let mut line = format!("rawrec: gpu: run A#{run}: copy and GPU A#{t_copy} us (the GPU A#{gpu_us} us), packing on cores G#{masks:X?} A#{pack_us} us, the frame A#{total} us, coded A#{coded} bytes");
        if run == 0 {
            // The check: every band against the CPU coder's, from the frame as copied into the block.
            let frame = dev.frame(0);
            let mut work = Box::new(rc::Work::new());
            let mut want = vec![0u8; rc::max_band_len(w, rc::BAND_ROWS)];
            let mut bad = 0;
            for b in outs.iter().flatten() {
                let rows = rc::band_rows(h, b.0);
                let (la, lb) = rc::encode_band(&frame[rc::band_src(b.0, h, w, stride)], stride, w, rows, rc::Cfa::Bayer, &mut work, &mut want);
                if (la, lb) != (b.1, b.2) || want[..la + lb] != b.3[..] {
                    if bad == 0 {
                        line += &format!("; band A#{} differs (A#{la}+A#{lb} against the GPU's A#{}+A#{})", b.0, b.1, b.2);
                    }
                    bad += 1;
                }
            }
            line += &format!("; A#{bad} of A#{nb} bands differ from encode_band");
            // Packing one band on one A715 from the block, then from a copy in the process's own memory: whether the block reads slower than ordinary memory.
            let (ci, cc) = (info.to_vec(), bodies.to_vec());
            let one = |info: &[u32], bodies: &[u32]| -> u64 {
                std::thread::scope(|sc| {
                    sc.spawn(|| {
                        ferros_clip::ferros::pin(0x0010);
                        let mut o = vec![0u8; rc::max_band_len(w, rc::BAND_ROWS)];
                        let mut us = 0;
                        for pass in 0..2 {
                            let t = Instant::now();
                            for k in [8usize, 9] {
                                ferros_clip::gpu::pack_band(info, bodies, k, w, h, &mut o);
                            }
                            if pass == 1 {
                                us = t.elapsed().as_micros() as u64;
                            }
                        }
                        us
                    })
                    .join()
                    .unwrap()
                })
            };
            line += &format!("; two bands packed on an A715 from the block A#{} us, from a copy A#{} us", one(info, bodies), one(&ci, &cc));
            if bad != 0 {
                // Which blocks: every block's body against the CPU's codes end to end (`split_codes`), then the same frame run again and checked again (whether the GPU's output changes from run to run).
                let check = |dev: &ferros_clip::gpu::Device| -> String {
                    let (info, bodies) = (dev.info(0), dev.bodies(0));
                    let frame = dev.frame(0);
                    let nbk = rc::row_blocks(w);
                    let mut work = Box::new(rc::Work::new());
                    let (mut ci, mut cc) = (vec![0u32; rc::BAND_ROWS * nbk], vec![0u16; rc::BAND_ROWS * w]);
                    let (mut nbad, mut first) = (0usize, String::new());
                    let mut words_bad = [0usize; 32];
                    for k in 0..nb {
                        let rows = rc::band_rows(h, k);
                        rc::split_codes(&frame[rc::band_src(k, h, w, stride)], stride, w, rows, rc::Cfa::Bayer, &mut work, &mut ci, &mut cc);
                        for y in 0..rows {
                            for j in 0..nbk {
                                let at = ((k * rc::BAND_ROWS + y) << ferros_clip::gpu::BLOCKS_LOG2) + j;
                                let mut want = [0u32; 32];
                                let mut pos = 0usize;
                                for &cd in &cc[y * w + j * rc::BLOCK..y * w + ((j + 1) * rc::BLOCK).min(w)] {
                                    let l = (cd >> rc::SPLIT_LEN_SHIFT) as usize;
                                    for s in (0..l).rev() {
                                        if ((cd & 0x3FF) as u32) >> s & 1 != 0 {
                                            want[pos >> 5] |= 1 << (31 - (pos & 31));
                                        }
                                        pos += 1;
                                    }
                                }
                                let got = &bodies[(at << ferros_clip::gpu::BODY_LOG2) + ferros_clip::gpu::BODY_AT..(at << ferros_clip::gpu::BODY_LOG2) + 32];
                                let used = pos.div_ceil(32);
                                let wrong = (0..used).find(|&q| got[q] != want[q]);
                                let fact_wrong = info[at] & 31 != ci[y * nbk + j] || (info[at] >> rc::SPLIT_BITS_SHIFT) as usize != pos;
                                if wrong.is_some() || fact_wrong {
                                    if let Some(q) = wrong {
                                        words_bad[q] += 1;
                                    }
                                    if nbad < 3 {
                                        let q = wrong.unwrap_or(0);
                                        first += &format!(" [band A#{k} row A#{y} block A#{j}: A#{pos} bits, facts G#{:X} against G#{:X}; word A#{q} G#{:08X} against G#{:08X}, its byte A#{} of its 64]", info[at], ci[y * nbk + j] | (pos as u32) << rc::SPLIT_BITS_SHIFT, got[q], want[q], ((at << ferros_clip::gpu::BODY_LOG2) * 4 + q * 4) & 63);
                                    }
                                    nbad += 1;
                                }
                            }
                        }
                    }
                    format!("A#{nbad} blocks wrong, the first wrong word's index in its body A#{:?};{first}", words_bad)
                };
                println!("rawrec: gpu: check: {}", check(&dev));
                let again = dev.go(0);
                println!("rawrec: gpu: the same frame again ({again:?}): {}", check(&dev));
            }
        }
        println!("{line}");
        // The next run on a newer frame.
        if call(S_CAMERA, &[5, mapped], &mut hd) == ERR {
            break;
        }
        mapped = hd[40];
    }
    close(mapped, &mut r);
    if held {
        ext.delete();
    }
}

/// Job 8: the camera alone, the baseline a recording's power is measured against: the binned main at +1 frames a second (0: 30) for +4 seconds (u16 LE, 0: 64), every frame taken (MAP) and given back, nothing coded or written; the hottest zone every 2^4 s.
fn camera_only(inp: &[u8]) {
    let arg = |k: usize| inp.get(k).copied().unwrap_or(0) as u64;
    let fps = if arg(1) == 0 { 30 } else { arg(1).min(60) } as u32;
    let secs = match arg(4) | arg(5) << 8 {
        0 => 64,
        s => s,
    };
    let mut r = [0u8; 64];
    let n = call(S_CAMERA, &Pacing::of(fps).open_msg(0, 0), &mut r);
    if n == ERR || r[0] != 0 || n < 48 {
        println!("rawrec: camera only: OPEN refused");
        return;
    }
    let mut hd = [0u8; 64];
    let mut held = 0xFFu8;
    let t0 = Instant::now();
    let (mut frames, mut next) = (0u64, 0u64);
    while t0.elapsed().as_secs() < secs {
        if call(S_CAMERA, &[5, held], &mut hd) == ERR {
            println!("rawrec: camera only: MAP failed");
            break;
        }
        held = hd[40];
        frames += 1;
        let el = t0.elapsed().as_secs();
        if el >= next {
            next = el + (1 << 4);
            println!("rawrec: camera only: A#{el} s: A#{frames} frames, hottest A#{} C", hottest());
        }
    }
    let _ = call(S_CAMERA, &[6, held], &mut r);
    let _ = call(S_CAMERA, &[4], &mut r);
    println!("rawrec: camera only: A#{frames} frames in A#{} ms at A#{fps} a second, hottest A#{} C", t0.elapsed().as_millis(), hottest());
}

/// Job 4: this build's band coder (NEON with `--features neon`) against the scalar one, byte for byte, on frames of the binned main at four gains (+1 the frames at each, 0: 4), then each band decoded and compared with the frame; the times on four A715s.
fn neon_check(inp: &[u8]) {
    let per = if inp.get(1).copied().unwrap_or(0) == 0 { 4 } else { inp[1] as usize };
    println!("rawrec: neon check: this build codes with NEON {}", ferros_rawcodec::NEON);
    let mut r = [0u8; 64];
    let n = call(S_CAMERA, &Pacing::of(30).open_msg(0, 16_000_000), &mut r);
    if n == ERR || r[0] != 0 || n < 48 {
        println!("rawrec: neon check: OPEN refused");
        return;
    }
    let mut tok = [0u8; 32];
    tok.copy_from_slice(&r[1..33]);
    let base = svc(SYS_DMA, off(tok.as_ptr()), 0, 0, 0, 0);
    let (w, h) = (u16::from_le_bytes([r[36], r[37]]) as usize, u16::from_le_bytes([r[38], r[39]]) as usize);
    let stride = u32::from_le_bytes(r[40..44].try_into().unwrap()) as usize;
    let mut hd = [0u8; 64];
    let mut held = 0xFFu8;
    let (mut frames, mut bad, mut ts, mut tn) = (0, 0, 0u64, 0u64);
    let mut work = Box::new(ferros_rawcodec::Work::new());
    let mut px = vec![0u16; w * ferros_rawcodec::BAND_ROWS];
    let mut o = vec![0u8; ferros_rawcodec::max_band_len(w, ferros_rawcodec::BAND_ROWS)];
    for g in 0..4u64 {
        set(1, 1024 << (2 * g));
        for _ in 0..per + 4 {
            if call(S_CAMERA, &[5, held], &mut hd) == ERR {
                println!("rawrec: neon check: MAP failed");
                let _ = call(S_CAMERA, &[4], &mut r);
                return;
            }
            held = hd[40];
        }
        for _ in 0..per {
            if call(S_CAMERA, &[5, held], &mut hd) == ERR {
                break;
            }
            held = hd[40];
            // SAFETY: the view's buffer this MAP names, held until the next MAP.
            let rows = unsafe { core::slice::from_raw_parts((base + u64::from_le_bytes(hd[48..56].try_into().unwrap())) as *const u8, h * stride) };
            let (d, s, t, coded) = neon_frame(rows, w, h, stride);
            // Every band of this build's code decoded and compared with the frame's samples.
            let mut lossy = 0;
            for k in 0..ferros_rawcodec::bands(h) {
                let n = ferros_rawcodec::band_rows(h, k);
                let src = &rows[ferros_rawcodec::band_src(k, h, w, stride)];
                let (la, lb) = ferros_rawcodec::encode_band(src, stride, w, n, ferros_rawcodec::Cfa::Bayer, &mut work, &mut o);
                let (oa, ob) = o.split_at(la);
                ferros_rawcodec::decode_band(oa, &ob[..lb], w, n, ferros_rawcodec::Cfa::Bayer, &mut work, &mut px, w);
                let same = (0..n).all(|y| (0..w / 4).all(|gq| (0..4).all(|i| px[y * w + gq * 4 + i] == ((src[y * stride + gq * 5 + i] as u16) << 2 | ((src[y * stride + gq * 5 + 4] >> (2 * i)) & 3) as u16))));
                lossy += !same as usize;
            }
            frames += 1;
            bad += d + lossy;
            ts += s;
            tn += t;
            println!("rawrec: neon check: gain A#{}x frame: A#{d} bands differ from the scalar, A#{lossy} do not decode to the frame; ratio A#{}.{:02}; scalar A#{} us, this build A#{} us on four A715s", 1u64 << (2 * g), (h * w * 5 / 4) / coded, (h * w * 5 / 4) * 100 / coded % 100, s, t);
        }
    }
    let _ = call(S_CAMERA, &[6, held], &mut r);
    let _ = call(S_CAMERA, &[4], &mut r);
    println!("rawrec: neon check: A#{frames} frames, A#{bad} bands wrong; scalar A#{} us a frame, this build A#{} us a frame on average (four A715s)", ts / frames.max(1) as u64, tn / frames.max(1) as u64);
}

fn main() {
    let inp = input();
    let arg = |k: usize| inp.get(k).copied().unwrap_or(0);
    let le16 = |k: usize| u16::from_le_bytes([arg(k), arg(k + 1)]) as u64;
    // +19: the clip's name, `rawrec<n>.clip` (0: rawrec.clip), so one clip can be recorded while another is left open (a recovery beside an OPEN).
    let name = if arg(19) == 0 { NAME.to_string() } else { format!("rawrec{}.clip", arg(19)) };
    let Some(slot) = ferros_clip::ferros::store_slot() else {
        println!("rawrec: no vault scope among the grants (spawn with --use camera --store test)");
        return;
    };
    let ext = Extent::new(slot, &name);
    println!("rawrec: the recording space: {:?}", ferros_clip::ferros::space(slot));
    match arg(0) {
        1 => {
            println!("rawrec: {name}: {:?}", ext.info());
            return;
        }
        5 => {
            // The clip as sealed (a recovered one too) read back by its index, every 2^(+1)-th frame (0: each) decoded against its own hash.
            let Some(info) = ext.info() else {
                println!("rawrec: {name}: no record");
                return;
            };
            let t = Instant::now();
            let r = match vsf::visual::clip::Reader::open(ExtentSource { extent: Extent::new(slot, &name), len: info.bytes }) {
                Ok(r) => r,
                Err(e) => {
                    println!("rawrec: {name}: the clip did not open: {e}");
                    return;
                }
            };
            let step = 1usize << arg(1).min(16);
            let n = r.frames().len();
            let bad = (0..n).step_by(step).filter(|&k| r.decode(k).is_err()).count();
            println!("rawrec: {name}: A#{} bytes sealed {}, A#{n} frames by its index (recovered {}), A#{} decoded against their hashes, A#{bad} failed, in A#{} ms", info.bytes, info.sealed, r.recovered(), n.div_ceil(step), t.elapsed().as_millis());
            return;
        }
        2 => {
            println!("rawrec: {name} deleted: {}", ext.delete());
            return;
        }
        3 => {
            seal_bench();
            return;
        }
        4 => {
            neon_check(&inp);
            return;
        }
        6 => {
            profile(slot);
            return;
        }
        7 => {
            gpu_bench(slot, &inp);
            return;
        }
        8 => {
            camera_only(&inp);
            return;
        }
        _ => {}
    }
    let fps = if arg(1) == 0 { 30 } else { arg(1).min(60) } as u32;
    let pacing = Pacing::of(fps);
    let (frames_want, secs) = (le16(2), le16(4));
    let frames_want = if frames_want == 0 && secs == 0 { 64 } else { frames_want };
    let a715 = match arg(6) {
        0 => 4,
        0xFF => 0,
        n => n.min(4),
    } as usize;
    let a510 = arg(7).min(4) as usize;
    let shutter_us = u32::from_le_bytes([arg(8), arg(9), arg(10), arg(11)]) as u64;
    let hash_log2 = if arg(13) == 0 { 3 } else { arg(13).min(16) } as u32;
    let slots = if arg(14) == 0 { 4 } else { arg(14) } as usize;
    let stop_c = if arg(15) == 0 { 85 } else { arg(15) } as u64;
    let verify = arg(16) != 2;
    // The recorder's own loop (MAP, the hand-over, the hashes) on the A715s beside its workers; the writer on the A510s.
    // +18: the recorder's own loop (MAP, the hand-over, its start's allocations) on the A715s (0) or the A510s (1), as Lumis's camera thread may run.
    ferros_clip::ferros::pin(if arg(18) == 1 { 0x000F } else { 0x00F0 });
    println!("rawrec: A#{fps} a second (fll A#{}, the sensor's frame A#{} ns, every A#{}-th kept: the clip's frame A#{} ns), A#{} frames or A#{secs} s, workers A#{a715} A715 + A#{a510} A510, shutter A#{shutter_us} us, gain byte A#{}, every A#{}-th frame hashed, A#{slots} slots, stop at A#{stop_c} C", pacing.fll, pacing.sensor_ns, pacing.keep, pacing.period_ns, frames_want, arg(12), 1u64 << hash_log2);

    // The camera: the binned main at this pacing, with its view.
    let mut r = [0u8; 64];
    let open = pacing.open_msg(0, shutter_us * 1000);
    let n = call(S_CAMERA, &open, &mut r);
    if n == ERR || r[0] != 0 || n < 48 {
        println!("rawrec: OPEN refused (G#{:X}, A#{} bytes)", if n == ERR { 0xEEE } else { r[0] as u64 }, if n == ERR { 0 } else { n });
        return;
    }
    let mut tok = [0u8; 32];
    tok.copy_from_slice(&r[1..33]);
    let base = svc(SYS_DMA, off(tok.as_ptr()), 0, 0, 0, 0);
    let size = svc(SYS_DMA, off(tok.as_ptr()), 1, 0, 0, 0);
    let (w, h) = (u16::from_le_bytes([r[36], r[37]]) as usize, u16::from_le_bytes([r[38], r[39]]) as usize);
    let stride = u32::from_le_bytes(r[40..44].try_into().unwrap()) as usize;
    println!("rawrec: the view at G#{base:X}, A#{} MiB, a ring of A#{}, A#{w} x A#{h}, A#{stride} bytes a line, write-back A#{}", if size == ERR { 0 } else { size >> 20 }, r[33], r[53]);
    if base == ERR {
        let _ = call(S_CAMERA, &[4], &mut r);
        return;
    }
    if arg(12) != 0 {
        println!("rawrec: SET gain A#{} times unity answered A#{}", 1u64 << (arg(12).min(8) - 1), set(1, 1024u64 << (arg(12).min(8) - 1)));
    }
    let mut hd = [0u8; 64];
    let mut held = 0xFFu8;
    let map = |held: u8, hd: &mut [u8; 64]| -> bool { call(S_CAMERA, &[5, held], hd) != ERR };
    if !map(held, &mut hd) {
        println!("rawrec: MAP: no first frame");
        let _ = call(S_CAMERA, &[4], &mut r);
        return;
    }
    held = hd[40];
    if shutter_us != 0 {
        println!("rawrec: SET shutter A#{shutter_us} us answered A#{}", set(0, shutter_us * 1000));
    }
    let w8 = |hd: &[u8; 64], k: usize| u64::from_le_bytes(hd[k..k + 8].try_into().unwrap());
    let cam = Camera {
        width: w,
        height: h,
        stride,
        cfa: cfa_of_byte(hd[9]),
        black: u16::from_le_bytes([hd[10], hd[11]]),
        white: u16::from_le_bytes([hd[12], hd[13]]),
        make: "Google".into(),
        model: "Pixel 8 Pro".into(),
        sensor_module: "main".into(),
        lens: String::new(),
        f_number: None,
        focal_mm: None,
        characterization: None,
        observer: String::new(),
        orientation: 1,
    };
    let raw_frame = (h * w * 5 / 4) as u64;
    let want = if frames_want != 0 { frames_want } else { secs * fps as u64 };
    // Room for every frame uncoded (the seal gives back what the clip did not use), or the longest free stretch when that is less (a minute at 60/s uncoded is 56 GB, the clips window 48 GiB; coded it is about a third).
    let blocks = ((want * raw_frame + (1 << 24)) >> 12).min(ferros_clip::ferros::space(slot).map_or(u64::MAX, |sp| sp.largest_free_bytes >> 12));
    let mut workers = vec![0x00F0u64; a715];
    workers.extend(std::iter::repeat_n(0x000Fu64, a510));
    // +20, the 60/s experiments: bit 0 the workers one to a core (ferros_clip `spread`), bit 1 no source digests, bit 2 a coder on the X3 too, bit 3 the GPU off.
    if arg(20) & 4 != 0 {
        workers.push(0x0100);
    }
    if arg(20) & 1 != 0 {
        workers = ferros_clip::spread(&workers);
    }
    println!("rawrec: the coders' cores G#{:?}, source digests A#{}", workers.iter().map(|m| format!("{m:X}")).collect::<Vec<_>>(), (arg(20) & 2 == 0) as u8);
    // The settings' frames let pass (a SET reaches the sensor a frame or two later, and an open's first frames come as the stream settles): the clip starts on a steady stream, as Lumis's does.
    for _ in 0..4 {
        if !map(held, &mut hd) {
            println!("rawrec: MAP: no frame while settling");
            let _ = call(S_CAMERA, &[4], &mut r);
            return;
        }
        held = hd[40];
    }
    // The copies kept whole for the check after the clip, their pages taken now and not in the camera loop (at 30 a second and under, or without source digests, when they are the only check: a copy is a few ms).
    let digests = arg(20) & 2 == 0;
    let keep_copies = fps <= 30 || !digests;
    let mut copy_bufs: Vec<Vec<u8>> = if keep_copies { (0..4).map(|_| vec![0u8; h * stride]).collect() } else { Vec::new() };
    let mode = Mode { pacing, title: format!("rawrec A#{fps} a second"), workers, writer: [0x000F, 0x00F0, 0x0100][arg(17).min(2) as usize], slots, blocks, at: ferros_clip::eagle_osc(w8(&hd, 32)), source_digests: arg(20) & 2 == 0 };
    // +20 bit 3: the GPU path off (the CPU coders alone, as before the 60/s work).
    ferros_clip::gpu::enable(arg(20) & 8 == 0);
    let mut rec = match Recorder::start(&cam, &mode, Box::new(ExtentSink(Extent::new(slot, &name)))) {
        Ok(r) => r,
        Err(e) => {
            println!("rawrec: the recorder did not start: {e}");
            let _ = call(S_CAMERA, &[4], &mut r);
            return;
        }
    };
    println!("rawrec: recording into {name}, A#{blocks} blocks asked; the mosaic G#{:02X}, black A#{}, white A#{}", hd[9], cam.black, cam.white);
    println!("rawrec: coded on {}", rec.gpu_note());
    // A fresh frame after the start: the open may have waited (a clip left open by a writer that died is closed first), and the frame held from before it would put the time in between down as dropped.
    if !map(held, &mut hd) {
        println!("rawrec: MAP: no frame after the start");
        let _ = call(S_CAMERA, &[4], &mut r);
        return;
    }
    held = hd[40];
    // Every frame's source hash as the recorder's workers took it (the clip's frame number, the hash), and four frames kept whole (one every 2^6) for the check on the device.
    let mut hashes: Vec<(u64, [u8; 32])> = Vec::with_capacity(want as usize);
    let mut copies: Vec<(u64, Vec<u8>)> = Vec::new();
    let t0 = Instant::now();
    let mut next_say = 1u64;
    let mut hot = hottest();
    let mut stopped = String::from("its frames were all taken");
    let (mut last_frames, mut last_written) = (0u64, 0u64);
    loop {
        let f = FrameIn {
            // SAFETY: the view holds the ring's buffers (`size` bytes from `base`), this one's h lines of `stride` bytes from its offset inside it; held (MAP) until the next MAP gives it back, after `frame` returns.
            rows: unsafe { core::slice::from_raw_parts((base + w8(&hd, 48)) as *const u8, h * stride) },
            count: u32::from_le_bytes(hd[0..4].try_into().unwrap()) as u64,
            eagle_ns: w8(&hd, 32),
            exposure_ns: w8(&hd, 16),
            gain: u32::from_le_bytes(hd[24..28].try_into().unwrap()),
            focus: u32::from_le_bytes(hd[28..32].try_into().unwrap()),
        };
        let before = rec.stats().frames;
        let taken = rec.frame(&f);
        if taken == Taken::Recorded {
            hashes.push((before, rec.source_hash()));
            if before & ((1 << 6) - 1) == 0 {
                if let Some(mut b) = copy_bufs.pop() {
                    b.copy_from_slice(f.rows);
                    copies.push((before, b));
                }
            }
        }
        if taken == Taken::Ended {
            stopped = String::from("the recorder ended (the extent full or a write failed)");
            break;
        }
        let st = rec.stats();
        if st.frames >= want {
            break;
        }
        let el = t0.elapsed().as_secs();
        if el >= next_say {
            next_say = el + 1;
            hot = hottest();
            let wr = st.written;
            println!("rawrec: A#{el} s: A#{} frames (+A#{}), A#{} dropped, A#{} paced, A#{} MB written (A#{} MB/s), ratio A#{}.{:02}, coding A#{} us a frame at most, A#{} on average, hottest A#{hot} C", st.frames, st.frames - last_frames, st.dropped, st.paced, wr >> 20, (wr - last_written) >> 20, st.raw / st.coded.max(1), st.raw * 100 / st.coded.max(1) % 100, st.code_us_most, st.code_us_sum / st.frames.max(1));
            if let Some(g) = rec.gpu() {
                println!("rawrec: A#{el} s: the GPU A#{} us a frame at most, A#{} on average; packed behind the hand-over A#{} us at most, A#{} on average{}", g.run_us_most, g.run_us_sum / g.runs.max(1), g.behind_us_most, g.behind_us_sum / g.runs.max(1), g.failed.map(|e| format!("; the GPU failed: {e}")).unwrap_or_default());
            }
            last_frames = st.frames;
            last_written = wr;
            // An unknown fact reads all ones: only a real reading (under 2^10 C) ends the run.
            if hot >= stop_c && hot < 1 << 10 {
                stopped = format!("the hottest zone reached A#{hot} C");
                break;
            }
        }
        if !map(held, &mut hd) {
            stopped = String::from("MAP failed");
            break;
        }
        held = hd[40];
    }
    let el = t0.elapsed();
    let gs = rec.gpu();
    let info = rec.stop();
    let _ = call(S_CAMERA, &[6, held], &mut r);
    let n = call(S_CAMERA, &[4], &mut r);
    let s = info.stats;
    println!("rawrec: stopped: {stopped}; camera CLOSE G#{:X}", if n == ERR { 0xEEE } else { r[0] as u64 });
    println!(
        "rawrec: A#{} frames in A#{} ms (A#{} a second), A#{} dropped, A#{} paced; coded A#{} of A#{} bytes (ratio A#{}.{:02}); the clip A#{} bytes, written in A#{} ms of the writer's time (A#{} MB/s while writing, A#{} MB/s over the clip); coding A#{} us a frame at most, A#{} on average; hottest A#{hot} C{}",
        s.frames,
        el.as_millis(),
        s.frames * 1000 / (el.as_millis() as u64).max(1),
        s.dropped,
        s.paced,
        s.coded,
        s.raw,
        s.raw / s.coded.max(1),
        s.raw * 100 / s.coded.max(1) % 100,
        info.len,
        info.write_us / 1000,
        info.len / info.write_us.max(1),
        info.len / (el.as_micros() as u64).max(1),
        s.code_us_most,
        s.code_us_sum / s.frames.max(1),
        info.error.as_ref().map(|e| format!("; {e}")).unwrap_or_default()
    );
    if let Some(g) = gs {
        println!("rawrec: the GPU: A#{} frames run, A#{} us a frame at most, A#{} on average; packed behind the hand-over A#{} us at most, A#{} on average{}", g.runs, g.run_us_most, g.run_us_sum / g.runs.max(1), g.behind_us_most, g.behind_us_sum / g.runs.max(1), g.failed.map(|e| format!("; the GPU failed: {e}")).unwrap_or_default());
    }
    println!("rawrec: seal {}", hex(&info.hash));
    if let Some(i) = ext.info() {
        println!("rawrec: the extent: A#{} blocks from block A#{} of the clips area (partition block A#{}), A#{} bytes, sealed {}", i.blocks, i.first, (6u64 << 20) + i.first, i.bytes, i.sealed);
        println!("rawrec: the recording space: {:?}", ferros_clip::ferros::space(slot));
    }
    let doc = hashes_doc(&hashes);
    println!("rawrec: A#{} frames' source hashes stored as test.rawrec.hashes ({} bytes): {}", hashes.len(), doc.len(), store_put(slot, "rawrec.hashes", &doc));
    if !verify || info.len == 0 {
        println!("rawrec: ends");
        return;
    }
    // The clip read back from the flash and decoded here: every hashed frame against its hash, the copies sample for sample.
    let t = Instant::now();
    let reader = match vsf::visual::clip::Reader::open(ExtentSource { extent: Extent::new(slot, &name), len: info.len }) {
        Ok(r) => r,
        Err(e) => {
            println!("rawrec: the clip did not open from the flash: {e}");
            return;
        }
    };
    println!("rawrec: read back: A#{} frames by its index (recovered {}), opened in A#{} ms", reader.frames().len(), reader.recovered(), t.elapsed().as_millis());
    let (mut good, mut bad) = (0, 0);
    for (k, hh) in hashes.iter().filter(|(k, _)| k & ((1 << hash_log2) - 1) == 0) {
        let px = match reader.decode(*k as usize) {
            Ok(px) => px,
            Err(e) => {
                println!("rawrec: frame A#{k} did not decode: {e}");
                bad += 1;
                continue;
            }
        };
        // Without source digests the recorder keeps no frame's hash: the copies below are the check.
        if !digests {
        } else if ferros_clip::samples_source_hash(&px, w, h) == *hh {
            good += 1;
        } else {
            bad += 1;
            println!("rawrec: frame A#{k} differs from the frame captured");
        }
        if let Some((_, raw)) = copies.iter().find(|(c, _)| c == k) {
            let same = (0..h).all(|y| (0..w / 4).all(|g| (0..4).all(|i| px[y * w + g * 4 + i] == ((raw[y * stride + g * 5 + i] as u16) << 2 | ((raw[y * stride + g * 5 + 4] >> (2 * i)) & 3) as u16))));
            println!("rawrec: frame A#{k} against its copy, sample for sample: {}", if same { "equal" } else { "DIFFERENT" });
            if !digests {
                if same {
                    good += 1;
                } else {
                    bad += 1;
                }
            }
        }
    }
    println!("rawrec: decoded on the device: A#{good} of the frames checked (every A#{}-th) bit exact, A#{bad} not, in A#{} ms", 1u64 << hash_log2, t.elapsed().as_millis());
    println!("rawrec: ends");
}
