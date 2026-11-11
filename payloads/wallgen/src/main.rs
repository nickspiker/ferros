//! wallgen: the person's wallpaper made once on this device and kept (owner, 2026-11-07: "we don't want to be recomputing the wallpaper every attest"; per device, each screen its own size; a process of its own, since the display's heap cannot hold the document).
//!
//! Given the wallpaper seed (its input, 32 bytes: ahau_core::wallpaper_seed, derived one way from the identity seed), it asks the display for the screen's size. If the person's space already holds a wallpaper made from this seed at this size (`wallpaper.made.key`), it ends.
//! Otherwise it draws the `wallpaper` crate's picture on the GPU, and only there (GPU-KISTS.md plan step 0; owner, 2026-11-11: "We should be using GPU path for the brot in all cases and never 'falling back' to the CPU", "Black until we bring the GPU up. I did say no fallbacks"): the crate's own code recorded through valhall (`wallpaper::gpu`), compiled here, run by the GPU service in wallgen's region of the layer arena.
//! - The survey: each candidate the seed offers drawn at a sixteenth of the screen's size (one compile and one run each), its counts and distances read back and tallied here (`wallpaper::tally`), until one passes, as the CPU's survey chooses.
//! - The first pass: one compile, run in bands of `BAND_PX` pixels (each well inside the service's one-second guard), one sample a pixel.
//! - The marks: the pixels whose colour differs from a neighbour's, from the first pass where the GPU left it (`wallpaper::mark_rows`, integer compares on the CPU), listed.
//! - The refine: one compile, run over the list `CHUNK` pixels at a time, each pixel the mean of its 4 × 4 samples, in place.
//! No GPU service (after a jump), no arena or a screen too big for wallgen's region: nothing is drawn or kept, and it says so; the display shows no wallpaper.
//! It packs the picture as a VSF visual (one plane, VSF RGB, gamma2tanh at 16 bits, as tools/wallpaper writes a person's own), stores it as `wallpaper.made` in pieces and then its key, and asks the display to show it (op 44).
//!
//! The heap is 120 MiB, inside the 128 MiB arc the kernel gives it either way (arcs are powers of two): the frame is the arena's now, so the heap holds the compiles (the refine's graph and words, a few MB, gone after), the marks and their list, then the samples (24 MB) and the document: writing it holds four copies of the samples at its peak (the packed tensor, the section the builder keeps, the bytes written, and the provenance hash's zeroed copy). 60 MiB failed at the write, and 96 MiB on the Pro the night of 2026-11-07 (the builder's output grew by doubling, 48 MB at its last step; vsf now sizes it once).
//!
//! Grant slots: 0 console, 1 the vault scope `map.`, 2 use display, 3 use gpu, 4 the layer arena. Input: the wallpaper seed.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use ferros_sys::{call, cap, console, freq, now, ERR};

const HEAP_BYTES: usize = 120 << 20;

#[repr(C, align(16))]
struct Region<const N: usize>(UnsafeCell<[u8; N]>);
unsafe impl<const N: usize> Sync for Region<N> {}
static HEAP: Region<HEAP_BYTES> = Region(UnsafeCell::new([0; HEAP_BYTES]));

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
const S_GPU: u64 = 3;
const S_LAYERS: u64 = 4;
static mut GRANTS: u64 = 0;

/// The GPU's view of the layer arena (payloads/gpu ARENA_VA), and wallgen's region of it: the first 32 MiB, which gputest's bench also uses (never at once: gputest is a development tool the bridge spawns); ahau's is above (payloads/ahau GPU_CODE, 32 MiB on).
const ARENA_VA: u64 = 0x8000_0000;
const REGION: usize = 32 << 20;
/// The frame at the region's start (working values, `wallpaper::pack`'s, w × h of them, then room for the 15 a last workgroup writes past them); the survey's counts and distances there too, before the frame is drawn.
const FRAME: usize = 0;
/// Pixels a first-pass run draws: 2^20, a quarter of the Pro's screen, two orbits each.
const BAND_PX: usize = 1 << 20;
/// Pixels a refine run takes from the list: 2^17, each sixteen orbits and an orbit for the far layer.
const CHUNK: usize = 1 << 17;
/// The most words a kernel may have: the service's code region (payloads/gpu CODE_PAGES, 32 pages) less its 16 words of padding.
const CODE_WORDS: usize = (32 << 12) / 8 - 16;

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

fn ms(t: u64) -> u64 {
    (now() - t) * 1000 / freq()
}

/// The GPU's side of wallgen: the service, the arena as wallgen sees it, and where in it the kernel's words go.
struct Gpu {
    cap: u64,
    arena: usize,
    code: usize,
    /// The GPU's own time over every run so far (µs, as the service measures each).
    us: u64,
    runs: u32,
}

