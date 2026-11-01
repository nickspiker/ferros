//! The USB device driver as a process (USB.md: USB out of the kernel): the DWC3 controller on HSI0, driven through its registers mapped for EL0 (`usb-regs`) and DMA memory it owns (`usb-dma`), woken by its interrupt (`usb-dwc3`), serving Photon Transport to the bridge. Being built step by step; for now the input `p` probes the grants and exits.
//!
//! Spawn: ferros-bridge spawn usb.bin p --name usb --dma usb-regs --dma usb-dma --irq usb-dwc3 --pt --power usb
//! Grant slots, in that order: 0 console, 1 usb-regs, 2 usb-dma, 3 the interrupt, 4 the kernel's PT commands, 5 the `usb` power switch.
//!
//! Inputs: `p` probes the grants; `d` hands the kernel a plain `diag` through the PT endpoint (USB.md step 3) and prints the reply's length and its first lines. `h` takes the controller from the kernel for 2 s and gives it back; `k` takes it and exits, for the kernel to take it back from a driver that is gone.
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console};

const S_REGS: u64 = 1;
const S_DMA: u64 = 2;
const S_PT: u64 = 4;
const S_POWER: u64 = 5;

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
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let input = unsafe { core::slice::from_raw_parts((ferros_sys::arc_base() + input) as *const u8, input_len as usize) };
    if input.first() == Some(&b'd') {
        diag(grants);
    }
    if let Some(&m @ (b'h' | b'k')) = input.first() {
        let on = ferros_sys::power(cap(grants, S_POWER), true);
        say!(grants, "usb: controller taken: {on}\n");
        if m == b'h' {
            ferros_sys::sleep_until(ferros_sys::now() + 2 * ferros_sys::freq());
            let off = ferros_sys::power(cap(grants, S_POWER), false);
            say!(grants, "usb: controller given back: {off}\n");
        }
        ferros_sys::exit(0);
    }
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

/// `d`: a plain `diag` through the kernel's PT endpoint, timed, its reply's length and first lines printed.
fn diag(grants: u64) -> ! {
    let pt = cap(grants, S_PT);
    let mut cmd = [0u8; 64];
    let n = ferros_pt::command::encode(&mut cmd, &ferros_pt::command::dev_cap(ferros_pt::command::caps::DIAG), ferros_pt::Op::Read, b"");
    let t0 = ferros_sys::now();
    if !ferros_sys::pt_submit(pt, &cmd[..n], false) {
        say!(grants, "usb: the kernel refused the command\n");
        ferros_sys::exit(1);
    }
    let mut buf = [0u8; 4096];
    loop {
        match ferros_sys::pt_take(pt, 0, &mut buf) {
            ferros_sys::PtReply::Running => ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 1000),
            ferros_sys::PtReply::None => {
                say!(grants, "usb: no command in\n");
                ferros_sys::exit(1);
            }
            ferros_sys::PtReply::Done(total, act) => {
                let us = ferros_sys::us_since(t0);
                say!(grants, "usb: diag reply A#{total} bytes in A#{us} us, act {act}\n");
                // The kernel copied at most the buffer: the reply can be longer.
                let shown = total.min(buf.len());
                let end = buf[..shown].iter().enumerate().filter(|(_, c)| **c == b'\n').nth(7).map_or(shown, |(i, _)| i + 1);
                ferros_sys::console(grants, &buf[..end]);
                ferros_sys::exit(0);
            }
        }
    }
}
