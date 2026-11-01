//! The USB device driver as a process (USB.md: USB out of the kernel): the DWC3 controller on HSI0, driven through its registers mapped for EL0 (`usb-regs`) and DMA memory it owns (`usb-dma`), woken by its interrupt (`usb-dwc3`), serving Photon Transport to the bridge. Being built step by step; for now the input `p` probes the grants and exits.
//!
//! Spawn: ferros-bridge spawn usb.bin p --name usb --dma usb-regs --dma usb-dma --irq usb-dwc3
//! Grant slots, in that order: 0 console, 1 usb-regs, 2 usb-dma, 3 the interrupt.
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console};

const S_REGS: u64 = 1;
const S_DMA: u64 = 2;

/// GSNPSID: the controller's ID and release (G#5533xxxx for a DWC_usb3).
const GSNPSID: usize = 0xC120;

struct Line {
    b: [u8; 256],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line { b: [0; 256], n: 0 };
        let _ = write!(l, $($t)*);
        console($g, &l.b[..l.n]);
    }};
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let regs = ferros_sys::dma(cap(grants, S_REGS)) as usize;
    let dma = ferros_sys::dma(cap(grants, S_DMA)) as usize;
    say!(grants, "usb: regs at G#{regs:X}, DMA at G#{dma:X}\n");
    // The controller's ID, read straight from its registers at EL0.
    let id = unsafe { core::ptr::read_volatile((regs + GSNPSID) as *const u32) };
    // A word written and read back at the far end of the DMA region.
    let probe = (dma + (2 << 20) - 8) as *mut u64;
    unsafe { core::ptr::write_volatile(probe, 0x5553_4244_4D41_0001) };
    let back = unsafe { core::ptr::read_volatile(probe) };
    say!(grants, "usb: GSNPSID G#{id:08X}; DMA word back G#{back:X}\n");
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
