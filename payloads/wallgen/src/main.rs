//! wallgen: the person's wallpaper made once on this device and kept (owner, 2026-11-07: "we don't want to be recomputing the wallpaper every attest"; per device, each screen its own size; a process of its own, since the display's heap cannot hold the document).
//!
//! Given the wallpaper seed (its input, 32 bytes: ahau_core::wallpaper_seed, derived one way from the identity seed), it asks the display for the screen's size. If the person's space already holds a wallpaper made from this seed at this size (`wallpaper.made.key`), it ends.
//! Otherwise it draws the `wallpaper` crate's picture on four threads, in three passes with a barrier between: the first pass, the edges marked, the marked pixels drawn again 4 × 4. The picture is the same one the display draws from the same seed.
//! It packs the picture as a VSF visual (one plane, VSF RGB, gamma2tanh at 16 bits, as tools/wallpaper writes a person's own), stores it as `wallpaper.made` in pieces and then its key, and asks the display to show it (op 44).
//!
//! The heap is 96 MiB: the frame (working values, 32 MB), then the samples (24 MB), then the document: writing it holds about three copies of the samples (the packed tensor, the section the builder keeps, the bytes written); 60 MiB failed at the write, 2026-11-07.
//!
//! Grant slots: 0 console, 1 the vault scope `map.`, 2 use display. Input: the wallpaper seed.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use ferros_sys::{call, cap, console, freq, now, ERR};

const HEAP_BYTES: usize = 96 << 20;
const STACK_BYTES: usize = 1 << 16;
const WORKERS: usize = 3;
/// Rows a thread takes at a time.
const BAND: usize = 1 << 5;

#[repr(C, align(16))]
struct Region<const N: usize>(UnsafeCell<[u8; N]>);
unsafe impl<const N: usize> Sync for Region<N> {}
static HEAP: Region<HEAP_BYTES> = Region(UnsafeCell::new([0; HEAP_BYTES]));
static STACKS: [Region<STACK_BYTES>; WORKERS] = [const { Region(UnsafeCell::new([0; STACK_BYTES])) }; WORKERS];

core::arch::global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 0
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad 0
    .size _entry, . - _entry
"#,
    main = sym wall_main
);

#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

const S_STORE: u64 = 1;
const S_DISPLAY: u64 = 2;
static mut GRANTS: u64 = 0;

struct Log {
    b: [u8; 256],
    n: usize,
}

impl core::fmt::Write for Log {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}

macro_rules! say {
    ($($a:tt)*) => {{
        let mut l = Log { b: [0; 256], n: 0 };
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!($($a)*));
        let _ = core::fmt::Write::write_str(&mut l, "\n");
        console(unsafe { GRANTS }, &l.b[..l.n]);
    }};
}

/// The shared job: the pass the threads are on (0 none, 1 draw, 2 mark, 3 refine), the next band to take, the bands done, and the frame, the mask and the picture's parameters they work on.
static PASS: AtomicU64 = AtomicU64::new(0);
static NEXT: AtomicUsize = AtomicUsize::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
static FRAME: AtomicUsize = AtomicUsize::new(0);
static MASK: AtomicUsize = AtomicUsize::new(0);
static mut PARAMS: Option<wallpaper::Params> = None;

/// Bands of the current pass, taken one at a time until none is left. Each touches only its own rows of what it writes (the mark pass reads the whole frame, which no thread writes in that pass).
fn take_bands(pass: u64) {
    let p = unsafe { (*(&raw const PARAMS)).as_ref().expect("params") };
    let (w, h) = (p.width, p.height);
    let stride = wallpaper::mask_stride(w);
    loop {
        let b = NEXT.fetch_add(1, Ordering::AcqRel);
        let y0 = b * BAND;
        if y0 >= h {
            return;
        }
        // The last band stops at the screen's bottom.
        let y1 = (y0 + BAND).min(h);
        let frame = unsafe { core::slice::from_raw_parts_mut(FRAME.load(Ordering::Acquire) as *mut u64, w * h) };
        let mask = unsafe { core::slice::from_raw_parts_mut(MASK.load(Ordering::Acquire) as *mut u8, stride * h) };
        match pass {
            1 => wallpaper::render_rows(p, y0..y1, &mut frame[y0 * w..y1 * w]),
            2 => {
                let (all, rows) = unsafe { (core::slice::from_raw_parts(frame.as_ptr(), w * h), &mut *(&raw mut mask[y0 * stride..y1 * stride])) };
                wallpaper::mark_rows(p, all, y0..y1, rows);
            }
            _ => wallpaper::refine_rows(p, y0..y1, &mask[y0 * stride..y1 * stride], &mut frame[y0 * w..y1 * w]),
        }
        DONE.fetch_add(1, Ordering::AcqRel);
        ferros_sys::wake(&DONE, 1);
    }
}

extern "C" fn worker(_: u64) -> ! {
    let mut seen = 0u64;
    loop {
        let p = PASS.load(Ordering::Acquire);
        if p == seen {
            ferros_sys::wait(&PASS, p, 0);
            continue;
        }
        seen = p;
        take_bands(p & 3);
    }
}

