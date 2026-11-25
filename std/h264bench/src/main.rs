//! OpenH264 on ferros (docs/STD-TARGET.md "C++ through ferros_rt"): synthetic frames encoded and decoded round trip, the picture's PSNR checked, encode and decode timed per frame, at beam sizes, one slice and several, one encoder thread and several.
//! The raw C API (openh264-sys2) for the slice and thread settings the openh264 crate does not expose, and the crate's own Encoder and Decoder once (Photon's path, src/wave/h264.rs).
//! Every case prints one line; the process exits 0 only when every picture came back above the PSNR floor.

use std::time::Instant;

use openh264::formats::{YUVSlices, YUVSource};
use openh264_sys2 as sys;

/// The PSNR every decoded picture must reach (luma, dB): a 4 Mbit/s baseline stream of this content lands well above it; a broken codec (wrong NEON, a corrupted reference) lands far below.
const PSNR_FLOOR: f64 = 30.;

/// Frames a case encodes: the first an IDR, the rest P frames.
const FRAMES: usize = 1 << 5;

struct Picture {
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

/// Frame `n` of a moving test scene: a diagonal gradient scrolling, a textured square crossing the frame, and a little deterministic noise (so the encoder has detail and motion to code, never a flat field).
fn scene(w: usize, h: usize, n: usize) -> Picture {
    let mut rng = 0x9E37_79B9_7F4A_7C15u64 ^ n as u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let (bx, by, side) = ((n * 7) % w, (n * 3) % h, w / 5);
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            let g = ((c + r + 2 * n) & 0xFF) as u8;
            // WHY wrapping: `c - bx < side` as one unsigned compare is bx <= c < bx + side (left of the square wraps to a huge number).
            let inside = c.wrapping_sub(bx) < side && r.wrapping_sub(by) < side;
            let t = if inside { (((c ^ r) & 8) * 24) as u8 } else { g };
            y[r * w + c] = t.wrapping_add((next() & 3) as u8);
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    for r in 0..ch {
        for c in 0..cw {
            u[r * cw + c] = (64 + ((c + n) & 0x7F)) as u8;
            v[r * cw + c] = (64 + ((r + 2 * n) & 0x7F)) as u8;
        }
    }
    Picture { y, u, v }
}

/// Luma PSNR of a decoded plane (`stride` bytes a row) against the source.
fn psnr(src: &[u8], w: usize, h: usize, dec: &[u8], stride: usize) -> f64 {
    let mut se = 0u64;
    for r in 0..h {
        for c in 0..w {
            let d = src[r * w + c] as i64 - dec[r * stride + c] as i64;
            se += (d * d) as u64;
        }
    }
    if se == 0 {
        return 99.;
    }
    let mse = se as f64 / (w * h) as f64;
    10. * (255. * 255. / mse).log10()
}

/// Microseconds, sorted: (mean, median, largest).
fn stats(mut v: Vec<u128>) -> (u128, u128, u128) {
    v.sort_unstable();
    (v.iter().sum::<u128>() / v.len() as u128, v[v.len() / 2], v[v.len() - 1])
}

struct Case {
    w: usize,
    h: usize,
    slices: u32,
    threads: u16,
    bps: i32,
}

/// One case through the raw API; true when every picture passed.
fn raw_case(k: &Case) -> bool {
    unsafe {
        let mut enc: *mut sys::ISVCEncoder = std::ptr::null_mut();
        assert_eq!(sys::source::APILoader::WelsCreateSVCEncoder(&mut enc), 0);
        let ev = &**enc;
        let mut p: sys::SEncParamExt = std::mem::zeroed();
        (ev.GetDefaultParams.unwrap())(enc, &mut p);
        p.iUsageType = sys::CAMERA_VIDEO_REAL_TIME;
        p.iPicWidth = k.w as i32;
        p.iPicHeight = k.h as i32;
        p.iTargetBitrate = k.bps;
        p.iRCMode = sys::RC_BITRATE_MODE;
        p.fMaxFrameRate = 30.;
        p.bEnableFrameSkip = false;
        p.uiIntraPeriod = 0;
        p.iMultipleThreadIdc = k.threads;
        p.iSpatialLayerNum = 1;
        let l = &mut p.sSpatialLayers[0];
        l.iVideoWidth = k.w as i32;
        l.iVideoHeight = k.h as i32;
        l.fFrameRate = 30.;
        l.iSpatialBitrate = k.bps;
        l.uiProfileIdc = sys::PRO_BASELINE;
        l.sSliceArgument.uiSliceMode = if k.slices == 1 { sys::SM_SINGLE_SLICE } else { sys::SM_FIXEDSLCNUM_SLICE };
        l.sSliceArgument.uiSliceNum = k.slices;
        assert_eq!((ev.InitializeExt.unwrap())(enc, &p), 0, "encoder init");

        let mut dec: *mut sys::ISVCDecoder = std::ptr::null_mut();
        assert_eq!(sys::source::APILoader::WelsCreateDecoder(&mut dec), 0);
        let dv = &**dec;
        let mut dp: sys::SDecodingParam = std::mem::zeroed();
        dp.eEcActiveIdc = sys::ERROR_CON_SLICE_MV_COPY_CROSS_IDR;
        dp.sVideoProperty.eVideoBsType = sys::VIDEO_BITSTREAM_AVC;
        assert_eq!((dv.Initialize.unwrap())(dec, &dp), 0, "decoder init");

        let frames: Vec<Picture> = (0..FRAMES).map(|n| scene(k.w, k.h, n)).collect();
        let (mut te, mut td, mut ps) = (Vec::new(), Vec::new(), Vec::new());
        let (mut bytes, mut nals) = (0usize, Vec::new());
        let mut ok = true;
        for (n, f) in frames.iter().enumerate() {
            let mut pic: sys::SSourcePicture = std::mem::zeroed();
            pic.iColorFormat = sys::videoFormatI420 as i32;
            pic.iPicWidth = k.w as i32;
            pic.iPicHeight = k.h as i32;
            pic.iStride = [k.w as i32, (k.w / 2) as i32, (k.w / 2) as i32, 0];
            pic.pData = [f.y.as_ptr() as *mut u8, f.u.as_ptr() as *mut u8, f.v.as_ptr() as *mut u8, std::ptr::null_mut()];
            pic.uiTimeStamp = (n * 1000 / 30) as i64;
            let mut info: sys::SFrameBSInfo = std::mem::zeroed();
            let t = Instant::now();
            let r = (ev.EncodeFrame.unwrap())(enc, &pic, &mut info);
            te.push(t.elapsed().as_micros());
            assert_eq!(r, 0, "encode frame {n}");
            // The access unit: every layer's NAL units back to back (Annex B, start codes in place).
            let mut au = Vec::new();
            let mut slice_nals = 0;
            for li in 0..info.iLayerNum as usize {
                let layer = &info.sLayerInfo[li];
                let lens = std::slice::from_raw_parts(layer.pNalLengthInByte, layer.iNalCount as usize);
                let mut at = layer.pBsBuf;
                for &len in lens {
                    let nal = std::slice::from_raw_parts(at, len as usize);
                    // The NAL type after the start code (00 00 00 01 or 00 00 01): 1 and 5 are slices.
                    let sc = if nal[2] == 1 { 3 } else { 4 };
                    if matches!(nal[sc] & 0x1F, 1 | 5) {
                        slice_nals += 1;
                    }
                    au.extend_from_slice(nal);
                    at = at.add(len as usize);
                }
            }
            nals.push(slice_nals);
            bytes += au.len();
            let mut dst = [std::ptr::null_mut::<u8>(); 3];
            let mut bi: sys::SBufferInfo = std::mem::zeroed();
            let t = Instant::now();
            let state = (dv.DecodeFrameNoDelay.unwrap())(dec, au.as_ptr(), au.len() as i32, dst.as_mut_ptr(), &mut bi);
            td.push(t.elapsed().as_micros());
            if state != 0 || bi.iBufferStatus != 1 {
                println!("h264bench: frame {n}: decode state {state}, picture {}", bi.iBufferStatus);
                ok = false;
                continue;
            }
            let sb = bi.UsrData.sSystemBuffer;
            assert_eq!((sb.iWidth as usize, sb.iHeight as usize), (k.w, k.h), "decoded size");
            let y = std::slice::from_raw_parts(dst[0], sb.iStride[0] as usize * k.h);
            ps.push(psnr(&f.y, k.w, k.h, y, sb.iStride[0] as usize));
        }
        (ev.Uninitialize.unwrap())(enc);
        sys::source::APILoader::WelsDestroySVCEncoder(enc);
        (dv.Uninitialize.unwrap())(dec);
        sys::source::APILoader::WelsDestroyDecoder(dec);

        let (em, e50, emax) = stats(te);
        let (dm, d50, dmax) = stats(td);
        let pmin = ps.iter().cloned().fold(f64::INFINITY, f64::min);
        let pmean = ps.iter().sum::<f64>() / ps.len().max(1) as f64;
        let (smin, smax) = (*nals.iter().min().unwrap(), *nals.iter().max().unwrap());
        ok &= ps.len() == FRAMES && pmin >= PSNR_FLOOR && smin as u32 == k.slices;
        println!(
            "h264bench: {}x{} slices {} (seen {}-{}) threads {}: encode us mean {} p50 {} max {}; decode us mean {} p50 {} max {}; {} bytes a frame; PSNR-Y mean {:.2} min {:.2}: {}",
            k.w, k.h, k.slices, smin, smax, k.threads, em, e50, emax, dm, d50, dmax, bytes / FRAMES, pmean, pmin,
            if ok { "pass" } else { "FAIL" }
        );
        ok
    }
}

/// The openh264 crate's own Encoder and Decoder with Photon's encoder settings (src/wave/h264.rs): the API Photon calls, on ferros.
fn crate_case(w: usize, h: usize) -> bool {
    use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, Profile, RateControlMode, UsageType};
    use openh264::{OpenH264API, Timestamp};
    let cfg = EncoderConfig::new()
        .usage_type(UsageType::CameraVideoRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .bitrate(BitRate::from_bps(4_000_000))
        .max_frame_rate(FrameRate::from_hz(30.))
        .skip_frames(false)
        .profile(Profile::Baseline)
        .intra_frame_period(IntraFramePeriod::from_num_frames(0));
    let mut enc = Encoder::with_api_config(OpenH264API::from_source(), cfg).expect("crate encoder");
    let mut dec = openh264::decoder::Decoder::new().expect("crate decoder");
    let mut pmin = f64::INFINITY;
    let mut pictures = 0;
    for n in 0..FRAMES {
        let f = scene(w, h, n);
        let src = YUVSlices::new((&f.y, &f.u, &f.v), (w, h), (w, w / 2, w / 2));
        let au = enc.encode_at(&src, Timestamp::from_millis((n * 1000 / 30) as u64)).expect("crate encode").to_vec();
        if let Some(pic) = dec.decode(&au).expect("crate decode") {
            let (sy, _, _) = pic.strides();
            pmin = pmin.min(psnr(&f.y, w, h, pic.y(), sy));
            pictures += 1;
        }
    }
    let ok = pictures == FRAMES && pmin >= PSNR_FLOOR;
    println!("h264bench: openh264 crate {w}x{h}: {pictures} pictures, PSNR-Y min {pmin:.2}: {}", if ok { "pass" } else { "FAIL" });
    ok
}

/// ferros_rt's POSIX shims (posix.rs), called as C calls them (ferros only: elsewhere these names are the host libc's, with other layouts): the layouts its header set gives (std/openh264-sys2/c/include).
#[cfg(target_os = "ferros")]
mod posix {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct Timespec {
        pub sec: i64,
        pub nsec: i64,
    }

