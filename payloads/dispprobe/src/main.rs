//! Display-state probe (READ ONLY) — what did ABL leave the Pixel 8 DPU in?
//!
//! Decides feasibility of "reuse ABL's display for boot breadcrumbs" vs "full DECON bring-up".
//! Zuma DPU = cal_9865 (from the google display module). MMIO reads of DECON/DPP are known-safe (pixel8_boot_findings); no writes.
//!
//! Reads:
//! - DECON0 GLOBAL_CON: is DECON enabled, running, and in command vs video mode?
//! - DECON0 FRAME_COUNT twice (with a delay): advancing = actively scanning out; static = stopped after the splash (command-mode self-refresh).
//! - DPP0 RDMA_BASEADDR: the framebuffer IOVA DECON is reading from (nonzero = a real surface is configured).
//!
//! Interpretation: DECON enabled + cmd mode + static frame count + valid FB base = ABL set it up and is holding one frame; breadcrumbs need FB-physical-addr + a re-trigger. DECON disabled = ABL tore it down; full bring-up required.

#![no_std]
#![no_main]

const DECON0: usize = 0x1947_0000;
const DPP0_DMA: usize = 0x1990_0000;

// DECON regs (cal_9865/regs-decon.h)
const DECON_VERSION: usize = 0x0000;
const FRAME_COUNT: usize = 0x0004;
const GLOBAL_CON: usize = 0x0020;
// DPP DMA regs (cal_9865/regs-dpp.h)
const RDMA_BASEADDR_P0: usize = 0x0040;
const RDMA_BASEADDR_P1: usize = 0x0044;

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
    fn s(&mut self, s: &[u8]) {
        for &b in s { self.put(b); }
    }
    fn hex(&mut self, v: u32) {
        let h = b"0123456789ABCDEF";
        for i in (0..8).rev() {
            self.put(h[((v >> (i * 4)) & 0xF) as usize]);
        }
    }
    fn line(&mut self, label: &[u8], v: u32) {
        self.s(label);
        self.put(b'=');
        self.hex(v);
        self.put(b'\n');
    }
}

fn rd(base: usize, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}

/// Entry — pinned at blob offset 0 by payload.ld.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_inp: *const u8, _in_len: usize, out: *mut u8, out_cap: usize) -> u64 {
    let mut o = Out { ptr: out, cap: out_cap, n: 0 };

    o.line(b"DECON_VERSION", rd(DECON0, DECON_VERSION));
    let gc = rd(DECON0, GLOBAL_CON);
    o.line(b"GLOBAL_CON", gc);
    o.s(b"  DECON_EN=");
    o.hex((gc >> 1) & 1);
    o.s(b" RUN=");
    o.hex((gc >> 4) & 1);
    o.s(b" IDLE=");
    o.hex((gc >> 5) & 1);
    o.s(b" CMD_MODE=");
    o.hex((gc >> 8) & 1);
    o.put(b'\n');

    let f0 = rd(DECON0, FRAME_COUNT);
    for _ in 0..2_000_000 { core::hint::spin_loop(); }
    let f1 = rd(DECON0, FRAME_COUNT);
    o.line(b"FRAME_COUNT_0", f0);
    o.line(b"FRAME_COUNT_1", f1);
    o.s(if f0 == f1 { b"  -> STATIC (holding one frame)\n" } else { b"  -> ADVANCING (active scanout)\n" });

    o.line(b"DPP0_RDMA_BASE_P0", rd(DPP0_DMA, RDMA_BASEADDR_P0));
    o.line(b"DPP0_RDMA_BASE_P1", rd(DPP0_DMA, RDMA_BASEADDR_P1));

    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
