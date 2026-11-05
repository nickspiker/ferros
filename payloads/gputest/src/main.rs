//! A client of the GPU service (payloads/gpu, GPU.md "Layers"): the Mandelbrot kernel built through the valhall builder, its words put in the layer arena, run through `gpu`, and every pixel compared with the CPU's own iteration.
//! Spawn: ferros-bridge spawn gputest.bin --name gputest --use gpu --dma layers
//! Grant slots: 0 console, 1 gpu (use), 2 the layer arena.
#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, exit};
use valhall::*;

const S_GPU: u64 = 1;
const S_ARENA: u64 = 2;
/// The arena at the GPU's VA (payloads/gpu ARENA_VA): the code at offset 0, the output 1 MiB in.
const ARENA_VA: u64 = 0x8000_0000;
const OUT: usize = 1 << 20;
const SIDE_SHIFT: u32 = 6;
const PIXELS: usize = 1 << (2 * SIDE_SHIFT);
const MAX_ITER: u32 = 64;

struct Line([u8; 160], usize);
impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.1 < self.0.len() {
                self.0[self.1] = b;
                self.1 += 1;
            }
        }
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($a:tt)*) => {{
        let mut l = Line([0; 160], 0);
        let _ = core::fmt::write(&mut l, format_args!($($a)*));
        console($g, &l.0[..l.1]);
    }};
}

/// The Mandelbrot kernel (valhall/tests/kernels.rs `mandelbrot`, verified word for word): per invocation one pixel of a 2^shift-wide grid, its escape count to the output.
fn mandelbrot(k: &mut Kernel<32>) {
    k.rshift_or_i32(r(13), r(60), u(1).w0(), Byte::B0, Lut::ZERO);
    k.lshift_or_i32(r(12), r(13), u(1).w0(), Byte::B0, Lut::ZERO);
    k.isub_s32(r(12), r(60), r(12));
    k.u32_to_f32(r(12), r(12));
    k.u32_to_f32(r(13), r(13));
    k.mov_i32(r(10), u(3).w0());
    k.fma_f32(r(6), r(12), r(10), u(2).w0());
    k.fma_f32(r(7), r(13), r(10), u(2).w1());
    k.iadd_imm_i32(r(4), Lut::ZERO, 4f32.to_bits());
    k.iadd_imm_i32(r(0), Lut::ZERO, 0);
    k.iadd_imm_i32(r(1), Lut::ZERO, 0);
    k.iadd_imm_i32(r(5), Lut::ZERO, 0).reconverge();
    let top = k.here();
    k.fma_f32(r(8), r(1), r(1), Lut::ZERO.neg());
    k.fma_f32(r(9), r(5), r(5), Lut::ZERO.neg());
    k.fadd_f32(r(10), r(8), r(9));
    k.icmp_or_u32(r(11), Cond::Ne, CmpResult::M1, r(0), u(1).w1(), Lut::ZERO);
    k.fcmp_or_f32(r(12), Cond::Le, CmpResult::M1, r(10), r(4), Lut::ZERO);
    k.lshift_and_i32(r(11), r(11), Lut::ZERO, Byte::B0, r(12));
    k.fadd_f32(r(10), r(1), r(1));
    k.fma_f32(r(5), r(10), r(5), r(7));
    k.fadd_f32(r(10), r(8), r(9).neg());
    k.fadd_f32(r(1), r(10), r(6));
    k.isub_s32(r(0), r(0), r(11));
    k.branchz(r(11), top).reconverge();
    k.shaddx_u64(pair(2), u(0), pair(60), 2);
    k.store_i32(r(0), pair(2), 0, Slot::S0).end();
}

/// The same pixel on the CPU, in the kernel's order of operations.
fn reference(gid: u32, x0: f32, y0: f32, step: f32) -> u32 {
    let py = gid >> SIDE_SHIFT;
    let px = gid - (py << SIDE_SHIFT);
    let (cr, ci) = (px as f32 * step + x0, py as f32 * step + y0);
    let (mut zx, mut zy, mut k) = (0f32, 0f32, 0u32);
    loop {
        let (x2, y2) = (zx * zx, zy * zy);
        if k == MAX_ITER || !(x2 + y2 <= 4.0) {
            return k;
        }
        zy = (zx + zx) * zy + ci;
        zx = (x2 - y2) + cr;
        k += 1;
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let arena = ferros_sys::dma(cap(grants, S_ARENA)) as usize;
    if arena == 0 || arena == usize::MAX {
        say!(grants, "gputest: no layer arena\n");
        exit(1);
    }
    let mut k = Kernel::<32>::new();
    mandelbrot(&mut k);
    let words = k.finish();
    for (i, &w) in words.iter().enumerate() {
        unsafe { core::ptr::write_volatile((arena + 8 * i) as *mut u64, w) };
    }
    for i in 0..PIXELS {
        unsafe { core::ptr::write_volatile((arena + OUT + 4 * i) as *mut u32, u32::MAX) };
    }
    let (x0, y0, step) = (-2.0f32, -1.25f32, 0.0390625f32);
    let uniforms = [ARENA_VA + OUT as u64, SIDE_SHIFT as u64 | (MAX_ITER as u64) << 32, x0.to_bits() as u64 | (y0.to_bits() as u64) << 32, step.to_bits() as u64];
    // [1][preload u16][code offset u32][code words u16][groups x u32][groups y u32][n u8][n × u64]
    let mut msg = [0u8; 18 + 8 * 4];
    msg[0] = 1;
    msg[1..3].copy_from_slice(&0x1000u16.to_le_bytes());
    msg[3..7].copy_from_slice(&0u32.to_le_bytes());
    msg[7..9].copy_from_slice(&(words.len() as u16).to_le_bytes());
    msg[9..13].copy_from_slice(&((PIXELS / 16) as u32).to_le_bytes());
    msg[13..17].copy_from_slice(&1u32.to_le_bytes());
    msg[17] = uniforms.len() as u8;
    for (i, u) in uniforms.iter().enumerate() {
        msg[18 + 8 * i..26 + 8 * i].copy_from_slice(&u.to_le_bytes());
    }
    let mut reply = [0u8; 5];
    let t0 = ferros_sys::now();
    let n = call(cap(grants, S_GPU), &msg, &mut reply);
    let call_us = ferros_sys::us_since(t0);
    let job_us = u32::from_le_bytes([reply[1], reply[2], reply[3], reply[4]]);
    let mut wrong = 0u32;
    let mut first = None;
    for gid in 0..PIXELS as u32 {
        let got = unsafe { core::ptr::read_volatile((arena + OUT + 4 * gid as usize) as *const u32) };
        if got != reference(gid, x0, y0, step) {
            wrong += 1;
            first.get_or_insert(gid);
        }
    }
    say!(grants, "gputest: {} words; reply {n} bytes, status {}, job {job_us} us, call {call_us} us; {wrong} of {PIXELS} pixels differ from the CPU (the first at gid {})\n", words.len(), reply[0], first.map_or(-1, |g| g as i64));
    exit(if reply[0] == 0 && wrong == 0 { 0 } else { 1 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(0xDEAD);
}
