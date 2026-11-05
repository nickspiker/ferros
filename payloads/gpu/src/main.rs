//! The GPU driver (GPU.md): the Mali-G715's CSF microcontroller booted from Arm's firmware, its global interface answering, and command-stream jobs run on it — as a process, the kernel holding only the GPU's DMA fence (s2mpu::fence_gpu) and its power.
//!
//! Spawn: `ferros-bridge spawn gpu.bin --name gpu --dma gpu-regs --dma gpu-mem --store gpu`. Grant slots: 0 console, 1 the GPU's registers (`gpu-regs`, EL0 Device, mapped at their physical address), 2 the memory the GPU and this process share (`gpu-mem`, Normal non-cacheable), 3 the vault scope `gpu.` (the firmware, `gpu.fw`).
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console, exit, store_get_at};

const S_REGS: u64 = 1;
const S_MEM: u64 = 2;
const S_STORE: u64 = 3;

/// A console line in a fixed buffer.
struct Line([u8; 200], usize);
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        let n = b.len().min(self.0.len() - self.1);
        self.0[self.1..self.1 + n].copy_from_slice(&b[..n]);
        self.1 += n;
        Ok(())
    }
}

macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line([0; 200], 0);
        let _ = write!(l, $($t)*);
        console($g, &l.0[..l.1]);
    }};
}

/// The GPU's registers, at the address `gpu-regs` is mapped at.
struct Gpu {
    regs: usize,
}

impl Gpu {
    fn rd(&self, off: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.regs + off) as *const u32) }
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let regs = ferros_sys::dma(cap(grants, S_REGS)) as usize;
    let mem = ferros_sys::dma(cap(grants, S_MEM)) as usize;
    let gpu = Gpu { regs };
    say!(grants, "gpu: registers at G#{regs:X}, memory at G#{mem:X}\n");
    say!(grants, "gpu: GPU_ID G#{:X} L2_FEATURES G#{:X} SHADER_PRESENT G#{:X} AS_PRESENT G#{:X} MCU_STATUS G#{:X}\n", gpu.rd(0), gpu.rd(4), gpu.rd(0x100), gpu.rd(0x18), gpu.rd(0x704));
    let mut head = [0u8; 32];
    match store_get_at(cap(grants, S_STORE), b"fw", 0, &mut head) {
        Some(n) => say!(grants, "gpu: firmware {n} bytes, magic G#{:X}\n", u32::from_le_bytes([head[0], head[1], head[2], head[3]])),
        None => say!(grants, "gpu: no gpu.fw in the vault\n"),
    }
    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