impl Gpu {
    /// One kernel run by the service: its words into the code area, `groups` workgroups of 16, its uniforms `addrs` then its constants. Ends wallgen, said, when the service does not run it (none receiving: after a jump; or a job that did not finish): there is no other path (owner, 2026-11-11: "Black until we bring the GPU up").
    fn run(&mut self, what: &str, k: &wallpaper::gpu::Compiled, addrs: &[u64], groups: u32) {
        if k.words.len() > CODE_WORDS {
            say!("wallgen: the {} kernel is {} words, more than the GPU service's {}; no wallpaper", what, k.words.len(), CODE_WORDS);
            ferros_sys::exit(1);
        }
        // SAFETY: the code area lies inside wallgen's region of the arena (checked against REGION when it was placed), which no other process writes while wallgen runs; the service reads it only during the call below.
        let code = unsafe { core::slice::from_raw_parts_mut((self.arena + self.code) as *mut u64, k.words.len()) };
        code.copy_from_slice(&k.words);
        // [1][preload u16, bit 15: 64 registers][code offset u32][words u16][groups x u32][groups y u32][n u8][n × u64], as payloads/gpu takes a run.
        let mut msg = [0u8; 18 + 8 * 16];
        msg[0] = 1;
        msg[1..3].copy_from_slice(&(0x1000u16 | if k.regs64 { 0x8000 } else { 0 }).to_le_bytes());
        msg[3..7].copy_from_slice(&(self.code as u32).to_le_bytes());
        msg[7..9].copy_from_slice(&(k.words.len() as u16).to_le_bytes());
        msg[9..13].copy_from_slice(&groups.to_le_bytes());
        msg[13..17].copy_from_slice(&1u32.to_le_bytes());
        let nu = addrs.len() + k.consts.len();
        msg[17] = nu as u8;
        for (i, a) in addrs.iter().chain(k.consts.iter()).enumerate() {
            msg[18 + 8 * i..26 + 8 * i].copy_from_slice(&a.to_le_bytes());
        }
        let mut reply = [1u8, 0, 0, 0, 0];
        let n = ferros_sys::call_live(self.cap, &msg[..18 + 8 * nu], &mut reply);
        if n == ERR {
            say!("wallgen: no GPU service receiving (after a jump, or not started); no wallpaper until the GPU is up");
            ferros_sys::exit(1);
        }
        if reply[0] != 0 {
            say!("wallgen: the GPU service answered {} to the {} run; no wallpaper", reply[0], what);
            ferros_sys::exit(1);
        }
        self.us += u32::from_le_bytes([reply[1], reply[2], reply[3], reply[4]]) as u64;
        self.runs += 1;
    }
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
    let arena = ferros_sys::dma(cap(g, S_LAYERS)) as usize;
    if arena == 0 || arena == usize::MAX {
        say!("wallgen: no layer arena granted; no wallpaper");
        ferros_sys::exit(1);
    }
    // The region: the frame and the 15 pixels a last workgroup writes past it, the list, then the code. WHY the check: the screen's size comes from the display, and a frame past the region would run into ahau's part of the arena.
    let n = w * h;
    let list = (FRAME + 8 * (n + 16)).next_multiple_of(1 << 12);
    let code = list + 4 * CHUNK;
    if code + 8 * (CODE_WORDS + 16) > REGION {
        say!("wallgen: a {}x{} frame does not fit wallgen's {} MiB of the layer arena; no wallpaper", w, h, REGION >> 20);
        ferros_sys::exit(1);
    }
    let mut gpu = Gpu { cap: cap(g, S_GPU), arena, code, us: 0, runs: 0 };
    let t_all = now();
    // The survey, each candidate's preview on the GPU and tallied here.
    let t = now();
    let (mut compile_us, mut attempts) = (0u64, 0u32);
    let p = wallpaper::params_by(
        &seed,
        w,
        h,
        &wallpaper::Knobs::DEFAULT,
        |c| {
            let pv = wallpaper::preview(c);
            let count = pv.width * pv.height;
            let tc = now();
            let k = wallpaper::gpu::survey_kernel(c);
            compile_us += ferros_sys::us_since(tc);
            gpu.run("survey", &k, &[ARENA_VA + FRAME as u64], count.div_ceil(16) as u32);
            attempts += 1;
            // SAFETY: the run wrote count entries of two f32 from FRAME (and up to 15 more), inside the region; the GPU's writes reach the CPU's caches (the arena is mapped coherent on the GPU's side).
            let nd = unsafe { core::slice::from_raw_parts((arena + FRAME) as *const [f32; 2], count) };
            wallpaper::tally(c, &wallpaper::Knobs::DEFAULT, |emit| {
                for (i, v) in nd.iter().enumerate() {
                    emit(i % pv.width, i / pv.width, v[0], v[1]);
                }
            })
        },
        |_| {},
    );
    let (t_survey, survey_gpu) = (ms(t), gpu.us);
    // The first pass, band by band.
    let t = now();
    let tc = now();
    let first = wallpaper::gpu::render_kernel(&p);
    let first_us = ferros_sys::us_since(tc);
    for b in (0..n).step_by(BAND_PX) {
        let len = BAND_PX.min(n - b);
        gpu.run("first pass", &first, &[ARENA_VA + FRAME as u64, b as u64], len.div_ceil(16) as u32);
    }
    let first_words = first.words.len();
    drop(first);
    let (t_draw, draw_gpu) = (ms(t), gpu.us - survey_gpu);
    // The marks and their list.
    let t = now();
    let stride = wallpaper::mask_stride(w);
    let mut mask: Vec<u8> = alloc::vec![0; stride * h];
    {
        // SAFETY: the first pass wrote every pixel of the frame, inside the region; read through the CPU's caches, coherent with the GPU's writes. The slice ends here, before the refine's runs write the frame again.
        let frame = unsafe { core::slice::from_raw_parts((arena + FRAME) as *const u64, n) };
        wallpaper::mark_rows(&p, frame, 0..h, &mut mask);
    }
    let mut marked: Vec<u32> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if mask[y * stride + x / 8] >> (x % 8) & 1 == 1 {
                marked.push((y * w + x) as u32);
            }
        }
    }
    drop(mask);
    let t_mark = ms(t);
    // The refine, a chunk of the list at a time: each chunk into the list area, padded to whole workgroups with its last index (drawn twice, the same value).
    let t = now();
    let tc = now();
    let fine = wallpaper::gpu::refine_kernel(&p);
    let fine_us = ferros_sys::us_since(tc);
    for part in marked.chunks(CHUNK) {
        let padded = part.len().next_multiple_of(16);
        // SAFETY: the list area holds CHUNK u32s, and a padded chunk is at most CHUNK (a multiple of 16).
        let slots = unsafe { core::slice::from_raw_parts_mut((arena + list) as *mut u32, padded) };
        slots[..part.len()].copy_from_slice(part);
        let last = part[part.len() - 1];
        slots[part.len()..].fill(last);
        gpu.run("refine", &fine, &[ARENA_VA + FRAME as u64, ARENA_VA + list as u64], (padded / 16) as u32);
    }
    let fine_words = fine.words.len();
    drop(fine);
    let (t_refine, refine_gpu) = (ms(t), gpu.us - survey_gpu - draw_gpu);
    say!("wallgen: drawn on the GPU in {} ms: survey {} candidates {} ms (GPU {} us, compiles {} us); first pass {} ms (GPU {} us, {} words compiled in {} us); marks {} ms, {} of {} pixels; refined {} ms (GPU {} us, {} words compiled in {} us); {} runs", ms(t_all), attempts, t_survey, survey_gpu, compile_us, t_draw, draw_gpu, first_words, first_us, t_mark, marked.len(), n, t_refine, refine_gpu, fine_words, fine_us, gpu.runs);
    drop(marked);
    // The working values (gamma-2 codes, white at 16384) as gamma2tanh samples of their light, one plane a channel.
    let t = now();
    // SAFETY: the refine's last run has finished (each run is held until done); the frame is whole.
    let frame = unsafe { core::slice::from_raw_parts((arena + FRAME) as *const u64, n) };
    let mut samples: Vec<u16> = alloc::vec![0; 3 * n];
    for (i, &v) in frame.iter().enumerate() {
        for c in 0..3 {
            let code = ((v >> (16 * c)) & 0xFFFF) as f32 * (1.0 / 16384.0);
            samples[c * n + i] = vsf::colour::convert::delinearize_gamma2tanh_bits_f32(code * code, 16) as u16;
        }
    }
    let tensor = vsf::types::BitPackedTensor::pack(16, alloc::vec![3, h, w], &samples);
    drop(samples);
    let mut plane = vsf::visual::Plane::encoded("display", "wallpaper", w, h, vsf::visual::Primaries::Vsf, vsf::visual::Encoding::Gamma2Tanh, tensor);
    plane.name = "main".into();
    let v = vsf::visual::Visual { title: "wallpaper".into(), text: "The person's wallpaper, made on this device from their wallpaper seed (wallgen, on the GPU).".into(), planes: alloc::vec![plane], ..Default::default() };
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
    say!("wallgen: made {}x{} in {} ms (packed {} ms, stored {} B in {} ms)", w, h, ms(t_all), t_pack, bytes.len(), t_store);
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
