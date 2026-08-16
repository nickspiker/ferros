//! READ-ONLY PMU reboot-register probe.
//!
//! An earlier blind WRITE of G#8000_00FC to G#1546_0810 (the DT reboot-cmd-offset) left USB dead across warm resets — a cold power cycle recovered it. Before any write is trusted again we need to know what actually lives at that offset. This payload only READS: a window of PMU registers around G#810, so we can tell a dead scratch cell (reads 0, safe to stamp) from a live control register (nonzero, reserved fields — do NOT poke).
//!
//! No writes. Safe to run repeatedly. Exynos PMU base from the live device tree (root): G#1546_0000 (samsung,exynos-pmu), reg size G#0001_0000.

#![no_std]
#![no_main]

const PMU: usize = 0x1546_0000;

struct Out {
    ptr: *mut u8,
    cap: usize,
    n: usize,
}

impl Out {
    fn put(&mut self, b: u8) {
        if self.n < self.cap {
            unsafe { *self.ptr.add(self.n) = b; }
            self.n += 1;
        }
    }
    fn hex(&mut self, val: u32) {
        let h = b"0123456789ABCDEF";
        for i in (0..8).rev() {
            self.put(h[((val >> (i * 4)) & 0xF) as usize]);
        }
    }
    fn line(&mut self, off: u32, val: u32) {
        for &b in b"PMU+" {
            self.put(b);
        }
        self.hex(off);
        self.put(b'=');
        self.hex(val);
        self.put(b'\n');
    }
}

fn rd(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Entry — pinned at blob offset 0 by payload.ld.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_inp: *const u8, _in_len: usize, out: *mut u8, out_cap: usize) -> u64 {
    let mut o = Out { ptr: out, cap: out_cap, n: 0 };

    // Window around the DT reboot-cmd-offset (G#810): 16 words each side, 4-byte stride.
    // A scratch reason-register reads 0 (or a stale reason); a live control block shows structured nonzero values across the window.
    let mut off = 0x800u32;
    while off <= 0x84C {
        o.line(off, rd(PMU + off as usize));
        off += 4;
    }

    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
