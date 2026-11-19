//! rawrec: the RAW video recorder's test harness (docs/RAW-VIDEO.md "The recorder").
//! Spawned with the camera and a vault scope, in that order: `ferros-bridge spawn rawrec.bin <hex> --name rawrec --use camera --store test`.
//!
//! The input (hex bytes):
//! - +0 the job: 0 record, 1 the clip's record (info), 2 delete it.
//! - record (+17 the writer's cores: 0 the A510s, 1 the A715s, 2 the X3): +1 frames a second (1 to 60, 0: 30); +2 frames to record, u16 LE (0: by seconds); +4 seconds, u16 LE (0: 64 frames); +6 A715 workers (0 to 4, G#FF none; 0: 4); +7 A510 workers (0 to 4); +8 the shutter in microseconds, u32 LE (0: the driver's); +12 the gain as 2^(n-1) times unity (0: the driver's); +13 every 2^n-th frame hashed as captured (0: 3, every 8th); +14 coded frames waiting for the flash (0: 4); +15 the hottest zone that ends the run, C (0: 85); +16 1: decode the clip on the device after, every hashed frame against its hash (0: yes, 2: no).
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

fn main() {
    let inp = input();
    let arg = |k: usize| inp.get(k).copied().unwrap_or(0);
    let le16 = |k: usize| u16::from_le_bytes([arg(k), arg(k + 1)]) as u64;
    let Some(slot) = ferros_clip::ferros::store_slot() else {
        println!("rawrec: no vault scope among the grants (spawn with --use camera --store test)");
        return;
    };
    let ext = Extent::new(slot, NAME);
    println!("rawrec: the recording space: {:?}", ferros_clip::ferros::space(slot));
    match arg(0) {
        1 => {
            println!("rawrec: {NAME}: {:?}", ext.info());
            return;
        }
        2 => {
            println!("rawrec: {NAME} deleted: {}", ext.delete());
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
    ferros_clip::ferros::pin(0x00F0);
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
    // Room for every frame uncoded (the seal gives back what the clip did not use).
    let blocks = (want * raw_frame + (1 << 24)) >> 12;
    let mut workers = vec![0x00F0u64; a715];
    workers.extend(std::iter::repeat_n(0x000Fu64, a510));
    // The settings' frames let pass (a SET reaches the sensor a frame or two later, and an open's first frames come as the stream settles): the clip starts on a steady stream, as Lumis's does.
    for _ in 0..4 {
        if !map(held, &mut hd) {
            println!("rawrec: MAP: no frame while settling");
            let _ = call(S_CAMERA, &[4], &mut r);
            return;
        }
        held = hd[40];
    }
    // The copies kept whole for the check after the clip, their pages taken now and not in the camera loop (at 30 a second and under only: a copy is a few ms).
    let keep_copies = fps <= 30;
    let mut copy_bufs: Vec<Vec<u8>> = if keep_copies { (0..4).map(|_| vec![0u8; h * stride]).collect() } else { Vec::new() };
    let mode = Mode { pacing, title: format!("rawrec A#{fps} a second"), workers, writer: [0x000F, 0x00F0, 0x0100][arg(17).min(2) as usize], slots, blocks, at: ferros_clip::eagle_osc(w8(&hd, 32)), source_digests: true };
    let mut rec = match Recorder::start(&cam, &mode, Box::new(ExtentSink(Extent::new(slot, NAME)))) {
        Ok(r) => r,
        Err(e) => {
            println!("rawrec: the recorder did not start: {e}");
            let _ = call(S_CAMERA, &[4], &mut r);
            return;
        }
    };
    println!("rawrec: recording into {NAME}, A#{blocks} blocks asked; the mosaic G#{:02X}, black A#{}, white A#{}", hd[9], cam.black, cam.white);
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
    let reader = match vsf::visual::clip::Reader::open(ExtentSource { extent: Extent::new(slot, NAME), len: info.len }) {
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
        if ferros_clip::samples_source_hash(&px, w, h) == *hh {
            good += 1;
        } else {
            bad += 1;
            println!("rawrec: frame A#{k} differs from the frame captured");
        }
        if let Some((_, raw)) = copies.iter().find(|(c, _)| c == k) {
            let same = (0..h).all(|y| (0..w / 4).all(|g| (0..4).all(|i| px[y * w + g * 4 + i] == ((raw[y * stride + g * 5 + i] as u16) << 2 | ((raw[y * stride + g * 5 + 4] >> (2 * i)) & 3) as u16))));
            println!("rawrec: frame A#{k} against its copy, sample for sample: {}", if same { "equal" } else { "DIFFERENT" });
        }
    }
    println!("rawrec: decoded on the device: A#{good} of the frames checked (every A#{}-th) bit exact, A#{bad} not, in A#{} ms", 1u64 << hash_log2, t.elapsed().as_millis());
    println!("rawrec: ends");
}
