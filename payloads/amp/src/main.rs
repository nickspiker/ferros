//! The speaker amplifiers' control side (AUDIO.md, KERNEL-OUT.md move 7): two Cirrus CS35L41 on spi_19 (HSI0 USI1), the left on the controller's own select, the right on gpn1-0. Audio itself reaches them over TDM from the always-on DSP (AoC); this is only their registers, until audio is built. What was the kernel's `diag amp state|up` (amp.rs), as a process.
//!
//! The kernel keeps what a process may not do behind the `amp` power switch (kernel amp.rs `power`): the port's clocks and USI mode (the HSI0 CMU every port shares), the sub PMIC's buck A (VA) over ACPM, the pins' modes, and both resets pulsed (the right's, gph2-1, sits in HSI1's bank, the modem's). Here, thru register grants: the port, the right's select (gpn1's data bit; touch drives only gpn0's registers in that block), and the left's reset data bit (gpp1-1; the display and GPS drive other banks of that block), held low only to prove the right answers on its own select.
//!
//! From Google's driver (private/google-modules/amplifiers/cs35l41) and the husky DT (zuma-shusky-audio.dtsi): registers are 32-bit, addressed by a 32-bit word with bit 31 set for a read, then 16 bits of padding, then the 32-bit value big-endian. Out of reset the chip boots from its OTP and says so in IRQ1_STATUS4; then the ID registers (DEVID G#35A40, or G#35B40 for the R part).
//!
//! A development tool for now (nothing plays sound): started by hand, not by init.
//! Spawn: ferros-bridge spawn amp.bin --name amp --serve amp --hw usi1-spi --hw gpio-gpn0 --hw gpio-gpp0 --power amp
//! Grant slots, in that order: 0 console, 1 the `amp` endpoint (serve), 2 the SPI port, 3 gpio-gpn0 (the right's select), 4 gpio-gpp0 (the left's reset), 5 the power switch.
//!
//! At its start: the switch on, each amplifier's OTP boot awaited (up to 128 ms), its IDs, then the right read with the left held in reset, all to the console. Then the `amp` endpoint, first byte the op:
//! - `0` STATE: both DEVIDs read now, u32 LE each (left, then right; 0 for a read that failed), and a line on the console.
//! - `1` READ + amp (0 left, 1 right) + register (u32 LE): its value, u32 LE; nothing when the transfer failed.
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_spi::{Cs, Regs, Spi};
use ferros_sys::{cap, console, freq, mmio_read, mmio_write, now, recv, reply, sleep_until, ERR};

const S_AMP: u64 = 1;
const S_SPI: u64 = 2;
const S_GPN: u64 = 3;
const S_GPP: u64 = 4;
const S_POWER: u64 = 5;

/// gpn1's data word in gpio-gpn0 (the bank at +G#20): pin 0 is the right amplifier's select, active low.
const GPN1_DAT: u64 = 0x24;
const RIGHT_CS: u32 = 0;
/// gpp1's data word in gpio-gpp0 (the bank at +G#20): pin 1 is the left amplifier's reset, active low.
const GPP1_DAT: u64 = 0x24;
const LEFT_RESET: u32 = 1;

const DEVID: u32 = 0x0;
const REVID: u32 = 0x4;
const FABID: u32 = 0x8;
const IRQ1_STATUS4: u32 = 0x1_001C;
const OTP_BOOT_DONE: u32 = 0x2;
/// Polls for OTP_BOOT_DONE, a millisecond apart: Android's reset path waits a few.
const BOOT_POLLS: u32 = 1 << 7;

struct Line {
    b: [u8; 512],
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
        let mut l = Line { b: [0; 512], n: 0 };
        let _ = write!(l, $($t)*);
        console($g, &l.b[..l.n]);
    }};
}

/// The port thru its hardware grant.
struct Grant(u64);
impl Regs for Grant {
    fn rd(&self, off: usize) -> u32 {
        mmio_read(self.0, off as u64) as u32
    }
    fn wr(&self, off: usize, v: u32) {
        mmio_write(self.0, off as u64, v);
    }
    fn now(&self) -> u64 {
        now()
    }
    fn freq(&self) -> u64 {
        freq()
    }
}

fn sleep_us(us: u64) {
    sleep_until(now() + freq() / 1_000_000 * us);
}

/// One data bit of a granted GPIO bank, its mode the kernel's.
fn pin(grants: u64, slot: u64, dat: u64, n: u32, high: bool) {
    let c = cap(grants, slot);
    let v = mmio_read(c, dat) as u32;
    mmio_write(c, dat, if high { v | 1 << n } else { v & !(1 << n) });
}

struct Amps {
    grants: u64,
    spi: Spi<Grant>,
}

