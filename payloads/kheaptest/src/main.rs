//! kheaptest: the kernel heap's window on the device (docs/NO-CEILINGS.md step 4), driven by calls whose messages the kernel holds in its heap until a server takes them.
//! Grant slot 1 is an endpoint nobody serves (`ferros-bridge spawn kheaptest.bin [mode] --name kheaptest --use kfill`), so every call waits there with its message in the kernel's heap.
//! Mode 00 (the default), grow and trim: one call of 64 MiB, held 8 s (the window grows past the fixed regions: `diag heap`), then the process ends and the message goes, and the service loop gives the window's top back.
//! Mode 01, the fill: calls of 128 MiB, each on its own thread, until one is answered ERR (the kernel heap could not hold it: "kernel out of memory" on the console, never a reset); then held 8 s and the process ends.
//! Every step is a console line.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering::SeqCst};
use ferros_sys::{cap, console, freq, now, ERR};

/// The heap's ceiling in the image header: what the device holds (ferros_sys::HEAP_DEVICE).
const HEAP_CEILING: usize = ferros_sys::HEAP_DEVICE;

core::arch::global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 1
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad {heap}
    .size _entry, . - _entry
"#,
    main = sym test_main,
    heap = const HEAP_CEILING
);

unsafe extern "C" {
    static __bss_end: u8;
}

#[global_allocator]
static ALLOC: ferros_heap::ArcHeap<ferros_heap::Kernel> = ferros_heap::ArcHeap::new(ferros_heap::Kernel);

const S_FILL: u64 = 1;
const MIB: usize = 1 << 20;
/// The grow mode's one message: 64 MiB, past both fixed regions (22 MiB).
const GROW: usize = 64 * MIB;
/// The fill mode's message a call: 128 MiB.
const PIECE: usize = 128 * MIB;
/// A waiting thread's stack.
const STACK: usize = 1 << 14;
static mut GRANTS: u64 = 0;
/// The message every call sends (one buffer, read by every thread), its address and length.
static MSG: AtomicU64 = AtomicU64::new(0);
static MSG_LEN: AtomicU64 = AtomicU64::new(0);
/// Calls answered (each one ERR or a reply), and of those answered ERR.
static ANSWERED: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);

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

/// One call on the unserved endpoint with the shared message; it waits there until the process ends, or is answered ERR at once when the kernel cannot hold the message.
extern "C" fn caller(_: u64) -> ! {
    let msg = unsafe { core::slice::from_raw_parts(MSG.load(SeqCst) as *const u8, MSG_LEN.load(SeqCst) as usize) };
    let mut r = [0u8; 8];
    let n = ferros_sys::call(cap(unsafe { GRANTS }, S_FILL), msg, &mut r);
    if n == ERR {
        REFUSED.fetch_add(1, SeqCst);
    }
    ANSWERED.fetch_add(1, SeqCst);
    ferros_sys::thread_exit(0)
}

/// A thread making one call; false when the thread could not be started.
fn call_on_thread() -> bool {
    let stack: &'static mut [u8] = Vec::leak(alloc::vec![0u8; STACK]);
    ferros_sys::thread_spawn(caller, stack, 0) != ERR
}

fn sleep_ms(ms: u64) {
    ferros_sys::sleep_until(now() + ms * freq() / 1000);
}

#[unsafe(no_mangle)]
pub extern "C" fn test_main(g: u64, input: u64, input_len: u64) -> ! {
    let mode = if input_len > 0 { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } } else { 0 };
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init((&raw const __bss_end) as usize, input as usize, input_len as usize) };
    unsafe { GRANTS = g };
    let len = if mode == 1 { PIECE } else { GROW };
    let mut msg: Vec<u8> = Vec::new();
    if msg.try_reserve_exact(len).is_err() {
        say!("kheaptest: no room for the A#{len}-byte message in this process");
        ferros_sys::exit(1);
    }
    msg.resize(len, 0x5A);
    MSG.store(msg.as_ptr() as u64, SeqCst);
    MSG_LEN.store(len as u64, SeqCst);
    if mode != 1 {
        let t = now();
        call_on_thread();
        sleep_ms(1 << 10);
        say!("kheaptest grow: a call of A#{len} bytes {} after A#{} ms (diag heap now: the window holds it)", if ANSWERED.load(SeqCst) == 0 { "waits at the endpoint" } else if REFUSED.load(SeqCst) != 0 { "was refused (ERR)" } else { "was answered" }, (now() - t) * 1000 / freq());
        sleep_ms(1 << 13);
        say!("kheaptest grow: ending; the message goes with the call");
        ferros_sys::exit(0);
    }
    let mut calls = 0u64;
    loop {
        if !call_on_thread() {
            say!("kheaptest fill: no thread for call A#{calls}");
            break;
        }
        calls += 1;
        sleep_ms(1 << 8);
        if REFUSED.load(SeqCst) != 0 {
            say!("kheaptest fill: call A#{calls} refused (ERR); A#{} calls of A#{len} bytes held, A#{} MiB in the kernel's heap", calls - 1, (calls - 1) * (len as u64 >> 20));
            break;
        }
        if calls % 8 == 0 {
            say!("kheaptest fill: A#{calls} calls of A#{len} bytes held");
        }
    }
    sleep_ms(1 << 13);
    say!("kheaptest fill: ending; every held message goes with its call");
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::exit(0xDEAD)
}
