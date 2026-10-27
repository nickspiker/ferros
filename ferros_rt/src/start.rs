//! The image's entry and its header, the process's grants and input, exit.
//!
//! The kernel enters offset 0 at EL0 with x0 = the grants page's offset, x1 = the input's offset, x2 = its length, x28 = the arc base, x27 = its mask, SP at the arc's top (KERNEL.md, process ABI). `_entry` branches to `__ferros_start`, which relocates the image, builds the heap from the zeroed tail and the arc's slack, finds the entropy grant, gives the main thread its TLS block and a stack from the heap, and calls the C `main` rustc generated — `std::rt::lang_start` — whose return is the exit code.
//!
//! The header at offset 8 is what the kernel reads before the process runs (process.rs `image_tail`) and what the runtime reads after: `ferrosRT`, then five words — the zeroed tail (.bss + 16 + heap), the TLS image size, the TLS block size, the TLS alignment, the heap size — every one an absolute number the linker script computed (ferros.ld).

use core::alloc::Layout;
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicU64, Ordering};

use crate::sys::{self, nr};

global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {start}
    .4byte 0
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad __ferros_tdata_size
    .quad __ferros_tls_size
    .quad __ferros_tls_align
    .quad __ferros_heap
    .size _entry, . - _entry

    .section .tdata, "awT", %progbits
    .balign 8
__ferros_tdata_mark:
    .quad 1

    .section .tbss, "awT", %nobits
    .balign 8
__ferros_tbss_mark:
    .zero 8
    "#,
    start = sym __ferros_start,
);

/// The main thread's stack, from the heap (the kernel's entry stack is 64 KiB: enough to build the heap, not for an app).
pub const MAIN_STACK: usize = 1024 * 1024;

static GRANTS: AtomicU64 = AtomicU64::new(0);
static INPUT_OFF: AtomicU64 = AtomicU64::new(0);
static INPUT_LEN: AtomicU64 = AtomicU64::new(0);

/// The header's five words.
#[derive(Clone, Copy)]
pub struct Header {
    pub tail: usize,
    pub tdata_size: usize,
    pub tls_size: usize,
    pub tls_align: usize,
    pub heap: usize,
}

pub fn header() -> Header {
    let p = (sys::image_base() + 16) as *const u64;
    let w = |i: usize| unsafe { p.add(i).read() as usize };
    Header { tail: w(0), tdata_size: w(1), tls_size: w(2), tls_align: w(3), heap: w(4) }
}

/// The grants page's arc offset.
pub fn grants() -> u64 {
    GRANTS.load(Ordering::Relaxed)
}

/// How many grants the process holds (the page's first word).
pub fn grant_count() -> u64 {
    unsafe { ((sys::arc_base() + grants()) as *const u32).read_volatile() as u64 }
}

/// The arc offset of grant `slot`'s 32-byte token (slot 0 is the console).
#[inline]
pub fn cap(slot: u64) -> u64 {
    grants() + 4 + 32 * slot
}

/// The spawn input, in the arc for the life of the process.
pub fn input() -> &'static [u8] {
    let (o, n) = (INPUT_OFF.load(Ordering::Relaxed), INPUT_LEN.load(Ordering::Relaxed) as usize);
    unsafe { core::slice::from_raw_parts((sys::arc_base() + o) as *const u8, n) }
}

/// End the process; the kernel prints `name: exit G#<code>`.
pub fn exit(code: i32) -> ! {
    sys::svc(nr::EXIT, code as u32 as u64, 0, 0, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

unsafe extern "C" {
    static __bss_end: u8;
    fn main(argc: i32, argv: *const *const u8) -> i32;
}

#[unsafe(no_mangle)]
unsafe extern "C" fn __ferros_start(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { sys::relocate() };
    GRANTS.store(grants, Ordering::Relaxed);
    INPUT_OFF.store(input, Ordering::Relaxed);
    INPUT_LEN.store(input_len, Ordering::Relaxed);
    let base = sys::arc_base() as usize;
    // The heap: the zeroed tail after .bss, up to the input (the kernel put the input at the tail's end), then the arc's slack past the input, up to the entry stack.
    let a0 = (&raw const __bss_end) as usize;
    let a1 = base + input as usize;
    if a1 > a0 {
        unsafe { crate::alloc::add_region(a0, a1 - a0) };
    }
    let b0 = base + (input + input_len) as usize;
    let b1 = base + sys::arc_mask() as usize + 1 - crate::thread::ENTRY_STACK;
    if b1 > b0 {
        unsafe { crate::alloc::add_region(b0, b1 - b0) };
    }
    crate::random::detect(grant_count());
    let Some((tls, _)) = crate::thread::tls::new_block() else { exit(0xDEAD) };
    unsafe { crate::thread::tls::install(tls) };
    let stack = unsafe { crate::alloc::alloc(Layout::from_size_align_unchecked(MAIN_STACK, 16)) };
    if stack.is_null() {
        exit(0xDEAD);
    }
    let top = stack as usize + MAIN_STACK;
    unsafe { asm!("mov sp, {top}", "b {main}", top = in(reg) top, main = sym rt_main, options(noreturn, nostack)) }
}

extern "C" fn rt_main() -> ! {
    let code = unsafe { main(0, core::ptr::null()) };
    exit(code)
}