    unsafe extern "C" {
        pub fn pthread_create(t: *mut usize, attr: *const c_void, start: unsafe extern "C" fn(*mut c_void) -> *mut c_void, arg: *mut c_void) -> i32;
        pub fn pthread_join(t: usize, ret: *mut *mut c_void) -> i32;
        pub fn pthread_mutex_init(m: *mut u64, attr: *const c_void) -> i32;
        pub fn pthread_mutex_lock(m: *mut u64) -> i32;
        pub fn pthread_mutex_unlock(m: *mut u64) -> i32;
        pub fn pthread_cond_init(c: *mut u64, attr: *const c_void) -> i32;
        pub fn pthread_cond_wait(c: *mut u64, m: *mut u64) -> i32;
        pub fn pthread_cond_timedwait(c: *mut u64, m: *mut u64, at: *const Timespec) -> i32;
        pub fn pthread_cond_signal(c: *mut u64) -> i32;
        pub fn sem_init(s: *mut [u64; 2], shared: i32, v: u32) -> i32;
        pub fn sem_wait(s: *mut [u64; 2]) -> i32;
        pub fn sem_post(s: *mut [u64; 2]) -> i32;
        pub fn sem_timedwait(s: *mut [u64; 2], at: *const Timespec) -> i32;
        pub fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
        pub fn __errno_location() -> *mut i32;
        pub fn __cxa_guard_acquire(g: *mut u64) -> i32;
        pub fn __cxa_guard_release(g: *mut u64);
    }
}

#[cfg(target_os = "ferros")]
/// The shared state of the POSIX check's threads.
struct Shared {
    m: u64,
    c: u64,
    count: u64,
    ready: u64,
    ping: [u64; 2],
    pong: [u64; 2],
    guard: u64,
    inits: u64,
}

#[cfg(target_os = "ferros")]
/// Each worker: a run of locked increments, a static-init guard raced with the others, then (worker 0 only) the semaphore ping-pong with main.
unsafe extern "C" fn worker(arg: *mut std::ffi::c_void) -> *mut std::ffi::c_void {
    let s = arg as *mut Shared;
    unsafe {
        for _ in 0..(1 << 14) {
            posix::pthread_mutex_lock(&raw mut (*s).m);
            (*s).count += 1;
            posix::pthread_mutex_unlock(&raw mut (*s).m);
        }
        if posix::__cxa_guard_acquire(&raw mut (*s).guard) == 1 {
            // Held long enough that the others find the guard busy and sleep on it.
            std::thread::sleep(std::time::Duration::from_millis(2));
            posix::pthread_mutex_lock(&raw mut (*s).m);
            (*s).inits += 1;
            posix::pthread_mutex_unlock(&raw mut (*s).m);
            posix::__cxa_guard_release(&raw mut (*s).guard);
        }
        posix::pthread_mutex_lock(&raw mut (*s).m);
        (*s).ready += 1;
        posix::pthread_cond_signal(&raw mut (*s).c);
        posix::pthread_mutex_unlock(&raw mut (*s).m);
    }
    arg
}

#[cfg(target_os = "ferros")]
unsafe extern "C" fn ponger(arg: *mut std::ffi::c_void) -> *mut std::ffi::c_void {
    let s = arg as *mut Shared;
    unsafe {
        for _ in 0..(1 << 10) {
            posix::sem_wait(&raw mut (*s).ping);
            posix::sem_post(&raw mut (*s).pong);
        }
    }
    std::ptr::null_mut()
}

#[cfg(target_os = "ferros")]
/// The shims as openh264's threads use them: four pthreads counting under one mutex, a condition variable that tells main they are done, a static-init guard that only one may run, a semaphore round trip between two threads (timed: a wake's cost), and timed waits that must time out at their deadline.
fn posix_check() -> bool {
    use posix::*;
    let mut ok = true;
    let s = Box::into_raw(Box::new(Shared { m: 0, c: 0, count: 0, ready: 0, ping: [0; 2], pong: [0; 2], guard: 0, inits: 0 }));
    unsafe {
        pthread_mutex_init(&raw mut (*s).m, std::ptr::null());
        pthread_cond_init(&raw mut (*s).c, std::ptr::null());
        sem_init(&raw mut (*s).ping, 0, 0);
        sem_init(&raw mut (*s).pong, 0, 0);
        let mut ts = [0usize; 4];
        for t in ts.iter_mut() {
            assert_eq!(pthread_create(t, std::ptr::null(), worker, s as *mut _), 0);
        }
        pthread_mutex_lock(&raw mut (*s).m);
        while (*s).ready < 4 {
            pthread_cond_wait(&raw mut (*s).c, &raw mut (*s).m);
        }
        pthread_mutex_unlock(&raw mut (*s).m);
        for &t in &ts {
            let mut ret = std::ptr::null_mut();
            pthread_join(t, &mut ret);
            ok &= ret == s as *mut _;
        }
        ok &= (*s).count == 4 << 14 && (*s).inits == 1 && (*s).guard & 0xFF == 1;
        println!("h264bench: posix: 4 threads counted {} of {}, the guard ran its initialiser {} time(s)", (*s).count, 4 << 14, (*s).inits);

        let mut p = 0usize;
        assert_eq!(pthread_create(&mut p, std::ptr::null(), ponger, s as *mut _), 0);
        let mut rt = Vec::new();
        for _ in 0..(1 << 10) {
            let t = Instant::now();
            sem_post(&raw mut (*s).ping);
            sem_wait(&raw mut (*s).pong);
            rt.push(t.elapsed().as_nanos() / 1000);
        }
        pthread_join(p, std::ptr::null_mut());
        let (m, p50, max) = stats(rt);
        println!("h264bench: posix: semaphore round trip between two threads, us mean {m} p50 {p50} max {max}");

        // Timed waits on a semaphore nobody posts and a condition nobody signals: each must answer ETIMEDOUT at its deadline, 2 ms on.
        for which in ["sem_timedwait", "pthread_cond_timedwait"] {
            let mut now = Timespec { sec: 0, nsec: 0 };
            clock_gettime(0, &mut now);
            let ns = now.nsec + 2_000_000;
            let at = Timespec { sec: now.sec + ns / 1_000_000_000, nsec: ns % 1_000_000_000 };
            let t = Instant::now();
            let r = if which == "sem_timedwait" {
                let r = sem_timedwait(&raw mut (*s).ping, &at);
                if r == 0 { 0 } else { *__errno_location() }
            } else {
                pthread_mutex_lock(&raw mut (*s).m);
                let r = pthread_cond_timedwait(&raw mut (*s).c, &raw mut (*s).m, &at);
                pthread_mutex_unlock(&raw mut (*s).m);
                r
            };
            let us = t.elapsed().as_micros();
            let good = r == 110 && (2000..4000).contains(&us);
            ok &= good;
            println!("h264bench: posix: {which} to 2 ms: {} after {us} us: {}", if r == 110 { "ETIMEDOUT" } else { "no timeout" }, if good { "pass" } else { "FAIL" });
        }
        drop(Box::from_raw(s));
    }
    println!("h264bench: posix: {}", if ok { "pass" } else { "FAIL" });
    ok
}

/// How long the whole run may take before the watchdog ends it: the bench runs in seconds, and the Pro is shared.
const WATCHDOG_S: u64 = 1 << 6;

fn main() {
    // A real deadline, not a poll: if a shim deadlocked, the bench would sit on the shared device with its threads asleep; at the deadline it says so and the process ends (SYS_EXIT ends every thread of it).
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(WATCHDOG_S));
        println!("h264bench: watchdog: still running after {WATCHDOG_S} s, ending");
        std::process::exit(2);
    });
    let v = unsafe { sys::source::APILoader::WelsGetCodecVersion() };
    println!("h264bench: OpenH264 {}.{}.{}", v.uMajor, v.uMinor, v.uRevision);
    #[cfg(target_os = "ferros")]
    let posix_ok = posix_check();
    #[cfg(not(target_os = "ferros"))]
    let posix_ok = true;
    let cases = [
        Case { w: 1280, h: 720, slices: 1, threads: 1, bps: 4_000_000 },
        Case { w: 1280, h: 720, slices: 4, threads: 1, bps: 4_000_000 },
        Case { w: 1280, h: 720, slices: 4, threads: 4, bps: 4_000_000 },
        Case { w: 672, h: 1496, slices: 1, threads: 1, bps: 4_000_000 },
        Case { w: 672, h: 1496, slices: 8, threads: 4, bps: 4_000_000 },
    ];
    let mut ok = posix_ok & crate_case(1280, 720);
    for k in &cases {
        ok &= raw_case(k);
    }
    println!("h264bench: {}", if ok { "all pass" } else { "FAILED" });
    std::process::exit(if ok { 0 } else { 1 });
}
