//! RUN payload: find a cluster-watchdog programming that resets the SoC on demand (for `reboot_to_bootloader`).
//!
//! The service loop pets the watchdogs every iteration, so this has to run as a payload that owns the CPU. Input byte 0 picks the variant; if no reset arrives within 5 s the payload returns readbacks of both watchdogs, and the next boot's reason (`watchdog,apc` vs. nothing) says whether a variant fired.
//!
//!   0 = the zuma s3c2410 driver's restart sequence (WTCON=0, WTMINCNT=G#100, WTCNT=WTDAT=G#80, WTCON=EN|RSTEN|DIV16|PRESCALE(G#20)|WINDOWED), back to back
//!   1 = the same with 1 ms between register writes
//!   2 = WTCON=0, WTDAT=1, WTCNT=1, WTCON=EN|RSTEN (no prescale, DIV16) with 1 ms between writes
//!   3 = keep the running configuration and only write WTCNT=1
//!
//! Build: cargo build -p ferros-payload-wdtfire --target aarch64-unknown-none --release && aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none/release/wdtfire wdtfire.bin
//! Run:   ferros-bridge run wdtfire.bin 01

#![no_std]
#![no_main]

const WATCHDOGS: [usize; 2] = [0x1006_0000, 0x1007_0000];
const WTCON: usize = 0x00;
const WTDAT: usize = 0x04;
const WTCNT: usize = 0x08;
const WTMINCNT: usize = 0x10;

fn rd(a: usize) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe {
        core::ptr::write_volatile(a as *mut u32, v);
        core::arch::asm!("dsb sy");
    }
}
fn ticks() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("isb", "mrs {}, cntpct_el0", out(reg) v) };
    v
}
fn freq() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) v) };
    if v == 0 { 24_576_000 } else { v }
}
fn delay_ms(ms: u64) {
    let t0 = ticks();
    let n = freq() / 1000 * ms;
    while ticks().wrapping_sub(t0) < n {
        core::hint::spin_loop();
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(inp: *const u8, in_len: usize, out: *mut u8, out_cap: usize) -> u64 {
    let variant = if in_len > 0 { unsafe { *inp } } else { 0 };
    let gap = |ms: u64| if variant == 0 { () } else { delay_ms(ms) };
    for &w in &WATCHDOGS {
        match variant {
            0 | 1 => {
                wr(w + WTCON, 0);
                gap(1);
                wr(w + WTMINCNT, 0x100);
                gap(1);
                wr(w + WTCNT, 0x80);
                gap(1);
                wr(w + WTDAT, 0x80);
                gap(1);
                wr(w + WTCON, (1 << 5) | (1 << 0) | (0x20 << 8) | (1 << 20));
            }
            2 => {
                wr(w + WTCON, 0);
                delay_ms(1);
                wr(w + WTDAT, 1);
                delay_ms(1);
                wr(w + WTCNT, 1);
                delay_ms(1);
                wr(w + WTCON, (1 << 5) | (1 << 0));
            }
            _ => wr(w + WTCNT, 1),
        }
    }
    // Snapshot right after programming, then wait for the reset.
    let mut snap = [0u32; 10];
    for (i, &w) in WATCHDOGS.iter().enumerate() {
        snap[i * 5] = rd(w + WTCON);
        snap[i * 5 + 1] = rd(w + WTDAT);
        snap[i * 5 + 2] = rd(w + WTCNT);
        snap[i * 5 + 3] = rd(w + WTMINCNT);
    }
    delay_ms(5000);
    // Still here: report the snapshot and the counters now (index 4/9 = WTCNT after 5 s).
    snap[4] = rd(WATCHDOGS[0] + WTCNT);
    snap[9] = rd(WATCHDOGS[1] + WTCNT);
    let mut n = 0usize;
    for v in snap {
        for b in v.to_le_bytes() {
            if n < out_cap {
                unsafe { *out.add(n) = b };
                n += 1;
            }
        }
    }
    n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
