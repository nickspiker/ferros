//! tecount: whether the panel's TE reaches the chip — the TE pin (gpp0-3, GPP0_DAT bit 3) sampled for a quarter second, its rising edges counted and its high time summed, with the pin's function from GPP0_CON. A running panel at 120 Hz gives about 30 edges. For the jump's stalled display (2026-11-04: DECON started one frame and no more): `ferros-bridge spawn tecount.bin --hw gpio-gpp0`. Grant slots: 0 console, 1 the GPIO bank.
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console, exit, freq, mmio_read, now};

struct Line([u8; 160], usize);
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        let n = b.len().min(self.0.len() - self.1);
        self.0[self.1..self.1 + n].copy_from_slice(&b[..n]);
        self.1 += n;
        Ok(())
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let bank = cap(grants, 1);
    let con = mmio_read(bank, 0);
    let (t0, span) = (now(), freq() / 4);
    let (mut edges, mut samples, mut high, mut last) = (0u32, 0u32, 0u32, mmio_read(bank, 4) >> 3 & 1);
    while now().wrapping_sub(t0) < span {
        let pin = mmio_read(bank, 4) >> 3 & 1;
        samples += 1;
        high += pin as u32;
        if pin == 1 && last == 0 {
            edges += 1;
        }
        last = pin;
    }
    let mut l = Line([0; 160], 0);
    let _ = write!(l, "tecount: TE pin function {}, {} rising edges in 250 ms, high in {} of {} samples\n", con >> 12 & 0xF, edges, high, samples);
    console(grants, &l.0[..l.1]);
    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