/// One pass on every thread, this one too, and back once every band is done.
fn pass(n: u64, bands: u64, round: &mut u64) {
    NEXT.store(0, Ordering::Release);
    DONE.store(0, Ordering::Release);
    // The pass word moves every round (the low bits the pass), so a worker never mistakes the next round for one it has done.
    *round += 4;
    PASS.store(*round | n, Ordering::Release);
    ferros_sys::wake(&PASS, WORKERS as u64);
    take_bands(n);
    loop {
        let d = DONE.load(Ordering::Acquire);
        if d >= bands {
            return;
        }
        ferros_sys::wait(&DONE, d, 0);
    }
}

fn ms(t: u64) -> u64 {
    (now() - t) * 1000 / freq()
}

#[unsafe(no_mangle)]
pub extern "C" fn wall_main(g: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_BYTES) };
    unsafe { GRANTS = g };
    if input_len != 32 {
        say!("wallgen: no wallpaper seed (32 bytes of input)");
        ferros_sys::exit(1);
    }
    let mut seed = [0u8; 32];
    for (k, b) in seed.iter_mut().enumerate() {
        *b = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + k as u64) as *const u8) };
    }
    let (store, display) = (cap(g, S_STORE), cap(g, S_DISPLAY));
    let mut q = [0u8; 16];
    if call(display, &[5], &mut q) < 4 {
        say!("wallgen: the display did not say the screen's size");
        ferros_sys::exit(1);
    }
    let (w, h) = (u16::from_le_bytes([q[0], q[1]]) as usize, u16::from_le_bytes([q[2], q[3]]) as usize);
    // What this wallpaper is: its seed and the screen it is for.
    let mut kh = blake3::Hasher::new();
    kh.update(&seed);
    kh.update(&(w as u32).to_le_bytes());
    kh.update(&(h as u32).to_le_bytes());
    let key = *kh.finalize().as_bytes();
    let mut have = [0u8; 32];
    if ferros_sys::store_get(store, b"wallpaper.made.key", &mut have) == Some(32) && have == key {
        say!("wallgen: this device already keeps the person's wallpaper for this screen");
        ferros_sys::exit(0);
    }
    for k in 0..WORKERS {
        ferros_sys::thread_spawn(worker, unsafe { &mut *STACKS[k].0.get() }, k as u64);
    }
    let t_all = now();
    let p = wallpaper::params(&seed, w, h);
    unsafe { PARAMS = Some(p) };
    let mut frame: Vec<u64> = alloc::vec![0; w * h];
    let mut mask: Vec<u8> = alloc::vec![0; wallpaper::mask_stride(w) * h];
    FRAME.store(frame.as_mut_ptr() as usize, Ordering::Release);
    MASK.store(mask.as_mut_ptr() as usize, Ordering::Release);
    let bands = h.div_ceil(BAND) as u64;
    let mut round = 0u64;
    let t = now();
    pass(1, bands, &mut round);
    let t_draw = ms(t);
    let t = now();
    pass(2, bands, &mut round);
    pass(3, bands, &mut round);
    let t_refine = ms(t);
    drop(mask);
    // The working values (gamma-2 codes, white at 16384) as gamma2tanh samples of their light, one plane a channel.
    let t = now();
    let n = w * h;
    let mut samples: Vec<u16> = alloc::vec![0; 3 * n];
    for (i, &v) in frame.iter().enumerate() {
        for c in 0..3 {
            let code = ((v >> (16 * c)) & 0xFFFF) as f32 * (1.0 / 16384.0);
            samples[c * n + i] = vsf::colour::convert::delinearize_gamma2tanh_bits_f32(code * code, 16) as u16;
        }
    }
    drop(frame);
    let tensor = vsf::types::BitPackedTensor::pack(16, alloc::vec![3, h, w], &samples);
    drop(samples);
    let mut plane = vsf::visual::Plane::encoded("display", "wallpaper", w, h, vsf::visual::Primaries::Vsf, vsf::visual::Encoding::Gamma2Tanh, tensor);
    plane.name = "main".into();
    let v = vsf::visual::Visual { title: "wallpaper".into(), text: "The person's wallpaper, made on this device from their wallpaper seed (wallgen).".into(), planes: alloc::vec![plane], ..Default::default() };
    let bytes = match v.write() {
        Ok(b) => b,
        Err(_) => {
            say!("wallgen: the visual did not write");
            ferros_sys::exit(1);
        }
    };
    drop(v);
    let t_pack = ms(t);
    // Into the person's space in pieces, then its key: a wallpaper with no key is one an interrupted run left, made again next time.
    let t = now();
    let mut off = 0;
    while off < bytes.len() {
        let end = (off + 4096).min(bytes.len());
        if !ferros_sys::store_set_at(store, b"wallpaper.made", off, &bytes[off..end], end < bytes.len()) {
            say!("wallgen: the vault refused a piece at {}", off);
            ferros_sys::exit(1);
        }
        off = end;
    }
    if !ferros_sys::store_set(store, b"wallpaper.made.key", &key) {
        say!("wallgen: the vault refused the key");
        ferros_sys::exit(1);
    }
    let t_store = ms(t);
    say!("wallgen: made {}x{} in {} ms (drawn {} ms, refined {} ms, packed {} ms, stored {} B in {} ms)", w, h, ms(t_all), t_draw, t_refine, t_pack, bytes.len(), t_store);
    let mut r = [0u8; 4];
    if call(display, &[44], &mut r) == ERR {
        say!("wallgen: the display did not take op 44");
    }
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("wallgen: panic: {}", info);
    ferros_sys::exit(1)
}
