//! S2MPU write-passthrough test for the PMU reboot-reason scratch cell.
//!
//! pmuprobe showed G#1546_0810 reads 0 and sits in a clean all-zero region (0x800..0x82C) — a scratch cell, not a live control register. This decides the one thing a read can't: does a write from EL2 actually LAND, or does S2MPU silently swallow it (the way it blocked the display SYSMMU)?
//!
//! Sequence, NO reboot: read 0x810, write G#8000_00FC, read back, restore 0, read again. If the read-back shows G#8000_00FC the write path is clear and the reboot-fastboot mechanism is sound (my earlier failure was the enumeration streak, not the write). If it reads 0, S2MPU is eating PMU writes and reboot-fastboot needs an EL3/SMC path instead.
//!
//! Safe: only touches a scratch cell, restores it, never resets.

#![no_std]
#![no_main]

const REBOOT_REG: usize = 0x1546_0810;

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
    fn line(&mut self, label: &[u8], val: u32) {
        for &b in label {
            self.put(b);
        }
        self.put(b'=');
        let h = b"0123456789ABCDEF";
        for i in (0..8).rev() {
            self.put(h[((val >> (i * 4)) & 0xF) as usize]);
        }
        self.put(b'\n');
    }
}

/// Entry — pinned at blob offset 0 by payload.ld.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_inp: *const u8, _in_len: usize, out: *mut u8, out_cap: usize) -> u64 {
    let mut o = Out { ptr: out, cap: out_cap, n: 0 };
    let reg = REBOOT_REG as *mut u32;

    let before = unsafe { core::ptr::read_volatile(reg) };
    o.line(b"before", before);

    unsafe {
        core::ptr::write_volatile(reg, 0x8000_00FC);
        core::arch::asm!("dsb sy");
    }
    let after_write = unsafe { core::ptr::read_volatile(reg) };
    o.line(b"after_write", after_write);

    unsafe {
        core::ptr::write_volatile(reg, 0);
        core::arch::asm!("dsb sy");
    }
    let after_restore = unsafe { core::ptr::read_volatile(reg) };
    o.line(b"after_restore", after_restore);

    if after_write == 0x8000_00FC {
        for &b in b"VERDICT: write LANDS (S2MPU passes)\n" {
            o.put(b);
        }
    } else {
        for &b in b"VERDICT: write SWALLOWED (S2MPU blocks)\n" {
            o.put(b);
        }
    }

    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
