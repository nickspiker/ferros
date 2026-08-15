//! Minimal ferros RUN payload — the template for hot-loaded programs.
//!
//! Build:
//!   cargo build -p ferros-payload-hello --target aarch64-unknown-none --release
//!   aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none/release/hello hello.bin
//! Run:
//!   ferros-bridge run hello.bin [hex-input]
//!
//! ABI (see ferros_pt::command::caps::RUN): the kernel CALLS blob offset 0 as `extern "C" fn(in_ptr, in_len, out_ptr, out_cap) -> u64` and ships back [ret][out bytes]. Code must be PC-relative — no absolute addresses, no std, no heap. The kernel's staging slot is execute-proven; you are running at either G#8008_0000+32M or G#8208_0000+... wherever the free slot was.

#![no_std]
#![no_main]

/// Entry — pinned at blob offset 0 by payload.ld.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(inp: *const u8, in_len: usize, out: *mut u8, out_cap: usize) -> u64 {
    let mut n = 0usize;
    let mut put = |b: u8| {
        if n < out_cap {
            unsafe { *out.add(n) = b; }
            n += 1;
        }
    };

    for &b in b"hello from a hot-loaded payload @ G#" {
        put(b);
    }

    // Report our own PC — live proof of where this code executes.
    let pc: usize;
    unsafe { core::arch::asm!("adr {}, .", out(reg) pc); }
    let hex = b"0123456789ABCDEF";
    for i in (0..16).rev() {
        put(hex[(pc >> (i * 4)) & 0xF]);
    }
    put(b'\n');

    // Echo any input the host sent with Exec.
    if in_len > 0 {
        for &b in b"input: " {
            put(b);
        }
        for i in 0..in_len {
            put(unsafe { *inp.add(i) });
        }
        put(b'\n');
    }

    n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
