//! Does a process's floating point survive the kernel? Three checks, each printed: (1) runtime f32 arithmetic that LLVM cannot fold (the inputs come from the clock), (2) the hardware `fsqrt`, (3) a long run of f32 work spanning many time slices and syscalls, counting every result that differs from the first — a kernel that doesn't save the vector registers across a trap shows up here as corruption. Spawn: ferros-bridge spawn fptest.bin 00 --name fptest
#![no_std]
#![no_main]

use ferros_sys::{console, now};

struct Out([u8; 512], usize);
impl Out {
    fn s(&mut self, t: &[u8]) {
        for &b in t {
            if self.1 < self.0.len() {
                self.0[self.1] = b;
                self.1 += 1;
            }
        }
    }
    fn dec(&mut self, mut v: u64) {
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let t = d;
        self.s(&t[i..]);
    }
}

#[inline(never)]
fn newton_sqrt(x: f32) -> f32 {
    let mut r = if x > 1.0 { x } else { 1.0 };
    for _ in 0..24 {
        r = 0.5 * (r + x / r);
    }
    r
}

#[inline(never)]
fn fsqrt(x: f32) -> f32 {
    let r: f32;
    unsafe { core::arch::asm!("fsqrt {0:s}, {1:s}", out(vreg) r, in(vreg) x, options(pure, nomem, nostack)) };
    r
}

#[inline(never)]
fn fpcr() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, fpcr", out(reg) v, options(nomem, nostack)) };
    v
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut o = Out([0; 512], 0);
    // (1) Runtime values the compiler cannot know: 4.0 built from the clock's low bit.
    let four = 4.0f32 + (now() & 1) as f32 * 0.0;
    o.s(b"fptest: fpcr ");
    o.dec(fpcr());
    o.s(b" newton sqrt(4)*1000 = ");
    o.dec((newton_sqrt(four) * 1000.0) as u64);
    o.s(b", fsqrt(4)*1000 = ");
    o.dec((fsqrt(four) * 1000.0) as u64);
    o.s(b", 4*0.75*1000 = ");
    o.dec((four * 0.75 * 1000.0) as u64);
    o.s(b", (u64) 2000 as f32 /3 *1000 = ");
    o.dec(((2000u64 + (now() & 1)) as f32 / 3.0 * 1000.0) as u64);
    o.s(b"\n");
    console(grants, &o.0[..o.1]);
    o.1 = 0;
    // (3) Many slices: 64 M iterations of dependent f32 work, a syscall every million, the result compared each round.
    let mut bad = 0u64;
    let mut rounds = 0u64;
    let mut first = 0.0f32;
    let t0 = now();
    while now().wrapping_sub(t0) < ferros_sys::freq() * 4 {
        let mut acc = 1.0f32 + (now() & 1) as f32 * 0.0;
        for i in 0..1_000_000u32 {
            acc = acc * 1.000001 + 0.000001 * (i & 7) as f32;
            if acc > 1000.0 {
                acc -= 999.0;
            }
        }
        if rounds == 0 {
            first = acc;
        } else if acc != first {
            bad += 1;
        }
        rounds += 1;
        console(grants, b"");
    }
    o.s(b"fptest: ");
    o.dec(rounds);
    o.s(b" rounds across slices and syscalls, ");
    o.dec(bad);
    o.s(b" differed from the first; result*1000 = ");
    o.dec((first * 1000.0) as u64);
    o.s(b"\n");
    console(grants, &o.0[..o.1]);
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