impl Amps {
    /// One register of the left (0) or right (1) amplifier.
    fn read(&self, amp: u8, reg: u32) -> Result<u32, &'static str> {
        let r = (reg | 0x8000_0000).to_be_bytes();
        let mut v = [0u8; 4];
        let g = self.grants;
        let right = |sel: bool| pin(g, S_GPN, GPN1_DAT, RIGHT_CS, !sel);
        let cs = if amp == 0 { Cs::Native } else { Cs::Pin(&right) };
        self.spi.transact(cs, &[r[0], r[1], r[2], r[3], 0, 0], &mut v)?;
        Ok(u32::from_be_bytes(v))
    }

    fn id(&self, amp: u8, reg: u32, l: &mut Line) {
        let _ = match self.read(amp, reg) {
            Ok(v) => write!(l, "G#{v:X}"),
            Err(e) => write!(l, "{e}"),
        };
    }

    /// Out of reset (the switch's pulse): OTP_BOOT_DONE awaited, then DEVID, REVID, FABID.
    fn boot(&self, amp: u8) {
        let name = if amp == 0 { "left" } else { "right" };
        let t0 = now();
        let mut boot = None;
        for _ in 0..BOOT_POLLS {
            match self.read(amp, IRQ1_STATUS4) {
                Ok(s) if s & OTP_BOOT_DONE != 0 => {
                    boot = Some(s);
                    break;
                }
                Ok(_) => sleep_us(1000),
                Err(e) => {
                    say!(self.grants, "amp: {name}: SPI {e}\n");
                    break;
                }
            }
        }
        let ms = now().wrapping_sub(t0) * 1000 / freq();
        match boot {
            Some(s) => say!(self.grants, "amp: {name}: OTP boot done after A#{ms} ms (IRQ1_STATUS4 G#{s:X})\n"),
            None => say!(self.grants, "amp: {name}: no OTP_BOOT_DONE in A#{ms} ms\n"),
        }
        let mut l = Line { b: [0; 512], n: 0 };
        let _ = write!(l, "amp: {name}: DEVID ");
        self.id(amp, DEVID, &mut l);
        let _ = write!(l, " (CS35L41 is G#35A40, R part G#35B40) REVID ");
        self.id(amp, REVID, &mut l);
        let _ = write!(l, " FABID ");
        self.id(amp, FABID, &mut l);
        let _ = writeln!(l);
        console(self.grants, &l.b[..l.n]);
    }

    /// Both answer with the same IDs: the right proved to answer on its own select by holding the left in reset while both are read.
    fn alone(&self) {
        pin(self.grants, S_GPP, GPP1_DAT, LEFT_RESET, false);
        sleep_us(2100);
        let mut l = Line { b: [0; 512], n: 0 };
        let _ = write!(l, "amp: with the left held in reset: right DEVID ");
        self.id(1, DEVID, &mut l);
        let _ = write!(l, ", left DEVID ");
        self.id(0, DEVID, &mut l);
        let _ = writeln!(l, " (expect the right to answer and the left not to)");
        pin(self.grants, S_GPP, GPP1_DAT, LEFT_RESET, true);
        sleep_us(2100);
        console(self.grants, &l.b[..l.n]);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    if !ferros_sys::power(cap(grants, S_POWER), true) {
        say!(grants, "amp: power on failed (the port's clocks gated, buck A, or HSI1 off)\n");
        ferros_sys::exit(1);
    }
    let amps = Amps { grants, spi: Spi { regs: Grant(cap(grants, S_SPI)), mode: 0, fb: 0 } };
    amps.boot(0);
    amps.boot(1);
    amps.alone();
    let mut msg = [0u8; 64];
    let mut token = [0u8; 32];
    loop {
        let n = recv(cap(grants, S_AMP), &mut msg, &mut token, false);
        if n == ERR || n == 0 {
            continue;
        }
        match msg[0] {
            0 => {
                let (l, r) = (amps.read(0, DEVID).unwrap_or(0), amps.read(1, DEVID).unwrap_or(0));
                say!(grants, "amp: DEVID left G#{l:X}, right G#{r:X}\n");
                let mut out = [0u8; 8];
                out[..4].copy_from_slice(&l.to_le_bytes());
                out[4..].copy_from_slice(&r.to_le_bytes());
                reply(&token, &out);
            }
            1 if n == 6 && msg[1] < 2 => match amps.read(msg[1], u32::from_le_bytes([msg[2], msg[3], msg[4], msg[5]])) {
                Ok(v) => {
                    reply(&token, &v.to_le_bytes());
                }
                Err(e) => {
                    say!(grants, "amp: read: {e}\n");
                    reply(&token, &[]);
                }
            },
            _ => {
                reply(&token, &[]);
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
