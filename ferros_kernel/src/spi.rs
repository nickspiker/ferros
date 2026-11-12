//! The HSI0 USI SPI ports' clock paths: what the touch, GPS and amplifier power switches open before their drivers run the port (KERNEL-OUT.md move 7). The CMU and the USI mode word are shared by every port, so they stay the kernel's; the controller itself is set up and run by ferros_spi, the same code the drivers use thru their register grants (payloads/touch, payloads/gps, payloads/amp).
//!
//! Each port is clocked from the 24.576 MHz oscillator (÷1; the controller divides by 4 inside: about 6.1 MHz on the wire), its USI switched to SPI in sysreg_hsi0, its USI reset released, and set up as `s3c64xx_spi_hwinit` does.

use core::ptr::{read_volatile, write_volatile};

/// One port and the clock path in front of it (cmucal-sfr.c; usi-offset from zuma-usi.dtsi).
pub struct Port {
    pub base: usize,
    pub usi_mode: usize,
    pub mux: usize,
    pub div: usize,
    pub gates: [usize; 4],
}

const CMU_HSI0: usize = 0x1100_0000;
const SYSREG_HSI0: usize = 0x1102_0000;
/// spi_20, HSI0 USI2: the touch controller.
pub const TOUCH: Port = Port { base: 0x111D_0000, usi_mode: SYSREG_HSI0 + 0x101C, mux: CMU_HSI0 + 0x101C, div: CMU_HSI0 + 0x181C, gates: [CMU_HSI0 + 0x2120, CMU_HSI0 + 0x20FC, CMU_HSI0 + 0x2100, CMU_HSI0 + 0x20BC] };
/// spi_19, HSI0 USI1: the two CS35L41 speaker amplifiers (CS0 native, CS1 = gpn1-0).
pub const AMPS: Port = Port { base: 0x111C_0000, usi_mode: SYSREG_HSI0 + 0x1018, mux: CMU_HSI0 + 0x1018, div: CMU_HSI0 + 0x1818, gates: [CMU_HSI0 + 0x211C, CMU_HSI0 + 0x20F4, CMU_HSI0 + 0x20F8, CMU_HSI0 + 0x20B0] };
/// spi_21, HSI0 USI3: the GPS chip (BCM4776; zuma-board-gps.dtsi: mode 3, feedback delay 1, 26 MHz at most; cmucal-sfr.c for the clock path).
pub const GPS: Port = Port { base: 0x111E_0000, usi_mode: SYSREG_HSI0 + 0x1020, mux: CMU_HSI0 + 0x1020, div: CMU_HSI0 + 0x1820, gates: [CMU_HSI0 + 0x2124, CMU_HSI0 + 0x2104, CMU_HSI0 + 0x2108, CMU_HSI0 + 0x20D0] };

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}

/// A port's registers at their physical addresses, for ferros_spi.
struct Phys(usize);
impl ferros_spi::Regs for Phys {
    fn rd(&self, off: usize) -> u32 {
        rd(self.0 + off)
    }
    fn wr(&self, off: usize, v: u32) {
        wr(self.0 + off, v)
    }
    fn now(&self) -> u64 {
        crate::platform::cntpct()
    }
    fn freq(&self) -> u64 {
        crate::platform::cntfrq()
    }
}

/// A CMU gate is closed only when manual control is on (bit 20) and its value (bit 21) is off.
pub fn gate_open(v: u32) -> bool {
    !(v & (1 << 20) != 0 && v & (1 << 21) == 0)
}

impl Port {
    /// Every gate on the port's clock path open (its registers are safe to touch).
    pub fn clocked(&self) -> bool {
        self.gates.iter().all(|&a| gate_open(rd(a)))
    }

    /// Clock from the oscillator, SPI mode, then the controller's own setup (ferros_spi `init`: USI reset released, `s3c64xx_spi_hwinit`'s registers).
    pub fn up(&self) {
        wr(self.mux, (rd(self.mux) & !1) | 1);
        wr(self.div, rd(self.div) & !0xF);
        for a in self.gates {
            wr(a, rd(a) | 1 << 20 | 1 << 21);
        }
        wr(self.usi_mode, (rd(self.usi_mode) & !7) | 2);
        ferros_spi::Spi { regs: Phys(self.base), mode: 0, fb: 0 }.init();
    }

    /// The port's clock halved (the CMU divider's field holds the ratio less one): 12.288 MHz into the controller, about 3.07 MHz on the wire. The amplifiers' OTP is read at 4 MHz or less (cs35l41.h `CS35L41_SPI_MAX_FREQ_OTP`, cs35l41.c `cs35l41_otp_unpack`), and the audio service reads it at every start; their other traffic is a few hundred words, so the whole port runs slow rather than switching (docs/AUDIO.md).
    pub fn halved(&self) {
        wr(self.div, (rd(self.div) & !0xF) | 1);
    }
}
