//! beamtest: a test-card beam from the phone, to prove the whole send path on hardware (BEAM.md step 4).
//!
//! Each frame is an animated 4:2:0 test card, encoded as H.264 (photon_core::h264), cut into RaptorQ-coded beam datagrams (photon_core::beam) and sealed under a fixed TEST wave secret, with a plaid audio window of silence beside it (photon_core::wave). Everything goes out through the `udp` service to a receiver built on Photon's own library (tools/photon-interop, `beam-recv`), which holds the same test secret. Not a real wave: there is no friendship and no signalling yet.
//!
//! Spawn after `ip`: ferros-bridge spawn beamtest.bin <input hex> --name beamtest --use udp
//! Grant slots: 0 console, 1 udp. Input: destination IPv4 (4 bytes), port (u16 BE), frame count (u16 BE), frames per second (u8).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use ferros_sys::{arc_base, cap, console, input, now, us_since, ERR};
use photon_core::{beam, h264, wave};

/// The heap lives in `.data`, not `.bss`: the kernel sizes an arc from the flat image, which carries no `.bss`, so a `.bss` heap would overlap the stack.
const HEAP_BYTES: usize = 1536 * 1024;

#[repr(C, align(16))]
struct Heap(UnsafeCell<[u8; HEAP_BYTES]>);
unsafe impl Sync for Heap {}
#[unsafe(link_section = ".data.heap")]
static HEAP: Heap = Heap(UnsafeCell::new([1; HEAP_BYTES]));

/// A bump allocator that starts over whenever every allocation has been freed: once per frame here, since nothing that lives across frames is on the heap. Single-threaded, so plain cells suffice.
struct Bump {
    next: UnsafeCell<usize>,
    live: UnsafeCell<usize>,
}
unsafe impl Sync for Bump {}

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let base = HEAP.0.get() as usize;
        let start = (base + *self.next.get() + l.align() - 1) & !(l.align() - 1);
        if start + l.size() > base + HEAP_BYTES {
            return core::ptr::null_mut();
        }
        *self.next.get() = start + l.size() - base;
        *self.live.get() += 1;
        start as *mut u8
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {
        *self.live.get() -= 1;
        if *self.live.get() == 0 {
            *self.next.get() = 0;
        }
    }
}

#[global_allocator]
static ALLOC: Bump = Bump { next: UnsafeCell::new(0), live: UnsafeCell::new(0) };

/// The TEST wave secret, shared with the dev-box receiver. A real wave derives it from the friendship and both sides' nonces.
fn test_secret() -> [u8; 32] {
    wave::derive_wave_secret(&[0xFE; 32], &[0xBE; 16], &[0xA7; 32], &[0xC0; 32])
}

const W: usize = 176;
const H: usize = 144;
const SRC_PORT: u16 = 4393;

struct Out {
    grants: u64,
    dest: [u8; 4],
    port: u16,
    sent: u32,
    failed: u32,
}

impl Out {
    /// One datagram through the udp service; a first send may only resolve the next hop, so retry a few times.
    fn send(&mut self, payload: &[u8]) {
        let mut req = Vec::with_capacity(22 + payload.len());
        req.extend_from_slice(&[1, 4]);
        req.extend_from_slice(&self.dest);
        req.extend_from_slice(&[0; 12]);
        req.extend_from_slice(&self.port.to_be_bytes());
        req.extend_from_slice(&SRC_PORT.to_be_bytes());
        req.extend_from_slice(payload);
        let mut r = [0u8; 16];
        for _ in 0..64 {
            let n = ferros_sys::call(cap(self.grants, 1), &req, &mut r);
            if n != ERR && n >= 1 && r[0] == 0 {
                self.sent += 1;
                return;
            }
            if n == ERR || r[0] != 1 {
                break;
            }
        }
        self.failed += 1;
    }
}

fn dec(v: u32, out: &mut Vec<u8>) {
    let mut d = [0u8; 10];
    let mut i = d.len();
    let mut v = v;
    loop {
        i -= 1;
        d[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out.extend_from_slice(&d[i..]);
}

/// An animated test card: diagonal bars sliding by the frame number, a bright block marching across, and slow colour bands.
fn card(f: u32, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    for row in 0..H {
        for col in 0..W {
            let bar = ((col + row + f as usize * 3) / 12) % 2;
            let block = col / 16 == f as usize % (W / 16) && row / 16 == 4;
            y[row * W + col] = if block { 235 } else if bar == 0 { 60 + (row * 100 / H) as u8 } else { 180 };
        }
    }
    for row in 0..H / 2 {
        for col in 0..W / 2 {
            u[row * (W / 2) + col] = (64 + (col * 128 / (W / 2))) as u8;
            v[row * (W / 2) + col] = (64 + ((row + f as usize) % (H / 2)) * 128 / (H / 2)) as u8;
        }
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _in: u64, _inlen: u64) -> ! {
    let _ = arc_base();
    let mut cfg = [0u8; 9];
    let got = input(0, &mut cfg);
    if got < 9 {
        console(grants, b"beamtest: input = dest IPv4, port, frames, fps\n");
        ferros_sys::exit(1);
    }
    let mut out = Out { grants, dest: [cfg[0], cfg[1], cfg[2], cfg[3]], port: u16::from_be_bytes([cfg[4], cfg[5]]), sent: 0, failed: 0 };
    let frames = u16::from_be_bytes([cfg[6], cfg[7]]) as u32;
    let fps = cfg[8].max(1) as u64;
    let secret = test_secret();
    let mut tx = beam::BeamTx::new(&secret, wave::Direction::OriginToAnswer);
    let mut audio = wave::AudioTx::new(&secret, wave::Direction::OriginToAnswer);
    let info = beam::Info { width: W as u16, height: H as u16, fps: fps as u8, codec: beam::CODEC_H264, colour: Vec::new() };
    let (mut y, mut u, mut v) = ([0u8; W * H], [0u8; W * H / 4], [0u8; W * H / 4]);
    let silence = [0i16; wave::FRAME_SAMPLES];
    let t0 = now();
    for f in 0..frames {
        if f % beam::INFO_EVERY_FRAMES == 0 {
            let p = tx.info(&info);
            out.send(&p);
            let p = tx.orient(0);
            out.send(&p);
        }
        card(f, &mut y, &mut u, &mut v);
        let au = h264::encode_pcm_idr(&h264::Picture { width: W, height: H, y: &y, u: &u, v: &v }, f % 2);
        for p in tx.frame(f, beam::Gains::NONE, &au) {
            out.send(&p);
        }
        drop(au);
        let p = audio.plaid(f, &silence, None);
        out.send(&p);
        if f % 16 == 15 {
            let mut m = Vec::new();
            m.extend_from_slice(b"beamtest: frame ");
            dec(f + 1, &mut m);
            m.extend_from_slice(b", datagrams sent ");
            dec(out.sent, &mut m);
            m.extend_from_slice(b", failed ");
            dec(out.failed, &mut m);
            m.push(b'\n');
            console(grants, &m);
        }
        // Pace to the frame rate.
        let due = (f as u64 + 1) * 1_000_000 / fps;
        while us_since(t0) < due {}
    }
    let mut m = Vec::new();
    m.extend_from_slice(b"beamtest: done, ");
    dec(frames, &mut m);
    m.extend_from_slice(b" frames, ");
    dec(out.sent, &mut m);
    m.extend_from_slice(b" datagrams sent, ");
    dec(out.failed, &mut m);
    m.extend_from_slice(b" failed\n");
    console(grants, &m);
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
